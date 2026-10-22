//! PRINTS ON THE BAR (Max, 2026-10-21: "you do the controls and pills… also
//! a process on the status bar"): every print CUPS is working on is a task
//! on the OPTIONS bar's task pill (`tasks.rs`) — "Printing report.pdf",
//! "Waiting to print…", or, when its printer stops it, what stopped it ("HP
//! DeskJet 1200 series is out of paper"); its Cancel cancels the print. What
//! CUPS says comes from the Brain (the engine's printing layer, woken by
//! CUPS's own signals), so a print from any app is on the bar the moment it
//! is sent.
//!
//! NO PERCENTAGE: CUPS counts the pages it has made, not the ones out of the
//! printer (three made in half a second, twelve seconds on paper — measured
//! on Max's DeskJet), so the number's place stays empty and the fill comes
//! whole when the printer has had it all. A print that ends any other way
//! leaves at once; one that FAILED is said in a notification too — the pill
//! is gone by then.

use std::collections::HashMap;
use std::process::{Command, Stdio};

use options_engine::{JobState, PrintJob, PrinterProblem, PrintingState};
use tracing::{info, warn};

use crate::i18n::{fill, tr};
use crate::tasks::TaskHandle;
use crate::App;

/// The prints on the bar: each one's task, and the words it says now.
#[derive(Debug, Default)]
pub(crate) struct PrintTasks {
    live: HashMap<u32, (TaskHandle, String)>,
}

/// A printer as people call it (its description, else its queue's name).
pub(crate) fn printer_name(state: &PrintingState, queue: &str) -> String {
    state
        .printer(queue)
        .map(|p| if p.info.is_empty() { p.name.clone() } else { p.info.clone() })
        .unwrap_or_else(|| queue.to_owned())
}

/// A print's name: its document's, without the folders an app may send.
pub(crate) fn title_of(job: &PrintJob) -> String {
    let t = job.title.rsplit('/').next().unwrap_or_default().trim();
    if t.is_empty() {
        tr("a document").to_owned()
    } else {
        t.to_owned()
    }
}

/// What stops a printer, said about it (`{printer}` stands for its name).
pub(crate) fn problem_words(problem: &PrinterProblem) -> &'static str {
    match problem {
        PrinterProblem::NoPaper => "{printer} is out of paper",
        PrinterProblem::PaperJam => "Paper jam in {printer}",
        PrinterProblem::NoInk => "{printer} is out of ink",
        PrinterProblem::DoorOpen => "{printer} has a door open",
        PrinterProblem::Offline => "{printer} is off",
        PrinterProblem::Paused => "{printer} is paused",
        PrinterProblem::Other(_) => "{printer} has stopped",
    }
}

/// What a print's task says: what stops its printer, if anything does, else
/// how far along it is.
pub(crate) fn words(job: &PrintJob, state: &PrintingState) -> String {
    let printer = printer_name(state, &job.printer);
    if let Some(problem) = state.printer(&job.printer).and_then(|p| p.problem.as_ref()) {
        return fill(tr(problem_words(problem)), &[("printer", &printer)]);
    }
    let title = title_of(job);
    match job.state {
        JobState::Held => fill(tr("On hold: {title}"), &[("title", &title)]),
        JobState::Pending => fill(tr("Waiting to print {title}"), &[("title", &title)]),
        JobState::Stopped => fill(tr("{printer} is paused"), &[("printer", &printer)]),
        _ => fill(tr("Printing {title}"), &[("title", &title)]),
    }
}

impl App {
    /// The Brain's printing changed: a task for each print on its way, its
    /// words kept true; the ones that ended let go — a printed one with its
    /// fill whole, any other at once.
    pub(crate) fn printing_seen(&mut self, state: &PrintingState) {
        let shown: Vec<u32> = self.print_tasks.live.keys().copied().collect();
        for id in shown {
            let job = state.jobs.iter().find(|j| j.id == id);
            if job.is_some_and(|j| !j.state.ended()) {
                continue;
            }
            let Some((task, _)) = self.print_tasks.live.remove(&id) else { continue };
            match job.map(|j| j.state) {
                // Letting go of it ends it: whole, for a moment.
                Some(JobState::Completed) => info!("printing: print {id} printed"),
                ended => {
                    if self.tasks.cancel(task.id()) {
                        self.tasks_changed();
                    }
                    if let (Some(JobState::Aborted), Some(j)) = (ended, job) {
                        warn!("printing: print {id} failed ({})", j.message);
                        notify(
                            tr("Not printed"),
                            &fill(
                                tr("{title} on {printer}"),
                                &[("title", &title_of(j)), ("printer", &printer_name(state, &j.printer))],
                            ),
                        );
                    }
                }
            }
        }
        for job in state.jobs.iter().filter(|j| !j.state.ended()) {
            let said = words(job, state);
            match self.print_tasks.live.get_mut(&job.id) {
                Some((task, was)) => {
                    if *was != said {
                        task.label(&said);
                        *was = said;
                    }
                }
                None => {
                    info!("printing: print {} on the bar: {said}", job.id);
                    let task = self.task_begin_with(&said, Some(String::new()));
                    self.print_tasks.live.insert(job.id, (task, said));
                }
            }
        }
    }

    /// The Printers page, while it is open, read again: what it shows changed.
    pub(crate) fn printers_page_follow(&mut self) {
        let open = |p: &crate::panel::Panel| p.open_control_label() == Some("Printers");
        if open(self.settings_panel()) || open(&self.quick) {
            let ui = self.ui_facts();
            self.controls_send(crate::controls::ControlCommand::Read("Printers", ui));
        }
    }

    /// A task cancelled from the bar: if it was a print's, the print is
    /// cancelled (off the loop: CUPS is asked).
    pub(crate) fn printing_task_cancelled(&mut self, task: u64) {
        let Some(id) = self.print_tasks.live.iter().find(|(_, (t, _))| t.id() == task).map(|(id, _)| *id) else {
            return;
        };
        self.print_tasks.live.remove(&id);
        info!("printing: print {id} cancelled from the bar");
        let spawned = std::thread::Builder::new().name("print-cancel".into()).spawn(move || {
            if let Err(e) = options_engine::cups::cancel_job(id) {
                warn!("printing: print {id} could not be cancelled: {e}");
            }
        });
        if let Err(e) = spawned {
            warn!("printing: no thread to cancel print {id}: {e}");
        }
    }
}

/// A desktop notification through the session bus (as the battery's).
fn notify(title: &str, body: &str) {
    let r = Command::new("busctl")
        .args([
            "--user",
            "call",
            "org.freedesktop.Notifications",
            "/org/freedesktop/Notifications",
            "org.freedesktop.Notifications",
            "Notify",
            "susssasa{sv}i",
            "waverunner",
            "0",
            "printer",
            title,
            body,
            "0",
            "0",
            "10000",
        ])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn();
    if let Err(e) = r {
        warn!("printing: no notification: {e}");
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use options_engine::{Printer, PrinterState};

    fn state(problem: Option<PrinterProblem>) -> PrintingState {
        PrintingState {
            printers: vec![Printer {
                name: "DeskJet-1200-series".into(),
                info: "HP DeskJet 1200 series".into(),
                state: PrinterState::Printing,
                problem,
                ..Default::default()
            }],
            jobs: vec![],
        }
    }

    fn job(state: JobState, title: &str) -> PrintJob {
        PrintJob { id: 7, printer: "DeskJet-1200-series".into(), title: title.into(), state, ..Default::default() }
    }

    #[test]
    fn a_print_says_how_far_along_it_is_or_what_stops_its_printer() {
        let fine = state(None);
        assert_eq!(words(&job(JobState::Printing, "/home/max/report.pdf"), &fine), "Printing report.pdf");
        assert_eq!(words(&job(JobState::Pending, "report.pdf"), &fine), "Waiting to print report.pdf");
        assert_eq!(words(&job(JobState::Printing, ""), &fine), "Printing a document");
        let stopped = state(Some(PrinterProblem::NoPaper));
        assert_eq!(words(&job(JobState::Printing, "report.pdf"), &stopped), "HP DeskJet 1200 series is out of paper");
        let off = state(Some(PrinterProblem::Offline));
        assert_eq!(words(&job(JobState::Pending, "report.pdf"), &off), "HP DeskJet 1200 series is off");
    }
}
