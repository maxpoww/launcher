//! The media transport BOX — a media player is active and its box pill was
//! clicked, so a full panel grows into the topbar's reserved dropdown region:
//! track title, prev / play-pause / next, and click-to-set seek + volume bars.
//!
//! This is the "5-pill cap overflow" surface: the quick controls still ride the
//! bar as OPTION pills, but the box gives the whole transport in one place.
//! Drawn directly into the pre-reserved dropdown area (no layer resize), so it
//! sits alongside the clipboard/notification boxes without touching their code.

use crate::content::{Label, Rect, RectInst, Scene};
use crate::options::{EDGE_PAD, PILL_MARGIN_Y};
use options_engine::MediaState;

// FontAwesome nerd glyphs for the transport buttons.
const GLYPH_PLAY: &str = "\u{f04b}";
const GLYPH_PAUSE: &str = "\u{f04c}";
const GLYPH_PREV: &str = "\u{f048}"; // step-backward
const GLYPH_NEXT: &str = "\u{f051}"; // step-forward
const GLYPH_VOL: &str = "\u{f028}"; // volume-up

const BOX_W: f32 = 360.0;
const BOX_H: f32 = 128.0;
const PAD: f32 = 16.0;
const BTN_D: f32 = 34.0; // transport circle diameter
const BTN_GAP: f32 = 16.0;
const BAR_H: f32 = 6.0; // seek / volume track thickness

impl crate::App {
    /// The active media player (present and its source layer alive), if any.
    pub(crate) fn media_now(&self) -> Option<&MediaState> {
        let ctx = self.brain.as_ref()?;
        if !ctx.health.hardware.alive {
            return None;
        }
        ctx.media.as_ref()
    }

    /// The media box panel rect, just below the bar in the dropdown region.
    pub(crate) fn media_box_geom(&self) -> Rect {
        let y = self.config.options.height as f32 + PILL_MARGIN_Y;
        Rect::new(EDGE_PAD, y, BOX_W, BOX_H)
    }

    /// Fully-expanded bottom of the media box, for the input region.
    pub(crate) fn media_input_bottom(&self) -> f32 {
        let g = self.media_box_geom();
        g.y + g.h
    }

    /// The seek track rect (inside the box).
    fn seek_track(&self, box_r: Rect) -> Rect {
        Rect::new(
            box_r.x + PAD,
            box_r.y + PAD + 18.0 + BTN_D + 14.0,
            box_r.w - 2.0 * PAD,
            BAR_H,
        )
    }

    /// The volume track rect (inside the box), below the seek track.
    fn vol_track(&self, box_r: Rect) -> Rect {
        let s = self.seek_track(box_r);
        Rect::new(s.x + 20.0, s.y + 22.0, s.w - 20.0, BAR_H)
    }

    /// The three transport button rects: (prev, play/pause, next).
    fn transport_btns(&self, box_r: Rect) -> [Rect; 3] {
        let total = 3.0 * BTN_D + 2.0 * BTN_GAP;
        let x0 = box_r.x + (box_r.w - total) / 2.0;
        let y = box_r.y + PAD + 16.0;
        [
            Rect::new(x0, y, BTN_D, BTN_D),
            Rect::new(x0 + BTN_D + BTN_GAP, y, BTN_D, BTN_D),
            Rect::new(x0 + 2.0 * (BTN_D + BTN_GAP), y, BTN_D, BTN_D),
        ]
    }

    /// Draw the media box into the scene (panel + track + transport + bars).
    pub(crate) fn push_media_box(&self, scene: &mut Scene) {
        let Some(m) = self.media_now() else {
            return;
        };
        let box_r = self.media_box_geom();
        let (fill3, ink) = self.options_box_surface();
        let bright = self.options_bar_is_bright();
        let alpha = self.box_panel_alpha();
        let dim = [ink[0], ink[1], ink[2], ink[3] * 0.55];
        let accent = [ink[0], ink[1], ink[2], ink[3] * 0.9];
        let track_c = [ink[0], ink[1], ink[2], ink[3] * 0.18];

        // Panel.
        crate::options::push_neumorph(scene, box_r, 14.0, bright, alpha);
        scene.rects.push(RectInst {
            rect: box_r,
            radius: 14.0,
            color: [fill3[0], fill3[1], fill3[2], alpha],
            glass: 0.0,
            border: 0.0,
        });

        // Title — artist.
        let title = match (m.title.is_empty(), m.artist.is_empty()) {
            (false, false) => format!("{}  ·  {}", m.title, m.artist),
            (false, true) => m.title.clone(),
            _ => m.player_name.clone(),
        };
        scene.labels.push(Label {
            text: title,
            pos: (box_r.x + PAD, box_r.y + 8.0),
            max_w: box_r.w - 2.0 * PAD,
            font_px: 14.0,
            line_px: 18.0,
            centered: false,
            dim: false,
            cache: true,
            clip: Some(box_r),
            family: None,
            color: Some(ink),
        });

        // Transport buttons.
        let btns = self.transport_btns(box_r);
        let glyphs = [
            GLYPH_PREV,
            if m.is_playing {
                GLYPH_PAUSE
            } else {
                GLYPH_PLAY
            },
            GLYPH_NEXT,
        ];
        for (r, g) in btns.iter().zip(glyphs) {
            scene.rects.push(RectInst {
                rect: *r,
                radius: r.h / 2.0,
                color: track_c,
                glass: 0.0,
                border: 0.0,
            });
            scene.labels.push(Label {
                text: g.to_owned(),
                pos: (r.x + r.w / 2.0, r.y + (r.h - 16.0) / 2.0),
                max_w: r.w,
                font_px: 15.0,
                line_px: 16.0,
                centered: true,
                dim: false,
                cache: true,
                clip: Some(box_r),
                family: Some(crate::options::NERD),
                color: Some(accent),
            });
        }

        // Seek bar: track + filled progress.
        let seek = self.seek_track(box_r);
        let frac = if m.length_secs > 0 {
            (m.position_secs as f32 / m.length_secs as f32).clamp(0.0, 1.0)
        } else {
            0.0
        };
        push_bar(scene, seek, frac, track_c, accent);
        // Time labels (mm:ss / mm:ss) below the seek bar.
        if m.length_secs > 0 {
            scene.labels.push(Label {
                text: format!(
                    "{} / {}",
                    fmt_time(m.position_secs),
                    fmt_time(m.length_secs)
                ),
                pos: (seek.x, seek.y + 8.0),
                max_w: seek.w,
                font_px: 11.0,
                line_px: 14.0,
                centered: false,
                dim: true,
                cache: false,
                clip: Some(box_r),
                family: None,
                color: Some(dim),
            });
        }

        // Volume bar with a speaker glyph on its left.
        let vol = self.vol_track(box_r);
        let vfrac = self
            .brain
            .as_ref()
            .map(|c| (c.audio.default_sink_volume as f32 / 100.0).clamp(0.0, 1.5))
            .unwrap_or(0.0)
            .min(1.0);
        scene.labels.push(Label {
            text: GLYPH_VOL.to_owned(),
            pos: (vol.x - 20.0, vol.y - 6.0),
            max_w: 18.0,
            font_px: 13.0,
            line_px: 14.0,
            centered: false,
            dim: true,
            cache: true,
            clip: Some(box_r),
            family: Some(crate::options::NERD),
            color: Some(dim),
        });
        push_bar(scene, vol, vfrac, track_c, accent);
    }

    /// Handle a click at `(px, py)` while the media box is open. Returns whether
    /// the click was consumed (a transport button or a seek/volume set); a click
    /// elsewhere inside the box is swallowed too so it doesn't fall through.
    pub(crate) fn media_box_click(&mut self, px: f32, py: f32) -> bool {
        if !self.media_box_open {
            return false;
        }
        let box_r = self.media_box_geom();
        if !box_r.contains((px, py)) {
            return false;
        }
        // Transport buttons.
        let btns = self.transport_btns(box_r);
        if btns[0].contains((px, py)) {
            self.media_spawn(&["playerctl", "previous"]);
            return true;
        }
        if btns[1].contains((px, py)) {
            self.media_spawn(&["playerctl", "play-pause"]);
            return true;
        }
        if btns[2].contains((px, py)) {
            self.media_spawn(&["playerctl", "next"]);
            return true;
        }
        // Seek bar (a band around the thin track for easy hitting).
        let seek = self.seek_track(box_r);
        if hit_band(seek, px, py) {
            if let Some(len) = self.media_now().map(|m| m.length_secs).filter(|l| *l > 0) {
                let frac = ((px - seek.x) / seek.w).clamp(0.0, 1.0);
                let secs = (frac * len as f32).round() as u64;
                self.media_spawn(&["playerctl", "position", &secs.to_string()]);
            }
            return true;
        }
        // Volume bar.
        let vol = self.vol_track(box_r);
        if hit_band(vol, px, py) {
            let frac = ((px - vol.x) / vol.w).clamp(0.0, 1.0);
            self.media_spawn(&[
                "wpctl",
                "set-volume",
                "@DEFAULT_AUDIO_SINK@",
                &format!("{frac:.2}"),
            ]);
            return true;
        }
        // A click on the panel but not a control: swallow (keep the box open).
        true
    }

    /// Fire a media control command, fully detached (shell-quoted; argv is
    /// engine/daemon-owned, never raw user text).
    fn media_spawn(&self, argv: &[&str]) {
        let line = argv
            .iter()
            .map(|a| crate::launch::shell_quote(a))
            .collect::<Vec<_>>()
            .join(" ");
        if let Err(e) = crate::launch::launch(&line, false, &self.config.launch.terminal) {
            tracing::warn!("media box: spawn failed ({line}): {e:#}");
        }
    }
}

/// Draw a track bar with a filled portion `frac` (0..=1).
fn push_bar(scene: &mut Scene, track: Rect, frac: f32, track_c: [f32; 4], fill_c: [f32; 4]) {
    scene.rects.push(RectInst {
        rect: track,
        radius: track.h / 2.0,
        color: track_c,
        glass: 0.0,
        border: 0.0,
    });
    let fw = (track.w * frac.clamp(0.0, 1.0)).max(track.h);
    scene.rects.push(RectInst {
        rect: Rect::new(track.x, track.y, fw, track.h),
        radius: track.h / 2.0,
        color: fill_c,
        glass: 0.0,
        border: 0.0,
    });
}

/// Whether `(px, py)` is within a generous vertical band around a thin track
/// (so a 6px bar is easy to click).
fn hit_band(track: Rect, px: f32, py: f32) -> bool {
    px >= track.x
        && px <= track.x + track.w
        && py >= track.y - 10.0
        && py <= track.y + track.h + 10.0
}

/// `123` seconds → `2:03`.
fn fmt_time(secs: u64) -> String {
    format!("{}:{:02}", secs / 60, secs % 60)
}
