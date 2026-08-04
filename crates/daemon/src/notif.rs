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
    push_neumorph, PillId, EDGE_PAD, FONT_PX, GLYPH_BELL, GROUP_GAP, LINE_PX, NERD, PILL_MARGIN_Y,
    PILL_PAD_X,
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

    /// Draw the whole morphing element: the pill/rectangle fill, the bell glyph,
    /// the preview line (fading out as it expands), and the history list (fading
    /// in). One rounded rect — the pill literally becomes the box.
    pub(crate) fn push_notif_pill(&self, scene: &mut Scene, rect: Rect) {
        let ph = self.notif_band_h();
        let radius = ph / 2.0; // stadium ends collapsed; rounded corners expanded
        let bright = self.options_bar_is_bright();
        let hovered = self.options_hover == Some(PillId::Notif);
        let e = self.notif.expand_t;

        push_neumorph(scene, rect, radius, bright, 1.0);
        // Fill morphs from the subtle pill wash to a readable dark panel as it
        // grows down over the desktop (below the bar there's no fill behind it).
        let pill_base = if hovered {
            self.options_hover_wash()
        } else {
            self.options_rest_wash()
        };
        // The open box MIMICS THE BUBBLE LIVE: as it expands it fills with the
        // bubble's own colour — the live bar-matched window colour with the pill
        // wash composited on top — made opaque so it reads over the desktop
        // without text bleeding through. So the drawer looks like the bubble
        // grown, tracking the window colour in real time, not a separate dark
        // panel. With no colour match (transparent bar) fall back to a readable
        // dark panel.
        let text_color = self.options_text_color();
        let (expanded_fill, expanded_ink) = match self.options_bar_matched {
            Some(c) => {
                // Composite the (translucent) pill wash over the matched colour
                // so the opaque box equals what the translucent bubble shows.
                let a = pill_base[3];
                let blend = [
                    c[0] * (1.0 - a) + pill_base[0] * a,
                    c[1] * (1.0 - a) + pill_base[1] * a,
                    c[2] * (1.0 - a) + pill_base[2] * a,
                    1.0,
                ];
                (blend, text_color) // bar colour is already legible for text_color
            }
            None => ([0.11, 0.11, 0.14, 1.0], [0.93, 0.93, 0.96, 1.0]),
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
            push_notif_row(scene, band, tx, right, band_ty, ink, pa, rect);
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
            let mut ry = list_top + LIST_PAD - self.notif.list_scroll;
            for info in self.notif.rows.iter().skip(self.notif.index + 1) {
                if ry + ROW_H >= list_top && ry <= rect.y + rect.h {
                    push_notif_row(scene, info, tx, right, ry + (ROW_H - LINE_PX) / 2.0, ink, e, list_clip);
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
    let dim = [ink[0], ink[1], ink[2], 0.55 * alpha];
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
