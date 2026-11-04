// src/gfx/webgl_scene.rs — draws the depth queue's projected triangles on the GPU (WebGL2, wasm only).
//
// The engine still projects, clips and sorts on the CPU. Instead of filling every
// triangle in software, `DepthQueue::flush` packs them into a `ling_gl::Scene`
// and hands it to `submit`. At present time `webgl::blit_rgb` uploads the CPU frame
// (sky, post effects, anything drawn on the CPU) and calls `draw`, which renders the
// scene over it with a hardware depth buffer, textures, per-pixel point lights and fog.
// Same maths as `ling-gl`'s WGSL shader.

use ling_gl::{Blend, Scene, Vertex, ZFAR};
use std::cell::{Cell, RefCell};
use std::collections::{HashMap, HashSet};
use web_sys::{WebGl2RenderingContext as Gl, WebGlBuffer, WebGlProgram, WebGlTexture, WebGlUniformLocation};

const FLOATS_PER_VERT: usize = std::mem::size_of::<Vertex>() / 4;
const MAX_KNOWN_TEXTURES: usize = 400;

const VERT: &str = r#"#version 300 es
layout(location = 0) in vec2 aXY;
layout(location = 1) in float aIw;
layout(location = 2) in vec2 aUvw;
layout(location = 3) in vec4 aTint;
layout(location = 4) in vec3 aWpos;
uniform vec2 uSize;
out float vIw;
out vec2 vUvw;
out vec4 vTint;
out vec3 vWpos;
void main() {
    vec2 ndc = (aXY / uSize) * 2.0 - 1.0;
    gl_Position = vec4(ndc.x, -ndc.y, 0.5, 1.0);
    vIw = aIw;
    vUvw = aUvw;
    vTint = aTint;
    vWpos = aWpos;
}
"#;

const FRAG: &str = r#"#version 300 es
precision highp float;
in float vIw;
in vec2 vUvw;
in vec4 vTint;
in vec3 vWpos;
uniform sampler2D uTex;
uniform float uZfar;
uniform float uZd;
uniform vec3 uFogColor;
uniform float uFogStart;
uniform float uFogEnd;
uniform float uAmb;
uniform float uLit;
uniform float uN;
// light i: uLights[2i] = (x, y, z, intensity), uLights[2i+1] = (r, g, b, radius)
uniform vec4 uLights[16];
out vec4 fragColor;
void main() {
    float w = 1.0 / max(vIw, 1e-6);
    vec4 c = texture(uTex, vUvw * w) * vTint;
    if (c.a < 0.004) discard;
    vec3 rgb = c.rgb;
    if (uLit > 0.5) {
        vec3 p = vWpos * w;
        vec3 l = vec3(uAmb);
        for (int k = 0; k < 8; k++) {
            if (float(k) >= uN) break;
            vec4 a = uLights[2 * k];
            vec4 b = uLights[2 * k + 1];
            vec3 d = p - a.xyz;
            float d2 = dot(d, d);
            float r2 = b.w * b.w;
            if (d2 < r2) {
                float f = 1.0 - sqrt(d2 / r2);
                l += b.rgb * (f * f * a.w);
            }
        }
        rgb = min(rgb * l, vec3(1.0));
    }
    if (uFogEnd > uFogStart) {
        float f = clamp((w - uZd - uFogStart) / (uFogEnd - uFogStart), 0.0, 1.0);
        rgb = mix(rgb, uFogColor, f);
    }
    fragColor = vec4(rgb, c.a);
    gl_FragDepth = clamp(w / uZfar, 0.0, 1.0);
}
"#;

struct Uniforms {
    size: Option<WebGlUniformLocation>,
    tex: Option<WebGlUniformLocation>,
    zfar: Option<WebGlUniformLocation>,
    zd: Option<WebGlUniformLocation>,
    fog_color: Option<WebGlUniformLocation>,
    fog_start: Option<WebGlUniformLocation>,
    fog_end: Option<WebGlUniformLocation>,
    amb: Option<WebGlUniformLocation>,
    lit: Option<WebGlUniformLocation>,
    n: Option<WebGlUniformLocation>,
    lights: Option<WebGlUniformLocation>,
}

struct Gpu {
    prog: WebGlProgram,
    buf: WebGlBuffer,
    white: WebGlTexture,
    u: Uniforms,
    textures: HashMap<u64, WebGlTexture>,
    /// Ids already shipped to `textures` (or queued in a pending scene).
    known: HashSet<u64>,
    evict: bool,
    pending: Vec<Scene>,
}

enum Init {
    Untried,
    Failed,
    Ready(Box<Gpu>),
}

thread_local! {
    static GPU: RefCell<Init> = const { RefCell::new(Init::Untried) };
    static ENABLED: Cell<bool> = const { Cell::new(true) };
}

fn log(msg: &str) {
    web_sys::console::log_1(&wasm_bindgen::JsValue::from_str(msg));
}

fn compile(ctx: &Gl, kind: u32, src: &str) -> Result<web_sys::WebGlShader, String> {
    let sh = ctx.create_shader(kind).ok_or("create_shader failed")?;
    ctx.shader_source(&sh, src);
    ctx.compile_shader(&sh);
    if ctx.get_shader_parameter(&sh, Gl::COMPILE_STATUS).as_bool().unwrap_or(false) {
        Ok(sh)
    } else {
        Err(ctx.get_shader_info_log(&sh).unwrap_or_default())
    }
}

fn upload_texture(ctx: &Gl, w: u32, h: u32, rgba: &[u8]) -> Option<WebGlTexture> {
    let tex = ctx.create_texture()?;
    ctx.active_texture(Gl::TEXTURE0);
    ctx.bind_texture(Gl::TEXTURE_2D, Some(&tex));
    ctx.tex_parameteri(Gl::TEXTURE_2D, Gl::TEXTURE_MIN_FILTER, Gl::LINEAR as i32);
    ctx.tex_parameteri(Gl::TEXTURE_2D, Gl::TEXTURE_MAG_FILTER, Gl::LINEAR as i32);
    ctx.tex_parameteri(Gl::TEXTURE_2D, Gl::TEXTURE_WRAP_S, Gl::REPEAT as i32);
    ctx.tex_parameteri(Gl::TEXTURE_2D, Gl::TEXTURE_WRAP_T, Gl::REPEAT as i32);
    ctx.tex_image_2d_with_i32_and_i32_and_i32_and_format_and_type_and_opt_u8_array(
        Gl::TEXTURE_2D,
        0,
        Gl::RGBA as i32,
        w as i32,
        h as i32,
        0,
        Gl::RGBA,
        Gl::UNSIGNED_BYTE,
        Some(rgba),
    )
    .ok()?;
    Some(tex)
}

impl Gpu {
    fn new(ctx: &Gl) -> Result<Gpu, String> {
        let vs = compile(ctx, Gl::VERTEX_SHADER, VERT)?;
        let fs = compile(ctx, Gl::FRAGMENT_SHADER, FRAG)?;
        let prog = ctx.create_program().ok_or("create_program failed")?;
        ctx.attach_shader(&prog, &vs);
        ctx.attach_shader(&prog, &fs);
        ctx.link_program(&prog);
        if !ctx.get_program_parameter(&prog, Gl::LINK_STATUS).as_bool().unwrap_or(false) {
            return Err(ctx.get_program_info_log(&prog).unwrap_or_default());
        }
        let loc = |n: &str| ctx.get_uniform_location(&prog, n);
        let u = Uniforms {
            size: loc("uSize"),
            tex: loc("uTex"),
            zfar: loc("uZfar"),
            zd: loc("uZd"),
            fog_color: loc("uFogColor"),
            fog_start: loc("uFogStart"),
            fog_end: loc("uFogEnd"),
            amb: loc("uAmb"),
            lit: loc("uLit"),
            n: loc("uN"),
            lights: loc("uLights"),
        };
        let buf = ctx.create_buffer().ok_or("create_buffer failed")?;
        let white = upload_texture(ctx, 1, 1, &[255, 255, 255, 255]).ok_or("white texture failed")?;
        Ok(Gpu { prog, buf, white, u, textures: HashMap::new(), known: HashSet::new(), evict: false, pending: Vec::new() })
    }
}

/// Turn the GPU scene path on or off (page option `?gl=0`).
pub fn set_enabled(on: bool) {
    ENABLED.with(|e| e.set(on));
}

/// True once the WebGL2 context and shaders are up and the path is enabled.
pub fn ready() -> bool {
    if !ENABLED.with(|e| e.get()) {
        return false;
    }
    GPU.with(|g| {
        let mut g = g.borrow_mut();
        if matches!(*g, Init::Untried) {
            match super::webgl::with_ctx(Gpu::new) {
                // The canvas isn't initialised yet; try again next frame.
                None => return false,
                Some(Ok(gpu)) => {
                    log("[ling webgl] GPU scene renderer ready");
                    *g = Init::Ready(Box::new(gpu));
                },
                Some(Err(e)) => {
                    log(&format!("[ling webgl] GPU scene renderer unavailable, using the CPU rasteriser: {e}"));
                    *g = Init::Failed;
                },
            }
        }
        matches!(*g, Init::Ready(_))
    })
}

/// Queue this frame's sorted draw calls; `draw` renders them at present time.
pub fn submit(calls: &[super::depth::DrawCall]) {
    GPU.with(|g| {
        if let Init::Ready(gpu) = &mut *g.borrow_mut() {
            if gpu.known.len() > MAX_KNOWN_TEXTURES {
                gpu.known.clear();
                gpu.evict = true;
            }
            let scene = super::depth::gl_scene::build(calls, &mut gpu.known);
            gpu.pending.push(scene);
        }
    });
}

fn as_f32s(verts: &[Vertex]) -> &[f32] {
    // Vertex is repr(C) and made only of f32 fields.
    unsafe { std::slice::from_raw_parts(verts.as_ptr() as *const f32, verts.len() * FLOATS_PER_VERT) }
}

/// Render the queued scenes into the current framebuffer, over what is already there.
pub fn draw(ctx: &Gl, width: usize, height: usize) {
    GPU.with(|g| {
        let mut g = g.borrow_mut();
        let Init::Ready(gpu) = &mut *g else { return };
        if gpu.pending.is_empty() {
            return;
        }
        if gpu.evict {
            for (_, t) in gpu.textures.drain() {
                ctx.delete_texture(Some(&t));
            }
            gpu.evict = false;
        }
        let scenes = std::mem::take(&mut gpu.pending);

        ctx.use_program(Some(&gpu.prog));
        ctx.uniform2f(gpu.u.size.as_ref(), width as f32, height as f32);
        ctx.uniform1f(gpu.u.zfar.as_ref(), ZFAR);
        ctx.uniform1i(gpu.u.tex.as_ref(), 0);
        ctx.enable(Gl::DEPTH_TEST);
        ctx.depth_func(Gl::LEQUAL);

        ctx.bind_buffer(Gl::ARRAY_BUFFER, Some(&gpu.buf));
        let stride = (FLOATS_PER_VERT * 4) as i32;
        for (loc, size, off) in [(0u32, 2, 0), (1, 1, 8), (2, 2, 12), (3, 4, 20), (4, 3, 36)] {
            ctx.enable_vertex_attrib_array(loc);
            ctx.vertex_attrib_pointer_with_i32(loc, size, Gl::FLOAT, false, stride, off);
        }

        for scene in &scenes {
            for t in &scene.textures {
                let n = t.w as usize * t.h as usize;
                if t.w > 0 && t.h > 0 && t.px.len() >= n && !gpu.textures.contains_key(&t.id) {
                    if let Some(tex) = upload_texture(ctx, t.w, t.h, &ling_gl::argb_to_rgba(&t.px[..n])) {
                        gpu.textures.insert(t.id, tex);
                    }
                }
            }

            ctx.depth_mask(true);
            ctx.clear(Gl::DEPTH_BUFFER_BIT);
            ctx.uniform1f(gpu.u.zd.as_ref(), scene.zdist);

            let total: usize = scene.batches.iter().map(|b| b.verts.len() / 3 * 3).sum();
            if total == 0 {
                continue;
            }
            ctx.buffer_data_with_i32(Gl::ARRAY_BUFFER, (total * FLOATS_PER_VERT * 4) as i32, Gl::DYNAMIC_DRAW);
            let mut first = 0usize;
            let mut ranges: Vec<(usize, usize)> = Vec::with_capacity(scene.batches.len());
            for b in &scene.batches {
                let n = b.verts.len() / 3 * 3;
                if n > 0 {
                    let data = as_f32s(&b.verts[..n]);
                    // Safety: the view is consumed by the call below, before anything can reallocate wasm memory.
                    unsafe {
                        let view = js_sys::Float32Array::view(data);
                        ctx.buffer_sub_data_with_i32_and_array_buffer_view(
                            Gl::ARRAY_BUFFER,
                            (first * FLOATS_PER_VERT * 4) as i32,
                            &view,
                        );
                    }
                }
                ranges.push((first, n));
                first += n;
            }

            let mut lights = [0.0f32; 64];
            for (b, (first, n)) in scene.batches.iter().zip(&ranges) {
                if *n == 0 {
                    continue;
                }
                match b.blend {
                    Blend::Opaque => {
                        ctx.disable(Gl::BLEND);
                        ctx.depth_mask(true);
                    },
                    Blend::Alpha => {
                        ctx.enable(Gl::BLEND);
                        ctx.blend_func_separate(Gl::SRC_ALPHA, Gl::ONE_MINUS_SRC_ALPHA, Gl::ONE, Gl::ONE_MINUS_SRC_ALPHA);
                        ctx.depth_mask(false);
                    },
                    Blend::Additive => {
                        ctx.enable(Gl::BLEND);
                        ctx.blend_func_separate(Gl::SRC_ALPHA, Gl::ONE, Gl::ZERO, Gl::ONE);
                        ctx.depth_mask(false);
                    },
                }
                ctx.uniform3f(gpu.u.fog_color.as_ref(), b.fog.color[0], b.fog.color[1], b.fog.color[2]);
                ctx.uniform1f(gpu.u.fog_start.as_ref(), b.fog.start);
                ctx.uniform1f(gpu.u.fog_end.as_ref(), b.fog.end);
                ctx.uniform1f(gpu.u.amb.as_ref(), b.lighting.amb);
                ctx.uniform1f(gpu.u.lit.as_ref(), if b.lighting.lit { 1.0 } else { 0.0 });
                let nl = b.lighting.lights.len().min(ling_gl::MAX_LIGHTS);
                ctx.uniform1f(gpu.u.n.as_ref(), nl as f32);
                if nl > 0 {
                    lights.fill(0.0);
                    for (i, l) in b.lighting.lights.iter().take(nl).enumerate() {
                        lights[i * 8..i * 8 + 8].copy_from_slice(&[l[0], l[1], l[2], l[6], l[3], l[4], l[5], l[7]]);
                    }
                    ctx.uniform4fv_with_f32_array(gpu.u.lights.as_ref(), &lights);
                }
                let tex = b.texture.and_then(|id| gpu.textures.get(&id)).unwrap_or(&gpu.white);
                ctx.active_texture(Gl::TEXTURE0);
                ctx.bind_texture(Gl::TEXTURE_2D, Some(tex));
                ctx.draw_arrays(Gl::TRIANGLES, *first as i32, *n as i32);
            }
        }

        // Leave the state the engine's blit path expects.
        for loc in 0..5u32 {
            ctx.disable_vertex_attrib_array(loc);
        }
        ctx.disable(Gl::BLEND);
        ctx.depth_mask(true);
        ctx.disable(Gl::DEPTH_TEST);
    });
}
