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
use std::sync::OnceLock;

use tracing::{info, warn};

static TABLE: OnceLock<HashMap<String, String>> = OnceLock::new();

/// Load the translation table for the session locale. Call once, before the
/// first draw and before threads that call [`tr`] exist.
pub fn init() {
    let _ = TABLE.set(load_table());
}

/// The display form of an English source string: its translation when the
/// loaded table has one, the literal itself otherwise.
pub fn tr(en: &'static str) -> &'static str {
    match TABLE.get().and_then(|t| t.get(en)) {
        Some(s) => s.as_str(),
        None => en,
    }
}

fn load_table() -> HashMap<String, String> {
    // Explicit file override wins (the dictionary's `$WAVERUNNER_DICT` shape).
    if let Ok(path) = std::env::var("WAVERUNNER_I18N") {
        if !path.is_empty() {
            return read_file(std::path::Path::new(&path)).unwrap_or_default();
        }
    }
    let Some(tags) = locale_tags(
        std::env::var("LC_ALL").ok(),
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
    }
    HashMap::new()
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

    #[test]
    fn tr_without_init_returns_the_literal() {
        // Tests never call init(); every string must survive untranslated.
        assert_eq!(tr("No results"), "No results");
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
