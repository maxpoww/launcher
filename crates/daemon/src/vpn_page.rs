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
//!
//! 2026-11-09 (a review: imports failed in common cases and always blamed
//! the file): a file's kind is told by what is IN it (an OpenVPN
//! `client.conf` went to WireGuard by its ending); a WireGuard file goes in
//! through a copy named as NetworkManager wants (an interface's name: "wg0
//! (1).conf" and a provider's long names were refused) and gets its own name
//! back; nmcli's complaints are kept and said in plain words; on and off run
//! on a thread of their own for as long as a VPN takes to come up.

use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Mutex;
use std::time::Duration;

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
/// What the last action said (a line a sentence); cleared by the next one.
static SAID: Mutex<Option<String>> = Mutex::new(None);
static PICKING: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
/// The on or off on its way. Only the newest one speaks, so an up taken
/// back by a down does not then say it failed.
static TURNING: Mutex<Option<Turning>> = Mutex::new(None);
static TURNS: AtomicU64 = AtomicU64::new(0);

#[derive(Clone)]
struct Turning {
    /// Its number in `TURNS`.
    n: u64,
    uuid: String,
    /// What the VPN is being turned to: its pill is lit so meanwhile (the
    /// page is read before the thread has done anything).
    on: bool,
    /// "Connecting to Office…".
    words: String,
}

/// How long on or off may take: `connection up` waits for the whole
/// activation, and a plugin VPN takes seconds. nmcli is told a little less
/// (`TURN_WAIT`, seconds) so that it ends with its own words first.
const TURN_PATIENCE: Duration = Duration::from_secs(60);
const TURN_WAIT: &str = "50";
const IMPORT_PATIENCE: Duration = Duration::from_secs(30);
/// No VPN file is near this size; anything bigger is not one.
const BIGGEST: u64 = 1 << 20;

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
    let turning = TURNING.lock().ok().and_then(|t| t.clone());
    if !vpns.is_empty() {
        let lit = |v: &Vpn| turning.as_ref().filter(|t| t.uuid == v.uuid).map_or(v.up, |t| t.on);
        let words = vpns.iter().map(|v| choice_of(v.name.clone(), lit(v), v.uuid.clone())).collect();
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
    if let Some(t) = turning {
        view.groups.push(Group { centred: true, ..line_of(t.words) });
    }
    if let Some(said) = SAID.lock().ok().and_then(|s| s.clone()) {
        // (A line a sentence: the page's lines do not wrap.)
        for line in said.lines() {
            view.groups.push(Group { centred: true, ..line_of(line.to_owned()) });
        }
    }
    view.groups.push(Group { centred: true, ..line_of(tr("A WireGuard .conf or an OpenVPN .ovpn file, as your provider gives it.").to_owned()) });
}

/// A pick on the worker. True when the key was this page's.
pub(crate) fn apply(key: &str, value: &str) -> bool {
    match key {
        "vpn" => {
            if let Some(v) = list().into_iter().find(|v| v.uuid == value) {
                turn(v);
            }
        }
        "vpn-do" if value == "add" => add(),
        "vpn-do" => {
            if let Some(uuid) = value.strip_prefix("forget:") {
                say(None);
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

fn say(words: Option<String>) {
    if let Ok(mut s) = SAID.lock() {
        *s = words;
    }
}

/// On or off on a thread of its own (2026-11-09: the controls' runner ended
/// `connection up` at 4 s, so a VPN that took 6 s said "could not be turned
/// on" while NetworkManager went on and connected it); "Connecting to …"
/// meanwhile, the page read again at its end.
fn turn(v: Vpn) {
    let mine = TURNS.fetch_add(1, Ordering::Relaxed) + 1;
    let doing = if v.up { tr("Disconnecting from {name}…") } else { tr("Connecting to {name}…") };
    say(None);
    if let Ok(mut t) = TURNING.lock() {
        *t = Some(Turning { n: mine, uuid: v.uuid.clone(), on: !v.up, words: fill(doing, &[("name", &v.name)]) });
    }
    let spawned = std::thread::Builder::new().name("vpn-turn".into()).spawn(move || {
        let verb = if v.up { "down" } else { "up" };
        let answer = nmcli(&["-w", TURN_WAIT, "connection", verb, "uuid", &v.uuid], TURN_PATIENCE);
        let newest = TURNS.load(Ordering::Relaxed) == mine;
        if let Ok(mut t) = TURNING.lock() {
            if t.as_ref().is_some_and(|t| t.n == mine) {
                *t = None;
            }
        }
        if answer.ok {
            info!("vpn: {} {verb}", v.name);
        } else {
            warn!("vpn: {} {verb} failed{}: {}", v.name, if answer.late { " (no answer, ended)" } else { "" }, answer.err.trim());
            if newest {
                say(Some(turn_trouble(&v.name, v.up, &answer)));
            }
        }
        crate::controls::read_again("VPN");
    });
    if let Err(e) = spawned {
        warn!("vpn: no thread for an on or off ({e})");
        if let Ok(mut t) = TURNING.lock() {
            *t = None;
        }
    }
}

/// What went wrong with an on or off, in plain words.
fn turn_trouble(name: &str, was_up: bool, answer: &Answer) -> String {
    let err = answer.err.to_lowercase();
    if err.contains("networkmanager is not running") {
        return tr("The network service is not running.").to_owned();
    }
    // nmcli has no terminal to ask on and Standard no VPN password prompt yet:
    // say so, and the way that works today. (Two lines: they do not wrap.)
    if !was_up && needs_secret(&err) {
        return [
            fill(tr("{name} needs a password, and Standard cannot ask for one yet."), &[("name", &name)]),
            fill(tr("Until it can, connect it from a terminal: nmcli --ask connection up \"{name}\""), &[("name", &name)]),
        ]
        .join("\n");
    }
    // (NetworkManager goes on with an activation nmcli stopped waiting for.)
    if !was_up && (answer.late || err.contains("timeout expired")) {
        return fill(tr("{name} did not connect within a minute. It may still connect."), &[("name", &name)]);
    }
    // (A whole sentence each way: "on"/"off" slotted into one
    // reads wrong in every other language.)
    let said = if was_up { tr("{name} could not be turned off.") } else { tr("{name} could not be turned on.") };
    fill(said, &[("name", &name)])
}

/// nmcli's words for a connection that wanted a secret nobody gave: its
/// own warning ("cannot ask without '--ask'") and NetworkManager's reasons.
fn needs_secret(err: &str) -> bool {
    ["secrets were required", "secrets are required", "cannot ask without", "password"].iter().any(|w| err.contains(w))
}

/// The picker on a thread; the file imported by its kind; the page read again.
fn add() {
    if PICKING.swap(true, std::sync::atomic::Ordering::Relaxed) {
        return;
    }
    say(None);
    let spawned = std::thread::Builder::new().name("vpn-add".into()).spawn(|| {
        if let Some(path) = crate::bt_files::pick_file_waiting() {
            say(Some(import(&path)));
        }
        PICKING.store(false, std::sync::atomic::Ordering::Relaxed);
        crate::controls::read_again("VPN");
    });
    if spawned.is_err() {
        PICKING.store(false, std::sync::atomic::Ordering::Relaxed);
    }
}

/// A VPN file's kind, told by what is in it.
#[derive(Debug, Clone, Copy, PartialEq)]
enum FileKind {
    WireGuard,
    OpenVpn,
}

/// WireGuard's `[Interface]` / `PrivateKey`, else OpenVPN's `client`,
/// `remote …`, `dev tun|tap` (2026-11-09: by the ending, an OpenVPN
/// `client.conf` was imported as WireGuard's).
fn kind_of(text: &str) -> Option<FileKind> {
    let mut openvpn = false;
    for line in text.trim_start_matches('\u{feff}').lines() {
        let l = line.trim().to_ascii_lowercase();
        let key = l.split('=').next().unwrap_or("").trim();
        if l == "[interface]" || (key == "privatekey" && l.contains('=')) {
            return Some(FileKind::WireGuard);
        }
        let mut words = l.split_whitespace();
        let (first, second) = (words.next().unwrap_or(""), words.next().unwrap_or(""));
        openvpn |= l == "client" || (first == "remote" && !second.is_empty()) || (first == "dev" && (second.starts_with("tun") || second.starts_with("tap")));
    }
    openvpn.then_some(FileKind::OpenVpn)
}

/// The picked file into NetworkManager; what the page then says.
fn import(path: &Path) -> String {
    let file = path.to_string_lossy().into_owned();
    let file_name = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    let stem = path.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
    let mut bytes = Vec::new();
    let read = std::fs::File::open(path).and_then(|f| f.take(BIGGEST + 1).read_to_end(&mut bytes));
    if let Err(e) = read {
        warn!("vpn: {file} could not be read: {e}");
        return tr("The file could not be read.").to_owned();
    }
    let kind = if bytes.len() as u64 > BIGGEST { None } else { kind_of(&String::from_utf8_lossy(&bytes)) };
    let answer = match kind {
        None => return tr("Not a VPN file: there is no WireGuard or OpenVPN setup in it.").to_owned(),
        // In place: its certificates and keys are often files beside it,
        // named from it.
        Some(FileKind::OpenVpn) => nmcli(&["connection", "import", "type", "openvpn", "file", &file], IMPORT_PATIENCE),
        Some(FileKind::WireGuard) => return import_wireguard(&bytes, &stem, &file_name),
    };
    if !answer.ok {
        warn!("vpn: import of {file} failed: {}", answer.err.trim());
        return import_trouble(&answer, &file, &file_name);
    }
    info!("vpn: imported {file}: {}", answer.out.trim());
    fill(tr("{name} added. Click it to connect."), &[("name", &stem)])
}

/// NetworkManager names a WireGuard connection after its file and refuses a
/// file whose name is not an interface's (wg-quick's rule: 15 at most of
/// letters, digits, `_=+.-`). So the file goes in as a copy so named, in a
/// folder only we can read (it holds the private key), and the connection
/// is then given the file's own name back.
fn import_wireguard(bytes: &[u8], stem: &str, file_name: &str) -> String {
    let iface = iface_name(stem);
    let answer = match private_copy(bytes, &iface) {
        Ok((dir, copy)) => {
            let copy = copy.to_string_lossy().into_owned();
            let answer = nmcli(&["connection", "import", "type", "wireguard", "file", &copy], IMPORT_PATIENCE);
            let _ = std::fs::remove_dir_all(&dir);
            if !answer.ok {
                warn!("vpn: import of {file_name} (as {iface}) failed: {}", answer.err.trim());
                return import_trouble(&answer, &copy, file_name);
            }
            answer
        }
        Err(e) => {
            warn!("vpn: no private copy of {file_name}: {e}");
            return fill(tr("The file could not be imported: {why}"), &[("why", &e)]);
        }
    };
    info!("vpn: imported {file_name} as {iface}: {}", answer.out.trim());
    let mut name = iface.clone();
    if stem != iface && !stem.is_empty() {
        match uuid_in(&answer.out) {
            Some(uuid) => {
                let renamed = nmcli(&["connection", "modify", "uuid", uuid, "connection.id", stem], IMPORT_PATIENCE);
                if renamed.ok {
                    name = stem.to_owned();
                } else {
                    warn!("vpn: {iface} kept its name: {}", renamed.err.trim());
                }
            }
            None => warn!("vpn: no uuid in nmcli's answer; {iface} keeps its name"),
        }
    }
    fill(tr("{name} added. Click it to connect."), &[("name", &name)])
}

/// An interface's name made from a file's: the letters it may hold kept,
/// every run of others one `-`, 15 at most (the kernel's IFNAMSIZ).
fn iface_name(stem: &str) -> String {
    let mut name = String::new();
    for c in stem.chars() {
        let c = if c.is_ascii_alphanumeric() || "_=+.-".contains(c) { c } else { '-' };
        if !(c == '-' && (name.is_empty() || name.ends_with('-'))) {
            name.push(c);
        }
    }
    name.truncate(15);
    let name = name.trim_end_matches('-');
    if name.is_empty() || name == "." || name == ".." {
        "wg0".to_owned()
    } else {
        name.to_owned()
    }
}

/// `<iface>.conf` with these bytes in a new folder of the runtime dir (the
/// temp dir without one) that only we can open; made new, never one that
/// was there. The folder and the copy.
fn private_copy(bytes: &[u8], iface: &str) -> std::io::Result<(PathBuf, PathBuf)> {
    use std::os::unix::fs::{DirBuilderExt, OpenOptionsExt};
    let base = std::env::var_os("XDG_RUNTIME_DIR").map(PathBuf::from).unwrap_or_else(std::env::temp_dir);
    let n = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.subsec_nanos()).unwrap_or(0);
    let dir = base.join(format!("golem-vpn-{}-{n}", std::process::id()));
    std::fs::DirBuilder::new().mode(0o700).create(&dir)?;
    let copy = dir.join(format!("{iface}.conf"));
    let written = std::fs::OpenOptions::new().write(true).create_new(true).mode(0o600).open(&copy).and_then(|mut f| f.write_all(bytes));
    if let Err(e) = written {
        let _ = std::fs::remove_dir_all(&dir);
        return Err(e);
    }
    Ok((dir, copy))
}

/// The uuid in "Connection 'wg0' (2b0e…) successfully added." (the last
/// brackets: a name may hold some).
fn uuid_in(out: &str) -> Option<&str> {
    let open = out.rfind('(')?;
    let close = open + out[open..].find(')')?;
    let uuid = &out[open + 1..close];
    (uuid.len() == 36 && uuid.chars().all(|c| c.is_ascii_hexdigit() || c == '-')).then_some(uuid)
}

/// What nmcli's complaint about an import means, in plain words; else its
/// own reason (2026-11-09: every failure said "Is it as your provider gave
/// it?", the log `None`).
fn import_trouble(answer: &Answer, given: &str, file_name: &str) -> String {
    let err = answer.err.to_lowercase();
    if answer.late {
        return tr("NetworkManager did not answer in time.").to_owned();
    }
    if err.contains("networkmanager is not running") {
        return tr("The network service is not running.").to_owned();
    }
    // A NetworkManager has the VPN plugins its system lists (on NixOS
    // `networking.networkmanager.plugins`, empty by default): with none,
    // every .ovpn ends here.
    if err.contains("vpn plugin") || err.contains("client plugin") {
        return tr("This computer cannot use OpenVPN yet: NetworkManager's OpenVPN plugin is not installed.").to_owned();
    }
    let why = reason(&answer.err, given, file_name);
    if why.is_empty() {
        return tr("The file could not be imported. Is it as your provider gave it?").to_owned();
    }
    fill(tr("The file could not be imported: {why}"), &[("why", &why)])
}

/// nmcli's reason: its last "Error: …" line without "failed to import
/// '<path>': ", the copy's path told as the picked file's name.
fn reason(err: &str, given: &str, file_name: &str) -> String {
    let lines = || err.lines().map(str::trim).filter(|l| !l.is_empty());
    let line = lines().rev().find(|l| l.starts_with("Error:")).or_else(|| lines().next_back()).unwrap_or("");
    let line = line.strip_prefix("Error:").unwrap_or(line).trim();
    let prefix = format!("failed to import '{given}': ");
    let line = line.strip_prefix(prefix.as_str()).unwrap_or(line);
    line.trim_end_matches('.').replace(given, file_name)
}

/// What nmcli answered: whether it did it, what it printed, what it
/// complained of, and whether it was ended for taking too long.
struct Answer {
    ok: bool,
    out: String,
    err: String,
    late: bool,
}

/// nmcli with its complaints KEPT (the controls' runner sends every page's
/// to /dev/null) and a patience of this page's own. Its messages in English
/// to be read here, in UTF-8 so names keep their letters.
fn nmcli(args: &[&str], patience: Duration) -> Answer {
    let mut cmd = Command::new("nmcli");
    cmd.args(args).env("LC_ALL", "C.UTF-8").env_remove("LANGUAGE");
    cmd.stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::piped());
    let mut child = match cmd.spawn() {
        Ok(c) => c,
        Err(e) => {
            warn!("vpn: nmcli could not start: {e}");
            // (Without nmcli there is no NetworkManager to ask: said as its words.)
            return Answer { ok: false, out: String::new(), err: "Error: NetworkManager is not running.".into(), late: false };
        }
    };
    // Each pipe read on its own thread: a full one would stop nmcli.
    fn drain<R: Read + Send + 'static>(pipe: Option<R>) -> Option<std::thread::JoinHandle<String>> {
        pipe.map(|mut p| {
            std::thread::spawn(move || {
                let mut bytes = Vec::new();
                let _ = p.read_to_end(&mut bytes);
                String::from_utf8_lossy(&bytes).into_owned()
            })
        })
    }
    let (out, err) = (drain(child.stdout.take()), drain(child.stderr.take()));
    let start = std::time::Instant::now();
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break Some(status),
            Ok(None) if start.elapsed() < patience => std::thread::sleep(Duration::from_millis(20)),
            _ => {
                let _ = child.kill();
                let _ = child.wait();
                break None;
            }
        }
    };
    let text = |h: Option<std::thread::JoinHandle<String>>| h.and_then(|h| h.join().ok()).unwrap_or_default();
    Answer { ok: status.is_some_and(|s| s.success()), out: text(out), err: text(err), late: status.is_none() }
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

    #[test]
    fn a_file_is_told_by_what_is_in_it() {
        let wg = "\u{feff}[Interface]\r\nPrivateKey = abc=\r\nAddress = 10.0.0.2/32\r\n\r\n[Peer]\r\nEndpoint = 1.2.3.4:51820\r\n";
        assert_eq!(kind_of(wg), Some(FileKind::WireGuard));
        assert_eq!(kind_of("privatekey=abc\n"), Some(FileKind::WireGuard));
        // An OpenVPN client.conf: WireGuard's ending, OpenVPN's insides.
        assert_eq!(kind_of("# a provider\nclient\ndev tun\nproto udp\nremote vpn.example.com 1194\n"), Some(FileKind::OpenVpn));
        assert_eq!(kind_of("remote\t10.0.0.1 443 tcp\n"), Some(FileKind::OpenVpn));
        assert_eq!(kind_of("dev tap0\n"), Some(FileKind::OpenVpn));
        // Commented out, or something else altogether.
        assert_eq!(kind_of("# remote x\n; client\n"), None);
        assert_eq!(kind_of("[Desktop Entry]\nName=x\n"), None);
        assert_eq!(kind_of("clientele\nremoteness\n"), None);
    }

    #[test]
    fn a_wireguard_copy_is_named_as_an_interface() {
        assert_eq!(iface_name("wg0"), "wg0");
        assert_eq!(iface_name("wg0 (1)"), "wg0-1");
        assert_eq!(iface_name("Mullvad Sweden Stockholm"), "Mullvad-Sweden");
        assert_eq!(iface_name("se-sto-wg-001"), "se-sto-wg-001");
        assert_eq!(iface_name("a_b=c+d.e-f"), "a_b=c+d.e-f");
        assert_eq!(iface_name("exactly15chars_"), "exactly15chars_");
        assert_eq!(iface_name("Café Wi-Fi"), "Caf-Wi-Fi");
        assert_eq!(iface_name("日本"), "wg0");
        assert_eq!(iface_name(".."), "wg0");
        assert_eq!(iface_name(""), "wg0");
        for stem in ["wg0 (1)", "Mullvad Sweden Stockholm", "Café Wi-Fi", "  x  "] {
            let n = iface_name(stem);
            assert!(n.len() <= 15 && n.chars().all(|c| c.is_ascii_alphanumeric() || "_=+.-".contains(c)), "{n:?}");
        }
    }

    #[test]
    fn nmcli_says_the_new_connections_uuid() {
        let out = "Connection 'wg (home)' (2b0e3c1a-1111-4222-8333-944455556666) successfully added.\n";
        assert_eq!(uuid_in(out), Some("2b0e3c1a-1111-4222-8333-944455556666"));
        assert_eq!(uuid_in("Connection 'x' successfully added.\n"), None);
    }

    fn failed(err: &str) -> Answer {
        Answer { ok: false, out: String::new(), err: err.into(), late: false }
    }

    #[test]
    fn an_import_failure_is_said_in_plain_words() {
        let ovpn = "/home/u/Downloads/se.ovpn";
        let said = import_trouble(&failed("Error: failed to find VPN plugin for openvpn.\n"), ovpn, "se.ovpn");
        assert!(said.starts_with("This computer cannot use OpenVPN yet"), "{said}");
        let said = import_trouble(&failed("Error: NetworkManager is not running.\n"), ovpn, "se.ovpn");
        assert_eq!(said, "The network service is not running.");
        // The copy's path is told as the picked file's name.
        let copy = "/run/user/1000/golem-vpn-7-9/wg0-1.conf";
        let err = format!("Error: failed to import '{copy}': unrecognized line at {copy}:3.\n");
        assert_eq!(import_trouble(&failed(&err), copy, "wg0 (1).conf"), "The file could not be imported: unrecognized line at wg0 (1).conf:3");
        assert_eq!(import_trouble(&failed(""), copy, "x.conf"), "The file could not be imported. Is it as your provider gave it?");
        let late = Answer { late: true, ..failed("") };
        assert_eq!(import_trouble(&late, copy, "x.conf"), "NetworkManager did not answer in time.");
    }

    #[test]
    fn an_on_or_off_failure_is_said_in_plain_words() {
        let said = turn_trouble("Office", false, &failed("Warning: password for 'vpn.secrets.password' not given in 'passwd-file' and nmcli cannot ask without '--ask' option.\nError: Connection activation failed: Secrets were required, but not provided.\n"));
        let lines: Vec<&str> = said.lines().collect();
        assert_eq!(lines.len(), 2, "{said}");
        assert!(lines[0].starts_with("Office needs a password"), "{said}");
        assert!(lines[1].ends_with("nmcli --ask connection up \"Office\""), "{said}");
        let said = turn_trouble("Office", false, &failed("Error: Timeout expired (50 seconds)\n"));
        assert!(said.starts_with("Office did not connect within a minute"), "{said}");
        assert!(turn_trouble("Office", false, &Answer { late: true, ..failed("") }).starts_with("Office did not connect"));
        assert_eq!(turn_trouble("Office", false, &failed("Error: Connection activation failed: The VPN service stopped unexpectedly.\n")), "Office could not be turned on.");
        assert_eq!(turn_trouble("Office", true, &failed("Error: not active\n")), "Office could not be turned off.");
        assert_eq!(turn_trouble("Office", true, &failed("Error: NetworkManager is not running.\n")), "The network service is not running.");
    }
}
