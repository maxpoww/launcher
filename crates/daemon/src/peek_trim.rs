//! THE PENCIL on a VIDEO or a SONG (Max, 2026-10-16, the lab: "click en el
//! lápiz, te deja recortar el principio y fin, y pocas cosas más como en
//! Android"): two handles on a line as long as it lasts cut its start and
//! its end; a video's sound can be taken away. Saved WITHOUT re-encoding
//! (`ffmpeg -c copy`): quick on any machine, the cut falling on the nearest
//! keyframe before the start (as Android's does). Save writes it over (the
//! original kept in the versions folder), Save copy beside it. A song can
//! FADE in and out (its sound alone is made again for that — a song is
//! quick to encode). A video's moment is kept as a PHOTO from the camera on
//! its bar (`peek_snap`).

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
/// A fade's length, in seconds (a song a third of it at most).
const FADE_IN: f32 = 2.0;
const FADE_OUT: f32 = 3.0;

pub(crate) struct TrimEdit {
    tag: u64,
    secs: f32,
    video: bool,
    /// Where it starts and ends now.
    a: f32,
    b: f32,
    mute: bool,
    /// A song fades in and out.
    fade: bool,
    /// The handle in hand (0 the start, 1 the end).
    grab: Option<usize>,
    armed: bool,
    saving: bool,
}

impl TrimEdit {
    fn changed(&self) -> bool {
        self.a > 0.05 || self.b < self.secs - 0.05 || self.mute || self.fade
    }
}

/// The trim's parts in a frame `r`: the line, and the switch (a video's
/// sound, a song's fades).
pub(crate) struct TrimParts {
    pub strip: Rect,
    pub mute: Rect,
}

pub(crate) fn trim_parts(r: Rect, video: bool) -> TrimParts {
    let w = (r.w - 48.0).max(120.0);
    let strip = Rect::new(r.x + 24.0, r.y + r.h - 16.0 - 30.0 - 46.0, w, 40.0);
    let mute_w = switch_word(video).chars().count() as f32 * 12.0 * 0.56 + 26.0;
    let mute = Rect::new(
        r.x + r.w - 24.0 - mute_w,
        r.y + r.h - 16.0 - 28.0,
        mute_w,
        26.0,
    );
    TrimParts { strip, mute }
}

/// What the switch says.
fn switch_word(video: bool) -> &'static str {
    if video {
        tr("Take the sound away")
    } else {
        tr("Fade in and out")
    }
}

/// The encoder's settings for a sound made again, by the file's ending (as
/// good as what is usual for it; else ffmpeg's own choice).
fn audio_quality(ext: &str) -> &'static [&'static str] {
    match ext.to_ascii_lowercase().as_str() {
        ".mp3" => &["-q:a", "2"],
        ".m4a" | ".aac" | ".mp4" => &["-b:a", "192k"],
        ".ogg" | ".oga" => &["-q:a", "6"],
        ".opus" => &["-b:a", "160k"],
        _ => &[],
    }
}

/// The fades for a cut `len` seconds long.
fn fades(len: f32) -> String {
    let fin = FADE_IN.min(len / 3.0);
    let fout = FADE_OUT.min(len / 3.0);
    format!(
        "afade=t=in:st=0:d={fin:.3},afade=t=out:st={:.3}:d={fout:.3}",
        (len - fout).max(0.0)
    )
}

/// Cut `path` to `a`..`b` seconds (no re-encoding), its sound taken away if
/// `mute` or faded in and out if `fade` (only the sound made again); over it
/// (its original kept) or beside it.
#[allow(clippy::too_many_arguments)]
fn save(
    path: &Path,
    a: f32,
    b: f32,
    mute: bool,
    fade: bool,
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
    let tmp = dir.join(format!(
        ".{stem}.{}.golem-tmp{ext}",
        crate::peek_edit::nonce()
    ));
    let mut cmd = crate::peek_edit::dies_with_us(std::process::Command::new(ffmpeg));
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
    .arg(path);
    if fade && !mute {
        // (A cover picture rides along as it is.)
        cmd.args(["-map", "0", "-c:v", "copy", "-af", &fades(b - a)])
            .args(audio_quality(&ext));
    } else {
        cmd.args(["-map", "0", "-c", "copy"]);
    }
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

/// Keep the frame `at` seconds into the video `path` as a JPEG beside it.
fn snap(path: &Path, at: f32, word: &str) -> anyhow::Result<PathBuf> {
    use anyhow::Context;
    let ffmpeg = crate::thumbs::ffmpeg_tool().context("ffmpeg is not here")?;
    let name = path
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    let stem = match name.rsplit_once('.') {
        Some((s, _)) if !s.is_empty() => s.to_owned(),
        _ => name.clone(),
    };
    let dir = path.parent().unwrap_or(Path::new(".")).to_path_buf();
    // ("clip (frame 1-05).jpg": no colon in a file's name.)
    let when = clock(at).replace(':', "-");
    let to = (1..)
        .map(|n| {
            dir.join(if n == 1 {
                format!("{stem} ({word} {when}).jpg")
            } else {
                format!("{stem} ({word} {when} {n}).jpg")
            })
        })
        .find(|p| !p.exists())
        .context("no free name")?;
    let tmp = dir.join(format!(
        ".{stem}.{}.golem-tmp.jpg",
        crate::peek_edit::nonce()
    ));
    let out = crate::peek_edit::dies_with_us(std::process::Command::new(ffmpeg))
        .args(["-v", "error", "-y", "-ss", &format!("{at:.3}"), "-i"])
        .arg(path)
        .args(["-frames:v", "1", "-q:v", "2"])
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
    std::fs::rename(&tmp, &to).context("writing")?;
    Ok(to)
}

impl App {
    /// The video's moment on screen, kept as a photo beside it (the camera
    /// on its bar).
    pub(crate) fn peek_snap(&mut self) {
        let Some(secs) = self.peek_shown().and_then(|(_, p)| p.video).map(|(s, _)| s) else {
            return;
        };
        let Some(tx) = self.peek_edit_channel() else {
            return;
        };
        let Some(up) = self.peek.up.as_mut() else {
            return;
        };
        // (Playing, it stops there: the photo is of what is seen.)
        let at = up.song_at().clamp(0.0, (secs - 0.05).max(0.0));
        up.stop();
        up.at = at;
        let path = up.path.clone();
        let word = tr("frame").to_owned();
        let spawned = std::thread::Builder::new()
            .name("quick-look-snap".into())
            .spawn(move || {
                let result = snap(&path, at, &word).map_err(|e| format!("{e:#}"));
                let _ = tx.send(crate::peek_editor::Msg::Snapped(result));
            });
        if let Err(e) = spawned {
            warn!("quick look: no snap thread: {e}");
        }
        self.request_desktop_top_draw();
    }

    /// A photo of a video's moment came back.
    pub(crate) fn snapped(&mut self, result: Result<PathBuf, String>) {
        let word = match result {
            Ok(to) => fill(
                tr("Photo saved: {name}"),
                &[(
                    "name",
                    &to.file_name()
                        .map(|n| n.to_string_lossy().into_owned())
                        .unwrap_or_default(),
                )],
            ),
            Err(e) => {
                warn!("quick look: no photo: {e}");
                fill(tr("Could not save: {why}"), &[("why", &e)])
            }
        };
        if let Some(up) = self.peek.up.as_mut() {
            up.notice = Some((word, Instant::now()));
        }
        self.request_desktop_top_draw();
    }

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
            fade: false,
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
        let (a, b, mute, fade, tag) = (t.a, t.b, t.mute, t.fade, t.tag);
        let word = tr("edited").to_owned();
        let spawned = std::thread::Builder::new()
            .name("quick-look-trim".into())
            .spawn(move || {
                let result =
                    save(&path, a, b, mute, fade, how, &word).map_err(|e| format!("{e:#}"));
                let _ = tx.send(crate::peek_editor::Msg::Saved(tag, result));
            });
        if let Err(e) = spawned {
            warn!("quick look: no trim thread: {e}");
        }
        self.request_desktop_top_draw();
    }

    /// A save of the cut came back.
    pub(crate) fn trim_saved(&mut self, tag: u64, result: Result<PathBuf, String>) {
        // (Only this cut's: one of an older one is not this one's.)
        if self
            .peek
            .up
            .as_ref()
            .and_then(|u| u.trim_edit.as_ref())
            .is_none_or(|t| t.tag != tag)
        {
            return;
        }
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
                if t.video {
                    t.mute = !t.mute;
                } else {
                    t.fade = !t.fade;
                }
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
        let parts = trim_parts(up.rect, up.trim_edit.as_ref().is_none_or(|t| t.video));
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
                if parts.mute.contains(at) {
                    if t.video {
                        t.mute = !t.mute;
                    } else {
                        t.fade = !t.fade;
                    }
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
        let parts = trim_parts(r, t.video);
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
        } else if t.fade {
            tr("the sound is made again for the fades")
        } else {
            tr("not re-encoded · cut at the nearest keyframe")
        };
        {
            let on = t.mute || t.fade;
            let lit = [1.0, 1.0, 1.0, (if on { 0.32 } else { 0.12 }) * open];
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
                    switch_word(t.video),
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
            // (A song has no sound button: the note has the line's width.)
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
            fade: false,
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
        t.mute = false;
        t.fade = true;
        assert!(t.changed());
    }

    #[test]
    fn fades_fit_a_short_cut() {
        assert_eq!(
            fades(60.0),
            "afade=t=in:st=0:d=2.000,afade=t=out:st=57.000:d=3.000"
        );
        // (Three seconds: a second each way.)
        assert_eq!(
            fades(3.0),
            "afade=t=in:st=0:d=1.000,afade=t=out:st=2.000:d=1.000"
        );
    }
}
