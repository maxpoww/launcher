//! STARTUP APPS (Max, 2026-11-06, the Controls gap audit: what opens when
//! the session starts, a page every settings app has): Controls → STARTUP
//! APPS — the apps that start with the session, each lit while it does (a
//! click turns one off and on), and Add an app: the apps of the grid, most
//! used first, as cards; one picked starts with the session from then on.
//!
//! The list IS the freedesktop autostart folder (`~/.config/autostart/*.desktop`,
//! what the session's own autostart generator runs at login — Golem's
//! session is uwsm's, which runs `xdg-desktop-autostart.target`): an app
//! added is a desktop file written there with its own command, one turned
//! off keeps its file with `Hidden=true` (the standard way to leave an entry
//! out), so it is listed and can be turned on again. An app added is also
//! started now (Max's rule: a pick acts at once).

use std::path::PathBuf;

use crate::controls::{choice_of, group_of, line_of, ControlView, Group, UiFacts};
use crate::i18n::tr;

/// One entry of the folder.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct Entry {
    pub file: String,
    pub name: String,
    pub on: bool,
}

/// The Add cards are shown (a static: the page is read on the worker).
pub(crate) static ADDING: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// How many apps the Add cards show.
const CARDS: usize = 18;

pub(crate) fn dir() -> PathBuf {
    std::env::var_os("XDG_CONFIG_HOME")
        .map_or_else(|| PathBuf::from(std::env::var_os("HOME").unwrap_or_default()).join(".config"), PathBuf::from)
        .join("autostart")
}

/// The entries of the folder, by name.
pub(crate) fn entries() -> Vec<Entry> {
    let Ok(read) = std::fs::read_dir(dir()) else { return Vec::new() };
    let mut out: Vec<Entry> = read
        .flatten()
        .filter_map(|d| {
            let file = d.file_name().to_string_lossy().into_owned();
            if !file.ends_with(".desktop") {
                return None;
            }
            let text = std::fs::read_to_string(d.path()).ok()?;
            Some(parse(&file, &text))
        })
        .collect();
    out.sort_by_key(|e| e.name.to_lowercase());
    out
}

/// What a desktop file says: its name (localised lines left to the
/// plain one), and whether it is turned off.
pub(crate) fn parse(file: &str, text: &str) -> Entry {
    let mut name = None;
    let mut on = true;
    for line in text.lines() {
        let line = line.trim();
        if let Some(n) = line.strip_prefix("Name=") {
            name = Some(n.trim().to_owned());
        } else if line.eq_ignore_ascii_case("Hidden=true") || line.eq_ignore_ascii_case("X-GNOME-Autostart-enabled=false") {
            on = false;
        }
    }
    let name = name.filter(|n| !n.is_empty()).unwrap_or_else(|| file.trim_end_matches(".desktop").to_owned());
    Entry { file: file.to_owned(), name, on }
}

/// Turn an entry on or off: `Hidden=` rewritten (or added), the rest kept.
pub(crate) fn set_on(file: &str, on: bool) -> bool {
    set_on_in(&dir(), file, on)
}

fn set_on_in(dir: &std::path::Path, file: &str, on: bool) -> bool {
    if file.contains('/') || !file.ends_with(".desktop") {
        return false;
    }
    let path = dir.join(file);
    let Ok(text) = std::fs::read_to_string(&path) else { return false };
    let mut lines: Vec<String> = text
        .lines()
        .filter(|l| {
            let t = l.trim();
            !t.starts_with("Hidden=") && !t.starts_with("X-GNOME-Autostart-enabled=")
        })
        .map(str::to_owned)
        .collect();
    if !on {
        // (Right after the header, where every reader expects keys.)
        let at = lines.iter().position(|l| l.trim() == "[Desktop Entry]").map_or(0, |i| i + 1);
        lines.insert(at, "Hidden=true".to_owned());
    }
    crate::persist::write_text("startup app", &path, &(lines.join("\n") + "\n"));
    true
}

/// The desktop file an added app gets: its name, its own command, its icon.
pub(crate) fn file_for(id: &str, name: &str, exec: &str, icon: Option<&str>, terminal: bool) -> (String, String) {
    let file = if id.ends_with(".desktop") { id.to_owned() } else { format!("{id}.desktop") };
    let mut text = format!("[Desktop Entry]\nType=Application\nName={name}\nExec={exec}\n");
    if let Some(icon) = icon {
        text.push_str(&format!("Icon={icon}\n"));
    }
    if terminal {
        text.push_str("Terminal=true\n");
    }
    text.push_str("X-Golem-Startup=true\n");
    (file, text)
}

/// Write the file for an app of the grid. The UI thread calls it (the
/// entries are its): `App::control_pick`, key `startup-add`.
pub(crate) fn add(id: &str, name: &str, exec: &str, icon: Option<&str>, terminal: bool) {
    let (file, text) = file_for(id, name, exec, icon, terminal);
    let _ = std::fs::create_dir_all(dir());
    crate::persist::write_text("startup app", &dir().join(&file), &text);
    ADDING.store(false, std::sync::atomic::Ordering::Relaxed);
}

/// Controls → Startup apps.
pub(crate) fn view(view: &mut ControlView, ui: &UiFacts) {
    let list = entries();
    let on = list.iter().filter(|e| e.on).count();
    view.subtitle = match on {
        0 => tr("Nothing opens by itself when you sign in.").to_owned(),
        1 => tr("One app opens when you sign in.").to_owned(),
        n => crate::i18n::fill(tr("{n} apps open when you sign in."), &[("n", &n)]),
    };
    if !list.is_empty() {
        let words = list.iter().map(|e| choice_of(e.name.clone(), e.on, e.file.clone())).collect();
        view.groups.push(Group { labelled: true, ..group_of(Some("At sign-in"), "startup", words) });
    }
    if ADDING.load(std::sync::atomic::Ordering::Relaxed) {
        let listed: Vec<String> = list.iter().map(|e| e.file.clone()).collect();
        let cards: Vec<_> = ui
            .apps
            .iter()
            .filter(|(id, _, _)| !listed.iter().any(|f| f == id || f.trim_end_matches(".desktop") == id.trim_end_matches(".desktop")))
            .take(CARDS)
            .map(|(id, name, entry)| crate::controls::Choice { icon: Some(*entry), ..choice_of(name.clone(), false, id.clone()) })
            .collect();
        if cards.is_empty() {
            view.groups.push(Group { centred: true, ..line_of(tr("Every app already opens at sign-in.").to_owned()) });
        } else {
            view.groups.push(Group { centred: true, ..line_of(tr("Pick an app to open it at every sign-in.").to_owned()) });
            view.groups.push(Group { tiles: true, grid_rows: Some(3), ..group_of(None, "startup-add", cards) });
        }
        view.groups.push(Group { buttons: true, ..group_of(None, "startup-adding", vec![choice_of(tr("Cancel"), false, "off")]) });
    } else {
        view.groups.push(Group { buttons: true, ..group_of(None, "startup-adding", vec![choice_of(tr("Add an app…"), false, "on")]) });
    }
}

/// A pick on the worker. True when the key was this page's.
pub(crate) fn apply(key: &str, value: &str) -> bool {
    match key {
        "startup" => {
            if let Some(e) = entries().into_iter().find(|e| e.file == value) {
                set_on(&e.file, !e.on);
            }
        }
        "startup-adding" => ADDING.store(value == "on", std::sync::atomic::Ordering::Relaxed),
        _ => return false,
    }
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_desktop_file_is_read_and_turned_off() {
        let e = parse("x.desktop", "[Desktop Entry]\nName=X App\nName[es]=App X\nExec=x\n");
        assert_eq!(e, Entry { file: "x.desktop".into(), name: "X App".into(), on: true });
        let e = parse("y.desktop", "[Desktop Entry]\nHidden=true\nName=Y\n");
        assert!(!e.on);
        let e = parse("z.desktop", "[Desktop Entry]\nX-GNOME-Autostart-enabled=false\n");
        assert_eq!((e.name.as_str(), e.on), ("z", false));
        let (file, text) = file_for("org.app.desktop", "App", "app --flag", Some("app"), false);
        assert_eq!(file, "org.app.desktop");
        assert!(text.contains("Exec=app --flag\n") && text.contains("Icon=app\n") && !text.contains("Terminal"));
        assert_eq!(file_for("plain", "P", "p", None, true).0, "plain.desktop");
    }

    #[test]
    fn turning_off_writes_hidden_once() {
        let dir = std::env::temp_dir().join(format!("golem-startup-{}", std::process::id()));
        let auto = dir.join("autostart");
        let _ = std::fs::create_dir_all(&auto);
        std::fs::write(auto.join("a.desktop"), "[Desktop Entry]\nName=A\nExec=a\n").unwrap();
        assert!(set_on_in(&auto, "a.desktop", false));
        let text = std::fs::read_to_string(auto.join("a.desktop")).unwrap();
        assert_eq!(text, "[Desktop Entry]\nHidden=true\nName=A\nExec=a\n");
        assert!(set_on_in(&auto, "a.desktop", true));
        let text = std::fs::read_to_string(auto.join("a.desktop")).unwrap();
        assert_eq!(text, "[Desktop Entry]\nName=A\nExec=a\n");
        assert!(!set_on_in(&auto, "../a.desktop", true));
        let _ = std::fs::remove_dir_all(&dir);
    }
}
