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
//!
//! THEN (2026-10-22, Max: "do this"): the INK of a printer that does not
//! tell CUPS — an HP over USB — read by Golem's `golem-printer-ink` (HPLIP's
//! own device code) off the worker, kept ten minutes, never while a print
//! holds the printer; the SCANNERS there are (`scanners.rs`'s watcher), each
//! with Scan, or Install Simple Scan; ADD A PRINTER BY ITS ADDRESS — a field
//! whose Enter tries the address the driverless way every printer made since
//! about 2013 speaks (IPP Everywhere: `lpadmin -m everywhere` on
//! ipp://<address>/ipp/print, then ipps://, then /ipp), the printer then
//! called what it says it is; and REMOVE, asked once.

use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Mutex;
use std::time::{Duration, Instant};

use options_engine::cups::{self, Default as Want};
use options_engine::{JobState, Printer, PrintingState};
use tracing::{info, warn};

use crate::controls::{field_of, group_of, line_of, on_off_of, Choice, ControlView, Group};
use crate::i18n::{fill, tr};
use crate::printing::{printer_name, problem_words, title_of};

/// The printer asked for (its queue's name): a pick, or one just added.
static PICKED: Mutex<Option<String>> = Mutex::new(None);
/// The printer the page SHOWS — what its actions act on (a printer added a
/// moment ago must not take a click meant for the one on screen).
static SHOWN: Mutex<Option<String>> = Mutex::new(None);

/// WHAT CUPS SAID, kept a few seconds: a key typed in the address field reads
/// the page again, and must not ask CUPS each time. Any change (a pick, a
/// printer added, the Brain's printing moving) lets it go (`stale`).
type Kept<T> = Mutex<Option<(Instant, T)>>;
static KEPT_STATE: Kept<PrintingState> = Mutex::new(None);
static KEPT_ENDED: Kept<Vec<options_engine::PrintJob>> = Mutex::new(None);
static KEPT_DEFAULTS: Kept<(String, cups::Defaults)> = Mutex::new(None);
const KEEP: Duration = Duration::from_secs(5);

/// What CUPS said is old: the next read asks it again.
pub(crate) fn stale() {
    if let Ok(mut k) = KEPT_STATE.lock() {
        *k = None;
    }
    if let Ok(mut k) = KEPT_ENDED.lock() {
        *k = None;
    }
    if let Ok(mut k) = KEPT_DEFAULTS.lock() {
        *k = None;
    }
}

/// What `slot` keeps if it is young (and `fits`), else `ask` — kept if it answered.
fn kept<T: Clone>(slot: &Kept<T>, fits: impl Fn(&T) -> bool, ask: impl FnOnce() -> Result<T, cups::CupsError>) -> Result<T, cups::CupsError> {
    if let Some((at, v)) = slot.lock().ok().and_then(|k| k.clone()) {
        if at.elapsed() < KEEP && fits(&v) {
            return Ok(v);
        }
    }
    let v = ask()?;
    if let Ok(mut k) = slot.lock() {
        *k = Some((Instant::now(), v.clone()));
    }
    Ok(v)
}

/// THE INK of the printers that do not tell CUPS: each reading by the
/// printer's device address — when, and what (`None`: it did not answer) —
/// and whether one is being taken.
type InkReading = (String, Instant, Option<Vec<(u32, u8)>>);
static INK: Mutex<Vec<InkReading>> = Mutex::new(Vec::new());
static INK_ASKING: AtomicBool = AtomicBool::new(false);
/// Golem's program that reads it (`desktop/printing.nix`).
const INK_PROGRAM: &str = "golem-printer-ink";
/// How long a reading stands; one that failed is tried again sooner.
const INK_FRESH: Duration = Duration::from_secs(600);
const INK_AGAIN: Duration = Duration::from_secs(60);
const INK_PATIENCE: Duration = Duration::from_secs(40);

/// The scanners' list older than this: the watcher is asked for a look.
const SCANNERS_OLD: Duration = Duration::from_secs(120);

/// ADDING A PRINTER BY ITS ADDRESS: what the page says about it.
#[derive(Debug, Clone, PartialEq)]
enum Adding {
    Trying(String),
    /// Nothing answered there (said for `ASKED_FOR`).
    Failed(String, Instant),
}
static ADDING: Mutex<Option<Adding>> = Mutex::new(None);
/// How long `lpadmin` may try one address (it asks the printer itself).
const ADD_PATIENCE: Duration = Duration::from_secs(30);

/// THE PAGE'S TWO SECTIONS, once there are scanners (the page does not
/// scroll): the printers, or the scanners.
static ON_SCANNERS: AtomicBool = AtomicBool::new(false);

/// The printer asked about being removed, and when (the second click
/// removes it; the question goes after `ASKED_FOR`).
static REMOVING: Mutex<Option<(String, Instant)>> = Mutex::new(None);
const ASKED_FOR: Duration = Duration::from_secs(60);

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
const RECENT_SHOWN: usize = 1;
/// The most prints listed (the page does not scroll): the ones on their way
/// first, the ended after them.
const PRINTS_SHOWN: usize = 3;
/// The rows a page holds in the smallest box (a 1440×900 screen's).
const PAGE_ROWS: usize = 8;

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

/// The page (`controls::read("Printers")`, on the controls' worker): the
/// printer, then the scanners, then a printer added by its address.
pub(crate) fn view(view: &mut ControlView) {
    let scanners = scanners_known();
    let on_scanners = !scanners.is_empty() && ON_SCANNERS.load(Ordering::Relaxed);
    // (The status line is the printer's either way: read first, then the
    // section words go on top of what it made.)
    let mut printers = ControlView { label: view.label, ..Default::default() };
    printer_part(&mut printers, !scanners.is_empty());
    view.subtitle = printers.subtitle;
    if !scanners.is_empty() {
        let tabs = vec![choice(tr("Printers"), !on_scanners, "printers"), choice(tr("Scanners"), on_scanners, "scanners")];
        view.groups.push(Group { centred: true, ..group_of(None, "printers-tab", tabs) });
    }
    if on_scanners {
        scanner_part(view, scanners);
    } else {
        view.groups.extend(printers.groups);
        add_part(view);
    }
}

/// The scanners the watcher has found; a fresh look asked for when that is
/// old, or there has been none yet (the watcher's first waits for the start
/// to settle: now, since someone is looking).
fn scanners_known() -> Vec<crate::scanners::Scanner> {
    match crate::scanners::known() {
        Some((found, age)) => {
            if age > SCANNERS_OLD {
                crate::scanners::look_now();
            }
            found
        }
        None => {
            crate::scanners::look_now();
            Vec::new()
        }
    }
}

/// The printer the page is about, its prints and what can be done to it.
fn printer_part(view: &mut ControlView, tabs: bool) {
    if !cups::present() {
        view.subtitle = tr("This computer has no printing.").to_owned();
        return;
    }
    let state = match kept(&KEPT_STATE, |_| true, cups::snapshot) {
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
    if let Ok(mut shown) = SHOWN.lock() {
        *shown = Some(p.name.clone());
    }
    view.subtitle = how(&state, p);
    // WHICH ONE, with several.
    if state.printers.len() > 1 {
        let names = state.printers.iter().map(|o| choice(short(&printer_name(&state, &o.name)), o.name == p.name, o.name.clone())).collect();
        view.groups.push(words("Printer", "printers-pick", names));
    }
    // ITS INK: what it tells CUPS, else what Golem reads from it.
    let printing = state.jobs.iter().any(|j| j.printer == p.name && !j.state.ended());
    let mut ink: Vec<String> = p
        .markers
        .iter()
        .filter_map(|m| m.level.map(|l| format!("{} {l}%", m.name)))
        .collect();
    if ink.is_empty() {
        ink = hp_ink(p, printing)
            .unwrap_or_default()
            .into_iter()
            .map(|(kind, level)| format!("{} {level}%", tr(ink_name(kind))))
            .collect();
    }
    // (On the status line: the page does not scroll, and a row is dear.)
    if !ink.is_empty() {
        view.subtitle = format!("{}  ·  {}", view.subtitle, ink.join("  ·  "));
    }
    // WHAT A PRINT GETS unless its app asks otherwise.
    let name = p.name.clone();
    match kept(&KEPT_DEFAULTS, |(n, _)| *n == p.name, || cups::defaults(&name).map(|d| (name.clone(), d))).map(|(_, d)| d) {
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
    let ended = kept(&KEPT_ENDED, |_| true, || cups::ended_since(hour_ago)).unwrap_or_default();
    let recent = ended.iter().filter(|j| j.printer == p.name).take(RECENT_SHOWN);
    // As many as the box has rows left for (the section words, the address
    // field and the buttons still to come), and the one printing at least.
    let used = view.groups.len() + usize::from(tabs) + 2;
    let room = PAGE_ROWS.saturating_sub(used).clamp(1, PRINTS_SHOWN);
    for j in state.jobs.iter().filter(|j| j.printer == p.name).chain(recent).take(room) {
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
    // WHAT CAN BE DONE TO IT: removing it is asked once.
    let asked = REMOVING.lock().ok().and_then(|r| r.clone()).is_some_and(|(r, at)| r == p.name && at.elapsed() < ASKED_FOR);
    if asked {
        view.groups.push(Group {
            centred: true,
            ..line_of(fill(tr("Remove {printer}? Its waiting prints go with it."), &[("printer", &printer_name(&state, &p.name))]))
        });
        view.groups.push(Group {
            buttons: true,
            ..group_of(None, "printers-remove", vec![choice(tr("Keep"), false, "keep"), choice(tr("Remove"), false, "yes")])
        });
        return;
    }
    let pause = if p.state == options_engine::PrinterState::Stopped {
        choice(tr("Resume"), false, "resume")
    } else {
        choice(tr("Pause"), false, "pause")
    };
    let mut acts = vec![choice(tr("Remove"), false, "remove"), pause];
    if state.printers.len() > 1 && !p.default {
        acts.push(choice(tr("Make default"), false, "default"));
    }
    acts.push(choice(tr("Test page"), false, "test"));
    view.groups.push(Group { buttons: true, ..group_of(None, "printers-do", acts) });
}

/// A name cut to what a row of names can hold.
fn short(name: &str) -> String {
    const MOST: usize = 24;
    if name.chars().count() <= MOST {
        return name.to_owned();
    }
    let cut: String = name.chars().take(MOST - 1).collect();
    format!("{}…", cut.trim_end())
}

/// What a driverless printer says it is, made a name: its make and model
/// without what CUPS adds ("Deskjet 1200 Series, hpcups 3.26.4 - IPP
/// Everywhere" → "Deskjet 1200 Series").
fn model_name(model: &str) -> String {
    let m = model.split(" - IPP Everywhere").next().unwrap_or(model);
    m.split(", ").next().unwrap_or(m).trim().to_owned()
}

/// HP's kinds of ink (HPLIP's agent types, base/codes.py), as the page
/// names them.
fn ink_name(kind: u32) -> &'static str {
    match kind {
        1 => "Black",
        2 => "Colour",
        3 => "Photo",
        4 => "Cyan",
        5 => "Magenta",
        6 => "Yellow",
        17 => "Light grey",
        18 | 24 => "Grey",
        20 => "Photo black",
        21 => "Matte black",
        22 => "Light cyan",
        23 => "Light magenta",
        _ => "Ink",
    }
}

/// `golem-printer-ink`'s lines: `<kind> TAB <percent>`, anything else left out.
fn parse_ink(out: &str) -> Vec<(u32, u8)> {
    out.lines()
        .filter_map(|l| {
            let (kind, level) = l.split_once('\t')?;
            Some((kind.trim().parse().ok()?, level.trim().parse::<u8>().ok()?.min(100)))
        })
        .collect()
}

/// The ink of an HP printer (it does not tell CUPS) as last read — and a new
/// reading begun, off the worker, when that one is old and no print holds
/// the printer. The page is read again when it comes.
fn hp_ink(p: &Printer, printing: bool) -> Option<Vec<(u32, u8)>> {
    if !p.uri.starts_with("hp:") {
        return None;
    }
    let (fresh, levels) = INK
        .lock()
        .ok()
        .and_then(|ink| {
            ink.iter().find(|(u, ..)| *u == p.uri).map(|(_, at, l)| {
                let lasts = if l.is_some() { INK_FRESH } else { INK_AGAIN };
                (at.elapsed() < lasts, l.clone())
            })
        })
        .unwrap_or((false, None));
    if !fresh && !printing && crate::launch::on_path(INK_PROGRAM) && !INK_ASKING.swap(true, Ordering::AcqRel) {
        let uri = p.uri.clone();
        let spawned = std::thread::Builder::new().name("printer-ink".into()).spawn(move || {
            let read = read_ink(&uri);
            info!("printers: ink of {uri}: {read:?}");
            if let Ok(mut ink) = INK.lock() {
                ink.retain(|(u, ..)| *u != uri);
                ink.push((uri, Instant::now(), read));
            }
            INK_ASKING.store(false, Ordering::Release);
            crate::controls::read_again("Printers");
        });
        if spawned.is_err() {
            INK_ASKING.store(false, Ordering::Release);
        }
    }
    levels
}

/// Run `golem-printer-ink` on a printer's address, `INK_PATIENCE` at most.
fn read_ink(uri: &str) -> Option<Vec<(u32, u8)>> {
    let out = run_for(Command::new(INK_PROGRAM).arg(uri), INK_PATIENCE)?;
    let levels = parse_ink(&out);
    (!levels.is_empty()).then_some(levels)
}

/// A program's output if it ends well within `patience` (else it is ended).
fn run_for(cmd: &mut Command, patience: Duration) -> Option<String> {
    let mut child = cmd.stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::null()).spawn().ok()?;
    let reader = child.stdout.take().map(|mut o| {
        std::thread::spawn(move || {
            let mut text = String::new();
            let _ = std::io::Read::read_to_string(&mut o, &mut text);
            text
        })
    });
    let start = Instant::now();
    let ok = loop {
        match child.try_wait() {
            Ok(Some(status)) => break status.success(),
            Ok(None) if start.elapsed() < patience => std::thread::sleep(Duration::from_millis(50)),
            _ => {
                let _ = child.kill();
                let _ = child.wait();
                break false;
            }
        }
    };
    let text = reader.and_then(|r| r.join().ok()).unwrap_or_default();
    ok.then_some(text)
}

/// A scanner as people call it: SANE's words less its plumbing
/// ("eSCL Canon MF240 ip=192.168.1.40" → "Canon MF240").
fn scanner_name(what: &str) -> String {
    let mut w = what.split(" ip=").next().unwrap_or(what).trim();
    for lead in ["eSCL ", "WSD "] {
        w = w.strip_prefix(lead).unwrap_or(w);
    }
    w.replace('_', " ")
}

/// The scanners there are, each with what scans with it.
fn scanner_part(view: &mut ControlView, found: Vec<crate::scanners::Scanner>) {
    let app = crate::scanners::SCAN_PROGRAMS.iter().any(|p| crate::launch::on_path(p));
    for s in found {
        let act = if app { choice(tr("Scan"), false, s.device.clone()) } else { choice(tr("Install Simple Scan"), false, "install") };
        let key = if app { "printers-scan" } else { "printers-scan-app" };
        view.groups.push(Group { caption: Some(scanner_name(&s.what)), ..words("Scanner", key, vec![act]) });
    }
}

/// A printer added by its address: the field, and how the last try went.
fn add_part(view: &mut ControlView) {
    if !cups::present() {
        return;
    }
    let said = ADDING.lock().ok().and_then(|a| a.clone()).and_then(|a| match a {
        Adding::Trying(at) => Some(fill(tr("Looking for a printer at {address}…"), &[("address", &at)])),
        Adding::Failed(at, when) => (when.elapsed() < ASKED_FOR).then(|| fill(tr("No printer answered at {address}."), &[("address", &at)])),
    });
    if let Some(said) = said {
        view.groups.push(Group { centred: true, ..line_of(said) });
    }
    view.groups.push(field_of("printers-add", "", tr("Add a printer by its address"), 260.0));
}

/// The address typed in the field (Enter): tried off every loop, the page
/// read again with the outcome — the new printer shown, or why not.
pub(crate) fn add_typed(address: &str) {
    let address = address.trim().to_owned();
    if address.is_empty() {
        return;
    }
    if let Ok(mut a) = ADDING.lock() {
        *a = Some(Adding::Trying(address.clone()));
    }
    let spawned = std::thread::Builder::new().name("printer-add".into()).spawn(move || {
        let outcome = add_printer(&address);
        if let Ok(mut a) = ADDING.lock() {
            *a = match &outcome {
                Ok(_) => None,
                Err(why) => {
                    warn!("printers: none added at {address}: {why}");
                    Some(Adding::Failed(address.clone(), Instant::now()))
                }
            };
        }
        if let Ok(name) = outcome {
            info!("printers: {name} added at {address}");
            if let Ok(mut picked) = PICKED.lock() {
                *picked = Some(name);
            }
        }
        stale();
        crate::controls::read_again("Printers");
    });
    if let Err(e) = spawned {
        warn!("printers: no thread to add a printer: {e}");
    }
}

/// The addresses a printer at `address` may answer on, in the order tried:
/// a whole URI as it is, else IPP Everywhere's own path, then the same
/// over TLS, then the older one.
fn addresses(address: &str) -> Vec<String> {
    let a = address.trim().trim_end_matches('/');
    if a.contains("://") {
        return vec![a.to_owned()];
    }
    vec![format!("ipp://{a}/ipp/print"), format!("ipps://{a}/ipp/print"), format!("ipp://{a}/ipp")]
}

/// A queue's name for a printer at `address` ("Printer-192-168-1-40"), not
/// one CUPS has already.
fn queue_name(address: &str, taken: &[String]) -> String {
    let host = address.split("://").last().unwrap_or(address).split('/').next().unwrap_or(address);
    let mut base: String = host.chars().map(|c| if c.is_ascii_alphanumeric() { c } else { '-' }).collect();
    base = base.trim_matches('-').chars().take(40).collect();
    let base = format!("Printer-{base}");
    (1..)
        .map(|n| if n == 1 { base.clone() } else { format!("{base}-{n}") })
        .find(|n| !taken.iter().any(|t| t.eq_ignore_ascii_case(n)))
        .unwrap_or(base)
}

/// Make a queue for the printer at `address`, driverless: the new queue's
/// name, or why none was made.
fn add_printer(address: &str) -> Result<String, String> {
    let taken: Vec<String> = cups::snapshot().map(|s| s.printers.into_iter().map(|p| p.name).collect()).unwrap_or_default();
    let name = queue_name(address, &taken);
    for uri in addresses(address) {
        let made = run_for(Command::new("lpadmin").args(["-p", &name, "-E", "-v", &uri, "-m", "everywhere"]), ADD_PATIENCE);
        if made.is_none() {
            continue;
        }
        // Called what it says it is (lpadmin read its make and model).
        let model = cups::snapshot().ok().and_then(|s| s.printer(&name).map(|p| model_name(&p.model))).unwrap_or_default();
        if !model.is_empty() {
            run_for(Command::new("lpadmin").args(["-p", &name, "-D", &model]), Duration::from_secs(8));
        }
        return Ok(name);
    }
    Err(format!("nothing answered as a printer on {}", addresses(address).join(", ")))
}

/// A pick on the page (`printers-*`), on the controls' worker.
pub(crate) fn apply(key: &str, value: &str) {
    // (The sections and Scan need no printer.)
    if key == "printers-tab" {
        ON_SCANNERS.store(value == "scanners", Ordering::Relaxed);
        return;
    }
    if key == "printers-scan" {
        scan_with(value);
        return;
    }
    // (Whatever is done, what CUPS said before is old.)
    stale();
    let Some(name) = SHOWN.lock().ok().and_then(|p| p.clone()) else {
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
            "remove" => {
                if let Ok(mut r) = REMOVING.lock() {
                    *r = Some((name.clone(), Instant::now()));
                }
                Ok(())
            }
            "default" => cups::set_default(&name),
            "pause" => cups::pause_printer(&name),
            "resume" => cups::resume_printer(&name),
            "test" => cups::print_bytes(&name, test_page(&name).into_bytes(), tr("Golem test page")).map(|_| ()),
            _ => Ok(()),
        },
        "printers-remove" => {
            let asked = REMOVING.lock().ok().and_then(|mut r| r.take());
            match (value, asked) {
                ("yes", Some((n, at))) if n == name && at.elapsed() < ASKED_FOR => {
                    if let Ok(mut picked) = PICKED.lock() {
                        *picked = None;
                    }
                    cups::delete_printer(&name)
                }
                _ => Ok(()),
            }
        }
        _ => Ok(()),
    };
    match done {
        Ok(()) => info!("printers: {key} = {value} on {name}"),
        Err(e) => warn!("printers: {key} = {value} on {name}: {e}"),
    }
}

/// SCAN: the scanning app on that scanner (Simple Scan takes it as its
/// argument; another app opens on its own).
fn scan_with(device: &str) {
    let Some(app) = crate::scanners::SCAN_PROGRAMS.iter().find(|p| crate::launch::on_path(p)) else {
        return;
    };
    let exec = if *app == "simple-scan" { format!("simple-scan {}", crate::launch::shell_quote(device)) } else { (*app).to_owned() };
    if let Err(e) = crate::launch::launch(&exec, false, "") {
        warn!("printers: {exec}: {e}");
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
    fn hp_ink_lines_are_kind_and_percent() {
        let out = "warning: python-dbus not installed.\n1\t60\n2\t35\nnonsense\n4\t130\n";
        assert_eq!(parse_ink(out), vec![(1, 60), (2, 35), (4, 100)]);
        assert_eq!(ink_name(1), "Black");
        assert_eq!(ink_name(2), "Colour");
        assert_eq!(ink_name(99), "Ink");
    }

    #[test]
    fn a_driverless_printer_is_called_by_its_make_and_model() {
        assert_eq!(model_name("Deskjet 1200 Series, hpcups 3.26.4 - IPP Everywhere"), "Deskjet 1200 Series");
        assert_eq!(model_name("HP OfficeJet Pro 9010 series - IPP Everywhere"), "HP OfficeJet Pro 9010 series");
        assert_eq!(short("HP DeskJet 1200 series"), "HP DeskJet 1200 series");
        assert_eq!(short("Canon imageCLASS MF743Cdw printer of the office"), "Canon imageCLASS MF743C…");
    }

    #[test]
    fn a_scanner_is_called_by_its_make_and_model() {
        assert_eq!(scanner_name("eSCL Canon MF240 ip=192.168.1.40"), "Canon MF240");
        assert_eq!(scanner_name("Hewlett-Packard DeskJet_2700 all-in-one"), "Hewlett-Packard DeskJet 2700 all-in-one");
    }

    #[test]
    fn an_address_is_tried_the_driverless_ways_and_named_once() {
        assert_eq!(
            addresses("192.168.1.40/"),
            vec!["ipp://192.168.1.40/ipp/print", "ipps://192.168.1.40/ipp/print", "ipp://192.168.1.40/ipp"]
        );
        assert_eq!(addresses("ipp://office.local/ipp/print"), vec!["ipp://office.local/ipp/print"]);
        assert_eq!(queue_name("192.168.1.40", &[]), "Printer-192-168-1-40");
        assert_eq!(queue_name("localhost:8631", &["printer-localhost-8631".into()]), "Printer-localhost-8631-2");
        assert_eq!(queue_name("ipp://office.local/ipp/print", &[]), "Printer-office-local");
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
