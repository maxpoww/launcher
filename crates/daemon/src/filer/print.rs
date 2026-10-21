//! PRINTING (Max, 2026-10-21): offered only where it can happen — a
//! printer CUPS knows (`lpstat -e`), asked off the loop and kept for half a
//! minute (`filer_printers`; again as a window opens) — and done the
//! standard way, `lp` to the default destination (`lpstat -d`, else the
//! first): documents as they are, photos too (made a PNG first where CUPS
//! takes no such picture), office files made a PDF by LibreOffice first
//! (`soffice`, only when it is installed). What can be printed is
//! `smart::printable`.

use std::process::{Command, Stdio};

use super::smart::{sort_of, Sort};
use super::*;

/// What this computer can print: is there a printer, and is LibreOffice
/// there to make office files PDFs.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub(crate) struct Can {
    pub(crate) print: bool,
    pub(crate) office: bool,
}

/// How long an answer about the printers is kept.
const FRESH: Duration = Duration::from_secs(30);
/// The most a CUPS command is waited for.
const PATIENCE: Duration = Duration::from_secs(8);
/// The most LibreOffice is waited for, making one PDF (its first run sets
/// up its profile).
const OFFICE_PATIENCE: Duration = Duration::from_secs(120);

/// The printers as last asked: the answer, when, and whether it is being
/// asked now.
#[derive(Default)]
pub(crate) struct Printers {
    pub(super) can: Can,
    at: Option<Instant>,
    asking: bool,
}

/// Run `cmd` for at most `patience`, in the C locale (CUPS's words are
/// read): what it printed, or why it failed.
fn run(cmd: &mut Command, patience: Duration) -> Result<String, String> {
    let mut child = cmd
        .env("LC_ALL", "C")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| e.to_string())?;
    let start = Instant::now();
    loop {
        match child.try_wait() {
            Ok(Some(_)) => break,
            Ok(None) if start.elapsed() < patience => {
                std::thread::sleep(Duration::from_millis(20));
            }
            _ => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(tr("it took too long").to_owned());
            }
        }
    }
    let out = child.wait_with_output().map_err(|e| e.to_string())?;
    let said = |bytes: &[u8]| String::from_utf8_lossy(bytes).trim().to_owned();
    if out.status.success() {
        Ok(said(&out.stdout))
    } else {
        // (Its first line, without the program's name: "lp: The printer
        // or class does not exist." → "The printer or class does not exist.")
        let why = said(&out.stderr);
        let line = why.lines().next().unwrap_or_default();
        let line = line.split_once(": ").map_or(line, |(_, rest)| rest);
        Err(line.to_owned())
    }
}

/// The destinations CUPS knows, by name; none without its commands.
fn destinations() -> Vec<String> {
    if !crate::launch::on_path("lpstat") || !crate::launch::on_path("lp") {
        return Vec::new();
    }
    run(Command::new("lpstat").arg("-e"), PATIENCE)
        .map(|out| names_of(&out))
        .unwrap_or_default()
}

/// The destinations in `lpstat -e`'s words: one a line.
fn names_of(said: &str) -> Vec<String> {
    said.lines()
        .map(str::trim)
        .filter(|l| !l.is_empty())
        .map(str::to_owned)
        .collect()
}

/// The default destination in `lpstat -d`'s words ("system default
/// destination: <name>"; "no system default destination").
fn default_of(said: &str) -> Option<String> {
    said.lines()
        .find_map(|l| l.split_once("destination:"))
        .map(|(_, name)| name.trim().to_owned())
        .filter(|name| !name.is_empty())
}

/// What can be printed now (asked off the loop: CUPS may be slow).
fn can_now() -> Can {
    Can {
        print: !destinations().is_empty(),
        office: crate::launch::on_path("soffice"),
    }
}

/// Where a print goes: the default destination, else the first.
fn destination() -> Option<String> {
    let all = destinations();
    if all.is_empty() {
        return None;
    }
    run(Command::new("lpstat").arg("-d"), PATIENCE)
        .ok()
        .and_then(|said| default_of(&said))
        .or_else(|| all.into_iter().next())
}

/// The pictures CUPS prints as they are; any other is made a PNG first.
fn cups_takes(name: &str) -> bool {
    let end = name
        .rsplit_once('.')
        .map(|(_, e)| e.to_lowercase())
        .unwrap_or_default();
    matches!(
        end.as_str(),
        "jpg" | "jpeg" | "png" | "gif" | "tif" | "tiff" | "bmp"
    )
}

/// `file` as CUPS can take it, made in `scratch` where it must be: an
/// office file as a PDF (LibreOffice), a picture CUPS takes no such of as
/// a PNG. Else why not.
fn ready(file: &Path, scratch: &Path, n: usize) -> Result<PathBuf, String> {
    let name = file_name(file);
    match sort_of(&name) {
        Sort::Office => {
            // (Its own folder each, so two "report" files do not meet; its
            // own profile, so an open LibreOffice is not handed the job.)
            let out = scratch.join(n.to_string());
            let profile = crate::apps::cache_base().join("waverunner/print-office");
            run(
                Command::new("soffice")
                    .arg(format!(
                        "-env:UserInstallation=file://{}",
                        profile.display()
                    ))
                    .args([
                        "--headless",
                        "--norestore",
                        "--convert-to",
                        "pdf",
                        "--outdir",
                    ])
                    .arg(&out)
                    .arg(file),
                OFFICE_PATIENCE,
            )?;
            // (Its name with the ending put to .pdf: "notes.v2.docx" →
            // "notes.v2.pdf".)
            let stem = file.file_stem().unwrap_or_default().to_string_lossy();
            let pdf = out.join(format!("{stem}.pdf"));
            if pdf.is_file() {
                Ok(pdf)
            } else {
                Err(tr("LibreOffice could not make it a PDF").to_owned())
            }
        }
        Sort::Photo if !cups_takes(&name) => {
            let png = scratch.join(format!("{n}.png"));
            image::open(file)
                .ok()
                .and_then(|img| img.save_with_format(&png, image::ImageFormat::Png).ok())
                .map(|()| png)
                .ok_or_else(|| tr("this picture cannot be read here").to_owned())
        }
        _ => Ok(file.to_path_buf()),
    }
}

/// Print `files` on the default printer: the line to say, and what could
/// not be printed and why (said quietly).
pub(super) fn print(files: &[PathBuf]) -> (Option<String>, Option<String>) {
    let Some(dest) = destination() else {
        return (None, Some(tr("No printer is set up").to_owned()));
    };
    let scratch = std::env::temp_dir().join(format!(
        "golem-print-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |d| d.as_nanos())
    ));
    let _ = std::fs::create_dir_all(&scratch);
    let mut printed = Vec::new();
    let mut failed: Vec<(String, String)> = Vec::new();
    for (n, file) in files.iter().enumerate() {
        let name = file_name(file);
        let sent = ready(file, &scratch, n).and_then(|doc| {
            let mut lp = Command::new("lp");
            lp.args(["-d", &dest, "-t", &name]);
            // (Text is printed as text, whatever its ending says.)
            let end = doc
                .extension()
                .and_then(|e| e.to_str())
                .map(str::to_ascii_lowercase);
            if matches!(end.as_deref(), Some("txt" | "md")) {
                lp.args(["-o", "document-format=text/plain"]);
            }
            run(lp.arg("--").arg(&doc), PATIENCE)
        });
        match sent {
            Ok(_) => printed.push(file),
            Err(why) => failed.push((name, why)),
        }
    }
    // (`lp` has handed each file to CUPS: the copies can go.)
    let _ = std::fs::remove_dir_all(&scratch);
    let photos = printed
        .iter()
        .all(|p| sort_of(&file_name(p)) == Sort::Photo);
    let said = match printed.as_slice() {
        [] => None,
        [one] => Some(fill(tr("Printing “{name}”"), &[("name", &file_name(one))])),
        many if photos => Some(fill(tr("Printing {n} photos"), &[("n", &many.len())])),
        many => Some(fill(tr("Printing {n} documents"), &[("n", &many.len())])),
    };
    let why = match failed.as_slice() {
        [] => None,
        [(name, why)] => Some(fill(
            tr("“{name}” could not be printed: {why}"),
            &[("name", name), ("why", why)],
        )),
        [(_, why), ..] => Some(fill(
            tr("{n} documents could not be printed: {why}"),
            &[("n", &failed.len()), ("why", why)],
        )),
    };
    (said, why)
}

impl App {
    /// What can be printed: asked again off the loop when the answer is
    /// older than half a minute (`now`: whatever its age); the windows draw
    /// again when it changed. Never waited for.
    pub(super) fn filer_printers(&mut self, now: bool) {
        let printers = &mut self.filer.printers;
        if printers.asking || (!now && printers.at.is_some_and(|at| at.elapsed() < FRESH)) {
            return;
        }
        printers.asking = true;
        self.desktop_off_loop(can_now, |app, can: Can| {
            let printers = &mut app.filer.printers;
            printers.asking = false;
            printers.at = Some(Instant::now());
            if printers.can != can {
                info!(
                    "files: can print {}, office files {}",
                    can.print, can.office
                );
                printers.can = can;
                app.filer_redraw_all();
            }
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cups_words_are_read() {
        assert_eq!(
            names_of("Office_Inkjet\n\nLaser/draft\n"),
            ["Office_Inkjet", "Laser/draft"]
        );
        assert_eq!(
            default_of("system default destination: Office_Inkjet\n").as_deref(),
            Some("Office_Inkjet")
        );
        assert_eq!(default_of("no system default destination\n"), None);
        assert!(cups_takes("a.JPG") && cups_takes("b.png"));
        assert!(!cups_takes("c.heic") && !cups_takes("d.webp"));
    }
}
