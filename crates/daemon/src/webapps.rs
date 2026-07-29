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

use std::path::PathBuf;

const BROWSER: &str = "google-chrome-stable";
const FLAG: &str = "--disable-client-side-decorations";

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

    /// The frameless `Exec=` command — also used for a "try it" launch.
    pub fn exec(&self) -> String {
        format!("{BROWSER} --app={} {FLAG}", self.url)
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
        .unwrap_or_else(|_| PathBuf::from(std::env::var("HOME").unwrap_or_default()).join(".config"))
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
        let unchanged = std::fs::read_to_string(&path).map(|c| c == contents).unwrap_or(false);
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
        assert_eq!(
            e.exec(),
            "google-chrome-stable --app=https://www.netflix.com --disable-client-side-decorations"
        );
        assert_eq!(e.wm_class(), "chrome-www.netflix.com__-Default");
        assert!(e.desktop_contents().contains("Icon=netflix"));
    }
}
