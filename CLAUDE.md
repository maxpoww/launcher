# CLAUDE.md — waverunner

> Big picture first: read **`GOLEM.md`** — waverunner is the *body* of Golem, a
> Linux distribution built around OPTIONS. This file covers the body's code.

Auto-hiding Wayland/Hyprland **dock + launcher**, Rust. Persistent daemon holding a
layer-shell surface at the bottom edge. Touch the edge → slim dock; scroll/expand →
full search popup; type to fuzzy-search. GPU-rendered (wgpu), frame-callback driven,
idle at rest. `waverunner-ctl toggle|show|hide|expand|collapse` over a unix socket.

## Build / run / test (always via the flake)
```
nix develop -c cargo build
nix develop -c cargo clippy --workspace -- -D warnings   # keep clean
nix develop -c cargo test --workspace
~/launcher/waverunner-dev        # dev launcher (sets LD_LIBRARY_PATH); Hyprland runs this on start
```
No headless compositor — visual/drag behavior needs a live Hyprland session; describe
what to check rather than claiming it's verified.

## Layout
- `crates/daemon` — the binary (`waverunner`): wayland loop, wgpu renderer, state, search, drag.
- `crates/client` — `waverunner-ctl`.
- `crates/core` — config, `.desktop` index (`DesktopIndex`/`AppEntry`), fuzzy search.
- `crates/proto` — socket message types.

## Model
- **Dock** = pinned apps only. **Grid** (`SECTION_APPS`) = every indexed `.desktop`.
  **Install section** (`SECTION_INSTALL`) = nixpkgs packages **and webapps** (see below),
  searchable, drag-to-grid to install. **Files** = live home listing.
- App discovery on one background thread → `LoadedApps` over a channel; `request_rescan(_fresh)`
  re-indexes. Live reload: inotify on the XDG app dirs (calloop `Generic` source) → rescan.
- Icons: freedesktop theme chain (Papirus-Dark → hicolor), SVG via resvg, PNG via tiny-skia,
  256² premultiplied RGBA mip chains.
- Installs (packages) are declarative: waverunner edits `~/.config/waverunner/packages.list`
  (one nixpkgs attr per line); a root `systemd.path` runs `waverunner-apply` which generates
  root-owned `/etc/nixos/waverunner-packages.nix` and `nixos-rebuild switch`es. See `applier.rs`.

## Webapp catalog (installable, like packages)
Curated Chrome webapps from `~/.config/webapps.list` (`Name | URL | icon`). Every entry is
materialized as `webapp-<slug>.desktop` (`webapps::materialize_catalog`) so the indexer
rasterizes its icon. Classified at runtime by id-prefix + `managed_webapps` membership:
`webapp-*` not in `managed_webapps` → Install section (catalog); installed → grid. Click/
drag-out = "try" (launch, window shows in dock unpinned); drag to grid = install
(`managed_webapps.add` → `waverunner-webapps.nix`); drag installed → Install = uninstall.
Files: `webapps.rs`, `managed_webapps.rs`, routing in `main.rs::refilter`, drag arms in `dragging.rs`.

## Conventions
Edition 2021, rustfmt, clippy clean at `-D warnings`. `anyhow` in the binary, `thiserror` in
libs; no `unwrap` outside `main` startup/tests. One event loop; all shared state lives on it.
Background work runs on dedicated worker threads that only talk back via calloop channels: the
app indexer, and the nix index/rank, install/uninstall mutation, and try-it (`nix build`)
workers — kept separate so a slow build never blocks an install, or search.
`tracing` for logs. Never `git push` or amend. Commits: conventional (`feat(daemon): …`).

## Non-negotiables (don't revisit)
Persistent daemon (no per-invocation spawn); fixed-size layer surface (animate content, never
resize the surface); hotkeys are compositor-side (ctl writes the socket); pointer-reveal is a
thin surface-side input strip; all animation dt-based (test 60/144Hz).

## Seeing the UI (visual verification)
There is no headless renderer — a UI change is only "verified" once looked at on the live
Hyprland session. Use the **`verify-ui` skill** (`.claude/skills/verify-ui/`): it builds,
restarts the daemon with the new binary, optionally reveals a surface, and `grim`s a screenshot
to `/tmp/waverunner-verify/` for you to Read. `--reveal open|dock` drives the launcher/dock;
`--reveal clip|clip-detail|notif` force-open the OPTIONS boxes via the daemon's `debug-*` ctl
verbs (`waverunner-ctl debug-clip|debug-clip-detail|debug-notif`). The topbar is still
pointer-only — have the user hover it, then capture with `--no-build --no-restart`. Prefer a
tight `--geom "X,Y WxH"` crop over full-screen to save image tokens (the boxes hug the edges).
Design source of truth for the OPTIONS surfaces = the HTML mockups in `~/*-mockup`.

## Compositor
Custom **Hyprland 0.55.4 with a Lua dispatch API** (`hl.dsp.*`), not stock. Window actions live
under `hl.dsp.window.*`; wrong names fail silently. See `docs/hypr-api.md` before writing any
new `hypr::dispatch(...)` call.

## Where things live (jump, don't grep)
The big daemon files are cohesive but long — go straight to the function:
- `main.rs` — event loop, App struct + init, `refilter` (section routing), channel wiring.
- `options.rs` — OPTIONS topbar pills (window/clock/controls), hit-testing, `options_text_color`.
- `clipboard.rs` — clipboard OPTION: capture/serve worker, history, box + `push_clip_detail`
  (metadata detail view/animation), `serve_newest_clip` (keeps newest pasteable).
- `notif.rs` — notification OPTION: bell/DND, card rendering, footer, content preview.
- `renderer.rs` + `shaders/*.wgsl` — wgpu pipelines; frosted glass (`box_backdrop.wgsl`), SDF
  rounded rects, separable blur, neumorph/edge shadows, icon atlas.
- `animation.rs` — `lerp`, `ease_toward` (exp approach), `Follower` (damped spring / AGUA body).
- `install.rs` / `applier.rs` / `nix.rs` — declarative install flow + nixpkgs index.
- `hypr.rs` — all compositor IPC (dispatch + JSON reads).
- Engine (separate crate): `options-engine/src/{collectors,mind}` — the headless "Brain".
