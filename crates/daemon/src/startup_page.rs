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

/// What a desktop file says: its name (the plain `Name=` of its `[Desktop
/// Entry]` — an action's name is not the app's), and whether the session
/// starts it, read the way systemd's xdg-autostart generator reads it (what
/// Golem's session runs), so the lit pill is what happens at sign-in:
/// `Hidden=true`, an `OnlyShowIn`/`NotShowIn` that leaves this desktop out,
/// `X-systemd-skip`, a `TryExec` not found or no `Exec` mean it does not
/// start. `X-GNOME-Autostart-enabled` is GNOME's own: the generator ignores
/// it, so an entry "off" by it starts at every login (2026-11-09).
pub(crate) fn parse(file: &str, text: &str) -> Entry {
    let desktops: Vec<String> =
        std::env::var("XDG_CURRENT_DESKTOP").unwrap_or_default().split(':').filter(|d| !d.is_empty()).map(str::to_owned).collect();
    parse_in(file, text, &desktops, &found)
}

fn parse_in(file: &str, text: &str, desktops: &[String], found: &dyn Fn(&str) -> bool) -> Entry {
    let keys = entry_keys(text);
    let get = |k: &str| keys.iter().find(|(key, _)| key == k).map(|(_, v)| v.as_str());
    let yes = |k: &str| get(k).is_some_and(|v| v.eq_ignore_ascii_case("true"));
    let names_us = |k: &str| get(k).map(|v| v.split(';').any(|d| !d.is_empty() && desktops.iter().any(|x| x.eq_ignore_ascii_case(d))));
    let shown_here = names_us("OnlyShowIn") != Some(false) && names_us("NotShowIn") != Some(true);
    let runs = get("Exec").is_some_and(|e| !e.trim().is_empty()) && get("TryExec").is_none_or(|t| found(t.trim()));
    let on = !yes("Hidden") && !yes("X-systemd-skip") && shown_here && runs;
    let name = get("Name").map(str::trim).filter(|n| !n.is_empty()).map_or_else(|| file.trim_end_matches(".desktop").to_owned(), str::to_owned);
    Entry { file: file.to_owned(), name, on }
}

/// The keys of a desktop file's `[Desktop Entry]` group, the first of each.
fn entry_keys(text: &str) -> Vec<(String, String)> {
    let mut out: Vec<(String, String)> = Vec::new();
    let mut inside = false;
    for line in text.lines() {
        let t = line.trim();
        if t.starts_with('[') {
            inside = t == "[Desktop Entry]";
            continue;
        }
        if !inside || t.starts_with('#') {
            continue;
        }
        if let Some((k, v)) = t.split_once('=') {
            let k = k.trim();
            if !out.iter().any(|(key, _)| key == k) {
                out.push((k.to_owned(), v.trim().to_owned()));
            }
        }
    }
    out
}

/// A program the session would find: a path that is there, or a name on PATH.
fn found(program: &str) -> bool {
    if program.starts_with('/') {
        return std::path::Path::new(program).exists();
    }
    std::env::var_os("PATH").is_some_and(|p| std::env::split_paths(&p).any(|d| d.join(program).is_file()))
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
    // A stub that only hides the system's entry (no Exec of its own):
    // turned on, it goes, and the system's entry starts again.
    if on && !entry_keys(&text).iter().any(|(k, v)| k == "Exec" && !v.trim().is_empty()) {
        return std::fs::remove_file(&path).is_ok();
    }
    // Turned on, what keeps it from starting goes too; off is `Hidden=true`.
    let drop: &[&str] = if on {
        &["Hidden=", "X-GNOME-Autostart-enabled=", "OnlyShowIn=", "NotShowIn=", "X-systemd-skip="]
    } else {
        &["Hidden=", "X-GNOME-Autostart-enabled="]
    };
    let mut lines: Vec<String> = text
        .lines()
        .filter(|l| {
            let t = l.trim();
            !drop.iter().any(|d| t.starts_with(d))
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

/// The command an added app starts with at sign-in (the index's Exec, its
/// field codes gone): a program named by its store path
/// (`/nix/store/…-vlc/bin/vlc`) by its name when the session finds it — the
/// path is gone after the next update and its collection, and the generator
/// then starts nothing, quietly —; a literal `%` written `%%` (a field code
/// otherwise); a terminal app inside the terminal (the generator ignores
/// `Terminal=`), as the dock starts one (`launch::launch`).
pub(crate) fn exec_for(exec: &str, terminal: Option<&str>, found: &dyn Fn(&str) -> bool) -> String {
    let (first, rest) = exec.split_once(' ').map_or((exec, None), |(f, r)| (f, Some(r)));
    let program = match first.rsplit('/').next() {
        Some(name) if first.starts_with("/nix/store/") && found(name) => name,
        _ => first,
    };
    let line = rest.map_or_else(|| program.to_owned(), |r| format!("{program} {r}")).replace('%', "%%");
    match terminal {
        // (The desktop file's own quoting: inside double quotes \ " ` $ are
        // escaped with a backslash.)
        Some(term) => {
            let quoted: String = line.chars().flat_map(|c| if matches!(c, '\\' | '"' | '`' | '$') { vec!['\\', c] } else { vec![c] }).collect();
            format!("{term} sh -c \"{quoted}\"")
        }
        None => line,
    }
}

/// The desktop file an added app gets: its name, its command, its icon.
pub(crate) fn file_for(id: &str, name: &str, exec: &str, icon: Option<&str>) -> (String, String) {
    let file = if id.ends_with(".desktop") { id.to_owned() } else { format!("{id}.desktop") };
    let mut text = format!("[Desktop Entry]\nType=Application\nName={name}\nExec={exec}\n");
    if let Some(icon) = icon {
        text.push_str(&format!("Icon={icon}\n"));
    }
    text.push_str("X-Golem-Startup=true\n");
    (file, text)
}

/// Write the file for an app of the grid. The UI thread calls it (the
/// entries are its): `App::control_pick`, key `startup-add`. True when the
/// file is there.
pub(crate) fn add(id: &str, name: &str, exec: &str, icon: Option<&str>, terminal: Option<&str>) -> bool {
    let (file, text) = file_for(id, name, &exec_for(exec, terminal, &found), icon);
    let _ = std::fs::create_dir_all(dir());
    let path = dir().join(&file);
    crate::persist::write_text("startup app", &path, &text);
    ADDING.store(false, std::sync::atomic::Ordering::Relaxed);
    path.is_file()
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
            .startup_apps
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
    fn a_desktop_file_is_read_as_the_session_starts_it() {
        let here = ["Hyprland".to_owned()];
        let all = |_: &str| true;
        let read = |t: &str| parse_in("x.desktop", t, &here, &all);
        let e = read("[Desktop Entry]\nName=X App\nName[es]=App X\nExec=x\n[Desktop Action new]\nName=New Window\n");
        assert_eq!(e, Entry { file: "x.desktop".into(), name: "X App".into(), on: true });
        assert!(!read("[Desktop Entry]\nHidden=true\nName=Y\nExec=y\n").on);
        // GNOME's own switch: the generator starts it anyway.
        assert!(read("[Desktop Entry]\nExec=z\nX-GNOME-Autostart-enabled=false\n").on);
        assert!(!read("[Desktop Entry]\nExec=z\nOnlyShowIn=GNOME;KDE;\n").on);
        assert!(read("[Desktop Entry]\nExec=z\nOnlyShowIn=Hyprland;\n").on);
        assert!(!read("[Desktop Entry]\nExec=z\nNotShowIn=Hyprland;\n").on);
        assert!(!read("[Desktop Entry]\nExec=z\nX-systemd-skip=true\n").on);
        assert!(!parse_in("x.desktop", "[Desktop Entry]\nExec=z\nTryExec=zz\n", &here, &|_: &str| false).on);
        // A stub hiding the system's entry starts nothing.
        let e = read("[Desktop Entry]\nHidden=true\n");
        assert_eq!((e.name.as_str(), e.on), ("x", false));
    }

    #[test]
    fn an_added_app_starts_by_a_command_that_lasts() {
        let on_path = |n: &str| n == "vlc";
        assert_eq!(exec_for("/nix/store/abc-vlc-3/bin/vlc --started-from-file", None, &on_path), "vlc --started-from-file");
        // Not found by its name: the path stays (better than nothing).
        assert_eq!(exec_for("/nix/store/abc-x/bin/x", None, &on_path), "/nix/store/abc-x/bin/x");
        assert_eq!(exec_for("app --rate 50%", None, &on_path), "app --rate 50%%");
        assert_eq!(exec_for("htop", Some("foot"), &on_path), "foot sh -c \"htop\"");
        assert_eq!(exec_for("say \"$HOME\"", Some("foot"), &on_path), "foot sh -c \"say \\\"\\$HOME\\\"\"");
        let (file, text) = file_for("org.app.desktop", "App", "app --flag", Some("app"));
        assert_eq!(file, "org.app.desktop");
        assert!(text.contains("Exec=app --flag\n") && text.contains("Icon=app\n") && !text.contains("Terminal"));
        assert_eq!(file_for("plain", "P", "p", None).0, "plain.desktop");
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
        // On again, what kept it from this desktop goes; a stub goes whole.
        std::fs::write(auto.join("b.desktop"), "[Desktop Entry]\nExec=b\nOnlyShowIn=GNOME;\nHidden=true\n").unwrap();
        assert!(set_on_in(&auto, "b.desktop", true));
        assert_eq!(std::fs::read_to_string(auto.join("b.desktop")).unwrap(), "[Desktop Entry]\nExec=b\n");
        std::fs::write(auto.join("c.desktop"), "[Desktop Entry]\nHidden=true\n").unwrap();
        assert!(set_on_in(&auto, "c.desktop", true));
        assert!(!auto.join("c.desktop").exists());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
