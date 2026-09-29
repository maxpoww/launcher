//! The main card's two doors on the dock: the Apps button, which opens it on
//! the apps grid (macOS's Launchpad), and Golem's configuration panel, which
//! opens it empty.
//!
//! OPTIONS offers the right control at the right moment, but it can't guess
//! every one (a screen resolution, a scale). The panel is the place to go for
//! the rest: a gear among the apps ([`crate::apps::SETTINGS_ID`]) that opens
//! the same card the apps live in, with the sections and search cleared away
//! so the card itself is the surface. The dock band stays, so the gear that
//! opened the panel is the one that closes it.

use std::f32::consts::TAU;

use tracing::info;

use crate::content::{self, Layout, Rect};
use crate::options::{FONT_PX, LINE_PX, OPTION_GAP, PILL_PAD_X, TEXT_FONT};
use crate::state::Target;
use crate::{apps, groups, App};
use waverunner_proto::Command;

/// The panel's settings, for now: placeholder labels to see how a field of
/// bubbles reads. Each will become a real setting.
const DEMO_SETTINGS: [&str; 20] = [
    "Resolution",
    "Scale",
    "Night light",
    "Wi-Fi",
    "Bluetooth",
    "Sound output",
    "Volume",
    "Brightness",
    "Dark mode",
    "Accent colour",
    "Wallpaper",
    "Keyboard layout",
    "Touchpad speed",
    "Natural scrolling",
    "Language",
    "Time zone",
    "Power mode",
    "Notifications",
    "Updates",
    "About Golem",
];

/// How far a bubble drifts from its spot, base px (scaled like the bar).
const FLOAT_AMP: f32 = 2.5;
/// Extra air between bubbles beyond the bar's own gap between OPTIONS, base
/// px (scaled like the bar): how spread the semicircle is. Tune here.
const SPREAD: f32 = 26.0;
/// Gravity packing: the search for a free spot walks outward from the
/// centre in rings this far apart (px), each sampled every this many px of
/// its circumference — fine enough that bubbles settle snug, not gridded.
const RING_STEP: f32 = 3.0;
const RING_SAMPLE: f32 = 6.0;

/// Where the bubbles sit in the field, computed once per field size and
/// bar scale.
pub(crate) struct Scatter {
    /// The field size and bar scale the spots were computed for.
    key: (f32, f32, f32),
    /// Each bubble's resting rect, relative to the field's top-left.
    spots: Vec<Rect>,
    /// Per bubble: (x period, y period, x phase, y phase) of its drift.
    drift: Vec<[f32; 4]>,
}

/// A tiny deterministic generator: the field looks the same every time it
/// opens (and across restarts) instead of reshuffling under the eye.
struct Rng(u32);

impl Rng {
    fn next(&mut self) -> f32 {
        // xorshift32
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 17;
        self.0 ^= self.0 << 5;
        (self.0 >> 8) as f32 / (1u32 << 24) as f32
    }
}

/// Signed air between two rects: positive = the gap, negative = overlap.
fn air(a: Rect, b: Rect) -> f32 {
    let sx = (a.x - (b.x + b.w)).max(b.x - (a.x + a.w));
    let sy = (a.y - (b.y + b.h)).max(b.y - (a.y + a.h));
    sx.max(sy)
}

/// A bubble's (font px, line px) at bar scale `scale`: the OPTIONS pill's.
pub(crate) fn bubble_font(scale: f32) -> (f32, f32) {
    (FONT_PX * scale, LINE_PX * scale)
}

/// Gather `sizes` in a semicircle hanging from the middle of a `field`'s
/// top edge — the bottom edge of the dock above it, the heap's centre of
/// gravity. Widest first, each takes the free spot nearest that point by
/// true distance (so the order is ruled by one radius: a semicircle), with
/// `gap` of air to every neighbour and `margin` from the field's edges. A
/// random offset per bubble along each ring keeps the heap organic instead
/// of mirrored.
fn gather(sizes: &[(f32, f32)], field: (f32, f32), gap: f32, margin: f32) -> Vec<Rect> {
    let mut rng = Rng(0x9E37_79B9);
    let mut order: Vec<usize> = (0..sizes.len()).collect();
    order.sort_by(|&a, &b| sizes[b].0.total_cmp(&sizes[a].0));
    let (ox, oy) = (field.0 / 2.0, margin);
    let reach = field.0.hypot(field.1);
    let mut placed: Vec<Option<Rect>> = vec![None; sizes.len()];
    let mut done: Vec<Rect> = Vec::with_capacity(sizes.len());
    for i in order {
        let (w, h) = sizes[i];
        let off = rng.next();
        let fits = |r: Rect| {
            r.x >= margin
                && r.y >= margin
                && r.x + r.w <= field.0 - margin
                && r.y + r.h <= field.1 - margin
                && done.iter().all(|&d| air(r, d) >= gap)
        };
        // The bubble's own centre walks the rings: its nearest edge to the
        // origin is its top-middle, so hang it from there — the first ring
        // (radius 0) seats the first bubble right under the dock's middle.
        let mut spot = None;
        let mut radius = 0.0f32;
        'rings: while radius <= reach {
            // The lower half only: angle 0..π sweeps right → down → left.
            let samples = ((std::f32::consts::PI * radius / RING_SAMPLE).ceil() as usize).max(1);
            for k in 0..samples {
                let a = std::f32::consts::PI * ((k as f32 + off) / samples as f32);
                let (px, py) = (ox + radius * a.cos(), oy + radius * a.sin());
                let r = Rect::new(px - w / 2.0, py, w, h);
                if fits(r) {
                    spot = Some(r);
                    break 'rings;
                }
            }
            radius += RING_STEP;
        }
        // No room left anywhere (a tiny field): stack it at the origin
        // rather than drop a setting.
        let r = spot.unwrap_or(Rect::new(ox - w / 2.0, oy, w, h));
        placed[i] = Some(r);
        done.push(r);
    }
    placed.into_iter().map(Option::unwrap_or_default).collect()
}

impl App {
    /// One-shot: pin the Settings gear to the dock, just before the Recycle
    /// Bin so the bin keeps its place. Same marker rule as `pin_trash_once`: a
    /// user who later unpins or moves it isn't overruled on the next start.
    pub(crate) fn pin_settings_once(&mut self) {
        let trash = format!("group:{}", groups::TRASH_ID);
        let slot = self
            .pins
            .pins()
            .iter()
            .position(|p| *p == trash)
            .unwrap_or(self.pins.pins().len());
        self.pin_once(apps::SETTINGS_ID, "settings-pinned", slot);
    }

    /// One-shot: pin the Apps button first on the dock, where Launchpad sits
    /// on a Mac. Same marker rule as [`Self::pin_settings_once`].
    pub(crate) fn pin_apps_grid_once(&mut self) {
        self.pin_once(apps::APPS_GRID_ID, "apps-grid-pinned", 0);
    }

    /// Pin `id` at `slot` unless the `marker` says it was done before.
    fn pin_once(&mut self, id: &str, marker: &str, slot: usize) {
        let marker_path = crate::persist::data_path(marker);
        if marker_path.exists() {
            return;
        }
        if !self.pins.is_pinned(id) {
            self.pins.pin_at(id, slot);
        }
        crate::persist::write_text(marker, &marker_path, "1\n");
    }

    /// The Apps button was clicked: open the card on the apps grid, or close
    /// it if the apps are what's showing. Over the settings panel the apps
    /// come back in place.
    pub(crate) fn toggle_apps_grid(&mut self) {
        self.close_group();
        if self.ui.target() == Target::Open {
            if self.settings_panel {
                info!("apps: in place of the settings panel");
                self.settings_panel = false;
                self.schedule_frame();
            } else {
                info!("apps: closing the grid");
                self.handle_command(Command::Collapse);
            }
            return;
        }
        info!("apps: opening the grid");
        self.handle_command(Command::Toggle);
    }

    /// The gear was clicked: open the card as the panel, or close it if the
    /// panel is what's showing. With the card already open on the apps, the
    /// sections clear in place.
    pub(crate) fn toggle_settings_panel(&mut self) {
        self.close_group();
        if self.ui.target() == Target::Open {
            if self.settings_panel {
                info!("settings: closing the panel");
                self.handle_command(Command::Collapse);
            } else {
                info!("settings: panel in place of the apps");
                self.settings_panel = true;
                self.search.open = false;
                self.schedule_frame();
            }
            return;
        }
        info!("settings: opening the panel");
        self.settings_opening = true;
        self.handle_command(Command::Toggle);
        // Refused (e.g. the dock is suppressed): don't let a later open
        // come up as the panel.
        self.settings_opening = false;
    }

    /// The panel's bubbles for this frame: each label at its spot in the
    /// semicircle hanging from the dock, drifting on its own slow loop.
    pub(crate) fn panel_bubbles(&mut self, layout: &Layout) -> Vec<(Rect, &'static str)> {
        // Clones of the OPTIONS pills: the bar's scale, pill height, font,
        // padding and gap — so a bubble here is a pill from up there.
        let scale = self.options_scale();
        let apps = &layout.sections[content::SECTION_APPS];
        let top = apps.title_pos.1;
        let field = Rect::new(
            apps.viewport.x,
            top,
            apps.viewport.w,
            (layout.search_box.y + layout.search_box.h - top).max(0.0),
        );
        let size = (field.w, field.h);
        let key = (size.0, size.1, scale);
        let stale = self.panel_scatter.as_ref().is_none_or(|s| {
            (s.key.0 - key.0).abs() > 0.5
                || (s.key.1 - key.1).abs() > 0.5
                || (s.key.2 - key.2).abs() > 0.001
        });
        if stale {
            let h = self.options_pill_h();
            let (font_px, _) = crate::panel::bubble_font(scale);
            // Measured by the real text shaper, as the bar measures its own.
            let Some(r) = self.renderer.as_mut() else {
                return Vec::new();
            };
            let sizes: Vec<(f32, f32)> = DEMO_SETTINGS
                .iter()
                .map(|t| {
                    let w = r.measure_text(crate::i18n::tr(t), font_px, TEXT_FONT)
                        + 2.0 * PILL_PAD_X;
                    (w.max(h), h)
                })
                .collect();
            let amp = FLOAT_AMP * scale;
            // The bar's gap between distinct OPTIONS and the panel's own
            // spread, plus the room two neighbours need to drift toward
            // each other without touching.
            let gap = (OPTION_GAP + SPREAD) * scale + 2.0 * amp;
            let spots = gather(&sizes, size, gap, amp);
            let mut rng = Rng(0x85EB_CA6B);
            let drift = (0..spots.len())
                .map(|_| {
                    [
                        5.0 + rng.next() * 4.0,
                        4.0 + rng.next() * 3.0,
                        rng.next() * TAU,
                        rng.next() * TAU,
                    ]
                })
                .collect();
            self.panel_scatter = Some(Scatter { key, spots, drift });
        }
        let Some(sc) = &self.panel_scatter else {
            return Vec::new();
        };
        let t = self.panel_clock;
        let amp = FLOAT_AMP * scale;
        sc.spots
            .iter()
            .zip(&sc.drift)
            .zip(DEMO_SETTINGS)
            .map(|((r, d), label)| {
                let dx = amp * 0.6 * (t * TAU / d[0] + d[2]).sin();
                let dy = amp * (t * TAU / d[1] + d[3]).sin();
                (
                    Rect::new(field.x + r.x + dx, field.y + r.y + dy, r.w, r.h),
                    crate::i18n::tr(label),
                )
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The demo set at bar scale 1 (estimated widths — no shaper in tests).
    fn demo_sizes() -> Vec<(f32, f32)> {
        let h = 25.0;
        DEMO_SETTINGS
            .iter()
            .map(|t| {
                let w = crate::options::est_text_w(t, FONT_PX) + 2.0 * PILL_PAD_X;
                (w.max(h), h)
            })
            .collect()
    }

    #[test]
    fn gather_hangs_a_semicircle_from_the_top_middle_without_touching() {
        let sizes = demo_sizes();
        let field = (1200.0, 560.0);
        let gap = OPTION_GAP + SPREAD + 2.0 * FLOAT_AMP;
        let spots = gather(&sizes, field, gap, FLOAT_AMP);
        assert_eq!(spots.len(), sizes.len());
        for (i, r) in spots.iter().enumerate() {
            assert!(
                r.x >= FLOAT_AMP - 0.01
                    && r.y >= FLOAT_AMP - 0.01
                    && r.x + r.w <= field.0 - FLOAT_AMP + 0.01
                    && r.y + r.h <= field.1 - FLOAT_AMP + 0.01,
                "bubble {i} leaves the field: {r:?}"
            );
            for (j, o) in spots.iter().enumerate().skip(i + 1) {
                assert!(air(*r, *o) >= gap - 0.01, "bubbles {i} and {j} touch: {r:?} {o:?}");
            }
        }
        // A semicircle hanging from the middle of the top edge: the heap
        // starts right at the top, is centred on the middle, and reaches
        // down less far than across (half a disc, not a column).
        let origin = (field.0 / 2.0, FLOAT_AMP);
        let (x0, y0, x1, y1) = spots.iter().fold(
            (f32::MAX, f32::MAX, f32::MIN, f32::MIN),
            |(a, b, c, d), r| (a.min(r.x), b.min(r.y), c.max(r.x + r.w), d.max(r.y + r.h)),
        );
        assert!(y0 <= FLOAT_AMP + 0.01, "the heap hangs from the top edge (top at {y0})");
        assert!(
            ((x0 + x1) / 2.0 - origin.0).abs() < 60.0,
            "heap centred at x {} (want {})",
            (x0 + x1) / 2.0,
            origin.0
        );
        assert!(
            y1 - y0 < x1 - x0,
            "heap {}×{}: deeper than wide is no semicircle",
            x1 - x0,
            y1 - y0
        );
    }
}
