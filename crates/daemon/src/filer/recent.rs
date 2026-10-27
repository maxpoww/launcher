//! RECENTS (Max, 2026-10-21: "add recents"): a place of its own, first in
//! the sidebar's Favourites, that lists the files used lately, newest first.
//! What it lists is the freedesktop record every GTK and KDE app keeps,
//! `~/.local/share/recently-used.xbel`; Golem Files adds to it the
//! standard way each time it opens a file (`note`), so its own opens are
//! there too. A file that is gone is left out; at most `MAX` are shown.
//!
//! It is a place, not a folder: nothing is made, pasted or dropped into it
//! (`filer_act`, `filer_bring`); what is thrown away from it goes to the
//! bin, the real file.

use super::*;

/// The most it lists.
const MAX: usize = 100;

/// The place's own path: no folder on the disk is called this.
pub(crate) fn root() -> PathBuf {
    PathBuf::from("recent:///")
}

/// Where the record is kept.
fn xbel() -> PathBuf {
    std::env::var_os("XDG_DATA_HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| home().join(".local/share"))
        .join("recently-used.xbel")
}

/// The value of attribute `name` in a tag's text.
fn attr<'a>(tag: &'a str, name: &str) -> Option<&'a str> {
    let key = format!(" {name}=\"");
    let at = tag.find(&key)? + key.len();
    let len = tag[at..].find('"')?;
    Some(&tag[at..at + len])
}

/// Every entry of a record: the file and when it was last used (an RFC
/// 3339 time in UTC, which sorts as text), newest first, each file once.
pub(crate) fn entries(text: &str) -> Vec<(PathBuf, String)> {
    let mut out: Vec<(PathBuf, String)> = Vec::new();
    for part in text.split("<bookmark ").skip(1) {
        let Some(tag) = part.split('>').next() else {
            continue;
        };
        let tag = format!(" {tag}");
        let Some(href) = attr(&tag, "href") else {
            continue;
        };
        let Some(path) = href.strip_prefix("file://") else {
            continue;
        };
        let path = PathBuf::from(crate::trash::decode_path(&path.replace("&amp;", "&")));
        let when = ["visited", "modified", "added"]
            .iter()
            .filter_map(|k| attr(&tag, k))
            .max()
            .unwrap_or("")
            .to_owned();
        out.push((path, when));
    }
    out.sort_by(|a, b| b.1.cmp(&a.1));
    let mut seen = HashSet::new();
    out.retain(|(p, _)| seen.insert(p.clone()));
    out
}

/// The place's things: the files used lately that are still there.
pub(crate) fn list() -> Vec<Item> {
    let text = std::fs::read_to_string(xbel()).unwrap_or_default();
    entries(&text)
        .into_iter()
        .filter(|(p, _)| p.is_file())
        .take(MAX)
        .map(|(p, _)| {
            let name = file_name(&p);
            crate::desktop::item_for(p, name, false)
        })
        .collect()
}

/// Now, as the record writes a time.
fn now() -> String {
    let since = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default();
    let secs = since.as_secs() as libc::time_t;
    // SAFETY: `gmtime_r` fills a caller-owned `tm` from a valid `time_t`.
    unsafe {
        let mut tm: libc::tm = std::mem::zeroed();
        if libc::gmtime_r(&secs, &mut tm).is_null() {
            return "1970-01-01T00:00:00Z".to_owned();
        }
        format!(
            "{:04}-{:02}-{:02}T{:02}:{:02}:{:02}.{:06}Z",
            tm.tm_year + 1900,
            tm.tm_mon + 1,
            tm.tm_mday,
            tm.tm_hour,
            tm.tm_min,
            tm.tm_sec,
            since.subsec_micros()
        )
    }
}

/// `text` with the entry for `href` used again at `when` — or with a new
/// entry for it, as GTK writes one (its kind, Golem Files as the app).
pub(crate) fn noted(text: &str, href: &str, mime: &str, when: &str) -> String {
    noted_by(text, href, mime, when, ("Golem Files", "xdg-open %u"))
}

/// [`noted`], a new entry's app being `app` (its name and command).
fn noted_by(text: &str, href: &str, mime: &str, when: &str, (app, exec): (&str, &str)) -> String {
    let key = format!("<bookmark href=\"{href}\"");
    if let Some(at) = text.find(&key) {
        let end = text[at..].find('>').map_or(text.len(), |e| at + e);
        let mut tag = text[at..end].to_owned();
        for name in ["modified", "visited"] {
            let k = format!(" {name}=\"");
            match tag.find(&k) {
                Some(i) => {
                    let from = i + k.len();
                    let len = tag[from..].find('"').unwrap_or(0);
                    tag.replace_range(from..from + len, when);
                }
                None => tag.push_str(&format!(" {name}=\"{when}\"")),
            }
        }
        return format!("{}{tag}{}", &text[..at], &text[end..]);
    }
    let (app, exec) = (xml(app), xml(exec));
    let entry = format!(
        "  <bookmark href=\"{href}\" added=\"{when}\" modified=\"{when}\" visited=\"{when}\">\n    <info>\n      <metadata owner=\"http://freedesktop.org\">\n        <mime:mime-type type=\"{mime}\"/>\n        <bookmark:applications>\n          <bookmark:application name=\"{app}\" exec=\"&apos;{exec}&apos;\" modified=\"{when}\" count=\"1\"/>\n        </bookmark:applications>\n      </metadata>\n    </info>\n  </bookmark>\n"
    );
    match text.rfind("</xbel>") {
        Some(end) => format!("{}{entry}{}", &text[..end], &text[end..]),
        None => format!(
            "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<xbel version=\"1.0\"\n      xmlns:bookmark=\"http://www.freedesktop.org/standards/desktop-bookmarks\"\n      xmlns:mime=\"http://www.freedesktop.org/standards/shared-mime-info\"\n>\n{entry}</xbel>\n"
        ),
    }
}

/// Golem Files opened `path`: it goes in the record (on the caller's
/// thread — it reads and writes a file).
pub(crate) fn note(path: &Path) {
    note_as(path, None);
}

/// [`note`], as opened by `app` (its name and command) when it is known
/// — the dock box notes what it opens as opened by the app that opens it,
/// so that app's own recents have it too —, else by Golem Files.
pub(crate) fn note_as(path: &Path, app: Option<&(String, String)>) {
    let file = xbel();
    let text = std::fs::read_to_string(&file).unwrap_or_default();
    let lower = file_name(path).to_ascii_lowercase();
    let mime = crate::thumb_more::mime_of(&lower).unwrap_or("application/octet-stream");
    let href = format!("file://{}", crate::trash::encode_path(path));
    let by = app.map_or(("Golem Files", "xdg-open %u"), |(name, exec)| (name.as_str(), exec.as_str()));
    crate::persist::write_text("recently-used", &file, &noted_by(&text, &href, mime, &now(), by));
}

/// Text as an XML attribute's value.
fn xml(s: &str) -> String {
    s.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;").replace('"', "&quot;").replace('\'', "&apos;")
}

impl App {
    /// Golem Files opened `path` (a file): noted where every app looks
    /// for what was used lately, off the loop; a Recents window shows it.
    pub(super) fn filer_used(&mut self, path: &Path) {
        let path = path.to_path_buf();
        self.desktop_off_loop(
            move || note(&path),
            |app, ()| {
                let ids: Vec<u32> = app
                    .filer
                    .wins
                    .iter()
                    .filter(|w| w.dir == root())
                    .map(|w| w.id)
                    .collect();
                for id in ids {
                    app.filer_list(id);
                }
            },
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_record_is_read_newest_first_each_file_once() {
        let text = r#"<?xml version="1.0"?>
<xbel version="1.0">
  <bookmark href="file:///home/max/a%20b.txt" added="2026-10-01T10:00:00Z" modified="2026-10-01T10:00:00Z" visited="2026-10-01T10:00:00Z">
  </bookmark>
  <bookmark href="file:///home/max/new.png" added="2026-10-20T10:00:00Z" modified="2026-10-20T10:00:00Z" visited="2026-10-20T10:00:00Z">
  </bookmark>
  <bookmark href="https://example.com/" added="2026-10-21T10:00:00Z" modified="2026-10-21T10:00:00Z" visited="2026-10-21T10:00:00Z">
  </bookmark>
</xbel>"#;
        let got: Vec<PathBuf> = entries(text).into_iter().map(|(p, _)| p).collect();
        assert_eq!(
            got,
            [
                PathBuf::from("/home/max/new.png"),
                PathBuf::from("/home/max/a b.txt")
            ]
        );
    }

    #[test]
    fn a_file_opened_is_added_or_its_entry_used_again() {
        let href = "file:///home/max/x.txt";
        let fresh = noted("", href, "text/plain", "2026-10-21T00:00:00.000000Z");
        assert!(fresh.starts_with("<?xml") && fresh.trim_end().ends_with("</xbel>"));
        assert_eq!(entries(&fresh)[0].0, PathBuf::from("/home/max/x.txt"));
        let again = noted(&fresh, href, "text/plain", "2026-10-22T00:00:00.000000Z");
        assert_eq!(again.matches("<bookmark ").count(), 1);
        assert_eq!(entries(&again)[0].1, "2026-10-22T00:00:00.000000Z");
        // (Another file: a second entry, the first kept.)
        let two = noted(
            &again,
            "file:///home/max/y.txt",
            "text/plain",
            "2026-10-23T00:00:00.000000Z",
        );
        assert_eq!(two.matches("<bookmark ").count(), 2);
        assert_eq!(entries(&two)[0].0, PathBuf::from("/home/max/y.txt"));
    }

    #[test]
    fn what_the_box_opens_is_noted_as_opened_by_its_app() {
        let href = "file:///home/max/n.txt";
        let by = noted_by("", href, "text/plain", "2026-10-27T00:00:00.000000Z", ("Neovim \"wrapper\"", "nvim %F"));
        assert!(by.contains(r#"<bookmark:application name="Neovim &quot;wrapper&quot;" exec="&apos;nvim %F&apos;""#), "{by}");
        // (Used again: its entry's time, its app kept.)
        let again = noted(&by, href, "text/plain", "2026-10-28T00:00:00.000000Z");
        assert!(again.contains("Neovim") && !again.contains("Golem Files"));
        assert_eq!(entries(&again)[0].1, "2026-10-28T00:00:00.000000Z");
    }
}
