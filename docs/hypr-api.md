# Hyprland dispatch API (this fork) — quick reference

This machine runs a **custom Hyprland (0.55.4) with a Lua config + dispatch
API**, not stock Hyprland. Dispatchers are `hl.dsp.<name>(...)` and most window
actions live under the **`hl.dsp.window.*`** namespace. Getting the name wrong
fails *silently* (see the `closewindow` → `window.close` bug, 2026-08), so use
this list rather than guessing from stock-Hyprland memory.

Send one over the control socket as `dispatch <lua>` (see
`crates/daemon/src/hypr.rs::dispatch`). From a shell, `hyprctl dispatch` wraps
args in Lua and does **not** accept the classic `movecursor 5 5` syntax.

## Verified dispatchers (in use, confirmed working)

| Lua call | What it does |
|---|---|
| `hl.dsp.window.close({ window = "address:0x…" })` | Close a window by address. `hl.dsp.window.close()` closes the focused one. |
| `hl.dsp.window.float({ action = "toggle" })` | Toggle floating on the focused window. |
| `hl.dsp.window.pseudo({ action = "toggle"\|"on"\|"off", window? })` | Pseudotile the focused (or given) window. **Prefer explicit on/off** — pseudo state is NOT readable anywhere (not in clients JSON, not on HL.Window), so toggles are blind. The Golem pill routes through `golemPseudoToggle()` in hyprland.lua (tag = state). |
| `hl.dsp.window.resize({ x, y, relative?, window? })` | Resize a window. Absolute by default (delta computed against the goal size); `relative = true` for deltas (negatives allowed there only). On a **pseudo** window this sets the pseudo size (clamped to the tile); on a tiled window it resizes the layout node. GOTCHA: `clients -j` `size` reports the mid-ANIMATION value — sleep ~1s before reading back. |
| `hl.dsp.window.tag({ tag = "+name"\|"-name"\|"name", window? })` | Set/unset/toggle a window tag. Tags show in `clients -j` and are matchable in `hl.window_rule` (`match = { tag = "name" }`, no negation). Dynamic rule props (rounding, border_size) re-apply on tag flips; static ones (pseudo, size) apply at map only. |
| `hl.dsp.window.fullscreen({ action = "toggle" })` | Toggle fullscreen on the focused window. |
| `hl.dsp.window.move({ workspace = N \| "special:magic" })` | Move focused window to a workspace. |
| `hl.dsp.window.resize()` / `hl.dsp.window.drag()` | Interactive mouse resize / move (bound to mouse). |
| `hl.dsp.window.alter_zorder(...)` | Change stacking order. |
| `hl.dsp.focus({ window = "address:0x…" })` | Focus a window by address. |
| `hl.dsp.send_shortcut({ mods = "…", key = "…", window = "address:0x…" })` | Inject a keystroke into a window (used for paste). |
| `hl.dsp.workspace.toggle_special(...)` | Toggle the special (scratchpad) workspace. |
| `hl.dsp.exec_cmd("…")` | Run a command. |
| `hl.dsp.layout(...)` / `hl.dsp.exit()` | Layout op / exit compositor. |

Option tables use named fields (`{ action = … }`, `{ window = … }`,
`{ workspace = … }`) — positional args are not the convention here.

## Reads (JSON over the control socket)

`request("j/…")` returns JSON (see `hypr.rs`):
- `j/activewindow` → focused window `{ address, class, title, fullscreen, … }`
  (`fullscreen >= 2` = true fullscreen covering the bar).
- Address `"0x0"` / empty ⇒ nothing focused (empty workspace).

## Layer rules for waverunner (from `/etc/nixos/hyprland.lua`)

```lua
hl.layer_rule({ match = { namespace = "waverunner" }, blur = true })
hl.layer_rule({ match = { namespace = "waverunner" }, ignore_alpha = 0.5 })
hl.exec_cmd("/home/max/launcher/waverunner-dev")   -- launches the daemon
```

## If you need a dispatcher not listed here

Check the compositor's own example config, which is authoritative for this
fork's API surface:

```
/nix/store/*-hyprland-0.55.4/share/hypr/hyprland.lua
```

Grep it for the real name/signature before writing a new `dispatch(...)` call.
