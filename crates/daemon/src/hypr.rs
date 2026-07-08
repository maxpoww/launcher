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
//! Known limitation: Hyprland emits no event for interactively moving
//! or resizing a floating window within a workspace, so a floating
//! window dragged over the zone is only noticed at the next relevant
//! event or reveal.

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

/// The focused monitor: geometry in layout pixels plus its active
/// workspace id.
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
        .map(|m| MonitorInfo {
            x: m["x"].as_f64().unwrap_or(0.0),
            y: m["y"].as_f64().unwrap_or(0.0),
            w: m["width"].as_f64().unwrap_or(0.0),
            h: m["height"].as_f64().unwrap_or(0.0),
            active_ws: m["activeWorkspace"]["id"].as_i64().unwrap_or(-1),
        })
        .ok_or_else(|| anyhow!("no focused monitor in Hyprland reply"))
}

/// Whether any window on workspace `active_ws` overlaps `zone`
/// (x, y, w, h in layout pixels), or is fullscreen.
///
/// Filtering is by workspace id: this Hyprland reports `visible: true`
/// even for clients parked on inactive workspaces.
pub fn dock_zone_occupied(zone: (f64, f64, f64, f64), active_ws: i64) -> anyhow::Result<bool> {
    let clients: serde_json::Value =
        serde_json::from_str(&request("j/clients")?).context("parsing clients JSON")?;
    let (zx, zy, zw, zh) = zone;
    for c in clients.as_array().into_iter().flatten() {
        if c["workspace"]["id"].as_i64().unwrap_or(-2) != active_ws
            || !c["mapped"].as_bool().unwrap_or(false)
            || c["hidden"].as_bool().unwrap_or(false)
        {
            continue;
        }
        if c["fullscreen"].as_i64().unwrap_or(0) > 0 {
            return Ok(true);
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
            return Ok(true);
        }
    }
    Ok(false)
}
