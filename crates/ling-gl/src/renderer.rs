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
    batch_layout: wgpu::BindGroupLayout,
    sampler: wgpu::Sampler,
    white: GpuTexture,
    textures: HashMap<u64, GpuTexture>,
    /// Last encode's texture uploads: [count, pixels, ms] (for profiling).
    pub last_upload: [f64; 3],
    /// Per-frame constants: one persistent buffer + bind group, rewritten each encode.
    frame_buf: wgpu::Buffer,
    frame_bg: wgpu::BindGroup,
    /// Vertex / batch-uniform buffers, reused across encodes and grown on demand.
    vbuf: Option<wgpu::Buffer>,
    ubuf: Option<wgpu::Buffer>,
    /// Batch bind groups per texture (None = white): the uniforms bind at a dynamic
    /// offset, so these live as long as `ubuf` and the texture do.
    groups: HashMap<Option<u64>, wgpu::BindGroup>,
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
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Uniform,
                        has_dynamic_offset: true,
                        min_binding_size: wgpu::BufferSize::new(UNIFORM_BYTES as u64),
                    },
                    count: None,
                },
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
        let frame_buf = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("ling-gl-frame"),
            size: 16,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let frame_bg = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("ling-gl-frame"),
            layout: &frame_layout,
            entries: &[wgpu::BindGroupEntry { binding: 0, resource: frame_buf.as_entire_binding() }],
        });
        Renderer {
            pipelines,
            batch_layout,
            sampler,
            white,
            textures: HashMap::new(),
            last_upload: [0.0; 3],
            frame_buf,
            frame_bg,
            vbuf: None,
            ubuf: None,
            groups: HashMap::new(),
        }
    }

    /// Drop cached textures (e.g. after the engine reloads its assets).
    pub fn clear_textures(&mut self) {
        self.textures.clear();
        self.groups.clear();
    }

    /// Free one cached texture (it is re-uploaded if a later scene lists it again).
    pub fn remove_texture(&mut self, id: u64) {
        self.textures.remove(&id);
        self.groups.remove(&Some(id));
    }

    /// A buffer of `usage` holding at least `need` bytes: `slot`'s when it fits, else a
    /// new one with headroom. Returns true when it was (re)allocated.
    fn ensure(device: &wgpu::Device, slot: &mut Option<wgpu::Buffer>, need: u64, usage: wgpu::BufferUsages, label: &str) -> bool {
        if matches!(slot, Some(b) if b.size() >= need) {
            return false;
        }
        let size = (need + need / 2).max(1 << 16).div_ceil(256) * 256;
        *slot = Some(device.create_buffer(&wgpu::BufferDescriptor {
            label: Some(label),
            size,
            usage: usage | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        }));
        true
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
        let t_up = std::time::Instant::now();
        let (mut n_up, mut px_up) = (0usize, 0usize);
        for t in &scene.textures {
            if t.w > 0 && t.h > 0 && t.px.len() >= (t.w * t.h) as usize && !self.textures.contains_key(&t.id) {
                let g = upload(device, queue, t.w, t.h, &argb_to_rgba(&t.px[..(t.w * t.h) as usize]));
                self.textures.insert(t.id, g);
                n_up += 1;
                px_up += (t.w * t.h) as usize;
            }
        }
        self.last_upload = [n_up as f64, px_up as f64, t_up.elapsed().as_secs_f64() * 1000.0];
        let frame: [f32; 4] = [width as f32, height as f32, ZFAR, scene.zdist];
        queue.write_buffer(&self.frame_buf, 0, bytemuck::cast_slice(&frame));

        // Every batch shares one persistent vertex buffer and one uniform buffer
        // (dynamic offsets), so a frame allocates no GPU objects once warm. Queue
        // writes land before this encoder's submit, so one encode per submit.
        let live: Vec<&Batch> = scene.batches.iter().filter(|b| b.verts.len() >= 3).collect();
        let align = device.limits().min_uniform_buffer_offset_alignment.max(1) as usize;
        let ustride = UNIFORM_BYTES.div_ceil(align) * align;
        let mut ubytes: Vec<u8> = vec![0; ustride * live.len()];
        let mut draws: Vec<(usize, u32, u32, Option<u64>)> = Vec::with_capacity(live.len());
        let mut total = 0usize;
        for (i, b) in live.iter().enumerate() {
            let n = b.verts.len() / 3 * 3;
            let u = batch_uniforms(b);
            ubytes[i * ustride..i * ustride + UNIFORM_BYTES].copy_from_slice(bytemuck::cast_slice(&u));
            let pi = match b.blend {
                Blend::Opaque => 0,
                Blend::Alpha => 1,
                Blend::Additive => 2,
            };
            // Missing textures draw white; key by what is actually bound.
            let key = b.texture.filter(|id| self.textures.contains_key(id));
            draws.push((pi, total as u32, n as u32, key));
            total += n;
        }
        if total > 0 {
            let vsz = std::mem::size_of::<Vertex>();
            Self::ensure(device, &mut self.vbuf, (total * vsz) as u64, wgpu::BufferUsages::VERTEX, "ling-gl-verts");
            if Self::ensure(device, &mut self.ubuf, ubytes.len() as u64, wgpu::BufferUsages::UNIFORM, "ling-gl-batch") {
                self.groups.clear();
            }
            let (Some(vb), Some(ub)) = (&self.vbuf, &self.ubuf) else { return };
            if let Some(mut view) = queue.write_buffer_with(vb, 0, wgpu::BufferSize::new((total * vsz) as u64).unwrap()) {
                // Copy each batch straight into the staging memory (no concatenated Vec).
                let mut rest: &mut [u8] = &mut view;
                let mut jobs: Vec<(&mut [u8], &[Vertex])> = Vec::with_capacity(live.len());
                for b in &live {
                    let src = &b.verts[..b.verts.len() / 3 * 3];
                    let (dst, tail) = std::mem::take(&mut rest).split_at_mut(src.len() * vsz);
                    jobs.push((dst, src));
                    rest = tail;
                }
                #[cfg(not(target_arch = "wasm32"))]
                {
                    use rayon::prelude::*;
                    jobs.into_par_iter().for_each(|(d, s)| d.copy_from_slice(bytemuck::cast_slice(s)));
                }
                #[cfg(target_arch = "wasm32")]
                for (d, s) in jobs {
                    d.copy_from_slice(bytemuck::cast_slice(s));
                }
            }
            queue.write_buffer(ub, 0, &ubytes);
            for &(_, _, _, key) in &draws {
                if !self.groups.contains_key(&key) {
                    let g = self.batch_group(device, key, ub);
                    self.groups.insert(key, g);
                }
            }
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
        rp.set_bind_group(0, &self.frame_bg, &[]);
        if let (Some(vb), true) = (&self.vbuf, total > 0) {
            rp.set_vertex_buffer(0, vb.slice(..));
            let mut cur_pi = usize::MAX;
            for (i, &(pi, first, n, key)) in draws.iter().enumerate() {
                if pi != cur_pi {
                    rp.set_pipeline(&self.pipelines[pi]);
                    cur_pi = pi;
                }
                rp.set_bind_group(1, &self.groups[&key], &[(i * ustride) as u32]);
                rp.draw(first..first + n, 0..1);
            }
        }
    }

    fn batch_group(&self, device: &wgpu::Device, key: Option<u64>, ub: &wgpu::Buffer) -> wgpu::BindGroup {
        let tex = key.and_then(|id| self.textures.get(&id)).unwrap_or(&self.white);
        device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("ling-gl-batch"),
            layout: &self.batch_layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::Buffer(wgpu::BufferBinding {
                        buffer: ub,
                        offset: 0,
                        size: wgpu::BufferSize::new(UNIFORM_BYTES as u64),
                    }),
                },
                wgpu::BindGroupEntry { binding: 1, resource: wgpu::BindingResource::TextureView(&tex.view) },
                wgpu::BindGroupEntry { binding: 2, resource: wgpu::BindingResource::Sampler(&self.sampler) },
            ],
        })
    }
}

const UNIFORM_BYTES: usize = 72 * 4;

fn batch_uniforms(b: &Batch) -> [f32; 72] {
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
    u
}

/// Offscreen renderer with CPU readback (native only).
#[cfg(not(target_arch = "wasm32"))]
pub struct Headless {
    device: wgpu::Device,
    queue: wgpu::Queue,
    renderer: Renderer,
    adapter_name: String,
    /// Render targets + readback buffers, reused while the frame size holds.
    targets: Option<Targets>,
    /// Last render's [encode+submit, GPU wait, unpack] in ms (for profiling).
    pub timings: [f64; 3],
}

/// Per-size offscreen colour/depth targets and their mappable readback buffers.
#[cfg(not(target_arch = "wasm32"))]
struct Targets {
    width: usize,
    height: usize,
    color: wgpu::Texture,
    color_view: wgpu::TextureView,
    depth: wgpu::Texture,
    depth_view: wgpu::TextureView,
    rb: wgpu::Buffer,
    zrb: wgpu::Buffer,
}

#[cfg(not(target_arch = "wasm32"))]
impl Headless {
    /// `None` when no GPU adapter or device is available.
    pub fn new() -> Option<Self> {
        let backends = wgpu::util::backend_bits_from_env().unwrap_or(wgpu::Backends::all());
        let instance = wgpu::Instance::new(wgpu::InstanceDescriptor { backends, ..Default::default() });
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
        Some(Headless { adapter_name: format!("{} ({:?})", adapter.get_info().name, adapter.get_info().backend), device, queue, renderer, targets: None, timings: [0.0; 3] })
    }

    /// Release GPU objects in order and wait for the device to idle. process::exit otherwise
    /// destroys the Vulkan device from thread-local destructors inside ExitProcess, which crashes.
    pub fn shutdown(self) {
        let Headless { device, queue, renderer, targets, .. } = self;
        drop(targets);
        let idle = queue.submit(std::iter::empty());
        device.poll(wgpu::Maintain::wait_for(idle));
        drop(renderer);
        device.poll(wgpu::Maintain::Wait);
        drop(queue);
        drop(device);
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
        use rayon::prelude::*;
        if width == 0 || height == 0 || out.len() < width * height {
            return false;
        }
        if let Some((z, _)) = &zbuf {
            if z.len() < width * height {
                zbuf = None;
            }
        }
        let size = wgpu::Extent3d { width: width as u32, height: height as u32, depth_or_array_layers: 1 };
        let padded = (width * 4).div_ceil(256) * 256;
        // Targets and readback buffers are reused frame to frame (every pass clears
        // both attachments), instead of allocating ~4 full-frame GPU objects per flush.
        if !matches!(&self.targets, Some(t) if t.width == width && t.height == height) {
            self.targets = None;
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
            let rb = self.device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("ling-gl-readback"),
                size: (padded * height) as u64,
                usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
                mapped_at_creation: false,
            });
            let zrb = self.device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("ling-gl-depth-readback"),
                size: (padded * height) as u64,
                usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
                mapped_at_creation: false,
            });
            let color_view = color.create_view(&Default::default());
            let depth_view = depth.create_view(&Default::default());
            self.targets = Some(Targets { width, height, color, color_view, depth, depth_view, rb, zrb });
        }
        let Some(t) = self.targets.as_ref() else { return false };
        let t_enc = std::time::Instant::now();
        let mut enc = self
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor { label: Some("ling-gl") });
        self.renderer.encode(
            &self.device,
            &self.queue,
            &mut enc,
            &t.color_view,
            &t.depth_view,
            scene,
            width as u32,
            height as u32,
        );
        enc.copy_texture_to_buffer(
            wgpu::ImageCopyTexture {
                texture: &t.color,
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            wgpu::ImageCopyBuffer {
                buffer: &t.rb,
                layout: wgpu::ImageDataLayout {
                    offset: 0,
                    bytes_per_row: Some(padded as u32),
                    rows_per_image: Some(height as u32),
                },
            },
            size,
        );
        let want_z = zbuf.is_some();
        if want_z {
            enc.copy_texture_to_buffer(
                wgpu::ImageCopyTexture {
                    texture: &t.depth,
                    mip_level: 0,
                    origin: wgpu::Origin3d::ZERO,
                    aspect: wgpu::TextureAspect::DepthOnly,
                },
                wgpu::ImageCopyBuffer {
                    buffer: &t.zrb,
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
        let enc_ms = t_enc.elapsed().as_secs_f64() * 1000.0;
        let t_wait = std::time::Instant::now();

        let slice = t.rb.slice(..);
        let (tx, rx) = std::sync::mpsc::channel();
        slice.map_async(wgpu::MapMode::Read, move |r| {
            let _ = tx.send(r);
        });
        let zslice = if want_z { Some(t.zrb.slice(..)) } else { None };
        let (ztx, zrx) = std::sync::mpsc::channel();
        if let Some(zs) = &zslice {
            zs.map_async(wgpu::MapMode::Read, move |r| {
                let _ = ztx.send(r);
            });
        }
        let _ = self.device.poll(wgpu::Maintain::Wait);
        let color_ok = matches!(rx.recv(), Ok(Ok(())));
        let z_ok = zslice.is_some() && matches!(zrx.recv(), Ok(Ok(())));
        let wait_ms = t_wait.elapsed().as_secs_f64() * 1000.0;
        let t_unpack = std::time::Instant::now();
        if !color_ok {
            // Leave nothing mapped behind for the next frame: rebuild the targets.
            self.targets = None;
            return false;
        }
        if let (Some(zs), Some((z, zd)), true) = (&zslice, zbuf.as_mut(), z_ok) {
            let data = zs.get_mapped_range();
            let bytes: &[u8] = &data;
            let zd = *zd;
            z[..width * height].par_chunks_mut(width).enumerate().for_each(|(y, zrow)| {
                let row = &bytes[y * padded..y * padded + width * 4];
                match bytemuck::try_cast_slice::<u8, f32>(row) {
                    Ok(ds) => {
                        for (&d, o) in ds.iter().zip(zrow.iter_mut()) {
                            if d < 0.9999 {
                                let v = d * ZFAR - zd;
                                if v < *o {
                                    *o = v;
                                }
                            }
                        }
                    },
                    Err(_) => {
                        for (p, o) in row.chunks_exact(4).zip(zrow.iter_mut()) {
                            let d = f32::from_le_bytes([p[0], p[1], p[2], p[3]]);
                            if d < 0.9999 {
                                let v = d * ZFAR - zd;
                                if v < *o {
                                    *o = v;
                                }
                            }
                        }
                    },
                }
            });
        }
        if z_ok {
            t.zrb.unmap();
        }
        {
            let data = slice.get_mapped_range();
            let bytes: &[u8] = &data;
            // RGBA8 little-endian word → 0x00RRGGBB
            #[inline(always)]
            fn rgb(p: u32) -> u32 {
                ((p & 0xFF) << 16) | (p & 0xFF00) | ((p >> 16) & 0xFF)
            }
            #[inline(always)]
            fn over_px(p: u32, d: u32) -> u32 {
                // Colour is premultiplied by the blend state, so: src + dst * (1 - a).
                let a = p >> 24;
                if a == 255 {
                    rgb(p)
                } else if p == 0 {
                    d & 0x00FF_FFFF // nothing drawn here
                } else {
                    let inv = 255 - a;
                    let ch = |s: u32, sh: u32| ((s + ((d >> sh) & 0xFF) * inv / 255).min(255)) << sh;
                    ch(p & 0xFF, 16) | ch((p >> 8) & 0xFF, 8) | ch((p >> 16) & 0xFF, 0)
                }
            }
            out[..width * height].par_chunks_mut(width).enumerate().for_each(|(y, orow)| {
                let row = &bytes[y * padded..y * padded + width * 4];
                let words: std::borrow::Cow<[u32]> = match bytemuck::try_cast_slice::<u8, u32>(row) {
                    Ok(ws) => std::borrow::Cow::Borrowed(ws),
                    Err(_) => std::borrow::Cow::Owned(
                        row.chunks_exact(4).map(|p| u32::from_le_bytes([p[0], p[1], p[2], p[3]])).collect(),
                    ),
                };
                if over {
                    for (&p, o) in words.iter().zip(orow.iter_mut()) {
                        *o = over_px(p, *o);
                    }
                } else {
                    for (&p, o) in words.iter().zip(orow.iter_mut()) {
                        *o = rgb(p);
                    }
                }
            });
        }
        t.rb.unmap();
        self.timings = [enc_ms, wait_ms, t_unpack.elapsed().as_secs_f64() * 1000.0];
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
        let mut z = vec![f32::INFINITY; w * h];
        eprintln!("adapter {}", gpu.adapter_name());
        for i in 0..12 {
            let t = std::time::Instant::now();
            assert!(gpu.render_over_depth(&scene, w, h, &mut out, &mut z));
            eprintln!("frame {i}: {:?} enc/wait/unpack {:?}", t.elapsed(), gpu.timings);
            scene.textures.clear();
        }
    }

    /// The game's dense-area shape (run 5: ~40k tris in ~130 batches, ~30 textures).
    /// cargo test --release -p ling-gl dense_bench -- --ignored --nocapture
    #[test]
    #[ignore]
    fn dense_bench() {
        let Some(mut gpu) = Headless::new() else { return };
        let (w, h) = (1920usize, 1080usize);
        let mut scene = Scene::new([0.0; 3]);
        for id in 0..30u64 {
            scene.textures.push(Texture { id, w: 256, h: 256, px: vec![0xFF8040C0 ^ id as u32; 256 * 256] });
        }
        let lights = vec![[0.0, 5.0, 0.0, 1.0, 0.9, 0.8, 1.0, 40.0]; 4];
        for b in 0..130usize {
            let mut verts = Vec::with_capacity(310 * 3);
            for i in 0..310usize {
                let k = b * 310 + i;
                let (x, y, z) = ((k % 140) as f32 * 13.7, (k / 140 % 77) as f32 * 14.0, 10.0 + (k % 9) as f32);
                for (dx, dy) in [(0.0, 0.0), (26.0, 0.0), (0.0, 13.0)] {
                    verts.push(vert(x + dx, y + dy, z, dx / 26.0, dy / 13.0, [1.0; 4]));
                }
            }
            scene.batches.push(Batch {
                texture: Some((b % 30) as u64),
                blend: if b % 13 == 12 { Blend::Alpha } else { Blend::Opaque },
                fog: Fog { start: 20.0, end: 90.0, color: [0.5, 0.6, 0.7] },
                lighting: Lighting { lit: true, amb: 0.4, lights: lights.clone() },
                verts,
            });
        }
        let mut out = vec![0u32; w * h];
        let mut z = vec![f32::INFINITY; w * h];
        eprintln!("adapter {} tris {}", gpu.adapter_name(), scene.tri_count());
        let mut acc = [0.0f64; 4];
        let n = 40;
        for i in 0..n + 5 {
            let t = std::time::Instant::now();
            assert!(gpu.render_over_depth(&scene, w, h, &mut out, &mut z));
            let wall = t.elapsed().as_secs_f64() * 1000.0;
            if i >= 5 {
                acc[0] += gpu.timings[0];
                acc[1] += gpu.timings[1];
                acc[2] += gpu.timings[2];
                acc[3] += wall;
            }
        }
        let f = n as f64;
        eprintln!(
            "dense: encode {:.2} wait {:.2} unpack {:.2} total {:.2} ms",
            acc[0] / f,
            acc[1] / f,
            acc[2] / f,
            acc[3] / f
        );
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
