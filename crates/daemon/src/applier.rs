//! Declarative install/uninstall: waverunner's side of the privileged
//! apply pipeline.
//!
//! Installs are declarative. The single source of truth is the plain-text
//! package list `~/.config/waverunner/packages.list` (one nixpkgs attr per
//! line). waverunner only ever edits *that* file — it never runs a rebuild
//! or touches anything root-owned. A root systemd `.path` unit watches the
//! list; when it changes, the privileged `waverunner-apply` helper (the
//! system flake's `waverunner-apply.nix`) validates it, regenerates the
//! `waverunner-packages.nix` that the home config imports, rebuilds the
//! system, and writes the result to
//! `~/.config/waverunner/apply-status.json`.
//!
//! The status file is the truth the daemon trusts (F9/F10/F11, 2026-08-30):
//! declared-in-the-list only counts as installed once a successful run
//! postdates the list write; a busy helper means QUEUED, not failed (and
//! systemd drops path triggers that fire mid-run, so the waiter re-trips
//! the watch when the foreign run lands); an empty first-boot seed writes
//! nothing at all.
//!
//! The privilege boundary is the file: the helper parses the list as *data*
//! (strict attr charset) and generates the Nix itself, so a user-writable
//! file can never inject an expression into a root rebuild.
//!
//! [`apply_install`] / [`apply_uninstall`] run on the nix mutation thread:
//! they edit the list and block until the helper reports a terminal status,
//! then return success — so the existing `Event::Done` flow (grid tile
//! resolve, dock pin, rescan) is unchanged from the old imperative path.

use std::path::PathBuf;
use std::time::{Duration, Instant, SystemTime};

use serde::Deserialize;
use tracing::{info, warn};

/// How long to wait for the apply helper to *start* (write a status newer
/// than our edit) before concluding the trigger isn't wired — otherwise a
/// misconfigured system would hang the install forever.
const START_TIMEOUT: Duration = Duration::from_secs(120);
/// Hard cap on a single rebuild (a large first build / slow download).
const BUILD_TIMEOUT: Duration = Duration::from_secs(60 * 60);
/// Status poll interval.
const POLL: Duration = Duration::from_millis(500);

/// One line of the apply helper's status file.
#[derive(Debug, Deserialize)]
struct ApplyStatus {
    /// "building" while the rebuild runs, "done" when it finished.
    #[serde(default)]
    phase: String,
    /// `Some(true|false)` on a finished run, `None` while building.
    #[serde(default)]
    ok: Option<bool>,
    /// Unix epoch (fractional seconds) the run started / finished.
    #[serde(default)]
    started: f64,
    #[serde(default)]
    finished: f64,
    /// The `nixos-rebuild` error tail on failure.
    #[serde(default)]
    error: Option<String>,
}

/// `~/.config/waverunner/` (respecting `XDG_CONFIG_HOME`).
fn config_dir() -> PathBuf {
    std::env::var("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|_| {
            PathBuf::from(std::env::var("HOME").unwrap_or_default()).join(".config")
        })
        .join("waverunner")
}

/// The declarative package list waverunner owns (the trigger the helper
/// watches).
pub fn list_path() -> PathBuf {
    config_dir().join("packages.list")
}

/// The status file the privileged helper writes back.
fn status_path() -> PathBuf {
    config_dir().join("apply-status.json")
}

/// A valid nixpkgs attr token — the same charset the helper enforces, so
/// waverunner never writes a line the helper would silently drop.
fn is_valid_attr(s: &str) -> bool {
    let mut chars = s.chars();
    chars.next().is_some_and(|c| c.is_ascii_alphanumeric())
        && s.chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-'))
}

/// Parse a list file's text into the desired attr set, sorted and
/// de-duplicated. Comments (`#…`) and blank/invalid lines are ignored — the
/// list is tolerant of hand edits.
fn parse_list(text: &str) -> Vec<String> {
    let mut attrs: Vec<String> = text
        .lines()
        .filter_map(|line| {
            let line = line.split('#').next().unwrap_or("").trim();
            (!line.is_empty() && is_valid_attr(line)).then(|| line.to_owned())
        })
        .collect();
    attrs.sort_unstable();
    attrs.dedup();
    attrs
}

/// Render an attr set back to list-file text, sorted and de-duplicated.
fn render_list(attrs: &[String]) -> String {
    let mut sorted: Vec<&String> = attrs.iter().collect();
    sorted.sort();
    sorted.dedup();
    let mut text = String::from("# waverunner declarative packages — one nixpkgs attr per line.\n");
    for a in sorted {
        text.push_str(a);
        text.push('\n');
    }
    text
}

/// The current desired package set, in sorted order.
pub fn list_attrs() -> Vec<String> {
    parse_list(&std::fs::read_to_string(list_path()).unwrap_or_default())
}

/// Write the desired set to the list. The write is what trips the helper's
/// systemd `.path` watch and starts a rebuild.
///
/// Deliberately an in-place write (not temp + atomic rename): the path unit
/// triggers on `IN_CLOSE_WRITE` of the watched inode, which a rename-over
/// would not reliably deliver. A torn read is impossible here because installs
/// are serialized on the mutation thread — [`apply_install`] blocks in
/// [`wait_for_apply`] until the rebuild finishes, so waverunner never rewrites
/// the list while the helper is reading it.
fn write_list(attrs: &[String]) {
    let path = list_path();
    if let Some(dir) = path.parent() {
        if let Err(e) = std::fs::create_dir_all(dir) {
            warn!("packages: cannot create {dir:?}: {e}");
            return;
        }
    }
    if let Err(e) = std::fs::write(&path, render_list(attrs)) {
        warn!("packages: cannot write {path:?}: {e}");
    }
}

/// Seed the list from `attrs` when it does not exist yet — the one-time
/// migration from the old imperative `nix profile` set to the declarative
/// list. A present (even empty) list is left untouched.
pub fn seed_if_missing(attrs: &[String]) {
    if list_path().exists() {
        return;
    }
    let valid: Vec<String> = attrs.iter().filter(|a| is_valid_attr(a)).cloned().collect();
    // An EMPTY seed writes nothing (F11): the write itself trips the apply
    // watch, and a fresh machine's 0-attr seed was triggering a pointless
    // multi-minute first-boot rebuild — the first real install creates the
    // file instead.
    if valid.is_empty() {
        return;
    }
    info!(
        "seeding declarative package list with {} attrs",
        valid.len()
    );
    write_list(&valid);
}

/// Current unix epoch as fractional seconds, matching the helper's
/// `date +%s.%N` timestamps.
fn now_epoch() -> f64 {
    SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .map(|d| d.as_secs_f64())
        .unwrap_or(0.0)
}

/// Add `attr` to the list and block until the helper's rebuild finishes.
/// Returns whether the package is now installed. On a failed rebuild the
/// line is reverted so the desired set stays consistent with what actually
/// built (the helper already restored the last-good generated Nix, so the
/// system itself is never left broken).
pub fn apply_install(attr: &str) -> bool {
    let mut attrs = list_attrs();
    if attrs.iter().any(|a| a == attr) {
        // Declared ≠ applied (F9): the line may be mid-rebuild, or may
        // never have been picked up at all (external edit, dropped path
        // trigger). Fast-true only when a successful run postdates the
        // list; otherwise join/re-trigger and block like a normal install.
        if applied_since_list_write() {
            info!("{attr} already in package list and applied; treating as installed");
            return true;
        }
        info!("{attr} declared but not yet applied; ensuring rebuild");
        return wait_for_apply(now_epoch());
    }
    let since = now_epoch();
    attrs.push(attr.to_owned());
    info!("declaratively installing {attr} (nixos-rebuild switch)…");
    write_list(&attrs);
    if wait_for_apply(since) {
        return true;
    }
    // Rebuild failed / timed out: drop the line again so the list matches
    // the (reverted) system state.
    warn!("install of {attr} failed; reverting package list");
    let reverted: Vec<String> = list_attrs().into_iter().filter(|a| a != attr).collect();
    write_list(&reverted);
    false
}

/// Remove `attr` from the list and block until the rebuild finishes.
/// Returns whether the package is now gone. A failed rebuild re-adds the
/// line (the helper kept the last-good Nix, so the package is still there).
pub fn apply_uninstall(attr: &str) -> bool {
    let attrs = list_attrs();
    if !attrs.iter().any(|a| a == attr) {
        // Same F9 rule in reverse: absent from the list only means gone
        // once a successful run postdates the list write that removed it.
        if applied_since_list_write() {
            info!("{attr} not in package list and applied; treating as already removed");
            return true;
        }
        info!("{attr} absent but list not yet applied; ensuring rebuild");
        return wait_for_apply(now_epoch());
    }
    let since = now_epoch();
    let pruned: Vec<String> = attrs.into_iter().filter(|a| a != attr).collect();
    info!("declaratively uninstalling {attr} (nixos-rebuild switch)…");
    write_list(&pruned);
    if wait_for_apply(since) {
        return true;
    }
    warn!("uninstall of {attr} failed; restoring package list");
    let mut restored = list_attrs();
    if !restored.iter().any(|a| a == attr) {
        restored.push(attr.to_owned());
    }
    write_list(&restored);
    false
}

/// Read the current status, `None` if it is missing or unparsable.
fn read_status() -> Option<ApplyStatus> {
    crate::persist::read_json(&status_path())
}

/// The list file's mtime as fractional epoch seconds (0.0 when missing).
fn list_mtime_epoch() -> f64 {
    std::fs::metadata(list_path())
        .and_then(|m| m.modified())
        .ok()
        .and_then(|t| t.duration_since(SystemTime::UNIX_EPOCH).ok())
        .map(|d| d.as_secs_f64())
        .unwrap_or(0.0)
}

/// Whether a SUCCESSFUL apply run finished after the last list write —
/// i.e. what the list declares is what the system actually has.
fn applied_since_list_write() -> bool {
    read_status().is_some_and(|st| {
        st.phase == "done" && st.ok == Some(true) && st.finished >= list_mtime_epoch()
    })
}

/// Whether the startup reconcile (F13) has anything to prove: a non-empty
/// declared list with no successful apply run postdating its last write —
/// a daemon killed mid-install, a failed run whose revert never landed, or
/// an uninstall edit the helper never picked up. A missing/empty list needs
/// nothing (and must trigger nothing — F11).
pub fn needs_apply() -> bool {
    list_path().exists() && !list_attrs().is_empty() && !applied_since_list_write()
}

/// F13: make the system provably match the list, blocking like an install.
/// Fast-true when a successful run already postdates the last list write.
/// Otherwise join the in-flight rebuild or (via [`wait_for_apply`]'s nudge)
/// trip a fresh one. `force` re-trips the watch even when the status claims
/// applied — for drift the status file cannot see (a boot into an older
/// generation reverts the profile under a truthful "done ok" status).
pub fn ensure_applied(force: bool) -> bool {
    if !force && applied_since_list_write() {
        return true;
    }
    // Never CREATE the list here (F11: an empty write would trigger a
    // pointless first-boot rebuild); with nothing declared there is nothing
    // to reconcile.
    if !list_path().exists() {
        return true;
    }
    let since = now_epoch();
    if force {
        info!("reconcile: re-tripping the apply watch (profile drift)");
        write_list(&list_attrs());
    } else {
        info!("reconcile: list newer than last successful apply; ensuring rebuild");
    }
    wait_for_apply(since)
}

/// How long to observe an untouched, idle status before re-tripping the
/// watch (the initial write may have raced the helper's own read).
const NUDGE_AFTER: Duration = Duration::from_secs(5);
/// Give up re-tripping after this many attempts — past that the trigger
/// really is unwired.
const MAX_NUDGES: u32 = 3;
/// How often to re-verify that a foreign "building" status corresponds to a
/// live helper unit (one `systemctl` fork per poll would be noise).
const LIVENESS_EVERY: Duration = Duration::from_secs(5);

/// Whether the privileged apply helper's oneshot is actually running. A
/// status file stuck on "building" with the service inactive is a corpse —
/// the machine (or the helper) died mid-run and never wrote a terminal
/// status; honoring it would block every waiter for the full
/// [`BUILD_TIMEOUT`] (seen on the host, 2026-08-31: a shutdown mid-rebuild
/// left `building` behind and the next morning's startup reconcile hung on
/// it). Query failures err on "alive", so an environment without systemd
/// degrades to the old timeout behavior instead of spuriously nudging.
fn helper_active() -> bool {
    std::process::Command::new("systemctl")
        .args(["is-active", "--quiet", "waverunner-apply.service"])
        .status()
        .map(|s| s.success())
        .unwrap_or(true)
}

/// Whether the privileged apply mechanism is installed AT ALL — i.e. the
/// systemd `.path` unit that watches the list exists. On the live ISO (and
/// any machine with no flake checkout, `golem.flakeDir = null`) the unit is
/// absent BY DESIGN, so writing the list triggers nothing: without this
/// check every install would block the mutation thread for the full
/// [`START_TIMEOUT`] (120 s of nudging) before reporting a false "Failed" —
/// the "won't install anything" symptom on the live medium. Detecting the
/// missing unit lets an install fail FAST and honestly instead of hanging.
///
/// `systemctl` missing/erroring returns `true` (assume present): a test box
/// or a non-systemd environment then degrades to the old timeout behavior
/// rather than refusing every install outright.
fn apply_mechanism_present() -> bool {
    match std::process::Command::new("systemctl")
        .args(["cat", "waverunner-apply.path"])
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
    {
        Ok(s) => s.success(),
        Err(_) => true,
    }
}

/// Block until the apply helper reports a terminal status for a run that
/// covers a list write made at `since`. Returns the rebuild's success.
///
/// F10 rules: a run that STARTED BEFORE `since` and is still `building` is
/// someone else's rebuild, not a missing trigger — our edit is already on
/// disk, but systemd DROPS path triggers that fire while the service is
/// active, so when that run completes we re-trip the watch by rewriting
/// the (unchanged) list and keep waiting. Only a status that never moves
/// at all fails the [`START_TIMEOUT`] "trigger not wired" escape; a run
/// past [`BUILD_TIMEOUT`] is a hard timeout.
fn wait_for_apply(since: f64) -> bool {
    // No apply unit on this machine (live ISO / no flake checkout): the list
    // write triggered nothing and never will. Fail immediately rather than
    // nudge and poll for the full START_TIMEOUT — an instant, honest "Failed"
    // beats a two-minute hang that ends in the same place.
    if !apply_mechanism_present() {
        warn!(
            "waverunner-apply is not installed on this system (live medium / no flake checkout?); \
             package changes cannot be applied here"
        );
        return false;
    }
    let start = Instant::now();
    let mut saw_ours = false;
    let mut nudges = 0u32;
    let mut idle_since = Instant::now();
    let mut liveness: Option<(Instant, bool)> = None;
    loop {
        std::thread::sleep(POLL);
        let mut foreign_building = false;
        let status = read_status();
        if let Some(st) = &status {
            if st.started >= since || st.finished >= since {
                saw_ours = true;
                if st.phase == "done" && st.finished >= since {
                    if let Some(err) = st.error.as_deref().filter(|_| st.ok != Some(true)) {
                        warn!("apply failed: {}", err.trim());
                    }
                    return st.ok.unwrap_or(false);
                }
            } else if st.phase == "building" {
                // A pre-existing run is mid-flight; our trigger was
                // swallowed. Wait it out — the nudge below fires once it
                // lands. But only if the helper is actually ALIVE: a
                // "building" status with the service inactive is a corpse
                // (died mid-run, no terminal status ever written) — treat
                // it as idle so the nudge re-trips the watch instead of
                // blocking on a run that will never finish.
                let alive = match liveness {
                    Some((at, alive)) if at.elapsed() < LIVENESS_EVERY => alive,
                    _ => {
                        let alive = helper_active();
                        if !alive {
                            warn!(
                                "stale apply status: phase 'building' but the helper is not running; treating as idle"
                            );
                        }
                        liveness = Some((Instant::now(), alive));
                        alive
                    }
                };
                if alive {
                    foreign_building = true;
                    idle_since = Instant::now();
                }
            }
        }
        // Idle helper (stale terminal status or no status at all) and our
        // run never appeared: the write raced the helper's read or the
        // trigger was dropped — re-trip the watch with an identical
        // rewrite.
        if !saw_ours
            && !foreign_building
            && nudges < MAX_NUDGES
            && idle_since.elapsed() > NUDGE_AFTER
        {
            info!(
                "apply run not picked up; re-tripping the watch (nudge {})",
                nudges + 1
            );
            write_list(&list_attrs());
            nudges += 1;
            idle_since = Instant::now();
        }
        let elapsed = start.elapsed();
        if !saw_ours && !foreign_building && elapsed > START_TIMEOUT {
            warn!(
                "waverunner-apply never started (after {nudges} nudges, {}s) — is the systemd path unit installed?",
                START_TIMEOUT.as_secs()
            );
            return false;
        }
        if elapsed > BUILD_TIMEOUT {
            warn!(
                "waverunner-apply timed out after {}s",
                BUILD_TIMEOUT.as_secs()
            );
            return false;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn valid_attr_charset() {
        assert!(is_valid_attr("vlc"));
        assert!(is_valid_attr("kdePackages.kdenlive"));
        assert!(is_valid_attr("ardour_8"));
        assert!(is_valid_attr("telegram-desktop"));
        assert!(!is_valid_attr(""));
        assert!(!is_valid_attr(".hidden"));
        assert!(!is_valid_attr("foo bar"));
        assert!(!is_valid_attr("evil; rm -rf"));
        assert!(!is_valid_attr("a=b"));
        assert!(!is_valid_attr("import ./x.nix"));
    }

    #[test]
    fn parse_drops_junk_sorts_and_dedups() {
        let text = "\
# a comment\n\
vlc\n\
  audacity  \n\
vlc\n\
kdePackages.kdenlive # inline comment\n\
bad name\n\
evil; rm\n\
\n";
        // Comments/blank/invalid dropped; dups collapse; sorted.
        assert_eq!(
            parse_list(text),
            vec![
                "audacity".to_string(),
                "kdePackages.kdenlive".to_string(),
                "vlc".to_string(),
            ]
        );
    }

    #[test]
    fn render_round_trips_through_parse() {
        let attrs = vec!["vlc".to_string(), "audacity".to_string()];
        let rendered = render_list(&attrs);
        assert!(rendered.starts_with('#'), "keeps a header comment");
        assert_eq!(
            parse_list(&rendered),
            vec!["audacity".to_string(), "vlc".to_string()]
        );
    }
}
