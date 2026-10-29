# CLAUDE.md — waverunner

> Big picture first: read **`GOLEM.md`** — waverunner is the *body* of Golem, a
> Linux distribution built around OPTIONS. This file covers the body's code.

Auto-hiding Wayland/Hyprland **dock + launcher**, Rust. Persistent daemon holding a
layer-shell surface at the bottom edge. Touch the edge → slim dock; scroll/expand →
full search popup; type to fuzzy-search. GPU-rendered (wgpu), frame-callback driven,
idle at rest. `waverunner-ctl toggle|show|hide|expand|collapse` over a unix socket.

## Build / run / test (always via the flake)
```
nix develop -c cargo build
nix develop -c cargo clippy --workspace -- -D warnings   # keep clean
nix develop -c cargo test --workspace
~/launcher/waverunner-dev        # dev launcher (sets LD_LIBRARY_PATH); Hyprland runs this on start
```
No headless compositor — visual/drag behavior needs a live Hyprland session; describe
what to check rather than claiming it's verified.

## Layout
- `crates/daemon` — the binary (`waverunner`): wayland loop, wgpu renderer, state, search, drag.
- `crates/client` — `waverunner-ctl`.
- `crates/core` — config, `.desktop` index (`DesktopIndex`/`AppEntry`), fuzzy search.
- `crates/proto` — socket message types.
- `crates/options-engine` — the headless "Brain": the collectors (what the machine is doing) and the Mind
  (`Engine::start_shell` runs the shell's); see the end of this file.
- `crates/options-notify` — OPTIONS' own notification server (`org.freedesktop.Notifications`, zbus) that
  feeds the bell's box.

## Model
- **Dock** = pinned apps only. **Grid** (`SECTION_APPS`) = every indexed `.desktop`.
  **Install section** (`SECTION_INSTALL`) = nixpkgs packages **and webapps** (see below),
  searchable, drag-to-grid to install. **Files** = live home listing.
- App discovery on one background thread → `LoadedApps` over a channel; `request_rescan(_fresh)`
  re-indexes. Live reload: inotify on the XDG app dirs (calloop `Generic` source) → rescan.
- Icons: freedesktop theme chain (Papirus-Dark → hicolor), SVG via resvg, PNG via tiny-skia,
  256² premultiplied RGBA mip chains.
- Installs (packages) are declarative: waverunner edits `~/.config/waverunner/packages.list`
  (one nixpkgs attr per line); a root `systemd.path` runs `waverunner-apply` which generates
  root-owned `/etc/nixos/waverunner-packages.nix` and `nixos-rebuild switch`es. See `applier.rs`.

## Webapp catalog (installable, like packages)
Curated webapps from `~/.config/webapps.list` (`Name | URL | icon`), run by **Seam**
(Golem's browser): `Exec=seam -golem-app <slug> <url>` opens a webapp window of the
running Seam (no tabs/toolbar, window class `webapp-<slug>` = `StartupWMClass` = the
desktop id, one process + one sign-in with the browser; Seam side = Golem's
`seam/golem-chrome.js` WEBAPPS). Copy-link reads Seam's `$XDG_RUNTIME_DIR/seam/apps.json`
(was Chrome's CDP port; Chrome `--app` until 2026-09-30). Every entry is
materialized as `webapp-<slug>.desktop` (`webapps::materialize_catalog`) so the indexer
rasterizes its icon. Classified at runtime by id-prefix + `managed_webapps` membership:
`webapp-*` not in `managed_webapps` → Install section (catalog); installed → grid. Click/
drag-out = "try" (launch, window shows in dock unpinned); drag to grid = install
(`managed_webapps.add` → `waverunner-webapps.nix`); drag installed → Install = uninstall.
Files: `webapps.rs`, `managed_webapps.rs`, routing in `main.rs::refilter`, drag arms in `dragging.rs`.

## Clipboard link enrichment (the link "share card")
A text clip that is a single bare http(s) URL (`ClipEntry::is_link`) becomes a *link
clip*: it grows a `title` + hero `preview_image` shown as the row thumbnail and detail
hero, plus Title/About rows in the metadata sheet. Hero precedence: **og:image miniature → browser-window screenshot → link glyph.** Two tiers:
- **Phase 1 (always on, local, private):** the `title` is seeded from the source
  window's title at copy time (browser suffix stripped); when the copy came from a
  **browser** (`is_browser_source`) the source window is snapshotted with **`grim`**
  into `preview_image` as a fallback hero (useful for pages with no og:image, e.g.
  Facebook). *A URL copied from a terminal/editor is **not** snapshotted — that would
  just capture an unrelated window — so it shows the glyph until unfurl resolves.* No
  network.
- **Phase 2 (opt-in, network):** `[options] link_unfurl` (**default false**) spawns
  the `unfurl.rs` worker — shells out to **`curl`**, oEmbed fast-path for YouTube then a
  small OpenGraph/`<title>` scrape, downloads `og:image`; `on_unfurl` folds the clean
  title/description/image in (og:image **supersedes** the Phase-1 snapshot). Enabling it
  means one outbound request per copied URL — hence off by default.

Runtime deps (best-effort, absent → graceful fallback): **`grim`** (P1), **`curl`** (P2).
Files: `unfurl.rs`, link arms in `clipboard.rs` (`detect_url`, `capture_window_snapshot`,
`is_browser_source`, `on_unfurl`, `clip_tile`), `hypr::active_window_geom`, flag in `core::config`.

## Clipboard type-to-search
The open history box **holds the keyboard** (`sync_clip_keyboard` → `set_interactive`,
Exclusive) for as long as it is open — that is what lets any printable key start a
search instead of falling through to the window below, and it is released the instant
the box collapses. Keys route in `main.rs::handle_key_event` (after the dict branch) →
`clip_key`: typing morphs the footer's three circles into a search field
(`push_clip_search_field`, `search_t`) and filters the list live; Up/Down move a
selection (`search_sel`, held by clip **id** — a clip captured mid-search shifts every
index), Enter copies the selection and closes, Escape backs out one layer at a time
(query → field → box). Matching is case-insensitive substring (`clip_matches` over the
clip's text head, preview, link title/description and source; `contains_ci` folds
lazily — no lowercased copy per keystroke). `ClipState::shown` is the ONE filtered
index list the draw, hit-test, heights and scroll span all walk; `refilter_clips` is
its only writer — call it after *any* history change. The box height is pinned while
the field is out so filtering can't walk the field around under the typing hand.
Verify with `waverunner-ctl debug-clip-search <query>` (no keyboard needed).

## Clipboard emoji picker (`emoji.rs`)
The footer's 🙂 button wipes a scrollable grid of every emoji over the list; a click
**types it into the window below** and the box stays open for the next one. The box's
search field filters the grid while it is up ("happy" → 😀) — **word-prefix** matching,
not the list's substring (else "cat" drags in eduCATion/notifiCATion).
- **Data**: `emoji_table.rs` is GENERATED from github/gemoji (MIT) by
  `tools/gen-emoji/gen-emoji.sh` — 1871 entries with CLDR name, gemoji aliases+tags
  (the search words a name lacks) and a group. Vendored, not flake-built like the
  dictionaries: small, and the picker should have no data file to find.
- **Colour** needs the family named: `options::EMOJI_FONT` ("Noto Color Emoji"). The
  sans-serif fallback chain resolves the smiley block to **DejaVu Sans**, which has
  monochrome outlines for it — 🎉 came out colour and 😀 a black ring until named.
  EVERY label does it now (2026-10-29): `renderer::set_words` shapes a text in runs,
  its emoji (`emoji_runs`: U+1F300 on, flags, any sign with U+FE0F, keycaps, with
  what joins them) in the colour font by name — "❤️" had been DejaVu's plain heart and
  a gap. A colour glyph ignored the text's alpha (glyphon drew it straight from its
  atlas): glyphon 0.8.0 is vendored with that fix (`third_party/glyphon.patch`).
  Test: `cargo test emoji_words` (the colour check skips where the font is missing —
  the toolbox: run it with `FONTCONFIG_FILE` at the host's fonts).
- **Spanish words** (2026-10-29): `emoji_words.rs` (GENERATED by
  `tools/gen-emoji/gen-emoji-words.sh` from Unicode CLDR es + es-419, keyed by the emoji
  without U+FE0F, sorted) — searched only while the shell is in Spanish ("car" would
  find every "cara"); accents optional (`plain`). Both pickers (this one, the card's).
- **Typing** goes through `clipboard::paste_into_window_below`: serve the emoji to our
  data-control source (`serve_transient_text` — suppressed from the history via
  `ClipState::suppress_hash`; it does take the clipboard over), then **hand the keyboard
  back** (`yield_clip_keyboard`) and paste. Four things had to line up, all found live
  on 2026-09-13:
  1. **Wayland offers the clipboard only to the keyboard-focused client** — while the
     box holds the type-to-search grab, the injected Ctrl+V reaches the app with
     nothing to paste.
  2. Releasing the grab is not enough: the window never stopped being `activewindow`,
     so the seat stays stranded on the layer until focus actually *changes* — it takes
     `hypr::focus_window_no_warp`'s **neighbour bounce** (a direct focus is a no-op).
  3. That focus must **not warp the pointer** (the no-warp helper flips
     `cursor.no_warps` around the bounce), or it drags the mouse off the box and you
     can't pick a second emoji.
  4. The interactivity commit must be **flushed** (`conn.flush()`) before the focus, or
     the compositor answers it while we still claim exclusivity.
- **The box does not take the keyboard back by itself** (`ClipState::keyboard_yielded`):
  after a paste you keep typing in *your* window. Clicking the search field
  (`rearm_clip_keyboard`) is the way back in; reopening the box also resets it. Same
  hand-back runs on **close** — otherwise every box collapse left the window unable to
  type. Re-serving a clip we already own is skipped: the tear-down/rebuild of the
  selection swallowed a paste that landed in the gap (same emoji twice in a row).
- Target apps must treat Ctrl+V as paste — true of GUI apps, not terminals
  (Ctrl+Shift+V), the same limit the paste pill always had.
- Verify without a pointer: `waverunner-ctl debug-emoji [query]`, or `debug-emoji
  <query>!` (trailing bang) to also TYPE the first match.

## Clipboard footer buttons + dictionary ("define a word")
The open history box's footer holds four circular buttons: **new note** (pencil, stub),
**emoji** (🙂, see above), **dictionary** (book) and **clear all** (can). The dictionary opens an
in-box **type-to-look-up** panel (`dict_open`, wipes over the list since the renderer
draws all labels in one late pass — the list must be *skipped*, not painted over): a
`‹ Back` button, a search field, and a **scrollable** answer. Typing works because the
panel grabs the keyboard on the OPTIONS surface (`open_dict` → `set_interactive`; keys
route via `handle_key_event`'s `dict_open` branch → `dict_key`); the box's pointer-leave
auto-collapse is guarded while it's open. Lookup (`dict.rs`) is offline, multi-language
(shows every language that has the word), **accent-insensitive** (`corazon`→`corazón`;
`ñ`≠`n`), and shows the **etymology** when present. `debug-dict` (ctl verb) force-opens it.
- Data: two JSON files, English `dictionary.json` (Webster 1913, plain `{word:def}`) and
  Spanish `dictionary-es.json` (RAE, `{word:{e,d}}` with etymology). Loaded lazily on a
  worker thread from `$WAVERUNNER_DICT[_ES]` (else the data dir). **Built declaratively**
  by the flake's `dictionaries` package from pinned upstreams (`tools/rae-parse` compiles
  with `rustc` and parses the RAE dump); `waverunner-daemon`'s wrapper + `waverunner-dev`
  set the env vars. Regenerate the ES data with `nix build .#dictionaries`.
Files: `dict.rs`, panel in `clipboard.rs` (`open_dict`/`dict_key`/`push_clip_dict`/
`dict_answer_lines`/`clip_dict_scroll_span`), `tools/rae-parse/parse_rae.rs`, flake
`dictionaries` pkg. See memory `dict_data_provisioning`.

## Conventions
Edition 2021, rustfmt, clippy clean at `-D warnings`. `anyhow` in the binary, `thiserror` in
libs; no `unwrap` outside `main` startup/tests. One event loop; all shared state lives on it.
Background work runs on dedicated worker threads that only talk back via calloop channels: the
app indexer, and the nix index/rank, install/uninstall mutation, and try-it (`nix build`)
workers — kept separate so a slow build never blocks an install, or search.
`tracing` for logs. Never `git push` or amend. Commits: conventional (`feat(daemon): …`).

## Non-negotiables (don't revisit)
Persistent daemon (no per-invocation spawn); fixed-size layer surface (animate content, never
resize the surface); hotkeys are compositor-side (ctl writes the socket); pointer-reveal is a
thin surface-side input strip; all animation dt-based (test 60/144Hz).

## Seeing the UI (visual verification)
There is no headless renderer — a UI change is only "verified" once looked at on the live
Hyprland session. Use the **`verify-ui` skill** (`.claude/skills/verify-ui/`): it builds,
restarts the daemon with the new binary, optionally reveals a surface, and `grim`s a screenshot
to `/tmp/waverunner-verify/` for you to Read. `--reveal open|dock` drives the launcher/dock;
`--reveal clip|clip-detail|notif` force-open the OPTIONS boxes via the daemon's `debug-*` ctl
verbs (`waverunner-ctl debug-clip|debug-clip-detail|debug-notif`). The topbar is still
pointer-only — have the user hover it, then capture with `--no-build --no-restart`. Prefer a
tight `--geom "X,Y WxH"` crop over full-screen to save image tokens (the boxes hug the edges).
Design source of truth for the OPTIONS surfaces = the HTML mockups in `~/*-mockup`.

## Compositor
Custom **Hyprland 0.55.4 with a Lua dispatch API** (`hl.dsp.*`), not stock. Window actions live
under `hl.dsp.window.*`; wrong names fail silently. See `docs/hypr-api.md` before writing any
new `hypr::dispatch(...)` call.

## Where things live (jump, don't grep)
The big daemon files are cohesive but long — go straight to the function:
- `main.rs` — event loop, App struct + init, `refilter` (section routing), channel wiring.
- `options.rs` — OPTIONS topbar pills (window/clock/controls), hit-testing, `options_text_color`.
- `clipboard.rs` — clipboard OPTION: capture/serve worker, history, box + `push_clip_detail`
  (metadata detail view/animation), `serve_newest_clip` (keeps newest pasteable), link
  enrichment (detect/snapshot/hero — see the link "share card" section).
- `unfurl.rs` — opt-in link unfurl worker (`link_unfurl`): curl + oEmbed/OpenGraph → `og:image`.
- `notif.rs` — notification OPTION: bell/DND, card rendering, footer, content preview.
- `renderer.rs` + `shaders/*.wgsl` — wgpu pipelines; frosted glass (`box_backdrop.wgsl`), SDF
  rounded rects, separable blur, neumorph/edge shadows, icon atlas.
- `animation.rs` — `lerp`, `ease_toward` (exp approach), `Follower` (damped spring / AGUA body).
- `install.rs` / `applier.rs` / `nix.rs` — declarative install flow + nixpkgs index.
- `hypr.rs` — all compositor IPC (dispatch + JSON reads).
- `panel.rs` — the control panel (the card as a field of settings pills); an open setting's
  content is `open_content`: Scale and Resolution from `display.rs`, every other setting from
  `controls.rs` (no placeholder is left: Colours was the last, 2026-10-14).
- `controls.rs` — THE OTHER SETTINGS (2026-10-13): what each one shows open and what a click on
  a choice does, in the idiom Scale and Resolution set — a subtitle, then groups of choice pills,
  the one in use lit (`ControlView { subtitle, groups: [Group { caption, key, width, choices }] }`).
  A worker thread (spawned at the first read, asleep otherwise) reads the machine (`read(label)`)
  and carries out a pick (`apply(key, value)`); every program it asks (`wpctl`, `pw-dump`,
  `nmcli`, `bluetoothctl`, `powerprofilesctl`, `gsettings`, `golem-brightness`, `xdg-mime`,
  `awww`, `locale`) is ended at 4 s (`finish`). The panel asks for a read when a setting opens
  (`control_due`) and after each pick; a pick is lit at once and the read confirms it. The keys
  that are the UI thread's (`ui_key`: `night` → `set_screen_temperature`, `dnd` →
  `toggle_notif_mute`, `open` → the gear box's page via `stats_show_page`) are run in
  `App::control_pick`. WHERE A CHOICE LIVES: the compositor's (keyboard layout, natural
  scrolling, pointer speed) follow the display settings' three steps — live `hl.config`,
  `~/.config/golem/settings.json`, the generated `settings.lua` (`GolemSettings::input_lua`);
  the SESSION's (language, time zone) are saved there too, handed to every app the dock starts
  (`launch::launch_env`), written for the next login (`~/.config/uwsm/env`, a marked block) and
  taken by the shell itself at once (`i18n::init`, `adopt_zone`); the rest belong to their own
  services (PipeWire, NetworkManager, dconf…). A `settings.json` written by something else — the
  installer seeds language, zone and keyboard — takes effect as the dock starts
  (`controls::reassert`). TIME ZONE is chosen in three folded steps (region → the hour the clock
  should show there → the place, a screenful at a time; `ZonePick`). Pointer-free:
  `waverunner-ctl display show <setting without spaces>` and `display pick <group> <n>` (both
  counted from 0; a pick of a place that is not there logs what is). ⚠ in a test rig these picks
  are REAL for what is not the compositor's: never pick volume, Wi-Fi, Bluetooth, dark mode or a
  power mode from a rig that can reach the live session's PipeWire or buses.
- QUICK CONTROLS (Max, 2026-10-13, Super+H → `waverunner-ctl fast-controls`): FAST LAUNCH for the
  control panel's settings — the same card, motion and ranking (`fast_launch.rs`,
  `FastLaunch::controls`): it opens on the three settings used most, each as ITS PILL as on the control panel
  (`App::fast_pills`, `content::FastPill`; round sign pills were tried and taken back the same
  day): the front one a size up, lit and ringed, its name dimmed with what was typed bright; the
  two at its sides slide out from behind it as the icons do; names match in the shell's language and in English, then by keyword; ties
  go to the setting used most (the settings panel's own standings, `Panel::use_count`). Enter (or a
  click) opens the one in front IN A BOX OF ITS OWN mid-screen (`fast.boxed`, `App::quick_open`):
  a third `Panel` in solo mode (`Panel::solo` — no field of pills, only the open setting, drawn by
  the same `open_content`/`draw_open`), growing out of the card, as tall as its content
  (`quick_rect`), on the dock's colour nearly solid (`content::QuickDraw`). A click on a choice
  picks it (`quick_click` → the same `control_pick`); Escape, a click outside or Super+H again
  folds it. A setting made of PICTURES — Wallpaper (Max, 2026-10-13) — takes nearly the whole
  screen (`QUICK_WHOLE`) and shows each as a preview (`controls::Group::tiles`, `Panel::flow_tiles`:
  as many columns as make them the biggest with every one still in the box; `PanelDraw::images` →
  `App::picture_layer`, the Files thumbnailer's textures, each fitted by its own shape); big
  pictures' previews are kept in `~/.cache/waverunner/previews`. For this the DOCK SURFACE is now
  never shorter than the screen (`scaled_surface_size`; everything else is laid out from its bottom
  edge). ZOOM: Ctrl +/−/0 while fast launch or the quick controls are up make the card and the
  box bigger, smaller, as shipped — one zoom for both (`settings.fast_zoom`, `App::fast_zoom`,
  `zoom("fast", …)`; `debug-desktop "zoom fast in|out|reset|<n>"`). THE KEYBOARD in an open setting (the box's AND the control panel's): the arrows and
  Tab walk the choices (`Panel::key_move`, a ring where the keys are — shown from the first key
  on), Enter or Space picks (`key_pick`); a setting chosen in steps moves the keys to its newest
  group (`key_follow`). Pointer-free: `fast-controls`, `debug-query <letters>` types, `debug-query <letters>!`
  also presses Enter, `display pick <group> <n>` picks in the box, `display key
  left|right|up|down|tab|enter|escape` presses a key there (or on the panel's open setting).
- `i18n.rs` + `tools/i18n/` — THE SHELL'S LANGUAGES (2026-10-13). Every visible string goes
  through `tr("English")`, a sentence with values through `fill(tr("… {name} …"), &[("name",
  &v)])`; the catalogue is `assets/i18n/<tag>.json`, built into the binary (Spanish is whole).
  The language changes live (the control panel's Language). A string that is ALSO an id (a
  setting's name, a menu's row, a stored marker like the card's "Voice note") stays as written
  in its table and is translated where it is drawn. `tools/i18n/README.md` has the tools
  (`scan.py` finds what is not wrapped, `apply.py` wraps, `extract.py --missing es.json`) and
  `WAVERUNNER_I18N=pseudo` (every translated string between « »). `cargo test` fails on a
  `tr("…")` with no Spanish.
- `modules.rs` — MODULES: the catalog (`assets/modules.json`) and its Apply
  (`App::modules_apply`, 2026-10-13): each package goes through `start_managed_install` /
  `Request::Remove` — the drag-install pipeline, so a module is ONE rebuild — a web program is
  its catalog webapp (`webapp_slug`), the task pill counts the programs home (`ModuleJob`,
  `module_program_done` from `nix::Event::Done`) and a notice says how it ended. The system a
  program needs beside its package (Steam's 32-bit graphics, Docker's daemon…) is Golem's
  `system/Modular/desktop/app-setup.nix`, keyed on the installed packages' names.
- `display.rs` — LIVE display settings (scale, resolution): reads the screen (`j/monitors`),
  applies an `hl.monitor` rule at once, saves the owner's choice to
  `~/.config/golem/settings.json` + the generated `settings.lua` that Golem's `hyprland.lua`
  runs last (so it is there at compositor start, dock or no dock). A resolution is only
  TRIED: it goes back after 15 s unless kept, and is saved only then. Pointer-free:
  `waverunner-ctl display [scale <n>|mode <WxH[@Hz]>|keep|back|reset|show <scale|resolution>]`.
- `transition.rs` — a scale change is DISSOLVED, not shown bare (every client redraws on its own
  frame and the compositor slides each layer surface: it read as the screen jumping). A still of
  the screen (`wlr-screencopy` → plain shm overlay layer, namespace `golem-transition`, no input)
  covers everything, the change is made under it with compositor animation and our own motion
  snapping, then the still fades out (`wp_alpha_modifier_v1`). The still's size at the NEW scale
  is committed just before the change so it never shows magnified. Any failure → the change is
  made bare; the still cannot outlive 2 s. `App::dissolve(output, new_scale, then)`.
- `gear.rs` — the gear box's pages with content: **Wi-Fi** and **Bluetooth** (the readout's
  first two readings). A page is a `View` (rows, cards, key/value lines, round buttons); ONE
  layout (`gear_layout`) places it and both the draw (`push_gear_page`) and the hit-test walk
  it. The footer stretches into a field for a password, a search or a rename, and the box
  holds the keyboard while one of these pages is open (`gear_sync` — also starts/stops the
  workers' polling; every path that closes the box must reach it). Backends, each a worker
  thread spawned on first use, polling only while its page is on screen:
  `net.rs` (NetworkManager through **`nmcli`**: list, join, forget, hotspot, manual address),
  `bt.rs` (BlueZ on the system bus, with its own pairing agent; the sound side through
  **`pw-dump`**/**`wpctl`**), `bt_files.rs` (OBEX on the session bus: send through the desktop
  portal's file picker, receive into Downloads; results go out as notifications).
  The machine pages (this computer, storage, processor, memory, graphics, battery) are views on
  the same engine in `gear_pages.rs`, fed by `sys.rs` (`/proc`, `/sys`, `lsblk`, `udisksctl`,
  `powerprofilesctl`; folder sizes on a thread of their own). A row exists only where the
  machine can answer. Things that cannot be undone are asked once (`gear_arm`: the footer says
  what a second click does). Standing choices: idle times are a copy of Golem's hypridle config
  with the owner's numbers plus a unit drop-in (`apply_idle`); "lid does nothing" is an inhibitor
  the dock holds; apps on the fast card get NVIDIA's offload variables at launch (`gpu_exec`).
  Pointer-free: `waverunner-ctl debug-gear <open net|bt|gear|disk|cpu|ram|gpu|bat>`, then
  `detail|hotspot|files|page <about|health|clean|drive|folder|card|app|system>|close|…`.
- DRAGS AND FOCUS (Max, 2026-10-10, macOS's way): on Golem a drag does NOT focus or raise the
  windows it passes over (Golem's hyprland.lua: `misc.always_follow_on_dnd = false`), so a file is
  carried across one window onto another under it. HOLDING is how a window is asked forward: over
  a window, the compositor does it (Golem's `hyprland-dnd-hold-raise.patch`, 650 ms within 14 px);
  over an app's tile in the dock, `desktop.rs::dock_spring_*` does (`Spring`: any drag over the
  dock — the dock comes up for one that is not the desktop's own — held `SPRING` on a tile with a
  window → `hypr::focus_window`, or `restore_minimized`).
- `look.rs` — THE LOOK, light or dark (Max, 2026-10-14): one flag (`look::dark()`), set at start
  from `controls::dark_now` and by the control panel's Dark mode (`App::look_set`). It picks the
  ICON THEME's tone — Golem's own icons come as `Golem` (silver) and `Golem-Dark` (graphite),
  made by Golem's `tools/icons`; `look::icon_theme` maps the configured name and every
  `IconLoader` rebuilds itself when `look::generation()` moves (`IconLoader::sync`) — the dock's
  three figures (`look::figure`: the cast silver recoloured graphite), and Golem Files' colours
  (`filer::Palette`). A change rescans the dock and drops the desktop's and the file windows'
  theme pictures (`desktop_look_changed`, `filer_look_changed`); `controls::assert_dark` also
  writes dconf's icon-theme for the apps.
- `palette.rs` — THE PALETTE (Max, 2026-10-14: "the pallete have to be everywhere, like omarchy
  does"): sixteen colours in the base16 layout (base00 ground … base07 brightest ink, base08..0F
  the hues), a dark and a light variant and an accent slot. Golem's own is `assets/palettes/
  golem.json` (its base01 IS Seam's chrome colour rgb(29,32,38); its light greys ARE the light Files
  Max tuned), beside Catppuccin, Gruvbox and Nord (tinted-theming base16, hues in the standard
  slots so the accent's names stay true; `every_built_in_palette_reads_and_reads_well`); the owner's go in
  `~/.config/golem/palettes/*.json`. The choice is `colors` in `settings.json` (`ColorChoice`:
  `dynamic`, `palette`, `dark_websites`); the ACCENT is always the palette's own (its file's
  `accent` slot — Max, 2026-10-15: "should come from the pallete, not apart"; no picker). DYNAMIC ON (default) = the shell
  exactly as before (screen sampling); OFF = `Backdrop::palette()` in `options.rs` stands in for
  the sampled regime of the BOXES and the DOCK (the palette's surface as a match + its ink); the
  BANNER and its pills stay on the screen's colours whatever the mode (Max, 2026-10-15: "i dont
  want the banner to be opaque… should stay as it is" — `options_regime`/`clip_regime`); the window border is the accent all round and
  the inactive border base02 (`border_stops`, `push_window_border`). `App::colors_apply` (start,
  a pick, a look change) resolves the `Theme` and `publish`es it on a thread into
  `~/.local/state/golem/theme/`: colors.json, foot.ini (both variants, for new windows; OPEN
  terminals are recoloured LIVE by colour sequences written into every foot child's pty —
  `foot_osc`/`foot_ptys`, the pywal way), hyprlock.conf ($golem_* variables), gtk4.css (libadwaita variables, both looks via
  prefers-color-scheme), gtk3.css, qt.conf (+ qt5ct/qt6ct.conf when nobody else owns them), and
  dconf's accent-color (nearest libadwaita name). Golem's home layer points every app there.
  SIGNAL COLOURS follow it too (`palette::accent_srgb/danger_srgb/warn_srgb`, sRGB fractions like
  the consts they replaced): the bell's/task's/phone's chip and the gear's warnings = the accent,
  every "throw away"/recording/battery red = base08, the bar's warn glyph = base0A, the phone
  Configure box's accent = the accent. Golem's palette gives back the old #ffbe98 / #e05252.
  "Dark websites" (`colors-websites`) is on the DARK MODE page, not Colours (it is a choice of the
  dark look; Colours must fit a 900 px field — the panel does not scroll). The setting is "Colours" in the control panel / Super+H (`controls.rs`, keys `colors-*`, run on
  the UI thread in `App::control_pick`).
- `lighting.rs` — AUTO LIGHTING (Max, 2026-10-18; Colours page: "Auto" beside the Lighting slider, "Include the
  background" under it): the Lighting follows the sun (`line`: Light 60 from +45°, one even slope down; DUSK round the horizon — Light down to 0 in the last 2.5° above it, Dark from 100 in the first 5° below (`DUSK_ABOVE`/`DUSK_BELOW`: ~20 min of stronger darkening, the switch at the looks' nearest) — to Dark 20 by -12°; the
  elevation is `options_engine::sun_elevation_deg`, the place from the timezone) and changes ONLY IN FRONT OF THE
  OWNER (Golem §7). Presence = ext-idle-notify-v1 (120 s "away", 3 s "quiet"; `hyprlock` running or a sleep =
  gone); `Plan` (pure, tested) says when to look: present AND settled (2 min after a dock start; 2 s after the
  first touch back from away — `RETURN_CUE`) AND not under a fullscreen window, every minute (`SLOT_SECS`, one
  point of the line a fade, `MIN_STEP`); missed slots are never replayed. THE SWITCH between the looks never happens
  in front of the owner (Max, 2026-10-23, after a cut, 3/8/30 s dissolves and a twilight all showed): present, the
  light stops at the look's edge (`within_look`: Light 0 at dusk, Dark 100 at dawn); away, locked or just back from a
  sleep, the tick sets the sun's line AT ONCE, switch and wallpaper card included (`lighting_follow`/`lighting_set`). The look starts a
  20 s eased FADE: ~15 frames a second move only the SHELL's pair (`palette::ShellPlan`, made once; a frame on the
  same step redraws nothing) and the desktop's VEIL; the end saves the step (what the owner last saw) and the apps/
  web/files are made for it once, at the next quiet moment (`colors_apply`). Auto turned ON is the owner's act: the
  sun's line is set at once; the slider turns Auto off. THE VEIL: a black wash under the desktop's icons (`Lighting` <
  50, `veil_alpha`), a `RectInst` with `glass = VEIL_GLASS` (`rounded_rect.wgsl` skips the scene's fade for it).
  A change of the Lighting alone is a change for `colors_apply` (`lighting::applied`).
- `wall_follow.rs` + `palette_card::wall_svg` — THE WALLPAPER THAT FOLLOWS YOUR COLOURS (Max, 2026-10-18): the
  wallpaper box's FIRST tile is the palette in use as its card, "Follows your colours" (`Choice::caption` = a
  picture with its words under it, `Panel::flow_tiles`; its `value` is the card's PNG, a pick of it is told by where
  that file lives, `wall_follow::is_tile`). Picked, `GolemSettings::wallpaper_follows` turns on and the palette's
  card is made for the screen — the card's capsules about half its relative size, centred and a little low, the
  ground run out to the exact shape, resvg at the screen's pixel size (`display::wallpaper_size`: the biggest
  connected screen), a ±1 dither (`palette_card::dither`) against banding — into `~/.cache/waverunner/palette-walls/`
  (named by colours + size + version, written whole then renamed, the newest four kept) off every loop, and put up
  through the ordinary pick (`controls::pick_wallpaper`: the picture program's 1 s fade). `wallpaper` holds that
  file while it follows, so the login restore, the box's mark and Dynamic need no knowledge of it
  (`wall_follow::login_picture` makes it again if it is gone). A palette or shell-look pick in Colours calls
  `wall_follow::refresh` (`App::colors_pick`). DYNAMIC AND FOLLOWING ARE OPPOSITES: with Dynamic in use the tile says
  "Choose a palette to use this" and does nothing; choosing Dynamic ends following (`after_palette`) and the last
  picture stays as the wallpaper; any other wallpaper pick ends it too (`picked_other`, under the same lock a
  finishing picture takes). Samples: `cargo test -- --ignored four_samples` → `/tmp/claude-1000/follow-samples/`.
- `zoom.rs` — ZOOM (Max, 2026-10-10): Ctrl +/−/0 make the top bar and the desktop bigger, smaller,
  as shipped. The BAR on HOVER: compositor binds made on the pointer's enter and taken away on its
  leave (`bar_zoom_keys`, the card's way) → `waverunner-ctl debug-desktop zoom bar in|out|reset`;
  its zoom multiplies `App::options_scale` (banner, pills, boxes; `sync_options_zone` does the
  rest, as for a change of screen). The DESKTOP when FOCUSED: the keys come through
  `desktop_shortcut`; its zoom multiplies the icon scale of its grid (`desktop_icon_scale`), an
  icon keeps its cell. Both kept in `settings.json` (`bar_zoom`, `desktop_zoom`; 0.7–2.0 by
  tenths). The desktop's names grow with its icons (`Live::text`). The DOCK box has its own Ctrl +/− (four icon sizes, while it holds the keyboard) and Ctrl 0 puts it back.
- `phones.rs` + `desktop_config.rs` — a PHONE'S SETTINGS and its CONFIGURE box (Max, 2026-10-10,
  mockup `~/phone-configure-mockup`): the "Configure" row of a phone's menu (above Properties, a
  click) opens the box, on the menus' surface; nothing changes until Apply. A phone is known by
  its SERIAL (`phones.json` in the data dir, `Phones::of`); its things are kept in ONE folder
  `~/Phones/<name>` (the name typed by its owner — two phones of one model are two phones), a
  subfolder a KIND (`phones::KINDS`: Photos ← DCIM/Pictures/Movies, Music, Downloads, Documents,
  Recordings, WhatsApp; `phones::place`). The box: one `build()` places every part and its click
  rect (draw and pointer both walk it); the name is a field (the desktop takes the keyboard while
  it is typed, as for an icon's name); a choice is a pill that a click turns. SYNC OVER WI-FI:
  Apply allows the plugged-in phone over adb TCP (`desktop_phone::wifi_allow`: `adb tcpip 5555`,
  its wlan0 address kept); every `SYNC_TICK` (60 s) `phone_sync_tick` knocks on each synced
  phone (`wifi_there`, a 1 s TCP connect), and one that is there and due (`Every`, or just
  arrived) and charging if asked is synced (`phone_sync` → `desktop_phone::transfer`, on the task
  pill, a notification only if something came); on its cable and silent over Wi-Fi, it is allowed
  again (a phone forgets at restart). CLEAN: asked once more in the box, then `transfer(…,
  remove = true)`: everything of the ticked kinds comes here and each file leaves the phone ONLY
  once its copy here is whole (`Item::here`, by size; `rm` a batch, names quoted) — run against a
  real phone by the ignored test `a_real_phone_is_copied_from_and_cleaned`
  (`GOLEM_TEST_PHONE=<serial> cargo test -- --ignored a_real_phone`). "Import photos" is the same
  transfer for the photos kind into `<folder>/Photos`; an older import's `~/Pictures/<phone>` is
  adopted there by a rename (`adopt_pictures`). Pointer-free: `debug-desktop "config <n>|sync|kind
  <i>|every|charging|after|clean|really|notnow|apply|cancel|name <text>"`.
- PRINTING AND SCANNING (2026-10-21/22; Golem's system side is `desktop/printing.nix`): the engine
  speaks CUPS itself — `options-engine/src/cups.rs`, IPP over the scheduler's socket (401 → asked again
  with `PeerCred <user>`; an answer read to its length, CUPS keeps a connection open after a 401):
  snapshot, a job, cancel/restart, pause/resume, default, a printer's IPP `*-default`s (colour,
  quality, media, sides — CUPS maps them onto the driver), print a file or bytes, delete, the ended
  prints since a time — and `collectors/printing.rs` (`Layer::Printing`, in `start_shell`) follows it
  through a CUPS `dbus://` subscription (org.cups.cupsd.Notifier signals wake a look; 3 s / 60 s
  timer behind it; a print that left the list is asked how it ended and kept 60 s). ⚠ CUPS's
  `job-media-sheets-completed` is pages RENDERED, not out of the printer: no percentage anywhere.
  `printing.rs`: every unfinished print is a task on the TASK PILL ("Printing x", "Waiting to print
  x", or what stops its printer — "… is out of paper / off / paused"); `Task::number` Some("") = no
  number, the fill whole at its end; the pill's Cancel cancels the print (`printing_task_cancelled`);
  a failed one is a notice. `printers_page.rs` = Controls → PRINTERS: the status line (with the ink),
  section words Printers · Scanners once a scanner is there, the printer chooser with several,
  Colour / Quality / Paper / Both sides, its prints (Cancel; the last hour's ended: Print again) as
  many as the box has room for (`PAGE_ROWS`: the page does not scroll), Remove (asked) · Pause|Resume
  · Make default · Test page (Golem's own, Latin-1 PostScript), and a field "Add a printer by its
  address" (Enter → `lpadmin -m everywhere` on ipp://a/ipp/print, ipps://, /ipp; named after its
  make and model). Print-time problems are said only while a print is on its way (CUPS kept "printing
  job 0, out of paper" after a cancel). INK of an HP that does not tell CUPS: Golem's
  `golem-printer-ink` (HPLIP's device code with D-Bus off — hp-levels starts a tray icon and an
  update check), off the worker, 10 min, never while a print holds the printer. `scanners.rs`: the
  watcher keeps the scanners SANE lists (`known`, `look_now`; webcams and SANE's test device out
  unless `WAVERUNNER_SCANNER_TEST`), the first ever installs Simple Scan once; Scan = `simple-scan
  <device>`. Rig: `CUPS_SERVER=/run/host/run/cups/cups.sock DBUS_SYSTEM_BUS_ADDRESS=unix:path=
  /run/host/run/dbus/system_bus_socket` (+ host bin on PATH for lpadmin/golem-printer-ink), `display
  show printers`, `display pick <g> <i>`, `display type <text>|enter`; the engine alone:
  `cargo run -p options-engine --example printing [cancel|restart|resume|pause|default|options|set|print …]`;
  CUPS answers as a driverless printer at ipp://localhost:631/printers/<queue> (an add to test with).
- `tasks.rs` + `task_pill.rs` — TASKS: the long things being done, as ONE pill on the OPTIONS bar
  (what on the left, the pill filling like the player's track, the percentage on the right in a
  slot reserved at "100%"; left of the bell, growing out of its right end so it moves no other
  OPTION; `PillId::Task`, inert on click). GLOBAL by design (Max, 2026-10-09): any work is wired in
  with `let task = app.task_begin("Moving x.iso to Home")` on the loop, `task.set(done, total)` from
  its thread (reports thin themselves out), and dropping the handle ends it — the pill shows the
  newest running task (`+N` for the others), a finished one lingers `LINGER` at 100%. Wired so far:
  the photo import (bytes) and files dropped on a phone. Pointer-free: `waverunner-ctl debug-desktop
  "task <seconds> [what | another | …]"` (one task a name), `debug-desktop "taskbox open|close"`.
  It stands CENTRED between the left band's end and the window's pill (else left of the bell),
  a `STRETCH` longer than its words need. SEVERAL: a scroll down on it opens its BOX — the pill
  growing down into the other tasks, a row each (`push_task_rows`, the playing box's morph and
  zebra); a scroll up or leaving the bar folds it. From the right in the pill: `+N` (the bell's
  amber chip), `Cancel` (same chip; `Tasks::cancel` → the worker's `task.cancelled()` → it
  stops where it can; the task leaves at once), the number. A CLICK on the band COLLAPSES it to
  its number and `+N` (no words, no fill; `task_pill_click`); collapsed, hover shows it whole
  and leaving puts it back, a click while whole keeps it whole (`update_task_hover`). The look
  was settled on `~/task-pill-mockup`. `debug-desktop "taskbox open|close|collapse|peek|unpeek|cancel"`.
- THE CAMERA PILL is gone (Max, 2026-10-11, ff99e88: "get rid of the camera pill"): a phone used as a
  camera is driven from the PHONE pill's box — its camera page (flash, lens, picture, each a line a click
  sets; `phone_pill.rs`) — and the phone's menu (Use as camera / Stop camera, `desktop_camera_use`).
- `phone_pill.rs` — the PHONE pill (Max, 2026-10-11; the start of "a proper phones pill, not only
  for the cam but the whole menu"): on the OPTIONS bar while a phone is plugged in
  (`Desktop::volumes`), a ROUND pill with a phone on it, left of the bell. The pointer on it
  brings a LONG pill out from behind it, leftward, the way the bell's preview comes out: whose
  phone and how it stands — `Max's Pixel · Synced 2 h ago` / `Syncing…` / `Sync off` / `Not set
  up` (not in `phones.json`) / `Locked` (plugged in, not open). A CLICK on the round pill EJECTS
  the phone (`desktop_eject`). One `PillId::Phone`; what stands by the bell stops short of it
  (`phone_pill_reserve`: the task pill's fallback place, the camera pill's right end), and what
  it opens over leaves the bar while it is out (`options_pills`, as for the bell). Pointer-free:
  `debug-desktop "phonepill fake [words]|gone|open|close|free"` — never a click (it ejects).
  ITS BOX'S MENU IS THE ONE MENU (Max, 2026-10-21: "in the one menu look"): the first page of the
  box is a `desktop_menu::Menu` (`phone_menu_ctx` → `phone_menu_sync`, every frame of the bar),
  HELD by the box (`Menu::fit`: its corner, its width, at most its height — past that it scrolls)
  and drawn by the menu's painter with the panel left to the box (`Menu::push_rows`): the same
  symbols (from the desktop's warmed glyphs, put after the covers in the bar's icon array —
  `phone_glyphs_sync`, `set_options_icons`' 4th slice), the quiet accent tint, the red, the menus'
  Ctrl +/−/0 size (the bar's own zoom binds act on `menu` while the box holds the keys). The box
  keeps its header (the long pill) and its place under the pill. The KEYBOARD is the box's while
  it is open (not on the Configure page, whose fields want it; `set_phone_keyboard`, shared with
  the clipboard's and the gear's grabs): `Menu::key` on the menu (a page row turns the box's page —
  `menu_act`), ← / Escape / Backspace back to the menu from a page. Rig: `phonepill fakehelp` (a
  phone whose USB debugging is off: the help rows), `phonepill key down|up|home|end|enter|left|right|
  escape|<letter>` (a fake phone turns pages and does nothing else).
- `desktop_menu.rs` + `glyphs.rs` — THE ONE MENU (Max, 2026-10-21, mockups v50/v52 "All menus":
  "Build all of it"): EVERY right-click — Files' file / folder / several / archive / app shortcut /
  empty space / bin item / bin / sidebar place, the desktop's wallpaper and what stands on it, a
  stick, a phone (and the bar's phone pill's box) — is built by ONE builder, `rows_for(&Ctx)`
  (`Ctx`: `On` = what was clicked, plus what the rows depend on: `here`, `clip`, `desk`, `stick`,
  `undo`, `moving`, `volume`…). The same ORDER everywhere: Open · Copy/Move · Change · Send · Keep ·
  Get info · the way out LAST (Move to bin, or Eject). A row is a SYMBOL + sentence-case words
  (`Row::Item { label, icon, action, danger, off }`; the symbols are the Files mockup's strokes,
  `glyphs.rs` — written once as SVGs, read by the icon loader by path: a file window uploads those its
  menu wears with its own icons, `chrome::icon_keys`; the desktop's top surface lends its first layers
  while a menu is up, `top_glyphs_sync` — Quick Look takes them back; EVERY symbol is read once ahead of
  the first menu — `filer_glyphs_warm` into `Filer::glyphs` for all the windows, `top_glyphs_warm` for the
  desktop — and tests hold every name a row, the pill, the ↶ line or a kind wears to `glyphs::GLYPHS`: an
  unknown name is an empty gap by its words); `›` = the row turns the menu's
  PAGE (`Page`: Open with · Send to · Sort by, each led back by "‹ <its name>"; `Menu::turn` /
  `show_main`; no flyouts); "…" = opens a window (Format…, Wallpapers…, Colours…, Display…,
  Configure…); RED only for what cannot be undone (Delete for good, Empty the bin, Format…). ⛔ NOTHING
  OPENS ON HOVER (Max: "i dont want the open on hover on the menus"): pages and Get info on a CLICK. THE
  KEYBOARD (Max, 2026-10-21; `Menu::key` → `MenuKey`, the hosts `filer_key` / `desktop_shortcut`): ↓/↑ the next
  / previous row that does something (over rules, notes, rows with nothing to do; round the ends; none
  highlighted: the first ↓ takes the first, the first ↑ the last), Home/End, Enter or Space choose, → on a ›
  row turns its page with its first choice highlighted, ← back with the row that turned it highlighted
  (`turn` / `show_main` when the keys drive), Escape closes, a letter the next row it begins. The keys'
  highlight is the hover's tint and stays theirs until the pointer MOVES (`Menu::pointer`, by where the pointer
  stood — hosts update the hover only through it). In Files Shift+F10 / the Menu key open the menu on the
  selection or the folder (`filer_menu_from_keys`). Rig: `files key down|up|home|end|enter|space|right|left|
  shift f10|menukey`, `files type <letter>`, `debug-desktop "key down|up|left|right|space|escape"`. ANOTHER APP
  (Max, 2026-10-21; `other_apps.rs`): the Open with page ends, after a rule, with "Another app" › — a page
  (`Page::OtherApp`, `OtherApps`, `other_rows`, `Menu::show_other` / `other_found`) of every app that opens files
  (`openers`: its desktop file's Exec takes `%f %F %u %U`, web apps left out), A→Z with its OWN icon (a row's
  `pic`, an icon key drawn in its own colours; the file window's `icon_keys` and the desktop's
  `top_glyphs_sync` load `Menu::pictures` as they do the symbols); a SEARCH line at the top (`Row::Search`,
  `search_type`: what is typed filters by substring — Space types there too —, the first found highlighted);
  at the foot "Always use for <kind>" (`kind_of`: shared-mime-info's comment in the shell's language —
  `i18n::lang` —, English made plural), off at first (`toggle_always`, "box"/"boxed"). Choosing an app opens the
  file (`open_with_other`); with the check on, `xdg-mime default` makes it the app (`other_app_default`), it is
  listed first on Open with, and the history keeps `Undo::Default` ("Opened PNG images with Image Viewer from
  now on"), taken back to the owner's OWN choice before (`chosen_default`, read from mimeapps.list; none → its
  line taken out — `unset_default`). PAGES SCROLL (`Menu::scroll`, `pinned`, `capped`, `middle`, `scroll_by`,
  `reveal`): no page taller than the surface; Another app at most `PAGE_ROWS` (12) apps, its search and check
  pinned; the rows between scroll under the wheel and follow the keys; a row scrolled out is neither drawn nor
  hit (`seen`).
  The look: rows 29 px, 14 px symbols at 75 % of the ink, hairline rules between groups, the width the
  words want (214 at least, `width_of`). THE ROW UNDER THE POINTER is a QUIET HOVER (Max, 2026-10-21, after
  macOS and Windows: both are instant, and the blink was our accent band with white words flipping on every
  row — a glide and then a crossfade of it were both taken back): a SOFT TINT at once ("put at least some of
  the colour on the selection") — the panel mixed toward the accent (`MenuPaint::accent`: the palette's, the
  Files window's own) 16 % in the light, 22 % in the dark, toward the red on a red row 14 % / 20 % (`HOVER`,
  `HOVER_RED`, `MenuPaint::hover`, mixed in sRGB, solid as the menu), and never so far that the row's words
  read worse than 4.5 : 1 (or four fifths of what the bare panel gives): a very light accent in the dark stays
  a tint; at the row's radius; the words and symbol keep their ink, a red row its red — never white; none on
  a rule, a note or a row with nothing to do. THE RED (Max, 2026-10-21: tangerine's light "Delete for good" read
  grey): the palette's own red only when it is CLEARLY red as the eye sees it (`reads_red`: OKLCH hue 10°–40°,
  chroma ≥ 0.11 — tangerine's #73310e is a brown at 45°) AND reads at 4.5 : 1 on the panel; else Golem's
  (#c62828 on a light panel, #ff6b6b on a dark one) walked toward black or white until it does (`danger_on`,
  `MenuPaint::danger`); the hover keeps a row that read at 4.5 : 1 at 4.5 : 1. Every shipped palette, both looks,
  the Lighting's and the Saturation's ends, passes (`every_palettes_danger_rows_read_red`, which prints which
  keep their own: 48 of 236). The selection pill's buttons stay neutral (the panel 7 % toward
  the ink). MENU SIZE (Max, 2026-10-21: "add ctrl +/-/0 to
  the context menus (make it bigger or smaller)" — for kids and grandmas): while a menu is OPEN, Ctrl + (=, keypad
  +) / Ctrl − / Ctrl 0 and Ctrl + the wheel over it make it bigger, smaller, as shipped — 80 % to 200 % a tenth a
  step, rows, words, symbols, air, corners and width together, at once (no motion). ONE size for every menu,
  kept (`settings.menu_zoom`, `desktop_menu::scale`/`set_scale`, read the first time a menu opens; `App::zoom("menu", …)`): every menu
  opens at it and one that is up is laid out again (`Menu::rescale`, `filer_menus_rescaled`,
  `desktop_menu_rescaled`), its row under the pointer what is under it now. Its corner STAYS (`Menu::hang`: an
  anchor and which edges hang from it — the pointer's corner as it opened, the pill's ⋯ hanging up, the sort
  menu down from its button), shifted back onto the surface where it would leave it (`place`; back where it
  was at its old size). While a menu is up those keys are the MENU's (`filer_key`, `desktop_shortcut`), not
  the window's zoom; the DESKTOP takes the keyboard while its menu is up (`Desktop::keys_for_menu`, given back
  when nothing is up — `sync_desktop_top_input`), so Escape closes it too. Rig: `files key ctrl plus|minus|zero`,
  `debug-desktop "zoom menu in|out|reset|<n>"`; real keys through the nested compositor with
  `hl.dsp.send_shortcut({ mods = "CTRL", key = "equal" })` (no window: the keyboard's surface). ⚠ WORDS ARE
  PAINTED AFTER EVERY GRID (the renderer's one text pass): a big menu over the selection pill covered its
  symbols and showed its words — so in a file window every word of the chrome under the menu or the box (the
  pill's, the top line's, the i card's) is LEFT OUT, as the files' names are (`chrome::leave_out`, by the width
  it is drawn at). The symbols were never the matter: one picture a symbol, drawn at any size. NEW ROWS: Open in new tab (`filer_tab_open`), Quick look, Duplicate ("name copy",
  `filer::copy_name`, undoable), Paste into folder, Select all, Copy to <the one stick plugged in>
  (`one_stick`), Send to › (a COPY now), New folder with these ("New folder with items", the selection
  moved in, its name typed; `Undo::Grouped`), Get info (Files: the i card; a place, the desktop: the
  Properties box). THE DESKTOP'S WALLPAPER: Undo <what> (`Desktop::history` — its own, 30 kept, recorded
  by rename, bin, new folder, paste, Move to Home, duplicate, send, group; `desktop_undo`, Ctrl+Z) · Paste ·
  New folder · Select all · Sort by › (`desktop_sort`: folders first, then the key; volumes keep their
  cells) · Clean up · Hide/Show icons · Next wallpaper (`controls::next_wallpaper`: the next picture of
  Pictures/Wallpapers, round, through the box's own pick) · Wallpapers… / Colours… / Display… (the
  setting's box mid-screen, as Super+H opens it: `App::settings_box`) · Pause/Play moving wallpaper (only
  while it moves: `controls::HOLD_USER`). Rig: `files menu [n]|placemenu <n>|rows|pick <row>`,
  `debug-desktop "menu [n]|rows|pick <row>"`.
- `desktop.rs` — the DESKTOP: `~/Desktop` as icons behind the windows (macOS/Windows style) on
  its own `Bottom`-layer surface (`surface::create_desktop_surface`; Bottom, not Background, so
  swww can never map over it) with its own renderer + icon array (layer = item index). Lists the
  folder (folders first, case-insensitive; `.desktop` files are launchers with their app icon)
  onto a cell `Grid` at the dock's icon scale (side margins equal and HALF of what centring leaves — `Grid::x0`; the rest of the spare width is shared between the columns, `Grid::pitch`): each file's cell is REMEMBERED in cell units
  (`desktop.json`, path → [col,row]; `place` + `stick`), new files take the first free cell
  column-major and move nothing, `forget` re-flows. Draws the Files-section tile
  (carrier/thumbnail + its name on up to TWO lines as Files' grid — `filer::two_lines`, measured,
  cut in the middle past two (Max, 2026-10-22) — white on a dark shadow; a selected item's wash
  reaches over a second line and the wash of the one under it begins past it; `Desktop::hit` looks
  at the cell above the pointer too, where a second line reaches); no hover magnification (Max). Click = select, DOUBLE click (400 ms) = open (`xdg-open` / the launcher's `Exec=`).
  DRAG = a real Wayland drag of ours (`desktop_lift`: `DragSource` offering `text/uri-list` +
  text, the icon raster on an shm surface as the drag image, `start_drag` with the press serial
  — Hyprland ignores it): over the desktop nothing is shown (no landing-cell wash — Max) and
  the drop settles the item in the free cell nearest the pointer (`desktop_dnd_drop`,
  own-drag branch); over the dock the bin reacts
  (`desktop_drag_dock_pos` = the DnD motion on the dock surface; the dock is raised for the
  drag) and a drop on it → `trash_file`; over any other app it arrives as a file
  (`desktop_send_drag` writes the payload on a thread). `DataSourceHandler` (main.rs) ends it:
  `dnd_finished`/`cancelled` → `desktop_drag_end`. DROP TARGET for other apps: the same
  `DataDeviceHandler` routes a `text/uri-list` drag over the desktop to `desktop_dnd_enter`
  (accept Move|Copy), on drop read the pipe on a thread (`OwnedFd::from(pipe)`, NEVER
  `into_raw_fd` — SCTK closes it) → `import` (move via rename, else copy — a USB file stays on
  the stick; clashes get `name (2).ext` before the FIRST dot) and cluster the files around the
  drop point; a `leave` right after `drop` is Hyprland's habit and is ignored. The input region
  is the WHOLE surface (a drop only reaches a surface through its input region). inotify on
  the folder reloads. Icons come from its own `notif_icons` worker (`unplated` for carriers);
  thumbnails ride the Files thumbnailer (`desktop_on_thumb`). `[desktop] enabled/render_scale`
  in config; `WAVERUNNER_DESKTOP_DIR` overrides the folder (test rigs). Pointer-free:
  SELECTION: a press on bare wallpaper that travels draws a rubber band (`band_hits`: icons
  + names it touches are selected, by path); AN ITEM IS ITS PICTURE AND ITS NAME (Max, 2026-10-22,
  as in Files: `Desktop::thing` — what its picture shows, `seen` from `filer::ink_box` as it goes
  into its layer, and its name's width as last written, `names_w`): `hit` finds nothing on the rest
  of a cell, so a band starts and the wallpaper's menu opens between icons, and a drop there is not
  into a folder (rig: move the pointer, then `debug-desktop "pointer press|release|rclick"`);
  a press on an unselected icon selects it alone;
  a drag of a selected icon takes the whole selection (`Drag.items`, grabbed one first;
  payload = all URIs; a drop on the desktop lands them keeping their arrangement via
  `place_group`, on the bin trashes them all); a click on wallpaper clears, opening clears, and so
  does a RIGHT click on it (Max, 2026-10-22: "the selection should be cleared" — `desktop_open_menu`:
  the wallpaper's menu acts on the desktop; on an icon not selected it is selected alone first, on a
  selected one the selection stays).
  Selected items wear one soft wash (`sel_rect`, the mockup at ~/desktop-menu-mockup).
  Pointer-free: `waverunner-ctl debug-desktop [reload|open <n>|move <n> <col> <row>|import
  <col> <row> <uri…>|select <n…>|band <x0> <y0> <x1> <y1>|forget]`; the live daemon logs
  to `~/.local/state/waverunner/daemon.log`. HIDE/SHOW: a click on bare wallpaper toggles the
  icons (fade; `settings.desktop_hidden`). MENU (`desktop_menu.rs`, THE ONE MENU — its own entry above): right-click → `Menu::new` at the
  pointer (`desktop_menu_ctx`: the wallpaper's, a volume's, an item's), drawn as a `GridContent`
  so it paints over the icons (names under it are skipped), a left press on a row acts on
  release (`desktop_menu_row`), elsewhere closes; `debug-desktop menu [n]` / `pick <row>` / `rows`. RENAME in place: the name
  becomes a field (`Rename`, all-selected first key replaces); the desktop takes the keyboard
  EXCLUSIVE for exactly that long (a Bottom layer gets it on hover with Exclusive, never via
  OnDemand without a click) and `begin_keyboard_handback(KbSurface::Desktop)` returns it;
  keys route first in `main.rs::handle_key_event` → `desktop_key`; a keyboard `leave` mid-name
  keeps what was typed. New folder = "untitled folder" at the clicked cell → rename. PROPERTIES
  (`desktop_props.rs`): the menu's Get info, opened by a CLICK — the menu becomes the box (the
  panel lerps from the menu's rect, `grow_from`; `Props.back` keeps the menu, the `‹ Back` row
  turns it back on a click, any other press closes). Lines: Kind, Contains (folders/files/hidden), Size (a
  folder's tree walked on a thread, `folder_size`), Dimensions (images), Link to, Where, Owner,
  Access, Modified, Created, Opened. `debug-desktop props <n>` opens it without a menu. Not yet:
  per-output surfaces, Escape closing the menu (no keyboard while it is up). SEND TO (Move to until
  2026-10-21; Copy and Move are rows of their own now): that row turns the menu's PAGE
  (`App::send_targets_ask`, for the desktop's menu or a file window's — `SendFor`; `desktop_menu::send_rows`;
  `‹ Send to` → `show_main`): "Plugged in" — the sticks and phones · "On the network" — the other computers. The lists are
  `desktop_send.rs`: sticks = volumes mounted under `/run/media/<user>` or `/media` (read from
  `/proc/mounts` at once; an unmounted stick is not listed) PLUS every volume on the desktop
  (`Desktop::volumes` — a phone goes to `phone_dest`: its first storage's Download), devices = KDE Connect's paired AND
  reachable ones (`kdeconnect-cli -a --id-name-only`, asked on a thread — "Looking for devices…"
  until it answers; a `desktop`/`laptop` type, read with `busctl`, goes in the second list).
  `Action::SendTo(n)` indexes `Desktop::targets`; `desktop_send::send` runs on a thread (`App::send_to`): a stick
  gets a COPY (then `sync -f`; taken back as `Undo::Copied`), a device a SEND
  (`kdeconnect-cli --share`, the file stays; folders are skipped), and the outcome is a
  notification (`desktop_send_notify`). PASTE (wallpaper menu, only when the newest clip is
  files — `App::clipboard_files`): `desktop_send::put` copies them in, or moves them if they
  were cut, around the click. PLUGGED-IN VOLUMES (`mounts.rs`): one worker follows the session's
  volume service through `gio` (`gio mount -o` for changes, `-li` for what there is —
  `parse_listing`) and MOUNTS every removable volume and phone (MTP/gphoto2/AFC) that wants
  automounting, once per time it appears (so an eject is respected while the stick stays in);
  EVERY ANDROID LANDS (Max, 2026-10-09): a phone is told apart by its address AND its USB node (it re-enumerates when its owner allows the computer — by the address alone it stayed "already tried"); one that refuses to mount (locked) is asked again every `TICK` while plugged in; the USB bus itself is read every `TICK` (`usb_phones`, sysfs only: adb or MTP/PTP interface, or a phone maker's device that is nothing else) for phones the service does not show — one of those with adb allowed is switched to file transfer (`nudge`: `adb shell svc usb setFunctions mtp`, twice at most), and any phone that is plugged in but not open stands on the desktop CLOSED (`closed()`, `Mounted::closed`, at the path its folder will have so the icon keeps its cell): Open says what to do on the phone, its menu has no terminal and no Eject, Mirror/camera work as ever; A PHONE'S OTHER ROWS (`desktop_phone.rs`, over adb, off the loop; Max, 2026-10-09): Import photos (`import_photos`: DCIM + Pictures → `Pictures/<phone>/<album>/`, only what is not there at that size — `parse_shots`/`missing`, `adb pull -a` a folder's batch; from the mounted folder when there is no adb), Use / Stop phone's internet (`phone_internet`: `svc usb setFunctions rndis`, then `ncm`, the cable saying whether it took — `UsbPhone::tether`; a tethering phone stands CLOSED and is never nudged back), Properties last (the Properties box with the phone's own lines, `facts`/`parse_facts`: model, Android, battery, storage); files DROPPED ON a volume's icon are copied onto it (`desktop_volume_under`, `desktop_drop_on_volume`: a phone's Download via `adb push`, a stick's top folder), from the desktop or from another app; the mounted list comes to `App::on_mounts` and each stands on the desktop as a `Kind::Volume`
  item (path = the mount's folder: `/run/media/<user>/<label>`, or `$XDG_RUNTIME_DIR/gvfs/
  mtp:host=…` for a phone — `mount_path`). A volume is NOT a file of the desktop's: its menu is
  Open · Open in terminal · [Mirror screen, a phone: `scrcpy --serial` from the mount uri, `phone_serial`/`mirror`; its window opens floating, centred and phone-shaped — `mirror_size` from `adb shell wm size`, declared with `window_memory::declare_sized`; `adb devices` is asked first (`phone_debugging`): not listed → "turn on USB debugging", unauthorized → "allow this computer", as a notification · Use as camera → the menu turns its PAGE to the phone's LENSES (`Menu::show_lenses`, `desktop_show_lenses`: `scrcpy --list-cameras` asked once per phone off the loop — "Asking the phone…" — `parse_lenses`: a back camera is ONE camera to Android and changes lens with the zoom, so Ultra wide = zoom 0.5 when its range goes below 0.6, Main = 1, Telephoto = 5 (or 2) when it reaches that, Front = its own id; `Action::Lens(n)` starts it, the one in use is ticked) — while it is on the menu itself has Stop camera (`Action::CameraStop`) and, right under it, the lenses (`desktop_menu::Volume`, the one in use ticked; picking one ends the camera and starts it through that lens): `camera()` = `scrcpy --video-source=camera --camera-id= [--camera-zoom=] --camera-fps=60 --video-bit-rate=16M --v4l2-buffer=50 --no-window --v4l2-sink=` the loopback device (`camera_device`, Golem's "Android WebCam" /dev/video10; fed = its sysfs `state` reads `capture`, `camera_fed`), running ones in `Desktop::cameras` (path → pid; the row SIGTERMs it; it dies with the dock, PDEATHSIG). The apps get it through `golem-camera-relay <device> <name>` (`camera_relay`; Golem's `system/home/phone.nix`: gst `v4l2src ! queue ! videoconvert ! YUY2 ! pipewiresink mode=provide`, PipeWire node `golem-phone-cam` under the phone's name) — the device lends an app only TWO frames (keep two → half the frames, keep three → none) and has one reader at a time; ⚠ never raise v4l2loopback's max_buffers (8 segfaulted PipeWire). Without the relay on PATH: a source made on the device by hand (`camera_announce`, `pw-cli create-node`, node `golem-phone-camera`; `camera_withdraw` at the end and at dock start). `adb devices` is asked first (`phone_debugging`): not listed → "turn on USB debugging", unauthorized → "allow this computer", as a notification] · Eject (`Menu::for_volume`, `mounts::eject` = `gio mount -e`, else
  `-u`); it is left out of Cut/Copy/Move to/Move to Home; on the bin (the row or a drag) it is
  EJECTED, never trashed; a drag of it offers a copy only; its remembered cell goes when it does. It stands on the RIGHT (`place`: first free cell
  from the top-right). Hide/show is a FADE of the whole surface (`scene.alpha`), volumes included.
  A device connected WHILE the icons are away shows ALONE once (`Desktop::solo`, `Live::only`,
  fading by the same surface opacity — one group fades at a time); the next wallpaper click
  brings the others back beside it (at once, no fade: one surface opacity), the one after puts them all away.
  MENUS ON TOP (`desktop_top.rs`): the menu and the Properties box are drawn on a SECOND layer
  surface (`waverunner-desktop-menu`, Overlay, same anchors and size as the desktop's so points
  agree) — the desktop is under the windows and its menus must not be. It takes the pointer over
  the whole screen only while one is up (a click anywhere else, a window included, just closes
  it); its pointer events go to `desktop_pointer` (`PointerSurface::DesktopTop`). Without it
  (`desktop_top_ready` false) they are drawn on the desktop as before.
  KEYBOARD (`desktop_take_keys` / `desktop_keys_arrived` / `desktop_drop_keys`, `Desktop::keys`):
  the layer asks for NO keyboard at rest (on-demand would be handed it on mere hover). A left
  click takes it EXCLUSIVE (immediate), and on the keyboard `enter` it drops to ON-DEMAND at
  once — ⚠ an exclusive layer has the POINTER pinned to it too (Hyprland `m_exclusiveLSes`:
  every click anywhere goes to it; the first cut trapped the workspace). On-demand it keeps the
  keyboard until a window is clicked (follow_mouse=2); the keyboard `leave` sets None again. It
  is also given back when a window opens/closes/moves or the workspace changes (hypr.rs), and on
  Escape with nothing selected. Rename is Exclusive only while typing. Keys route in
  `handle_key_event` after the rename branch → `desktop_shortcut`: Ctrl+C/X/V/A, Delete, Enter,
  F2, arrows (`neighbour`), Escape.
  HARDENING (2026-10-08 review): a `.desktop` file is a shortcut only when TRUSTED — executable,
  or an installed app's own (same file name + Exec as an indexed entry, lifted in
  `reload_desktop`); otherwise a plain file under its real name whose double click says so and
  whose menu starts with "Allow to run" (`locked_shortcut`, `Action::AllowRun` = chmod u+x).
  File work for a drop, a paste and Move to Home runs OFF the loop (`desktop_off_loop`); copies
  refuse to go into themselves or round a link (`desktop_send::inside`, `copy_all`), a move takes
  the original only after a successful `sync`. Folder changes are coalesced (`desktop_reload_soon`,
  40 ms). At a reload, rename/menu/press are carried over BY PATH. A texture layer belongs to a
  PICTURE key, not an item index (`Desktop::layer_of`, `Live::layers`).
- `card/` — the CARD: a shelf that rides the windows (the mockup at ~/terminal-mockup/new). ONE
  card with ONE list; it sits inside a window (under the bar, on the right, `WIDTH` wide, 10 in
  from the edges), is turned on per window from the card button at the right end of the window's
  title bar (drawn by the waveview plugin; it sends `card toggle <addr>`, we tell it the state
  with `hl.plugin.waveview.card(addr, on)` — `card_tell_bar`), follows the focus only onto
  windows it is on for and stays on its window otherwise, and unrolls from the top. Four files
  plus drag-and-drop:
  - `model.rs` — the items (`card.json` in the data dir): a text is kept whole; a
    file/folder/picture is kept as its PATH (not copied); a picture that came as pixels is saved
    under `card/` and goes with its item. `facts()` reads what a path is — on a THREAD
    (`card_add_paths`): a folder is counted and a phone's storage is slow. `wrap`, the MIME
    tables, `out_mimes`/`payload` for a drag out.
  - `view.rs` — one frame as a `Scene` + the `Tile`s the pointer is tested against.
  - `place.rs` — where on the window: `Place` (the distance from the NEARER side, so a resting or
    thrown card is still exactly there when the window is resized — not a share of the width),
    `card_rect` (edges on device pixels), `travel` (the capped pace: `TRAVEL_SPEED`/`ACCEL`/
    `BRAKE`), `Swipe` (a run of sliding scroll; `thrown()` = brief, one way, far enough).
  - `dnd.rs` — DROP IN (`card_dnd_enter/drop/received`, routed by `DataDeviceHandler` when the
    drag is over the card's surface): the richest type first — `text/uri-list` (local paths →
    items; none local → the picture's pixels if offered, else the text), one `receive` at a time
    on a thread that gives up on an app silent for `DROP_PATIENCE` (`read_patiently`),
    `finish()`/`destroy()` always sent. DRAG OUT (`card_lift`): a real Wayland drag,
    COPY only, the item stays. It is carried as a PICTURE OF ITSELF (`card_drag_picture`: the
    tile drawn on the CPU — `view::tile_picture` on a `Canvas`, text through
    `Renderer::text_to_pixels` — on an shm surface, `App::drag_picture`), held where grabbed. ORDER: a drag over the card — another app's or one of the card's
    own items — makes the list open a place where it would land (`Card::opening` →
    `view::insert_index`, judged where each item BELONGS; the items from there on ease down by
    `View::shifts`; the carried item is `View::hidden`); a drop of an own item is `Card::move_to`,
    of anything else an insert at that place (`drop_index` → `card_push(…, at)`).
  - `mod.rs` — the state and everything on the loop. PER WINDOW (`Card::wins`, by address →
    `Win { on, place, width }`): all three last until the window closes (`card_window_closed`,
    from `closewindow`) and survive turning the card off and on; `Session` keeps them in the
    runtime dir so a restarted DOCK finds them (`card_restore_session`; addresses are checked
    with `valid_addr` + `window_exists`). AWAY (`Away` bits: `Drag` — the plugin's `card lifted
    <addr>` / `window-placed <addr>`; `Nudge` — the OPTIONS pill's window gestures, from
    `nub_drag.rs`; `Swipe` — the plugin's `card away` / `card back` around a workspace swipe):
    the card is gone at once and back, unrolling, only when NO cause is left (`card_away` /
    `card_back`). It does not follow a moving window. WAITS (`Wait`, one timer for all —
    `card_wait(what, delay)` sets or pushes back a deadline, `card_waited` acts): the wheel
    gesture's end, the pill scroll's end, the throw's judgement, the return after a swipe. THE
    SURFACE: its own `Top` layer covering the output (`surface::create_card_surface`), ordered
    UNDER the dock by a layer rule (`declare_layer_rule`); the card is drawn where
    `hypr::window_spot` says its window is, only its box is in the input region; the renderer is
    built at the first summon and PARKED (shrunk to 8 px) while there is no card anywhere. The
    window's place is re-read on layout events and the plugin's notices; a 2 s poll runs only
    while a card is showing (the safety net, and what ends a drag never reported over).
    SIDEWAYS: a scroll on the title bar (`card slide <addr> <delta>`, every step as it comes) or
    sideways over the card (`card_wheel`: one axis per gesture; while it slides, the whole
    surface takes the pointer) sets the card's `Place`; the card TRAVELS there (`Card::at`). A
    CLICK on an item (press and release without carrying it off) = `card_paste`: the item goes
    on the clipboard (text, or its file) and is pasted into the card's window (Ctrl+V;
    Ctrl+Shift+V for `SHIFT_PASTERS` — not foot, which Golem sets to Ctrl+V; `card paste <n>` does it
    without a pointer). A
    brief scroll that stops is a THROW: outside the window on that side. Edges resize it (`GRIP`,
    `WIDTH_MIN..MAX`). Pictures ride the Files thumbnailer (`card_on_thumb`) into a 32-layer
    array; one pushed out is forgotten and asked for again when next on screen.
  SESSIONS (Max, 2026-10-09): the card's HEAD — its buttons are at the TOP, right under the
  pointer that called it (`view::foot_buttons`, `FOOT_H`; the names say foot from when they were
  there) — holds Pinned · Memory. New is Memory's FIRST ROW (`model::new_row`, id `NEW_ROW`). A SESSION (`model::Past { id, at, name, items }`) is a working set; `Card::memory` is
  ALL of them, newest first, and EACH WINDOW works with one (`Win::session`, until it closes).
  When the card comes to a window (`card_attach`, from `card_summon`) it shows, in this order: the
  session that window was given; else the one a window of the same TITLE was given before
  (`Card::titles`, groomed title → session, kept in `card.json` — so a resumed task finds its card
  another day); else the only one there is; else it opens on MEMORY for one to be picked
  (`Card::session_for`). With no session at all a first one is started. NEW (`card_begin`) starts
  a session for this window, NAMED after its title (`card_host_title` = `task_title::groom`); it is
  in memory at once, even empty, so another window can pick it straight away. MEMORY's page lists a
  row a session (`model::session_row`: name, date · count, its first things; this window's own is
  rimmed); a click gives it to this window and remembers it for the title (`card_recall`), its ×
  forgets it for good (`card_forget` → `Card::lose`). PINNED's page is the pinned items; an item
  is pinned by the PIN beside its × (`Tile::pin`, `Card::pin` — a COPY, so it outlives the session;
  the pin stays lit, a second click unpins). The button of the page that is up is lit; pressing it
  again goes back to the session. `Card::items` is ALWAYS the list on screen — the open session's
  (`Card::open`), the pins, or the rows — taken OUT of `memory`/`pinned` meanwhile (`put_back` /
  `take_out`; `Card::sessions()` is the whole truth for saving), so order, drags, × and
  click-to-paste work the same on every page. A drop on a window with no session starts one
  (`card_push`). An item carries `at` (when) and `from` (the app's class) — for memory and for the
  PHONE BRIDGE to come (an Android panel showing the session and the pins; not built). A picture of
  the card's own is deleted with the last item that shows it (`Card::keeps`). Not built: renaming
  a session by hand.
  THE CARD IS A CHAT WITH YOURSELF BESIDE EVERY WINDOW (Max, 2026-10-09; the approved mockup is
  ~/terminal-mockup/chat — `#demo` plays it, `#chats` shows Memory). What that added, over the
  sessions above: HEAD = Memory · Pinned by name, then THREE ROUND BUTTONS with an icon — clipboard,
  dictionary, emoji (`Foot`, `foot_buttons`). DICTIONARY (`Page::Dictionary`, `card_dict_rows`):
  the card's search is where the word is typed (it has the cursor as the page opens) and each
  language that has the word answers as a row — the clipboard box's own offline data, through
  `App::dict_define` (loaded on first use, `card_dict_loaded` when it is in). EMOJI
  (`Page::Emoji`): the picker as the WHOLE card under the head (`bar::bottom`'s `Page::Emoji` arm),
  and a pick goes straight into the window. Clipboard and Dictionary are LENT pages
  (`Page::lent`): no ×, the pin's place is a `+` = keep it in the memory. An open memory's NAME floats over
  its items (`bar::name_rect`; a click renames it — `Field::Name`); every item carries a small
  line with its time (`View::notes`, `model::when_text`). MEMORY lists the memories as a messenger
  lists chats (`ROW_H`, a coloured initial, the name, the last thing, its time) with NEW at the
  bottom, in the input box's place. CLIPBOARD (`Page::Clipboard`, `card_clip_rows`) is the plain
  clipboard's history as rows (ids `CLIP_IDS + clip.id`; a click pastes, the pin's place is a `+`
  = keep it in the memory, `card_keep_clip`; refreshed by `card_clip_changed` from
  `clipboard.rs`). `bar.rs` is the BOTTOM (`bottom()` lays it out, `draw()` paints it, `Bottom::hit`
  tests it): the SEARCH — a circle with the magnifier that opens into a field (`Field::Seek`,
  `Card::query`, `Card::matches`; a memory is found by its name or anything in it) — and under it
  the INPUT BOX (an open memory only): the paperclip on its left, talk-to-text and record on its
  right, all inside it (no emoji button: emoji are for the WINDOW, from their own page). THE KEYBOARD: the card has none until the input box, the search or the
  name is clicked (`card_take_keys`: an exclusive grab that drops to on-demand the moment the
  keyboard arrives, as the desktop does; while it has it the WHOLE surface takes the pointer and a
  click off the card gives it back — `card_drop_keys` — because the card's window is still the
  compositor's active one: a click on it changes nothing there and the keyboard stayed on the card;
  `KbSurface::Card`; keys route first in `main.rs::handle_key_event` → `card_key`). In the box
  it has a real WRITING CURSOR (`Card::caret`, a place among the draft's characters;
  `model::wrap_spans` breaks the draft into lines WITHOUT dropping a character, so a place in a
  line is a place in the text): the arrows, Home/End, Delete, and a click puts it between the two
  letters nearest the pointer (`card_place_caret`, by measured widths); and a SELECTION
  (`Card::anchor` = its other end: Shift with a move, Ctrl+A, or a press in the words and travel —
  `Press::Select`; typing, Backspace and Delete replace it, Ctrl+C / Ctrl+X copy and cut it), drawn
  as a band per line behind the words (`BarView::select`); the wheel over the box scrolls its lines
  (`Card::draft_first`), and its lines break by the measured width of each letter
  (`model::wrap_spans_by`, `Card::letter_w`); the other fields (search,
  name, the picker's search) still write at their end only.
  ENTER KEEPS what is written in the memory, CTRL+ENTER SENDS it to the window (text only, pasted
  after the keyboard is handed back), Shift+Enter breaks the line, Escape gives the keyboard back.
  The EMOJI picker (a tray over the box: EVERY emoji of `emoji_table::EMOJI` on a grid that
  scrolls under the wheel, with its own search — `Field::Pick`, `Card::repick`, the clipboard
  box's `emoji::emoji_matches`; the ones used lately come first (`Card::recent`, kept in
  `card.json`), and a row of KINDS along its bottom jumps the grid to faces, people, animals,
  food… — `Bottom::kinds`, `Hover::Kind`; emoji ONLY: GIFs and stickers were cut, Max 2026-10-09,
  each chat app sends its own properly and takes a pasted one its own way) puts an emoji where the cursor is: in the
  box while the card has the keyboard, straight into the window otherwise. The PAPERCLIP opens the
  desktop portal's file picker (`bt_files::pick_file_waiting`, on a thread). `voice.rs`: a VOICE
  NOTE is recorded with `pw-record` (16 kHz mono WAV under the card's folder; stopped with SIGINT
  so the file is closed properly) and kept as a `Kind::Voice` item (`aspect` = its seconds; a play
  button, a wave drawn from its id — NOT its real sound — lit as it plays through `pw-play`);
  a MUTED microphone is said before recording (`voice::mic_muted`, `wpctl`) and a recording of
  nothing but silence is said and not kept (`voice::loudest`);
  TALK-TO-TEXT records the same way and runs **whisper.cpp** on it when the talking stops
  (`voice::engine`: `whisper-cli` from `WAVERUNNER_WHISPER` / PATH / `<data>/whisper/engine/bin`,
  the model = `WAVERUNNER_WHISPER_MODEL` / the first `<data>/whisper/ggml-*.bin`), the words land
  in the input box to be fixed before they go anywhere. Not built from the mockup: ONE item made
  of several things (text + picture + voice composed together — each still lands as its own item),
  dragging a single piece out of such an item, GIFs and stickers (no source), typing to search without clicking the magnifier first (the card has no keyboard
  until clicked). Pointer-free: `card write <text>|keep|send|find <words>|rename <name>|clipboard`.
  ZOOM (`card_zoom`, `Card::zoom()`, kept in `card.json`): of the WHOLE card — tabs, items, icons,
  emoji, the input box (Max, 2026-10-09). Everything is laid out as if the card were `1/zoom` its
  size about its top-left corner (`view::virt`) and the finished scene is scaled up from that corner
  (`view::zoom_scene`; the card's own frame, rim and shadow hold). `Card::tiles`, `drop_y` and every
  hit test live in that laid-out space: the pointer is brought into it first (`Card::inside`,
  `Card::outside` back) — so NO layout code knows about the zoom (`View::zoom` is always 1 now;
  `ZOOM_MIN..ZOOM_MAX`, a tenth a step); Ctrl +/− and Ctrl 0 do it ONLY
  while the pointer is on the card. The card never has the keyboard, so those keys are compositor
  BINDS that exist exactly that long (`card_keys`: `hl.unbind` then `hl.bind(key,
  exec_cmd("waverunner-ctl card zoom in|out|reset"))` on the pointer's enter, `hl.unbind` on its
  leave and whenever the card goes away; `ZOOM_KEYS`).
  Pointer-free: `waverunner-ctl card [toggle [addr]|all|add text <…>|add file <path>|remove
  <n>|clear|state|rate <speed> [accel]|new|memory|pinned|session|pin <n>|open <n>|forget <n>|zoom in|out|reset|<n>]` (`open`/`forget` take a row of Memory). Not built: a card button for Seam/staged windows (no
  Golem bar), drawing it in its window's place in the stack (it draws over a window overlapping
  its own), per-output.
  THE CARD, REDESIGNED (Max, 2026-10-10; the design is the mockup `~/terminal-mockup/card/index.html`,
  look = its `?zpills`; being built in stages — what follows is what is IN, and replaces what the
  paragraphs above say where they differ). LOOK: no orange — `Paint::accent()` is the card's own hue,
  stronger and brighter (not linearised: the words are drawn with these values); what sits on the card
  is in two ZEBRA tones of its colour (`Paint::zebra`, `tone`; steps `ZEBRA_1/2`): the buttons the
  lighter, the button of the page that is up the other, the first item the darker, the second the
  lighter. HEAD: round buttons from the left (emoji, pinned, clipboard, dictionary), Memory by name at
  the right; the lit one is an ARROW UP (back to the session); the page's name small under them
  (`Page::title`, `head_h`, `list_rect(rect, Some(page), bottom)`); Memory's page is "Sessions" — a
  row is the name and the date (`ROW_H` 38). TIMES (`model::when_text`): "14:21", "3d ago",
  "8/30 10:30", "9/25/25 12:15"; month first only for en_US/C. THE KEYS FOLLOW THE POINTER
  (`Field::Card`, `card_hover_keys`, `card_rest`, `Card::tabbed`): on the card — only while its window
  is the focused one — it has them with no click; Tab, Escape or a click-paste hand them to the window
  until a click on the card or the pointer leaving and returning; while something is written it keeps
  them. The card is HIDDEN while its window is not the focused one (`Away::Focus`; staying unless
  covered, and not drawing its covered part, were tried and rejected — the real fix is waveview
  drawing it in the window's place in the stack). BOTTOM (`bar::bottom(rect, page, seeking, picking)`):
  one line of hint while it has the keys, the search field in its place once a letter is typed; no
  search button, no New, no input box. WRITING IS IN THE LIST: Enter (or Enter in the search) on a
  session or on Pinned opens the box after the last item (`card_write_new`), an item's PENCIL opens it
  in the item's own place (`Hover::Edit`, `card_edit`, `Card::edit`); `FootView::writing`, the tile of
  id `view::WRITER`, `Bottom::with_input`, `bar::box_h`; Enter keeps, Escape drops; Enter on Sessions =
  a new session. THE DISK (`Hover::Pin` now means it; `GLYPH_DISK`) on every item of every list, its
  menu (`Card::menu`, `menu_rect`, `menu_rows`, `MenuView`, `card_menu`): Pin · Memory · New session —
  always a COPY of its own (`copy_of`, `pin_copy`, `keep_in`); Memory = choose a session on its page
  (`Card::picking`, `card_picked`; title "Keep in which session?"). `card_flash` says what was done
  where the hint is. Pointer-free: `card key enter|escape|tab|backspace`, `card edit <n>`,
  `card disk <n>`, `card menu <row>`. THEN (2026-10-10 night): "Removed · Undo" / "Session forgotten ·
  Undo" in the hint (`Undone`, `card_undo`, `card_undo_over`, `card_flash_long`); a session is
  forgotten at the SECOND click of its × (`Card::arming`), renamed by the pencil of its row
  (`Card::renaming`); a session with nothing in it is gone once left (`Card::prune`); HOLD SPACE = a
  voice note for as long as it is held (`Card::space/held`, `card_key_release`, `HOLD_MS`), talk on
  voice-to-text's page; a dashed frame on an empty session / Pinned; "Nothing to search" on an empty
  list; a × on the clipboard's rows (deletes the clip). AN ITEM CAN BE MADE OF PARTS
  (`Item::parts`, each an item; `body` = their words): laid out and drawn by `view::pieces`,
  `mixed_height`, `draw_pieces`, `piece_rects` (files as 64 px thumbnails side by side, a voice note
  as a row, a part with `from` under a rule with that name = an ANSWER); the writing box holds the
  parts already put in (`Card::sealed`, `card_seal`, `card_unseal`) over the line being written, and
  has ONE button, the +, whose menu (`Card::plus`, `card_plus`) is Image · File · Voice note · Voice
  to text; while `Field::Box` is up whatever `card_push` is given becomes a part (`Card::keeping`
  guards the keep itself); Backspace on an empty line takes the last part back. A CLICK on an item
  made of parts OPENS it (`Card::wide`, `card_click`): its voice notes are written out
  (`card_write_out` → `Item::said`), and the piece under the pointer answers alone — `Card::item` /
  `item_mut` look into parts, so paste, lift and play take a part's id. A long text shows
  `MAX_LINES` (6) and "Show all" (`has_more`, `Tile::more`, `Hover::More`). THE VOICE NOTE: play
  symbol, its title (`body`, unless it starts with `VOICE_NAME`), a line with its two times,
  "Transcribe" (`Hover::More` on it); its pencil writes the title, one recorded with Space asks for
  it (`Card::name_next`). VOICE TO TEXT has its own page (`Page::Talk`, `Foot::Talk` first in the
  head, `talk_mic`, `Hover::Mic`, `card_said`): what is said goes into the WINDOW and stays as the
  page's one item (`Card::said_last`). THE DICTIONARY works as Pinned (`Card::looked`, saved;
  `card_look_up`, `card_keep_word`): a word is written in the box (no + there: `Bottom::plain`) and
  kept as an item whose parts are the word and one ANSWER a language. Pointer-free too: `card undo`,
  `card key space|space-up`, `card plus`, `card click <n>`, `card talk`, `card said <words>`,
  `card dictionary <word>`. ⚠ never `rustfmt crates/daemon/src/main.rs` (it reformats the whole
  crate): format `card/*.rs` only. THEN (2026-10-11): Memory's search lists the THINGS found under
  their session's row, then "Pinned" (`Card::finds/find_rows`, `search_all`, `is_row`, `PINNED_ROW`,
  `FootView::rows`); a click on a voice note's line plays from there (`Hover::Line`, `Tile::line`,
  `card_play_from`, `voice::tail_wav` — the player is given a copy that begins there); an item made
  of parts is carried as its first words. THE TRANSLATOR (`Page::Translate`, `Foot::Translate`
  before the dictionary's, `FEET` 7, `Card::translated`, saved): the Pinned flow, what is written
  kept with its translation as an ANSWER named by the two languages; told from its words
  (`language_of`), else the pair picked from the menu a click on that rule opens (`Hover::Lang`,
  `Card::lang`, `lang_strip`, `LANG_ROWS`, `card_lang`; kept in the item's `from` as "es>en",
  `pair_of`); done by `translate()` on a thread — `translateLocally -m <from>-<to>-tiny`, the
  words on its input, through English when neither side is English — the engine being the flake's
  `translate` package (pkgs.translatelocally + the es/fr/de↔en tiny models), out-linked by
  `waverunner-dev` at `~/.local/state/waverunner/translate` and named by `WAVERUNNER_TRANSLATE`.
  Pointer-free: `card translate <words>`, `card lang <n> <row>`. The translator knows six languages (en es fr de pt it; pt/it are
  Mozilla's Firefox tiny models, the flake's `mozModel`). EVERYTHING on the card scrolls the same way
  (`card_scroll_by`: fingers 1:1, a wheel's notch eased, a glide when let go fast) — the list and the emoji
  grid (`Card::scroll_pick` says which); what is written in the box scrolls by the pixel
  (`Card::draft_off`, `BarView.first_off`). A VOICE NOTE (2026-10-11): its title is written IN the note (dimmed "Type a title",
  no box); its button is play/PAUSE (`Card::paused`), a click on its line plays from there when playing
  and pauses there when not; "x1" at its foot goes round `voice::SPEEDS` (x1, x1.5, x0.75 — for every
  note; `voice::stretch` = WSOLA, the pitch kept; `Playing::at()` is the place in the note). THE CARD'S PILL ON THE BAR (2026-10-12, a first cut "to see
  how it feels"): the clipboard pill's place and machinery (`PillId::Clipboard`/`ClipboardBox`, `clip.peek_t`)
  with the card's symbol (`GLYPH_CARD`); a CLICK turns the card on/off for the focused window
  (`card_pill_click`), HOVER shows the long pill with the session's last thing (`card_pill_words`), a SCROLL
  DOWN hangs the card itself under the bar (`Card::banner`, `card_banner`, a fake window spot in `sync_card`)
  and a scroll up or leaving both folds it (`card_banner_check`). The old clipboard box is still in the
  code (its shortcut opens it); `clip_paste` is unused. Test verbs: `card banner open|close|click|words`.
  Icons of the head: each at its own measured size and nudge (`Icon`, `card icons <f> <dx> <dy>`,
  tools/card-icons/measure.py). The whole mockup is in; what it
  has that this has not: Portuguese and Italian (no packaged model), a download on a click.
  LAST (2026-10-11): a drag held over a button of the head turns the card to that page
  (`Card::dnd_foot`), and a drop does what its page does (`card_push`: translated, looked up, kept in
  the session whose row it was let go on — `Card::drop_session`; an item of the card's own lands on
  another page as a copy, `Drag::item`); the pencil is on every item of one's own (a file opens in the
  box as a part); THE EMOJI'S PAGE has its kinds along the top (the one in view marked:
  `BarView::kind_now`, `Bottom::first_in_view`), the grid under them, and at the bottom a line of hint
  where the search shows once something is typed. Not done: a drop on the clipboard's page.
- `quicklook.rs` — QUICK LOOK (Max, 2026-10-16, the lab https://claude.ai/artifact/MGRESgAmUyjC6rq1GL3ytn):
  Space in a file window shows the thing the keys are on, big, over everything, GROWING OUT OF
  ITS ICON (`peek_show`; the icon's place = `filer_peek_thing`: the cell, the window's corner from
  `hypr::active_window_geom`); arrows walk with it up (`filer_peek_follow`), Space/Escape shrink
  it back, Enter opens with the app, a click off the frame closes. Drawn on the desktop's TOP
  surface (`desktop_top.rs`, Overlay, whole output; no top surface = no Quick Look) — the
  keyboard stays with the file window (`peek_key` first in `filer_key`). One frame for every
  kind: head (badge, name, "KIND · size · w × h", "Open with <app>" from `xdg-mime`, ×) and a
  body: a PICTURE (image crate, SVG via resvg, RAW by its embedded JPEG) decoded at the size it
  is shown and CUT IN TILES of the icon array (`cut`: 254 texels + 1 repeated round each, at most
  `TILES`), drawn clipped to the frame's corner by `icon.wgsl`'s clipped-picture mode (ring in
  (-1.65,-1.55), clip rect in `tint`, corner in `plate.x`); TEXT (its first lines, mono, wheel /
  PgUp/PgDn); else INFO (the window's picture + kind, size, modified, where). FAST: the window's
  own thumbnail blown up at once (SOFT layer), the sharp body fades in; what the keys are on and
  its neighbours are PREPARED in the background as they move (`filer_peek_near` from
  `filer_draw` → `peek_want`: one nice-10 thread, newest first, stale dropped) within `BUDGET`.
  A JPEG is decoded STRAIGHT SMALL (`peek_kinds::jpeg_scaled`,
  the `jpeg-decoder` crate's IDCT scaling to 1/2–1/8, no default features): a realistic 24 MP photo
  250 → 110 ms on the dev box (a very noisy one only 1.5×: entropy decoding is most of it). Every
  picture is TURNED AS ITS CAMERA SAYS (EXIF orientation: `open_oriented`), in Quick Look, the
  editor (what is saved is upright) and the Files thumbnails (kept previews salted "o1" so the
  old sideways ones are made again). PHASE 2 (`peek_kinds.rs`, what reads each kind, all on the worker): a PDF a PAGE at a time
  (`mutool draw`, else `pdftoppm`; PgUp/PgDn or the wheel turn it, the next page prepared ahead,
  "2 / 3" pill in a grid of its own over the page — a grid's fills draw under its pictures; the
  cache is keyed by (path, page)); a VIDEO = a frame a tenth in + length/size (`ffprobe`/`ffmpeg`);
  a SONG = cover (tiles), title, artist · album, loudness bars, play/pause (`mpv`, P or the button;
  stopped when it closes or moves on — `Up`'s drop); a FONT = a sheet in itself (glyphon,
  `thumb_more::draw_text_in`); an ARCHIVE = its list (`bsdtar -tvf`); anything with a freedesktop
  thumbnailer (Office through libgsf, epub…) = its picture asked big (`by_thumbnailer_at`). Golem's
  home.nix brings mutool (mupdf-headless) and bsdtar (libarchive). `xdg-mime` is asked once a kind
  (it is a slow shell script). A MOVING PICTURE (animated GIF or WebP — `ffmpeg` cannot read
  an animated WebP) is read whole in-process (`peek_kinds::animation`: `image`'s
  AnimationDecoder, frames brought to the screen's size and, all together, within 48 MB) and
  played round on the loop's clock through the renderer's video texture over its first frame
  (`Up::anim_ms`), as Finder plays them: no bar; no pencil (saved, it would stand still). A PHOTO ZOOMS (`peek_zoom.rs`, as Finder's): Ctrl+wheel about
  the pointer, + / −, a double click (fitted ↔ its own pixels), 0 fits; zoomed, drag or wheel to
  move; drawn in a grid scissored to the body. Once it stands still 160 ms the part on screen is
  read again at the screen's size off the loop (a JPEG decoded straight at that size) into its
  OWN layers (`DETAIL` = TILES.., so the array is 2×TILES+2) and laid over; its rev is the
  request's unique TAG (a counter per photo let one photo show another's part). A PDF's PAGE zooms
  too (as close as 8×; a double click 2.5×): its sharp part is drawn by `pdftoppm`'s crop (`-r dpi
  -x -y -W -H`: only what is on screen), and turning the page fits it again. The wheel is the
  zoom's only with Ctrl or once zoomed (else it turns a PDF's pages, as before). Rig verbs:
  `files key space|escape|left|right|pgdown|plus|minus|zero`, `debug-desktop "peek pan dx dy|dbl x y"`. PHASE 3: a VIDEO
  PLAYS as it is shown (Finder's way): `peek_kinds::stream` — `ffmpeg -re` hands RGBA frames at
  most 1280×720 / 30 a second through a reader thread (tagged, an empty frame = the end), the
  sound is `mpv`'s; the frames go into the renderer's VIDEO TEXTURE (`Renderer::set_video_frame`,
  one layer, no mips, the icon pipeline with its own bind group; `Scene::video` = drawn after its
  grid, under the bar's grid), the poster under it. Its bar: play/pause, the line (a click seeks),
  the time; P too. The damage repaints the video's rect at EVERY frame (its texture is written
  behind the damage's back: a paused one showed an old swapchain buffer). No `-hwaccel auto`: it
  cost 2 s to the first frame under the dock's environment (100 ms without). A BIG video (over
  1920×1080) is decoded, scaled AND turned RGBA on the graphics chip (VA-API: `scale_vaapi=…:
  format=rgba,hwdownload`) — 4K→720p, 8 s of it: 6.3 s of processor by software, ~1.8 s this way;
  the render node that can is found ONCE a session (`gpu_node`: one frame tried on each, thrown to
  `-f null` — a frame left in an unread pipe hung the try), and a chip that gives no first frame
  falls back to the processor in the same thread (`Stream` holds whichever decoder runs). A FOLDER lists what
  is in it; Quick Look works from the DESKTOP's icons too (`quicklook::DESKTOP` as its window
  id; `debug-desktop "key space|…"`). THE PENCIL (`peek_edit.rs` = what is done, `peek_editor.rs` =
  the frame's side): on a picture the head gets a pencil; taken, the head is Cancel · Save copy ·
  Save and a toolbar comes up (Crop · Draw · Text · Blur · Tint · Turn, six inks, Undo; the tint's
  panel has Brightness/Contrast/Warmth sliders + Black and white). An edit is a list of `Op`s in
  SHARES of the picture: the same list draws the screen-size copy (read again whole, off the
  loop) and, on save, the whole picture off the loop. A line drawn live re-cuts only the tiles it
  crosses. Save writes over (the original kept in `~/.local/share/golem/versions/<secs>-name`),
  Save copy writes "name (edited).ext" beside it (a RAW is only ever copied, as JPEG); Cancel
  with changes asks once ("Discard changes?"); Ctrl+S / Ctrl+Shift+S / Ctrl+Z / Escape. Rig:
  `debug-desktop "peek edit|tool <t>|drag x0 y0 x1 y1|click x y|type <w>|slide i k|turn|undo|
  apply|save|copy|cancel|state"` (the drag goes through the real pointer code). TEXT read WHOLE
  (`Prepared::whole_text`: small, no line cut) gets the pencil too (`peek_text.rs`): written
  straight in the frame — caret (measured in the font as drawn, `TextEdit::caret_x`), arrows,
  Home/End, PgUp/PgDn, Enter keeps the indent, Tab, Backspace/Delete; Ctrl+S over (original kept
  in versions), Ctrl+Shift+S beside; tabs, `\r\n` and the last newline come back as they were
  read. A saved file's pictures in the file windows and on the desktop are let go and made anew
  (`App::thumb_stale`). A VIDEO or a SONG is
  CUT (`peek_trim.rs`): two handles on a line as long as it lasts, "Take the sound away" for a
  video; saved with `ffmpeg -ss a -to b -c copy` (no re-encoding: quick anywhere, the cut on the
  keyframe before), over (original kept) or beside. A SONG's switch is "Fade in and out" instead (2 s in, 3 s out, a third of the cut at most; only the sound is encoded again, `-c:v copy` keeps the cover, quality by ending — `audio_quality`). The CAMERA on a video's bar keeps the moment on screen as a JPEG beside it (`peek_snap`, "name (frame 0-02).jpg"; playing, it pauses there). Rig: `peek cut <a> <b>|mute|save|copy|cancel` (`mute` = the switch: a song's fades), `peek snap`. SEVERAL SELECTED (Finder's way): Space shows the one the keys are on, the head says "2 of 5 · …" (`Thing::of`), and the arrows walk among the selected only, round, the selection kept (`filer_peek_walk`); on the DESKTOP the same, in the order of the cells (`desktop_picked`, `Desktop::peek_on`). The graphics chip is tried by ONE video at a time (a second meanwhile goes on the processor). CODE IN COLOUR (`peek_syntax.rs`): a small tokenizer of our own (comments, strings, numbers, keywords, capitalised types) for ~20 languages by ending, run on the worker (`Prepared::syntax`); drawn as a label a piece in the palette's hues (base04 comments lifted toward the ink, 0B strings, 09 numbers, 0E keywords, 0A types), each piece placed where the renderer MEASURED it to begin (`peek_code_measure` in `peek_upload`, `QuickLook::code_x`, once a line) — the Nerd family is not monospaced everywhere; keyed by the line's words, so the TEXT EDITOR shares it: what is written is coloured again as it changes (`TextEdit::recolor`, by a hash of the lines). A PHONE'S HEIC (and what else `image` cannot read but FFMPEG can) is decoded by FFMPEG straight at the shown size through a pipe (`peek_kinds::ffmpeg_still`, its own size from `probe`); the pencil takes it too and saves a JPEG copy (`whole_picture`'s fallback). JPEG XL: libjxl's `djxl` (Golem's home.nix brings libjxl, and with it the `jxl.thumbnailer` Files uses), whole as 8-bit PAM through a pipe (`peek_kinds::jxl_picture`, `pam_rgba`), then brought to size; the pencil takes it too (JPEG copy). A WEB PAGE (.html .htm .xhtml, `peek_web.rs`, Max 2026-10-17: "seen as a website, not text") is shown as Seam draws it: Seam's Firefox HEADLESS (`--screenshot` with a WIDTH only = the page WHOLE, as tall as it is, up to 16 000 px; Max 2026-10-17: "la página completa scrolleable") in Quick Look's own profile (`~/.cache/waverunner/web-profile`, DNS off + a dead proxy: a page can call nobody; scripts run), the page kept RAW in `~/.cache/waverunner/web/` (`<hash>.rgba`, 8 bytes of size then rows; the newest 40 kept) by path+size+mtime; Quick Look holds a RUN of it (`Long`: `CROP` 2500 px starting every `STEP` 500 — within the 64 tiles), the run `k` being the cache key's page; the wheel/PgUp/PgDn move the page under the frame (`peek_scroll`, `up.scroll`) and ask for the run under it and its neighbours (a read of the raw file, ~35 ms); the old run stays where it stands until the new one is up; a thin bar shows where; no zoom on a long page (≈1.8 s the first look on the dev box, then a read); its pencil writes the SOURCE (`written_whole` → the text editor, with its colours); not drawn → text as before. An SVG's pencil writes its text too. JSON on one long line is shown LAID OUT (serde_json pretty), with no pencil (it is not the file as it is). MARKDOWN is shown FORMATTED and a TABLE OF VALUES (.csv/.tsv; comma, semicolon or tab, quotes and line ends in quotes) AS A TABLE, the same way (`peek_web::drawn`): made into a page (pulldown-cmark; our own `parse_rows`, the first 300 rows, numbers right-aligned) in the palette's colours (`style()`, part of the cache key — another palette draws it anew), with `<base>` at the document's folder for its pictures, then drawn WHOLE by the same headless Seam (a document 900 wide, a table 1280) and scrolled the same way; the pencil writes its text.
  A PDF's page is MARKED (`peek_pdf.rs`): Highlight · Draw · Text · Sign on the page drawn at
  the screen's size; saved as the PDF's own ANNOTATIONS (Highlight, Ink, FreeText) by `mutool run`
  with a generated script — the page itself is never changed. The signature is drawn once in a
  box (3 : 1) and kept (`~/.local/share/golem/signature.json`); "New signature" draws another.
  Rig: `peek tool mark|sign`, `padline x0 y0 x1 y1`, `paduse`, `click x y`.
- `filer.rs` — FILES, Golem's file manager (Max, 2026-10-13: a REAL WINDOW, icons as on the
  desktop, Nautilus stays the system's until this one covers the daily things). Each window is an
  `xdg_toplevel` OF THE DAEMON (class `golem-files`, `WindowHandler` + `delegate_xdg_*` at the
  file's foot) with its own renderer and icon array (`Win`; `Filer::take`/`put` lends one out to
  work on with the rest of the app in hand). Routing by id: `fractional::SurfaceKind::Files(id)`,
  `options::PointerSurface::Files(id)`, `Filer::keys` (the window the keyboard is on —
  `handle_key_event` asks it first), `CompositorHandler::frame` → `filer_frame`. ONE folder a
  window: the desktop's cell with TWO lines of name (`two_lines`; see MIDDLE CUT below), row by row (`Layout`: `cell`,
  `index_at`, `in_view`, `reveal`), scrolling THE CARD'S WAY (Max, 2026-10-14; `filer_scroll` / `filer_fling`, the card's own numbers: a step ×2.4, fingers followed step for step, a wheel's notch eased at 24, a fast scroll let go of gliding on at 4.5 for its speed × 0.32), only what is
  on screen laid out and holding a texture layer (`filer_sync_icons`; thumbnails ride the Files
  thumbnailer — `filer_on_thumb` — at most `CHAINS_MAX` kept). A NAME THAT DOES NOT FIT IS CUT IN THE MIDDLE (Max, 2026-10-22: "i want the … in the middle as macOS
  does it, like: dhadjk…kjhasoj.mp4"): `desktop::fit_name` keeps the start and the end, the end just
  under half of the room (`END_SHARE`) and never less than the name's ending (`desktop::ending`: not a
  dotfile's dot) with three characters before it while the room holds that much, one "…", spaces at
  the cut trimmed, measured by the renderer's shaping (`measure_text` takes `&self`, so a draw holding
  `&self` — Quick Look's — measures too). Past TWO lines in the grid (and on the desktop) the first
  line is as much of the start as it holds and the second "…" and the end (`two_lines`,
  `desktop::fit_end`). Everywhere a file's or a folder's name shows: the grid, list rows (a thing
  found below: "folder/name"), the pill's “name” (cut inside its quotes), the i card's title, the
  sidebar's places (fitted to the sidebar open), the tabs' chips, the path line's last step when it
  stands alone and is still too long, Quick Look's title, the Properties box's title
  (`Props::fit_title`), the desktop, and the dock's file tiles — its Files row and the lone app's
  recents (`content::tile_label`, by the estimate `truncate_label` uses; a web page's title is not a
  name). Words that are not names (menus, rows of the i card, an app's actions —
  `two_lines_of_words`) keep `fit_label`'s cut at the end; the rename field shows the whole name.
  THE CHROME — "A · Unified" (Max, 2026-10-21: "is
  perfect! build it to the real golem files"; mockup https://claude.ai/artifact/DudbvDzykY1oVnMnmn7dvW,
  `filer/chrome.rs`; it REPLACED the floating pills of 2026-10-17): a standard file manager's frame.
  THE SIDEBAR on the page's own colour, one hairline at its right, no shadow, no light (`content::UNLIT`
  — the dark look's night light crushed a near-black panel to black): 48 px of carved SYMBOLS at rest;
  the pointer on it opens it to 200 OVER the files after `SIDE_WAIT` (120 ms; `Win::side_wait`,
  `filer_side_wait`), names and the section headings fading in, leaving closes it; the PIN (11 px, top
  right, only while open; white when on, a dark hairline edge in the light) keeps it open BESIDE the
  files (`settings.files_pinned`, `filer_pin`, `Win::field_left`). At its top the two MODES (Files ·
  Media): at rest only the one in use, open both as halves, the one in use sunken; MEDIA shows only
  pictures and videos (`Win::media`, `is_media`, in `show_found`). Then `..` (Max: "add the '..' place
  too" — `with_up`, the first row over Favourites' heading: the folder this one is in; a click goes up
  leaving this folder selected, a drop on it moves things up, held there with a drag it climbs —
  `up_hold`; it climbs past Home to `/`; it is ALWAYS in its spot so nothing under it jumps — where
  there is nowhere up (`/`, the bin's top, Recents) DIMMED to a disabled row's ink (`NOWHERE_INK`) and
  inert: no band, no click, no drop (`Place::nowhere`, a `..` with no folder; Max, 2026-10-22: "hacé que
  '..' se quede hasta en / pero dimmed", reversing 10-14's "hidden, not grey"; Media is the same folder
  filtered, so its `..` goes up as Files' does), FAVOURITES
  (RECENTS — below —, Home under the owner's name, their folders, `~/Phones`, the bookmarks) and
  LOCATIONS (GOLEM DISK = `/`, other disks, what is plugged in, the Bin) — `places()`, `Place::gap` =
  Locations' first. FILES IS UNLOCKED (Max, 2026-10-22: "the lock we did to the file manager makes no
  sense, golem is for every user, i just needed to go to /tmp … add a confirmation for changes made on
  specific directories"; it REPLACED the 2026-10-15 floor, `floor`/`Win::root`/`filer_roots`, gone):
  every folder can be shown — `..`, a typed path, Golem Disk, a folder asked for from outside
  (FileManager1, xdg-open, ctl) opens as any other; the way here starts at the owner's name inside
  Home, at "Golem Disk" outside it ("Golem Disk › tmp"). What a CHANGE may do depends on where — THREE
  ZONES (`filer/zones.rs`: `zone_of` a thing changed, `zone_in` a folder changed inside; remembered
  3 s; `Win::zone` is its folder's): YOURS (Home and below but the delicate places, your sticks,
  phones and disks, /tmp, /var/tmp — no question, Undo covers it; /tmp and /var/tmp wear a quiet chip
  "Emptied at restart" at the end of the path line — words at the ink, three quarters, on a fill a step
  stronger than a field's, `tones.chip_on`); DELICATE (`delicate_at`: ~/Golem and all in it,
  Home's top dot entries and all in them, the owner's folders THEMSELVES — not what is in them —,
  ~/Phones and all in it) — a change asks first, `filer/confirm.rs`: a panel in the menus' paint,
  centred over the window, ~360 wide, a shield, the act named in the title ("Move “.config” to the
  bin?", "Rename “Pictures”?", "Move 3 items inside “Golem” to the bin?", "Change files inside
  “Golem”?" — `confirm_title`, one panel an action), ONE sentence for the place (`Why::sentence`:
  Golem, settings, keys, the browser — `.mozilla` and Seam's `.local/share/seam` —, the main
  folders, Phones, else "It's part of how Golem works."), [Cancel] [<act> anyway] (`Act::anyway`);
  Cancel has the focus, Tab/←/→ move it, Enter/Space press it, Escape or a press off the panel
  cancels; the buttons are a menu's rows (Max: no rings, no hairline round the panel): the focused or
  pointed one tinted as a hovered row (`MenuPaint::row_tint`), the act's words ALWAYS the menus' red
  and its tint the red row's (`MenuPaint::red_tint`), Cancel never red; THE SYSTEM'S (`system_tree`: `/`, /etc /nix /boot /usr /proc /sys
  /dev /root /srv /opt /bin /sbin /lib*, /var but /var/tmp, /run but the owner's media and gvfs;
  and whatever the owner cannot write or does not own) — looked at, copied, never changed: no row
  that changes files (`Action::changes`, `Ctx::readonly`; Set as wallpaper stays), no Move/Rename/
  New folder/bin on the pill, `put_here` false, no room made under a drag, a drop or paste INTO it
  refused with a quiet flash "This is the system's. Golem manages it." (`filer_refuse`); a dim chip
  with a lock, "System"; NO "open as administrator" (a superuser has the terminal). THE GATE
  (`filer_gate`, Act + the things changed + the folder changed inside → go, refuse, or ask with the
  change kept as a `Pending` and done again on "anyway" — `filer_do`, the gate opened once by
  `filer_confirmed`) stands in every way a change is made: `filer_bin`, `filer_begin_rename`,
  `filer_paste_into`, `filer_bring` (drops; it ends the drag at once whatever is asked; what is the
  system's is COPIED from, never moved), `filer_new_folder`, `filer_duplicate`, `filer_group`,
  `filer_archive`, Rotate; Cut of the system's is refused. A just-made folder's name is not asked
  about again. `trash::protected` (the hard rule every way of throwing away meets) is now ONLY `/`,
  Home and above it, the bin, a volume's own folder and the system's; the dock's bin cannot show the
  question, so a delicate thing dropped there stays, quietly (`trash_file`) — a file window's drag
  on it asks in that window (`filer_bin`) — and the desktop COPIES what is not the owner's to change
  (`desktop_dnd_received`). Rig (never Max's real Home): `RIG_HOME=<a fake home>` for the rig dock,
  `files go /etc`, `files key tab|enter|escape|delete`, `files rename <n>`, `files bin`. RECENTS
  (`filer/recent.rs`, Max 2026-10-21): a place of no folder (`recent::root()` = `recent:///`)
  listing the files of the freedesktop `recently-used.xbel` that are still there, newest
  first, at most 100 (`recent::list`, read by `filer_list` like the bin's records; not watched — read
  again when Files opens a file); every file Files opens (double click, Enter, Open with) is NOTED there
  as GTK does (`filer_used` → `recent::note`, off the loop: the entry's times moved, or a new
  `<bookmark>` with its kind and Golem Files as the app). Nothing is made, pasted or dropped into it
  (`filer_act`, `filer_bring`, `filer_new_folder`, `filer_paste`; its ground has no menu); Delete
  throws the REAL file away; the path line says "Recents"; rig `files go recents`. Symbols are CARVED
  (`chrome::tones`): light = the ground pressed in (`carve`: 42 %, the current place a 16 % sunken square
  with its symbol at 55 %); dark = light greys toward the ink (the look's own). THE TOP LINE (32 px,
  `HEAD_H`, page colour, no line under it): the TABS (`Win::tabs`, `chrome::Tab`: each its folder and
  its own back/forward; the current one's live in `dir`/`back`/`forward` — the title bar's arrows act on
  it): the current one is the way here (`crumbs`; the first step the home's name with no icon, the
  others with a folder), ✎ and × at its end on hover; the others are dimmed CHIPS (× on hover, middle
  click closes); + opens Home right of the current (`filer_tab_new/take/close`, Ctrl+T, Ctrl+W closes a
  tab while there are others, Ctrl+Tab); widths EASE (`Win::tab_w`) so a tab folds into its chip. ✎
  (or Ctrl+L, or `/`/`~` typed) turns the CURRENT tab's path into a field its width (`Win::go_to`,
  `edit_w`): Enter goes (`filer_go_typed`, a file → its folder with it selected; nothing there → red),
  Escape, ✎ again or a press anywhere off the tab closes it. At its right end: sort (`filer_sort_menu`:
  Name · Date Modified · Size · Kind, Ascending · Descending — `Action::SortBy/SortDir`, `SORT_MENU`;
  `SortKey::Kind`; Name descending turns the folder's own order round) · i · grid | list · the EYE ·
  search (a 170 px field slides out between the eye and the magnifier, `Win::find_t`; while it is out,
  with words or none, the magnifier is an × in the pressed look — "Cancel search", Max 2026-10-22 — and
  a click on it puts the search away, its words with it, the folder whole again: `filer_find(id, None)`,
  what Escape in the field does). THE EYE (Max,
  2026-10-22: "agrega a los botones 'see hidden files' a la izquierda de buscar"; `Spot::Hidden`,
  `Hits::hidden`) is the ground menu's "Show / Hide hidden files" — one state, `Win::hidden`, the same
  `Action::ToggleHidden` (and Ctrl+H) — its symbol the state: the slashed eye (`eyeoff`) while hidden
  files are not shown, the eye PRESSED while they are ("el icono debería cambiar"); the tabs and the
  way here give way to it as to the other tools. Rig: `files where hidden`. KEYBOARD MODE (Max,
  2026-10-22: "… y activa 'keyboard mode'"; its top-line button was taken out the same day): CTRL+K
  turns it on and off anywhere in a file window, a field that has the keys too (first in `filer_key`);
  one choice for every window, remembered (`settings.files_keys`, `filer_keys`). It shows in the
  SELECTION PILL: its keyboard glyph at the pill's LEFT end in every state (`PillView::keys`; a click on
  it is the shortcuts table, `PillAct::Keys`), and with nothing else to say the pill stays as the glyph,
  "Keyboard driven" and a × that turns the mode off ("agregale la x a la píldora por si un usuario lo
  activa sin querer": `PillAct::KeysOff`, "Turn off keyboard mode (Ctrl+K)") — two clicks always get
  out; off, an empty pill hides as ever. While on,
  plain letters (no Ctrl/Alt/Super) are commands in `filer_key` — W A S D walk as the arrows (Shift
  stretches), Q back, E opens (Enter), Z undoes, X / C / V cut, copy, paste (the menu's own acts, so the
  pill's flow and the zones' gate), R names the one selected; Delete as ever; and THE EIGHT MORE (Max,
  2026-10-22: "constrúilos todos"): U up (`..`; nothing at `/`), N a new folder with its name typed (the
  zones ask or refuse as ever), F the search, G the first thing and Shift+G the last (plain moves, not
  visual mode), I the i card, H hidden files (the eye's own state), L grid ↔ list, T a tab on this folder
  and Shift+T closes it (nothing on the last); AND MORE: `/` Golem Disk (it no longer opens the path
  field while the mode is on — ✎ and Ctrl+L still do, mode or not), `.` Home, 1…9 the steps of the path
  line AS SHOWN (`Hits::crumbs`: inside Home 1 = the owner's name; outside, 1 = Golem Disk; past the last
  does nothing), ← → the tabs round (↑ ↓ walk as ever), ` THE SIDEBAR CYCLE (Max, 2026-10-22: "add ` to
  jump over the items on the sidebar, so i press ````` when i stop 0.5s it opens the selected";
  `filer_cycle`, `Win::cycle`): the first press opens the rested sidebar at once and lights the place
  after the folder shown (or after the one lit), each press the next round the places (never `..`;
  Shift+` — `~` — the one before), in a menu row's quiet tint; `CYCLE_REST` (0.5 s) after the last press
  the place lit opens in this tab and the sidebar folds unless pinned (`filer_cycle_end`, only the newest
  press's wait counts); Escape ends it with nothing opened; a press, or the pointer coming onto the
  sidebar, takes over as ever (the sidebar is held open meanwhile — `side_follow`), `?` THE SHORTCUTS TABLE (`filer/shortcuts.rs`,
  `filer_shortcuts`; also a click on the pill's glyph; Ctrl+K in its "Always"): the confirmation's
  material centred over the window, ~520 wide, two columns cut where they balance, a key cap and the
  words a row under "Keyboard mode" and "Always"; `?`, Escape or any press puts it away, and nothing else
  answers meanwhile. The letters and their words are `chrome::KEYS` (Spanish
  in es.json; said in the log as the mode turns on — the button's words once the top line's buttons are
  named: `keys_words`; Ctrl+K among them). No other letter types and nothing searches by itself (Ctrl+F and the magnifier
  still do). A field that is up (search, path,
  name, a menu's search) and a menu take their keys first, as before; Alt+W/A/S/D/Q/E still work with it
  off. Quiet: the pill's glyph is the only sign. Rig: `files keymode on|off`, `files key ctrl k`, `files pill <n>`,
  `files key w|a|s|d|e|q|z|x|c|v|r|u|n|f|g|G|i|h|l|t|T|slash|period|question|grave|tilde|1|2|3|9|left|right`,
  `files key ctrl l` (the state names the cursor's thing, the search,
  the i card, hidden, list, the tabs). Buttons 22×20, 12 px
  symbols — the mockup's own strokes written as SVGs into `~/.cache/waverunner/files-glyphs/` once
  (`chrome::write_glyphs`, `glyph`) and read by the icon loader by path, washed with their colour
  (`symbol`: icon.wgsl's ghost, ring -1.17). THE i CARD (`Win::info_on`, `chrome::Info`, read off the
  loop by `filer_info_due` whenever the selection changes — `info_key`): preview, name, kind, Contains or
  Size (+ bytes), Modified, Where, Opens with (`quicklook::default_app` → the dock's index); its foot
  "N items, X available" / "n of N selected, X available". Solid (no blur to frost it); the words under it
  left out by the width each is drawn at (`chrome::leave_out`, as under the menu, the boxes, the
  history and the pill — not by the row: a list once lost every name beside the card, 2026-10-22),
  and a list's names beside it end before it, cut as names are. SELECTION AND CLICKS, the mockup's (Max, 2026-10-21, "SELECTED: one soft box"): a selected
  thing in the grid wears ONE soft box over its picture AND its name (`SEL_TOP` 6 over what the
  picture shows, `SEL_PAD` 4 at the sides and under; radius 9; black 7.5 % / white 10 %, `pic_mark`,
  UNLIT), hugging that thing — a one-line name's box is not stretched — and its name in its own box of
  the same soft colour laid again (`name_mark`, `NAME_PAD`, radius 4); a selected row is the soft
  colour across — a SQUARE band, edge to edge, over its stripe (`band_on_stripe`) — with its name in
  the darker box, the words as ever; NO accent but in a name being
  typed. Where two selected things stand one above the other the lower box begins under the upper's
  (the rows are close). PICTURES SIT ON THE BOTTOM of their space and the name is ~3 px under what
  they show (`LABEL_GAP` 4 with the name box's pixel): each picture's empty rows above and under are
  read once from its pixels (`ink_box`, `Win::ink`: rows AND columns) and it is moved down by those
  under. A THING IS ITS PICTURE AND ITS NAME (Max, 2026-10-22: "selection have to happen only clicking
  the icon or text, the rest should be empty space, so i can right click on the bg (dir) and use the
  rubber band"): in the grid only what the picture shows (`ink_box`) and the name's own box (`NAME_PAD`),
  ±2 px (`THING_SLOP`) with the strip between them, are the thing (`Thing`, `thing_in`, `Win::thing`,
  `Win::thing_at` — the cell under the pointer, the one above it and either side, for a second line or
  a wide name). The rest of a cell is the folder's ground: a press there draws a band, a right click
  clears the selection and has the folder's menu (`filer_open_menu`; in a list, under the last row),
  a click clears, the cursor is the arrow; the band takes what it TOUCHES of a
  picture or a name (`band_hits`, a thing per cell); a drag is let go INTO a folder only on its picture
  or name, its bare ground makes room in this folder (`filer_drag_over`'s `onto`; springs follow). The
  soft box is drawn from the same `Thing`. A LIST keeps its rows: a row is the thing (Finder's). Rig:
  `files press <x> <y>` / `drag <x> <y>` / `unband`, `files where corner <n>` (a cell's bare corner). The list's
  zebra is the A mockup's faint one (3.5 % / 4 %), so a selected row stands out of it; its stripes are
  SQUARE and run EDGE TO EDGE (sidebar to window side, the grid's clip cuts them) and on PAST the last
  row to the bottom of what is seen, scrolling with the rows (Max, 2026-10-21, mockup v44: a stripe
  every other row SLOT, first plain; none in an empty folder); the words keep their 18 px in. Finder's clicks: one click
  selects; a SECOND click on the one thing selected — on its PICTURE opens it, on its NAME types its name
  in place once the double click's time is out (`Press::then`, `Then`, `Win::part_at`,
  `filer_rename_soon`, `rename_due`; another click meanwhile is a double click and opens; travel is a
  drag and does neither; `Win::opened` keeps a double click from opening twice). A name being typed has
  its STEM selected (the ending stays out — `Rename::picked`, a caret `Rename::caret`, ←/→/Home/End/
  Delete), in a field of the page's colour with a hairline (radius 4; centred in the grid, left in a
  list — `Typed`), the selected part in the ACCENT with white words; kept inside the field
  (`name_field_x`), the names under it left out. Rig:
  `files click <x> <y>` (a real click at a window point), `files where item <n>` (logged). The grid
  and the list begin at `field_left`; the list's rows are rounded zebra
  stripes `LIST_AIR` in. Rig: `files side open|close`, `pin on|off`, `info on|off`, `tab new|close
  [n]|take <n>`, `pencil`, `sortmenu` (then `pick <row>`), `mode media|files`, `files where
  grid|list|find|seek|sort|info|card|pin|pencil|add|current|tab <n>|tabx <n>|mode <n>|side`; a real
  pointer in a nested rig: `hyprctl dispatch 'hl.dsp.cursor.move({ x = …, y = … })'`. BACK AND FORWARD ARE ON THE WINDOW'S BAR (Max, 2026-10-14: "como las webapps"): each window is TAGGED `golem-files-<id>` (xdg-toplevel-tag, `Filer::tags`) and its history is published to `$XDG_RUNTIME_DIR/waverunner-files-nav` (`filer_note_nav`, the webapps' `seam/nav` format: `<tag> <back 0|1> <fwd 0|1>`); waveview ≥ 2.18 draws the arrows only while there is somewhere to go and a click sends the window Alt+←/→. Keys and the mouse's side buttons as ever.
  (`crumbs`: from Home, a stick or a phone, else `/`; the first steps give way in a narrow
  window). Side: `places` (Home, the owner's folders under their names on disk from
  `user-dirs.dirs`, `~/Phones`, what is plugged in — `Desktop::volumes`, refreshed from
  `on_mounts`); left out under `SIDE_FROM` wide. Listed OFF the loop (`filer_list` →
  `desktop_off_loop`, a `generation` drops a late answer), folders first then `by_name` (numbers
  by value), followed with inotify per window (60 ms coalesce). Click = select (Ctrl adds), double
  click = open (a folder here, a file through `xdg-open`), mouse side buttons / Backspace /
  Alt+←→↑ = back, forward, up (coming out of a folder leaves it selected — `Win::reveal`), arrows
  walk, Enter opens, with ALT held W A S D walk as the arrows, E is Enter and Q goes back (the left
  hand's keys), typing searches (below), Ctrl+A/H/R/N/W, F5. The ground is the
  dock's colour made solid (colours are LINEAR here). THE MENU (2026-10-13, second cut) is the
  desktop's `Menu` with this window's rows (`menu_rows`, `Menu::with_rows`), drawn IN the window
  (names under it are left out — `covered`; it cannot hang past the window's edge; UNLIT, as the
  Properties box, the sort menu and the History — `MenuPaint::glass` from `filer_panel_paint`: the
  dark look's night light sank their lower rows toward black; the desktop's menus too, `desktop_panel_paint`): its rows are THE
  ONE MENU's (`desktop_menu::rows_for`, from `filer_open_menu`'s `Ctx`; a place of the sidebar has its own —
  `filer_place_menu`, a stick's or a phone's rows going to its desktop icon's handlers: `desktop_volume_act`);
  a row acts on RELEASE (`filer_menu_row`: a page turns, what cannot be undone asks once, Get info opens the i
  card — a place's, the Properties box grown out of the menu), everything goes through `filer_act` — and so do the keys: Ctrl+C/X/V, Delete, F2, Ctrl+Shift+N,
  Escape. Cut/copy = `serve_files`, paste = `desktop_send::put` off the loop (what came is left
  selected), the bin = `trash_file`, a name is typed in place (`Rename`, by PATH; the rules are
  the desktop's, now shared: `desktop::typed_name` / `rename_file`; a new folder's name is typed
  as soon as it is listed — `rename_next`; the keyboard leaving keeps what was typed). ZOOM (Max, 2026-10-13: "it zooms
  all"): Ctrl +/−/0 → `zoom("files", …)`, one zoom for every file window (`settings.files_zoom`);
  the window is laid out `1/zoom` its size (`Win::virt`) and the renderer's scale is multiplied by
  it (`filer_zoomed`, the configure, `rescale_surface`), the pointer divided (`filer_pointer`) — no
  layout code knows. `files zoom in|out|reset|<n>`. DRAGS (third cut, the desktop's way): a
  press that travels `DRAG_START` (`Press`, `filer_travel`) on a thing LIFTS it and its group
  (`filer_lift`: a real Wayland drag, `Filer::drag` = `Lifted`, the icon as its image, uri-list +
  text; `DataSourceHandler` asks `Filer::is_drag_source` first), from bare ground draws a RUBBER
  BAND (`Win::band`, `band_hits`; Ctrl keeps what was selected). A drag over a window —
  another app's, the desktop's, a window's own — is `Filer::dnd` (`DataDeviceHandler` asks
  `filer.id_of(surface)` / `dnd_active` first): the folder under it is washed (`Win::drop_on`), the
  drop ALWAYS reads the list through the wire (`filer_dnd_drop` → `filer_bring` →
  `desktop::import`: a move on one disk, a copy across disks; into the folder under the pointer,
  else the folder shown; back where it came from = nothing). Over the DESKTOP a window's drag is
  another app's to it; over the DOCK it is "ours" (`Filer::dragging` in `desktop_drag_dock_pos`,
  `desktop_dnd_enter`, `desktop_dnd_drop`): the bin opens and takes it. Rig verbs: `files band
  <x0> <y0> <x1> <y1>`, `unband`, `drop <n|-> <path…>` (the bringing-in only — the wire itself
  needs a real pointer). THE BIN is a place (`bin_root` =
  `Trash/files`, read through the trash's own records — `list_bin`: the names things had, newest
  first; `Win::in_bin` → `InBin`): its menu is Open · Put back · Delete for good, the ground's
  Empty the bin (`Action::PutBack|Erase|EmptyBin`; what cannot be undone is asked once — the row
  turns to "Click again to delete", `Win::armed`; a thing is never put back over one that took
  its place); nothing is made, named or pasted there. A drag can be let go ON A PLACE
  (`Win::drop_place`), and into the bin — its place or its window — means thrown away
  (`filer_bring`). IT IS THE SYSTEM'S FILE MANAGER (Max, 2026-10-13): the entry claims `inode/directory` and is made
  the default for it ONCE (`xdg-mime default`, marker `files-default-set`; Golem's `home.nix` names
  it first), and the shell's own folders open in it directly — the desktop's (`desktop_activate`),
  a notification's "view" (`notif_open`). "SHOW IN FOLDER" is ours too: the dock IS
  `org.freedesktop.FileManager1` on the session bus (`serve_file_manager`, a thread with its own
  runtime; the name asked for over whoever holds it, else waited for in line): ShowFolders opens
  them, ShowItems / ShowItemProperties open each thing's folder with it selected
  (`Shown`, `shown_places`, `filer_serve_dbus`). ⚠ a test rig must NOT share the owner's session
  bus (it would answer for the real desktop): give it `DBUS_SESSION_BUS_ADDRESS` of a private
  `dbus-daemon`, then `gdbus call --session --dest org.freedesktop.FileManager1 …`. THE TILE: `materialize_entry` writes `golem-files.desktop` ("Golem Files",
  `Exec=<the ctl beside the daemon> files`, `StartupWMClass=golem-files`) at start; it claims no
  file type. MAKE ROOM (Max,
  2026-10-13: "que hagan espacio como en el grid de aplicaciones… usa el mismo código"): while a
  drag is over a window the things part around where it would land — the Apps grid's reflow
  generalised to a group (`parted`; a test holds it equal to `pages::shifted_slot` for one
  icon), eased at the grid's `MAKEROOM_RATE` (`Win::slide`, a display slot per thing, drawn
  through `Layout::cell_at`, the grid's `cell_pos`); what is in hand leaves its place
  (`in_hand`), the gap (`Win::gap`, counted among what is not in hand) goes to the side of a
  thing the pointer is on (`gap_at`, `Layout::slot_at`), and a FOLDER'S MIDDLE holds still to be
  dropped into (`filer_drag_over`). What is let go LANDS (Max, 2026-10-14: it went back to its place and came out again): things of the
  SAME folder are reordered AT ONCE, on the loop, nothing read (`settled`, in `filer_dnd_drop`);
  for what comes from elsewhere the order is saved BEFORE it arrives and the gap stays open until
  the listing has it (`Win::gap_keep`, `filer_bring`). Let go in the gap, it STAYS there: the folder is ARRANGED BY
  HAND from then on (`Orders`, `files-order.json`: dir → names in order; `arranged` at every
  listing, newcomers last; a rename keeps its place) — also how things of one window are put in
  order by dragging. The ground's menu gets "Clean up" = by name again. Never in the bin. Rig:
  `files over <x> <y>` (a drag hovering there; alone: gone), then `files drop - <path…>` lands
  in the room made. LONG COPIES are TASKS on the bar
  (`filer_copy_task` → `task_begin`, only when bytes will be copied — `desktop_send::will_copy`:
  a move on one disk is a rename): the worker weighs first (`weigh`), copies a megabyte at a time
  (`Meter`, `copy_counted`, `put_metered` / `desktop::import_metered` — the old `put`/`import`
  are these with no meter), reports, and stops between two megabytes when Cancel is pressed
  (half a copy is taken away, and that is not a failure); what could not be copied is said once
  as a notification (`filer_say_failed`). A drag held near the field's top or bottom SCROLLS the
  folder (`EDGE_ZONE`, `filer_edge_tick`). ARCHIVES: "Extract here" on one, "Compress" on any
  selection = File Roller (`file-roller --extract-here` / `--add`, `Action::Extract|Compress`,
  `is_archive`). `menu_rows` takes an `Asked`. THE LIST (`settings.files_list`, one
  choice for every window; the top line's grid | list or Ctrl+1 / Ctrl+2, `filer_set_list`): a
  `Layout` of ONE column whose cell is a row (`Layout::of(…, list)`, `LIST_ROW`), so the pointer,
  the keys, the band, drags and make-room know rows with no code of their own (`slot_at` gives
  how far DOWN a row); only the drawing differs (`icon_in`, `columns`: name · size · date, the
  last two only where there is room; `Win::meta`, read with the listing). No strip of column
  names: the top line's sort menu orders (`Win::sort`, `sorted`: in THIS window, folders first; Name =
  the folder's own order) — while ordered by anything but Name no room is made for a drag. SEARCH (JUST TYPING — the first letter brings the field out, the clipboard box's way; also the
  magnifier, or Ctrl+F): the field at the top line's right end, the folder showing
  only what holds the words (`Win::find`, `Win::all`, `found`, `filer_find`; Escape puts it
  away, Enter opens the first found, the walking keys go on to what is found); not into
  sub-folders. Rig: `files view
  list|icons`, `sort name|size|date [up]`, `find <words>|-`. SHIFT is VIM'S VISUAL MODE (Max,
  2026-10-22: "mantener apretado shift selecciones como visual mode en vim"; arrows and W A S D alike):
  the anchor is where the cursor stood as Shift went down (none: the first walk picks the first thing;
  `Win::visual`, `filer_walk_to`), each walk moves the cursor and the selection is the RUN between
  anchor and cursor in reading order (`select_to`, `span`) — back toward the anchor it shrinks, past it
  it turns; letting Shift go keeps it (`filer_shift_up`, from `update_modifiers`) and the next Shift
  begins again from the cursor; a plain walk is one thing; Escape clears it once nothing else is up. A
  click sets cursor and anchor, Ctrl+click the cursor only, Shift+click runs from the anchor. No ring on
  the cursor's thing. Rig: `files key shift <d|a|…>` (held), `files shiftup`. A drag HELD
  on a folder `SPRING` opens it under the drag (`Win::spring`, a timer from `filer_drag_over`).
  SPRING-LOADED SIDEBAR (Max, 2026-10-21: "the dirs open when i drag something on top"): a drag over
  the rested sidebar opens it AT ONCE (no `SIDE_WAIT`; `side_follow` folds it when the drag leaves it,
  unless pinned, and `filer_dnd_leave` when it leaves the window); held `SPRING` on a folder place
  (`Win::place_spring`) the place BLINKS (`place_blink`, its box gone 110 ms) and opens in this tab
  (`filer_place_spring`) — the drag goes on over its files and their own spring; a drop on a place
  still goes straight into it. Not Recents, not the bin, not the folder shown; `..` climbs as ever
  (`up_hold`). As the files' spring, `sprung_from` takes the window back if the drag leaves without
  being let go. Rig: `files over <x> <y>` is a drag the springs believe (`Filer::rig_drag`), `files
  over` alone ends it.
  THE SESSION: the open windows' folders are noted in the runtime dir (`session_path`,
  `filer_note_session`) and a dock started again within the login opens them again
  (`filer_restore_session`, from `main` just before the loop) — where each stood is the
  compositor's to give. THE SEARCH GOES ON BELOW: `DEEP_WAIT` after the
  last letter the folders under this one are walked off the loop (`search_below`: breadth first,
  `DEEP_DEPTH` deep, `DEEP_TIME`, `DEEP_MAX` things, links to folders not followed) and what is
  found there follows the folder's own (`Win::deep`, `deep_from`, `show_found`; `deep_gen` drops
  a late answer), each said with its folder (`below`: dim under the name, before it in the
  list). OPEN WITH (a file's menu): the row turns the menu's PAGE, as the desktop's "Move to"
  does (`filer_menu_apps` / `filer_menu_back`) to the apps
  that say they open that kind of file — `xdg-mime query filetype` then `gio mime`, off the
  loop (`apps_for`, `parse_gio_mime`), each id looked up in the dock's own index for its name
  and command (`Win::open_with`, `Action::OpenApp(n)`). Rig: `files apps` / `menuback` with a
  menu up. THE OWNER'S OWN PLACES are GTK's bookmarks (`gtk-3.0/bookmarks`: `marks_path`,
  `parse_marks`, `marks_with`; `Win::marks`) — a folder's menu has "Add to sidebar" / "Remove
  from sidebar" (`Action::AddPlace|RemovePlace`), a kept place's own row has a menu to take it
  out (`filer_place_menu`, the place handed to `filer_act_on`), and what is kept here is in every app's open/save dialog, and the
  other way round. The ground's menu shows or puts away dot files (`Action::ToggleHidden`).
  What was CUT is drawn FADED until it is pasted (`clipboard_files` cut → an icon wash toward the
  ground at `ring = -1.3`: at -1.0 a tinted icon also wears the dock's bin can; `filer_clip_changed`
  redraws). With Ctrl held the WHEEL zooms, a notch a step (`Win::zoom_run`). A VIDEO OR A GIF PLAYS IN ITS TILE while the pointer rests on it (Max, 2026-10-17, `filer_live.rs`): after `REST` (250 ms) Quick Look's player runs at `Way::TILE` (15 fps, silent, `-stream_loop -1`, frames on the tile's 256² clear square, a big video on the graphics chip) and each frame goes into the layer the thumbnail is in (`filer_tile_layer`); the pointer leaving (or the window closing) stops it and the still goes back (`filer_still`). One decoder at most, only while looked at. An animated WEBP (no ffmpeg decoder) is read in-process at the tile's size on a thread (`peek_kinds::animation`, `squared`) and played on the loop's clock (`filer_live_tick`).
  HARDENING (a review, 2026-10-14 — keep these): a menu's row acts on the
  thing the menu is ON (its group only while it is one of it — `filer_act`); a click is on what is
  under the pointer NOW (`filer_click` reads the spot again: the folder scrolls under a still
  pointer); a drag LET GO is no longer in hand (`Lifted::dropped`, from the source's
  `dnd_dropped` — its target may read for minutes; `Filer::dragging` is false from then, the
  dock's bin knows ours by `Filer::over_dock`); the bin's Put back / Delete act only on what lies
  IN the bin (by its record's name — never a thing inside a thrown-away folder; no search below
  in the bin; nothing is dragged OUT of the bin); `anchor` and `cursor` follow their thing across
  a listing; a typed name keeps the file's REAL ending (`desktop::ending`: `tar.gz`, not the
  "2 final" of "v1.2 final"); a folder is only "arranged by hand" when a drop changed its order;
  throwing away, undo, put back, delete and empty run OFF the loop; a folder that changes while
  it is read is read once more after (`Win::again`), not on every change. UNDO — THE ↶ AND ITS HISTORY (Max, 2026-10-21, mockup v47:
  "good build it"; `filer/history.rs`, drawn in `filer/chrome.rs`): each WINDOW keeps what was done
  in it (`Win::history`, a `history::Did` each — the `Undo`, what it is called, its symbol, the
  folder, when; newest last, up to 100, gone with the window), recorded by `filer_did(id, …)` /
  `filer_binned` (a thrown thing is named as it was, not by its name in the bin): a name goes back,
  things moved go back where they were (`undo_of` tells a move from a copy by what was left
  behind), copies go to the bin, what was thrown away comes back, a new folder goes while it is
  still empty — never over something that took a place since (`take_back`; `can_undo` asks the
  same of the disk, OFF the loop, at every listing, after each thing done or undone, and while the
  history is drawn at most once a second — `filer_history_check`, `Did::can`). The ↶ stands LEFT of
  sort, 12 px from it, only while something can be taken back: it slides out of nothing (its width
  and its air, `Win::undo_t`) and away — never a dimmed button. A click on it or Ctrl+Z takes back
  the newest that still can be (`filer_undo`). A RIGHT click opens the HISTORY under it, its right
  end at the tools' end, in the menus' paint (unlit: the dark look's night light sank its lower
  rows): a dim "History", then the rows newest first — the symbol, what was done
  (`Renamed “README.txt” to “notes.txt”`, `Moved 3 items to the Bin`, `New folder “…”`, `Copied 2
  items to Pictures`…, fitted), a dim line `<folder> · just now|3 min ago|1 h ago`, and a quiet
  Undo (a step off the panel, the ink, no accent) — or "Undone" (the words struck through) or
  "Can’t undo". A row's Undo takes back THAT one, an older one too, the newer staying
  (`filer_undo_at`; marked at once, unmarked with a notice if it failed); the menu STAYS and
  goes by itself with the last thing there is to take back; Escape or a press elsewhere puts it
  away (a right press opens that menu instead); the wheel scrolls its rows (`hist_first`); words of
  the files under it are left out by their drawn width (a list's names beside it stay). What was
  taken back is said QUIETLY (a transient, low notice for 2.5 s). Not undone, not listed: a hand
  arrangement, an emptied bin, a delete for good, what the dock's bin took. THE SELECTION PILL (Max,
  2026-10-21, mockups v48–v49, `chrome::filer_pill`): floating 14 px above the foot of the files, centred on
  them, in the menus' paint with a hairline and a soft shadow; it RISES and fades in (`Win::pill_t`, the view
  kept while it goes — `pill_view`), and the files get room under their last row while it is up
  (`Layout::foot`, `PILL_ROOM`). What it shows (`filer_pill_view`): a line SAID a moment (`filer_flash`,
  `FLASH` 2.6 s: "Copied 3 items to KINGSTON", "Sent … to Pixel 8"); else the SELECTION — "“name” 2.6 MB" /
  "5 selected · 2 folders · 3.4 MB", then Copy · Move · Rename (one) · Open with (one file; the ⋯'s menu on its
  Open with page) · New folder (two or more: "New folder with items") · Copy to <stick> — five with words at
  most — the bin, ⋯ (THE ONE MENU for the selection, hanging UP from over the pill: `Menu::up`,
  `filer_pill_menu`) and × (`PillAct`); in the bin only what it is and ×; else the FILES ON THE CLIPBOARD:
  Copy / Move (the button, the menu, Ctrl+C/X) let go of the selection and they wait there — "3 items copied
  · Copy here · ×", "“x” to move · Move here · ×" (greyed where nothing can be put: Recents, the bin, Media —
  `Win::put_here`); what waits to be moved is drawn at 45 %; × forgets them (`Filer::clip_forgot`), a move
  pasted is spent. Its symbols are the chrome's own (always in the window's layers). Its first words (the name,
  the count, a line said) are NOT bold (Max: thin, never heavy): `content::FONT_MEDIUM`, the words' own family's
  medium where it has one (`renderer::SANS_MEDIUM`), else regular in full ink — Golem's FreeSans has none, and a
  bare weight of 500 matched JetBrains Mono's Medium instead. Rig: `files pill <n>`
  (its nth button), `files flash <words>`. ACTIONS BY KIND (Max, 2026-10-21, mockup v51: "one action per case
  on the pill, two, even 3 on the context menu; only actions that just happen"; `filer/smart.rs`, `sort_of` by
  ending, `filer_smart` → the rules alone, `smart_files`, tested as a table): songs/videos → Play (+ Shuffle for
  2+); one photo → Set as wallpaper, Rotate, then Print with a printer, else Slideshow from here; photos →
  Slideshow, Rotate (+ Print with a printer) — the pill keeps Set as wallpaper / Slideshow; a PDF, text or
  PostScript → Print; an office file (doc docx odt rtf xls xlsx ods ppt pptx odp) → Print only with LibreOffice
  too; any mix where EVERY one can be printed → Print; with no printer none of these, and a document's pill shows
  Open with again; an archive → Extract here (the pill only); a
  folder that is mostly songs or photos (`folder_kind`: 70 % of at least 3 files, read once a change —
  `kind_of_folder`) → Play all (+ Shuffle) / Slideshow. The pill wears ONE, first, in Open with's place; the menu
  up to three, right after Open (`Ctx::smart`). THE DESKTOP'S item menus get the same rows from the same rules
  (Max, 2026-10-21: `filer::smart_for`, in `desktop_menu_ctx`; whether there is a printer asked as the desktop
  starts and its menu opens — `print_can`), and the same doing (`App::smart_act`, for a file window's menu or
  the desktop's — `SendFor`; `desktop_smart_act`): Quick Look plays and shows from the desktop
  (`quicklook::DESKTOP`), "from here" through the desktop's photos in the order of their cells, wallpaper and
  rotate in the desktop's own history (`said_of`), a print said quietly (no pill there). BY THE ↶: the folder shown's own Play all / Slideshow (icon +
  word, `filer_here_kind`, `Hits::here`), standing still left of sort — the ↶ slides in LEFT of it. What each
  does (`filer_smart_act`): PLAY = Quick Look's player through a PLAYLIST (`peek_slides.rs`: `peek_playlist`,
  each started by `peek_autoplay`, the next at its end — a song's mpv gone, a video's last frame; a song that
  ends as it begins could not play: the playlist stops, it does not race on); SLIDESHOW = Quick Look full screen
  on black (`Slides`, `slides_push`: the photo fitted, fading in over 0.45 s, the next every `SLIDE` 2.8 s, the
  caption "name · 3 of 52 · ×"; the arrows or a click turn it, its × or Escape end it; "from here" starts at
  that photo in the folder's order); SET AS WALLPAPER = the box's own pick (`controls::pick_wallpaper`),
  undoable back to the one before (`Undo::Wallpaper`); ROTATE = a quarter clockwise and NEVER a pixel lost — a
  JPEG by its EXIF orientation only (`exiftool`, else `jpegtran -perfect`), a PNG drawn again; undoable
  (`Undo::Rotated`: three more quarters); its pictures made again (`thumb_stale`, `peek_forget`); PRINT
  (`filer/print.rs`, Max 2026-10-21: "only when a printer exists") — what can be printed (`print::Can`: a
  destination in `lpstat -e`, `soffice` on PATH) is asked OFF the loop and kept 30 s (`Filer::printers`,
  `filer_printers`: again as a window opens, and from a draw once old; never waited for; the windows draw again
  when it changes), so it follows whatever CUPS the system has (no `lpstat`/`lp` on PATH = no printer); printing is
  `lp -d <dest> -t <name>` to `lpstat -d`'s default, else the first, off the loop (C locale; 8 s at most a
  command): a photo CUPS takes no such of (heic, webp…) is made a PNG first, an office file a PDF by
  `soffice --headless --convert-to pdf` (its own profile in the cache, 120 s at most), in a scratch folder
  removed after; text goes as `document-format=text/plain`. Flash "Printing “x”" / "Printing 3 documents" (or
  photos; "Getting ready to print…" first for office files); what failed is said quietly with lp's reason. Rig:
  `files printers` (asks again now; the reply is in the log); stubs that log their arguments in place of
  `lpstat`/`lp`/`soffice` on the rig dock's PATH. Rig: `files
  here` (the button by the ↶). Rig: `files undo`,
  `files hist [open|close|undo <row>]` (rows from the top; the reply/log is the history),
  `files where undo|histrow <n>|histundo <n>`, `files rclick <x> <y>`. A PATH CAN BE TYPED (Ctrl+L, or a `/` or `~` typed —
  `Win::go_to`, drawn in the steps' place): Enter goes (`typed_place`; a file's folder comes up
  with the file selected), Tab finishes the name as far as one thing can be meant
  (`complete_path`), Escape or a click leaves. Rig: `files path <text>`. NOT BUILT:
  `inode/directory` + FileManager1. Not wired either: a dock tile / `.desktop` entry, `inode/directory`,
  FileManager1. Pointer-free: `waverunner-ctl files [<folder>]` opens a window; `files go
  <folder>|back|forward|up|select <n>|activate <n>|key <name>|type <letters>|scroll
  <px>|hidden|menu [<n>]|pick <row>|copy|cut|paste|newfolder|rename <n>|bin|props <n>|close|state`
  drive the newest (⚠ `activate`/`key enter` on a file really opens it; `bin`, `paste`, `pick` are REAL).
- `app_actions.rs` + `web_actions.rs` — THE LONE APP (Max, 2026-10-22; look "3a", canvas
  https://claude.ai/artifact/9WByNs3AHU4wZNYCR9tUgi): a search that leaves ONE app in Apps lays its
  row out round it — the app in the middle, selected (`SectionLayout::anchor`); to its right its
  desktop file's actions, Golem's own for apps it knows (`KNOWN`: VLC, mpv, GIMP, OBS), A WEBAPP's
  known pages (`web_actions::SITES`, ~45 of the world's most used sites: YouTube's Subscriptions,
  Instagram's Messages, Amazon's orders… opened in its window by `seam -golem-app <slug> <url>`) and
  Uninstall last (asked twice); to its left, while it has windows, Show all (waveview's `spread_app`)
  and Close all; under it "Recent in <app>" (`AppsPart` room, `RECENT_ROOM`): its files (the system's
  `recently-used.xbel`, VLC's list; the program names the app only when no other app shares it) or,
  for EVERY webapp (never the files: its program is Seam's) and a BROWSER, its pages — a webapp's
  from Seam's history, its site's kinds of page when `SITES` knows it, else any page of its own host
  but its start, a sign-in or one titled only with its name (`webapp_pages`; Keep's are all
  "Google Keep": none); Seam's own without the installed webapps' sites — read OFF THE LOOP from
  the browser's own history (Seam/Firefox/LibreWolf `places.sqlite`, Chrome/Chromium/Brave/Edge/
  Vivaldi `History`; each reading its OWN copy, removed when it is done — `Copied`: two readings at
  once shared one and each copied over the other's; kept `FRESH` 60 s by app; an app's FILES are read
  off the loop the same way, `Source::Files` — the record read whole and each file looked for must not
  hold the box), pictured
  with no network: a YouTube video by its own picture from SEAM'S DISK CACHE (`cache2`, an index of
  `i.ytimg.com/vi/<id>/` entries, cropped square), else the page's favicon on a light tile
  (`tile_in`). A page opens in its app's window, which is brought forward (`web_opened`: Seam cannot
  raise one on Wayland). An action is its app's tile at half light with a white symbol
  (`SYMBOLS`; `act_cells` in `content.rs`). A SITE'S SEARCH: "amazon zapatillas" (`Site::words`,
  then the words — never when the query begins an app's name: "google maps") is the first cell,
  alone in its row and selected, "Search Amazon" / "“zapatillas”" spread over three cells
  (`WIDE_LABEL`), opened in the webapp when it is installed (on its own country: amazon.com.mx),
  else in Seam. THE EXACT NAME (Max, 2026-10-22: "'googl' muestra la grilla, agrego la e queda solo
  Google, agrego un espacio y aparece la grilla sin Google"; `narrow`): a query that IS an app's name
  leaves that app alone (Golem's own also by their name without "Golem"; the most used of several
  of one name), and a SPACE after it the apps whose names go on past it, without it ("youtube " →
  YouTube Music). SHIFT ON AN APP (Max, 2026-10-22: "press shift at any time on an icon in the box
  to take it to this"; `box_shift`): Shift TAPPED alone (pressed and let go with no other key between —
  `press_key` / `release_key`, so a capital letter never does it) takes the app under the pointer, or
  the one the keys are on when they moved last (`SearchState::keyed`), to its row, whatever was typed
  or with nothing typed (`SearchState::focus` = the app and the query then; `grid_resting` is false
  meanwhile); Shift again, Escape, typing or the box going away go back. Apps of the grid only (not a
  file, a package, a box, an action, a recent). Shift WRITES THE APP'S NAME in the search — with a space after
  it when it can search, the words come next (`SearchState::shifted`: what is typed after that name searches in
  it, even what begins another app's name) — (Max, 2026-10-27:
  "am, Shift on Amazon, space, pixel 8 = browse for pixel 8"; Shift again / Escape put back what was typed):
  a query that begins with an installed app's NAME then a space and words SEARCHES IN IT (`search_asked`,
  the longest name: "YouTube Music lofi" is YouTube Music's) — a webapp whose site has a search (in that
  webapp), or a BROWSER (`browser_search`: the Firefox family `--search`, Chromium's their engine's
  address); else a site by one of its words ("yt gatos"). NAMES TOO LONG FOR A LINE (Max: "dos líneas cuando sea necesario"; they were
  cut at a word by the estimate, so "YouTube Music" read "YouTube"): measured once a cell's room
  (`App::fitted_names`, `name_fit`), TWO lines broken between words where the space under the cell
  is free (no cell below, a last row with no page dots, the parted row), else ONE with "…" — the
  rows are a line apart (`content::NameFit`, `SECOND_LINE`); files keep their middle cut
  (`tile_label`). THE RECENTS' ORDER (Max, 2026-10-22: "los primeros tres ítems sí sean recientes
  puros… y después el ranking"; `web_actions::rank`): the `NEWEST_FIRST` (3) last opened by time —
  to take up where one left off —, then the rest by use: a page's visits (Firefox
  `moz_historyvisits`, Chromium `visits`) or a file's count in the record, each forgetting as the
  dock's launches do (`usage::decayed`, two-day half-life), plus the owner's PICKS from the box
  (`recents-use.json`, `SearchState::picks`, worth three visits and as new as the pick). Up to
  `RECENT_MAX` (24) in PAGES of a line (`RECENT_PAGES` 4): dots under the line, → past its last /
  ← before its first turn them, the wheel over them too (`recents_wheel`, the grid's pager). WHAT
  SEAM COOKED (Max: "make Seam leave the files 'cooked' and pick them up with the shell from a
  dir" — the dock NEVER goes on the network; Golem's `seam/golem-chrome.js`, the recents' kitchen):
  `$XDG_STATE_HOME/golem/seam/recents/recents.json` + `pics/` — a page's title as Seam saw it last
  (`retitled`), its preview picture, and what it PLAYED (`played`: a music site's songs as "song •
  artist", opened where they played when that is a page of the site's own, else as a search of the
  site for them — Spotify plays from a search page; with their covers); `with_plays` makes a song and
  its page one. Pictures: what played > the page's preview > YouTube's from Seam's cache > the
  favicon, each kept WHOLE in its own shape (`fitted`; Max: "forma libre" — a video's a rectangle, a
  cover square; only what YouTube padded it with cut — a 4:3 picture's black bands (looked for a JPEG block away from the picture: its edge rings), and on a video's page or a list's (`list_id`: its picture is its first video's) the bars of one colour round a song's cover, `unbanded`, its whole cut kept for Quick Look, `big_of`; on a video's page the bigger of what played and the page's own, `heard_or_page`; Seam fetches the covers and names of songs and lists heard before the kitchen, COVERS FROM BEFORE) and drawn so (`content::RecentsDraw`,
  `preview_rect`: its bottom on the icons' line, `PREVIEW_H` an icon high at most, a cell wide), the
  recents' pages SLIDING as the box's do (a `Pager`, `recents_turn` / `recents_slide`: the page that
  went is kept as `Leaving` and slides out the other way; every picture a page had a hand in decoded within bounds —
  `decode`: 4096 a side, 64 MB —; the picture caches trimmed to `PICTURES_KEPT`). Read as data a page had a hand in (`Kitchen`: 4 MB,
  http(s) only, pictures only `<16 hex>.<jpg|png|webp>` plain files in its `pics/`). On a
  short screen the card GROWS for the recents (a negative trim,
  `apps_trim_target`). Keys: ←→ walk the row, ↓ the recent under, ↑ back (`app_actions_key`).
  Real data: `cargo test -- --ignored real_recents --nocapture`; THE AUDIT of every installed webapp's row
  (`web_actions/audit.rs`, `XDG_CACHE_HOME=<scratch> cargo test -- --ignored real_every_webapp --nocapture`:
  one line a finding, none the pass). 2026-10-28 (Max: "check all the apps… use all the bugs we found as clues
  for a deep debug, we need no errors"): ONE PAGE ONCE (`page_key`: a video, a list, on a known site the thing
  after its kind — `ID_MARKS`: a Reddit post's comment or picture open, an X post's photo —, else the address
  without a "#lightbox" or a GitHub "?tab="); named by ITS OWN title (`once_per_page`: the newest one that is
  not one the app's own pages wore — `chrome_titles`: its start, known pages, search, sign-in; `chrome_word`:
  "Home", "Inbox", "Web Player", "<q> - Search" — nor denied by its address's words, `fits_address`), else
  its address's words (`address_title`: "What to work on outside of work"), else not shown; a site's known
  page is its row's action, not a recent (`is_link`); a sign-in's steps (`passing_path`: GitHub's "/sessions/",
  "client_id="…) and a chat not kept yet (`placeholder`: ChatGPT's "/c/local-…") no place; titles without the
  marks that show nothing nor runs of spaces (`INVISIBLE`), a site's last word stripped too (" : r/*"); a
  music site's play with no artist is an advertisement; a page with no favicon wears a sheet (`SHEET`: Quick
  Look shows it by it); Chromium's favicons fall back to the site's. NOTHING OF A PAGE WIPED KEPT: a reading
  (`Reading`) says which of the owner's picks and put away the history has no more (`history_with`, `gone` —
  the loop forgets them, `recents_tidy`) and when Seam's history was cleared whole (the kitchen's `cleared`:
  every web key and the pictures go, once — `Forgot::seam_cleared`); the dock's copies of Seam's pictures
  only while Seam keeps what they were made of (`sweep_unkept`). 2026-10-29: a song's page wears its
  play's name when that says who sings it (`with_plays`: "Mentirosa" → "Mentirosa • Ráfaga"); Seam
  asks YouTube for the rest (Golem `rkWhole`, `noArtist`). Site WORDS are never a word said every
  day, a letter or a name ("so", "x", "g", "ali" asked a search: gone), and an app named by one
  letter (X) searches only Shift-written. Google Keep is a site (links: keep.new, #reminders,
  #archive, #trash; search `#search/text%253D{q}`; no recents: every page is "Google Keep").
  A FILE CARRIED OUT OF THE BOX (`filer.rs` `box_file_carried_out`): a File entry dragged out of
  the card (not over the dock) becomes a Wayland drag of ours — `filer::Lifted` with
  `win: quicklook::BOX`, so every file target of the Files window's drag works (another app, the
  desktop, a Files window, the bin) — the box collapses and hands the keyboard back (an
  exclusive layer pins Hyprland's pointer), and the dock goes as a closed box's at the drop
  (`filer_dock_back` → `dismiss`). Picture: `thumbs::made`, else the type icon
  (`notif_icons::resolve_now`). Rig-checked into a Files window (moved), foot (path pasted),
  the bar (let go nowhere). A RECENT of the lone app's row (`box_recent_lift`, from the press
  motion past 14 px) goes out at once — a file as itself, a page as its LINK (`Lifted::links`:
  uri-list + text); the desktop, a Files window and the bin refuse links (`carries_links`),
  the desktop is for files. APPS NEVER LEAVE the dock or box (Max): their drags stay internal.
  PICTURES OF ONE PAGE (2026-10-29, Max: "give [Wikipedia] thumbnails… use that issue as a clue"):
  a visit wears the kitchen's picture of its own address before the grouping (`kept_pictures`:
  a page several visits are shows the one any has — the dock looked only at the address it
  shows); the kitchen's logo check counts one page's addresses as one (`page_key` in
  `kitchen_in`: a post and its comments). Wikipedia: a "#/media/" picture view is its article,
  `WIKI_OWN` pages none; Seam fetches pictures for the articles from before (Golem
  `rkWikiBackfill`). The audit prints "previews: N of M" per app. 2026-10-27 (Max: "do 1 2 3 and
  5"): PUT AWAY — Delete on a recent (the pointer's, or the keys' when they moved last:
  `box_delete`) hides it until it is SEEN AGAIN (`web_actions::Forgot`, `recents-forgot.json`: a
  key → when; one visited after comes back, as in a history) and forgets its picks; the Launcher
  page's Recents switch (`launcher_recents_off`: none shown, NONE READ) and Clear recents
  (`recents_clear`: everything seen until now put away, picks and `web-tiles`/`web-pictures`
  gone). Every reading takes a `Wanted` (picks, forgot, words, max) and filters BEFORE ranking;
  the row filters again by `Recent::last` (a reading that was in flight). RECENTS IN A SEARCH
  ("youtube daft", `add_search_cell` → `add_recents(.., Some(words))`): under the search cell,
  the app's recents that say every word (`web_actions::says`: title, or the address for words of
  3+), in place of the other apps found; read off the loop for the words (`found_recents`, the
  whole history, one reading at a time per app; meanwhile those read already that say them).
  SPACE on the SELECTED recent (the keys' or the pointer's — the box has one selection,
  `pointer_takes_selection`; typing puts it back on the app, so "youtube linux" still types)
  = QUICK LOOK (`box_peek`, `quicklook::BOX`): a file as any;
  a PAGE as the page (`peek_page.rs`, Max: "the quick look is small and tells nothing"): its picture
  as big as it was found (`web_actions::big_picture`: the kitchen's file, or a video's out of Seam's
  cache, `Recent::big`) at most `SHARE` (70 %) of the screen's width and `GROW` (1.75×) its own pixels
  (Max: "es muy grande, los thumbnails se pixelan", then "un poco más grande" thrice), under it the
  WHOLE title (3 lines), "Watched / Played / Visited <when> · N times" (`seen_line`,
  `Recent::times`/`played`) and the address; a favicon-only page is a card, the tile beside the
  words; the head is the app's name, its host, and THE CARD'S BUTTONS (Max: "use the buttons we use
  on the card instead of the pills" — the quiet head: a Nerd icon alone at 72 % ink, a round wash
  under the pointer; `peek_page::head`/`page_head_push`): open · link · bin · ×, evenly (Max: "change
  the order … and align those 4 buttons"), each icon sized and placed so its DRAWING is ~12 px and in
  the middle of its button (`peek_page::icon`, measured on the rig with `measure-head.py`-style ink
  boxes), a check on the link a moment after a copy; rested on `TIP_DELAY` (0.5 s) a button says
  what it does in Quick Look's dark pill under it (`page_tip_follow`/`page_tip_push`, a grid drawn
  last: over the picture), and once one has, the others AT ONCE while the pointer stays on the head
  (`Up::tips_warm`, the dock's names' way). A PRESS Quick Look took keeps its RELEASE
  (`SearchState::peek_press`): the × closed Quick Look and the release, now the box's, read as a
  click off its card and closed the box too. The bin or Delete puts the recent away and closes it (`box_peek_bin`);
  Ctrl+C copies. ⚠ rig: Nerd icons are tofu unless the rig dock gets a fonts.conf that includes the
  HOST's conf.d (its store path; the host fonts.conf includes `/etc/fonts/conf.d` = the toolbox's). ⚠ the box holds the
  keyboard EXCLUSIVELY, so Hyprland pins the POINTER to the dock's layer: while Quick Look is up from
  the box the dock hands it its pointer events (`box_point_on_screen`, moved by `box_to_peek_dy` —
  Quick Look's surface begins under the bar, the dock's at the screen's top); the
  arrows walk the recents with it following, Enter / Open opens the recent in its app
  (`box_peek_open` → `run_recent`), Space / Escape put it back, the box going closes it. OPEN
  COPIED LINK: the clipboard's newest clip, a link copied within `COPIED_FOR` (1 h), first beside
  a webapp when it is on its site's hosts (`web_actions::on_hosts`), beside a browser whatever it
  is (Seam's: not an installed webapp's) — `copied_link_exec`.
- `file_search.rs` (+ `files.rs::file_results`, `app_actions.rs::add_file_row`) — THE BOX'S FILE SEARCH
  (Max, 2026-10-27: "maybe improve file search?" … "do 1 to 7, also quick view on files"). The home walk
  (`apps::scan_home_files`, off the loop at each rescan: hidden, links, `target`/`node_modules`/
  `__pycache__`, any folder with a `CACHEDIR.TAG` — Cargo's `target-*` flooded results — and Golem's own
  `~/Golem` and `~/Golem-local-edits` (`apps::GOLEM_OWN`: on the lab laptops "golem", "home", "system"
  brought two hundred of the checkout's files) left out)
  keeps each file's FOLDER below home folded (`FileEntry::dir`) and its MTIME. `Ranker::rank`: each
  word of the query is a SUBSTRING of the name (nucleo `AtomKind::Substring` — scattered letters
  brought "notification" for "notas") or begins a word of its folders' path (`word_start`: "playa
  2024" → `Pictures/Playa 2024/IMG.jpg`, "factura junio" → `Documents/Facturas/junio.pdf`; one word
  alone of 3+ letters brings a folder's things below what it names); a name saying every word ×1.5;
  KIND WORDS (`KIND_WORDS`, es+en: pdf, fotos/photos, videos, música/music, documentos/docs,
  carpetas/folders, comprimidos/zips) narrow to a kind and alone list it newest first; then THE
  OWNER'S USE — opened lately (`recently-used.xbel`, `opened_from_record`, read with the walk;
  `App::file_opened`) and changed lately (mtime), each `usage::decayed`. WATCHED: the tops of
  Desktop/Documents/Downloads/Music/Pictures(+Screenshots)/Videos (`watched_dirs`, inotify →
  `files_fresh_soon`, 300 ms, `list_top` off the loop, `merge_tops`): a download is found at once.
  What the box opens is NOTED in the record as opened by the app that opens its kind
  (`file_opened_from_box` → `quicklook::default_app`, else the first of `filer::apps_for` →
  `filer::recent::note_as`), so Files' Recents and that app's "Recent in" have it. SPACE on a found
  file the arrows or the pointer CHOSE (`SearchState::chosen`: never the best match typing landed on,
  so "factura junio" types its space) = QUICK LOOK (`box_peek_file`, the arrows follow,
  Enter/Open = `activate`). SHIFT on a found file (or one of the home row) = ITS ROW
  (`SearchState::focus_file`, `add_file_row`): left Copy · Show in Files, the file, right Open with
  <app> (`file_openers`, off the loop) · Quick Look · next openers · Copy path (`Does::QuickLook|
  ShowInFiles|CopyFile|CopyPath`; `filer_show_item`), under it "In <folder>" — its folder's other
  things, a folder's own contents (`Source::Folder`, `folder_recents`; never switched off or put away
  as recents are; the file itself left out as they are read, `Source::Folder::except`); Escape /
  Shift again / typing / the box folding (any way: `sync_surface_state`) go back. Open with runs the
  APP's own command, terminal and graphics card (`Does::OpenWith`) and notes it as opened by it;
  Space anywhere on the row shows the file; Delete there puts nothing away (not recents). A KIND
  WORD is read both ways (`Query::all`): of that kind with the other words, or naming the file —
  "Documentos" still finds ~/Documentos first. A record entry opened by another app gets that app
  added (`noted_by`). A watched-folder change re-ranks only while the owner is not walking results
  (`chosen`) or in Quick Look; one during a reading is read again after (`file_fresh_again`); the
  tops leave out what the walk does (`apps::walk_skips`). A SEARCH THAT LEAVES NO APP (Max,
  2026-10-27: "show the apps empty, install (1 line) and two lines of files";
  `Layout::apps_none`, `content::files_take_apps_row`, from `layout_stretched`): Apps is its
  title over a short "No results" line (`NO_APPS_H`), Install keeps its line, and Files take the
  Apps' row as a SECOND line when they fill more than one (12 a page at six across) — taken out
  of the Apps' room, so what is under Files stays put; `apps_trim_target` aims the card at exactly
  that and says whether the line is wanted, and `App::files_second` eases it open and shut with
  the trim (Install and Files glide, never jump a row). Not under a box open in the grid
  (`box_in_grid`: it keeps its row's room); a package lifted out of Install brings the grid back
  with at least a row to let it go on (the held trim, never under a row). Real home: `cargo test -- --ignored
  real_home_search --nocapture` (QUERIES=a,b,…). Rig: `HOME=<fake home>` for the rig dock, an
  `xdg-open` stub on its PATH, and the host's `xdg-mime`/`gio` (the toolbox has neither).
- More of THE BOX'S FILE SEARCH (Max, 2026-10-27, after a study of Windows' new search, Spotlight,
  KRunner, Raycast, Alfred and Everything: "build all the features for a simple fast search on
  regular search"): WORDS THAT SAY WHEN AND HOW BIG (`file_search::parse` → `Query::when`/`size`;
  "hoy/today", "ayer/yesterday", "esta semana/this week", "semana pasada/last week", "este mes",
  "este año"; ">10mb", "<500kb", "grandes/big" = 100 MB up — a file changed OR opened then; alone
  they list it, newest first); A TYPO FORGIVEN when nothing has every word (`rank_forgiving`: a
  word of 4+ letters one letter off a name's word — two from 8 letters, Damerau `near` —, or off
  the start of a longer one, or its letters in order; names folded once at the walk,
  `FileEntry::folded`, 5–6 ms a keystroke on 17.6k files); "Show all in Files" as the Files'
  last cell (`deep_search::show_all_cell`, `SHOW_ALL_ID`: Golem Files on home searching the same
  words — `filer_find`, which goes on below); Ctrl+Enter on a found file shows it in Files
  (`box_show_in_files`); Move to Bin in a file's row, furthest left (cut on a narrow box: then
  Ctrl+Delete — `box_bin`: out of the index at once, the box stays).
- `deep_search.rs` + `file_inside.rs` — THE DEEP FILE SEARCH ("/" typed into an empty search; Max:
  "the FILE: will be /… a deep file search"): the box is files and nothing else. A "Files" CHIP
  in the search pill (`DeepDraw::chip`; "Files › Facturas" kept to a folder); the THREE SECTIONS
  ARE SHELVES, retitled (`Deep::shelves`, `DeepDraw::titles`): before a word Recent (the record
  + what the box opened) / New files (the watched tops, newest; "New" alone is a new palette's word) / Places (the home's folders); with
  words a shelf A KIND, the best kind first, "PDF · 4" (KRunner's categories), and "Inside
  files" — what SAYS the words; a shelf with nothing FOLDS away, its room the first's, which
  shows more rows (`content::fold_shelves`, `Layout::shelf_room`; the layout stacks the sections
  from the Apps' trimmed room even past nothing, so what is under them stays where the card's
  bottom holds it; `apps_trim_target`'s deep branch aims at the rows the first's page needs).
  Under each name when it was used and, from a MB, how big ("2 h · 8.4 MB", `ago`/`big`; the
  page dots a line lower). THE TABS (Max, 2026-10-27: "no pills… letters as we use on the boxes… under
  the searchbox… 9, then two, three pages more… the ones the user uses start being closer… if i search
  '.html' the tag should be there"): WORDS under the search field (in the boxes' word style — the text font, the lit one(s) underlined — the field moves up into the band
  the System row has — `deep_band`, `controls_layer` —, the tabs `DEEP_LIFT` (14 px) over the field's own place, the field and the tabs' words with EQUAL AIR from the last shelf's dots to the field, the field to the words, the words to their own dots (`deep_dots_bottom`, `deep_tab_h`, `tabs_text_y`, `tabs_dots_y`; the words clipped only at the sides), then the field `DEEP_FIELD_DOWN` (6 px) lower, all of it `DEEP_DOWN` lower and the tabs `DEEP_TABS_DOWN` lower again (4 px each); the field `DEEP_PILL_FAT` (4 px) taller than a bar pill (`search_pill_h`): `Layout::tabs_row`,
  their rects `Layout::controls`; `TAB_FONT_PX` 16.5, one size on every page — smaller only when a box is
  too narrow for its widest page —; the pages SLIDE as the box's do, `Deep::slide` eased toward
  `slide_to` at the pager's rate, round the end the nearer way, `Layout::tabs_at`/`tabs_slide`; dots
  under them lit as the slide passes), the lit one(s) in the full ink, the others quieter (`DeepDraw::tabs`/`lit`). A `Tag`:
  All, a kind, a TYPE by its ending (".html"), a PLACE ("Downloads/" — keeps the search to it);
  NINE a page (`TABS_PAGE`), the first as `first_tabs` (the kinds, Downloads/, .html; then Desktop/,
  Archives, .txt .md .docx … , the other places), then ORDERED BY USE (`by_use`,
  `SearchState::tab_uses` = `deep-tabs.json`, fast launch's forgetting; a use = a click, or a file
  opened while it is lit — `deep_used`; All first; made as "/" opens, never under the pointer). A
  click (again: off), Tab / Shift+Tab (`deep_tab_walk`: one alone, its page shown), the wheel over
  them (`deep_tabs_wheel`), or typing: a kind word or an ENDING (`file_search::ext_of`, `Query::ext`,
  `Narrow::ext`) lights its tab and turns to its page once; an ending some file has and no tab has
  gets one (`deep_tabs`), and kept by use it is a tab from then on. Tab on a CHOSEN FOLDER keeps the search to it (`Narrow::scope`);
  Backspace in the empty field takes the folder off, then the kind, then the deep search; Escape
  lets go of what is picked, then leaves. Ctrl+↑/↓ jump shelves (`deep_jump`). Ctrl+Space or
  Ctrl+click PICK files (`deep_mark`, a check at the picture's corner); Shift then opens the row
  for all of them (`FocusFile::more`: Copy all · Show in Files · N files · Open all · Copy paths
  · Move to Bin). Space shows the chosen one big, Ctrl+Enter in Files, "Show all in Files" ends
  the first shelf. The file helpers take any shelf (`found_file`, `chosen_file`). INSIDE FILES
  (`file_inside.rs`): text files read where they are by `rg` in the watched folders (or the
  scope), PDFs and office files read ONCE into plain text in `~/.cache/waverunner/inside`
  (`refresh`: `pdftotext` 40 pages, `bsdtar` the document's XML, `index.tsv` key/mtime/path; off
  the loop, the thread at nice 19, again after 30 min) and `rg` reads them there; accents and
  case ignored (`pattern`), every word in the file (the longest first); asked 280 ms after the
  last letter, one reading at a time (`deep_inside_soon`), shown when the words are still those
  and the owner is not walking the results. Golem's home layer brings `rg`, `pdftotext`,
  `bsdtar` (ripgrep, poppler-utils, libarchive). Rig: link them into the rig's PATH (the toolbox
  has none); a fake home with real documents.
- `*_icon.rs` (whatsapp, youtube, ytmusic, discord, spotify, google, chrome, files, compass, gears, apps, bin) — the dock's MOVING ICONS, each directed by Max in the App Motion Lab. **`docs/icon-motion.md` is the spec**: parts and pivots, hover = held turn + one gesture, click = the event with a beat or counter-spin, the numbers, the vocabulary of moves, how to build and wire one, and how to generate the rest; **`docs/icon-list.md`** is the catalogue of 200 to animate, most used first, with dock ids and mark sources. Read both before touching or adding any.
- Engine (separate crate): `options-engine/src/{collectors,mind}` — the headless "Brain".
