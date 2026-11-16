//! PRINT TAKES THE WHOLE SCREEN AT ONCE (Max, 2026-11-16, the third round on
//! the capture tool: "the print key takes a full screen and shows on the card
//! pill (the main) kind of how we show a notification, the user can click to
//! edit the screenshot (section, window, etc) or let it save as is (2 s later
//! it goes away to the parent (the card pill)). also start recording by
//! holding the print key for 1.5 s"; agreed with him the same night: about
//! four seconds and held while the pointer is on it, an edit REPLACES the
//! whole-screen file, a tap of Print stops a running recording).
//!
//! THE KEY (`Press`, a small state machine with the time handed in). Golem's
//! hyprland.lua says when it goes down and when it comes up:
//! - `capture press`: a recording that runs is STOPPED, and that is all (the
//!   release after it does nothing). Else the screen the pointer is on is
//!   taken NOW (`grim -o`, off the loop; whatever of the tool or of an
//!   earlier preview is on the screen is taken off first and its frame
//!   given `HIDE_FIRST` to be shown), together with the windows that are on
//!   it at that moment.
//! - `capture release` before `HOLD`: the shot IS the screenshot.
//! - `HOLD` (1.5 s) with the key still down: the shot is dropped — never
//!   saved, never on the clipboard — and the screen recording starts (the
//!   whole screen, as `record start`); the release that follows does
//!   nothing. From `SIGN_FROM` (300 ms) of holding the card's long pill
//!   comes out saying so, filling as the task pill does until the recording
//!   starts: the hold is a shortcut BESIDE the visible controls (the card's
//!   Record button, the tool's Video), with a sign while it is held. The
//!   timer exists only while the key is down.
//! - `capture shot`: press and release in one (a keyboard whose Print cannot
//!   be held; a rig).
//!
//! THE SCREENSHOT, once it is one: on the clipboard and in Pictures →
//! Screenshots at once, as ever — and NO notification: THE PREVIEW is the
//! word. The card's long pill (`clipboard.rs::push_clip_pill`, the pill that
//! shows the card's last thing on hover) comes out of the card's pill by
//! itself, as the bell's does for a new notification, with a small picture of
//! the shot (the clipboard's own thumbnails, in the bar's icon array), the
//! word `Screenshot` and a × at its end. It stays `PREVIEW_FOR`, then folds
//! back into the card's pill; under the pointer its time STANDS STILL and
//! goes on from where it stood when the pointer leaves (`preview_left`). A
//! CLICK on it opens THE EDITOR; its × throws the shot away (the file is
//! deleted). A second Print meanwhile settles the first as it is and takes
//! its place.
//!
//! THE SHOT, BIGGER, ON HOVER (the fourth round, the same night: the pill's
//! thumbnail is too small to judge a shot by). With the pointer on the
//! preview the long pill GROWS DOWN into a box holding the shot at about
//! `HANG_W` (never wider than a third of the screen, the shot's own shape) —
//! the way the clipboard's box grows down out of this very pill, the
//! lightest idiom there is that can hold a picture: the same pill, rect
//! (`clip_geom`), motion, corner and fill (`push_clip_pill` takes
//! `capture_hang_t` beside its own `expand_t`), no surface and no layer of
//! its own. The picture is a copy of the shot made at that size off the loop
//! as the shot lands (`Kept::small`), shown through the bar renderer's one
//! big picture (`set_video_frame`), not the thumbnail blown up. The pointer
//! away and it folds, and the preview's own time goes on from there; a
//! click on the picture is a click on the pill (the editor); the × stays on
//! the pill's row. A Print, or the live tool opening, takes pill AND picture
//! off in one step, at once (`capture_preview_off_now`), before anything is
//! taken. One clock while it moves (`schedule_tick`), none at rest.
//!
//! THE EDITOR is the capture tool (`capture.rs`) over the FROZEN shot
//! (`Tool::still`): the shot is drawn whole on the tool's surface, pixel for
//! pixel (the renderer's one big picture, `Renderer::set_video_frame`: no
//! tiles, no mips, the screen's own pixels on the screen's own pixels), under
//! the dim; the banner's pills are `Region · Window · Screen · Copy text`
//! (nothing of a film here), the windows offered are the ones that were on
//! the screen WHEN THE SHOT WAS TAKEN, and the button by the selection says
//! `Save`: the kept PNG is cut in process (`crop_px`, `cut`; off the loop),
//! written OVER the whole-screen file under the same name (`part_beside`,
//! then a rename) and put on the clipboard in the whole one's place. `Screen`
//! + Save, Escape or the round pill leave the shot as it was taken.
//!
//! Rig: `capture press|release|shot`, `capture preview click|close|state|
//! hover|unhover` (the pointer on the preview, and off it),
//! `capture edit` (the editor on the shot whose preview is out).

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime};

use tracing::{info, warn};

use crate::capture::{Area, Screen, Stacked};
use crate::content::{IconInst, Label, Rect, RectInst, Scene};
use crate::i18n::tr;
use crate::options::{PillId, FONT_PX, LINE_PX, NERD, PILL_PAD_X};
use crate::record::Aim;
use crate::App;

/// The key held this long is a recording, not a screenshot.
pub(crate) const HOLD: Duration = Duration::from_millis(1500);
/// From here on a held key shows its sign.
pub(crate) const SIGN_FROM: Duration = Duration::from_millis(300);
/// How long the preview stays out by itself.
pub(crate) const PREVIEW_FOR: Duration = Duration::from_secs(4);
/// …and the least it stays after the pointer left it.
const PREVIEW_AFTER_HAND: Duration = Duration::from_millis(1200);
/// How often a preview that is out looks at whether to fold (ms).
const PREVIEW_LOOK: u64 = 250;
/// What of ours was on the screen is given this long to be off it (ms).
const HIDE_FIRST: u64 = 150;
/// The hanging picture: how wide at the bar's scale (and never wider than
/// this share of the screen), the air round it in its box, its own corner.
const HANG_W: f32 = 360.0;
const HANG_SHARE: f32 = 1.0 / 3.0;
const HANG_PAD: f32 = 6.0;
const HANG_CORNER: f32 = 6.0;
/// The sign's frames while the key is held (ms).
const SIGN_FRAME: u64 = 33;
/// fa-camera, where a preview's picture is not there yet; fa-video-camera,
/// the hold's sign.
const GLYPH_SHOT: &str = "\u{f030}";
const GLYPH_FILM: &str = "\u{f03d}";

/// What a press of the key asks for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum OnPress {
    /// A recording runs: stop it (and the release that follows is nothing).
    StopRecording,
    /// Take the screen now; what becomes of it is the release's to say.
    Shoot,
}

/// THE KEY: down since when, and whether its release still means anything.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Press {
    down: Option<Instant>,
    /// This press is spoken for (it stopped a recording, or became one).
    spent: bool,
    /// Counts presses: a timer armed for one does nothing for the next.
    pub(crate) n: u64,
}

impl Press {
    /// The key went down at `now`. `None`: it was down already (a repeat).
    pub(crate) fn press(&mut self, now: Instant, recording: bool) -> Option<OnPress> {
        if self.down.is_some() {
            return None;
        }
        self.n += 1;
        self.down = Some(now);
        self.spent = recording;
        Some(if recording {
            OnPress::StopRecording
        } else {
            OnPress::Shoot
        })
    }

    /// The key came up at `now`. True: the shot taken at its press is the
    /// screenshot. False: nothing (it stopped a recording, it became one, or
    /// it was never down).
    pub(crate) fn release(&mut self, now: Instant) -> bool {
        let Some(down) = self.down.take() else {
            return false;
        };
        !std::mem::take(&mut self.spent) && now.duration_since(down) < HOLD
    }

    /// Whether press `n`, still down at `now`, has been held long enough to
    /// be a recording — said once: the press is spent with it.
    pub(crate) fn held_enough(&mut self, n: u64, now: Instant) -> bool {
        let due =
            self.n == n && !self.spent && self.down.is_some_and(|d| now.duration_since(d) >= HOLD);
        self.spent |= due;
        due
    }

    /// How far the hold's sign is at `now`: none before `SIGN_FROM` (a tap
    /// shows nothing), then 0 → 1 at `HOLD`.
    pub(crate) fn sign(&self, now: Instant) -> Option<f32> {
        let held = now.duration_since(self.down.filter(|_| !self.spent)?);
        (held >= SIGN_FROM).then(|| {
            ((held - SIGN_FROM).as_secs_f32() / (HOLD - SIGN_FROM).as_secs_f32()).clamp(0.0, 1.0)
        })
    }

    pub(crate) fn is_down(&self) -> bool {
        self.down.is_some()
    }
}

/// Region `a` of a screen `logical` big, in the pixels of its picture `px`
/// big: `(x, y, w, h)`. Each edge is rounded on its own (so two regions that
/// meet on the screen meet in the picture, whatever the scale — 0.75, 1.6),
/// kept inside the picture, and never empty.
pub(crate) fn crop_px(a: Area, logical: (i32, i32), px: (u32, u32)) -> (u32, u32, u32, u32) {
    let (sx, sy) = (
        f64::from(px.0) / f64::from(logical.0.max(1)),
        f64::from(px.1) / f64::from(logical.1.max(1)),
    );
    let edge = |v: i32, s: f64, most: u32| ((f64::from(v) * s).round().max(0.0) as u32).min(most);
    let (x0, y0) = (
        edge(a.x, sx, px.0.saturating_sub(1)),
        edge(a.y, sy, px.1.saturating_sub(1)),
    );
    let (x1, y1) = (
        edge(a.x + a.w, sx, px.0).max(x0 + 1),
        edge(a.y + a.h, sy, px.1).max(y0 + 1),
    );
    (
        x0,
        y0,
        (x1 - x0).min(px.0.saturating_sub(x0)).max(1),
        (y1 - y0).min(px.1.saturating_sub(y0)).max(1),
    )
}

/// Where a picture is written before it takes `file`'s place: beside it
/// (the same folder: the rename is then one step), hidden, under its name.
pub(crate) fn part_beside(file: &Path) -> PathBuf {
    let name = file
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    file.with_file_name(format!(".{name}.part"))
}

/// `picture` (a PNG) cut to `(x, y, w, h)` of its pixels, as a PNG.
pub(crate) fn cut(picture: &[u8], (x, y, w, h): (u32, u32, u32, u32)) -> Option<Vec<u8>> {
    let whole = image::load_from_memory_with_format(picture, image::ImageFormat::Png).ok()?;
    let part = whole.crop_imm(x, y, w, h);
    let mut out = std::io::Cursor::new(Vec::new());
    part.write_to(&mut out, image::ImageFormat::Png).ok()?;
    Some(out.into_inner())
}

/// `bytes` in `file`'s place, whole or not at all.
fn replace(file: &Path, bytes: &[u8]) -> bool {
    let part = part_beside(file);
    let done = std::fs::write(&part, bytes).is_ok() && std::fs::rename(&part, file).is_ok();
    if !done {
        let _ = std::fs::remove_file(&part);
    }
    done
}

/// The screen as it was when the key went down.
pub(crate) struct Frame {
    /// The PNG as `grim` made it.
    png: Vec<u8>,
    /// The screen it is of (its corner in the layout, its logical size), and
    /// the windows that were on it then, topmost first, on that screen.
    screen: Screen,
    wins: Vec<Area>,
}

/// A screenshot that was kept: its file, and what the editor needs of it.
pub(crate) struct Kept {
    pub file: PathBuf,
    pub png: Vec<u8>,
    /// Its pixels, for the editor's still (RGBA), and how many there are.
    pub rgba: Vec<u8>,
    pub px: (u32, u32),
    pub screen: Screen,
    pub wins: Vec<Area>,
    /// The shot made small for the picture that hangs under its preview:
    /// its size and its pixels (RGBA).
    small: (u32, u32, Vec<u8>),
}

/// A shot whose key has not said yet what it is.
struct Pending {
    n: u64,
    /// The release came: it is the screenshot (as soon as it is here).
    keep: bool,
    /// What `grim` gave (`None` inside: nothing).
    frame: Option<Option<Frame>>,
}

/// A preview that is out on the card's long pill.
struct Preview {
    kept: Arc<Kept>,
    /// How much of its time is left, as of `looked` (`preview_left`).
    left: Duration,
    looked: Instant,
    n: u64,
}

/// What is left of a preview's time, `passed` after `left` was. Under the
/// hand its clock STANDS STILL — and it is owed `PREVIEW_AFTER_HAND` at the
/// least, so it never folds from under the hand nor the moment the hand
/// leaves; with no hand on it its time runs, on from where it stood.
pub(crate) fn preview_left(left: Duration, passed: Duration, hand: bool) -> Duration {
    if hand {
        left.max(PREVIEW_AFTER_HAND)
    } else {
        left.saturating_sub(passed)
    }
}

/// The key's and the screenshot's state (in `capture::Capture`).
#[derive(Default)]
pub(crate) struct Shots {
    press: Press,
    pending: Option<Pending>,
    preview: Option<Preview>,
    /// The shot the editor opens on, on its way through the tool's opening.
    pub(crate) opening: Option<Arc<Kept>>,
    /// Counts previews (a fold timer armed for one leaves the next alone).
    previews: u64,
    /// THE PICTURE under the preview: wanted (the pointer is on it), how far
    /// down it is (0 → 1), its clock, and whether its pixels are in the bar.
    hang: bool,
    hang_t: f32,
    hang_last: Option<Instant>,
    hang_pending: bool,
    hang_in: bool,
    /// A rig's pointer on the preview (`capture preview hover`).
    hover_forced: bool,
    /// The long pill is held shut: a picture is being taken.
    shut: bool,
}

/// How big the hanging picture is, for a shot `px` big on a bar `bar_w`
/// wide at scale `s`: `HANG_W` wide at the bar's scale, never more than
/// `HANG_SHARE` of the screen, as tall as the shot's own shape makes it.
pub(crate) fn hang_size(px: (u32, u32), bar_w: f32, s: f32) -> (f32, f32) {
    let w = (HANG_W * s).min(bar_w * HANG_SHARE).max(1.0).round();
    (w, (w * px.1 as f32 / px.0.max(1) as f32).round().max(1.0))
}

/// The screen `name` as a PNG, off the loop.
fn grab_screen(name: &str) -> Option<Vec<u8>> {
    let tmp = crate::capture::scratch_png();
    let taken = crate::capture::grab(&Aim::Screen(name.to_owned()), false, &tmp);
    let png = if taken {
        std::fs::read(&tmp).ok()
    } else {
        None
    };
    let _ = std::fs::remove_file(&tmp);
    png
}

/// A frame made a screenshot, off the loop: the clipboard first, then its
/// file, then its pixels for the editor. `None`: there is no file.
fn settle(frame: Frame, small_w: u32) -> Option<Kept> {
    let copied = crate::capture::copy("image/png", &frame.png);
    let dir = crate::record::shots_folder();
    let file = crate::capture::keep(&frame.png, &dir, SystemTime::now());
    info!(
        "capture: the screen {} → {file:?} (clipboard: {copied})",
        frame.screen.name
    );
    let file = file?;
    let pixels = image::load_from_memory_with_format(&frame.png, image::ImageFormat::Png)
        .ok()?
        .to_rgba8();
    let px = (pixels.width(), pixels.height());
    // (The picture that hangs under the preview: made once, here, at the
    // size it is shown at.)
    let sw = small_w.clamp(1, px.0.max(1));
    let sh = ((u64::from(sw) * u64::from(px.1)) / u64::from(px.0.max(1))).max(1) as u32;
    let small = image::imageops::resize(&pixels, sw, sh, image::imageops::FilterType::CatmullRom);
    Some(Kept {
        small: (sw, sh, small.into_raw()),
        file,
        png: frame.png,
        rgba: pixels.into_raw(),
        px,
        screen: frame.screen,
        wins: frame.wins,
    })
}

impl App {
    /// `capture press`: the key went down.
    pub(crate) fn capture_key_press(&mut self) {
        let now = Instant::now();
        match self
            .capture_tool
            .shots
            .press
            .press(now, self.recording.is_some())
        {
            None => {}
            Some(OnPress::StopRecording) => {
                let said = self.record_command("stop");
                info!("capture: the key stopped the recording ({said})");
            }
            Some(OnPress::Shoot) => {
                let n = self.capture_tool.shots.press.n;
                self.capture_tool.shots.pending = Some(Pending {
                    n,
                    keep: false,
                    frame: None,
                });
                self.capture_shoot(n);
                // (While the key is down, and only then: its sign, and the
                // moment it becomes a recording.)
                self.after_ms(SIGN_FROM.as_millis() as u64, move |app| {
                    app.capture_sign_frame(n)
                });
                self.after_ms(HOLD.as_millis() as u64, move |app| app.capture_held(n));
            }
        }
    }

    /// `capture release`: the key came up.
    pub(crate) fn capture_key_release(&mut self) {
        let keep = self.capture_tool.shots.press.release(Instant::now());
        self.capture_sign_off();
        if !keep {
            return;
        }
        if let Some(p) = self.capture_tool.shots.pending.as_mut() {
            p.keep = true;
        }
        self.capture_settle_if_ready();
    }

    /// Press `n` has been down `HOLD`: its shot is dropped and the screen is
    /// recorded.
    fn capture_held(&mut self, n: u64) {
        if !self.capture_tool.shots.press.held_enough(n, Instant::now()) {
            return;
        }
        self.capture_tool.shots.pending = None;
        self.capture_sign_off();
        let said = self.record_command("start");
        info!("capture: the key was held — {said}");
    }

    /// Take the screen the pointer is on, for press `n`: now, or once what
    /// of ours is on it (the tool, its pills, a preview) has been taken off.
    fn capture_shoot(&mut self, n: u64) {
        let mut ours = false;
        if self.capture_up() {
            self.capture_tool.tool = None;
            self.capture_pills_off_now();
            self.capture_changed();
            ours = true;
        }
        // (A preview that is out is settled as it is: its file stays. Its
        // pill and the picture hanging from it go in this same step.
        // The long pill stays shut until the picture is in: the pointer
        // resting where the preview was would bring the card's own hover
        // pill out in its place, into the picture.)
        self.capture_tool.shots.shut = true;
        if self.capture_preview_off_now() {
            ours = true;
        } else {
            self.capture_tool.shots.shut = false;
        }
        if ours {
            self.after_ms(HIDE_FIRST, move |app| app.capture_grab(n));
        } else {
            self.capture_grab(n);
        }
    }

    fn capture_grab(&mut self, n: u64) {
        let pointer = crate::hypr::cursor_pos();
        let monitors = crate::hypr::all_monitors();
        let under = pointer.and_then(|(x, y)| {
            monitors
                .iter()
                .find(|m| x >= m.x && x < m.x + m.w && y >= m.y && y < m.y + m.h)
                .cloned()
        });
        let Some(monitor) = under.or_else(|| crate::hypr::focused_monitor().ok()) else {
            warn!("capture: no screen to take");
            self.capture_grabbed(n, None);
            return;
        };
        let screen = Screen {
            name: monitor.name.clone(),
            x: monitor.x.round() as i32,
            y: monitor.y.round() as i32,
            w: monitor.w.round() as i32,
            h: monitor.h.round() as i32,
        };
        let bounds = (screen.w, screen.h);
        // (The windows as they stand NOW: the editor offers these.)
        let wins = crate::capture::stack(
            crate::hypr::client_geometries()
                .into_iter()
                .filter(|c| !c.special && c.workspace == monitor.active_ws)
                .filter_map(|c| {
                    let area = Area::new(
                        (c.x - monitor.x).round() as i32,
                        (c.y - monitor.y).round() as i32,
                        c.w.round() as i32,
                        c.h.round() as i32,
                    );
                    Some(Stacked {
                        area: area.clipped(bounds)?,
                        floating: c.floating,
                        fullscreen: c.fullscreen,
                        rank: c.focus_rank,
                    })
                })
                .collect(),
        );
        let name = screen.name.clone();
        let mut rest = Some((screen, wins));
        self.desktop_off_loop(
            move || grab_screen(&name),
            move |app, png| {
                let frame =
                    png.zip(rest.take())
                        .map(|(png, (screen, wins))| Frame { png, screen, wins });
                app.capture_grabbed(n, frame);
            },
        );
    }

    /// `grim` answered for press `n`.
    fn capture_grabbed(&mut self, n: u64, frame: Option<Frame>) {
        // (The picture is in: the long pill is the pointer's again.)
        if std::mem::take(&mut self.capture_tool.shots.shut) {
            self.update_clip_reveal();
        }
        match self
            .capture_tool
            .shots
            .pending
            .as_mut()
            .filter(|p| p.n == n)
        {
            Some(p) => p.frame = Some(frame),
            // (Held into a recording meanwhile, or another press: dropped.)
            None => return,
        }
        self.capture_settle_if_ready();
    }

    /// The pending shot becomes the screenshot once both are true: the key
    /// said so, and the picture is here.
    fn capture_settle_if_ready(&mut self) {
        let shots = &mut self.capture_tool.shots;
        if !shots
            .pending
            .as_ref()
            .is_some_and(|p| p.keep && p.frame.is_some())
        {
            return;
        }
        let Some(Pending {
            frame: Some(frame), ..
        }) = shots.pending.take()
        else {
            return;
        };
        let Some(frame) = frame else {
            crate::capture::notify(tr("The screenshot could not be taken"), "", None);
            return;
        };
        let mut frame = Some(frame);
        // (The hanging picture's width in the bar's own pixels.)
        let (bar_w, s) = (self.options_size.0 as f32, self.options_scale());
        let small_w = (hang_size((16, 9), bar_w, s).0
            * self.surface_scale(crate::fractional::SurfaceKind::Options))
        .round()
        .max(1.0) as u32;
        self.desktop_off_loop(
            move || frame.take().and_then(|f| settle(f, small_w)),
            |app, kept| match kept {
                Some(kept) => app.capture_preview_show(Arc::new(kept)),
                None => crate::capture::notify(tr("The screenshot could not be taken"), "", None),
            },
        );
    }

    /// THE PREVIEW out of the card's pill, for `PREVIEW_FOR`.
    fn capture_preview_show(&mut self, kept: Arc<Kept>) {
        let shots = &mut self.capture_tool.shots;
        shots.previews += 1;
        let n = shots.previews;
        self.clip_thumb_ask(&kept.file.to_string_lossy());
        // (Its bigger picture into the bar's one big picture, for the hover.)
        if let Some(renderer) = self.options_renderer.as_mut() {
            renderer.set_video_frame(kept.small.0, kept.small.1, &kept.small.2);
            self.capture_tool.shots.hang_in = true;
        }
        self.capture_tool.shots.preview = Some(Preview {
            kept,
            left: PREVIEW_FOR,
            looked: Instant::now(),
            n,
        });
        self.clip_long_show(true, false);
        self.capture_preview_look(n);
    }

    /// A preview that is out looks whether to fold: when its time is spent,
    /// and its time does not run under the pointer.
    fn capture_preview_look(&mut self, n: u64) {
        self.after_ms(PREVIEW_LOOK, move |app| {
            let hand = app.capture_preview_hand();
            app.capture_hang_sync();
            let Some(p) = app.capture_tool.shots.preview.as_mut().filter(|p| p.n == n) else {
                return;
            };
            let now = Instant::now();
            p.left = preview_left(p.left, now.duration_since(p.looked), hand);
            p.looked = now;
            if p.left.is_zero() {
                app.capture_tool.shots.preview = None;
                app.capture_hang_sync();
                app.clip_long_show(false, false);
            } else {
                app.capture_preview_look(n);
            }
        });
    }

    /// Whether the card's long pill is held shut (a picture is being taken).
    pub(crate) fn capture_long_shut(&self) -> bool {
        self.capture_tool.shots.shut
    }

    /// Whether a preview is out.
    pub(crate) fn capture_preview_out(&self) -> bool {
        self.capture_tool.shots.preview.is_some()
    }

    /// Whether the card's long pill is the capture's now (a preview, or the
    /// held key's sign): the card's own hover leaves it alone meanwhile.
    pub(crate) fn capture_long_out(&self) -> bool {
        self.capture_preview_out() || self.capture_tool.shots.press.sign(Instant::now()).is_some()
    }

    /// The preview's ×: the shot is thrown away.
    pub(crate) fn capture_preview_close(&mut self) -> bool {
        let Some(p) = self.capture_tool.shots.preview.take() else {
            return false;
        };
        let gone = std::fs::remove_file(&p.kept.file).is_ok();
        info!(
            "capture: the screenshot {:?} thrown away ({gone})",
            p.kept.file
        );
        self.capture_hang_sync();
        self.clip_long_show(false, false);
        self.draw_options();
        true
    }

    /// A click on the preview (`capture edit`): THE EDITOR over the shot.
    pub(crate) fn capture_edit(&mut self) -> bool {
        let Some(p) = self.capture_tool.shots.preview.take() else {
            return false;
        };
        self.capture_hang_sync();
        self.clip_long_show(false, false);
        if self.capture_up() {
            self.capture_close();
        }
        let name = p.kept.screen.name.clone();
        self.capture_tool.shots.opening = Some(p.kept);
        if !self.capture_follow(&name) {
            self.capture_open_on(&name);
        }
        true
    }

    /// A click on the card's long pill while a preview is on it: its × (the
    /// pill's last stretch) throws the shot away, the rest opens the editor.
    pub(crate) fn capture_preview_clicked(&mut self) {
        // (The × is on the pill's row only: the picture hanging under it is
        // the pill's, all of it.)
        let (r, ph) = (self.clip_rect(), self.options_pill_h());
        let on_close = self
            .options_ptr
            .is_some_and(|(x, y)| x >= r.x + r.w - ph * 1.2 && y <= r.y + ph);
        if on_close {
            self.capture_preview_close();
        } else {
            self.capture_edit();
        }
    }

    /// The held key's sign: frames while it shows, the long pill out for it.
    fn capture_sign_frame(&mut self, n: u64) {
        let press = self.capture_tool.shots.press;
        if press.n != n || press.sign(Instant::now()).is_none() {
            return;
        }
        self.clip_long_show(true, false);
        self.draw_options();
        self.after_ms(SIGN_FRAME, move |app| app.capture_sign_frame(n));
    }

    /// The key is up (or became a recording): its sign goes.
    fn capture_sign_off(&mut self) {
        if !self.capture_preview_out() {
            self.clip_long_show(false, false);
        }
        self.draw_options();
    }

    /// Whether the hand is on the preview: the pointer on the card's pill,
    /// its long pill or the picture hanging from it (one rect), or a rig's.
    fn capture_preview_hand(&self) -> bool {
        self.capture_tool.shots.hover_forced
            || matches!(
                self.options_hover,
                Some(PillId::Clipboard | PillId::ClipboardBox)
            )
    }

    /// `capture preview hover|unhover`: a rig's pointer on the preview.
    pub(crate) fn capture_preview_hover(&mut self, on: bool) {
        self.capture_tool.shots.hover_forced = on && self.capture_preview_out();
        self.capture_hang_sync();
    }

    /// The picture hangs while the hand is on a preview that is out, and
    /// folds when either ends (eased; its clock runs only while it moves).
    pub(crate) fn capture_hang_sync(&mut self) {
        if !self.capture_preview_out() {
            self.capture_tool.shots.hover_forced = false;
        }
        let want = self.capture_preview_out() && self.capture_preview_hand();
        let shots = &mut self.capture_tool.shots;
        if shots.hang != want || (shots.hang_t > 0.0 && !want) {
            shots.hang = want;
            self.schedule_tick(
                |a| {
                    (
                        &mut a.capture_tool.shots.hang_pending,
                        &mut a.capture_tool.shots.hang_last,
                    )
                },
                App::tick_capture_hang,
            );
        }
    }

    fn tick_capture_hang(&mut self) {
        let now = Instant::now();
        let shots = &mut self.capture_tool.shots;
        let dt = shots
            .hang_last
            .map_or(0.0, |l| now.duration_since(l).as_secs_f32().min(0.05));
        shots.hang_last = Some(now);
        let (t, moving) = crate::animation::ease_toward(
            shots.hang_t,
            f32::from(u8::from(shots.hang)),
            dt,
            crate::animation::MORPH_RATE,
            crate::animation::settle_t(HANG_W * 0.6),
        );
        shots.hang_t = t;
        self.sync_options_input();
        self.draw_options();
        if moving {
            self.schedule_tick(
                |a| {
                    (
                        &mut a.capture_tool.shots.hang_pending,
                        &mut a.capture_tool.shots.hang_last,
                    )
                },
                App::tick_capture_hang,
            );
        } else {
            self.capture_tool.shots.hang_last = None;
            self.capture_hang_let_go();
        }
    }

    /// The bar's big picture is given back once nothing shows it.
    fn capture_hang_let_go(&mut self) {
        let shots = &mut self.capture_tool.shots;
        if shots.preview.is_none() && shots.hang_t <= 0.0 && std::mem::take(&mut shots.hang_in) {
            if let Some(renderer) = self.options_renderer.as_mut() {
                renderer.drop_video();
            }
        }
    }

    /// The preview AND the picture hanging from it off the banner NOW, in
    /// one step and with no easing (a picture is about to be taken, or the
    /// live tool opens); the shot stays as it was saved. Whether anything of
    /// them was out.
    pub(crate) fn capture_preview_off_now(&mut self) -> bool {
        let shots = &mut self.capture_tool.shots;
        let was = shots.preview.take().is_some() || shots.hang_t > 0.0;
        if !was {
            return false;
        }
        (shots.hang, shots.hang_t, shots.hover_forced) = (false, 0.0, false);
        self.capture_hang_let_go();
        self.clip_long_show(false, true);
        self.sync_options_input();
        self.draw_options();
        true
    }

    /// How far down the picture hangs (0: not at all).
    pub(crate) fn capture_hang_t(&self) -> f32 {
        self.capture_tool.shots.hang_t
    }

    /// The hanging picture's size on the bar (`None`: no preview is out).
    fn capture_hang_size(&self) -> Option<(f32, f32)> {
        let p = self.capture_tool.shots.preview.as_ref()?;
        Some(hang_size(
            p.kept.px,
            self.options_size.0 as f32,
            self.options_scale(),
        ))
    }

    /// How much taller than its pill the long pill is for the picture, when
    /// it hangs whole (`capture_hang_extra`: now).
    pub(crate) fn capture_hang_full(&self) -> f32 {
        self.capture_hang_size()
            .map_or(0.0, |(_, h)| h + 2.0 * HANG_PAD * self.options_scale())
    }

    pub(crate) fn capture_hang_extra(&self) -> f32 {
        self.capture_hang_t().clamp(0.0, 1.0) * self.capture_hang_full()
    }

    /// The key's and the preview's state, for the state line.
    pub(crate) fn capture_shot_state(&self) -> String {
        let s = &self.capture_tool.shots;
        let preview = s.preview.as_ref().map_or_else(
            || "none".to_owned(),
            |p| {
                format!(
                    "out {:?} {}x{}",
                    p.kept.file.file_name().unwrap_or_default(),
                    p.kept.px.0,
                    p.kept.px.1
                )
            },
        );
        let picture = if s.hang {
            "hangs"
        } else if s.hang_t > 0.0 {
            "folding"
        } else {
            "folded"
        };
        format!(
            "key {} preview {preview} picture {picture}",
            if s.press.is_down() { "down" } else { "up" }
        )
    }

    /// SAVE in the editor: `area` of the still cut out, written over the
    /// shot's file and put on the clipboard in the whole one's place — off
    /// the loop. (The whole screen: nothing to do, the shot stays.)
    pub(crate) fn capture_still_save(&mut self, kept: Arc<Kept>, area: Area) {
        let at = crop_px(area, (kept.screen.w, kept.screen.h), kept.px);
        let spawned = std::thread::Builder::new()
            .name("capture-cut".into())
            .spawn(move || {
                let done = cut(&kept.png, at).is_some_and(|part| {
                    let copied = crate::capture::copy("image/png", &part);
                    let written = replace(&kept.file, &part);
                    info!(
                        "capture: {:?} cut to {at:?} (file: {written}, clipboard: {copied})",
                        kept.file
                    );
                    written
                });
                if !done {
                    crate::capture::notify(tr("The screenshot could not be saved"), "", None);
                }
            });
        if let Err(e) = spawned {
            warn!("capture: cannot cut the screenshot ({e})");
        }
    }

    /// COPY TEXT in the editor: the words in `area` of the still.
    pub(crate) fn capture_still_text(&mut self, kept: Arc<Kept>, area: Area) {
        let at = crop_px(area, (kept.screen.w, kept.screen.h), kept.px);
        let spawned = std::thread::Builder::new()
            .name("capture-read".into())
            .spawn(move || {
                let tmp = crate::capture::scratch_png();
                let written =
                    cut(&kept.png, at).is_some_and(|part| std::fs::write(&tmp, part).is_ok());
                if written {
                    crate::capture::read_picture(&tmp, &format!("{at:?} of {:?}", kept.file));
                } else {
                    crate::capture::notify(tr("The text could not be read"), "", None);
                }
                let _ = std::fs::remove_file(&tmp);
            });
        if let Err(e) = spawned {
            warn!("capture: cannot read the screenshot ({e})");
        }
    }

    /// THE CARD'S LONG PILL as the capture's (`push_clip_pill` asks first):
    /// the preview — its small picture, `Screenshot`, a × — or the held
    /// key's sign, filling. False: it is the card's own.
    pub(crate) fn push_capture_long(
        &self,
        scene: &mut Scene,
        rect: Rect,
        ink: [f32; 4],
        a: f32,
    ) -> bool {
        let shots = &self.capture_tool.shots;
        let sign = shots.press.sign(Instant::now());
        if shots.preview.is_none() && sign.is_none() {
            return false;
        }
        if a <= 0.01 {
            return true;
        }
        let s = self.options_scale();
        // (The pill's own row: under it, the picture that hangs on hover.)
        let ph = self.options_pill_h().min(rect.h);
        let hang = shots.hang_t.clamp(0.0, 1.0);
        if let (Some((pw, pic_h)), true) = (self.capture_hang_size(), hang > 0.01 && shots.hang_in)
        {
            let at = Rect::new(
                (rect.x + (rect.w - pw) / 2.0).round(),
                (rect.y + ph + HANG_PAD * s).round(),
                pw,
                pic_h,
            );
            // (Cut by the box as it grows: the picture is uncovered, not moved.)
            scene.video = Some((
                scene.grids.len(),
                IconInst {
                    rect: at,
                    layer: 0,
                    tint: [at.x, at.y, at.x + at.w, at.y + at.h],
                    ring: -1.6,
                    plate: [
                        HANG_CORNER * s,
                        a * ((hang - 0.2) / 0.6).clamp(0.0, 1.0),
                        0.0,
                        0.0,
                    ],
                },
            ));
            scene.grids.push(crate::content::GridContent {
                clip: rect,
                ..Default::default()
            });
        }
        let ty = rect.y + (ph - LINE_PX * s) / 2.0;
        let word = |text: &str,
                    x: f32,
                    max_w: f32,
                    family: Option<&'static str>,
                    k: f32,
                    centered: bool| Label {
            text: text.to_owned(),
            pos: (x, ty),
            max_w,
            font_px: FONT_PX * s,
            line_px: LINE_PX * s,
            centered,
            dim: false,
            cache: true,
            family,
            color: Some([ink[0], ink[1], ink[2], ink[3] * a * k]),
            clip: Some(rect),
        };
        let lead = Rect::new(rect.x + PILL_PAD_X * s * 0.6, rect.y, ph, ph);
        let tx = lead.x + lead.w + PILL_PAD_X * s * 0.5;
        // THE HELD KEY: the pill fills as the task pill does, and says what
        // comes when it is full.
        if let Some(fill) = sign {
            let cap = ph / 2.0;
            let fw = (rect.w * fill).max(2.0 * cap).min(rect.w);
            scene.rects.push(RectInst {
                rect: Rect::new(rect.x, rect.y, fw, ph),
                radius: cap,
                color: [ink[0], ink[1], ink[2], 0.14 * a],
                glass: 0.0,
                border: 0.0,
            });
            scene.labels.push(word(
                GLYPH_FILM,
                lead.x + lead.w / 2.0,
                lead.w + 4.0,
                Some(NERD),
                1.0,
                true,
            ));
            scene.labels.push(word(
                tr("Keep holding to record the screen"),
                tx,
                (rect.x + rect.w - PILL_PAD_X * s - tx).max(0.0),
                None,
                1.0,
                false,
            ));
            return true;
        }
        let Some(p) = shots.preview.as_ref() else {
            return true;
        };
        // THE PREVIEW: the shot small (the camera until its picture is in),
        // its word, and a × at the pill's end.
        match self.clip_thumb_of(&p.kept.file.to_string_lossy()) {
            Some(layer) => {
                let side = ph - 4.0 * s;
                let tile = Rect::new(
                    lead.x + (lead.w - side) / 2.0,
                    rect.y + (ph - side) / 2.0,
                    side,
                    side,
                );
                scene.icons.push(IconInst {
                    rect: tile,
                    layer,
                    tint: [0.0; 4],
                    ring: -1.0,
                    plate: crate::content::NO_PLATE,
                });
                scene.icon_fade.push(a);
            }
            None => scene.labels.push(word(
                GLYPH_SHOT,
                lead.x + lead.w / 2.0,
                lead.w + 4.0,
                Some(NERD),
                1.0,
                true,
            )),
        }
        let close = Rect::new(rect.x + rect.w - ph, rect.y, ph, ph);
        scene.labels.push(word(
            tr("Screenshot"),
            tx,
            (close.x - tx).max(0.0),
            None,
            1.0,
            false,
        ));
        let over_close = self.options_hover == Some(PillId::ClipboardBox)
            && self
                .options_ptr
                .is_some_and(|(x, y)| x >= rect.x + rect.w - ph * 1.2 && y <= rect.y + ph);
        scene.labels.push(word(
            "×",
            close.x + close.w / 2.0,
            close.w + 4.0,
            None,
            if over_close { 1.0 } else { 0.55 },
            true,
        ));
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ms(n: u64) -> Duration {
        Duration::from_millis(n)
    }

    #[test]
    fn the_hanging_picture_is_360_wide_at_the_bars_scale_a_third_of_the_screen_at_most_and_the_shots_shape(
    ) {
        assert_eq!(hang_size((1440, 900), 1440.0, 1.0), (360.0, 225.0));
        // A bigger bar: bigger with it.
        assert_eq!(hang_size((2880, 1800), 1920.0, 1.5), (540.0, 338.0));
        // A narrow screen: a third of it, no more.
        assert_eq!(hang_size((1024, 768), 900.0, 1.0), (300.0, 225.0));
        // A screen turned on its side keeps its shape.
        let (w, h) = hang_size((1080, 1920), 1080.0, 1.0);
        assert_eq!((w, h), (360.0, 640.0));
    }

    #[test]
    fn a_previews_time_stands_still_under_the_hand_and_goes_on_when_it_leaves() {
        let look = ms(250);
        // Alone: its four seconds, then gone.
        let (mut left, mut looks) = (PREVIEW_FOR, 0);
        while !left.is_zero() {
            left = preview_left(left, look, false);
            looks += 1;
        }
        assert_eq!(looks, 16);
        // The hand comes after one second and stays ten: nothing is spent
        // meanwhile, and what was left is left when it goes.
        let mut left = PREVIEW_FOR;
        for _ in 0..4 {
            left = preview_left(left, look, false);
        }
        assert_eq!(left, ms(3000));
        for _ in 0..40 {
            left = preview_left(left, look, true);
        }
        assert_eq!(left, ms(3000), "its own time goes on from where it stood");
        left = preview_left(left, look, false);
        assert_eq!(left, ms(2750));
        // The hand comes at the very end: it is not folded from under the
        // hand, nor the moment the hand leaves.
        let left = preview_left(ms(100), look, true);
        assert_eq!(left, PREVIEW_AFTER_HAND);
        assert!(!preview_left(left, look, false).is_zero());
    }

    #[test]
    fn a_tap_is_a_screenshot() {
        let t0 = Instant::now();
        let mut key = Press::default();
        assert_eq!(key.press(t0, false), Some(OnPress::Shoot));
        // (A tap shows no sign.)
        assert_eq!(key.sign(t0 + ms(120)), None);
        assert!(
            key.release(t0 + ms(120)),
            "the shot taken at the press is the screenshot"
        );
        // Its hold timer, firing later, finds the key up.
        assert!(!key.held_enough(key.n, t0 + HOLD));
        assert!(!key.is_down());
    }

    #[test]
    fn a_hold_becomes_a_recording_and_its_release_is_nothing() {
        let t0 = Instant::now();
        let mut key = Press::default();
        assert_eq!(key.press(t0, false), Some(OnPress::Shoot));
        let n = key.n;
        // The sign: from 300 ms, growing to whole at 1.5 s.
        assert_eq!(key.sign(t0 + ms(299)), None);
        assert_eq!(key.sign(t0 + SIGN_FROM), Some(0.0));
        let half = key.sign(t0 + ms(900)).unwrap();
        assert!((half - 0.5).abs() < 0.01, "{half}");
        assert_eq!(key.sign(t0 + HOLD), Some(1.0));
        // Not yet at 1.4 s; at 1.5 s, once.
        assert!(!key.held_enough(n, t0 + ms(1400)));
        assert!(key.held_enough(n, t0 + HOLD));
        assert!(!key.held_enough(n, t0 + HOLD + ms(10)), "said once");
        assert_eq!(key.sign(t0 + HOLD + ms(10)), None, "the sign goes with it");
        assert!(
            !key.release(t0 + ms(2100)),
            "the release after it does nothing"
        );
    }

    #[test]
    fn a_release_just_before_the_hold_is_still_a_screenshot_and_one_after_is_not() {
        let t0 = Instant::now();
        let mut key = Press::default();
        key.press(t0, false);
        assert!(key.release(t0 + ms(1499)));
        // A hold whose timer was late: the release itself says it was held.
        key.press(t0 + ms(5000), false);
        assert!(!key.release(t0 + ms(5000) + HOLD));
    }

    #[test]
    fn a_press_while_recording_stops_it_and_shoots_nothing() {
        let t0 = Instant::now();
        let mut key = Press::default();
        assert_eq!(key.press(t0, true), Some(OnPress::StopRecording));
        assert_eq!(key.sign(t0 + ms(800)), None);
        assert!(
            !key.held_enough(key.n, t0 + HOLD),
            "never a new recording out of the press that stopped one"
        );
        assert!(!key.release(t0 + ms(2000)));
        // The next press is an ordinary one.
        assert_eq!(key.press(t0 + ms(3000), false), Some(OnPress::Shoot));
    }

    #[test]
    fn a_repeat_of_a_key_that_is_down_is_not_a_press_and_an_old_timer_is_not_this_press() {
        let t0 = Instant::now();
        let mut key = Press::default();
        key.press(t0, false);
        let first = key.n;
        assert_eq!(key.press(t0 + ms(40), false), None);
        assert!(key.release(t0 + ms(100)));
        assert!(!key.release(t0 + ms(110)), "a release with no press");
        // A second press: the first one's timer does not make it a recording.
        key.press(t0 + ms(1000), false);
        assert!(!key.held_enough(first, t0 + ms(1000) + HOLD));
        assert!(key.held_enough(key.n, t0 + ms(1000) + HOLD));
    }

    #[test]
    fn a_region_is_cut_in_the_pictures_own_pixels() {
        // Scale 1: as it is.
        assert_eq!(
            crop_px(Area::new(300, 100, 640, 400), (1440, 900), (1440, 900)),
            (300, 100, 640, 400)
        );
        // Scale 2.
        assert_eq!(
            crop_px(Area::new(300, 100, 640, 400), (1440, 900), (2880, 1800)),
            (600, 200, 1280, 800)
        );
        // Scale 1.6 (a 2560×1600 panel shown as 1600×1000).
        assert_eq!(
            crop_px(Area::new(10, 15, 101, 33), (1600, 1000), (2560, 1600)),
            (16, 24, 162, 53)
        );
        // Scale 0.75 (a 1440×900 panel shown as 1920×1200): each edge rounded.
        assert_eq!(
            crop_px(Area::new(10, 10, 101, 51), (1920, 1200), (1440, 900)),
            (8, 8, 75, 38)
        );
        // The whole screen is the whole picture, at any scale.
        assert_eq!(
            crop_px(Area::new(0, 0, 1920, 1200), (1920, 1200), (1440, 900)),
            (0, 0, 1440, 900)
        );
        assert_eq!(
            crop_px(Area::new(0, 0, 1600, 1000), (1600, 1000), (2560, 1600)),
            (0, 0, 2560, 1600)
        );
    }

    #[test]
    fn two_regions_that_meet_on_the_screen_meet_in_the_picture() {
        for (logical, px) in [
            ((1920, 1200), (1440, 900)),
            ((1600, 1000), (2560, 1600)),
            ((1440, 900), (1440, 900)),
        ] {
            for split in [1, 7, 333, 801, 1001] {
                let left = crop_px(Area::new(0, 0, split, 100), logical, px);
                let right = crop_px(Area::new(split, 0, logical.0 - split, 100), logical, px);
                assert_eq!(left.0 + left.2, right.0, "{logical:?} {px:?} {split}");
                assert_eq!(right.0 + right.2, px.0);
            }
        }
    }

    #[test]
    fn a_cut_never_leaves_the_picture_and_is_never_empty() {
        let px = (1440, 900);
        let (x, y, w, h) = crop_px(Area::new(1430, 890, 500, 500), (1440, 900), px);
        assert_eq!((x + w, y + h), px);
        let (_, _, w, h) = crop_px(Area::new(5, 5, 1, 1), (1920, 1200), px);
        assert!(w >= 1 && h >= 1);
        let (x, y, w, h) = crop_px(Area::new(1920, 1200, 10, 10), (1920, 1200), px);
        assert!(x < px.0 && y < px.1 && x + w <= px.0 && y + h <= px.1 && w >= 1 && h >= 1);
    }

    #[test]
    fn the_cut_takes_the_whole_files_place_under_its_own_name() {
        let file = Path::new("/home/u/Pictures/Screenshots/Screenshot 2026-11-16 22-03-21.png");
        let part = part_beside(file);
        assert_eq!(
            part,
            Path::new("/home/u/Pictures/Screenshots/.Screenshot 2026-11-16 22-03-21.png.part")
        );
        assert_eq!(
            part.parent(),
            file.parent(),
            "beside it: the rename is one step"
        );
        // On the disk: the picture is cut, and its file is the cut one after.
        let dir = std::env::temp_dir().join(format!("wr-capture-cut-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let whole =
            image::RgbaImage::from_fn(40, 30, |x, y| image::Rgba([x as u8, y as u8, 7, 255]));
        let mut png = std::io::Cursor::new(Vec::new());
        whole.write_to(&mut png, image::ImageFormat::Png).unwrap();
        let file = dir.join("Screenshot.png");
        std::fs::write(&file, png.get_ref()).unwrap();
        let part = cut(png.get_ref(), (10, 5, 20, 12)).expect("cut");
        assert!(replace(&file, &part));
        let after = image::open(&file).unwrap().to_rgba8();
        assert_eq!((after.width(), after.height()), (20, 12));
        assert_eq!(
            after.get_pixel(0, 0).0,
            [10, 5, 7, 255],
            "from where the region began"
        );
        assert_eq!(
            std::fs::read_dir(&dir).unwrap().count(),
            1,
            "nothing left beside it"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }
}
