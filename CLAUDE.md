# CLAUDE.md — waverunner

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
- Installs (packages) recorded declaratively in `~/.config/home-manager/waverunner-packages.nix`.

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
