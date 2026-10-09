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

use std::collections::HashSet;
use std::io::BufRead;
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::sync::mpsc;
use std::time::Duration;

use calloop::channel::Sender;
use tracing::{debug, info, warn};

/// How long the service is let settle after it reports a change (a stick
/// arriving is a burst of lines: the drive, its volume, its media).
const SETTLE: Duration = Duration::from_millis(500);

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
            })
        })
        .collect()
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
fn scan(tried: &mut HashSet<String>) -> Option<Vec<Volume>> {
    let volumes = parse_listing(&listing()?);
    // Gone from the port: next time it comes, it is mounted again.
    let present: HashSet<String> = volumes.iter().map(Volume::id).collect();
    tried.retain(|id| present.contains(id));
    let mut any = false;
    for vol in &volumes {
        if !(vol.removable && vol.can_mount && vol.automount) || vol.mount.is_some() {
            // Mounted by someone (us last time, a file manager): it counts
            // as seen, so an eject is respected.
            if vol.mount.is_some() {
                tried.insert(vol.id());
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
                any = true;
            }
            Ok(out) => {
                let why = String::from_utf8_lossy(&out.stderr);
                warn!("mounts: {} did not mount: {}", vol.name, why.trim());
                // A phone that said no is asked again when it comes anew;
                // forgetting it here would ask it on every change.
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
                    if let Some(volumes) = scan(&mut tried) {
                        let now = mounted(&volumes, &runtime_dir);
                        if last.as_ref() != Some(&now) {
                            if results.send(now.clone()).is_err() {
                                let _ = child.kill();
                                return; // the loop is gone
                            }
                            last = Some(now);
                        }
                    }
                    // Until something changes — then let the burst finish.
                    if rx.recv().is_err() {
                        break; // the monitor went away: start it again
                    }
                    std::thread::sleep(SETTLE);
                    while rx.try_recv().is_ok() {}
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
