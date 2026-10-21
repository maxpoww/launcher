//! THE MENU: one grammar for every right-click in Golem (Max, 2026-10-21,
//! mockup v50 "All menus" and v52's desktop: "Build all of it"). Files — a
//! file, a folder, several, an archive, an app's shortcut, the empty space
//! of a folder, the bin and what is in it, a place of the sidebar, a stick,
//! a phone — and the desktop — its wallpaper, what stands on it — all build
//! their rows here, from ONE builder (`rows_for`), so what is learnt in one
//! place works in every other.
//!
//! The same order everywhere: OPEN · hand over (COPY / MOVE) · CHANGE ·
//! SEND · KEEP · GET INFO · the way out LAST (Move to bin, or Eject). Every
//! row is a symbol and words in sentence case; `›` at its right turns the
//! menu's PAGE (Open with, Send to, Sort by — a page leads back with
//! "‹ <its name>"; no flyouts); "…" opens a window; RED is only for what
//! cannot be undone (Delete for good, Empty the bin, Format…). NOTHING
//! OPENS ON HOVER (Max: "i dont want the open on hover on the menus"): a
//! page turns, Get info opens, on a CLICK.
//!
//! Pure here: what the rows are, where they sit, which one is under a point,
//! and how they are drawn into a [`Scene`]. The desktop (`desktop.rs`) and
//! the file windows (`filer.rs`) open them and act on a row.

use crate::content::{GridContent, IconInst, Label, Rect, RectInst, Scene, FONT_BOLD};
use crate::i18n::{fill, tr};

/// What a row does.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Action {
    Open,
    /// A folder (a place, a stick) in a new tab of the window.
    OpenInTab,
    OpenTerminal,
    /// The thing big, over everything (Space).
    QuickLook,
    Rename,
    /// A box of what there is to know about it (the file windows: the i card).
    Properties,
    /// …and back to the first page.
    Back,
    /// Onto the clipboard, as a file manager's cut ("Move") or copy.
    Cut,
    Copy,
    /// The clipboard's files, here — or into the folder the menu is on.
    Paste,
    PasteInto,
    /// A copy beside it, "name copy".
    Duplicate,
    /// Everything here selected.
    SelectAll,
    /// To the one stick plugged in, a copy.
    CopyToStick,
    /// Turn the page to where it can be sent, and to the `n`th of those.
    SendPage,
    SendTo(usize),
    /// Turn the page to the apps that open this kind of file, and open it
    /// with the `n`th of those.
    OpenWith,
    OpenApp(usize),
    /// Turn the page to the orders; order by the `n`th key (name, date
    /// modified, size, kind), and which way (descending?).
    SortPage,
    SortBy(u8),
    SortDir(bool),
    /// A plugged-in volume (a stick, a phone): let it go.
    Eject,
    /// A stick or a memory card: its Format page in the gear box…
    Format,
    /// …checked for errors and mended.
    CheckDrive,
    /// A phone: its screen in a window here (scrcpy).
    Mirror,
    /// A phone as this computer's camera (its lens, its picture and its
    /// flash are chosen on the bar's camera pill)…
    Camera,
    /// …and off again.
    CameraStop,
    /// A phone: its new photos and videos, to this computer.
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
    /// The selection into a new folder ("New folder with items").
    NewFolderWith,
    CleanUp,
    /// A shortcut that came from elsewhere: let it run from now on.
    AllowRun,
    /// In the bin: back where it was thrown away from, gone for good, and
    /// the whole bin gone for good.
    PutBack,
    Erase,
    EmptyBin,
    /// An archive's contents out beside it; the selection into an archive.
    Extract,
    Compress,
    /// A folder kept among the places down a file window's side (the
    /// bookmarks file dialogs show too), and taken out again; dot files
    /// shown or put away.
    AddPlace,
    RemovePlace,
    ToggleHidden,
    /// THE DESKTOP'S OWN: the last thing done on it taken back; its icons
    /// put away or brought back; the next wallpaper; the Wallpaper, Colours
    /// and Display settings in their boxes; a moving wallpaper held still or
    /// let go on.
    Undo,
    ToggleIcons,
    NextWallpaper,
    Wallpapers,
    MovingWallpaper,
    Colours,
    Display,
    /// WHAT A KIND OF THING IS FOR (`filer/smart.rs`): songs and videos
    /// played (in a shuffled order), photos shown one after another, one
    /// made the wallpaper or turned, documents printed; `…Here`: of the
    /// folder shown (the button by the ↶).
    Play,
    Shuffle,
    PlayHere,
    Slideshow,
    SlideshowHere,
    SetWallpaper,
    Rotate,
    Print,
}

/// One row: an action, a line that only says something, or a rule.
#[derive(Debug, Clone, PartialEq)]
pub(crate) enum Row {
    Item {
        label: String,
        /// Its symbol (a name of `glyphs.rs`; "" none).
        icon: &'static str,
        action: Action,
        /// Drawn in red: it cannot be undone.
        danger: bool,
        /// Shown, but there is nothing for it to do now.
        off: bool,
    },
    /// A heading over a list of a page ("Plugged in"), or a line that only
    /// says something ("Looking for more…").
    Note(String),
    Sep,
}

impl Row {
    pub fn item(label: impl Into<String>, icon: &'static str, action: Action) -> Self {
        Row::Item {
            label: label.into(),
            icon,
            action,
            danger: false,
            off: false,
        }
    }

    pub fn danger(label: impl Into<String>, icon: &'static str, action: Action) -> Self {
        Row::Item {
            label: label.into(),
            icon,
            action,
            danger: true,
            off: false,
        }
    }

    pub fn note(text: impl Into<String>) -> Self {
        Row::Note(text.into())
    }

    /// Its action, if it has one.
    pub fn action(&self) -> Option<Action> {
        match self {
            Row::Item { action, .. } => Some(*action),
            _ => None,
        }
    }
}

/// Whether a row with `action` turns the menu's page (a `›` at its right).
pub(crate) fn turns_page(action: Action) -> bool {
    matches!(
        action,
        Action::OpenWith | Action::SendPage | Action::SortPage
    )
}

/// What was right-clicked, as the menu needs to know it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub(crate) enum On {
    /// The empty space of a folder.
    #[default]
    Ground,
    File,
    Archive,
    /// A program's shortcut (a `.desktop` file).
    App,
    Folder,
    /// One of several selected.
    Many,
    /// The desktop's bare wallpaper.
    Desk,
    /// In the bin: a thing thrown away; one inside a thrown-away folder;
    /// the bin's empty space; the bin as a place of the sidebar.
    BinItem,
    BinDeep,
    BinGround,
    BinPlace,
    Stick,
    Phone,
    /// A place of the sidebar.
    Place,
}

/// Everything a menu's rows depend on.
#[derive(Debug, Clone, PartialEq, Default)]
pub(crate) struct Ctx {
    pub on: On,
    /// How many are selected (`Many`).
    pub n: usize,
    /// The one stick plugged in, by its label: "Copy to <it>".
    pub stick: Option<String>,
    /// Things can be made where this is (Duplicate, Compress, New folder).
    pub here: bool,
    /// Files are on the clipboard and can be put here (Paste, Paste into
    /// folder).
    pub clip: bool,
    /// On the desktop (Move to Home; no new tab).
    pub desk: bool,
    /// A shortcut from elsewhere, not yet allowed to run.
    pub untrusted: bool,
    /// A folder that is a place of the sidebar.
    pub placed: bool,
    /// Something of the system's: not thrown away.
    pub system: bool,
    /// Dot files shown.
    pub hidden: bool,
    /// The folder is arranged by hand (Clean up puts it in order).
    pub by_hand: bool,
    /// A terminal can be opened here (the empty space of a real folder).
    pub terminal: bool,
    /// The desktop: the last thing done there, by its name ("Undo <it>").
    pub undo: Option<String>,
    /// The desktop: its icons are put away.
    pub icons_hidden: bool,
    /// The desktop: the wallpaper moves (a video), and whether it is held.
    pub moving: Option<bool>,
    /// A place of the sidebar that is not the owner's own to take out.
    pub fixed: bool,
    /// The bin: how many things are in it.
    pub bin_n: usize,
    /// A stick or a phone: what it is and can do.
    pub volume: Volume,
    /// What this kind of thing is FOR (songs: Play…), right after Open.
    pub smart: Vec<Row>,
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

/// THE ROWS of a right-click on `c` — the one builder every menu uses.
pub(crate) fn rows_for(c: &Ctx) -> Vec<Row> {
    let mut out: Vec<Row> = Vec::new();
    let mut grp = |rows: Vec<Option<Row>>| {
        let rows: Vec<Row> = rows.into_iter().flatten().collect();
        if rows.is_empty() {
            return;
        }
        if !out.is_empty() {
            out.push(Row::Sep);
        }
        out.extend(rows);
    };
    let row = |label: &str, icon, action| Some(Row::item(label, icon, action));
    let hand = || {
        vec![
            row(tr("Copy"), "copy", Action::Copy),
            row(tr("Move"), "move", Action::Cut),
        ]
    };
    let send = || {
        vec![
            c.stick.as_ref().map(|s| {
                Row::item(
                    fill(tr("Copy to {stick}"), &[("stick", s)]),
                    "usb",
                    Action::CopyToStick,
                )
            }),
            row(tr("Send to"), "send", Action::SendPage),
        ]
    };
    let info = || vec![row(tr("Get info"), "info", Action::Properties)];
    let bin =
        || vec![(!c.system).then(|| Row::item(tr("Move to bin"), "trash", Action::MoveToBin))];
    let home = || {
        vec![c
            .desk
            .then(|| Row::item(tr("Move to Home"), "home", Action::MoveToHome))]
    };
    let smart = || c.smart.iter().cloned().map(Some).collect::<Vec<_>>();
    match c.on {
        On::File | On::Archive | On::App => {
            let app = c.on == On::App;
            let mut open = vec![row(tr("Open"), "open", Action::Open)];
            open.extend(smart());
            open.extend([
                c.untrusted
                    .then(|| Row::item(tr("Allow to run"), "shield", Action::AllowRun)),
                (c.on == On::Archive)
                    .then(|| Row::item(tr("Extract here"), "extract", Action::Extract)),
                (!app).then(|| Row::item(tr("Open with"), "openwith", Action::OpenWith)),
                (!app).then(|| Row::item(tr("Quick look"), "eye", Action::QuickLook)),
            ]);
            grp(open);
            let mut copy = hand();
            copy.push(
                c.here
                    .then(|| Row::item(tr("Duplicate"), "dup", Action::Duplicate)),
            );
            grp(copy);
            grp(vec![
                row(tr("Rename"), "rename", Action::Rename),
                (c.here && c.on != On::Archive)
                    .then(|| Row::item(tr("Compress"), "zip", Action::Compress)),
            ]);
            grp(send());
            grp(home());
            grp(info());
            grp(bin());
        }
        On::Folder => {
            let mut open = vec![row(tr("Open"), "open", Action::Open)];
            open.extend(smart());
            open.extend([
                (!c.desk).then(|| Row::item(tr("Open in new tab"), "tab", Action::OpenInTab)),
                row(tr("Open in terminal"), "term", Action::OpenTerminal),
            ]);
            grp(open);
            let mut copy = hand();
            copy.push(
                c.clip
                    .then(|| Row::item(tr("Paste into folder"), "paste", Action::PasteInto)),
            );
            copy.push(
                c.here
                    .then(|| Row::item(tr("Duplicate"), "dup", Action::Duplicate)),
            );
            grp(copy);
            grp(vec![
                row(tr("Rename"), "rename", Action::Rename),
                c.here
                    .then(|| Row::item(tr("Compress"), "zip", Action::Compress)),
            ]);
            grp(send());
            grp(vec![Some(if c.desk {
                Row::item(tr("Move to Home"), "home", Action::MoveToHome)
            } else if c.placed {
                Row::item(tr("Remove from sidebar"), "unplace", Action::RemovePlace)
            } else {
                Row::item(tr("Add to sidebar"), "place", Action::AddPlace)
            })]);
            grp(info());
            grp(bin());
        }
        On::Many => {
            let mut open = vec![Some(Row::item(
                fill(tr("Open {n} items"), &[("n", &c.n)]),
                "open",
                Action::Open,
            ))];
            open.extend(smart());
            grp(open);
            let mut copy = hand();
            copy.push(
                c.here
                    .then(|| Row::item(tr("Duplicate"), "dup", Action::Duplicate)),
            );
            grp(copy);
            grp(vec![
                c.here
                    .then(|| Row::item(tr("New folder with these"), "newf", Action::NewFolderWith)),
                c.here
                    .then(|| Row::item(tr("Compress"), "zip", Action::Compress)),
            ]);
            grp(send());
            grp(home());
            grp(info());
            grp(bin());
        }
        On::Ground => {
            grp(vec![c
                .clip
                .then(|| Row::item(tr("Paste"), "paste", Action::Paste))]);
            grp(vec![
                c.here
                    .then(|| Row::item(tr("New folder"), "newf", Action::NewFolder)),
                row(tr("Select all"), "selall", Action::SelectAll),
            ]);
            grp(vec![
                row(tr("Sort by"), "sort", Action::SortPage),
                Some(if c.hidden {
                    Row::item(tr("Hide hidden files"), "eyeoff", Action::ToggleHidden)
                } else {
                    Row::item(tr("Show hidden files"), "eye", Action::ToggleHidden)
                }),
                c.by_hand
                    .then(|| Row::item(tr("Clean up"), "cleanup", Action::CleanUp)),
            ]);
            grp(vec![c.terminal.then(|| {
                Row::item(tr("Open in terminal"), "term", Action::OpenTerminal)
            })]);
            grp(info());
        }
        On::Desk => {
            grp(vec![c.undo.as_ref().map(|what| {
                Row::item(
                    fill(tr("Undo {what}"), &[("what", what)]),
                    "undo",
                    Action::Undo,
                )
            })]);
            grp(vec![c
                .clip
                .then(|| Row::item(tr("Paste"), "paste", Action::Paste))]);
            grp(vec![
                row(tr("New folder"), "newf", Action::NewFolder),
                row(tr("Select all"), "selall", Action::SelectAll),
            ]);
            grp(vec![
                row(tr("Sort by"), "sort", Action::SortPage),
                row(tr("Clean up"), "cleanup", Action::CleanUp),
                Some(if c.icons_hidden {
                    Row::item(tr("Show icons"), "eye", Action::ToggleIcons)
                } else {
                    Row::item(tr("Hide icons"), "eyeoff", Action::ToggleIcons)
                }),
            ]);
            grp(vec![
                row(tr("Next wallpaper"), "next", Action::NextWallpaper),
                row(tr("Wallpapers…"), "pic", Action::Wallpapers),
                c.moving.map(|held| {
                    if held {
                        Row::item(tr("Play moving wallpaper"), "play", Action::MovingWallpaper)
                    } else {
                        Row::item(
                            tr("Pause moving wallpaper"),
                            "pause",
                            Action::MovingWallpaper,
                        )
                    }
                }),
            ]);
            grp(vec![
                row(tr("Colours…"), "palette", Action::Colours),
                row(tr("Display…"), "display", Action::Display),
            ]);
        }
        On::BinItem => {
            grp(vec![row(tr("Quick look"), "eye", Action::QuickLook)]);
            grp(vec![row(tr("Put back"), "undo", Action::PutBack)]);
            grp(vec![Some(Row::danger(
                tr("Delete for good"),
                "erase",
                Action::Erase,
            ))]);
        }
        On::BinDeep => {
            grp(vec![
                row(tr("Open"), "open", Action::Open),
                row(tr("Quick look"), "eye", Action::QuickLook),
            ]);
            grp(vec![row(tr("Copy"), "copy", Action::Copy)]);
            grp(info());
        }
        On::BinGround | On::BinPlace => {
            if c.on == On::BinPlace {
                grp(vec![
                    row(tr("Open"), "open", Action::Open),
                    row(tr("Open in new tab"), "tab", Action::OpenInTab),
                ]);
            }
            grp(vec![Some(Row::Item {
                label: tr("Empty the bin").to_owned(),
                icon: "erase",
                action: Action::EmptyBin,
                danger: true,
                off: c.bin_n == 0,
            })]);
        }
        On::Stick => {
            let open = !c.volume.closed;
            grp(vec![
                row(tr("Open"), "open", Action::Open),
                (!c.desk && open)
                    .then(|| Row::item(tr("Open in new tab"), "tab", Action::OpenInTab)),
                open.then(|| Row::item(tr("Open in terminal"), "term", Action::OpenTerminal)),
            ]);
            if open {
                grp(vec![
                    row(tr("Rename"), "rename", Action::Rename),
                    row(tr("Check for errors"), "check", Action::CheckDrive),
                    Some(Row::danger(tr("Format…"), "drive", Action::Format)),
                ]);
            }
            grp(info());
            grp(vec![
                open.then(|| Row::item(tr("Eject"), "eject", Action::Eject))
            ]);
        }
        On::Phone => {
            // ONE list for a phone's menu, on the desktop, in the sidebar and
            // in the bar's phone pill (Max, 2026-10-11): what it cannot do
            // here is not offered in either; why, and the way to it, is said.
            let v = &c.volume;
            let no_adb = v.limits.no_adb();
            grp(vec![
                row(tr("Open"), "open", Action::Open),
                (!no_adb).then(|| Row::item(tr("Mirror screen"), "mirror", Action::Mirror)),
            ]);
            if !no_adb {
                // The camera is a group of its own (Max, 2026-10-09). On:
                // its Stop, and its settings (flash, lens, picture).
                grp(if v.limits.old_camera {
                    vec![Some(Row::note(tr(
                        "Use as camera: needs Android 12 or newer",
                    )))]
                } else if v.camera {
                    vec![
                        row(tr("Stop camera"), "camera", Action::CameraStop),
                        row(tr("Configure camera"), "gear", Action::CameraSettings),
                    ]
                } else {
                    vec![row(tr("Use as camera"), "camera", Action::Camera)]
                });
            }
            grp(vec![
                row(tr("Import photos"), "download", Action::ImportPhotos),
                (!no_adb).then(|| {
                    Row::item(
                        if v.tether {
                            tr("Stop phone's internet")
                        } else {
                            tr("Use phone's internet")
                        },
                        "net",
                        Action::Tether,
                    )
                }),
            ]);
            grp(vec![(!no_adb).then(|| {
                Row::item(tr("Configure…"), "gear", Action::Configure)
            })]);
            // What stands between this phone and the rest, and how it is
            // got past.
            grp(vec![
                v.limits.no_files.then(|| {
                    Row::item(
                        tr("It shows no files: what to do on the phone"),
                        "phone",
                        Action::HelpFiles,
                    )
                }),
                if v.limits.not_allowed {
                    Some(Row::item(
                        tr("Allow this computer on the phone for more features"),
                        "shield",
                        Action::HelpDebugging,
                    ))
                } else if v.limits.debugging_off {
                    Some(Row::item(
                        tr("Turn on USB debugging for more features"),
                        "shield",
                        Action::HelpDebugging,
                    ))
                } else {
                    None
                },
            ]);
            grp(info());
            grp(vec![
                (!v.closed).then(|| Row::item(tr("Eject"), "eject", Action::Eject))
            ]);
        }
        On::Place => {
            grp(vec![
                row(tr("Open"), "open", Action::Open),
                row(tr("Open in new tab"), "tab", Action::OpenInTab),
            ]);
            grp(vec![(!c.fixed).then(|| {
                Row::item(tr("Remove from sidebar"), "unplace", Action::RemovePlace)
            })]);
            grp(info());
        }
    }
    out
}

/// What a page of the menu is: its first, or the one a `›` row turned to.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub(crate) enum Page {
    #[default]
    Main,
    OpenWith,
    Send,
    Sort,
}

/// The row a page leads back with: "‹ <its name>".
fn back(title: &str) -> Vec<Row> {
    vec![Row::item(title, "left", Action::Back), Row::Sep]
}

/// The Open with page: the apps by their names (`None`: still being
/// asked).
pub(crate) fn open_with_rows(apps: Option<&[String]>) -> Vec<Row> {
    let mut rows = back(tr("Open with"));
    match apps {
        None => rows.push(Row::note(tr("Looking for apps…"))),
        Some([]) => rows.push(Row::note(tr("No app says it opens this"))),
        Some(apps) => rows.extend(
            apps.iter()
                .enumerate()
                .map(|(n, name)| Row::item(name.clone(), "app", Action::OpenApp(n))),
        ),
    }
    rows
}

/// One place a thing can be sent to: its name, and whether it is a phone.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct Target {
    pub name: String,
    pub phone: bool,
    /// A computer on the network (else plugged in, or carried).
    pub far: bool,
}

/// The Send to page: what is plugged in, then the computers on the
/// network, each by its name; the `n`th of `targets` is `SendTo(n)`.
/// `looking`: the network is still being asked.
pub(crate) fn send_rows(targets: &[Target], looking: bool) -> Vec<Row> {
    let mut rows = back(tr("Send to"));
    for (far, heading) in [(false, tr("Plugged in")), (true, tr("On the network"))] {
        let here: Vec<(usize, &Target)> = targets
            .iter()
            .enumerate()
            .filter(|(_, t)| t.far == far)
            .collect();
        if here.is_empty() && !(far && looking) {
            continue;
        }
        rows.push(Row::note(heading));
        for (n, t) in here {
            let icon = if t.far {
                "pc"
            } else if t.phone {
                "phone"
            } else {
                "usb"
            };
            rows.push(Row::item(t.name.clone(), icon, Action::SendTo(n)));
        }
    }
    if looking {
        rows.push(Row::note(tr("Looking for more…")));
    } else if targets.is_empty() {
        rows.push(Row::note(tr("Nothing is plugged in or nearby")));
    }
    rows
}

/// The orders (the top line's sort menu, and the Sort by page): the key in
/// use and the way ticked.
pub(crate) fn sort_rows(key: u8, desc: bool) -> Vec<Row> {
    let tick = |on: bool| if on { "tick" } else { "" };
    let mut rows: Vec<Row> = [tr("Name"), tr("Date Modified"), tr("Size"), tr("Kind")]
        .into_iter()
        .enumerate()
        .map(|(n, words)| Row::item(words, tick(n as u8 == key), Action::SortBy(n as u8)))
        .collect();
    rows.push(Row::Sep);
    rows.push(Row::item(
        tr("Ascending"),
        tick(!desc),
        Action::SortDir(false),
    ));
    rows.push(Row::item(
        tr("Descending"),
        tick(desc),
        Action::SortDir(true),
    ));
    rows
}

/// The Sort by page.
pub(crate) fn sort_page(key: u8, desc: bool) -> Vec<Row> {
    let mut rows = back(tr("Sort by"));
    rows.extend(sort_rows(key, desc));
    rows
}

/// The narrowest a menu is (the mockup's `min-width`), and the widest.
pub(crate) const WIDTH: f32 = 214.0;
const WIDTH_MAX: f32 = 380.0;
/// Padding inside the panel, around the rows.
const PAD: f32 = 5.0;
/// A row's height, a rule's, a note's.
const ROW_H: f32 = 29.0;
const SEP_H: f32 = 9.0;
const NOTE_H: f32 = 22.0;
/// A row's band's corner; the panel's is the boxes' (`MenuPaint::radius`).
const ROW_RADIUS: f32 = 5.0;
/// Text: size, line, and where it starts (after the symbol).
const FONT_PX: f32 = 12.5;
const LINE_PX: f32 = 16.0;
const NOTE_PX: f32 = 10.5;
const ICON: f32 = 14.0;
const ICON_X: f32 = 10.0;
const TEXT_X: f32 = ICON_X + ICON + 8.0;
/// The `›` of a row that turns the page.
const CHEV: f32 = 11.0;
/// How far from the pointer the panel's corner sits.
const GAP: f32 = 6.0;
/// A symbol's ink (the mockup's 75 %), a note's, a rule's; a row that has
/// nothing to do now.
const ICON_INK: f32 = 0.75;
const NOTE_INK: f32 = 0.5;
const LINE_INK: f32 = 0.12;
const OFF_INK: f32 = 0.38;

/// A colour given in sRGB, as the renderer takes it.
fn lin(c: [f32; 4]) -> [f32; 4] {
    let l = |v: f32| {
        if v <= 0.040_45 {
            v / 12.92
        } else {
            ((v + 0.055) / 1.055).powf(2.4)
        }
    };
    [l(c[0]), l(c[1]), l(c[2]), c[3]]
}

/// The one colour of its own: what cannot be undone — the palette's red.
fn red() -> [f32; 4] {
    lin(crate::palette::danger_srgb())
}

/// What the menu is painted with: the OPTIONS boxes' adaptive surface
/// (`App::box_surface_at`), read where the menu is.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct MenuPaint {
    /// The panel.
    pub fill: [f32; 4],
    /// Text that reads on it.
    pub ink: [f32; 4],
    /// A pointed-at band on its boxes (the phone's Configure box, the
    /// History).
    pub wash: [f32; 4],
    /// The panel's corner.
    pub radius: f32,
    /// The panel's `glass`: 0.0 (as every solid), or `content::UNLIT` in
    /// Golem Files' windows — there the dark look's night light sank a
    /// menu's lower rows toward black (Max, 2026-10-21: as the History).
    pub glass: f32,
    /// The row under the pointer: the accent, its words white.
    pub accent: [f32; 4],
}

impl MenuPaint {
    /// The accent the palette wears now (the desktop's menus).
    pub fn accent_now() -> [f32; 4] {
        lin(crate::palette::accent_srgb())
    }
}

/// A menu that is up.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct Menu {
    /// The item it was opened on, if any (the empty space's otherwise).
    pub item: Option<usize>,
    /// Opened on one of several selected items.
    pub many: bool,
    /// What it was opened on: its first page is made from this again.
    pub ctx: Ctx,
    /// The page that is up.
    pub page: Page,
    /// Where the pointer was: a new folder goes in that cell.
    pub at: (f32, f32),
    pub rows: Vec<Row>,
    /// The panel, placed so it stays on the surface.
    pub rect: Rect,
    /// The surface's size.
    room: (f32, f32),
    /// The row under the pointer.
    pub hover: Option<usize>,
    /// The row the left button went down on.
    pub pressed: Option<usize>,
    /// Its entrance, 0..1: it fades in and settles down a few pixels.
    pub t: f32,
    /// Coming back from the Properties box: the shape it shrinks from.
    pub grow_from: Option<Rect>,
    /// It hangs UP from its foot (the selection pill's ⋯): a new page
    /// keeps the foot where it is.
    pub up: bool,
}

impl Menu {
    /// The menu of `ctx` at `at` on a `w`×`h` surface: hanging to the right
    /// and below the pointer, or flipped to stay on the surface.
    pub fn new(ctx: Ctx, item: Option<usize>, many: bool, at: (f32, f32), w: f32, h: f32) -> Self {
        let rows = rows_for(&ctx);
        Self::open(ctx, rows, item, many, at, w, h)
    }

    /// A menu of rows of the caller's own (the sort menu).
    pub fn open(
        ctx: Ctx,
        rows: Vec<Row>,
        item: Option<usize>,
        many: bool,
        at: (f32, f32),
        w: f32,
        h: f32,
    ) -> Self {
        let (width, height) = (width_of(&rows), height_of(&rows));
        let x = if at.0 + GAP + width <= w {
            at.0 + GAP
        } else {
            (at.0 - GAP - width).max(0.0)
        };
        let y = if at.1 + GAP + height <= h {
            at.1 + GAP
        } else {
            (at.1 - GAP - height).max(0.0)
        };
        Self {
            item,
            many,
            ctx,
            page: Page::Main,
            at,
            rows,
            rect: Rect::new(x, y, width, height),
            room: (w, h),
            hover: None,
            pressed: None,
            t: 0.0,
            grow_from: None,
            up: false,
        }
    }

    /// Put new rows in the panel where it stands: the same corner, the size
    /// that fits them, kept on the surface.
    fn set_rows(&mut self, rows: Vec<Row>) {
        let (width, height) = (width_of(&rows), height_of(&rows));
        let foot = self.rect.y + self.rect.h;
        self.rows = rows;
        self.rect.w = width;
        self.rect.h = height;
        if self.up {
            self.rect.y = (foot - height).max(0.0);
        }
        self.rect.x = self.rect.x.min((self.room.0 - width).max(0.0));
        self.rect.y = self.rect.y.min((self.room.1 - height).max(0.0));
        self.hover = None;
        self.pressed = None;
        self.grow_from = None;
    }

    /// Turn to `page`, made of `rows`.
    pub fn turn(&mut self, page: Page, rows: Vec<Row>) {
        self.page = page;
        self.set_rows(rows);
    }

    /// Back to the first page.
    pub fn show_main(&mut self) {
        let rows = rows_for(&self.ctx);
        self.turn(Page::Main, rows);
    }

    /// Each row's rectangle, in order (rules included).
    pub fn row_rects(&self) -> Vec<Rect> {
        let mut y = self.rect.y + PAD;
        self.rows
            .iter()
            .map(|r| {
                let h = row_h(r);
                let rect = Rect::new(self.rect.x + PAD, y, self.rect.w - 2.0 * PAD, h);
                y += h;
                rect
            })
            .collect()
    }

    /// The action row under `pos` (never a rule, a note, or a row with
    /// nothing to do).
    pub fn hit(&self, pos: (f32, f32)) -> Option<usize> {
        self.row_rects()
            .iter()
            .enumerate()
            .find(|(i, r)| r.contains(pos) && matches!(self.rows[*i], Row::Item { off: false, .. }))
            .map(|(i, _)| i)
    }

    /// The action of row `i`.
    pub fn action(&self, i: usize) -> Option<Action> {
        self.rows.get(i).and_then(Row::action)
    }

    /// The symbols its rows wear (for the surface's icon layers).
    pub fn glyphs(&self) -> Vec<&'static str> {
        let mut names = Vec::new();
        for row in &self.rows {
            if let Row::Item { icon, action, .. } = row {
                if !icon.is_empty() && !names.contains(icon) {
                    names.push(*icon);
                }
                if turns_page(*action) && !names.contains(&"chev") {
                    names.push("chev");
                }
            }
        }
        names
    }

    /// Draw the menu over everything in `scene`. `glyph` gives the icon
    /// layer a symbol is in (none: the row has words alone).
    pub fn push(&self, scene: &mut Scene, paint: &MenuPaint, glyph: &dyn Fn(&str) -> Option<u32>) {
        let t = self.t.clamp(0.0, 1.0);
        // Its entrance: fading in while settling down from 4 px above — or,
        // back from the Properties box, that box's panel shrinking to ours.
        let lift = if self.grow_from.is_some() {
            0.0
        } else {
            (1.0 - t) * -4.0
        };
        let fade = |c: [f32; 4]| [c[0], c[1], c[2], c[3] * t];
        let ink_at = |a: f32| [paint.ink[0], paint.ink[1], paint.ink[2], paint.ink[3] * a];
        let panel = match &self.grow_from {
            Some(from) => crate::desktop_props::lerp_rect(from, &self.rect, t),
            None => Rect::new(self.rect.x, self.rect.y + lift, self.rect.w, self.rect.h),
        };
        let panel_fill = if self.grow_from.is_some() {
            paint.fill
        } else {
            fade(paint.fill)
        };
        let mut grid = GridContent {
            clip: panel,
            ..Default::default()
        };
        grid.rects.push(RectInst {
            rect: panel,
            radius: paint.radius,
            color: panel_fill,
            glass: paint.glass,
            border: 0.0,
        });
        let white = [1.0, 1.0, 1.0, 1.0];
        let label = |text: &str,
                     pos: (f32, f32),
                     max_w: f32,
                     px: f32,
                     line: f32,
                     bold: bool,
                     color: [f32; 4]| Label {
            text: text.to_owned(),
            pos,
            max_w,
            font_px: px,
            line_px: line,
            centered: false,
            dim: false,
            cache: true,
            clip: Some(panel),
            family: bold.then_some(FONT_BOLD),
            color: Some(fade(color)),
        };
        let symbol = |grid: &mut GridContent, name: &str, r: Rect, color: [f32; 4]| {
            if let Some(layer) = glyph(name) {
                grid.icons.push(IconInst {
                    rect: r,
                    layer,
                    tint: [color[0], color[1], color[2], 1.0],
                    ring: -1.17,
                    plate: [0.0, 0.0, 0.0, color[3] * t],
                });
            }
        };
        for (i, (row, rect)) in self.rows.iter().zip(self.row_rects()).enumerate() {
            let rect = Rect::new(rect.x, rect.y + lift, rect.w, rect.h);
            match row {
                Row::Sep => grid.rects.push(RectInst {
                    rect: Rect::new(
                        rect.x + 6.0,
                        rect.y + (rect.h - 1.0) / 2.0,
                        rect.w - 12.0,
                        1.0,
                    ),
                    radius: 0.0,
                    color: fade(ink_at(LINE_INK)),
                    glass: 0.0,
                    border: 0.0,
                }),
                Row::Note(text) => grid.labels.push(label(
                    text,
                    (rect.x + 10.0, rect.y + rect.h - NOTE_PX - 7.0),
                    rect.w - 20.0,
                    NOTE_PX,
                    NOTE_PX + 4.0,
                    false,
                    ink_at(NOTE_INK),
                )),
                Row::Item {
                    label: words,
                    icon,
                    action,
                    danger,
                    off,
                } => {
                    // The row under the pointer: a band of the accent (red
                    // for what cannot be undone), its words and symbol white.
                    let hot = self.hover == Some(i) && !off;
                    if hot {
                        grid.rects.push(RectInst {
                            rect,
                            radius: ROW_RADIUS,
                            color: fade(if *danger { red() } else { paint.accent }),
                            glass: 0.0,
                            border: 0.0,
                        });
                    }
                    let base = if hot {
                        white
                    } else if *danger {
                        red()
                    } else {
                        paint.ink
                    };
                    let k = if *off { OFF_INK } else { 1.0 };
                    let at = |a: f32| [base[0], base[1], base[2], base[3] * a * k];
                    let cy = rect.y + rect.h / 2.0;
                    if !icon.is_empty() {
                        let r = Rect::new(
                            (rect.x + ICON_X).round(),
                            (cy - ICON / 2.0).round(),
                            ICON,
                            ICON,
                        );
                        symbol(&mut grid, icon, r, at(if hot { 1.0 } else { ICON_INK }));
                    }
                    let chev = turns_page(*action);
                    if chev {
                        let r = Rect::new(
                            (rect.x + rect.w - 8.0 - CHEV).round(),
                            (cy - CHEV / 2.0).round(),
                            CHEV,
                            CHEV,
                        );
                        symbol(&mut grid, "chev", r, at(if hot { 1.0 } else { 0.55 }));
                    }
                    let room = rect.w - TEXT_X - if chev { CHEV + 12.0 } else { 8.0 };
                    grid.labels.push(label(
                        words,
                        (rect.x + TEXT_X, cy - LINE_PX / 2.0),
                        room,
                        FONT_PX,
                        LINE_PX,
                        *action == Action::Back,
                        at(1.0),
                    ));
                }
            }
        }
        scene.grids.push(grid);
    }
}

fn row_h(row: &Row) -> f32 {
    match row {
        Row::Item { .. } => ROW_H,
        Row::Note(_) => NOTE_H,
        Row::Sep => SEP_H,
    }
}

fn height_of(rows: &[Row]) -> f32 {
    PAD * 2.0 + rows.iter().map(row_h).sum::<f32>()
}

/// As wide as its longest row's words want (an estimate of the words at
/// the menu's size: a menu is laid out before anything is drawn), between
/// the mockup's narrowest and a cap.
fn width_of(rows: &[Row]) -> f32 {
    let words = rows
        .iter()
        .map(|r| match r {
            Row::Item { label, action, .. } => {
                let bold = if *action == Action::Back { 1.08 } else { 1.0 };
                let chev = if turns_page(*action) {
                    CHEV + 12.0
                } else {
                    8.0
                };
                TEXT_X + label.chars().count() as f32 * FONT_PX * 0.54 * bold + chev
            }
            Row::Note(text) => 20.0 + text.chars().count() as f32 * NOTE_PX * 0.54,
            Row::Sep => 0.0,
        })
        .fold(0.0, f32::max);
    (words + 2.0 * PAD).clamp(WIDTH, WIDTH_MAX).ceil()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn acts(rows: &[Row]) -> Vec<Action> {
        rows.iter().filter_map(Row::action).collect()
    }

    /// The groups of a menu, each its actions.
    fn groups(rows: &[Row]) -> Vec<Vec<Action>> {
        rows.split(|r| *r == Row::Sep).map(acts).collect()
    }

    #[test]
    fn every_menu_keeps_the_same_order_and_the_way_out_last() {
        let file = rows_for(&Ctx {
            on: On::File,
            here: true,
            stick: Some("KINGSTON".into()),
            ..Default::default()
        });
        assert_eq!(
            groups(&file),
            vec![
                vec![Action::Open, Action::OpenWith, Action::QuickLook],
                vec![Action::Copy, Action::Cut, Action::Duplicate],
                vec![Action::Rename, Action::Compress],
                vec![Action::CopyToStick, Action::SendPage],
                vec![Action::Properties],
                vec![Action::MoveToBin],
            ]
        );
        // No terminal on a single file; Move to bin is plain ink now.
        assert!(!acts(&file).contains(&Action::OpenTerminal));
        assert!(file
            .iter()
            .all(|r| !matches!(r, Row::Item { danger: true, .. })));
        let folder = rows_for(&Ctx {
            on: On::Folder,
            here: true,
            clip: true,
            ..Default::default()
        });
        assert_eq!(
            groups(&folder),
            vec![
                vec![Action::Open, Action::OpenInTab, Action::OpenTerminal],
                vec![
                    Action::Copy,
                    Action::Cut,
                    Action::PasteInto,
                    Action::Duplicate
                ],
                vec![Action::Rename, Action::Compress],
                vec![Action::SendPage],
                vec![Action::AddPlace],
                vec![Action::Properties],
                vec![Action::MoveToBin],
            ]
        );
        let stick = rows_for(&Ctx {
            on: On::Stick,
            ..Default::default()
        });
        assert_eq!(acts(&stick).last(), Some(&Action::Eject));
        assert!(stick.contains(&Row::danger("Format…", "drive", Action::Format)));
    }

    #[test]
    fn an_archive_extracts_in_place_of_compress_and_a_shortcut_has_no_open_with() {
        let archive = acts(&rows_for(&Ctx {
            on: On::Archive,
            here: true,
            ..Default::default()
        }));
        assert!(archive.contains(&Action::Extract) && !archive.contains(&Action::Compress));
        let app = acts(&rows_for(&Ctx {
            on: On::App,
            here: true,
            desk: true,
            untrusted: true,
            ..Default::default()
        }));
        assert_eq!(&app[..2], &[Action::Open, Action::AllowRun]);
        assert!(!app.contains(&Action::OpenWith) && app.contains(&Action::MoveToHome));
    }

    #[test]
    fn the_desktop_has_its_own_rows_in_their_order() {
        let all = rows_for(&Ctx {
            on: On::Desk,
            undo: Some("Move to bin".into()),
            clip: true,
            moving: Some(false),
            ..Default::default()
        });
        assert_eq!(
            groups(&all),
            vec![
                vec![Action::Undo],
                vec![Action::Paste],
                vec![Action::NewFolder, Action::SelectAll],
                vec![Action::SortPage, Action::CleanUp, Action::ToggleIcons],
                vec![
                    Action::NextWallpaper,
                    Action::Wallpapers,
                    Action::MovingWallpaper
                ],
                vec![Action::Colours, Action::Display],
            ]
        );
        assert!(all.contains(&Row::item("Undo Move to bin", "undo", Action::Undo)));
        // Most of the time: nothing to undo, nothing copied, a still picture.
        let bare = rows_for(&Ctx {
            on: On::Desk,
            ..Default::default()
        });
        assert_eq!(groups(&bare)[0], vec![Action::NewFolder, Action::SelectAll]);
        assert!(!acts(&bare).contains(&Action::MovingWallpaper));
    }

    #[test]
    fn the_bin_is_red_only_where_nothing_comes_back() {
        let item = rows_for(&Ctx {
            on: On::BinItem,
            ..Default::default()
        });
        assert_eq!(
            acts(&item),
            vec![Action::QuickLook, Action::PutBack, Action::Erase]
        );
        let empty = rows_for(&Ctx {
            on: On::BinGround,
            ..Default::default()
        });
        assert!(
            matches!(
                empty[0],
                Row::Item {
                    danger: true,
                    off: true,
                    ..
                }
            ),
            "nothing to empty"
        );
        let m = Menu::open(
            Ctx::default(),
            empty,
            None,
            false,
            (0.0, 0.0),
            1000.0,
            800.0,
        );
        let r = m.row_rects()[0];
        assert_eq!(
            m.hit((r.x + 5.0, r.y + 5.0)),
            None,
            "a row with nothing to do is not hit"
        );
    }

    #[test]
    fn a_phone_offers_only_what_works_and_says_why() {
        let v = Volume {
            phone: true,
            limits: Limits {
                debugging_off: true,
                ..Default::default()
            },
            ..Default::default()
        };
        let rows = rows_for(&Ctx {
            on: On::Phone,
            volume: v,
            ..Default::default()
        });
        let a = acts(&rows);
        assert!(
            !a.contains(&Action::Mirror)
                && !a.contains(&Action::Camera)
                && !a.contains(&Action::Configure)
        );
        assert!(a.contains(&Action::HelpDebugging));
        assert_eq!(a.last(), Some(&Action::Eject));
        let open = rows_for(&Ctx {
            on: On::Phone,
            volume: Volume {
                phone: true,
                ..Default::default()
            },
            ..Default::default()
        });
        assert!(
            acts(&open).contains(&Action::Configure)
                && !acts(&open).contains(&Action::OpenTerminal)
        );
    }

    #[test]
    fn a_menu_hangs_right_and_below_the_pointer_and_flips_at_the_edges() {
        let ctx = Ctx {
            on: On::Ground,
            ..Default::default()
        };
        let m = Menu::new(ctx.clone(), None, false, (100.0, 100.0), 1000.0, 800.0);
        assert_eq!((m.rect.x, m.rect.y), (106.0, 106.0));
        assert!(m.rect.w >= WIDTH);
        assert_eq!(m.rect.h, height_of(&m.rows));
        let m = Menu::new(ctx, None, false, (950.0, 780.0), 1000.0, 800.0);
        assert_eq!(m.rect.x, 950.0 - GAP - m.rect.w);
        assert!(m.rect.y + m.rect.h <= 780.0 - GAP + 0.01);
    }

    #[test]
    fn hit_finds_rows_but_never_rules_or_the_padding() {
        let m = Menu::new(
            Ctx {
                on: On::File,
                ..Default::default()
            },
            Some(0),
            false,
            (0.0, 0.0),
            1000.0,
            800.0,
        );
        let rects = m.row_rects();
        let mid = |r: &Rect| (r.x + r.w / 2.0, r.y + r.h / 2.0);
        assert_eq!(m.hit(mid(&rects[0])), Some(0));
        let rule = m.rows.iter().position(|r| *r == Row::Sep).unwrap();
        assert_eq!(m.hit(mid(&rects[rule])), None);
        assert_eq!(m.hit((m.rect.x + 1.0, m.rect.y + 1.0)), None, "the padding");
        assert_eq!(m.hit((5000.0, 5000.0)), None);
    }

    #[test]
    fn a_page_turns_and_leads_back() {
        let mut m = Menu::new(
            Ctx {
                on: On::File,
                ..Default::default()
            },
            Some(0),
            false,
            (100.0, 700.0),
            1000.0,
            800.0,
        );
        let first = m.rows.clone();
        let targets = vec![
            Target {
                name: "KINGSTON".into(),
                phone: false,
                far: false,
            },
            Target {
                name: "Pixel 8".into(),
                phone: true,
                far: false,
            },
            Target {
                name: "studio-pc".into(),
                phone: false,
                far: true,
            },
        ];
        m.turn(Page::Send, send_rows(&targets, true));
        assert_eq!(m.page, Page::Send);
        assert_eq!(
            acts(&m.rows),
            vec![
                Action::Back,
                Action::SendTo(0),
                Action::SendTo(1),
                Action::SendTo(2)
            ]
        );
        assert_eq!(m.rows.last(), Some(&Row::note("Looking for more…")));
        assert!(m.rect.y + m.rect.h <= 800.0 + 0.01);
        m.show_main();
        assert_eq!(m.rows, first);
        // Nothing anywhere, and done asking: said, and no action.
        assert_eq!(
            send_rows(&[], false).last(),
            Some(&Row::note("Nothing is plugged in or nearby"))
        );
        let sort = sort_page(2, true);
        assert!(sort.contains(&Row::item("Size", "tick", Action::SortBy(2))));
        assert!(sort.contains(&Row::item("Descending", "tick", Action::SortDir(true))));
    }
    /// Every row wears a symbol that is drawn: a name `glyphs.rs` has (one
    /// it has not is an empty gap where the symbol belongs).
    #[test]
    fn every_row_wears_a_symbol_there_is() {
        let ons = [
            On::Ground,
            On::File,
            On::Archive,
            On::App,
            On::Folder,
            On::Many,
            On::Desk,
            On::BinItem,
            On::BinDeep,
            On::BinGround,
            On::BinPlace,
            On::Stick,
            On::Phone,
            On::Place,
        ];
        let mut menus = Vec::new();
        for on in ons {
            for all in [false, true] {
                for phone in [false, true] {
                    let limits = Limits {
                        debugging_off: all,
                        not_allowed: all,
                        no_files: all,
                        old_camera: all,
                    };
                    menus.push(rows_for(&Ctx {
                        on,
                        n: 3,
                        stick: all.then(|| "KINGSTON".into()),
                        here: all,
                        clip: all,
                        desk: phone,
                        untrusted: all,
                        placed: all,
                        system: !all,
                        hidden: all,
                        by_hand: all,
                        terminal: all,
                        undo: all.then(|| "Rename".into()),
                        icons_hidden: all,
                        moving: [None, Some(all)][usize::from(phone)],
                        fixed: !all,
                        bin_n: usize::from(all) * 4,
                        volume: Volume {
                            phone,
                            closed: !all,
                            camera: all,
                            tether: all,
                            limits,
                        },
                        smart: Vec::new(),
                    }));
                }
            }
        }
        let names = vec!["Seam".to_owned()];
        menus.push(open_with_rows(Some(&names)));
        let targets = [(false, false), (true, false), (false, true)].map(|(phone, far)| Target {
            name: "x".into(),
            phone,
            far,
        });
        menus.push(send_rows(&targets, true));
        menus.push(sort_page(1, false));
        let known: Vec<&str> = crate::glyphs::GLYPHS.iter().map(|(n, _)| *n).collect();
        for rows in &menus {
            for row in rows {
                if let Row::Item { label, icon, .. } = row {
                    assert!(
                        icon.is_empty() || known.contains(icon),
                        "{label}: no symbol {icon}"
                    );
                }
            }
        }
    }
}
