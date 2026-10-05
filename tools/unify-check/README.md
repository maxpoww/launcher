# unify-check — the lab scripts of the 2026-10-04 "unify" night

Lab tools, as they were used: paths (the session scratchpad, the Golem
worktree) and the laptop's address (192.168.1.99, the Acer) are hard-coded.
See `docs/unify-audit-2026-10-04.md`.

- `builddock.sh [host]` — build this worktree's dock through Golem's flake
  (`--override-input waverunner`), copy it to the laptop, print the store path.
- `swapdock.sh <store path|old> [settle s]` — run ON the laptop (`ssh … 'bash -s' <`):
  point the dock's user service at that build with a runtime drop-in (gone at
  reboot), or back to the installed one.
- `idle.sh [s]` — per-thread CPU and wakeups of dock, Hyprland, options-notify.
- `gpumem.sh` — the dock's GPU memory (fdinfo), RSS, threads.
- `capcount.sh [s]` — screen captures at rest (Hyprland `screencast` events).
- `vis.sh` / `visrun.sh <tag>` — screenshots of 8 shell states; diff two tags
  with ImageMagick `compare -fuzz 3%`.
- `colortest.sh` / `ctrun.sh <tag>` — the bar follows a tiled terminal whose
  background changes with no layout event.
- `rescan.sh`, `soak.sh` / `soakrun.sh <tag>`, `audiotest.sh`, `coldstart.sh`.

## Round 3 (performance)

- `build2.sh [host]` — as `builddock.sh`, from Golem's `layer-damage` worktree
  (so the test system also has the patched Hyprland); roots what it builds
  here (`~/.cache/unify3-roots`) and on the laptop (`pushroot.sh`,
  `/nix/var/nix/gcroots/lab/`) — a nightly garbage collection took a whole
  night's builds once.
- `swapdock.sh <store path|old> [settle s] [ENV=VALUE …]` — now also sets
  environment for the test dock (`WAVERUNNER_DAMAGE_CHECK=1`,
  `WAVERUNNER_FULL_DAMAGE=1`, `VK_DRIVER_FILES=/nonexistent.json` to force GL).
- `swaphypr.sh <store path|old|keep>` — run the session on another Hyprland
  until the next reboot (a drop-in under `/run`), restart the session
  (greetd's autologin again) and make the compositor SCHED_RR as the
  installed one is.
- `work.sh <tag> [workloads]` / `wlrun.sh <dock> <tag> [workloads]` /
  `wldiff.sh <a> <b>` — the scripted workloads (idle, boxes, launcher, fast,
  panel, hover, typing, notifs, stage) with CPU, GPU, instructions and the
  daemon's counters per workload; end-state screenshots and their diff.
- `deep.sh <tag>` — 14 scenarios: suspend, scale, fullscreen, overview,
  stage, minimize, workspaces, lock, notifications, clipboard, dpms, dock
  killed, compositor crash, launcher after everything.
- `kms.sh <tag> [scenarios]` / `kmscmp.sh <tag>` / `detile.py` — the REAL
  screen (the KMS scanout buffer through `ffmpeg -f kmsgrab`, Intel X-tiling
  undone) after a run of partial-damage frames against the same after a
  forced full redraw.
- `leakprof.sh <n>` / `leak3.sh` — Hyprland's cost per frame before and after
  n screen recordings; its screencast announcements per recording.
- `ctlat.sh <tag>` — how soon the bar follows a window's colour.
- `tracehypr.sh on|off` — restart the session with `HYPRLAND_TRACE=1`.
- `kmscmp2.sh <tagA> <tagB>` — the real screen in the same still states
  under two builds or settings.
- `foreign.sh <tag> [one|all]` — what the shell's blurred layers cost while
  ANOTHER app draws (a terminal scrolling), with their blur rule on and off.
- `video.sh <tag> [s]` — a windowed 30 fps video under a resting shell:
  Hyprland CPU/GPU, GPU busy, captures taken.
- `boxtest.sh <tag>` — the Recycle Bin box (the frosted, two-step path) by a
  virtual click; needs something in the trash.
- `stageshot.sh <tag>` — the settled deck, three states, for old-against-new.
- `dockprof.sh <workload>` — where the dock's main thread goes (perf sampling).
- `threadcpu.sh <workload>` — CPU per thread of the dock AND of the helper
  processes it starts, with a whole-process profile.
- `steady.sh [s] [states…]` — every RESTING state of the shell, entered, left
  alone and measured (CPU, helpers, Hyprland, GPU busy, frames, processes
  started). `FULL=1` adds the daemon's counters and what was started.
- `stageidle.sh [s]` / `idlespawn.sh [s]` — stage mode, and the plain
  desktop, at rest: dock + helpers + PipeWire, processes started and by whom.
- `whodraws.sh <state> [s]` — captures woken in a resting state (run the dock
  with `RUST_LOG=info,waverunner::screencopy=debug`).
- `badge.sh <tag>` — the deck's speaker badge through a player starting, a
  one-second sound, a mute, an unmute, the end; photographs each step and
  times every probe. Needs a PulseAudio-API player (`mpv --ao=pulse`): a
  native PipeWire stream names no process.
- `audiowatch.sh` — the audio sensor after outside changes (volume, mute,
  default sink, a client that changes nothing) and with its watcher killed.
- `leak2.sh` — which kind of recording leaves the compositor slower.

Debug switches of the dock (set them with `swapdock.sh … ENV=VALUE`):
`WAVERUNNER_DAMAGE_CHECK=1|paths`, `WAVERUNNER_FULL_DAMAGE=1`,
`WAVERUNNER_NO_VISIBLE_REGION=1`, `WAVERUNNER_NO_SAMPLER=1`,
`VK_DRIVER_FILES=/nonexistent.json` (forces the GL backend).

Always wrap a remote test in `timeout`: a player started without the
session's D-Bus address hung an ssh for minutes (mpv's MPRIS script).

Do NOT put uprobes on the live compositor by raw address (`perf probe -x …
0x…`): it took the Acer's session down once. Sampling (`perf record`) and
`perf annotate` are enough and safe.

