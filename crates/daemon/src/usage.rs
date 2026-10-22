//! What you launch NOW — app-launch frequency that forgets.
//!
//! Every launch adds 1 to the app's score, and every score halves each
//! [`HALF_LIFE`]. So the ranking follows the present, not the record: an app
//! used every day for a year and one you switched to this week are judged by
//! the last few days alone, and at equal use the newcomer passes the old
//! habit after ONE half-life — however long the old habit lasted (Max,
//! 2026-10-13: Chrome, unused for weeks, still stood first in fast launch on
//! the strength of 266 launches from when it was his browser; *"si uso
//! firefox por un año, y de repente quiero cambiar a brave… me gustaría que
//! [la vieja] se mueva lo más rápido posible a un costado"*).
//!
//! Forgetting is exponential, which has two properties worth relying on:
//! * it keeps RATIOS — a week away from the computer decays everything alike,
//!   so the order you left is the order you come back to;
//! * a score never reaches zero, so apps you no longer use still sort by how
//!   recently you last did, instead of collapsing into a tie.
//!
//! Stored in `$XDG_DATA_HOME/waverunner/usage-recent.json` as
//! `{ "app-id": { "score": f64, "at": unix_seconds } }` — the score as it
//! stood at `at`; it is decayed to the moment it is read.
//!
//! The file this replaces (`usage.json`, plain lifetime counts, no dates) is
//! read ONCE, the first time, and left alone. Its counts carry no dates, so
//! nothing in it can say what is current: each becomes a seed of at most
//! [`LEGACY_SEED`] (in proportion to its count, as of the migration). That
//! keeps the old order for the apps not launched since, and puts every app
//! you actually launch — worth a full 1.0 the first time — above all of it.
//!
//! Writes are synchronous and best-effort: a failed write is logged but
//! never fatal — the in-memory scores are always correct for the running
//! session.

use std::collections::HashMap;
use std::path::PathBuf;

use serde::{Deserialize, Serialize};

/// How long it takes a score to halve. Two days: yesterday still counts for
/// most of what it was, last week for almost nothing.
pub(crate) const HALF_LIFE: f64 = 2.0 * 24.0 * 3600.0;

/// What one use `age` seconds ago is worth now (1 at once, ½ a
/// [`HALF_LIFE`] later) — the same forgetting for other things in use (a
/// lone app's recents, `web_actions::rank`).
pub(crate) fn decayed(age: f64) -> f64 {
    0.5f64.powf(age.max(0.0) / HALF_LIFE)
}

/// Now, as unix seconds.
pub(crate) fn now_secs() -> f64 {
    now()
}

/// The most a lifetime count from the old file is worth — under one launch.
const LEGACY_SEED: f64 = 0.5;

/// A score as it stood at a moment.
#[derive(Clone, Copy, Debug, Serialize, Deserialize)]
struct Mark {
    score: f64,
    /// Unix seconds.
    at: f64,
}

impl Mark {
    fn at(self, now: f64) -> f64 {
        // A clock set back must not INFLATE a score: no negative ages.
        let age = (now - self.at).max(0.0);
        self.score * 0.5f64.powf(age / HALF_LIFE)
    }
}

/// In-memory launch-frequency database, backed by a JSON file.
pub struct UsageDb {
    marks: HashMap<String, Mark>,
    path: PathBuf,
}

fn now() -> f64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs_f64())
        .unwrap_or(0.0)
}

/// Lifetime counts → seeds, in proportion, the largest worth [`LEGACY_SEED`].
fn seeds(counts: &HashMap<String, u32>, at: f64) -> HashMap<String, Mark> {
    let top = counts.values().copied().max().unwrap_or(0).max(1) as f64;
    counts
        .iter()
        .filter(|(_, c)| **c > 0)
        .map(|(id, c)| (id.clone(), Mark { score: LEGACY_SEED * *c as f64 / top, at }))
        .collect()
}

impl UsageDb {
    /// A database of its own in the data dir, `file` (no old counts to seed
    /// from): the same forgetting for other things than apps.
    pub fn load_named(file: &str) -> Self {
        let path = crate::persist::data_path(file);
        let marks = crate::persist::read_json::<HashMap<String, Mark>>(&path).unwrap_or_default();
        Self { marks, path }
    }

    /// Every key's score now and when it was last used (unix seconds).
    pub fn snapshot(&self) -> HashMap<String, (f64, f64)> {
        let now = now();
        self.marks.iter().map(|(k, m)| (k.clone(), (m.at(now), m.at))).collect()
    }

    /// Load from disk; the first time, seed from the old lifetime counts (see
    /// the module doc); with neither file, start empty.
    pub fn load() -> Self {
        let path = crate::persist::data_path("usage-recent.json");
        let marks = match crate::persist::read_json::<HashMap<String, Mark>>(&path) {
            Some(m) => m,
            None => {
                let old: HashMap<String, u32> =
                    crate::persist::read_json(&crate::persist::data_path("usage.json")).unwrap_or_default();
                let m = seeds(&old, now());
                if !m.is_empty() {
                    tracing::info!("usage: seeded {} apps from the lifetime counts; ranking follows recent use from here", m.len());
                    crate::persist::write_json("usage", &path, &m);
                }
                m
            }
        };
        Self { marks, path }
    }

    /// One more launch of `app_id`, now; persisted.
    pub fn increment(&mut self, app_id: &str) {
        self.increment_at(app_id, now());
        crate::persist::write_json("usage", &self.path, &self.marks);
    }

    fn increment_at(&mut self, app_id: &str, now: f64) {
        let score = self.marks.get(app_id).map_or(0.0, |m| m.at(now)) + 1.0;
        self.marks.insert(app_id.to_owned(), Mark { score, at: now });
    }

    /// How much `app_id` is in use NOW, as a number to SORT by (0 if never
    /// launched): its decayed score, in thousandths. Not a count of anything.
    pub fn count(&self, app_id: &str) -> u32 {
        self.rank_at(app_id, now())
    }

    fn rank_at(&self, app_id: &str, now: f64) -> u32 {
        let s = self.marks.get(app_id).map_or(0.0, |m| m.at(now));
        // Thousandths keep apart scores a week of forgetting has brought
        // close; the ceiling keeps a still-nonzero score from rounding into a
        // tie with "never launched".
        if s <= 0.0 { 0 } else { (s * 1000.0).ceil().min(u32::MAX as f64) as u32 }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const DAY: f64 = 24.0 * 3600.0;

    fn db() -> UsageDb {
        UsageDb { marks: HashMap::new(), path: std::env::temp_dir().join("waverunner-test-usage-recent.json") }
    }
    /// `per_day` launches a day of `app`, for `days` days ending at `end`.
    fn use_daily(db: &mut UsageDb, app: &str, per_day: usize, days: usize, end: f64) {
        for d in (0..days).rev() {
            for k in 0..per_day {
                db.increment_at(app, end - d as f64 * DAY - k as f64 * 600.0);
            }
        }
    }

    #[test]
    fn launches_accumulate_and_unknown_is_zero() {
        let mut db = db();
        assert_eq!(db.rank_at("firefox", 0.0), 0);
        db.increment_at("firefox", 0.0);
        db.increment_at("firefox", 0.0);
        db.increment_at("foot", 0.0);
        assert!(db.rank_at("firefox", 0.0) > db.rank_at("foot", 0.0));
        assert_eq!(db.rank_at("never-launched", 0.0), 0);
    }

    #[test]
    fn a_score_halves_every_half_life_and_never_reaches_zero() {
        let mut db = db();
        db.increment_at("a", 0.0);
        assert_eq!(db.rank_at("a", 0.0), 1000);
        assert_eq!(db.rank_at("a", HALF_LIFE), 500);
        assert_eq!(db.rank_at("a", 2.0 * HALF_LIFE), 250);
        // A month on: next to nothing, but still ahead of "never".
        assert!(db.rank_at("a", 30.0 * DAY) >= 1);
        // A clock set back does not inflate it.
        assert_eq!(db.rank_at("a", -5.0 * DAY), 1000);
    }

    #[test]
    fn a_new_habit_passes_a_year_old_one_within_a_half_life() {
        // Firefox, five times a day for a year; then Brave instead, the same five a day.
        let mut db = db();
        let switch = 365.0 * DAY;
        use_daily(&mut db, "firefox", 5, 365, switch);
        // The day of the switch: Firefox is still far ahead.
        use_daily(&mut db, "brave", 5, 1, switch + DAY);
        assert!(db.rank_at("firefox", switch + DAY) > db.rank_at("brave", switch + DAY));
        // Two days of Brave: level. Three: Brave is clearly first.
        use_daily(&mut db, "brave", 5, 2, switch + 3.0 * DAY);
        assert!(db.rank_at("brave", switch + 3.0 * DAY) > db.rank_at("firefox", switch + 3.0 * DAY));
        // And the length of the old habit did not matter: ten years of it loses just as fast.
        let mut long = self::db();
        use_daily(&mut long, "firefox", 5, 3650, 3650.0 * DAY);
        use_daily(&mut long, "brave", 5, 3, 3653.0 * DAY);
        assert!(long.rank_at("brave", 3653.0 * DAY) > long.rank_at("firefox", 3653.0 * DAY));
    }

    #[test]
    fn time_away_keeps_the_order() {
        let mut db = db();
        use_daily(&mut db, "foot", 6, 10, 10.0 * DAY);
        use_daily(&mut db, "thunar", 2, 10, 10.0 * DAY);
        let after = 31.0 * DAY; // three weeks without touching the computer
        assert!(db.rank_at("foot", after) > db.rank_at("thunar", after));
        assert!(db.rank_at("thunar", after) > 0);
    }

    #[test]
    fn lifetime_counts_seed_below_a_single_launch() {
        let old: HashMap<String, u32> =
            [("google-chrome", 266u32), ("foot", 244), ("seam", 90), ("zero", 0)].into_iter().map(|(k, v)| (k.to_owned(), v)).collect();
        let mut db = db();
        db.marks = seeds(&old, 0.0);
        // The old order holds among apps not launched since…
        assert!(db.rank_at("google-chrome", 0.0) > db.rank_at("foot", 0.0));
        assert!(db.rank_at("foot", 0.0) > db.rank_at("seam", 0.0));
        assert_eq!(db.rank_at("zero", 0.0), 0);
        // …and ONE launch of anything puts it above the lot.
        db.increment_at("seam", 60.0);
        assert!(db.rank_at("seam", 60.0) > db.rank_at("google-chrome", 60.0));
        db.increment_at("brand-new", 120.0);
        assert!(db.rank_at("brand-new", 120.0) > db.rank_at("google-chrome", 120.0));
    }
}
