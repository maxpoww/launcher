//! What is plugged in, on the desktop: a stick or a phone is MOUNTED the
//! moment it arrives and stands on the desktop as an icon until it is
//! ejected (Max, 2026-10-08: *"i want to auto mount sticks and android in to
//! the desktop"*) — macOS's habit.
//!
//! All of it through the session's own volume service (gvfs, which Golem
//! runs: its UDisks2, MTP, gphoto2 and AFC monitors), spoken to with `gio`:
//! `gio mount -o` says when something changes, `gio mount -li` says what
//! there is, `gio mount -d <device>` / `gio mount <address>` mounts it. A
//! stick lands under `/run/media/<user>/<label>`; a phone (MTP: Android in
//! "file transfer" mode) under the runtime dir's `gvfs/` folder, where any
//! program sees it as a folder. One worker thread; it tells the loop the
//! list of what is mounted whenever that changes.
//!
//! A volume is mounted ONCE per time it appears: ejecting it from the
//! desktop must not bring it straight back while the stick is still in the
//! port. A phone that refuses (locked, or only charging) is tried again when
//! it shows up anew — which is what switching it to file transfer does.

use std::collections::{HashMap, HashSet};
use std::io::BufRead;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::mpsc;
use std::time::Duration;

use calloop::channel::Sender;
use tracing::{debug, info, warn};

/// How long the service is let settle after it reports a change (a stick
/// arriving is a burst of lines: the drive, its volume, its media).
const SETTLE: Duration = Duration::from_millis(500);
/// How often the USB bus is looked at for a phone the volume service does
/// not show (a read of sysfs, no process), and a phone that would not open
/// is asked again.
const TICK: Duration = Duration::from_secs(3);
/// How many times one phone is switched to file transfer for its owner.
const NUDGES: u8 = 2;

/// One volume as `gio mount -li` lists it.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct Volume {
    pub name: String,
    /// `/dev/sdb1`, for a volume the disk service knows.
    pub device: Option<String>,
    /// `mtp://…/`, for one that is an address and not a device (a phone).
    pub address: Option<String>,
    /// A phone's USB node (`/dev/bus/usb/003/043`): new each time it is
    /// plugged in, or drops off the port and comes back.
    pub port: Option<String>,
    pub can_mount: bool,
    pub automount: bool,
    /// On a drive that can be taken out, or a phone/camera.
    pub removable: bool,
    /// A phone or a camera, not a disk.
    pub phone: bool,
    /// Where it is mounted, as the service says it (`file://…`, `mtp://…`).
    pub mount: Option<String>,
}

impl Volume {
    /// What it is told apart by, for as long as it is plugged in.
    fn id(&self) -> String {
        // A phone keeps its address when it drops off the port and comes
        // straight back (what it does when its owner allows the computer, or
        // changes its USB mode): by the address alone it was "already
        // tried" and stayed unmounted — a second Android never reached the
        // desktop (Max, 2026-10-09). Its USB node is new each time.
        let phone = || self.address.as_ref().map(|a| format!("{a} {}", self.port.as_deref().unwrap_or("")));
        self.device.clone().or_else(phone).unwrap_or_else(|| self.name.clone())
    }
}

/// Something mounted that belongs on the desktop.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Mounted {
    pub name: String,
    /// The folder it is seen through.
    pub path: PathBuf,
    /// How the service names the mount (what `gio mount -e` is told).
    pub uri: String,
    pub phone: bool,
    /// A phone's USB node (`/dev/bus/usb/003/043`), when the service says.
    pub port: Option<String>,
    /// A phone that is plugged in but will not open (locked, or set to
    /// charge only): it stands on the desktop all the same, `path` being a
    /// name for it and no folder.
    pub closed: bool,
}

/// Read `gio mount -li`: the volumes, each with what its drive says about
/// being removable and where (if anywhere) it is mounted.
pub(crate) fn parse_listing(listing: &str) -> Vec<Volume> {
    let mut out: Vec<Volume> = Vec::new();
    // The drive the current lines belong to is removable; `None` outside
    // any drive.
    let mut drive: Option<bool> = None;
    let mut in_volume = false;
    for raw in listing.lines() {
        let indent = raw.len() - raw.trim_start().len();
        let line = raw.trim();
        if let Some(rest) = line.strip_prefix("Drive(") {
            let _ = rest;
            drive = Some(false);
            in_volume = false;
        } else if let Some(rest) = line.strip_prefix("Volume(") {
            // A volume written at the margin has no drive (a phone).
            if indent == 0 {
                drive = None;
            }
            out.push(Volume {
                name: rest.split_once("): ").map_or("", |(_, n)| n).to_owned(),
                removable: drive == Some(true),
                ..Default::default()
            });
            in_volume = true;
        } else if let Some(rest) = line.strip_prefix("Mount(") {
            // A mount at the margin belongs to no volume (a network place).
            if indent == 0 {
                in_volume = false;
                continue;
            }
            if let (true, Some(vol)) = (in_volume, out.last_mut()) {
                vol.mount = rest
                    .split_once(" -> ")
                    .map(|(_, uri)| uri.trim().to_owned());
            }
        } else if !in_volume {
            if line == "is_removable=1" || line == "is_media_removable=1" {
                drive = drive.map(|_| true);
            }
        } else if let Some(vol) = out.last_mut() {
            // A mount's own lines come after the volume's; only the
            // volume's are read (they are the less indented ones, but the
            // keys below do not occur under a mount anyway).
            if let Some(dev) = line.strip_prefix("unix-device: ") {
                let dev = dev.trim_matches('\'');
                // (A phone's "device" is its USB node, not something to
                // mount — but it is what changes when the phone comes anew.)
                if dev.starts_with("/dev/bus/") {
                    vol.port = Some(dev.to_owned());
                } else {
                    vol.device = Some(dev.to_owned());
                }
            } else if let Some(root) = line.strip_prefix("activation_root=") {
                vol.address = Some(root.to_owned());
            } else if line == "can_mount=1" {
                vol.can_mount = true;
            } else if line == "should_automount=1" {
                vol.automount = true;
            } else if line.starts_with("Type: ")
                && ["MTP", "GPhoto2", "AFC"]
                    .iter()
                    .any(|m| line.contains(&format!("VolumeMonitor{m}")))
            {
                vol.phone = true;
                vol.removable = true;
            }
        }
    }
    out
}

/// The folder a mount is seen through: a `file://` one is its own path; any
/// other (`mtp://host/`) is under the runtime dir's `gvfs/`, as
/// `<scheme>:host=<host>`.
pub(crate) fn mount_path(uri: &str, runtime_dir: &str) -> Option<PathBuf> {
    if let Some(path) = uri.strip_prefix("file://") {
        return Some(PathBuf::from(crate::trash::decode_path(path)));
    }
    let (scheme, rest) = uri.split_once("://")?;
    let host = rest.split('/').next().filter(|h| !h.is_empty())?;
    Some(
        PathBuf::from(runtime_dir)
            .join("gvfs")
            .join(format!("{scheme}:host={host}")),
    )
}

/// What of a listing stands on the desktop: the removable volumes that are
/// mounted somewhere a folder can be found.
pub(crate) fn mounted(volumes: &[Volume], runtime_dir: &str) -> Vec<Mounted> {
    volumes
        .iter()
        .filter(|v| v.removable)
        .filter_map(|v| {
            let uri = v.mount.clone()?;
            Some(Mounted {
                name: v.name.clone(),
                path: mount_path(&uri, runtime_dir)?,
                uri,
                phone: v.phone,
                port: v.port.clone(),
                closed: false,
            })
        })
        .collect()
}

/// A phone seen on the USB bus itself.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct UsbPhone {
    /// `/dev/bus/usb/<bus>/<dev>`, as the volume service spells it.
    pub port: String,
    pub name: String,
    pub serial: Option<String>,
    /// It offers `adb` (USB debugging is on).
    pub adb: bool,
    /// What the volume service will call it once it shows files
    /// (`SAMSUNG_SAMSUNG_Android_R9HN80AD0QJ`), when all three parts are known.
    pub host: Option<String>,
}

/// Makers of phones, by USB vendor id: a device of theirs that is nothing
/// else (no disk, printer, hub, keyboard, camera or sound) is a phone that
/// offers nothing yet — set to charge only.
const PHONE_VENDORS: [&str; 16] = [
    "18d1", "04e8", "2717", "12d1", "2a70", "22d9", "22b8", "1004", "0fce", "0bb4", "2ae5", "05c6", "19d2", "2916",
    "29a9", "0e8d",
];
/// Interface classes that say "not a phone": sound, keyboards and mice,
/// printers, disks, hubs, cameras (UVC), wireless.
const NOT_A_PHONE: [&str; 7] = ["01", "03", "07", "08", "09", "0e", "e0"];

/// The phones on the bus, from the kernel's account under `sysfs`
/// (`/sys/bus/usb/devices`). EVERY Android must land on the desktop (Max,
/// 2026-10-09), and the volume service only shows one that is in file
/// transfer mode. A device is a phone when it offers `adb` (interface
/// ff/42/01) or MTP/PTP (class 06, or an interface that calls itself MTP),
/// or is a phone maker's and nothing else.
pub(crate) fn usb_phones(sysfs: &Path) -> Vec<UsbPhone> {
    let read = |p: PathBuf| std::fs::read_to_string(p).ok().map(|s| s.trim().to_owned());
    let mut out = Vec::new();
    let Ok(dir) = std::fs::read_dir(sysfs) else {
        return out;
    };
    let mut entries: Vec<PathBuf> = dir.flatten().map(|e| e.path()).collect();
    entries.sort();
    for dev in &entries {
        let node = dev.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
        // Devices are `3-9`; `3-9:1.0` is one of its interfaces, `usb3` a root.
        if node.contains(':') || node.starts_with("usb") {
            continue;
        }
        let (Some(bus), Some(num)) = (
            read(dev.join("busnum")).and_then(|s| s.parse::<u32>().ok()),
            read(dev.join("devnum")).and_then(|s| s.parse::<u32>().ok()),
        ) else {
            continue;
        };
        let (mut adb, mut mtp, mut other) = (false, false, false);
        for face in entries.iter().filter(|e| {
            e.file_name().is_some_and(|n| n.to_string_lossy().starts_with(&format!("{node}:")))
        }) {
            let class = read(face.join("bInterfaceClass")).unwrap_or_default();
            let sub = read(face.join("bInterfaceSubClass")).unwrap_or_default();
            let proto = read(face.join("bInterfaceProtocol")).unwrap_or_default();
            let said = read(face.join("interface")).unwrap_or_default();
            adb |= (class.as_str(), sub.as_str(), proto.as_str()) == ("ff", "42", "01");
            mtp |= class == "06" || said.contains("MTP");
            other |= NOT_A_PHONE.contains(&class.as_str());
        }
        let maker = read(dev.join("idVendor")).is_some_and(|v| PHONE_VENDORS.contains(&v.as_str()));
        if !(adb || mtp || (maker && !other)) {
            continue;
        }
        let name = read(dev.join("product"))
            .filter(|p| !p.is_empty())
            .or_else(|| read(dev.join("manufacturer")))
            .map(|n| n.replace('_', " "))
            .unwrap_or_else(|| "Phone".to_owned());
        let serial = read(dev.join("serial")).filter(|s| !s.is_empty());
        let host = match (read(dev.join("manufacturer")), read(dev.join("product")), &serial) {
            (Some(maker), Some(product), Some(serial)) => Some(format!("{maker}_{product}_{serial}").replace(' ', "_")),
            _ => None,
        };
        out.push(UsbPhone { port: format!("/dev/bus/usb/{bus:03}/{num:03}"), name, serial, adb, host });
    }
    out
}

/// The phones that are plugged in and NOT open, to stand on the desktop all
/// the same: one the service shows that refused to mount (`refused`, by
/// volume id), and one only the bus shows. A phone that was open and was
/// ejected is neither: it was let go.
pub(crate) fn closed(
    volumes: &[Volume],
    refused: &HashSet<String>,
    usb: &[UsbPhone],
    runtime_dir: &str,
) -> Vec<Mounted> {
    // Each stands where its folder WILL be once it opens, so the icon keeps
    // its cell: the service's own address for one it shows, and for one only
    // the bus shows the address the service will give it (maker, product and
    // serial, joined its way). A wrong guess costs a moved icon, no more.
    let stand = |name: &str, path: PathBuf, uri: String, port: Option<String>| Mounted {
        name: name.to_owned(),
        path,
        uri,
        phone: true,
        port,
        closed: true,
    };
    let unnamed = |key: &str| {
        let tidy: String = key.chars().map(|c| if c.is_ascii_alphanumeric() { c } else { '-' }).collect();
        PathBuf::from(runtime_dir).join("gvfs").join(format!(".closed-{tidy}"))
    };
    let mut out: Vec<Mounted> = volumes
        .iter()
        .filter(|v| v.phone && v.mount.is_none() && refused.contains(&v.id()))
        .map(|v| {
            let address = v.address.clone().unwrap_or_default();
            let path = mount_path(&address, runtime_dir).unwrap_or_else(|| unnamed(&v.id()));
            stand(&v.name, path, address, v.port.clone())
        })
        .collect();
    for phone in usb {
        if volumes.iter().any(|v| v.port.as_deref() == Some(phone.port.as_str())) {
            continue;
        }
        let path = phone
            .host
            .as_ref()
            .and_then(|host| mount_path(&format!("mtp://{host}/"), runtime_dir))
            .unwrap_or_else(|| unnamed(&phone.port));
        out.push(stand(&phone.name, path, format!("usb:{}", phone.port), Some(phone.port.clone())));
    }
    out
}

/// Whether `adb` has `serial` as a phone that lets this computer in.
fn adb_ready(serial: &str) -> bool {
    Command::new("adb")
        .arg("devices")
        .stderr(Stdio::null())
        .output()
        .is_ok_and(|out| {
            String::from_utf8_lossy(&out.stdout).lines().any(|l| {
                let mut parts = l.split_whitespace();
                parts.next() == Some(serial) && parts.next() == Some("device")
            })
        })
}

/// Switch the phones that are set to charge only, and let this computer in
/// over `adb`, to file transfer — what their owner would do by hand on the
/// phone. A phone the volume service already shows is left alone; each is
/// switched `NUDGES` times at most (it drops off the bus and comes back as a
/// phone with files; one that does not is not asked for ever).
fn nudge(usb: &[UsbPhone], volumes: &[Volume], nudged: &mut HashMap<String, u8>, asked: &mut HashSet<String>) {
    asked.retain(|port| usb.iter().any(|p| &p.port == port));
    for phone in usb {
        let Some(serial) = phone.serial.as_deref() else {
            continue;
        };
        if !phone.adb || volumes.iter().any(|v| v.port.as_deref() == Some(phone.port.as_str())) {
            continue;
        }
        // Once each time it is on the bus: `adb` is a process to run.
        if !asked.insert(phone.port.clone()) {
            continue;
        }
        let times = nudged.entry(serial.to_owned()).or_insert(0);
        if *times >= NUDGES || !adb_ready(serial) {
            continue;
        }
        *times += 1;
        let switched = Command::new("adb")
            .args(["-s", serial, "shell", "svc", "usb", "setFunctions", "mtp"])
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status();
        info!("mounts: {} was on charge only; switched to file transfer ({switched:?})", phone.name);
    }
}

fn listing() -> Option<String> {
    match Command::new("gio")
        .args(["mount", "-li"])
        .stderr(Stdio::null())
        .output()
    {
        Ok(out) if out.status.success() => Some(String::from_utf8_lossy(&out.stdout).into_owned()),
        Ok(_) => None,
        Err(e) => {
            debug!("mounts: no gio ({e}); nothing is mounted for the desktop");
            None
        }
    }
}

/// Mount what has newly appeared and wants it; the list as it is after.
fn scan(tried: &mut HashSet<String>, refused: &mut HashSet<String>) -> Option<Vec<Volume>> {
    let volumes = parse_listing(&listing()?);
    // Gone from the port: next time it comes, it is mounted again.
    let present: HashSet<String> = volumes.iter().map(Volume::id).collect();
    tried.retain(|id| present.contains(id));
    refused.retain(|id| present.contains(id));
    let mut any = false;
    for vol in &volumes {
        if !(vol.removable && vol.can_mount && vol.automount) || vol.mount.is_some() {
            // Mounted by someone (us last time, a file manager): it counts
            // as seen, so an eject is respected.
            if vol.mount.is_some() {
                tried.insert(vol.id());
                refused.remove(&vol.id());
            }
            continue;
        }
        if !tried.insert(vol.id()) {
            continue;
        }
        let mut cmd = Command::new("gio");
        cmd.arg("mount");
        match (&vol.device, &vol.address) {
            (Some(dev), _) => cmd.args(["-d", dev]),
            (None, Some(address)) => cmd.arg(address),
            (None, None) => continue,
        };
        // Never a question on a terminal that is not there (a locked
        // volume's password): it just does not mount.
        let done = cmd
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::piped())
            .output();
        match done {
            Ok(out) if out.status.success() => {
                info!("mounts: {} mounted", vol.name);
                refused.remove(&vol.id());
                any = true;
            }
            Ok(out) => {
                let why = String::from_utf8_lossy(&out.stderr);
                // A phone that says no (locked, or its owner has not yet
                // allowed the computer) is asked again every `TICK` for as
                // long as it is there: unlocking it must be enough. It has
                // never been open, so no eject is being overridden. Said
                // once, not every time.
                if vol.phone {
                    tried.remove(&vol.id());
                    if refused.insert(vol.id()) {
                        warn!("mounts: {} did not mount (asked again while it is plugged in): {}", vol.name, why.trim());
                    }
                } else {
                    warn!("mounts: {} did not mount: {}", vol.name, why.trim());
                }
            }
            Err(e) => warn!("mounts: cannot run gio for {}: {e}", vol.name),
        }
    }
    if any {
        return listing().map(|l| parse_listing(&l));
    }
    Some(volumes)
}

/// Eject what is mounted at `uri` (a stick is powered down where it can be;
/// a phone is just let go). Off the loop: flushing a stick takes a while.
pub(crate) fn eject(uri: String, name: String) {
    std::thread::spawn(move || {
        let run = |flag: &str| {
            Command::new("gio")
                .args(["mount", flag, &uri])
                .stdin(Stdio::null())
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .status()
                .is_ok_and(|s| s.success())
        };
        if run("-e") || run("-u") {
            info!("mounts: {name} ejected");
        } else {
            warn!("mounts: {name} could not be ejected (in use?)");
        }
    });
}

/// Start the watcher: mounts what is plugged in now and from now on, and
/// sends the desktop's list each time it changes.
pub(crate) fn spawn(results: Sender<Vec<Mounted>>) {
    let runtime_dir = std::env::var("XDG_RUNTIME_DIR").unwrap_or_default();
    let spawned = std::thread::Builder::new()
        .name("waverunner-mounts".into())
        .spawn(move || {
            let mut tried: HashSet<String> = HashSet::new();
            let mut refused: HashSet<String> = HashSet::new();
            let mut nudged: HashMap<String, u8> = HashMap::new();
            let mut asked: HashSet<String> = HashSet::new();
            let mut on_bus: Vec<UsbPhone>;
            let sysfs = Path::new("/sys/bus/usb/devices");
            let mut last: Option<Vec<Mounted>> = None;
            loop {
                // The service's own account of changes, a line at a time.
                let (tx, rx) = mpsc::channel::<()>();
                let child = Command::new("gio")
                    .args(["mount", "-o"])
                    .stdin(Stdio::null())
                    .stdout(Stdio::piped())
                    .stderr(Stdio::null())
                    .spawn();
                let mut child = match child {
                    Ok(child) => child,
                    Err(e) => {
                        debug!("mounts: no gio ({e}); sticks and phones are not mounted");
                        return;
                    }
                };
                if let Some(out) = child.stdout.take() {
                    std::thread::spawn(move || {
                        for line in std::io::BufReader::new(out).lines() {
                            if line.is_err() || tx.send(()).is_err() {
                                break;
                            }
                        }
                    });
                }
                loop {
                    on_bus = usb_phones(sysfs);
                    if let Some(volumes) = scan(&mut tried, &mut refused) {
                        nudge(&on_bus, &volumes, &mut nudged, &mut asked);
                        let mut now = mounted(&volumes, &runtime_dir);
                        now.extend(closed(&volumes, &refused, &on_bus, &runtime_dir));
                        if last.as_ref() != Some(&now) {
                            if results.send(now.clone()).is_err() {
                                let _ = child.kill();
                                return; // the loop is gone
                            }
                            last = Some(now);
                        }
                    }
                    // Until something changes — then let the burst finish.
                    // The service says nothing of a phone it does not show,
                    // so the bus is looked at every `TICK` (sysfs only), and
                    // a phone that would not open is asked again.
                    let mut gone = false;
                    loop {
                        match rx.recv_timeout(TICK) {
                            Ok(()) => {
                                std::thread::sleep(SETTLE);
                                while rx.try_recv().is_ok() {}
                                break;
                            }
                            Err(mpsc::RecvTimeoutError::Timeout) => {
                                if !refused.is_empty() || usb_phones(sysfs) != on_bus {
                                    break;
                                }
                            }
                            Err(mpsc::RecvTimeoutError::Disconnected) => {
                                gone = true;
                                break;
                            }
                        }
                    }
                    if gone {
                        break; // the monitor went away: start it again
                    }
                }
                let _ = child.wait();
                std::thread::sleep(Duration::from_secs(5));
            }
        });
    if let Err(e) = spawned {
        warn!("cannot spawn the mounts thread: {e}");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const LISTING: &str = "\
Drive(0): SAMSUNG MZVL21T0HCLR-00BL2
  Type: GProxyDrive (GProxyVolumeMonitorUDisks2)
  ids:
   unix-device: '/dev/nvme0n1'
  is_removable=0
  is_media_removable=0
  can_eject=0
Drive(1): PNY USB 2.0 FD
  Type: GProxyDrive (GProxyVolumeMonitorUDisks2)
  ids:
   unix-device: '/dev/sda'
  is_removable=1
  is_media_removable=1
  can_eject=1
  Volume(0): PHOTOS
    Type: GProxyVolume (GProxyVolumeMonitorUDisks2)
    ids:
     class: 'device'
     unix-device: '/dev/sda1'
     uuid: '6FDF-7443'
     label: 'PHOTOS'
    can_mount=1
    can_eject=1
    should_automount=1
Drive(2): SanDisk Ultra
  Type: GProxyDrive (GProxyVolumeMonitorUDisks2)
  ids:
   unix-device: '/dev/sdb'
  is_removable=1
  Volume(0): My Stick
    Type: GProxyVolume (GProxyVolumeMonitorUDisks2)
    ids:
     unix-device: '/dev/sdb1'
    can_mount=1
    should_automount=1
    Mount(0): My Stick -> file:///run/media/max/My%20Stick
      Type: GProxyMount (GProxyVolumeMonitorUDisks2)
      default_location=file:///run/media/max/My%20Stick
      can_unmount=1
Volume(0): Pixel 8 Pro
  Type: GProxyVolume (GProxyVolumeMonitorMTP)
  ids:
   unix-device: '/dev/bus/usb/001/012'
  activation_root=mtp://Google_Pixel_8_Pro_38ABC/
  can_mount=1
  can_eject=0
  should_automount=1
  Mount(0): Pixel 8 Pro -> mtp://Google_Pixel_8_Pro_38ABC/
    Type: GProxyShadowMount (GProxyVolumeMonitorMTP)
Mount(1): share -> smb://nas/share/
  Type: GDaemonMount
";

    #[test]
    fn the_listing_gives_each_volume_its_drive_its_device_and_its_mount() {
        let v = parse_listing(LISTING);
        assert_eq!(v.len(), 3);
        // A stick not mounted yet: it wants mounting, by its device.
        assert_eq!(v[0].name, "PHOTOS");
        assert_eq!(v[0].device.as_deref(), Some("/dev/sda1"));
        assert!(v[0].removable && v[0].can_mount && v[0].automount && !v[0].phone);
        assert_eq!(v[0].mount, None);
        // One that is.
        assert_eq!(
            v[1].mount.as_deref(),
            Some("file:///run/media/max/My%20Stick")
        );
        // A phone: no drive, an address and not a device, and removable.
        assert_eq!(v[2].name, "Pixel 8 Pro");
        assert_eq!(v[2].device, None);
        assert_eq!(
            v[2].address.as_deref(),
            Some("mtp://Google_Pixel_8_Pro_38ABC/")
        );
        assert!(v[2].phone && v[2].removable);
        assert_eq!(
            v[2].mount.as_deref(),
            Some("mtp://Google_Pixel_8_Pro_38ABC/")
        );
    }

    #[test]
    fn the_desktop_gets_what_is_mounted_and_removable_as_folders() {
        let on = mounted(&parse_listing(LISTING), "/run/user/1000");
        assert_eq!(on.len(), 2);
        assert_eq!(on[0].name, "My Stick");
        assert_eq!(on[0].path, PathBuf::from("/run/media/max/My Stick"));
        assert!(!on[0].phone);
        assert_eq!(on[1].name, "Pixel 8 Pro");
        assert_eq!(
            on[1].path,
            PathBuf::from("/run/user/1000/gvfs/mtp:host=Google_Pixel_8_Pro_38ABC")
        );
        assert!(on[1].phone);
        // The system's own disk is never one of them.
        assert!(parse_listing(LISTING)
            .iter()
            .all(|v| v.name != "SAMSUNG MZVL21T0HCLR-00BL2"));
    }

    /// A fake `/sys/bus/usb/devices`: (node, files).
    fn bus(tag: &str, devices: &[(&str, &[(&str, &str)])]) -> PathBuf {
        let root = std::env::temp_dir().join(format!("wr-bus-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        for (node, files) in devices {
            let dir = root.join(node);
            std::fs::create_dir_all(&dir).unwrap();
            for (name, text) in *files {
                std::fs::write(dir.join(name), format!("{text}\n")).unwrap();
            }
        }
        root
    }

    #[test]
    fn every_android_on_the_bus_is_found() {
        let root = bus(
            "phones",
            &[
                // In file transfer with debugging on.
                ("3-9", &[("busnum", "3"), ("devnum", "43"), ("idVendor", "04e8"), ("manufacturer", "SAMSUNG"), ("product", "SAMSUNG_Android"), ("serial", "R9HN80AD0QJ")]),
                ("3-9:1.0", &[("bInterfaceClass", "06"), ("bInterfaceSubClass", "01"), ("bInterfaceProtocol", "01"), ("interface", "MTP")]),
                ("3-9:1.1", &[("bInterfaceClass", "ff"), ("bInterfaceSubClass", "42"), ("bInterfaceProtocol", "01")]),
                // Charge only, nothing offered: a phone maker's, and nothing else.
                ("3-2", &[("busnum", "3"), ("devnum", "7"), ("idVendor", "18d1"), ("manufacturer", "Google"), ("product", "Pixel 8 Pro"), ("serial", "3A301FDJG000UW")]),
                ("3-2:1.0", &[("bInterfaceClass", "ff"), ("bInterfaceSubClass", "ff"), ("bInterfaceProtocol", "00")]),
                // A Samsung SSD is a disk, not a phone; nor is the webcam, nor a root hub.
                ("4-1", &[("busnum", "4"), ("devnum", "2"), ("idVendor", "04e8"), ("product", "Portable SSD T7")]),
                ("4-1:1.0", &[("bInterfaceClass", "08"), ("bInterfaceSubClass", "06"), ("bInterfaceProtocol", "62")]),
                ("3-4", &[("busnum", "3"), ("devnum", "2"), ("idVendor", "04f2"), ("product", "Integrated Camera")]),
                ("3-4:1.0", &[("bInterfaceClass", "0e"), ("bInterfaceSubClass", "01"), ("bInterfaceProtocol", "00")]),
                ("usb3", &[("busnum", "3"), ("devnum", "1"), ("idVendor", "1d6b")]),
            ],
        );
        let found = usb_phones(&root);
        let _ = std::fs::remove_dir_all(&root);
        let names: Vec<(&str, &str, bool)> = found.iter().map(|p| (p.name.as_str(), p.port.as_str(), p.adb)).collect();
        assert_eq!(
            names,
            vec![("Pixel 8 Pro", "/dev/bus/usb/003/007", false), ("SAMSUNG Android", "/dev/bus/usb/003/043", true)]
        );
        assert_eq!(found[1].serial.as_deref(), Some("R9HN80AD0QJ"));

        // The desktop: the Samsung's volume refused to mount, the Pixel has
        // no volume at all — both stand there, closed. An ejected phone
        // (volume there, not refused) does not.
        let listing = "Volume(0): SAMSUNG Android\n  Type: GProxyVolume (GProxyVolumeMonitorMTP)\n  ids:\n   unix-device: '/dev/bus/usb/003/043'\n  activation_root=mtp://SAMSUNG_SAMSUNG_Android_R9HN80AD0QJ/\n  can_mount=1\n  can_eject=0\n  should_automount=1\n";
        let volumes = parse_listing(listing);
        let refused: HashSet<String> = [volumes[0].id()].into();
        let stand = closed(&volumes, &refused, &found, "/run/user/1000");
        assert_eq!(stand.iter().map(|m| m.name.as_str()).collect::<Vec<_>>(), vec!["SAMSUNG Android", "Pixel 8 Pro"]);
        assert!(stand.iter().all(|m| m.closed && m.phone));
        assert_eq!(stand[1].port.as_deref(), Some("/dev/bus/usb/003/007"));
        // Each stands where its folder will be.
        assert_eq!(stand[0].path, PathBuf::from("/run/user/1000/gvfs/mtp:host=SAMSUNG_SAMSUNG_Android_R9HN80AD0QJ"));
        assert_eq!(stand[1].path, PathBuf::from("/run/user/1000/gvfs/mtp:host=Google_Pixel_8_Pro_3A301FDJG000UW"));
        assert_eq!(found[1].host.as_deref(), Some("SAMSUNG_SAMSUNG_Android_R9HN80AD0QJ"));
        let ejected = closed(&volumes, &HashSet::new(), &found, "/run/user/1000");
        assert_eq!(ejected.iter().map(|m| m.name.as_str()).collect::<Vec<_>>(), vec!["Pixel 8 Pro"]);
    }

    #[test]
    fn a_phone_that_comes_back_is_a_new_arrival() {
        let listing = |node: &str| {
            format!(
                "Volume(0): SAMSUNG Android\n  Type: GProxyVolume (GProxyVolumeMonitorMTP)\n  ids:\n   unix-device: '/dev/bus/usb/003/{node}'\n  activation_root=mtp://SAMSUNG_SAMSUNG_Android_R9HN80AD0QJ/\n  can_mount=1\n  can_eject=0\n  should_automount=1\n"
            )
        };
        let (before, after) = (parse_listing(&listing("042")), parse_listing(&listing("043")));
        assert_eq!(before[0].address, after[0].address);
        assert_eq!(before[0].device, None, "its USB node is not something to mount");
        assert_ne!(before[0].id(), after[0].id(), "off the port and back: to be mounted again");
        assert_eq!(before[0].id(), parse_listing(&listing("042"))[0].id());
    }

    #[test]
    fn a_fixed_disks_volume_is_not_the_desktops() {
        let listing = "\
Drive(0): Internal
  is_removable=0
  Volume(0): Data
    ids:
     unix-device: '/dev/sdc1'
    can_mount=1
    should_automount=1
    Mount(0): Data -> file:///mnt/data
";
        let v = parse_listing(listing);
        assert!(!v[0].removable);
        assert!(mounted(&v, "/run/user/1000").is_empty());
    }
}
