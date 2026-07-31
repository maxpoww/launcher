//! Layer-shell surface setup.
//!
//! Design decision #2: the surface is created once, at full popup size,
//! anchored to the bottom edge, and never resized per frame. All motion
//! happens in what we *draw* inside it.

use smithay_client_toolkit::compositor::{CompositorState, Region};
use smithay_client_toolkit::shell::wlr_layer::{
    Anchor, KeyboardInteractivity, Layer, LayerShell, LayerSurface,
};
use smithay_client_toolkit::shell::WaylandSurface;
use wayland_client::QueueHandle;

use crate::App;

/// Create and commit the (initially input-less) layer surface.
///
/// The first `configure` event from the compositor tells us the final
/// buffer size; only then is the wgpu renderer created.
pub fn create_layer_surface(
    compositor: &CompositorState,
    layer_shell: &LayerShell,
    qh: &QueueHandle<App>,
    width: u32,
    height: u32,
    render_scale: u32,
) -> LayerSurface {
    let surface = compositor.create_surface(qh);
    let layer = layer_shell.create_layer_surface(
        qh,
        surface,
        Layer::Top,
        Some("waverunner"),
        None, // let the compositor pick the active output
    );

    layer.set_anchor(Anchor::BOTTOM);
    layer.set_size(width, height);
    // Integer supersampling: the buffer we attach is `logical × render_scale`
    // physical pixels, so declare the scale up front — it must be active
    // before the renderer commits its first (`logical × render_scale`)
    // buffer, or the compositor would misread the buffer size. Surface-local
    // coordinates (size, input region) stay logical regardless.
    layer.wl_surface().set_buffer_scale(render_scale as i32);
    // Hidden at startup: never steal focus until shown.
    layer.set_keyboard_interactivity(KeyboardInteractivity::None);
    layer.commit();
    layer
}

/// Create and commit the OPTIONS topbar surface: a strip anchored to the
/// top edge, spanning the full output width, reserving its own exclusive
/// zone so windows and the dock lay out beneath it.
///
/// The surface is `surface_height` tall but only reserves `bar_height` — the
/// extra sliver overhangs the window's top edge (transparent, click-through)
/// so, while colour-matched, the bar can paint over the window's top border
/// and hide the seam between them.
///
/// Like the dock, the renderer is built on the first `configure` — anchoring
/// left+right with a `0` width lets the compositor stretch the surface to the
/// output width and report it back. The input region is empty: the strip is
/// pointer-inert (currently just a near-transparent surface, no content).
pub fn create_top_surface(
    compositor: &CompositorState,
    layer_shell: &LayerShell,
    qh: &QueueHandle<App>,
    bar_height: u32,
    surface_height: u32,
    render_scale: u32,
) -> LayerSurface {
    let surface = compositor.create_surface(qh);
    let layer = layer_shell.create_layer_surface(
        qh,
        surface,
        Layer::Top,
        Some("waverunner-options"),
        None, // let the compositor pick the active output
    );

    // Anchoring top + left + right with width 0 stretches to the output width;
    // the real size arrives in the first configure.
    layer.set_anchor(Anchor::TOP | Anchor::LEFT | Anchor::RIGHT);
    layer.set_size(0, surface_height);
    // Reserve only the bar strip; the overhang below it is not reserved.
    layer.set_exclusive_zone(bar_height as i32);
    layer.wl_surface().set_buffer_scale(render_scale as i32);
    layer.set_keyboard_interactivity(KeyboardInteractivity::None);
    // Empty input region: pointer events pass through, never reaching the
    // dock's pointer handlers.
    if let Ok(region) = Region::new(compositor) {
        layer.wl_surface().set_input_region(Some(region.wl_region()));
    }
    layer.commit();
    layer
}

/// Set a layer surface's pointer input region to the union of `rects`
/// (logical `x, y, w, h`). An empty slice makes the whole surface
/// click-through.
pub fn set_input_rects(
    compositor: &CompositorState,
    layer: &LayerSurface,
    rects: &[(i32, i32, i32, i32)],
) {
    let Ok(region) = Region::new(compositor) else {
        return;
    };
    for &(x, y, w, h) in rects {
        region.add(x, y, w, h);
    }
    layer
        .wl_surface()
        .set_input_region(Some(region.wl_region()));
    layer.commit();
}

/// Flip keyboard interactivity as the popup expands/collapses.
///
/// `Exclusive` while fully open: the popup only opens by deliberate
/// gesture, and grabbing the keyboard is what makes type-to-search
/// work without a click (the P4 focus-model decision). Losing focus
/// (compositor override, alt-tab on some setups) still collapses via
/// the keyboard-leave handler. `None` while docked or hidden: the dock
/// is pointer-only and must never steal keys.
pub fn set_interactive(layer: &LayerSurface, interactive: bool) {
    layer.set_keyboard_interactivity(if interactive {
        KeyboardInteractivity::Exclusive
    } else {
        KeyboardInteractivity::None
    });
    layer.commit();
}

/// Restrict pointer input to the bottom `extent` pixels of the surface.
///
/// The surface is full popup size at all times (design decision #2), so
/// without this the transparent area above the dock would swallow clicks
/// meant for windows behind it. `extent == 0` makes the whole surface
/// click-through.
pub fn set_input_extent(
    compositor: &CompositorState,
    layer: &LayerSurface,
    (surface_w, surface_h): (u32, u32),
    extent: u32,
    inset_x: u32,
) -> anyhow::Result<()> {
    let region =
        Region::new(compositor).map_err(|e| anyhow::anyhow!("cannot create input region: {e}"))?;
    if extent > 0 {
        let extent = extent.min(surface_h);
        // Inset horizontally to the card's bounds: the transparent
        // drag margin on the sides stays click-through to windows
        // behind it.
        let inset_x = inset_x.min(surface_w / 2);
        region.add(
            inset_x as i32,
            (surface_h - extent) as i32,
            (surface_w - 2 * inset_x) as i32,
            extent as i32,
        );
    }
    layer
        .wl_surface()
        .set_input_region(Some(region.wl_region()));
    layer.commit();
    Ok(())
}
