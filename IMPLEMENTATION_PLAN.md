# waverunner — Implementation Plan

Derived from CLAUDE.md. Phases are strictly ordered; a phase starts only
after the previous phase's acceptance criteria pass on a live Hyprland
session.

## Current status (scaffold)

The workspace is generated and structured per CLAUDE.md. Implemented and
unit-tested (headless):

- `proto`: line-based socket protocol (`Command`, `Response`, `socket_path`).
- `core::config`: full TOML config schema with defaults, XDG loading,
  hex-color parsing, `deny_unknown_fields` typo detection.
- `core::search`: nucleo-backed fuzzy `Searcher`.
- `daemon::animation`: dt-based spring + easing engine (60/144 Hz
  invariance tests included).
- `daemon::state`: `Hidden → Opening → Open → Closing` state machine,
  interrupt-safe (hide mid-open does not jump).
- `client`: complete `waverunner-ctl toggle|show|hide`.
- `daemon::ipc`: calloop-integrated Unix socket server with stale-socket
  cleanup and socket-file guard.
- `daemon::{main,surface,renderer}`: P1 code written (layer surface,
  wgpu clear-only renderer, frame-callback loop) — **not yet compiled or
  run against a compositor**; treat as the P1 starting point, not done.

Deferred stubs: `core::index::DesktopIndex::scan()` returns empty (P4).

---

## P1 — Skeleton

**Goal:** prove smithay-client-toolkit + wgpu render onto a bottom-anchored,
transparent, fixed-size layer surface on Hyprland.

Tasks:
1. `nix develop && cargo build` — fix compile errors in the daemon first.
   The sctk handler trait signatures and the wgpu (pinned `24.x`)
   surface-creation API are the likely friction points; compare against
   sctk's `simple_layer` example and wgpu's raw-handle examples before
   restructuring anything.
2. Run `cargo run -p waverunner-daemon` inside Hyprland. Debug surface
   creation / adapter selection (`LD_LIBRARY_PATH` from the flake must
   expose `libvulkan.so` and `libwayland-client.so`).
3. Validate the design decisions: surface created once at full size,
   anchored bottom, transparent when progress = 0.

Acceptance criteria (visual, on Hyprland):
- Daemon starts, logs adapter info, no panic; a transparent surface exists
  (check `hyprctl layers`).
- `waverunner-ctl show` makes a translucent rectangle fade in at the
  bottom edge; `hide` fades it out (P1 renders opacity only — slide/grow
  come with P3 geometry).
- When settled (hidden or open), `top` shows ~0% CPU and `WAYLAND_DEBUG=1`
  shows no frame callbacks being requested.
- Surface is never resized after mapping.

## P2 — IPC

**Goal:** hotkey-driven toggle with < 50 ms keypress-to-visible.

Tasks:
1. Exercise `waverunner-ctl toggle|show|hide` against the daemon;
   verify stale-socket recovery after `kill -9`.
2. Hyprland keybind docs in README:
   `bind = SUPER, SPACE, exec, waverunner-ctl toggle`.
3. Measure latency: timestamp in client before connect vs. first frame
   presented (add a `tracing` span; target < 50 ms).
4. Decide single-instance policy (flock on the socket path) if stale-socket
   handling proves insufficient.

Acceptance criteria:
- Keybind toggles reliably under repeated mashing (no stuck states —
  the state machine's interrupt tests model this, verify live).
- Malformed socket input gets an `err ...` response and the daemon stays up.
- Measured keypress-to-visible < 50 ms.

## P3 — Animation engine

**Goal:** the slide-up + grow + fade motion, fully config-driven, idle at rest.

Tasks:
1. Add real geometry to the renderer: a rounded-rect pipeline (single quad
   + SDF corner shader) replacing clear-only rendering. Per frame:
   `y_offset = lerp(surface_h, 0, progress)`,
   `content_h = lerp(input_bar_h, full_h, progress)`, `opacity = progress`
   — translate/clip *content inside* the fixed surface.
2. Wire `[animation]` config (already parsed) through: kind, duration,
   spring params for open and close independently.
3. Handle scale factor in pixel math (integer scale now; fractional-scale
   protocol is P5).
4. Verify frame pacing at 60 Hz and 144 Hz monitors (animation engine is
   dt-based and unit-tested for this; confirm visually).

Acceptance criteria:
- Open: springy slide-up that settles without big overshoot; close:
  ~120–160 ms ease-in. Both tunable via config.toml with visible effect.
- Zero frame requests when settled (verify with `WAYLAND_DEBUG=1`).
- No blurry rendering at scale 2.

## P4 — Launcher core

**Goal:** it actually launches things.

Tasks:
1. Implement `core::index::DesktopIndex::scan()` with
   `freedesktop-desktop-entry`: XDG data dirs, locale-aware `Name=`,
   dedupe by desktop-file ID, honor `NoDisplay`/`Hidden`, strip Exec field
   codes. Cache to `$XDG_CACHE_HOME/waverunner/index` with mtime
   invalidation.
2. Index on a background thread at daemon start (the one allowed thread);
   deliver via `calloop::channel` into the event loop.
3. Text input: xkb keysym → UTF-8 (sctk provides this on `KeyEvent`),
   maintain query string, re-rank with `core::Searcher` per keystroke.
4. Render input bar text + result list + selection highlight (this is
   where `egui`'s wgpu backend may come in for text/list widgets —
   animation offsets stay in our engine per CLAUDE.md).
5. Exec + detach: `fork`/`setsid`, close fds, `exec` via `/bin/sh -c`;
   hide popup on launch.

Acceptance criteria:
- Typing filters apps with nucleo ranking; Up/Down/Enter work; Escape hides.
- Launched apps survive daemon restart (properly detached, no zombies).
- Cold start with cold cache < 100 ms to interactive; warm cache instant.

## P5 — Polish

- Icons: `freedesktop-icons` lookup + texture atlas.
- Theming: colors, corner radius, timings from `[theme]` (schema exists).
- Auto-hide on focus loss: keyboard-leave handler exists; verify against
  alt-tab, click-elsewhere, workspace switch (known risk area with
  `OnDemand` interactivity).
- Fractional scaling via `wp-fractional-scale-v1` + `wp-viewporter`.

## P6 — Modes

- Prefix-dispatched modes (`=` calc, `ssh ` hosts, clipboard).
- `Mode` trait in `core` (input → ranked items → activate action);
  the app launcher becomes the default mode.

---

## Cross-cutting rules (from CLAUDE.md — enforced throughout)

- `nix develop` first, always; missing system libs go into `flake.nix`.
- Clippy clean at `-D warnings`; no `unwrap`/`expect` outside `main.rs`
  startup and tests; `anyhow` in binaries, `thiserror` in libs.
- Daemon single-threaded except the P4 indexer thread.
- `tracing` only (client crate may `println!`).
- Rendering changes cannot be verified headless — describe what to check
  visually in the PR/commit instead of claiming it was tested.
- Conventional commits scoped by crate; never push, never amend.

## Risk register

| Risk | Phase | Mitigation |
|---|---|---|
| sctk ↔ wgpu raw-handle plumbing breaks at compile or runtime | P1 | Follow sctk `simple_layer` + wgpu raw-handle examples verbatim before inventing; keep wgpu pinned to one major |
| `OnDemand` keyboard focus-loss detection unreliable | P2/P5 | Test alt-tab, click-other-window, workspace switch explicitly; fall back to Hyprland IPC events if needed |
| Fractional scaling blurs text | P3/P5 | All pixel math via one `scale()` helper; integer scale until wp-fractional-scale lands |
| Compositor throttles frame callbacks for occluded/offscreen surfaces | P3 | Never rely on a frame callback arriving to make progress; IPC show must draw immediately |
| nucleo/wgpu API churn across versions | all | Workspace pins majors; upgrade deliberately, one crate at a time |
