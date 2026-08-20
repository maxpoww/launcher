---
name: wayland-system
description: Work on waverunner's system-level integration — the Wayland layer-shell surface, wlr protocols (data-control clipboard, screencopy colour-match), Hyprland IPC / the hl.dsp Lua dispatch API, the calloop event loop, D-Bus notifications, or the declarative NixOS install flow. Use whenever a change touches hypr.rs, clip_source.rs, screencopy.rs, ipc.rs, install.rs/applier.rs/nix.rs, the flake, or anything that talks to the compositor or the OS.
---

# wayland-system

waverunner is a single persistent daemon on a fixed layer-shell surface, driven
by one calloop event loop. All shared state lives on that loop; background work
(app index, nix, install, thumbs, clipboard) runs on worker threads that talk
back only via calloop channels. Never block the loop.

## Compositor (custom Hyprland 0.55.4, Lua dispatch API)

- Actions go over the control socket as `dispatch <lua>`; window ops live under
  `hl.dsp.window.*`. **Wrong names fail silently** — always check
  `docs/hypr-api.md` (verified dispatchers + signatures) before writing a new
  `hypr::dispatch(...)`. The authoritative source is
  `/nix/store/*-hyprland-0.55.4/share/hypr/hyprland.lua`.
- Reads are JSON: `request("j/activewindow")` etc. (see `hypr.rs`).
- `hyprctl dispatch` from a shell does **not** accept classic syntax here
  (it wraps args in Lua) — drive the daemon via `waverunner-ctl` instead.

## wlr protocols in use

- **data-control** (`clip_source.rs`) — own the selection with *all* MIME types
  at once (files paste into thunar *and* editors). We only ever serve; every
  incoming event is ignored except `send` (write bytes) / `cancelled` (lost it).
  The daemon keeps the newest clip served so it stays pasteable (`serve_newest_clip`).
- **screencopy** (`screencopy.rs`) — sample a maximized window's top row so the
  OPTIONS bar can colour-match it (the smart-gaps "one surface" look).
- **layer-shell** — the fixed surface; animate content, never resize it. Layer
  rules for the `waverunner` namespace (blur, ignore_alpha) live in
  `/etc/nixos/hyprland.lua`.

## Debugging

- Daemon logs: `/tmp/waverunner-verify/daemon.log` after a `verify-ui` restart
  (tracing output; set `RUST_LOG=debug` in `waverunner-dev` for more).
- Protocol traffic: `WAYLAND_DEBUG=1` in front of the daemon to trace requests/events.
- Compositor introspection: `hyprctl clients -j`, `hyprctl layers`, `hyprctl monitors`.
- Control the daemon for repro: `waverunner-ctl <toggle|show|expand|debug-clip|debug-notif|…>`.

## Declarative install flow (the distribution's package mechanism)

`packages.list` (one nixpkgs attr/line) → a root `systemd.path` runs
`waverunner-apply`, which generates root-owned `/etc/nixos/waverunner-packages.nix`
and `nixos-rebuild switch`es. Webapps: `managed_webapps` → `waverunner-webapps.nix`.
See `install.rs` / `applier.rs` / `nix.rs`.

## SAFETY (hard limits — do not cross unprompted)

- **Never** run `nixos-rebuild switch`, `sudo`, or write `/etc/nixos/*` on the
  live host on your own — a bad generation can pin the boot (this is why the
  `deploy-health` provider exists). Build/verify in the flake or a VM; leave the
  actual switch to the user.
- **Never** edit the flake (`flake.nix`/`flake.lock`) casually — a broken flake
  blocks every build. Propose the change, let the user apply.
- Safe fast loops you own: `nix develop -c cargo build|clippy|test`, `verify-ui`,
  `grim`, `waverunner-ctl`, `hyprctl` reads.
