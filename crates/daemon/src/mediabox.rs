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

/// An in-progress drag on the media box's seek or volume bar.
#[derive(Debug, Clone, Copy)]
pub(crate) struct MediaDrag {
    pub(crate) seek: bool, // true = seek bar, false = volume bar
    pub(crate) frac: f32,  // current dragged fraction 0..=1
}

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
        let s = self.options_scale();
        let y = self.options_bar_h() + PILL_MARGIN_Y;
        Rect::new(EDGE_PAD, y, BOX_W * s, BOX_H * s)
    }

    /// Fully-expanded bottom of the media box, for the input region.
    pub(crate) fn media_input_bottom(&self) -> f32 {
        let g = self.media_box_geom();
        g.y + g.h
    }

    /// The seek track rect (inside the box), at the live box scale.
    fn seek_track(&self, box_r: Rect) -> Rect {
        seek_track_at(box_r, self.options_scale())
    }

    /// The volume track rect (inside the box), at the live box scale.
    fn vol_track(&self, box_r: Rect) -> Rect {
        vol_track_at(box_r, self.options_scale())
    }

    /// The three transport button rects, at the live box scale.
    fn transport_btns(&self, box_r: Rect) -> [Rect; 3] {
        transport_btns_at(box_r, self.options_scale())
    }

    /// Draw the media box into the scene (panel + track + transport + bars).
    pub(crate) fn push_media_box(&self, scene: &mut Scene) {
        let Some(m) = self.media_now() else {
            return;
        };
        let box_r = self.media_box_geom();
        let s = self.options_scale();
        let (fill3, ink) = self.options_box_surface();
        let bright = self.options_bar_is_bright();
        let alpha = self.box_panel_alpha();
        let dim = [ink[0], ink[1], ink[2], ink[3] * 0.55];
        let accent = [ink[0], ink[1], ink[2], ink[3] * 0.9];
        let track_c = [ink[0], ink[1], ink[2], ink[3] * 0.18];

        // Panel.
        let panel_r = 14.0 * s;
        crate::options::push_neumorph(scene, box_r, panel_r, bright, alpha);
        scene.rects.push(RectInst {
            rect: box_r,
            radius: panel_r,
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
            pos: (box_r.x + PAD * s, box_r.y + 8.0 * s),
            max_w: box_r.w - 2.0 * PAD * s,
            font_px: 14.0 * s,
            line_px: 18.0 * s,
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
                pos: (r.x + r.w / 2.0, r.y + (r.h - 16.0 * s) / 2.0),
                max_w: r.w,
                font_px: 15.0 * s,
                line_px: 16.0 * s,
                centered: true,
                dim: false,
                cache: true,
                clip: Some(box_r),
                family: Some(crate::options::NERD),
                color: Some(accent),
            });
        }

        // Seek bar: track + filled progress (the live drag fraction while
        // dragging, else the sensed position).
        let seek = self.seek_track(box_r);
        let seek_drag = self.media_drag.filter(|d| d.seek).map(|d| d.frac);
        let frac = seek_drag.unwrap_or(if m.length_secs > 0 {
            (m.position_secs as f32 / m.length_secs as f32).clamp(0.0, 1.0)
        } else {
            0.0
        });
        push_bar(scene, seek, frac, track_c, accent);
        // Time labels (mm:ss / mm:ss) below the seek bar.
        if m.length_secs > 0 {
            scene.labels.push(Label {
                text: format!(
                    "{} / {}",
                    fmt_time(m.position_secs),
                    fmt_time(m.length_secs)
                ),
                pos: (seek.x, seek.y + 8.0 * s),
                max_w: seek.w,
                font_px: 11.0 * s,
                line_px: 14.0 * s,
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
            .media_drag
            .filter(|d| !d.seek)
            .map(|d| d.frac)
            .unwrap_or_else(|| {
                self.brain
                    .as_ref()
                    .map(|c| (c.audio.default_sink_volume as f32 / 100.0).clamp(0.0, 1.5))
                    .unwrap_or(0.0)
                    .min(1.0)
            });
        scene.labels.push(Label {
            text: GLYPH_VOL.to_owned(),
            pos: (vol.x - 20.0 * s, vol.y - 6.0 * s),
            max_w: 18.0 * s,
            font_px: 13.0 * s,
            line_px: 14.0 * s,
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
        // The seek and volume bars are handled by the drag path (press → drag →
        // release-commit), so a plain click there is already consumed. Any other
        // click inside the panel is swallowed so it doesn't fall through.
        true
    }

    /// Begin a drag if the press at `(px, py)` landed on the seek or volume
    /// bar. Returns whether a drag started (so the press is consumed).
    pub(crate) fn media_drag_start(&mut self, px: f32, py: f32) -> bool {
        if !self.media_box_open {
            return false;
        }
        let box_r = self.media_box_geom();
        let seek = self.seek_track(box_r);
        let vol = self.vol_track(box_r);
        let bar = if hit_band(seek, px, py) {
            Some((true, seek))
        } else if hit_band(vol, px, py) {
            Some((false, vol))
        } else {
            None
        };
        if let Some((is_seek, track)) = bar {
            let frac = ((px - track.x) / track.w).clamp(0.0, 1.0);
            self.media_drag = Some(MediaDrag {
                seek: is_seek,
                frac,
            });
            self.draw_options();
            return true;
        }
        false
    }

    /// Update the in-progress drag's fraction from the pointer x. Returns
    /// whether a drag is active (so the motion is consumed).
    pub(crate) fn media_drag_update(&mut self, px: f32) -> bool {
        let Some(mut d) = self.media_drag else {
            return false;
        };
        let box_r = self.media_box_geom();
        let track = if d.seek {
            self.seek_track(box_r)
        } else {
            self.vol_track(box_r)
        };
        d.frac = ((px - track.x) / track.w).clamp(0.0, 1.0);
        self.media_drag = Some(d);
        self.draw_options();
        true
    }

    /// Commit the drag on release: seek to the fraction of the track length, or
    /// set the sink volume. Returns whether a drag was committed.
    pub(crate) fn media_drag_commit(&mut self) -> bool {
        let Some(d) = self.media_drag.take() else {
            return false;
        };
        if d.seek {
            if let Some(len) = self.media_now().map(|m| m.length_secs).filter(|l| *l > 0) {
                let secs = (d.frac * len as f32).round() as u64;
                self.media_spawn(&["playerctl", "position", &secs.to_string()]);
            }
        } else {
            self.media_spawn(&[
                "wpctl",
                "set-volume",
                "@DEFAULT_AUDIO_SINK@",
                &format!("{:.2}", d.frac),
            ]);
        }
        self.draw_options();
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

/// The seek track rect inside `box_r`, with every offset scaled by `s` — the
/// same factor `media_box_geom` applies to the panel, so the layout is uniform.
fn seek_track_at(box_r: Rect, s: f32) -> Rect {
    Rect::new(
        box_r.x + PAD * s,
        box_r.y + (PAD + 18.0 + BTN_D + 14.0) * s,
        box_r.w - 2.0 * PAD * s,
        BAR_H * s,
    )
}

/// The volume track rect inside `box_r`, below the seek track.
fn vol_track_at(box_r: Rect, s: f32) -> Rect {
    let t = seek_track_at(box_r, s);
    Rect::new(t.x + 20.0 * s, t.y + 22.0 * s, t.w - 20.0 * s, BAR_H * s)
}

/// The three transport button rects inside `box_r`: (prev, play/pause, next).
fn transport_btns_at(box_r: Rect, s: f32) -> [Rect; 3] {
    let (btn_d, btn_gap) = (BTN_D * s, BTN_GAP * s);
    let total = 3.0 * btn_d + 2.0 * btn_gap;
    let x0 = box_r.x + (box_r.w - total) / 2.0;
    let y = box_r.y + (PAD + 16.0) * s;
    [
        Rect::new(x0, y, btn_d, btn_d),
        Rect::new(x0 + btn_d + btn_gap, y, btn_d, btn_d),
        Rect::new(x0 + 2.0 * (btn_d + btn_gap), y, btn_d, btn_d),
    ]
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

#[cfg(test)]
mod tests {
    use super::{
        fmt_time, hit_band, seek_track_at, transport_btns_at, vol_track_at, BAR_H, BOX_H, BOX_W,
        BTN_D, BTN_GAP, PAD,
    };
    use crate::content::Rect;

    /// The panel as `media_box_geom` builds it: edge-anchored origin, scaled size.
    fn box_at(s: f32) -> Rect {
        Rect::new(24.0, 40.0, BOX_W * s, BOX_H * s)
    }

    #[test]
    fn box_scaling_is_lockstep_and_reduces_at_unity() {
        // Unity is a strict no-op vs the pre-scale constants.
        let b = box_at(1.0);
        let seek = seek_track_at(b, 1.0);
        assert!((seek.x - (b.x + PAD)).abs() < 0.01);
        assert!((seek.y - (b.y + PAD + 18.0 + BTN_D + 14.0)).abs() < 0.01);
        assert!((seek.w - (b.w - 2.0 * PAD)).abs() < 0.01);
        assert!((seek.h - BAR_H).abs() < 0.01);
        let btns = transport_btns_at(b, 1.0);
        assert!((btns[1].x - btns[0].x - (BTN_D + BTN_GAP)).abs() < 0.01);
        assert!((btns[0].w - BTN_D).abs() < 0.01);

        // Every interior offset scales by exactly the box factor (no drift), and
        // nothing interactive escapes the shrunken panel.
        for s in [0.82_f32, 0.889, 0.95] {
            let bs = box_at(s);
            for (full, small) in [
                (seek, seek_track_at(bs, s)),
                (vol_track_at(b, 1.0), vol_track_at(bs, s)),
                (btns[0], transport_btns_at(bs, s)[0]),
                (btns[2], transport_btns_at(bs, s)[2]),
            ] {
                assert!(
                    ((small.x - bs.x) - (full.x - b.x) * s).abs() < 0.01
                        && ((small.y - bs.y) - (full.y - b.y) * s).abs() < 0.01,
                    "offset scales at {s}"
                );
                assert!(
                    (small.w - full.w * s).abs() < 0.01 && (small.h - full.h * s).abs() < 0.01,
                    "size scales at {s}"
                );
                assert!(
                    small.x >= bs.x
                        && small.y >= bs.y
                        && small.x + small.w <= bs.x + bs.w + 0.01
                        && small.y + small.h <= bs.y + bs.h + 0.01,
                    "stays inside the panel at {s}"
                );
            }
        }
    }

    #[test]
    fn fmt_time_pads_seconds_and_carries_minutes() {
        assert_eq!(fmt_time(0), "0:00");
        assert_eq!(fmt_time(5), "0:05");
        assert_eq!(fmt_time(65), "1:05");
        assert_eq!(fmt_time(123), "2:03");
        // Long track: minutes are not wrapped to hours (a plain m:ss clock).
        assert_eq!(fmt_time(3661), "61:01");
    }

    #[test]
    fn hit_band_is_generous_vertically_but_bounded_horizontally() {
        let track = Rect::new(100.0, 200.0, 80.0, 6.0);
        // On the bar.
        assert!(hit_band(track, 140.0, 203.0));
        // Within the 10px vertical slop above/below.
        assert!(hit_band(track, 140.0, 192.0));
        assert!(hit_band(track, 140.0, 214.0));
        // Past the vertical slop → miss.
        assert!(!hit_band(track, 140.0, 180.0));
        // Left/right of the track → miss (no horizontal slop).
        assert!(!hit_band(track, 90.0, 203.0));
        assert!(!hit_band(track, 200.0, 203.0));
    }
}
