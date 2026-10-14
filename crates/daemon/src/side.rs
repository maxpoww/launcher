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
//! - **Dragging one out**: a pointer drag of a parked window is watched; once
//!   it has come [`DRAG_OUT`] towards the centre it is let go — its size comes
//!   back under the hand, the column closes up — and it is a plain window
//!   again. Put down near its place instead, it snaps back into the column.
//! - **Window memory** never learns the parked size: a window closing on the
//!   side is remembered at the size and place it had before it was thrown.
//! - A thrown window goes under the working windows; a called one comes to
//!   the front. The throw is judged when the swipe STOPS, the card's way
//!   (`nub_drag.rs::Fling`): a short run, far enough, never turned back. A
//!   long or slow scroll is the plain pill drag it always was.

use std::collections::HashMap;
use std::time::Duration;

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
/// A window this near its place is on it.
const NEAR: i32 = 12;
/// A parked window dragged this far towards the centre has left the side.
const DRAG_OUT: i32 = 100;
/// How often a parked window in hand is looked at.
const DRAG_POLL: Duration = Duration::from_millis(40);

/// A window on the side, and what it was before.
struct Parked {
    addr: String,
    /// Its size and place before it was thrown (monitor-relative, as window
    /// memory keeps them): what it goes back to, and what is remembered of it.
    home: Place,
}

/// The side: the column, top to bottom, and what has been called out of it.
#[derive(Default)]
pub(crate) struct Side {
    parked: Vec<Parked>,
    /// Called out to the centre by the pill (address → its home): the pill's
    /// next right-click parks it again.
    out: HashMap<String, Place>,
    /// A parked window in hand (a pointer drag), watched for leaving.
    dragging: Option<String>,
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
}

/// Where the column's windows go, top to bottom, given each one's height:
/// their top-left corners in layout px. Flush: each one's title bar sits in
/// the gap under the one above.
pub(crate) fn layout(heights: &[i32], mon: &hypr::MonitorInfo) -> Vec<(i32, i32)> {
    let (_, t, r, _) = mon.reserved;
    let x = (mon.x + mon.w - r - PEEK).round() as i32;
    let mut y = mon.y + t + GAP;
    heights
        .iter()
        .map(|&h| {
            let at = (x, y.round() as i32);
            y += f64::from(h) + GAP;
            at
        })
        .collect()
}

fn near(a: (i32, i32), b: (i32, i32)) -> bool {
    (a.0 - b.0).abs() <= NEAR && (a.1 - b.1).abs() <= NEAR
}

/// One mapped window as the side sees it: where it is, how big, and whether
/// it floats on a real workspace (the only kind that can hang on the side).
#[derive(Clone, Copy)]
struct Seen {
    x: i32,
    y: i32,
    w: i32,
    h: i32,
    floating: bool,
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
                    floating: c.floating && !c.special,
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
    /// home it had. Floating windows only, and not while the stage owns the
    /// screen.
    pub(crate) fn side_throw(&mut self, addr: &str) {
        if self.stage.is_on() {
            return;
        }
        let Ok(mon) = hypr::focused_monitor() else {
            return;
        };
        if self.side.has(addr) {
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
            "(function() local a = \"address:{addr}\" \
             hl.dispatch(hl.dsp.window.resize({{ x = {PARK_W}, y = {PARK_H}, window = a }})) \
             hl.dispatch(hl.dsp.window.alter_zorder({{ mode = \"bottom\", window = a }})) \
             return hl.dsp.no_op() end)()"
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

    /// Put the column in order: every parked window at its place, top to
    /// bottom, on the sizes they really have — the ones below a spot that
    /// opened move up. Windows that are gone leave the column.
    pub(crate) fn side_layout(&mut self) {
        let Ok(mon) = hypr::focused_monitor() else {
            return;
        };
        let here = seen();
        self.side.parked.retain(|p| here.contains_key(&p.addr));
        let heights: Vec<i32> = self.side.parked.iter().map(|p| here[&p.addr].h).collect();
        let places = layout(&heights, &mon);
        for (p, (x, y)) in self.side.parked.iter().zip(places) {
            let s = here[&p.addr];
            if !near((s.x, s.y), (x, y)) {
                crate::nub_drag::move_window(&p.addr, x, y);
            }
        }
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

    /// A pointer drag began on a parked window: watch it for leaving.
    pub(crate) fn side_drag_began(&mut self, addr: &str) {
        if !self.side.has(addr) || self.side.dragging.is_some() {
            return;
        }
        self.side.dragging = Some(addr.to_owned());
        let timer = calloop::timer::Timer::from_duration(DRAG_POLL);
        let _ = self.loop_handle.insert_source(timer, |_, _, app: &mut App| {
            let Some(addr) = app.side.dragging.clone() else {
                return calloop::timer::TimeoutAction::Drop;
            };
            if app.side_drag_watch(&addr) {
                calloop::timer::TimeoutAction::ToDuration(DRAG_POLL)
            } else {
                app.side.dragging = None;
                calloop::timer::TimeoutAction::Drop
            }
        });
    }

    /// One look at a parked window in hand: `true` to keep watching. Come
    /// [`DRAG_OUT`] towards the centre, it is let go: its size comes back
    /// under the hand, and the column closes up behind it.
    fn side_drag_watch(&mut self, addr: &str) -> bool {
        let Ok(mon) = hypr::focused_monitor() else {
            return false;
        };
        let here = seen();
        let (Some(s), Some(i)) = (here.get(addr).copied(), self.side.index_of(addr)) else {
            return false;
        };
        let heights: Vec<i32> = self.side.parked.iter().map(|p| here.get(&p.addr).map_or(PARK_H, |s| s.h)).collect();
        let (px, _) = layout(&heights, &mon)[i];
        if s.x > px - DRAG_OUT {
            return true;
        }
        let Some(p) = self.side.remove(addr) else {
            return false;
        };
        let (w, h) = (p.home.w.round() as i32, p.home.h.round() as i32);
        hypr::dispatch(&format!(
            "hl.dsp.window.resize({{ x = {w}, y = {h}, window = \"address:{addr}\" }})"
        ));
        info!("side: {addr} dragged out, back to {w}x{h}");
        self.side_layout();
        false
    }

    /// The drag ended: a window still parked is snapped back into its place.
    pub(crate) fn side_drag_ended(&mut self, addr: &str) {
        if self.side.dragging.as_deref() == Some(addr) {
            self.side.dragging = None;
        }
        if self.side.has(addr) {
            self.side_layout();
        }
    }

    /// A window closed: its spot closes, and nothing is owed to it.
    pub(crate) fn side_window_closed(&mut self, addr: &str) {
        self.side.out.remove(addr);
        if self.side.dragging.as_deref() == Some(addr) {
            self.side.dragging = None;
        }
        if self.side.remove(addr).is_some() {
            self.side_layout();
        }
    }

    /// Without a touchpad: `debug-desktop "side throw|call|layout|state"`
    /// (`throw` and `call` act on the focused window).
    pub(crate) fn side_debug(&mut self, what: &str) -> String {
        match what {
            "throw" | "call" => match hypr::active_window() {
                Some(addr) => {
                    if what == "throw" {
                        self.side_throw(&addr);
                    } else if !self.side_click(&addr) {
                        return format!("{addr} is not on the side");
                    }
                    format!("{addr} {what}")
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
            vec![(2334, 56), (2334, 321), (2334, 586)],
            "his five webapps: 226 in, 265 apart"
        );
        // A window that would not go that small takes the room it has.
        assert_eq!(layout(&[240, 400, 240], &mon()), vec![(2334, 56), (2334, 321), (2334, 746)]);
        assert!(layout(&[], &mon()).is_empty());
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
