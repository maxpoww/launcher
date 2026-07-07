# waverunner

A rofi-like application launcher for Wayland/Hyprland, written in Rust.
Persistent daemon + layer-shell surface; the UI slides up from the bottom
edge with spring animation. See `CLAUDE.md` for design decisions and
`IMPLEMENTATION_PLAN.md` for the phased roadmap.

## Interaction model

Three rest states: **hidden → dock → open**.

- `waverunner-ctl toggle` slides a slim dock bar up from the bottom edge
  (toggle again from any state hides it).
- **Scroll on the dock** to smoothly expand it to the full popup;
  scroll the other way (or press Escape, or focus another window) to
  slide back down to the dock. Natural scroll direction by default
  (`[input] natural_scroll = false` for classic wheel direction).
- `expand` / `collapse` are also available as socket commands for
  scripting the same transitions.

## Build & run (NixOS)

```sh
nix develop                     # always enter the dev shell first
cargo build
cargo run -p waverunner-daemon  # needs a live Wayland (Hyprland) session
cargo run -p waverunner-client -- toggle
cargo test --workspace
cargo clippy --workspace -- -D warnings
```

## Hyprland keybind

```conf
exec-once = waverunner
bind = SUPER, SPACE, exec, waverunner-ctl toggle
```

## Configuration

`$XDG_CONFIG_HOME/waverunner/config.toml` — all keys optional:

```toml
[window]
width = 720
height = 420
input_bar_height = 48   # also the dock height

[animation.open]
kind = "spring"            # spring | ease-out-quart | ease-out-cubic | ease-in-cubic
spring_stiffness = 550.0
spring_damping = 42.0
spring_mass = 1.0

[animation.close]
kind = "ease-in-cubic"
duration_ms = 140

[theme]
background = "#1e1e2ecc"   # #rrggbb or #rrggbbaa
corner_radius = 24.0       # top corners only; bottom edge stays square

[input]
natural_scroll = true      # dock gesture follows natural scroll direction
```
