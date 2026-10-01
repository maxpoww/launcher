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

/// The bubble and its icons, in logical px at bar scale 1 (scaled by the
/// OPTIONS scale).
const BUBBLE_H: f32 = 46.0;
const BUBBLE_MIN_W: f32 = 240.0;
const BUBBLE_PAD_X: f32 = 22.0;
pub(crate) const FONT_PX: f32 = 22.0;
pub(crate) const LINE_PX: f32 = 28.0;
const ICON: f32 = 56.0;
const ICON_GAP: f32 = 14.0;
/// Space between the bubble and the icons beside it.
const ICON_AIR: f32 = 18.0;
/// How fast the bubble and the icons ease in (1/s): snappy.
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

/// Where the bubble and its icons sit on the dock surface: the bubble's
/// centre is the screen's centre (or as near as the surface reaches), and
/// the icons follow it on the right, centred as a group with the bubble.
/// `text_w` is the shaped width of what the bubble shows.
pub(crate) fn geometry(surface: (f32, f32), screen_h: f32, scale: f32, text_w: f32, n: usize) -> (Rect, Vec<Rect>) {
    let (w, h) = surface;
    let bh = BUBBLE_H * scale;
    let bw = (text_w + 2.0 * BUBBLE_PAD_X * scale).max(BUBBLE_MIN_W * scale);
    let icon = ICON * scale;
    let icons_w = if n == 0 {
        0.0
    } else {
        ICON_AIR * scale + n as f32 * icon + (n - 1) as f32 * ICON_GAP * scale
    };
    // The surface is anchored to the screen's bottom edge: the screen's
    // centre is `screen_h / 2` above that edge.
    let cy = (h - screen_h / 2.0).clamp(bh, h - bh);
    let x0 = (w - bw - icons_w) / 2.0;
    let bubble = Rect::new(x0, cy - bh / 2.0, bw, bh);
    let icons = (0..n)
        .map(|i| {
            let x = x0 + bw + ICON_AIR * scale + i as f32 * (icon + ICON_GAP * scale);
            Rect::new(x, cy - icon / 2.0, icon, icon)
        })
        .collect();
    (bubble, icons)
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

    /// A left click while the bubble is open: an icon launches; anywhere
    /// else closes.
    pub(crate) fn fast_click(&mut self, pos: (f32, f32)) {
        let (_, icons) = self.fast_geometry();
        match icons.iter().position(|r| r.contains(pos)) {
            Some(i) => self.fast_launch_pick(Some(i)),
            None => {
                let (bubble, _) = self.fast_geometry();
                if !bubble.contains(pos) {
                    self.close_fast_launch();
                }
            }
        }
    }

    /// The bubble's and the icons' rects right now.
    pub(crate) fn fast_geometry(&mut self) -> (Rect, Vec<Rect>) {
        let scale = self.options_scale();
        let text = if self.fast.query.is_empty() { crate::i18n::tr("Launch") } else { self.fast.query.as_str() };
        let text_w = self
            .renderer
            .as_mut()
            .map(|r| r.measure_text(text, FONT_PX * scale, crate::options::TEXT_FONT))
            .unwrap_or(text.len() as f32 * FONT_PX * scale * 0.55);
        let screen_h = self.output_logical_height().unwrap_or(self.buffer_size.1 as f32);
        geometry(
            (self.buffer_size.0 as f32, self.buffer_size.1 as f32),
            screen_h,
            scale,
            text_w,
            self.fast.shown().len(),
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
    fn the_bubble_sits_mid_screen_with_its_icons_beside_it() {
        // A 1250-tall screen, the dock surface its bottom 760.
        let (b, icons) = geometry((2000.0, 760.0), 1250.0, 1.0, 100.0, 3);
        assert!((b.y + b.h / 2.0 - (760.0 - 625.0)).abs() < 0.01, "the screen's centre");
        assert_eq!(icons.len(), 3);
        assert!(icons[0].x > b.x + b.w);
        let span = icons[2].x + icons[2].w - b.x;
        assert!((b.x + span / 2.0 - 1000.0).abs() < 0.5, "bubble + icons centred together");
    }
}
