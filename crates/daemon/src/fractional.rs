//! Fractional scaling: draw each surface at the output's REAL scale.
//!
//! Without this every surface drew at a fixed integer supersample
//! (`render_scale`, 2) and the compositor resampled it to the output. On a
//! 1.6 panel that is 4.0 pixels per logical pixel where 2.56 are shown, 56%
//! more fragment work for a slightly softer result; on a 1.0 panel it is 4×
//! the pixels. Measured on the Lenovo (Intel Alder Lake iGPU, 3200×2000 at
//! 165 Hz, 2026-09-29): the GPU sat at 85% busy through a box open/close;
//! drawing at 1× dropped it to 65%.
//!
//! With `wp_fractional_scale_v1` the compositor tells each surface its
//! preferred scale (in 120ths), and with `wp_viewporter` a buffer of exactly
//! `round(logical × scale)` pixels is shown at the logical size: the pixels we
//! draw are the pixels on screen. Both protocols are optional; without them
//! the integer `render_scale` path stays exactly as it was.

use wayland_client::protocol::wl_surface::WlSurface;
use wayland_client::globals::GlobalList;
use wayland_client::{Connection, Dispatch, QueueHandle};
use wayland_protocols::wp::fractional_scale::v1::client::wp_fractional_scale_manager_v1::{
    self, WpFractionalScaleManagerV1,
};
use wayland_protocols::wp::fractional_scale::v1::client::wp_fractional_scale_v1::{
    self, WpFractionalScaleV1,
};
use wayland_protocols::wp::viewporter::client::wp_viewport::{self, WpViewport};
use wayland_protocols::wp::viewporter::client::wp_viewporter::{self, WpViewporter};

use crate::App;

/// Which of our surfaces a scale belongs to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SurfaceKind {
    Dock,
    Options,
    Deck,
    Desktop,
    /// The desktop's menus, above the windows (`desktop_top.rs`).
    DesktopTop,
    Card,
}

/// The two globals, bound only when the compositor offers both.
pub struct Fractional {
    manager: WpFractionalScaleManagerV1,
    viewporter: WpViewporter,
}

impl Fractional {
    pub fn bind(globals: &GlobalList, qh: &QueueHandle<App>) -> Option<Self> {
        let manager = globals
            .bind::<WpFractionalScaleManagerV1, App, _>(qh, 1..=1, ())
            .ok()?;
        let viewporter = globals.bind::<WpViewporter, App, _>(qh, 1..=1, ()).ok()?;
        Some(Self { manager, viewporter })
    }

    /// Put `surface` on the fractional path. Its buffer scale becomes 1: the
    /// viewport, not the buffer scale, maps the buffer onto the logical size.
    /// Double-buffered state, so this holds from the first buffer attached.
    pub fn attach(&self, surface: &WlSurface, kind: SurfaceKind, qh: &QueueHandle<App>) -> SurfaceScale {
        surface.set_buffer_scale(1);
        SurfaceScale {
            _fractional: self.manager.get_fractional_scale(surface, qh, kind),
            viewport: self.viewporter.get_viewport(surface, qh, ()),
            scale120: None,
        }
    }
}

/// One surface's fractional state.
pub struct SurfaceScale {
    _fractional: WpFractionalScaleV1,
    viewport: WpViewport,
    /// The compositor's preferred scale in 120ths; `None` until it says.
    scale120: Option<u32>,
}

impl SurfaceScale {
    /// The scale to draw at: the compositor's, or `fallback` until it tells us.
    pub fn scale_or(&self, fallback: f32) -> f32 {
        self.scale120.map_or(fallback, |s| s as f32 / 120.0)
    }

    /// Show the buffer at `logical` size, whatever its pixel size.
    pub fn set_logical_size(&self, width: u32, height: u32) {
        self.viewport.set_destination(width as i32, height as i32);
    }
}

/// Physical pixels for a logical length at `scale` (never 0).
pub fn physical(logical: u32, scale: f32) -> u32 {
    ((logical as f32 * scale).round() as u32).max(1)
}

impl Dispatch<WpFractionalScaleV1, SurfaceKind> for App {
    fn event(
        app: &mut Self,
        _: &WpFractionalScaleV1,
        event: wp_fractional_scale_v1::Event,
        kind: &SurfaceKind,
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        if let wp_fractional_scale_v1::Event::PreferredScale { scale } = event {
            let slot = match kind {
                SurfaceKind::Dock => app.dock_fscale.as_mut(),
                SurfaceKind::Options => app.options_fscale.as_mut(),
                SurfaceKind::Deck => app.deck_fscale.as_mut(),
                SurfaceKind::Desktop => app.desktop_fscale.as_mut(),
                SurfaceKind::DesktopTop => app.desktop_top.fscale.as_mut(),
                SurfaceKind::Card => app.card_fscale.as_mut(),
            };
            let Some(slot) = slot else {
                return;
            };
            if slot.scale120 == Some(scale) {
                return;
            }
            slot.scale120 = Some(scale);
            // A change of scale we asked for has landed: the pointer goes
            // back to where it was on the glass (see `display.rs`).
            if matches!(kind, SurfaceKind::Dock) {
                app.display_pointer_back(Some(scale));
            }
            tracing::info!("{kind:?}: output scale {:.3}", scale as f32 / 120.0);
            app.rescale_surface(*kind);
        }
    }
}

// The managers and the viewport emit nothing we act on.
impl Dispatch<WpFractionalScaleManagerV1, ()> for App {
    fn event(
        _: &mut Self,
        _: &WpFractionalScaleManagerV1,
        _: wp_fractional_scale_manager_v1::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
    }
}
impl Dispatch<WpViewporter, ()> for App {
    fn event(
        _: &mut Self,
        _: &WpViewporter,
        _: wp_viewporter::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
    }
}
impl Dispatch<WpViewport, ()> for App {
    fn event(
        _: &mut Self,
        _: &WpViewport,
        _: wp_viewport::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
    }
}
