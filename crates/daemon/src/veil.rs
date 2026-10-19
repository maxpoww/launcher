//! THE VEIL: the Lighting's darkening of the wallpaper, on ONE surface.
//!
//! It used to be drawn twice — a black rect in the desktop layer's scene and
//! another in the bar's, because the desktop stops below the bar's reserved
//! band. At the owner's fractional scale (1.6) the two met mid-pixel and left
//! "a weird border on the banner" (Max, 2026-10-18). One wash on one surface
//! has no seam.
//!
//! `waverunner-veil` sits on the `Bottom` layer, created BEFORE the desktop's
//! so it stacks under the icons, anchored to all four edges with an exclusive
//! zone of -1 so it reaches under the bar's band too. Its content is a 1×1
//! shm pixel of black at the veil's alpha, stretched over the output by
//! `wp_viewporter`: no GPU scene, no per-frame work, and at 0 nothing is
//! attached at all, so the compositor skips the surface. Input is empty.

use tracing::warn;
use smithay_client_toolkit::compositor::{CompositorState, Region};
use smithay_client_toolkit::shell::wlr_layer::{Anchor, KeyboardInteractivity, Layer, LayerShell, LayerSurface, LayerSurfaceConfigure};
use smithay_client_toolkit::shell::WaylandSurface;
use smithay_client_toolkit::shm::raw::RawPool;
use wayland_client::protocol::wl_shm;
use wayland_client::QueueHandle;
use wayland_protocols::wp::viewporter::client::wp_viewport::WpViewport;
use wayland_protocols::wp::viewporter::client::wp_viewporter::WpViewporter;

use crate::App;

/// The veil's surface and what it needs to be kept in step with the output.
pub(crate) struct VeilLayer {
    layer: LayerSurface,
    viewport: WpViewport,
    /// The output's logical size from the last configure (none before it).
    size: Option<(u32, u32)>,
}

/// The pixel for `alpha` (0..1) as `wl_shm` ARGB8888 wants it on a
/// little-endian machine (bytes B, G, R, A), premultiplied: black stays black
/// and only the alpha carries.
pub(crate) fn pixel(alpha: f32) -> [u8; 4] {
    [0, 0, 0, (alpha.clamp(0.0, 1.0) * 255.0).round() as u8]
}

/// Whether anything is attached for `alpha` — at 0 the surface is unmapped.
pub(crate) fn shown(alpha: f32) -> bool {
    pixel(alpha)[3] > 0
}

/// Create and commit the veil's surface (nothing attached until a configure).
pub(crate) fn create(
    compositor: &CompositorState,
    layer_shell: &LayerShell,
    qh: &QueueHandle<App>,
    viewporter: &WpViewporter,
) -> VeilLayer {
    let surface = compositor.create_surface(qh);
    let viewport = viewporter.get_viewport(&surface, qh, ());
    let layer = layer_shell.create_layer_surface(qh, surface, Layer::Bottom, Some("waverunner-veil"), None);
    layer.set_anchor(Anchor::TOP | Anchor::BOTTOM | Anchor::LEFT | Anchor::RIGHT);
    layer.set_size(0, 0);
    layer.set_exclusive_zone(-1);
    layer.set_keyboard_interactivity(KeyboardInteractivity::None);
    if let Ok(region) = Region::new(compositor) {
        layer.wl_surface().set_input_region(Some(region.wl_region()));
    }
    layer.commit();
    VeilLayer { layer, viewport, size: None }
}

impl App {
    /// Whether `surface` is the veil's.
    pub(crate) fn is_veil_surface(&self, surface: &wayland_client::protocol::wl_surface::WlSurface) -> bool {
        self.veil_layer.as_ref().is_some_and(|v| v.layer.wl_surface() == surface)
    }

    /// The veil's surface was configured: remember the size and show it.
    pub(crate) fn configure_veil(&mut self, configure: LayerSurfaceConfigure) {
        if let Some(v) = self.veil_layer.as_mut() {
            if configure.new_size.0 > 0 && configure.new_size.1 > 0 {
                v.size = Some(configure.new_size);
            }
        }
        self.veil_sync();
    }

    /// The veil's output went: the shell carries on without it.
    pub(crate) fn veil_closed(&mut self) {
        warn!("veil surface closed by the compositor; the background is not darkened");
        self.veil_layer = None;
    }

    /// Put the veil now in use: a new pixel for its alpha, or nothing at 0.
    /// Cheap — called when the veil moves (a fade is ~30 steps), never per frame.
    pub(crate) fn veil_sync(&mut self) {
        let alpha = crate::lighting::veil_now();
        let (Some(v), Some(shm)) = (self.veil_layer.as_ref(), self.shm.as_ref()) else {
            return;
        };
        let Some((w, h)) = v.size else {
            return;
        };
        let surface = v.layer.wl_surface();
        v.viewport.set_destination(w as i32, h as i32);
        if !shown(alpha) {
            surface.attach(None, 0, 0);
            surface.commit();
            return;
        }
        let mut pool = match RawPool::new(4, shm) {
            Ok(pool) => pool,
            Err(e) => {
                warn!("veil: no shm pool ({e})");
                return;
            }
        };
        pool.mmap()[..4].copy_from_slice(&pixel(alpha));
        let buffer = pool.create_buffer(0, 1, 1, 4, wl_shm::Format::Argb8888, (), &self.qh);
        surface.attach(Some(&buffer), 0, 0);
        surface.damage_buffer(0, 0, 1, 1);
        surface.commit();
        // (The compositor keeps what it was given: the buffer and pool can go.)
        buffer.destroy();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pixel_is_premultiplied_black_in_argb_little_endian() {
        assert_eq!(pixel(0.0), [0, 0, 0, 0]);
        assert_eq!(pixel(0.45), [0, 0, 0, 115]);
        assert_eq!(pixel(1.0), [0, 0, 0, 255]);
        assert_eq!(pixel(7.0), [0, 0, 0, 255]);
    }

    #[test]
    fn nothing_is_attached_at_zero() {
        assert!(!shown(0.0));
        assert!(!shown(0.001));
        assert!(shown(0.05));
    }
}
