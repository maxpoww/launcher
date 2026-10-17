//! THE PENCIL in Quick Look (Max, 2026-10-16, the lab: the same frame turns
//! into a small editor; its head becomes Cancel · Save copy · Save — "Save"
//! writes over the picture, its original kept; "Save copy" writes beside it).
//! What it does to the picture is `peek_edit.rs`; this is the frame's side:
//! its toolbar (Crop · Draw · Text · Blur · Tint · Turn, the inks, Undo),
//! the tint's sliders, a line drawn live, a marquee, words being typed.
//!
//! The picture edited is the one on the screen (read again whole and brought
//! to the screen's size as the pencil is taken, off the loop); a line being
//! drawn changes only the tiles it crosses; anything else draws it again.
//! Saving reads the whole picture again and does the same list to it, off the
//! loop.

use std::collections::HashSet;
use std::path::PathBuf;
use std::time::Instant;

use image::RgbaImage;
use smithay_client_toolkit::reexports::calloop;
use smithay_client_toolkit::seat::keyboard::Keysym;
use tracing::warn;

use crate::content::{GridContent, IconInst, Label, Rect, RectInst, FONT_BOLD, WINDOW_CORNER};
use crate::i18n::{fill, tr};
use crate::peek_edit::{Ink, Op, SaveAs, Tint, INKS};
use crate::quicklook::{text_label, Pic, HEAD, STEP, TILE};
use crate::App;

/// A line's width and words' height, as shares of the picture.
const LINE: f32 = 0.006;
const WORDS: f32 = 0.055;
/// The tint's sliders go this far each way.
const LIGHT: (f32, f32) = (0.5, 1.5);

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum Tool {
    Crop,
    Draw,
    Text,
    Blur,
    Tint,
    /// A PDF's: the highlighter, and the signature.
    Mark,
    Sign,
}

/// What comes back from the editor's threads.
pub(crate) enum Msg {
    /// The picture to edit, at the screen's size (straight RGBA).
    Base(u64, RgbaImage),
    /// Saved there (or why not).
    Saved(u64, Result<PathBuf, String>),
    /// A video's moment kept as a photo there (or why not).
    Snapped(Result<PathBuf, String>),
}

pub(crate) struct Edit {
    tag: u64,
    base: Option<RgbaImage>,
    /// `base` with everything done to it, and its tiles.
    shown: Option<RgbaImage>,
    pub(crate) pic: Option<Pic>,
    /// Tiles to write again (`None`: all of them, `Some(empty)`: none); the
    /// drawing's number, for the upload to know.
    pub(crate) dirty: Option<HashSet<u32>>,
    pub(crate) rev: u64,
    ops: Vec<Op>,
    tint: Tint,
    tool: Tool,
    ink: Ink,
    /// A line being drawn; a marquee being pulled (from, to); one waiting
    /// for "Apply crop"; words being typed and where.
    live: Option<Vec<(f32, f32)>>,
    pull: Option<((f32, f32), (f32, f32))>,
    crop: Option<[f32; 4]>,
    typing: Option<((f32, f32), String)>,
    /// The tint's slider in hand.
    slider: Option<usize>,
    /// Cancel asked once with changes made: the second asks nothing.
    armed: bool,
    saving: bool,
    /// The last tint drawn (they come fast while a slider moves).
    tint_at: Option<Instant>,
    /// A PDF's page being marked (its marks become the PDF's annotations).
    pdf_page: Option<u32>,
    /// The signature: being drawn in its box (its lines; whether a line is
    /// in hand), and the one to place.
    pad: Option<Vec<Vec<(f32, f32)>>>,
    pad_live: bool,
    sig: Option<Vec<Vec<(f32, f32)>>>,
}

impl Edit {
    fn changed(&self) -> bool {
        !self.ops.is_empty() || !self.tint.is_none()
    }
}

/// The parts of the editor's frame at `r`.
pub(crate) struct EditParts {
    pub cancel: Rect,
    pub copy: Rect,
    pub save: Rect,
    /// The toolbar's pill and its buttons.
    pub bar: Rect,
    pub tools: Vec<(Btn, Rect)>,
    /// The tint's panel, its three sliders' lines, its grey switch.
    pub tint: Rect,
    pub sliders: [Rect; 3],
    pub grey: Rect,
}

#[derive(Clone, Copy, PartialEq, Debug)]
pub(crate) enum Btn {
    Tool(Tool),
    Turn,
    Ink(usize),
    Apply,
    Undo,
    /// Draw the signature again.
    NewSign,
}

fn btn_word(b: Btn) -> &'static str {
    match b {
        Btn::Tool(Tool::Crop) => tr("Crop"),
        Btn::Tool(Tool::Draw) => tr("Draw"),
        Btn::Tool(Tool::Text) => tr("Text"),
        Btn::Tool(Tool::Blur) => tr("Blur"),
        Btn::Tool(Tool::Tint) => tr("Tint"),
        Btn::Turn => tr("Turn"),
        Btn::Apply => tr("Apply crop"),
        Btn::Undo => tr("Undo"),
        Btn::NewSign => tr("New signature"),
        Btn::Tool(Tool::Mark) => tr("Highlight"),
        Btn::Tool(Tool::Sign) => tr("Sign"),
        Btn::Ink(_) => "",
    }
}

fn word_w(word: &str, px: f32) -> f32 {
    word.chars().count() as f32 * px * 0.56
}

pub(crate) fn edit_parts(r: Rect, crop_waiting: bool) -> EditParts {
    edit_parts_for(r, crop_waiting, None)
}

/// The same for a PDF's page (`pdf`: whether a signature is kept, to offer
/// drawing a new one): its own tools.
pub(crate) fn edit_parts_for(r: Rect, crop_waiting: bool, pdf: Option<bool>) -> EditParts {
    let save_w = word_w(tr("Save"), 12.0) + 28.0;
    let copy_w = word_w(tr("Save copy"), 12.0) + 28.0;
    let cancel_w = word_w(tr("Discard changes?"), 12.0).max(word_w(tr("Cancel"), 12.0)) + 28.0;
    let save = Rect::new(r.x + r.w - 12.0 - save_w, r.y + 10.0, save_w, 26.0);
    let copy = Rect::new(save.x - 8.0 - copy_w, r.y + 10.0, copy_w, 26.0);
    let cancel = Rect::new(copy.x - 8.0 - cancel_w, r.y + 10.0, cancel_w, 26.0);
    // The toolbar: words and dots in a row, centred at the foot.
    let tools: &[Tool] = if pdf.is_some() {
        &[Tool::Mark, Tool::Draw, Tool::Text, Tool::Sign]
    } else {
        &[Tool::Crop, Tool::Draw, Tool::Text, Tool::Blur, Tool::Tint]
    };
    let mut items: Vec<(Btn, f32)> = tools
        .iter()
        .map(|&t| (Btn::Tool(t), word_w(btn_word(Btn::Tool(t)), 12.0) + 22.0))
        .collect();
    if pdf.is_none() {
        items.push((Btn::Turn, word_w(btn_word(Btn::Turn), 12.0) + 22.0));
    }
    if pdf == Some(true) {
        items.push((Btn::NewSign, word_w(btn_word(Btn::NewSign), 12.0) + 22.0));
    }
    for i in 0..INKS.len() {
        items.push((Btn::Ink(i), 22.0));
    }
    if crop_waiting {
        items.push((Btn::Apply, word_w(btn_word(Btn::Apply), 12.0) + 22.0));
    }
    items.push((Btn::Undo, word_w(btn_word(Btn::Undo), 12.0) + 22.0));
    let gap = 4.0;
    let total: f32 =
        items.iter().map(|(_, w)| w).sum::<f32>() + gap * (items.len() as f32 - 1.0) + 12.0;
    let bar = Rect::new(
        r.x + (r.w - total) / 2.0,
        r.y + r.h - 16.0 - 34.0,
        total,
        34.0,
    );
    let mut x = bar.x + 6.0;
    let tools = items
        .into_iter()
        .map(|(b, w)| {
            let rect = if matches!(b, Btn::Ink(_)) {
                Rect::new(x, bar.y + 6.0, 22.0, 22.0)
            } else {
                Rect::new(x, bar.y + 4.0, w, 26.0)
            };
            x += w + gap;
            (b, rect)
        })
        .collect();
    let tint = Rect::new(
        r.x + (r.w - 300.0) / 2.0,
        bar.y - 10.0 - 132.0,
        300.0,
        132.0,
    );
    let sliders =
        [0, 1, 2].map(|i| Rect::new(tint.x + 110.0, tint.y + 14.0 + i as f32 * 28.0, 170.0, 20.0));
    let grey = Rect::new(tint.x + 110.0, tint.y + 98.0, 170.0, 24.0);
    EditParts {
        cancel,
        copy,
        save,
        bar,
        tools,
        tint,
        sliders,
        grey,
    }
}

/// The editor's parts for `edit` in a frame `r`.
fn parts_of(r: Rect, edit: &Edit) -> EditParts {
    let pdf = edit
        .pdf_page
        .map(|_| edit.sig.is_some() && edit.tool == Tool::Sign);
    edit_parts_for(r, edit.crop.is_some(), pdf)
}

/// The signature's box in a frame `r`: where it is drawn (3 : 1), Clear,
/// Use, Cancel.
pub(crate) struct PadParts {
    pub panel: Rect,
    pub draw: Rect,
    pub clear: Rect,
    pub done: Rect,
    pub cancel: Rect,
}

pub(crate) fn pad_parts(r: Rect) -> PadParts {
    let w = (r.w - 80.0).clamp(240.0, 480.0);
    let draw_h = w / 3.0;
    let panel = Rect::new(
        r.x + (r.w - w - 28.0) / 2.0,
        r.y + HEAD + ((r.h - HEAD) - draw_h - 92.0) / 2.0,
        w + 28.0,
        draw_h + 92.0,
    );
    let draw = Rect::new(panel.x + 14.0, panel.y + 38.0, w, draw_h);
    let by = draw.y + draw_h + 10.0;
    let done_w = word_w(tr("Use"), 12.0) + 28.0;
    let clear_w = word_w(tr("Clear"), 12.0) + 28.0;
    let cancel_w = word_w(tr("Cancel"), 12.0) + 28.0;
    let done = Rect::new(draw.x + draw.w - done_w, by, done_w, 26.0);
    let clear = Rect::new(done.x - 8.0 - clear_w, by, clear_w, 26.0);
    let cancel = Rect::new(clear.x - 8.0 - cancel_w, by, cancel_w, 26.0);
    PadParts {
        panel,
        draw,
        clear,
        done,
        cancel,
    }
}

/// A drawing of the edited picture never numbered before (a counter per
/// edit started at 0 again, and its first drawing was taken for the last
/// edit's, already in the layers).
fn next_rev() -> u64 {
    static N: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);
    N.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
}

/// Where a picture of `w`×`h` stands in `body`: as big as fits, centred.
pub(crate) fn pic_rect(body: Rect, w: u32, h: u32) -> Rect {
    let k = (body.w / w.max(1) as f32).min(body.h / h.max(1) as f32);
    let (pw, ph) = (w as f32 * k, h as f32 * k);
    Rect::new(
        body.x + (body.w - pw) / 2.0,
        body.y + (body.h - ph) / 2.0,
        pw,
        ph,
    )
}

/// One tile of a straight picture, premultiplied, its mips made.
fn tile(img: &RgbaImage, c: u32, r: u32) -> Vec<u8> {
    let (w, h) = img.dimensions();
    let mut base = vec![0u8; (TILE * TILE * 4) as usize];
    for ty in 0..TILE {
        let y = (r * STEP + ty) as i64 - 1;
        if y > h as i64 {
            break;
        }
        let y = y.clamp(0, h as i64 - 1) as u32;
        for tx in 0..TILE {
            let x = (c * STEP + tx) as i64 - 1;
            if x > w as i64 {
                break;
            }
            let x = x.clamp(0, w as i64 - 1) as u32;
            let p = img.get_pixel(x, y).0;
            let a = u32::from(p[3]);
            let at = ((ty * TILE + tx) * 4) as usize;
            base[at..at + 4].copy_from_slice(&[
                (u32::from(p[0]) * a / 255) as u8,
                (u32::from(p[1]) * a / 255) as u8,
                (u32::from(p[2]) * a / 255) as u8,
                p[3],
            ]);
        }
    }
    crate::apps::with_mips(base)
}

fn pic_of(img: &RgbaImage) -> Pic {
    let (w, h) = img.dimensions();
    let (cols, rows) = (w.div_ceil(STEP), h.div_ceil(STEP));
    let tiles = (0..rows)
        .flat_map(|r| (0..cols).map(move |c| (c, r)))
        .map(|(c, r)| tile(img, c, r))
        .collect();
    Pic {
        w,
        h,
        cols,
        rows,
        tiles,
    }
}

impl App {
    /// Whether the pencil may be taken on what Quick Look shows: a picture
    /// of a kind it can read whole (not a PDF's page, a video's frame…).
    pub(crate) fn peek_editable(&self) -> bool {
        let Some(up) = self.peek.up.as_ref() else {
            return false;
        };
        let name = up
            .path
            .file_name()
            .map(|n| n.to_string_lossy().to_ascii_lowercase())
            .unwrap_or_default();
        let picture = crate::thumb_more::is_raw(&name)
            || crate::peek_edit::writable(&name)
            || name.ends_with(".gif")
            || name.ends_with(".avif");
        let shown_picture = self.peek_shown().is_some_and(|(_, p)| {
            // (Not a moving picture: saved, it would stand still.)
            matches!(p.body, crate::quicklook::Body::Picture(_))
                && p.pages == 0
                && p.video.is_none()
                && p.anim.is_none()
        });
        let text = self.peek_shown().is_some_and(|(_, p)| p.whole_text);
        let cut = self.trim_able().is_some();
        let pdf = self.peek_pdf_page().is_some();
        ((picture && shown_picture) || text || cut || pdf)
            && up.edit.is_none()
            && up.text_edit.is_none()
            && up.trim_edit.is_none()
            && !up.closing
    }

    /// The page of a PDF on screen, when the pencil can mark it (MuPDF is here).
    pub(crate) fn peek_pdf_page(&self) -> Option<u32> {
        let up = self.peek.up.as_ref()?;
        let (key, p) = self.peek_shown()?;
        let pdf = up
            .path
            .extension()
            .is_some_and(|e| e.eq_ignore_ascii_case("pdf"));
        (pdf && p.pages > 0 && crate::launch::on_path("mutool")).then_some(key.1)
    }

    pub(crate) fn peek_edit_channel(&mut self) -> Option<calloop::channel::Sender<Msg>> {
        if self.peek.edits.is_none() {
            let (sender, receiver) = calloop::channel::channel();
            let inserted = self
                .loop_handle
                .insert_source(receiver, |event, _, app: &mut App| {
                    if let calloop::channel::Event::Msg(msg) = event {
                        app.edit_msg(msg);
                    }
                });
            if let Err(e) = inserted {
                warn!("quick look: {e}");
                return None;
            }
            self.peek.edits = Some(sender);
        }
        self.peek.edits.clone()
    }

    /// Take the pencil: the picture is read again whole, off the loop.
    pub(crate) fn edit_begin(&mut self) {
        if !self.peek_editable() {
            return;
        }
        // (Fitted first: a zoomed photo's sharp part would be laid over the
        // edit, and after it over what was saved.)
        if let Some(up) = self.peek.up.as_mut() {
            up.zoom = crate::peek_zoom::Zoom::new();
        }
        // Text is written straight in the frame (`peek_text`).
        if self.peek_shown().is_some_and(|(_, p)| p.whole_text) {
            return self.text_begin();
        }
        // A video or a song is cut (`peek_trim`).
        if self.trim_able().is_some() {
            return self.trim_begin();
        }
        let Some(tx) = self.peek_edit_channel() else {
            return;
        };
        self.peek.tag += 1;
        let tag = self.peek.tag;
        let max = self.peek_max();
        // A PDF's page: marked, its marks written as annotations.
        let pdf_page = self.peek_pdf_page();
        let Some(up) = self.peek.up.as_mut() else {
            return;
        };
        up.stop();
        let first_tool = if pdf_page.is_some() {
            Tool::Mark
        } else {
            Tool::Draw
        };
        up.edit = Some(Edit {
            tag,
            base: None,
            shown: None,
            pic: None,
            dirty: None,
            rev: 0,
            ops: Vec::new(),
            tint: Tint::default(),
            tool: first_tool,
            ink: if pdf_page.is_some() {
                crate::peek_edit::SIGN_INK
            } else {
                INKS[0]
            },
            live: None,
            pull: None,
            crop: None,
            typing: None,
            slider: None,
            armed: false,
            saving: false,
            tint_at: None,
            pdf_page,
            pad: None,
            pad_live: false,
            sig: crate::peek_pdf::kept_signature(),
        });
        let path = up.path.clone();
        let spawned = std::thread::Builder::new()
            .name("quick-look-edit".into())
            .spawn(move || {
                // (A PDF's page drawn at the screen's size; a picture read whole.)
                let whole = match pdf_page {
                    Some(page) => crate::peek_kinds::pdf_facts(&path).and_then(|(_, aspect)| {
                        crate::peek_kinds::pdf_page(&path, page, aspect, max)
                    }),
                    None => crate::peek_edit::whole_picture(&path),
                };
                let Some(whole) = whole else {
                    return;
                };
                let (w, h) = crate::quicklook::tile_fit(whole.dimensions(), max);
                let img = if (w, h) == whole.dimensions() {
                    whole
                } else {
                    image::imageops::resize(&whole, w, h, image::imageops::FilterType::Triangle)
                };
                let _ = tx.send(Msg::Base(tag, img));
            });
        if let Err(e) = spawned {
            warn!("quick look: no edit thread: {e}");
        }
        self.request_desktop_top_draw();
    }

    fn edit_msg(&mut self, msg: Msg) {
        match msg {
            Msg::Snapped(result) => self.snapped(result),
            Msg::Base(tag, img) => {
                let Some(edit) = self
                    .peek
                    .up
                    .as_mut()
                    .and_then(|u| u.edit.as_mut())
                    .filter(|e| e.tag == tag)
                else {
                    return;
                };
                edit.base = Some(img);
                self.edit_redraw();
            }
            Msg::Saved(tag, result) => {
                // (Whoever's it is now, the file written is not what was
                // prepared of it or pictured any more.)
                if let Ok(to) = &result {
                    self.peek_forget(to);
                    self.thumb_stale(&to.to_string_lossy());
                }
                // (The text's own save comes back the same way.)
                if self
                    .peek
                    .up
                    .as_ref()
                    .and_then(|u| u.text_edit.as_ref())
                    .is_some()
                {
                    self.text_saved(tag, result);
                    return;
                }
                if self
                    .peek
                    .up
                    .as_ref()
                    .and_then(|u| u.trim_edit.as_ref())
                    .is_some()
                {
                    self.trim_saved(tag, result);
                    return;
                }
                let ours = self
                    .peek
                    .up
                    .as_ref()
                    .and_then(|u| u.edit.as_ref())
                    .is_some_and(|e| e.tag == tag);
                if !ours {
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
                        self.edit_end();
                        self.peek_forget(&to);
                        // (Its picture in the file windows and on the desktop, made anew.)
                        self.thumb_stale(&to.to_string_lossy());
                        if let Some(up) = self.peek.up.as_mut() {
                            if !same {
                                up.name = to
                                    .file_name()
                                    .map(|n| n.to_string_lossy().into_owned())
                                    .unwrap_or_default();
                                up.path = to.clone();
                            }
                            up.notice = Some((word, Instant::now()));
                        }
                        let page = self.peek.up.as_ref().map_or(0, |u| u.page_shown());
                        self.peek_want_now((to, page));
                    }
                    Err(e) => {
                        warn!("quick look: saving failed: {e}");
                        if let Some(edit) = self.peek.up.as_mut().and_then(|u| u.edit.as_mut()) {
                            edit.saving = false;
                        }
                        if let Some(up) = self.peek.up.as_mut() {
                            up.notice = Some((
                                fill(tr("Could not save: {why}"), &[("why", &e)]),
                                Instant::now(),
                            ));
                        }
                    }
                }
                self.request_desktop_top_draw();
            }
        }
    }

    /// Put the pencil down (nothing saved).
    pub(crate) fn edit_end(&mut self) {
        if let Some(up) = self.peek.up.as_mut() {
            if up.edit.take().is_some() {
                up.start = up.rect;
                up.t = 0.0;
            }
        }
        // (The view's own tiles go back in the layers.)
        self.desktop_top.peek_tiles = None;
        self.request_desktop_top_draw();
    }

    /// Draw the edited picture anew (all of it).
    fn edit_redraw(&mut self) {
        let Some(up) = self.peek.up.as_mut() else {
            return;
        };
        let Some(edit) = up.edit.as_mut() else { return };
        let Some(base) = edit.base.as_ref() else {
            return;
        };
        let shown = crate::peek_edit::render(base, &edit.ops, edit.tint);
        let reshaped = edit
            .shown
            .as_ref()
            .is_none_or(|s| s.dimensions() != shown.dimensions());
        edit.pic = Some(pic_of(&shown));
        edit.shown = Some(shown);
        edit.dirty = None;
        edit.rev = next_rev();
        if reshaped {
            up.start = up.rect;
            up.t = 0.0;
        }
        self.request_desktop_top_draw();
    }

    /// Do `op` to the picture shown and keep it.
    fn edit_do(&mut self, op: Op) {
        if let Some(edit) = self.peek.up.as_mut().and_then(|u| u.edit.as_mut()) {
            edit.ops.push(op);
            edit.armed = false;
        }
        self.edit_redraw();
    }

    /// The pointer as a share of the picture (inside it or not).
    fn edit_share(&self, at: (f32, f32)) -> Option<(f32, f32)> {
        let up = self.peek.up.as_ref()?;
        let pic = up.edit.as_ref()?.pic.as_ref()?;
        let pr = pic_rect(
            Rect::new(up.rect.x, up.rect.y + HEAD, up.rect.w, up.rect.h - HEAD),
            pic.w,
            pic.h,
        );
        Some(((at.0 - pr.x) / pr.w, (at.1 - pr.y) / pr.h))
    }

    /// The pointer while editing: whether it was the editor's.
    pub(crate) fn edit_pointer(
        &mut self,
        event: &smithay_client_toolkit::reexports::client::protocol::wl_pointer::Event,
    ) -> bool {
        use smithay_client_toolkit::reexports::client::protocol::wl_pointer::{ButtonState, Event};
        use smithay_client_toolkit::reexports::client::WEnum;
        let Some(up) = self.peek.up.as_ref() else {
            return false;
        };
        let Some(edit) = up.edit.as_ref() else {
            return false;
        };
        let at = up.ptr.unwrap_or((-1.0, -1.0));
        let parts = parts_of(up.rect, edit);
        let tool = edit.tool;
        // The signature's box is up: the pointer is its.
        if edit.pad.is_some() {
            let pad = pad_parts(up.rect);
            let share = |p: (f32, f32)| {
                (
                    ((p.0 - pad.draw.x) / pad.draw.w).clamp(0.0, 1.0),
                    ((p.1 - pad.draw.y) / pad.draw.h).clamp(0.0, 1.0),
                )
            };
            match event {
                Event::Motion {
                    surface_x,
                    surface_y,
                    ..
                } => {
                    let p = (*surface_x as f32, *surface_y as f32);
                    if let Some(up) = self.peek.up.as_mut() {
                        up.ptr = Some(p);
                        if let Some(edit) = up.edit.as_mut().filter(|e| e.pad_live) {
                            if let Some(line) = edit.pad.as_mut().and_then(|l| l.last_mut()) {
                                line.push(share(p));
                            }
                        }
                    }
                }
                Event::Button {
                    state: WEnum::Value(ButtonState::Pressed),
                    ..
                } => {
                    let Some(edit) = self.peek.up.as_mut().and_then(|u| u.edit.as_mut()) else {
                        return true;
                    };
                    if pad.done.contains(at) {
                        if let Some(lines) =
                            edit.pad.take().filter(|l| l.iter().any(|s| s.len() > 1))
                        {
                            crate::peek_pdf::keep_signature(&lines);
                            edit.sig = Some(lines);
                            edit.tool = Tool::Sign;
                        }
                    } else if pad.clear.contains(at) {
                        edit.pad = Some(Vec::new());
                    } else if pad.cancel.contains(at) {
                        edit.pad = None;
                    } else if pad.draw.contains(at) {
                        if let Some(lines) = edit.pad.as_mut() {
                            lines.push(vec![share(at)]);
                        }
                        edit.pad_live = true;
                    }
                }
                Event::Button {
                    state: WEnum::Value(ButtonState::Released),
                    ..
                } => {
                    if let Some(edit) = self.peek.up.as_mut().and_then(|u| u.edit.as_mut()) {
                        edit.pad_live = false;
                    }
                }
                _ => return false,
            }
            self.request_desktop_top_draw();
            return true;
        }
        match event {
            Event::Motion {
                surface_x,
                surface_y,
                ..
            } => {
                let at = (*surface_x as f32, *surface_y as f32);
                if let Some(up) = self.peek.up.as_mut() {
                    up.ptr = Some(at);
                }
                let share = self.edit_share(at);
                let slider = self
                    .peek
                    .up
                    .as_ref()
                    .and_then(|u| u.edit.as_ref())
                    .and_then(|e| e.slider);
                if let Some(i) = slider {
                    let line = parts.sliders[i];
                    self.edit_slide(i, ((at.0 - line.x) / line.w).clamp(0.0, 1.0));
                } else if let (Some(s), Some(edit)) =
                    (share, self.peek.up.as_mut().and_then(|u| u.edit.as_mut()))
                {
                    let s = (s.0.clamp(0.0, 1.0), s.1.clamp(0.0, 1.0));
                    if let Some(points) = edit.live.as_mut() {
                        let last = *points.last().unwrap_or(&s);
                        points.push(s);
                        // (Only this piece of the line, and only its tiles.)
                        if let Some(shown) = edit.shown.as_mut() {
                            let w = shown.width() as f32;
                            crate::peek_edit::stroke(shown, &[last, s], LINE * w, edit.ink);
                            let (sw, sh) = (shown.width() as f32, shown.height() as f32);
                            let pad = LINE * w;
                            let (x0, x1) = (
                                (last.0.min(s.0) * sw - pad).max(0.0),
                                (last.0.max(s.0) * sw + pad).min(sw - 1.0),
                            );
                            let (y0, y1) = (
                                (last.1.min(s.1) * sh - pad).max(0.0),
                                (last.1.max(s.1) * sh + pad).min(sh - 1.0),
                            );
                            if let Some(pic) = edit.pic.as_mut() {
                                // (All of it waiting to go up stays all of it.)
                                let mut dirty = edit.dirty.take();
                                for r in (y0 as u32 / STEP).saturating_sub(1)
                                    ..=((y1 as u32 / STEP) + 1).min(pic.rows - 1)
                                {
                                    for c in (x0 as u32 / STEP).saturating_sub(1)
                                        ..=((x1 as u32 / STEP) + 1).min(pic.cols - 1)
                                    {
                                        let i = r * pic.cols + c;
                                        pic.tiles[i as usize] = tile(shown, c, r);
                                        if let Some(d) = dirty.as_mut() {
                                            d.insert(i);
                                        }
                                    }
                                }
                                edit.dirty = dirty;
                                edit.rev = next_rev();
                            }
                        }
                    } else if let Some((from, _)) = edit.pull {
                        edit.pull = Some((from, s));
                    }
                }
                self.request_desktop_top_draw();
                true
            }
            Event::Button {
                state: WEnum::Value(ButtonState::Pressed),
                ..
            } => {
                // The head's buttons.
                if parts.save.contains(at) {
                    self.edit_save(SaveAs::Same);
                    return true;
                }
                if parts.copy.contains(at) {
                    self.edit_save(SaveAs::Copy);
                    return true;
                }
                if parts.cancel.contains(at) {
                    self.edit_cancel();
                    return true;
                }
                // The toolbar.
                if let Some(&(b, _)) = parts.tools.iter().find(|(_, r)| r.contains(at)) {
                    self.edit_button(b);
                    return true;
                }
                // The tint's panel.
                if tool == Tool::Tint && parts.tint.contains(at) {
                    if parts.grey.contains(at) {
                        if let Some(edit) = self.peek.up.as_mut().and_then(|u| u.edit.as_mut()) {
                            edit.tint.grey = !edit.tint.grey;
                        }
                        self.edit_redraw();
                    } else if let Some(i) = parts
                        .sliders
                        .iter()
                        .position(|l| Rect::new(l.x - 8.0, l.y, l.w + 16.0, l.h).contains(at))
                    {
                        if let Some(edit) = self.peek.up.as_mut().and_then(|u| u.edit.as_mut()) {
                            edit.slider = Some(i);
                        }
                        let line = parts.sliders[i];
                        self.edit_slide(i, ((at.0 - line.x) / line.w).clamp(0.0, 1.0));
                    }
                    return true;
                }
                // On the picture.
                let Some(s) = self
                    .edit_share(at)
                    .filter(|s| (0.0..=1.0).contains(&s.0) && (0.0..=1.0).contains(&s.1))
                else {
                    return true;
                };
                let Some(edit) = self.peek.up.as_mut().and_then(|u| u.edit.as_mut()) else {
                    return true;
                };
                match tool {
                    Tool::Draw => edit.live = Some(vec![s]),
                    Tool::Sign => {
                        match edit.sig.clone() {
                            // (A signature a third of the page wide, centred where clicked.)
                            Some(strokes) => {
                                let (pw, ph) = edit
                                    .pic
                                    .as_ref()
                                    .map_or((3.0, 4.0), |p| (p.w as f32, p.h as f32));
                                let w = 0.3;
                                let h = w * pw / ph / 3.0;
                                let rect = [
                                    (s.0 - w / 2.0).clamp(0.0, 1.0 - w),
                                    (s.1 - h / 2.0).clamp(0.0, 1.0 - h),
                                    w,
                                    h,
                                ];
                                self.edit_do(Op::Sign { rect, strokes });
                            }
                            None => edit.pad = Some(Vec::new()),
                        }
                    }
                    Tool::Crop | Tool::Blur | Tool::Mark => {
                        edit.pull = Some((s, s));
                        edit.crop = None;
                    }
                    Tool::Text => {
                        // (Words being typed elsewhere are kept first.)
                        let done = edit.typing.take().filter(|(_, t)| !t.trim().is_empty());
                        edit.typing = Some((s, String::new()));
                        if let Some((at, text)) = done {
                            let ink = edit.ink;
                            self.edit_do(Op::Text {
                                at,
                                text,
                                size: WORDS,
                                ink,
                            });
                        }
                    }
                    Tool::Tint => {}
                }
                self.request_desktop_top_draw();
                true
            }
            Event::Button {
                state: WEnum::Value(ButtonState::Released),
                ..
            } => {
                let Some(edit) = self.peek.up.as_mut().and_then(|u| u.edit.as_mut()) else {
                    return true;
                };
                edit.slider = None;
                if let Some(points) = edit.live.take() {
                    let ink = edit.ink;
                    edit.ops.push(Op::Stroke {
                        points,
                        width: LINE,
                        ink,
                    });
                    edit.armed = false;
                } else if let Some((a, b)) = edit.pull.take() {
                    let rect = [
                        a.0.min(b.0),
                        a.1.min(b.1),
                        (a.0 - b.0).abs(),
                        (a.1 - b.1).abs(),
                    ];
                    if rect[2] > 0.02 && rect[3] > 0.02 {
                        match tool {
                            Tool::Crop => edit.crop = Some(rect),
                            Tool::Blur => self.edit_do(Op::Blur(rect)),
                            Tool::Mark => self.edit_do(Op::Mark(rect)),
                            _ => {}
                        }
                    }
                }
                self.request_desktop_top_draw();
                true
            }
            _ => false,
        }
    }

    fn edit_slide(&mut self, i: usize, k: f32) {
        let Some(edit) = self.peek.up.as_mut().and_then(|u| u.edit.as_mut()) else {
            return;
        };
        let v = LIGHT.0 + (LIGHT.1 - LIGHT.0) * k;
        match i {
            0 => edit.tint.light = v,
            1 => edit.tint.contrast = v,
            _ => edit.tint.warmth = k * 2.0 - 1.0,
        }
        edit.armed = false;
        // (At most every 40 ms: the whole picture is drawn again.)
        if edit.tint_at.is_some_and(|t| t.elapsed().as_millis() < 40) {
            self.request_desktop_top_draw();
            return;
        }
        edit.tint_at = Some(Instant::now());
        self.edit_redraw();
    }

    fn edit_button(&mut self, b: Btn) {
        let Some(edit) = self.peek.up.as_mut().and_then(|u| u.edit.as_mut()) else {
            return;
        };
        match b {
            Btn::Tool(t) => {
                edit.tool = t;
                edit.crop = None;
                edit.pull = None;
                // (No signature yet: its box, to draw one.)
                if t == Tool::Sign && edit.sig.is_none() {
                    edit.pad = Some(Vec::new());
                }
                self.edit_keep_words();
            }
            Btn::NewSign => edit.pad = Some(Vec::new()),
            Btn::Ink(i) => edit.ink = INKS[i],
            Btn::Turn => self.edit_do(Op::Turn),
            Btn::Apply => {
                if let Some(rect) = edit.crop.take() {
                    self.edit_do(Op::Crop(rect));
                }
            }
            Btn::Undo => {
                if edit.typing.take().is_none() && edit.ops.pop().is_none() {
                    edit.tint = Tint::default();
                }
                self.edit_redraw();
            }
        }
        self.request_desktop_top_draw();
    }

    /// Words being typed become part of the picture.
    fn edit_keep_words(&mut self) {
        let Some(edit) = self.peek.up.as_mut().and_then(|u| u.edit.as_mut()) else {
            return;
        };
        if let Some((at, text)) = edit.typing.take().filter(|(_, t)| !t.trim().is_empty()) {
            let ink = edit.ink;
            self.edit_do(Op::Text {
                at,
                text,
                size: WORDS,
                ink,
            });
        }
    }

    /// Cancel: once more with changes made.
    fn edit_cancel(&mut self) {
        let Some(edit) = self.peek.up.as_mut().and_then(|u| u.edit.as_mut()) else {
            return;
        };
        if edit.changed() && !edit.armed {
            edit.armed = true;
            self.request_desktop_top_draw();
            return;
        }
        self.edit_end();
    }

    /// Save over it (its original kept) or beside it, off the loop.
    fn edit_save(&mut self, how: SaveAs) {
        self.edit_keep_words();
        let Some(tx) = self.peek_edit_channel() else {
            return;
        };
        let Some(up) = self.peek.up.as_mut() else {
            return;
        };
        let path = up.path.clone();
        let name = path
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default();
        let Some(edit) = up.edit.as_mut() else { return };
        if edit.saving || !edit.changed() {
            if !edit.changed() {
                self.edit_end();
            }
            return;
        }
        // (A RAW is never written over: it becomes a JPEG beside it.)
        let pdf_page = edit.pdf_page;
        let how = if crate::peek_edit::writable(&name) || pdf_page.is_some() {
            how
        } else {
            SaveAs::Copy
        };
        edit.saving = true;
        let (ops, tint, tag) = (edit.ops.clone(), edit.tint, edit.tag);
        let word = tr("edited").to_owned();
        let spawned = std::thread::Builder::new()
            .name("quick-look-save".into())
            .spawn(move || {
                // (A PDF's marks become its own annotations.)
                let result = match pdf_page {
                    Some(page) => crate::peek_pdf::save(&path, page, &ops, how, &word),
                    None => crate::peek_edit::save(&path, &ops, tint, how, &word),
                }
                .map_err(|e| format!("{e:#}"));
                let _ = tx.send(Msg::Saved(tag, result));
            });
        if let Err(e) = spawned {
            warn!("quick look: no save thread: {e}");
        }
        self.request_desktop_top_draw();
    }

    /// A key while editing (the file window hands it on): always the editor's.
    pub(crate) fn edit_key(&mut self, key: Keysym, utf8: Option<&str>) {
        let (ctrl, shift) = (self.modifiers.ctrl, self.modifiers.shift);
        let Some(edit) = self.peek.up.as_mut().and_then(|u| u.edit.as_mut()) else {
            return;
        };
        // Words being typed take the keys.
        if let Some((_, text)) = edit.typing.as_mut() {
            match key {
                Keysym::Escape => edit.typing = None,
                Keysym::Return | Keysym::KP_Enter => self.edit_keep_words(),
                Keysym::BackSpace => {
                    text.pop();
                }
                _ if !ctrl => {
                    if let Some(t) = utf8.filter(|t| !t.chars().any(char::is_control)) {
                        text.push_str(t);
                    }
                }
                _ => {}
            }
            self.request_desktop_top_draw();
            return;
        }
        match key {
            Keysym::s | Keysym::S if ctrl => {
                self.edit_save(if shift { SaveAs::Copy } else { SaveAs::Same })
            }
            Keysym::z | Keysym::Z if ctrl => self.edit_button(Btn::Undo),
            Keysym::Escape => self.edit_cancel(),
            Keysym::Return | Keysym::KP_Enter if edit.crop.is_some() => {
                self.edit_button(Btn::Apply)
            }
            _ => {}
        }
    }

    /// The editor into the frame: the picture, the head's buttons, the
    /// toolbar (in `over`, drawn over the picture), what is in hand.
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn edit_push(
        &self,
        g: &mut GridContent,
        over: &mut GridContent,
        r: Rect,
        open: f32,
        ink: [f32; 4],
        ground: [f32; 4],
        tiles_up: bool,
    ) {
        let Some(up) = self.peek.up.as_ref() else {
            return;
        };
        let Some(edit) = up.edit.as_ref() else { return };
        let body = Rect::new(r.x, r.y + HEAD, r.w, (r.h - HEAD).max(0.0));
        let clip = [r.x, r.y, r.x + r.w, r.y + r.h];
        let clipped = |rect: Rect, layer: u32| IconInst {
            rect,
            layer,
            tint: clip,
            ring: -1.6,
            plate: [WINDOW_CORNER, open, 0.0, 0.0],
        };
        let mut dim = ink;
        dim[3] *= 0.6;
        // The picture (its tiles once they are up).
        let pr = edit.pic.as_ref().map(|p| pic_rect(body, p.w, p.h));
        if let (Some(pic), Some(pr), true) = (edit.pic.as_ref(), pr, tiles_up) {
            let (kx, ky) = (pr.w / pic.w as f32, pr.h / pic.h as f32);
            for row in 0..pic.rows {
                for col in 0..pic.cols {
                    let x = pr.x + (col * STEP) as f32 * kx - kx;
                    let y = pr.y + (row * STEP) as f32 * ky - ky;
                    g.icons.push(clipped(
                        Rect::new(x, y, TILE as f32 * kx, TILE as f32 * ky),
                        row * pic.cols + col,
                    ));
                }
            }
        } else {
            g.labels.push(Label {
                centered: true,
                ..text_label(
                    tr("Getting it ready…"),
                    (body.x + body.w / 2.0, body.y + body.h / 2.0 - 8.0),
                    body.w,
                    13.0,
                    None,
                    dim,
                )
            });
        }
        let parts = parts_of(r, edit);
        // A marquee being pulled or waiting (crop, blur).
        let marquee = edit
            .pull
            .map(|(a, b)| {
                [
                    a.0.min(b.0),
                    a.1.min(b.1),
                    (a.0 - b.0).abs(),
                    (a.1 - b.1).abs(),
                ]
            })
            .or(edit.crop);
        if let (Some(m), Some(pr)) = (marquee, pr) {
            let mr = Rect::new(
                pr.x + m[0] * pr.w,
                pr.y + m[1] * pr.h,
                m[2] * pr.w,
                m[3] * pr.h,
            );
            if edit.tool == Tool::Crop {
                // (What is cut away, darkened.)
                for shade in [
                    Rect::new(pr.x, pr.y, pr.w, mr.y - pr.y),
                    Rect::new(pr.x, mr.y + mr.h, pr.w, pr.y + pr.h - mr.y - mr.h),
                    Rect::new(pr.x, mr.y, mr.x - pr.x, mr.h),
                    Rect::new(mr.x + mr.w, mr.y, pr.x + pr.w - mr.x - mr.w, mr.h),
                ] {
                    over.rects.push(RectInst {
                        rect: shade,
                        radius: 0.0,
                        color: [0.0, 0.0, 0.0, 0.5 * open],
                        glass: 0.0,
                        border: 0.0,
                    });
                }
            }
            over.rects.push(RectInst {
                rect: mr,
                radius: 2.0,
                color: [1.0, 1.0, 1.0, 0.9 * open],
                glass: 0.0,
                border: 1.5,
            });
        }
        // Words being typed, with a caret.
        if let (Some((at, text)), Some(pr)) = (edit.typing.as_ref(), pr) {
            let px = (WORDS * pr.h).max(10.0);
            let mut c = [0.0f32; 4];
            for (i, v) in edit.ink.iter().enumerate() {
                c[i] = (f32::from(*v) / 255.0).powf(2.2);
            }
            c[3] = open;
            let word = format!("{text}|");
            over.labels.push(text_label(
                &word,
                (pr.x + at.0 * pr.w, pr.y + at.1 * pr.h),
                pr.x + pr.w - (pr.x + at.0 * pr.w),
                px,
                Some(FONT_BOLD),
                c,
            ));
        }
        // The head: its name, what it is doing, Cancel · Save copy · Save.
        let text_w = (parts.cancel.x - (r.x + 46.0) - 12.0).max(20.0);
        g.labels.push(text_label(
            &up.name,
            (r.x + 46.0, r.y + 7.0),
            text_w,
            13.5,
            Some(FONT_BOLD),
            ink,
        ));
        let doing = if edit.saving {
            tr("Saving…")
        } else {
            tr("Editing")
        };
        g.labels.push(text_label(
            doing,
            (r.x + 46.0, r.y + 25.0),
            text_w,
            11.5,
            None,
            dim,
        ));
        let name = up
            .path
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default();
        let can_save = crate::peek_edit::writable(&name) || edit.pdf_page.is_some();
        let hot = |rect: Rect| up.ptr.is_some_and(|p| rect.contains(p));
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
        let soft = |rect: Rect| {
            [
                ink[0],
                ink[1],
                ink[2],
                (if hot(rect) { 0.26 } else { 0.14 }) * open,
            ]
        };
        let (cancel_word, cancel_fill, cancel_ink) = if edit.armed {
            (
                tr("Discard changes?"),
                [0.88, 0.32, 0.32, open],
                [1.0, 1.0, 1.0, open],
            )
        } else {
            (tr("Cancel"), soft(parts.cancel), ink)
        };
        pill(g, parts.cancel, cancel_fill, cancel_word, cancel_ink);
        // (The one that does what is most done is lit; a RAW saves a copy only.)
        let accent = [1.0, 0.75, 0.6, open];
        if can_save {
            pill(g, parts.copy, soft(parts.copy), tr("Save copy"), ink);
            pill(
                g,
                parts.save,
                accent,
                tr("Save"),
                [ground[0], ground[1], ground[2], open],
            );
        } else {
            pill(
                g,
                Rect {
                    x: parts.save.x + parts.save.w - parts.copy.w,
                    ..parts.copy
                },
                accent,
                tr("Save copy"),
                [ground[0], ground[1], ground[2], open],
            );
        }
        // The toolbar.
        over.rects.push(RectInst {
            rect: parts.bar,
            radius: parts.bar.h / 2.0,
            color: [0.06, 0.07, 0.09, 0.84 * open],
            glass: 0.0,
            border: 0.0,
        });
        let white = [1.0, 1.0, 1.0, open];
        for &(b, rect) in &parts.tools {
            match b {
                Btn::Ink(i) => {
                    let mut c = [0.0f32; 4];
                    for (k, v) in INKS[i].iter().enumerate() {
                        c[k] = (f32::from(*v) / 255.0).powf(2.2);
                    }
                    c[3] = open;
                    if INKS[i] == edit.ink {
                        over.rects.push(RectInst {
                            rect: Rect::new(rect.x - 2.0, rect.y - 2.0, rect.w + 4.0, rect.h + 4.0),
                            radius: 13.0,
                            color: white,
                            glass: 0.0,
                            border: 2.0,
                        });
                    }
                    over.rects.push(RectInst {
                        rect: Rect::new(rect.x + 3.0, rect.y + 3.0, rect.w - 6.0, rect.h - 6.0),
                        radius: 8.0,
                        color: c,
                        glass: 0.0,
                        border: 0.0,
                    });
                }
                _ => {
                    let on = matches!(b, Btn::Tool(t) if t == edit.tool);
                    let lit = if b == Btn::Apply {
                        [1.0, 0.75, 0.6, open]
                    } else {
                        [
                            1.0,
                            1.0,
                            1.0,
                            (if on {
                                0.24
                            } else if hot(rect) {
                                0.1
                            } else {
                                0.0
                            }) * open,
                        ]
                    };
                    if lit[3] > 0.0 {
                        over.rects.push(RectInst {
                            rect,
                            radius: rect.h / 2.0,
                            color: lit,
                            glass: 0.0,
                            border: 0.0,
                        });
                    }
                    let word_ink = if b == Btn::Apply {
                        [0.07, 0.08, 0.1, open]
                    } else {
                        white
                    };
                    over.labels.push(Label {
                        centered: true,
                        ..text_label(
                            btn_word(b),
                            (rect.x + rect.w / 2.0, rect.y + 5.0),
                            rect.w,
                            12.0,
                            Some(FONT_BOLD),
                            word_ink,
                        )
                    });
                }
            }
        }
        // A signature: where to put it said; or its box to draw it in.
        if edit.tool == Tool::Sign && edit.sig.is_some() && edit.pad.is_none() {
            let word = tr("Click where the signature goes");
            let w = word_w(word, 12.0) + 28.0;
            let hint = Rect::new(r.x + (r.w - w) / 2.0, parts.bar.y - 36.0, w, 26.0);
            over.rects.push(RectInst {
                rect: hint,
                radius: 13.0,
                color: [0.06, 0.07, 0.09, 0.84 * open],
                glass: 0.0,
                border: 0.0,
            });
            over.labels.push(Label {
                centered: true,
                ..text_label(
                    word,
                    (hint.x + hint.w / 2.0, hint.y + 5.0),
                    w,
                    12.0,
                    None,
                    white,
                )
            });
        }
        if let Some(lines) = edit.pad.as_ref() {
            let pad = pad_parts(r);
            over.rects.push(RectInst {
                rect: Rect::new(r.x, r.y + HEAD, r.w, r.h - HEAD),
                radius: 0.0,
                color: [0.0, 0.0, 0.0, 0.45 * open],
                glass: 0.0,
                border: 0.0,
            });
            over.rects.push(RectInst {
                rect: pad.panel,
                radius: 14.0,
                color: [ground[0], ground[1], ground[2], open],
                glass: 0.0,
                border: 0.0,
            });
            over.labels.push(text_label(
                tr("Your signature"),
                (pad.draw.x, pad.panel.y + 12.0),
                pad.draw.w,
                13.0,
                Some(FONT_BOLD),
                ink,
            ));
            over.rects.push(RectInst {
                rect: pad.draw,
                radius: 8.0,
                color: [0.98, 0.98, 0.97, open],
                glass: 0.0,
                border: 0.0,
            });
            // Its lines, as dots close together.
            let mut pen = [0.0f32; 4];
            for (k, v) in crate::peek_edit::SIGN_INK.iter().enumerate() {
                pen[k] = (f32::from(*v) / 255.0).powf(2.2);
            }
            pen[3] = open;
            let dot = 3.0;
            for line in lines {
                let pts: Vec<(f32, f32)> = line
                    .iter()
                    .map(|&(x, y)| (pad.draw.x + x * pad.draw.w, pad.draw.y + y * pad.draw.h))
                    .collect();
                for pair in pts.windows(2) {
                    let ((x0, y0), (x1, y1)) = (pair[0], pair[1]);
                    let n = ((x1 - x0).hypot(y1 - y0) / 1.5).ceil().max(1.0) as usize;
                    for i in 0..=n {
                        let k = i as f32 / n as f32;
                        let (x, y) = (x0 + (x1 - x0) * k, y0 + (y1 - y0) * k);
                        over.rects.push(RectInst {
                            rect: Rect::new(x - dot / 2.0, y - dot / 2.0, dot, dot),
                            radius: dot / 2.0,
                            color: pen,
                            glass: 0.0,
                            border: 0.0,
                        });
                    }
                }
            }
            for (rect, word, lit) in [
                (pad.cancel, tr("Cancel"), false),
                (pad.clear, tr("Clear"), false),
                (pad.done, tr("Use"), true),
            ] {
                let fill = if lit {
                    [1.0, 0.75, 0.6, open]
                } else {
                    [ink[0], ink[1], ink[2], 0.14 * open]
                };
                let word_ink = if lit {
                    [ground[0], ground[1], ground[2], open]
                } else {
                    ink
                };
                over.rects.push(RectInst {
                    rect,
                    radius: 13.0,
                    color: fill,
                    glass: 0.0,
                    border: 0.0,
                });
                over.labels.push(Label {
                    centered: true,
                    ..text_label(
                        word,
                        (rect.x + rect.w / 2.0, rect.y + 5.0),
                        rect.w,
                        12.0,
                        Some(FONT_BOLD),
                        word_ink,
                    )
                });
            }
        }
        // The tint's panel.
        if edit.tool == Tool::Tint {
            over.rects.push(RectInst {
                rect: parts.tint,
                radius: 14.0,
                color: [0.06, 0.07, 0.09, 0.84 * open],
                glass: 0.0,
                border: 0.0,
            });
            let values = [
                (edit.tint.light - LIGHT.0) / (LIGHT.1 - LIGHT.0),
                (edit.tint.contrast - LIGHT.0) / (LIGHT.1 - LIGHT.0),
                (edit.tint.warmth + 1.0) / 2.0,
            ];
            for (i, word) in [tr("Brightness"), tr("Contrast"), tr("Warmth")]
                .into_iter()
                .enumerate()
            {
                let line = parts.sliders[i];
                over.labels.push(text_label(
                    word,
                    (parts.tint.x + 16.0, line.y + 2.0),
                    90.0,
                    12.0,
                    None,
                    white,
                ));
                over.rects.push(RectInst {
                    rect: Rect::new(line.x, line.y + line.h / 2.0 - 2.0, line.w, 4.0),
                    radius: 2.0,
                    color: [1.0, 1.0, 1.0, 0.3 * open],
                    glass: 0.0,
                    border: 0.0,
                });
                let x = line.x + values[i].clamp(0.0, 1.0) * line.w;
                over.rects.push(RectInst {
                    rect: Rect::new(x - 7.0, line.y + line.h / 2.0 - 7.0, 14.0, 14.0),
                    radius: 7.0,
                    color: white,
                    glass: 0.0,
                    border: 0.0,
                });
            }
            let lit = [
                1.0,
                1.0,
                1.0,
                (if edit.tint.grey { 0.3 } else { 0.12 }) * open,
            ];
            over.rects.push(RectInst {
                rect: parts.grey,
                radius: 12.0,
                color: lit,
                glass: 0.0,
                border: 0.0,
            });
            over.labels.push(Label {
                centered: true,
                ..text_label(
                    tr("Black and white"),
                    (parts.grey.x + parts.grey.w / 2.0, parts.grey.y + 5.0),
                    parts.grey.w,
                    12.0,
                    None,
                    white,
                )
            });
        }
    }

    /// `debug-desktop "peek …"` (the rig: no pointer, no keys): `edit`,
    /// `tool crop|draw|text|blur|tint`, `turn`, `undo`, `apply`, `drag x0
    /// y0 x1 y1` (shares of the picture, through the real pointer code),
    /// `type <words>` (then Enter), `slide <0|1|2> <0..1>`, `save`, `copy`,
    /// `cancel`, `state`.
    pub(crate) fn edit_debug(&mut self, what: &str) -> String {
        use smithay_client_toolkit::reexports::client::protocol::wl_pointer::{ButtonState, Event};
        use smithay_client_toolkit::reexports::client::WEnum;
        let mut words = what.split_whitespace();
        let verb = words.next().unwrap_or("");
        let rest: Vec<&str> = words.collect();
        // A zoomed photo: `pan <dx> <dy>` (screen pixels), `dbl <x> <y>`
        // (shares of the body: a double click there).
        if verb == "pan" {
            let d: Vec<f32> = rest.iter().filter_map(|w| w.parse().ok()).collect();
            if let [dx, dy] = d.as_slice() {
                self.peek_pan((*dx, *dy));
            }
            let z = self.peek.up.as_ref().map(|u| (u.zoom.k, u.zoom.center));
            return format!("zoom {z:?}");
        }
        if verb == "dbl" {
            let d: Vec<f32> = rest.iter().filter_map(|w| w.parse().ok()).collect();
            if let (Some((body, _, _)), [x, y]) = (self.peek_zoom_frame(), d.as_slice()) {
                let at = (body.x + x * body.w, body.y + y * body.h);
                self.peek_zoom_press(at);
                self.peek_zoom_release();
                self.peek_zoom_press(at);
                self.peek_zoom_release();
            }
            let z = self.peek.up.as_ref().map(|u| (u.zoom.k, u.zoom.center));
            return format!("zoom {z:?}");
        }
        // A video's moment as a photo.
        if verb == "snap" {
            self.peek_snap();
            return "snap".to_owned();
        }
        // A cut: `cut <from s> <to s>`, `mute`, `save`, `copy`, `cancel`.
        if self.peek.up.as_ref().is_some_and(|u| u.trim_edit.is_some()) {
            match verb {
                "cut" => {
                    let a = rest.first().and_then(|v| v.parse().ok()).unwrap_or(0.0);
                    let b = rest.get(1).and_then(|v| v.parse().ok()).unwrap_or(f32::MAX);
                    self.trim_set(a, b, None);
                }
                "mute" => self.trim_set(f32::NAN, f32::NAN, Some(())),
                "save" => self.trim_save(SaveAs::Same),
                "copy" => self.trim_save(SaveAs::Copy),
                "cancel" => self.trim_key(Keysym::Escape),
                _ => {}
            }
            return "cutting".to_owned();
        }
        // Text being written: `type <words>`, `key enter|back|up|down|
        // left|right|home|end|escape`, `save`, `copy`, `cancel`.
        if self.peek.up.as_ref().is_some_and(|u| u.text_edit.is_some()) {
            match verb {
                "type" => {
                    for c in rest.join(" ").chars() {
                        self.text_key(Keysym::NoSymbol, Some(&c.to_string()));
                    }
                }
                "key" => {
                    let k = match rest.first().copied() {
                        Some("enter") => Keysym::Return,
                        Some("back") => Keysym::BackSpace,
                        Some("up") => Keysym::Up,
                        Some("down") => Keysym::Down,
                        Some("left") => Keysym::Left,
                        Some("right") => Keysym::Right,
                        Some("home") => Keysym::Home,
                        Some("end") => Keysym::End,
                        _ => Keysym::Escape,
                    };
                    self.text_key(k, None);
                }
                "save" => self.text_key_save(false),
                "copy" => self.text_key_save(true),
                "cancel" => self.text_key(Keysym::Escape, None),
                _ => {}
            }
            return "writing text".to_owned();
        }
        match verb {
            "edit" => self.edit_begin(),
            "tool" => {
                let t = match rest.first().copied() {
                    Some("crop") => Tool::Crop,
                    Some("text") => Tool::Text,
                    Some("blur") => Tool::Blur,
                    Some("tint") => Tool::Tint,
                    Some("mark") => Tool::Mark,
                    Some("sign") => Tool::Sign,
                    _ => Tool::Draw,
                };
                self.edit_button(Btn::Tool(t));
            }
            "turn" => self.edit_button(Btn::Turn),
            "undo" => self.edit_button(Btn::Undo),
            "apply" => self.edit_button(Btn::Apply),
            "save" => self.edit_save(SaveAs::Same),
            "copy" => self.edit_save(SaveAs::Copy),
            "cancel" => self.edit_cancel(),
            "slide" => {
                let (i, k) = (
                    rest.first().and_then(|v| v.parse().ok()).unwrap_or(0),
                    rest.get(1).and_then(|v| v.parse().ok()).unwrap_or(0.5),
                );
                self.edit_slide(i, k);
                if let Some(edit) = self.peek.up.as_mut().and_then(|u| u.edit.as_mut()) {
                    edit.tint_at = None;
                }
                self.edit_redraw();
            }
            "type" => {
                for c in rest.join(" ").chars() {
                    self.edit_key(Keysym::NoSymbol, Some(&c.to_string()));
                }
                self.edit_key(Keysym::Return, None);
            }
            "drag" | "click" => {
                let v: Vec<f32> = rest.iter().filter_map(|w| w.parse().ok()).collect();
                let (a, b) = match v.as_slice() {
                    [x0, y0, x1, y1] => ((*x0, *y0), (*x1, *y1)),
                    [x, y] => ((*x, *y), (*x, *y)),
                    _ => return "drag x0 y0 x1 y1 | click x y".to_owned(),
                };
                let Some(up) = self.peek.up.as_ref() else {
                    return "nothing up".to_owned();
                };
                let Some(pic) = up.edit.as_ref().and_then(|e| e.pic.as_ref()) else {
                    return "not editing yet".to_owned();
                };
                let pr = pic_rect(
                    Rect::new(up.rect.x, up.rect.y + HEAD, up.rect.w, up.rect.h - HEAD),
                    pic.w,
                    pic.h,
                );
                let at =
                    |s: (f32, f32)| (f64::from(pr.x + s.0 * pr.w), f64::from(pr.y + s.1 * pr.h));
                let motion = |p: (f64, f64)| Event::Motion {
                    time: 0,
                    surface_x: p.0,
                    surface_y: p.1,
                };
                let button = |state| Event::Button {
                    serial: 0,
                    time: 0,
                    button: 0x110,
                    state: WEnum::Value(state),
                };
                self.edit_pointer(&motion(at(a)));
                self.edit_pointer(&button(ButtonState::Pressed));
                for n in 1..=12 {
                    let k = n as f32 / 12.0;
                    self.edit_pointer(&motion(at((a.0 + (b.0 - a.0) * k, a.1 + (b.1 - a.1) * k))));
                }
                self.edit_pointer(&button(ButtonState::Released));
            }
            // The signature's box: a line drawn in it (shares of it), Use.
            "padline" | "paduse" => {
                let Some(up) = self.peek.up.as_ref() else {
                    return "nothing up".to_owned();
                };
                let pad = pad_parts(up.rect);
                let v: Vec<f32> = rest.iter().filter_map(|w| w.parse().ok()).collect();
                let at = |s: (f32, f32)| {
                    (
                        f64::from(pad.draw.x + s.0 * pad.draw.w),
                        f64::from(pad.draw.y + s.1 * pad.draw.h),
                    )
                };
                let motion = |p: (f64, f64)| Event::Motion {
                    time: 0,
                    surface_x: p.0,
                    surface_y: p.1,
                };
                let button = |state| Event::Button {
                    serial: 0,
                    time: 0,
                    button: 0x110,
                    state: WEnum::Value(state),
                };
                if verb == "paduse" {
                    let c = pad.done;
                    self.edit_pointer(&motion((
                        f64::from(c.x + c.w / 2.0),
                        f64::from(c.y + c.h / 2.0),
                    )));
                    self.edit_pointer(&button(ButtonState::Pressed));
                    self.edit_pointer(&button(ButtonState::Released));
                } else if let [x0, y0, x1, y1] = v.as_slice() {
                    self.edit_pointer(&motion(at((*x0, *y0))));
                    self.edit_pointer(&button(ButtonState::Pressed));
                    for n in 1..=16 {
                        let k = n as f32 / 16.0;
                        let wave = (k * 12.0).sin() * 0.25;
                        self.edit_pointer(&motion(at((
                            x0 + (x1 - x0) * k,
                            y0 + (y1 - y0) * k + wave,
                        ))));
                    }
                    self.edit_pointer(&button(ButtonState::Released));
                }
            }
            "state" => {}
            _ => return format!("no peek verb {verb}"),
        }
        let Some(edit) = self.peek.up.as_ref().and_then(|u| u.edit.as_ref()) else {
            return "not editing".to_owned();
        };
        format!(
            "editing: {} ops, tool {:?}, picture {:?}, crop {:?}, tint {:?}, saving {}",
            edit.ops.len(),
            edit.tool,
            edit.pic.as_ref().map(|p| (p.w, p.h)),
            edit.crop,
            edit.tint,
            edit.saving
        )
    }
}
