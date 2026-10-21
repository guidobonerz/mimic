//! wgpu presentation: the frame is uploaded as a texture and drawn through
//! the CRT shader of `video-crt` into a letterboxed rectangle that keeps the
//! monitor profile's aspect ratio. The status footer, when shown, is a
//! second texture drawn 1:1 into a strip under the picture.

use std::sync::Arc;

use video_crt::Profile;
use winit::window::Window;

use crate::emu::Frame;

pub struct Gfx {
    surface: wgpu::Surface<'static>,
    device: wgpu::Device,
    queue: wgpu::Queue,
    config: wgpu::SurfaceConfiguration,
    pipeline: wgpu::RenderPipeline,
    layout: wgpu::BindGroupLayout,
    params: wgpu::Buffer,
    texture: Option<(wgpu::Texture, wgpu::BindGroup, u32, u32)>,
    profile: Profile,
    /// Output size of the last frame through the profile (aspect source).
    out_size: (u32, u32),
    footer_pipeline: wgpu::RenderPipeline,
    footer_layout: wgpu::BindGroupLayout,
    footer_sampler: wgpu::Sampler,
    /// Footer texture, its bind group and size in window pixels.
    footer: Option<(wgpu::Texture, wgpu::BindGroup, u32, u32)>,
}

/// Draws a texture over the whole viewport, nearest-neighbour.
const FOOTER_SHADER: &str = "
@group(0) @binding(0) var tex: texture_2d<f32>;
@group(0) @binding(1) var smp: sampler;
struct VOut { @builtin(position) pos: vec4f, @location(0) uv: vec2f };
@vertex fn vs_main(@builtin(vertex_index) i: u32) -> VOut {
    let uv = vec2f(f32((i << 1u) & 2u), f32(i & 2u));
    var o: VOut;
    o.pos = vec4f(uv.x * 2.0 - 1.0, 1.0 - uv.y * 2.0, 0.0, 1.0);
    o.uv = uv;
    return o;
}
@fragment fn fs_main(v: VOut) -> @location(0) vec4f {
    return textureSample(tex, smp, v.uv);
}
";

impl Gfx {
    pub fn new(window: Arc<Window>, profile: Profile) -> Result<Self, String> {
        let instance = wgpu::Instance::new(&wgpu::InstanceDescriptor::default());
        let size = window.inner_size();
        let surface = instance
            .create_surface(window)
            .map_err(|e| format!("surface: {e}"))?;
        let adapter = pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions {
            compatible_surface: Some(&surface),
            ..Default::default()
        }))
        .ok_or("no graphics adapter")?;
        let (device, queue) = pollster::block_on(adapter.request_device(
            &wgpu::DeviceDescriptor {
                label: Some("mimic"),
                required_features: wgpu::Features::empty(),
                required_limits:
                    wgpu::Limits::downlevel_webgl2_defaults().using_resolution(adapter.limits()),
                memory_hints: wgpu::MemoryHints::default(),
            },
            None,
        ))
        .map_err(|e| format!("device: {e}"))?;

        let mut config = surface
            .get_default_config(&adapter, size.width.max(1), size.height.max(1))
            .ok_or("surface not supported by the adapter")?;
        // The shader writes display-ready (gamma-encoded) values: prefer a
        // non-sRGB target so they are not encoded twice.
        let caps = surface.get_capabilities(&adapter);
        if let Some(f) = caps.formats.iter().find(|f| !f.is_srgb()) {
            config.format = *f;
        }
        config.present_mode = wgpu::PresentMode::AutoVsync;
        // One frame in flight: every redraw waits for the next vertical
        // blank, so redraws come evenly instead of in bursts that would
        // find the frame queue empty or overfull.
        config.desired_maximum_frame_latency = 1;
        surface.configure(&device, &config);

        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("crt"),
            source: wgpu::ShaderSource::Wgsl(video_crt::gpu::SHADER.into()),
        });
        let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("crt"),
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Float { filterable: false },
                        view_dimension: wgpu::TextureViewDimension::D2,
                        multisampled: false,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 1,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Uniform,
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                    count: None,
                },
            ],
        });
        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("crt"),
            bind_group_layouts: &[&layout],
            push_constant_ranges: &[],
        });
        let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("crt"),
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
                    blend: None,
                    write_mask: wgpu::ColorWrites::ALL,
                })],
            }),
            primitive: wgpu::PrimitiveState::default(),
            depth_stencil: None,
            multisample: wgpu::MultisampleState::default(),
            multiview: None,
            cache: None,
        });
        let params = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("crt params"),
            size: 48,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let footer_shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("footer"),
            source: wgpu::ShaderSource::Wgsl(FOOTER_SHADER.into()),
        });
        let footer_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("footer"),
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
        let footer_pipeline_layout =
            device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
                label: Some("footer"),
                bind_group_layouts: &[&footer_layout],
                push_constant_ranges: &[],
            });
        let footer_pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("footer"),
            layout: Some(&footer_pipeline_layout),
            vertex: wgpu::VertexState {
                module: &footer_shader,
                entry_point: Some("vs_main"),
                compilation_options: Default::default(),
                buffers: &[],
            },
            fragment: Some(wgpu::FragmentState {
                module: &footer_shader,
                entry_point: Some("fs_main"),
                compilation_options: Default::default(),
                targets: &[Some(wgpu::ColorTargetState {
                    format: config.format,
                    blend: None,
                    write_mask: wgpu::ColorWrites::ALL,
                })],
            }),
            primitive: wgpu::PrimitiveState::default(),
            depth_stencil: None,
            multisample: wgpu::MultisampleState::default(),
            multiview: None,
            cache: None,
        });
        let footer_sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("footer"),
            mag_filter: wgpu::FilterMode::Nearest,
            min_filter: wgpu::FilterMode::Nearest,
            ..Default::default()
        });
        Ok(Self {
            surface,
            device,
            queue,
            config,
            pipeline,
            layout,
            params,
            texture: None,
            profile,
            out_size: (4, 3),
            footer_pipeline,
            footer_layout,
            footer_sampler,
            footer: None,
        })
    }

    /// Footer width in window pixels.
    pub fn footer_width(&self) -> u32 {
        self.config.width.max(1)
    }

    /// Height the footer takes from the window, in window pixels.
    fn footer_height(&self) -> u32 {
        self.footer
            .as_ref()
            .map_or(0, |(_, _, _, h)| (*h).min(self.config.height / 2))
    }

    /// Show `rgba` (`width` × `height` window pixels) as the footer, or
    /// hide it with `None`.
    pub fn set_footer(&mut self, rgba: Option<(&[u8], u32, u32)>) {
        let Some((rgba, width, height)) = rgba else {
            self.footer = None;
            return;
        };
        let fresh = !matches!(&self.footer, Some((_, _, w, h)) if (*w, *h) == (width, height));
        if fresh {
            let texture = self.device.create_texture(&wgpu::TextureDescriptor {
                label: Some("footer"),
                size: wgpu::Extent3d {
                    width,
                    height,
                    depth_or_array_layers: 1,
                },
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format: wgpu::TextureFormat::Rgba8Unorm,
                usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
                view_formats: &[],
            });
            let view = texture.create_view(&Default::default());
            let bind = self.device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("footer"),
                layout: &self.footer_layout,
                entries: &[
                    wgpu::BindGroupEntry {
                        binding: 0,
                        resource: wgpu::BindingResource::TextureView(&view),
                    },
                    wgpu::BindGroupEntry {
                        binding: 1,
                        resource: wgpu::BindingResource::Sampler(&self.footer_sampler),
                    },
                ],
            });
            self.footer = Some((texture, bind, width, height));
        }
        if let Some((texture, ..)) = &self.footer {
            self.queue.write_texture(
                wgpu::TexelCopyTextureInfo {
                    texture,
                    mip_level: 0,
                    origin: wgpu::Origin3d::ZERO,
                    aspect: wgpu::TextureAspect::All,
                },
                rgba,
                wgpu::TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: Some(width * 4),
                    rows_per_image: Some(height),
                },
                wgpu::Extent3d {
                    width,
                    height,
                    depth_or_array_layers: 1,
                },
            );
        }
    }

    /// The frame pixel under window position (`x`, `y`), if the pointer is
    /// over the picture.
    pub fn frame_pos(&self, x: f64, y: f64) -> Option<(u32, u32)> {
        let (_, _, fw, fh) = self.texture.as_ref()?;
        let (vx, vy, vw, vh) = self.viewport();
        let (px, py) = (x - f64::from(vx), y - f64::from(vy));
        if px < 0.0 || py < 0.0 || px >= f64::from(vw) || py >= f64::from(vh) {
            return None;
        }
        Some((
            (px * f64::from(*fw) / f64::from(vw)) as u32,
            (py * f64::from(*fh) / f64::from(vh)) as u32,
        ))
    }

    pub fn resize(&mut self, width: u32, height: u32) {
        if width > 0 && height > 0 {
            self.config.width = width;
            self.config.height = height;
            self.surface.configure(&self.device, &self.config);
        }
    }

    /// Upload a new frame.
    pub fn upload(&mut self, frame: &Frame, scale_hint: u32) {
        let fresh =
            !matches!(&self.texture, Some((_, _, w, h)) if (*w, *h) == (frame.width, frame.height));
        if fresh {
            let texture = self.device.create_texture(&wgpu::TextureDescriptor {
                label: Some("frame"),
                size: wgpu::Extent3d {
                    width: frame.width,
                    height: frame.height,
                    depth_or_array_layers: 1,
                },
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format: wgpu::TextureFormat::Rgba8Unorm,
                usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
                view_formats: &[],
            });
            let view = texture.create_view(&Default::default());
            let bind = self.device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("crt"),
                layout: &self.layout,
                entries: &[
                    wgpu::BindGroupEntry {
                        binding: 0,
                        resource: wgpu::BindingResource::TextureView(&view),
                    },
                    wgpu::BindGroupEntry {
                        binding: 1,
                        resource: self.params.as_entire_binding(),
                    },
                ],
            });
            self.texture = Some((texture, bind, frame.width, frame.height));
        }
        // Signal stage on the CPU, CRT stage in the shader.
        let decoded = video_crt::signal::process_rgba(
            frame.width,
            frame.height,
            &frame.rgba,
            frame.signal,
            &self.profile,
        );
        if let Some((texture, ..)) = &self.texture {
            self.queue.write_texture(
                wgpu::TexelCopyTextureInfo {
                    texture,
                    mip_level: 0,
                    origin: wgpu::Origin3d::ZERO,
                    aspect: wgpu::TextureAspect::All,
                },
                decoded.as_deref().unwrap_or(&frame.rgba),
                wgpu::TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: Some(frame.width * 4),
                    rows_per_image: Some(frame.height),
                },
                wgpu::Extent3d {
                    width: frame.width,
                    height: frame.height,
                    depth_or_array_layers: 1,
                },
            );
        }
        let f = emu_core::FrameRef {
            width: frame.width,
            height: frame.height,
            pixels: emu_core::frame::PixelData::Rgb(&[]),
            luma_chroma: None,
            signal: frame.signal,
        };
        self.out_size = video_crt::cpu::output_size(&f, &self.profile, scale_hint);
    }

    /// The largest rectangle with the output's aspect ratio in the window.
    fn viewport(&self) -> (u32, u32, u32, u32) {
        let (sw, sh) = (self.config.width, self.config.height - self.footer_height());
        let (ow, oh) = (self.out_size.0.max(1), self.out_size.1.max(1));
        let (w, h) = if u64::from(sw) * u64::from(oh) > u64::from(sh) * u64::from(ow) {
            ((u64::from(sh) * u64::from(ow) / u64::from(oh)) as u32, sh)
        } else {
            (sw, (u64::from(sw) * u64::from(oh) / u64::from(ow)) as u32)
        };
        ((sw - w) / 2, (sh - h) / 2, w.max(1), h.max(1))
    }

    pub fn render(&mut self) {
        let Some((_, bind, fw, fh)) = &self.texture else {
            return;
        };
        let (x, y, w, h) = self.viewport();
        let params = video_crt::gpu::params(&self.profile, (*fw, *fh), (w, h), (x, y));
        let bytes: Vec<u8> = params.iter().flat_map(|f| f.to_le_bytes()).collect();
        self.queue.write_buffer(&self.params, 0, &bytes);

        let target = match self.surface.get_current_texture() {
            Ok(t) => t,
            Err(_) => {
                self.surface.configure(&self.device, &self.config);
                return;
            }
        };
        let view = target.texture.create_view(&Default::default());
        let mut encoder = self.device.create_command_encoder(&Default::default());
        {
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("crt"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &view,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(wgpu::Color::BLACK),
                        store: wgpu::StoreOp::Store,
                    },
                })],
                depth_stencil_attachment: None,
                timestamp_writes: None,
                occlusion_query_set: None,
            });
            pass.set_pipeline(&self.pipeline);
            pass.set_bind_group(0, bind, &[]);
            pass.set_viewport(x as f32, y as f32, w as f32, h as f32, 0.0, 1.0);
            pass.draw(0..3, 0..1);
            if let Some((_, footer, fw, _)) = &self.footer {
                let fh = self.footer_height();
                pass.set_pipeline(&self.footer_pipeline);
                pass.set_bind_group(0, footer, &[]);
                pass.set_viewport(
                    0.0,
                    (self.config.height - fh) as f32,
                    (*fw).min(self.config.width) as f32,
                    fh as f32,
                    0.0,
                    1.0,
                );
                pass.draw(0..3, 0..1);
            }
        }
        self.queue.submit([encoder.finish()]);
        target.present();
    }
}
