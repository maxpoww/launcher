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

## Runtime config writes (how the dynamic window border works)

`hl.config` is callable over the socket, so the *colours* Hyprland draws
windows with can be changed live — that is how the daemon keeps the window
border matched to the shell (`options.rs::push_window_border`):

```lua
hl.config({ general = { col = { active_border = {
    colors = { 'rgba(RRGGBBff)', ... }, angle = 90 } } } })
```

`angle` is in degrees; `angle = 90` puts the FIRST stop at the top of the
frame (verified live 2026-09-11). Read the value back with `hyprctl getoption
general:col.active_border`.

**Ten stops is the ceiling, and the config will not tell you so.** The
parser accepts more — twelve set and stored fine in a live test — but the
border shader's uniform is `vec4 gradient[10]` (`src/render/shaders/glsl/
border.glsl`), so anything past the tenth is more than the renderer can
read. Stay at or under 10.

A plugin sees the same object via
`CConfigValue<Config::IComplexConfigValue>("general:col.active_border")` —
that is how waveview's overview rings inherit the border's paint.

## `hl.dsp.cursor.*` argument shape (verified live 2026-09-11)

`move` takes a **named table**, not positional args: `hl.dsp.cursor.move({ x =
900, y = 700 })`. `hl.dsp.cursor.move(900, 700)` errors with "expected a table
{ x, y }", and the array form `{ 900, 700 }` errors with "'x' is required" —
both loudly, unusually for this API.

## The COMPLETE `hl.dsp.window.*` surface (introspected 2026-09-04)

Enumerated live, so this is exhaustive — if a name is not here it does not exist:

```
alter_zorder  bring_to_top  center      clear_tags  close     cycle_next
deny_from_group  drag       float       fullscreen  fullscreen_state
kill          move         pin         pseudo      resize    set_prop
signal        swap         tag         toggle_swallow
```

And the top-level `hl.dsp.*` namespaces:

```
cursor  dpms  event  exec_cmd  exec_raw  exit  focus  force_idle
force_renderer_reload  global  group  layout  no_op  pass
send_key_state  send_shortcut  submap  window  workspace
```

**There is NO absolute-position dispatcher.** No `position`, no pixel move. `move`
takes a *workspace*; `center` centres; `resize` sets a size. The C++ dispatchers
`movewindowpixel` / `resizewindowpixel` / `movetoworkspacesilent` do exist in the
binary, but **the Lua layer does not expose them** — so a window cannot be placed
at an arbitrary x/y from here. Anything needing an exact rect must come from the
compositor's own layout instead: layer-surface **exclusive zones** + `gaps_out`
(+ `fullscreen_state` maximize), not pixel math.

`hl.config`, `hl.animation`, `hl.workspace_rule` and `hl.monitor` are all plain
functions and are **callable at runtime** over the socket, so gaps, animations and
workspace rules can be changed live and put back.

## Introspecting the API yourself (read-only, no side effects)

`eval` runs arbitrary Lua but its **return value is not sent back** (you just get
`ok`). Lua `io` works, so write the answer to a file and read it:

```sh
SOCK=$XDG_RUNTIME_DIR/hypr/$HYPRLAND_INSTANCE_SIGNATURE/.socket.sock
printf '%s' "eval local f=io.open('/tmp/x','w')
  local t={} for k,v in pairs(hl.dsp.window) do t[#t+1]=k end
  table.sort(t) f:write(table.concat(t,', ')) f:close()" | socat - "UNIX-CONNECT:$SOCK"
```

Argument *shapes* still can't be introspected — `share/hypr/stubs/hl.meta.lua`
types every dispatcher as `fun(...)`. Unknown signatures need one live test on a
scratch window; never guess, because a wrong field fails silently.

## If you need a dispatcher not listed here

Check the compositor's own example config, which is authoritative for this
fork's API surface:

```
/nix/store/*-hyprland-0.55.4/share/hypr/hyprland.lua
```

Grep it for the real name/signature before writing a new `dispatch(...)` call.
