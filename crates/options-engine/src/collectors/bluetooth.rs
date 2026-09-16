//! Layer 4 (part) — Bluetooth, over BlueZ.
//!
//! Asks `org.bluez`'s ObjectManager for everything it manages and folds it into
//! a [`BluetoothState`]: is there an adapter, is it powered, what is connected.
//! One call per poll on the SYSTEM bus (BlueZ lives there, unlike every other
//! bus client in this crate), deduplicated so a delta only lands when something
//! actually changes.
//!
//! # Why its own layer
//!
//! An absent adapter, a stopped `bluetoothd` and "nothing is paired" are three
//! different answers and only the last is knowledge. On its own
//! [`Layer::Bluetooth`] the first two leave the layer dark, and the freshness
//! gate keeps anything built on it off the surface — rather than a confident
//! "0 connected" for a machine whose Bluetooth we simply cannot see.

use std::collections::HashMap;
use std::time::Duration;

use tokio::sync::{mpsc, watch};
use zbus::zvariant::{OwnedObjectPath, OwnedValue};
use zbus::{Connection, Proxy};

use crate::collector::{Collector, CollectorFuture};
use crate::message::{ContextDelta, Update};
use crate::state::{BluetoothState, ContextState, Layer};

/// Slow-changing state: a device connects or drops on human timescales.
const POLL: Duration = Duration::from_secs(3);
const RECONNECT: Duration = Duration::from_secs(10);
const DEST: &str = "org.bluez";
const OBJECT_MANAGER: &str = "org.freedesktop.DBus.ObjectManager";
const ADAPTER_IFACE: &str = "org.bluez.Adapter1";
const DEVICE_IFACE: &str = "org.bluez.Device1";

/// The managed-objects reply: path → interface → properties.
type Managed = HashMap<OwnedObjectPath, HashMap<String, HashMap<String, OwnedValue>>>;

#[derive(Default)]
pub struct BluetoothCollector;

impl BluetoothCollector {
    pub fn new() -> Self {
        Self
    }
}

impl Collector for BluetoothCollector {
    fn name(&self) -> &'static str {
        "bluetooth"
    }
    fn layer(&self) -> Layer {
        Layer::Bluetooth
    }
    fn run(
        self: Box<Self>,
        _ctx: watch::Receiver<ContextState>,
        tx: mpsc::Sender<Update>,
    ) -> CollectorFuture {
        Box::pin(async move {
            loop {
                let proxy = match connect().await {
                    Ok(p) => p,
                    Err(e) => {
                        tracing::debug!("bluetooth: connect failed: {e}");
                        tokio::time::sleep(RECONNECT).await;
                        continue;
                    }
                };
                let mut last: Option<BluetoothState> = None;
                loop {
                    match managed_objects(&proxy).await {
                        Ok(objects) => {
                            let state = summarize(&objects);
                            if last.as_ref() != Some(&state) {
                                last = Some(state.clone());
                                if tx
                                    .send(Update::Delta(
                                        Layer::Bluetooth,
                                        ContextDelta::Bluetooth(state),
                                    ))
                                    .await
                                    .is_err()
                                {
                                    return Ok(()); // aggregator gone
                                }
                            }
                        }
                        Err(e) => {
                            tracing::debug!("bluetooth: GetManagedObjects failed: {e}");
                            break;
                        }
                    }
                    tokio::time::sleep(POLL).await;
                }
                // BlueZ went away: the layer goes dark rather than leaving its
                // last answer standing as if it were still true.
                let _ = tx.send(Update::Health(Layer::Bluetooth, false)).await;
                tokio::time::sleep(RECONNECT).await;
            }
        })
    }
}

/// Connect to the SYSTEM bus and build BlueZ's ObjectManager proxy.
async fn connect() -> zbus::Result<Proxy<'static>> {
    let conn = Connection::system().await?;
    Proxy::new(&conn, DEST, "/", OBJECT_MANAGER).await
}

async fn managed_objects(proxy: &Proxy<'_>) -> zbus::Result<Managed> {
    proxy.call("GetManagedObjects", &()).await
}

/// Fold BlueZ's object tree into the state. Pure over the parsed shape, so the
/// interesting part — what counts as connected — is unit-tested.
fn summarize(objects: &Managed) -> BluetoothState {
    let mut out = BluetoothState::default();
    for interfaces in objects.values() {
        if let Some(adapter) = interfaces.get(ADAPTER_IFACE) {
            out.present = true;
            // ANY powered adapter powers the machine's Bluetooth: with two
            // radios, one switched off does not mean Bluetooth is off.
            out.powered |= as_bool(adapter, "Powered").unwrap_or(false);
        }
        if let Some(device) = interfaces.get(DEVICE_IFACE) {
            if as_bool(device, "Connected").unwrap_or(false) {
                out.connected += 1;
                if out.device.is_empty() {
                    // Alias is what BlueZ shows the user (a renamed device keeps
                    // its Name); fall back to the raw Name, then to nothing.
                    out.device = as_string(device, "Alias")
                        .or_else(|| as_string(device, "Name"))
                        .unwrap_or_default();
                }
            }
        }
    }
    out
}

fn as_bool(props: &HashMap<String, OwnedValue>, key: &str) -> Option<bool> {
    props.get(key)?.downcast_ref::<bool>().ok()
}

fn as_string(props: &HashMap<String, OwnedValue>, key: &str) -> Option<String> {
    let v = props.get(key)?;
    v.downcast_ref::<zbus::zvariant::Str>()
        .ok()
        .map(|s| s.as_str().to_owned())
        .filter(|s| !s.is_empty())
}

#[cfg(test)]
mod tests {
    use super::*;
    use zbus::zvariant::Value;

    fn props(pairs: &[(&str, Value<'static>)]) -> HashMap<String, OwnedValue> {
        pairs
            .iter()
            .map(|(k, v)| {
                (
                    (*k).to_owned(),
                    OwnedValue::try_from(v.clone()).expect("value"),
                )
            })
            .collect()
    }

    fn object(
        path: &str,
        ifaces: Vec<(&str, HashMap<String, OwnedValue>)>,
    ) -> (OwnedObjectPath, HashMap<String, HashMap<String, OwnedValue>>) {
        (
            OwnedObjectPath::try_from(path).expect("path"),
            ifaces.into_iter().map(|(k, v)| (k.to_owned(), v)).collect(),
        )
    }

    #[test]
    fn no_adapter_is_not_the_same_as_nothing_connected() {
        let empty: Managed = Managed::new();
        let s = summarize(&empty);
        assert!(!s.present, "a machine with no radio must say so");
        assert!(!s.powered);
        assert_eq!(s.connected, 0);
    }

    #[test]
    fn a_powered_adapter_with_a_connected_device() {
        let objects: Managed = [
            object(
                "/org/bluez/hci0",
                vec![(ADAPTER_IFACE, props(&[("Powered", Value::Bool(true))]))],
            ),
            object(
                "/org/bluez/hci0/dev_AA",
                vec![(
                    DEVICE_IFACE,
                    props(&[
                        ("Connected", Value::Bool(true)),
                        ("Alias", Value::new("Sony WH-1000XM4")),
                    ]),
                )],
            ),
            // A paired but disconnected device must not be counted.
            object(
                "/org/bluez/hci0/dev_BB",
                vec![(
                    DEVICE_IFACE,
                    props(&[
                        ("Connected", Value::Bool(false)),
                        ("Alias", Value::new("Keyboard")),
                    ]),
                )],
            ),
        ]
        .into_iter()
        .collect();

        let s = summarize(&objects);
        assert!(s.present && s.powered);
        assert_eq!(s.connected, 1);
        assert_eq!(s.device, "Sony WH-1000XM4", "the connected one, by alias");
    }

    /// Two radios, one switched off: Bluetooth is still on.
    #[test]
    fn any_powered_adapter_counts_as_powered() {
        let objects: Managed = [
            object(
                "/org/bluez/hci0",
                vec![(ADAPTER_IFACE, props(&[("Powered", Value::Bool(false))]))],
            ),
            object(
                "/org/bluez/hci1",
                vec![(ADAPTER_IFACE, props(&[("Powered", Value::Bool(true))]))],
            ),
        ]
        .into_iter()
        .collect();
        assert!(summarize(&objects).powered);
    }
}
