//! Offline dictionary lookup for the clipboard OPTION's "define a word" panel.
//!
//! Fully local: one or more word→definition maps are loaded from JSON files
//! (`{ "word": "definition", … }`, lowercase headwords — e.g. the public-domain
//! Webster's 1913 for English, a synonyms list for Spanish), then every lookup
//! is an in-memory hash hit across every loaded language. The multi-MB parse
//! must never block the render loop, so the load runs on a worker thread and the
//! finished [`Dict`] is handed back over a calloop channel (like the unfurl /
//! thumbnail workers); the event loop then answers keystrokes synchronously.
//!
//! Data paths (each optional, absent → skipped; all absent → a "not installed"
//! hint): English `$WAVERUNNER_DICT` or `<data-dir>/dictionary.json`; Spanish
//! `$WAVERUNNER_DICT_ES` or `<data-dir>/dictionary-es.json`.

use std::collections::HashMap;
use std::path::PathBuf;

use calloop::channel::Sender;
use tracing::{debug, warn};

/// The languages looked up, in display order: `(label, data file, env override)`.
const LANGS: &[(&str, &str, &str)] = &[
    ("English", "dictionary.json", "WAVERUNNER_DICT"),
    ("Español", "dictionary-es.json", "WAVERUNNER_DICT_ES"),
];

/// A headword's content: its definition and, when the source has one, its
/// etymology (Spanish RAE entries carry "Del lat. …"; the English Webster set
/// does not).
struct Def {
    def: String,
    etym: Option<String>,
}

/// On-disk value for a headword: either a bare definition string, or a
/// `{"e": etymology, "d": definition}` object. Both forms are accepted so the
/// English file (plain strings) and the Spanish file (objects with etymology)
/// share one loader.
#[derive(serde::Deserialize)]
#[serde(untagged)]
enum RawEntry {
    Plain(String),
    Rich {
        #[serde(default)]
        e: String,
        d: String,
    },
}

impl From<RawEntry> for Def {
    fn from(r: RawEntry) -> Self {
        match r {
            RawEntry::Plain(d) => Def { def: d, etym: None },
            RawEntry::Rich { e, d } => Def {
                def: d,
                etym: (!e.is_empty()).then_some(e),
            },
        }
    }
}

/// One loaded language: its label, the headword→content map (lowercased keys),
/// and an accent-folded index (folded headword → canonical key) so a query typed
/// without accents ("corazon") still finds the accented entry ("corazón") —
/// Spanish speakers routinely omit the tildes.
struct LangData {
    label: &'static str,
    map: HashMap<String, Def>,
    fold: HashMap<String, String>,
}

/// A resident multi-language dictionary, kept in display order.
pub struct Dict {
    langs: Vec<LangData>,
}

/// One language's hit for a looked-up word.
pub struct Entry<'a> {
    /// Display label of the language ("English" / "Español").
    pub lang: &'static str,
    /// The definition text for that language.
    pub definition: &'a str,
    /// The word's origin ("Del lat. cor"), when the source records one.
    pub etymology: Option<&'a str>,
}

/// Strip Spanish/Latin diacritics so an unaccented query matches an accented
/// headword. `ñ` and `ç` are kept — they're distinct letters (año ≠ ano).
fn fold_accents(s: &str) -> String {
    s.chars()
        .map(|c| match c {
            'á' | 'à' | 'ä' | 'â' | 'ã' => 'a',
            'é' | 'è' | 'ë' | 'ê' => 'e',
            'í' | 'ì' | 'ï' | 'î' => 'i',
            'ó' | 'ò' | 'ö' | 'ô' | 'õ' => 'o',
            'ú' | 'ù' | 'ü' | 'û' => 'u',
            c => c,
        })
        .collect()
}

impl Dict {
    /// Every language that defines `word`, in display order — so a word present
    /// in both (a "false friend") shows both entries. Case-, space-, and
    /// accent-insensitive: an exact (lowercased) match wins; failing that, an
    /// accent-folded match ("corazon" → "corazón").
    pub fn lookup(&self, word: &str) -> Vec<Entry<'_>> {
        let key = word.trim().to_lowercase();
        if key.is_empty() {
            return Vec::new();
        }
        let folded = fold_accents(&key);
        let mut out = Vec::new();
        for lang in &self.langs {
            let entry = lang
                .map
                .get(&key)
                .or_else(|| lang.fold.get(&folded).and_then(|canon| lang.map.get(canon)));
            if let Some(entry) = entry {
                out.push(Entry {
                    lang: lang.label,
                    definition: entry.def.as_str(),
                    etymology: entry.etym.as_deref(),
                });
            }
        }
        out
    }

    /// Total headwords across all languages (a load confirmation in the log).
    pub fn total_words(&self) -> usize {
        self.langs.iter().map(|l| l.map.len()).sum()
    }
}

/// A finished load, folded into the clipboard state on the event loop.
pub enum Event {
    /// `Ok` = the resident dictionary; `Err` = a human-readable reason (no data
    /// files found) for the panel's "not installed" hint.
    Loaded(Result<Dict, String>),
}

/// A language file's path: its `$WAVERUNNER_DICT*` override, else the data dir.
fn dict_path(file: &str, env: &str) -> PathBuf {
    if let Ok(p) = std::env::var(env) {
        if !p.is_empty() {
            return PathBuf::from(p);
        }
    }
    crate::persist::data_path(file)
}

/// Load one language file: `None` if it's simply absent, an error string if it's
/// present but unreadable / malformed. Headwords are lowercased (Unicode-aware)
/// so accented Spanish queries match whatever the source's casing was.
fn load_one(file: &str, env: &str) -> Result<Option<HashMap<String, Def>>, String> {
    let path = dict_path(file, env);
    let bytes = match std::fs::read(&path) {
        Ok(b) => b,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(format!("{}: {e}", path.display())),
    };
    let raw: HashMap<String, RawEntry> =
        serde_json::from_slice(&bytes).map_err(|e| format!("parse {}: {e}", path.display()))?;
    let map = raw
        .into_iter()
        .map(|(k, v)| (k.to_lowercase(), v.into()))
        .collect();
    Ok(Some(map))
}

/// Read + parse every available language file. Slow (multi-MB parse) — worker
/// thread only. A malformed file is skipped with a warning; only all-absent is
/// a hard error (so the panel can hint that data isn't installed).
fn load() -> Result<Dict, String> {
    let mut langs = Vec::new();
    for &(label, file, env) in LANGS {
        match load_one(file, env) {
            Ok(Some(map)) if !map.is_empty() => {
                // Accent-folded index: only for keys that actually carry accents
                // (unaccented keys are found by the exact match); first accented
                // form wins a collision (público/publicó → publico).
                let mut fold = HashMap::new();
                for k in map.keys() {
                    let f = fold_accents(k);
                    if f != *k {
                        fold.entry(f).or_insert_with(|| k.clone());
                    }
                }
                langs.push(LangData { label, map, fold });
            }
            Ok(_) => {}
            Err(e) => warn!("dict: skipping {label} ({e})"),
        }
    }
    if langs.is_empty() {
        return Err("no dictionary data (expected dictionary.json in the data dir)".to_owned());
    }
    Ok(Dict { langs })
}

/// Load the dictionary on a one-shot worker thread, delivering the result to the
/// event loop via `tx`. Called lazily the first time the panel opens, so users
/// who never look up a word never pay the parse or the resident maps.
pub fn spawn_load(tx: Sender<Event>) {
    let spawned = std::thread::Builder::new()
        .name("waverunner-dict".into())
        .spawn(move || {
            let result = load();
            match &result {
                Ok(d) => debug!("dict: loaded {} words", d.total_words()),
                Err(e) => debug!("dict: load failed ({e})"),
            }
            let _ = tx.send(Event::Loaded(result));
        });
    if let Err(e) = spawned {
        warn!("cannot spawn dict thread: {e}");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dict() -> Dict {
        let map: HashMap<String, Def> = [
            ("corazón", "órgano", Some("Del lat. cor")),
            ("ano", "orificio", None),
            ("año", "periodo", None),
            ("publico", "yo publico", None),
            ("público", "gente", None),
        ]
        .into_iter()
        .map(|(k, d, e)| {
            (
                k.to_owned(),
                Def {
                    def: d.to_owned(),
                    etym: e.map(str::to_owned),
                },
            )
        })
        .collect();
        let mut fold = HashMap::new();
        for k in map.keys() {
            let f = fold_accents(k);
            if f != *k {
                fold.entry(f).or_insert_with(|| k.clone());
            }
        }
        Dict {
            langs: vec![LangData {
                label: "Español",
                map,
                fold,
            }],
        }
    }

    fn def<'a>(d: &'a Dict, w: &str) -> Option<&'a str> {
        d.lookup(w).first().map(|e| e.definition)
    }

    #[test]
    fn unaccented_query_finds_accented_headword() {
        assert_eq!(def(&dict(), "corazon"), Some("órgano"));
        assert_eq!(def(&dict(), "CORAZON"), Some("órgano"));
        assert_eq!(def(&dict(), "corazón"), Some("órgano"));
    }

    #[test]
    fn etymology_is_exposed_when_present() {
        let d = dict();
        assert_eq!(d.lookup("corazon")[0].etymology, Some("Del lat. cor"));
        assert_eq!(d.lookup("ano")[0].etymology, None);
    }

    #[test]
    fn exact_match_wins_over_fold_and_enye_is_distinct() {
        // "ano" and "año" are different words — ñ is never folded to n.
        assert_eq!(def(&dict(), "ano"), Some("orificio"));
        assert_eq!(def(&dict(), "año"), Some("periodo"));
        // An exact (unaccented) headword beats its accented fold-twin.
        assert_eq!(def(&dict(), "publico"), Some("yo publico"));
        assert_eq!(def(&dict(), "público"), Some("gente"));
    }
}
