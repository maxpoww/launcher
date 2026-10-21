//! WHAT A KIND OF THING IS FOR (Max, 2026-10-21, mockup v51: "one action
//! per case on the pill, two, even 3 on the context menu; only actions that
//! just happen"). Songs and videos PLAY (and shuffle), photos make a
//! SLIDESHOW, one photo becomes the WALLPAPER or is TURNED, documents
//! PRINT. The pill shows one, first, in Open with's place; the menu up to
//! three, right after Open; a folder that is mostly songs or photos (70 %
//! of at least 3 files) gets its own by the ↶ (`filer_head`).
//!
//! Playing and the slideshow are Quick Look's (`peek_slides.rs`): what
//! is played goes through its player, a song after a song; the photos are
//! shown full screen on black, the next every 2.8 s.

use super::*;

/// What a file is, as its actions see it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Sort {
    Song,
    Video,
    Photo,
    Doc,
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
        _ => Sort::Other,
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

/// What a folder is mostly, read once (and again when it changes).
fn kind_of_folder(dir: &Path) -> Option<FolderKind> {
    type Seen = HashMap<PathBuf, (Option<std::time::SystemTime>, Option<FolderKind>)>;
    static SEEN: std::sync::Mutex<Option<Seen>> = std::sync::Mutex::new(None);
    let changed = std::fs::metadata(dir).and_then(|m| m.modified()).ok();
    let mut seen = SEEN.lock().unwrap_or_else(|e| e.into_inner());
    let seen = seen.get_or_insert_with(HashMap::new);
    if let Some((at, kind)) = seen.get(dir) {
        if *at == changed {
            return *kind;
        }
    }
    let files = files_in(dir);
    let kind = folder_kind(
        files
            .iter()
            .map(|p| p.file_name().and_then(|n| n.to_str()).unwrap_or("")),
    );
    if seen.len() > 200 {
        seen.clear();
    }
    seen.insert(dir.to_path_buf(), (changed, kind));
    kind
}

/// The actions a selection is for: the pill's one (its symbol, its words,
/// what it does) and the menu's.
pub(super) struct Smart {
    pub(super) bar: Option<(&'static str, &'static str, Action)>,
    pub(super) menu: Vec<Row>,
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

impl App {
    /// What `sel` (in window `win`) is for (the mockup's `smartFor`).
    pub(super) fn filer_smart(&self, win: &Win, sel: &[&Item]) -> Smart {
        if sel.is_empty() || win.in_bin() != InBin::No {
            return Smart::none();
        }
        let play = ("play", tr("Play"), Action::Play);
        let shuffle = ("shuffle", tr("Shuffle"), Action::Shuffle);
        let slides = ("slides", tr("Slideshow"), Action::Slideshow);
        if let [one] = sel {
            if one.kind == Kind::Folder {
                return match kind_of_folder(Path::new(&one.path)) {
                    Some(FolderKind::Play) => {
                        let all = ("play", tr("Play all"), Action::Play);
                        Smart::of(all, &[all, shuffle])
                    }
                    Some(FolderKind::Slides) => Smart::of(slides, &[slides]),
                    None => Smart::none(),
                };
            }
        }
        if sel.iter().any(|it| it.kind != Kind::File) {
            return Smart::none();
        }
        let sorts: Vec<Sort> = sel.iter().map(|it| sort_of(&it.name)).collect();
        let all = |f: fn(Sort) -> bool| sorts.iter().all(|s| f(*s));
        if all(|s| matches!(s, Sort::Song | Sort::Video)) {
            return if sel.len() > 1 {
                Smart::of(play, &[play, shuffle])
            } else {
                Smart::of(play, &[play])
            };
        }
        if all(|s| s == Sort::Photo) {
            let rotate = ("rotate", tr("Rotate"), Action::Rotate);
            return if sel.len() == 1 {
                let wall = ("pic", tr("Set as wallpaper"), Action::SetWallpaper);
                let here = ("slides", tr("Slideshow from here"), Action::Slideshow);
                Smart::of(wall, &[wall, rotate, here])
            } else {
                Smart::of(slides, &[slides, rotate])
            };
        }
        if all(|s| s == Sort::Doc) {
            let print = ("print", tr("Print"), Action::Print);
            return Smart::of(print, &[print]);
        }
        if let [one] = sel {
            if is_archive(&one.name) {
                return Smart {
                    bar: Some(("extract", tr("Extract here"), Action::Extract)),
                    menu: Vec::new(),
                };
            }
        }
        Smart::none()
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
                self.peek_playlist(id, list, 0);
            }
            Action::Slideshow | Action::SlideshowHere => {
                let (list, at) = match (action, one_folder, picked.as_slice()) {
                    (Action::SlideshowHere, ..) => (photos(here), 0),
                    (_, Some(dir), _) => (photos(files_in(&dir)), 0),
                    // One photo: "from here", in the folder's order.
                    (_, None, [one]) => {
                        let list = photos(here);
                        let at = list.iter().position(|p| p == one).unwrap_or(0);
                        (list, at)
                    }
                    _ => (photos(picked), 0),
                };
                self.peek_slides(id, list, at);
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
                self.filer_flash(
                    id,
                    fill(tr("“{name}” is your wallpaper"), &[("name", &name)]),
                );
                if let Some(was) = was {
                    self.filer_did(id, Undo::Wallpaper { was, now: path });
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
                        app.filer_did(id, Undo::Rotated(turned));
                        app.filer_list(id);
                    },
                );
            }
            Action::Print => {
                let docs: Vec<PathBuf> = picked
                    .into_iter()
                    .filter(|p| sort_of(&file_name(p)) == Sort::Doc)
                    .collect();
                self.desktop_off_loop(
                    move || print(&docs),
                    move |app, said: Result<String, String>| {
                        info!("files: print: {said:?}");
                        match said {
                            Ok(said) => app.filer_flash(id, said),
                            Err(said) => say_quietly(said),
                        }
                    },
                );
            }
            _ => {}
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
/// keeps every pixel). Whether it was turned.
pub(crate) fn rotate(path: &Path, quarters: u32) -> bool {
    let end = file_name(path)
        .rsplit_once('.')
        .map(|(_, e)| e.to_lowercase())
        .unwrap_or_default();
    match end.as_str() {
        "jpg" | "jpeg" => {
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
                let tmp = path.with_extension("turning.jpg");
                let ok = std::process::Command::new("jpegtran")
                    .args(["-copy", "all", "-perfect", "-rotate"])
                    .arg((90 * quarters % 360).to_string())
                    .arg("-outfile")
                    .arg(&tmp)
                    .arg(path)
                    .status()
                    .is_ok_and(|s| s.success());
                return ok && std::fs::rename(&tmp, path).is_ok() || {
                    let _ = std::fs::remove_file(&tmp);
                    false
                };
            }
            false
        }
        "png" => {
            let Ok(img) = image::open(path) else {
                return false;
            };
            let img = match quarters % 4 {
                1 => img.rotate90(),
                2 => img.rotate180(),
                3 => img.rotate270(),
                _ => img,
            };
            let tmp = path.with_extension("turning.png");
            img.save_with_format(&tmp, image::ImageFormat::Png).is_ok()
                && std::fs::rename(&tmp, path).is_ok()
        }
        _ => false,
    }
}

/// Print `docs` on the default printer (`lp`): what to say, or why not.
fn print(docs: &[PathBuf]) -> Result<String, String> {
    let none = || Err(tr("No printer is set up").to_owned());
    if docs.is_empty() || !crate::launch::on_path("lp") {
        return none();
    }
    let default = std::process::Command::new("lpstat")
        .arg("-d")
        .output()
        .ok()
        .map(|o| String::from_utf8_lossy(&o.stdout).into_owned())
        .unwrap_or_default();
    if !default.contains(':') || default.contains("no system default") {
        return none();
    }
    let printed = docs
        .iter()
        .filter(|d| {
            std::process::Command::new("lp")
                .arg(d)
                .status()
                .is_ok_and(|s| s.success())
        })
        .count();
    match (printed, docs) {
        (0, _) => Err(tr("It could not be printed").to_owned()),
        (_, [one]) => Ok(fill(tr("Printing “{name}”"), &[("name", &file_name(one))])),
        (n, _) => Ok(fill(tr("Printing {n} documents"), &[("n", &n)])),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
