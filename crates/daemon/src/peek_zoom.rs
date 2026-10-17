//! ZOOM in Quick Look (as Finder's): a photo is brought closer about the
//! pointer — Ctrl with the wheel, + and −, a double click (fitted ↔ its own
//! pixels) — and moved about by dragging it or with the wheel; 0 fits it
//! again. It stays sharp: once it has stood still a moment, the part on the
//! screen is read again from the file at the screen's size, off the loop
//! (its own tiles, `DETAIL` layers on, laid over the fitted picture where
//! they belong).

use std::time::Instant;

use smithay_client_toolkit::reexports::calloop;
use tracing::warn;

use crate::content::Rect;
use crate::quicklook::{Pic, HEAD};
use crate::App;

/// The farthest in, as times the fitted size.
const MOST: f32 = 12.0;
/// How long the picture stands still before its part is read sharp.
const SETTLE_MS: u128 = 160;
/// Two clicks within this are a double click.
const DOUBLE_MS: u128 = 350;

#[derive(Default)]
pub(crate) struct Zoom {
    /// 1 = fitted.
    pub k: f32,
    /// The point of the picture at the body's middle (shares of it).
    pub center: (f32, f32),
    /// A drag: where the pointer and the middle were as it began.
    drag: Option<((f32, f32), (f32, f32))>,
    /// The sharp part read for where it stands: its tiles and where they go
    /// (x, y, w, h shares of the picture); which drawing of it is in the
    /// layers.
    pub detail: Option<(Pic, [f32; 4])>,
    pub detail_rev: u64,
    due: Option<Instant>,
    tag: u64,
    last_click: Option<Instant>,
}

impl Zoom {
    pub(crate) fn new() -> Self {
        Self {
            k: 1.0,
            center: (0.5, 0.5),
            ..Default::default()
        }
    }

    pub(crate) fn zoomed(&self) -> bool {
        self.k > 1.001
    }
}

/// Where the whole picture stands for zoom `z`: `fit` (the fitted rect)
/// grown `z.k` times about `z.center`, kept covering `body` where it can.
pub(crate) fn view_rect(body: Rect, fit: Rect, z: &Zoom) -> Rect {
    let k = z.k.max(1.0);
    let (w, h) = (fit.w * k, fit.h * k);
    let (cx, cy) = (body.x + body.w / 2.0, body.y + body.h / 2.0);
    let place = |mid: f32, share: f32, size: f32, lo: f32, room: f32| {
        if size <= room {
            lo + (room - size) / 2.0
        } else {
            (mid - share * size).clamp(lo + room - size, lo)
        }
    };
    Rect::new(
        place(cx, z.center.0, w, body.x, body.w),
        place(cy, z.center.1, h, body.y, body.h),
        w,
        h,
    )
}

/// The middle of the body as a share of a picture standing at `rect`.
fn center_of(body: Rect, rect: Rect) -> (f32, f32) {
    (
        ((body.x + body.w / 2.0) - rect.x) / rect.w,
        ((body.y + body.h / 2.0) - rect.y) / rect.h,
    )
}

impl App {
    /// The frame's body, the fitted picture and its own size, when what is
    /// up is a photo that can be zoomed.
    pub(crate) fn peek_zoom_frame(&self) -> Option<(Rect, Rect, (u32, u32))> {
        let up = self.peek.up.as_ref()?;
        if up.edit.is_some() || up.closing {
            return None;
        }
        let (_, p) = self.peek_shown()?;
        let crate::quicklook::Body::Picture(pic) = &p.body else {
            return None;
        };
        // (A PDF's page has no pixels of its own: it is drawn sharp at any size
        // — as close as eight times.)
        let own = if p.pages > 0 {
            (pic.w * 8, pic.h * 8)
        } else {
            p.own
        };
        // (A long page scrolls; it does not zoom.)
        if p.video.is_some() || p.anim.is_some() || p.long.is_some() || own == (0, 0) {
            return None;
        }
        let body = Rect::new(up.rect.x, up.rect.y + HEAD, up.rect.w, up.rect.h - HEAD);
        Some((body, crate::peek_editor::pic_rect(body, pic.w, pic.h), own))
    }

    /// Bring it closer (`factor` > 1) or farther, the point under `at`
    /// staying under it.
    pub(crate) fn peek_zoom_at(&mut self, factor: f32, at: (f32, f32)) {
        let Some((body, fit, _)) = self.peek_zoom_frame() else {
            return;
        };
        let Some(up) = self.peek.up.as_mut() else {
            return;
        };
        let z = &mut up.zoom;
        let before = view_rect(body, fit, z);
        let share = ((at.0 - before.x) / before.w, (at.1 - before.y) / before.h);
        z.k = (z.k * factor).clamp(1.0, MOST);
        let (w, h) = (fit.w * z.k, fit.h * z.k);
        let wanted = Rect::new(at.0 - share.0 * w, at.1 - share.1 * h, w, h);
        z.center = center_of(body, wanted);
        let placed = view_rect(body, fit, z);
        z.center = center_of(body, placed);
        if !z.zoomed() {
            z.detail = None;
            z.center = (0.5, 0.5);
        }
        z.due = z.zoomed().then(Instant::now);
        self.request_desktop_top_draw();
    }

    /// Move it by (dx, dy) screen pixels.
    pub(crate) fn peek_pan(&mut self, d: (f32, f32)) {
        let Some((body, fit, _)) = self.peek_zoom_frame() else {
            return;
        };
        let Some(up) = self.peek.up.as_mut() else {
            return;
        };
        let z = &mut up.zoom;
        if !z.zoomed() {
            return;
        }
        let r = view_rect(body, fit, z);
        let moved = Rect::new(r.x + d.0, r.y + d.1, r.w, r.h);
        z.center = center_of(body, moved);
        let placed = view_rect(body, fit, z);
        z.center = center_of(body, placed);
        z.due = Some(Instant::now());
        self.request_desktop_top_draw();
    }

    /// Fitted again.
    pub(crate) fn peek_zoom_reset(&mut self) {
        if let Some(up) = self.peek.up.as_mut() {
            up.zoom = Zoom::new();
        }
        self.request_desktop_top_draw();
    }

    /// A press on the picture: a double click toggles fitted ↔ its own
    /// pixels; zoomed, a drag begins. Whether it was the zoom's.
    pub(crate) fn peek_zoom_press(&mut self, at: (f32, f32)) -> bool {
        let Some((body, fit, own)) = self.peek_zoom_frame() else {
            return false;
        };
        if !body.contains(at) {
            return false;
        }
        let scale = self.surface_scale(crate::fractional::SurfaceKind::DesktopTop);
        let Some(up) = self.peek.up.as_mut() else {
            return false;
        };
        let double = up
            .zoom
            .last_click
            .is_some_and(|t| t.elapsed().as_millis() < DOUBLE_MS);
        up.zoom.last_click = Some(Instant::now());
        if double {
            up.zoom.last_click = None;
            if up.zoom.zoomed() {
                self.peek_zoom_reset();
            } else {
                // (Its own pixels: one of them to one of the screen's.)
                let pdf = self.peek_shown().is_some_and(|(_, p)| p.pages > 0);
                let actual = if pdf {
                    2.5
                } else {
                    (own.0 as f32 / scale / fit.w).max(2.0)
                };
                self.peek_zoom_at(actual, at);
            }
            return true;
        }
        if up.zoom.zoomed() {
            up.zoom.drag = Some((at, up.zoom.center));
            return true;
        }
        false
    }

    /// The pointer moved: a drag moves the picture. Whether it was the zoom's.
    pub(crate) fn peek_zoom_motion(&mut self, at: (f32, f32)) -> bool {
        let Some((body, fit, _)) = self.peek_zoom_frame() else {
            return false;
        };
        let Some(up) = self.peek.up.as_mut() else {
            return false;
        };
        let Some((from, center)) = up.zoom.drag else {
            return false;
        };
        let z = &mut up.zoom;
        z.center = center;
        let r = view_rect(body, fit, z);
        let moved = Rect::new(r.x + at.0 - from.0, r.y + at.1 - from.1, r.w, r.h);
        z.center = center_of(body, moved);
        let placed = view_rect(body, fit, z);
        z.center = center_of(body, placed);
        z.due = Some(Instant::now());
        self.request_desktop_top_draw();
        true
    }

    pub(crate) fn peek_zoom_release(&mut self) {
        if let Some(up) = self.peek.up.as_mut() {
            up.zoom.drag = None;
        }
    }

    /// Once it stands still: the part on the screen read sharp, off the
    /// loop. Whether it is still waiting.
    pub(crate) fn peek_zoom_tick(&mut self) -> bool {
        let Some((body, fit, own)) = self.peek_zoom_frame() else {
            return false;
        };
        let scale = self.surface_scale(crate::fractional::SurfaceKind::DesktopTop);
        let Some(up) = self.peek.up.as_mut() else {
            return false;
        };
        let Some(due) = up.zoom.due else { return false };
        if due.elapsed().as_millis() < SETTLE_MS || up.zoom.drag.is_some() {
            return true;
        }
        up.zoom.due = None;
        let r = view_rect(body, fit, &up.zoom);
        // The part on the screen (a little more round it), in shares.
        let x0 = ((body.x - r.x) / r.w - 0.03).clamp(0.0, 1.0);
        let y0 = ((body.y - r.y) / r.h - 0.03).clamp(0.0, 1.0);
        let x1 = ((body.x + body.w - r.x) / r.w + 0.03).clamp(0.0, 1.0);
        let y1 = ((body.y + body.h - r.y) / r.h + 0.03).clamp(0.0, 1.0);
        let region = [x0, y0, x1 - x0, y1 - y0];
        // The whole picture at the size it is shown (never past its own).
        let whole = (
            ((r.w * scale) as u32).min(own.0).max(1),
            ((r.h * scale) as u32).min(own.1).max(1),
        );
        self.peek.tag += 1;
        let tag = self.peek.tag;
        up.zoom.tag = tag;
        let path = up.path.clone();
        let page = up.page_shown();
        let pdf = self.peek_shown().is_some_and(|(_, p)| p.pages > 0);
        let Some(tx) = self.peek_zoom_channel() else {
            return false;
        };
        let spawned = std::thread::Builder::new()
            .name("quick-look-zoom".into())
            .spawn(move || {
                let part = if pdf {
                    pdf_part(&path, page, whole, region)
                } else {
                    read_part(&path, whole, region)
                };
                if let Some(pic) = part {
                    let _ = tx.send((tag, pic, region));
                }
            });
        if let Err(e) = spawned {
            warn!("quick look: no zoom thread: {e}");
        }
        false
    }

    fn peek_zoom_channel(&mut self) -> Option<calloop::channel::Sender<(u64, Pic, [f32; 4])>> {
        if self.peek.zooms.is_none() {
            let (sender, receiver) = calloop::channel::channel();
            let inserted = self
                .loop_handle
                .insert_source(receiver, |event, _, app: &mut App| {
                    if let calloop::channel::Event::Msg((tag, pic, region)) = event {
                        if let Some(up) = app
                            .peek
                            .up
                            .as_mut()
                            .filter(|u| u.zoom.tag == tag && u.zoom.zoomed())
                        {
                            up.zoom.detail = Some((pic, region));
                            // (The tag: never the same for two pictures, so a
                            // part of another is never taken for this one's.)
                            up.zoom.detail_rev = tag;
                            app.request_desktop_top_draw();
                        }
                    }
                });
            if let Err(e) = inserted {
                warn!("quick look: {e}");
                return None;
            }
            self.peek.zooms = Some(sender);
        }
        self.peek.zooms.clone()
    }
}

/// `region` (shares) of the picture at `path`, the whole read at `whole`
/// texels (a JPEG decoded straight at that size), premultiplied, in tiles.
/// `region` (shares) of page `page` of a PDF drawn as if the whole page
/// were `whole` texels — only that part drawn (`pdftoppm`'s crop), on white.
fn pdf_part(path: &std::path::Path, page: u32, whole: (u32, u32), region: [f32; 4]) -> Option<Pic> {
    let (pw, _) = crate::peek_kinds::pdf_points(path, page)?;
    let dpi = whole.0 as f32 / pw * 72.0;
    let (x, y) = (
        (region[0] * whole.0 as f32) as u32,
        (region[1] * whole.1 as f32) as u32,
    );
    let (w, h) = (
        (region[2] * whole.0 as f32) as u32,
        (region[3] * whole.1 as f32) as u32,
    );
    let img = crate::peek_kinds::pdf_crop(path, page, dpi, (x, y, w.max(1), h.max(1)))?;
    let (fw, fh) = crate::quicklook::tile_fit(img.dimensions(), img.dimensions());
    let img = if (fw, fh) != img.dimensions() {
        image::imageops::thumbnail(&img, fw, fh)
    } else {
        img
    };
    Some(Pic::of_premultiplied(&img))
}

fn read_part(path: &std::path::Path, whole: (u32, u32), region: [f32; 4]) -> Option<Pic> {
    let name = path.file_name()?.to_string_lossy().to_ascii_lowercase();
    let img = if name.ends_with(".jpg") || name.ends_with(".jpeg") {
        crate::peek_kinds::jpeg_scaled(path, whole).map(|(img, _)| img)
    } else {
        None
    }
    .or_else(|| {
        crate::peek_edit::whole_picture(path).map(|img| {
            image::imageops::resize(
                &img,
                whole.0,
                whole.1,
                image::imageops::FilterType::Triangle,
            )
        })
    })?;
    let (w, h) = (img.width() as f32, img.height() as f32);
    let (x, y) = ((region[0] * w) as u32, (region[1] * h) as u32);
    let (cw, ch) = (
        ((region[2] * w) as u32).max(1).min(img.width() - x),
        ((region[3] * h) as u32).max(1).min(img.height() - y),
    );
    let mut part = image::imageops::crop_imm(&img, x, y, cw, ch).to_image();
    // (Within the layers it has.)
    let (fw, fh) = crate::quicklook::tile_fit(part.dimensions(), part.dimensions());
    if (fw, fh) != part.dimensions() {
        part = image::imageops::thumbnail(&part, fw, fh);
    }
    for px in part.pixels_mut() {
        let a = u32::from(px[3]);
        if a < 255 {
            for c in 0..3 {
                px[c] = (u32::from(px[c]) * a / 255) as u8;
            }
        }
    }
    Some(Pic::of_premultiplied(&part))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_zoomed_picture_covers_the_body_and_a_small_one_stays_centred() {
        let body = Rect::new(0.0, 0.0, 400.0, 300.0);
        let fit = Rect::new(0.0, 0.0, 400.0, 300.0);
        let mut z = Zoom::new();
        assert_eq!(view_rect(body, fit, &z), fit);
        z.k = 2.0;
        z.center = (0.0, 0.0);
        let r = view_rect(body, fit, &z);
        assert_eq!(
            (r.x, r.y, r.w, r.h),
            (0.0, 0.0, 800.0, 600.0),
            "pulled back to cover"
        );
        z.center = (1.0, 1.0);
        let r = view_rect(body, fit, &z);
        assert_eq!((r.x, r.y), (-400.0, -300.0));
    }
}
