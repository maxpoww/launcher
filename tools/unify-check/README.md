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
- `boxtest.sh <tag>` — opens the Recycle Bin box with a virtual click (wlrctl).
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

Do NOT put uprobes on the live compositor by raw address (`perf probe -x …
0x…`): it took the Acer's session down once. Sampling (`perf record`) and
`perf annotate` are enough and safe.

