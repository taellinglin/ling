// src/gfx/wgpu_raster.rs — native GPU (wgpu) offscreen triangle rasteriser.
//
// Foundation for native GPU drawing. The engine already projects every draw
// call to screen space and accumulates it in the `DepthQueue` (see depth.rs),
// exactly like the WebGL backend (webgl.rs). So the GPU only needs to fill
// flat / Gouraud-shaded triangles — no projection, no clipping.
//
// minifb owns the native window's pixel buffer, so wgpu cannot present to it
// directly. We therefore render to an *offscreen* RGBA8 target and read it
// back into the `u32` framebuffer minifb blits. (The zero-readback path is to
// migrate the window to a wgpu surface; this module is the rasteriser half of
// that work and is correct/verifiable today.)
//
// Build with `--features gpu`.  `cargo test --features gpu gpu_triangle`
// renders a triangle on the GPU and asserts the pixels — proving the path.

use std::sync::Mutex;

/// One screen-space vertex: pixel position (x right, y down) + linear RGB 0..1.
#[repr(C)]
#[derive(Clone, Copy)]
pub struct Vert {
    pub pos: [f32; 2],
    pub color: [f32; 3],
}

struct Gpu {
    device: wgpu::Device,
    queue: wgpu::Queue,
    pipeline: wgpu::RenderPipeline,
    uniform: wgpu::Buffer,
    bind_group: wgpu::BindGroup,
}

// Process-wide GPU state. `None` until first use; stays `None` (and `raster`
// returns false → CPU fallback) if no adapter/device is available.
static GPU: Mutex<Option<Gpu>> = Mutex::new(None);
static INIT_FAILED: Mutex<bool> = Mutex::new(false);

const WGSL: &str = r#"
struct Uniforms { size: vec2<f32> };
@group(0) @binding(0) var<uniform> u: Uniforms;

struct VOut {
    @builtin(position) pos: vec4<f32>,
    @location(0) color: vec3<f32>,
};

@vertex
fn vs(@location(0) screen: vec2<f32>, @location(1) color: vec3<f32>) -> VOut {
    var o: VOut;
    // screen pixels -> NDC; flip Y so framebuffer row 0 is the top.
    let ndc = (screen / u.size) * 2.0 - vec2<f32>(1.0, 1.0);
    o.pos = vec4<f32>(ndc.x, -ndc.y, 0.0, 1.0);
    o.color = color;
    return o;
}

@fragment
fn fs(i: VOut) -> @location(0) vec4<f32> {
    return vec4<f32>(i.color, 1.0);
}
"#;

fn try_init() -> Option<Gpu> {
    let instance = wgpu::Instance::new(wgpu::InstanceDescriptor::default());
    let adapter = pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions {
        power_preference: wgpu::PowerPreference::HighPerformance,
        force_fallback_adapter: false,
        compatible_surface: None,
    }))?;
    let (device, queue) = pollster::block_on(adapter.request_device(
        &wgpu::DeviceDescriptor {
            label: Some("ling-gpu-raster"),
            required_features: wgpu::Features::empty(),
            required_limits: wgpu::Limits::downlevel_defaults(),
            memory_hints: wgpu::MemoryHints::Performance,
        },
        None,
    ))
    .ok()?;

    let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
        label: Some("ling-raster-shader"),
        source: wgpu::ShaderSource::Wgsl(WGSL.into()),
    });

    let uniform = device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("ling-uniform"),
        size: 16, // vec2<f32> padded to 16 bytes
        usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
        mapped_at_creation: false,
    });

    let bgl = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
        label: Some("ling-bgl"),
        entries: &[wgpu::BindGroupLayoutEntry {
            binding: 0,
            visibility: wgpu::ShaderStages::VERTEX,
            ty: wgpu::BindingType::Buffer {
                ty: wgpu::BufferBindingType::Uniform,
                has_dynamic_offset: false,
                min_binding_size: None,
            },
            count: None,
        }],
    });
    let bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: Some("ling-bg"),
        layout: &bgl,
        entries: &[wgpu::BindGroupEntry {
            binding: 0,
            resource: uniform.as_entire_binding(),
        }],
    });

    let pl = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
        label: Some("ling-pl"),
        bind_group_layouts: &[&bgl],
        push_constant_ranges: &[],
    });

    let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
        label: Some("ling-pipeline"),
        layout: Some(&pl),
        vertex: wgpu::VertexState {
            module: &shader,
            entry_point: "vs",
            compilation_options: Default::default(),
            buffers: &[wgpu::VertexBufferLayout {
                array_stride: 20, // 2 + 3 floats
                step_mode: wgpu::VertexStepMode::Vertex,
                attributes: &[
                    wgpu::VertexAttribute {
                        format: wgpu::VertexFormat::Float32x2,
                        offset: 0,
                        shader_location: 0,
                    },
                    wgpu::VertexAttribute {
                        format: wgpu::VertexFormat::Float32x3,
                        offset: 8,
                        shader_location: 1,
                    },
                ],
            }],
        },
        fragment: Some(wgpu::FragmentState {
            module: &shader,
            entry_point: "fs",
            compilation_options: Default::default(),
            targets: &[Some(wgpu::ColorTargetState {
                format: wgpu::TextureFormat::Rgba8Unorm,
                blend: Some(wgpu::BlendState::ALPHA_BLENDING),
                write_mask: wgpu::ColorWrites::ALL,
            })],
        }),
        primitive: wgpu::PrimitiveState::default(),
        depth_stencil: None,
        multisample: wgpu::MultisampleState::default(),
        multiview: None,
        cache: None,
    });

    Some(Gpu {
        device,
        queue,
        pipeline,
        uniform,
        bind_group,
    })
}

/// Rasterise `verts` (a triangle list, 3 vertices per triangle) into `out`
/// (len `width*height`, each `0x00RRGGBB`), clearing to `clear` (linear RGB)
/// first. Returns `false` if no GPU is available — callers fall back to the
/// CPU rasteriser.
pub fn raster(
    verts: &[Vert],
    width: usize,
    height: usize,
    clear: [f32; 3],
    out: &mut [u32],
) -> bool {
    if width == 0 || height == 0 {
        return false;
    }
    if *INIT_FAILED.lock().unwrap() {
        return false;
    }
    let mut guard = GPU.lock().unwrap();
    if guard.is_none() {
        match try_init() {
            Some(g) => *guard = Some(g),
            None => {
                *INIT_FAILED.lock().unwrap() = true;
                return false;
            }
        }
    }
    let g = guard.as_ref().unwrap();

    // Uniform: viewport size.
    let size = [width as f32, height as f32, 0.0f32, 0.0f32];
    g.queue.write_buffer(&g.uniform, 0, f32_bytes(&size));

    // Vertex buffer (interleaved [x, y, r, g, b]).
    let mut vbytes: Vec<u8> = Vec::with_capacity(verts.len() * 20);
    for v in verts {
        for f in [v.pos[0], v.pos[1], v.color[0], v.color[1], v.color[2]] {
            vbytes.extend_from_slice(&f.to_le_bytes());
        }
    }
    let vbuf = g.device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("ling-verts"),
        size: vbytes.len().max(20) as u64,
        usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
        mapped_at_creation: false,
    });
    if !vbytes.is_empty() {
        g.queue.write_buffer(&vbuf, 0, &vbytes);
    }

    // Offscreen colour target.
    let tex = g.device.create_texture(&wgpu::TextureDescriptor {
        label: Some("ling-target"),
        size: wgpu::Extent3d {
            width: width as u32,
            height: height as u32,
            depth_or_array_layers: 1,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: wgpu::TextureFormat::Rgba8Unorm,
        usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
        view_formats: &[],
    });
    let view = tex.create_view(&wgpu::TextureViewDescriptor::default());

    // Readback buffer — rows padded to 256-byte alignment per wgpu rules.
    let unpadded = width * 4;
    let align = 256usize;
    let padded = unpadded.div_ceil(align) * align;
    let rb = g.device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("ling-readback"),
        size: (padded * height) as u64,
        usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
        mapped_at_creation: false,
    });

    let mut enc = g
        .device
        .create_command_encoder(&wgpu::CommandEncoderDescriptor {
            label: Some("ling-enc"),
        });
    {
        let mut rp = enc.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("ling-rp"),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view: &view,
                resolve_target: None,
                ops: wgpu::Operations {
                    load: wgpu::LoadOp::Clear(wgpu::Color {
                        r: clear[0] as f64,
                        g: clear[1] as f64,
                        b: clear[2] as f64,
                        a: 1.0,
                    }),
                    store: wgpu::StoreOp::Store,
                },
            })],
            depth_stencil_attachment: None,
            timestamp_writes: None,
            occlusion_query_set: None,
        });
        if !verts.is_empty() {
            rp.set_pipeline(&g.pipeline);
            rp.set_bind_group(0, &g.bind_group, &[]);
            rp.set_vertex_buffer(0, vbuf.slice(..));
            rp.draw(0..verts.len() as u32, 0..1);
        }
    }
    enc.copy_texture_to_buffer(
        wgpu::ImageCopyTexture {
            texture: &tex,
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
        wgpu::Extent3d {
            width: width as u32,
            height: height as u32,
            depth_or_array_layers: 1,
        },
    );
    g.queue.submit(Some(enc.finish()));

    // Map the readback buffer and copy RGBA8 → 0x00RRGGBB.
    let slice = rb.slice(..);
    let (tx, rx) = std::sync::mpsc::channel();
    slice.map_async(wgpu::MapMode::Read, move |r| {
        let _ = tx.send(r);
    });
    let _ = g.device.poll(wgpu::Maintain::Wait);
    if !matches!(rx.recv(), Ok(Ok(()))) {
        return false;
    }
    {
        let data = slice.get_mapped_range();
        let n = (width * height).min(out.len());
        for y in 0..height {
            let base = y * padded;
            for x in 0..width {
                let i = y * width + x;
                if i >= n {
                    break;
                }
                let p = base + x * 4;
                let r = data[p] as u32;
                let gg = data[p + 1] as u32;
                let b = data[p + 2] as u32;
                out[i] = (r << 16) | (gg << 8) | b;
            }
        }
    }
    rb.unmap();
    true
}

fn f32_bytes(a: &[f32]) -> &[u8] {
    // Safe: f32 has no padding/invalid bit patterns for reinterpret-as-bytes.
    unsafe { std::slice::from_raw_parts(a.as_ptr() as *const u8, std::mem::size_of_val(a)) }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn gpu_triangle_renders() {
        // A red triangle covering the centre of a 64×64 black target.
        let (w, h) = (64usize, 64usize);
        let verts = vec![
            Vert { pos: [6.0, 58.0], color: [1.0, 0.0, 0.0] },
            Vert { pos: [58.0, 58.0], color: [1.0, 0.0, 0.0] },
            Vert { pos: [32.0, 6.0], color: [1.0, 0.0, 0.0] },
        ];
        let mut out = vec![0u32; w * h];
        if !raster(&verts, w, h, [0.0, 0.0, 0.0], &mut out) {
            eprintln!("no GPU adapter available — skipping GPU raster test");
            return;
        }
        let center = out[(h / 2) * w + w / 2];
        let cr = (center >> 16) & 0xFF;
        assert!(cr > 200, "centre pixel should be red, got {center:06X}");
        // A top corner is outside the triangle → background (black).
        let corner = out[2];
        assert!(
            (corner & 0x00FF_FFFF) < 0x0010_1010,
            "corner should be ~black, got {corner:06X}"
        );
    }
}
