//! INSIDE FILES — the deep search's words found in what the owner's files
//! SAY, not only in their names (Max, 2026-10-27, after Windows' index,
//! Raycast's content matches, Alfred's "in" and GNOME Files' full text).
//! Local, as everything here: no index service, two programs the system has.
//!
//! - TEXT FILES (`TEXT_ENDINGS`: notes, Markdown, tables of values, code) are
//!   read where they are, by `rg`, in the owner's Desktop, Documents and
//!   Downloads.
//! - PAPERS — PDFs and office files (`PAPER_ENDINGS`) — are read ONCE into
//!   plain text in the cache (`refresh`: `pdftotext` its first pages,
//!   `bsdtar` the document's XML out of an office file), a file a paper, kept
//!   while it does not change (`index.tsv`: its key, when it changed, its
//!   path), off the loop at the lowest priority; `rg` reads them there.
//! - A word is found WITHOUT ITS ACCENTS and its case ("musica" finds
//!   "Música": `pattern`); every word must be in the file (`find`: the
//!   longest first, the rest only in what it found).

use std::collections::HashMap;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use tracing::{info, warn};

/// Files read as they are.
pub(crate) const TEXT_ENDINGS: &[&str] = &[
    "txt", "md", "markdown", "csv", "tsv", "json", "xml", "html", "htm", "rs", "py", "js", "ts", "c", "h", "cpp", "go", "java", "sh",
    "toml", "yaml", "yml", "ini", "conf", "log", "tex", "org", "rst",
];

/// Files read once into the cache (`extract`).
pub(crate) const PAPER_ENDINGS: &[&str] = &["pdf", "docx", "odt", "pptx", "odp", "xlsx", "ods"];

/// A paper bigger than this is not read.
const PAPER_MAX: u64 = 60 << 20;
/// The pages of a PDF read.
const PDF_PAGES: &str = "40";
/// The text kept of one paper.
const TEXT_MAX: usize = 400 << 10;
/// How long one program may take on one file.
const PATIENCE: Duration = Duration::from_secs(20);

/// Where the papers' text is kept.
pub(crate) fn cache_dir() -> PathBuf {
    std::env::var_os("XDG_CACHE_HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| crate::filer::home().join(".cache"))
        .join("waverunner/inside")
}

/// A file's ending, lower case.
pub(crate) fn ending(path: &str) -> String {
    Path::new(path).extension().map(|e| e.to_string_lossy().to_ascii_lowercase()).unwrap_or_default()
}

/// A paper's key in the cache: its path, hashed.
fn key_of(path: &str) -> String {
    use std::hash::{Hash, Hasher};
    let mut h = std::collections::hash_map::DefaultHasher::new();
    path.hash(&mut h);
    format!("{:016x}", h.finish())
}

/// The cache's list: key → (when the paper changed, its path).
fn read_manifest(dir: &Path) -> HashMap<String, (f64, String)> {
    let text = std::fs::read_to_string(dir.join("index.tsv")).unwrap_or_default();
    text.lines()
        .filter_map(|l| {
            let mut parts = l.splitn(3, '\t');
            let (key, mtime, path) = (parts.next()?, parts.next()?, parts.next()?);
            Some((key.to_owned(), (mtime.parse().ok()?, path.to_owned())))
        })
        .collect()
}

fn write_manifest(dir: &Path, list: &HashMap<String, (f64, String)>) {
    let mut text = String::new();
    for (key, (mtime, path)) in list {
        text.push_str(&format!("{key}\t{mtime}\t{path}\n"));
    }
    let tmp = dir.join("index.tsv.tmp");
    if std::fs::write(&tmp, text).is_ok() {
        let _ = std::fs::rename(&tmp, dir.join("index.tsv"));
    }
}

/// BRING THE CACHE UP TO THE PAPERS (their path, when each changed, its
/// size): what is new or changed is read, what is gone let go. Slow the first
/// time (a few minutes for thousands of PDFs) — off the loop, and this thread
/// at the lowest priority, the programs it starts with it. Gives how many it
/// read.
pub(crate) fn refresh(papers: Vec<(String, f64, u64)>) -> usize {
    // SAFETY: setpriority on the calling thread (`who` 0) changes nothing else.
    unsafe {
        libc::setpriority(libc::PRIO_PROCESS, 0, 19);
    }
    let dir = cache_dir();
    if std::fs::create_dir_all(&dir).is_err() {
        return 0;
    }
    let started = Instant::now();
    let mut list = read_manifest(&dir);
    let wanted: HashMap<String, (f64, String)> =
        papers.iter().filter(|(_, _, size)| *size <= PAPER_MAX).map(|(p, m, _)| (key_of(p), (*m, p.clone()))).collect();
    // What is gone.
    list.retain(|key, _| {
        let keep = wanted.contains_key(key);
        if !keep {
            let _ = std::fs::remove_file(dir.join(format!("{key}.txt")));
        }
        keep
    });
    let mut read = 0;
    for (key, (mtime, path)) in &wanted {
        if list.get(key).is_some_and(|(m, _)| (m - mtime).abs() < 0.5) {
            continue;
        }
        // (Not read — its program missing, or too slow —: tried again next time.)
        let Some(text) = extract(path) else {
            continue;
        };
        let tmp = dir.join(format!("{key}.tmp"));
        if std::fs::write(&tmp, &text).is_ok() && std::fs::rename(&tmp, dir.join(format!("{key}.txt"))).is_ok() {
            list.insert(key.clone(), (*mtime, path.clone()));
            read += 1;
        }
        // (Kept as it goes: a dock stopped half way keeps what it read.)
        if read % 50 == 0 && read > 0 {
            write_manifest(&dir, &list);
        }
    }
    write_manifest(&dir, &list);
    if read > 0 {
        info!("inside files: read {read} papers in {:.1} s ({} kept)", started.elapsed().as_secs_f32(), list.len());
    }
    read
}

/// A paper's words: a PDF's first pages, an office file's document XML.
fn extract(path: &str) -> Option<String> {
    let text = match ending(path).as_str() {
        "pdf" => String::from_utf8_lossy(&run_for(Command::new("pdftotext").args(["-q", "-l", PDF_PAGES, "-enc", "UTF-8", path, "-"]))?)
            .into_owned(),
        e => {
            let inside: &[&str] = match e {
                "docx" => &["word/document.xml"],
                "xlsx" => &["xl/sharedStrings.xml"],
                "pptx" => &["ppt/slides/slide*.xml"],
                _ => &["content.xml"],
            };
            let xml = run_for(Command::new("bsdtar").arg("-xOf").arg(path).args(inside))?;
            strip_xml(&String::from_utf8_lossy(&xml))
        }
    };
    let mut text = text;
    if text.len() > TEXT_MAX {
        let mut cut = TEXT_MAX;
        while !text.is_char_boundary(cut) {
            cut -= 1;
        }
        text.truncate(cut);
    }
    Some(text)
}

/// What a program writes, given at most `PATIENCE` (then it is ended).
fn run_for(cmd: &mut Command) -> Option<Vec<u8>> {
    let mut child = cmd.stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::null()).spawn().ok()?;
    let mut out = child.stdout.take()?;
    let reader = std::thread::spawn(move || {
        let mut buf = Vec::new();
        let _ = out.read_to_end(&mut buf);
        buf
    });
    let until = Instant::now() + PATIENCE;
    loop {
        match child.try_wait() {
            Ok(Some(_)) => break,
            Ok(None) if Instant::now() < until => std::thread::sleep(Duration::from_millis(30)),
            _ => {
                let _ = child.kill();
                let _ = child.wait();
                break;
            }
        }
    }
    reader.join().ok()
}

/// An office file's XML as words: tags out, entities back, spaces between.
fn strip_xml(xml: &str) -> String {
    let mut out = String::with_capacity(xml.len() / 3);
    let mut in_tag = false;
    for c in xml.chars() {
        match c {
            '<' => {
                in_tag = true;
                if !out.ends_with(' ') {
                    out.push(' ');
                }
            }
            '>' => in_tag = false,
            c if !in_tag => out.push(c),
            _ => {}
        }
    }
    out.replace("&amp;", "&").replace("&lt;", "<").replace("&gt;", ">").replace("&quot;", "\"").replace("&apos;", "'")
}

/// A word as `rg` looks for it: each vowel (and n, c) with its accents, the
/// rest as itself.
pub(crate) fn pattern(word: &str) -> String {
    crate::file_search::fold(word)
        .chars()
        .map(|c| match c {
            'a' => "[aáàäâã]".to_owned(),
            'e' => "[eéèëê]".to_owned(),
            'i' => "[iíìïî]".to_owned(),
            'o' => "[oóòöôõ]".to_owned(),
            'u' => "[uúùüû]".to_owned(),
            'n' => "[nñ]".to_owned(),
            'c' => "[cç]".to_owned(),
            c if "\\.+*?()|[]{}^$#&-~".contains(c) => format!("\\{c}"),
            c => c.to_string(),
        })
        .collect()
}

/// THE FILES WHOSE TEXT SAYS EVERY WORD (their paths, at most `max`): the
/// text files in `roots` and the papers' text in the cache. None when `rg`
/// is not there.
pub(crate) fn find(words: &[String], roots: &[PathBuf], cache: &Path, max: usize) -> Vec<String> {
    let mut words: Vec<&String> = words.iter().filter(|w| !w.is_empty()).collect();
    if words.is_empty() {
        return Vec::new();
    }
    // (The longest first: it finds the fewest.)
    words.sort_by_key(|w| std::cmp::Reverse(w.chars().count()));
    let dir = cache.to_path_buf();
    let mut first = Command::new("rg");
    first.args(["-l", "-i", "--no-messages", "--max-filesize", "8M", "--max-depth", "8"]);
    for e in TEXT_ENDINGS {
        first.arg("-g").arg(format!("*.{e}"));
    }
    first.arg("-e").arg(pattern(words[0]));
    let mut any = false;
    for r in roots.iter().chain([&dir]) {
        if r.is_dir() {
            first.arg(r);
            any = true;
        }
    }
    if !any {
        return Vec::new();
    }
    let Some(out) = run_for(&mut first) else {
        warn!("inside files: rg did not run");
        return Vec::new();
    };
    let mut found: Vec<String> = String::from_utf8_lossy(&out).lines().map(str::to_owned).collect();
    for w in &words[1..] {
        if found.is_empty() {
            break;
        }
        found.truncate(2000);
        let mut next = Command::new("rg");
        next.args(["-l", "-i", "--no-messages", "-e"]).arg(pattern(w)).arg("--").args(&found);
        found = run_for(&mut next).map(|o| String::from_utf8_lossy(&o).lines().map(str::to_owned).collect()).unwrap_or_default();
    }
    // The papers' text, as the papers.
    let list = read_manifest(&dir);
    let mut out: Vec<String> = Vec::new();
    for f in found {
        let path = match Path::new(&f).strip_prefix(&dir) {
            Ok(rel) => {
                let key = rel.to_string_lossy().trim_end_matches(".txt").to_owned();
                match list.get(&key) {
                    Some((_, p)) => p.clone(),
                    None => continue,
                }
            }
            Err(_) => f,
        };
        if !out.contains(&path) {
            out.push(path);
        }
        if out.len() >= max {
            break;
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_word_is_looked_for_with_its_accents_and_safely() {
        assert_eq!(pattern("Música"), "m[uúùüû]s[iíìïî][cç][aáàäâã]");
        assert_eq!(pattern("a.b"), "[aáàäâã]\\.b");
    }

    #[test]
    fn an_office_file_reads_as_its_words() {
        let xml = "<w:p><w:t>Factura</w:t></w:p><w:p><w:t>junio &amp; julio</w:t></w:p>";
        let text = strip_xml(xml);
        assert!(text.contains("Factura") && text.contains("junio & julio"), "{text}");
    }

    #[test]
    fn text_files_and_papers_are_found_by_every_word() {
        if Command::new("rg").arg("--version").output().is_err() {
            return;
        }
        let root = std::env::temp_dir().join(format!("inside-test-{}", std::process::id()));
        let docs = root.join("Documents");
        std::fs::create_dir_all(&docs).unwrap();
        std::fs::write(docs.join("notas.txt"), "La factura de JUNIO llegó tarde.\n").unwrap();
        std::fs::write(docs.join("otra.md"), "Nada que ver.\n").unwrap();
        let words = |w: &[&str]| w.iter().map(|s| s.to_string()).collect::<Vec<_>>();
        let cache = root.join("cache");
        let got = find(&words(&["junio", "llego"]), std::slice::from_ref(&docs), &cache, 10);
        assert_eq!(got.len(), 1, "{got:?}");
        assert!(got[0].ends_with("notas.txt"));
        assert!(find(&words(&["junio", "nada"]), std::slice::from_ref(&docs), &cache, 10).is_empty());
        let _ = std::fs::remove_dir_all(&root);
    }
}
