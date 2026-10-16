//! THE PENCIL on a VIDEO or a SONG (Max, 2026-10-16, the lab: "click en el
//! lápiz, te deja recortar el principio y fin, y pocas cosas más como en
//! Android"): two handles on a line as long as it lasts cut its start and
//! its end; a video's sound can be taken away. Saved WITHOUT re-encoding
//! (`ffmpeg -c copy`): quick on any machine, the cut falling on the nearest
//! keyframe before the start (as Android's does). Save writes it over (the
//! original kept in the versions folder), Save copy beside it.

use std::path::{Path, PathBuf};
use std::time::Instant;

use smithay_client_toolkit::seat::keyboard::Keysym;
use tracing::warn;

use crate::content::{GridContent, Label, Rect, RectInst, FONT_BOLD};
use crate::i18n::{fill, tr};
use crate::peek_edit::SaveAs;
use crate::peek_kinds::clock;
use crate::quicklook::{text_label, HEAD};
use crate::App;

/// The shortest a cut may leave, in seconds.
const SHORTEST: f32 = 1.0;

pub(crate) struct TrimEdit {
    tag: u64,
    secs: f32,
    video: bool,
    /// Where it starts and ends now.
    a: f32,
    b: f32,
    mute: bool,
    /// The handle in hand (0 the start, 1 the end).
    grab: Option<usize>,
    armed: bool,
    saving: bool,
}

impl TrimEdit {
    fn changed(&self) -> bool {
        self.a > 0.05 || self.b < self.secs - 0.05 || self.mute
    }
}

/// The trim's parts in a frame `r`: the line, and the sound switch.
pub(crate) struct TrimParts {
    pub strip: Rect,
    pub mute: Rect,
}

pub(crate) fn trim_parts(r: Rect) -> TrimParts {
    let w = (r.w - 48.0).max(120.0);
    let strip = Rect::new(r.x + 24.0, r.y + r.h - 16.0 - 30.0 - 46.0, w, 40.0);
    let mute_w = tr("Take the sound away").chars().count() as f32 * 12.0 * 0.56 + 26.0;
    let mute = Rect::new(
        r.x + r.w - 24.0 - mute_w,
        r.y + r.h - 16.0 - 28.0,
        mute_w,
        26.0,
    );
    TrimParts { strip, mute }
}

/// Cut `path` to `a`..`b` seconds (no re-encoding), its sound taken away if
/// `mute`; over it (its original kept) or beside it.
fn save(
    path: &Path,
    a: f32,
    b: f32,
    mute: bool,
    how: SaveAs,
    word: &str,
) -> anyhow::Result<PathBuf> {
    use anyhow::Context;
    let ffmpeg = crate::thumbs::ffmpeg_tool().context("ffmpeg is not here")?;
    let name = path
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    let (stem, ext) = match name.rsplit_once('.') {
        Some((s, e)) if !s.is_empty() => (s.to_owned(), format!(".{e}")),
        _ => (name.clone(), String::new()),
    };
    let dir = path.parent().unwrap_or(Path::new(".")).to_path_buf();
    // (The muxer is chosen by the ending: the temporary file keeps it.)
    let tmp = dir.join(format!(".{stem}.golem-tmp{ext}"));
    let mut cmd = std::process::Command::new(ffmpeg);
    cmd.args([
        "-v",
        "error",
        "-y",
        "-ss",
        &format!("{a:.3}"),
        "-to",
        &format!("{b:.3}"),
        "-i",
    ])
    .arg(path)
    .args(["-map", "0", "-c", "copy"]);
    if mute {
        cmd.arg("-an");
    }
    let out = cmd
        .arg(&tmp)
        .stdin(std::process::Stdio::null())
        .output()
        .context("running ffmpeg")?;
    if !out.status.success() || !tmp.is_file() {
        let _ = std::fs::remove_file(&tmp);
        anyhow::bail!(
            "ffmpeg: {}",
            String::from_utf8_lossy(&out.stderr)
                .lines()
                .last()
                .unwrap_or("failed")
        );
    }
    let to = match how {
        SaveAs::Same => {
            let versions = crate::peek_edit::versions_dir();
            std::fs::create_dir_all(&versions)?;
            let stamp = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map_or(0, |d| d.as_secs());
            std::fs::copy(path, versions.join(format!("{stamp}-{name}")))
                .context("keeping the original")?;
            path.to_path_buf()
        }
        SaveAs::Copy => (1..)
            .map(|n| {
                dir.join(if n == 1 {
                    format!("{stem} ({word}){ext}")
                } else {
                    format!("{stem} ({word} {n}){ext}")
                })
            })
            .find(|p| !p.exists())
            .unwrap_or_else(|| dir.join(format!("{stem} ({word}){ext}"))),
    };
    std::fs::rename(&tmp, &to).context("writing")?;
    Ok(to)
}

impl App {
    /// Whether what is up can be cut: a video or a song.
    pub(crate) fn trim_able(&self) -> Option<(f32, bool)> {
        let (_, p) = self.peek_shown()?;
        match &p.body {
            crate::quicklook::Body::Song { secs, .. } if *secs > SHORTEST => Some((*secs, false)),
            _ => p
                .video
                .filter(|(secs, _)| *secs > SHORTEST)
                .map(|(secs, _)| (secs, true)),
        }
    }

    /// Take the pencil on a video or a song.
    pub(crate) fn trim_begin(&mut self) {
        let Some((secs, video)) = self.trim_able() else {
            return;
        };
        self.peek.tag += 1;
        let tag = self.peek.tag;
        let Some(up) = self.peek.up.as_mut() else {
            return;
        };
        up.stop();
        up.trim_edit = Some(TrimEdit {
            tag,
            secs,
            video,
            a: 0.0,
            b: secs,
            mute: false,
            grab: None,
            armed: false,
            saving: false,
        });
        self.request_desktop_top_draw();
    }

    fn trim_end(&mut self) {
        if let Some(up) = self.peek.up.as_mut() {
            up.trim_edit = None;
        }
        self.request_desktop_top_draw();
    }

    fn trim_cancel(&mut self) {
        let Some(t) = self.peek.up.as_mut().and_then(|u| u.trim_edit.as_mut()) else {
            return;
        };
        if t.changed() && !t.armed {
            t.armed = true;
            self.request_desktop_top_draw();
            return;
        }
        self.trim_end();
    }

    pub(crate) fn trim_save(&mut self, how: SaveAs) {
        let Some(tx) = self.peek_edit_channel() else {
            return;
        };
        let Some(up) = self.peek.up.as_mut() else {
            return;
        };
        let path = up.path.clone();
        let Some(t) = up.trim_edit.as_mut() else {
            return;
        };
        if t.saving {
            return;
        }
        if !t.changed() {
            return self.trim_end();
        }
        t.saving = true;
        let (a, b, mute, tag) = (t.a, t.b, t.mute, t.tag);
        let word = tr("edited").to_owned();
        let spawned = std::thread::Builder::new()
            .name("quick-look-trim".into())
            .spawn(move || {
                let result = save(&path, a, b, mute, how, &word).map_err(|e| format!("{e:#}"));
                let _ = tx.send(crate::peek_editor::Msg::Saved(tag, result));
            });
        if let Err(e) = spawned {
            warn!("quick look: no trim thread: {e}");
        }
        self.request_desktop_top_draw();
    }

    /// A save of the cut came back.
    pub(crate) fn trim_saved(&mut self, result: Result<PathBuf, String>) {
        match result {
            Ok(to) => {
                let same = self.peek.up.as_ref().is_some_and(|u| u.path == to);
                let word = if same {
                    tr("Saved. The original is kept.").to_owned()
                } else {
                    fill(
                        tr("Copy saved: {name}"),
                        &[(
                            "name",
                            &to.file_name()
                                .map(|n| n.to_string_lossy().into_owned())
                                .unwrap_or_default(),
                        )],
                    )
                };
                self.trim_end();
                self.peek_forget(&to);
                self.thumb_stale(&to.to_string_lossy());
                if let Some(up) = self.peek.up.as_mut() {
                    if !same {
                        up.name = to
                            .file_name()
                            .map(|n| n.to_string_lossy().into_owned())
                            .unwrap_or_default();
                        up.path = to.clone();
                    }
                    up.frames = 0;
                    up.notice = Some((word, Instant::now()));
                }
                self.peek_want_now((to, 0));
            }
            Err(e) => {
                warn!("quick look: cutting failed: {e}");
                if let Some(up) = self.peek.up.as_mut() {
                    if let Some(t) = up.trim_edit.as_mut() {
                        t.saving = false;
                    }
                    up.notice = Some((
                        fill(tr("Could not save: {why}"), &[("why", &e)]),
                        Instant::now(),
                    ));
                }
            }
        }
        self.request_desktop_top_draw();
    }

    /// Set the cut (`NaN` keeps a side), and turn the sound if asked (the rig).
    pub(crate) fn trim_set(&mut self, a: f32, b: f32, mute: Option<()>) {
        if let Some(t) = self.peek.up.as_mut().and_then(|u| u.trim_edit.as_mut()) {
            if !a.is_nan() {
                t.a = a.clamp(0.0, t.secs - SHORTEST);
            }
            if !b.is_nan() {
                t.b = b.clamp(t.a + SHORTEST, t.secs);
            }
            if mute.is_some() {
                t.mute = !t.mute;
            }
        }
        self.request_desktop_top_draw();
    }

    /// A key while cutting.
    pub(crate) fn trim_key(&mut self, key: Keysym) {
        let (ctrl, shift) = (self.modifiers.ctrl, self.modifiers.shift);
        match key {
            Keysym::s | Keysym::S if ctrl => {
                self.trim_save(if shift { SaveAs::Copy } else { SaveAs::Same })
            }
            Keysym::Escape => self.trim_cancel(),
            _ => {}
        }
    }

    /// The pointer while cutting.
    pub(crate) fn trim_pointer(
        &mut self,
        event: &smithay_client_toolkit::reexports::client::protocol::wl_pointer::Event,
    ) {
        use smithay_client_toolkit::reexports::client::protocol::wl_pointer::{ButtonState, Event};
        use smithay_client_toolkit::reexports::client::WEnum;
        let Some(up) = self.peek.up.as_ref() else {
            return;
        };
        let at = up.ptr.unwrap_or((-1.0, -1.0));
        let parts = trim_parts(up.rect);
        let head = crate::peek_editor::edit_parts(up.rect, false);
        match event {
            Event::Motion {
                surface_x,
                surface_y,
                ..
            } => {
                let at = (*surface_x as f32, *surface_y as f32);
                if let Some(up) = self.peek.up.as_mut() {
                    up.ptr = Some(at);
                    if let Some(t) = up.trim_edit.as_mut() {
                        if let Some(h) = t.grab {
                            let s =
                                ((at.0 - parts.strip.x) / parts.strip.w).clamp(0.0, 1.0) * t.secs;
                            if h == 0 {
                                t.a = s.min(t.b - SHORTEST).max(0.0);
                            } else {
                                t.b = s.max(t.a + SHORTEST).min(t.secs);
                            }
                            t.armed = false;
                        }
                    }
                }
                self.request_desktop_top_draw();
            }
            Event::Button {
                state: WEnum::Value(ButtonState::Pressed),
                ..
            } => {
                if head.save.contains(at) {
                    return self.trim_save(SaveAs::Same);
                }
                if head.copy.contains(at) {
                    return self.trim_save(SaveAs::Copy);
                }
                if head.cancel.contains(at) {
                    return self.trim_cancel();
                }
                let Some(t) = self.peek.up.as_mut().and_then(|u| u.trim_edit.as_mut()) else {
                    return;
                };
                if t.video && parts.mute.contains(at) {
                    t.mute = !t.mute;
                    t.armed = false;
                } else if Rect::new(
                    parts.strip.x - 12.0,
                    parts.strip.y - 8.0,
                    parts.strip.w + 24.0,
                    parts.strip.h + 16.0,
                )
                .contains(at)
                {
                    // (The handle nearer the pointer.)
                    let s = ((at.0 - parts.strip.x) / parts.strip.w).clamp(0.0, 1.0) * t.secs;
                    t.grab = Some(if (s - t.a).abs() <= (s - t.b).abs() {
                        0
                    } else {
                        1
                    });
                }
                self.request_desktop_top_draw();
            }
            Event::Button {
                state: WEnum::Value(ButtonState::Released),
                ..
            } => {
                if let Some(t) = self.peek.up.as_mut().and_then(|u| u.trim_edit.as_mut()) {
                    t.grab = None;
                }
            }
            _ => {}
        }
    }

    /// The cut into the frame `r` (over the picture or the cover).
    pub(crate) fn trim_push(
        &self,
        g: &mut GridContent,
        over: &mut GridContent,
        r: Rect,
        open: f32,
        ink: [f32; 4],
        ground: [f32; 4],
    ) {
        let Some(up) = self.peek.up.as_ref() else {
            return;
        };
        let Some(t) = up.trim_edit.as_ref() else {
            return;
        };
        let parts = trim_parts(r);
        let head = crate::peek_editor::edit_parts(r, false);
        let white = [1.0, 1.0, 1.0, open];
        // A dark band under the line and the time, so they read on any picture.
        let band = Rect::new(
            r.x,
            parts.strip.y - 12.0,
            r.w,
            r.y + r.h - (parts.strip.y - 12.0),
        );
        over.rects.push(RectInst {
            rect: band,
            radius: 0.0,
            color: [0.04, 0.05, 0.06, 0.72 * open],
            glass: 0.0,
            border: 0.0,
        });
        let s = parts.strip;
        let x = |secs: f32| s.x + secs / t.secs * s.w;
        over.rects.push(RectInst {
            rect: s,
            radius: 8.0,
            color: [1.0, 1.0, 1.0, 0.16 * open],
            glass: 0.0,
            border: 0.0,
        });
        // What is kept, lit; what goes, dark.
        over.rects.push(RectInst {
            rect: Rect::new(x(t.a), s.y, x(t.b) - x(t.a), s.h),
            radius: 6.0,
            color: [1.0, 0.75, 0.6, 0.35 * open],
            glass: 0.0,
            border: 0.0,
        });
        for (h, secs) in [t.a, t.b].into_iter().enumerate() {
            let hot = t.grab == Some(h);
            over.rects.push(RectInst {
                rect: Rect::new(x(secs) - 6.0, s.y - 3.0, 12.0, s.h + 6.0),
                radius: 4.0,
                color: [1.0, 0.75, 0.6, (if hot { 1.0 } else { 0.9 }) * open],
                glass: 0.0,
                border: 0.0,
            });
        }
        let words = format!(
            "{} – {} · {}",
            clock(t.a),
            clock(t.b),
            fill(tr("{n} s"), &[("n", &((t.b - t.a).round() as u32))])
        );
        over.labels.push(text_label(
            &words,
            (s.x, s.y + s.h + 12.0),
            260.0,
            12.5,
            None,
            white,
        ));
        let note = if t.mute {
            tr("without sound · cut at the nearest keyframe")
        } else {
            tr("not re-encoded · cut at the nearest keyframe")
        };
        if t.video {
            let lit = [1.0, 1.0, 1.0, (if t.mute { 0.32 } else { 0.12 }) * open];
            over.rects.push(RectInst {
                rect: parts.mute,
                radius: 13.0,
                color: lit,
                glass: 0.0,
                border: 0.0,
            });
            over.labels.push(Label {
                centered: true,
                ..text_label(
                    tr("Take the sound away"),
                    (parts.mute.x + parts.mute.w / 2.0, parts.mute.y + 5.0),
                    parts.mute.w,
                    12.0,
                    Some(FONT_BOLD),
                    white,
                )
            });
        }
        let mut dim = white;
        dim[3] *= 0.6;
        over.labels.push(text_label(
            note,
            (s.x + 270.0, s.y + s.h + 12.0),
            (parts.mute.x - s.x - 290.0).max(60.0),
            11.5,
            None,
            dim,
        ));
        // The head.
        let mut dim_ink = ink;
        dim_ink[3] *= 0.6;
        let text_w = (head.cancel.x - (r.x + 46.0) - 12.0).max(20.0);
        g.labels.push(text_label(
            &up.name,
            (r.x + 46.0, r.y + 7.0),
            text_w,
            13.5,
            Some(FONT_BOLD),
            ink,
        ));
        g.labels.push(text_label(
            if t.saving {
                tr("Saving…")
            } else {
                tr("Editing")
            },
            (r.x + 46.0, r.y + 25.0),
            text_w,
            11.5,
            None,
            dim_ink,
        ));
        let hot = |rect: Rect| up.ptr.is_some_and(|p| rect.contains(p));
        let soft = |rect: Rect| {
            [
                ink[0],
                ink[1],
                ink[2],
                (if hot(rect) { 0.26 } else { 0.14 }) * open,
            ]
        };
        let pill =
            |g: &mut GridContent, rect: Rect, color: [f32; 4], word: &str, word_ink: [f32; 4]| {
                g.rects.push(RectInst {
                    rect,
                    radius: rect.h / 2.0,
                    color,
                    glass: 0.0,
                    border: 0.0,
                });
                g.labels.push(Label {
                    centered: true,
                    ..text_label(
                        word,
                        (rect.x + rect.w / 2.0, rect.y + 5.0),
                        rect.w - 6.0,
                        12.0,
                        Some(FONT_BOLD),
                        word_ink,
                    )
                });
            };
        let (cw, cf, ci) = if t.armed {
            (tr("Discard changes?"), [0.88, 0.32, 0.32, open], white)
        } else {
            (tr("Cancel"), soft(head.cancel), ink)
        };
        pill(g, head.cancel, cf, cw, ci);
        pill(g, head.copy, soft(head.copy), tr("Save copy"), ink);
        pill(
            g,
            head.save,
            [1.0, 0.75, 0.6, open],
            tr("Save"),
            [ground[0], ground[1], ground[2], open],
        );
        let _ = HEAD;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_cut_says_whether_it_changed_anything() {
        let mut t = TrimEdit {
            tag: 0,
            secs: 60.0,
            video: true,
            a: 0.0,
            b: 60.0,
            mute: false,
            grab: None,
            armed: false,
            saving: false,
        };
        assert!(!t.changed());
        t.a = 3.0;
        assert!(t.changed());
        t.a = 0.0;
        t.mute = true;
        assert!(t.changed());
    }
}
