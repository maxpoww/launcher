//! WHAT AN APP'S EXCEPTION CAN DO (Max, 2026-10-27: each app is offered
//! only the choices that really reach it — Windows greys out what cannot
//! apply, Android's per-app dark warns that apps may refuse).
//!
//! An exception reaches a program through its environment at launch
//! (`palette::skip_vars`): GTK_THEME for GTK (its makers' Adwaita, or the
//! pinned Golem-Dark/-Light), QT_QPA_PLATFORMTHEME cleared for Qt (its own
//! colours only: Qt's palette is one file for every program). Webapps go
//! through Seam. Every other program follows the Apps whatever is set for
//! it — KDE's read kdeglobals, Chromium and Electron paint their own, a
//! terminal's programs wear the terminal's, GIMP and Inkscape keep the theme
//! Golem set in their own settings, Seam is the Web section's — so it is
//! offered nothing.
//!
//! What a program is is read once per dock start, off the loop: behind the
//! Nix wrappers (scripts and binary ones), an ELF by what it links (the
//! dynamic section's NEEDED names, a few kilobytes) or a toolkit it loads as
//! a plug-in; a script by what it imports; failing those, the desktop file's
//! categories.

use std::collections::HashMap;
use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};

use tracing::{debug, info};
use waverunner_core::index::AppEntry;

/// The choices an app's exception has.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Caps {
    /// Its own colours, and a look of its own (GTK, webapps).
    Both,
    /// Its own colours only (Qt).
    OwnOnly,
    /// Nothing reaches it.
    None,
}

static CAPS: std::sync::Mutex<Option<HashMap<String, Caps>>> = std::sync::Mutex::new(None);

/// What `id` can be given; `None` while not known yet (all offered, as before).
pub(crate) fn of(id: &str) -> Option<Caps> {
    if id.starts_with("webapp-") {
        return Some(Caps::Both);
    }
    CAPS.lock().ok()?.as_ref()?.get(id).copied()
}

/// Learn every app's (a thread: the programs are read from disk).
pub(crate) fn learn(entries: &[AppEntry]) {
    let apps: Vec<(String, String, bool, Option<PathBuf>)> = entries
        .iter()
        .filter(|e| !e.id.starts_with("webapp-") && e.path.is_some())
        .map(|e| (e.id.clone(), e.exec.clone(), e.needs_terminal, e.path.clone()))
        .collect();
    let _ = std::thread::Builder::new().name("app-caps".into()).spawn(move || {
        let path = std::env::var("PATH").unwrap_or_default();
        let known: HashMap<String, Caps> = apps
            .into_iter()
            .map(|(id, exec, terminal, desktop)| {
                let caps = caps_of(&exec, terminal, &path, desktop.as_deref());
                debug!("app caps: {id} {caps:?}");
                (id, caps)
            })
            .collect();
        let count = |c: Caps| known.values().filter(|k| **k == c).count();
        info!("app caps: {} apps — {} own colours and look, {} own colours only, {} nothing reaches", known.len(), count(Caps::Both), count(Caps::OwnOnly), count(Caps::None));
        if let Ok(mut g) = CAPS.lock() {
            *g = Some(known);
        }
    });
}

/// Programs that keep the theme Golem wrote into their own settings, and
/// Seam (the Web section's).
const OWN_SETTINGS: [&str; 5] = ["gimp", "gimp-3.0", "gimp-2.10", "inkscape", "seam"];

fn caps_of(exec: &str, terminal: bool, path: &str, desktop: Option<&Path>) -> Caps {
    if terminal {
        return Caps::None;
    }
    let first = exec.split_whitespace().find(|w| !w.contains('=') && *w != "env").unwrap_or("");
    let name = first.rsplit('/').next().unwrap_or("");
    if name.is_empty() || OWN_SETTINGS.contains(&name) {
        return Caps::None;
    }
    let told = match which(first, path).and_then(|p| program_behind(&p)) {
        Some(Program::Elf(binary)) => by_links(&binary),
        Some(Program::Script(text)) => by_imports(&text),
        None => None,
    };
    // (Else what the app says of itself: its desktop file's categories.)
    told.unwrap_or_else(|| desktop.map_or(Caps::None, by_categories))
}

/// What an ELF tells by what it links (and, for a toolkit it loads as a
/// plug-in — VLC's Qt —, by what its package carries). None: it tells nothing.
fn by_links(binary: &Path) -> Option<Caps> {
    let needed = needed(binary)?;
    let links = |prefix: &str| needed.iter().any(|n| n.starts_with(prefix));
    // (Chromium and Electron carry their packs beside the program.)
    let chromium = links("libffmpeg.so") || binary.parent().is_some_and(|d| d.join("resources.pak").is_file());
    if chromium || links("libKF5") || links("libKF6") {
        Some(Caps::None)
    } else if links("libgtk-3.so") || links("libgtk-4.so") {
        Some(Caps::Both)
    } else if ["libQt5Widgets", "libQt6Widgets", "libQt5Gui", "libQt6Gui"].iter().any(|q| links(q)) || qt_plugin(binary) {
        Some(Caps::OwnOnly)
    } else {
        None
    }
}

/// A Qt interface loaded as a plug-in, in the program's own package
/// (`<package>/lib/…/libqt_plugin.so`, VLC's way).
fn qt_plugin(binary: &Path) -> bool {
    let Some(package) = binary.ancestors().find(|a| a.parent() == Some(Path::new("/nix/store"))) else { return false };
    let mut seen = 0;
    let mut dirs = vec![(package.join("lib"), 0)];
    while let Some((dir, depth)) = dirs.pop() {
        for e in std::fs::read_dir(&dir).into_iter().flatten().flatten() {
            seen += 1;
            if seen > 4000 {
                return false;
            }
            let p = e.path();
            if e.file_type().is_ok_and(|t| t.is_dir()) {
                if depth < 4 {
                    dirs.push((p, depth + 1));
                }
            } else if e.file_name().to_string_lossy().starts_with("libqt_plugin") {
                return true;
            }
        }
    }
    false
}

/// What a script tells by the toolkit it imports (Python's gi or PyQt, GJS).
fn by_imports(text: &str) -> Option<Caps> {
    let gtk = ["require_version('Gtk'", "require_version(\"Gtk\"", "repository import Gtk", "gi://Gtk", "imports.gi.Gtk"];
    if gtk.iter().any(|g| text.contains(g)) {
        Some(Caps::Both)
    } else if ["PyQt5", "PyQt6", "PySide2", "PySide6"].iter().any(|q| text.contains(q)) {
        Some(Caps::OwnOnly)
    } else {
        None
    }
}

/// What a desktop file says of its app (the freedesktop categories GTK,
/// GNOME, Qt, KDE).
fn by_categories(desktop: &Path) -> Caps {
    let text = std::fs::read_to_string(desktop).unwrap_or_default();
    let cats: Vec<&str> = text.lines().find_map(|l| l.strip_prefix("Categories=")).map(|c| c.split(';').collect()).unwrap_or_default();
    if cats.contains(&"KDE") {
        Caps::None
    } else if cats.contains(&"GTK") || cats.contains(&"GNOME") {
        Caps::Both
    } else if cats.contains(&"Qt") {
        Caps::OwnOnly
    } else {
        Caps::None
    }
}

/// The program's file: as written when absolute, else found on the PATH.
fn which(program: &str, path: &str) -> Option<PathBuf> {
    if program.starts_with('/') {
        return Some(PathBuf::from(program));
    }
    path.split(':').map(|d| Path::new(d).join(program)).find(|p| p.is_file())
}

/// What finally runs.
enum Program {
    Elf(PathBuf),
    /// A script run by its interpreter (Python, GJS…): its text.
    Script(String),
}

/// What runs behind `path`, through Nix's wrappers a few levels deep: a
/// wrapper SCRIPT (makeWrapper: a shell's `exec` of the last store path), a
/// wrapper PROGRAM (makeBinaryWrapper: a small ELF naming its `…-wrapped`).
fn program_behind(path: &Path) -> Option<Program> {
    let mut at = std::fs::canonicalize(path).ok()?;
    for _ in 0..6 {
        let bytes = read_head(&at, 1 << 20)?;
        if bytes.starts_with(b"\x7fELF") {
            match wrapped_in(&bytes, &at) {
                Some(next) => at = next,
                None => return Some(Program::Elf(at)),
            }
            continue;
        }
        if !bytes.starts_with(b"#!") {
            return None;
        }
        let text = String::from_utf8_lossy(&bytes).into_owned();
        let shell = text.lines().next().is_some_and(|l| l.contains("bash") || l.trim_end().ends_with("/sh") || l.contains("/sh "));
        if !shell {
            return Some(Program::Script(text));
        }
        // The last store path in the script that is a program: what it runs.
        let here = at.to_string_lossy().into_owned();
        at = text
            .split(|c: char| c.is_whitespace() || c == '"' || c == '\'')
            .filter(|w| w.starts_with("/nix/store/") && *w != here)
            .map(PathBuf::from)
            .rfind(|p| p.is_file() && !p.ends_with("bash") && !p.ends_with("sh"))?;
    }
    None
}

/// The program a binary wrapper runs: a store path ending `-wrapped` among
/// its strings (a real program names none).
fn wrapped_in(bytes: &[u8], at: &Path) -> Option<PathBuf> {
    let text = String::from_utf8_lossy(bytes);
    let here = at.to_string_lossy();
    text.split(|c: char| c == '\0' || c.is_whitespace() || c == '"')
        .filter(|w| w.starts_with("/nix/store/") && w.ends_with("-wrapped") && *w != here)
        .map(PathBuf::from)
        .find(|p| p.is_file())
}

/// Up to `max` bytes of a file.
fn read_head(path: &Path, max: u64) -> Option<Vec<u8>> {
    let mut b = Vec::new();
    std::fs::File::open(path).ok()?.take(max).read_to_end(&mut b).ok()?;
    Some(b)
}

/// The libraries an ELF64 (little-endian) asks for by name.
fn needed(path: &Path) -> Option<Vec<String>> {
    let mut f = std::fs::File::open(path).ok()?;
    let mut h = [0u8; 64];
    f.read_exact(&mut h).ok()?;
    if h[..4] != *b"\x7fELF" || h[4] != 2 || h[5] != 1 {
        return None;
    }
    let u16_at = |b: &[u8], o: usize| u16::from_le_bytes([b[o], b[o + 1]]);
    let u32_at = |b: &[u8], o: usize| u32::from_le_bytes([b[o], b[o + 1], b[o + 2], b[o + 3]]);
    let u64_at = |b: &[u8], o: usize| u64::from_le_bytes(b[o..o + 8].try_into().unwrap_or([0; 8]));
    let (shoff, shentsize, shnum) = (u64_at(&h, 0x28), u16_at(&h, 0x3A), u64::from(u16_at(&h, 0x3C)));
    if shentsize != 64 || shnum == 0 || shnum > 4096 {
        return None;
    }
    let mut sh = vec![0u8; (shnum * 64) as usize];
    f.seek(SeekFrom::Start(shoff)).ok()?;
    f.read_exact(&mut sh).ok()?;
    let sec = |i: u64| &sh[(i * 64) as usize..(i * 64 + 64) as usize];
    // SHT_DYNAMIC, and the string table it names (sh_link).
    let dynamic = (0..shnum).map(sec).find(|s| u32_at(s, 4) == 6)?;
    let (doff, dsize, link) = (u64_at(dynamic, 0x18), u64_at(dynamic, 0x20), u64::from(u32_at(dynamic, 0x28)));
    if link >= shnum || dsize > 1 << 20 {
        return None;
    }
    let strtab = sec(link);
    let (soff, ssize) = (u64_at(strtab, 0x18), u64_at(strtab, 0x20));
    if ssize > 4 << 20 {
        return None;
    }
    let read = |f: &mut std::fs::File, off: u64, len: u64| -> Option<Vec<u8>> {
        let mut b = vec![0u8; len as usize];
        f.seek(SeekFrom::Start(off)).ok()?;
        f.read_exact(&mut b).ok()?;
        Some(b)
    };
    let (dyn_bytes, strs) = (read(&mut f, doff, dsize)?, read(&mut f, soff, ssize)?);
    let mut out = Vec::new();
    for e in dyn_bytes.chunks_exact(16) {
        match u64_at(e, 0) {
            0 => break,
            // DT_NEEDED: an offset into the string table.
            1 => {
                let at = u64_at(e, 8) as usize;
                if let Some(s) = strs.get(at..) {
                    let end = s.iter().position(|&b| b == 0).unwrap_or(s.len());
                    out.push(String::from_utf8_lossy(&s[..end]).into_owned());
                }
            }
            _ => {}
        }
    }
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_program_is_offered_only_what_reaches_it() {
        let path = std::env::var("PATH").unwrap_or_default();
        // Run in a terminal, or one of the programs that keep their own settings: nothing.
        assert_eq!(caps_of("htop", true, &path, None), Caps::None);
        assert_eq!(caps_of("gimp %U", false, &path, None), Caps::None);
        assert_eq!(caps_of("env FOO=1 inkscape", false, &path, None), Caps::None);
        assert_eq!(caps_of("", false, &path, None), Caps::None);
        // This very test is an ELF: read for what it links (no toolkit: nothing).
        let me = std::env::current_exe().expect("the test's own program");
        assert!(needed(&me).is_some_and(|n| n.iter().any(|l| l.starts_with("libc.so"))), "{:?}", needed(&me));
        assert_eq!(caps_of(&me.to_string_lossy(), false, &path, None), Caps::None);
        assert_eq!(of("webapp-gmail"), Some(Caps::Both));
        // A script tells by what it imports; a desktop file by its categories.
        assert_eq!(by_imports("import gi\ngi.require_version('Gtk', '3.0')"), Some(Caps::Both));
        assert_eq!(by_imports("from PyQt5.QtWidgets import QApplication"), Some(Caps::OwnOnly));
        assert_eq!(by_imports("print('hi')"), None);
        let dir = std::env::temp_dir().join(format!("app-caps-{}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("a test folder");
        for (cats, want) in [("GNOME;GTK;Utility;", Caps::Both), ("Qt;KDE;Network;", Caps::None), ("Qt;AudioVideo;", Caps::OwnOnly), ("AudioVideo;", Caps::None)] {
            let f = dir.join("x.desktop");
            std::fs::write(&f, format!("[Desktop Entry]\nName=X\nCategories={cats}\n")).expect("a desktop file");
            assert_eq!(by_categories(&f), want, "{cats}");
        }
        let _ = std::fs::remove_dir_all(&dir);
    }
}
