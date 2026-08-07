//! The **notification OPTION**: the bell on the topbar and its metamorphoses.
//!
//! One element, three "become more" states — the *same* shape morphs through all
//! of them (it never spawns a second surface):
//! 1. **CollapsedBell** — a resting circle just left of the clock (amber bell
//!    glyph while there are unread notifications).
//! 2. **ExtendedPreview** — the circle grows leftward into a preview pill
//!    `[icon | summary · body | time]`. Entered on hover, and **auto-entered for
//!    ~1.5 s when a new notification arrives** (then holds and collapses, like
//!    the clock↔date pill). Scroll UP steps back through history (older).
//! 3. **HistoryDrawer** — scroll DOWN and *this very pill stretches downward*,
//!    its height interpolating from the bar into a tall rounded rectangle whose
//!    lower half is the history list. The pill becomes the box.
//!
//! Data arrives from the `options-notify` daemon over D-Bus (see
//! [`crate::notifications`]); waverunner keeps its own append-only `history` so
//! dismissed notifications remain browsable. All animation is dt-based
//! (`ease_toward`) on the same frame-scheduler as the clock metamorphosis, and
//! all geometry stays within the fixed (taller) surface — Zero Layout Shift.

use std::collections::HashSet;
use std::time::{Duration, Instant};

use calloop::timer::{TimeoutAction, Timer};

use crate::animation::{ease_toward, lerp};
use crate::content::{Label, Rect, RectInst, Scene};
use crate::notifications::{ActiveNotification, NotifEvent, NotifHandle};
use crate::options::{
    push_neumorph, wash, PillId, EDGE_PAD, FONT_PX, GLYPH_BELL, GROUP_GAP, LINE_PX, NERD,
    PILL_MARGIN_Y, PILL_PAD_X,
};
use crate::App;

/// Amber for the "unread" bell glyph (Shinings "Proactive Suggestion", `#E5A93C`).
const AMBER: [f32; 4] = [0.898, 0.663, 0.235, 1.0];

/// Glide rate of the two morph progresses (exponential approach; a springier
/// curve with overshoot is a later polish pass).
const MORPH_RATE: f32 = 13.0;
const MORPH_EPS: f32 = 0.001;
/// How long the preview/history holds open after the pointer leaves, and how
/// long a new notification auto-shows before collapsing (matches the date pill).
const HOLD: Duration = Duration::from_millis(1500);
/// One wheel notch in `wl_pointer` axis units.
const NOTCH: f32 = 15.0;

/// Target width of the extended preview pill (and thus the history rectangle).
const EXTENDED_W: f32 = 280.0;
/// Target height of the fully-expanded history rectangle (fits within the
/// surface's reserved dropdown area, [`crate::OPTIONS_DROPDOWN_H`]).
const EXPANDED_H: f32 = 240.0;
/// Gap between the summary and the trailing time / between summary and body.
const TEXT_GAP: f32 = 8.0;
/// History-list row height and inner padding within the expanded rectangle.
const ROW_H: f32 = 30.0;
const LIST_PAD: f32 = 6.0;
/// Zebra striping for the history list — alternate rows get a wash so adjacent
/// lines read as distinct (old-Finder style). Direction is **adaptive**: a dark
/// box lightens its stripes, a light box darkens them, keyed off the box's own
/// luminance. Asymmetric alphas because a white wash reads stronger than a
/// black one at equal alpha (same reasoning as the pill washes).
const STRIPE_LIGHTEN: f32 = 0.31;
const STRIPE_DARKEN: f32 = 0.48;
/// Resting text opacity of the open box's lines (band + list). The whole list
/// sits muted as soon as it opens; the hovered line pops back to full contrast.
/// Lower = more muted rest / stronger hover pop.
const LIST_DIM: f32 = 0.55;

/// One notification's pre-measured render fields — a single canonical layout so
/// every row (pinned band item and list rows alike) looks identical.
struct RowInfo {
    summary: String,
    body: String,
    time: String,
    summary_w: f32,
    time_w: f32,
}

/// All notification-OPTION state (owned by [`App`] as `notif`).
pub(crate) struct NotifState {
    /// Append-only, newest first — survives dismissals (our own history).
    pub(crate) history: Vec<ActiveNotification>,
    /// Ids already folded into `history`, to detect genuinely new arrivals.
    seen: HashSet<u32>,
    /// How many notifications are currently *active* (unread) on the daemon —
    /// drives the amber bell.
    active_count: usize,
    /// Which history entry the preview shows (0 = newest).
    index: usize,
    /// Preview open (hovered, held after leave, or auto-shown on arrival).
    peek_reveal: bool,
    /// History rectangle open.
    pub(crate) expanded: bool,
    /// Morph progress: 0 bell → 1 preview pill (horizontal growth).
    peek_t: f32,
    /// Morph progress: 0 pill → 1 tall history rectangle (vertical growth).
    expand_t: f32,
    /// Vertical scroll (px) within the history list.
    list_scroll: f32,
    /// History list row under the pointer (0 = first list row below the band),
    /// for the per-row hover highlight. `None` = none / collapsed.
    hover_row: Option<usize>,
    /// Accumulated wheel delta, so one notch = one step.
    scroll_accum: f32,
    /// Pre-measured render fields for every history item, so the band (pinned
    /// top) and the list rows all render through one identical layout (widths
    /// must be measured off-frame, not in the `&self` draw).
    rows: Vec<RowInfo>,
    last: Option<Instant>,
    frame_pending: bool,
    hold_deadline: Option<Instant>,
    /// Kept alive so the worker's command channel stays open; used for
    /// dismiss/act once those gestures land.
    #[allow(dead_code)]
    handle: Option<NotifHandle>,
}

impl NotifState {
    pub(crate) fn new(handle: Option<NotifHandle>) -> Self {
        Self {
            history: Vec::new(),
            seen: HashSet::new(),
            active_count: 0,
            index: 0,
            peek_reveal: false,
            expanded: false,
            peek_t: 0.0,
            expand_t: 0.0,
            list_scroll: 0.0,
            hover_row: None,
            scroll_accum: 0.0,
            rows: Vec::new(),
            last: None,
            frame_pending: false,
            hold_deadline: None,
            handle,
        }
    }

    /// Whether the notification box currently extends below the bar — its
    /// history drawer is open or mid-animation (`expand_t` drives the only
    /// downward growth; the preview pill stays within the bar). While it does,
    /// it paints its panel over the window right where the colour-match samples
    /// the toolbar, so the sampler must pause to avoid matching the panel.
    pub(crate) fn occludes_below_bar(&self) -> bool {
        self.expand_t > 0.01
    }

    /// Whether the history drawer is open (intent), so the colour-match should
    /// sample the bar's frosted colour for the box. Uses `expanded` (set the
    /// instant the user scrolls open) rather than `expand_t` (which lags behind
    /// as it animates), so the box has its colour ready before it finishes
    /// growing.
    pub(crate) fn drawer_open(&self) -> bool {
        self.expanded
    }
}

/// Local `HH:MM` for a unix-millis timestamp, via libc (respects timezone).
fn fmt_time(ms: u64) -> String {
    if ms == 0 {
        return String::new();
    }
    // SAFETY: `localtime_r` fills a caller-owned `tm`.
    unsafe {
        let t = (ms / 1000) as libc::time_t;
        let mut tm: libc::tm = std::mem::zeroed();
        libc::localtime_r(&t, &mut tm);
        format!("{:02}:{:02}", tm.tm_hour, tm.tm_min)
    }
}

fn lerp4(a: [f32; 4], b: [f32; 4], t: f32) -> [f32; 4] {
    [
        lerp(a[0], b[0], t),
        lerp(a[1], b[1], t),
        lerp(a[2], b[2], t),
        lerp(a[3], b[3], t),
    ]
}

impl App {
    /// Fold a worker update into notification state (called from the loop).
    pub(crate) fn on_notif_event(&mut self, ev: NotifEvent) {
        let mut arrived = false;
        match ev {
            NotifEvent::Active(list) => {
                for n in list.iter().rev() {
                    if self.notif.seen.insert(n.id) {
                        self.notif.history.insert(0, n.clone());
                        arrived = true;
                    } else if let Some(slot) =
                        self.notif.history.iter_mut().find(|h| h.id == n.id)
                    {
                        *slot = n.clone(); // in-place update (replace/edit)
                    }
                }
                self.notif.active_count = list.len();
            }
            NotifEvent::Closed { .. } => {
                self.notif.active_count = self.notif.active_count.saturating_sub(1);
            }
            NotifEvent::Disconnected => {
                self.notif.active_count = 0;
            }
        }
        self.notif.index = self
            .notif
            .index
            .min(self.notif.history.len().saturating_sub(1));
        self.measure_notif();

        // A fresh notification pops the preview open for a beat — but never yank
        // the surface while the user is actively hovering or browsing it.
        let busy = self.notif.expanded || self.options_hover == Some(PillId::Notif);
        if arrived && !busy {
            self.notif_flash();
        } else if self.options_layer.is_some() {
            self.draw_options();
        }
    }

    /// Auto-show the newest notification in the preview pill, then hold for
    /// [`HOLD`] and collapse (the "pop in on arrival" behaviour).
    fn notif_flash(&mut self) {
        self.notif.index = 0;
        self.notif.expanded = false;
        self.measure_notif();
        self.notif.peek_reveal = true;
        self.notif.last = None;
        self.schedule_notif_frame();
        self.schedule_notif_collapse();
    }

    /// Re-measure every history item into [`RowInfo`]s (one canonical layout for
    /// the band and the list). Cheap; only on data change.
    pub(crate) fn measure_notif(&mut self) {
        let items: Vec<(String, String, String)> = self
            .notif
            .history
            .iter()
            .map(|n| {
                let summary = if n.summary.is_empty() {
                    n.app_name.clone()
                } else {
                    n.summary.clone()
                };
                (summary, n.body.clone(), fmt_time(n.timestamp_ms))
            })
            .collect();
        let mut rows = Vec::with_capacity(items.len());
        if let Some(r) = self.options_renderer.as_mut() {
            for (summary, body, time) in items {
                let summary_w = r.measure_text(&summary, FONT_PX, None);
                let time_w = r.measure_text(&time, FONT_PX, None);
                rows.push(RowInfo { summary, body, time, summary_w, time_w });
            }
        } else {
            for (summary, body, time) in items {
                rows.push(RowInfo { summary, body, time, summary_w: 0.0, time_w: 0.0 });
            }
        }
        self.notif.rows = rows;
    }

    /// The bar-pill height (the collapsed element's diameter / the preview band).
    fn notif_band_h(&self) -> f32 {
        (self.config.options.height as f32 - 2.0 * PILL_MARGIN_Y).max(1.0)
    }

    /// Geometry of the whole morphing element given its pinned right edge, top,
    /// and band height: width from `peek_t`, height from `expand_t` (grows down).
    pub(crate) fn notif_geom(&self, right: f32, y: f32, ph: f32) -> Rect {
        let mut w = lerp(ph, EXTENDED_W, self.notif.peek_t).max(ph);
        if right - w < EDGE_PAD {
            w = (right - EDGE_PAD).max(ph);
        }
        let h = lerp(ph, EXPANDED_H, self.notif.expand_t);
        Rect::new(right - w, y, w, h)
    }

    /// The element's current rect (from anywhere: input region, scroll clamps).
    pub(crate) fn notif_rect(&self) -> Rect {
        let ph = self.notif_band_h();
        let y = PILL_MARGIN_Y;
        let right = self.options_clock_left() - GROUP_GAP;
        self.notif_geom(right, y, ph)
    }

    /// Bottom edge (surface px) the pointer-input region must reach while the
    /// drawer is open — the *fully-expanded* box height, independent of the
    /// expand animation. Sizing the input region off the live (animating)
    /// height instead left it short until some later re-sync, so hover/scroll
    /// cut out partway down the list at an unstable row.
    pub(crate) fn notif_input_bottom(&self) -> f32 {
        PILL_MARGIN_Y + EXPANDED_H
    }

    /// The box line under the pointer — `0` = the pinned band, `1..` = list
    /// rows — or `None` (collapsed, outside the box, or off the rows). Drives
    /// the per-line hover highlight.
    fn notif_hover_row(&self) -> Option<usize> {
        if self.notif.expand_t < 0.5 {
            return None;
        }
        let p = self.options_ptr?;
        let rect = self.notif_rect();
        if p.0 < rect.x || p.0 >= rect.x + rect.w {
            return None;
        }
        let list_top = rect.y + self.notif_band_h();
        // The pinned band occupies the top band; treat it as line 0.
        if p.1 >= rect.y && p.1 < list_top {
            return Some(0);
        }
        let ry0 = list_top + LIST_PAD - self.notif.list_scroll;
        if p.1 < ry0 || p.1 >= rect.y + rect.h {
            return None;
        }
        let i = ((p.1 - ry0) / ROW_H) as usize;
        let n = self.notif.rows.len().saturating_sub(self.notif.index + 1);
        (i < n).then_some(i + 1)
    }

    /// Recompute the hovered row from the pointer; returns whether it changed
    /// (so the caller can redraw). Called on pointer motion over the bar.
    pub(crate) fn update_notif_hover_row(&mut self) -> bool {
        let new = self.notif_hover_row();
        let changed = new != self.notif.hover_row;
        self.notif.hover_row = new;
        changed
    }

    /// Draw the whole morphing element: the pill/rectangle fill, the bell glyph,
    /// the preview line (fading out as it expands), and the history list (fading
    /// in). One rounded rect — the pill literally becomes the box.
    pub(crate) fn push_notif_pill(&self, scene: &mut Scene, rect: Rect) {
        let ph = self.notif_band_h();
        let radius = ph / 2.0; // stadium ends collapsed; rounded corners expanded
        let bright = self.options_bar_is_bright();
        let e = self.notif.expand_t;

        push_neumorph(scene, rect, radius, bright, 1.0);
        // The box fill never reacts to hover — hover is per-row (the pointed
        // line's text brightens/darkens; see the list loop below). So the fill
        // always uses the resting wash, matched or frosted alike.
        let pill_base = self.options_rest_wash();
        // The open box MIMICS THE BUBBLE LIVE, opaque so text doesn't bleed
        // through. Its fill is the pill's *exact* apparent colour: the pill's
        // backdrop with the same wash composited on top — identical maths in
        // both cases, so the box is literally the pill grown. The backdrop is
        // the matched window colour when the bar is colour-matched, else the
        // bar's own sampled frosted/wallpaper shade (`options_pill_color`).
        let text_color = self.options_text_color();
        let backdrop = self.options_bar_matched.or(self.options_pill_color);
        let (expanded_fill, expanded_ink) = match backdrop {
            Some(c) => {
                // Composite the (translucent) pill wash over the backdrop so the
                // opaque box equals what the translucent pill shows.
                let a = pill_base[3];
                let blend = [
                    c[0] * (1.0 - a) + pill_base[0] * a,
                    c[1] * (1.0 - a) + pill_base[1] * a,
                    c[2] * (1.0 - a) + pill_base[2] * a,
                    1.0,
                ];
                // Matched: bar text colour already adapts to it. Frosted: adapt
                // ink to the blended shade so it stays legible on any wallpaper.
                let ink = if self.options_bar_matched.is_some() {
                    text_color
                } else {
                    let lum = 0.2126 * blend[0] + 0.7152 * blend[1] + 0.0722 * blend[2];
                    if lum > 0.179 {
                        [0.0, 0.0, 0.0, 1.0]
                    } else {
                        [0.93, 0.93, 0.96, 1.0]
                    }
                };
                (blend, ink)
            }
            // Not sampled yet (first frames after opening): neutral dark.
            None => ([0.10, 0.10, 0.12, 1.0], [0.93, 0.93, 0.96, 1.0]),
        };
        scene.rects.push(RectInst {
            rect,
            radius,
            color: lerp4(pill_base, expanded_fill, e),
            glass: 0.0,
        });

        // The bell, top-left, is the constant identity anchor. Ink tracks the
        // fill: steady when the box mimics the (already-legible) bar colour,
        // else brightening as the fallback panel darkens.
        let ink = lerp4(text_color, expanded_ink, e);
        // Per-line spotlight: the whole open box (pinned band + list) rests
        // muted; the pointed line jumps to full contrast (white on a dark box,
        // black on a light one). `hover_row` numbers lines with the band as 0
        // and list rows as 1, 2, … See `notif_hover_row`.
        let hover_line = self.notif.hover_row;
        let dark_ink = ink[0] + ink[1] + ink[2] < 1.5;
        let hover_ink = if dark_ink {
            [0.0, 0.0, 0.0, 1.0]
        } else {
            [1.0, 1.0, 1.0, 1.0]
        };
        let dim_ink = [ink[0], ink[1], ink[2], ink[3] * LIST_DIM];
        let bell_color = if self.notif.active_count > 0 { AMBER } else { ink };
        let bell_cx = rect.x + ph / 2.0;
        let band_ty = rect.y + (ph - LINE_PX) / 2.0;
        scene.labels.push(Label {
            text: GLYPH_BELL.to_owned(),
            pos: (bell_cx, band_ty),
            max_w: ph,
            font_px: FONT_PX,
            line_px: LINE_PX,
            centered: true,
            dim: false,
            cache: true,
            family: Some(NERD),
            color: Some(bell_color),
            clip: Some(rect),
        });

        let tx = rect.x + ph; // content starts just past the bell
        let right = rect.x + rect.w - PILL_PAD_X;

        // The pinned notification in the band — drawn through the SAME row
        // layout as the list, so top and rest look identical.
        let placeholder = RowInfo {
            summary: "No notifications".to_owned(),
            body: String::new(),
            time: String::new(),
            summary_w: 0.0,
            time_w: 0.0,
        };
        let band = self.notif.rows.get(self.notif.index).unwrap_or(&placeholder);
        let pa = ((self.notif.peek_t - 0.35) / 0.5).clamp(0.0, 1.0);
        if pa > 0.01 {
            let band_ink = if hover_line == Some(0) {
                hover_ink
            } else {
                // Full while it's just a preview (e≈0); dims as the box opens.
                lerp4(ink, dim_ink, e)
            };
            push_notif_row(scene, band, tx, right, band_ty, band_ink, pa, rect);
        }

        // The rest of the history reveals below the band — identical layout,
        // clipped so scrolled rows slide *under* the pinned one.
        if e > 0.01 {
            let list_top = rect.y + ph;
            let list_clip = Rect::new(
                rect.x,
                list_top,
                rect.w,
                (rect.y + rect.h - list_top).max(0.0),
            );
            // Adaptive zebra stripe colour: lighten a dark box, darken a light
            // one, off the box's own fill luminance (0.179 = the WCAG flip point
            // we use for ink too, so stripe direction matches text direction).
            let flum = 0.2126 * expanded_fill[0] + 0.7152 * expanded_fill[1] + 0.0722 * expanded_fill[2];
            let stripe = if flum <= 0.179 {
                wash(true, STRIPE_LIGHTEN)
            } else {
                wash(false, STRIPE_DARKEN)
            };
            // Keep stripes out of the box's rounded bottom corners (square rects
            // would poke past the radius over the wallpaper).
            let stripe_bot_max = rect.y + rect.h - radius;
            let mut ry = list_top + LIST_PAD - self.notif.list_scroll;
            for (i, info) in self.notif.rows.iter().skip(self.notif.index + 1).enumerate() {
                if ry + ROW_H >= list_top && ry <= rect.y + rect.h {
                    // Every other row (the band above counts as line 0) gets the
                    // wash, fading in with the expand so it can't pop.
                    if i % 2 == 0 {
                        let top = ry.max(list_top);
                        let bot = (ry + ROW_H).min(stripe_bot_max);
                        if bot > top {
                            scene.rects.push(RectInst {
                                rect: Rect::new(rect.x, top, rect.w, bot - top),
                                radius: 0.0,
                                color: [stripe[0], stripe[1], stripe[2], stripe[3] * e],
                                glass: 0.0,
                            });
                        }
                    }
                    // List rows are lines 1, 2, … (band is line 0). Muted at rest,
                    // full contrast when hovered.
                    let row_ink = if hover_line == Some(i + 1) { hover_ink } else { dim_ink };
                    push_notif_row(scene, info, tx, right, ry + (ROW_H - LINE_PX) / 2.0, row_ink, e, list_clip);
                }
                ry += ROW_H;
            }
        }
    }

    /// Max vertical scroll of the history list within the expanded rectangle.
    fn notif_list_max_scroll(&self) -> f32 {
        let ph = self.notif_band_h();
        let visible = (EXPANDED_H - ph - 2.0 * LIST_PAD).max(0.0);
        // The list starts after the banded notification (index).
        let remaining = self.notif.history.len().saturating_sub(self.notif.index + 1);
        let content = remaining as f32 * ROW_H;
        (content - visible).max(0.0)
    }

    /// A wheel event over the notification OPTION (raw axis value). Accumulates
    /// to whole notches, then steps. Negated so a natural "scroll down" gesture
    /// expands the history (and scroll up browses older / collapses).
    pub(crate) fn notif_axis(&mut self, value: f32) {
        self.notif.scroll_accum += -value;
        while self.notif.scroll_accum <= -NOTCH {
            self.notif.scroll_accum += NOTCH;
            self.notif_step(true); // up
        }
        while self.notif.scroll_accum >= NOTCH {
            self.notif.scroll_accum -= NOTCH;
            self.notif_step(false); // down
        }
    }

    /// One scroll step. `up` = scroll up (older / collapse), else down (expand /
    /// scroll the list).
    fn notif_step(&mut self, up: bool) {
        self.notif.hold_deadline = None; // a scroll keeps it open
        if self.notif.expanded {
            if up {
                if self.notif.list_scroll <= 0.5 {
                    self.notif.expanded = false; // collapse back to the preview pill
                } else {
                    self.notif.list_scroll = (self.notif.list_scroll - ROW_H).max(0.0);
                }
            } else {
                self.notif.list_scroll =
                    (self.notif.list_scroll + ROW_H).min(self.notif_list_max_scroll());
            }
        } else if up {
            if self.notif.index + 1 < self.notif.history.len() {
                self.notif.index += 1;
                self.measure_notif();
            }
        } else {
            // Preview + scroll down → the pill stretches into the history box.
            self.notif.expanded = true;
            self.notif.list_scroll = 0.0;
        }
        self.sync_options_input();
        // Opening/closing the drawer changes whether we sample the bar's frosted
        // colour for the box — re-evaluate now so the colour is ready as it
        // grows, rather than waiting up to a full poll tick.
        self.reeval_options_bar();
        self.schedule_notif_frame();
    }

    /// Recompute whether the bell should preview, and manage the hold/collapse
    /// after the pointer leaves (mirrors the clock metamorphosis).
    pub(crate) fn update_notif_reveal(&mut self) {
        let on = self.options_hover == Some(PillId::Notif);
        if on {
            self.notif.hold_deadline = None;
            if !self.notif.peek_reveal {
                self.notif.peek_reveal = true;
                self.notif.last = None;
                self.schedule_notif_frame();
            }
        } else if (self.notif.peek_reveal || self.notif.expanded)
            && self.notif.hold_deadline.is_none()
        {
            self.schedule_notif_collapse();
        }
    }

    /// After the pointer leaves (or an auto-show ends), hold briefly then
    /// collapse the preview + history.
    fn schedule_notif_collapse(&mut self) {
        let deadline = Instant::now() + HOLD;
        self.notif.hold_deadline = Some(deadline);
        let timer = Timer::from_duration(HOLD);
        let _ = self.loop_handle.insert_source(timer, move |_, _, app: &mut App| {
            if app.notif.hold_deadline == Some(deadline) {
                app.notif.hold_deadline = None;
                if app.options_hover != Some(PillId::Notif) {
                    app.notif.peek_reveal = false;
                    app.notif.expanded = false;
                    app.notif.index = 0;
                    app.notif.list_scroll = 0.0;
                    app.notif.last = None;
                    app.sync_options_input();
                    app.schedule_notif_frame();
                }
            }
            TimeoutAction::Drop
        });
    }

    fn schedule_notif_frame(&mut self) {
        if self.notif.frame_pending {
            return;
        }
        self.notif.frame_pending = true;
        if self.notif.last.is_none() {
            self.notif.last = Some(Instant::now());
        }
        let timer = Timer::from_duration(Duration::from_millis(8));
        let _ = self.loop_handle.insert_source(timer, |_, _, app: &mut App| {
            app.notif.frame_pending = false;
            app.tick_notif();
            TimeoutAction::Drop
        });
    }

    /// Advance both morph progresses one frame; width, crossfade, and height all
    /// derive from these at draw time.
    fn tick_notif(&mut self) {
        let now = Instant::now();
        let dt = self
            .notif
            .last
            .map_or(0.0, |l| now.duration_since(l).as_secs_f32().min(0.05));
        self.notif.last = Some(now);

        let peek_target = if self.notif.peek_reveal || self.notif.expanded {
            1.0
        } else {
            0.0
        };
        let expand_target = if self.notif.expanded { 1.0 } else { 0.0 };
        let (pt, pm) = ease_toward(self.notif.peek_t, peek_target, dt, MORPH_RATE, MORPH_EPS);
        let (et, em) = ease_toward(self.notif.expand_t, expand_target, dt, MORPH_RATE, MORPH_EPS);
        self.notif.peek_t = pt;
        self.notif.expand_t = et;

        self.draw_options();
        if pm || em {
            self.schedule_notif_frame();
        } else {
            self.notif.last = None;
        }
    }
}

/// Draw one notification row in the canonical layout — `summary` (primary),
/// `body` (dim) sharing the line, `time` (dim) trailing at the right. Shared by
/// the pinned band item and every list row so they render identically. `alpha`
/// fades the whole row (the band by its peek, the list by the expand).
#[allow(clippy::too_many_arguments)]
fn push_notif_row(
    scene: &mut Scene,
    info: &RowInfo,
    tx: f32,
    right: f32,
    ty: f32,
    ink: [f32; 4],
    alpha: f32,
    clip: Rect,
) {
    let prim = [ink[0], ink[1], ink[2], ink[3] * alpha];
    let dim = [ink[0], ink[1], ink[2], ink[3] * 0.55 * alpha];
    let mut content_right = right;
    if !info.time.is_empty() {
        let time_x = (right - info.time_w).max(tx);
        content_right = time_x - TEXT_GAP;
        scene
            .labels
            .push(mk_line(info.time.clone(), time_x, ty, info.time_w + 2.0, dim, clip));
    }
    let sum_max = (content_right - tx).max(0.0);
    scene
        .labels
        .push(mk_line(info.summary.clone(), tx, ty, sum_max, prim, clip));
    let bx = tx + info.summary_w.min(sum_max) + TEXT_GAP;
    if !info.body.is_empty() && bx < content_right {
        scene
            .labels
            .push(mk_line(info.body.clone(), bx, ty, content_right - bx, dim, clip));
    }
}

/// A left-anchored, clipped single-line label (the notif element's text rows).
fn mk_line(text: String, x: f32, y: f32, max_w: f32, color: [f32; 4], clip: Rect) -> Label {
    Label {
        text,
        pos: (x, y),
        max_w,
        font_px: FONT_PX,
        line_px: LINE_PX,
        centered: false,
        dim: false,
        cache: false,
        family: None,
        color: Some(color),
        clip: Some(clip),
    }
}
