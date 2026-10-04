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
