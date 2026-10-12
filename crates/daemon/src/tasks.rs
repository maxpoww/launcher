//! TASKS: the long things this computer is doing for its owner, each with a
//! name and how far along it is — shown as ONE pill on the OPTIONS bar: what
//! is being done on the left, a bar, the percentage on the right (Max,
//! 2026-10-09: *"a pill on options, with the progress bar… like global: it
//! picks up processes and shows the progress… designed from the beginning
//! with that idea, so I can wire processes later"*).
//!
//! Wiring one in is three lines, wherever the work is done:
//!
//! ```ignore
//! let task = app.task_begin("Importing photos from Pixel 8 Pro"); // on the loop
//! std::thread::spawn(move || {
//!     task.set(done_bytes, total_bytes); // as often as it likes
//!     // …and when `task` is dropped, the work is over.
//! });
//! ```
//!
//! A task can be CANCELLED from its pill: the worker asks `task.cancelled()`
//! wherever it can stop (between two files, each turn of a wait) and stops;
//! one that never asks simply runs on unseen — the pill lets it go at once.
//!
//! The handle is all a worker needs: it is `Send`, it thins its own reports
//! out, and letting go of it is the end of the task — a worker that returns
//! early or panics cannot leave a pill behind. The list lives on the loop
//! (`App::tasks`); this file knows nothing of how the pill is drawn.

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use calloop::channel::Sender;

/// How long a finished task stays, full, before it leaves the bar.
pub(crate) const LINGER: Duration = Duration::from_millis(1400);
/// A worker's reports are passed on no more often than this.
const EVERY: Duration = Duration::from_millis(80);

/// What a worker says of its task.
#[derive(Debug, Clone, PartialEq)]
pub(crate) enum Report {
    /// `done` of `total`, in whatever the work is counted in (bytes, files).
    Progress { id: u64, done: u64, total: u64 },
    /// What it is doing now, if that changes along the way.
    // (No worker says this yet: it is there for the ones wired in later.)
    #[allow(dead_code)]
    Label { id: u64, label: String },
    /// It is over.
    End { id: u64 },
}

/// One task as the bar shows it.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct Task {
    pub id: u64,
    /// What is being done, in the owner's words ("Importing photos from Pixel 8 Pro").
    pub label: String,
    pub done: u64,
    /// 0 until the work knows how much there is.
    pub total: u64,
    /// When it ended; it lingers for `LINGER` after.
    pub ended: Option<Instant>,
}

impl Task {
    /// How far along, 0…1 (a finished one is whole; one that does not yet
    /// know its size is at the start).
    pub fn fraction(&self) -> f32 {
        if self.ended.is_some() {
            1.0
        } else if self.total == 0 {
            0.0
        } else {
            (self.done as f64 / self.total as f64).clamp(0.0, 1.0) as f32
        }
    }

    /// The percentage as the pill writes it.
    pub fn percent(&self) -> u32 {
        // Not rounded up: 100 is said when it is over, not just before.
        let p = (self.fraction() * 100.0).floor() as u32;
        if self.ended.is_some() { 100 } else { p.min(99) }
    }
}

/// The tasks there are, oldest first.
#[derive(Debug, Default)]
pub(crate) struct Tasks {
    list: Vec<Task>,
    next: u64,
    /// Each running task's "stop" flag, shared with its worker's handle.
    stops: HashMap<u64, Arc<AtomicBool>>,
}

impl Tasks {
    /// A new task: its id, and the flag its worker reads to know it was
    /// cancelled.
    pub fn begin(&mut self, label: &str) -> (u64, Arc<AtomicBool>) {
        self.next += 1;
        self.list.push(Task { id: self.next, label: label.to_owned(), done: 0, total: 0, ended: None });
        let stop = Arc::new(AtomicBool::new(false));
        self.stops.insert(self.next, stop.clone());
        (self.next, stop)
    }

    /// Cancel a running task: its worker is told, and it leaves the list at
    /// once (it does not linger at 100%: it did not get there). Whether there
    /// was such a task.
    pub fn cancel(&mut self, id: u64) -> bool {
        let Some(at) = self.list.iter().position(|t| t.id == id && t.ended.is_none()) else {
            return false;
        };
        if let Some(stop) = self.stops.remove(&id) {
            stop.store(true, Ordering::Relaxed);
        }
        self.list.remove(at);
        true
    }

    /// Take a worker's report in. Whether anything the pill shows changed.
    pub fn report(&mut self, report: Report, now: Instant) -> bool {
        let before = self.shown().cloned();
        match report {
            Report::Progress { id, done, total } => {
                if let Some(t) = self.list.iter_mut().find(|t| t.id == id && t.ended.is_none()) {
                    (t.done, t.total) = (done, total);
                }
            }
            Report::Label { id, label } => {
                if let Some(t) = self.list.iter_mut().find(|t| t.id == id) {
                    t.label = label;
                }
            }
            Report::End { id } => {
                self.stops.remove(&id);
                if let Some(t) = self.list.iter_mut().find(|t| t.id == id && t.ended.is_none()) {
                    t.ended = Some(now);
                }
            }
        }
        before.as_ref() != self.shown()
    }

    /// Let the finished ones that have lingered go. Whether any went.
    pub fn sweep(&mut self, now: Instant) -> bool {
        let before = self.list.len();
        self.list.retain(|t| t.ended.is_none_or(|at| now.duration_since(at) < LINGER));
        self.list.len() != before
    }

    /// The task the pill shows: the newest one still running — else the one
    /// that ended last, while it lingers.
    pub fn shown(&self) -> Option<&Task> {
        self.list
            .iter()
            .rev()
            .find(|t| t.ended.is_none())
            .or_else(|| self.list.iter().max_by_key(|t| t.ended))
    }

    /// The running ones beside the one shown, newest first: the rows of the
    /// pill's box.
    pub fn rest(&self) -> Vec<&Task> {
        let shown = self.shown().map(|t| t.id);
        self.list.iter().rev().filter(|t| t.ended.is_none() && Some(t.id) != shown).collect()
    }

    /// How many are running beside the one shown.
    pub fn others(&self) -> usize {
        self.list.iter().filter(|t| t.ended.is_none()).count().saturating_sub(1)
    }

    /// When the next lingering one is due to go, if any.
    pub fn next_leave(&self) -> Option<Instant> {
        self.list.iter().filter_map(|t| t.ended).min().map(|at| at + LINGER)
    }

    pub fn is_empty(&self) -> bool {
        self.list.is_empty()
    }
}

/// A worker's hold on its task. Dropping it ends the task.
#[derive(Debug)]
pub(crate) struct TaskHandle {
    id: u64,
    tx: Sender<Report>,
    last: std::sync::Mutex<Option<Instant>>,
    stop: Arc<AtomicBool>,
}

impl TaskHandle {
    pub(crate) fn new(id: u64, tx: Sender<Report>, stop: Arc<AtomicBool>) -> Self {
        Self { id, tx, last: std::sync::Mutex::new(None), stop }
    }

    /// A handle to nothing: for work that runs with no task to show (tests).
    #[cfg(test)]
    pub(crate) fn detached() -> Self {
        Self::new(0, calloop::channel::channel::<Report>().0, Arc::new(AtomicBool::new(false)))
    }

    /// The owner cancelled it from the bar: stop where you can.
    pub fn cancelled(&self) -> bool {
        self.stop.load(Ordering::Relaxed)
    }

    /// `done` of `total` so far. Called as often as the work likes: reports
    /// closer together than `EVERY` are dropped, except the last (`done ==
    /// total`).
    pub fn set(&self, done: u64, total: u64) {
        let now = Instant::now();
        if let Ok(mut last) = self.last.lock() {
            if done < total && last.is_some_and(|at| now.duration_since(at) < EVERY) {
                return;
            }
            *last = Some(now);
        }
        let _ = self.tx.send(Report::Progress { id: self.id, done, total });
    }

    /// What it is doing now.
    #[allow(dead_code)]
    pub fn label(&self, label: &str) {
        let _ = self.tx.send(Report::Label { id: self.id, label: label.to_owned() });
    }
}

impl Drop for TaskHandle {
    fn drop(&mut self) {
        let _ = self.tx.send(Report::End { id: self.id });
    }
}

impl crate::App {
    /// Start a task the bar shows ("Moving golem.iso to Home"): hand the
    /// handle to whatever does the work. On the loop.
    pub(crate) fn task_begin(&mut self, label: &str) -> TaskHandle {
        if self.task_tx.is_none() {
            let (tx, rx) = calloop::channel::channel::<Report>();
            let heard = self.loop_handle.insert_source(rx, |event, _, app: &mut crate::App| {
                if let calloop::channel::Event::Msg(report) = event {
                    let ended = matches!(report, Report::End { .. });
                    if app.tasks.report(report, Instant::now()) {
                        app.tasks_changed();
                    }
                    if ended {
                        app.tasks_sweep_later();
                    }
                }
            });
            if let Err(e) = heard {
                tracing::warn!("tasks: their reports cannot be heard: {e}");
            }
            self.task_tx = Some(tx);
        }
        let (id, stop) = self.tasks.begin(label);
        tracing::info!("tasks: {label} (#{id})");
        self.tasks_changed();
        // (The sender is there: made just above.)
        let tx = self.task_tx.clone().unwrap_or_else(|| calloop::channel::channel::<Report>().0);
        TaskHandle::new(id, tx, stop)
    }

    /// A finished task leaves the bar once it has lingered.
    fn tasks_sweep_later(&mut self) {
        let Some(due) = self.tasks.next_leave() else {
            return;
        };
        let wait = due.saturating_duration_since(Instant::now()) + Duration::from_millis(20);
        let timer = calloop::timer::Timer::from_duration(wait);
        let _ = self.loop_handle.insert_source(timer, |_, _, app: &mut crate::App| {
            if app.tasks.sweep(Instant::now()) {
                app.tasks_changed();
            }
            if app.tasks.next_leave().is_some() {
                app.tasks_sweep_later();
            }
            calloop::timer::TimeoutAction::Drop
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_task_is_shown_from_its_start_to_a_moment_after_its_end() {
        let mut tasks = Tasks::default();
        let t0 = Instant::now();
        assert!(tasks.shown().is_none());
        let (photos, _) = tasks.begin("Importing photos from Pixel 8 Pro");
        assert_eq!(tasks.shown().map(|t| (t.label.as_str(), t.percent())), Some(("Importing photos from Pixel 8 Pro", 0)));
        assert!(tasks.report(Report::Progress { id: photos, done: 218, total: 2059 }, t0));
        assert_eq!(tasks.shown().unwrap().percent(), 10);
        // The last file in: 99 until it says it is over.
        tasks.report(Report::Progress { id: photos, done: 2059, total: 2059 }, t0);
        assert_eq!(tasks.shown().unwrap().percent(), 99);
        assert!(tasks.report(Report::End { id: photos }, t0));
        assert_eq!(tasks.shown().unwrap().percent(), 100);
        assert_eq!(tasks.next_leave(), Some(t0 + LINGER));
        assert!(!tasks.sweep(t0 + LINGER / 2), "it lingers");
        assert!(tasks.sweep(t0 + LINGER));
        assert!(tasks.is_empty() && tasks.shown().is_none());
    }

    #[test]
    fn of_several_the_newest_running_one_is_shown() {
        let mut tasks = Tasks::default();
        let t0 = Instant::now();
        let (photos, _) = tasks.begin("Importing photos from Pixel 8 Pro");
        let (iso, _) = tasks.begin("Moving golem.iso to Home");
        assert_eq!(tasks.shown().unwrap().id, iso);
        assert_eq!(tasks.others(), 1);
        assert_eq!(tasks.rest().iter().map(|t| t.id).collect::<Vec<_>>(), vec![photos]);
        // The newer one ends: the older, still running, is the one to show.
        tasks.report(Report::End { id: iso }, t0);
        assert_eq!(tasks.shown().unwrap().id, photos);
        assert_eq!(tasks.others(), 0);
        // A report for one that is over changes nothing.
        assert!(!tasks.report(Report::Progress { id: iso, done: 1, total: 2 }, t0));
        tasks.report(Report::End { id: photos }, t0 + Duration::from_millis(10));
        assert_eq!(tasks.shown().unwrap().id, photos, "the last to end lingers on the pill");
    }

    #[test]
    fn a_cancelled_task_tells_its_worker_and_leaves_at_once() {
        let mut tasks = Tasks::default();
        let (photos, _) = tasks.begin("Importing photos from Pixel 8 Pro");
        let (iso, stop) = tasks.begin("Moving golem.iso to Home");
        let (tx, _rx) = calloop::channel::channel::<Report>();
        let worker = TaskHandle::new(iso, tx, stop);
        assert!(!worker.cancelled());
        assert!(tasks.cancel(iso));
        assert!(worker.cancelled(), "the worker is told");
        assert_eq!(tasks.shown().map(|t| t.id), Some(photos), "and it is gone, not lingering");
        assert_eq!(tasks.others(), 0);
        assert!(!tasks.cancel(iso), "once");
        // Its worker's last word changes nothing.
        assert!(!tasks.report(Report::End { id: iso }, Instant::now()));
    }

    #[test]
    fn letting_go_of_the_handle_ends_the_task() {
        let (tx, rx) = calloop::channel::channel::<Report>();
        let handle = TaskHandle::new(7, tx, Arc::new(AtomicBool::new(false)));
        handle.set(1, 10);
        handle.set(2, 10); // too soon after: dropped
        handle.set(10, 10); // the last is always said
        drop(handle);
        let mut got = Vec::new();
        while let Ok(report) = rx.try_recv() {
            got.push(report);
        }
        assert_eq!(
            got,
            vec![
                Report::Progress { id: 7, done: 1, total: 10 },
                Report::Progress { id: 7, done: 10, total: 10 },
                Report::End { id: 7 },
            ]
        );
    }
}
