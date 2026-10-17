//! A VIDEO or a GIF PLAYS IN ITS TILE while the pointer rests on it (Max,
//! 2026-10-17: "show a sec of the live videos and gifs on the files… as
//! android do", then: under the pointer, and "it should keep playing until
//! the user moves the pointer away").
//!
//! The pointer held still on one `REST` → Quick Look's own player
//! (`peek_kinds::stream`) at the tile's way (`Way::TILE`: fifteen frames a
//! second, silent, round and round, each frame the picture centred on the
//! tile's clear square, a big video on the graphics chip) — its frames go
//! straight into the texture layer the thumbnail is in. The pointer goes
//! elsewhere, or the window closes: the player is stopped and the still
//! thumbnail goes back into its layer. Nothing is made ahead and nothing is
//! kept: one decoder runs, only while something is looked at.
//!
//! An animated WEBP (ffmpeg cannot read one) is read whole in-process, as
//! Quick Look reads it (`peek_kinds::animation`, at the tile's size, on a
//! thread), and played on the loop's clock at its own frames' delays.

use std::sync::Arc;
use std::time::{Duration, Instant};

use smithay_client_toolkit::reexports::calloop;

use crate::peek_kinds::{Anim, Stream, Way};
use crate::App;

/// How long the pointer rests on a tile before it plays (passing across
/// a folder starts nothing).
const REST: Duration = Duration::from_millis(250);

#[derive(Default)]
pub(crate) struct Live {
    /// The window and the thing the pointer rests on, waiting out `REST`
    /// (the counter drops a wait that was overtaken).
    wish: Option<(u32, String)>,
    wish_no: u64,
    playing: Option<Playing>,
    frames: Option<calloop::channel::Sender<(u64, Vec<u8>)>>,
    /// A WebP's frames, read off the loop (`None`: it does not move).
    anims: Option<calloop::channel::Sender<(u64, Option<Arc<Anim>>)>>,
    tag: u64,
}

struct Playing {
    win: u32,
    path: String,
    /// The picture key whose layer it plays in.
    key: String,
    tag: u64,
    /// The decoder (a video, a GIF), while it runs.
    _stream: Option<Stream>,
    /// A WebP's frames (each on the tile's square), since when, and the
    /// one shown.
    anim: Option<(Arc<Anim>, Instant, usize)>,
}

/// Whether a file of this name moves: a video, a GIF.
fn moves(name: &str) -> bool {
    let lower = name.to_ascii_lowercase();
    // (The usual endings too, where no MIME database is installed.)
    [
        ".gif", ".webp", ".mp4", ".m4v", ".mkv", ".webm", ".mov", ".avi", ".ogv", ".3gp",
    ]
    .iter()
    .any(|e| lower.ends_with(e))
        || crate::thumb_more::mime_of(&lower).is_some_and(|m| m.starts_with("video/"))
}

impl App {
    /// The pointer moved in window `id`: what is under it plays (after a
    /// rest), what it left stops.
    pub(crate) fn filer_live_hover(&mut self, id: u32) {
        // (Only what wears its own thumbnail: a carrier icon has no frames.)
        let under = self
            .filer_hovered(id)
            .filter(|(_, name, key)| moves(name) && key.starts_with("thumb:"));
        let live = &self.filer.live;
        let same = |w: u32, p: &str| {
            under
                .as_ref()
                .is_some_and(|(path, _, _)| w == id && p == path)
        };
        if live.playing.as_ref().is_some_and(|p| !same(p.win, &p.path)) {
            self.filer_live_stop();
        }
        let live = &mut self.filer.live;
        let Some((path, _, _)) = under else {
            live.wish = None;
            return;
        };
        if live.playing.is_some()
            || live
                .wish
                .as_ref()
                .is_some_and(|(w, p)| *w == id && *p == path)
        {
            return;
        }
        live.wish = Some((id, path));
        live.wish_no += 1;
        let no = live.wish_no;
        let timer = calloop::timer::Timer::from_duration(REST);
        let _ = self
            .loop_handle
            .insert_source(timer, move |_, _, app: &mut App| {
                app.filer_live_start(no);
                calloop::timer::TimeoutAction::Drop
            });
    }

    /// The rest is over: if the pointer is still there, it plays.
    fn filer_live_start(&mut self, no: u64) {
        if self.filer.live.wish_no != no || self.filer.live.playing.is_some() {
            return;
        }
        let Some((id, path)) = self.filer.live.wish.take() else {
            return;
        };
        let Some((now, _, key)) = self.filer_hovered(id) else {
            return;
        };
        if now != path {
            return;
        }
        if self.filer.live.frames.is_none() {
            let (sender, receiver) = calloop::channel::channel();
            let inserted = self
                .loop_handle
                .insert_source(receiver, |event, _, app: &mut App| {
                    if let calloop::channel::Event::Msg((tag, frame)) = event {
                        app.filer_live_frame(tag, frame);
                    }
                });
            if inserted.is_err() {
                return;
            }
            self.filer.live.frames = Some(sender);
        }
        self.filer.live.tag += 1;
        let tag = self.filer.live.tag;
        if path.to_ascii_lowercase().ends_with(".webp") {
            return self.filer_live_webp(id, path, key, tag);
        }
        let Some(frames) = self.filer.live.frames.clone() else {
            return;
        };
        let n = crate::apps::ICON_SIZE;
        let Some(stream) = crate::peek_kinds::stream(
            std::path::Path::new(&path),
            0.0,
            (n, n),
            (0, 0),
            tag,
            frames,
            Way::TILE,
        ) else {
            return;
        };
        tracing::debug!("files: {path} plays in its tile");
        self.filer.live.playing = Some(Playing {
            win: id,
            path,
            key,
            tag,
            _stream: Some(stream),
            anim: None,
        });
    }

    /// A WebP under the pointer: its frames read on a thread, then played.
    fn filer_live_webp(&mut self, id: u32, path: String, key: String, tag: u64) {
        if self.filer.live.anims.is_none() {
            let (sender, receiver) = calloop::channel::channel();
            let inserted = self
                .loop_handle
                .insert_source(receiver, |event, _, app: &mut App| {
                    if let calloop::channel::Event::Msg((tag, anim)) = event {
                        app.filer_live_anim(tag, anim);
                    }
                });
            if inserted.is_err() {
                return;
            }
            self.filer.live.anims = Some(sender);
        }
        let Some(anims) = self.filer.live.anims.clone() else {
            return;
        };
        let file = path.clone();
        let spawned = std::thread::Builder::new()
            .name("files-live-webp".into())
            .spawn(move || {
                let n = crate::apps::ICON_SIZE;
                let anim = crate::peek_kinds::animation(std::path::Path::new(&file), (n, n))
                    .map(|a| Arc::new(squared(a, n)));
                let _ = anims.send((tag, anim));
            });
        if spawned.is_err() {
            return;
        }
        self.filer.live.playing = Some(Playing {
            win: id,
            path,
            key,
            tag,
            _stream: None,
            anim: None,
        });
    }

    /// A WebP's frames came: it plays, a frame when its time comes.
    fn filer_live_anim(&mut self, tag: u64, anim: Option<Arc<Anim>>) {
        let Some(p) = self.filer.live.playing.as_mut().filter(|p| p.tag == tag) else {
            return;
        };
        let Some(anim) = anim else {
            // (A still WebP: it stays as it is.)
            return;
        };
        p.anim = Some((anim, Instant::now(), usize::MAX));
        let tick = calloop::timer::Timer::from_duration(Duration::ZERO);
        let _ = self
            .loop_handle
            .insert_source(tick, move |_, _, app: &mut App| app.filer_live_tick(tag));
    }

    /// Show the WebP's frame of now; come back for the next.
    fn filer_live_tick(&mut self, tag: u64) -> calloop::timer::TimeoutAction {
        let Some(p) = self.filer.live.playing.as_mut().filter(|p| p.tag == tag) else {
            return calloop::timer::TimeoutAction::Drop;
        };
        let Some((anim, since, shown)) = p.anim.as_mut() else {
            return calloop::timer::TimeoutAction::Drop;
        };
        let at = anim.at(since.elapsed().as_secs_f32() * 1000.0);
        let frame = (*shown != at).then(|| anim.frames[at].0.clone());
        *shown = at;
        let (win, key) = (p.win, p.key.clone());
        if let Some(frame) = frame {
            let chain = crate::apps::with_mips(frame);
            if !self.filer_tile_layer(win, &key, &chain) {
                self.filer_live_stop();
                return calloop::timer::TimeoutAction::Drop;
            }
        }
        calloop::timer::TimeoutAction::ToDuration(Duration::from_millis(20))
    }

    /// A frame of what plays: into its tile's layer.
    fn filer_live_frame(&mut self, tag: u64, frame: Vec<u8>) {
        let Some(p) = self.filer.live.playing.as_ref().filter(|p| p.tag == tag) else {
            return;
        };
        let n = crate::apps::ICON_SIZE as usize;
        if frame.len() != n * n * 4 {
            // (The end, or something it could not read: the still again.)
            return self.filer_live_stop();
        }
        let (win, key) = (p.win, p.key.clone());
        let chain = crate::apps::with_mips(frame);
        if !self.filer_tile_layer(win, &key, &chain) {
            // (Its layer was given to another picture: scrolled away.)
            self.filer_live_stop();
        }
    }

    /// Stop what plays; its tile wears its still picture again.
    pub(crate) fn filer_live_stop(&mut self) {
        let Some(p) = self.filer.live.playing.take() else {
            return;
        };
        // (Dropping the stream ends its decoder.)
        let (win, key) = (p.win, p.key.clone());
        drop(p);
        if let Some(still) = self.filer_still(win, &key) {
            self.filer_tile_layer(win, &key, &still);
        }
    }

    /// Window `id` closed: what played or waited in it is let go.
    pub(crate) fn filer_live_closed(&mut self, id: u32) {
        if self.filer.live.wish.as_ref().is_some_and(|(w, _)| *w == id) {
            self.filer.live.wish = None;
        }
        if self
            .filer
            .live
            .playing
            .as_ref()
            .is_some_and(|p| p.win == id)
        {
            // (Its layers are gone with it: nothing to put back.)
            self.filer.live.playing = None;
        }
    }
}

/// `anim`'s frames each centred on a clear `n`×`n` square, as a thumbnail
/// stands in its tile.
fn squared(anim: Anim, n: u32) -> Anim {
    let (w, h) = anim.size;
    let (x0, y0) = (
        (n.saturating_sub(w) / 2) as usize,
        (n.saturating_sub(h) / 2) as usize,
    );
    let (w, h, n) = (w.min(n) as usize, h.min(n) as usize, n as usize);
    let frames = anim
        .frames
        .into_iter()
        .map(|(px, ms)| {
            let mut sq = vec![0u8; n * n * 4];
            for y in 0..h {
                let from = &px[y * w * 4..(y + 1) * w * 4];
                let at = ((y0 + y) * n + x0) * 4;
                sq[at..at + w * 4].copy_from_slice(from);
            }
            (sq, ms)
        })
        .collect();
    Anim {
        size: (n as u32, n as u32),
        frames,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn videos_and_gifs_move_pictures_do_not() {
        assert!(moves("clip.MP4"));
        assert!(moves("cat.gif"));
        assert!(moves("film.mkv"));
        assert!(!moves("photo.jpg"));
        assert!(!moves("notes.txt"));
        assert!(moves("sticker.webp"));
    }

    #[test]
    fn a_frame_stands_in_the_middle_of_its_square() {
        let anim = Anim {
            size: (2, 1),
            frames: vec![(vec![9; 8], 100)],
        };
        let sq = squared(anim, 4);
        assert_eq!(sq.size, (4, 4));
        let px = &sq.frames[0].0;
        // (Row 1, columns 1 and 2.)
        assert_eq!(&px[(4 + 1) * 4..(4 + 3) * 4], &[9; 8]);
        assert_eq!(px.iter().filter(|&&b| b == 9).count(), 8);
    }
}
