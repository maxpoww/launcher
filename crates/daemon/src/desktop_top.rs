//! The desktop's menus, ABOVE the windows (Max, 2026-10-08: "if I see only
//! the corner of a folder behind a floating window, I can right-click it and
//! see the menu on top of the window").
//!
//! The desktop itself is a surface UNDER the windows, so anything drawn on
//! it is covered by them. Its right-click menu and its Properties box are
//! therefore drawn on a second surface of the same size and place (anchored
//! the same way, so a point means the same on both) on the Overlay layer.
//! It paints nothing and takes no input at rest. While a menu or a box is
//! up it takes the pointer over the WHOLE screen: a click anywhere else,
//! on a window too, closes the menu and is nothing more — what a menu does
//! everywhere. Its pointer events go to the desktop's own handler
//! (`App::desktop_pointer`); only the drawing lives here.

use smithay_client_toolkit::shell::wlr_layer::LayerSurface;
use smithay_client_toolkit::shell::WaylandSurface;
use tracing::{error, warn};

use crate::content::Scene;
use crate::desktop_menu::MenuPaint;
use crate::renderer::Renderer;
use crate::App;

/// The top surface and what it needs to draw.
#[derive(Default)]
pub(crate) struct Top {
    pub layer: Option<LayerSurface>,
    pub renderer: Option<Renderer>,
    /// Its logical size (the desktop's own, once both are configured).
    pub size: (u32, u32),
    pub fscale: Option<crate::fractional::SurfaceScale>,
    pub visible: Option<crate::visible::SurfaceVisible>,
    /// One frame callback in flight at a time; a draw asked for meanwhile
    /// waits for it.
    pub frame_pending: bool,
    pub dirty: bool,
    /// When it last drew while a panel was easing in, for dt.
    pub last_frame: Option<std::time::Instant>,
    pub tick_timer: bool,
    /// Whether it is taking the pointer (a menu or a box is up).
    pub(crate) input_on: bool,
    /// Whether its last frame showed a panel (so one more is drawn to
    /// take it off).
    pub(crate) drawn: bool,
}

impl App {
    /// Whether the menus are drawn on the top surface (else, as a way out,
    /// on the desktop itself — under the windows, but there).
    pub(crate) fn desktop_top_ready(&self) -> bool {
        self.desktop_top.layer.is_some() && self.desktop_top.renderer.is_some()
    }

    /// A `configure` for the top surface: learn its size, build or resize
    /// its renderer.
    pub(crate) fn configure_desktop_top(
        &mut self,
        configure: smithay_client_toolkit::shell::wlr_layer::LayerSurfaceConfigure,
    ) {
        let (width, height) = configure.new_size;
        if width == 0 || height == 0 {
            return;
        }
        self.desktop_top.size = (width, height);
        let scale = self.surface_scale(crate::fractional::SurfaceKind::DesktopTop);
        let (pw, ph) = (
            crate::fractional::physical(width, scale),
            crate::fractional::physical(height, scale),
        );
        if let Some(fs) = &self.desktop_top.fscale {
            fs.set_logical_size(width, height);
        }
        if let Some(renderer) = self.desktop_top.renderer.as_mut() {
            renderer.set_scale(scale);
            renderer.resize(pw, ph);
        } else {
            let built = {
                let Some(layer) = self.desktop_top.layer.as_ref() else {
                    return;
                };
                Renderer::new(&self.conn, layer.wl_surface(), pw, ph, scale)
            };
            match built {
                Ok(renderer) => self.desktop_top.renderer = Some(renderer),
                Err(e) => {
                    // Never fatal: the menus fall back to the desktop itself.
                    error!("desktop menu surface: renderer init failed: {e:#}");
                    return;
                }
            }
        }
        self.desktop_top.drawn = true; // draw once, whatever is (not) up
        self.request_desktop_top_draw();
    }

    /// The surface takes the pointer everywhere while a menu or a box is
    /// up, and nowhere otherwise.
    fn sync_desktop_top_input(&mut self) {
        let up = self.desktop.menu.is_some() || self.desktop.props.is_some() || self.desktop.config.is_some();
        if up == self.desktop_top.input_on {
            return;
        }
        let Some(layer) = self.desktop_top.layer.as_ref() else {
            return;
        };
        let (w, h) = self.desktop_top.size;
        let rects: &[(i32, i32, i32, i32)] = if up { &[(0, 0, w as i32, h as i32)] } else { &[] };
        crate::surface::set_input_rects(&self.compositor, layer, rects);
        self.desktop_top.input_on = up;
    }

    /// Draw the top surface now, or once the frame in flight has been
    /// shown — when there is something up, or something to take off.
    pub(crate) fn request_desktop_top_draw(&mut self) {
        if !self.desktop_top_ready() {
            return;
        }
        self.sync_desktop_top_input();
        let up = self.desktop.menu.is_some() || self.desktop.props.is_some() || self.desktop.config.is_some();
        if !up && !self.desktop_top.drawn {
            return;
        }
        if self.desktop_top.frame_pending {
            self.desktop_top.dirty = true;
        } else {
            self.draw_desktop_top();
        }
    }

    /// The panel's paint: the OPTIONS boxes' surface, read where it is.
    pub(crate) fn desktop_panel_paint(&self, rect: crate::content::Rect) -> MenuPaint {
        let (fill, ink) = self.box_surface_at(rect);
        MenuPaint {
            fill,
            ink,
            wash: self.options_hover_wash(),
            radius: crate::clipboard::BOX_RADIUS,
        }
    }

    /// Ease the menu's and the box's entrance by `dt`; whether either still
    /// moves.
    pub(crate) fn desktop_ease_panels(&mut self, dt: f32) -> bool {
        let mut moving = false;
        if let Some(menu) = self.desktop.menu.as_mut() {
            let (t, m) = crate::animation::ease_toward(menu.t, 1.0, dt, 20.0, 0.004);
            menu.t = t;
            moving |= m;
        }
        if let Some(props) = self.desktop.props.as_mut() {
            let (t, m) = crate::animation::ease_toward(props.t, 1.0, dt, 16.0, 0.004);
            props.t = t;
            moving |= m;
        }
        if let Some(config) = self.desktop.config.as_mut() {
            let (t, m) = crate::animation::ease_toward(config.t, 1.0, dt, 16.0, 0.004);
            config.t = t;
            moving |= m;
        }
        moving
    }

    /// The menu and the box that are up, into `scene`.
    pub(crate) fn desktop_push_panels(&self, scene: &mut Scene) {
        if let Some(menu) = self.desktop.menu.as_ref() {
            menu.push(scene, &self.desktop_panel_paint(menu.rect));
        }
        if let Some(props) = self.desktop.props.as_ref() {
            props.push(scene, &self.desktop_panel_paint(props.rect));
        }
        if let Some(config) = self.desktop.config.as_ref() {
            config.push(scene, &self.desktop_panel_paint(config.rect));
        }
    }

    /// Draw one frame of the top surface.
    pub(crate) fn draw_desktop_top(&mut self) {
        let (w, h) = self.desktop_top.size;
        if w == 0 || h == 0 {
            return;
        }
        self.desktop_top.dirty = false;
        let now = std::time::Instant::now();
        let dt = self
            .desktop_top
            .last_frame
            .map(|l| now.duration_since(l).as_secs_f32().min(0.1))
            .unwrap_or(0.0);
        let moving = self.desktop_ease_panels(dt);
        self.desktop_top.last_frame = moving.then_some(now);
        let mut scene = Scene {
            alpha: 1.0,
            ..Default::default()
        };
        self.desktop_push_panels(&mut scene);
        self.desktop_top.drawn = !scene.grids.is_empty();
        let squircle = self.config.theme.icon_squircle;
        let top = &mut self.desktop_top;
        let Some(renderer) = top.renderer.as_mut() else {
            return;
        };
        let (layer, qh, pending) = (top.layer.as_ref(), &self.qh, &mut top.frame_pending);
        let mut presented = false;
        match renderer.render(
            &scene,
            [1.0, 1.0, 1.0, 1.0],
            None,
            squircle,
            u32::MAX,
            top.visible.as_mut(),
            &mut || {
                if let (Some(layer), false) = (layer, *pending) {
                    let surface = layer.wl_surface();
                    surface.frame(qh, surface.clone());
                    *pending = true;
                }
            },
        ) {
            Ok(crate::renderer::Frame::Presented) => presented = true,
            Ok(crate::renderer::Frame::Unchanged) => {}
            Err(e) => warn!("desktop menu surface: render failed: {e:#}"),
        }
        if moving {
            self.desktop_top.dirty = true;
            if !presented && !self.desktop_top.frame_pending && !self.desktop_top.tick_timer {
                // An easing frame that presented nothing gets no callback.
                let timer = calloop::timer::Timer::from_duration(std::time::Duration::from_millis(8));
                let armed = self
                    .loop_handle
                    .insert_source(timer, |_, _, app: &mut App| {
                        app.desktop_top.tick_timer = false;
                        app.draw_desktop_top();
                        calloop::timer::TimeoutAction::Drop
                    })
                    .is_ok();
                self.desktop_top.tick_timer = armed;
            }
        }
    }
}
