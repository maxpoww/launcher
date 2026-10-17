//! QUICK LOOK on a WEB PAGE (Max, 2026-10-17: "an html i think should be
//! seen as a website, not text, and offer the pencil to edit as text"): the
//! page as Seam shows it, its first screen — taken by Seam's own engine
//! (Firefox) HEADLESS, in a profile of Quick Look's own, the network cut
//! (no name is ever looked up: a page read here can call nobody), its
//! scripts run. Kept in the cache by the file's size and time, so a second
//! look is a picture read from disk. The pencil writes its source.

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

/// The page's screen, in CSS pixels, and how fine it is drawn.
const VIEW: (u32, u32) = (1280, 800);
const DENSITY: &str = "1.5";
/// The longest a page may take (a page that never settles is let go).
const PATIENCE: Duration = Duration::from_secs(12);

/// Whether a file of this name is a web page.
pub(crate) fn is_web(name: &str) -> bool {
    let lower = name.to_ascii_lowercase();
    [".html", ".htm", ".xhtml"]
        .iter()
        .any(|e| lower.ends_with(e))
}

/// The engine: the Firefox Seam's launcher runs (its profile is Seam's
/// own, so the launcher itself is not called), else any `firefox`.
fn engine() -> Option<PathBuf> {
    let path = std::env::var_os("PATH")?;
    for dir in std::env::split_paths(&path) {
        let seam = dir.join("seam");
        if let Ok(script) = std::fs::read_to_string(&seam) {
            // (`exec "/nix/store/…-seam-158.0/bin/firefox" --name seam …`)
            if let Some(at) = script.find("/bin/firefox\"") {
                let start = script[..at].rfind('"').map_or(0, |i| i + 1);
                let bin = PathBuf::from(&script[start..at + "/bin/firefox".len()]);
                if bin.is_file() {
                    return Some(bin);
                }
            }
        }
    }
    crate::launch::on_path("firefox").then(|| PathBuf::from("firefox"))
}

/// Quick Look's own profile: no network, nothing asked at start.
fn profile() -> Option<PathBuf> {
    let dir = crate::apps::cache_base().join("waverunner/web-profile");
    std::fs::create_dir_all(&dir).ok()?;
    let prefs = [
        ("network.dns.disabled", "true"),
        ("network.proxy.type", "1"),
        ("network.proxy.socks", "\"127.0.0.1\""),
        ("network.proxy.socks_port", "9"),
        ("network.proxy.allow_hijacking_localhost", "true"),
        ("layout.css.devPixelsPerPx", &format!("\"{DENSITY}\"")),
        ("browser.shell.checkDefaultBrowser", "false"),
        ("datareporting.policy.dataSubmissionEnabled", "false"),
        ("toolkit.telemetry.enabled", "false"),
        ("app.update.enabled", "false"),
        ("browser.startup.page", "0"),
        ("browser.aboutwelcome.enabled", "false"),
        ("media.autoplay.default", "5"),
    ];
    let text: String = prefs
        .iter()
        .map(|(k, v)| format!("user_pref(\"{k}\", {v});\n"))
        .collect();
    std::fs::write(dir.join("user.js"), text).ok()?;
    Some(dir)
}

/// Where the picture of `path` as it is now is kept.
fn kept(path: &Path) -> Option<PathBuf> {
    let meta = std::fs::metadata(path).ok()?;
    let changed = meta
        .modified()
        .ok()?
        .duration_since(std::time::UNIX_EPOCH)
        .ok()?
        .as_nanos();
    // FNV-1a: a name for a cache file, not a secret.
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for b in path
        .to_string_lossy()
        .bytes()
        .chain(meta.len().to_le_bytes())
        .chain(changed.to_le_bytes())
    {
        h ^= u64::from(b);
        h = h.wrapping_mul(0x0100_0000_01b3);
    }
    Some(crate::apps::cache_base().join(format!("waverunner/web/{h:016x}.png")))
}

/// The first screen of the page at `path`, as Seam draws it.
pub(crate) fn page(path: &Path) -> Option<image::RgbaImage> {
    let out = kept(path)?;
    if let Ok(img) = image::open(&out) {
        return Some(img.to_rgba8());
    }
    let engine = engine()?;
    let profile = profile()?;
    std::fs::create_dir_all(out.parent()?).ok()?;
    let tmp = out.with_extension(format!("{}.png", crate::peek_edit::nonce()));
    let url = format!("file://{}", path.display());
    let mut child = crate::peek_edit::dies_with_us(Command::new(engine))
        .args(["--headless", "--no-remote", "--profile"])
        .arg(&profile)
        .args([
            "--window-size",
            &format!("{},{}", VIEW.0, VIEW.1),
            "--screenshot",
        ])
        .arg(&tmp)
        .arg(url)
        .env("MOZ_CRASHREPORTER_DISABLE", "1")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .ok()?;
    let started = Instant::now();
    let done = loop {
        match child.try_wait() {
            Ok(Some(status)) => break status.success(),
            Ok(None) if started.elapsed() < PATIENCE => {
                std::thread::sleep(Duration::from_millis(25));
            }
            _ => {
                tracing::warn!("quick look: the page {} took too long", path.display());
                let _ = child.kill();
                let _ = child.wait();
                break false;
            }
        }
    };
    let img = done.then(|| image::open(&tmp).ok()).flatten();
    if img.is_some() {
        let _ = std::fs::rename(&tmp, &out);
    } else {
        let _ = std::fs::remove_file(&tmp);
    }
    Some(img?.to_rgba8())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn web_pages_are_known_by_their_endings() {
        assert!(is_web("index.HTML"));
        assert!(is_web("a.htm"));
        assert!(!is_web("notes.md"));
    }
}
