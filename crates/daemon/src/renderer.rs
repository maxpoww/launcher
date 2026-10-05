//! wgpu rendering onto the layer-shell surface.
//!
//! Draws the scene assembled by [`crate::content`]: instanced rounded
//! rectangles (card background, hover highlights) via an SDF shader,
//! app icons as instanced quads over one `ICON_SIZE`² texture array,
//! and app names via glyphon. Grid content is clipped with a scissor
//! rect; everything below the surface edge is clipped by the
//! framebuffer. Text and icons fade with the card's animation alpha.

use std::ptr::NonNull;

use anyhow::{anyhow, Context};
use glyphon::{
    Attrs, Buffer as TextBuffer, Cache as TextCache, Family, FontSystem, Metrics, Resolution,
    Shaping, SwashCache, TextArea, TextAtlas, TextBounds, TextRenderer, Viewport, Weight,
};
use raw_window_handle::{
    RawDisplayHandle, RawWindowHandle, WaylandDisplayHandle, WaylandWindowHandle,
};
use wayland_client::protocol::wl_surface::WlSurface;
use wayland_client::{Connection, Proxy};
use wgpu::util::DeviceExt;

use crate::apps::{ICON_CHAIN_BYTES, ICON_MIPS, ICON_SIZE};
use crate::content::Scene;

/// The largest texture side the device is asked for — and so the largest
/// framebuffer a surface may have (`App::surface_scale` caps the scale to it).
pub const MAX_TEXTURE_SIDE: u32 = 8192;

/// Global uniforms shared by the rect and icon pipelines.
#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
struct Globals {
    screen: [f32; 2],
    alpha: f32,
    time: f32,
    cursor: [f32; 2], // pointer in surface pixels; [-9999,-9999] = absent
    squircle: f32,    // icon corner superellipse exponent (icon.wgsl only; 0 = off)
    thumb_base: f32,  // first thumbnail texture layer (icon.wgsl; ≥ it skips squircle)
    // Banner blister: (bar_edge_y, k, _, _); the sunset module's banner rect
    // smooth-unions with the half-plane above bar_edge_y. x < -9000 = off.
    neck: [f32; 4],
}

/// Per-instance data for one rounded rectangle.
#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
struct RectInstance {
    rect_min: [f32; 2],
    rect_max: [f32; 2],
    color: [f32; 4],
    radius: f32,
    glass: f32,  // 0 = solid fill, 1 = liquid-glass material
    border: f32, // 0 = filled; >0 = stroke width just inside the edge
    _pad: f32,
}

/// Per-instance data for one top-edge shadow band.
#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
struct ShadowInstance {
    rect_min: [f32; 2],
    rect_max: [f32; 2],
    color: [f32; 4],
    radius: f32,
    blur: f32,
    edges: [f32; 4],
}

/// Per-instance data for one icon quad.
#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
struct IconInstance {
    rect_min: [f32; 2],
    rect_max: [f32; 2],
    layer: u32,
    // Silhouette tint: rgb colour + strength in a (0 = untinted). Packed
    // contiguously (offset 20) so `vertex_attr_array` lays it out with no
    // padding.
    tint: [f32; 4],
    // Progress-ring mode: >=0 draws a circular install ring that filled, <0
    // draws the icon. Offset 36.
    ring: f32,
    // Squircle plate drawn under the glyph in-shader (rgba; a <= 0 = none).
    // Offset 40 → the struct is exactly 56 bytes, Pod-clean.
    plate: [f32; 4],
}

/// Per-instance data for the open box's frosted backdrop quad.
#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
struct BoxBackdropInstance {
    rect_min: [f32; 2],
    rect_max: [f32; 2],
    radius: f32,
    screen: [f32; 2],
    _pad: f32,
}

/// The GPU the whole shell draws on: one wgpu instance, adapter, device and
/// queue, opened by the first [`Renderer`] and cloned (they are handles) by
/// every later one.
///
/// Each renderer used to open its OWN device. On the Acer (Intel HD 5500,
/// hasvk) the kernel's per-client accounting showed what that cost: 311 +
/// 223 + 144 MB of resident GPU memory for dock, OPTIONS bar and deck — the
/// deck's 144 MB for a strip that was not even on screen is all device
/// overhead — on a machine with 3.8 GB (night audit, 2026-10-04). It also
/// meant three walks of the adapter ladder at every start.
#[derive(Clone)]
struct SharedGpu {
    instance: wgpu::Instance,
    adapter: wgpu::Adapter,
    device: wgpu::Device,
    queue: wgpu::Queue,
}

/// How many measured widths are kept before the table starts over.
const MEASURED_CAP: usize = 2048;

thread_local! {
    /// Text widths already measured (see [`Renderer::measure_text`]), and the
    /// scratch string their keys are built in.
    static MEASURED: std::cell::RefCell<(std::collections::HashMap<String, f32>, String)> =
        std::cell::RefCell::new((std::collections::HashMap::new(), String::new()));
}

/// A label shaped for a frame and kept for the next ones: the OPTIONS bar's
/// labels are all "volatile" (uncached — their text can change at any time),
/// so every one of them was shaped again on every frame, ten per frame
/// through a whole box animation in which none of them changed.
struct VolatileLabel {
    buffer: TextBuffer,
    /// The width it was shaped within (physical px)…
    max_w: f32,
    /// …and whether the whole text fit on one line of `line_w` px: then the
    /// same buffer is right for any bound at least that wide.
    fits: bool,
    line_w: f32,
    /// The last frame that drew it.
    used: u64,
}

/// Frames an unused volatile label is kept for (a blink, a list scrolled back).
const VOLATILE_KEEP: u64 = 90;

type SharedFonts = std::rc::Rc<std::cell::RefCell<FontSystem>>;
type SharedSwash = std::rc::Rc<std::cell::RefCell<SwashCache>>;

thread_local! {
    static SHARED_TEXT: std::cell::RefCell<Option<(SharedFonts, SharedSwash)>> =
        const { std::cell::RefCell::new(None) };
    /// When a surface of the shell last presented a frame.
    static LAST_PRESENT: std::cell::Cell<Option<std::time::Instant>> =
        const { std::cell::Cell::new(None) };
}

/// When the shell itself last changed the screen (any surface's last present).
pub(crate) fn last_present() -> Option<std::time::Instant> {
    LAST_PRESENT.with(std::cell::Cell::get)
}

/// The one font system (and glyph-image cache) every renderer shapes and
/// rasterises with. Each renderer used to build its own: the monospace faces
/// parsed three times at start, and the same strings shaped and rasterised
/// into three separate caches (the panel's pills are drawn by the dock AND
/// by the OPTIONS bar).
fn shared_text() -> (SharedFonts, SharedSwash) {
    SHARED_TEXT.with(|t| {
        t.borrow_mut()
            .get_or_insert_with(|| {
                let fonts = FontSystem::new_with_locale_and_db(
                    crate::font_index::locale(),
                    crate::font_index::database(),
                );
                (
                    std::rc::Rc::new(std::cell::RefCell::new(fonts)),
                    std::rc::Rc::new(std::cell::RefCell::new(SwashCache::new())),
                )
            })
            .clone()
    })
}

/// Everything a renderer draws WITH that does not depend on its surface's
/// size: built once on the shared device for one surface format.
#[derive(Clone)]
struct Pipelines {
    format: wgpu::TextureFormat,
    globals_layout: wgpu::BindGroupLayout,
    shadow_pipeline: wgpu::RenderPipeline,
    rect_pipeline: wgpu::RenderPipeline,
    icon_pipeline: wgpu::RenderPipeline,
    icon_bind_layout: wgpu::BindGroupLayout,
    icon_sampler: wgpu::Sampler,
    blit_pipeline: wgpu::RenderPipeline,
    blit_layout: wgpu::BindGroupLayout,
    blit_sampler: wgpu::Sampler,
    box_backdrop_pipeline: wgpu::RenderPipeline,
    box_erase_pipeline: wgpu::RenderPipeline,
    blur_pipeline_h: wgpu::RenderPipeline,
    blur_pipeline_v: wgpu::RenderPipeline,
}

thread_local! {
    static SHARED_PIPELINES: std::cell::RefCell<Option<Pipelines>> =
        const { std::cell::RefCell::new(None) };
    /// Renderers live on the event loop's thread; so does the GPU they share.
    static SHARED_GPU: std::cell::RefCell<Option<SharedGpu>> =
        const { std::cell::RefCell::new(None) };
}

/// What [`Renderer::render`] did with a frame.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Frame {
    /// Drawn and presented (the present commits the surface).
    Presented,
    /// It shows exactly what the last presented frame does, so nothing was
    /// drawn and the surface was NOT committed: a frame request made for it
    /// still needs a commit to reach the compositor.
    Unchanged,
}

pub struct Renderer {
    surface: wgpu::Surface<'static>,
    device: wgpu::Device,
    queue: wgpu::Queue,
    config: wgpu::SurfaceConfiguration,
    /// Sustained ambient animation must be frame-throttled: true for a
    /// software adapter (llvmpipe — every frame costs real cores). The GL
    /// backend no longer is: its present doesn't block since `NoVblankWait`.
    /// See [`Renderer::needs_frame_throttle`] and the constructor's note
    /// (F12 / Golem #40).
    frame_throttle: bool,
    /// Pace frames by the GPU (see [`Renderer::gpu_ready`]): true on every
    /// hardware adapter (the software one has the fixed throttle instead). `gpu_busy` is set at submit and cleared when the GPU reports
    /// the frame's work done.
    pace_by_gpu: bool,
    gpu_busy: std::sync::Arc<std::sync::atomic::AtomicBool>,
    /// Physical pixels per logical pixel: the output's fractional scale (see
    /// `fractional.rs`), or the integer `render_scale` supersample without
    /// that protocol. `config.width/height` are physical (`logical × scale`);
    /// geometry is authored in logical px and scaled up automatically (see
    /// [`Renderer::render`]).
    scale: f32,

    globals_buf: wgpu::Buffer,
    globals_bind: wgpu::BindGroup,
    shadow_pipeline: wgpu::RenderPipeline,
    rect_pipeline: wgpu::RenderPipeline,

    /// Fullscreen textured-quad pipeline (copies the offscreen scene to the
    /// screen, see [`FrostTargets`]).
    blit_pipeline: wgpu::RenderPipeline,
    blit_layout: wgpu::BindGroupLayout,
    blit_sampler: wgpu::Sampler,
    /// Frosted-glass backdrop for the open box: samples the blurred scene
    /// over the box region (shares `blit_layout`).
    box_backdrop_pipeline: wgpu::RenderPipeline,
    /// Clears the box region to transparent (× (1−coverage)) before the
    /// backdrop fill, so the frost replaces the base instead of stacking.
    box_erase_pipeline: wgpu::RenderPipeline,
    /// The three full-surface textures a frosted box needs (see
    /// [`FrostTargets`]) — only while a box is open. A frame without one is
    /// drawn straight into the swapchain image.
    ///
    /// They used to exist for the life of every renderer, and every frame of
    /// every surface was drawn into the offscreen one and copied across: two
    /// full-surface passes for a hover on the dock, on surfaces (OPTIONS bar,
    /// deck) whose scenes never open a box at all.
    frost: Option<FrostTargets>,
    blur_pipeline_h: wgpu::RenderPipeline,
    blur_pipeline_v: wgpu::RenderPipeline,

    icon_pipeline: wgpu::RenderPipeline,
    icon_bind_layout: wgpu::BindGroupLayout,
    icon_sampler: wgpu::Sampler,
    /// Bind group over the icon texture array; `None` until the indexer
    /// delivers icons.
    icon_bind: Option<wgpu::BindGroup>,
    /// The icon texture array itself, kept for per-layer updates of the
    /// dynamic package icons; layer count includes the reserved tail.
    icon_texture: Option<wgpu::Texture>,
    icon_layer_count: u32,
    /// The most layers the array may grow to (its content + the reserved
    /// tail).
    icon_layer_cap: u32,

    /// The shell's ONE font system and glyph-image cache (see
    /// [`shared_text`]): the same on every renderer.
    font_system: SharedFonts,
    swash: SharedSwash,
    text_viewport: Viewport,
    text_atlas: TextAtlas,
    text_renderer: TextRenderer,
    /// Shaped label buffers, keyed by label text; invalidated when a
    /// new app set arrives via [`Renderer::set_icons`].
    label_cache: std::collections::HashMap<String, TextBuffer>,
    /// Shaped uncached labels, reused while their text stays the same.
    volatile: std::collections::HashMap<String, VolatileLabel>,
    frame_no: u64,
    /// Accumulated render time — only advances while frames are drawn, so
    /// there are no phase jumps when the dock hides and reappears.
    anim_time: f32,
    last_render: Option<std::time::Instant>,

    /// Work out what each frame changed (see [`crate::damage`]): a frame
    /// that changed nothing is not drawn at all. Off with
    /// `WAVERUNNER_FULL_DAMAGE=1` (every frame drawn, presented whole).
    track_damage: bool,
    /// …and say so in the present (the Vulkan and GL backends can pass it
    /// on).
    present_damage: bool,
    /// The tiles of the frame last presented. `None` until one is, and after
    /// anything that makes the next image new as a whole (a resize, a scale
    /// change, a rebuilt swapchain): that present damages everything.
    damage_prev: Option<crate::damage::TileMap>,
    /// The map the next frame is worked out in (the allocation is reused).
    damage_cur: crate::damage::TileMap,
    /// What an icon draw's pixels depend on besides its instance: the array
    /// it samples (`icon_epoch`, bumped when the array is replaced) and the
    /// layer's own content (`icon_layer_gen`, bumped when it is rewritten).
    icon_epoch: u64,
    icon_layer_gen: Vec<u32>,
    /// `WAVERUNNER_DAMAGE_CHECK=1` (see [`DamageCheck`]).
    damage_check: Option<DamageCheck>,
}

/// The damage, verified: with `WAVERUNNER_DAMAGE_CHECK=1` every frame is
/// composed into a texture of its own, read back and compared with the frame
/// before. A pixel that changed outside the frame's damage would have stayed
/// stale on screen — it is logged and counted (`debug-perf`). Slow (a
/// readback per frame); for test runs.
#[derive(Default)]
struct DamageCheck {
    target: Option<CheckTarget>,
    /// The previous frame's pixels, rows tightly packed, and its size.
    prev: Vec<u8>,
    prev_size: (u32, u32),
    /// `WAVERUNNER_DAMAGE_CHECK=paths`: a frame drawn straight into its
    /// target is ALSO drawn the two-step way (offscreen, then copied across,
    /// as every frame was before round 3) into `two_step`, and the two must
    /// come out the same, pixel for pixel.
    paths: bool,
    two_step: Option<(CheckTarget, wgpu::TextureView, wgpu::BindGroup)>,
    /// Whether this frame has a two-step twin to compare.
    twin: bool,
}

struct CheckTarget {
    width: u32,
    height: u32,
    tex: wgpu::Texture,
    view: wgpu::TextureView,
    bind: wgpu::BindGroup,
    buf: wgpu::Buffer,
    /// Bytes per row in `buf` (wgpu pads rows to 256).
    stride: u32,
}

/// What a draw's pixels depend on besides its own instance data — the
/// uniforms its shader reads. Folded into its hash (see [`crate::damage`]).
struct FrameDeps {
    /// Every rect and icon fades with the scene's alpha.
    fade: u64,
    /// The glass material's edge lights move with time and the pointer.
    lights: u64,
    /// The banner blister takes its neck from the globals (when it is on).
    neck: Option<u64>,
    /// Icons: the corner shape, the array and its layers.
    icons: u64,
}

/// The glass material's lights (rounded_rect.wgsl: the iridescent rim within
/// 30 px of the edge, the pointer's edge reflection falling off as
/// exp(-d²/100)) reach this far in from a glass rect's edges, logical px.
/// Beyond it — 0.06·exp(-20) — the material is the instance's alone.
const GLASS_LIGHT_BAND: f32 = 45.0;

const KIND_SHADOW: u64 = 1;
const KIND_RECT: u64 = 2;
const KIND_ICON: u64 = 3;
const KIND_BACKDROP: u64 = 4;
const KIND_LABEL: u64 = 5;

/// A grid's clip as the scissor rectangle it is drawn under (physical px:
/// x, y, width, height), or `None` when nothing of it is on the surface.
fn scissor_of(clip: &crate::content::Rect, scale: f32, w: u32, h: u32) -> Option<[u32; 4]> {
    // Scissor rects address the physical framebuffer; the clip is logical,
    // so scale it up.
    let sx = ((clip.x.max(0.0) * scale) as u32).min(w);
    let sy = ((clip.y.max(0.0) * scale) as u32).min(h);
    let sw = ((clip.w * scale) as u32).min(w - sx);
    let sh = (((clip.y + clip.h) * scale).min(h as f32) as u32).saturating_sub(sy);
    (sw != 0 && sh != 0).then_some([sx, sy, sw, sh])
}

/// A pixel box cut to a scissor rectangle.
fn cut(b: [f32; 4], clip: Option<[u32; 4]>) -> [f32; 4] {
    match clip {
        Some([sx, sy, sw, sh]) => [
            b[0].max(sx as f32),
            b[1].max(sy as f32),
            b[2].min((sx + sw) as f32),
            b[3].min((sy + sh) as f32),
        ],
        None => b,
    }
}

fn clip_hash(h: u64, clip: Option<[u32; 4]>) -> u64 {
    match clip {
        Some(c) => crate::damage::hash_bytes(h, bytemuck::bytes_of(&c)),
        None => h,
    }
}

fn mark_shadow(tiles: &mut crate::damage::TileMap, s: &ShadowInstance, scale: f32) {
    // edge_shadow.wgsl: the quad is the rect grown by `blur + 2`; it reads
    // no uniform but the screen size.
    let m = s.blur + 2.0;
    tiles.mark(
        [
            (s.rect_min[0] - m) * scale,
            (s.rect_min[1] - m) * scale,
            (s.rect_max[0] + m) * scale,
            (s.rect_max[1] + m) * scale,
        ],
        crate::damage::hash_bytes(KIND_SHADOW, bytemuck::bytes_of(s)),
    );
}

fn mark_rect(
    tiles: &mut crate::damage::TileMap,
    r: &RectInstance,
    clip: Option<[u32; 4]>,
    scale: f32,
    deps: &FrameDeps,
) {
    // rounded_rect.wgsl: the quad is the rect grown by 1 px.
    let b = cut(
        [
            (r.rect_min[0] - 1.0) * scale,
            (r.rect_min[1] - 1.0) * scale,
            (r.rect_max[0] + 1.0) * scale,
            (r.rect_max[1] + 1.0) * scale,
        ],
        clip,
    );
    let h = clip_hash(
        crate::damage::hash_bytes(deps.fade ^ KIND_RECT, bytemuck::bytes_of(r)),
        clip,
    );
    // The shader's own three ways, in its order: the blister (needs the
    // neck), a solid fill, the glass material.
    match deps.neck {
        Some(neck) if (r.glass - 2.0).abs() < 0.5 => tiles.mark(b, crate::damage::mix(h, neck)),
        _ if r.glass < 0.5 => tiles.mark(b, h),
        _ => {
            let band = GLASS_LIGHT_BAND;
            let inner = [
                (r.rect_min[0] + band) * scale,
                (r.rect_min[1] + band) * scale,
                (r.rect_max[0] - band) * scale,
                (r.rect_max[1] - band) * scale,
            ];
            tiles.mark_split(b, inner, h, crate::damage::mix(h, deps.lights));
        }
    }
}

fn mark_icon(
    tiles: &mut crate::damage::TileMap,
    i: &IconInstance,
    clip: Option<[u32; 4]>,
    scale: f32,
    deps: &FrameDeps,
    layer_gen: &[u32],
) {
    // icon.wgsl: the quad is the rect itself.
    let b = cut(
        [
            i.rect_min[0] * scale,
            i.rect_min[1] * scale,
            i.rect_max[0] * scale,
            i.rect_max[1] * scale,
        ],
        clip,
    );
    let gen = layer_gen.get(i.layer as usize).copied().unwrap_or(0);
    let h = crate::damage::hash_bytes(deps.icons ^ KIND_ICON, bytemuck::bytes_of(i));
    tiles.mark(b, clip_hash(crate::damage::mix(h, u64::from(gen)), clip));
}

/// What the open box's frosted glass is made from (see [`Renderer::frost`]):
/// the base scene, drawn offscreen so it can be sampled, and the separable
/// Gaussian's ping-pong pair — scene → `a` (horizontal) → `b` (vertical); the
/// box's backdrop samples `b`. Same size and format as the swapchain.
struct FrostTargets {
    scene_view: wgpu::TextureView,
    scene_bind: wgpu::BindGroup,
    a_view: wgpu::TextureView,
    a_bind: wgpu::BindGroup,
    b_view: wgpu::TextureView,
    b_bind: wgpu::BindGroup,
}

/// Begin a pass that clears `view` to transparent.
fn clear_pass<'a>(
    encoder: &'a mut wgpu::CommandEncoder,
    view: &wgpu::TextureView,
    label: &'static str,
) -> wgpu::RenderPass<'a> {
    encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
        label: Some(label),
        color_attachments: &[Some(wgpu::RenderPassColorAttachment {
            view,
            resolve_target: None,
            ops: wgpu::Operations {
                load: wgpu::LoadOp::Clear(wgpu::Color::TRANSPARENT),
                store: wgpu::StoreOp::Store,
            },
        })],
        depth_stencil_attachment: None,
        timestamp_writes: None,
        occlusion_query_set: None,
    })
}

/// Build one full-surface colour target (texture + view + a bind group to
/// sample it with) at `width`×`height`: the offscreen scene, or a blur target.
fn make_scene_target(
    device: &wgpu::Device,
    format: wgpu::TextureFormat,
    width: u32,
    height: u32,
    layout: &wgpu::BindGroupLayout,
    sampler: &wgpu::Sampler,
) -> (wgpu::Texture, wgpu::TextureView, wgpu::BindGroup) {
    let tex = device.create_texture(&wgpu::TextureDescriptor {
        label: Some("waverunner.scene-tex"),
        size: wgpu::Extent3d {
            width: width.max(1),
            height: height.max(1),
            depth_or_array_layers: 1,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format,
        usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::TEXTURE_BINDING,
        view_formats: &[],
    });
    let view = tex.create_view(&wgpu::TextureViewDescriptor::default());
    let bind = device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: Some("waverunner.blit-bind"),
        layout,
        entries: &[
            wgpu::BindGroupEntry {
                binding: 0,
                resource: wgpu::BindingResource::TextureView(&view),
            },
            wgpu::BindGroupEntry {
                binding: 1,
                resource: wgpu::BindingResource::Sampler(sampler),
            },
        ],
    });
    (tex, view, bind)
}

/// The damage check's frame: a colour target like the scene's that can also
/// be copied out, and the buffer it is copied into.
fn make_check_target(
    device: &wgpu::Device,
    format: wgpu::TextureFormat,
    width: u32,
    height: u32,
    layout: &wgpu::BindGroupLayout,
    sampler: &wgpu::Sampler,
) -> CheckTarget {
    let tex = device.create_texture(&wgpu::TextureDescriptor {
        label: Some("waverunner.damage-check"),
        size: wgpu::Extent3d {
            width: width.max(1),
            height: height.max(1),
            depth_or_array_layers: 1,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format,
        usage: wgpu::TextureUsages::RENDER_ATTACHMENT
            | wgpu::TextureUsages::TEXTURE_BINDING
            | wgpu::TextureUsages::COPY_SRC,
        view_formats: &[],
    });
    let view = tex.create_view(&wgpu::TextureViewDescriptor::default());
    let bind = device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: Some("waverunner.damage-check"),
        layout,
        entries: &[
            wgpu::BindGroupEntry {
                binding: 0,
                resource: wgpu::BindingResource::TextureView(&view),
            },
            wgpu::BindGroupEntry {
                binding: 1,
                resource: wgpu::BindingResource::Sampler(sampler),
            },
        ],
    });
    let align = wgpu::COPY_BYTES_PER_ROW_ALIGNMENT;
    let stride = (width.max(1) * 4).div_ceil(align) * align;
    let buf = device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("waverunner.damage-check"),
        size: u64::from(stride) * u64::from(height.max(1)),
        usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
        mapped_at_creation: false,
    });
    CheckTarget {
        width,
        height,
        tex,
        view,
        bind,
        buf,
        stride,
    }
}

impl Renderer {
    /// Create the wgpu device and configure the swapchain against an
    /// already-configured layer surface of `width` x `height` physical
    /// (buffer) pixels. `scale` is physical pixels per logical pixel:
    /// physical = logical × scale (fractional on the viewport path).
    pub fn new(
        conn: &Connection,
        wl_surface: &WlSurface,
        width: u32,
        height: u32,
        scale: f32,
    ) -> anyhow::Result<Self> {
        // Held for the whole setup: the GL path initializes its EGL display
        // in create_surface, not Instance::new. See `NoVblankWait`.
        let _no_vblank_wait = NoVblankWait::arm();
        let display = NonNull::new(conn.backend().display_ptr().cast())
            .ok_or_else(|| anyhow!("null wl_display"))?;
        let window = NonNull::new(wl_surface.id().as_ptr().cast())
            .ok_or_else(|| anyhow!("null wl_surface"))?;

        // A LADDER, not a single attempt (F8): the machine that cannot
        // present through its Vulkan path must still get a shell. Venus
        // (Vulkan passthrough in the VM) gave us a surface with NO adapter,
        // and one failed attempt used to be fatal. Each rung is tried in
        // turn and the reason for the previous one is logged.
        // ONE GPU device for every surface of the shell (see [`SharedGpu`]):
        // the first renderer walks the adapter ladder and opens the device,
        // the others draw on it.
        let reuse = SHARED_GPU.with(|g| g.borrow().clone()).and_then(|g| {
            let surface = unsafe {
                g.instance
                    .create_surface_unsafe(wgpu::SurfaceTargetUnsafe::RawHandle {
                        raw_display_handle: RawDisplayHandle::Wayland(WaylandDisplayHandle::new(
                            display,
                        )),
                        raw_window_handle: RawWindowHandle::Wayland(WaylandWindowHandle::new(
                            window,
                        )),
                    })
            }
            .map_err(|e| tracing::warn!("renderer: no surface on the shared GPU: {e}"))
            .ok()?;
            if !g.adapter.is_surface_supported(&surface) {
                tracing::warn!("renderer: the shared GPU cannot present to this surface");
                return None;
            }
            tracing::info!(
                "renderer: sharing the GPU device ({})",
                g.adapter.get_info().name
            );
            Some((surface, g.adapter, g.device, g.queue))
        });
        let (surface, adapter, device, queue, on_shared_gpu) = if let Some(reuse) = reuse {
            (reuse.0, reuse.1, reuse.2, reuse.3, true)
        } else {
            const ATTEMPTS: [(wgpu::Backends, bool, &str); 3] = [
                // What we want: a real GPU on Vulkan. Vulkan ALONE: asking for
                // GL too made every start also bring up EGL, Mesa's GL driver
                // (libgallium, 60 MB) and two GL contexts on machines that never
                // draw through them — reads a spinning disk pays for at login
                // (ThinkPad, 2026-10-01). LLVM still loads on most machines
                // (RADV and lavapipe link it). A machine without a usable Vulkan
                // GPU reaches GL on the next rung, as before.
                (wgpu::Backends::VULKAN, false, "gpu"),
                // Some stacks present fine on GL while their Vulkan surface path
                // is broken; asking for GL alone changes which one is picked.
                // MUST come before the software fallback: on pre-Skylake Intel
                // (Haswell — "Vulkan support is incomplete") the only Vulkan
                // adapter mesa offers is llvmpipe, so the attempt above yields a
                // CPU rasterizer while a perfectly good REAL GPU sits on the GL
                // path (crocus). Found live on a 2013 MacBook Air: the whole
                // shell was software-rendered (menubox = 80% CPU) until GL was
                // tried before accepting CPU (2026-09-02).
                (wgpu::Backends::GL, false, "gl only"),
                // Anything at all, including lavapipe/llvmpipe on the CPU. Slow
                // (the F12 throttle exists for exactly this) but it is a desktop.
                (
                    wgpu::Backends::from_bits_truncate(
                        wgpu::Backends::VULKAN.bits() | wgpu::Backends::GL.bits(),
                    ),
                    true,
                    "software fallback",
                ),
            ];

            let mut chosen: Option<(wgpu::Surface<'static>, wgpu::Adapter)> = None;
            // The instance must outlive the surface it created; hold the winning
            // one until the device is built below.
            let mut _live_instance: Option<wgpu::Instance> = None;
            for (backends, force_fallback_adapter, label) in ATTEMPTS {
                let instance = wgpu::Instance::new(&wgpu::InstanceDescriptor {
                    backends,
                    ..Default::default()
                });
                // SAFETY: both handles point at live Wayland objects owned by
                // App, which outlives the renderer and drops it first.
                let surface = match unsafe {
                    instance.create_surface_unsafe(wgpu::SurfaceTargetUnsafe::RawHandle {
                        raw_display_handle: RawDisplayHandle::Wayland(WaylandDisplayHandle::new(
                            display,
                        )),
                        raw_window_handle: RawWindowHandle::Wayland(WaylandWindowHandle::new(
                            window,
                        )),
                    })
                } {
                    Ok(s) => s,
                    Err(e) => {
                        tracing::warn!("renderer: no surface via {label}: {e}");
                        continue;
                    }
                };
                match pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions {
                    power_preference: wgpu::PowerPreference::LowPower,
                    compatible_surface: Some(&surface),
                    force_fallback_adapter,
                })) {
                    Some(adapter) => {
                        // A CPU rasterizer only counts on the explicit software
                        // attempt — a non-fallback attempt returning one (mesa's
                        // llvmpipe posing as the Vulkan adapter on old Intel)
                        // must keep looking so a real GPU on another backend
                        // gets its turn.
                        if !force_fallback_adapter
                            && adapter.get_info().device_type == wgpu::DeviceType::Cpu
                        {
                            tracing::warn!(
                                "renderer: {label} offered a CPU adapter ({}); trying next backend",
                                adapter.get_info().name
                            );
                            continue;
                        }
                        tracing::info!("renderer: adapter via {label}: {:?}", adapter.get_info());
                        chosen = Some((surface, adapter));
                        _live_instance = Some(instance);
                        break;
                    }
                    None => tracing::warn!("renderer: no adapter via {label}"),
                }
            }
            let (surface, adapter) = chosen.ok_or_else(|| {
                anyhow!("no GPU or software adapter could present to the surface (is vulkan-loader on LD_LIBRARY_PATH?)")
            })?;
            let (device, queue) = pollster::block_on(adapter.request_device(
                &wgpu::DeviceDescriptor {
                    label: Some("waverunner"),
                    required_features: wgpu::Features::empty(),
                    required_limits: wgpu::Limits {
                        // Allow surface expansion to full screen height (>2048 on 4K displays).
                        max_texture_dimension_2d: MAX_TEXTURE_SIDE,
                        // The icon array is one texture layer per app icon plus a
                        // reserved block (rank hits + pending installs + thumbs +
                        // minimized = 113). downlevel_defaults() caps
                        // texture_array_layers at
                        // 256, so a machine with ~160+ .desktop entries overflowed
                        // it — create_texture("waverunner.icons") panicked the
                        // daemon on cold start (267 layers on a 170-app machine,
                        // 2026-09-01). Request what the adapter actually offers
                        // (2048 on any real GPU, incl. this Iris Xe / RTX 4050);
                        // this never exceeds hardware, so device creation is safe.
                        max_texture_array_layers: adapter.limits().max_texture_array_layers,
                        ..wgpu::Limits::downlevel_defaults()
                    },
                    // Small device-memory blocks (8 MB, growing to 64): the
                    // default hint, Performance, takes GPU memory 128 MB at a
                    // time — sized for a game, and most of what the shell held
                    // on the Acer (night audit, 2026-10-04).
                    memory_hints: wgpu::MemoryHints::MemoryUsage,
                },
                None,
            ))
            .context("wgpu device request failed")?;
            let live_instance =
                _live_instance.ok_or_else(|| anyhow!("the chosen adapter has no instance"))?;
            let shared = SHARED_GPU.with(|g| {
                let mut g = g.borrow_mut();
                // A renderer that could not use the shared device keeps its
                // own; the shared one stays what the others draw on.
                if g.is_none() {
                    *g = Some(SharedGpu {
                        instance: live_instance,
                        adapter: adapter.clone(),
                        device: device.clone(),
                        queue: queue.clone(),
                    });
                    true
                } else {
                    false
                }
            });
            (surface, adapter, device, queue, shared)
        };
        let software = adapter.get_info().device_type == wgpu::DeviceType::Cpu;
        // Only a software adapter gets the fixed throttle: every frame there
        // costs real cores. The GL backend used to be throttled too (100 ms a
        // frame), because Mesa's EGL/Wayland swap blocked the loop until the
        // compositor's frame callback (Golem #40, the ASUS 2026-09-09).
        // `NoVblankWait` removed that wait, and the fixed 10 fps had become the
        // problem: every animation the pointer didn't start (Super+Space, F4,
        // the install ring) crawled on GL machines only (Max, 2026-09-29: "on
        // the thinkpad it works amazingly… on the macbook, it is not the
        // same"). But unthrottled, an old iGPU saturates (the MacBook's HD 5000
        // sat at 97-100% busy through an open), frames queue in the driver, and
        // the loop stalls up to 1.8 s again. So GL is paced by the GPU itself:
        // a new frame starts only when the previous one's GPU work is done
        // (`gpu_ready`), the smoothest rate the machine can actually deliver,
        // and the loop never waits on it.
        let frame_throttle = software;
        //
        // Vulkan needs the same pacing (2026-09-30): when the GPU saturates,
        // `get_current_texture` waits for a free image, again on the event
        // loop. On Max's Lenovo (Iris Xe, 3200x2000 at 165 Hz) the loop
        // went silent for 11.9 s while the settings panel animated, and every
        // Super+Space queued behind it ("my dock still gets stuck/confused").
        // The ThinkPad's GPU has headroom, so it never showed there. So every
        // hardware adapter is paced by the GPU; a fast GPU finishes each frame
        // long before the next, so pacing costs it nothing.
        let pace_by_gpu = !software;
        // The Vulkan and GL backends can pass a present's damage on (see
        // `third_party/wgpu-hal`).
        let track_damage = std::env::var_os("WAVERUNNER_FULL_DAMAGE").is_none();
        let present_damage = track_damage
            && matches!(
                adapter.get_info().backend,
                wgpu::Backend::Vulkan | wgpu::Backend::Gl
            );

        let caps = surface.get_capabilities(&adapter);
        // Transparency requires a premultiplied compositing mode.
        let alpha_mode = if caps
            .alpha_modes
            .contains(&wgpu::CompositeAlphaMode::PreMultiplied)
        {
            wgpu::CompositeAlphaMode::PreMultiplied
        } else if adapter.get_info().backend == wgpu::Backend::Gl {
            // wgpu's GL backend always REPORTS Opaque (a `//TODO` in
            // wgpu-hal), but on Wayland the EGL config has 8 alpha bits, the
            // buffers go out as ARGB8888 and the compositor blends them:
            // the bar and the dock's glass are translucent. Measured over an
            // orange wallpaper on the ASUS X550LC (Haswell, GL), 2026-10-02,
            // identical to the same chip on Vulkan. Nothing to warn about.
            tracing::debug!(
                "GL reports {:?}; its Wayland buffers carry alpha",
                caps.alpha_modes
            );
            caps.alpha_modes[0]
        } else {
            tracing::warn!(
                "premultiplied alpha unsupported, transparency may be wrong: {:?}",
                caps.alpha_modes
            );
            caps.alpha_modes[0]
        };
        let format = caps
            .formats
            .first()
            .copied()
            .ok_or_else(|| anyhow!("surface reports no formats"))?;
        let config = wgpu::SurfaceConfiguration {
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
            format,
            width,
            height,
            present_mode: wgpu::PresentMode::Mailbox,
            desired_maximum_frame_latency: 2,
            alpha_mode,
            view_formats: vec![],
        };
        let config = if caps.present_modes.contains(&config.present_mode) {
            config
        } else {
            wgpu::SurfaceConfiguration {
                present_mode: wgpu::PresentMode::Fifo,
                ..config
            }
        };
        surface.configure(&device, &config);

        // Group 0: globals (screen size + animation alpha).
        let globals_buf = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("waverunner.globals"),
            size: std::mem::size_of::<Globals>() as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        // Shaders, pipelines, layouts and samplers: built ONCE per GPU device
        // and format ([`Pipelines`]); every later renderer on the shared
        // device clones the handles instead of compiling the same five
        // shaders and eight pipelines again.
        let format = config.format;
        let cached = SHARED_PIPELINES
            .with(|p| p.borrow().clone())
            .filter(|p| on_shared_gpu && p.format == format);
        let pipes = match cached {
            Some(pipes) => pipes,
            None => {
                let globals_layout =
                    device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
                        label: Some("waverunner.globals"),
                        entries: &[wgpu::BindGroupLayoutEntry {
                            binding: 0,
                            visibility: wgpu::ShaderStages::VERTEX_FRAGMENT,
                            ty: wgpu::BindingType::Buffer {
                                ty: wgpu::BufferBindingType::Uniform,
                                has_dynamic_offset: false,
                                min_binding_size: None,
                            },
                            count: None,
                        }],
                    });
                let blend = wgpu::BlendState {
                    color: wgpu::BlendComponent {
                        src_factor: wgpu::BlendFactor::One,
                        dst_factor: wgpu::BlendFactor::OneMinusSrcAlpha,
                        operation: wgpu::BlendOperation::Add,
                    },
                    alpha: wgpu::BlendComponent {
                        src_factor: wgpu::BlendFactor::One,
                        dst_factor: wgpu::BlendFactor::OneMinusSrcAlpha,
                        operation: wgpu::BlendOperation::Add,
                    },
                };
                let target = [Some(wgpu::ColorTargetState {
                    format: config.format,
                    blend: Some(blend),
                    write_mask: wgpu::ColorWrites::ALL,
                })];

                // Top-edge shadow pipeline (instanced gradient bands). Shares the
                // globals bind group and premultiplied blend target with the rects.
                let shadow_shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
                    label: Some("waverunner.edge_shadow"),
                    source: wgpu::ShaderSource::Wgsl(
                        include_str!("shaders/edge_shadow.wgsl").into(),
                    ),
                });
                let shadow_layout =
                    device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
                        label: Some("waverunner.shadow"),
                        bind_group_layouts: &[&globals_layout],
                        push_constant_ranges: &[],
                    });
                let shadow_pipeline =
                    device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                        label: Some("waverunner.shadow"),
                        layout: Some(&shadow_layout),
                        vertex: wgpu::VertexState {
                            module: &shadow_shader,
                            entry_point: Some("vs_main"),
                            compilation_options: Default::default(),
                            buffers: &[wgpu::VertexBufferLayout {
                                array_stride: std::mem::size_of::<ShadowInstance>() as u64,
                                step_mode: wgpu::VertexStepMode::Instance,
                                attributes: &wgpu::vertex_attr_array![
                                    0 => Float32x2, 1 => Float32x2, 2 => Float32x4,
                                    3 => Float32, 4 => Float32, 5 => Float32x4
                                ],
                            }],
                        },
                        fragment: Some(wgpu::FragmentState {
                            module: &shadow_shader,
                            entry_point: Some("fs_main"),
                            compilation_options: Default::default(),
                            targets: &target,
                        }),
                        primitive: wgpu::PrimitiveState {
                            topology: wgpu::PrimitiveTopology::TriangleStrip,
                            ..Default::default()
                        },
                        depth_stencil: None,
                        multisample: wgpu::MultisampleState::default(),
                        multiview: None,
                        cache: None,
                    });

                // Rounded-rect pipeline (instanced SDF quads).
                let rect_shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
                    label: Some("waverunner.rounded_rect"),
                    source: wgpu::ShaderSource::Wgsl(
                        include_str!("shaders/rounded_rect.wgsl").into(),
                    ),
                });
                let rect_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
                    label: Some("waverunner.rect"),
                    bind_group_layouts: &[&globals_layout],
                    push_constant_ranges: &[],
                });
                let rect_pipeline =
                    device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                        label: Some("waverunner.rect"),
                        layout: Some(&rect_layout),
                        vertex: wgpu::VertexState {
                            module: &rect_shader,
                            entry_point: Some("vs_main"),
                            compilation_options: Default::default(),
                            buffers: &[wgpu::VertexBufferLayout {
                                array_stride: std::mem::size_of::<RectInstance>() as u64,
                                step_mode: wgpu::VertexStepMode::Instance,
                                attributes: &wgpu::vertex_attr_array![
                                    0 => Float32x2, 1 => Float32x2, 2 => Float32x4, 3 => Float32,
                                    4 => Float32, 5 => Float32
                                ],
                            }],
                        },
                        fragment: Some(wgpu::FragmentState {
                            module: &rect_shader,
                            entry_point: Some("fs_main"),
                            compilation_options: Default::default(),
                            targets: &target,
                        }),
                        primitive: wgpu::PrimitiveState {
                            topology: wgpu::PrimitiveTopology::TriangleStrip,
                            ..Default::default()
                        },
                        depth_stencil: None,
                        multisample: wgpu::MultisampleState::default(),
                        multiview: None,
                        cache: None,
                    });

                // Icon pipeline (instanced textured quads over a texture array).
                let icon_shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
                    label: Some("waverunner.icon"),
                    source: wgpu::ShaderSource::Wgsl(include_str!("shaders/icon.wgsl").into()),
                });
                let icon_bind_layout =
                    device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
                        label: Some("waverunner.icons"),
                        entries: &[
                            wgpu::BindGroupLayoutEntry {
                                binding: 0,
                                visibility: wgpu::ShaderStages::FRAGMENT,
                                ty: wgpu::BindingType::Texture {
                                    sample_type: wgpu::TextureSampleType::Float {
                                        filterable: true,
                                    },
                                    view_dimension: wgpu::TextureViewDimension::D2Array,
                                    multisampled: false,
                                },
                                count: None,
                            },
                            wgpu::BindGroupLayoutEntry {
                                binding: 1,
                                visibility: wgpu::ShaderStages::FRAGMENT,
                                ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                                count: None,
                            },
                        ],
                    });
                let icon_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
                    label: Some("waverunner.icon"),
                    bind_group_layouts: &[&globals_layout, &icon_bind_layout],
                    push_constant_ranges: &[],
                });
                let icon_pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                    label: Some("waverunner.icon"),
                    layout: Some(&icon_layout),
                    vertex: wgpu::VertexState {
                        module: &icon_shader,
                        entry_point: Some("vs_main"),
                        compilation_options: Default::default(),
                        buffers: &[wgpu::VertexBufferLayout {
                            array_stride: std::mem::size_of::<IconInstance>() as u64,
                            step_mode: wgpu::VertexStepMode::Instance,
                            attributes: &wgpu::vertex_attr_array![
                                0 => Float32x2, 1 => Float32x2, 2 => Uint32, 3 => Float32x4, 4 => Float32,
                                5 => Float32x4
                            ],
                        }],
                    },
                    fragment: Some(wgpu::FragmentState {
                        module: &icon_shader,
                        entry_point: Some("fs_main"),
                        compilation_options: Default::default(),
                        targets: &target,
                    }),
                    primitive: wgpu::PrimitiveState {
                        topology: wgpu::PrimitiveTopology::TriangleStrip,
                        ..Default::default()
                    },
                    depth_stencil: None,
                    multisample: wgpu::MultisampleState::default(),
                    multiview: None,
                    cache: None,
                });
                let icon_sampler = device.create_sampler(&wgpu::SamplerDescriptor {
                    label: Some("waverunner.icons"),
                    mag_filter: wgpu::FilterMode::Linear,
                    min_filter: wgpu::FilterMode::Linear,
                    // Trilinear across the mip chain so minified icons (the small
                    // size level, and magnification transitions) stay clean.
                    mipmap_filter: wgpu::FilterMode::Linear,
                    ..Default::default()
                });

                // Blit pipeline: copies the offscreen scene texture to the screen
                // (and, later, samples the blurred copy for the box backdrop).
                let blit_shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
                    label: Some("waverunner.blit"),
                    source: wgpu::ShaderSource::Wgsl(include_str!("shaders/blit.wgsl").into()),
                });
                let blit_layout =
                    device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
                        label: Some("waverunner.blit"),
                        entries: &[
                            wgpu::BindGroupLayoutEntry {
                                binding: 0,
                                visibility: wgpu::ShaderStages::FRAGMENT,
                                ty: wgpu::BindingType::Texture {
                                    sample_type: wgpu::TextureSampleType::Float {
                                        filterable: true,
                                    },
                                    view_dimension: wgpu::TextureViewDimension::D2,
                                    multisampled: false,
                                },
                                count: None,
                            },
                            wgpu::BindGroupLayoutEntry {
                                binding: 1,
                                visibility: wgpu::ShaderStages::FRAGMENT,
                                ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                                count: None,
                            },
                        ],
                    });
                let blit_sampler = device.create_sampler(&wgpu::SamplerDescriptor {
                    label: Some("waverunner.blit"),
                    mag_filter: wgpu::FilterMode::Linear,
                    min_filter: wgpu::FilterMode::Linear,
                    ..Default::default()
                });
                let blit_pl = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
                    label: Some("waverunner.blit"),
                    bind_group_layouts: &[&blit_layout],
                    push_constant_ranges: &[],
                });
                // Replace blend: write the (premultiplied) source pixels verbatim.
                let blit_target = [Some(wgpu::ColorTargetState {
                    format: config.format,
                    blend: None,
                    write_mask: wgpu::ColorWrites::ALL,
                })];
                let blit_pipeline =
                    device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                        label: Some("waverunner.blit"),
                        layout: Some(&blit_pl),
                        vertex: wgpu::VertexState {
                            module: &blit_shader,
                            entry_point: Some("vs_main"),
                            compilation_options: Default::default(),
                            buffers: &[],
                        },
                        fragment: Some(wgpu::FragmentState {
                            module: &blit_shader,
                            entry_point: Some("fs_main"),
                            compilation_options: Default::default(),
                            targets: &blit_target,
                        }),
                        primitive: wgpu::PrimitiveState::default(),
                        depth_stencil: None,
                        multisample: wgpu::MultisampleState::default(),
                        multiview: None,
                        cache: None,
                    });
                // Box frosted backdrop: samples/blurs the scene texture over the box
                // region, premultiplied "over" blend (same as the scene pipelines).
                let backdrop_shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
                    label: Some("waverunner.box-backdrop"),
                    source: wgpu::ShaderSource::Wgsl(
                        include_str!("shaders/box_backdrop.wgsl").into(),
                    ),
                });
                let backdrop_pl = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
                    label: Some("waverunner.box-backdrop"),
                    bind_group_layouts: &[&blit_layout],
                    push_constant_ranges: &[],
                });
                let box_backdrop_pipeline =
                    device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                        label: Some("waverunner.box-backdrop"),
                        layout: Some(&backdrop_pl),
                        vertex: wgpu::VertexState {
                            module: &backdrop_shader,
                            entry_point: Some("vs_main"),
                            compilation_options: Default::default(),
                            buffers: &[wgpu::VertexBufferLayout {
                                array_stride: std::mem::size_of::<BoxBackdropInstance>() as u64,
                                step_mode: wgpu::VertexStepMode::Instance,
                                attributes: &wgpu::vertex_attr_array![
                                    0 => Float32x2, 1 => Float32x2, 2 => Float32, 3 => Float32x2
                                ],
                            }],
                        },
                        fragment: Some(wgpu::FragmentState {
                            module: &backdrop_shader,
                            entry_point: Some("fs_main"),
                            compilation_options: Default::default(),
                            targets: &target,
                        }),
                        primitive: wgpu::PrimitiveState {
                            topology: wgpu::PrimitiveTopology::TriangleStrip,
                            ..Default::default()
                        },
                        depth_stencil: None,
                        multisample: wgpu::MultisampleState::default(),
                        multiview: None,
                        cache: None,
                    });

                // Box erase: multiplies the box region by (1 - coverage) so the
                // backdrop fill replaces the sharp base rather than stacking on it.
                let erase_blend = wgpu::BlendState {
                    color: wgpu::BlendComponent {
                        src_factor: wgpu::BlendFactor::Zero,
                        dst_factor: wgpu::BlendFactor::OneMinusSrcAlpha,
                        operation: wgpu::BlendOperation::Add,
                    },
                    alpha: wgpu::BlendComponent {
                        src_factor: wgpu::BlendFactor::Zero,
                        dst_factor: wgpu::BlendFactor::OneMinusSrcAlpha,
                        operation: wgpu::BlendOperation::Add,
                    },
                };
                let erase_target = [Some(wgpu::ColorTargetState {
                    format: config.format,
                    blend: Some(erase_blend),
                    write_mask: wgpu::ColorWrites::ALL,
                })];
                let box_erase_pipeline =
                    device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                        label: Some("waverunner.box-erase"),
                        layout: Some(&backdrop_pl),
                        vertex: wgpu::VertexState {
                            module: &backdrop_shader,
                            entry_point: Some("vs_main"),
                            compilation_options: Default::default(),
                            buffers: &[wgpu::VertexBufferLayout {
                                array_stride: std::mem::size_of::<BoxBackdropInstance>() as u64,
                                step_mode: wgpu::VertexStepMode::Instance,
                                attributes: &wgpu::vertex_attr_array![
                                    0 => Float32x2, 1 => Float32x2, 2 => Float32, 3 => Float32x2
                                ],
                            }],
                        },
                        fragment: Some(wgpu::FragmentState {
                            module: &backdrop_shader,
                            entry_point: Some("fs_erase"),
                            compilation_options: Default::default(),
                            targets: &erase_target,
                        }),
                        primitive: wgpu::PrimitiveState {
                            topology: wgpu::PrimitiveTopology::TriangleStrip,
                            ..Default::default()
                        },
                        depth_stencil: None,
                        multisample: wgpu::MultisampleState::default(),
                        multiview: None,
                        cache: None,
                    });

                let blur_shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
                    label: Some("waverunner.blur"),
                    source: wgpu::ShaderSource::Wgsl(include_str!("shaders/blur.wgsl").into()),
                });
                // Replace-blend (overwrite the target); both passes share the layout.
                let blur_target = [Some(wgpu::ColorTargetState {
                    format: config.format,
                    blend: None,
                    write_mask: wgpu::ColorWrites::ALL,
                })];
                let make_blur_pipeline = |entry: &str| {
                    device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                        label: Some("waverunner.blur"),
                        layout: Some(&blit_pl),
                        vertex: wgpu::VertexState {
                            module: &blur_shader,
                            entry_point: Some("vs_main"),
                            compilation_options: Default::default(),
                            buffers: &[],
                        },
                        fragment: Some(wgpu::FragmentState {
                            module: &blur_shader,
                            entry_point: Some(entry),
                            compilation_options: Default::default(),
                            targets: &blur_target,
                        }),
                        primitive: wgpu::PrimitiveState::default(),
                        depth_stencil: None,
                        multisample: wgpu::MultisampleState::default(),
                        multiview: None,
                        cache: None,
                    })
                };
                let blur_pipeline_h = make_blur_pipeline("fs_horizontal");
                let blur_pipeline_v = make_blur_pipeline("fs_vertical");
                let built = Pipelines {
                    format,
                    globals_layout,
                    shadow_pipeline,
                    rect_pipeline,
                    icon_pipeline,
                    icon_bind_layout,
                    icon_sampler,
                    blit_pipeline,
                    blit_layout,
                    blit_sampler,
                    box_backdrop_pipeline,
                    box_erase_pipeline,
                    blur_pipeline_h,
                    blur_pipeline_v,
                };
                if on_shared_gpu {
                    SHARED_PIPELINES.with(|p| *p.borrow_mut() = Some(built.clone()));
                }
                built
            }
        };
        let Pipelines {
            format: _,
            globals_layout,
            shadow_pipeline,
            rect_pipeline,
            icon_pipeline,
            icon_bind_layout,
            icon_sampler,
            blit_pipeline,
            blit_layout,
            blit_sampler,
            box_backdrop_pipeline,
            box_erase_pipeline,
            blur_pipeline_h,
            blur_pipeline_v,
        } = pipes;
        let globals_bind = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("waverunner.globals"),
            layout: &globals_layout,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: globals_buf.as_entire_binding(),
            }],
        });

        // Text stack (glyphon).
        // One font database for every renderer, remembered between runs
        // (crate::font_index): the cold scan was the slowest part of a start
        // on a spinning disk.
        let (font_system, swash) = shared_text();
        let text_cache = TextCache::new(&device);
        let text_viewport = Viewport::new(&device, &text_cache);
        let mut text_atlas = TextAtlas::new(&device, &queue, &text_cache, format);
        let text_renderer = TextRenderer::new(
            &mut text_atlas,
            &device,
            wgpu::MultisampleState::default(),
            None,
        );

        Ok(Self {
            surface,
            device,
            queue,
            config,
            frame_throttle,
            pace_by_gpu,
            gpu_busy: std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false)),
            scale: if scale > 0.0 { scale } else { 1.0 },
            globals_buf,
            globals_bind,
            shadow_pipeline,
            rect_pipeline,
            blit_pipeline,
            blit_layout,
            blit_sampler,
            box_backdrop_pipeline,
            box_erase_pipeline,
            frost: None,
            blur_pipeline_h,
            blur_pipeline_v,
            icon_pipeline,
            icon_bind_layout,
            icon_sampler,
            icon_bind: None,
            icon_texture: None,
            icon_layer_count: 0,
            icon_layer_cap: 0,
            font_system,
            swash,
            text_viewport,
            text_atlas,
            text_renderer,
            label_cache: std::collections::HashMap::new(),
            volatile: std::collections::HashMap::new(),
            frame_no: 0,
            anim_time: 0.0,
            last_render: None,
            track_damage,
            present_damage,
            damage_prev: None,
            damage_cur: crate::damage::TileMap::default(),
            icon_epoch: 0,
            icon_layer_gen: Vec::new(),
            damage_check: std::env::var_os("WAVERUNNER_DAMAGE_CHECK").map(|v| DamageCheck {
                paths: v == "paths",
                ..DamageCheck::default()
            }),
        })
    }

    /// Handle a compositor-driven buffer size change (output scale change;
    /// the logical surface size itself is fixed by design).
    pub fn resize(&mut self, width: u32, height: u32) {
        if width == self.config.width && height == self.config.height {
            return;
        }
        self.config.width = width;
        self.config.height = height;
        self.surface.configure(&self.device, &self.config);
        self.damage_prev = None;
        // The frost targets are rebuilt at the new size by the next frame
        // that needs them.
        self.frost = None;
    }

    /// Upload the icon texture array delivered by the indexer thread.
    /// `icons` holds one premultiplied RGBA8 `ICON_SIZE`² image per app.
    /// `RANK_HITS_MAX` + `PENDING_INSTALL_CAP` + `THUMB_CAP` +
    /// `MIN_THUMB_CAP` extra layers are reserved past the end, in that
    /// order: dynamic package-search icons, packages installing in the
    /// grid, file thumbnails, and minimized-window thumbnails
    /// ([`Renderer::update_icon_layer`]).
    pub fn set_icons(&mut self, icons: &[Vec<u8>]) {
        // New app set: previously shaped labels may be stale.
        self.label_cache.clear();
        let reserved = crate::nix::RANK_HITS_MAX
            + crate::nix::PENDING_INSTALL_CAP
            + crate::thumbs::THUMB_CAP
            + crate::minimized::MIN_THUMB_CAP;
        self.upload_icon_array(icons.len(), reserved, icons.iter());
    }

    /// Populate the OPTIONS surface's icon array. The topbar renderer is a
    /// separate instance from the dock's, so this array is entirely its own:
    /// the notification card avatars occupy layers `[0, notif.len())` and the
    /// clipboard thumbnails follow at `[notif.len(), notif.len() + clip.len())`,
    /// each addressed by [`IconInst::layer`]. Re-uploaded wholesale whenever
    /// either set changes (see `App::upload_options_icons`).
    /// The OPTIONS surface's icon array: notification avatars, then clipboard
    /// thumbnails, then the playing box's cover art. Order is the layer
    /// numbering, so every caller offsets by the lengths of the arrays before
    /// its own — see `App::play_art_slot`.
    pub fn set_options_icons(&mut self, notif: &[Vec<u8>], clip: &[Vec<u8>], art: &[Vec<u8>]) {
        self.upload_icon_array(
            notif.len() + clip.len() + art.len(),
            0,
            notif.iter().chain(clip.iter()).chain(art.iter()),
        );
    }

    /// Allocate an **empty** icon array of exactly `layers` layers, each to be
    /// filled later by [`Renderer::update_icon_layer`].
    ///
    /// For surfaces that stream layers in one at a time — the STAGE deck — and
    /// size their need up front. [`Renderer::set_icons`] would be wrong for
    /// them twice over: it reserves the dock's 97 extra layers (~34MB of
    /// texture the deck can never address), and growing by one means
    /// reallocating and re-uploading the whole array. Pre-sized, an arriving
    /// thumbnail is only ever a single-layer write.
    pub fn alloc_icon_array(&mut self, layers: u32) {
        self.upload_icon_array(layers as usize, 0, std::iter::empty());
    }

    /// Shared core: (re)allocate the icon texture array with `count + reserved`
    /// layers, write each chain to its layer, and rebuild the sampler bind group.
    fn upload_icon_array<'a>(
        &mut self,
        count: usize,
        reserved: usize,
        chains: impl Iterator<Item = &'a Vec<u8>>,
    ) {
        // The RESERVED layers are not allocated up front: they are the
        // dock's slots for search hits, pending installs, file thumbnails and
        // minimized windows — 113 layers, ~39 MB of GPU memory, mostly never
        // written. The array is made for what it holds and grows when a
        // reserved layer is first written ([`Self::update_icon_layer`]).
        let layers = icon_layers(count);
        self.icon_layer_cap = icon_layers(count + reserved);
        // A new array: every icon draw may show something else now.
        self.icon_epoch += 1;
        self.icon_layer_gen.clear();
        let texture = self.create_icon_texture(layers);
        for (i, chain) in chains.enumerate() {
            write_icon_chain(&self.queue, &texture, i as u32, chain);
        }
        self.bind_icon_texture(texture, layers);
    }

    fn create_icon_texture(&self, layers: u32) -> wgpu::Texture {
        self.device.create_texture(&wgpu::TextureDescriptor {
            label: Some("waverunner.icons"),
            size: wgpu::Extent3d {
                width: ICON_SIZE,
                height: ICON_SIZE,
                depth_or_array_layers: layers,
            },
            mip_level_count: ICON_MIPS,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Rgba8UnormSrgb,
            usage: wgpu::TextureUsages::TEXTURE_BINDING
                | wgpu::TextureUsages::COPY_DST
                | wgpu::TextureUsages::COPY_SRC,
            view_formats: &[],
        })
    }

    fn bind_icon_texture(&mut self, texture: wgpu::Texture, layers: u32) {
        let view = texture.create_view(&wgpu::TextureViewDescriptor {
            dimension: Some(wgpu::TextureViewDimension::D2Array),
            ..Default::default()
        });
        self.icon_bind = Some(self.device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("waverunner.icons"),
            layout: &self.icon_bind_layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::TextureView(&view),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::Sampler(&self.icon_sampler),
                },
            ],
        }));
        self.icon_layer_count = layers;
        self.icon_texture = Some(texture);
    }

    /// Make the icon array hold at least `need` layers (within its cap),
    /// keeping what is in it: the old layers are copied on the GPU.
    fn grow_icon_array(&mut self, need: u32) {
        let Some(old) = self.icon_texture.take() else {
            return;
        };
        let had = self.icon_layer_count;
        // A step at a time, not a layer at a time: one copy per burst of
        // new thumbnails rather than one per thumbnail.
        let layers = icon_layers(need.max(had + ICON_GROW_STEP) as usize).min(self.icon_layer_cap);
        let texture = self.create_icon_texture(layers);
        let mut encoder = self
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("waverunner.icons-grow"),
            });
        for mip in 0..ICON_MIPS {
            let size = (ICON_SIZE >> mip).max(1);
            let at = |texture| wgpu::TexelCopyTextureInfo {
                texture,
                mip_level: mip,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            };
            encoder.copy_texture_to_texture(
                at(&old),
                at(&texture),
                wgpu::Extent3d {
                    width: size,
                    height: size,
                    depth_or_array_layers: had,
                },
            );
        }
        self.queue.submit(Some(encoder.finish()));
        self.bind_icon_texture(texture, layers);
    }

    /// Width in pixels of `text` shaped at `font_px` — the same family
    /// and shaping the labels render with, so the search caret can sit
    /// exactly after the glyphs instead of guessing from char counts.
    /// Whether sustained ambient animation must be frame-throttled to keep
    /// the single-threaded event loop responsive — true on a software
    /// adapter (see the `frame_throttle` field).
    /// The output's scale changed (fractional scaling): physical pixels per
    /// logical pixel from now on. Pair with [`Renderer::resize`].
    pub fn set_scale(&mut self, scale: f32) {
        if scale > 0.0 && scale != self.scale {
            self.scale = scale;
            // Cached labels were shaped at the OLD physical size (metrics x
            // scale), and their key has no scale in it. Kept, they draw at the
            // wrong size and off-centre. On the MacBook (1.0) the renderer came
            // up at the 2x fallback before the compositor's scale arrived, and
            // the top bar's icon glyphs stayed at 2x, spilling out of their
            // pills while the uncached clock was right (Max, 2026-09-30: "the
            // macbook options are oversized").
            self.label_cache.clear();
            self.volatile.clear();
            self.damage_prev = None;
        }
    }

    pub fn needs_frame_throttle(&self) -> bool {
        self.frame_throttle
    }

    /// Whether the GPU has finished the last frame, so a new one can start
    /// without queueing behind it. Always true on a software adapter. Never
    /// blocks: a non-blocking poll delivers the completion, if it happened.
    pub fn gpu_ready(&self) -> bool {
        if !self.pace_by_gpu {
            return true;
        }
        let _ = self.device.poll(wgpu::Maintain::Poll);
        !self.gpu_busy.load(std::sync::atomic::Ordering::Acquire)
    }

    pub fn measure_text(&mut self, text: &str, font_px: f32, family: Option<&str>) -> f32 {
        if text.is_empty() {
            return 0.0;
        }
        let _perf = crate::perf::MEASURE.time();
        // Measured once per (text, size, family): the dock measured the word
        // "Search" on every frame it drew — a fresh buffer, shaped, ~150 µs
        // on the Acer, 8 % of a launcher frame (2026-10-04). The answer only
        // depends on the fonts, which are the same for every renderer.
        MEASURED.with(|cache| {
            let mut cache = cache.borrow_mut();
            let (map, key) = &mut *cache;
            key.clear();
            use std::fmt::Write as _;
            let _ = write!(
                key,
                "{}\u{1}{}\u{1}{text}",
                font_px.to_bits(),
                family.unwrap_or("")
            );
            if let Some(&w) = map.get(key.as_str()) {
                return w;
            }
            crate::perf::SHAPE.hit();
            let mut font_system = self.font_system.borrow_mut();
            let mut buffer =
                TextBuffer::new(&mut font_system, Metrics::new(font_px, font_px * 1.3));
            let (fam, weight) = resolve_family(family);
            buffer.set_text(
                &mut font_system,
                text,
                Attrs::new().family(fam).weight(weight),
                Shaping::Advanced,
            );
            buffer.shape_until_scroll(&mut font_system, false);
            let w = buffer
                .layout_runs()
                .map(|run| run.line_w)
                .fold(0.0, f32::max);
            // Live text (a query, a clock) makes new keys for ever: start
            // over rather than grow.
            if map.len() >= MEASURED_CAP {
                map.clear();
            }
            map.insert(key.clone(), w);
            w
        })
    }

    /// Overwrite one icon texture-array layer (a dynamic package icon in
    /// the reserved tail of the array). Out-of-range layers and missing
    /// textures are ignored — a rescan re-uploads shortly anyway.
    pub fn update_icon_layer(&mut self, layer: u32, pixels: &[u8]) {
        if self.icon_texture.is_none() {
            return;
        }
        if layer >= self.icon_layer_cap || pixels.len() != ICON_CHAIN_BYTES {
            return;
        }
        if layer >= self.icon_layer_count {
            self.grow_icon_array(layer + 1);
        }
        let Some(texture) = &self.icon_texture else {
            return;
        };
        write_icon_chain(&self.queue, texture, layer, pixels);
        // The layer shows something else: its draws must damage.
        let at = layer as usize;
        if self.icon_layer_gen.len() <= at {
            self.icon_layer_gen.resize(at + 1, 0);
        }
        self.icon_layer_gen[at] = self.icon_layer_gen[at].wrapping_add(1);
    }

    /// Render one frame of the given scene.
    ///
    /// `cursor` is the pointer position in surface pixels, used for the
    /// glass cursor-spotlight effect; `None` when the pointer is outside
    /// the surface.
    pub fn render(
        &mut self,
        scene: &Scene,
        text_color: [f32; 4],
        cursor: Option<(f32, f32)>,
        squircle: f32,
        thumb_base: u32,
        mut visible: Option<&mut crate::visible::SurfaceVisible>,
    ) -> anyhow::Result<Frame> {
        let _perf = crate::perf::RENDER.time();
        // `w`/`h` are the physical framebuffer size; the scene is authored in
        // logical px. Geometry pipelines map `px / screen → NDC`, so feeding a
        // *logical* `screen` while the framebuffer is physical scales all
        // geometry up to physical resolution for free — no shader changes.
        // Text (glyphon) is the exception: it must be shaped at physical px to
        // stay crisp, so its metrics, positions and clip bounds are scaled by
        // `scale` below.
        let (w, h) = (self.config.width, self.config.height);
        let scale = self.scale;
        let (lw, lh) = (w as f32 / scale, h as f32 / scale);

        // Advance anim_time only while frames are rendered — no phase jump
        // when the dock hides (no frames) and then reappears.
        let now = std::time::Instant::now();
        let dt = self
            .last_render
            .map(|l| now.duration_since(l).as_secs_f32().min(0.1))
            .unwrap_or(0.0);
        self.last_render = Some(now);
        self.anim_time += dt;

        let cursor_px = cursor.map(|(x, y)| [x, y]).unwrap_or([-9999.0, -9999.0]);

        // Instance buffers: unclipped ranges first, then one scissored
        // range per section grid.
        // The first rect is always the card background (per Scene layout);
        // give it the glass material flag so the shader applies all 9 layers.
        let mut shadows: Vec<ShadowInstance> = scene.shadows.iter().map(shadow_instance).collect();
        let n_shadows = shadows.len() as u32;
        // Overlay shadows (neumorphic button depth) ride the same buffer but are
        // drawn after the unclipped fills, not behind them.
        shadows.extend(scene.overlay_shadows.iter().map(shadow_instance));
        let n_overlay_shadows = shadows.len() as u32 - n_shadows;
        let mut rects: Vec<RectInstance> = scene.rects.iter().map(rect_instance).collect();
        let n_rects_unclipped = rects.len() as u32;
        let mut icons: Vec<IconInstance> = scene.icons.iter().map(icon_instance).collect();
        let n_icons_unclipped = icons.len() as u32;
        // Per grid: (clip, rect range, icon range) into the shared buffers.
        let grid_ranges: Vec<(
            crate::content::Rect,
            std::ops::Range<u32>,
            std::ops::Range<u32>,
        )> = scene
            .grids
            .iter()
            .map(|grid| {
                let r0 = rects.len() as u32;
                rects.extend(grid.rects.iter().map(rect_instance));
                let i0 = icons.len() as u32;
                icons.extend(grid.icons.iter().map(icon_instance));
                (grid.clip, r0..rects.len() as u32, i0..icons.len() as u32)
            })
            .collect();
        // Overlay icons (drag ghost) ride the same buffer, drawn last.
        let o0 = icons.len() as u32;
        icons.extend(scene.overlay.iter().map(icon_instance));
        let overlay_range = o0..icons.len() as u32;

        // Shape the visible labels and hand them to glyphon.
        let alpha = scene.alpha.clamp(0.0, 1.0);
        let text_rgba = glyphon::Color::rgba(
            (text_color[0] * 255.0) as u8,
            (text_color[1] * 255.0) as u8,
            (text_color[2] * 255.0) as u8,
            (text_color[3] * alpha * 255.0) as u8,
        );
        // Collect every label with its default clip: grid labels clip to
        // the grid viewport, top-level labels to their own clip rect.
        let full = crate::content::Rect {
            x: 0.0,
            y: 0.0,
            w: lw,
            h: lh,
        };
        let mut all_labels: Vec<(&crate::content::Label, crate::content::Rect)> = Vec::new();
        for label in &scene.labels {
            all_labels.push((label, label.clip.unwrap_or(full)));
        }
        for grid in &scene.grids {
            for label in &grid.labels {
                all_labels.push((label, label.clip.unwrap_or(grid.clip)));
            }
        }

        // Shaping is by far the most expensive step of a frame, so
        // cacheable labels (stable text like app names) keep their
        // shaped buffers across frames; volatile ones (the live query)
        // are shaped fresh into `fresh` each frame.
        // Shaped at physical px (metrics × scale) so glyphs are rasterized at
        // the resolution they are displayed, then laid out in physical coords.
        let shape = |font_system: &mut FontSystem, label: &crate::content::Label| {
            crate::perf::SHAPE.hit();
            let mut buffer = TextBuffer::new(
                font_system,
                Metrics::new(label.font_px * scale, label.line_px * scale),
            );
            buffer.set_size(
                font_system,
                Some(label.max_w * scale),
                Some(label.line_px * scale),
            );
            let (family, weight) = resolve_family(label.family);
            buffer.set_text(
                font_system,
                &label.text,
                Attrs::new().family(family).weight(weight),
                Shaping::Advanced,
            );
            buffer.shape_until_scroll(font_system, false);
            buffer
        };
        self.frame_no += 1;
        let frame_no = self.frame_no;
        // Volatile labels nobody drew for a while are forgotten.
        if self.volatile.len() > 512 {
            self.volatile.retain(|_, v| v.used + 1 >= frame_no);
        } else {
            self.volatile
                .retain(|_, v| v.used + VOLATILE_KEEP >= frame_no);
        }
        // Where each label's shaped buffer lives: (volatile?, key).
        let mut shaped: Vec<(bool, String)> = Vec::with_capacity(all_labels.len());
        let font_system = self.font_system.clone();
        let mut font_system = font_system.borrow_mut();
        let swash = self.swash.clone();
        let mut swash = swash.borrow_mut();
        for (label, _) in &all_labels {
            if label.cache {
                let key = label_key(label);
                if !self.label_cache.contains_key(&key) {
                    let buffer = shape(&mut font_system, label);
                    self.label_cache.insert(key.clone(), buffer);
                }
                shaped.push((false, key));
            } else {
                let key = volatile_key(label);
                let want_w = label.max_w * scale;
                match self.volatile.get_mut(&key) {
                    Some(v) if v.max_w == want_w || (v.fits && want_w >= v.line_w) => {
                        v.used = frame_no;
                    }
                    _ => {
                        let buffer = shape(&mut font_system, label);
                        let laid_out: usize = buffer
                            .lines
                            .iter()
                            .map(|l| l.layout_opt().as_ref().map_or(0, Vec::len))
                            .sum();
                        let line_w = buffer.layout_runs().next().map_or(0.0, |run| run.line_w);
                        self.volatile.insert(
                            key.clone(),
                            VolatileLabel {
                                buffer,
                                max_w: want_w,
                                fits: laid_out <= 1,
                                line_w,
                                used: frame_no,
                            },
                        );
                    }
                }
                shaped.push((true, key));
            }
        }

        let dim_rgba = glyphon::Color::rgba(
            (text_color[0] * 255.0) as u8,
            (text_color[1] * 255.0) as u8,
            (text_color[2] * 255.0) as u8,
            (text_color[3] * alpha * 0.45 * 255.0) as u8,
        );
        let mut text_buffers: Vec<(&TextBuffer, (f32, f32), TextBounds, glyphon::Color)> =
            Vec::new();
        // Where each label's ink can fall and what it shows, for the damage.
        let track_damage = self.track_damage;
        let mut label_marks: Vec<([f32; 4], u64)> = Vec::new();
        for (i, (label, clip)) in all_labels.iter().enumerate() {
            let (volatile, key) = &shaped[i];
            let buffer = if *volatile {
                match self.volatile.get(key) {
                    Some(v) => &v.buffer,
                    None => continue,
                }
            } else {
                match self.label_cache.get(key) {
                    Some(buffer) => buffer,
                    None => continue,
                }
            };
            // Measure the shaped line; center about the anchor when
            // requested; snap to whole pixels so glyphs stay crisp.
            // Everything here is physical px: the buffer was shaped at
            // metrics × scale, so `line_w` and the anchor/clip must scale too.
            let line_w = buffer
                .layout_runs()
                .next()
                .map(|run| run.line_w)
                .unwrap_or(0.0)
                .min(label.max_w * scale);
            let left = if label.centered {
                (label.pos.0 * scale - line_w / 2.0).round()
            } else {
                (label.pos.0 * scale).round()
            };
            let top = (label.pos.1 * scale).round();
            let bounds = TextBounds {
                left: (clip.x * scale) as i32,
                top: (clip.y * scale) as i32,
                right: ((clip.x + clip.w) * scale) as i32,
                bottom: ((clip.y + clip.h) * scale).min(h as f32) as i32,
            };
            let col = match label.color {
                Some(c) => glyphon::Color::rgba(
                    (c[0] * 255.0) as u8,
                    (c[1] * 255.0) as u8,
                    (c[2] * 255.0) as u8,
                    (c[3] * alpha * 255.0) as u8,
                ),
                None if label.dim => dim_rgba,
                None => text_rgba,
            };
            if track_damage {
                // Every glyph's ink lies around its origin on the baseline:
                // a font size to each side and below, one and a half above
                // (overhangs, accents, an emoji's bitmap) — and never outside
                // the label's clip. Stacked marks carry their own offsets.
                let (mut x0, mut y0, mut x1, mut y1) = (f32::MAX, f32::MAX, f32::MIN, f32::MIN);
                for run in buffer.layout_runs() {
                    for g in run.glyphs {
                        let f = g.font_size;
                        let ox = left + g.x + f * g.x_offset;
                        let oy = top + run.line_y + g.y - f * g.y_offset;
                        x0 = x0.min(ox - f);
                        x1 = x1.max(ox + g.w + f);
                        y0 = y0.min(oy - 1.5 * f);
                        y1 = y1.max(oy + f);
                    }
                }
                if x1 > x0 {
                    use crate::damage::hash_bytes;
                    let mut hash = hash_bytes(KIND_LABEL, label.text.as_bytes());
                    hash = hash_bytes(hash, label.family.unwrap_or("").as_bytes());
                    hash = hash_bytes(
                        hash,
                        bytemuck::bytes_of(&[
                            label.font_px * scale,
                            label.line_px * scale,
                            label.max_w * scale,
                            left,
                            top,
                        ]),
                    );
                    hash = hash_bytes(
                        hash,
                        bytemuck::bytes_of(&[
                            bounds.left,
                            bounds.top,
                            bounds.right,
                            bounds.bottom,
                            col.0 as i32,
                        ]),
                    );
                    label_marks.push((
                        [
                            x0.max(bounds.left as f32),
                            y0.max(bounds.top as f32),
                            x1.min(bounds.right as f32),
                            y1.min(bounds.bottom as f32),
                        ],
                        hash,
                    ));
                }
            }
            text_buffers.push((buffer, (left, top), bounds, col));
        }

        // Grids before `split` are the base scene (offscreen, blurred behind
        // the box); from `split` on is the box overlay (composited on top).
        let split = scene
            .blur_split
            .unwrap_or(grid_ranges.len())
            .min(grid_ranges.len());

        // What this frame shows, tile by tile, in draw order — and from it
        // what changed since the frame before (see `crate::damage`).
        // `None`: everything.
        let mut damage: Option<Vec<crate::damage::Rect>> = None;
        let mut frame_tiles: Option<crate::damage::TileMap> = None;
        if track_damage {
            use crate::damage::{hash_bytes, mix};
            let mut tiles = std::mem::take(&mut self.damage_cur);
            tiles.reset(w, h);
            let fade = u64::from(alpha.to_bits());
            let deps = FrameDeps {
                fade,
                lights: hash_bytes(
                    fade,
                    bytemuck::bytes_of(&[self.anim_time, cursor_px[0], cursor_px[1]]),
                ),
                neck: scene
                    .neck
                    .filter(|n| n[0] > -9000.0)
                    .map(|n| hash_bytes(fade, bytemuck::bytes_of(&n))),
                icons: mix(
                    hash_bytes(fade, bytemuck::bytes_of(&[squircle, thumb_base as f32])),
                    self.icon_epoch,
                ),
            };
            let gens = &self.icon_layer_gen;
            let grid = |tiles: &mut crate::damage::TileMap,
                        (clip, rect_range, icon_range): &(
                crate::content::Rect,
                std::ops::Range<u32>,
                std::ops::Range<u32>,
            )| {
                let Some(clip) = scissor_of(clip, scale, w, h) else {
                    return;
                };
                for r in &rects[rect_range.start as usize..rect_range.end as usize] {
                    mark_rect(tiles, r, Some(clip), scale, &deps);
                }
                for i in &icons[icon_range.start as usize..icon_range.end as usize] {
                    mark_icon(tiles, i, Some(clip), scale, &deps, gens);
                }
            };
            for s in &shadows[..n_shadows as usize] {
                mark_shadow(&mut tiles, s, scale);
            }
            for r in &rects[..n_rects_unclipped as usize] {
                mark_rect(&mut tiles, r, None, scale, &deps);
            }
            for s in &shadows[n_shadows as usize..] {
                mark_shadow(&mut tiles, s, scale);
            }
            if self.icon_bind.is_some() {
                for i in &icons[..n_icons_unclipped as usize] {
                    mark_icon(&mut tiles, i, None, scale, &deps, gens);
                }
            }
            for g in &grid_ranges[..split] {
                grid(&mut tiles, g);
            }
            // The frosted box shows a blur of everything under it: it
            // depends on the whole base scene.
            if let Some((r, radius)) = scene.box_rect {
                let base = tiles.digest();
                tiles.mark(
                    [
                        r.x * scale,
                        r.y * scale,
                        (r.x + r.w) * scale,
                        (r.y + r.h) * scale,
                    ],
                    mix(
                        hash_bytes(
                            KIND_BACKDROP,
                            bytemuck::bytes_of(&[r.x, r.y, r.w, r.h, radius]),
                        ),
                        base,
                    ),
                );
            }
            for g in &grid_ranges[split..] {
                grid(&mut tiles, g);
            }
            for (b, hash) in &label_marks {
                tiles.mark(*b, *hash);
            }
            for i in &icons[overlay_range.start as usize..overlay_range.end as usize] {
                mark_icon(&mut tiles, i, None, scale, &deps, gens);
            }
            damage = match &self.damage_prev {
                Some(prev) if prev.same_size(&tiles) => Some(tiles.changed(prev)),
                _ => None,
            };
            frame_tiles = Some(tiles);
        }

        // It shows exactly what the last presented frame does: nothing to
        // draw, nothing to present. (The damage check draws it anyway: an
        // unchanged frame must come out pixel for pixel the same.)
        if matches!(&damage, Some(d) if d.is_empty()) && self.damage_check.is_none() {
            crate::perf::DAMAGE_FRAMES.hit();
            crate::perf::DAMAGE_NONE.hit();
            crate::perf::SURFACE_PX.add(u64::from(w) * u64::from(h));
            if let Some(tiles) = frame_tiles {
                self.damage_cur = tiles;
            }
            return Ok(Frame::Unchanged);
        }

        let frame = match self.surface.get_current_texture() {
            Ok(frame) => frame,
            // Timeout is recovered the same way as Lost/Outdated: a fresh
            // swapchain. On AMD (RADV) under Hyprland's explicit sync, the
            // compositor can drop our buffers without signalling their release
            // (seen when a system switch finished, thinkpad 2026-09-29). The
            // first acquire then times out, and every later acquire on that
            // swapchain spins inside Mesa's release wait forever: the main
            // loop is wedged at ~70% of a core, IPC stops answering, install
            // tiles freeze on "Installing…". Reconfiguring drops the stranded
            // images, so the next acquire gets a new one.
            Err(
                e @ (wgpu::SurfaceError::Lost
                | wgpu::SurfaceError::Outdated
                | wgpu::SurfaceError::Timeout),
            ) => {
                if matches!(e, wgpu::SurfaceError::Timeout) {
                    tracing::warn!("swapchain acquire timed out; recreating it");
                }
                self.surface.configure(&self.device, &self.config);
                // A new swapchain: its first image is new as a whole.
                self.damage_prev = None;
                damage = None;
                self.surface
                    .get_current_texture()
                    .context("swapchain unrecoverable after reconfigure")?
            }
            Err(e) => return Err(anyhow!("get_current_texture: {e}")),
        };
        let view = frame
            .texture
            .create_view(&wgpu::TextureViewDescriptor::default());

        self.queue.write_buffer(
            &self.globals_buf,
            0,
            bytemuck::bytes_of(&Globals {
                screen: [lw, lh],
                alpha: scene.alpha.clamp(0.0, 1.0),
                time: self.anim_time,
                cursor: cursor_px,
                squircle,
                thumb_base: thumb_base as f32,
                neck: scene.neck.unwrap_or([-9999.0, 0.0, 0.0, 0.0]),
            }),
        );

        let rect_buf = self
            .device
            .create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: Some("waverunner.rects"),
                contents: bytemuck::cast_slice(&rects),
                usage: wgpu::BufferUsages::VERTEX,
            });
        // `create_buffer_init` rejects empty contents; only build the shadow
        // buffer when there is at least one band to draw.
        let shadow_buf = (!shadows.is_empty()).then(|| {
            self.device
                .create_buffer_init(&wgpu::util::BufferInitDescriptor {
                    label: Some("waverunner.shadows"),
                    contents: bytemuck::cast_slice(&shadows),
                    usage: wgpu::BufferUsages::VERTEX,
                })
        });
        let icon_buf = self
            .device
            .create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: Some("waverunner.icon-instances"),
                contents: bytemuck::cast_slice(&icons),
                usage: wgpu::BufferUsages::VERTEX,
            });
        // One-instance buffer for the box's frosted backdrop (only when a
        // box is open).
        let backdrop_buf = scene.box_rect.map(|(r, radius)| {
            let inst = BoxBackdropInstance {
                rect_min: [r.x, r.y],
                rect_max: [r.x + r.w, r.y + r.h],
                radius,
                screen: [lw, lh],
                _pad: 0.0,
            };
            self.device
                .create_buffer_init(&wgpu::util::BufferInitDescriptor {
                    label: Some("waverunner.box-backdrop"),
                    contents: bytemuck::bytes_of(&inst),
                    usage: wgpu::BufferUsages::VERTEX,
                })
        });

        self.text_viewport.update(
            &self.queue,
            Resolution {
                width: w,
                height: h,
            },
        );
        let areas = text_buffers
            .iter()
            .map(|(buffer, pos, bounds, col)| TextArea {
                buffer,
                left: pos.0,
                top: pos.1,
                scale: 1.0,
                bounds: *bounds,
                default_color: *col,
                custom_glyphs: &[],
            });
        self.text_renderer
            .prepare(
                &self.device,
                &self.queue,
                &mut font_system,
                &mut self.text_atlas,
                &self.text_viewport,
                areas,
                &mut swash,
            )
            .context("glyphon prepare failed")?;

        // The damage check composes the frame into a texture it can read.
        if let Some(check) = &mut self.damage_check {
            let make = || {
                make_check_target(
                    &self.device,
                    self.config.format,
                    w,
                    h,
                    &self.blit_layout,
                    &self.blit_sampler,
                )
            };
            if !matches!(&check.target, Some(t) if t.width == w && t.height == h) {
                check.target = Some(make());
            }
            check.twin = check.paths && backdrop_buf.is_none();
            if check.twin
                && !matches!(&check.two_step, Some((t, ..)) if t.width == w && t.height == h)
            {
                let (_, scene_view, scene_bind) = make_scene_target(
                    &self.device,
                    self.config.format,
                    w,
                    h,
                    &self.blit_layout,
                    &self.blit_sampler,
                );
                check.two_step = Some((make(), scene_view, scene_bind));
            }
        }
        let check_target = self.damage_check.as_ref().and_then(|c| c.target.as_ref());
        let two_step = self
            .damage_check
            .as_ref()
            .filter(|c| c.twin)
            .and_then(|c| c.two_step.as_ref());

        // A frame with a frosted box is drawn in two steps (the base scene
        // offscreen, so the box can sample a blurred copy of it); every other
        // frame goes straight into its target.
        if backdrop_buf.is_none() {
            self.frost = None;
        } else if self.frost.is_none() {
            let make = || {
                make_scene_target(
                    &self.device,
                    self.config.format,
                    w,
                    h,
                    &self.blit_layout,
                    &self.blit_sampler,
                )
            };
            let (_, scene_view, scene_bind) = make();
            let (_, a_view, a_bind) = make();
            let (_, b_view, b_bind) = make();
            self.frost = Some(FrostTargets {
                scene_view,
                scene_bind,
                a_view,
                a_bind,
                b_view,
                b_bind,
            });
        }
        let target = check_target.map_or(&view, |t| &t.view);

        // Scissored grid draws: rects, then icons, per grid.
        let draw_grids = |pass: &mut wgpu::RenderPass<'_>,
                          grids: &[(
            crate::content::Rect,
            std::ops::Range<u32>,
            std::ops::Range<u32>,
        )]| {
            for (clip, rect_range, icon_range) in grids {
                let Some([sx, sy, sw, sh]) = scissor_of(clip, scale, w, h) else {
                    continue;
                };
                pass.set_scissor_rect(sx, sy, sw, sh);
                if !rect_range.is_empty() {
                    pass.set_pipeline(&self.rect_pipeline);
                    pass.set_vertex_buffer(0, rect_buf.slice(..));
                    pass.draw(0..4, rect_range.clone());
                }
                if !icon_range.is_empty() {
                    if let Some(icon_bind) = &self.icon_bind {
                        pass.set_pipeline(&self.icon_pipeline);
                        pass.set_bind_group(1, icon_bind, &[]);
                        pass.set_vertex_buffer(0, icon_buf.slice(..));
                        pass.draw(0..4, icon_range.clone());
                    }
                }
            }
            pass.set_scissor_rect(0, 0, w, h);
        };
        // The base scene: everything a box would frost.
        let draw_base = |pass: &mut wgpu::RenderPass<'_>| {
            pass.set_bind_group(0, &self.globals_bind, &[]);

            // Behind everything: the dock's soft top-edge shadow.
            if let Some(shadow_buf) = &shadow_buf {
                pass.set_pipeline(&self.shadow_pipeline);
                pass.set_vertex_buffer(0, shadow_buf.slice(..));
                pass.draw(0..4, 0..n_shadows);
            }

            // Unclipped: card background + dock hover, then dock icons.
            if n_rects_unclipped > 0 {
                pass.set_pipeline(&self.rect_pipeline);
                pass.set_vertex_buffer(0, rect_buf.slice(..));
                pass.draw(0..4, 0..n_rects_unclipped);
            }
            // Over the fills: neumorphic button shadows.
            if n_overlay_shadows > 0 {
                if let Some(shadow_buf) = &shadow_buf {
                    pass.set_pipeline(&self.shadow_pipeline);
                    pass.set_vertex_buffer(0, shadow_buf.slice(..));
                    pass.draw(0..4, n_shadows..(n_shadows + n_overlay_shadows));
                }
            }
            if n_icons_unclipped > 0 {
                if let Some(icon_bind) = &self.icon_bind {
                    pass.set_pipeline(&self.icon_pipeline);
                    pass.set_bind_group(1, icon_bind, &[]);
                    pass.set_vertex_buffer(0, icon_buf.slice(..));
                    pass.draw(0..4, 0..n_icons_unclipped);
                }
            }

            // Base section grids (everything before the box overlay), each
            // under its own scissor rect.
            draw_grids(pass, &grid_ranges[..split]);
        };
        // What goes over it: the box's panel and members (the grids from
        // `split` on), all text, the drag ghost.
        let draw_overlay = |pass: &mut wgpu::RenderPass<'_>| -> anyhow::Result<()> {
            pass.set_bind_group(0, &self.globals_bind, &[]);
            draw_grids(pass, &grid_ranges[split..]);

            // Text renders unscissored: every TextArea carries its own clip
            // bounds, so labels outside the grid still show.
            if !text_buffers.is_empty() {
                self.text_renderer
                    .render(&self.text_atlas, &self.text_viewport, pass)
                    .context("glyphon render failed")?;
            }

            // Topmost: the drag ghost. Glyphon replaced bind group 0 with
            // its atlas; restore our globals before touching our pipelines.
            if !overlay_range.is_empty() {
                pass.set_bind_group(0, &self.globals_bind, &[]);
                if let Some(icon_bind) = &self.icon_bind {
                    pass.set_pipeline(&self.icon_pipeline);
                    pass.set_bind_group(1, icon_bind, &[]);
                    pass.set_vertex_buffer(0, icon_buf.slice(..));
                    pass.draw(0..4, overlay_range.clone());
                }
            }
            Ok(())
        };

        let mut encoder = self
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("waverunner.frame"),
            });
        match (&self.frost, &backdrop_buf) {
            (Some(frost), Some(backdrop_buf)) => {
                // Pass 1: the base scene into the offscreen texture.
                draw_base(&mut clear_pass(
                    &mut encoder,
                    &frost.scene_view,
                    "waverunner.scene",
                ));

                // Blur passes: scene → a (horizontal) → b (vertical).
                // Separable Gaussian for a smooth frost.
                for (target_view, pipeline, src_bind, label) in [
                    (
                        &frost.a_view,
                        &self.blur_pipeline_h,
                        &frost.scene_bind,
                        "waverunner.blur-h",
                    ),
                    (
                        &frost.b_view,
                        &self.blur_pipeline_v,
                        &frost.a_bind,
                        "waverunner.blur-v",
                    ),
                ] {
                    let mut pass = clear_pass(&mut encoder, target_view, label);
                    pass.set_pipeline(pipeline);
                    pass.set_bind_group(0, src_bind, &[]);
                    pass.draw(0..3, 0..1);
                }

                // Pass 2: the base scene onto the target, the frosted box over
                // it, then the overlay.
                let mut pass = clear_pass(&mut encoder, target, "waverunner.composite");
                pass.set_pipeline(&self.blit_pipeline);
                pass.set_bind_group(0, &frost.scene_bind, &[]);
                pass.draw(0..3, 0..1);

                // Frosted backdrop: erase the box region, then fill it with the
                // blurred (b) scene — together a mix(base, blurred), so the box
                // keeps the base's translucency instead of going opaque.
                pass.set_vertex_buffer(0, backdrop_buf.slice(..));
                pass.set_bind_group(0, &frost.b_bind, &[]);
                pass.set_pipeline(&self.box_erase_pipeline);
                pass.draw(0..4, 0..1);
                pass.set_pipeline(&self.box_backdrop_pipeline);
                pass.draw(0..4, 0..1);

                draw_overlay(&mut pass)?;
            }
            _ => {
                // No box: one pass, straight into the target.
                let mut pass = clear_pass(&mut encoder, target, "waverunner.frame");
                draw_base(&mut pass);
                draw_overlay(&mut pass)?;
            }
        }
        if let Some((twin, scene_view, scene_bind)) = two_step {
            // The same frame, the two-step way, to be compared.
            draw_base(&mut clear_pass(
                &mut encoder,
                scene_view,
                "waverunner.check-scene",
            ));
            {
                let mut pass = clear_pass(&mut encoder, &twin.view, "waverunner.check-composite");
                pass.set_pipeline(&self.blit_pipeline);
                pass.set_bind_group(0, scene_bind, &[]);
                pass.draw(0..3, 0..1);
                draw_overlay(&mut pass)?;
            }
            encoder.copy_texture_to_buffer(
                twin.tex.as_image_copy(),
                wgpu::TexelCopyBufferInfo {
                    buffer: &twin.buf,
                    layout: wgpu::TexelCopyBufferLayout {
                        offset: 0,
                        bytes_per_row: Some(twin.stride),
                        rows_per_image: Some(h),
                    },
                },
                wgpu::Extent3d {
                    width: w,
                    height: h,
                    depth_or_array_layers: 1,
                },
            );
        }

        if let Some(t) = check_target {
            // The checked frame goes to the screen unchanged, and into a
            // buffer to be compared.
            {
                let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                    label: Some("waverunner.damage-check"),
                    color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                        view: &view,
                        resolve_target: None,
                        ops: wgpu::Operations {
                            load: wgpu::LoadOp::Clear(wgpu::Color::TRANSPARENT),
                            store: wgpu::StoreOp::Store,
                        },
                    })],
                    depth_stencil_attachment: None,
                    timestamp_writes: None,
                    occlusion_query_set: None,
                });
                pass.set_pipeline(&self.blit_pipeline);
                pass.set_bind_group(0, &t.bind, &[]);
                pass.draw(0..3, 0..1);
            }
            encoder.copy_texture_to_buffer(
                t.tex.as_image_copy(),
                wgpu::TexelCopyBufferInfo {
                    buffer: &t.buf,
                    layout: wgpu::TexelCopyBufferLayout {
                        offset: 0,
                        bytes_per_row: Some(t.stride),
                        rows_per_image: Some(h),
                    },
                },
                wgpu::Extent3d {
                    width: w,
                    height: h,
                    depth_or_array_layers: 1,
                },
            );
        }

        self.queue.submit(std::iter::once(encoder.finish()));
        if self.pace_by_gpu {
            use std::sync::atomic::Ordering;
            self.gpu_busy.store(true, Ordering::Release);
            let busy = self.gpu_busy.clone();
            self.queue
                .on_submitted_work_done(move || busy.store(false, Ordering::Release));
        }
        if track_damage {
            let area = |r: &crate::damage::Rect| r[2] as u64 * r[3] as u64;
            let full = u64::from(w) * u64::from(h);
            crate::perf::DAMAGE_FRAMES.hit();
            crate::perf::SURFACE_PX.add(full);
            crate::perf::DAMAGE_PX.add(damage.as_ref().map_or(full, |d| d.iter().map(area).sum()));
            if matches!(&damage, Some(d) if d.is_empty()) {
                crate::perf::DAMAGE_NONE.hit();
            }
            crate::perf::DAMAGE_RECTS.add(damage.as_ref().map_or(1, |d| d.len() as u64));
        }
        // Where this frame has anything to show: the compositor draws the
        // surface, and blurs behind it, nowhere else (see `crate::visible`).
        // Set here, with nothing fallible left before the present: its commit
        // carries the region and the frame together.
        if let (Some(visible), Some(tiles)) = (visible.as_deref_mut(), &frame_tiles) {
            visible.set(&tiles.drawn());
        }
        if self.damage_check.is_some() {
            self.check_damage(
                w,
                h,
                damage.as_deref(),
                visible.as_deref().and_then(|v| v.sent()),
            );
        }
        if self.present_damage {
            match damage {
                Some(rects) => wgpu_hal::present_damage::set_next(rects),
                None => wgpu_hal::present_damage::clear(),
            }
        }
        frame.present();
        LAST_PRESENT.with(|t| t.set(Some(now)));
        if let Some(tiles) = frame_tiles {
            if let Some(old) = self.damage_prev.replace(tiles) {
                self.damage_cur = old;
            }
        }
        self.text_atlas.trim();
        Ok(Frame::Presented)
    }

    /// `WAVERUNNER_DAMAGE_CHECK`: read the frame just submitted back and
    /// compare it with the one before; every pixel that differs must lie in
    /// `damage` (`None` = the whole surface), and every pixel that is not
    /// transparent in `visible` (`None` = no region was given).
    fn check_damage(
        &mut self,
        w: u32,
        h: u32,
        damage: Option<&[crate::damage::Rect]>,
        visible: Option<&[crate::damage::Rect]>,
    ) {
        let Some(check) = &mut self.damage_check else {
            return;
        };
        let Some(t) = &check.target else {
            return;
        };
        let slice = t.buf.slice(..);
        let mapped = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let done = mapped.clone();
        slice.map_async(wgpu::MapMode::Read, move |r| {
            done.store(r.is_ok(), std::sync::atomic::Ordering::Release);
        });
        let _ = self.device.poll(wgpu::Maintain::Wait);
        if !mapped.load(std::sync::atomic::Ordering::Acquire) {
            tracing::warn!("damage check: the frame could not be read back");
            return;
        }
        let row = w as usize * 4;
        let stride = t.stride as usize;
        {
            let data = slice.get_mapped_range();
            crate::perf::DAMAGE_CHECKED.hit();
            // The two-step twin of a frame drawn straight into its target.
            if let Some((twin, ..)) = check.two_step.as_ref().filter(|_| check.twin) {
                let twin_slice = twin.buf.slice(..);
                let ok = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
                let done = ok.clone();
                twin_slice.map_async(wgpu::MapMode::Read, move |r| {
                    done.store(r.is_ok(), std::sync::atomic::Ordering::Release);
                });
                let _ = self.device.poll(wgpu::Maintain::Wait);
                if ok.load(std::sync::atomic::Ordering::Acquire) {
                    {
                        let other = twin_slice.get_mapped_range();
                        let differ = (0..h as usize)
                            .map(|y| {
                                let a = &data[y * stride..y * stride + row];
                                let b = &other[y * stride..y * stride + row];
                                if a == b {
                                    0
                                } else {
                                    a.chunks_exact(4)
                                        .zip(b.chunks_exact(4))
                                        .filter(|(p, q)| p != q)
                                        .count() as u64
                                }
                            })
                            .sum::<u64>();
                        crate::perf::PATHS_CHECKED.hit();
                        if differ > 0 {
                            crate::perf::PATHS_DIFFER.hit();
                            tracing::warn!(
                                "path check: frame {} ({w}x{h}): {differ} px differ between the one-pass and the two-step frame",
                                self.frame_no
                            );
                        }
                    }
                    twin.buf.unmap();
                }
            }
            if check.prev_size == (w, h) && check.prev.len() == row * h as usize {
                // Changed pixels outside the damage: how many, and their box.
                let (mut wrong, mut changed) = (0u64, 0u64);
                let (mut x0, mut y0, mut x1, mut y1) = (i32::MAX, i32::MAX, -1, -1);
                for y in 0..h as usize {
                    let cur = &data[y * stride..y * stride + row];
                    let old = &check.prev[y * row..(y + 1) * row];
                    if cur == old {
                        continue;
                    }
                    for x in 0..w as usize {
                        if cur[x * 4..x * 4 + 4] == old[x * 4..x * 4 + 4] {
                            continue;
                        }
                        changed += 1;
                        let (px, py) = (x as i32, y as i32);
                        if damage.is_some_and(|d| !crate::damage::covers(d, px, py)) {
                            wrong += 1;
                            x0 = x0.min(px);
                            y0 = y0.min(py);
                            x1 = x1.max(px);
                            y1 = y1.max(py);
                        }
                    }
                }
                if wrong > 0 {
                    crate::perf::DAMAGE_MISSED.hit();
                    crate::perf::DAMAGE_MISSED_PX.add(wrong);
                    tracing::warn!(
                        "damage check: frame {} ({w}x{h}): {wrong} of {changed} changed px lie outside the damage, within x {x0}..={x1} y {y0}..={y1}; damage {:?}",
                        self.frame_no,
                        damage
                    );
                }
            }
            if let Some(visible) = visible {
                // Drawn pixels the compositor was told nothing is at.
                let mut outside = 0u64;
                for y in 0..h as usize {
                    let cur = &data[y * stride..y * stride + row];
                    if cur.iter().all(|b| *b == 0) {
                        continue;
                    }
                    for (x, px) in cur.chunks_exact(4).enumerate() {
                        if px != [0, 0, 0, 0] && !crate::damage::covers(visible, x as i32, y as i32)
                        {
                            outside += 1;
                        }
                    }
                }
                if outside > 0 {
                    crate::perf::VISIBLE_MISSED.hit();
                    tracing::warn!(
                        "visible-region check: frame {} ({w}x{h}): {outside} drawn px lie outside the region {:?}",
                        self.frame_no,
                        visible
                    );
                }
            }
            check.prev.clear();
            check.prev.reserve(row * h as usize);
            for y in 0..h as usize {
                check
                    .prev
                    .extend_from_slice(&data[y * stride..y * stride + row]);
            }
            check.prev_size = (w, h);
        }
        t.buf.unmap();
    }
}

/// Upload one icon's full mip chain (`ICON_CHAIN_BYTES` of base followed
/// by each downsample) into `layer` of the array texture — one
/// `write_texture` per mip level. Chains are produced by
/// [`crate::apps::with_mips`], so the levels are contiguous and match the
/// texture's `ICON_MIPS`.
/// Layers an icon array grows by when a reserved layer is first needed.
const ICON_GROW_STEP: u32 = 16;

/// The layer count to ALLOCATE for `count` icons, around two guesses of
/// wgpu's GLES backend, which cannot see our explicit D2Array view dimension
/// and guesses the texture's kind from its layer count:
/// - ONE layer is a plain 2D texture, and the `texture_2d_array` sampler
///   reads black — the OPTIONS bar's array with a single notification avatar
///   showed a black square on the GL machines (MacBook, 2026-10-04);
/// - 6 layers is a Cube, and a multiple of 6 above it a CubeArray — the
///   transient "app icons go black" bug on the GL-backend Haswell (Golem
///   #42; the count shifts with app/pending counts, so icons rendered fine
///   until a rescan hit a multiple of 6).
///
/// A pad layer dodges each; harmless on Vulkan.
fn icon_layers(count: usize) -> u32 {
    let layers = count.max(2) as u32;
    if layers == 6 || (layers > 6 && layers.is_multiple_of(6)) {
        layers + 1
    } else {
        layers
    }
}

fn write_icon_chain(queue: &wgpu::Queue, texture: &wgpu::Texture, layer: u32, chain: &[u8]) {
    let mut offset = 0usize;
    let mut size = ICON_SIZE;
    for mip in 0..ICON_MIPS {
        let len = (size * size * 4) as usize;
        queue.write_texture(
            wgpu::TexelCopyTextureInfo {
                texture,
                mip_level: mip,
                origin: wgpu::Origin3d {
                    x: 0,
                    y: 0,
                    z: layer,
                },
                aspect: wgpu::TextureAspect::All,
            },
            &chain[offset..offset + len],
            wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(size * 4),
                rows_per_image: Some(size),
            },
            wgpu::Extent3d {
                width: size,
                height: size,
                depth_or_array_layers: 1,
            },
        );
        offset += len;
        size /= 2;
    }
}

fn shadow_instance(s: &crate::content::ShadowInst) -> ShadowInstance {
    ShadowInstance {
        rect_min: [s.rect.x, s.rect.y],
        rect_max: [s.rect.x + s.rect.w, s.rect.y + s.rect.h],
        color: s.color,
        radius: s.radius,
        blur: s.blur,
        edges: s.edges,
    }
}

fn rect_instance(r: &crate::content::RectInst) -> RectInstance {
    RectInstance {
        rect_min: [r.rect.x, r.rect.y],
        rect_max: [r.rect.x + r.rect.w, r.rect.y + r.rect.h],
        color: r.color,
        radius: r.radius,
        glass: r.glass,
        border: r.border,
        _pad: 0.0,
    }
}

fn icon_instance(i: &crate::content::IconInst) -> IconInstance {
    IconInstance {
        rect_min: [i.rect.x, i.rect.y],
        rect_max: [i.rect.x + i.rect.w, i.rect.y + i.rect.h],
        layer: i.layer,
        tint: i.tint,
        ring: i.ring,
        plate: i.plate,
    }
}

/// Resolve a `Label::family` into a glyphon family + weight.
///
/// [`crate::content::FONT_BOLD`] is a sentinel, not a real family name: it
/// means "the default sans, bold" (see its docs for why weight rides this
/// field). Everything else is a literal family name.
fn resolve_family(family: Option<&str>) -> (Family<'_>, Weight) {
    match family {
        Some(f) if f == crate::content::FONT_BOLD => (Family::SansSerif, Weight::BOLD),
        Some(name) => (Family::Name(name), Weight::NORMAL),
        None => (Family::SansSerif, Weight::NORMAL),
    }
}

/// Cache key for a volatile label: everything its shaping depends on but the
/// width bound (see [`VolatileLabel`]).
fn volatile_key(label: &crate::content::Label) -> String {
    format!(
        "{}\u{1}{}\u{1}{}\u{1}{}",
        label.text,
        label.font_px.to_bits(),
        label.line_px.to_bits(),
        label.family.unwrap_or("")
    )
}

/// Cache key for a shaped label. Includes the FAMILY as well as the text and
/// size: the same string shaped sans vs Nerd vs bold is three different
/// buffers, and keying on text alone silently served whichever was shaped
/// first (bold hover made that visible).
fn label_key(label: &crate::content::Label) -> String {
    format!(
        "{}\u{1}{}\u{1}{}",
        label.text,
        label.font_px,
        label.family.unwrap_or("")
    )
}

/// Never wait on the compositor inside present.
///
/// The GL path (the MacBook's crocus, and any machine the ladder drops to GL)
/// presents with eglSwapBuffers. At EGL's default swap interval of 1, Mesa
/// blocks there until the compositor's frame callback for the previous frame
/// arrives. Every frame is already paced by frame callbacks (`frame()` only
/// draws on one), so this second wait only stalls the event loop. During the
/// box's open/close the loop sat in present for 0.5-1.7 s at a time,
/// Super+Space presses queued behind it and replayed for seconds (Max,
/// 2026-09-29: "the dock gets stuck").
///
/// Mesa takes its default interval from `vblank_mode` when the EGL display
/// initializes, which happens while `Renderer::new` builds the surface. So it
/// is set for exactly that span and removed on drop, on every path: apps the
/// dock launches must never inherit it. Vulkan ignores it, and a value the
/// user set is left alone.
struct NoVblankWait(bool);

impl NoVblankWait {
    fn arm() -> Self {
        let armed = std::env::var_os("vblank_mode").is_none();
        if armed {
            std::env::set_var("vblank_mode", "0");
        }
        Self(armed)
    }
}

impl Drop for NoVblankWait {
    fn drop(&mut self) {
        if self.0 {
            std::env::remove_var("vblank_mode");
        }
    }
}
