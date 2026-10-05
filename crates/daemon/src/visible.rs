//! Tell the compositor where a surface has anything to show.
//!
//! The shell's surfaces never change size: the dock's holds the whole
//! launcher (83 % of the screen), the OPTIONS bar's is 510 px tall for a 28 px
//! bar so its boxes have room to drop. Most of each is transparent most of
//! the time — and Hyprland cannot know: with the `blur` layer rule it works
//! out the blur for whatever is damaged under the WHOLE surface, and only then
//! throws it away pixel by pixel (`ignore_alpha`). So anything another client
//! drew under them paid for one or two blurs it never showed: on the Acer a
//! terminal scrolling text kept the GPU 42 % busy; with the shell's blur rule
//! off, 11 % (round 3, 2026-10-05).
//!
//! `hyprland_surface_v1.set_visible_region` is the hint for exactly this: the
//! compositor does not draw the surface, nor blur behind it, outside the
//! region; a region that misses the buffer altogether (a hidden dock) takes
//! the surface out of the frame. The renderer knows which tiles a frame drew
//! in (see `crate::damage`), so the region is always the frame's own: it is
//! set, together with the buffer, by the same commit.
//!
//! The protocol is Hyprland's own and optional: without it nothing is sent
//! and everything is drawn as before.

use wayland_client::globals::GlobalList;
use wayland_client::protocol::wl_compositor::WlCompositor;
use wayland_client::protocol::wl_region::WlRegion;
use wayland_client::protocol::wl_surface::WlSurface;
use wayland_client::{Connection, Dispatch, QueueHandle};

use self::proto::hyprland_surface_manager_v1::{self, HyprlandSurfaceManagerV1};
use self::proto::hyprland_surface_v1::{self, HyprlandSurfaceV1};
use crate::damage::Rect;
use crate::App;

/// `hyprland-surface-v1`, generated from the protocol's XML.
#[allow(
    clippy::all,
    dead_code,
    missing_docs,
    non_camel_case_types,
    non_snake_case,
    non_upper_case_globals,
    unused_imports
)]
pub mod proto {
    use wayland_client;
    use wayland_client::protocol::*;

    pub mod __interfaces {
        use wayland_client::protocol::__interfaces::*;
        wayland_scanner::generate_interfaces!("protocols/hyprland-surface-v1.xml");
    }
    use self::__interfaces::*;

    wayland_scanner::generate_client_code!("protocols/hyprland-surface-v1.xml");
}

/// The global, bound when the compositor has it (version 2: the visible
/// region).
pub(crate) struct VisibleRegions {
    manager: HyprlandSurfaceManagerV1,
    compositor: WlCompositor,
}

impl VisibleRegions {
    pub(crate) fn bind(
        globals: &GlobalList,
        compositor: &WlCompositor,
        qh: &QueueHandle<App>,
    ) -> Option<Self> {
        if std::env::var_os("WAVERUNNER_NO_VISIBLE_REGION").is_some() {
            return None;
        }
        let manager = globals
            .bind::<HyprlandSurfaceManagerV1, App, _>(qh, 2..=2, ())
            .ok()?;
        Some(Self {
            manager,
            compositor: compositor.clone(),
        })
    }

    pub(crate) fn attach(&self, surface: &WlSurface, qh: &QueueHandle<App>) -> SurfaceVisible {
        SurfaceVisible {
            hypr: self.manager.get_hyprland_surface(surface, qh, ()),
            compositor: self.compositor.clone(),
            qh: qh.clone(),
            sent: None,
        }
    }
}

/// One surface's visible region.
pub(crate) struct SurfaceVisible {
    hypr: HyprlandSurfaceV1,
    compositor: WlCompositor,
    qh: QueueHandle<App>,
    /// The region last set (`None`: never — the compositor draws all of it).
    sent: Option<Vec<Rect>>,
}

impl SurfaceVisible {
    /// The frame about to be committed draws inside `rects` and nowhere else
    /// (none: it draws nothing). Takes effect with that commit.
    pub(crate) fn set(&mut self, rects: &[Rect]) {
        if self.sent.as_deref() == Some(rects) {
            return;
        }
        let region = self.compositor.create_region(&self.qh, ());
        if rects.is_empty() {
            // An EMPTY region means "no hint"; one that misses the buffer
            // means "nothing to draw".
            region.add(-16, -16, 1, 1);
        }
        for r in rects {
            region.add(r[0], r[1], r[2], r[3]);
        }
        self.hypr.set_visible_region(Some(&region));
        region.destroy();
        self.sent = Some(rects.to_vec());
        crate::perf::VISIBLE_SET.hit();
    }

    /// What was last set, for the damage check.
    pub(crate) fn sent(&self) -> Option<&[Rect]> {
        self.sent.as_deref()
    }
}

// Neither object says anything back.
impl Dispatch<HyprlandSurfaceManagerV1, ()> for App {
    fn event(
        _: &mut Self,
        _: &HyprlandSurfaceManagerV1,
        _: hyprland_surface_manager_v1::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
    }
}
impl Dispatch<HyprlandSurfaceV1, ()> for App {
    fn event(
        _: &mut Self,
        _: &HyprlandSurfaceV1,
        _: hyprland_surface_v1::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
    }
}
wayland_client::delegate_noop!(App: WlRegion);
