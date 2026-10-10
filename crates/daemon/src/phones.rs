//! The phones this computer knows: what its owner set for each in the
//! Configure box (`desktop_config.rs`) — the name of its folder here, whether
//! it is synced over Wi-Fi and what of it, whether a sync takes what it
//! copied off the phone.
//!
//! A phone is known by its SERIAL: two phones of one model are two phones
//! (Max, 2026-10-10: "we can have more than one Pixel 8 Pro" — so its folder
//! is named by its owner, the model's name being only what is offered).
//! Everything from a phone is kept in ONE folder, `~/Phones/<name>`, a
//! subfolder a kind of thing.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

/// A kind of thing a phone holds that is its owner's (not an app's, not the
/// system's): where it is on the phone, and the subfolder it has here.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Kind {
    pub key: &'static str,
    pub label: &'static str,
    /// Folders of the phone's storage (`/sdcard/<dir>`).
    pub dirs: &'static [&'static str],
    /// Its subfolder in the phone's folder here.
    pub folder: &'static str,
    /// Synced unless the owner says otherwise.
    pub usual: bool,
}

pub(crate) const KINDS: [Kind; 6] = [
    Kind { key: "photos", label: "Photos and videos", dirs: &["DCIM", "Pictures", "Movies"], folder: "Photos", usual: true },
    Kind { key: "music", label: "Music", dirs: &["Music", "Podcasts", "Audiobooks"], folder: "Music", usual: true },
    Kind { key: "downloads", label: "Downloads", dirs: &["Download"], folder: "Downloads", usual: true },
    Kind { key: "documents", label: "Documents", dirs: &["Documents"], folder: "Documents", usual: true },
    Kind { key: "recordings", label: "Recordings", dirs: &["Recordings"], folder: "Recordings", usual: false },
    Kind {
        key: "whatsapp",
        label: "WhatsApp media",
        dirs: &["Android/media/com.whatsapp/WhatsApp/Media"],
        folder: "WhatsApp",
        usual: false,
    },
];

pub(crate) fn kind(key: &str) -> Option<&'static Kind> {
    KINDS.iter().find(|k| k.key == key)
}

/// How often a phone on the Wi-Fi is synced.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub(crate) enum Every {
    /// When it comes onto the Wi-Fi, then every hour.
    #[default]
    Hour,
    Quarter,
    Day,
}

impl Every {
    pub const ALL: [Every; 3] = [Every::Hour, Every::Quarter, Every::Day];

    pub fn label(self) -> &'static str {
        match self {
            Every::Hour => "When it joins the Wi-Fi, then hourly",
            Every::Quarter => "Every 15 minutes",
            Every::Day => "Once a day",
        }
    }

    pub fn secs(self) -> u64 {
        match self {
            Every::Hour => 3600,
            Every::Quarter => 900,
            Every::Day => 86_400,
        }
    }

    pub fn next(self) -> Self {
        let at = Self::ALL.iter().position(|e| *e == self).unwrap_or(0);
        Self::ALL[(at + 1) % Self::ALL.len()]
    }
}

/// What its owner set for one phone.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub(crate) struct Phone {
    /// Its folder's name under `~/Phones`.
    pub name: String,
    /// Synced over Wi-Fi.
    #[serde(default)]
    pub sync: bool,
    /// The kinds synced and cleaned (`Kind::key`).
    #[serde(default = "usual_kinds")]
    pub kinds: Vec<String>,
    #[serde(default)]
    pub every: Every,
    #[serde(default = "yes")]
    pub charging_only: bool,
    /// A sync takes what it copied off the phone ("keep it clean").
    #[serde(default)]
    pub remove_after_sync: bool,
    /// Where it answers `adb` over Wi-Fi (`192.168.1.82:5555`), once allowed.
    #[serde(default)]
    pub addr: Option<String>,
    /// When it was last synced (seconds since the epoch).
    #[serde(default)]
    pub last_sync: Option<u64>,
}

fn yes() -> bool {
    true
}

pub(crate) fn usual_kinds() -> Vec<String> {
    KINDS.iter().filter(|k| k.usual).map(|k| k.key.to_owned()).collect()
}

impl Phone {
    /// What a phone has before its owner sets anything: its own name.
    pub fn new(name: &str) -> Self {
        Self {
            name: folder_name(name),
            sync: false,
            kinds: usual_kinds(),
            every: Every::default(),
            charging_only: true,
            remove_after_sync: false,
            addr: None,
            last_sync: None,
        }
    }

    /// Its folder on this computer.
    pub fn folder(&self) -> PathBuf {
        phones_dir().join(folder_name(&self.name))
    }

    /// The kinds it is synced and cleaned for.
    pub fn its_kinds(&self) -> Vec<&'static Kind> {
        KINDS.iter().filter(|k| self.kinds.iter().any(|key| key == k.key)).collect()
    }
}

/// Where every phone's folder is.
pub(crate) fn phones_dir() -> PathBuf {
    PathBuf::from(std::env::var("HOME").unwrap_or_default()).join("Phones")
}

/// A folder name out of what was typed (or a phone's own name): no slash, no
/// leading dot, never empty.
pub(crate) fn folder_name(name: &str) -> String {
    let tidy: String = name.chars().map(|c| if c == '/' || c.is_control() { ' ' } else { c }).collect();
    let tidy = tidy.trim().trim_start_matches('.').trim();
    if tidy.is_empty() { "Phone".to_owned() } else { tidy.to_owned() }
}

/// The phones known, by serial.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub(crate) struct Phones {
    #[serde(default)]
    pub by_serial: HashMap<String, Phone>,
}

impl Phones {
    pub fn load() -> Self {
        crate::persist::read_json(&Self::path()).unwrap_or_default()
    }

    pub fn save(&self) {
        crate::persist::write_json("phones", &Self::path(), self);
    }

    fn path() -> PathBuf {
        crate::persist::data_path("phones.json")
    }

    /// What is set for `serial`, or what a phone called `name` starts with.
    pub fn of(&self, serial: &str, name: &str) -> Phone {
        self.by_serial.get(serial).cloned().unwrap_or_else(|| Phone::new(name))
    }
}

/// Where a file of the phone goes here: `DCIM/Camera/a.jpg` of the photos →
/// `<folder>/Photos/Camera/a.jpg` (the kind's subfolder, then its path
/// without the phone's own top folder). `None` for a file under none of the
/// kind's folders.
pub(crate) fn place(kind: &Kind, path: &str, folder: &Path) -> Option<PathBuf> {
    let rest = kind.dirs.iter().find_map(|dir| path.strip_prefix(dir)?.strip_prefix('/'))?;
    (!rest.is_empty()).then(|| folder.join(kind.folder).join(rest))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_phones_things_have_their_places_here() {
        let folder = Path::new("/home/x/Phones/My Pixel");
        let photos = kind("photos").unwrap();
        assert_eq!(place(photos, "DCIM/Camera/a.jpg", folder), Some(folder.join("Photos/Camera/a.jpg")));
        assert_eq!(place(photos, "Pictures/Screenshots/s.png", folder), Some(folder.join("Photos/Screenshots/s.png")));
        assert_eq!(place(photos, "Movies/clip.mp4", folder), Some(folder.join("Photos/clip.mp4")));
        assert_eq!(place(photos, "Music/song.mp3", folder), None);
        let whatsapp = kind("whatsapp").unwrap();
        assert_eq!(
            place(whatsapp, "Android/media/com.whatsapp/WhatsApp/Media/WhatsApp Images/i.jpg", folder),
            Some(folder.join("WhatsApp/WhatsApp Images/i.jpg"))
        );
        // (A folder whose name only starts the same is not that folder.)
        assert_eq!(place(photos, "DCIM2/a.jpg", folder), None);
    }

    #[test]
    fn a_phone_starts_with_its_own_name_and_the_usual_kinds() {
        let phone = Phones::default().of("3A301FDJG000UW", "Pixel 8 Pro");
        assert_eq!(phone.name, "Pixel 8 Pro");
        assert!(!phone.sync && !phone.remove_after_sync && phone.charging_only);
        assert_eq!(phone.kinds, vec!["photos", "music", "downloads", "documents"]);
        assert!(phone.folder().ends_with("Phones/Pixel 8 Pro"));
        assert_eq!(folder_name("../etc"), "etc");
        assert_eq!(folder_name("  "), "Phone");
        assert_eq!(Every::Day.next(), Every::Hour);
        // What an older file lacks takes its usual value.
        let old: Phone = serde_json::from_str(r#"{"name":"Mine"}"#).unwrap();
        assert!(old.charging_only && old.kinds == usual_kinds());
    }
}
