//! The media and Bluetooth layers are TOLD what changes (they asked every second
//! until 2026-10-30): a player appearing, a pause, a player going, an adapter
//! switched off must each reach the layer well before its slow look behind the
//! signals (60 s, 120 s) — on a private bus, with fake services. Needs a
//! `dbus-daemon` on PATH (skipped without one).

use std::collections::HashMap;
use std::io::{BufRead, BufReader};
use std::process::{Child, Command, Stdio};
use std::time::Duration;

use options_engine::{BluetoothCollector, Collector, ContextDelta, ContextState, MediaCollector, PlaybackState, Update};
use tokio::sync::{mpsc, watch};
use zbus::interface;
use zbus::zvariant::OwnedValue;

struct Bus(Child);

impl Drop for Bus {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

fn private_bus() -> Option<(Bus, String)> {
    let config = std::env::temp_dir().join(format!("told-by-the-bus-{}.conf", std::process::id()));
    std::fs::write(
        &config,
        "<busconfig><type>session</type><listen>unix:tmpdir=/tmp</listen><policy context=\"default\">\
         <allow send_destination=\"*\" eavesdrop=\"true\"/><allow eavesdrop=\"true\"/><allow own=\"*\"/>\
         </policy></busconfig>",
    )
    .ok()?;
    let mut child = Command::new("dbus-daemon")
        .arg(format!("--config-file={}", config.display()))
        .args(["--nofork", "--nopidfile", "--print-address"])
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .ok()?;
    let mut address = String::new();
    BufReader::new(child.stdout.take()?).read_line(&mut address).ok()?;
    Some((Bus(child), address.trim().to_owned()))
}

struct Root;

#[interface(name = "org.mpris.MediaPlayer2")]
impl Root {
    #[zbus(property)]
    fn identity(&self) -> String {
        "Fake".into()
    }
}

struct Player {
    status: String,
}

#[interface(name = "org.mpris.MediaPlayer2.Player")]
impl Player {
    #[zbus(property)]
    fn playback_status(&self) -> String {
        self.status.clone()
    }
    #[zbus(property)]
    fn metadata(&self) -> HashMap<String, OwnedValue> {
        HashMap::new()
    }
}

struct Adapter {
    powered: bool,
}

#[interface(name = "org.bluez.Adapter1")]
impl Adapter {
    #[zbus(property)]
    fn powered(&self) -> bool {
        self.powered
    }
}

/// The layer's next update that `pick` wants, within `within`.
async fn update<T>(
    rx: &mut mpsc::Receiver<Update>,
    within: Duration,
    mut pick: impl FnMut(ContextDelta) -> Option<T>,
) -> Option<T> {
    tokio::time::timeout(within, async {
        loop {
            if let Update::Delta(_, delta) = rx.recv().await? {
                if let Some(found) = pick(delta) {
                    return Some(found);
                }
            }
        }
    })
    .await
    .ok()
    .flatten()
}

#[tokio::test(flavor = "multi_thread")]
async fn the_layers_are_told_what_changes() {
    let Some((_bus, address)) = private_bus() else {
        eprintln!("no dbus-daemon on PATH: skipped");
        return;
    };
    // SAFETY: this file's only test; the addresses are set before any thread
    // of ours reads the environment.
    unsafe {
        std::env::set_var("DBUS_SESSION_BUS_ADDRESS", &address);
        std::env::set_var("DBUS_SYSTEM_BUS_ADDRESS", &address);
    }
    let (_ctx_tx, ctx) = watch::channel(ContextState::default());

    // ---- MEDIA
    let (tx, mut rx) = mpsc::channel(32);
    tokio::spawn(Box::new(MediaCollector::new()).run(ctx.clone(), tx));
    let nobody = update(&mut rx, Duration::from_secs(3), |d| match d {
        ContextDelta::MprisPlayers(p) => Some(p),
        _ => None,
    })
    .await
    .expect("the layer's first look");
    assert!(nobody.is_empty());

    // A player appears, paused: nothing is polling now, it has to be told.
    let player = zbus::connection::Builder::session()
        .unwrap()
        .serve_at("/org/mpris/MediaPlayer2", Root)
        .unwrap()
        .serve_at("/org/mpris/MediaPlayer2", Player { status: "Paused".into() })
        .unwrap()
        .name("org.mpris.MediaPlayer2.fake")
        .unwrap()
        .build()
        .await
        .unwrap();
    let state = |want: PlaybackState| {
        move |d: ContextDelta| match d {
            ContextDelta::MprisPlayers(p) if p.iter().any(|x| x.state == want) => Some(()),
            _ => None,
        }
    };
    update(&mut rx, Duration::from_secs(2), state(PlaybackState::Paused))
        .await
        .expect("a player appearing is told");

    // It plays: told again (the signal, with nothing polling yet).
    let iface = player
        .object_server()
        .interface::<_, Player>("/org/mpris/MediaPlayer2")
        .await
        .unwrap();
    iface.get_mut().await.status = "Playing".into();
    iface.get().await.playback_status_changed(iface.signal_emitter()).await.unwrap();
    update(&mut rx, Duration::from_secs(2), state(PlaybackState::Playing))
        .await
        .expect("a play is told");

    // It goes: told, through its name.
    drop(iface);
    drop(player);
    update(&mut rx, Duration::from_secs(2), |d| match d {
        ContextDelta::MprisPlayers(p) if p.is_empty() => Some(()),
        _ => None,
    })
    .await
    .expect("a player going is told");

    // ---- BLUETOOTH
    let bluez = zbus::connection::Builder::session()
        .unwrap()
        .serve_at("/", zbus::fdo::ObjectManager)
        .unwrap()
        .serve_at("/org/bluez/hci0", Adapter { powered: true })
        .unwrap()
        .name("org.bluez")
        .unwrap()
        .build()
        .await
        .unwrap();
    let (tx, mut rx) = mpsc::channel(32);
    tokio::spawn(Box::new(BluetoothCollector::new()).run(ctx, tx));
    let on = update(&mut rx, Duration::from_secs(3), |d| match d {
        ContextDelta::Bluetooth(b) => Some(b),
        _ => None,
    })
    .await
    .expect("the layer's first look");
    assert!(on.present && on.powered);

    let adapter = bluez
        .object_server()
        .interface::<_, Adapter>("/org/bluez/hci0")
        .await
        .unwrap();
    adapter.get_mut().await.powered = false;
    adapter.get().await.powered_changed(adapter.signal_emitter()).await.unwrap();
    let off = update(&mut rx, Duration::from_secs(4), |d| match d {
        ContextDelta::Bluetooth(b) => Some(b),
        _ => None,
    })
    .await
    .expect("an adapter switched off is told");
    assert!(off.present && !off.powered);
}
