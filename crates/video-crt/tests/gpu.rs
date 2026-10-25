//! The WGSL shader against the CPU path: the test card through every
//! preset, rendered offscreen with wgpu at integer and non-integer sizes,
//! must match `video_crt::render_size` within a tolerance (the CPU rounds
//! to 8 bits before its gamma table, the GPU works in floats; curved
//! edges and mask boundaries may fall on the other side of a pixel).
//! Persistence is checked over two frames with a ping-pong pair of
//! targets, as the desktop draws it.
//!
//! Without a graphics adapter (headless CI) the test prints a message and
//! passes.

mod common;

use common::{diff, frame, pattern};
use video_crt::{gpu, signal, Crt, Profile};

struct Gpu {
    device: wgpu::Device,
    queue: wgpu::Queue,
    pipeline: wgpu::RenderPipeline,
    layout: wgpu::BindGroupLayout,
    params: wgpu::Buffer,
}

const FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba8Unorm;

impl Gpu {
    fn new() -> Option<Self> {
        let instance = wgpu::Instance::new(&wgpu::InstanceDescriptor::default());
        let adapter =
            pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions::default()))?;
        let (device, queue) = pollster::block_on(adapter.request_device(
            &wgpu::DeviceDescriptor {
                label: None,
                required_features: wgpu::Features::empty(),
                required_limits:
                    wgpu::Limits::downlevel_webgl2_defaults().using_resolution(adapter.limits()),
                memory_hints: wgpu::MemoryHints::default(),
            },
            None,
        ))
        .ok()?;
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("crt"),
            source: wgpu::ShaderSource::Wgsl(gpu::SHADER.into()),
        });
        let texture = |binding| wgpu::BindGroupLayoutEntry {
            binding,
            visibility: wgpu::ShaderStages::FRAGMENT,
            ty: wgpu::BindingType::Texture {
                sample_type: wgpu::TextureSampleType::Float { filterable: false },
                view_dimension: wgpu::TextureViewDimension::D2,
                multisampled: false,
            },
            count: None,
        };
        let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: None,
            entries: &[
                texture(0),
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
                texture(2),
            ],
        });
        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: None,
            bind_group_layouts: &[&layout],
            push_constant_ranges: &[],
        });
        let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: None,
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
                    format: FORMAT,
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
            label: None,
            size: gpu::PARAMS_SIZE,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        Some(Self {
            device,
            queue,
            pipeline,
            layout,
            params,
        })
    }

    fn texture(&self, w: u32, h: u32, usage: wgpu::TextureUsages) -> wgpu::Texture {
        self.device.create_texture(&wgpu::TextureDescriptor {
            label: None,
            size: wgpu::Extent3d {
                width: w,
                height: h,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: FORMAT,
            usage,
            view_formats: &[],
        })
    }

    /// Draw the RGBA source through `profile` into `target`, reading
    /// `previous`, and return the target's RGB bytes.
    fn draw(
        &self,
        src: (&[u8], u32, u32),
        profile: &Profile,
        target: &wgpu::Texture,
        previous: &wgpu::Texture,
    ) -> Vec<u8> {
        let (rgba, sw, sh) = src;
        let (w, h) = (target.width(), target.height());
        let source = self.texture(
            sw,
            sh,
            wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
        );
        self.queue.write_texture(
            wgpu::TexelCopyTextureInfo {
                texture: &source,
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            rgba,
            wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(sw * 4),
                rows_per_image: Some(sh),
            },
            source.size(),
        );
        self.queue.write_buffer(
            &self.params,
            0,
            &gpu::params_bytes(profile, (sw, sh), (w, h), (0, 0)),
        );
        let source_view = source.create_view(&Default::default());
        let previous_view = previous.create_view(&Default::default());
        let bind = self.device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: None,
            layout: &self.layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::TextureView(&source_view),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: self.params.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 2,
                    resource: wgpu::BindingResource::TextureView(&previous_view),
                },
            ],
        });
        let view = target.create_view(&Default::default());
        let row = (w * 4).div_ceil(256) * 256;
        let readback = self.device.create_buffer(&wgpu::BufferDescriptor {
            label: None,
            size: u64::from(row * h),
            usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
            mapped_at_creation: false,
        });
        let mut encoder = self.device.create_command_encoder(&Default::default());
        {
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: None,
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
            pass.set_bind_group(0, &bind, &[]);
            pass.draw(0..3, 0..1);
        }
        encoder.copy_texture_to_buffer(
            wgpu::TexelCopyTextureInfo {
                texture: target,
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            wgpu::TexelCopyBufferInfo {
                buffer: &readback,
                layout: wgpu::TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: Some(row),
                    rows_per_image: Some(h),
                },
            },
            target.size(),
        );
        self.queue.submit([encoder.finish()]);
        let slice = readback.slice(..);
        slice.map_async(wgpu::MapMode::Read, |r| r.expect("map"));
        self.device.poll(wgpu::Maintain::Wait);
        let data = slice.get_mapped_range();
        let mut rgb = Vec::with_capacity((w * h * 3) as usize);
        for y in 0..h {
            let line = &data[(y * row) as usize..][..(w * 4) as usize];
            for p in line.as_chunks::<4>().0 {
                rgb.extend_from_slice(&p[..3]);
            }
        }
        rgb
    }

    fn target(&self, w: u32, h: u32) -> wgpu::Texture {
        self.texture(
            w,
            h,
            wgpu::TextureUsages::RENDER_ATTACHMENT
                | wgpu::TextureUsages::TEXTURE_BINDING
                | wgpu::TextureUsages::COPY_SRC,
        )
    }
}

/// The card after the signal stage, as the desktop uploads it.
fn source_rgba(px: &[u8], profile: &Profile) -> Vec<u8> {
    let f = frame(px);
    let rgba: Vec<u8> = (0..f.height)
        .flat_map(|y| (0..f.width).map(move |x| (x, y)))
        .flat_map(|(x, y)| {
            let c = f.rgb_at(x, y).unwrap();
            [c.r, c.g, c.b, 255]
        })
        .collect();
    signal::process_rgba(f.width, f.height, &rgba, f.signal, profile).unwrap_or(rgba)
}

fn adapter() -> Option<Gpu> {
    let gpu = Gpu::new();
    if gpu.is_none() {
        eprintln!("no graphics adapter: GPU-vs-CPU comparison skipped");
    }
    gpu
}

/// Mean difference below 1.5 levels, at most 0.5 % of the bytes off by
/// more than 6.
fn close(name: &str, gpu: &[u8], cpu: &[u8], failures: &mut Vec<String>) {
    let (max, mean, off) = diff(gpu, cpu, 6);
    eprintln!(
        "{name}: max {max}, mean {mean:.3}, off {:.3} %",
        off * 100.0
    );
    if mean > 1.5 || off > 0.005 {
        failures.push(format!(
            "{name}: max {max}, mean {mean:.3}, {:.3} % off by more than 6",
            off * 100.0
        ));
    }
}

#[test]
fn shader_matches_the_cpu_path() {
    let Some(gpu) = adapter() else {
        return;
    };
    let px = pattern(0);
    let blank = gpu.target(1, 1);
    let mut failures = Vec::new();
    for preset in Profile::PRESETS {
        let mut profile = Profile::preset(preset).unwrap();
        profile.persistence = 0.0;
        let src = source_rgba(&px, &profile);
        let (w2, h2) = video_crt::cpu::output_size(&frame(&px), &profile, 2);
        // Integer scale 2, a non-integer scale and one below 1.
        for (w, h) in [(w2, h2), (w2 * 7 / 4, h2 * 7 / 4 + 1), (150, 101)] {
            let target = gpu.target(w, h);
            let out = gpu.draw((&src, common::W, common::H), &profile, &target, &blank);
            let cpu = video_crt::render_size(&frame(&px), &profile, w, h);
            close(&format!("{preset} {w}x{h}"), &out, &cpu.rgb, &mut failures);
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

#[test]
fn persistence_matches_the_stateful_cpu_renderer() {
    let Some(gpu) = adapter() else {
        return;
    };
    let mut failures = Vec::new();
    for preset in ["pet-green", "gt65"] {
        let profile = Profile::preset(preset).unwrap();
        let (w, h) = video_crt::cpu::output_size(&frame(&pattern(0)), &profile, 2);
        let targets = [gpu.target(w, h), gpu.target(w, h)];
        let mut crt = Crt::new();
        for (i, shift) in [0, 12, 24].into_iter().enumerate() {
            let px = pattern(shift);
            let src = source_rgba(&px, &profile);
            let out = gpu.draw(
                (&src, common::W, common::H),
                &profile,
                &targets[i % 2],
                &targets[1 - i % 2],
            );
            let cpu = crt.render(&frame(&px), &profile, 2);
            close(
                &format!("{preset} frame {i}"),
                &out,
                &cpu.rgb,
                &mut failures,
            );
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}
