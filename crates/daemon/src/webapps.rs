//! The webapp catalog: curated Chrome web-apps that live in the Install
//! search section (like nixpkgs packages) rather than on the Apps grid.
//!
//! The catalog is read from `~/.config/webapps.list` (`Name | URL | icon`,
//! `#` comments) — the same declarative, nix-managed file the reactive
//! `webapps-gen` uses. A catalog entry is *tried* by launching Chrome in
//! app mode, and *installed* by materializing a `webapp-<slug>.desktop`
//! launcher (so it becomes an ordinary grid app); the installed set is
//! recorded declaratively by [`crate::managed_webapps`].
//!
//! Frameless app mode = `google-chrome-stable --app=<URL>
//! --disable-client-side-decorations` (the `--app-id` form Chrome installs
//! keeps a title bar; only `--app=<URL>` honours the flag).

use std::path::{Path, PathBuf};
use std::process::Command;

use tracing::{debug, info, warn};

/// App-mode browsers in preference order: Chrome (the reference for our CDP +
/// CSD flags), then Chromium (flag-compatible, and the free one a Golem install
/// can actually ship). Resolved against PATH once per process — on a machine
/// with neither, webapp launches used to no-op silently (SH audit F2).
const BROWSERS: [&str; 3] = ["google-chrome-stable", "chromium", "chromium-browser"];
const FLAG: &str = "--disable-client-side-decorations";

/// The first [`BROWSERS`] entry present on PATH (falls back to the first and
/// warns when none are — the launch will then fail visibly in the log).
fn browser() -> &'static str {
    static RESOLVED: std::sync::OnceLock<&'static str> = std::sync::OnceLock::new();
    RESOLVED.get_or_init(|| {
        if let Some(b) = BROWSERS.into_iter().find(|b| crate::launch::on_path(b)) {
            debug!("webapps: using {b}");
            return b;
        }
        warn!("webapps: none of {BROWSERS:?} on PATH; webapp launches will fail");
        BROWSERS[0]
    })
}
/// Fixed CDP port the shared webapp Chrome instance listens on, so the clipboard
/// "copy link" pill can read an app-mode window's current URL (no address bar to
/// `Ctrl+L`). See [`crate::clipboard`]'s `copy_active_link`.
pub const CDP_PORT: u16 = 9333;

/// Absolute path of the dedicated Chrome profile all webapps share — separate
/// from the user's main browser so this one instance can own the debug port
/// (`--remote-debugging-port`). Keeps the profile dir named `Default`, so the
/// window `StartupWMClass` (`chrome-<host>__-Default`) is unchanged.
fn profile_dir() -> PathBuf {
    crate::persist::data_path("webapp-chrome")
}

/// Clear a *stale* Chrome `SingletonLock` from the shared webapp profile before
/// a webapp launch, so webapps still open after an unclean Chrome exit or a
/// machine rename. Called from [`crate::launch::launch`] for every launch; a
/// no-op unless `exec` targets our profile dir, so ordinary app launches pay
/// nothing.
///
/// Chrome encodes the lock as a `SingletonLock -> <hostname>-<pid>` symlink and
/// refuses to start while it looks live. It only reclaims a lock that names a
/// *dead* PID *on this host* — if the hostname differs it assumes the profile is
/// open "on another computer" and never clears it, so a single rename (this box
/// went `Golum` → `Golem`) permanently breaks every webapp launch. Because this
/// profile is waverunner-owned and single-purpose, we can safely clear a lock
/// that names a dead PID *or* a foreign host. A genuinely live instance (our
/// hostname + a live PID) is left intact, so a second webapp window still
/// attaches to it instead of spawning a rival process.
pub fn clear_stale_profile_lock_for(exec: &str) {
    let profile = profile_dir();
    let Some(profile_str) = profile.to_str() else { return };
    if !exec.contains(profile_str) {
        return; // not a webapp launch
    }
    let lock = profile.join("SingletonLock");
    let Ok(target) = std::fs::read_link(&lock) else { return }; // no lock → nothing to clear
    let target = target.to_string_lossy();
    if lock_is_live(&target, &hostname()) {
        return;
    }
    match std::fs::remove_file(&lock) {
        Ok(()) => info!("cleared stale Chrome webapp SingletonLock ({target})"),
        Err(e) => warn!("could not clear stale webapp SingletonLock: {e}"),
    }
}

/// Whether a `SingletonLock` target (`<hostname>-<pid>`) names a live instance
/// on this host: our hostname AND a live PID. A foreign hostname is never live
/// for us (the machine-rename bug), so we may reclaim it. Split on the LAST '-'
/// since hostnames may contain '-' but a PID never does.
fn lock_is_live(target: &str, this_host: &str) -> bool {
    target
        .rsplit_once('-')
        .is_some_and(|(host, pid)| host == this_host && pid.parse::<i32>().is_ok_and(pid_alive))
}

/// This machine's hostname (Linux: `/proc/sys/kernel/hostname`).
fn hostname() -> String {
    std::fs::read_to_string("/proc/sys/kernel/hostname")
        .map(|s| s.trim().to_owned())
        .unwrap_or_default()
}

/// Whether `pid` is a live process.
fn pid_alive(pid: i32) -> bool {
    Path::new(&format!("/proc/{pid}")).exists()
}

/// Whether a window class is an app-mode Chrome webapp (`--app=` window), whose
/// URL must be read via CDP — as opposed to a full browser (`google-chrome` /
/// `firefox`), where the address bar (`Ctrl+L`) works. The `--app` WM_CLASS is
/// always `chrome-<host>__-<profile>`.
pub fn is_app_window(class: &str) -> bool {
    class.starts_with("chrome-") && class.contains("__")
}

/// The host encoded in an app window's class (`chrome-www.youtube.com__-Default`
/// → `www.youtube.com`).
fn class_host(class: &str) -> Option<&str> {
    class.strip_prefix("chrome-")?.split("__").next()
}

/// The host of an http(s) URL.
pub fn url_host(url: &str) -> Option<&str> {
    let rest = url.strip_prefix("https://").or_else(|| url.strip_prefix("http://"))?;
    Some(rest.split(['/', '?', '#']).next().unwrap_or(""))
}

/// The host of the `--app=<url>` in a webapp launcher's `Exec` line, for
/// matching a copied link against an installed webapp.
pub fn exec_app_host(exec: &str) -> Option<&str> {
    let url = exec.split("--app=").nth(1)?.split_whitespace().next()?;
    url_host(url)
}

/// Build the frameless app-mode launch command for `url_token` (already a single
/// shell token) in the shared webapp profile + debug port — the one place the
/// browser flags live, used by installed webapps and by opening a link "as a
/// webapp".
fn app_exec_with(url_token: &str) -> String {
    format!(
        "{} --app={} {FLAG} --user-data-dir={} --remote-debugging-port={CDP_PORT}{}",
        browser(),
        url_token,
        profile_dir().display(),
        extension_flag(std::env::var("WAVERUNNER_WEBAPP_EXTENSION").ok()),
    )
}

/// ` --load-extension=<dir>` for the shared webapp profile when the
/// distribution ships an unpacked extension (WAVERUNNER_WEBAPP_EXTENSION,
/// set by the home-manager module — Golem points it at its vendored
/// notification-fix, which un-breaks FB/Messenger notifications on Wayland).
/// Chromium honours the flag; branded Chrome removed it in 137 (verified
/// ignored on 152), so on Chrome the extension still needs a one-time manual
/// "Load unpacked" — the flag is harmless there. Env unset/empty = no flag.
fn extension_flag(ext: Option<String>) -> String {
    match ext {
        Some(dir) if !dir.is_empty() => format!(" --load-extension={dir}"),
        _ => String::new(),
    }
}

/// Open an arbitrary link as a webapp (app-mode, shared profile) — the clipboard
/// "Open" pill's route for links that belong to a webapp. Shell-quoted because
/// link URLs carry `&`/`?` query strings and `launch` runs via `sh -c`.
pub fn app_open_exec(url: &str) -> String {
    app_exec_with(&crate::launch::shell_quote(url))
}

/// The live URL of the focused app-mode webapp window, read from the shared
/// Chrome instance's DevTools endpoint (`/json/list`). Best-effort: `None` if
/// the instance isn't up (webapp launched before this landed, so no debug port),
/// `curl` is missing, or no page matches. Matches the focused window by its host
/// (from the class) then its title, so the right one is picked when several
/// webapp windows are open. Blocking (localhost `curl`) — worker-thread only.
pub fn active_app_url(class: &str, title: &str) -> Option<String> {
    let host = class_host(class)?;
    let out = Command::new("curl")
        .args(["-s", "--max-time", "2", &format!("http://127.0.0.1:{CDP_PORT}/json/list")])
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    let targets: Vec<serde_json::Value> = serde_json::from_slice(&out.stdout).ok()?;
    let url = pick_page_url(&targets, host, title)?;
    debug!("webapp copy-link: {host} -> {url}");
    Some(url)
}

/// Pick the focused webapp window's URL from CDP `/json/list` targets: among
/// `page` targets on `host` with an http(s) URL (the webapp's own windows),
/// prefer the one whose title matches the focused window, else the first.
fn pick_page_url(targets: &[serde_json::Value], host: &str, title: &str) -> Option<String> {
    let pages: Vec<&serde_json::Value> = targets
        .iter()
        .filter(|t| t["type"] == "page")
        .filter(|t| t["url"].as_str().and_then(url_host).is_some_and(|h| h == host))
        .collect();
    let pick = pages
        .iter()
        .find(|t| t["title"].as_str() == Some(title))
        .or_else(|| pages.first());
    pick?.get("url")?.as_str().map(str::to_owned)
}


/// One curated web-app from the catalog.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WebappEntry {
    /// Display name (`Netflix`).
    pub name: String,
    /// Start URL opened in app mode.
    pub url: String,
    /// Freedesktop icon name (or path) for the tile and the launcher.
    pub icon: String,
    /// Filesystem-safe slug derived from the name; the launcher id is
    /// `webapp-<slug>` and the file `webapp-<slug>.desktop`.
    pub slug: String,
    /// Shown in the empty-query Install storefront (a `*` prefix on the Name
    /// in webapps.list). Non-recommended entries only surface when searched.
    pub recommended: bool,
}

impl WebappEntry {
    /// The desktop-file id this entry installs as (`webapp-netflix`).
    pub fn desktop_id(&self) -> String {
        id_for_slug(&self.slug)
    }

    /// The frameless `Exec=` command — also used for a "try it" launch. Runs in
    /// the shared webapp profile with the CDP port so the "copy link" pill can
    /// read the window's live URL. Catalog URLs are clean (no query string), so
    /// they need no quoting.
    pub fn exec(&self) -> String {
        app_exec_with(&self.url)
    }

    /// `StartupWMClass` Chrome reports for an `--app=<URL>` window.
    fn wm_class(&self) -> String {
        let host = self
            .url
            .split_once("://")
            .map(|(_, rest)| rest)
            .unwrap_or(&self.url)
            .split('/')
            .next()
            .unwrap_or("");
        format!("chrome-{host}__-Default")
    }

    /// The `.desktop` contents for the installed launcher.
    fn desktop_contents(&self) -> String {
        format!(
            "[Desktop Entry]\n\
             Type=Application\n\
             Name={}\n\
             Exec={}\n\
             Icon={}\n\
             StartupWMClass={}\n\
             StartupNotify=true\n\
             Terminal=false\n\
             Categories=Network;\n",
            self.name,
            self.exec(),
            self.icon,
            self.wm_class(),
        )
    }
}

/// Turn a display name into a slug (`YouTube Music` -> `youtube-music`).
pub fn slug_of(name: &str) -> String {
    let mut slug = String::with_capacity(name.len());
    let mut prev_dash = true; // trim leading dashes
    for ch in name.chars() {
        if ch.is_ascii_alphanumeric() {
            slug.push(ch.to_ascii_lowercase());
            prev_dash = false;
        } else if !prev_dash {
            slug.push('-');
            prev_dash = true;
        }
    }
    while slug.ends_with('-') {
        slug.pop();
    }
    slug
}

/// `$XDG_CONFIG_HOME/webapps.list` (falling back to `~/.config`).
fn catalog_path() -> PathBuf {
    config_dir().join("webapps.list")
}

/// `$XDG_CONFIG_HOME` or `~/.config`.
fn config_dir() -> PathBuf {
    std::env::var("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|_| {
            PathBuf::from(std::env::var("HOME").unwrap_or_default()).join(".config")
        })
}

/// `$XDG_DATA_HOME/applications` (falling back to `~/.local/share`).
fn applications_dir() -> PathBuf {
    std::env::var("XDG_DATA_HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|_| {
            PathBuf::from(std::env::var("HOME").unwrap_or_default()).join(".local/share")
        })
        .join("applications")
}

/// Parse the catalog file into entries. A missing or unreadable file is an
/// empty catalog (never fatal). Lines are `Name | URL | icon`; `#` starts a
/// comment; an entry needs at least a name and a URL.
pub fn load_catalog() -> Vec<WebappEntry> {
    std::fs::read_to_string(catalog_path())
        .map(|t| parse_catalog(&t))
        .unwrap_or_default()
}

fn parse_catalog(text: &str) -> Vec<WebappEntry> {
    let mut out = Vec::new();
    for line in text.lines() {
        let line = line.split('#').next().unwrap_or("").trim();
        if line.is_empty() {
            continue;
        }
        let mut cols = line.split('|').map(str::trim);
        let (Some(raw_name), Some(url)) = (cols.next(), cols.next()) else {
            continue;
        };
        // A leading `*` marks a storefront recommendation.
        let (recommended, name) = match raw_name.strip_prefix('*') {
            Some(rest) => (true, rest.trim()),
            None => (false, raw_name),
        };
        if name.is_empty() || url.is_empty() {
            continue;
        }
        let icon = cols.next().unwrap_or("").trim().to_string();
        out.push(WebappEntry {
            name: name.to_string(),
            url: url.to_string(),
            icon: if icon.is_empty() {
                name.to_string()
            } else {
                icon
            },
            slug: slug_of(name),
            recommended,
        });
    }
    out
}

/// Materialize a `.desktop` launcher for every catalog entry, so the
/// indexer discovers them (icons rasterized by the normal pipeline) and
/// they become searchable. Whether each shows on the grid or only in the
/// Install section is decided at runtime by [`crate::managed_webapps`]
/// membership — the file merely has to exist. Write-only-on-change so it
/// doesn't churn the applications dir (and needlessly trip the rescan).
pub fn materialize_catalog() {
    let dir = applications_dir();
    if std::fs::create_dir_all(&dir).is_err() {
        return;
    }
    for entry in load_catalog() {
        let path = dir.join(format!("{}.desktop", entry.desktop_id()));
        let contents = entry.desktop_contents();
        let unchanged = std::fs::read_to_string(&path)
            .map(|c| c == contents)
            .unwrap_or(false);
        if !unchanged {
            let _ = std::fs::write(&path, contents);
        }
    }
}

/// The slug of a webapp desktop id (`webapp-netflix` -> `netflix`), or
/// `None` for a non-webapp id.
pub fn slug_of_id(id: &str) -> Option<&str> {
    id.strip_prefix("webapp-")
}

/// The desktop id a slug installs as (`netflix` -> `webapp-netflix`) —
/// the inverse of [`slug_of_id`], matching [`WebappEntry::desktop_id`].
pub fn id_for_slug(slug: &str) -> String {
    format!("webapp-{slug}")
}

/// Slugs of the catalog entries marked as storefront recommendations
/// (`*` prefix). Read once at startup.
pub fn recommended_slugs() -> std::collections::HashSet<String> {
    load_catalog()
        .into_iter()
        .filter(|e| e.recommended)
        .map(|e| e.slug)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn slugs_are_filesystem_safe() {
        assert_eq!(slug_of("YouTube Music"), "youtube-music");
        assert_eq!(slug_of("Proton Mail"), "proton-mail");
        assert_eq!(slug_of("  X "), "x");
        assert_eq!(slug_of("Google (Search)!"), "google-search");
    }

    #[test]
    fn app_window_detection_and_host_parse() {
        assert!(is_app_window("chrome-www.youtube.com__-Default"));
        assert!(!is_app_window("google-chrome"));
        assert!(!is_app_window("firefox"));
        assert_eq!(class_host("chrome-www.youtube.com__-Default"), Some("www.youtube.com"));
        assert_eq!(url_host("https://www.youtube.com/watch?v=x"), Some("www.youtube.com"));
    }

    #[test]
    fn singleton_lock_liveness() {
        let host = "Golem";
        let me = std::process::id();
        // Our host + our own (live) PID → live, keep it.
        assert!(lock_is_live(&format!("{host}-{me}"), host));
        // Foreign host, even with a live PID → stale (the machine-rename bug).
        assert!(!lock_is_live(&format!("Golum-{me}"), host));
        // Our host but a dead PID → stale.
        assert!(!lock_is_live(&format!("{host}-2147483647"), host));
        // Hostname with a '-' is parsed correctly (split on the last '-').
        assert!(lock_is_live(&format!("my-box-{me}"), "my-box"));
        // Garbage → not live (don't leave a lock we can't understand).
        assert!(!lock_is_live("garbage", host));
    }

    #[test]
    fn picks_the_focused_webapp_page_by_host_then_title() {
        let targets = serde_json::json!([
            { "type": "service_worker", "title": "sw", "url": "chrome-extension://x/sw.js" },
            { "type": "page", "title": "Home - YouTube", "url": "https://www.youtube.com/" },
            { "type": "page", "title": "Cool Video - YouTube", "url": "https://www.youtube.com/watch?v=abc" },
            { "type": "page", "title": "Spotify", "url": "https://open.spotify.com/" }
        ]);
        let targets = targets.as_array().unwrap();
        // Title match wins among same-host pages.
        assert_eq!(
            pick_page_url(targets, "www.youtube.com", "Cool Video - YouTube").as_deref(),
            Some("https://www.youtube.com/watch?v=abc")
        );
        // No title match → first page on the host.
        assert_eq!(
            pick_page_url(targets, "www.youtube.com", "nonexistent").as_deref(),
            Some("https://www.youtube.com/")
        );
        // Host with no page → None.
        assert_eq!(pick_page_url(targets, "example.com", "x"), None);
    }

    #[test]
    fn parses_recommended_star_prefix_and_columns() {
        let cat = parse_catalog(
            "# comment\n\
             *YouTube | https://youtube.com | youtube\n\
             Netflix  | https://netflix.com | netflix\n\
             \n\
             Bare | https://bare.example\n",
        );
        assert_eq!(cat.len(), 3);
        let yt = &cat[0];
        assert!(yt.recommended);
        assert_eq!(yt.name, "YouTube"); // `*` stripped
        assert_eq!(yt.slug, "youtube");
        assert!(!cat[1].recommended);
        // Missing icon column falls back to the name.
        assert_eq!(cat[2].icon, "Bare");
    }

    #[test]
    fn entry_derivations() {
        let e = WebappEntry {
            name: "Netflix".into(),
            url: "https://www.netflix.com".into(),
            icon: "netflix".into(),
            slug: "netflix".into(),
            recommended: false,
        };
        assert_eq!(e.desktop_id(), "webapp-netflix");
        let exec = e.exec();
        assert!(exec
            .starts_with("google-chrome-stable --app=https://www.netflix.com --disable-client-side-decorations"));
        assert!(exec.contains(&format!("--remote-debugging-port={CDP_PORT}")));
        assert!(exec.contains("--user-data-dir="));
        assert_eq!(e.wm_class(), "chrome-www.netflix.com__-Default");
        assert!(e.desktop_contents().contains("Icon=netflix"));
    }

    #[test]
    fn extension_flag_only_when_set() {
        assert_eq!(extension_flag(None), "");
        assert_eq!(extension_flag(Some(String::new())), "");
        assert_eq!(
            extension_flag(Some("/nix/store/abc-notification-fix".into())),
            " --load-extension=/nix/store/abc-notification-fix"
        );
    }
}
