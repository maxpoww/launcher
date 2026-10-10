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
  content is `open_content` (real: Scale, Resolution) or the placeholder rows.
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
  zebra); a scroll up or leaving the bar folds it.
- `desktop.rs` — the DESKTOP: `~/Desktop` as icons behind the windows (macOS/Windows style) on
  its own `Bottom`-layer surface (`surface::create_desktop_surface`; Bottom, not Background, so
  swww can never map over it) with its own renderer + icon array (layer = item index). Lists the
  folder (folders first, case-insensitive; `.desktop` files are launchers with their app icon)
  onto a cell `Grid` at the dock's icon scale (side margins equal and HALF of what centring leaves — `Grid::x0`; the rest of the spare width is shared between the columns, `Grid::pitch`): each file's cell is REMEMBERED in cell units
  (`desktop.json`, path → [col,row]; `place` + `stick`), new files take the first free cell
  column-major and move nothing, `forget` re-flows. Draws the Files-section tile
  (carrier/thumbnail + one-line name, fitted by MEASURED width with "…", white on a dark
  shadow); no hover magnification (Max). Click = select, DOUBLE click (400 ms) = open (`xdg-open` / the launcher's `Exec=`).
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
  + names it touches are selected, by path); a press on an unselected icon selects it alone;
  a drag of a selected icon takes the whole selection (`Drag.items`, grabbed one first;
  payload = all URIs; a drop on the desktop lands them keeping their arrangement via
  `place_group`, on the bin trashes them all); a click on wallpaper clears, opening clears.
  Selected items wear one soft wash (`sel_rect`, the mockup at ~/desktop-menu-mockup).
  Pointer-free: `waverunner-ctl debug-desktop [reload|open <n>|move <n> <col> <row>|import
  <col> <row> <uri…>|select <n…>|band <x0> <y0> <x1> <y1>|forget]`; the live daemon logs
  to `~/.local/state/waverunner/daemon.log`. HIDE/SHOW: a click on bare wallpaper toggles the
  icons (fade; `settings.desktop_hidden`). MENU (`desktop_menu.rs`, the approved mockup at
  ~/desktop-menu-mockup): right-click → `Menu::open` at the pointer (icon: Open · Open in
  terminal · Rename · ─ · Move to · Move to Home · Move to bin · ─ · Properties; wallpaper: [Paste ·] New folder · Clean up), drawn as a `GridContent`
  so it paints over the icons (names under it are skipped), a left press on a row acts on
  release, elsewhere closes; `debug-desktop menu [n]` / `pick <row>`. RENAME in place: the name
  becomes a field (`Rename`, all-selected first key replaces); the desktop takes the keyboard
  EXCLUSIVE for exactly that long (a Bottom layer gets it on hover with Exclusive, never via
  OnDemand without a click) and `begin_keyboard_handback(KbSurface::Desktop)` returns it;
  keys route first in `main.rs::handle_key_event` → `desktop_key`; a keyboard `leave` mid-name
  keeps what was typed. New folder = "untitled folder" at the clicked cell → rename. PROPERTIES
  (`desktop_props.rs`): the menu's LAST row, opened by HOVER — the menu becomes the box (the
  panel lerps from the menu's rect, `grow_from`; `Props.back` keeps the menu, the `‹ Back` row
  turns it back on hover or click, any other press closes). Lines: Kind, Contains (folders/files/hidden), Size (a
  folder's tree walked on a thread, `folder_size`), Dimensions (images), Link to, Where, Owner,
  Access, Modified, Created, Opened. `debug-desktop props <n>` opens it without a menu. Not yet:
  per-output surfaces, Escape closing the menu (no keyboard while it is up). MOVE TO: that row
  does not act, it turns the menu's PAGE (`Menu::show_targets`, the menu stays up; `‹ Back`, on hover or click →
  `show_main`): Cut · Copy (`App::serve_files` puts the selection on the clipboard as a file
  manager does, verb included) · the sticks and phones · the other computers. The lists are
  `desktop_send.rs`: sticks = volumes mounted under `/run/media/<user>` or `/media` (read from
  `/proc/mounts` at once; an unmounted stick is not listed) PLUS every volume on the desktop
  (`Desktop::volumes` — a phone goes to `phone_dest`: its first storage's Download), devices = KDE Connect's paired AND
  reachable ones (`kdeconnect-cli -a --id-name-only`, asked on a thread — "Looking for devices…"
  until it answers; a `desktop`/`laptop` type, read with `busctl`, goes in the second list).
  `Action::SendTo(n)` indexes `Desktop::targets`; `desktop_send::send` runs on a thread: a stick
  gets a MOVE (copy, then the original goes once it has arrived, then `sync -f`), a device a SEND
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
- Engine (separate crate): `options-engine/src/{collectors,mind}` — the headless "Brain".
