//! MODULES' catalog (Max, 2026-10-03): what each module offers, read from
//! data, not code. Golem manages the catalog — the order, the groups, which
//! programs "Install recommended" takes — and the dock only draws it, so
//! changing a default (or, one day, a placement) is an edit to
//! `assets/modules.json`, never to the dock. A machine may carry its own
//! copy at `/etc/golem/modules.json` (shipped with Golem's updates), which
//! wins over the built-in one.
//!
//! The front end only, for now: an open module lists its programs with a
//! switch each (on = in this computer), and Apply writes the wanted set to
//! `~/.config/golem/modules.list` — the file the system side (built next)
//! turns into a rebuild. See `~/Golem/docs/system/GolemModules.md`.

use std::collections::BTreeSet;
use std::path::PathBuf;
use std::sync::OnceLock;

use serde::Deserialize;
use tracing::{info, warn};

/// The built-in catalog.
const BUILT_IN: &str = include_str!("../assets/modules.json");
/// A newer catalog shipped with Golem's updates, if the machine has one.
const SYSTEM_COPY: &str = "/etc/golem/modules.json";

#[derive(Debug, Deserialize)]
pub(crate) struct Catalog {
    pub modules: Vec<Module>,
}

#[derive(Debug, Deserialize)]
pub(crate) struct Module {
    pub name: String,
    /// The field's layer it starts on (0 near … 2 far).
    pub layer: u8,
    /// What else people call it or look for it by.
    pub keywords: String,
    pub groups: Vec<Group>,
}

#[derive(Debug, Deserialize)]
pub(crate) struct Group {
    pub name: String,
    pub programs: Vec<Program>,
}

#[derive(Debug, Deserialize)]
pub(crate) struct Program {
    pub name: String,
    /// What it is, in a few words.
    pub what: String,
    /// Where it comes from: a nixpkgs attribute, or a webapp's address.
    #[serde(default)]
    pub pkg: Option<String>,
    #[serde(default)]
    pub web: Option<String>,
    /// Part of "Install recommended": the basics, nothing niche.
    #[serde(default)]
    pub recommended: bool,
    /// What the machine must have for it to be offered at all.
    #[serde(default)]
    pub needs: Option<Need>,
    /// Editorial by default. "Install recommended" only ever takes
    /// editorial entries, whatever else the catalog holds.
    #[serde(default)]
    pub placement: Placement,
}

#[derive(Debug, Deserialize, Clone, Copy, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub(crate) enum Need {
    /// A graphics card a program can compute on.
    Gpu,
    /// The CPU's virtualisation (VT-x / AMD-V).
    Virt,
    /// Memory enough for local models (8 GB or more).
    Ram,
}

#[derive(Debug, Deserialize, Clone, Copy, PartialEq, Eq, Default)]
#[serde(rename_all = "lowercase")]
pub(crate) enum Placement {
    #[default]
    Editorial,
    Sponsored,
}

impl Program {
    /// Its line in the selection file: what the system side installs.
    pub fn source(&self) -> String {
        match (&self.pkg, &self.web) {
            (Some(p), _) => format!("pkg:{p}"),
            (None, Some(w)) => format!("web:{w}"),
            _ => format!("name:{}", self.name),
        }
    }

    /// Whether "Install recommended" takes it.
    pub fn in_recommended(&self) -> bool {
        self.recommended && self.placement == Placement::Editorial
    }
}

impl Module {
    /// Its programs this machine can run, in order.
    pub fn programs(&self) -> impl Iterator<Item = &Program> {
        self.groups.iter().flat_map(|g| g.programs.iter()).filter(|p| machine().can(p.needs))
    }
}

/// The catalog: the system's copy if it reads, else the built-in one.
pub(crate) fn catalog() -> &'static Catalog {
    static CAT: OnceLock<Catalog> = OnceLock::new();
    CAT.get_or_init(|| {
        if let Ok(text) = std::fs::read_to_string(SYSTEM_COPY) {
            match serde_json::from_str::<Catalog>(&text) {
                Ok(c) if !c.modules.is_empty() => {
                    info!("modules: the catalog from {SYSTEM_COPY} ({} modules)", c.modules.len());
                    return c;
                }
                Ok(_) => warn!("modules: {SYSTEM_COPY} has no modules; the built-in catalog stands"),
                Err(e) => warn!("modules: {SYSTEM_COPY} unreadable ({e}); the built-in catalog stands"),
            }
        }
        serde_json::from_str(BUILT_IN).unwrap_or_else(|e| {
            warn!("modules: built-in catalog unreadable: {e}");
            Catalog { modules: Vec::new() }
        })
    })
}

/// A module by name.
pub(crate) fn module(name: &str) -> Option<&'static Module> {
    catalog().modules.iter().find(|m| m.name == name)
}

/// The field's pills: (name, layer, search words — its keywords and every
/// program's name, so "steam" finds Gaming).
pub(crate) fn items() -> &'static [(&'static str, u8, &'static str)] {
    static ITEMS: OnceLock<Vec<(&'static str, u8, &'static str)>> = OnceLock::new();
    ITEMS.get_or_init(|| {
        catalog()
            .modules
            .iter()
            .map(|m| {
                let mut words = m.keywords.to_lowercase();
                for p in m.groups.iter().flat_map(|g| &g.programs) {
                    words.push(' ');
                    words.push_str(&p.name.to_lowercase());
                }
                let words: &'static str = Box::leak(words.into_boxed_str());
                (m.name.as_str(), m.layer.min(2), words)
            })
            .collect()
    })
}

/// What this machine has, for [`Need`].
pub(crate) struct Machine {
    gpu: bool,
    virt: bool,
    ram: bool,
}

impl Machine {
    pub fn can(&self, need: Option<Need>) -> bool {
        match need {
            None => true,
            Some(Need::Gpu) => self.gpu,
            Some(Need::Virt) => self.virt,
            Some(Need::Ram) => self.ram,
        }
    }
}

/// Read once: a GPU to compute on (an NVIDIA or AMD card), the CPU's
/// virtualisation flags, and the memory.
pub(crate) fn machine() -> &'static Machine {
    static M: OnceLock<Machine> = OnceLock::new();
    M.get_or_init(|| {
        let gpu = std::fs::read_dir("/sys/class/drm")
            .into_iter()
            .flatten()
            .flatten()
            .filter_map(|e| std::fs::read_to_string(e.path().join("device/vendor")).ok())
            .any(|v| matches!(v.trim(), "0x10de" | "0x1002"));
        let cpu = std::fs::read_to_string("/proc/cpuinfo").unwrap_or_default();
        let virt = cpu.split_whitespace().any(|w| w == "vmx" || w == "svm");
        let ram = std::fs::read_to_string("/proc/meminfo")
            .ok()
            .and_then(|m| {
                m.lines()
                    .find(|l| l.starts_with("MemTotal:"))
                    .and_then(|l| l.split_whitespace().nth(1)?.parse::<u64>().ok())
            })
            .is_some_and(|kb| kb >= 7_500_000);
        info!("modules: machine gpu={gpu} virt={virt} ram8g={ram}");
        Machine { gpu, virt, ram }
    })
}

/// The selection file the system side reads.
fn selection_path() -> PathBuf {
    let base = std::env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".config")))
        .unwrap_or_else(|| PathBuf::from("."));
    base.join("golem/modules.list")
}

/// Apply: fold one module's changes into the selection file — one source
/// per line (`pkg:<attr>` / `web:<url>`), sorted. The system side (next)
/// watches it and rebuilds; for now this is where the front end ends.
pub(crate) fn apply(module: &str, add: &[&Program], remove: &[&Program]) {
    let path = selection_path();
    let mut set: BTreeSet<String> = std::fs::read_to_string(&path)
        .unwrap_or_default()
        .lines()
        .map(str::trim)
        .filter(|l| !l.is_empty() && !l.starts_with('#'))
        .map(str::to_owned)
        .collect();
    for p in add {
        set.insert(p.source());
    }
    for p in remove {
        set.remove(&p.source());
    }
    let mut text = String::from("# Golem modules: the programs wanted (written by the dock's Modules).\n");
    for line in &set {
        text.push_str(line);
        text.push('\n');
    }
    if let Some(dir) = path.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    let tmp = path.with_extension("list.tmp");
    match std::fs::write(&tmp, text).and_then(|()| std::fs::rename(&tmp, &path)) {
        Ok(()) => info!(
            "modules: {module} — {} to add, {} to remove → {}",
            add.len(),
            remove.len(),
            path.display()
        ),
        Err(e) => warn!("modules: could not write {}: {e}", path.display()),
    }
}

/// The catalog webapp a module's web program stands for: the entry at the
/// same address (its host, a leading `www.` aside). `None`: the catalog
/// does not carry it.
pub(crate) fn webapp_slug(url: &str, catalog: &[crate::webapps::WebappEntry]) -> Option<String> {
    let host = |u: &str| crate::webapps::url_host(u).map(|h| h.trim_start_matches("www.").to_lowercase());
    let want = host(url)?;
    // The same page first, then the same site.
    let same_page = |e: &&crate::webapps::WebappEntry| e.url.trim_end_matches('/') == url.trim_end_matches('/');
    catalog
        .iter()
        .find(same_page)
        .or_else(|| catalog.iter().find(|e| host(&e.url).as_deref() == Some(want.as_str())))
        .map(|e| e.slug.clone())
}

/// A module being installed or removed: what is still on its way through
/// the rebuild, on the task pill until the last of it lands.
pub(crate) struct ModuleJob {
    pub name: String,
    task: crate::tasks::TaskHandle,
    waiting: BTreeSet<String>,
    total: u64,
    failed: u64,
    /// Whether it is taking programs away (the closing words differ).
    removing: bool,
}

impl crate::App {
    /// The module's Apply, carried out: every program goes through the
    /// pipeline a drag into the grid uses (`packages.list` → the privileged
    /// helper → ONE rebuild for all of them, see [`crate::applier`]); a web
    /// program is its catalog webapp, installed at once. The selection file
    /// is still written: it is the record of what was asked for.
    pub(crate) fn modules_apply(&mut self, module: &'static str, add: &[&'static Program], remove: &[&'static Program]) {
        apply(module, add, remove);
        let catalog = crate::webapps::load_catalog();
        let mut waiting = BTreeSet::new();
        let mut web_changed = false;
        for p in add {
            match (&p.pkg, &p.web) {
                (Some(attr), _) => {
                    if !self.managed.contains(attr) && !crate::applier::is_installed(attr) && !self.busy_ids.contains(attr) {
                        self.start_managed_install(attr, Vec::new());
                        waiting.insert(attr.clone());
                    }
                }
                (None, Some(url)) => match webapp_slug(url, &catalog) {
                    Some(slug) if !self.managed_webapps.contains(&slug) => {
                        self.managed_webapps.add(&slug);
                        web_changed = true;
                    }
                    Some(_) => {}
                    None => info!("modules: {} is not in the webapp catalog; left out", p.name),
                },
                _ => {}
            }
        }
        for p in remove {
            match (&p.pkg, &p.web) {
                (Some(attr), _) => {
                    if self.managed.contains(attr) && !self.busy_ids.contains(attr) {
                        // The app's own tile, when it has one, so its windows
                        // are closed and its pin goes; else the bare package.
                        let tile = self.managed.desktop_ids_for(attr).into_iter().find(|id| self.removable_ids.contains(id));
                        let started = match tile {
                            Some(id) => self.uninstall_app(&id),
                            None => {
                                self.busy_ids.insert(attr.clone());
                                self.nix.request(crate::nix::Request::Remove { id: attr.clone(), attr: attr.clone() });
                                true
                            }
                        };
                        if started {
                            waiting.insert(attr.clone());
                        }
                    }
                }
                (None, Some(url)) => {
                    if let Some(slug) = webapp_slug(url, &catalog).filter(|s| self.managed_webapps.contains(s)) {
                        let id = crate::webapps::id_for_slug(&slug);
                        self.kill_app_windows(&id);
                        self.managed_webapps.remove(&slug);
                        self.pins.unpin(&id);
                        web_changed = true;
                    }
                }
                _ => {}
            }
        }
        if web_changed {
            self.refilter();
            self.schedule_frame();
        }
        if waiting.is_empty() {
            return;
        }
        let removing = add.is_empty();
        let label = crate::i18n::fill(
            crate::i18n::tr(if removing { "Removing {module}" } else { "Installing {module}" }),
            &[("module", &crate::i18n::tr_dyn(module))],
        );
        let total = waiting.len() as u64;
        let task = self.task_begin(&label);
        task.set(0, total);
        info!("modules: {module}: {total} program(s) through the rebuild");
        self.module_job = Some(ModuleJob { name: module.to_owned(), task, waiting, total, failed: 0, removing });
    }

    /// One program of the module being applied came out of the rebuild.
    pub(crate) fn module_program_done(&mut self, attr: &str, ok: bool) {
        let Some(job) = self.module_job.as_mut() else {
            return;
        };
        if !job.waiting.remove(attr) {
            return;
        }
        if !ok {
            job.failed += 1;
        }
        job.task.set(job.total - job.waiting.len() as u64, job.total);
        if !job.waiting.is_empty() {
            return;
        }
        let Some(job) = self.module_job.take() else {
            return;
        };
        let name = crate::i18n::tr_dyn(&job.name).to_owned();
        let body = if job.failed == 0 {
            crate::i18n::fill(
                crate::i18n::tr(if job.removing { "{module} was removed." } else { "{module} is ready." }),
                &[("module", &name)],
            )
        } else if job.failed == job.total {
            crate::i18n::fill(crate::i18n::tr("{module} could not be changed. Nothing was touched."), &[("module", &name)])
        } else {
            crate::i18n::fill(
                crate::i18n::tr("{module}: {n} of {total} programs could not be changed."),
                &[("module", &name), ("n", &job.failed), ("total", &job.total)],
            )
        };
        crate::desktop_send_notify_open(crate::i18n::tr("Modules"), "package-x-generic", &body, None);
        // What is installed changed: the open module shows it.
        let installed = self.modules_installed();
        self.modules_panel_mut().set_installed(installed);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_web_program_is_its_catalog_webapp() {
        let entry = |name: &str, url: &str| crate::webapps::WebappEntry {
            name: name.into(),
            url: url.into(),
            icon: name.into(),
            slug: crate::webapps::slug_of(name),
            recommended: false,
        };
        let catalog = [entry("YouTube", "https://www.youtube.com"), entry("YouTube Studio", "https://studio.youtube.com/")];
        // The same page, a trailing slash aside; else the same site.
        assert_eq!(webapp_slug("https://studio.youtube.com", &catalog).as_deref(), Some("youtube-studio"));
        assert_eq!(webapp_slug("https://youtube.com/feed", &catalog).as_deref(), Some("youtube"));
        assert_eq!(webapp_slug("https://example.org", &catalog), None);
    }

    #[test]
    fn the_built_in_catalog_reads_and_is_whole() {
        let c: Catalog = serde_json::from_str(BUILT_IN).expect("modules.json parses");
        assert_eq!(c.modules.len(), 26, "25 modules and a Coming soon spot");
        let mut layers = [0; 3];
        for m in &c.modules {
            layers[m.layer as usize] += 1;
            for p in m.groups.iter().flat_map(|g| &g.programs) {
                assert!(p.pkg.is_some() != p.web.is_some(), "{} / {}: one source", m.name, p.name);
            }
            let real = m.name != "Coming soon";
            assert!(!real || m.groups.iter().flat_map(|g| &g.programs).any(|p| p.recommended), "{}: something recommended", m.name);
        }
        assert_eq!(layers, [5, 8, 13]);
    }

    #[test]
    fn recommended_never_takes_a_sponsored_entry() {
        let p: Program = serde_json::from_str(r#"{"name":"X","what":"x","pkg":"x","recommended":true,"placement":"sponsored"}"#).unwrap();
        assert!(!p.in_recommended());
        let p: Program = serde_json::from_str(r#"{"name":"Y","what":"y","web":"https://y","recommended":true}"#).unwrap();
        assert!(p.in_recommended());
        assert_eq!(p.source(), "web:https://y");
    }
}
