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

use smithay_client_toolkit::seat::keyboard::Keysym;

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

/// THE MENUS' SIZE (Max, 2026-10-21: "add ctrl +/-/0 to the context menus
/// (make it bigger or smaller)" — for kids and grandmas): ONE for every
/// menu, the owner's standing choice (`settings.menu_zoom`, `App::zoom`'s
/// "menu"), a tenth a step from 80 % to 200 %. A menu is laid out at it as
/// it opens, and again at once when it changes (`Menu::rescale`) — rows,
/// words, symbols, air, corners and width together. (0: not read yet.)
static SCALE: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
pub(crate) const SCALE_MIN: f32 = 0.8;
pub(crate) const SCALE_MAX: f32 = 2.0;

/// The menus' size now (the first time asked, the owner's from the
/// settings; as shipped in tests).
pub(crate) fn scale() -> f32 {
    let bits = SCALE.load(std::sync::atomic::Ordering::Relaxed);
    if bits != 0 {
        return f32::from_bits(bits);
    }
    #[cfg(not(test))]
    set_scale(crate::settings::Settings::load().menu_zoom);
    #[cfg(test)]
    set_scale(0.0);
    f32::from_bits(SCALE.load(std::sync::atomic::Ordering::Relaxed))
}

/// Make every menu from now on this size (0: as shipped).
pub(crate) fn set_scale(s: f32) {
    let s = if s > 0.0 {
        s.clamp(SCALE_MIN, SCALE_MAX)
    } else {
        1.0
    };
    SCALE.store(s.to_bits(), std::sync::atomic::Ordering::Relaxed);
}
/// A symbol's ink (the mockup's 75 %), a note's, a rule's; a row that has
/// nothing to do now.
const ICON_INK: f32 = 0.75;
const NOTE_INK: f32 = 0.5;
const LINE_INK: f32 = 0.12;
const OFF_INK: f32 = 0.38;
/// How far the row under the pointer is tinted toward the accent, in the
/// light and in the dark; toward the red on a red row.
const HOVER: (f32, f32) = (0.16, 0.22);
const HOVER_RED: (f32, f32) = (0.14, 0.20);

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

/// What a key on a menu asks of its host (`Menu::key`).
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) enum MenuKey {
    /// Nothing for it (or nothing to do).
    Nothing,
    /// The highlight moved: draw again.
    Moved,
    /// Row `n` chosen (Enter, Space, → on a page's row).
    Choose(usize),
    /// Back to the first page (←).
    Back,
    /// Put away (Escape).
    Close,
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
    /// What the row under the pointer is tinted with (`MenuPaint::hover`).
    pub accent: [f32; 4],
}

impl MenuPaint {
    /// The accent the palette wears now (the desktop's menus).
    pub fn accent_now() -> [f32; 4] {
        lin(crate::palette::accent_srgb())
    }

    /// The row under the pointer: the panel tinted `k` of the way toward
    /// `tint` (`k_dark` in the dark), mixed as a page mixes them — and
    /// never so far that `ink`, the row's words as they are, reads much
    /// less well on it than on the panel (a very light accent in the dark
    /// stays a tint: at least 4.5 : 1, or four fifths of what the panel
    /// gives when that is less).
    pub fn hover(&self, tint: [f32; 4], k: f32, k_dark: f32, ink: [f32; 4]) -> [f32; 4] {
        let luma = |c: [f32; 4]| 0.2126 * c[0] + 0.7152 * c[1] + 0.0722 * c[2];
        let contrast = |a: [f32; 4], b: [f32; 4]| {
            let (x, y) = (luma(a) + 0.05, luma(b) + 0.05);
            x.max(y) / x.min(y)
        };
        let srgb = |v: f32| {
            if v <= 0.003_130_8 {
                v * 12.92
            } else {
                1.055 * v.powf(1.0 / 2.4) - 0.055
            }
        };
        let f = self.fill;
        let at = |k: f32| {
            let m = |i: usize| srgb(f[i]) + (srgb(tint[i]) - srgb(f[i])) * k;
            lin([m(0), m(1), m(2), f[3]])
        };
        let floor = 4.5_f32.min(contrast(ink, f) * 0.8);
        let dark = luma(self.ink) > luma(f);
        let mut k = if dark { k_dark } else { k };
        while k > 0.0 && contrast(ink, at(k)) < floor {
            k -= 0.01;
        }
        at(k.max(0.0))
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
    /// Its size (`scale()` as it opened, or since).
    pub scale: f32,
    /// The corner that stays where it is — at the pointer, or by the button
    /// it hangs from — whatever its size or page: its left edge or its
    /// right, its top or its foot (`Menu::hang`).
    anchor: (f32, f32),
    from_left: bool,
    from_top: bool,
    /// THE KEYS (Max, 2026-10-21): where the pointer stood when they last
    /// took the highlight — it stays theirs until the pointer moves.
    keys_at: Option<Option<(f32, f32)>>,
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
        let s = scale();
        let (width, height, gap) = (width_of(&rows, s), height_of(&rows, s), GAP * s);
        // To the right of the pointer and below it — or, with no room
        // there, to its left, above it.
        let from_left = at.0 + gap + width <= w;
        let from_top = at.1 + gap + height <= h;
        let anchor = (
            if from_left { at.0 + gap } else { at.0 - gap },
            if from_top { at.1 + gap } else { at.1 - gap },
        );
        let mut menu = Self {
            item,
            many,
            ctx,
            page: Page::Main,
            at,
            rows,
            rect: Rect::new(0.0, 0.0, width, height),
            room: (w, h),
            hover: None,
            pressed: None,
            t: 0.0,
            grow_from: None,
            scale: s,
            anchor,
            from_left,
            from_top,
            keys_at: None,
        };
        menu.place();
        menu
    }

    /// Hang it from `anchor` instead: that point its left edge or its
    /// right (`from_left`), its top or its foot (`from_top`) — the
    /// selection pill's ⋯ hangs UP from over the pill, the sort menu down
    /// from its button.
    pub fn hang(&mut self, anchor: (f32, f32), from_left: bool, from_top: bool) {
        (self.anchor, self.from_left, self.from_top) = (anchor, from_left, from_top);
        self.place();
    }

    /// Its panel: the size its rows want at its scale, its corner at its
    /// anchor, shifted back onto the surface where it would leave it.
    fn place(&mut self) {
        let s = self.scale;
        let (w, h) = (width_of(&self.rows, s), height_of(&self.rows, s));
        let (ax, ay) = self.anchor;
        let x = if self.from_left { ax } else { ax - w };
        let y = if self.from_top { ay } else { ay - h };
        let x = x.min(self.room.0 - w).max(0.0);
        let y = y.min(self.room.1 - h).max(0.0);
        self.rect = Rect::new(x, y, w, h);
    }

    /// At size `s` now (the owner's Ctrl +/−/0): laid out again at once,
    /// its corner where it was.
    pub fn rescale(&mut self, s: f32) {
        self.scale = s;
        self.place();
    }

    /// Put new rows in the panel where it stands: the same corner, the size
    /// that fits them, kept on the surface.
    fn set_rows(&mut self, rows: Vec<Row>) {
        self.rows = rows;
        self.place();
        self.hover = None;
        self.pressed = None;
        self.grow_from = None;
    }

    /// Turn to `page`, made of `rows`. (Turned by the keys: its first
    /// choice is highlighted.)
    pub fn turn(&mut self, page: Page, rows: Vec<Row>) {
        self.page = page;
        self.set_rows(rows);
        if self.keys_at.is_some() {
            self.hover = self.first_choice();
        }
    }

    /// Back to the first page. (By the keys: the row that turned to the
    /// page left is highlighted.)
    pub fn show_main(&mut self) {
        let from = self.page;
        let rows = rows_for(&self.ctx);
        self.turn(Page::Main, rows);
        if self.keys_at.is_some() {
            let to = match from {
                Page::OpenWith => Some(Action::OpenWith),
                Page::Send => Some(Action::SendPage),
                Page::Sort => Some(Action::SortPage),
                Page::Main => None,
            };
            if let Some(i) = to.and_then(|a| self.rows.iter().position(|r| r.action() == Some(a))) {
                self.hover = Some(i);
            }
        }
    }

    // ── the keys ──────────────────────────────────────────────────────

    /// A key while the menu is up (`at`: where the pointer is): ↓/↑ walk
    /// the rows that do something, round the ends; Home/End; Enter or Space
    /// choose; → turns a row's page, ← goes back; Escape closes; a letter
    /// goes to the next row it begins. What the host is to do.
    pub fn key(&mut self, keysym: Keysym, utf8: Option<&str>, at: Option<(f32, f32)>) -> MenuKey {
        match keysym {
            Keysym::Down | Keysym::KP_Down => self.key_step(true, at),
            Keysym::Up | Keysym::KP_Up => self.key_step(false, at),
            Keysym::Home | Keysym::KP_Home => self.key_end(false, at),
            Keysym::End | Keysym::KP_End => self.key_end(true, at),
            Keysym::Return | Keysym::KP_Enter | Keysym::space => {
                return self.hover.map_or(MenuKey::Nothing, MenuKey::Choose);
            }
            Keysym::Right | Keysym::KP_Right => {
                return match self.hover {
                    Some(i) if self.action(i).is_some_and(turns_page) => MenuKey::Choose(i),
                    _ => MenuKey::Nothing,
                };
            }
            Keysym::Left | Keysym::KP_Left if self.page != Page::Main => {
                self.keys_at = Some(at);
                return MenuKey::Back;
            }
            Keysym::Escape => return MenuKey::Close,
            _ => {
                let Some(c) = utf8
                    .and_then(|t| t.chars().next())
                    .filter(|c| !c.is_control())
                else {
                    return MenuKey::Nothing;
                };
                if !self.key_letter(c, at) {
                    return MenuKey::Nothing;
                }
            }
        }
        MenuKey::Moved
    }

    /// Whether row `i` does something (the keys stop only on those).
    fn is_choice(&self, i: usize) -> bool {
        matches!(self.rows.get(i), Some(Row::Item { off: false, .. }))
    }

    /// The first row that does something past the way back.
    fn first_choice(&self) -> Option<usize> {
        let choices: Vec<usize> = (0..self.rows.len())
            .filter(|&i| self.is_choice(i))
            .collect();
        choices
            .iter()
            .copied()
            .find(|&i| self.action(i) != Some(Action::Back))
            .or(choices.first().copied())
    }

    /// ↓ / ↑: the next row that does something, or the one before, round
    /// the ends; with none highlighted the first, or the last. `at`: where
    /// the pointer is (the highlight is the keys' until it moves).
    pub fn key_step(&mut self, down: bool, at: Option<(f32, f32)>) {
        let n = self.rows.len();
        let next = |i: usize| if down { (i + 1) % n } else { (i + n - 1) % n };
        let start = match self.hover {
            Some(i) => next(i),
            None if down => 0,
            None => n.saturating_sub(1),
        };
        let mut i = start;
        for _ in 0..n {
            if self.is_choice(i) {
                self.hover = Some(i);
                break;
            }
            i = next(i);
        }
        self.keys_at = Some(at);
    }

    /// Home / End: the first row that does something, or the last.
    pub fn key_end(&mut self, last: bool, at: Option<(f32, f32)>) {
        let mut choices = (0..self.rows.len()).filter(|&i| self.is_choice(i));
        self.hover = if last {
            choices.next_back()
        } else {
            choices.next()
        };
        self.keys_at = Some(at);
    }

    /// A letter: the next row whose words begin with it, round the end.
    /// Whether there was one.
    pub fn key_letter(&mut self, c: char, at: Option<(f32, f32)>) -> bool {
        let c = c.to_lowercase().next().unwrap_or(c);
        let n = self.rows.len();
        let from = self.hover.map_or(0, |i| i + 1);
        let found = (0..n).map(|k| (from + k) % n).find(|&i| {
            self.is_choice(i)
                && matches!(&self.rows[i], Row::Item { label, .. }
                    if label.to_lowercase().starts_with(c))
        });
        if found.is_some() {
            self.hover = found;
            self.keys_at = Some(at);
        }
        found.is_some()
    }

    /// The pointer is at `at`: the row under it is highlighted — unless the
    /// keys took the highlight and the pointer has not moved since.
    /// Whether the highlight changed.
    pub fn pointer(&mut self, at: Option<(f32, f32)>) -> bool {
        if self.keys_at == Some(at) {
            return false;
        }
        self.keys_at = None;
        let over = at.and_then(|p| self.hit(p));
        let changed = over != self.hover;
        self.hover = over;
        changed
    }

    /// Each row's rectangle, in order (rules included).
    pub fn row_rects(&self) -> Vec<Rect> {
        let (s, pad) = (self.scale, PAD * self.scale);
        let mut y = self.rect.y + pad;
        self.rows
            .iter()
            .map(|r| {
                let h = row_h(r) * s;
                let rect = Rect::new(self.rect.x + pad, y, self.rect.w - 2.0 * pad, h);
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
        // (Everything at its size.)
        let s = self.scale;
        // Its entrance: fading in while settling down from 4 px above — or,
        // back from the Properties box, that box's panel shrinking to ours.
        let lift = if self.grow_from.is_some() {
            0.0
        } else {
            (1.0 - t) * -4.0 * s
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
            radius: paint.radius * s,
            color: panel_fill,
            glass: paint.glass,
            border: 0.0,
        });
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
                        rect.x + 6.0 * s,
                        (rect.y + (rect.h - 1.0) / 2.0).round(),
                        rect.w - 12.0 * s,
                        1.0,
                    ),
                    radius: 0.0,
                    color: fade(ink_at(LINE_INK)),
                    glass: 0.0,
                    border: 0.0,
                }),
                Row::Note(text) => grid.labels.push(label(
                    text,
                    (rect.x + 10.0 * s, rect.y + rect.h - (NOTE_PX + 7.0) * s),
                    rect.w - 20.0 * s,
                    NOTE_PX * s,
                    (NOTE_PX + 4.0) * s,
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
                    // The row under the pointer: a SOFT TINT of the accent
                    // (red for what cannot be undone), at once (Max,
                    // 2026-10-21: a QUIET hover, as macOS and Windows — the
                    // accent band with white words flipping row to row was
                    // the blink — "but put at least some of the colour on
                    // the selection"); its words and symbol keep their ink,
                    // a red row its red.
                    let base = if *danger { red() } else { paint.ink };
                    if self.hover == Some(i) && !off {
                        let fill = if *danger {
                            paint.hover(red(), HOVER_RED.0, HOVER_RED.1, base)
                        } else {
                            paint.hover(paint.accent, HOVER.0, HOVER.1, base)
                        };
                        grid.rects.push(RectInst {
                            rect,
                            radius: ROW_RADIUS * s,
                            color: fade(fill),
                            glass: 0.0,
                            border: 0.0,
                        });
                    }
                    let k = if *off { OFF_INK } else { 1.0 };
                    let at = |a: f32| [base[0], base[1], base[2], base[3] * a * k];
                    let cy = rect.y + rect.h / 2.0;
                    if !icon.is_empty() {
                        let r = Rect::new(
                            (rect.x + ICON_X * s).round(),
                            (cy - ICON * s / 2.0).round(),
                            ICON * s,
                            ICON * s,
                        );
                        symbol(&mut grid, icon, r, at(ICON_INK));
                    }
                    let chev = turns_page(*action);
                    if chev {
                        let r = Rect::new(
                            (rect.x + rect.w - (8.0 + CHEV) * s).round(),
                            (cy - CHEV * s / 2.0).round(),
                            CHEV * s,
                            CHEV * s,
                        );
                        symbol(&mut grid, "chev", r, at(0.55));
                    }
                    let room = rect.w - (TEXT_X + if chev { CHEV + 12.0 } else { 8.0 }) * s;
                    grid.labels.push(label(
                        words,
                        (rect.x + TEXT_X * s, cy - LINE_PX * s / 2.0),
                        room,
                        FONT_PX * s,
                        LINE_PX * s,
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

/// Its height at size `s`.
fn height_of(rows: &[Row], s: f32) -> f32 {
    (PAD * 2.0 + rows.iter().map(row_h).sum::<f32>()) * s
}

/// As wide as its longest row's words want (an estimate of the words at
/// the menu's size: a menu is laid out before anything is drawn), between
/// the mockup's narrowest and a cap — at size `s`.
fn width_of(rows: &[Row], s: f32) -> f32 {
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
    ((words + 2.0 * PAD).clamp(WIDTH, WIDTH_MAX) * s).ceil()
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
        assert_eq!(m.rect.h, height_of(&m.rows, 1.0));
        let m = Menu::new(ctx, None, false, (950.0, 780.0), 1000.0, 800.0);
        assert_eq!(m.rect.x, 950.0 - GAP - m.rect.w);
        assert!(m.rect.y + m.rect.h <= 780.0 - GAP + 0.01);
    }

    /// Bigger or smaller (Ctrl +/−/0): everything together, the corner at
    /// the pointer where it was, shifted back onto the surface where it
    /// would leave it — and back where it was at its old size.
    #[test]
    fn a_menu_grows_from_its_corner_and_stays_on_the_surface() {
        let ctx = Ctx {
            on: On::File,
            here: true,
            ..Default::default()
        };
        let mut m = Menu::new(ctx.clone(), None, false, (100.0, 100.0), 1000.0, 800.0);
        let (w, h) = (m.rect.w, m.rect.h);
        let row = m.row_rects()[0];
        m.rescale(1.6);
        assert_eq!((m.rect.x, m.rect.y), (106.0, 106.0));
        assert!((m.rect.w - (w * 1.6).ceil()).abs() <= 1.0 && (m.rect.h - h * 1.6).abs() < 0.01);
        let big = m.row_rects()[0];
        assert!((big.h - row.h * 1.6).abs() < 0.01);
        // The row under a point stays the row under it, grown.
        assert_eq!(m.hit((big.x + 5.0, big.y + big.h / 2.0)), Some(0));
        // Flipped at the far corner: its right edge and its foot stay there.
        let mut m = Menu::new(ctx.clone(), None, false, (950.0, 780.0), 1000.0, 800.0);
        m.rescale(1.2);
        assert!((m.rect.x + m.rect.w - (950.0 - GAP)).abs() < 0.01);
        assert!((m.rect.y + m.rect.h - (780.0 - GAP)).abs() < 0.01);
        // Grown past the surface's right side: shifted back in; at its old
        // size, back where it was.
        let mut m = Menu::new(ctx, None, false, (700.0, 100.0), 1000.0, 800.0);
        m.rescale(2.0);
        assert!(m.rect.x + m.rect.w <= 1000.0 + 0.01 && m.rect.x < 706.0);
        m.rescale(1.0);
        assert_eq!(m.rect.x, 706.0);
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
    /// The row under the pointer is the panel tinted toward the accent —
    /// and a light accent in the dark stays a tint the words read on.
    #[test]
    fn the_hover_is_a_soft_tint_the_words_still_read_on() {
        let paint = |fill: [f32; 4], ink: [f32; 4]| MenuPaint {
            fill,
            ink,
            wash: [0.0; 4],
            radius: 10.0,
            glass: 0.0,
            accent: [0.0; 4],
        };
        let luma = |c: [f32; 4]| 0.2126 * c[0] + 0.7152 * c[1] + 0.0722 * c[2];
        let contrast = |a: [f32; 4], b: [f32; 4]| {
            let (x, y) = (luma(a) + 0.05, luma(b) + 0.05);
            x.max(y) / x.min(y)
        };
        // The light: #e8e5de toward #8148b3 by 16 % (in sRGB) = #d7cbd5.
        let ink = lin([0.11, 0.11, 0.11, 1.0]);
        let light = paint(lin([0.91, 0.898, 0.871, 1.0]), ink);
        let got = light.hover(lin([0.506, 0.282, 0.702, 1.0]), 0.16, 0.22, ink);
        let want = lin([0.8454, 0.8, 0.8439, 1.0]);
        for c in 0..3 {
            assert!((got[c] - want[c]).abs() < 0.004, "{got:?} {want:?}");
        }
        // The dark, a lavender accent: a tint, the light ink still at 4.5 : 1
        // or more.
        let ink = lin([0.9, 0.9, 0.92, 1.0]);
        let dark = paint(lin([0.07, 0.07, 0.08, 1.0]), ink);
        let lavender = lin([0.85, 0.72, 1.0, 1.0]);
        let got = dark.hover(lavender, 0.16, 0.22, ink);
        assert!(luma(got) > luma(dark.fill), "it is tinted");
        assert!(contrast(ink, got) >= 4.5, "{}", contrast(ink, got));
        // Even asked for far too much, it stops where the words still read.
        let got = dark.hover(lavender, 0.9, 0.9, ink);
        assert!(contrast(ink, got) >= 4.5 && luma(got) > luma(dark.fill));
        assert_eq!(got[3], 1.0);
    }

    /// A file's menu at (10, 10) on a big surface.
    fn file_menu() -> Menu {
        Menu::new(
            Ctx {
                on: On::File,
                here: true,
                ..Default::default()
            },
            Some(0),
            false,
            (10.0, 10.0),
            900.0,
            900.0,
        )
    }

    /// The words of row `i`.
    fn words(m: &Menu, i: Option<usize>) -> String {
        match i.and_then(|i| m.rows.get(i)) {
            Some(Row::Item { label, .. }) => label.clone(),
            _ => String::new(),
        }
    }

    #[test]
    fn the_keys_walk_the_rows_that_do_something_round_the_ends() {
        let mut m = file_menu();
        let at = Some((800.0, 800.0));
        assert_eq!(m.key(Keysym::Down, None, at), MenuKey::Moved);
        assert_eq!(words(&m, m.hover), "Open");
        // Over the rule between Quick look and Copy, never onto it.
        m.key(Keysym::Down, None, at);
        m.key(Keysym::Down, None, at);
        assert_eq!(words(&m, m.hover), "Quick look");
        m.key(Keysym::Down, None, at);
        assert_eq!(words(&m, m.hover), "Copy");
        // Round the ends, both ways; Home and End.
        m.key(Keysym::End, None, at);
        assert_eq!(words(&m, m.hover), "Move to bin");
        m.key(Keysym::Down, None, at);
        assert_eq!(words(&m, m.hover), "Open");
        m.key(Keysym::Up, None, at);
        assert_eq!(words(&m, m.hover), "Move to bin");
        m.key(Keysym::Home, None, at);
        assert_eq!(words(&m, m.hover), "Open");
        // A letter: the next row it begins, then the one after.
        m.key(Keysym::c, Some("c"), at);
        assert_eq!(words(&m, m.hover), "Copy");
        m.key(Keysym::c, Some("c"), at);
        assert_eq!(words(&m, m.hover), "Compress");
        // Enter and Space choose; Escape closes.
        assert_eq!(
            m.key(Keysym::Return, None, at),
            MenuKey::Choose(m.hover.unwrap())
        );
        assert_eq!(
            m.key(Keysym::space, None, at),
            MenuKey::Choose(m.hover.unwrap())
        );
        assert_eq!(m.key(Keysym::Escape, None, at), MenuKey::Close);
        // With nothing highlighted, the first ↑ takes the last.
        let mut m = file_menu();
        m.key(Keysym::Up, None, at);
        assert_eq!(words(&m, m.hover), "Move to bin");
    }

    #[test]
    fn the_keys_keep_the_highlight_until_the_pointer_moves() {
        let mut m = file_menu();
        let at = Some((800.0, 800.0));
        m.key(Keysym::Down, None, at);
        m.key(Keysym::Down, None, at);
        // The pointer still where it was: the highlight stays the keys'.
        assert!(!m.pointer(at));
        assert_eq!(words(&m, m.hover), "Open with");
        // It moves (onto Copy): the highlight is the pointer's again, and
        // the keys go on from there.
        let copy = m
            .rows
            .iter()
            .position(|r| r.action() == Some(Action::Copy))
            .unwrap();
        let r = m.row_rects()[copy];
        assert!(m.pointer(Some((r.x + 5.0, r.y + r.h / 2.0))));
        assert_eq!(m.hover, Some(copy));
        m.key(Keysym::Down, None, Some((r.x + 5.0, r.y + r.h / 2.0)));
        assert_eq!(words(&m, m.hover), "Move");
    }

    #[test]
    fn right_turns_a_rows_page_and_left_comes_back_to_it() {
        let mut m = file_menu();
        let at = None;
        m.key(Keysym::Down, None, at);
        // → on a row with no page does nothing; on Open with it is chosen.
        assert_eq!(m.key(Keysym::Right, None, at), MenuKey::Nothing);
        m.key(Keysym::Down, None, at);
        let open_with = m.hover.unwrap();
        assert_eq!(m.key(Keysym::Right, None, at), MenuKey::Choose(open_with));
        // (The host turns it.) The page's first choice is highlighted.
        m.turn(
            Page::OpenWith,
            open_with_rows(Some(&["GIMP".to_owned(), "Seam".to_owned()])),
        );
        assert_eq!(words(&m, m.hover), "GIMP");
        assert_eq!(m.key(Keysym::Left, None, at), MenuKey::Back);
        m.show_main();
        assert_eq!(m.hover, Some(open_with), "back on the row that turned it");
        // ← on the first page is nothing.
        assert_eq!(m.key(Keysym::Left, None, at), MenuKey::Nothing);
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
