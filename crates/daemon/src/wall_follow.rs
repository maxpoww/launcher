//! THE WALLPAPER THAT FOLLOWS YOUR COLOURS (Max, 2026-10-18: the first tile of
//! the wallpaper box is the palette in use, "Follows your colours"). Picked,
//! the wallpaper becomes a picture of the palette — its card made for the
//! screen (`palette_card::wall_svg`) — and from then on it FOLLOWS: change
//! the palette, or the shell's Dark/Light, in Colours and the picture is made
//! again and put up with the picture program's own fade. Picking any other
//! wallpaper ends it.
//!
//! DYNAMIC AND FOLLOWING ARE OPPOSITES. Dynamic makes the colours from the
//! picture; following makes the picture from the colours — together they
//! would chase each other for ever. So while the palette is Dynamic the tile
//! says "Choose a palette to use this" and a pick of it does nothing; and
//! choosing Dynamic while following ENDS following and keeps the last picture
//! as the wallpaper, so Dynamic makes its colours from it — almost the same
//! palette, no jump.
//!
//! While it follows, `GolemSettings::wallpaper` holds the picture that was
//! made (so the login's restore, the box's mark and Dynamic all keep working
//! with no knowledge of any of this) and `wallpaper_follows` says why.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Mutex;
use std::time::{Duration, SystemTime};

use tracing::{info, warn};

use crate::display::GolemSettings;
use crate::palette::{ColorChoice, DYNAMIC_ID};

/// The newest request to make the picture again: an older one that finishes
/// after it is dropped, so the picture put up is always the last asked for.
static LATEST: AtomicU64 = AtomicU64::new(0);

/// Following starts, ends, and puts a picture up under this: a pick of
/// another wallpaper and a picture finishing at the same moment cannot cross.
static ONE: Mutex<()> = Mutex::new(());

fn one() -> std::sync::MutexGuard<'static, ()> {
    ONE.lock().unwrap_or_else(|e| e.into_inner())
}

/// How many of the pictures made are kept (each is megabytes). The one in
/// use is kept whatever its age.
const KEEP: usize = 4;

// ------------------------------------------------------------------- pure

/// Whether `path` is one of the pictures made to measure.
pub(crate) fn is_wall(path: &str) -> bool {
    crate::palette_card::walls_dir().is_some_and(|d| Path::new(path).starts_with(d))
}

/// Whether `path` is what the first tile of the box carries: a palette's
/// card (or Dynamic's wheel). The tile shows that picture; a pick of it
/// arrives as that path.
pub(crate) fn is_tile(path: &str) -> bool {
    crate::palette_card::cards_dir().is_some_and(|d| Path::new(path).starts_with(d))
}

/// Whether the wallpaper can follow this choice: any palette but Dynamic.
pub(crate) fn can_follow(c: &ColorChoice) -> bool {
    c.palette_id() != DYNAMIC_ID
}

/// A pick of the first tile that has nothing to do: Dynamic is in use.
pub(crate) fn is_idle_tile(path: &str) -> bool {
    is_tile(path) && !can_follow(&crate::palette::choice())
}

/// The palette `id` was chosen: Dynamic ends following (the last picture
/// stays, as the wallpaper). True when the settings changed.
pub(crate) fn after_palette(s: &mut GolemSettings, id: &str) -> bool {
    if id == DYNAMIC_ID && s.wallpaper_follows {
        s.wallpaper_follows = false;
        return true;
    }
    false
}

/// Another wallpaper was picked: following ends. True when it changed.
pub(crate) fn after_pick(s: &mut GolemSettings) -> bool {
    std::mem::take(&mut s.wallpaper_follows)
}

/// Whether the picture `made` is to be put up: not when it already is the
/// wallpaper (and nothing else is on its way up).
pub(crate) fn should_show(s: &GolemSettings, made: &str, another_pending: bool) -> bool {
    another_pending || s.wallpaper.as_deref() != Some(made)
}

/// Which of the pictures `files` (path, modified) are let go: all but the
/// newest `keep`, and never one in `in_use`.
pub(crate) fn stale(files: &[(PathBuf, SystemTime)], keep: usize, in_use: &[&str]) -> Vec<PathBuf> {
    let mut order: Vec<&(PathBuf, SystemTime)> = files.iter().collect();
    order.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
    order
        .into_iter()
        .skip(keep)
        .map(|(p, _)| p.clone())
        .filter(|p| !in_use.iter().any(|u| Path::new(u) == p))
        .collect()
}

// ----------------------------------------------------------------- effects

/// The first tile was picked (Dynamic aside): the wallpaper follows from now,
/// and its first picture is made. Called on the settings' worker.
pub(crate) fn start() {
    if !can_follow(&crate::palette::choice()) {
        return;
    }
    {
        let _one = one();
        GolemSettings::change(|s| {
            if !s.wallpaper_follows {
                s.wallpaper_follows = true;
                s.store();
            }
        });
    }
    make_again();
}

/// The palette (or the shell's look) changed: when the wallpaper follows,
/// the picture is made again. Cheap when it does not.
pub(crate) fn refresh() {
    if GolemSettings::load().wallpaper_follows {
        make_again();
    }
}

/// Another wallpaper is being picked: following ends first, then `pick` puts
/// the picture up — under the same lock a picture finishing would take, so
/// the picture made never lands over the one the owner chose.
pub(crate) fn picked_other(pick: impl FnOnce()) {
    let _one = one();
    GolemSettings::change(|s| {
        if after_pick(s) {
            s.store();
        }
    });
    pick();
}

/// Make the picture of the palette in use, off every loop, and put it up.
fn make_again() {
    let mine = LATEST.fetch_add(1, Ordering::SeqCst) + 1;
    let spawned = std::thread::Builder::new().name("wall-follow".into()).spawn(move || {
        let newest = || LATEST.load(Ordering::SeqCst) == mine;
        let Some(swatch) = crate::palette::swatch_in_use() else {
            return;
        };
        let Some((w, h)) = crate::display::wallpaper_size() else {
            warn!("wallpaper: the screen's size is not known, the picture of the colours is not made");
            return;
        };
        // (Asked for again meanwhile: that one's picture is the one wanted.)
        if !newest() {
            return;
        }
        let started = std::time::Instant::now();
        let Some(png) = crate::palette_card::ensure_wall(&swatch, w, h) else {
            warn!("wallpaper: the picture of the colours could not be made ({w}x{h})");
            return;
        };
        let made = png.to_string_lossy().into_owned();
        let _one = one();
        if !newest() {
            return;
        }
        let s = GolemSettings::load();
        // (Another wallpaper was picked, or Dynamic chosen, while it was made.)
        if !s.wallpaper_follows || !should_show(&s, &made, crate::controls::wallpaper_pending()) {
            return;
        }
        info!("wallpaper: the colours' picture {w}x{h} ready in {:?}", started.elapsed());
        crate::controls::pick_wallpaper(made.clone());
        prune(&[&made, s.wallpaper.as_deref().unwrap_or("")]);
    });
    if let Err(e) = spawned {
        warn!("wallpaper: no thread for the picture of the colours: {e}");
    }
}

/// Let go of the old pictures made (and any half-written one a crash left).
fn prune(in_use: &[&str]) {
    let Some(dir) = crate::palette_card::walls_dir() else {
        return;
    };
    let mut pictures = Vec::new();
    for entry in std::fs::read_dir(&dir).into_iter().flatten().flatten() {
        let path = entry.path();
        let Some(modified) = entry.metadata().and_then(|m| m.modified()).ok() else {
            continue;
        };
        if path.extension().is_some_and(|e| e == "png") {
            pictures.push((path, modified));
        } else if modified.elapsed().is_ok_and(|age| age > Duration::from_secs(600)) {
            let _ = std::fs::remove_file(&path);
        }
    }
    for gone in stale(&pictures, KEEP, in_use) {
        let _ = std::fs::remove_file(gone);
    }
}

/// At login: the picture to put up when the wallpaper follows the colours —
/// made again if it is gone (a new cache, a cleaned disk) or the screen is
/// another size than when it was made. `None`: not following, or the colours
/// or the screen could not be read (the saved wallpaper stands).
pub(crate) fn login_picture() -> Option<String> {
    let s = GolemSettings::load();
    if !s.wallpaper_follows {
        return None;
    }
    let swatch = crate::palette::swatch_in_use()?;
    // (The compositor may be a moment from answering at the very start.)
    let mut size = None;
    for _ in 0..10 {
        size = crate::display::wallpaper_size();
        if size.is_some() {
            break;
        }
        std::thread::sleep(Duration::from_millis(300));
    }
    let (w, h) = size?;
    let made = crate::palette_card::ensure_wall(&swatch, w, h)?.to_string_lossy().into_owned();
    if s.wallpaper.as_deref() != Some(made.as_str()) {
        let _one = one();
        GolemSettings::change(|s| {
            if s.wallpaper_follows {
                s.wallpaper = Some(made.clone());
                s.store();
            }
        });
    }
    Some(made)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn following() -> GolemSettings {
        GolemSettings { wallpaper: Some("/cache/palette-walls/a-2560x1600.png".into()), wallpaper_follows: true, ..Default::default() }
    }

    #[test]
    fn dynamic_ends_following_and_keeps_the_last_picture() {
        let mut s = following();
        // Another palette: following goes on.
        assert!(!after_palette(&mut s, "jade"));
        assert!(s.wallpaper_follows);
        // Dynamic: following ends, the picture stays the wallpaper (Dynamic
        // makes its colours from it).
        assert!(after_palette(&mut s, DYNAMIC_ID));
        assert!(!s.wallpaper_follows);
        assert_eq!(s.wallpaper.as_deref(), Some("/cache/palette-walls/a-2560x1600.png"));
        // Nothing to end the second time.
        assert!(!after_palette(&mut s, DYNAMIC_ID));
    }

    #[test]
    fn picking_another_wallpaper_ends_following() {
        let mut s = following();
        assert!(after_pick(&mut s));
        assert!(!s.wallpaper_follows);
        assert!(!after_pick(&mut s));
    }

    #[test]
    fn only_a_palette_can_be_followed() {
        // A fresh choice is Dynamic (the default): nothing to follow.
        assert!(!can_follow(&ColorChoice::default()));
        assert!(!can_follow(&ColorChoice { palette: Some(DYNAMIC_ID.into()), ..Default::default() }));
        assert!(can_follow(&ColorChoice { palette: Some("jade".into()), ..Default::default() }));
        assert!(can_follow(&ColorChoice { palette: Some("golem".into()), ..Default::default() }));
    }

    #[test]
    fn the_flag_is_saved_only_while_it_is_on() {
        let json = serde_json::to_string(&GolemSettings::default()).unwrap();
        assert!(!json.contains("wallpaper_follows"), "{json}");
        let on = serde_json::to_string(&following()).unwrap();
        assert!(on.contains("\"wallpaper_follows\":true"), "{on}");
        let back: GolemSettings = serde_json::from_str(&on).unwrap();
        assert!(back.wallpaper_follows);
        // A settings file from before it: off.
        let old: GolemSettings = serde_json::from_str("{\"wallpaper\":\"/x.png\"}").unwrap();
        assert!(!old.wallpaper_follows);
    }

    #[test]
    fn a_picture_already_up_is_not_put_up_again() {
        let s = following();
        let made = "/cache/palette-walls/a-2560x1600.png";
        assert!(!should_show(&s, made, false));
        // …unless another pick is on its way (it would land after).
        assert!(should_show(&s, made, true));
        assert!(should_show(&s, "/cache/palette-walls/b-2560x1600.png", false));
        assert!(should_show(&GolemSettings::default(), made, false));
    }

    #[test]
    fn the_old_pictures_go_and_the_one_in_use_stays() {
        let t0 = SystemTime::UNIX_EPOCH;
        let at = |n: u64| t0 + Duration::from_secs(n);
        let files: Vec<(PathBuf, SystemTime)> = (0..8).map(|n| (PathBuf::from(format!("/w/{n}.png")), at(n))).collect();
        // The newest three stay.
        let gone = stale(&files, 3, &[]);
        assert_eq!(gone.len(), 5);
        assert!(gone.iter().all(|p| !["/w/7.png", "/w/6.png", "/w/5.png"].contains(&p.to_str().unwrap())));
        // The oldest is in use: it stays too.
        let gone = stale(&files, 3, &["/w/0.png"]);
        assert_eq!(gone.len(), 4);
        assert!(!gone.contains(&PathBuf::from("/w/0.png")));
        // Fewer than the number kept: nothing goes.
        assert!(stale(&files[..2], 3, &[]).is_empty());
    }
}
