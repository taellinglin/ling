// src/gfx/webgl.rs — WebGL2 backend for wasm32 targets.
//
// All 3-D and 2-D draw calls project to screen space on the CPU (unchanged
// from the native path) and are accumulated in the DepthQueue.  At present()
// time the queue (already sorted back-to-front by painter's algorithm) is
// replayed here in order, so GPU depth testing is not needed or wanted.
//
// Vertex layout (5 f32 per vertex):  [screen_x, screen_y, r, g, b]

use wasm_bindgen::{JsCast, JsValue};
use web_sys::{
    WebGl2RenderingContext as Gl, WebGlBuffer, WebGlProgram, WebGlShader, WebGlTexture,
    WebGlUniformLocation,
};

thread_local! {
    static FRAME: std::cell::RefCell<u32> = std::cell::RefCell::new(0);
}

const FLOATS_PER_VERT: usize = 5;

// Ordered list of batched draw calls.  Consecutive same-type draw calls are
// merged so we minimise GPU uploads while preserving painter's sort order.
enum Batch {
    Tri(Vec<f32>),
    Line(Vec<f32>),
}

struct State {
    ctx: Gl,
    canvas: web_sys::OffscreenCanvas,
    prog: WebGlProgram,
    buf: WebGlBuffer,
    u_size: WebGlUniformLocation,
    batches: Vec<Batch>,
    width: f32,
    height: f32,
    // Software-framebuffer blit (the `present()` path): a textured fullscreen
    // quad that uploads the CPU buffer to the canvas for full native parity.
    tex_prog: WebGlProgram,
    tex: WebGlTexture,
    quad: WebGlBuffer,
    u_tex: WebGlUniformLocation,
}

thread_local! {
    static STATE: std::cell::RefCell<Option<State>> = std::cell::RefCell::new(None);
}

// ── GLSL shaders ─────────────────────────────────────────────────────────────

const VERT: &str = r#"#version 300 es
in vec2 aScreen;
in vec3 aColor;
uniform vec2 uSize;
out vec3 vColor;
void main() {
    vec2 ndc = (aScreen / uSize) * 2.0 - 1.0;
    ndc.y = -ndc.y;
    gl_Position = vec4(ndc, 0.0, 1.0);
    vColor = aColor;
}
"#;

const FRAG: &str = r#"#version 300 es
precision mediump float;
in vec3 vColor;
out vec4 fragColor;
void main() {
    fragColor = vec4(vColor, 1.0);
}
"#;

// Fullscreen-quad shaders for blitting the software framebuffer to the canvas.
const TEX_VERT: &str = r#"#version 300 es
in vec2 aPos;
out vec2 vUv;
void main() {
    // aPos spans the NDC quad [-1,1]; map to UV and flip Y so framebuffer row 0
    // (top) lands at the top of the canvas.
    vUv = vec2((aPos.x + 1.0) * 0.5, 1.0 - (aPos.y + 1.0) * 0.5);
    gl_Position = vec4(aPos, 0.0, 1.0);
}
"#;

const TEX_FRAG: &str = r#"#version 300 es
precision mediump float;
in vec2 vUv;
uniform sampler2D uTex;
out vec4 fragColor;
void main() {
    fragColor = texture(uTex, vUv);
}
"#;

// ── shader helpers ────────────────────────────────────────────────────────────

fn compile_shader(ctx: &Gl, kind: u32, src: &str) -> WebGlShader {
    let sh = ctx.create_shader(kind).expect("create_shader");
    ctx.shader_source(&sh, src);
    ctx.compile_shader(&sh);
    assert!(
        ctx.get_shader_parameter(&sh, Gl::COMPILE_STATUS)
            .as_bool()
            .unwrap_or(false),
        "shader compile: {}",
        ctx.get_shader_info_log(&sh).unwrap_or_default()
    );
    sh
}

fn link_program(ctx: &Gl, vs: &WebGlShader, fs: &WebGlShader) -> WebGlProgram {
    let prog = ctx.create_program().expect("create_program");
    ctx.attach_shader(&prog, vs);
    ctx.attach_shader(&prog, fs);
    ctx.link_program(&prog);
    assert!(
        ctx.get_program_parameter(&prog, Gl::LINK_STATUS)
            .as_bool()
            .unwrap_or(false),
        "program link: {}",
        ctx.get_program_info_log(&prog).unwrap_or_default()
    );
    prog
}

// ── public API ────────────────────────────────────────────────────────────────

/// Called once from ling-wasm before running the program.
pub fn init_canvas(canvas: web_sys::OffscreenCanvas) {
    let ctx: Gl = canvas
        .get_context("webgl2")
        .expect("get_context call")
        .expect("webgl2 unavailable")
        .dyn_into::<Gl>()
        .expect("cast to WebGl2RenderingContext");

    let vs = compile_shader(&ctx, Gl::VERTEX_SHADER, VERT);
    let fs = compile_shader(&ctx, Gl::FRAGMENT_SHADER, FRAG);
    let prog = link_program(&ctx, &vs, &fs);
    let buf = ctx.create_buffer().expect("create_buffer");

    // No GPU depth testing — painter's algorithm in the CPU depth queue
    // already sorts draw calls back-to-front.  WebGL draw order handles the rest.
    ctx.disable(Gl::DEPTH_TEST);

    let u_size = ctx
        .get_uniform_location(&prog, "uSize")
        .expect("uniform uSize not found");

    // Framebuffer-blit pipeline: textured fullscreen quad.
    let tvs = compile_shader(&ctx, Gl::VERTEX_SHADER, TEX_VERT);
    let tfs = compile_shader(&ctx, Gl::FRAGMENT_SHADER, TEX_FRAG);
    let tex_prog = link_program(&ctx, &tvs, &tfs);
    let u_tex = ctx
        .get_uniform_location(&tex_prog, "uTex")
        .expect("uniform uTex not found");
    let tex = ctx.create_texture().expect("create_texture");
    let quad = ctx.create_buffer().expect("create_buffer quad");
    ctx.bind_buffer(Gl::ARRAY_BUFFER, Some(&quad));
    {
        // Two triangles covering NDC [-1,1]².
        let verts: [f32; 12] = [
            -1.0, -1.0, 1.0, -1.0, 1.0, 1.0, -1.0, -1.0, 1.0, 1.0, -1.0, 1.0,
        ];
        // Safety: `verts` outlives this upload call.
        unsafe {
            let arr = js_sys::Float32Array::view(&verts);
            ctx.buffer_data_with_array_buffer_view(Gl::ARRAY_BUFFER, &arr, Gl::STATIC_DRAW);
        }
    }

    let width = canvas.width() as f32;
    let height = canvas.height() as f32;
    ctx.viewport(0, 0, canvas.width() as i32, canvas.height() as i32);

    STATE.with(|cell| {
        *cell.borrow_mut() = Some(State {
            ctx,
            canvas,
            prog,
            buf,
            u_size,
            batches: Vec::new(),
            width,
            height,
            tex_prog,
            tex,
            quad,
            u_tex,
        });
    });
}

/// Return the current canvas backing-store dimensions.
pub fn canvas_size() -> (u32, u32) {
    STATE.with(|cell| {
        cell.borrow()
            .as_ref()
            .map(|s| (s.canvas.width(), s.canvas.height()))
            .unwrap_or((800, 600))
    })
}

/// Resize both the OffscreenCanvas backing store and the WebGL viewport.
pub fn resize(width: u32, height: u32) {
    STATE.with(|cell| {
        if let Some(s) = cell.borrow_mut().as_mut() {
            s.canvas.set_width(width);
            s.canvas.set_height(height);
            s.width = width as f32;
            s.height = height as f32;
            s.ctx.viewport(0, 0, width as i32, height as i32);
        }
    });
}

#[inline]
fn unpack(color: u32) -> (f32, f32, f32) {
    (
        ((color >> 16) & 0xFF) as f32 / 255.0,
        ((color >> 8) & 0xFF) as f32 / 255.0,
        (color & 0xFF) as f32 / 255.0,
    )
}

/// Queue a projected screen-space triangle for this frame.
/// Consecutive triangles are merged into one GPU batch to reduce draw calls
/// while preserving painter's sort order from the depth queue.
pub fn push_triangle(
    color: u32,
    x0: f32,
    y0: f32,
    x1: f32,
    y1: f32,
    x2: f32,
    y2: f32,
    _depth: f32,
) {
    STATE.with(|cell| {
        if let Some(s) = cell.borrow_mut().as_mut() {
            let (r, g, b) = unpack(color);
            let verts = [x0, y0, r, g, b, x1, y1, r, g, b, x2, y2, r, g, b];
            match s.batches.last_mut() {
                Some(Batch::Tri(v)) => v.extend_from_slice(&verts),
                _ => s.batches.push(Batch::Tri(verts.to_vec())),
            }
        }
    });
}

/// Queue a projected screen-space line for this frame.
pub fn push_line(color: u32, x0: f32, y0: f32, x1: f32, y1: f32, _depth: f32) {
    STATE.with(|cell| {
        if let Some(s) = cell.borrow_mut().as_mut() {
            let (r, g, b) = unpack(color);
            let verts = [x0, y0, r, g, b, x1, y1, r, g, b];
            match s.batches.last_mut() {
                Some(Batch::Line(v)) => v.extend_from_slice(&verts),
                _ => s.batches.push(Batch::Line(verts.to_vec())),
            }
        }
    });
}

fn upload_and_draw(
    ctx: &Gl,
    prog: &WebGlProgram,
    buf: &WebGlBuffer,
    u_size: &WebGlUniformLocation,
    verts: &[f32],
    w: f32,
    h: f32,
    mode: u32,
) {
    ctx.bind_buffer(Gl::ARRAY_BUFFER, Some(buf));

    // Safety: slice lives for the duration of this call.
    unsafe {
        let arr = js_sys::Float32Array::view(verts);
        ctx.buffer_data_with_array_buffer_view(Gl::ARRAY_BUFFER, &arr, Gl::DYNAMIC_DRAW);
    }

    ctx.use_program(Some(prog));
    ctx.uniform2f(Some(u_size), w, h);

    let stride = (FLOATS_PER_VERT * 4) as i32; // 5 floats × 4 bytes = 20

    let loc_screen = ctx.get_attrib_location(prog, "aScreen") as u32;
    let loc_color = ctx.get_attrib_location(prog, "aColor") as u32;

    ctx.enable_vertex_attrib_array(loc_screen);
    ctx.vertex_attrib_pointer_with_i32(loc_screen, 2, Gl::FLOAT, false, stride, 0);

    ctx.enable_vertex_attrib_array(loc_color);
    ctx.vertex_attrib_pointer_with_i32(loc_color, 3, Gl::FLOAT, false, stride, 8);

    ctx.draw_arrays(mode, 0, (verts.len() / FLOATS_PER_VERT) as i32);
}

/// Clear the canvas and replay all queued geometry in painter's sort order.
pub fn flush(fill_r: f32, fill_g: f32, fill_b: f32, width: usize, height: usize) {
    STATE.with(|cell| {
        let mut opt = cell.borrow_mut();
        if let Some(s) = opt.as_mut() {
            let ctx = &s.ctx;
            ctx.clear_color(fill_r, fill_g, fill_b, 1.0);
            ctx.clear(Gl::COLOR_BUFFER_BIT);

            let w = width  as f32;
            let h = height as f32;
            let batches = std::mem::take(&mut s.batches);

            // Log first 3 frames so the developer can confirm rendering is active.
            FRAME.with(|fc| {
                let n = *fc.borrow();
                if n < 3 {
                    web_sys::console::log_1(&JsValue::from_str(&format!(
                        "[ling webgl] frame={n} batches={} canvas={}x{} fill=({fill_r:.2},{fill_g:.2},{fill_b:.2})",
                        batches.len(), w as u32, h as u32,
                    )));
                }
                *fc.borrow_mut() = n + 1;
            });

            for batch in &batches {
                match batch {
                    Batch::Tri(verts)  =>
                        upload_and_draw(ctx, &s.prog, &s.buf, &s.u_size, verts, w, h, Gl::TRIANGLES),
                    Batch::Line(verts) =>
                        upload_and_draw(ctx, &s.prog, &s.buf, &s.u_size, verts, w, h, Gl::LINES),
                }
            }

            // Flush GPU command buffer so the OffscreenCanvas compositor
            // can read the finished frame (required by the OffscreenCanvas spec).
            ctx.flush();
        }
    });
}

/// Upload the software framebuffer (`0x00RRGGBB`, row-major, top-left origin) as
/// a texture and draw it across the whole canvas. This is the `present()` path:
/// everything the CPU rasteriser produced — 3-D meshes, 2-D shapes, vector
/// textures, holographic UI — shows on the web exactly as it does natively.
pub fn blit_rgb(buffer: &[u32], width: usize, height: usize) {
    if width == 0 || height == 0 {
        return;
    }
    STATE.with(|cell| {
        let mut opt = cell.borrow_mut();
        let Some(s) = opt.as_mut() else { return };
        let ctx = &s.ctx;

        // 0x00RRGGBB → tightly packed RGBA8 (opaque).
        let n = width * height;
        let mut px = Vec::with_capacity(n * 4);
        for i in 0..n {
            let c = buffer.get(i).copied().unwrap_or(0);
            px.push(((c >> 16) & 0xFF) as u8);
            px.push(((c >> 8) & 0xFF) as u8);
            px.push((c & 0xFF) as u8);
            px.push(255);
        }

        ctx.active_texture(Gl::TEXTURE0);
        ctx.bind_texture(Gl::TEXTURE_2D, Some(&s.tex));
        ctx.tex_parameteri(Gl::TEXTURE_2D, Gl::TEXTURE_MIN_FILTER, Gl::NEAREST as i32);
        ctx.tex_parameteri(Gl::TEXTURE_2D, Gl::TEXTURE_MAG_FILTER, Gl::NEAREST as i32);
        ctx.tex_parameteri(Gl::TEXTURE_2D, Gl::TEXTURE_WRAP_S, Gl::CLAMP_TO_EDGE as i32);
        ctx.tex_parameteri(Gl::TEXTURE_2D, Gl::TEXTURE_WRAP_T, Gl::CLAMP_TO_EDGE as i32);
        let _ = ctx.tex_image_2d_with_i32_and_i32_and_i32_and_format_and_type_and_opt_u8_array(
            Gl::TEXTURE_2D,
            0,
            Gl::RGBA as i32,
            width as i32,
            height as i32,
            0,
            Gl::RGBA,
            Gl::UNSIGNED_BYTE,
            Some(&px),
        );

        ctx.use_program(Some(&s.tex_prog));
        ctx.uniform1i(Some(&s.u_tex), 0);
        ctx.bind_buffer(Gl::ARRAY_BUFFER, Some(&s.quad));
        let loc = ctx.get_attrib_location(&s.tex_prog, "aPos") as u32;
        ctx.enable_vertex_attrib_array(loc);
        ctx.vertex_attrib_pointer_with_i32(loc, 2, Gl::FLOAT, false, 0, 0);
        ctx.draw_arrays(Gl::TRIANGLES, 0, 6);

        // Required so the OffscreenCanvas compositor reads the finished frame.
        ctx.flush();
    });
}
