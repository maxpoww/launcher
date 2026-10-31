//! Background application discovery: desktop-entry scan plus icon
//! rasterization, run on the one background thread the daemon allows.
//!
//! The thread is long-lived and rescans on request (the daemon asks
//! whenever the dock is summoned), so newly installed apps appear and
//! uninstalled ones vanish without a daemon restart. Rasterized icons
//! are cached across rescans keyed by resolved file path — a nix
//! store path changes when the theme updates, invalidating naturally —
//! so a rescan costs only the `.desktop` parse plus new icons. The
//! rasters are also persisted under `$XDG_CACHE_HOME/waverunner`, so a
//! cold daemon start skips rasterization entirely for unchanged icon
//! files. Results are handed to the event loop over a calloop channel;
//! nothing here touches Wayland or wgpu.

use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::mpsc;

use calloop::channel::Sender;
use resvg::tiny_skia;
use tracing::{debug, info, warn};
use waverunner_core::index::{AppEntry, DesktopIndex};

/// Side length of every rasterized icon, in pixels. All icons are
/// normalized to this size so the renderer can pack them into one
/// texture array layer each.
///
/// Sized to cover the largest on-screen draw at the highest icon-size
/// level and render scale: grid level‑3 (`GRID_ICON × ICON_SCALES[3]`
/// ≈ 89 logical px) at render scale 2 ≈ 178 px, with headroom so even a
/// hover-magnified dock icon is downscaled, never upscaled. A power of
/// two so the mip chain halves cleanly. Icons are mostly SVG, so
/// rasterizing larger is free vector quality.
///
/// The small tier (`icons-128`, the weak machines' build) halves the side:
/// a quarter of every tile everywhere, and at a scale of 1 nothing on screen
/// meets more than 128 texels of an icon.
pub const ICON_SIZE: u32 = if cfg!(feature = "icons-128") { 128 } else { 256 };

/// Number of mip levels in the icon texture (base + downsamples), i.e.
/// `log2(ICON_SIZE) + 1`. The smallest level is 1×1.
pub const ICON_MIPS: u32 = ICON_SIZE.trailing_zeros() + 1;

/// Byte length of one icon's full mip chain (level 0 followed by every
/// downsample, premultiplied RGBA8): `ICON_SIZE² + (ICON_SIZE/2)² + … + 1`
/// texels × 4. Every producer emits this so the renderer uploads chains
/// uniformly and the on-disk cache stores them.
pub const ICON_CHAIN_BYTES: usize = icon_chain_bytes(ICON_SIZE);

/// The mip level a menu SYMBOL's chain begins at: drawn at 12–14 logical
/// px (28 physical at a 2× scale, 56 at the menus' 200 %), levels from
/// 128² serve every size the eye meets, and the 256² base — 262 KB of a
/// chain's 350 — was never sampled. Sixty symbols: 22 MB → 5.8 (2026-10-29).
pub(crate) const SYMBOL_MIP: u32 = 1;

/// A symbol's chain from [`SYMBOL_MIP`] on (a full chain's tail; a chain of
/// any other length as it is). The renderer writes a tail into the levels
/// it covers (`update_icon_layer` tells the length).
pub(crate) fn symbol_chain(mut chain: Vec<u8>) -> Vec<u8> {
    if chain.len() == ICON_CHAIN_BYTES {
        let keep = icon_chain_bytes(ICON_SIZE >> SYMBOL_MIP);
        chain.split_off(ICON_CHAIN_BYTES - keep)
    } else {
        chain
    }
}

pub(crate) const fn icon_chain_bytes(mut size: u32) -> usize {
    let mut total = 0usize;
    loop {
        total += (size * size * 4) as usize;
        if size == 1 {
            return total;
        }
        size /= 2;
    }
}

/// What an entry is, deciding which popup section shows it. Applications
/// come from `.desktop` files; files are home folders and file-search
/// results (opened with `xdg-open` rather than launched); assets are
/// invisible icon carriers (the generic folder/file icons that dynamic
/// search results borrow a texture layer from).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EntryKind {
    App,
    File,
    /// A nixpkgs package search result in the Install section
    /// (transient, not launchable; drag to Apps installs it).
    Package,
    /// An app-group cell in the Apps grid (transient; opens on click,
    /// renders a mini preview of its members).
    Group,
    Asset,
}

/// One indexed home file for type-to-search (not a full entry: search
/// results become transient entries borrowing an asset icon).
#[derive(Debug, Clone)]
pub struct FileEntry {
    /// File name (the fuzzy-match target).
    pub name: String,
    /// Absolute path.
    pub path: String,
    pub is_dir: bool,
    /// Its folder below home, as searched (`file_search::dir_of`): a word
    /// of the query may be found there.
    pub dir: String,
    /// When it changed last (unix seconds; 0 unknown).
    pub mtime: f64,
    /// Its size in bytes (0 for a folder).
    pub size: u64,
    /// Its name as searched (`file_search::fold`), made once at the walk.
    pub folded: String,
}

/// The indexer thread's result: entries plus one RGBA8 (premultiplied)
/// `ICON_SIZE`² image per entry, aligned by index.
pub struct LoadedApps {
    /// Discovered entries, sorted by name: applications first, then the
    /// home folders, then the icon assets.
    pub entries: Vec<AppEntry>,
    /// What each entry is, aligned with `entries`.
    pub kinds: Vec<EntryKind>,
    /// Premultiplied RGBA8 mip chain, [`ICON_CHAIN_BYTES`] per entry
    /// (base `ICON_SIZE`² plus downsamples); a generated placeholder tile
    /// where no icon was found.
    pub icons: Vec<Vec<u8>>,
    /// `true` for entries whose icon could not be resolved (placeholder tile).
    pub placeholders: Vec<bool>,
    /// Ids of the synthetic CLI-tool tiles ([`managed_cli_tiles`]) among the
    /// entries — the fabricated stand-ins for installed packages that ship no
    /// `.desktop`. The daemon excludes these when resolving a pending install
    /// to its real app, so a wrapped GUI package (`chromium` →
    /// `chromium-browser`) never latches onto its own `chromium` placeholder.
    pub cli_ids: Vec<String>,
    /// Content fingerprint of the load (entries, kinds, icon bytes,
    /// placeholders, cli ids), computed on the indexer thread.
    /// Most rescans (every dock summon requests one) reproduce the identical
    /// index; the event loop compares this against the previous load and
    /// skips the expensive rebuild — the full-array icon re-upload +
    /// label-cache clear used to land mid-open-animation as one long frame
    /// (SH: "box opening cuts at ~1/4").
    pub fingerprint: u64,
}

/// What the indexer sends: the apps as soon as their tiles are ready, the
/// home walk after them (the apps landed 0.4 s later for it on the 2013
/// MacBook Air, 2026-10-29).
pub enum IndexEvent {
    Apps(LoadedApps),
    /// Home-tree file index for search (fresh every rescan), and when each
    /// file was opened last, as the system's record says
    /// (`file_search::opened_from_record`; read with the walk).
    Files { files: Vec<FileEntry>, opened: crate::file_search::Opened },
}

/// Hash the parts of a load that would require the event loop to rebuild
/// state and re-upload textures. The home files are not in it — they come
/// in a message of their own, fresh every scan and cheap to swap in.
fn load_fingerprint(
    entries: &[AppEntry],
    kinds: &[EntryKind],
    icons: &[Vec<u8>],
    placeholders: &[bool],
    cli_ids: &[String],
) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut h = std::collections::hash_map::DefaultHasher::new();
    for e in entries {
        e.id.hash(&mut h);
        e.name.hash(&mut h);
        e.exec.hash(&mut h);
        e.description.hash(&mut h);
    }
    for k in kinds {
        std::mem::discriminant(k).hash(&mut h);
    }
    for i in icons {
        i.hash(&mut h);
    }
    placeholders.hash(&mut h);
    cli_ids.hash(&mut h);
    h.finish()
}

/// Handle to the long-lived indexer thread.
pub struct Indexer {
    /// `true` = flush stale negative icon-path cache entries before scanning
    /// (sent after a successful nix profile install so fresh icons are found).
    requests: mpsc::Sender<bool>,
}

impl Indexer {
    /// Ask for a rescan. Multiple queued requests coalesce into one
    /// scan; a dead indexer thread makes this a no-op.
    pub fn request_rescan(&self) {
        let _ = self.requests.send(false);
    }

    /// Ask for a rescan AND flush stale negative icon-path cache entries
    /// first, so a freshly-installed app's icon is found instead of using
    /// the cached "not found" result from before the install.
    pub fn request_rescan_fresh(&self) {
        let _ = self.requests.send(true);
    }
}

/// Append `~/.nix-profile/share` to `XDG_DATA_DIRS` when it is not already
/// present. Nix profile apps install their icons there; `freedesktop_icons`
/// needs it in `XDG_DATA_DIRS` to resolve them. Called once at startup so
/// both the indexer thread and the nix thread inherit the updated value.
pub fn patch_xdg_data_dirs() {
    let Ok(home) = std::env::var("HOME") else {
        return;
    };
    let nix_share = format!("{home}/.nix-profile/share");
    if !std::path::Path::new(&nix_share).exists() {
        return;
    }
    let current = std::env::var("XDG_DATA_DIRS").unwrap_or_default();
    if current.split(':').any(|d| d == nix_share) {
        return;
    }
    let patched = if current.is_empty() {
        nix_share
    } else {
        format!("{nix_share}:{current}")
    };
    std::env::set_var("XDG_DATA_DIRS", patched);
}

/// Spawn the indexer thread and queue the initial scan. The thread
/// exits when either channel closes (daemon shutdown).
pub fn spawn_indexer(icon_theme: String, results: Sender<IndexEvent>) -> Indexer {
    let (requests, rx) = mpsc::channel::<bool>();
    let spawned = std::thread::Builder::new()
        .name("waverunner-index".into())
        .spawn(move || {
            let mut loader = IconLoader::new(icon_theme);
            let mut first = true;
            // The parses of the last scan: only the files that changed are read.
            let scan_file = cache_base().join("waverunner").join("desktop-index.json");
            let mut scan_cache = waverunner_core::index::ScanCache::load(&scan_file);
            while let Ok(flush) = rx.recv() {
                // Coalesce any requests that queued up meanwhile; OR the
                // flush flags so a fresh-rescan request is never lost.
                let mut needs_flush = flush;
                while let Ok(f) = rx.try_recv() {
                    needs_flush |= f;
                }
                // After a nix profile install the icon path resolver may
                // hold stale "not found" entries for newly installed apps.
                // Flush them so the next lookup finds the installed icon.
                if needs_flush {
                    loader.resolutions.clear_negative();
                }

                let started = std::time::Instant::now();
                let (index, scan_changed) = DesktopIndex::scan_cached(&mut scan_cache);
                if scan_changed {
                    scan_cache.save(&scan_file);
                }
                let scanned = std::time::Instant::now();
                let mut entries = index.entries;
                let mut kinds = vec![EntryKind::App; entries.len()];
                // Installed CLI tools (fastfetch, htop, …) ship no
                // `.desktop`, so the scan above misses them. Synthesize a
                // tile for every waverunner-managed package not already
                // represented by an installed app, so it can be launched
                // (in a terminal) and dragged out to uninstall like any
                // app. Rebuilt on every rescan, so it tracks install/
                // uninstall.
                let t_cli = std::time::Instant::now();
                let cli = managed_cli_tiles(&entries);
                let t_cli = t_cli.elapsed();
                let cli_ids: Vec<String> = cli.iter().map(|e| e.id.clone()).collect();
                kinds.extend(std::iter::repeat_n(EntryKind::App, cli.len()));
                entries.extend(cli);
                entries.push(control_panel_tile());
                kinds.push(EntryKind::App);
                entries.push(modules_tile());
                kinds.push(EntryKind::App);
                entries.push(apps_grid_tile());
                kinds.push(EntryKind::App);
                let t_folders = std::time::Instant::now();
                let folders = home_folders();
                let t_folders = t_folders.elapsed();
                kinds.extend(std::iter::repeat_n(EntryKind::File, folders.len()));
                entries.extend(folders);
                for asset in icon_assets() {
                    entries.push(asset);
                    kinds.push(EntryKind::Asset);
                }
                let t_open = std::time::Instant::now();
                loader.disk.open_pack();
                let t_open = t_open.elapsed();
                let (icons, placeholders): (Vec<_>, Vec<_>) =
                    entries.iter().map(|entry| loader.icon_for(entry)).unzip();
                let t_save = std::time::Instant::now();
                loader.resolutions.save();
                let t_save = t_save.elapsed();
                let t_close = std::time::Instant::now();
                loader.disk.close_pack();
                let t_close = t_close.elapsed();
                // The apps go first; the home walk follows in its own message.
                let fingerprint =
                    load_fingerprint(&entries, &kinds, &icons, &placeholders, &cli_ids);
                let apps = LoadedApps {
                    entries,
                    kinds,
                    icons,
                    placeholders,
                    cli_ids,
                    fingerprint,
                };
                let n_entries = apps.entries.len();
                if results.send(IndexEvent::Apps(apps)).is_err() {
                    return; // event loop is gone
                }
                let walk = std::time::Instant::now();
                let files = scan_home_files();
                let opened = crate::file_search::opened_from_record();
                // (Said at info when it took its time: on the 2013 MacBook Air
                // the apps came 9.6 s after the dock, and nothing said why.)
                let took = started.elapsed();
                let (from_disk, rasterized, where_) = loader.take_counts();
                let line = format!(
                    "indexed {} entries + {} home files in {:?} (scan {:?}, icons {:?}: {from_disk} read, {rasterized} rasterized, {where_}, pack open {t_open:?} close {t_close:?}, cli {t_cli:?}, folders {t_folders:?}, save {t_save:?}, files {:?})",
                    n_entries,
                    files.len(),
                    took,
                    scanned - started,
                    walk - scanned,
                    walk.elapsed()
                );
                // (A run's first index is always said: it is the start's
                // measure on any machine.)
                if first || took > std::time::Duration::from_millis(1500) {
                    tracing::info!("{line}");
                } else {
                    debug!("{line}");
                }
                first = false;
                if results.send(IndexEvent::Files { files, opened }).is_err() {
                    return; // event loop is gone
                }
            }
        });
    if let Err(e) = spawned {
        warn!("cannot spawn indexer thread: {e}");
    }
    let indexer = Indexer { requests };
    indexer.request_rescan();
    indexer
}

/// The XDG application directories (`$XDG_DATA_HOME/applications` and each
/// `$XDG_DATA_DIRS/applications`) — where `.desktop` launchers live and the
/// desktop index scans.
fn application_dirs() -> Vec<std::path::PathBuf> {
    let mut dirs = Vec::new();
    if let Ok(home) = std::env::var("HOME") {
        let data_home =
            std::env::var("XDG_DATA_HOME").unwrap_or_else(|_| format!("{home}/.local/share"));
        dirs.push(std::path::PathBuf::from(format!(
            "{data_home}/applications"
        )));
    }
    let data_dirs = std::env::var("XDG_DATA_DIRS")
        .unwrap_or_else(|_| "/usr/local/share:/usr/share".to_string());
    for base in data_dirs.split(':').filter(|s| !s.is_empty()) {
        dirs.push(std::path::PathBuf::from(format!("{base}/applications")));
    }
    dirs
}

/// Start watching the application directories for `.desktop` changes and
/// return the inotify descriptor (non-blocking, level-triggered readable on
/// any change). Register it as a calloop source and drain it with
/// [`drain_inotify`]. Returns `None` if inotify is unavailable or no
/// application directory could be watched — live reload then simply stays off.
pub fn watch_application_dirs() -> Option<std::os::fd::OwnedFd> {
    watch_dirs(&application_dirs())
}

/// Start watching `dirs` for entries added, removed, renamed or rewritten,
/// and return the inotify descriptor (non-blocking, level-triggered readable
/// on any change). Register it as a calloop source and drain it with
/// [`drain_inotify`]. `None` if inotify is unavailable or none of the
/// directories could be watched.
pub fn watch_dirs(dirs: &[std::path::PathBuf]) -> Option<std::os::fd::OwnedFd> {
    use std::os::fd::FromRawFd;
    use std::os::unix::ffi::OsStrExt;
    // SAFETY: inotify_init1 returns a fresh fd (or -1); nothing else owns it.
    let fd = unsafe { libc::inotify_init1(libc::IN_NONBLOCK | libc::IN_CLOEXEC) };
    if fd < 0 {
        warn!("inotify_init1 failed; live reload disabled");
        return None;
    }
    // SAFETY: `fd` is a valid, freshly created descriptor we now take ownership of.
    let owned = unsafe { std::os::fd::OwnedFd::from_raw_fd(fd) };
    let mask = libc::IN_CREATE
        | libc::IN_DELETE
        | libc::IN_MOVED_TO
        | libc::IN_MOVED_FROM
        | libc::IN_CLOSE_WRITE;
    let mut watched = 0u32;
    for dir in dirs {
        let Ok(cpath) = std::ffi::CString::new(dir.as_os_str().as_bytes()) else {
            continue;
        };
        // SAFETY: `fd` is our inotify descriptor; `cpath` is a valid NUL-terminated path.
        let wd = unsafe { libc::inotify_add_watch(fd, cpath.as_ptr(), mask) };
        if wd >= 0 {
            watched += 1;
        }
    }
    if watched == 0 {
        return None;
    }
    Some(owned)
}

/// Drain all pending events from an inotify descriptor created by
/// [`watch_dirs`]. The event contents are irrelevant — any event means the
/// watched directories changed — so we only clear the queue so the
/// level-triggered source goes quiet until the next change.
pub fn drain_inotify(fd: std::os::fd::BorrowedFd<'_>) {
    use std::os::fd::AsRawFd;
    let raw = fd.as_raw_fd();
    let mut buf = [0u8; 8192];
    // SAFETY: reading into a stack buffer we own; `raw` is a valid fd.
    while unsafe { libc::read(raw, buf.as_mut_ptr().cast::<libc::c_void>(), buf.len()) } > 0 {}
}

/// Entry id of Golem's control panel tile (the gear). It is one of the
/// apps — gridded, pinnable, searchable — but a click opens the control
/// panel instead of launching anything (see `App::activate`). The id keeps
/// its first name: it is stored in the dock's pins.
/// The dock's fixed trio, left to right (Max, 2026-09-30): the Apps button,
/// the Recycle Bin and the control panel's gear. They close the pinned part
/// of the dock (the running and minimized zone follows, after the divider)
/// and can't be unpinned or moved (`PinDb` ignores both).
pub const DOCK_FIXED: [&str; 3] = [APPS_GRID_ID, "group:trash", CONTROL_PANEL_ID];

/// The fixed trio's own figures (Max's metal icon lab, 2026-10-01): cast
/// silver, drawn at 0.90 of the plate at 0.60 opacity. Built into the
/// binary, so every Golem shows the same three whatever icon theme it has;
/// the dock still draws the plate under them. Regenerate from the lab with
/// `assets/dock/lab/gen-icons.js` if the design changes.
const DOCK_FIGURES: [(&str, &[u8]); 3] = [
    (APPS_GRID_ID, include_bytes!("../assets/dock/apps.svg")),
    ("group:trash", include_bytes!("../assets/dock/bin-body.svg")),
    (CONTROL_PANEL_ID, include_bytes!("../assets/dock/control.svg")),
];

/// The Recycle Bin's lid, drawn apart from its can so it can rise under a
/// drag (the can is `DOCK_FIGURES`' "group:trash").
pub(crate) fn bin_lid_figure() -> Option<Vec<u8>> {
    Some(with_mips(rasterize_svg(&crate::look::figure(include_bytes!("../assets/dock/bin-lid.svg")))?.take()))
}


/// The control panel's gears turned by `angle`, ready to upload: the drawn
/// frame nearest it, each pixel doubled to the icon size (the mips below
/// are the frame itself), or `None` while it is not drawn yet.
pub(crate) fn gears_turned(angle: f32) -> Option<Vec<u8>> {
    Some(frame_chain(&crate::gears_icon::frame(angle)?, crate::gears_icon::FRAME))
}

/// A drawn frame of `frame` px square, each pixel doubled to the icon size
/// (the mips below are the frame itself).
fn frame_chain(small: &[u8], frame: u32) -> Vec<u8> {
    let (n, k) = (frame as usize, (ICON_SIZE / frame) as usize);
    let side = n * k;
    let mut base = vec![0u8; side * side * 4];
    for y in 0..side {
        for x in 0..side {
            let (s, d) = (((y / k) * n + x / k) * 4, (y * side + x) * 4);
            base[d..d + 4].copy_from_slice(&small[s..s + 4]);
        }
    }
    with_mips(base)
}


/// Seam's compass with its needle and its plate where they are, ready to
/// upload, once both are drawn.
pub(crate) fn compass_at(needle: f32, plate: f32) -> Option<Vec<u8>> {
    Some(frame_chain(&crate::compass_icon::frame(needle, plate)?, crate::compass_icon::FRAME))
}

/// WhatsApp's mark with its plate and its phone where they are, ready to
/// upload, once both are drawn.
pub(crate) fn whatsapp_at(plate: f32, phone: f32) -> Option<Vec<u8>> {
    Some(frame_chain(&crate::whatsapp_icon::frame(plate, phone)?, crate::whatsapp_icon::FRAME))
}

/// YouTube's mark in pose `p`, ready to upload.
pub(crate) fn youtube_at(p: &crate::youtube_icon::Pose) -> Option<Vec<u8>> {
    Some(frame_chain(&crate::youtube_icon::frame(p)?, crate::youtube_icon::FRAME))
}

/// YouTube Music's mark in pose `p`, ready to upload.
pub(crate) fn ytmusic_at(p: &crate::ytmusic_icon::Pose) -> Option<Vec<u8>> {
    Some(frame_chain(&crate::ytmusic_icon::frame(p)?, crate::ytmusic_icon::FRAME))
}

/// Discord's mark in pose `p`, ready to upload.
pub(crate) fn discord_at(p: &crate::discord_icon::Pose) -> Option<Vec<u8>> {
    Some(frame_chain(&crate::discord_icon::frame(p)?, crate::discord_icon::FRAME))
}

/// Spotify's mark in pose `p`, ready to upload.
pub(crate) fn spotify_at(p: &crate::spotify_icon::Pose) -> Option<Vec<u8>> {
    Some(frame_chain(&crate::spotify_icon::frame(p)?, crate::spotify_icon::FRAME))
}

/// Google's mark in pose `p`, ready to upload.
pub(crate) fn google_at(p: &crate::google_icon::Pose) -> Option<Vec<u8>> {
    Some(frame_chain(&crate::google_icon::frame(p)?, crate::google_icon::FRAME))
}

/// Chrome's mark with its lens `s` times its size and spun `spin`, ready to
/// upload.
pub(crate) fn chrome_at(s: f32, spin: f32) -> Option<Vec<u8>> {
    Some(frame_chain(&crate::chrome_icon::frame(s, spin)?, crate::chrome_icon::FRAME))
}

/// The apps button's stencil (its nine squares), ready to upload.
pub(crate) fn apps_stencil() -> Option<Vec<u8>> {
    Some(with_mips(rasterize_svg(crate::apps_icon::stencil_svg().as_bytes())?.take()))
}

/// The apps button with its squares `pages` to the left and `down` of a
/// drop down, ready to upload, once that frame is drawn.
pub(crate) fn apps_moved(pages: f32, down: f32) -> Option<Vec<u8>> {
    Some(frame_chain(&crate::apps_icon::frame(pages, down)?, crate::apps_icon::FRAME))
}

/// Golem Files' launcher figures, ready to upload (`files_icon::figures`).
pub(crate) fn files_figures() -> Option<Vec<Vec<u8>>> {
    crate::files_icon::figures()
        .iter()
        .map(|svg| Some(with_mips(rasterize_svg(svg)?.take())))
        .collect()
}

/// The ready-to-upload tile of a fixed-tail figure, by entry id.
/// The dock's own figures — the bin and its lid, the apps button's stencil,
/// Golem Files' folders, the actions' symbols — rasterised from SVG. Made
/// as the index landed, they held the loop 145 ms (2026-10-29, the shell
/// stress: the index lands right as the shell comes up). Now a set is made
/// on a thread at start for the look of that moment; the landing takes it
/// if the look is still that one, and otherwise reserves blank layers and
/// has the set made off the loop, filling the layers when it comes
/// (`App::figures_fill`) — the icons' tone is settled by the palette a
/// little after the shell starts, so the set made ahead is not always the
/// right one.
pub(crate) struct Figures {
    pub dark: bool,
    pub icons_dark: bool,
    pub bin: Option<(Vec<u8>, Vec<u8>)>,
    pub stencil: Option<Vec<u8>>,
    pub files: Option<Vec<Vec<u8>>>,
    pub actions: Option<Vec<Vec<u8>>>,
}

impl Figures {
    /// Blank chains, one per figure the real set will have (its layers).
    pub(crate) fn blank() -> Self {
        let blank = || vec![0u8; ICON_CHAIN_BYTES];
        Self {
            dark: crate::look::dark(),
            icons_dark: crate::look::icons_dark(),
            bin: Some((blank(), blank())),
            stencil: Some(blank()),
            files: Some((0..crate::files_icon::figures().len()).map(|_| blank()).collect()),
            actions: Some((0..crate::app_actions::figure_count()).map(|_| blank()).collect()),
        }
    }
}

static PREWARMED_FIGURES: std::sync::Mutex<Option<Figures>> = std::sync::Mutex::new(None);

/// The set for the present look, rasterised now (off the loop, please).
pub(crate) fn make_figures() -> Figures {
    Figures {
        dark: crate::look::dark(),
        icons_dark: crate::look::icons_dark(),
        bin: dock_figure("group:trash").zip(bin_lid_figure()),
        stencil: apps_stencil(),
        files: files_figures(),
        actions: crate::app_actions::figures(),
    }
}

/// Make the figures off the loop, ahead of the index.
pub(crate) fn prewarm_figures() {
    // (`WAVERUNNER_NO_FIGURES_AHEAD=1`: the way to measure what it costs.)
    if std::env::var_os("WAVERUNNER_NO_FIGURES_AHEAD").is_some() {
        return;
    }
    let spawned = std::thread::Builder::new().name("figures".into()).spawn(|| {
        let made = make_figures();
        if let Ok(mut slot) = PREWARMED_FIGURES.lock() {
            *slot = Some(made);
        }
    });
    if let Err(e) = spawned {
        tracing::warn!("apps: no thread for the dock's figures ({e})");
    }
}

/// The figures made ahead, if they are for the present look.
pub(crate) fn take_figures() -> Option<Figures> {
    let (dark, icons_dark) = (crate::look::dark(), crate::look::icons_dark());
    let mut slot = PREWARMED_FIGURES.lock().ok()?;
    match slot.take() {
        Some(f) if f.dark == dark && f.icons_dark == icons_dark => Some(f),
        _ => None,
    }
}

/// The SVG a fixed tile is drawn from (the control panel's gears, the apps
/// grid's, the brand marks at rest, the dock's figures), or `None` for an
/// ordinary app. The tile itself is [`dock_figure`]; the indexer keeps it
/// in the disk cache by what the SVG holds (a dozen of these were drawn
/// at every index: 300 ms on the dev box, over a second on the 2013
/// MacBook Air, 2026-10-29).
pub(crate) fn dock_figure_svg(id: &str) -> Option<Vec<u8>> {
    // (The gears' turning frames and the apps button's are drawn meanwhile,
    // off the loop.)
    if id == CONTROL_PANEL_ID {
        crate::gears_icon::warm();
        return Some(crate::gears_icon::svg(0.0).into_bytes());
    }
    if id == APPS_GRID_ID {
        crate::apps_icon::warm();
        return Some(crate::apps_icon::svg(0.0, 0.0).into_bytes());
    }
    // Seam wears the compass (`compass_icon`), its frames drawn meanwhile.
    if id == crate::compass_icon::SEAM_ID {
        crate::compass_icon::warm();
        return Some(crate::compass_icon::rest_svg().into_bytes());
    }
    // WhatsApp wears its moving mark (`whatsapp_icon`), frames drawn meanwhile.
    if id == crate::whatsapp_icon::WHATSAPP_ID {
        crate::whatsapp_icon::warm();
        return Some(crate::whatsapp_icon::rest_svg().into_bytes());
    }
    if id == crate::youtube_icon::YOUTUBE_ID {
        return Some(crate::youtube_icon::rest_svg().into_bytes());
    }
    if id == crate::ytmusic_icon::YTMUSIC_ID {
        return Some(crate::ytmusic_icon::rest_svg().into_bytes());
    }
    if id == crate::discord_icon::DISCORD_ID {
        return Some(crate::discord_icon::rest_svg().into_bytes());
    }
    if id == crate::spotify_icon::SPOTIFY_ID {
        return Some(crate::spotify_icon::rest_svg().into_bytes());
    }
    if id == crate::google_icon::GOOGLE_ID {
        return Some(crate::google_icon::rest_svg().into_bytes());
    }
    if id == crate::chrome_icon::CHROME_ID {
        return Some(crate::chrome_icon::rest_svg().into_bytes());
    }
    let &(_, svg) = DOCK_FIGURES.iter().find(|(fid, _)| *fid == id)?;
    // (Silver, or graphite in the dark: `look::figure`.)
    Some(crate::look::figure(svg).into_owned())
}

pub(crate) fn dock_figure(id: &str) -> Option<Vec<u8>> {
    let svg = dock_figure_svg(id)?;
    Some(with_mips(rasterize_svg(&svg)?.take()))
}

/// Whether `id` is one of the dock's fixed trio ([`DOCK_FIXED`]).
pub fn is_dock_fixed(id: &str) -> bool {
    DOCK_FIXED.contains(&id)
}

pub(crate) const CONTROL_PANEL_ID: &str = "golem-settings";

/// The control panel's tile. Synthesized, like the home folders: the
/// panel is part of waverunner, so no `.desktop` file stands behind it.
fn control_panel_tile() -> AppEntry {
    AppEntry {
        id: CONTROL_PANEL_ID.to_owned(),
        name: crate::i18n::tr("Control panel").to_owned(),
        description: Some(crate::i18n::tr("Configure Golem").to_owned()),
        exec: String::new(),
        icon: Some("org.gnome.Settings".to_owned()),
        startup_wm_class: None,
        needs_terminal: false,
        path: None,
    }
}

/// Entry id of the MODULES tile: an app among the apps whose click opens
/// the card as the modules' field (see `App::toggle_modules`).
pub(crate) const MODULES_ID: &str = "golem-modules";

/// The Modules tile. Synthesized like [`control_panel_tile`].
fn modules_tile() -> AppEntry {
    AppEntry {
        id: MODULES_ID.to_owned(),
        name: crate::i18n::tr("Modules").to_owned(),
        description: Some(crate::i18n::tr("Add a whole setup: software, tuned").to_owned()),
        exec: String::new(),
        icon: Some("org.gnome.Extensions".to_owned()),
        startup_wm_class: None,
        needs_terminal: false,
        path: None,
    }
}

/// Entry id of the Apps button (macOS's Launchpad): a click opens the card on
/// the apps grid, or closes it (see `App::toggle_apps_grid`).
pub(crate) const APPS_GRID_ID: &str = "golem-apps";

/// The Apps button's tile. Synthesized like [`control_panel_tile`]; it lives in the
/// grid too, so an unpinned button can always be dragged back to the dock.
fn apps_grid_tile() -> AppEntry {
    AppEntry {
        id: APPS_GRID_ID.to_owned(),
        name: crate::i18n::tr("Apps").to_owned(),
        description: Some(crate::i18n::tr("Show all apps").to_owned()),
        exec: String::new(),
        icon: Some("app-launcher".to_owned()),
        startup_wm_class: None,
        needs_terminal: false,
        path: None,
    }
}

/// Every visible top-level folder in the home directory, alphabetical,
/// as entries opened with `xdg-open` — the unfiltered Files strip.
/// (The daemon's usage sort then puts the most-opened ones first.)
fn home_folders() -> Vec<AppEntry> {
    let Ok(home) = std::env::var("HOME") else {
        return Vec::new();
    };
    let Ok(read) = std::fs::read_dir(&home) else {
        return Vec::new();
    };
    let mut folders: Vec<AppEntry> = read
        .flatten()
        .filter_map(|e| {
            let name = e.file_name().to_string_lossy().into_owned();
            if name.starts_with('.') || !e.file_type().ok()?.is_dir() {
                return None;
            }
            let path = format!("{home}/{name}");
            Some(AppEntry {
                id: format!("folder-{name}"),
                name,
                description: Some(path.clone()),
                exec: format!("xdg-open {}", crate::launch::shell_quote(&path)),
                icon: Some("folder".to_owned()),
                startup_wm_class: None,
                needs_terminal: false,
                path: None,
            })
        })
        .collect();
    folders.sort_by(|a, b| a.name.cmp(&b.name));
    folders
}

/// The invisible icon-carrier asset set: (asset id, themed icon name).
/// File entries pick one by extension (see `files::file_asset_name`).
pub(crate) const ICON_ASSETS: [(&str, &str); 10] = [
    ("asset-folder", "folder"),
    ("asset-file", "text-x-generic"),
    ("asset-pkg", "package-x-generic"),
    ("asset-audio", "audio-x-generic"),
    ("asset-video", "video-x-generic"),
    ("asset-image", "image-x-generic"),
    ("asset-pdf", "application-pdf"),
    ("asset-archive", "application-x-archive"),
    ("asset-doc", "x-office-document"),
    ("asset-code", "text-x-script"),
];

/// Invisible icon-carrier entries: dynamic file-search results borrow
/// these texture layers (one per keystroke can't rasterize new icons).
/// Empty names keep them out of every fuzzy match.
fn icon_assets() -> Vec<AppEntry> {
    ICON_ASSETS
        .into_iter()
        .map(|(id, icon)| AppEntry {
            id: id.to_owned(),
            name: String::new(),
            description: None,
            exec: "true".to_owned(),
            icon: Some(icon.to_owned()),
            startup_wm_class: None,
            needs_terminal: false,
            path: None,
        })
        .collect()
}

/// Synthetic Apps-grid tiles for waverunner-managed packages that ship no
/// installed `.desktop` — CLI tools like fastfetch. Without these an
/// installed CLI tool would be invisible (nothing to click, nothing to
/// drag out to uninstall). A tile carries the attr as its id (so
/// removable/uninstall detection maps straight back to the managed list)
/// and the generic package icon. Launching opens a terminal on the
/// user's shell behind the same Golem banner as the "try it" nix
/// shell — the tool is already on PATH (home-manager installed it), and
/// the banner names the command to type. Skips any package a real
/// `.desktop` already covers (`apps` = the scanned entries).
fn managed_cli_tiles(apps: &[AppEntry]) -> Vec<AppEntry> {
    cli_tiles_from(crate::managed::snapshot(), apps)
}

/// Pure core of [`managed_cli_tiles`] (snapshot passed in, so it's
/// testable): decide which managed packages earn a synthetic CLI tile and
/// build them. Split out because the shipped bug lived in the FILTER —
/// which packages get a fabricated tile — not in the scan plumbing.
fn cli_tiles_from(
    snapshot: Vec<(String, Vec<String>, Option<bool>)>,
    apps: &[AppEntry],
) -> Vec<AppEntry> {
    let app_ids: HashSet<&str> = apps.iter().map(|e| e.id.as_str()).collect();
    snapshot
        .into_iter()
        // A known GUI app (`gui == Some(true)`) never gets a synthetic
        // terminal tile — not even in the window right after install,
        // before its `.desktop` reaches the scan. That fabrication used to
        // be pinned in place of the real app (a green box that opened a
        // shell) and never healed until a relogin. CLI tools
        // (`Some(false)`) and legacy entries (`None`) still get a tile when
        // no scanned `.desktop` already covers them.
        .filter(|(_, desktop_ids, gui)| {
            *gui != Some(true) && !desktop_ids.iter().any(|d| app_ids.contains(d.as_str()))
        })
        .map(|(attr, _, _)| AppEntry {
            id: attr.clone(),
            name: attr.clone(),
            description: Some(crate::i18n::tr("Command-line tool").to_owned()),
            // attr ≈ the package's main program (the banner's "run:").
            exec: format!(
                "{} exec \"${{SHELL:-bash}}\"",
                crate::launch::banner_cmd(&attr, None, &cli_commands(&attr))
            ),
            icon: Some("package-x-generic".to_owned()),
            startup_wm_class: None,
            needs_terminal: true,
            path: None,
        })
        .collect()
}

/// Cap on the home-file index: keeps per-keystroke ranking and memory
/// bounded. Breadth-first order means shallow (more relevant) paths
/// survive the cut when a huge tree hits the cap.
const FILE_INDEX_CAP: usize = 50_000;
/// Directory depth limit of the home walk (home itself = depth 0).
const FILE_WALK_DEPTH: usize = 6;
/// Build/VCS trees indexed by no one on purpose — and any folder that says it
/// is a cache (`CACHEDIR.TAG`: Cargo's `target-*` trees flooded file search
/// with build output).
const FILE_WALK_SKIP: [&str; 3] = ["target", "node_modules", "__pycache__"];
/// Golem's own folders at the home's top — the system's checkout and where
/// the nightly update sets an owner's edits to it aside — are not what one
/// looks for (the lab laptops: "golem", "home", "system", "power" brought
/// two hundred of their files); Files still shows them.
const GOLEM_OWN: [&str; 2] = ["Golem", "Golem-local-edits"];

/// Whether a thing the home walk meets is left out of file search: hidden,
/// a build tree, a folder that says it is a cache (`file_search::list_top`
/// leaves out the same).
pub(crate) fn walk_skips(name: &str, path: &std::path::Path, is_dir: bool) -> bool {
    name.starts_with('.') || FILE_WALK_SKIP.contains(&name) || (is_dir && path.join("CACHEDIR.TAG").exists())
}

/// Walk the home directory (breadth-first, hidden entries and symlinks
/// skipped) into the search index. Runs on the indexer thread on every
/// rescan, so results track the disk.
pub(crate) fn scan_home_files() -> Vec<FileEntry> {
    let Ok(home) = std::env::var("HOME") else {
        return Vec::new();
    };
    let home_path = PathBuf::from(&home);
    let mut out = Vec::new();
    let mut queue = std::collections::VecDeque::from([(PathBuf::from(&home), 0usize)]);
    while let Some((dir, depth)) = queue.pop_front() {
        let Ok(read) = std::fs::read_dir(&dir) else {
            continue;
        };
        // (Every file of a folder is in that folder: its words once.)
        let folder = crate::file_search::dir_of(&dir.join("x"), &home_path);
        for e in read.flatten() {
            let name = e.file_name().to_string_lossy().into_owned();
            let Ok(ft) = e.file_type() else { continue };
            if ft.is_symlink() {
                continue;
            }
            let is_dir = ft.is_dir();
            let path = e.path();
            if walk_skips(&name, &path, is_dir) || (depth == 0 && is_dir && GOLEM_OWN.contains(&name.as_str())) {
                continue;
            }
            if is_dir && depth + 1 < FILE_WALK_DEPTH {
                queue.push_back((path.clone(), depth + 1));
            }
            let meta = e.metadata().ok();
            let mtime = meta.as_ref().map_or(0.0, crate::file_search::mtime_of);
            let size = meta.as_ref().filter(|_| !is_dir).map_or(0, std::fs::Metadata::len);
            let folded = crate::file_search::fold(&name);
            out.push(FileEntry {
                name,
                path: path.to_string_lossy().into_owned(),
                is_dir,
                dir: folder.clone(),
                mtime,
                size,
                folded,
            });
            if out.len() >= FILE_INDEX_CAP {
                return out;
            }
        }
    }
    out
}

/// Turns an `AppEntry` into `ICON_SIZE`² pixels, cheapest source first:
/// in-memory rasters (the tiles the disk cache does not hold), on-disk
/// rasters (cold start and rescans), then rasterizing the icon file. Icon-name → file-path
/// resolutions get their own persistent cache because the theme
/// directory walk — not rasterization — dominates index time.
/// Tiles the loader keeps in memory of those not on disk (see `icon_for`).
const RASTERS_KEPT: usize = 12;

pub(crate) struct IconLoader {
    /// The theme the config names; which tone of it is walked follows the
    /// look (`look::icon_theme`).
    configured: String,
    /// The look's generation this loader was built for: when it moves, the
    /// loader builds itself again for the new tone (see [`Self::sync`]).
    look: u32,
    /// Theme fallback chain, best first (see [`theme_chain`]).
    themes: Vec<String>,
    rasters: HashMap<String, (Vec<u8>, bool)>,
    /// Tiles read from the disk cache and rasterized since `take_counts`,
    /// and where the time went (resolving the name, naming the tile,
    /// reading it).
    loaded: usize,
    rasterized: usize,
    t_resolve: std::time::Duration,
    t_key: std::time::Duration,
    t_load: std::time::Duration,
    t_placeholder: std::time::Duration,
    placeholders: usize,
    disk: DiskCache,
    resolutions: ResolutionCache,
}

impl IconLoader {
    pub(crate) fn new(theme: String) -> Self {
        let look = crate::look::generation();
        let themes = theme_chain(&crate::look::icon_theme(&theme));
        Self {
            resolutions: ResolutionCache::load(&themes.join("+")),
            rasters: HashMap::new(),
            loaded: 0,
            rasterized: 0,
            t_resolve: std::time::Duration::ZERO,
            t_key: std::time::Duration::ZERO,
            t_load: std::time::Duration::ZERO,
            t_placeholder: std::time::Duration::ZERO,
            placeholders: 0,
            disk: DiskCache::new(),
            configured: theme,
            look,
            themes,
        }
    }

    /// The look changed since this loader was built: build it again, so what
    /// it resolves and rasterizes from now on is in the new tone (the old
    /// tone's resolutions are dropped with it).
    pub(crate) fn sync(&mut self) {
        if self.look != crate::look::generation() {
            self.resolutions.save();
            *self = Self::new(std::mem::take(&mut self.configured));
        }
    }

    /// The theme fallback chain this loader searches, best first.
    pub(crate) fn themes(&self) -> &[String] {
        &self.themes
    }

    /// Persist newly learned icon-name resolutions (no-op when clean).
    /// The nix thread calls this after each ranked batch; concurrent
    /// saves with the indexer thread are benign (atomic whole-file
    /// writes of the same mapping, last one wins).
    pub(crate) fn save_resolutions(&mut self) {
        self.resolutions.save();
    }

    /// The entry's icon as `(pixels, is_placeholder)`.
    pub(crate) fn icon_for(&mut self, entry: &AppEntry) -> (Vec<u8>, bool) {
        self.sync();
        // The fixed tiles wear their built-in figure, sized as designed (not
        // re-fitted to the 80% every theme icon is normalized to): kept on
        // disk by what the SVG holds, like any tile.
        if let Some(svg) = dock_figure_svg(&entry.id) {
            let key = format!("figure|{:016x}|{}", fnv1a64_bytes(&svg), svg.len());
            let file = self.disk.dir.join(format!("{:016x}", fnv1a64(&key)));
            let t = std::time::Instant::now();
            if let Some(chain) = self.disk.load(&file) {
                self.loaded += 1;
                self.t_load += t.elapsed();
                return (chain, false);
            }
            if let Some(chain) = rasterize_svg(&svg).map(|p| with_mips(p.take())) {
                self.rasterized += 1;
                self.disk.store(&file, &chain);
                return (chain, false);
            }
        }
        // `asset-*` carriers are keyed by their id (not the resolved path):
        // they render unplated while an app using the same themed icon stays
        // plated, so the two must not share a cache slot.
        let is_asset = entry.id.starts_with("asset-");
        let t0 = std::time::Instant::now();
        let (key, path) = match self.resolve(entry) {
            Some(path) if is_asset => (entry.id.clone(), Some(path)),
            Some(path) => (path.clone(), Some(path)),
            None => (format!("placeholder:{}", entry.name), None),
        };
        if let Some((pixels, is_placeholder)) = self.rasters.get(&key) {
            return (pixels.clone(), *is_placeholder);
        }
        // Placeholder tiles are cheap to regenerate and never touch disk;
        // real icons go through the persistent cache, the unplated asset
        // carriers under a tag of their own (they skipped it as "cheap":
        // on the 2013 MacBook Air the assets were 2.7 s of every start's
        // index, 2026-10-29).
        self.t_resolve += t0.elapsed();
        let t1 = std::time::Instant::now();
        let tag = if is_asset { "unplated" } else { "" };
        let mut cache_file = path.as_deref().and_then(|p| self.disk.file_for(p, tag));
        let (key, path) = if cache_file.is_none() && path.is_some() {
            // The cached path is gone (a theme update): the name is walked
            // again, once.
            if let Some(name) = entry.icon.as_deref() {
                self.resolutions.forget(name);
            }
            let again = match self.resolve(entry) {
                Some(path) if is_asset => (entry.id.clone(), Some(path)),
                Some(path) => (path.clone(), Some(path)),
                None => (format!("placeholder:{}", entry.name), None),
            };
            cache_file = again.1.as_deref().and_then(|p| self.disk.file_for(p, tag));
            again
        } else {
            (key, path)
        };
        self.t_key += t1.elapsed();
        let t2 = std::time::Instant::now();
        // `on_disk`: the persistent cache holds this tile, so a rescan reads
        // it back from there (one 349 KB file, usually still in the page
        // cache) and it is NOT kept in memory as well. Every tile used to
        // stay in `rasters` for the life of the daemon — ~37 MB for 108
        // apps on the Acer, growing with every app and every package search
        // (night audit, 2026-10-04).
        let (pixels, is_placeholder, on_disk) =
            match cache_file.as_deref().and_then(|f| self.disk.load(f)) {
                Some(pixels) => {
                    self.loaded += 1;
                    self.t_load += t2.elapsed();
                    (pixels, false, true)
                }
                None => match path.and_then(|p| rasterize_icon_file(&p, &entry.id)) {
                    Some(pixels) => {
                        self.rasterized += 1;
                        let stored = cache_file
                            .as_deref()
                            .is_some_and(|file| self.disk.store(file, &pixels));
                        (pixels, false, stored)
                    }
                    None => {
                        let t3 = std::time::Instant::now();
                        let tile = placeholder_icon(&entry.name);
                        self.t_placeholder += t3.elapsed();
                        self.placeholders += 1;
                        (tile, true, false)
                    }
                },
            };
        if !on_disk {
            // (A few, for a burst of the same: the unplated carriers and
            // the menus' symbols are asked for by the surfaces that keep
            // them, so the loader keeping them all was 21 MB a resolver.)
            if self.rasters.len() >= RASTERS_KEPT {
                self.rasters.clear();
            }
            self.rasters.insert(key, (pixels.clone(), is_placeholder));
        }
        (pixels, is_placeholder)
    }

    /// How many tiles came from the disk cache and how many were rasterized
    /// since the last call.
    pub(crate) fn take_counts(&mut self) -> (usize, usize, String) {
        let where_ = format!(
            "resolve {:?} ({} walked), name {:?}, read {:?}, {} placeholders {:?}",
            std::mem::take(&mut self.t_resolve),
            self.resolutions.take_walked(),
            std::mem::take(&mut self.t_key),
            std::mem::take(&mut self.t_load),
            std::mem::take(&mut self.placeholders),
            std::mem::take(&mut self.t_placeholder)
        );
        (std::mem::take(&mut self.loaded), std::mem::take(&mut self.rasterized), where_)
    }

    /// Resolve an entry's icon name to a file path (absolute names pass
    /// through; the rest go via the theme lookup and its cache).
    fn resolve(&mut self, entry: &AppEntry) -> Option<String> {
        let name = entry.icon.as_deref()?;
        if name.starts_with('/') {
            return Some(name.to_owned());
        }
        self.resolutions.resolve(name, &self.themes)
    }
}

/// Persistent raster cache: one raw premultiplied-RGBA8 file per source
/// icon under `$XDG_CACHE_HOME/waverunner/icons-<SIZE>/` (falling back
/// to `~/.cache/...`), named by a hash of the icon's path, size, and
/// mtime. An edited icon file re-rasterizes via the mtime, and nix
/// store path churn (theme updates) invalidates via the path; stale
/// entries are tiny (`ICON_SIZE`² × 4 bytes) and simply linger.
struct DiskCache {
    dir: PathBuf,
    /// An icon file's content hash and length as last read, by its REAL
    /// path (`hashes.tsv`; a profile path keeps its name while the store
    /// file behind it changes): a store file is immutable, its bytes are
    /// never read again; any other file is trusted while its length and
    /// mtime stand (one stat, not a read: 70 webapp PNGs read cold were
    /// 127 ms of a MacBook start, 2026-10-29).
    hashes: HashMap<String, Known>,
    /// The same by the path the entry NAMES, good while the icon
    /// directories are what they were (`dirs_fence`): no canonicalize —
    /// 125 of them on a cold disk were 179 ms of a MacBook start
    /// (2026-10-29). `mtime` 0: the real file is in the store.
    by_given: HashMap<String, Known>,
    given_fence: String,
    hashes_dirty: bool,
    /// THE PACK (`pack.bin`): every tile of the last index in one file, read
    /// once at a start — 115 tiles read one file each from a cold SSD were
    /// 2.2 s on the 2013 MacBook Air (2026-10-29). Held only between
    /// `open_pack` and `close_pack` (the indexer's pass), by tile name.
    pack: Option<Pack>,
    /// The tiles this pass served or stored, for the next pack.
    used: Vec<PathBuf>,
}

/// The tiles of the last index in one file: a header of names and places,
/// then the chains.
struct Pack {
    bytes: Vec<u8>,
    at: HashMap<u64, (usize, usize)>,
}

/// What an icon file held when it was last read, and when (`mtime` 0: a
/// store file, immutable).
#[derive(Clone, Copy)]
struct Known {
    hash: u64,
    len: usize,
    mtime: u64,
}

const PACK_MAGIC: &[u8; 4] = b"GTP1";

impl Pack {
    fn open(file: &std::path::Path) -> Option<Self> {
        let bytes = std::fs::read(file).ok()?;
        if bytes.len() < 8 || &bytes[..4] != PACK_MAGIC {
            return None;
        }
        let count = u32::from_le_bytes(bytes[4..8].try_into().ok()?) as usize;
        let head = 8 + count * 20;
        if bytes.len() < head {
            return None;
        }
        let mut at = HashMap::with_capacity(count);
        for i in 0..count {
            let e = 8 + i * 20;
            let name = u64::from_le_bytes(bytes[e..e + 8].try_into().ok()?);
            let offset = u64::from_le_bytes(bytes[e + 8..e + 16].try_into().ok()?) as usize;
            let len = u32::from_le_bytes(bytes[e + 16..e + 20].try_into().ok()?) as usize;
            if offset.checked_add(len).is_none_or(|end| end > bytes.len()) {
                return None;
            }
            at.insert(name, (offset, len));
        }
        Some(Self { bytes, at })
    }

    fn get(&self, name: u64) -> Option<&[u8]> {
        self.at.get(&name).map(|&(offset, len)| &self.bytes[offset..offset + len])
    }
}

/// A tile file's name as a number (the hex of its key's hash).
fn tile_name(file: &std::path::Path) -> Option<u64> {
    u64::from_str_radix(file.file_name()?.to_str()?, 16).ok()
}

impl DiskCache {
    fn new() -> Self {
        // The cache stores finished tiles (normalized + mipped), so its
        // identity must track that processing: the directory carries a hash
        // of every normalize parameter, so any tuning lands in a fresh
        // directory instead of serving stale tiles (and `raw` when plating
        // is off). The `live` marker parts this cache from the old one that
        // baked the plate pixels in (the plate is shader-drawn now).
        let variant = if icon_plate_enabled() {
            let params = format!("live|{PLATE_CONTENT}|{PLATE_ICON_DESAT}|{PLATE_ICON_OPACITY}");
            format!("plate-{:08x}", fnv1a64(&params))
        } else {
            "raw".to_owned()
        };
        Self::at(
            cache_base()
                .join("waverunner")
                .join(format!("icons-{ICON_SIZE}-{variant}")),
        )
    }

    /// The cache in `dir`, its remembered hashes read.
    fn at(dir: PathBuf) -> Self {
        let mut hashes = HashMap::new();
        let mut by_given = HashMap::new();
        let given_fence = dirs_fence();
        if let Ok(text) = std::fs::read_to_string(dir.join("hashes.tsv")) {
            // `#fence <dirs>` heads the file; a given-path line (`G`) serves
            // under that fence only.
            let fence_held = text
                .lines()
                .next()
                .and_then(|l| l.strip_prefix("#fence "))
                .is_some_and(|f| f == given_fence);
            for line in text.lines() {
                // hash, len, mtime, path — or the older hash, len, path (a
                // store file by its real path); G, hash, len, mtime, given.
                let parts: Vec<&str> = line.splitn(5, '\t').collect();
                let (given, h, l, m, path) = match parts[..] {
                    ["G", h, l, m, path] => (true, h, l, m, path),
                    [h, l, m, path] => (false, h, l, m, path),
                    [h, l, path] => (false, h, l, "0", path),
                    _ => continue,
                };
                if let (Ok(hash), Ok(len), Ok(mtime)) = (u64::from_str_radix(h, 16), l.parse::<usize>(), m.parse::<u64>()) {
                    if given {
                        if fence_held {
                            by_given.insert(path.to_owned(), Known { hash, len, mtime });
                        }
                    } else {
                        hashes.insert(path.to_owned(), Known { hash, len, mtime });
                    }
                }
            }
        }
        Self {
            dir,
            hashes,
            by_given,
            given_fence,
            hashes_dirty: false,
            pack: None,
            used: Vec::new(),
        }
    }

    /// The index begins: the pack of the last one is read, once, to serve
    /// its tiles from memory.
    fn open_pack(&mut self) {
        self.used.clear();
        self.pack = Pack::open(&self.dir.join("pack.bin"));
    }

    /// The index is done: the tiles it used become the next pack — unless
    /// they are exactly the pack's own (the usual start: nothing to write;
    /// reading every tile back and writing 36 MB was 885 ms of each index
    /// on the 2013 MacBook Air). A changed set is built from the pack in
    /// memory where it has the tile, from the tile's file where not.
    fn close_pack(&mut self) {
        let pack = self.pack.take();
        let used = std::mem::take(&mut self.used);
        if !used.is_empty() {
            let mut names: Vec<(u64, PathBuf)> = used.iter().filter_map(|f| Some((tile_name(f)?, f.clone()))).collect();
            names.sort_by_key(|(n, _)| *n);
            names.dedup_by_key(|(n, _)| *n);
            let same = pack
                .as_ref()
                .is_some_and(|p| p.at.len() == names.len() && names.iter().all(|(n, _)| p.at.contains_key(n)));
            if !same {
                let mut head = Vec::with_capacity(8 + names.len() * 20);
                let mut body: Vec<u8> = Vec::new();
                let mut count = 0u32;
                for (name, file) in &names {
                    let pixels: Vec<u8> = match pack.as_ref().and_then(|p| p.get(*name)) {
                        Some(px) => px.to_vec(),
                        None => {
                            let Ok(px) = std::fs::read(file) else { continue };
                            still_used(file);
                            px
                        }
                    };
                    if pixels.len() != ICON_CHAIN_BYTES {
                        continue;
                    }
                    head.extend_from_slice(&name.to_le_bytes());
                    head.extend_from_slice(&(body.len() as u64).to_le_bytes());
                    head.extend_from_slice(&(pixels.len() as u32).to_le_bytes());
                    body.extend_from_slice(&pixels);
                    count += 1;
                }
                // (The places are from the body's start: moved past the header.)
                let header_len = 8 + count as usize * 20;
                for i in 0..count as usize {
                    let e = i * 20 + 8;
                    let offset = u64::from_le_bytes(head[e..e + 8].try_into().unwrap_or([0; 8])) + header_len as u64;
                    head[e..e + 8].copy_from_slice(&offset.to_le_bytes());
                }
                let mut out = Vec::with_capacity(header_len + body.len());
                out.extend_from_slice(PACK_MAGIC);
                out.extend_from_slice(&count.to_le_bytes());
                out.extend_from_slice(&head);
                out.extend_from_slice(&body);
                let file = self.dir.join("pack.bin");
                let tmp = self.dir.join(format!("pack.tmp{}", std::process::id()));
                if std::fs::create_dir_all(&self.dir)
                    .and_then(|()| std::fs::write(&tmp, &out))
                    .and_then(|()| std::fs::rename(&tmp, &file))
                    .is_err()
                {
                    let _ = std::fs::remove_file(&tmp);
                }
            }
        }
        if self.hashes_dirty {
            self.hashes_dirty = false;
            let mut text = format!("#fence {}\n", self.given_fence);
            for (path, k) in &self.hashes {
                text.push_str(&format!("{:016x}\t{}\t{}\t{path}\n", k.hash, k.len, k.mtime));
            }
            for (path, k) in &self.by_given {
                text.push_str(&format!("G\t{:016x}\t{}\t{}\t{path}\n", k.hash, k.len, k.mtime));
            }
            let file = self.dir.join("hashes.tsv");
            let tmp = self.dir.join(format!("hashes.tmp{}", std::process::id()));
            if std::fs::create_dir_all(&self.dir).and_then(|()| std::fs::write(&tmp, text)).and_then(|()| std::fs::rename(&tmp, &file)).is_err() {
                let _ = std::fs::remove_file(&tmp);
            }
        }
    }

    /// Cache file for `icon_path`, or `None` if the icon file cannot be
    /// stat'ed (vanished between resolve and now).
    fn file_for(&mut self, icon_path: &str, tag: &str) -> Option<PathBuf> {
        // Named by the file it REALLY is: a theme path through a profile
        // (`/etc/profiles/…/share/icons/…`) stays the same across rebuilds
        // while the store file behind it changes, and every store file has
        // the same mtime — a recoloured icon of the same size (silver and
        // graphite differ only in their colours) kept serving the old raster
        // (Max, 2026-10-15: fast launch's Golem Files still silver in the
        // dark, after Golem-Dark was fixed). The store path changes with the
        // content.
        // Named by what the file HOLDS, not where it is: a new system puts
        // the same icons at new store paths, and keyed by path every tile
        // was drawn again at the first start after each update — 76 of 125
        // on the 2013 MacBook Air, 4.3 s (2026-10-29). The bytes of an icon
        // are small next to its tile; a PNG's are read once a start.
        // (A store file never changes: its hash is remembered by its real
        // path — a theme path through a profile is a link into the store.)
        // Known by the path the entry names, under the directories' fence:
        // a store-backed one as it was, any other while its length and
        // mtime stand (a stat through the link — no canonicalize).
        let by_given = match self.by_given.get(icon_path) {
            Some(k) if k.mtime == 0 => Some((k.hash, k.len)),
            Some(k) => std::fs::metadata(icon_path)
                .ok()
                .filter(|m| m.len() as usize == k.len && file_mtime_secs(m) == k.mtime)
                .map(|_| (k.hash, k.len)),
            None => None,
        };
        let (hash, len) = match by_given {
            Some(known) => known,
            None => {
                let real = std::fs::canonicalize(icon_path).ok();
                let real = real.as_deref().and_then(|p| p.to_str()).unwrap_or(icon_path);
                let in_store = real.starts_with("/nix/store/");
                // Known by its real path: a store file as it was; any other
                // while its length and mtime stand (a stat).
                let known = match self.hashes.get(real) {
                    Some(k) if in_store => Some((k.hash, k.len)),
                    Some(k) => std::fs::metadata(real)
                        .ok()
                        .filter(|m| m.len() as usize == k.len && file_mtime_secs(m) == k.mtime)
                        .map(|_| (k.hash, k.len)),
                    None => None,
                };
                let mtime = if in_store {
                    0
                } else {
                    std::fs::metadata(real).map(|m| file_mtime_secs(&m)).unwrap_or(1).max(1)
                };
                let (hash, len) = match known {
                    Some(known) => known,
                    None => {
                        let bytes = std::fs::read(real).ok()?;
                        let known = (fnv1a64_bytes(&bytes), bytes.len());
                        self.hashes.insert(real.to_owned(), Known { hash: known.0, len: known.1, mtime });
                        known
                    }
                };
                self.by_given.insert(icon_path.to_owned(), Known { hash, len, mtime });
                self.hashes_dirty = true;
                (hash, len)
            }
        };
        // (`tag`: the same file drawn another way — an unplated carrier —
        // is another tile.)
        let key = format!("{hash:016x}|{len}|{tag}");
        Some(self.dir.join(format!("{:016x}", fnv1a64(&key))))
    }

    /// Read a cached mip chain back, rejecting files of the wrong size
    /// (truncated write or a stale format/`ICON_SIZE`).
    fn load(&mut self, file: &std::path::Path) -> Option<Vec<u8>> {
        if let Some(pixels) = self.pack.as_ref().and_then(|p| p.get(tile_name(file)?)) {
            if pixels.len() == ICON_CHAIN_BYTES {
                self.used.push(file.to_path_buf());
                return Some(pixels.to_vec());
            }
        }
        let pixels = std::fs::read(file).ok()?;
        still_used(file);
        if pixels.len() == ICON_CHAIN_BYTES {
            self.used.push(file.to_path_buf());
            Some(pixels)
        } else {
            None
        }
    }

    /// Persist a raster via write-to-temp + rename, so a concurrent
    /// reader never sees a half-written file. Failures only cost a
    /// re-rasterize next cold start. Returns whether the tile is on disk.
    fn store(&mut self, file: &std::path::Path, pixels: &[u8]) -> bool {
        self.used.push(file.to_path_buf());
        if let Err(e) = std::fs::create_dir_all(&self.dir) {
            debug!("icon cache: cannot create {:?}: {e}", self.dir);
            return false;
        }
        let tmp = file.with_extension(format!("tmp{}", std::process::id()));
        let write = std::fs::write(&tmp, pixels).and_then(|()| std::fs::rename(&tmp, file));
        if let Err(e) = write {
            debug!("icon cache: cannot write {file:?}: {e}");
            let _ = std::fs::remove_file(&tmp);
            return false;
        }
        true
    }
}

/// FNV-1a, 64-bit: tiny, deterministic, and collision-safe enough for a
/// few hundred cache file names.
fn fnv1a64(s: &str) -> u64 {
    s.bytes().fold(0xcbf29ce484222325, |h, b| {
        (h ^ b as u64).wrapping_mul(0x100000001b3)
    })
}

/// FNV-1a over raw bytes (what [`fnv1a64`] is over a string).
fn fnv1a64_bytes(bytes: &[u8]) -> u64 {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for b in bytes {
        h ^= *b as u64;
        h = h.wrapping_mul(0x0000_0100_0000_01b3);
    }
    h
}

/// Persistent icon-name → file-path resolution cache
/// (`$XDG_CACHE_HOME/waverunner/icon-paths-<fence>.json`). The theme-directory
/// walk costs ~15 ms per icon on a long NixOS `XDG_DATA_DIRS` — failed
/// lookups are the worst, walking everything twice — so resolutions,
/// including negative ones, are memoized across daemon runs.
///
/// Staleness control: positive hits must still exist on disk (re-looked
/// up otherwise), and the whole map is dropped when the [`fence`]
/// fingerprint moves — theme installs/removals, config theme changes,
/// and nix store churn all move it. The residual gap (a file added deep
/// inside an existing theme without touching top-level mtimes) costs a
/// placeholder icon until the fence moves.
struct ResolutionCache {
    file: PathBuf,
    fence: String,
    /// Icon name → resolved path; `None` records a failed lookup.
    map: HashMap<String, Option<String>>,
    dirty: bool,
    /// Names walked through the themes since `take_walked`.
    walked: usize,
}

impl ResolutionCache {
    /// Load from disk. A file per fence: the loaders of both tones (the
    /// dark's and the light's theme chains) live side by side and keep their
    /// own — sharing one file, each dropped the other's and walked every
    /// name again at every start (2026-10-29). A fence that moved starts a
    /// new file; old ones go at the next save.
    fn load(theme: &str) -> Self {
        let fence = fence(theme);
        let file = cache_base().join("waverunner").join(format!("icon-paths-{fence}.json"));
        let held = std::fs::read_to_string(&file).ok();
        let map = held
            .as_deref()
            .and_then(|s| parse_resolutions(s, &fence))
            .unwrap_or_default();
        debug!(
            "icon paths: {} names for {theme} (fence {fence}{})",
            map.len(),
            match held.as_deref().and_then(stored_fence) {
                Some(f) if f == fence => "".to_owned(),
                Some(f) => format!(", the file held {f}: dropped"),
                None => ", no file".to_owned(),
            }
        );
        Self {
            file,
            fence,
            map,
            dirty: false,
            walked: 0,
        }
    }

    /// How many names were walked since the last call.
    fn take_walked(&mut self) -> usize {
        std::mem::take(&mut self.walked)
    }

    /// Resolve `name` via the cache, falling back to the theme walk.
    fn resolve(&mut self, name: &str, themes: &[String]) -> Option<String> {
        // (A cached path is taken as it is — no stat: 125 of them on a cold
        // disk were 215 ms of a start on the 2013 MacBook Air. One that
        // vanished, a theme update, is found out as its tile is named and
        // walked again then: `forget`.)
        match self.map.get(name) {
            Some(None) => return None,
            Some(Some(path)) => return Some(path.clone()),
            None => {}
        }
        let found = themes
            .iter()
            .find_map(|theme| {
                freedesktop_icons::lookup(name)
                    .with_size(ICON_SIZE as u16)
                    .with_theme(theme)
                    .find()
            })
            .or_else(|| {
                freedesktop_icons::lookup(name)
                    .with_size(ICON_SIZE as u16)
                    .find()
            })
            .map(|p| p.to_string_lossy().into_owned());
        self.map.insert(name.to_owned(), found.clone());
        self.dirty = true;
        self.walked += 1;
        found
    }

    /// Drop one name's entry: its cached path vanished (a theme update),
    /// the next lookup walks the theme again.
    fn forget(&mut self, name: &str) {
        if self.map.remove(name).is_some() {
            self.dirty = true;
        }
    }

    /// Drop every cached "not found" entry so the next lookup re-walks
    /// the theme. Called after a nix profile install so freshly-installed
    /// icons are found despite prior negative hits.
    pub(crate) fn clear_negative(&mut self) {
        let before = self.map.len();
        self.map.retain(|_, v| v.is_some());
        if self.map.len() != before {
            self.dirty = true;
        }
    }

    /// Persist if anything changed since the last save.
    fn save(&mut self) {
        if !self.dirty {
            return;
        }
        self.dirty = false;
        let json = serde_json::json!({ "fence": self.fence, "entries": self.map });
        if let Some(dir) = self.file.parent() {
            if let Err(e) = std::fs::create_dir_all(dir) {
                debug!("icon paths: cannot create {dir:?}: {e}");
                return;
            }
        }
        let tmp = self
            .file
            .with_extension(format!("tmp{}", std::process::id()));
        let write =
            std::fs::write(&tmp, json.to_string()).and_then(|()| std::fs::rename(&tmp, &self.file));
        if let Err(e) = write {
            debug!("icon paths: cannot write {:?}: {e}", self.file);
            let _ = std::fs::remove_file(&tmp);
        }
        self.sweep();
    }

    /// Drop the files of fences gone by: any sibling not written for a
    /// month, and the one file every fence once shared.
    fn sweep(&self) {
        let Some(dir) = self.file.parent() else { return };
        let _ = std::fs::remove_file(dir.join("icon-paths.json"));
        let Ok(read) = std::fs::read_dir(dir) else { return };
        let month = std::time::Duration::from_secs(30 * 24 * 3600);
        for entry in read.flatten() {
            let path = entry.path();
            if path == self.file {
                continue;
            }
            let name = entry.file_name();
            let name = name.to_string_lossy();
            if !(name.starts_with("icon-paths-") && name.ends_with(".json")) {
                continue;
            }
            let old = entry
                .metadata()
                .and_then(|m| m.modified())
                .ok()
                .and_then(|t| t.elapsed().ok())
                .is_some_and(|age| age > month);
            if old {
                let _ = std::fs::remove_file(&path);
            }
        }
    }
}

/// The fence a resolution-cache file was written under.
fn stored_fence(s: &str) -> Option<String> {
    let v: serde_json::Value = serde_json::from_str(s).ok()?;
    Some(v.get("fence")?.as_str()?.to_owned())
}

/// Parse the resolution-cache file, rejecting it wholesale on a fence
/// mismatch or any shape surprise.
fn parse_resolutions(s: &str, fence: &str) -> Option<HashMap<String, Option<String>>> {
    let v: serde_json::Value = serde_json::from_str(s).ok()?;
    if v.get("fence")?.as_str()? != fence {
        return None;
    }
    let entries = v.get("entries")?.as_object()?;
    Some(
        entries
            .iter()
            .map(|(k, v)| (k.clone(), v.as_str().map(str::to_owned)))
            .collect(),
    )
}

/// Fingerprint of the icon search environment: theme name, icon size,
/// the data-dir list itself, and the mtime of each top-level icon dir.
/// Installing/removing a theme touches a top-level dir; changing the
/// configured theme or any nix store path in `XDG_DATA_DIRS` changes
/// the string; either drops the resolution cache wholesale.
fn fence(theme: &str) -> String {
    format!("{:016x}", fnv1a64(&format!("{theme}|{ICON_SIZE}|{}", dirs_fence())))
}

/// What the icon directories ARE: each one's path, mtime and real path
/// (a profile or system path keeps its name while the store directory
/// behind it changes with every generation — the real path says so, where
/// a store directory's mtime, always 1, never would), and the nix profile
/// link's own mtime (`nix profile install/remove` rewrites the link).
/// Under one value of this, a path names the same file.
fn dirs_fence() -> String {
    let mut parts = String::new();
    for dir in icon_dirs() {
        let real = std::fs::canonicalize(&dir)
            .ok()
            .map(|p| p.to_string_lossy().into_owned())
            .unwrap_or_default();
        parts.push_str(&format!("|{dir}:{}:{real}", mtime_secs(&dir)));
    }
    let home = std::env::var("HOME").unwrap_or_default();
    let profile = std::path::PathBuf::from(&home).join(".nix-profile");
    let profile_mtime = std::fs::symlink_metadata(&profile)
        .and_then(|m| m.modified())
        .ok()
        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|d| d.as_secs())
        .unwrap_or(0);
    parts.push_str(&format!("|nix-profile:{profile_mtime}"));
    format!("{:016x}", fnv1a64(&parts))
}

/// Every directory icon themes may live in, per the freedesktop icon
/// spec (data home, data dirs, `~/.icons`, plus flat pixmaps).
fn icon_dirs() -> impl Iterator<Item = String> {
    let home = std::env::var("HOME").unwrap_or_default();
    let data_home =
        std::env::var("XDG_DATA_HOME").unwrap_or_else(|_| format!("{home}/.local/share"));
    let data_dirs =
        std::env::var("XDG_DATA_DIRS").unwrap_or_else(|_| "/usr/local/share:/usr/share".to_owned());
    std::iter::once(format!("{data_home}/icons"))
        .chain(
            data_dirs
                .split(':')
                .map(|d| format!("{d}/icons"))
                .collect::<Vec<_>>(),
        )
        .chain([format!("{home}/.icons"), "/usr/share/pixmaps".to_owned()])
}

/// The theme fallback chain for icon lookups: the configured theme
/// first, then the highest-coverage packs installed on this system.
/// (Papirus ships icons for thousands of apps; breeze fills in KDE's
/// `org.kde.*` names.) The bare spec lookup at the end of `resolve`
/// covers hicolor and pixmaps.
fn theme_chain(configured: &str) -> Vec<String> {
    let mut chain = vec![configured.to_owned()];
    for fallback in ["Papirus-Dark", "Papirus", "breeze"] {
        if chain.iter().any(|t| t == fallback) {
            continue;
        }
        let installed = icon_dirs().any(|d| std::path::Path::new(&d).join(fallback).is_dir());
        if installed {
            chain.push(fallback.to_owned());
        }
    }
    chain
}

/// Every icon name available in `themes` or the flat pixmaps dirs —
/// one recursive readdir sweep, keyed by the lowercased file stem
/// mapping to the actual (lookup-usable) name, so `qbittorrent` still
/// finds `qBittorrent.svg`. Lets callers skip the expensive per-name
/// theme walk entirely for names that exist nowhere (the common case
/// for CLI-only packages).
pub(crate) fn available_icon_names(themes: &[String]) -> HashMap<String, String> {
    fn collect(dir: &std::path::Path, depth: u8, names: &mut HashMap<String, String>) {
        let Ok(read) = std::fs::read_dir(dir) else {
            return;
        };
        for entry in read.flatten() {
            let Ok(kind) = entry.file_type() else {
                continue;
            };
            let path = entry.path();
            if kind.is_dir() && depth > 0 {
                collect(&path, depth - 1, names);
            } else if kind.is_file() || kind.is_symlink() {
                if let Some(stem) = path.file_stem().and_then(|s| s.to_str()) {
                    names
                        .entry(stem.to_lowercase())
                        .or_insert_with(|| stem.to_owned());
                }
            }
        }
    }
    let mut names = HashMap::new();
    for dir in icon_dirs() {
        let dir = std::path::PathBuf::from(dir);
        if dir.ends_with("pixmaps") {
            collect(&dir, 0, &mut names);
            continue;
        }
        for theme in themes.iter().map(String::as_str).chain(["hicolor"]) {
            // Theme layouts nest either <size>/<category>/ or
            // <category>/<size>/ — two directory levels above files.
            collect(&dir.join(theme), 2, &mut names);
        }
    }
    names
}

/// A path's mtime in unix seconds; 0 if it cannot be stat'ed.
fn mtime_secs(path: &str) -> u64 {
    std::fs::metadata(path).map(|m| file_mtime_secs(&m)).unwrap_or(0)
}

/// A file's mtime in seconds (0 when it has none).
fn file_mtime_secs(m: &std::fs::Metadata) -> u64 {
    m.modified()
        .ok()
        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

/// `$XDG_CACHE_HOME`, falling back to `~/.cache`.
/// A kept file was read: its date says so (at most once a day), which is
/// what [`prune_caches`] goes by.
pub(crate) fn still_used(file: &std::path::Path) {
    let day = std::time::Duration::from_secs(24 * 3600);
    let old = std::fs::metadata(file).and_then(|m| m.modified()).is_ok_and(|t| t.elapsed().is_ok_and(|e| e > day));
    if old {
        if let Ok(f) = std::fs::File::options().write(true).open(file) {
            let _ = f.set_modified(std::time::SystemTime::now());
        }
    }
}

/// How long a kept file may go unread before it is let go, and how long an
/// icon cache of ANOTHER tuning (an older build's) may stand untouched.
const KEPT_FOR: std::time::Duration = std::time::Duration::from_secs(45 * 24 * 3600);
const OLD_TUNING_FOR: std::time::Duration = std::time::Duration::from_secs(7 * 24 * 3600);

/// THE CACHE IS KEPT SMALL. Nothing ever left `~/.cache/waverunner`: every
/// change of the icons' tuning opened a new `icons-…` directory beside the
/// old ones (fourteen dead ones, half a gigabyte, on Max's computer by
/// 2026-10-13), and every icon whose store path changed left its 350 KB
/// tile behind. Once, a while after the start and out of the way: the icon
/// caches of other tunings that nothing has written to in a week go whole;
/// in this build's own, and among the kept previews and posters, the files
/// nothing has read in 45 days go.
pub(crate) fn prune_caches() {
    let spawned = std::thread::Builder::new().name("cache-prune".into()).spawn(|| {
        std::thread::sleep(std::time::Duration::from_secs(40));
        let base = cache_base().join("waverunner");
        let mine = DiskCache::new().dir;
        let unused = |p: &std::path::Path, more_than: std::time::Duration| {
            std::fs::metadata(p).and_then(|m| m.modified()).is_ok_and(|t| t.elapsed().is_ok_and(|e| e > more_than))
        };
        let (mut dirs, mut files) = (0u32, 0u32);
        for entry in std::fs::read_dir(&base).into_iter().flatten().flatten() {
            let path = entry.path();
            let name = entry.file_name().to_string_lossy().into_owned();
            // (By its newest file, not its own date: reading dates the
            // files, and a cache read every day gains no new one for weeks.)
            let idle = |dir: &std::path::Path| {
                unused(dir, OLD_TUNING_FOR)
                    && std::fs::read_dir(dir).into_iter().flatten().flatten().all(|f| unused(&f.path(), OLD_TUNING_FOR))
            };
            if name.starts_with("icons-") && path.is_dir() && path != mine && idle(&path) {
                dirs += u32::from(std::fs::remove_dir_all(&path).is_ok());
            }
        }
        for dir in [mine, base.join("previews"), base.join("posters")] {
            for entry in std::fs::read_dir(&dir).into_iter().flatten().flatten() {
                let path = entry.path();
                if path.is_file() && unused(&path, KEPT_FOR) {
                    files += u32::from(std::fs::remove_file(&path).is_ok());
                }
            }
        }
        if dirs + files > 0 {
            info!("cache: let go of {dirs} old icon cache(s) and {files} file(s) nothing had read in 45 days");
        }
    });
    if let Err(e) = spawned {
        warn!("cache: no thread to prune on: {e}");
    }
}

pub(crate) fn cache_base() -> PathBuf {
    std::env::var("XDG_CACHE_HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|_| PathBuf::from(std::env::var("HOME").unwrap_or_default()).join(".cache"))
}

/// Read and rasterize one icon file into a ready-to-upload tile: a full
/// [`ICON_CHAIN_BYTES`] mip chain, plated when `icon_plate` is on — except
/// the `asset-*` file-type carrier icons, which stay unplated so the Files
/// section (and pinned filesystem items) shows square icons like its
/// thumbnails, never a rounded frosted plate.
pub(crate) fn rasterize_icon_file(path: &str, id: &str) -> Option<Vec<u8>> {
    let path = std::path::Path::new(path);
    let data = std::fs::read(path).ok()?;
    let pixmap = match path.extension().and_then(|e| e.to_str()) {
        Some("svg") | Some("svgz") => rasterize_svg(&data)?,
        Some("png") => fit_pixmap(tiny_skia::Pixmap::decode_png(&data).ok()?),
        other => {
            debug!("unsupported icon format {other:?} for {id}");
            return None;
        }
    };
    let tile = pixmap.take();
    Some(if id.starts_with("asset-") {
        with_mips(tile)
    } else {
        finish_tile(tile)
    })
}

/// Turn straight (non-premultiplied) RGBA8 pixels of size `w×h` — e.g. a
/// notification's captured avatar / media art — into an uploadable
/// [`ICON_CHAIN_BYTES`] mip chain: centered in the tile and left UNPLATED (it's
/// a photo/logo, not an app icon that wants the frosted plate). `None` when the
/// buffer length doesn't match `w*h*4` or a pixmap can't be built.
pub(crate) fn rasterize_rgba(rgba: &[u8], w: u32, h: u32) -> Option<Vec<u8>> {
    if w == 0 || h == 0 || rgba.len() != (w as usize) * (h as usize) * 4 {
        return None;
    }
    // tiny_skia pixmaps are premultiplied sRGB; the captured pixels are straight.
    let mut premul = rgba.to_vec();
    for px in premul.chunks_exact_mut(4) {
        let a = px[3] as u16;
        px[0] = (px[0] as u16 * a / 255) as u8;
        px[1] = (px[1] as u16 * a / 255) as u8;
        px[2] = (px[2] as u16 * a / 255) as u8;
    }
    let size = tiny_skia::IntSize::from_wh(w, h)?;
    let pixmap = tiny_skia::Pixmap::from_vec(premul, size)?;
    let mut tile = fit_pixmap(pixmap).take();
    circle_mask(&mut tile);
    Some(with_mips(tile))
}

/// Clip an `ICON_SIZE`² premultiplied tile to its inscribed circle: pixels
/// outside fade to transparent (a ~1px antialiased edge, further smoothed by the
/// mip chain). Used to render notification avatars as circles. Scales every
/// channel (incl. alpha) by coverage since the tile is premultiplied.
fn circle_mask(tile: &mut [u8]) {
    let r = ICON_SIZE as f32 / 2.0;
    let c = r - 0.5; // pixel-center of the tile
    for y in 0..ICON_SIZE {
        for x in 0..ICON_SIZE {
            let (dx, dy) = (x as f32 - c, y as f32 - c);
            let d = (dx * dx + dy * dy).sqrt();
            let cov = (r - d + 0.5).clamp(0.0, 1.0); // 1 inside, feather at edge
            if cov < 1.0 {
                let i = ((y * ICON_SIZE + x) * 4) as usize;
                for ch in 0..4 {
                    tile[i + ch] = (tile[i + ch] as f32 * cov) as u8;
                }
            }
        }
    }
}

/// Whether to composite icons onto a frosted plate; set once at startup
/// from `theme.icon_plate` (defaults on). A process-wide switch because the
/// tile producers ([`rasterize_icon_file`], [`placeholder_icon`]) are free
/// functions called from several worker threads.
static ICON_PLATE: std::sync::OnceLock<bool> = std::sync::OnceLock::new();

/// Set the icon-plate mode. Call once, before the icon workers start.
pub fn set_icon_plate(on: bool) {
    let _ = ICON_PLATE.set(on);
}

fn icon_plate_enabled() -> bool {
    *ICON_PLATE.get().unwrap_or(&true)
}

/// Icon content fills this fraction of the plate's larger dimension; the
/// rest is the frosted margin that unifies differently-sized icons.
const PLATE_CONTENT: f32 = 0.80;
/// The icon sitting on the plate is muted for a calmer, cohesive dock:
/// pulled `PLATE_ICON_DESAT` of the way toward its own luminance (0 = full
/// color, 1 = greyscale) and drawn at `PLATE_ICON_OPACITY` so the frosted
/// plate shows through a little.
const PLATE_ICON_DESAT: f32 = 0.0;
const PLATE_ICON_OPACITY: f32 = 1.0;

/// Turn a raw `ICON_SIZE`² base tile into the uploaded mip chain,
/// normalizing it first when `icon_plate` is on. The plate itself is no
/// longer painted here: it is drawn live in the icon shader (see
/// `IconInst::plate` / `icon.wgsl`) so it can wear the surface's adaptive
/// colour — the raster only guarantees the glyph geometry the plate frames.
pub(crate) fn finish_tile(base: Vec<u8>) -> Vec<u8> {
    if icon_plate_enabled() {
        with_mips(normalize_tile(base))
    } else {
        with_mips(base)
    }
}

/// Normalize `base` for the live-drawn plate: trim it to its alpha
/// content, scale that to [`PLATE_CONTENT`] of the tile and center it, so
/// a mix of circular, square and padded theme icons all share one
/// silhouette on the plate. Returns a plain `ICON_SIZE`² premultiplied
/// tile (the caller mips it).
fn normalize_tile(base: Vec<u8>) -> Vec<u8> {
    let bounds = alpha_bounds(&base);
    let mut tile = match tiny_skia::Pixmap::new(ICON_SIZE, ICON_SIZE) {
        Some(p) => p,
        None => return base,
    };
    if let (Some((x0, y0, x1, y1)), Some(size)) =
        (bounds, tiny_skia::IntSize::from_wh(ICON_SIZE, ICON_SIZE))
    {
        if let Some(mut src) = tiny_skia::Pixmap::from_vec(base, size) {
            desaturate_premult(src.data_mut(), PLATE_ICON_DESAT);
            let (bw, bh) = ((x1 - x0) as f32, (y1 - y0) as f32);
            let s = (ICON_SIZE as f32 * PLATE_CONTENT) / bw.max(bh);
            let (bcx, bcy) = ((x0 + x1) as f32 / 2.0, (y0 + y1) as f32 / 2.0);
            let c = ICON_SIZE as f32 / 2.0;
            tile.draw_pixmap(
                0,
                0,
                src.as_ref(),
                &tiny_skia::PixmapPaint {
                    opacity: PLATE_ICON_OPACITY,
                    quality: tiny_skia::FilterQuality::Bilinear,
                    ..Default::default()
                },
                tiny_skia::Transform::from_scale(s, s).post_translate(c - s * bcx, c - s * bcy),
                None,
            );
        }
    }
    tile.take()
}

/// Mute premultiplied-RGBA pixels toward their own luminance by `amount`
/// (0 = unchanged, 1 = greyscale). Safe in premultiplied space: the
/// luminance of `(r,g,b)` is a convex mix, so it stays ≤ alpha and the
/// premultiplied invariant holds.
fn desaturate_premult(px: &mut [u8], amount: f32) {
    if amount <= 0.0 {
        return;
    }
    for p in px.chunks_exact_mut(4) {
        let (r, g, b) = (p[0] as f32, p[1] as f32, p[2] as f32);
        let lum = 0.299 * r + 0.587 * g + 0.114 * b;
        p[0] = (r + (lum - r) * amount).round() as u8;
        p[1] = (g + (lum - g) * amount).round() as u8;
        p[2] = (b + (lum - b) * amount).round() as u8;
    }
}

/// Tight bounding box `(x0, y0, x1, y1)` (high ends exclusive) of pixels
/// whose alpha clears a small threshold, or `None` if fully transparent.
fn alpha_bounds(px: &[u8]) -> Option<(u32, u32, u32, u32)> {
    let s = ICON_SIZE as usize;
    let (mut x0, mut y0, mut x1, mut y1) = (s, s, 0usize, 0usize);
    for y in 0..s {
        for x in 0..s {
            if px[(y * s + x) * 4 + 3] > 8 {
                x0 = x0.min(x);
                y0 = y0.min(y);
                x1 = x1.max(x + 1);
                y1 = y1.max(y + 1);
            }
        }
    }
    (x1 > x0 && y1 > y0).then_some((x0 as u32, y0 as u32, x1 as u32, y1 as u32))
}

/// Append the full mip chain to a base `ICON_SIZE`² premultiplied-RGBA
/// image, yielding [`ICON_CHAIN_BYTES`]: `level0 || level1 || … || 1×1`.
/// Each level box-filters the previous one in linear space (RGB is
/// srgb-encoded, alpha linear) to match the GPU's srgb texture sampling,
/// so minified icons stay clean instead of aliasing. Runs on the icon
/// worker threads and its result is cached to disk, so the cost is paid
/// at most once per icon.
pub(crate) fn with_mips(base: Vec<u8>) -> Vec<u8> {
    let mut chain = base;
    chain.reserve(ICON_CHAIN_BYTES - chain.len());
    let mut size = ICON_SIZE;
    let mut level_start = 0usize;
    while size > 1 {
        let level_len = (size * size * 4) as usize;
        let down = downsample_half(&chain[level_start..level_start + level_len], size);
        level_start += level_len;
        chain.extend_from_slice(&down);
        size /= 2;
    }
    chain
}

/// Box-downsample one `size`×`size` premultiplied-RGBA image to half its
/// side, averaging RGB in linear light and alpha directly.
pub(crate) fn downsample_half(src: &[u8], size: u32) -> Vec<u8> {
    let dec = srgb_decode_lut();
    let enc = srgb_encode_lut();
    let s = size as usize;
    let h = s / 2;
    let mut dst = vec![0u8; h * h * 4];
    for y in 0..h {
        for x in 0..h {
            let (mut r, mut g, mut b, mut a) = (0.0f32, 0.0, 0.0, 0.0);
            for dy in 0..2 {
                for dx in 0..2 {
                    let i = (((y * 2 + dy) * s) + (x * 2 + dx)) * 4;
                    r += dec[src[i] as usize];
                    g += dec[src[i + 1] as usize];
                    b += dec[src[i + 2] as usize];
                    a += src[i + 3] as f32;
                }
            }
            let o = (y * h + x) * 4;
            dst[o] = enc[(r * 0.25 * ENC_LUT_MAX as f32) as usize];
            dst[o + 1] = enc[(g * 0.25 * ENC_LUT_MAX as f32) as usize];
            dst[o + 2] = enc[(b * 0.25 * ENC_LUT_MAX as f32) as usize];
            dst[o + 3] = (a * 0.25).round() as u8;
        }
    }
    dst
}

/// Highest index of the linear→srgb encode LUT (its length is this + 1).
const ENC_LUT_MAX: usize = 4096;

/// srgb byte → linear float, memoized (256 entries).
fn srgb_decode_lut() -> &'static [f32; 256] {
    static LUT: std::sync::OnceLock<[f32; 256]> = std::sync::OnceLock::new();
    LUT.get_or_init(|| {
        std::array::from_fn(|i| {
            let c = i as f32 / 255.0;
            if c <= 0.04045 {
                c / 12.92
            } else {
                ((c + 0.055) / 1.055).powf(2.4)
            }
        })
    })
}

/// linear float (quantized to `ENC_LUT_MAX + 1` steps) → srgb byte.
fn srgb_encode_lut() -> &'static [u8; ENC_LUT_MAX + 1] {
    static LUT: std::sync::OnceLock<[u8; ENC_LUT_MAX + 1]> = std::sync::OnceLock::new();
    LUT.get_or_init(|| {
        std::array::from_fn(|i| {
            let c = i as f32 / ENC_LUT_MAX as f32;
            let s = if c <= 0.003_130_8 {
                c * 12.92
            } else {
                1.055 * c.powf(1.0 / 2.4) - 0.055
            };
            (s * 255.0).round().clamp(0.0, 255.0) as u8
        })
    })
}

/// The ready-to-upload tile of an SVG drawn whole (a symbol of the lone
/// app's actions — `app_actions.rs`).
pub(crate) fn svg_figure(svg: &str) -> Option<Vec<u8>> {
    Some(with_mips(rasterize_svg(svg.as_bytes())?.take()))
}

/// Render an SVG into an `ICON_SIZE`² pixmap.
fn rasterize_svg(data: &[u8]) -> Option<tiny_skia::Pixmap> {
    let tree = resvg::usvg::Tree::from_data(data, &resvg::usvg::Options::default()).ok()?;
    let mut pixmap = tiny_skia::Pixmap::new(ICON_SIZE, ICON_SIZE)?;
    let size = tree.size();
    let scale = (ICON_SIZE as f32 / size.width()).min(ICON_SIZE as f32 / size.height());
    let tx = (ICON_SIZE as f32 - size.width() * scale) / 2.0;
    let ty = (ICON_SIZE as f32 - size.height() * scale) / 2.0;
    resvg::render(
        &tree,
        tiny_skia::Transform::from_scale(scale, scale).post_translate(tx, ty),
        &mut pixmap.as_mut(),
    );
    Some(pixmap)
}

/// Scale an arbitrary-size pixmap down (or up) into `ICON_SIZE`².
fn fit_pixmap(src: tiny_skia::Pixmap) -> tiny_skia::Pixmap {
    if src.width() == ICON_SIZE && src.height() == ICON_SIZE {
        return src;
    }
    let mut dst = match tiny_skia::Pixmap::new(ICON_SIZE, ICON_SIZE) {
        Some(dst) => dst,
        None => return src,
    };
    let scale = (ICON_SIZE as f32 / src.width() as f32).min(ICON_SIZE as f32 / src.height() as f32);
    let tx = (ICON_SIZE as f32 - src.width() as f32 * scale) / 2.0;
    let ty = (ICON_SIZE as f32 - src.height() as f32 * scale) / 2.0;
    dst.draw_pixmap(
        0,
        0,
        src.as_ref(),
        &tiny_skia::PixmapPaint {
            quality: tiny_skia::FilterQuality::Bilinear,
            ..Default::default()
        },
        tiny_skia::Transform::from_scale(scale, scale).post_translate(tx, ty),
        None,
    );
    dst
}

/// Deterministic colored tile for apps without a resolvable icon,
/// so every entry stays clickable and visually distinct.
pub(crate) fn placeholder_icon(name: &str) -> Vec<u8> {
    let hash = name
        .bytes()
        .fold(2166136261u32, |h, b| (h ^ b as u32).wrapping_mul(16777619));
    let (r, g, b) = hue_to_rgb((hash % 360) as f32);

    let mut pixmap = match tiny_skia::Pixmap::new(ICON_SIZE, ICON_SIZE) {
        Some(p) => p,
        None => return vec![0; ICON_CHAIN_BYTES],
    };
    let mut paint = tiny_skia::Paint::default();
    paint.set_color_rgba8(r, g, b, 215);
    paint.anti_alias = true;

    // Rounded-rectangle path with transparent corners so the tile blends
    // cleanly against any card background without hard square edges.
    let s = ICON_SIZE as f32;
    if let Some(path) = rounded_rect_path(4.0, 4.0, s - 8.0, s - 8.0, 10.0) {
        pixmap.fill_path(
            &path,
            &paint,
            tiny_skia::FillRule::Winding,
            tiny_skia::Transform::identity(),
            None,
        );
    }
    finish_tile(pixmap.take())
}

/// A rounded-rectangle path at `(x, y)` of size `w × h` with corner radius
/// `rad`, or `None` if the geometry is degenerate.
fn rounded_rect_path(x: f32, y: f32, w: f32, h: f32, rad: f32) -> Option<tiny_skia::Path> {
    let mut pb = tiny_skia::PathBuilder::new();
    pb.move_to(x + rad, y);
    pb.line_to(x + w - rad, y);
    pb.quad_to(x + w, y, x + w, y + rad);
    pb.line_to(x + w, y + h - rad);
    pb.quad_to(x + w, y + h, x + w - rad, y + h);
    pb.line_to(x + rad, y + h);
    pb.quad_to(x, y + h, x, y + h - rad);
    pb.line_to(x, y + rad);
    pb.quad_to(x, y, x + rad, y);
    pb.close();
    pb.finish()
}

/// Muted-palette hue to RGB (fixed saturation/lightness).
fn hue_to_rgb(hue: f32) -> (u8, u8, u8) {
    let h = hue / 60.0;
    let c = 0.35f32; // chroma: keep tiles muted
    let x = c * (1.0 - (h % 2.0 - 1.0).abs());
    let (r, g, b) = match h as u32 {
        0 => (c, x, 0.0),
        1 => (x, c, 0.0),
        2 => (0.0, c, x),
        3 => (0.0, x, c),
        4 => (x, 0.0, c),
        _ => (c, 0.0, x),
    };
    let m = 0.35f32; // lightness floor
    (
        ((r + m) * 255.0) as u8,
        ((g + m) * 255.0) as u8,
        ((b + m) * 255.0) as u8,
    )
}

/// What to type for a command-line package: its name where that is a command, else
/// what the package put on the owner's profile, whose links name it: `sfizz` is
/// `sfizz_jack` and `sfizz_render`, `bluez` is `bluetoothctl`… The banner said
/// "run: sfizz", a command that does not exist.
fn cli_commands(attr: &str) -> String {
    if crate::launch::on_path(attr) {
        return attr.to_owned();
    }
    let user = std::env::var("USER").unwrap_or_default();
    let links = std::fs::read_dir(format!("/etc/profiles/per-user/{user}/bin"))
        .into_iter()
        .flatten()
        .flatten()
        .filter_map(|e| {
            let target = std::fs::canonicalize(e.path()).ok()?;
            Some((e.file_name().into_string().ok()?, target.to_string_lossy().into_owned()))
        });
    commands_of(attr, links)
}

/// The attr itself when no link names its package; else three commands at most, shortest first.
fn commands_of(attr: &str, links: impl Iterator<Item = (String, String)>) -> String {
    let package = format!("-{}-", attr.rsplit('.').next().unwrap_or(attr));
    let mut found: Vec<String> =
        links.filter(|(_, target)| target.contains(&package)).map(|(name, _)| name).collect();
    found.sort_by_key(|name| (name.len(), name.clone()));
    found.truncate(3);
    if found.is_empty() {
        attr.to_owned()
    } else {
        found.join(", ")
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn a_package_not_named_as_its_command_says_its_commands() {
        let links = || {
            [("sfizz_render", "/nix/store/a-sfizz-1.2/bin/sfizz_render"),
             ("sfizz_jack", "/nix/store/a-sfizz-1.2/bin/sfizz_jack"),
             ("fzf", "/nix/store/b-fzf-0.6/bin/fzf")]
            .into_iter()
            .map(|(n, t)| (n.to_owned(), t.to_owned()))
        };
        assert_eq!(super::commands_of("sfizz", links()), "sfizz_jack, sfizz_render");
        assert_eq!(super::commands_of("nothing", links()), "nothing");
    }

    use super::*;

    /// Fresh scratch cache dir plus a fake "icon" file inside it.
    /// `name` keeps parallel tests out of each other's directories.
    fn scratch(name: &str) -> (DiskCache, PathBuf) {
        let dir = std::env::temp_dir().join(format!(
            "waverunner-icon-cache-test-{}-{name}",
            std::process::id()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let icon = dir.join("icon.svg");
        std::fs::write(&icon, b"<svg/>").unwrap();
        (DiskCache::at(dir), icon)
    }

    #[test]
    fn disk_cache_round_trips() {
        let (mut cache, icon) = scratch("round-trip");
        let icon = icon.to_string_lossy().into_owned();
        let file = cache.file_for(&icon, "").unwrap();
        assert_eq!(cache.load(&file), None, "cold cache misses");

        let pixels = vec![7u8; ICON_CHAIN_BYTES];
        cache.store(&file, &pixels);
        assert_eq!(cache.load(&file), Some(pixels.clone()));

        // The pack: the tiles used are written as one file, served from
        // memory next time even with the tile's own file gone.
        cache.close_pack();
        assert!(cache.dir.join("pack.bin").exists());
        std::fs::remove_file(&file).unwrap();
        cache.open_pack();
        assert_eq!(cache.load(&file), Some(pixels));
        cache.close_pack();

        std::fs::remove_dir_all(&cache.dir).unwrap();
    }

    #[test]
    fn disk_cache_rejects_wrong_size_and_tracks_content() {
        let (mut cache, icon) = scratch("reject");
        let icon_str = icon.to_string_lossy().into_owned();
        let file = cache.file_for(&icon_str, "").unwrap();
        cache.store(&file, &[7u8; 3]);
        assert_eq!(cache.load(&file), None, "truncated raster rejected");

        // Growing the icon file changes its cache key.
        std::fs::write(&icon, b"<svg></svg>").unwrap();
        assert_ne!(cache.file_for(&icon_str, "").unwrap(), file);
        // A vanished icon file has no key at all.
        assert_eq!(cache.file_for("/nonexistent/icon.svg", ""), None);
        // The same file drawn unplated is another tile.
        assert_ne!(cache.file_for(&icon_str, "unplated"), cache.file_for(&icon_str, ""));

        std::fs::remove_dir_all(&cache.dir).unwrap();
    }

    #[test]
    fn resolution_cache_parses_and_fences() {
        let json = r#"{"fence":"abc","entries":{"firefox":"/path/firefox.svg","gone":null}}"#;
        let map = parse_resolutions(json, "abc").unwrap();
        assert_eq!(
            map.get("firefox").unwrap().as_deref(),
            Some("/path/firefox.svg")
        );
        assert_eq!(map.get("gone").unwrap(), &None, "negative result kept");

        assert_eq!(
            parse_resolutions(json, "other"),
            None,
            "fence mismatch drops all"
        );
        assert_eq!(parse_resolutions("not json", "abc"), None);
        assert_eq!(parse_resolutions(r#"{"entries":{}}"#, "abc"), None);
    }

    #[test]
    fn resolution_cache_save_load_round_trips() {
        let (disk, _icon) = scratch("resolutions");
        // The file every fence once shared, and a sibling of a fence gone by.
        std::fs::write(disk.dir.join("icon-paths.json"), "{}").unwrap();
        let stale = disk.dir.join("icon-paths-0.json");
        std::fs::write(&stale, "{}").unwrap();
        let long_ago = std::time::SystemTime::now() - std::time::Duration::from_secs(40 * 24 * 3600);
        std::fs::File::open(&stale).unwrap().set_modified(long_ago).unwrap();
        let mut cache = ResolutionCache {
            file: disk.dir.join("icon-paths-f.json"),
            fence: "f".to_owned(),
            map: HashMap::from([
                ("firefox".to_owned(), Some("/p/firefox.svg".to_owned())),
                ("gone".to_owned(), None),
            ]),
            dirty: true,
            walked: 0,
        };
        cache.save();

        let written = std::fs::read_to_string(&cache.file).unwrap();
        assert_eq!(parse_resolutions(&written, "f").unwrap(), cache.map);
        assert_eq!(stored_fence(&written).as_deref(), Some("f"));
        // A moved fence rejects the same file.
        assert_eq!(parse_resolutions(&written, "g"), None);
        // The save swept the shared file and the stale sibling.
        assert!(!disk.dir.join("icon-paths.json").exists());
        assert!(!stale.exists());

        std::fs::remove_dir_all(&disk.dir).unwrap();
    }

    fn scanned(id: &str) -> AppEntry {
        AppEntry {
            id: id.to_owned(),
            name: id.to_owned(),
            description: None,
            exec: "true".to_owned(),
            icon: None,
            startup_wm_class: None,
            needs_terminal: false,
            path: None,
        }
    }

    #[test]
    fn cli_tiles_never_fabricate_a_tile_for_a_gui_app() {
        // The shipped bug: a GUI package (gui==Some(true)) got a synthetic
        // terminal tile pinned in place of the real app — a green box that
        // opened a shell, unhealed until relogin. It must NEVER get a tile,
        // even before its .desktop reaches the scan (apps empty here).
        let snap = vec![("gimp".to_owned(), vec!["gimp".to_owned()], Some(true))];
        assert!(cli_tiles_from(snap, &[]).is_empty());
    }

    #[test]
    fn cli_tiles_cover_real_cli_tools_and_legacy_entries() {
        let snap = vec![
            ("ripgrep".to_owned(), vec![], Some(false)), // a genuine CLI tool
            ("htop".to_owned(), vec![], None),           // legacy (gui unknown)
        ];
        let tiles = cli_tiles_from(snap, &[]);
        let ids: Vec<&str> = tiles.iter().map(|e| e.id.as_str()).collect();
        assert_eq!(ids, ["ripgrep", "htop"]);
        // The fabricated tile is a terminal launcher, not a GUI app.
        assert!(tiles.iter().all(|t| t.needs_terminal));
    }

    #[test]
    fn cli_tiles_yield_to_a_scanned_desktop_that_already_covers_them() {
        // A CLI-flagged package whose desktop id IS present in the scan
        // gets no duplicate tile — the real .desktop already represents it.
        let snap = vec![("mpv".to_owned(), vec!["mpv".to_owned()], Some(false))];
        assert!(cli_tiles_from(snap.clone(), &[scanned("mpv")]).is_empty());
        // But when the scan lacks it, the tile appears.
        assert_eq!(cli_tiles_from(snap, &[scanned("firefox")]).len(), 1);
    }

    #[test]
    fn icon_mip_chain_math_is_exact() {
        // ICON_MIPS = full chain down to 1×1 inclusive (256 → 9 levels,
        // 128 → 8).
        assert_eq!(ICON_MIPS, ICON_SIZE.trailing_zeros() + 1);
        // The byte total is the geometric sum 4·(S²+(S/2)²+…+1²), which the
        // texture upload reserves — an off-by-one here mis-sizes the buffer.
        let expected: usize = (0..ICON_MIPS)
            .map(|l| {
                let s = ICON_SIZE as usize >> l;
                s * s * 4
            })
            .sum();
        assert_eq!(ICON_CHAIN_BYTES, expected);
        assert_eq!(icon_chain_bytes(1), 4); // a 1×1 chain is one texel
        assert_eq!(icon_chain_bytes(2), 4 * 4 + 4); // 2×2 + 1×1
    }
}
