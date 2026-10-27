//! FILE SEARCH — what the box's Files section finds for a query (Max,
//! 2026-10-27: "maybe improve file search?" … "do 1 to 7"). The home's files
//! are walked off the loop (`apps::scan_home_files`, with each one's folder
//! and when it changed) and ranked here:
//!
//! - BY NAME — each word a piece of it, word starts first (`nucleo`'s
//!   substring: scattered letters, as apps are found, brought "notification"
//!   for "notas" out of twenty thousand files) — and WORD BY WORD THROUGH
//!   THE PATH: every word
//!   of the query is in the file's name or, from the start of one of its
//!   words, in the folders it is in — "playa 2024" finds
//!   `Pictures/Playa 2024/IMG_0412.jpg`, "factura junio" a PDF in
//!   `Documents/Facturas`. One word alone brings what is in a folder of
//!   that name too ("downloads": what came lately), below what it names —
//!   a word of three letters at least.
//! - KIND WORDS narrow to a kind, in Spanish and English ("pdf contrato",
//!   "fotos playa", "videos", "música"); alone, they list that kind, the
//!   newest first.
//! - THE OWNER'S USE first: what was opened lately (the system's record,
//!   `recently-used.xbel` — where the box now notes what it opens, too) and
//!   what changed lately (a download) weigh more, each forgetting as the
//!   dock's launches do (`usage::decayed`).
//!
//! - WHEN AND HOW BIG, in words (Max, 2026-10-27, after Windows, Spotlight,
//!   Everything): "hoy"/"today", "ayer"/"yesterday", "esta semana"/"this
//!   week", "semana pasada"/"last week", "este mes"/"this month", "este
//!   año"/"this year"; ">10mb", "<500kb", "grandes"/"big" (100 MB and up) —
//!   each keeps to the files changed or opened then, or of that size, and is
//!   not a word to find; alone, they list them, the newest first.
//! - A TYPO STILL FINDS (Windows' "utlook"): when nothing has every word, a
//!   word of four letters or more is found in a name's word one letter off
//!   (two from eight letters: `near`) — a letter missing, one too many, one
//!   changed, two swapped.
//!
//! The owner's folders' tops (Desktop, Documents, Downloads, Music,
//! Pictures and its Screenshots, Videos) are WATCHED (`watched_dirs`): what
//! lands there is found at once, not at the next walk (`App::files_fresh`).

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use nucleo_matcher::pattern::{Atom, AtomKind, CaseMatching, Normalization};
use nucleo_matcher::{Matcher, Utf32Str};

use crate::apps::FileEntry;

/// A kind of file a word of the query can ask for.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Kind {
    Pdf,
    Picture,
    Video,
    Music,
    Document,
    Archive,
    Folder,
}

/// The words that ask for a kind, folded (`fold`), Spanish and English.
const KIND_WORDS: &[(&str, Kind)] = &[
    ("pdf", Kind::Pdf),
    ("pdfs", Kind::Pdf),
    ("foto", Kind::Picture),
    ("fotos", Kind::Picture),
    ("photo", Kind::Picture),
    ("photos", Kind::Picture),
    ("imagen", Kind::Picture),
    ("imagenes", Kind::Picture),
    ("image", Kind::Picture),
    ("images", Kind::Picture),
    ("pictures", Kind::Picture),
    ("video", Kind::Video),
    ("videos", Kind::Video),
    ("pelicula", Kind::Video),
    ("peliculas", Kind::Video),
    ("musica", Kind::Music),
    ("music", Kind::Music),
    ("cancion", Kind::Music),
    ("canciones", Kind::Music),
    ("song", Kind::Music),
    ("songs", Kind::Music),
    ("documento", Kind::Document),
    ("documentos", Kind::Document),
    ("document", Kind::Document),
    ("documents", Kind::Document),
    ("docs", Kind::Document),
    ("comprimido", Kind::Archive),
    ("comprimidos", Kind::Archive),
    ("archive", Kind::Archive),
    ("archives", Kind::Archive),
    ("zip", Kind::Archive),
    ("zips", Kind::Archive),
    ("carpeta", Kind::Folder),
    ("carpetas", Kind::Folder),
    ("folder", Kind::Folder),
    ("folders", Kind::Folder),
];

/// What kind a file is, by its ending (a folder by being one).
pub(crate) fn kind_of(name: &str, is_dir: bool) -> Option<Kind> {
    if is_dir {
        return Some(Kind::Folder);
    }
    let ext = name.rsplit_once('.').map(|(_, e)| e.to_ascii_lowercase())?;
    Some(match ext.as_str() {
        "pdf" => Kind::Pdf,
        "jpg" | "jpeg" | "png" | "gif" | "webp" | "heic" | "heif" | "avif" | "bmp" | "tif" | "tiff" | "svg" | "jxl" | "raw"
        | "cr2" | "cr3" | "nef" | "arw" | "dng" | "orf" | "rw2" => Kind::Picture,
        "mp4" | "mkv" | "webm" | "mov" | "avi" | "m4v" | "wmv" | "flv" | "mpg" | "mpeg" | "3gp" => Kind::Video,
        "mp3" | "flac" | "ogg" | "opus" | "m4a" | "wav" | "aac" | "wma" | "alac" | "aiff" => Kind::Music,
        "doc" | "docx" | "odt" | "rtf" | "txt" | "md" | "xls" | "xlsx" | "ods" | "csv" | "ppt" | "pptx" | "odp" | "epub" => {
            Kind::Document
        }
        "zip" | "tar" | "gz" | "tgz" | "xz" | "zst" | "bz2" | "7z" | "rar" => Kind::Archive,
        _ => return None,
    })
}

/// Whether a file of kind `has` is one `asked` for (a PDF is a document too).
fn is_kind(has: Option<Kind>, asked: Kind) -> bool {
    has == Some(asked) || (asked == Kind::Document && has == Some(Kind::Pdf))
}

/// Text as searched: lower case, without accents (ñ is n: "espana" finds
/// "España").
pub(crate) fn fold(s: &str) -> String {
    s.chars()
        .flat_map(char::to_lowercase)
        .map(|c| match c {
            'á' | 'à' | 'ä' | 'â' | 'ã' | 'å' => 'a',
            'é' | 'è' | 'ë' | 'ê' => 'e',
            'í' | 'ì' | 'ï' | 'î' => 'i',
            'ó' | 'ò' | 'ö' | 'ô' | 'õ' => 'o',
            'ú' | 'ù' | 'ü' | 'û' => 'u',
            'ñ' => 'n',
            'ç' => 'c',
            c => c,
        })
        .collect()
}

/// What a query asks of the files: the words to find (as typed) and a kind
/// — and, a kind asked, every word as typed, the kind's too: a file may be
/// NAMED by it ("Documentos", "video tutorial notes.txt") —, when (`When`)
/// and how big (bytes, from..to).
#[derive(Debug, Default, PartialEq)]
pub(crate) struct Query {
    pub words: Vec<String>,
    pub kind: Option<Kind>,
    pub all: Vec<String>,
    pub when: Option<When>,
    pub size: Option<(u64, u64)>,
}

impl Query {
    /// Whether it asks for anything at all.
    pub(crate) fn asks(&self) -> bool {
        !self.words.is_empty() || self.kind.is_some() || self.when.is_some() || self.size.is_some()
    }
}

/// A time a query asks for, in days back from today's start (local): from
/// `from` days ago (0 = today) up to `to` days ago (`None`: up to now).
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct When {
    pub from: u32,
    pub to: Option<u32>,
}

impl When {
    /// The unix seconds it spans at `now`: today begins at local midnight.
    pub(crate) fn span(&self, now: f64) -> (f64, f64) {
        let midnight = local_midnight(now);
        let day = 86_400.0;
        let start = midnight - f64::from(self.from) * day;
        let end = self.to.map_or(f64::INFINITY, |t| midnight - f64::from(t) * day + day);
        (start, end)
    }
}

/// Today's start, here (unix seconds).
pub(crate) fn local_midnight(now: f64) -> f64 {
    // SAFETY: `localtime_r` writes only into the `tm` we give it.
    let off = unsafe {
        let t = now as libc::time_t;
        let mut tm: libc::tm = std::mem::zeroed();
        if libc::localtime_r(&t, &mut tm).is_null() { 0 } else { tm.tm_gmtoff as i64 }
    } as f64;
    ((now + off) / 86_400.0).floor() * 86_400.0 - off
}

/// The words that say when, folded: one word, or two read together.
fn when_of(words: &[String], i: usize) -> Option<(When, usize)> {
    let w = words[i].as_str();
    let next = words.get(i + 1).map(String::as_str);
    let one = |from, to| Some((When { from, to }, 1));
    let two = |from, to| Some((When { from, to }, 2));
    match (w, next) {
        ("esta" | "this", Some("semana" | "week")) => two(6, None),
        ("este" | "this", Some("mes" | "month")) => two(30, None),
        ("este" | "this", Some("ano" | "year")) => two(365, None),
        ("semana" | "week", Some("pasada")) => two(13, Some(7)),
        ("last", Some("week")) => two(13, Some(7)),
        ("mes" | "month", Some("pasado")) => two(60, Some(30)),
        ("last", Some("month")) => two(60, Some(30)),
        ("hoy" | "today", _) => one(0, None),
        ("ayer" | "yesterday", _) => one(1, Some(1)),
        ("reciente" | "recientes" | "recent", _) => one(6, None),
        _ => None,
    }
}

/// A size a word asks for: ">10mb", "<500kb", "big" (bytes, from..to).
fn size_of(w: &str) -> Option<(u64, u64)> {
    if matches!(w, "grande" | "grandes" | "big" | "large" | "pesado" | "pesados") {
        return Some((100 << 20, u64::MAX));
    }
    let (more, rest) = match w.as_bytes().first()? {
        b'>' => (true, &w[1..]),
        b'<' => (false, &w[1..]),
        _ => return None,
    };
    let at = rest.find(|c: char| !c.is_ascii_digit() && c != '.').unwrap_or(rest.len());
    let n: f64 = rest[..at].parse().ok()?;
    let unit = match &rest[at..] {
        "" | "b" => 1.0,
        "k" | "kb" => 1024.0,
        "m" | "mb" => 1024.0 * 1024.0,
        "g" | "gb" => 1024.0 * 1024.0 * 1024.0,
        _ => return None,
    };
    let bytes = (n * unit) as u64;
    Some(if more { (bytes, u64::MAX) } else { (0, bytes) })
}

/// A query read: its first kind word (when it has more than one word, or is
/// one) asks for that kind and is not a word to find; so with the words that
/// say when or how big.
pub(crate) fn parse(query: &str) -> Query {
    let mut q = Query::default();
    let typed: Vec<&str> = query.split_whitespace().collect();
    let folded: Vec<String> = typed.iter().map(|w| fold(w)).collect();
    let mut i = 0;
    while i < typed.len() {
        if q.when.is_none() {
            if let Some((when, n)) = when_of(&folded, i) {
                q.when = Some(when);
                i += n;
                continue;
            }
        }
        if q.size.is_none() {
            if let Some(size) = size_of(&folded[i]) {
                q.size = Some(size);
                i += 1;
                continue;
            }
        }
        let w = typed[i];
        let kind = KIND_WORDS.iter().find(|(k, _)| *k == folded[i]).map(|&(_, k)| k);
        match kind {
            Some(k) if q.kind.is_none() => q.kind = Some(k),
            _ => q.words.push(w.to_owned()),
        }
        q.all.push(w.to_owned());
        i += 1;
    }
    if q.kind.is_none() {
        q.all.clear();
    }
    q
}

/// What else narrows a search, beside the query: a kind chosen (the deep
/// search's chips, `deep_search.rs`) and a folder it keeps to.
#[derive(Debug, Default, Clone, PartialEq)]
pub(crate) struct Narrow {
    pub kind: Option<Kind>,
    pub scope: Option<PathBuf>,
}

/// Whether `a` and `b` are one letter apart — missing, one too many,
/// changed, or two side by side swapped — (two from eight letters on).
pub(crate) fn near(a: &[char], b: &[char]) -> bool {
    let max = if a.len() >= 8 { 2 } else { 1 };
    if a.len().abs_diff(b.len()) > max {
        return false;
    }
    // Damerau–Levenshtein (optimal string alignment), three rows.
    let (n, m) = (a.len(), b.len());
    let mut prev2 = vec![0usize; m + 1];
    let mut prev: Vec<usize> = (0..=m).collect();
    let mut cur = vec![0usize; m + 1];
    for i in 1..=n {
        cur[0] = i;
        let mut best = cur[0];
        for j in 1..=m {
            let cost = usize::from(a[i - 1] != b[j - 1]);
            let mut d = (prev[j] + 1).min(cur[j - 1] + 1).min(prev[j - 1] + cost);
            if i > 1 && j > 1 && a[i - 1] == b[j - 2] && a[i - 2] == b[j - 1] {
                d = d.min(prev2[j - 2] + 1);
            }
            cur[j] = d;
            best = best.min(d);
        }
        if best > max {
            return false;
        }
        std::mem::swap(&mut prev2, &mut prev);
        std::mem::swap(&mut prev, &mut cur);
    }
    prev[m] <= max
}

/// Whether `word` (folded) begins one of the words of `hay` (folded): at
/// its start, or after a separator ("fact" in "documents/facturas").
fn word_start(hay: &str, word: &str) -> bool {
    if word.is_empty() {
        return false;
    }
    let mut from = 0;
    while let Some(i) = hay[from..].find(word) {
        let at = from + i;
        let before = hay[..at].chars().next_back();
        if before.is_none_or(|c| !c.is_alphanumeric()) {
            return true;
        }
        from = at + hay[at..].chars().next().map_or(1, char::len_utf8);
    }
    false
}

/// When each file was opened last (its path → unix seconds).
pub(crate) type Opened = HashMap<String, f64>;

/// A word found in a folder the file is in, not in its name: worth this
/// (a name's word is worth what `nucleo` scores it, some 30 to 150).
const PATH_WORD: f64 = 28.0;
/// A name that says EVERY word is worth this much more.
const NAME_ALL: f64 = 1.5;
/// How much opening a file lately, and its changing lately, add (times
/// their freshness, 1 now, a half two days on).
const OPENED: f64 = 1.0;
const FRESH: f64 = 0.6;

/// Reusable atoms and buffer for one query (made once a keystroke).
pub(crate) struct Ranker {
    matcher: Matcher,
}

impl Default for Ranker {
    fn default() -> Self {
        Self { matcher: Matcher::new(nucleo_matcher::Config::DEFAULT) }
    }
}

impl Ranker {
    /// The files of `index` that `q` finds, best first, at most `max` (their
    /// places in `index`): a kind asked for keeps to it; the words as the
    /// module says; then the owner's use (`opened`, and when each changed),
    /// at equal worth the newer first.
    pub(crate) fn rank(&mut self, index: &[FileEntry], q: &Query, opened: &Opened, now: f64, max: usize) -> Vec<usize> {
        self.rank_narrow(index, q, &Narrow::default(), opened, now, max)
    }

    /// [`Ranker::rank`], narrowed: a kind chosen stands for the query's (whose
    /// word is then one to find, as any), a scope keeps to what is under it.
    /// Nothing found with every word: the same with a typo forgiven.
    pub(crate) fn rank_narrow(&mut self, index: &[FileEntry], q: &Query, narrow: &Narrow, opened: &Opened, now: f64, max: usize) -> Vec<usize> {
        let found = self.rank_strict(index, q, narrow, opened, now, max);
        if !found.is_empty() || q.words.is_empty() {
            return found;
        }
        self.rank_forgiving(index, q, narrow, opened, now, max)
    }

    fn rank_strict(&mut self, index: &[FileEntry], q: &Query, narrow: &Narrow, opened: &Opened, now: f64, max: usize) -> Vec<usize> {
        if !q.asks() && narrow.kind.is_none() && narrow.scope.is_none() {
            return Vec::new();
        }
        // A kind chosen: it is the kind; a kind word typed is then a word.
        let (kind, words_typed, all_typed): (Option<Kind>, &[String], &[String]) = match (narrow.kind, q.kind) {
            (Some(k), Some(_)) if q.kind != Some(k) => (Some(k), &q.all, &[]),
            (Some(k), _) => (Some(k), &q.words, &[]),
            (None, k) => (k, &q.words, &q.all),
        };
        let atoms_of = |words: &[String]| -> Vec<(Atom, String)> {
            words
                .iter()
                .map(|w| (Atom::new(w, CaseMatching::Ignore, Normalization::Smart, AtomKind::Substring, false), fold(w)))
                .collect()
        };
        let (words, all) = (atoms_of(words_typed), atoms_of(all_typed));
        let span = q.when.map(|w| w.span(now));
        let mut buf = Vec::new();
        let mut scored: Vec<(usize, f64, f64)> = Vec::new();
        for (i, f) in index.iter().enumerate() {
            if !self.keeps(f, q, narrow, span, opened) {
                continue;
            }
            // Two readings when a kind is asked: of that kind, with the other
            // words; or named by every word, the kind's too.
            let as_kind = kind
                .filter(|&k| is_kind(kind_of(&f.name, f.is_dir), k))
                .and_then(|_| if words.is_empty() { Some(1.0) } else { self.worth(f, &words, &mut buf) });
            let as_named = if all.is_empty() { None } else { self.worth(f, &all, &mut buf) };
            let worth = match (kind, as_kind, as_named) {
                // (Only when or how big, or a scope: everything there.)
                (None, _, _) if words.is_empty() => 1.0,
                (None, _, _) => match self.worth(f, &words, &mut buf) {
                    Some(w) => w,
                    None => continue,
                },
                (Some(_), a, n) => match a.into_iter().chain(n).reduce(f64::max) {
                    Some(w) => w,
                    None => continue,
                },
            };
            let used = opened.get(&f.path).map_or(0.0, |&t| crate::usage::decayed((now - t).max(0.0)));
            let fresh = if f.mtime > 0.0 { crate::usage::decayed((now - f.mtime).max(0.0)) } else { 0.0 };
            scored.push((i, worth * (1.0 + OPENED * used + FRESH * fresh), f.mtime));
        }
        scored.sort_by(|a, b| b.1.total_cmp(&a.1).then(b.2.total_cmp(&a.2)).then(a.0.cmp(&b.0)));
        scored.into_iter().take(max).map(|(i, _, _)| i).collect()
    }

    /// Whether `f` is within what is asked beside the words: the scope, the
    /// time (changed or opened then) and the size.
    fn keeps(&self, f: &FileEntry, q: &Query, narrow: &Narrow, span: Option<(f64, f64)>, opened: &Opened) -> bool {
        if narrow.scope.as_ref().is_some_and(|s| !Path::new(&f.path).starts_with(s) || Path::new(&f.path) == s.as_path()) {
            return false;
        }
        if let Some((from, to)) = span {
            let used = opened.get(&f.path).copied().unwrap_or(0.0);
            let at = |t: f64| t >= from && t < to;
            if !at(f.mtime) && !at(used) {
                return false;
            }
        }
        if let Some((lo, hi)) = q.size {
            if f.is_dir || f.size < lo || f.size > hi {
                return false;
            }
        }
        true
    }

    /// Nothing had every word: each word of four letters or more one letter
    /// off a word of the name (`near`), or its letters in order in it (one
    /// left out: "utlook"); shorter words as they were.
    fn rank_forgiving(&mut self, index: &[FileEntry], q: &Query, narrow: &Narrow, opened: &Opened, now: f64, max: usize) -> Vec<usize> {
        let words: Vec<(Vec<char>, Atom)> = q
            .words
            .iter()
            .map(|w| (fold(w).chars().collect(), Atom::new(w, CaseMatching::Ignore, Normalization::Smart, AtomKind::Fuzzy, false)))
            .collect();
        if words.iter().all(|(w, _)| w.len() < 4) {
            return Vec::new();
        }
        let kind = narrow.kind.or(q.kind);
        let span = q.when.map(|w| w.span(now));
        let mut buf = Vec::new();
        let mut scored: Vec<(usize, f64, f64)> = Vec::new();
        'files: for (i, f) in index.iter().enumerate() {
            if !self.keeps(f, q, narrow, span, opened) || kind.is_some_and(|k| !is_kind(kind_of(&f.name, f.is_dir), k)) {
                continue;
            }
            let name = fold(&f.name);
            let parts: Vec<Vec<char>> = name.split(|c: char| !c.is_alphanumeric()).filter(|p| !p.is_empty()).map(|p| p.chars().collect()).collect();
            let mut worth = 0.0;
            for (w, atom) in &words {
                let s: String = w.iter().collect();
                if name.contains(&s) {
                    worth += 2.0;
                } else if w.len() >= 4 && parts.iter().any(|p| near(w, p) || (p.len() > w.len() && near(w, &p[..w.len()]))) {
                    worth += 1.0;
                } else if w.len() >= 4 && atom.score(Utf32Str::new(&f.name, &mut buf), &mut self.matcher).is_some_and(|sc| f64::from(sc) >= 12.0 * w.len() as f64) {
                    worth += 0.5;
                } else {
                    continue 'files;
                }
            }
            let used = opened.get(&f.path).map_or(0.0, |&t| crate::usage::decayed((now - t).max(0.0)));
            scored.push((i, worth * (1.0 + OPENED * used), f.mtime));
        }
        scored.sort_by(|a, b| b.1.total_cmp(&a.1).then(b.2.total_cmp(&a.2)).then(a.0.cmp(&b.0)));
        scored.into_iter().take(max).map(|(i, _, _)| i).collect()
    }

    /// What file `f` is worth to `words` (each a piece of its name, or the
    /// start of a word of its folders' path): none when one is in neither.
    fn worth(&mut self, f: &FileEntry, words: &[(Atom, String)], buf: &mut Vec<char>) -> Option<f64> {
        let hay = Utf32Str::new(&f.name, buf);
        let (mut total, mut in_name) = (0.0, 0usize);
        for (atom, word) in words {
            if let Some(s) = atom.score(hay, &mut self.matcher) {
                total += f64::from(s);
                in_name += 1;
            } else if word_start(&f.dir, word) {
                total += PATH_WORD;
            } else {
                return None;
            }
        }
        // (One short word in no name: nothing — "a" is in every path.)
        if in_name == 0 && words.len() == 1 && words[0].1.chars().count() < 3 {
            return None;
        }
        Some(if in_name == words.len() { total * NAME_ALL } else { total })
    }
}

/// When each file of the system's record was used last (`recently-used.xbel`).
pub(crate) fn opened_from_record() -> Opened {
    let file = std::env::var_os("XDG_DATA_HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| crate::filer::home().join(".local/share"))
        .join("recently-used.xbel");
    let text = std::fs::read_to_string(file).unwrap_or_default();
    crate::filer::recent::entries(&text)
        .into_iter()
        .filter_map(|(p, when)| Some((p.to_string_lossy().into_owned(), crate::app_actions::iso_secs(&when)?)))
        .collect()
}

/// A file's folder below home, as searched (`fold`; "" at home's top).
pub(crate) fn dir_of(path: &Path, home: &Path) -> String {
    let parent = path.parent().unwrap_or(path);
    fold(&parent.strip_prefix(home).unwrap_or(parent).to_string_lossy())
}

/// When a file changed last (unix seconds; 0 unknown).
pub(crate) fn mtime_of(meta: &std::fs::Metadata) -> f64 {
    meta.modified()
        .ok()
        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
        .map_or(0.0, |d| d.as_secs_f64())
}

/// The owner's folders whose tops are watched: Desktop, Documents,
/// Downloads, Music, Pictures (and its Screenshots), Videos — those there.
pub(crate) fn watched_dirs() -> Vec<PathBuf> {
    let home = crate::filer::home();
    let conf = std::env::var_os("XDG_CONFIG_HOME").map_or_else(|| home.join(".config"), PathBuf::from);
    let text = std::fs::read_to_string(conf.join("user-dirs.dirs")).unwrap_or_default();
    let fallback = |key: &str| match key {
        "DESKTOP" => "Desktop",
        "DOCUMENTS" => "Documents",
        "DOWNLOAD" => "Downloads",
        "MUSIC" => "Music",
        "PICTURES" => "Pictures",
        _ => "Videos",
    };
    let mut out: Vec<PathBuf> = ["DESKTOP", "DOCUMENTS", "DOWNLOAD", "MUSIC", "PICTURES", "VIDEOS"]
        .iter()
        .map(|k| crate::filer::user_dir(&text, &home, k).unwrap_or_else(|| home.join(fallback(k))))
        .collect();
    if let Some(pics) = out.get(4).cloned() {
        out.push(pics.join("Screenshots"));
    }
    let mut seen = std::collections::HashSet::new();
    out.retain(|d| d.is_dir() && seen.insert(d.components().collect::<PathBuf>()));
    out
}

/// A watched folder's top as the index has it (no hidden, no links).
pub(crate) fn list_top(dir: &Path, home: &Path) -> Vec<FileEntry> {
    let Ok(read) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    let folder = dir_of(&dir.join("x"), home);
    read.flatten()
        .filter_map(|e| {
            let name = e.file_name().to_string_lossy().into_owned();
            let ft = e.file_type().ok()?;
            if ft.is_symlink() || crate::apps::walk_skips(&name, &e.path(), ft.is_dir()) {
                return None;
            }
            let meta = e.metadata().ok();
            let mtime = meta.as_ref().map_or(0.0, mtime_of);
            let size = meta.as_ref().filter(|_| !ft.is_dir()).map_or(0, std::fs::Metadata::len);
            Some(FileEntry { path: e.path().to_string_lossy().into_owned(), name, is_dir: ft.is_dir(), dir: folder.clone(), mtime, size })
        })
        .collect()
}

/// `index` with the tops of `dirs` as they are now (`tops`, listed off the
/// loop): what left is gone, what came is there; the rest as it was.
pub(crate) fn merge_tops(index: &mut Vec<FileEntry>, dirs: &[PathBuf], tops: Vec<FileEntry>) {
    // (As paths, not text: "Downloads/" is "Downloads".)
    index.retain(|f| !Path::new(&f.path).parent().is_some_and(|p| dirs.iter().any(|d| d.as_path() == p)));
    index.extend(tops);
}

#[cfg(test)]
mod tests {
    use super::*;

    fn file(path: &str, mtime: f64) -> FileEntry {
        let home = Path::new("/h");
        let p = Path::new(path);
        FileEntry {
            name: p.file_name().unwrap().to_string_lossy().into_owned(),
            path: path.to_owned(),
            is_dir: !path.contains('.'),
            dir: dir_of(p, home),
            mtime,
            size: 1000,
        }
    }

    fn names(index: &[FileEntry], got: Vec<usize>) -> Vec<String> {
        got.into_iter().map(|i| index[i].name.clone()).collect()
    }

    #[test]
    fn a_kind_word_asks_for_that_kind_and_is_not_searched() {
        let q = parse("pdf contrato");
        assert_eq!((q.words, q.kind), (vec!["contrato".to_owned()], Some(Kind::Pdf)));
        assert_eq!(parse("Fotos playa").kind, Some(Kind::Picture));
        let q = parse("música");
        assert_eq!((q.words.len(), q.kind, q.all), (0, Some(Kind::Music), vec!["música".to_owned()]));
        assert_eq!(parse("factura junio"), Query { words: vec!["factura".into(), "junio".into()], ..Default::default() });
        assert_eq!(kind_of("a.JPG", false), Some(Kind::Picture));
        assert_eq!(kind_of("x", true), Some(Kind::Folder));
        assert!(is_kind(Some(Kind::Pdf), Kind::Document));
    }

    #[test]
    fn words_say_when_and_how_big_and_are_not_searched() {
        let q = parse("fotos esta semana");
        assert_eq!((q.kind, q.when, q.words.len()), (Some(Kind::Picture), Some(When { from: 6, to: None }), 0));
        let q = parse("factura ayer");
        assert_eq!((q.words, q.when), (vec!["factura".to_owned()], Some(When { from: 1, to: Some(1) })));
        assert_eq!(parse("semana pasada").when, Some(When { from: 13, to: Some(7) }));
        assert_eq!(parse("videos >1gb").size, Some((1 << 30, u64::MAX)));
        assert_eq!(parse("<500kb notas").size, Some((0, 500 * 1024)));
        assert_eq!(parse("grandes").size, Some((100 << 20, u64::MAX)));
        // (A word that only looks so is a word.)
        assert_eq!(parse(">abc").words, vec![">abc".to_owned()]);
        // Yesterday spans one day ending where today begins.
        let now = 1_800_000_000.0;
        let (from, to) = When { from: 1, to: Some(1) }.span(now);
        assert!((to - from - 86_400.0).abs() < 1.0 && to <= now && now - to < 86_400.0);
    }

    #[test]
    fn when_and_size_keep_to_what_they_say_and_alone_list_it() {
        let now = 1_800_000_000.0;
        let mut index = vec![file("/h/Downloads/new.pdf", now - 100.0), file("/h/Downloads/old.pdf", now - 90.0 * 86_400.0)];
        index[1].size = 900 << 20;
        let mut r = Ranker::default();
        let opened = Opened::new();
        assert_eq!(names(&index, r.rank(&index, &parse("hoy"), &opened, now, 9)), vec!["new.pdf"]);
        assert_eq!(names(&index, r.rank(&index, &parse("pdf grandes"), &opened, now, 9)), vec!["old.pdf"]);
        // Opened today counts as today.
        let opened: Opened = [("/h/Downloads/old.pdf".to_owned(), now - 60.0)].into();
        let got = names(&index, r.rank(&index, &parse("pdf hoy"), &opened, now, 9));
        assert!(got.contains(&"old.pdf".to_owned()) && got.len() == 2);
    }

    #[test]
    fn a_typo_still_finds_when_nothing_has_every_word() {
        let index = vec![file("/h/Documents/factura junio.pdf", 0.0), file("/h/Documents/presupuesto.xlsx", 0.0), file("/h/x/Outlook notes.txt", 0.0)];
        let mut r = Ranker::default();
        let o = Opened::new();
        assert_eq!(names(&index, r.rank(&index, &parse("factrua"), &o, 0.0, 9)), vec!["factura junio.pdf"]);
        assert_eq!(names(&index, r.rank(&index, &parse("presupusto"), &o, 0.0, 9)), vec!["presupuesto.xlsx"]);
        assert_eq!(names(&index, r.rank(&index, &parse("utlook"), &o, 0.0, 9)), vec!["Outlook notes.txt"]);
        // (Typing on: a word begun is forgiven against the word's start.)
        assert_eq!(names(&index, r.rank(&index, &parse("factr junio"), &o, 0.0, 9)), vec!["factura junio.pdf"]);
        assert_eq!(names(&index, r.rank(&index, &parse("facturq"), &o, 0.0, 9)), vec!["factura junio.pdf"]);
        // Short words are not forgiven.
        assert!(r.rank(&index, &parse("fcx"), &o, 0.0, 9).is_empty());
        assert!(r.rank(&index, &parse("zzzzzz"), &o, 0.0, 9).is_empty());
        assert!(near(&"factura".chars().collect::<Vec<_>>(), &"factrua".chars().collect::<Vec<_>>()));
        assert!(!near(&"casa".chars().collect::<Vec<_>>(), &"pera".chars().collect::<Vec<_>>()));
    }

    #[test]
    fn a_chosen_kind_and_a_scope_narrow_the_search() {
        let index = vec![
            file("/h/Pictures/playa.jpg", 0.0),
            file("/h/Documents/playa notes.txt", 0.0),
            file("/h/Downloads/playa.mp4", 0.0),
        ];
        let mut r = Ranker::default();
        let o = Opened::new();
        let pics = Narrow { kind: Some(Kind::Picture), scope: None };
        assert_eq!(names(&index, r.rank_narrow(&index, &parse("playa"), &pics, &o, 0.0, 9)), vec!["playa.jpg"]);
        // A chosen kind with nothing typed lists that kind.
        assert_eq!(names(&index, r.rank_narrow(&index, &parse(""), &pics, &o, 0.0, 9)), vec!["playa.jpg"]);
        let docs = Narrow { kind: None, scope: Some(PathBuf::from("/h/Documents")) };
        assert_eq!(names(&index, r.rank_narrow(&index, &parse("playa"), &docs, &o, 0.0, 9)), vec!["playa notes.txt"]);
        assert_eq!(names(&index, r.rank_narrow(&index, &parse(""), &docs, &o, 0.0, 9)), vec!["playa notes.txt"]);
    }

    #[test]
    fn words_are_found_in_the_name_or_from_a_word_start_in_the_folders() {
        assert!(word_start("pictures/playa 2024", "playa"));
        assert!(word_start("documents/facturas", "fact"));
        assert!(!word_start("documents/facturas", "actu"));
        assert_eq!(fold("Música/España"), "musica/espana");
        let index = vec![
            file("/h/Pictures/Playa 2024/IMG_0412.jpg", 0.0),
            file("/h/Pictures/Playa 2024", 0.0),
            file("/h/Pictures/montana/IMG_0001.jpg", 0.0),
            file("/h/Documents/Facturas/junio.pdf", 0.0),
            file("/h/Documents/Facturas/julio.pdf", 0.0),
        ];
        let mut r = Ranker::default();
        let none = Opened::new();
        let got = names(&index, r.rank(&index, &parse("playa 2024"), &none, 0.0, 10));
        assert_eq!(got.first().map(String::as_str), Some("Playa 2024"), "the folder by its name first: {got:?}");
        assert!(got.contains(&"IMG_0412.jpg".to_owned()) && !got.contains(&"IMG_0001.jpg".to_owned()), "{got:?}");
        let got = names(&index, r.rank(&index, &parse("factura junio"), &none, 0.0, 10));
        assert_eq!(got, vec!["junio.pdf".to_owned()]);
        // One word alone brings what is in a folder of that name, below what
        // it names; a short one does not.
        let got = names(&index, r.rank(&index, &parse("facturas"), &none, 0.0, 10));
        assert_eq!(got.len(), 2, "{got:?}");
        let got = names(&index, r.rank(&index, &parse("pla"), &none, 0.0, 10));
        assert_eq!(got.first().map(String::as_str), Some("Playa 2024"), "{got:?}");
        assert!(names(&index, r.rank(&index, &parse("fa"), &none, 0.0, 10)).is_empty());
        let got = names(&index, r.rank(&index, &parse("pdf"), &none, 0.0, 10));
        assert_eq!(got.len(), 2);
    }

    #[test]
    fn a_kind_word_still_finds_what_it_names() {
        let index = vec![
            file("/h/Documentos", 0.0),
            file("/h/Documentos/carta.odt", 5.0),
            file("/h/Videos", 0.0),
            file("/h/notes/video tutorial notes.txt", 0.0),
            file("/h/Videos/clase.mp4", 3.0),
        ];
        let mut r = Ranker::default();
        let none = Opened::new();
        let got = names(&index, r.rank(&index, &parse("Documentos"), &none, 0.0, 10));
        assert_eq!(got.first().map(String::as_str), Some("Documentos"), "the folder of that name first: {got:?}");
        assert!(got.contains(&"carta.odt".to_owned()), "{got:?}");
        let got = names(&index, r.rank(&index, &parse("video tutorial"), &none, 0.0, 10));
        assert_eq!(got, vec!["video tutorial notes.txt".to_owned()]);
    }

    #[test]
    fn what_was_opened_or_changed_lately_comes_first() {
        let now = 1_000_000.0;
        let index = vec![
            file("/h/a/notas viejas.txt", now - 400.0 * 86_400.0),
            file("/h/b/notas.txt", now - 300.0 * 86_400.0),
            file("/h/Downloads/notas-2.txt", now - 600.0),
        ];
        let mut r = Ranker::default();
        let mut opened = Opened::new();
        let got = names(&index, r.rank(&index, &parse("notas"), &opened, now, 10));
        assert_eq!(got[0], "notas-2.txt", "the download of a moment ago: {got:?}");
        opened.insert("/h/a/notas viejas.txt".into(), now - 60.0);
        let got = names(&index, r.rank(&index, &parse("notas"), &opened, now, 10));
        assert_eq!(got[0], "notas viejas.txt", "opened a minute ago: {got:?}");
        // A kind alone: the newest first.
        let got = names(&index, r.rank(&index, &parse("documentos"), &Opened::new(), now, 10));
        assert_eq!(got, vec!["notas-2.txt", "notas.txt", "notas viejas.txt"]);
    }

    /// The owner's real home, read only: how long the walk and a keystroke
    /// take, and what a few queries find. `cargo test -- --ignored
    /// real_home_search --nocapture`.
    #[test]
    #[ignore]
    fn real_home_search() {
        let t = std::time::Instant::now();
        let index = crate::apps::scan_home_files();
        println!("walk: {} files in {:?}", index.len(), t.elapsed());
        let opened = opened_from_record();
        println!("record: {} opened", opened.len());
        let now = crate::usage::now_secs();
        let mut r = Ranker::default();
        let queries = std::env::var("QUERIES").unwrap_or_else(|_| "a,pdf,golem,golem iso,fotos,videos,notas,screenshot,descargas,factura junio".into());
        for q in queries.split(',') {
            let t = std::time::Instant::now();
            let got = r.rank(&index, &parse(q), &opened, now, 8);
            let took = t.elapsed();
            let shown: Vec<String> = got.iter().map(|&i| index[i].path.trim_start_matches(&*crate::filer::home().to_string_lossy()).to_owned()).collect();
            println!("{q:>14} {took:>10.2?}  {shown:?}");
        }
    }

    #[test]
    fn a_watched_top_is_replaced_and_the_rest_kept() {
        let mut index = vec![file("/h/Downloads/old.pdf", 0.0), file("/h/Downloads/sub/deep.pdf", 0.0), file("/h/x.txt", 0.0)];
        // (A folder written with its slash is that folder.)
        merge_tops(&mut index, &[PathBuf::from("/h/Downloads/")], vec![file("/h/Downloads/new.pdf", 5.0)]);
        let got: Vec<&str> = index.iter().map(|f| f.name.as_str()).collect();
        assert_eq!(got, vec!["deep.pdf", "x.txt", "new.pdf"]);
    }
}
