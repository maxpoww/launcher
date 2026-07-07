//! Fuzzy matching over indexed applications, backed by `nucleo-matcher`.

use nucleo_matcher::pattern::{CaseMatching, Normalization, Pattern};
use nucleo_matcher::{Config, Matcher};

/// A reusable fuzzy searcher. Owns the (heap-heavy) `nucleo` matcher so it
/// is allocated once per daemon, not once per keystroke.
pub struct Searcher {
    matcher: Matcher,
}

impl Searcher {
    /// Create a searcher tuned for GUI item matching.
    pub fn new() -> Self {
        Self {
            matcher: Matcher::new(Config::DEFAULT),
        }
    }

    /// Rank `haystack` against `query`, best score first.
    ///
    /// An empty query returns the haystack unranked (score 0), so the UI
    /// can show the full list before the user types.
    pub fn search<'a>(&mut self, query: &str, haystack: &[&'a str]) -> Vec<(&'a str, u32)> {
        if query.is_empty() {
            return haystack.iter().map(|s| (*s, 0)).collect();
        }
        let pattern = Pattern::parse(query, CaseMatching::Ignore, Normalization::Smart);
        pattern.match_list(haystack.iter().copied(), &mut self.matcher)
    }
}

impl Default for Searcher {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ranks_prefix_match_first() {
        let mut s = Searcher::new();
        let results = s.search("fire", &["LibreOffice", "Firefox", "Files"]);
        assert_eq!(results.first().map(|(name, _)| *name), Some("Firefox"));
    }

    #[test]
    fn empty_query_returns_everything() {
        let mut s = Searcher::new();
        assert_eq!(s.search("", &["a", "b"]).len(), 2);
    }

    #[test]
    fn no_match_returns_empty() {
        let mut s = Searcher::new();
        assert!(s.search("zzzz", &["Firefox"]).is_empty());
    }
}
