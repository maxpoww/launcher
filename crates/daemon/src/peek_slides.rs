//! Files' "Play" and "Slideshow" (Max, 2026-10-21, mockup v51), through
//! Quick Look: a PLAYLIST plays its songs and videos one after another in
//! Quick Look's player (`peek_autoplay` starts each; its end — a song's
//! player gone, a video's last frame — brings the next); a SLIDESHOW shows
//! photos full screen on black, the next every `SLIDE`, fading in, with a
//! caption "name · 3 of 52 · ×" (`quicklook::slides_push`); the arrows or a
//! click turn it, Escape or its × end it.

use std::path::{Path, PathBuf};
use std::time::Duration;

use crate::content::Rect;
use crate::i18n::{fill, tr};
use crate::quicklook::Thing;
use crate::App;

/// How long a photo of a slideshow stays.
const SLIDE: Duration = Duration::from_millis(2800);

/// What plays, one after another (`at`: the one up), for file window `win`.
#[derive(Debug, Clone)]
pub(crate) struct Playlist {
    pub win: u32,
    pub list: Vec<PathBuf>,
    pub at: usize,
}

/// The photos of a slideshow (`at`: the one up); `gen` tells its clock
/// from an older one's.
#[derive(Debug, Clone)]
pub(crate) struct Slides {
    pub win: u32,
    pub list: Vec<PathBuf>,
    pub at: usize,
    pub gen: u64,
}

impl Slides {
    /// "name · 3 of 52".
    pub(crate) fn caption(&self, name: &str) -> String {
        let of = fill(
            tr("{n} of {all}"),
            &[("n", &(self.at + 1)), ("all", &self.list.len())],
        );
        format!("{name} · {of}")
    }
}

/// A thing for Quick Look that is not a window's icon: it grows from the
/// middle of the screen.
fn thing(path: &Path, of: Option<(usize, usize)>, screen: (f32, f32)) -> Thing {
    Thing {
        path: path.to_path_buf(),
        name: path
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default(),
        thumb: None,
        icon: Rect::new(screen.0 / 2.0 - 40.0, screen.1 / 2.0 - 40.0, 80.0, 80.0),
        of,
        said: None,
    }
}

impl App {
    fn peek_screen(&self) -> (f32, f32) {
        (
            self.desktop_top.size.0 as f32,
            self.desktop_top.size.1 as f32,
        )
    }

    /// PLAY `list` from `at` (file window `win`'s): each in Quick Look's
    /// player, the next when it ends.
    pub(crate) fn peek_playlist(&mut self, win: u32, list: Vec<PathBuf>, at: usize) {
        if at >= list.len() {
            return;
        }
        tracing::info!("quick look: playing {} thing(s)", list.len());
        self.peek.slides = None;
        self.peek.playlist = Some(Playlist { win, list, at });
        self.peek_list_show();
    }

    /// The playlist's thing that is up, shown (and playing once it is
    /// ready: `peek_autoplay`); the one after it prepared.
    fn peek_list_show(&mut self) {
        let Some(p) = self.peek.playlist.clone() else {
            return;
        };
        let screen = self.peek_screen();
        self.peek_show(
            p.win,
            thing(&p.list[p.at], Some((p.at + 1, p.list.len())), screen),
        );
        if let Some(next) = p.list.get(p.at + 1) {
            self.peek_want(vec![(next.clone(), 0)]);
        }
    }

    /// The thing that played ended: the next one, or (the last) the end.
    pub(crate) fn peek_list_next(&mut self) {
        let Some(p) = self.peek.playlist.as_mut() else {
            return;
        };
        if p.at + 1 >= p.list.len() {
            self.peek.playlist = None;
            return;
        }
        p.at += 1;
        self.peek_list_show();
    }

    /// A SLIDESHOW of `list` from `at` (file window `win`'s): full screen,
    /// the next every `SLIDE`.
    pub(crate) fn peek_slides(&mut self, win: u32, list: Vec<PathBuf>, at: usize) {
        if at >= list.len() {
            return;
        }
        tracing::info!(
            "quick look: a slideshow of {} photo(s) from {at}",
            list.len()
        );
        self.peek.playlist = None;
        let gen = self.peek.slides.as_ref().map_or(1, |s| s.gen + 1);
        self.peek.slides = Some(Slides { win, list, at, gen });
        self.peek_slide_show();
        let timer = calloop::timer::Timer::from_duration(SLIDE);
        let _ = self
            .loop_handle
            .insert_source(timer, move |_, _, app: &mut App| {
                if app.peek.slides.as_ref().is_some_and(|s| s.gen == gen) {
                    app.peek_slide_step(1);
                    calloop::timer::TimeoutAction::ToDuration(SLIDE)
                } else {
                    calloop::timer::TimeoutAction::Drop
                }
            });
    }

    /// The photo that is up, shown fading in from black; the next prepared.
    fn peek_slide_show(&mut self) {
        let Some(s) = self.peek.slides.clone() else {
            return;
        };
        let screen = self.peek_screen();
        self.peek_show(
            s.win,
            thing(&s.list[s.at], Some((s.at + 1, s.list.len())), screen),
        );
        if let Some(up) = self.peek.up.as_mut() {
            up.sharp_from_black();
        }
        let next = s.list[(s.at + 1) % s.list.len()].clone();
        self.peek_want(vec![(next, 0)]);
        self.request_desktop_top_draw();
    }

    /// The slideshow turned on (or back) by `by`, round.
    pub(crate) fn peek_slide_step(&mut self, by: isize) {
        let Some(s) = self.peek.slides.as_mut() else {
            return;
        };
        let n = s.list.len() as isize;
        s.at = (s.at as isize + by).rem_euclid(n.max(1)) as usize;
        self.peek_slide_show();
    }
}
