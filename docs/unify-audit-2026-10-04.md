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
side is Golem's branch `layer-damage`, and one system fix found on the way is
Golem's branch `heal-one-readlink`.

This round measured first. The daemon got always-on counters
(`waverunner-ctl debug-perf`: frames per surface, shapes, Hyprland requests,
captures, damage) and a scripted workload runner (`tools/unify-check/work.sh`)
that reads, per workload: dock and Hyprland CPU, each one's GPU time (the
kernel's per-client `drm-engine-render`), whole-GPU busy (i915 PMU) and
instructions (CPU seconds lie when the governor changes the clock). Later a
second runner (`steady.sh`) entered each RESTING state of the shell, left it
alone and measured what it cost — which is where the second half of the
round's findings came from.

### What the measurements showed — the shell in motion

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
   for the shell's own drawing to rest (150 ms; never more than 2.4 s).
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
5. **Every frame was drawn twice** — into an offscreen texture, then copied
   across — because an open box frosts a blurred copy of the scene. Only the
   dock ever opens one. → the offscreen target and the blur pair exist only
   while a box is open; every other frame is one pass straight into the
   swapchain. The dock's own GPU time halves; 16 MB less GPU memory.
6. **The compositor blurred under our EMPTY glass.** With the `blur` layer
   rule Hyprland works out the blur for whatever is damaged under the whole
   surface and throws it away pixel by pixel afterwards. So anything another
   app drew under the shell's transparent area paid for blurs nobody saw: a
   terminal scrolling text under an idle shell kept the GPU 40–42 % busy; with
   the shell's blur rule removed, 8–11 %. → `visible.rs`: each frame tells
   the compositor where it has anything to show
   (`hyprland_surface_v1.set_visible_region`, from the same tiles the damage
   uses); a frame that draws nothing takes the surface out of the
   compositor's frame. The terminal test: 11–12 %. It works on the stock
   Hyprland too.

### What the measurements showed — the shell at rest

`steady.sh` puts the shell in a state (dock out, launcher open, each box,
settings, fast launch, stage, overview, spread, a player running, locked),
waits, and measures 20 s of nothing happening.

7. **STAGE mode cost 17 % of a core for as long as it was on.** The deck ran
   a `pw-dump` of its own every second for its speaker badges; each one, a
   PipeWire client coming and going, set the Brain's audio sensor off twice
   (its probe and the confirming one): five processes a second, three 100 KB
   dumps parsed. → the sensor (which reads the same dump when PipeWire
   changes) publishes who can be heard, and the deck matches that to its
   tiles; the poller thread is gone. 17.3 % → 0.6 % of a core, 146 processes
   in 30 s → 6. The badges are the same picture (old against new, 0 px) and
   now follow within ~0.2 s instead of up to a second.
8. **The audio sensor read everything twice.** Its probes are PipeWire
   clients, so each run reports itself; that echo was judged by the clock
   (anything within 150 ms of a probe) plus one confirming probe. It now reads
   `pw-mon`'s stream far enough to know WHOSE event a block is — its own
   probes by their pid, real change by the kind of object — so a change costs
   one probe, not two, the heartbeat one a minute, not two; and a stream that
   stops inside the old window is no longer missed until the next heartbeat.
   The dump is parsed once instead of three times. A watcher that dies
   (PipeWire restarting) is started again after 30 s instead of leaving the
   sensor polling every 2 s for the rest of the session.
9. **With a player running, the bar woke the compositor and the colour
   sampler once a second for nothing.** The player's position moves, the bar
   is asked to redraw, the frame comes out identical and is not presented —
   but the surface was committed anyway to get its frame callback, and to
   Hyprland a commit that asks for a frame is a frame to produce, damage or
   none; a capture waiting "for the screen to change" is delivered on it.
   → nothing is committed for such a frame. 23 captures in 20 s → 2.
10. **Golem checked its home links with 76 processes, every minute**
    (`golem-home-heal.timer`: one `readlink` per managed file). Not the
    shell's, but the largest source of process starts on an idle Golem.
    → one `readlink` for all of them (Golem `heal-one-readlink`): 221 ms →
    14 ms, same verdict in ten fault states.

What is left at rest (Acer, 20 s each):

| state | dock CPU | Hyprland CPU | GPU busy | note |
|---|---|---|---|---|
| desktop, dock out, fast launch, stage, locked, a pointer left on the dock/bar | 0.03–0.07 s | 0.01–0.04 s | 0 % | nothing is drawn |
| a box open (notifications, stats, clipboard) | 0.05–0.08 s | 0.03–0.06 s | 0.1–0.4 % | |
| a player running | 0.07 s | 0.02 s | 0 % | was 0.14 s / 1.6 % |
| launcher open | 0.04 s | 0.02 s | 0 % | after ~2 s of settling when the pointer enters |
| **settings panel open** | **1.66 s** | **1.39 s** | **43 %** | 61 fps by design: the pills drift |
| overview / spread (the plugin) | 0.06 s | 0.13 / 0.34 s | 1.5 / 7 % | the plugin's own live refresh |

### What Hyprland needed (four patches, Golem `hyprland-patches/`)

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
- **visible-region-damage** — stock never resets "the visible region
  changed", so after the first change it re-damaged the whole surface on
  every commit; and a change damaged the whole box. Now: once, and only the
  old and new regions.

None of them changes a header (the waveview plugin's ABI is untouched). The
dock does not NEED them: items 1–3, 5–9 are its own, and the visible region
works on stock.

Looked at and NOT kept: scissoring the blur's stencil clear (2–4 %),
`render:use_shader_blur_blend` (a third less blur cost, but it draws
differently), fewer damage rectangles (the average is already 1.0), capturing
only the strip the sampler reads (no measurable gain on the Acer).

### Results (Acer E5-573, HD 5500, 1366×768, blur on)

Before = round 2 on stock Hyprland; after = `unify-3` on the patched one.
Seconds of CPU/GPU for the same scripted workload.

| workload | dock CPU | Hyprland CPU | Hyprland GPU | dock + Hyprland GPU | GPU busy |
|---|---|---|---|---|---|
| 12 OPTIONS boxes | 2.01 → 0.41 | 1.36 → 0.38 | 6.12 → 0.67 | −89 % | 21 → 2.4 % |
| launcher ×5 | 2.91 → 1.53 | 1.68 → 1.19 | 10.06 → 3.18 | −62 % | 52 → 20 % |
| settings panel ×4 | 3.74 → 1.90 | 1.61 → 0.96 | 8.29 → 3.05 | −58 % | 68 → 30 % |
| pointer over the dock | 0.84 → 0.32 | 0.75 → 0.32 | 3.49 → 0.38 | −85 % | 45 → 9 % |
| typing a search | 1.09 → 0.74 | 0.48 → 0.35 | 2.77 → 1.04 | −54 % | 36 → 17 % |
| fast launch ×5 | 0.33 → 0.18 | 0.27 → 0.19 | 1.32 → 0.28 | −75 % | 12 → 3 % |
| stage ×3 | 1.73 → 0.52 | 1.47 → 1.06 | 4.40 → 1.55 | −64 % | 15 → 6 % |

(The stage workload's dock CPU does not count its helper processes: with
them, 3.1 s → 0.5 s.)

The same dock on the STOCK Hyprland (measured before items 7–9): Hyprland GPU
2.61 / 5.18 / 4.60 / 1.09 / 1.52 / — / 2.13 s, GPU busy 9 / 28 / 38 / 18 /
21 / — / 8 % — most of the gain needs no compositor patch.

On the GL backend (forced on the Acer; what the ASUS and the MacBook run):
Hyprland GPU 0.68 / 3.15 / 3.04 / 0.39 / 1.03 / 0.28 / 1.49 s, GPU busy 2.7 /
24 / 33 / 12 / 20 / 4 / 6 %.

Frames drawn for the same animations: OPTIONS 1318 → 637 (12 boxes), dock
1161 → 1037 (launcher), 1374 → 866 (panel). Captures at rest: 5 per 30 s → 0.

### How round 3 was verified

- **The damage, pixel by pixel**: `WAVERUNNER_DAMAGE_CHECK=1` composes every
  frame into a texture of its own, reads it back and compares it with the
  frame before; a pixel that changed outside the frame's damage (or at all,
  in a frame that would have been skipped) is logged and counted — and so is
  a drawn pixel outside the visible region the compositor was given.
  `=paths` also draws every one-pass frame the old two-step way and compares
  the two. Every workload and the 14 deep scenarios, Vulkan and GL, scale
  0.67 and 1: 0 wrong, 0 outside, 0 differ (~3 500 frames a run).
- **The real screen**: a screenshot proves nothing here (taking one makes
  Hyprland redraw everything). `kms.sh` grabs the scanout buffer itself
  (`ffmpeg -f kmsgrab`, de-tiled by `detile.py`) after a run of partial
  frames, then again after a forced full redraw. Identical in every still
  scenario, on Vulkan and GL. The method was proven first by breaking the
  damage on purpose: 20 000+ stale pixels.
- **The speaker badges** (`badge.sh`): a player in a terminal starts, plays a
  one-second sound, a longer one that is muted and unmuted, ends; the deck is
  photographed at each step under the old build and the new: 0 px differ.
  `audiowatch.sh`: an outside volume change, mute, default-sink set, a client
  that changes nothing — one probe each, ~75 ms later; the watcher killed, and
  PipeWire restarted — polling within 2 s, the watcher back after 30 s.
- End-state screenshots against round 2 (clock digits only), the bar still
  follows a window's colour in ~0.1 s, the recording leak test, clippy, the
  tests (359 in the daemon, 120 in the engine).
- Switches: `WAVERUNNER_FULL_DAMAGE=1` (no damage tracking, no skipped
  frames), `WAVERUNNER_NO_VISIBLE_REGION=1`, `WAVERUNNER_NO_SAMPLER=1`.

Not verified (no such machine in my hands): Iris Xe/anv at 3200×2000 scale
1.6 with VRR at 165 Hz (the dev box), NVIDIA, AMD, two monitors, the ASUS and
the MacBook themselves (their GL path ran on the Acer); the overview and the
spread as a hand opens them (a scripted toggle opens the map without drawing
it).

Things learned about measuring:

- The compositor started by path (not through `/run/wrappers`) is not
  SCHED_RR and the governor then halves its clock — compare instructions, or
  `chrt` it.
- A screen RECORDER on this Hyprland changes what the compositor draws (the
  third patch), so it cannot be the witness for damage.
- On the GL backend (Mesa, Broadwell) a texture read back AFTER a pass had
  sampled it came out with a few stale pixels around the last thing drawn —
  the check's own copy, not the screen. The path check reported 4–26 px in
  every deck frame until the copies were taken before anything samples the
  texture. A third, relayed copy settled which picture was right.
- A process's CPU does not include the helpers it starts: the stage's 17 %
  was in `pw-dump` children. Count process starts
  (`perf stat -e sched:sched_process_exec`) and the children's time.
- Leaving a state alone and measuring it found more than any animation did.

### Round 3 on the dev box (2026-10-06): the one regression, and the fix

Deployed on the dev box (Iris Xe, anv, 3200×2000 at 1.25, 165 Hz VRR, blur)
Max found OPTIONS "slow, and choppy". Measured with the gear box opened by
script (`debug-gear open net` / `close`, `tools/unify-check/options-anim.sh`)
and `debug-perf`: **7–9 OPTIONS frames per animation**, where the same dock
with `WAVERUNNER_FULL_DAMAGE=1` drew 40–90. VRR off, `debug:damage_tracking
0`, blur off, the visible region off, the sampler off: no change.
`WAYLAND_DEBUG=1` showed it: during the animation the surface presented
every **100 ms exactly** (`FRAME_OVERDUE`), made no frame request and got no
callback.

The cause was two round-3 changes meeting: (1) "a frame that changed nothing
is not committed" left its `wl_surface.frame` request on the surface ("the
next frame that shows something serves it"); the request then rode on the
bar's next *bare* commit (an input-region update). (2) The
`layer-commit-damage` patch no longer damages a layer's whole box on a bare
commit — and a layer gets its frame callbacks only when it is drawn (Hyprland
sends frame events to the workspace's *windows* when nothing is damaged;
layers go through the surface pass). Stock answered bare commits by drawing
the whole layer every time. So the bar waited on a callback that could not
come until something else damaged it, and drew at the overdue fallback. At
165 Hz two consecutive frames are identical far more often than at 60 Hz,
which is why the laptops never showed it.

Fix, both sides: the renderer takes a `before_present` hook and the frame
request is made **in the commit that presents** (dock, OPTIONS bar, deck —
no request is ever left waiting on a bare commit; a frame that presents
nothing while an animation runs ticks on an 8 ms timer instead), and the
compositor patch damages the whole box again for a commit without damage
that carries frame callbacks (stock behaviour for exactly that case). After
the dock fix alone: 60–92 frames per animation, every present with its
request, callback latency median 4.9 ms.

## Still duplicated — candidates, in the order I would take them

| # | What | Gain | Risk |
|---|---|---|---|
| 1 | The 800 ms zone poll exists only because float moves emit no Hyprland event. The plugin already sees drags: one "geometry changed" verb from it removes the last idle timer (5.6 requests/s → 0). | idle → ~0 | plugin code (a crash there is the session) |
| 2 | ~~Icon array reserve~~ — done in round 2. | | |
| 3 | ~~Pipelines, font system~~ — done in round 2 (the glyph ATLAS is still per renderer). | | |
| 4 | ~~Hyprland readers~~ — one shared parse in round 2; typed structs instead of `serde_json::Value` remain possible. | | |
| 5 | Media (1 s) and Bluetooth (3 s) are polled over D-Bus; both have signals. | brain idle → ~0 | medium (progress bar timing) |
| 6 | `schedule_*_frame` is copied 9 times; XDG path helpers 14 times; `lerp` ×3; the exp-approach ease ×5 (the copies skip `reduce_motion` — an accessibility bug). | simpler | low |
| 7 | The OPTIONS surface is always 510 px tall for a 28 px bar: its swapchain is ~18× the visible strip. (The compositor no longer pays for the empty part — item 6 of round 3 — only the memory is left.) | −30 MB | high (surface sizing is a non-negotiable) |
| 8 | Plugin and daemon both sample the window-top colour (plugin from the texture, daemon by screen capture); four capture flows in the plugin; the minimized set lives in three places. | simpler | plugin code |
| 9 | Initrd activation runs two cold Perl scripts (~4 s on the Acer); the desktop waits for NetworkManager behind the splash (~8 s). See Golem `work/parity.md`. | boot | visual handoff |
| 10 | While anything on screen keeps changing (a video in a window), the colour sampler takes a full-screen capture every 0.8 s: 17 in 12 s, 1–2 points of GPU busy on the Acer (more on a 3200×2000 screen: each is a 25 MB read-back). The plugin already reads the window-top colour from the texture (item 8): if it handed the samples over, the shell would capture nothing at all. A slower pace while the colour stands still would also do, at the price of following an event-less colour change later. | no captures | plugin code, or a timing decision (Max) |
| 11 | The settings panel draws at 61 fps for as long as it is open (its pills drift, by design): 43 % GPU busy and 15 % of a core on the Acer just standing there — the dearest resting state by far. Drawing the drift at 30 fps would halve it. Underneath: Hyprland recomputes the blur behind a layer that redraws ITSELF, every frame, although nothing behind it changed; a cached blur per layer would be the real fix, and a deep patch. | panel at rest ÷2 or more | a look decision (Max) / compositor |
| 12 | ~~One pass when no box is open~~ — done in round 3. | | |
| 13 | To Hyprland, a commit that asks for a frame callback is a frame to produce even with no damage: the output is committed again and waiting captures are delivered. The bar no longer does that (item 9 of round 3); the dock and the deck still commit their unchanged frames while an animation settles (13–25 % of a box's or the stage's frames), as their clock. A timer could be that clock. | fewer empty frames, steadier VRR | medium (the dock's frame loop) |
| 14 | The speaker badge, and the bar's "who is playing", know a stream's process only when the stream comes through the PulseAudio server (browsers do). A native PipeWire client (mpv's default output, `pw-play`) has no pid on its node; its client object does (`client.id`). A few lines in the audio sensor — but it changes what the bar shows (a native player's MPRIS row and its stream would merge). | correctness | a behaviour change (Max) |
| 15 | The overview's live refresh (a 150 ms timer in the plugin) keeps the compositor drawing and the sampler capturing once a second while it is open; the spread at rest holds the GPU at 7 %. | overview/spread at rest | plugin code |

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
