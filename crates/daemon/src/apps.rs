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
use tracing::{debug, warn};
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
pub const ICON_SIZE: u32 = 256;

/// Number of mip levels in the icon texture (base + downsamples), i.e.
/// `log2(ICON_SIZE) + 1`. The smallest level is 1×1.
pub const ICON_MIPS: u32 = ICON_SIZE.trailing_zeros() + 1;

/// Byte length of one icon's full mip chain (level 0 followed by every
/// downsample, premultiplied RGBA8): `ICON_SIZE² + (ICON_SIZE/2)² + … + 1`
/// texels × 4. Every producer emits this so the renderer uploads chains
/// uniformly and the on-disk cache stores them.
pub const ICON_CHAIN_BYTES: usize = icon_chain_bytes(ICON_SIZE);

const fn icon_chain_bytes(mut size: u32) -> usize {
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
    /// Home-tree file index for search (fresh every rescan).
    pub files: Vec<FileEntry>,
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
pub fn spawn_indexer(icon_theme: String, results: Sender<LoadedApps>) -> Indexer {
    let (requests, rx) = mpsc::channel::<bool>();
    let spawned = std::thread::Builder::new()
        .name("waverunner-index".into())
        .spawn(move || {
            let mut loader = IconLoader::new(icon_theme);
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
                let index = DesktopIndex::scan();
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
                let cli = managed_cli_tiles(&entries);
                kinds.extend(std::iter::repeat_n(EntryKind::App, cli.len()));
                entries.extend(cli);
                let folders = home_folders();
                kinds.extend(std::iter::repeat_n(EntryKind::File, folders.len()));
                entries.extend(folders);
                for asset in icon_assets() {
                    entries.push(asset);
                    kinds.push(EntryKind::Asset);
                }
                let (icons, placeholders): (Vec<_>, Vec<_>) =
                    entries.iter().map(|entry| loader.icon_for(entry)).unzip();
                loader.resolutions.save();
                let files = scan_home_files();
                debug!(
                    "indexed {} entries + {} home files in {:?} (scan {:?}, icons {:?})",
                    entries.len(),
                    files.len(),
                    started.elapsed(),
                    scanned - started,
                    scanned.elapsed()
                );
                if results
                    .send(LoadedApps {
                        entries,
                        kinds,
                        icons,
                        placeholders,
                        files,
                    })
                    .is_err()
                {
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
/// user's shell behind the same StandardOS banner as the "try it" nix
/// shell — the tool is already on PATH (home-manager installed it), and
/// the banner names the command to type. Skips any package a real
/// `.desktop` already covers (`apps` = the scanned entries).
fn managed_cli_tiles(apps: &[AppEntry]) -> Vec<AppEntry> {
    let app_ids: HashSet<&str> = apps.iter().map(|e| e.id.as_str()).collect();
    crate::managed::snapshot()
        .into_iter()
        .filter(|(_, desktop_ids)| !desktop_ids.iter().any(|d| app_ids.contains(d.as_str())))
        .map(|(attr, _)| AppEntry {
            id: attr.clone(),
            name: attr.clone(),
            description: Some("Command-line tool".to_owned()),
            // attr ≈ the package's main program (the banner's "run:").
            exec: format!(
                "{} exec \"${{SHELL:-bash}}\"",
                crate::launch::banner_cmd(&attr, None, &attr)
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
/// Build/VCS trees indexed by no one on purpose.
const FILE_WALK_SKIP: [&str; 2] = ["target", "node_modules"];

/// Walk the home directory (breadth-first, hidden entries and symlinks
/// skipped) into the search index. Runs on the indexer thread on every
/// rescan, so results track the disk.
fn scan_home_files() -> Vec<FileEntry> {
    let Ok(home) = std::env::var("HOME") else {
        return Vec::new();
    };
    let mut out = Vec::new();
    let mut queue = std::collections::VecDeque::from([(PathBuf::from(&home), 0usize)]);
    while let Some((dir, depth)) = queue.pop_front() {
        let Ok(read) = std::fs::read_dir(&dir) else {
            continue;
        };
        for e in read.flatten() {
            let name = e.file_name().to_string_lossy().into_owned();
            if name.starts_with('.') || FILE_WALK_SKIP.contains(&name.as_str()) {
                continue;
            }
            let Ok(ft) = e.file_type() else { continue };
            if ft.is_symlink() {
                continue;
            }
            let is_dir = ft.is_dir();
            let path = e.path();
            if is_dir && depth + 1 < FILE_WALK_DEPTH {
                queue.push_back((path.clone(), depth + 1));
            }
            out.push(FileEntry {
                name,
                path: path.to_string_lossy().into_owned(),
                is_dir,
            });
            if out.len() >= FILE_INDEX_CAP {
                return out;
            }
        }
    }
    out
}

/// Turns an `AppEntry` into `ICON_SIZE`² pixels, cheapest source first:
/// in-memory rasters (covers rescans), on-disk rasters (covers cold
/// start), then rasterizing the icon file. Icon-name → file-path
/// resolutions get their own persistent cache because the theme
/// directory walk — not rasterization — dominates index time.
pub(crate) struct IconLoader {
    /// Theme fallback chain, best first (see [`theme_chain`]).
    themes: Vec<String>,
    rasters: HashMap<String, (Vec<u8>, bool)>,
    disk: DiskCache,
    resolutions: ResolutionCache,
}

impl IconLoader {
    pub(crate) fn new(theme: String) -> Self {
        let themes = theme_chain(&theme);
        Self {
            resolutions: ResolutionCache::load(&themes.join("+")),
            rasters: HashMap::new(),
            disk: DiskCache::new(),
            themes,
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
        let (key, path) = match self.resolve(entry) {
            Some(path) => (path.clone(), Some(path)),
            None => (format!("placeholder:{}", entry.name), None),
        };
        if let Some((pixels, is_placeholder)) = self.rasters.get(&key) {
            return (pixels.clone(), *is_placeholder);
        }
        // Placeholder tiles are cheap to regenerate and never touch disk;
        // real icons go through the persistent cache.
        let cache_file = path.as_deref().and_then(|p| self.disk.file_for(p));
        let (pixels, is_placeholder) = match cache_file.as_deref().and_then(DiskCache::load) {
            Some(pixels) => (pixels, false),
            None => match path.and_then(|p| rasterize_icon_file(&p, &entry.id)) {
                Some(pixels) => {
                    if let Some(file) = &cache_file {
                        self.disk.store(file, &pixels);
                    }
                    (pixels, false)
                }
                None => (placeholder_icon(&entry.name), true),
            },
        };
        self.rasters.insert(key, (pixels.clone(), is_placeholder));
        (pixels, is_placeholder)
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
}

impl DiskCache {
    fn new() -> Self {
        // The cache stores finished tiles (plated + mipped), so its identity
        // must track the plate appearance: the directory carries a hash of
        // every plate parameter, so any tuning lands in a fresh directory
        // instead of serving stale tiles (and `raw` when plating is off).
        let variant = if icon_plate_enabled() {
            let params = format!(
                "{PLATE_CONTENT}|{PLATE_INSET}|{PLATE_RADIUS}|{PLATE_FILL_ALPHA}|{PLATE_EDGE_ALPHA}|{PLATE_EDGE_WIDTH}|{PLATE_ICON_DESAT}|{PLATE_ICON_OPACITY}"
            );
            format!("plate-{:08x}", fnv1a64(&params))
        } else {
            "raw".to_owned()
        };
        Self {
            dir: cache_base()
                .join("waverunner")
                .join(format!("icons-{ICON_SIZE}-{variant}")),
        }
    }

    /// Cache file for `icon_path`, or `None` if the icon file cannot be
    /// stat'ed (vanished between resolve and now).
    fn file_for(&self, icon_path: &str) -> Option<PathBuf> {
        let meta = std::fs::metadata(icon_path).ok()?;
        let mtime = meta
            .modified()
            .ok()
            .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
            .map(|d| d.as_secs())
            .unwrap_or(0);
        let key = format!("{icon_path}|{}|{mtime}", meta.len());
        Some(self.dir.join(format!("{:016x}", fnv1a64(&key))))
    }

    /// Read a cached mip chain back, rejecting files of the wrong size
    /// (truncated write or a stale format/`ICON_SIZE`).
    fn load(file: &std::path::Path) -> Option<Vec<u8>> {
        let pixels = std::fs::read(file).ok()?;
        (pixels.len() == ICON_CHAIN_BYTES).then_some(pixels)
    }

    /// Persist a raster via write-to-temp + rename, so a concurrent
    /// reader never sees a half-written file. Failures only cost a
    /// re-rasterize next cold start.
    fn store(&self, file: &std::path::Path, pixels: &[u8]) {
        if let Err(e) = std::fs::create_dir_all(&self.dir) {
            debug!("icon cache: cannot create {:?}: {e}", self.dir);
            return;
        }
        let tmp = file.with_extension(format!("tmp{}", std::process::id()));
        let write = std::fs::write(&tmp, pixels).and_then(|()| std::fs::rename(&tmp, file));
        if let Err(e) = write {
            debug!("icon cache: cannot write {file:?}: {e}");
            let _ = std::fs::remove_file(&tmp);
        }
    }
}

/// FNV-1a, 64-bit: tiny, deterministic, and collision-safe enough for a
/// few hundred cache file names.
fn fnv1a64(s: &str) -> u64 {
    s.bytes().fold(0xcbf29ce484222325, |h, b| {
        (h ^ b as u64).wrapping_mul(0x100000001b3)
    })
}

/// Persistent icon-name → file-path resolution cache
/// (`$XDG_CACHE_HOME/waverunner/icon-paths.json`). The theme-directory
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
}

impl ResolutionCache {
    /// Load from disk, dropping the map if the fence moved.
    fn load(theme: &str) -> Self {
        let file = cache_base().join("waverunner").join("icon-paths.json");
        let fence = fence(theme);
        let map = std::fs::read_to_string(&file)
            .ok()
            .and_then(|s| parse_resolutions(&s, &fence))
            .unwrap_or_default();
        Self {
            file,
            fence,
            map,
            dirty: false,
        }
    }

    /// Resolve `name` via the cache, falling back to the theme walk.
    fn resolve(&mut self, name: &str, themes: &[String]) -> Option<String> {
        match self.map.get(name) {
            Some(None) => return None,
            Some(Some(path)) if std::path::Path::new(path).exists() => return Some(path.clone()),
            // Cached path vanished (theme update): re-walk below.
            _ => {}
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
        found
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
    }
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
    let mut parts = format!("{theme}|{ICON_SIZE}");
    for dir in icon_dirs() {
        parts.push_str(&format!("|{dir}:{}", mtime_secs(&dir)));
    }
    // The nix profile symlink's own mtime changes whenever `nix profile
    // install/remove` runs (the symlink target is updated). Nix store
    // directory mtimes are always 1 for reproducibility, so following the
    // symlink would never detect a profile change; we must read the link
    // itself. Including this in the fence ensures the on-disk resolution
    // cache is dropped after any nix profile mutation.
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
    std::fs::metadata(path)
        .and_then(|m| m.modified())
        .ok()
        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

/// `$XDG_CACHE_HOME`, falling back to `~/.cache`.
fn cache_base() -> PathBuf {
    std::env::var("XDG_CACHE_HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|_| PathBuf::from(std::env::var("HOME").unwrap_or_default()).join(".cache"))
}

/// Read and rasterize one icon file into a ready-to-upload tile: a full
/// [`ICON_CHAIN_BYTES`] mip chain, plated when `icon_plate` is on.
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
    Some(finish_tile(pixmap.take()))
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
/// Plate margin inside the `ICON_SIZE` tile (at `ICON_SIZE = 256`). Small,
/// so the plate fills its slot like a group box does.
const PLATE_INSET: f32 = 6.0;
/// Plate corner radius (~22% of the side, matching the group-box tiles).
const PLATE_RADIUS: f32 = 56.0;
/// Frosted fill (white) and hairline-edge (white) opacities over the dark
/// card, plus the edge width. Edge is thick at `ICON_SIZE` so it survives
/// downscaling to the small dock size (~1px there).
const PLATE_FILL_ALPHA: u8 = 64;
const PLATE_EDGE_ALPHA: u8 = 96;
const PLATE_EDGE_WIDTH: f32 = 5.0;
/// The icon sitting on the plate is muted for a calmer, cohesive dock:
/// pulled `PLATE_ICON_DESAT` of the way toward its own luminance (0 = full
/// color, 1 = greyscale) and drawn at `PLATE_ICON_OPACITY` so the frosted
/// plate shows through a little.
const PLATE_ICON_DESAT: f32 = 0.30;
const PLATE_ICON_OPACITY: f32 = 0.88;

/// Turn a raw `ICON_SIZE`² base tile into the uploaded mip chain, plating
/// it first when `icon_plate` is on.
pub(crate) fn finish_tile(base: Vec<u8>) -> Vec<u8> {
    if icon_plate_enabled() {
        with_mips(plate_tile(base))
    } else {
        with_mips(base)
    }
}

/// Composite `base` onto a frosted rounded-square plate: trim it to its
/// alpha content, scale that to [`PLATE_CONTENT`] of the tile and center
/// it, so a mix of circular, square and padded theme icons all share one
/// silhouette. Returns a plain `ICON_SIZE`² premultiplied tile (the caller
/// mips it).
fn plate_tile(base: Vec<u8>) -> Vec<u8> {
    let bounds = alpha_bounds(&base);
    let mut tile = match tiny_skia::Pixmap::new(ICON_SIZE, ICON_SIZE) {
        Some(p) => p,
        None => return base,
    };
    draw_plate(&mut tile);
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

/// Draw the frosted plate: a translucent white rounded square with a
/// slightly brighter hairline edge, so it reads as glass over the dark
/// card. The shape's transparent corners give the cohesive silhouette.
fn draw_plate(tile: &mut tiny_skia::Pixmap) {
    let s = ICON_SIZE as f32;
    let Some(path) = rounded_rect_path(
        PLATE_INSET,
        PLATE_INSET,
        s - 2.0 * PLATE_INSET,
        s - 2.0 * PLATE_INSET,
        PLATE_RADIUS,
    ) else {
        return;
    };
    let mut fill = tiny_skia::Paint::default();
    fill.set_color_rgba8(255, 255, 255, PLATE_FILL_ALPHA);
    fill.anti_alias = true;
    tile.fill_path(
        &path,
        &fill,
        tiny_skia::FillRule::Winding,
        tiny_skia::Transform::identity(),
        None,
    );
    let mut edge = tiny_skia::Paint::default();
    edge.set_color_rgba8(255, 255, 255, PLATE_EDGE_ALPHA);
    edge.anti_alias = true;
    tile.stroke_path(
        &path,
        &edge,
        &tiny_skia::Stroke {
            width: PLATE_EDGE_WIDTH,
            ..Default::default()
        },
        tiny_skia::Transform::identity(),
        None,
    );
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
fn downsample_half(src: &[u8], size: u32) -> Vec<u8> {
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

#[cfg(test)]
mod tests {
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
        (DiskCache { dir }, icon)
    }

    #[test]
    fn disk_cache_round_trips() {
        let (cache, icon) = scratch("round-trip");
        let icon = icon.to_string_lossy().into_owned();
        let file = cache.file_for(&icon).unwrap();
        assert_eq!(DiskCache::load(&file), None, "cold cache misses");

        let pixels = vec![7u8; ICON_CHAIN_BYTES];
        cache.store(&file, &pixels);
        assert_eq!(DiskCache::load(&file), Some(pixels));

        std::fs::remove_dir_all(&cache.dir).unwrap();
    }

    #[test]
    fn disk_cache_rejects_wrong_size_and_tracks_content() {
        let (cache, icon) = scratch("reject");
        let icon_str = icon.to_string_lossy().into_owned();
        let file = cache.file_for(&icon_str).unwrap();
        cache.store(&file, &[7u8; 3]);
        assert_eq!(DiskCache::load(&file), None, "truncated raster rejected");

        // Growing the icon file changes its cache key.
        std::fs::write(&icon, b"<svg></svg>").unwrap();
        assert_ne!(cache.file_for(&icon_str).unwrap(), file);
        // A vanished icon file has no key at all.
        assert_eq!(cache.file_for("/nonexistent/icon.svg"), None);

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
        let mut cache = ResolutionCache {
            file: disk.dir.join("icon-paths.json"),
            fence: "f".to_owned(),
            map: HashMap::from([
                ("firefox".to_owned(), Some("/p/firefox.svg".to_owned())),
                ("gone".to_owned(), None),
            ]),
            dirty: true,
        };
        cache.save();

        let written = std::fs::read_to_string(&cache.file).unwrap();
        assert_eq!(parse_resolutions(&written, "f").unwrap(), cache.map);
        // A moved fence rejects the same file.
        assert_eq!(parse_resolutions(&written, "g"), None);

        std::fs::remove_dir_all(&disk.dir).unwrap();
    }
}
