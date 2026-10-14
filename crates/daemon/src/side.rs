//! THE SIDE: windows thrown to the right edge (Max, 2026-10-14).
//!
//! A fast, short two-finger swipe on the [current task] pill THROWS the
//! focused window to the right edge of the screen, where it hangs small —
//! Seam's smallest window, 804×240, *"i like that size"* — with its left
//! 226 px showing: the title and enough of the content to know it. The first
//! hangs right under the bar, each next one flush under the one above (its
//! title bar in the gap), a column Max laid out by hand first and then asked
//! for: *"throw a window there, change its size, but the size restores when
//! i call it back"*.
//!
//! - **Calling one** is the pill's right-click: the window comes to the centre
//!   at the size it had before it was thrown, in front; the ones below it
//!   move up to close its spot. The next right-click parks it again, at the
//!   bottom of the column. (For any other window the pill's right-click is
//!   still `nub_drag.rs::click_move`, centre ↔ back.)
//! - **Taking one in hand** — a pointer drag of it, or a scroll on the pill
//!   while it is focused — is watched: come [`DRAG_OUT`] towards the centre it
//!   is let go, its size back under the hand, in front, the column closing up
//!   behind it, a plain window again. Let go short of that it snaps back into
//!   its place (Max: *"it goes behind another small one, it's like they are
//!   not aware that that spot is being used"*). A pinch on the pill does
//!   nothing to a parked window: its size is the column's.
//! - **One column a workspace**, laid out on the sizes the windows really
//!   have. **Overflow**: when the column would run off the bottom the windows
//!   overlap, each one over the one above it, spaced so that every title bar
//!   still shows whole (Max: *"start overlapping on top of the top one,
//!   always showing the buttons"*); clicking one raises it, and the order is
//!   put back so the bars below it show again.
//! - **Window memory** never learns the parked size or place: a parked window
//!   closing, or put down, is remembered as it was before it was thrown.
//! - The column is kept in the runtime dir and found again by a dock started
//!   within the same login, so a restart does not lose the spots.
//! - A thrown window goes under the working windows; a called one comes to
//!   the front. The throw is judged when the swipe STOPS, the card's way
//!   (`nub_drag.rs::Fling`): a short run, far enough, never turned back. A
//!   long or slow scroll is the plain pill drag it always was.

use std::collections::HashMap;
use std::path::PathBuf;
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};
use tracing::{debug, info};

use crate::window_memory::Place;
use crate::{hypr, App};

/// How much of a parked window shows, in from the right edge (logical px).
const PEEK: f64 = 226.0;
/// The parked size: Seam's smallest window. A window whose own minimum is
/// bigger keeps it; the column is laid out on the sizes they really have.
const PARK_W: i32 = 804;
const PARK_H: i32 = 240;
/// Between two parked windows: the lower one's title bar, which stands above
/// its box. The first one's title bar is the same distance under the bar.
const GAP: f64 = 25.0;
/// In overflow the windows are never closer than this: the title bar (25),
/// the border (3) and a hair, so every bar shows whole.
const MIN_STEP: f64 = 32.0;
/// A window this near its place is on it.
const NEAR: i32 = 12;
/// A parked window brought this far towards the centre has left the side
/// (100 at first; Max: "closer to the center to restore, not a lot but some
/// more").
const DRAG_OUT: i32 = 170;
/// How often a parked window in hand is looked at.
const DRAG_POLL: Duration = Duration::from_millis(40);
/// A window moved by scroll has no "put down" of its own: still for this long
/// and it is down.
const STILL: Duration = Duration::from_millis(250);

/// A window on the side, and what it was before.
#[derive(Clone, Serialize, Deserialize)]
struct Parked {
    addr: String,
    /// Its size and place before it was thrown (monitor-relative, as window
    /// memory keeps them): what it goes back to, and what is remembered of it.
    home: Place,
}

/// A parked window in hand.
struct Dragging {
    addr: String,
    /// Moved by scroll: it is down once it has been still for [`STILL`].
    /// A pointer drag says when it is put down.
    self_ending: bool,
    last: (i32, i32),
    still_since: Instant,
}

/// The side: the column, top to bottom, and what has been called out of it.
#[derive(Default)]
pub(crate) struct Side {
    parked: Vec<Parked>,
    /// Called out to the centre by the pill (address → its home): the pill's
    /// next right-click parks it again.
    out: HashMap<String, Place>,
    dragging: Option<Dragging>,
}

/// What survives a dock restart within the login (addresses are the
/// compositor's and last as long as the windows do).
#[derive(Default, Serialize, Deserialize)]
struct Saved {
    parked: Vec<Parked>,
    out: HashMap<String, Place>,
}

fn saved_path() -> PathBuf {
    PathBuf::from(std::env::var("XDG_RUNTIME_DIR").unwrap_or_else(|_| "/tmp".to_owned()))
        .join("waverunner-side.json")
}

impl Side {
    fn index_of(&self, addr: &str) -> Option<usize> {
        self.parked.iter().position(|p| p.addr == addr)
    }

    pub(crate) fn has(&self, addr: &str) -> bool {
        self.index_of(addr).is_some()
    }

    /// What a parked window was before it was thrown.
    pub(crate) fn home_of(&self, addr: &str) -> Option<Place> {
        self.parked.iter().find(|p| p.addr == addr).map(|p| p.home)
    }

    fn remove(&mut self, addr: &str) -> Option<Parked> {
        let i = self.index_of(addr)?;
        Some(self.parked.remove(i))
    }

    fn save(&self) {
        let saved = Saved { parked: self.parked.clone(), out: self.out.clone() };
        crate::persist::write_json("the side", &saved_path(), &saved);
    }
}

/// Where the column's windows go, top to bottom, given each one's height:
/// their top-left corners in layout px, and whether they had to overlap.
/// Flush — each one's title bar in the gap under the one above — while that
/// fits; past the bottom they overlap, each over the one above, evenly, never
/// closer than [`MIN_STEP`].
pub(crate) fn layout(heights: &[i32], mon: &hypr::MonitorInfo) -> (Vec<(i32, i32)>, bool) {
    let (_, t, r, b) = mon.reserved;
    let x = (mon.x + mon.w - r - PEEK).round() as i32;
    let top = mon.y + t + GAP;
    let bottom = mon.y + mon.h - b;
    let n = heights.len();
    if n == 0 {
        return (Vec::new(), false);
    }
    let flush: Vec<f64> = heights
        .iter()
        .scan(top, |y, &h| {
            let at = *y;
            *y += f64::from(h) + GAP;
            Some(at)
        })
        .collect();
    let last_h = f64::from(heights[n - 1]);
    if flush[n - 1] + last_h <= bottom || n == 1 {
        return (flush.iter().map(|&y| (x, y.round() as i32)).collect(), false);
    }
    let step = ((bottom - last_h - top) / (n - 1) as f64).max(MIN_STEP);
    (
        (0..n).map(|i| (x, (top + step * i as f64).round() as i32)).collect(),
        true,
    )
}

/// The column's left edge on `mon`.
fn column_x(mon: &hypr::MonitorInfo) -> i32 {
    (mon.x + mon.w - mon.reserved.2 - PEEK).round() as i32
}

fn near(a: (i32, i32), b: (i32, i32)) -> bool {
    (a.0 - b.0).abs() <= NEAR && (a.1 - b.1).abs() <= NEAR
}

/// One mapped window as the side sees it.
#[derive(Clone, Copy)]
struct Seen {
    x: i32,
    y: i32,
    w: i32,
    h: i32,
    /// Floating on a real workspace and not fullscreen: the only kind that
    /// can hang on the side.
    floating: bool,
    workspace: i64,
}

fn seen() -> HashMap<String, Seen> {
    hypr::client_geometries()
        .into_iter()
        .map(|c| {
            (
                c.address,
                Seen {
                    x: c.x.round() as i32,
                    y: c.y.round() as i32,
                    w: c.w.round() as i32,
                    h: c.h.round() as i32,
                    floating: c.floating && !c.special && !c.fullscreen,
                    workspace: c.workspace,
                },
            )
        })
        .collect()
}

/// The usable area's top-left corner and size, layout px.
fn usable(mon: &hypr::MonitorInfo) -> (f64, f64, f64, f64) {
    let (l, t, r, b) = mon.reserved;
    (mon.x + l, mon.y + t, (mon.w - l - r).max(1.0), (mon.h - t - b).max(1.0))
}

impl App {
    /// Throw a window to the side: it goes small and under the others, at
    /// the bottom of the column. One called out before goes back with the
    /// home it had; one already parked snaps back into its place. Floating
    /// windows only, and not while the stage owns the screen.
    pub(crate) fn side_throw(&mut self, addr: &str) {
        if self.stage.is_on() {
            return;
        }
        let Ok(mon) = hypr::focused_monitor() else {
            return;
        };
        if self.side.has(addr) {
            self.side_stop_watching(addr);
            self.side_layout();
            return;
        }
        let Some(s) = seen().get(addr).copied().filter(|s| s.floating) else {
            debug!("side: {addr} is not a floating window; not thrown");
            return;
        };
        let home = self.side.out.remove(addr).unwrap_or(Place {
            x: f64::from(s.x) - mon.x,
            y: f64::from(s.y) - mon.y,
            w: f64::from(s.w),
            h: f64::from(s.h),
        });
        self.side.parked.push(Parked { addr: addr.to_owned(), home });
        hypr::dispatch(&format!(
            "hl.dsp.window.resize({{ x = {PARK_W}, y = {PARK_H}, window = \"address:{addr}\" }})"
        ));
        info!(
            "side: {addr} thrown (was {}x{} at {},{}), {} on the side",
            home.w,
            home.h,
            home.x,
            home.y,
            self.side.parked.len()
        );
        self.side_layout();
    }

    /// Put the columns in order — one a workspace: every parked window at its
    /// place, top to bottom, on the sizes they really have (the ones below a
    /// spot that opened move up), each under the one below it and all under
    /// the working windows. Windows that are gone, or no longer floating,
    /// leave the column.
    pub(crate) fn side_layout(&mut self) {
        let Ok(mon) = hypr::focused_monitor() else {
            return;
        };
        let here = seen();
        // A window no longer floating (fullscreen, minimized, tiled) leaves
        // the column. One MISSING from the compositor's list is only on its
        // way out: its close event takes it out, and until then its home is
        // still owed to window memory — a burst of closes had the first
        // one's layout drop the rest before their events came, and their
        // parked size was remembered as the app's place (2026-10-14).
        self.side
            .parked
            .retain(|p| here.get(&p.addr).is_none_or(|s| s.floating));
        let mut lua = String::new();
        let mut workspaces: Vec<i64> = Vec::new();
        for p in &self.side.parked {
            let Some(s) = here.get(&p.addr) else {
                continue;
            };
            if !workspaces.contains(&s.workspace) {
                workspaces.push(s.workspace);
            }
        }
        for ws in workspaces {
            let group: Vec<&Parked> = self
                .side
                .parked
                .iter()
                .filter(|p| here.get(&p.addr).is_some_and(|s| s.workspace == ws))
                .collect();
            let heights: Vec<i32> = group.iter().map(|p| here[&p.addr].h).collect();
            let (places, _) = layout(&heights, &mon);
            for (p, &(x, y)) in group.iter().zip(&places) {
                let s = here[&p.addr];
                if !near((s.x, s.y), (x, y)) {
                    lua.push_str(&format!(
                        "hl.dispatch(hl.dsp.window.move({{ x = {x}, y = {y}, window = \"address:{}\" }})) ",
                        p.addr
                    ));
                }
            }
            // The first one at the very bottom, each next one just above it.
            for p in group.iter().rev() {
                lua.push_str(&format!(
                    "hl.dispatch(hl.dsp.window.alter_zorder({{ mode = \"bottom\", window = \"address:{}\" }})) ",
                    p.addr
                ));
            }
        }
        if !lua.is_empty() {
            hypr::dispatch(&format!("(function() {lua} return hl.dsp.no_op() end)()"));
        }
        self.side.save();
    }

    /// Whether the column `addr` hangs in had to overlap.
    fn side_overlapping(&self, addr: &str, here: &HashMap<String, Seen>, mon: &hypr::MonitorInfo) -> bool {
        let Some(ws) = here.get(addr).map(|s| s.workspace) else {
            return false;
        };
        let heights: Vec<i32> = self
            .side
            .parked
            .iter()
            .filter_map(|p| here.get(&p.addr))
            .filter(|s| s.workspace == ws)
            .map(|s| s.h)
            .collect();
        layout(&heights, mon).1
    }

    /// The pill's right-click on a window of the side's: a parked one is
    /// CALLED — to the centre, at the size it had, in front, its spot closed
    /// behind it; a called one is parked again. `false` for any other window
    /// (the pill's ordinary centre ↔ back then applies).
    pub(crate) fn side_click(&mut self, addr: &str) -> bool {
        if self.side.out.contains_key(addr) {
            self.side_throw(addr);
            return true;
        }
        if !self.side.has(addr) {
            return false;
        }
        self.side_stop_watching(addr);
        let Some(p) = self.side.remove(addr) else {
            return false;
        };
        let Ok(mon) = hypr::focused_monitor() else {
            return true;
        };
        let (ux, uy, uw, uh) = usable(&mon);
        let (w, h) = (p.home.w.round() as i32, p.home.h.round() as i32);
        hypr::dispatch(&format!(
            "(function() local a = \"address:{addr}\" \
             hl.dispatch(hl.dsp.window.resize({{ x = {w}, y = {h}, window = a }})) \
             local win = hl.get_window(a) \
             local sw, sh = {w}, {h} \
             if win and win.size then sw, sh = win.size.x, win.size.y end \
             hl.dispatch(hl.dsp.window.move({{ x = math.floor({ux} + ({uw} - sw) / 2 + 0.5), y = math.floor({uy} + ({uh} - sh) / 2 + 0.5), window = a }})) \
             hl.dispatch(hl.dsp.window.alter_zorder({{ mode = \"top\", window = a }})) \
             return hl.dsp.no_op() end)()"
        ));
        info!("side: {addr} called to the centre at {w}x{h}");
        self.side.out.insert(addr.to_owned(), p.home);
        self.side_layout();
        true
    }

    /// A parked window was taken in hand — by a pointer drag (`self_ending`
    /// false: the drag says when it is put down) or by a scroll on the pill
    /// (true: it is down once still) — watch it for leaving.
    pub(crate) fn side_drag_began(&mut self, addr: &str, self_ending: bool) {
        if !self.side.has(addr) || self.side.dragging.is_some() {
            return;
        }
        let at = seen().get(addr).map_or((0, 0), |s| (s.x, s.y));
        self.side.dragging = Some(Dragging {
            addr: addr.to_owned(),
            self_ending,
            last: at,
            still_since: Instant::now(),
        });
        let timer = calloop::timer::Timer::from_duration(DRAG_POLL);
        let _ = self.loop_handle.insert_source(timer, |_, _, app: &mut App| {
            let Some(addr) = app.side.dragging.as_ref().map(|d| d.addr.clone()) else {
                return calloop::timer::TimeoutAction::Drop;
            };
            if app.side_drag_watch(&addr) {
                calloop::timer::TimeoutAction::ToDuration(DRAG_POLL)
            } else {
                calloop::timer::TimeoutAction::Drop
            }
        });
    }

    /// One look at a parked window in hand: `true` to keep watching. Come
    /// [`DRAG_OUT`] towards the centre, it is let go. Moved by scroll and
    /// still for [`STILL`], it is down.
    fn side_drag_watch(&mut self, addr: &str) -> bool {
        let Ok(mon) = hypr::focused_monitor() else {
            self.side.dragging = None;
            return false;
        };
        let (Some(s), true) = (seen().get(addr).copied(), self.side.has(addr)) else {
            self.side.dragging = None;
            return false;
        };
        if s.x < column_x(&mon) - DRAG_OUT {
            self.side.dragging = None;
            self.side_release(addr);
            return false;
        }
        let Some(d) = self.side.dragging.as_mut() else {
            return false;
        };
        if d.last != (s.x, s.y) {
            d.last = (s.x, s.y);
            d.still_since = Instant::now();
        } else if d.self_ending && d.still_since.elapsed() >= STILL {
            self.side_drag_ended(addr);
            return false;
        }
        true
    }

    /// A parked window has left the side under the hand: its size comes
    /// back, it comes to the front, the column closes up behind it.
    fn side_release(&mut self, addr: &str) {
        let Some(p) = self.side.remove(addr) else {
            return;
        };
        let (w, h) = (p.home.w.round() as i32, p.home.h.round() as i32);
        hypr::dispatch(&format!(
            "(function() local a = \"address:{addr}\" \
             hl.dispatch(hl.dsp.window.resize({{ x = {w}, y = {h}, window = a }})) \
             hl.dispatch(hl.dsp.window.alter_zorder({{ mode = \"top\", window = a }})) \
             return hl.dsp.no_op() end)()"
        ));
        info!("side: {addr} taken out, back to {w}x{h}");
        self.side_layout();
    }

    fn side_stop_watching(&mut self, addr: &str) {
        if self.side.dragging.as_ref().is_some_and(|d| d.addr == addr) {
            self.side.dragging = None;
        }
    }

    /// A window in hand was put down (a pointer drag's end, a pill drag's
    /// end, a scroll gone still): far enough out it is let go, else it snaps
    /// back into its place.
    pub(crate) fn side_drag_ended(&mut self, addr: &str) {
        self.side_stop_watching(addr);
        if !self.side.has(addr) {
            return;
        }
        let far = match (hypr::focused_monitor(), seen().get(addr)) {
            (Ok(mon), Some(s)) => s.x < column_x(&mon) - DRAG_OUT,
            _ => false,
        };
        if far {
            self.side_release(addr);
        } else {
            self.side_layout();
        }
    }

    /// A window was focused: a parked one in an overlapping column was raised
    /// by the compositor over the bars below it — the order is put back.
    pub(crate) fn side_focus_changed(&mut self, addr: &str) {
        if !self.side.has(addr) {
            return;
        }
        let Ok(mon) = hypr::focused_monitor() else {
            return;
        };
        if self.side_overlapping(addr, &seen(), &mon) {
            self.side_layout();
        }
    }

    /// A window closed: its spot closes, and nothing is owed to it.
    pub(crate) fn side_window_closed(&mut self, addr: &str) {
        self.side.out.remove(addr);
        self.side_stop_watching(addr);
        if self.side.remove(addr).is_some() {
            self.side_layout();
        }
    }

    /// At start: the column a dock of this login left behind, for the windows
    /// still there and still parked (floating, at the column's edge).
    pub(crate) fn side_restore(&mut self) {
        let Some(saved) = crate::persist::read_json::<Saved>(&saved_path()) else {
            return;
        };
        let Ok(mon) = hypr::focused_monitor() else {
            return;
        };
        let here = seen();
        let x = column_x(&mon);
        self.side.parked = saved
            .parked
            .into_iter()
            .filter(|p| {
                here.get(&p.addr)
                    .is_some_and(|s| s.floating && (s.x - x).abs() <= NEAR)
            })
            .collect();
        self.side.out = saved
            .out
            .into_iter()
            .filter(|(addr, _)| here.contains_key(addr))
            .collect();
        if !self.side.parked.is_empty() || !self.side.out.is_empty() {
            info!(
                "side: {} parked, {} called out, found again",
                self.side.parked.len(),
                self.side.out.len()
            );
        }
        self.side_layout();
    }

    /// Without a touchpad: `debug-desktop "side throw|call [addr]|layout|state"`.
    /// `throw` and `call` act on the window named, else on the focused one —
    /// name it from a rig: a focus asked for can fail to take, and the verb
    /// then acts on whatever the owner has in front (it threw his file
    /// manager, twice).
    pub(crate) fn side_debug(&mut self, what: &str) -> String {
        let (verb, named) = match what.split_once(' ') {
            Some((v, a)) => (v, Some(a.trim().to_owned()).filter(|a| !a.is_empty())),
            None => (what, None),
        };
        match verb {
            "throw" | "call" => match named.or_else(hypr::active_window) {
                Some(addr) => {
                    if verb == "throw" {
                        self.side_throw(&addr);
                    } else if !self.side_click(&addr) {
                        return format!("{addr} is not on the side");
                    }
                    format!("{addr} {verb}")
                }
                None => "no focused window".to_owned(),
            },
            "layout" => {
                self.side_layout();
                "laid out".to_owned()
            }
            "state" => {
                let column: Vec<String> = self
                    .side
                    .parked
                    .iter()
                    .enumerate()
                    .map(|(i, p)| format!("{i}: {} (home {}x{} at {},{})", p.addr, p.home.w, p.home.h, p.home.x, p.home.y))
                    .collect();
                let out: Vec<&str> = self.side.out.keys().map(String::as_str).collect();
                format!("parked:\n{}\nout: {}", column.join("\n"), out.join(" "))
            }
            _ => "side throw|call|layout|state".to_owned(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn mon() -> hypr::MonitorInfo {
        hypr::MonitorInfo {
            x: 0.0,
            y: 0.0,
            w: 2560.0,
            h: 1600.0,
            reserved: (0.0, 31.0, 0.0, 0.0),
            active_ws: 1,
            scale: 1.25,
            name: "eDP-1".into(),
        }
    }

    #[test]
    fn the_column_is_flush_under_the_bar_as_max_laid_it_out() {
        assert_eq!(
            layout(&[240, 240, 240], &mon()),
            (vec![(2334, 56), (2334, 321), (2334, 586)], false),
            "his five webapps: 226 in, 265 apart"
        );
        // A window that would not go that small takes the room it has.
        assert_eq!(layout(&[240, 400, 240], &mon()).0, vec![(2334, 56), (2334, 321), (2334, 746)]);
        assert_eq!(layout(&[], &mon()), (vec![], false));
    }

    #[test]
    fn past_the_bottom_the_windows_overlap_evenly_with_every_bar_showing() {
        let five = layout(&[240; 5], &mon());
        assert!(!five.1, "five fit flush (the last ends at 1381)");
        let (eight, overlapping) = layout(&[240; 8], &mon());
        assert!(overlapping);
        assert_eq!(eight[0], (2334, 56), "the first stays under the bar");
        assert_eq!(eight[7].1 + 240, 1600, "the last ends at the bottom");
        let step = eight[1].1 - eight[0].1;
        assert!(step < 265 && step >= MIN_STEP as i32, "evenly, each bar whole: {step}");
        assert!(eight.windows(2).all(|w| (w[1].1 - w[0].1 - step).abs() <= 1), "to the pixel");
        // Far too many: never closer than a bar, even past the bottom.
        let (many, _) = layout(&[240; 80], &mon());
        assert_eq!(many[1].1 - many[0].1, MIN_STEP as i32);
    }

    #[test]
    fn a_called_window_leaves_the_column_and_comes_back_at_the_bottom() {
        let home = Place { x: 1.0, y: 2.0, w: 3.0, h: 4.0 };
        let mut side = Side::default();
        for a in ["a", "b", "c"] {
            side.parked.push(Parked { addr: a.into(), home });
        }
        let b = side.remove("b").expect("parked");
        side.out.insert(b.addr, b.home);
        assert_eq!(side.index_of("c"), Some(1), "c moved up");
        assert!(side.out.contains_key("b"));
        assert_eq!(side.home_of("b"), None, "not parked while out");
        side.parked.push(Parked { addr: "b".into(), home: side.out.remove("b").unwrap() });
        assert_eq!(side.index_of("b"), Some(2), "parked again at the bottom");
        assert_eq!(side.home_of("b"), Some(home), "with the home it had");
    }
}
