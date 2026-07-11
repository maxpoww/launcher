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
    Shaping, SwashCache, TextArea, TextAtlas, TextBounds, TextRenderer, Viewport,
};
use raw_window_handle::{
    RawDisplayHandle, RawWindowHandle, WaylandDisplayHandle, WaylandWindowHandle,
};
use wayland_client::protocol::wl_surface::WlSurface;
use wayland_client::{Connection, Proxy};
use wgpu::util::DeviceExt;

use crate::apps::ICON_SIZE;
use crate::content::Scene;

/// Global uniforms shared by the rect and icon pipelines.
#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
struct Globals {
    screen: [f32; 2],
    alpha: f32,
    _pad: f32,
}

/// Per-instance data for one rounded rectangle.
#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
struct RectInstance {
    rect_min: [f32; 2],
    rect_max: [f32; 2],
    color: [f32; 4],
    radius: f32,
    _pad: [f32; 3],
}

/// Per-instance data for one icon quad.
#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
struct IconInstance {
    rect_min: [f32; 2],
    rect_max: [f32; 2],
    layer: u32,
    _pad: [u32; 3],
}

pub struct Renderer {
    surface: wgpu::Surface<'static>,
    device: wgpu::Device,
    queue: wgpu::Queue,
    config: wgpu::SurfaceConfiguration,

    globals_buf: wgpu::Buffer,
    globals_bind: wgpu::BindGroup,
    rect_pipeline: wgpu::RenderPipeline,

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
}

impl Renderer {
    /// Create the wgpu device and configure the swapchain against an
    /// already-configured layer surface of `width` x `height` (buffer
    /// pixels).
    pub fn new(
        conn: &Connection,
        wl_surface: &WlSurface,
        width: u32,
        height: u32,
    ) -> anyhow::Result<Self> {
        let instance = wgpu::Instance::new(&wgpu::InstanceDescriptor {
            backends: wgpu::Backends::VULKAN | wgpu::Backends::GL,
            ..Default::default()
        });

        let display = NonNull::new(conn.backend().display_ptr().cast())
            .ok_or_else(|| anyhow!("null wl_display"))?;
        let window = NonNull::new(wl_surface.id().as_ptr().cast())
            .ok_or_else(|| anyhow!("null wl_surface"))?;
        // SAFETY: both handles point at live Wayland objects owned by App,
        // which outlives the renderer and drops it first.
        let surface = unsafe {
            instance.create_surface_unsafe(wgpu::SurfaceTargetUnsafe::RawHandle {
                raw_display_handle: RawDisplayHandle::Wayland(WaylandDisplayHandle::new(display)),
                raw_window_handle: RawWindowHandle::Wayland(WaylandWindowHandle::new(window)),
            })
        }
        .context("wgpu surface creation failed")?;

        let adapter = pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions {
            power_preference: wgpu::PowerPreference::LowPower,
            compatible_surface: Some(&surface),
            force_fallback_adapter: false,
        }))
        .ok_or_else(|| anyhow!("no suitable GPU adapter (is vulkan-loader on LD_LIBRARY_PATH?)"))?;
        tracing::info!("using adapter: {:?}", adapter.get_info());

        let (device, queue) = pollster::block_on(adapter.request_device(
            &wgpu::DeviceDescriptor {
                label: Some("waverunner"),
                required_features: wgpu::Features::empty(),
                required_limits: wgpu::Limits::downlevel_defaults(),
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
                        0 => Float32x2, 1 => Float32x2, 2 => Float32x4, 3 => Float32
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
                        0 => Float32x2, 1 => Float32x2, 2 => Uint32
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
            ..Default::default()
        });

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
            globals_buf,
            globals_bind,
            rect_pipeline,
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
    }

    /// Upload the icon texture array delivered by the indexer thread.
    /// `icons` holds one premultiplied RGBA8 `ICON_SIZE`² image per app.
    /// `RANK_HITS_MAX` extra layers are reserved past the end for the
    /// dynamic package-search icons ([`Renderer::update_icon_layer`]).
    pub fn set_icons(&mut self, icons: &[Vec<u8>]) {
        // New app set: previously shaped labels may be stale.
        self.label_cache.clear();
        let layers = (icons.len() + crate::nix::RANK_HITS_MAX).max(1) as u32;
        let texture = self.device.create_texture(&wgpu::TextureDescriptor {
            label: Some("waverunner.icons"),
            size: wgpu::Extent3d {
                width: ICON_SIZE,
                height: ICON_SIZE,
                depth_or_array_layers: layers,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Rgba8UnormSrgb,
            usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
            view_formats: &[],
        });
        for (i, pixels) in icons.iter().enumerate() {
            self.queue.write_texture(
                wgpu::TexelCopyTextureInfo {
                    texture: &texture,
                    mip_level: 0,
                    origin: wgpu::Origin3d {
                        x: 0,
                        y: 0,
                        z: i as u32,
                    },
                    aspect: wgpu::TextureAspect::All,
                },
                pixels,
                wgpu::TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: Some(ICON_SIZE * 4),
                    rows_per_image: Some(ICON_SIZE),
                },
                wgpu::Extent3d {
                    width: ICON_SIZE,
                    height: ICON_SIZE,
                    depth_or_array_layers: 1,
                },
            );
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

    /// Overwrite one icon texture-array layer (a dynamic package icon in
    /// the reserved tail of the array). Out-of-range layers and missing
    /// textures are ignored — a rescan re-uploads shortly anyway.
    pub fn update_icon_layer(&mut self, layer: u32, pixels: &[u8]) {
        let Some(texture) = &self.icon_texture else {
            return;
        };
        if layer >= self.icon_layer_count || pixels.len() != (ICON_SIZE * ICON_SIZE * 4) as usize {
            return;
        }
        self.queue.write_texture(
            wgpu::TexelCopyTextureInfo {
                texture,
                mip_level: 0,
                origin: wgpu::Origin3d {
                    x: 0,
                    y: 0,
                    z: layer,
                },
                aspect: wgpu::TextureAspect::All,
            },
            pixels,
            wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(ICON_SIZE * 4),
                rows_per_image: Some(ICON_SIZE),
            },
            wgpu::Extent3d {
                width: ICON_SIZE,
                height: ICON_SIZE,
                depth_or_array_layers: 1,
            },
        );
    }

    /// Render one frame of the given scene.
    pub fn render(&mut self, scene: &Scene, text_color: [f32; 4]) -> anyhow::Result<()> {
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
        let (w, h) = (self.config.width, self.config.height);

        self.queue.write_buffer(
            &self.globals_buf,
            0,
            bytemuck::bytes_of(&Globals {
                screen: [w as f32, h as f32],
                alpha: scene.alpha.clamp(0.0, 1.0),
                _pad: 0.0,
            }),
        );

        // Instance buffers: unclipped ranges first, then one scissored
        // range per section grid.
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
        let icon_buf = self
            .device
            .create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: Some("waverunner.icon-instances"),
                contents: bytemuck::cast_slice(&icons),
                usage: wgpu::BufferUsages::VERTEX,
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
            w: w as f32,
            h: h as f32,
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
        let shape = |font_system: &mut FontSystem, label: &crate::content::Label| {
            let mut buffer =
                TextBuffer::new(font_system, Metrics::new(label.font_px, label.line_px));
            buffer.set_size(font_system, Some(label.max_w), Some(label.line_px));
            buffer.set_text(
                font_system,
                &label.text,
                Attrs::new().family(Family::SansSerif),
                Shaping::Advanced,
            );
            buffer.shape_until_scroll(font_system, false);
            buffer
        };
        let mut fresh: Vec<TextBuffer> = Vec::new();
        let mut fresh_of: Vec<Option<usize>> = Vec::with_capacity(all_labels.len());
        for (label, _) in &all_labels {
            if label.cache {
                let key = format!("{}\u{1}{}", label.text, label.font_px);
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

        let mut text_buffers: Vec<(&TextBuffer, (f32, f32), TextBounds, bool)> = Vec::new();
        for (i, (label, clip)) in all_labels.iter().enumerate() {
            let buffer = match fresh_of[i] {
                Some(fi) => &fresh[fi],
                None => {
                    let key = format!("{}\u{1}{}", label.text, label.font_px);
                    match self.label_cache.get(&key) {
                        Some(buffer) => buffer,
                        None => continue,
                    }
                }
            };
            // Measure the shaped line; center about the anchor when
            // requested; snap to whole pixels so glyphs stay crisp.
            let line_w = buffer
                .layout_runs()
                .next()
                .map(|run| run.line_w)
                .unwrap_or(0.0)
                .min(label.max_w);
            let left = if label.centered {
                (label.pos.0 - line_w / 2.0).round()
            } else {
                label.pos.0.round()
            };
            let top = label.pos.1.round();
            let bounds = TextBounds {
                left: clip.x as i32,
                top: clip.y as i32,
                right: (clip.x + clip.w) as i32,
                bottom: (clip.y + clip.h).min(h as f32) as i32,
            };
            text_buffers.push((buffer, (left, top), bounds, label.dim));
        }
        self.text_viewport.update(
            &self.queue,
            Resolution {
                width: w,
                height: h,
            },
        );
        let dim_rgba = glyphon::Color::rgba(
            (text_color[0] * 255.0) as u8,
            (text_color[1] * 255.0) as u8,
            (text_color[2] * 255.0) as u8,
            (text_color[3] * alpha * 0.45 * 255.0) as u8,
        );
        let areas = text_buffers
            .iter()
            .map(|(buffer, pos, bounds, dim)| TextArea {
                buffer,
                left: pos.0,
                top: pos.1,
                scale: 1.0,
                bounds: *bounds,
                default_color: if *dim { dim_rgba } else { text_rgba },
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

        let mut encoder = self
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("waverunner.frame"),
            });
        {
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("waverunner.scene"),
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
            pass.set_bind_group(0, &self.globals_bind, &[]);

            // Unclipped: card background + dock hover, then dock icons.
            if n_rects_unclipped > 0 {
                pass.set_pipeline(&self.rect_pipeline);
                pass.set_vertex_buffer(0, rect_buf.slice(..));
                pass.draw(0..4, 0..n_rects_unclipped);
            }
            if n_icons_unclipped > 0 {
                if let Some(icon_bind) = &self.icon_bind {
                    pass.set_pipeline(&self.icon_pipeline);
                    pass.set_bind_group(1, icon_bind, &[]);
                    pass.set_vertex_buffer(0, icon_buf.slice(..));
                    pass.draw(0..4, 0..n_icons_unclipped);
                }
            }

            // Each section's grid content under its own scissor rect.
            for (clip, rect_range, icon_range) in &grid_ranges {
                let sx = (clip.x.max(0.0) as u32).min(w);
                let sy = (clip.y.max(0.0) as u32).min(h);
                let sw = (clip.w as u32).min(w - sx);
                let sh = ((clip.y + clip.h).min(h as f32) as u32).saturating_sub(sy);
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

            // Text renders unscissored: every TextArea carries its own
            // clip bounds (grid viewport for app names, the search box
            // for the query), so labels outside the grid still show.
            if !text_buffers.is_empty() {
                self.text_renderer
                    .render(&self.text_atlas, &self.text_viewport, &mut pass)
                    .context("glyphon render failed")?;
            }

            // Topmost: the drag ghost — above grids and labels alike.
            if !overlay_range.is_empty() {
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

fn rect_instance(r: &crate::content::RectInst) -> RectInstance {
    RectInstance {
        rect_min: [r.rect.x, r.rect.y],
        rect_max: [r.rect.x + r.rect.w, r.rect.y + r.rect.h],
        color: r.color,
        radius: r.radius,
        _pad: [0.0; 3],
    }
}

fn icon_instance(i: &crate::content::IconInst) -> IconInstance {
    IconInstance {
        rect_min: [i.rect.x, i.rect.y],
        rect_max: [i.rect.x + i.rect.w, i.rect.y + i.rect.h],
        layer: i.layer,
        _pad: [0; 3],
    }
}
