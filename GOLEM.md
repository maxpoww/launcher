# Golem

> A Linux distribution with a soul. Golem (formerly "StandardOS") is not a
> theme on top of Linux — it's an operating system built around one idea:
> **OPTIONS**. Everything else exists to serve it.

This file is orientation, not law. It's here so any future session knows *what*
we're building and *how* we work, without re-deriving it. Treat it as a compass;
the code, `CLAUDE.md`, and the memory notes are the map.

---

## What we're building

A **distribution** — a complete, opinionated OS experience on NixOS + a custom
Hyprland, where the desktop is a single living surface rather than a pile of
apps and panels. Reproducible, declarative, and coherent end to end.

## OPTIONS — the soul

OPTIONS is **game-design philosophy applied to an OS**: dynamic, diegetic,
context-aware support that surfaces the *right tool at the right moment and
removes the rest*. It turns friction into revelation — the system notices what
you're doing and offers exactly what helps, then gets out of the way.

It is not a launcher, a menu, or a widget board. It's an ambient layer of
affordances (window controls, clipboard, notifications, and more to come) that
appear where they belong, move with intent, and never yank you out of flow. The
canonical definition lives in the manifesto (`~/Golem/Golem.md`, published at
golem-os.com); the motion/feel language lives in the `animation` skill.

## The Brain — `options-engine`

A **headless async Rust "Context Core"** that senses the system and decides what
OPTIONS should offer. Collectors (audio, git, media, hyprland, notifications,
selection, system, deploy) feed a mind (activity / affordance / decide /
session). It's the intelligence; the body subscribes to it. Built and
live-verified; not yet fully consumed by the daemon.

## The Body — `waverunner`

The persistent, GPU-rendered daemon the user actually sees: a macOS-style
auto-hiding **dock + launcher**, the **OPTIONS topbar** and its **boxes**
(clipboard, notifications), declarative **install** flows, dragging, thumbnails.
One layer-shell surface, one event loop, idle at rest. This repo *is* the body.

---

## Technologies

- **Rust** (edition 2021, clippy clean at `-D warnings`) — everything.
- **wgpu 24 + WGSL** — hand-rolled renderer: SDF rounded rects, separable blur,
  premultiplied frosted glass, neumorphic/edge shadows, an icon atlas.
- **Wayland / smithay-client-toolkit / wlr protocols** — layer-shell surface,
  data-control clipboard, screencopy colour-match.
- **calloop** — single event loop; workers talk back over channels, never block it.
- **glyphon** (text), **resvg** (SVG icons), **zbus/tokio** (D-Bus notifications).
- **Custom Hyprland 0.55.4** with a Lua `hl.dsp.*` dispatch API (see `docs/hypr-api.md`).
- **NixOS + flake** — reproducible builds; declarative package installs.

## Implementation layout (crates)

- `daemon` — the body (dock, topbar, OPTIONS boxes, install, render). The bulk.
- `options-engine` — the Brain (headless sensing + decision core).
- `options-notify` — OPTIONS' own `org.freedesktop.Notifications` D-Bus server.
- `core` — config, `.desktop` index, fuzzy search.
- `proto` / `client` — the `waverunner-ctl` socket protocol + CLI.

See `CLAUDE.md` "where things live" for the per-file map. Phased status:
**`~/Golem/roadmap.md`** (the living plan, S1–S11 + W-A);
`IMPLEMENTATION_PLAN.md` is the P1–P4 historical record.

---

## How we work (guidelines)

- **See it before you believe it.** UI/animation/material changes aren't done
  until looked at on the live session — use the `verify-ui` skill (screenshots,
  box-open `debug-*` verbs) and the `animation` skill (motion frame strips).
- **Motion is dt-based**, frame-rate independent; the 60/144 Hz tests stay green.
- **Mockups are the acceptance target** for OPTIONS surfaces (`~/*-mockup`).
- **Coherence over features.** Match the design language (3 flows, Shinings,
  flow-protection); the right tool at the right moment, remove the rest.
- **Safety at the OS edge.** Never `nixos-rebuild switch`/`sudo`/edit `/etc/nixos`
  or the flake unprompted — a bad generation can pin the boot. Own the fast, safe
  loops (`cargo build|clippy|test`, `verify-ui`, `grim`, `ctl`); leave the switch
  to the user.
- **Token economy.** Jump via the `CLAUDE.md` map instead of re-reading big files;
  prefer tight `--geom` screenshot crops; keep memory current so sessions start warm.

## Skills & tooling (`.claude/skills/`)

- **verify-ui** — build, restart the daemon, reveal a surface, screenshot it.
- **animation** — the motion design language + `capture-seq.sh` frame strips.
- **materials** — the wgpu/WGSL render architecture + shader-validation loop.
- **wayland-system** — Wayland/wlr/Hyprland IPC + NixOS install flow + safety.

Plus `docs/hypr-api.md` (the compositor's dispatchers) and the planning
directory **`~/Golem/`** (manifesto, roadmap, features/apps/android strategy,
per-section todos) — read `roadmap.md` + the current todo first.
