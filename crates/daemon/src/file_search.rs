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
/// NAMED by it ("Documentos", "video tutorial notes.txt").
#[derive(Debug, Default, PartialEq)]
pub(crate) struct Query {
    pub words: Vec<String>,
    pub kind: Option<Kind>,
    pub all: Vec<String>,
}

/// A query read: its first kind word (when it has more than one word, or is
/// one) asks for that kind and is not a word to find.
pub(crate) fn parse(query: &str) -> Query {
    let mut q = Query::default();
    for w in query.split_whitespace() {
        let folded = fold(w);
        let kind = KIND_WORDS.iter().find(|(k, _)| *k == folded).map(|&(_, k)| k);
        match kind {
            Some(k) if q.kind.is_none() => q.kind = Some(k),
            _ => q.words.push(w.to_owned()),
        }
        q.all.push(w.to_owned());
    }
    if q.kind.is_none() {
        q.all.clear();
    }
    q
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
        if q.words.is_empty() && q.kind.is_none() {
            return Vec::new();
        }
        let atoms_of = |words: &[String]| -> Vec<(Atom, String)> {
            words
                .iter()
                .map(|w| (Atom::new(w, CaseMatching::Ignore, Normalization::Smart, AtomKind::Substring, false), fold(w)))
                .collect()
        };
        let (words, all) = (atoms_of(&q.words), atoms_of(&q.all));
        let mut buf = Vec::new();
        let mut scored: Vec<(usize, f64, f64)> = Vec::new();
        for (i, f) in index.iter().enumerate() {
            // Two readings when a kind is asked: of that kind, with the other
            // words; or named by every word, the kind's too.
            let as_kind = q
                .kind
                .filter(|&k| is_kind(kind_of(&f.name, f.is_dir), k))
                .and_then(|_| if words.is_empty() { Some(1.0) } else { self.worth(f, &words, &mut buf) });
            let as_named = if all.is_empty() { None } else { self.worth(f, &all, &mut buf) };
            let worth = match (q.kind, as_kind, as_named) {
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
            let mtime = e.metadata().map_or(0.0, |m| mtime_of(&m));
            Some(FileEntry { path: e.path().to_string_lossy().into_owned(), name, is_dir: ft.is_dir(), dir: folder.clone(), mtime })
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
        assert_eq!(parse("factura junio"), Query { words: vec!["factura".into(), "junio".into()], kind: None, all: vec![] });
        assert_eq!(kind_of("a.JPG", false), Some(Kind::Picture));
        assert_eq!(kind_of("x", true), Some(Kind::Folder));
        assert!(is_kind(Some(Kind::Pdf), Kind::Document));
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
