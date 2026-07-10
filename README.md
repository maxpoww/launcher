# waverunner

A macOS-style auto-hiding dock and app launcher for Wayland/Hyprland,
written in Rust. One persistent daemon, one fixed layer-shell surface,
GPU-rendered (wgpu), fully idle at rest. See `CLAUDE.md` for design
decisions and `IMPLEMENTATION_PLAN.md` for the phased roadmap.

## What it does

The whole UI is one rounded "card" that slides up from the bottom edge:

- **Edge reveal** — touch the bottom edge of the screen with the pointer
  and the dock (the card's top sliver, a row of app icons) rises.
- **Intellihide** — the dock parks visible while no window overlaps its
  zone and dodges out of the way when one does (via Hyprland IPC;
  degrades to plain auto-hide elsewhere).
- **Magnification** — icons swell under the cursor with cosine falloff,
  in the dock row and in the grid.
- **Scroll to open** — scrolling on the dock raises the full card: a
  launchpad-style app grid with names, plus a search box.
- **Type to search** — the open card takes the keyboard; typing filters
  the grid live (nucleo fuzzy matching), the best match is
  auto-selected, Enter or a click launches it (with a launch bounce),
  Escape clears then collapses.
- **Self-updating** — the app index rescans whenever the dock is
  summoned, so installs/uninstalls show up without a restart. Icons
  come from your freedesktop icon theme (SVG and PNG).
- Launched apps are fully detached (`setsid`, double-fork): they
  survive daemon restarts.

## Build & run (NixOS)

```sh
nix develop                     # always enter the dev shell first
cargo build
cargo run -p waverunner-daemon  # needs a live Hyprland session
cargo run -p waverunner-client -- toggle
cargo test --workspace
cargo clippy --workspace -- -D warnings
```

## Hyprland integration

The daemon never grabs global hotkeys; bind the CLI client instead:

```conf
exec-once = waverunner
bind = SUPER, SPACE, exec, waverunner-ctl toggle
```

`waverunner-ctl` speaks `toggle | show | hide | expand | collapse` over
a Unix socket in `$XDG_RUNTIME_DIR`. Pass `--time` to print the
command round-trip (the daemon draws the first frame before it
responds, so this is command-to-first-frame; ~5–8 ms typical).

## Configuration

`$XDG_CONFIG_HOME/waverunner/config.toml` — all keys optional:

```toml
[window]
width = 720
height = 560            # full card height
input_bar_height = 48   # dock sliver height
bottom_margin = 12      # gap under the fully risen card

[animation.open]
kind = "spring"         # spring | ease-out-quart | ease-out-cubic | ease-in-cubic
spring_stiffness = 550.0
spring_damping = 42.0
spring_mass = 1.0

[animation.close]
kind = "ease-in-cubic"
duration_ms = 140

[theme]
background = "#1e1e2ecc"   # #rrggbb or #rrggbbaa
corner_radius = 24.0
text = "#e6e6efff"
highlight = "#ffffff26"
icon_theme = "hicolor"     # e.g. "Papirus-Dark"

[input]
natural_scroll = true      # dock gesture follows natural scroll direction
edge_reveal = true
edge_reveal_px = 5         # pointer-sensitive strip while hidden
autohide = true
autohide_delay_ms = 300
intellihide = true         # park while no window overlaps the dock zone
```

With home-manager, manage it declaratively:

```nix
xdg.configFile."waverunner/config.toml".text = ''
  [theme]
  icon_theme = "Papirus-Dark"
'';
```

## Status

Core launcher (P1–P4) works and is verified live on Hyprland: dock,
grid, search, launch, animations, intellihide, persistent icon cache
(cold start ~26 ms warm), terminal apps, drag-and-drop dock pinning,
and the three-section popup (Apps / Install / Files). Open work: real
GPU + scale-2 + 144 Hz visual checks, package search for the Install
section, fractional scaling, theming polish, and prefix modes (see
the plan).
