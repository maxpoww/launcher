//! The sunset OPTION's settings box.
//!
//! Clicking the module's gear grows the current-task prompt DOWNWARD into a
//! panel — the same "pill becomes a box" morph the notification and clipboard
//! OPTIONS use. The shape is one continuous glass rect (see
//! [`crate::options::App::sunset_box_rect`], driven by `sunset_box_e`); this
//! module only lays the panel's *content* out against that rect and eases the
//! open progress. The message and [turn on] fade out as the box opens; the
//! gear stays at the top-right as the close affordance.

use std::time::{Duration, Instant};

use calloop::timer::{TimeoutAction, Timer};

use crate::animation::{ease_toward, settle_t, MORPH_RATE, SETTLE_PX};
use crate::content::{Label, Rect, RectInst, Scene};
use crate::options::{push_neumorph, FONT_PX, LINE_PX, SUNSET_GROW_H};
use crate::App;

/// The screen-temperature presets the panel offers (Kelvin). 6500 is
/// hyprsunset's neutral identity (protection off); lower is warmer.
const PRESETS: [u32; 4] = [6500, 5000, 4000, 3000];

impl App {
    /// Open/close the settings box (the gear toggles it). Opening starts the
    /// downward morph; closing eases it back to the pill.
    pub(crate) fn toggle_sunset_box(&mut self) {
        self.sunset_box_open = !self.sunset_box_open;
        self.sunset_box_last = None;
        self.schedule_sunset_box_frame();
        self.sync_options_input();
    }

    /// Force the box shut (e.g. when the prompt is resolved or dismissed).
    pub(crate) fn close_sunset_box(&mut self) {
        if self.sunset_box_open || self.sunset_box_e > 0.0 {
            self.sunset_box_open = false;
            self.sunset_box_last = None;
            self.schedule_sunset_box_frame();
            self.sync_options_input();
        }
    }

    /// The fully-expanded box bottom — the input region reaches this while the
    /// box is open (stable the instant it opens, like the other boxes).
    pub(crate) fn sunset_box_input_bottom(&self) -> f32 {
        let ph = self.options_pill_h();
        let module_h = ph + SUNSET_GROW_H;
        self.sunset_box_rect().y + (crate::options::SUNSET_BOX_H * self.options_scale()).max(module_h)
    }

    fn schedule_sunset_box_frame(&mut self) {
        if self.sunset_box_frame_pending {
            return;
        }
        self.sunset_box_frame_pending = true;
        if self.sunset_box_last.is_none() {
            self.sunset_box_last = Some(Instant::now());
        }
        let timer = Timer::from_duration(Duration::from_millis(8));
        let _ = self
            .loop_handle
            .insert_source(timer, |_, _, app: &mut App| {
                app.sunset_box_frame_pending = false;
                app.tick_sunset_box();
                TimeoutAction::Drop
            });
    }

    fn tick_sunset_box(&mut self) {
        let now = Instant::now();
        let dt = self
            .sunset_box_last
            .map_or(0.0, |l| now.duration_since(l).as_secs_f32())
            .min(0.05);
        self.sunset_box_last = Some(now);
        let target = if self.sunset_box_open { 1.0 } else { 0.0 };
        let span = (crate::options::SUNSET_BOX_H * self.options_scale()).max(1.0);
        let (e, moving) = ease_toward(
            self.sunset_box_e,
            target,
            dt,
            MORPH_RATE,
            settle_t(span).max(SETTLE_PX / span),
        );
        self.sunset_box_e = e;
        self.draw_options();
        if moving {
            self.schedule_sunset_box_frame();
        } else {
            self.sunset_box_last = None;
            // The box has fully closed — re-evaluate the prompt, so the module
            // withdraws now if the offer went away while the box was open
            // (see `sync_sunset_prompt`). Nothing happens if it's still offered.
            if !self.sunset_box_open && self.sunset_box_e <= 0.0 {
                self.sync_sunset_prompt();
            }
        }
    }

    /// The four preset-button rects (and their Kelvin values), laid out in a
    /// row in the panel's content area. Shared by draw and hit-test.
    fn sunset_presets(&self, rect: Rect) -> [(Rect, u32); 4] {
        let s = self.options_scale();
        let pad = 16.0 * s;
        let gap = 8.0 * s;
        let band_h = self.options_pill_h() + SUNSET_GROW_H;
        let row_y = rect.y + band_h + 34.0 * s;
        let btn_h = 34.0 * s;
        let avail = rect.w - 2.0 * pad;
        let btn_w = (avail - 3.0 * gap) / 4.0;
        std::array::from_fn(|i| {
            let x = rect.x + pad + i as f32 * (btn_w + gap);
            (Rect::new(x, row_y, btn_w, btn_h), PRESETS[i])
        })
    }

    /// The "automatically at sunset" toggle row's clickable rect.
    fn sunset_auto_rect(&self, rect: Rect) -> Rect {
        let s = self.options_scale();
        let pad = 16.0 * s;
        let band_h = self.options_pill_h() + SUNSET_GROW_H;
        let y = rect.y + band_h + 88.0 * s;
        Rect::new(rect.x + pad, y, rect.w - 2.0 * pad, 30.0 * s)
    }

    /// Draw the settings-box content over the (already-drawn) glass panel.
    /// Fades in with the open progress; nothing when the box is shut.
    pub(crate) fn push_sunset_box(&self, scene: &mut Scene) {
        let e = self.sunset_box_e;
        // Never draw the settings content unless the module is actually the
        // sunset prompt (defence against a stale box outliving the prompt).
        if e < 0.01 || !self.sunset_prompt_shown {
            return;
        }
        let rect = self.sunset_box_rect();
        let s = self.options_scale();
        let ink = self.sunset_module_ink();
        // The content only reads once the box is mostly a box — while the
        // shape is still a squished pill, it would be cramped garbage. Fade the
        // content in on the BACK half of the open (e: 0.5→1), so it appears in
        // a settled panel, not a morphing sliver.
        let a = ((e - 0.5) / 0.5).clamp(0.0, 1.0);
        if a < 0.01 {
            return;
        }
        let pad = 16.0 * s;
        let (font, line) = (FONT_PX * s, LINE_PX * s);
        let band_h = self.options_pill_h() + SUNSET_GROW_H;

        // Title in the top band (where the message was), left-aligned.
        let band_ty = rect.y + (band_h - line) / 2.0;
        scene.labels.push(Label {
            text: "Eye protection".to_owned(),
            pos: (rect.x + pad, band_ty),
            max_w: rect.w - 2.0 * pad,
            font_px: font,
            line_px: line,
            centered: false,
            dim: false,
            cache: false,
            family: None,
            color: Some([ink[0], ink[1], ink[2], ink[3] * a]),
            clip: Some(rect),
        });

        // A hairline divider under the header band.
        scene.rects.push(RectInst {
            rect: Rect::new(rect.x + pad, rect.y + band_h, rect.w - 2.0 * pad, (1.0 * s).max(1.0)),
            radius: 0.0,
            color: [ink[0], ink[1], ink[2], 0.10 * a],
            glass: 0.0,
            border: 0.0,
        });

        // "Screen temperature" caption above the preset row.
        let dim = [ink[0], ink[1], ink[2], ink[3] * 0.6 * a];
        scene.labels.push(Label {
            text: "Screen temperature".to_owned(),
            pos: (rect.x + pad, rect.y + band_h + 11.0 * s),
            max_w: rect.w - 2.0 * pad,
            font_px: 13.0 * s,
            line_px: 16.0 * s,
            centered: false,
            dim: false,
            cache: false,
            family: None,
            color: Some(dim),
            clip: Some(rect),
        });

        // Preset buttons — the box's own flat material, not the dock's glass:
        // the same fill+alpha pair the parent module and its [turn on]/gear
        // children already share (`sunset_fill_alpha`, `options.rs`), so
        // everything inside this panel reads as one continuous surface
        // (Max, 2026-09-08: "the buttons on the box are still the dock
        // material" — these were left on the pre-fix `glass: 1.0` costume
        // when [turn on]/gear were moved off it).
        let bright = self.options_bar_is_bright();
        let (bfill, _) = self.options_box_surface();
        let fa = self.sunset_fill_alpha();
        for (br, k) in self.sunset_presets(rect) {
            let radius = br.h / 2.0;
            push_neumorph(scene, br, radius, bright, a);
            scene.rects.push(RectInst {
                rect: br,
                radius,
                color: [bfill[0], bfill[1], bfill[2], fa * a],
                glass: 0.0,
                border: 0.0,
            });
            // Hairline border, a touch stronger on the active temperature.
            let active = self.sunset_temp == Some(k);
            let ba = if active { 0.5 } else { 0.11 };
            scene.rects.push(RectInst {
                rect: br,
                radius,
                color: [ink[0], ink[1], ink[2], ba * a],
                glass: 0.0,
                border: (1.4 * s).max(1.0),
            });
            scene.labels.push(Label {
                text: format!("{k}K"),
                pos: (br.x + br.w / 2.0, br.y + (br.h - line) / 2.0),
                max_w: br.w,
                font_px: font,
                line_px: line,
                centered: true,
                dim: false,
                cache: false,
                family: None,
                color: Some([ink[0], ink[1], ink[2], ink[3] * a]),
                clip: Some(br),
            });
        }

        // "Automatically at sunset" toggle row: a label + a pill switch.
        let ar = self.sunset_auto_rect(rect);
        scene.labels.push(Label {
            text: "Automatically at sunset".to_owned(),
            pos: (ar.x, ar.y + (ar.h - line) / 2.0),
            max_w: ar.w - 60.0 * s,
            font_px: 14.0 * s,
            line_px: line,
            centered: false,
            dim: false,
            cache: false,
            family: None,
            color: Some([ink[0], ink[1], ink[2], ink[3] * a]),
            clip: Some(rect),
        });
        // Switch: a stadium with a knob that slides on when enabled.
        let sw_w = 40.0 * s;
        let sw_h = 20.0 * s;
        let sw = Rect::new(ar.x + ar.w - sw_w, ar.y + (ar.h - sw_h) / 2.0, sw_w, sw_h);
        let on = self.sunset_auto;
        let track = if on {
            [ink[0], ink[1], ink[2], 0.35 * a]
        } else {
            [ink[0], ink[1], ink[2], 0.14 * a]
        };
        scene.rects.push(RectInst {
            rect: sw,
            radius: sw_h / 2.0,
            color: track,
            glass: 0.0,
            border: 0.0,
        });
        let knob_d = sw_h - 4.0 * s;
        let kx = if on {
            sw.x + sw.w - knob_d - 2.0 * s
        } else {
            sw.x + 2.0 * s
        };
        scene.rects.push(RectInst {
            rect: Rect::new(kx, sw.y + 2.0 * s, knob_d, knob_d),
            radius: knob_d / 2.0,
            color: [ink[0], ink[1], ink[2], ink[3] * a],
            glass: 0.0,
            border: 0.0,
        });
    }

    /// Hit-test a click at `(px, py)` against the open box's controls. Returns
    /// whether the click was consumed (so it doesn't fall through to the pill).
    pub(crate) fn sunset_box_click(&mut self, px: f32, py: f32) -> bool {
        if self.sunset_box_e < 0.5 {
            return false;
        }
        let rect = self.sunset_box_rect();
        let p = (px, py);
        for (br, k) in self.sunset_presets(rect) {
            if br.contains(p) {
                self.set_screen_temperature(k);
                return true;
            }
        }
        if self.sunset_auto_rect(rect).contains(p) {
            self.sunset_auto = !self.sunset_auto;
            tracing::info!("options: sunset auto {}", self.sunset_auto);
            self.draw_options();
            return true;
        }
        // A click inside the panel (but not on a control) is swallowed so it
        // doesn't dismiss anything; only a click OUTSIDE closes the box.
        rect.contains(p)
    }

    /// Apply a screen temperature via hyprsunset (starting it if needed), and
    /// remember it so the panel can mark the active preset. 6500 K is neutral
    /// (protection effectively off).
    fn set_screen_temperature(&mut self, k: u32) {
        let cmd = format!("hyprctl hyprsunset temperature {k} || hyprsunset -t {k}");
        if let Err(e) = crate::launch::launch(&cmd, false, &self.config.launch.terminal) {
            tracing::warn!("options: set temperature {k} failed: {e:#}");
        }
        self.sunset_temp = Some(k);
        self.draw_options();
    }
}
