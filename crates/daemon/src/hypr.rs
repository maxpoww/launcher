//! Hyprland IPC integration for "intellihide": the dock stays visible
//! while no window overlaps its zone and gets out of the way when one
//! does (macOS dodge-windows behavior).
//!
//! Wayland gives clients no view of other windows' geometry, so this
//! is compositor-specific by necessity (the plan's risk register
//! blesses Hyprland IPC as the fallback). Everything degrades
//! gracefully: without the Hyprland sockets the daemon behaves exactly
//! as before (always auto-hide).
//!
//! Hyprland emits no event for float toggles or interactive float
//! moves/resizes (verified on socket2), so the daemon combines events
//! (instant) with a steady poll (the only reliable signal for the
//! silent transitions).

use std::io::{Read, Write};
use std::os::unix::net::UnixStream;
use std::path::PathBuf;

use anyhow::{anyhow, Context};
use calloop::generic::Generic;
use calloop::{Interest, LoopHandle, Mode, PostAction};
use tracing::{debug, warn};

use crate::App;

/// Events that can change whether a window overlaps the dock zone.
/// Prefix-matched, so `movewindowv2`, `workspacev2` etc. count too.
/// Events after which the screen's colour matrix may have been thrown away
/// under us — the output's DRM state is rebuilt and the CTM goes with it.
/// They are deliberately NOT in [`RELEVANT`]: this costs one idempotent
/// `hyprctl` call, not a layout re-evaluation.
const SCREEN_DRIFT: &[&str] = &["monitoradded", "monitorremoved", "configreloaded"];

/// Events that change how many tiles a space is divided between — what Golem's
/// solitary-pseudo rule answers to. `movewindow` covers both spaces it touches,
/// since the sweep looks at every workspace anyway.
const LAYOUT_SHAPE: &[&str] = &["openwindow", "closewindow", "movewindow"];

const RELEVANT: &[&str] = &[
    "openwindow",
    "closewindow",
    "movewindow",
    "changefloatingmode",
    "fullscreen",
    "workspace",
    "focusedmon",
    "minimized",
    "pin",
];

fn instance_dir() -> anyhow::Result<PathBuf> {
    let sig = std::env::var_os("HYPRLAND_INSTANCE_SIGNATURE")
        .ok_or_else(|| anyhow!("HYPRLAND_INSTANCE_SIGNATURE not set (not under Hyprland?)"))?;
    let runtime =
        std::env::var_os("XDG_RUNTIME_DIR").ok_or_else(|| anyhow!("XDG_RUNTIME_DIR not set"))?;
    Ok(PathBuf::from(runtime).join("hypr").join(sig))
}

/// One-shot request over Hyprland's control socket (`j/` = JSON reply).
fn request(cmd: &str) -> anyhow::Result<String> {
    let mut stream = UnixStream::connect(instance_dir()?.join(".socket.sock"))
        .context("connecting to Hyprland control socket")?;
    stream.write_all(cmd.as_bytes())?;
    let mut out = String::new();
    stream.read_to_string(&mut out)?;
    Ok(out)
}

/// Fire a Hyprland dispatch (this Hyprland's Lua form) over the control
/// socket. Best effort: failures are logged, never fatal — focus
/// niceties must not crash the dock, and everything degrades to plain
/// layer behavior without Hyprland.
pub fn dispatch(lua: &str) {
    match request(&format!("dispatch {lua}")) {
        Ok(reply) if reply.trim() == "ok" => {}
        Ok(reply) => debug!("Hyprland dispatch {lua:?} replied: {}", reply.trim()),
        Err(e) => debug!("Hyprland dispatch {lua:?} failed: {e:#}"),
    }
}

/// [`dispatch`] for state the daemon OWNS — the float rule, anything whose
/// loss degrades silently. A focus nicety that fails is a `debug!`; a lost
/// mode assertion left a whole morning's windows tiled under a floating
/// store with nothing in the log to say why (2026-09-15). Same best-effort
/// contract, but the failure is said out loud and handed back.
pub fn dispatch_checked(lua: &str) -> bool {
    match request(&format!("dispatch {lua}")) {
        Ok(reply) if reply.trim() == "ok" => true,
        Ok(reply) => {
            warn!("Hyprland dispatch {lua:?} replied: {}", reply.trim());
            false
        }
        Err(e) => {
            warn!("Hyprland dispatch {lua:?} failed: {e:#}");
            false
        }
    }
}

/// Golem's pseudo size, as a fraction of the window's tile: the
/// "frame inset" look Max picked live (2026-08-31) — proportional, so it
/// reads the same on every screen and inside any split.
const PSEUDO_W: f64 = 0.89;
const PSEUDO_H: f64 = 0.84;
/// The tag that marks a Golem-pseudo window. It is BOTH the state (read
/// back from `clients` JSON) and the match key for hyprland.lua's frame
/// rule, which restores rounding + border under smart gaps.
const PSEUDO_TAG: &str = "golem-pseudo";

/// Golem's floating size, as a fraction of the MONITOR — the float mode's
/// answer to [`PSEUDO_W`]/[`PSEUDO_H`], and proportional for the same reason:
/// it should read the same on every screen rather than being a pixel count
/// that happens to suit one.
///
/// Taken from the window Max pointed at (2026-09-04): 1097×677 on a 2000×1250
/// logical output. Big enough to work in, unmistakably not a tile.
const FLOAT_W: f64 = 0.55;
const FLOAT_H: f64 = 0.54;

/// How a window is laid out. **Exactly one of these holds at a time** — the
/// four are mutually exclusive, and picking one drops whatever was on.
///
/// The compositor does not see it that way: there, floating, pseudo and
/// fullscreen are independent flags that can overlap (a floating window can be
/// fullscreened; pseudo is a tiled sub-mode that quietly does nothing while
/// fullscreen). That orthogonality is expressive and hard to hold in your head
/// — you can be in states that have no name and no obvious way out. The bar
/// presents the four modes a window can actually *be in*, and switching to one
/// is a single act that leaves the previous one behind.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WindowMode {
    /// In the layout, filling its tile. The state everything falls back to.
    Tiled,
    /// Out of the layout, free-floating.
    Floating,
    /// In the layout, but drawn smaller than its tile (Golem policy:
    /// [`PSEUDO_W`]×[`PSEUDO_H`] of it).
    Pseudo,
    /// Covering the output.
    Fullscreen,
}

/// The last tile size seen for a window, by address.
///
/// Golem's pseudo size is a fraction of the window's TILE, and a window that is
/// fullscreen or floating cannot report its tile — it reports the output or the
/// float. Measuring it afterwards costs a second beat the user can see.
///
/// So it is remembered on the way past instead: every mode change reads the
/// window, and any time one is found tiled its size is kept here. Going
/// fullscreen and then straight to pseudo therefore needs no measuring, because
/// the trip into fullscreen already went through this function.
static LAST_TILE: std::sync::Mutex<Option<(String, f64, f64)>> = std::sync::Mutex::new(None);

/// Remember a window's tile, so a later pseudo can be sized without measuring.
fn note_tile(addr: &str, json: &serde_json::Value) {
    let (Some(w), Some(h)) = (json["size"][0].as_f64(), json["size"][1].as_f64()) else {
        return;
    };
    if w < 1.0 || h < 1.0 {
        return;
    }
    if let Ok(mut slot) = LAST_TILE.lock() {
        *slot = Some((addr.to_owned(), w, h));
    }
}

/// The remembered tile for `addr`, if the last one seen was this window's.
fn remembered_tile(addr: &str) -> Option<(f64, f64)> {
    let slot = LAST_TILE.lock().ok()?;
    match slot.as_ref() {
        Some((a, w, h)) if a == addr => Some((*w, *h)),
        _ => None,
    }
}

/// Read a mode out of an `activewindow`/`clients` entry.
///
/// Precedence matters where the compositor's flags overlap: fullscreen wins
/// because it is what you can see, then floating, then pseudo. A window with
/// no flag set is tiled.
///
/// Pseudo is read from a TAG rather than from the window: the Lua
/// `window.tags` field reads as an empty table even for a tagged window
/// (verified 2026-08-31) and pseudo state is exposed nowhere at all, so this
/// side is the only place that can tell "on" from "off". That is also why the
/// whole mode policy lives here rather than in `hyprland.lua`.
fn mode_of(json: &serde_json::Value) -> WindowMode {
    if json["fullscreen"].as_i64().unwrap_or(0) != 0 {
        WindowMode::Fullscreen
    } else if json["floating"].as_bool().unwrap_or(false) {
        WindowMode::Floating
    } else if json["tags"]
        .as_array()
        .is_some_and(|t| t.iter().any(|v| v.as_str() == Some(PSEUDO_TAG)))
    {
        WindowMode::Pseudo
    } else {
        WindowMode::Tiled
    }
}

/// One window, as the LAYOUT rules see it: where it is, what it is, and the
/// rectangle a pseudo would be a fraction of.
pub struct LayoutWindow {
    pub address: String,
    pub workspace: i64,
    pub mode: WindowMode,
}

impl LayoutWindow {
    /// Whether this window is a **tile** — one of the things sharing the
    /// workspace's space. Floating windows are over the layout rather than in
    /// it, and a fullscreen one is a deliberate act that owns the whole output;
    /// neither is a tile, and neither should be counted when asking "how many
    /// windows is this space divided between".
    pub fn is_tile(&self) -> bool {
        matches!(self.mode, WindowMode::Tiled | WindowMode::Pseudo)
    }
}

/// Every mapped window as the layout sees it — one `j/clients` read, so a rule
/// that has to look at a whole workspace at once costs one round trip rather
/// than one per window.
pub fn layout_windows() -> Vec<LayoutWindow> {
    let Ok(raw) = request("j/clients") else {
        return Vec::new();
    };
    let Ok(clients) = serde_json::from_str::<serde_json::Value>(&raw) else {
        return Vec::new();
    };
    clients
        .as_array()
        .into_iter()
        .flatten()
        .filter(|c| {
            c["mapped"].as_bool().unwrap_or(false) && !c["hidden"].as_bool().unwrap_or(false)
        })
        .filter_map(|c| {
            Some(LayoutWindow {
                address: c["address"].as_str()?.to_owned(),
                workspace: c["workspace"]["id"].as_i64().unwrap_or(0),
                mode: mode_of(c),
            })
        })
        .collect()
}

/// The rectangle a window ALONE on a workspace fills: the output minus what the
/// layer surfaces have reserved, and nothing else.
///
/// Computed rather than measured, and that is the whole point: measuring means
/// waiting for the window to finish animating into its tile before the pseudo
/// can be sized from it, which is exactly the half-second of "it opens, then it
/// shrinks" the rule is not allowed to have. The value is knowable in advance —
/// a lone tile takes the whole usable output, because the smart-gaps rule
/// (`w[tv1]`, `/etc/nixos/hyprland.lua`) zeroes its gaps for this very case.
/// Measured against it: `[0,28] 2000×1222` on a 2000×1250 output with a 28px bar.
///
/// Reads the FOCUSED monitor: a window opening unfocused on a second output
/// would be sized from the wrong one, which is a real limit and not one this
/// machine can hit.
pub fn solitary_tile() -> Option<(f64, f64)> {
    let m = focused_monitor().ok()?;
    let (l, t, r, b) = m.reserved;
    let (w, h) = (m.w - l - r, m.h - t - b);
    (w > 1.0 && h > 1.0).then_some((w, h))
}

/// Pseudotile one window at Golem's fraction of `tile`.
///
/// [`set_window_mode`] is about the window you are looking at; this is for the
/// ones the LAYOUT decides for (see `App::sync_solitary_pseudo`). It is handed
/// the tile rather than reading one, so it can act the instant a window maps —
/// while the window is still animating in, which is what makes it look like it
/// opened this way rather than settled into it.
pub fn pseudo_on(addr: &str, tile: (f64, f64)) {
    let (w, h) = tile;
    if w < 1.0 || h < 1.0 {
        return;
    }
    let lua = format!(
        "hl.dispatch(hl.dsp.window.tag({{ tag = \"+{PSEUDO_TAG}\", window = \"address:{addr}\" }})) \
         hl.dispatch(hl.dsp.window.pseudo({{ action = \"on\", window = \"address:{addr}\" }})) \
         hl.dispatch(hl.dsp.window.resize({{ x = {}, y = {}, window = \"address:{addr}\" }})) ",
        (w * PSEUDO_W) as i64,
        (h * PSEUDO_H) as i64,
    );
    if let Err(e) = request(&format!("eval {lua}")) {
        debug!("pseudo_on({addr}) failed: {e:#}");
    }
}

/// Hand one window back to the plain layout.
pub fn pseudo_off(addr: &str) {
    let lua = format!(
        "hl.dispatch(hl.dsp.window.tag({{ tag = \"-{PSEUDO_TAG}\", window = \"address:{addr}\" }})) \
         hl.dispatch(hl.dsp.window.pseudo({{ action = \"off\", window = \"address:{addr}\" }})) "
    );
    if let Err(e) = request(&format!("eval {lua}")) {
        debug!("pseudo_off({addr}) failed: {e:#}");
    }
}

/// Which of the four modes the focused window is in, or `None` when there is no
/// focused window.
///
/// The bar shows this (the state pill beside the close), so it is read rather
/// than remembered: a window can be sent fullscreen by the client itself — a
/// video going full-screen, a game starting — with nothing passing through
/// [`set_window_mode`] to notice.
/// Returns the window's ADDRESS with its mode: a mode means nothing without
/// knowing whose it is. The caller compares addresses to tell "this window just
/// changed" from "focus landed on a different window", which decide very
/// different things.
pub fn active_window_mode() -> Option<(String, WindowMode)> {
    let raw = request("j/activewindow").ok()?;
    let json = serde_json::from_str::<serde_json::Value>(&raw).ok()?;
    let addr = json["address"]
        .as_str()
        .filter(|a| !a.is_empty() && *a != "0x0")?
        .to_owned();
    Some((addr, mode_of(&json)))
}

/// Put the focused window into `target`, leaving whatever mode it was in.
///
/// Asking for the mode it is already in returns it to [`WindowMode::Tiled`], so
/// every control on the bar stays a toggle: press to enter, press again to come
/// back to the layout.
///
/// Everything that can be done at once is emitted as ONE eval chunk, so the
/// window never passes through a visible intermediate state. The exception is
/// entering pseudo from fullscreen or floating: the Golem pseudo size is a
/// fraction of the window's TILE, and the tile size is not knowable until the
/// window is back in the layout. That case returns `true` to ask for a second
/// pass once it has settled — the honest two-beat, rather than pseudotiling to
/// a fraction of the wrong rectangle.
pub fn set_window_mode(target: WindowMode) -> bool {
    let Ok(raw) = request("j/activewindow") else {
        return false;
    };
    let Ok(json) = serde_json::from_str::<serde_json::Value>(&raw) else {
        return false;
    };
    let Some(addr) = json["address"]
        .as_str()
        .filter(|a| !a.is_empty() && *a != "0x0")
    else {
        return false;
    };
    let current = mode_of(&json);
    // Seen in the layout: remember its tile, so a pseudo asked for later —
    // from fullscreen, from floating — needs no second beat to measure one.
    if current == WindowMode::Tiled {
        note_tile(addr, &json);
    }
    // Pressing the mode you are already in is how you get out of it.
    let target = if current == target {
        WindowMode::Tiled
    } else {
        target
    };
    if current == target {
        return false;
    }
    let win = format!("window = \"address:{addr}\"");
    let mut lua = String::new();
    // Leave the old mode first, so the window is back in the layout before the
    // new one is applied to it.
    match current {
        WindowMode::Pseudo => {
            lua.push_str(&format!(
                "hl.dispatch(hl.dsp.window.tag({{ tag = \"-{PSEUDO_TAG}\", {win} }})) \
                 hl.dispatch(hl.dsp.window.pseudo({{ action = \"off\", {win} }})) "
            ));
        }
        WindowMode::Floating => {
            lua.push_str(&format!(
                "hl.dispatch(hl.dsp.window.float({{ action = \"toggle\", {win} }})) "
            ));
        }
        WindowMode::Fullscreen => {
            lua.push_str(&format!(
                "hl.dispatch(hl.dsp.window.fullscreen({{ action = \"toggle\", {win} }})) "
            ));
        }
        WindowMode::Tiled => {}
    }
    // Then enter the new one.
    let mut needs_second_pass = false;
    match target {
        WindowMode::Tiled => {}
        WindowMode::Floating => {
            lua.push_str(&format!(
                "hl.dispatch(hl.dsp.window.float({{ action = \"toggle\", {win} }})) "
            ));
            // A float gets Golem's size AND Golem's place, the way a pseudo
            // gets Golem's size — so "float this" means one predictable shape
            // in one predictable spot every time, instead of whatever geometry
            // the window last happened to hold, wherever its tile happened to
            // be. Centred after the resize so it centres the final size, and
            // all in the same chunk as the toggle, so the window arrives
            // floated, sized and placed in one motion (`OptionUXRules.md` §6).
            //
            // `center` respects the reserved area, so a centred float sits in
            // the usable region rather than half under the bar.
            if let Some((w, h)) = float_size() {
                lua.push_str(&format!(
                    "hl.dispatch(hl.dsp.window.resize({{ x = {w}, y = {h}, {win} }})) "
                ));
            }
            lua.push_str(&format!("hl.dispatch(hl.dsp.window.center({{ {win} }})) "));
        }
        WindowMode::Fullscreen => {
            lua.push_str(&format!(
                "hl.dispatch(hl.dsp.window.fullscreen({{ action = \"toggle\", {win} }})) "
            ));
        }
        WindowMode::Pseudo => match pseudo_lua(&json, addr, current) {
            Some(chunk) => lua.push_str(&chunk),
            // Coming from fullscreen/floating: the tile size is still unknown.
            // Leave the window tiled now and ask to be called again.
            None => needs_second_pass = true,
        },
    }
    if !lua.is_empty() {
        if let Err(e) = request(&format!("eval {lua}")) {
            debug!("window mode -> {target:?} failed: {e:#}");
        }
    }
    needs_second_pass
}

/// Golem's floating size in logical pixels, for the focused monitor.
///
/// Read per call rather than cached: the answer changes with the output, and a
/// float toggle is far too rare for one `j/monitors` read to matter.
fn float_size() -> Option<(i64, i64)> {
    let m = focused_monitor().ok()?;
    let (w, h) = ((m.w * FLOAT_W) as i64, (m.h * FLOAT_H) as i64);
    (w > 1 && h > 1).then_some((w, h))
}

/// The chunk that pseudotiles a window at Golem's fraction of its tile.
///
/// The window's own reported size is only the tile when it is *in* the layout;
/// fullscreen it reports the output and floating it reports the float, and
/// sizing off either would shrink the window to a fraction of the wrong
/// rectangle. For those, the tile remembered on the way past is used instead,
/// so the whole transition still lands in one motion.
///
/// `None` only when this window has never been seen tiled — it reached
/// fullscreen without passing through here — and there is genuinely nothing to
/// measure against yet.
fn pseudo_lua(json: &serde_json::Value, addr: &str, current: WindowMode) -> Option<String> {
    let (w, h) = if matches!(current, WindowMode::Tiled) {
        (
            json["size"][0].as_f64().unwrap_or(0.0),
            json["size"][1].as_f64().unwrap_or(0.0),
        )
    } else {
        remembered_tile(addr)?
    };
    if w < 1.0 || h < 1.0 {
        return None;
    }
    Some(format!(
        "hl.dispatch(hl.dsp.window.tag({{ tag = \"+{PSEUDO_TAG}\", window = \"address:{addr}\" }})) \
         hl.dispatch(hl.dsp.window.pseudo({{ action = \"on\", window = \"address:{addr}\" }})) \
         hl.dispatch(hl.dsp.window.resize({{ x = {}, y = {}, window = \"address:{addr}\" }})) ",
        (w * PSEUDO_W) as i64,
        (h * PSEUDO_H) as i64,
    ))
}

/// Put a message on the COMPOSITOR's own notification OSD.
///
/// Deliberately not our own notification stack: waverunner IS the surface
/// that draws those, so when the renderer is the thing that failed, a
/// notification routed through us would be invisible — the exact silence
/// F8 is about. Hyprland draws this one itself.
pub fn notify_user(msg: &str) {
    // `notify <icon> <ms> <colour> <message>`; icon 3 = error, colour 0 =
    // the theme's default.
    let msg = msg.replace('\n', " ");
    if let Err(e) = request(&format!("notify 3 8000 0 {msg}")) {
        debug!("compositor notify failed: {e:#}");
    }
}

/// Close the waveview overview (its Lua toggle, which closes when open) —
/// the topbar's X while the overview owns the screen.
pub fn close_overview() {
    // `close`, not `toggle`. The plugin shuts the map itself when it is told the
    // stage is opening, so this call lands on an overview that is already on its
    // way out — and a toggle there opened it straight back up, which is why
    // entering the stage from the map used to leave the map on screen. A close
    // that is a close is idempotent, and two of them are still one close.
    if let Err(e) = request("eval hl.plugin.waveview.close()") {
        debug!("overview close failed: {e:#}");
    }
}

/// Close a window by address (best effort) — used when uninstalling an app
/// that's still open, so it goes away with its package instead of lingering
/// on screen with nothing behind it.
pub fn close_window(addr: &str) {
    dispatch(&format!(
        "hl.dsp.window.close({{ window = \"address:{addr}\" }})"
    ));
}

/// Address (`0x…`) of the currently-focused window, if any.
pub fn active_window() -> Option<String> {
    let json: serde_json::Value = serde_json::from_str(&request("j/activewindow").ok()?).ok()?;
    json["address"].as_str().map(str::to_owned)
}

/// The focused window's address and the workspace it is on, in one read — what
/// the per-workspace focus memory records (see [`crate::focus_cycle`]).
pub fn active_focus() -> Option<(String, i64)> {
    let json: serde_json::Value = serde_json::from_str(&request("j/activewindow").ok()?).ok()?;
    let addr = json["address"]
        .as_str()
        .filter(|a| !a.is_empty() && *a != "0x0")?
        .to_owned();
    Some((addr, json["workspace"]["id"].as_i64()?))
}

/// The window on `ws` the compositor focused most recently, by
/// `focusHistoryID` (0 = current). The fallback for a space we hold no note of
/// ourselves — trustworthy precisely when nothing is focused, because then
/// nothing has arrived to displace the history.
pub fn last_focused_on(ws: i64) -> Option<String> {
    let json: serde_json::Value = serde_json::from_str(&request("j/clients").ok()?).ok()?;
    json.as_array()?
        .iter()
        .filter(|c| c["workspace"]["id"].as_i64() == Some(ws))
        .filter(|c| c["mapped"].as_bool().unwrap_or(true))
        .min_by_key(|c| c["focusHistoryID"].as_i64().unwrap_or(i64::MAX))
        .and_then(|c| c["address"].as_str())
        .map(str::to_owned)
}

/// Whether `addr` is a live window on workspace `ws` — the check before
/// restoring focus to a remembered window that may since have been closed or
/// dragged somewhere else.
pub fn window_is_on(addr: &str, ws: i64) -> bool {
    let Ok(raw) = request("j/clients") else {
        return false;
    };
    let Ok(json) = serde_json::from_str::<serde_json::Value>(&raw) else {
        return false;
    };
    json.as_array().is_some_and(|cs| {
        cs.iter().any(|c| {
            c["address"].as_str() == Some(addr) && c["workspace"]["id"].as_i64() == Some(ws)
        })
    })
}

/// The focused window's address, title, and whether it's fullscreen — for the
/// OPTIONS window pill and the fullscreen auto-hide. `None` when nothing is
/// focused (empty workspace).
pub fn active_window_info() -> Option<(String, String, bool)> {
    let json: serde_json::Value = serde_json::from_str(&request("j/activewindow").ok()?).ok()?;
    let address = json["address"].as_str()?.to_owned();
    if address.is_empty() || address == "0x0" {
        return None;
    }
    let title = json["title"].as_str().unwrap_or("").to_owned();
    // `fullscreen`: 0 none, 1 maximized (respects the reserved bar), 2 true
    // fullscreen (covers the whole output, including the bar) — only the latter
    // should trigger the auto-hide.
    let fullscreen = json["fullscreen"].as_i64().unwrap_or(0) >= 2;
    Some((address, title, fullscreen))
}

/// The focused window's app class and title — the clipboard OPTION's
/// "copied/pasted from where" metadata. `None` when nothing is focused.
pub fn active_window_where() -> Option<(String, String)> {
    let json: serde_json::Value = serde_json::from_str(&request("j/activewindow").ok()?).ok()?;
    let address = json["address"].as_str()?;
    if address.is_empty() || address == "0x0" {
        return None;
    }
    let class = json["class"].as_str().unwrap_or("").to_owned();
    let title = json["title"].as_str().unwrap_or("").to_owned();
    if class.is_empty() && title.is_empty() {
        return None;
    }
    Some((class, title))
}

/// One mapped window's focus-cycle inputs (see `crate::focus_cycle`).
pub struct WsWindow {
    pub addr: String,
    /// Workspace id (special workspaces are negative).
    pub workspace: i32,
    /// Compositor focus recency (0 = focused, 1 = previous, …).
    pub history: i32,
}

/// Every mapped window with its workspace and focus-history rank, plus the
/// focused window's address — one `clients` read, the focus cycle's
/// snapshot.
pub fn workspace_windows() -> Option<(Vec<WsWindow>, Option<String>)> {
    let json: serde_json::Value = serde_json::from_str(&request("j/clients").ok()?).ok()?;
    let mut windows = Vec::new();
    let mut focused = None;
    for w in json.as_array()? {
        if !w["mapped"].as_bool().unwrap_or(false) {
            continue;
        }
        let Some(addr) = w["address"].as_str() else {
            continue;
        };
        let history = w["focusHistoryID"].as_i64().unwrap_or(i64::MAX) as i32;
        if history == 0 {
            focused = Some(addr.to_owned());
        }
        windows.push(WsWindow {
            addr: addr.to_owned(),
            workspace: w["workspace"]["id"].as_i64().unwrap_or(0) as i32,
            history,
        });
    }
    Some((windows, focused))
}

/// Focus a window by address, directly and with a checked reply (switching
/// workspaces if it lives elsewhere). Unlike [`focus_window`], no
/// neighbor-bounce: the focus cycle needs exactly one clean focus change
/// (its stats hook suppresses one pending address, and a detour would leak
/// a phantom focus event into the frecency scores).
///
/// The pointer stays where it is: the focus dispatch normally warps the
/// cursor into the window, but a pill-driven cycle must leave the mouse on
/// the pill (Max, 2026-08-31). One atomic Lua chunk flips
/// `cursor.no_warps` around the dispatch — Golem's config never sets it,
/// so restoring to false is restoring the default.
pub fn focus_window_direct(addr: &str) -> anyhow::Result<()> {
    let reply = request(&format!(
        "eval hl.config({{ [\"cursor.no_warps\"] = true }}) \
         hl.dispatch(hl.dsp.focus({{ window = \"address:{addr}\" }})) \
         hl.config({{ [\"cursor.no_warps\"] = false }})"
    ))?;
    if reply.trim() == "ok" {
        Ok(())
    } else {
        anyhow::bail!("focus eval replied: {}", reply.trim())
    }
}

/// The focused window's geometry in logical compositor coordinates
/// (`(x, y, w, h)`) — the space `grim -g` expects — for a window snapshot.
/// `None` if no real window is focused or it reports a zero size.
pub fn active_window_geom() -> Option<(i32, i32, i32, i32)> {
    let json: serde_json::Value = serde_json::from_str(&request("j/activewindow").ok()?).ok()?;
    let address = json["address"].as_str()?;
    if address.is_empty() || address == "0x0" {
        return None;
    }
    let x = json["at"][0].as_i64()? as i32;
    let y = json["at"][1].as_i64()? as i32;
    let w = json["size"][0].as_i64()? as i32;
    let h = json["size"][1].as_i64()? as i32;
    if w <= 0 || h <= 0 {
        return None;
    }
    Some((x, y, w, h))
}

/// Inject a modifier+key chord into the currently focused window via this fork's
/// keystroke dispatcher `hl.dsp.send_shortcut{ mods, key, window }`. Used by the
/// clipboard OPTION (paste / copy / cut / select-all) so a pill click acts where
/// the user is working. Targets the focused window explicitly — clicking the
/// topbar doesn't move keyboard focus, so it's still the app the user was in.
/// `key` should be the lowercase keysym (e.g. `v`) so no implicit Shift is sent.
pub fn send_shortcut_active(mods: &str, key: &str) {
    let Some(addr) = active_window() else {
        debug!("send_shortcut: no active window for {mods}+{key}");
        return;
    };
    dispatch(&format!(
        "hl.dsp.send_shortcut({{ mods = \"{mods}\", key = \"{key}\", window = \"address:{addr}\" }})"
    ));
}

/// Send a shortcut to a specific window, focused or not.
///
/// The escape hatch for players that publish MPRIS and then ignore it. Firefox
/// does exactly that (2026-09-12): it advertises `CanControl: true`, accepts
/// `PlayPause` over D-Bus without error, and keeps playing — and it ignores the
/// `XF86AudioPlay` keysym too. An ordinary key sent to its window does work.
///
/// Targeted by ADDRESS rather than by focusing first: pausing a video should
/// not steal your place, and the window making the sound is usually not the one
/// you are looking at.
pub fn send_key_to(addr: &str, mods: &str, key: &str) {
    dispatch(&format!(
        "hl.dsp.send_shortcut({{ mods = \"{mods}\", key = \"{key}\", window = \"address:{addr}\" }})"
    ));
}

/// Inject a paste (Ctrl+V) into the focused window.
pub fn paste_active() {
    send_shortcut_active("CTRL", "v");
}

/// Give keyboard focus back to a window by address.
///
/// After our layer releases its exclusive keyboard grab the window we
/// opened over is still `activewindow`, so focusing it directly is a
/// no-op that leaves the keyboard seat stranded on the (now dead) layer.
/// The compositor only actually moves the seat when focus *changes*
/// between windows — so we bounce focus through another window on the
/// same workspace and back. The detour is what re-routes the keyboard.
pub fn focus_window(addr: &str) {
    if let Some(other) = same_workspace_neighbor(addr) {
        dispatch(&format!("hl.dsp.focus({{ window = \"address:{other}\" }})"));
    }
    dispatch(&format!("hl.dsp.focus({{ window = \"address:{addr}\" }})"));
}

/// The active workspace's id and its most-recent window address, if any.
pub fn active_workspace() -> Option<(i64, Option<String>)> {
    let ws: serde_json::Value = serde_json::from_str(&request("j/activeworkspace").ok()?).ok()?;
    let id = ws["id"].as_i64()?;
    let last = ws["lastwindow"]
        .as_str()
        .filter(|a| !a.is_empty() && *a != "0x0")
        .map(str::to_owned);
    Some((id, last))
}

/// Whether the ACTIVE workspace holds no windows — a **new** workspace, in
/// Max's words (2026-09-12), and one of the five arrangement states OPTIONS is
/// conditioned on.
///
/// One `activeworkspace` read, which carries its own `windows` count, rather
/// than walking every client and filtering by workspace id. This is deliberately
/// NOT inferred from "nothing is focused": an unfocused floating window leaves
/// the workspace occupied while focus is empty, and the two states want
/// different OPTIONS — a new workspace is an invitation, an unfocused one is
/// just a pause.
///
/// `None` when the compositor cannot be reached; callers keep their last
/// answer rather than claiming the workspace emptied.
pub fn active_workspace_is_empty() -> Option<bool> {
    let ws: serde_json::Value = serde_json::from_str(&request("j/activeworkspace").ok()?).ok()?;
    Some(ws["windows"].as_i64()? == 0)
}

/// Switch to a workspace by id (`hl.dsp.focus` with a `workspace` field —
/// verified in the fork's example config, mainMod+[0-9] binds).
pub fn focus_workspace(id: i64) {
    dispatch(&format!("hl.dsp.focus({{ workspace = {id} }})"));
}

/// Focus the window whose class **exactly** matches `class` (case-insensitive on
/// `class`/`initialClass`), preferring the most-recently-focused. Returns whether
/// one was found and focused. This is how the notification OPTION raises a
/// webapp's own PWA window precisely (`chrome-<host>__-Default`) instead of the
/// plain browser — a `false` return means no such window is open, so the caller
/// can launch the webapp instead.
pub fn focus_exact_class(class: &str) -> bool {
    let Ok(reply) = request("j/clients") else {
        return false;
    };
    let Ok(clients) = serde_json::from_str::<serde_json::Value>(&reply) else {
        return false;
    };
    let empty = Vec::new();
    let windows = clients.as_array().unwrap_or(&empty);
    let mut best: Option<(i64, String)> = None;
    for c in windows {
        let matched = ["class", "initialClass"]
            .iter()
            .any(|f| c[*f].as_str().unwrap_or("").eq_ignore_ascii_case(class));
        if !matched {
            continue;
        }
        let Some(addr) = c["address"].as_str() else {
            continue;
        };
        let fh = c["focusHistoryID"].as_i64().unwrap_or(i64::MAX);
        if best.as_ref().is_none_or(|(bfh, _)| fh < *bfh) {
            best = Some((fh, addr.to_owned()));
        }
    }
    if let Some((_, addr)) = best {
        dispatch(&format!("hl.dsp.focus({{ window = \"address:{addr}\" }})"));
        true
    } else {
        false
    }
}

/// Known browser window classes (lowercased substrings) for [`focus_browser`]
/// and [`is_browser_class`].
const BROWSER_CLASSES: &[&str] = &[
    "firefox",
    "chrome",
    "chromium",
    "brave",
    "edge",
    "vivaldi",
    "opera",
    "librewolf",
    "zen",
];

/// Whether a window's app class is a known browser — the apps whose focused
/// window has a copyable page URL (the clipboard "copy link" affordance).
pub fn is_browser_class(class: &str) -> bool {
    let c = class.to_lowercase();
    BROWSER_CLASSES.iter().any(|b| c.contains(b))
}

/// Raise/focus the most-recently-focused browser window (any known browser
/// class) — used after opening a link so the freshly-loaded tab comes to the
/// front. Returns whether one was found; `false` means no browser is open yet
/// (a cold launch will focus its own new window).
pub fn focus_browser() -> bool {
    let Ok(reply) = request("j/clients") else {
        return false;
    };
    let Ok(clients) = serde_json::from_str::<serde_json::Value>(&reply) else {
        return false;
    };
    let empty = Vec::new();
    let windows = clients.as_array().unwrap_or(&empty);
    let mut best: Option<(i64, String)> = None;
    for c in windows {
        let is_browser = ["class", "initialClass"].iter().any(|f| {
            let v = c[*f].as_str().unwrap_or("").to_lowercase();
            BROWSER_CLASSES.iter().any(|b| v.contains(b))
        });
        if !is_browser {
            continue;
        }
        let Some(addr) = c["address"].as_str() else {
            continue;
        };
        let fh = c["focusHistoryID"].as_i64().unwrap_or(i64::MAX);
        if best.as_ref().is_none_or(|(bfh, _)| fh < *bfh) {
            best = Some((fh, addr.to_owned()));
        }
    }
    if let Some((_, addr)) = best {
        focus_window(&addr);
        true
    } else {
        false
    }
}

/// Address of another mapped window sharing `addr`'s workspace, if any —
/// a detour target for [`focus_window`]. Restricted to the same
/// workspace so the bounce never triggers a workspace switch.
fn same_workspace_neighbor(addr: &str) -> Option<String> {
    let clients: serde_json::Value = serde_json::from_str(&request("j/clients").ok()?).ok()?;
    let arr = clients.as_array()?;
    let ws = arr
        .iter()
        .find(|c| c["address"].as_str() == Some(addr))
        .and_then(|c| c["workspace"]["id"].as_i64())?;
    arr.iter()
        .filter(|c| {
            c["mapped"].as_bool().unwrap_or(false)
                && !c["hidden"].as_bool().unwrap_or(false)
                && c["workspace"]["id"].as_i64() == Some(ws)
        })
        .find_map(|c| {
            let a = c["address"].as_str()?;
            (a != addr).then(|| a.to_owned())
        })
}

/// Subscribe to Hyprland's event socket; relevant events re-evaluate
/// the dock zone via [`App::on_layout_changed`].
pub fn subscribe(handle: &LoopHandle<'static, App>) -> anyhow::Result<()> {
    let stream = UnixStream::connect(instance_dir()?.join(".socket2.sock"))
        .context("connecting to Hyprland event socket")?;
    stream
        .set_nonblocking(true)
        .context("event socket non-blocking")?;

    let mut pending: Vec<u8> = Vec::new();
    handle
        .insert_source(
            Generic::new(stream, Interest::READ, Mode::Level),
            move |_, stream, app: &mut App| {
                let mut relevant = false;
                let mut buf = [0u8; 4096];
                // NoIoDrop only exposes a shared ref; &UnixStream is Read.
                let mut reader: &UnixStream = stream;
                loop {
                    match reader.read(&mut buf) {
                        Ok(0) => {
                            warn!("Hyprland event socket closed; intellihide inactive");
                            return Ok(PostAction::Remove);
                        }
                        Ok(n) => pending.extend_from_slice(&buf[..n]),
                        Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => break,
                        Err(e) => {
                            warn!("Hyprland event socket error: {e}");
                            return Ok(PostAction::Remove);
                        }
                    }
                }
                while let Some(nl) = pending.iter().position(|&b| b == b'\n') {
                    let line: Vec<u8> = pending.drain(..=nl).collect();
                    if let Ok(line) = std::str::from_utf8(&line) {
                        let name = line.split(">>").next().unwrap_or("");
                        // `openwindow>>ADDR,workspace,class,title` — focus a
                        // just-launched app the instant its window maps.
                        if name.starts_with("openwindow") {
                            if let Some(addr) =
                                line.split(">>").nth(1).and_then(|d| d.split(',').next())
                            {
                                let addr = addr.trim();
                                let addr = if addr.starts_with("0x") {
                                    addr.to_owned()
                                } else {
                                    format!("0x{addr}")
                                };
                                // While the stage owns the screen the arrival
                                // is the stage's to handle — the ordinary
                                // path's plain focus broke the staged layout
                                // (see `on_window_opened_staged`).
                                if app.stage.is_on() {
                                    app.on_window_opened_staged(&addr);
                                } else {
                                    app.on_window_opened(&addr);
                                }
                            }
                        }
                        // The space's shape changed: a window arrived, left, or
                        // moved between spaces. Golem pseudotiles a space that
                        // is down to one tile and hands the proportions back
                        // when it has to be shared — see
                        // `App::sync_solitary_pseudo`.
                        if LAYOUT_SHAPE.iter().any(|r| name.starts_with(r)) {
                            app.schedule_solitary_pseudo();
                        }
                        // `workspacev2>>ID,NAME` — the space changed. Hand back
                        // the window that was left there, if the way in didn't
                        // (see [`crate::focus_cycle`]).
                        if name.starts_with("workspace") && !name.starts_with("workspacerule") {
                            if let Some(id) = line
                                .split(">>")
                                .nth(1)
                                .and_then(|d| d.split(',').next())
                                .and_then(|d| d.trim().parse::<i64>().ok())
                            {
                                app.on_workspace_changed(id);
                            }
                        }
                        // `closewindowv2>>ADDRESS` — a window is gone. STAGE
                        // mode keeps a deck of addresses, so it has to hear
                        // about this or it will offer a tile that no longer
                        // exists (and, if the staged one closed, sit in front
                        // of nothing).
                        if name.starts_with("closewindow") {
                            if let Some(addr) = line.split(">>").nth(1).map(str::trim) {
                                let addr = format!("0x{}", addr.trim_start_matches("0x"));
                                let was_on = app.stage.is_on();
                                app.stage.forget(&addr);
                                // Addresses are window pointers and Hyprland
                                // reuses them, so a thumbnail left filed under a
                                // dead window's address would eventually be
                                // shown for an unrelated application that
                                // happened to land on it.
                                app.deck_thumb_layer.remove(&addr);
                                // `forget` fixes the stage's own bookkeeping;
                                // the tiles are a separate copy and have to be
                                // rebuilt, or the deck keeps offering a window
                                // that is gone — clicking it raises and frames a
                                // dead tile while the stage stays put.
                                if was_on {
                                    app.rebuild_deck();
                                }
                                // The dock's minimized tiles are keyed by this
                                // same reusable address: evict now rather than
                                // trusting the plugin's `min-del` to arrive (a
                                // window can die minimized), or a click on the
                                // stale tile would "restore" whatever window
                                // later inherits the address.
                                if app.evict_minimized(&addr) {
                                    app.refilter();
                                }
                            }
                        }
                        // `openwindow>>ADDRESS,WS,CLASS,TITLE` — while Golem
                        // is a FLOATING window manager a window that arrives
                        // TILED is proof the compositor no longer has the
                        // float rule: it forgets runtime rules on config
                        // reload, and a re-assertion lost in a socket race
                        // stays lost (2026-09-15 — a whole morning's windows
                        // mapped tiled). Heal both halves: the rule, and the
                        // window that slipped past it.
                        if name.starts_with("openwindow") {
                            if let Some(addr) = line
                                .split(">>")
                                .nth(1)
                                .and_then(|d| d.split(',').next())
                            {
                                let addr =
                                    format!("0x{}", addr.trim().trim_start_matches("0x"));
                                app.heal_floating_map(&addr);
                            }
                        }
                        // (No `changefloatingmode` hook here on purpose: this
                        // fork never emits one — measured on the event socket
                        // 2026-09-12, across float toggles by address. The bar's
                        // state pill watches the engine's `is_floating` instead,
                        // which costs no read at all.)
                        //
                        // A monitor coming/going or the config being re-read
                        // rebuilds the output's DRM state, which silently drops
                        // the colour matrix hyprsunset set (Hyprland pushes a
                        // CTM only when it *changes*, so nothing puts it back).
                        // Re-assert the remembered screen look — see `screen.rs`.
                        if SCREEN_DRIFT.iter().any(|r| name.starts_with(r)) {
                            debug!("hypr event: {} — re-asserting screen", name.trim());
                            app.reassert_screen_state();
                            app.reassert_floating_mode();
                        }
                        if RELEVANT.iter().any(|r| name.starts_with(r)) {
                            debug!("hypr event: {}", name.trim());
                            relevant = true;
                        }
                    }
                }
                if relevant {
                    app.on_layout_changed();
                }
                Ok(PostAction::Continue)
            },
        )
        .map_err(|e| anyhow!("registering Hyprland event source: {e}"))?;
    Ok(())
}

/// The focused monitor: geometry in **logical** pixels plus its active
/// workspace id. Hyprland reports monitor dimensions as physical pixels
/// and window positions as logical pixels; dividing by scale brings them
/// into the same coordinate space for the zone overlap check.
pub struct MonitorInfo {
    pub x: f64,
    pub y: f64,
    pub w: f64,
    pub h: f64,
    /// What layer surfaces have taken out of the output, logical px, in
    /// Hyprland's order: left, top, right, bottom. The OPTIONS bar is the 28 at
    /// the top.
    pub reserved: (f64, f64, f64, f64),
    pub active_ws: i64,
    /// Output scale factor (physical = logical × scale).
    pub scale: f64,
    /// Connector name (e.g. `DP-1`) — matches the `wl_output` name.
    pub name: String,
}

/// Query the focused monitor.
pub fn focused_monitor() -> anyhow::Result<MonitorInfo> {
    let monitors: serde_json::Value =
        serde_json::from_str(&request("j/monitors")?).context("parsing monitors JSON")?;
    monitors
        .as_array()
        .into_iter()
        .flatten()
        .find(|m| m["focused"].as_bool().unwrap_or(false))
        .map(|m| {
            // `width`/`height` are physical pixels; `at`/`size` in
            // j/clients are logical pixels — divide by scale so both
            // sides of the zone-overlap check are in the same space.
            let scale = m["scale"].as_f64().unwrap_or(1.0).max(0.1);
            MonitorInfo {
                x: m["x"].as_f64().unwrap_or(0.0),
                y: m["y"].as_f64().unwrap_or(0.0),
                w: m["width"].as_f64().unwrap_or(0.0) / scale,
                h: m["height"].as_f64().unwrap_or(0.0) / scale,
                reserved: {
                    let r = |i: usize| m["reserved"][i].as_f64().unwrap_or(0.0);
                    (r(0), r(1), r(2), r(3))
                },
                active_ws: m["activeWorkspace"]["id"].as_i64().unwrap_or(-1),
                scale,
                name: m["name"].as_str().unwrap_or("").to_owned(),
            }
        })
        .ok_or_else(|| anyhow!("no focused monitor in Hyprland reply"))
}

/// What the OPTIONS bar needs to colour-match a maximized window: the
/// `wl_output` name and the physical row (window's top) to sample.
pub struct TopFill {
    /// Connector name of the monitor to capture.
    pub monitor: String,
    /// Physical y of the window's top row (just below the reserved bar).
    pub sample_y: u32,
}

/// How far into the window (logical px below its top edge) to sample the
/// colour match. Kept **shallow** — just past the ~1–2px top border/highlight
/// the bar overhang already paints over — so on a two-tone top (e.g. Chrome's
/// black tab strip over its darker-grey toolbar) the bar takes the strip it
/// actually abuts. Combined with side-only horizontal sampling (see
/// `read_sample`), this reads the top strip's *background*, not a centred
/// URL/search field.
const WINDOW_TOP_INSET: f64 = 4.0;

/// Detect a single tiled window filling the space under the bar, so the bar
/// can take its colour and the two read as one surface. Returns what to
/// sample, or `None` when there's nothing unambiguous to match: an empty
/// workspace, a split (2+ tiled windows), or only floating windows — in which
/// case the bar stays transparent.
///
/// Unlike a strict "smart-gaps flush" test, this matches whether or not the
/// window's gaps have collapsed: any lone tiled window sitting under the bar
/// qualifies, so the bar tracks it in both gapped and edge-to-edge layouts.
/// It samples from the window's *actual* top edge, so a gapped window (pushed
/// down by a top gap) is still read at the right row.
pub fn top_fill(bar_h_logical: f64) -> Option<TopFill> {
    let mon = focused_monitor().ok()?;
    let clients: serde_json::Value = serde_json::from_str(&request("j/clients").ok()?).ok()?;
    let usable_top = mon.y + bar_h_logical;

    // Tiled (non-floating), mapped, visible, non-fullscreen windows on the
    // focused workspace. Fullscreen is handled separately by the caller.
    let mut tiled = clients.as_array()?.iter().filter(|c| {
        c["workspace"]["id"].as_i64() == Some(mon.active_ws)
            && c["mapped"].as_bool().unwrap_or(false)
            && !c["hidden"].as_bool().unwrap_or(false)
            && !c["floating"].as_bool().unwrap_or(false)
            && c["fullscreen"].as_i64().unwrap_or(0) == 0
    });
    // Exactly one tiled window ⇒ unambiguous colour to match. A split is left
    // transparent (which of the two colours would the full-width bar take?).
    let win = tiled.next()?;
    if tiled.next().is_some() {
        return None;
    }
    // It must sit right under the bar — top at (or just below, allowing for a
    // top gap) the reserved zone — not pushed far down the screen.
    let top = win["at"][1].as_f64()?;
    if top < usable_top - 3.0 || top > usable_top + 40.0 {
        return None;
    }
    Some(TopFill {
        monitor: mon.name,
        // Sample a little below the window's actual top edge ([`WINDOW_TOP_INSET`])
        // — past the top-edge band (CSD rounding/border/gradient) yet firmly in
        // the chrome, and below the bar overhang so it never samples the bar.
        sample_y: ((top + WINDOW_TOP_INSET) * mon.scale.max(0.1)).round() as u32,
    })
}

/// What the dock needs to colour-match a maximized window sitting flush
/// above it — the bottom-edge twin of [`TopFill`]. Same fields, same
/// meaning: `wl_output` name and the physical row to sample.
pub struct BottomFill {
    /// Connector name of the monitor to capture.
    pub monitor: String,
    /// Physical y of the sample row, just above the window's bottom edge.
    pub sample_y: u32,
}

/// How far above the window's actual bottom edge (logical px) to sample —
/// the bottom-edge mirror of [`WINDOW_TOP_INSET`]. Shallow, for the same
/// reason: stay inside the chrome just past any bottom border/shadow
/// without drifting up into unrelated content.
const WINDOW_BOTTOM_INSET: f64 = 4.0;

/// Detect a single tiled window filling the space down to the true screen
/// bottom, so the dock (which floats over it, reserving no exclusive zone —
/// unlike the bar) can take its colour. The bottom-edge twin of [`top_fill`];
/// same ambiguity rule: an empty workspace, a split, or only floating
/// windows return `None` and the dock falls back to its own frosted
/// backdrop.
pub fn bottom_fill() -> Option<BottomFill> {
    let mon = focused_monitor().ok()?;
    let clients: serde_json::Value = serde_json::from_str(&request("j/clients").ok()?).ok()?;
    // Unlike the bar, the dock reserves NO exclusive zone (an auto-hide
    // overlay, floating over whatever's there) — so a flush window tiles all
    // the way to the real screen edge, not to `screen_bottom - dock_height`.
    let usable_bottom = mon.y + mon.h;

    let mut tiled = clients.as_array()?.iter().filter(|c| {
        c["workspace"]["id"].as_i64() == Some(mon.active_ws)
            && c["mapped"].as_bool().unwrap_or(false)
            && !c["hidden"].as_bool().unwrap_or(false)
            && !c["floating"].as_bool().unwrap_or(false)
            && c["fullscreen"].as_i64().unwrap_or(0) == 0
    });
    let win = tiled.next()?;
    if tiled.next().is_some() {
        return None;
    }
    // Its bottom edge must sit right at the screen's bottom — at or a little
    // above `usable_bottom` (a bottom gap pulls it up further), never past
    // it by more than a hair.
    let top = win["at"][1].as_f64()?;
    let h = win["size"][1].as_f64()?;
    let bottom = top + h;
    if bottom > usable_bottom + 3.0 || bottom < usable_bottom - 40.0 {
        return None;
    }
    Some(BottomFill {
        monitor: mon.name,
        // Sample a little above the window's actual bottom edge
        // ([`WINDOW_BOTTOM_INSET`]) — past any bottom border/shadow, still
        // inside the chrome.
        sample_y: ((bottom - WINDOW_BOTTOM_INSET) * mon.scale.max(0.1)).round() as u32,
    })
}

/// One live compositor window, for matching against dock apps.
pub struct RunningWindow {
    /// Window address (`0x…`) — the focus/activate handle.
    pub address: String,
    /// The app_id/class the window reports. `initialClass` is preferred
    /// (stable across title-driven class changes), falling back to
    /// `class`.
    pub class: String,
}

/// All mapped, non-hidden windows with their class and address. The
/// currently-focused window is moved to the front so a click on a
/// running app can activate the most-recently-used window first.
/// Empty (not an error) when Hyprland is unreachable.
pub fn running_windows() -> Vec<RunningWindow> {
    let Ok(reply) = request("j/clients") else {
        return Vec::new();
    };
    let Ok(clients) = serde_json::from_str::<serde_json::Value>(&reply) else {
        return Vec::new();
    };
    let active = active_window();
    let mut out: Vec<RunningWindow> = clients
        .as_array()
        .into_iter()
        .flatten()
        .filter(|c| {
            c["mapped"].as_bool().unwrap_or(false) && !c["hidden"].as_bool().unwrap_or(false)
        })
        .filter_map(|c| {
            let address = c["address"].as_str()?.to_owned();
            let class = c["initialClass"]
                .as_str()
                .filter(|s| !s.is_empty())
                .or_else(|| c["class"].as_str())
                .unwrap_or("")
                .to_owned();
            (!class.is_empty()).then_some(RunningWindow { address, class })
        })
        .collect();
    if let Some(active) = active {
        // Stable partition: the focused window's app leads.
        out.sort_by_key(|w| w.address != active);
    }
    out
}

// ─────────────────────────── STAGE mode primitives ───────────────────────────
//
// Everything here was verified live against Hyprland 0.55.4 on 2026-09-04
// (scratch window, empty workspace). Wrong Lua names fail *silently* on this
// fork, so nothing below is guessed — see `docs/hypr-api.md`.
//
// The stage rect is NOT set by positioning the window: this fork's Lua API has
// no absolute-position dispatcher at all. It comes from a workspace rule whose
// `gaps_out` opens exactly the inset we want, which the compositor then lays the
// window into. Measured: the rule below puts the window at `[10,31] 1980×1019`
// on Max's 2000×1250 output — the design rect, to the pixel.

/// Run a Lua expression over the control socket. Best effort, like
/// [`dispatch`]: the config API (`hl.workspace_rule`, `hl.animation`, …) is
/// reachable this way, and a failure must never be fatal.
pub fn eval(lua: &str) {
    match request(&format!("eval {lua}")) {
        Ok(reply) if reply.trim() == "ok" => {}
        Ok(reply) => debug!("Hyprland eval {lua:?} replied: {}", reply.trim()),
        Err(e) => debug!("Hyprland eval {lua:?} failed: {e:#}"),
    }
}

/// The two workspace selectors that must be re-pointed for the stage inset.
///
/// `w[tv1]` (one tiled window) and `f[1]` (one fullscreen window) are the
/// "smart gaps" rules from `/etc/nixos/hyprland.lua:357-358`, which normally
/// zero the gaps so a solitary window sits flush. A solitary staged window
/// matches the first; a *maximized* one (the sibling case) matches the second.
/// Re-pointing both means the stage rect holds however many windows the
/// workspace happens to contain.
const SMART_GAP_SELECTORS: [&str; 2] = ["w[tv1]", "f[1]"];

/// A runtime workspace rule BEATS the configured smart-gaps rule (verified), so
/// this is how the stage rect is produced. `band` is the bottom gap — the deck's
/// strip — and the other three come from [`crate::stage`], which is also where
/// the thumbnail capture reads them, so the photograph frames exactly the
/// window.
///
/// Assert the staged window's frame rule and the backdrop dim, every enter.
///
/// The rule lives in `/etc/nixos/hyprland.lua` too, but a `hyprctl reload`
/// re-reads the *store* copy of the config — which lags `/etc/nixos` until the
/// next `nixos-rebuild` — and wipes any runtime settings with it. That is
/// exactly how the stage dim quietly vanished once (`dim_around` measured back
/// at its 0.4 default; the strength here is 0.8, Max's pick). Re-asserting
/// from the daemon on every enter makes the
/// mode self-sufficient: idempotent, and whatever a reload did, entering the
/// stage puts the stage's look back.
///
/// `dim_around` (the strength) is a global, but only a window carrying this
/// rule's `dim_around = true` ever engages it — nothing else in the config
/// does — so setting it without restoring is safe.
pub fn assert_stage_frame() {
    eval(
        "hl.window_rule({ name = \"golem-stage-frame\", match = { tag = \"golem-stage\" }, \
         border_size = 0, rounding = 12, no_shadow = false, dim_around = true }) \
         hl.config({ [\"decoration.dim_around\"] = 0.8 })",
    );
}

/// A workspace rule can be overridden but never *removed*, so
/// [`clear_stage_gaps`] restores the literal values from the config rather than
/// trying to undo this.
pub fn set_stage_gaps(band: i32) {
    let (top, side) = (crate::stage::GAP_TOP, crate::stage::GAP_SIDE);
    for sel in SMART_GAP_SELECTORS {
        eval(&format!(
            "hl.workspace_rule({{ workspace = \"{sel}\", \
             gaps_out = {{ top = {top}, left = {side}, right = {side}, bottom = {band} }}, \
             gaps_in = 5 }})"
        ));
    }
}

/// The desktop's general gaps, as the config left them.
///
/// Values are in CSS order — top, right, bottom, left — because that is how
/// Hyprland reports them, and reading them back in the same order they arrive is
/// one fewer place to get a rotation wrong.
#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
pub struct GapSnapshot {
    out: [i64; 4],
    inner: i64,
}

impl Default for GapSnapshot {
    /// `/etc/nixos/hyprland.lua:126-134` — what to put back when there is no
    /// reading to put back instead (a socket error, or a breadcrumb written by
    /// a daemon that predates the snapshot).
    fn default() -> Self {
        GapSnapshot {
            out: [3, 10, 10, 10],
            inner: 5,
        }
    }
}

/// Read the general gaps before the stage changes them.
///
/// Snapshotting rather than hard-coding, for the reason the animation leaves are
/// snapshotted: restoring is then exact by construction and cannot go stale the
/// day those numbers are retuned in the config.
pub fn snapshot_general_gaps() -> GapSnapshot {
    fn css(option: &str) -> Option<Vec<i64>> {
        let raw = request(&format!("j/getoption {option}")).ok()?;
        let v: serde_json::Value = serde_json::from_str(&raw).ok()?;
        let css = v["css"].as_str()?;
        let parts: Vec<i64> = css
            .split_whitespace()
            .filter_map(|p| p.parse().ok())
            .collect();
        (parts.len() == 4).then_some(parts)
    }
    let fallback = GapSnapshot::default();
    GapSnapshot {
        out: css("general:gaps_out")
            .and_then(|p| p.try_into().ok())
            .unwrap_or(fallback.out),
        inner: css("general:gaps_in")
            .map(|p| p[0])
            .unwrap_or(fallback.inner),
    }
}

/// Open the stage inset on **every** workspace at once, by moving the desktop's
/// general gaps.
///
/// [`set_stage_gaps`] is enough for a staged task: it is maximized, so it
/// matches the smart-gaps selectors that rule is re-pointing. A whole desk
/// matches neither selector — it is however many windows the user put there — so
/// the general gaps are what lay it out, and those are what the stage moves.
///
/// A **numeric** workspace rule was the obvious alternative and is wrong: a
/// workspace rule can be overridden but never removed, and a numeric rule beats
/// the `w[tv1]` smart-gaps selector, so every desk visited would have come out
/// of the mode permanently un-smart — measured, a lone window landing at
/// `[10,31]` where it belonged flush at `[0,28]`. A global is restored exactly
/// because it is one value with one previous reading.
pub fn set_general_gaps(band: i32) {
    let (top, side) = (crate::stage::GAP_TOP, crate::stage::GAP_SIDE);
    eval(&format!(
        "hl.config({{ general = {{ \
         gaps_out = {{ top = {top}, left = {side}, right = {side}, bottom = {band} }} }} }})"
    ));
}

/// Put the general gaps back to what [`snapshot_general_gaps`] found.
///
/// Harmless when the stage never moved them — it writes the same values that
/// are already there — which is what lets both the exit path and crash recovery
/// call it unconditionally rather than tracking whether it is owed.
pub fn restore_general_gaps(snap: &GapSnapshot) {
    let [top, right, bottom, left] = snap.out;
    let inner = snap.inner;
    eval(&format!(
        "hl.config({{ general = {{ \
         gaps_out = {{ top = {top}, right = {right}, bottom = {bottom}, left = {left} }}, \
         gaps_in = {inner} }} }})"
    ));
}

/// Put the smart-gaps rules back to their configured values
/// (`gaps_out = 0, gaps_in = 0`, `/etc/nixos/hyprland.lua:357-358`). Exactly
/// reversible because the original is a known constant, not a guess.
pub fn clear_stage_gaps() {
    for sel in SMART_GAP_SELECTORS {
        eval(&format!(
            "hl.workspace_rule({{ workspace = \"{sel}\", gaps_out = 0, gaps_in = 0 }})"
        ));
    }
}

/// Silence **all** compositor animation while the stage is up.
///
/// Switching tasks maximizes one window and un-maximizes another, and Hyprland
/// animates both — so a swap that is supposed to read as an instant cut came
/// with a resize, a fade and a border transition riding along. The deck's own
/// tile motion is drawn by us, inside our own surface, so it is untouched by
/// this: the only animation left in the mode is the one that was designed.
///
/// `animations:enabled` is a single master bool and does not disturb the
/// per-leaf configuration, so flipping it back is exact — unlike disabling the
/// leaves one by one, which would have to restore every speed and curve from
/// `/etc/nixos/hyprland.lua:172-188` by hand.
/// The animation leaves a task switch goes through: the maximize/un-maximize,
/// the workspace change, the fades and the border transition.
const STAGE_LEAVES: &[&str] = &[
    "global",
    "windows",
    "windowsIn",
    "windowsOut",
    "fade",
    "fadeIn",
    "fadeOut",
    "border",
    "workspaces",
    "workspacesIn",
    "workspacesOut",
];

/// One animation leaf's live settings.
///
/// Serializable so the snapshot can ride in the stage breadcrumb: a daemon that
/// dies mid-stage would otherwise leave every one of these off, with no record
/// of what they were, and recovery would have to guess from a copy of the config.
#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
pub struct AnimLeaf {
    name: String,
    enabled: bool,
    speed: f64,
    /// As reported: either a bezier name, or `spring:NAME` for a spring.
    bezier: String,
    style: String,
}

/// Read the current settings of the leaves the stage silences.
///
/// Snapshotting beats hard-coding the values from `/etc/nixos/hyprland.lua`:
/// restoring is then exact by construction and cannot go stale the day those
/// curves are retuned.
pub fn snapshot_animations() -> Vec<AnimLeaf> {
    let Ok(raw) = request("j/animations") else {
        return Vec::new();
    };
    let Ok(v) = serde_json::from_str::<serde_json::Value>(&raw) else {
        return Vec::new();
    };
    // `j/animations` is `[[leaves…], [beziers…]]`.
    v.get(0)
        .and_then(|l| l.as_array())
        .into_iter()
        .flatten()
        .filter_map(|a| {
            let name = a["name"].as_str()?.to_owned();
            STAGE_LEAVES.contains(&name.as_str()).then(|| AnimLeaf {
                name,
                enabled: a["enabled"].as_bool().unwrap_or(false),
                speed: a["speed"].as_f64().unwrap_or(0.0),
                bezier: a["bezier"].as_str().unwrap_or("").to_owned(),
                style: a["style"].as_str().unwrap_or("").to_owned(),
            })
        })
        .collect()
}

/// Silence every leaf a switch animates.
///
/// The master `animations:enabled` flag is NOT enough on its own — measured:
/// with it false, every leaf the config explicitly enabled still reported
/// `enabled=1` and still animated. Nor does disabling the `global` leaf cascade
/// to its children. Each leaf has to be turned off by name.
pub fn silence_animations() {
    eval("hl.config({ [\"animations.enabled\"] = false })");
    for leaf in STAGE_LEAVES {
        eval(&format!(
            "hl.animation({{ leaf = \"{leaf}\", enabled = false }})"
        ));
    }
}

/// Put the leaves back from a [`snapshot_animations`] snapshot.
///
/// The full spec must be re-sent: disabling a leaf **wipes its speed and
/// curve** (`spring:easy` @4.79 becomes `default` @1.00), and a bare
/// `enabled = true` does not even re-enable it. Both measured the hard way.
pub fn restore_animations(snapshot: &[AnimLeaf]) {
    eval("hl.config({ [\"animations.enabled\"] = true })");
    for leaf in snapshot {
        if !leaf.enabled {
            continue; // it was already off; leave it that way
        }
        let mut spec = format!(
            "leaf = \"{}\", enabled = true, speed = {}",
            leaf.name, leaf.speed
        );
        // A spring is reported as `spring:NAME` but must be set back as
        // `spring = "NAME"` — feeding it as a bezier name would not resolve.
        if let Some(spring) = leaf.bezier.strip_prefix("spring:") {
            spec.push_str(&format!(", spring = \"{spring}\""));
        } else if !leaf.bezier.is_empty() {
            spec.push_str(&format!(", bezier = \"{}\"", leaf.bezier));
        }
        if !leaf.style.is_empty() {
            spec.push_str(&format!(", style = \"{}\"", leaf.style));
        }
        eval(&format!("hl.animation({{ {spec} }})"));
    }
}

/// Maximize (`fullscreen` state 1 — fills the workspace area, honouring the
/// reserved bar and the gaps) or un-maximize the **focused** window.
///
/// BOTH fields are required: bare `{ internal = 1 }` is a silent no-op
/// (verified). `client = 0` keeps the app itself unaware, so it renders as a
/// normal window rather than going true-fullscreen.
///
/// Operates on the focused window because `fullscreen_state`'s support for a
/// `window =` field is unverified — every caller here focuses first, so an
/// address form is not needed.
pub fn maximize_focused(on: bool) {
    set_fullscreen_focused(i64::from(on));
}

/// Set the focused window's internal fullscreen state outright: 0 windowed,
/// 1 maximized, 2 true fullscreen.
///
/// Staging must *restore* what it found rather than assume 0 — a window that was
/// already true-fullscreen when it went on the stage has to come back
/// fullscreen, or the mode has quietly destroyed user state.
pub fn set_fullscreen_focused(internal: i64) {
    dispatch(&format!(
        "hl.dsp.window.fullscreen_state({{ internal = {internal}, client = 0 }})"
    ));
}

/// Set a **named** window's internal fullscreen state, rather than the focused
/// one's.
///
/// This is what the stage uses, and the distinction is not cosmetic.
/// [`set_fullscreen_focused`] acts on whatever the compositor considers focused
/// at that instant, and on a workspace holding several windows that is not
/// reliably the window the stage just asked for: focusing across a workspace
/// boundary is not settled by the time the next request lands, and the daemon's
/// own workspace-focus restore can move focus again afterwards. The result was a
/// stage that tagged the right window and maximized a different one — or none —
/// leaving the whole workspace tiled on screen. Naming the window removes the
/// question entirely.
pub fn set_fullscreen_of(addr: &str, internal: i64) {
    dispatch(&format!(
        "hl.dsp.window.fullscreen_state({{ internal = {internal}, client = 0, \
         window = \"address:{addr}\" }})"
    ));
}

/// Ask waveview for square thumbnails of several windows at once, each written
/// to `<dir>/<address>.rgba` as raw RGBA.
///
/// The compositor is the only thing that can photograph a window that is not on
/// screen, and waveview already renders exactly this — a window's texture
/// cropped into a small framebuffer — for the overview's minis. This is that
/// render, read back and handed out.
///
/// **Always ask for the whole set at once**, even when it is one window. The
/// plugin renders every workspace the set touches in a single pass, so a batch
/// of eight costs about what one does; eight separate calls would pay the
/// monitor-resolution workspace render eight times.
///
/// The plugin's own answer is deliberately not consulted: a file either exists
/// at the expected length or it does not, and that holds whether the plugin is
/// loaded, is an older build without the entry point, or is absent entirely —
/// all of which degrade to title-only tiles rather than to an error.
pub fn capture_deck(addrs: &[String], size: u32, tile_aspect: f32, dir: &std::path::Path) {
    if addrs.is_empty() {
        return;
    }
    let Some(d) = dir.to_str() else {
        return;
    };
    eval(&format!(
        "hl.plugin.waveview.capture_deck(\"{}\", {size}, {tile_aspect}, \"{d}\")",
        addrs.join(",")
    ));
}

/// Ask waveview for square thumbnails of several **workspaces** at once, each
/// written to `<dir>/ws-<id>.rgba` as raw RGBA.
///
/// The desk deck's counterpart to [`capture_deck`]: a tile there stands for a
/// whole workspace, so the picture has to be the workspace — its windows where
/// the user put them, not one of them on its own. The plugin already renders
/// exactly that for the overview's grid.
///
/// Same contract as [`capture_deck`] in every other respect: one call for the
/// whole set, and the answer is read off the filesystem rather than from the
/// plugin, so an absent or older plugin degrades to title-only tiles.
pub fn capture_desks(workspaces: &[i64], size: u32, tile_aspect: f32, dir: &std::path::Path) {
    if workspaces.is_empty() {
        return;
    }
    let Some(d) = dir.to_str() else {
        return;
    };
    let list = workspaces
        .iter()
        .map(|ws| ws.to_string())
        .collect::<Vec<_>>()
        .join(",");
    eval(&format!(
        "hl.plugin.waveview.capture_desks(\"{list}\", {size}, {tile_aspect}, \"{d}\")"
    ));
}

/// The workspace floating windows are parked on while the stage is up.
///
/// An ordinary workspace, deliberately, not a **special** one: moving a window
/// to a special workspace *shows* that workspace as an overlay, so the window
/// stayed on screen — parked and still in the way. A plain move to a numbered
/// workspace changes nothing about what is displayed, leaves the active
/// workspace alone, and returns the window with its position and size intact.
/// Chosen high to stay clear of the workspaces anyone binds keys to.
pub const PARK_WS: i64 = 99;

/// Move one floating window out of sight, and bring one home again.
///
/// Used on entering and leaving the mode, where a single eval buys nothing —
/// one dispatch per window means a window that closed in between costs only
/// itself, rather than aborting the rest (an eval stops at its first failure).
pub fn park_window(addr: &str) {
    unpark_window(addr, PARK_WS);
}

pub fn unpark_window(addr: &str, workspace: i64) {
    dispatch(&format!(
        "hl.dsp.window.move({{ workspace = {workspace}, window = \"address:{addr}\" }})"
    ));
}

/// Everything one stage hand-over has to do, in the order it has to happen.
pub struct Handover<'a> {
    /// The task leaving the stage. It loses the frame but **keeps the stage
    /// shape**, so coming back to it costs the client no relayout.
    pub untag: Option<&'a str>,
    /// Tasks that must give the shape back, as `(address, the fullscreen state
    /// to return them to)`. A workspace holds one fullscreen window, so a task
    /// already shaped on the incoming one's workspace has to let go first.
    pub restore: &'a [(String, i64)],
    /// Floating windows to move out of the way. Floating windows render *above*
    /// tiled ones, so a maximized stage does not cover them — without this a
    /// floating sibling sits on top of the staged task.
    pub park: &'a [String],
    /// Parked windows to bring home, as `(address, the workspace it came from)`.
    pub unpark: &'a [(String, i64)],
    /// The task arriving.
    pub addr: &'a str,
    /// A window sharing the arriving task's workspace, for the focus bounce
    /// (focusing an already-focused window is a no-op, and the compositor only
    /// moves the keyboard seat when focus actually changes).
    ///
    /// Supplied by the caller from the `window_states` read it already made —
    /// looking it up here cost a second blocking `j/clients` round-trip at the
    /// exact moment click latency shows.
    pub neighbor: Option<&'a str>,
}

/// Hand the stage from one task to another: **the entire swap in one eval**.
///
/// Every part of this is here for a reason found the hard way:
///
/// * **Atomic.** Split into separate requests, the compositor renders between
///   them — and a frame where one task has let go and the next has not yet taken
///   hold shows that whole workspace tiled. Switching between two tasks on a
///   ten-window workspace flashed all ten. In one eval nothing is drawn until
///   every dispatch has run.
/// * **Focus before maximize.** `fullscreen_state` silently does nothing to a
///   window whose workspace is not the active one — and still answers `ok` — so
///   the focus, which is what brings that workspace forward, has to land first.
///   Maximizing first looked correct and did nothing, which is how a switch onto
///   a busy workspace left the whole workspace on screen.
/// * **Every window named.** The un-maximize and the maximize address their
///   window rather than acting on "the focused one", which during a
///   cross-workspace switch is not reliably either of them.
/// * **Restores before the maximize.** Same one-per-workspace rule: the second
///   `fullscreen_state` on a workspace is refused, silently.
pub fn swap_stage_no_warp(h: Handover) {
    let Handover {
        untag,
        restore,
        park,
        unpark,
        addr,
        neighbor,
    } = h;
    let mut lua = String::from("hl.config({ [\"cursor.no_warps\"] = true }) ");
    if let Some(prev) = untag {
        lua.push_str(&format!(
            "hl.dispatch(hl.dsp.window.tag({{ tag = \"-{STAGE_TAG}\", \
             window = \"address:{prev}\" }})) "
        ));
    }
    // Bring parked tasks home before anything else — one of them may be the task
    // arriving, and it cannot take a stage it is not on.
    for (a, ws) in unpark {
        lua.push_str(&format!(
            "hl.dispatch(hl.dsp.window.move({{ workspace = {ws}, window = \"address:{a}\" }})) "
        ));
    }
    for a in park {
        lua.push_str(&format!(
            "hl.dispatch(hl.dsp.window.move({{ workspace = {PARK_WS}, \
             window = \"address:{a}\" }})) "
        ));
    }
    for (a, fs) in restore {
        lua.push_str(&format!(
            "hl.dispatch(hl.dsp.window.fullscreen_state({{ internal = {fs}, client = 0, \
             window = \"address:{a}\" }})) "
        ));
    }
    // Focusing an already-focused window is a no-op, so the focus bounces off a
    // neighbour first — the same detour `focus_window_no_warp` uses.
    if let Some(other) = neighbor {
        lua.push_str(&format!(
            "hl.dispatch(hl.dsp.focus({{ window = \"address:{other}\" }})) "
        ));
    }
    lua.push_str(&format!(
        "hl.dispatch(hl.dsp.focus({{ window = \"address:{addr}\" }})) \
         hl.dispatch(hl.dsp.window.fullscreen_state({{ internal = 1, client = 0, \
         window = \"address:{addr}\" }})) \
         hl.dispatch(hl.dsp.window.tag({{ tag = \"+{STAGE_TAG}\", \
         window = \"address:{addr}\" }})) "
    ));
    lua.push_str("hl.config({ [\"cursor.no_warps\"] = false })");
    if let Err(e) = request(&format!("eval {lua}")) {
        debug!("swap_stage_no_warp({addr}) failed: {e:#}");
    }
}

/// What the stage needs to know about a window it is about to touch.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct WindowState {
    /// Internal fullscreen mode: 0 windowed, 1 maximized, 2 true fullscreen.
    pub fullscreen: i64,
    /// Which workspace it sits on. A workspace holds **one** fullscreen window,
    /// so this decides which task has to give the stage shape back before
    /// another can take it.
    pub workspace: i64,
    /// Floating windows render **above** tiled ones, so a maximized stage does
    /// not cover them — they have to be moved aside instead. See [`PARK_WS`].
    pub floating: bool,
}

/// Every live window's state, keyed by address — **one** `clients` read
/// answering "does it still exist", "what state was it in" and "where is it".
///
/// The click path used to ask those questions separately, which meant three
/// socket round-trips (~50ms of blocking) between the click and the first frame
/// of the tile animation. One read keeps the swap feeling immediate.
pub fn window_states() -> std::collections::HashMap<String, WindowState> {
    let Ok(reply) = request("j/clients") else {
        return std::collections::HashMap::new();
    };
    let Ok(clients) = serde_json::from_str::<serde_json::Value>(&reply) else {
        return std::collections::HashMap::new();
    };
    clients
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|c| {
            Some((
                c["address"].as_str()?.to_owned(),
                WindowState {
                    fullscreen: c["fullscreen"].as_i64().unwrap_or(0),
                    workspace: c["workspace"]["id"].as_i64().unwrap_or(i64::MIN),
                    floating: c["floating"].as_bool().unwrap_or(false),
                },
            ))
        })
        .collect()
}

/// Take the compositor's overview keybind away while the stage owns the screen,
/// and give it back on the way out.
///
/// `hl.unbind` is the only lever the Lua API offers here — verified live. The
/// rebind restores the exact form from `/etc/nixos/hyprland.lua:282`, and it has
/// to be a deferred lookup for the same reason the config binds it that way: the
/// plugin's function is resolved at keypress, not at bind time.
///
/// NOTE the workspace-swipe **gestures** cannot be handled this way. There is no
/// unregister, re-registering is refused ("will be overshadowed by a previous
/// gesture") and `action = "none"` is rejected. They are neutralised instead by
/// snapping focus back — see `App::on_workspace_changed`.
/// Focus a window **without the compositor dragging the pointer to it**.
///
/// Focusing warps the cursor by default, so clicking a deck tile threw the mouse
/// into the middle of the stage. `cursor.no_warps` suppresses that.
///
/// It is set and cleared *inside one eval*, around these dispatches only —
/// deliberately not held for the whole mode. `cursor.no_warps` is a single
/// global that other daemon paths also drive (the dock's focus hand-back via
/// [`focus_window_direct`] sets it and resets it to `false`), so a mode-long
/// setting gets silently clobbered — measured: it read `false` while staged.
/// One atomic eval cannot be interleaved, and leaves the baseline untouched.
///
/// Keeps [`focus_window`]'s neighbour-bounce: the compositor only moves the
/// keyboard seat when focus actually *changes* between windows.
pub fn focus_window_no_warp(addr: &str) {
    let mut lua = String::from("hl.config({ [\"cursor.no_warps\"] = true }) ");
    if let Some(other) = same_workspace_neighbor(addr) {
        lua.push_str(&format!(
            "hl.dispatch(hl.dsp.focus({{ window = \"address:{other}\" }})) "
        ));
    }
    lua.push_str(&format!(
        "hl.dispatch(hl.dsp.focus({{ window = \"address:{addr}\" }})) "
    ));
    lua.push_str("hl.config({ [\"cursor.no_warps\"] = false })");
    if let Err(e) = request(&format!("eval {lua}")) {
        debug!("focus_window_no_warp({addr}) failed: {e:#}");
    }
}

/// The tag that marks the window currently on the stage. It is BOTH the marker
/// and the match key for hyprland.lua's `golem-stage-frame` rule, which hands
/// the window back its border and rounding.
///
/// Needed because the staged window is tiled and maximized, so it matches the
/// smart-gaps rules (`no-gaps-wtv1` / `no-gaps-f1`) that strip `border_size` and
/// `rounding` to zero. On the stage the window is a card in its own inset and
/// should read like one. Same mechanism as [`PSEUDO_TAG`].
const STAGE_TAG: &str = "golem-stage";

/// Tag or untag a window as the staged one. `rounding`/`border_size` are dynamic
/// rule props, so the frame appears and disappears as the tag flips.
pub fn set_stage_tag(addr: &str, on: bool) {
    let sign = if on { '+' } else { '-' };
    dispatch(&format!(
        "hl.dsp.window.tag({{ tag = \"{sign}{STAGE_TAG}\", window = \"address:{addr}\" }})"
    ));
}

/// Switch into the `stage` submap, or back out of it.
///
/// A submap is Hyprland's own "only these binds are live" mode, which is
/// exactly the ask: while the stage owns the screen nothing should launch,
/// close, tile or move a window. The submap is declared in
/// `/etc/nixos/hyprland.lua` and holds only the ways out; the control keys
/// (volume/brightness/media) survive because they are marked
/// `submap_universal`. Ordinary typing is unaffected — a submap changes *binds*,
/// not input, so the staged window still receives keystrokes.
///
/// Leaving is `"reset"`. Harmless to call when no submap is active.
pub fn set_stage_submap(on: bool) {
    let name = if on { "stage" } else { "reset" };
    dispatch(&format!("hl.dsp.submap(\"{name}\")"));
}

/// Tell the waveview plugin whether the stage owns the screen.
///
/// This is the *only* thing that actually stops either the overview's 3-finger
/// swipe or the workspace swipe: both are gestures, and the plugin is the one
/// place able to consume them (`info.cancelled`). Unbinding Super+R covers the
/// keyboard route only — the swipe never passes through the bind system, which
/// is why the overview stayed reachable from the stage until now.
///
/// Best effort: an older plugin without `set_stage` just errors in the eval and
/// the mode still works, minus the gesture lock.
pub fn set_plugin_stage(on: bool) {
    eval(&format!("hl.plugin.waveview.set_stage({on})"));
}

// The stage used to take Super+R away while it owned the screen; it doesn't any
// more (the overview opens over the stage now), so the unbind/rebind pair that
// did it is gone. The lesson it left is still worth keeping: `hl.bind` **adds**,
// so binding over a live bind leaves TWO and one press fires both — which is
// why nothing here rebinds a key it did not first unbind.

/// One task for the stage deck.
pub struct StageTask {
    pub address: String,
    pub class: String,
    pub title: String,
    pub workspace: i64,
    /// Compositor focus recency (0 = focused). Seeds the deck's initial order.
    pub history: i64,
    /// Internal fullscreen mode (0/1/2) — what the entry sweep records so exit
    /// can restore it.
    pub fullscreen: i64,
    /// Floating windows render above the maximized stage and get parked; see
    /// `stage::Stage::parked`.
    pub floating: bool,
    /// Owning process — how an audio stream is matched to its tile (the
    /// stream's pid ancestry is walked and checked against this).
    pub pid: i64,
}

/// Every mapped window as a deck task, most-recently-focused first — one
/// `clients` read (~16ms measured). Empty (not an error) if Hyprland is
/// unreachable, so the mode degrades to "nothing to show" instead of failing.
pub fn stage_tasks() -> Vec<StageTask> {
    let Ok(reply) = request("j/clients") else {
        return Vec::new();
    };
    let Ok(clients) = serde_json::from_str::<serde_json::Value>(&reply) else {
        return Vec::new();
    };
    let mut out: Vec<StageTask> = clients
        .as_array()
        .into_iter()
        .flatten()
        .filter(|c| {
            c["mapped"].as_bool().unwrap_or(false) && !c["hidden"].as_bool().unwrap_or(false)
        })
        .filter_map(|c| {
            Some(StageTask {
                address: c["address"].as_str()?.to_owned(),
                class: c["initialClass"]
                    .as_str()
                    .filter(|s| !s.is_empty())
                    .or_else(|| c["class"].as_str())
                    .unwrap_or("")
                    .to_owned(),
                title: c["title"].as_str().unwrap_or("").to_owned(),
                workspace: c["workspace"]["id"].as_i64().unwrap_or(0),
                history: c["focusHistoryID"].as_i64().unwrap_or(i64::MAX),
                fullscreen: c["fullscreen"].as_i64().unwrap_or(0),
                floating: c["floating"].as_bool().unwrap_or(false),
                pid: c["pid"].as_i64().unwrap_or(0),
            })
        })
        .collect();
    out.sort_by_key(|t| t.history);
    out
}

/// Whether `addr` is still a live mapped window — the check before focusing a
/// remembered address (the anchor) that may have been closed meanwhile.
pub fn window_exists(addr: &str) -> bool {
    let Ok(reply) = request("j/clients") else {
        return false;
    };
    let Ok(clients) = serde_json::from_str::<serde_json::Value>(&reply) else {
        return false;
    };
    clients
        .as_array()
        .is_some_and(|cs| cs.iter().any(|c| c["address"].as_str() == Some(addr)))
}

/// Result of a dock-zone evaluation.
pub struct ZoneState {
    /// A window overlaps the zone (or is fullscreen): the dock dodges.
    pub occupied: bool,
}

/// Evaluate `zone` (x, y, w, h in layout pixels) against the windows
/// of workspace `active_ws`.
pub fn zone_state(zone: (f64, f64, f64, f64), active_ws: i64) -> anyhow::Result<ZoneState> {
    zone_state_from(&request("j/clients")?, zone, active_ws)
}

/// Pure core of [`zone_state`], parsing a `j/clients` JSON reply.
///
/// Filtering is by workspace id: this Hyprland reports `visible: true`
/// even for clients parked on inactive workspaces.
fn zone_state_from(
    clients_json: &str,
    zone: (f64, f64, f64, f64),
    active_ws: i64,
) -> anyhow::Result<ZoneState> {
    let clients: serde_json::Value =
        serde_json::from_str(clients_json).context("parsing clients JSON")?;
    let (zx, zy, zw, zh) = zone;
    let mut occupied = false;
    for c in clients.as_array().into_iter().flatten() {
        if c["workspace"]["id"].as_i64().unwrap_or(-2) != active_ws
            || !c["mapped"].as_bool().unwrap_or(false)
            || c["hidden"].as_bool().unwrap_or(false)
        {
            continue;
        }
        if c["fullscreen"].as_i64().unwrap_or(0) > 0 {
            occupied = true;
            continue;
        }
        let (x, y) = (
            c["at"][0].as_f64().unwrap_or(0.0),
            c["at"][1].as_f64().unwrap_or(0.0),
        );
        let (w, h) = (
            c["size"][0].as_f64().unwrap_or(0.0),
            c["size"][1].as_f64().unwrap_or(0.0),
        );
        if x < zx + zw && x + w > zx && y < zy + zh && y + h > zy {
            occupied = true;
        }
    }
    Ok(ZoneState { occupied })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// One mode kills the other: whatever combination of the compositor's
    /// overlapping flags a window is carrying, it reads back as exactly ONE of
    /// the four modes, with fullscreen winning over floating and floating over
    /// pseudo — because that is their order of visibility.
    #[test]
    fn a_window_is_in_exactly_one_mode() {
        let win = |fullscreen: i64, floating: bool, pseudo: bool| {
            serde_json::json!({
                "address": "0x1",
                "fullscreen": fullscreen,
                "floating": floating,
                "tags": if pseudo { vec![PSEUDO_TAG] } else { vec![] },
            })
        };
        // The clean cases.
        assert_eq!(mode_of(&win(0, false, false)), WindowMode::Tiled);
        assert_eq!(mode_of(&win(0, false, true)), WindowMode::Pseudo);
        assert_eq!(mode_of(&win(0, true, false)), WindowMode::Floating);
        assert_eq!(mode_of(&win(2, false, false)), WindowMode::Fullscreen);
        // The overlaps the compositor allows and the bar does not: every one
        // resolves to the mode you can actually see.
        assert_eq!(mode_of(&win(2, true, true)), WindowMode::Fullscreen);
        assert_eq!(mode_of(&win(2, false, true)), WindowMode::Fullscreen);
        assert_eq!(mode_of(&win(0, true, true)), WindowMode::Floating);
        // Maximized (fullscreen == 1) is still fullscreen as far as the mode
        // controls are concerned — only the bar's auto-hide distinguishes it.
        assert_eq!(mode_of(&win(1, false, false)), WindowMode::Fullscreen);
        // Missing fields (an older compositor, a partial reply) must not panic
        // or invent a mode.
        assert_eq!(
            mode_of(&serde_json::json!({ "address": "0x1" })),
            WindowMode::Tiled
        );
    }

    /// The real dock zone at default config: 720 wide centered on a
    /// 1280x800 layout, bottom 60 px.
    const ZONE: (f64, f64, f64, f64) = (280.0, 740.0, 720.0, 60.0);

    fn client(ws: i64, at: (i64, i64), size: (i64, i64)) -> String {
        format!(
            r#"{{"workspace": {{"id": {ws}}}, "mapped": true, "hidden": false,
                "fullscreen": 0, "at": [{}, {}], "size": [{}, {}]}}"#,
            at.0, at.1, size.0, size.1,
        )
    }

    fn occupied(clients: &[String], ws: i64) -> bool {
        let json = format!("[{}]", clients.join(","));
        zone_state_from(&json, ZONE, ws).unwrap().occupied
    }

    #[test]
    fn empty_workspace_is_free() {
        assert!(!occupied(&[], 9));
    }

    #[test]
    fn fullscreen_sized_tiled_window_occupies() {
        assert!(occupied(&[client(1, (0, 0), (1280, 800))], 1));
    }

    #[test]
    fn window_on_other_workspace_does_not_occupy() {
        assert!(!occupied(&[client(1, (0, 0), (1280, 800))], 9));
    }

    #[test]
    fn small_float_away_from_zone_is_free() {
        // The case from the field: a floating window parked at the top
        // must leave the dock zone free.
        assert!(!occupied(&[client(1, (100, 50), (400, 300))], 1));
    }

    #[test]
    fn small_float_over_zone_occupies() {
        assert!(occupied(&[client(1, (400, 700), (400, 300))], 1));
    }

    #[test]
    fn float_beside_zone_horizontally_is_free() {
        // Bottom-left corner, outside the centered zone's x-range.
        assert!(!occupied(&[client(1, (0, 700), (250, 100))], 1));
    }

    #[test]
    fn fullscreen_flag_occupies_regardless_of_geometry() {
        let c =
            client(1, (0, 0), (100, 100)).replacen(r#""fullscreen": 0"#, r#""fullscreen": 2"#, 1);
        assert!(occupied(&[c], 1));
    }

    #[test]
    fn hidden_or_unmapped_windows_do_not_occupy() {
        let hidden = client(1, (400, 700), (400, 300)).replacen(
            r#""hidden": false"#,
            r#""hidden": true"#,
            1,
        );
        let unmapped = client(1, (400, 700), (400, 300)).replacen(
            r#""mapped": true"#,
            r#""mapped": false"#,
            1,
        );
        assert!(!occupied(&[hidden, unmapped], 1));
    }

    #[test]
    fn browser_class_match_is_case_insensitive_and_substring() {
        // Real Hyprland class strings vary in case and carry suffixes.
        assert!(is_browser_class("firefox"));
        assert!(is_browser_class("Firefox"));
        assert!(is_browser_class("org.mozilla.firefox"));
        assert!(is_browser_class("Google-chrome"));
        assert!(is_browser_class("Brave-browser"));
        assert!(is_browser_class("zen-alpha"));
        // Not browsers — the copy-link / find offers must not fire here.
        assert!(!is_browser_class("foot"));
        assert!(!is_browser_class("org.gnome.Nautilus"));
        assert!(!is_browser_class(""));
    }
}

/// The name of the window rule that makes every window OPEN floating — one
/// named rule, re-declared to flip it, because this fork addresses rules by
/// name and has no "remove".
const FLOAT_RULE: &str = "golem-float-mode";

/// Turn Golem's floating mode on or off **at the compositor**: a catch-all
/// window rule that floats every window AS IT MAPS.
///
/// This is the whole reason the mode is a rule and not a sweep on
/// `openwindow`: a rule is applied while the window is being mapped, so an app
/// *launches* floating. Reacting to the open event instead would let it arrive
/// tiled and then jump — which is exactly what Max ruled out (2026-09-13: *"i
/// dont want them to come tiled and become floating, they have to launch as
/// floating"*).
///
/// Verified live on this fork the same day: a named rule can be re-declared to
/// replace it, and `enabled` is honoured — `enabled = true` gave a floating
/// window, `enabled = false` a tiled one, with no reload and no rule pile-up.
/// `remove` is NOT a field (it errors), so `enabled` is the off switch.
pub fn set_float_rule(on: bool) {
    // `hl.window_rule` is not a dispatcher, and the socket's `dispatch` wraps
    // whatever it is given in `hl.dispatch(...)` — which rejects a non-dispatch
    // value. Wrapping the call in a function that ends with `no_op` gives the
    // socket the dispatcher it insists on while the rule call does the work.
    // Golem's float SIZE and PLACE ride the same rule, so a window arrives
    // floated, sized and centred in ONE motion rather than being floated and
    // then shoved (Max, 2026-09-13: *"i want them placed and sized for
    // Golem"*).
    //
    // ⚠️ ABSOLUTE logical pixels, not percentages. `size = "55% 54%"` is
    // ACCEPTED by the parser and then silently does nothing (measured: the
    // window kept its own 700x500 while `center` took effect) — the one form
    // that actually applies is a pixel pair. So the rule carries
    // `float_size()`, the same numbers the per-window float toggle uses, and
    // is re-declared when the monitor changes, which `reassert_floating_mode`
    // already does off Hyprland's monitor events.
    //
    // `center = true` DOES respect the reserved area — measured at
    // y = 302 on a 1250-tall output under a 28px bar, i.e. centred in the
    // usable 1222, not in the whole screen.
    let size = match float_size() {
        Some((w, h)) => format!("size = {{ {w}, {h} }}, center = true, "),
        None => String::new(),
    };
    // Checked, not fire-and-forget: this rule is the floating mode. When the
    // assertion is lost (a socket race at session start, a reply that is an
    // error) every window from then on maps tiled and nothing says why —
    // measured 2026-09-15. The heal on `openwindow` (`heal_floating_map`)
    // catches whatever still slips through.
    dispatch_checked(&format!(
        "(function() hl.window_rule({{ name = \"{FLOAT_RULE}\", \
         match = {{ class = \".*\" }}, float = true, {size}enabled = {on} }}) \
         return hl.dsp.no_op() end)()"
    ));
}

/// Bring every window that is currently in the wrong state into the mode: all
/// tiles float, or all floats return to the layout.
///
/// **Fullscreen windows are left alone.** Fullscreen is neither tiled nor
/// floating — it is a third thing the user asked for explicitly, and toggling
/// float underneath it would drop them out of it for a reason they did not ask
/// for. They join the mode when they leave fullscreen.
///
/// One dispatch for the whole sweep: the toggles arrive in a single chunk so
/// the screen re-lays-out once rather than once per window.
pub fn float_all(on: bool) {
    let windows = layout_windows();
    // Where a swept window lands. A window that merely STOPS being tiled keeps
    // whatever the layout gave it — which for a maximized tile is the whole
    // screen, so the desk filled up with huge floats (Max, 2026-09-13: *"the
    // windows that are already open go huge"*). They get Golem's float size and
    // place, the same as one arriving under the rule.
    let place = float_size().zip(focused_monitor().ok());
    // Counted per WORKSPACE: the cascade is about what you can see at once, and
    // two desks' windows never overlap on screen.
    let mut nth: std::collections::HashMap<i64, usize> = std::collections::HashMap::new();
    let mut lua = String::new();
    for w in &windows {
        let wrong = match w.mode {
            WindowMode::Tiled | WindowMode::Pseudo => on,
            WindowMode::Floating => !on,
            WindowMode::Fullscreen => false,
        };
        if !wrong {
            continue;
        }
        let win = format!("window = \"address:{}\"", w.address);
        // A pseudo is a tile wearing Golem's proportions; drop the tag with it
        // so the frame rule does not keep painting a pseudo that is now a float.
        if w.mode == WindowMode::Pseudo {
            lua.push_str(&format!(
                "hl.dispatch(hl.dsp.window.tag({{ tag = \"-{PSEUDO_TAG}\", {win} }})) \
                 hl.dispatch(hl.dsp.window.pseudo({{ action = \"off\", {win} }})) "
            ));
        }
        lua.push_str(&format!(
            "hl.dispatch(hl.dsp.window.float({{ action = \"toggle\", {win} }})) "
        ));
        if !on {
            continue; // back into the layout; the layout decides the geometry
        }
        if let Some(((fw, fh), m)) = place.as_ref() {
            let (fw, fh) = (*fw, *fh);
            let i = nth.entry(w.workspace).or_default();
            let (x, y) = cascade_at(m, (fw, fh), *i);
            *i += 1;
            // Size THEN place, in this order and in the same chunk: `resize`
            // grows from the window's centre, so placing first would leave the
            // window somewhere else by the time it is sized.
            lua.push_str(&format!(
                "hl.dispatch(hl.dsp.window.resize({{ x = {fw}, y = {fh}, {win} }})) \
                 hl.dispatch(hl.dsp.window.move({{ x = {x}, y = {y}, {win} }})) "
            ));
        }
    }
    if lua.is_empty() {
        return;
    }
    dispatch(&format!("(function() {lua} return hl.dsp.no_op() end)()"));
}

/// Float ONE window, Golem-style — the single-window arm of [`float_all`],
/// for a window that mapped tiled while the mode says floating (the rule was
/// lost; see `settings.rs::heal_floating_map`). Same moves in the same order
/// as the sweep: pseudo tag off with the pseudo, float, then size THEN place
/// in one chunk.
pub fn float_window(w: &LayoutWindow) {
    let win = format!("window = \"address:{}\"", w.address);
    let mut lua = String::new();
    if w.mode == WindowMode::Pseudo {
        lua.push_str(&format!(
            "hl.dispatch(hl.dsp.window.tag({{ tag = \"-{PSEUDO_TAG}\", {win} }})) \
             hl.dispatch(hl.dsp.window.pseudo({{ action = \"off\", {win} }})) "
        ));
    }
    lua.push_str(&format!(
        "hl.dispatch(hl.dsp.window.float({{ action = \"toggle\", {win} }})) "
    ));
    let place = float_size().zip(focused_monitor().ok());
    if let Some(((fw, fh), m)) = place.as_ref() {
        let (fw, fh) = (*fw, *fh);
        let (x, y) = cascade_at(m, (fw, fh), 0);
        lua.push_str(&format!(
            "hl.dispatch(hl.dsp.window.resize({{ x = {fw}, y = {fh}, {win} }})) \
             hl.dispatch(hl.dsp.window.move({{ x = {x}, y = {y}, {win} }})) "
        ));
    }
    dispatch(&format!("(function() {lua} return hl.dsp.no_op() end)()"));
}

/// How far each window in a cascade steps down and right from the one before.
/// About a titlebar's worth — enough that every window in the pile shows its
/// own top edge.
const CASCADE_STEP: f64 = 30.0;
/// How many windows the cascade walks before starting over at the top. Without
/// it the tenth window on a busy desk marches off the screen.
const CASCADE_WRAP: usize = 5;

/// Where the `i`th swept window goes: centred in the USABLE area (the output
/// minus what the bar and the dock reserve), then stepped down-right so a pile
/// of them reads as a pile rather than as one window.
///
/// Clamped to the usable area, so the step can never push a window under the
/// bar or off the right edge however the numbers are set.
fn cascade_at(m: &MonitorInfo, size: (i64, i64), i: usize) -> (i64, i64) {
    let (l, t, r, b) = m.reserved;
    let (uw, uh) = (m.w - l - r, m.h - t - b);
    let (fw, fh) = (size.0 as f64, size.1 as f64);
    let step = CASCADE_STEP * (i % CASCADE_WRAP) as f64;
    let x = (l + (uw - fw) / 2.0 + step).clamp(l, (l + uw - fw).max(l));
    let y = (t + (uh - fh) / 2.0 + step).clamp(t, (t + uh - fh).max(t));
    (x.round() as i64, y.round() as i64)
}
