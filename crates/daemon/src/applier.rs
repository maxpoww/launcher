//! Declarative install/uninstall: waverunner's side of the privileged
//! apply pipeline.
//!
//! Installs are declarative. The single source of truth is the plain-text
//! package list `~/.config/waverunner/packages.list` (one nixpkgs attr per
//! line). waverunner only ever edits *that* file — it never runs a rebuild
//! or touches anything root-owned. A root systemd `.path` unit watches the
//! list; when it changes, the privileged `waverunner-apply` helper (defined
//! in `/etc/nixos/waverunner-apply.nix`) validates it, regenerates the
//! root-owned `/etc/nixos/waverunner-packages.nix` that `home.nix` imports,
//! runs `nixos-rebuild switch`, and writes the result to
//! `~/.config/waverunner/apply-status.json`.
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
        // Already declared — assume applied (idempotent re-drop).
        info!("{attr} already in package list; treating as installed");
        return true;
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
        info!("{attr} not in package list; treating as already removed");
        return true;
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

/// Block until the apply helper reports a terminal status for a run that
/// started at or after `since`. Returns the rebuild's success. Gives up
/// (false) if the helper never starts within [`START_TIMEOUT`] (trigger not
/// wired) or a run runs past [`BUILD_TIMEOUT`].
fn wait_for_apply(since: f64) -> bool {
    let start = Instant::now();
    let mut saw_run = false;
    loop {
        std::thread::sleep(POLL);
        if let Some(st) = read_status() {
            let ours = st.started >= since || st.finished >= since;
            if ours {
                saw_run = true;
                if st.phase == "done" && st.finished >= since {
                    if let Some(err) = st.error.as_deref().filter(|_| st.ok != Some(true)) {
                        warn!("apply failed: {}", err.trim());
                    }
                    return st.ok.unwrap_or(false);
                }
            }
        }
        let elapsed = start.elapsed();
        if !saw_run && elapsed > START_TIMEOUT {
            warn!(
                "waverunner-apply did not start within {}s — is the systemd path unit installed?",
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
