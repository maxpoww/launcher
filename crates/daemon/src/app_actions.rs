//! THE LONE APP (Max, 2026-10-22: "cuando el resultado sea una sola app
//! quiero que se muestren más opciones … y desinstalar, con confirmación";
//! then, seeing VLC alone with most of the box empty: "mostrar la app
//! seleccionada en el medio, a la derecha las acciones, a la izquierda las
//! de sus ventanas abiertas, y abajo «Recientes en VLC»"). A search that
//! leaves ONE app in Apps lays the row out around it:
//!
//! - THE APP in the middle, under the title, the one selected — Enter opens
//!   it as ever.
//! - TO ITS RIGHT what can be done with it: its desktop file's own actions
//!   (`Actions=`: Chrome's New Incognito Window, Seam's New Private
//!   Window…), then Golem's own for apps it knows (`KNOWN`: VLC opens a
//!   disc or the copied link, OBS starts recording…), and — for an app the
//!   dock can take away (a package it installed, an installed webapp, never
//!   the system's own) — Uninstall, last.
//! - TO ITS LEFT, while it has windows open: Show all (the SPREAD of only
//!   its windows, from every workspace: waveview's `spread_app`) and, past
//!   it, Close all.
//! - UNDER IT, "Recent in <app>": the files it opened lately (the system's
//!   record, `recently-used.xbel`, by the app that opened each; and VLC's
//!   own list) as the Files row shows things, a full line at most; a click
//!   opens one in the app.
//!
//! Each side holds what fits beside the centred app; past that the app's own
//! actions give way from the end, Uninstall last of all.
//!
//! THE LOOK (Max's "3a", canvas https://claude.ai/artifact/9WByNs3AHU4wZNYCR9tUgi):
//! an action is its app's own tile — the plate, the app's icon at half its
//! light, its colours kept — with the action's SYMBOL big and white in the
//! middle over a soft shadow, and its name under it on up to two lines. The
//! symbols are the apps' own where Golem has them (Chrome's hat and glasses
//! and its window, from its resources; Seam's mask and window, from
//! Firefox's `omni.ja`), else one by what the action says, else an arrow.
//!
//! They are cells like any other (temporary entries: `act:<n>` for an
//! action, the file's path for a recent), so hover, Enter and clicks are the
//! grid's; the arrows walk the row and go down to the recents and back up to
//! the app (`app_actions_key`). An action starts the way its app does
//! (`launch::launch`: its graphics card, its terminal) and the box goes.
//! UNINSTALL ASKS ONCE: the first press turns its plate red and its name
//! "Again to uninstall"; a second uninstalls (`uninstall_app`). Escape, the
//! keys or the pointer leaving it, or `ARM_FOR` with nothing done put it back.

use std::collections::HashMap;
use std::path::PathBuf;
use std::time::{Duration, Instant};

use calloop::timer::{TimeoutAction, Timer};
use smithay_client_toolkit::seat::keyboard::Keysym;
use tracing::{error, info, warn};
use waverunner_core::index::AppEntry;

use crate::i18n::{fill, tr};
use crate::{apps, content, hypr, launch, surface, web_actions, webapps, App, KbSurface};

/// The id prefix of an action's cell (`act:<n>`, n its place in the list).
pub(crate) const PREFIX: &str = "act:";
/// How long Uninstall waits for its second press.
pub(crate) const ARM_FOR: Duration = Duration::from_secs(4);
/// How big the symbol's square is, of its tile (the symbol itself takes the
/// middle 16 of its 20 units, clear of the squircle's corners).
pub(crate) const SYMBOL: f32 = 0.675;
/// How much of its light the app's icon keeps under the symbol.
pub(crate) const DIM: f32 = 0.5;
/// The room opened under the app's row for the actions' second lines and
/// the recents' title (px at icon scale 1).
pub(crate) const RECENT_ROOM: f32 = 44.0;
/// How many cells a site's search cell's name may spread over (it stands
/// alone in its row).
pub(crate) const WIDE_LABEL: f32 = 3.0;

/// Chrome's own symbols (its resources' New Window and Incognito, BSD-3 like
/// Chromium) and Seam's (Firefox's `window.svg` and `privateBrowsing.svg`,
/// MPL 2.0), each in a 16-unit box, drawn white.
const CHROME_WINDOW: &str = r#"<path fill-rule="evenodd" clip-rule="evenodd" d="M1 3.5A1.5 1.5 0 0 1 2.5 2H13a1.5 1.5 0 0 1 1.5 1.5v9A1.5 1.5 0 0 1 13 14H2.5A1.5 1.5 0 0 1 1 12.5ZM2.5 5v7.5H13V5Z"/>"#;
const INCOGNITO: &str = r#"<path d="M10.75 9c-.83 0-1.55.46-1.94 1.13-.26-.08-.53-.13-.81-.13s-.55.05-.81.13A2.249 2.249 0 0 0 5.25 9C4.01 9 3 10.01 3 11.25s1.01 2.25 2.25 2.25c1.12 0 2.04-.82 2.21-1.89.17-.07.35-.11.54-.11.19 0 .37.05.54.11.17 1.07 1.1 1.89 2.21 1.89 1.24 0 2.25-1.01 2.25-2.25S11.99 9 10.75 9Zm-5.5 3c-.41 0-.75-.34-.75-.75s.34-.75.75-.75.75.34.75.75-.34.75-.75.75Zm5.5 0c-.41 0-.75-.34-.75-.75s.34-.75.75-.75.75.34.75.75-.34.75-.75.75Zm.87-5.5-1.41-4a.763.763 0 0 0-.43-.45.787.787 0 0 0-.62.02L8 2.65l-1.16-.58a.764.764 0 0 0-1.05.43l-1.41 4H2V8h12V6.5h-2.38Zm-5.65 0 .95-2.7.74.37c.21.11.46.11.67 0l.74-.37.95 2.7H5.97Z"/>"#;
const SEAM_WINDOW: &str = r#"<path d="M13 1 3 1a2 2 0 0 0-2 2l0 9a2 2 0 0 0 2 2l10 0a2 2 0 0 0 2-2l0-9a2 2 0 0 0-2-2zM2.85 2.25l10.3 0 .6.6 0 2.15-11.5 0 0-2.15.6-.6zm10.3 10.5-10.3 0-.6-.6 0-5.9 11.5 0 0 5.9-.6.6z"/>"#;
const MASK: &str = r#"<path d="M15.931 5.479c-1.525-2.001-4.517-2.257-6.381-.545l-1.043.958-1.02 0-1.042-.958C4.581 3.223 1.589 3.478.064 5.479c-.197.78.108 3.66.28 4.113.347 1.83 1.842 3.206 3.635 3.206.895 0 1.706-.357 2.347-.929l.391-.334a1.936 1.936 0 0 1 2.511-.021l.586.486 0-.003c.617.497 1.375.801 2.203.801 1.792 0 3.288-1.376 3.635-3.206.172-.453.488-3.321.279-4.113zM6.232 8.452c-.427.482-1.088.792-1.832.792-.744 0-1.405-.31-1.832-.792a.693.693 0 0 1 0-.905c.427-.482 1.088-.792 1.832-.792.744 0 1.405.31 1.832.792a.693.693 0 0 1 0 .905zm7.2 0c-.427.482-1.088.792-1.832.792-.744 0-1.405-.31-1.832-.792a.693.693 0 0 1 0-.905c.427-.482 1.088-.792 1.832-.792.744 0 1.405.31 1.832.792a.693.693 0 0 1 0 .905z"/>"#;
/// Golem's own, in its strokes' manner: a disc, a link, a note, the
/// spread's four windows, a cross, a dot for recording.
const DISC: &str = r#"<circle cx="8" cy="8" r="6.6"/><circle cx="8" cy="8" r="1.9"/><path d="M8 3.6a4.4 4.4 0 0 1 4.4 4.4"/>"#;
const LINK: &str = r#"<path d="M6.4 9.6l3.2-3.2M7.3 4.5l1.2-1.2a2.9 2.9 0 0 1 4.2 4.2l-1.2 1.2M8.7 11.5l-1.2 1.2a2.9 2.9 0 0 1-4.2-4.2l1.2-1.2"/>"#;
const MUSIC: &str = r#"<path d="M6 12.2V3.8l7.2-1.6v8.4"/><circle cx="4.2" cy="12.3" r="1.8"/><circle cx="11.4" cy="10.7" r="1.8"/>"#;
const SPREAD: &str = r#"<rect x="1.6" y="2.6" width="5.6" height="4.6" rx="1"/><rect x="8.8" y="2.6" width="5.6" height="4.6" rx="1"/><rect x="1.6" y="8.8" width="5.6" height="4.6" rx="1"/><rect x="8.8" y="8.8" width="5.6" height="4.6" rx="1"/>"#;
const CLOSE: &str = r#"<path d="M3.5 3.5l9 9M12.5 3.5l-9 9"/>"#;
const RECORD: &str = r#"<circle cx="8" cy="8" r="5.2"/>"#;
/// The web's (`web_actions.rs`): a chat, a heart, a cart, a bell, a
/// compass, a star, a person, a bookmark, a parcel.
const CHAT: &str = r#"<path d="M2.5 4.2c0-.9.7-1.6 1.6-1.6h7.8c.9 0 1.6.7 1.6 1.6v5c0 .9-.7 1.6-1.6 1.6H7.2l-3.1 2.6v-2.6h0c-.9 0-1.6-.7-1.6-1.6z"/>"#;
const HEART: &str = r#"<path d="M8 13.2S2.2 9.8 2.2 5.9A2.9 2.9 0 0 1 8 4.6a2.9 2.9 0 0 1 5.8 1.3C13.8 9.8 8 13.2 8 13.2z"/>"#;
const CART: &str = r#"<path d="M1.8 2.6h1.9l1.5 7.4h7l1.4-5.2H4.4"/><circle cx="6" cy="12.6" r="1"/><circle cx="11.4" cy="12.6" r="1"/>"#;
const BELL: &str = r#"<path d="M4 11.2V7.4a4 4 0 0 1 8 0v3.8l1.2 1.2H2.8z"/><path d="M6.6 13.6a1.5 1.5 0 0 0 2.8 0"/>"#;
const COMPASS: &str = r#"<circle cx="8" cy="8" r="6.2"/><path d="M10.6 5.4 9.1 9.1 5.4 10.6 6.9 6.9z"/>"#;
const STAR: &str = r#"<path d="M8 2.2l1.8 3.7 4 .6-2.9 2.8.7 4L8 11.4l-3.6 1.9.7-4-2.9-2.8 4-.6z"/>"#;
const PERSON: &str = r#"<circle cx="8" cy="5.4" r="2.6"/><path d="M2.8 13.6c.6-2.6 2.7-4.2 5.2-4.2s4.6 1.6 5.2 4.2"/>"#;
const BOOKMARK: &str = r#"<path d="M4.2 2.4h7.6v11.2L8 10.8l-3.8 2.8z"/>"#;
const PACKAGE: &str = r#"<path d="M8 1.9l5.6 2.9v6.4L8 14.1l-5.6-2.9V4.8z"/><path d="M2.4 4.8 8 7.7l5.6-2.9M8 7.7v6.4"/>"#;

/// Every symbol an action can wear, in the order of their texture layers.
const SYMBOLS: &[(&str, Symbol)] = &[
    ("window", Symbol::Filled(CHROME_WINDOW)),
    ("incognito", Symbol::Filled(INCOGNITO)),
    ("seam-window", Symbol::Filled(SEAM_WINDOW)),
    ("mask", Symbol::Filled(MASK)),
    ("trash", Symbol::Glyph("trash")),
    ("home", Symbol::Glyph("home")),
    ("computer", Symbol::Glyph("pc")),
    ("terminal", Symbol::Glyph("term")),
    ("open", Symbol::Glyph("open")),
    ("picture", Symbol::Glyph("pic")),
    ("camera", Symbol::Glyph("camera")),
    ("disc", Symbol::Lines(DISC)),
    ("link", Symbol::Lines(LINK)),
    ("music", Symbol::Lines(MUSIC)),
    ("spread", Symbol::Lines(SPREAD)),
    ("close", Symbol::Lines(CLOSE)),
    ("record", Symbol::Filled(RECORD)),
    ("chat", Symbol::Lines(CHAT)),
    ("heart", Symbol::Lines(HEART)),
    ("cart", Symbol::Lines(CART)),
    ("bell", Symbol::Lines(BELL)),
    ("compass", Symbol::Lines(COMPASS)),
    ("star", Symbol::Lines(STAR)),
    ("person", Symbol::Lines(PERSON)),
    ("bookmark", Symbol::Lines(BOOKMARK)),
    ("package", Symbol::Lines(PACKAGE)),
    ("clock", Symbol::Glyph("clock")),
    ("list", Symbol::Glyph("list")),
    ("play", Symbol::Glyph("play")),
    ("plus", Symbol::Glyph("plus")),
    ("pen", Symbol::Glyph("pen")),
    ("send", Symbol::Glyph("send")),
    ("search", Symbol::Glyph("search")),
];

#[derive(Clone, Copy)]
enum Symbol {
    /// A filled path of its own.
    Filled(&'static str),
    /// Strokes of its own.
    Lines(&'static str),
    /// One of Golem's own strokes, by its name in `glyphs::GLYPHS`.
    Glyph(&'static str),
}

impl Symbol {
    fn svg(self) -> Option<String> {
        let body = match self {
            Symbol::Filled(body) => {
                return Some(format!(
                    r##"<svg xmlns="http://www.w3.org/2000/svg" width="20" height="20" viewBox="-2 -2 20 20" fill="#fff">{body}</svg>"##
                ))
            }
            Symbol::Lines(body) => body,
            Symbol::Glyph(name) => crate::glyphs::GLYPHS.iter().find(|(n, _)| *n == name)?.1,
        };
        Some(format!(
            r##"<svg xmlns="http://www.w3.org/2000/svg" width="20" height="20" viewBox="-2 -2 20 20" fill="none" stroke="#fff" stroke-width="1.5" stroke-linecap="round" stroke-linejoin="round">{body}</svg>"##
        ))
    }
}

/// A symbol's place in `SYMBOLS` (the open arrow for one not there).
pub(crate) fn symbol(name: &str) -> usize {
    let at = |name: &str| SYMBOLS.iter().position(|(n, _)| *n == name);
    at(name).or_else(|| at("open")).unwrap_or(0)
}

/// Whether `SYMBOLS` has one of that name.
#[cfg(test)]
pub(crate) fn has_symbol(name: &str) -> bool {
    SYMBOLS.iter().any(|(n, _)| *n == name)
}

/// The symbols' tiles, one texture layer each in `SYMBOLS`' order (put
/// after the entries' icons as the dock's other figures are).
pub(crate) fn figures() -> Option<Vec<Vec<u8>>> {
    SYMBOLS.iter().map(|(_, s)| apps::svg_figure(&s.svg()?)).collect()
}

/// Which symbol a desktop action of `app` wears, by its id and name: the
/// browsers' own for a new window and a private one, then by what it says,
/// else the open arrow.
fn symbol_for(app: &str, id: &str, name: &str) -> usize {
    let (app, said) = (app.to_lowercase(), format!("{} {}", id.to_lowercase(), name.to_lowercase()));
    let seam = app.contains("seam") || app.contains("firefox");
    let says = |words: &[&str]| words.iter().any(|w| said.contains(w));
    if says(&["private", "incognito", "privad", "incógnito"]) {
        return symbol(if seam { "mask" } else { "incognito" });
    }
    if says(&["window", "ventana"]) {
        return symbol(if seam { "seam-window" } else { "window" });
    }
    let by_word = [
        ("home", "home"),
        ("trash", "trash"),
        ("papelera", "trash"),
        ("computer", "computer"),
        ("equipo", "computer"),
        ("terminal", "terminal"),
    ];
    by_word.iter().find(|(w, _)| said.contains(w)).map_or(symbol("open"), |(_, s)| symbol(s))
}

/// GOLEM'S OWN ACTIONS for apps it knows (their desktop files bring none):
/// the apps (desktop ids, or their last dotted part), the action's name (in
/// English, said in the shell's language), its command (run by `sh -c`)
/// and its symbol.
struct Known {
    apps: &'static [&'static str],
    name: &'static str,
    exec: &'static str,
    symbol: &'static str,
}

const KNOWN: &[Known] = &[
    Known { apps: &["vlc"], name: "Open disc", exec: "vlc dvd://", symbol: "disc" },
    Known { apps: &["vlc"], name: "Play copied link", exec: r#"vlc "$(wl-paste -n)""#, symbol: "link" },
    Known { apps: &["vlc"], name: "Play Music", exec: r#"vlc "$(xdg-user-dir MUSIC)""#, symbol: "music" },
    Known {
        apps: &["mpv"],
        name: "Play copied link",
        exec: r#"mpv --force-window=immediate "$(wl-paste -n)""#,
        symbol: "link",
    },
    Known {
        apps: &["mpv"],
        name: "Play Music",
        exec: r#"mpv --force-window=immediate --shuffle "$(xdg-user-dir MUSIC)""#,
        symbol: "music",
    },
    Known {
        apps: &["gimp"],
        name: "Open copied image",
        exec: r#"f="$(mktemp --suffix=.png)" && wl-paste -t image/png > "$f" && gimp "$f""#,
        symbol: "picture",
    },
    Known { apps: &["studio", "obs"], name: "Start recording", exec: "obs --startrecording", symbol: "record" },
    Known { apps: &["studio", "obs"], name: "Start virtual camera", exec: "obs --startvirtualcam", symbol: "camera" },
];

/// Golem's own actions for the app of desktop id `id`.
fn known_for(id: &str) -> impl Iterator<Item = &'static Known> + '_ {
    let id = id.to_lowercase();
    let last = id.rsplit('.').next().unwrap_or(&id).to_owned();
    KNOWN.iter().filter(move |k| k.apps.iter().any(|a| *a == id || *a == last))
}

/// A name on two lines under its tile, broken between WORDS: as many as
/// fit on the first, the rest on the second (cut with "…" if still too
/// long); a first word too long for the line is broken as Files breaks a
/// name (`filer::two_lines_of_words`).
pub(crate) fn by_words(text: &str, room: f32, measure: &mut dyn FnMut(&str) -> f32) -> (String, String) {
    let words: Vec<&str> = text.split_whitespace().collect();
    let fits = |n: usize, measure: &mut dyn FnMut(&str) -> f32| measure(&words[..n].join(" ")) <= room - crate::desktop::FIT_SLACK;
    let mut n = 0;
    while n < words.len() && fits(n + 1, measure) {
        n += 1;
    }
    if words.len() == 1 && measure(text) > room - crate::desktop::FIT_SLACK {
        return hyphenated(text, room, measure);
    }
    if n == 0 || n == words.len() {
        return crate::filer::two_lines_of_words(text, room, measure);
    }
    (words[..n].join(" "), crate::desktop::fit_label(&words[n..].join(" "), room, measure))
}

/// One word too long for its line, broken with a hyphen: as much as fits
/// with it on the first line, the rest on the second (cut with "…").
fn hyphenated(word: &str, room: f32, measure: &mut dyn FnMut(&str) -> f32) -> (String, String) {
    let chars: Vec<char> = word.chars().collect();
    let mut k = chars.len().saturating_sub(1);
    while k > 1 && measure(&format!("{}-", chars[..k].iter().collect::<String>())) > room - crate::desktop::FIT_SLACK {
        k -= 1;
    }
    let (head, tail): (String, String) = (chars[..k].iter().collect(), chars[k..].iter().collect());
    (format!("{head}-"), crate::desktop::fit_label(&tail, room, measure))
}

// ---- the exact name ------------------------------------------------------------

/// A name as typed to find it: lower case, and Golem's own also without
/// "Golem" ("Golem Files" is found as "files").
fn spoken(name: &str) -> [String; 2] {
    let n = name.trim().to_lowercase();
    let bare = n.strip_prefix("golem ").unwrap_or(&n).to_owned();
    [n, bare]
}

/// THE EXACT NAME (Max, 2026-10-22: "'googl' muestra la grilla, agrego la e
/// queda solo Google, agrego un espacio y aparece la grilla sin Google"):
/// which of the apps named `names` (used `used` much) what was typed leaves —
/// `None`: all of them, as ranked.
/// - The query IS an app's name: that app alone (the most used of several
///   of that name: Golem Files over Nautilus's "Files").
/// - The query ends in a SPACE after a name: the apps whose names go on past
///   it ("google " → Keep, Docs, Calendar…; "youtube " → YouTube Music),
///   without the one of that very name.
fn narrow(query: &str, names: &[&str], used: &[u32]) -> Option<Vec<usize>> {
    let q = query.trim().to_lowercase();
    if q.is_empty() {
        return None;
    }
    if query.ends_with(char::is_whitespace) {
        let past = format!("{q} ");
        let on: Vec<usize> = (0..names.len()).filter(|&i| spoken(names[i]).iter().any(|n| n.starts_with(&past))).collect();
        return (!on.is_empty()).then_some(on);
    }
    let exact = (0..names.len()).filter(|&i| spoken(names[i]).contains(&q));
    exact.max_by_key(|&i| (used.get(i).copied().unwrap_or(0), std::cmp::Reverse(i))).map(|i| vec![i])
}

// ---- the recents --------------------------------------------------------------

/// The value of attribute `name` in a tag's text.
fn attr<'a>(tag: &'a str, name: &str) -> Option<&'a str> {
    let key = format!(" {name}=\"");
    let at = tag.find(&key)? + key.len();
    let len = tag[at..].find('"')?;
    Some(&tag[at..at + len])
}

/// A command's program, as a name: `'/nix/store/…/bin/gimp-3.0 %U'` → `gimp-3.0`.
fn program(exec: &str) -> String {
    let exec = exec.replace("&apos;", "'").replace("&quot;", "\"");
    let first = exec.trim().trim_matches(|c| c == '\'' || c == '"').split_whitespace().next().unwrap_or("");
    first.rsplit('/').next().unwrap_or(first).to_lowercase()
}

/// Whether a record's app (its `name`, its `exec`) is the one `keys` names
/// (lowercased: the app's name, its desktop id, its program).
fn is_app(name: &str, exec: &str, keys: &[String]) -> bool {
    let (name, prog) = (name.to_lowercase(), program(exec));
    keys.iter().any(|k| *k == name || *k == prog || prog.strip_prefix(k.as_str()).is_some_and(|rest| rest.starts_with('-')))
}

/// The files of the system's record (`recently-used.xbel`) that the app
/// `keys` names opened, newest first, each once.
fn xbel_by_app(text: &str, keys: &[String]) -> Vec<PathBuf> {
    let mut found: Vec<(String, PathBuf)> = Vec::new();
    for part in text.split("<bookmark ").skip(1) {
        let Some(tag) = part.split('>').next() else {
            continue;
        };
        let tag = format!(" {tag}");
        let Some(path) = attr(&tag, "href").and_then(|h| h.strip_prefix("file://")) else {
            continue;
        };
        let mut when: Option<String> = None;
        for app in part.split("<bookmark:application ").skip(1) {
            let app = format!(" {}", app.split('>').next().unwrap_or(""));
            if is_app(attr(&app, "name").unwrap_or(""), attr(&app, "exec").unwrap_or(""), keys) {
                when = Some(attr(&app, "modified").unwrap_or("").to_owned());
            }
        }
        let Some(when) = when else {
            continue;
        };
        let when = ["visited", "modified", "added"].iter().filter_map(|k| attr(&tag, k)).chain([when.as_str()]).max().unwrap_or("");
        let path = PathBuf::from(crate::trash::decode_path(&path.replace("&amp;", "&")));
        found.push((when.to_owned(), path));
    }
    found.sort_by(|a, b| b.0.cmp(&a.0));
    let mut seen = std::collections::HashSet::new();
    found.into_iter().map(|(_, p)| p).filter(|p| seen.insert(p.clone())).collect()
}

/// VLC's own list (`vlc-qt-interface.conf`, `[RecentsMRL]` `list=`), newest
/// first: its files and folders.
fn vlc_recents(text: &str) -> Vec<PathBuf> {
    let mut section = false;
    for line in text.lines() {
        let line = line.trim();
        if line.starts_with('[') {
            section = line == "[RecentsMRL]";
        } else if let Some(list) = line.strip_prefix("list=").filter(|_| section) {
            return list
                .split(", ")
                .filter_map(|mrl| mrl.trim().strip_prefix("file://").or_else(|| mrl.trim().strip_prefix("directory://")))
                .map(|p| PathBuf::from(crate::trash::decode_path(p)))
                .collect();
        }
    }
    Vec::new()
}

/// What `app` opened lately that is still there — files and folders,
/// newest first, at most `max`. Its program names it too unless `shared`
/// (other apps start through the same one: a wrapper's records are theirs).
fn recent_for(app: &AppEntry, shared: bool, max: usize) -> Vec<PathBuf> {
    let home = crate::filer::home();
    let prog = program(&app.exec);
    let mut keys = vec![app.name.to_lowercase(), app.id.to_lowercase()];
    if !shared {
        keys.push(prog.clone());
    }
    keys.extend(app.id.rsplit('.').next().map(str::to_lowercase));
    let mut all = Vec::new();
    if prog == "vlc" {
        let conf = home.join(".config/vlc/vlc-qt-interface.conf");
        all.extend(vlc_recents(&std::fs::read_to_string(conf).unwrap_or_default()));
    }
    let xbel = std::env::var_os("XDG_DATA_HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| home.join(".local/share"))
        .join("recently-used.xbel");
    all.extend(xbel_by_app(&std::fs::read_to_string(xbel).unwrap_or_default(), &keys));
    let mut seen = std::collections::HashSet::new();
    all.into_iter().filter(|p| p.exists() && seen.insert(p.clone())).take(max).collect()
}

/// A webapp's site, when Golem knows it (by its slug, else its address).
fn site_of(slug: &str, start: &str) -> Option<&'static web_actions::Site> {
    let host = web_actions::origin(start).map(|(_, h)| h);
    web_actions::site(slug).or_else(|| web_actions::site_of_host(host.as_deref()?))
}

// ---- the actions ----------------------------------------------------------------

/// What an action does.
#[derive(Debug, Clone, PartialEq)]
pub(crate) enum Does {
    /// One of the app's desktop actions or Golem's, or a page of its site
    /// (`web_actions.rs`): its command.
    Run(String),
    /// A search of a site asked by name ("amazon zapatillas"): its command,
    /// no app alone.
    Open(String),
    /// Take the app away.
    Uninstall,
    /// Its windows, every one, in a spread (waveview's `spread_app`).
    ShowAll,
    /// Close every window it has.
    CloseAll,
}

/// One of the lone app's actions.
#[derive(Debug, Clone)]
pub(crate) struct Action {
    /// Its name, as its cell says it.
    pub name: String,
    pub does: Does,
    /// Its symbol (an index of `SYMBOLS`).
    pub symbol: usize,
}

/// The lone app's row: its actions, the cells they stand in, its recents.
#[derive(Debug, Default)]
pub(crate) struct Actions {
    /// The app they are of (its desktop id); `None` while no app is alone.
    pub app: Option<String>,
    /// The actions in the row's order, left to right — the app not among
    /// them: it stands after the first `left`.
    pub list: Vec<Action>,
    pub left: usize,
    /// The entry each action's cell is, this refilter (`list` order).
    pub cells: Vec<usize>,
    /// The recents' entries, in their row's order.
    pub recents: Vec<usize>,
    /// The recents' title ("Recent in VLC"), while there are any.
    pub title: Option<String>,
    /// The grid's columns the row was laid out for.
    pub cols: usize,
    /// Since when Uninstall has been asking to be sure.
    pub armed: Option<Instant>,
    /// Whether its pages open in its window (a webapp, a browser): that
    /// window is brought forward (Seam cannot raise one on Wayland).
    pub web: bool,
    /// The installed webapp a search opens in (its entry).
    pub opener: Option<usize>,
}

/// Where a lone app's recent pages come from and how they open
/// (`web_actions.rs`).
#[derive(Clone)]
enum Source {
    /// A webapp — EVERY one, never the files path (its program is Seam's,
    /// whose records are the files Seam opened): its pages in Seam's
    /// history, opened in its window. Its site when Golem knows it (`SITES`).
    Webapp { site: Option<&'static web_actions::Site>, slug: String, start: String, name: String },
    /// A browser: its own history, opened by it — none of the sites in its
    /// second (Seam's: the installed webapps', whose pages are theirs).
    Browser(web_actions::Browser, Vec<String>),
}

/// How many recent pages are read (a full line is at most this).
const RECENT_MAX: usize = 24;

impl Actions {
    /// The Uninstall action's place in `list`.
    fn uninstall(&self) -> Option<usize> {
        self.list.iter().position(|a| a.does == Does::Uninstall)
    }

    /// How many cells the app's row has (the app and its actions).
    fn row(&self) -> usize {
        self.list.len() + 1
    }

    /// Whether the row is a site's search (no app alone).
    fn searching(&self) -> bool {
        self.app.is_none() && self.list.first().is_some_and(|a| matches!(a.does, Does::Open(_)))
    }
}

/// Which of `right` the room beside the app keeps: the app's own and
/// Golem's actions give way from the end, Uninstall last of all.
fn keep_right(right: &mut Vec<Action>, room: usize) {
    while right.len() > room {
        let at = right.iter().rposition(|a| a.does != Does::Uninstall).unwrap_or(right.len() - 1);
        right.remove(at);
    }
}

impl App {
    /// Whether the dock can uninstall `id`: a package it installed, or an
    /// installed webapp — as `uninstall_app` does them; nothing busy.
    pub(crate) fn can_uninstall(&self, id: &str) -> bool {
        !self.busy_ids.contains(id)
            && ((self.removable_ids.contains(id) && self.managed.attr_for_app(id).is_some())
                || self.installed_webapp_slug(id).is_some())
    }

    /// The action entry `index` is, as its place in the list.
    pub(crate) fn action_of(&self, index: usize) -> Option<usize> {
        self.entries.get(index)?.id.strip_prefix(PREFIX)?.parse().ok()
    }

    /// Whether entry `index` is one of the lone app's cells (an action or a
    /// recent): nothing to drag.
    pub(crate) fn lone_owns(&self, index: usize) -> bool {
        let a = &self.search.actions;
        a.cells.contains(&index) || a.recents.contains(&index)
    }

    /// Where the lone app stands in its row (the cell under the title),
    /// while one is alone.
    pub(crate) fn lone_anchor(&self) -> Option<usize> {
        let a = &self.search.actions;
        match &a.app {
            Some(_) => Some(a.left),
            // (A site's search stands alone in its row, in the middle.)
            None => a.searching().then_some(0),
        }
    }

    /// The room opened under the app's row while it has recents.
    pub(crate) fn lone_room(&self) -> f32 {
        if self.search.actions.title.is_some() {
            RECENT_ROOM * self.icon_scale()
        } else {
            0.0
        }
    }

    /// The Apps grid's columns now.
    fn apps_cols(&self) -> usize {
        let layout = self.layout_at(self.ui.extent_of(crate::state::Target::Open));
        layout.sections[content::SECTION_APPS].cols.max(1)
    }

    /// After a search's Apps are ranked (`refilter`): when they are ONE app,
    /// lay its row out around it — its actions as cells of their own, its
    /// windows' on the left, its recents under it. The same app keeps its
    /// asking; another starts afresh.
    pub(crate) fn add_action_cells(&mut self, apps: &mut Vec<usize>, resting: bool) {
        // The exact name: that app alone; a space after it, the ones past it.
        if !resting && !self.control_panel {
            let names: Vec<&str> = apps.iter().map(|&i| self.entries[i].name.as_str()).collect();
            let used: Vec<u32> = apps.iter().map(|&i| self.usage.count(&self.entries[i].id)).collect();
            if let Some(kept) = narrow(&self.search.query, &names, &used) {
                *apps = kept.into_iter().map(|k| apps[k]).collect();
            }
        }
        // A site's search asked by its name: its cell first, no app alone —
        // unless what is typed begins an app's name ("google maps", "youtube
        // music" are apps to find, not searches).
        if !resting && !self.control_panel {
            if let Some((site, words)) = web_actions::asked(&self.search.query) {
                if !self.begins_a_name(&self.search.query) {
                    self.add_search_cell(apps, site, &words);
                    return;
                }
            }
        }
        let lone = match apps.as_slice() {
            &[only] if !resting && !self.control_panel && !self.search.query.is_empty() => Some(only),
            _ => None,
        };
        let Some((index, entry)) = lone.and_then(|i| Some((i, self.entries.get(i)?.clone()))) else {
            self.search.actions = Actions::default();
            return;
        };
        let cols = self.apps_cols();
        let side = cols.saturating_sub(1) / 2;
        // To the right: its own actions, Golem's, Uninstall.
        let mut right: Vec<Action> = entry
            .path
            .as_deref()
            .map(waverunner_core::index::desktop_actions)
            .unwrap_or_default()
            .into_iter()
            .map(|a| Action { symbol: symbol_for(&entry.id, &a.id, &a.name), name: a.name, does: Does::Run(a.exec) })
            .collect();
        right.extend(known_for(&entry.id).map(|k| Action {
            name: tr(k.name).to_string(),
            does: Does::Run(k.exec.to_owned()),
            symbol: symbol(k.symbol),
        }));
        // A webapp: the pages Golem knows its site has, in its window.
        let source = self.source_of(&entry);
        if let Some(Source::Webapp { site: Some(site), slug, start, .. }) = &source {
            let start = web_actions::origin(start).map_or_else(|| start.clone(), |(o, _)| o);
            right.extend(site.links.iter().map(|l| Action {
                name: tr(l.name).to_string(),
                does: Does::Run(webapps::app_open_exec(slug, &web_actions::link_url(l, &start))),
                symbol: symbol(l.symbol),
            }));
        }
        if self.can_uninstall(&entry.id) {
            right.push(Action {
                name: fill(tr("Uninstall {app}"), &[("app", &entry.name)]),
                does: Does::Uninstall,
                symbol: symbol("trash"),
            });
        }
        keep_right(&mut right, side);
        // To the left, while it has windows: Show all beside it, Close all past it.
        let mut left: Vec<Action> = Vec::new();
        if self.running.get(&index).is_some_and(|w| !w.is_empty()) {
            left.push(Action { name: tr("Show all windows").to_string(), does: Does::ShowAll, symbol: symbol("spread") });
            left.push(Action { name: tr("Close all windows").to_string(), does: Does::CloseAll, symbol: symbol("close") });
        }
        left.truncate(side);
        left.reverse();
        let same = self.search.actions.app.as_deref() == Some(entry.id.as_str());
        let armed = self.search.actions.armed.filter(|_| same);
        let n_left = left.len();
        left.extend(right);
        self.search.actions = Actions {
            app: Some(entry.id.clone()),
            list: left,
            left: n_left,
            cols,
            armed,
            web: source.is_some(),
            ..Default::default()
        };
        let (placeholder, layer) = (
            self.placeholders.get(index).copied().unwrap_or(false),
            self.icon_layers.get(index).copied().unwrap_or(index as u32),
        );
        for n in 0..self.search.actions.list.len() {
            let name = self.action_label(n);
            let exec = match &self.search.actions.list[n].does {
                Does::Run(exec) => exec.clone(),
                _ => String::new(),
            };
            let cell = AppEntry {
                id: format!("{PREFIX}{n}"),
                name,
                description: None,
                exec,
                icon: entry.icon.clone(),
                startup_wm_class: None,
                needs_terminal: entry.needs_terminal,
                path: None,
            };
            let at = self.push_transient(cell, apps::EntryKind::App, placeholder, layer);
            self.search.actions.cells.push(at);
        }
        let cells = &self.search.actions.cells;
        *apps = cells[..n_left].iter().copied().chain([index]).chain(cells[n_left..].iter().copied()).collect();
        // Under it: what it opened lately, opened again in it by a click —
        // a webapp's or a browser's pages, else files.
        match source {
            Some(source) => {
                for page in self.web_recents(&entry.id, &source).into_iter().take(cols) {
                    let exec = match &source {
                        Source::Webapp { slug, .. } => webapps::app_open_exec(slug, &page.url),
                        Source::Browser(..) => format!("{} {}", entry.exec, launch::shell_quote(&page.url)),
                    };
                    let at = self.push_web_recent(page, exec, (placeholder, layer));
                    self.search.actions.recents.push(at);
                    apps.push(at);
                }
            }
            None => {
                let prog = program(&entry.exec);
                let base = &self.entries[..self.base_len.min(self.entries.len())];
                let shared = base.iter().filter(|e| program(&e.exec) == prog).count() > 1;
                for path in recent_for(&entry, shared, cols) {
                    let name = path.file_name().map_or_else(|| path.to_string_lossy().into_owned(), |n| n.to_string_lossy().into_owned());
                    let exec = format!("{} {}", entry.exec, launch::shell_quote(&path.to_string_lossy()));
                    let at = self.push_transient_file(&path.to_string_lossy(), &name, exec, path.is_dir());
                    self.search.actions.recents.push(at);
                    apps.push(at);
                }
            }
        }
        if !self.search.actions.recents.is_empty() {
            self.search.actions.title = Some(fill(tr("Recent in {app}"), &[("app", &entry.name)]));
        }
    }

    /// Whether `query` begins the name of an app (installed, or one of the
    /// catalog's webapps).
    fn begins_a_name(&self, query: &str) -> bool {
        let q = query.trim().to_lowercase();
        let base = &self.entries[..self.base_len.min(self.entries.len())];
        base.iter().zip(&self.kinds).any(|(e, k)| *k == apps::EntryKind::App && e.name.to_lowercase().starts_with(&q))
    }

    /// The search of `site` for `words` as the first of Apps, selected: its
    /// webapp's icon (installed or in the catalog, else Seam's) with the
    /// search's symbol; Enter opens it in the webapp when it is installed,
    /// else in Seam.
    fn add_search_cell(&mut self, apps: &mut Vec<usize>, site: &'static web_actions::Site, words: &str) {
        self.search.actions = Actions::default();
        let id = webapps::id_for_slug(site.slug);
        let base = &self.entries[..self.base_len.min(self.entries.len())];
        let webapp = base.iter().position(|e| e.id == id);
        let opener = webapp.filter(|&i| !self.is_catalog_webapp(i));
        let start = opener.and_then(|i| webapps::exec_app_url(&base[i].exec));
        let Some(url) = web_actions::search_url_from(site, words, start) else {
            return;
        };
        let exec = match opener {
            Some(_) => webapps::app_open_exec(site.slug, &url),
            None => format!("seam {}", launch::shell_quote(&url)),
        };
        let looks = webapp.or_else(|| base.iter().position(|e| program(&e.exec) == "seam"));
        let (placeholder, layer) = looks.map_or((true, 0), |i| {
            (self.placeholders.get(i).copied().unwrap_or(true), self.icon_layers.get(i).copied().unwrap_or(0))
        });
        // (On two lines: the site, then the words — whole, as the box has room.)
        let name = format!("{}\n{}", fill(tr("Search {site}"), &[("site", &site.name)]), fill(tr("“{words}”"), &[("words", &words)]));
        info!("search cell: {name} → {url}");
        self.search.actions = Actions {
            list: vec![Action { name, does: Does::Open(exec.clone()), symbol: symbol("search") }],
            opener,
            web: true,
            cols: self.apps_cols(),
            ..Default::default()
        };
        let cell = AppEntry {
            id: format!("{PREFIX}0"),
            name: self.action_label(0),
            description: None,
            exec,
            icon: looks.and_then(|i| self.entries[i].icon.clone()),
            startup_wm_class: None,
            needs_terminal: false,
            path: None,
        };
        let at = self.push_transient(cell, apps::EntryKind::App, placeholder, layer);
        self.search.actions.cells.push(at);
        apps.insert(0, at);
    }

    /// Where an app's recent pages come from: a webapp (every one), or a
    /// browser's history — Seam's without the installed webapps' sites.
    fn source_of(&self, app: &AppEntry) -> Option<Source> {
        if let Some(slug) = webapps::exec_app_slug(&app.exec) {
            let start = webapps::exec_app_url(&app.exec).unwrap_or_default().to_owned();
            let site = site_of(slug, &start);
            return Some(Source::Webapp { site, slug: slug.to_owned(), start, name: app.name.clone() });
        }
        let browser = web_actions::browser_for(&program(&app.exec))?;
        let skip = if browser == web_actions::seam() {
            let base = &self.entries[..self.base_len.min(self.entries.len())];
            (0..base.len())
                .filter(|&i| !self.is_catalog_webapp(i))
                .filter_map(|i| {
                    let exec = &base[i].exec;
                    let (slug, start) = (webapps::exec_app_slug(exec)?, webapps::exec_app_url(exec)?);
                    Some(web_actions::webapp_hosts(site_of(slug, start), start))
                })
                .flatten()
                .collect()
        } else {
            Vec::new()
        };
        Some(Source::Browser(browser, skip))
    }

    /// The recent pages of app `id` from `source`: those read lately, and a
    /// reading asked for off the loop when they are not fresh (the row is
    /// laid out again when it brings something new while the app is still
    /// alone).
    fn web_recents(&mut self, id: &str, source: &Source) -> Vec<web_actions::Recent> {
        let got = self.search.web.got.get(id).cloned();
        let fresh = got.as_ref().is_some_and(|(at, _)| at.elapsed() < web_actions::FRESH);
        if !fresh && self.search.web.asking.insert(id.to_owned()) {
            let (key, source) = (id.to_owned(), source.clone());
            self.desktop_off_loop(
                move || match &source {
                    Source::Webapp { site, start, name, .. } => web_actions::webapp_recents(*site, start, name, RECENT_MAX),
                    Source::Browser(browser, skip) => web_actions::browser_recents(browser, skip, RECENT_MAX),
                },
                move |app, pages| {
                    app.search.web.asking.remove(&key);
                    info!("web recents of {key}: {}", pages.len());
                    let new = app.search.web.got.get(&key).is_none_or(|(_, old)| *old != pages);
                    app.search.web.got.insert(key.clone(), (Instant::now(), pages));
                    if new && app.search.actions.app.as_deref() == Some(key.as_str()) {
                        // The app's row is the same: the keys stay where they were on it.
                        let (sel, row) = (app.search.selected, app.search.actions.row());
                        app.refilter();
                        if let Some(sel) = sel.filter(|&s| s < row && app.search.actions.app.as_deref() == Some(key.as_str())) {
                            app.search.selected = Some(sel);
                        }
                        app.schedule_frame();
                    }
                },
            );
        }
        got.map(|(_, pages)| pages).unwrap_or_default()
    }

    /// A recent page's cell: its picture once the thumbnailer has it (the
    /// app's own icon meanwhile, and without one), its title as its name.
    fn push_web_recent(&mut self, page: web_actions::Recent, exec: String, (placeholder, layer): (bool, u32)) -> usize {
        let (mut placeholder, mut layer) = (placeholder, layer);
        let id = match &page.picture {
            Some(picture) => {
                let path = picture.to_string_lossy().into_owned();
                if let Some(&(slot, _)) = self.thumb_map.get(&path) {
                    layer = self.thumb_layer_base() + slot as u32;
                    placeholder = false;
                } else if self.thumb_pending.insert(path.clone()) {
                    self.thumbs.request(&path);
                }
                path
            }
            None => format!("web:{}", page.url),
        };
        let cell = AppEntry {
            id,
            name: page.title,
            description: None,
            exec,
            icon: None,
            startup_wm_class: None,
            needs_terminal: false,
            path: None,
        };
        self.push_transient(cell, apps::EntryKind::File, placeholder, layer)
    }

    /// Whether entry `index` is one of the lone app's recents.
    pub(crate) fn lone_recent(&self, index: usize) -> bool {
        self.search.actions.recents.contains(&index)
    }

    /// Open recent `index` in the lone app: a page in a webapp's or a
    /// browser's window, which comes forward; a file as it always opened.
    pub(crate) fn run_recent(&mut self, index: usize) {
        let Some(exec) = self.entries.get(index).map(|e| e.exec.clone()) else {
            return;
        };
        info!("opening a recent of {:?}", self.search.actions.app);
        if let Err(e) = launch::launch(&exec, false, &self.config.launch.terminal) {
            error!("recent failed to open: {e:#}");
        }
        let app = self.search.actions.app.clone().and_then(|id| self.entries.iter().position(|e| e.id == id));
        match app {
            Some(app) if self.search.actions.web => self.web_opened(app),
            _ => self.launched(index),
        }
    }

    /// After a page was asked of app `index`'s window: that window comes
    /// forward (Seam loads it there but cannot raise it on Wayland), else
    /// the app starts as any other.
    fn web_opened(&mut self, index: usize) {
        let windows = self.running.get(&index).cloned().unwrap_or_default();
        let shown = windows.iter().find(|a| !self.parked_windows.contains(*a)).cloned();
        let parked = windows.first().filter(|a| self.parked_windows.contains(*a)).cloned();
        if shown.is_none() && parked.is_none() {
            self.launched(index);
            return;
        }
        if let Some(id) = self.entries.get(index).map(|e| e.id.clone()) {
            self.usage.increment(&id);
        }
        if self.interactive {
            self.begin_keyboard_handback(KbSurface::Launcher, shown.clone());
            surface::set_interactive(&self.layer, false);
            self.interactive = false;
        } else if let Some(addr) = &shown {
            hypr::focus_window(addr);
        }
        if let (None, Some(addr)) = (&shown, &parked) {
            self.restore_minimized(addr);
        }
        self.dismiss();
    }

    /// The grid's cells whose names do not fit on one line, by entry: each
    /// measured once a room (`name_fit`), then two lines between words and
    /// one with "…" (`content::NameFit`). Files with a path keep their own
    /// cut (`content::tile_label`); the actions have theirs already.
    pub(crate) fn fitted_names(&mut self, layout: &content::Layout) -> HashMap<usize, content::NameFit> {
        let mut out = HashMap::new();
        for s in 0..content::N_SECTIONS {
            let sec = &layout.sections[s];
            let room = sec.viewport.w / sec.cols.max(1) as f32 - 12.0;
            if room <= 0.0 {
                continue;
            }
            let key = room.round() as u32;
            for &e in &self.search.visible[s] {
                let Some(entry) = self.entries.get(e) else {
                    continue;
                };
                let file = self.kinds.get(e) == Some(&apps::EntryKind::File) && entry.description.is_some();
                if file || entry.name.contains('\n') || entry.name.is_empty() {
                    continue;
                }
                if let Some(known) = self.name_fit.get(&key).and_then(|m| m.get(entry.name.as_str())) {
                    if let Some(fit) = known {
                        out.insert(e, fit.clone());
                    }
                    continue;
                }
                let name = entry.name.clone();
                let mut measure = |t: &str| match self.renderer.as_mut() {
                    Some(r) => r.measure_text(t, content::LABEL_FONT_PX, None),
                    None => crate::options::est_text_w(t, content::LABEL_FONT_PX),
                };
                let fit = (measure(&name) > room - crate::desktop::FIT_SLACK).then(|| content::NameFit {
                    one: crate::desktop::fit_label(&name, room, &mut measure),
                    two: by_words(&name, room, &mut measure),
                });
                if let Some(fit) = &fit {
                    out.insert(e, fit.clone());
                }
                self.name_fit.entry(key).or_default().insert(name, fit);
            }
        }
        out
    }

    /// The lone app's display slots (`refilter`, after the search's dense
    /// ones): its row from the first slot, its recents from the second row.
    pub(crate) fn lone_slots(&mut self) {
        let a = &self.search.actions;
        if a.searching() {
            // The search alone in the first row; the apps found from the next.
            let n = self.search.visible[content::SECTION_APPS].len();
            let cols = a.cols.max(1);
            self.apps_slots = (0..n).map(|i| if i == 0 { 0 } else { cols + i - 1 }).collect();
            self.apps_span = self.apps_slots.last().map_or(0, |s| s + 1);
            return;
        }
        if a.app.is_none() {
            return;
        }
        let row = a.row();
        let cols = a.cols.max(row);
        let mut slots: Vec<usize> = (0..row).collect();
        slots.extend((0..a.recents.len()).map(|j| cols + j));
        self.apps_span = slots.last().map_or(0, |s| s + 1);
        self.apps_slots = slots;
    }

    /// What action `n`'s cell says: its name — Uninstall asking, "Again to
    /// uninstall" — on up to two lines (a `\n` between them), broken where
    /// the grid's label has room.
    fn action_label(&mut self, n: usize) -> String {
        let a = &self.search.actions;
        let Some(action) = a.list.get(n) else {
            return String::new();
        };
        let text = if action.does == Does::Uninstall && a.armed.is_some() {
            tr("Again to uninstall").to_string()
        } else {
            action.name.clone()
        };
        let layout = self.layout_at(self.ui.extent_of(crate::state::Target::Open));
        let sec = &layout.sections[content::SECTION_APPS];
        let spread = if a.searching() { WIDE_LABEL } else { 1.0 };
        let room = sec.viewport.w / sec.cols.max(1) as f32 * spread - 12.0;
        let mut measure = |s: &str| match self.renderer.as_mut() {
            Some(r) => r.measure_text(s, content::LABEL_FONT_PX, None),
            None => crate::options::est_text_w(s, content::LABEL_FONT_PX),
        };
        // A name of two lines already (a search's) keeps them, each fitted.
        let (first, second) = match text.split_once('\n') {
            Some((a, b)) => (crate::desktop::fit_label(a, room, &mut measure), crate::desktop::fit_label(b, room, &mut measure)),
            None => by_words(&text, room, &mut measure),
        };
        if second.is_empty() {
            first
        } else {
            format!("{first}\n{second}")
        }
    }

    /// Write the actions' names into their cells again (the asking came or
    /// went).
    fn relabel_actions(&mut self) {
        for n in 0..self.search.actions.cells.len() {
            let name = self.action_label(n);
            if let Some(entry) = self.search.actions.cells.get(n).and_then(|&i| self.entries.get_mut(i)) {
                entry.name = name;
            }
        }
        self.schedule_frame();
    }

    /// What the frame draws of them: each action cell's entry with its
    /// symbol's texture layer, the cell asking to be sure, and the one whose
    /// name may spread (a search's).
    pub(crate) fn action_draw(&self) -> (Vec<(usize, u32)>, Option<usize>, Option<usize>) {
        let a = &self.search.actions;
        let Some(base) = self.act_symbol_layer else {
            return (Vec::new(), None, None);
        };
        let cells = a.cells.iter().zip(&a.list).map(|(&e, act)| (e, base + act.symbol as u32)).collect();
        let asking = a.uninstall().filter(|_| a.armed.is_some()).and_then(|u| a.cells.get(u).copied());
        let wide = a.cells.first().copied().filter(|_| a.searching());
        (cells, asking, wide)
    }

    /// The dock box's keys, first, while an app is alone: Escape puts an
    /// asking Uninstall back (the next Escape is the box's); ← → walk ITS
    /// row (the app's, or the recents'), never off its ends; ↓ from the
    /// app's row goes to the recent under it, ↑ from the recents back to
    /// the app, ↓ from the recents on to what follows. Returns whether the
    /// key was theirs.
    pub(crate) fn app_actions_key(&mut self, keysym: Keysym) -> bool {
        let a = &self.search.actions;
        if a.app.is_none() {
            return false;
        }
        if keysym == Keysym::Escape {
            if a.armed.is_some() {
                self.actions_disarm();
                return true;
            }
            return false;
        }
        let (row, recents) = (a.row(), a.recents.len());
        let Some(cur) = self.search.selected.filter(|&c| c < row + recents) else {
            return false;
        };
        let in_row = cur < row;
        let to = match keysym {
            Keysym::Left if in_row => Some(cur.saturating_sub(1)),
            Keysym::Left => Some(cur.saturating_sub(1).max(row)),
            Keysym::Right if in_row => Some((cur + 1).min(row - 1)),
            Keysym::Right => Some((cur + 1).min(row + recents - 1)),
            Keysym::Down if in_row && recents > 0 => Some(row + self.recent_under(cur)),
            Keysym::Down if !in_row && self.flat_len() > row + recents => Some(row + recents),
            Keysym::Down if !in_row => Some(cur),
            Keysym::Up if in_row => Some(cur),
            Keysym::Up => Some(a.left),
            _ => None,
        };
        let Some(to) = to else {
            return false;
        };
        self.search.selected = Some(to);
        self.actions_settle();
        self.schedule_frame();
        true
    }

    /// The recent nearest under cell `cur` of the app's row (both rows are
    /// centred on their own).
    fn recent_under(&self, cur: usize) -> usize {
        let a = &self.search.actions;
        let layout = self.current_layout();
        let sec = &layout.sections[content::SECTION_APPS];
        let cw = sec.viewport.w / sec.cols.max(1) as f32;
        let x = sec.row_shift(0) + cur as f32 * cw;
        let below = sec.cols.max(a.row());
        let x0 = sec.row_shift(below);
        let j = ((x - x0) / cw).round().max(0.0) as usize;
        j.min(a.recents.len().saturating_sub(1))
    }

    /// Put Uninstall back once neither the keys nor the pointer are on its
    /// cell (every frame of the box).
    pub(crate) fn actions_settle(&mut self) {
        let a = &self.search.actions;
        let Some(u) = a.uninstall().filter(|_| a.armed.is_some()) else {
            return;
        };
        let Some(&entry) = a.cells.get(u) else {
            return;
        };
        let at = self.search.visible[content::SECTION_APPS].iter().position(|&e| e == entry);
        let keys_on = at.is_some() && self.search.selected == at;
        let pointer_on = at.is_some_and(|i| self.hover == Some(content::Hit::GridCell(content::SECTION_APPS, i)));
        if !keys_on && !pointer_on {
            self.actions_disarm();
        }
    }

    /// Uninstall stops asking.
    fn actions_disarm(&mut self) {
        if self.search.actions.armed.take().is_some() {
            self.relabel_actions();
        }
    }

    /// The box goes, the keyboard back to where it was (Show all and Close
    /// all leave no app starting to give it to).
    fn lone_leave(&mut self) {
        if self.interactive {
            self.begin_keyboard_handback(KbSurface::Launcher, None);
            surface::set_interactive(&self.layer, false);
            self.interactive = false;
        }
        self.dismiss();
    }

    /// Run action `n`: a command starts and the box goes; Show all spreads
    /// the app's windows, Close all closes them; Uninstall asks, and
    /// uninstalls when asked again.
    pub(crate) fn run_app_action(&mut self, n: usize) {
        let Some(action) = self.search.actions.list.get(n).cloned() else {
            return;
        };
        if let Does::Open(exec) = &action.does {
            info!("searching: {}", action.name);
            if let Err(e) = launch::launch(exec, false, &self.config.launch.terminal) {
                error!("search failed to open: {e:#}");
            }
            match self.search.actions.opener {
                Some(webapp) => self.web_opened(webapp),
                None => self.lone_leave(),
            }
            return;
        }
        let Some(app) = self.search.actions.app.clone() else {
            return;
        };
        let Some(index) = self.entries.iter().position(|e| e.id == app) else {
            return;
        };
        let needs_terminal = self.entries[index].needs_terminal;
        let windows = self.running.get(&index).cloned().unwrap_or_default();
        match action.does {
            Does::Run(exec) => {
                info!("app action {:?} of {app}", action.name);
                let exec = self.gpu_exec(&app, exec);
                if let Err(e) = launch::launch(&exec, needs_terminal, &self.config.launch.terminal) {
                    error!("app action failed for {app}: {e:#}");
                }
                if self.search.actions.web {
                    self.web_opened(index);
                } else {
                    self.launched(index);
                }
            }
            Does::ShowAll => {
                let class = hypr::running_windows().into_iter().find(|w| windows.contains(&w.address)).map(|w| w.class);
                self.lone_leave();
                match class {
                    Some(class) => {
                        info!("showing every window of {app} ({class})");
                        let class = class.replace('\\', "\\\\").replace('"', "\\\"");
                        hypr::eval(&format!("hl.plugin.waveview.spread_app(\"{class}\")"));
                    }
                    None => warn!("no window of {app} to show"),
                }
            }
            // (Taken before: it needs no app.)
            Does::Open(_) => {}
            Does::CloseAll => {
                info!("closing every window of {app} ({})", windows.len());
                for addr in &windows {
                    hypr::close_window(addr);
                }
                self.lone_leave();
            }
            Does::Uninstall if self.search.actions.armed.is_none() => {
                self.search.actions.armed = Some(Instant::now());
                self.relabel_actions();
                let timer = Timer::from_duration(ARM_FOR);
                if let Err(e) = self.loop_handle.insert_source(timer, |_, _, app: &mut App| {
                    if app.search.actions.armed.is_some_and(|t| t.elapsed() >= ARM_FOR) {
                        app.actions_disarm();
                    }
                    TimeoutAction::Drop
                }) {
                    warn!("failed to arm the uninstall's wait: {e}");
                }
            }
            Does::Uninstall => {
                self.search.actions.armed = None;
                if self.uninstall_app(&app) {
                    info!("uninstalling {app} from its search");
                }
                self.refilter();
                self.schedule_frame();
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The paths as text.
    fn named(paths: &[PathBuf]) -> Vec<String> {
        paths.iter().map(|p| p.to_string_lossy().into_owned()).collect()
    }

    #[test]
    fn each_action_wears_its_apps_own_symbol_or_one_by_what_it_says() {
        let of = |app: &str, id: &str, name: &str| SYMBOLS[symbol_for(app, id, name)].0;
        assert_eq!(of("google-chrome", "new-private-window", "New Incognito Window"), "incognito");
        assert_eq!(of("google-chrome", "new-window", "New Window"), "window");
        assert_eq!(of("seam", "new-private-window", "New Private Window"), "mask");
        assert_eq!(of("seam", "new-window", "Nueva ventana"), "seam-window");
        assert_eq!(of("code", "new-empty-window", "New Empty Window"), "window");
        assert_eq!(of("thunar", "open-home", "Home"), "home");
        assert_eq!(of("thunar", "open-computer", "Computer"), "computer");
        assert_eq!(of("thunar", "open-trash", "Trash"), "trash");
        assert_eq!(of("Alacritty", "New", "New Terminal"), "terminal");
        assert_eq!(of("org.inkscape.Inkscape", "new-drawing", "New Drawing"), "open");
    }

    #[test]
    fn golem_knows_vlc_mpv_gimp_and_obs() {
        let names = |id: &str| known_for(id).map(|k| k.name).collect::<Vec<_>>();
        assert_eq!(names("vlc"), ["Open disc", "Play copied link", "Play Music"]);
        assert_eq!(names("mpv"), ["Play copied link", "Play Music"]);
        assert_eq!(names("org.gimp.GIMP"), ["Open copied image"]);
        assert_eq!(names("com.obsproject.Studio"), ["Start recording", "Start virtual camera"]);
        assert!(names("google-chrome").is_empty());
        for k in KNOWN {
            assert!(SYMBOLS.iter().any(|(n, _)| *n == k.symbol), "{} has no symbol {}", k.name, k.symbol);
        }
    }

    #[test]
    fn the_apps_own_actions_give_way_before_uninstall() {
        let act = |does: Does| Action { name: String::new(), does, symbol: 0 };
        let mut right = vec![act(Does::Run("a".into())), act(Does::Run("b".into())), act(Does::Uninstall)];
        keep_right(&mut right, 2);
        assert_eq!(right.iter().map(|a| a.does.clone()).collect::<Vec<_>>(), [Does::Run("a".into()), Does::Uninstall]);
        keep_right(&mut right, 1);
        assert_eq!(right[0].does, Does::Uninstall);
        keep_right(&mut right, 0);
        assert!(right.is_empty());
    }

    #[test]
    fn the_exact_name_stands_alone_and_a_space_shows_the_ones_past_it() {
        let names = ["YouTube", "YouTube Music", "Google", "Google Keep", "Google Docs", "Files", "Golem Files", "Foot"];
        let used = [9, 6, 3, 1, 2, 1, 39, 1];
        let keep = |q: &str| narrow(q, &names, &used).map(|v| v.into_iter().map(|i| names[i]).collect::<Vec<_>>());
        assert_eq!(keep("youtub"), None);
        assert_eq!(keep("youtube"), Some(vec!["YouTube"]));
        assert_eq!(keep("YouTube "), Some(vec!["YouTube Music"]));
        assert_eq!(keep("google"), Some(vec!["Google"]));
        assert_eq!(keep("google "), Some(vec!["Google Keep", "Google Docs"]));
        assert_eq!(keep("files"), Some(vec!["Golem Files"]));
        assert_eq!(keep("foot "), None);
        assert_eq!(keep("googl"), None);
        assert_eq!(keep(""), None);
    }

    #[test]
    fn a_name_breaks_between_words() {
        // Ten pixels a letter, spaces too.
        let mut w = |s: &str| s.chars().count() as f32 * 10.0;
        let room = 140.0 + crate::desktop::FIT_SLACK;
        assert_eq!(by_words("New Window", room, &mut w), ("New Window".into(), String::new()));
        assert_eq!(by_words("New Incognito Window", room, &mut w), ("New Incognito".into(), "Window".into()));
        assert_eq!(by_words("Uninstall Google Chrome", room, &mut w), ("Uninstall".into(), "Google Chrome".into()));
        // One word too long: broken with a hyphen.
        let (first, second) = by_words("Subscriptionsxxxxx", room, &mut w);
        assert!(first.ends_with('-') && w(&first) <= room - crate::desktop::FIT_SLACK, "{first}");
        assert_eq!(format!("{}{second}", first.trim_end_matches('-')), "Subscriptionsxxxxx");
    }

    #[test]
    fn the_record_gives_each_apps_own_files_newest_first() {
        let xbel = r#"<?xml version="1.0"?>
<xbel version="1.0" xmlns:bookmark="http://www.freedesktop.org/standards/desktop-bookmarks">
  <bookmark href="file:///home/max/a%20b.png" added="2026-10-01T10:00:00Z" modified="2026-10-01T10:00:00Z" visited="2026-10-01T10:00:00Z">
    <info><metadata owner="http://freedesktop.org"><bookmark:applications>
      <bookmark:application name="GNU Image Manipulation Program" exec="&apos;gimp-3.0 %u&apos;" modified="2026-10-01T10:00:00Z" count="1"/>
    </bookmark:applications></metadata></info>
  </bookmark>
  <bookmark href="file:///home/max/c.png" added="2026-10-03T10:00:00Z" modified="2026-10-03T10:00:00Z" visited="2026-10-03T10:00:00Z">
    <info><metadata owner="http://freedesktop.org"><bookmark:applications>
      <bookmark:application name="Golem Files" exec="&apos;waverunner-ctl files %u&apos;" modified="2026-10-03T10:00:00Z" count="1"/>
      <bookmark:application name="gimp" exec="&apos;/nix/store/x/bin/gimp %u&apos;" modified="2026-10-04T10:00:00Z" count="1"/>
    </bookmark:applications></metadata></info>
  </bookmark>
  <bookmark href="file:///home/max/notes.txt" added="2026-10-05T10:00:00Z" modified="2026-10-05T10:00:00Z" visited="2026-10-05T10:00:00Z">
    <info><metadata owner="http://freedesktop.org"><bookmark:applications>
      <bookmark:application name="Text Editor" exec="&apos;gnome-text-editor %U&apos;" modified="2026-10-05T10:00:00Z" count="1"/>
    </bookmark:applications></metadata></info>
  </bookmark>
</xbel>"#;
        let keys = vec!["gimp".to_owned(), "org.gimp.gimp".to_owned()];
        assert_eq!(named(&xbel_by_app(xbel, &keys)), ["/home/max/c.png", "/home/max/a b.png"]);
    }

    #[test]
    fn vlcs_own_list_is_read() {
        let conf = "[General]\nfoo=1\n\n[RecentsMRL]\nlist=file:///home/max/Music/a.mp4, file:///home/max/2026-07-21%2014-40-10.avi, directory:///home/max\ntimes=0, 0, 0\n";
        assert_eq!(named(&vlc_recents(conf)), ["/home/max/Music/a.mp4", "/home/max/2026-07-21 14-40-10.avi", "/home/max"]);
        assert!(vlc_recents("[General]\nlist=file:///x\n").is_empty());
    }

    #[test]
    fn every_symbol_is_drawn() {
        for (name, s) in SYMBOLS {
            let svg = s.svg().unwrap_or_else(|| panic!("{name} has no symbol"));
            assert!(apps::svg_figure(&svg).is_some(), "{name} does not draw");
        }
    }
}
