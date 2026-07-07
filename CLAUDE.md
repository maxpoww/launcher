# CLAUDE.md

Project context for Claude Code. Read this before making changes.

## Project: waverunner (working name)

An auto-hiding dock + launcher for Wayland/Hyprland, written in Rust.
Runs as a persistent daemon holding a layer-shell surface anchored to the
bottom edge of the screen. The dock is conceptually always present but
auto-hidden: touching the bottom screen edge with the pointer reveals a
slim dock bar; from there you click an icon to launch, scroll to expand
into the full search popup, or start typing to search. Moving the pointer
away auto-hides it again after a short grace period. All motion is fluid
spring/eased animation. `waverunner-ctl toggle|show|hide` over the control
socket remains as the hotkey-driven alternative path.

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
5. **Pointer reveal is surface-side.** While hidden, a thin input-region
   strip (a few px) stays alive at the bottom edge of the surface;
   pointer-enter on it reveals the dock, pointer-leave starts a grace
   timer that hides it again. No compositor plugins, no cursor polling.

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

Three rest points: `Hidden <-> Dock (slim bar) <-> Open (full popup)`.
Pointer edge-touch / `show` moves Hidden->Dock; scroll on the dock (or
`expand`) moves Dock->Open; Escape / focus loss collapse, pointer-leave
(after the grace timer) hides.

The animated quantity is `extent`: the visible content height in pixels,
growing up from the bottom edge. Each transition animates `extent` from
wherever it currently is toward the target rest point, so interrupting an
animation never causes a visual jump.

Per frame while animating:
```
progress = spring.step(dt)                    // damped spring, or eased
extent   = lerp(start_extent, target_extent, progress)
alpha    = clamp(extent / dock_extent, 0, 1)  // fades only below dock level
```
- Growing transitions: damped spring (slight settle, no big overshoot)
  or ease-out-quart.
- Shrinking transitions: faster, ease-in-cubic, ~120-160ms.
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

- [x] **P1 — Skeleton:** layer-shell surface on Hyprland, transparent,
      bottom-anchored. Validate the smithay-client-toolkit + wgpu combo
      works before anything else. *(accepted 2026-07-06 on a Hyprland VM,
      llvmpipe, scale 1 — recheck on real GPU + scale 2)*
- [~] **P2 — IPC:** socket server + `waverunner-ctl`; Hyprland keybind docs;
      target < 50ms keypress-to-visible. *(mash/malformed/SIGKILL-recovery/
      second-instance verified; latency measurement + keybind docs open)*
- [~] **P3 — Animation & dock interaction:** frame-callback loop,
      spring/easing, grow + fade, idle-at-rest; edge-reveal strip,
      scroll expand/collapse, pointer-leave auto-hide.
      *(working; visual pacing at 144Hz and scale-2 checks open)*
- [ ] **P4 — Launcher core:** .desktop indexing + cache, dock icon row,
      click-to-launch, nucleo fuzzy search, keyboard nav, exec + detach
      (setsid, close fds).
- [ ] **P5 — Polish:** icons everywhere, TOML theming (colors, radius,
      timings), focus-loss handling, fractional scaling.
- [ ] **P6 — Modes:** rofi-style prefixed modes (calc, ssh, clipboard).

Work strictly in phase order. Do not start a phase before the previous
one's acceptance criteria are met.

## Known risk areas (be careful here)

- smithay-client-toolkit + wgpu surface creation lifetimes (raw window
  handle plumbing). If stuck, look at `wgpu` examples and sctk's
  `simple_layer` example before inventing something.
- Keyboard focus with layer-shell `OnDemand` interactivity — focus-loss
  detection drives auto-collapse; test alt-tabbing and clicking other
  windows.
- "Start typing to search" needs keyboard focus without a click. `OnDemand`
  only grants focus on click; likely needs `Exclusive` while Dock/Open is
  visible (which steals keys from the focused window). Decide in P4.
- Fractional scaling on Hyprland: all pixel math must respect the surface
  scale factor, or text will blur.

## Permissions & workflow expectations

- Always show diffs and ask before applying file changes (maintainer
  preference: explicit control over edits).
- Never run `git push`, never amend commits.
- Commit messages: conventional commits (`feat:`, `fix:`, `refactor:`),
  scoped by crate, e.g. `feat(daemon): add spring animation stepper`.
