//! The gear box's machine pages: this computer, storage, processor, memory,
//! graphics and battery.
//!
//! Each is a [`View`] for the engine in [`crate::gear`], fed by the worker in
//! [`crate::sys`]. A row appears only where the machine can actually answer:
//! no power modes without the profiles daemon, no charge limit the hardware
//! does not have, no second graphics card on a machine with one.
//!
//! Anything that cannot be undone is asked once ([`App::gear_arm`]): the
//! footer turns into a line saying what will happen, and a second click does
//! it.

use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};

use tracing::{info, warn};

use crate::drives::{DriveCommand, DriveEvent, DriveHandle, Target};
use crate::gear::{btn, Extra, Hit, Item, PageKind, Tone, Trail, View};
use crate::sys::{
    AppUse, Disk, Drive, Junk, Live, Machine, SysCommand, SysEvent, SysHandle, SysPage,
};
use crate::App;

const G_CPU: &str = "\u{f2db}";
const G_RAM: &str = "\u{f1c0}";
const G_DISK: &str = "\u{f02ca}";
const G_GPU: &str = "\u{f08ae}";
const G_BATTERY: &str = "\u{f240}";
const G_BOLT: &str = "\u{f0e7}";
const G_LAPTOP: &str = "\u{f109}";
const G_FOLDER: &str = "\u{f07b}";
const G_USB: &str = "\u{f287}";
const G_EJECT: &str = "\u{f052}";
const G_INFO: &str = "\u{f05a}";
const G_POWER: &str = "\u{f011}";
const G_MOON: &str = "\u{f186}";
const G_LOCK: &str = "\u{f023}";
const G_REFRESH: &str = "\u{f021}";
const G_COG: &str = "\u{f013}";
const G_TRASH: &str = "\u{f014}";
const G_BACK: &str = "\u{f053}";
const G_TIMES: &str = "\u{f00d}";
const G_MORE: &str = "\u{f141}";
const G_CHECK: &str = "\u{f00c}";
const G_PLUS: &str = "\u{f067}";
const G_WINDOW: &str = "\u{f2d0}";
const G_FILM: &str = "\u{f008}";
const G_PENCIL: &str = "\u{f040}";

/// How many samples the processor and graphics graphs hold.
pub(crate) const HIST_LEN: usize = 48;
/// Below this share of memory still available, the page says so.
const MEM_TIGHT: f32 = 0.10;
/// The battery level the saver switches on at.
const SAVER_AT: u8 = 20;
/// What apps opened "with the fast card" are started with (NVIDIA's offload).
const FAST_CARD_ENV: &str =
    "env __NV_PRIME_RENDER_OFFLOAD=1 __NV_PRIME_RENDER_OFFLOAD_PROVIDER=NVIDIA-G0 \
     __GLX_VENDOR_LIBRARY_NAME=nvidia __VK_LAYER_NV_optimus=NVIDIA_only ";

#[derive(Debug, Clone, PartialEq)]
pub(crate) enum SysHit {
    // this computer
    About,
    Float,
    Awake,
    AllSettings,
    Lock,
    Sleep,
    AskRestart,
    DoRestart,
    AskOff,
    DoOff,
    // apps (processor, memory, battery)
    App(String),
    AppClose(String),
    AskKill(String),
    DoKill(String),
    SystemTasks,
    PowerPage,
    // battery
    Mode(String),
    AutoSaver,
    ChargeLimit,
    ScreenOff(u32),
    SleepAfter(u32),
    Lid(bool),
    Health,
    // graphics
    Card(usize),
    Fast(String),
    FastAdd,
    Effects(bool),
    // storage
    Folder(usize),
    OpenFolder(usize),
    OpenPath(PathBuf),
    Clean,
    Junk(Junk),
    AskClean,
    DoClean,
    Drive(usize),
    OpenDrive(usize),
    Eject(usize),
    ExtMount(usize),
    ExtCheck(usize),
    ExtRename(usize),
    ExtUnlock(usize),
    ExtFormat(usize),
    FormatFor(Target),
    FormatLock,
    FormatThorough,
    FormatName,
    AskFormat,
    DoFormat,
    CheckUpdates,
    Video,
    AskErase,
    DoErase,
    StopErase,
}

#[derive(Debug, Clone, PartialEq, Default)]
enum PView {
    #[default]
    List,
    About,
    App(String),
    Health,
    Card(usize),
    Folder(usize),
    Clean,
    Drive(usize),
    Format(usize),
    Video,
}

#[derive(Debug, Clone, Copy, PartialEq, Default)]
enum Erase {
    #[default]
    Idle,
    Running(u8),
    Done,
    Refused,
}

#[derive(Default)]
pub(crate) struct PagesState {
    sys: Option<SysHandle>,
    machine: Machine,
    live: Live,
    disk: Disk,
    /// Window class → the app's name, as the dock knows it.
    names: HashMap<String, String>,
    cpu_hist: Vec<f32>,
    gpu_hist: Vec<f32>,
    view: PView,
    show_system: bool,
    /// What the cleaner is told to leave alone.
    junk_off: HashSet<Junk>,
    cleaning: bool,
    freed: Option<u64>,
    erase: Erase,
    /// The saver was switched on by the low battery (and from which mode).
    saver_from: Option<String>,
    /// Held while the lid is set to do nothing.
    lid_guard: Option<Child>,
    drives: Option<DriveHandle>,
    /// What is being done to a plugged-in drive right now.
    drive_busy: Option<&'static str>,
    /// How the last act on a drive ended, when it is worth a line.
    drive_note: Option<(String, bool)>,
    format: FormatDraft,
}

/// What the format view has been told so far.
#[derive(Debug, Clone, Default)]
struct FormatDraft {
    name: String,
    target: Target,
    lock: bool,
    thorough: bool,
    password: String,
}

/// What a footer field on these pages is asking for.
#[derive(Debug, Clone, PartialEq)]
pub(crate) enum PField {
    FastApp,
    DriveName(usize),
    DrivePassword(usize),
    FormatName,
    FormatPassword,
}

impl PagesState {
    pub(crate) fn pins_box(&self) -> bool {
        self.cleaning || self.drive_busy.is_some() || matches!(self.erase, Erase::Running(_))
    }
}

/// A size the way the file manager says it.
pub(crate) fn size_text(bytes: u64) -> String {
    const G: f64 = 1024.0 * 1024.0 * 1024.0;
    let gb = bytes as f64 / G;
    if gb >= 100.0 {
        format!("{gb:.0} GB")
    } else if gb >= 1.0 {
        format!("{gb:.1} GB")
    } else {
        format!("{:.0} MB", bytes as f64 / (1024.0 * 1024.0))
    }
}

/// "3 h 20 min", "35 min", "2 days".
pub(crate) fn span_text(minutes: u64) -> String {
    match minutes {
        m if m >= 2 * 24 * 60 => format!("{} days", m / (24 * 60)),
        m if m >= 60 && m % 60 == 0 => format!("{} h", m / 60),
        m if m >= 60 => format!("{} h {} min", m / 60, m % 60),
        m => format!("{m} min"),
    }
}

fn mode_name(profile: &str) -> &'static str {
    match profile {
        "power-saver" => "Saver",
        "balanced" => "Balanced",
        _ => "Performance",
    }
}

fn mode_note(profile: &str) -> &'static str {
    match profile {
        "power-saver" => "Lasts longest. Slower bursts",
        "balanced" => "Full speed when something needs it",
        _ => "Fastest. The fans and the battery pay for it",
    }
}

/// The first letter of a name as a tile's symbol.
fn monogram(name: &str) -> &'static str {
    const LETTERS: [&str; 26] = [
        "A", "B", "C", "D", "E", "F", "G", "H", "I", "J", "K", "L", "M", "N", "O", "P", "Q", "R",
        "S", "T", "U", "V", "W", "X", "Y", "Z",
    ];
    name.chars()
        .find(|c| c.is_ascii_alphabetic())
        .map_or(G_WINDOW, |c| {
            LETTERS[(c.to_ascii_uppercase() as u8 - b'A') as usize]
        })
}

fn kv(key: &str, value: impl Into<String>) -> Item {
    Item::Kv {
        hit: None,
        key: key.into(),
        value: value.into(),
    }
}

fn head(text: &str) -> Item {
    Item::Heading {
        text: text.into(),
        busy: false,
    }
}

fn sys(h: SysHit) -> Hit {
    Hit::Sys(h)
}

fn row(tile: Option<&'static str>, title: impl Into<String>, sub: impl Into<String>) -> Item {
    Item::Row {
        hit: Hit::None,
        more: None,
        tile,
        title: title.into(),
        sub: sub.into(),
        tone: Tone::Normal,
        trail: Trail::None,
        sel: false,
    }
}

/// The same row with what a press does, its corner control and its trail.
fn row_with(
    item: Item,
    hit: Hit,
    more: Option<(Hit, &'static str)>,
    trail: Trail,
    tone: Tone,
) -> Item {
    match item {
        Item::Row {
            tile,
            title,
            sub,
            sel,
            ..
        } => Item::Row {
            hit,
            more,
            tile,
            title,
            sub,
            tone,
            trail,
            sel,
        },
        other => other,
    }
}

fn back_only(items: Vec<Item>) -> View {
    View {
        strip: vec![btn(Hit::Back, G_BACK)],
        items,
        sheet_from: Some(1),
        ..View::default()
    }
}

/// The idle steps as the base hypridle config gives them: `(screen off,
/// sleep)` in seconds.
pub(crate) fn idle_base(config: &str) -> (Option<u32>, Option<u32>) {
    let (mut screen, mut sleep) = (None, None);
    for block in config.split("listener").skip(1) {
        let timeout = block
            .lines()
            .find_map(|l| l.trim().strip_prefix("timeout"))
            .and_then(|v| v.trim().trim_start_matches('=').trim().parse::<u32>().ok());
        if block.contains("golem-dpms off") {
            screen = timeout;
        } else if block.contains("golem-idle-suspend") {
            sleep = timeout;
        }
    }
    (screen, sleep)
}

/// The base config with the owner's times put in (`Some(0)` = never: that
/// step is left out).
pub(crate) fn idle_with(config: &str, screen: Option<u32>, sleep: Option<u32>) -> String {
    let mut parts = config.split("listener");
    let mut out = parts.next().unwrap_or("").to_owned();
    for block in parts {
        let want = if block.contains("golem-dpms off") {
            screen
        } else if block.contains("golem-idle-suspend") {
            sleep
        } else {
            None
        };
        match want {
            Some(0) => {
                // Drop the whole block (up to and including its closing brace).
                if let Some(end) = block.find('}') {
                    out.push_str(block[end + 1..].trim_start_matches('\n'));
                }
            }
            Some(secs) => {
                out.push_str("listener");
                for line in block.split_inclusive('\n') {
                    if line.trim_start().starts_with("timeout") {
                        out.push_str(&format!("  timeout={secs}\n"));
                    } else {
                        out.push_str(line);
                    }
                }
            }
            None => {
                out.push_str("listener");
                out.push_str(block);
            }
        }
    }
    out
}

fn config_home() -> PathBuf {
    std::env::var_os("XDG_CONFIG_HOME").map_or_else(
        || PathBuf::from(std::env::var_os("HOME").unwrap_or_default()).join(".config"),
        PathBuf::from,
    )
}

fn idle_base_text() -> Option<String> {
    std::fs::read_to_string(config_home().join("hypr/hypridle.conf")).ok()
}

/// Run a command to its end on a thread of its own (so it is reaped and the
/// event loop never waits for it).
fn run_detached(bin: &'static str, args: Vec<String>) {
    let _ = std::thread::Builder::new()
        .name("gear-cmd".into())
        .spawn(move || match Command::new(bin).args(&args).output() {
            Ok(o) if o.status.success() => {}
            Ok(o) => warn!(
                "gear: {bin} {args:?}: {}",
                String::from_utf8_lossy(&o.stderr).trim()
            ),
            Err(e) => warn!("gear: cannot run {bin}: {e}"),
        });
}

impl App {
    fn sys_send(&mut self, cmd: SysCommand) {
        if self.gear.pages.sys.is_none() {
            let (tx, rx) = calloop::channel::channel::<SysEvent>();
            let _ = self.loop_handle.insert_source(rx, |ev, _, app: &mut App| {
                if let calloop::channel::Event::Msg(e) = ev {
                    app.on_sys_event(e);
                }
            });
            self.gear.pages.sys = Some(crate::sys::spawn(tx));
        }
        if let Some(h) = &self.gear.pages.sys {
            h.send(cmd);
        }
    }

    /// Start or stop the worker's polling for the page on screen.
    pub(crate) fn pages_sync(&mut self, page: Option<PageKind>) {
        let want = match page {
            Some(PageKind::Gear) => Some(SysPage::Machine),
            Some(PageKind::Disk) => Some(SysPage::Disk),
            Some(PageKind::Cpu) => Some(SysPage::Cpu),
            Some(PageKind::Ram) => Some(SysPage::Ram),
            Some(PageKind::Gpu) => Some(SysPage::Gpu),
            Some(PageKind::Battery) => Some(SysPage::Battery),
            _ => None,
        };
        self.gear.pages.view = PView::List;
        if want.is_none() && self.gear.pages.sys.is_none() {
            return;
        }
        if want.is_some() {
            self.sys_send_windows();
        }
        self.sys_send(SysCommand::Watch(want));
    }

    fn sys_send_windows(&mut self) {
        let windows = crate::hypr::window_pids();
        self.sys_send(SysCommand::Windows(windows));
    }

    fn on_sys_event(&mut self, ev: SysEvent) {
        match ev {
            SysEvent::Machine(m) => self.gear.pages.machine = m,
            SysEvent::Live(live) => {
                let p = &mut self.gear.pages;
                p.cpu_hist.push(live.cpu);
                if p.cpu_hist.len() > HIST_LEN {
                    p.cpu_hist.remove(0);
                }
                p.live = *live;
                let gpu = self.brain.as_ref().and_then(|c| c.metrics.gpu_usage_pct);
                if let Some(g) = gpu {
                    let p = &mut self.gear.pages;
                    p.gpu_hist.push(g);
                    if p.gpu_hist.len() > HIST_LEN {
                        p.gpu_hist.remove(0);
                    }
                }
                self.resolve_app_names();
                // The next reading groups processes by the windows of now.
                self.sys_send_windows();
            }
            SysEvent::Disk(d) => self.gear.pages.disk = *d,
            SysEvent::Cleaned { freed } => {
                info!("gear: cleaned, {freed} bytes freed");
                self.gear.pages.cleaning = false;
                self.gear.pages.freed = Some(freed);
                self.update_stats_reveal();
            }
            SysEvent::Erase { pct, ok } => {
                self.gear.pages.erase = match (pct, ok) {
                    (Some(p), _) => Erase::Running(p),
                    (None, true) => Erase::Done,
                    (None, false) => Erase::Refused,
                };
                if pct.is_none() {
                    self.update_stats_reveal();
                }
            }
        }
        self.gear_changed();
    }

    /// Give every window class the name the dock shows for that app.
    fn resolve_app_names(&mut self) {
        let missing: Vec<String> = self
            .gear
            .pages
            .live
            .apps
            .iter()
            .filter(|a| !a.system && !self.gear.pages.names.contains_key(&a.key))
            .map(|a| a.key.clone())
            .collect();
        if missing.is_empty() {
            return;
        }
        let mut by_class: HashMap<String, &str> = HashMap::new();
        for (entry, &kind) in self.entries.iter().zip(&self.kinds) {
            for key in Self::app_match_keys(entry, kind) {
                by_class.entry(key).or_insert(entry.name.as_str());
            }
        }
        for class in missing {
            let name = by_class
                .get(&class.to_lowercase())
                .map_or_else(|| class.clone(), |n| (*n).to_owned());
            self.gear.pages.names.insert(class, name);
        }
    }

    fn app_name(&self, a: &AppUse) -> String {
        self.gear
            .pages
            .names
            .get(&a.key)
            .cloned()
            .unwrap_or_else(|| a.key.clone())
    }

    fn app(&self, key: &str) -> Option<&AppUse> {
        self.gear.pages.live.apps.iter().find(|a| a.key == key)
    }

    pub(crate) fn pages_can_search(&self) -> bool {
        matches!(self.stats_page_kind(), PageKind::Cpu | PageKind::Ram)
            && self.gear.pages.view == PView::List
    }

    /// `exec`, started on the fast graphics card when the owner asked for
    /// that app to be.
    pub(crate) fn gpu_exec(&self, id: &str, exec: String) -> String {
        if self.settings.gpu_fast.iter().any(|a| a == id) {
            format!("{FAST_CARD_ENV}{exec}")
        } else {
            exec
        }
    }

    /// The saver that comes on by itself when the battery runs low, and goes
    /// back when it no longer is. Called on every reading of the battery.
    pub(crate) fn battery_watch(&mut self, pct: Option<u8>, charging: bool) {
        let Some(pct) = pct else { return };
        if !self.settings.battery_saver_off && !charging && pct <= SAVER_AT {
            if self.gear.pages.saver_from.is_none() {
                let from = self
                    .gear
                    .pages
                    .live
                    .profile
                    .clone()
                    .unwrap_or_else(|| "balanced".into());
                info!("gear: battery at {pct}% — saver on (was {from})");
                self.gear.pages.saver_from = Some(from);
                self.sys_send(SysCommand::Profile("power-saver".into()));
            }
        } else if let Some(from) = self.gear.pages.saver_from.take() {
            info!("gear: battery no longer low — back to {from}");
            self.sys_send(SysCommand::Profile(from));
        }
    }

    /// Put the owner's standing choices back after a start: the idle times
    /// and the lid.
    pub(crate) fn pages_startup(&mut self) {
        // An erase the dock did not live to finish leaves its file behind,
        // holding the very space it was writing over.
        crate::sys::remove_erase_leftover();
        if self.settings.screen_off_secs.is_some() || self.settings.sleep_secs.is_some() {
            self.apply_idle();
        }
        if self.settings.lid_nothing {
            self.hold_lid(true);
        }
    }

    fn caffeine_path() -> PathBuf {
        config_home().join("golem/caffeine")
    }

    /// Write the idle config with the owner's times and point hypridle at it
    /// (a drop-in on its unit); with no choice left, both are removed.
    fn apply_idle(&mut self) {
        let Some(base) = idle_base_text() else { return };
        let (screen, sleep) = (self.settings.screen_off_secs, self.settings.sleep_secs);
        let conf = config_home().join("golem/hypridle.conf");
        let dropin_dir = config_home().join("systemd/user/hypridle.service.d");
        let dropin = dropin_dir.join("50-golem-idle.conf");
        if screen.is_none() && sleep.is_none() {
            let _ = std::fs::remove_file(&dropin);
            let _ = std::fs::remove_file(&conf);
        } else {
            let bin = Command::new("systemctl")
                .args(["--user", "show", "hypridle.service", "-p", "ExecStart"])
                .output()
                .ok()
                .map(|o| String::from_utf8_lossy(&o.stdout).into_owned())
                .and_then(|s| {
                    s.split("path=")
                        .nth(1)
                        .and_then(|r| r.split_whitespace().next())
                        .map(|p| p.trim_end_matches(';').to_owned())
                });
            let Some(bin) = bin else {
                warn!("gear: cannot find hypridle's own command; idle times not applied");
                return;
            };
            crate::persist::write_text("idle config", &conf, &idle_with(&base, screen, sleep));
            let _ = std::fs::create_dir_all(&dropin_dir);
            crate::persist::write_text(
                "idle drop-in",
                &dropin,
                &format!(
                    "[Service]\nExecStart=\nExecStart={bin} -c {}\n",
                    conf.display()
                ),
            );
        }
        let _ = std::thread::Builder::new()
            .name("gear-idle".into())
            .spawn(|| {
                for args in [
                    &["--user", "daemon-reload"][..],
                    &["--user", "try-restart", "hypridle.service"][..],
                ] {
                    let _ = Command::new("systemctl").args(args).output();
                }
            });
    }

    /// Hold (or let go of) the inhibitor that makes closing the lid do nothing.
    fn hold_lid(&mut self, hold: bool) {
        if let Some(mut child) = self.gear.pages.lid_guard.take() {
            let _ = child.kill();
            let _ = child.wait();
        }
        if !hold {
            return;
        }
        match Command::new("systemd-inhibit")
            .args([
                "--what=handle-lid-switch",
                "--who=Golem",
                "--why=The lid is set to do nothing",
                "--mode=block",
                "sleep",
                "infinity",
            ])
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
        {
            Ok(child) => self.gear.pages.lid_guard = Some(child),
            Err(e) => warn!("gear: cannot hold the lid: {e}"),
        }
    }

    // --- the views -----------------------------------------------------------

    pub(crate) fn pages_view(&self, kind: PageKind) -> View {
        let p = &self.gear.pages;
        match (&p.view, kind) {
            (PView::App(key), _) => self.app_detail_view(key),
            (PView::About, _) => self.about_view(),
            (PView::Health, _) => self.health_view(),
            (PView::Card(i), _) => self.card_view(*i),
            (PView::Folder(i), _) => self.folder_view(*i),
            (PView::Clean, _) => self.clean_view(),
            (PView::Drive(i), _) => self.drive_view(*i),
            (PView::Format(i), _) => self.format_view(*i),
            (PView::Video, _) => self.video_view(),
            (PView::List, PageKind::Gear) => self.machine_view(),
            (PView::List, PageKind::Disk) => self.disk_view(),
            (PView::List, PageKind::Cpu) => self.cpu_view(),
            (PView::List, PageKind::Ram) => self.ram_view(),
            (PView::List, PageKind::Gpu) => self.gpu_view(),
            (PView::List, PageKind::Battery) => self.battery_view(),
            _ => View::default(),
        }
    }

    /// Rows for the apps, sorted by `metric`, with what `label` says of each.
    fn app_rows(
        &self,
        metric: impl Fn(&AppUse) -> f64,
        label: impl Fn(&AppUse) -> String,
    ) -> Vec<Item> {
        let p = &self.gear.pages;
        let query = self.search_query();
        let mut apps: Vec<&AppUse> = p
            .live
            .apps
            .iter()
            .filter(|a| !a.system || p.show_system)
            .filter(|a| query.is_empty() || self.app_name(a).to_lowercase().contains(&query))
            .collect();
        apps.sort_by(|a, b| {
            metric(b)
                .total_cmp(&metric(a))
                .then_with(|| a.key.cmp(&b.key))
        });
        // The system runs hundreds of small things; the busiest are the story.
        let cap = if p.show_system { 24 } else { usize::MAX };
        let mut rows: Vec<Item> = apps
            .into_iter()
            .take(cap)
            .map(|a| {
                let name = self.app_name(a);
                let sub = if a.system {
                    "Part of the system".to_owned()
                } else {
                    match a.windows.len() {
                        0 | 1 => "1 window".to_owned(),
                        n => format!("{n} windows"),
                    }
                };
                row_with(
                    row(Some(monogram(&name)), name, sub),
                    sys(SysHit::App(a.key.clone())),
                    (!a.system).then(|| (sys(SysHit::AppClose(a.key.clone())), G_TIMES)),
                    Trail::Text(label(a)),
                    Tone::Normal,
                )
            })
            .collect();
        if rows.is_empty() {
            rows.push(head(if query.is_empty() {
                "Nothing is open"
            } else {
                "No app matches"
            }));
        }
        rows
    }

    fn system_toggle(&self) -> Item {
        Item::Toggle {
            hit: sys(SysHit::SystemTasks),
            label: "Show system tasks".into(),
            hint: "What Golem itself is running".into(),
            on: self.gear.pages.show_system,
        }
    }

    fn cpu_view(&self) -> View {
        let p = &self.gear.pages;
        let l = &p.live;
        let hot = l.temp.is_some_and(|t| t >= 92.0);
        let mut parts = vec![format!("{:.0}%", l.cpu)];
        if let Some(t) = l.temp {
            parts.push(format!("{t:.0}°"));
        }
        parts.push(format!("{} cores", p.machine.cores));
        let status = row_with(
            row(
                Some(G_CPU),
                "Processor",
                if hot {
                    "Running hot, so it slows itself down".to_owned()
                } else {
                    parts.join(" · ")
                },
            ),
            Hit::None,
            None,
            Trail::None,
            if hot { Tone::Warn } else { Tone::Normal },
        );
        let mut items = vec![
            status,
            Item::Graph(p.cpu_hist.clone()),
            Item::Cores(l.cores.clone()),
        ];
        if let Some(profile) = &l.profile {
            if self.stats_page_for(PageKind::Battery).is_some() {
                items.push(Item::Kv {
                    hit: Some(sys(SysHit::PowerPage)),
                    key: "Power mode".into(),
                    value: mode_name(profile).into(),
                });
            } else {
                // A desktop has no battery page: the mode lives here.
                items.push(self.mode_choice(profile));
                items.push(Item::Note(mode_note(profile).into()));
            }
        }
        items.push(head("Using the processor"));
        items.extend(self.app_rows(|a| f64::from(a.cpu), |a| format!("{:.0}%", a.cpu)));
        items.push(self.system_toggle());
        View {
            items,
            zebra: true,
            ..View::default()
        }
    }

    fn ram_view(&self) -> View {
        let l = &self.gear.pages.live;
        let total = l.mem_total.max(1);
        let tight = (l.mem_free + l.mem_cache) as f32 / total as f32 <= MEM_TIGHT;
        let status = row_with(
            row(
                Some(G_RAM),
                "Memory",
                if tight {
                    "Almost full. Apps may slow down".to_owned()
                } else {
                    format!(
                        "{} of {} in use",
                        size_text(l.mem_apps),
                        size_text(l.mem_total)
                    )
                },
            ),
            Hit::None,
            None,
            Trail::None,
            if tight { Tone::Warn } else { Tone::Normal },
        );
        let seg = |name: &str, bytes: u64, opacity: f32| {
            (
                format!("{name} {}", size_text(bytes)),
                bytes as f32,
                opacity,
            )
        };
        let mut items = vec![
            status,
            Item::Bar(vec![
                seg("Apps", l.mem_apps, 1.0),
                seg("Kept ready", l.mem_cache, 0.5),
                seg("Free", l.mem_free, 0.16),
            ]),
        ];
        if let Some((orig, compr)) = l.packed {
            items.push(kv(
                "Packed away",
                if orig < 64 * 1024 * 1024 {
                    "Nothing. There is room".to_owned()
                } else {
                    format!("{} squeezed into {}", size_text(orig), size_text(compr))
                },
            ));
        }
        if tight {
            if let Some(big) = l.apps.iter().filter(|a| !a.system).max_by_key(|a| a.mem) {
                items.push(row_with(
                    row(
                        Some(G_BOLT),
                        format!("Close {}", self.app_name(big)),
                        format!("Frees about {}", size_text(big.mem)),
                    ),
                    sys(SysHit::AppClose(big.key.clone())),
                    None,
                    Trail::None,
                    Tone::Normal,
                ));
            }
        }
        items.push(head("Using the most memory"));
        items.extend(self.app_rows(|a| a.mem as f64, |a| size_text(a.mem)));
        items.push(self.system_toggle());
        View {
            items,
            zebra: true,
            ..View::default()
        }
    }

    fn mode_choice(&self, current: &str) -> Item {
        Item::Choice {
            key: "Mode".into(),
            opts: self
                .gear
                .pages
                .live
                .profiles
                .iter()
                .map(|p| {
                    (
                        sys(SysHit::Mode(p.clone())),
                        mode_name(p).to_owned(),
                        p == current,
                    )
                })
                .collect(),
        }
    }

    fn battery_view(&self) -> View {
        let p = &self.gear.pages;
        let l = &p.live;
        let Some(b) = &l.battery else {
            return View {
                items: vec![row(
                    Some(G_BATTERY),
                    "Battery",
                    "No battery in this computer",
                )],
                ..View::default()
            };
        };
        let low = !b.charging && !b.full && b.pct <= SAVER_AT;
        let time = b.minutes.map(|m| span_text(u64::from(m)));
        let state = if b.charging {
            time.map_or("charging".to_owned(), |t| format!("charging · full in {t}"))
        } else if b.full {
            "plugged in".to_owned()
        } else {
            time.map_or("on battery".to_owned(), |t| format!("about {t} left"))
        };
        let mut items = vec![row_with(
            row(
                Some(if b.charging { G_BOLT } else { G_BATTERY }),
                "Battery",
                format!("{}% · {state}", b.pct),
            ),
            Hit::None,
            None,
            Trail::None,
            if low { Tone::Warn } else { Tone::Normal },
        )];
        if let Some(profile) = &l.profile {
            items.push(self.mode_choice(profile));
            items.push(Item::Note(
                if p.saver_from.is_some() {
                    "On Saver because the battery is low"
                } else {
                    mode_note(profile)
                }
                .into(),
            ));
            if l.profiles.iter().any(|p| p == "power-saver") {
                items.push(Item::Toggle {
                    hit: sys(SysHit::AutoSaver),
                    label: "Save battery when low".into(),
                    hint: format!("Switches to Saver below {SAVER_AT}%"),
                    on: !self.settings.battery_saver_off,
                });
            }
        }
        if b.limit_writable {
            items.push(Item::Toggle {
                hit: sys(SysHit::ChargeLimit),
                label: "Stop charging at 80%".into(),
                hint: "Keeps the battery healthy for more years".into(),
                on: b.limit.is_some_and(|l| l <= 80),
            });
        }
        if crate::launch::on_path("golem-caffeine") {
            items.push(Item::Toggle {
                hit: sys(SysHit::Awake),
                label: "Keep awake".into(),
                hint: "No dimming, no sleep".into(),
                on: Self::caffeine_path().exists(),
            });
        }
        items.extend(self.idle_rows());
        if b.design_wh > 1.0 {
            let health = (100.0 * b.full_wh / b.design_wh).round().clamp(0.0, 100.0) as u8;
            items.push(row_with(
                row(
                    Some(G_INFO),
                    "Battery health",
                    format!(
                        "{}. Holds {health}% of what it did new",
                        health_word(health)
                    ),
                ),
                sys(SysHit::Health),
                Some((sys(SysHit::Health), G_MORE)),
                Trail::None,
                if health < 60 {
                    Tone::Warn
                } else {
                    Tone::Normal
                },
            ));
        }
        items.push(head("Using the most power"));
        items.extend(self.app_rows(|a| f64::from(a.cpu), |a| format!("{:.0}%", a.cpu)));
        View {
            items,
            zebra: true,
            ..View::default()
        }
    }

    /// When the screen goes dark, when the computer sleeps, what the lid does:
    /// on the battery page, or on the gear's own where a machine has none.
    fn idle_rows(&self) -> Vec<Item> {
        let mut items = Vec::new();
        if let Some(base) = idle_base_text() {
            let (base_screen, base_sleep) = idle_base(&base);
            let pills = |base: Option<u32>,
                         picked: Option<u32>,
                         stops: [u32; 2],
                         hit: fn(u32) -> SysHit| {
                let current = picked.or(base);
                let mut secs: Vec<u32> = stops.into_iter().chain(base).collect();
                secs.sort_unstable();
                secs.dedup();
                secs.push(0);
                secs.into_iter()
                    .map(|s| {
                        let label = if s == 0 {
                            "Never".to_owned()
                        } else {
                            span_text(u64::from(s / 60))
                        };
                        (sys(hit(s)), label, current == Some(s))
                    })
                    .collect::<Vec<_>>()
            };
            if base_screen.is_some() {
                items.push(Item::Choice {
                    key: "Screen".into(),
                    opts: pills(
                        base_screen,
                        self.settings.screen_off_secs,
                        [120, 900],
                        SysHit::ScreenOff,
                    ),
                });
            }
            if base_sleep.is_some() {
                items.push(Item::Choice {
                    key: "Sleep".into(),
                    opts: pills(
                        base_sleep,
                        self.settings.sleep_secs,
                        [1800, 3600],
                        SysHit::SleepAfter,
                    ),
                });
            }
        }
        if std::path::Path::new("/proc/acpi/button/lid").exists() {
            let nothing = self.settings.lid_nothing;
            items.push(Item::Choice {
                key: "Lid".into(),
                opts: vec![
                    (sys(SysHit::Lid(false)), "Sleep".into(), !nothing),
                    (sys(SysHit::Lid(true)), "Nothing".into(), nothing),
                ],
            });
        }
        items
    }

    fn health_view(&self) -> View {
        let Some(b) = &self.gear.pages.live.battery else {
            return back_only(Vec::new());
        };
        let health = (100.0 * b.full_wh / b.design_wh.max(0.1))
            .round()
            .clamp(0.0, 100.0) as u8;
        let mut items = vec![
            Item::Card {
                glyph: G_BATTERY,
                title: "Battery health".into(),
                sub: health_word(health).into(),
                extra: Extra::Meter(health, format!("Holds {health}% of what it did new")),
                rename: None,
            },
            kv("Holds now", format!("{:.1} Wh", b.full_wh)),
            kv("When new", format!("{:.1} Wh", b.design_wh)),
        ];
        if let Some(c) = b.cycles {
            items.push(kv("Cycles", c.to_string()));
        }
        if b.watts > 0.3 {
            items.push(kv(
                if b.charging { "Charging at" } else { "Using" },
                format!("{:.1} W", b.watts),
            ));
        }
        items.push(Item::Note("A battery counts as worn at about 80%.".into()));
        back_only(items)
    }

    fn app_detail_view(&self, key: &str) -> View {
        let Some(a) = self.app(key) else {
            return back_only(vec![Item::Empty("It is closed now".into())]);
        };
        let name = self.app_name(a);
        let mut strip = vec![btn(Hit::Back, G_BACK)];
        if !a.system {
            strip.push(btn(sys(SysHit::AppClose(a.key.clone())), G_TIMES));
            strip.push(btn(sys(SysHit::AskKill(a.key.clone())), G_BOLT));
        }
        View {
            strip,
            items: vec![
                Item::Card {
                    glyph: monogram(&name),
                    title: name,
                    sub: if a.system {
                        "Part of the system".into()
                    } else {
                        match a.windows.len() {
                            0 | 1 => "1 window".into(),
                            n => format!("{n} windows"),
                        }
                    },
                    extra: Extra::None,
                    rename: None,
                },
                kv("Processor", format!("{:.0}%", a.cpu)),
                kv("Memory", size_text(a.mem)),
                kv(
                    "Made of",
                    match a.pids.len() {
                        1 => "1 process".to_owned(),
                        n => format!("{n} processes"),
                    },
                ),
            ],
            sheet_from: Some(1),
            ..View::default()
        }
    }

    fn machine_view(&self) -> View {
        let p = &self.gear.pages;
        let m = &p.machine;
        let on_for = span_text(p.live.uptime_secs / 60);
        let stale = self
            .brain
            .as_ref()
            .is_some_and(|c| c.deploy.stale_generation);
        let (title, sub, tone, hit) = if p.live.updating {
            (
                "Updating now…",
                "Keep working. It finishes at the next restart".to_owned(),
                Tone::Busy,
                Hit::None,
            )
        } else if stale {
            (
                "Restart to finish updating",
                "Click to restart now".to_owned(),
                Tone::Warn,
                sys(SysHit::AskRestart),
            )
        } else {
            (
                "Up to date",
                p.live
                    .last_update_secs
                    .map_or("Click to check now".to_owned(), |s| {
                        format!("Checked {} ago", span_text(s / 60))
                    }),
                Tone::Normal,
                sys(SysHit::CheckUpdates),
            )
        };
        let mut items = vec![
            row_with(
                row(
                    Some(G_LAPTOP),
                    if m.host.is_empty() {
                        "This computer".to_owned()
                    } else {
                        m.host.clone()
                    },
                    // "Golem 26.05 (Yarara)": the release's own name is for the
                    // details view; the row has room for the version.
                    format!("{} · up {on_for}", m.os.split(" (").next().unwrap_or(&m.os)),
                ),
                sys(SysHit::About),
                Some((sys(SysHit::About), G_MORE)),
                Trail::None,
                Tone::Normal,
            ),
            row_with(
                row(Some(G_REFRESH), title, sub),
                hit,
                None,
                Trail::None,
                tone,
            ),
            Item::Toggle {
                hit: sys(SysHit::Float),
                label: "Floating windows".into(),
                hint: if self.floating_mode() {
                    "Every window floats".into()
                } else {
                    "Windows tile. Golem places them".into()
                },
                on: self.floating_mode(),
            },
        ];
        if crate::launch::on_path("golem-caffeine") {
            items.push(Item::Toggle {
                hit: sys(SysHit::Awake),
                label: "Keep awake".into(),
                hint: "No dimming, no sleep".into(),
                on: Self::caffeine_path().exists(),
            });
        }
        if self.stats_page_for(PageKind::Battery).is_none() {
            items.extend(self.idle_rows());
        }
        items.push(Item::Toggle {
            hit: Hit::Airplane,
            label: "Airplane mode".into(),
            hint: "Wi-Fi and Bluetooth off".into(),
            on: self.settings.airplane,
        });
        items.push(row_with(
            row(Some(G_COG), "All settings", "Screen, sound and the rest"),
            sys(SysHit::AllSettings),
            None,
            Trail::None,
            Tone::Normal,
        ));
        View {
            items,
            footer: vec![
                btn(sys(SysHit::Lock), G_LOCK),
                btn(sys(SysHit::Sleep), G_MOON),
                btn(sys(SysHit::AskRestart), G_REFRESH),
                btn(sys(SysHit::AskOff), G_POWER),
            ],
            zebra: true,
            ..View::default()
        }
    }

    fn about_view(&self) -> View {
        let p = &self.gear.pages;
        let m = &p.machine;
        let mut items = vec![Item::Card {
            glyph: G_LAPTOP,
            title: if m.host.is_empty() {
                "This computer".into()
            } else {
                m.host.clone()
            },
            sub: m.os.clone(),
            extra: Extra::None,
            rename: None,
        }];
        if !m.model.is_empty() {
            items.push(kv("Model", m.model.clone()));
        }
        items.push(kv("Processor", m.cpu.clone()));
        items.push(kv("Memory", size_text(m.mem_bytes)));
        if !m.cards.is_empty() {
            let names: Vec<&str> = m.cards.iter().map(|c| c.name.as_str()).collect();
            items.push(kv("Graphics", names.join(" · ")));
        }
        items.push(kv("On for", span_text(p.live.uptime_secs / 60)));
        back_only(items)
    }

    fn gpu_view(&self) -> View {
        let p = &self.gear.pages;
        let usage = self.brain.as_ref().and_then(|c| c.metrics.gpu_usage_pct);
        let cards = &p.machine.cards;
        let mut items = vec![row(
            Some(G_GPU),
            "Graphics",
            usage.map_or_else(
                || match cards.len() {
                    1 => "1 graphics card".to_owned(),
                    n => format!("{n} graphics cards"),
                },
                |u| format!("{u:.0}% busy"),
            ),
        )];
        if usage.is_some() {
            items.push(Item::Graph(p.gpu_hist.clone()));
        }
        items.push(head(if cards.len() == 1 {
            "Graphics card"
        } else {
            "Graphics cards"
        }));
        for (i, c) in cards.iter().enumerate() {
            let awake = p.live.cards_awake.get(i).copied().unwrap_or(c.awake);
            let sub = if c.primary {
                "In use. Drives the screen"
            } else if awake {
                "Awake. An app is using it"
            } else {
                "Asleep. Wakes for apps that ask"
            };
            items.push(row_with(
                row(Some(G_GPU), c.name.clone(), sub),
                sys(SysHit::Card(i)),
                Some((sys(SysHit::Card(i)), G_MORE)),
                if c.primary {
                    Trail::Glyph(G_CHECK)
                } else {
                    Trail::None
                },
                Tone::Normal,
            ));
        }
        if self.has_fast_card() {
            items.push(head("Open with the fast card"));
            for id in &self.settings.gpu_fast {
                let name = self
                    .entries
                    .iter()
                    .find(|e| &e.id == id)
                    .map_or_else(|| id.clone(), |e| e.name.clone());
                items.push(Item::Toggle {
                    hit: sys(SysHit::Fast(id.clone())),
                    label: name,
                    hint: String::new(),
                    on: true,
                });
            }
            items.push(row_with(
                row(
                    Some(G_PLUS),
                    "Add an app",
                    "It opens on the fast card from then on",
                ),
                sys(SysHit::FastAdd),
                None,
                Trail::None,
                Tone::Normal,
            ));
        }
        if !p.machine.video.is_empty() {
            let on_card = p.machine.video.iter().filter(|(_, ok)| *ok).count();
            items.push(row_with(
                row(
                    Some(G_FILM),
                    "Video playback",
                    if on_card > 0 {
                        "Decoded by the graphics card"
                    } else {
                        "Decoded by the processor"
                    },
                ),
                sys(SysHit::Video),
                Some((sys(SysHit::Video), G_MORE)),
                Trail::None,
                Tone::Normal,
            ));
        }
        let light = crate::display::GolemSettings::load().light_effects;
        items.push(head("Looks"));
        items.push(Item::Choice {
            key: "Effects".into(),
            opts: vec![
                (sys(SysHit::Effects(false)), "Full".into(), !light),
                (sys(SysHit::Effects(true)), "Light".into(), light),
            ],
        });
        items.push(Item::Note(
            if light {
                "No blur. For weak graphics and long battery"
            } else {
                "Blur and glass everywhere"
            }
            .into(),
        ));
        View {
            items,
            zebra: true,
            ..View::default()
        }
    }

    /// A second card that sleeps until asked: the case "open with the fast
    /// card" exists for.
    fn has_fast_card(&self) -> bool {
        let cards = &self.gear.pages.machine.cards;
        cards.len() > 1 && cards.iter().any(|c| !c.primary && c.driver == "nvidia")
    }

    fn card_view(&self, i: usize) -> View {
        let p = &self.gear.pages;
        let Some(c) = p.machine.cards.get(i) else {
            return back_only(Vec::new());
        };
        let awake = p.live.cards_awake.get(i).copied().unwrap_or(c.awake);
        back_only(vec![
            Item::Card {
                glyph: G_GPU,
                title: c.name.clone(),
                sub: if c.primary {
                    "In use".into()
                } else if awake {
                    "Awake".into()
                } else {
                    "Asleep".into()
                },
                extra: Extra::None,
                rename: None,
            },
            kv(
                "Role",
                if c.primary {
                    "Drives the screen"
                } else {
                    "Wakes for apps that ask for it"
                },
            ),
            kv("Driver", c.driver.clone()),
        ])
    }

    fn disk_view(&self) -> View {
        let p = &self.gear.pages;
        let d = &p.disk;
        let failing = d.drives.iter().any(|x| x.system && x.failing == Some(true));
        let mut items = vec![row_with(
            row(
                Some(G_DISK),
                "Storage",
                if failing {
                    "This drive is failing. Back up now".to_owned()
                } else if d.total == 0 {
                    "Reading…".to_owned()
                } else {
                    format!("{} of {} used", size_text(d.used), size_text(d.total))
                },
            ),
            Hit::None,
            None,
            Trail::None,
            if failing { Tone::Warn } else { Tone::Normal },
        )];
        if d.measured {
            let mut segs = vec![(
                format!("Apps and system {}", size_text(d.system_bytes)),
                d.system_bytes as f32,
                1.0,
            )];
            let fade = [0.8, 0.64, 0.5, 0.38, 0.3];
            for (i, f) in d.folders.iter().enumerate() {
                segs.push((
                    format!("{} {}", f.name, size_text(f.bytes)),
                    f.bytes as f32,
                    fade[i.min(4)],
                ));
            }
            segs.push((
                format!("Everything else {}", size_text(d.other_bytes)),
                d.other_bytes as f32,
                0.22,
            ));
            items.push(Item::Bar(segs));
            items.push(row_with(
                row(Some(G_COG), "Apps and system", ""),
                Hit::None,
                None,
                Trail::Text(size_text(d.system_bytes)),
                Tone::Normal,
            ));
            for (i, f) in d.folders.iter().enumerate() {
                items.push(row_with(
                    row(Some(G_FOLDER), f.name.clone(), ""),
                    sys(SysHit::Folder(i)),
                    Some((sys(SysHit::Folder(i)), G_MORE)),
                    Trail::Text(size_text(f.bytes)),
                    Tone::Normal,
                ));
            }
            items.push(row_with(
                row(Some(G_FOLDER), "Everything else", ""),
                Hit::None,
                None,
                Trail::Text(size_text(d.other_bytes)),
                Tone::Normal,
            ));
            items.push(head("Free up space"));
            let known: u64 = d.junk.iter().filter_map(|(_, b)| *b).sum();
            items.push(row_with(
                row(
                    Some(G_TRASH),
                    "Clean up",
                    match p.freed {
                        Some(f) if known < 64 * 1024 * 1024 => {
                            format!("Clean. {} freed", size_text(f))
                        }
                        _ => format!("About {} can go: trash and caches", size_text(known)),
                    },
                ),
                sys(SysHit::Clean),
                Some((sys(SysHit::Clean), G_MORE)),
                Trail::None,
                Tone::Normal,
            ));
        } else {
            items.push(Item::Heading {
                text: "Measuring what fills it…".into(),
                busy: true,
            });
        }
        items.push(head("Drives"));
        for (i, x) in d.drives.iter().enumerate() {
            let kind = if x.rotational { "spinning disk" } else { "SSD" };
            if x.removable {
                items.push(row_with(
                    row(Some(G_USB), x.name.clone(), self.ext_status(x)),
                    sys(SysHit::Drive(i)),
                    Some((sys(SysHit::Eject(i)), G_EJECT)),
                    Trail::None,
                    if p.drive_busy.is_some() {
                        Tone::Busy
                    } else {
                        Tone::Normal
                    },
                ));
            } else {
                let bad = x.failing == Some(true);
                items.push(row_with(
                    row(
                        Some(G_DISK),
                        x.name.clone(),
                        if bad {
                            "Failing. Back up now".to_owned()
                        } else {
                            let health = if x.failing == Some(false) {
                                "Healthy · "
                            } else {
                                ""
                            };
                            format!("{health}{} {kind}", size_text(x.size))
                        },
                    ),
                    sys(SysHit::Drive(i)),
                    Some((sys(SysHit::Drive(i)), G_MORE)),
                    Trail::None,
                    if bad { Tone::Warn } else { Tone::Normal },
                ));
            }
        }
        View {
            items,
            zebra: true,
            ..View::default()
        }
    }

    fn folder_view(&self, i: usize) -> View {
        let Some(f) = self.gear.pages.disk.folders.get(i) else {
            return back_only(Vec::new());
        };
        let mut items = vec![
            Item::Card {
                glyph: G_FOLDER,
                title: f.name.clone(),
                sub: size_text(f.bytes),
                extra: Extra::None,
                rename: None,
            },
            head("The biggest things in it"),
        ];
        for (name, bytes) in &f.top {
            items.push(row_with(
                row(None, name.clone(), ""),
                sys(SysHit::OpenPath(f.path.join(name))),
                None,
                Trail::Text(size_text(*bytes)),
                Tone::Normal,
            ));
        }
        View {
            strip: vec![
                btn(Hit::Back, G_BACK),
                btn(sys(SysHit::OpenFolder(i)), G_FOLDER),
            ],
            items,
            sheet_from: Some(1),
            ..View::default()
        }
    }

    fn junk_bytes(&self, only_chosen: bool) -> u64 {
        let p = &self.gear.pages;
        p.disk
            .junk
            .iter()
            .filter(|(j, _)| !only_chosen || self.junk_chosen(*j))
            .filter_map(|(_, b)| *b)
            .sum()
    }

    fn junk_chosen(&self, j: Junk) -> bool {
        let p = &self.gear.pages;
        let browser_open = j == Junk::Browser && p.disk.browser_running;
        !(p.junk_off.contains(&j) || browser_open)
    }

    fn clean_view(&self) -> View {
        let p = &self.gear.pages;
        let chosen = self.junk_bytes(true);
        let any = Junk::ALL.iter().any(|j| self.junk_chosen(*j));
        let mut items = vec![Item::Card {
            glyph: G_TRASH,
            title: "Clean up".into(),
            sub: if p.cleaning {
                "Cleaning…".into()
            } else if let Some(f) = p.freed {
                format!("Done. {} freed", size_text(f))
            } else if any {
                format!("{} chosen", size_text(chosen))
            } else {
                "Nothing chosen".into()
            },
            extra: Extra::None,
            rename: None,
        }];
        for (j, bytes) in p
            .disk
            .junk
            .iter()
            .filter(|(j, _)| *j != Junk::System || p.machine.has_system_clean)
        {
            let (name, hint) = match j {
                Junk::Trash => ("Trash", "Files you already threw away"),
                Junk::Caches => ("App caches and thumbnails", "Apps rebuild them as needed"),
                Junk::Browser if p.disk.browser_running => {
                    ("Browser cache", "Close the browser first")
                }
                Junk::Browser => ("Browser cache", "Pages load a little slower once"),
                Junk::Store => ("Unused system files", "What no system version still needs"),
                Junk::System => (
                    "Old system versions and logs",
                    "Keeps the last two versions",
                ),
            };
            items.push(Item::Toggle {
                hit: sys(SysHit::Junk(*j)),
                label: bytes
                    .map_or_else(|| name.to_owned(), |b| format!("{name} · {}", size_text(b))),
                hint: hint.into(),
                on: self.junk_chosen(*j),
            });
        }
        items.push(row_with(
            row(
                Some(G_CHECK),
                if p.cleaning {
                    "Cleaning…"
                } else {
                    "Clean now"
                },
                if p.cleaning {
                    String::new()
                } else if p.freed.is_some() && chosen < 1024 * 1024 {
                    "Nothing left to clean".to_owned()
                } else if any {
                    format!("Frees {} or more", size_text(chosen))
                } else {
                    "Choose something above".to_owned()
                },
            ),
            if any && !p.cleaning {
                sys(SysHit::AskClean)
            } else {
                Hit::None
            },
            None,
            Trail::None,
            if p.cleaning { Tone::Busy } else { Tone::Normal },
        ));
        if let Some(i) = p.disk.folders.iter().position(|f| f.name == "Downloads") {
            items.push(head("Yours to decide"));
            items.push(row_with(
                row(
                    Some(G_FOLDER),
                    format!("Downloads · {}", size_text(p.disk.folders[i].bytes)),
                    "Your files. Opens the folder, deletes nothing",
                ),
                sys(SysHit::OpenFolder(i)),
                None,
                Trail::None,
                Tone::Normal,
            ));
        }
        back_only(items)
    }

    fn drive_view(&self, i: usize) -> View {
        let p = &self.gear.pages;
        let Some(x) = p.disk.drives.get(i) else {
            return back_only(vec![Item::Empty("It was unplugged".into())]);
        };
        if x.removable {
            return self.ext_view(i, x);
        }
        let mut items = vec![Item::Card {
            glyph: G_DISK,
            title: x.name.clone(),
            sub: match x.failing {
                Some(true) => "Failing. Copy your files somewhere else now".into(),
                Some(false) => "Healthy".into(),
                None => "It does not report its health".into(),
            },
            extra: Extra::None,
            rename: None,
        }];
        items.push(kv(
            "Kind",
            format!(
                "{} · {}",
                if x.rotational { "Spinning disk" } else { "SSD" },
                size_text(x.size)
            ),
        ));
        if let Some(w) = x.wear_pct {
            items.push(kv("Wear", format!("{w}% used up")));
        }
        if let Some(t) = x.temp_c {
            items.push(kv("Temperature", format!("{t:.0}°")));
        }
        if let Some(h) = x.power_on_hours {
            items.push(kv("Powered on", span_text(h * 60)));
        }
        if x.system {
            items.push(kv("Care", "Kept in shape weekly"));
            let (title, sub, tone, hit) = match p.erase {
                Erase::Running(pct) => (
                    "Stop erasing",
                    format!("Erasing… {pct}%"),
                    Tone::Busy,
                    sys(SysHit::StopErase),
                ),
                Erase::Done => (
                    "Erase empty space",
                    "Done".to_owned(),
                    Tone::Normal,
                    sys(SysHit::AskErase),
                ),
                Erase::Refused => (
                    "Erase empty space",
                    "This system does not allow it yet".to_owned(),
                    Tone::Warn,
                    Hit::None,
                ),
                Erase::Idle => (
                    "Erase empty space",
                    if x.rotational {
                        "Writes over it. Takes hours".to_owned()
                    } else {
                        "Trims the drive. A few seconds".to_owned()
                    },
                    Tone::Normal,
                    sys(SysHit::AskErase),
                ),
            };
            items.push(row_with(
                row(Some(G_BOLT), title, sub),
                hit,
                None,
                Trail::None,
                tone,
            ));
        }
        back_only(items)
    }

    // --- acting --------------------------------------------------------------

    fn page_view(&mut self, v: PView) {
        self.gear.pages.view = v;
        self.gear_show_view();
    }

    /// Back out of a details view; returns whether there was one.
    pub(crate) fn pages_back(&mut self) -> bool {
        if self.gear.pages.view == PView::List {
            return false;
        }
        self.page_view(PView::List);
        true
    }

    fn open_path(path: &std::path::Path) {
        run_detached("xdg-open", vec![path.to_string_lossy().into_owned()]);
    }

    pub(crate) fn sys_click(&mut self, hit: SysHit) {
        match hit {
            SysHit::About => self.page_view(PView::About),
            SysHit::Float => self.toggle_floating_mode(),
            SysHit::Awake => {
                let on = Self::caffeine_path().exists();
                run_detached("golem-caffeine", vec![if on { "off" } else { "on" }.into()]);
                // The command's own file is the truth; reflect it at once.
                let path = Self::caffeine_path();
                if on {
                    let _ = std::fs::remove_file(&path);
                } else if let Some(dir) = path.parent() {
                    let _ = std::fs::create_dir_all(dir);
                    let _ = std::fs::write(&path, "");
                }
            }
            SysHit::AllSettings => {
                self.set_stats_box(false);
                self.toggle_control_panel();
            }
            SysHit::Lock => {
                self.set_stats_box(false);
                crate::hypr::dispatch("hl.dsp.exec_cmd(\"hyprlock\")");
            }
            SysHit::Sleep => {
                self.set_stats_box(false);
                run_detached("systemctl", vec!["suspend".into()]);
            }
            SysHit::AskRestart => self.gear_arm("Restart now? Click again", sys(SysHit::DoRestart)),
            SysHit::DoRestart => run_detached("systemctl", vec!["reboot".into()]),
            SysHit::AskOff => self.gear_arm("Power off? Click again", sys(SysHit::DoOff)),
            SysHit::DoOff => run_detached("systemctl", vec!["poweroff".into()]),
            SysHit::App(key) => self.page_view(PView::App(key)),
            SysHit::AppClose(key) => {
                if let Some(a) = self.app(&key) {
                    for addr in a.windows.clone() {
                        crate::hypr::close_window(&addr);
                    }
                }
                if self.gear.pages.view == PView::App(key) {
                    self.page_view(PView::List);
                }
            }
            SysHit::AskKill(key) => {
                let name = self.app(&key).map(|a| self.app_name(a)).unwrap_or_default();
                self.gear_arm(
                    &format!("Force quit {name}? Click again"),
                    sys(SysHit::DoKill(key)),
                );
            }
            SysHit::DoKill(key) => {
                if let Some(pids) = self.app(&key).filter(|a| !a.system).map(|a| a.pids.clone()) {
                    self.sys_send(SysCommand::Kill(pids));
                }
                self.page_view(PView::List);
            }
            SysHit::SystemTasks => self.gear.pages.show_system = !self.gear.pages.show_system,
            SysHit::PowerPage => {
                if let Some(page) = self.stats_page_for(PageKind::Battery) {
                    self.stats_open_page(page);
                }
            }
            SysHit::Mode(profile) => {
                // A mode chosen by hand ends the low-battery saver's claim.
                self.gear.pages.saver_from = None;
                self.gear.pages.live.profile = Some(profile.clone());
                self.sys_send(SysCommand::Profile(profile));
            }
            SysHit::AutoSaver => {
                self.settings.battery_saver_off = !self.settings.battery_saver_off;
                self.settings.save();
            }
            SysHit::ChargeLimit => {
                let on = self
                    .gear
                    .pages
                    .live
                    .battery
                    .as_ref()
                    .is_some_and(|b| b.limit.is_some_and(|l| l <= 80));
                self.sys_send(SysCommand::ChargeLimit(!on));
            }
            SysHit::ScreenOff(secs) => {
                self.settings.screen_off_secs = self.idle_choice(secs, true);
                self.settings.save();
                self.apply_idle();
            }
            SysHit::SleepAfter(secs) => {
                self.settings.sleep_secs = self.idle_choice(secs, false);
                self.settings.save();
                self.apply_idle();
            }
            SysHit::Lid(nothing) => {
                self.settings.lid_nothing = nothing;
                self.settings.save();
                self.hold_lid(nothing);
            }
            SysHit::Health => self.page_view(PView::Health),
            SysHit::Card(i) => self.page_view(PView::Card(i)),
            SysHit::Fast(id) => {
                self.settings.gpu_fast.retain(|a| *a != id);
                self.settings.save();
            }
            SysHit::FastAdd => self.gear_ask(
                PField::FastApp,
                "Name of the app",
                G_PLUS,
                false,
                String::new(),
            ),
            SysHit::Effects(light) => {
                let mut s = crate::display::GolemSettings::load();
                s.set_light_effects(light);
            }
            SysHit::Folder(i) => self.page_view(PView::Folder(i)),
            SysHit::OpenFolder(i) => {
                if let Some(f) = self.gear.pages.disk.folders.get(i) {
                    Self::open_path(&f.path);
                }
                self.set_stats_box(false);
            }
            SysHit::OpenPath(path) => {
                // The folder that holds it: a file would open in its app.
                Self::open_path(if path.is_dir() {
                    &path
                } else {
                    path.parent().unwrap_or(&path)
                });
                self.set_stats_box(false);
            }
            SysHit::Clean => {
                self.gear.pages.freed = None;
                self.page_view(PView::Clean);
            }
            SysHit::Junk(j) => {
                let off = &mut self.gear.pages.junk_off;
                if !off.remove(&j) {
                    off.insert(j);
                }
            }
            SysHit::AskClean => {
                let n = self.junk_bytes(true);
                self.gear_arm(
                    &format!("Remove {} for good? Click again", size_text(n)),
                    sys(SysHit::DoClean),
                );
            }
            SysHit::DoClean => {
                let what: Vec<Junk> = Junk::ALL
                    .into_iter()
                    .filter(|j| self.junk_chosen(*j))
                    .collect();
                self.gear.pages.cleaning = true;
                self.gear.pages.freed = None;
                self.sys_send(SysCommand::Clean(what));
            }
            SysHit::Drive(i) => {
                self.gear.pages.erase = match self.gear.pages.erase {
                    Erase::Running(p) => Erase::Running(p),
                    _ => Erase::Idle,
                };
                self.page_view(PView::Drive(i));
            }
            SysHit::OpenDrive(i) => {
                if let Some(m) = self
                    .gear
                    .pages
                    .disk
                    .drives
                    .get(i)
                    .and_then(|d| d.mount.clone())
                {
                    Self::open_path(std::path::Path::new(&m));
                    self.set_stats_box(false);
                }
            }
            SysHit::AskErase => {
                let hours = self
                    .gear
                    .pages
                    .disk
                    .drives
                    .iter()
                    .any(|d: &Drive| d.system && d.rotational);
                self.gear_arm(
                    if hours {
                        "Erase empty space? Takes hours. Click again"
                    } else {
                        "Erase empty space? Click again"
                    },
                    sys(SysHit::DoErase),
                );
            }
            SysHit::DoErase => {
                self.gear.pages.erase = Erase::Running(0);
                self.sys_send(SysCommand::Erase(true));
            }
            SysHit::StopErase => {
                self.gear.pages.erase = Erase::Idle;
                self.sys_send(SysCommand::Erase(false));
                self.update_stats_reveal();
            }
            other => self.drive_click(other),
        }
    }

    /// The idle time to store for a pick: `None` when it is the system's own.
    fn idle_choice(&self, secs: u32, screen: bool) -> Option<u32> {
        let (base_screen, base_sleep) = idle_base_text().map_or((None, None), |b| idle_base(&b));
        let base = if screen { base_screen } else { base_sleep };
        (Some(secs) != base).then_some(secs)
    }

    /// The app named in the field joins the ones opened on the fast card.
    fn pages_add_fast_app(&mut self, name: &str) {
        let q = name.trim().to_lowercase();
        if q.is_empty() {
            return;
        }
        let found = self
            .entries
            .iter()
            .zip(&self.kinds)
            .filter(|(_, k)| **k == crate::apps::EntryKind::App)
            .map(|(e, _)| e)
            .find(|e| e.name.to_lowercase() == q)
            .or_else(|| {
                self.entries
                    .iter()
                    .zip(&self.kinds)
                    .filter(|(_, k)| **k == crate::apps::EntryKind::App)
                    .map(|(e, _)| e)
                    .find(|e| e.name.to_lowercase().contains(&q))
            })
            .map(|e| e.id.clone());
        if let Some(id) = found {
            if !self.settings.gpu_fast.contains(&id) {
                self.settings.gpu_fast.push(id);
                self.settings.save();
            }
        }
    }

    /// `debug-gear do <what>`: press one of these pages' controls without a
    /// pointer. A question it raises is answered with `debug-gear confirm`, a
    /// field it opens with `debug-gear type …` then `debug-gear enter`.
    pub(crate) fn pages_debug_do(&mut self, what: &str) -> bool {
        let (verb, arg) = what.split_once(' ').unwrap_or((what, ""));
        let ext = self.gear.pages.disk.drives.iter().position(|d| d.removable);
        let secs = arg.parse::<u32>().unwrap_or(0);
        let hit = match (verb, ext) {
            ("ext-mount", Some(i)) => SysHit::ExtMount(i),
            ("ext-check", Some(i)) => SysHit::ExtCheck(i),
            ("ext-rename", Some(i)) => SysHit::ExtRename(i),
            ("ext-unlock", Some(i)) => SysHit::ExtUnlock(i),
            ("ext-format", Some(i)) => SysHit::ExtFormat(i),
            ("eject", Some(i)) => SysHit::Eject(i),
            ("format-for", _) => SysHit::FormatFor(match arg {
                "golem" => Target::Golem,
                "win" => Target::Windows,
                _ => Target::Any,
            }),
            ("format-lock", _) => SysHit::FormatLock,
            ("format-thorough", _) => SysHit::FormatThorough,
            ("format-name", _) => SysHit::FormatName,
            ("format", _) => SysHit::AskFormat,
            ("mode", _) => SysHit::Mode(arg.to_owned()),
            ("saver", _) => SysHit::AutoSaver,
            ("limit", _) => SysHit::ChargeLimit,
            ("awake", _) => SysHit::Awake,
            ("effects", _) => SysHit::Effects(arg == "light"),
            ("lid", _) => SysHit::Lid(arg == "nothing"),
            ("screen", _) => SysHit::ScreenOff(secs),
            ("sleepafter", _) => SysHit::SleepAfter(secs),
            ("updates", _) => SysHit::CheckUpdates,
            ("erase", _) => SysHit::AskErase,
            ("stop-erase", _) => SysHit::StopErase,
            ("app-close", _) => SysHit::AppClose(arg.to_owned()),
            ("app-kill", _) => SysHit::AskKill(arg.to_owned()),
            ("system-tasks", _) => SysHit::SystemTasks,
            ("clean", _) => SysHit::AskClean,
            ("junk", _) => SysHit::Junk(match arg {
                "trash" => Junk::Trash,
                "caches" => Junk::Caches,
                "browser" => Junk::Browser,
                "store" => Junk::Store,
                _ => Junk::System,
            }),
            _ => return false,
        };
        self.gear_click(Hit::Sys(hit));
        true
    }

    /// The open apps as the pages know them, for the debug verb's `state`.
    pub(crate) fn pages_debug_apps(&self) -> String {
        self.gear
            .pages
            .live
            .apps
            .iter()
            .filter(|a| !a.system)
            .map(|a| format!("{}({} win, {} proc)", a.key, a.windows.len(), a.pids.len()))
            .collect::<Vec<_>>()
            .join(" ")
    }

    /// `debug-gear page <what>`: reach a details view without a pointer.
    pub(crate) fn pages_debug(&mut self, what: &str) -> bool {
        let v = match what {
            "about" => PView::About,
            "health" => PView::Health,
            "clean" => PView::Clean,
            "drive" => PView::Drive(0),
            "folder" => PView::Folder(0),
            "card" => PView::Card(0),
            "video" => PView::Video,
            "ext" | "format" => {
                let Some(i) = self.gear.pages.disk.drives.iter().position(|d| d.removable) else {
                    return false;
                };
                if what == "ext" {
                    PView::Drive(i)
                } else {
                    PView::Format(i)
                }
            }
            "app" => match self.gear.pages.live.apps.iter().find(|a| !a.system) {
                Some(a) => PView::App(a.key.clone()),
                None => return false,
            },
            "system" => {
                self.gear.pages.show_system = true;
                PView::List
            }
            _ => return false,
        };
        self.page_view(v);
        true
    }
}

/// `name` cut to what a file system's label can hold. A name one letter too
/// long failed the whole format at its last step — after a thorough erase,
/// half an hour in (the PNY stick, 2026-10-06: "Label for exFAT filesystem is
/// too long").
fn fit_label(name: &str, fstype: &str) -> String {
    match fstype {
        // 11 UTF-16 units (exFAT as the tools here make it; FAT's own limit).
        "exfat" | "vfat" => {
            let mut out = String::new();
            let mut units = 0;
            for c in name.chars() {
                units += c.len_utf16();
                if units > 11 {
                    break;
                }
                out.push(c);
            }
            out
        }
        // 16 bytes.
        "ext4" | "ext3" | "ext2" => {
            let mut end = name.len().min(16);
            while !name.is_char_boundary(end) {
                end -= 1;
            }
            name[..end].to_owned()
        }
        // NTFS: 32 characters.
        _ => name.chars().take(32).collect(),
    }
    .trim()
    .to_owned()
}

fn health_word(pct: u8) -> &'static str {
    match pct {
        80..=u8::MAX => "Good",
        60..=79 => "Worn",
        _ => "Needs replacing",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const BASE: &str = "general {\n  lock_cmd=/nix/store/x-golem-lock\n}\n\nlistener {\n  on-timeout=/nix/store/x-golem-lock\n  timeout=300\n}\n\nlistener {\n  on-resume=/nix/store/y-golem-dpms on\n  on-timeout=/nix/store/y-golem-dpms off\n  timeout=360\n}\n\nlistener {\n  on-timeout=/nix/store/z-golem-idle-suspend\n  timeout=900\n}\n";

    #[test]
    fn a_drive_name_is_cut_to_what_its_file_system_holds() {
        assert_eq!(fit_label("GEARXGEARWIN", "exfat"), "GEARXGEARWI");
        assert_eq!(fit_label("short", "exfat"), "short");
        assert_eq!(
            fit_label("ñandú ñandú ñandú", "ext4").len(),
            15,
            "cut on a character, not inside one"
        );
        assert_eq!(fit_label(&"x".repeat(40), "ntfs").chars().count(), 32);
        assert_eq!(fit_label("ten chars  and", "vfat"), "ten chars");
    }

    #[test]
    fn the_idle_steps_are_read_from_the_systems_own_config() {
        assert_eq!(idle_base(BASE), (Some(360), Some(900)));
        assert_eq!(idle_base("general {\n}\n"), (None, None));
    }

    #[test]
    fn the_owners_times_replace_only_their_own_step() {
        let out = idle_with(BASE, Some(120), None);
        assert_eq!(idle_base(&out), (Some(120), Some(900)));
        assert!(
            out.contains("timeout=300"),
            "the lock step is not ours to move"
        );
        assert!(
            out.contains("golem-dpms on"),
            "the commands are kept as they are"
        );
        let never = idle_with(BASE, Some(0), Some(3600));
        assert_eq!(
            idle_base(&never),
            (None, Some(3600)),
            "never = the step is gone"
        );
        assert_eq!(never.matches("listener").count(), 2);
        assert_eq!(idle_with(BASE, None, None), BASE, "no choice, no change");
    }

    #[test]
    fn sizes_and_spans_read_plainly() {
        assert_eq!(size_text(512 * 1024 * 1024), "512 MB");
        assert_eq!(
            size_text(3 * 1024 * 1024 * 1024 + 200 * 1024 * 1024),
            "3.2 GB"
        );
        assert_eq!(size_text(512 * 1024 * 1024 * 1024), "512 GB");
        assert_eq!(span_text(35), "35 min");
        assert_eq!(span_text(200), "3 h 20 min");
        assert_eq!(span_text(120), "2 h");
        assert_eq!(span_text(3 * 24 * 60), "3 days");
    }

    #[test]
    fn a_tile_wears_the_apps_first_letter() {
        assert_eq!(monogram("seam"), "S");
        assert_eq!(monogram("7zip"), "Z");
        assert_eq!(monogram("123"), G_WINDOW);
    }

    #[test]
    fn a_failed_act_is_told_in_plain_words() {
        assert_eq!(
            plain_problem("Failed to activate device: Incorrect passphrase."),
            "Wrong password"
        );
        assert!(plain_problem("Input/output error while writing").contains("stopped responding"));
        assert_eq!(plain_problem("something else"), "something else");
    }

    #[test]
    fn battery_health_has_three_words() {
        assert_eq!(health_word(88), "Good");
        assert_eq!(health_word(70), "Worn");
        assert_eq!(health_word(40), "Needs replacing");
    }
}

/// The disk service's reason, in a person's words where we know the case.
fn plain_problem(reason: &str) -> &str {
    let r = reason.to_lowercase();
    if r.contains("passphrase") {
        "Wrong password"
    } else if r.contains("input/output") || r.contains("timed out") {
        "The drive stopped responding. It may be failing"
    } else if r.contains("no such interface") {
        "Nothing readable inside. Format it"
    } else if r.contains("not authorized") {
        "This system does not allow it"
    } else {
        reason
    }
}

/// How a file system is named to a person, and who can read it.
fn format_words(fstype: &str) -> &'static str {
    match fstype {
        "exfat" => "exFAT · any computer",
        "vfat" => "FAT · any computer",
        "ext4" | "ext3" | "ext2" => "ext4 · this system",
        "ntfs" | "ntfs3" => "NTFS · Windows",
        "btrfs" => "Btrfs · this system",
        "iso9660" | "udf" => "A disc image · startup stick",
        "" => "Not formatted",
        _ => "Another format",
    }
}

fn target_words(t: Target) -> (&'static str, &'static str) {
    match t {
        Target::Any => ("Anywhere", "exFAT. Windows, Mac, phones, TVs and this one"),
        Target::Golem => ("Golem", "ext4. Linux only. It can be locked"),
        Target::Windows => ("Windows", "NTFS. For a drive that lives on Windows"),
    }
}

impl App {
    fn drive_send(&mut self, what: &'static str, cmd: DriveCommand) {
        if self.gear.pages.drives.is_none() {
            let (tx, rx) = calloop::channel::channel::<DriveEvent>();
            let _ = self.loop_handle.insert_source(rx, |ev, _, app: &mut App| {
                if let calloop::channel::Event::Msg(DriveEvent::Done { what, problem }) = ev {
                    let p = &mut app.gear.pages;
                    p.drive_busy = None;
                    p.drive_note = match (what, problem) {
                        (_, Some(problem)) => Some((plain_problem(&problem).to_owned(), false)),
                        ("check", None) => Some(("No problems found".to_owned(), true)),
                        _ => None,
                    };
                    // An ejected drive is gone; a formatted one is still the
                    // drive the view was opened on.
                    if what == "eject" {
                        p.view = PView::List;
                    }
                    app.sys_send(SysCommand::Refresh);
                    app.update_stats_reveal();
                    app.gear_changed();
                }
            });
            self.gear.pages.drives = Some(crate::drives::spawn(tx));
        }
        self.gear.pages.drive_busy = Some(what);
        self.gear.pages.drive_note = None;
        if let Some(h) = &self.gear.pages.drives {
            h.send(cmd);
        }
    }

    /// A plugged-in drive's one line: what is happening to it, or how full.
    fn ext_status(&self, x: &Drive) -> String {
        if let Some(busy) = self.gear.pages.drive_busy {
            return busy.to_owned();
        }
        match &x.vol {
            Some(v) if v.locked => "Locked. Click to unlock".to_owned(),
            Some(v) => match (&v.mount, v.used, v.size) {
                (Some(_), Some(used), Some(size)) => {
                    format!(
                        "{} of {} free",
                        size_text(size.saturating_sub(used)),
                        size_text(size)
                    )
                }
                _ => "Plugged in, not in use".to_owned(),
            },
            None => format!("{} · not formatted", size_text(x.size)),
        }
    }

    fn ext_view(&self, i: usize, x: &Drive) -> View {
        let p = &self.gear.pages;
        let vol = x.vol.as_ref();
        let mounted = vol.is_some_and(|v| v.mount.is_some());
        let locked = vol.is_some_and(|v| v.locked);
        let mut strip = vec![btn(Hit::Back, G_BACK)];
        if mounted {
            strip.push(btn(sys(SysHit::OpenDrive(i)), G_FOLDER));
        }
        strip.push(btn(sys(SysHit::Eject(i)), G_EJECT));
        let meter = match vol.and_then(|v| v.used.zip(v.size)) {
            Some((used, size)) if mounted && size > 0 => Extra::Meter(
                (used.saturating_mul(100) / size).min(100) as u8,
                format!("{} used", size_text(used)),
            ),
            _ => Extra::None,
        };
        let mut items = vec![Item::Card {
            glyph: G_USB,
            title: x.name.clone(),
            sub: self.ext_status(x),
            extra: meter,
            rename: (vol.is_some() && !locked).then(|| sys(SysHit::ExtRename(i))),
        }];
        if let Some((note, good)) = &p.drive_note {
            items.push(row_with(
                if *good {
                    row(None, note.clone(), "")
                } else {
                    row(None, "That did not work", note.clone())
                },
                Hit::None,
                None,
                Trail::None,
                if *good { Tone::Normal } else { Tone::Warn },
            ));
        }
        let kind = match x.tran.as_str() {
            "usb" => "USB drive",
            "mmc" => "Memory card",
            _ => "Drive",
        };
        items.push(kv("Kind", format!("{kind} · {}", size_text(x.size))));
        items.push(kv(
            "Format",
            match vol {
                Some(v) if v.encrypted => "Locked with a password".to_owned(),
                Some(v) => format_words(&v.fstype).to_owned(),
                None => "Not formatted".to_owned(),
            },
        ));
        let act = |tile, title: &str, sub: &str, hit: SysHit| {
            row_with(
                row(Some(tile), title, sub),
                sys(hit),
                None,
                Trail::None,
                Tone::Normal,
            )
        };
        if locked {
            items.push(act(
                G_LOCK,
                "Unlock",
                "Asks for its password",
                SysHit::ExtUnlock(i),
            ));
        } else if vol.is_some() {
            items.push(if mounted {
                act(
                    G_FOLDER,
                    "Stop using it",
                    "It stays plugged in",
                    SysHit::ExtMount(i),
                )
            } else {
                act(
                    G_FOLDER,
                    "Use it",
                    "Mounts it to open its files",
                    SysHit::ExtMount(i),
                )
            });
            items.push(act(
                G_CHECK,
                "Check for errors",
                "Finds and repairs damage",
                SysHit::ExtCheck(i),
            ));
        }
        items.push(act(
            G_BOLT,
            "Format",
            "Erases everything on it",
            SysHit::ExtFormat(i),
        ));
        items.push(act(
            G_EJECT,
            "Eject",
            "Safe to unplug after",
            SysHit::Eject(i),
        ));
        View {
            strip,
            items,
            sheet_from: Some(1),
            ..View::default()
        }
    }

    fn format_view(&self, i: usize) -> View {
        let p = &self.gear.pages;
        let Some(x) = p.disk.drives.get(i) else {
            return back_only(vec![Item::Empty("It was unplugged".into())]);
        };
        let f = &p.format;
        let mut items = vec![
            Item::Card {
                glyph: G_USB,
                title: format!("Format {}", x.name),
                sub: "Erases everything on it".into(),
                extra: Extra::None,
                rename: None,
            },
            Item::Kv {
                hit: Some(sys(SysHit::FormatName)),
                key: "Name".into(),
                value: if f.name.is_empty() {
                    "Untitled".into()
                } else {
                    f.name.clone()
                },
            },
            Item::Choice {
                key: "For".into(),
                opts: [Target::Any, Target::Golem, Target::Windows]
                    .into_iter()
                    .map(|t| {
                        (
                            sys(SysHit::FormatFor(t)),
                            target_words(t).0.to_owned(),
                            t == f.target,
                        )
                    })
                    .collect(),
            },
            Item::Note(target_words(f.target).1.into()),
        ];
        if f.target == Target::Golem {
            items.push(Item::Toggle {
                hit: sys(SysHit::FormatLock),
                label: "Lock with a password".into(),
                hint: "Asked each time it is plugged in".into(),
                on: f.lock,
            });
        }
        items.push(Item::Toggle {
            hit: sys(SysHit::FormatThorough),
            label: "Erase thoroughly".into(),
            hint: "Writes over everything first. Slow".into(),
            on: f.thorough,
        });
        items.push(row_with(
            row(
                Some(G_BOLT),
                "Format now",
                if f.thorough {
                    "Takes a long time on a big drive"
                } else if f.lock {
                    "Under a minute"
                } else {
                    "A few seconds"
                },
            ),
            sys(SysHit::AskFormat),
            None,
            Trail::None,
            Tone::Normal,
        ));
        back_only(items)
    }

    fn video_view(&self) -> View {
        let mut items = vec![Item::Card {
            glyph: G_FILM,
            title: "Video playback".into(),
            sub: "What the graphics card decodes".into(),
            extra: Extra::None,
            rename: None,
        }];
        for (codec, on_card) in &self.gear.pages.machine.video {
            items.push(kv(
                codec,
                if *on_card {
                    "Graphics card"
                } else {
                    "Processor"
                },
            ));
        }
        items.push(Item::Note(
            "On the card, video keeps the fans quiet.".into(),
        ));
        back_only(items)
    }

    /// The names every volume of a drive goes by, for acts that stop using it.
    fn ext_parts(x: &Drive) -> (Vec<String>, Option<String>) {
        match &x.vol {
            Some(v) => (
                vec![v.fs_dev().to_owned()],
                (v.encrypted && !v.locked).then(|| v.part.clone()),
            ),
            None => (Vec::new(), None),
        }
    }

    fn drive_click(&mut self, hit: SysHit) {
        let drive = |app: &App, i: usize| app.gear.pages.disk.drives.get(i).cloned();
        match hit {
            SysHit::Eject(i) => {
                if let Some(x) = drive(self, i) {
                    let (volumes, locked_part) = Self::ext_parts(&x);
                    self.drive_send(
                        "Ejecting…",
                        DriveCommand::Eject {
                            drive: x.kname,
                            volumes,
                            locked_part,
                        },
                    );
                }
            }
            SysHit::ExtMount(i) => {
                if let Some(v) = drive(self, i).and_then(|x| x.vol) {
                    let dev = v.fs_dev().to_owned();
                    if v.mount.is_some() {
                        self.drive_send("Stopping…", DriveCommand::Unmount(dev));
                    } else {
                        self.drive_send("Starting…", DriveCommand::Mount(dev));
                    }
                }
            }
            SysHit::ExtCheck(i) => {
                if let Some(v) = drive(self, i).and_then(|x| x.vol) {
                    self.drive_send(
                        "Checking…",
                        DriveCommand::Check {
                            volume: v.fs_dev().to_owned(),
                            mounted: v.mount.is_some(),
                        },
                    );
                }
            }
            SysHit::ExtRename(i) => {
                let label = drive(self, i)
                    .and_then(|x| x.vol)
                    .map(|v| v.label)
                    .unwrap_or_default();
                self.gear_ask(
                    PField::DriveName(i),
                    "New name for this drive",
                    G_PENCIL,
                    false,
                    label,
                );
            }
            SysHit::ExtUnlock(i) => {
                self.gear_ask(
                    PField::DrivePassword(i),
                    "Password for this drive",
                    G_LOCK,
                    true,
                    String::new(),
                );
            }
            SysHit::ExtFormat(i) => {
                let x = drive(self, i);
                self.gear.pages.format = FormatDraft {
                    name: x.and_then(|x| x.vol).map(|v| v.label).unwrap_or_default(),
                    ..FormatDraft::default()
                };
                self.page_view(PView::Format(i));
            }
            SysHit::FormatFor(t) => {
                let f = &mut self.gear.pages.format;
                f.target = t;
                if t != Target::Golem {
                    f.lock = false;
                }
            }
            SysHit::FormatLock => self.gear.pages.format.lock = !self.gear.pages.format.lock,
            SysHit::FormatThorough => {
                self.gear.pages.format.thorough = !self.gear.pages.format.thorough;
            }
            SysHit::FormatName => {
                let name = self.gear.pages.format.name.clone();
                self.gear_ask(
                    PField::FormatName,
                    "Name for the drive",
                    G_PENCIL,
                    false,
                    name,
                );
            }
            SysHit::AskFormat => {
                let f = &self.gear.pages.format;
                // A locked drive needs its password before the question.
                if f.lock && f.password.chars().count() < 8 {
                    self.gear_ask(
                        PField::FormatPassword,
                        "Password, 8 characters or more",
                        G_LOCK,
                        true,
                        String::new(),
                    );
                    return;
                }
                self.ask_format();
            }
            SysHit::DoFormat => {
                let PView::Format(i) = self.gear.pages.view else {
                    return;
                };
                if let Some(x) = drive(self, i) {
                    let f = self.gear.pages.format.clone();
                    let (volumes, locked_part) = Self::ext_parts(&x);
                    self.gear.pages.format.password.clear();
                    self.page_view(PView::Drive(i));
                    self.drive_send(
                        "Formatting…",
                        DriveCommand::Format {
                            drive: x.kname,
                            volumes,
                            locked_part,
                            name: fit_label(
                                if f.name.trim().is_empty() {
                                    "Untitled"
                                } else {
                                    f.name.trim()
                                },
                                match f.target {
                                    Target::Any => "exfat",
                                    Target::Golem => "ext4",
                                    Target::Windows => "ntfs",
                                },
                            ),
                            target: f.target,
                            password: (f.lock && f.target == Target::Golem).then_some(f.password),
                            thorough: f.thorough,
                        },
                    );
                }
            }
            SysHit::CheckUpdates => {
                // The system's own updater, started now. It is the system's to
                // allow; the row turns to "Updating now…" when it runs.
                run_detached(
                    "systemctl",
                    vec![
                        "start".into(),
                        "--no-block".into(),
                        "golem-autoupdate.service".into(),
                    ],
                );
            }
            SysHit::Video => self.page_view(PView::Video),
            _ => {}
        }
    }

    fn ask_format(&mut self) {
        let PView::Format(i) = self.gear.pages.view else {
            return;
        };
        let name = self
            .gear
            .pages
            .disk
            .drives
            .get(i)
            .map(|x| x.name.clone())
            .unwrap_or_default();
        self.gear_arm(&format!("Erase {name}? Click again"), sys(SysHit::DoFormat));
    }

    /// The footer field of a machine page was confirmed with `text`.
    pub(crate) fn pages_field_submit(&mut self, what: PField, text: String) {
        let trimmed = text.trim().to_owned();
        match what {
            PField::FastApp => self.pages_add_fast_app(&trimmed),
            PField::DriveName(i) => {
                let vol = self
                    .gear
                    .pages
                    .disk
                    .drives
                    .get(i)
                    .and_then(|x| x.vol.clone());
                if let (Some(v), false) = (vol, trimmed.is_empty()) {
                    self.drive_send(
                        "Renaming…",
                        DriveCommand::Rename {
                            volume: v.fs_dev().to_owned(),
                            label: fit_label(&trimmed, &v.fstype),
                            mounted: v.mount.is_some(),
                        },
                    );
                }
            }
            PField::DrivePassword(i) => {
                let part = self
                    .gear
                    .pages
                    .disk
                    .drives
                    .get(i)
                    .and_then(|x| x.vol.as_ref())
                    .map(|v| v.part.clone());
                if let (Some(part), false) = (part, text.is_empty()) {
                    self.drive_send(
                        "Unlocking…",
                        DriveCommand::Unlock {
                            part,
                            password: text,
                        },
                    );
                }
            }
            PField::FormatName => self.gear.pages.format.name = trimmed,
            PField::FormatPassword => {
                if text.chars().count() >= 8 {
                    self.gear.pages.format.password = text;
                    self.ask_format();
                }
            }
        }
        self.gear_changed();
    }
}
