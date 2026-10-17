//! What can be done to a plugged-in drive, through the system's disk service
//! (UDisks2 on the system bus): use it or stop using it, eject, rename, check
//! for errors, unlock, format.
//!
//! The service lets the logged-in owner do all of this on removable drives
//! without a password, which is why none of it needs a helper of ours. Reading
//! the drives is `sys.rs`'s job (`lsblk`); this worker only acts, and says how
//! each act ended. A password travels over the bus, never on a command line.

use std::collections::HashMap;

use calloop::channel::Sender;
use tokio::sync::mpsc;
use tracing::{debug, warn};
use zbus::zvariant::{OwnedObjectPath, Value};
use zbus::{Connection, Proxy};

const DEST: &str = "org.freedesktop.UDisks2";
const BLOCK: &str = "org.freedesktop.UDisks2.Block";
const FS: &str = "org.freedesktop.UDisks2.Filesystem";
const ENCRYPTED: &str = "org.freedesktop.UDisks2.Encrypted";
const TABLE: &str = "org.freedesktop.UDisks2.PartitionTable";
const DRIVE: &str = "org.freedesktop.UDisks2.Drive";

type Opts<'a> = HashMap<&'a str, Value<'a>>;

/// What a drive is formatted for.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub(crate) enum Target {
    /// exFAT: every computer, phone and television.
    #[default]
    Any,
    /// ext4: this system (and the one that can be locked).
    Golem,
    /// NTFS.
    Windows,
    /// FAT32: what cameras, car radios and older things read (Max,
    /// 2026-10-17: a camera's memory card wants it back the way it came).
    Camera,
}

impl Target {
    pub(crate) fn fstype(self) -> &'static str {
        match self {
            Target::Any => "exfat",
            Target::Golem => "ext4",
            Target::Windows => "ntfs",
            Target::Camera => "vfat",
        }
    }
}

/// What the owner saw of a drive when they chose to format it: checked
/// against the service's own view of the device right before, because a
/// kernel name (`sdb`) is given again to the next drive plugged in where one
/// was pulled.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub(crate) struct Expect {
    pub size: u64,
    /// Its serial, where both sides have one (compared only then).
    pub serial: String,
}

/// Every command names devices by their kernel name (`sdb`, `sdb1`, `dm-0`).
#[derive(Debug)]
pub(crate) enum DriveCommand {
    Mount(String),
    Unmount(String),
    /// Stop using every volume, then cut the drive's power.
    Eject {
        drive: String,
        volumes: Vec<String>,
        locked_part: Option<String>,
    },
    Rename {
        volume: String,
        label: String,
        mounted: bool,
        /// The file system's uuid as seen when asked (`None`: not known).
        uuid: Option<String>,
    },
    Check {
        volume: String,
        mounted: bool,
        uuid: Option<String>,
    },
    Unlock {
        part: String,
        password: String,
    },
    Format {
        drive: String,
        volumes: Vec<String>,
        locked_part: Option<String>,
        name: String,
        target: Target,
        password: Option<String>,
        thorough: bool,
        expect: Expect,
    },
}

#[derive(Debug)]
pub(crate) enum DriveEvent {
    /// An act ended: `what` says which, `problem` is `None` when it went well.
    Done {
        what: &'static str,
        problem: Option<String>,
    },
}

pub(crate) struct DriveHandle {
    tx: mpsc::UnboundedSender<DriveCommand>,
}

impl DriveHandle {
    /// False when the worker is gone (the command was dropped).
    pub(crate) fn send(&self, cmd: DriveCommand) -> bool {
        match self.tx.send(cmd) {
            Ok(()) => true,
            Err(e) => {
                warn!("drives: worker gone, dropping command: {e}");
                false
            }
        }
    }
}

pub(crate) fn spawn(events: Sender<DriveEvent>) -> DriveHandle {
    let (tx, mut rx) = mpsc::unbounded_channel::<DriveCommand>();
    let spawned = std::thread::Builder::new()
        .name("drives".into())
        .spawn(move || {
            let rt = match tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
            {
                Ok(rt) => rt,
                Err(e) => {
                    warn!("drives: cannot build runtime: {e}");
                    return;
                }
            };
            rt.block_on(async move {
                while let Some(cmd) = rx.recv().await {
                    // Without the password in the log.
                    debug!("drives: {}", command_name(&cmd));
                    let what = command_name(&cmd);
                    let res = match Connection::system().await {
                        Ok(conn) => run(&conn, cmd).await,
                        Err(e) => Err(e),
                    };
                    let problem = res.err().map(|e| plain_error(&e));
                    if let Some(p) = &problem {
                        warn!("drives: {what} failed: {p}");
                    }
                    if events.send(DriveEvent::Done { what, problem }).is_err() {
                        return;
                    }
                }
            });
        });
    if let Err(e) = spawned {
        warn!("drives: cannot spawn the worker: {e}");
    }
    DriveHandle { tx }
}

fn command_name(cmd: &DriveCommand) -> &'static str {
    match cmd {
        DriveCommand::Mount(_) => "mount",
        DriveCommand::Unmount(_) => "unmount",
        DriveCommand::Eject { .. } => "eject",
        DriveCommand::Rename { .. } => "rename",
        DriveCommand::Check { .. } => "check",
        DriveCommand::Unlock { .. } => "unlock",
        DriveCommand::Format { .. } => "format",
    }
}

/// The service's error as one plain line (it prefixes them with its own
/// error names).
fn plain_error(e: &zbus::Error) -> String {
    let text = match e {
        zbus::Error::MethodError(_, Some(msg), _) => msg.clone(),
        other => other.to_string(),
    };
    text.rsplit(": ").next().unwrap_or(&text).trim().to_owned()
}

/// A device's object in the disk service: its kernel name, with everything
/// but letters and digits written as `_xx`.
pub(crate) fn object_path(kname: &str) -> String {
    let mut out = String::from("/org/freedesktop/UDisks2/block_devices/");
    for b in kname.bytes() {
        if b.is_ascii_alphanumeric() {
            out.push(b as char);
        } else {
            out.push_str(&format!("_{b:02x}"));
        }
    }
    out
}

async fn proxy<'a>(
    conn: &'a Connection,
    kname: &str,
    iface: &'static str,
) -> zbus::Result<Proxy<'a>> {
    Proxy::new(conn, DEST, object_path(kname), iface).await
}

fn none<'a>() -> Opts<'a> {
    HashMap::new()
}

async fn mount(conn: &Connection, kname: &str) -> zbus::Result<()> {
    proxy(conn, kname, FS)
        .await?
        .call::<_, _, String>("Mount", &(none(),))
        .await
        .map(|_| ())
}

async fn unmount(conn: &Connection, kname: &str) -> zbus::Result<()> {
    proxy(conn, kname, FS)
        .await?
        .call("Unmount", &(none(),))
        .await
}

/// Stop using everything on a drive: unmount what is mounted, lock what was
/// unlocked. Problems are logged, not fatal — a volume that was not mounted
/// refuses to be unmounted, and that is fine.
/// The drive is still the one the owner chose: the service's own size and
/// serial of the device at that kernel name against what they saw.
async fn same_drive(conn: &Connection, drive: &str, expect: &Expect) -> zbus::Result<()> {
    let block = proxy(conn, drive, BLOCK).await?;
    let size: u64 = block.get_property("Size").await?;
    let path: OwnedObjectPath = block.get_property("Drive").await?;
    let serial: String = match Proxy::new(conn, DEST, path, DRIVE).await {
        Ok(d) => d.get_property("Serial").await.unwrap_or_default(),
        Err(_) => String::new(),
    };
    let (serial, want) = (serial.trim(), expect.serial.trim());
    let same_serial = serial.is_empty() || want.is_empty() || serial == want;
    if size != expect.size || !same_serial {
        return Err(zbus::Error::Failure("another drive took its place".into()));
    }
    Ok(())
}

/// The volume is still the one asked about: its uuid, where one was seen.
async fn same_volume(conn: &Connection, volume: &str, uuid: Option<&str>) -> zbus::Result<()> {
    let Some(want) = uuid.filter(|u| !u.is_empty()) else {
        return Ok(());
    };
    let have: String = proxy(conn, volume, BLOCK).await?.get_property("IdUUID").await?;
    if have != want {
        return Err(zbus::Error::Failure("another drive took its place".into()));
    }
    Ok(())
}

async fn release(conn: &Connection, volumes: &[String], locked_part: Option<&str>) {
    for v in volumes {
        if let Err(e) = unmount(conn, v).await {
            debug!("drives: unmount {v}: {e}");
        }
    }
    if let Some(part) = locked_part {
        if let Ok(p) = proxy(conn, part, ENCRYPTED).await {
            if let Err(e) = p.call::<_, _, ()>("Lock", &(none(),)).await {
                debug!("drives: lock {part}: {e}");
            }
        }
    }
}

async fn run(conn: &Connection, cmd: DriveCommand) -> zbus::Result<()> {
    match cmd {
        DriveCommand::Mount(v) => mount(conn, &v).await,
        DriveCommand::Unmount(v) => unmount(conn, &v).await,
        DriveCommand::Eject {
            drive,
            volumes,
            locked_part,
        } => {
            release(conn, &volumes, locked_part.as_deref()).await;
            let block = proxy(conn, &drive, BLOCK).await?;
            let path: OwnedObjectPath = block.get_property("Drive").await?;
            let d = Proxy::new(conn, DEST, path, DRIVE).await?;
            // Cutting the power is the honest "safe to unplug"; a drive that
            // cannot do that is at least ejected.
            match d.call::<_, _, ()>("PowerOff", &(none(),)).await {
                Ok(()) => Ok(()),
                Err(_) => d.call("Eject", &(none(),)).await,
            }
        }
        DriveCommand::Rename {
            volume,
            label,
            mounted,
            uuid,
        } => {
            same_volume(conn, &volume, uuid.as_deref()).await?;
            // Most file systems only take a new name while not in use.
            if mounted {
                unmount(conn, &volume).await?;
            }
            let res = proxy(conn, &volume, FS)
                .await?
                .call::<_, _, ()>("SetLabel", &(label.as_str(), none()))
                .await;
            if mounted {
                let _ = mount(conn, &volume).await;
            }
            res
        }
        DriveCommand::Check { volume, mounted, uuid } => {
            same_volume(conn, &volume, uuid.as_deref()).await?;
            if mounted {
                unmount(conn, &volume).await?;
            }
            let res = proxy(conn, &volume, FS)
                .await?
                .call::<_, _, bool>("Repair", &(none(),))
                .await;
            if mounted {
                let _ = mount(conn, &volume).await;
            }
            match res {
                Ok(true) => Ok(()),
                Ok(false) => Err(zbus::Error::Failure(
                    "it has damage that could not be repaired".into(),
                )),
                Err(e) => Err(e),
            }
        }
        DriveCommand::Unlock { part, password } => {
            let clear: OwnedObjectPath = proxy(conn, &part, ENCRYPTED)
                .await?
                .call("Unlock", &(password.as_str(), none()))
                .await?;
            // Unlocked is not yet usable: mount what was inside.
            Proxy::new(conn, DEST, clear, FS)
                .await?
                .call::<_, _, String>("Mount", &(none(),))
                .await
                .map(|_| ())
        }
        DriveCommand::Format {
            drive,
            volumes,
            locked_part,
            name,
            target,
            password,
            thorough,
            expect,
        } => {
            same_drive(conn, &drive, &expect).await?;
            release(conn, &volumes, locked_part.as_deref()).await;
            // A fresh table with one volume filling the drive: what a stick
            // from a shop looks like, and what every device expects to find.
            let mut wipe: Opts<'_> = HashMap::new();
            if thorough {
                wipe.insert("erase", Value::from("zero"));
            }
            proxy(conn, &drive, BLOCK)
                .await?
                .call::<_, _, ()>("Format", &("dos", wipe))
                .await?;
            let mut opts: Opts<'_> = HashMap::new();
            opts.insert("label", Value::from(name.as_str()));
            opts.insert("update-partition-type", Value::from(true));
            if target == Target::Golem {
                // Or only root could write to it.
                opts.insert("take-ownership", Value::from(true));
            }
            if let Some(pw) = &password {
                opts.insert("encrypt.passphrase", Value::from(pw.as_str()));
                opts.insert("encrypt.type", Value::from("luks2"));
            }
            let part: OwnedObjectPath = proxy(conn, &drive, TABLE)
                .await?
                .call(
                    "CreatePartitionAndFormat",
                    &(0u64, 0u64, "", "", none(), target.fstype(), opts),
                )
                .await?;
            // Leave it ready to use. A locked one is left unlocked by the
            // format, with its file system on the device inside.
            let inside = match Proxy::new(conn, DEST, part.clone(), ENCRYPTED).await {
                Ok(e) if password.is_some() => e
                    .get_property::<OwnedObjectPath>("CleartextDevice")
                    .await
                    .ok()
                    .filter(|p| p.as_str() != "/"),
                _ => None,
            };
            let fs = Proxy::new(conn, DEST, inside.unwrap_or(part), FS).await?;
            if let Err(e) = fs.call::<_, _, String>("Mount", &(none(),)).await {
                debug!("drives: mounting the new volume: {e}");
            }
            Ok(())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_device_name_becomes_its_object_path() {
        assert_eq!(
            object_path("sdb1"),
            "/org/freedesktop/UDisks2/block_devices/sdb1"
        );
        assert_eq!(
            object_path("dm-0"),
            "/org/freedesktop/UDisks2/block_devices/dm_2d0"
        );
        assert_eq!(
            object_path("nvme0n1p2"),
            "/org/freedesktop/UDisks2/block_devices/nvme0n1p2"
        );
    }

    #[test]
    fn each_target_is_one_file_system() {
        assert_eq!(Target::Any.fstype(), "exfat");
        assert_eq!(Target::Golem.fstype(), "ext4");
        assert_eq!(Target::Windows.fstype(), "ntfs");
        assert_eq!(Target::Camera.fstype(), "vfat");
    }
}
