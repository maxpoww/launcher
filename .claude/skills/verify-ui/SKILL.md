---
name: verify-ui
description: See the waverunner UI after a change — build, restart the live daemon, optionally reveal the dock/launcher, and screenshot the screen so the result can actually be looked at. Use whenever a change affects rendering, layout, animation, colors, the dock, the launcher/install grid, the topbar, or an OPTIONS box, and you'd otherwise have to say "I can't visually verify."
---

# verify-ui

Closes the visual feedback loop. waverunner has no headless renderer, so the
only way to check a UI change is to run it on the live Hyprland session and
look. This skill does that end to end.

## How to run it

```
.claude/skills/verify-ui/verify-ui.sh [--build] [--restart] [--reveal dock|open|none] [--geom "X,Y WxH"] [--out PATH]
```

Defaults: `--build --restart --reveal none`. It prints the screenshot path on
the last stdout line — **Read that PNG** to inspect the result.

Common invocations:
- Whole screen after a change: `verify-ui.sh` (build + restart + full screenshot)
- The launcher / install grid: `verify-ui.sh --reveal open`
- Just the dock sliver: `verify-ui.sh --reveal dock`
- The clipboard box: `verify-ui.sh --reveal clip`
- The clipboard metadata detail view: `verify-ui.sh --reveal clip-detail`
- The notification box: `verify-ui.sh --reveal notif`
- A tight region (less to look at, cheaper): `verify-ui.sh --geom "0,0 900x220"`
- Screenshot only, don't disturb the running daemon: `verify-ui.sh --no-build --no-restart`

`--reveal clip|clip-detail|notif` drive the daemon's `debug-*` ctl verbs
(`waverunner-ctl debug-clip` etc.), which force-open the OPTIONS surfaces so
they can be captured without pointer input.

**Token economy:** a full-screen PNG is a large image to read back. Once you
know where a surface sits, prefer a tight `--geom "X,Y WxH"` crop — the OPTIONS
boxes hug the screen edges, so e.g. the left-edge clipboard box is roughly
`"0,0 380x480"`. Capture the whole screen only when you don't yet know the
layout. Reuse one `--out` path instead of accumulating shots.

## What it does / guarantees

- Builds with `nix develop -c cargo build`. **If the build fails it stops and
  leaves the running daemon alone** — your desktop keeps working.
- Restarts only the `target/debug/waverunner` binary, relaunched detached via
  `waverunner-dev` (same LD paths Hyprland uses). Verifies it came back up.
- Never runs `nixos-rebuild`, `sudo`, or anything that can affect the system or
  boot. Screenshots go to `/tmp/waverunner-verify/`, never the repo.

## Reaching the pointer-driven surfaces

`waverunner-ctl` drives the dock/launcher (`--reveal dock|open`), but the
OPTIONS **boxes** (clipboard, notifications) and the **topbar** are opened by
pointer hover and can't be triggered from the CLI on this compositor (no
input-sim tool, Lua-dispatch hyprctl). To verify those:

1. Ask the user to hover the relevant edge/pill to open the box, then run
   `verify-ui.sh --no-build --no-restart` to capture it; **or**
2. If a deterministic capture is needed often, add a debug verb to
   `waverunner-ctl` / the daemon that opens a given OPTION at a fixed state,
   and extend this skill's `--reveal` cases to call it.

## After capturing

Read the PNG, describe what you actually see, and compare against the design
intent (the HTML mockups in `~/*-mockup`, and the OPTIONS UX guidelines). If it
doesn't match, iterate — don't claim a visual change works until you've looked.
