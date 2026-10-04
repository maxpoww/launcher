//! Bluetooth for the gear's page: BlueZ over the system bus.
//!
//! One worker thread with its own small runtime, spawned the first time the
//! page opens. Like [`crate::net`], it polls only while the page is on screen
//! and answers every command with a fresh [`BtSnapshot`].
//!
//! Pairing needs an **agent** — the object BlueZ asks "do these numbers
//! match?". Ours lives on the same connection that calls `Pair`, which is the
//! agent BlueZ uses for that pairing, so nothing has to be made the system's
//! default and another agent (blueman's) is left alone.
//!
//! The sound side (high quality or calls, which output is in use) is not
//! BlueZ's at all: it belongs to PipeWire, read with `pw-dump` and set with
//! `wpctl`. Both are best-effort; without them the details view simply has no
//! Sound rows.

use std::collections::HashMap;
use std::process::Command;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use calloop::channel::Sender;
use tokio::sync::{mpsc, oneshot};
use tracing::{debug, warn};
use zbus::zvariant::{ObjectPath, OwnedObjectPath, OwnedValue};
use zbus::{Connection, Proxy};

const POLL: Duration = Duration::from_secs(2);
const RECONNECT: Duration = Duration::from_secs(5);
const DEST: &str = "org.bluez";
const ADAPTER: &str = "org.bluez.Adapter1";
const DEVICE: &str = "org.bluez.Device1";
const BATTERY: &str = "org.bluez.Battery1";
const AGENT_PATH: &str = "/org/golem/bluetooth_agent";
/// How long a "do the numbers match?" question waits for an answer.
const CONFIRM_WAIT: Duration = Duration::from_secs(45);
/// A2DP sink and OBEX object push, the two services the page asks about.
const UUID_AUDIO_SINK: &str = "0000110b";
const UUID_HEADSET: &str = "00001108";
const UUID_HANDSFREE: &str = "0000111e";
const UUID_OBEX_PUSH: &str = "00001105";

type Props = HashMap<String, OwnedValue>;
type Managed = HashMap<OwnedObjectPath, HashMap<String, Props>>;

#[derive(Debug, Clone, Default, PartialEq)]
pub(crate) struct BtDevice {
    pub(crate) path: String,
    pub(crate) address: String,
    pub(crate) name: String,
    /// BlueZ's icon name ("audio-headphones", "input-mouse", "phone", …).
    pub(crate) icon: String,
    pub(crate) paired: bool,
    pub(crate) connected: bool,
    pub(crate) trusted: bool,
    pub(crate) battery: Option<u8>,
    /// Heard in the current search (it has a signal reading).
    pub(crate) nearby: bool,
    pub(crate) audio: bool,
    pub(crate) files: bool,
}

#[derive(Debug, Clone, Default, PartialEq)]
pub(crate) struct BtSnapshot {
    /// The bus answered. False = bluetoothd is not running.
    pub(crate) reachable: bool,
    /// The machine has a Bluetooth radio.
    pub(crate) present: bool,
    pub(crate) powered: bool,
    pub(crate) discoverable: bool,
    pub(crate) discovering: bool,
    /// The name other devices see this computer under.
    pub(crate) alias: String,
    pub(crate) devices: Vec<BtDevice>,
}

/// A device's sound side, from PipeWire.
#[derive(Debug, Clone, Default, PartialEq)]
pub(crate) struct BtAudio {
    pub(crate) address: String,
    card_id: u32,
    /// Profile indices for the two choices the page offers, when the card has
    /// them.
    hq: Option<u32>,
    calls: Option<u32>,
    /// Whether the high-quality profile is the one in use.
    pub(crate) high_quality: bool,
    pub(crate) codec: String,
    sink_id: Option<u32>,
    /// This device is where the computer's sound goes.
    pub(crate) is_output: bool,
}

impl BtAudio {
    pub(crate) fn can_switch(&self) -> bool {
        self.hq.is_some() && self.calls.is_some()
    }
    pub(crate) fn has_sink(&self) -> bool {
        self.sink_id.is_some()
    }
}

#[derive(Debug)]
pub(crate) enum BtCommand {
    Watch(bool),
    Power(bool),
    Scan(bool),
    Discoverable(bool),
    Connect(String),
    Disconnect(String),
    Pair(String),
    Forget(String),
    Trust(String, bool),
    Rename(String, String),
    AdapterAlias(String),
    /// The answer to a [`BtEvent::PairConfirm`].
    PairAnswer(bool),
    Audio(String),
    SoundQuality {
        address: String,
        high: bool,
    },
    UseForSound(String),
}

#[derive(Debug)]
pub(crate) enum BtEvent {
    Snapshot(BtSnapshot),
    /// BlueZ asks whether `code` is what the other device shows.
    PairConfirm {
        path: String,
        code: String,
    },
    /// `code` must be typed on the other device (a keyboard).
    PairShow {
        path: String,
        code: String,
    },
    /// A connect, disconnect or pairing has ended.
    Done {
        path: String,
        ok: bool,
    },
    Audio(BtAudio),
}

pub(crate) struct BtHandle {
    tx: mpsc::UnboundedSender<BtCommand>,
}

impl BtHandle {
    pub(crate) fn send(&self, cmd: BtCommand) {
        if let Err(e) = self.tx.send(cmd) {
            warn!("bt: worker gone, dropping command: {e}");
        }
    }
}

pub(crate) fn spawn(events: Sender<BtEvent>) -> BtHandle {
    let (tx, rx) = mpsc::unbounded_channel();
    let spawned = std::thread::Builder::new()
        .name("bt".into())
        .spawn(move || run_worker(&events, rx));
    if let Err(e) = spawned {
        warn!("bt: cannot spawn the worker: {e}");
    }
    BtHandle { tx }
}

fn run_worker(events: &Sender<BtEvent>, mut commands: mpsc::UnboundedReceiver<BtCommand>) {
    let rt = match tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
    {
        Ok(rt) => rt,
        Err(e) => {
            warn!("bt: cannot build runtime: {e}");
            return;
        }
    };
    rt.block_on(async move {
        let mut watch = false;
        loop {
            if let Err(e) = session(events, &mut commands, &mut watch).await {
                debug!("bt: session ended ({e}); retrying");
                let _ = events.send(BtEvent::Snapshot(BtSnapshot::default()));
            }
            if commands.is_closed() {
                break;
            }
            tokio::time::sleep(RECONNECT).await;
        }
    });
}

/// The pending "do the numbers match?" question, if one is open.
type Pending = Arc<Mutex<Option<oneshot::Sender<bool>>>>;

struct Agent {
    events: Sender<BtEvent>,
    pending: Pending,
}

impl Agent {
    async fn ask(&self, device: &ObjectPath<'_>, code: String) -> zbus::fdo::Result<()> {
        let (tx, rx) = oneshot::channel();
        if let Ok(mut p) = self.pending.lock() {
            *p = Some(tx);
        }
        let _ = self.events.send(BtEvent::PairConfirm {
            path: device.to_string(),
            code,
        });
        match tokio::time::timeout(CONFIRM_WAIT, rx).await {
            Ok(Ok(true)) => Ok(()),
            _ => Err(zbus::fdo::Error::Failed("rejected".into())),
        }
    }
}

#[zbus::interface(name = "org.bluez.Agent1")]
impl Agent {
    fn release(&self) {}

    /// A legacy device with a fixed PIN: the one nearly all of them use.
    fn request_pin_code(&self, _device: ObjectPath<'_>) -> String {
        "0000".to_owned()
    }

    fn display_pin_code(&self, device: ObjectPath<'_>, pincode: String) {
        let _ = self.events.send(BtEvent::PairShow {
            path: device.to_string(),
            code: pincode,
        });
    }

    fn request_passkey(&self, _device: ObjectPath<'_>) -> zbus::fdo::Result<u32> {
        Err(zbus::fdo::Error::Failed("no passkey entry".into()))
    }

    fn display_passkey(&self, device: ObjectPath<'_>, passkey: u32, _entered: u16) {
        let _ = self.events.send(BtEvent::PairShow {
            path: device.to_string(),
            code: format!("{passkey:06}"),
        });
    }

    async fn request_confirmation(
        &self,
        device: ObjectPath<'_>,
        passkey: u32,
    ) -> zbus::fdo::Result<()> {
        self.ask(&device, format!("{passkey:06}")).await
    }

    /// A pairing we started ourselves and that needs no number: yes.
    fn request_authorization(&self, _device: ObjectPath<'_>) {}

    fn authorize_service(&self, _device: ObjectPath<'_>, _uuid: String) {}

    fn cancel(&self) {
        if let Ok(mut p) = self.pending.lock() {
            p.take();
        }
    }
}

async fn session(
    events: &Sender<BtEvent>,
    commands: &mut mpsc::UnboundedReceiver<BtCommand>,
    watch: &mut bool,
) -> zbus::Result<()> {
    let conn = Connection::system().await?;
    let pending: Pending = Arc::default();
    conn.object_server()
        .at(
            AGENT_PATH,
            Agent {
                events: events.clone(),
                pending: pending.clone(),
            },
        )
        .await?;
    let manager = Proxy::new(&conn, DEST, "/org/bluez", "org.bluez.AgentManager1").await?;
    let agent_path = ObjectPath::try_from(AGENT_PATH)?;
    if let Err(e) = manager
        .call::<_, _, ()>("RegisterAgent", &(&agent_path, "KeyboardDisplay"))
        .await
    {
        debug!("bt: RegisterAgent: {e}");
    }
    let mut adapter: Option<String> = None;
    loop {
        let cmd = if *watch {
            match tokio::time::timeout(POLL, commands.recv()).await {
                Ok(Some(c)) => Some(c),
                Ok(None) => return Ok(()),
                Err(_) => None,
            }
        } else {
            match commands.recv().await {
                Some(c) => Some(c),
                None => return Ok(()),
            }
        };
        if let Some(cmd) = cmd {
            debug!("bt: {cmd:?}");
            if adapter.is_none() {
                adapter = adapter_path(&managed(&conn).await?);
            }
            dispatch(cmd, &conn, events, adapter.as_deref(), &pending, watch).await;
        }
        if *watch {
            let objects = managed(&conn).await?;
            adapter = adapter_path(&objects);
            if events.send(BtEvent::Snapshot(summarize(&objects))).is_err() {
                return Ok(());
            }
        }
    }
}

async fn managed(conn: &Connection) -> zbus::Result<Managed> {
    Proxy::new(conn, DEST, "/", "org.freedesktop.DBus.ObjectManager")
        .await?
        .call("GetManagedObjects", &())
        .await
}

fn adapter_path(objects: &Managed) -> Option<String> {
    objects
        .iter()
        .filter(|(_, i)| i.contains_key(ADAPTER))
        .map(|(p, _)| p.to_string())
        .min()
}

async fn dispatch(
    cmd: BtCommand,
    conn: &Connection,
    events: &Sender<BtEvent>,
    adapter: Option<&str>,
    pending: &Pending,
    watch: &mut bool,
) {
    let adapter_set = |name: &'static str, value: bool| async move {
        if let Some(a) = adapter {
            if let Ok(p) = Proxy::new(conn, DEST, a.to_owned(), ADAPTER).await {
                if let Err(e) = p.set_property(name, value).await {
                    warn!("bt: setting {name} failed: {e}");
                }
            }
        }
    };
    match cmd {
        BtCommand::Watch(on) => *watch = on,
        BtCommand::Power(on) => adapter_set("Powered", on).await,
        BtCommand::Discoverable(on) => adapter_set("Discoverable", on).await,
        BtCommand::Scan(on) => {
            if let Some(a) = adapter {
                if let Ok(p) = Proxy::new(conn, DEST, a.to_owned(), ADAPTER).await {
                    let method = if on {
                        "StartDiscovery"
                    } else {
                        "StopDiscovery"
                    };
                    if let Err(e) = p.call::<_, _, ()>(method, &()).await {
                        debug!("bt: {method}: {e}");
                    }
                }
            }
        }
        BtCommand::AdapterAlias(name) => {
            if let Some(a) = adapter {
                if let Ok(p) = Proxy::new(conn, DEST, a.to_owned(), ADAPTER).await {
                    let _ = p.set_property("Alias", name).await;
                }
            }
        }
        BtCommand::Rename(path, name) => {
            if let Ok(p) = Proxy::new(conn, DEST, path, DEVICE).await {
                let _ = p.set_property("Alias", name).await;
            }
        }
        BtCommand::Trust(path, on) => {
            if let Ok(p) = Proxy::new(conn, DEST, path, DEVICE).await {
                let _ = p.set_property("Trusted", on).await;
            }
        }
        BtCommand::Forget(path) => {
            let (Some(a), Ok(dev)) = (adapter, OwnedObjectPath::try_from(path)) else {
                return;
            };
            if let Ok(p) = Proxy::new(conn, DEST, a.to_owned(), ADAPTER).await {
                if let Err(e) = p.call::<_, _, ()>("RemoveDevice", &(dev,)).await {
                    warn!("bt: RemoveDevice failed: {e}");
                }
            }
        }
        // The slow ones run beside the loop: a pairing waits on an answer that
        // arrives through this same command channel.
        BtCommand::Connect(path) => spawn_device_call(conn, events, path, &["Connect"]),
        BtCommand::Disconnect(path) => spawn_device_call(conn, events, path, &["Disconnect"]),
        BtCommand::Pair(path) => spawn_device_call(conn, events, path, &["Pair", "Connect"]),
        BtCommand::PairAnswer(yes) => {
            if let Some(tx) = pending.lock().ok().and_then(|mut p| p.take()) {
                let _ = tx.send(yes);
            }
        }
        BtCommand::Audio(address) => {
            let events = events.clone();
            tokio::task::spawn_blocking(move || {
                let _ = events.send(BtEvent::Audio(read_audio(&address)));
            });
        }
        BtCommand::SoundQuality { address, high } => {
            let events = events.clone();
            tokio::task::spawn_blocking(move || {
                let a = read_audio(&address);
                if let Some(index) = if high { a.hq } else { a.calls } {
                    run_quiet(
                        "wpctl",
                        &["set-profile", &a.card_id.to_string(), &index.to_string()],
                    );
                    std::thread::sleep(Duration::from_millis(600));
                }
                let _ = events.send(BtEvent::Audio(read_audio(&address)));
            });
        }
        BtCommand::UseForSound(address) => {
            let events = events.clone();
            tokio::task::spawn_blocking(move || {
                if let Some(id) = read_audio(&address).sink_id {
                    run_quiet("wpctl", &["set-default", &id.to_string()]);
                }
                let _ = events.send(BtEvent::Audio(read_audio(&address)));
            });
        }
    }
}

/// Run `methods` on a device one after another, off the command loop, then
/// report how it went. A fresh pairing is also trusted, so the device
/// reconnects by itself next time — what "paired" means to a person.
fn spawn_device_call(
    conn: &Connection,
    events: &Sender<BtEvent>,
    path: String,
    methods: &'static [&'static str],
) {
    let conn = conn.clone();
    let events = events.clone();
    tokio::spawn(async move {
        let mut ok = false;
        if let Ok(p) = Proxy::new(&conn, DEST, path.clone(), DEVICE).await {
            ok = true;
            for m in methods {
                match p.call::<_, _, ()>(*m, &()).await {
                    Ok(()) => {
                        if *m == "Pair" {
                            let _ = p.set_property("Trusted", true).await;
                        }
                    }
                    // Pairing something already paired, or connecting a device
                    // with no profile to connect, is not a failure worth a word.
                    Err(e) if *m == "Connect" && methods.len() > 1 => {
                        debug!("bt: Connect after Pair: {e}");
                    }
                    Err(e) => {
                        warn!("bt: {m} on {path} failed: {e}");
                        ok = false;
                        break;
                    }
                }
            }
        }
        let _ = events.send(BtEvent::Done { path, ok });
        if let Ok(objects) = managed(&conn).await {
            let _ = events.send(BtEvent::Snapshot(summarize(&objects)));
        }
    });
}

fn as_bool(props: &Props, key: &str) -> bool {
    props
        .get(key)
        .and_then(|v| v.downcast_ref::<bool>().ok())
        .unwrap_or(false)
}

fn as_string(props: &Props, key: &str) -> String {
    props
        .get(key)
        .and_then(|v| v.downcast_ref::<zbus::zvariant::Str>().ok())
        .map(|s| s.as_str().to_owned())
        .unwrap_or_default()
}

fn has_uuid(props: &Props, short: &str) -> bool {
    props
        .get("UUIDs")
        .and_then(|v| v.try_clone().ok())
        .and_then(|v| Vec::<String>::try_from(v).ok())
        .is_some_and(|l| l.iter().any(|u| u.starts_with(short)))
}

fn summarize(objects: &Managed) -> BtSnapshot {
    let mut out = BtSnapshot {
        reachable: true,
        ..BtSnapshot::default()
    };
    let adapter = adapter_path(objects);
    for (path, interfaces) in objects {
        if let Some(a) = interfaces.get(ADAPTER) {
            if adapter.as_deref() != Some(path.as_str()) {
                continue;
            }
            out.present = true;
            out.powered = as_bool(a, "Powered");
            out.discoverable = as_bool(a, "Discoverable");
            out.discovering = as_bool(a, "Discovering");
            out.alias = as_string(a, "Alias");
        }
        let Some(d) = interfaces.get(DEVICE) else {
            continue;
        };
        let address = as_string(d, "Address");
        let alias = as_string(d, "Alias");
        out.devices.push(BtDevice {
            path: path.to_string(),
            // A device that never said its name shows its address, which is
            // what BlueZ puts in the alias for it anyway.
            name: if alias.is_empty() {
                address.clone()
            } else {
                alias
            },
            address,
            icon: as_string(d, "Icon"),
            paired: as_bool(d, "Paired"),
            connected: as_bool(d, "Connected"),
            trusted: as_bool(d, "Trusted"),
            battery: interfaces
                .get(BATTERY)
                .and_then(|b| b.get("Percentage"))
                .and_then(|v| v.downcast_ref::<u8>().ok()),
            nearby: d.contains_key("RSSI"),
            audio: has_uuid(d, UUID_AUDIO_SINK)
                || has_uuid(d, UUID_HEADSET)
                || has_uuid(d, UUID_HANDSFREE),
            files: has_uuid(d, UUID_OBEX_PUSH),
        });
    }
    sort_devices(&mut out.devices);
    out
}

/// Connected first, then the rest of what is paired, then strangers — each by
/// name, so the list does not shuffle between two polls.
pub(crate) fn sort_devices(devices: &mut [BtDevice]) {
    devices.sort_by(|a, b| {
        let rank = |d: &BtDevice| (d.connected, d.paired);
        rank(b)
            .cmp(&rank(a))
            .then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase()))
            .then_with(|| a.address.cmp(&b.address))
    });
}

fn run_quiet(bin: &str, args: &[&str]) {
    match Command::new(bin).args(args).output() {
        Ok(o) if o.status.success() => {}
        Ok(o) => warn!(
            "bt: {bin} {args:?}: {}",
            String::from_utf8_lossy(&o.stderr).trim()
        ),
        Err(e) => warn!("bt: cannot run {bin}: {e}"),
    }
}

/// The device's sound side, read from `pw-dump`. Empty when PipeWire has no
/// card for it (not connected, not an audio device, or no `pw-dump`).
fn read_audio(address: &str) -> BtAudio {
    let dump = Command::new("pw-dump")
        .output()
        .ok()
        .filter(|o| o.status.success())
        .and_then(|o| serde_json::from_slice::<serde_json::Value>(&o.stdout).ok());
    match dump {
        Some(v) => parse_audio(address, &v),
        None => BtAudio {
            address: address.to_owned(),
            ..BtAudio::default()
        },
    }
}

pub(crate) fn parse_audio(address: &str, dump: &serde_json::Value) -> BtAudio {
    let mut out = BtAudio {
        address: address.to_owned(),
        ..BtAudio::default()
    };
    let card = format!("bluez_card.{}", address.replace(':', "_"));
    let Some(objects) = dump.as_array() else {
        return out;
    };
    let prop = |o: &serde_json::Value, k: &str| o["info"]["props"][k].clone();
    let Some(dev) = objects
        .iter()
        .find(|o| prop(o, "device.name").as_str() == Some(card.as_str()))
    else {
        return out;
    };
    out.card_id = dev["id"].as_u64().unwrap_or(0) as u32;
    let active = dev["info"]["params"]["Profile"][0]["index"].as_u64();
    if let Some(profiles) = dev["info"]["params"]["EnumProfile"].as_array() {
        for p in profiles {
            let (Some(index), Some(name)) = (p["index"].as_u64(), p["name"].as_str()) else {
                continue;
            };
            if p["available"].as_str() == Some("no") {
                continue;
            }
            let is_hq = name.starts_with("a2dp-sink");
            let is_calls = name.starts_with("headset-head-unit");
            // PipeWire lists the best codec of each kind first.
            if is_hq && out.hq.is_none() {
                out.hq = Some(index as u32);
            }
            if is_calls && out.calls.is_none() {
                out.calls = Some(index as u32);
            }
            if Some(index) == active {
                out.high_quality = is_hq;
                out.codec = codec_of(p["description"].as_str().unwrap_or(""));
            }
        }
    }
    let mut sink_name = String::new();
    for o in objects {
        if prop(o, "device.id").as_u64() == Some(u64::from(out.card_id))
            && prop(o, "media.class").as_str() == Some("Audio/Sink")
        {
            out.sink_id = o["id"].as_u64().map(|n| n as u32);
            sink_name = prop(o, "node.name").as_str().unwrap_or("").to_owned();
        }
    }
    out.is_output = !sink_name.is_empty()
        && objects.iter().any(|o| {
            o["metadata"].as_array().is_some_and(|m| {
                m.iter().any(|e| {
                    e["key"].as_str() == Some("default.audio.sink")
                        && e["value"]["name"].as_str() == Some(sink_name.as_str())
                })
            })
        });
    out
}

/// "High Fidelity Playback (A2DP Sink, codec LDAC)" → "LDAC".
fn codec_of(description: &str) -> String {
    description
        .split("codec ")
        .nth(1)
        .map(|s| s.trim_end_matches(')').trim().to_owned())
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_list_reads_connected_then_paired_then_strangers() {
        let d = |name: &str, connected, paired| BtDevice {
            name: name.into(),
            connected,
            paired,
            ..BtDevice::default()
        };
        let mut v = vec![
            d("stranger", false, false),
            d("Zeta", false, true),
            d("mouse", true, true),
            d("alpha", false, true),
        ];
        sort_devices(&mut v);
        let order: Vec<&str> = v.iter().map(|d| d.name.as_str()).collect();
        assert_eq!(order, ["mouse", "alpha", "Zeta", "stranger"]);
    }

    #[test]
    fn the_sound_side_is_read_from_the_pipewire_dump() {
        let dump = serde_json::json!([
            { "id": 70, "type": "PipeWire:Interface:Device",
              "info": { "props": { "device.name": "bluez_card.AC_80_0A_3E_11_D2" },
                "params": {
                  "EnumProfile": [
                    { "index": 0, "name": "off", "description": "Off" },
                    { "index": 1, "name": "a2dp-sink", "description": "High Fidelity Playback (A2DP Sink, codec LDAC)" },
                    { "index": 2, "name": "a2dp-sink-sbc", "description": "High Fidelity Playback (A2DP Sink, codec SBC)" },
                    { "index": 3, "name": "headset-head-unit", "description": "Headset Head Unit (HSP/HFP, codec mSBC)" }
                  ],
                  "Profile": [ { "index": 1, "name": "a2dp-sink" } ] } } },
            { "id": 91, "type": "PipeWire:Interface:Node",
              "info": { "props": { "device.id": 70, "media.class": "Audio/Sink",
                                   "node.name": "bluez_output.AC_80_0A_3E_11_D2.1" } } },
            { "id": 40, "type": "PipeWire:Interface:Metadata",
              "metadata": [ { "key": "default.audio.sink",
                              "value": { "name": "bluez_output.AC_80_0A_3E_11_D2.1" } } ] }
        ]);
        let a = parse_audio("AC:80:0A:3E:11:D2", &dump);
        assert!(a.can_switch() && a.has_sink());
        assert_eq!(
            (a.hq, a.calls),
            (Some(1), Some(3)),
            "the first of each kind"
        );
        assert!(a.high_quality);
        assert_eq!(a.codec, "LDAC");
        assert!(a.is_output);
        // A device PipeWire has no card for has no sound side at all.
        let none = parse_audio("00:00:00:00:00:00", &dump);
        assert!(!none.can_switch() && !none.has_sink() && none.codec.is_empty());
    }
}
