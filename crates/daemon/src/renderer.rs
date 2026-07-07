//! wgpu rendering onto the layer-shell surface.
//!
//! Draws the popup as a single rounded rectangle (top corners only)
//! anchored to the bottom edge, via an SDF fragment shader. The rect's
//! height is the animation's `extent`; its opacity is the animation's
//! `alpha`. Text and list content arrive with P4.

use std::ptr::NonNull;

use anyhow::{anyhow, Context};
use raw_window_handle::{
    RawDisplayHandle, RawWindowHandle, WaylandDisplayHandle, WaylandWindowHandle,
};
use wayland_client::protocol::wl_surface::WlSurface;
use wayland_client::{Connection, Proxy};

/// Uniforms for the rounded-rect shader. Layout must match `Params` in
/// `shaders/rounded_rect.wgsl` (48 bytes, std140-compatible).
#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
struct Params {
    rect_min: [f32; 2],
    rect_max: [f32; 2],
    color: [f32; 4],
    radius: f32,
    alpha: f32,
    _pad: [f32; 2],
}

pub struct Renderer {
    surface: wgpu::Surface<'static>,
    device: wgpu::Device,
    queue: wgpu::Queue,
    config: wgpu::SurfaceConfiguration,
    pipeline: wgpu::RenderPipeline,
    params_buf: wgpu::Buffer,
    bind_group: wgpu::BindGroup,
    background: [f32; 4],
    corner_radius: f32,
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
        background: [f32; 4],
        corner_radius: f32,
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

        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("waverunner.rounded_rect"),
            source: wgpu::ShaderSource::Wgsl(include_str!("shaders/rounded_rect.wgsl").into()),
        });

        let params_buf = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("waverunner.params"),
            size: std::mem::size_of::<Params>() as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });

        let bind_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("waverunner.params"),
            entries: &[wgpu::BindGroupLayoutEntry {
                binding: 0,
                visibility: wgpu::ShaderStages::FRAGMENT,
                ty: wgpu::BindingType::Buffer {
                    ty: wgpu::BufferBindingType::Uniform,
                    has_dynamic_offset: false,
                    min_binding_size: None,
                },
                count: None,
            }],
        });
        let bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("waverunner.params"),
            layout: &bind_layout,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: params_buf.as_entire_binding(),
            }],
        });

        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("waverunner.pipeline"),
            bind_group_layouts: &[&bind_layout],
            push_constant_ranges: &[],
        });
        let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("waverunner.rounded_rect"),
            layout: Some(&pipeline_layout),
            vertex: wgpu::VertexState {
                module: &shader,
                entry_point: Some("vs_main"),
                compilation_options: Default::default(),
                buffers: &[],
            },
            fragment: Some(wgpu::FragmentState {
                module: &shader,
                entry_point: Some("fs_main"),
                compilation_options: Default::default(),
                targets: &[Some(wgpu::ColorTargetState {
                    format: config.format,
                    // Shader outputs premultiplied alpha.
                    blend: Some(wgpu::BlendState {
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
                    }),
                    write_mask: wgpu::ColorWrites::ALL,
                })],
            }),
            primitive: wgpu::PrimitiveState::default(),
            depth_stencil: None,
            multisample: wgpu::MultisampleState::default(),
            multiview: None,
            cache: None,
        });

        Ok(Self {
            surface,
            device,
            queue,
            config,
            pipeline,
            params_buf,
            bind_group,
            background,
            corner_radius,
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

    /// Render one frame: the popup rect occupying the bottom `extent`
    /// pixels of the surface at the given opacity.
    pub fn render(&mut self, extent: f32, alpha: f32) -> anyhow::Result<()> {
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

        let (w, h) = (self.config.width as f32, self.config.height as f32);
        let extent = extent.clamp(0.0, h);
        // Don't let the corner rounding exceed the visible sliver.
        let radius = self.corner_radius.min(extent);
        let params = Params {
            rect_min: [0.0, h - extent],
            rect_max: [w, h],
            color: self.background,
            radius,
            alpha: alpha.clamp(0.0, 1.0),
            _pad: [0.0; 2],
        };
        self.queue
            .write_buffer(&self.params_buf, 0, bytemuck::bytes_of(&params));

        let mut encoder = self
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("waverunner.frame"),
            });
        {
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("waverunner.rounded_rect"),
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
            if extent > 0.0 && alpha > 0.0 {
                pass.set_pipeline(&self.pipeline);
                pass.set_bind_group(0, &self.bind_group, &[]);
                pass.draw(0..3, 0..1);
            }
        }

        self.queue.submit(std::iter::once(encoder.finish()));
        frame.present();
        Ok(())
    }
}
