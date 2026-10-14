//! THE SIDE: windows thrown to the right edge (Max, 2026-10-14).
//!
//! A fast, short two-finger swipe on the [current task] pill THROWS the
//! focused window to the right edge of the screen, where it hangs small —
//! Seam's smallest window, 804×240, *"i like that size"* — with its left
//! 226 px showing: the title and enough of the content to know it. The first
//! hangs right under the bar, each next one flush under the one above (its
//! title bar in the gap), a column Max laid out by hand first and then asked
//! for. The workflow, his words: throw a few there; click one; right-click
//! the pill → it comes to the centre to be used; right-click again → it goes
//! back to its place on the side; click the next one…
//!
//! The model, settled with Max after the first cuts (2026-10-14, night):
//!
//! - **The strip is the column's.** Every floating window whose left edge
//!   stands at the column's x is of the column — thrown there by the pill or
//!   put there by hand (`side_adopt`: the first cut knew only what it had
//!   parked itself, and a thrown window landed on top of one placed by hand:
//!   *"it's like they are not aware that that spot is being used"*).
//! - **A place is the window's until it closes.** One called out, or dragged
//!   out, leaves its place EMPTY and comes back to it; a new window goes to
//!   the end; only a close takes a place away, and the ones below move up.
//!   One column a workspace.
//! - **Calling** is the pill's right-click: to the centre, at the size it
//!   had before it was parked, in front. The next right-click, a flick, or a
//!   drag back to the strip parks it again in its place, small. A window
//!   resized while out is parked with that size as its own.
//! - **Taking one in hand** — a pointer drag, or a scroll on the pill with it
//!   focused — is watched: come [`DRAG_OUT`] towards the centre it is out, its
//!   size back under the hand; let go short of that it snaps back. A pinch
//!   does nothing to a parked window.
//! - **Always on top, like a dock** (Max's pick over "under the working
//!   windows"): the column is raised again whenever anything else is, each
//!   window over the one above it, so in overflow — past the bottom the
//!   column overlaps evenly, never closer than a title bar — every bar shows.
//! - **The focus goes back** to the window the owner was working in when one
//!   is thrown, so the next gesture is not on the one just put away; and the
//!   pill's scroll is ignored while the throw settles (the fingers' tail end
//!   re-grabbed the parked window and dragged it straight back out).
//! - **Window memory** never learns the parked size or place: a parked window
//!   closing, or put down, is remembered as it was before it was parked.
//! - The columns are kept in the runtime dir and found again by a dock
//!   started within the same login.

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
/// A window this near its place, or the strip, is there.
const NEAR: i32 = 12;
/// A parked window brought this far towards the centre is out.
const DRAG_OUT: i32 = 170;
/// How often a parked window in hand is looked at.
const DRAG_POLL: Duration = Duration::from_millis(40);
/// A window moved by scroll has no "put down" of its own: still for this long
/// and it is down.
const STILL: Duration = Duration::from_millis(250);
/// After a throw the pill's scroll is ignored this long: the tail of the
/// swipe would re-grab the window just parked.
const SETTLE: Duration = Duration::from_millis(600);
/// At start, how long the found-again columns wait to be laid out: the bar
/// has to be up and have its strip of the screen reserved first.
const RESTORE_WAIT: Duration = Duration::from_millis(2500);

/// A window of the column's, and what it was before.
#[derive(Clone, Serialize, Deserialize)]
struct Parked {
    addr: String,
    /// Its size and place before it was parked (monitor-relative, as window
    /// memory keeps them): what it goes back to, and what is remembered of it.
    home: Place,
    /// Out of the column for now — called, or dragged out — its place kept.
    out: bool,
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

/// One column: a place a slot, top to bottom. `None` is an empty place (left
/// by a window put there by hand further down).
type Column = Vec<Option<Parked>>;

/// The side: a column a workspace.
#[derive(Default)]
pub(crate) struct Side {
    columns: HashMap<i64, Column>,
    dragging: Option<Dragging>,
    /// The last throw: the pill's scroll is ignored while it settles.
    thrown_at: Option<Instant>,
}

/// What survives a dock restart within the login (addresses are the
/// compositor's and last as long as the windows do).
#[derive(Default, Serialize, Deserialize)]
struct Saved {
    columns: HashMap<i64, Column>,
}

fn saved_path() -> PathBuf {
    PathBuf::from(std::env::var("XDG_RUNTIME_DIR").unwrap_or_else(|_| "/tmp".to_owned()))
        .join("waverunner-side.json")
}

impl Side {
    fn find(&self, addr: &str) -> Option<(i64, usize)> {
        self.columns.iter().find_map(|(&ws, col)| {
            col.iter()
                .position(|p| p.as_ref().is_some_and(|p| p.addr == addr))
                .map(|i| (ws, i))
        })
    }

    fn get(&self, addr: &str) -> Option<&Parked> {
        let (ws, i) = self.find(addr)?;
        self.columns[&ws][i].as_ref()
    }

    fn get_mut(&mut self, addr: &str) -> Option<&mut Parked> {
        let (ws, i) = self.find(addr)?;
        self.columns.get_mut(&ws)?[i].as_mut()
    }

    /// Of the column's, in or out.
    pub(crate) fn has(&self, addr: &str) -> bool {
        self.find(addr).is_some()
    }

    /// In the column now (not out).
    pub(crate) fn in_column(&self, addr: &str) -> bool {
        self.get(addr).is_some_and(|p| !p.out)
    }

    /// What a window in the column was before it was parked.
    pub(crate) fn home_of(&self, addr: &str) -> Option<Place> {
        self.get(addr).filter(|p| !p.out).map(|p| p.home)
    }

    /// Take a window out of the side for good: the ones below move up.
    fn remove(&mut self, addr: &str) -> Option<Parked> {
        let (ws, i) = self.find(addr)?;
        let col = self.columns.get_mut(&ws)?;
        let gone = col.remove(i);
        while col.last().is_some_and(Option::is_none) {
            col.pop();
        }
        if col.is_empty() {
            self.columns.remove(&ws);
        }
        gone
    }

    /// Give `addr` a place in `ws`'s column: `want` if that place is free (a
    /// window found by hand where it stands), else the first empty place,
    /// else a new one at the end.
    fn place(&mut self, ws: i64, want: Option<usize>, parked: Parked) -> usize {
        let col = self.columns.entry(ws).or_default();
        let slot = match want {
            Some(i) if i >= col.len() => {
                col.resize_with(i + 1, || None);
                i
            }
            Some(i) if col[i].is_none() => i,
            _ => match col.iter().position(Option::is_none) {
                Some(i) => i,
                None => {
                    col.push(None);
                    col.len() - 1
                }
            },
        };
        col[slot] = Some(parked);
        slot
    }

    fn save(&self) {
        let saved = Saved { columns: self.columns.clone() };
        crate::persist::write_json("the side", &saved_path(), &saved);
    }
}

/// Where a column's places are, top to bottom, given each place's height:
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

/// The place a window standing at `y` would have in a flush column.
fn slot_at_y(y: i32, mon: &hypr::MonitorInfo) -> usize {
    let top = mon.y + mon.reserved.1 + GAP;
    ((f64::from(y) - top) / (f64::from(PARK_H) + GAP)).round().max(0.0) as usize
}

/// One mapped window as the side sees it.
#[derive(Clone)]
struct Seen {
    class: String,
    x: i32,
    y: i32,
    w: i32,
    h: i32,
    /// Floating on a real workspace and not fullscreen: the only kind that
    /// can hang on the side.
    floating: bool,
    /// Mapped on a real workspace at all (a tiled window is a working one).
    working: bool,
    workspace: i64,
    /// The compositor's focus history: 0 is the focused window.
    focus_rank: i64,
}

fn seen() -> HashMap<String, Seen> {
    hypr::client_geometries()
        .into_iter()
        .map(|c| {
            (
                c.address,
                Seen {
                    class: c.class,
                    x: c.x.round() as i32,
                    y: c.y.round() as i32,
                    w: c.w.round() as i32,
                    h: c.h.round() as i32,
                    floating: c.floating && !c.special && !c.fullscreen,
                    working: !c.special,
                    workspace: c.workspace,
                    focus_rank: c.focus_rank,
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

fn resize_lua(addr: &str, w: i32, h: i32) -> String {
    format!("hl.dispatch(hl.dsp.window.resize({{ x = {w}, y = {h}, window = \"address:{addr}\" }})) ")
}

fn move_lua(addr: &str, x: i32, y: i32) -> String {
    format!("hl.dispatch(hl.dsp.window.move({{ x = {x}, y = {y}, window = \"address:{addr}\" }})) ")
}

fn zorder_lua(addr: &str, mode: &str) -> String {
    format!("hl.dispatch(hl.dsp.window.alter_zorder({{ mode = \"{mode}\", window = \"address:{addr}\" }})) ")
}

fn run_lua(lua: &str) {
    if !lua.is_empty() {
        hypr::dispatch(&format!("(function() {lua} return hl.dsp.no_op() end)()"));
    }
}

impl App {
    /// Throw a window to the side. New: a place at the end of its workspace's
    /// column, small, the focus handed back to the window the owner was in.
    /// Out: back into its own place. In the column: snapped into its place.
    /// Floating windows only, and not while the stage owns the screen.
    pub(crate) fn side_throw(&mut self, addr: &str) {
        if self.stage.is_on() {
            return;
        }
        if self.side.has(addr) {
            self.side_stop_watching(addr);
            self.side_park(addr);
            return;
        }
        let Ok(mon) = hypr::focused_monitor() else {
            return;
        };
        let here = seen();
        let Some(s) = here.get(addr).filter(|s| s.floating) else {
            debug!("side: {addr} is not a floating window; not thrown");
            return;
        };
        let home = Place {
            x: f64::from(s.x) - mon.x,
            y: f64::from(s.y) - mon.y,
            w: f64::from(s.w),
            h: f64::from(s.h),
        };
        let ws = s.workspace;
        let slot = self.side.place(ws, None, Parked { addr: addr.to_owned(), home, out: false });
        run_lua(&resize_lua(addr, PARK_W, PARK_H));
        info!(
            "side: {addr} thrown to place {slot} of workspace {ws} (was {}x{} at {},{})",
            home.w, home.h, home.x, home.y
        );
        self.side.thrown_at = Some(Instant::now());
        self.side_focus_back(ws, addr, &here);
        self.side_layout();
    }

    /// Whether the pill's scroll should be ignored: a throw is settling.
    pub(crate) fn side_settling(&self) -> bool {
        self.side.thrown_at.is_some_and(|t| t.elapsed() < SETTLE)
    }

    /// The window the owner was working in, once `thrown` is put away: the
    /// most recently focused one of the workspace that is not of the column.
    fn side_focus_back(&self, ws: i64, thrown: &str, here: &HashMap<String, Seen>) {
        let back = here
            .iter()
            .filter(|(a, s)| {
                a.as_str() != thrown && s.working && s.workspace == ws && !self.side.in_column(a)
            })
            .min_by_key(|(_, s)| s.focus_rank)
            .map(|(a, _)| a.clone());
        if let Some(back) = back {
            if let Err(e) = hypr::focus_window_direct(&back) {
                debug!("side: focus back to {back} failed: {e:#}");
            }
        }
    }

    /// Park a window of the column's that is out (or snap one that is in):
    /// what it is now is its home, it goes small, into its place.
    fn side_park(&mut self, addr: &str) {
        let Ok(mon) = hypr::focused_monitor() else {
            return;
        };
        let here = seen();
        let Some(s) = here.get(addr) else {
            return;
        };
        let was_out = self.side.get(addr).is_some_and(|p| p.out);
        if let Some(p) = self.side.get_mut(addr) {
            if p.out {
                p.home = Place {
                    x: f64::from(s.x) - mon.x,
                    y: f64::from(s.y) - mon.y,
                    w: f64::from(s.w),
                    h: f64::from(s.h),
                };
                p.out = false;
            }
        }
        if was_out {
            run_lua(&resize_lua(addr, PARK_W, PARK_H));
            info!("side: {addr} parked again in its place");
        }
        self.side_layout();
    }

    /// Put the columns in order: windows found standing in the strip are
    /// adopted; every window in a column is at its place, top to bottom, on
    /// the sizes they really have (an empty place, or one whose window is
    /// out, keeps the parked height); windows gone, or no longer floating,
    /// leave; then the whole side is raised. Windows that moved to another
    /// workspace go to that workspace's column.
    pub(crate) fn side_layout(&mut self) {
        let Ok(mon) = hypr::focused_monitor() else {
            return;
        };
        let mut here = seen();
        // Gone for good, or no longer floating: out of the side. (One missing
        // from the compositor's list is only on its way out: its close event
        // takes it out, and until then its home is still owed to window
        // memory.)
        let leaving: Vec<String> = self
            .side
            .columns
            .values()
            .flatten()
            .flatten()
            .filter(|p| here.get(&p.addr).is_some_and(|s| !s.floating))
            .map(|p| p.addr.clone())
            .collect();
        for addr in leaving {
            debug!("side: {addr} is no longer floating; it has left the side");
            self.side.remove(&addr);
        }
        // Moved to another workspace: to that workspace's column.
        let moved: Vec<(String, i64)> = self
            .side
            .columns
            .iter()
            .flat_map(|(&ws, col)| col.iter().flatten().map(move |p| (ws, p)))
            .filter_map(|(ws, p)| here.get(&p.addr).filter(|s| s.workspace != ws).map(|s| (p.addr.clone(), s.workspace)))
            .collect();
        for (addr, ws) in moved {
            if let Some(p) = self.side.remove(&addr) {
                self.side.place(ws, None, p);
            }
        }
        self.side_adopt(&mon, &mut here);
        let mut lua = String::new();
        for (&ws, col) in &self.side.columns {
            let _ = ws;
            let heights: Vec<i32> = col
                .iter()
                .map(|p| match p {
                    Some(p) if !p.out => here.get(&p.addr).map_or(PARK_H, |s| s.h),
                    _ => PARK_H,
                })
                .collect();
            let (places, _) = layout(&heights, &mon);
            for (p, &(x, y)) in col.iter().zip(&places) {
                let Some(p) = p.as_ref().filter(|p| !p.out) else {
                    continue;
                };
                // To the pixel: "near enough" left windows a few px off
                // their places after an overflow had spread them.
                if let Some(s) = here.get(&p.addr) {
                    if (s.x, s.y) != (x, y) {
                        lua.push_str(&move_lua(&p.addr, x, y));
                    }
                }
            }
        }
        run_lua(&lua);
        self.side_raise();
        self.side.save();
    }

    /// Windows standing in the strip that the side does not know — put there
    /// by hand, or parked before the side could remember — are adopted, each
    /// into the place where it stands if free, and made small. One already
    /// small has no size of its own to go back to: it goes back to what
    /// window memory keeps for its app, else to Golem's own float size.
    fn side_adopt(&mut self, mon: &hypr::MonitorInfo, here: &mut HashMap<String, Seen>) {
        let x = column_x(mon);
        let mut found: Vec<(String, Seen)> = here
            .iter()
            .filter(|(a, s)| s.floating && (s.x - x).abs() <= NEAR && !self.side.has(a))
            .map(|(a, s)| (a.clone(), s.clone()))
            .collect();
        if found.is_empty() {
            return;
        }
        found.sort_by_key(|(_, s)| s.y);
        let default = hypr::float_size().unwrap_or((PARK_W as i64 * 2, PARK_H as i64 * 3));
        let (ux, uy, uw, uh) = usable(mon);
        let mut lua = String::new();
        for (addr, s) in found {
            let small = (s.w - PARK_W).abs() <= 2 && (s.h - PARK_H).abs() <= 2;
            let home = if small {
                self.window_memory.place_of(&s.class).unwrap_or(Place {
                    x: ux - mon.x + (uw - default.0 as f64) / 2.0,
                    y: uy - mon.y + (uh - default.1 as f64) / 2.0,
                    w: default.0 as f64,
                    h: default.1 as f64,
                })
            } else {
                Place {
                    x: f64::from(s.x) - mon.x,
                    y: f64::from(s.y) - mon.y,
                    w: f64::from(s.w),
                    h: f64::from(s.h),
                }
            };
            let want = Some(slot_at_y(s.y, mon));
            let slot = self.side.place(s.workspace, want, Parked { addr: addr.clone(), home, out: false });
            if !small {
                lua.push_str(&resize_lua(&addr, PARK_W, PARK_H));
                if let Some(s) = here.get_mut(&addr) {
                    s.w = PARK_W;
                    s.h = PARK_H;
                }
            }
            info!("side: {addr} ({}) found in the strip, adopted into place {slot}", s.class);
        }
        run_lua(&lua);
    }

    /// The side over everything, like a dock: each column's windows raised
    /// from the top place down, so each lies over the one above it and all
    /// over the working windows. Whenever anything else is raised.
    pub(crate) fn side_raise(&self) {
        let mut lua = String::new();
        for col in self.side.columns.values() {
            for p in col.iter().flatten().filter(|p| !p.out) {
                lua.push_str(&zorder_lua(&p.addr, "top"));
            }
        }
        run_lua(&lua);
    }

    /// The pill's right-click on a window of the side's: one in its place is
    /// CALLED — to the centre, at the size it had, in front, its place kept
    /// empty; one out is parked again in its place. `false` for any other
    /// window (the pill's ordinary centre ↔ back then applies).
    pub(crate) fn side_click(&mut self, addr: &str) -> bool {
        let Some((out, home)) = self.side.get(addr).map(|p| (p.out, p.home)) else {
            return false;
        };
        if out {
            self.side_park(addr);
            return true;
        }
        self.side_stop_watching(addr);
        if let Some(p) = self.side.get_mut(addr) {
            p.out = true;
        }
        let Ok(mon) = hypr::focused_monitor() else {
            return true;
        };
        let (ux, uy, uw, uh) = usable(&mon);
        let (w, h) = (home.w.round() as i32, home.h.round() as i32);
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
        info!("side: {addr} called to the centre at {w}x{h}, its place kept");
        self.side.save();
        true
    }

    /// A window of the column's was taken in hand — by a pointer drag
    /// (`self_ending` false: the drag says when it is put down) or by a
    /// scroll on the pill (true: it is down once still) — watch it for
    /// leaving.
    pub(crate) fn side_drag_began(&mut self, addr: &str, self_ending: bool) {
        if !self.side.in_column(addr) || self.side.dragging.is_some() {
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

    /// One look at a window in hand: `true` to keep watching. Come
    /// [`DRAG_OUT`] towards the centre, it is out. Moved by scroll and still
    /// for [`STILL`], it is down.
    fn side_drag_watch(&mut self, addr: &str) -> bool {
        let Ok(mon) = hypr::focused_monitor() else {
            self.side.dragging = None;
            return false;
        };
        let (Some(s), true) = (seen().get(addr).cloned(), self.side.in_column(addr)) else {
            self.side.dragging = None;
            return false;
        };
        if s.x < column_x(&mon) - DRAG_OUT {
            self.side.dragging = None;
            self.side_let_out(addr);
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

    /// A window of the column's has come out under the hand: its size comes
    /// back, it comes to the front, its place waits for it.
    fn side_let_out(&mut self, addr: &str) {
        let Some(p) = self.side.get_mut(addr) else {
            return;
        };
        p.out = true;
        let (w, h) = (p.home.w.round() as i32, p.home.h.round() as i32);
        run_lua(&format!("{}{}", resize_lua(addr, w, h), zorder_lua(addr, "top")));
        info!("side: {addr} taken out by hand, back to {w}x{h}, its place kept");
        self.side.save();
    }

    fn side_stop_watching(&mut self, addr: &str) {
        if self.side.dragging.as_ref().is_some_and(|d| d.addr == addr) {
            self.side.dragging = None;
        }
    }

    /// A window was put down (a pointer drag's end, a pill drag's end, a
    /// scroll gone still). One of the column's far enough out is out; one
    /// short of that snaps back. One that is out, put down in the strip, is
    /// parked again. Any other, put down in the strip, is adopted by the
    /// layout that follows.
    pub(crate) fn side_drag_ended(&mut self, addr: &str) {
        self.side_stop_watching(addr);
        let (Ok(mon), Some(s)) = (hypr::focused_monitor(), seen().remove(addr)) else {
            return;
        };
        let in_strip = (s.x - column_x(&mon)).abs() <= NEAR;
        match self.side.get(addr) {
            Some(p) if !p.out => {
                if s.x < column_x(&mon) - DRAG_OUT {
                    self.side_let_out(addr);
                } else {
                    self.side_layout();
                }
            }
            Some(_) if in_strip => self.side_park(addr),
            Some(_) => {}
            None => {
                if in_strip {
                    self.side_layout();
                }
            }
        }
    }

    /// Something was focused or mapped: the side goes back on top.
    pub(crate) fn side_focus_changed(&mut self, _addr: &str) {
        if !self.side.columns.is_empty() {
            self.side_raise();
        }
    }

    /// A window closed: its place closes, the ones below move up.
    pub(crate) fn side_window_closed(&mut self, addr: &str) {
        self.side_stop_watching(addr);
        if self.side.remove(addr).is_some() {
            self.side_layout();
        }
    }

    /// At start: the columns a dock of this login left behind, for the
    /// windows still there (one out stays out).
    pub(crate) fn side_restore(&mut self) {
        let here = seen();
        if let Some(saved) = crate::persist::read_json::<Saved>(&saved_path()) {
            for (ws, col) in saved.columns {
                let kept: Column = col
                    .into_iter()
                    .map(|p| p.filter(|p| here.get(&p.addr).is_some_and(|s| s.floating)))
                    .collect();
                if kept.iter().any(Option::is_some) {
                    self.side.columns.insert(ws, kept);
                }
            }
            let n: usize = self.side.columns.values().map(|c| c.iter().flatten().count()).sum();
            if n > 0 {
                info!("side: {n} window(s) found again");
            }
        }
        // Laid out once the bar has its strip of the screen back: at start
        // the reserved area is not there yet, and the column went 31 px up
        // under where the bar would be.
        self.side_raise();
        let timer = calloop::timer::Timer::from_duration(RESTORE_WAIT);
        let _ = self.loop_handle.insert_source(timer, |_, _, app: &mut App| {
            app.side_layout();
            calloop::timer::TimeoutAction::Drop
        });
    }

    /// Without a touchpad: `debug-desktop "side throw|call|park <addr>|layout|state"`.
    /// `throw`, `call` and `park` act on the window named, else on the
    /// focused one — name it from a rig: a focus asked for can fail to take,
    /// and the verb then acts on whatever the owner has in front.
    pub(crate) fn side_debug(&mut self, what: &str) -> String {
        let (verb, named) = match what.split_once(' ') {
            Some((v, a)) => (v, Some(a.trim().to_owned()).filter(|a| !a.is_empty())),
            None => (what, None),
        };
        match verb {
            "throw" | "call" | "park" => match named.or_else(hypr::active_window) {
                Some(addr) => {
                    match verb {
                        "throw" => self.side_throw(&addr),
                        "park" if self.side.has(&addr) => self.side_park(&addr),
                        "call" if self.side_click(&addr) => {}
                        _ => return format!("{addr} is not on the side"),
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
                let mut out = Vec::new();
                let mut wss: Vec<&i64> = self.side.columns.keys().collect();
                wss.sort();
                for ws in wss {
                    out.push(format!("workspace {ws}:"));
                    for (i, p) in self.side.columns[ws].iter().enumerate() {
                        out.push(match p {
                            Some(p) => format!(
                                "  {i}: {}{} (home {}x{} at {},{})",
                                p.addr,
                                if p.out { " OUT" } else { "" },
                                p.home.w,
                                p.home.h,
                                p.home.x,
                                p.home.y
                            ),
                            None => format!("  {i}: -"),
                        });
                    }
                }
                out.join("\n")
            }
            _ => "side throw|call|park [addr]|layout|state".to_owned(),
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

    fn parked(addr: &str) -> Parked {
        Parked { addr: addr.into(), home: Place { x: 1.0, y: 2.0, w: 3.0, h: 4.0 }, out: false }
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
        // A window standing where a place would be is read as that place.
        assert_eq!(slot_at_y(56, &mon()), 0);
        assert_eq!(slot_at_y(590, &mon()), 2);
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
        let (many, _) = layout(&[240; 80], &mon());
        assert_eq!(many[1].1 - many[0].1, MIN_STEP as i32);
    }

    #[test]
    fn a_place_is_the_windows_until_it_closes() {
        let mut side = Side::default();
        assert_eq!(side.place(4, None, parked("a")), 0);
        assert_eq!(side.place(4, None, parked("b")), 1);
        assert_eq!(side.place(4, None, parked("c")), 2);
        // b called out: its place is kept, c stays where it is.
        side.get_mut("b").unwrap().out = true;
        assert!(side.has("b") && !side.in_column("b"));
        assert_eq!(side.home_of("b"), None, "not in the column while out");
        assert_eq!(side.find("c"), Some((4, 2)), "c did not move up");
        assert_eq!(side.place(4, None, parked("d")), 3, "a new one goes to the end");
        // b closes: its place goes, the ones below move up.
        side.remove("b");
        assert_eq!(side.find("c"), Some((4, 1)));
        assert_eq!(side.find("d"), Some((4, 2)));
        // Another workspace has its own column.
        assert_eq!(side.place(2, None, parked("e")), 0);
    }

    #[test]
    fn a_window_found_by_hand_takes_the_place_where_it_stands() {
        let mut side = Side::default();
        assert_eq!(side.place(4, Some(2), parked("y")), 2, "two empty places above it");
        assert_eq!(side.place(4, Some(0), parked("s")), 0);
        assert_eq!(side.place(4, Some(0), parked("a")), 1, "its place taken: the first empty one");
        assert_eq!(side.place(4, None, parked("n")), 3, "a throw goes to the end");
        side.remove("y");
        assert_eq!(side.find("n"), Some((4, 2)));
        side.remove("n");
        side.remove("a");
        side.remove("s");
        assert!(side.columns.is_empty(), "an empty column is gone");
    }
}
