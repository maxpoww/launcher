//! Files over Bluetooth for the gear's page: send one, and take what paired
//! devices send.
//!
//! This is OBEX, which is not BlueZ's main daemon but `obexd`, on the
//! **session** bus (started on demand by the bus). Sending asks the desktop
//! portal for a file, opens an object-push session to the device and watches
//! the transfer; receiving is an agent `obexd` asks before it accepts a push.
//!
//! Progress and results are told through Golem's own notifications rather
//! than drawn in the box: choosing a file means leaving the box (the picker is
//! a window), so the box is not where the eyes are when a transfer ends.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::time::Duration;

use calloop::channel::Sender;
use futures_util::StreamExt;
use tokio::sync::mpsc;
use tracing::{debug, warn};
use zbus::zvariant::{ObjectPath, OwnedObjectPath, OwnedValue, Value};
use zbus::{Connection, Proxy};

const OBEX: &str = "org.bluez.obex";
const OBEX_ROOT: &str = "/org/bluez/obex";
const TRANSFER: &str = "org.bluez.obex.Transfer1";
const AGENT_PATH: &str = "/org/golem/obex_agent";
const PORTAL: &str = "org.freedesktop.portal.Desktop";
const PORTAL_PATH: &str = "/org/freedesktop/portal/desktop";
/// How often a running transfer is looked at.
const TICK: Duration = Duration::from_millis(500);
/// A transfer that has not ended by then is given up on.
const TRANSFER_CAP: Duration = Duration::from_secs(30 * 60);

#[derive(Debug)]
pub(crate) enum FileCommand {
    /// Pick a file and send it to the device at `address` (called `name`).
    Send { address: String, name: String },
    /// Accept files from paired devices, or stop.
    Receive(bool),
}

#[derive(Debug)]
pub(crate) enum FileEvent {
    /// What the device's row says while a transfer to it runs.
    Progress { address: String, text: String },
    /// The transfer to `address` is over (sent, failed or cancelled).
    Done { address: String },
    /// Receiving could not be switched on (another program already answers
    /// for incoming files): the switch goes back.
    ReceiveFailed,
}

pub(crate) struct FileHandle {
    tx: mpsc::UnboundedSender<FileCommand>,
}

impl FileHandle {
    pub(crate) fn send(&self, cmd: FileCommand) {
        if let Err(e) = self.tx.send(cmd) {
            warn!("bt files: worker gone, dropping command: {e}");
        }
    }
}

pub(crate) fn spawn(events: Sender<FileEvent>) -> FileHandle {
    let (tx, rx) = mpsc::unbounded_channel();
    let spawned = std::thread::Builder::new()
        .name("bt-files".into())
        .spawn(move || run_worker(&events, rx));
    if let Err(e) = spawned {
        warn!("bt files: cannot spawn the worker: {e}");
    }
    FileHandle { tx }
}

fn run_worker(events: &Sender<FileEvent>, mut commands: mpsc::UnboundedReceiver<FileCommand>) {
    let rt = match tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
    {
        Ok(rt) => rt,
        Err(e) => {
            warn!("bt files: cannot build runtime: {e}");
            return;
        }
    };
    rt.block_on(async move {
        let conn = match Connection::session().await {
            Ok(c) => c,
            Err(e) => {
                warn!("bt files: no session bus: {e}");
                return;
            }
        };
        let mut token = 0u32;
        let mut receiving = false;
        while let Some(cmd) = commands.recv().await {
            debug!("bt files: {cmd:?}");
            match cmd {
                FileCommand::Send { address, name } => {
                    token += 1;
                    let (conn, events) = (conn.clone(), events.clone());
                    tokio::spawn(async move {
                        send_file(&conn, &events, &address, &name, token).await;
                        let _ = events.send(FileEvent::Done { address });
                    });
                }
                FileCommand::Receive(on) if on != receiving => {
                    match set_receiving(&conn, on).await {
                        Ok(()) => receiving = on,
                        Err(e) => {
                            warn!("bt files: receive {on}: {e}");
                            if on {
                                notify(
                                    &conn,
                                    "Cannot receive files",
                                    "Another program on this computer already takes Bluetooth files",
                                )
                                .await;
                                let _ = events.send(FileEvent::ReceiveFailed);
                            }
                        }
                    }
                }
                FileCommand::Receive(_) => {}
            }
        }
    });
}

/// Ask the desktop for one file. `None` when the picker was cancelled.
async fn pick_file(conn: &Connection, token: u32) -> zbus::Result<Option<PathBuf>> {
    let handle = format!("golem{token}");
    // The portal answers on a request object whose path is known beforehand,
    // so the answer can be listened for BEFORE the question is asked.
    let sender = conn
        .unique_name()
        .map(|n| n.trim_start_matches(':').replace('.', "_"))
        .unwrap_or_default();
    let request_path = format!("{PORTAL_PATH}/request/{sender}/{handle}");
    let request = Proxy::new(conn, PORTAL, request_path, "org.freedesktop.portal.Request").await?;
    let mut answers = request.receive_signal("Response").await?;
    let chooser = Proxy::new(
        conn,
        PORTAL,
        PORTAL_PATH,
        "org.freedesktop.portal.FileChooser",
    )
    .await?;
    let options: HashMap<&str, Value<'_>> =
        HashMap::from([("handle_token", Value::from(handle.as_str()))]);
    chooser
        .call::<_, _, OwnedObjectPath>("OpenFile", &("", "Send a file", options))
        .await?;
    let Some(msg) = answers.next().await else {
        return Ok(None);
    };
    let (code, results): (u32, HashMap<String, OwnedValue>) = msg.body().deserialize()?;
    if code != 0 {
        return Ok(None);
    }
    let uri = results
        .get("uris")
        .and_then(|v| v.try_clone().ok())
        .and_then(|v| Vec::<String>::try_from(v).ok())
        .and_then(|l| l.into_iter().next());
    Ok(uri.as_deref().and_then(uri_to_path))
}

/// `file:///home/max/a%20b.jpg` → `/home/max/a b.jpg`.
pub(crate) fn uri_to_path(uri: &str) -> Option<PathBuf> {
    let rest = uri.strip_prefix("file://")?;
    let bytes = rest.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' && i + 2 < bytes.len() {
            if let Ok(b) = u8::from_str_radix(&rest[i + 1..i + 3], 16) {
                out.push(b);
                i += 3;
                continue;
            }
        }
        out.push(bytes[i]);
        i += 1;
    }
    let path = String::from_utf8(out).ok()?;
    path.starts_with('/').then(|| PathBuf::from(path))
}

async fn send_file(
    conn: &Connection,
    events: &Sender<FileEvent>,
    address: &str,
    name: &str,
    token: u32,
) {
    let path = match pick_file(conn, token).await {
        Ok(Some(p)) => p,
        Ok(None) => return,
        Err(e) => {
            warn!("bt files: file picker: {e}");
            notify(conn, "Could not open the file picker", "").await;
            return;
        }
    };
    let file = path
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    let progress = |text: String| {
        let _ = events.send(FileEvent::Progress {
            address: address.to_owned(),
            text,
        });
    };
    progress(format!("Sending {file}…"));
    match push(conn, address, &path, |pct| {
        progress(format!("Sending {file}… {pct}%"))
    })
    .await
    {
        Ok(()) => notify(conn, &format!("Sent {file}"), &format!("To {name}")).await,
        Err(e) => {
            warn!("bt files: sending {file} to {address}: {e}");
            notify(
                conn,
                &format!("Could not send {file}"),
                &format!("{name} did not take it"),
            )
            .await;
        }
    }
}

async fn push(
    conn: &Connection,
    address: &str,
    path: &Path,
    progress: impl Fn(u64),
) -> zbus::Result<()> {
    let client = Proxy::new(conn, OBEX, OBEX_ROOT, "org.bluez.obex.Client1").await?;
    let args: HashMap<&str, Value<'_>> = HashMap::from([("Target", Value::from("opp"))]);
    let session: OwnedObjectPath = client.call("CreateSession", &(address, args)).await?;
    let result = async {
        let opp = Proxy::new(conn, OBEX, session.clone(), "org.bluez.obex.ObjectPush1").await?;
        let (transfer, _): (OwnedObjectPath, HashMap<String, OwnedValue>) = opp
            .call("SendFile", &(path.to_string_lossy().as_ref(),))
            .await?;
        let t = Proxy::new(conn, OBEX, transfer, TRANSFER).await?;
        wait_transfer(&t, progress).await
    }
    .await;
    let _ = client.call::<_, _, ()>("RemoveSession", &(session,)).await;
    result
}

/// Follow a transfer to its end. `Ok` only when it completed.
async fn wait_transfer(t: &Proxy<'_>, progress: impl Fn(u64)) -> zbus::Result<()> {
    let started = std::time::Instant::now();
    loop {
        // A finished transfer's object goes away; a property that can no
        // longer be read after it was active means it ended.
        let status: String = t.get_property("Status").await?;
        match status.as_str() {
            "complete" => return Ok(()),
            "error" => return Err(zbus::Error::Failure("the transfer failed".into())),
            _ => {}
        }
        if let (Ok(done), Ok(size)) = (
            t.get_property::<u64>("Transferred").await,
            t.get_property::<u64>("Size").await,
        ) {
            if let Some(pct) = (done * 100).checked_div(size) {
                progress(pct);
            }
        }
        if started.elapsed() > TRANSFER_CAP {
            return Err(zbus::Error::Failure("the transfer timed out".into()));
        }
        tokio::time::sleep(TICK).await;
    }
}

/// A line in Golem's notifications.
async fn notify(conn: &Connection, summary: &str, body: &str) {
    let Ok(p) = Proxy::new(
        conn,
        "org.freedesktop.Notifications",
        "/org/freedesktop/Notifications",
        "org.freedesktop.Notifications",
    )
    .await
    else {
        return;
    };
    let hints: HashMap<&str, Value<'_>> = HashMap::new();
    let actions: Vec<&str> = Vec::new();
    if let Err(e) = p
        .call::<_, _, u32>(
            "Notify",
            &(
                "Bluetooth",
                0u32,
                "bluetooth",
                summary,
                body,
                actions,
                hints,
                -1i32,
            ),
        )
        .await
    {
        debug!("bt files: notify: {e}");
    }
}

// --- receiving ---------------------------------------------------------------

struct ObexAgent;

#[zbus::interface(name = "org.bluez.obex.Agent1")]
impl ObexAgent {
    /// `obexd` asks before taking a pushed file. The answer is the name to
    /// store it under, or an error to refuse.
    async fn authorize_push(
        &self,
        transfer: ObjectPath<'_>,
        #[zbus(connection)] conn: &Connection,
    ) -> zbus::fdo::Result<String> {
        let refuse = |why: &str| zbus::fdo::Error::Failed(why.to_owned());
        let t = Proxy::new(conn, OBEX, transfer.to_owned(), TRANSFER)
            .await
            .map_err(|e| refuse(&e.to_string()))?;
        let name: String = t
            .get_property("Name")
            .await
            .map_err(|e| refuse(&e.to_string()))?;
        // Only the bare name: a sender must not choose where the file lands.
        let name = Path::new(&name)
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .filter(|n| !n.is_empty() && n != "." && n != "..")
            .ok_or_else(|| refuse("no file name"))?;
        let session: OwnedObjectPath = t
            .get_property("Session")
            .await
            .map_err(|e| refuse(&e.to_string()))?;
        let from = match Proxy::new(conn, OBEX, session, "org.bluez.obex.Session1").await {
            Ok(s) => s
                .get_property::<String>("Destination")
                .await
                .unwrap_or_default(),
            Err(_) => String::new(),
        };
        if !is_paired(&from).await {
            warn!("bt files: refused {name} from {from}: not a paired device");
            return Err(refuse("not paired"));
        }
        let conn = conn.clone();
        let (path, kept) = (transfer.to_string(), name.clone());
        tokio::spawn(async move {
            received(&conn, &path, &kept).await;
        });
        Ok(name)
    }

    fn cancel(&self) {}

    fn release(&self) {}
}

/// Whether `address` is one of this computer's paired devices.
async fn is_paired(address: &str) -> bool {
    type Managed = HashMap<OwnedObjectPath, HashMap<String, HashMap<String, OwnedValue>>>;
    let Ok(sys) = Connection::system().await else {
        return false;
    };
    let Ok(om) = Proxy::new(&sys, "org.bluez", "/", "org.freedesktop.DBus.ObjectManager").await
    else {
        return false;
    };
    let Ok(objects) = om.call::<_, _, Managed>("GetManagedObjects", &()).await else {
        return false;
    };
    objects.values().any(|i| {
        i.get("org.bluez.Device1").is_some_and(|d| {
            let s = |k: &str| {
                d.get(k)
                    .and_then(|v| v.downcast_ref::<zbus::zvariant::Str>().ok())
                    .map(|s| s.as_str().to_owned())
            };
            let paired = d
                .get("Paired")
                .and_then(|v| v.downcast_ref::<bool>().ok())
                .unwrap_or(false);
            paired && s("Address").is_some_and(|a| a.eq_ignore_ascii_case(address))
        })
    })
}

/// Follow an accepted push; when it lands, move it into Downloads and say so.
async fn received(conn: &Connection, transfer: &str, name: &str) {
    let Ok(t) = Proxy::new(conn, OBEX, transfer.to_owned(), TRANSFER).await else {
        return;
    };
    // Where obexd writes it (its own folder); known once the transfer starts.
    let mut stored: Option<String> = None;
    let started = std::time::Instant::now();
    let ok = loop {
        if stored.is_none() {
            stored = t
                .get_property::<String>("Filename")
                .await
                .ok()
                .filter(|s| !s.is_empty());
        }
        match t.get_property::<String>("Status").await.as_deref() {
            Ok("complete") => break true,
            Ok("error") => break false,
            // The object is gone: obexd removes it when the transfer ends.
            Err(_) => break stored.as_deref().is_some_and(|p| Path::new(p).exists()),
            _ => {}
        }
        if started.elapsed() > TRANSFER_CAP {
            break false;
        }
        tokio::time::sleep(TICK).await;
    };
    if !ok {
        notify(
            conn,
            &format!("Could not receive {name}"),
            "The transfer stopped",
        )
        .await;
        return;
    }
    let Some(src) = stored.map(PathBuf::from) else {
        return;
    };
    let dest = free_name(&downloads_dir(), name);
    let moved = std::fs::create_dir_all(downloads_dir()).and_then(|()| {
        std::fs::rename(&src, &dest).or_else(|_| std::fs::copy(&src, &dest).map(|_| ()))
    });
    match moved {
        Ok(()) => {
            let _ = std::fs::remove_file(&src);
            notify(conn, &format!("Received {name}"), "It is in Downloads").await;
        }
        Err(e) => {
            warn!("bt files: moving {} to Downloads: {e}", src.display());
            notify(
                conn,
                &format!("Received {name}"),
                &format!("It is in {}", src.display()),
            )
            .await;
        }
    }
}

fn downloads_dir() -> PathBuf {
    let home = std::env::var_os("HOME").map_or_else(|| PathBuf::from("/tmp"), PathBuf::from);
    home.join("Downloads")
}

/// `dir/name`, or `dir/name (2).ext` and so on when that is taken.
pub(crate) fn free_name(dir: &Path, name: &str) -> PathBuf {
    let first = dir.join(name);
    if !first.exists() {
        return first;
    }
    let p = Path::new(name);
    let stem = p
        .file_stem()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_default();
    let ext = p
        .extension()
        .map(|e| format!(".{}", e.to_string_lossy()))
        .unwrap_or_default();
    (2..10_000)
        .map(|n| dir.join(format!("{stem} ({n}){ext}")))
        .find(|c| !c.exists())
        .unwrap_or(first)
}

async fn set_receiving(conn: &Connection, on: bool) -> zbus::Result<()> {
    let manager = Proxy::new(conn, OBEX, OBEX_ROOT, "org.bluez.obex.AgentManager1").await?;
    let path = ObjectPath::try_from(AGENT_PATH)?;
    if on {
        conn.object_server().at(AGENT_PATH, ObexAgent).await?;
        manager.call::<_, _, ()>("RegisterAgent", &(&path,)).await
    } else {
        let _ = manager.call::<_, _, ()>("UnregisterAgent", &(&path,)).await;
        conn.object_server()
            .remove::<ObexAgent, _>(AGENT_PATH)
            .await
            .map(|_| ())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_picked_uri_becomes_its_path() {
        assert_eq!(
            uri_to_path("file:///home/max/a%20b%C3%A9.jpg"),
            Some(PathBuf::from("/home/max/a bé.jpg"))
        );
        assert_eq!(uri_to_path("https://example.com/x"), None);
        assert_eq!(uri_to_path("file://relative"), None);
    }

    #[test]
    fn a_received_file_never_overwrites_one_already_there() {
        let dir = std::env::temp_dir().join(format!("golem-btfiles-{}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("temp dir");
        assert_eq!(free_name(&dir, "photo.jpg"), dir.join("photo.jpg"));
        std::fs::write(dir.join("photo.jpg"), b"x").expect("write");
        assert_eq!(free_name(&dir, "photo.jpg"), dir.join("photo (2).jpg"));
        std::fs::write(dir.join("photo (2).jpg"), b"x").expect("write");
        assert_eq!(free_name(&dir, "photo.jpg"), dir.join("photo (3).jpg"));
        let _ = std::fs::remove_dir_all(&dir);
    }
}
