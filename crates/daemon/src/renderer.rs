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
    // Up to 4 simultaneous ripples: (x, y, age, 0); x < -9000 = inactive.
    ripples: [[f32; 4]; 4],
    // Up to 2 box-open/close waves: (cx, cy, age, 0); cx < -9000 = inactive.
    box_waves: [[f32; 4]; 2],
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
    // draws the icon. Offset 36 → the struct is exactly 40 bytes, Pod-clean.
    ring: f32,
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

pub struct Renderer {
    surface: wgpu::Surface<'static>,
    device: wgpu::Device,
    queue: wgpu::Queue,
    config: wgpu::SurfaceConfiguration,
    /// The adapter renders on the CPU (llvmpipe & friends): every frame
    /// costs real cores, so sustained animations must be throttled (F12).
    software: bool,
    /// Integer supersampling factor. `config.width/height` are physical
    /// (`logical × scale`); geometry is authored in logical px and scaled
    /// up automatically (see [`Renderer::render`]).
    scale: u32,

    globals_buf: wgpu::Buffer,
    globals_bind: wgpu::BindGroup,
    shadow_pipeline: wgpu::RenderPipeline,
    rect_pipeline: wgpu::RenderPipeline,

    /// Offscreen colour target the scene is rendered into, then blitted to
    /// the swapchain — so the box can sample a blurred copy of it (frosted
    /// glass). Same size/format as the swapchain; rebuilt on resize.
    scene_tex: wgpu::Texture,
    scene_view: wgpu::TextureView,
    /// Fullscreen textured-quad pipeline (copies `scene_tex` to the screen).
    blit_pipeline: wgpu::RenderPipeline,
    blit_layout: wgpu::BindGroupLayout,
    blit_sampler: wgpu::Sampler,
    /// Bind group over `scene_view` for the blit; rebuilt on resize.
    blit_bind: wgpu::BindGroup,
    /// Frosted-glass backdrop for the open box: samples the blurred scene
    /// over the box region (shares `blit_layout`).
    box_backdrop_pipeline: wgpu::RenderPipeline,
    /// Clears the box region to transparent (× (1−coverage)) before the
    /// backdrop fill, so the frost replaces the base instead of stacking.
    box_erase_pipeline: wgpu::RenderPipeline,
    /// Separable-Gaussian blur ping-pong: scene → `blur_a` (horizontal) →
    /// `blur_b` (vertical); the box backdrop samples `blur_b`. Rebuilt on
    /// resize.
    blur_a_tex: wgpu::Texture,
    blur_a_view: wgpu::TextureView,
    blur_a_bind: wgpu::BindGroup,
    blur_b_tex: wgpu::Texture,
    blur_b_view: wgpu::TextureView,
    blur_b_bind: wgpu::BindGroup,
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

    font_system: FontSystem,
    swash: SwashCache,
    text_viewport: Viewport,
    text_atlas: TextAtlas,
    text_renderer: TextRenderer,
    /// Shaped label buffers, keyed by label text; invalidated when a
    /// new app set arrives via [`Renderer::set_icons`].
    label_cache: std::collections::HashMap<String, TextBuffer>,
    /// Accumulated render time — only advances while frames are drawn, so
    /// there are no phase jumps when the dock hides and reappears.
    anim_time: f32,
    last_render: Option<std::time::Instant>,
    /// Active ripples: (surface position, anim_time at spawn).
    ripples: Vec<([f32; 2], f32)>,
    /// Active box open/close waves: (icon center, anim_time at spawn).
    box_waves: Vec<([f32; 2], f32)>,
}

/// Build the offscreen scene colour target (texture + view + blit bind
/// group) at `width`×`height`. Called at init and on every resize.
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

impl Renderer {
    /// Create the wgpu device and configure the swapchain against an
    /// already-configured layer surface of `width` x `height` physical
    /// (buffer) pixels. `scale` is the integer supersampling factor:
    /// physical = logical × scale.
    pub fn new(
        conn: &Connection,
        wl_surface: &WlSurface,
        width: u32,
        height: u32,
        scale: u32,
    ) -> anyhow::Result<Self> {
        let display = NonNull::new(conn.backend().display_ptr().cast())
            .ok_or_else(|| anyhow!("null wl_display"))?;
        let window = NonNull::new(wl_surface.id().as_ptr().cast())
            .ok_or_else(|| anyhow!("null wl_surface"))?;

        // A LADDER, not a single attempt (F8): the machine that cannot
        // present through its Vulkan path must still get a shell. Venus
        // (Vulkan passthrough in the VM) gave us a surface with NO adapter,
        // and one failed attempt used to be fatal. Each rung is tried in
        // turn and the reason for the previous one is logged.
        const ATTEMPTS: [(wgpu::Backends, bool, &str); 3] = [
            // What we want: a real GPU on Vulkan (or GL).
            (
                wgpu::Backends::from_bits_truncate(
                    wgpu::Backends::VULKAN.bits() | wgpu::Backends::GL.bits(),
                ),
                false,
                "gpu",
            ),
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
                    raw_window_handle: RawWindowHandle::Wayland(WaylandWindowHandle::new(window)),
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
        let software = adapter.get_info().device_type == wgpu::DeviceType::Cpu;

        let (device, queue) = pollster::block_on(adapter.request_device(
            &wgpu::DeviceDescriptor {
                label: Some("waverunner"),
                required_features: wgpu::Features::empty(),
                required_limits: wgpu::Limits {
                    // Allow surface expansion to full screen height (>2048 on 4K displays).
                    max_texture_dimension_2d: 8192,
                    // The icon array is one texture layer per app icon plus a
                    // reserved block (rank hits + pending installs + thumbs =
                    // 97). downlevel_defaults() caps texture_array_layers at
                    // 256, so a machine with ~160+ .desktop entries overflowed
                    // it — create_texture("waverunner.icons") panicked the
                    // daemon on cold start (267 layers on a 170-app machine,
                    // 2026-09-01). Request what the adapter actually offers
                    // (2048 on any real GPU, incl. this Iris Xe / RTX 4050);
                    // this never exceeds hardware, so device creation is safe.
                    max_texture_array_layers: adapter.limits().max_texture_array_layers,
                    ..wgpu::Limits::downlevel_defaults()
                },
                memory_hints: wgpu::MemoryHints::default(),
            },
            None,
        ))
        .context("wgpu device request failed")?;

        let caps = surface.get_capabilities(&adapter);
        // Transparency requires a premultiplied compositing mode.
        let alpha_mode = if caps
            .alpha_modes
            .contains(&wgpu::CompositeAlphaMode::PreMultiplied)
        {
            wgpu::CompositeAlphaMode::PreMultiplied
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
        let globals_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
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
        let globals_bind = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("waverunner.globals"),
            layout: &globals_layout,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: globals_buf.as_entire_binding(),
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
            source: wgpu::ShaderSource::Wgsl(include_str!("shaders/edge_shadow.wgsl").into()),
        });
        let shadow_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("waverunner.shadow"),
            bind_group_layouts: &[&globals_layout],
            push_constant_ranges: &[],
        });
        let shadow_pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
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
            source: wgpu::ShaderSource::Wgsl(include_str!("shaders/rounded_rect.wgsl").into()),
        });
        let rect_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("waverunner.rect"),
            bind_group_layouts: &[&globals_layout],
            push_constant_ranges: &[],
        });
        let rect_pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
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
        let icon_bind_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("waverunner.icons"),
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Float { filterable: true },
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
                        0 => Float32x2, 1 => Float32x2, 2 => Uint32, 3 => Float32x4, 4 => Float32
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
        let blit_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("waverunner.blit"),
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Float { filterable: true },
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
        let blit_pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
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
        let (scene_tex, scene_view, blit_bind) = make_scene_target(
            &device,
            config.format,
            width,
            height,
            &blit_layout,
            &blit_sampler,
        );

        // Box frosted backdrop: samples/blurs the scene texture over the box
        // region, premultiplied "over" blend (same as the scene pipelines).
        let backdrop_shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("waverunner.box-backdrop"),
            source: wgpu::ShaderSource::Wgsl(include_str!("shaders/box_backdrop.wgsl").into()),
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
        let box_erase_pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
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

        // Separable-Gaussian blur ping-pong targets (frost the box backdrop).
        let (blur_a_tex, blur_a_view, blur_a_bind) = make_scene_target(
            &device,
            config.format,
            width,
            height,
            &blit_layout,
            &blit_sampler,
        );
        let (blur_b_tex, blur_b_view, blur_b_bind) = make_scene_target(
            &device,
            config.format,
            width,
            height,
            &blit_layout,
            &blit_sampler,
        );
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

        // Text stack (glyphon).
        let font_system = FontSystem::new();
        let swash = SwashCache::new();
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
            software,
            scale: scale.max(1),
            globals_buf,
            globals_bind,
            shadow_pipeline,
            rect_pipeline,
            scene_tex,
            scene_view,
            blit_pipeline,
            blit_layout,
            blit_sampler,
            blit_bind,
            box_backdrop_pipeline,
            box_erase_pipeline,
            blur_a_tex,
            blur_a_view,
            blur_a_bind,
            blur_b_tex,
            blur_b_view,
            blur_b_bind,
            blur_pipeline_h,
            blur_pipeline_v,
            icon_pipeline,
            icon_bind_layout,
            icon_sampler,
            icon_bind: None,
            icon_texture: None,
            icon_layer_count: 0,
            font_system,
            swash,
            text_viewport,
            text_atlas,
            text_renderer,
            label_cache: std::collections::HashMap::new(),
            anim_time: 0.0,
            last_render: None,
            ripples: Vec::new(),
            box_waves: Vec::new(),
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
        let rebuild = |dev: &wgpu::Device| {
            make_scene_target(
                dev,
                self.config.format,
                width,
                height,
                &self.blit_layout,
                &self.blit_sampler,
            )
        };
        let (tex, view, bind) = rebuild(&self.device);
        self.scene_tex = tex;
        self.scene_view = view;
        self.blit_bind = bind;
        let (tex, view, bind) = rebuild(&self.device);
        self.blur_a_tex = tex;
        self.blur_a_view = view;
        self.blur_a_bind = bind;
        let (tex, view, bind) = rebuild(&self.device);
        self.blur_b_tex = tex;
        self.blur_b_view = view;
        self.blur_b_bind = bind;
    }

    /// Upload the icon texture array delivered by the indexer thread.
    /// `icons` holds one premultiplied RGBA8 `ICON_SIZE`² image per app.
    /// `RANK_HITS_MAX` + `PENDING_INSTALL_CAP` extra layers are reserved
    /// past the end: the first block for the dynamic package-search icons,
    /// the second for packages installing in the grid
    /// ([`Renderer::update_icon_layer`]).
    pub fn set_icons(&mut self, icons: &[Vec<u8>]) {
        // New app set: previously shaped labels may be stale.
        self.label_cache.clear();
        let reserved =
            crate::nix::RANK_HITS_MAX + crate::nix::PENDING_INSTALL_CAP + crate::thumbs::THUMB_CAP;
        self.upload_icon_array(icons.len(), reserved, icons.iter());
    }

    /// Populate the OPTIONS surface's icon array. The topbar renderer is a
    /// separate instance from the dock's, so this array is entirely its own:
    /// the notification card avatars occupy layers `[0, notif.len())` and the
    /// clipboard thumbnails follow at `[notif.len(), notif.len() + clip.len())`,
    /// each addressed by [`IconInst::layer`]. Re-uploaded wholesale whenever
    /// either set changes (see `App::upload_options_icons`).
    pub fn set_options_icons(&mut self, notif: &[Vec<u8>], clip: &[Vec<u8>]) {
        self.upload_icon_array(notif.len() + clip.len(), 0, notif.iter().chain(clip.iter()));
    }

    /// Shared core: (re)allocate the icon texture array with `count + reserved`
    /// layers, write each chain to its layer, and rebuild the sampler bind group.
    fn upload_icon_array<'a>(
        &mut self,
        count: usize,
        reserved: usize,
        chains: impl Iterator<Item = &'a Vec<u8>>,
    ) {
        let layers = (count + reserved).max(1) as u32;
        let texture = self.device.create_texture(&wgpu::TextureDescriptor {
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
            usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
            view_formats: &[],
        });
        for (i, chain) in chains.enumerate() {
            write_icon_chain(&self.queue, &texture, i as u32, chain);
        }
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

    /// Width in pixels of `text` shaped at `font_px` — the same family
    /// and shaping the labels render with, so the search caret can sit
    /// exactly after the glyphs instead of guessing from char counts.
    /// Whether frames are rendered on the CPU (see the `software` field).
    pub fn is_software(&self) -> bool {
        self.software
    }

    pub fn measure_text(&mut self, text: &str, font_px: f32, family: Option<&str>) -> f32 {
        if text.is_empty() {
            return 0.0;
        }
        let mut buffer =
            TextBuffer::new(&mut self.font_system, Metrics::new(font_px, font_px * 1.3));
        let (fam, weight) = resolve_family(family);
        buffer.set_text(
            &mut self.font_system,
            text,
            Attrs::new().family(fam).weight(weight),
            Shaping::Advanced,
        );
        buffer.shape_until_scroll(&mut self.font_system, false);
        buffer
            .layout_runs()
            .map(|run| run.line_w)
            .fold(0.0, f32::max)
    }

    /// Overwrite one icon texture-array layer (a dynamic package icon in
    /// the reserved tail of the array). Out-of-range layers and missing
    /// textures are ignored — a rescan re-uploads shortly anyway.
    pub fn update_icon_layer(&mut self, layer: u32, pixels: &[u8]) {
        let Some(texture) = &self.icon_texture else {
            return;
        };
        if layer >= self.icon_layer_count || pixels.len() != ICON_CHAIN_BYTES {
            return;
        }
        write_icon_chain(&self.queue, texture, layer, pixels);
    }

    /// Spawn a ripple at the given surface position. Under reduce-motion
    /// no ripple spawns — the click's effect is the feedback.
    pub fn record_click(&mut self, x: f32, y: f32) {
        if crate::animation::reduce_motion() {
            return;
        }
        self.ripples.push(([x, y], self.anim_time));
    }

    /// True while any ripple is still animating.
    pub fn has_active_ripple(&self) -> bool {
        !self.ripples.is_empty()
    }

    /// Spawn a box open/close wave centred on the icon at (x, y). Skipped
    /// under reduce-motion, like the click ripple.
    pub fn record_box_wave(&mut self, x: f32, y: f32) {
        if crate::animation::reduce_motion() {
            return;
        }
        self.box_waves.push(([x, y], self.anim_time));
    }

    /// True while any box wave is still animating.
    pub fn has_active_box_wave(&self) -> bool {
        !self.box_waves.is_empty()
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
    ) -> anyhow::Result<()> {
        let frame = match self.surface.get_current_texture() {
            Ok(frame) => frame,
            Err(wgpu::SurfaceError::Lost | wgpu::SurfaceError::Outdated) => {
                self.surface.configure(&self.device, &self.config);
                self.surface
                    .get_current_texture()
                    .context("swapchain unrecoverable after reconfigure")?
            }
            Err(e) => return Err(anyhow!("get_current_texture: {e}")),
        };
        let view = frame
            .texture
            .create_view(&wgpu::TextureViewDescriptor::default());
        // `w`/`h` are the physical framebuffer size; the scene is authored in
        // logical px. Geometry pipelines map `px / screen → NDC`, so feeding a
        // *logical* `screen` while the framebuffer is physical scales all
        // geometry up to physical resolution for free — no shader changes.
        // Text (glyphon) is the exception: it must be shaped at physical px to
        // stay crisp, so its metrics, positions and clip bounds are scaled by
        // `scale` below.
        let (w, h) = (self.config.width, self.config.height);
        let scale = self.scale as f32;
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

        // Expire ripples older than 3.5 s; box waves older than 1.0 s.
        self.ripples.retain(|(_, t)| self.anim_time - t <= 3.5);
        self.box_waves.retain(|(_, t)| self.anim_time - t <= 1.0);

        let cursor_px = cursor.map(|(x, y)| [x, y]).unwrap_or([-9999.0, -9999.0]);

        let inactive = [-9999.0_f32, -9999.0, 999.0, 0.0];
        let mut ripples = [inactive; 4];
        for (slot, (pos, t)) in self.ripples.iter().rev().take(4).enumerate() {
            ripples[slot] = [pos[0], pos[1], self.anim_time - t, 0.0];
        }
        let mut box_waves = [inactive; 2];
        for (slot, (pos, t)) in self.box_waves.iter().rev().take(2).enumerate() {
            box_waves[slot] = [pos[0], pos[1], self.anim_time - t, 0.0];
        }

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
                ripples,
                box_waves,
            }),
        );

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
        let mut fresh: Vec<TextBuffer> = Vec::new();
        let mut fresh_of: Vec<Option<usize>> = Vec::with_capacity(all_labels.len());
        for (label, _) in &all_labels {
            if label.cache {
                let key = label_key(label);
                if !self.label_cache.contains_key(&key) {
                    let buffer = shape(&mut self.font_system, label);
                    self.label_cache.insert(key, buffer);
                }
                fresh_of.push(None);
            } else {
                fresh.push(shape(&mut self.font_system, label));
                fresh_of.push(Some(fresh.len() - 1));
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
        for (i, (label, clip)) in all_labels.iter().enumerate() {
            let buffer = match fresh_of[i] {
                Some(fi) => &fresh[fi],
                None => {
                    let key = label_key(label);
                    match self.label_cache.get(&key) {
                        Some(buffer) => buffer,
                        None => continue,
                    }
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
            text_buffers.push((buffer, (left, top), bounds, col));
        }
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
                &mut self.font_system,
                &mut self.text_atlas,
                &self.text_viewport,
                areas,
                &mut self.swash,
            )
            .context("glyphon prepare failed")?;

        // Grids before `split` are the base scene (offscreen, blurred behind
        // the box); from `split` on is the box overlay (composited on top).
        let split = scene
            .blur_split
            .unwrap_or(grid_ranges.len())
            .min(grid_ranges.len());

        let mut encoder = self
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("waverunner.frame"),
            });
        {
            // Pass 1: the whole scene into the offscreen texture (so the
            // box can later sample a blurred copy of it).
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("waverunner.scene"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &self.scene_view,
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
            // under its own scissor rect. The box overlay (grids from
            // `split` on) is held back for pass 2 so it draws over the blur.
            for (clip, rect_range, icon_range) in &grid_ranges[..split] {
                // Scissor rects address the physical framebuffer; the clip is
                // logical, so scale it up.
                let sx = ((clip.x.max(0.0) * scale) as u32).min(w);
                let sy = ((clip.y.max(0.0) * scale) as u32).min(h);
                let sw = ((clip.w * scale) as u32).min(w - sx);
                let sh = (((clip.y + clip.h) * scale).min(h as f32) as u32).saturating_sub(sy);
                if sw == 0 || sh == 0 {
                    continue;
                }
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
        }

        // Blur passes (only when a box is open): scene → blur_a (horizontal)
        // → blur_b (vertical). Separable Gaussian for a smooth frost.
        if backdrop_buf.is_some() {
            for (target_view, pipeline, src_bind, label) in [
                (
                    &self.blur_a_view,
                    &self.blur_pipeline_h,
                    &self.blit_bind,
                    "waverunner.blur-h",
                ),
                (
                    &self.blur_b_view,
                    &self.blur_pipeline_v,
                    &self.blur_a_bind,
                    "waverunner.blur-v",
                ),
            ] {
                let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                    label: Some(label),
                    color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                        view: target_view,
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
                pass.set_pipeline(pipeline);
                pass.set_bind_group(0, src_bind, &[]);
                pass.draw(0..3, 0..1);
            }
        }

        {
            // Pass 2: blit the offscreen base scene onto the swapchain, then
            // draw the box overlay (and all text + the drag ghost) over it.
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("waverunner.composite"),
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
            pass.set_bind_group(0, &self.blit_bind, &[]);
            pass.draw(0..3, 0..1);

            // Frosted backdrop: erase the box region, then fill it with the
            // blurred (blur_b) scene — together a mix(base, blurred), so the
            // box keeps the base's translucency instead of going opaque.
            if let Some(backdrop_buf) = &backdrop_buf {
                pass.set_vertex_buffer(0, backdrop_buf.slice(..));
                pass.set_bind_group(0, &self.blur_b_bind, &[]);
                pass.set_pipeline(&self.box_erase_pipeline);
                pass.draw(0..4, 0..1);
                pass.set_pipeline(&self.box_backdrop_pipeline);
                pass.draw(0..4, 0..1);
            }

            // The box overlay: panel + members, over the frosted backdrop.
            // Same scissored grid draw as the base grids.
            pass.set_bind_group(0, &self.globals_bind, &[]);
            for (clip, rect_range, icon_range) in &grid_ranges[split..] {
                let sx = ((clip.x.max(0.0) * scale) as u32).min(w);
                let sy = ((clip.y.max(0.0) * scale) as u32).min(h);
                let sw = ((clip.w * scale) as u32).min(w - sx);
                let sh = (((clip.y + clip.h) * scale).min(h as f32) as u32).saturating_sub(sy);
                if sw == 0 || sh == 0 {
                    continue;
                }
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

            // Text renders unscissored: every TextArea carries its own clip
            // bounds, so labels outside the grid still show.
            if !text_buffers.is_empty() {
                self.text_renderer
                    .render(&self.text_atlas, &self.text_viewport, &mut pass)
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
        }

        self.queue.submit(std::iter::once(encoder.finish()));
        frame.present();
        self.text_atlas.trim();
        Ok(())
    }
}

/// Upload one icon's full mip chain (`ICON_CHAIN_BYTES` of base followed
/// by each downsample) into `layer` of the array texture — one
/// `write_texture` per mip level. Chains are produced by
/// [`crate::apps::with_mips`], so the levels are contiguous and match the
/// texture's `ICON_MIPS`.
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
