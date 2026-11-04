//! THE BOX'S TERMINAL (Max, 2026-11-03): *"when i do ./ i want a terminal…
//! autocompletion… send commands to different terminals, like './1 sudo…'
//! and that opens the command on the tab 1… tabs 1 2 3 that i can access all
//! the times"* — inside the box (his pick, look "A · Words" of the mockup
//! https://claude.ai/artifact/TY3gvBZj6k3CqrHgt9AuKe).
//!
//! THE ENGINE is tmux, its own server (`-L golem`, session `golem`, a window a
//! tab, counted from 1), started as a user service of its own
//! (`systemd-run`): the shells outlive the box closing and the dock
//! restarting, and get the session's environment, not the dock's. tmux does
//! everything a terminal is (escape codes, colours, the cursor, the history).
//!
//! THE BOX IS A VIEW of one tab: a control-mode client (`-C attach`) only
//! tells us when a pane printed or the windows changed; what a tab shows is
//! asked of tmux (`capture-pane -e`, its colours as SGR) on a thread of its
//! own, and keys go back with `send-keys` (text as bytes, `-H`: nothing to
//! quote).
//!
//! WHAT IS TYPED: `./` starts it. `./2 sudo nixos-rebuild switch` + Enter runs
//! the command in tab 2 and shows it; `./2` + Enter shows tab 2; `./ls` runs
//! in the tab shown (`./configure` is that script when the tab's folder has
//! it). A number that is not a tab yet makes it. A program on the whole
//! screen (vim, htop, less: tmux's alternate screen) takes every key until it
//! ends, Ctrl+Escape giving the keys back to the field; a password asked
//! (sudo's) is typed in the field as dots.
//!
//! COMPLETIONS AND HISTORY (Max: "add autocompletion and history next and
//! make the tabs clickeable and i guess the tmux binds also"): what is typed
//! shows rows over the tabs, the best nearest the field — commands typed
//! before (the shell's history file) that begin so, the programs on the
//! shell's PATH for the first word, the tab folder's folders and files for
//! the others; Up and Down pick, Tab takes the one picked, Escape hides them.
//! With nothing typed, Up and Down walk the history. The tabs' words take a
//! click, a "+" after them makes a tab, and tmux's prefix works as in tmux:
//! Ctrl+B then a number, n, p, l (the last one), c (a new one), Ctrl+B again
//! for the program's own.

use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::mpsc;
use std::time::{Duration, Instant};

use calloop::channel::Event;
use smithay_client_toolkit::seat::keyboard::Keysym;
use tracing::{info, warn};

use crate::App;

/// tmux's socket name (a test rig's its own: `GOLEM_RIG`) and the session the
/// tabs are the windows of.
fn socket() -> &'static str {
    if std::env::var_os("GOLEM_RIG").is_some() {
        "golem-rig"
    } else {
        "golem"
    }
}
const SESSION: &str = "golem";
/// The user service the server runs as.
const UNIT: &str = "golem-terminal";
/// What a tab's shell can be: a tab running one of these is at its prompt.
const SHELLS: &[&str] = &["zsh", "bash", "fish", "sh", "dash", "nu", "ksh", "tcsh", "elvish", "xonsh"];
/// The quickest a tab's screen is read again while it prints.
const CAPTURE_EVERY: Duration = Duration::from_millis(33);
/// The most completions shown at once (and the history's, searched).
const MAX_COMPS: usize = 5;
const MAX_SEARCH: usize = 8;
/// The most commands of the history kept.
const MAX_HISTORY: usize = 20_000;
/// A command that ran this long and ended while its tab was out of sight
/// says so (a notification).
const LONG_RUN: Duration = Duration::from_secs(10);
/// How long the PATH's programs are trusted before they are read again.
const PROGRAMS_FOR: Duration = Duration::from_secs(60);
/// Words that take a command after them: what follows them is a program.
const RUNNERS: &[&str] = &["sudo", "doas", "run0", "pkexec", "env", "nohup", "time", "watch", "exec", "command", "nice", "strace"];
/// The terminal's words: the box's size for a line of them.
pub(crate) const FONT_PX: f32 = 13.5;
pub(crate) const LINE_PX: f32 = 21.0;

/// The server's settings: no status line (the box shows the tabs), tabs from
/// 1, the windows the size the box asks for, keys straight through.
const CONF: &str = "\
set -g base-index 1
setw -g pane-base-index 1
set -g status off
set -g history-limit 20000
set -g default-terminal tmux-256color
set -sg escape-time 0
set -g window-size manual
set -g mouse on
set -g allow-rename on
set -g set-clipboard on
set -g focus-events on
setw -g automatic-rename on
";

/// One colour of a cell.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) enum Colour {
    #[default]
    Default,
    /// One of the 256 (0..16: the palette's sixteen).
    Idx(u8),
    Rgb(u8, u8, u8),
}

/// A run of cells alike, from column `col`.
#[derive(Clone, Debug, Default, PartialEq)]
pub(crate) struct Span {
    pub col: u16,
    pub text: String,
    pub fg: Colour,
    pub bg: Colour,
    pub bold: bool,
    pub dim: bool,
}

/// What a tab shows.
#[derive(Clone, Debug, Default)]
pub(crate) struct Screen {
    pub tab: u32,
    pub lines: Vec<Vec<Span>>,
    pub cursor: Option<(u16, u16)>,
    /// A program on the whole screen (vim, htop): it takes the keys.
    pub alternate: bool,
    pub command: String,
    /// How many lines back it was read from (0: the screen as it is now),
    /// and how many the tab keeps above its screen.
    pub scroll: u32,
    pub history: u32,
}

/// How many of a screen's lines are in use: up to the last with words, or
/// the cursor's.
pub(crate) fn used_lines(lines: &[Vec<Span>], cursor: Option<(u16, u16)>) -> usize {
    lines.iter().rposition(|l| l.iter().any(|s| !s.text.trim().is_empty())).max(cursor.map(|(_, y)| usize::from(y))).map_or(0, |i| i + 1)
}

/// A screen line as its cells (a character each, spaces where nothing is).
pub(crate) fn line_cells(line: &[Span]) -> Vec<char> {
    let mut cells: Vec<char> = Vec::new();
    for span in line {
        let at = usize::from(span.col);
        if cells.len() < at {
            cells.resize(at, ' ');
        }
        cells.truncate(at);
        cells.extend(span.text.chars());
    }
    cells
}

/// The word round cell `col` of a line, as columns (a double click's).
fn word_at(cells: &[char], col: usize) -> Option<(usize, usize)> {
    let is = |c: char| !c.is_whitespace() && !"\"'`()[]{}<>|,;".contains(c);
    if !cells.get(col).is_some_and(|&c| is(c)) {
        return None;
    }
    let mut a = col;
    while a > 0 && is(cells[a - 1]) {
        a -= 1;
    }
    let mut b = col;
    while b + 1 < cells.len() && is(cells[b + 1]) {
        b += 1;
    }
    Some((a, b))
}

/// A link under cell `col`: an address (http, https, file), or a path that is
/// there (from the tab's folder `dir`; a ":12:3" after it left out).
pub(crate) fn link_at(cells: &[char], col: usize, dir: &str) -> Option<String> {
    let (a, b) = word_at(cells, col)?;
    let word: String = cells[a..=b].iter().collect();
    let word = word.trim_end_matches(['.', ',', ';', ':', '!', '?', '\'', '"']);
    if ["http://", "https://", "file://"].iter().any(|p| word.starts_with(p)) {
        return Some(word.to_owned());
    }
    let mut path = word;
    while let Some((head, tail)) = path.rsplit_once(':') {
        if tail.is_empty() || tail.bytes().all(|b| b.is_ascii_digit()) {
            path = head;
        } else {
            break;
        }
    }
    let home = std::env::var("HOME").unwrap_or_default();
    let full = if let Some(rest) = path.strip_prefix("~/") {
        Path::new(&home).join(rest)
    } else {
        Path::new(dir).join(path)
    };
    full.exists().then(|| full.to_string_lossy().into_owned())
}

/// A tab: tmux's window `n` and its pane.
#[derive(Clone, Debug, Default, PartialEq)]
pub(crate) struct Tab {
    pub n: u32,
    pub pane: String,
    pub command: String,
    pub path: String,
    /// The name given by hand (Ctrl+B ,), when one was.
    pub title: Option<String>,
}

impl Tab {
    /// What its word says after the number: what runs in it, or — at the
    /// shell's prompt — its folder.
    /// Whether it is at its shell's prompt (nothing runs in it).
    pub(crate) fn idle(&self) -> bool {
        self.command.is_empty() || SHELLS.contains(&self.command.as_str())
    }

    pub(crate) fn name(&self) -> String {
        if let Some(t) = &self.title {
            return t.clone();
        }
        if !SHELLS.contains(&self.command.as_str()) && !self.command.is_empty() {
            return self.command.clone();
        }
        let home = std::env::var("HOME").unwrap_or_default();
        if self.path == home {
            return "~".into();
        }
        Path::new(&self.path).file_name().map_or_else(|| self.path.clone(), |n| n.to_string_lossy().into_owned())
    }
}

/// A key for a tab: text (sent as its bytes) or one of tmux's key names.
#[derive(Clone, Debug, PartialEq)]
pub(crate) enum Key {
    Text(String),
    Name(String),
}

/// What a completion is: a command typed before (when), a program, a
/// script of the tab's folder, a folder, a file.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum CompKind {
    History(i64),
    Program,
    Script,
    Folder,
    File,
    /// What zsh's own completion offers for an argument.
    Arg,
    /// A folder often gone to (`cd`).
    Frequent,
    /// A word the tab's screen shows (a hash, a number, a name printed).
    Screen,
    /// Ctrl+B ?: a key and what it does.
    Help,
}

/// One completion: the whole command it makes of what is typed.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Comp {
    pub text: String,
    pub kind: CompKind,
    /// What zsh says it is ("-- create a new archive"), when it says.
    pub desc: String,
}

impl Comp {
    pub(crate) fn new(text: String, kind: CompKind) -> Self {
        Comp { text, kind, desc: String::new() }
    }

    /// Its note at the row's end: "history · 3 h", "program", "folder".
    pub(crate) fn note(&self) -> String {
        use crate::i18n::tr;
        match self.kind {
            CompKind::History(when) => {
                let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0.0, |d| d.as_secs_f64());
                let ago = crate::deep_search::ago(when as f64, now);
                if ago.is_empty() {
                    tr("history").to_string()
                } else {
                    format!("{} · {ago}", tr("history"))
                }
            }
            _ if !self.desc.is_empty() => {
                let d: String = self.desc.chars().take(48).collect();
                if d.len() < self.desc.len() {
                    format!("{d}…")
                } else {
                    d
                }
            }
            CompKind::Arg | CompKind::Help => String::new(),
            CompKind::Frequent => tr("frequent").to_string(),
            CompKind::Screen => tr("on screen").to_string(),
            CompKind::Program => tr("program").to_string(),
            CompKind::Script => tr("script").to_string(),
            CompKind::Folder => tr("folder").to_string(),
            CompKind::File => tr("file").to_string(),
        }
    }
}

/// How the caret moves in the field: a character, a word (letters and
/// digits), a word as the shell splits them (spaces), to an end.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Move {
    Back,
    On,
    WordBack,
    WordOn,
    SpaceBack,
    Start,
    End,
}

/// Ctrl+F: what is looked for in a tab (its history and its screen), where
/// it was found and which find is shown.
#[derive(Clone, Debug, Default)]
pub(crate) struct Find {
    pub needle: String,
    /// Each find: its line (counted from the history's first), column and
    /// length, in cells; top to bottom.
    pub hits: Vec<(u64, u16, u16)>,
    pub at: usize,
}

/// Where `needle` is in `lines` (small letters: either case): line, column,
/// length — the lines numbered from 0.
pub(crate) fn find_in(lines: &[&str], needle: &str) -> Vec<(u64, u16, u16)> {
    if needle.is_empty() {
        return Vec::new();
    }
    let fold = !needle.chars().any(char::is_uppercase);
    let want: Vec<char> = needle.chars().collect();
    let mut hits = Vec::new();
    for (i, line) in lines.iter().enumerate() {
        let cells: Vec<char> = if fold { line.chars().map(|c| c.to_lowercase().next().unwrap_or(c)).collect() } else { line.chars().collect() };
        let mut at = 0;
        while at + want.len() <= cells.len() {
            if cells[at..at + want.len()] == want[..] {
                hits.push((i as u64, at as u16, want.len() as u16));
                at += want.len();
            } else {
                at += 1;
            }
        }
    }
    hits
}

/// What the box asks the engine's thread.
enum Req {
    /// The server up (made if it is not), the watcher on, the tabs read.
    Start { cols: u16, rows: u16 },
    Tabs,
    /// Its screen, `scroll` lines back (`rows` tall).
    Capture { tab: u32, pane: String, scroll: u32, rows: u16 },
    /// Keys for tab `tab` (made first when it is not there yet, in `dir`).
    Keys { tab: u32, keys: Vec<Key>, dir: Option<String> },
    Resize { cols: u16, rows: u16 },
    /// What completes `cmd` in the folder `dir` (`all`: asked with Tab, the
    /// folder's files even for no word yet).
    Complete { cmd: String, dir: String, all: bool },
    /// The shell's history file, when it changed since it was last read.
    History,
    /// Text pasted into a tab (as a paste: bracketed when its program asks).
    Paste { tab: u32, text: String },
    /// A tab closed (what runs in it ended).
    Close { tab: u32 },
    /// A tab's name given by hand (empty: named by what runs in it again).
    Rename { tab: u32, name: String },
    /// What a tab printed since its history's line `from`, to the prompt.
    CopyOutput { pane: String, from: u64 },
    /// Where `needle` is in a tab, all its history.
    Find { pane: String, needle: String },
}

/// What the engine's thread tells the box.
enum Ev {
    Tabs(Vec<Tab>),
    Screen(Screen),
    /// A pane printed.
    Printed(String),
    /// The windows changed (made, closed, renamed).
    Changed,
    /// The server is gone (every tab ended), or could not be started.
    Gone(String),
    /// The server could not be started: why (a `Gone` follows).
    Failed,
    Completions { cmd: String, rows: Vec<Comp> },
    /// What `CopyOutput` found.
    Copied(String),
    Found { needle: String, hits: Vec<(u64, u16, u16)> },
    History(Vec<(i64, String)>),
}

/// The terminal's state on the loop.
#[derive(Default)]
pub(crate) struct TermState {
    to_engine: Option<mpsc::Sender<Req>>,
    to_completer: Option<mpsc::Sender<Req>>,
    to_loop: Option<calloop::channel::Sender<Ev>>,
    pub tabs: Vec<Tab>,
    /// The tab shown when `./` names none.
    pub current: u32,
    pub screen: Option<Screen>,
    /// The size the box gives a tab, in cells.
    pub size: (u16, u16),
    /// A read of the shown tab on its way, another wanted after it, and when
    /// the last one was asked.
    capturing: bool,
    again: bool,
    asked_at: Option<Instant>,
    capture_timer: bool,
    /// The keys go to the field even though a program holds the screen.
    pub keys_to_field: bool,
    /// The cell's width at the terminal's size, measured.
    pub cell_w: f32,
    pub cell_for: f32,
    /// The tab words' widths, as last drawn.
    pub words_w: Vec<f32>,
    /// The engine said it could not start (its `Gone` is a failure).
    failed: bool,
    /// The commands typed before, oldest first, with when (0: not known):
    /// the shell's history file, then what the box sent since.
    hist: Vec<(i64, String)>,
    sent: Vec<(i64, String)>,
    /// Walking the history: how far back, what was typed before the walk,
    /// and the field as the walk left it (anything else typed ends it).
    hist_at: Option<usize>,
    hist_typed: String,
    hist_query: Option<String>,
    /// The completions shown (the best first: nearest the field), the
    /// engine's for `engine_for`, the one picked and whether by hand.
    pub comps: Vec<Comp>,
    engine_comps: Vec<Comp>,
    engine_for: String,
    pub comp_sel: usize,
    comp_moved: bool,
    /// Escape hid the completions of the field as it was.
    comps_hidden: Option<String>,
    /// Tab asked for every completion of the field as it was.
    comps_all: Option<String>,
    /// Ctrl+B pressed: the next key is a tab's (tmux's prefix).
    pub prefix: bool,
    /// The tab shown before (Ctrl+B l), and the one shown.
    last: u32,
    was_shown: u32,
    /// The completions' notes' widths, as last drawn.
    pub notes_w: Vec<f32>,
    /// The caret: how many characters before the end of what is typed.
    pub caret_back: usize,
    /// Ctrl+R: the rows are the whole history searched for what is typed
    /// (every word in it, fuzzy).
    pub searching: bool,
    /// What the history would add to what is typed (fish's suggestion):
    /// shown faint after it, Right takes it.
    pub ghost: String,
    /// Read back in the tab's history: how many lines (0: as it is now),
    /// and the wheel's part of a line not spent yet.
    pub scroll: u32,
    wheel: f32,
    /// The text picked with the pointer (cells: column, screen line), from
    /// where the press began to where it is; the pointer still down.
    pub sel: Option<((u16, usize), (u16, usize))>,
    selecting: bool,
    /// The last click (a second one soon on the same cell: its word).
    clicked: Option<(Instant, (u16, usize))>,
    /// A tab being named (Ctrl+B ,): which, and the name typed so far.
    pub renaming: Option<(u32, String)>,
    /// A tab asked to close while something runs in it: Enter closes it.
    pub closing: Option<u32>,
    /// Tabs that printed out of sight since they were last shown, and those
    /// whose last command failed out of sight.
    pub busy: std::collections::HashSet<u32>,
    pub failed_tabs: std::collections::HashSet<u32>,
    /// Where each tab's screen stood (its history's line) when the box last
    /// sent it a command: what it printed since is the last output.
    sent_at: std::collections::HashMap<u32, u64>,
    /// A word in the chip for a moment ("Copied").
    flash: Option<(&'static str, Instant)>,
    /// Ctrl+B ?: the rows are the keys.
    pub help: bool,
    /// Ctrl+F: the tab's history searched.
    pub find: Option<Find>,
    /// What runs in each tab and since when (its end out of sight, after a
    /// while, is told).
    running: std::collections::HashMap<u32, (String, Instant)>,
    /// When the tabs were last read on account of output, and a read owed.
    tabs_read: Option<Instant>,
    tabs_owed: bool,
    /// Each pane's status file as last read, and when each tab was told.
    status_seen: std::collections::HashMap<String, String>,
    /// When (unix seconds) the engine started here.
    started_at: i64,
    told_at: std::collections::HashMap<u32, Instant>,
    /// The folders the tabs went to, how often (counted, kept on disk) and
    /// when last; the folder each tab was last seen in.
    dirs: Option<Vec<(String, f32, i64)>>,
    tab_dirs: std::collections::HashMap<u32, String>,
}

/// What `./…` asks: a tab (none: the one shown) and a command (none: show it).
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Ask {
    pub tab: Option<u32>,
    pub cmd: String,
    /// More than one tab, or a new one: `./*`, `./1,3`, `./+`.
    pub many: Option<Many>,
}

/// The tabs a command goes to besides one.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum Many {
    /// Every tab (`./*`).
    All,
    /// These (`./1,3`).
    These(Vec<u32>),
    /// A new one, the next number free (`./+`): the command runs there, the
    /// tab shown stays.
    New,
}

/// `./2 sudo x` → tab 2, "sudo x"; `./2` → tab 2; `./ls -l` → "ls -l".
pub(crate) fn parse(query: &str) -> Option<Ask> {
    let rest = query.strip_prefix("./")?;
    // (What names the tabs ends at a space, or the end.)
    let word = rest.split(' ').next().unwrap_or("");
    let after = rest[word.len()..].trim_start().to_owned();
    match word {
        "*" => return Some(Ask { tab: None, cmd: after, many: Some(Many::All) }),
        "+" => return Some(Ask { tab: None, cmd: after, many: Some(Many::New) }),
        _ => {}
    }
    let tab = |d: &str| d.parse::<u32>().ok().filter(|n| (1..=99).contains(n));
    if !word.is_empty() && word.bytes().all(|b| b.is_ascii_digit()) {
        if let Some(n) = tab(word) {
            return Some(Ask { tab: Some(n), cmd: after, many: None });
        }
    }
    if word.contains(',') && word.split(',').all(|d| tab(d).is_some()) {
        let these = word.split(',').filter_map(tab).collect();
        return Some(Ask { tab: None, cmd: after, many: Some(Many::These(these)) });
    }
    Some(Ask { tab: None, cmd: rest.trim_start().to_owned(), many: None })
}

/// `./configure`: the command is that script when the tab's folder has a file
/// of that name; else the command as written.
fn as_script(cmd: &str, dir: &str) -> String {
    let first = cmd.split_whitespace().next().unwrap_or("");
    if !first.is_empty() && !first.contains('/') && Path::new(dir).join(first).is_file() {
        format!("./{cmd}")
    } else {
        cmd.to_owned()
    }
}

/// Whether the line the cursor is on asks for a password (sudo's
/// "[sudo] password for max: ", ssh's "Password:"): the field shows dots.
pub(crate) fn asks_password(screen: &Screen) -> bool {
    let Some((_, y)) = screen.cursor else {
        return false;
    };
    let line: String = screen.lines.get(usize::from(y)).map(|l| l.iter().map(|s| s.text.as_str()).collect()).unwrap_or_default();
    let l = line.trim_end().to_lowercase();
    (l.contains("password") || l.contains("passphrase") || l.contains("contraseña")) && l.ends_with(':')
}

/// tmux's name for a key pressed with these modifiers, for a program that
/// holds the screen; `None` for what is typed as text.
pub(crate) fn key_name(keysym: Keysym, ctrl: bool, alt: bool, utf8: Option<&str>) -> Option<Key> {
    let named = match keysym {
        Keysym::Return | Keysym::KP_Enter => Some("Enter"),
        Keysym::BackSpace => Some("BSpace"),
        Keysym::Tab => Some("Tab"),
        Keysym::ISO_Left_Tab => Some("BTab"),
        Keysym::Escape => Some("Escape"),
        Keysym::Up => Some("Up"),
        Keysym::Down => Some("Down"),
        Keysym::Left => Some("Left"),
        Keysym::Right => Some("Right"),
        Keysym::Home => Some("Home"),
        Keysym::End => Some("End"),
        Keysym::Prior => Some("PPage"),
        Keysym::Next => Some("NPage"),
        Keysym::Delete => Some("DC"),
        Keysym::Insert => Some("IC"),
        Keysym::F1 => Some("F1"),
        Keysym::F2 => Some("F2"),
        Keysym::F3 => Some("F3"),
        Keysym::F4 => Some("F4"),
        Keysym::F5 => Some("F5"),
        Keysym::F6 => Some("F6"),
        Keysym::F7 => Some("F7"),
        Keysym::F8 => Some("F8"),
        Keysym::F9 => Some("F9"),
        Keysym::F10 => Some("F10"),
        Keysym::F11 => Some("F11"),
        Keysym::F12 => Some("F12"),
        _ => None,
    };
    let with = |base: &str| {
        let mut s = String::new();
        if ctrl {
            s.push_str("C-");
        }
        if alt {
            s.push_str("M-");
        }
        s.push_str(base);
        s
    };
    if let Some(n) = named {
        return Some(Key::Name(with(n)));
    }
    let text = utf8.filter(|t| !t.is_empty())?;
    // (With Ctrl the text is the control character: name the letter.)
    if ctrl || alt {
        let raw = keysym.key_char().filter(|c| c.is_ascii_graphic()).map(|c| c.to_ascii_lowercase().to_string());
        return raw.map(|c| Key::Name(with(&c)));
    }
    if text.chars().any(char::is_control) {
        return None;
    }
    Some(Key::Text(text.to_owned()))
}

/// How long something took, in words: "42 s", "3 min", "1 h 20 min".
pub(crate) fn took_words(d: Duration) -> String {
    let s = d.as_secs();
    if s < 60 {
        format!("{s} s")
    } else if s < 3600 {
        format!("{} min", s / 60)
    } else {
        format!("{} h {} min", s / 3600, (s % 3600) / 60)
    }
}

/// A notification of the terminal's own (OPTIONS shows it).
fn notify(title: &str, body: &str) {
    info!("term: told \"{title}\"");
    let args = ["-a", "Terminal", "-i", "utilities-terminal", title, body].map(str::to_owned);
    let _ = std::thread::Builder::new().name("term-notify".into()).spawn(move || {
        if let Err(e) = Command::new("notify-send").args(&args).stdin(Stdio::null()).status() {
            warn!("term: no notification: {e}");
        }
    });
}

/// Ctrl+B ?'s rows: the keys, and what each does.
fn help_rows() -> Vec<Comp> {
    use crate::i18n::tr;
    [
        ("Ctrl+B 1…9 · Alt+1…9", tr("show that tab")),
        ("Ctrl+B n p · Ctrl+Tab", tr("the next tab, the one before")),
        ("Ctrl+B l", tr("the tab shown last")),
        ("Ctrl+B c · ./+", tr("a new tab")),
        ("Ctrl+B x · middle click", tr("close the tab")),
        ("Ctrl+B ,", tr("name the tab")),
        ("Ctrl+B y", tr("copy what the last command printed")),
        ("Ctrl+R", tr("search the history")),
        ("Ctrl+F", tr("find in the tab's history")),
        ("./* · ./1,3", tr("a command to every tab, to some")),
        ("Shift+PageUp · wheel", tr("back in the tab's history")),
        ("Ctrl+click", tr("open a link or a file")),
        ("Ctrl+Esc", tr("the keys back from a program")),
    ]
    .into_iter()
    .rev()
    .map(|(keys, what)| Comp { text: keys.to_owned(), kind: CompKind::Help, desc: what.to_owned() })
    .collect()
}

/// A key that is only a modifier (Ctrl, Shift…): never a key of its own.
fn is_modifier(keysym: Keysym) -> bool {
    matches!(
        keysym,
        Keysym::Shift_L
            | Keysym::Shift_R
            | Keysym::Control_L
            | Keysym::Control_R
            | Keysym::Alt_L
            | Keysym::Alt_R
            | Keysym::Super_L
            | Keysym::Super_R
            | Keysym::Meta_L
            | Keysym::Meta_R
            | Keysym::ISO_Level3_Shift
            | Keysym::Caps_Lock
    )
}

// ------------------------------------------------------------ the screen

/// One line of `capture-pane -e`: its runs of cells alike.
pub(crate) fn parse_line(line: &str) -> Vec<Span> {
    let mut spans: Vec<Span> = Vec::new();
    let mut cur = Span::default();
    let mut col: u16 = 0;
    let mut chars = line.chars().peekable();
    let flush = |spans: &mut Vec<Span>, cur: &mut Span, col: u16| {
        if !cur.text.is_empty() {
            let next = Span { col, text: String::new(), ..cur.clone() };
            spans.push(std::mem::replace(cur, next));
        } else {
            cur.col = col;
        }
    };
    while let Some(c) = chars.next() {
        if c == '\u{1b}' && chars.peek() == Some(&'[') {
            chars.next();
            let mut params = String::new();
            let mut fin = '\0';
            for d in chars.by_ref() {
                if d.is_ascii_digit() || d == ';' || d == ':' {
                    params.push(d);
                } else {
                    fin = d;
                    break;
                }
            }
            if fin == 'm' {
                flush(&mut spans, &mut cur, col);
                sgr(&mut cur, &params);
            }
            continue;
        }
        if c.is_control() {
            continue;
        }
        cur.text.push(c);
        col = col.saturating_add(1);
    }
    flush(&mut spans, &mut cur, col);
    spans
}

/// Apply one SGR's parameters to `span`'s look.
fn sgr(span: &mut Span, params: &str) {
    let ps: Vec<u32> = if params.is_empty() {
        vec![0]
    } else {
        params.split([';', ':']).map(|p| p.parse().unwrap_or(0)).collect()
    };
    let mut i = 0;
    while i < ps.len() {
        match ps[i] {
            0 => {
                span.fg = Colour::Default;
                span.bg = Colour::Default;
                span.bold = false;
                span.dim = false;
            }
            1 => span.bold = true,
            2 => span.dim = true,
            22 => {
                span.bold = false;
                span.dim = false;
            }
            7 => std::mem::swap(&mut span.fg, &mut span.bg),
            n @ 30..=37 => span.fg = Colour::Idx((n - 30) as u8),
            n @ 90..=97 => span.fg = Colour::Idx((n - 90 + 8) as u8),
            39 => span.fg = Colour::Default,
            n @ 40..=47 => span.bg = Colour::Idx((n - 40) as u8),
            n @ 100..=107 => span.bg = Colour::Idx((n - 100 + 8) as u8),
            49 => span.bg = Colour::Default,
            n @ (38 | 48) => {
                let colour = match ps.get(i + 1) {
                    Some(5) => {
                        let c = ps.get(i + 2).map(|&v| Colour::Idx(v.min(255) as u8));
                        i += 2;
                        c
                    }
                    Some(2) => {
                        let get = |k: usize| ps.get(i + k).map_or(0, |&v| v.min(255) as u8);
                        let c = Some(Colour::Rgb(get(2), get(3), get(4)));
                        i += 4;
                        c
                    }
                    _ => None,
                };
                if let Some(c) = colour {
                    if n == 38 {
                        span.fg = c;
                    } else {
                        span.bg = c;
                    }
                }
            }
            _ => {}
        }
        i += 1;
    }
}

/// One of the 256 as sRGB: past the sixteen, the 6×6×6 cube and the greys.
pub(crate) fn xterm(i: u8, sixteen: &[[f32; 4]; 16]) -> [f32; 4] {
    if i < 16 {
        return sixteen[usize::from(i)];
    }
    if i < 232 {
        let v = i - 16;
        let step = |c: u8| if c == 0 { 0.0 } else { (55.0 + 40.0 * f32::from(c)) / 255.0 };
        return [step(v / 36), step((v / 6) % 6), step(v % 6), 1.0];
    }
    let g = (8.0 + 10.0 * f32::from(i - 232)) / 255.0;
    [g, g, g, 1.0]
}

// ------------------------------------------------------------ history

/// The shell's history file: `HISTFILE` when it is given, else zsh's
/// (Golem's, in the state folder, or the classic one) or bash's, the one
/// written last.
fn history_file() -> Option<PathBuf> {
    if let Some(h) = std::env::var_os("HISTFILE").map(PathBuf::from).filter(|p| p.is_file()) {
        return Some(h);
    }
    let home = PathBuf::from(std::env::var_os("HOME")?);
    let state = std::env::var_os("XDG_STATE_HOME").map_or_else(|| home.join(".local/state"), PathBuf::from);
    [state.join("zsh/history"), home.join(".zsh_history"), home.join(".bash_history")]
        .into_iter()
        .filter_map(|p| Some((std::fs::metadata(&p).ok()?.modified().ok()?, p)))
        .max_by_key(|(t, _)| *t)
        .map(|(_, p)| p)
}

/// A history file's commands, oldest first, with when: zsh's extended lines
/// (": 1793750145:0;btop", its bytes "metafied"), bash's plain ones (a
/// "#1793750145" before each when it keeps the time). A command of several
/// lines is left out: the field is one line.
pub(crate) fn parse_history(bytes: &[u8]) -> Vec<(i64, String)> {
    // (zsh writes the bytes from 0x83 up as 0x83 and the byte xor 0x20.)
    let mut raw = Vec::with_capacity(bytes.len());
    let mut it = bytes.iter();
    while let Some(&b) = it.next() {
        if b == 0x83 {
            if let Some(&n) = it.next() {
                raw.push(n ^ 0x20);
            }
        } else {
            raw.push(b);
        }
    }
    let text = String::from_utf8_lossy(&raw);
    let mut out = Vec::new();
    let mut skipping = false;
    let mut bash_when = 0;
    for line in text.lines() {
        let more = line.ends_with('\\');
        if skipping || more {
            skipping = more;
            continue;
        }
        if let Some(t) = line.strip_prefix('#').filter(|t| !t.is_empty() && t.bytes().all(|b| b.is_ascii_digit())) {
            bash_when = t.parse().unwrap_or(0);
            continue;
        }
        let (when, cmd) = match line.strip_prefix(": ").and_then(|r| r.split_once(';')) {
            Some((meta, cmd)) => (meta.split(':').next().and_then(|t| t.trim().parse().ok()).unwrap_or(0), cmd),
            None => (std::mem::take(&mut bash_when), line),
        };
        if !cmd.trim().is_empty() {
            out.push((when, cmd.to_owned()));
        }
    }
    let extra = out.len().saturating_sub(MAX_HISTORY);
    out.drain(..extra);
    out
}

// ------------------------------------------------------------ completion

/// How well `needle` finds `hay` (Ctrl+R's search): every word of it must,
/// as a piece of it (the best) or its letters in order; a word typed in
/// small letters finds both. `None`: not found.
pub(crate) fn fuzzy(needle: &str, hay: &str) -> Option<i32> {
    let mut score = 0;
    for word in needle.split_whitespace() {
        let fold = !word.chars().any(char::is_uppercase);
        let h: String = if fold { hay.to_lowercase() } else { hay.to_owned() };
        if let Some(at) = h.find(word) {
            let start = at == 0 || h[..at].ends_with(|c: char| !c.is_alphanumeric());
            score += 100 + if start { 30 } else { 0 } - (at as i32 / 8).min(20);
            continue;
        }
        let mut chars = h.chars().peekable();
        let mut run = 0;
        let mut got = 0;
        for w in word.chars() {
            let mut gap = false;
            loop {
                match chars.next() {
                    Some(c) if c == w => break,
                    Some(_) => gap = true,
                    None => return None,
                }
            }
            run = if gap { 0 } else { run + 1 };
            got += 1 + run;
        }
        score += got;
    }
    Some(score)
}

/// Which characters of `hay` show what was typed: its start (a completion),
/// or — searching — the pieces and letters `fuzzy` found.
pub(crate) fn marks(needle: &str, hay: &str, searching: bool) -> Vec<bool> {
    let hc: Vec<char> = hay.chars().collect();
    let mut on = vec![false; hc.len()];
    if !searching {
        for (i, (a, b)) in hc.iter().zip(needle.chars()).enumerate() {
            if *a != b {
                break;
            }
            on[i] = true;
        }
        return on;
    }
    for word in needle.split_whitespace() {
        let fold = !word.chars().any(char::is_uppercase);
        let low: Vec<char> = if fold { hc.iter().map(|c| c.to_lowercase().next().unwrap_or(*c)).collect() } else { hc.clone() };
        let w: Vec<char> = word.chars().collect();
        if let Some(at) = (0..=low.len().saturating_sub(w.len())).find(|&i| low.len() >= w.len() && low[i..i + w.len()] == w[..]) {
            on[at..at + w.len()].iter_mut().for_each(|m| *m = true);
            continue;
        }
        let mut k = 0;
        for (i, c) in low.iter().enumerate() {
            if k < w.len() && *c == w[k] {
                on[i] = true;
                k += 1;
            }
        }
    }
    on
}

/// A name as the shell reads it: its spaces and signs escaped.
fn escape(name: &str) -> String {
    let mut s = String::with_capacity(name.len());
    for c in name.chars() {
        if c.is_whitespace() || "'\"\\()&;$`!*?[]{}<>|#~".contains(c) {
            s.push('\\');
        }
        s.push(c);
    }
    s
}

/// What is typed, as the name it means: its escapes undone.
fn unescape(word: &str) -> String {
    let mut s = String::with_capacity(word.len());
    let mut chars = word.chars();
    while let Some(c) = chars.next() {
        if c == '\\' {
            if let Some(n) = chars.next() {
                s.push(n);
            }
        } else {
            s.push(c);
        }
    }
    s
}

fn is_exec(p: &Path) -> bool {
    use std::os::unix::fs::PermissionsExt;
    std::fs::metadata(p).is_ok_and(|m| m.is_file() && m.permissions().mode() & 0o111 != 0)
}

/// The programs on the tabs' PATH (the server's own), each name once.
fn programs() -> Vec<String> {
    let path = run(&["show-environment", "-g", "PATH"])
        .and_then(|l| l.trim().strip_prefix("PATH=").map(str::to_owned))
        .or_else(|| std::env::var("PATH").ok())
        .unwrap_or_default();
    let mut names = std::collections::BTreeSet::new();
    for dir in path.split(':').filter(|d| !d.is_empty()) {
        let Ok(entries) = std::fs::read_dir(dir) else {
            continue;
        };
        for e in entries.flatten() {
            let name = e.file_name().to_string_lossy().into_owned();
            if !names.contains(&name) && is_exec(&e.path()) {
                names.insert(name);
            }
        }
    }
    names.into_iter().collect()
}

/// What completes `cmd` in the folder `dir`: its first word (or the one
/// after `sudo`) a program or a script of the folder, any other a folder or
/// a file (`all`: the folder's even with no word typed yet).
pub(crate) fn complete(cmd: &str, dir: &str, programs: &[String], all: bool) -> Vec<Comp> {
    let (head, word) = split_word(cmd);
    let first = first_word(head);
    let mut out = Vec::new();
    if word.is_empty() && (first || !all) {
        return out;
    }
    let home = PathBuf::from(std::env::var("HOME").unwrap_or_default());
    if first && !word.contains('/') {
        let mut found: Vec<&String> = programs.iter().filter(|p| p.starts_with(word) && p.as_str() != word).collect();
        found.sort_by(|a, b| a.len().cmp(&b.len()).then(a.cmp(b)));
        out.extend(found.into_iter().take(MAX_COMPS).map(|p| Comp::new(format!("{head}{} ", escape(p)), CompKind::Program)));
        if let Ok(entries) = std::fs::read_dir(dir) {
            let want = unescape(word);
            let mut scripts: Vec<String> = entries
                .flatten()
                .map(|e| e.file_name().to_string_lossy().into_owned())
                .filter(|n| n.starts_with(&want) && n != &want && (want.starts_with('.') || !n.starts_with('.')))
                .filter(|n| is_exec(&Path::new(dir).join(n)))
                .collect();
            scripts.sort();
            let room = MAX_COMPS.saturating_sub(out.len()).max(2);
            out.splice(0..0, scripts.into_iter().take(room).map(|n| Comp::new(format!("{head}{} ", escape(&n)), CompKind::Script)));
        }
        out.truncate(MAX_COMPS);
        return out;
    }
    // A path: the folder typed (from /, ~/ or the tab's folder) and the
    // start of a name in it.
    let cut = word.rfind('/').map_or(0, |i| i + 1);
    let (typed_dir, base) = word.split_at(cut);
    let plain_dir = unescape(typed_dir);
    let look = if plain_dir.starts_with('/') {
        PathBuf::from(&plain_dir)
    } else if let Some(r) = plain_dir.strip_prefix("~/") {
        home.join(r)
    } else {
        Path::new(dir).join(&plain_dir)
    };
    let base = unescape(base);
    let Ok(entries) = std::fs::read_dir(&look) else {
        return out;
    };
    let names: Vec<(bool, String)> = entries
        .flatten()
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .filter(|n| base.starts_with('.') || !n.starts_with('.'))
        .map(|n| (look.join(&n).is_dir(), n))
        .collect();
    let pick = |exact: bool| -> Vec<&(bool, String)> {
        let lower = base.to_lowercase();
        names
            .iter()
            .filter(|(_, n)| if exact { n.starts_with(&base) } else { n.to_lowercase().starts_with(&lower) })
            .filter(|(d, n)| *d || n != &base)
            .collect()
    };
    let mut found = pick(true);
    if found.is_empty() {
        found = pick(false);
    }
    found.sort_by(|a, b| b.0.cmp(&a.0).then_with(|| a.1.to_lowercase().cmp(&b.1.to_lowercase())));
    out.extend(found.into_iter().take(MAX_COMPS).map(|(d, n)| Comp {
        text: format!("{head}{typed_dir}{}{}", escape(n), if *d { "/" } else { " " }),
        kind: if *d { CompKind::Folder } else { CompKind::File },
        desc: String::new(),
    }));
    out
}

// ------------------------------------------------------------ zsh's own

/// The session the completion shell lives in — on a tmux server of its own
/// (the tabs' keeps its settings, and ends with its last tab).
const COMPLETE_SESSION: &str = "complete";
/// How long an answer of zsh's is waited for.
const ZSH_WAIT: Duration = Duration::from_millis(1500);

/// THE COMPLETION SHELL: a zsh of its own (none of the user's files: `-d`
/// and its own ZDOTDIR) in a tmux session of its own, its completion
/// system asked by the dock — the request a file ("folder NUL line"),
/// Ctrl+G the doorbell, the answer another file ("word TAB description" a
/// line). What every `compadd` would offer is caught (the technique of
/// zsh-capture-completion), nothing listed, nothing put in its line.
const COMPLETE_ZSH: &str = r#"zmodload zsh/zutil
typeset -g _gc_dir=${GOLEM_COMPLETE_DIR:?}
fpath=($fpath /run/current-system/sw/share/zsh/site-functions /etc/profiles/per-user/$USER/share/zsh/site-functions $HOME/.nix-profile/share/zsh/site-functions)
autoload -Uz compinit && compinit -i -d $_gc_dir/zcompdump
PROMPT='' RPROMPT=''
unsetopt zle_bracketed_paste 2>/dev/null
zstyle ':completion:*' list-grouped false
zstyle ':completion:*' insert-tab false
zstyle ':completion:*' verbose yes
typeset -ga _gc_out
compadd () {
  if [[ ${@[1,(i)(-|--)]} == *-(O|A|D)\ * ]]; then builtin compadd "$@"; return $?; fi
  typeset -a __hits __dscr __tmp
  if (( $@[(I)-d] )); then
    __tmp=${@[$[${@[(i)-d]}+1]]}
    if [[ $__tmp == \(* ]]; then eval "__dscr=$__tmp"; else __dscr=( "${(@P)__tmp}" ); fi
  fi
  builtin compadd -A __hits -D __dscr "$@"
  setopt localoptions norcexpandparam extendedglob
  typeset -A apre hpre hsuf asuf
  zparseopts -E P:=apre p:=hpre S:=asuf s:=hsuf
  integer dirsuf=0
  if [[ -z $hsuf && "${${@//-default-/}% -# *}" == *-[[:alnum:]]#f* ]]; then dirsuf=1; fi
  [[ -n $__hits ]] || return
  local dsuf dscr i
  for i in {1..$#__hits}; do
    (( dirsuf )) && [[ -d $__hits[$i] ]] && dsuf=/ || dsuf=
    (( $#__dscr >= $i )) && dscr="${${__dscr[$i]}##$__hits[$i] #}" || dscr=
    _gc_out+=("$IPREFIX$apre$hpre$__hits[$i]$dsuf$hsuf$asuf"$'\t'"${dscr//$'\n'/ }")
  done
}
_gc_write () {
  print -rl -- $_gc_out >| $_gc_dir/answer.tmp && mv -f $_gc_dir/answer.tmp $_gc_dir/answer
}
_gc_main () {
  _gc_out=()
  _main_complete
  compstate[list]=''
  compstate[insert]=''
  _gc_write
}
zle -C _gc_comp complete-word _gc_main
_gc_ask () {
  local req; req=$(<$_gc_dir/request) || return
  local dir=${req%%$'\0'*} line=${req#*$'\0'}
  cd -q -- $dir 2>/dev/null
  BUFFER=$line; CURSOR=$#BUFFER
  zle _gc_comp
  BUFFER=''
}
zle -N _gc_ask
bindkey '^G' _gc_ask
print -n >| $_gc_dir/ready
"#;

/// The completion shell's tmux, run: its output when it ended well, nothing
/// said when it did not. (Its server: none of the dock's own environment.)
fn run_quiet(args: &[&str]) -> Option<String> {
    let mut c = Command::new(tmux_bin());
    c.args(["-L", &format!("{}-complete", socket()), "-f", "/dev/null"]).env_remove("TMUX");
    for (k, _) in std::env::vars_os() {
        let k = k.to_string_lossy();
        if k.starts_with("WAVERUNNER_") || k == "LD_LIBRARY_PATH" || k.starts_with("VK_") || k.starts_with("__EGL") || k.starts_with("__GLX") || k == "RUST_LOG" {
            c.env_remove(k.as_ref());
        }
    }
    let out = c.args(args).stdin(Stdio::null()).stderr(Stdio::null()).output().ok()?;
    out.status.success().then(|| String::from_utf8_lossy(&out.stdout).into_owned())
}

/// The completion shell, as the completion thread holds it.
#[derive(Default)]
struct Zsh {
    /// When it was last started (not again within a minute).
    tried: Option<Instant>,
}

impl Zsh {
    fn dir() -> PathBuf {
        conf_path().with_file_name("term-complete")
    }

    /// Up: there, or started now (its files written, its compinit waited for).
    fn up(&mut self) -> bool {
        if run_quiet(&["has-session", "-t", COMPLETE_SESSION]).is_some() {
            return true;
        }
        if self.tried.is_some_and(|t| t.elapsed() < Duration::from_secs(60)) {
            return false;
        }
        self.tried = Some(Instant::now());
        let dir = Self::dir();
        let zdot = dir.join("zdot");
        if std::fs::create_dir_all(&zdot).is_err() {
            return false;
        }
        let script = dir.join("complete.zsh");
        let _ = std::fs::remove_file(dir.join("ready"));
        let wrote = std::fs::write(&script, COMPLETE_ZSH).and_then(|()| std::fs::write(zdot.join(".zshrc"), format!("source {}\n", escape(&script.to_string_lossy()))));
        if wrote.is_err() {
            return false;
        }
        let d = escape(&dir.to_string_lossy());
        let cmd = format!("exec env GOLEM_COMPLETE_DIR={d} ZDOTDIR={} zsh -d -i", escape(&zdot.to_string_lossy()));
        if run_quiet(&["new-session", "-d", "-s", COMPLETE_SESSION, "-x", "200", "-y", "50", &cmd]).is_none() {
            warn!("term: no completion shell (zsh)");
            return false;
        }
        for _ in 0..100 {
            if dir.join("ready").exists() {
                info!("term: the completion shell is up");
                return true;
            }
            std::thread::sleep(Duration::from_millis(30));
        }
        warn!("term: the completion shell did not get ready");
        false
    }

    /// What zsh offers at the end of `line` typed in `dir`: (word,
    /// description) — `None` when it could not be asked or did not answer.
    fn ask(&mut self, dir: &str, line: &str) -> Option<Vec<(String, String)>> {
        if !self.up() {
            return None;
        }
        let d = Self::dir();
        let answer = d.join("answer");
        let _ = std::fs::remove_file(&answer);
        let tmp = d.join("request.tmp");
        std::fs::write(&tmp, format!("{dir}\0{line}")).and_then(|()| std::fs::rename(&tmp, d.join("request"))).ok()?;
        run_quiet(&["send-keys", "-t", COMPLETE_SESSION, "C-g"])?;
        let asked = Instant::now();
        while asked.elapsed() < ZSH_WAIT {
            if let Ok(text) = std::fs::read_to_string(&answer) {
                return Some(
                    text.lines()
                        .filter_map(|l| {
                            let (w, desc) = l.split_once('\t').unwrap_or((l, ""));
                            (!w.is_empty()).then(|| (w.to_owned(), desc.trim().trim_start_matches("-- ").trim().to_owned()))
                        })
                        .collect(),
                );
            }
            std::thread::sleep(Duration::from_millis(8));
        }
        warn!("term: zsh did not answer in time");
        None
    }
}

/// zsh's words as rows: the word typed replaced by each (a space after it
/// unless it goes on: a folder's /, an option's =), each once.
fn zsh_rows(head: &str, found: &[(String, String)]) -> Vec<Comp> {
    let mut seen = std::collections::HashSet::new();
    found
        .iter()
        .filter(|(w, _)| seen.insert(w.clone()))
        .take(MAX_COMPS * 2)
        .map(|(w, desc)| {
            let mut word = escape(w);
            if let Some(rest) = word.strip_prefix("\\~") {
                word = format!("~{rest}");
            }
            let space = if w.ends_with(['/', '=', ':', ',']) { "" } else { " " };
            Comp { text: format!("{head}{word}{space}"), kind: CompKind::Arg, desc: desc.clone() }
        })
        .collect()
}

/// The word being typed at the end of `cmd`, and what stands before it.
fn split_word(cmd: &str) -> (&str, &str) {
    let start = cmd.char_indices().rev().find(|(_, c)| c.is_whitespace()).map_or(0, |(i, c)| i + c.len_utf8());
    cmd.split_at(start)
}

/// Whether the word after `head` is a command's name (the first, or after
/// `sudo`…).
fn first_word(head: &str) -> bool {
    head.split_whitespace().last().is_none_or(|w| RUNNERS.contains(&w))
}

/// THE COMPLETION THREAD: at once the programs and the folder's names, then
/// — an argument — what zsh's own completion offers, with its words. Only
/// the newest ask is answered.
fn completer(rx: mpsc::Receiver<Req>, to_loop: calloop::channel::Sender<Ev>) {
    let mut progs: Option<(Instant, Vec<String>)> = None;
    let mut zsh = Zsh::default();
    // (Started ahead: its first compinit is the slow one.)
    zsh.up();
    while let Ok(mut req) = rx.recv() {
        while let Ok(newer) = rx.try_recv() {
            req = newer;
        }
        let Req::Complete { cmd, dir, all } = req else {
            continue;
        };
        if progs.as_ref().is_none_or(|(t, _)| t.elapsed() > PROGRAMS_FOR) {
            progs = Some((Instant::now(), programs()));
        }
        let rows = complete(&cmd, &dir, progs.as_ref().map_or(&[][..], |(_, p)| &p[..]), all);
        let (head, word) = split_word(&cmd);
        let ask_zsh = !first_word(head) && (!word.is_empty() || all);
        if !ask_zsh {
            let _ = to_loop.send(Ev::Completions { cmd, rows });
            continue;
        }
        let _ = to_loop.send(Ev::Completions { cmd: cmd.clone(), rows: rows.clone() });
        if let Some(found) = zsh.ask(&dir, &cmd) {
            let z = zsh_rows(head, &found);
            if !z.is_empty() {
                let _ = to_loop.send(Ev::Completions { cmd, rows: z });
            }
        }
    }
}

// ------------------------------------------------------------ the engine

fn tmux_bin() -> String {
    std::env::var("WAVERUNNER_TMUX").unwrap_or_else(|_| "tmux".into())
}

fn tmux() -> Command {
    let mut c = Command::new(tmux_bin());
    c.args(["-L", socket()]);
    c.env_remove("TMUX");
    c
}

/// Run tmux with `args`; its output when it ended well.
fn run(args: &[&str]) -> Option<String> {
    let out = tmux().args(args).stdin(Stdio::null()).stderr(Stdio::piped()).output().ok()?;
    if out.status.success() {
        Some(String::from_utf8_lossy(&out.stdout).into_owned())
    } else {
        let e = String::from_utf8_lossy(&out.stderr);
        if !e.trim().is_empty() {
            warn!("term: tmux {}: {}", args.first().unwrap_or(&""), e.trim());
        }
        None
    }
}

fn conf_path() -> PathBuf {
    let dir = std::env::var_os("XDG_RUNTIME_DIR").map_or_else(std::env::temp_dir, PathBuf::from).join("waverunner");
    dir.join("term.conf")
}

/// THE TABS' SHELL HOOK (zsh's, the way kitty's and ghostty's shell
/// integration go in): a tab's zsh starts with ZDOTDIR here; this .zshenv
/// gives the user's own ZDOTDIR back and reads their .zshenv (their .zshrc
/// and the rest follow from it as ever), and — interactive, in a tab — after
/// each command writes its status and times where the dock reads them
/// (`term-status/<pane>`: "status start end line").
const HOOK_ZSHENV: &str = r#"if [[ -n ${GOLEM_USER_ZDOTDIR+x} ]]; then ZDOTDIR=$GOLEM_USER_ZDOTDIR; else unset ZDOTDIR; fi
unset GOLEM_USER_ZDOTDIR
[[ -f ${ZDOTDIR:-$HOME}/.zshenv ]] && source ${ZDOTDIR:-$HOME}/.zshenv
if [[ -o interactive && -n $TMUX_PANE && -n $GOLEM_TERM_STATUS ]]; then
  zmodload zsh/datetime 2>/dev/null
  typeset -g _golem_st=$GOLEM_TERM_STATUS/${TMUX_PANE#%} _golem_t= _golem_c=
  _golem_preexec() { _golem_t=$EPOCHSECONDS; _golem_c=${1%%$'\n'*} }
  _golem_precmd() { local s=$?; [[ -n $_golem_t ]] && print -r -- "$s $_golem_t $EPOCHSECONDS $_golem_c" >| $_golem_st 2>/dev/null; _golem_t= }
  autoload -Uz add-zsh-hook
  add-zsh-hook preexec _golem_preexec
  add-zsh-hook precmd _golem_precmd
fi
"#;

/// Where the tabs' shells write each command's status.
fn status_dir() -> PathBuf {
    conf_path().with_file_name("term-status")
}

/// What a tab's shell is started with: the hook (a zsh only; any other
/// shell has nothing to read it).
fn shell_env() -> Vec<String> {
    if !std::env::var("SHELL").is_ok_and(|s| s.ends_with("zsh")) {
        return Vec::new();
    }
    let hook = conf_path().with_file_name("term-zsh");
    let file = hook.join(".zshenv");
    let fresh = std::fs::read_to_string(&file).is_ok_and(|t| t == HOOK_ZSHENV);
    if !fresh {
        let tmp = hook.join(".zshenv.tmp");
        let wrote = std::fs::create_dir_all(&hook).and_then(|()| std::fs::write(&tmp, HOOK_ZSHENV)).and_then(|()| std::fs::rename(&tmp, &file));
        if wrote.is_err() {
            return Vec::new();
        }
    }
    let _ = std::fs::create_dir_all(status_dir());
    let mut env = vec!["-e".to_owned(), format!("ZDOTDIR={}", hook.display()), "-e".to_owned(), format!("GOLEM_TERM_STATUS={}", status_dir().display())];
    if let Ok(z) = std::env::var("ZDOTDIR") {
        env.extend(["-e".to_owned(), format!("GOLEM_USER_ZDOTDIR={z}")]);
    }
    env
}

/// What a shell hook's status file says: the status, the seconds the
/// command ran, its line.
pub(crate) fn parse_status(text: &str) -> Option<(i32, u64, String)> {
    let mut f = text.trim_end_matches('\n').splitn(4, ' ');
    let status = f.next()?.parse().ok()?;
    let start: i64 = f.next()?.parse().ok()?;
    let end: i64 = f.next()?.parse().ok()?;
    Some((status, (end - start).max(0) as u64, f.next().unwrap_or("").trim().to_owned()))
}

/// Where the tabs' folders are kept (`n<TAB>folder` a line): the server
/// gone (a restart), the tabs come back in them.
fn tabs_file() -> PathBuf {
    let home = std::env::var_os("HOME").map(PathBuf::from).unwrap_or_default();
    let state = std::env::var_os("XDG_STATE_HOME").map_or_else(|| home.join(".local/state"), PathBuf::from);
    state.join("waverunner").join(if socket() == "golem" { "term-tabs" } else { "term-tabs-rig" })
}

/// Where the folders gone to are kept (count, when, folder a line).
fn dirs_file() -> PathBuf {
    tabs_file().with_file_name(if socket() == "golem" { "term-dirs" } else { "term-dirs-rig" })
}

/// The tabs as they are, kept (only when that changed).
fn save_tabs(tabs: &[Tab], saved: &mut String) {
    let text: String = tabs.iter().map(|t| format!("{}\t{}\n", t.n, t.path)).collect();
    if text.is_empty() || text == *saved {
        return;
    }
    let file = tabs_file();
    if let Some(dir) = file.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    let tmp = file.with_extension("tmp");
    if std::fs::write(&tmp, &text).and_then(|()| std::fs::rename(&tmp, &file)).is_ok() {
        *saved = text;
    }
}

/// The server up: there already, or started (its tabs back in the folders
/// they were in when it ended).
fn ensure_server(cols: u16, rows: u16) -> Result<(), String> {
    if run(&["has-session", "-t", SESSION]).is_some() {
        return Ok(());
    }
    start_server(cols, rows)?;
    let kept = std::fs::read_to_string(tabs_file()).unwrap_or_default();
    for line in kept.lines() {
        let Some((n, dir)) = line.split_once('\t') else {
            continue;
        };
        if !Path::new(dir).is_dir() {
            continue;
        }
        let target = format!("{SESSION}:{n}");
        if n == "1" {
            // (The first is there already, at home: it goes there.)
            let keys = format!("cd {}", escape(dir));
            run(&["send-keys", "-t", &target, "-l", &format!(" {keys}")]);
            run(&["send-keys", "-t", &target, "Enter", "C-l"]);
        } else {
            let mut args = vec!["new-window", "-d", "-t", &target, "-c", dir];
            let env = shell_env();
            args.extend(env.iter().map(String::as_str));
            run(&args);
        }
    }
    if !kept.is_empty() {
        info!("term: the tabs back in their folders");
    }
    Ok(())
}

/// The server started, as a service of its own when it can be.
fn start_server(cols: u16, rows: u16) -> Result<(), String> {
    let conf = conf_path();
    if let Some(dir) = conf.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    let tmp = conf.with_extension("tmp");
    let text = format!("{CONF}set -g default-size {cols}x{rows}\n");
    std::fs::write(&tmp, text).and_then(|()| std::fs::rename(&tmp, &conf)).map_err(|e| format!("cannot write {}: {e}", conf.display()))?;
    let home = std::env::var("HOME").unwrap_or_else(|_| "/".into());
    let conf_s = conf.to_string_lossy().into_owned();
    let bin = tmux_bin();
    let env = shell_env();
    let mut server: Vec<&str> = vec![bin.as_str(), "-L", socket(), "-f", &conf_s, "new-session", "-d", "-s", SESSION, "-c", &home];
    server.extend(env.iter().map(String::as_str));
    // A user service: its shells get the session's environment (not the
    // dock's) and outlive the dock. (Never from a test rig.)
    let rig = std::env::var_os("GOLEM_RIG").is_some();
    let unit = if rig { Command::new("false").status() } else { Command::new("systemd-run")
        .args(["--user", "--quiet", "--collect", "--unit", UNIT, "-p", "Type=forking", "--description", "Golem: the box's terminal tabs", "--"])
        .args(&server)
        .stdin(Stdio::null())
        .status() };
    if unit.is_ok_and(|s| s.success()) && run(&["has-session", "-t", SESSION]).is_some() {
        info!("term: the server is up ({UNIT})");
        return Ok(());
    }
    // (No user manager to ask: started here, without what is the dock's own.)
    warn!("term: no user service, starting tmux directly");
    let mut c = tmux();
    c.args(["-f", &conf_s, "new-session", "-d", "-s", SESSION, "-c", &home]).args(&env);
    for (k, _) in std::env::vars_os() {
        let k = k.to_string_lossy();
        if k.starts_with("WAVERUNNER_") || k == "LD_LIBRARY_PATH" || k.starts_with("VK_") || k.starts_with("__EGL") || k.starts_with("__GLX") || k == "RUST_LOG" {
            c.env_remove(k.as_ref());
        }
    }
    match c.stdin(Stdio::null()).status() {
        Ok(s) if s.success() => Ok(()),
        Ok(s) => Err(format!("tmux exited {s}")),
        Err(e) => Err(format!("cannot run tmux ({}): {e}", tmux_bin())),
    }
}

/// The tabs as tmux has them.
fn read_tabs() -> Option<Vec<Tab>> {
    let out = run(&[
        "list-windows",
        "-t",
        SESSION,
        "-F",
        "#{window_index}\t#{pane_id}\t#{pane_current_command}\t#{pane_current_path}\t#{?automatic-rename,,1}\t#{window_name}",
    ])?;
    Some(
        out.lines()
            .filter_map(|l| {
                let mut f = l.split('\t');
                let n = f.next()?.parse().ok()?;
                let pane = f.next()?.to_owned();
                let command = f.next().unwrap_or("").to_owned();
                let path = f.next().unwrap_or("").to_owned();
                let named = f.next() == Some("1");
                let title = f.next().filter(|_| named).map(str::to_owned);
                Some(Tab { n, pane, command, path, title })
            })
            .collect(),
    )
}

/// What tab `tab` shows now (its pane `pane`).
fn read_screen(tab: u32, pane: &str, scroll: u32, rows: u16) -> Option<Screen> {
    let info = "#{cursor_x}\t#{cursor_y}\t#{alternate_on}\t#{pane_current_command}\t#{history_size}";
    // (Back in its history: from `scroll` lines over the screen's top.)
    let (from, to) = (format!("-{scroll}"), (i64::from(rows) - 1 - i64::from(scroll)).to_string());
    let mut args = vec!["capture-pane", "-p", "-e", "-N", "-t", pane];
    if scroll > 0 {
        args.extend(["-S", &from, "-E", &to]);
    }
    args.extend([";", "display-message", "-p", "-t", pane, info]);
    let out = run(&args)?;
    let mut lines: Vec<&str> = out.lines().collect();
    let last = lines.pop()?;
    let mut f = last.split('\t');
    let x = f.next()?.parse().ok()?;
    let y: u32 = f.next()?.parse().ok()?;
    let alternate = f.next() == Some("1");
    let command = f.next().unwrap_or("").to_owned();
    let history = f.next().and_then(|h| h.parse().ok()).unwrap_or(0);
    // (Read back, the cursor is that many lines lower: off the bottom, gone.)
    let y = y + scroll;
    Some(Screen {
        tab,
        lines: lines.iter().map(|l| parse_line(l)).collect(),
        cursor: (y < u32::from(rows)).then_some((x, y as u16)),
        alternate,
        command,
        scroll,
        history,
    })
}

/// Keys for a tab, made first when it is not there.
fn send_keys(tab: u32, keys: &[Key], dir: Option<&str>) {
    let target = format!("{SESSION}:{tab}");
    if run(&["list-panes", "-t", &target]).is_none() {
        let home = std::env::var("HOME").unwrap_or_else(|_| "/".into());
        let dir = dir.filter(|d| Path::new(d).is_dir()).unwrap_or(&home);
        let env = shell_env();
        let mut args = vec!["new-window", "-d", "-t", &target, "-c", dir];
        args.extend(env.iter().map(String::as_str));
        if run(&args).is_none() {
            return;
        }
        info!("term: tab {tab} made");
        // (Its shell first draws its prompt: keys sent before it were echoed
        // by the terminal over the top line, then read again by the shell.)
        for _ in 0..40 {
            if run(&["capture-pane", "-p", "-t", &target]).is_some_and(|t| !t.trim().is_empty()) {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(50));
        }
    }
    if keys.is_empty() {
        return;
    }
    let mut args: Vec<String> = Vec::new();
    for k in keys {
        if !args.is_empty() {
            args.push(";".into());
        }
        args.extend(["send-keys".into(), "-t".into(), target.clone()]);
        match k {
            Key::Text(t) => {
                args.push("-H".into());
                args.extend(t.bytes().map(|b| format!("{b:02x}")));
            }
            Key::Name(n) => args.push(n.clone()),
        }
    }
    let refs: Vec<&str> = args.iter().map(String::as_str).collect();
    run(&refs);
}

fn resize_all(cols: u16, rows: u16) {
    let size = format!("{cols}x{rows}");
    run(&["set", "-g", "default-size", &size]);
    for t in read_tabs().unwrap_or_default() {
        run(&["resize-window", "-t", &format!("{SESSION}:{}", t.n), "-x", &cols.to_string(), "-y", &rows.to_string()]);
    }
}

/// The control-mode client: only what tmux tells (a pane printed, the
/// windows changed, the server ended); its replies are skipped.
fn watch(to_loop: calloop::channel::Sender<Ev>) {
    let child = tmux().args(["-C", "attach", "-t", SESSION]).stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::null()).spawn();
    let mut child = match child {
        Ok(c) => c,
        Err(e) => {
            let _ = to_loop.send(Ev::Gone(format!("no control client: {e}")));
            return;
        }
    };
    // (Its stdin is held open for as long as it runs: control mode ends on
    // its end of file.)
    let _stdin = child.stdin.take();
    let Some(out) = child.stdout.take() else {
        return;
    };
    let mut in_block = false;
    for line in BufReader::new(out).lines() {
        let Ok(line) = line else { break };
        if line.starts_with("%begin") {
            in_block = true;
            continue;
        }
        if line.starts_with("%end") || line.starts_with("%error") {
            in_block = false;
            continue;
        }
        if in_block {
            continue;
        }
        let ev = if let Some(rest) = line.strip_prefix("%output ") {
            rest.split(' ').next().map(|p| Ev::Printed(p.to_owned()))
        } else if line.starts_with("%window-add")
            || line.starts_with("%window-close")
            || line.starts_with("%unlinked-window")
            || line.starts_with("%window-renamed")
            || line.starts_with("%session-window-changed")
        {
            Some(Ev::Changed)
        } else if line.starts_with("%exit") {
            break;
        } else {
            None
        };
        if let Some(ev) = ev {
            if to_loop.send(ev).is_err() {
                break;
            }
        }
    }
    let _ = child.kill();
    let _ = child.wait();
    let _ = to_loop.send(Ev::Gone("the tabs ended".into()));
}

/// The engine's thread: one request at a time, in order.
fn engine(rx: mpsc::Receiver<Req>, to_loop: calloop::channel::Sender<Ev>) {
    let mut watching = false;
    let mut saved = String::new();
    let mut hist_read: Option<(PathBuf, std::time::SystemTime, u64)> = None;
    while let Ok(req) = rx.recv() {
        match req {
            Req::Start { cols, rows } => {
                if let Err(e) = ensure_server(cols, rows) {
                    let _ = to_loop.send(Ev::Failed);
                    let _ = to_loop.send(Ev::Gone(e));
                    // (Nothing more can be done without a server.)
                    break;
                }
                if !watching {
                    watching = true;
                    let tx = to_loop.clone();
                    let _ = std::thread::Builder::new().name("term-watch".into()).spawn(move || watch(tx));
                }
                if let Some(tabs) = read_tabs() {
                    let _ = to_loop.send(Ev::Tabs(tabs));
                }
            }
            Req::Tabs => {
                if let Some(tabs) = read_tabs() {
                    save_tabs(&tabs, &mut saved);
                    let _ = to_loop.send(Ev::Tabs(tabs));
                }
            }
            Req::Capture { tab, pane, scroll, rows } => {
                let screen = read_screen(tab, &pane, scroll, rows).unwrap_or(Screen { tab, scroll, ..Default::default() });
                let _ = to_loop.send(Ev::Screen(screen));
            }
            Req::Keys { tab, keys, dir } => {
                send_keys(tab, &keys, dir.as_deref());
                if let Some(tabs) = read_tabs() {
                    let _ = to_loop.send(Ev::Tabs(tabs));
                }
            }
            Req::Resize { cols, rows } => resize_all(cols, rows),
            // (The completion thread's: never here.)
            Req::Complete { .. } => {}
            Req::Close { tab } => {
                run(&["kill-window", "-t", &format!("{SESSION}:{tab}")]);
            }
            Req::Rename { tab, name } => {
                let target = format!("{SESSION}:{tab}");
                if name.trim().is_empty() {
                    run(&["set-window-option", "-t", &target, "automatic-rename", "on"]);
                } else {
                    run(&["rename-window", "-t", &target, "--", name.trim()]);
                }
                if let Some(tabs) = read_tabs() {
                    let _ = to_loop.send(Ev::Tabs(tabs));
                }
            }
            Req::Find { pane, needle } => {
                let text = run(&["capture-pane", "-p", "-S", "-", "-E", "-", "-t", &pane]).unwrap_or_default();
                let lines: Vec<&str> = text.lines().collect();
                let hits = find_in(&lines, &needle);
                let _ = to_loop.send(Ev::Found { needle, hits });
            }
            Req::CopyOutput { pane, from } => {
                let now = run(&["display-message", "-p", "-t", &pane, "#{history_size}\t#{cursor_y}"]).unwrap_or_default();
                let mut f = now.trim().split('\t');
                let (hs, cy): (i64, i64) = (f.next().and_then(|v| v.parse().ok()).unwrap_or(0), f.next().and_then(|v| v.parse().ok()).unwrap_or(0));
                // (From the line after the command's to the one before the
                // prompt that came back, as lines of the screen: ≤ 0 is the
                // history's.)
                let (start, end) = (from as i64 + 1 - hs, cy - 1);
                let text = if end >= start {
                    run(&["capture-pane", "-p", "-J", "-S", &start.to_string(), "-E", &end.to_string(), "-t", &pane]).unwrap_or_default()
                } else {
                    String::new()
                };
                let _ = to_loop.send(Ev::Copied(text.trim_end().to_owned()));
            }
            Req::Paste { tab, text } => {
                let target = format!("{SESSION}:{tab}");
                if run(&["set-buffer", "-b", "golem-paste", "--", &text]).is_some() {
                    run(&["paste-buffer", "-p", "-d", "-b", "golem-paste", "-t", &target]);
                }
            }
            Req::History => {
                let Some(file) = history_file() else {
                    continue;
                };
                let Ok(meta) = std::fs::metadata(&file) else {
                    continue;
                };
                let stamp = (file.clone(), meta.modified().unwrap_or(std::time::UNIX_EPOCH), meta.len());
                if hist_read.as_ref() == Some(&stamp) {
                    continue;
                }
                if let Ok(bytes) = std::fs::read(&file) {
                    hist_read = Some(stamp);
                    let _ = to_loop.send(Ev::History(parse_history(&bytes)));
                }
            }
        }
    }
}

impl App {
    /// Whether the box is the terminal: what is typed begins with `./`.
    pub(crate) fn term_on(&self) -> bool {
        self.search.query.starts_with("./")
    }

    /// The tab the box shows: the one `./N` names, else the last one shown.
    pub(crate) fn term_shown(&self) -> u32 {
        parse(&self.search.query).and_then(|a| a.tab).unwrap_or(self.term.current.max(1))
    }

    fn term_tab(&self, n: u32) -> Option<&Tab> {
        self.term.tabs.iter().find(|t| t.n == n)
    }

    /// Ask the engine; its thread made on the first use.
    fn term_ask(&mut self, req: Req) {
        if self.term.to_engine.is_none() {
            let (to_loop, from_engine) = calloop::channel::channel::<Ev>();
            self.term.to_loop = Some(to_loop.clone());
            let inserted = self.loop_handle.insert_source(from_engine, |ev, _, app: &mut App| {
                if let Event::Msg(ev) = ev {
                    app.term_event(ev);
                }
            });
            if inserted.is_err() {
                warn!("term: no channel on the loop");
                return;
            }
            let (tx, rx) = mpsc::channel();
            if let Err(e) = std::thread::Builder::new().name("term".into()).spawn(move || engine(rx, to_loop)) {
                warn!("term: no thread: {e}");
                return;
            }
            self.term.to_engine = Some(tx);
            self.term.started_at = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_secs() as i64);
            // The completion thread: its own, so a slow answer never holds
            // the screen.
            let (ctx, crx) = mpsc::channel();
            let to_loop2 = self.term.to_loop.clone();
            if let Some(to_loop2) = to_loop2 {
                if std::thread::Builder::new().name("term-complete".into()).spawn(move || completer(crx, to_loop2)).is_ok() {
                    self.term.to_completer = Some(ctx);
                }
            }
            let (cols, rows) = self.term.size;
            if let Some(tx) = &self.term.to_engine {
                let _ = tx.send(Req::Start { cols: cols.max(20), rows: rows.max(5) });
            }
        }
        if matches!(req, Req::Complete { .. }) {
            if let Some(tx) = &self.term.to_completer {
                if tx.send(req).is_err() {
                    self.term.to_completer = None;
                }
            }
            return;
        }
        if let Some(tx) = &self.term.to_engine {
            if tx.send(req).is_err() {
                self.term.to_engine = None;
            }
        }
    }

    /// What is typed changed while the box is the terminal: the engine up,
    /// the tab named shown.
    pub(crate) fn term_sync(&mut self) {
        if self.term.current == 0 {
            self.term.current = 1;
        }
        let shown = self.term_shown();
        if self.term.screen.as_ref().is_some_and(|s| s.tab != shown) {
            self.term.screen = None;
            self.term.keys_to_field = false;
        }
        if shown != self.term.was_shown {
            self.term.last = self.term.was_shown;
            self.term.was_shown = shown;
        }
        self.term.help = false;
        self.term.find = None;
        // (What is typed changed: the view back to the screen as it is.)
        if self.term.scroll > 0 {
            self.term.scroll = 0;
            self.term.sel = None;
        }
        // (Typing anything but what the walk put there ends the walk.)
        if self.term.hist_query.as_deref().is_some_and(|q| q != self.search.query) {
            self.term.hist_at = None;
            self.term.hist_query = None;
        }
        if self.term.to_engine.is_none() {
            // (The size first: a tab made at another size wraps its first
            // lines there.)
            if self.term.size == (0, 0) {
                let layout = self.layout_at_rest(self.ui.extent_of(crate::state::Target::Open));
                self.term_fit(&layout);
            }
            self.term_ask(Req::Tabs);
        } else {
            self.term_capture();
        }
        // (The file read again only when it changed: commands typed in
        // other terminals come in too.)
        self.term_ask(Req::History);
        self.term_complete();
    }

    /// The command typed after `./N`, and what stands before it.
    fn term_typed(&self) -> (String, String) {
        let q = &self.search.query;
        let cmd = parse(q).map(|a| a.cmd).unwrap_or_default();
        (q[..q.len() - cmd.len()].to_owned(), cmd)
    }

    /// The completions for what is typed: the engine asked, the rows made
    /// of what is known now.
    fn term_complete(&mut self) {
        let (_, cmd) = self.term_typed();
        self.term.comp_sel = 0;
        self.term.comp_moved = false;
        let wanted = !cmd.is_empty() && !self.term_raw() && !self.term_secret() && self.term.hist_at.is_none();
        if wanted && cmd != self.term.engine_for {
            let all = self.term.comps_all.as_deref() == Some(self.search.query.as_str());
            let dir = self.term_tab(self.term_shown()).or_else(|| self.term_tab(self.term.current)).map(|t| t.path.clone());
            let dir = dir.unwrap_or_else(|| std::env::var("HOME").unwrap_or_default());
            self.term_ask(Req::Complete { cmd: cmd.clone(), dir, all });
        }
        self.term_rows();
    }

    /// The rows shown: the history's that begin with what is typed (newest
    /// first), the engine's that still fit it; the pick kept when it stays.
    fn term_rows(&mut self) {
        let (_, cmd) = self.term_typed();
        let hidden = self.term.comps_hidden.as_deref() == Some(self.search.query.as_str());
        let mut rows: Vec<Comp> = Vec::new();
        let quiet = self.term_raw() || self.term_secret();
        if self.term.help {
            rows = help_rows();
            self.term.ghost.clear();
            if rows != self.term.comps {
                self.term.comps = rows;
                self.schedule_frame();
            }
            return;
        }
        // The suggestion: the newest command typed before that goes on from
        // what is typed (the caret at its end).
        self.term.ghost = if cmd.is_empty() || quiet || self.term.searching || self.term.hist_at.is_some() || self.term.caret_back > 0 {
            String::new()
        } else {
            self.term.hist.iter().chain(&self.term.sent).rev().find(|(_, h)| h.len() > cmd.len() && h.starts_with(&cmd)).map(|(_, h)| h[cmd.len()..].to_owned()).unwrap_or_default()
        };
        if self.term.searching && !quiet {
            // Ctrl+R: every word typed in the command, anywhere, in order
            // or loosely; the best first, then the newest.
            let mut seen = std::collections::HashSet::new();
            let mut found: Vec<(i32, usize, Comp)> = self
                .term
                .hist
                .iter()
                .chain(&self.term.sent)
                .rev()
                .filter(|(_, h)| seen.insert(h.clone()))
                .enumerate()
                .filter_map(|(age, (when, h))| fuzzy(&cmd, h).map(|score| (score, age, Comp::new(h.clone(), CompKind::History(*when)))))
                .take(4000)
                .collect();
            found.sort_by(|a, b| b.0.cmp(&a.0).then(a.1.cmp(&b.1)));
            rows.extend(found.into_iter().take(MAX_SEARCH).map(|(_, _, c)| c));
        } else if !cmd.is_empty() && !hidden && !quiet && self.term.hist_at.is_none() {
            let mut seen = std::collections::HashSet::new();
            let hist: Vec<Comp> = self
                .term
                .hist
                .iter()
                .chain(&self.term.sent)
                .rev()
                .filter(|(_, h)| h.starts_with(&cmd) && *h != cmd)
                .filter(|(_, h)| seen.insert(h.clone()))
                .take(MAX_COMPS)
                .map(|(when, h)| Comp::new(h.clone(), CompKind::History(*when)))
                .collect();
            let exact = self.term.engine_for == cmd;
            let others: Vec<Comp> = self
                .term
                .engine_comps
                .iter()
                .filter(|c| exact || (c.text.starts_with(&cmd) && c.text != cmd))
                .filter(|c| !seen.contains(&c.text))
                .cloned()
                .collect();
            let from_hist = hist.len().min(MAX_COMPS.saturating_sub(others.len()).max(3));
            rows.extend(hist.into_iter().take(from_hist));
            rows.extend(others);
            rows.truncate(MAX_COMPS);
            // The words the screen shows that go on from the word typed: a
            // hash a log printed, a number, a name (iTerm2's).
            let (head, word) = split_word(&cmd);
            if word.chars().count() >= 2 && !first_word(head) && rows.len() < MAX_COMPS {
                let shown: std::collections::HashSet<String> = rows.iter().map(|c| c.text.clone()).collect();
                let mut seen = std::collections::HashSet::new();
                let words: Vec<Comp> = self
                    .term
                    .screen
                    .iter()
                    .flat_map(|s| s.lines.iter().rev())
                    .flat_map(|l| {
                        let text: String = line_cells(l).into_iter().collect();
                        text.split(|c: char| c.is_whitespace() || "'\"()[]{}<>,;|=`".contains(c)).map(str::to_owned).collect::<Vec<_>>()
                    })
                    .filter(|w| w.len() > word.len() && w.len() >= 3 && w.starts_with(word))
                    .filter(|w| seen.insert(w.clone()))
                    .map(|w| Comp::new(format!("{head}{} ", escape(&w)), CompKind::Screen))
                    .filter(|c| !shown.contains(&c.text))
                    .take(MAX_COMPS - rows.len())
                    .collect();
                rows.extend(words.into_iter().take(2));
            }
            // `cd x`: the folders often gone to that x finds, after the
            // folder's own (zoxide's jump, in the rows).
            if matches!(head.trim(), "cd" | "pushd") && !word.is_empty() {
                let room = MAX_COMPS.saturating_sub(rows.len()).max(2);
                let shown: Vec<String> = rows.iter().map(|c| c.text.clone()).collect();
                let freq: Vec<Comp> = self
                    .term_frequent(word)
                    .into_iter()
                    .map(|p| Comp::new(format!("{head}{}/", escape(&p).replacen("\\~", "~", 1)), CompKind::Frequent))
                    .filter(|c| !shown.contains(&c.text))
                    .take(room)
                    .collect();
                rows.truncate(MAX_COMPS - freq.len().min(MAX_COMPS));
                rows.extend(freq);
            }
        }
        // (The engine's rows coming in keep the one picked where it went.)
        let picked = self.term.comps.get(self.term.comp_sel).map(|c| c.text.clone());
        match picked.and_then(|t| rows.iter().position(|c| c.text == t)) {
            Some(i) => self.term.comp_sel = i,
            None => {
                self.term.comp_sel = 0;
                self.term.comp_moved = false;
            }
        }
        if rows != self.term.comps {
            self.term.comps = rows;
            self.schedule_frame();
        }
    }

    /// Read the shown tab's screen again (one read at a time; one wanted
    /// meanwhile follows it, no sooner than `CAPTURE_EVERY`).
    fn term_capture(&mut self) {
        if !self.term_on() {
            return;
        }
        let shown = self.term_shown();
        let Some(pane) = self.term_tab(shown).map(|t| t.pane.clone()) else {
            return;
        };
        if self.term.capturing {
            self.term.again = true;
            return;
        }
        let soon = self.term.asked_at.map_or(Duration::ZERO, |t| CAPTURE_EVERY.saturating_sub(t.elapsed()));
        if !soon.is_zero() {
            if !self.term.capture_timer {
                self.term.capture_timer = true;
                let _ = self.loop_handle.insert_source(calloop::timer::Timer::from_duration(soon), |_, _, app: &mut App| {
                    app.term.capture_timer = false;
                    app.term_capture();
                    calloop::timer::TimeoutAction::Drop
                });
            }
            return;
        }
        self.term.capturing = true;
        self.term.asked_at = Some(Instant::now());
        let (scroll, rows) = (self.term.scroll, self.term.size.1.max(3));
        self.term_ask(Req::Capture { tab: shown, pane, scroll, rows });
    }

    fn term_event(&mut self, ev: Ev) {
        match ev {
            Ev::Tabs(tabs) => {
                self.term_ran(&tabs);
                self.term_went(&tabs);
                self.term.tabs = tabs;
                self.term_capture();
                self.schedule_frame();
            }
            Ev::Screen(screen) => {
                self.term.capturing = false;
                if screen.tab == self.term_shown() {
                    if !screen.alternate {
                        self.term.keys_to_field = false;
                    }
                    // Read back, the view stays on what it shows as the tab
                    // goes on printing (its lines move up into the history).
                    if self.term.scroll > 0 && screen.scroll == self.term.scroll {
                        let before = self.term.screen.as_ref().filter(|s| s.tab == screen.tab).map_or(screen.history, |s| s.history);
                        if screen.history > before {
                            self.term.scroll += screen.history - before;
                            self.term.again = true;
                        }
                    }
                    self.term.scroll = self.term.scroll.min(screen.history);
                    if screen.alternate {
                        self.term.scroll = 0;
                    }
                    self.term.screen = Some(screen);
                    self.schedule_frame();
                }
                if std::mem::take(&mut self.term.again) {
                    self.term_capture();
                }
            }
            Ev::Printed(pane) => {
                let shown = self.term_on() && self.term_tab(self.term_shown()).is_some_and(|t| t.pane == pane);
                if shown {
                    self.term_capture();
                } else if let Some(n) = self.term.tabs.iter().find(|t| t.pane == pane).map(|t| t.n) {
                    // Out of sight: its word gets a dot.
                    if self.term.busy.insert(n) && self.term_on() {
                        self.schedule_frame();
                    }
                }
                // (What runs in the tabs, read again now and then as they
                // print: a command started or ended.)
                // (At most each second; one owed meanwhile comes at its end:
                // the prompt coming back is the last output.)
                match self.term.tabs_read.map(|t| t.elapsed()) {
                    Some(e) if e < Duration::from_secs(1) => {
                        if !self.term.tabs_owed {
                            self.term.tabs_owed = true;
                            let _ = self.loop_handle.insert_source(calloop::timer::Timer::from_duration(Duration::from_secs(1) - e), |_, _, app: &mut App| {
                                app.term.tabs_owed = false;
                                app.term.tabs_read = Some(Instant::now());
                                app.term_ask(Req::Tabs);
                                calloop::timer::TimeoutAction::Drop
                            });
                        }
                    }
                    _ => {
                        self.term.tabs_read = Some(Instant::now());
                        self.term_ask(Req::Tabs);
                    }
                }
            }
            Ev::Changed => self.term_ask(Req::Tabs),
            Ev::Failed => self.term.failed = true,
            Ev::Found { needle, hits } => {
                if let Some(f) = self.term.find.as_mut().filter(|f| f.needle == needle) {
                    // (The newest — the lowest — first.)
                    f.at = hits.len().saturating_sub(1);
                    f.hits = hits;
                    self.term_find_show();
                }
            }
            Ev::Copied(text) => {
                if text.trim().is_empty() {
                    self.term_flash(crate::i18n::tr("Nothing printed"));
                } else {
                    self.copy_text(&text);
                    self.term_flash(crate::i18n::tr("Copied"));
                }
            }
            Ev::Completions { cmd, rows } => {
                self.term.engine_for = cmd;
                self.term.engine_comps = rows;
                self.term_rows();
            }
            Ev::History(hist) => {
                self.term.hist = hist;
                // (What the box sent is in the file now, when the shell
                // writes it as it goes: kept only when it is not.)
                let tail: std::collections::HashSet<&String> = self.term.hist.iter().rev().take(200).map(|(_, h)| h).collect();
                let sent: Vec<(i64, String)> = self.term.sent.iter().filter(|(_, h)| !tail.contains(h)).cloned().collect();
                self.term.sent = sent;
                self.term_rows();
            }
            Ev::Gone(why) => {
                info!("term: {why}");
                self.term.to_engine = None;
                self.term.to_completer = None;
                self.term.to_loop = None;
                self.term.tabs.clear();
                self.term.capturing = false;
                // (Not the tabs ending: what went wrong, on the screen.)
                self.term.screen = self.term.failed.then(|| Screen {
                    tab: self.term_shown(),
                    lines: vec![parse_line(&crate::i18n::fill(crate::i18n::tr("The terminal could not start: {why}"), &[("why", &why)]))],
                    ..Default::default()
                });
                self.term.failed = false;
                self.schedule_frame();
            }
        }
    }

    /// The folders gone to, as kept (read the first time).
    fn term_dirs(&mut self) -> &mut Vec<(String, f32, i64)> {
        self.term.dirs.get_or_insert_with(|| {
            std::fs::read_to_string(dirs_file())
                .unwrap_or_default()
                .lines()
                .filter_map(|l| {
                    let mut f = l.splitn(3, '\t');
                    let count = f.next()?.parse().ok()?;
                    let when = f.next()?.parse().ok()?;
                    Some((f.next()?.to_owned(), count, when))
                })
                .collect()
        })
    }

    /// The tabs' folders now: one gone to counts.
    fn term_went(&mut self, tabs: &[Tab]) {
        let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_secs() as i64);
        let mut changed = false;
        for t in tabs {
            if t.path.is_empty() || self.term.tab_dirs.get(&t.n) == Some(&t.path) {
                continue;
            }
            let first = !self.term.tab_dirs.contains_key(&t.n);
            self.term.tab_dirs.insert(t.n, t.path.clone());
            // (A tab first seen is not a visit: it was there.)
            if first {
                continue;
            }
            let path = t.path.clone();
            let dirs = self.term_dirs();
            match dirs.iter_mut().find(|(p, _, _)| *p == path) {
                Some(d) => {
                    d.1 += 1.0;
                    d.2 = now;
                }
                None => dirs.push((path, 1.0, now)),
            }
            changed = true;
        }
        if changed {
            let dirs = self.term_dirs();
            // (Kept to the 400 best, as zoxide ages them.)
            dirs.sort_by(|a, b| b.1.total_cmp(&a.1));
            dirs.truncate(400);
            let text: String = dirs.iter().map(|(p, c, w)| format!("{c}\t{w}\t{p}\n")).collect();
            let file = dirs_file();
            if let Some(d) = file.parent() {
                let _ = std::fs::create_dir_all(d);
            }
            let tmp = file.with_extension("tmp");
            if std::fs::write(&tmp, text).and_then(|()| std::fs::rename(&tmp, &file)).is_err() {
                warn!("term: the folders gone to not kept");
            }
        }
    }

    /// Folders gone to often that `word` finds (a `cd`'s), the best first:
    /// often and lately (zoxide's way).
    fn term_frequent(&mut self, word: &str) -> Vec<String> {
        let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_secs() as i64);
        let home = std::env::var("HOME").unwrap_or_default();
        let mut found: Vec<(f32, String)> = self
            .term_dirs()
            .iter()
            .filter_map(|(p, count, when)| {
                let age = now - when;
                let fresh = if age < 3600 {
                    4.0
                } else if age < 86_400 {
                    2.0
                } else if age < 7 * 86_400 {
                    0.5
                } else {
                    0.25
                };
                fuzzy(word, p).map(|s| (count * fresh + s as f32 / 50.0, p.clone()))
            })
            .collect();
        found.sort_by(|a, b| b.0.total_cmp(&a.0));
        found
            .into_iter()
            .map(|(_, p)| match p.strip_prefix(&home) {
                Some(rest) if !home.is_empty() => format!("~{rest}"),
                _ => p,
            })
            .take(3)
            .collect()
    }

    /// What runs in the tabs now: a command that ran a while and ended out
    /// of sight (another tab shown, the box closed) is told.
    fn term_ran(&mut self, tabs: &[Tab]) {
        let now = Instant::now();
        let open = self.term_on() && self.ui.target() == crate::state::Target::Open;
        let shown = self.term_shown();
        for t in tabs {
            let seen = open && shown == t.n;
            // The shell hook's word on the last command, when it changed:
            // its status, its own times, its line.
            let raw = std::fs::read_to_string(status_dir().join(t.pane.trim_start_matches('%'))).unwrap_or_default();
            let before = self.term.status_seen.insert(t.pane.clone(), raw.clone());
            // (News: changed since last read — or, a tab first seen, written
            // since the terminal started here.)
            let news = match &before {
                Some(b) => *b != raw,
                None => raw.split(' ').nth(2).and_then(|e| e.parse::<i64>().ok()).is_some_and(|end| end >= self.term.started_at),
            };
            if let Some((status, secs, line)) = parse_status(&raw).filter(|_| news) {
                self.term.running.remove(&t.n);
                self.term_told(t.n, &line, Duration::from_secs(secs), Some(status), seen);
                continue;
            }
            if !t.idle() {
                let same = self.term.running.get(&t.n).is_some_and(|(c, _)| *c == t.command);
                if !same {
                    self.term.running.insert(t.n, (t.command.clone(), now));
                }
                continue;
            }
            // (No hook in its shell: what the box saw run, and for how long.)
            if let Some((cmd, since)) = self.term.running.remove(&t.n) {
                self.term_told(t.n, &cmd, since.elapsed(), None, seen);
            }
        }
        self.term.running.retain(|n, _| tabs.iter().any(|t| t.n == *n));
    }

    /// A command ended in tab `n`: a failure marks its word (red, out of
    /// sight); one that ran a while out of sight is told — failed or done.
    fn term_told(&mut self, n: u32, what: &str, took: Duration, status: Option<i32>, seen: bool) {
        if status.is_some_and(|s| s != 0) && !seen {
            self.term.failed_tabs.insert(n);
        }
        // (Told once: the hook and the box's own eyes may both see it.)
        let again = self.term.told_at.get(&n).is_some_and(|t| t.elapsed() < Duration::from_secs(5));
        if took < LONG_RUN || seen || again {
            self.schedule_frame();
            return;
        }
        self.term.told_at.insert(n, Instant::now());
        self.term.busy.insert(n);
        let words = match status {
            Some(0) => crate::i18n::tr("{program} is done · tab {n}"),
            Some(_) => crate::i18n::tr("{program} failed · tab {n}"),
            None => crate::i18n::tr("{program} ended · tab {n}"),
        };
        let program: String = what.chars().take(40).collect();
        let title = crate::i18n::fill(words, &[("program", &program), ("n", &n.to_string())]);
        let body = crate::i18n::fill(crate::i18n::tr("It ran {time}"), &[("time", &took_words(took))]);
        notify(&title, &body);
        self.schedule_frame();
    }

    /// The words' size in the box (the box's own zoom), a line's height.
    pub(crate) fn term_px(&self) -> (f32, f32) {
        let k = self.icon_scale().clamp(0.8, 1.6);
        (FONT_PX * k, LINE_PX * k)
    }

    /// The box's terminal room in cells, from `layout` (a cell measured at the
    /// words' size): tmux told when it changed.
    pub(crate) fn term_fit(&mut self, layout: &crate::content::Layout) {
        let (font, line) = self.term_px();
        if let Some(r) = self.renderer.as_mut() {
            if (self.term.cell_for - font).abs() > 0.01 || self.term.cell_w <= 0.0 {
                self.term.cell_w = r.measure_text("MMMMMMMMMM", font, Some(crate::options::NERD)) / 10.0;
                self.term.cell_for = font;
            }
        }
        let area = crate::content::term_area(layout, line.max(crate::deep_search::TAB_LINE_PX));
        if self.term.cell_w > 0.0 && area.w > 0.0 {
            let cols = (area.w / self.term.cell_w).floor().clamp(10.0, 500.0) as u16;
            let rows = (area.h / line).floor().clamp(3.0, 200.0) as u16;
            self.term_resize(cols, rows);
        }
    }

    /// The box's terminal room changed size (in cells): every tab follows.
    pub(crate) fn term_resize(&mut self, cols: u16, rows: u16) {
        if (cols, rows) == self.term.size || cols < 10 || rows < 3 {
            return;
        }
        self.term.size = (cols, rows);
        if self.term.to_engine.is_some() {
            self.term_ask(Req::Resize { cols, rows });
            self.term_capture();
        }
    }

    /// Whether a program holds the shown tab's screen and the keys are its.
    pub(crate) fn term_raw(&self) -> bool {
        self.term_on() && !self.term.keys_to_field && self.term.screen.as_ref().is_some_and(|s| s.alternate && s.tab == self.term_shown())
    }

    /// Whether the shown tab asks for a password (the field shows dots).
    pub(crate) fn term_secret(&self) -> bool {
        self.term_on() && self.term.screen.as_ref().is_some_and(|s| s.tab == self.term_shown() && asks_password(s))
    }

    /// A key while the box is the terminal: the ones that are its own.
    pub(crate) fn term_key(&mut self, keysym: Keysym, utf8: Option<&str>) -> bool {
        if !self.term_on() {
            return false;
        }
        let (ctrl, alt) = (self.modifiers.ctrl, self.modifiers.alt);
        let shown = self.term_shown();
        if is_modifier(keysym) {
            return false;
        }
        // A tab being named: the field is its name.
        if let Some((tab, mut name)) = self.term.renaming.take() {
            match keysym {
                Keysym::Return | Keysym::KP_Enter => self.term_ask(Req::Rename { tab, name }),
                Keysym::Escape => {}
                Keysym::BackSpace => {
                    name.pop();
                    self.term.renaming = Some((tab, name));
                }
                _ => {
                    name.extend(utf8.unwrap_or("").chars().filter(|c| !c.is_control()));
                    self.term.renaming = Some((tab, name));
                }
            }
            self.schedule_frame();
            return true;
        }
        // Ctrl+F: the field is what is looked for; Enter and Up the find
        // before (older), Down the one after, Escape back to the screen.
        if let Some(mut f) = self.term.find.take() {
            let mut look = false;
            match keysym {
                Keysym::Escape => {
                    self.term_scroll_by(-i64::from(u32::MAX));
                    self.schedule_frame();
                    return true;
                }
                Keysym::Return | Keysym::KP_Enter | Keysym::Up if !self.modifiers.shift => f.at = f.at.saturating_sub(1),
                Keysym::Return | Keysym::KP_Enter | Keysym::Down => f.at = (f.at + 1).min(f.hits.len().saturating_sub(1)),
                _ if ctrl && matches!(keysym.key_char(), Some('f' | 'F')) => f.at = f.at.saturating_sub(1),
                Keysym::BackSpace => {
                    f.needle.pop();
                    look = true;
                }
                _ => {
                    let text: String = utf8.unwrap_or("").chars().filter(|c| !c.is_control()).collect();
                    if text.is_empty() || ctrl || alt {
                        self.term.find = Some(f);
                        return true;
                    }
                    f.needle.push_str(&text);
                    look = true;
                }
            }
            let needle = f.needle.clone();
            self.term.find = Some(f);
            if look {
                self.term_find(needle);
            } else {
                self.term_find_show();
            }
            self.schedule_frame();
            return true;
        }
        // A busy tab asked to close: Enter or y closes it, anything else not.
        if let Some(tab) = self.term.closing.take() {
            if matches!(keysym, Keysym::Return | Keysym::KP_Enter | Keysym::y | Keysym::Y) {
                self.term_close_now(tab);
            }
            self.schedule_frame();
            return true;
        }
        // The terminals' keys for the tabs: Alt+1…9, Ctrl+Tab and
        // Ctrl+Shift+Tab, Ctrl+PageDown/PageUp.
        if alt && !ctrl {
            if let Some(d) = keysym.key_char().and_then(|c| c.to_digit(10)) {
                let n = if d == 0 { 10 } else { d };
                if self.term_tab(n).is_some() {
                    self.term_show(n);
                }
                return true;
            }
        }
        if ctrl && matches!(keysym, Keysym::Tab | Keysym::ISO_Left_Tab | Keysym::Next | Keysym::Prior | Keysym::KP_Next | Keysym::KP_Prior) {
            let back = matches!(keysym, Keysym::ISO_Left_Tab | Keysym::Prior | Keysym::KP_Prior) || self.modifiers.shift;
            self.term_step(back);
            return true;
        }
        // tmux's prefix: Ctrl+B, then the tab's key.
        if self.term.prefix {
            self.term.prefix = false;
            self.term_prefixed(keysym, ctrl);
            self.schedule_frame();
            return true;
        }
        if ctrl && !alt && matches!(keysym, Keysym::b | Keysym::B) {
            self.term.prefix = true;
            self.schedule_frame();
            return true;
        }
        // Ctrl+Shift+C / V: the text picked to the clipboard, the clipboard
        // in (a program holding the screen gets it as a paste).
        if ctrl && self.modifiers.shift && matches!(keysym.key_char(), Some('c' | 'C')) {
            if let Some(text) = self.term_selected_text() {
                self.copy_text(&text);
            }
            return true;
        }
        if ctrl && self.modifiers.shift && matches!(keysym.key_char(), Some('v' | 'V')) {
            self.paste();
            return true;
        }
        // Shift+PageUp/PageDown, Shift+Home/End: back in the tab's history.
        if self.modifiers.shift && !self.term.screen.as_ref().is_some_and(|s| s.alternate) {
            let page = i64::from(self.term.size.1.saturating_sub(2).max(1));
            let by = match keysym {
                Keysym::Prior | Keysym::KP_Prior => Some(page),
                Keysym::Next | Keysym::KP_Next => Some(-page),
                Keysym::Home | Keysym::KP_Home => Some(i64::from(u32::MAX)),
                Keysym::End | Keysym::KP_End => Some(-i64::from(u32::MAX)),
                _ => None,
            };
            if let Some(by) = by {
                self.term_scroll_by(by);
                return true;
            }
        }
        if self.term_raw() {
            // Ctrl+Escape: the keys back to the field.
            if ctrl && keysym == Keysym::Escape {
                self.term.keys_to_field = true;
                self.schedule_frame();
                return true;
            }
            if let Some(key) = key_name(keysym, ctrl, alt, utf8) {
                self.term_ask(Req::Keys { tab: shown, keys: vec![key], dir: None });
            }
            return true;
        }
        self.term_line_key(keysym, utf8)
    }

    /// A key while what is typed is the field's (no program holds the
    /// screen): the rows, the history, the field's own editing.
    fn term_line_key(&mut self, keysym: Keysym, utf8: Option<&str>) -> bool {
        let (ctrl, alt, shift) = (self.modifiers.ctrl, self.modifiers.alt, self.modifiers.shift);
        let shown = self.term_shown();
        let rows = !self.term.comps.is_empty();
        let ch = keysym.key_char().map(|c| c.to_ascii_lowercase());
        let (_, cmd) = self.term_typed();
        let at_end = self.term.caret_back == 0;
        // Ctrl+F: find in the tab's history.
        if ctrl && ch == Some('f') && !self.term.screen.as_ref().is_some_and(|s| s.alternate) {
            self.term.find = Some(Find::default());
            self.schedule_frame();
            return true;
        }
        // Ctrl+R: the history searched (again: the next one up).
        if ctrl && ch == Some('r') {
            if self.term.searching && rows {
                self.term.comp_sel = (self.term.comp_sel + 1).min(self.term.comps.len() - 1);
                self.term.comp_moved = true;
            } else {
                self.term.searching = true;
                self.term.hist_at = None;
                self.term.hist_query = None;
                self.term_rows();
            }
            self.schedule_frame();
            return true;
        }
        match keysym {
            Keysym::Prior | Keysym::KP_Prior => {
                self.term_scroll_by(i64::from(self.term.size.1.saturating_sub(2).max(1)));
                return true;
            }
            Keysym::Next | Keysym::KP_Next => {
                self.term_scroll_by(-i64::from(self.term.size.1.saturating_sub(2).max(1)));
                return true;
            }
            // Escape read back: to the screen as it is now (the pick goes).
            Keysym::Escape if self.term.scroll > 0 || self.term.sel.is_some() => {
                self.term.sel = None;
                self.term_scroll_by(-i64::from(u32::MAX));
                return true;
            }
            Keysym::Return | Keysym::KP_Enter => {
                // (A row picked by hand — or any, searching — runs; else
                // what is typed.)
                if rows && (self.term.comp_moved || self.term.searching) {
                    self.term_take(self.term.comp_sel);
                }
                self.term_enter();
                return true;
            }
            Keysym::Tab if rows => {
                self.term_take(self.term.comp_sel);
                return true;
            }
            // Tab with no rows: every completion, the folder's files too.
            Keysym::Tab => {
                self.term.comps_hidden = None;
                self.term.comps_all = Some(self.search.query.clone());
                self.term.engine_for.clear();
                self.term_complete();
                return true;
            }
            // (The best row stands nearest the field: Up goes away from it.)
            Keysym::Up | Keysym::ISO_Left_Tab if rows => {
                self.term.comp_sel = (self.term.comp_sel + 1).min(self.term.comps.len() - 1);
                self.term.comp_moved = true;
                self.schedule_frame();
                return true;
            }
            Keysym::Down if rows => {
                self.term.comp_sel = self.term.comp_sel.saturating_sub(1);
                self.term.comp_moved = true;
                self.schedule_frame();
                return true;
            }
            Keysym::Up => {
                self.term_walk(true);
                return true;
            }
            Keysym::Down => {
                self.term_walk(false);
                return true;
            }
            // Escape: out of the search (what is typed stays), the rows
            // hidden, a walk's command back to what was typed.
            Keysym::Escape if self.term.searching => {
                self.term.searching = false;
                self.term_rows();
                self.schedule_frame();
                return true;
            }
            Keysym::Escape if self.term.help => {
                self.term.help = false;
                self.term_rows();
                return true;
            }
            Keysym::Escape if rows => {
                self.term.comps_hidden = Some(self.search.query.clone());
                self.term_rows();
                return true;
            }
            Keysym::Escape if self.term.hist_at.is_some() => {
                let (head, _) = self.term_typed();
                self.term.hist_at = None;
                self.term.hist_query = None;
                self.search.query = format!("{head}{}", self.term.hist_typed);
                self.refilter();
                return true;
            }
            // (Never the box's own walk while it is the terminal.)
            Keysym::ISO_Left_Tab => return true,
            _ => {}
        }
        // THE FIELD'S EDITING, the shells' keys: the caret moves by a
        // character or a word, Right at the end takes the suggestion.
        let words = alt || ctrl;
        match keysym {
            Keysym::Left | Keysym::KP_Left => {
                self.term_caret_to(if words { Move::WordBack } else { Move::Back });
                return true;
            }
            Keysym::Right | Keysym::KP_Right if at_end && !self.term.ghost.is_empty() => {
                self.term_take_ghost(words);
                return true;
            }
            Keysym::Right | Keysym::KP_Right => {
                self.term_caret_to(if words { Move::WordOn } else { Move::On });
                return true;
            }
            Keysym::Home | Keysym::KP_Home => {
                self.term_caret_to(Move::Start);
                return true;
            }
            Keysym::End | Keysym::KP_End if at_end && !self.term.ghost.is_empty() => {
                self.term_take_ghost(false);
                return true;
            }
            Keysym::End | Keysym::KP_End => {
                self.term_caret_to(Move::End);
                return true;
            }
            Keysym::Delete | Keysym::KP_Delete => {
                self.term_cut(Move::On);
                return true;
            }
            Keysym::BackSpace if alt || ctrl => {
                self.term_cut(Move::WordBack);
                return true;
            }
            Keysym::BackSpace if !at_end => {
                self.term_cut(Move::Back);
                return true;
            }
            _ => {}
        }
        if ctrl && !alt {
            match ch {
                Some('a') => self.term_caret_to(Move::Start),
                Some('e') if at_end && !self.term.ghost.is_empty() => self.term_take_ghost(false),
                Some('e') => self.term_caret_to(Move::End),
                Some('w') => self.term_cut(Move::SpaceBack),
                Some('u') => self.term_cut(Move::Start),
                Some('k') => self.term_cut(Move::End),
                // Ctrl+C: what is typed goes; with nothing typed, the tab's.
                Some('c') if !cmd.is_empty() => {
                    let (head, _) = self.term_typed();
                    self.term.caret_back = 0;
                    self.term.searching = false;
                    self.search.query = head;
                    self.refilter();
                }
                Some(c @ ('c' | 'd' | 'l')) if cmd.is_empty() => {
                    self.term_ask(Req::Keys { tab: shown, keys: vec![Key::Name(format!("C-{c}"))], dir: None });
                }
                Some('l') => self.term_ask(Req::Keys { tab: shown, keys: vec![Key::Name("C-l".into())], dir: None }),
                _ => return false,
            }
            return true;
        }
        if alt && !ctrl {
            match ch {
                Some('b') => self.term_caret_to(Move::WordBack),
                Some('f') if at_end && !self.term.ghost.is_empty() => self.term_take_ghost(true),
                Some('f') => self.term_caret_to(Move::WordOn),
                Some('d') => self.term_cut(Move::WordOn),
                _ => return false,
            }
            return true;
        }
        // Text typed with the caret back in the line: where it stands.
        if !at_end && !ctrl && !alt {
            let text: String = utf8.unwrap_or("").chars().filter(|c| !c.is_control()).collect();
            if !text.is_empty() {
                let at = self.term_caret();
                self.search.query.insert_str(at, &text);
                self.refilter();
                return true;
            }
        }
        let _ = shift;
        false
    }

    /// Where the caret stands in the query (a byte index; never before the
    /// command).
    fn term_caret(&self) -> usize {
        let q = &self.search.query;
        let start = self.term_typed().0.len();
        let n = q[start..].chars().count();
        let back = self.term.caret_back.min(n);
        q[start..].char_indices().nth(n - back).map_or(q.len(), |(i, _)| start + i)
    }

    /// Where a move from the caret lands (a byte index in the query).
    fn term_target(&self, m: Move) -> usize {
        let q = &self.search.query;
        let start = self.term_typed().0.len();
        let at = self.term_caret();
        let before: Vec<(usize, char)> = q[start..at].char_indices().map(|(i, c)| (start + i, c)).collect();
        let after: Vec<(usize, char)> = q[at..].char_indices().map(|(i, c)| (at + i, c)).collect();
        let word = |c: char| c.is_alphanumeric() || c == '_';
        match m {
            Move::Start => start,
            Move::End => q.len(),
            Move::Back => before.last().map_or(start, |(i, _)| *i),
            Move::On => after.get(1).map_or(q.len(), |(i, _)| *i),
            // A word back: past what is not a word, then to the word's start.
            Move::WordBack | Move::SpaceBack => {
                let is = |c: char| if m == Move::SpaceBack { !c.is_whitespace() } else { word(c) };
                let mut k = before.len();
                while k > 0 && !is(before[k - 1].1) {
                    k -= 1;
                }
                while k > 0 && is(before[k - 1].1) {
                    k -= 1;
                }
                before.get(k).map_or(at, |(i, _)| *i).min(at)
            }
            Move::WordOn => {
                let mut k = 0;
                while k < after.len() && !word(after[k].1) {
                    k += 1;
                }
                while k < after.len() && word(after[k].1) {
                    k += 1;
                }
                after.get(k).map_or(q.len(), |(i, _)| *i)
            }
        }
    }

    fn term_caret_to(&mut self, m: Move) {
        let to = self.term_target(m);
        self.term.caret_back = self.search.query[to..].chars().count();
        // (The caret moved off the end: no suggestion meanwhile.)
        self.term_rows();
        self.schedule_frame();
    }

    /// Cut from the caret to where a move lands.
    fn term_cut(&mut self, m: Move) {
        let (a, b) = (self.term_caret(), self.term_target(m));
        let (from, to) = (a.min(b), a.max(b));
        if from == to {
            return;
        }
        let tail = self.search.query[a.max(b)..].chars().count();
        self.search.query.replace_range(from..to, "");
        self.term.caret_back = tail;
        self.refilter();
    }

    /// The suggestion into the field: all of it, or its next word.
    fn term_take_ghost(&mut self, word: bool) {
        let ghost = std::mem::take(&mut self.term.ghost);
        let take = if word {
            let lead = ghost.len() - ghost.trim_start().len();
            let rest = &ghost[lead..];
            let end = rest.find(|c: char| c.is_whitespace() || c == '/').map_or(rest.len(), |i| i + rest[i..].chars().next().map_or(0, char::len_utf8));
            &ghost[..lead + end]
        } else {
            &ghost[..]
        };
        self.search.query.push_str(take);
        self.term.caret_back = 0;
        self.refilter();
    }

    /// Look for `needle` in the shown tab.
    fn term_find(&mut self, needle: String) {
        if let Some(f) = self.term.find.as_mut() {
            f.hits.clear();
            f.at = 0;
        }
        if needle.is_empty() {
            return;
        }
        if let Some(pane) = self.term_tab(self.term_shown()).map(|t| t.pane.clone()) {
            self.term_ask(Req::Find { pane, needle });
        }
    }

    /// The find picked, in sight: the view read back so it stands in the
    /// middle (the screen's own lines: as it is).
    fn term_find_show(&mut self) {
        let Some(&(line, _, _)) = self.term.find.as_ref().and_then(|f| f.hits.get(f.at)) else {
            return;
        };
        let history = u64::from(self.term.screen.as_ref().map_or(0, |s| s.history));
        let half = u64::from(self.term.size.1 / 2);
        let to = if line >= history { 0 } else { (history - line + half).min(history) };
        let by = to as i64 - i64::from(self.term.scroll);
        self.term_scroll_by(by);
        self.schedule_frame();
    }

    /// The finds in sight: (screen line, column, length, the one picked).
    pub(crate) fn term_found(&self) -> Vec<(usize, u16, u16, bool)> {
        let (Some(f), Some(s)) = (self.term.find.as_ref(), self.term.screen.as_ref()) else {
            return Vec::new();
        };
        let top = u64::from(s.history.saturating_sub(s.scroll));
        f.hits
            .iter()
            .enumerate()
            .filter(|(_, (l, _, _))| *l >= top && *l < top + s.lines.len() as u64)
            .map(|(i, &(l, c, n))| ((l - top) as usize, c, n, i == f.at))
            .collect()
    }

    /// Read `by` lines further back in the tab's history (less: forward).
    fn term_scroll_by(&mut self, by: i64) {
        let most = i64::from(self.term.screen.as_ref().map_or(0, |s| s.history));
        let to = (i64::from(self.term.scroll) + by).clamp(0, most) as u32;
        if to != self.term.scroll {
            self.term.scroll = to;
            self.term.sel = None;
            self.term_capture();
            self.schedule_frame();
        }
    }

    /// The wheel over the box while it is the terminal: back and forth in
    /// the tab's history, or — a program holding the screen — Up and Down
    /// for it (its own scrolling: less, man, a pager).
    pub(crate) fn term_wheel(&mut self, value: f64) -> bool {
        if !self.term_on() {
            return false;
        }
        let line = self.term_px().1;
        self.term.wheel += value as f32 / line * 1.5;
        let lines = self.term.wheel.trunc() as i64;
        if lines == 0 {
            return true;
        }
        self.term.wheel -= lines as f32;
        if self.term.screen.as_ref().is_some_and(|s| s.alternate) {
            let key = if lines > 0 { "Down" } else { "Up" };
            let keys = (0..lines.unsigned_abs().min(20)).map(|_| Key::Name(key.into())).collect();
            self.term_ask(Req::Keys { tab: self.term_shown(), keys, dir: None });
        } else {
            self.term_scroll_by(-lines);
        }
        true
    }

    /// The screen's line and column under `pos`, as drawn (the rows over the
    /// field lift the lines; the column clamped to the line).
    fn term_cell_at(&self, pos: (f32, f32)) -> Option<(u16, usize)> {
        let screen = self.term.screen.as_ref()?;
        let layout = self.current_layout();
        let (_, line) = self.term_px();
        let tab_h = line.max(crate::deep_search::TAB_LINE_PX);
        let used = used_lines(&screen.lines, screen.cursor);
        let (area, skip, rows) = crate::content::term_view(&layout, tab_h, line, screen.lines.len(), used, self.term.comps.len());
        if !area.contains(pos) || self.term.cell_w <= 0.0 {
            return None;
        }
        let row = ((pos.1 - area.y) / line).floor().max(0.0) as usize;
        if row >= rows {
            return None;
        }
        let col = ((pos.0 - area.x) / self.term.cell_w).floor().max(0.0) as u16;
        Some((col.min(self.term.size.0.saturating_sub(1)), skip + row))
    }

    /// A press on the screen: a pick of its text begins (a second click on
    /// the same cell soon: its word, copied); with Ctrl, a link under it
    /// opens.
    pub(crate) fn term_press(&mut self, pos: (f32, f32)) -> bool {
        if !self.term_on() {
            return false;
        }
        let Some(cell) = self.term_cell_at(pos) else {
            return false;
        };
        let cells = self.term.screen.as_ref().and_then(|s| s.lines.get(cell.1)).map(|l| line_cells(l)).unwrap_or_default();
        if self.modifiers.ctrl {
            let dir = self.term_tab(self.term_shown()).map(|t| t.path.clone()).unwrap_or_default();
            if let Some(link) = link_at(&cells, usize::from(cell.0), &dir) {
                info!("term: opening a link");
                let cmd = format!("xdg-open {}", crate::launch::shell_quote(&link));
                if let Err(e) = crate::launch::launch(&cmd, false, &self.config.launch.terminal) {
                    warn!("term: {e}");
                }
                self.dismiss();
                return true;
            }
        }
        let double = self.term.clicked.is_some_and(|(t, c)| c == cell && t.elapsed() < Duration::from_millis(400));
        self.term.clicked = Some((Instant::now(), cell));
        if double {
            if let Some((a, b)) = word_at(&cells, usize::from(cell.0)) {
                self.term.sel = Some(((a as u16, cell.1), (b as u16, cell.1)));
                self.term.selecting = false;
                if let Some(text) = self.term_selected_text() {
                    self.copy_text(&text);
                }
                self.schedule_frame();
                return true;
            }
        }
        self.term.sel = Some((cell, cell));
        self.term.selecting = true;
        self.schedule_frame();
        true
    }

    /// The pointer moving with the press held: the pick follows it.
    pub(crate) fn term_drag(&mut self, pos: (f32, f32)) {
        if !self.term.selecting {
            return;
        }
        if let (Some((from, _)), Some(cell)) = (self.term.sel, self.term_cell_at(pos)) {
            if self.term.sel.map(|s| s.1) != Some(cell) {
                self.term.sel = Some((from, cell));
                self.schedule_frame();
            }
        }
    }

    /// The press let go: what was picked (more than a cell) is copied; a
    /// mere click picks nothing.
    pub(crate) fn term_release(&mut self) -> bool {
        if !std::mem::take(&mut self.term.selecting) {
            return false;
        }
        match self.term.sel {
            Some((a, b)) if a != b => {
                if let Some(text) = self.term_selected_text() {
                    self.copy_text(&text);
                }
            }
            _ => self.term.sel = None,
        }
        self.schedule_frame();
        true
    }

    /// The text picked, line by line (each line's trailing spaces left out).
    pub(crate) fn term_selected_text(&self) -> Option<String> {
        let ((ac, al), (bc, bl)) = self.term.sel?;
        let ((ac, al), (bc, bl)) = if (al, ac) <= (bl, bc) { ((ac, al), (bc, bl)) } else { ((bc, bl), (ac, al)) };
        let screen = self.term.screen.as_ref()?;
        let mut out = Vec::new();
        for l in al..=bl {
            let cells = screen.lines.get(l).map(|x| line_cells(x)).unwrap_or_default();
            let from = if l == al { usize::from(ac) } else { 0 };
            let to = if l == bl { usize::from(bc) + 1 } else { cells.len() };
            let piece: String = cells.get(from.min(cells.len())..to.min(cells.len())).unwrap_or(&[]).iter().collect();
            out.push(piece.trim_end().to_owned());
        }
        let text = out.join("\n");
        (!text.trim().is_empty()).then_some(text)
    }

    /// A paste while the box is the terminal: a program holding the screen
    /// gets it (as a paste: bracketed when it asks), so does the tab when it
    /// is several lines; else into the field at the caret.
    pub(crate) fn term_paste(&mut self, text: &str) -> bool {
        if !self.term_on() {
            return false;
        }
        let shown = self.term_shown();
        if self.term_raw() || text.trim_end_matches('\n').contains('\n') {
            self.term_ask(Req::Paste { tab: shown, text: text.to_owned() });
            return true;
        }
        let line: String = text.chars().filter(|c| !c.is_control()).collect();
        if line.is_empty() {
            return true;
        }
        let at = self.term_caret();
        self.search.query.insert_str(at, &line);
        self.refilter();
        true
    }

    /// The key after Ctrl+B, as tmux's: a number shows that tab, n and p
    /// the next and previous, l the last one shown, c a new one; Ctrl+B
    /// again is the program's own.
    fn term_prefixed(&mut self, keysym: Keysym, ctrl: bool) {
        let tabs: Vec<u32> = self.term.tabs.iter().map(|t| t.n).collect();
        let shown = self.term_shown();
        let at = tabs.iter().position(|&n| n == shown);
        let to = match keysym.key_char() {
            Some('b' | 'B') if ctrl => {
                self.term_ask(Req::Keys { tab: shown, keys: vec![Key::Name("C-b".into())], dir: None });
                None
            }
            Some(d @ '0'..='9') => {
                let n = d.to_digit(10).map_or(0, |d| if d == 0 { 10 } else { d });
                tabs.contains(&n).then_some(n)
            }
            Some('n') if !tabs.is_empty() => Some(at.map_or(tabs[0], |i| tabs[(i + 1) % tabs.len()])),
            Some('p') if !tabs.is_empty() => Some(at.map_or(tabs[tabs.len() - 1], |i| tabs[(i + tabs.len() - 1) % tabs.len()])),
            Some('l') => tabs.contains(&self.term.last).then_some(self.term.last),
            Some('c') => {
                self.term_new_tab();
                None
            }
            Some('x' | '&') => {
                self.term_close(shown);
                None
            }
            Some('y') => {
                match (self.term.sent_at.get(&shown).copied(), self.term_tab(shown).map(|t| t.pane.clone())) {
                    (Some(from), Some(pane)) => self.term_ask(Req::CopyOutput { pane, from }),
                    _ => self.term_flash(crate::i18n::tr("Nothing printed")),
                }
                None
            }
            Some('?') => {
                self.term.help = !self.term.help;
                self.term_rows();
                None
            }
            Some(',') => {
                let name = self.term_tab(shown).and_then(|t| t.title.clone()).unwrap_or_default();
                self.term.renaming = Some((shown, name));
                None
            }
            _ if matches!(keysym, Keysym::Right | Keysym::Left) => {
                self.term_step(keysym == Keysym::Left);
                None
            }
            _ => None,
        };
        if let Some(n) = to {
            self.term_show(n);
        }
    }

    /// The next tab (`back`: the one before), round.
    fn term_step(&mut self, back: bool) {
        let tabs: Vec<u32> = self.term.tabs.iter().map(|t| t.n).collect();
        if tabs.is_empty() {
            return;
        }
        let shown = self.term_shown();
        let i = tabs.iter().position(|&n| n == shown);
        let to = match (i, back) {
            (None, _) => tabs[0],
            (Some(i), false) => tabs[(i + 1) % tabs.len()],
            (Some(i), true) => tabs[(i + tabs.len() - 1) % tabs.len()],
        };
        self.term_show(to);
    }

    /// Close tab `n`: at once when it is at its prompt, else asked first
    /// (the field says what runs in it).
    pub(crate) fn term_close(&mut self, n: u32) {
        match self.term_tab(n) {
            Some(t) if !t.idle() => self.term.closing = Some(n),
            Some(_) => self.term_close_now(n),
            None => {}
        }
        self.schedule_frame();
    }

    fn term_close_now(&mut self, n: u32) {
        info!("term: tab {n} closed");
        // (The tab shown goes: the one beside it is shown.)
        if self.term_shown() == n {
            let tabs: Vec<u32> = self.term.tabs.iter().map(|t| t.n).filter(|&t| t != n).collect();
            let next = tabs.iter().copied().filter(|&t| t < n).max().or_else(|| tabs.first().copied()).unwrap_or(1);
            self.term_show(next);
        }
        self.term.busy.remove(&n);
        self.term.running.remove(&n);
        self.term_ask(Req::Close { tab: n });
    }

    /// A middle click on the tabs' row: that tab closes (as a browser's).
    pub(crate) fn term_middle(&mut self, i: usize) {
        if let Some(&(n, _)) = self.term_tab_list().get(i).filter(|(n, _)| *n != 0) {
            self.term_close(n);
        }
    }

    /// Show tab `n`: the field keeps what is typed, without a tab named.
    fn term_show(&mut self, n: u32) {
        let (_, cmd) = self.term_typed();
        self.term.current = n;
        self.term.busy.remove(&n);
        self.term.failed_tabs.remove(&n);
        self.term.engine_for.clear();
        self.search.query = format!("./{cmd}");
        self.refilter();
    }

    /// A new tab, the next number free, in the shown tab's folder.
    fn term_new_tab(&mut self) {
        let n = self.term_next_free();
        let dir = self.term_tab(self.term_shown()).map(|t| t.path.clone());
        self.term_ask(Req::Keys { tab: n, keys: Vec::new(), dir });
        self.term_show(n);
    }

    fn term_next_free(&self) -> u32 {
        self.term.tabs.iter().map(|t| t.n).chain([self.term_shown()]).max().unwrap_or(0) + 1
    }

    /// Row `i`'s completion into the field.
    fn term_take(&mut self, i: usize) {
        let Some(c) = self.term.comps.get(i).cloned() else {
            return;
        };
        if c.kind == CompKind::Help {
            self.term.help = false;
            self.term_rows();
            return;
        }
        let (head, _) = self.term_typed();
        self.term.comps_hidden = None;
        self.term.comps_all = None;
        self.term.comp_moved = false;
        self.term.searching = false;
        self.term.caret_back = 0;
        self.search.query = format!("{head}{}", c.text);
        self.refilter();
    }

    /// Up (`back`) or Down with no rows shown: the commands typed before
    /// that begin with what was typed, newest first.
    fn term_walk(&mut self, back: bool) {
        let (head, cmd) = self.term_typed();
        if self.term.hist_at.is_none() {
            if !back {
                return;
            }
            self.term.hist_typed = cmd;
        }
        let typed = self.term.hist_typed.clone();
        let mut seen = std::collections::HashSet::new();
        let found: Vec<&String> = self
            .term
            .hist
            .iter()
            .chain(&self.term.sent)
            .rev()
            .map(|(_, h)| h)
            .filter(|h| h.starts_with(&typed) && **h != typed && seen.insert(*h))
            .collect();
        let at = match (self.term.hist_at, back) {
            (None, _) => 0,
            (Some(i), true) => (i + 1).min(found.len().saturating_sub(1)),
            (Some(0), false) => {
                // (Down past the newest: what was typed back.)
                self.term.hist_at = None;
                self.term.hist_query = None;
                self.search.query = format!("{head}{typed}");
                self.refilter();
                return;
            }
            (Some(i), false) => i - 1,
        };
        let Some(entry) = found.get(at).map(|s| (*s).clone()) else {
            return;
        };
        self.term.hist_at = Some(at);
        self.term.caret_back = 0;
        self.search.query = format!("{head}{entry}");
        self.term.hist_query = Some(self.search.query.clone());
        self.refilter();
    }

    /// A click on the tabs' row: word `i` (the last one the "+"), or on a
    /// completion's row past them.
    pub(crate) fn term_click(&mut self, i: usize) {
        let tabs = self.term_tab_list();
        if let Some(&(n, _)) = tabs.get(i) {
            if n == 0 {
                self.term_new_tab();
            } else {
                self.term_show(n);
            }
            return;
        }
        self.term_take(i - tabs.len());
    }

    /// A word in the chip for a moment.
    fn term_flash(&mut self, word: &'static str) {
        self.term.flash = Some((word, Instant::now()));
        self.schedule_frame();
        let _ = self.loop_handle.insert_source(calloop::timer::Timer::from_duration(Duration::from_millis(1600)), |_, _, app: &mut App| {
            app.schedule_frame();
            calloop::timer::TimeoutAction::Drop
        });
    }

    /// The chip in the field: "Terminal", or tmux's prefix waiting.
    pub(crate) fn term_chip(&self) -> &'static str {
        if let Some((word, _)) = self.term.flash.filter(|(_, at)| at.elapsed() < Duration::from_millis(1500)) {
            return word;
        }
        if self.term.prefix {
            "Ctrl+B"
        } else if self.term.find.is_some() {
            crate::i18n::tr("Find")
        } else if self.term.renaming.is_some() {
            crate::i18n::tr("Name")
        } else if self.term.closing.is_some() {
            crate::i18n::tr("Close")
        } else if self.term.searching {
            crate::i18n::tr("History")
        } else {
            crate::i18n::tr("Terminal")
        }
    }

    /// Where the caret stands in what the field shows, as the bytes before
    /// it (`None`: at the end), and the suggestion after it.
    pub(crate) fn term_field_caret(&self) -> (Option<usize>, &str) {
        if !self.term_on() || self.term_raw() || self.term_secret() || self.term.renaming.is_some() || self.term.closing.is_some() || self.term.find.is_some() {
            return (None, "");
        }
        let caret = (self.term.caret_back > 0).then(|| self.term_caret());
        (caret, &self.term.ghost)
    }

    /// Enter: the command to its tab (made when it is new), the tab shown,
    /// the field back to `./`.
    fn term_enter(&mut self) {
        let Some(ask) = parse(&self.search.query) else {
            return;
        };
        // `./*`, `./1,3`, `./+`: the command to each (a new tab: made).
        if let Some(many) = &ask.many {
            let dir = self.term_tab(self.term.current).map(|t| t.path.clone());
            let tabs: Vec<u32> = match many {
                Many::All => self.term.tabs.iter().map(|t| t.n).collect(),
                Many::These(v) => v.clone(),
                Many::New => vec![self.term_next_free()],
            };
            let keys = if ask.cmd.is_empty() { Vec::new() } else { vec![Key::Text(ask.cmd.clone()), Key::Name("Enter".into())] };
            info!("term: {} tabs, a command", tabs.len());
            for &tab in &tabs {
                self.term_ask(Req::Keys { tab, keys: keys.clone(), dir: dir.clone() });
            }
            if !ask.cmd.is_empty() {
                let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_secs() as i64);
                self.term.sent.push((now, ask.cmd.clone()));
            }
            // (A new tab with nothing to run: shown.)
            if *many == Many::New && ask.cmd.is_empty() {
                self.term.current = tabs[0];
            }
            self.term.caret_back = 0;
            self.term.searching = false;
            self.search.query = "./".into();
            self.refilter();
            return;
        }
        let tab = ask.tab.unwrap_or(self.term.current.max(1));
        let dir = self.term_tab(self.term.current).map(|t| t.path.clone());
        let mut keys = Vec::new();
        if !ask.cmd.is_empty() || self.term_secret() {
            let here = self.term_tab(tab).map(|t| t.path.clone()).or(dir.clone()).unwrap_or_default();
            let cmd = if self.term_secret() { ask.cmd.clone() } else { as_script(&ask.cmd, &here) };
            if !cmd.is_empty() {
                keys.push(Key::Text(cmd));
            }
            keys.push(Key::Name("Enter".into()));
        }
        info!("term: tab {tab}{}", if keys.is_empty() { "" } else { ", a command" });
        if !ask.cmd.is_empty() && !self.term_secret() {
            let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_secs() as i64);
            self.term.sent.push((now, ask.cmd.clone()));
        }
        self.term.hist_at = None;
        self.term.hist_query = None;
        self.term.comps_hidden = None;
        self.term.comps_all = None;
        self.term.searching = false;
        self.term.caret_back = 0;
        // (Where its screen stands as the command goes: Ctrl+B y copies
        // what it prints from there.)
        match self.term.screen.as_ref().filter(|s| s.tab == tab && s.scroll == 0 && !keys.is_empty()) {
            Some(s) => {
                let at = u64::from(s.history) + s.cursor.map_or(0, |(_, y)| u64::from(y));
                self.term.sent_at.insert(tab, at);
            }
            None if !keys.is_empty() => {
                self.term.sent_at.remove(&tab);
            }
            None => {}
        }
        self.term.current = tab;
        self.term_ask(Req::Keys { tab, keys, dir });
        self.term_ask(Req::History);
        self.search.query = "./".into();
        self.refilter();
    }

    /// What the field shows: what is typed, or — a password asked — its
    /// dots after the `./N`.
    pub(crate) fn term_field_text(&self) -> Option<String> {
        if let Some(f) = &self.term.find {
            if f.needle.is_empty() || f.hits.is_empty() {
                return Some(f.needle.clone());
            }
            return Some(format!("{}   {}/{}", f.needle, f.at + 1, f.hits.len()));
        }
        if let Some((_, name)) = &self.term.renaming {
            return Some(name.clone());
        }
        if let Some(n) = self.term.closing {
            let program = self.term_tab(n).map(|t| t.command.clone()).unwrap_or_default();
            return Some(crate::i18n::fill(crate::i18n::tr("{program} is running · Enter closes the tab, Esc keeps it"), &[("program", &program)]));
        }
        // (A program holds the keys: which, and the way back to the field.)
        if self.term_raw() {
            let program = self.term.screen.as_ref().map(|s| s.command.clone()).unwrap_or_default();
            return Some(crate::i18n::fill(crate::i18n::tr("{program} has the keys · Ctrl+Esc types here"), &[("program", &program)]));
        }
        if !self.term_secret() {
            return None;
        }
        let ask = parse(&self.search.query)?;
        let head = &self.search.query[..self.search.query.len() - ask.cmd.len()];
        Some(format!("{head}{}", "•".repeat(ask.cmd.chars().count())))
    }

    /// `debug-desktop "term key <tmux key>|text <words>|state"`: the shown
    /// tab without a keyboard.
    pub(crate) fn term_debug(&mut self, what: &str) -> String {
        let (verb, arg) = what.trim().split_once(' ').unwrap_or((what.trim(), ""));
        let tab = self.term_shown();
        match verb {
            "key" => self.term_ask(Req::Keys { tab, keys: vec![Key::Name(arg.to_owned())], dir: None }),
            "text" => self.term_ask(Req::Keys { tab, keys: vec![Key::Text(arg.to_owned())], dir: None }),
            // The box's own keys: "press [c-][a-][s-]up|down|left|right|home|
            // end|tab|stab|esc|enter|bs|del|<a letter>".
            "press" => {
                let mut name = arg;
                let was = self.modifiers;
                for (pre, set) in [("c-", 0), ("a-", 1), ("s-", 2)] {
                    if let Some(rest) = name.strip_prefix(pre) {
                        name = rest;
                        match set {
                            0 => self.modifiers.ctrl = true,
                            1 => self.modifiers.alt = true,
                            _ => self.modifiers.shift = true,
                        }
                    }
                }
                let k = match name {
                    "up" => Keysym::Up,
                    "down" => Keysym::Down,
                    "left" => Keysym::Left,
                    "right" => Keysym::Right,
                    "home" => Keysym::Home,
                    "end" => Keysym::End,
                    "tab" => Keysym::Tab,
                    "stab" => Keysym::ISO_Left_Tab,
                    "esc" => Keysym::Escape,
                    "bs" => Keysym::BackSpace,
                    "del" => Keysym::Delete,
                    "pgup" => Keysym::Prior,
                    "pgdn" => Keysym::Next,
                    one if one.chars().count() == 1 => Keysym::from_char(one.chars().next().unwrap_or(' ')),
                    _ => Keysym::Return,
                };
                let text = (name.chars().count() == 1 && !self.modifiers.ctrl && !self.modifiers.alt).then_some(name);
                if !self.term_key(k, text) {
                    // (What the box would do with it: typed at the end.)
                    if let Some(t) = text {
                        self.search.query.push_str(t);
                        self.refilter();
                    } else if k == Keysym::BackSpace && self.search.query.pop().is_some() {
                        self.refilter();
                    }
                }
                self.modifiers = was;
            }
            "paste" => {
                let text = arg.replace("\\n", "\n");
                self.term_paste(&text);
            }
            // tmux's prefix and a key after it: "prefix 2", "prefix n".
            "prefix" => {
                let k = arg.chars().next().map_or(Keysym::Escape, Keysym::from_char);
                self.term_prefixed(k, false);
            }
            "click" => self.term_click(arg.parse().unwrap_or(0)),
            _ => {}
        }
        let sc = self.term.screen.as_ref();
        let tab = self.term_shown();
        format!(
            "term: query {:?} caret-back {} ghost {:?} searching {}, rows {:?}, sel {} walk {:?}, shown {tab}, tabs {:?}, size {:?}, cell {:.1}, screen {} lines alt={} raw={} secret={} cursor={:?}",
            self.search.query,
            self.term.caret_back,
            self.term.ghost,
            self.term.searching,
            self.term.comps.iter().map(|c| format!("{} · {}", c.text, c.note())).collect::<Vec<_>>(),
            self.term.comp_sel,
            self.term.hist_at,
            self.term.tabs.iter().map(|t| format!("{}:{}", t.n, t.name())).collect::<Vec<_>>(),
            self.term.size,
            self.term.cell_w,
            sc.map_or(0, |s| s.lines.len()),
            sc.is_some_and(|s| s.alternate),
            self.term_raw(),
            self.term_secret(),
            sc.and_then(|s| s.cursor),
        )
    }

    /// The tabs' words, each with its tab: "1 launcher", "2 Golem"; a tab
    /// named but not made yet stands as "3 new"; last a "+" (tab 0) for a
    /// new one.
    fn term_tab_list(&self) -> Vec<(u32, String)> {
        let shown = self.term_shown();
        let mut words: Vec<(u32, String)> = self.term.tabs.iter().map(|t| (t.n, format!("{} {}", t.n, t.name()))).collect();
        if !words.iter().any(|(n, _)| *n == shown) {
            words.push((shown, format!("{shown} {}", crate::i18n::tr("new"))));
            words.sort_by_key(|(n, _)| *n);
        }
        words.push((0, "+".into()));
        words
    }

    /// Which of the tabs' words carry a dot: printed (or ended) out of sight.
    pub(crate) fn term_dots(&self) -> Vec<u8> {
        self.term_tab_list()
            .iter()
            .map(|(n, _)| {
                if self.term.failed_tabs.contains(n) {
                    2
                } else {
                    u8::from(self.term.busy.contains(n))
                }
            })
            .collect()
    }

    /// The words of the tabs for the field's row, with the one lit.
    pub(crate) fn term_words(&self) -> (Vec<String>, usize) {
        let shown = self.term_shown();
        let words = self.term_tab_list();
        let lit = words.iter().position(|(n, _)| *n == shown).unwrap_or(0);
        (words.into_iter().map(|(_, w)| w).collect(), lit)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn what_is_typed_names_a_tab_and_a_command() {
        let ask = |tab: Option<u32>, cmd: &str, many: Option<Many>| Some(Ask { tab, cmd: cmd.into(), many });
        assert_eq!(parse("./2 sudo nixos-rebuild switch"), ask(Some(2), "sudo nixos-rebuild switch", None));
        assert_eq!(parse("./2"), ask(Some(2), "", None));
        assert_eq!(parse("./ls -la"), ask(None, "ls -la", None));
        assert_eq!(parse("./"), ask(None, "", None));
        // (A number that is part of a word is the command's.)
        assert_eq!(parse("./2to3 x.py"), ask(None, "2to3 x.py", None));
        assert_eq!(parse("ls"), None);
        assert_eq!(parse("./0 x"), ask(None, "0 x", None));
        assert_eq!(parse("./* git pull"), ask(None, "git pull", Some(Many::All)));
        assert_eq!(parse("./1,3 make"), ask(None, "make", Some(Many::These(vec![1, 3]))));
        assert_eq!(parse("./+ cargo build"), ask(None, "cargo build", Some(Many::New)));
        assert_eq!(parse("./*.sh"), ask(None, "*.sh", None));
    }

    #[test]
    fn colours_and_runs_come_out_of_a_line() {
        let spans = parse_line("\u{1b}[34m~/Golem\u{1b}[0m \u{1b}[1;38;5;141m❯\u{1b}[0m git");
        assert_eq!(spans[0], Span { col: 0, text: "~/Golem".into(), fg: Colour::Idx(4), ..Default::default() });
        assert_eq!(spans[1].text, " ");
        assert_eq!(spans[2], Span { col: 8, text: "❯".into(), fg: Colour::Idx(141), bold: true, ..Default::default() });
        assert_eq!(spans[3].col, 9);
        let rgb = parse_line("\u{1b}[48;2;10;20;30m  \u{1b}[49mx");
        assert_eq!(rgb[0].bg, Colour::Rgb(10, 20, 30));
        assert_eq!(rgb[1], Span { col: 2, text: "x".into(), ..Default::default() });
        // (Reverse video swaps the two.)
        assert_eq!(parse_line("\u{1b}[31;7mX")[0].bg, Colour::Idx(1));
    }

    #[test]
    fn a_password_asked_is_seen_on_the_cursors_line() {
        let screen = Screen { lines: vec![vec![], parse_line("[sudo] password for max: ")], cursor: Some((25, 1)), ..Default::default() };
        assert!(asks_password(&screen));
        let not = Screen { lines: vec![parse_line("~/Golem ❯ echo password")], cursor: Some((5, 0)), ..Default::default() };
        assert!(!asks_password(&not));
    }

    #[test]
    fn a_tab_is_named_by_what_runs_or_where_it_is() {
        let home = std::env::var("HOME").unwrap_or_default();
        let at = |cmd: &str, path: &str| Tab { n: 1, pane: "%1".into(), command: cmd.into(), path: path.into(), title: None }.name();
        assert_eq!(at("zsh", "/home/max/Golem"), "Golem");
        assert_eq!(at("htop", "/home/max"), "htop");
        assert_eq!(at("bash", &home), "~");
    }

    #[test]
    fn keys_for_a_program_on_the_screen() {
        assert_eq!(key_name(Keysym::Return, false, false, Some("\r")), Some(Key::Name("Enter".into())));
        assert_eq!(key_name(Keysym::q, false, false, Some("q")), Some(Key::Text("q".into())));
        assert_eq!(key_name(Keysym::c, true, false, Some("\u{3}")), Some(Key::Name("C-c".into())));
        assert_eq!(key_name(Keysym::Up, false, true, None), Some(Key::Name("M-Up".into())));
    }

    #[test]
    fn the_cube_and_the_greys() {
        let sixteen = [[0.0, 0.0, 0.0, 1.0]; 16];
        assert_eq!(xterm(16, &sixteen), [0.0, 0.0, 0.0, 1.0]);
        assert_eq!(xterm(231, &sixteen), [1.0, 1.0, 1.0, 1.0]);
        assert!((xterm(232, &sixteen)[0] - 8.0 / 255.0).abs() < 1e-6);
    }

    #[test]
    fn the_hook_says_status_time_and_line() {
        assert_eq!(parse_status("1 100 112 cargo build --release\n"), Some((1, 12, "cargo build --release".into())));
        assert_eq!(parse_status("0 5 5 \n"), Some((0, 0, String::new())));
        assert_eq!(parse_status(""), None);
    }

    #[test]
    fn a_find_marks_each_place() {
        let lines = ["error: one", "fine", "ERROR again, error"];
        assert_eq!(find_in(&lines, "error"), vec![(0, 0, 5), (2, 0, 5), (2, 13, 5)]);
        assert_eq!(find_in(&lines, "ERROR"), vec![(2, 0, 5)]);
        assert!(find_in(&lines, "").is_empty());
    }

    #[test]
    fn the_history_is_searched_by_every_word() {
        assert!(fuzzy("rebuild", "sudo nixos-rebuild switch").is_some());
        assert!(fuzzy("nix sw", "sudo nixos-rebuild switch").is_some());
        assert!(fuzzy("nrs", "nixos-rebuild switch").is_some());
        assert!(fuzzy("zzz", "nixos-rebuild switch").is_none());
        // (A piece beats loose letters; small letters find capitals.)
        assert!(fuzzy("git", "git status") > fuzzy("git", "grep -i thing"));
        assert!(fuzzy("readme", "cat README.md").is_some());
        assert!(fuzzy("README", "cat readme.md").is_none());
        let on = marks("nix sw", "nixos switch", true);
        assert_eq!(on.iter().filter(|m| **m).count(), 5);
        assert!(on[0] && on[1] && on[2] && on[6] && on[7]);
        assert_eq!(marks("git s", "git status", false), [true, true, true, true, true, false, false, false, false, false]);
    }

    #[test]
    fn the_history_file_reads_as_commands() {
        let zsh = b": 1793750145:0;btop\n: 1793750173:0;claude code\n: 1793750200:0;echo a \\\nb\n: 1793750300:0;ls\n";
        assert_eq!(
            parse_history(zsh),
            vec![(1793750145, "btop".into()), (1793750173, "claude code".into()), (1793750300, "ls".into())]
        );
        // (zsh's metafied bytes: "ñ" is c3 b1, b1 written as 83 91.)
        assert_eq!(parse_history(b": 1:0;echo \xc3\x83\x91\n"), vec![(1, "echo ñ".into())]);
        assert_eq!(parse_history(b"#1700000000\ngit pull\nls -la\n"), vec![(1700000000, "git pull".into()), (0, "ls -la".into())]);
    }

    #[test]
    fn completions_are_programs_then_paths() {
        let dir = std::env::temp_dir().join(format!("term-comp-{}", std::process::id()));
        std::fs::create_dir_all(dir.join("src/bin")).unwrap();
        std::fs::create_dir_all(dir.join("My Docs")).unwrap();
        std::fs::write(dir.join("setup.sh"), "").unwrap();
        std::fs::write(dir.join(".hidden"), "").unwrap();
        let d = dir.to_string_lossy();
        let progs: Vec<String> = ["nix", "nix-store", "nixos-rebuild", "nl"].map(String::from).to_vec();
        let texts = |c: Vec<Comp>| c.into_iter().map(|c| c.text).collect::<Vec<_>>();
        assert_eq!(texts(complete("nix", &d, &progs, false)), ["nix-store ", "nixos-rebuild "]);
        assert_eq!(texts(complete("sudo nixo", &d, &progs, false)), ["sudo nixos-rebuild "]);
        assert_eq!(texts(complete("ls s", &d, &progs, false)), ["ls src/", "ls setup.sh "]);
        assert_eq!(texts(complete("cd src/", &d, &progs, false)), ["cd src/bin/"]);
        assert_eq!(texts(complete("cd my", &d, &progs, false)), ["cd My\\ Docs/"]);
        // (No word yet: nothing, but for Tab; the hidden ones only asked.)
        assert!(complete("ls ", &d, &progs, false).is_empty());
        assert_eq!(texts(complete("ls ", &d, &progs, true)), ["ls My\\ Docs/", "ls src/", "ls setup.sh "]);
        assert_eq!(texts(complete("cat .h", &d, &progs, false)), ["cat .hidden "]);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_script_in_the_tabs_folder_runs_as_itself() {
        let dir = std::env::temp_dir().join(format!("term-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("configure"), "#!/bin/sh\n").unwrap();
        let d = dir.to_string_lossy();
        assert_eq!(as_script("configure --prefix=/x", &d), "./configure --prefix=/x");
        assert_eq!(as_script("ls -la", &d), "ls -la");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
