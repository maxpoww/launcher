//! Layer 4 (part) — printing, from CUPS.
//!
//! What the printers are and what they are doing, and every print that is
//! waiting or printing — asked of CUPS over IPP ([`crate::cups`]) whenever
//! CUPS says something changed. It says so on the system bus: a subscription
//! (renewed before its lease ends) makes the scheduler's `dbus` notifier
//! announce every print created, progressing or ending and every printer
//! changing state, and each announcement here is one fresh look. A look also
//! comes on a slow timer, and a quick one while a print is running, so a
//! missed signal — or no bus at all — only makes it slower, never wrong.
//!
//! A print that leaves CUPS's list of unfinished prints is asked for once
//! more, to know how it ended (printed, cancelled, failed), and stays in the
//! state for [`ENDED_KEEP`] so a surface can say so.
//!
//! # Why its own layer
//!
//! "No printers" is knowledge only when CUPS says it. A machine without CUPS
//! (Golem's lean build) or a scheduler that does not answer leaves
//! [`Layer::Printing`] dark instead.

use std::collections::BTreeSet;
use std::pin::Pin;
use std::time::{Duration, Instant};

use futures_core::Stream;
use tokio::sync::{mpsc, watch};
use zbus::{Connection, MatchRule, MessageStream};

use crate::collector::{Collector, CollectorFuture};
use crate::cups;
use crate::message::{ContextDelta, Update};
use crate::state::{ContextState, Layer, PrintJob, PrinterState, PrintingState};

/// No scheduler socket: looked for again after this.
const ABSENT: Duration = Duration::from_secs(60);
/// The scheduler did not answer: asked again after this.
const RETRY: Duration = Duration::from_secs(10);
/// A print running, with CUPS's signals heard: a safety look this often.
const ACTIVE: Duration = Duration::from_secs(3);
/// A print running and no signals: this often.
const ACTIVE_BLIND: Duration = Duration::from_millis(1500);
/// Nothing running, signals heard.
const IDLE: Duration = Duration::from_secs(60);
/// Nothing running, no signals.
const IDLE_BLIND: Duration = Duration::from_secs(10);
/// A burst of signals (a page and a state change together) is one look.
const SETTLE: Duration = Duration::from_millis(150);
/// How long a print that ended stays in the state.
pub const ENDED_KEEP: Duration = Duration::from_secs(60);
/// The subscription's lease, and how long before its end it is renewed.
const LEASE: u32 = 3600;
const RENEW_BEFORE: Duration = Duration::from_secs(300);

#[derive(Default)]
pub struct PrintingCollector;

impl PrintingCollector {
    pub fn new() -> Self {
        Self
    }
}

impl Collector for PrintingCollector {
    fn name(&self) -> &'static str {
        "printing"
    }
    fn layer(&self) -> Layer {
        Layer::Printing
    }
    fn run(
        self: Box<Self>,
        _ctx: watch::Receiver<ContextState>,
        tx: mpsc::Sender<Update>,
    ) -> CollectorFuture {
        Box::pin(async move {
            loop {
                if !cups::present() {
                    tokio::time::sleep(ABSENT).await;
                    continue;
                }
                let mut events = Events::start().await;
                let mut seen = Seen::default();
                let mut last: Option<PrintingState> = None;
                loop {
                    let state = match blocking(cups::snapshot).await {
                        Ok(s) => seen.fold(s).await,
                        Err(e) => {
                            tracing::debug!("printing: CUPS did not answer: {e}");
                            break;
                        }
                    };
                    let busy = running(&state);
                    let fading = state.jobs.iter().any(|j| j.state.ended());
                    if last.as_ref() != Some(&state) {
                        last = Some(state.clone());
                        if tx
                            .send(Update::Delta(
                                Layer::Printing,
                                ContextDelta::Printing(state),
                            ))
                            .await
                            .is_err()
                        {
                            events.end().await;
                            return Ok(()); // aggregator gone
                        }
                    }
                    let wait = match (busy || fading, events.listening()) {
                        (true, true) => ACTIVE,
                        (true, false) => ACTIVE_BLIND,
                        (false, true) => IDLE,
                        (false, false) => IDLE_BLIND,
                    };
                    events.wait(wait).await;
                }
                // CUPS went away: dark, rather than its last answer standing.
                events.end().await;
                let _ = tx.send(Update::Health(Layer::Printing, false)).await;
                tokio::time::sleep(RETRY).await;
            }
        })
    }
}

/// Whether anything is printing or waiting to.
fn running(s: &PrintingState) -> bool {
    s.jobs.iter().any(|j| !j.state.ended())
        || s.printers.iter().any(|p| p.state == PrinterState::Printing)
}

/// Run a blocking CUPS call off the async thread.
async fn blocking<T: Send + 'static>(
    f: impl FnOnce() -> Result<T, cups::CupsError> + Send + 'static,
) -> Result<T, cups::CupsError> {
    tokio::task::spawn_blocking(f)
        .await
        .unwrap_or_else(|e| Err(cups::CupsError::Refused(format!("worker failed: {e}"))))
}

/// The prints seen unfinished, and those that just ended (with when).
#[derive(Default)]
struct Seen {
    open: BTreeSet<u32>,
    ended: Vec<(PrintJob, Instant)>,
}

impl Seen {
    /// Add to a fresh snapshot how each print that left the list ended.
    async fn fold(&mut self, mut state: PrintingState) -> PrintingState {
        let now: BTreeSet<u32> = state.jobs.iter().map(|j| j.id).collect();
        for &id in self.open.difference(&now) {
            match blocking(move || cups::job(id)).await {
                Ok(Some(j)) if j.state.ended() => self.ended.push((j, Instant::now())),
                // Not ended after all, or gone with no trace: nothing to say.
                Ok(_) => {}
                Err(e) => tracing::debug!("printing: how print {id} ended: {e}"),
            }
        }
        self.open = now;
        self.ended.retain(|(_, at)| at.elapsed() < ENDED_KEEP);
        merge_ended(&mut state, &self.ended);
        state
    }
}

/// The just-ended prints after the open ones, each once.
fn merge_ended(state: &mut PrintingState, ended: &[(PrintJob, Instant)]) {
    for (j, _) in ended {
        if !state.jobs.iter().any(|o| o.id == j.id) {
            state.jobs.push(j.clone());
        }
    }
}

/// CUPS's signals on the system bus, and the subscription that makes it send
/// them.
struct Events {
    stream: Option<MessageStream>,
    subscription: Option<(i32, Instant)>,
    // Kept for the stream's life.
    _conn: Option<Connection>,
}

impl Events {
    /// Listen first, then subscribe (no signal is lost between the two).
    /// Either failing leaves the timer alone in charge.
    async fn start() -> Events {
        let mut ev = Events {
            stream: None,
            subscription: None,
            _conn: None,
        };
        let listened = async {
            let conn = Connection::system().await?;
            let rule = MatchRule::builder()
                .msg_type(zbus::message::Type::Signal)
                .interface("org.cups.cupsd.Notifier")?
                .build();
            let stream = MessageStream::for_match_rule(rule, &conn, Some(64)).await?;
            Ok::<_, zbus::Error>((conn, stream))
        }
        .await;
        match listened {
            Ok((conn, stream)) => {
                ev.stream = Some(stream);
                ev._conn = Some(conn);
            }
            Err(e) => tracing::debug!("printing: no system bus for CUPS's signals: {e}"),
        }
        if ev.stream.is_some() {
            ev.renew().await;
        }
        ev
    }

    fn listening(&self) -> bool {
        self.stream.is_some() && self.subscription.is_some()
    }

    /// A new subscription, then the old one ended.
    async fn renew(&mut self) {
        match blocking(|| cups::subscribe(LEASE)).await {
            Ok(id) => {
                if let Some((old, _)) = self.subscription.replace((id, Instant::now())) {
                    let _ = blocking(move || cups::unsubscribe(old)).await;
                }
            }
            Err(e) => {
                tracing::debug!("printing: CUPS would not subscribe: {e}");
                self.subscription = None;
            }
        }
    }

    /// Until CUPS says something (and the burst it came in has settled), or
    /// `most` has passed. The subscription is renewed when it is due.
    async fn wait(&mut self, most: Duration) {
        if let Some((_, at)) = self.subscription {
            if at.elapsed() + RENEW_BEFORE >= Duration::from_secs(u64::from(LEASE)) {
                self.renew().await;
            }
        }
        let Some(stream) = self.stream.as_mut() else {
            tokio::time::sleep(most).await;
            return;
        };
        match tokio::time::timeout(most, next(stream)).await {
            Ok(Some(_)) => {
                tokio::time::sleep(SETTLE).await;
                // What came during the settle belongs to this look.
                while let Ok(Some(_)) = tokio::time::timeout(Duration::ZERO, next(stream)).await {}
            }
            Ok(None) => {
                // The bus connection ended: the timer alone from now on.
                self.stream = None;
            }
            Err(_) => {}
        }
    }

    async fn end(&mut self) {
        if let Some((id, _)) = self.subscription.take() {
            let _ = blocking(move || cups::unsubscribe(id)).await;
        }
    }
}

/// The next message of a zbus stream.
async fn next(stream: &mut MessageStream) -> Option<()> {
    std::future::poll_fn(|cx| Pin::new(&mut *stream).poll_next(cx))
        .await
        .map(|_| ())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::state::JobState;

    fn job(id: u32, state: JobState) -> PrintJob {
        PrintJob {
            id,
            state,
            ..Default::default()
        }
    }

    #[test]
    fn a_print_that_ended_follows_the_open_ones_once() {
        let mut s = PrintingState {
            printers: vec![],
            jobs: vec![job(4, JobState::Printing)],
        };
        let ended = vec![
            (job(3, JobState::Completed), Instant::now()),
            (job(4, JobState::Completed), Instant::now()),
        ];
        merge_ended(&mut s, &ended);
        let ids: Vec<u32> = s.jobs.iter().map(|j| j.id).collect();
        // 4 is still open in the fresh list: the open one wins.
        assert_eq!(ids, vec![4, 3]);
        assert_eq!(s.jobs[0].state, JobState::Printing);
    }

    #[test]
    fn running_is_a_print_not_ended_or_a_printer_printing() {
        let mut s = PrintingState::default();
        assert!(!running(&s));
        s.jobs.push(job(1, JobState::Completed));
        assert!(!running(&s));
        s.jobs.push(job(2, JobState::Pending));
        assert!(running(&s));
    }
}
