//! wgpu presentation: the frame is uploaded as a texture and drawn through
//! the CRT shader of `video-crt` into a letterboxed rectangle that keeps the
//! monitor profile's aspect ratio. The status footer, when shown, is a
//! second texture drawn 1:1 into a strip under the picture.
//!
//! Phosphor persistence needs the previous output: with a profile whose
//! `persistence` is above 0 the shader draws into one of two offscreen
//! targets the size of the picture rectangle, reading the other (the last
//! output), and the result is copied 1:1 to the window. The pair swaps
//! every frame and is rebuilt (history cleared) when the rectangle's size
//! changes. Without persistence the shader draws straight to the window.

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
    /// The frame texture, its view and size.
    texture: Option<(wgpu::Texture, wgpu::TextureView, u32, u32)>,
    /// Stands in for the previous output when there is no persistence.
    no_history: wgpu::TextureView,
    /// Ping-pong targets for persistence.
    history: Option<History>,
    profile: Profile,
    /// Output size of the last frame through the profile (aspect source).
    out_size: (u32, u32),
    footer_pipeline: wgpu::RenderPipeline,
    footer_layout: wgpu::BindGroupLayout,
    footer_sampler: wgpu::Sampler,
    /// Footer texture, its bind group and size in window pixels.
    footer: Option<(wgpu::Texture, wgpu::BindGroup, u32, u32)>,
}

/// Two render targets of the picture rectangle's size: the shader draws into
/// `views[current]` reading the other one, which holds the last output.
struct History {
    size: (u32, u32),
    _targets: [wgpu::Texture; 2],
    views: [wgpu::TextureView; 2],
    /// Bind groups of the copy to the window (footer pipeline), per target.
    blits: [wgpu::BindGroup; 2],
    current: usize,
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
        // Mailbox where there is one (DX12, some Vulkan drivers): a present
        // never blocks and the display shows the newest frame at its next
        // vertical blank, so a frame goes out the moment the emulation has
        // it. FIFO (the fallback) queues presents and, through the
        // compositor, releases them in bursts of two or three; a frame then
        // waits up to two refreshes, and the render thread paces itself by
        // the blocking (`render::run`).
        config.present_mode = if caps.present_modes.contains(&wgpu::PresentMode::Mailbox) {
            wgpu::PresentMode::Mailbox
        } else {
            wgpu::PresentMode::AutoVsync
        };
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
                // The previous output (persistence).
                wgpu::BindGroupLayoutEntry {
                    binding: 2,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Float { filterable: false },
                        view_dimension: wgpu::TextureViewDimension::D2,
                        multisampled: false,
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
            size: video_crt::gpu::PARAMS_SIZE,
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
        let no_history = device
            .create_texture(&wgpu::TextureDescriptor {
                label: Some("no history"),
                size: wgpu::Extent3d {
                    width: 1,
                    height: 1,
                    depth_or_array_layers: 1,
                },
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format: wgpu::TextureFormat::Rgba8Unorm,
                usage: wgpu::TextureUsages::TEXTURE_BINDING,
                view_formats: &[],
            })
            .create_view(&Default::default());
        Ok(Self {
            surface,
            device,
            queue,
            config,
            pipeline,
            layout,
            params,
            texture: None,
            no_history,
            history: None,
            profile,
            out_size: (4, 3),
            footer_pipeline,
            footer_layout,
            footer_sampler,
            footer: None,
        })
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

    /// Presents block until the display took the previous frame (FIFO),
    /// which paces a loop of presents at the refresh rate.
    pub fn presents_block(&self) -> bool {
        self.config.present_mode != wgpu::PresentMode::Mailbox
    }

    /// The uploaded frame's size (0 x 0 before the first).
    pub fn frame_size(&self) -> (u32, u32) {
        self.texture
            .as_ref()
            .map_or((0, 0), |(_, _, w, h)| (*w, *h))
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
            self.texture = Some((texture, view, frame.width, frame.height));
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
    /// The picture's rectangle in the window: x, y, width, height.
    pub fn viewport(&self) -> (u32, u32, u32, u32) {
        let (sw, sh) = (self.config.width, self.config.height - self.footer_height());
        let (ow, oh) = (self.out_size.0.max(1), self.out_size.1.max(1));
        let (w, h) = if u64::from(sw) * u64::from(oh) > u64::from(sh) * u64::from(ow) {
            ((u64::from(sh) * u64::from(ow) / u64::from(oh)) as u32, sh)
        } else {
            (sw, (u64::from(sw) * u64::from(oh) / u64::from(ow)) as u32)
        };
        ((sw - w) / 2, (sh - h) / 2, w.max(1), h.max(1))
    }

    /// Switch the monitor profile (live editing); takes effect with the
    /// next upload and draw.
    pub fn set_profile(&mut self, profile: Profile) {
        if profile.persistence <= 0.0 {
            self.history = None;
        }
        self.profile = profile;
    }

    /// The persistence targets for a `w` × `h` picture, rebuilt (history
    /// cleared: new textures start black) when the size changes.
    fn ensure_history(&mut self, w: u32, h: u32) {
        if self
            .history
            .as_ref()
            .is_some_and(|hist| hist.size != (w, h))
        {
            self.history = None;
        }
        if self.history.is_some() {
            return;
        }
        let target = || {
            self.device.create_texture(&wgpu::TextureDescriptor {
                label: Some("crt history"),
                size: wgpu::Extent3d {
                    width: w,
                    height: h,
                    depth_or_array_layers: 1,
                },
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format: self.config.format,
                usage: wgpu::TextureUsages::RENDER_ATTACHMENT
                    | wgpu::TextureUsages::TEXTURE_BINDING,
                view_formats: &[],
            })
        };
        let targets = [target(), target()];
        let views = [
            targets[0].create_view(&Default::default()),
            targets[1].create_view(&Default::default()),
        ];
        let blit = |view: &wgpu::TextureView| {
            self.device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("crt history"),
                layout: &self.footer_layout,
                entries: &[
                    wgpu::BindGroupEntry {
                        binding: 0,
                        resource: wgpu::BindingResource::TextureView(view),
                    },
                    wgpu::BindGroupEntry {
                        binding: 1,
                        resource: wgpu::BindingResource::Sampler(&self.footer_sampler),
                    },
                ],
            })
        };
        let blits = [blit(&views[0]), blit(&views[1])];
        self.history = Some(History {
            size: (w, h),
            _targets: targets,
            views,
            blits,
            current: 0,
        });
    }

    /// The CRT shader's bind group: the frame, the params and `previous`.
    fn crt_bind_group(&self, previous: &wgpu::TextureView) -> Option<wgpu::BindGroup> {
        let (_, view, ..) = self.texture.as_ref()?;
        Some(self.device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("crt"),
            layout: &self.layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::TextureView(view),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: self.params.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 2,
                    resource: wgpu::BindingResource::TextureView(previous),
                },
            ],
        }))
    }

    /// Take the window's next image. With one frame in flight this blocks
    /// until the display has consumed the previous one (its vertical
    /// blank). `None`: the surface was lost and is configured again.
    pub fn acquire(&mut self) -> Option<wgpu::SurfaceTexture> {
        match self.surface.get_current_texture() {
            Ok(t) => Some(t),
            Err(_) => {
                self.surface.configure(&self.device, &self.config);
                None
            }
        }
    }

    /// Draw the frame and the footer into `target` and present it.
    pub fn draw(&mut self, target: wgpu::SurfaceTexture) {
        let Some((_, _, fw, fh)) = &self.texture else {
            return;
        };
        let src = (*fw, *fh);
        let (x, y, w, h) = self.viewport();
        let persistent = self.profile.persistence > 0.0;
        if persistent {
            self.ensure_history(w, h);
        }
        // With persistence the shader draws into a history target at
        // (0, 0), otherwise straight into the window's rectangle.
        let origin = if persistent { (0, 0) } else { (x, y) };
        let bytes = video_crt::gpu::params_bytes(&self.profile, src, (w, h), origin);
        self.queue.write_buffer(&self.params, 0, &bytes);

        let view = target.texture.create_view(&Default::default());
        let mut encoder = self.device.create_command_encoder(&Default::default());
        let history = self.history.as_ref().filter(|_| persistent);
        let direct = match history {
            Some(hist) => {
                let current = hist.current;
                let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                    label: Some("crt history"),
                    color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                        view: &hist.views[current],
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
                if let Some(bind) = self.crt_bind_group(&hist.views[1 - current]) {
                    pass.set_pipeline(&self.pipeline);
                    pass.set_bind_group(0, &bind, &[]);
                    pass.draw(0..3, 0..1);
                }
                None
            }
            None => self.crt_bind_group(&self.no_history),
        };
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
            pass.set_viewport(x as f32, y as f32, w as f32, h as f32, 0.0, 1.0);
            if let Some(bind) = &direct {
                pass.set_pipeline(&self.pipeline);
                pass.set_bind_group(0, bind, &[]);
                pass.draw(0..3, 0..1);
            }
            if let Some(hist) = history {
                // The finished picture, 1:1 into its rectangle.
                pass.set_pipeline(&self.footer_pipeline);
                pass.set_bind_group(0, &hist.blits[hist.current], &[]);
                pass.draw(0..3, 0..1);
            }
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
        if let Some(hist) = self.history.as_mut().filter(|_| persistent) {
            hist.current = 1 - hist.current;
        }
    }
}
