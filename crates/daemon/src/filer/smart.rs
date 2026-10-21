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
#[derive(Debug)]
pub(crate) struct Smart {
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

impl App {
    /// What `sel` (in window `win`) is for (the mockup's `smartFor`).
    pub(super) fn filer_smart(&self, win: &Win, sel: &[&Item]) -> Smart {
        if sel.is_empty() || win.in_bin() != InBin::No {
            return Smart::none();
        }
        if let [one] = sel {
            if one.kind == Kind::Folder {
                let shuffle = ("shuffle", tr("Shuffle"), Action::Shuffle);
                let slides = ("slides", tr("Slideshow"), Action::Slideshow);
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
        let archive = matches!(sel, [one] if is_archive(&one.name));
        smart_files(&sorts, archive, self.filer.printers.can)
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
                let can = self.filer.printers.can;
                let files: Vec<PathBuf> = picked
                    .into_iter()
                    .filter(|p| printable(sort_of(&file_name(p)), can))
                    .collect();
                if files.is_empty() {
                    return;
                }
                // (LibreOffice takes a while to make a PDF: said at once.)
                if files.iter().any(|p| sort_of(&file_name(p)) == Sort::Office) {
                    self.filer_flash(id, tr("Getting ready to print…").to_owned());
                }
                self.desktop_off_loop(
                    move || super::print::print(&files),
                    move |app, (said, failed): (Option<String>, Option<String>)| {
                        info!("files: print: {said:?}, {failed:?}");
                        if let Some(said) = said {
                            app.filer_flash(id, said);
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
