//! Shell-string i18n: every user-visible string the daemon renders or sends
//! as a notification goes through [`tr`], gettext-style — the English literal
//! at the call site is both the displayed fallback and the lookup key. That
//! makes the catalogue extractable with no build step:
//!
//! ```sh
//! grep -rhoE 'i18n::tr\("[^"]+"\)' crates/daemon/src | sort -u
//! ```
//!
//! One family of keys escapes that grep: the month abbreviations
//! ("Jan".."Dec"), which reach [`tr`] through `clipboard::fmt_datetime`'s
//! `MON` array rather than as literal arguments.
//!
//! Translations are data, not code, mirroring the dictionary (`dict.rs`):
//! a flat JSON object `{"English source": "translation"}` looked up from
//! `$WAVERUNNER_I18N` (a file path override) or `<data-dir>/i18n/<tag>.json`,
//! where `<tag>` is tried as the full locale ("es_BO") then its language
//! ("es"), taken from `LC_ALL` → `LC_MESSAGES` → `LANG`. English ships in the
//! literals, so no `en.json` exists; a missing file, a malformed file, or a
//! missing key all fall back to the English literal. Templates keep their
//! `{placeholder}` markers through translation and callers substitute with
//! `str::replace`, so word order is the translator's to choose.
//!
//! [`init`] runs once in `main` before any worker thread spawns; [`tr`] after
//! that is a lock-free read. Tests never call `init`, so they see English.

use std::collections::HashMap;
use std::sync::atomic::{AtomicPtr, Ordering};

use tracing::{info, warn};

/// The table in use. Swapped whole when the language changes; a replaced
/// table is leaked on purpose, because `tr` hands out `&'static str`s into
/// it (a language change is a handful of kilobytes, a handful of times).
static TABLE: AtomicPtr<HashMap<String, String>> = AtomicPtr::new(std::ptr::null_mut());

/// The catalogues that ship inside the binary: a language works on every
/// machine the shell runs on, with nothing to install beside it. A file in
/// the data directory with the same tag wins over these (a translator's
/// working copy).
const EMBEDDED: [(&str, &str); 1] = [("es", include_str!("../assets/i18n/es.json"))];

fn table() -> Option<&'static HashMap<String, String>> {
    // SAFETY: the pointer is null or a leaked `Box`, never freed.
    unsafe { TABLE.load(Ordering::Acquire).as_ref() }
}

/// Load the translation table for the session's language. Called once at
/// start, and again whenever the owner picks another language.
pub fn init() {
    let map = load_table();
    let ptr = if map.is_empty() { std::ptr::null_mut() } else { Box::into_raw(Box::new(map)) };
    TABLE.store(ptr, Ordering::Release);
}

/// The display form of an English source string: its translation when the
/// loaded table has one, the literal itself otherwise.
pub fn tr(en: &'static str) -> &'static str {
    match table().and_then(|t| t.get(en)) {
        Some(s) => s.as_str(),
        None if pseudo() => marked(en),
        None => en,
    }
}

/// `WAVERUNNER_I18N=pseudo`: no language, but every string that goes through
/// the translator comes back between « and ». What is on the screen without
/// them is a string a translator would never be shown — the test of how
/// much of the shell is translatable (`tools/i18n/README.md`).
fn pseudo() -> bool {
    static ON: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *ON.get_or_init(|| std::env::var("WAVERUNNER_I18N").is_ok_and(|v| v == "pseudo"))
}

fn marked(en: &str) -> &'static str {
    static SEEN: std::sync::Mutex<Option<HashMap<String, &'static str>>> = std::sync::Mutex::new(None);
    let Ok(mut seen) = SEEN.lock() else {
        return "«?»";
    };
    let seen = seen.get_or_insert_with(HashMap::new);
    if let Some(s) = seen.get(en) {
        return s;
    }
    let s: &'static str = Box::leak(format!("«{en}»").into_boxed_str());
    seen.insert(en.to_owned(), s);
    s
}

/// A translated template with its `{name}` holes filled: the display form of
/// what was a `format!`. The template is looked up whole, so the translator
/// decides where each value goes in the sentence:
///
/// ```ignore
/// fill(tr("Connected to {name}"), &[("name", &ssid)])
/// ```
///
/// A hole with no value stays as written; a value with no hole is unused.
pub fn fill(template: &str, values: &[(&str, &(dyn std::fmt::Display + Send + Sync))]) -> String {
    let mut out = String::with_capacity(template.len() + 16);
    let mut rest = template;
    while let Some(open) = rest.find('{') {
        out.push_str(&rest[..open]);
        let after = &rest[open + 1..];
        match after.find('}') {
            Some(close) => {
                let name = &after[..close];
                match values.iter().find(|(k, _)| *k == name) {
                    Some((_, v)) => {
                        use std::fmt::Write;
                        let _ = write!(out, "{v}");
                    }
                    None => {
                        out.push('{');
                        out.push_str(name);
                        out.push('}');
                    }
                }
                rest = &after[close + 1..];
            }
            None => {
                out.push('{');
                rest = after;
            }
        }
    }
    out.push_str(rest);
    out
}

/// [`tr`] for strings that aren't literals in this crate — offer titles
/// arrive from the options engine as `String`s, and they are user-visible
/// (the pill tooltips), so they must hit the same table. The English text
/// is the key exactly as with [`tr`]; a translation (being `'static`)
/// happily outlives any input lifetime.
pub fn tr_dyn(en: &str) -> &str {
    match table().and_then(|t| t.get(en)) {
        Some(s) => s.as_str(),
        None if pseudo() => marked(en),
        None => en,
    }
}

fn load_table() -> HashMap<String, String> {
    // Explicit file override wins (the dictionary's `$WAVERUNNER_DICT` shape).
    if let Ok(path) = std::env::var("WAVERUNNER_I18N") {
        if !path.is_empty() && path != "pseudo" {
            return read_file(std::path::Path::new(&path)).unwrap_or_default();
        }
    }
    // The language chosen in the control panel, then the session's.
    let chosen = crate::display::GolemSettings::load().language;
    let Some(tags) = locale_tags(
        chosen.or_else(|| std::env::var("LC_ALL").ok()),
        std::env::var("LC_MESSAGES").ok(),
        std::env::var("LANG").ok(),
    ) else {
        return HashMap::new();
    };
    for tag in tags {
        let path = crate::persist::data_path(&format!("i18n/{tag}.json"));
        if let Some(map) = read_file(&path) {
            info!("i18n: loaded {} strings from {}", map.len(), path.display());
            return map;
        }
        if let Some((_, text)) = EMBEDDED.iter().find(|(t, _)| *t == tag) {
            match serde_json::from_str::<HashMap<String, String>>(text) {
                Ok(map) => {
                    info!("i18n: {} strings of the built-in {tag} catalogue", map.len());
                    return map;
                }
                Err(e) => warn!("i18n: the built-in {tag} catalogue does not parse: {e}"),
            }
        }
    }
    HashMap::new()
}

/// The languages there is a catalogue for (their tags), besides English.
pub fn catalogues() -> Vec<String> {
    let mut tags: Vec<String> = EMBEDDED.iter().map(|(t, _)| (*t).to_owned()).collect();
    let dir = crate::persist::data_path("i18n");
    for e in std::fs::read_dir(dir).into_iter().flatten().flatten() {
        let name = e.file_name().to_string_lossy().into_owned();
        if let Some(tag) = name.strip_suffix(".json") {
            // `es_BO.json` is a country's wording of `es`, not another language.
            if !tag.contains('_') && !tags.iter().any(|t| t == tag) {
                tags.push(tag.to_owned());
            }
        }
    }
    tags.sort();
    tags
}

/// A language's name as its speakers write it.
pub fn language_name(tag: &str) -> String {
    match tag {
        "en" => "English",
        "es" => "Español",
        "pt" => "Português",
        "fr" => "Français",
        "de" => "Deutsch",
        "it" => "Italiano",
        "nl" => "Nederlands",
        "pl" => "Polski",
        "tr" => "Türkçe",
        "ru" => "Русский",
        "ja" => "日本語",
        "zh" => "中文",
        other => other,
    }
    .to_owned()
}

fn read_file(path: &std::path::Path) -> Option<HashMap<String, String>> {
    let bytes = std::fs::read(path).ok()?;
    match serde_json::from_slice(&bytes) {
        Ok(map) => Some(map),
        Err(e) => {
            warn!(
                "i18n: {} is not a flat string map ({e}) — ignoring",
                path.display()
            );
            None
        }
    }
}

/// The locale tags to try, most specific first: for `es_BO.UTF-8` that is
/// `["es_BO", "es"]`. `C`/`POSIX` and English mean "no table" (`None`) —
/// English is the source language, there is nothing to load.
fn locale_tags(
    lc_all: Option<String>,
    lc_messages: Option<String>,
    lang: Option<String>,
) -> Option<Vec<String>> {
    let raw = [lc_all, lc_messages, lang]
        .into_iter()
        .flatten()
        .find(|v| !v.is_empty())?;
    let tag = raw.split(['.', '@']).next().unwrap_or("");
    let primary = tag.split('_').next().unwrap_or("");
    if matches!(primary, "" | "C" | "POSIX" | "en") {
        return None;
    }
    let mut tags = vec![tag.to_owned()];
    if primary != tag {
        tags.push(primary.to_owned());
    }
    Some(tags)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The string literals handed to `tr(…)` / `tr_dyn(…)` in a source text.
    fn tr_literals(src: &str) -> Vec<String> {
        let mut found = Vec::new();
        for marker in ["tr(\"", "tr_dyn(\""] {
            let mut rest = src;
            while let Some(at) = rest.find(marker) {
                // (`str(`, `attr(`…: only the whole word.)
                let whole = !rest[..at].ends_with(|c: char| c.is_alphanumeric() || c == '_');
                let body = &rest[at + marker.len()..];
                let mut text = String::new();
                let mut chars = body.chars();
                let mut closed = false;
                while let Some(c) = chars.next() {
                    match c {
                        '"' => {
                            closed = true;
                            break;
                        }
                        '\\' => match chars.next() {
                            Some('n') => text.push('\n'),
                            Some('t') => text.push('\t'),
                            Some(other) => text.push(other),
                            None => break,
                        },
                        c => text.push(c),
                    }
                }
                // Only `tr("…")` with nothing after the literal: a call that
                // goes on (`tr("a" …`) is not a plain key.
                if whole && closed && chars.as_str().trim_start().starts_with(')') && !text.contains("\\u{") {
                    found.push(text);
                }
                rest = &rest[at + marker.len()..];
            }
        }
        found
    }

    fn sources(dir: &std::path::Path, out: &mut Vec<std::path::PathBuf>) {
        for e in std::fs::read_dir(dir).into_iter().flatten().flatten() {
            let p = e.path();
            if p.is_dir() {
                sources(&p, out);
            } else if p.extension().is_some_and(|x| x == "rs") {
                out.push(p);
            }
        }
    }

    /// Every string the shell hands the translator has its Spanish: a new
    /// `tr("…")` without an entry in `assets/i18n/es.json` fails here, with
    /// the strings to add. (The other half — a visible literal that never
    /// reaches `tr` — is `tools/i18n/scan.py`'s to find.)
    #[test]
    fn every_translated_string_has_its_spanish() {
        let es: HashMap<String, String> = serde_json::from_str(EMBEDDED[0].1).expect("es.json parses");
        let mut files = Vec::new();
        sources(&std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src"), &mut files);
        assert!(files.len() > 50, "the daemon's sources were not found");
        let mut missing = Vec::new();
        for file in files {
            let src = std::fs::read_to_string(&file).unwrap_or_default();
            // (Tests hand `tr` strings of their own.)
            let body = src.split("\n#[cfg(test)]").next().unwrap_or(&src);
            for key in tr_literals(body) {
                if !es.contains_key(&key) && !missing.contains(&key) {
                    missing.push(key);
                }
            }
        }
        assert!(missing.is_empty(), "no Spanish for {} string(s): {missing:#?}", missing.len());
    }

    /// A translation keeps the holes of its English: a `{name}` lost or
    /// misspelt would show the braces, or drop a value.
    #[test]
    fn a_translation_keeps_its_holes() {
        let es: HashMap<String, String> = serde_json::from_str(EMBEDDED[0].1).expect("es.json parses");
        let holes = |s: &str| {
            let mut found: Vec<String> = Vec::new();
            let mut rest = s;
            while let Some(open) = rest.find('{') {
                let Some(close) = rest[open..].find('}') else { break };
                found.push(rest[open..open + close + 1].to_owned());
                rest = &rest[open + close + 1..];
            }
            found.sort();
            found
        };
        for (en, spanish) in &es {
            assert_eq!(holes(en), holes(spanish), "{en:?} → {spanish:?}");
            assert!(!spanish.is_empty(), "{en:?} has an empty translation");
        }
    }

    #[test]
    fn fill_puts_each_value_in_its_hole() {
        assert_eq!(fill("Connected to {name}", &[("name", &"HOLA")]), "Connected to HOLA");
        // The translator's order, not the caller's.
        assert_eq!(fill("{1} de {0} en uso", &[("0", &"8 GB"), ("1", &3.5)]), "3.5 de 8 GB en uso");
        // An unknown hole and a lone brace are left as written.
        assert_eq!(fill("{x} and {y", &[("y", &1)]), "{x} and {y");
        assert_eq!(fill("no holes", &[]), "no holes");
    }

    #[test]
    fn tr_without_init_returns_the_literal() {
        // Tests never call init(); every string must survive untranslated.
        assert_eq!(tr("No results"), "No results");
    }

    #[test]
    fn tr_dyn_passes_dynamic_strings_through_untranslated() {
        // Engine-built titles (Strings, not literals) survive the same way.
        let title = String::from("Open copied files");
        assert_eq!(tr_dyn(&title), "Open copied files");
    }

    #[test]
    fn locale_tags_prefer_lc_all_and_order_specific_first() {
        assert_eq!(
            locale_tags(Some("es_BO.UTF-8".into()), Some("de_DE".into()), None),
            Some(vec!["es_BO".to_owned(), "es".to_owned()])
        );
        assert_eq!(
            locale_tags(None, None, Some("fr".into())),
            Some(vec!["fr".to_owned()])
        );
        // Empty LC_ALL falls through to the next variable.
        assert_eq!(
            locale_tags(Some(String::new()), None, Some("pt_BR".into())),
            Some(vec!["pt_BR".to_owned(), "pt".to_owned()])
        );
    }

    #[test]
    fn locale_tags_treat_c_posix_and_english_as_source() {
        assert_eq!(locale_tags(Some("C".into()), None, None), None);
        assert_eq!(locale_tags(Some("POSIX".into()), None, None), None);
        assert_eq!(locale_tags(Some("en_US.UTF-8".into()), None, None), None);
        assert_eq!(locale_tags(None, None, None), None);
    }

    #[test]
    fn read_file_parses_a_flat_map_and_rejects_junk() {
        let dir = std::env::temp_dir().join("waverunner-i18n-test");
        std::fs::create_dir_all(&dir).unwrap();
        let good = dir.join("es.json");
        std::fs::write(&good, br#"{"No results": "Sin resultados"}"#).unwrap();
        let map = read_file(&good).unwrap();
        assert_eq!(map.get("No results").unwrap(), "Sin resultados");
        let bad = dir.join("bad.json");
        std::fs::write(&bad, b"[1,2,3]").unwrap();
        assert!(read_file(&bad).is_none());
        assert!(read_file(&dir.join("absent.json")).is_none());
    }
}
