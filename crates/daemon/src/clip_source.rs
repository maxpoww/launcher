//! Native multi-type clipboard source (`wlr-data-control`).
//!
//! `wl-copy` can only own the selection with a *single* content-type at a time,
//! so a file clip restored from history offered only `text/uri-list` — which
//! thunar (and other file managers) ignore for pasting files, so paste did
//! nothing. To behave like a real Ctrl+C/X/V we must advertise every relevant
//! type *simultaneously*: `x-special/gnome-copied-files` (file managers, carries
//! the copy/cut verb), `text/uri-list` (GTK choosers, browsers), and
//! `text/plain*` (editors, terminals).
//!
//! The only way to do that is to own the selection with a real data source, so
//! this module runs a tiny clipboard-manager on its own Wayland connection and
//! thread (independent of the render loop, exactly as `wl-copy` runs
//! out-of-process): it binds `zwlr_data_control_manager_v1`, creates a
//! `zwlr_data_control_source_v1` that `offer()`s each type, and answers each
//! `send(mime, fd)` with that type's bytes. [`spawn`] returns `None` when the
//! compositor lacks the protocol, and the caller falls back to `wl-copy`.

use std::io::Write;
use std::time::{Duration, Instant};

use calloop::EventLoop;
use calloop_wayland_source::WaylandSource;
use tracing::{debug, warn};

use crate::clipboard::ClipEvent;

/// After serving a clip, ignore reads for this long: our own `wl-paste` capture
/// re-reads the fresh selection, and that must not count as a user paste.
const SELF_READ_IGNORE: Duration = Duration::from_millis(1200);
/// A single user paste often reads several MIME types back-to-back; collapse
/// reads within this window into one recorded paste.
const PASTE_DEBOUNCE: Duration = Duration::from_millis(400);
use wayland_client::globals::{registry_queue_init, GlobalListContents};
use wayland_client::protocol::wl_registry::{self, WlRegistry};
use wayland_client::protocol::wl_seat::{self, WlSeat};
use wayland_client::{event_created_child, Connection, Dispatch, QueueHandle};
use wayland_protocols_wlr::data_control::v1::client::zwlr_data_control_device_v1::{
    self, ZwlrDataControlDeviceV1, EVT_DATA_OFFER_OPCODE,
};
use wayland_protocols_wlr::data_control::v1::client::zwlr_data_control_manager_v1::ZwlrDataControlManagerV1;
use wayland_protocols_wlr::data_control::v1::client::zwlr_data_control_offer_v1::{
    self, ZwlrDataControlOfferV1,
};
use wayland_protocols_wlr::data_control::v1::client::zwlr_data_control_source_v1::{
    self, ZwlrDataControlSourceV1,
};

/// One clip's full offer set: `(mime, bytes)` for every type it advertises.
type Offers = Vec<(String, Vec<u8>)>;

/// Handle to the clipboard-source thread. Send it a clip's [`Offers`] to make it
/// the current selection.
pub struct ClipSource {
    tx: calloop::channel::Sender<Offers>,
}

impl ClipSource {
    /// Own the selection, advertising every `(mime, bytes)` in `offers`.
    pub fn serve(&self, offers: Offers) {
        if let Err(e) = self.tx.send(offers) {
            warn!("clip-source: worker gone, dropping serve: {e}");
        }
    }
}

/// Start the clipboard-source thread. Returns `None` (and logs) if a Wayland
/// connection or the `wlr-data-control` protocol is unavailable, so the caller
/// can fall back to `wl-copy`. `events` carries [`ClipEvent::Pasted`] back when
/// an app reads what we serve.
pub fn spawn(events: calloop::channel::Sender<ClipEvent>) -> Option<ClipSource> {
    let (tx, rx) = calloop::channel::channel::<Offers>();
    let (ready_tx, ready_rx) = std::sync::mpsc::channel();
    std::thread::Builder::new()
        .name("clip-source".into())
        .spawn(move || run(rx, ready_tx, events))
        .expect("spawn clip-source thread");
    // Block briefly until the thread reports whether it bound the protocol, so
    // the fallback decision is made once, up front.
    match ready_rx.recv() {
        Ok(true) => Some(ClipSource { tx }),
        _ => None,
    }
}

/// State driven by the clipboard-source event loop.
struct State {
    manager: ZwlrDataControlManagerV1,
    device: ZwlrDataControlDeviceV1,
    qh: QueueHandle<State>,
    conn: Connection,
    /// The source currently owning the selection (if any).
    source: Option<ZwlrDataControlSourceV1>,
    /// The bytes to hand back per requested MIME, for the current source.
    offers: Offers,
    /// Paste reporting.
    events: calloop::channel::Sender<ClipEvent>,
    /// When we last set the selection (to ignore our own capture re-read).
    last_serve: Option<Instant>,
    /// When we last reported a paste (to debounce multi-type reads).
    last_paste: Option<Instant>,
}

impl State {
    /// Replace the current selection with a fresh source advertising `offers`.
    fn serve(&mut self, offers: Offers) {
        if offers.is_empty() {
            return;
        }
        if let Some(old) = self.source.take() {
            old.destroy();
        }
        let source = self.manager.create_data_source(&self.qh, ());
        for (mime, _) in &offers {
            source.offer(mime.clone());
        }
        self.device.set_selection(Some(&source));
        self.source = Some(source);
        self.offers = offers;
        self.last_serve = Some(Instant::now());
        if let Err(e) = self.conn.flush() {
            warn!("clip-source: flush failed: {e}");
        }
    }

    /// A read of our selection arrived. Count it as a user paste unless it's our
    /// own post-serve capture re-read or a debounced multi-type burst.
    fn note_read(&mut self) {
        let now = Instant::now();
        let self_read = self
            .last_serve
            .is_some_and(|t| now.duration_since(t) < SELF_READ_IGNORE);
        let debounced = self
            .last_paste
            .is_some_and(|t| now.duration_since(t) < PASTE_DEBOUNCE);
        if self_read || debounced {
            return;
        }
        self.last_paste = Some(now);
        let _ = self.events.send(ClipEvent::Pasted);
    }
}

fn run(
    rx: calloop::channel::Channel<Offers>,
    ready: std::sync::mpsc::Sender<bool>,
    events: calloop::channel::Sender<ClipEvent>,
) {
    macro_rules! bail {
        ($($arg:tt)*) => {{
            warn!($($arg)*);
            let _ = ready.send(false);
            return;
        }};
    }

    let conn = match Connection::connect_to_env() {
        Ok(c) => c,
        Err(e) => bail!("clip-source: no wayland connection: {e}"),
    };
    let (globals, event_queue) = match registry_queue_init::<State>(&conn) {
        Ok(v) => v,
        Err(e) => bail!("clip-source: registry init failed: {e}"),
    };
    let qh = event_queue.handle();

    let manager: ZwlrDataControlManagerV1 = match globals.bind(&qh, 1..=1, ()) {
        Ok(m) => m,
        Err(e) => bail!("clip-source: wlr-data-control unavailable ({e}); using wl-copy fallback"),
    };
    let seat: WlSeat = match globals.bind(&qh, 1..=8, ()) {
        Ok(s) => s,
        Err(e) => bail!("clip-source: no wl_seat: {e}"),
    };
    let device = manager.get_data_device(&seat, &qh, ());

    let mut state = State {
        manager,
        device,
        qh: qh.clone(),
        conn: conn.clone(),
        source: None,
        offers: Vec::new(),
        events,
        last_serve: None,
        last_paste: None,
    };

    let mut event_loop: EventLoop<State> = match EventLoop::try_new() {
        Ok(el) => el,
        Err(e) => bail!("clip-source: event loop init failed: {e}"),
    };
    let handle = event_loop.handle();
    if let Err(e) = WaylandSource::new(conn.clone(), event_queue).insert(handle.clone()) {
        bail!("clip-source: insert wayland source failed: {e}");
    }
    if let Err(e) = handle.insert_source(rx, |event, _, state: &mut State| {
        if let calloop::channel::Event::Msg(offers) = event {
            state.serve(offers);
        }
    }) {
        bail!("clip-source: insert command channel failed: {e}");
    }

    let _ = ready.send(true);
    let timeout: Option<Duration> = None;
    if let Err(e) = event_loop.run(timeout, &mut state, |_| {}) {
        warn!("clip-source: event loop exited: {e}");
    }
}

// ---- Dispatch glue ----
//
// We only ever *serve* the selection, so every incoming event is ignored except
// a source's `send` (write the bytes) and `cancelled` (we lost ownership).

impl Dispatch<WlRegistry, GlobalListContents> for State {
    fn event(
        _: &mut Self,
        _: &WlRegistry,
        _: wl_registry::Event,
        _: &GlobalListContents,
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
    }
}

impl Dispatch<WlSeat, ()> for State {
    fn event(
        _: &mut Self,
        _: &WlSeat,
        _: wl_seat::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
    }
}

impl Dispatch<ZwlrDataControlManagerV1, ()> for State {
    fn event(
        _: &mut Self,
        _: &ZwlrDataControlManagerV1,
        _: <ZwlrDataControlManagerV1 as wayland_client::Proxy>::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
    }
}

impl Dispatch<ZwlrDataControlDeviceV1, ()> for State {
    fn event(
        _: &mut Self,
        _: &ZwlrDataControlDeviceV1,
        event: zwlr_data_control_device_v1::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        // Incoming offers/selections aren't our concern (we serve, never read).
        // Destroy any offer object handed to us so it doesn't leak.
        if let zwlr_data_control_device_v1::Event::DataOffer { id } = event {
            id.destroy();
        }
    }

    event_created_child!(State, ZwlrDataControlDeviceV1, [
        EVT_DATA_OFFER_OPCODE => (ZwlrDataControlOfferV1, ()),
    ]);
}

impl Dispatch<ZwlrDataControlOfferV1, ()> for State {
    fn event(
        _: &mut Self,
        _: &ZwlrDataControlOfferV1,
        _: zwlr_data_control_offer_v1::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
    }
}

impl Dispatch<ZwlrDataControlSourceV1, ()> for State {
    fn event(
        state: &mut Self,
        source: &ZwlrDataControlSourceV1,
        event: zwlr_data_control_source_v1::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        match event {
            // A paster asked for one of our offered types: write its bytes to
            // the pipe on a detached thread (an image can be large; never block
            // the event loop) and let the fd close on drop.
            zwlr_data_control_source_v1::Event::Send { mime_type, fd } => {
                match state.offers.iter().find(|(m, _)| *m == mime_type) {
                    Some((_, bytes)) => {
                        let bytes = bytes.clone();
                        std::thread::spawn(move || {
                            let mut file = std::fs::File::from(fd);
                            if let Err(e) = file.write_all(&bytes) {
                                debug!("clip-source: send write failed: {e}");
                            }
                        });
                        // A read of our selection = a (best-effort) paste.
                        state.note_read();
                    }
                    None => drop(fd), // unknown type: close the pipe empty
                }
            }
            // Another client took the selection: drop our source.
            zwlr_data_control_source_v1::Event::Cancelled => {
                source.destroy();
                if state.source.as_ref() == Some(source) {
                    state.source = None;
                    state.offers.clear();
                }
            }
            _ => {}
        }
    }
}
