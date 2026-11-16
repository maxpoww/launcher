# waverunner

The shell of **Golem**: one persistent daemon that draws the dock, the top
bar and everything that opens from them. Rust, Wayland layer-shell surfaces on
Hyprland, GPU-rendered (wgpu), idle at rest.

It started as a dock and launcher (the name of the repo, `launcher`, is from
then). Read `GOLEM.md` for what Golem is and how the work is done, and
`CLAUDE.md` for the code: every part of the shell has its section there.
`IMPLEMENTATION_PLAN.md` is the plan of the first weeks (August 2026) and is
kept as history; it does not describe what is here now.

## What is in it

- **The dock** at the bottom edge: pinned and running apps, groups that open
  in a line, the bin, minimized windows; it hides when a window needs the
  space.
- **The box** that rises from the dock: every app in a grid, search as you
  type, the Install section (nixpkgs packages, webapps and whole modules,
  installed by dragging), files, and Controls (the settings pages).
- **The top bar**: the current window's own controls, the clock, the gear's
  quick settings (Wi-Fi, Bluetooth, sound, power, battery, storage),
  notifications, the clipboard with its history, and what the machine offers
  by itself (OPTIONS).
- **Golem Files**, Quick Look, the desktop's icons, the card (notes, emoji,
  dictation), the box terminal.
- **The look**: palettes, lighting that follows the day, every app themed to
  match.
- **Seven languages** besides English: es, es_ES, pt, pt_PT, fr, de, it
  (`crates/daemon/assets/i18n/`, compiled into the binary).

## Layout

| Crate | What it is |
|---|---|
| `crates/daemon` | the `waverunner` binary: the Wayland loop, the renderer, every surface |
| `crates/client` | `waverunner-ctl`, the command line that talks to the daemon |
| `crates/core` | config, the `.desktop` index, fuzzy search |
| `crates/proto` | the socket's message types |
| `crates/options-engine` | the headless engine: collectors (what the machine is doing) and the Mind (what to offer) |
| `crates/options-notify` | the notification server (`org.freedesktop.Notifications`) behind the bell |

`nixos/` holds the privileged helper for installs (`nixos/README.md`),
`tools/` the translation scripts (`tools/i18n/README.md`), the performance
guards and other checks.

## Build and test

```sh
nix develop -c cargo build
nix develop -c cargo clippy --workspace -- -D warnings   # kept clean
nix develop -c cargo test --workspace
```

The tests cover the logic (layout, search, stores, translations: a string
without its translation fails). What a surface looks like is checked by
running it, in a nested Hyprland so the live desktop is not touched.

## Running it

- **On an installed Golem** the shell comes from this flake:
  `homeManagerModules.default` (`programs.waverunner`) installs the daemon,
  `waverunner-ctl` and the package index and starts the daemon with the
  session; `nixosModules.notification-service` runs the notification server.
  Golem's home layer (`~/Golem/system/home/home.nix`) wires both.
- **On the development machine** Hyprland starts `./waverunner-dev`, which
  runs `target/release/waverunner` with the libraries the flake names, and
  the key binds call `target/debug/waverunner-ctl`. So `cargo build` gives a
  new `waverunner-ctl` at once; a new daemon needs a release build and a
  restart of the dock.

The daemon takes no global keys. Hyprland binds `waverunner-ctl <command>`;
`waverunner-ctl --help` lists the commands (`toggle`, `expand`,
`control-panel`, `focus-next`, … and the `debug-*` ones the test rigs use).
`--time` prints the round trip of a command.

## Installing apps

Installs are declarative. The dock edits one file the owner can write,
`~/.config/waverunner/packages.list` (one nixpkgs attribute a line); a root
`systemd.path` unit sees the change and rebuilds the system with that list.
Nothing is installed imperatively, and an install that fails leaves the system
as it was. `nixos/README.md` has the whole path.

## Configuration

`$XDG_CONFIG_HOME/waverunner/config.toml`, every key optional. Most of what a
person changes is not here but in Controls, which writes
`~/.config/golem/settings.json`; the file is for the few things with no
control yet (`CLAUDE.md` names each key where its feature is described).

```toml
[theme]
icon_theme = "Papirus-Dark"

[options]
link_unfurl = false   # fetch a copied link's title and picture (off: no network)
```
