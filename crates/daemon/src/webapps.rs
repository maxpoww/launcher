//! The webapp catalog: curated web-apps that live in the Install search
//! section (like nixpkgs packages) rather than on the Apps grid.
//!
//! The catalog is read from `~/.config/webapps.list` (`Name | URL | icon`,
//! `#` comments). A catalog entry is *tried* by launching it, and *installed*
//! by materializing a `webapp-<slug>.desktop` launcher (so it becomes an
//! ordinary grid app); the installed set is recorded declaratively by
//! [`crate::managed_webapps`].
//!
//! A webapp runs in **Seam**, Golem's browser: `seam -golem-app <slug> <url>`
//! opens `<url>` as a webapp window of the running Seam: no tabs, no toolbar,
//! its own window class `webapp-<slug>`, one process and one sign-in with the
//! browser. The Seam half is Golem's `seam/golem-chrome.js` (WEBAPPS). Until
//! 2026-09-30 a webapp was a Chrome `--app` window on a shared Chrome profile,
//! and Golem ships no Chrome.

use std::path::PathBuf;

use tracing::debug;

/// The browser that runs webapps: Seam, on PATH on every Golem machine.
const SEAM: &str = "seam";
/// Seam's command-line flag for a webapp window (golem-chrome.js WEBAPPS).
const FLAG: &str = "-golem-app";
/// The window-class prefix Seam gives a webapp window (`webapp-<slug>`), and
/// the desktop-id prefix of its launcher: the two match, so the dock pairs a
/// running webapp with its tile by `StartupWMClass`.
const PREFIX: &str = "webapp-";

/// Whether a window class is a webapp window (`webapp-<slug>`), whose page is
/// read from Seam's report, as opposed to a full browser, where the address
/// bar (`Ctrl+L`) works.
pub fn is_app_window(class: &str) -> bool {
    class_slug(class).is_some()
}

/// The slug in a webapp window's class (`webapp-youtube-music` → `youtube-music`).
fn class_slug(class: &str) -> Option<&str> {
    class.strip_prefix(PREFIX).filter(|s| !s.is_empty())
}

/// The host of an http(s) URL.
pub fn url_host(url: &str) -> Option<&str> {
    let rest = url
        .strip_prefix("https://")
        .or_else(|| url.strip_prefix("http://"))?;
    Some(rest.split(['/', '?', '#']).next().unwrap_or(""))
}

/// The `(slug, url)` of a webapp launcher's `Exec` line
/// (`seam -golem-app <slug> <url>`).
fn exec_app(exec: &str) -> Option<(&str, &str)> {
    let mut it = exec.split_whitespace();
    it.find(|t| *t == FLAG)?;
    Some((it.next()?, it.next()?))
}

/// The slug a webapp launcher opens (`webapp-<slug>.desktop`'s `Exec`).
pub fn exec_app_slug(exec: &str) -> Option<&str> {
    exec_app(exec).map(|(slug, _)| slug)
}

/// The host of a webapp launcher's start URL, for matching a copied link
/// against an installed webapp.
pub fn exec_app_host(exec: &str) -> Option<&str> {
    url_host(exec_app(exec)?.1)
}

/// The launch command for webapp `slug` at `url_token` (already a single
/// shell token): the one place the Seam flags live, used by installed webapps
/// and by opening a link in a webapp.
fn app_exec_with(slug: &str, url_token: &str) -> String {
    format!("{SEAM} {FLAG} {slug} {url_token}")
}

/// Open an arbitrary link in the webapp `slug`: the clipboard "Open" pill's
/// route for a link that belongs to an installed webapp. Seam loads it in that
/// webapp's window (a launch at the webapp's own start URL only focuses it).
/// Shell-quoted: links carry `&`/`?`, and `launch` runs via `sh -c`.
pub fn app_open_exec(slug: &str, url: &str) -> String {
    app_exec_with(slug, &crate::launch::shell_quote(url))
}

/// Where Seam reports every open webapp window's page:
/// `$XDG_RUNTIME_DIR/seam/apps.json` = `{ "<slug>": { "url", "title" } }`.
fn apps_report_path() -> Option<PathBuf> {
    std::env::var_os("XDG_RUNTIME_DIR").map(|d| PathBuf::from(d).join("seam/apps.json"))
}

/// The live URL of a webapp window, from Seam's report (the window has no
/// address bar to copy from). `None` when Seam has not reported that slug.
pub fn active_app_url(class: &str) -> Option<String> {
    let slug = class_slug(class)?;
    let text = std::fs::read_to_string(apps_report_path()?).ok()?;
    let url = report_url(&text, slug)?;
    debug!("webapp copy-link: {slug} -> {url}");
    Some(url)
}

/// `slug`'s URL in Seam's report.
fn report_url(text: &str, slug: &str) -> Option<String> {
    let v: serde_json::Value = serde_json::from_str(text).ok()?;
    v.get(slug)?
        .get("url")?
        .as_str()
        .filter(|u| !u.is_empty())
        .map(str::to_owned)
}

/// One curated web-app from the catalog.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WebappEntry {
    /// Display name (`Netflix`).
    pub name: String,
    /// Start URL the webapp window opens at.
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

    /// The `Exec=` command, also used for a "try it" launch. Catalog URLs are
    /// clean (no query string) and slugs are `[a-z0-9-]`, so neither needs
    /// quoting.
    pub fn exec(&self) -> String {
        app_exec_with(&self.slug, &self.url)
    }

    /// `StartupWMClass`: the class Seam gives this webapp's window.
    fn wm_class(&self) -> String {
        format!("{PREFIX}{}", self.slug)
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
    id.strip_prefix(PREFIX)
}

/// The desktop id a slug installs as (`netflix` -> `webapp-netflix`) —
/// the inverse of [`slug_of_id`], matching [`WebappEntry::desktop_id`].
pub fn id_for_slug(slug: &str) -> String {
    format!("{PREFIX}{slug}")
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
        assert!(is_app_window("webapp-youtube-music"));
        assert!(!is_app_window("webapp-"));
        assert!(!is_app_window("seam"));
        assert!(!is_app_window("chrome-www.youtube.com__-Default"));
        assert_eq!(class_slug("webapp-youtube-music"), Some("youtube-music"));
        assert_eq!(
            url_host("https://www.youtube.com/watch?v=x"),
            Some("www.youtube.com")
        );
    }

    #[test]
    fn reads_the_webapp_page_from_seams_report() {
        let report = r#"{"youtube-music":{"url":"https://music.youtube.com/watch?v=abc","title":"Song"},"empty":{"url":""}}"#;
        assert_eq!(
            report_url(report, "youtube-music").as_deref(),
            Some("https://music.youtube.com/watch?v=abc")
        );
        assert_eq!(report_url(report, "empty"), None);
        assert_eq!(report_url(report, "absent"), None);
        assert_eq!(report_url("not json", "youtube-music"), None);
    }

    #[test]
    fn parses_a_launcher_exec() {
        let exec = "seam -golem-app youtube-music https://music.youtube.com";
        assert_eq!(exec_app_slug(exec), Some("youtube-music"));
        assert_eq!(exec_app_host(exec), Some("music.youtube.com"));
        assert_eq!(exec_app_slug("firefox https://x.org"), None);
        assert_eq!(exec_app_host("seam -golem-app lonely"), None);
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
        assert_eq!(e.exec(), "seam -golem-app netflix https://www.netflix.com");
        assert_eq!(e.wm_class(), "webapp-netflix");
        assert_eq!(e.wm_class(), e.desktop_id()); // the tile pairs with its window by class
        assert!(e.desktop_contents().contains("Icon=netflix"));
        assert!(e.desktop_contents().contains("StartupWMClass=webapp-netflix"));
        assert_eq!(
            app_open_exec("youtube", "https://www.youtube.com/watch?v=x&t=1"),
            "seam -golem-app youtube 'https://www.youtube.com/watch?v=x&t=1'"
        );
    }
}
