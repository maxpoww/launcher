//! THE MONITORS PAGE (Max, 2026-10-21; the lab
//! https://claude.ai/artifact/R8qrwi81LUvQnvVYuHjZJ1): the control panel's
//! page for the screens. A drawing of them, to arrange by dragging (their
//! sides stick to each other's, their ends line up) and to turn by a
//! corner; the screen picked in it named under it with its Resolution and
//! Scale; then, with more than one screen, how they work together —
//! Screens (Extend · Mirror · Isolated), where the Options bar and the Dock
//! are (Main screen · Every screen · Follows you) and what a closed lid
//! does.
//!
//! THE PAGE FIRST, THE FEATURES ONE BY ONE (the Launcher page's way, Max
//! 2026-10-20): every choice here is SAVED (`settings.json`'s `monitors`,
//! [`MonitorsChoice`]) and drawn. Applied so far: RESOLUTION AND SCALE, per
//! screen (Max, 2026-10-21: "now make resolution and scale work per
//! monitor") — the picked screen's, live, through the display settings'
//! own way ([`crate::display`]: a resolution is tried and goes back by
//! itself unless kept; a scale is made under a dissolve), the page showing
//! what the screen has.
//!
//! The arrangement is kept in the compositor's own terms: each screen's
//! top-left corner in logical pixels (pixels over scale), as Hyprland
//! places them, and its turn in quarters.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use tracing::info;

use crate::controls::{Choice, ControlView, Group, ScreenShape};
use crate::display::{GolemSettings, Monitor};
use crate::i18n::tr;
use crate::App;

/// Where one screen stands on the page: its corner (logical px) and turn.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub(crate) struct Placed {
    pub x: f64,
    pub y: f64,
    /// Quarter turns clockwise, in degrees (0, 90, 180, 270).
    #[serde(default)]
    pub rot: u16,
}

/// What the page has saved (not applied yet: see the module's note).
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub(crate) struct MonitorsChoice {
    /// The screen picked in the drawing (`Monitor::selector`).
    pub picked: Option<String>,
    /// Each screen's place, by selector.
    pub arrange: BTreeMap<String, Placed>,
    /// Screens: `extend` (absent), `mirror`, `isolated`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub screens: Option<String>,
    /// Where the Options bar is: `main` (absent), `every`, `follows`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub options: Option<String>,
    /// Where the dock is: `follows` (absent), `main`, `every`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub dock: Option<String>,
    /// A closed lid leaves the built-in ON (the switch off).
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub lid_keeps_built_in: bool,
}

impl MonitorsChoice {
    pub(crate) fn is_default(&self) -> bool {
        *self == Self::default()
    }
}

/// A box in logical px.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct Box2 {
    pub x: f64,
    pub y: f64,
    pub w: f64,
    pub h: f64,
}

impl Box2 {
    fn overlaps(&self, o: &Box2) -> bool {
        self.x < o.x + o.w && self.x + self.w > o.x && self.y < o.y + o.h && self.y + self.h > o.y
    }
}

/// Which side of the other screen a screen stands at.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Side {
    Right,
    Left,
    Below,
    Above,
}

/// How far apart Isolated screens are kept (logical px).
pub(crate) const ISOLATED_GAP: f64 = 240.0;

/// THE MAGNET: where a screen of `size` held at `raw` lands among `others`
/// — a side that comes within `reach` of one of theirs sticks to it (a
/// `gap` apart: 0 touching), and along that side its ends line up with
/// that screen's ends or middle when within `reach` of them. The side it
/// stuck to, and to which screen, when it did.
pub(crate) fn magnet(raw: Box2, others: &[Box2], reach: f64, gap: f64) -> (f64, f64, Option<(Side, usize)>) {
    let mut best: Option<(f64, Side, usize, f64)> = None;
    for (i, o) in others.iter().enumerate() {
        let facing_x = raw.y < o.y + o.h && raw.y + raw.h > o.y;
        let facing_y = raw.x < o.x + o.w && raw.x + raw.w > o.x;
        let sides = [
            (Side::Right, o.x + o.w + gap, (raw.x - (o.x + o.w + gap)).abs(), facing_x),
            (Side::Left, o.x - raw.w - gap, (raw.x + raw.w - (o.x - gap)).abs(), facing_x),
            (Side::Below, o.y + o.h + gap, (raw.y - (o.y + o.h + gap)).abs(), facing_y),
            (Side::Above, o.y - raw.h - gap, (raw.y + raw.h - (o.y - gap)).abs(), facing_y),
        ];
        for (side, at, dist, facing) in sides {
            if facing && dist < reach && best.is_none_or(|b| dist < b.0) {
                best = Some((dist, side, i, at));
            }
        }
    }
    let Some((_, side, i, at)) = best else {
        return (raw.x, raw.y, None);
    };
    let o = others[i];
    let (mut x, mut y) = (raw.x, raw.y);
    match side {
        Side::Right | Side::Left => {
            x = at;
            if let Some(&l) = [o.y, o.y + o.h - raw.h, o.y + (o.h - raw.h) / 2.0].iter().find(|&&l| (l - raw.y).abs() < reach) {
                y = l;
            }
        }
        Side::Below | Side::Above => {
            y = at;
            if let Some(&l) = [o.x, o.x + o.w - raw.w, o.x + (o.w - raw.w) / 2.0].iter().find(|&&l| (l - raw.x).abs() < reach) {
                x = l;
            }
        }
    }
    (x, y, Some((side, i)))
}

/// DROPPED: where a screen let go at `d` rests. Extend: touching — on the
/// side it stuck to, or slid to the nearest side of the nearest screen;
/// Isolated: where it was, unless it covers another (then just off it).
pub(crate) fn settle(d: Box2, others: &[Box2], isolated: bool) -> (f64, f64) {
    if others.is_empty() {
        return (d.x, d.y);
    }
    let gap = if isolated { ISOLATED_GAP } else { 0.0 };
    let free = |b: &Box2| !others.iter().any(|o| b.overlaps(o));
    if isolated && free(&d) {
        return (d.x, d.y);
    }
    // (Touching already, and over nothing: it stays.)
    if !isolated && free(&d) {
        let (x, y, side) = magnet(d, others, 1.0, 0.0);
        if side.is_some() {
            return (x, y);
        }
    }
    // Every side of every screen it could rest at, with at least a little
    // of the two sides facing; the nearest that covers nothing.
    let keep = 120.0_f64;
    let mut spots = Vec::new();
    for o in others {
        let cy = d.y.clamp(o.y - d.h + keep, o.y + o.h - keep);
        let cx = d.x.clamp(o.x - d.w + keep, o.x + o.w - keep);
        spots.push((o.x + o.w + gap, cy));
        spots.push((o.x - d.w - gap, cy));
        spots.push((cx, o.y + o.h + gap));
        spots.push((cx, o.y - d.h - gap));
    }
    spots.sort_by(|a, b| {
        let da = (a.0 - d.x).hypot(a.1 - d.y);
        let db = (b.0 - d.x).hypot(b.1 - d.y);
        da.total_cmp(&db)
    });
    let at = spots
        .iter()
        .copied()
        .find(|&(x, y)| free(&Box2 { x, y, ..d }))
        .unwrap_or(spots[0]);
    // (Lined up with what it rests against, when near.)
    let (x, y, _) = magnet(Box2 { x: at.0, y: at.1, ..d }, others, 120.0, gap);
    (x.round(), y.round())
}

/// A quarter turn (`by` 90 clockwise, -90 back) of a screen about its
/// middle: its new box.
pub(crate) fn turned(b: Box2, rot: u16, by: i32) -> (Box2, u16) {
    let rot = (i32::from(rot) + by).rem_euclid(360) as u16;
    let (w, h) = (b.h, b.w);
    (Box2 { x: b.x + (b.w - w) / 2.0, y: b.y + (b.h - h) / 2.0, w, h }, rot)
}

/// One screen as the page has it: the compositor's, with the page's
/// choices over it.
#[derive(Debug, Clone)]
pub(crate) struct PageScreen {
    pub sel: String,
    pub title: String,
    pub short: String,
    pub builtin: bool,
    /// Its resolution and scale, as the compositor has them.
    pub mode: (u32, u32),
    pub scale: f64,
    pub rot: u16,
    pub at: (f64, f64),
    pub sizes: Vec<(u32, u32)>,
    pub stops: Vec<f64>,
    /// The one in use (the compositor's focus).
    pub focused: bool,
}

impl PageScreen {
    /// Its box in logical px, turned.
    pub(crate) fn logical(&self) -> Box2 {
        let s = if self.scale > 0.0 { self.scale } else { 1.0 };
        let (w, h) = (f64::from(self.mode.0) / s, f64::from(self.mode.1) / s);
        let (w, h) = if self.rot.is_multiple_of(180) { (w, h) } else { (h, w) };
        Box2 { x: self.at.0, y: self.at.1, w, h }
    }
}

/// The screens as the page shows them.
pub(crate) fn page_screens(screens: &[Monitor], choice: &MonitorsChoice) -> Vec<PageScreen> {
    screens
        .iter()
        .map(|m| {
            let sel = m.selector();
            let builtin = crate::launcher_prefs::built_in(m);
            // (What the screen has: these two are applied, not only saved.)
            let mode = (m.mode.w, m.mode.h);
            let scale = m.scale;
            let placed = choice.arrange.get(&sel).copied();
            // The sizes: the one in use and the biggest few others.
            let mut sizes: Vec<(u32, u32)> = Vec::new();
            for md in &m.modes {
                if !sizes.contains(&(md.w, md.h)) {
                    sizes.push((md.w, md.h));
                }
            }
            sizes.sort_by(|a, b| (u64::from(b.0) * u64::from(b.1)).cmp(&(u64::from(a.0) * u64::from(a.1))));
            sizes.truncate(4);
            if !sizes.contains(&mode) {
                sizes.pop();
                sizes.push(mode);
            }
            // The scales: 100 %, the one in use, and the nearest to it.
            let all = crate::display::scale_stops(mode.0, mode.1, scale);
            let mut stops: Vec<f64> = all.clone();
            stops.sort_by(|a, b| (a - scale).abs().total_cmp(&(b - scale).abs()));
            stops.truncate(5);
            if !stops.iter().any(|s| (s - 1.0).abs() < 1e-6) && all.iter().any(|s| (s - 1.0).abs() < 1e-6) {
                stops.pop();
                stops.push(1.0);
            }
            stops.sort_by(f64::total_cmp);
            PageScreen {
                short: if builtin { tr("Built-in").to_owned() } else { m.title() },
                title: m.title(),
                sel,
                builtin,
                mode,
                scale,
                rot: placed.map_or(0, |p| p.rot),
                at: placed.map_or(m.pos, |p| (p.x, p.y)),
                sizes,
                stops,
                focused: m.focused,
            }
        })
        .collect()
}

/// Screens whose places were saved but whose sizes have changed since (a
/// new resolution or scale) are laid against the others again, so the
/// drawing never shows two overlapping, or (Extend) one standing apart:
/// the first screen stays, each other is settled in turn.
fn keep_together(screens: &mut [PageScreen], isolated: bool) {
    for i in 1..screens.len() {
        let others: Vec<Box2> = screens.iter().enumerate().filter(|(j, _)| *j != i).map(|(_, o)| o.logical()).collect();
        let b = screens[i].logical();
        let touching = magnet(b, &others, 1.0, if isolated { ISOLATED_GAP } else { 0.0 }).2.is_some();
        let overlaps = others.iter().any(|o| b.overlaps(o));
        if overlaps || (!isolated && !touching) {
            screens[i].at = settle(b, &others, isolated);
        }
    }
}

/// The screen picked in the drawing: the saved one (here, or on the Scale
/// and Resolution pages), else the one in use, as those pages set it.
fn picked<'a>(screens: &'a [PageScreen], choice: &MonitorsChoice) -> Option<&'a PageScreen> {
    choice
        .picked
        .as_ref()
        .and_then(|p| screens.iter().find(|s| &s.sel == p))
        .or_else(|| screens.iter().find(|s| s.focused))
        .or_else(|| screens.iter().find(|s| s.builtin))
        .or(screens.first())
}

/// The words of a three-way row, and which value each sends.
const SCREENS: [(&str, &str); 3] = [("Extend", "extend"), ("Mirror", "mirror"), ("Isolated", "isolated")];
const WHERE: [(&str, &str); 3] = [("Main screen", "main"), ("Every screen", "every"), ("Follows you", "follows")];

fn words(caption: &'static str, key: &'static str, list: &[(&'static str, &'static str)], lit: &str) -> Group {
    let choices = list.iter().map(|&(word, value)| choice(tr(word), value == lit, value)).collect();
    Group { labelled: true, ..crate::controls::group_of(Some(caption), key, choices) }
}

fn choice(text: impl Into<String>, on: bool, value: impl Into<String>) -> Choice {
    Choice { text: text.into(), on, value: value.into(), swatch: Vec::new(), caption: false, icon: None, hint: None }
}

/// The page (`controls::read("Monitors")`, on the controls' worker).
pub(crate) fn view(view: &mut ControlView) {
    let saved = GolemSettings::load().monitors;
    let monitors = crate::display::monitors();
    let mut screens = page_screens(&monitors, &saved);
    if !saved.arrange.is_empty() && saved.screens.as_deref() != Some("mirror") {
        keep_together(&mut screens, saved.screens.as_deref() == Some("isolated"));
    }
    view.subtitle = tr("How your screens work together.").to_owned();
    let Some(p) = picked(&screens, &saved) else {
        view.subtitle = tr("No screen to set.").to_owned();
        return;
    };
    let many = screens.len() > 1;
    let options = saved.options.as_deref().unwrap_or("main");
    let dock = saved.dock.as_deref().unwrap_or("follows");
    let mirrored = many && saved.screens.as_deref() == Some("mirror");
    // THE DRAWING: each screen a box (clicked: picked), the picked one's two
    // turning corners after them.
    let shapes: Vec<ScreenShape> = screens
        .iter()
        .map(|s| {
            let b = s.logical();
            ScreenShape {
                sel: s.sel.clone(),
                short: s.short.clone(),
                x: b.x,
                y: b.y,
                w: b.w,
                h: b.h,
                rot: s.rot,
                bar: !mirrored && (s.builtin || options == "every" || (!screens.iter().any(|x| x.builtin) && s.sel == screens[0].sel)),
                dock: !mirrored && (s.builtin || dock == "every" || (!screens.iter().any(|x| x.builtin) && s.sel == screens[0].sel)),
                picked: s.sel == p.sel,
                mirrored,
                isolated: saved.screens.as_deref() == Some("isolated"),
            }
        })
        .collect();
    let mut picks: Vec<Choice> = screens.iter().map(|s| choice(s.short.clone(), s.sel == p.sel, format!("pick:{}", s.sel))).collect();
    if !mirrored {
        picks.push(choice(tr("Turn back"), false, format!("ccw:{}", p.sel)));
        picks.push(choice(tr("Turn"), false, format!("cw:{}", p.sel)));
    }
    view.groups.push(Group { screens: shapes, ..crate::controls::group_of(None, "monitors-arrange", picks) });
    // ITS NAME under the drawing, and what it is set to.
    view.groups.push(Group { centred: true, ..crate::controls::line_of(p.title.clone()) });
    let sizes = p
        .sizes
        .iter()
        .map(|&(w, h)| choice(format!("{w} × {h}"), (w, h) == p.mode, format!("{w}x{h}")))
        .collect();
    view.groups.push(Group { labelled: true, ..crate::controls::group_of(Some("Resolution"), "monitors-mode", sizes) });
    let stops = p
        .stops
        .iter()
        .map(|&st| choice(format!("{}%", (st * 100.0).round() as u32), (st - p.scale).abs() < 1e-4, format!("{st}")))
        .collect();
    view.groups.push(Group { labelled: true, ..crate::controls::group_of(Some("Scale"), "monitors-scale", stops) });
    if !many {
        return;
    }
    view.groups.push(words("Screens", "monitors-screens", &SCREENS, saved.screens.as_deref().unwrap_or("extend")));
    if !mirrored {
        view.groups.push(words("Options", "monitors-options", &WHERE, options));
        view.groups.push(words("Dock", "monitors-dock", &WHERE, dock));
    }
    if screens.iter().any(|s| s.builtin) {
        let mut lid = crate::controls::on_off_of(Some("Lid closed"), "monitors-lid", !saved.lid_keeps_built_in);
        if let Some(on) = lid.choices.first_mut() {
            on.hint = Some(tr("The built-in turns off and everything goes to the other screen.").to_owned());
        }
        view.groups.push(lid);
    }
}

impl App {
    /// A pick on the Monitors page (`monitors-*` keys): saved, and the page
    /// read again. Nothing is applied to the screens yet.
    pub(crate) fn monitors_pick(&mut self, key: &str, value: &str) {
        let monitors = crate::display::monitors();
        // RESOLUTION AND SCALE are the picked screen's own, applied live
        // (the display settings' way: tried and kept, or under a dissolve).
        if key == "monitors-mode" || key == "monitors-scale" {
            let saved = GolemSettings::load().monitors;
            let screens = page_screens(&monitors, &saved);
            let Some(sel) = picked(&screens, &saved).map(|p| p.sel.clone()) else { return };
            info!("monitors: {key} = {value} on {sel}");
            if key == "monitors-mode" {
                let size = value.split_once('x').and_then(|(w, h)| Some((w.parse::<u32>().ok()?, h.parse::<u32>().ok()?)));
                if let Some((w, h)) = size {
                    self.set_display_mode_on(Some(&sel), w, h, None);
                }
            } else if let Ok(s) = value.parse::<f64>() {
                self.set_display_scale_on(Some(&sel), s);
            }
            return;
        }
        GolemSettings::change(|g| {
            let c = &mut g.monitors;
            let screens = page_screens(&monitors, c);
            let isolated = c.screens.as_deref() == Some("isolated");
            match key {
                "monitors-arrange" => {
                    let (verb, sel) = value.split_once(':').unwrap_or((value, ""));
                    match verb {
                        "pick" => c.picked = Some(sel.to_owned()),
                        "cw" | "ccw" => {
                            if let Some(s) = screens.iter().find(|s| s.sel == sel) {
                                let (b, rot) = turned(s.logical(), s.rot, if verb == "cw" { 90 } else { -90 });
                                let others: Vec<Box2> = screens.iter().filter(|o| o.sel != sel).map(PageScreen::logical).collect();
                                let (x, y) = settle(b, &others, isolated);
                                c.arrange.insert(sel.to_owned(), Placed { x, y, rot });
                                c.picked = Some(sel.to_owned());
                            }
                        }
                        _ => {}
                    }
                }
                "monitors-screens" => c.screens = (value != "extend").then(|| value.to_owned()),
                "monitors-options" => c.options = (value != "main").then(|| value.to_owned()),
                "monitors-dock" => c.dock = (value != "follows").then(|| value.to_owned()),
                "monitors-lid" => c.lid_keeps_built_in = value == "off",
                _ => return,
            }
            g.store();
        });
        info!("monitors: {key} = {value}");
    }

    /// The Monitors page, when it is open, read again in `after`: the
    /// compositor takes a moment to make a change of resolution or scale.
    pub(crate) fn monitors_read_soon(&mut self, after: std::time::Duration) {
        use smithay_client_toolkit::reexports::calloop::timer::{TimeoutAction, Timer};
        let open = |p: &crate::panel::Panel| p.open_control_label() == Some("Monitors");
        if !open(self.settings_panel()) && !open(&self.quick) {
            return;
        }
        let armed = self.loop_handle.insert_source(Timer::from_duration(after), |_, _, app: &mut App| {
            let ui = app.ui_facts();
            app.controls_send(crate::controls::ControlCommand::Read("Monitors", ui));
            TimeoutAction::Drop
        });
        if let Err(e) = armed {
            tracing::warn!("monitors: cannot read the page again ({e})");
        }
    }

    /// A screen dragged in the drawing and let go at `(x, y)` (logical px):
    /// it rests where the magnet and its sides say, saved; the page is read
    /// again.
    pub(crate) fn monitors_moved(&mut self, sel: &str, x: f64, y: f64) {
        let monitors = crate::display::monitors();
        GolemSettings::change(|g| {
            let c = &mut g.monitors;
            let screens = page_screens(&monitors, c);
            let Some(s) = screens.iter().find(|s| s.sel == sel) else { return };
            let others: Vec<Box2> = screens.iter().filter(|o| o.sel != sel).map(PageScreen::logical).collect();
            let d = Box2 { x, y, ..s.logical() };
            let (x, y) = settle(d, &others, c.screens.as_deref() == Some("isolated"));
            c.arrange.insert(sel.to_owned(), Placed { x, y, rot: s.rot });
            c.picked = Some(sel.to_owned());
            g.store();
            info!("monitors: {sel} arranged at {x},{y}");
        });
        let ui = self.ui_facts();
        self.controls_send(crate::controls::ControlCommand::Read("Monitors", ui));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const LAPTOP: Box2 = Box2 { x: 0.0, y: 0.0, w: 2000.0, h: 1250.0 };

    #[test]
    fn a_side_near_another_sticks_to_it_and_lines_up() {
        // Dropped a little off the laptop's right side, a little low.
        let held = Box2 { x: 2060.0, y: 40.0, w: 3840.0, h: 2160.0 };
        let (x, y, side) = magnet(held, &[LAPTOP], 100.0, 0.0);
        assert_eq!((x, y, side), (2000.0, 0.0, Some((Side::Right, 0))), "touching, tops lined up");
        // Far from every side: free.
        let far = Box2 { x: 2600.0, y: 900.0, ..held };
        assert_eq!(magnet(far, &[LAPTOP], 100.0, 0.0), (2600.0, 900.0, None));
    }

    #[test]
    fn dropped_anywhere_it_rests_against_the_nearest_side() {
        let held = Box2 { x: 300.0, y: 1700.0, w: 3840.0, h: 2160.0 };
        let (x, y) = settle(held, &[LAPTOP], false);
        assert_eq!(y, 1250.0, "below the laptop, touching");
        assert!(x < 2000.0 - 120.0 && x + 3840.0 > 120.0, "and facing it");
        // On top of it: off it.
        let over = Box2 { x: 200.0, y: 100.0, w: 3840.0, h: 2160.0 };
        let (x, y) = settle(over, &[LAPTOP], false);
        assert!(!Box2 { x, y, ..over }.overlaps(&LAPTOP));
    }

    #[test]
    fn isolated_screens_stay_apart() {
        let away = Box2 { x: 5000.0, y: 3000.0, w: 1920.0, h: 1080.0 };
        assert_eq!(settle(away, &[LAPTOP], true), (5000.0, 3000.0), "left where it was");
        let over = Box2 { x: 100.0, y: 100.0, ..away };
        let (x, y) = settle(over, &[LAPTOP], true);
        let b = Box2 { x, y, ..away };
        assert!(!b.overlaps(&LAPTOP));
        let gap = (b.x - (LAPTOP.x + LAPTOP.w)).max(LAPTOP.x - (b.x + b.w)).max(b.y - (LAPTOP.y + LAPTOP.h)).max(LAPTOP.y - (b.y + b.h));
        assert!((gap - ISOLATED_GAP).abs() < 1.0, "a gap apart");
    }

    #[test]
    fn a_turn_swaps_the_sides_about_the_middle_and_back() {
        let tv = Box2 { x: 2000.0, y: 0.0, w: 3840.0, h: 2160.0 };
        let (b, rot) = turned(tv, 0, 90);
        assert_eq!(rot, 90);
        assert_eq!((b.w, b.h), (2160.0, 3840.0));
        assert_eq!((b.x + b.w / 2.0, b.y + b.h / 2.0), (tv.x + tv.w / 2.0, tv.y + tv.h / 2.0));
        let (back, rot) = turned(b, rot, -90);
        assert_eq!((back, rot), (tv, 0));
        assert_eq!(turned(tv, 0, -90).1, 270);
    }

    #[test]
    fn nothing_chosen_is_nothing_saved() {
        assert!(MonitorsChoice::default().is_default());
        let json = serde_json::to_string(&MonitorsChoice::default()).unwrap();
        let back: MonitorsChoice = serde_json::from_str(&json).unwrap();
        assert_eq!(back, MonitorsChoice::default());
    }
}
