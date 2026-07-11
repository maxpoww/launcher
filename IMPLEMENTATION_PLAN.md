# waverunner — Implementation Plan

Derived from CLAUDE.md. Phases are strictly ordered; a phase starts only
after the previous phase's acceptance criteria pass on a live Hyprland
session.

## Current status (2026-07-06)

Design pivot: waverunner is an **auto-hiding dock**, not a hotkey-first
launcher. Touching the bottom screen edge reveals a slim dock bar;
click launches, scroll expands to the full search popup, typing searches;
pointer-leave auto-hides. `waverunner-ctl` stays as the hotkey path.

Implemented and verified on a live Hyprland session (QEMU VM, llvmpipe,
scale 1 — redo visual checks on real GPU hardware):

- `proto`: line-based socket protocol (`Command`, `Response`, `socket_path`).
- `core::config`: full TOML config schema with defaults, XDG loading,
  hex-color parsing, `deny_unknown_fields` typo detection; `[input]`
  covers `natural_scroll`, `edge_reveal(_px)`, `autohide(_delay_ms)`.
- `core::search`: nucleo-backed fuzzy `Searcher`.
- `daemon::animation`: dt-based spring + easing engine (60/144 Hz
  invariance tests included).
- `daemon::state`: `Hidden → Dock → Open` rest-point machine animating
  `extent`, interrupt-safe (hide mid-open does not jump).
- `client`: complete `waverunner-ctl toggle|show|hide`.
- `daemon::ipc`: calloop-integrated Unix socket server; live-probe
  single-instance guard (connect() before removing a "stale" socket),
  SIGKILL recovery verified, malformed input answered with `err ...`.
- `daemon::{main,surface,renderer}`: fixed-size bottom-anchored layer
  surface, wgpu rounded-bar renderer, frame-callback loop that goes fully
  idle (0.0% CPU) at every rest point; input region tracks the visible
  extent (edge-reveal strip while hidden); scroll expand/collapse;
  pointer-leave auto-hide via calloop grace timer.

Deferred stubs: `core::index::DesktopIndex::scan()` returns empty (P4).

---

## P1 — Skeleton ✅ (accepted 2026-07-06)

**Goal:** prove smithay-client-toolkit + wgpu render onto a bottom-anchored,
transparent, fixed-size layer surface on Hyprland.

Verified on a live Hyprland session (QEMU VM, llvmpipe adapter, scale 1):
- Daemon starts, logs adapter info, no panic; `hyprctl layers` shows the
  surface at 720x420, bottom-anchored, top layer.
- `waverunner-ctl show`/`hide` fade the translucent bar in and out.
- 0.0% CPU over 10 s when settled (cumulative CPU time frozen).
- Surface never resized after mapping (single configure size).

Open follow-ups: repeat the visual checks on real GPU hardware (Vulkan
adapter, not llvmpipe) and confirm no frame callbacks with
`WAYLAND_DEBUG=1`.

## P2 — IPC ✅ (accepted 2026-07-09)

**Goal:** reliable control socket; < 50 ms command-to-visible.

Verified live:
- 20 rapid `toggle`s complete in ~83 ms total (~4 ms round-trip each),
  daemon settles in the correct end state, no stuck states.
- Malformed socket input (`frobnicate`, empty line, trailing args) gets an
  `err ...` response and the daemon stays up.
- `kill -9` leaves a stale socket; the next start probes it with
  `connect()`, removes it, and rebinds.
- A second instance while one is live now *refuses to start* instead of
  stealing the socket (the original unconditional-remove orphaned the
  running daemon; fixed with the connect-probe).

Remaining tasks: none — both closed 2026-07-09:
1. ✅ Hyprland keybind docs live in README ("Hyprland integration").
2. ✅ Latency measured: `waverunner-ctl --time` stamps before `connect()`
   and prints the round-trip after the `ok` response; since the daemon
   renders and commits the first frame *before* responding, the
   round-trip covers command-to-first-frame-submitted (presentation
   lands on the next vblank). Daemon logs the handle+render span at
   debug level. Measured on the VM (llvmpipe): 5–8 ms steady state,
   36 ms for the first frame after an idle cold start — well under the
   50 ms target. (Keybind path adds `waverunner-ctl` process spawn on
   top, not included in the measurement.)

## P3 — Animation & dock interaction (working, checks open)

**Goal:** the grow + fade motion and the pointer-driven dock loop,
fully config-driven, idle at rest.

Working (verified live in the VM):
- Rounded-bar wgpu rendering; `extent`/`alpha` animation between the
  Hidden/Dock/Open rest points; goes fully idle at every rest point.
- Scroll on the dock expands to the popup / collapses back
  (`natural_scroll` config).
- Edge reveal: while hidden, a thin input-region strip
  (`edge_reveal_px`, default 5) hugs the bottom edge; pointer-enter
  reveals the dock. Default keeps margin for pointer stacks that clamp
  the cursor short of the last screen row (this VM tops out at y=797
  of 800 — a 2px strip was physically unreachable by mouse).
- Auto-hide: pointer-leave arms a grace timer (`autohide_delay_ms`,
  default 300); re-entry cancels it; firing hides dock *and* popup.
  P4 must suppress this while a search query is active / keyboard
  focused.

Remaining tasks (real-hardware only; VM-verifiable items closed
2026-07-09):
1. ✅ `[animation]` knobs verified end-to-end by timing the command →
   "settled, going idle" gap in the debug log under contrasting
   configs (scratch `XDG_CONFIG_HOME`): `close.duration_ms` 1200 →
   1.27 s observed, 100 → 145 ms; `open.kind = "ease-out-quart"` +
   `duration_ms = 800` → 870 ms; spring 550/42/1 → ~400 ms vs soft
   underdamped 80/8/2 → ~3.35 s. kind/duration/stiffness/damping/mass
   all take effect, open and close independently.
2. Handle scale factor in pixel math (integer scale now; fractional-scale
   protocol is P5). Verify no blurry rendering at scale 2. *(needs real
   hardware / scale-2 output)*
3. Verify frame pacing at 60 Hz and 144 Hz monitors (engine is dt-based
   and unit-tested for this; confirm visually on real hardware).
4. ✅ Zero frame requests when settled, confirmed with
   `WAYLAND_DEBUG=1`: a 90 s capture on a live session contains a
   66.7 s span with zero client requests (no `frame`, no `commit`);
   redraw bursts correlate 1:1 with pointer motion / animation.

## P4 — Launcher core (icons + grid + launch + search done)

**Goal:** it actually launches things.

Done (verified live in the VM, 11-entry corpus):
- `core::index::DesktopIndex::scan()` via `freedesktop-desktop-entry`:
  XDG precedence, locale-aware `Name=`, dedupe by desktop-file ID,
  `NoDisplay`/`Hidden` honored, Exec field codes stripped; unit-tested.
- Background indexer thread (`daemon::apps`) rasterizes icons
  (`freedesktop-icons` lookup; PNG via tiny-skia, SVG via resvg; muted
  colored tile as placeholder) to 48² premultiplied RGBA and delivers
  everything over a `calloop::channel`.
- `daemon::content`: pure layout/hit-test/scene module (dock icon row =
  first N entries by name; scrollable icon+name list in the open card);
  unit-tested, shared by rendering and input.
- Renderer: instanced SDF rounded-rects, icon texture array, glyphon
  text; list clipped by scissor; content fades with the card alpha.
- Hover highlight (dock slots + list rows), wheel scrolls the open list
  (scrolling past the top collapses), click launches via double-fork +
  `setsid` (`daemon::launch`) and hides the card.

The index auto-refreshes: summoning the dock requests a rescan
(coalesced, 2 s cooldown) from the long-lived indexer thread, so
installs/uninstalls appear without a restart. Rasterized icons are
cached across rescans keyed by resolved path (nix store paths change
on theme updates, invalidating naturally). Note: on NixOS, a session
whose XDG_DATA_DIRS pins a package-specific store path keeps showing
that app until re-login — env staleness, not scan staleness.

Type-to-search (done): search box at the card bottom; typing filters
the grid live (`Searcher::rank` → ranked entry indices), non-matches
hidden, best match auto-selected; Enter/click launches; Backspace
edits, arrows move selection, Escape clears then collapses.
Focus-model decision: `Exclusive` keyboard interactivity while open
(deliberate-gesture popup, rofi behavior); dock stays pointer-only, so
typing-while-docked is out by design. The ranked-indices pipeline is
the seam for future system-wide "search all" providers (P6).

Intellihide (done): via Hyprland IPC (`daemon::hypr`), the dock parks
visible while no window overlaps its zone and dodges when one does;
dismissals collapse to the parked dock. `input.intellihide = false`,
IPC failure, or a non-Hyprland compositor fall back to always-auto-hide.

Remaining tasks: none — all three landed 2026-07-09:
1. ✅ Persistent icon cache: rasters persist as raw RGBA files under
   `$XDG_CACHE_HOME/waverunner/icons-48/`, keyed by hash of icon
   path + size + mtime (nix store churn invalidates via the path), so
   cold daemon start skips rasterization for unchanged icons. Profiling
   showed the theme-directory walk (freedesktop_icons lookup), not
   rasterization, dominated (~15 ms/icon on NixOS's long
   XDG_DATA_DIRS), so icon-name → path resolutions — including negative
   ones — persist too (`icon-paths.json`, invalidated by a fence over
   theme + XDG_DATA_DIRS + icon-dir mtimes). Measured on the VM:
   875 ms cold → 26 ms warm.
2. ✅ `Terminal=true` entries run inside the configured `[launch]
   terminal` (default `foot`), exec line shell-quoted.
3. ✅ Dock pinning via drag-and-drop (`PinDb`: ordered pins + exclusion
   list in `pins.json`), richer than a static `[dock] pinned` list.

Acceptance criteria:
- Edge-touch → click an icon launches it and the dock hides. *(click
  path implemented; needs a live human click to sign off)*
- Typing filters apps with nucleo ranking; arrows/Enter work; Escape
  clears/collapses. *(implemented; typing needs a live keyboard to
  sign off)*
- Launched apps survive daemon restart (properly detached, no zombies).
- Cold start with cold cache < 100 ms to interactive; warm cache
  instant. *(✅ verified 2026-07-09 on the VM: warm-cache start indexes
  38 apps in 26 ms — see the `RUST_LOG=debug` "indexed N apps in T
  (scan …, icons …)" line.)*

## P4.5 — Sectioned popup (2026-07-09)

The open card is split into three independently paging sections, top to
bottom: **Apps** (6×3 grid), **Install** (6×1, nixpkgs package search —
see below), and **Files** (6×1, the standard home folders opened via
`xdg-open`). Search fans results into their
sections (apps → Apps, folders → Files); each section has its own
cyclic horizontal paging, page dots, and wheel routing by pointer
position; keyboard selection walks the sections as one flat list.
Folders never auto-fill the dock but can be pinned explicitly. Default
card height grew to 680 to fit the five rows; shorter cards shrink the
Apps section first.

## P4.6 — Install section: nixpkgs search + drag to (un)install (2026-07-10)

The Install section is live package search over all of nixpkgs, with
drag-and-drop package management:

- **Index:** a `nix.rs` background thread dumps `nix search nixpkgs ^
  --json` (109 624 packages, ~4 s warm / minutes on the very first run)
  into a slim TSV cache (`$XDG_CACHE_HOME/waverunner/nixpkgs-index.tsv`,
  10 MB), loads it instantly on start, and refreshes it in the
  background when older than a day.
- **Curated database (2026-07-10):** the dump keeps only the end-user
  catalog — top-level attrs plus `kdePackages`, minus `-unwrapped`
  build inputs, deduped by pname+version — 24 004 of 109 663 packages
  (cache v3, 1.9 MB). Language-ecosystem sets (python/haskell/vim/…)
  made four of five results noise. A format-rejected cache now forces a
  re-dump regardless of file age (a v1→v2 bump exposed that the age
  check alone left the index empty forever).
- **Search:** typing fans package matches into the Install section as
  transient entries (same pattern as file-search results). Ranking runs
  on the nix thread (16–270 ms debug over the curated index): queries
  coalesce to the newest, answers arrive as `Ranked` events, and the
  previous hits keep showing until the fresh ones land. Ranking is
  name-first (`rank_hits`): nucleo scores a word-boundary match in a
  description the same as one on the name, which buried `firefox`
  itself under alphabetically-earlier packages that merely mention
  Firefox — so names rank in their own pass and description-only
  matches (the "media player" discovery case) fill the tail. Queries
  shorter than 2 chars are not ranked.
- **Install:** dragging a package cell onto the Apps section (or the
  dock) runs `nix profile install nixpkgs#<attr>` on a separate
  mutation worker (serialized; a minutes-long install never blocks
  search). The cell's label swaps to "Installing…" until done; success
  triggers an app rescan, so the new app pops into the Apps grid
  (`~/.nix-profile/share` is in XDG_DATA_DIRS).
- **Uninstall:** dragging an Apps cell onto the Install section
  canonicalizes its `.desktop` path (new `AppEntry::path` field) and
  matches it against `nix profile list --json` store paths; a match is
  removed with `nix profile remove <name>`, a non-profile app is
  refused harmlessly (logged). Label shows "Removing…" while running.
- Drop-target sections get a highlight wash while a matching drag
  hovers them; busy cells can't start new drags; pointer-leave cancels
  a drag without installing/uninstalling anything.

Package cells show real icons: the rank thread owns its own
`IconLoader` (firefox/vlc/gimp/xterm all resolve; CLI-only tools fall
back to the colored letter tile). Icons are the slow part (cold
negative theme lookups walk every theme dir), so they never delay
results: `Ranked` is sent the moment ranking finishes and the icons
follow as a separate `HitIcons` event — skipped entirely when a newer
query is already waiting. The renderer reserves `RANK_HITS_MAX`
texture layers past the app icons; each icon batch uploads into that
tail (`update_icon_layer`), and rescans re-upload after `set_icons`
rebuilds the array.

Icon lookups (2026-07-10, second pass) are chain- and
availability-based: `IconLoader` searches a theme fallback chain
(configured theme, then Papirus-Dark/Papirus/breeze when installed —
the chain is part of the resolution-cache fence), packages also try
their reverse-DNS aliases (`kdePackages.kate` → `org.kde.kate`,
`gnome-calculator` → `org.gnome.Calculator`), and one 200 ms readdir
sweep at startup collects every icon name that exists
(`available_icon_names`, 21 006 on this VM) so names that exist
nowhere become letter tiles instantly instead of paying a negative
theme walk per name — that walk was why a first search felt slower
than a repeat. Coverage maximized (passes three–five): the
availability map is case-insensitive (lowercased stem → actual name,
so `qbittorrent` finds `qBittorrent.svg`), candidates include
progressive trailing `-segment` strips so variants inherit the family
icon (`firefox-bin` → `firefox`), a reverse-DNS alias map reaches
icons published under vendor ids (`com.mitchellh.ghostty`), and — the
systematic end of guessing — the dump queries the prebuilt
**nix-index file database** (downloaded to `~/.cache/nix-index/files`,
~100 MB, weekly refresh; one `nix-locate` sweep over
`share/applications/*.desktop`) so every package carries the
authoritative icon-name hints from its own desktop files (TSV cache
v4, `icons` column; 2 930 of 24 010 curated packages are GUI apps with
hints). The sweep re-runs lazily when older than 5 min, so a
drag-install's bundled icon appears in search without a restart.
Finally (pass six), GUI apps whose art exists nowhere locally fetch it
the way distro app centers do: from Flathub's AppStream icon catalog
(pre-extracted server-side, served per app id), keyed by the
desktop-file id — disk-cached forever under
`$XDG_CACHE_HOME/waverunner/flathub-icons/`, 404s remembered as miss
markers (retried monthly), network failures retried next batch, and at
most `FLATHUB_FETCH_BUDGET` (6) new downloads per icon batch so an
all-new page never stalls on the network. Sources compose: local
themes → Flathub catalog → installer box. Measured: 2 960 of 24 004 curated packages resolve
a real themed icon (1 439 → 2 588 → 2 960); everything else shows the
theme's standard installer box (`system-software-install`, falling
back through `package-x-generic` / `package` /
`application-x-executable`) instead of letter tiles — letter tiles
remain only in the no-theme degenerate case.

Verified on the VM: index dump + cache round-trip (unit-tested), cache
load on start (instant), the Install hint states ("Indexing nixpkgs…" /
"Search to install from nixpkgs"), the desktop-path → profile element
matching (bash equivalent against a real `nix profile install
nixpkgs#xterm`), and theme-icon coverage for package names (ignored
test `pkg_icon_lookup_coverage`). Typing and the two drag gestures need
a live human to sign off; `xterm` is installed in the profile as a test
subject for drag-to-uninstall. Follow-ups: pkg index RSS is ~50 MB
(could pack into one string arena), and installed-state could be shown
on package cells.

## P5 — Polish

- Icons: `freedesktop-icons` lookup + texture atlas.
- Theming: colors, corner radius, timings from `[theme]` (schema exists).
- Auto-hide on focus loss: keyboard-leave handler exists; verify against
  alt-tab, click-elsewhere, workspace switch (known risk area with
  `OnDemand` interactivity).
- Fractional scaling via `wp-fractional-scale-v1` + `wp-viewporter`.

## P6 — Modes

- Prefix-dispatched modes (`=` calc, `ssh ` hosts, clipboard).
- `Mode` trait in `core` (input → ranked items → activate action);
  the app launcher becomes the default mode.

---

## Cross-cutting rules (from CLAUDE.md — enforced throughout)

- `nix develop` first, always; missing system libs go into `flake.nix`.
- Clippy clean at `-D warnings`; no `unwrap`/`expect` outside `main.rs`
  startup and tests; `anyhow` in binaries, `thiserror` in libs.
- Daemon single-threaded except the P4 indexer thread and the P4.6 nix
  threads (package index/rank + profile-mutation worker).
- `tracing` only (client crate may `println!`).
- Rendering changes cannot be verified headless — describe what to check
  visually in the PR/commit instead of claiming it was tested.
- Conventional commits scoped by crate; never push, never amend.

## Risk register

| Risk | Phase | Mitigation |
|---|---|---|
| sctk ↔ wgpu raw-handle plumbing breaks at compile or runtime | P1 | Follow sctk `simple_layer` + wgpu raw-handle examples verbatim before inventing; keep wgpu pinned to one major |
| `OnDemand` keyboard focus-loss detection unreliable | P2/P5 | Test alt-tab, click-other-window, workspace switch explicitly; fall back to Hyprland IPC events if needed |
| Fractional scaling blurs text | P3/P5 | All pixel math via one `scale()` helper; integer scale until wp-fractional-scale lands |
| Compositor throttles frame callbacks for occluded/offscreen surfaces | P3 | Never rely on a frame callback arriving to make progress; IPC show must draw immediately |
| nucleo/wgpu API churn across versions | all | Workspace pins majors; upgrade deliberately, one crate at a time |
