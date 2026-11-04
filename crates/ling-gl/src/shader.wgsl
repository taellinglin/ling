struct Frame { size: vec2<f32>, zfar: f32, zd: f32 };
struct Batch {
    fog_color: vec3<f32>,
    fog_start: f32,
    fog_end: f32,
    amb: f32,
    lit: f32,
    nlights: f32,
    // light i: lights[2i] = (x, y, z, intensity), lights[2i+1] = (r, g, b, radius)
    lights: array<vec4<f32>, 16>,
};
@group(0) @binding(0) var<uniform> frame: Frame;
@group(1) @binding(0) var<uniform> batch: Batch;
@group(1) @binding(1) var tex: texture_2d<f32>;
@group(1) @binding(2) var samp: sampler;

struct VOut {
    @builtin(position) pos: vec4<f32>,
    @location(0) iw: f32,
    @location(1) uvw: vec2<f32>,
    @location(2) tint: vec4<f32>,
    @location(3) wpos: vec3<f32>,
};

@vertex
fn vs(@location(0) xy: vec2<f32>, @location(1) iw: f32, @location(2) uvw: vec2<f32>, @location(3) tint: vec4<f32>, @location(4) wpos: vec3<f32>) -> VOut {
    var o: VOut;
    let ndc = (xy / frame.size) * 2.0 - vec2<f32>(1.0, 1.0);
    o.pos = vec4<f32>(ndc.x, -ndc.y, 0.5, 1.0);
    o.iw = iw;
    o.uvw = uvw;
    o.tint = tint;
    o.wpos = wpos;
    return o;
}

struct FOut {
    @location(0) color: vec4<f32>,
    @builtin(frag_depth) depth: f32,
};

@fragment
fn fs(i: VOut) -> FOut {
    var o: FOut;
    let iw = max(i.iw, 1e-6);
    let w = 1.0 / iw;
    let uv = i.uvw * w;
    let c = textureSample(tex, samp, uv) * i.tint;
    if (c.a < 0.004) {
        discard;
    }
    var rgb = c.rgb;
    if (batch.lit > 0.5) {
        let p = i.wpos * w;
        var l = vec3<f32>(batch.amb);
        for (var k = 0u; k < u32(batch.nlights); k = k + 1u) {
            let a = batch.lights[2u * k];
            let b = batch.lights[2u * k + 1u];
            let d = p - a.xyz;
            let d2 = dot(d, d);
            let r2 = b.w * b.w;
            if (d2 < r2) {
                let f = 1.0 - sqrt(d2 / r2);
                l = l + b.rgb * (f * f * a.w);
            }
        }
        rgb = min(rgb * l, vec3<f32>(1.0));
    }
    if (batch.fog_end > batch.fog_start) {
        let f = clamp((w - frame.zd - batch.fog_start) / (batch.fog_end - batch.fog_start), 0.0, 1.0);
        rgb = mix(rgb, batch.fog_color, f);
    }
    o.color = vec4<f32>(rgb, c.a);
    o.depth = clamp(w / frame.zfar, 0.0, 1.0);
    return o;
}
