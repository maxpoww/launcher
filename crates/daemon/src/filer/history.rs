//! THE UNDO BUTTON AND ITS HISTORY (Max, 2026-10-21: the mockup's v47,
//! "good build it"). Each file window keeps what was done in it — a name,
//! a move, a copy, a throwing away, a new folder (`Undo`) — newest last,
//! for as long as the window is open. The top line's ↶ is there only
//! while something can be taken back (it slides out, and away); a click
//! (or Ctrl+Z) takes back the newest that still can be. A right click on
//! it opens the HISTORY under it (drawn by `chrome::filer_history`):
//! everything done here, newest first, each with its Undo — an older one
//! too, the newer staying as they are — "Undone" once taken back (its
//! words struck through), "Can't undo" when the world moved on since (the
//! file renamed again, gone from the bin, its old place taken, the new
//! folder no longer empty). The menu stays up for another, and goes with
//! the last thing there is to take back.
//!
//! Undoing never throws anything away (`take_back`'s rules): copies go to
//! the bin, a new folder goes only while empty, nothing comes back over
//! what took its place. A delete for good is not here: it cannot be undone.

use super::*;

/// The most a window keeps.
const KEEP: usize = 100;

/// One thing done in a window.
pub(super) struct Did {
    /// Its number in the window (what a reading of the disk answers to).
    n: u64,
    pub(super) undo: Undo,
    /// What the history says of it, and its symbol (a chrome glyph).
    pub(super) text: String,
    pub(super) sign: &'static str,
    /// The folder it was done in (its name), and when.
    pub(super) place: String,
    pub(super) at: Instant,
    /// Taken back already; still possible to take back (as last read).
    pub(super) done: bool,
    pub(super) can: bool,
}

/// What throwing `names` away is called.
fn binned_text(names: &[String]) -> String {
    match names {
        [one] => fill(tr("Moved “{name}” to the Bin"), &[("name", one)]),
        _ => fill(tr("Moved {n} items to the Bin"), &[("n", &names.len())]),
    }
}

/// What a thing done is called in the history, and its symbol.
/// What `undo` is called (the desktop's history keeps it by its words).
pub(super) fn said_of(undo: &Undo) -> String {
    describe(undo).0
}

fn describe(undo: &Undo) -> (String, &'static str) {
    let folder = |p: &Path| p.parent().map(file_name).unwrap_or_default();
    match undo {
        Undo::Renamed { from, to } => (
            fill(
                tr("Renamed “{old}” to “{new}”"),
                &[("old", &file_name(from)), ("new", &file_name(to))],
            ),
            "rename",
        ),
        Undo::Binned(names) => (binned_text(names), "trash"),
        Undo::Made(path) => (
            fill(tr("New folder “{name}”"), &[("name", &file_name(path))]),
            "newf",
        ),
        Undo::Moved(pairs) => {
            let to = pairs
                .first()
                .map(|(_, now)| folder(now))
                .unwrap_or_default();
            let text = match pairs.as_slice() {
                [(from, _)] => fill(
                    tr("Moved “{name}” to {to}"),
                    &[("name", &file_name(from)), ("to", &to)],
                ),
                _ => fill(
                    tr("Moved {n} items to {to}"),
                    &[("n", &pairs.len()), ("to", &to)],
                ),
            };
            (text, "open")
        }
        Undo::Wallpaper { now, .. } => (
            fill(
                tr("Wallpaper: “{name}”"),
                &[("name", &file_name(Path::new(now)))],
            ),
            "pic",
        ),
        Undo::Rotated(paths) => (
            match paths.as_slice() {
                [one] => fill(tr("Rotated “{name}”"), &[("name", &file_name(one))]),
                _ => fill(tr("Rotated {n} items"), &[("n", &paths.len())]),
            },
            "rotate",
        ),
        Undo::Default { kind, app, .. } => (crate::other_apps::said_as(kind, app), "app"),
        Undo::Grouped { folder, moved } => (
            fill(
                tr("Put {n} items in “{name}”"),
                &[("n", &moved.len()), ("name", &file_name(folder))],
            ),
            "newf",
        ),
        Undo::Copied(copies) => {
            let to = copies.first().map(|c| folder(c)).unwrap_or_default();
            let text = match copies.as_slice() {
                [one] => fill(
                    tr("Copied “{name}” to {to}"),
                    &[("name", &file_name(one)), ("to", &to)],
                ),
                _ => fill(
                    tr("Copied {n} items to {to}"),
                    &[("n", &copies.len()), ("to", &to)],
                ),
            };
            (text, "copy")
        }
    }
}

/// Whether `undo` can still be taken back, the disk as it is now (`bin`:
/// what the bin holds) — what `take_back` asks before it acts.
pub(super) fn can_undo(undo: &Undo, bin: &[crate::trash::TrashItem]) -> bool {
    match undo {
        Undo::Renamed { from, to } => to.exists() && !from.exists(),
        Undo::Moved(pairs) => pairs
            .iter()
            .any(|(from, now)| now.exists() && !from.exists()),
        Undo::Copied(copies) => copies.iter().any(|c| c.exists()),
        Undo::Binned(names) => names.iter().any(|n| {
            bin.iter()
                .any(|t| &t.name == n && !t.original_path.exists())
        }),
        Undo::Made(path) => std::fs::read_dir(path).is_ok_and(|mut d| d.next().is_none()),
        Undo::Grouped { moved, .. } => moved
            .iter()
            .any(|(from, now)| now.exists() && !from.exists()),
        Undo::Wallpaper { now, .. } => {
            crate::controls::wallpaper_now().as_deref() == Some(now.as_str())
        }
        Undo::Rotated(paths) => paths.iter().any(|p| p.exists()),
        // (The owner's own choice, read from its file: no program asked.)
        Undo::Default { mime, now, .. } => {
            crate::other_apps::chosen_default(mime).as_deref() == Some(now.as_str())
        }
    }
}

/// "just now", "3 min ago", "1 h ago".
pub(super) fn ago(at: Instant) -> String {
    let s = at.elapsed().as_secs();
    if s < 45 {
        tr("just now").to_owned()
    } else if s < 3600 {
        fill(tr("{n} min ago"), &[("n", &(s / 60).max(1))])
    } else {
        fill(tr("{n} h ago"), &[("n", &(s / 3600))])
    }
}

/// The history as the rig reads it: the ↶ and the menu, then a line a
/// row, newest first.
pub(super) fn state(win: &Win) -> String {
    let rows: Vec<String> = win
        .history
        .iter()
        .rev()
        .enumerate()
        .map(|(row, d)| {
            let what = if d.done {
                "undone"
            } else if d.can {
                "undo"
            } else {
                "can't"
            };
            format!("{row}: [{what}] {} · {}", d.text, d.place)
        })
        .collect();
    format!(
        "button {} · menu {}\n{}",
        if win.undoable() { "out" } else { "away" },
        if win.hist_open { "up" } else { "down" },
        rows.join("\n")
    )
}

/// Say quietly what was taken back: a passing notice, low and not kept
/// with the others (the history says it already).
pub(crate) fn say_quietly(body: String) {
    let title = tr("Files");
    std::thread::spawn(move || {
        let _ = std::process::Command::new("busctl")
            .args([
                "--user",
                "call",
                "org.freedesktop.Notifications",
                "/org/freedesktop/Notifications",
                "org.freedesktop.Notifications",
                "Notify",
                "susssasa{sv}i",
                "waverunner",
                "0",
                "folder",
                title,
                &body,
                "0",
                "2",
                "transient",
                "b",
                "true",
                "urgency",
                "y",
                "0",
                "2500",
            ])
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status();
    });
}

impl Win {
    /// Whether anything done here can still be taken back.
    pub(super) fn undoable(&self) -> bool {
        self.history.iter().any(|d| !d.done && d.can)
    }
}

impl App {
    /// Something was done in window `id`: it goes in its history.
    pub(crate) fn filer_did(&mut self, id: u32, undo: Undo) {
        let (text, sign) = describe(&undo);
        self.filer_did_said(id, undo, text, sign);
    }

    /// Things were thrown away from window `id`: `names` in the bin,
    /// `shown` the names they had.
    pub(super) fn filer_binned(&mut self, id: u32, names: Vec<String>, shown: &[String]) {
        let text = binned_text(shown);
        self.filer_did_said(id, Undo::Binned(names), text, "trash");
    }

    pub(super) fn filer_did_said(&mut self, id: u32, undo: Undo, text: String, sign: &'static str) {
        let Some(win) = self.filer.win_mut(id) else {
            return;
        };
        let n = win.history.last().map_or(0, |d| d.n + 1);
        win.history.push(Did {
            n,
            undo,
            text,
            sign,
            place: super::chrome::tab_name(&win.dir, &win.places),
            at: Instant::now(),
            done: false,
            can: true,
        });
        if win.history.len() > KEEP {
            win.history.remove(0);
        }
        self.filer_hover(id);
        self.filer_request_draw(id);
        self.filer_history_check(id);
    }

    /// Read again — off the loop: a stick or a phone can be slow — which
    /// things done in window `id` can still be taken back (the ↶ is out
    /// while one can; the history says it of each).
    pub(super) fn filer_history_check(&mut self, id: u32) {
        let Some(win) = self.filer.win(id) else {
            return;
        };
        let ask: Vec<(u64, Undo)> = win
            .history
            .iter()
            .filter(|d| !d.done)
            .map(|d| (d.n, d.undo.clone()))
            .collect();
        if ask.is_empty() {
            return;
        }
        self.desktop_off_loop(
            move || {
                let bin = if ask.iter().any(|(_, u)| matches!(u, Undo::Binned(_))) {
                    crate::trash::Trash::home().list()
                } else {
                    Vec::new()
                };
                ask.into_iter()
                    .map(|(n, u)| (n, can_undo(&u, &bin)))
                    .collect::<Vec<_>>()
            },
            move |app, read: Vec<(u64, bool)>| {
                let Some(win) = app.filer.win_mut(id) else {
                    return;
                };
                let mut changed = false;
                for (n, can) in read {
                    if let Some(d) = win.history.iter_mut().find(|d| d.n == n) {
                        changed |= d.can != can;
                        d.can = can;
                    }
                }
                if win.hist_open && !win.undoable() {
                    win.hist_open = false;
                    changed = true;
                }
                if changed {
                    app.filer_hover(id);
                    app.filer_request_draw(id);
                }
            },
        );
    }

    /// Ctrl+Z, or the ↶: take back the newest thing done in window `id`
    /// that still can be.
    pub(super) fn filer_undo(&mut self, id: u32) {
        let k = self
            .filer
            .win(id)
            .and_then(|w| w.history.iter().rposition(|d| !d.done && d.can));
        match k {
            Some(k) => self.filer_undo_at(id, k),
            None => info!("files: nothing to undo in window {id}"),
        }
    }

    /// Take back thing `k` of window `id`'s history (an older one too: the
    /// newer stay as they are). Off the loop — a move back across disks is
    /// a copy.
    pub(super) fn filer_undo_at(&mut self, id: u32, k: usize) {
        let Some(win) = self.filer.win_mut(id) else {
            return;
        };
        let Some(did) = win.history.get_mut(k).filter(|d| !d.done && d.can) else {
            return;
        };
        // (Marked at once: the row says "Undone" and the ↶ moves on to the
        // one before; if it could not be done after all, it is undone no
        // more and a notice says so.)
        did.done = true;
        let (n, undo) = (did.n, did.undo.clone());
        info!("files: undo {undo:?}");
        if !win.undoable() {
            win.hist_open = false;
        }
        self.filer_hover(id);
        self.filer_request_draw(id);
        self.desktop_off_loop(
            move || take_back(undo),
            move |app, said: Option<String>| {
                match said {
                    Some(said) => say_quietly(said),
                    None => {
                        if let Some(d) = app
                            .filer
                            .win_mut(id)
                            .and_then(|w| w.history.iter_mut().find(|d| d.n == n))
                        {
                            d.done = false;
                        }
                        let said = tr("That could not be undone").to_owned();
                        std::thread::spawn(move || crate::desktop_send_notify(&said));
                    }
                }
                app.refilter();
                let ids: Vec<u32> = app.filer.wins.iter().map(|w| w.id).collect();
                for w in ids {
                    app.filer_list(w);
                }
            },
        );
    }

    /// The history opened (a right click on the ↶), or put away.
    pub(super) fn filer_history_toggle(&mut self, id: u32) {
        let Some(win) = self.filer.win_mut(id) else {
            return;
        };
        win.hist_open = !win.hist_open && win.undoable();
        if win.hist_open {
            win.hist_first = 0.0;
            win.menu = None;
            win.menu_path = None;
            win.props = None;
            self.filer_history_check(id);
        }
        self.filer_hover(id);
        self.filer_request_draw(id);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_line_wears_a_symbol_there_is() {
        let p = |s: &str| PathBuf::from(s);
        let all = [
            Undo::Renamed {
                from: p("/h/a"),
                to: p("/h/b"),
            },
            Undo::Moved(vec![(p("/h/a"), p("/h/d/a"))]),
            Undo::Copied(vec![p("/h/a")]),
            Undo::Binned(vec!["a".into()]),
            Undo::Made(p("/h/d")),
            Undo::Grouped {
                folder: p("/h/d"),
                moved: Vec::new(),
            },
            Undo::Wallpaper {
                was: String::new(),
                now: "/h/a.jpg".into(),
            },
            Undo::Rotated(vec![p("/h/a.jpg")]),
            Undo::Default {
                mime: "image/png".into(),
                kind: "PNG images".into(),
                app: "GIMP".into(),
                was: None,
                now: "gimp.desktop".into(),
            },
        ];
        for undo in all {
            let sign = describe(&undo).1;
            assert!(
                crate::glyphs::GLYPHS.iter().any(|(n, _)| *n == sign),
                "no symbol {sign}"
            );
        }
    }

    #[test]
    fn what_was_done_is_said_plainly() {
        let p = |s: &str| PathBuf::from(s);
        let said = |u: Undo| describe(&u).0;
        assert_eq!(
            describe(&Undo::Renamed {
                from: p("/h/README.txt"),
                to: p("/h/notes.txt")
            }),
            ("Renamed “README.txt” to “notes.txt”".to_owned(), "rename")
        );
        assert_eq!(
            said(Undo::Binned(vec!["Golem.png".into()])),
            "Moved “Golem.png” to the Bin"
        );
        assert_eq!(
            said(Undo::Binned(vec!["a".into(), "b".into(), "c".into()])),
            "Moved 3 items to the Bin"
        );
        assert_eq!(
            said(Undo::Made(p("/h/untitled folder"))),
            "New folder “untitled folder”"
        );
        assert_eq!(
            said(Undo::Moved(vec![(p("/h/x"), p("/h/Downloads/x"))])),
            "Moved “x” to Downloads"
        );
        assert_eq!(
            said(Undo::Copied(vec![p("/h/Pictures/a"), p("/h/Pictures/b")])),
            "Copied 2 items to Pictures"
        );
    }

    #[test]
    fn what_can_be_taken_back_is_read_from_the_disk() {
        let dir = std::env::temp_dir().join(format!("waverunner-history-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        // A new folder only while it is empty.
        assert!(can_undo(&Undo::Made(dir.clone()), &[]));
        std::fs::write(dir.join("a"), "x").unwrap();
        assert!(!can_undo(&Undo::Made(dir.clone()), &[]));
        // A name back only while the new one is there and the old one free.
        let (a, b) = (dir.join("a"), dir.join("b"));
        let renamed = Undo::Renamed {
            from: a.clone(),
            to: b.clone(),
        };
        assert!(!can_undo(&renamed, &[]));
        std::fs::rename(&a, &b).unwrap();
        assert!(can_undo(&renamed, &[]));
        std::fs::write(&a, "taken").unwrap();
        assert!(!can_undo(&renamed, &[]));
        // Thrown away: only while it is in the bin and its place is free.
        let thrown = |original: &Path| crate::trash::TrashItem {
            name: "b".into(),
            original_path: original.to_path_buf(),
            deleted_at: String::new(),
            is_dir: false,
        };
        let binned = Undo::Binned(vec!["b".into()]);
        assert!(!can_undo(&binned, &[]));
        assert!(!can_undo(&binned, &[thrown(&a)]));
        assert!(can_undo(&binned, &[thrown(&dir.join("free"))]));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn times_are_said_roughly() {
        assert_eq!(ago(Instant::now()), "just now");
        let ten = Instant::now() - Duration::from_secs(600);
        assert_eq!(ago(ten), "10 min ago");
        let two = Instant::now() - Duration::from_secs(7300);
        assert_eq!(ago(two), "2 h ago");
    }
}
