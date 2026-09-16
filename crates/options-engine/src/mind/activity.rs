//! Context awareness: the mind's read of *what the user is doing*.
//!
//! Raw context (a window class, a git branch, a live mic) is not yet
//! understanding. [`infer_activity`] folds the whole [`ContextState`] into a
//! single [`Activity`] — the situation the mind reasons about. It is
//! deliberately a pure, ordered classifier: higher-priority situations (you're
//! in a call; you're presenting) win over ambient ones (something's playing).
//!
//! This is the seat of "context/activity awareness": every decision the mind
//! makes can be conditioned on the context, so options fit the situation and
//! the wrong ones are cleared away (pillars 2 & 3).
//!
//! # The seventeen
//!
//! This enum **is** the curated context list in `~/Golem/OPTIONS/contexts.md`,
//! approved by Max 2026-09-12 — it replaced eight coarse values that had no
//! drawing, no video editing, no gaming and no music production. Every OPTION's
//! `Shows in:` line names one of these.
//!
//! Two things this axis deliberately does NOT carry, because they are a
//! different question:
//!
//! * **Ambient states** — media playing, a live camera, on battery, a device
//!   arriving. Those are true *alongside* a context, so they stay as their own
//!   fields on [`ContextState`] and an offer reads them directly.
//! * **Shell arrangement** — normal tile, stage, overview, empty workspace,
//!   second monitor. That is *where* you are, not *what you are doing*, and it
//!   conditions OPTIONS just as hard. It lives in [`ShellState`](super::shell).

use serde::Serialize;

use crate::state::ContextState;

/// What the user is doing right now, at a glance. One at a time.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize)]
pub enum Activity {
    /// Nothing focused — an empty workspace.
    Idle,
    /// A live conversation: the mic is captured, or a meeting app is focused.
    Call,
    /// Showing something to an audience — slides fullscreen, or screencasting.
    Presenting,
    /// Playing.
    Gaming,
    /// Video with your attention on it: a player focused, or the focused
    /// browser is the thing making sound (a video tab).
    Watching,
    /// Writing software — an editor is focused, or a terminal where development
    /// work is actually happening (not merely one sitting in a repo).
    Coding,
    /// Prose — documents, notes, long-form.
    Writing,
    /// Raster and vector art, and layout.
    Drawing,
    /// Developing and correcting photographs.
    PhotoEditing,
    /// Cutting and grading video.
    VideoEditing,
    /// Composing, recording, mixing — a DAW.
    MusicProduction,
    /// Modelling, sculpting, parametric design — 3D and CAD.
    Modeling,
    /// Documents, books, documentation.
    Reading,
    /// General web, not clearly reading.
    Browsing,
    /// Chat, asynchronous.
    Messaging,
    /// Shell work that is not clearly coding.
    Terminal,
    /// Moving, sorting, organizing files.
    Files,
    /// Settings, packages, Modules. **The bar should go quiet here** — the user
    /// is already somewhere deliberate, and an offer is an interruption.
    Configuring,
    /// Focused on an app we don't classify. The honest fallback: guessing a
    /// context in order to have something to offer is how a shell starts lying.
    #[default]
    Unknown,
}

impl Activity {
    /// The stable lower-case name, for the `Shows in:` line of the OPTIONS list
    /// and for debug output. Kept in one place so a context can be renamed
    /// without hunting strings.
    pub fn name(self) -> &'static str {
        match self {
            Activity::Idle => "idle",
            Activity::Call => "on a call",
            Activity::Presenting => "presenting",
            Activity::Gaming => "gaming",
            Activity::Watching => "watching",
            Activity::Coding => "coding",
            Activity::Writing => "writing",
            Activity::Drawing => "drawing",
            Activity::PhotoEditing => "photo editing",
            Activity::VideoEditing => "video editing",
            Activity::MusicProduction => "music production",
            Activity::Modeling => "3d / cad",
            Activity::Reading => "reading",
            Activity::Browsing => "browsing",
            Activity::Messaging => "messaging",
            Activity::Terminal => "terminal",
            Activity::Files => "files",
            Activity::Configuring => "configuring",
            Activity::Unknown => "unknown",
        }
    }
}

// --- Window-class tables -----------------------------------------------------
//
// Substring match on the lower-cased class. Kept as plain data so a machine
// that meets an unlisted app is a one-line fix, not a redesign — the same
// growth rule the module census uses: add a key when a real app demands it.
//
// ORDER MATTERS where two tables could both match a class. `GAMES` is checked
// before `FILE_MANAGERS` because `dolphin-emu` (the GameCube emulator) would
// otherwise be caught by KDE's `dolphin`.

/// Code editors and IDEs.
const EDITORS: &[&str] = &[
    "code",
    "vscodium",
    "nvim",
    "vim",
    "neovide",
    "emacs",
    "zed",
    "sublime_text",
    "helix",
    "kate",
    "jetbrains",
    "idea",
    "pycharm",
    "goland",
    "clion",
    "webstorm",
    "rustrover",
];

/// Terminal emulators.
const TERMINALS: &[&str] = &[
    "foot",
    "alacritty",
    "kitty",
    "wezterm",
    "gnome-terminal",
    "konsole",
    "xterm",
    "urxvt",
    "terminator",
    "st",
    "tilix",
    "ghostty",
];

/// Web browsers.
const BROWSERS: &[&str] = &[
    "firefox",
    "chromium",
    "chrome",
    "google-chrome",
    "brave",
    "librewolf",
    "vivaldi",
    "edge",
    "zen",
];

/// Prose: word processors, note-takers, long-form editors.
const WRITERS: &[&str] = &[
    "libreoffice-writer",
    "libreoffice writer",
    "abiword",
    "ghostwriter",
    "obsidian",
    "logseq",
    "joplin",
    "zettlr",
    "typora",
    "marktext",
    "apostrophe",
    "gnome-text-editor",
];

/// Raster and vector art, and page layout.
const DRAWING_APPS: &[&str] = &[
    "krita",
    "gimp",
    "inkscape",
    "aseprite",
    "mypaint",
    "drawpile",
    "pinta",
    "scribus",
    "penpot",
    "figma",
];

/// Photo development and correction.
const PHOTO_APPS: &[&str] = &["darktable", "rawtherapee", "digikam", "shotwell", "luminance"];

/// Non-linear video editors.
const VIDEO_EDITORS: &[&str] = &[
    "kdenlive",
    "shotcut",
    "davinci",
    "resolve",
    "openshot",
    "flowblade",
    "olive",
    "pitivi",
];

/// DAWs and audio production.
const DAWS: &[&str] = &[
    "ardour",
    "reaper",
    "bitwig",
    "lmms",
    "renoise",
    "mixbus",
    "qtractor",
    "rosegarden",
    "audacity",
    "tenacity",
    "carla",
    "hydrogen",
];

/// 3D and CAD.
const MODELING_APPS: &[&str] = &[
    "blender",
    "freecad",
    "openscad",
    "solvespace",
    "meshlab",
    "librecad",
    "cura",
    "prusa-slicer",
    "prusaslicer",
];

/// Document and ebook readers.
const READERS: &[&str] = &[
    "zathura",
    "evince",
    "papers",
    "okular",
    "mupdf",
    "sioyek",
    "foliate",
    "calibre",
];

/// Video players — Watching when focused.
const PLAYERS: &[&str] = &[
    "mpv",
    "vlc",
    "celluloid",
    "totem",
    "haruna",
    "smplayer",
    "kodi",
];

/// Chat and mail.
const MESSAGING_APPS: &[&str] = &[
    "signal",
    "telegram",
    "discord",
    "element",
    "slack",
    "whatsapp",
    "thunderbird",
    "fractal",
    "dino",
    "gajim",
    "mattermost",
];

/// Meeting apps — the class-based fallback when the mic isn't (yet) live.
const MEETING_APPS: &[&str] = &["zoom", "teams", "jitsi", "skype", "webex"];

/// Presentation tools.
const PRESENTATION_APPS: &[&str] = &["impress", "powerpoint", "keynote", "pdfpc", "sent"];

/// Games, launchers and emulators. Checked BEFORE file managers — see the
/// `dolphin` note above.
const GAMES: &[&str] = &[
    "steam",
    "lutris",
    "heroic",
    "gamescope",
    "retroarch",
    "dolphin-emu",
    "minecraft",
    "ppsspp",
    "pcsx2",
    "rpcs3",
    "yuzu",
    "ryujinx",
];

/// File managers.
const FILE_MANAGERS: &[&str] = &[
    "nautilus",
    "dolphin",
    "thunar",
    "nemo",
    "pcmanfm",
    "caja",
    "spacefm",
    "krusader",
];

/// Settings, package managers, and Golem's own configuration surfaces.
const CONFIG_APPS: &[&str] = &[
    "gnome-control-center",
    "systemsettings",
    "pavucontrol",
    "blueman",
    "nm-connection-editor",
    "gnome-software",
    "plasma-discover",
    "synaptic",
    "gnome-disks",
    "golem-modules",
];

fn matches_any(class_lower: &str, set: &[&str]) -> bool {
    set.iter().any(|k| class_lower.contains(k))
}

/// Whether a browser window title reads like a documentation / reference page —
/// the reading-mode heuristic used in place of a browser bridge. Kept to strong
/// markers so a random page rarely trips it (a false positive only adds the mild
/// reading offers — find + brightness — which are harmless on any page).
fn title_looks_like_docs(title: &str) -> bool {
    let t = title.to_lowercase();
    const MARKERS: &[&str] = &[
        "documentation",
        "readthedocs",
        "read the docs",
        "mdn",
        "stack overflow",
        "wikipedia",
        "reference manual",
        "man page",
        "api reference",
        "developer guide",
        "user guide",
        " docs",
        " — docs",
    ];
    MARKERS.iter().any(|m| t.contains(m))
}

/// Whether the last command run in the focused shell is development work — the
/// evidence that turns a terminal from [`Activity::Terminal`] into
/// [`Activity::Coding`]. Deliberately a whitelist of tools you *build* with:
/// anything unrecognised (a monitor, a pager, ssh, cd, ls) leaves the terminal
/// alone, because the cost of a false Coding is a bar full of git controls the
/// user did not ask for, while the cost of a false Terminal is one extra
/// keystroke to run git yourself.
fn looks_like_dev_work(last_cmd: &str) -> bool {
    // The command word, after any leading env assignments (FOO=1 cargo …),
    // wrapper prefixes (`sudo`/`doas`/`env`/`nice`/`time`/`command`/`exec`)
    // AND those wrappers' own flags with their bare-number values
    // (`nice -n 10 make`, `env -i cargo`) — a flag or a number is never the
    // command. A path (…/bin/cargo) reduces to its file name.
    let word = last_cmd
        .split_whitespace()
        .find(|w| {
            !w.contains('=')
                && !w.starts_with('-')
                && !w.chars().all(|c| c.is_ascii_digit())
                && !matches!(
                    *w,
                    "sudo" | "doas" | "env" | "nice" | "time" | "command" | "exec"
                )
        })
        .unwrap_or("")
        .rsplit('/')
        .next()
        .unwrap_or("");
    const DEV_TOOLS: &[&str] = &[
        "git",
        "cargo",
        "rustc",
        "make",
        "cmake",
        "meson",
        "ninja",
        "gcc",
        "g++",
        "clang",
        "go",
        "zig",
        "npm",
        "pnpm",
        "yarn",
        "node",
        "deno",
        "python",
        "python3",
        "pip",
        "ruby",
        "gem",
        "mvn",
        "gradle",
        "dotnet",
        "nix",
        "nix-build",
        "nix-shell",
        "nixos-rebuild",
        "docker",
        "podman",
        "pytest",
        "tox",
        "rustfmt",
        "clippy-driver",
        "vim",
        "nvim",
        "emacs",
        "hx",
        "kak",
        "helix",
    ];
    DEV_TOOLS.contains(&word)
}

/// Whether the focused window is itself the thing playing — a video tab, as
/// opposed to music running in some other window. The MPRIS player name is
/// matched against the focused class both ways, since a browser reports
/// `firefox` while its class may be `org.mozilla.firefox`.
fn focused_window_is_the_player(ctx: &ContextState) -> bool {
    // The honest answer, now that the engine carries pids: the focused window
    // IS the player when the process making sound is the process that owns the
    // window. Before the inventory landed this had to be guessed by comparing
    // an MPRIS name against a window class, which got `firefox` vs
    // `org.mozilla.firefox` right and `VLC media player` vs `vlc` wrong.
    if ctx.focused_is_playing() {
        return true;
    }
    // Fallback for the sources that publish no pid at all (a phone over
    // kdeconnect, a headless player): compare names, loosely, as before.
    let class = ctx.window.class.to_lowercase();
    if class.is_empty() {
        return false;
    }
    ctx.playing_now().filter(|p| p.pid.is_none()).any(|p| {
        let app = p.app.to_lowercase();
        !app.is_empty() && (class.contains(&app) || app.contains(&class))
    })
}

/// Classify the current context from a snapshot. Ordered by priority: the first
/// situation that fits wins.
///
/// The order encodes what is *more specific*, not what is more important. A
/// live mic beats everything because a call is the most specific thing a person
/// can be doing at a keyboard; an app class beats "something is playing"
/// because having a tool open is more specific than having sound.
pub fn infer_activity(ctx: &ContextState) -> Activity {
    // Nothing focused.
    if ctx.window.pid == 0 && ctx.window.class.is_empty() {
        return Activity::Idle;
    }
    let class = ctx.window.class.to_lowercase();

    // A live mic dominates: you're in a conversation, whatever is on screen.
    if ctx.audio.is_mic_active {
        return Activity::Call;
    }

    // Showing something to an audience. Screencasting is the strong signal;
    // a presentation app gone fullscreen is the one that needs no portal.
    if ctx.is_screencasting
        || (matches_any(&class, PRESENTATION_APPS) && ctx.window.is_fullscreen)
    {
        return Activity::Presenting;
    }

    // Games before file managers: `dolphin-emu` vs KDE's `dolphin`.
    if matches_any(&class, GAMES) {
        return Activity::Gaming;
    }

    // Watching: a player focused, or the focused window IS the thing playing
    // (a browser video tab). Music in another window is ambient, not watching.
    if matches_any(&class, PLAYERS) || focused_window_is_the_player(ctx) {
        return Activity::Watching;
    }

    // A meeting app focused, before its mic goes live (joining, lobby).
    if matches_any(&class, MEETING_APPS) {
        return Activity::Call;
    }

    // Creation tools — each one specific enough to name on sight.
    if matches_any(&class, DRAWING_APPS) {
        return Activity::Drawing;
    }
    if matches_any(&class, PHOTO_APPS) {
        return Activity::PhotoEditing;
    }
    if matches_any(&class, VIDEO_EDITORS) {
        return Activity::VideoEditing;
    }
    if matches_any(&class, DAWS) {
        return Activity::MusicProduction;
    }
    if matches_any(&class, MODELING_APPS) {
        return Activity::Modeling;
    }
    if matches_any(&class, PRESENTATION_APPS) {
        return Activity::Presenting;
    }

    // The browser's two states. Reading docs is the more specific one; absent a
    // browser bridge (blocked: an extension needs a store upload, and enabling
    // the main browser's remote-debugging port is a security exposure), infer it
    // from the window title the compositor already gives us.
    if ctx.app_internal.is_reading_docs {
        return Activity::Reading;
    }
    if matches_any(&class, BROWSERS) || ctx.app_internal.browser_url.is_some() {
        if title_looks_like_docs(&ctx.window.title) {
            return Activity::Reading;
        }
        return Activity::Browsing;
    }

    // A document reader is Reading outright.
    if matches_any(&class, READERS) {
        return Activity::Reading;
    }

    // An editor is coding outright; a terminal has to earn it (just below).
    if matches_any(&class, EDITORS) || ctx.app_internal.editor_file.is_some() {
        return Activity::Coding;
    }
    if matches_any(&class, WRITERS) {
        return Activity::Writing;
    }
    if matches_any(&class, MESSAGING_APPS) {
        return Activity::Messaging;
    }
    if matches_any(&class, FILE_MANAGERS) {
        return Activity::Files;
    }
    if matches_any(&class, CONFIG_APPS) {
        return Activity::Configuring;
    }

    if matches_any(&class, TERMINALS) {
        // A terminal is Coding only when the user is DOING development in it —
        // not merely because its working directory happens to be a repo. That
        // rule filled the bar with git controls while Max sat watching btop
        // inside ~/Golem (2026-09-02: "the options engine is shit, im on btop
        // it show me 5 options for git hub"). A directory is not an intention.
        //
        // The evidence we accept is the last command the shell bridge saw: a
        // dev tool means dev work. A monitor (btop/top/htop), a pager, ssh, or
        // no command at all leaves it a plain Terminal.
        return if ctx.app_internal.editor_file.is_some()
            || ctx
                .app_internal
                .shell_last_cmd
                .as_deref()
                .is_some_and(looks_like_dev_work)
        {
            Activity::Coding
        } else {
            Activity::Terminal
        };
    }

    // Sound with no recognised tool in front of it: you are listening to
    // something. That is Watching's weakest form and the last thing we guess.
    if ctx.playing_now().next().is_some() {
        return Activity::Watching;
    }

    Activity::Unknown
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::state::{
        ActiveWindow, AppInternalContext, AudioState, GitContext, PlaybackState, Playing,
        PlayingSource,
    };

    /// The focused window is always pid 1 here, so a `Playing` with
    /// `pid: Some(1)` is "the thing in front of you" and any other pid is
    /// "somewhere else".
    fn focused(class: &str) -> ContextState {
        ContextState {
            window: ActiveWindow {
                class: class.into(),
                pid: 1,
                ..Default::default()
            },
            ..Default::default()
        }
    }

    fn plays(app: &str, pid: Option<u32>) -> Vec<Playing> {
        vec![Playing {
            source: PlayingSource::Stream { node: 1 },
            app: app.into(),
            title: String::new(),
            artist: String::new(),
            state: PlaybackState::Playing,
            pid,
            output: None,
            position_secs: 0,
            length_secs: 0,
            art_url: None,
        }]
    }

    #[test]
    fn empty_is_idle() {
        assert_eq!(infer_activity(&ContextState::default()), Activity::Idle);
    }

    #[test]
    fn live_mic_beats_everything() {
        let mut ctx = focused("foot");
        ctx.audio = AudioState {
            is_mic_active: true,
            ..Default::default()
        };
        ctx.playing = plays("VLC media player", Some(99));
        assert_eq!(infer_activity(&ctx), Activity::Call);
    }

    /// Every creation context is recognised on sight — the four that did not
    /// exist at all before the curated list (drawing, photo, video, music, 3D).
    #[test]
    fn the_creation_contexts_are_recognised() {
        for (class, want) in [
            ("krita", Activity::Drawing),
            ("GIMP", Activity::Drawing),
            ("org.inkscape.Inkscape", Activity::Drawing),
            ("darktable", Activity::PhotoEditing),
            ("rawtherapee", Activity::PhotoEditing),
            ("kdenlive", Activity::VideoEditing),
            ("resolve", Activity::VideoEditing),
            ("Ardour", Activity::MusicProduction),
            ("bitwig-studio", Activity::MusicProduction),
            ("audacity", Activity::MusicProduction),
            ("blender", Activity::Modeling),
            ("FreeCAD", Activity::Modeling),
        ] {
            assert_eq!(infer_activity(&focused(class)), want, "class {class:?}");
        }
    }

    #[test]
    fn the_rest_of_the_seventeen() {
        for (class, want) in [
            ("obsidian", Activity::Writing),
            ("libreoffice-writer", Activity::Writing),
            ("zathura", Activity::Reading),
            ("calibre", Activity::Reading),
            ("Signal", Activity::Messaging),
            ("thunderbird", Activity::Messaging),
            ("org.gnome.Nautilus", Activity::Files),
            ("thunar", Activity::Files),
            ("pavucontrol", Activity::Configuring),
            ("gnome-control-center", Activity::Configuring),
            ("steam", Activity::Gaming),
            ("lutris", Activity::Gaming),
            ("mpv", Activity::Watching),
            ("vlc", Activity::Watching),
            ("zoom", Activity::Call),
            ("Code", Activity::Coding),
            ("firefox", Activity::Browsing),
            ("foot", Activity::Terminal),
            ("somerandomapp", Activity::Unknown),
        ] {
            assert_eq!(infer_activity(&focused(class)), want, "class {class:?}");
        }
    }

    /// The one class collision the tables actually contain: KDE's file manager
    /// `dolphin` and the emulator `dolphin-emu`. Games are checked first, so
    /// both land correctly — and this test is why that order may not be
    /// "tidied" into alphabetical.
    #[test]
    fn dolphin_the_emulator_is_not_dolphin_the_file_manager() {
        assert_eq!(infer_activity(&focused("dolphin-emu")), Activity::Gaming);
        assert_eq!(infer_activity(&focused("org.kde.dolphin")), Activity::Files);
    }

    #[test]
    fn presenting_needs_an_audience_not_just_the_app() {
        // Slides in a window is authoring, not presenting.
        assert_eq!(
            infer_activity(&focused("libreoffice-impress")),
            Activity::Presenting,
            "a presentation app is Presenting even windowed — it has no other context"
        );
        // Fullscreen slides, and screencasting anything, are the strong cases.
        let mut full = focused("libreoffice-impress");
        full.window.is_fullscreen = true;
        assert_eq!(infer_activity(&full), Activity::Presenting);
        let mut casting = focused("Code");
        casting.is_screencasting = true;
        assert_eq!(
            infer_activity(&casting),
            Activity::Presenting,
            "sharing your screen outranks whatever is on it"
        );
    }

    /// Watching is about *your attention*, not about sound existing. The
    /// focused window must be the thing playing.
    #[test]
    fn watching_means_the_focused_window_is_the_player() {
        // A browser video tab: the sound comes from the focused window's own
        // process. Matched by PID now, not by guessing that "firefox" the MPRIS
        // name looks like "firefox" the window class.
        let mut tab = focused("firefox");
        tab.playing = plays("Mozilla Firefox", Some(1));
        assert_eq!(infer_activity(&tab), Activity::Watching);

        // Music from another window while you code is NOT watching.
        let mut coding = focused("Code");
        coding.playing = plays("Spotify", Some(742));
        assert_eq!(infer_activity(&coding), Activity::Coding);

        // Sound with no recognised tool in front of it is the weak fallback.
        let mut nothing = focused("somerandomapp");
        nothing.playing = plays("Spotify", Some(742));
        assert_eq!(infer_activity(&nothing), Activity::Watching);
    }

    /// The list is the point of #83: two players at once must both survive, and
    /// each must be findable by where it is. This is Max's sentence, as a test —
    /// *"a video on chrome on ws1 and music on spotify on ws5 and a video paused
    /// on ws8"*.
    #[test]
    fn three_players_on_three_workspaces_are_three_entries() {
        use crate::state::WindowInfo;
        fn win(pid: u32, class: &str, ws: i32) -> WindowInfo {
            WindowInfo {
                address: format!("0x{pid:x}"),
                class: class.into(),
                pid,
                workspace_id: ws,
                ..Default::default()
            }
        }
        fn play(app: &str, pid: u32, state: PlaybackState) -> Playing {
            Playing {
                source: PlayingSource::Stream { node: pid },
                app: app.into(),
                title: String::new(),
                artist: String::new(),
                state,
                pid: Some(pid),
                output: None,
                position_secs: 0,
                length_secs: 0,
                art_url: None,
            }
        }
        let mut ctx = focused("Code");
        ctx.windows = vec![
            win(10, "chromium", 1),
            win(20, "Spotify", 5),
            win(30, "mpv", 8),
        ];
        ctx.playing = vec![
            play("Chromium", 10, PlaybackState::Playing),
            play("Spotify", 20, PlaybackState::Playing),
            play("mpv", 30, PlaybackState::Paused),
        ];

        // All three are held, not collapsed to one.
        assert_eq!(ctx.playing.len(), 3);
        // Two are actually making sound; the paused one is still known.
        assert_eq!(ctx.playing_now().count(), 2);

        // And each one can say WHERE it is — the join that did not exist.
        let where_is = |app: &str| {
            ctx.playing
                .iter()
                .find(|p| p.app == app)
                .and_then(|p| ctx.workspace_of(p))
        };
        assert_eq!(where_is("Chromium"), Some(1));
        assert_eq!(where_is("Spotify"), Some(5));
        assert_eq!(where_is("mpv"), Some(8));

        // None of it is the focused window, so all of it is out of sight.
        assert_eq!(ctx.playing_out_of_sight().count(), 2);
        assert!(!ctx.focused_is_playing());
    }

    /// A repo working directory is NOT an intention: watching btop in a
    /// terminal that happens to sit in a repo must stay Terminal, or the bar
    /// fills with git controls nobody asked for (Max, 2026-09-02). It takes an
    /// actual dev command — or an open editor file — to make it Coding.
    #[test]
    fn terminal_needs_dev_work_not_just_a_repo_to_be_coding() {
        let mut ctx = focused("foot");
        assert_eq!(infer_activity(&ctx), Activity::Terminal);
        ctx.git = GitContext {
            branch: Some("main".into()),
            ..Default::default()
        };
        assert_eq!(
            infer_activity(&ctx),
            Activity::Terminal,
            "cwd in a repo alone is not coding"
        );
        for monitoring in [
            "btop",
            "htop",
            "top -d 1",
            "less NOTES.md",
            "ssh box",
            "ls -la",
        ] {
            ctx.app_internal.shell_last_cmd = Some(monitoring.into());
            assert_eq!(
                infer_activity(&ctx),
                Activity::Terminal,
                "'{monitoring}' is not development"
            );
        }
        for dev in [
            "git status",
            "cargo test --workspace",
            "sudo nixos-rebuild build-vm",
            "RUST_LOG=debug cargo run",
            "/run/current-system/sw/bin/nix build",
            "nvim src/main.rs",
        ] {
            ctx.app_internal.shell_last_cmd = Some(dev.into());
            assert_eq!(infer_activity(&ctx), Activity::Coding, "'{dev}' is dev work");
        }
        // An open editor file counts even with no shell command seen.
        ctx.app_internal.shell_last_cmd = None;
        ctx.app_internal.editor_file = Some("/home/max/x.rs".into());
        assert_eq!(infer_activity(&ctx), Activity::Coding);
    }

    /// The dev-work matcher's edge cases: paths, env prefixes, wrapper chains
    /// (with their flags), and the exact-word rule that keeps lookalikes out.
    #[test]
    fn dev_work_matcher_edge_cases() {
        // A path reduces to its file name.
        assert!(looks_like_dev_work("/usr/bin/git status"));
        assert!(looks_like_dev_work("/run/current-system/sw/bin/cargo build"));
        // Env assignments and wrapper prefixes are skipped — including chains,
        // and including the wrappers' own flags (a flag is never the command).
        assert!(looks_like_dev_work("FOO=1 BAR=2 cargo build"));
        assert!(looks_like_dev_work("sudo env RUST_LOG=debug nix build"));
        assert!(looks_like_dev_work("nice -n 10 make -j8"));
        assert!(looks_like_dev_work("time -v cargo test"));
        assert!(looks_like_dev_work("command git push"));
        assert!(looks_like_dev_work("exec nvim ."));
        // Exact word match: no substring creep in either direction.
        assert!(!looks_like_dev_work("gitk"));
        assert!(
            !looks_like_dev_work("./git-hooks.sh"),
            "a path that merely contains a tool name is not the tool"
        );
        // A non-dev command stays non-dev under any wrapper.
        assert!(!looks_like_dev_work("sudo btop"));
        assert!(!looks_like_dev_work("nice -n 5 htop"));
        // Degenerate inputs match nothing and never panic.
        assert!(!looks_like_dev_work(""));
        assert!(!looks_like_dev_work("   "));
        assert!(!looks_like_dev_work("sudo"));
        assert!(!looks_like_dev_work("FOO=1"));
        assert!(!looks_like_dev_work("--help"));
    }

    #[test]
    fn browser_is_browsing_unless_reading_docs() {
        assert_eq!(infer_activity(&focused("firefox")), Activity::Browsing);
        let mut ctx = focused("firefox");
        ctx.app_internal = AppInternalContext {
            is_reading_docs: true,
            ..Default::default()
        };
        assert_eq!(infer_activity(&ctx), Activity::Reading);
    }

    #[test]
    fn browser_doc_title_infers_reading_without_a_bridge() {
        let mut ctx = focused("firefox");
        for docish in [
            "array — Python documentation",
            "Array - JavaScript | MDN",
            "regex - How to ... - Stack Overflow",
            "requests: HTTP for Humans — Docs",
            "Rust By Example - User Guide",
        ] {
            ctx.window.title = docish.into();
            assert_eq!(infer_activity(&ctx), Activity::Reading, "{docish:?}");
        }
        // An ordinary page stays Browsing.
        ctx.window.title = "Cat videos - YouTube".into();
        assert_eq!(infer_activity(&ctx), Activity::Browsing);
        // The title heuristic only applies to browsers, not other apps.
        let mut term = focused("foot");
        term.window.title = "man page".into();
        assert_eq!(infer_activity(&term), Activity::Terminal);
    }

    #[test]
    fn doc_title_marker_matching() {
        assert!(title_looks_like_docs("NumPy Documentation"));
        assert!(title_looks_like_docs("something - MDN"));
        assert!(!title_looks_like_docs("My Cool Blog Post"));
        assert!(!title_looks_like_docs(""));
    }

    /// Every context has a distinct name, and no name is empty — the
    /// `Shows in:` line of the OPTIONS list is matched against these.
    #[test]
    fn every_context_has_a_distinct_name() {
        let all = [
            Activity::Idle,
            Activity::Call,
            Activity::Presenting,
            Activity::Gaming,
            Activity::Watching,
            Activity::Coding,
            Activity::Writing,
            Activity::Drawing,
            Activity::PhotoEditing,
            Activity::VideoEditing,
            Activity::MusicProduction,
            Activity::Modeling,
            Activity::Reading,
            Activity::Browsing,
            Activity::Messaging,
            Activity::Terminal,
            Activity::Files,
            Activity::Configuring,
            Activity::Unknown,
        ];
        assert_eq!(all.len(), 19, "17 contexts + Idle + Unknown");
        let mut names: Vec<&str> = all.iter().map(|a| a.name()).collect();
        names.sort_unstable();
        let before = names.len();
        names.dedup();
        assert_eq!(names.len(), before, "context names must be distinct");
        assert!(names.iter().all(|n| !n.is_empty()));
    }
}
