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
}

/// A tab: tmux's window `n` and its pane.
#[derive(Clone, Debug, Default, PartialEq)]
pub(crate) struct Tab {
    pub n: u32,
    pub pane: String,
    pub command: String,
    pub path: String,
}

impl Tab {
    /// What its word says after the number: what runs in it, or — at the
    /// shell's prompt — its folder.
    pub(crate) fn name(&self) -> String {
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

/// What the box asks the engine's thread.
enum Req {
    /// The server up (made if it is not), the watcher on, the tabs read.
    Start { cols: u16, rows: u16 },
    Tabs,
    Capture { tab: u32, pane: String },
    /// Keys for tab `tab` (made first when it is not there yet, in `dir`).
    Keys { tab: u32, keys: Vec<Key>, dir: Option<String> },
    Resize { cols: u16, rows: u16 },
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
}

/// The terminal's state on the loop.
#[derive(Default)]
pub(crate) struct TermState {
    to_engine: Option<mpsc::Sender<Req>>,
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
}

/// What `./…` asks: a tab (none: the one shown) and a command (none: show it).
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Ask {
    pub tab: Option<u32>,
    pub cmd: String,
}

/// `./2 sudo x` → tab 2, "sudo x"; `./2` → tab 2; `./ls -l` → "ls -l".
pub(crate) fn parse(query: &str) -> Option<Ask> {
    let rest = query.strip_prefix("./")?;
    let digits: String = rest.chars().take_while(char::is_ascii_digit).collect();
    if !digits.is_empty() {
        let after = &rest[digits.len()..];
        if after.is_empty() || after.starts_with(' ') {
            if let Some(n) = digits.parse::<u32>().ok().filter(|n| (1..=99).contains(n)) {
                return Some(Ask { tab: Some(n), cmd: after.trim_start().to_owned() });
            }
        }
    }
    Some(Ask { tab: None, cmd: rest.trim_start().to_owned() })
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

/// The server up: there already, or started as a service of its own.
fn ensure_server(cols: u16, rows: u16) -> Result<(), String> {
    if run(&["has-session", "-t", SESSION]).is_some() {
        return Ok(());
    }
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
    let server = [bin.as_str(), "-L", socket(), "-f", &conf_s, "new-session", "-d", "-s", SESSION, "-c", &home];
    // A user service: its shells get the session's environment (not the
    // dock's) and outlive the dock. (Never from a test rig.)
    let rig = std::env::var_os("GOLEM_RIG").is_some();
    let unit = if rig { Command::new("false").status() } else { Command::new("systemd-run")
        .args(["--user", "--quiet", "--collect", "--unit", UNIT, "-p", "Type=forking", "--description", "Golem: the box's terminal tabs", "--"])
        .args(server)
        .stdin(Stdio::null())
        .status() };
    if unit.is_ok_and(|s| s.success()) && run(&["has-session", "-t", SESSION]).is_some() {
        info!("term: the server is up ({UNIT})");
        return Ok(());
    }
    // (No user manager to ask: started here, without what is the dock's own.)
    warn!("term: no user service, starting tmux directly");
    let mut c = tmux();
    c.args(["-f", &conf_s, "new-session", "-d", "-s", SESSION, "-c", &home]);
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
    let out = run(&["list-windows", "-t", SESSION, "-F", "#{window_index}\t#{pane_id}\t#{pane_current_command}\t#{pane_current_path}"])?;
    Some(
        out.lines()
            .filter_map(|l| {
                let mut f = l.split('\t');
                let n = f.next()?.parse().ok()?;
                Some(Tab {
                    n,
                    pane: f.next()?.to_owned(),
                    command: f.next().unwrap_or("").to_owned(),
                    path: f.next().unwrap_or("").to_owned(),
                })
            })
            .collect(),
    )
}

/// What tab `tab` shows now (its pane `pane`).
fn read_screen(tab: u32, pane: &str) -> Option<Screen> {
    let info = "#{cursor_x}\t#{cursor_y}\t#{alternate_on}\t#{pane_current_command}";
    let out = run(&["capture-pane", "-p", "-e", "-N", "-t", pane, ";", "display-message", "-p", "-t", pane, info])?;
    let mut lines: Vec<&str> = out.lines().collect();
    let last = lines.pop()?;
    let mut f = last.split('\t');
    let x = f.next()?.parse().ok()?;
    let y = f.next()?.parse().ok()?;
    let alternate = f.next() == Some("1");
    Some(Screen {
        tab,
        lines: lines.iter().map(|l| parse_line(l)).collect(),
        cursor: Some((x, y)),
        alternate,
        command: f.next().unwrap_or("").to_owned(),
    })
}

/// Keys for a tab, made first when it is not there.
fn send_keys(tab: u32, keys: &[Key], dir: Option<&str>) {
    let target = format!("{SESSION}:{tab}");
    if run(&["list-panes", "-t", &target]).is_none() {
        let home = std::env::var("HOME").unwrap_or_else(|_| "/".into());
        let dir = dir.filter(|d| Path::new(d).is_dir()).unwrap_or(&home);
        if run(&["new-window", "-d", "-t", &target, "-c", dir]).is_none() {
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
                    let _ = to_loop.send(Ev::Tabs(tabs));
                }
            }
            Req::Capture { tab, pane } => {
                let screen = read_screen(tab, &pane).unwrap_or(Screen { tab, ..Default::default() });
                let _ = to_loop.send(Ev::Screen(screen));
            }
            Req::Keys { tab, keys, dir } => {
                send_keys(tab, &keys, dir.as_deref());
                if let Some(tabs) = read_tabs() {
                    let _ = to_loop.send(Ev::Tabs(tabs));
                }
            }
            Req::Resize { cols, rows } => resize_all(cols, rows),
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
            let (cols, rows) = self.term.size;
            if let Some(tx) = &self.term.to_engine {
                let _ = tx.send(Req::Start { cols: cols.max(20), rows: rows.max(5) });
            }
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
        if self.term.to_engine.is_none() {
            // (The size first: a tab made at another size wraps its first
            // lines there.)
            if self.term.size == (0, 0) {
                let layout = self.layout_at_rest(self.ui.extent_of(crate::state::Target::Open));
                self.term_fit(&layout);
            }
            self.term_ask(Req::Tabs);
            return;
        }
        self.term_capture();
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
        self.term_ask(Req::Capture { tab: shown, pane });
    }

    fn term_event(&mut self, ev: Ev) {
        match ev {
            Ev::Tabs(tabs) => {
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
                    self.term.screen = Some(screen);
                    self.schedule_frame();
                }
                if std::mem::take(&mut self.term.again) {
                    self.term_capture();
                }
            }
            Ev::Printed(pane) => {
                if self.term_on() && self.term_tab(self.term_shown()).is_some_and(|t| t.pane == pane) {
                    self.term_capture();
                }
            }
            Ev::Changed => self.term_ask(Req::Tabs),
            Ev::Failed => self.term.failed = true,
            Ev::Gone(why) => {
                info!("term: {why}");
                self.term.to_engine = None;
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
        match keysym {
            Keysym::Return | Keysym::KP_Enter => {
                self.term_enter();
                true
            }
            // Ctrl+C and Ctrl+D with nothing typed after `./N`: the tab's.
            _ if ctrl && matches!(keysym.key_char(), Some('c' | 'C' | 'd' | 'D')) && parse(&self.search.query).is_some_and(|a| a.cmd.is_empty()) => {
                let c = keysym.key_char().map_or('c', |c| c.to_ascii_lowercase());
                self.term_ask(Req::Keys { tab: shown, keys: vec![Key::Name(format!("C-{c}"))], dir: None });
                true
            }
            // (Tab completes, later; it never walks the box meanwhile.)
            Keysym::Tab | Keysym::ISO_Left_Tab | Keysym::Up | Keysym::Down => true,
            _ => false,
        }
    }

    /// Enter: the command to its tab (made when it is new), the tab shown,
    /// the field back to `./`.
    fn term_enter(&mut self) {
        let Some(ask) = parse(&self.search.query) else {
            return;
        };
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
        self.term.current = tab;
        self.term_ask(Req::Keys { tab, keys, dir });
        self.search.query = "./".into();
        self.refilter();
    }

    /// What the field shows: what is typed, or — a password asked — its
    /// dots after the `./N`.
    pub(crate) fn term_field_text(&self) -> Option<String> {
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
            _ => {}
        }
        let sc = self.term.screen.as_ref();
        format!(
            "term: shown {tab}, tabs {:?}, size {:?}, cell {:.1}, screen {} lines alt={} raw={} secret={} cursor={:?}",
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

    /// The words of the tabs for the field's row: "1 launcher", "2 Golem";
    /// a tab named but not made yet stands as "3 new". With the one lit.
    pub(crate) fn term_words(&self) -> (Vec<String>, usize) {
        let shown = self.term_shown();
        let mut words: Vec<(u32, String)> = self.term.tabs.iter().map(|t| (t.n, format!("{} {}", t.n, t.name()))).collect();
        if !words.iter().any(|(n, _)| *n == shown) {
            words.push((shown, format!("{shown} {}", crate::i18n::tr("new"))));
            words.sort_by_key(|(n, _)| *n);
        }
        let lit = words.iter().position(|(n, _)| *n == shown).unwrap_or(0);
        (words.into_iter().map(|(_, w)| w).collect(), lit)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn what_is_typed_names_a_tab_and_a_command() {
        assert_eq!(parse("./2 sudo nixos-rebuild switch"), Some(Ask { tab: Some(2), cmd: "sudo nixos-rebuild switch".into() }));
        assert_eq!(parse("./2"), Some(Ask { tab: Some(2), cmd: String::new() }));
        assert_eq!(parse("./ls -la"), Some(Ask { tab: None, cmd: "ls -la".into() }));
        assert_eq!(parse("./"), Some(Ask { tab: None, cmd: String::new() }));
        // (A number that is part of a word is the command's.)
        assert_eq!(parse("./2to3 x.py"), Some(Ask { tab: None, cmd: "2to3 x.py".into() }));
        assert_eq!(parse("ls"), None);
        assert_eq!(parse("./0 x"), Some(Ask { tab: None, cmd: "0 x".into() }));
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
        let at = |cmd: &str, path: &str| Tab { n: 1, pane: "%1".into(), command: cmd.into(), path: path.into() }.name();
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
