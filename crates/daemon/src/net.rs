//! Wi-Fi for the gear's network page: NetworkManager, through `nmcli`.
//!
//! One worker thread owns every call. It is spawned the first time the page
//! opens and then sleeps: it polls only while the page is on screen
//! ([`NetCommand::Watch`]), so a closed box costs nothing. Every command is
//! followed by a fresh [`NetSnapshot`], which is the only thing the page draws
//! from — the UI never guesses what a command did.
//!
//! `nmcli` rather than NetworkManager's D-Bus API: its terse output is a
//! stable interface, it already handles secrets and profile creation, and it
//! is on every Golem (NetworkManager brings it). Runtime dep, absent → the
//! page says the network service cannot be reached.

use std::collections::HashMap;
use std::process::Command;
use std::sync::mpsc::{self, RecvTimeoutError};
use std::time::Duration;

use calloop::channel::Sender;
use tracing::{debug, warn};

/// How often the list is re-read while the page is on screen.
const POLL: Duration = Duration::from_secs(3);
/// How long a connection attempt may take before `nmcli` gives up.
const CONNECT_WAIT: &str = "30";
/// The one profile the hotspot lives in — named, so it is never listed as a
/// saved network and turning it off is a `con down` of a known name.
pub(crate) const HOTSPOT_PROFILE: &str = "golem-hotspot";

/// One network in range (the strongest access point of each name).
#[derive(Debug, Clone, Default, PartialEq)]
pub(crate) struct Ap {
    pub(crate) ssid: String,
    /// 0–100.
    pub(crate) signal: u8,
    /// `""` for an open network, else what NetworkManager calls it ("WPA2").
    pub(crate) security: String,
    pub(crate) active: bool,
    pub(crate) freq_mhz: u32,
    pub(crate) chan: u32,
    pub(crate) rate: String,
    /// The saved profile's uuid, when this network has one.
    pub(crate) saved: Option<String>,
    pub(crate) autoconnect: bool,
}

impl Ap {
    pub(crate) fn secured(&self) -> bool {
        !self.security.is_empty()
    }
    pub(crate) fn band(&self) -> &'static str {
        match self.freq_mhz {
            0 => "",
            f if f < 3000 => "2.4 GHz",
            f if f < 5900 => "5 GHz",
            _ => "6 GHz",
        }
    }
}

/// Everything the list draws.
#[derive(Debug, Clone, Default, PartialEq)]
pub(crate) struct NetSnapshot {
    /// False when `nmcli` could not be run or NetworkManager did not answer.
    pub(crate) reachable: bool,
    /// The Wi-Fi interface, when the machine has one.
    pub(crate) wifi_dev: Option<String>,
    pub(crate) wifi_on: bool,
    pub(crate) aps: Vec<Ap>,
    /// A connected cable: its link speed as text ("1 Gb/s"), possibly empty.
    pub(crate) wired: Option<String>,
    /// The network wants a sign-in page before it lets anything through.
    pub(crate) portal: bool,
    pub(crate) hotspot_on: bool,
}

/// What the details view shows for one network, read on demand.
#[derive(Debug, Clone, Default, PartialEq)]
pub(crate) struct NetDetail {
    pub(crate) ssid: String,
    pub(crate) ip: String,
    pub(crate) gateway: String,
    pub(crate) dns: String,
    pub(crate) mac: String,
    pub(crate) metered: bool,
    pub(crate) manual: bool,
    pub(crate) private_mac: bool,
    /// Only filled by [`NetCommand::Secret`].
    pub(crate) password: Option<String>,
}

#[derive(Debug)]
pub(crate) enum NetCommand {
    Watch(bool),
    Rescan,
    Radio(bool),
    Connect {
        ssid: String,
        password: Option<String>,
        hidden: bool,
    },
    Disconnect,
    Forget(String),
    Auto(String, bool),
    Metered(String, bool),
    Private(String, bool),
    Manual {
        ssid: String,
        ip: String,
        gateway: String,
        dns: String,
    },
    AutoIp(String),
    Detail(String),
    Secret(String),
    Hotspot {
        on: bool,
        name: String,
        password: String,
        band5: bool,
    },
}

#[derive(Debug)]
pub(crate) enum NetEvent {
    Snapshot(NetSnapshot),
    Detail(NetDetail),
    /// A connection attempt ended badly.
    Failed {
        ssid: String,
        wrong_password: bool,
    },
    /// A rescan or a connection attempt has finished (well or not).
    Idle,
}

pub(crate) struct NetHandle {
    tx: mpsc::Sender<NetCommand>,
}

impl NetHandle {
    pub(crate) fn send(&self, cmd: NetCommand) {
        if let Err(e) = self.tx.send(cmd) {
            warn!("net: worker gone, dropping command: {e}");
        }
    }
}

pub(crate) fn spawn(events: Sender<NetEvent>) -> NetHandle {
    let (tx, rx) = mpsc::channel();
    let spawned = std::thread::Builder::new()
        .name("net".into())
        .spawn(move || run(&events, &rx));
    if let Err(e) = spawned {
        warn!("net: cannot spawn the worker: {e}");
    }
    NetHandle { tx }
}

fn run(events: &Sender<NetEvent>, rx: &mpsc::Receiver<NetCommand>) {
    let mut watch = false;
    // uuid → (ssid, is access-point profile). A profile's SSID costs a call of
    // its own and never changes, so it is read once.
    let mut profiles: HashMap<String, String> = HashMap::new();
    loop {
        let cmd = if watch {
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
        if let Some(cmd) = cmd {
            debug!("net: {cmd:?}");
            match cmd {
                NetCommand::Watch(on) => watch = on,
                other => execute(other, events, &mut profiles),
            }
        }
        if watch
            && events
                .send(NetEvent::Snapshot(snapshot(&mut profiles)))
                .is_err()
        {
            return;
        }
    }
}

fn execute(cmd: NetCommand, events: &Sender<NetEvent>, profiles: &mut HashMap<String, String>) {
    match cmd {
        NetCommand::Watch(_) => {}
        NetCommand::Rescan => {
            let _ = nmcli(&["dev", "wifi", "rescan"]);
            // The scan itself takes a few seconds; the list fills in on the
            // next polls. Tell the page so its button can stop turning.
            std::thread::sleep(Duration::from_millis(2500));
            let _ = events.send(NetEvent::Idle);
        }
        NetCommand::Radio(on) => {
            let _ = nmcli(&["radio", "wifi", if on { "on" } else { "off" }]);
        }
        NetCommand::Connect {
            ssid,
            password,
            hidden,
        } => {
            let uuid = saved_uuid(&ssid, profiles);
            let res = match (&uuid, &password) {
                (Some(u), Some(pw)) => nmcli(&["con", "mod", "uuid", u, "wifi-sec.psk", pw])
                    .and_then(|_| nmcli(&["-w", CONNECT_WAIT, "con", "up", "uuid", u])),
                (Some(u), None) => nmcli(&["-w", CONNECT_WAIT, "con", "up", "uuid", u]),
                (None, pw) => {
                    let mut args = vec!["-w", CONNECT_WAIT, "dev", "wifi", "connect", &ssid];
                    if let Some(pw) = pw {
                        args.extend(["password", pw]);
                    }
                    if hidden {
                        args.extend(["hidden", "yes"]);
                    }
                    nmcli(&args)
                }
            };
            if let Err(msg) = res {
                let wrong_password = is_secret_failure(&msg);
                // A first attempt leaves its half-made profile behind; with a
                // wrong password in it the network would look "saved".
                if uuid.is_none() {
                    let _ = nmcli(&["con", "delete", "id", &ssid]);
                }
                warn!("net: connecting to {ssid:?} failed: {msg}");
                let _ = events.send(NetEvent::Failed {
                    ssid,
                    wrong_password,
                });
            }
            profiles.clear();
            let _ = events.send(NetEvent::Idle);
        }
        NetCommand::Disconnect => {
            if let Some(dev) = wifi_device() {
                let _ = nmcli(&["dev", "disconnect", &dev]);
            }
        }
        NetCommand::Forget(ssid) => {
            if let Some(u) = saved_uuid(&ssid, profiles) {
                let _ = nmcli(&["con", "delete", "uuid", &u]);
                profiles.remove(&u);
            }
        }
        NetCommand::Auto(ssid, on) => modify(&ssid, profiles, &["connection.autoconnect", yn(on)]),
        NetCommand::Metered(ssid, on) => {
            modify(&ssid, profiles, &["connection.metered", yn(on)]);
            send_detail(&ssid, profiles, events, false);
        }
        NetCommand::Private(ssid, on) => {
            let mode = if on { "stable" } else { "permanent" };
            modify(
                &ssid,
                profiles,
                &["802-11-wireless.cloned-mac-address", mode],
            );
            send_detail(&ssid, profiles, events, false);
        }
        NetCommand::Manual {
            ssid,
            ip,
            gateway,
            dns,
        } => {
            let addr = if ip.contains('/') {
                ip
            } else {
                format!("{ip}/24")
            };
            modify(
                &ssid,
                profiles,
                &[
                    "ipv4.method",
                    "manual",
                    "ipv4.addresses",
                    &addr,
                    "ipv4.gateway",
                    &gateway,
                    "ipv4.dns",
                    &dns,
                ],
            );
            reapply(&ssid, profiles);
            send_detail(&ssid, profiles, events, false);
        }
        NetCommand::AutoIp(ssid) => {
            modify(
                &ssid,
                profiles,
                &[
                    "ipv4.method",
                    "auto",
                    "ipv4.addresses",
                    "",
                    "ipv4.gateway",
                    "",
                    "ipv4.dns",
                    "",
                ],
            );
            reapply(&ssid, profiles);
            send_detail(&ssid, profiles, events, false);
        }
        NetCommand::Detail(ssid) => send_detail(&ssid, profiles, events, false),
        NetCommand::Secret(ssid) => send_detail(&ssid, profiles, events, true),
        NetCommand::Hotspot {
            on,
            name,
            password,
            band5,
        } => {
            if on {
                let Some(dev) = wifi_device() else { return };
                let band = if band5 { "a" } else { "bg" };
                if let Err(e) = nmcli(&[
                    "-w",
                    "20",
                    "dev",
                    "wifi",
                    "hotspot",
                    "ifname",
                    &dev,
                    "con-name",
                    HOTSPOT_PROFILE,
                    "ssid",
                    &name,
                    "band",
                    band,
                    "password",
                    &password,
                ]) {
                    warn!("net: hotspot failed: {e}");
                }
            } else {
                let _ = nmcli(&["con", "down", "id", HOTSPOT_PROFILE]);
            }
            let _ = events.send(NetEvent::Idle);
        }
    }
}

fn yn(on: bool) -> &'static str {
    if on {
        "yes"
    } else {
        "no"
    }
}

/// Whether a failed connection was the password's fault.
fn is_secret_failure(msg: &str) -> bool {
    let m = msg.to_lowercase();
    m.contains("secrets were required") || m.contains("password") || m.contains("802-1x")
}

fn modify(ssid: &str, profiles: &mut HashMap<String, String>, settings: &[&str]) {
    let Some(u) = saved_uuid(ssid, profiles) else {
        return;
    };
    let mut args = vec!["con", "mod", "uuid", &u];
    args.extend_from_slice(settings);
    if let Err(e) = nmcli(&args) {
        warn!("net: changing {ssid:?} failed: {e}");
    }
}

/// Bring a changed profile up again, but only if it is the one in use.
fn reapply(ssid: &str, profiles: &mut HashMap<String, String>) {
    let active = wifi_list().iter().any(|a| a.active && a.ssid == ssid);
    if let (true, Some(u)) = (active, saved_uuid(ssid, profiles)) {
        let _ = nmcli(&["-w", CONNECT_WAIT, "con", "up", "uuid", &u]);
    }
}

fn send_detail(
    ssid: &str,
    profiles: &mut HashMap<String, String>,
    events: &Sender<NetEvent>,
    with_secret: bool,
) {
    let mut d = NetDetail {
        ssid: ssid.to_owned(),
        ..NetDetail::default()
    };
    let uuid = saved_uuid(ssid, profiles);
    if let Some(u) = &uuid {
        // `-g` prints one line per field, in the order asked.
        if let Ok(out) = nmcli(&[
            "-g",
            "connection.metered,ipv4.method,ipv4.addresses,ipv4.gateway,ipv4.dns,802-11-wireless.cloned-mac-address",
            "con",
            "show",
            "uuid",
            u,
        ]) {
            let l: Vec<&str> = out.lines().collect();
            let at = |i: usize| l.get(i).copied().unwrap_or("").trim().to_owned();
            d.metered = at(0) == "yes";
            d.manual = at(1) == "manual";
            if d.manual {
                d.ip = at(2);
                d.gateway = at(3);
                d.dns = at(4);
            }
            d.private_mac = matches!(at(5).as_str(), "stable" | "random");
        }
        if with_secret {
            d.password = nmcli(&[
                "-s",
                "-g",
                "802-11-wireless-security.psk",
                "con",
                "show",
                "uuid",
                u,
            ])
            .ok()
            .map(|s| s.trim().to_owned())
            .filter(|s| !s.is_empty());
        }
    }
    // What the link actually has right now wins over what the profile asks for.
    let active = wifi_list().iter().any(|a| a.active && a.ssid == ssid);
    if let (true, Some(dev)) = (active, wifi_device()) {
        if let Ok(out) = nmcli(&[
            "-t",
            "-f",
            "GENERAL.HWADDR,IP4.ADDRESS,IP4.GATEWAY,IP4.DNS",
            "dev",
            "show",
            &dev,
        ]) {
            let mut dns = Vec::new();
            for line in out.lines() {
                let f = split_terse(line);
                let (Some(k), Some(v)) = (f.first(), f.get(1)) else {
                    continue;
                };
                if k.starts_with("GENERAL.HWADDR") {
                    d.mac = v.to_lowercase();
                } else if k.starts_with("IP4.ADDRESS") && (d.ip.is_empty() || !d.manual) {
                    d.ip = v.clone();
                } else if k.starts_with("IP4.GATEWAY") && !v.is_empty() {
                    d.gateway = v.clone();
                } else if k.starts_with("IP4.DNS") {
                    dns.push(v.clone());
                }
            }
            if !dns.is_empty() {
                d.dns = dns.join(", ");
            }
        }
    }
    let _ = events.send(NetEvent::Detail(d));
}

/// Run `nmcli`; `Ok(stdout)` on success, `Err(stderr)` otherwise.
fn nmcli(args: &[&str]) -> Result<String, String> {
    let out = Command::new("nmcli")
        .env("LC_ALL", "C")
        .args(args)
        .output()
        .map_err(|e| format!("nmcli: {e}"))?;
    if out.status.success() {
        Ok(String::from_utf8_lossy(&out.stdout).into_owned())
    } else {
        Err(String::from_utf8_lossy(&out.stderr).trim().to_owned())
    }
}

/// Split one line of `nmcli -t` output: fields are `:`-separated and a literal
/// `:` or `\` inside a value is backslash-escaped.
pub(crate) fn split_terse(line: &str) -> Vec<String> {
    let mut out = vec![String::new()];
    let mut chars = line.chars();
    while let Some(c) = chars.next() {
        match c {
            '\\' => {
                if let (Some(n), Some(last)) = (chars.next(), out.last_mut()) {
                    last.push(n);
                }
            }
            ':' => out.push(String::new()),
            _ => {
                if let Some(last) = out.last_mut() {
                    last.push(c);
                }
            }
        }
    }
    out
}

/// `(device, type, state)` of every interface.
fn devices() -> Result<Vec<(String, String, String)>, String> {
    let out = nmcli(&["-t", "-f", "DEVICE,TYPE,STATE", "dev"])?;
    Ok(out
        .lines()
        .map(split_terse)
        .filter(|f| f.len() >= 3)
        .map(|f| (f[0].clone(), f[1].clone(), f[2].clone()))
        .collect())
}

fn wifi_device() -> Option<String> {
    devices()
        .ok()?
        .into_iter()
        .find(|(_, t, _)| t == "wifi")
        .map(|(d, _, _)| d)
}

/// The networks in range, one per name, without their saved state.
fn wifi_list() -> Vec<Ap> {
    let Ok(out) = nmcli(&[
        "-t",
        "-f",
        "ACTIVE,SSID,SIGNAL,SECURITY,FREQ,CHAN,RATE",
        "dev",
        "wifi",
        "list",
        "--rescan",
        "no",
    ]) else {
        return Vec::new();
    };
    parse_wifi_list(&out)
}

/// One entry per network name: the access point in use if there is one, else
/// the strongest. A house with three repeaters is one network, not three rows.
pub(crate) fn parse_wifi_list(out: &str) -> Vec<Ap> {
    let mut by_name: Vec<Ap> = Vec::new();
    for line in out.lines() {
        let f = split_terse(line);
        if f.len() < 7 || f[1].is_empty() {
            continue; // a hidden network has no name to show
        }
        let ap = Ap {
            active: f[0] == "yes",
            ssid: f[1].clone(),
            signal: f[2].parse().unwrap_or(0),
            security: if f[3] == "--" {
                String::new()
            } else {
                f[3].clone()
            },
            freq_mhz: f[4]
                .split_whitespace()
                .next()
                .and_then(|n| n.parse().ok())
                .unwrap_or(0),
            chan: f[5].parse().unwrap_or(0),
            rate: f[6].replace("Mbit/s", "Mb/s"),
            saved: None,
            autoconnect: false,
        };
        match by_name.iter_mut().find(|a| a.ssid == ap.ssid) {
            Some(old) => {
                if (ap.active && !old.active) || (ap.active == old.active && ap.signal > old.signal)
                {
                    *old = ap;
                }
            }
            None => by_name.push(ap),
        }
    }
    by_name
}

/// `(uuid, autoconnect, active)` of every saved Wi-Fi profile by SSID, with the
/// hotspot's own profile left out.
fn saved_profiles(
    profiles: &mut HashMap<String, String>,
) -> (HashMap<String, (String, bool)>, bool) {
    let mut out = HashMap::new();
    let mut hotspot_on = false;
    let Ok(list) = nmcli(&[
        "-t",
        "-f",
        "NAME,UUID,TYPE,AUTOCONNECT,ACTIVE",
        "con",
        "show",
    ]) else {
        return (out, false);
    };
    for line in list.lines() {
        let f = split_terse(line);
        if f.len() < 5 || f[2] != "802-11-wireless" {
            continue;
        }
        if f[0] == HOTSPOT_PROFILE {
            hotspot_on = f[4] == "yes";
            continue;
        }
        let uuid = f[1].clone();
        let ssid = match profiles.get(&uuid) {
            Some(s) => s.clone(),
            None => {
                let s = nmcli(&["-g", "802-11-wireless.ssid", "con", "show", "uuid", &uuid])
                    .map(|s| s.trim().to_owned())
                    .unwrap_or_default();
                profiles.insert(uuid.clone(), s.clone());
                s
            }
        };
        if !ssid.is_empty() {
            out.insert(ssid, (uuid, f[3] == "yes"));
        }
    }
    (out, hotspot_on)
}

fn saved_uuid(ssid: &str, profiles: &mut HashMap<String, String>) -> Option<String> {
    saved_profiles(profiles).0.remove(ssid).map(|(u, _)| u)
}

fn snapshot(profiles: &mut HashMap<String, String>) -> NetSnapshot {
    let Ok(devs) = devices() else {
        return NetSnapshot::default();
    };
    let mut snap = NetSnapshot {
        reachable: true,
        ..NetSnapshot::default()
    };
    snap.wifi_dev = devs
        .iter()
        .find(|(_, t, _)| t == "wifi")
        .map(|(d, _, _)| d.clone());
    snap.wired = devs
        .iter()
        .find(|(_, t, s)| t == "ethernet" && s.starts_with("connected"))
        .map(|(d, _, _)| wired_speed(d));
    snap.wifi_on = nmcli(&["-t", "-f", "WIFI", "radio"]).is_ok_and(|s| s.trim() == "enabled");
    snap.portal =
        nmcli(&["-t", "-f", "CONNECTIVITY", "general"]).is_ok_and(|s| s.trim() == "portal");
    let (saved, hotspot_on) = saved_profiles(profiles);
    snap.hotspot_on = hotspot_on;
    if snap.wifi_on && !hotspot_on {
        snap.aps = wifi_list();
        for ap in &mut snap.aps {
            if let Some((uuid, auto)) = saved.get(&ap.ssid) {
                ap.saved = Some(uuid.clone());
                ap.autoconnect = *auto;
            }
        }
        sort_aps(&mut snap.aps);
    }
    snap
}

/// The order the list reads in: the one in use, then the ones this machine
/// knows, then everything else, each by signal.
pub(crate) fn sort_aps(aps: &mut [Ap]) {
    aps.sort_by(|a, b| {
        let rank = |x: &Ap| (x.active, x.saved.is_some(), x.signal);
        rank(b).cmp(&rank(a)).then_with(|| a.ssid.cmp(&b.ssid))
    });
}

/// A cable's negotiated speed as text, from sysfs (megabits per second).
fn wired_speed(dev: &str) -> String {
    let mbps = std::fs::read_to_string(format!("/sys/class/net/{dev}/speed"))
        .ok()
        .and_then(|s| s.trim().parse::<i64>().ok())
        .filter(|n| *n > 0);
    match mbps {
        Some(n) if n >= 1000 => format!("{} Gb/s", n as f64 / 1000.0),
        Some(n) => format!("{n} Mb/s"),
        None => String::new(),
    }
}

/// The text a phone camera reads to join a network.
pub(crate) fn wifi_qr_payload(ssid: &str, password: &str, secured: bool) -> String {
    let esc = |s: &str| {
        let mut o = String::with_capacity(s.len());
        for c in s.chars() {
            if matches!(c, '\\' | ';' | ',' | ':' | '"') {
                o.push('\\');
            }
            o.push(c);
        }
        o
    };
    if secured {
        format!("WIFI:T:WPA;S:{};P:{};;", esc(ssid), esc(password))
    } else {
        format!("WIFI:T:nopass;S:{};;", esc(ssid))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn terse_fields_unescape_their_colons() {
        let f = split_terse(r"yes:HOLA:44::5765 MHz:153:1170 Mbit/s:82\:85\:4E\:36\:8D\:31");
        assert_eq!(f.len(), 8);
        assert_eq!(f[1], "HOLA");
        assert_eq!(f[3], "", "an open network has an empty security field");
        assert_eq!(f[7], "82:85:4E:36:8D:31");
        assert_eq!(
            split_terse(r"a\\b:c"),
            vec![r"a\b".to_owned(), "c".to_owned()]
        );
    }

    #[test]
    fn repeaters_of_one_network_are_one_row() {
        let out = "no:HOLA:65::2462 MHz:11:1170 Mbit/s\n\
                   no::50:WPA2 WPA3:5765 MHz:153:1170 Mbit/s\n\
                   yes:HOLA:44::5765 MHz:153:1170 Mbit/s\n\
                   no:Vecinos:70:WPA2:2412 MHz:1:130 Mbit/s\n\
                   no:Vecinos:20:WPA2:2412 MHz:1:130 Mbit/s\n";
        let aps = parse_wifi_list(out);
        assert_eq!(aps.len(), 2, "the nameless one is dropped, the rest merged");
        let hola = &aps[0];
        assert!(
            hola.active,
            "the access point in use wins over a stronger one"
        );
        assert_eq!((hola.signal, hola.chan, hola.band()), (44, 153, "5 GHz"));
        assert!(!hola.secured());
        assert_eq!(aps[1].signal, 70, "otherwise the strongest");
        assert_eq!(aps[1].rate, "130 Mb/s");
    }

    #[test]
    fn the_list_reads_in_use_then_known_then_by_signal() {
        let ap = |ssid: &str, signal, active, saved: bool| Ap {
            ssid: ssid.into(),
            signal,
            active,
            saved: saved.then(|| "u".to_owned()),
            ..Ap::default()
        };
        let mut aps = vec![
            ap("loud", 90, false, false),
            ap("known", 30, false, true),
            ap("mine", 40, true, true),
            ap("quiet", 10, false, false),
        ];
        sort_aps(&mut aps);
        let order: Vec<&str> = aps.iter().map(|a| a.ssid.as_str()).collect();
        assert_eq!(order, ["mine", "known", "loud", "quiet"]);
    }

    #[test]
    fn a_wrong_password_is_told_apart_from_other_failures() {
        assert!(is_secret_failure(
            "Error: Connection activation failed: (7) Secrets were required, but not provided."
        ));
        assert!(!is_secret_failure("Error: No network with SSID 'x' found."));
    }

    #[test]
    fn the_qr_text_escapes_what_the_format_reserves() {
        assert_eq!(
            wifi_qr_payload("Casa;5G", "a:b\\c", true),
            r"WIFI:T:WPA;S:Casa\;5G;P:a\:b\\c;;"
        );
        assert_eq!(wifi_qr_payload("Open", "", false), "WIFI:T:nopass;S:Open;;");
    }
}
