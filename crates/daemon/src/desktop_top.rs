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
    /// QUICK LOOK draws here too (`quicklook.rs`): its icon array is
    /// allocated, the picture whose tiles are in it, the soft picture and
    /// badge are written.
    pub(crate) peek_layers: bool,
    pub(crate) peek_tiles: Option<(std::path::PathBuf, u32)>,
    pub(crate) peek_soft: bool,
    /// Which drawing of the pencil's picture is in the layers.
    pub(crate) peek_edit_rev: u64,
    /// Which sharp part of a zoomed photo is in the layers.
    pub(crate) peek_detail_rev: u64,
    /// THE MENUS' SYMBOLS (`glyphs.rs`, their pixels in `App::symbols`, one
    /// copy for every surface): which layer holds which — the first layers,
    /// lent while a menu is up (Quick Look, which uses them too, is never up
    /// with a menu: each takes them back).
    pub(crate) glyph_layers: Vec<String>,
    /// Every symbol asked for once, ahead of the first menu.
    pub(crate) glyphs_warm: bool,
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
                crate::perf::mark("top renderer");
                Renderer::new(&self.conn, layer.wl_surface(), pw, ph, scale)
            };
            match built {
                Ok(mut renderer) => {
                    // (Menus, Properties and the phone's box hand their words
                    // over linear, as their grounds: `Renderer::labels_linear`.)
                    renderer.labels_linear = true;
                    self.desktop_top.renderer = Some(renderer);
                }
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
        let up = self.desktop.menu.is_some() || self.desktop.props.is_some() || self.desktop_config_up() || self.peek.is_up();
        // (The keyboard taken only for a menu goes back with it.)
        if !up && std::mem::take(&mut self.desktop.keys_for_menu) {
            self.desktop_drop_keys();
        }
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
        tracing::debug!("desktop top: takes the pointer: {up}");
    }

    /// Draw the top surface now, or once the frame in flight has been
    /// shown — when there is something up, or something to take off.
    pub(crate) fn request_desktop_top_draw(&mut self) {
        let _perf = crate::perf::DESKTOP_TOP.time();
        if !self.desktop_top_ready() {
            return;
        }
        self.sync_desktop_top_input();
        let up = self.desktop.menu.is_some() || self.desktop.props.is_some() || self.desktop_config_up() || self.peek.is_up();
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
        // The menus are the SHELL's (Max, 2026-10-17): its slider's colours
        // and its Dark/Light when it wears them, else the screen's.
        let (fill, ink) = crate::palette::shell().unwrap_or_else(|| self.box_surface_at(rect));
        MenuPaint {
            fill,
            ink,
            wash: self.options_hover_wash(),
            radius: crate::clipboard::BOX_RADIUS,
            // (Flat, as every menu: the dark look's night light sank their
            // lower rows toward black.)
            glass: crate::content::UNLIT,
            accent: MenuPaint::accent_now(),
        }
    }

    /// The desktop's own panels (its menu, the Properties box, a phone's
    /// Configure box): the panel's paint through the Shell's Transparency.
    /// (Files' menus are its window's, not the Shell's: they keep
    /// [`Self::desktop_panel_paint`].)
    pub(crate) fn shell_panel_paint(&self, rect: crate::content::Rect) -> MenuPaint {
        let mut paint = self.desktop_panel_paint(rect);
        paint.fill[3] = crate::options::shell_glass(paint.fill[3]);
        paint
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
            let layers = &self.desktop_top.glyph_layers;
            // (A symbol by its name; an app's icon by its own key.)
            let glyph = |name: &str| {
                let key = if name.contains(':') { name.to_owned() } else { crate::glyphs::key(name) };
                layers.iter().position(|k| *k == key).map(|l| l as u32)
            };
            menu.push(scene, &self.shell_panel_paint(menu.rect), &glyph);
        }
        if let Some(props) = self.desktop.props.as_ref() {
            props.push(scene, &self.shell_panel_paint(props.rect));
        }
        if let Some(config) = self.desktop.config.as_ref().filter(|c| !c.hosted) {
            config.push(scene, &self.shell_panel_paint(config.rect));
        }
    }

    /// The symbols of the menu that is up into the first layers: written
    /// where their pixels are in hand, asked of the desktop's resolver where
    /// not (`desktop_on_glyph` draws again when they come).
    fn top_glyphs_sync(&mut self) {
        let Some(menu) = self.desktop.menu.as_ref() else {
            return;
        };
        let mut asked = Vec::new();
        // (Its symbols, and the apps' own icons of its Another app page.)
        let keys: Vec<String> = menu.glyphs().into_iter().map(crate::glyphs::key).chain(menu.pictures()).collect();
        for key in keys {
            if self.desktop_top.glyph_layers.contains(&key) {
                continue;
            }
            let top = &mut self.desktop_top;
            let (Some(chain), Some(renderer)) = (self.symbols.get(&key), top.renderer.as_mut()) else {
                if self.symbols_asked.insert(key.clone()) {
                    asked.push(key);
                }
                continue;
            };
            if !top.peek_layers {
                renderer.alloc_icon_array_upto(crate::quicklook::LAYERS);
                top.peek_layers = true;
            }
            renderer.update_icon_layer(top.glyph_layers.len() as u32, chain);
            top.glyph_layers.push(key);
            // (Quick Look's pictures there are gone: written again when it is up.)
            top.peek_tiles = None;
            top.peek_soft = false;
            top.peek_edit_rev = u64::MAX;
            top.peek_detail_rev = u64::MAX;
        }
        if let Some(icons) = self.desktop_icons.as_ref() {
            for key in asked {
                if let Some(req) = crate::desktop::icon_request(&key) {
                    icons.request(req);
                }
            }
        }
    }

    /// Every symbol asked of the desktop's resolver once, ahead of the
    /// first menu: one asked for only as its menu opens comes a few frames
    /// late, an empty gap where it belongs.
    pub(crate) fn top_glyphs_warm(&mut self) {
        let Some(icons) = self.desktop_icons.as_ref() else {
            return;
        };
        if std::mem::replace(&mut self.desktop_top.glyphs_warm, true) {
            return;
        }
        crate::glyphs::write_all();
        for (name, _) in crate::glyphs::GLYPHS {
            let key = crate::glyphs::key(name);
            if self.symbols.contains_key(&key) || !self.symbols_asked.insert(key.clone()) {
                continue;
            }
            if let Some(req) = crate::desktop::icon_request(&key) {
                icons.request(req);
            }
        }
    }

    /// The resolver answered for a symbol of the menus: kept, and drawn.
    pub(crate) fn desktop_on_glyph(&mut self, key: String, chain: Option<Vec<u8>>) {
        self.symbols_asked.remove(&key);
        match chain {
            Some(chain) => {
                self.symbols.insert(key, crate::apps::symbol_chain(chain));
                self.request_desktop_top_draw();
            }
            None => warn!("desktop: no picture for the symbol {key}"),
        }
    }

    /// Draw one frame of the top surface.
    pub(crate) fn draw_desktop_top(&mut self) {
        let (w, h) = self.desktop_top.size;
        if w == 0 || h == 0 || self.part_lost(crate::shell_screens::Part::DesktopTop) {
            return;
        }
        self.desktop_top.dirty = false;
        let now = std::time::Instant::now();
        let dt = self
            .desktop_top
            .last_frame
            .map(|l| now.duration_since(l).as_secs_f32().min(0.1))
            .unwrap_or(0.0);
        let moving = self.desktop_ease_panels(dt) | self.peek_ease(dt);
        // (Quick Look may have just gone back into its icon: the pointer goes
        // back to the windows with it — else every click stayed the desktop's.)
        self.sync_desktop_top_input();
        self.desktop_top.last_frame = moving.then_some(now);
        // (The menus are the Shell's: its Transparency. Quick Look shows what
        // was asked for whole.)
        let mut scene = Scene {
            alpha: if self.peek.is_up() { 1.0 } else { crate::options::shell_content() },
            ..Default::default()
        };
        // (Quick Look and a menu are never up together: each has the first
        // layers while it is.)
        if self.peek.is_up() {
            self.desktop_top.glyph_layers.clear();
        } else {
            self.top_glyphs_sync();
        }
        self.desktop_push_panels(&mut scene);
        // (Quick Look over the menus: they are not up together.)
        self.peek_upload();
        self.peek_push(&mut scene);
        self.desktop_top.drawn = !scene.grids.is_empty();
        // (Quick Look's layers are pictures: no squircle on them.)
        let thumb_base = if self.peek.is_up() { 0 } else { u32::MAX };
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
            thumb_base,
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
