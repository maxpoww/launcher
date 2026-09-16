//! The playing box: everything making sound, one row each.
//!
//! Scroll down on the track name and the pill **becomes** the list — the same
//! move the clipboard and notification OPTIONS make, built from the same parts
//! (Max, 2026-09-12: *"it have to follow all the same design, even it have to
//! use the same code as much as possible"*).
//!
//! # What is shared, and why that matters
//!
//! The first cut of this file drew its own panel: its own rect, its own radius,
//! its own fill, its own padding. It compiled and it looked wrong, because a
//! box on this bar is not a rectangle with rows in it — it is a **pill grown**,
//! and that is a specific set of behaviours:
//!
//! * one rect that morphs, so the thing you opened is the thing you are looking
//!   at — never a panel that appears near it;
//! * `radius` lerping from the stadium's `ph/2` to [`BOX_RADIUS`];
//! * opacity LEADING the height (`solid`), or the fill reads as a translucent
//!   ghost inflating instead of a solid panel swelling;
//! * the fill running from the resting pill wash to the box fill, so a
//!   half-open box is never a colour the bar does not otherwise contain;
//! * [`App::zebra_stripe`] and [`App::dim_ink`] for the list, which are already
//!   shared by the clipboard and notification histories.
//!
//! Every one of those comes from `options`/`clipboard` here rather than being
//! re-derived. The remaining duplication — row geometry and the scroll — is the
//! standing "no shared box machinery" debt, and this is now the fourth box
//! paying it.

use crate::content::{Label, Rect, RectInst, Scene};
use crate::options::{FONT_PX, LINE_PX, PILL_PAD_X, TEXT_FONT};
use crate::App;

/// Row height in pill-heights: art plus two lines of text.
const ROW_H: f32 = 2.0;
/// The art square's side, as a fraction of the row height.
const ART: f32 = 0.74;
/// Inset from the box's left edge to the art.
const PAD_X: f32 = 8.0;
/// Gap between the art and the words.
const ART_GAP: f32 = 9.0;

impl App {
    /// A decoded cover arrived: give it a layer and redraw.
    pub(crate) fn on_play_art(&mut self, art: crate::play_art::Art) {
        self.play_art_pending.remove(&art.key);
        let Some(chain) = art.chain else {
            return; // unreadable; the row keeps its empty plate
        };
        if self.play_art_slot.contains_key(&art.key) {
            return; // a duplicate reply
        }
        let slot = self.play_art_chains.len() as u32;
        self.play_art_chains.push(chain);
        self.play_art_slot.insert(art.key, slot);
        self.upload_options_icons();
        self.draw_options();
    }

    /// Ask for any cover we do not have yet. Called as the box opens, so
    /// nothing is decoded for a panel nobody looked at.
    pub(crate) fn request_play_art(&mut self) {
        let wanted: Vec<String> = self
            .play_rows()
            .iter()
            .filter_map(|p| p.art_url.clone())
            .filter(|u| !self.play_art_slot.contains_key(u) && !self.play_art_pending.contains(u))
            .collect();
        for url in wanted {
            if let Some(loader) = self.play_art.as_ref() {
                loader.request(url.clone());
            }
            self.play_art_pending.insert(url);
        }
    }

    /// The texture layer holding a source's cover, if it has arrived.
    ///
    /// The art block sits after the notification and clipboard blocks in the
    /// one OPTIONS icon array, so a slot is offset by both — see
    /// `Renderer::set_options_icons`, which concatenates them in that order.
    fn play_art_layer(&self, p: &options_engine::Playing) -> Option<u32> {
        let slot = *self.play_art_slot.get(p.art_url.as_deref()?)?;
        let base = self.notif_icon_chains.len() + self.clip.icon_chains.len();
        Some(base as u32 + slot)
    }

    /// Sources worth a row: everything the engine can see making or holding
    /// sound. Playing first, so what you can hear is at the top and a paused
    /// thing never pushes it down.
    pub(crate) fn play_rows(&self) -> Vec<options_engine::Playing> {
        let Some(ctx) = self.brain.as_ref() else {
            return Vec::new();
        };
        let mut rows = ctx.playing.clone();
        rows.sort_by_key(|p| u8::from(!p.is_playing()));
        rows
    }

    /// One row's height in px.
    pub(crate) fn play_row_h(&self) -> f32 {
        self.options_pill_h() * ROW_H
    }

    /// The box's full open height: the track band it grew from, plus a row per
    /// source. The band stays — the pill does not vanish into its own list.
    pub(crate) fn play_box_full_h(&self) -> f32 {
        let band = self.options_pill_h();
        band + self.play_rows().len() as f32 * self.play_row_h()
    }

    /// How far the pointer-sensitive region must reach while the box is open —
    /// the grown pill's own bottom, since the box IS the pill.
    pub(crate) fn play_box_input_bottom(&self) -> f32 {
        self.cava_now_rect()
            .map(|r| r.y + r.h)
            .unwrap_or_else(|| self.options_bar_h())
    }

    /// The list, drawn inside the grown pill below its track band.
    ///
    /// `rect` is the whole morphing pill; `solid` is the opacity-leads-height
    /// ease the clipboard uses, so the rows arrive with the panel rather than
    /// on top of a still-translucent one.
    pub(crate) fn push_play_rows(&self, scene: &mut Scene, rect: Rect, solid: f32) {
        let e = self.play_box_e;
        if e < 0.01 {
            return;
        }
        let s = self.options_scale();
        let band = self.options_pill_h();
        // The same adaptive pair every striped OPTIONS list uses. Sampled with
        // the CLIPBOARD's regime: it is the nearest correctly-positioned frost
        // (the cava cluster sits immediately right of the clipboard pill), and
        // `options_box_surface` is sampled beside the notification pill at the
        // OPPOSITE edge of the screen — the exact mistake that gave the
        // clipboard a violet zebra off a neutral wallpaper.
        let (fill, box_ink) = self.clip_box_surface();
        let stripe = self.zebra_stripe(fill);
        let ink = crate::animation::lerp4(self.options_text_color(), box_ink, solid);
        let dim = self.dim_ink(ink);
        let (font_px, line_px) = (FONT_PX * s, LINE_PX * s);
        let row_h = self.play_row_h();
        let art = row_h * ART;

        for (idx, p) in self.play_rows().iter().enumerate() {
            let rr = Rect::new(rect.x, rect.y + band + idx as f32 * row_h, rect.w, row_h);
            // Clipped to the opening box, so rows appear as it grows rather
            // than spilling past its edge.
            let top = rr.y.max(rect.y + band);
            let bot = (rr.y + rr.h).min(rect.y + rect.h);
            if bot <= top {
                continue;
            }
            // Zebra on odd rows — first row stays plain, as in both histories.
            if idx % 2 == 1 {
                scene.rects.push(RectInst {
                    rect: Rect::new(rr.x, top, rr.w, bot - top),
                    radius: 0.0,
                    color: stripe,
                    glass: 0.0,
                    border: 0.0,
                });
            }
            let clip = Rect::new(rr.x, top, rr.w, bot - top);
            let art_rect = Rect::new(rr.x + PAD_X * s, rr.y + (row_h - art) / 2.0, art, art);
            // A plate whether or not a picture has arrived, so the row's shape
            // never changes when one lands.
            scene.rects.push(RectInst {
                rect: art_rect,
                radius: 5.0 * s,
                color: [ink[0], ink[1], ink[2], 0.10 * solid],
                glass: 0.0,
                border: 0.0,
            });
            if let Some(layer) = self.play_art_layer(p) {
                crate::notif::push_boxed_icon(scene, clip, art_rect, layer);
            }

            let tx = art_rect.x + art + ART_GAP * s;
            let tw = (rr.x + rr.w - PILL_PAD_X - tx).max(1.0);
            let what = match (p.title.trim(), p.artist.trim()) {
                ("", _) => p.app.trim().to_owned(),
                (t, "") => t.to_owned(),
                (t, a) => format!("{a} — {t}"),
            };
            scene.labels.push(Label {
                text: what,
                pos: (tx, rr.y + row_h / 2.0 - line_px * 0.95),
                max_w: tw,
                font_px,
                line_px,
                centered: false,
                dim: false,
                cache: false,
                clip: Some(clip),
                family: TEXT_FONT,
                color: Some(ink),
            });
            scene.labels.push(Label {
                text: self.play_row_detail(p),
                pos: (tx, rr.y + row_h / 2.0 + line_px * 0.05),
                max_w: tw,
                font_px,
                line_px,
                centered: false,
                dim: true,
                cache: false,
                clip: Some(clip),
                family: TEXT_FONT,
                color: Some(dim),
            });
        }
    }

    /// A row's second line: who, what state, where it is, and where it comes
    /// out — the four facts the one-line pill has to leave out.
    fn play_row_detail(&self, p: &options_engine::Playing) -> String {
        let state = match p.state {
            options_engine::PlaybackState::Playing => "playing",
            options_engine::PlaybackState::Paused => "paused",
            options_engine::PlaybackState::Stopped => "stopped",
        };
        let ctx = self.brain.as_ref();
        let out = p
            .output
            .as_deref()
            .and_then(|name| {
                ctx?.outputs
                    .iter()
                    .find(|o| o.name == name)
                    .map(|o| o.description.clone())
            })
            .or_else(|| ctx?.default_output().map(|o| o.description.clone()))
            .unwrap_or_default();
        let place = ctx
            .and_then(|c| c.window_of(p))
            .map(|w| format!("  ·  ws{}", w.workspace_id))
            .unwrap_or_default();
        format!("{}  ·  {state}{place}  ·  {out}", p.app.trim())
    }
}
