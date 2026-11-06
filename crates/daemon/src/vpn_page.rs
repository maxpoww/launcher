//! VPN (Max, 2026-11-06, the Controls gap audit: every settings app adds a
//! VPN from a file and turns it on; NetworkManager already knew how, the
//! page was missing): Controls → VPN — the VPNs this computer has, each lit
//! while it is up (a click brings it up or takes it down), Add from a file
//! (a WireGuard `.conf` or an OpenVPN `.ovpn`, through the portal's picker,
//! imported by `nmcli`), and Forget, asked once.
//!
//! All of it is NetworkManager's, through `nmcli` as the gear's Wi-Fi page
//! does it: `connection show` for the list and what is active, `connection
//! up` / `down`, `connection import`, `connection delete`.

use std::sync::Mutex;

use tracing::{info, warn};

use crate::controls::{choice_of, group_of, line_of, out_of, run_of, ControlView, Group};
use crate::i18n::{fill, tr};

/// One VPN connection: its name, its NetworkManager uuid, its kind, up.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct Vpn {
    pub name: String,
    pub uuid: String,
    pub kind: String,
    pub up: bool,
}

/// Forget is asked once: the uuid armed by the first click.
static ARMED: Mutex<Option<String>> = Mutex::new(None);
/// What the last add said.
static SAID: Mutex<Option<String>> = Mutex::new(None);
static PICKING: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// The VPNs, from nmcli's terse listing.
pub(crate) fn list() -> Vec<Vpn> {
    out_of("nmcli", &["-t", "-f", "NAME,UUID,TYPE,ACTIVE", "connection", "show"]).map(|o| parse(&o)).unwrap_or_default()
}

/// `name:uuid:type:active` lines; the kinds that are VPNs kept.
pub(crate) fn parse(text: &str) -> Vec<Vpn> {
    text.lines()
        .filter_map(|l| {
            let f: Vec<&str> = l.rsplitn(3, ':').collect();
            // (A name may hold colons: the three last fields are split off
            // from the right, the name is what is left.)
            if f.len() != 3 {
                return None;
            }
            let (active, kind, rest) = (f[0], f[1], f[2]);
            let (name, uuid) = rest.rsplit_once(':')?;
            if !matches!(kind, "vpn" | "wireguard") {
                return None;
            }
            Some(Vpn { name: name.to_owned(), uuid: uuid.to_owned(), kind: kind.to_owned(), up: active == "yes" })
        })
        .collect()
}

/// Controls → VPN.
pub(crate) fn view(view: &mut ControlView) {
    let vpns = list();
    let up: Vec<&Vpn> = vpns.iter().filter(|v| v.up).collect();
    view.subtitle = match up.as_slice() {
        [] if vpns.is_empty() => tr("No VPN yet. Add one from its file.").to_owned(),
        [] => tr("No VPN is on.").to_owned(),
        [one] => fill(tr("Connected through {name}."), &[("name", &one.name)]),
        many => fill(tr("{n} VPNs are on."), &[("n", &many.len())]),
    };
    if !vpns.is_empty() {
        let words = vpns.iter().map(|v| choice_of(v.name.clone(), v.up, v.uuid.clone())).collect();
        view.groups.push(Group { labelled: true, ..group_of(Some("VPNs"), "vpn", words) });
    }
    let picking = PICKING.load(std::sync::atomic::Ordering::Relaxed);
    let mut buttons = vec![choice_of(if picking { tr("Choosing…") } else { tr("Add from a file…") }, false, "add")];
    let armed = ARMED.lock().ok().and_then(|a| a.clone());
    for v in &vpns {
        let word = if armed.as_deref() == Some(v.uuid.as_str()) {
            fill(tr("Click again to forget {name}"), &[("name", &v.name)])
        } else {
            fill(tr("Forget {name}"), &[("name", &v.name)])
        };
        buttons.push(choice_of(word, false, format!("forget:{}", v.uuid)));
    }
    view.groups.push(Group { buttons: true, ..group_of(None, "vpn-do", buttons) });
    if let Some(said) = SAID.lock().ok().and_then(|s| s.clone()) {
        view.groups.push(Group { centred: true, ..line_of(said) });
    }
    view.groups.push(Group { centred: true, ..line_of(tr("A WireGuard .conf or an OpenVPN .ovpn file, as your provider gives it.").to_owned()) });
}

/// A pick on the worker. True when the key was this page's.
pub(crate) fn apply(key: &str, value: &str) -> bool {
    match key {
        "vpn" => {
            if let Some(v) = list().into_iter().find(|v| v.uuid == value) {
                let verb = if v.up { "down" } else { "up" };
                if !run_of("nmcli", &["connection", verb, "uuid", &v.uuid]) {
                    if let Ok(mut s) = SAID.lock() {
                        *s = Some(fill(tr("{name} could not be turned {verb}."), &[("name", &v.name), ("verb", &tr(if v.up { "off" } else { "on" }))]));
                    }
                }
            }
        }
        "vpn-do" if value == "add" => add(),
        "vpn-do" => {
            if let Some(uuid) = value.strip_prefix("forget:") {
                let mut armed = ARMED.lock().unwrap_or_else(|e| e.into_inner());
                if armed.as_deref() == Some(uuid) {
                    *armed = None;
                    if run_of("nmcli", &["connection", "delete", "uuid", uuid]) {
                        info!("vpn: forgot {uuid}");
                    }
                } else {
                    *armed = Some(uuid.to_owned());
                }
            }
        }
        _ => return false,
    }
    true
}

/// The picker on a thread; the file imported by its kind; the page read again.
fn add() {
    if PICKING.swap(true, std::sync::atomic::Ordering::Relaxed) {
        return;
    }
    let spawned = std::thread::Builder::new().name("vpn-add".into()).spawn(|| {
        if let Some(path) = crate::bt_files::pick_file_waiting() {
            let kind = match path.extension().and_then(|e| e.to_str()).map(str::to_lowercase).as_deref() {
                Some("conf") => "wireguard",
                Some("ovpn") => "openvpn",
                _ => "",
            };
            let said = if kind.is_empty() {
                tr("Not a VPN file: it needs to end in .conf (WireGuard) or .ovpn (OpenVPN).").to_owned()
            } else {
                let file = path.to_string_lossy().into_owned();
                match out_of("nmcli", &["connection", "import", "type", kind, "file", &file]) {
                    Some(o) if !o.contains("Error") => {
                        info!("vpn: imported {file}: {}", o.trim());
                        let name = path.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
                        fill(tr("{name} added. Click it to connect."), &[("name", &name)])
                    }
                    other => {
                        warn!("vpn: import of {file} failed: {other:?}");
                        tr("The file could not be imported. Is it as your provider gave it?").to_owned()
                    }
                }
            };
            if let Ok(mut s) = SAID.lock() {
                *s = Some(said);
            }
        }
        PICKING.store(false, std::sync::atomic::Ordering::Relaxed);
        crate::controls::read_again("VPN");
    });
    if spawned.is_err() {
        PICKING.store(false, std::sync::atomic::Ordering::Relaxed);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_listing_keeps_the_vpns_only() {
        let text = "Wi-Fi casa:11111111-aaaa:802-11-wireless:yes\nOffice:22222222-bbbb:vpn:no\nwg0:33333333-cccc:wireguard:yes\nlo:4:loopback:yes\n";
        let v = parse(text);
        assert_eq!(v.len(), 2);
        assert_eq!(v[0], Vpn { name: "Office".into(), uuid: "22222222-bbbb".into(), kind: "vpn".into(), up: false });
        assert!(v[1].up && v[1].kind == "wireguard");
        // A name with a colon in it.
        let v = parse("Home: office:5555:vpn:no\n");
        assert_eq!(v[0].name, "Home: office");
        assert_eq!(v[0].uuid, "5555");
    }
}
