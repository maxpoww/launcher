//! The **notification OPTION**: the bell on the topbar and its metamorphoses.
//!
//! One element, three "become more" states — the *same* shape morphs through all
//! of them (it never spawns a second surface):
//! 1. **CollapsedBell** — a resting circle just left of the clock (amber bell
//!    glyph while there are unread notifications).
//! 2. **ExtendedPreview** — the circle grows leftward into a preview pill
//!    `[summary · body | time]` of the **newest** notification. Entered on hover,
//!    and **auto-entered for ~1.5 s when a new notification arrives** (then holds
//!    and collapses, like the clock↔date pill). A scroll (either direction) opens
//!    the box.
//! 3. **HistoryDrawer** — the pill stretches downward into a tall rounded
//!    rectangle: the top card **height-morphs** from the one-liner up to a full
//!    card while the rest of the history stacks below it. The box always opens
//!    with the newest flush at the top; `list_scroll` is a single offset over the
//!    whole history (newest first), always snapped to a card boundary so the top
//!    card is never sliced and the order can never invert. Scroll direction is
//!    consistent: up → newer, down → older.
//!
//! Data arrives from the `options-notify` daemon over D-Bus (see
//! [`crate::notifications`]); waverunner keeps its own append-only `history` so
//! dismissed notifications remain browsable, and **persists it to disk** so the
//! list survives reboots — it is reloaded on startup and grows unbounded until
//! the user erases it (an erase gesture is a later pass). All animation is dt-based
//! (`ease_toward`) on the same frame-scheduler as the clock metamorphosis, and
//! all geometry stays within the fixed (taller) surface — Zero Layout Shift.

use std::collections::{HashMap, HashSet};
use std::time::{Duration, Instant};

use calloop::timer::{TimeoutAction, Timer};

use crate::animation::{ease_toward, lerp};
use crate::content::{GridContent, IconInst, Label, Rect, RectInst, Scene, ShadowInst};
use crate::notifications::{
    action_pairs, ActiveNotification, NotifCommand, NotifEvent, NotifHandle,
};
use crate::options::{
    hover_grow, push_neumorph, wash, PillId, BOND_GAP, EDGE_PAD, FONT_PX, GLYPH_BELL,
    GLYPH_BELL_SLASH, LINE_PX, NERD, OPTION_GAP, PILL_MARGIN_Y, PILL_PAD_X,
};
use crate::App;

/// Comfortable inner padding of a history card (matches the browser mockup —
/// no compact mode). Horizontal on both sides; vertical top and bottom.
const CARD_PAD_X: f32 = 14.0;
const CARD_PAD_Y: f32 = 11.0;
/// Right inset for the trailing time / dismiss can — tighter than `CARD_PAD_X`
/// so they hug the card's top-right corner (matches the clipboard list).
const TRAIL_PAD_X: f32 = 6.0;
/// The app-identity tile at the card's top-left — a real avatar (circular) /
/// app icon, or the initial-letter monogram fallback.
const ICON_SZ: f32 = 40.0;
const ICON_GAP: f32 = 11.0;
/// Body wraps to at most this many lines; the overflow is clipped (an
/// interactive "show more" is a later pass).
const MAX_BODY_LINES: usize = 4;
/// Gap between the header line and the wrapped body.
const BODY_GAP: f32 = 3.0;
/// Per-card control (×) hot-square and the gap to the header text.
const CTRL_SZ: f32 = 18.0;
const CTRL_GAP_N: f32 = 6.0;
/// fa-trash-o (outline can with vertical lines) — the per-card delete control.
const GLYPH_TRASH: &str = "\u{f014}";

/// Crimson for destructive controls (× dismiss), Shinings "Live" `#E05252`.
const CRIMSON: [f32; 4] = [0.878, 0.322, 0.322, 1.0];
/// Unread-indicator colour — the bell-pill tint/glow and the "+N" count badge.
/// Matched to the user's Hyprland active window-border colour (`#ffbe98`, a warm
/// peach) so the unread cue speaks the same accent as the rest of the desktop.
const AMBER: [f32; 4] = [1.0, 0.745, 0.596, 1.0];
/// The battery alarm's red (the bell accent while low/beating/critical).
const BATTERY_RED: [f32; 4] = [0.92, 0.26, 0.21, 1.0];
/// fa-triangle-exclamation — the battery awareness symbol.
const GLYPH_BATTERY_WARN: &str = "\u{f071}";

/// Glide rate of the two morph progresses (exponential approach; a springier
/// curve with overshoot is a later polish pass).
const MORPH_RATE: f32 = 13.0;
const MORPH_EPS: f32 = 0.001;
/// How long the preview/history holds open after the pointer leaves, and how
/// long a new notification auto-shows before collapsing (matches the date pill).
const HOLD: Duration = Duration::from_millis(1500);
/// Shorter grace before collapsing once the pointer *leaves* — snappy to hide,
/// but enough to survive crossing a small gap between the pill and mute pill.
const LEAVE_HOLD: Duration = Duration::from_millis(300);
/// After "Clear all", how long the box lingers on the empty state before it
/// falls away on its own (even if the pointer is still over it).
const EMPTY_HOLD: Duration = Duration::from_millis(950);
/// While muted (DND on), a fresh notification beats the bell pill for this long
/// instead of popping the preview — a silent visual cue. Kept at exactly one
/// [`BLINK_PERIOD`] so it's a single slow heartbeat (swell + settle).
const BLINK_DURATION: Duration = Duration::from_millis(500);
/// One heartbeat of the muted-arrival pill beat (swell + settle).
const BLINK_PERIOD: Duration = Duration::from_millis(500);
/// One wheel notch in `wl_pointer` axis units — the travel that opens the box
/// from the collapsed preview.
const NOTCH: f32 = 15.0;
/// Pixels of list scroll per axis unit (so one wheel notch ≈ `NOTCH * SCROLL_SPEED`
/// px of travel). Tunable for scroll feel.
const SCROLL_SPEED: f32 = 3.0;
/// Exponential approach rate of `list_scroll` toward `scroll_target` — higher is
/// snappier, lower is floatier. This is what makes the wheel feel smooth rather
/// than stepping a whole card at a time.
const SCROLL_RATE: f32 = 20.0;

/// Target width of the extended preview pill (and thus the history rectangle).
/// Kept narrow so the open box reads as a portrait phone-style notification
/// shade (clearly taller than wide) rather than a squat panel.
const EXTENDED_W: f32 = 380.0;
/// Target height of the fully-expanded history rectangle (fits within the
/// surface's reserved dropdown area, [`crate::OPTIONS_DROPDOWN_H`]).
const EXPANDED_H: f32 = 505.0;
/// Height of the open box when there are no notifications — a small panel just
/// tall enough to hold the centred "No notifications" message.
const EMPTY_H: f32 = 120.0;
/// Gap between the summary and the trailing time / between summary and body.
const TEXT_GAP: f32 = 8.0;
/// Bottom padding below the last card before scrolling stops.
const LIST_PAD: f32 = 6.0;
/// The box's corner radius once fully open. The collapsed element is a full
/// stadium/circle (`ph/2`); it eases to this gentler radius as it expands so the
/// edge-to-edge zebra stripes can round to the box outline without ballooning
/// into pills.
const BOX_RADIUS: f32 = 10.0;
/// Zebra striping for the history list — alternate rows get a wash so adjacent
/// lines read as distinct (old-Finder style). Direction is **adaptive**: a dark
/// box lightens its stripes, a light box darkens them, keyed off the box's own
/// luminance. Asymmetric alphas because a white wash reads stronger than a
/// black one at equal alpha (same reasoning as the pill washes).
const STRIPE_LIGHTEN: f32 = 0.31;
const STRIPE_DARKEN: f32 = 0.48;
/// Resting text opacity of the open box's lines (band + list). The whole list
/// sits muted as soon as it opens; the hovered line pops back to full contrast.
/// Lower = more muted rest / stronger hover pop. `LIST_DIM` is tuned for dark
/// boxes (light ink); light boxes (dark ink) use the stronger `LIST_DIM_LIGHT`
/// so dark-on-light text stays legible at the same perceived muting.
const LIST_DIM: f32 = 0.55;
const LIST_DIM_LIGHT: f32 = 0.82;

/// How recently a matching message must have been surfaced for a new arrival to
/// count as its *echo* (same chat mirrored by the webapp + KDE Connect) and skip
/// the second preview pop. Comfortably covers the seconds-apart cross-source gap
/// without swallowing genuinely new messages.
const DUP_WINDOW_MS: u64 = 20_000;

/// On-disk store for the browsable history (in the daemon's XDG data dir, next
/// to the other stores). Reloaded on startup so notifications survive reboots.
/// Grows unbounded by design — nothing auto-prunes; the user decides when to
/// erase.
const HISTORY_FILE: &str = "notif-history.json";

/// On-disk store for the OPTION's *interaction* state — DND (mute) and read-state
/// — so the whole thing looks identical after a waverunner restart or a reboot.
const PREFS_FILE: &str = "notif-state.json";

/// Persisted interaction state (see [`PREFS_FILE`]): the DND toggle plus the
/// read baseline and the set of individually-read notification timestamps.
#[derive(serde::Serialize, serde::Deserialize)]
struct NotifPrefs {
    muted: bool,
    last_read_ms: u64,
    /// Timestamps (unix-ms) of individually-read notifications — a stable key
    /// across a reboot (unlike ids). A `Vec` on disk, a `HashSet` in memory.
    read_ms: Vec<u64>,
}

/// Plain-serde twin of [`ActiveNotification`] for JSON persistence. The wire
/// type derives zvariant's dict codec (`a{sv}`, values wrapped as `Variant`s),
/// which `serde_json` can't reconstruct, so the on-disk history uses this
/// field-for-field mirror and converts at the boundary.
#[derive(serde::Serialize, serde::Deserialize)]
struct StoredNotification {
    id: u32,
    app_name: String,
    app_icon: String,
    desktop_entry: String,
    summary: String,
    body: String,
    actions: Vec<String>,
    urgency: u8,
    timestamp_ms: u64,
    /// Cache filename of this notification's image (under [`IMAGE_DIR`]), or
    /// `None`. The pixels live in that file, not inline, so history JSON stays
    /// small. `#[serde(default)]` keeps older history files (no image keys)
    /// loadable.
    #[serde(default)]
    image_file: Option<String>,
    #[serde(default)]
    image_width: u32,
    #[serde(default)]
    image_height: u32,
}

impl From<&ActiveNotification> for StoredNotification {
    fn from(n: &ActiveNotification) -> Self {
        Self {
            id: n.id,
            app_name: n.app_name.clone(),
            app_icon: n.app_icon.clone(),
            desktop_entry: n.desktop_entry.clone(),
            summary: n.summary.clone(),
            body: n.body.clone(),
            actions: n.actions.clone(),
            urgency: n.urgency,
            timestamp_ms: n.timestamp_ms,
            // The image is written + linked by `save_notif_history` (which does
            // the IO); the bare conversion leaves it unset.
            image_file: None,
            image_width: 0,
            image_height: 0,
        }
    }
}

impl From<StoredNotification> for ActiveNotification {
    fn from(n: StoredNotification) -> Self {
        Self {
            id: n.id,
            app_name: n.app_name,
            app_icon: n.app_icon,
            desktop_entry: n.desktop_entry,
            summary: n.summary,
            body: n.body,
            actions: n.actions,
            urgency: n.urgency,
            timestamp_ms: n.timestamp_ms,
            // Image pixels aren't persisted (they'd bloat the JSON); a reloaded
            // notification falls back to its resolved app icon. Live and
            // still-active notifications keep their real image.
            image_rgba: Vec::new(),
            image_width: 0,
            image_height: 0,
            // A persisted notification is by definition not transient — transient
            // ones are never written to disk (see `save_notif_history`).
            transient: None,
        }
    }
}

/// Per-history-item inputs `measure_notif` computes before the renderer borrow:
/// `(app, summary, body, time, icon_key)`.
type NotifRowInput = (String, String, String, u64, Option<String>);

/// One notification's pre-measured render fields. The collapsed preview pill
/// uses the single-line fields (`summary`/`body`/`time`); the open box uses the
/// wrapped `body_lines` + `height` to lay each card out at its own size.
struct RowInfo {
    summary: String,
    /// The single line shown on the collapsed preview pill: the *newest* message
    /// of a multi-message stack (the last line of the body), so the pill reflects
    /// the latest message and updates as new ones land. The open card uses the
    /// wrapped `body_lines` instead.
    preview: String,
    /// Copy time (unix ms) — the compact relative label ("15m"/"2h"/"1d") is
    /// computed live at render, like the clipboard list, so it stays current.
    timestamp_ms: u64,
    /// First letter of the app/summary, for the identity tile (the fallback
    /// when no real icon resolves).
    initial: String,
    /// Resolved-icon key for this row (the icon name/path handed to the
    /// resolver; also the [`App::notif_icon_slot`] key). `None` when the
    /// notification carries no usable icon hint.
    icon_key: Option<String>,
    /// Body pre-wrapped to the card's content width, capped at
    /// [`MAX_BODY_LINES`] (last line ellipsised on overflow).
    body_lines: Vec<String>,
    summary_w: f32,
    /// Full card height in the open box: padding + header + wrapped body.
    height: f32,
}

/// What the pointer is over inside the notification OPTION while the box is
/// open — drives per-target click dispatch and the pointer cursor. Card
/// indices are into `notif.history` / `notif.rows`.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum NotifHit {
    None,
    /// The body of a card (index) — click opens it (its `default` action + raise
    /// the source window).
    Card(usize),
    /// A card's × dismiss control.
    Close(usize),
    /// The footer's ✕ (dismiss all).
    DismissAll,
}

/// All notification-OPTION state (owned by [`App`] as `notif`).
pub(crate) struct NotifState {
    /// Append-only, newest first — survives dismissals (our own history).
    pub(crate) history: Vec<ActiveNotification>,
    /// Ids already folded into `history`, to detect genuinely new arrivals.
    seen: HashSet<u32>,
    /// How many notifications are currently *active* on the daemon (its live set).
    active_count: usize,
    /// Unix-ms baseline for "read": history older than this (everything at the
    /// moment the box was opened) counts as read. Combined with `read_ms` below to
    /// decide what's unread. Persisted, so read-state survives restart/reboot.
    last_read_ms: u64,
    /// Timestamps of notifications the user has *individually* seen without opening
    /// the box — the newest is marked when they glance at the preview pill. Keyed
    /// by timestamp (stable across a reboot, unlike the daemon's reused ids), so a
    /// glance reads only the pill's notification while the rest stay unread.
    /// Persisted.
    read_ms: HashSet<u64>,
    /// Preview open (hovered, held after leave, or auto-shown on arrival).
    peek_reveal: bool,
    /// Set right after a click on the pop-up opens+dismisses a notification: keeps
    /// the preview collapsed even though the pointer is still over the pill (so it
    /// doesn't immediately re-reveal the *next* notification). Cleared the moment
    /// the pointer leaves.
    peek_suppressed: bool,
    /// Do-Not-Disturb: while muted, arrivals are still recorded to history but
    /// never pop the preview or play a sound — instead the bell blinks. The
    /// resting bell shows the muted glyph. Toggled from the mute pill.
    muted: bool,
    /// When the DND arrival blink ends (`None` = not blinking). The bell pulses
    /// until then.
    blink_until: Option<Instant>,
    /// History rectangle open.
    pub(crate) expanded: bool,
    /// Morph progress: 0 bell → 1 preview pill (horizontal growth).
    peek_t: f32,
    /// Morph progress: 0 pill → 1 tall history rectangle (vertical growth).
    expand_t: f32,
    /// The *animated* full-open box height (px). Eases toward the content-fit
    /// target ([`App::notif_full_h`]) so that a change in content — clearing to the
    /// empty state, dismissing a card — animates the box height smoothly instead of
    /// snapping. `0.0` before the first open (seeded on open).
    box_h: f32,
    /// Vertical scroll (px) within the history list — the *animated* value the
    /// draw reads. Eases toward `scroll_target` for smooth scrolling.
    list_scroll: f32,
    /// Where `list_scroll` is heading (px). Set directly by the wheel (clamped to
    /// the pixel span); `list_scroll` eases toward it each frame.
    scroll_target: f32,
    /// History card under the pointer (index into `history`/`rows`), for the
    /// per-card hover spotlight. `None` = none / collapsed.
    hover_card: Option<usize>,
    /// Fine-grained hit target inside the open box (card control / footer),
    /// recomputed on motion and used by click + cursor.
    hit: NotifHit,
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
        // Durable history: reload what the last session left so notifications
        // persist across reboots until explicitly erased. A missing or corrupt
        // file simply yields an empty history (see [`crate::persist`]).
        let mut history: Vec<ActiveNotification> =
            crate::persist::read_json::<Vec<StoredNotification>>(&crate::persist::data_path(
                HISTORY_FILE,
            ))
            .unwrap_or_default()
            .into_iter()
            .map(|s| {
                // Rehydrate the image from the on-disk cache before converting,
                // so a reloaded card shows its real avatar (not the fallback).
                let img = s
                    .image_file
                    .as_deref()
                    .and_then(|f| load_stored_image(f, s.image_width, s.image_height))
                    .map(|px| (px, s.image_width, s.image_height));
                let mut n: ActiveNotification = s.into();
                if let Some((px, w, h)) = img {
                    n.image_rgba = px;
                    n.image_width = w;
                    n.image_height = h;
                }
                n
            })
            .collect();
        // Clean up a stack that predates the conversation-collapse: newest-first,
        // then one card per conversation.
        history.sort_by_key(|h| std::cmp::Reverse(h.timestamp_ms));
        collapse_stacks(&mut history);
        // Restore DND + read-state so the whole OPTION looks identical after a
        // restart/reboot (a fresh install with no file defaults to: not muted,
        // everything currently in history already read). The default baseline is
        // *persisted immediately* — otherwise it would reset to "now" on every
        // restart and wrongly mark all pending notifications as read.
        let prefs_path = crate::persist::data_path(PREFS_FILE);
        let prefs: NotifPrefs = crate::persist::read_json(&prefs_path).unwrap_or_else(|| {
            let p = NotifPrefs {
                muted: false,
                last_read_ms: now_ms(),
                read_ms: Vec::new(),
            };
            crate::persist::write_json("notif-state", &prefs_path, &p);
            p
        });
        // `seen` is intentionally NOT seeded from the loaded ids: the daemon
        // restarts its id counter at 1 each boot, so a reused id must be able to
        // arrive as a genuinely new notification rather than collide with an old
        // history entry. Duplicate re-hydration (waverunner restart while the
        // daemon keeps running) is instead caught by a content-identity check in
        // `on_notif_event`.
        Self {
            history,
            seen: HashSet::new(),
            active_count: 0,
            last_read_ms: prefs.last_read_ms,
            read_ms: prefs.read_ms.into_iter().collect(),
            peek_reveal: false,
            peek_suppressed: false,
            muted: prefs.muted,
            blink_until: None,
            expanded: false,
            peek_t: 0.0,
            expand_t: 0.0,
            box_h: 0.0,
            list_scroll: 0.0,
            scroll_target: 0.0,
            hover_card: None,
            hit: NotifHit::None,
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

    /// Peek-morph progress (0 = bell at rest, 1 = fully-grown preview pill). The
    /// mute pill behind the bell is uncovered by exactly this fraction, so the
    /// draw/hit code gates on it.
    pub(crate) fn peek_progress(&self) -> f32 {
        self.peek_t
    }
}

/// Current unix time in milliseconds (0 if the clock is before the epoch).
fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

/// Local `HH:MM` for a unix-millis timestamp, via libc (respects timezone).
/// Local day-number (days since the epoch in *local* time) and clock hour/minute
/// for a unix-ms stamp — via `tm_gmtoff` so the day boundary is local midnight.
fn local_day_hm(ms: u64) -> (i64, i32, i32) {
    // SAFETY: `localtime_r` fills a caller-owned `tm`.
    unsafe {
        let t = (ms / 1000) as libc::time_t;
        let mut tm: libc::tm = std::mem::zeroed();
        libc::localtime_r(&t, &mut tm);
        let local = t as i64 + tm.tm_gmtoff as i64;
        (local.div_euclid(86_400), tm.tm_hour, tm.tm_min)
    }
}

/// Time label — matches the clipboard list. Same-day items show the clock time
/// (`14:32`); older items show a compact day count (`1d`/`2d`), since a full date
/// is too much. Computed live at render. Empty for a zero stamp.
fn fmt_relative(ms: u64) -> String {
    if ms == 0 {
        return String::new();
    }
    let (day, hh, mm) = local_day_hm(ms);
    let (today, ..) = local_day_hm(now_ms());
    let diff = today - day;
    if diff <= 0 {
        format!("{hh:02}:{mm:02}")
    } else {
        format!("{diff}d")
    }
}

/// Approx label width for a relative-time string (chars × ~half em), the same
/// cheap estimate the clipboard list uses so no renderer is needed at draw time.
fn rel_time_w(time: &str) -> f32 {
    time.chars().count() as f32 * FONT_PX * 0.55
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
        let mut changed = false;
        let mut new_rows = 0usize;
        // Whether at least one arrival should pop the preview (a genuinely new
        // message, not a cross-source echo of one just shown).
        let mut fresh_pop = false;
        match ev {
            NotifEvent::Active(list) => {
                for n in list.iter().rev() {
                    if self.notif.seen.insert(n.id) {
                        // First time this session sees this id. It's a genuinely
                        // new notification UNLESS a byte-identical entry is already
                        // in our persisted history — which happens when waverunner
                        // restarts while the daemon keeps running and re-hydrates
                        // the same active notifications. Adopt those silently so
                        // restarts never duplicate. (Across a reboot the daemon
                        // reuses ids from 1, but with fresh timestamps, so an old
                        // same-id entry is never identical and correctly arrives.)
                        if !self.notif.history.iter().any(|h| h == n) {
                            // An echo of a message we already surfaced moments ago
                            // — the same chat mirrored by both the webapp and KDE
                            // Connect (same conversation + same text). It still
                            // merges into the one history card (see the collapse
                            // below); it just must not pop the preview a *second*
                            // time. A genuinely new message (different text) still
                            // pops.
                            let nkey = conversation_key(n);
                            let echo = if nkey.is_some() {
                                // Chat: the SAME message text mirrored by another
                                // source (webapp + KDE Connect). It merges but must
                                // not pop twice; a new message (different text) pops.
                                let nbody = web_format(n).1.trim().to_lowercase();
                                !nbody.is_empty()
                                    && self.notif.history.iter().any(|h| {
                                        n.timestamp_ms.saturating_sub(h.timestamp_ms)
                                            < DUP_WINDOW_MS
                                            && conversation_key(h) == nkey
                                            && web_format(h).1.trim().to_lowercase() == nbody
                                    })
                            } else {
                                // Non-chat: an identical alert is already in history
                                // (same app + summary + body). Its card's time just
                                // refreshes — don't re-announce the preview.
                                let nk = stack_key(n);
                                self.notif.history.iter().any(|h| stack_key(h) == nk)
                            };
                            self.notif.history.insert(0, n.clone());
                            arrived = true;
                            changed = true;
                            new_rows += 1;
                            fresh_pop |= !echo;
                        }
                    } else if let Some(slot) = self.notif.history.iter_mut().find(|h| h.id == n.id)
                    {
                        if slot != n {
                            *slot = n.clone(); // in-place update (replace/edit)
                            changed = true;
                        }
                    }
                }
                self.notif.active_count = list.len();
                // Reap transient notifications that have left the active set — they
                // must not linger in the durable/browsable history (OSD volume bars
                // etc.). Normal notifications stay; only these ephemeral ones are
                // reaped. Also drop them from `seen` so a reused id can re-arrive.
                let active: HashSet<u32> = list.iter().map(|n| n.id).collect();
                let gone = stale_transient_ids(&self.notif.history, &active);
                if !gone.is_empty() {
                    self.notif.history.retain(|h| !gone.contains(&h.id));
                    for id in &gone {
                        self.notif.seen.remove(id);
                    }
                    changed = true;
                }
            }
            NotifEvent::Closed { id, .. } => {
                self.notif.active_count = self.notif.active_count.saturating_sub(1);
                // A phone message read/cleared on the device closes its KDE Connect
                // notification — those are live unread indicators, not durable
                // history, so drop the card (it "resets" once you've read it).
                if let Some(pos) = self
                    .notif
                    .history
                    .iter()
                    .position(|h| h.id == id && is_kdeconnect(h))
                {
                    self.notif.history.remove(pos);
                    self.notif.seen.remove(&id);
                    changed = true;
                }
            }
            NotifEvent::Disconnected => {
                self.notif.active_count = 0;
            }
        }
        if changed {
            // The one ordering invariant: strictly newest-first by timestamp,
            // enforced no matter what order a batch was folded in (a hydrate or
            // multi-notification `ActiveChanged` can arrive oldest-first). This is
            // what keeps the box from ever showing an inverted list. `sort_by` is
            // stable and near-linear on an already-ordered list, and notifications
            // are infrequent, so re-sorting on every change is cheap.
            self.notif
                .history
                .sort_by_key(|h| std::cmp::Reverse(h.timestamp_ms));
            // One card per conversation: the newest wins, older same-conversation
            // cards (the KDE Connect message stack, or a phone+desktop duplicate of
            // the same chat) are dropped. Runs after the sort so "newest" is first.
            let removed = collapse_stacks(&mut self.notif.history);
            for id in &removed {
                self.notif.seen.remove(id);
            }
            // Keep the read-timestamp set bounded: drop marks for notifications no
            // longer in the history (dismissed / collapsed away), so it — and the
            // persisted state file — can't grow without limit.
            let live: HashSet<u64> = self.notif.history.iter().map(|h| h.timestamp_ms).collect();
            self.notif.read_ms.retain(|ts| live.contains(ts));
        }
        self.measure_notif();
        // Keep the open box anchored on the same notifications when new ones
        // arrive at the top: shift the scroll down by the combined height of the
        // inserted cards so the viewed cards don't jump — unless we're already at
        // the very top, where a fresh arrival should simply appear. Then clamp to
        // the (possibly grown/shrunk) pixel span. Both the drawn value and its
        // target move together so an in-flight ease isn't disturbed.
        if self.notif.expanded {
            let span = self.notif_scroll_span();
            let shift = if self.notif.list_scroll > 0.5 && new_rows > 0 {
                self.card_offset(new_rows)
            } else {
                0.0
            };
            self.notif.list_scroll = (self.notif.list_scroll + shift).clamp(0.0, span);
            self.notif.scroll_target = (self.notif.scroll_target + shift).clamp(0.0, span);
        }
        // Only touch the disk when the durable history actually changed — a bare
        // close/active-count update leaves the browsable list untouched.
        if changed {
            self.save_notif_history();
        }

        // How a fresh arrival announces itself depends on DND (`muted`). Never
        // yank the surface while the user is actively hovering/browsing it.
        let busy = self.notif.expanded
            || matches!(self.options_hover, Some(PillId::Notif | PillId::NotifMute));
        if arrived {
            // A new arrival is unread (its timestamp is newer than `last_read_ms`),
            // holding until the user looks — see `mark_notif_read`. If the box is
            // already open they're looking at it, so it's read on arrival.
            if self.notif.expanded {
                self.mark_notif_read();
            }
            // Sound is left to the posting apps; the shade only shows the preview
            // (or, under DND, blinks the bell) — and only for a fresh message, not
            // a cross-source echo of one just shown.
            if fresh_pop && !busy {
                if self.notif.muted {
                    // DND on: no pill — just blink the bell icon.
                    self.notif_blink();
                } else {
                    // DND off: pop the preview pill.
                    self.notif_flash();
                }
            } else if self.options_layer.is_some() {
                self.draw_options();
            }
        } else if self.options_layer.is_some() {
            self.draw_options();
        }
    }

    /// A muted (DND) arrival: no pill, no sound — pulse the bell for a beat so
    /// something landing is still felt, then settle.
    fn notif_blink(&mut self) {
        self.notif.blink_until = Some(Instant::now() + BLINK_DURATION);
        self.notif.last = None;
        self.schedule_notif_frame();
    }

    /// The muted-arrival heartbeat (`0.0..=1.0`, `None` when idle): a smooth pulse
    /// over [`BLINK_PERIOD`] that swells to `1` and settles back to `0`, driving the
    /// bell *pill's* beat (grow + hover wash). Smoothstepped so it breathes rather
    /// than ticks.
    fn bell_blink(&self) -> Option<f32> {
        let until = self.notif.blink_until?;
        let now = Instant::now();
        if now >= until {
            return None;
        }
        let rem = (until - now).as_secs_f32();
        let phase = (rem / BLINK_PERIOD.as_secs_f32()).fract();
        let tri = 1.0 - (phase * 2.0 - 1.0).abs();
        Some(tri * tri * (3.0 - 2.0 * tri)) // smoothstep for an eased beat
    }

    /// Write the browsable history to disk (best-effort, atomic — see
    /// [`crate::persist`]). Called whenever the history changes so it survives a
    /// reboot; the running session's in-memory copy is always authoritative.
    fn save_notif_history(&self) {
        let stored: Vec<StoredNotification> = self
            .notif
            .history
            .iter()
            // Transient notifications (volume/brightness OSD, synchronous toasts)
            // bypass the durable log — shown live, never written to disk, so they
            // don't survive a reboot or accumulate.
            .filter(|n| is_persistable(n))
            .map(|n| {
                let mut s = StoredNotification::from(n);
                // Spill the image to the content-addressed cache and link it, so
                // the avatar survives a restart without inflating the JSON.
                if !n.image_rgba.is_empty() && n.image_width > 0 {
                    s.image_file = Some(store_image(image_hash(n), &n.image_rgba));
                    s.image_width = n.image_width;
                    s.image_height = n.image_height;
                }
                s
            })
            .collect();
        crate::persist::write_json(
            "notif-history",
            &crate::persist::data_path(HISTORY_FILE),
            &stored,
        );
    }

    /// Auto-show the newest notification in the preview pill, then hold and
    /// collapse (the "pop in on arrival" behaviour). The hold scales with the
    /// newest arrival's urgency (see [`flash_hold`]): low is brief, critical
    /// lingers so it isn't missed.
    fn notif_flash(&mut self) {
        self.notif.expanded = false;
        self.measure_notif();
        self.notif.peek_reveal = true;
        self.notif.last = None;
        self.schedule_notif_frame();
        let hold = self
            .notif
            .history
            .first()
            .map_or(HOLD, |n| flash_hold(n.urgency));
        self.schedule_notif_collapse(hold);
    }

    /// Re-measure every history item into [`RowInfo`]s (one canonical layout for
    /// the band and the list). Cheap; only on data change.
    pub(crate) fn measure_notif(&mut self) {
        // Which captured images are generic app logos vs real per-contact
        // avatars: the daemon captures `app_icon`'s browser logo as the "image"
        // when a notification has no avatar, so the SAME image ends up on many
        // notifications from DIFFERENT senders. An image shared across 2+ distinct
        // summaries is therefore a logo (don't use it as the tile) — a real avatar
        // stays bound to its one contact.
        let logo_keys: HashSet<String> = {
            let mut by_img: HashMap<String, HashSet<&str>> = HashMap::new();
            for n in &self.notif.history {
                if !n.image_rgba.is_empty() && n.image_width > 0 {
                    by_img
                        .entry(image_key(n))
                        .or_default()
                        .insert(n.summary.as_str());
                }
            }
            by_img
                .into_iter()
                .filter(|(_, s)| s.len() >= 2)
                .map(|(k, _)| k)
                .collect()
        };

        // Icon hints are resolved off `self.entries` here (before the mutable
        // renderer borrow below) so each row carries the key its real icon will
        // land under once the resolver replies.
        let items: Vec<NotifRowInput> = self
            .notif
            .history
            .iter()
            .map(|n| {
                let (summary, body) = web_format(n);
                (
                    n.app_name.clone(),
                    summary,
                    body,
                    n.timestamp_ms,
                    // Icon priority: a real per-contact avatar wins; but a captured
                    // browser logo (shared image) is NOT used — those fall to the
                    // web service icon / resolved app icon.
                    {
                        let avatar = (!n.image_rgba.is_empty() && n.image_width > 0)
                            .then(|| image_key(n))
                            .filter(|k| !logo_keys.contains(k));
                        avatar.or_else(|| self.service_icon(n).or_else(|| self.notif_icon_hint(n)))
                    },
                )
            })
            .collect();
        // The body wraps to the card's text column: full width minus both pads
        // and the identity tile + its gap.
        let body_w = (EXTENDED_W - 2.0 * CARD_PAD_X - ICON_SZ - ICON_GAP).max(1.0);
        let mut rows = Vec::with_capacity(items.len());
        if let Some(r) = self.options_renderer.as_mut() {
            for (app, summary, body, timestamp_ms, icon_key) in items {
                let summary_w = r.measure_text(&summary, FONT_PX, None);
                let mut m = |s: &str| r.measure_text(s, FONT_PX, None);
                let body_lines = wrap_text(&mut m, &body, body_w);
                let initial = card_initial(&app, &summary);
                let height = card_height(&body_lines);
                let preview = newest_line(&body);
                rows.push(RowInfo {
                    summary,
                    preview,
                    timestamp_ms,
                    initial,
                    icon_key,
                    body_lines,
                    summary_w,
                    height,
                });
            }
        } else {
            for (app, summary, body, timestamp_ms, icon_key) in items {
                let initial = card_initial(&app, &summary);
                let preview = newest_line(&body);
                rows.push(RowInfo {
                    summary,
                    preview,
                    timestamp_ms,
                    initial,
                    icon_key,
                    body_lines: Vec::new(),
                    summary_w: 0.0,
                    height: card_height(&[]),
                });
            }
        }
        self.notif.rows = rows;
        self.ensure_notif_images();
        self.request_notif_icons();
    }

    /// Build + upload the icon chain for every history notification that ships
    /// its own image and isn't in the icon array yet. Unlike app icons (resolved
    /// off-thread from a name), these pixels are already in hand, so the chain is
    /// built inline; it's guarded by the slot map, so repeat calls are cheap.
    fn ensure_notif_images(&mut self) {
        let pending: Vec<(String, Vec<u8>, u32, u32)> = self
            .notif
            .history
            .iter()
            .filter(|n| !n.image_rgba.is_empty() && n.image_width > 0)
            .map(|n| {
                (
                    image_key(n),
                    n.image_rgba.clone(),
                    n.image_width,
                    n.image_height,
                )
            })
            .filter(|(k, ..)| !self.notif_icon_slot.contains_key(k))
            .collect();
        let mut added = false;
        for (key, rgba, w, h) in pending {
            if self.notif_icon_slot.contains_key(&key) {
                continue; // a duplicate image within this batch
            }
            let Some(chain) = crate::apps::rasterize_rgba(&rgba, w, h) else {
                continue;
            };
            let slot = self.notif_icon_chains.len() as u32;
            self.notif_icon_chains.push(chain);
            self.notif_icon_slot.insert(key, slot);
            added = true;
        }
        if added {
            self.upload_options_icons();
        }
    }

    /// The best icon name/path to resolve for a notification, or `None` when it
    /// carries no usable hint. Preference: an explicit `app_icon` (a themed name
    /// or absolute path the app chose), then the icon of the matching indexed
    /// `.desktop` (by `desktop_entry` id, then by display name). The returned
    /// string doubles as the resolver/slot key, so notifications sharing an icon
    /// share one texture layer.
    fn notif_icon_hint(&self, n: &ActiveNotification) -> Option<String> {
        // Web notification: prefer the installed webapp's SERVICE icon (Messenger,
        // WhatsApp, …) over the generic browser icon.
        if let Some(icon) = self.webapp_icon_for(n) {
            return Some(icon);
        }
        // Most reliable: the notifying app's indexed `.desktop` icon.
        if !n.desktop_entry.is_empty() {
            if let Some(icon) = self.entry_icon(|e| e.id == n.desktop_entry) {
                return Some(icon);
            }
        }
        // An icon the app chose explicitly: a themed name, or a real file path.
        if let Some(icon) = usable_app_icon(&n.app_icon) {
            return Some(icon);
        }
        // Last resort: match the app by display name.
        if !n.app_name.is_empty() {
            if let Some(icon) = self.entry_icon(|e| e.name.eq_ignore_ascii_case(&n.app_name)) {
                return Some(icon);
            }
        }
        None
    }

    /// Service icon for a *service-level* web notification — one with no message
    /// text (body is empty once the origin is trimmed), e.g. Messenger's
    /// content-less "you have activity". These arrive with a generic browser logo
    /// captured as the image, so we override it with the installed webapp's
    /// service icon, identified by the body origin or the summary. A real message
    /// (non-empty body) returns `None` so it keeps its per-contact avatar.
    fn service_icon(&self, n: &ActiveNotification) -> Option<String> {
        // Content-less = the cleaned body is empty or just the bare origin host
        // (a lone origin isn't trimmed to empty). A real message has other text.
        let cb = clean_body(&n.body);
        if !(cb.is_empty() || looks_like_host(cb.trim_end_matches('/'))) {
            return None;
        }
        if let Some(kw) = notif_app_keyword(&n.body) {
            if let Some(icon) = self.entry_icon(|e| e.id == format!("webapp-{kw}")) {
                return Some(icon);
            }
        }
        let s = strip_markup(&n.summary);
        let host = s.trim_end_matches('/');
        if !s.is_empty() && !looks_like_host(host) {
            return self
                .entry_icon(|e| e.id.starts_with("webapp-") && e.name.eq_ignore_ascii_case(&s));
        }
        None
    }

    /// The service icon of an installed webapp matching a web notification, or
    /// `None`. Matches by (1) the body's site origin → `webapp-<keyword>` id or a
    /// webapp whose `Exec` hosts that site, then (2) the summary / app name
    /// against a webapp's display name — Chrome sets a PWA notification's title to
    /// its app name (e.g. "Messenger") even when the body carries no origin.
    fn webapp_icon_for(&self, n: &ActiveNotification) -> Option<String> {
        if let Some(kw) = notif_app_keyword(&n.body) {
            if let Some(icon) = self.entry_icon(|e| e.id == format!("webapp-{kw}")) {
                return Some(icon);
            }
            if let Some(icon) = self
                .entry_icon(|e| e.id.starts_with("webapp-") && e.exec.to_lowercase().contains(&kw))
            {
                return Some(icon);
            }
        }
        for raw in [&n.summary, &n.app_name] {
            let name = strip_markup(raw);
            if name.is_empty() || looks_like_host(name.trim_end_matches('/')) {
                continue;
            }
            if let Some(icon) =
                self.entry_icon(|e| e.id.starts_with("webapp-") && e.name.eq_ignore_ascii_case(&name))
            {
                return Some(icon);
            }
        }
        None
    }

    /// The icon name of the first indexed app matching `pred`, if it has one.
    fn entry_icon(
        &self,
        pred: impl Fn(&waverunner_core::index::AppEntry) -> bool,
    ) -> Option<String> {
        self.entries
            .iter()
            .find(|e| pred(e))
            .and_then(|e| e.icon.clone())
    }

    /// Queue resolver jobs for every row icon we don't already have (or haven't
    /// already asked for). Cheap and idempotent — safe to call on any re-measure.
    fn request_notif_icons(&mut self) {
        let mut wanted: Vec<String> = self
            .notif
            .rows
            .iter()
            .filter_map(|r| r.icon_key.clone())
            .filter(|k| {
                !self.notif_icon_slot.contains_key(k) && !self.notif_icon_pending.contains(k)
            })
            .collect();
        // Many rows share one icon (all Chrome cards → `google-chrome`); resolve
        // each distinct hint once.
        wanted.sort_unstable();
        wanted.dedup();
        for icon in wanted {
            if let Some(handle) = &self.notif_icons_handle {
                handle.request(crate::notif_icons::Request {
                    key: icon.clone(),
                    icon: icon.clone(),
                    name: String::new(),
                });
            }
            self.notif_icon_pending.insert(icon);
        }
    }

    /// A resolver reply landed: adopt a real icon into the OPTIONS icon array and
    /// redraw so the card swaps its monogram for the real tile. A `None` chain
    /// (unresolvable) just clears the pending mark — the card keeps its monogram.
    pub(crate) fn on_notif_icon(&mut self, res: crate::notif_icons::Resolved) {
        self.notif_icon_pending.remove(&res.key);
        let Some(chain) = res.chain else { return };
        if self.notif_icon_slot.contains_key(&res.key) {
            return; // already have it (a duplicate reply)
        }
        let slot = self.notif_icon_chains.len() as u32;
        self.notif_icon_chains.push(chain);
        self.notif_icon_slot.insert(res.key, slot);
        self.upload_options_icons();
        self.draw_options();
    }

    /// Push the OPTIONS renderer's icon array: the notif card avatars first, then
    /// the clipboard thumbnails. A no-op until that renderer exists — so it must
    /// also be called once the renderer is created (see `frame.rs`), since a
    /// resolve can land first and its chain would otherwise never reach the GPU.
    /// Shared by both the notif icon resolver and the clipboard thumbnailer.
    pub(crate) fn upload_options_icons(&mut self) {
        if let Some(r) = self.options_renderer.as_mut() {
            r.set_options_icons(&self.notif_icon_chains, &self.clip.icon_chains);
        }
    }

    /// Cumulative Y of card `idx`'s top from the box's content top (sum of the
    /// heights of the cards above it) — always a valid scroll *boundary*.
    fn card_offset(&self, idx: usize) -> f32 {
        offset_of(&self.notif_heights(), idx)
    }

    /// Per-card heights, newest first. All the scroll geometry works purely on
    /// these (see the pure `offset_of` / `index_at` helpers), so it can be
    /// unit-tested without a live `App`.
    fn notif_heights(&self) -> Vec<f32> {
        self.notif.rows.iter().map(|r| r.height).collect()
    }

    /// Total height of all cards stacked in the open box.
    fn cards_total_h(&self) -> f32 {
        self.notif.rows.iter().map(|r| r.height).sum()
    }

    /// The bar-pill height (the collapsed element's diameter / the preview band).
    fn notif_band_h(&self) -> f32 {
        (self.config.options.height as f32 - 2.0 * PILL_MARGIN_Y).max(1.0)
    }

    /// Geometry of the whole morphing element given its pinned right edge, top,
    /// and band height: width from `peek_t`, height from `expand_t` (grows down).
    pub(crate) fn notif_geom(&self, right: f32, y: f32, ph: f32) -> Rect {
        // Two distinct pills: the fixed bell sits at `right`; the preview lives
        // *behind* it at rest and slides out to the left as `peek_t` rises,
        // clearing the bell (plus a bond gap) so the two read as separate pills.
        let right = right - (ph + BOND_GAP) * self.notif.peek_t;
        let mut w = lerp(ph, EXTENDED_W, self.notif.peek_t).max(ph);
        if right - w < EDGE_PAD {
            w = (right - EDGE_PAD).max(ph);
        }
        // Use the *eased* full height (`box_h`), so a content change (clear to the
        // empty state, dismiss a card) morphs the box smoothly. Falls back to the
        // live target before the first open seeds it.
        let full = if self.notif.box_h > 0.0 {
            self.notif.box_h
        } else {
            self.notif_full_h(ph)
        };
        let h = lerp(ph, full, self.notif.expand_t);
        Rect::new(right - w, y, w, h)
    }

    /// The element's current rect (from anywhere: input region, scroll clamps).
    pub(crate) fn notif_rect(&self) -> Rect {
        let ph = self.notif_band_h();
        let y = PILL_MARGIN_Y;
        let right = self.options_clock_left() - OPTION_GAP;
        self.notif_geom(right, y, ph)
    }

    /// Bottom edge (surface px) the pointer-input region must reach while the
    /// drawer is open — the *fully-expanded* box height, independent of the
    /// expand animation. Sizing the input region off the live (animating)
    /// height instead left it short until some later re-sync, so hover/scroll
    /// cut out partway down the list at an unstable row.
    pub(crate) fn notif_input_bottom(&self) -> f32 {
        // Match the box's height so the input region hugs the actual box — no dead
        // hover zone below a short box. Use the larger of the eased height and its
        // target so the region covers the box all through a shrink animation.
        let ph = self.notif_band_h();
        PILL_MARGIN_Y + self.notif_full_h(ph).max(self.notif.box_h)
    }

    /// The box's fully-expanded height: fit to content (cards + pad + footer band,
    /// capped at the dropdown max) so a short list gives a short box; a compact
    /// fixed panel when there are no notifications (the centred empty state).
    fn notif_full_h(&self, ph: f32) -> f32 {
        if self.notif.rows.is_empty() {
            return EMPTY_H;
        }
        (self.cards_total_h() + LIST_PAD + self.notif_footer_h()).clamp(ph, EXPANDED_H)
    }

    /// Diameter of the footer ✕ pill — noticeably larger than a bar pill so it
    /// reads as the box's primary action, floating on the fill.
    fn footer_button_d(&self) -> f32 {
        self.notif_band_h() * 1.4
    }

    /// Height reserved at the box bottom for the floating ✕ pill — enough that the
    /// (enlarged) circle clears the list with breathing room above and below. No
    /// strip; it floats on the box fill, and this just keeps the list clear of it.
    fn notif_footer_h(&self) -> f32 {
        // No footer (nothing to clear) when there are no notifications.
        if self.notif.rows.is_empty() {
            return 0.0;
        }
        self.footer_button_d() + 3.0 * PILL_MARGIN_Y
    }

    /// The box's content region — the area above the floating footer pills. The
    /// reserve grows in with the expand, so the content shrinks to meet it.
    fn notif_content_rect(&self, rect: Rect) -> Rect {
        let foot = self.notif_footer_h() * self.notif.expand_t;
        Rect::new(rect.x, rect.y, rect.w, (rect.h - foot).max(0.0))
    }

    /// Lay out the cards that intersect the open box's content region: `(index,
    /// full-card rect)`, newest (index 0) flush at the content top, each stacked
    /// below by its own height and shifted up by `list_scroll`. Shared by the
    /// draw and the hit-test so they can never disagree.
    fn notif_cards(&self, rect: Rect) -> Vec<(usize, Rect)> {
        let content = self.notif_content_rect(rect);
        let mut out = Vec::new();
        let mut top = content.y - self.notif.list_scroll;
        for (idx, r) in self.notif.rows.iter().enumerate() {
            let h = r.height;
            let bottom = top + h;
            if bottom > content.y && top < content.y + content.h {
                out.push((idx, Rect::new(rect.x, top, rect.w, h)));
            }
            top = bottom;
        }
        out
    }

    /// The footer zone rect at the box bottom (the reserved area the pills float
    /// in — used for hit-testing the region, not painted).
    fn notif_footer_rect(&self, rect: Rect) -> Rect {
        let h = self.notif_footer_h();
        Rect::new(rect.x, rect.y + rect.h - h, rect.w, h)
    }

    /// The single centred footer button — the "Clear all" ✕ pill, an enlarged
    /// circle centred in the bottom reserve, floating on the box fill.
    fn footer_button_rect(&self, rect: Rect) -> Rect {
        let f = self.notif_footer_rect(rect);
        let d = self.footer_button_d();
        let x = f.x + (f.w - d) / 2.0;
        let y = f.y + (f.h - d) / 2.0;
        Rect::new(x, y, d, d)
    }

    /// What the pointer is over inside the open box (footer > cards).
    fn notif_hit(&self) -> NotifHit {
        if self.notif.expand_t < 0.5 {
            return NotifHit::None;
        }
        let Some(p) = self.options_ptr else {
            return NotifHit::None;
        };
        let rect = self.notif_rect();
        if self.notif_footer_rect(rect).contains(p) {
            if self.footer_button_rect(rect).contains(p) {
                return NotifHit::DismissAll;
            }
            return NotifHit::None;
        }
        for (idx, crect) in self.notif_cards(rect) {
            if crect.contains(p) {
                if card_close_rect(crect).contains(p) {
                    return NotifHit::Close(idx);
                }
                return NotifHit::Card(idx);
            }
        }
        NotifHit::None
    }

    /// Recompute the hit target + hovered card from the pointer; returns whether
    /// anything changed (so the caller can redraw). Called on pointer motion.
    pub(crate) fn update_notif_hit(&mut self) -> bool {
        let hit = self.notif_hit();
        let hover_card = match hit {
            NotifHit::Card(i) | NotifHit::Close(i) => Some(i),
            _ => None,
        };
        let changed = hit != self.notif.hit || hover_card != self.notif.hover_card;
        self.notif.hit = hit;
        self.notif.hover_card = hover_card;
        changed
    }

    /// Whether the pointer is on a clickable notif target (for the cursor shape).
    pub(crate) fn notif_hit_clickable(&self) -> bool {
        match self.notif.hit {
            NotifHit::None => false,
            // A card body is clickable only when it can open (has a default action).
            NotifHit::Card(i) => self.card_has_default(i),
            _ => true,
        }
    }

    /// Draw the whole morphing element: the pill/rectangle fill, the bell glyph
    /// (only while collapsed — it fades out as the pill opens), the preview line,
    /// and the history list (fading in). One rounded rect — the pill becomes the box.
    pub(crate) fn push_notif_pill(&self, scene: &mut Scene, rect: Rect) {
        let ph = self.notif_band_h();
        let bright = self.options_bar_is_bright();
        let e = self.notif.expand_t;
        // Full stadium/circle while collapsed, easing to the gentler box radius as
        // it opens so the full-width stripes can round to the corners cleanly.
        let radius = lerp(ph / 2.0, BOX_RADIUS, e);

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
        // Spotlight ink: the open box rests muted; the hovered card pops to full
        // contrast (white on a dark box, black on a light one).
        let dark_ink = ink[0] + ink[1] + ink[2] < 1.5;
        let hover_ink = if dark_ink {
            [0.0, 0.0, 0.0, 1.0]
        } else {
            [1.0, 1.0, 1.0, 1.0]
        };
        // Dark ink on a light box needs more presence than light ink on a dark one
        // to read at the same muting (a black wash is perceptually weaker than a
        // white one at equal alpha) — so lift the resting dim on light boxes and
        // keep the tuned dark-box value.
        let list_dim = if dark_ink { LIST_DIM_LIGHT } else { LIST_DIM };
        let dim_ink = [ink[0], ink[1], ink[2], ink[3] * list_dim];
        let band_ty = rect.y + (ph - LINE_PX) / 2.0;
        // The bell is now a separate fixed element (see `push_notif_mute`); this
        // sliding element is pure preview/box that slides out from behind it.

        // Preview text-slide anchors (from the pill's left) and the preview
        // fade-in fraction (rides the peek).
        let tx = lerp(rect.x + ph, rect.x + PILL_PAD_X, self.notif.peek_t);
        let right = rect.x + rect.w - PILL_PAD_X;
        let pa = ((self.notif.peek_t - 0.35) / 0.5).clamp(0.0, 1.0);

        // Adaptive zebra stripe colour: lighten a dark box, darken a light one,
        // off the box fill luminance (0.179 = the WCAG flip used for ink too).
        // Pre-composited over the fill into an OPAQUE colour so overlapping stripe
        // pieces overwrite instead of double-blending.
        let flum =
            0.2126 * expanded_fill[0] + 0.7152 * expanded_fill[1] + 0.0722 * expanded_fill[2];
        let stripe = if flum <= 0.179 {
            wash(true, STRIPE_LIGHTEN)
        } else {
            wash(false, STRIPE_DARKEN)
        };
        let sa = stripe[3];
        let stripe_opaque = [
            stripe[0] * sa + expanded_fill[0] * (1.0 - sa),
            stripe[1] * sa + expanded_fill[1] * (1.0 - sa),
            stripe[2] * sa + expanded_fill[2] * (1.0 - sa),
            1.0,
        ];

        let content = self.notif_content_rect(rect);
        let full_h = self
            .notif
            .rows
            .first()
            .map_or_else(|| card_height(&[]), |r| r.height);
        let morph_h = lerp(ph, full_h, e);

        // The open transition plays the height-morph: the newest card grows from
        // the one-liner and pushes the rest down. This only applies while the box
        // is opening AND still anchored at the top (`list_scroll ≈ 0`, which is
        // where every open starts). Once the box is fully open — or as soon as the
        // user scrolls — rendering switches to the plain, pixel-positioned list so
        // scrolling is smooth (a partially-scrolled top card is fine and expected).
        let morphing = self.notif.list_scroll < 0.5 && e < 0.999;
        if self.notif.rows.is_empty() {
            // Empty state: a single centred "No notifications" line (no cards, no
            // footer, no scroll), fading in with the reveal.
            if pa > 0.01 {
                let a = [dim_ink[0], dim_ink[1], dim_ink[2], dim_ink[3] * pa];
                scene.labels.push(Label {
                    text: "No notifications".to_owned(),
                    pos: (content.x + content.w / 2.0, content.y + (content.h - LINE_PX) / 2.0),
                    max_w: content.w,
                    font_px: FONT_PX,
                    line_px: LINE_PX,
                    centered: true,
                    dim: false,
                    cache: true,
                    family: None,
                    color: Some(a),
                    clip: Some(content),
                });
            }
        } else if pa > 0.01 && (e <= 0.01 || morphing) {
            self.push_notif_top_card(
                scene, 0, rect, content, ph, e, pa, band_ty, tx, right, radius, ink, dim_ink,
                hover_ink, stripe_opaque,
            );
            // Cards below the morphing top, pushed down as it grows.
            if e > 0.01 {
                let mut y = content.y + morph_h;
                for idx in 1..self.notif.rows.len() {
                    let h = self.notif.rows[idx].height;
                    if y >= content.y + content.h {
                        break;
                    }
                    if y + h > content.y {
                        let crect = Rect::new(rect.x, y, rect.w, h);
                        self.push_notif_card(
                            scene, idx, crect, content, radius, e, ink, dim_ink, hover_ink,
                            stripe_opaque, expanded_fill,
                        );
                    }
                    y += h;
                }
            }
        } else if e > 0.01 {
            // Settled / scrolling: every visible card at its pixel position (the
            // same layout the hit-test uses, so they never disagree).
            for (idx, crect) in self.notif_cards(rect) {
                self.push_notif_card(
                    scene, idx, crect, content, radius, e, ink, dim_ink, hover_ink, stripe_opaque,
                    expanded_fill,
                );
            }
        }

        if e > 0.01 && !self.notif.rows.is_empty() {
            self.push_notif_footer(scene, rect, e, bright);
        }
    }

    /// Draw the top card as a true height-morph: at `e = 0` it is the collapsed
    /// one-liner (summary · body … time centred in the bar); at `e = 1` it is a
    /// full card (header + wrapped body + identity tile). The summary/time stay
    /// solid and slide into place while the card grows, the icon and block body
    /// fade in, and the inline body preview fades out.
    #[allow(clippy::too_many_arguments)]
    fn push_notif_top_card(
        &self,
        scene: &mut Scene,
        idx: usize,
        rect: Rect,
        content: Rect,
        ph: f32,
        e: f32,
        pa: f32,
        band_ty: f32,
        tx: f32,
        right: f32,
        radius: f32,
        ink: [f32; 4],
        dim_ink: [f32; 4],
        hover_ink: [f32; 4],
        stripe_opaque: [f32; 4],
    ) {
        let placeholder = RowInfo {
            summary: "No notifications".to_owned(),
            preview: String::new(),
            timestamp_ms: 0,
            initial: String::new(),
            icon_key: None,
            body_lines: Vec::new(),
            summary_w: 0.0,
            height: card_height(&[]),
        };
        let info = self.notif.rows.get(idx).unwrap_or(&placeholder);
        let full_h = info.height;
        let morph_h = lerp(ph, full_h, e);
        let card_rect = Rect::new(rect.x, content.y, rect.w, morph_h);

        let hovered = self.notif.hover_card == Some(idx) && e > 0.5;
        // Full contrast while peeking; settles to the muted list ink (or the
        // hovered pop) as the box opens.
        let base_ink = if hovered {
            hover_ink
        } else {
            lerp4(ink, dim_ink, e)
        };
        let prim = [base_ink[0], base_ink[1], base_ink[2], base_ink[3] * pa];
        let dim = [
            base_ink[0],
            base_ink[1],
            base_ink[2],
            base_ink[3] * 0.6 * pa,
        ];

        // Zebra (only when this card is an odd index — the newest, 0, has none).
        if idx % 2 == 1 && e > 0.01 {
            let top = card_rect.y.max(content.y);
            let bot = (card_rect.y + card_rect.h).min(content.y + content.h);
            if bot > top {
                let at_top = top <= content.y + 0.5;
                let sr = if at_top { radius } else { 0.0 };
                scene.rects.push(RectInst {
                    rect: Rect::new(rect.x, top, rect.w, bot - top),
                    radius: sr,
                    color: stripe_opaque,
                    glass: 0.0,
                });
                if at_top {
                    let y = top + radius;
                    if bot > y {
                        scene.rects.push(RectInst {
                            rect: Rect::new(rect.x, y, rect.w, bot - y),
                            radius: 0.0,
                            color: stripe_opaque,
                            glass: 0.0,
                        });
                    }
                }
            }
        }

        // Identity tile fades in as the card forms.
        let icon = Rect::new(rect.x + CARD_PAD_X, rect.y + CARD_PAD_Y, ICON_SZ, ICON_SZ);
        if e > 0.01 {
            scene.rects.push(RectInst {
                rect: icon,
                radius: ICON_SZ / 2.0, // circle, matching the round avatars
                color: [ink[0], ink[1], ink[2], 0.10 * e],
                glass: 0.0,
            });
            if !info.initial.is_empty() {
                scene.labels.push(centered_glyph(
                    &info.initial,
                    icon,
                    None,
                    [prim[0], prim[1], prim[2], prim[3] * e],
                    content,
                ));
            }
        }

        // Header (summary + time) — solid, sliding from the one-liner position to
        // the card header position as the card grows.
        let text_x = icon.x + ICON_SZ + ICON_GAP;
        let header_x = lerp(tx, text_x, e);
        let header_y = lerp(band_ty, rect.y + CARD_PAD_Y, e);
        let full_rect = Rect::new(rect.x, rect.y, rect.w, full_h);
        let card_header_right = card_close_rect(full_rect).x - CTRL_GAP_N;
        let header_right = lerp(right, card_header_right, e);

        // Hidden-count badge — a filled green chip with the count inside, on the
        // newest card's PREVIEW line (idx 0), sitting just left of the time: how
        // many MORE notifications are hidden in the history (the one on the pill
        // isn't counted). Resets once you open the box (all read). Fades out as the
        // box opens.
        let count = if idx == 0 { self.notif_hidden_count() } else { 0 };
        let badge_num = (count > 0).then(|| format!("+{count}"));
        let chip_h = LINE_PX;
        let chip_w = badge_num.as_ref().map_or(0.0, |n| {
            (n.chars().count() as f32 * FONT_PX * 0.6 + 12.0).max(chip_h)
        });
        let has_badge = badge_num.is_some();
        // Badge nests in the pill's rounded right cap: inset from the true right
        // edge by the same margin as its vertical centering, so it hugs the edge.
        let badge_x = rect.x + rect.w - (ph - chip_h) / 2.0 - chip_w;

        // Amber count chip: flush to the FAR-RIGHT edge on the preview, fading out
        // as the box opens. Same amber as the unread bell.
        if let Some(n) = badge_num {
            if e < 0.999 {
                let a = 1.0 - e;
                let cr = Rect::new(
                    badge_x,
                    header_y + (LINE_PX - chip_h) / 2.0,
                    chip_w,
                    chip_h,
                );
                scene.rects.push(RectInst {
                    rect: cr,
                    radius: chip_h / 2.0,
                    // Border-accent fill (softened relative to the bell, since a
                    // flat chip reads stronger than the bell's blended tint).
                    color: [AMBER[0], AMBER[1], AMBER[2], 0.2 * a],
                    glass: 0.0,
                });
                scene.labels.push(Label {
                    text: n,
                    pos: (cr.x + cr.w / 2.0, cr.y + (cr.h - LINE_PX) / 2.0),
                    max_w: cr.w + 4.0,
                    font_px: FONT_PX,
                    line_px: LINE_PX,
                    centered: true,
                    dim: false,
                    cache: true,
                    family: None,
                    // Adaptive full-contrast ink (dark on a light box, light on a
                    // dark one) so the count stays legible on any background.
                    color: Some([hover_ink[0], hover_ink[1], hover_ink[2], a]),
                    clip: Some(cr),
                });
            }
        }
        // Time: right-aligned, just left of the badge on the preview, sliding to the
        // header edge as the box opens.
        let mut sum_right = header_right;
        let time = fmt_relative(info.timestamp_ms);
        if !time.is_empty() {
            let time_w = rel_time_w(&time);
            let time_right = if has_badge {
                lerp(badge_x - TEXT_GAP, header_right, e)
            } else {
                header_right
            };
            let time_x = (time_right - time_w).max(header_x);
            scene.labels.push(mk_line(
                time,
                time_x,
                header_y,
                time_w + 2.0,
                dim,
                content,
            ));
            sum_right = time_x - TEXT_GAP;
        } else if has_badge {
            sum_right = (badge_x - TEXT_GAP).max(header_x);
        }
        let sum_max = (sum_right - header_x).max(0.0);
        scene.labels.push(mk_line(
            info.summary.clone(),
            header_x,
            header_y,
            sum_max,
            prim,
            content,
        ));

        // Inline body preview (the one-liner tail) fades OUT as the block body
        // below fades IN. Shows the NEWEST message of the stack (`preview`), so the
        // pill reflects the latest and updates as new ones land — not the first.
        if !info.preview.is_empty() {
            let bx = header_x + info.summary_w.min(sum_max) + TEXT_GAP;
            if e < 0.999 && bx < sum_right {
                let inline = [dim[0], dim[1], dim[2], dim[3] * (1.0 - e)];
                scene.labels.push(mk_line(
                    info.preview.clone(),
                    bx,
                    header_y,
                    sum_right - bx,
                    inline,
                    content,
                ));
            }
        }

        // Block body (wrapped) fades in, clipped to the growing card so it is
        // revealed as the card opens.
        if e > 0.01 && !info.body_lines.is_empty() {
            let clip = Rect::new(
                card_rect.x,
                card_rect.y,
                card_rect.w,
                morph_h.min(content.h),
            );
            let body_top = rect.y + CARD_PAD_Y + LINE_PX + BODY_GAP;
            let body_max = (rect.x + rect.w - CARD_PAD_X - text_x).max(0.0);
            let bink = [base_ink[0], base_ink[1], base_ink[2], base_ink[3] * 0.6 * e];
            for (li, line) in info.body_lines.iter().enumerate() {
                let ly = body_top + li as f32 * LINE_PX;
                scene
                    .labels
                    .push(mk_line(line.clone(), text_x, ly, body_max, bink, clip));
            }
        }

        // × dismiss once the box is open.
        if hovered {
            let close = card_close_rect(full_rect);
            let xc = if self.notif.hit == NotifHit::Close(idx) {
                CRIMSON
            } else {
                dim_ink
            };
            scene
                .labels
                .push(centered_glyph(GLYPH_TRASH, close, Some(NERD), xc, content));
        }
    }

    /// Draw one history card: zebra band, identity tile, header (summary, time,
    /// and the hover controls), and the wrapped body — all clipped to `content`
    /// and faded by `alpha` (the expand).
    #[allow(clippy::too_many_arguments)]
    fn push_notif_card(
        &self,
        scene: &mut Scene,
        idx: usize,
        rect: Rect,
        content: Rect,
        radius: f32,
        alpha: f32,
        ink: [f32; 4],
        dim_ink: [f32; 4],
        hover_ink: [f32; 4],
        stripe_opaque: [f32; 4],
        fill: [f32; 4],
    ) {
        let Some(info) = self.notif.rows.get(idx) else {
            return;
        };
        // Zebra: alternate cards get the opaque stripe, clipped to the content
        // region; only the corner meeting the box top rounds (a square overlay
        // trims the rest so interior boundaries stay straight).
        if idx % 2 == 1 {
            let top = rect.y.max(content.y);
            let bot = (rect.y + rect.h).min(content.y + content.h);
            if bot > top {
                let at_top = top <= content.y + 0.5;
                let sr = if at_top { radius } else { 0.0 };
                scene.rects.push(RectInst {
                    rect: Rect::new(rect.x, top, rect.w, bot - top),
                    radius: sr,
                    color: stripe_opaque,
                    glass: 0.0,
                });
                if at_top {
                    let y = top + radius;
                    if bot > y {
                        scene.rects.push(RectInst {
                            rect: Rect::new(rect.x, y, rect.w, bot - y),
                            radius: 0.0,
                            color: stripe_opaque,
                            glass: 0.0,
                        });
                    }
                }
            }
        }

        let hovered = self.notif.hover_card == Some(idx);
        let card_ink = if hovered { hover_ink } else { dim_ink };
        let prim = [card_ink[0], card_ink[1], card_ink[2], card_ink[3] * alpha];
        let dim = [
            card_ink[0],
            card_ink[1],
            card_ink[2],
            card_ink[3] * 0.6 * alpha,
        ];

        // Identity tile: a real app icon once the resolver has one for this row,
        // else the monogram fallback (a soft rounded square + the initial).
        let icon = Rect::new(rect.x + CARD_PAD_X, rect.y + CARD_PAD_Y, ICON_SZ, ICON_SZ);
        // Opaque disc behind every icon so a transparent icon (a themed glyph, a
        // round avatar's corners, or the monogram fallback) shows this consistent
        // tone rather than letting the zebra stripe bleed through it. Composited on
        // the base card fill (not the stripe) plus the usual faint ink tint, so it
        // reads identically on striped and plain cards. Rides the same content
        // scissor as the icons so it clips to the list, never over the footer.
        push_notif_grid_rect(
            scene,
            content,
            RectInst {
                rect: icon,
                radius: ICON_SZ / 2.0, // circle, matching the round avatars
                color: [
                    ink[0] * 0.10 + fill[0] * 0.90,
                    ink[1] * 0.10 + fill[1] * 0.90,
                    ink[2] * 0.10 + fill[2] * 0.90,
                    alpha,
                ],
                glass: 0.0,
            },
        );
        let layer = info
            .icon_key
            .as_ref()
            .and_then(|k| self.notif_icon_slot.get(k))
            .copied();
        if let Some(layer) = layer {
            // Textured quads clip via a grid scissor, not per-item, so they must
            // ride a grid pinned to the box interior (they scroll with the list).
            push_notif_icon(scene, content, icon, layer);
        } else if !info.initial.is_empty() {
            scene
                .labels
                .push(centered_glyph(&info.initial, icon, None, prim, content));
        }

        // Header: summary (primary) + a single trailing control at the right.
        // Like the clipboard list, the relative time shows by default and swaps
        // to the dismiss can on hover (only ever one of the two, never both).
        let text_x = icon.x + ICON_SZ + ICON_GAP;
        // The trailing time / dismiss can sits in the card's top-right corner, on
        // the summary's header line.
        let header_ty = rect.y + CARD_PAD_Y;
        let close = card_close_rect(rect);
        let mut header_right = rect.x + rect.w - TRAIL_PAD_X;
        if hovered {
            // Dismiss can — no red, brighten to the hover ink on the target.
            // Drawn directly (not via `centered_glyph`) so it matches the clip
            // list: the larger `FONT_PX * 1.3` glyph, centred in the row.
            let on_x = self.notif.hit == NotifHit::Close(idx);
            let xc = if on_x { hover_ink } else { dim_ink };
            scene.labels.push(Label {
                text: GLYPH_TRASH.to_owned(),
                pos: (close.x + close.w / 2.0, header_ty),
                max_w: close.w + 8.0,
                font_px: FONT_PX * 1.3,
                line_px: LINE_PX,
                centered: true,
                dim: false,
                cache: true,
                family: Some(NERD),
                color: Some([xc[0], xc[1], xc[2], xc[3] * alpha]),
                clip: Some(content),
            });
            header_right = close.x - CTRL_GAP_N;
        } else {
            // Compact relative time ("15m"/"2h"/"1d"), computed live, centred.
            let time = fmt_relative(info.timestamp_ms);
            if !time.is_empty() {
                let time_w = rel_time_w(&time);
                let time_x = (header_right - time_w).max(text_x);
                scene
                    .labels
                    .push(mk_line(time, time_x, header_ty, time_w + 2.0, dim, content));
                header_right = time_x - TEXT_GAP;
            }
        }
        let sum_max = (header_right - text_x).max(0.0);
        scene.labels.push(mk_line(
            info.summary.clone(),
            text_x,
            header_ty,
            sum_max,
            prim,
            content,
        ));

        // Wrapped body beneath the header.
        let body_top = rect.y + CARD_PAD_Y + LINE_PX + BODY_GAP;
        let body_max = (rect.x + rect.w - CARD_PAD_X - text_x).max(0.0);
        for (li, line) in info.body_lines.iter().enumerate() {
            let ly = body_top + li as f32 * LINE_PX;
            scene
                .labels
                .push(mk_line(line.clone(), text_x, ly, body_max, dim, content));
        }
    }

    /// Draw the single footer option — the "Clear all" ✕ pill — styled exactly
    /// like a topbar OPTIONS control: a neumorphic circle with the bar's rest/hover
    /// wash and the red fa-times glyph. Fades in with `alpha` (the expand).
    fn push_notif_footer(&self, scene: &mut Scene, rect: Rect, alpha: f32, bright: bool) {
        // No divider — the pill floats free on the box fill (like the bar pills).
        let hovered = self.notif.hit == NotifHit::DismissAll;
        // Same unified hover lift as every bar pill.
        let br = self.footer_button_rect(rect);
        let br = if hovered { hover_grow(br) } else { br };
        let radius = br.h / 2.0; // stadium with w == h ⇒ circle
        push_neumorph(scene, br, radius, bright, alpha);
        let mut base = if hovered {
            self.options_hover_wash()
        } else {
            self.options_rest_wash()
        };
        base[3] *= alpha;
        scene.rects.push(RectInst {
            rect: br,
            radius,
            color: base,
            glass: 0.0,
        });
        // The trash-can glyph, scaled to the enlarged pill. Sized off the resting
        // diameter (not the hover-grown one) so it stays steady, and clipped a
        // touch wider than the pill so it isn't shaved.
        let d0 = self.footer_button_d();
        let gpx = d0 * 0.68;
        let cx = br.x + br.w / 2.0;
        let ty = br.y + (br.h - gpx) / 2.0;
        let gclip = Rect::new(br.x - 4.0, br.y - 4.0, br.w + 8.0, br.h + 8.0);
        let g = self.options_text_color();
        scene.labels.push(Label {
            text: GLYPH_TRASH.to_owned(),
            pos: (cx, ty),
            max_w: br.w + 16.0,
            font_px: gpx,
            line_px: gpx,
            centered: true,
            dim: false,
            cache: true,
            family: Some(NERD),
            color: Some([g[0], g[1], g[2], g[3] * alpha]),
            clip: Some(gclip),
        });
    }

    /// Draw the notification bell — the FIXED circle that is also the DND toggle.
    /// Its glyph is the current state (🔔 off / 🔕 on), amber while there are
    /// unread notifications and pulsing on a muted arrival. The sliding preview/box
    /// grows out from behind it; clicking it toggles DND. It fades out as the full
    /// history box opens so it doesn't sit on top of the first card.
    pub(crate) fn push_notif_mute(&self, scene: &mut Scene, rect: Rect) {
        // The bell is the fixed DND toggle: it stays fully present at all times,
        // whether the preview slides out or the history box is fully open.
        let alpha = 1.0;
        let hovered = self.options_hover == Some(PillId::NotifMute);
        // A muted arrival lands as a heartbeat on the *pill* (not the glyph): the
        // whole button pulses like the hover effect — a touch of grow + the hover
        // wash — swelling and settling over the beat. Hover pins it fully lifted.
        let beat = self.bell_blink().unwrap_or(0.0);
        let lift = if hovered { 1.0 } else { beat };
        let grown = hover_grow(rect);
        let rect = Rect::new(
            lerp(rect.x, grown.x, lift),
            lerp(rect.y, grown.y, lift),
            lerp(rect.w, grown.w, lift),
            lerp(rect.h, grown.h, lift),
        );
        let bright = self.options_bar_is_bright();
        let radius = rect.h / 2.0; // stadium with w == h ⇒ circle
        push_neumorph(scene, rect, radius, bright, alpha);
        // The beat overshoots the hover wash so the pulse reads far brighter than a
        // plain hover. On a dark bar the peak flashes toward white for maximum pop;
        // on a light bar it stays the strong adaptive (darkening) wash. A real hover
        // just settles at the normal hover wash.
        // When muted (DND on), the whole cue is gentler: the beat pulse is softer.
        let hover = self.options_hover_wash();
        let peak_a = if self.notif.muted { 0.55 } else { 0.92 };
        let peak_w = if self.notif.muted { 0.5 } else { 0.78 };
        let peak = if hovered {
            hover
        } else if bright {
            [hover[0], hover[1], hover[2], peak_a]
        } else {
            [1.0, 1.0, 1.0, peak_w]
        };
        let mut base = lerp4(self.options_rest_wash(), peak, lift);
        // Unread → the WHOLE pill goes amber: the fill is tinted amber, AND an amber
        // glow radiates around it. The glow carries the amber past the pill's rim,
        // so the light neumorph edge no longer contrasts against the fill and eats
        // into it — which is what made the plain-tinted pill read smaller. Both ease
        // out as the pill lifts (hover/beat). Matches the "+N" count badge.
        //
        // The amber lives on the bell ONLY while the preview pill is hidden: as the
        // big pill slides out (`peek_t` → 1) — on hover OR an auto-pop — the bell's
        // amber fades to nothing (the "+N" badge carries it there instead), and it
        // returns as the preview collapses.
        // Battery alarm owns the bell's accent when active (`battery.rs`):
        // the fill and glow go RED — steady at Low, breathing at Beating+ —
        // replacing (not stacking on) the unread amber. Always full strength:
        // a dying battery outranks the peek fade.
        if let Some(s) = self.battery_pulse() {
            let red_fill = [BATTERY_RED[0], BATTERY_RED[1], BATTERY_RED[2], 0.78];
            base = lerp4(base, red_fill, 0.55 * s * alpha);
            let g = 0.30 * s * alpha;
            if g > 0.001 {
                scene.overlay_shadows.push(ShadowInst {
                    rect,
                    radius,
                    blur: 3.0,
                    color: [BATTERY_RED[0], BATTERY_RED[1], BATTERY_RED[2], g],
                    edges: [1.0, 1.0, 1.0, 1.0],
                });
            }
        } else if self.notif_unread() > 0 {
            // Unread accent strength (shared by bell, muted bell, and "+N" badge).
            // Starting strong to match the window-border colour; will be softened.
            let soft = 1.0;
            let u = (1.0 - self.notif.peek_t) * alpha;
            // Fill: blend the wash toward amber so the pill body reads warm (gently).
            let amber_fill = [AMBER[0], AMBER[1], AMBER[2], 0.72];
            base = lerp4(base, amber_fill, 0.33 * u * soft);
            // Glow: a tight amber halo around the pill, bridging the fill to the edge.
            let g = 0.22 * u * soft;
            if g > 0.001 {
                scene.overlay_shadows.push(ShadowInst {
                    rect,
                    radius,
                    blur: 3.0,
                    color: [AMBER[0], AMBER[1], AMBER[2], g],
                    edges: [1.0, 1.0, 1.0, 1.0],
                });
            }
        }
        base[3] *= alpha;
        scene.rects.push(RectInst {
            rect,
            radius,
            color: base,
            glass: 0.0,
        });

        // The glyph keeps a steady colour — unread is signalled by the pill's amber
        // tint above, not the icon.
        let bc = self.options_text_color();
        // `fa-bell-slash` sits smaller in its em box than `fa-bell`, so scale the
        // muted glyph up to read at the same size as the open bell (the normal
        // bell keeps its exact original metrics).
        // Battery Critical (or the post-wake awareness pause) replaces the
        // bell with the warning triangle — the awareness symbol.
        let (glyph, gpx, glh) = if self.battery_warning() {
            (GLYPH_BATTERY_WARN, FONT_PX, LINE_PX)
        } else if self.notif.muted {
            (GLYPH_BELL_SLASH, FONT_PX * 1.4, LINE_PX * 1.4)
        } else {
            (GLYPH_BELL, FONT_PX, LINE_PX)
        };
        let cx = rect.x + rect.w / 2.0;
        let ty = rect.y + (rect.h - glh) / 2.0;
        scene.labels.push(Label {
            text: glyph.to_owned(),
            pos: (cx, ty),
            max_w: rect.w + 4.0,
            font_px: gpx,
            line_px: glh,
            centered: true,
            dim: false,
            cache: true,
            family: Some(NERD),
            color: Some([bc[0], bc[1], bc[2], bc[3] * alpha]),
            clip: Some(rect),
        });
    }

    /// Toggle notification muting (from a click on the mute pill).
    pub(crate) fn toggle_notif_mute(&mut self) {
        self.notif.muted = !self.notif.muted;
        self.save_notif_prefs();
        self.draw_options();
    }

    /// Persist DND + read-state so the OPTION looks identical after a restart or
    /// reboot. Best-effort (logged, never fatal) like the history save.
    fn save_notif_prefs(&self) {
        let prefs = NotifPrefs {
            muted: self.notif.muted,
            last_read_ms: self.notif.last_read_ms,
            read_ms: self.notif.read_ms.iter().copied().collect(),
        };
        crate::persist::write_json(
            "notif-state",
            &crate::persist::data_path(PREFS_FILE),
            &prefs,
        );
    }

    /// Whether a single history entry is unread: newer than the read baseline AND
    /// not individually marked read (glanced at on the pill).
    fn notif_is_unread(&self, n: &ActiveNotification) -> bool {
        n.timestamp_ms > self.notif.last_read_ms && !self.notif.read_ms.contains(&n.timestamp_ms)
    }

    /// Total unread notifications — drives the amber. Stays lit while any unread
    /// remain, so glancing at the pill (which reads only the newest) doesn't clear
    /// it if more are still hidden in the history.
    fn notif_unread(&self) -> usize {
        self.notif.history.iter().filter(|h| self.notif_is_unread(h)).count()
    }

    /// Unread notifications *hidden in the history* — everything except the newest
    /// one shown on the pill. This is the preview's "N ▾" count: how many more are
    /// waiting behind the one you're looking at.
    fn notif_hidden_count(&self) -> usize {
        self.notif
            .history
            .iter()
            .skip(1)
            .filter(|h| self.notif_is_unread(h))
            .count()
    }

    /// Mark EVERYTHING in the shade as read (opening the box, or clearing it):
    /// advances the baseline and drops the now-redundant per-timestamp marks.
    fn mark_notif_read(&mut self) {
        self.notif.last_read_ms = now_ms();
        self.notif.read_ms.clear();
        self.save_notif_prefs();
    }

    /// Mark just the newest notification (the one on the pill) as read — a glance
    /// at the preview. Older unread entries stay unread, so the amber persists if
    /// the history still holds unseen ones. Only persists on an actual change —
    /// this runs on every pointer-motion frame while hovering, so a blind save
    /// would thrash the disk.
    fn mark_pill_read(&mut self) {
        if let Some(top) = self.notif.history.first() {
            if self.notif.read_ms.insert(top.timestamp_ms) {
                self.save_notif_prefs();
            }
        }
    }

    /// Handle a left-click while the box is open, dispatched by the current hit
    /// target. Returns whether the click was consumed (so the caller stops).
    pub(crate) fn notif_click(&mut self) -> bool {
        // Preview pill (box not yet open): a click on the pop-up opens the newest
        // notification, exactly like clicking its card in the open box.
        if !self.notif.expanded
            && self.notif.peek_t > 0.5
            && self.options_hover == Some(PillId::Notif)
            && !self.notif.history.is_empty()
        {
            self.notif_open(0);
            // Vanish instantly instead of sliding to reveal the next one; stay
            // collapsed until the pointer leaves (see `update_notif_reveal`).
            self.notif.peek_reveal = false;
            self.notif.hold_deadline = None;
            self.notif.peek_suppressed = true;
            self.notif.last = None;
            self.schedule_notif_frame();
            return true;
        }
        match self.notif.hit {
            NotifHit::Close(i) => {
                self.notif_dismiss(i);
                true
            }
            NotifHit::DismissAll => {
                self.notif_dismiss_all();
                true
            }
            // Clicking a card body opens the notification: its `default` action
            // (so the app navigates) plus raising the source window.
            NotifHit::Card(i) => {
                self.notif_open(i);
                true
            }
            NotifHit::None => false,
        }
    }

    /// The installed webapp (`webapp-*.desktop`) this notification came from, if
    /// any. Matched the same way the service icon is (origin keyword → id / exec,
    /// else the display name), so Open and the icon always agree on the source.
    fn notif_webapp(&self, n: &ActiveNotification) -> Option<&waverunner_core::index::AppEntry> {
        if let Some(kw) = notif_app_keyword(&n.body) {
            if let Some(e) = self.entries.iter().find(|e| e.id == format!("webapp-{kw}")) {
                return Some(e);
            }
            if let Some(e) = self
                .entries
                .iter()
                .find(|e| e.id.starts_with("webapp-") && e.exec.to_lowercase().contains(&kw))
            {
                return Some(e);
            }
        }
        for raw in [&n.summary, &n.app_name] {
            let name = strip_markup(raw);
            if name.is_empty() || looks_like_host(name.trim_end_matches('/')) {
                continue;
            }
            if let Some(e) = self
                .entries
                .iter()
                .find(|e| e.id.starts_with("webapp-") && e.name.eq_ignore_ascii_case(&name))
            {
                return Some(e);
            }
        }
        None
    }

    /// Open a notification (a card-body click). Notifications do exactly one thing
    /// now, so it must be right: route to the source **webapp**, never the plain
    /// browser. If the service is an installed webapp we fire its `default` action
    /// (so it navigates to the chat) and raise its own PWA window — or launch it if
    /// it isn't running. If the service isn't installed as a webapp we don't dump
    /// the user in Chrome; we prompt them to install it.
    fn notif_open(&mut self, idx: usize) {
        let Some(n) = self.notif.history.get(idx) else {
            return;
        };
        let id = n.id;
        let has_default = has_default_action(&n.actions);
        // Only a browser web notification carries a site origin in its body; a
        // phone notification bridged in over KDE Connect (or any native app) does
        // not — so we never nag "Install X" for those.
        let is_web = notif_app_keyword(&n.body).is_some();
        let webapp = self
            .notif_webapp(n)
            .map(|e| (e.startup_wm_class.clone(), e.exec.clone()));
        match webapp {
            Some((class, exec)) => {
                if has_default {
                    if let Some(h) = &self.notif.handle {
                        h.send(NotifCommand::Invoke {
                            id,
                            key: "default".to_string(),
                        });
                    }
                }
                self.schedule_webapp_open(class, exec);
            }
            // Prompt to install only for a genuine web notification (a site origin
            // + a `default` action) whose webapp isn't installed — never for our
            // own prompts, plain local toasts, or bridged phone notifications.
            None if has_default && is_web => self.prompt_install(idx),
            None => {}
        }
        self.notif_dismiss(idx);
    }

    /// After a short beat (so the app's own `notificationclick` navigation runs
    /// first), raise the webapp's PWA window by its exact class; if no such window
    /// is open, launch the webapp instead.
    fn schedule_webapp_open(&mut self, class: Option<String>, exec: String) {
        let timer = Timer::from_duration(Duration::from_millis(750));
        let _ = self
            .loop_handle
            .insert_source(timer, move |_, _, _app: &mut App| {
                let focused = class
                    .as_deref()
                    .is_some_and(crate::hypr::focus_exact_class);
                if !focused {
                    let _ = crate::launch::launch(&exec, false, "");
                }
                TimeoutAction::Drop
            });
    }

    /// The notification's service isn't installed as a webapp: guide the user to
    /// install it (so future notifications open cleanly) rather than opening the
    /// plain browser. Posts a normal notification back through the daemon.
    fn prompt_install(&mut self, idx: usize) {
        let Some(n) = self.notif.history.get(idx) else {
            return;
        };
        let service = notif_app_keyword(&n.body)
            .map(|kw| friendly_service(&kw))
            .or_else(|| {
                let s = strip_markup(&n.summary);
                (!s.is_empty() && !looks_like_host(s.trim_end_matches('/'))).then_some(s)
            })
            .unwrap_or_else(|| n.app_name.clone());
        if let Some(h) = &self.notif.handle {
            h.send(NotifCommand::InstallPrompt { service });
        }
    }

    /// Whether card `idx`'s notification can be opened (offers a `default`
    /// action), so the cursor reads as clickable over it.
    fn card_has_default(&self, idx: usize) -> bool {
        self.notif
            .history
            .get(idx)
            .is_some_and(|n| has_default_action(&n.actions))
    }

    /// Dismiss one notification: drop it from our history, tell the daemon to
    /// close it, persist, and keep the view valid.
    fn notif_dismiss(&mut self, idx: usize) {
        if idx >= self.notif.history.len() {
            return;
        }
        let n = self.notif.history.remove(idx);
        if let Some(h) = &self.notif.handle {
            h.send(NotifCommand::Dismiss(n.id));
        }
        self.measure_notif();
        self.save_notif_history();
        if self.notif.history.is_empty() {
            // Retire the box the same graceful way every other empty path does,
            // and — crucially — drive the collapse animation so it hides even
            // when the pointer never leaves (e.g. a card-body click that opens
            // the app and pulls focus away). See `collapse_empty_box`.
            self.collapse_empty_box();
        }
        // Keep the scroll within the (now shorter) pixel span after removal.
        let span = self.notif_scroll_span();
        self.notif.list_scroll = self.notif.list_scroll.min(span);
        self.notif.scroll_target = self.notif.scroll_target.min(span);
        self.sync_options_input();
        self.update_notif_hit();
        self.draw_options();
    }

    /// Dismiss every notification (footer ✕): close them all on the daemon, clear
    /// our history, and collapse the box.
    fn notif_dismiss_all(&mut self) {
        if let Some(h) = &self.notif.handle {
            for n in &self.notif.history {
                h.send(NotifCommand::Dismiss(n.id));
            }
        }
        self.notif.history.clear();
        self.mark_notif_read();
        self.notif.list_scroll = 0.0;
        self.notif.scroll_target = 0.0;
        self.measure_notif();
        self.save_notif_history();
        self.collapse_empty_box();
        self.sync_options_input();
        self.update_notif_hit();
        self.draw_options();
    }

    /// Gracefully retire the open box once its history has just gone empty —
    /// shared by every path that can empty it (Clear all, the last card's ✕,
    /// opening the last card). Keeps the box open a beat on the centred "No
    /// notifications" state (shrinking to it), then lets it fall away on its
    /// own — even though the pointer is still over it, or focus has moved to an
    /// app we just opened — and doesn't re-reveal until the pointer leaves.
    ///
    /// The immediate `schedule_notif_frame` is what drives the collapse: without
    /// it `expand_t` never eases back to 0, so the box would stay stuck on the
    /// empty state until dismissed by hand (which is exactly what happened when a
    /// card-body click opened the app — no pointer-leave ever rescued it).
    fn collapse_empty_box(&mut self) {
        self.notif.peek_suppressed = true;
        let timer = Timer::from_duration(EMPTY_HOLD);
        let _ = self
            .loop_handle
            .insert_source(timer, move |_, _, app: &mut App| {
                // Unless a notification arrived in the meantime, collapse it away.
                if app.notif.history.is_empty() {
                    app.notif.expanded = false;
                    app.notif.peek_reveal = false;
                    app.notif.hold_deadline = None;
                    app.notif.last = None;
                    app.sync_options_input();
                    app.schedule_notif_frame();
                }
                TimeoutAction::Drop
            });
        self.schedule_notif_frame();
    }

    /// Maximum scroll (px): the newest at the top down to the last card's bottom
    /// resting at the content bottom. `list_scroll`/`scroll_target` are clamped to
    /// `[0, this]` — the box anchors the newest at the top on open, then scrolls
    /// smoothly and freely (a partial top card is expected while scrolling).
    fn notif_scroll_span(&self) -> f32 {
        let visible = (EXPANDED_H - self.notif_footer_h()).max(0.0);
        (self.cards_total_h() + LIST_PAD - visible).max(0.0)
    }

    /// A wheel event over the notification OPTION (raw axis value). Accumulates
    /// to whole notches, then steps.
    ///
    /// Direction honours the user's `input.natural_scroll` setting so the list
    /// scrolls the same way as the rest of the UI. `natural_scroll` (Max's
    /// default) is the "content follows the gesture" convention: pushing up the
    /// wheel walks toward the **newest** (top); the other setting flips it. The
    /// normalised delta is negative for an "up/newer" gesture and positive for a
    /// "down/older" one — the single place the raw axis sign is interpreted, so it
    /// can't disagree with itself anywhere else.
    pub(crate) fn notif_axis(&mut self, value: f32) {
        let delta = if self.config.input.natural_scroll {
            value
        } else {
            -value
        };
        if self.notif.expanded {
            // Smooth pixel scroll: nudge the target (clamped to the pixel span);
            // `list_scroll` eases toward it each frame. Negative delta (up gesture)
            // moves toward the newest (top); positive toward older. No card
            // snapping here — the anchor-to-top only happens on open.
            self.notif.hold_deadline = None; // a scroll keeps it open
            let span = self.notif_scroll_span();
            self.notif.scroll_target =
                (self.notif.scroll_target + delta * SCROLL_SPEED).clamp(0.0, span);
            self.notif.scroll_accum = 0.0;
            self.schedule_notif_frame();
        } else {
            // Collapsed: accumulate to a whole notch, then open the box with the
            // newest flush at the top (anchored, growing from the one-liner).
            self.notif.scroll_accum += delta;
            if self.notif.scroll_accum.abs() >= NOTCH {
                self.notif.scroll_accum = 0.0;
                self.open_notif_box();
            }
        }
    }

    /// Open the history box from the collapsed preview, anchored with the newest
    /// notification flush at the top (`list_scroll = 0`).
    pub(crate) fn open_notif_box(&mut self) {
        self.notif.hold_deadline = None;
        self.notif.expanded = true;
        self.mark_notif_read(); // opening the box = reading them
        // Seed the eased box height to the current content so the open morph plays
        // from the pill up to the right size (not from a stale height).
        self.notif.box_h = self.notif_full_h(self.notif_band_h());
        self.notif.list_scroll = 0.0;
        self.notif.scroll_target = 0.0;
        self.notif.scroll_accum = 0.0;
        self.sync_options_input();
        // Opening changes whether we sample the bar's frosted colour for the box —
        // re-evaluate now so the colour is ready as it grows.
        self.reeval_options_bar();
        self.schedule_notif_frame();
    }

    /// Recompute whether the bell should preview, and manage the hold/collapse
    /// after the pointer leaves (mirrors the clock metamorphosis).
    pub(crate) fn update_notif_reveal(&mut self) {
        // Hovering the bell OR the mute pill it reveals holds the peek open, so
        // the pointer can travel out to the uncovered mute pill without collapse.
        let on = matches!(self.options_hover, Some(PillId::Notif | PillId::NotifMute));
        if on {
            // A click on the pop-up just opened+dismissed it: keep the preview
            // hidden until the pointer leaves, so it doesn't re-reveal the next.
            if self.notif.peek_suppressed {
                return;
            }
            self.notif.hold_deadline = None;
            // Glancing at the preview reads ONLY the notification shown on the pill
            // (the newest). Anything still hidden in the history stays unread, so
            // the amber persists until those are seen too (box opened).
            if self.notif_unread() > 0 {
                self.mark_pill_read();
            }
            if !self.notif.peek_reveal {
                self.notif.peek_reveal = true;
                self.notif.last = None;
                self.schedule_notif_frame();
            }
        } else {
            // Pointer left the pill — the click-suppression lifts.
            self.notif.peek_suppressed = false;
            if (self.notif.peek_reveal || self.notif.expanded)
                && self.notif.hold_deadline.is_none()
            {
                self.schedule_notif_collapse(LEAVE_HOLD);
            }
        }
    }

    /// After the pointer leaves (or an auto-show ends), hold briefly then
    /// collapse the preview + history.
    fn schedule_notif_collapse(&mut self, hold: Duration) {
        let deadline = Instant::now() + hold;
        self.notif.hold_deadline = Some(deadline);
        let timer = Timer::from_duration(hold);
        let _ = self
            .loop_handle
            .insert_source(timer, move |_, _, app: &mut App| {
                if app.notif.hold_deadline == Some(deadline) {
                    app.notif.hold_deadline = None;
                    if app.options_hover != Some(PillId::Notif) {
                        // Start the collapse; the scroll/selection reset waits until
                        // the box has fully closed (see `tick_notif`) so it's unseen.
                        app.notif.peek_reveal = false;
                        app.notif.expanded = false;
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
        let _ = self
            .loop_handle
            .insert_source(timer, |_, _, app: &mut App| {
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
        let was_shown = self.notif.peek_t > MORPH_EPS;
        let (pt, pm) = ease_toward(self.notif.peek_t, peek_target, dt, MORPH_RATE, MORPH_EPS);
        let (et, em) = ease_toward(
            self.notif.expand_t,
            expand_target,
            dt,
            MORPH_RATE,
            MORPH_EPS,
        );
        self.notif.peek_t = pt;
        self.notif.expand_t = et;

        // Ease the open-box height toward its content-fit target, so clearing to the
        // empty state (or dismissing a card) morphs the box smoothly. A hair faster
        // than the open morph so the shrink feels crisp, not sluggish. Only runs
        // while the box is open/animating — when collapsed the height is unused
        // (`open_notif_box` reseeds it), so there's no idle frame churn.
        let bm = if self.notif.expand_t > MORPH_EPS {
            let box_target = self.notif_full_h(self.notif_band_h());
            let (bh, moving) = ease_toward(self.notif.box_h, box_target, dt, MORPH_RATE * 1.3, 0.5);
            self.notif.box_h = bh;
            moving
        } else {
            false
        };

        // Smooth scrolling: ease the drawn `list_scroll` toward its target.
        let (ls, lm) = ease_toward(
            self.notif.list_scroll,
            self.notif.scroll_target,
            dt,
            SCROLL_RATE,
            0.5,
        );
        self.notif.list_scroll = ls;

        // The frame it finishes collapsing back to the bell (fully hidden): reset
        // the box scroll so it reopens fresh at the newest. Only on the
        // shown→hidden transition, so a live scroll isn't clobbered.
        if was_shown
            && !self.notif.peek_reveal
            && !self.notif.expanded
            && self.notif.peek_t <= MORPH_EPS
        {
            self.notif.list_scroll = 0.0;
            self.notif.scroll_target = 0.0;
        }

        // DND arrival blink: keep pulsing until the deadline, then clear it.
        let blinking = self.notif.blink_until.is_some_and(|u| now < u);
        if !blinking {
            self.notif.blink_until = None;
        }

        self.draw_options();
        if pm || em || lm || bm || blinking {
            self.schedule_notif_frame();
        } else {
            self.notif.last = None;
        }
    }
}

/// A card's × dismiss hot-square (top-right).
fn card_close_rect(card: Rect) -> Rect {
    Rect::new(
        card.x + card.w - TRAIL_PAD_X - CTRL_SZ,
        card.y + CARD_PAD_Y - 1.0,
        CTRL_SZ,
        CTRL_SZ,
    )
}

/// A centred glyph/letter filling `r` (identity tile, footer pills, controls).
fn centered_glyph(
    text: &str,
    r: Rect,
    family: Option<&'static str>,
    color: [f32; 4],
    clip: Rect,
) -> Label {
    Label {
        text: text.to_string(),
        pos: (r.x + r.w / 2.0, r.y + (r.h - LINE_PX) / 2.0),
        max_w: r.w + 4.0,
        font_px: FONT_PX,
        line_px: LINE_PX,
        centered: true,
        dim: false,
        cache: false,
        family,
        color: Some(color),
        clip: Some(clip),
    }
}

/// Cumulative Y of card `idx`'s top — the sum of the heights before it. Used to
/// shift the scroll when new cards arrive at the front so the view doesn't jump.
/// `idx` past the end clamps to the total.
fn offset_of(heights: &[f32], idx: usize) -> f32 {
    heights.iter().take(idx).sum()
}

/// Push a card's real icon quad into the box's scissored icon grid, creating
/// that grid (clipped to the box interior) on first use. Textured quads can't
/// carry a per-item clip the way labels do, so they all ride one grid the
/// renderer scissors to `content`, keeping icons inside the box as it scrolls.
/// The options scene has no other grids, so a lazy first-or-create is safe.
fn push_notif_icon(scene: &mut Scene, content: Rect, icon: Rect, layer: u32) {
    notif_grid(scene, content).icons.push(IconInst {
        rect: icon,
        layer,
        tint: [0.0; 4],
        ring: -1.0,
    });
}

/// Push a fill into the same content-scissored grid the real icons ride, so the
/// monogram fallback tile clips to the list interior instead of bleeding past
/// the footer (it's a plain rect, which is otherwise drawn unclipped).
fn push_notif_grid_rect(scene: &mut Scene, content: Rect, rect: RectInst) {
    notif_grid(scene, content).rects.push(rect);
}

/// The box's content-scissored grid (creating it, clipped to `content`, on first
/// use) — shared by the card avatars and monogram tiles so both clip alike.
fn notif_grid(scene: &mut Scene, content: Rect) -> &mut GridContent {
    if scene.grids.is_empty() {
        scene.grids.push(GridContent {
            clip: content,
            ..Default::default()
        });
    }
    scene.grids.last_mut().expect("grid just ensured")
}

/// Normalize a notification's `app_icon` hint to something resolvable, or
/// `None`. A themed icon name passes through as-is; a path (accepting a
/// `file://` URI) counts only if the file still exists — apps like Chrome point
/// `app_icon` at a transient scoped-temp logo that is usually already gone by
/// the time the card is drawn, so a dead path must fall back to other hints.
fn usable_app_icon(app_icon: &str) -> Option<String> {
    if app_icon.is_empty() {
        return None;
    }
    if let Some(path) = app_icon.strip_prefix("file://") {
        return std::path::Path::new(path).exists().then(|| path.to_owned());
    }
    if app_icon.starts_with('/') {
        return std::path::Path::new(app_icon)
            .exists()
            .then(|| app_icon.to_owned());
    }
    Some(app_icon.to_owned()) // a themed name
}

/// Content hash of a notification's own image (pixels + dims). Stable across
/// runs (fixed-key `DefaultHasher`), so it doubles as the on-disk cache
/// filename and the in-memory dedup key.
fn image_hash(n: &ActiveNotification) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut h = std::collections::hash_map::DefaultHasher::new();
    n.image_rgba.hash(&mut h);
    n.image_width.hash(&mut h);
    n.image_height.hash(&mut h);
    h.finish()
}

/// Slot key for a notification's image: identical images (a contact's avatar
/// across many messages) share one texture layer.
fn image_key(n: &ActiveNotification) -> String {
    format!("img:{:016x}", image_hash(n))
}

/// Directory (under the daemon's XDG data dir) holding the persisted
/// notification images — one raw premultiplied-*straight* RGBA file per unique
/// image, named by its content hash, so avatars survive a waverunner restart
/// without bloating the history JSON.
const IMAGE_DIR: &str = "notif-images";

/// Persist a notification image (best-effort, atomic), returning its cache
/// filename. Skips the write when the hash-named file already exists — images
/// are immutable by content, so an existing file is already correct.
fn store_image(hash: u64, rgba: &[u8]) -> String {
    let file = format!("{hash:016x}.raw");
    let path = crate::persist::data_path(IMAGE_DIR).join(&file);
    if !path.exists() {
        crate::persist::write_bytes("notif-image", &path, rgba);
    }
    file
}

/// Load a persisted image back into raw RGBA, or `None` if it's missing or the
/// wrong size (a truncated write, or a `w`/`h` that disagrees with the file).
fn load_stored_image(file: &str, w: u32, h: u32) -> Option<Vec<u8>> {
    if w == 0 || h == 0 {
        return None;
    }
    let path = crate::persist::data_path(IMAGE_DIR).join(file);
    let bytes = std::fs::read(path).ok()?;
    (bytes.len() == (w as usize) * (h as usize) * 4).then_some(bytes)
}

/// The identity-tile letter: first char of the app name (or summary), upper-cased.
fn card_initial(app: &str, summary: &str) -> String {
    let src = if app.is_empty() { summary } else { app };
    src.chars()
        .next()
        .map(|c| c.to_uppercase().to_string())
        .unwrap_or_default()
}

/// Full card height for a wrapped body: comfortable padding top+bottom, one
/// header line, then the body block (with its gap) — but never shorter than the
/// identity tile.
fn card_height(body_lines: &[String]) -> f32 {
    let body_h = if body_lines.is_empty() {
        0.0
    } else {
        BODY_GAP + body_lines.len() as f32 * LINE_PX
    };
    let text_h = LINE_PX + body_h;
    2.0 * CARD_PAD_Y + text_h.max(ICON_SZ)
}

/// The newest message of a (possibly multi-message) body — its last non-empty
/// line. A KDE Connect conversation bundles `msg1\nmsg2\n…msgN` oldest-first, so
/// the last line is the most recent; a single-message body returns itself.
fn newest_line(body: &str) -> String {
    body.lines()
        .map(str::trim)
        .rfind(|l| !l.is_empty())
        .unwrap_or("")
        .to_owned()
}

/// Clean a notification body for display: strip markup/entities, then drop the
/// site-origin token web notifications prepend (see [`trim_origin`]).
fn clean_body(raw: &str) -> String {
    trim_origin(&strip_markup(raw)).to_owned()
}

/// A friendly service name for a web-notification origin keyword.
fn friendly_service(kw: &str) -> String {
    match kw {
        "messenger" => "Messenger".to_string(),
        "whatsapp" => "WhatsApp".to_string(),
        "instagram" => "Instagram".to_string(),
        "facebook" => "Facebook".to_string(),
        "youtube" => "YouTube".to_string(),
        _ => {
            let mut c = kw.chars();
            c.next()
                .map(|f| f.to_uppercase().collect::<String>() + c.as_str())
                .unwrap_or_default()
        }
    }
}

/// Whether a notification was bridged in from a paired phone over KDE Connect.
fn is_kdeconnect(n: &ActiveNotification) -> bool {
    n.desktop_entry.starts_with("org.kde.kdeconnect")
        || n.app_name.eq_ignore_ascii_case("KDE Connect")
}

/// Collapse runs of whitespace to single spaces (trimmed).
fn collapse_ws(s: &str) -> String {
    s.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// Parse a KDE Connect message body — `"Sender: msg1<br/>msg2<br/>…<br/>msgN"`
/// (the sender prefixes only the first line; KDE Connect double-encodes the
/// breaks as `&lt;br/&gt;`) — into `(sender, messages)`, where `messages` is every
/// unread message, one per line (`\n`). `None` when the body isn't a
/// `Sender: message` (a system toast like "Ping!").
fn kdeconnect_sender_body(raw_body: &str) -> Option<(String, String)> {
    // Decode entities so escaped breaks become real, turn breaks into newlines,
    // then clean each message line.
    let normalized = decode_entities(raw_body)
        .replace("<br/>", "\n")
        .replace("<br />", "\n")
        .replace("<br>", "\n");
    let mut lines: Vec<String> = normalized
        .split('\n')
        .map(|l| collapse_ws(&strip_emoji(&strip_tags(l))))
        .filter(|l| !l.is_empty())
        .collect();
    let (sender, first_msg) = lines.first()?.split_once(": ")?;
    let sender = sender.trim().to_owned();
    if sender.is_empty() || sender.chars().count() > 40 {
        return None;
    }
    // Drop the "Sender:" prefix from the first line; the rest are bare messages.
    lines[0] = first_msg.trim().to_owned();
    let body = lines
        .into_iter()
        .filter(|l| !l.is_empty())
        .collect::<Vec<_>>()
        .join("\n");
    Some((sender, body))
}

/// Reformat a KDE Connect (phone) notification to read like a desktop message:
/// the sender becomes the title and the body is every unread message, one per
/// line (`"WhatsApp" / "Yesi: Gg<br/>Tf<br/>Hh"` → `"Yesi" / "Gg\nTf\nHh"`). A
/// prefix-less toast keeps the app summary + cleaned body.
fn kdeconnect_format(app_summary: &str, raw_body: &str) -> (String, String) {
    kdeconnect_sender_body(raw_body)
        .unwrap_or_else(|| (app_summary.to_owned(), clean_body(raw_body)))
}

/// A conversation identity `(service, sender)` for collapsing duplicate/stacked
/// message notifications: the KDE Connect stack (one card per message) and a
/// phone+desktop duplicate of the same chat both fold to one card. `None` for
/// anything that isn't a message (so system toasts never merge).
fn conversation_key(n: &ActiveNotification) -> Option<(String, String)> {
    if is_kdeconnect(n) {
        let (sender, _) = kdeconnect_sender_body(&n.body)?;
        let service = collapse_ws(&strip_markup(&n.summary)).to_lowercase();
        Some((service, sender.to_lowercase()))
    } else if let Some(service) = notif_app_keyword(&n.body) {
        let sender = collapse_ws(&strip_markup(&n.summary)).to_lowercase();
        (!sender.is_empty()).then_some((service, sender))
    } else {
        None
    }
}

/// Keep one card per conversation (the newest), dropping older same-conversation
/// duplicates — this both collapses a KDE Connect message stack and merges a
/// phone+desktop duplicate of the same chat. `history` must be newest-first;
/// non-message notifications (no conversation key) are always kept. Returns the
/// removed ids so the caller can clear them from `seen`.
/// The key two notifications must share to stack into a single card: a chat's
/// conversation, or — for everything else — identical content (app + summary +
/// body). The content key is namespaced (`\u{1}`) so it can never collide with a
/// conversation key.
fn stack_key(n: &ActiveNotification) -> (String, String) {
    if let Some(key) = conversation_key(n) {
        return key;
    }
    let app = collapse_ws(&strip_markup(&n.app_name)).to_lowercase();
    let summary = collapse_ws(&strip_markup(&n.summary)).to_lowercase();
    let body = collapse_ws(&web_format(n).1).to_lowercase();
    (format!("{app}\u{1}{summary}"), body)
}

/// Collapse the history so each stack ([`stack_key`]) keeps only its newest card;
/// older duplicates — a chat's message pile, or a service re-firing the same alert
/// (battery, sync, "build done") — are removed, folding their time onto the one
/// survivor. Returns the removed ids. Runs after the newest-first sort.
fn collapse_stacks(history: &mut Vec<ActiveNotification>) -> Vec<u32> {
    let mut seen: HashSet<(String, String)> = HashSet::new();
    let mut removed = Vec::new();
    history.retain(|h| {
        if seen.insert(stack_key(h)) {
            true // first (newest) card for this stack
        } else {
            removed.push(h.id);
            false
        }
    });
    removed
}

/// Display `(summary, body)` for a card, cleaned + web-formatted.
fn web_format(n: &ActiveNotification) -> (String, String) {
    let raw_summary = if n.summary.is_empty() {
        &n.app_name
    } else {
        &n.summary
    };
    let mut summary = strip_markup(raw_summary);
    let mut body = clean_body(&n.body);
    // Phone notifications bridged in over KDE Connect arrive as app=summary
    // ("WhatsApp") + body "Sender: message". Reformat to match a desktop message
    // notification: the sender is the title, the message is the body.
    if is_kdeconnect(n) {
        return kdeconnect_format(&summary, &n.body);
    }
    if let Some(kw) = notif_app_keyword(&n.body) {
        // Drop an origin-only body (no message text).
        if body.is_empty() || looks_like_host(body.trim_end_matches('/')) {
            body.clear();
        }
        // A generic summary (empty → app name, or the bare origin host) is not a
        // real sender — show the friendly service name instead ("Messenger").
        let generic = summary.is_empty()
            || looks_like_host(summary.trim_end_matches('/'))
            || summary.eq_ignore_ascii_case(&n.app_name);
        if generic {
            summary = friendly_service(&kw);
        }
    }
    (summary, body)
}

/// The site keyword of a web notification, for matching its webapp window on
/// Open — the second-level domain of the origin the browser prepends to the
/// body (`web.whatsapp.com` → "whatsapp", `www.instagram.com` → "instagram").
/// `None` when the body carries no host origin (a native app).
fn notif_app_keyword(raw_body: &str) -> Option<String> {
    let cleaned = strip_markup(raw_body);
    let host = cleaned.split_whitespace().next()?.trim_end_matches('/');
    if !looks_like_host(host) {
        return None;
    }
    let parts: Vec<&str> = host.split('.').collect();
    (parts.len() >= 2).then(|| parts[parts.len() - 2].to_string())
}

/// Drop a single leading bare-hostname token (e.g. `web.whatsapp.com`,
/// `youtube.com`) that Chrome-style web notifications prepend to the body — it
/// arrives as the `<a href>` anchor text ahead of the real message. Now that
/// the card shows the real avatar/service icon it's just noise. Only trimmed
/// when more text follows, so a body that IS just a link is left intact.
fn trim_origin(body: &str) -> &str {
    let body = body.trim_start();
    let Some((first, rest)) = body.split_once(char::is_whitespace) else {
        return body; // a single token is the whole message — keep it
    };
    let rest = rest.trim_start();
    if !rest.is_empty() && looks_like_host(first) {
        rest
    } else {
        body
    }
}

/// Whether `tok` looks like a bare hostname: dotted labels, an alphabetic TLD,
/// and only host-legal characters (no path, space, or scheme). Deliberately
/// strict so ordinary words ending in a period (`etc.`) aren't mistaken for one.
fn looks_like_host(tok: &str) -> bool {
    let tok = tok.trim_end_matches('/');
    let Some((_, tld)) = tok.rsplit_once('.') else {
        return false;
    };
    if tld.len() < 2 || !tld.chars().all(|c| c.is_ascii_alphabetic()) {
        return false;
    }
    tok.chars()
        .all(|c| c.is_ascii_alphanumeric() || c == '.' || c == '-')
}

/// Strip the FreeDesktop `body-markup` subset and unescape XML entities so a
/// card shows readable text, not tag source. We advertise `body-markup` /
/// `body-hyperlinks` in `GetCapabilities`, so senders (KDE Connect, Chromium,
/// …) are entitled to send `<a href="…">Photo</a>`, `<b>`, `<i>`, `<u>`,
/// `<img alt="…">`, `<br>` and `&amp;`-style entities — none of which should
/// reach the glyph run. Tags are dropped (an `<img>`'s `alt` text is kept, so a
/// bare inline image still reads as e.g. "Photo"); `<br>`/`<p>` become spaces;
/// runs of whitespace are collapsed (the card lays out on one flowed block).
fn strip_markup(input: &str) -> String {
    // Two phases, in this order on purpose: decode entities FIRST, then strip
    // tags. Some senders (KDE Connect bridging Android notifications) double-encode
    // their markup — the body arrives as `Gg&lt;br/&gt;Gg`, so the tags only
    // become real tags *after* entity decoding. A single left-to-right pass would
    // leave the decoded `<br/>` as literal text (it's revealed behind the
    // already-passed stripper). Decoding first fixes that.
    let stripped = strip_emoji(&strip_tags(&decode_entities(input)));
    // Collapse the whitespace that stripped tags / emoji / line breaks leave.
    stripped.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// Whether `c` is an emoji / pictograph / dingbat the monospace UI font can't
/// render — those show as tofu boxes in the cards, so they're stripped from
/// notification text. Ordinary symbols and punctuation are left alone.
fn is_emoji(c: char) -> bool {
    matches!(
        c as u32,
        0x1F000..=0x1FAFF   // emoji, pictographs, supplemental symbols
        | 0x2600..=0x27BF   // misc symbols + dingbats (☀ ✔ …)
        | 0x2300..=0x23FF   // misc technical (⌚ ⏰ …)
        | 0x2B00..=0x2BFF   // misc symbols and arrows (⭐ …)
        | 0xFE00..=0xFE0F   // variation selectors
        | 0x200D            // zero-width joiner (emoji sequences)
    )
}

/// Drop every [`is_emoji`] character from `s`.
fn strip_emoji(s: &str) -> String {
    s.chars().filter(|c| !is_emoji(*c)).collect()
}

/// Decode every `&…;` entity to its character, repeating until stable so
/// *multiply-encoded* content fully unescapes: KDE Connect double-encodes, so a
/// reaction body arrives as `&amp;quot;…&amp;quot;` — one pass leaves `&quot;`,
/// a second turns it into `"`. Bounded so pathological input can't spin.
fn decode_entities(input: &str) -> String {
    let mut cur = decode_entities_once(input);
    for _ in 0..3 {
        let next = decode_entities_once(&cur);
        if next == cur {
            break;
        }
        cur = next;
    }
    cur
}

/// One decode pass. Unrecognised entities are left literal (so a bare `&`
/// survives). See [`decode_entity`].
fn decode_entities_once(input: &str) -> String {
    let mut out = String::with_capacity(input.len());
    let mut i = 0;
    while i < input.len() {
        let rest = &input[i..];
        if rest.starts_with('&') {
            // Entities are short; only look a little way ahead for the ';'.
            let decoded = rest
                .get(..rest.find(';').map_or(0, |s| s + 1))
                .filter(|e| e.len() <= 12 && e.len() > 2)
                .and_then(|e| decode_entity(&e[1..e.len() - 1]).map(|ch| (ch, e.len())));
            if let Some((ch, len)) = decoded {
                out.push(ch);
                i += len;
                continue;
            }
        }
        let c = rest.chars().next().unwrap_or('\u{fffd}');
        out.push(c);
        i += c.len_utf8();
    }
    out
}

/// Strip HTML tags: `<br>`/`<br/>`/`<p>` become a space, an `<img>` falls back to
/// its `alt` text, everything else is dropped. A lone `<` with no `>` is kept.
fn strip_tags(input: &str) -> String {
    let mut out = String::with_capacity(input.len());
    let mut i = 0;
    while i < input.len() {
        let rest = &input[i..];
        if rest.starts_with('<') {
            if let Some(end) = rest.find('>') {
                let inner = &rest[1..end]; // between the angle brackets
                let name = inner.trim_start_matches('/').trim();
                let lname = name.to_ascii_lowercase();
                let is_break = lname == "br"
                    || lname.starts_with("br ")
                    || lname.starts_with("br/")
                    || lname == "p"
                    || lname.starts_with("p ");
                if is_break {
                    out.push(' ');
                } else if lname.starts_with("img") {
                    if let Some(alt) = attr_value(inner, "alt") {
                        out.push_str(&alt);
                    }
                }
                i += end + 1;
                continue;
            }
        }
        let c = rest.chars().next().unwrap_or('\u{fffd}');
        out.push(c);
        i += c.len_utf8();
    }
    out
}

/// Decode a single XML/HTML entity body (the text between `&` and `;`) to its
/// character: the five predefined entities, `&nbsp;`, and numeric `&#NN;` /
/// `&#xHH;` forms. Returns `None` for anything unrecognised (left literal).
fn decode_entity(ent: &str) -> Option<char> {
    match ent {
        "amp" => Some('&'),
        "lt" => Some('<'),
        "gt" => Some('>'),
        "quot" => Some('"'),
        "apos" => Some('\''),
        "nbsp" => Some('\u{00a0}'),
        _ => {
            let num = ent.strip_prefix('#')?;
            let code = match num.strip_prefix(['x', 'X']) {
                Some(hex) => u32::from_str_radix(hex, 16).ok()?,
                None => num.parse().ok()?,
            };
            char::from_u32(code)
        }
    }
}

/// Extract a double/single-quoted attribute value (e.g. `alt`) from a tag's
/// inner text, or `None` if the attribute is absent/unquoted.
fn attr_value(tag_inner: &str, name: &str) -> Option<String> {
    let lower = tag_inner.to_ascii_lowercase();
    let key = format!("{name}=");
    let start = lower.find(&key)? + key.len();
    let after = &tag_inner[start..];
    let quote = after.chars().next()?;
    if quote != '"' && quote != '\'' {
        return None;
    }
    let val = &after[1..];
    let end = val.find(quote)?;
    Some(val[..end].to_string())
}

/// Wrap `text` to `max_w`, capped at [`MAX_BODY_LINES`]. Explicit newlines (a KDE
/// Connect multi-message body puts one unread message per line) each start a new
/// line and are then greedily word-wrapped. Over the cap: a single message keeps
/// its start (ellipsised); a multi-message thread keeps the NEWEST messages
/// (marking that older ones were dropped). `measure` is the drawn width at the
/// body font.
fn wrap_text<F: FnMut(&str) -> f32>(measure: &mut F, text: &str, max_w: f32) -> Vec<String> {
    let multi = text.contains('\n');
    let mut all: Vec<String> = Vec::new();
    for segment in text.split('\n') {
        wrap_segment(measure, segment, max_w, &mut all);
    }
    if all.len() <= MAX_BODY_LINES {
        return all;
    }
    if multi {
        // Keep the newest messages (the tail); flag that older ones were dropped.
        let mut kept = all.split_off(all.len() - MAX_BODY_LINES);
        if let Some(first) = kept.first_mut() {
            *first = format!("… {first}");
        }
        kept
    } else {
        // A single long message: keep its start, ellipsise the last kept line.
        all.truncate(MAX_BODY_LINES);
        if let Some(last) = all.last_mut() {
            ellipsize(measure, last, max_w);
        }
        all
    }
}

/// Greedily word-wrap one line to `max_w`, appending the wrapped lines to `out`.
fn wrap_segment<F: FnMut(&str) -> f32>(
    measure: &mut F,
    text: &str,
    max_w: f32,
    out: &mut Vec<String>,
) {
    let words: Vec<&str> = text.split_whitespace().collect();
    if words.is_empty() {
        return;
    }
    let mut cur = String::new();
    for word in &words {
        let trial = if cur.is_empty() {
            (*word).to_string()
        } else {
            format!("{cur} {word}")
        };
        if cur.is_empty() || measure(&trial) <= max_w {
            cur = trial;
        } else {
            out.push(std::mem::take(&mut cur));
            cur = (*word).to_string();
        }
    }
    if !cur.is_empty() {
        out.push(cur);
    }
}

/// Trim `s` from the end until `…` fits within `max_w`.
fn ellipsize<F: FnMut(&str) -> f32>(measure: &mut F, s: &mut String, max_w: f32) {
    s.push('…');
    while measure(s) > max_w {
        s.pop(); // drop the ellipsis
        if s.pop().is_none() {
            s.push('…');
            return;
        }
        s.push('…'); // re-append and re-measure
    }
}

/// How long a freshly-arrived notification auto-shows in the preview before it
/// collapses, by FreeDesktop urgency (Low=0, Normal=1, Critical=2). Low is brief,
/// normal is the baseline [`HOLD`], and critical lingers so it can't be missed —
/// it still never leaves the history or clears the amber bell, so this only tunes
/// the transient peek, never data loss.
fn flash_hold(urgency: u8) -> Duration {
    match urgency {
        0 => Duration::from_millis(1000),
        2 => Duration::from_millis(8000),
        _ => HOLD,
    }
}

/// Whether a notification offers the FreeDesktop `default` action — the one a
/// plain click on the notification activates (open the app / focus the chat).
fn has_default_action(actions: &[String]) -> bool {
    action_pairs(actions).iter().any(|(k, _)| *k == "default")
}

/// Ids of transient notifications that have dropped out of the daemon's active
/// set. They're ephemeral (OSD / synchronous toasts) — shown live, but the moment
/// they're gone from the active list they must be removed from the browsable
/// history too (and they never hit disk). Normal notifications are untouched here:
/// they persist after close, which is the durable-history feature.
fn stale_transient_ids(history: &[ActiveNotification], active: &HashSet<u32>) -> Vec<u32> {
    history
        .iter()
        .filter(|h| h.transient.unwrap_or(false) && !active.contains(&h.id))
        .map(|h| h.id)
        .collect()
}

/// Whether a notification belongs in the durable (on-disk) history. Transient
/// notifications (the `transient` hint / synchronous OSD toasts) are shown live
/// but never persisted, so they don't survive a reboot or pile up. `None` on the
/// wire means an older daemon that predates the flag → treat as persistable.
fn is_persistable(n: &ActiveNotification) -> bool {
    !n.transient.unwrap_or(false)
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

#[cfg(test)]
mod tests {
    use super::*;

    fn sample(id: u32, ts: u64, summary: &str) -> ActiveNotification {
        ActiveNotification {
            id,
            app_name: "App".into(),
            app_icon: String::new(),
            desktop_entry: String::new(),
            summary: summary.into(),
            body: "Body".into(),
            actions: Vec::new(),
            urgency: 1,
            timestamp_ms: ts,
            image_rgba: Vec::new(),
            image_width: 0,
            image_height: 0,
            transient: None,
        }
    }

    /// Body that fits stays a single line, verbatim.
    #[test]
    fn short_body_is_one_line() {
        let mut m = |s: &str| s.chars().count() as f32 * 10.0;
        assert_eq!(
            wrap_text(&mut m, "hi there", 200.0),
            vec!["hi there".to_string()]
        );
    }

    /// Whitespace-only / empty bodies yield no lines (so the card has no body block).
    #[test]
    fn empty_body_is_no_lines() {
        let mut m = |_: &str| 0.0;
        assert!(wrap_text(&mut m, "   ", 100.0).is_empty());
    }

    /// A long body wraps, caps at `MAX_BODY_LINES`, and ellipsises the last kept
    /// line when content is dropped.
    #[test]
    fn wrap_caps_and_ellipsizes() {
        let mut m = |s: &str| s.chars().count() as f32 * 10.0; // ~10 chars / 100px
        let lines = wrap_text(
            &mut m,
            "one two three four five six seven eight nine ten",
            100.0,
        );
        assert!(lines.len() <= MAX_BODY_LINES);
        assert!(lines.last().unwrap().ends_with('…'));
    }

    /// A hyperlinked body (KDE Connect / WhatsApp) renders as just its anchor
    /// text — the `<a href>` tag is stripped, not shown as source.
    #[test]
    fn strip_markup_keeps_anchor_text() {
        assert_eq!(strip_markup("<a href=\"https://x/y\">Photo</a>"), "Photo");
    }

    /// Double-encoded entities (KDE Connect reaction bodies arrive as
    /// `&amp;quot;`) fully unescape, and tofu emoji are dropped.
    #[test]
    fn strip_markup_double_decodes_and_drops_emoji() {
        assert_eq!(strip_markup("Reacted 😂 to &amp;quot;hi&amp;quot;"), "Reacted to \"hi\"");
        assert_eq!(strip_markup("✔ done 🔗"), "done");
    }

    /// Bold/italic tags vanish, entities decode, and an inline image falls back
    /// to its `alt` text.
    #[test]
    fn strip_markup_tags_entities_and_img_alt() {
        assert_eq!(strip_markup("<b>Hi</b> &amp; <i>bye</i>"), "Hi & bye");
        assert_eq!(strip_markup("Tom &lt;3 &#65;&#x42;"), "Tom <3 AB");
        assert_eq!(
            strip_markup("<img src=\"x.png\" alt=\"Sticker\"/>"),
            "Sticker"
        );
    }

    /// `<br>` becomes a space and runs of whitespace collapse to one line.
    #[test]
    fn strip_markup_breaks_and_whitespace() {
        assert_eq!(strip_markup("line one<br>line two"), "line one line two");
        assert_eq!(strip_markup("  a\n\n  b  "), "a b");
    }

    /// A lone `<` or `&` with no closing delimiter is kept literally, not eaten.
    #[test]
    fn strip_markup_literal_fallbacks() {
        assert_eq!(strip_markup("5 < 6 & 7"), "5 < 6 & 7");
    }

    /// Double-encoded markup (KDE Connect bridges Android notifications as
    /// `&lt;br/&gt;`) is decoded and then stripped, not left as literal `<br/>`.
    #[test]
    fn strip_markup_double_encoded_breaks() {
        assert_eq!(
            strip_markup("Yesi: Gg&lt;br/&gt;Gg&lt;br/&gt;Tf"),
            "Yesi: Gg Gg Tf"
        );
    }

    /// A phone (KDE Connect) message reformats to `sender` title + every unread
    /// message (one per line, escaped breaks resolved); a prefix-less toast keeps
    /// the app summary.
    #[test]
    fn kdeconnect_format_promotes_sender_and_all_messages() {
        // Single message.
        assert_eq!(
            kdeconnect_format("WhatsApp", "Yesi: Gg"),
            ("Yesi".to_string(), "Gg".to_string())
        );
        // Bundle (KDE Connect double-encodes the breaks) → all messages, one line
        // each, sender prefix dropped.
        assert_eq!(
            kdeconnect_format("WhatsApp", "Yesi: Gg&lt;br/&gt;Tf&lt;br/&gt;Hh"),
            ("Yesi".to_string(), "Gg\nTf\nHh".to_string())
        );
        // No "Sender:" → keep the app summary + cleaned body.
        assert_eq!(
            kdeconnect_format("Pixel 8 Pro", "Ping!"),
            ("Pixel 8 Pro".to_string(), "Ping!".to_string())
        );
    }

    /// A multi-message body wraps one message per line and, over the cap, keeps
    /// the newest with a leading ellipsis.
    #[test]
    fn wrap_text_keeps_newest_messages() {
        let mut m = |s: &str| s.chars().count() as f32 * 10.0;
        // 6 short messages, cap 4 → keep the last 4, first flagged with "… ".
        let lines = wrap_text(&mut m, "m1\nm2\nm3\nm4\nm5\nm6", 500.0);
        assert_eq!(lines, vec!["… m3", "m4", "m5", "m6"]);
    }

    /// The same chat from the phone (KDE Connect) and the desktop webapp share a
    /// conversation key, so they collapse to one card; a device toast has none.
    #[test]
    fn conversation_key_merges_phone_and_desktop() {
        let mut phone = sample(1, 100, "WhatsApp");
        phone.app_name = "KDE Connect".into();
        phone.desktop_entry = "org.kde.kdeconnect.daemon".into();
        phone.body = "Yesi: Hi there".into();

        let mut desktop = sample(2, 200, "Yesi");
        desktop.body = "web.whatsapp.com Hi there".into();

        let key = conversation_key(&phone);
        assert_eq!(key, Some(("whatsapp".into(), "yesi".into())));
        assert_eq!(conversation_key(&desktop), key); // same → collapse

        let mut ping = sample(3, 300, "Pixel 8 Pro");
        ping.app_name = "KDE Connect".into();
        ping.desktop_entry = "org.kde.kdeconnect.daemon".into();
        ping.body = "Ping!".into();
        assert_eq!(conversation_key(&ping), None);
    }

    /// The leading site-origin token web notifications prepend is dropped, but
    /// only when real text follows and the token is genuinely host-shaped.
    #[test]
    fn trim_origin_drops_leading_host() {
        assert_eq!(trim_origin("web.whatsapp.com Photo"), "Photo");
        assert_eq!(trim_origin("youtube.com/ New upload"), "New upload");
        assert_eq!(trim_origin("web.whatsapp.com"), "web.whatsapp.com"); // lone token kept
        assert_eq!(trim_origin("Hello world"), "Hello world"); // not a host
        assert_eq!(trim_origin("etc. and so on"), "etc. and so on"); // trailing-dot word
    }

    /// End-to-end body cleanup: markup stripped AND origin trimmed.
    #[test]
    fn clean_body_strips_markup_and_origin() {
        let raw = "<a href=\"https://web.whatsapp.com/\">web.whatsapp.com</a>\n\nPhoto";
        assert_eq!(clean_body(raw), "Photo");
    }

    /// A themed icon name passes through; a `file://`/absolute path only counts
    /// when it exists (Chrome's dead scoped-temp logo must be rejected).
    #[test]
    fn usable_app_icon_name_and_paths() {
        assert_eq!(
            usable_app_icon("google-chrome"),
            Some("google-chrome".into())
        );
        assert_eq!(usable_app_icon(""), None);
        assert_eq!(
            usable_app_icon("file:///tmp/definitely/gone/logo.png"),
            None
        );
        assert_eq!(usable_app_icon("/no/such/path.png"), None);
        // An existing file resolves via both the bare path and the file:// URI.
        let mut f = std::env::temp_dir();
        f.push(format!("waverunner-icon-test-{}.png", std::process::id()));
        std::fs::write(&f, b"x").unwrap();
        let path = f.to_str().unwrap().to_owned();
        assert_eq!(usable_app_icon(&path), Some(path.clone()));
        assert_eq!(
            usable_app_icon(&format!("file://{path}")),
            Some(path.clone())
        );
        let _ = std::fs::remove_file(&f);
    }

    /// `offset_of` sums the heights before a card (used to shift the scroll when
    /// new cards land at the front) and clamps past the end to the total.
    #[test]
    fn offset_of_sums_and_clamps() {
        let h = [30.0, 50.0, 40.0, 60.0];
        assert_eq!(offset_of(&h, 0), 0.0);
        assert_eq!(offset_of(&h, 2), 80.0);
        assert_eq!(offset_of(&h, 4), 180.0); // full total
        assert_eq!(offset_of(&h, 9), 180.0); // past the end clamps to the total
        assert_eq!(offset_of(&[], 3), 0.0); // empty
    }

    /// Card height grows with the wrapped body and never shrinks below the tile.
    #[test]
    fn card_height_grows_with_body() {
        let none = card_height(&[]);
        let two = card_height(&["a".to_string(), "b".to_string()]);
        assert!(two > none);
        assert!(none >= ICON_SZ + 2.0 * CARD_PAD_Y - 0.01);
    }

    /// The persistence path maps each `ActiveNotification` to a `StoredNotification`
    /// (plain serde), JSON-encodes it, and reverses that on load. Guard the whole
    /// round-trip so a schema change that breaks it is caught in tests, not by a
    /// silently-empty history on the next reboot.
    #[test]
    fn history_round_trips_through_json() {
        let history = vec![sample(1, 1000, "First"), sample(2, 2000, "Second")];
        let stored: Vec<StoredNotification> =
            history.iter().map(StoredNotification::from).collect();
        let json = serde_json::to_string(&stored).expect("serialize history");
        let back: Vec<StoredNotification> =
            serde_json::from_str(&json).expect("deserialize history");
        let restored: Vec<ActiveNotification> = back.into_iter().map(Into::into).collect();
        assert_eq!(history, restored);
    }

    /// Transient notifications that have left the active set are reaped from
    /// history; active transient ones and all normal ones stay.
    #[test]
    fn stale_transient_ids_reaps_only_gone_transients() {
        let mut osd_active = sample(1, 1000, "Volume");
        osd_active.transient = Some(true);
        let mut osd_gone = sample(2, 2000, "Brightness");
        osd_gone.transient = Some(true);
        let normal_gone = sample(3, 3000, "Message"); // transient None
        let history = vec![osd_active, osd_gone, normal_gone];
        // Only id 1 is still active.
        let active: std::collections::HashSet<u32> = [1].into_iter().collect();
        let gone = stale_transient_ids(&history, &active);
        assert_eq!(gone, vec![2]); // the departed OSD only
    }

    /// A `default` action is detected (drives click-to-open); its absence leaves
    /// a body click inert.
    #[test]
    fn default_action_detection() {
        let with = vec![
            "default".to_string(),
            "Open".to_string(),
            "archive".to_string(),
            "Archive".to_string(),
        ];
        assert!(has_default_action(&with));
        let without = vec!["archive".to_string(), "Archive".to_string()];
        assert!(!has_default_action(&without));
        assert!(!has_default_action(&[]));
        // A lone unpaired "default" isn't a valid action (needs a label).
        assert!(!has_default_action(&["default".to_string()]));
    }

    /// Transient notifications are excluded from durable persistence; normal ones
    /// (and older-daemon `None`) are kept.
    #[test]
    fn transient_notifications_are_not_persisted() {
        let mut normal = sample(1, 1000, "Normal");
        normal.transient = Some(false);
        let mut osd = sample(2, 2000, "Volume");
        osd.transient = Some(true);
        let legacy = sample(3, 3000, "Legacy"); // transient defaults to None
        assert!(is_persistable(&normal));
        assert!(!is_persistable(&osd));
        assert!(is_persistable(&legacy));
    }

    /// The preview auto-show hold scales with urgency: low brief, normal the
    /// baseline, critical the longest (and an unknown urgency falls to normal).
    #[test]
    fn flash_hold_scales_with_urgency() {
        assert!(flash_hold(0) < flash_hold(1)); // low < normal
        assert_eq!(flash_hold(1), HOLD); // normal == baseline
        assert!(flash_hold(2) > flash_hold(1)); // critical > normal
        assert_eq!(flash_hold(9), HOLD); // unknown falls to normal
    }
}
