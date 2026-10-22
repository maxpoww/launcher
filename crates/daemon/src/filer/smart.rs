//! WHAT A KIND OF THING IS FOR (Max, 2026-10-21, mockup v51: "one action
//! per case on the pill, two, even 3 on the context menu; only actions that
//! just happen"). Songs and videos PLAY (and shuffle), photos make a
//! SLIDESHOW, one photo becomes the WALLPAPER or is TURNED, documents
//! PRINT — only while there is a printer (`print.rs`; office files only
//! with LibreOffice too). The pill shows one, first, in Open with's place;
//! the menu up to three, right after Open; a folder that is mostly songs or
//! photos (70 % of at least 3 files) gets its own by the ↶ (`filer_head`).
//!
//! Playing and the slideshow are Quick Look's (`peek_slides.rs`): what
//! is played goes through its player, a song after a song; the photos are
//! shown full screen on black, the next every 2.8 s.

use super::print::Can;
use super::*;
use crate::desktop::SendFor;

/// What a file is, as its actions see it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Sort {
    Song,
    Video,
    Photo,
    /// Printed as it is (PDF, text, PostScript).
    Doc,
    /// Printed once LibreOffice has made it a PDF.
    Office,
    Other,
}

/// What a file is, by its ending.
pub(crate) fn sort_of(name: &str) -> Sort {
    let end = name
        .rsplit_once('.')
        .map(|(_, e)| e.to_lowercase())
        .unwrap_or_default();
    match end.as_str() {
        "mp3" | "flac" | "ogg" | "oga" | "opus" | "m4a" | "aac" | "wav" | "wma" | "aiff"
        | "alac" => Sort::Song,
        "mp4" | "mkv" | "webm" | "mov" | "m4v" | "avi" | "wmv" | "mpg" | "mpeg" => Sort::Video,
        "jpg" | "jpeg" | "png" | "webp" | "gif" | "heic" | "heif" | "bmp" | "tif" | "tiff"
        | "avif" | "jxl" => Sort::Photo,
        "pdf" | "txt" | "md" | "ps" => Sort::Doc,
        "doc" | "docx" | "odt" | "rtf" | "xls" | "xlsx" | "ods" | "ppt" | "pptx" | "odp" => {
            Sort::Office
        }
        _ => Sort::Other,
    }
}

/// Whether a file of `sort` can be printed here now.
pub(crate) fn printable(sort: Sort, can: Can) -> bool {
    can.print
        && match sort {
            Sort::Doc | Sort::Photo => true,
            Sort::Office => can.office,
            _ => false,
        }
}

/// A folder that is mostly one kind of thing.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum FolderKind {
    Play,
    Slides,
}

/// What a folder holding files of these names is mostly: 70 % of at least
/// three files playable, or photos.
pub(crate) fn folder_kind<'a>(names: impl Iterator<Item = &'a str>) -> Option<FolderKind> {
    let (mut n, mut play, mut photos) = (0usize, 0usize, 0usize);
    for name in names {
        n += 1;
        match sort_of(name) {
            Sort::Song | Sort::Video => play += 1,
            Sort::Photo => photos += 1,
            _ => {}
        }
    }
    if n < 3 {
        None
    } else if play * 10 >= n * 7 {
        Some(FolderKind::Play)
    } else if photos * 10 >= n * 7 {
        Some(FolderKind::Slides)
    } else {
        None
    }
}

/// The files in folder `dir` (not its folders, nor dot files), by name —
/// at most a few thousand looked at.
pub(crate) fn files_in(dir: &Path) -> Vec<PathBuf> {
    let mut found: Vec<PathBuf> = std::fs::read_dir(dir)
        .into_iter()
        .flatten()
        .flatten()
        .take(4000)
        .filter(|e| e.file_type().is_ok_and(|t| t.is_file()))
        .filter(|e| !e.file_name().to_string_lossy().starts_with('.'))
        .map(|e| e.path())
        .collect();
    found.sort_by_key(|p| file_name(p).to_lowercase());
    found
}

/// A folder as last read for what it holds (`kind_of_folder`).
struct Folder {
    /// Its time of change when it was read.
    at: Option<std::time::SystemTime>,
    kind: Option<FolderKind>,
    /// On a slow place: when it was last looked at, and whether a thread is
    /// looking now.
    checked: Instant,
    reading: bool,
}

static FOLDERS: std::sync::Mutex<Option<HashMap<PathBuf, Folder>>> = std::sync::Mutex::new(None);

/// The loop's bell, rung when a folder read off the loop is in
/// (`filer_folder_bell`): the windows draw again with what it is.
static BELL: std::sync::OnceLock<calloop::ping::Ping> = std::sync::OnceLock::new();

/// How often a folder on a slow place is looked at again while it is drawn.
const RECHECK: Duration = Duration::from_secs(2);

/// What a folder is mostly, read once (and again when it changes). It is
/// asked at every draw of a window where it is selected, so on a SLOW place
/// (`slow_place`: a phone over MTP, the network) it is never read on the
/// loop: a Pixel's Camera folder (1555 photos) took 7 to 50 s to list, and
/// the whole shell stood still that long (2026-10-22). There it is read on a
/// thread and is not known (no Play all, no Slideshow) until it is in.
fn kind_of_folder(dir: &Path) -> Option<FolderKind> {
    let mut guard = FOLDERS.lock().unwrap_or_else(|e| e.into_inner());
    let folders = guard.get_or_insert_with(HashMap::new);
    let bell = BELL.get().filter(|_| slow_place(dir));
    let Some(bell) = bell else {
        let changed = std::fs::metadata(dir).and_then(|m| m.modified()).ok();
        if let Some(f) = folders.get(dir) {
            if f.at == changed {
                return f.kind;
            }
        }
        drop(guard);
        let kind = kind_of_files(&files_in(dir));
        let mut guard = FOLDERS.lock().unwrap_or_else(|e| e.into_inner());
        let folders = guard.get_or_insert_with(HashMap::new);
        if folders.len() > 200 {
            folders.clear();
        }
        folders.insert(dir.to_path_buf(), Folder { at: changed, kind, checked: Instant::now(), reading: false });
        return kind;
    };
    let (known, due) = match folders.get(dir) {
        Some(f) => (f.kind, !f.reading && f.checked.elapsed() >= RECHECK),
        None => (None, true),
    };
    if !due {
        return known;
    }
    if folders.len() > 200 {
        folders.clear();
    }
    let entry = folders.entry(dir.to_path_buf()).or_insert(Folder {
        at: None,
        kind: None,
        checked: Instant::now(),
        reading: false,
    });
    entry.reading = true;
    let (was, fresh) = (entry.at, entry.kind.is_none() && entry.at.is_none());
    drop(guard);
    let (dir, key, bell) = (dir.to_path_buf(), dir.to_path_buf(), bell.clone());
    let looked = std::thread::Builder::new().name("folder-kind".into()).spawn(move || {
        let changed = std::fs::metadata(&dir).and_then(|m| m.modified()).ok();
        let kind = (fresh || changed != was).then(|| kind_of_files(&files_in(&dir)));
        let mut guard = FOLDERS.lock().unwrap_or_else(|e| e.into_inner());
        let folders = guard.get_or_insert_with(HashMap::new);
        let f = folders.entry(dir).or_insert(Folder { at: None, kind: None, checked: Instant::now(), reading: false });
        let before = f.kind;
        f.at = changed;
        if let Some(kind) = kind {
            f.kind = kind;
        }
        f.checked = Instant::now();
        f.reading = false;
        let ring = f.kind != before;
        drop(guard);
        if ring {
            bell.ping();
        }
    });
    if looked.is_err() {
        let mut guard = FOLDERS.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(f) = guard.as_mut().and_then(|m| m.get_mut(&key)) {
            f.reading = false;
        }
    }
    known
}

/// What these files make a folder (`folder_kind` by their names).
fn kind_of_files(files: &[PathBuf]) -> Option<FolderKind> {
    folder_kind(files.iter().map(|p| p.file_name().and_then(|n| n.to_str()).unwrap_or("")))
}

/// Whether `dir` is on a place every read of which is slow: a FUSE mount
/// (a phone over MTP through gvfs, sshfs, rclone…) or the network — by the
/// kind of the mount it is on (the longest mount point it is under).
pub(crate) fn slow_place(dir: &Path) -> bool {
    slow_mount(dir).is_some()
}

/// The slow place `dir` is on (`slow_place`): its mount point, and whether
/// it is the desktop's own (gvfs: a phone over MTP, a camera, a share).
pub(crate) fn slow_mount(dir: &Path) -> Option<(PathBuf, bool)> {
    with_mounts(|mounts| slow_in(mounts, dir))
}

/// Whether `a` and `b` are on one disk (the same mount): a move between
/// them is a rename, not a copy. Told from the mount table alone — neither
/// is touched (a phone's storage can be slow to answer, or gone).
pub(crate) fn same_disk(a: &Path, b: &Path) -> bool {
    with_mounts(|mounts| mount_in(mounts, a).map(|m| m.0) == mount_in(mounts, b).map(|m| m.0))
}

/// `f` of the mount table, read again at most every 2 s (this is asked at
/// draws).
fn with_mounts<R>(f: impl FnOnce(&str) -> R) -> R {
    static MOUNTS: std::sync::Mutex<Option<(Instant, String)>> = std::sync::Mutex::new(None);
    let mut guard = MOUNTS.lock().unwrap_or_else(|e| e.into_inner());
    if guard.as_ref().is_none_or(|(at, _)| at.elapsed() >= Duration::from_secs(2)) {
        *guard = Some((Instant::now(), std::fs::read_to_string("/proc/self/mounts").unwrap_or_default()));
    }
    f(guard.as_ref().map_or("", |(_, m)| m.as_str()))
}

fn slow_in(mounts: &str, dir: &Path) -> Option<(PathBuf, bool)> {
    let (at, kind) = mount_in(mounts, dir)?;
    let slow = kind.starts_with("fuse")
        || matches!(kind, "nfs" | "nfs4" | "cifs" | "smb3" | "9p" | "davfs" | "ceph" | "afs");
    slow.then(|| (PathBuf::from(at), kind == "fuse.gvfsd-fuse"))
}

/// The mount `dir` is on — the longest mount point it is under (the last
/// mounted there, of several): its point and its kind.
fn mount_in<'a>(mounts: &'a str, dir: &Path) -> Option<(String, &'a str)> {
    let mut best: Option<(String, &str)> = None;
    for line in mounts.lines() {
        let mut f = line.split(' ');
        let (Some(_), Some(at), Some(kind)) = (f.next(), f.next(), f.next()) else {
            continue;
        };
        let at = crate::desktop_send::unescape_mount(at);
        if !dir.starts_with(&at) {
            continue;
        }
        if best.as_ref().is_none_or(|(len, _)| at.len() >= len.len()) {
            best = Some((at, kind));
        }
    }
    best
}

/// The actions a selection is for: the pill's one (its symbol, its words,
/// what it does) and the menu's.
#[derive(Debug)]
pub(crate) struct Smart {
    pub(crate) bar: Option<(&'static str, &'static str, Action)>,
    pub(crate) menu: Vec<Row>,
}

impl Smart {
    fn none() -> Self {
        Smart {
            bar: None,
            menu: Vec::new(),
        }
    }

    fn of(
        bar: (&'static str, &'static str, Action),
        menu: &[(&'static str, &'static str, Action)],
    ) -> Self {
        Smart {
            bar: Some(bar),
            menu: menu
                .iter()
                .map(|(icon, words, action)| Row::item(*words, icon, *action))
                .collect(),
        }
    }
}

/// What files of these sorts are for, `can` being what this computer can
/// do (`archive`: the one file is an archive) — the rules, apart from the
/// disk.
pub(crate) fn smart_files(sorts: &[Sort], archive: bool, can: Can) -> Smart {
    let play = ("play", tr("Play"), Action::Play);
    let shuffle = ("shuffle", tr("Shuffle"), Action::Shuffle);
    let slides = ("slides", tr("Slideshow"), Action::Slideshow);
    let rotate = ("rotate", tr("Rotate"), Action::Rotate);
    let print = ("print", tr("Print"), Action::Print);
    let all = |f: &dyn Fn(Sort) -> bool| !sorts.is_empty() && sorts.iter().all(|s| f(*s));
    if all(&|s| matches!(s, Sort::Song | Sort::Video)) {
        return if sorts.len() > 1 {
            Smart::of(play, &[play, shuffle])
        } else {
            Smart::of(play, &[play])
        };
    }
    // Photos keep their one on the pill; Print takes the menu's third
    // place when there is a printer ("Slideshow from here" without one).
    if all(&|s| s == Sort::Photo) {
        return if sorts.len() == 1 {
            let wall = ("pic", tr("Set as wallpaper"), Action::SetWallpaper);
            let here = ("slides", tr("Slideshow from here"), Action::Slideshow);
            Smart::of(wall, &[wall, rotate, if can.print { print } else { here }])
        } else if can.print {
            Smart::of(slides, &[slides, rotate, print])
        } else {
            Smart::of(slides, &[slides, rotate])
        };
    }
    // Documents, and documents with photos: Print, when every one can be.
    if all(&|s| printable(s, can)) {
        return Smart::of(print, &[print]);
    }
    if archive && sorts.len() == 1 {
        return Smart {
            bar: Some(("extract", tr("Extract here"), Action::Extract)),
            menu: Vec::new(),
        };
    }
    Smart::none()
}

/// What the things at these paths are for (each with whether it is a
/// folder): a folder that is mostly songs or photos by what it holds, the
/// files by `smart_files` — for a file window's selection and the
/// desktop's alike.
pub(crate) fn smart_for(things: &[(&str, bool)], can: Can) -> Smart {
    if let [(path, true)] = things {
        let shuffle = ("shuffle", tr("Shuffle"), Action::Shuffle);
        let slides = ("slides", tr("Slideshow"), Action::Slideshow);
        return match kind_of_folder(Path::new(path)) {
            Some(FolderKind::Play) => {
                let all = ("play", tr("Play all"), Action::Play);
                Smart::of(all, &[all, shuffle])
            }
            Some(FolderKind::Slides) => Smart::of(slides, &[slides]),
            None => Smart::none(),
        };
    }
    if things.is_empty() || things.iter().any(|(_, folder)| *folder) {
        return Smart::none();
    }
    let names: Vec<String> = things
        .iter()
        .map(|(p, _)| file_name(Path::new(p)))
        .collect();
    let sorts: Vec<Sort> = names.iter().map(|n| sort_of(n)).collect();
    let archive = matches!(names.as_slice(), [one] if is_archive(one));
    smart_files(&sorts, archive, can)
}

impl App {
    /// The windows draw again when a folder read off the loop is in
    /// (`BELL`, `kind_of_folder`).
    pub(crate) fn filer_folder_bell(&mut self) {
        let Ok((ping, source)) = calloop::ping::make_ping() else {
            return;
        };
        let heard = self.loop_handle.insert_source(source, |_, _, app: &mut App| app.filer_redraw_all());
        if heard.is_ok() {
            let _ = BELL.set(ping);
        }
    }

    /// What `sel` (in window `win`) is for (the mockup's `smartFor`).
    pub(super) fn filer_smart(&self, win: &Win, sel: &[&Item]) -> Smart {
        if sel.is_empty() || win.in_bin() != InBin::No {
            return Smart::none();
        }
        let things: Vec<(&str, bool)> = sel
            .iter()
            .filter(|it| matches!(it.kind, Kind::File | Kind::Folder))
            .map(|it| (it.path.as_str(), it.kind == Kind::Folder))
            .collect();
        if things.len() != sel.len() {
            return Smart::none();
        }
        smart_for(&things, self.filer.printers.can)
    }

    /// What the folder window `win` shows is mostly (the button by the ↶):
    /// not in the bin or Recents. Read once a listing.
    pub(super) fn filer_here_kind(win: &mut Win) -> Option<FolderKind> {
        if win.in_bin() != InBin::No || win.dir == recent::root() {
            return None;
        }
        // (Read again once the listing is in: kept for it, by its number.)
        if let Some((_, kind)) = win
            .here_kind
            .filter(|(g, _)| *g == win.generation && !win.loading)
        {
            return kind;
        }
        let kind = folder_kind(
            win.all
                .iter()
                .filter(|it| it.kind == Kind::File)
                .map(|it| it.name.as_str()),
        );
        if !win.loading {
            win.here_kind = Some((win.generation, kind));
        }
        kind
    }

    /// An action by kind on window `id`'s selection (or, `…Here`, on the
    /// folder shown).
    pub(super) fn filer_smart_act(&mut self, id: u32, action: Action) {
        let Some(win) = self.filer.win(id) else {
            return;
        };
        let sel: Vec<&Item> = win
            .items
            .iter()
            .filter(|it| win.selected.contains(&it.path))
            .collect();
        // The files of the folder shown, in its order.
        let here: Vec<PathBuf> = win
            .items
            .iter()
            .filter(|it| it.kind == Kind::File)
            .map(|it| PathBuf::from(&it.path))
            .collect();
        let one_folder = match sel.as_slice() {
            [one] if one.kind == Kind::Folder => Some(PathBuf::from(&one.path)),
            _ => None,
        };
        let picked: Vec<PathBuf> = sel
            .iter()
            .filter(|it| it.kind == Kind::File)
            .map(|it| PathBuf::from(&it.path))
            .collect();
        // (Turning changes the photos: the zones' gate first.)
        if action == Action::Rotate
            && !self.filer_gate(id, super::Act::Rotate, &picked, None, super::Pending::Smart(action))
        {
            return;
        }
        self.smart_act(SendFor::Files(id), action, picked, here, one_folder);
    }

    /// What is printable now, as last asked (asked again when old): for
    /// the desktop's menus too.
    pub(crate) fn print_can(&mut self) -> Can {
        self.filer_printers(false);
        self.filer.printers.can
    }

    /// An action by kind for `whose` menu (a file window's, or the
    /// desktop's): on `picked` (the files, in order) or `one_folder`, with
    /// `here` the files of the place they are in, in its order. Played and
    /// shown by Quick Look from there; what is done kept in its history,
    /// what is said said in its way (a file window's pill, else quietly).
    pub(crate) fn smart_act(
        &mut self,
        whose: SendFor,
        action: Action,
        picked: Vec<PathBuf>,
        here: Vec<PathBuf>,
        one_folder: Option<PathBuf>,
    ) {
        // A folder's own files are read off the loop (on a phone that is
        // seconds: `kind_of_folder`), then played or shown as a selection.
        if let (Some(dir), Action::Play | Action::Shuffle | Action::Slideshow) = (&one_folder, action) {
            let dir = dir.clone();
            self.desktop_off_loop(
                move || files_in(&dir),
                move |app, files: Vec<PathBuf>| app.smart_act(whose, action, files.clone(), files, None),
            );
            return;
        }
        let peek = match whose {
            SendFor::Files(id) => id,
            SendFor::Desktop => crate::quicklook::DESKTOP,
        };
        let playable = |list: Vec<PathBuf>| -> Vec<PathBuf> {
            list.into_iter()
                .filter(|p| matches!(sort_of(&file_name(p)), Sort::Song | Sort::Video))
                .collect()
        };
        let photos = |list: Vec<PathBuf>| -> Vec<PathBuf> {
            list.into_iter()
                .filter(|p| sort_of(&file_name(p)) == Sort::Photo)
                .collect()
        };
        match action {
            Action::Play | Action::Shuffle | Action::PlayHere => {
                let mut list = match (action, one_folder) {
                    (Action::PlayHere, _) => playable(here),
                    (_, Some(dir)) => playable(files_in(&dir)),
                    _ => playable(picked),
                };
                if action == Action::Shuffle {
                    shuffle(&mut list);
                }
                self.peek_playlist(peek, list, 0);
            }
            Action::Slideshow | Action::SlideshowHere => {
                let (list, at) = match (action, one_folder, picked.as_slice()) {
                    (Action::SlideshowHere, ..) => (photos(here), 0),
                    (_, Some(dir), _) => (photos(files_in(&dir)), 0),
                    // One photo: "from here", in the place's order.
                    (_, None, [one]) => {
                        let list = photos(here);
                        let at = list.iter().position(|p| p == one).unwrap_or(0);
                        (list, at)
                    }
                    _ => (photos(picked), 0),
                };
                self.peek_slides(peek, list, at);
            }
            Action::SetWallpaper => {
                let Some(path) = picked.first().map(|p| p.to_string_lossy().into_owned()) else {
                    return;
                };
                let was = crate::controls::wallpaper_now();
                info!("files: {path} is the wallpaper (was {was:?})");
                let pick = path.clone();
                crate::wall_follow::picked_other(move || crate::controls::pick_wallpaper(pick));
                let name = file_name(Path::new(&path));
                self.smart_said(
                    whose,
                    fill(tr("“{name}” is your wallpaper"), &[("name", &name)]),
                    false,
                );
                if let Some(was) = was {
                    self.smart_did(whose, Undo::Wallpaper { was, now: path });
                }
            }
            Action::Rotate => {
                let photos = photos(picked);
                self.desktop_off_loop(
                    move || {
                        photos
                            .into_iter()
                            .filter(|p| rotate(p, 1))
                            .collect::<Vec<PathBuf>>()
                    },
                    move |app, turned: Vec<PathBuf>| {
                        if turned.is_empty() {
                            say_quietly(tr("This picture cannot be turned here").to_owned());
                            return;
                        }
                        for p in &turned {
                            app.thumb_stale(&p.to_string_lossy());
                            app.peek_forget(p);
                        }
                        app.smart_did(whose, Undo::Rotated(turned));
                        match whose {
                            SendFor::Files(id) => app.filer_list(id),
                            SendFor::Desktop => app.reload_desktop(),
                        }
                    },
                );
            }
            Action::Print => {
                let can = self.print_can();
                let files: Vec<PathBuf> = picked
                    .into_iter()
                    .filter(|p| printable(sort_of(&file_name(p)), can))
                    .collect();
                if files.is_empty() {
                    return;
                }
                // (LibreOffice takes a while to make a PDF: said at once.)
                if files.iter().any(|p| sort_of(&file_name(p)) == Sort::Office) {
                    self.smart_said(whose, tr("Getting ready to print…").to_owned(), false);
                }
                self.desktop_off_loop(
                    move || super::print::print(&files),
                    move |app, (said, failed): (Option<String>, Option<String>)| {
                        info!("files: print: {said:?}, {failed:?}");
                        if let Some(said) = said {
                            app.smart_said(whose, said, true);
                        }
                        if let Some(failed) = failed {
                            say_quietly(failed);
                        }
                    },
                );
            }
            _ => {}
        }
    }

    /// Say what was done by kind: in a file window's pill, else quietly
    /// (`always`: a notice the desktop gives too — the desktop has no pill,
    /// and says only what is worth a notice).
    fn smart_said(&mut self, whose: SendFor, said: String, always: bool) {
        match whose {
            SendFor::Files(id) => self.filer_flash(id, said),
            SendFor::Desktop if always => say_quietly(said),
            SendFor::Desktop => {}
        }
    }

    /// Keep what was done by kind in the history of `whose` menu.
    fn smart_did(&mut self, whose: SendFor, undo: Undo) {
        match whose {
            SendFor::Files(id) => self.filer_did(id, undo),
            SendFor::Desktop => {
                let said = super::history::said_of(&undo);
                self.desktop_did(undo, said);
            }
        }
    }
}

/// Put `list` in a random order (a shuffle, from the clock).
fn shuffle(list: &mut [PathBuf]) {
    let mut seed = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(1, |d| d.as_nanos() as u64)
        | 1;
    for i in (1..list.len()).rev() {
        // xorshift
        seed ^= seed << 13;
        seed ^= seed >> 7;
        seed ^= seed << 17;
        list.swap(i, (seed % (i as u64 + 1)) as usize);
    }
}

/// The EXIF orientation a picture shown as `o` takes when it is turned a
/// quarter clockwise.
pub(crate) fn turned(o: u16) -> u16 {
    match o {
        1 => 6,
        6 => 3,
        3 => 8,
        8 => 1,
        2 => 7,
        7 => 4,
        4 => 5,
        5 => 2,
        _ => 6,
    }
}

/// Turn the picture at `path` a quarter clockwise `quarters` times, NEVER
/// losing anything: a JPEG by its orientation tag only (`exiftool`, else
/// `jpegtran` — the pixels themselves, losslessly), a PNG drawn again (PNG
/// keeps every pixel) with what else it carried (`png_carry`). A file that
/// is not what its ending says, or a moving PNG (it would come out as its
/// first frame), is left as it is. Whether it was turned.
pub(crate) fn rotate(path: &Path, quarters: u32) -> bool {
    let end = file_name(path)
        .rsplit_once('.')
        .map(|(_, e)| e.to_lowercase())
        .unwrap_or_default();
    let Ok(bytes) = std::fs::read(path) else {
        return false;
    };
    match end.as_str() {
        // (A web picture saved as ".jpg" is no JPEG: its "orientation" read
        // as 1 and the undo turned it the wrong way.)
        "jpg" | "jpeg" if bytes.starts_with(&[0xFF, 0xD8, 0xFF]) => {
            if crate::launch::on_path("exiftool") {
                let mut o = crate::peek_kinds::jpeg_orientation_tag(path);
                for _ in 0..quarters {
                    o = turned(o);
                }
                return std::process::Command::new("exiftool")
                    .args(["-q", "-overwrite_original", "-n"])
                    .arg(format!("-Orientation={o}"))
                    .arg(path)
                    .stdout(std::process::Stdio::null())
                    .stderr(std::process::Stdio::null())
                    .status()
                    .is_ok_and(|s| s.success());
            }
            if crate::launch::on_path("jpegtran") {
                let tmp = beside(path);
                let made = std::process::Command::new("jpegtran")
                    .args(["-copy", "all", "-perfect", "-rotate"])
                    .arg((90 * quarters % 360).to_string())
                    .arg("-outfile")
                    .arg(&tmp)
                    .arg(path)
                    .status()
                    .is_ok_and(|s| s.success());
                return made && put_over(&tmp, path);
            }
            false
        }
        "png" if png_chunks(&bytes).iter().any(|(kind, _)| kind == b"IHDR") => {
            if png_chunks(&bytes).iter().any(|(kind, _)| kind == b"acTL") {
                return false;
            }
            let Ok(img) = image::load_from_memory_with_format(&bytes, image::ImageFormat::Png) else {
                return false;
            };
            let img = match quarters % 4 {
                1 => img.rotate90(),
                2 => img.rotate180(),
                3 => img.rotate270(),
                _ => img,
            };
            let mut made = Vec::new();
            if img.write_to(&mut std::io::Cursor::new(&mut made), image::ImageFormat::Png).is_err() {
                return false;
            }
            let tmp = beside(path);
            std::fs::write(&tmp, png_carry(&bytes, &made, quarters % 2 == 1)).is_ok() && put_over(&tmp, path) || {
                let _ = std::fs::remove_file(&tmp);
                false
            }
        }
        _ => false,
    }
}

/// A name beside `path` for what is being made of it that nothing has: a
/// hidden one, never a name of the owner's (`photo.turning.jpg` was one —
/// written over, then renamed over the photo — and two turns at once wrote
/// into the same file).
fn beside(path: &Path) -> PathBuf {
    static N: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
    let dir = path.parent().unwrap_or(Path::new("."));
    let name = file_name(path);
    loop {
        let n = N.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let tmp = dir.join(format!(".{name}.turning-{}-{n}", std::process::id()));
        if std::fs::symlink_metadata(&tmp).is_err() {
            return tmp;
        }
    }
}

/// `tmp` (what was made) in the place of `path`, with `path`'s permissions
/// (a private picture stays private); `tmp` is gone either way.
fn put_over(tmp: &Path, path: &Path) -> bool {
    let kept = std::fs::metadata(path).and_then(|m| std::fs::set_permissions(tmp, m.permissions()));
    let done = kept.is_ok() && std::fs::rename(tmp, path).is_ok();
    if !done {
        let _ = std::fs::remove_file(tmp);
    }
    done
}

/// A PNG's chunks after its signature: each one's kind and the whole of it
/// (length, kind, data, check). Nothing for what is not a PNG.
fn png_chunks(b: &[u8]) -> Vec<([u8; 4], &[u8])> {
    let mut out = Vec::new();
    if !b.starts_with(b"\x89PNG\r\n\x1a\n") {
        return out;
    }
    let mut i = 8;
    while let (Some(len), Some(kind)) = (b.get(i..i + 4), b.get(i + 4..i + 8)) {
        let (Ok(len), Ok(kind)) = (<[u8; 4]>::try_from(len), <[u8; 4]>::try_from(kind)) else {
            break;
        };
        let end = i + 12 + u32::from_be_bytes(len) as usize;
        let Some(chunk) = b.get(i..end) else {
            break;
        };
        out.push((kind, chunk));
        i = end;
        if &kind == b"IEND" {
            break;
        }
    }
    out
}

/// One PNG chunk: its length, kind, data and check (CRC-32 of kind + data).
fn png_chunk(kind: &[u8; 4], data: &[u8]) -> Vec<u8> {
    let mut crc = 0xFFFF_FFFFu32;
    for &byte in kind.iter().chain(data) {
        crc ^= u32::from(byte);
        for _ in 0..8 {
            crc = if crc & 1 != 0 { 0xEDB8_8320 ^ (crc >> 1) } else { crc >> 1 };
        }
    }
    let mut out = Vec::with_capacity(data.len() + 12);
    out.extend_from_slice(&(data.len() as u32).to_be_bytes());
    out.extend_from_slice(kind);
    out.extend_from_slice(data);
    out.extend_from_slice(&(!crc).to_be_bytes());
    out
}

/// The PNG `made` again from `orig`'s pixels, with what else `orig` carried
/// that a new encoding leaves out: how its colours are meant (iCCP, sRGB,
/// gAMA, cHRM — a Display P3 screenshot shifted colour without), what is
/// written in it (tEXt, zTXt, iTXt, eXIf, tIME) and a pixel's size (pHYs,
/// its two sides swapped when `sideways`) — right after the header, where
/// they belong (before the image data). What the encoder wrote itself stays.
fn png_carry(orig: &[u8], made: &[u8], sideways: bool) -> Vec<u8> {
    const KEPT: [&[u8; 4]; 10] = [b"iCCP", b"sRGB", b"gAMA", b"cHRM", b"tEXt", b"zTXt", b"iTXt", b"eXIf", b"tIME", b"pHYs"];
    let new = png_chunks(made);
    let Some(((_, header), rest)) = new.split_first().filter(|((kind, _), _)| kind == b"IHDR") else {
        return made.to_vec();
    };
    let mut out = Vec::with_capacity(made.len() + 4096);
    out.extend_from_slice(&made[..8]);
    out.extend_from_slice(header);
    for (kind, chunk) in png_chunks(orig) {
        if !KEPT.contains(&&kind) || new.iter().any(|(k, _)| *k == kind) {
            continue;
        }
        match (&kind, chunk.get(8..17)) {
            (b"pHYs", Some(d)) if sideways => out.extend(png_chunk(b"pHYs", &[&d[4..8], &d[0..4], &d[8..9]].concat())),
            _ => out.extend_from_slice(chunk),
        }
    }
    for (_, chunk) in rest {
        out.extend_from_slice(chunk);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A 2×1 PNG (red, blue) with `extra` chunks put after its header.
    fn png_with(extra: &[Vec<u8>]) -> Vec<u8> {
        let img = image::RgbaImage::from_fn(2, 1, |x, _| if x == 0 { image::Rgba([255, 0, 0, 255]) } else { image::Rgba([0, 0, 255, 255]) });
        let mut plain = Vec::new();
        image::DynamicImage::ImageRgba8(img).write_to(&mut std::io::Cursor::new(&mut plain), image::ImageFormat::Png).unwrap();
        let chunks = png_chunks(&plain);
        let mut out = plain[..8].to_vec();
        out.extend_from_slice(chunks[0].1);
        for e in extra {
            out.extend_from_slice(e);
        }
        for (_, c) in &chunks[1..] {
            out.extend_from_slice(c);
        }
        out
    }

    fn temp(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("wr-turn-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    #[test]
    fn a_png_turned_keeps_its_words_its_colours_and_who_may_read_it() {
        use std::os::unix::fs::PermissionsExt;
        let d = temp("png");
        let path = d.join("shot.png");
        let text = png_chunk(b"tEXt", b"parameters\0a cat, oil on canvas");
        let phys = png_chunk(b"pHYs", &[0, 0, 0x0B, 0x13, 0, 0, 0x16, 0x26, 1]);
        let srgb = png_chunk(b"sRGB", &[0]);
        std::fs::write(&path, png_with(&[srgb.clone(), phys, text.clone()])).unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600)).unwrap();
        assert!(rotate(&path, 1));
        let after = std::fs::read(&path).unwrap();
        let chunks = png_chunks(&after);
        assert!(chunks.iter().any(|(_, c)| *c == text.as_slice()), "the words stay");
        assert!(chunks.iter().any(|(_, c)| *c == srgb.as_slice()), "the colours stay");
        let phys = chunks.iter().find(|(k, _)| k == b"pHYs").unwrap().1;
        assert_eq!(&phys[8..17], &[0, 0, 0x16, 0x26, 0, 0, 0x0B, 0x13, 1], "a pixel's two sides swapped");
        let turned = image::load_from_memory(&after).unwrap().to_rgba8();
        assert_eq!(turned.dimensions(), (1, 2));
        assert_eq!(turned.get_pixel(0, 0).0, [255, 0, 0, 255], "red on top after a quarter clockwise");
        assert_eq!(std::fs::metadata(&path).unwrap().permissions().mode() & 0o777, 0o600);
        let left: Vec<_> = std::fs::read_dir(&d).unwrap().flatten().map(|e| e.file_name()).collect();
        assert_eq!(left.len(), 1, "nothing left beside it: {left:?}");
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn a_moving_png_or_a_picture_not_what_its_name_says_is_left_as_it_is() {
        let d = temp("left");
        let apng = d.join("sticker.png");
        let bytes = png_with(&[png_chunk(b"acTL", &[0, 0, 0, 2, 0, 0, 0, 0])]);
        std::fs::write(&apng, &bytes).unwrap();
        assert!(!rotate(&apng, 1));
        assert_eq!(std::fs::read(&apng).unwrap(), bytes);
        let webp = d.join("download.jpg");
        std::fs::write(&webp, b"RIFF\0\0\0\0WEBPVP8 ").unwrap();
        assert!(!rotate(&webp, 1));
        assert_eq!(std::fs::read(&webp).unwrap(), b"RIFF\0\0\0\0WEBPVP8 ");
        // The owner's own file of the old scratch name is never touched.
        std::fs::write(d.join("photo.turning.png"), b"mine").unwrap();
        std::fs::write(d.join("photo.png"), png_with(&[])).unwrap();
        assert!(rotate(&d.join("photo.png"), 1));
        assert_eq!(std::fs::read(d.join("photo.turning.png")).unwrap(), b"mine");
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn a_phone_or_the_network_is_a_slow_place_a_disk_or_a_stick_is_not() {
        let mounts = "/dev/nvme0n1p2 / ext4 rw 0 0\n\
                      gvfsd-fuse /run/user/1000/gvfs fuse.gvfsd-fuse rw 0 0\n\
                      /dev/sda1 /run/media/max/My\\040Stick exfat rw 0 0\n\
                      server:/srv /mnt/nas nfs4 rw 0 0\n";
        let slow = |p: &str| slow_in(mounts, Path::new(p)).is_some();
        assert!(slow("/run/user/1000/gvfs/mtp:host=Pixel/Internal shared storage/DCIM/Camera"));
        assert!(slow("/mnt/nas/photos"));
        assert!(!slow("/home/max/Pictures"));
        assert!(!slow("/run/media/max/My Stick/DCIM"), "a stick is read at once, as before");
        assert!(!slow("/run/user/1000/gvfsx"), "a name that only begins the same");
        assert_eq!(slow_in(mounts, Path::new("/run/user/1000/gvfs/mtp:host=P")), Some((PathBuf::from("/run/user/1000/gvfs"), true)));
        assert_eq!(slow_in(mounts, Path::new("/mnt/nas/a")), Some((PathBuf::from("/mnt/nas"), false)));
    }

    #[test]
    fn one_disk_is_told_by_the_mount_each_path_is_under() {
        let mounts = "/dev/nvme0n1p2 / ext4 rw 0 0\n\
                      /dev/nvme0n1p3 /home ext4 rw 0 0\n\
                      /dev/sda1 /run/media/max/My\\040Stick exfat rw 0 0\n";
        let on = |p: &str| mount_in(mounts, Path::new(p)).map(|m| m.0);
        assert_eq!(on("/home/max/a.txt"), on("/home/max/Pictures/not-there-yet"));
        assert_ne!(on("/home/max/a.txt"), on("/run/media/max/My Stick/a.txt"));
        assert_ne!(on("/home/max/a.txt"), on("/tmp/a.txt"), "another mount of one device is another disk to a rename");
        assert_eq!(on("/run/media/max/My Stick/DCIM"), Some("/run/media/max/My Stick".to_owned()));
    }

    #[test]
    fn a_folder_is_mostly_one_kind_at_seventy_in_a_hundred_of_three_or_more() {
        assert_eq!(
            folder_kind(["a.mp3", "b.flac"].into_iter()),
            None,
            "fewer than three"
        );
        assert_eq!(
            folder_kind(["a.mp3", "b.flac", "c.mkv"].into_iter()),
            Some(FolderKind::Play)
        );
        assert_eq!(
            folder_kind(["a.jpg", "b.png", "c.heic", "notes.txt"].into_iter()),
            Some(FolderKind::Slides)
        );
        assert_eq!(
            folder_kind(["a.jpg", "b.png", "c.txt", "d.txt"].into_iter()),
            None,
            "half is not mostly"
        );
    }

    /// The rules: a printer or none × LibreOffice or none × what is
    /// selected → the pill's one and the menu's.
    #[test]
    fn print_is_offered_only_where_it_can_happen() {
        use Action::*;
        let said = |names: &[&str], print: bool, office: bool| {
            let sorts: Vec<Sort> = names.iter().map(|n| sort_of(n)).collect();
            let archive = matches!(names, [one] if is_archive(one));
            let smart = smart_files(&sorts, archive, Can { print, office });
            let menu: Vec<Action> = smart.menu.iter().filter_map(Row::action).collect();
            (smart.bar.map(|(_, _, a)| a), menu)
        };
        for office in [false, true] {
            // No printer: no Print anywhere; a document's pill falls back
            // to Open with (nothing of its kind).
            for names in [
                &["letter.pdf"][..],
                &["notes.txt", "readme.md"],
                &["report.docx"],
                &["a.jpg"],
                &["a.jpg", "b.png"],
                &["letter.pdf", "a.jpg"],
                &["sheet.xlsx", "slides.pptx"],
            ] {
                let (bar, menu) = said(names, false, office);
                assert_ne!(bar, Some(Print), "{names:?}");
                assert!(!menu.contains(&Print), "{names:?}");
            }
            assert_eq!(said(&["letter.pdf"], false, office), (None, vec![]));
            assert_eq!(
                said(&["a.jpg"], false, office),
                (Some(SetWallpaper), vec![SetWallpaper, Rotate, Slideshow])
            );
            assert_eq!(
                said(&["a.jpg", "b.png"], false, office),
                (Some(Slideshow), vec![Slideshow, Rotate])
            );
            // A printer: text documents, photos, and the two mixed.
            for names in [
                &["letter.pdf"][..],
                &["notes.txt", "x.ps", "y.md"],
                &["letter.pdf", "a.jpg"],
            ] {
                assert_eq!(
                    said(names, true, office),
                    (Some(Print), vec![Print]),
                    "{names:?}"
                );
            }
            assert_eq!(
                said(&["a.jpg"], true, office),
                (Some(SetWallpaper), vec![SetWallpaper, Rotate, Print])
            );
            assert_eq!(
                said(&["a.jpg", "b.heic"], true, office),
                (Some(Slideshow), vec![Slideshow, Rotate, Print])
            );
            // Never for what cannot be printed, mixed in or alone.
            assert_eq!(
                said(&["letter.pdf", "song.mp3"], true, office),
                (None, vec![])
            );
            assert_eq!(said(&["song.mp3"], true, office), (Some(Play), vec![Play]));
            assert_eq!(said(&["photos.tar.gz"], true, office).0, Some(Extract));
        }
        // Office files: a printer AND LibreOffice.
        for names in [
            &["report.docx"][..],
            &["a.doc", "b.odt", "c.rtf"],
            &["d.xls", "e.xlsx", "f.ods"],
            &["g.ppt", "h.pptx", "i.odp"],
            &["report.docx", "a.jpg", "letter.pdf"],
        ] {
            assert_eq!(
                said(names, true, true),
                (Some(Print), vec![Print]),
                "{names:?}"
            );
            assert_eq!(said(names, true, false), (None, vec![]), "{names:?}");
            assert_eq!(said(names, false, true), (None, vec![]), "{names:?}");
        }
    }

    /// The same rules for things given by path (the desktop's selection):
    /// a folder of songs plays, a photo is for the wallpaper.
    #[test]
    fn things_by_path_are_for_what_their_kind_is_for() {
        let dir = std::env::temp_dir().join(format!("golem-smart-{}", std::process::id()));
        let _ = std::fs::create_dir_all(&dir);
        for n in ["a.mp3", "b.flac", "c.ogg"] {
            let _ = std::fs::write(dir.join(n), b"x");
        }
        let acts = |s: Smart| -> (Option<Action>, Vec<Action>) {
            let menu = s.menu.iter().filter_map(Row::action).collect();
            (s.bar.map(|(_, _, a)| a), menu)
        };
        let folder = dir.to_string_lossy().into_owned();
        assert_eq!(
            acts(smart_for(&[(folder.as_str(), true)], Can::default())),
            (Some(Action::Play), vec![Action::Play, Action::Shuffle])
        );
        let print = Can {
            print: true,
            office: false,
        };
        assert_eq!(
            acts(smart_for(&[("/d/photo.jpg", false)], print)).1,
            vec![Action::SetWallpaper, Action::Rotate, Action::Print]
        );
        // A folder among files, or nothing: nothing of a kind.
        assert_eq!(
            acts(smart_for(
                &[("/d/photo.jpg", false), (folder.as_str(), true)],
                print
            )),
            (None, vec![])
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_quarter_turn_goes_round_in_four() {
        for start in 1..=8 {
            let mut o = start;
            for _ in 0..4 {
                o = turned(o);
            }
            assert_eq!(o, start);
        }
        assert_eq!(turned(1), 6);
    }

    #[test]
    fn a_shuffle_keeps_everything() {
        let mut list: Vec<PathBuf> = (0..20)
            .map(|n| PathBuf::from(format!("/m/{n}.mp3")))
            .collect();
        let before = list.clone();
        shuffle(&mut list);
        let mut sorted = list.clone();
        sorted.sort();
        let mut want = before;
        want.sort();
        assert_eq!(sorted, want);
    }
}
