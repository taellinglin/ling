use crate::scene::{argb_to_rgba, Batch, Blend, Scene, Vertex, ZFAR};
use std::collections::HashMap;
use wgpu::util::DeviceExt;

pub const DEPTH_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Depth32Float;

struct GpuTexture {
    view: wgpu::TextureView,
}

/// Encodes [`Scene`]s into render passes on a device you provide.
pub struct Renderer {
    pipelines: [wgpu::RenderPipeline; 3],
    frame_layout: wgpu::BindGroupLayout,
    batch_layout: wgpu::BindGroupLayout,
    sampler: wgpu::Sampler,
    white: GpuTexture,
    textures: HashMap<u64, GpuTexture>,
}

fn pipeline(
    device: &wgpu::Device,
    shader: &wgpu::ShaderModule,
    layout: &wgpu::PipelineLayout,
    format: wgpu::TextureFormat,
    blend: Blend,
) -> wgpu::RenderPipeline {
    let (state, depth_write) = match blend {
        Blend::Opaque => (None, true),
        Blend::Alpha => (Some(wgpu::BlendState::ALPHA_BLENDING), false),
        Blend::Additive => (
            Some(wgpu::BlendState {
                color: wgpu::BlendComponent {
                    src_factor: wgpu::BlendFactor::SrcAlpha,
                    dst_factor: wgpu::BlendFactor::One,
                    operation: wgpu::BlendOperation::Add,
                },
                alpha: wgpu::BlendComponent {
                    src_factor: wgpu::BlendFactor::Zero,
                    dst_factor: wgpu::BlendFactor::One,
                    operation: wgpu::BlendOperation::Add,
                },
            }),
            false,
        ),
    };
    device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
        label: Some("ling-gl"),
        layout: Some(layout),
        vertex: wgpu::VertexState {
            module: shader,
            entry_point: "vs",
            compilation_options: Default::default(),
            buffers: &[wgpu::VertexBufferLayout {
                array_stride: std::mem::size_of::<Vertex>() as u64,
                step_mode: wgpu::VertexStepMode::Vertex,
                attributes: &wgpu::vertex_attr_array![0 => Float32x2, 1 => Float32, 2 => Float32x2, 3 => Float32x4, 4 => Float32x3],
            }],
        },
        fragment: Some(wgpu::FragmentState {
            module: shader,
            entry_point: "fs",
            compilation_options: Default::default(),
            targets: &[Some(wgpu::ColorTargetState {
                format,
                blend: state,
                write_mask: wgpu::ColorWrites::ALL,
            })],
        }),
        primitive: wgpu::PrimitiveState::default(),
        depth_stencil: Some(wgpu::DepthStencilState {
            format: DEPTH_FORMAT,
            depth_write_enabled: depth_write,
            depth_compare: wgpu::CompareFunction::LessEqual,
            stencil: Default::default(),
            bias: Default::default(),
        }),
        multisample: wgpu::MultisampleState::default(),
        multiview: None,
        cache: None,
    })
}

fn upload(device: &wgpu::Device, queue: &wgpu::Queue, w: u32, h: u32, rgba: &[u8]) -> GpuTexture {
    let tex = device.create_texture_with_data(
        queue,
        &wgpu::TextureDescriptor {
            label: Some("ling-gl-tex"),
            size: wgpu::Extent3d { width: w, height: h, depth_or_array_layers: 1 },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Rgba8Unorm,
            usage: wgpu::TextureUsages::TEXTURE_BINDING,
            view_formats: &[],
        },
        wgpu::util::TextureDataOrder::LayerMajor,
        rgba,
    );
    GpuTexture { view: tex.create_view(&Default::default()) }
}

impl Renderer {
    /// `format` is the colour target format the pipelines will draw into.
    pub fn new(device: &wgpu::Device, queue: &wgpu::Queue, format: wgpu::TextureFormat) -> Self {
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("ling-gl"),
            source: wgpu::ShaderSource::Wgsl(include_str!("shader.wgsl").into()),
        });
        let uniform = |binding, vis| wgpu::BindGroupLayoutEntry {
            binding,
            visibility: vis,
            ty: wgpu::BindingType::Buffer {
                ty: wgpu::BufferBindingType::Uniform,
                has_dynamic_offset: false,
                min_binding_size: None,
            },
            count: None,
        };
        let frame_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("ling-gl-frame"),
            entries: &[uniform(0, wgpu::ShaderStages::VERTEX_FRAGMENT)],
        });
        let batch_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("ling-gl-batch"),
            entries: &[
                uniform(0, wgpu::ShaderStages::FRAGMENT),
                wgpu::BindGroupLayoutEntry {
                    binding: 1,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Float { filterable: true },
                        view_dimension: wgpu::TextureViewDimension::D2,
                        multisampled: false,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 2,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                    count: None,
                },
            ],
        });
        let pl = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("ling-gl"),
            bind_group_layouts: &[&frame_layout, &batch_layout],
            push_constant_ranges: &[],
        });
        let pipelines = [
            pipeline(device, &shader, &pl, format, Blend::Opaque),
            pipeline(device, &shader, &pl, format, Blend::Alpha),
            pipeline(device, &shader, &pl, format, Blend::Additive),
        ];
        let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            address_mode_u: wgpu::AddressMode::Repeat,
            address_mode_v: wgpu::AddressMode::Repeat,
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            ..Default::default()
        });
        let white = upload(device, queue, 1, 1, &[255, 255, 255, 255]);
        Renderer { pipelines, frame_layout, batch_layout, sampler, white, textures: HashMap::new() }
    }

    /// Drop cached textures (e.g. after the engine reloads its assets).
    pub fn clear_textures(&mut self) {
        self.textures.clear();
    }

    /// Record the scene into `enc`, clearing `color`/`depth` first.
    pub fn encode(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        enc: &mut wgpu::CommandEncoder,
        color: &wgpu::TextureView,
        depth: &wgpu::TextureView,
        scene: &Scene,
        width: u32,
        height: u32,
    ) {
        for t in &scene.textures {
            if t.w > 0 && t.h > 0 && t.px.len() >= (t.w * t.h) as usize && !self.textures.contains_key(&t.id) {
                let g = upload(device, queue, t.w, t.h, &argb_to_rgba(&t.px[..(t.w * t.h) as usize]));
                self.textures.insert(t.id, g);
            }
        }
        let frame: [f32; 4] = [width as f32, height as f32, ZFAR, scene.zdist];
        let frame_buf = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("ling-gl-frame"),
            contents: bytemuck::cast_slice(&frame),
            usage: wgpu::BufferUsages::UNIFORM,
        });
        let frame_bg = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("ling-gl-frame"),
            layout: &self.frame_layout,
            entries: &[wgpu::BindGroupEntry { binding: 0, resource: frame_buf.as_entire_binding() }],
        });

        // Buffers and bind groups must outlive the pass, so build them first.
        let mut prepared: Vec<(usize, wgpu::Buffer, wgpu::BindGroup, u32)> = Vec::new();
        for b in scene.batches.iter().filter(|b| b.verts.len() >= 3) {
            let n = (b.verts.len() / 3 * 3) as u32;
            let vb = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: Some("ling-gl-verts"),
                contents: bytemuck::cast_slice(&b.verts[..n as usize]),
                usage: wgpu::BufferUsages::VERTEX,
            });
            let bg = self.batch_group(device, b);
            let pi = match b.blend {
                Blend::Opaque => 0,
                Blend::Alpha => 1,
                Blend::Additive => 2,
            };
            prepared.push((pi, vb, bg, n));
        }

        let mut rp = enc.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("ling-gl"),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view: color,
                resolve_target: None,
                ops: wgpu::Operations {
                    load: wgpu::LoadOp::Clear(wgpu::Color {
                        r: scene.clear[0] as f64,
                        g: scene.clear[1] as f64,
                        b: scene.clear[2] as f64,
                        a: scene.clear_alpha as f64,
                    }),
                    store: wgpu::StoreOp::Store,
                },
            })],
            depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                view: depth,
                depth_ops: Some(wgpu::Operations {
                    load: wgpu::LoadOp::Clear(1.0),
                    store: wgpu::StoreOp::Discard,
                }),
                stencil_ops: None,
            }),
            timestamp_writes: None,
            occlusion_query_set: None,
        });
        rp.set_bind_group(0, &frame_bg, &[]);
        for (pi, vb, bg, n) in &prepared {
            rp.set_pipeline(&self.pipelines[*pi]);
            rp.set_bind_group(1, bg, &[]);
            rp.set_vertex_buffer(0, vb.slice(..));
            rp.draw(0..*n, 0..1);
        }
    }

    fn batch_group(&self, device: &wgpu::Device, b: &Batch) -> wgpu::BindGroup {
        let mut u = [0.0f32; 72];
        u[..8].copy_from_slice(&[
            b.fog.color[0],
            b.fog.color[1],
            b.fog.color[2],
            b.fog.start,
            b.fog.end,
            b.lighting.amb,
            if b.lighting.lit { 1.0 } else { 0.0 },
            b.lighting.lights.len().min(crate::MAX_LIGHTS) as f32,
        ]);
        for (i, l) in b.lighting.lights.iter().take(crate::MAX_LIGHTS).enumerate() {
            u[8 + i * 8..8 + i * 8 + 8].copy_from_slice(&[l[0], l[1], l[2], l[6], l[3], l[4], l[5], l[7]]);
        }
        let ub = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("ling-gl-batch"),
            contents: bytemuck::cast_slice(&u),
            usage: wgpu::BufferUsages::UNIFORM,
        });
        let tex = b.texture.and_then(|id| self.textures.get(&id)).unwrap_or(&self.white);
        device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("ling-gl-batch"),
            layout: &self.batch_layout,
            entries: &[
                wgpu::BindGroupEntry { binding: 0, resource: ub.as_entire_binding() },
                wgpu::BindGroupEntry { binding: 1, resource: wgpu::BindingResource::TextureView(&tex.view) },
                wgpu::BindGroupEntry { binding: 2, resource: wgpu::BindingResource::Sampler(&self.sampler) },
            ],
        })
    }
}

/// Offscreen renderer with CPU readback (native only).
#[cfg(not(target_arch = "wasm32"))]
pub struct Headless {
    device: wgpu::Device,
    queue: wgpu::Queue,
    renderer: Renderer,
    adapter_name: String,
}

#[cfg(not(target_arch = "wasm32"))]
impl Headless {
    /// `None` when no GPU adapter or device is available.
    pub fn new() -> Option<Self> {
        let instance = wgpu::Instance::new(wgpu::InstanceDescriptor::default());
        let adapter = pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions {
            power_preference: wgpu::PowerPreference::HighPerformance,
            force_fallback_adapter: false,
            compatible_surface: None,
        }))?;
        let (device, queue) = pollster::block_on(adapter.request_device(
            &wgpu::DeviceDescriptor {
                label: Some("ling-gl"),
                required_features: wgpu::Features::empty(),
                required_limits: adapter.limits(),
                memory_hints: wgpu::MemoryHints::Performance,
            },
            None,
        ))
        .ok()?;
        device.on_uncaptured_error(Box::new(|e| eprintln!("[ling-gl] GPU error: {e}")));
        let renderer = Renderer::new(&device, &queue, wgpu::TextureFormat::Rgba8Unorm);
        Some(Headless { adapter_name: adapter.get_info().name, device, queue, renderer })
    }

    pub fn adapter_name(&self) -> &str {
        &self.adapter_name
    }

    pub fn renderer_mut(&mut self) -> &mut Renderer {
        &mut self.renderer
    }

    /// Render `scene` and write `0x00RRGGBB` pixels into `out` (`width * height`).
    pub fn render(&mut self, scene: &Scene, width: usize, height: usize, out: &mut [u32]) -> bool {
        self.render_impl(scene, width, height, out, false, None)
    }

    /// Render with a transparent background (set `scene.clear_alpha = 0`) and
    /// composite the result over the existing contents of `out`.
    pub fn render_over(&mut self, scene: &Scene, width: usize, height: usize, out: &mut [u32]) -> bool {
        self.render_impl(scene, width, height, out, true, None)
    }

    /// Like [`render_over`], and also merges the GPU depth into `zbuf` as the CPU
    /// rasteriser's camera-space depth (`1/iw - zdist`, nearest wins) so DoF, SSAO and
    /// other depth post-effects see the GPU geometry.
    pub fn render_over_depth(&mut self, scene: &Scene, width: usize, height: usize, out: &mut [u32], zbuf: &mut [f32]) -> bool {
        self.render_impl(scene, width, height, out, true, Some((zbuf, scene.zdist)))
    }

    fn render_impl(&mut self, scene: &Scene, width: usize, height: usize, out: &mut [u32], over: bool, mut zbuf: Option<(&mut [f32], f32)>) -> bool {
        if width == 0 || height == 0 || out.len() < width * height {
            return false;
        }
        if let Some((z, _)) = &zbuf {
            if z.len() < width * height {
                zbuf = None;
            }
        }
        let size = wgpu::Extent3d { width: width as u32, height: height as u32, depth_or_array_layers: 1 };
        let color = self.device.create_texture(&wgpu::TextureDescriptor {
            label: Some("ling-gl-color"),
            size,
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Rgba8Unorm,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
            view_formats: &[],
        });
        let depth = self.device.create_texture(&wgpu::TextureDescriptor {
            label: Some("ling-gl-depth"),
            size,
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: DEPTH_FORMAT,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
            view_formats: &[],
        });
        let padded = (width * 4).div_ceil(256) * 256;
        let rb = self.device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("ling-gl-readback"),
            size: (padded * height) as u64,
            usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
            mapped_at_creation: false,
        });
        let zrb = zbuf.as_ref().map(|_| {
            self.device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("ling-gl-depth-readback"),
                size: (padded * height) as u64,
                usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
                mapped_at_creation: false,
            })
        });
        let mut enc = self
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor { label: Some("ling-gl") });
        self.renderer.encode(
            &self.device,
            &self.queue,
            &mut enc,
            &color.create_view(&Default::default()),
            &depth.create_view(&Default::default()),
            scene,
            width as u32,
            height as u32,
        );
        enc.copy_texture_to_buffer(
            wgpu::ImageCopyTexture {
                texture: &color,
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            wgpu::ImageCopyBuffer {
                buffer: &rb,
                layout: wgpu::ImageDataLayout {
                    offset: 0,
                    bytes_per_row: Some(padded as u32),
                    rows_per_image: Some(height as u32),
                },
            },
            size,
        );
        if let Some(zrb) = &zrb {
            enc.copy_texture_to_buffer(
                wgpu::ImageCopyTexture {
                    texture: &depth,
                    mip_level: 0,
                    origin: wgpu::Origin3d::ZERO,
                    aspect: wgpu::TextureAspect::DepthOnly,
                },
                wgpu::ImageCopyBuffer {
                    buffer: zrb,
                    layout: wgpu::ImageDataLayout {
                        offset: 0,
                        bytes_per_row: Some(padded as u32),
                        rows_per_image: Some(height as u32),
                    },
                },
                size,
            );
        }
        self.queue.submit(Some(enc.finish()));

        let slice = rb.slice(..);
        let (tx, rx) = std::sync::mpsc::channel();
        slice.map_async(wgpu::MapMode::Read, move |r| {
            let _ = tx.send(r);
        });
        let zslice = zrb.as_ref().map(|b| b.slice(..));
        let (ztx, zrx) = std::sync::mpsc::channel();
        if let Some(zs) = &zslice {
            zs.map_async(wgpu::MapMode::Read, move |r| {
                let _ = ztx.send(r);
            });
        }
        let _ = self.device.poll(wgpu::Maintain::Wait);
        if !matches!(rx.recv(), Ok(Ok(()))) {
            return false;
        }
        if let (Some(zs), Some((z, zd))) = (&zslice, zbuf.as_mut()) {
            if matches!(zrx.recv(), Ok(Ok(()))) {
                let data = zs.get_mapped_range();
                for y in 0..height {
                    let row = &data[y * padded..y * padded + width * 4];
                    for (x, p) in row.chunks_exact(4).enumerate() {
                        let d = f32::from_le_bytes([p[0], p[1], p[2], p[3]]);
                        if d < 0.9999 {
                            let v = d * ZFAR - *zd;
                            let o = &mut z[y * width + x];
                            if v < *o {
                                *o = v;
                            }
                        }
                    }
                }
            }
        }
        if let Some(zrb) = &zrb {
            zrb.unmap();
        }
        {
            let data = slice.get_mapped_range();
            for y in 0..height {
                let row = &data[y * padded..y * padded + width * 4];
                for (x, p) in row.chunks_exact(4).enumerate() {
                    let o = &mut out[y * width + x];
                    if over {
                        // Colour is premultiplied by the blend state, so: src + dst * (1 - a).
                        let inv = 255 - p[3] as u32;
                        let d = *o;
                        let ch = |s: u8, sh: u32| ((s as u32 + ((d >> sh) & 0xFF) * inv / 255).min(255)) << sh;
                        *o = ch(p[0], 16) | ch(p[1], 8) | ch(p[2], 0);
                    } else {
                        *o = ((p[0] as u32) << 16) | ((p[1] as u32) << 8) | p[2] as u32;
                    }
                }
            }
        }
        rb.unmap();
        true
    }
}

#[cfg(all(test, not(target_arch = "wasm32")))]
mod tests {
    use super::*;
    use crate::scene::*;

    fn vert(x: f32, y: f32, z: f32, u: f32, v: f32, tint: [f32; 4]) -> Vertex {
        let iw = 1.0 / z;
        Vertex { x, y, iw, uw: u * iw, vw: v * iw, tint, w: [0.0; 3] }
    }

    #[test]
    fn net_after_gpu_init() {
        let Some(mut gpu) = Headless::new() else { return };
        let mut scene = Scene::new([0.0; 3]);
        scene.batches.push(Batch { verts: vec![Vertex::default(); 3], ..Default::default() });
        let mut out = vec![0u32; 64 * 64];
        gpu.render(&scene, 64, 64, &mut out);
        let r = std::net::TcpStream::connect_timeout(&"127.0.0.1:7779".parse().unwrap(), std::time::Duration::from_secs(3));
        eprintln!("connect after gpu: {:?}", r.as_ref().map(|_| ()));
        assert!(r.is_ok());
    }

    #[test]
    fn frame_timing() {
        let Some(mut gpu) = Headless::new() else { return };
        let (w, h) = (1920usize, 1080usize);
        let mut scene = Scene::new([0.0; 3]);
        scene.textures.push(Texture { id: 7, w: 64, h: 64, px: vec![0xFF8040C0; 64 * 64] });
        let mut verts = Vec::new();
        for i in 0..5000 {
            let (x, y, z) = ((i % 70) as f32 * 27.0, (i / 70) as f32 * 14.0, 10.0 + (i % 9) as f32);
            for (dx, dy) in [(0.0, 0.0), (26.0, 0.0), (0.0, 13.0)] {
                verts.push(vert(x + dx, y + dy, z, dx / 26.0, dy / 13.0, [1.0; 4]));
            }
        }
        scene.batches.push(Batch { texture: Some(7), verts, ..Default::default() });
        let mut out = vec![0u32; w * h];
        for i in 0..8 {
            let t = std::time::Instant::now();
            assert!(gpu.render(&scene, w, h, &mut out));
            eprintln!("frame {i}: {:?}", t.elapsed());
            scene.textures.clear();
        }
    }

    #[test]
    fn depth_and_texture() {
        let Some(mut gpu) = Headless::new() else {
            eprintln!("no GPU adapter - skipping");
            return;
        };
        let (w, h) = (64usize, 64usize);
        let red = [1.0, 0.0, 0.0, 1.0];
        let blue = [0.0, 0.0, 1.0, 1.0];
        let big = |z: f32, c: [f32; 4]| {
            vec![
                vert(2.0, 2.0, z, 0.0, 0.0, c),
                vert(62.0, 2.0, z, 1.0, 0.0, c),
                vert(2.0, 62.0, z, 0.0, 1.0, c),
            ]
        };
        let mut scene = Scene::new([0.0; 3]);
        // Blue (far) is submitted last but red (near) must win via the depth buffer.
        scene.batches.push(Batch { texture: None, blend: Blend::Opaque, verts: big(5.0, red), ..Default::default() });
        scene.batches.push(Batch { texture: None, blend: Blend::Opaque, verts: big(50.0, blue), ..Default::default() });
        let mut out = vec![0u32; w * h];
        assert!(gpu.render(&scene, w, h, &mut out));
        let p = out[20 * w + 12];
        assert!((p >> 16) & 0xFF > 200 && p & 0xFF < 40, "near red wins: {p:06X}");
        assert_eq!(out[(h - 2) * w + w - 2] & 0xFFFFFF, 0, "outside triangle is clear");

        // Textured: a solid green texel must tint through.
        let mut s2 = Scene::new([0.0; 3]);
        s2.textures.push(Texture { id: 1, w: 2, h: 2, px: vec![0xFF00FF00; 4] });
        s2.batches.push(Batch { texture: Some(1), blend: Blend::Opaque, verts: big(5.0, [1.0; 4]), ..Default::default() });
        assert!(gpu.render(&s2, w, h, &mut out));
        let p = out[20 * w + 12];
        assert!((p >> 8) & 0xFF > 200 && (p >> 16) & 0xFF < 40, "texture green: {p:06X}");
    }
}
