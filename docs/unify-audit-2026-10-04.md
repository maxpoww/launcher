# Unifying the shell — audit and first pass (night of 2026-10-04)

Brief (Max): make the shell lighter, smarter, faster and simpler **without
changing the UI/UX**, and list what is overkill for a 1.0.

Everything here lives on the branch `night/unify` (not on `main`). It was
tested on the Acer E5-573 (Vulkan, spinning disk, 3.8 GB) and, for the GL
path, on the MacBook Air. Every change was compared against the official dock
by pixel diff; the only differences ever seen were the clock and the live
readout numbers — and one bug fix (below).

## Results

Idle desktop, nobody touching the machine.

| | Acer before | Acer after | MacBook before | MacBook after |
|---|---|---|---|---|
| dock CPU | 1.78 % | 0.25 % | 3.25 % | 0.38 % |
| dock wakeups / s | 664 | 48 | 492 | 34 |
| Hyprland CPU | 1.03 % | 0.10 % | 1.52 % | 0.28 % |
| Hyprland requests / s | 29 | 5.6 | — | — |
| full-screen captures / min | ~100 | 2 | — | — |
| dock GPU memory | 677 MB | ~185 MB | 164 MB | 140 MB |
| dock RSS | 135 MB | 101 MB | 181 MB | 148 MB |
| MemAvailable | 2.19 GB | 2.83 GB | — | — |
| threads | 28 | 24 | 26 | 24 |
| cold dock start (HDD) | 13.6–14.4 s | 12.8 s | — | — |

GPU memory is NOT in a process's RSS: read it from
`/proc/<pid>/fdinfo/*` (`drm-resident-system0`). On Intel graphics it is
ordinary, unswappable RAM.

## What was duplicated or wasted, and what was done

1. **Three GPU devices** — dock, OPTIONS bar and deck each opened their own
   wgpu instance/adapter/device. → one `SharedGpu`; the others clone its
   handles (`renderer.rs`). 677 → 381 MB.
2. **128 MB memory blocks** — wgpu's default hint is sized for games. →
   `MemoryHints::MemoryUsage`. 317 → 181 MB.
3. **Blur textures always allocated** on every surface; only the dock uses
   them, and only while a box is open. → built on demand.
4. **Icons kept twice** — every rasterised tile stayed in RAM next to a disk
   cache holding the same tiles. → RAM only for tiles the disk does not hold.
5. **The same question asked 5 times** — one layout re-evaluation asked
   Hyprland `j/monitors` ×5 and `j/clients` ×4, on two timers and every
   event. → `hypr::SNAPSHOT`: one reply per query per turn of the loop.
6. **Two timers doing the same re-evaluation** (700 ms colour poll, 800 ms
   zone poll). → the zone poll carries both.
7. **The screen redrawn and read back forever** — the colour-match captured
   the whole output every 700 ms, and Hyprland redraws the entire monitor for
   the first frame of a capture session (a session ends 500 ms after its last
   frame, so every capture was a first frame). → a *sentinel* capture armed
   right after each delivery waits for real damage; at rest nothing is drawn
   or read.
8. **The brain's audio sensor** forked `pw-dump` + `wpctl` every 2 s although
   `pw-mon` already says when something changes (590 wakeups/s). →
   event-driven, one confirming probe after the probes' own echo, 60 s net.
9. **Sensors nothing reads** — the shell now starts 5 collectors
   (`Engine::start_shell`), not 12; the system sensor skips the `/proc`
   sweeps for camera/recorder/trash fields nothing reads.
10. **A thread waking every second** to look at a flag (`deck_audio`) → parked.

Bug found on the way (fixed, the one visible change): **notification avatars
were black squares on every GL machine** (MacBook, ASUS). A one-layer texture
array is a plain 2D texture to wgpu's GL backend. Padded to two layers.

## Round 2 (same day)

On top of `main` with the modules work (`cb7c07c`), same method, baseline =
that commit's own build:

- **Pipelines, font system, glyph-image cache** built once for all surfaces
  (was once per renderer). RSS after the same exercise 125 → 94 MB; first
  adapter → all surfaces up 3.6 → 2.7 s cold.
- **Icon array grows on demand** (GPU-side copy, steps of 16) instead of
  reserving 113 empty layers. Dock GPU memory at rest 190 → 160 MB; it
  grows back toward the old size only once thumbnails or minimized windows
  need the far layers.
- **`reply_json`**: 26 Hyprland readers share one parse per query per turn.
- **`schedule_tick`**: the nine identical frame schedulers are one (−100
  lines).

Verified on the Acer: 8 states + Recycle Bin box pixel-identical, the 14
deep scenarios (suspend, scale, fullscreen, overview/spread, stage, minimize,
workspaces, lock, notifications, clipboard, dpms, dock kill, compositor
crash), window memory (move/close/reopen). GL path (MacBook): launcher,
minimized thumbnail and icons after a growth are correct.

Looked at and deliberately NOT merged: the 14 XDG path lookups (they differ
in empty-variable and fallback handling — a merge can move a path for no
gain) and the duplicated ease/lerp helpers (the copies skip `reduce_motion`;
unifying them changes behaviour for reduce-motion users — a decision).

## Still duplicated — candidates, in the order I would take them

| # | What | Gain | Risk |
|---|---|---|---|
| 1 | The 800 ms zone poll exists only because float moves emit no Hyprland event. The plugin already sees drags: one "geometry changed" verb from it removes the last idle timer (5.6 requests/s → 0). | idle → ~0 | plugin code (a crash there is the session) |
| 2 | ~~Icon array reserve~~ — done in round 2. | | |
| 3 | ~~Pipelines, font system~~ — done in round 2 (the glyph ATLAS is still per renderer). | | |
| 4 | ~~Hyprland readers~~ — one shared parse in round 2; typed structs instead of `serde_json::Value` remain possible. | | |
| 5 | Media (1 s) and Bluetooth (3 s) are polled over D-Bus; both have signals. | brain idle → ~0 | medium (progress bar timing) |
| 6 | `schedule_*_frame` is copied 9 times; XDG path helpers 14 times; `lerp` ×3; the exp-approach ease ×5 (the copies skip `reduce_motion` — an accessibility bug). | simpler | low |
| 7 | The OPTIONS surface is always 510 px tall for a 28 px bar: its swapchain and scene texture are ~18× the visible strip. | −30 MB | high (surface sizing is a non-negotiable) |
| 8 | Plugin and daemon both sample the window-top colour (plugin from the texture, daemon by screen capture); four capture flows in the plugin; the minimized set lives in three places. | simpler | plugin code |
| 9 | Initrd activation runs two cold Perl scripts (~4 s on the Acer); the desktop waits for NetworkManager behind the splash (~8 s). See Golem `work/parity.md`. | boot | visual handoff |

## Overkill for 1.0 — decisions for Max

Nothing here was removed. Each item is code that runs or ships and feeds
nothing a user sees today.

1. **The Mind** (`options-engine/src/mind`, ~1,500 lines + the option-pill
   path in the daemon). `PROVIDERS` is empty, so the option set is always
   empty; activity/settle/session compute for a log line. Either the first
   real providers get written, or it goes behind a feature flag.
2. **Seven sensors** (git, bridge, selection, deploy, notifications,
   downloads, daylight — ~1,400 lines). Not started by the shell any more;
   still in the crate. Keep only those a planned OPTION needs.
3. **The sunset prompt** is only reachable through `debug-sunset`: nothing
   emits it. The module box and the "automatically at sunset" toggle are
   separate and do work.
4. **Config knobs no Golem sets**: `[window]`, `[animation]` curves,
   `[input]`, several `[theme]` and `[options]` fields; `theme.highlight` has
   no reader at all. Hardwire, keep `[accessibility]`.
5. **Debug verbs tied to the empty Mind**: `debug-options`,
   `debug-hover-option`, `options-trigger`. `stage-show`, `stage-mode`,
   `float-mode` have no caller in Golem.
6. **Small dead items**: `managed_webapps::slugs`, the generated
   `waverunner-webapps.nix` nothing imports, `trash::{is_empty, erase,
   restore}` (tests only), the unused `notification_closed` reason, an empty
   swipe dispatch, `thiserror` in options-engine.
7. **Two settings stores** (`~/.local/share/waverunner/settings.json` with
   one bool, `~/.config/golem/settings.json`) plus three one-value files.

## How it was verified

Scripts (in the session scratchpad, to be moved into `tools/` if wanted):
build the branch through Golem's flake with `--override-input`, swap the dock
on the laptop with a runtime drop-in (gone at reboot), then

- per-thread CPU and wakeups over 60 s; GPU memory from fdinfo;
- 8 shell states (rest, dock, launcher, stats, clipboard, notifications,
  panel, fast launch) + the Recycle Bin box (frosted backdrop) → pixel diff;
- a tiled terminal changing its background with no layout event → the bar
  follows in the same time as before;
- a rescan (a `.desktop` file added and removed) → same icons;
- notification, clipboard, stage on/off → pixel diff;
- the audio sensor traced: no probe at rest; a stream start/stop or a volume
  change seen in ~0.1 s.

Not verified: multi-monitor, NVIDIA, AMD, fullscreen video (direct scanout),
a long session. The ASUS (GL + a failing second GPU) was off.
