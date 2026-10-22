//! ANOTHER APP (Max, 2026-10-21): the menus' Open with page ends with
//! "Another app", a page of EVERY app of the computer that opens files —
//! an app whose desktop entry's `Exec` takes them (`%f`, `%F`, `%u`, `%U`) —
//! A→Z with its own icon, found by typing; and "Always use for <kind>",
//! which makes the app chosen the default for that kind of file
//! (`xdg-mime default`), undoably. What it asks of the system is here,
//! every call of it off the loop.

use std::path::{Path, PathBuf};
use std::process::Command;

use tracing::{error, info};

use crate::desktop::SendFor;
use crate::desktop_menu::OtherApp;
use crate::filer::Undo;
use crate::i18n::{fill, tr};
use crate::App;

/// What `program args` printed, trimmed (empty when it could not run).
fn said(program: &str, args: &[&str]) -> String {
    Command::new(program)
        .args(args)
        .output()
        .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_owned())
        .unwrap_or_default()
}

/// Whether a desktop entry's text has an `Exec` that takes files.
pub(crate) fn takes_files(entry: &str) -> bool {
    let mut in_entry = false;
    for line in entry.lines() {
        let line = line.trim();
        if line.starts_with('[') {
            in_entry = line == "[Desktop Entry]";
            continue;
        }
        if in_entry {
            if let Some(exec) = line.strip_prefix("Exec=") {
                return ["%f", "%F", "%u", "%U"].iter().any(|c| exec.contains(c));
            }
        }
    }
    false
}

/// The apps that open files, of `apps` (each its id, name, icon and
/// desktop file): A→Z by name, a web app of the dock's left out (it opens
/// a site, not a file). Reads every desktop file: call it off the loop.
pub(crate) fn openers(apps: Vec<(String, String, Option<String>, PathBuf)>) -> Vec<OtherApp> {
    let mut found: Vec<OtherApp> = apps
        .into_iter()
        .filter(|(id, ..)| !id.starts_with("webapp-"))
        .filter(|(.., file)| std::fs::read_to_string(file).is_ok_and(|t| takes_files(&t)))
        .map(|(id, name, icon, _)| OtherApp {
            name,
            pic: format!(
                "asset:{}",
                icon.unwrap_or_else(|| "application-x-executable".to_owned())
            ),
            id,
        })
        .collect();
    found.sort_by_key(|a| a.name.to_lowercase());
    found.dedup_by(|a, b| a.name == b.name && a.id == b.id);
    found
}

/// The kind of the file at `path` (its MIME type), if it can be told.
pub(crate) fn mime_of(path: &Path) -> Option<String> {
    let kind = said("xdg-mime", &["query", "filetype", &path.to_string_lossy()]);
    (!kind.is_empty() && kind.contains('/')).then_some(kind)
}

/// The kind `mime` is, in words, for "Always use for <kind>": its own
/// description (shared-mime-info), in the shell's language when it has
/// one — in English made plural ("PNG image" → "PNG images").
pub(crate) fn kind_of(mime: &str) -> Option<String> {
    let dirs = std::env::var("XDG_DATA_HOME")
        .ok()
        .into_iter()
        .chain(std::env::var("XDG_DATA_DIRS").ok())
        .flat_map(|d| d.split(':').map(PathBuf::from).collect::<Vec<_>>())
        .chain([PathBuf::from("/usr/share")]);
    let lang = crate::i18n::lang();
    for dir in dirs {
        let Ok(xml) = std::fs::read_to_string(dir.join("mime").join(format!("{mime}.xml"))) else {
            continue;
        };
        if let Some(tag) = &lang {
            if let Some(words) = comment(&xml, Some(tag)) {
                return Some(words);
            }
        }
        return comment(&xml, None).map(|w| plural(&w));
    }
    None
}

/// A mime XML's `<comment>`: the one of language `tag`, or the plain one.
fn comment(xml: &str, tag: Option<&str>) -> Option<String> {
    let open = match tag {
        Some(t) => format!("<comment xml:lang=\"{t}\">"),
        None => "<comment>".to_owned(),
    };
    let start = xml.find(&open)? + open.len();
    let end = xml[start..].find("</comment>")?;
    let words = xml[start..start + end].trim();
    (!words.is_empty()).then(|| unescape(words))
}

fn unescape(s: &str) -> String {
    s.replace("&amp;", "&")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&apos;", "'")
        .replace("&quot;", "\"")
}

/// An English kind in the plural: "PNG image" → "PNG images",
/// "PDF document" → "PDF documents"; one that already ends in s as it is.
pub(crate) fn plural(words: &str) -> String {
    if words.ends_with('s') {
        words.to_owned()
    } else {
        format!("{words}s")
    }
}

/// The app that opens `mime` now (its `<id>.desktop`), if one is set.
pub(crate) fn default_for(mime: &str) -> Option<String> {
    let id = said("xdg-mime", &["query", "default", mime]);
    (!id.is_empty()).then_some(id)
}

/// Make `desktop` (`<id>.desktop`) the app for `mime`. Whether it took.
pub(crate) fn set_default(desktop: &str, mime: &str) -> bool {
    Command::new("xdg-mime")
        .args(["default", desktop, mime])
        .status()
        .is_ok_and(|s| s.success())
        && default_for(mime).as_deref() == Some(desktop)
}

/// The app the owner chose for `mime` (`~/.config/mimeapps.list`'s
/// defaults: the file `xdg-mime default` writes), read at once — for what
/// may be asked on the loop (whether a choice can still be taken back).
pub(crate) fn chosen_default(mime: &str) -> Option<String> {
    chosen_line(mime)?
        .split(';')
        .map(str::trim)
        .find(|v| !v.is_empty())
        .map(str::to_owned)
}

/// The owner's whole choice for `mime` as written (`a.desktop;b.desktop;`:
/// the first, then the ones to try after it) — what a taking back puts
/// back, all of it.
pub(crate) fn chosen_line(mime: &str) -> Option<String> {
    let text = std::fs::read_to_string(mimeapps()).ok()?;
    let mut section = "";
    for line in text.lines() {
        let t = line.trim();
        if t.starts_with('[') {
            section = t;
        } else if section == "[Default Applications]" {
            if let Some((k, v)) = t.split_once('=') {
                if k.trim() == mime && !v.trim().is_empty() {
                    return Some(v.trim().to_owned());
                }
            }
        }
    }
    None
}

fn mimeapps() -> PathBuf {
    std::env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(std::env::var("HOME").unwrap_or_default()).join(".config"))
        .join("mimeapps.list")
}

/// The owner's own choice for `mime` taken away (none was set before it):
/// its line out of `~/.config/mimeapps.list`'s defaults, the rest as it is.
pub(crate) fn unset_default(mime: &str) -> bool {
    rewrite_mimeapps(|text| without_default(text, mime))
}

/// The owner's choice for `mime` put back as it was written (`line`: every
/// app of it, in its order — `xdg-mime default` would keep only one).
pub(crate) fn restore_default(mime: &str, line: &str) -> bool {
    rewrite_mimeapps(|text| with_default(text, mime, line)) || chosen_line(mime).as_deref() == Some(line)
}

/// `~/.config/mimeapps.list` made `change(text)`, written whole (a file
/// beside it, renamed over it). Where it is a LINK (dotfiles kept in a
/// repository), what it points at is written and the link stays a link —
/// the rename used to put a plain file in its place. Whether it changed.
fn rewrite_mimeapps(change: impl FnOnce(&str) -> String) -> bool {
    rewrite_at(&mimeapps(), change)
}

fn rewrite_at(file: &Path, change: impl FnOnce(&str) -> String) -> bool {
    let real = std::fs::canonicalize(file).unwrap_or_else(|_| file.to_path_buf());
    let Ok(text) = std::fs::read_to_string(&real) else {
        return false;
    };
    let kept = change(&text);
    if kept == text {
        return false;
    }
    let tmp = real.with_extension("list.golem-tmp");
    let written = std::fs::write(&tmp, &kept).is_ok()
        && std::fs::metadata(&real).and_then(|m| std::fs::set_permissions(&tmp, m.permissions())).is_ok()
        && std::fs::rename(&tmp, &real).is_ok();
    if !written {
        let _ = std::fs::remove_file(&tmp);
    }
    written
}

/// `text` (a mimeapps.list) with `mime=line` in `[Default Applications]`:
/// in place of its line there, or added to that section (made if missing).
pub(crate) fn with_default(text: &str, mime: &str, line: &str) -> String {
    let entry = format!("{mime}={line}");
    let mut section = String::new();
    let (mut out, mut done) = (Vec::new(), false);
    let mut lines = text.lines().peekable();
    while let Some(l) = lines.next() {
        let t = l.trim();
        if t.starts_with('[') {
            section = t.to_owned();
        } else if section == "[Default Applications]" && t.split_once('=').is_some_and(|(k, _)| k.trim() == mime) {
            if !done {
                out.push(entry.clone());
                done = true;
            }
            continue;
        }
        out.push(l.to_owned());
        // The section's end (the next one, or the file's): added there.
        let leaving = lines.peek().is_none_or(|n| n.trim().starts_with('['));
        if !done && section == "[Default Applications]" && leaving {
            // (Before blank lines that close the section.)
            let mut blanks = Vec::new();
            while out.last().is_some_and(|l: &String| l.trim().is_empty()) {
                blanks.push(out.pop().unwrap_or_default());
            }
            out.push(entry.clone());
            out.extend(blanks.into_iter().rev());
            done = true;
        }
    }
    if !done {
        if out.last().is_some_and(|l| !l.trim().is_empty()) {
            out.push(String::new());
        }
        out.push("[Default Applications]".to_owned());
        out.push(entry);
    }
    let mut kept = out.join("\n");
    if text.ends_with('\n') || text.is_empty() {
        kept.push('\n');
    }
    kept
}

/// `text` (a mimeapps.list) with `mime`'s line in `[Default Applications]`
/// taken out.
pub(crate) fn without_default(text: &str, mime: &str) -> String {
    let mut section = String::new();
    let mut out: Vec<&str> = Vec::new();
    for line in text.lines() {
        let t = line.trim();
        if t.starts_with('[') {
            section = t.to_owned();
        } else if section == "[Default Applications]"
            && t.split_once('=').is_some_and(|(k, _)| k.trim() == mime)
        {
            continue;
        }
        out.push(line);
    }
    let mut kept = out.join("\n");
    if text.ends_with('\n') {
        kept.push('\n');
    }
    kept
}

/// What making `app` the default for `kind` is called in the history.
pub(crate) fn said_as(kind: &str, app: &str) -> String {
    fill(
        tr("Opened {kind} with {app} from now on"),
        &[("kind", &kind), ("app", &app)],
    )
}

impl App {
    /// The dock's apps that have a desktop file (for `openers`, off the
    /// loop): each its id, name, icon and file.
    pub(crate) fn other_app_entries(&self) -> Vec<(String, String, Option<String>, PathBuf)> {
        self.entries
            .iter()
            .filter_map(|e| {
                Some((
                    e.id.clone(),
                    e.name.clone(),
                    e.icon.clone(),
                    e.path.clone()?,
                ))
            })
            .collect()
    }

    /// Open `path` with `app` (its command as the dock knows it). Whether
    /// it was started.
    pub(crate) fn open_with_other(&mut self, app: &OtherApp, path: &Path) -> bool {
        let Some(entry) = self.entries.iter().find(|e| e.id == app.id) else {
            return false;
        };
        let exec = format!(
            "{} {}",
            entry.exec,
            crate::launch::shell_quote(&path.to_string_lossy())
        );
        info!("other apps: {} opened with {}", path.display(), app.name);
        match crate::launch::launch(&exec, entry.needs_terminal, &self.config.launch.terminal) {
            Ok(()) => true,
            Err(e) => {
                error!("other apps: {} could not be started: {e:#}", app.name);
                false
            }
        }
    }

    /// `app` the default for `mime` (`kind` in words) from now on — the
    /// check was on: made off the loop, then kept in the history of the
    /// menu's owner (`whose`) to be taken back to the one before.
    pub(crate) fn other_app_default(
        &mut self,
        app: OtherApp,
        mime: String,
        kind: String,
        whose: SendFor,
    ) {
        let desktop = format!("{}.desktop", app.id);
        self.desktop_off_loop(
            move || {
                // (The owner's own choice before — none: the system's —
                // so taking it back gives exactly what was.)
                let was = chosen_line(&mime);
                let took = set_default(&desktop, &mime);
                (took, was, mime, desktop)
            },
            move |me, (took, was, mime, now)| {
                if !took {
                    crate::filer::say_quietly(fill(
                        tr("{app} could not be made the app for {kind}"),
                        &[("app", &app.name), ("kind", &kind)],
                    ));
                    return;
                }
                info!("other apps: {now} opens {mime} from now on (was {was:?})");
                let said = said_as(&kind, &app.name);
                let undo = Undo::Default {
                    mime,
                    kind: kind.clone(),
                    app: app.name.clone(),
                    was,
                    now,
                };
                match whose {
                    SendFor::Files(id) => me.filer_did(id, undo),
                    SendFor::Desktop => me.desktop_did(undo, said),
                }
            },
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_app_takes_files_by_its_exec() {
        assert!(takes_files(
            "[Desktop Entry]\nName=Gimp\nExec=gimp-2.10 %U\n"
        ));
        assert!(takes_files("[Desktop Entry]\nExec=evince %f\n"));
        assert!(!takes_files("[Desktop Entry]\nExec=htop\n"));
        // (An action's Exec is not the app's.)
        assert!(!takes_files(
            "[Desktop Entry]\nExec=app\n[Desktop Action new]\nExec=app %f\n"
        ));
    }

    #[test]
    fn a_kind_is_read_and_said_in_the_plural() {
        let xml = "<mime-type type=\"image/png\">\n  <comment>PNG image</comment>\n  <comment xml:lang=\"es\">imagen PNG</comment>\n</mime-type>";
        assert_eq!(comment(xml, None).as_deref(), Some("PNG image"));
        assert_eq!(comment(xml, Some("es")).as_deref(), Some("imagen PNG"));
        assert_eq!(comment(xml, Some("fr")), None);
        assert_eq!(plural("PNG image"), "PNG images");
        assert_eq!(plural("PDF document"), "PDF documents");
        assert_eq!(
            plural("Microsoft Word documents"),
            "Microsoft Word documents"
        );
    }

    #[test]
    fn a_choice_is_put_back_whole_where_it_belongs() {
        let text = "[Default Applications]\nimage/png=gimp.desktop\n\n[Added Associations]\nimage/png=gimp.desktop;\n";
        // Replaced in place, every app of it.
        assert_eq!(
            with_default(text, "image/png", "org.gnome.Loupe.desktop;gimp.desktop;"),
            "[Default Applications]\nimage/png=org.gnome.Loupe.desktop;gimp.desktop;\n\n[Added Associations]\nimage/png=gimp.desktop;\n"
        );
        // Added at the section's end (before the blank line that closes it).
        assert_eq!(
            with_default(text, "text/plain", "gedit.desktop;"),
            "[Default Applications]\nimage/png=gimp.desktop\ntext/plain=gedit.desktop;\n\n[Added Associations]\nimage/png=gimp.desktop;\n"
        );
        // No such section: made.
        assert_eq!(
            with_default("[Added Associations]\nx/y=a.desktop;\n", "x/y", "b.desktop;"),
            "[Added Associations]\nx/y=a.desktop;\n\n[Default Applications]\nx/y=b.desktop;\n"
        );
    }

    #[test]
    fn a_linked_mimeapps_list_stays_a_link() {
        let d = std::env::temp_dir().join(format!("wr-mime-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(d.join("dotfiles")).unwrap();
        let real = d.join("dotfiles/mimeapps.list");
        std::fs::write(&real, "[Default Applications]\nimage/png=gimp.desktop\n").unwrap();
        let link = d.join("mimeapps.list");
        std::os::unix::fs::symlink(&real, &link).unwrap();
        assert!(rewrite_at(&link, |t| without_default(t, "image/png")));
        assert!(std::fs::symlink_metadata(&link).unwrap().file_type().is_symlink(), "still a link");
        assert_eq!(std::fs::read_to_string(&real).unwrap(), "[Default Applications]\n");
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn a_default_is_taken_out_of_its_section_only() {
        let text = "[Default Applications]\nimage/png=gimp.desktop\ntext/plain=gedit.desktop\n[Added Associations]\nimage/png=gimp.desktop;\n";
        assert_eq!(
            without_default(text, "image/png"),
            "[Default Applications]\ntext/plain=gedit.desktop\n[Added Associations]\nimage/png=gimp.desktop;\n"
        );
    }
}
