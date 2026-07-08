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

    /// Rank `haystack` against `query`, returning *indices* into it,
    /// best first; non-matching items are dropped. An empty query keeps
    /// every index in original order — the UI's unfiltered view.
    pub fn rank(&mut self, query: &str, haystack: &[&str]) -> Vec<usize> {
        if query.is_empty() {
            return (0..haystack.len()).collect();
        }
        let pattern = Pattern::parse(query, CaseMatching::Ignore, Normalization::Smart);
        let mut buf = Vec::new();
        let mut scored: Vec<(usize, u32)> = haystack
            .iter()
            .enumerate()
            .filter_map(|(i, s)| {
                let utf32 = nucleo_matcher::Utf32Str::new(s, &mut buf);
                pattern
                    .score(utf32, &mut self.matcher)
                    .map(|score| (i, score))
            })
            .collect();
        // Stable order for equal scores: keep the alphabetical entry order.
        scored.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(&b.0)));
        scored.into_iter().map(|(i, _)| i).collect()
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

    #[test]
    fn rank_returns_indices_best_first() {
        let mut s = Searcher::new();
        let ranked = s.rank("fire", &["LibreOffice", "Firefox", "Files"]);
        assert_eq!(ranked.first(), Some(&1));
    }

    #[test]
    fn rank_empty_query_keeps_order() {
        let mut s = Searcher::new();
        assert_eq!(s.rank("", &["b", "a", "c"]), vec![0, 1, 2]);
    }
}
