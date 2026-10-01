//! FAST LAUNCH (Max, 2026-10-01): Super+Alt+Space → one bubble in the
//! middle of the screen, a search for APPS only, warm with what you use.
//! Nothing shows while more than three apps match; at three their icons
//! appear beside the search, then two, then one. Enter launches the best
//! (a NEW instance) even before it has narrowed; a click on an icon launches
//! that one; Escape or a click anywhere else closes it. No card, no grid —
//! the dock surface draws it with the app icons it already holds, so it
//! costs no extra memory.
//!
//! Ranking, best first: a name that starts with the letters, then a word of
//! the name that does ("code" → Visual Studio Code), then the initials ("vs"
//! → Visual Studio Code), then the letters anywhere in the name; ties go to
//! the app used most, then the shorter name.

use smithay_client_toolkit::seat::keyboard::Keysym;

use crate::content::Rect;
use crate::{apps, App, KbSurface, LaunchFrom};

/// How many matches have to remain before their icons show.
pub(crate) const REVEAL_AT: usize = 3;

/// Max's look (the Fast Launch mockup, 2026-10-01), logical px at bar
/// scale 1: an empty glass SLOT anchored mid-screen; the best match takes
/// it, the others hang smaller off its sides without moving it; the typed
/// letters sit right under it; every icon wears its name on top.
const SLOT: f32 = 58.0;
const SIDE: f32 = 44.0;
const SIDE_GAP: f32 = 36.0;
pub(crate) const FONT_PX: f32 = 20.0;
pub(crate) const LINE_PX: f32 = 25.0;
pub(crate) const NAME_PX: f32 = 13.0;
pub(crate) const SIDE_NAME_PX: f32 = 11.5;
/// How fast the slot and the icons ease in (1/s): snappy.
pub(crate) const EASE_RATE: f32 = 26.0;

/// How well an app `name` matches `q` (both lowercased): 4 its name starts
/// with it, 3 a word does, 2 the initials do, 1 it is anywhere in the name;
/// 0 no match.
pub(crate) fn match_tier(name: &str, q: &str) -> u8 {
    if q.is_empty() {
        return 0;
    }
    if name.starts_with(q) {
        return 4;
    }
    let words: Vec<&str> = name
        .split(|c: char| !c.is_alphanumeric())
        .filter(|w| !w.is_empty())
        .collect();
    if words.iter().any(|w| w.starts_with(q)) {
        return 3;
    }
    let initials: String = words.iter().filter_map(|w| w.chars().next()).collect();
    if initials.starts_with(q) {
        return 2;
    }
    if name.contains(q) {
        return 1;
    }
    0
}

/// Rank `apps` — (name, times used) — for the query `q`: the indices of the
/// matches, best first (see the module doc).
pub(crate) fn rank(apps: &[(&str, u32)], q: &str) -> Vec<usize> {
    let q = q.trim().to_lowercase();
    let mut hits: Vec<(u8, u32, usize, usize)> = apps
        .iter()
        .enumerate()
        .filter_map(|(i, (name, used))| {
            let name = name.to_lowercase();
            let tier = match_tier(&name, &q);
            (tier > 0).then_some((tier, *used, name.len(), i))
        })
        .collect();
    hits.sort_by(|a, b| b.0.cmp(&a.0).then(b.1.cmp(&a.1)).then(a.2.cmp(&b.2)));
    hits.into_iter().map(|(.., i)| i).collect()
}

/// The fast-launch state.
#[derive(Default)]
pub(crate) struct FastLaunch {
    pub open: bool,
    pub query: String,
    /// Which of the shown icons Enter launches (0 = the best).
    pub sel: usize,
    /// The bubble's presence, easing 0 → 1 (and back as it closes).
    pub k: f32,
    /// The matching apps (entry indices), best first.
    pub matches: Vec<usize>,
    /// Each shown icon's presence, easing in as it appears.
    pub icon_k: Vec<f32>,
}

impl FastLaunch {
    /// The icons on show: the matches, once there are at most three.
    pub(crate) fn shown(&self) -> &[usize] {
        if self.matches.len() <= REVEAL_AT {
            &self.matches
        } else {
            &[]
        }
    }
}

/// The order the shown icons take their places in: the selected one in
/// the slot, the others left then right of it.
pub(crate) fn placement(n: usize, sel: usize) -> Vec<usize> {
    let mut order = Vec::with_capacity(n);
    if n > 0 {
        order.push(sel.min(n - 1));
        order.extend((0..n).filter(|&i| i != sel.min(n - 1)));
    }
    order
}

/// Where the slot and the icons beside it sit on the dock surface: the slot
/// is centred on the screen's centre (or as near as the surface reaches) and
/// never moves; place 1 hangs off its left, place 2 off its right.
pub(crate) fn geometry(surface: (f32, f32), screen_h: f32, scale: f32) -> [Rect; 3] {
    let (w, h) = surface;
    let slot = SLOT * scale;
    let side = SIDE * scale;
    let gap = SIDE_GAP * scale;
    // The surface is anchored to the screen's bottom edge: the screen's
    // centre is `screen_h / 2` above that edge.
    let cy = (h - screen_h / 2.0).clamp(slot, h - slot);
    let main = Rect::new(w / 2.0 - slot / 2.0, cy - slot / 2.0, slot, slot);
    let left = Rect::new(main.x - gap - side, cy - side / 2.0, side, side);
    let right = Rect::new(main.x + slot + gap, cy - side / 2.0, side, side);
    [main, left, right]
}

impl App {
    /// Super+Alt+Space: open the bubble, or close it if it is up.
    pub(crate) fn toggle_fast_launch(&mut self) {
        if self.fast.open {
            self.close_fast_launch();
            return;
        }
        // The big card is the other way to launch: out of the way.
        if self.ui.target() == crate::state::Target::Open {
            self.handle_command(waverunner_proto::Command::Collapse);
        }
        self.fast = FastLaunch { open: true, k: self.fast.k, ..FastLaunch::default() };
        // Take the keyboard: what you type goes into the bubble.
        self.cancel_keyboard_handback(KbSurface::Launcher);
        crate::surface::set_interactive(&self.layer, true);
        self.interactive = true;
        self.sync_input_region();
        self.schedule_frame();
    }

    /// Close the bubble and hand the keyboard back to the window you were on.
    pub(crate) fn close_fast_launch(&mut self) {
        if !self.fast.open {
            return;
        }
        self.fast.open = false;
        if self.interactive && self.ui.target() != crate::state::Target::Open {
            self.begin_keyboard_handback(KbSurface::Launcher, None);
            crate::surface::set_interactive(&self.layer, false);
            self.interactive = false;
        }
        self.sync_input_region();
        self.schedule_frame();
    }

    /// Re-rank for the current query: apps only, the real ones (no catalog
    /// webapps, no Apps button / control panel / Bin, no transients).
    pub(crate) fn fast_rematch(&mut self) {
        let candidates: Vec<usize> = (0..self.base_len.min(self.entries.len()))
            .filter(|&i| {
                self.kinds.get(i) == Some(&apps::EntryKind::App)
                    && !apps::is_dock_fixed(&self.entries[i].id)
                    && !self.is_catalog_webapp(i)
            })
            .collect();
        let named: Vec<(&str, u32)> = candidates
            .iter()
            .map(|&i| (self.entries[i].name.as_str(), self.usage.count(&self.entries[i].id)))
            .collect();
        let order = rank(&named, &self.fast.query);
        let matches: Vec<usize> = order.into_iter().map(|j| candidates[j]).collect();
        if matches != self.fast.matches {
            // Icons already on show keep their presence; new ones ease in.
            let keep = self.fast.icon_k.clone();
            let was = self.fast.shown().to_vec();
            self.fast.matches = matches;
            self.fast.icon_k = self
                .fast
                .shown()
                .iter()
                .map(|e| was.iter().position(|w| w == e).and_then(|p| keep.get(p).copied()).unwrap_or(0.0))
                .collect();
        }
        self.fast.sel = self.fast.sel.min(self.fast.shown().len().saturating_sub(1));
    }

    /// A key while the bubble is open: it takes every key.
    pub(crate) fn fast_key(&mut self, keysym: Keysym, utf8: Option<&str>) {

        match keysym {
            Keysym::Escape => self.close_fast_launch(),
            Keysym::Return | Keysym::KP_Enter => self.fast_launch_pick(None),
            Keysym::BackSpace => {
                self.fast.query.pop();
                self.fast.sel = 0;
                self.fast_rematch();
            }
            Keysym::Tab | Keysym::Right | Keysym::Down => {
                let n = self.fast.shown().len();
                if n > 0 {
                    self.fast.sel = (self.fast.sel + 1) % n;
                }
            }
            Keysym::ISO_Left_Tab | Keysym::Left | Keysym::Up => {
                let n = self.fast.shown().len();
                if n > 0 {
                    self.fast.sel = (self.fast.sel + n - 1) % n;
                }
            }
            _ => {
                if let Some(text) = utf8 {
                    let printable: String = text.chars().filter(|c| !c.is_control()).collect();
                    if !printable.is_empty() {
                        self.fast.query.push_str(&printable);
                        self.fast.sel = 0;
                        self.fast_rematch();
                    }
                }
            }
        }
        self.schedule_frame();
    }

    /// Launch: the icon `slot` if given, else the selected icon, else the
    /// best match (Enter before the search has narrowed to three).
    pub(crate) fn fast_launch_pick(&mut self, slot: Option<usize>) {
        let shown = self.fast.shown();
        let pick = slot
            .and_then(|s| shown.get(s))
            .or_else(|| shown.get(self.fast.sel))
            .or_else(|| self.fast.matches.first())
            .copied();
        let Some(idx) = pick else {
            return;
        };
        self.fast.open = false;
        // Always a NEW instance — the box's rule: you asked to launch.
        self.activate(idx, LaunchFrom::Box);
        self.close_fast_launch_after_launch();
    }

    /// After `activate` took care of the keyboard (it hands it to the app it
    /// launches), just let the bubble fade and the input go.
    fn close_fast_launch_after_launch(&mut self) {
        self.fast.open = false;
        self.sync_input_region();
        self.schedule_frame();
    }

    /// A left click while the launcher is up: an icon launches; anywhere
    /// else closes.
    pub(crate) fn fast_click(&mut self, pos: (f32, f32)) {
        let places = self.fast_geometry();
        let order = placement(self.fast.shown().len(), self.fast.sel);
        match order.iter().enumerate().find(|(p, _)| places[*p].contains(pos)) {
            Some((_, &shown_i)) => self.fast_launch_pick(Some(shown_i)),
            None if places[0].contains(pos) => {}
            None => self.close_fast_launch(),
        }
    }

    /// The slot's and the side places' rects right now.
    pub(crate) fn fast_geometry(&self) -> [Rect; 3] {
        let screen_h = self.output_logical_height().unwrap_or(self.buffer_size.1 as f32);
        geometry(
            (self.buffer_size.0 as f32, self.buffer_size.1 as f32),
            screen_h,
            self.options_scale(),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tiers_rank_prefix_word_initials_anywhere() {
        assert_eq!(match_tier("firefox", "fi"), 4);
        assert_eq!(match_tier("visual studio code", "code"), 3);
        assert_eq!(match_tier("visual studio code", "vsc"), 2);
        assert_eq!(match_tier("thunar file manager", "ile"), 1);
        assert_eq!(match_tier("spotify", "zz"), 0);
    }

    #[test]
    fn the_app_you_use_wins_a_tie_and_a_prefix_beats_usage() {
        let apps = [("Spotify", 3), ("Speedcrunch", 40), ("Inkscape", 90)];
        assert_eq!(rank(&apps, "sp"), vec![1, 0], "both start with sp: the used one first");
        assert_eq!(rank(&apps, "cape"), vec![2], "anywhere in the name still counts");
        assert_eq!(rank(&apps, "spo"), vec![0]);
        assert!(rank(&apps, "").is_empty(), "nothing typed, nothing matched");
    }

    #[test]
    fn the_slot_is_anchored_and_the_others_hang_off_it() {
        // A 1250-tall screen, the dock surface its bottom 760.
        let [main, left, right] = geometry((2000.0, 760.0), 1250.0, 1.0);
        assert!((main.y + main.h / 2.0 - (760.0 - 625.0)).abs() < 0.01, "the screen's centre");
        assert!((main.x + main.w / 2.0 - 1000.0).abs() < 0.01, "and its middle");
        assert!(left.x + left.w < main.x && right.x > main.x + main.w);
        assert_eq!(placement(3, 1), vec![1, 0, 2], "the selected one takes the slot");
        assert_eq!(placement(1, 0), vec![0]);
    }
}
