//! THE SIDE: windows thrown to the right edge (Max, 2026-10-14).
//!
//! A fast, short two-finger swipe on the [current task] pill THROWS the
//! focused window to the right edge of the screen, where it hangs with only
//! its left strip showing — the title and the first words of each line — like
//! a card tucked into a sleeve. Max laid six of them out by hand that night
//! (160 px in from the edge, each about 85 px lower than the one above) and
//! said: *"the first one goes to the top, the next window thrown there goes a
//! little lower and so on, so i can have them there"*. The workflow, his
//! words: throw a few there; click one; right-click the pill → it comes to
//! the centre to be used; right-click again → it goes back to its place on the
//! side; click the next one… The centre ↔ back half is the pill's own
//! right-click (`nub_drag.rs`, `click_move`), which remembers where a window
//! was centred FROM — its slot — so nothing here has to.
//!
//! - A window keeps its slot while it is away at the centre (the next throw
//!   does not take it) and until it closes; one thrown again goes back to its
//!   own slot. A window carried off its slot by hand has left the side, and
//!   its slot is free again (`side_tidy`).
//! - The side is FANNED: each window lies under the one below it, and all of
//!   them under the working windows, so every strip can be seen and clicked.
//!   The compositor raises what is clicked, which would cover the strips below
//!   it, so the fan is put back whenever a window of the side is focused — and
//!   after a throw, and after a return. One brought to the centre comes to the
//!   front there.
//! - The throw is judged when the swipe STOPS, the card's way (`card/place.rs`):
//!   a short run, one way, far enough — `nub_drag.rs::Fling`. A long or slow
//!   scroll is the plain drag it always was.
//! - A window closing on the side closed where it was parked, not where it is
//!   used: window memory does not take that as the app's place.

use std::collections::HashMap;

use tracing::{debug, info};

use crate::{hypr, App};

/// How much of a window shows, in from the right edge (logical px).
const PEEK: f64 = 160.0;
/// Each slot this much lower than the one above.
const STEP: f64 = 84.0;
/// The first slot's top, below the usable area's top: the title bar (25) and
/// the border (3) stand above the window's box, plus a hair.
const FIRST: f64 = 30.0;
/// A window this near its slot is on it.
const AT_SLOT: i32 = 12;

/// The side: which window hangs in which slot.
#[derive(Default)]
pub(crate) struct Side {
    /// Slot → the window there (`None`: free).
    slots: Vec<Option<String>>,
}

impl Side {
    pub(crate) fn slot_of(&self, addr: &str) -> Option<usize> {
        self.slots.iter().position(|s| s.as_deref() == Some(addr))
    }

    pub(crate) fn has(&self, addr: &str) -> bool {
        self.slot_of(addr).is_some()
    }

    /// The first free slot, opened at the bottom if there is none.
    fn take_free(&mut self, addr: &str) -> usize {
        let slot = match self.slots.iter().position(Option::is_none) {
            Some(i) => i,
            None => {
                self.slots.push(None);
                self.slots.len() - 1
            }
        };
        self.slots[slot] = Some(addr.to_owned());
        slot
    }

    fn forget(&mut self, addr: &str) {
        if let Some(i) = self.slot_of(addr) {
            self.slots[i] = None;
        }
        while self.slots.last().is_some_and(Option::is_none) {
            self.slots.pop();
        }
    }
}

/// Where slot `slot` is on `mon`: the window's top-left corner, layout px.
/// Past the bottom the slots start again from the top.
pub(crate) fn slot_at(slot: usize, mon: &hypr::MonitorInfo) -> (i32, i32) {
    let (_, t, r, b) = mon.reserved;
    let usable_h = (mon.h - t - b).max(1.0);
    let capacity = (((usable_h - FIRST) / STEP).floor() as usize).max(1);
    let x = mon.x + mon.w - r - PEEK;
    let y = mon.y + t + FIRST + STEP * (slot % capacity) as f64;
    (x.round() as i32, y.round() as i32)
}

fn near(a: (i32, i32), b: (i32, i32)) -> bool {
    (a.0 - b.0).abs() <= AT_SLOT && (a.1 - b.1).abs() <= AT_SLOT
}

/// Every mapped window: its top-left corner, and whether it floats on a real
/// workspace (the only kind that can hang on the side).
fn positions() -> HashMap<String, (i32, i32, bool)> {
    hypr::client_geometries()
        .into_iter()
        .map(|c| {
            (c.address, (c.x.round() as i32, c.y.round() as i32, c.floating && !c.special))
        })
        .collect()
}

impl App {
    /// Throw a window to the side: its own slot if it has one, else the first
    /// free one. Floating windows only, and not while the stage owns the
    /// screen.
    pub(crate) fn side_throw(&mut self, addr: &str) {
        if self.stage.is_on() {
            return;
        }
        let Ok(mon) = hypr::focused_monitor() else {
            return;
        };
        let here = positions();
        match here.get(addr) {
            Some(&(_, _, true)) => {}
            _ => {
                debug!("side: {addr} is not a floating window; not thrown");
                return;
            }
        }
        self.side_tidy(&mon, &here);
        let slot = match self.side.slot_of(addr) {
            Some(s) => s,
            None => self.side.take_free(addr),
        };
        let (x, y) = slot_at(slot, &mon);
        crate::nub_drag::move_window(addr, x, y);
        info!("side: {addr} thrown to slot {slot} at ({x}, {y})");
        self.side_fan();
    }

    /// Drop the windows that have left the side: closed, no longer floating,
    /// or carried off their slot by hand. One away at the centre by the pill's
    /// right-click — its remembered home IS the slot — stays.
    fn side_tidy(&mut self, mon: &hypr::MonitorInfo, here: &HashMap<String, (i32, i32, bool)>) {
        let gone: Vec<String> = self
            .side
            .slots
            .iter()
            .enumerate()
            .filter_map(|(i, s)| {
                let addr = s.as_deref()?;
                let slot = slot_at(i, mon);
                let stays = match here.get(addr) {
                    Some(&(x, y, true)) => {
                        near((x, y), slot)
                            || self.nub_home.get(addr).is_some_and(|&h| near(h, slot))
                    }
                    _ => false,
                };
                (!stays).then(|| addr.to_owned())
            })
            .collect();
        for addr in gone {
            debug!("side: {addr} has left the side");
            self.side.forget(&addr);
        }
    }

    /// Lay the side out in order: each window under the one below it, all of
    /// them under the working windows. Only windows ON their slot are put in
    /// the order; one away at the centre is left where it is.
    pub(crate) fn side_fan(&mut self) {
        let Ok(mon) = hypr::focused_monitor() else {
            return;
        };
        let here = positions();
        self.side_tidy(&mon, &here);
        let mut lua = String::new();
        for (i, s) in self.side.slots.iter().enumerate().rev() {
            let Some(addr) = s.as_deref() else {
                continue;
            };
            let on_slot = here
                .get(addr)
                .is_some_and(|&(x, y, _)| near((x, y), slot_at(i, &mon)));
            if on_slot {
                lua.push_str(&format!(
                    "hl.dispatch(hl.dsp.window.alter_zorder({{ mode = \"bottom\", window = \"address:{addr}\" }})) "
                ));
            }
        }
        if !lua.is_empty() {
            hypr::dispatch(&format!("(function() {lua} return hl.dsp.no_op() end)()"));
        }
    }

    /// A window was focused: if it hangs on the side the compositor just
    /// raised it over the strips below — put the fan back.
    pub(crate) fn side_focus_changed(&mut self, addr: &str) {
        if self.side.has(addr) {
            self.side_fan();
        }
    }

    /// The pill's right-click moved a window of the side: back home — the fan
    /// is put back; to the centre — it comes to the front there.
    pub(crate) fn side_after_click(&mut self, addr: &str, went_home: bool) {
        if !self.side.has(addr) {
            return;
        }
        if went_home {
            self.side_fan();
        } else {
            hypr::dispatch(&format!(
                "hl.dsp.window.alter_zorder({{ mode = \"top\", window = \"address:{addr}\" }})"
            ));
        }
    }

    /// A window closed: its slot is free.
    pub(crate) fn side_window_closed(&mut self, addr: &str) {
        self.side.forget(addr);
    }

    /// Without a touchpad: `debug-desktop "side throw|fan|state"`.
    pub(crate) fn side_debug(&mut self, what: &str) -> String {
        match what {
            "throw" => match hypr::active_window() {
                Some(addr) => {
                    self.side_throw(&addr);
                    format!("{addr} thrown")
                }
                None => "no focused window".to_owned(),
            },
            "fan" => {
                self.side_fan();
                "fanned".to_owned()
            }
            "state" => self
                .side
                .slots
                .iter()
                .enumerate()
                .map(|(i, s)| format!("{i}: {}", s.as_deref().unwrap_or("-")))
                .collect::<Vec<_>>()
                .join("\n"),
            _ => "side throw|fan|state".to_owned(),
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
    fn slots_hang_from_the_top_under_the_bar_and_step_down() {
        assert_eq!(slot_at(0, &mon()), (2400, 61), "as Max placed the first one");
        assert_eq!(slot_at(1, &mon()), (2400, 145));
        // 18 fit; the 19th starts again at the top.
        assert_eq!(slot_at(18, &mon()), slot_at(0, &mon()));
    }

    #[test]
    fn a_freed_slot_is_taken_first_and_the_tail_is_trimmed() {
        let mut side = Side::default();
        assert_eq!(side.take_free("a"), 0);
        assert_eq!(side.take_free("b"), 1);
        assert_eq!(side.take_free("c"), 2);
        side.forget("a");
        assert_eq!(side.slot_of("b"), Some(1), "the others stay put");
        assert_eq!(side.take_free("d"), 0, "the hole is filled first");
        side.forget("c");
        assert_eq!(side.slots.len(), 2, "the tail is trimmed");
        assert!(!side.has("c"));
    }
}
