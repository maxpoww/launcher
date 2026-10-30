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
const CACHE_HEADER: &str = "waverunner-nixpkgs-v5";
/// Prebuilt nix-index file database (weekly, ~100 MB): the file
/// listing of every nixpkgs package, without downloading any of them.
const FILE_INDEX_URL: &str =
    "https://github.com/nix-community/nix-index-database/releases/latest/download/index-x86_64-linux";
/// Re-download the file database when older than this (upstream
/// publishes weekly).
const FILE_INDEX_MAX_AGE: Duration = Duration::from_secs(7 * 24 * 60 * 60);
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
    /// Icon-name hints from the package's own `.desktop` file names
    /// (via the nix-index file database) — authoritative, no guessing:
    /// `ghostty` ships `com.mitchellh.ghostty.desktop`.
    pub icons: Vec<String>,
    /// Store path of the best icon file inside the package itself
    /// (via nix-index), streamable from the binary cache when no local
    /// theme or catalog has art for it.
    pub icon_path: Option<String>,
}

/// Thread → daemon notifications.
pub enum Event {
    /// The package index is searchable (package count attached).
    IndexReady(usize),
    /// There is no cache and the dump failed; searching is unavailable.
    IndexFailed,
    /// The top matches for `query` (echoed so stale answers are
    /// recognizable after the query moved on). Sent the moment ranking
    /// finishes; the icons are asked for apart (`Request::Icons`) and come
    /// on their own thread as `HitIcons`, so no icon — nor a download of
    /// one — ever holds a result back.
    Ranked { query: String, hits: Vec<PkgEntry> },
    /// Icons for packages asked for (`Request::Icons`), a few at a time,
    /// in the order asked (what is on screen first) — each the theme's
    /// icon named like the package (Papirus ships icons for far more apps
    /// than are installed), else Flathub's or the package's own. A package
    /// that has none anywhere gets nothing: its cell keeps the installer
    /// box it wears meanwhile.
    HitIcons { query: String, icons: Vec<HitIcon> },
    /// What a package's page says of it beyond its name (`Request::Details`);
    /// empty where nothing is known.
    Details { attr: String, details: PkgDetails },
    /// A declarative install or uninstall finished — the package list was
    /// edited and the privileged apply helper ran `nixos-rebuild switch` to
    /// completion (see [`crate::applier`]). `id` echoes the request's cell
    /// id (package attr on install, app id on uninstall). `op` says WHAT
    /// finished — the handler must never infer it from daemon state: the
    /// `uninstalling` map is pruned by rescans, and a scan landing between
    /// the switch removing the desktop files and the worker noticing
    /// completion made a Remove-Done mis-route into the install branch
    /// (confirming the very managed entry it should remove — #57, the
    /// leaked-uninstall race, reproduced 12/12 on the ASUS batch
    /// 2026-09-10). `desktop_ids` is `Some(stems)` after a successful
    /// install — the `.desktop` entry stems the installed package actually
    /// ships, read straight from its store path (an empty vec = a CLI-only
    /// tool with no GUI launcher). It is `None` for a removal, a failed
    /// install, or when the store path could not be resolved. The daemon
    /// records it so a just-installed GUI app is never stood in for by a
    /// synthetic terminal tile while its `.desktop` is still propagating
    /// into the scan (which used to pin a green-box "CLI" in the real
    /// app's place).
    Done {
        id: String,
        op: DoneOp,
        ok: bool,
        desktop_ids: Option<Vec<String>>,
    },
    /// A `nix build` for an ephemeral "try it" run finished. `terminal`
    /// is read from the built package (a `.desktop` with `Terminal=true`,
    /// or no `.desktop` at all → a CLI tool) so the daemon runs it in a
    /// terminal vs headless. For a terminal tool, `program` (the main
    /// binary in `bin/`) and `version` (from the store path) name the
    /// command in the "try it" shell banner.
    Realized {
        attr: String,
        ok: bool,
        terminal: bool,
        program: Option<String>,
        version: Option<String>,
    },
}

/// What the Install section's PAGE of a package says of it beyond its name
/// and version: its whole description, its site, its licence, the program it
/// runs, its long description. From the details file made with the index
/// (`build_index`: the flake's `nix-env --meta`), else asked of nix once for
/// that package and kept on disk (`pkg-meta/`).
#[derive(Debug, Clone, Default, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct PkgDetails {
    pub description: String,
    pub homepage: Option<String>,
    pub license: Option<String>,
    pub program: Option<String>,
    pub long: Option<String>,
}

/// One package's icon (`Event::HitIcons`).
pub struct HitIcon {
    pub attr: String,
    /// `ICON_SIZE`² pixels, a mip chain.
    pub pixels: Vec<u8>,
    pub placeholder: bool,
}

/// Daemon → thread requests.
pub enum Request {
    /// Fuzzy-rank the index against a query; answered with `Ranked`.
    /// Queued queries coalesce to the newest one.
    Rank { query: String },
    /// The icons of these packages (the hits of `query` the daemon has
    /// none for yet), on the icon thread; answered with `HitIcons`. A newer
    /// request takes over from an older one still being answered.
    Icons { query: String, pkgs: Vec<PkgEntry> },
    /// A package's page opened: what is known of it (`Event::Details`).
    Details { attr: String },
    /// Install a package declaratively: add `attr` to the package list and
    /// wait for the privileged helper's `nixos-rebuild switch` to finish
    /// (see [`crate::applier`]). `id` echoes to `Done { id, .. }` — the
    /// grid cell whose busy / failed state the install resolves. On the
    /// install path `id` == `attr`.
    Install { id: String, attr: String },
    /// Several installs and removals that belong together (a module's
    /// Apply): handed to the worker as ONE message, so they are one list
    /// write and one rebuild — sent one by one, the worker could wake on the
    /// first and build it alone before the rest had arrived.
    Batch(Vec<Request>),
    /// Uninstall a package declaratively: remove `attr` from the package
    /// list and wait for the rebuild. `id` echoes to `Done { id, .. }` —
    /// the app cell whose busy / failed state the removal resolves (on the
    /// uninstall path `id` is the desktop id, not the attr).
    Remove { id: String, attr: String },
    /// Realize (`nix build`) a package so it can be run ephemerally
    /// ("try it" — drag a package out of the box). Answered with
    /// `Done { id: attr, .. }`; the daemon spawns the actual run once the
    /// build lands. This is the slow, cacheable part — the "Launching…"
    /// note covers it.
    Realize { attr: String },
    /// F13 startup reconcile: make the system provably match the declared
    /// package list (see [`crate::applier::ensure_applied`]). Serialized on
    /// the mutation thread like an install; answered with
    /// `Done { id: RECONCILE_ID, .. }`. `force` re-trips the apply watch
    /// even when the status file claims applied (profile drift).
    EnsureApplied { force: bool },
}

/// The `Done` id an [`Request::EnsureApplied`] run answers with. Contains
/// `:` — an invalid attr character — so it can never collide with a real
/// package install's id.
pub const RECONCILE_ID: &str = "waverunner:reconcile";

/// What a [`Event::Done`] completed. Carried IN the event so routing can
/// never depend on racy daemon-side maps (#57).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DoneOp {
    Install,
    /// The uninstall's package attr rides along: the handler needs it for
    /// the managed-cache removal and the residue sweep even when the
    /// `uninstalling` map entry was already pruned by a rescan.
    Remove {
        attr: String,
    },
    Reconcile,
}

/// How many top-ranked packages a `Ranked` reply carries. The renderer
/// reserves this many texture-array layers for their icons.
pub const RANK_HITS_MAX: usize = 25;

/// How many packages can be mid-install *in the grid* at once. The
/// renderer reserves this many more texture-array layers (past the
/// ranked-hit tail) so a dropped package keeps its own icon while it
/// installs, independent of what the live Install search is showing.
pub const PENDING_INSTALL_CAP: usize = 8;

/// The Install section's storefront: household names shown before any
/// query is typed, best-known first. Attrs missing from the index
/// (renames, channel drift) are skipped silently.
const RECOMMENDED: [&str; 25] = [
    "firefox",
    "chromium",
    "brave",
    "vlc",
    "mpv",
    "gimp",
    "inkscape",
    "krita",
    "blender",
    "obs-studio",
    "audacity",
    "kdePackages.kdenlive",
    "libreoffice",
    "thunderbird",
    "telegram-desktop",
    "signal-desktop",
    "discord",
    "spotify",
    "steam",
    "vscode",
    "ghostty",
    "alacritty",
    "fastfetch",
    "obsidian",
    "darktable",
];

/// The recommendation entries present in the index, in list order.
fn recommended_hits(pkgs: &[PkgEntry]) -> Vec<PkgEntry> {
    RECOMMENDED
        .iter()
        .filter_map(|attr| pkgs.iter().find(|p| &p.attr == attr).cloned())
        .take(RANK_HITS_MAX)
        .collect()
}

/// Handle to the nix threads; dropping it stops them after the work in
/// flight.
pub struct Nix {
    ranks: mpsc::Sender<String>,
    icons: mpsc::Sender<IconJob>,
    details: mpsc::Sender<String>,
    mutations: mpsc::Sender<Request>,
    realizes: mpsc::Sender<String>,
}

impl Nix {
    /// Queue a request. Dead threads make this a no-op (their reply
    /// channel is gone with them, so no state is left dangling). Ranking,
    /// profile mutations (install/uninstall), and "try it" builds each go
    /// to their own worker so none can stall behind another.
    pub fn request(&self, request: Request) {
        match request {
            Request::Rank { query } => {
                let _ = self.ranks.send(query);
            }
            Request::Icons { query, pkgs } => {
                let _ = self.icons.send(IconJob::Icons { query, pkgs });
            }
            Request::Details { attr } => {
                let _ = self.details.send(attr);
            }
            Request::Realize { attr } => {
                let _ = self.realizes.send(attr);
            }
            other => {
                let _ = self.mutations.send(other);
            }
        }
    }
}

/// Spawn the nix threads. Three workers, so none stalls behind another:
/// the **index** thread loads/refreshes the package index and serves rank
/// queries (coalesced to the newest, so slow ranking never queues behind
/// typing) plus hit-icon rasterization; the **mutation** thread runs
/// install/uninstall one at a time (a `nixos-rebuild switch` must not race
/// another); the **try-it** thread runs the ephemeral `nix build`s. Try-it
/// gets its own worker because a build can take minutes and must not hold
/// up an install queued behind it — a read-only realize runs fine
/// alongside a rebuild, as the nix daemon serializes store access itself.
pub fn spawn(events: Sender<Event>, icon_theme: String) -> Nix {
    let (ranks, ranks_rx) = mpsc::channel::<String>();
    let (icons, icons_rx) = mpsc::channel::<IconJob>();
    let (details, details_rx) = mpsc::channel::<String>();
    let (mutations, mutations_rx) = mpsc::channel::<Request>();
    let (realizes, realizes_rx) = mpsc::channel::<String>();

    let rank_events = events.clone();
    let warm = icons.clone();
    let spawned = std::thread::Builder::new()
        .name("waverunner-nix".into())
        .spawn(move || index_and_rank(rank_events, ranks_rx, warm));
    if let Err(e) = spawned {
        warn!("failed to spawn nix index thread: {e}");
    }

    let icon_events = events.clone();
    let spawned = std::thread::Builder::new()
        .name("waverunner-pkg-icons".into())
        .spawn(move || hit_icons(icon_events, icons_rx, icon_theme));
    if let Err(e) = spawned {
        warn!("failed to spawn the package icons' thread: {e}");
    }

    // A package's page: what is known of it, asked on a thread of its own
    // (nix can take a second to answer, and that must hold nothing up).
    let detail_events = events.clone();
    let spawned = std::thread::Builder::new()
        .name("waverunner-pkg-details".into())
        .spawn(move || {
            while let Ok(attr) = details_rx.recv() {
                let details = details_of(&attr);
                if detail_events.send(Event::Details { attr, details }).is_err() {
                    return;
                }
            }
        });
    if let Err(e) = spawned {
        warn!("failed to spawn the package details' thread: {e}");
    }

    let mut_events = events.clone();
    let spawned = std::thread::Builder::new()
        .name("waverunner-nix-mut".into())
        .spawn(move || {
            while let Ok(request) = mutations_rx.recv() {
                // One rebuild at a time: a switch never races the
                // generation it applies. But ops that QUEUED while a
                // rebuild ran are drained and COALESCED (#52): their list
                // edits fold into one write and one rebuild instead of N
                // sequential ones (~40 s each on the map machines). The
                // first drag still starts building immediately — draining
                // only ever picks up what accumulated behind it.
                let mut ops: Vec<(String, crate::applier::BatchOp)> = Vec::new();
                let mut reconcile: Option<bool> = None;
                let queue_op =
                    |req: Request, ops: &mut Vec<(String, crate::applier::BatchOp)>| match req {
                        Request::Install { id, attr } => {
                            ops.push((
                                id,
                                crate::applier::BatchOp {
                                    attr,
                                    install: true,
                                },
                            ));
                            None
                        }
                        Request::Remove { id, attr } => {
                            ops.push((
                                id,
                                crate::applier::BatchOp {
                                    attr,
                                    install: false,
                                },
                            ));
                            None
                        }
                        Request::EnsureApplied { force } => Some(force),
                        // Routed to their own threads; a batch is opened
                        // below, before it gets here.
                        Request::Rank { .. }
                        | Request::Icons { .. }
                        | Request::Details { .. }
                        | Request::Realize { .. }
                        | Request::Batch(_) => None,
                    };
                let mut waiting = vec![request];
                while let Ok(more) = mutations_rx.try_recv() {
                    waiting.push(more);
                }
                let each = waiting.into_iter().flat_map(|r| match r {
                    Request::Batch(inside) => inside,
                    one => vec![one],
                });
                for one in each {
                    reconcile = queue_op(one, &mut ops).or(reconcile);
                }
                match ops.len() {
                    0 => {}
                    1 => {
                        // Single op: the existing per-op path, unchanged
                        // semantics (fast paths, per-attr revert, logs).
                        let (id, op) = ops.remove(0);
                        let ok = if op.install {
                            crate::applier::apply_install(&op.attr)
                        } else {
                            crate::applier::apply_uninstall(&op.attr)
                        };
                        let done_op = if op.install {
                            DoneOp::Install
                        } else {
                            DoneOp::Remove {
                                attr: op.attr.clone(),
                            }
                        };
                        if mut_events
                            .send(Event::Done {
                                id,
                                op: done_op,
                                ok,
                                desktop_ids: None,
                            })
                            .is_err()
                        {
                            return;
                        }
                    }
                    _ => {
                        let batch: Vec<crate::applier::BatchOp> = ops
                            .iter()
                            .map(|(_, o)| crate::applier::BatchOp {
                                attr: o.attr.clone(),
                                install: o.install,
                            })
                            .collect();
                        let results = crate::applier::apply_batch(&batch);
                        for ((id, o), ok) in ops.into_iter().zip(results) {
                            let done_op = if o.install {
                                DoneOp::Install
                            } else {
                                DoneOp::Remove {
                                    attr: o.attr.clone(),
                                }
                            };
                            if mut_events
                                .send(Event::Done {
                                    id,
                                    op: done_op,
                                    ok,
                                    desktop_ids: None,
                                })
                                .is_err()
                            {
                                return;
                            }
                        }
                    }
                }
                if let Some(force) = reconcile {
                    let ok = crate::applier::ensure_applied(force);
                    if mut_events
                        .send(Event::Done {
                            id: RECONCILE_ID.to_owned(),
                            op: DoneOp::Reconcile,
                            ok,
                            desktop_ids: None,
                        })
                        .is_err()
                    {
                        return;
                    }
                }
            }
        });
    if let Err(e) = spawned {
        warn!("failed to spawn nix mutation thread: {e}");
    }

    let spawned = std::thread::Builder::new()
        .name("waverunner-nix-try".into())
        .spawn(move || {
            while let Ok(attr) = realizes_rx.recv() {
                let r = realize(&attr);
                let event = Event::Realized {
                    attr,
                    ok: r.ok,
                    terminal: r.terminal,
                    program: r.program,
                    version: r.version,
                };
                if events.send(event).is_err() {
                    return;
                }
            }
        });
    if let Err(e) = spawned {
        warn!("failed to spawn nix try-it thread: {e}");
    }

    Nix {
        ranks,
        icons,
        details,
        mutations,
        realizes,
    }
}

/// Body of the index thread: cache load, background refresh, then rank
/// service until the daemon goes away.
fn index_and_rank(events: Sender<Event>, ranks: mpsc::Receiver<String>, icons: mpsc::Sender<IconJob>) {
    // Park until the first rank request — nothing about the package index is
    // loaded until the user actually opens package search. The parsed index is
    // ~8.6 MB resident (23,673 entries) plus the icon-availability sweep; a
    // session that never searches nixpkgs pays none of it. The daemon kicks
    // this by sending one Rank the first time the Install section is shown.
    let first_query = match ranks.recv() {
        Ok(q) => q,
        Err(_) => return, // daemon gone before any search
    };
    let cache = cache_path();
    // A distribution-prebuilt index (WAVERUNNER_PKG_INDEX, set by the
    // flake) is authoritative: it was built from the exact nixpkgs the
    // system installs from, so a fresh dump could only diverge — and it
    // spares the machine the ~3GB `nix search` eval entirely (which
    // OOM-crash-looped the whole service on a 4G machine, 2026-08-30).
    let mut from_prebuilt = false;
    let mut pkgs = Vec::new();
    if let Some(path) = std::env::var("WAVERUNNER_PKG_INDEX")
        .ok()
        .filter(|p| !p.is_empty())
    {
        match load_cache(Path::new(&path)) {
            Ok(p) if !p.is_empty() => {
                info!("nixpkgs index: {} packages (prebuilt {path})", p.len());
                if events.send(Event::IndexReady(p.len())).is_err() {
                    return;
                }
                pkgs = p;
                from_prebuilt = true;
            }
            Ok(_) => warn!("prebuilt index is empty, ignoring: {path}"),
            Err(e) => warn!("prebuilt index unreadable ({path}): {e:#}"),
        }
    }
    if !from_prebuilt {
        pkgs = match load_cache(&cache) {
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
    }
    // A cache that failed to load (missing, unreadable, or an old
    // format) must dump regardless of its file age.
    if !from_prebuilt
        && (pkgs.is_empty() || cache_age(&cache).is_none_or(|age| age > REFRESH_AFTER))
    {
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

    // The icon thread readies its theme meanwhile: the first icons are
    // asked for a moment from now.
    let _ = icons.send(IconJob::Warm);
    let mut searcher = waverunner_core::Searcher::new();
    let mut pending: Option<String> = Some(first_query);
    loop {
        let mut query = match pending.take() {
            Some(query) => query,
            None => match ranks.recv() {
                Ok(query) => query,
                Err(_) => return,
            },
        };
        // Typing outruns ranking: serve only the newest queued query.
        while let Ok(newer) = ranks.try_recv() {
            query = newer;
        }
        let started = std::time::Instant::now();
        // The empty query is the storefront: curated recommendations
        // instead of a rank over everything.
        let hits: Vec<PkgEntry> = if query.is_empty() {
            recommended_hits(&pkgs)
        } else {
            rank_hits(&mut searcher, &query, &pkgs)
                .into_iter()
                .map(|i| pkgs[i].clone())
                .collect()
        };
        debug!(
            "pkg rank {query:?}: {} hits of {} in {:?}",
            hits.len(),
            pkgs.len(),
            started.elapsed()
        );
        if events.send(Event::Ranked { query, hits }).is_err() {
            return;
        }
    }
}

/// What the icon thread is asked.
enum IconJob {
    /// Ready the theme (the index was just loaded: icons are wanted soon).
    Warm,
    Icons { query: String, pkgs: Vec<PkgEntry> },
}

/// Icons sent together at most this many (the first page of the Install
/// row), or after this long since the first of them.
const ICONS_TOGETHER: usize = 6;
const ICONS_WAIT: Duration = Duration::from_millis(12);

/// THE PACKAGES' ICONS, on a thread of their own (they were the rank
/// thread's: a query's icons — and the downloads of them — held the next
/// query's results back, and every new answer showed the installer box
/// until all its icons came at once). What is on this computer, a few at a
/// time in the order asked; what must be downloaded goes to a thread of its
/// own (`fetch_icons`), so a download for an older query never holds the
/// newest one's icons back.
fn hit_icons(events: Sender<Event>, jobs: mpsc::Receiver<IconJob>, icon_theme: String) {
    // Parked until the index is loaded or icons are asked.
    let mut job = match jobs.recv() {
        Ok(job) => job,
        Err(_) => return,
    };
    let (fetches, fetch_rx) = mpsc::channel::<(String, Vec<PkgEntry>)>();
    let fetch_events = events.clone();
    if let Err(e) = std::thread::Builder::new()
        .name("waverunner-pkg-fetch".into())
        .spawn(move || fetch_icons(fetch_events, fetch_rx))
    {
        warn!("failed to spawn the package icon downloads' thread: {e}");
    }
    let mut loader = crate::apps::IconLoader::new(icon_theme);
    // One sweep over the theme dirs collects every icon name that
    // exists at all: hits get a real lookup, everything else becomes a
    // letter tile instantly instead of paying a full (negative) theme
    // walk per new name — that walk was why a repeated search felt so
    // much faster than a first one.
    let mut icon_names = IconNames::sweep(loader.themes());
    loop {
        let (query, pkgs) = match job {
            IconJob::Warm => (String::new(), Vec::new()),
            IconJob::Icons { query, pkgs } => (query, pkgs),
        };
        let mut next: Option<IconJob> = None;
        let asked = std::time::Instant::now();
        // A fresh install drops its bundled icon into hicolor: re-sweep
        // when stale so it shows up without a daemon restart.
        if !pkgs.is_empty() && icon_names.is_stale() {
            icon_names = IconNames::sweep(loader.themes());
        }
        // A newer request (not a warm-up) takes over.
        let newer = |next: &mut Option<IconJob>| loop {
            match jobs.try_recv() {
                Ok(IconJob::Warm) => continue,
                Ok(job) => {
                    *next = Some(job);
                    return true;
                }
                Err(_) => return next.is_some(),
            }
        };
        let send = |icons: Vec<HitIcon>| {
            if icons.is_empty() {
                return true;
            }
            debug!("pkg icons {query:?}: {} at {:?}", icons.len(), asked.elapsed());
            events.send(Event::HitIcons { query: query.clone(), icons }).is_ok()
        };
        // Icon sources compose like an app center's: local themes
        // first (Papirus and friends ship icons for far more apps than
        // are installed), then Flathub's pre-extracted icon catalog by
        // desktop-file id and the package's own icon streamed out of
        // the binary cache — kept on disk once fetched, so read here
        // too; only what was never fetched waits for the network.
        let mut batch: Vec<HitIcon> = Vec::new();
        let mut since = std::time::Instant::now();
        let mut fetch: Vec<PkgEntry> = Vec::new();
        let mut gone = false;
        for p in &pkgs {
            if newer(&mut next) {
                break;
            }
            let local = icon_candidates(p)
                .into_iter()
                .find_map(|c| icon_names.resolve(&c))
                .map(|icon| {
                    loader.icon_for(&waverunner_core::index::AppEntry {
                        id: format!("pkg-{}", p.attr),
                        name: p.name.clone(),
                        description: None,
                        exec: String::new(),
                        icon: Some(icon),
                        startup_wm_class: None,
                        needs_terminal: false,
                        path: None,
                    })
                })
                .or_else(|| downloaded_icon(p, &mut 0, &mut 0).map(|pixels| (pixels, false)));
            match local {
                Some((pixels, placeholder)) => {
                    if batch.is_empty() {
                        since = std::time::Instant::now();
                    }
                    batch.push(HitIcon { attr: p.attr.clone(), pixels, placeholder });
                }
                None => fetch.push(p.clone()),
            }
            let due = batch.len() >= ICONS_TOGETHER || !batch.is_empty() && since.elapsed() >= ICONS_WAIT;
            if due && !send(std::mem::take(&mut batch)) {
                gone = true;
                break;
            }
        }
        if gone || !send(std::mem::take(&mut batch)) {
            return;
        }
        loader.save_resolutions();
        // (Asked even when empty: the downloads of an older query stop.)
        if next.is_none() && !pkgs.is_empty() {
            let _ = fetches.send((query.clone(), fetch));
        }
        job = match next {
            Some(job) => job,
            None => match jobs.recv() {
                Ok(job) => job,
                Err(_) => return,
            },
        };
    }
}

/// The DOWNLOADS of the packages' icons, one by one in the order asked,
/// given up between two the moment a newer query's list comes; each is kept
/// on the disk, where the icon thread finds it the next time.
fn fetch_icons(events: Sender<Event>, jobs: mpsc::Receiver<(String, Vec<PkgEntry>)>) {
    let mut pending: Option<(String, Vec<PkgEntry>)> = None;
    loop {
        let mut job = match pending.take() {
            Some(job) => job,
            None => match jobs.recv() {
                Ok(job) => job,
                Err(_) => return,
            },
        };
        // (Only the newest list asked counts.)
        while let Ok(newer) = jobs.try_recv() {
            job = newer;
        }
        let (query, pkgs) = job;
        let started = std::time::Instant::now();
        let mut flathub_budget = FLATHUB_FETCH_BUDGET;
        let mut store_budget = STORE_FETCH_BUDGET;
        for p in &pkgs {
            if let Ok(newer) = jobs.try_recv() {
                pending = Some(newer);
                break;
            }
            if let Some(pixels) = downloaded_icon(p, &mut flathub_budget, &mut store_budget) {
                debug!("pkg icon {} downloaded for {query:?} at {:?}", p.attr, started.elapsed());
                let icons = vec![HitIcon { attr: p.attr.clone(), pixels, placeholder: false }];
                if events.send(Event::HitIcons { query: query.clone(), icons }).is_err() {
                    return;
                }
            }
        }
    }
}

/// A package's icon from Flathub's catalog or out of the binary cache:
/// what was fetched before is read from the disk; budgets of 0 fetch
/// nothing new.
fn downloaded_icon(p: &PkgEntry, flathub: &mut u32, store: &mut u32) -> Option<Vec<u8>> {
    p.icons
        .iter()
        .filter(|hint| hint.contains('.'))
        .find_map(|id| fetch_flathub_icon(id, flathub))
        .or_else(|| p.icon_path.as_deref().and_then(|path| fetch_store_icon(path, store)))
}

/// The top `RANK_HITS_MAX` package indices for `query`, name matches
/// first. nucleo scores a word-boundary match in a description the
/// same as one on the name (ties then break alphabetically), which
/// buried `firefox` itself under dozens of packages that merely
/// mention Firefox — so names rank in their own pass and always beat
/// description-only matches, which remain as discovery fill (a "media
/// player" query still finds players).
fn rank_hits(
    searcher: &mut waverunner_core::Searcher,
    query: &str,
    pkgs: &[PkgEntry],
) -> Vec<usize> {
    let names: Vec<&str> = pkgs.iter().map(|p| p.name.as_str()).collect();
    let mut hits: Vec<usize> = searcher
        .rank(query, &names)
        .into_iter()
        .take(RANK_HITS_MAX)
        .collect();
    if hits.len() < RANK_HITS_MAX {
        let keys: Vec<&str> = pkgs.iter().map(|p| p.haystack.as_str()).collect();
        for i in searcher.rank(query, &keys) {
            if !hits.contains(&i) {
                hits.push(i);
                if hits.len() == RANK_HITS_MAX {
                    break;
                }
            }
        }
    }
    hits
}

/// Generic fallback icon names for packages nothing else matched — the
/// theme's own default package face, best first. A cell wears the dock's
/// own of it (`apps::ICON_ASSETS`' `asset-pkg`, `package-x-generic`) until
/// its icon comes, and for good when it has none.
///
/// `package-x-generic` leads deliberately: it is the freedesktop default a
/// package wears everywhere else on the system, so the Install section
/// looks like the rest of the desktop rather than like our own idea of a
/// package. `system-software-install` used to lead (commit 1bc4ab1) and was
/// dropped (Max, 2026-09-13: "let's use default, keep it simple") — it is
/// an app-*store* icon, a multicoloured white box, where the default is the
/// plain green zippered one.
#[cfg(test)]
const GENERIC_PKG_ICONS: [&str; 3] = ["package-x-generic", "package", "application-x-executable"];

/// Re-sweep the icon availability map when older than this: a freshly
/// installed package drops its bundled icon into hicolor, and the next
/// search should pick it up without a daemon restart.
const ICON_SWEEP_MAX_AGE: Duration = Duration::from_secs(300);

/// Flathub's AppStream icon CDN — the same pre-extracted icon catalog
/// every distro app center downloads, keyed by the reverse-DNS app id.
const FLATHUB_ICON_BASE: &str = "https://dl.flathub.org/repo/appstream/x86_64/icons/64x64";
/// Retry a recorded Flathub miss (404) after this long.
const FLATHUB_MISS_RETRY: Duration = Duration::from_secs(30 * 24 * 60 * 60);
/// New downloads allowed per icon batch: keeps a page of all-new hits
/// from stalling the batch on the network; the rest catch up on later
/// queries (everything fetched is cached forever).
const FLATHUB_FETCH_BUDGET: u32 = 6;

/// The binary cache that built the nix-index database's store paths —
/// `nix store cat` streams single files (a package's own icon) out of
/// it without installing anything.
const BINARY_CACHE: &str = "https://cache.nixos.org";
/// Skip store-icon fetches whose compressed NAR is larger than this
/// (the whole NAR transits to extract one file).
const STORE_ICON_NAR_MAX: u64 = 20 * 1024 * 1024;
/// New store-icon downloads per icon batch (each may pull a few MB).
const STORE_FETCH_BUDGET: u32 = 3;

/// The icon names installed on this system, from one readdir sweep:
/// lowercased stem → actual lookup name, plus reverse-DNS aliases —
/// apps ship their icons under vendor ids no pname can guess
/// (`com.mitchellh.ghostty` becomes reachable as `ghostty`).
struct IconNames {
    names: std::collections::HashMap<String, String>,
    aliases: std::collections::HashMap<String, String>,
    built: std::time::Instant,
}

impl IconNames {
    fn sweep(themes: &[String]) -> Self {
        let started = std::time::Instant::now();
        let names = crate::apps::available_icon_names(themes);
        let mut aliases = std::collections::HashMap::new();
        for (lower, actual) in &names {
            if let Some((_, last)) = lower.rsplit_once('.') {
                // Guard tiny tails ("com.foo.x") from hijacking names.
                if last.len() >= 3 {
                    aliases
                        .entry(last.to_owned())
                        .or_insert_with(|| actual.clone());
                }
            }
        }
        debug!(
            "icon sweep: {} names, {} reverse-DNS aliases in {:?}",
            names.len(),
            aliases.len(),
            started.elapsed()
        );
        Self {
            names,
            aliases,
            built: started,
        }
    }

    fn is_stale(&self) -> bool {
        self.built.elapsed() > ICON_SWEEP_MAX_AGE
    }

    /// Actual lookup name for a candidate: exact (case-insensitive)
    /// stems first, reverse-DNS tails second.
    fn resolve(&self, candidate: &str) -> Option<String> {
        let lower = candidate.to_lowercase();
        self.names
            .get(&lower)
            .or_else(|| self.aliases.get(&lower))
            .cloned()
    }
}

/// Fetch a package icon from Flathub's AppStream CDN, disk-cached
/// under `$XDG_CACHE_HOME/waverunner/flathub-icons/` (misses recorded
/// as `.miss` markers and retried monthly; network errors leave no
/// marker so the next batch retries). `budget` caps new downloads per
/// batch. Returns rasterized `ICON_SIZE`² pixels.
fn fetch_flathub_icon(id: &str, budget: &mut u32) -> Option<Vec<u8>> {
    // Ids are reverse-DNS ([A-Za-z0-9._-]); anything else stays off
    // the filesystem and the URL.
    if !id
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-'))
    {
        return None;
    }
    let dir = cache_path().parent()?.join("flathub-icons");
    let png = dir.join(format!("{id}.png"));
    if png.exists() {
        return crate::apps::rasterize_icon_file(png.to_str()?, id);
    }
    let miss = dir.join(format!("{id}.miss"));
    if cache_age(&miss).is_some_and(|age| age < FLATHUB_MISS_RETRY) {
        return None;
    }
    if *budget == 0 {
        return None;
    }
    *budget -= 1;
    std::fs::create_dir_all(&dir).ok()?;
    let tmp = dir.join(format!("{id}.part"));
    let status = Command::new("curl")
        .args(["-fsSL", "--max-time", "5"])
        .arg(format!("{FLATHUB_ICON_BASE}/{id}.png"))
        .arg("-o")
        .arg(&tmp)
        .status()
        .ok()?;
    if status.success() {
        std::fs::rename(&tmp, &png).ok()?;
        debug!("flathub icon fetched: {id}");
        return crate::apps::rasterize_icon_file(png.to_str()?, id);
    }
    let _ = std::fs::remove_file(&tmp);
    // curl exits 22 on HTTP errors (404 = not on Flathub): remember
    // that; anything else (offline, timeout) retries next batch.
    if status.code() == Some(22) {
        let _ = std::fs::write(&miss, b"");
    }
    None
}

/// Stream a package's own icon file out of the binary cache, disk-
/// cached under `$XDG_CACHE_HOME/waverunner/store-icons/`. The whole
/// (compressed) NAR transits to extract the one file, so a narinfo
/// pre-check skips packages larger than [`STORE_ICON_NAR_MAX`], and
/// misses of any kind leave a monthly-retried marker. Returns
/// rasterized `ICON_SIZE`² pixels.
fn fetch_store_icon(store_path: &str, budget: &mut u32) -> Option<Vec<u8>> {
    // `/nix/store/<32-char-hash>-name/...` — the hash keys everything.
    let hash = store_path.strip_prefix("/nix/store/")?.get(..32)?;
    if !hash.chars().all(|c| c.is_ascii_alphanumeric()) {
        return None;
    }
    let ext = std::path::Path::new(store_path)
        .extension()
        .and_then(|e| e.to_str())?;
    let dir = cache_path().parent()?.join("store-icons");
    let file = dir.join(format!("{hash}.{ext}"));
    if file.exists() {
        return crate::apps::rasterize_icon_file(file.to_str()?, store_path);
    }
    let miss = dir.join(format!("{hash}.miss"));
    if cache_age(&miss).is_some_and(|age| age < FLATHUB_MISS_RETRY) {
        return None;
    }
    if *budget == 0 {
        return None;
    }
    *budget -= 1;
    std::fs::create_dir_all(&dir).ok()?;
    let mark_miss = || {
        let _ = std::fs::write(&miss, b"");
    };
    // narinfo first: how big is the NAR this icon rides in?
    let info = Command::new("curl")
        .args(["-fsSL", "--max-time", "5"])
        .arg(format!("{BINARY_CACHE}/{hash}.narinfo"))
        .output()
        .ok()?;
    if !info.status.success() {
        mark_miss(); // not in the cache (or gone): remember that
        return None;
    }
    let too_big = String::from_utf8_lossy(&info.stdout)
        .lines()
        .find_map(|l| l.strip_prefix("FileSize: ")?.trim().parse::<u64>().ok())
        .is_none_or(|size| size > STORE_ICON_NAR_MAX);
    if too_big {
        mark_miss();
        return None;
    }
    let out = Command::new("nix")
        .args(["store", "cat", "--store", BINARY_CACHE, store_path])
        .output()
        .ok()?;
    if !out.status.success() || out.stdout.is_empty() {
        mark_miss();
        return None;
    }
    std::fs::write(&file, &out.stdout).ok()?;
    debug!("store icon fetched: {store_path}");
    crate::apps::rasterize_icon_file(file.to_str()?, store_path)
}

/// Icon names worth trying for a package, best first: the package's
/// own `.desktop` stems (authoritative, from the nix-index file
/// database), the pname, the reverse-DNS aliases KDE and GNOME apps
/// publish their icons under (`kdePackages.kate` → `org.kde.kate`,
/// `gnome-calculator` → `org.gnome.Calculator`), then the pname with
/// trailing `-segments` progressively stripped so variants inherit the
/// family icon (`firefox-bin` → `firefox`, `telegram-desktop-bin` →
/// `telegram-desktop`).
fn icon_candidates(pkg: &PkgEntry) -> Vec<String> {
    let mut candidates = pkg.icons.clone();
    candidates.push(pkg.name.clone());
    if let Some(rest) = pkg.attr.strip_prefix("kdePackages.") {
        candidates.push(format!("org.kde.{rest}"));
    }
    if let Some(rest) = pkg.name.strip_prefix("gnome-") {
        let camel: String = rest
            .split('-')
            .map(|word| {
                let mut chars = word.chars();
                match chars.next() {
                    Some(first) => first.to_uppercase().chain(chars).collect::<String>(),
                    None => String::new(),
                }
            })
            .collect();
        candidates.push(format!("org.gnome.{camel}"));
    }
    let mut base = pkg.name.as_str();
    for _ in 0..2 {
        let Some((stripped, _)) = base.rsplit_once('-') else {
            break;
        };
        if stripped.len() < 3 {
            break;
        }
        candidates.push(stripped.to_owned());
        base = stripped;
    }
    candidates
}

/// What a `realize` produced: whether the build succeeded, whether the
/// package is a terminal tool, and (for a terminal tool) its main program
/// name and version for the "try it" shell banner.
#[derive(Default)]
struct Realized {
    ok: bool,
    terminal: bool,
    program: Option<String>,
    version: Option<String>,
}

/// `nix build` a package into the store so a following `nix run` /
/// `nix shell` starts it instantly (the slow, cacheable part of a
/// "try it" launch), and read back what the daemon needs to present it.
fn realize(attr: &str) -> Realized {
    info!("nix build nixpkgs#{attr}");
    let out = match Command::new("nix")
        .args([
            "build",
            "--no-link",
            "--print-out-paths",
            "--impure",
            &format!("nixpkgs#{attr}"),
        ])
        .env("NIXPKGS_ALLOW_UNFREE", "1")
        .output()
    {
        Ok(out) if out.status.success() => out,
        Ok(out) => {
            warn!(
                "realize of {attr} failed: {}",
                String::from_utf8_lossy(&out.stderr).trim()
            );
            return Realized::default();
        }
        Err(e) => {
            warn!("cannot run nix: {e}");
            return Realized::default();
        }
    };
    let paths = String::from_utf8_lossy(&out.stdout);
    let terminal = wants_terminal(paths.lines());
    // Only a terminal tool needs a program/version for its shell banner;
    // a GUI app just launches its window.
    let (program, version) = if terminal {
        main_program(attr, paths.lines())
    } else {
        (None, None)
    };
    Realized {
        ok: true,
        terminal,
        program,
        version,
    }
}

/// The package's main program and version, read from its built outputs:
/// the output carrying a `bin/` dir names the binary (preferring one that
/// matches the attr, e.g. `ripgrep`→`rg` falls through to the lone entry),
/// and that output's store-path name carries the version.
fn main_program<'a>(
    attr: &str,
    store_paths: impl Iterator<Item = &'a str>,
) -> (Option<String>, Option<String>) {
    // The install handle's leaf (`kdePackages.kdenlive` → `kdenlive`) is
    // the best guess at the primary binary's name.
    let leaf = attr.rsplit('.').next().unwrap_or(attr);
    for path in store_paths {
        let path = path.trim();
        let Ok(entries) = std::fs::read_dir(Path::new(path).join("bin")) else {
            continue; // -man / -dev / -lib outputs have no bin/
        };
        let mut bins: Vec<String> = entries
            .flatten()
            .filter_map(|e| e.file_name().into_string().ok())
            .collect();
        if bins.is_empty() {
            continue;
        }
        bins.sort_unstable();
        // Prefer the binary named like the package; else the shortest
        // name (usually the primary command among helpers).
        let program = bins
            .iter()
            .find(|b| b.as_str() == leaf)
            .cloned()
            .or_else(|| bins.iter().min_by_key(|b| b.len()).cloned());
        return (program, version_from_store_path(path));
    }
    (None, None)
}

/// Parse the version out of a `/nix/store/<hash>-<name>-<version>` path:
/// strip the 32-char hash, then take everything from the first
/// digit-leading dash-component on (`fastfetch-2.65.2` → `2.65.2`).
fn version_from_store_path(path: &str) -> Option<String> {
    let base = Path::new(path).file_name()?.to_str()?;
    let name_ver = base.get(33..)?; // 32-char hash + '-'
    let idx = name_ver
        .split('-')
        .position(|c| c.starts_with(|ch: char| ch.is_ascii_digit()))?;
    name_ver.splitn(idx + 1, '-').last().map(str::to_owned)
}

/// Whether a built package should be run in a terminal: it ships a
/// `.desktop` with `Terminal=true` (a TUI like btop/htop), or it ships
/// no `.desktop` at all (a plain CLI). A `.desktop` without `Terminal=true`
/// is a GUI app — run it headless.
fn wants_terminal<'a>(store_paths: impl Iterator<Item = &'a str>) -> bool {
    let mut saw_desktop = false;
    for path in store_paths {
        let apps = Path::new(path.trim()).join("share/applications");
        let Ok(entries) = std::fs::read_dir(&apps) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.extension().and_then(|e| e.to_str()) != Some("desktop") {
                continue;
            }
            saw_desktop = true;
            if let Ok(text) = std::fs::read_to_string(&path) {
                if text
                    .lines()
                    .any(|l| l.trim().eq_ignore_ascii_case("terminal=true"))
                {
                    return true;
                }
            }
        }
    }
    !saw_desktop
}

#[derive(Deserialize)]
struct SearchMeta {
    #[serde(default)]
    pname: String,
    #[serde(default)]
    version: String,
    #[serde(default)]
    description: String,
    // (The flake's index carries these too: `nix-env --meta`, flattened by
    // its jq; `nix search --json` has none of them.)
    #[serde(default)]
    homepage: String,
    #[serde(default)]
    license: String,
    #[serde(default)]
    program: String,
    #[serde(default)]
    long: String,
}

/// The curated cut of nixpkgs the Install section searches: top-level
/// attrs (the end-user catalog) plus `kdePackages` (real applications
/// live there). Language-ecosystem sets (`python313Packages`,
/// `haskellPackages`, `vimPlugins`, `texlivePackages`, …) are
/// libraries and plugins, not installable apps — they made four out of
/// five results noise.
fn keep_attr(attr: &str) -> bool {
    // Inner derivations of wrapper packages (firefox-unwrapped, …):
    // build inputs, not installables.
    if attr.ends_with("-unwrapped") {
        return false;
    }
    match attr.split_once('.') {
        None => true,
        Some((set, _)) => set == "kdePackages",
    }
}

/// Run `nix-locate` — straight from PATH when shipped (the flake's
/// runtimeTools), else via `nix shell nixpkgs#nix-index`, which costs a
/// full nixpkgs eval (~3GB) and is only acceptable on dev machines.
fn nix_locate(regex: &str) -> anyhow::Result<std::process::Output> {
    // WAVERUNNER_NIX_INDEX_DB: a directory holding the `files` database.
    // Set by the flake's package-index derivation (build sandbox: the
    // nix-index-database input, no download) — unset at runtime, where
    // ensure_file_index has downloaded into nix-locate's default dir.
    let db = std::env::var("WAVERUNNER_NIX_INDEX_DB").ok();
    let mut cmd = Command::new("nix-locate");
    if let Some(db) = &db {
        cmd.args(["--db", db]);
    }
    match cmd.args(["--regex", regex]).output() {
        Ok(out) => Ok(out),
        Err(_) => Ok(Command::new("nix")
            .args([
                "shell",
                "nixpkgs#nix-index",
                "-c",
                "nix-locate",
                "--regex",
                regex,
            ])
            .output()?),
    }
}

/// Make sure the prebuilt nix-index file database is present and
/// reasonably fresh at `~/.cache/nix-index/files` (where `nix-locate`
/// expects it).
fn ensure_file_index() -> anyhow::Result<()> {
    let home = std::env::var("HOME").unwrap_or_default();
    let dir = PathBuf::from(home).join(".cache").join("nix-index");
    let path = dir.join("files");
    if cache_age(&path).is_some_and(|age| age < FILE_INDEX_MAX_AGE) {
        return Ok(());
    }
    info!("downloading nix-index file database…");
    std::fs::create_dir_all(&dir)?;
    let tmp = dir.join("files.part");
    let out = Command::new("curl")
        .args(["-fsSL", FILE_INDEX_URL, "-o"])
        .arg(&tmp)
        .output()?;
    anyhow::ensure!(
        out.status.success(),
        "curl failed: {}",
        String::from_utf8_lossy(&out.stderr).trim().to_owned()
    );
    std::fs::rename(&tmp, &path)?;
    Ok(())
}

/// Every package's `.desktop` file stems (attr → stems), from one
/// `nix-locate` sweep over the file database. The stem doubles as the
/// app id and icon name for reverse-DNS-named apps — the authoritative
/// pname → icon mapping nothing else provides for uninstalled
/// packages.
fn desktop_icon_hints() -> anyhow::Result<std::collections::HashMap<String, Vec<String>>> {
    ensure_file_index()?;
    desktop_icon_hints_offline()
}

/// [`desktop_icon_hints`] without the database download — the caller
/// guarantees nix-locate can find one (WAVERUNNER_NIX_INDEX_DB in the
/// build sandbox).
fn desktop_icon_hints_offline() -> anyhow::Result<std::collections::HashMap<String, Vec<String>>> {
    let out = nix_locate(r"share/applications/[^/]*\.desktop$")?;
    anyhow::ensure!(
        out.status.success(),
        "nix-locate failed: {}",
        String::from_utf8_lossy(&out.stderr).trim().to_owned()
    );
    let mut hints: std::collections::HashMap<String, Vec<String>> =
        std::collections::HashMap::new();
    for line in String::from_utf8_lossy(&out.stdout).lines() {
        let mut cols = line.split_whitespace();
        let (Some(attr_out), Some(path)) = (cols.next(), cols.next_back()) else {
            continue;
        };
        if !path.contains("/share/applications/") {
            continue;
        }
        // `<attr>.<output>` — the store output suffix is always there.
        let attr = attr_out.rsplit_once('.').map_or(attr_out, |(a, _)| a);
        let Some(stem) = std::path::Path::new(path)
            .file_stem()
            .and_then(|s| s.to_str())
        else {
            continue;
        };
        let stems = hints.entry(attr.to_owned()).or_default();
        if stems.len() < 4 && !stems.iter().any(|s| s == stem) {
            stems.push(stem.to_owned());
        }
    }
    Ok(hints)
}

/// Preference of an in-package icon path: small raster sizes first
/// (they rasterize to `ICON_SIZE` anyway and download fastest), then
/// scalable, then huge rasters, pixmaps last.
fn icon_path_score(path: &str) -> u32 {
    for (i, marker) in [
        "/48x48/",
        "/64x64/",
        "/128x128/",
        "/scalable/",
        "/256x256/",
        "/512x512/",
    ]
    .iter()
    .enumerate()
    {
        if path.contains(marker) {
            return i as u32;
        }
    }
    if path.contains("/share/pixmaps/") {
        return 8;
    }
    7
}

/// Every package's best in-package icon file (attr → store path), from
/// one `nix-locate` sweep over hicolor and pixmaps.
fn package_icon_paths() -> anyhow::Result<std::collections::HashMap<String, String>> {
    let out = nix_locate(r"share/(icons/hicolor/[^/]*/apps|pixmaps)/[^/]*\.(png|svg)$")?;
    anyhow::ensure!(
        out.status.success(),
        "nix-locate failed: {}",
        String::from_utf8_lossy(&out.stderr).trim().to_owned()
    );
    let mut paths: std::collections::HashMap<String, String> = std::collections::HashMap::new();
    for line in String::from_utf8_lossy(&out.stdout).lines() {
        let mut cols = line.split_whitespace();
        let (Some(attr_out), Some(path)) = (cols.next(), cols.next_back()) else {
            continue;
        };
        if !path.starts_with("/nix/store/") {
            continue;
        }
        let attr = attr_out.rsplit_once('.').map_or(attr_out, |(a, _)| a);
        match paths.get(attr) {
            Some(best) if icon_path_score(best) <= icon_path_score(path) => {}
            _ => {
                paths.insert(attr.to_owned(), path.to_owned());
            }
        }
    }
    Ok(paths)
}

/// Run the full `nix search` dump and slim it into our own curated
/// database: junk package sets filtered out ([`keep_attr`]), sorted so
/// shallow/alphabetical attrs come first (ties in fuzzy score then
/// resolve to the likelier candidate), duplicate pname+version pairs
/// collapsed onto that first attr. Each package carries the icon-name
/// hints from its `.desktop` files.
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
    let desktops = desktop_icon_hints().unwrap_or_else(|e| {
        warn!("desktop-file hints unavailable: {e:#}");
        Default::default()
    });
    let icon_paths = package_icon_paths().unwrap_or_else(|e| {
        warn!("in-package icon paths unavailable: {e:#}");
        Default::default()
    });
    let pkgs = parse_search_json(&out.stdout, desktops, icon_paths)?;
    debug!("nixpkgs dump finished in {:?}", started.elapsed());
    Ok(pkgs)
}

/// `waverunner build-index <search-json> <out-tsv>`: turn a raw
/// `nix search --json` dump into the TSV cache the daemon loads
/// instantly. Run at flake build time (the launcher flake's
/// `package-index` derivation) so a fresh machine never evaluates
/// nixpkgs — the ~3GB eval OOM-crash-looped a 4G VM (2026-08-30).
/// With WAVERUNNER_NIX_INDEX_DB set (the nix-index-database input) the
/// index also carries desktop-file stems + in-package icon paths —
/// without them, multi-desktop suites can't resolve gui (libreoffice's
/// startcenter/writer/calc share no substring with the attr, so its
/// install landed as a CLI terminal tile, 2026-08-31).
pub fn build_index(json_path: &Path, out_path: &Path) -> anyhow::Result<()> {
    let json = std::fs::read(json_path)?;
    let (desktops, icon_paths) = if std::env::var("WAVERUNNER_NIX_INDEX_DB").is_ok() {
        let desktops = desktop_icon_hints_offline()?;
        let icon_paths = package_icon_paths()?;
        println!(
            "desktop hints for {} attrs, icon paths for {}",
            desktops.len(),
            icon_paths.len()
        );
        (desktops, icon_paths)
    } else {
        Default::default()
    };
    let pkgs = parse_search_json(&json, desktops, icon_paths)?;
    if let Some(dir) = out_path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    save_cache(out_path, &pkgs)?;
    println!("{} packages -> {}", pkgs.len(), out_path.display());
    // What each package's page says beyond the index (its site, licence,
    // long description), when the dump has it: beside the index, read only
    // as a page opens — the index the daemon keeps is unchanged.
    let details = search_json_details(&json, &pkgs)?;
    if !details.is_empty() {
        let path = details_path(out_path);
        save_details(&path, &details)?;
        println!("{} packages' details -> {}", details.len(), path.display());
    }
    Ok(())
}

/// The details file beside an index.
fn details_path(index: &Path) -> PathBuf {
    index.with_file_name("nixpkgs-details.tsv")
}

const DETAILS_HEADER: &str = "waverunner-nixpkgs-details-v1";

/// A field made one line of a TSV (and back): backslash, tab, new line escaped.
fn tsv_field(s: &str) -> String {
    s.replace('\\', "\\\\").replace('\t', "\\t").replace('\n', "\\n").replace('\r', "")
}

fn tsv_unfield(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut chars = s.chars();
    while let Some(c) = chars.next() {
        if c != '\\' {
            out.push(c);
            continue;
        }
        match chars.next() {
            Some('t') => out.push('\t'),
            Some('n') => out.push('\n'),
            Some(o) => out.push(o),
            None => {}
        }
    }
    out
}

/// The details of the kept packages out of a dump that carries them.
fn search_json_details(json: &[u8], pkgs: &[PkgEntry]) -> anyhow::Result<Vec<(String, PkgDetails)>> {
    let raw: std::collections::HashMap<String, SearchMeta> = serde_json::from_slice(json)?;
    let kept: std::collections::HashSet<&str> = pkgs.iter().map(|p| p.attr.as_str()).collect();
    let some = |s: String| Some(s.trim().to_owned()).filter(|s| !s.is_empty());
    let mut out: Vec<(String, PkgDetails)> = raw
        .into_iter()
        .filter_map(|(key, m)| {
            let attr = key.splitn(3, '.').nth(2)?.to_owned();
            let d = PkgDetails {
                description: m.description.trim().to_owned(),
                homepage: some(m.homepage),
                license: some(m.license),
                program: some(m.program),
                long: some(m.long),
            };
            let any = d.homepage.is_some() || d.license.is_some() || d.long.is_some() || d.program.is_some();
            (kept.contains(attr.as_str()) && any).then_some((attr, d))
        })
        .collect();
    out.sort_by(|a, b| a.0.cmp(&b.0));
    Ok(out)
}

fn save_details(path: &Path, details: &[(String, PkgDetails)]) -> anyhow::Result<()> {
    let mut text = String::from(DETAILS_HEADER);
    text.push('\n');
    let o = |s: &Option<String>| tsv_field(s.as_deref().unwrap_or(""));
    for (attr, d) in details {
        text.push_str(&format!(
            "{attr}\t{}\t{}\t{}\t{}\t{}\n",
            tsv_field(&d.description),
            o(&d.homepage),
            o(&d.license),
            o(&d.program),
            o(&d.long)
        ));
    }
    let tmp = path.with_extension("tsv.tmp");
    std::fs::write(&tmp, text)?;
    std::fs::rename(&tmp, path)?;
    Ok(())
}

/// `attr`'s line of a details file (read as a page opens: a scan of a few
/// MB, no copy kept).
fn details_in(path: &Path, attr: &str) -> Option<PkgDetails> {
    let text = std::fs::read_to_string(path).ok()?;
    let mut lines = text.lines();
    if lines.next() != Some(DETAILS_HEADER) {
        return None;
    }
    let want = format!("{attr}\t");
    let line = lines.find(|l| l.starts_with(&want))?;
    let mut cols = line.split('\t').skip(1).map(tsv_unfield);
    let mut next = || cols.next().filter(|s| !s.is_empty());
    let description = next().unwrap_or_default();
    Some(PkgDetails { description, homepage: next(), license: next(), program: next(), long: next() })
}

/// The index the daemon loads: the prebuilt one, else the cache.
fn index_path() -> PathBuf {
    std::env::var("WAVERUNNER_PKG_INDEX").ok().filter(|p| !p.is_empty()).map_or_else(cache_path, PathBuf::from)
}

/// An attr fit to name a file and a nix attribute.
fn plain_attr(attr: &str) -> bool {
    !attr.is_empty() && attr.len() <= 200 && attr.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-' | '+'))
}

/// What is known of `attr` for its page: the details file beside the index;
/// else what nix said of it before (`pkg-meta/<attr>.json`); else nix asked
/// now (its `meta` from this computer's nixpkgs, offline: under half a second)
/// and kept. The description falls back to the index's (clipped) one.
fn details_of(attr: &str) -> PkgDetails {
    if !plain_attr(attr) {
        return PkgDetails::default();
    }
    let index = index_path();
    let with_description = |mut d: PkgDetails| {
        if d.description.is_empty() {
            let text = std::fs::read_to_string(&index).unwrap_or_default();
            let want = format!("{attr}\t");
            d.description = text
                .lines()
                .find(|l| l.starts_with(&want))
                .and_then(|l| l.splitn(6, '\t').nth(5))
                .unwrap_or("")
                .to_owned();
        }
        d
    };
    if let Some(d) = details_in(&details_path(&index), attr) {
        return with_description(d);
    }
    let kept = cache_path().with_file_name("pkg-meta").join(format!("{attr}.json"));
    if let Some(d) = std::fs::read(&kept).ok().and_then(|b| serde_json::from_slice::<PkgDetails>(&b).ok()) {
        return with_description(d);
    }
    let asked = ask_nix_meta(attr);
    if let Some(d) = &asked {
        if let Some(dir) = kept.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        if let Ok(bytes) = serde_json::to_vec(d) {
            let tmp = kept.with_extension("json.tmp");
            if std::fs::write(&tmp, bytes).is_ok() {
                let _ = std::fs::rename(&tmp, &kept);
            }
        }
    }
    with_description(asked.unwrap_or_default())
}

/// The longest nix may take to say what a package is.
const META_PATIENCE: Duration = Duration::from_secs(10);

/// What a package's meta is made into, as the flake's index flattens it.
const META_FLAT: &str = r#"m: let one = x: if builtins.isAttrs x then (x.spdxId or x.shortName or x.fullName or "") else if builtins.isString x then x else ""; h = m.homepage or ""; l = m.license or null; in { description = m.description or ""; homepage = if builtins.isList h then (if h == [] then "" else builtins.head h) else h; license = if builtins.isList l then builtins.concatStringsSep " / " (map one l) else one l; program = m.mainProgram or ""; long = m.longDescription or ""; }"#;

/// `nix eval` of `attr`'s meta, from the nixpkgs this computer installs from
/// (`<nixpkgs>`: the system's channel or its flake) and OFFLINE — the dock asks
/// nothing of the network (`nixpkgs#…` went to the registry, which on the dev
/// box is no pin: GitHub's nixpkgs-unstable, downloaded). Each part of the
/// attr quoted (`plain_attr` keeps quotes and `${` out of it).
fn ask_nix_meta(attr: &str) -> Option<PkgDetails> {
    let mut cmd = Command::new("nix");
    cmd.args(["eval", "--offline", "--json", "--impure", "--expr", &meta_expr(attr)]);
    let started = std::time::Instant::now();
    let out = crate::controls::finish_within(cmd, true, META_PATIENCE)?;
    let f: SearchMeta = serde_json::from_str(&out).ok()?;
    let some = |s: String| Some(s.trim().to_owned()).filter(|s| !s.is_empty());
    debug!("nix meta of {attr} in {:?}", started.elapsed());
    Some(PkgDetails {
        description: f.description.trim().to_owned(),
        homepage: some(f.homepage),
        license: some(f.license),
        program: some(f.program),
        long: some(f.long),
    })
}

/// The expression `ask_nix_meta` evaluates for `attr`.
fn meta_expr(attr: &str) -> String {
    let path: String = attr.split('.').map(|part| format!(".\"{part}\"")).collect();
    format!("({META_FLAT}) ((import <nixpkgs> {{ config.allowUnfree = true; }}){path}.meta)")
}

/// The shared tail of a dump: parse the `nix search --json` map, keep
/// installable attrs, attach icon hints, sort shallow-first and dedupe
/// name+version pairs.
fn parse_search_json(
    json: &[u8],
    mut desktops: std::collections::HashMap<String, Vec<String>>,
    mut icon_paths: std::collections::HashMap<String, String>,
) -> anyhow::Result<Vec<PkgEntry>> {
    let raw: std::collections::HashMap<String, SearchMeta> = serde_json::from_slice(json)?;
    let total = raw.len();
    let mut pkgs: Vec<PkgEntry> = raw
        .into_iter()
        .filter_map(|(key, meta)| {
            // Keys look like `legacyPackages.x86_64-linux.<attr...>`.
            let attr = key.splitn(3, '.').nth(2)?.to_owned();
            if !keep_attr(&attr) || meta.pname.is_empty() {
                return None;
            }
            let icons = desktops.remove(&attr).unwrap_or_default();
            let icon_path = icon_paths.remove(&attr);
            Some(entry_full(
                attr,
                meta.pname,
                meta.version,
                &meta.description,
                icons,
                icon_path,
            ))
        })
        .collect();
    pkgs.sort_by(|a, b| {
        let depth = |p: &PkgEntry| p.attr.matches('.').count();
        depth(a).cmp(&depth(b)).then_with(|| a.attr.cmp(&b.attr))
    });
    let mut seen = std::collections::HashSet::new();
    pkgs.retain(|p| seen.insert((p.name.clone(), p.version.clone())));
    debug!("nixpkgs index: {} of {total} packages kept", pkgs.len());
    Ok(pkgs)
}

/// Build one entry with its pre-computed rank haystack.
/// [`entry_full`] without hints — the common shape in tests.
#[cfg(test)]
fn entry(attr: String, name: String, version: String, description: &str) -> PkgEntry {
    entry_full(attr, name, version, description, Vec::new(), None)
}

/// [`entry_full`] with icon-name hints only.
#[cfg(test)]
fn entry_with_icons(
    attr: String,
    name: String,
    version: String,
    description: &str,
    icons: Vec<String>,
) -> PkgEntry {
    entry_full(attr, name, version, description, icons, None)
}

fn entry_full(
    attr: String,
    name: String,
    version: String,
    description: &str,
    icons: Vec<String>,
    icon_path: Option<String>,
) -> PkgEntry {
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
        icons,
        icon_path,
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

/// TSV cache: header line, then
/// `attr\tversion\tname\ticon;hints\ticon-store-path\tdescription`.
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
            "{}\t{}\t{}\t{}\t{}\t{}\n",
            p.attr,
            p.version,
            p.name,
            p.icons.join(";"),
            p.icon_path.as_deref().unwrap_or(""),
            desc
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
            let mut cols = line.splitn(6, '\t');
            let attr = cols.next()?.to_owned();
            let version = cols.next()?.to_owned();
            let name = cols.next()?.to_owned();
            let icons: Vec<String> = cols
                .next()?
                .split(';')
                .filter(|s| !s.is_empty())
                .map(str::to_owned)
                .collect();
            let icon_path = Some(cols.next()?.to_owned()).filter(|p| !p.is_empty());
            let desc = cols.next().unwrap_or("");
            Some(entry_full(attr, name, version, desc, icons, icon_path))
        })
        .collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_packages_meta_is_asked_of_this_computers_nixpkgs_each_part_quoted() {
        let e = meta_expr("python3Packages.requests");
        assert!(e.contains(r#"(import <nixpkgs> { config.allowUnfree = true; })."python3Packages"."requests".meta"#), "{e}");
        assert!(meta_expr("gtk+3").contains(r#"."gtk+3".meta"#));
        assert!(!e.contains("nixpkgs#"));
    }

    #[test]
    fn a_packages_details_round_trip_beside_its_index() {
        let dir = std::env::temp_dir().join("waverunner-nix-details-test");
        let _ = std::fs::create_dir_all(&dir);
        let index = dir.join("nixpkgs-index.tsv");
        let gimp = PkgDetails {
            description: "GNU Image Manipulation Program".into(),
            homepage: Some("https://www.gimp.org/".into()),
            license: Some("GPL-3.0-or-later".into()),
            program: Some("gimp".into()),
            long: Some("Paint\tand\nedit \\ pictures".into()),
        };
        save_details(&details_path(&index), &[("gimp".into(), gimp.clone())]).unwrap();
        assert_eq!(details_in(&details_path(&index), "gimp"), Some(gimp));
        assert_eq!(details_in(&details_path(&index), "gim"), None);
        // The flake's dump flattened by its jq: what a page needs is kept.
        let json = br#"{"legacyPackages.x86_64-linux.krita":{"pname":"krita","version":"5.2","description":"Painting","homepage":"https://krita.org/","license":"GPL-3.0-only","program":"krita","long":""},
                        "legacyPackages.x86_64-linux.cowsay":{"pname":"cowsay","version":"3.8","description":"Cow"}}"#;
        let pkgs = parse_search_json(json, Default::default(), Default::default()).unwrap();
        let details = search_json_details(json, &pkgs).unwrap();
        assert_eq!(details.len(), 1, "a package with nothing beyond its description has no line");
        assert_eq!(details[0].1.homepage.as_deref(), Some("https://krita.org/"));
        assert!(!plain_attr("x; rm -rf") && plain_attr("kdePackages.kate") && plain_attr("gtk+3"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn cache_round_trips() {
        let dir = std::env::temp_dir().join("waverunner-nix-test");
        let path = dir.join("nixpkgs-index.tsv");
        let pkgs = vec![
            entry_full(
                "cowsay".into(),
                "cowsay".into(),
                "3.8.4".into(),
                "ASCII cow\twith\nnewline",
                vec!["org.example.Cowsay".into(), "cowsay-alt".into()],
                Some("/nix/store/abc-cowsay/share/pixmaps/cowsay.png".into()),
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
        assert_eq!(loaded[0].icons, pkgs[0].icons);
        assert_eq!(loaded[0].icon_path, pkgs[0].icon_path);
        assert_eq!(loaded[1].name, "c");
        assert!(loaded[1].icons.is_empty());
        assert_eq!(loaded[1].icon_path, None);
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn icon_path_score_prefers_small_rasters() {
        let s48 = icon_path_score("/nix/store/x-a/share/icons/hicolor/48x48/apps/a.png");
        let scalable = icon_path_score("/nix/store/x-a/share/icons/hicolor/scalable/apps/a.svg");
        let s512 = icon_path_score("/nix/store/x-a/share/icons/hicolor/512x512/apps/a.png");
        let pixmap = icon_path_score("/nix/store/x-a/share/pixmaps/a.png");
        assert!(s48 < scalable && scalable < s512 && s512 < pixmap);
    }

    /// Manual check of the binary-cache icon stream (network):
    /// `cargo test -p waverunner-daemon store_icon -- --ignored --nocapture`
    #[test]
    #[ignore = "touches the network"]
    fn store_icon_fetch_from_binary_cache() {
        let Ok(pkgs) = load_cache(&cache_path()) else {
            eprintln!("no cache; skipping");
            return;
        };
        let Some(pkg) = pkgs.iter().find(|p| p.attr == "firebird-emu") else {
            eprintln!("firebird-emu not in cache; skipping");
            return;
        };
        let path = pkg
            .icon_path
            .as_deref()
            .expect("firebird-emu ships an icon");
        eprintln!("fetching {path}");
        let mut budget = 1;
        let pixels = fetch_store_icon(path, &mut budget);
        assert!(pixels.is_some(), "icon must stream from the binary cache");
        // And again from the disk cache without budget.
        let mut none = 0;
        assert!(fetch_store_icon(path, &mut none).is_some());
    }

    #[test]
    fn recommendations_resolve_in_order_and_skip_missing() {
        let pkgs = vec![
            entry("gimp".into(), "gimp".into(), "2".into(), ""),
            entry("firefox".into(), "firefox".into(), "128".into(), ""),
        ];
        let hits = recommended_hits(&pkgs);
        let attrs: Vec<&str> = hits.iter().map(|p| p.attr.as_str()).collect();
        // List order (firefox before gimp), everything absent skipped.
        assert_eq!(attrs, vec!["firefox", "gimp"]);
    }

    #[test]
    fn icon_candidates_prefer_desktop_hints() {
        let ghostty = entry_with_icons(
            "ghostty".into(),
            "ghostty".into(),
            "1.3".into(),
            "",
            vec!["com.mitchellh.ghostty".into()],
        );
        assert_eq!(
            icon_candidates(&ghostty),
            vec!["com.mitchellh.ghostty", "ghostty"]
        );
    }

    #[test]
    fn icon_candidates_cover_aliases_and_variants() {
        let kate = entry("kdePackages.kate".into(), "kate".into(), "1".into(), "");
        assert_eq!(icon_candidates(&kate), vec!["kate", "org.kde.kate"]);
        let calc = entry(
            "gnome-calculator".into(),
            "gnome-calculator".into(),
            "1".into(),
            "",
        );
        assert_eq!(
            icon_candidates(&calc),
            vec!["gnome-calculator", "org.gnome.Calculator", "gnome"]
        );
        // Variants inherit the family icon via suffix stripping.
        let tg = entry(
            "telegram-desktop-bin".into(),
            "telegram-desktop-bin".into(),
            "1".into(),
            "",
        );
        assert_eq!(
            icon_candidates(&tg),
            vec!["telegram-desktop-bin", "telegram-desktop", "telegram"]
        );
        let plain = entry("cowsay".into(), "cowsay".into(), "1".into(), "");
        assert_eq!(icon_candidates(&plain), vec!["cowsay"]);
    }

    #[test]
    fn name_matches_outrank_description_mentions() {
        // Alphabetically before "firefox" AND mentioning it in the
        // description — the exact trap that buried the real package.
        let pkgs = vec![
            entry(
                "addwater".into(),
                "addwater".into(),
                "1".into(),
                "Firefox theme configurator",
            ),
            entry(
                "firefox".into(),
                "firefox".into(),
                "128".into(),
                "Web browser",
            ),
        ];
        let mut searcher = waverunner_core::Searcher::new();
        let hits = rank_hits(&mut searcher, "firefox", &pkgs);
        assert_eq!(hits.first(), Some(&1), "the name match must come first");
        assert!(hits.contains(&0), "description matches still fill the tail");
    }

    #[test]
    fn curation_keeps_apps_and_drops_ecosystems() {
        assert!(keep_attr("firefox"));
        assert!(keep_attr("gnome-calculator"));
        assert!(keep_attr("kdePackages.kate"));
        assert!(!keep_attr("python313Packages.requests"));
        assert!(!keep_attr("haskellPackages.lens"));
        assert!(!keep_attr("vimPlugins.telescope-nvim"));
        assert!(!keep_attr("linuxKernel.kernels.linux_6_12"));
        assert!(!keep_attr("tests.foo"));
        assert!(!keep_attr("firefox-unwrapped"));
        assert!(!keep_attr("firefox-beta-unwrapped"));
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
        let mut searcher = waverunner_core::Searcher::new();
        for query in ["fi", "firefox", "mozill", "media player", "zzqx"] {
            let started = std::time::Instant::now();
            let hits = rank_hits(&mut searcher, query, &pkgs);
            let top: Vec<&str> = hits
                .iter()
                .take(5)
                .map(|&i| pkgs[i].attr.as_str())
                .collect();
            eprintln!(
                "rank {query:?}: {} hits of {} in {:?}; top: {top:?}",
                hits.len(),
                pkgs.len(),
                started.elapsed()
            );
        }
    }

    /// Manual check of the Flathub icon fetch (network + disk cache):
    /// `cargo test -p waverunner-daemon flathub -- --ignored --nocapture`
    #[test]
    #[ignore = "touches the network"]
    fn flathub_icon_fetch_and_cache() {
        let mut budget = 2;
        let first = fetch_flathub_icon("dev.zed.Zed", &mut budget);
        assert!(first.is_some(), "Zed is on Flathub");
        // Second call must come from the disk cache, not the budget.
        let mut none = 0;
        let again = fetch_flathub_icon("dev.zed.Zed", &mut none);
        assert!(again.is_some(), "cached icon must not need budget");
        // A 404 leaves a miss marker and consumes budget once.
        let mut budget = 2;
        assert!(fetch_flathub_icon("not.a.realapp", &mut budget).is_none());
        assert_eq!(budget, 1);
        let mut budget = 2;
        assert!(fetch_flathub_icon("not.a.realapp", &mut budget).is_none());
        assert_eq!(budget, 2, "recorded miss must not refetch");
    }

    /// Manual check of theme-icon coverage for package names:
    /// `cargo test -p waverunner-daemon pkg_icon -- --ignored --nocapture`
    #[test]
    #[ignore = "needs the machine's icon theme"]
    fn pkg_icon_lookup_coverage() {
        let loader = crate::apps::IconLoader::new("Papirus-Dark".into());
        let icon_names = IconNames::sweep(loader.themes());
        eprintln!(
            "sweep: {} names, {} aliases from {:?}",
            icon_names.names.len(),
            icon_names.aliases.len(),
            loader.themes()
        );
        for probe in [
            "firefox",
            "vlc",
            "gimp",
            "cowsay",
            "ripgrep",
            "xterm",
            "ghostty",
            "org.kde.kate",
            "org.gnome.Calculator",
        ] {
            eprintln!("{probe}: {:?}", icon_names.resolve(probe));
        }
        let generic = GENERIC_PKG_ICONS
            .iter()
            .find_map(|name| icon_names.resolve(name));
        eprintln!("generic fallback icon: {generic:?}");
        if let Ok(pkgs) = load_cache(&cache_path()) {
            let covered = pkgs
                .iter()
                .filter(|p| {
                    icon_candidates(p)
                        .iter()
                        .any(|c| icon_names.resolve(c).is_some())
                })
                .count();
            eprintln!(
                "index coverage: {covered} of {} packages have a real icon",
                pkgs.len()
            );
        }
    }
}
