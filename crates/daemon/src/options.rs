//! OPTIONS content: the modular "pills" on the topbar and their behaviour.
//!
//! OPTIONS is the context-aware layer that lives on the topbar (see
//! [`crate::screencopy`] for the bar's diegetic colour-matching). Its UI is
//! built from independent **pill** modules; which ones show depends on context.
//! For now: a clock pill (far right), the focused window's name (centre), and
//! two control circles just right of it — a red close and a pseudotile toggle.
//!
//! Text pills use the **dock's font** (the default SansSerif — DejaVu Sans);
//! icon pills use a **Nerd Font**. Backgrounds are transparent, brightening on
//! hover; a pill holding a single glyph is a perfect circle (radius = height/2
//! makes every pill a stadium, which is a circle when width == height).
//! Proportional text means pill widths are measured (cached) rather than
//! estimated.

use std::time::{Duration, Instant};

use calloop::timer::{TimeoutAction, Timer};
use smithay_client_toolkit::reexports::protocols::wp::cursor_shape::v1::client::wp_cursor_shape_device_v1::Shape;
use smithay_client_toolkit::shell::WaylandSurface;
use wayland_client::protocol::wl_pointer;
use wayland_client::WEnum;

use crate::content::{Label, Rect, RectInst, Scene, ShadowInst};
use crate::{hypr, surface, App, BTN_LEFT};

/// Thickness of the top reveal strip (logical px) — the only pointer-sensitive
/// band while the bar is hidden in fullscreen.
const REVEAL_PX: f32 = 3.0;
/// How long the pointer must dwell at the top edge before the bar reveals — a
/// long, deliberate hold so it only happens when really intended.
const REVEAL_DWELL: Duration = Duration::from_millis(1500);
/// Grace after the pointer leaves the revealed bar before it conceals again.
const HIDE_GRACE: Duration = Duration::from_millis(500);

/// Nerd Font for the icon glyphs (close / pseudotile).
const NERD: &str = "JetBrainsMono Nerd Font Mono";
/// Text (clock, window name) uses `None` → the default SansSerif, which is
/// exactly the font the dock uses (fontconfig resolves it to DejaVu Sans).
const TEXT_FONT: Option<&str> = None;
const FONT_PX: f32 = 17.0;
const LINE_PX: f32 = 20.0;

/// Margin above/below the pills — leaves room for the neumorphic rim so the
/// pills themselves stay compact rather than filling the whole bar.
const PILL_MARGIN_Y: f32 = 2.5;
const PILL_PAD_X: f32 = 11.5;
const EDGE_PAD: f32 = 6.0;
/// Gaps between the window pill and the controls, and between the two control
/// circles — 3px, so each button keeps its full round outline (and its rim).
const GROUP_GAP: f32 = 3.0;
const CTRL_GAP: f32 = 3.0;
const TITLE_MAX: usize = 48;

// Nerd Font glyphs (Font Awesome range, present in JetBrainsMono NF).
const GLYPH_CLOSE: &str = "\u{f00d}"; // fa-times
const GLYPH_SQUARE: &str = "\u{f096}"; // fa-square-o (pseudotile)
const GLYPH_FLOAT: &str = "\u{f2d2}"; // fa-window-restore (floating)
const GLYPH_FULL: &str = "\u{f065}"; // fa-expand (fullscreen)

// Pill backgrounds (resting + hover) are adaptive washes — see
// `options_rest_wash` / `options_hover_wash`.
// The close glyph is red; its pill just brightens on hover like the rest.
const RED_GLYPH: [f32; 4] = [0.92, 0.30, 0.30, 1.0];

// A single, tiny soft shadow around the button circles for a touch of depth —
// black when the bar is bright, white when it's dark (never both). Strengths
// are display-meaningful (the white is gamma-corrected like the pill wash).
const NEU_BLUR: f32 = 3.5;
const NEU_DARK: f32 = 0.24; // black shadow on a bright bar
const NEU_LIGHT: f32 = 0.11; // white shadow on a dark bar

/// Which surface the pointer is over (only `Enter` carries the surface).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum PointerSurface {
    Dock,
    Options,
}

/// The pill modules currently on the bar.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum PillId {
    Clock,
    Window,
    Close,
    Pseudo,
    Float,
    Fullscreen,
}

struct Pill {
    id: PillId,
    rect: Rect,
    text: String,
    /// Font family (`None` = the dock's SansSerif; `Some` for the Nerd icons).
    family: Option<&'static str>,
    /// Glyph colour override (`None` = default text colour).
    glyph_color: Option<[f32; 4]>,
}

/// Local time as `HH:MM`, via libc so it respects the timezone.
fn clock_now() -> String {
    // SAFETY: `localtime_r` fills a caller-owned `tm`; `time` takes null.
    unsafe {
        let t = libc::time(std::ptr::null_mut());
        let mut tm: libc::tm = std::mem::zeroed();
        libc::localtime_r(&t, &mut tm);
        format!("{:02}:{:02}", tm.tm_hour, tm.tm_min)
    }
}

fn srgb_to_linear(c: f32) -> f32 {
    if c <= 0.04045 {
        c / 12.92
    } else {
        ((c + 0.055) / 1.055).powf(2.4)
    }
}

/// Build a pill wash whose alpha `a` means its true **on-screen** strength.
///
/// The bar's swapchain is an sRGB surface and the renderer outputs premultiplied
/// colour, so a plain white overlay's RGB is gamma-lifted far above its alpha —
/// a low-alpha white wash looks much stronger than the number implies (e.g.
/// `0.04` shows as ~22% grey on black). Pre-dividing the linearised alpha back
/// out of the RGB cancels that, so `a` becomes the actual displayed fraction.
/// Black needs no correction (0 stays 0 through the encode).
fn wash(white: bool, a: f32) -> [f32; 4] {
    if white && a > 0.0 {
        let v = srgb_to_linear(a) / a;
        [v, v, v, a]
    } else {
        [0.0, 0.0, 0.0, a]
    }
}

/// Push a single tiny soft shadow around a circle for a touch of depth: black
/// on a bright bar, white on a dark one (a uniform exterior penumbra). The
/// white is gamma-corrected so its strength means its true on-screen level.
fn push_neumorph(scene: &mut Scene, rect: Rect, radius: f32, bright: bool) {
    let color = if bright {
        [0.0, 0.0, 0.0, NEU_DARK]
    } else {
        let v = srgb_to_linear(NEU_LIGHT) / NEU_LIGHT;
        [v, v, v, NEU_LIGHT]
    };
    scene.overlay_shadows.push(ShadowInst {
        rect,
        radius,
        blur: NEU_BLUR,
        color,
        edges: [1.0, 1.0, 1.0, 1.0], // uniform soft penumbra all around
    });
}

fn truncate(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        return s.to_owned();
    }
    let head: String = s.chars().take(max.saturating_sub(1)).collect();
    format!("{head}…")
}

impl App {
    /// Startup clock value, so the pill shows immediately.
    pub(crate) fn options_clock_init() -> String {
        clock_now()
    }

    /// Compute the current pills (interactive modules) for the bar state.
    fn options_pills(&self) -> Vec<Pill> {
        let w = self.options_size.0 as f32;
        let bar_h = self.config.options.height as f32;
        if w == 0.0 {
            return Vec::new();
        }
        // Pills fill almost the whole bar height, top to bottom.
        let ph = (bar_h - 2.0 * PILL_MARGIN_Y).max(1.0);
        let y = PILL_MARGIN_Y;
        let mut pills = Vec::new();

        // Clock, far right.
        if !self.options_clock.is_empty() {
            let cw = (self.options_clock_w + 2.0 * PILL_PAD_X).max(ph);
            pills.push(Pill {
                id: PillId::Clock,
                rect: Rect::new(w - EDGE_PAD - cw, y, cw, ph),
                text: self.options_clock.clone(),
                family: TEXT_FONT,
                glyph_color: None,
            });
        }

        // The focused-window cluster, centred as a group:
        //   [X] [window name] [pseudo] [float] [fullscreen]
        if let Some(title) = &self.options_title {
            let shown = truncate(title, TITLE_MAX);
            let ww = (self.options_title_w + 2.0 * PILL_PAD_X).max(ph);
            let d = ph; // control-circle diameter
            let group_w =
                d + GROUP_GAP + ww + GROUP_GAP + d + CTRL_GAP + d + CTRL_GAP + d;
            let mut x = ((w - group_w) / 2.0).max(EDGE_PAD);
            let circle = |pills: &mut Vec<Pill>, x: f32, id, glyph: &str, color| {
                pills.push(Pill {
                    id,
                    rect: Rect::new(x, y, d, d),
                    text: glyph.to_owned(),
                    family: Some(NERD),
                    glyph_color: color,
                });
            };
            // Close, left of the window name.
            circle(&mut pills, x, PillId::Close, GLYPH_CLOSE, Some(RED_GLYPH));
            x += d + GROUP_GAP;
            pills.push(Pill {
                id: PillId::Window,
                rect: Rect::new(x, y, ww, ph),
                text: shown,
                family: TEXT_FONT,
                glyph_color: None,
            });
            x += ww + GROUP_GAP;
            // Window-mode toggles, right of the window name.
            circle(&mut pills, x, PillId::Pseudo, GLYPH_SQUARE, None);
            x += d + CTRL_GAP;
            circle(&mut pills, x, PillId::Float, GLYPH_FLOAT, None);
            x += d + CTRL_GAP;
            circle(&mut pills, x, PillId::Fullscreen, GLYPH_FULL, None);
        }
        pills
    }

    /// Re-measure the clock + window-title text widths (proportional font, so
    /// widths must be measured, not estimated). Cheap; only on data change.
    pub(crate) fn measure_options_text(&mut self) {
        let clock = self.options_clock.clone();
        let title = self.options_title.as_ref().map(|t| truncate(t, TITLE_MAX));
        let Some(r) = self.options_renderer.as_mut() else {
            return;
        };
        let cw = r.measure_text(&clock, FONT_PX, TEXT_FONT);
        let tw = title
            .as_deref()
            .map_or(0.0, |t| r.measure_text(t, FONT_PX, TEXT_FONT));
        self.options_clock_w = cw;
        self.options_title_w = tw;
    }

    /// Whether the matched bar is bright enough to want dark text/ink.
    /// (`options_bar_matched` is stored linear, so this is true relative
    /// luminance; 0.179 is the WCAG flip point where black and white contrast
    /// equally.) A transparent bar counts as dark.
    fn options_bar_is_bright(&self) -> bool {
        match self.options_bar_matched {
            Some(c) => 0.2126 * c[0] + 0.7152 * c[1] + 0.0722 * c[2] > 0.179,
            None => false,
        }
    }

    /// Adaptive default text colour: black on a bright matched bar, white on a
    /// dark one, so the pills stay legible against the window they blend into.
    /// Falls back to the theme colour when the bar is transparent.
    pub(crate) fn options_text_color(&self) -> [f32; 4] {
        match self.options_bar_matched {
            Some(_) if self.options_bar_is_bright() => [0.0, 0.0, 0.0, 1.0],
            Some(_) => [1.0, 1.0, 1.0, 1.0],
            None => self.config.theme.text_rgba(),
        }
    }

    /// Resting pill background — adaptive to the bar's brightness (same
    /// detector as the text colour). The alphas are **asymmetric on purpose**:
    /// white-on-dark reads ~2–3× stronger than black-on-white at equal alpha
    /// (we're far more sensitive to light added to darkness), so the white
    /// wash must be much lighter to feel as subtle as the black one.
    fn options_rest_wash(&self) -> [f32; 4] {
        if self.options_bar_is_bright() {
            wash(false, 0.10)
        } else {
            wash(true, 0.11)
        }
    }

    /// Hover wash — stronger than the resting wash, with the same asymmetry.
    fn options_hover_wash(&self) -> [f32; 4] {
        if self.options_bar_is_bright() {
            wash(false, 0.22)
        } else {
            wash(true, 0.20)
        }
    }

    /// Add the OPTIONS pills to the bar's scene (called after the base fill).
    pub(crate) fn push_options_pills(&self, scene: &mut Scene) {
        let hover_wash = self.options_hover_wash();
        let rest_wash = self.options_rest_wash();
        let bright = self.options_bar_is_bright();
        for pill in &self.options_pills() {
            let radius = pill.rect.h / 2.0; // stadium ⇒ circle when w == h
            // A touch of depth on every pill.
            push_neumorph(scene, pill.rect, radius, bright);
            let bg = if self.options_hover == Some(pill.id) {
                hover_wash
            } else {
                rest_wash
            };
            scene.rects.push(RectInst {
                rect: pill.rect,
                radius,
                color: bg,
                glass: 0.0,
            });
            scene.labels.push(Label {
                text: pill.text.clone(),
                pos: (
                    pill.rect.x + pill.rect.w / 2.0,
                    pill.rect.y + (pill.rect.h - LINE_PX) / 2.0,
                ),
                max_w: pill.rect.w,
                font_px: FONT_PX,
                line_px: LINE_PX,
                centered: true,
                dim: false,
                cache: true,
                family: pill.family,
                color: pill.glyph_color,
                clip: None,
            });
        }
    }

    /// Refresh the focused-window pill (title + address) on layout changes.
    pub(crate) fn refresh_options_content(&mut self) {
        if self.options_layer.is_none() {
            return;
        }
        let (addr, title, fullscreen) = match hypr::active_window_info() {
            Some((a, t, fs)) => (Some(a), Some(t), fs),
            None => (None, None, false),
        };
        if self.options_active_addr != addr || self.options_title != title {
            self.options_active_addr = addr;
            self.options_title = title;
            self.measure_options_text();
            self.sync_options_input();
            self.draw_options();
        }
        self.set_options_fullscreen(fullscreen);
    }

    /// React to the focused window entering/leaving fullscreen: conceal the
    /// bar while fullscreen (it reveals on a deliberate top-edge hold), show it
    /// again otherwise.
    fn set_options_fullscreen(&mut self, fs: bool) {
        if fs == self.options_fullscreen {
            return;
        }
        self.options_fullscreen = fs;
        self.options_reveal_deadline = None;
        self.options_hide_deadline = None;
        self.options_hidden = fs;
        if fs {
            self.options_hover = None;
        }
        self.sync_options_input();
        self.draw_options();
    }

    /// Arm the dwell timer that reveals a concealed bar. Idempotent while pending.
    fn arm_options_reveal(&mut self) {
        if self.options_reveal_deadline.is_some() {
            return;
        }
        let deadline = Instant::now() + REVEAL_DWELL;
        self.options_reveal_deadline = Some(deadline);
        let timer = Timer::from_duration(REVEAL_DWELL);
        let _ = self.loop_handle.insert_source(timer, move |_, _, app: &mut App| {
            if app.options_reveal_deadline == Some(deadline) {
                app.options_reveal_deadline = None;
                let still_at_top = app.options_ptr.is_some_and(|(_, y)| y <= REVEAL_PX);
                if app.options_hidden && still_at_top {
                    app.options_hidden = false;
                    app.sync_options_input();
                    app.draw_options();
                }
            }
            TimeoutAction::Drop
        });
    }

    /// Conceal the revealed bar after the grace period (unless the pointer came
    /// back or fullscreen ended).
    fn schedule_options_hide(&mut self) {
        let deadline = Instant::now() + HIDE_GRACE;
        self.options_hide_deadline = Some(deadline);
        let timer = Timer::from_duration(HIDE_GRACE);
        let _ = self.loop_handle.insert_source(timer, move |_, _, app: &mut App| {
            if app.options_hide_deadline == Some(deadline) {
                app.options_hide_deadline = None;
                if app.options_fullscreen && !app.options_hidden && app.options_ptr.is_none() {
                    app.options_hidden = true;
                    app.options_hover = None;
                    app.sync_options_input();
                    app.draw_options();
                }
            }
            TimeoutAction::Drop
        });
    }

    /// Tick the clock; returns whether the displayed `HH:MM` changed.
    pub(crate) fn tick_options_clock(&mut self) -> bool {
        let now = clock_now();
        if now != self.options_clock {
            self.options_clock = now;
            self.measure_options_text();
            true
        } else {
            false
        }
    }

    /// Set the surface's pointer input region: the whole bar strip while shown
    /// (so hover works across pills and the reveal can auto-hide on leave), or
    /// just the thin top reveal strip while concealed in fullscreen.
    pub(crate) fn sync_options_input(&mut self) {
        let (w, _) = self.options_size;
        if w == 0 {
            return;
        }
        let Some(layer) = self.options_layer.as_ref() else {
            return;
        };
        let h = if self.options_hidden {
            REVEAL_PX.ceil() as i32
        } else {
            self.config.options.height as i32
        };
        surface::set_input_rects(&self.compositor, layer, &[(0, 0, w as i32, h)]);
    }

    /// Classify which surface a pointer `Enter` targets.
    pub(crate) fn classify_pointer_surface(
        &self,
        surface: &wayland_client::protocol::wl_surface::WlSurface,
    ) -> PointerSurface {
        if self
            .options_layer
            .as_ref()
            .is_some_and(|l| l.wl_surface() == surface)
        {
            PointerSurface::Options
        } else {
            PointerSurface::Dock
        }
    }

    /// Route a pointer event that belongs to the OPTIONS surface.
    pub(crate) fn options_pointer(&mut self, event: wl_pointer::Event) {
        match event {
            wl_pointer::Event::Enter {
                serial,
                surface_x,
                surface_y,
                ..
            } => {
                self.enter_serial = serial;
                self.cursor_now = None;
                self.options_ptr = Some((surface_x as f32, surface_y as f32));
                self.options_on_motion(surface_y as f32);
            }
            wl_pointer::Event::Motion {
                surface_x,
                surface_y,
                ..
            } => {
                self.options_ptr = Some((surface_x as f32, surface_y as f32));
                self.options_on_motion(surface_y as f32);
            }
            wl_pointer::Event::Leave { .. } => {
                self.options_ptr = None;
                self.pointer_surface = PointerSurface::Dock;
                self.options_reveal_deadline = None;
                if !self.options_hidden {
                    if self.options_hover.take().is_some() {
                        self.draw_options();
                    }
                    // Revealed in fullscreen: conceal again shortly after leave.
                    if self.options_fullscreen {
                        self.schedule_options_hide();
                    }
                }
            }
            wl_pointer::Event::Button { button, state, .. }
                if button == BTN_LEFT
                    && state == WEnum::Value(wl_pointer::ButtonState::Released)
                    && !self.options_hidden =>
            {
                self.options_click();
            }
            _ => {}
        }
    }

    /// Shared Enter/Motion logic: reveal-dwell at the top edge while concealed,
    /// otherwise hover the pills and cancel any pending conceal.
    fn options_on_motion(&mut self, y: f32) {
        if self.options_hidden {
            if y <= REVEAL_PX {
                self.arm_options_reveal();
            } else {
                self.options_reveal_deadline = None;
            }
        } else {
            self.options_hide_deadline = None;
            self.options_update_hover();
        }
        self.options_apply_cursor();
    }

    fn options_update_hover(&mut self) {
        let hover = self.options_ptr.and_then(|p| {
            self.options_pills()
                .iter()
                .find(|pill| pill.rect.contains(p))
                .map(|pill| pill.id)
        });
        if hover != self.options_hover {
            self.options_hover = hover;
            self.draw_options();
        }
    }

    fn options_click(&mut self) {
        match self.options_hover {
            Some(PillId::Close) => {
                if let Some(addr) = self.options_active_addr.clone() {
                    hypr::close_window(&addr);
                }
            }
            Some(PillId::Pseudo) => hypr::pseudo_active(),
            Some(PillId::Float) => hypr::float_active(),
            Some(PillId::Fullscreen) => hypr::fullscreen_active(),
            _ => {}
        }
    }

    fn options_apply_cursor(&mut self) {
        let Some(device) = &self.cursor_device else {
            return;
        };
        let shape = match self.options_hover {
            Some(PillId::Clock) | Some(PillId::Window) | None => Shape::Default,
            Some(_) => Shape::Pointer, // any control circle
        };
        if self.cursor_now != Some(shape) {
            device.set_shape(self.enter_serial, shape);
            self.cursor_now = Some(shape);
        }
    }
}
