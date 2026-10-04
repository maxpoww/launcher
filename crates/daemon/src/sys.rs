//! What the gear box's machine pages read and do: the processor, memory,
//! graphics, battery, storage and the computer itself.
//!
//! One worker thread, spawned the first time one of those pages opens, that
//! polls only while a page is on screen ([`SysCommand::Watch`]). Everything is
//! read from the kernel's own files (`/proc`, `/sys`) or from tools Golem
//! already ships (`udevadm`, `lsblk`, `udisksctl`, `powerprofilesctl`); a
//! reading the machine does not offer is simply absent, and its row with it.
//!
//! The slow part — measuring what fills the home folder — runs on a thread of
//! its own so the live readings never wait for it.

use std::collections::HashMap;
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, RecvTimeoutError};
use std::sync::Arc;
use std::time::{Duration, Instant};

use calloop::channel::Sender;
use tracing::{debug, warn};

/// How often the live readings are taken while a page is open.
const POLL: Duration = Duration::from_millis(1500);
/// How long a measurement of the home folder stays good.
const MEASURE_FRESH: Duration = Duration::from_secs(300);
/// What an erase of the empty space leaves untouched, so the system is never
/// left with a full drive under it.
const ERASE_HEADROOM: u64 = 2 * 1024 * 1024 * 1024;
const ERASE_CHUNK: usize = 8 * 1024 * 1024;
/// The system's own cleaner: old system versions and old logs, as root (a
/// Golem unit the owner may start).
pub(crate) const SYSTEM_CLEAN_UNIT: &str = "golem-clean-system.service";

/// The pages this worker feeds.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum SysPage {
    Machine,
    Disk,
    Cpu,
    Ram,
    Gpu,
    Battery,
}

/// What does not change while the machine is on.
#[derive(Debug, Clone, Default, PartialEq)]
pub(crate) struct Machine {
    pub(crate) host: String,
    pub(crate) model: String,
    pub(crate) cpu: String,
    pub(crate) cores: usize,
    pub(crate) mem_bytes: u64,
    pub(crate) os: String,
    pub(crate) cards: Vec<Card>,
    /// `powerprofilesctl` is there to ask.
    pub(crate) has_profiles: bool,
    /// Golem's own system cleaner (old versions, old logs) is installed.
    pub(crate) has_system_clean: bool,
    /// What the graphics card can decode, when the probe is installed:
    /// `(codec, on the card)`.
    pub(crate) video: Vec<(&'static str, bool)>,
}

#[derive(Debug, Clone, Default, PartialEq)]
pub(crate) struct Card {
    pub(crate) name: String,
    pub(crate) driver: String,
    /// The one the firmware started the screen on.
    pub(crate) primary: bool,
    /// Runtime power state: a second card sleeps until something asks for it.
    pub(crate) awake: bool,
}

/// One app (every process under its windows), or one system task.
#[derive(Debug, Clone, Default, PartialEq)]
pub(crate) struct AppUse {
    /// The window class, or the process name for a system task.
    pub(crate) key: String,
    pub(crate) windows: Vec<String>,
    pub(crate) pids: Vec<i32>,
    /// Share of the whole processor, 0–100.
    pub(crate) cpu: f32,
    pub(crate) mem: u64,
    pub(crate) system: bool,
}

#[derive(Debug, Clone, Default, PartialEq)]
pub(crate) struct Battery {
    pub(crate) pct: u8,
    pub(crate) charging: bool,
    pub(crate) full: bool,
    pub(crate) watts: f32,
    /// To empty while discharging, to full while charging.
    pub(crate) minutes: Option<u32>,
    pub(crate) full_wh: f32,
    pub(crate) design_wh: f32,
    pub(crate) cycles: Option<u32>,
    /// The charge limit, where the hardware has one, and whether we may set it.
    pub(crate) limit: Option<u8>,
    pub(crate) limit_writable: bool,
}

/// The readings that move.
#[derive(Debug, Clone, Default, PartialEq)]
pub(crate) struct Live {
    pub(crate) cpu: f32,
    pub(crate) cores: Vec<f32>,
    pub(crate) temp: Option<f32>,
    pub(crate) mem_total: u64,
    pub(crate) mem_apps: u64,
    pub(crate) mem_cache: u64,
    pub(crate) mem_free: u64,
    /// Compressed memory: what went in, and what it takes now.
    pub(crate) packed: Option<(u64, u64)>,
    pub(crate) apps: Vec<AppUse>,
    pub(crate) cards_awake: Vec<bool>,
    pub(crate) battery: Option<Battery>,
    pub(crate) profile: Option<String>,
    pub(crate) profiles: Vec<String>,
    pub(crate) uptime_secs: u64,
    /// The system update is running right now.
    pub(crate) updating: bool,
    pub(crate) last_update_secs: Option<u64>,
}

#[derive(Debug, Clone, Default, PartialEq)]
pub(crate) struct Folder {
    pub(crate) name: String,
    pub(crate) path: PathBuf,
    pub(crate) bytes: u64,
    /// The biggest things directly inside it.
    pub(crate) top: Vec<(String, u64)>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) enum Junk {
    Trash,
    Caches,
    Browser,
    Store,
    /// Old system versions and old logs (the system's own cleaner).
    System,
}

impl Junk {
    pub(crate) const ALL: [Junk; 5] = [
        Junk::Trash,
        Junk::Caches,
        Junk::Browser,
        Junk::Store,
        Junk::System,
    ];
}

/// The part of a plugged-in drive a person uses: its one volume.
#[derive(Debug, Clone, Default, PartialEq)]
pub(crate) struct Volume {
    /// Kernel name of the partition (or of the whole drive when it has no
    /// table).
    pub(crate) part: String,
    /// Kernel name of the unlocked device inside an encrypted partition.
    pub(crate) inside: Option<String>,
    /// The file system ("exfat", "ext4", "ntfs", "vfat"); empty while locked.
    pub(crate) fstype: String,
    pub(crate) label: String,
    pub(crate) mount: Option<String>,
    pub(crate) used: Option<u64>,
    pub(crate) size: Option<u64>,
    pub(crate) encrypted: bool,
    pub(crate) locked: bool,
}

impl Volume {
    /// The device that holds the file system (what is mounted and renamed).
    pub(crate) fn fs_dev(&self) -> &str {
        self.inside.as_deref().unwrap_or(&self.part)
    }
}

#[derive(Debug, Clone, Default, PartialEq)]
pub(crate) struct Drive {
    pub(crate) dev: String,
    /// Kernel name (`sdb`).
    pub(crate) kname: String,
    /// How it is plugged in ("usb", "sata", "nvme").
    pub(crate) tran: String,
    pub(crate) vol: Option<Volume>,
    pub(crate) name: String,
    pub(crate) size: u64,
    pub(crate) rotational: bool,
    pub(crate) removable: bool,
    /// Holds the system.
    pub(crate) system: bool,
    pub(crate) mount: Option<String>,
    pub(crate) free: Option<u64>,
    pub(crate) failing: Option<bool>,
    pub(crate) temp_c: Option<f32>,
    pub(crate) power_on_hours: Option<u64>,
    pub(crate) wear_pct: Option<u8>,
}

#[derive(Debug, Clone, Default, PartialEq)]
pub(crate) struct Disk {
    pub(crate) total: u64,
    pub(crate) used: u64,
    /// The home folder has been measured (it takes a while on a big one).
    pub(crate) measured: bool,
    pub(crate) folders: Vec<Folder>,
    /// Everything on the system drive that is not the home folder.
    pub(crate) system_bytes: u64,
    /// The home folder's files outside the named folders.
    pub(crate) other_bytes: u64,
    pub(crate) junk: Vec<(Junk, Option<u64>)>,
    pub(crate) browser_running: bool,
    pub(crate) drives: Vec<Drive>,
}

#[derive(Debug)]
pub(crate) enum SysCommand {
    Watch(Option<SysPage>),
    /// `(class, address, pid)` of every window, sent by the UI thread (the
    /// compositor is asked there) so processes can be grouped into apps.
    Windows(Vec<(String, String, i32)>),
    Kill(Vec<i32>),
    Profile(String),
    ChargeLimit(bool),
    Clean(Vec<Junk>),
    Erase(bool),
    /// Read the drives again now (one was just changed).
    Refresh,
}

#[derive(Debug)]
pub(crate) enum SysEvent {
    Machine(Machine),
    Live(Box<Live>),
    Disk(Box<Disk>),
    Cleaned {
        freed: u64,
    },
    /// An erase of the empty space: how far, or how it ended.
    Erase {
        pct: Option<u8>,
        ok: bool,
    },
}

pub(crate) struct SysHandle {
    tx: mpsc::Sender<SysCommand>,
}

impl SysHandle {
    pub(crate) fn send(&self, cmd: SysCommand) {
        if let Err(e) = self.tx.send(cmd) {
            warn!("sys: worker gone, dropping command: {e}");
        }
    }
}

pub(crate) fn spawn(events: Sender<SysEvent>) -> SysHandle {
    let (tx, rx) = mpsc::channel();
    let spawned = std::thread::Builder::new()
        .name("sys".into())
        .spawn(move || run(&events, &rx));
    if let Err(e) = spawned {
        warn!("sys: cannot spawn the worker: {e}");
    }
    SysHandle { tx }
}

#[derive(Default)]
struct Prev {
    total: u64,
    idle: u64,
    cores: Vec<(u64, u64)>,
    procs: HashMap<i32, u64>,
}

fn run(events: &Sender<SysEvent>, rx: &mpsc::Receiver<SysCommand>) {
    let machine = read_machine();
    let _ = events.send(SysEvent::Machine(machine.clone()));
    let mut watch: Option<SysPage> = None;
    let mut prev = Prev::default();
    let mut windows: Vec<(String, String, i32)> = Vec::new();
    let mut measured: Option<(Instant, Measure)> = None;
    let (mtx, mrx) = mpsc::channel::<Measure>();
    let mut measuring = false;
    let erase_stop = Arc::new(AtomicBool::new(false));
    let mut disk_tick = 0u32;
    loop {
        let cmd = if watch.is_some() {
            match rx.recv_timeout(POLL) {
                Ok(c) => Some(c),
                Err(RecvTimeoutError::Timeout) => None,
                Err(RecvTimeoutError::Disconnected) => return,
            }
        } else {
            match rx.recv() {
                Ok(c) => Some(c),
                Err(_) => return,
            }
        };
        let mut disk_dirty = false;
        match cmd {
            Some(SysCommand::Watch(page)) => {
                // A page just opened: its first reading needs a baseline to
                // measure a rate against.
                if watch.is_none() && page.is_some() {
                    let _ = read_live(&machine, &mut prev, &windows);
                    std::thread::sleep(Duration::from_millis(250));
                }
                watch = page;
                disk_dirty = page == Some(SysPage::Disk);
            }
            Some(SysCommand::Windows(w)) => {
                windows = w;
                continue;
            }
            Some(SysCommand::Kill(pids)) => {
                for pid in pids {
                    // SAFETY: plain signal delivery; a pid that is gone is an
                    // error return, nothing more.
                    unsafe {
                        libc::kill(pid, libc::SIGKILL);
                    }
                }
            }
            Some(SysCommand::Profile(p)) => run_quiet("powerprofilesctl", &["set", &p]),
            Some(SysCommand::ChargeLimit(on)) => {
                if let Some(path) = battery_dir().map(|d| d.join("charge_control_end_threshold")) {
                    if let Err(e) = std::fs::write(&path, if on { "80" } else { "100" }) {
                        warn!("sys: setting the charge limit: {e}");
                    }
                }
            }
            Some(SysCommand::Clean(what)) => {
                let before = fs_free(&home());
                clean(&what);
                let freed = fs_free(&home()).saturating_sub(before);
                let _ = events.send(SysEvent::Cleaned { freed });
                measured = None;
                disk_dirty = true;
            }
            Some(SysCommand::Erase(start)) => {
                erase_stop.store(!start, Ordering::SeqCst);
                if start {
                    let (events, stop) = (events.clone(), erase_stop.clone());
                    let rotational = system_drive().is_some_and(|d| d.rotational);
                    let _ = std::thread::Builder::new()
                        .name("sys-erase".into())
                        .spawn(move || erase_free_space(&events, &stop, rotational));
                }
            }
            Some(SysCommand::Refresh) => disk_dirty = true,
            None => {}
        }
        if let Ok(m) = mrx.try_recv() {
            measured = Some((Instant::now(), m));
            measuring = false;
            disk_dirty = watch == Some(SysPage::Disk);
        }
        match watch {
            None => {}
            Some(SysPage::Disk) => {
                let stale = measured
                    .as_ref()
                    .is_none_or(|(at, _)| at.elapsed() > MEASURE_FRESH);
                if stale && !measuring {
                    measuring = true;
                    let mtx = mtx.clone();
                    let _ = std::thread::Builder::new()
                        .name("sys-measure".into())
                        .spawn(move || {
                            let _ = mtx.send(measure_home());
                        });
                }
                // Re-read on a change, and every few polls besides: drives
                // come and go (a USB stick).
                disk_tick += 1;
                if disk_dirty || disk_tick % 4 == 1 {
                    let disk = read_disk(measured.as_ref().map(|(_, m)| m));
                    if events.send(SysEvent::Disk(Box::new(disk))).is_err() {
                        return;
                    }
                }
            }
            Some(_) => {
                let live = read_live(&machine, &mut prev, &windows);
                if events.send(SysEvent::Live(Box::new(live))).is_err() {
                    return;
                }
            }
        }
    }
}

fn run_quiet(bin: &str, args: &[&str]) {
    match Command::new(bin).args(args).output() {
        Ok(o) if o.status.success() => {}
        Ok(o) => warn!(
            "sys: {bin} {args:?}: {}",
            String::from_utf8_lossy(&o.stderr).trim()
        ),
        Err(e) => warn!("sys: cannot run {bin}: {e}"),
    }
}

fn out_of(bin: &str, args: &[&str]) -> Option<String> {
    let o = Command::new(bin)
        .env("LC_ALL", "C")
        .args(args)
        .output()
        .ok()?;
    o.status
        .success()
        .then(|| String::from_utf8_lossy(&o.stdout).into_owned())
}

fn read(path: impl AsRef<Path>) -> Option<String> {
    std::fs::read_to_string(path)
        .ok()
        .map(|s| s.trim().to_owned())
}

fn read_num<T: std::str::FromStr>(path: impl AsRef<Path>) -> Option<T> {
    read(path)?.parse().ok()
}

fn home() -> PathBuf {
    std::env::var_os("HOME").map_or_else(|| PathBuf::from("/"), PathBuf::from)
}

// --- the machine -------------------------------------------------------------

fn read_machine() -> Machine {
    let cpuinfo = read("/proc/cpuinfo").unwrap_or_default();
    let cpu = cpuinfo
        .lines()
        .find(|l| l.starts_with("model name"))
        .and_then(|l| l.split(':').nth(1))
        .map(clean_cpu_name)
        .unwrap_or_default();
    let cores = cpuinfo
        .lines()
        .filter(|l| l.starts_with("processor"))
        .count()
        .max(1);
    let dmi = |f: &str| read(format!("/sys/class/dmi/id/{f}")).unwrap_or_default();
    let (vendor, product) = (dmi("sys_vendor"), dmi("product_name"));
    let model = [vendor.as_str(), product.as_str()]
        .iter()
        .filter(|s| !s.is_empty() && !s.contains("O.E.M.") && **s != "System Product Name")
        .copied()
        .collect::<Vec<_>>()
        .join(" ");
    let os = read("/etc/os-release")
        .unwrap_or_default()
        .lines()
        .find_map(|l| l.strip_prefix("PRETTY_NAME="))
        .map(|v| v.trim_matches('"').to_owned())
        .unwrap_or_else(|| "Golem".into());
    Machine {
        host: read("/proc/sys/kernel/hostname").unwrap_or_default(),
        model,
        cpu,
        cores,
        mem_bytes: meminfo().get("MemTotal").copied().unwrap_or(0),
        os,
        cards: read_cards(),
        has_profiles: out_of("powerprofilesctl", &["get"]).is_some(),
        has_system_clean: out_of("systemctl", &["cat", SYSTEM_CLEAN_UNIT]).is_some(),
        video: out_of("vainfo", &[])
            .map(|s| parse_video(&s))
            .unwrap_or_default(),
    }
}

/// Which codecs the card decodes, out of `vainfo`'s profile list.
pub(crate) fn parse_video(vainfo: &str) -> Vec<(&'static str, bool)> {
    let decodes = |profile: &str| {
        vainfo
            .lines()
            .any(|l| l.contains(profile) && l.contains("VAEntrypointVLD"))
    };
    [
        ("H.264", "VAProfileH264"),
        ("HEVC", "VAProfileHEVC"),
        ("VP9", "VAProfileVP9"),
        ("AV1", "VAProfileAV1"),
    ]
    .into_iter()
    .map(|(name, profile)| (name, decodes(profile)))
    .collect()
}

/// "Intel(R) Core(TM) i7-13700H CPU @ 2.40GHz" → "Intel Core i7-13700H".
pub(crate) fn clean_cpu_name(raw: &str) -> String {
    let mut s = raw
        .replace("(R)", "")
        .replace("(TM)", "")
        .replace("(tm)", "");
    for cut in [" CPU", " @", " with ", " w/ "] {
        if let Some(i) = s.find(cut) {
            s.truncate(i);
        }
    }
    s.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn read_cards() -> Vec<Card> {
    let mut cards = Vec::new();
    let Ok(dir) = std::fs::read_dir("/sys/class/drm") else {
        return cards;
    };
    let mut names: Vec<String> = dir
        .filter_map(|e| e.ok())
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .filter(|n| n.starts_with("card") && !n.contains('-'))
        .collect();
    names.sort();
    for n in names {
        let dev = format!("/sys/class/drm/{n}/device");
        let driver = std::fs::read_link(format!("{dev}/driver"))
            .ok()
            .and_then(|p| p.file_name().map(|f| f.to_string_lossy().into_owned()))
            .unwrap_or_default();
        let props = out_of("udevadm", &["info", "-q", "property", "-p", &dev]).unwrap_or_default();
        let prop = |k: &str| {
            props
                .lines()
                .find_map(|l| l.strip_prefix(k))
                .map(|v| v.trim_start_matches('=').to_owned())
        };
        let name = card_name(
            &prop("ID_VENDOR_FROM_DATABASE").unwrap_or_default(),
            &prop("ID_MODEL_FROM_DATABASE").unwrap_or_default(),
            &driver,
        );
        cards.push(Card {
            name,
            primary: read(format!("{dev}/boot_vga")).as_deref() == Some("1"),
            awake: card_awake(&dev),
            driver,
        });
    }
    // The one driving the screen first.
    cards.sort_by_key(|c| !c.primary);
    cards
}

fn card_awake(dev: &str) -> bool {
    read(format!("{dev}/power/runtime_status")).as_deref() != Some("suspended")
}

/// A card's name as a person says it, from the hardware database's long one:
/// the bracketed marketing name when there is one.
pub(crate) fn card_name(vendor: &str, model: &str, driver: &str) -> String {
    let brand = if vendor.contains("NVIDIA") {
        "NVIDIA"
    } else if vendor.contains("Intel") {
        "Intel"
    } else if vendor.contains("Advanced Micro") || vendor.contains("AMD") {
        "AMD"
    } else {
        vendor.split_whitespace().next().unwrap_or("")
    };
    let short = match (model.rfind('['), model.rfind(']')) {
        (Some(a), Some(b)) if b > a => &model[a + 1..b],
        _ => model,
    };
    let short = short
        .replace("GeForce ", "")
        .replace(" Laptop GPU", "")
        .replace(" Max-Q / Mobile", "")
        .replace(" / Max-Q", "")
        .replace("Mobile", "")
        .replace("Graphics", "");
    let short = short.split_whitespace().collect::<Vec<_>>().join(" ");
    let full = format!("{brand} {short}").trim().to_owned();
    if full.is_empty() {
        format!("Graphics ({driver})")
    } else {
        full
    }
}

// --- live readings -----------------------------------------------------------

fn meminfo() -> HashMap<String, u64> {
    read("/proc/meminfo")
        .unwrap_or_default()
        .lines()
        .filter_map(|l| {
            let (k, v) = l.split_once(':')?;
            let kb: u64 = v.split_whitespace().next()?.parse().ok()?;
            Some((k.to_owned(), kb * 1024))
        })
        .collect()
}

/// `(total, idle)` jiffies of one `/proc/stat` cpu line.
fn cpu_times(line: &str) -> Option<(u64, u64)> {
    let v: Vec<u64> = line
        .split_whitespace()
        .skip(1)
        .filter_map(|n| n.parse().ok())
        .collect();
    (v.len() >= 5).then(|| (v.iter().take(8).sum(), v[3] + v[4]))
}

fn pct(prev: (u64, u64), now: (u64, u64)) -> f32 {
    let total = now.0.saturating_sub(prev.0);
    let idle = now.1.saturating_sub(prev.1);
    if total == 0 {
        0.0
    } else {
        (100.0 * (total - idle.min(total)) as f32 / total as f32).clamp(0.0, 100.0)
    }
}

/// The processor's temperature: the package sensor, in whatever name the
/// vendor's driver gives it.
fn cpu_temp() -> Option<f32> {
    let dir = std::fs::read_dir("/sys/class/hwmon").ok()?;
    for e in dir.filter_map(|e| e.ok()) {
        let p = e.path();
        let name = read(p.join("name")).unwrap_or_default();
        if matches!(
            name.as_str(),
            "coretemp" | "k10temp" | "zenpower" | "cpu_thermal"
        ) {
            return read_num::<f32>(p.join("temp1_input")).map(|m| m / 1000.0);
        }
    }
    None
}

/// One process, from `/proc/<pid>/stat`: `(parent, cpu jiffies, resident pages, name)`.
pub(crate) fn parse_stat(stat: &str) -> Option<(i32, u64, u64, String)> {
    // The name is in parentheses and may itself hold spaces or parentheses:
    // split on the LAST one.
    let open = stat.find('(')?;
    let close = stat.rfind(')')?;
    let name = stat.get(open + 1..close)?.to_owned();
    let f: Vec<&str> = stat.get(close + 2..)?.split_whitespace().collect();
    // After the name: state(0) ppid(1) … utime(11) stime(12) … rss(21).
    let ppid = f.get(1)?.parse().ok()?;
    let cpu = f.get(11)?.parse::<u64>().ok()? + f.get(12)?.parse::<u64>().ok()?;
    let rss = f.get(21)?.parse().ok()?;
    Some((ppid, cpu, rss, name))
}

struct Proc {
    pid: i32,
    ppid: i32,
    cpu: u64,
    rss: u64,
    name: String,
}

fn processes() -> Vec<Proc> {
    let Ok(dir) = std::fs::read_dir("/proc") else {
        return Vec::new();
    };
    dir.filter_map(|e| e.ok())
        .filter_map(|e| {
            let pid: i32 = e.file_name().to_str()?.parse().ok()?;
            let (ppid, cpu, rss, name) = parse_stat(&read(e.path().join("stat"))?)?;
            Some(Proc {
                pid,
                ppid,
                cpu,
                rss,
                name,
            })
        })
        .collect()
}

/// Group processes into apps: every process under a window's process belongs
/// to that window's app (a terminal owns what runs in it, a browser its
/// helpers). What is left is the system's own, grouped by name.
fn group_apps(
    procs: &[Proc],
    windows: &[(String, String, i32)],
    prev: &HashMap<i32, u64>,
    total_delta: u64,
) -> Vec<AppUse> {
    const PAGE: u64 = 4096;
    let parent: HashMap<i32, i32> = procs.iter().map(|p| (p.pid, p.ppid)).collect();
    // The first class seen for a pid names it (webapp windows share Seam's).
    let mut root: HashMap<i32, &str> = HashMap::new();
    for (class, _, pid) in windows {
        root.entry(*pid).or_insert(class.as_str());
    }
    let mut out: HashMap<String, AppUse> = HashMap::new();
    for p in procs {
        // Walk up to the nearest window-owning ancestor.
        let mut at = p.pid;
        let mut owner = None;
        for _ in 0..64 {
            if let Some(class) = root.get(&at) {
                owner = Some(*class);
                break;
            }
            match parent.get(&at) {
                Some(&up) if up > 1 => at = up,
                _ => break,
            }
        }
        // Kernel threads have no memory of their own and are nobody's task.
        if owner.is_none() && p.rss == 0 {
            continue;
        }
        let (key, system) = match owner {
            Some(class) => (class.to_owned(), false),
            None => (task_name(&p.name), true),
        };
        let delta = p
            .cpu
            .saturating_sub(prev.get(&p.pid).copied().unwrap_or(p.cpu));
        let app = out.entry(key.clone()).or_insert_with(|| AppUse {
            key,
            system,
            ..AppUse::default()
        });
        app.pids.push(p.pid);
        app.mem += p.rss * PAGE;
        if total_delta > 0 {
            app.cpu += 100.0 * delta as f32 / total_delta as f32;
        }
    }
    for (class, addr, _) in windows {
        if let Some(app) = out.get_mut(class) {
            app.windows.push(addr.clone());
        }
    }
    let mut apps: Vec<AppUse> = out.into_values().collect();
    apps.sort_by(|a, b| b.mem.cmp(&a.mem).then_with(|| a.key.cmp(&b.key)));
    apps
}

/// A system task's name without the packaging's marks: `.foo-wrapped` is
/// `foo` to anyone reading the list.
pub(crate) fn task_name(comm: &str) -> String {
    let Some(n) = comm.strip_prefix('.') else {
        return comm.to_owned();
    };
    // The kernel keeps 15 characters of a name, so the suffix may be cut short.
    match n.rfind("-w") {
        Some(i) if i > 0 && "-wrapped".starts_with(&n[i..]) => n[..i].to_owned(),
        _ => n.to_owned(),
    }
}

fn battery_dir() -> Option<PathBuf> {
    std::fs::read_dir("/sys/class/power_supply")
        .ok()?
        .filter_map(|e| e.ok())
        .map(|e| e.path())
        .find(|p| read(p.join("type")).as_deref() == Some("Battery") && p.join("capacity").exists())
}

fn read_battery() -> Option<Battery> {
    let d = battery_dir()?;
    // Some batteries count energy (µWh), some charge (µAh); with the design
    // voltage the second becomes the first.
    let volts = read_num::<f64>(d.join("voltage_min_design")).map_or(1.0, |v| v / 1e6);
    let wh = |energy: &str, charge: &str| -> Option<f32> {
        read_num::<f64>(d.join(energy))
            .map(|e| e / 1e6)
            .or_else(|| read_num::<f64>(d.join(charge)).map(|c| c / 1e6 * volts))
            .map(|v| v as f32)
    };
    let now = wh("energy_now", "charge_now").unwrap_or(0.0);
    let full_wh = wh("energy_full", "charge_full").unwrap_or(0.0);
    let design_wh = wh("energy_full_design", "charge_full_design").unwrap_or(0.0);
    let watts = read_num::<f64>(d.join("power_now"))
        .map(|p| p / 1e6)
        .or_else(|| read_num::<f64>(d.join("current_now")).map(|c| c / 1e6 * volts))
        .unwrap_or(0.0) as f32;
    let status = read(d.join("status")).unwrap_or_default();
    let charging = status == "Charging";
    let minutes = (watts > 0.3).then(|| {
        let left = if charging {
            (full_wh - now).max(0.0)
        } else {
            now
        };
        (left / watts * 60.0) as u32
    });
    let limit_path = d.join("charge_control_end_threshold");
    Some(Battery {
        pct: read_num(d.join("capacity")).unwrap_or(0),
        charging,
        full: status == "Full" || status == "Not charging",
        watts,
        minutes,
        full_wh,
        design_wh,
        cycles: read_num::<u32>(d.join("cycle_count")).filter(|c| *c > 0),
        limit: read_num(&limit_path),
        limit_writable: std::fs::OpenOptions::new()
            .write(true)
            .open(&limit_path)
            .is_ok(),
    })
}

fn read_live(machine: &Machine, prev: &mut Prev, windows: &[(String, String, i32)]) -> Live {
    let mut live = Live::default();
    let stat = read("/proc/stat").unwrap_or_default();
    let mut lines = stat.lines();
    let all = lines.next().and_then(cpu_times).unwrap_or((0, 0));
    live.cpu = pct((prev.total, prev.idle), all);
    let total_delta = all.0.saturating_sub(prev.total);
    let cores: Vec<(u64, u64)> = lines
        .take_while(|l| l.starts_with("cpu"))
        .filter_map(cpu_times)
        .collect();
    live.cores = cores
        .iter()
        .enumerate()
        .map(|(i, now)| pct(prev.cores.get(i).copied().unwrap_or(*now), *now))
        .collect();
    live.temp = cpu_temp();
    let m = meminfo();
    let get = |k: &str| m.get(k).copied().unwrap_or(0);
    live.mem_total = get("MemTotal");
    live.mem_free = get("MemFree");
    live.mem_cache = get("Buffers") + get("Cached") + get("SReclaimable");
    live.mem_apps = live
        .mem_total
        .saturating_sub(live.mem_free + live.mem_cache);
    if let Some(z) = read("/sys/block/zram0/mm_stat") {
        let v: Vec<u64> = z
            .split_whitespace()
            .filter_map(|n| n.parse().ok())
            .collect();
        if let (Some(orig), Some(compr)) = (v.first(), v.get(1)) {
            live.packed = Some((*orig, *compr));
        }
    }
    let procs = processes();
    live.apps = group_apps(&procs, windows, &prev.procs, total_delta);
    // Reading the state never wakes a sleeping card.
    live.cards_awake = read_cards_awake();
    live.battery = read_battery();
    if machine.has_profiles {
        live.profile = out_of("powerprofilesctl", &["get"]).map(|s| s.trim().to_owned());
        live.profiles = out_of("powerprofilesctl", &["list"])
            .map(|s| parse_profiles(&s))
            .unwrap_or_default();
    }
    live.uptime_secs = read("/proc/uptime")
        .and_then(|s| s.split('.').next().and_then(|n| n.parse().ok()))
        .unwrap_or(0);
    let unit = out_of(
        "systemctl",
        &[
            "show",
            "golem-autoupdate.service",
            "-p",
            "ActiveState",
            "-p",
            "ExecMainExitTimestampMonotonic",
        ],
    )
    .unwrap_or_default();
    live.updating = unit
        .lines()
        .any(|l| l == "ActiveState=activating" || l == "ActiveState=active");
    live.last_update_secs = unit
        .lines()
        .find_map(|l| l.strip_prefix("ExecMainExitTimestampMonotonic="))
        .and_then(|v| v.parse::<u64>().ok())
        .filter(|us| *us > 0)
        .map(|us| live.uptime_secs.saturating_sub(us / 1_000_000));
    prev.total = all.0;
    prev.idle = all.1;
    prev.cores = cores;
    prev.procs = procs.iter().map(|p| (p.pid, p.cpu)).collect();
    live
}

/// Whether each card is awake, in the order [`read_cards`] lists them.
fn read_cards_awake() -> Vec<bool> {
    let Ok(dir) = std::fs::read_dir("/sys/class/drm") else {
        return Vec::new();
    };
    let mut cards: Vec<(bool, String, bool)> = dir
        .filter_map(|e| e.ok())
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .filter(|n| n.starts_with("card") && !n.contains('-'))
        .map(|n| {
            let dev = format!("/sys/class/drm/{n}/device");
            (
                read(format!("{dev}/boot_vga")).as_deref() == Some("1"),
                n,
                card_awake(&dev),
            )
        })
        .collect();
    cards.sort_by(|a, b| a.1.cmp(&b.1));
    cards.sort_by_key(|c| !c.0);
    cards.into_iter().map(|c| c.2).collect()
}

/// The profile names out of `powerprofilesctl list`, slowest first.
pub(crate) fn parse_profiles(list: &str) -> Vec<String> {
    let mut names: Vec<String> = list
        .lines()
        .filter(|l| l.trim_end().ends_with(':') && !l.starts_with("    "))
        .map(|l| {
            l.trim()
                .trim_start_matches('*')
                .trim()
                .trim_end_matches(':')
                .to_owned()
        })
        .collect();
    let rank = |n: &str| match n {
        "power-saver" => 0,
        "balanced" => 1,
        _ => 2,
    };
    names.sort_by_key(|n| rank(n));
    names
}

// --- storage -----------------------------------------------------------------

#[derive(Default)]
struct Measure {
    folders: Vec<Folder>,
    home_total: u64,
    trash: u64,
    caches: u64,
    browser: u64,
}

/// The named folders of a home, as the storage page lists them.
const FOLDERS: [&str; 5] = ["Videos", "Pictures", "Documents", "Downloads", "Music"];
/// Cache folders that are safe to remove under a running session: each is
/// rebuilt by its owner on demand, and none holds anything a person made.
const SAFE_CACHES: [&str; 9] = [
    "thumbnails",
    "mesa_shader_cache",
    "mesa_shader_cache_db",
    "fontconfig",
    "gstreamer-1.0",
    "nix",
    "pip",
    "babl",
    "gegl-0.4",
];
/// Where the browsers keep what they downloaded to show pages faster.
const BROWSER_CACHES: [&str; 3] = ["seam", "mozilla", "chromium"];

/// The size on disk of everything under `path`, staying on its file system
/// and following no links.
pub(crate) fn tree_size(path: &Path) -> u64 {
    let Ok(meta) = std::fs::symlink_metadata(path) else {
        return 0;
    };
    let dev = meta.dev();
    let mut total = meta.blocks() * 512;
    if !meta.is_dir() {
        return total;
    }
    let mut stack = vec![path.to_path_buf()];
    while let Some(dir) = stack.pop() {
        let Ok(entries) = std::fs::read_dir(&dir) else {
            continue;
        };
        for e in entries.filter_map(|e| e.ok()) {
            let Ok(m) = e.metadata() else { continue };
            if m.dev() != dev {
                continue;
            }
            total += m.blocks() * 512;
            if m.is_dir() {
                stack.push(e.path());
            }
        }
    }
    total
}

/// The sizes of a folder's direct children, biggest first.
fn children_sizes(dir: &Path) -> Vec<(String, u64)> {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut out: Vec<(String, u64)> = entries
        .filter_map(|e| e.ok())
        .map(|e| {
            (
                e.file_name().to_string_lossy().into_owned(),
                tree_size(&e.path()),
            )
        })
        .collect();
    out.sort_by_key(|c| std::cmp::Reverse(c.1));
    out
}

fn measure_home() -> Measure {
    let home = home();
    let started = Instant::now();
    let top = children_sizes(&home);
    let mut m = Measure {
        home_total: top.iter().map(|(_, b)| b).sum(),
        ..Measure::default()
    };
    for name in FOLDERS {
        let path = home.join(name);
        if !path.is_dir() {
            continue;
        }
        let mut kids = children_sizes(&path);
        let bytes = kids.iter().map(|(_, b)| b).sum();
        kids.truncate(6);
        m.folders.push(Folder {
            name: name.to_owned(),
            path,
            bytes,
            top: kids,
        });
    }
    m.trash = tree_size(&home.join(".local/share/Trash"));
    let cache = home.join(".cache");
    m.caches = SAFE_CACHES.iter().map(|c| tree_size(&cache.join(c))).sum();
    m.browser = BROWSER_CACHES
        .iter()
        .map(|c| tree_size(&cache.join(c)))
        .sum();
    debug!("sys: measured the home folder in {:?}", started.elapsed());
    m
}

/// `(total, free for the owner)` bytes of the file system holding `path`.
fn fs_space(path: &Path) -> (u64, u64) {
    let Ok(c) = std::ffi::CString::new(path.to_string_lossy().as_bytes()) else {
        return (0, 0);
    };
    // SAFETY: `statvfs` fills the struct it is handed; a failure leaves it
    // zeroed and is reported by the return value.
    let mut s: libc::statvfs = unsafe { std::mem::zeroed() };
    if unsafe { libc::statvfs(c.as_ptr(), &mut s) } != 0 {
        return (0, 0);
    }
    let block = s.f_frsize as u64;
    (s.f_blocks as u64 * block, s.f_bavail as u64 * block)
}

fn fs_free(path: &Path) -> u64 {
    fs_space(path).1
}

fn process_running(names: &[&str]) -> bool {
    processes()
        .iter()
        .any(|p| names.iter().any(|n| p.name.starts_with(n)))
}

fn read_disk(measure: Option<&Measure>) -> Disk {
    let (total, free) = fs_space(&home());
    let used = total.saturating_sub(free);
    let mut disk = Disk {
        total,
        used,
        drives: read_drives(),
        browser_running: process_running(&["seam", ".seam", "firefox", "chromium"]),
        ..Disk::default()
    };
    if let Some(m) = measure {
        disk.measured = true;
        disk.folders = m.folders.clone();
        let named: u64 = m.folders.iter().map(|f| f.bytes).sum();
        disk.other_bytes = m.home_total.saturating_sub(named);
        // Only meaningful when the home folder lives on the system's drive,
        // which is every Golem install.
        disk.system_bytes = used.saturating_sub(m.home_total);
        disk.junk = vec![
            (Junk::Trash, Some(m.trash)),
            (Junk::Caches, Some(m.caches)),
            (Junk::Browser, Some(m.browser)),
            // Unknown until it is done: finding what is unused IS the work.
            (Junk::Store, None),
            (Junk::System, None),
        ];
    }
    disk
}

fn system_drive() -> Option<Drive> {
    read_drives().into_iter().find(|d| d.system)
}

fn read_drives() -> Vec<Drive> {
    let Some(out) = out_of(
        "lsblk",
        &[
            "-J",
            "-b",
            "-o",
            "NAME,KNAME,TYPE,SIZE,ROTA,RM,HOTPLUG,MODEL,VENDOR,TRAN,MOUNTPOINTS,LABEL,FSAVAIL,FSTYPE,FSUSED,FSSIZE",
        ],
    ) else {
        return Vec::new();
    };
    let Ok(json) = serde_json::from_str::<serde_json::Value>(&out) else {
        return Vec::new();
    };
    parse_drives(&json)
        .into_iter()
        .map(|mut d| {
            if !d.removable {
                read_health(&mut d);
            }
            d
        })
        .collect()
}

/// The whole drives out of `lsblk -J`, with where each is mounted.
pub(crate) fn parse_drives(json: &serde_json::Value) -> Vec<Drive> {
    let num = |v: &serde_json::Value| {
        v.as_u64()
            .or_else(|| v.as_str().and_then(|s| s.parse().ok()))
    };
    let flag = |v: &serde_json::Value| v.as_bool().unwrap_or(false) || v.as_str() == Some("1");
    let mut out = Vec::new();
    for d in json["blockdevices"].as_array().into_iter().flatten() {
        if d["type"].as_str() != Some("disk") {
            continue;
        }
        let name = d["name"].as_str().unwrap_or("");
        if name.starts_with("zram") || name.starts_with("loop") {
            continue;
        }
        // An empty card reader is a drive with no size: nothing to show.
        if num(&d["size"]).unwrap_or(0) == 0 {
            continue;
        }
        // Every mount point under this drive (its partitions, and what is
        // layered on them).
        let mut mounts: Vec<(String, Option<u64>)> = Vec::new();
        let mut label = String::new();
        let mut stack = vec![d];
        while let Some(n) = stack.pop() {
            for m in n["mountpoints"].as_array().into_iter().flatten() {
                if let Some(m) = m.as_str() {
                    mounts.push((m.to_owned(), num(&n["fsavail"])));
                }
            }
            if label.is_empty() {
                label = n["label"].as_str().unwrap_or("").to_owned();
            }
            stack.extend(n["children"].as_array().into_iter().flatten());
        }
        let system = mounts
            .iter()
            .any(|(m, _)| m == "/" || m == "/nix" || m == "/nix/store" || m == "/home");
        let removable =
            !system && (flag(&d["rm"]) || flag(&d["hotplug"]) || d["tran"].as_str() == Some("usb"));
        let model = [d["vendor"].as_str(), d["model"].as_str()]
            .iter()
            .flatten()
            .map(|s| s.trim())
            .filter(|s| !s.is_empty())
            .collect::<Vec<_>>()
            .join(" ");
        let shown = mounts
            .iter()
            .find(|(m, _)| !m.starts_with("/boot") && m != "[SWAP]")
            .cloned();
        let vol = removable.then(|| volume_of(d, &num)).flatten();
        out.push(Drive {
            kname: d["kname"].as_str().unwrap_or(name).to_owned(),
            tran: d["tran"].as_str().unwrap_or("").to_owned(),
            vol,
            dev: format!("/dev/{name}"),
            name: if removable && !label.is_empty() {
                label
            } else if model.is_empty() {
                name.to_owned()
            } else {
                model
            },
            size: num(&d["size"]).unwrap_or(0),
            rotational: flag(&d["rota"]),
            removable,
            system,
            mount: shown.as_ref().map(|(m, _)| m.clone()),
            free: shown.and_then(|(_, f)| f),
            ..Drive::default()
        });
    }
    out.sort_by_key(|d| (!d.system, d.removable, d.dev.clone()));
    out
}

/// The volume a person uses on a plugged-in drive: its encrypted partition if
/// it has one, else its biggest partition with a file system, else the drive
/// itself when it was formatted without a table.
fn volume_of(
    drive: &serde_json::Value,
    num: &impl Fn(&serde_json::Value) -> Option<u64>,
) -> Option<Volume> {
    let kname = |n: &serde_json::Value| {
        n["kname"]
            .as_str()
            .or(n["name"].as_str())
            .unwrap_or("")
            .to_owned()
    };
    let fill = |v: &mut Volume, n: &serde_json::Value| {
        v.fstype = n["fstype"].as_str().unwrap_or("").to_owned();
        v.label = n["label"].as_str().unwrap_or("").to_owned();
        v.mount = n["mountpoints"]
            .as_array()
            .into_iter()
            .flatten()
            .find_map(|m| m.as_str().map(str::to_owned));
        v.used = num(&n["fsused"]);
        v.size = num(&n["fssize"]);
    };
    let mut nodes: Vec<&serde_json::Value> =
        drive["children"].as_array().into_iter().flatten().collect();
    nodes.push(drive);
    let has_fs = |n: &&&serde_json::Value| {
        n["fstype"]
            .as_str()
            .is_some_and(|f| !f.is_empty() && f != "swap")
    };
    let pick = nodes
        .iter()
        .find(|n| n["fstype"].as_str() == Some("crypto_LUKS"))
        .or_else(|| {
            nodes
                .iter()
                .filter(has_fs)
                .max_by_key(|n| num(&n["size"]).unwrap_or(0))
        })?;
    let mut v = Volume {
        part: kname(pick),
        ..Volume::default()
    };
    if pick["fstype"].as_str() == Some("crypto_LUKS") {
        v.encrypted = true;
        match pick["children"].as_array().and_then(|c| c.first()) {
            Some(inner) => {
                v.inside = Some(kname(inner));
                fill(&mut v, inner);
            }
            None => v.locked = true,
        }
    } else {
        fill(&mut v, pick);
    }
    Some(v)
}

/// A drive's own health report, through udisks (which may read it without
/// root). A drive that reports nothing keeps its `None`s.
fn read_health(d: &mut Drive) {
    let Some(info) = out_of("udisksctl", &["info", "-b", &d.dev]) else {
        return;
    };
    let Some(drive) = info
        .lines()
        .find_map(|l| l.trim().strip_prefix("Drive:"))
        .map(|v| v.trim().trim_matches('\'').to_owned())
    else {
        return;
    };
    let Some(text) = out_of(
        "udisksctl",
        &["info", "-d", drive.rsplit('/').next().unwrap_or("")],
    ) else {
        return;
    };
    parse_health(&text, d);
}

pub(crate) fn parse_health(text: &str, d: &mut Drive) {
    let field = |key: &str| {
        text.lines()
            .find_map(|l| l.trim().strip_prefix(key))
            .map(|v| v.trim().to_owned())
    };
    if let Some(v) = field("SmartFailing:") {
        d.failing = Some(v == "true");
    }
    // Kelvin, as udisks reports it; zero means "not known".
    if let Some(k) = field("SmartTemperature:")
        .and_then(|v| v.parse::<f32>().ok())
        .filter(|k| *k > 1.0)
    {
        d.temp_c = Some(k - 273.15);
    }
    if let Some(s) = field("SmartPowerOnSeconds:")
        .and_then(|v| v.parse::<u64>().ok())
        .filter(|s| *s > 0)
    {
        d.power_on_hours = Some(s / 3600);
    }
    if let Some(h) = field("SmartPowerOnHours:")
        .and_then(|v| v.parse::<u64>().ok())
        .filter(|h| *h > 0)
    {
        d.power_on_hours = Some(h);
    }
    if let Some(w) = field("SmartPercentageUsed:").and_then(|v| v.parse::<u8>().ok()) {
        d.wear_pct = Some(w);
    }
    // NVMe reports trouble as a list of warnings instead of one flag.
    if let Some(w) = field("SmartCriticalWarning:") {
        d.failing = Some(!w.is_empty() && w != "[]" && w != "0");
    }
}

/// Remove what was chosen. Nothing here touches a file a person made.
fn clean(what: &[Junk]) {
    let home = home();
    let cache = home.join(".cache");
    // The system's cleaner first: the store pass after it then finds what the
    // removed versions were holding.
    let mut what: Vec<Junk> = what.to_vec();
    what.sort_by_key(|j| *j != Junk::System);
    for j in &what {
        match j {
            Junk::Trash => {
                if let Err(e) = crate::trash::Trash::home().empty() {
                    warn!("sys: emptying the trash: {e}");
                }
            }
            Junk::Caches => {
                for c in SAFE_CACHES {
                    remove_tree(&cache.join(c));
                }
            }
            Junk::Browser => {
                // A running browser has these files open; it is asked to
                // close first (the page says so) and skipped otherwise.
                if !process_running(&["seam", ".seam", "firefox", "chromium"]) {
                    for c in BROWSER_CACHES {
                        remove_tree(&cache.join(c));
                    }
                }
            }
            // What no system version still uses. Old versions themselves are
            // the system's to remove (it keeps the recent ones by its own rule).
            Junk::Store => run_quiet("nix-collect-garbage", &[]),
            Junk::System => run_quiet("systemctl", &["start", SYSTEM_CLEAN_UNIT]),
        }
    }
}

fn remove_tree(path: &Path) {
    if path.exists() {
        if let Err(e) = std::fs::remove_dir_all(path) {
            warn!("sys: removing {}: {e}", path.display());
        }
    }
}

/// Write over the home drive's empty space, then give it back. On a spinning
/// disk that is what makes deleted files unreadable; an SSD is told to drop
/// them instead (a trim), which needs the system's own permission.
fn erase_free_space(events: &Sender<SysEvent>, stop: &AtomicBool, rotational: bool) {
    if !rotational {
        // Through the system's own weekly trim unit: starting it is the
        // system's to allow (a policy rule), and it says so when it does not.
        let ok = Command::new("systemctl")
            .args(["start", "fstrim.service"])
            .output()
            .is_ok_and(|o| o.status.success());
        let _ = events.send(SysEvent::Erase { pct: None, ok });
        return;
    }
    let path = home().join(".golem-erase.tmp");
    let start_free = fs_free(&home());
    let goal = start_free.saturating_sub(ERASE_HEADROOM);
    let zeros = vec![0u8; ERASE_CHUNK];
    let mut written = 0u64;
    let mut ok = false;
    if let Ok(mut f) = std::fs::File::create(&path) {
        use std::io::Write;
        let mut last = 0u8;
        loop {
            if stop.load(Ordering::SeqCst) {
                break;
            }
            if written >= goal || fs_free(&home()) <= ERASE_HEADROOM {
                ok = f.sync_all().is_ok();
                break;
            }
            if f.write_all(&zeros).is_err() {
                // Full sooner than expected: what could be written, was.
                ok = f.sync_all().is_ok();
                break;
            }
            written += ERASE_CHUNK as u64;
            let pct = (written.saturating_mul(100) / goal.max(1)).min(99) as u8;
            if pct != last {
                last = pct;
                let _ = events.send(SysEvent::Erase {
                    pct: Some(pct),
                    ok: true,
                });
            }
        }
    }
    let _ = std::fs::remove_file(&path);
    let _ = events.send(SysEvent::Erase { pct: None, ok });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_process_line_survives_a_name_with_spaces_and_parentheses() {
        let stat = "4242 (Web Content (x)) S 100 4242 4242 0 -1 4194560 1 2 3 4 500 250 0 0 20 0 9 0 12345 1000000 2048 18446744073709551615";
        let (ppid, cpu, rss, name) = parse_stat(stat).expect("parses");
        assert_eq!((ppid, cpu, rss), (100, 750, 2048));
        assert_eq!(name, "Web Content (x)");
    }

    #[test]
    fn processes_belong_to_the_window_above_them() {
        let p = |pid, ppid, cpu, rss, name: &str| Proc {
            pid,
            ppid,
            cpu,
            rss,
            name: name.into(),
        };
        let procs = vec![
            p(10, 1, 100, 50, "seam"),
            p(11, 10, 300, 100, "Web Content"),
            p(20, 1, 40, 10, "foot"),
            p(21, 20, 0, 5, "zsh"),
            p(22, 21, 60, 200, "cargo"),
            p(30, 1, 10, 4, "pipewire"),
            p(2, 0, 0, 0, "kthreadd"),
        ];
        let windows = vec![
            ("seam".to_owned(), "0xa".to_owned(), 10),
            ("webapp-mail".to_owned(), "0xb".to_owned(), 10),
            ("foot".to_owned(), "0xc".to_owned(), 20),
        ];
        let prev: HashMap<i32, u64> = procs.iter().map(|p| (p.pid, 0)).collect();
        let apps = group_apps(&procs, &windows, &prev, 1000);
        let get = |k: &str| apps.iter().find(|a| a.key == k).expect(k);
        let seam = get("seam");
        assert_eq!(seam.pids.len(), 2, "the browser owns its helpers");
        assert!((seam.cpu - 40.0).abs() < 0.01);
        assert_eq!(seam.mem, 150 * 4096);
        let term = get("foot");
        assert_eq!(term.pids.len(), 3, "a terminal owns what runs in it");
        assert!((term.cpu - 10.0).abs() < 0.01);
        assert!(get("pipewire").system);
        assert_eq!(task_name(".Hyprland-wrapp"), "Hyprland");
        assert_eq!(task_name(".easyeffects-wr"), "easyeffects");
        assert_eq!(task_name("pipewire"), "pipewire");
        assert_eq!(
            task_name("kwin-wayland"),
            "kwin-wayland",
            "only the packaging's own mark"
        );
        assert!(
            apps.iter().all(|a| a.key != "kthreadd"),
            "kernel threads are nobody's task"
        );
        assert!(
            apps.iter().all(|a| a.key != "webapp-mail"),
            "one process, one app"
        );
    }

    #[test]
    fn names_read_the_way_a_person_says_them() {
        assert_eq!(
            clean_cpu_name(" Intel(R) Core(TM) i7-13700H CPU @ 2.40GHz"),
            "Intel Core i7-13700H"
        );
        assert_eq!(
            clean_cpu_name("AMD Ryzen 5 4500U with Radeon Graphics"),
            "AMD Ryzen 5 4500U"
        );
        assert_eq!(
            card_name(
                "NVIDIA Corporation",
                "AD107M [GeForce RTX 4050 Max-Q / Mobile]",
                "nvidia"
            ),
            "NVIDIA RTX 4050"
        );
        assert_eq!(
            card_name(
                "Intel Corporation",
                "Raptor Lake-P [Iris Xe Graphics]",
                "i915"
            ),
            "Intel Iris Xe"
        );
        assert_eq!(card_name("", "", "virtio"), "Graphics (virtio)");
    }

    #[test]
    fn the_power_modes_come_out_slowest_first() {
        let list = "  performance:\n    CpuDriver:\tintel_pstate\n\n* balanced:\n    CpuDriver:\tintel_pstate\n\n  power-saver:\n    CpuDriver:\tintel_pstate\n";
        assert_eq!(
            parse_profiles(list),
            ["power-saver", "balanced", "performance"]
        );
    }

    #[test]
    fn drives_are_told_apart_by_where_they_are_mounted() {
        let json = serde_json::json!({ "blockdevices": [
            { "name": "nvme0n1", "type": "disk", "size": 512_000_000_000u64, "rota": false, "rm": false,
              "hotplug": false, "model": "SAMSUNG MZVL2512", "tran": "nvme", "mountpoints": [null],
              "children": [
                { "name": "nvme0n1p1", "type": "part", "mountpoints": ["/boot"], "fsavail": 400_000_000u64 },
                { "name": "nvme0n1p2", "type": "part", "mountpoints": ["/nix/store", "/"], "fsavail": 300_000_000_000u64 } ] },
            { "name": "sda", "type": "disk", "size": 32_000_000_000u64, "rota": true, "rm": true,
              "hotplug": true, "model": "Ultra", "vendor": "SanDisk ", "tran": "usb", "mountpoints": [null],
              "children": [ { "name": "sda1", "kname": "sda1", "type": "part", "label": "PHOTOS", "size": 31_000_000_000u64,
                              "fstype": "exfat", "fsused": 20_000_000_000u64, "fssize": 32_000_000_000u64,
                              "mountpoints": ["/run/media/max/PHOTOS"], "fsavail": 12_000_000_000u64 } ] },
            { "name": "zram0", "type": "disk", "size": 8_000_000_000u64, "mountpoints": ["[SWAP]"] },
            { "name": "sdb", "type": "disk", "size": 0, "rm": true, "tran": "usb", "mountpoints": [null] }
        ]});
        let d = parse_drives(&json);
        assert_eq!(
            d.len(),
            2,
            "compressed memory and an empty card reader are not drives"
        );
        assert!(d[0].system && !d[0].removable && !d[0].rotational);
        assert_eq!(d[0].name, "SAMSUNG MZVL2512");
        assert_eq!(d[0].free, Some(300_000_000_000));
        assert!(d[1].removable && !d[1].system);
        assert_eq!(d[1].name, "PHOTOS", "a stick goes by its label");
        assert_eq!(d[1].mount.as_deref(), Some("/run/media/max/PHOTOS"));
        let v = d[1].vol.as_ref().expect("a stick has a volume");
        assert_eq!(
            (v.part.as_str(), v.fstype.as_str(), v.label.as_str()),
            ("sda1", "exfat", "PHOTOS")
        );
        assert_eq!(v.used, Some(20_000_000_000));
        assert!(!v.encrypted && !v.locked && v.fs_dev() == "sda1");
        assert!(
            d[0].vol.is_none(),
            "the system's drive is not a volume to act on"
        );
    }

    #[test]
    fn a_locked_stick_is_told_from_an_unlocked_one() {
        let stick = |children: serde_json::Value| {
            serde_json::json!({ "blockdevices": [
                { "name": "sdb", "kname": "sdb", "type": "disk", "size": 64_000_000_000u64, "rm": true, "tran": "usb",
                  "mountpoints": [null],
                  "children": [ { "name": "sdb1", "kname": "sdb1", "type": "part", "size": 63_000_000_000u64,
                                  "fstype": "crypto_LUKS", "mountpoints": [null], "children": children } ] } ]})
        };
        let locked = parse_drives(&stick(serde_json::json!([])));
        let v = locked[0].vol.as_ref().expect("volume");
        assert!(v.encrypted && v.locked && v.mount.is_none());
        let open = parse_drives(&stick(serde_json::json!([
            { "name": "luks-1234", "kname": "dm-0", "type": "crypt", "fstype": "ext4", "label": "Backups",
              "mountpoints": ["/run/media/max/Backups"], "fsused": 1_000u64, "fssize": 60_000_000_000u64 } ])));
        let v = open[0].vol.as_ref().expect("volume");
        assert!(v.encrypted && !v.locked);
        assert_eq!(
            (v.part.as_str(), v.fs_dev(), v.label.as_str()),
            ("sdb1", "dm-0", "Backups")
        );
    }

    #[test]
    fn the_cards_decoders_are_read_from_the_probe() {
        let out = "      VAProfileH264Main               :	VAEntrypointVLD\n      VAProfileH264Main               :	VAEntrypointEncSlice\n      VAProfileHEVCMain               :	VAEntrypointVLD\n      VAProfileVP9Profile0            :	VAEntrypointVLD\n      VAProfileAV1Profile0            :	VAEntrypointEncSlice\n";
        assert_eq!(
            parse_video(out),
            [
                ("H.264", true),
                ("HEVC", true),
                ("VP9", true),
                ("AV1", false)
            ]
        );
    }

    #[test]
    fn a_drive_reports_its_own_health() {
        let mut d = Drive::default();
        parse_health(
            "  org.freedesktop.UDisks2.Drive.Ata:\n    SmartFailing:               false\n    SmartPowerOnSeconds:        18316800\n    SmartTemperature:           309.15\n",
            &mut d,
        );
        assert_eq!(d.failing, Some(false));
        assert_eq!(d.power_on_hours, Some(5088));
        assert!((d.temp_c.expect("temp") - 36.0).abs() < 0.1);
        let mut n = Drive::default();
        parse_health(
            "  org.freedesktop.UDisks2.NVMe.Controller:\n    SmartCriticalWarning:       []\n    SmartPercentageUsed:        3\n    SmartPowerOnHours:          2100\n    SmartTemperature:           312\n",
            &mut n,
        );
        assert_eq!(
            (n.failing, n.wear_pct, n.power_on_hours),
            (Some(false), Some(3), Some(2100))
        );
    }

    #[test]
    fn a_folder_is_measured_by_what_it_takes_on_disk() {
        let dir = std::env::temp_dir().join(format!("golem-sys-{}", std::process::id()));
        std::fs::create_dir_all(dir.join("a/b")).expect("dirs");
        std::fs::write(dir.join("a/b/big"), vec![1u8; 300_000]).expect("write");
        std::fs::write(dir.join("small"), b"x").expect("write");
        let total = tree_size(&dir);
        assert!(total >= 300_000, "counts what is nested: {total}");
        let kids = children_sizes(&dir);
        assert_eq!(kids[0].0, "a", "biggest first");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
