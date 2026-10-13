//! The desktop's right-click menu (the mockup at `~/desktop-menu-mockup`,
//! Max's "build the menu", 2026-10-07): a small panel of the dock's box
//! material at the pointer, one band per row, no icons, no shortcut hints.
//! On an icon: Open · Open in terminal · Rename · then Move to Home · Move to bin · then Properties (on hover the
//! menu becomes the Properties box). On bare
//! wallpaper: New folder · Clean up (and Paste, when files are on the
//! clipboard).
//!
//! "Move to" does not act: it turns the menu's page (`Menu::show_targets`)
//! to where the selection can GO — ‹ Back · Cut · Copy · the things plugged
//! in or paired (a stick, a phone) · the other computers on the network
//! (Max, 2026-10-08: *"click on that change the box content… cut, copy, and
//! a list of connected devices like sticks, phones, and then a list of
//! network connected devices like another pcs"*). The lists are the
//! desktop's (`desktop_send.rs`); the menu only shows them.
//!
//! Pure here: what the rows are, where they sit, which one is under a
//! point, and how they are drawn into a [`Scene`]. The desktop (`desktop.rs`)
//! opens and closes it and acts on a row.

use crate::content::{GridContent, Label, Rect, RectInst, Scene, FONT_BOLD};
use crate::i18n::tr;

/// What a row does.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Action {
    Open,
    OpenTerminal,
    Rename,
    /// A little box of what there is to know about the item.
    Properties,
    /// Turn the page to where the selection can go (see the module doc).
    MoveTo,
    /// …and back to the first page.
    Back,
    /// Onto the clipboard, as a file manager's cut or copy.
    Cut,
    Copy,
    /// The clipboard's files, onto the desktop.
    Paste,
    /// To the `n`th place of the page's list (`Menu::targets`).
    SendTo(usize),
    /// A plugged-in volume (a stick, a phone): let it go.
    Eject,
    /// A phone: its screen in a window here (scrcpy).
    Mirror,
    /// A phone as this computer's camera (its lens, its picture and its
    /// flash are chosen on the bar's camera pill)…
    Camera,
    /// …and off again.
    CameraStop,
    /// A phone: its new photos and videos, to this computer's Pictures.
    ImportPhotos,
    /// A phone: its internet for this computer over the cable, on or off.
    Tether,
    /// A phone: the box where what this computer does with it is set.
    Configure,
    /// A phone that is the camera: its flash, lens and picture (a page of
    /// the bar's phone pill).
    CameraSettings,
    /// A phone that does not let this computer in over adb: how USB
    /// debugging is turned on, or this computer allowed (same place).
    HelpDebugging,
    /// A phone that shows no files: how it is made to (same place).
    HelpFiles,
    /// Out of the desktop, into the home folder.
    MoveToHome,
    MoveToBin,
    NewFolder,
    CleanUp,
    /// A shortcut that came from elsewhere: let it run from now on.
    AllowRun,
}

/// One row: an action, or a rule between groups.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Row {
    Item {
        label: &'static str,
        action: Action,
        /// Drawn in red: it throws something away.
        danger: bool,
    },
    /// A place the selection can go, by its own name (a stick's label, a
    /// phone's name).
    Target { label: String, action: Action },
    /// A line that only says something ("Looking for devices…").
    Note(&'static str),
    Sep,
}

/// The menu's width.
pub(crate) const WIDTH: f32 = 192.0;
/// Padding inside the panel, around the rows.
const PAD: f32 = 6.0;
/// A row's height, and a rule's.
const ROW_H: f32 = 30.0;
const SEP_H: f32 = 11.0;
/// A row's hover band's corner; the panel's is the boxes' (`MenuPaint::radius`).
const ROW_RADIUS: f32 = 7.0;
/// Text: size and inset from the row's left edge.
const FONT_PX: f32 = 13.0;
const LINE_PX: f32 = 17.0;
const TEXT_X: f32 = 10.0;
/// How far from the pointer the panel's corner sits.
const GAP: f32 = 6.0;
/// A resting row's ink sits a little under full, so a hovered row (full,
/// bold) has somewhere to go — the boxes' rule.
const REST_INK: f32 = 0.86;
/// A line that only says something: the ink, dim.
const NOTE_INK: f32 = 0.5;
/// A rule between groups: the ink, faint.
const LINE_INK: f32 = 0.14;
/// The one colour of its own: a row that throws something away, and its
/// hover band.
const RED: [f32; 4] = [224.0 / 255.0, 82.0 / 255.0, 82.0 / 255.0, 1.0];
const HOVER_DANGER: [f32; 4] = [224.0 / 255.0, 82.0 / 255.0, 82.0 / 255.0, 0.16];

/// What the menu is painted with: the OPTIONS boxes' adaptive surface
/// (`App::box_surface_at`), read where the menu is.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct MenuPaint {
    /// The panel.
    pub fill: [f32; 4],
    /// Text that reads on it.
    pub ink: [f32; 4],
    /// A hovered row's band.
    pub wash: [f32; 4],
    /// The panel's corner.
    pub radius: f32,
}

/// The rows for a right-click on an item (`on_item`) or on bare wallpaper.
/// `many`: the click was on one of SEVERAL selected items — only what makes
/// sense for a group is offered (no terminal, no rename, no properties).
pub(crate) fn rows(on_item: bool, many: bool) -> Vec<Row> {
    if on_item && many {
        vec![
            Row::Item { label: tr("Open"), action: Action::Open, danger: false },
            Row::Sep,
            Row::Item { label: tr("Move to"), action: Action::MoveTo, danger: false },
            Row::Item { label: tr("Move to Home"), action: Action::MoveToHome, danger: false },
            Row::Item { label: tr("Move to bin"), action: Action::MoveToBin, danger: true },
        ]
    } else if on_item {
        vec![
            Row::Item { label: tr("Open"), action: Action::Open, danger: false },
            Row::Item { label: tr("Open in terminal"), action: Action::OpenTerminal, danger: false },
            Row::Sep,
            Row::Item { label: tr("Rename"), action: Action::Rename, danger: false },
            Row::Sep,
            Row::Item { label: tr("Move to"), action: Action::MoveTo, danger: false },
            Row::Item { label: tr("Move to Home"), action: Action::MoveToHome, danger: false },
            Row::Item { label: tr("Move to bin"), action: Action::MoveToBin, danger: true },
            Row::Sep,
            // Last, and opened by HOVER: the menu turns into the box.
            Row::Item { label: tr("Properties"), action: Action::Properties, danger: false },
        ]
    } else {
        vec![
            Row::Item { label: tr("New folder"), action: Action::NewFolder, danger: false },
            Row::Sep,
            Row::Item { label: tr("Clean up"), action: Action::CleanUp, danger: false },
        ]
    }
}

/// What a plugged-in volume's menu is made from.
#[derive(Debug, Clone, PartialEq, Default)]
pub(crate) struct Volume {
    /// A phone: its screen can be mirrored, its camera used.
    pub phone: bool,
    /// A phone that is plugged in but not open (locked, charge only): no
    /// folder to open a terminal in, nothing to eject.
    pub closed: bool,
    /// That phone is being used as a camera.
    pub camera: bool,
    /// That phone is giving this computer its internet.
    pub tether: bool,
    /// What that phone cannot do here, and why (`phone_pill.rs` finds out):
    /// the rows that would not work are not offered, and the way to them is.
    pub limits: Limits,
}

/// What a plugged-in phone cannot do with this computer.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub(crate) struct Limits {
    /// It does not let this computer in over adb: USB debugging is off…
    pub debugging_off: bool,
    /// …or on, and the phone has not allowed this computer yet.
    pub not_allowed: bool,
    /// It is mounted and shows no files (locked, or on "charge only").
    pub no_files: bool,
    /// Its Android is older than 12: its camera cannot be used.
    pub old_camera: bool,
}

impl Limits {
    /// Nothing that goes over adb works.
    pub fn no_adb(&self) -> bool {
        self.debugging_off || self.not_allowed
    }
}

/// The rows of a plugged-in volume's menu.
pub(crate) fn volume_rows(v: &Volume) -> Vec<Row> {
    let item = |label: &'static str, action: Action| Row::Item { label, action, danger: false };
    let mut rows = vec![item(tr("Open"), Action::Open)];
    if !v.closed {
        rows.push(item(tr("Open in terminal"), Action::OpenTerminal));
    }
    // A phone. ONE list for its menu on the desktop and for the box of the
    // bar's phone pill (Max, 2026-10-11: "the box and the menu are not the
    // same… the menu is offering me to use the camera, but not the pill":
    // the box left out what would not work and the menu did not). What a
    // phone cannot do here is not offered in either; why, and the way to
    // it, is said in both.
    if v.phone {
        let no_adb = v.limits.no_adb();
        if !no_adb {
            rows.push(item(tr("Mirror screen"), Action::Mirror));
            // The camera is a group of its own (Max, 2026-10-09). On: its
            // Stop, and its settings (flash, lens, picture).
            rows.push(Row::Sep);
            if v.limits.old_camera {
                rows.push(Row::Note(tr("Use as camera: needs Android 12 or newer")));
            } else if v.camera {
                rows.push(item(tr("Stop camera"), Action::CameraStop));
                rows.push(item(tr("Configure camera"), Action::CameraSettings));
            } else {
                rows.push(item(tr("Use as camera"), Action::Camera));
            }
        }
        // What a phone has for this computer (Max, 2026-10-09): its photos
        // (by its folder, when there is no adb), its internet.
        rows.push(Row::Sep);
        rows.push(item(tr("Import photos"), Action::ImportPhotos));
        if !no_adb {
            rows.push(item(if v.tether { tr("Stop phone's internet") } else { tr("Use phone's internet") }, Action::Tether));
        }
    }
    if !v.closed {
        rows.push(Row::Sep);
        rows.push(item(tr("Eject"), Action::Eject));
    }
    // What stands between this phone and the rest, and how it is got past.
    if v.phone && (v.limits.no_files || v.limits.no_adb()) {
        rows.push(Row::Sep);
        if v.limits.no_files {
            rows.push(item(tr("It shows no files: what to do on the phone"), Action::HelpFiles));
        }
        if v.limits.not_allowed {
            rows.push(item(tr("Allow this computer on the phone for more features"), Action::HelpDebugging));
        } else if v.limits.debugging_off {
            rows.push(item(tr("Turn on USB debugging for more features"), Action::HelpDebugging));
        }
    }
    // Last: a phone's Configure box (a click), and Properties, opened by
    // coming onto it as on a file's menu. (Both ask the phone over adb.)
    rows.push(Row::Sep);
    if v.phone && !v.limits.no_adb() {
        rows.push(item(tr("Configure"), Action::Configure));
    }
    rows.push(item(tr("Properties"), Action::Properties));
    rows
}

/// A menu that is up.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct Menu {
    /// The item it was opened on, if any (the wallpaper's menu otherwise).
    pub item: Option<usize>,
    /// Opened on one of several selected items.
    pub many: bool,
    /// Showing the "Move to" page (see `show_targets`).
    pub targets: bool,
    /// A plugged-in volume's menu (`for_volume`) — what Back goes back to.
    pub volume: Option<Volume>,
    /// Where the pointer was: a new folder goes in that cell.
    pub at: (f32, f32),
    pub rows: Vec<Row>,
    /// The panel, placed so it stays on the surface.
    pub rect: Rect,
    /// The row under the pointer.
    pub hover: Option<usize>,
    /// The row the left button went down on.
    pub pressed: Option<usize>,
    /// Its entrance, 0..1: it fades in and settles down a few pixels.
    pub t: f32,
    /// Coming back from the Properties box: the shape it shrinks from.
    pub grow_from: Option<Rect>,
}

impl Menu {
    /// A menu at `at` on a `w`×`h` surface: hanging to the right and below
    /// the pointer, or flipped to stay on the surface.
    pub fn open(item: Option<usize>, many: bool, at: (f32, f32), w: f32, h: f32) -> Self {
        let rows = rows(item.is_some(), many);
        let height = PAD * 2.0 + rows.iter().map(row_h).sum::<f32>();
        let x = if at.0 + GAP + WIDTH <= w { at.0 + GAP } else { (at.0 - GAP - WIDTH).max(0.0) };
        let y = if at.1 + GAP + height <= h { at.1 + GAP } else { (at.1 - GAP - height).max(0.0) };
        Self {
            item,
            many,
            targets: false,
            volume: None,
            at,
            rows,
            rect: Rect::new(x, y, WIDTH, height),
            hover: None,
            pressed: None,
            t: 0.0,
            grow_from: None,
        }
    }

    /// Put new rows in the panel where it stands: the same corner, the
    /// height that fits them, kept on a surface `h` tall.
    fn set_rows(&mut self, rows: Vec<Row>, h: f32) {
        let height = PAD * 2.0 + rows.iter().map(row_h).sum::<f32>();
        self.rows = rows;
        self.rect.h = height;
        self.rect.y = self.rect.y.min((h - height).max(0.0));
        self.hover = None;
        self.pressed = None;
        self.grow_from = None;
    }

    /// Turn the page to where the selection can go: ‹ Back, Cut and Copy,
    /// then `near` (what is plugged in or paired) and `far` (the other
    /// computers on the network), each by its name. The `n`th name of
    /// `near` then `far` is `Action::SendTo(n)`. `looking`: the lists are
    /// still being asked for.
    pub fn show_targets(&mut self, near: &[String], far: &[String], looking: bool, h: f32) {
        let mut rows = vec![
            Row::Item { label: tr("‹  Back"), action: Action::Back, danger: false },
            Row::Sep,
            Row::Item { label: tr("Cut"), action: Action::Cut, danger: false },
            Row::Item { label: tr("Copy"), action: Action::Copy, danger: false },
        ];
        let mut n = 0;
        for names in [near, far] {
            if names.is_empty() {
                continue;
            }
            rows.push(Row::Sep);
            for name in names {
                rows.push(Row::Target { label: name.clone(), action: Action::SendTo(n) });
                n += 1;
            }
        }
        if looking {
            rows.push(Row::Sep);
            rows.push(Row::Note(tr("Looking for devices…")));
        } else if n == 0 {
            rows.push(Row::Sep);
            rows.push(Row::Note(tr("No devices found")));
        }
        self.targets = true;
        self.set_rows(rows, h);
    }

    /// The menu of a plugged-in volume standing on the desktop: it is not a
    /// file of the desktop's — nothing renames, moves or bins it.
    pub fn for_volume(mut self, volume: Volume, h: f32) -> Self {
        self.set_rows(volume_rows(&volume), h);
        self.volume = Some(volume);
        self
    }

    /// On a shortcut that is not yet let run: that, first.
    pub fn with_allow(mut self, h: f32) -> Self {
        let mut rows = vec![Row::Item { label: tr("Allow to run"), action: Action::AllowRun, danger: false }, Row::Sep];
        rows.append(&mut self.rows);
        self.set_rows(rows, h);
        self
    }

    /// Back to the first page.
    pub fn show_main(&mut self, h: f32) {
        self.targets = false;
        let rows = match &self.volume {
            Some(volume) => volume_rows(volume),
            None => rows(self.item.is_some(), self.many),
        };
        self.set_rows(rows, h);
    }

    /// The wallpaper's menu, with Paste on top (files are on the clipboard).
    pub fn with_paste(mut self, h: f32) -> Self {
        let mut rows = vec![Row::Item { label: tr("Paste"), action: Action::Paste, danger: false }, Row::Sep];
        rows.append(&mut self.rows);
        self.set_rows(rows, h);
        self
    }

    /// Each row's rectangle, in order (rules included).
    pub fn row_rects(&self) -> Vec<Rect> {
        let mut y = self.rect.y + PAD;
        self.rows
            .iter()
            .map(|r| {
                let h = row_h(r);
                let rect = Rect::new(self.rect.x + PAD, y, WIDTH - 2.0 * PAD, h);
                y += h;
                rect
            })
            .collect()
    }

    /// The action row under `pos` (never a rule).
    pub fn hit(&self, pos: (f32, f32)) -> Option<usize> {
        self.row_rects()
            .iter()
            .enumerate()
            .find(|(i, r)| r.contains(pos) && matches!(self.rows[*i], Row::Item { .. } | Row::Target { .. }))
            .map(|(i, _)| i)
    }

    /// The action of row `i`.
    pub fn action(&self, i: usize) -> Option<Action> {
        match self.rows.get(i) {
            Some(Row::Item { action, .. } | Row::Target { action, .. }) => Some(*action),
            _ => None,
        }
    }

    /// Draw the menu over everything in `scene`: a grid of its own (grids
    /// paint after the icons), clipped to the panel.
    pub fn push(&self, scene: &mut Scene, paint: &MenuPaint) {
        let t = self.t.clamp(0.0, 1.0);
        // Its entrance: fading in while settling down from 4 px above — or,
        // back from the Properties box, that box's panel shrinking to ours.
        let lift = if self.grow_from.is_some() { 0.0 } else { (1.0 - t) * -4.0 };
        let fade = |c: [f32; 4]| [c[0], c[1], c[2], c[3] * t];
        let ink_at = |a: f32| [paint.ink[0], paint.ink[1], paint.ink[2], paint.ink[3] * a];
        let panel = match &self.grow_from {
            Some(from) => crate::desktop_props::lerp_rect(from, &self.rect, t),
            None => Rect::new(self.rect.x, self.rect.y + lift, self.rect.w, self.rect.h),
        };
        let panel_fill = if self.grow_from.is_some() { paint.fill } else { fade(paint.fill) };
        let mut grid = GridContent {
            clip: panel,
            ..Default::default()
        };
        grid.rects.push(RectInst {
            rect: panel,
            radius: paint.radius,
            color: panel_fill,
            glass: 0.0,
            border: 0.0,
        });
        for (i, (row, rect)) in self.rows.iter().zip(self.row_rects()).enumerate() {
            let rect = Rect::new(rect.x, rect.y + lift, rect.w, rect.h);
            match row {
                Row::Sep => grid.rects.push(RectInst {
                    rect: Rect::new(rect.x + 2.0, rect.y + (rect.h - 1.0) / 2.0, rect.w - 4.0, 1.0),
                    radius: 0.0,
                    color: fade(ink_at(LINE_INK)),
                    glass: 0.0,
                    border: 0.0,
                }),
                Row::Note(text) => grid.labels.push(Label {
                    text: (*text).to_owned(),
                    pos: (rect.x + TEXT_X, rect.y + (rect.h - LINE_PX) / 2.0),
                    max_w: rect.w - 2.0 * TEXT_X,
                    font_px: FONT_PX,
                    line_px: LINE_PX,
                    centered: false,
                    dim: false,
                    cache: true,
                    clip: Some(panel),
                    family: None,
                    color: Some(fade(ink_at(NOTE_INK))),
                }),
                Row::Item { .. } | Row::Target { .. } => {
                    let (label, danger): (&str, bool) = match row {
                        Row::Item { label, danger, .. } => (label, *danger),
                        Row::Target { label, .. } => (label.as_str(), false),
                        _ => ("", false),
                    };
                    let hot = self.hover == Some(i);
                    if hot {
                        grid.rects.push(RectInst {
                            rect,
                            radius: ROW_RADIUS,
                            color: fade(if danger { HOVER_DANGER } else { paint.wash }),
                            glass: 0.0,
                            border: 0.0,
                        });
                    }
                    // Hover is weight and full strength, as in every box's
                    // list: the colour itself does not move.
                    let color = if danger {
                        RED
                    } else if hot {
                        ink_at(1.0)
                    } else {
                        ink_at(REST_INK)
                    };
                    grid.labels.push(Label {
                        text: label.to_owned(),
                        pos: (rect.x + TEXT_X, rect.y + (rect.h - LINE_PX) / 2.0),
                        max_w: rect.w - 2.0 * TEXT_X,
                        font_px: FONT_PX,
                        line_px: LINE_PX,
                        centered: false,
                        dim: false,
                        cache: true,
                        clip: Some(panel),
                        family: hot.then_some(FONT_BOLD),
                        color: Some(fade(color)),
                    });
                }
            }
        }
        scene.grids.push(grid);
    }
}

fn row_h(row: &Row) -> f32 {
    match row {
        Row::Item { .. } | Row::Target { .. } | Row::Note(_) => ROW_H,
        Row::Sep => SEP_H,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_icon_menu_hangs_right_and_below_the_pointer_and_flips_at_the_edges() {
        let m = Menu::open(Some(3), false, (100.0, 100.0), 1000.0, 800.0);
        assert_eq!((m.rect.x, m.rect.y), (106.0, 106.0));
        assert_eq!(m.rect.w, WIDTH);
        // 7 rows + 3 rules, inside the padding.
        assert_eq!(m.rect.h, PAD * 2.0 + 7.0 * ROW_H + 3.0 * SEP_H);
        // Near the right and bottom edges it opens to the left and above.
        let m = Menu::open(Some(3), false, (950.0, 780.0), 1000.0, 800.0);
        assert_eq!(m.rect.x, 950.0 - GAP - WIDTH);
        assert!(m.rect.y + m.rect.h <= 780.0 - GAP + 0.01);
        // The wallpaper's menu is the short one.
        let m = Menu::open(None, false, (10.0, 10.0), 1000.0, 800.0);
        assert_eq!(m.rows.len(), 3);
        assert_eq!(m.action(0), Some(Action::NewFolder));
        assert_eq!(m.action(1), None, "a rule is not an action");
        assert_eq!(m.action(2), Some(Action::CleanUp));
    }

    #[test]
    fn several_selected_items_get_only_what_suits_a_group() {
        let m = Menu::open(Some(1), true, (10.0, 10.0), 1000.0, 800.0);
        let actions: Vec<Action> = (0..m.rows.len()).filter_map(|i| m.action(i)).collect();
        assert_eq!(actions, vec![Action::Open, Action::MoveTo, Action::MoveToHome, Action::MoveToBin]);
    }

    #[test]
    fn hit_finds_rows_but_never_rules_or_the_padding() {
        let m = Menu::open(Some(0), false, (0.0, 0.0), 1000.0, 800.0);
        let rows = m.row_rects();
        let mid = |r: &Rect| (r.x + r.w / 2.0, r.y + r.h / 2.0);
        assert_eq!(m.hit(mid(&rows[0])), Some(0));
        assert_eq!(m.action(0), Some(Action::Open));
        assert_eq!(m.hit(mid(&rows[2])), None, "the rule");
        assert_eq!(m.hit(mid(&rows[4])), None, "the second rule");
        assert_eq!(m.hit(mid(&rows[5])), Some(5));
        assert_eq!(m.action(5), Some(Action::MoveTo));
        assert_eq!(m.action(6), Some(Action::MoveToHome));
        assert_eq!(m.hit(mid(&rows[7])), Some(7));
        assert_eq!(m.action(7), Some(Action::MoveToBin));
        assert_eq!(m.hit(mid(&rows[8])), None, "the third rule");
        assert_eq!(m.action(9), Some(Action::Properties), "last");
        assert_eq!(m.hit((m.rect.x + 1.0, m.rect.y + 1.0)), None, "the padding");
        assert_eq!(m.hit((5000.0, 5000.0)), None);
    }

    #[test]
    fn move_to_turns_the_page_to_where_the_selection_can_go_and_back() {
        let mut m = Menu::open(Some(0), false, (100.0, 700.0), 1000.0, 800.0);
        let corner = (m.rect.x, m.rect.y + m.rect.h);
        let near = vec!["PHOTOS".to_owned(), "Pixel 8".to_owned()];
        let far = vec!["max-laptop".to_owned()];
        m.show_targets(&near, &far, false, 800.0);
        assert!(m.targets);
        let actions: Vec<Action> = (0..m.rows.len()).filter_map(|i| m.action(i)).collect();
        assert_eq!(
            actions,
            vec![
                Action::Back,
                Action::Cut,
                Action::Copy,
                Action::SendTo(0),
                Action::SendTo(1),
                Action::SendTo(2)
            ]
        );
        // The two lists stand apart, each device by its own name.
        assert_eq!(m.rows[6], Row::Target { label: "Pixel 8".into(), action: Action::SendTo(1) });
        assert_eq!(m.rows[7], Row::Sep);
        // The panel keeps its place and stays on the surface.
        assert_eq!(m.rect.x, corner.0);
        assert!(m.rect.y + m.rect.h <= 800.0 + 0.01);
        // Nothing found, or still asking: a line says so, and is no action.
        m.show_targets(&[], &[], false, 800.0);
        assert_eq!(m.rows.last(), Some(&Row::Note("No devices found")));
        assert_eq!(m.action(m.rows.len() - 1), None);
        let last = *m.row_rects().last().unwrap();
        assert_eq!(m.hit((last.x + 5.0, last.y + 5.0)), None);
        m.show_targets(&[], &[], true, 800.0);
        assert_eq!(m.rows.last(), Some(&Row::Note("Looking for devices…")));
        // Back: the first page again.
        m.show_main(800.0);
        assert!(!m.targets);
        assert_eq!(m.action(0), Some(Action::Open));
        // A volume's menu: open it or let it go, nothing else.
        let open = || Menu::open(Some(0), false, (10.0, 10.0), 1000.0, 800.0);
        let phone = open().for_volume(Volume { phone: true, ..Default::default() }, 800.0);
        assert_eq!(phone.action(2), Some(Action::Mirror), "a phone's screen can be mirrored");
        assert_eq!(phone.rows[3], Row::Sep);
        assert_eq!(phone.rows[4], Row::Item { label: "Use as camera", action: Action::Camera, danger: false });
        // While it is the camera: Stop, and the way to its settings (a page
        // of the bar's phone pill's box).
        let on = open().for_volume(Volume { phone: true, camera: true, ..Default::default() }, 800.0);
        assert_eq!(on.rows[4], Row::Item { label: "Stop camera", action: Action::CameraStop, danger: false });
        assert_eq!(on.rows[5], Row::Item { label: "Configure camera", action: Action::CameraSettings, danger: false });
        assert_eq!(on.rows[6], Row::Sep);
        // What a phone cannot do here is not offered, and why is said. No
        // adb (USB debugging off): what its cable alone does, and the way to
        // the rest.
        let rows_of = |limits: Limits| volume_rows(&Volume { phone: true, limits, ..Default::default() });
        let actions_of = |rows: &[Row]| -> Vec<Action> {
            rows.iter()
                .filter_map(|r| match r {
                    Row::Item { action, .. } | Row::Target { action, .. } => Some(*action),
                    _ => None,
                })
                .collect()
        };
        let off = rows_of(Limits { debugging_off: true, ..Default::default() });
        assert_eq!(
            actions_of(&off),
            vec![Action::Open, Action::OpenTerminal, Action::ImportPhotos, Action::Eject, Action::HelpDebugging, Action::Properties]
        );
        assert!(off.contains(&Row::Item {
            label: "Turn on USB debugging for more features",
            action: Action::HelpDebugging,
            danger: false
        }));
        // Seen, and not allowed yet: the same, said as it is.
        let waiting = rows_of(Limits { not_allowed: true, ..Default::default() });
        assert!(waiting.iter().any(|r| matches!(r, Row::Item { label, .. } if label.starts_with("Allow this computer"))));
        // An Android too old for the camera: the row says so, in its place.
        let old = rows_of(Limits { old_camera: true, ..Default::default() });
        assert_eq!(old[4], Row::Note("Use as camera: needs Android 12 or newer"));
        assert!(!actions_of(&old).contains(&Action::Camera));
        // Showing no files: how it is made to.
        let empty = rows_of(Limits { no_files: true, ..Default::default() });
        assert!(actions_of(&empty).contains(&Action::HelpFiles));
        // A phone that is not open: no terminal, no eject.
        let shut = open().for_volume(Volume { phone: true, closed: true, ..Default::default() }, 800.0);
        let actions: Vec<Action> = (0..shut.rows.len()).filter_map(|i| shut.action(i)).collect();
        assert_eq!(
            actions,
            vec![
                Action::Open,
                Action::Mirror,
                Action::Camera,
                Action::ImportPhotos,
                Action::Tether,
                Action::Configure,
                Action::Properties
            ]
        );
        let sharing = open().for_volume(Volume { phone: true, tether: true, ..Default::default() }, 800.0);
        assert!(sharing.rows.contains(&Row::Item { label: "Stop phone's internet", action: Action::Tether, danger: false }));
        let m = open().for_volume(Volume::default(), 800.0);
        let actions: Vec<Action> = (0..m.rows.len()).filter_map(|i| m.action(i)).collect();
        assert_eq!(actions, vec![Action::Open, Action::OpenTerminal, Action::Eject, Action::Properties]);
        // The wallpaper's menu takes Paste on top when there is something.
        let m = Menu::open(None, false, (10.0, 10.0), 1000.0, 800.0).with_paste(800.0);
        assert_eq!(m.action(0), Some(Action::Paste));
        assert_eq!(m.action(2), Some(Action::NewFolder));
    }

    const PAINT: MenuPaint = MenuPaint {
        fill: [0.1, 0.1, 0.12, 0.9],
        ink: [0.9, 0.9, 0.9, 1.0],
        wash: [1.0, 1.0, 1.0, 0.1],
        radius: 10.0,
    };

    #[test]
    fn the_drawn_menu_is_one_grid_with_a_panel_a_hover_band_and_its_labels() {
        let mut m = Menu::open(Some(0), false, (0.0, 0.0), 1000.0, 800.0);
        m.t = 1.0;
        m.hover = Some(7);
        let mut scene = Scene::default();
        m.push(&mut scene, &PAINT);
        assert_eq!(scene.grids.len(), 1);
        let g = &scene.grids[0];
        assert_eq!(g.clip, m.rect);
        // The panel, two rules, one hover band.
        assert_eq!(g.rects.len(), 5);
        assert_eq!(g.rects[0].rect, m.rect);
        assert_eq!(g.rects[0].color, PAINT.fill, "the boxes' fill");
        assert_eq!(g.rects[0].radius, PAINT.radius);
        assert_eq!(g.rects[3].color, HOVER_DANGER, "the bin's band is red");
        assert_eq!(g.labels.len(), 7);
        assert_eq!(g.labels[3].text, "Move to");
        assert_eq!(g.labels[4].text, "Move to Home");
        assert_eq!(g.labels[5].text, "Move to bin");
        assert_eq!(g.labels[5].color, Some(RED));
        assert_eq!(g.labels[5].family, Some(FONT_BOLD), "hovered: bold");
        assert_eq!(g.labels[6].text, "Properties");
        assert_eq!(g.labels[0].family, None);
        assert!((g.labels[0].color.unwrap()[3] - REST_INK).abs() < 1e-5, "resting: the ink, a little under full");
        // Half-way in, everything is half as strong and 2 px above its place.
        m.t = 0.5;
        let mut scene = Scene::default();
        m.push(&mut scene, &PAINT);
        let g = &scene.grids[0];
        assert!((g.rects[0].color[3] - PAINT.fill[3] * 0.5).abs() < 1e-5);
        assert!((g.rects[0].rect.y - (m.rect.y - 2.0)).abs() < 1e-5);
    }
}
