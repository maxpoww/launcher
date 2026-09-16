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
//! STARTED after the list write (only such a run read the written list);
//! a busy helper means QUEUED, not failed (and
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
    /// Unix epoch (fractional seconds) the run started. The file also
    /// carries `finished`, but coverage is started-based (only a run that
    /// STARTED after a write read that write), so the daemon ignores it.
    #[serde(default)]
    started: f64,
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

/// One operation of a coalesced batch (#52): an install or an uninstall
/// of a nixpkgs attr.
pub struct BatchOp {
    pub attr: String,
    pub install: bool,
}

/// What [`plan_batch`] decided for one op: already settled without any
/// rebuild, or riding the batch's single rebuild.
#[derive(Debug, PartialEq, Eq)]
enum BatchAction {
    /// Resolved by the F9 fast path (already in/absent from the list with
    /// a covering successful run) — no rebuild needed for this op.
    Fast(bool),
    /// Its list edit (or its join of the pending state) rides the wait.
    Ride,
}

/// Pure batch planner (#52): fold every op's list edit into ONE new list.
/// `applied` is [`applied_since_list_write`] at planning time — it decides
/// the F9 fast paths exactly as the single-op functions do. Returns the
/// folded list plus each op's action, in op order.
fn plan_batch(
    current: &[String],
    ops: &[BatchOp],
    applied: bool,
) -> (Vec<String>, Vec<BatchAction>) {
    let mut list: Vec<String> = current.to_vec();
    let mut actions = Vec::with_capacity(ops.len());
    for op in ops {
        let present = list.iter().any(|a| a == &op.attr);
        let action = match (op.install, present) {
            (true, true) | (false, false) if applied => BatchAction::Fast(true),
            (true, true) | (false, false) => BatchAction::Ride, // declared-but-unapplied: join
            (true, false) => {
                list.push(op.attr.clone());
                BatchAction::Ride
            }
            (false, true) => {
                list.retain(|a| a != &op.attr);
                BatchAction::Ride
            }
        };
        actions.push(action);
    }
    (list, actions)
}

/// Apply a COALESCED batch of installs/uninstalls with ONE list write and
/// ONE rebuild wait (#52): N drags queued behind a running rebuild used to
/// cost N sequential rebuilds (~40 s each on the map machines — a 5-drag
/// batch burned ~4 minutes). Failure is attributed batch-wide: a failed
/// rebuild reverts every edited op (same honest semantics as the single-op
/// revert — the helper's last-good already kept the tree buildable) and
/// each op reports false; a retry then runs it alone. Returns each op's
/// outcome, in op order.
pub fn apply_batch(ops: &[BatchOp]) -> Vec<bool> {
    let current = list_attrs();
    let (new_list, actions) = plan_batch(&current, ops, applied_since_list_write());
    if actions.iter().all(|a| matches!(a, BatchAction::Fast(_))) {
        return actions
            .iter()
            .map(|a| matches!(a, BatchAction::Fast(true)))
            .collect();
    }
    let since = now_epoch();
    let names: Vec<&str> = ops.iter().map(|o| o.attr.as_str()).collect();
    info!(
        "coalesced apply of {} ops ({}) — one rebuild",
        ops.len(),
        names.join(", ")
    );
    if new_list != current {
        write_list(&new_list);
    }
    let ok = wait_for_apply(since);
    if !ok {
        warn!("coalesced apply failed; reverting the batch's edits");
        // Restore exactly the pre-batch declarations for the batch's attrs
        // (the list may have been edited by others meanwhile — touch only
        // our own attrs, like the single-op reverts do).
        let mut reverted = list_attrs();
        for op in ops {
            let was_present = current.iter().any(|a| a == &op.attr);
            let is_present = reverted.iter().any(|a| a == &op.attr);
            if was_present && !is_present {
                reverted.push(op.attr.clone());
            } else if !was_present && is_present {
                reverted.retain(|a| a != &op.attr);
            }
        }
        write_list(&reverted);
    }
    actions
        .iter()
        .map(|a| match a {
            BatchAction::Fast(v) => *v,
            BatchAction::Ride => ok,
        })
        .collect()
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

/// Whether a SUCCESSFUL apply run covers the last list write — i.e. what
/// the list declares is what the system actually has. A run only proves
/// the list it READ, so it must have STARTED at or after the write: a run
/// that started earlier and merely *finished* after it built the previous
/// list (an install landing mid-run resolved as done while its package was
/// absent from the built generation — the ASUS overlap hole, Golem #43b).
fn applied_since_list_write() -> bool {
    read_status().is_some_and(|st| {
        st.phase == "done" && st.ok == Some(true) && st.started >= list_mtime_epoch()
    })
}

/// Synchronous, side-effect-free "is this package already installed?" —
/// declared in the list AND a successful apply postdates the list write.
/// The same fast-path condition [`apply_install`] uses, but it triggers no
/// rebuild. Used at restore so a pending tile whose install already
/// finished is cleared rather than re-animated forever (the stale-tile /
/// fresh-install deadlock: an "installing" ring that never completes spins
/// the single-threaded loop and starves IPC + the very completion event
/// that would clear it — Golem changes.md #37/#40).
pub fn is_installed(attr: &str) -> bool {
    list_attrs().iter().any(|a| a == attr) && applied_since_list_write()
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
///
/// NOT `systemctl is-active`: a `Type=oneshot` service reports
/// `ActiveState=activating` for the whole time its ExecStart runs, and
/// `is-active` exits non-zero for that — so every LIVE build read as a
/// corpse, the waiter nudged into the void (path triggers are dropped
/// while the unit is activating) and false-failed the install at
/// [`START_TIMEOUT`], reverting the list mid-build (the ASUS, 2026-09-09:
/// proven with `is-active` = `activating` polled through a real 36 s run).
fn helper_active() -> bool {
    std::process::Command::new("systemctl")
        .args([
            "show",
            "waverunner-apply.service",
            "-p",
            "ActiveState",
            "--value",
        ])
        .output()
        .map(|o| {
            matches!(
                String::from_utf8_lossy(&o.stdout).trim(),
                "active" | "activating" | "reloading" | "deactivating"
            )
        })
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
/// F10 rules: only a run that STARTED at or after `since` read our edit —
/// a run that started before it (someone else's rebuild) built the old
/// list, even if it finishes after us. While such a foreign run is alive
/// we just wait: our edit is already on disk, but systemd DROPS path
/// triggers that fire while the service is active, so when it lands the
/// nudge below re-trips the watch by rewriting the (unchanged) list and
/// the wait continues into the fresh run. A `building` status with the
/// helper dead — ours or foreign — is a corpse to nudge past, not a run
/// to honor. [`START_TIMEOUT`] measures IDLE time (nothing running,
/// nothing picked up), so a slow foreign build can't burn the pickup
/// budget; [`BUILD_TIMEOUT`] stays the wall-clock hard cap.
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
    let mut nudges = 0u32;
    let mut idle_since = Instant::now();
    let mut liveness: Option<(Instant, bool)> = None;
    loop {
        std::thread::sleep(POLL);
        // A live rebuild is in flight — ours or someone else's. Either way
        // the machinery works and a nudge now would be dropped, so just
        // keep waiting.
        let mut busy = false;
        if let Some(st) = read_status() {
            if st.phase == "done" && st.started >= since {
                // A terminal run that started after our edit read our
                // edit: this is our answer. (A foreign run finishing after
                // us built the OLD list — it proves nothing and falls
                // through to the nudge below.)
                if let Some(err) = st.error.as_deref().filter(|_| st.ok != Some(true)) {
                    warn!("apply failed: {}", err.trim());
                }
                return st.ok.unwrap_or(false);
            }
            if st.phase == "building" {
                // Mid-flight — but only if the helper is actually ALIVE: a
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
                    busy = true;
                    idle_since = Instant::now();
                }
            }
        }
        // Idle helper (stale terminal status or no status at all) and our
        // run never appeared: the write raced the helper's read or the
        // trigger was dropped — re-trip the watch with an identical
        // rewrite.
        if !busy && nudges < MAX_NUDGES && idle_since.elapsed() > NUDGE_AFTER {
            info!(
                "apply run not picked up; re-tripping the watch (nudge {})",
                nudges + 1
            );
            write_list(&list_attrs());
            nudges += 1;
            idle_since = Instant::now();
        }
        if !busy && idle_since.elapsed() > START_TIMEOUT {
            warn!(
                "waverunner-apply never picked up the change (after {nudges} nudges, {}s idle) — is the systemd path unit installed?",
                START_TIMEOUT.as_secs()
            );
            return false;
        }
        if start.elapsed() > BUILD_TIMEOUT {
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
    fn batch_planner_folds_edits_and_honors_fast_paths() {
        let cur = vec!["vlc".to_string(), "mousepad".to_string()];
        let ops = vec![
            BatchOp {
                attr: "gimp".into(),
                install: true,
            }, // new install → edit
            BatchOp {
                attr: "vlc".into(),
                install: false,
            }, // uninstall present → edit
            BatchOp {
                attr: "mousepad".into(),
                install: true,
            }, // already in list…
            BatchOp {
                attr: "krita".into(),
                install: false,
            }, // …and already absent
        ];
        // With a covering run: the last two are F9 fast-trues.
        let (list, actions) = plan_batch(&cur, &ops, true);
        assert_eq!(list, vec!["mousepad".to_string(), "gimp".to_string()]);
        assert_eq!(
            actions,
            vec![
                BatchAction::Ride,
                BatchAction::Ride,
                BatchAction::Fast(true),
                BatchAction::Fast(true),
            ]
        );
        // Without a covering run: everyone rides (declared-but-unapplied
        // must join the rebuild, same as the single-op F9 rule).
        let (_, actions) = plan_batch(&cur, &ops, false);
        assert!(actions.iter().all(|a| *a == BatchAction::Ride));
    }

    #[test]
    fn batch_planner_folds_conflicting_ops_in_order() {
        // install X then uninstall X in one batch: last op wins the list.
        let ops = vec![
            BatchOp {
                attr: "gimp".into(),
                install: true,
            },
            BatchOp {
                attr: "gimp".into(),
                install: false,
            },
        ];
        let (list, actions) = plan_batch(&[], &ops, true);
        assert!(list.is_empty());
        assert_eq!(actions, vec![BatchAction::Ride, BatchAction::Ride]);
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
