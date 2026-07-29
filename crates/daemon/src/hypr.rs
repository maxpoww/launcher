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

/// Close a window by address (best effort) — used when uninstalling an app
/// that's still open, so it goes away with its package instead of lingering
/// on screen with nothing behind it.
pub fn close_window(addr: &str) {
    dispatch(&format!(
        "hl.dsp.closewindow({{ window = \"address:{addr}\" }})"
    ));
}

/// Address (`0x…`) of the currently-focused window, if any.
pub fn active_window() -> Option<String> {
    let json: serde_json::Value = serde_json::from_str(&request("j/activewindow").ok()?).ok()?;
    json["address"].as_str().map(str::to_owned)
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
                                app.on_window_opened(&addr);
                            }
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
    pub active_ws: i64,
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
                active_ws: m["activeWorkspace"]["id"].as_i64().unwrap_or(-1),
            }
        })
        .ok_or_else(|| anyhow!("no focused monitor in Hyprland reply"))
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
}
