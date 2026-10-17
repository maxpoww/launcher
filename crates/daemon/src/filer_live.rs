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

use std::time::Duration;

use smithay_client_toolkit::reexports::calloop;

use crate::peek_kinds::{Stream, Way};
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
    tag: u64,
}

struct Playing {
    win: u32,
    path: String,
    /// The picture key whose layer it plays in.
    key: String,
    tag: u64,
    _stream: Stream,
}

/// Whether a file of this name moves: a video, a GIF.
fn moves(name: &str) -> bool {
    let lower = name.to_ascii_lowercase();
    // (The usual endings too, where no MIME database is installed.)
    [
        ".gif", ".mp4", ".m4v", ".mkv", ".webm", ".mov", ".avi", ".ogv", ".3gp",
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
        let Some(frames) = self.filer.live.frames.clone() else {
            return;
        };
        self.filer.live.tag += 1;
        let tag = self.filer.live.tag;
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
            _stream: stream,
        });
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
    }
}
