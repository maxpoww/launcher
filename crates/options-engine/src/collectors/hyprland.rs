//! Layer 1 — Hyprland compositor & spatial geometry.
//!
//! Two sockets under `$XDG_RUNTIME_DIR/hypr/$HYPRLAND_INSTANCE_SIGNATURE/`:
//! `.socket2.sock` is the async event stream (`name>>data\n` lines);
//! `.socket.sock` answers one-shot `j/…` JSON queries. The collector streams
//! events, and on any that can change the focused window it re-queries
//! `j/activewindow` for the full detail (pid, workspace, fullscreen, floating)
//! — the event payloads alone don't carry it.
//!
//! It also derives **focus-switch velocity** (window switches/sec over a
//! trailing window) purely from these events: a behavioural signal with zero
//! input capture, exactly the kind of non-invasive sensing OPTIONS prefers.
//!
//! Resilience: a self-contained reconnect loop with capped backoff. Without
//! Hyprland (env unset, socket gone) the layer simply stays dark — the mind
//! sees `health.compositor.alive == false` and treats its fields as unknown.

use std::collections::VecDeque;
use std::path::PathBuf;
use std::time::{Duration, Instant};

use anyhow::{anyhow, Context as _};
use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};
use tokio::net::UnixStream;
use tokio::sync::mpsc;

use tokio::sync::watch;

use crate::collector::{Collector, CollectorFuture};
use crate::message::{ContextDelta, Update};
use crate::state::{ActiveWindow, ContextState, Layer};

/// Trailing window over which focus switches are counted for velocity.
const FOCUS_WINDOW: Duration = Duration::from_secs(5);
const BACKOFF_MIN: Duration = Duration::from_millis(200);
const BACKOFF_MAX: Duration = Duration::from_secs(10);

/// Layer-1 collector. Stateless config; all runtime state lives in [`run`].
#[derive(Default)]
pub struct HyprlandCollector;

impl HyprlandCollector {
    pub fn new() -> Self {
        Self
    }
}

impl Collector for HyprlandCollector {
    fn name(&self) -> &'static str {
        "hyprland"
    }
    fn layer(&self) -> Layer {
        Layer::Compositor
    }
    fn run(
        self: Box<Self>,
        _ctx: watch::Receiver<ContextState>,
        tx: mpsc::Sender<Update>,
    ) -> CollectorFuture {
        Box::pin(async move {
            let mut backoff = BACKOFF_MIN;
            let mut tracker = FocusTracker::default();
            loop {
                match stream_events(&tx, &mut tracker).await {
                    Ok(()) => {
                        tracing::debug!("hyprland event socket closed; reconnecting");
                        backoff = BACKOFF_MIN;
                    }
                    Err(e) => tracing::debug!("hyprland collector: {e:#}"),
                }
                // Source is down until we reconnect.
                let _ = tx.send(Update::Health(Layer::Compositor, false)).await;
                tokio::time::sleep(backoff).await;
                backoff = (backoff * 2).min(BACKOFF_MAX);
            }
        })
    }
}

/// The instance socket directory, or an error if not running under Hyprland.
fn instance_dir() -> anyhow::Result<PathBuf> {
    let sig = std::env::var_os("HYPRLAND_INSTANCE_SIGNATURE")
        .ok_or_else(|| anyhow!("HYPRLAND_INSTANCE_SIGNATURE unset (not under Hyprland?)"))?;
    let runtime =
        std::env::var_os("XDG_RUNTIME_DIR").ok_or_else(|| anyhow!("XDG_RUNTIME_DIR unset"))?;
    Ok(PathBuf::from(runtime).join("hypr").join(sig))
}

/// One-shot `j/…` query over the control socket, returning the raw JSON reply.
async fn query(cmd: &str) -> anyhow::Result<String> {
    let path = instance_dir()?.join(".socket.sock");
    let mut stream = UnixStream::connect(&path)
        .await
        .with_context(|| format!("connecting {}", path.display()))?;
    stream.write_all(cmd.as_bytes()).await?;
    let mut out = String::new();
    // Hyprland writes the reply then closes, so read to EOF.
    stream.read_to_string(&mut out).await?;
    Ok(out)
}

/// Connect the event socket, seed initial state, then stream until it closes.
async fn stream_events(tx: &mpsc::Sender<Update>, tracker: &mut FocusTracker) -> anyhow::Result<()> {
    let path = instance_dir()?.join(".socket2.sock");
    let stream = UnixStream::connect(&path)
        .await
        .with_context(|| format!("connecting {}", path.display()))?;
    tx.send(Update::Health(Layer::Compositor, true)).await?;

    // Seed the focused window immediately so subscribers don't wait for the
    // first event to learn what's focused.
    refresh_window(tx, tracker).await;

    let mut lines = BufReader::new(stream).lines();
    while let Some(line) = lines.next_line().await? {
        match parse_event(&line) {
            Some(Event::RefreshWindow) => refresh_window(tx, tracker).await,
            Some(Event::Submap(s)) => {
                let _ = tx
                    .send(Update::Delta(Layer::Compositor, ContextDelta::Submap(s)))
                    .await;
            }
            Some(Event::ActiveLayout(l)) => {
                let _ = tx
                    .send(Update::Delta(Layer::Compositor, ContextDelta::ActiveLayout(l)))
                    .await;
            }
            Some(Event::Screencast(on)) => {
                let _ = tx
                    .send(Update::Delta(
                        Layer::Compositor,
                        ContextDelta::Screencasting(on),
                    ))
                    .await;
            }
            None => {}
        }
    }
    Ok(())
}

/// Query `j/activewindow`, emit the focused-window delta, and update focus
/// velocity when the focused address actually changed.
async fn refresh_window(tx: &mpsc::Sender<Update>, tracker: &mut FocusTracker) {
    let (address, window) = match query("j/activewindow").await {
        Ok(reply) => serde_json::from_str::<serde_json::Value>(&reply)
            .ok()
            .and_then(|v| active_window_from_json(&v))
            .unwrap_or_default(),
        Err(e) => {
            tracing::debug!("j/activewindow query failed: {e:#}");
            return;
        }
    };
    if tracker.note_focus(&address) {
        let v = tracker.velocity();
        let _ = tx
            .send(Update::Delta(
                Layer::Compositor,
                ContextDelta::FocusSwitchVelocity(v),
            ))
            .await;
    }
    let _ = tx
        .send(Update::Delta(Layer::Compositor, ContextDelta::Window(window)))
        .await;
}

/// Build an [`ActiveWindow`] from a `j/activewindow` reply, returning the
/// window's address alongside it (for focus-change detection). An empty /
/// `0x0` address (nothing focused) yields the cleared default.
fn active_window_from_json(v: &serde_json::Value) -> Option<(String, ActiveWindow)> {
    let address = v["address"].as_str().unwrap_or("");
    if address.is_empty() || address == "0x0" {
        return Some((String::new(), ActiveWindow::default()));
    }
    // `class` reflects the app *now*; fall back to the stable `initialClass`.
    let class = v["class"]
        .as_str()
        .filter(|s| !s.is_empty())
        .or_else(|| v["initialClass"].as_str())
        .unwrap_or("")
        .to_owned();
    let window = ActiveWindow {
        address: address.to_owned(),
        class,
        title: v["title"].as_str().unwrap_or("").to_owned(),
        pid: v["pid"].as_i64().unwrap_or(0).max(0) as u32,
        workspace_id: v["workspace"]["id"].as_i64().unwrap_or(-1) as i32,
        // Hyprland fullscreen: 0 none, 1 maximized (keeps the reserved bar),
        // 2 true fullscreen. Only 2 is "fullscreen" here (maximized still shows
        // the bar); a maximized flag can be added if the mind wants it.
        is_fullscreen: v["fullscreen"].as_i64().unwrap_or(0) >= 2,
        is_floating: v["floating"].as_bool().unwrap_or(false),
    };
    Some((address.to_owned(), window))
}

/// A parsed, actionable compositor event.
#[derive(Debug, PartialEq, Eq)]
enum Event {
    /// Something changed the focused window; re-query for full detail.
    RefreshWindow,
    /// Active submap (`""` = default).
    Submap(String),
    /// Active keyboard layout description.
    ActiveLayout(String),
    /// Screencast on/off.
    Screencast(bool),
}

/// Parse one `name>>data` line from `.socket2.sock`. Pure and total, so it can
/// be unit-tested without a compositor. Returns `None` for events OPTIONS
/// doesn't act on.
fn parse_event(line: &str) -> Option<Event> {
    let (name, data) = match line.split_once(">>") {
        Some((n, d)) => (n, d),
        None => (line, ""),
    };
    match name {
        // Focus / geometry changes → re-query the focused window. The event
        // payloads don't carry pid/workspace/fullscreen/floating, so a single
        // j/activewindow query is the source of truth.
        "activewindow" | "activewindowv2" | "fullscreen" | "changefloatingmode" | "workspace"
        | "workspacev2" | "focusedmon" => Some(Event::RefreshWindow),
        "submap" => Some(Event::Submap(data.trim().to_owned())),
        // `activelayout>>KEEBNAME,Layout Description` — keep the layout part.
        "activelayout" => Some(Event::ActiveLayout(
            data.split_once(',').map(|(_, l)| l).unwrap_or(data).trim().to_owned(),
        )),
        // `screencast>>STATE,OWNER` — STATE is 0/1.
        "screencast" => Some(Event::Screencast(data.split(',').next() == Some("1"))),
        _ => None,
    }
}

/// Trailing-window focus-switch counter → velocity in switches/second.
#[derive(Default)]
struct FocusTracker {
    last_address: String,
    switches: VecDeque<Instant>,
}

impl FocusTracker {
    /// Record the currently-focused `address`; returns whether focus changed
    /// to a different window (empty→empty and same→same don't count).
    fn note_focus(&mut self, address: &str) -> bool {
        if address == self.last_address {
            return false;
        }
        self.last_address = address.to_owned();
        // Only count switches *between* real windows, not focus loss/gain of
        // the empty desktop.
        if !address.is_empty() {
            self.switches.push_back(Instant::now());
        }
        true
    }

    /// Switches per second over the trailing [`FOCUS_WINDOW`].
    fn velocity(&mut self) -> f32 {
        let cutoff = Instant::now() - FOCUS_WINDOW;
        while self.switches.front().is_some_and(|&t| t < cutoff) {
            self.switches.pop_front();
        }
        self.switches.len() as f32 / FOCUS_WINDOW.as_secs_f32()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_focus_and_geometry_events_as_refresh() {
        for line in [
            "activewindowv2>>55f2a1b0c0",
            "activewindow>>foot,zsh",
            "fullscreen>>1",
            "changefloatingmode>>55f2a1b0c0,1",
            "workspace>>3",
            "workspacev2>>3,three",
            "focusedmon>>DP-1,3",
        ] {
            assert_eq!(parse_event(line), Some(Event::RefreshWindow), "{line}");
        }
    }

    #[test]
    fn parses_submap_including_empty_default() {
        assert_eq!(parse_event("submap>>resize"), Some(Event::Submap("resize".into())));
        assert_eq!(parse_event("submap>>"), Some(Event::Submap(String::new())));
    }

    #[test]
    fn parses_active_layout_keeping_description() {
        assert_eq!(
            parse_event("activelayout>>AT Translated Set 2 keyboard,English (US)"),
            Some(Event::ActiveLayout("English (US)".into()))
        );
    }

    #[test]
    fn parses_screencast_state() {
        assert_eq!(parse_event("screencast>>1,0"), Some(Event::Screencast(true)));
        assert_eq!(parse_event("screencast>>0,0"), Some(Event::Screencast(false)));
    }

    #[test]
    fn ignores_unrelated_events() {
        assert_eq!(parse_event("createworkspace>>3"), None);
        assert_eq!(parse_event("garbage-without-delim"), None);
    }

    #[test]
    fn active_window_json_maps_fields_and_fullscreen_level() {
        let json = serde_json::json!({
            "address": "0x55f2a1b0c0",
            "class": "foot",
            "initialClass": "foot",
            "title": "nvim",
            "pid": 1234,
            "workspace": {"id": 2},
            "fullscreen": 2,
            "floating": false
        });
        let (addr, w) = active_window_from_json(&json).unwrap();
        assert_eq!(addr, "0x55f2a1b0c0");
        assert_eq!(w.class, "foot");
        assert_eq!(w.title, "nvim");
        assert_eq!(w.pid, 1234);
        assert_eq!(w.workspace_id, 2);
        assert!(w.is_fullscreen);
    }

    #[test]
    fn maximized_is_not_fullscreen() {
        let json = serde_json::json!({
            "address": "0x1", "class": "foot", "title": "", "pid": 1,
            "workspace": {"id": 1}, "fullscreen": 1, "floating": false
        });
        let (_, w) = active_window_from_json(&json).unwrap();
        assert!(!w.is_fullscreen);
    }

    #[test]
    fn empty_address_is_cleared_window() {
        let json = serde_json::json!({ "address": "0x0" });
        let (addr, w) = active_window_from_json(&json).unwrap();
        assert!(addr.is_empty());
        assert_eq!(w.class, "");
        assert_eq!(w.pid, 0);
    }

    #[test]
    fn focus_tracker_counts_only_distinct_window_switches() {
        let mut t = FocusTracker::default();
        assert!(t.note_focus("0xA"));
        assert!(!t.note_focus("0xA")); // same window, no switch
        assert!(t.note_focus("0xB"));
        assert!(t.note_focus("")); // focus lost — a change, but not counted
        // Two real windows entered within the window ⇒ 2 / 5s.
        assert!((t.velocity() - 2.0 / 5.0).abs() < 1e-6);
    }
}
