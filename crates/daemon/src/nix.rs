//! Nix integration: one background thread owning the nixpkgs package
//! index and all mutations of the user's `nix profile`.
//!
//! The index is a full `nix search nixpkgs ^ --json` dump, slimmed to a
//! TSV cache at `$XDG_CACHE_HOME/waverunner/nixpkgs-index.tsv`. On start
//! the cache loads instantly (if present) and a fresh dump replaces it
//! in the background once it is older than a day — the first dump ever
//! also downloads and evaluates nixpkgs, which can take minutes; the UI
//! shows an indexing hint until the index arrives. Install and remove
//! requests run `nix profile ...` sequentially on the same thread, so
//! profile mutations never race each other.

use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::mpsc;
use std::time::Duration;

use calloop::channel::Sender;
use serde::Deserialize;
use tracing::{debug, info, warn};

/// Re-dump the package index when the cache is older than this.
const REFRESH_AFTER: Duration = Duration::from_secs(24 * 60 * 60);
/// Cache format marker (first line); bump to invalidate old caches.
const CACHE_HEADER: &str = "waverunner-nixpkgs-v1";
/// Rank-haystack cap on the description, keeping per-keystroke fuzzy
/// scoring over ~110k packages cheap.
const HAYSTACK_DESC_MAX: usize = 80;

/// One installable nixpkgs package.
#[derive(Debug, Clone)]
pub struct PkgEntry {
    /// Attribute path under nixpkgs (`cowsay`,
    /// `python312Packages.requests`, …) — the install handle and the
    /// unique id.
    pub attr: String,
    /// Package name (`pname`), the display label.
    pub name: String,
    /// Version string (may be empty).
    pub version: String,
    /// Pre-built fuzzy-match target: name, attr and a clipped
    /// description.
    pub haystack: String,
}

/// Thread → daemon notifications.
pub enum Event {
    /// The package index is searchable (package count attached).
    IndexReady(usize),
    /// There is no cache and the dump failed; searching is unavailable.
    IndexFailed,
    /// The top matches for `query` (echoed so stale answers are
    /// recognizable after the query moved on), each with a rasterized
    /// `ICON_SIZE`² icon: the theme icon named like the package when
    /// one exists (Papirus ships icons for far more apps than are
    /// installed), a letter tile otherwise (`placeholder` true).
    Ranked {
        query: String,
        hits: Vec<PkgEntry>,
        icons: Vec<Vec<u8>>,
        placeholders: Vec<bool>,
    },
    /// An install/remove finished. `id` echoes the entry id the request
    /// carried (package attr / desktop-entry id).
    Done { id: String, ok: bool },
}

/// Daemon → thread requests.
pub enum Request {
    /// Fuzzy-rank the index against a query; answered with `Ranked`.
    /// Queued queries coalesce to the newest one.
    Rank { query: String },
    /// `nix profile install nixpkgs#<attr>`. Answered with
    /// `Done { id: attr, .. }`.
    Install { attr: String },
    /// Remove the profile element that provides `desktop_path`.
    /// Answered with `Done { id, .. }`.
    Remove { id: String, desktop_path: String },
}

/// How many top-ranked packages a `Ranked` reply carries. The renderer
/// reserves this many texture-array layers for their icons.
pub const RANK_HITS_MAX: usize = 24;

/// Handle to the nix threads; dropping it stops them after the work in
/// flight.
pub struct Nix {
    ranks: mpsc::Sender<String>,
    mutations: mpsc::Sender<Request>,
}

impl Nix {
    /// Queue a request. Dead threads make this a no-op (their reply
    /// channel is gone with them, so no state is left dangling).
    pub fn request(&self, request: Request) {
        match request {
            Request::Rank { query } => {
                let _ = self.ranks.send(query);
            }
            other => {
                let _ = self.mutations.send(other);
            }
        }
    }
}

/// Spawn the nix threads. The index thread loads/refreshes the package
/// index and serves rank queries (they coalesce to the newest, so slow
/// ranking never queues up behind typing); it also rasterizes the hit
/// icons via `icon_theme`. Profile mutations run on their own worker:
/// a minutes-long install must not block search.
pub fn spawn(events: Sender<Event>, icon_theme: String) -> Nix {
    let (ranks, ranks_rx) = mpsc::channel::<String>();
    let (mutations, mutations_rx) = mpsc::channel::<Request>();

    let rank_events = events.clone();
    let spawned = std::thread::Builder::new()
        .name("waverunner-nix".into())
        .spawn(move || index_and_rank(rank_events, ranks_rx, icon_theme));
    if let Err(e) = spawned {
        warn!("failed to spawn nix index thread: {e}");
    }

    let spawned = std::thread::Builder::new()
        .name("waverunner-nix-mut".into())
        .spawn(move || {
            while let Ok(request) = mutations_rx.recv() {
                let (id, ok) = match request {
                    Request::Install { attr } => {
                        let ok = install(&attr);
                        (attr, ok)
                    }
                    Request::Remove { id, desktop_path } => {
                        let ok = remove(&desktop_path);
                        (id, ok)
                    }
                    Request::Rank { .. } => continue, // routed elsewhere
                };
                if events.send(Event::Done { id, ok }).is_err() {
                    return;
                }
            }
        });
    if let Err(e) = spawned {
        warn!("failed to spawn nix mutation thread: {e}");
    }

    Nix { ranks, mutations }
}

/// Body of the index thread: cache load, background refresh, then rank
/// service until the daemon goes away.
fn index_and_rank(events: Sender<Event>, ranks: mpsc::Receiver<String>, icon_theme: String) {
    let cache = cache_path();
    let mut pkgs = match load_cache(&cache) {
        Ok(pkgs) => {
            info!("nixpkgs index: {} packages (cache)", pkgs.len());
            if events.send(Event::IndexReady(pkgs.len())).is_err() {
                return;
            }
            pkgs
        }
        Err(e) => {
            debug!("no nixpkgs cache: {e:#}");
            Vec::new()
        }
    };
    if cache_age(&cache).is_none_or(|age| age > REFRESH_AFTER) {
        match dump_index() {
            Ok(fresh) => {
                info!("nixpkgs index: {} packages (fresh dump)", fresh.len());
                if let Err(e) = save_cache(&cache, &fresh) {
                    warn!("cannot write nixpkgs cache: {e:#}");
                }
                pkgs = fresh;
                if events.send(Event::IndexReady(pkgs.len())).is_err() {
                    return;
                }
            }
            Err(e) => {
                warn!("nixpkgs dump failed: {e:#}");
                if pkgs.is_empty() && events.send(Event::IndexFailed).is_err() {
                    return;
                }
            }
        }
    }

    let keys: Vec<&str> = pkgs.iter().map(|p| p.haystack.as_str()).collect();
    let mut searcher = waverunner_core::Searcher::new();
    let mut loader = crate::apps::IconLoader::new(icon_theme);
    while let Ok(mut query) = ranks.recv() {
        // Typing outruns ranking: serve only the newest queued query.
        while let Ok(newer) = ranks.try_recv() {
            query = newer;
        }
        let started = std::time::Instant::now();
        let ranked = searcher.rank(&query, &keys);
        let hits: Vec<PkgEntry> = ranked
            .iter()
            .take(RANK_HITS_MAX)
            .map(|&i| pkgs[i].clone())
            .collect();
        // Most package names double as theme icon names (firefox, vlc,
        // gimp, …) — Papirus and friends ship icons for far more apps
        // than are installed. Misses become letter tiles.
        let (icons, placeholders): (Vec<_>, Vec<_>) = hits
            .iter()
            .map(|p| {
                loader.icon_for(&waverunner_core::index::AppEntry {
                    id: format!("pkg-{}", p.attr),
                    name: p.name.clone(),
                    description: None,
                    exec: String::new(),
                    icon: Some(p.name.clone()),
                    needs_terminal: false,
                    path: None,
                })
            })
            .unzip();
        loader.save_resolutions();
        debug!(
            "pkg rank {query:?}: {} of {} in {:?} (incl. icons)",
            ranked.len(),
            keys.len(),
            started.elapsed()
        );
        if events
            .send(Event::Ranked {
                query,
                hits,
                icons,
                placeholders,
            })
            .is_err()
        {
            return;
        }
    }
}

/// `nix profile install nixpkgs#<attr>`; true on success.
fn install(attr: &str) -> bool {
    info!("nix profile install nixpkgs#{attr}");
    match Command::new("nix")
        .args(["profile", "install", &format!("nixpkgs#{attr}")])
        .output()
    {
        Ok(out) if out.status.success() => true,
        Ok(out) => {
            warn!(
                "install of {attr} failed: {}",
                String::from_utf8_lossy(&out.stderr).trim()
            );
            false
        }
        Err(e) => {
            warn!("cannot run nix: {e}");
            false
        }
    }
}

/// Remove the profile element whose store paths provide `desktop_path`
/// (resolved through the profile symlinks); true on success. An app not
/// installed via `nix profile` matches nothing and fails harmlessly.
fn remove(desktop_path: &str) -> bool {
    let canonical = match std::fs::canonicalize(desktop_path) {
        Ok(p) => p,
        Err(e) => {
            warn!("cannot resolve {desktop_path}: {e}");
            return false;
        }
    };
    let element = match profile_element_for(&canonical) {
        Ok(Some(name)) => name,
        Ok(None) => {
            info!("{desktop_path} is not from the nix profile; not removing");
            return false;
        }
        Err(e) => {
            warn!("nix profile list failed: {e:#}");
            return false;
        }
    };
    info!("nix profile remove {element}");
    match Command::new("nix")
        .args(["profile", "remove", &element])
        .output()
    {
        Ok(out) if out.status.success() => true,
        Ok(out) => {
            warn!(
                "remove of {element} failed: {}",
                String::from_utf8_lossy(&out.stderr).trim()
            );
            false
        }
        Err(e) => {
            warn!("cannot run nix: {e}");
            false
        }
    }
}

#[derive(Deserialize)]
struct ProfileList {
    elements: std::collections::HashMap<String, ProfileElement>,
}

#[derive(Deserialize)]
struct ProfileElement {
    #[serde(rename = "storePaths", default)]
    store_paths: Vec<String>,
}

/// Name of the profile element one of whose store paths contains
/// `canonical` (a fully resolved path inside /nix/store).
fn profile_element_for(canonical: &Path) -> anyhow::Result<Option<String>> {
    let out = Command::new("nix")
        .args(["profile", "list", "--json"])
        .output()?;
    anyhow::ensure!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr).trim().to_owned()
    );
    let list: ProfileList = serde_json::from_slice(&out.stdout)?;
    for (name, element) in list.elements {
        if element
            .store_paths
            .iter()
            .any(|sp| canonical.starts_with(sp))
        {
            return Ok(Some(name));
        }
    }
    Ok(None)
}

#[derive(Deserialize)]
struct SearchMeta {
    #[serde(default)]
    pname: String,
    #[serde(default)]
    version: String,
    #[serde(default)]
    description: String,
}

/// Run the full `nix search` dump and slim it into [`PkgEntry`]s,
/// sorted so top-level attrs come before nested package sets (ties in
/// fuzzy score then resolve to the likelier candidate).
fn dump_index() -> anyhow::Result<Vec<PkgEntry>> {
    let started = std::time::Instant::now();
    let out = Command::new("nix")
        .args(["search", "nixpkgs", "^", "--json"])
        .output()?;
    anyhow::ensure!(
        out.status.success(),
        "nix search failed: {}",
        String::from_utf8_lossy(&out.stderr).trim().to_owned()
    );
    let raw: std::collections::HashMap<String, SearchMeta> = serde_json::from_slice(&out.stdout)?;
    let mut pkgs: Vec<PkgEntry> = raw
        .into_iter()
        .filter_map(|(key, meta)| {
            // Keys look like `legacyPackages.x86_64-linux.<attr...>`.
            let attr = key.splitn(3, '.').nth(2)?.to_owned();
            Some(entry(attr, meta.pname, meta.version, &meta.description))
        })
        .collect();
    pkgs.sort_by(|a, b| {
        let depth = |p: &PkgEntry| p.attr.matches('.').count();
        depth(a).cmp(&depth(b)).then_with(|| a.attr.cmp(&b.attr))
    });
    debug!(
        "nixpkgs dump: {} packages in {:?}",
        pkgs.len(),
        started.elapsed()
    );
    Ok(pkgs)
}

/// Build one entry with its pre-computed rank haystack.
fn entry(attr: String, name: String, version: String, description: &str) -> PkgEntry {
    let clipped: String = description
        .chars()
        .take(HAYSTACK_DESC_MAX)
        .map(|c| if c == '\t' || c == '\n' { ' ' } else { c })
        .collect();
    let haystack = format!("{name} {attr} {clipped}");
    PkgEntry {
        attr,
        name,
        version,
        haystack,
    }
}

fn cache_path() -> PathBuf {
    let base = std::env::var("XDG_CACHE_HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|_| {
            PathBuf::from(std::env::var("HOME").unwrap_or_default()).join(".cache")
        });
    base.join("waverunner").join("nixpkgs-index.tsv")
}

fn cache_age(path: &Path) -> Option<Duration> {
    path.metadata()
        .and_then(|m| m.modified())
        .ok()
        .and_then(|t| t.elapsed().ok())
}

/// TSV cache: header line, then `attr\tversion\tname\tdescription`.
fn save_cache(path: &Path, pkgs: &[PkgEntry]) -> anyhow::Result<()> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let mut text = String::with_capacity(pkgs.len() * 96);
    text.push_str(CACHE_HEADER);
    text.push('\n');
    for p in pkgs {
        // The haystack's tail is the already-sanitized description.
        let desc = p
            .haystack
            .get(p.name.len() + p.attr.len() + 2..)
            .unwrap_or("");
        text.push_str(&format!(
            "{}\t{}\t{}\t{}\n",
            p.attr, p.version, p.name, desc
        ));
    }
    let tmp = path.with_extension("tsv.tmp");
    std::fs::write(&tmp, text)?;
    std::fs::rename(&tmp, path)?;
    Ok(())
}

fn load_cache(path: &Path) -> anyhow::Result<Vec<PkgEntry>> {
    let text = std::fs::read_to_string(path)?;
    let mut lines = text.lines();
    anyhow::ensure!(
        lines.next() == Some(CACHE_HEADER),
        "unknown cache format in {}",
        path.display()
    );
    Ok(lines
        .filter_map(|line| {
            let mut cols = line.splitn(4, '\t');
            let attr = cols.next()?.to_owned();
            let version = cols.next()?.to_owned();
            let name = cols.next()?.to_owned();
            let desc = cols.next().unwrap_or("");
            Some(entry(attr, name, version, desc))
        })
        .collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cache_round_trips() {
        let dir = std::env::temp_dir().join("waverunner-nix-test");
        let path = dir.join("nixpkgs-index.tsv");
        let pkgs = vec![
            entry(
                "cowsay".into(),
                "cowsay".into(),
                "3.8.4".into(),
                "ASCII cow\twith\nnewline",
            ),
            entry("a.b.c".into(), "c".into(), String::new(), ""),
        ];
        save_cache(&path, &pkgs).unwrap();
        let loaded = load_cache(&path).unwrap();
        assert_eq!(loaded.len(), 2);
        assert_eq!(loaded[0].attr, "cowsay");
        assert_eq!(loaded[0].version, "3.8.4");
        assert_eq!(loaded[0].haystack, pkgs[0].haystack);
        assert!(!loaded[0].haystack.contains('\t'));
        assert_eq!(loaded[1].name, "c");
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn haystack_clips_long_descriptions() {
        let long = "x".repeat(500);
        let e = entry("foo".into(), "foo".into(), "1".into(), &long);
        assert!(e.haystack.len() < 200);
    }

    /// Manual perf check against the machine's real package cache:
    /// `cargo test -p waverunner-daemon rank_the_real -- --ignored --nocapture`
    #[test]
    #[ignore = "needs the real nixpkgs cache"]
    fn rank_the_real_index_is_fast_enough() {
        let Ok(pkgs) = load_cache(&cache_path()) else {
            eprintln!("no cache; skipping");
            return;
        };
        let keys: Vec<&str> = pkgs.iter().map(|p| p.haystack.as_str()).collect();
        let mut searcher = waverunner_core::Searcher::new();
        for query in ["fi", "firefox", "media player", "zzqx"] {
            let started = std::time::Instant::now();
            let ranked = searcher.rank(query, &keys);
            eprintln!(
                "rank {query:?}: {} of {} in {:?}",
                ranked.len(),
                keys.len(),
                started.elapsed()
            );
        }
    }

    /// Manual check of theme-icon coverage for package names:
    /// `cargo test -p waverunner-daemon pkg_icon -- --ignored --nocapture`
    #[test]
    #[ignore = "needs the machine's icon theme"]
    fn pkg_icon_lookup_coverage() {
        let mut loader = crate::apps::IconLoader::new("Papirus-Dark".into());
        for name in ["firefox", "vlc", "gimp", "cowsay", "ripgrep", "xterm"] {
            let (_, placeholder) = loader.icon_for(&waverunner_core::index::AppEntry {
                id: format!("pkg-{name}"),
                name: name.into(),
                description: None,
                exec: String::new(),
                icon: Some(name.into()),
                needs_terminal: false,
                path: None,
            });
            eprintln!(
                "{name}: {}",
                if placeholder {
                    "letter tile"
                } else {
                    "themed icon"
                }
            );
        }
    }
}
