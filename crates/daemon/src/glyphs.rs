//! THE SHELL'S LINE SYMBOLS: the Files mockup's own (16 units, strokes 1.35
//! wide, round ends), written once as SVG files that the icon loader reads
//! by their path (`key`). Golem Files' top line and sidebar wear them, and
//! so does every right-click menu (`desktop_menu.rs`): the same symbol for
//! the same act wherever it is offered.

use std::path::PathBuf;

/// Every symbol, by name: what is drawn inside the 16×16 box.
pub(crate) const GLYPHS: &[(&str, &str)] = &[
    // The top line and the sidebar.
    (
        "clock",
        r#"<circle cx="8" cy="8" r="5.6"/><path d="M8 4.8V8l2.2 1.5"/>"#,
    ),
    (
        "pin",
        r#"<path d="M6 2.6h4M6.7 2.6l-.4 4L4.2 8.8h7.6L9.7 6.6l-.4-4M8 8.8v4.6"/>"#,
    ),
    (
        "files",
        r#"<path d="M2 4.4c0-.6.4-1 1-1h3.2l1.4 1.5H13c.6 0 1 .4 1 1v6.4c0 .6-.4 1-1 1H3c-.6 0-1-.4-1-1z"/>"#,
    ),
    (
        "media",
        r#"<rect x="2" y="3.6" width="9.6" height="8.8" rx="1.4"/><path d="M13.8 5.6v6.6c0 .9-.6 1.6-1.5 1.6H5"/><path d="m5.8 6.3 2.8 1.7-2.8 1.7z"/>"#,
    ),
    (
        "sort",
        r#"<path d="M2.8 4.2h10.4M4.8 8h6.4M6.8 11.8h2.4"/>"#,
    ),
    (
        "info",
        r#"<circle cx="8" cy="8" r="5.6"/><path d="M8 7.2v3.8M8 5.1v.1"/>"#,
    ),
    (
        "grid",
        r#"<rect x="2.4" y="2.4" width="4.6" height="4.6" rx="1"/><rect x="9" y="2.4" width="4.6" height="4.6" rx="1"/><rect x="2.4" y="9" width="4.6" height="4.6" rx="1"/><rect x="9" y="9" width="4.6" height="4.6" rx="1"/>"#,
    ),
    (
        "list",
        r#"<path d="M5.6 4h8M5.6 8h8M5.6 12h8"/><circle cx="2.8" cy="4" r=".6"/><circle cx="2.8" cy="8" r=".6"/><circle cx="2.8" cy="12" r=".6"/>"#,
    ),
    (
        "search",
        r#"<circle cx="7" cy="7" r="4.4"/><path d="m10.4 10.4 3.4 3.4"/>"#,
    ),
    (
        "pen",
        r#"<path d="M5 11.2l.5-2.1 4-4a1.05 1.05 0 0 1 1.5 0l.1.1a1.05 1.05 0 0 1 0 1.5l-4 4z"/><path d="M8.7 6l1.5 1.5"/>"#,
    ),
    (
        "x",
        r#"<path stroke-width="1.8" d="m3 3 10 10M13 3 3 13"/>"#,
    ),
    ("plus", r#"<path d="M8 3v10M3 8h10"/>"#),
    ("chev", r#"<path d="m5.5 3 5 5-5 5"/>"#),
    ("left", r#"<path d="M10 3 5 8l5 5"/>"#),
    (
        "folder",
        r#"<path d="M2 4.4c0-.6.4-1 1-1h3.2l1.4 1.5H13c.6 0 1 .4 1 1v6.4c0 .6-.4 1-1 1H3c-.6 0-1-.4-1-1z"/>"#,
    ),
    (
        "undo",
        r#"<path d="M5.6 3.8 3 6.4 5.6 9"/><path d="M3.3 6.4h6.2a3.2 3.2 0 0 1 0 6.4H7.4"/>"#,
    ),
    ("tick", r#"<path d="m3.6 8.4 2.8 2.8 6-6.4"/>"#),
    // What is done to things.
    (
        "open",
        r#"<path d="M9.4 2.6h4v4M13.2 2.8 7.4 8.6M11.8 9.6v3.2c0 .6-.4 1-1 1H3.2c-.6 0-1-.4-1-1V5.2c0-.6.4-1 1-1h3.2"/>"#,
    ),
    (
        "openwith",
        r#"<rect x="2.4" y="2.4" width="4.6" height="4.6" rx="1.2"/><rect x="9" y="2.4" width="4.6" height="4.6" rx="1.2"/><rect x="2.4" y="9" width="4.6" height="4.6" rx="1.2"/><path d="M11.3 9v4.6M9 11.3h4.6"/>"#,
    ),
    (
        "app",
        r#"<rect x="2.4" y="2.4" width="11.2" height="11.2" rx="3"/>"#,
    ),
    (
        "eye",
        r#"<path d="M1.8 8S4 3.8 8 3.8 14.2 8 14.2 8 12 12.2 8 12.2 1.8 8 1.8 8z"/><circle cx="8" cy="8" r="1.9"/>"#,
    ),
    (
        "eyeoff",
        r#"<path d="M1.8 8S4 3.8 8 3.8 14.2 8 14.2 8 12 12.2 8 12.2 1.8 8 1.8 8z"/><path d="M2.8 2.8l10.4 10.4"/>"#,
    ),
    (
        "tab",
        r#"<rect x="2.2" y="3.2" width="11.6" height="9.6" rx="1.4"/><path d="M2.2 6h5.4V3.2M10.4 8.2v3M8.9 9.7h3"/>"#,
    ),
    (
        "term",
        r#"<rect x="1.8" y="2.8" width="12.4" height="10.4" rx="1.6"/><path d="m4.4 6.2 2 1.8-2 1.8M7.8 10.2h3.4"/>"#,
    ),
    (
        "copy",
        r#"<rect x="5.2" y="5.2" width="8.4" height="8.4" rx="1.4"/><path d="M10.8 5.2V3.8c0-.8-.6-1.4-1.4-1.4H3.8c-.8 0-1.4.6-1.4 1.4v5.6c0 .8.6 1.4 1.4 1.4h1.4"/>"#,
    ),
    (
        "move",
        r#"<path d="M2.4 8h8.4M7.6 4.8 10.8 8l-3.2 3.2"/><path d="M13.6 3.2v9.6"/>"#,
    ),
    (
        "paste",
        r#"<rect x="3" y="3.2" width="10" height="10.8" rx="1.4"/><path d="M6 3.2V2.2h4v1"/><path d="M5.6 7.4h4.8M5.6 10.2h3.2"/>"#,
    ),
    (
        "dup",
        r#"<rect x="5.2" y="5.2" width="8.4" height="8.4" rx="1.4"/><path d="M10.8 5.2V3.8c0-.8-.6-1.4-1.4-1.4H3.8c-.8 0-1.4.6-1.4 1.4v5.6c0 .8.6 1.4 1.4 1.4h1.4M9.4 7.6v3.6M7.6 9.4h3.6"/>"#,
    ),
    (
        "rename",
        r#"<path d="M2.6 12.4V13.4h1l7.6-7.6-1-1zM10.2 3.8l1-1 2 2-1 1"/>"#,
    ),
    (
        "zip",
        r#"<path d="M3.6 2.4h6l2.8 2.8v8.4H3.6z"/><path d="M7 2.4v1.2M7 4.8V6M7 7.2v1.2"/><rect x="6.1" y="9.4" width="1.8" height="2" rx=".4"/>"#,
    ),
    (
        "extract",
        r#"<path d="M2.6 6.6v6c0 .6.4 1 1 1h8.8c.6 0 1-.4 1-1v-6"/><path d="M8 10.6V2.4M5.4 4.8 8 2.2l2.6 2.6"/>"#,
    ),
    (
        "newf",
        r#"<path d="M2 4.4c0-.6.4-1 1-1h3.2l1.4 1.5H13c.6 0 1 .4 1 1v6.4c0 .6-.4 1-1 1H3c-.6 0-1-.4-1-1z"/><path d="M8 7v4M6 9h4"/>"#,
    ),
    (
        "usb",
        r#"<rect x="5.4" y="1.8" width="5.2" height="3.8" rx=".6"/><rect x="3.8" y="5.6" width="8.4" height="8.6" rx="1.8"/><path d="M7 3.5h.1M9 3.5h.1"/>"#,
    ),
    (
        "send",
        r#"<path d="M14 2 6.8 9.2M14 2 9.6 14l-2.8-4.8L2 6.4z"/>"#,
    ),
    (
        "home",
        r#"<path d="M2.5 7.3 8 2.8l5.5 4.5"/><path d="M4 6.2V13.2h3V9.8h2v3.4h3V6.2"/>"#,
    ),
    (
        "place",
        r#"<rect x="1.8" y="2.8" width="12.4" height="10.4" rx="1.6"/><path d="M5.6 2.8v10.4M8.6 8h3.4M10.3 6.3v3.4"/>"#,
    ),
    (
        "unplace",
        r#"<rect x="1.8" y="2.8" width="12.4" height="10.4" rx="1.6"/><path d="M5.6 2.8v10.4M8.6 8h3.4"/>"#,
    ),
    (
        "shield",
        r#"<path d="M8 1.8 13 3.6v4c0 3.2-2.2 5.4-5 6.6-2.8-1.2-5-3.4-5-6.6v-4z"/><path d="m5.8 8 1.6 1.6 2.8-3"/>"#,
    ),
    (
        "selall",
        r#"<rect x="2.4" y="2.4" width="11.2" height="11.2" rx="1.6" stroke-dasharray="2 1.6"/>"#,
    ),
    // The check of a menu's row, off and on.
    (
        "box",
        r#"<rect x="2.6" y="2.6" width="10.8" height="10.8" rx="2.4"/>"#,
    ),
    (
        "boxed",
        r#"<rect x="2.6" y="2.6" width="10.8" height="10.8" rx="2.4"/><path d="m5.2 8.2 2 2 3.6-4"/>"#,
    ),
    (
        "cleanup",
        r#"<rect x="2.4" y="2.4" width="4.4" height="4.4" rx="1"/><rect x="9.2" y="2.4" width="4.4" height="4.4" rx="1"/><rect x="2.4" y="9.2" width="4.4" height="4.4" rx="1"/><rect x="9.2" y="9.2" width="4.4" height="4.4" rx="1"/>"#,
    ),
    (
        "trash",
        r#"<path d="M2.8 4.4h10.4M6.2 4.4V2.8h3.6v1.6M4.2 4.4l.7 8.8h6.2l.7-8.8M6.8 6.8v4M9.2 6.8v4"/>"#,
    ),
    (
        "erase",
        r#"<path d="M2.8 4.4h10.4M6.2 4.4V2.8h3.6v1.6M4.2 4.4l.7 8.8h6.2l.7-8.8M6.6 7.2l2.8 3.6M9.4 7.2l-2.8 3.6"/>"#,
    ),
    // Things plugged in, and what they do.
    (
        "check",
        r#"<circle cx="8" cy="8" r="5.8"/><path d="m5.4 8.2 1.8 1.8 3.4-3.6"/>"#,
    ),
    (
        "drive",
        r#"<rect x="1.8" y="4.6" width="12.4" height="6.8" rx="1.6"/><path d="M4.4 8h3.6"/><circle cx="11.4" cy="8" r=".5"/>"#,
    ),
    ("eject", r#"<path d="M8 3 13 9H3z"/><path d="M3 12.4h10"/>"#),
    (
        "phone",
        r#"<rect x="4.4" y="1.8" width="7.2" height="12.4" rx="1.6"/><path d="M7.2 12.2h1.6"/>"#,
    ),
    (
        "pc",
        r#"<rect x="1.8" y="2.8" width="12.4" height="8.4" rx="1.2"/><path d="M5.6 13.6h4.8M8 11.2v2.4"/>"#,
    ),
    (
        "mirror",
        r#"<rect x="1.8" y="3" width="8.6" height="6.4" rx="1"/><rect x="10.6" y="6" width="3.6" height="7" rx=".8"/><path d="M4.4 12.6h3.4"/>"#,
    ),
    (
        "camera",
        r#"<path d="M2 5.4c0-.6.4-1 1-1h1.8L6 2.8h4l1.2 1.6H13c.6 0 1 .4 1 1v6.6c0 .6-.4 1-1 1H3c-.6 0-1-.4-1-1z"/><circle cx="8" cy="8.4" r="2.4"/>"#,
    ),
    (
        "download",
        r#"<path d="M8 2.5v7.6M4.6 7 8 10.4 11.4 7M3 13.4h10"/>"#,
    ),
    (
        "net",
        r#"<circle cx="8" cy="8" r="5.6"/><path d="M2.4 8h11.2M8 2.4c2.2 2.2 2.2 9 0 11.2M8 2.4c-2.2 2.2-2.2 9 0 11.2"/>"#,
    ),
    (
        "gear",
        r#"<circle cx="8" cy="8" r="2"/><path d="M8 1.8v1.6M8 12.6v1.6M1.8 8h1.6M12.6 8h1.6M3.6 3.6l1.1 1.1M11.3 11.3l1.1 1.1M3.6 12.4l1.1-1.1M11.3 4.7l1.1-1.1"/>"#,
    ),
    // The desktop's own.
    (
        "pic",
        r#"<rect x="2" y="3" width="12" height="10" rx="1.6"/><circle cx="5.8" cy="6.4" r="1.1"/><path d="m2.4 11.6 3.4-3.2 2.6 2.4 2-1.8 3.2 2.9"/>"#,
    ),
    (
        "next",
        r#"<path d="M3.6 3.4v9.2L10 8z"/><path d="M12.4 3.4v9.2"/>"#,
    ),
    ("play", r#"<path d="M5 3.2v9.6L12.6 8z"/>"#),
    ("pause", r#"<path d="M5.6 3.6v8.8M10.4 3.6v8.8"/>"#),
    (
        "palette",
        r#"<path d="M8 2.2a5.8 5.8 0 1 0 0 11.6c.9 0 1.3-.5 1.3-1.1 0-.7-.6-1-.6-1.7 0-.6.5-1 1.1-1h1.5c1.5 0 2.5-1 2.5-2.6C13.8 4.5 11.2 2.2 8 2.2z"/><circle cx="5.2" cy="7" r=".7"/><circle cx="7.6" cy="4.9" r=".7"/><circle cx="10.4" cy="5.6" r=".7"/>"#,
    ),
    (
        "display",
        r#"<rect x="1.8" y="2.8" width="12.4" height="8.4" rx="1.2"/><path d="M5.6 13.6h4.8M8 11.2v2.4"/><path d="M6 8.6 8 5.2l2 3.4"/>"#,
    ),
    // What a kind of thing is for (songs, photos, documents).
    (
        "shuffle",
        r#"<path d="M2.4 4.6h2.2c1.6 0 2.4.8 3.4 3.4s1.8 3.4 3.4 3.4h2.2M2.4 11.4h2.2c1 0 1.7-.3 2.3-1.1M9.1 5.7c.6-.8 1.3-1.1 2.3-1.1h2.2M12 3l1.6 1.6L12 6.2M12 9.8l1.6 1.6-1.6 1.6"/>"#,
    ),
    (
        "slides",
        r#"<rect x="1.8" y="2.8" width="12.4" height="8.6" rx="1.4"/><path d="M6.8 5.2v3.8l3-1.9z"/><path d="M5.6 13.6h4.8"/>"#,
    ),
    (
        "rotate",
        r#"<path d="M3.2 8a4.8 4.8 0 1 0 1.6-3.6"/><path d="M2.8 2.6v2.4h2.4"/>"#,
    ),
    (
        "print",
        r#"<path d="M4.4 6V2.4h7.2V6"/><rect x="2" y="6" width="12" height="5.6" rx="1.2"/><path d="M4.4 9.6h7.2v4H4.4z"/>"#,
    ),
    (
        "more",
        r##"<circle cx="3.4" cy="8" r=".9" fill="#000"/><circle cx="8" cy="8" r=".9" fill="#000"/><circle cx="12.6" cy="8" r=".9" fill="#000"/>"##,
    ),
];

/// Where the symbols are written.
fn dir() -> PathBuf {
    std::env::var_os("XDG_CACHE_HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(std::env::var("HOME").unwrap_or_default()).join(".cache"))
        .join("waverunner/files-glyphs")
}

/// The icon key of symbol `name` (an `asset:` key by its absolute path).
pub(crate) fn key(name: &str) -> String {
    format!("asset:{}/{name}.svg", dir().display())
}

/// Whether `key` is one of these symbols.
pub(crate) fn is_key(key: &str) -> bool {
    key.strip_prefix("asset:")
        .is_some_and(|p| p.starts_with(&*dir().to_string_lossy()))
}

/// Write the symbols where `key` says (only what is not there as it is),
/// once a run.
pub(crate) fn write_all() {
    static ONCE: std::sync::Once = std::sync::Once::new();
    ONCE.call_once(|| {
        let dir = dir();
        if std::fs::create_dir_all(&dir).is_err() {
            return;
        }
        for (name, body) in GLYPHS {
            let svg = format!(
                r##"<svg xmlns="http://www.w3.org/2000/svg" width="16" height="16" viewBox="0 0 16 16" fill="none" stroke="#000" stroke-width="1.35" stroke-linecap="round" stroke-linejoin="round">{body}</svg>"##
            );
            let path = dir.join(format!("{name}.svg"));
            if std::fs::read_to_string(&path).ok().as_deref() != Some(svg.as_str()) {
                crate::persist::write_text("files-glyph", &path, &svg);
            }
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_symbol_has_one_name() {
        let mut names: Vec<&str> = GLYPHS.iter().map(|(n, _)| *n).collect();
        names.sort_unstable();
        let all = names.len();
        names.dedup();
        assert_eq!(names.len(), all);
    }
}
