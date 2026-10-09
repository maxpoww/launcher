//! What is ON the card: the items, how they are kept on disk, how a text is
//! wrapped, and what each one offers when it is dragged out.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

/// A text keeps this many lines on the card; the whole of it still leaves
/// with a drag.
pub(super) const MAX_LINES: usize = 12;

/// The largest drop read into memory (a picture out of a web page).
pub(super) const DROP_MAX: u64 = 48 * 1024 * 1024;

pub(super) const URI_LIST: &str = "text/uri-list";

pub(super) const TEXT_MIMES: [&str; 5] = [
    "text/plain;charset=utf-8",
    "UTF8_STRING",
    "text/plain",
    "TEXT",
    "STRING",
];

pub(super) const IMAGE_MIMES: [(&str, &str); 6] = [
    ("image/png", "png"),
    ("image/jpeg", "jpg"),
    ("image/webp", "webp"),
    ("image/gif", "gif"),
    ("image/bmp", "bmp"),
    ("image/svg+xml", "svg"),
];

pub(super) const ITEMS_FILE: &str = "card.json";

/// Where pictures that came as pixels are kept (under the data directory).
pub(super) const PICTURES_DIR: &str = "card";

/// What an item is.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub(crate) enum Kind {
    Text,
    Image,
    File,
    Folder,
    /// A voice note: `path` is its recording, `aspect` holds how many
    /// seconds it lasts.
    Voice,
}

impl Kind {
    /// The word over the item (a text has none).
    pub(super) fn word(self) -> Option<&'static str> {
        match self {
            Kind::Text => None,
            Kind::Image => Some("IMAGE"),
            Kind::File => Some("FILE"),
            Kind::Folder => Some("FOLDER"),
            Kind::Voice => None,
        }
    }
}

/// One thing on the card.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub(crate) struct Item {
    pub id: u64,
    pub kind: Kind,
    /// A text: the text. Anything else: the line shown under the kind word
    /// (the name and the size).
    pub body: String,
    /// The file, folder or picture on disk.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub path: Option<String>,
    /// A picture's width over its height (0 = not known).
    #[serde(default)]
    pub aspect: f32,
    /// The picture is the card's own copy (it came as pixels): it goes
    /// with the item.
    #[serde(default)]
    pub owned: bool,
    /// When it was put on the card (seconds since 1970; 0 = not known),
    /// and the app it came from (its window class) — kept with the item
    /// so what is remembered can say where and when it is from.
    #[serde(default)]
    pub at: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub from: Option<String>,
}

impl Item {
    /// Whether `other` is the same THING (a pinned copy of it, say),
    /// whatever its id.
    pub(super) fn same(&self, other: &Item) -> bool {
        self.kind == other.kind && self.body == other.body && self.path == other.path
    }
}

/// A session: one working set of things, named after the window it was
/// started in. MEMORY is the list of them all; each window shows the one it
/// was given (Max, 2026-10-09: *"if i start a new session, it takes the name
/// of the current window and appears on memory right away"*).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub(crate) struct Past {
    pub id: u64,
    /// When it was started (seconds since 1970).
    pub at: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    pub items: Vec<Item>,
}

/// Everything the card keeps, as it is on disk: the sessions (newest
/// first), the pinned items, and which session each window TITLE was given
/// (so the window of a task finds its session again another day). `items`
/// is the one list of before there were sessions; it is read into a session
/// of its own and written empty.
#[derive(Default, Serialize, Deserialize)]
pub(super) struct Saved {
    pub next_id: u64,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub items: Vec<Item>,
    #[serde(default)]
    pub pinned: Vec<Item>,
    #[serde(default)]
    pub memory: Vec<Past>,
    #[serde(default)]
    pub titles: std::collections::HashMap<String, u64>,
    /// How big the items are drawn (0 = never set: as designed).
    #[serde(default)]
    pub zoom: f32,
    /// The emoji used lately, the latest first (the picker shows them
    /// before the rest).
    #[serde(default)]
    pub emoji: Vec<String>,
}

/// The seconds since 1970, now.
pub(super) fn now_secs() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs())
}

/// A memory as ONE row of Memory's page, as a messenger lists a chat: its
/// name on the first line, the last thing in it on the second. (A row is
/// drawn from an item whose id is the memory's and whose time is its last
/// thing's.)
pub(super) fn session_row(session: &Past) -> Item {
    let name = session
        .name
        .clone()
        .unwrap_or_else(|| "Untitled".to_owned());
    let last = match session.items.last() {
        Some(item) if item.kind == Kind::Voice => "Voice note".to_owned(),
        Some(item) => item
            .body
            .trim()
            .lines()
            .next()
            .unwrap_or("")
            .trim()
            .to_owned(),
        None => "Nothing yet".to_owned(),
    };
    Item {
        id: session.id,
        kind: Kind::Text,
        body: format!("{name}\n{last}"),
        path: None,
        aspect: 0.0,
        owned: false,
        at: session
            .items
            .last()
            .map_or(session.at, |it| it.at.max(session.at)),
        from: None,
    }
}

/// When `at` was, as a messenger says it next to `now`: the time for
/// today, the day for anything older ("14:02", "Oct 6"). Local time.
pub(crate) fn when_text(at: u64, now: u64) -> String {
    const MONTHS: [&str; 12] = [
        "Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec",
    ];
    if at == 0 {
        return String::new();
    }
    let local = |secs: u64| -> Option<libc::tm> {
        let secs = secs as libc::time_t;
        // SAFETY: `localtime_r` fills a caller-owned `tm` from a valid `time_t`.
        unsafe {
            let mut tm: libc::tm = std::mem::zeroed();
            (!libc::localtime_r(&secs, &mut tm).is_null()).then_some(tm)
        }
    };
    let (Some(then), Some(today)) = (local(at), local(now)) else {
        return String::new();
    };
    if (then.tm_year, then.tm_yday) == (today.tm_year, today.tm_yday) {
        format!("{:02}:{:02}", then.tm_hour, then.tm_min)
    } else {
        format!(
            "{} {}",
            MONTHS[(then.tm_mon as usize).min(11)],
            then.tm_mday
        )
    }
}

/// What a drag out hands over for one type.
#[derive(Debug, Clone, PartialEq)]
pub(crate) enum Payload {
    Bytes(Vec<u8>),
    /// The file's own bytes, read when asked for.
    File(PathBuf),
}

/// Break `text` into lines of at most `cols` characters: its own line
/// breaks are kept, a long line breaks at its last space (mid-word where
/// there is none), a line's indentation survives. At most `max` lines; the
/// last ends in "…" when more was left.
pub(crate) fn wrap(text: &str, cols: usize, max: usize) -> Vec<String> {
    let cols = cols.max(4);
    let text = text.replace('\t', "    ");
    let mut out: Vec<String> = Vec::new();
    let mut cut = false;
    'lines: for raw in text.trim_end().split('\n') {
        let chars: Vec<char> = raw.trim_end().chars().collect();
        if chars.is_empty() {
            if out.len() >= max {
                cut = true;
                break;
            }
            out.push(String::new());
            continue;
        }
        let mut start = 0;
        while start < chars.len() {
            if out.len() >= max {
                cut = true;
                break 'lines;
            }
            let mut end = (start + cols).min(chars.len());
            if end < chars.len() && chars[end] != ' ' {
                if let Some(space) = (start + 1..end).rev().find(|&i| chars[i] == ' ') {
                    end = space;
                }
            }
            out.push(
                chars[start..end]
                    .iter()
                    .collect::<String>()
                    .trim_end()
                    .to_owned(),
            );
            start = end;
            while start < chars.len() && chars[start] == ' ' {
                start += 1;
            }
        }
    }
    if cut {
        if let Some(last) = out.last_mut() {
            let mut kept: String = last.chars().take(cols.saturating_sub(1)).collect();
            kept.push('…');
            *last = kept;
        }
    }
    out
}

/// Break `text` into the lines of a box `cols` characters wide, as RANGES
/// of its characters (start, end): its own line breaks end a line (the
/// break itself is in no line), a long line breaks after its last space
/// (mid-word where there is none). Unlike [`wrap`] nothing is trimmed or
/// dropped, so a place in a line is a place in the text — what a writing
/// cursor needs. A text that ends in a line break has an empty last line.
#[cfg(test)]
pub(crate) fn wrap_spans(text: &str, cols: usize) -> Vec<(usize, usize)> {
    wrap_spans_by(text, cols.max(4) as f32, |_| 1.0)
}

/// The same, by MEASURE: a line holds as many characters as fit in `room`,
/// each as wide as `width` says. (The input box breaks its lines by the
/// real widths of its letters — by a count of them it broke early and left
/// room unused at the right: Max, 2026-10-09.) A line always takes at
/// least one character.
pub(crate) fn wrap_spans_by(
    text: &str,
    room: f32,
    mut width: impl FnMut(char) -> f32,
) -> Vec<(usize, usize)> {
    let chars: Vec<char> = text.chars().collect();
    let mut spans = Vec::new();
    let mut start = 0;
    loop {
        let end = (start..chars.len())
            .find(|&i| chars[i] == '\n')
            .unwrap_or(chars.len());
        if start == end {
            spans.push((start, end));
        }
        let mut at = start;
        while at < end {
            // As far as fits…
            let mut to = at;
            let mut used = 0.0;
            while to < end {
                let w = width(chars[to]);
                if to > at && used + w > room {
                    break;
                }
                used += w;
                to += 1;
            }
            // …and back to after the last space, if the line was cut.
            if to < end {
                if let Some(space) = (at + 1..=to).rev().find(|&i| chars[i - 1] == ' ') {
                    to = space;
                }
            }
            spans.push((at, to));
            at = to;
        }
        if end == chars.len() {
            return spans;
        }
        start = end + 1;
    }
}

/// What a local path is to the card.
pub(crate) fn kind_of(path: &Path) -> Kind {
    if path.is_dir() {
        Kind::Folder
    } else if crate::files::file_asset_name(&path.to_string_lossy()) == "asset-image" {
        Kind::Image
    } else {
        Kind::File
    }
}

/// What the card keeps of a file, a folder or a picture: read from the disk,
/// which can be slow (a folder is counted; a phone's storage answers when it
/// pleases) — so never on the loop (`App::card_add_paths`).
#[derive(Debug, Clone, PartialEq)]
pub(super) struct Facts {
    pub kind: Kind,
    pub body: String,
    pub path: String,
    pub aspect: f32,
}

/// Read `path`'s facts; `None` if it is not there.
pub(super) fn facts(path: &Path) -> Option<Facts> {
    if !path.exists() {
        return None;
    }
    let kind = kind_of(path);
    Some(Facts {
        kind,
        body: file_body(path),
        path: path.to_string_lossy().into_owned(),
        aspect: if kind == Kind::Image {
            aspect_of(path)
        } else {
            0.0
        },
    })
}

/// The line under a file's kind word: its name, and its size (a folder:
/// how many things are in it).
pub(super) fn file_body(path: &Path) -> String {
    let name = path.file_name().map_or_else(
        || path.to_string_lossy().into_owned(),
        |n| n.to_string_lossy().into_owned(),
    );
    let Ok(meta) = std::fs::metadata(path) else {
        return name;
    };
    if meta.is_dir() {
        match std::fs::read_dir(path) {
            Ok(dir) => {
                let n = dir.count();
                format!("{name}/ · {n} item{}", if n == 1 { "" } else { "s" })
            }
            Err(_) => format!("{name}/"),
        }
    } else {
        format!("{name} · {}", size_text(meta.len()))
    }
}

/// A file's size as the mockup writes it: "9.8 KB", "2.4 MB".
pub(crate) fn size_text(bytes: u64) -> String {
    const UNITS: [&str; 4] = ["KB", "MB", "GB", "TB"];
    if bytes < 1024 {
        return format!("{bytes} B");
    }
    let mut value = bytes as f64 / 1024.0;
    let mut unit = 0;
    while value >= 1024.0 && unit + 1 < UNITS.len() {
        value /= 1024.0;
        unit += 1;
    }
    if value >= 100.0 {
        format!("{value:.0} {}", UNITS[unit])
    } else {
        format!("{value:.1} {}", UNITS[unit])
    }
}

/// A picture's width over its height, from the file's header.
pub(super) fn aspect_of(path: &Path) -> f32 {
    match image::image_dimensions(path) {
        Ok((w, h)) if w > 0 && h > 0 => w as f32 / h as f32,
        _ => 0.0,
    }
}

/// The first of our text types among `mimes`.
pub(crate) fn text_mime(mimes: &[String]) -> Option<&'static str> {
    TEXT_MIMES
        .into_iter()
        .find(|t| mimes.iter().any(|m| m == t))
}

/// The first of our picture types among `mimes`, and its file extension.
pub(crate) fn image_mime(mimes: &[String]) -> Option<(&'static str, &'static str)> {
    IMAGE_MIMES
        .into_iter()
        .find(|(t, _)| mimes.iter().any(|m| m == t))
}

/// The picture type a file's name says it is.
pub(super) fn mime_of(path: &str) -> Option<&'static str> {
    let ext = Path::new(path).extension()?.to_str()?.to_ascii_lowercase();
    let ext = if ext == "jpeg" { "jpg".to_owned() } else { ext };
    IMAGE_MIMES
        .into_iter()
        .find(|(_, e)| *e == ext)
        .map(|(t, _)| t)
}

/// The types a drag of `item` offers, the richest first: a file as a list
/// of URIs (and a picture as its own pixels), then its path as text; a
/// text as text.
pub(crate) fn out_mimes(item: &Item) -> Vec<&'static str> {
    let mut mimes = Vec::new();
    if let Some(path) = item.path.as_deref() {
        mimes.push(URI_LIST);
        if item.kind == Kind::Image {
            if let Some(mime) = mime_of(path) {
                mimes.push(mime);
            }
        }
    }
    mimes.extend(TEXT_MIMES);
    mimes
}

/// What `item` hands over as `mime` (`None`: a type it never offered).
pub(crate) fn payload(item: &Item, mime: &str) -> Option<Payload> {
    match item.path.as_deref() {
        Some(path) if mime == URI_LIST => Some(Payload::Bytes(
            format!("{}\r\n", crate::desktop::file_uri(path)).into_bytes(),
        )),
        Some(path) if item.kind == Kind::Image && mime_of(path) == Some(mime) => {
            Some(Payload::File(PathBuf::from(path)))
        }
        Some(path) if TEXT_MIMES.contains(&mime) => Some(Payload::Bytes(path.as_bytes().to_vec())),
        None if TEXT_MIMES.contains(&mime) => Some(Payload::Bytes(item.body.as_bytes().to_vec())),
        _ => None,
    }
}

/// The name at the end of the first remote address in a URI list (a
/// picture dragged out of a web page says where it came from).
pub(super) fn remote_name(list: &str) -> Option<String> {
    let url = list
        .lines()
        .map(str::trim)
        .find(|l| l.starts_with("http://") || l.starts_with("https://"))?;
    let path = url.split(['?', '#']).next()?;
    let name = path.rsplit('/').next().filter(|n| !n.is_empty())?;
    Some(
        crate::trash::decode_path(name)
            .to_string_lossy()
            .into_owned(),
    )
}

pub(super) fn load() -> Saved {
    crate::persist::read_json(&crate::persist::data_path(ITEMS_FILE)).unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn text(id: u64, body: &str) -> Item {
        Item {
            id,
            kind: Kind::Text,
            body: body.to_owned(),
            path: None,
            aspect: 0.0,
            owned: false,
            at: 0,
            from: None,
        }
    }

    fn file(id: u64, kind: Kind, path: &str) -> Item {
        Item {
            id,
            kind,
            body: path.rsplit('/').next().unwrap_or(path).to_owned(),
            path: Some(path.to_owned()),
            aspect: 1.5,
            owned: false,
            at: 0,
            from: None,
        }
    }

    #[test]
    fn wrap_breaks_at_spaces_keeps_line_breaks_and_indentation() {
        assert_eq!(wrap("one two three", 7, 9), ["one two", "three"]);
        assert_eq!(wrap("a\n\n  b", 10, 9), ["a", "", "  b"]);
        // No space to break at: mid-word.
        assert_eq!(wrap("abcdefghij", 4, 9), ["abcd", "efgh", "ij"]);
    }

    #[test]
    fn wrap_stops_at_the_cap_with_an_ellipsis() {
        let lines = wrap("1\n2\n3\n4", 10, 2);
        assert_eq!(lines, ["1", "2…"]);
        assert_eq!(wrap("1\n2", 10, 2), ["1", "2"]);
    }

    #[test]
    fn a_text_goes_out_as_text_and_a_picture_as_file_pixels_and_path() {
        let t = text(1, "hello");
        assert_eq!(out_mimes(&t), TEXT_MIMES.to_vec());
        assert_eq!(
            payload(&t, "text/plain"),
            Some(Payload::Bytes(b"hello".to_vec()))
        );
        assert_eq!(payload(&t, URI_LIST), None);

        let p = file(2, Kind::Image, "/tmp/a b.JPEG");
        assert_eq!(out_mimes(&p)[..2], [URI_LIST, "image/jpeg"]);
        assert_eq!(
            payload(&p, URI_LIST),
            Some(Payload::Bytes(b"file:///tmp/a%20b.JPEG\r\n".to_vec()))
        );
        assert_eq!(
            payload(&p, "image/jpeg"),
            Some(Payload::File(PathBuf::from("/tmp/a b.JPEG")))
        );
        assert_eq!(payload(&p, "image/png"), None);
        assert_eq!(
            payload(&p, "UTF8_STRING"),
            Some(Payload::Bytes(b"/tmp/a b.JPEG".to_vec()))
        );

        // A plain file never offers pixels.
        let f = file(3, Kind::File, "/tmp/notes.png.txt");
        assert_eq!(out_mimes(&f)[0], URI_LIST);
        assert!(!out_mimes(&f).contains(&"image/png"));
    }

    #[test]
    fn sizes_read_as_the_mockup_writes_them() {
        assert_eq!(size_text(6), "6 B");
        assert_eq!(size_text(10_035), "9.8 KB");
        assert_eq!(size_text(2_516_582), "2.4 MB");
        assert_eq!(size_text(29_360_128), "28.0 MB");
        assert_eq!(size_text(4_402_341_478), "4.1 GB");
    }

    #[test]
    fn a_drop_is_read_as_its_richest_type() {
        let m = |v: &[&str]| v.iter().map(|s| (*s).to_owned()).collect::<Vec<_>>();
        assert_eq!(
            text_mime(&m(&["STRING", "text/plain;charset=utf-8"])),
            Some("text/plain;charset=utf-8")
        );
        assert_eq!(text_mime(&m(&["text/html"])), None);
        assert_eq!(
            image_mime(&m(&["text/html", "image/jpeg"])),
            Some(("image/jpeg", "jpg"))
        );
    }

    #[test]
    fn a_web_picture_is_named_after_the_end_of_its_address() {
        assert_eq!(
            remote_name("https://upload.example.org/a/Nokota%20Horses.jpg?w=500\r\n").as_deref(),
            Some("Nokota Horses.jpg")
        );
        assert_eq!(remote_name("file:///home/x/a.png"), None);
    }

    #[test]
    fn the_list_survives_the_disk() {
        let saved = Saved {
            next_id: 9,
            items: vec![text(1, "a\nb"), file(2, Kind::Folder, "/tmp/d")],
            pinned: vec![text(3, "me@example.org")],
            titles: [("Golem new feature".to_owned(), 4)].into(),
            zoom: 1.2,
            emoji: vec!["👍".to_owned()],
            memory: vec![Past {
                id: 4,
                at: 1_700_000_000,
                name: None,
                items: vec![text(5, "old")],
            }],
        };
        let json = serde_json::to_string(&saved).unwrap();
        let back: Saved = serde_json::from_str(&json).unwrap();
        assert_eq!(back.items, saved.items);
        assert_eq!(back.next_id, 9);
        // (A list saved while the width was kept in it still reads.)
        let old: Saved = serde_json::from_str(r#"{"next_id":1,"items":[],"width":400.0}"#).unwrap();
        assert_eq!(old.next_id, 1);
        // A text has no path on disk at all.
        assert!(!json.contains("\"path\":null"));
    }

    #[test]
    fn a_card_file_from_before_memory_still_reads() {
        let saved: Saved =
            serde_json::from_str(r#"{"next_id":3,"items":[{"id":1,"kind":"text","body":"a"}]}"#)
                .unwrap();
        assert_eq!(saved.items.len(), 1);
        assert!(saved.pinned.is_empty() && saved.memory.is_empty());
        assert_eq!(saved.items[0].at, 0);
    }

    #[test]
    fn a_memory_reads_as_its_name_and_its_last_thing() {
        let mut last = text(2, "the last one\nsecond line");
        last.at = 1_700_000_500;
        let session = Past {
            id: 9,
            at: 1_700_000_000,
            name: Some("Golem new feature".to_owned()),
            items: vec![text(1, "first"), last],
        };
        let row = session_row(&session);
        assert_eq!((row.id, row.at), (9, 1_700_000_500));
        assert_eq!(row.body, "Golem new feature\nthe last one");
        let empty = Past {
            name: None,
            items: vec![],
            ..session
        };
        assert_eq!(session_row(&empty).body, "Untitled\nNothing yet");
        assert_eq!(session_row(&empty).at, 1_700_000_000);
    }

    #[test]
    fn a_time_reads_as_the_hour_today_and_as_the_day_before() {
        let now = 1_700_000_000;
        let today = when_text(now - 60, now);
        // (Unless the minute before crossed midnight where this runs.)
        assert!(
            today.len() == 5 && today.as_bytes()[2] == b':' || today.contains(' '),
            "{today}"
        );
        let old = when_text(now - 40 * 86_400, now);
        assert!(old.contains(' ') && !old.contains(':'), "{old}");
        assert_eq!(when_text(0, now), "");
    }

    #[test]
    fn a_box_of_text_is_lines_that_keep_every_character() {
        // Its own breaks end a line; a long line breaks after a space.
        assert_eq!(wrap_spans("ab\ncd", 10), [(0, 2), (3, 5)]);
        assert_eq!(wrap_spans("one two three", 8), [(0, 8), (8, 13)]);
        // No space to break at: mid-word.
        assert_eq!(wrap_spans("abcdefghij", 4), [(0, 4), (4, 8), (8, 10)]);
        // Nothing is one empty line; a trailing break opens another.
        assert_eq!(wrap_spans("", 10), [(0, 0)]);
        assert_eq!(wrap_spans("ab\n", 10), [(0, 2), (3, 3)]);
        assert_eq!(wrap_spans("a\n\nb", 10), [(0, 1), (2, 2), (3, 4)]);
        // By measure: narrow letters fit more to a line than wide ones.
        let w = |c: char| if c == 'i' { 1.0 } else { 3.0 };
        assert_eq!(wrap_spans_by("iiiiiiii", 6.0, w), [(0, 6), (6, 8)]);
        assert_eq!(wrap_spans_by("mmmm", 6.0, w), [(0, 2), (2, 4)]);
        // A letter wider than the box still takes a line of its own.
        assert_eq!(wrap_spans_by("mm", 1.0, w), [(0, 1), (1, 2)]);
    }
}
