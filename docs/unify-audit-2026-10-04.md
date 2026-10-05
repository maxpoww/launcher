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

## Round 3 (2026-10-04/05) — performance

Brief (Max): "study the unification, and improve it. optimize it. we want
higher performance." Same rules: no UI/UX change, the Acer as the test
machine, nothing on `main` without his go. Branch `unify-3`; the Hyprland
side is Golem's branch `layer-damage`.

This round measured first. The daemon got always-on counters
(`waverunner-ctl debug-perf`: frames per surface, shapes, Hyprland requests,
captures, damage) and a scripted workload runner (`tools/unify-check/work.sh`)
that reads, per workload: dock and Hyprland CPU, each one's GPU time (the
kernel's per-client `drm-engine-render`), whole-GPU busy (i915 PMU) and
instructions (CPU seconds lie when the governor changes the clock).

### What the measurements showed

1. **Surfaces drew above the display's rate.** The OPTIONS bar rendered 125
   frames a second (its animations tick on 8 ms timers and every state change
   drew), the dock ~95 (a draw outside a frame callback asked for a second
   callback, and from then on both drew). → one render per compositor frame.
2. **Text was shaped again every frame**: every label of the OPTIONS bar is
   "uncached" (its text may change), ~10 a frame through a whole animation in
   which none changed, and `measure_text("Search")` ran every dock frame. →
   shaped once, kept while the text stays (13 607 → 347 shapes for 12 boxes).
3. **Every screen capture made Hyprland redraw the whole output** (2–3 a
   second through any animation, 8 ms of its CPU and 18 ms of GPU each). →
   one frame object is held inside the capture session and asked to deliver
   when wanted; nothing is forced, nothing at rest. Unasked samples also wait
   for the shell's own drawing to rest (150 ms; never more than 2.4 s): a
   capture still costs ~16 ms of GPU on the Acer — a whole frame.
4. **Hyprland's GPU time was 5–6× the dock's own, and two thirds of it was
   blur under our surfaces.** Every frame went out as "all of it changed", and
   our surfaces are far larger than what they draw (OPTIONS: 510 px tall for a
   28 px bar; the dock's holds the whole launcher: 83 % of the screen).
   → `damage.rs`: the surface is cut into 32 px tiles; every draw folds a hash
   of everything its pixels depend on into the tiles it can touch; the tiles
   that differ from the last presented frame are the damage (2–28 % of the
   surface). A frame in which no tile differs is not drawn at all (13–25 % of
   the OPTIONS and deck frames of a box or stage animation).
   wgpu has no API for present damage, so a patched copy of `wgpu-hal`
   (`third_party/`, ~200 lines) passes it on: `VK_KHR_incremental_present` on
   Vulkan, `eglSwapBuffersWithDamage` on GL.

### What Hyprland needed (three patches, Golem `hyprland-patches/`)

- **layer-commit-damage** — stock damages a layer surface's WHOLE box on
  every commit, whatever the client says. Without this patch item 4 changes
  nothing.
- **keep-work-buffers** (`debug:invalidate_work_buffers`, off by default now)
  — stock invalidates its render buffers after every frame and clears each
  one whole at its next use: three full-screen clears a frame whatever the
  damage, and on Intel before gen 9 three full-screen resolves on top (~4 ms
  of GPU a frame on the Acer). This one is for the whole desktop, not just
  the shell.
- **screenshare-region-session** — an upstream bug found on the way: on any
  monitor whose scale is not 1, EVERY frame of a region capture creates a new
  session that is never freed (the lookup compares a logical box with a
  scaled one). Each forces a full redraw, announces a screencast start and
  stop, and stays in a list that every texture draw walks. `wf-recorder`
  always captures by region: on the Acer one 11 s recording left Hyprland
  12 % dearer per frame for the rest of its life, three left it 37 % dearer,
  about eight 2–3×. With the patch: flat, and 16 announcements instead of 485.

Looked at and NOT kept: scissoring the blur's stencil clear (2–4 %),
`render:use_shader_blur_blend` (a third less blur cost, but it draws
differently), fewer damage rectangles (the average is already 1.0).

### Results (Acer E5-573, HD 5500, 1366×768, blur on)

Before = round 2 on stock Hyprland; after = `unify-3` on the patched one.
Seconds of CPU/GPU for the same scripted workload.

| workload | dock CPU | Hyprland CPU | Hyprland GPU | all GPU | GPU busy |
|---|---|---|---|---|---|
| 12 OPTIONS boxes | 2.01 → 0.70 | 1.36 → 0.85 | 6.12 → 2.71 | −55 % | 21 → 10 % |
| launcher ×5 | 2.91 → 1.78 | 1.68 → 1.61 | 10.06 → 5.66 | −37 % | 52 → 33 % |
| settings panel ×4 | 3.74 → 1.99 | 1.61 → 1.31 | 8.29 → 4.88 | −39 % | 68 → 43 % |
| pointer over the dock | 0.84 → 0.33 | 0.75 → 0.38 | 3.49 → 1.18 | −59 % | 45 → 22 % |
| typing a search | 1.09 → 0.82 | 0.48 → 0.49 | 2.77 → 1.73 | −30 % | 36 → 26 % |
| fast launch ×5 | 0.33 → 0.19 | 0.27 → 0.23 | 1.32 → 0.89 | −29 % | 12 → 8 % |
| stage ×3 | 1.73 → 1.40 | 1.47 → 1.35 | 4.40 → 3.42 | −23 % | 15 → 12 % |

Frames drawn for the same animations: OPTIONS 1318 → 637 (12 boxes), dock
1161 → 1037 (launcher), 1374 → 866 (panel). Captures: ~70 (46 forced) → 39
(none forced); at rest 5 per 30 s → 0. The GL path, forced on the Acer:
Hyprland GPU −36 % to −66 % between full and real damage.

### How round 3 was verified

- **The damage, pixel by pixel**: `WAVERUNNER_DAMAGE_CHECK=1` composes every
  frame into a texture of its own, reads it back and compares it with the
  frame before; a pixel that changed outside the frame's damage (or at all,
  in a frame that would have been skipped) is logged and counted. All
  workloads, the gear pages and the 14 deep scenarios, Vulkan and GL, scale
  0.67 and 1: 0 wrong pixels in ~15 000 frames.
- **The real screen**: a screenshot proves nothing here (taking one makes
  Hyprland redraw everything). `kms.sh` grabs the scanout buffer itself
  (`ffmpeg -f kmsgrab`, de-tiled by `detile.py`) after a run of partial
  frames, then again after a forced full redraw. Identical in every still
  scenario, on Vulkan and GL. The method was proven first by breaking the
  damage on purpose: 20 000+ stale pixels.
- End-state screenshots against round 2 (clock digits only), the bar still
  follows a window's colour in ~0.1 s, the recording leak test, clippy, 358
  tests.
- `WAVERUNNER_FULL_DAMAGE=1` turns damage tracking and frame skipping off.

Not verified (no such machine in my hands): Iris Xe/anv at 3200×2000 scale
1.25 with VRR at 165 Hz (the dev box), NVIDIA, AMD, two monitors, the ASUS
and the MacBook themselves (their GL path ran on the Acer).

Two things learned about measuring: the compositor started by path (not
through `/run/wrappers`) is not SCHED_RR and the governor then halves its
clock — compare instructions, or `chrt` it; and a screen RECORDER on this
Hyprland changes what the compositor draws (see the third patch), so it
cannot be the witness for damage.

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
| 10 | A screen capture still costs ~16 ms of GPU on the Acer (Hyprland re-renders what changed, draws the whole output through a colour-management shader and reads all of it back, whatever region is asked). The plugin already reads the window-top colour from the texture (item 8): if it handed the samples over, the shell would capture nothing at all. | no capture hitches, −10–20 % GPU in animations | plugin code |
| 11 | The settings panel draws at 60 fps for as long as it is open (its pills drift, by design): ~45 % GPU busy on the Acer just standing there. Drawing the drift at 30 fps would halve it. | panel idle ÷2 | a look decision (Max) |
| 12 | The dock still draws every frame into an offscreen texture and blits it, also when no box is open (the texture exists for the box's frost). Drawing straight to the swapchain then saves a full-surface pass. | dock GPU −20–30 % | medium |

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
