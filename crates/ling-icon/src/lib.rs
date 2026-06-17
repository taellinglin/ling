//! ling-icon — turn an icon source (`.svg`, `.png`/`.jpg`/`.bmp`, or `.ico`) into
//! a multi-resolution Windows `.ico`, and embed it into an executable from a
//! build script.
//!
//! SVG is rendered faithfully (filters, gradients, blur) via `resvg`; raster
//! inputs are resized with `image`; an existing `.ico` is passed through. The
//! crate is used three ways:
//!   * the Ling toolchain's own `build.rs` (icons for `ling.exe`, `lingfu.exe`),
//!   * the `ling` binary at `ling build` time (rasterizes the user's icon),
//!   * the generated app crate's `build.rs` (embeds the produced `.ico`).

use std::path::Path;

/// Standard icon sizes packed into the `.ico` (Explorer picks the best one).
pub const DEFAULT_SIZES: &[u32] = &[16, 32, 48, 64, 128, 256];

/// Build multi-resolution `.ico` bytes from any supported source file.
pub fn ico_bytes(src: &Path, sizes: &[u32]) -> Result<Vec<u8>, String> {
    let ext = src
        .extension()
        .and_then(|e| e.to_str())
        .map(|e| e.to_ascii_lowercase())
        .unwrap_or_default();

    match ext.as_str() {
        // Already an icon — trust it as-is.
        "ico" => std::fs::read(src).map_err(|e| format!("read {}: {e}", src.display())),
        "svg" | "svgz" => svg_to_ico(src, sizes),
        _ => raster_to_ico(src, sizes),
    }
}

/// Build the `.ico` and write it to `out`.
pub fn write_ico(src: &Path, out: &Path, sizes: &[u32]) -> Result<(), String> {
    let bytes = ico_bytes(src, sizes)?;
    std::fs::write(out, bytes).map_err(|e| format!("write {}: {e}", out.display()))
}

// ── SVG → ICO ────────────────────────────────────────────────────────────────

fn svg_to_ico(src: &Path, sizes: &[u32]) -> Result<Vec<u8>, String> {
    use resvg::{tiny_skia, usvg};

    let data = std::fs::read(src).map_err(|e| format!("read {}: {e}", src.display()))?;
    let opt = usvg::Options::default();
    let tree = usvg::Tree::from_data(&data, &opt)
        .map_err(|e| format!("parse SVG {}: {e}", src.display()))?;
    let svg_size = tree.size();
    let (sw, sh) = (svg_size.width(), svg_size.height());

    let mut dir = ico::IconDir::new(ico::ResourceType::Icon);
    for &n in sizes {
        let n = n.clamp(1, 256);
        let mut pixmap =
            tiny_skia::Pixmap::new(n, n).ok_or_else(|| format!("alloc {n}x{n} pixmap"))?;
        // Scale uniformly to fit the square and centre it.
        let scale = (n as f32 / sw).min(n as f32 / sh);
        let tx = (n as f32 - sw * scale) / 2.0;
        let ty = (n as f32 - sh * scale) / 2.0;
        let transform = tiny_skia::Transform::from_scale(scale, scale).post_translate(tx, ty);
        resvg::render(&tree, transform, &mut pixmap.as_mut());

        // tiny_skia is premultiplied; encode_png writes straight-alpha PNG.
        let png = pixmap
            .encode_png()
            .map_err(|e| format!("encode png: {e}"))?;
        let image = ico::IconImage::read_png(&png[..]).map_err(|e| format!("ico read_png: {e}"))?;
        dir.add_entry(ico::IconDirEntry::encode(&image).map_err(|e| format!("ico encode: {e}"))?);
    }
    let mut buf = Vec::new();
    dir.write(&mut buf).map_err(|e| format!("write ico: {e}"))?;
    Ok(buf)
}

// ── Raster (PNG/JPG/BMP) → ICO ────────────────────────────────────────────────

fn raster_to_ico(src: &Path, sizes: &[u32]) -> Result<Vec<u8>, String> {
    let img = image::open(src).map_err(|e| format!("open {}: {e}", src.display()))?;
    let mut dir = ico::IconDir::new(ico::ResourceType::Icon);
    for &n in sizes {
        let n = n.clamp(1, 256);
        let scaled = img.resize_exact(n, n, image::imageops::FilterType::Lanczos3);
        let rgba = scaled.to_rgba8();
        let image = ico::IconImage::from_rgba_data(n, n, rgba.into_raw());
        dir.add_entry(ico::IconDirEntry::encode(&image).map_err(|e| format!("ico encode: {e}"))?);
    }
    let mut buf = Vec::new();
    dir.write(&mut buf).map_err(|e| format!("write ico: {e}"))?;
    Ok(buf)
}

// ── Build-script embedding ────────────────────────────────────────────────────

/// From a **build script**, embed `ico_path` as the executable icon for every
/// binary in the current crate. No-op on non-Windows targets.
#[cfg(windows)]
pub fn embed_in_build(ico_path: &Path) -> Result<(), String> {
    let path = ico_path.to_str().ok_or("icon path is not valid UTF-8")?;
    let mut res = winresource::WindowsResource::new();
    res.set_icon(path);
    res.compile()
        .map_err(|e| format!("winresource compile: {e}"))
}

#[cfg(not(windows))]
pub fn embed_in_build(_ico_path: &Path) -> Result<(), String> {
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use resvg::{tiny_skia, usvg};

    // Render an inline SVG straight to ICO bytes (mirrors `svg_to_ico` without a
    // file on disk) and assert the output is a well-formed icon.
    fn inline_svg_to_ico(svg: &str, sizes: &[u32]) -> Vec<u8> {
        let opt = usvg::Options::default();
        let tree = usvg::Tree::from_data(svg.as_bytes(), &opt).expect("parse svg");
        let sz = tree.size();
        let mut dir = ico::IconDir::new(ico::ResourceType::Icon);
        for &n in sizes {
            let mut pm = tiny_skia::Pixmap::new(n, n).unwrap();
            let scale = (n as f32 / sz.width()).min(n as f32 / sz.height());
            resvg::render(
                &tree,
                tiny_skia::Transform::from_scale(scale, scale),
                &mut pm.as_mut(),
            );
            let png = pm.encode_png().unwrap();
            let img = ico::IconImage::read_png(&png[..]).unwrap();
            dir.add_entry(ico::IconDirEntry::encode(&img).unwrap());
        }
        let mut buf = Vec::new();
        dir.write(&mut buf).unwrap();
        buf
    }

    #[test]
    fn svg_renders_to_valid_ico() {
        let svg = r##"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 100 100">
            <rect width="100" height="100" fill="#2244ff"/>
            <circle cx="50" cy="50" r="30" fill="#ffcc00"/>
        </svg>"##;
        let ico = inline_svg_to_ico(svg, &[16, 32, 48]);
        // ICONDIR header: reserved=0, type=1 (icon), count=3.
        assert_eq!(&ico[0..2], &[0, 0], "reserved field");
        assert_eq!(&ico[2..4], &[1, 0], "type = icon");
        assert_eq!(&ico[4..6], &[3, 0], "three images packed");
        assert!(ico.len() > 100, "ico should contain image data");
    }
}

/// Convenience for build scripts: rasterize `src` → `OUT_DIR/<stem>.ico` and
/// embed it. Returns the path of the produced `.ico`. Errors are returned so the
/// caller can warn-and-continue (a missing icon should never fail a build).
pub fn rasterize_and_embed(src: &Path, out_dir: &Path) -> Result<std::path::PathBuf, String> {
    let stem = src.file_stem().and_then(|s| s.to_str()).unwrap_or("app");
    let out = out_dir.join(format!("{stem}.ico"));
    write_ico(src, &out, DEFAULT_SIZES)?;
    embed_in_build(&out)?;
    Ok(out)
}
