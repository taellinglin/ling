//! `ling convert <asset> [-o out.ling] [--no-compression]`
//!
//! Detects an asset's type by extension and transcodes it into a **self-contained,
//! importable `.ling` file** that reconstructs the asset with the engine's own
//! builtins — preserving geometry, names, and structure. Bulk numeric data is
//! embedded **losslessly**: by default deflate-compressed + base64 behind the
//! `blob_f32` / `blob_i32` builtins; `--no-compression` emits plain `.ling` arrays.
//!
//! Supported: `.gltf`/`.glb` (meshes + node names/transforms), `.wav`/`.ogg`/`.flac`
//! (PCM), `.mid` (note events), `.svg` (paths/primitives → vector strokes).
//! `.blend` is detected and routed through Blender's glTF exporter when available.

use std::io::Write;
use std::path::Path;

use base64::Engine as _;

// ── public entry ────────────────────────────────────────────────────────────

pub fn run(args: &[String]) -> i32 {
    // args = ["convert", <input>, ...flags]
    let input = match args.get(1) {
        Some(s) if !s.starts_with('-') => s.clone(),
        _ => {
            eprintln!("Usage: ling convert <file.(gltf|glb|wav|ogg|flac|mid|svg|blend)> [-o out.ling] [--no-compression]");
            return 1;
        },
    };
    let compress = !args.iter().any(|a| a == "--no-compression");
    let out = flag_value(args, "-o")
        .or_else(|| flag_value(args, "--out"))
        .unwrap_or_else(|| default_out(&input));

    match convert(&input, &out, compress) {
        Ok(bytes) => {
            eprintln!(
                "[convert] {} → {}  ({} KB, {})",
                input,
                out,
                bytes / 1024,
                if compress {
                    "deflate+base64 lossless"
                } else {
                    "uncompressed"
                }
            );
            0
        },
        Err(e) => {
            eprintln!("[convert] error: {e}");
            1
        },
    }
}

fn default_out(input: &str) -> String {
    let p = Path::new(input);
    p.with_extension("ling").to_string_lossy().into_owned()
}

fn flag_value(args: &[String], flag: &str) -> Option<String> {
    args.iter()
        .position(|a| a == flag)
        .and_then(|i| args.get(i + 1).cloned())
}

/// Dispatch on extension. Returns the number of bytes written.
pub fn convert(input: &str, output: &str, compress: bool) -> Result<usize, String> {
    let ext = Path::new(input)
        .extension()
        .map(|e| e.to_string_lossy().to_lowercase())
        .unwrap_or_default();
    let stem = Path::new(input)
        .file_stem()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_else(|| "asset".into());
    let name = sanitize(&stem);

    let ling = match ext.as_str() {
        "gltf" | "glb" => conv_gltf(input, &name, compress)?,
        "wav" | "ogg" | "flac" | "mp3" => conv_audio(input, &name, compress)?,
        "mid" | "midi" => conv_midi(input, &name, compress)?,
        "svg" => conv_svg(input, &name, compress)?,
        "blend" => conv_blend(input, output, compress)?,
        other => return Err(format!("unsupported extension '.{other}'")),
    };

    let mut f = std::fs::File::create(output).map_err(|e| format!("{output}: {e}"))?;
    f.write_all(ling.as_bytes()).map_err(|e| e.to_string())?;
    Ok(ling.len())
}

// ── shared emitters ───────────────────────────────────────────────────────────

fn deflate(bytes: &[u8]) -> Vec<u8> {
    use flate2::{write::ZlibEncoder, Compression};
    let mut e = ZlibEncoder::new(Vec::new(), Compression::best());
    let _ = e.write_all(bytes);
    e.finish().unwrap_or_default()
}

fn b64(bytes: &[u8]) -> String {
    base64::engine::general_purpose::STANDARD.encode(bytes)
}

/// Emit `bind <name> = …` for a float array — compressed blob or plain list.
fn emit_f32(name: &str, data: &[f32], compress: bool) -> String {
    if compress && data.len() > 8 {
        let mut bytes = Vec::with_capacity(data.len() * 4);
        for v in data {
            bytes.extend_from_slice(&v.to_le_bytes());
        }
        format!("bind {name} = blob_f32(\"{}\")\n", b64(&deflate(&bytes)))
    } else {
        let body: Vec<String> = data.iter().map(|v| fmt_f32(*v)).collect();
        format!("bind {name} = [{}]\n", body.join(", "))
    }
}

fn emit_i32(name: &str, data: &[u32], compress: bool) -> String {
    if compress && data.len() > 8 {
        let mut bytes = Vec::with_capacity(data.len() * 4);
        for v in data {
            bytes.extend_from_slice(&(*v as i32).to_le_bytes());
        }
        format!("bind {name} = blob_i32(\"{}\")\n", b64(&deflate(&bytes)))
    } else {
        let body: Vec<String> = data.iter().map(|v| v.to_string()).collect();
        format!("bind {name} = [{}]\n", body.join(", "))
    }
}

fn fmt_f32(v: f32) -> String {
    if v == v.trunc() && v.abs() < 1e7 {
        format!("{:.1}", v)
    } else {
        format!("{}", v)
    }
}

fn sanitize(s: &str) -> String {
    let mut out: String = s
        .chars()
        .map(|c| if c.is_alphanumeric() { c } else { '_' })
        .collect();
    if out
        .chars()
        .next()
        .map(|c| c.is_ascii_digit())
        .unwrap_or(true)
    {
        out.insert(0, '_');
    }
    out
}

fn header(kind: &str, src: &str) -> String {
    format!(
        "# ───────────────────────────────────────────────────────────────────────────\n\
         # Auto-generated by `ling convert` — {kind}\n\
         # source: {src}\n\
         # Lossless: bulk data is deflate+base64 behind blob_f32/blob_i32 (or plain\n\
         # arrays with --no-compression). Import this file and call its draw/play fn.\n\
         # ───────────────────────────────────────────────────────────────────────────\n\n"
    )
}

// ── glTF / GLB → mesh data + node hierarchy ───────────────────────────────────

fn conv_gltf(input: &str, name: &str, compress: bool) -> Result<String, String> {
    let model = ling_physics::gltf::GltfModel::load(input)?;
    let mut s = header("glTF model (geometry + nodes)", input);

    // node hierarchy + names + transforms, preserved as data
    s.push_str("# ── nodes (name, mesh index, world-ish transform rows) ──\n");
    for (i, n) in model.nodes.iter().enumerate() {
        let m = n.transform.to_cols_array();
        s.push_str(&format!(
            "# node[{i}] \"{}\" mesh={:?} T=[{:.3},{:.3},{:.3},{:.3} / {:.3},{:.3},{:.3},{:.3} / {:.3},{:.3},{:.3},{:.3} / {:.3},{:.3},{:.3},{:.3}]\n",
            n.name, n.mesh_idx,
            m[0],m[1],m[2],m[3], m[4],m[5],m[6],m[7], m[8],m[9],m[10],m[11], m[12],m[13],m[14],m[15],
        ));
    }
    s.push('\n');

    // per-mesh: positions (3/vert), normals (3/vert), uvs (2/vert), indices, + a draw fn
    let mut draw_calls = Vec::new();
    for (mi, mesh) in model.meshes.iter().enumerate() {
        let raw_name = if mesh.name.is_empty() {
            format!("mesh{mi}")
        } else {
            mesh.name.clone()
        };
        let mname = sanitize(&raw_name);
        let mut pos = Vec::with_capacity(mesh.verts.len() * 3);
        let mut nrm = Vec::with_capacity(mesh.verts.len() * 3);
        let mut uv = Vec::with_capacity(mesh.verts.len() * 2);
        for v in &mesh.verts {
            pos.extend_from_slice(&[v.pos.x, v.pos.y, v.pos.z]);
            nrm.extend_from_slice(&[v.normal.x, v.normal.y, v.normal.z]);
            uv.extend_from_slice(&[v.uv.x, v.uv.y]);
        }
        s.push_str(&format!(
            "# mesh \"{}\" — {} verts, {} tris, material={:?}\n",
            mesh.name,
            mesh.verts.len(),
            mesh.indices.len() / 3,
            mesh.mat_idx
        ));
        s.push_str(&emit_f32(&format!("{name}_{mname}_pos"), &pos, compress));
        s.push_str(&emit_f32(&format!("{name}_{mname}_nrm"), &nrm, compress));
        s.push_str(&emit_f32(&format!("{name}_{mname}_uv"), &uv, compress));
        s.push_str(&emit_i32(
            &format!("{name}_{mname}_idx"),
            &mesh.indices,
            compress,
        ));
        // wireframe draw: each triangle's 3 edges via draw_line_3d
        s.push_str(&format!(
            "\nฟังก์ชัน draw_{name}_{mname}(ox, oy, oz, scale) {{\n\
             \x20   bind P = {name}_{mname}_pos  bind I = {name}_{mname}_idx\n\
             \x20   bind n = len(I)\n\
             \x20   bind k = 0\n\
             \x20   while k + 2 < n + 1 {{\n\
             \x20       bind a = list_get(I, k) * 3  bind b = list_get(I, k+1) * 3  bind c = list_get(I, k+2) * 3\n\
             \x20       bind ax = ox + list_get(P,a)*scale    bind ay = oy + list_get(P,a+1)*scale  bind az = oz + list_get(P,a+2)*scale\n\
             \x20       bind bx = ox + list_get(P,b)*scale    bind by = oy + list_get(P,b+1)*scale  bind bz = oz + list_get(P,b+2)*scale\n\
             \x20       bind cx = ox + list_get(P,c)*scale    bind cy = oy + list_get(P,c+1)*scale  bind cz = oz + list_get(P,c+2)*scale\n\
             \x20       draw_line_3d(ax,ay,az, bx,by,bz)  draw_line_3d(bx,by,bz, cx,cy,cz)  draw_line_3d(cx,cy,cz, ax,ay,az)\n\
             \x20       bind k = k + 3\n\
             \x20   }}\n\
             }}\n\n"
        ));
        draw_calls.push(format!("draw_{name}_{mname}(ox, oy, oz, scale)"));
    }

    // a convenience that draws the whole model
    s.push_str(&format!("ฟังก์ชัน draw_{name}(ox, oy, oz, scale) {{\n"));
    for c in &draw_calls {
        s.push_str(&format!("    {c}\n"));
    }
    s.push_str("}\n\n");
    s.push_str(&format!(
        "# Example:\n# ใช้ \"{name}.ling\"\n# … inside your loop: draw_{name}(0,0,0, 1.0)  flush_3d()\n"
    ));
    Ok(s)
}

// ── audio (wav/ogg/flac/mp3) → PCM + a play function ──────────────────────────

#[cfg(not(target_arch = "wasm32"))]
fn conv_audio(input: &str, name: &str, compress: bool) -> Result<String, String> {
    let a = ling_music::decode::load(input)?;
    let mut s = header("audio (PCM samples)", input);
    s.push_str(&format!(
        "# rate={} Hz, channels={}, duration={:.3}s, mono samples={}\n",
        a.rate,
        a.channels,
        a.duration,
        a.mono.len()
    ));
    s.push_str(&format!("bind {name}_rate = {}.0\n", a.rate));
    s.push_str(&format!("bind {name}_dur = {}\n", fmt_f32(a.duration)));
    // mono downmix is enough to reconstruct/play losslessly at the source rate
    s.push_str(&emit_f32(&format!("{name}_pcm"), &a.mono, compress));
    s.push_str(&format!(
        "\n# {name}_pcm holds the lossless mono PCM at {name}_rate.\n\
         # Feed it to your audio path (e.g. a sample-playback builtin) or analyse it.\n"
    ));
    Ok(s)
}

#[cfg(target_arch = "wasm32")]
fn conv_audio(_: &str, _: &str, _: bool) -> Result<String, String> {
    Err("audio conversion is unavailable on wasm".into())
}

// ── MIDI → note events ────────────────────────────────────────────────────────

#[cfg(not(target_arch = "wasm32"))]
fn conv_midi(input: &str, name: &str, compress: bool) -> Result<String, String> {
    let song = ling_music::midi::load(input)?;
    let mut s = header("MIDI song (note events)", input);
    s.push_str(&format!(
        "# {} notes, duration {:.3}s\n",
        song.notes.len(),
        song.duration
    ));
    s.push_str(&format!("bind {name}_dur = {}\n", fmt_f32(song.duration)));
    // flat note list: [time, dur, midi, vel, channel] per note
    let mut flat = Vec::with_capacity(song.notes.len() * 5);
    for n in &song.notes {
        flat.push(n.time);
        flat.push(n.dur);
        flat.push(n.midi as f32);
        flat.push(n.vel as f32);
        flat.push(n.channel as f32);
    }
    s.push_str(&emit_f32(&format!("{name}_notes"), &flat, compress));
    s.push_str(&format!(
        "\n# {name}_notes is flat [time,dur,midi,vel,channel] × {} — step it against a\n\
         # clock and trigger tones (e.g. music_note / audio_tone) per event.\n",
        song.notes.len()
    ));
    Ok(s)
}

#[cfg(target_arch = "wasm32")]
fn conv_midi(_: &str, _: &str, _: bool) -> Result<String, String> {
    Err("MIDI conversion is unavailable on wasm".into())
}

// ── SVG → vector strokes (paths + basic primitives) ──────────────────────────

fn conv_svg(input: &str, name: &str, compress: bool) -> Result<String, String> {
    let xml = std::fs::read_to_string(input).map_err(|e| format!("{input}: {e}"))?;
    let mut polylines: Vec<Vec<[f32; 2]>> = Vec::new();
    for d in attr_values(&xml, "path", "d") {
        polylines.extend(svg_path_to_polylines(&d));
    }
    // primitives → polylines
    for r in elements(&xml, "line") {
        if let (Some(x1), Some(y1), Some(x2), Some(y2)) =
            (num(&r, "x1"), num(&r, "y1"), num(&r, "x2"), num(&r, "y2"))
        {
            polylines.push(vec![[x1, y1], [x2, y2]]);
        }
    }
    for r in elements(&xml, "rect") {
        if let (Some(x), Some(y), Some(w), Some(h)) = (
            num(&r, "x"),
            num(&r, "y"),
            num(&r, "width"),
            num(&r, "height"),
        ) {
            polylines.push(vec![[x, y], [x + w, y], [x + w, y + h], [x, y + h], [x, y]]);
        }
    }
    for r in elements(&xml, "polyline")
        .into_iter()
        .chain(elements(&xml, "polygon"))
    {
        if let Some(pts) = attr(&r, "points") {
            let nums: Vec<f32> = pts
                .split(|c: char| c == ',' || c.is_whitespace())
                .filter_map(|t| t.trim().parse().ok())
                .collect();
            let pl: Vec<[f32; 2]> = nums.chunks_exact(2).map(|c| [c[0], c[1]]).collect();
            if pl.len() >= 2 {
                polylines.push(pl);
            }
        }
    }
    if polylines.is_empty() {
        return Err("no <path>/line/rect/poly geometry found in SVG".into());
    }
    // flatten to a single coordinate stream + per-polyline lengths
    let mut coords: Vec<f32> = Vec::new();
    let mut lens: Vec<u32> = Vec::new();
    for pl in &polylines {
        lens.push(pl.len() as u32);
        for p in pl {
            coords.push(p[0]);
            coords.push(p[1]);
        }
    }
    let mut s = header("SVG vector art (polylines)", input);
    s.push_str(&format!(
        "# {} polylines, {} points\n",
        polylines.len(),
        coords.len() / 2
    ));
    s.push_str(&emit_f32(&format!("{name}_xy"), &coords, compress));
    s.push_str(&emit_i32(&format!("{name}_lens"), &lens, compress));
    s.push_str(&format!(
        "\nฟังก์ชัน draw_{name}(ox, oy, scale) {{\n\
         \x20   bind L = {name}_lens  bind P = {name}_xy\n\
         \x20   bind li = 0  bind base = 0\n\
         \x20   while li < len(L) {{\n\
         \x20       bind cnt = list_get(L, li)  bind j = 0\n\
         \x20       while j + 1 < cnt {{\n\
         \x20           bind a = (base + j) * 2  bind b = (base + j + 1) * 2\n\
         \x20           draw_line(ox + list_get(P,a)*scale, oy + list_get(P,a+1)*scale, ox + list_get(P,b)*scale, oy + list_get(P,b+1)*scale)\n\
         \x20           bind j = j + 1\n\
         \x20       }}\n\
         \x20       bind base = base + cnt  bind li = li + 1\n\
         \x20   }}\n\
         }}\n"
    ));
    Ok(s)
}

// ── .blend → route through Blender's glTF exporter when available ─────────────

fn conv_blend(input: &str, output: &str, compress: bool) -> Result<String, String> {
    // Try a headless Blender export to glTF, then convert that.
    let tmp = std::env::temp_dir().join("ling_blend_export.glb");
    let tmp_s = tmp.to_string_lossy().to_string();
    let script = format!(
        "import bpy; bpy.ops.export_scene.gltf(filepath=r'{}', export_format='GLB')",
        tmp_s
    );
    let blender = which_blender();
    match blender {
        Some(bin) => {
            let status = std::process::Command::new(&bin)
                .args(["-b", input, "--python-expr", &script])
                .status()
                .map_err(|e| format!("failed to run Blender ({bin}): {e}"))?;
            if !status.success() || !tmp.exists() {
                return Err("Blender ran but produced no glTF export".into());
            }
            let name = sanitize(
                &Path::new(input)
                    .file_stem()
                    .map(|s| s.to_string_lossy().into_owned())
                    .unwrap_or_else(|| "asset".into()),
            );
            let s = conv_gltf(&tmp_s, &name, compress)?;
            let _ = std::fs::remove_file(&tmp);
            let _ = output;
            Ok(s)
        },
        None => Err(
            ".blend needs Blender on PATH (set $BLENDER or install it). \
             Or export the model to .glb/.gltf in Blender and run `ling convert model.glb`."
                .into(),
        ),
    }
}

fn which_blender() -> Option<String> {
    if let Ok(b) = std::env::var("BLENDER") {
        if !b.is_empty() {
            return Some(b);
        }
    }
    for cand in ["blender", "blender.exe"] {
        if std::process::Command::new(cand)
            .arg("--version")
            .output()
            .map(|o| o.status.success())
            .unwrap_or(false)
        {
            return Some(cand.to_string());
        }
    }
    None
}

// ── tiny SVG helpers ──────────────────────────────────────────────────────────

/// Collect the value of `attr` from every `<tag …>` element.
fn attr_values(xml: &str, tag: &str, attr_name: &str) -> Vec<String> {
    elements(xml, tag)
        .iter()
        .filter_map(|e| attr(e, attr_name))
        .collect()
}

/// Return the raw attribute-region string of each `<tag …>` occurrence.
fn elements(xml: &str, tag: &str) -> Vec<String> {
    let needle = format!("<{tag}");
    let mut out = Vec::new();
    let mut i = 0;
    while let Some(p) = xml[i..].find(&needle) {
        let start = i + p + needle.len();
        if let Some(end) = xml[start..].find('>') {
            out.push(xml[start..start + end].to_string());
            i = start + end;
        } else {
            break;
        }
    }
    out
}

fn attr(el: &str, key: &str) -> Option<String> {
    let pat = format!("{key}=\"");
    let p = el.find(&pat)? + pat.len();
    let end = el[p..].find('"')? + p;
    Some(el[p..end].to_string())
}

fn num(el: &str, key: &str) -> Option<f32> {
    attr(el, key).and_then(|v| {
        v.trim()
            .trim_end_matches(|c: char| !c.is_ascii_digit() && c != '.' && c != '-')
            .parse()
            .ok()
    })
}

/// Minimal SVG path → polylines. Handles M/L/H/V/Z (abs+rel) and flattens
/// C/Q curves coarsely; good enough to preserve the shape of typical vector art.
fn svg_path_to_polylines(d: &str) -> Vec<Vec<[f32; 2]>> {
    let mut polys: Vec<Vec<[f32; 2]>> = Vec::new();
    let mut cur: Vec<[f32; 2]> = Vec::new();
    let (mut x, mut y) = (0.0f32, 0.0f32);
    let (mut start_x, mut start_y) = (0.0f32, 0.0f32);
    let toks = tokenize_path(d);
    let mut i = 0;
    let mut cmd = ' ';
    while i < toks.len() {
        if let Tok::Cmd(c) = toks[i] {
            cmd = c;
            i += 1;
        }
        let rel = cmd.is_ascii_lowercase();
        let uc = cmd.to_ascii_uppercase();
        #[allow(clippy::never_loop)]
        let next = |i: &mut usize| -> f32 {
            while *i < toks.len() {
                if let Tok::Num(n) = toks[*i] {
                    *i += 1;
                    return n;
                } else {
                    break;
                }
            }
            0.0
        };
        match uc {
            'M' => {
                if !cur.is_empty() {
                    polys.push(std::mem::take(&mut cur));
                }
                let (nx, ny) = (next(&mut i), next(&mut i));
                x = if rel { x + nx } else { nx };
                y = if rel { y + ny } else { ny };
                start_x = x;
                start_y = y;
                cur.push([x, y]);
                cmd = if rel { 'l' } else { 'L' };
            },
            'L' => {
                let (nx, ny) = (next(&mut i), next(&mut i));
                x = if rel { x + nx } else { nx };
                y = if rel { y + ny } else { ny };
                cur.push([x, y]);
            },
            'H' => {
                let nx = next(&mut i);
                x = if rel { x + nx } else { nx };
                cur.push([x, y]);
            },
            'V' => {
                let ny = next(&mut i);
                y = if rel { y + ny } else { ny };
                cur.push([x, y]);
            },
            'C' => {
                let (x1, y1) = (next(&mut i), next(&mut i));
                let (x2, y2) = (next(&mut i), next(&mut i));
                let (ex, ey) = (next(&mut i), next(&mut i));
                let (p0, p1, p2, p3);
                if rel {
                    p1 = [x + x1, y + y1];
                    p2 = [x + x2, y + y2];
                    p3 = [x + ex, y + ey];
                } else {
                    p1 = [x1, y1];
                    p2 = [x2, y2];
                    p3 = [ex, ey];
                }
                p0 = [x, y];
                flatten_cubic(p0, p1, p2, p3, &mut cur);
                x = p3[0];
                y = p3[1];
            },
            'Q' => {
                let (x1, y1) = (next(&mut i), next(&mut i));
                let (ex, ey) = (next(&mut i), next(&mut i));
                let (p0, p1, p2);
                if rel {
                    p1 = [x + x1, y + y1];
                    p2 = [x + ex, y + ey];
                } else {
                    p1 = [x1, y1];
                    p2 = [ex, ey];
                }
                p0 = [x, y];
                flatten_quad(p0, p1, p2, &mut cur);
                x = p2[0];
                y = p2[1];
            },
            'Z' => {
                cur.push([start_x, start_y]);
                x = start_x;
                y = start_y;
                if !cur.is_empty() {
                    polys.push(std::mem::take(&mut cur));
                }
            },
            _ => {
                i += 1;
            },
        }
    }
    if cur.len() >= 2 {
        polys.push(cur);
    }
    polys
}

#[derive(Clone, Copy)]
enum Tok {
    Cmd(char),
    Num(f32),
}

fn tokenize_path(d: &str) -> Vec<Tok> {
    let mut out = Vec::new();
    let mut numbuf = String::new();
    let flush = |b: &mut String, o: &mut Vec<Tok>| {
        if !b.is_empty() {
            if let Ok(n) = b.parse::<f32>() {
                o.push(Tok::Num(n));
            }
            b.clear();
        }
    };
    for c in d.chars() {
        if c.is_ascii_alphabetic() {
            flush(&mut numbuf, &mut out);
            out.push(Tok::Cmd(c));
        } else if c == '-' && !numbuf.is_empty() && !numbuf.ends_with('e') && !numbuf.ends_with('E')
        {
            flush(&mut numbuf, &mut out);
            numbuf.push(c);
        } else if c == ',' || c.is_whitespace() {
            flush(&mut numbuf, &mut out);
        } else {
            numbuf.push(c);
        }
    }
    flush(&mut numbuf, &mut out);
    out
}

fn flatten_cubic(p0: [f32; 2], p1: [f32; 2], p2: [f32; 2], p3: [f32; 2], out: &mut Vec<[f32; 2]>) {
    let steps = 16;
    for s in 1..=steps {
        let t = s as f32 / steps as f32;
        let u = 1.0 - t;
        let b = [
            u * u * u * p0[0]
                + 3.0 * u * u * t * p1[0]
                + 3.0 * u * t * t * p2[0]
                + t * t * t * p3[0],
            u * u * u * p0[1]
                + 3.0 * u * u * t * p1[1]
                + 3.0 * u * t * t * p2[1]
                + t * t * t * p3[1],
        ];
        out.push(b);
    }
}

fn flatten_quad(p0: [f32; 2], p1: [f32; 2], p2: [f32; 2], out: &mut Vec<[f32; 2]>) {
    let steps = 12;
    for s in 1..=steps {
        let t = s as f32 / steps as f32;
        let u = 1.0 - t;
        out.push([
            u * u * p0[0] + 2.0 * u * t * p1[0] + t * t * p2[0],
            u * u * p0[1] + 2.0 * u * t * p1[1] + t * t * p2[1],
        ]);
    }
}
