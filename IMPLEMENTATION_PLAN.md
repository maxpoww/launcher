# waverunner — Implementation Plan

Derived from CLAUDE.md. Phases are strictly ordered; a phase starts only
after the previous phase's acceptance criteria pass on a live Hyprland
session.

## Current status (2026-07-06)

Design pivot: waverunner is an **auto-hiding dock**, not a hotkey-first
launcher. Touching the bottom screen edge reveals a slim dock bar;
click launches, scroll expands to the full search popup, typing searches;
pointer-leave auto-hides. `waverunner-ctl` stays as the hotkey path.

Implemented and verified on a live Hyprland session (QEMU VM, llvmpipe,
scale 1 — redo visual checks on real GPU hardware):

- `proto`: line-based socket protocol (`Command`, `Response`, `socket_path`).
- `core::config`: full TOML config schema with defaults, XDG loading,
  hex-color parsing, `deny_unknown_fields` typo detection; `[input]`
  covers `natural_scroll`, `edge_reveal(_px)`, `autohide(_delay_ms)`.
- `core::search`: nucleo-backed fuzzy `Searcher`.
- `daemon::animation`: dt-based spring + easing engine (60/144 Hz
  invariance tests included).
- `daemon::state`: `Hidden → Dock → Open` rest-point machine animating
  `extent`, interrupt-safe (hide mid-open does not jump).
- `client`: complete `waverunner-ctl toggle|show|hide`.
- `daemon::ipc`: calloop-integrated Unix socket server; live-probe
  single-instance guard (connect() before removing a "stale" socket),
  SIGKILL recovery verified, malformed input answered with `err ...`.
- `daemon::{main,surface,renderer}`: fixed-size bottom-anchored layer
  surface, wgpu rounded-bar renderer, frame-callback loop that goes fully
  idle (0.0% CPU) at every rest point; input region tracks the visible
  extent (edge-reveal strip while hidden); scroll expand/collapse;
  pointer-leave auto-hide via calloop grace timer.

Deferred stubs: `core::index::DesktopIndex::scan()` returns empty (P4).

---

## P1 — Skeleton ✅ (accepted 2026-07-06)

**Goal:** prove smithay-client-toolkit + wgpu render onto a bottom-anchored,
transparent, fixed-size layer surface on Hyprland.

Verified on a live Hyprland session (QEMU VM, llvmpipe adapter, scale 1):
- Daemon starts, logs adapter info, no panic; `hyprctl layers` shows the
  surface at 720x420, bottom-anchored, top layer.
- `waverunner-ctl show`/`hide` fade the translucent bar in and out.
- 0.0% CPU over 10 s when settled (cumulative CPU time frozen).
- Surface never resized after mapping (single configure size).

Open follow-ups: repeat the visual checks on real GPU hardware (Vulkan
adapter, not llvmpipe) and confirm no frame callbacks with
`WAYLAND_DEBUG=1`.

## P2 — IPC (mostly verified)

**Goal:** reliable control socket; < 50 ms command-to-visible.

Verified live:
- 20 rapid `toggle`s complete in ~83 ms total (~4 ms round-trip each),
  daemon settles in the correct end state, no stuck states.
- Malformed socket input (`frobnicate`, empty line, trailing args) gets an
  `err ...` response and the daemon stays up.
- `kill -9` leaves a stale socket; the next start probes it with
  `connect()`, removes it, and rebinds.
- A second instance while one is live now *refuses to start* instead of
  stealing the socket (the original unconditional-remove orphaned the
  running daemon; fixed with the connect-probe).

Remaining tasks:
1. Hyprland keybind docs in README:
   `bind = SUPER, SPACE, exec, waverunner-ctl toggle`.
2. Measure latency: timestamp in client before connect vs. first frame
   presented (add a `tracing` span; target < 50 ms).

## P3 — Animation & dock interaction (working, checks open)

**Goal:** the grow + fade motion and the pointer-driven dock loop,
fully config-driven, idle at rest.

Working (verified live in the VM):
- Rounded-bar wgpu rendering; `extent`/`alpha` animation between the
  Hidden/Dock/Open rest points; goes fully idle at every rest point.
- Scroll on the dock expands to the popup / collapses back
  (`natural_scroll` config).
- Edge reveal: while hidden, a thin input-region strip
  (`edge_reveal_px`, default 5) hugs the bottom edge; pointer-enter
  reveals the dock. Default keeps margin for pointer stacks that clamp
  the cursor short of the last screen row (this VM tops out at y=797
  of 800 — a 2px strip was physically unreachable by mouse).
- Auto-hide: pointer-leave arms a grace timer (`autohide_delay_ms`,
  default 300); re-entry cancels it; firing hides dock *and* popup.
  P4 must suppress this while a search query is active / keyboard
  focused.

Remaining tasks:
1. Wire `[animation]` config through end-to-end sanity pass (kind,
   duration, spring params for open and close independently) — schema is
   parsed and plumbed; confirm each knob has visible effect.
2. Handle scale factor in pixel math (integer scale now; fractional-scale
   protocol is P5). Verify no blurry rendering at scale 2.
3. Verify frame pacing at 60 Hz and 144 Hz monitors (engine is dt-based
   and unit-tested for this; confirm visually on real hardware).
4. Confirm zero frame requests when settled with `WAYLAND_DEBUG=1`.

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
3. Dock icon row: pinned/frequent apps rendered in the dock bar,
   hover highlight, click-to-launch. Suppress auto-hide while a launch
   click is in flight.
4. Text input: xkb keysym → UTF-8 (sctk provides this on `KeyEvent`),
   maintain query string, re-rank with `core::Searcher` per keystroke.
   Typing while docked expands to Open. Decide the keyboard-focus model
   here (`Exclusive` while visible vs. click-to-focus with `OnDemand`)
   and suppress auto-hide while a query is active.
5. Render input bar text + result list + selection highlight (this is
   where `egui`'s wgpu backend may come in for text/list widgets —
   animation offsets stay in our engine per CLAUDE.md).
6. Exec + detach: `fork`/`setsid`, close fds, `exec` via `/bin/sh -c`;
   hide popup on launch.

Acceptance criteria:
- Edge-touch → click an icon launches it and the dock hides.
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
