//! The FreeDesktop.org Trash — the shared "home trash" every Linux file
//! manager (yazi, Nautilus/`gio`, Dolphin, `trash-cli`, …) reads and writes,
//! so a file sent to the trash from any of them shows up in the Recycle Bin.
//!
//! Layout (spec: <https://specifications.freedesktop.org/trash-spec/>):
//! ```text
//! $XDG_DATA_HOME/Trash/            (default ~/.local/share/Trash)
//!   files/<name>                   the trashed file or directory
//!   info/<name>.trashinfo          [Trash Info] Path=… DeletionDate=…
//! ```
//! `Path` is the original absolute path, percent-encoded (RFC 3986, but `/`
//! left literal); `DeletionDate` is local time `YYYY-MM-DDThh:mm:ss`. The
//! `<name>` in `files/` and `info/` always match, and the name is *reserved*
//! by creating its `.trashinfo` with `O_EXCL` before the file is moved in — so
//! two trashers racing for the same name can't clobber each other.
//!
//! Scope: the **home trash** only. Files on other mounts may live in a
//! per-volume top-dir trash (`$topdir/.Trash-$uid`); reading those is a later
//! addition. A path from another filesystem (a stick, a card, a second disk)
//! is COPIED in — a folder whole, its links as links — and the original taken
//! away only once the copy is all there ([`move_across`]).

use std::fs;
use std::io::{self, Write};
use std::path::{Path, PathBuf};

/// One entry in the trash: a file/dir under `files/` with its `.trashinfo`.
#[derive(Debug, Clone)]
pub struct TrashItem {
    /// The entry name under `files/` (and the stem of `<name>.trashinfo`) —
    /// the stable id for restore/erase. May differ from the original file
    /// name when a collision forced a `name.2` suffix.
    pub name: String,
    /// Absolute path the file was trashed from (decoded from `Path=`).
    pub original_path: PathBuf,
    /// The raw `DeletionDate=` value (`YYYY-MM-DDThh:mm:ss`), kept verbatim for
    /// display; empty if the info file omitted it.
    pub deleted_at: String,
    /// Whether the trashed entry is a directory.
    pub is_dir: bool,
}

impl TrashItem {
    /// The display name — the original file name (basename of `original_path`),
    /// falling back to the on-disk `name` if the path is unusable.
    pub fn display_name(&self) -> &str {
        self.original_path
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or(&self.name)
    }
}

/// `$XDG_DATA_HOME` (or `~/.local/share`).
fn data_home() -> PathBuf {
    std::env::var_os("XDG_DATA_HOME")
        .map(PathBuf::from)
        .filter(|p| p.is_absolute())
        .unwrap_or_else(|| {
            PathBuf::from(std::env::var_os("HOME").unwrap_or_default()).join(".local/share")
        })
}

/// The home trash, `$XDG_DATA_HOME/Trash`.
pub struct Trash {
    root: PathBuf,
}

impl Trash {
    /// The user's home trash.
    pub fn home() -> Self {
        Self {
            root: data_home().join("Trash"),
        }
    }

    fn files_dir(&self) -> PathBuf {
        self.root.join("files")
    }
    fn info_dir(&self) -> PathBuf {
        self.root.join("info")
    }

    /// The `files/` directory itself (for anchoring a listing view).
    pub fn files_root(&self) -> PathBuf {
        self.files_dir()
    }

    /// The on-disk path of a listed entry (under `files/`) — what to open, and
    /// where the icon/thumbnail is read from.
    pub fn file_path(&self, name: &str) -> PathBuf {
        self.files_dir().join(name)
    }

    fn ensure_dirs(&self) -> io::Result<()> {
        fs::create_dir_all(self.files_dir())?;
        fs::create_dir_all(self.info_dir())?;
        Ok(())
    }

    /// Every current trash entry, newest `DeletionDate` first. Missing or
    /// unreadable trash dirs read as empty; an orphaned `.trashinfo` (no
    /// matching `files/` entry) or a bare `files/` entry (no info) is skipped.
    pub fn list(&self) -> Vec<TrashItem> {
        let info_dir = self.info_dir();
        let files_dir = self.files_dir();
        let Ok(entries) = fs::read_dir(&info_dir) else {
            return Vec::new();
        };
        let mut items: Vec<TrashItem> = entries
            .flatten()
            .filter_map(|e| {
                let path = e.path();
                let name = path
                    .file_name()?
                    .to_str()?
                    .strip_suffix(".trashinfo")?
                    .to_owned();
                let file = files_dir.join(&name);
                let meta = fs::symlink_metadata(&file).ok()?; // the trashed file must exist
                let text = fs::read_to_string(&path).ok()?;
                let (orig, date) = parse_trashinfo(&text)?;
                Some(TrashItem {
                    name,
                    original_path: orig,
                    deleted_at: date,
                    is_dir: meta.is_dir(),
                })
            })
            .collect();
        // Newest first (lexicographic on the ISO date is chronological).
        items.sort_by(|a, b| b.deleted_at.cmp(&a.deleted_at));
        items
    }

    /// How many entries are in the trash (cheap-ish; counts `info/`).
    #[allow(dead_code)] // trash API surface — the Recycle Bin UI cycles through callers
    pub fn is_empty(&self) -> bool {
        fs::read_dir(self.info_dir())
            .map(|mut d| d.next().is_none())
            .unwrap_or(true)
    }

    /// Permanently erase everything in the trash. Best-effort: it removes every
    /// `files/` entry and `info/` file it can, and returns the first error (if
    /// any) after attempting them all.
    pub fn empty(&self) -> io::Result<()> {
        let mut first_err = None;
        for dir in [self.files_dir(), self.info_dir()] {
            let Ok(entries) = fs::read_dir(&dir) else {
                continue;
            };
            for entry in entries.flatten() {
                let p = entry.path();
                let r = if entry.file_type().map(|t| t.is_dir()).unwrap_or(false) {
                    fs::remove_dir_all(&p)
                } else {
                    fs::remove_file(&p)
                };
                if let Err(e) = r {
                    first_err.get_or_insert(e);
                }
            }
        }
        first_err.map_or(Ok(()), Err)
    }

    /// Permanently erase one entry (its `files/` item and `.trashinfo`).
    #[allow(dead_code)] // trash API surface
    pub fn erase(&self, name: &str) -> io::Result<()> {
        let file = self.files_dir().join(name);
        let meta = fs::symlink_metadata(&file);
        if meta.map(|m| m.is_dir()).unwrap_or(false) {
            fs::remove_dir_all(&file)?;
        } else {
            let _ = fs::remove_file(&file);
        }
        let _ = fs::remove_file(self.info_dir().join(format!("{name}.trashinfo")));
        Ok(())
    }

    /// Restore an entry to its original path, then drop its `.trashinfo`.
    /// Creates the original parent directory if it has since gone away.
    #[allow(dead_code)] // trash API surface
    pub fn restore(&self, name: &str) -> io::Result<PathBuf> {
        let text = fs::read_to_string(self.info_dir().join(format!("{name}.trashinfo")))?;
        let (dest, _) = parse_trashinfo(&text)
            .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidData, "bad .trashinfo"))?;
        if let Some(parent) = dest.parent() {
            fs::create_dir_all(parent)?;
        }
        let src = self.files_dir().join(name);
        move_path(&src, &dest)?;
        let _ = fs::remove_file(self.info_dir().join(format!("{name}.trashinfo")));
        Ok(dest)
    }

    /// Move `path` into the trash, writing its `.trashinfo`. Returns the new
    /// entry. The name is reserved by creating the info file with `O_EXCL`
    /// (collisions retry as `name.2`, `name.3`, …), then the file is moved in.
    pub fn trash(&self, path: &Path) -> io::Result<TrashItem> {
        // The system's own folders never go in the bin, whoever asks
        // (`protected`): the one rule every way of throwing away meets.
        if protected(path) {
            return Err(io::Error::new(io::ErrorKind::PermissionDenied, "one of the system's folders: it stays"));
        }
        self.ensure_dirs()?;
        // The FOLDER is made real, the name kept: resolving the whole path
        // followed a symlink and binned what it pointed at, leaving the
        // link behind (a shortcut on the desktop took the real folder).
        let abs = match (path.parent().filter(|p| !p.as_os_str().is_empty()), path.file_name()) {
            (Some(dir), Some(name)) => std::fs::canonicalize(dir).map(|d| d.join(name)),
            _ => std::fs::canonicalize(path),
        }
        .or_else(|_| absolutize(path))
        .unwrap_or_else(|_| path.to_path_buf());
        let base = abs
            .file_name()
            .and_then(|n| n.to_str())
            .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "no file name"))?;

        // Reserve a free <name> by creating its info file exclusively.
        let (name, mut info_file) = self.reserve_name(base)?;
        let deleted_at = now_local_iso();
        let body = format!(
            "[Trash Info]\nPath={}\nDeletionDate={}\n",
            encode_path(&abs),
            deleted_at
        );
        info_file.write_all(body.as_bytes())?;
        info_file.sync_all().ok();
        drop(info_file);

        // Move the file into files/ under the reserved name. If the move fails,
        // release the reserved info file so the name isn't left dangling.
        let dest = self.files_dir().join(&name);
        if let Err(e) = move_path(&abs, &dest) {
            let _ = fs::remove_file(self.info_dir().join(format!("{name}.trashinfo")));
            return Err(e);
        }
        let is_dir = fs::symlink_metadata(&dest)
            .map(|m| m.is_dir())
            .unwrap_or(false);
        Ok(TrashItem {
            name,
            original_path: abs,
            deleted_at,
            is_dir,
        })
    }

    /// Find and exclusively create `info/<name>.trashinfo` for a free `<name>`
    /// derived from `base` (`base`, `base.2`, `base.3`, …), so concurrent
    /// trashers can't pick the same slot. Returns the chosen name + open file.
    fn reserve_name(&self, base: &str) -> io::Result<(String, fs::File)> {
        for n in 1..10_000u32 {
            let name = if n == 1 {
                base.to_owned()
            } else {
                format!("{base}.{n}")
            };
            let info = self.info_dir().join(format!("{name}.trashinfo"));
            match fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(&info)
            {
                Ok(f) => return Ok((name, f)),
                Err(e) if e.kind() == io::ErrorKind::AlreadyExists => continue,
                Err(e) => return Err(e),
            }
        }
        Err(io::Error::new(
            io::ErrorKind::AlreadyExists,
            "no free trash name",
        ))
    }
}

/// Move `src` to `dst`, falling back to copy+remove across filesystems
/// (`rename` returns `EXDEV`).
fn move_path(src: &Path, dst: &Path) -> io::Result<()> {
    match fs::rename(src, dst) {
        Ok(()) => Ok(()),
        Err(e) if e.raw_os_error() == Some(libc::EXDEV) => move_across(src, dst),
        Err(e) => Err(e),
    }
}

/// `src` moved to `dst` on another disk: copied as it is, then taken away.
/// A folder used to be refused here ("cross-filesystem directory move not
/// supported"), so nothing on a stick or a card could go to the bin.
/// Nothing is overwritten (`dst` must not be there), and a copy that fails
/// halfway is taken back with the original left as it was.
fn move_across(src: &Path, dst: &Path) -> io::Result<()> {
    if fs::symlink_metadata(dst).is_ok() {
        return Err(io::Error::new(io::ErrorKind::AlreadyExists, "something is already there"));
    }
    if let Err(e) = copy_as_is(src, dst) {
        let _ = remove_any(dst);
        return Err(e);
    }
    remove_any(src)
}

/// `src` copied to `dst` AS IT IS: a folder with all that is in it, a link
/// as a link — never followed (a link to the home folder would bring the
/// whole home along). Anything else (a pipe, a device) is refused.
fn copy_as_is(src: &Path, dst: &Path) -> io::Result<()> {
    let meta = fs::symlink_metadata(src)?;
    let kind = meta.file_type();
    if kind.is_symlink() {
        std::os::unix::fs::symlink(fs::read_link(src)?, dst)
    } else if kind.is_dir() {
        fs::create_dir(dst)?;
        for entry in fs::read_dir(src)? {
            let entry = entry?;
            copy_as_is(&entry.path(), &dst.join(entry.file_name()))?;
        }
        // (Last: a folder that may not be written to is filled first.)
        fs::set_permissions(dst, meta.permissions())
    } else if kind.is_file() {
        fs::copy(src, dst).map(|_| ())
    } else {
        Err(io::Error::other("not a file, a folder or a link"))
    }
}

/// Take `path` away, whatever it is (a link: the link, not what it points at).
fn remove_any(path: &Path) -> io::Result<()> {
    if fs::symlink_metadata(path)?.is_dir() {
        fs::remove_dir_all(path)
    } else {
        fs::remove_file(path)
    }
}

/// Make `path` absolute against the CWD without resolving symlinks (used when
/// `canonicalize` fails, e.g. the file is already gone by the time we format).
/// WHAT CAN NEVER BE THROWN AWAY (Max, 2026-10-15: "the user can delete only
/// what the user created"): what the system made —
/// - the root, Home and every folder above it;
/// - the owner's folders (Desktop, Documents, Downloads…, as `user-dirs.dirs`
///   names them), `~/Phones`, `~/Golem` (the system's own checkout), and
///   everything hidden at the top of Home (`.config`, `.local`…: the apps'
///   and the system's settings);
/// - the bin itself and the folders it lives in;
/// - anything that is not the owner's (`/nix`, `/etc`, another user's);
/// - a volume's own folder (a stick is ejected, not binned).
///
/// Inside those folders, what the owner put there is the owner's to throw.
pub(crate) fn protected(path: &Path) -> bool {
    use std::os::unix::fs::MetadataExt;
    // Its folder made real, its own name kept (a link is judged as itself).
    let abs = match (path.parent().filter(|p| !p.as_os_str().is_empty()), path.file_name()) {
        (Some(dir), Some(name)) => fs::canonicalize(dir).map(|d| d.join(name)),
        _ => fs::canonicalize(path),
    }
    .or_else(|_| absolutize(path))
    .unwrap_or_else(|_| path.to_path_buf());
    let home = fs::canonicalize(crate::filer::home()).unwrap_or_else(|_| crate::filer::home());
    let Some(parent) = abs.parent() else {
        return true; // the root
    };
    if home.starts_with(&abs) {
        return true;
    }
    if parent == home {
        let name = abs.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
        if name.starts_with('.') || name == "Phones" || name == "Golem" {
            return true;
        }
    }
    if crate::desktop::is_own_folder(&abs) || Trash::home().files_root().starts_with(&abs) {
        return true;
    }
    let Ok(meta) = fs::symlink_metadata(&abs) else {
        return false; // not there: nothing to keep
    };
    // SAFETY: geteuid has no preconditions.
    if meta.uid() != unsafe { libc::geteuid() } {
        return true;
    }
    fs::metadata(parent).is_ok_and(|p| p.dev() != meta.dev() && meta.is_dir())
}

fn absolutize(path: &Path) -> io::Result<PathBuf> {
    if path.is_absolute() {
        Ok(path.to_path_buf())
    } else {
        Ok(std::env::current_dir()?.join(path))
    }
}

/// Parse the `Path=` (percent-decoded) and `DeletionDate=` out of a
/// `.trashinfo` body. `None` if there's no usable `Path=`.
fn parse_trashinfo(text: &str) -> Option<(PathBuf, String)> {
    let mut path = None;
    let mut date = String::new();
    for line in text.lines() {
        if let Some(v) = line.strip_prefix("Path=") {
            path = Some(PathBuf::from(decode_path(v.trim())));
        } else if let Some(v) = line.strip_prefix("DeletionDate=") {
            date = v.trim().to_owned();
        }
    }
    path.map(|p| (p, date))
}

/// Percent-encode a path per the trash spec: every byte except the RFC 3986
/// unreserved set (`A-Za-z0-9-._~`) is `%XX`-escaped, but `/` is left literal.
pub(crate) fn encode_path(path: &Path) -> String {
    use std::os::unix::ffi::OsStrExt;
    let bytes = path.as_os_str().as_bytes();
    let mut out = String::with_capacity(bytes.len());
    for &b in bytes {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'.' | b'_' | b'~' | b'/' => {
                out.push(b as char)
            }
            _ => out.push_str(&format!("%{b:02X}")),
        }
    }
    out
}

/// Decode a percent-encoded `Path=` value (or the path of a `file://` URI,
/// which the desktop's drop target reads) back to raw bytes → an `OsString`.
pub(crate) fn decode_path(s: &str) -> std::ffi::OsString {
    use std::os::unix::ffi::OsStringExt;
    let b = s.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'%' && i + 2 < b.len() {
            if let Some(hi) = (b[i + 1] as char).to_digit(16) {
                if let Some(lo) = (b[i + 2] as char).to_digit(16) {
                    out.push((hi * 16 + lo) as u8);
                    i += 3;
                    continue;
                }
            }
        }
        out.push(b[i]);
        i += 1;
    }
    std::ffi::OsString::from_vec(out)
}

/// Current local time as the trash spec's `YYYY-MM-DDThh:mm:ss`, via libc's
/// `localtime_r` (no date-library dependency).
fn now_local_iso() -> String {
    // SAFETY: `time` takes a null pointer (returns now); `localtime_r` fills a
    // caller-owned `tm`. Both are the standard libc contracts.
    unsafe {
        let t = libc::time(std::ptr::null_mut());
        let mut tm: libc::tm = std::mem::zeroed();
        if libc::localtime_r(&t, &mut tm).is_null() {
            return String::new();
        }
        format!(
            "{:04}-{:02}-{:02}T{:02}:{:02}:{:02}",
            tm.tm_year + 1900,
            tm.tm_mon + 1,
            tm.tm_mday,
            tm.tm_hour,
            tm.tm_min,
            tm.tm_sec,
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_systems_folders_stay_and_the_owners_things_go() {
        let home = crate::filer::home();
        assert!(protected(Path::new("/")));
        assert!(protected(&home));
        assert!(protected(home.parent().unwrap()));
        assert!(protected(&home.join(".config")));
        assert!(protected(&home.join("Desktop")));
        assert!(protected(Path::new("/nix")), "not the owner's");
        let d = std::env::temp_dir().join(format!("wr-protect-{}", std::process::id()));
        fs::create_dir_all(d.join("mine")).unwrap();
        assert!(!protected(&d.join("mine")), "what the owner made goes");
        let t = Trash { root: d.join("trash") };
        assert!(t.trash(Path::new("/")).is_err());
        fs::remove_dir_all(&d).ok();
    }

    // A unique temp root per call so parallel tests never share a trash dir.
    fn unique_id() -> String {
        use std::sync::atomic::{AtomicU32, Ordering};
        static N: AtomicU32 = AtomicU32::new(0);
        format!(
            "{}-{}",
            std::process::id(),
            N.fetch_add(1, Ordering::Relaxed)
        )
    }

    fn temp_trash() -> Trash {
        let root = std::env::temp_dir().join(format!("wr-trash-test-{}", unique_id()));
        let _ = fs::remove_dir_all(&root);
        Trash { root }
    }

    #[test]
    fn encode_decode_round_trips_spaces_and_unicode() {
        let p = Path::new("/home/max/a file — café/x.txt");
        let enc = encode_path(p);
        assert!(enc.contains("%20"), "space encoded");
        assert!(enc.starts_with("/home/max/"), "slashes kept literal");
        assert_eq!(PathBuf::from(decode_path(&enc)), p);
    }

    #[test]
    fn binning_a_link_takes_the_link_not_what_it_points_at() {
        let t = temp_trash();
        let d = std::env::temp_dir().join(format!("wr-link-{}", std::process::id()));
        let _ = fs::remove_dir_all(&d);
        fs::create_dir_all(d.join("real")).unwrap();
        fs::write(d.join("real/keep.txt"), b"keep").unwrap();
        std::os::unix::fs::symlink(d.join("real"), d.join("shortcut")).unwrap();
        let item = t.trash(&d.join("shortcut")).unwrap();
        assert_eq!(item.display_name(), "shortcut");
        assert!(d.join("real/keep.txt").exists(), "the real folder stays");
        assert!(fs::symlink_metadata(d.join("shortcut")).is_err(), "the link went");
        let _ = fs::remove_dir_all(&d);
        let _ = fs::remove_dir_all(&t.root);
    }

    #[test]
    fn trash_then_list_restore() {
        let t = temp_trash();
        // A file to trash (outside the trash root so the move is intra-fs).
        let src_dir = std::env::temp_dir().join(format!("wr-src-{}", std::process::id()));
        let _ = fs::remove_dir_all(&src_dir);
        fs::create_dir_all(&src_dir).unwrap();
        let src = src_dir.join("notes.txt");
        fs::write(&src, b"hello").unwrap();

        let item = t.trash(&src).unwrap();
        assert_eq!(item.display_name(), "notes.txt");
        assert!(!src.exists(), "original moved out");

        let listed = t.list();
        assert_eq!(listed.len(), 1);
        assert_eq!(listed[0].original_path, src);
        assert!(!listed[0].deleted_at.is_empty(), "date written");

        // Restore puts it back and clears the trash.
        let restored = t.restore(&item.name).unwrap();
        assert_eq!(restored, src);
        assert_eq!(fs::read_to_string(&src).unwrap(), "hello");
        assert!(t.is_empty());

        let _ = fs::remove_dir_all(&src_dir);
        let _ = fs::remove_dir_all(&t.root);
    }

    #[test]
    fn a_folder_from_another_disk_is_copied_in_whole_then_taken_away() {
        // (`move_across` is what `rename`'s EXDEV leads to: one disk here.)
        let d = std::env::temp_dir().join(format!("wr-across-{}", unique_id()));
        let _ = fs::remove_dir_all(&d);
        let src = d.join("stick/Holiday");
        fs::create_dir_all(src.join("day 1")).unwrap();
        fs::write(src.join("day 1/a.jpg"), b"pixels").unwrap();
        fs::create_dir_all(d.join("home")).unwrap();
        fs::write(d.join("home/big.iso"), b"not to be copied").unwrap();
        std::os::unix::fs::symlink(d.join("home"), src.join("to home")).unwrap();
        let dst = d.join("bin/Holiday");
        fs::create_dir_all(d.join("bin")).unwrap();
        move_across(&src, &dst).unwrap();
        assert!(!src.exists(), "the original went");
        assert_eq!(fs::read(dst.join("day 1/a.jpg")).unwrap(), b"pixels");
        let link = dst.join("to home");
        assert!(fs::symlink_metadata(&link).unwrap().file_type().is_symlink(), "a link stays a link");
        assert_eq!(fs::read_link(&link).unwrap(), d.join("home"));
        assert!(d.join("home/big.iso").exists(), "what the link points at is untouched");
        // Nothing is ever copied over what is there.
        fs::create_dir_all(d.join("stick/again")).unwrap();
        let err = move_across(&d.join("stick/again"), &dst).unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::AlreadyExists);
        assert!(d.join("stick/again").exists() && dst.join("day 1/a.jpg").exists());
        let _ = fs::remove_dir_all(&d);
    }

    #[test]
    fn a_copy_that_fails_halfway_leaves_the_original_and_no_half_copy() {
        let d = std::env::temp_dir().join(format!("wr-half-{}", unique_id()));
        let _ = fs::remove_dir_all(&d);
        let src = d.join("stick/Mixed");
        fs::create_dir_all(&src).unwrap();
        fs::write(src.join("a.txt"), b"a").unwrap();
        // A pipe cannot be copied: the copy stops there.
        let fifo = std::ffi::CString::new(src.join("z-pipe").into_os_string().into_encoded_bytes()).unwrap();
        assert_eq!(unsafe { libc::mkfifo(fifo.as_ptr(), 0o600) }, 0);
        let dst = d.join("bin-Mixed");
        assert!(move_across(&src, &dst).is_err());
        assert!(src.join("a.txt").exists(), "the original is as it was");
        assert!(fs::symlink_metadata(&dst).is_err(), "no half copy left");
        let _ = fs::remove_dir_all(&d);
    }

    #[test]
    fn name_collision_gets_suffix_and_empty_clears() {
        let t = temp_trash();
        let src_dir = std::env::temp_dir().join(format!("wr-src2-{}", std::process::id()));
        let _ = fs::remove_dir_all(&src_dir);
        fs::create_dir_all(&src_dir).unwrap();

        // Trash two different files that share a basename.
        for sub in ["a", "b"] {
            let d = src_dir.join(sub);
            fs::create_dir_all(&d).unwrap();
            fs::write(d.join("dup.txt"), sub).unwrap();
            t.trash(&d.join("dup.txt")).unwrap();
        }
        let names: Vec<String> = t.list().into_iter().map(|i| i.name).collect();
        assert!(names.contains(&"dup.txt".to_string()));
        assert!(
            names.contains(&"dup.txt.2".to_string()),
            "collision suffixed: {names:?}"
        );

        t.empty().unwrap();
        assert!(t.is_empty());
        let _ = fs::remove_dir_all(&src_dir);
        let _ = fs::remove_dir_all(&t.root);
    }
}
