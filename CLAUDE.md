# CLAUDE.md

Project context for Claude Code. Read this before making changes.

## Project: waverunner (working name)

A rofi-like application launcher for Wayland/Hyprland, written in Rust.
Runs as a persistent daemon holding a layer-shell surface anchored to the
bottom edge of the screen. On hotkey toggle, the UI slides up from the bottom
and grows from a slim input bar into a full result list, with fluid
spring/eased animation. Auto-hides on focus loss, Escape, or selection.

## Non-negotiable design decisions (already made — do not revisit)

1. **Persistent daemon model.** No cold process spawn per invocation.
   A Unix domain socket receives `toggle` / `show` / `hide` commands.
2. **Fixed-size layer-shell surface.** The surface is created once at max
   popup size, anchored bottom, transparent background. Animations translate
   and clip the *content drawn inside* the surface. We never resize the
   Wayland surface per frame (compositor resize = jitter).
3. **GPU rendering via wgpu.** Frame-callback-driven render loop. Request
   Wayland `frame` callbacks only while animating or handling input; go
   fully idle (0% CPU, no frame requests) once settled.
4. **Hotkeys are compositor-side.** Hyprland keybind executes the CLI client
   which writes to the socket. The daemon never grabs global input itself.

## Tech stack

- **Wayland:** `smithay-client-toolkit` + `wlr-layer-shell` protocol
  (keyboard interactivity: `OnDemand`)
- **Rendering:** `wgpu`; UI layer with `egui` (wgpu backend) is acceptable
  for input/list widgets, but animation offsets are computed by our own
  animation engine, not egui's
- **Fuzzy matching:** `nucleo`
- **App discovery:** `freedesktop-desktop-entry` (+ `freedesktop-icons` later)
- **IPC:** Unix domain socket, `std::os::unix::net` (avoid pulling tokio in
  unless genuinely needed; the daemon is event-loop driven, not async-heavy)
- **Config:** `serde` + `toml`, loaded from
  `$XDG_CONFIG_HOME/waverunner/config.toml`

## Workspace layout

```
waverunner/
├── Cargo.toml            # workspace root
├── flake.nix             # NixOS dev shell (see Dev environment)
├── crates/
│   ├── daemon/           # main binary: wayland loop, surface, renderer
│   │   └── src/
│   │       ├── main.rs
│   │       ├── surface.rs     # layer-shell setup
│   │       ├── renderer.rs    # wgpu pipeline
│   │       ├── animation.rs   # spring/easing engine
│   │       ├── ipc.rs         # unix socket server
│   │       └── state.rs       # app state machine (Hidden/Opening/Open/Closing)
│   ├── client/           # tiny CLI: `waverunner-ctl toggle|show|hide`
│   ├── core/             # shared: config types, desktop-entry index, fuzzy search
│   └── proto/            # socket message types (shared by daemon + client)
```

## Animation model

State machine: `Hidden -> Opening -> Open -> Closing -> Hidden`.

Per frame while animating:
```
progress    = spring.step(dt)              // damped spring, or ease-out-cubic
y_offset    = lerp(surface_h, 0.0, progress)
content_h   = lerp(input_bar_h, full_h, progress)
opacity     = progress
```
- Open: damped spring (slight settle, no big overshoot) or ease-out-quart.
- Close: faster, ease-in-cubic, ~120-160ms.
- All timings/curves must be configurable in config.toml.
- Animation must be dt-based (frame-rate independent) — never per-frame
  fixed increments. Test at 60Hz and 144Hz.

## Dev environment (NixOS)

The maintainer runs NixOS + Hyprland. All tooling goes through the flake:

- `nix develop` provides: rust toolchain, `wayland`, `libxkbcommon`,
  `vulkan-loader`, `pkg-config`, and `RUSTFLAGS`/`LD_LIBRARY_PATH` needed
  for wgpu to find Vulkan at runtime.
- Do NOT suggest `rustup`, global `cargo install`, or apt/dnf packages.
  If a system library is missing, add it to `flake.nix` buildInputs.
- wgpu on NixOS commonly fails at runtime with missing `libvulkan.so` /
  `libwayland-client.so` — fix via `LD_LIBRARY_PATH` in the dev shell,
  not by hacks in code.

## Build / run / test

```
nix develop                 # enter dev shell first, always
cargo build                 # workspace build
cargo run -p daemon         # run the daemon (needs a Wayland session)
cargo run -p client -- toggle
cargo test --workspace
cargo clippy --workspace -- -D warnings
cargo fmt --all
```

Manual testing requires a live Hyprland session; there is no headless
compositor harness yet. When a change affects surface/rendering behavior,
describe what to verify visually instead of claiming it is tested.

## Coding conventions

- Edition 2021, `rustfmt` defaults, clippy clean at `-D warnings`.
- No `unwrap()`/`expect()` outside of `main.rs` startup and tests; use
  `anyhow` in binaries, `thiserror` in library crates.
- Wayland/wgpu resources: explicit ownership, no `Rc<RefCell<...>>` webs.
  Prefer passing `&mut State` through the event loop.
- Keep the daemon single-threaded except: desktop-entry indexing may run on
  a background thread and send results over a channel.
- Log with `tracing`; no `println!` outside the client crate.
- Every public item in `core` and `proto` gets a doc comment.

## Roadmap (phases)

- [ ] **P1 — Skeleton:** layer-shell surface on Hyprland, transparent,
      bottom-anchored, manual show/hide via stdin. Validate the
      smithay-client-toolkit + wgpu combo works before anything else.
- [ ] **P2 — IPC:** socket server + `waverunner-ctl`; Hyprland keybind docs;
      target < 50ms keypress-to-visible.
- [ ] **P3 — Animation engine:** frame-callback loop, spring/easing,
      slide + grow + fade, idle-at-rest behavior.
- [ ] **P4 — Launcher core:** .desktop indexing + cache, nucleo fuzzy
      search, keyboard nav, exec + detach (setsid, close fds).
- [ ] **P5 — Polish:** icons, TOML theming (colors, radius, timings),
      auto-hide on focus loss.
- [ ] **P6 — Modes:** rofi-style prefixed modes (calc, ssh, clipboard).

Work strictly in phase order. Do not start a phase before the previous
one's acceptance criteria are met.

## Known risk areas (be careful here)

- smithay-client-toolkit + wgpu surface creation lifetimes (raw window
  handle plumbing). If stuck, look at `wgpu` examples and sctk's
  `simple_layer` example before inventing something.
- Keyboard focus with layer-shell `OnDemand` interactivity — focus-loss
  detection drives auto-hide; test alt-tabbing and clicking other windows.
- Fractional scaling on Hyprland: all pixel math must respect the surface
  scale factor, or text will blur.

## Permissions & workflow expectations

- Always show diffs and ask before applying file changes (maintainer
  preference: explicit control over edits).
- Never run `git push`, never amend commits.
- Commit messages: conventional commits (`feat:`, `fix:`, `refactor:`),
  scoped by crate, e.g. `feat(daemon): add spring animation stepper`.
