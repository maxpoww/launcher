//! THE PRINTERS PAGE (Max, 2026-10-21: "idk about printing and scanners, so
//! you do the controls and pills"; the idea canvas
//! https://claude.ai/artifact/TJhk4hScgQMzU1hm6Mb2HV): the control panel's
//! page for printing, made of the idioms the other pages use.
//!
//! What it says, top down, for the printer it is about (the default one, or
//! the one picked when there are several): how it is (ready, printing, what
//! stops it) as the subtitle; with more than one, which printer and whether
//! it is the default; its ink when it tells; what a print gets unless the
//! app asks otherwise — Colour, Quality, Paper, Both sides (only what the
//! printer can do, the standard IPP defaults CUPS maps onto every driver);
//! its prints, each with Cancel, or Print again for one cancelled or failed;
//! then a Test page, and Pause or Resume.
//!
//! On the controls' worker (`controls::read` / `apply`): every reading and
//! change is one IPP request to CUPS (`options_engine::cups`). The page is
//! read again whenever the Brain's printing changes while it is open.

use std::sync::Mutex;

use options_engine::cups::{self, Default as Want};
use options_engine::{JobState, Printer, PrintingState};
use tracing::{info, warn};

use crate::controls::{group_of, line_of, on_off_of, Choice, ControlView, Group};
use crate::i18n::{fill, tr};
use crate::printing::{printer_name, problem_words, title_of};

/// The printer the page is about (its queue's name), once one was picked
/// or read.
static PICKED: Mutex<Option<String>> = Mutex::new(None);

/// The papers offered (PWG names), in this order, when the printer takes
/// them; the one it is set to is always there.
const PAPERS: [(&str, &str); 4] = [
    ("na_letter_8.5x11in", "Letter"),
    ("iso_a4_210x297mm", "A4"),
    ("na_legal_8.5x14in", "Legal"),
    ("na_foolscap_8.5x13in", "8.5 × 13"),
];

/// How far back the page shows the prints that ended, and how many of them.
const RECENT_SECS: u64 = 3600;
const RECENT_SHOWN: usize = 3;

/// IPP's print qualities.
const QUALITIES: [(i32, &str); 3] = [(3, "Draft"), (4, "Normal"), (5, "Best")];

fn choice(text: impl Into<String>, on: bool, value: impl Into<String>) -> Choice {
    Choice { text: text.into(), on, value: value.into(), swatch: Vec::new(), caption: false, icon: None, hint: None }
}

/// A word row: its name at the left, its words at the right.
fn words(caption: &'static str, key: &'static str, choices: Vec<Choice>) -> Group {
    Group { labelled: true, ..group_of(Some(caption), key, choices) }
}

/// A paper by its PWG name: the name people know, else its size as the
/// name says it (`na_index-4x6_4x6in` → `4x6in`).
fn paper_word(pwg: &str) -> String {
    PAPERS
        .iter()
        .find(|(p, _)| *p == pwg)
        .map(|(_, w)| tr(w).to_owned())
        .unwrap_or_else(|| pwg.rsplit('_').next().unwrap_or(pwg).to_owned())
}

/// The printer the page is about: the one picked (if it is still there),
/// else the default, else the first.
fn shown(state: &PrintingState) -> Option<&Printer> {
    let picked = PICKED.lock().ok().and_then(|p| p.clone());
    picked
        .and_then(|n| state.printer(&n))
        .or_else(|| state.default_printer())
        .or_else(|| state.printers.first())
}

/// How the printer is, in a few words. What a printer says while it prints
/// (no paper, a jam, no ink, a door) is said only while a print is on its
/// way: CUPS can keep the last of it after the print is gone (cancelled out
/// of paper, it stayed "printing job 0, out of paper" — seen 2026-10-21),
/// and with nothing to print it stands in no one's way. Off and paused are
/// the queue's own, and true at any time.
fn how(state: &PrintingState, p: &Printer) -> String {
    let name = printer_name(state, &p.name);
    let printing = state.jobs.iter().any(|j| j.printer == p.name && !j.state.ended());
    let now = p
        .problem
        .as_ref()
        .filter(|pr| printing || matches!(pr, options_engine::PrinterProblem::Offline | options_engine::PrinterProblem::Paused));
    if let Some(problem) = now {
        return fill(tr(problem_words(problem)), &[("printer", &name)]);
    }
    match state.jobs.iter().find(|j| j.printer == p.name && j.state == JobState::Printing) {
        Some(j) => fill(tr("Printing {title}"), &[("title", &title_of(j))]),
        None => fill(tr("{printer} is ready"), &[("printer", &name)]),
    }
}

/// The page (`controls::read("Printers")`, on the controls' worker).
pub(crate) fn view(view: &mut ControlView) {
    if !cups::present() {
        view.subtitle = tr("This computer has no printing.").to_owned();
        return;
    }
    let state = match cups::snapshot() {
        Ok(s) => s,
        Err(e) => {
            warn!("printers: {e}");
            view.subtitle = tr("Printing is not answering.").to_owned();
            return;
        }
    };
    let Some(p) = shown(&state) else {
        view.subtitle = tr("No printers. Plug one in, or turn on one on the network.").to_owned();
        return;
    };
    if let Ok(mut picked) = PICKED.lock() {
        *picked = Some(p.name.clone());
    }
    view.subtitle = how(&state, p);
    // WHICH ONE, with several.
    if state.printers.len() > 1 {
        let names = state.printers.iter().map(|o| choice(printer_name(&state, &o.name), o.name == p.name, o.name.clone())).collect();
        view.groups.push(words("Printer", "printers-pick", names));
        view.groups.push(on_off_of(Some("Default"), "printers-default", p.default));
    }
    // ITS INK, when it says.
    let ink: Vec<String> = p
        .markers
        .iter()
        .filter_map(|m| m.level.map(|l| format!("{} {l}%", m.name)))
        .collect();
    if !ink.is_empty() {
        view.groups.push(Group { centred: true, ..line_of(ink.join("  ·  ")) });
    }
    // WHAT A PRINT GETS unless its app asks otherwise.
    match cups::defaults(&p.name) {
        Ok(d) => {
            if let Some(colour) = d.colour {
                view.groups.push(words(
                    "Colour",
                    "printers-colour",
                    vec![choice(tr("Colour"), colour, "on"), choice(tr("Black and white"), !colour, "off")],
                ));
            }
            let qualities: Vec<Choice> = QUALITIES
                .iter()
                .filter(|(q, _)| d.qualities.contains(q))
                .map(|(q, w)| choice(tr(w), d.quality == Some(*q), q.to_string()))
                .collect();
            if qualities.len() > 1 {
                view.groups.push(words("Quality", "printers-quality", qualities));
            }
            let mut papers: Vec<Choice> = PAPERS
                .iter()
                .filter(|(pwg, _)| d.medias.iter().any(|m| m == pwg))
                .map(|(pwg, _)| choice(paper_word(pwg), d.media == *pwg, *pwg))
                .collect();
            if !d.media.is_empty() && !papers.iter().any(|c| c.on) {
                papers.push(choice(paper_word(&d.media), true, d.media.clone()));
            }
            if papers.len() > 1 {
                view.groups.push(words("Paper", "printers-paper", papers));
            }
            if let Some(both) = d.both_sides {
                view.groups.push(on_off_of(Some("Both sides"), "printers-sides", both));
            }
        }
        Err(e) => warn!("printers: {}'s defaults: {e}", p.name),
    }
    // ITS PRINTS: each with what can be done to it — the ones on their way,
    // then the last few that ended this past hour, to print again.
    let hour_ago = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs().saturating_sub(RECENT_SECS));
    let ended = cups::ended_since(hour_ago).unwrap_or_default();
    let recent = ended.iter().filter(|j| j.printer == p.name).take(RECENT_SHOWN);
    for j in state.jobs.iter().filter(|j| j.printer == p.name).chain(recent) {
        let (state_word, act) = match j.state {
            JobState::Pending => ("waiting", Some(("Cancel", "cancel"))),
            JobState::Held => ("on hold", Some(("Cancel", "cancel"))),
            JobState::Printing => ("printing", Some(("Cancel", "cancel"))),
            JobState::Stopped => ("stopped", Some(("Cancel", "cancel"))),
            JobState::Completed => ("printed", Some(("Print again", "again"))),
            JobState::Canceled => ("cancelled", Some(("Print again", "again"))),
            JobState::Aborted => ("not printed", Some(("Print again", "again"))),
        };
        let caption = format!("{}  ·  {}", title_of(j), tr(state_word));
        let choices = act.map(|(w, v)| vec![choice(tr(w), false, format!("{v}:{}", j.id))]).unwrap_or_default();
        if choices.is_empty() {
            view.groups.push(line_of(caption));
        } else {
            view.groups.push(Group { caption: Some(caption), ..words("Print", "printers-job", choices) });
        }
    }
    // WHAT CAN BE DONE TO IT.
    let pause = if p.state == options_engine::PrinterState::Stopped {
        choice(tr("Resume"), false, "resume")
    } else {
        choice(tr("Pause"), false, "pause")
    };
    view.groups.push(Group { buttons: true, ..group_of(None, "printers-do", vec![pause, choice(tr("Test page"), false, "test")]) });
}

/// A pick on the page (`printers-*`), on the controls' worker.
pub(crate) fn apply(key: &str, value: &str) {
    let Some(name) = PICKED.lock().ok().and_then(|p| p.clone()) else {
        return;
    };
    let on = value == "on";
    let done = match key {
        "printers-pick" => {
            if let Ok(mut picked) = PICKED.lock() {
                *picked = Some(value.to_owned());
            }
            Ok(())
        }
        // (One printer is always the default: switching it off is nothing.)
        "printers-default" if on => cups::set_default(&name),
        "printers-colour" => cups::set_default_option(&name, &Want::Colour(on)),
        "printers-quality" => match value.parse() {
            Ok(q) => cups::set_default_option(&name, &Want::Quality(q)),
            Err(_) => Ok(()),
        },
        "printers-paper" => cups::set_default_option(&name, &Want::Media(value.to_owned())),
        "printers-sides" => cups::set_default_option(&name, &Want::BothSides(on)),
        "printers-job" => {
            let (what, id) = value.split_once(':').unwrap_or((value, ""));
            match (what, id.parse::<u32>()) {
                ("cancel", Ok(id)) => cups::cancel_job(id),
                ("again", Ok(id)) => cups::restart_job(id),
                _ => Ok(()),
            }
        }
        "printers-do" => match value {
            "pause" => cups::pause_printer(&name),
            "resume" => cups::resume_printer(&name),
            "test" => cups::print_bytes(&name, test_page(&name).into_bytes(), tr("Golem test page")).map(|_| ()),
            _ => Ok(()),
        },
        _ => Ok(()),
    };
    match done {
        Ok(()) => info!("printers: {key} = {value} on {name}"),
        Err(e) => warn!("printers: {key} = {value} on {name}: {e}"),
    }
}

/// Text made safe inside a PostScript string, in Latin-1 (the page's fonts
/// are re-encoded so: "Página" prints whole); what Latin-1 has not is `?`.
fn ps(text: &str) -> String {
    text.chars()
        .filter(|c| !c.is_control())
        .map(|c| match c {
            '(' | ')' | '\\' => format!("\\{c}"),
            c if c.is_ascii() => c.to_string(),
            c if u32::from(c) <= 0xFF => format!("\\{:03o}", u32::from(c)),
            _ => "?".to_owned(),
        })
        .collect()
}

/// Golem's test page: its name, the printer's, the time, and a few thin
/// lines and greys — little ink, and enough to see a head that misses.
fn test_page(printer: &str) -> String {
    let when = crate::desktop_props::date_text(Some(std::time::SystemTime::now())).unwrap_or_default();
    let lines: String = (0..6).map(|i| format!("72 {} 468 0.6 rectfill\n", 470 - i * 10)).collect();
    let greys: String = (0..5)
        .map(|i| format!("{} setgray {} 380 72 36 rectfill\n", 0.8 - f64::from(i) * 0.2, 72 + i * 90))
        .collect();
    format!(
        "%!PS-Adobe-3.0\n%%BoundingBox: 0 0 612 792\n%%Pages: 1\n%%EndComments\n\
         /latin1 {{ findfont dup length dict begin {{ 1 index /FID ne {{ def }} {{ pop pop }} ifelse }} forall \
         /Encoding ISOLatin1Encoding def currentdict end definefont pop }} def\n\
         /GB /Helvetica-Bold latin1\n/G /Helvetica latin1\n%%Page: 1 1\n\
         0 setgray\n/GB findfont 28 scalefont setfont\n72 690 moveto ({title}) show\n\
         /G findfont 14 scalefont setfont\n72 660 moveto ({printer}) show\n72 640 moveto ({when}) show\n\
         72 600 moveto ({works}) show\n{lines}{greys}0 setgray\n\
         /G findfont 9 scalefont setfont\n72 340 moveto (ABCDEFGHIJKLMNOPQRSTUVWXYZ abcdefghijklmnopqrstuvwxyz 0123456789) show\n\
         showpage\n%%EOF\n",
        title = ps(tr("Golem test page")),
        printer = ps(printer),
        when = ps(&when),
        works = ps(tr("If you can read this, printing works.")),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_paper_is_called_what_people_call_it() {
        assert_eq!(paper_word("na_letter_8.5x11in"), "Letter");
        assert_eq!(paper_word("iso_a4_210x297mm"), "A4");
        assert_eq!(paper_word("na_index-4x6_4x6in"), "4x6in");
    }

    #[test]
    fn the_test_page_is_postscript_with_its_words_safe() {
        let page = test_page("Office (2nd floor)");
        assert!(page.starts_with("%!PS-Adobe-3.0"));
        assert!(page.contains("(Office \\(2nd floor\\)) show"));
        assert_eq!(ps("Página"), "P\\341gina");
        assert!(page.trim_end().ends_with("%%EOF"));
    }
}
