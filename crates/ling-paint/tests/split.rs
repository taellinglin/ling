//! Picture splitting end to end: a synthetic anime-style picture (sky, hills, two outlined + shaded
//! characters) is split every way; the layers must stack back to the original picture, and the stage parts
//! must leave no hole when the camera turns. Pictures land in $LINGPAINT_OUT (or the temp dir).
use ling_paint::{Studio, Val};

fn n(v: f64) -> Val {
    Val::Num(v)
}
fn s(v: &str) -> Val {
    Val::Str(v.into())
}
fn call(st: &mut Studio, name: &str, a: &[Val]) -> Val {
    st.call(name, a).unwrap_or_else(|| panic!("unknown call {name}")).unwrap()
}
fn num(v: Val) -> f64 {
    match v {
        Val::Num(x) => x,
        _ => f64::NAN,
    }
}
fn out(name: &str) -> String {
    let d = std::env::var("LINGPAINT_OUT").unwrap_or_else(|_| std::env::temp_dir().to_string_lossy().to_string());
    std::path::Path::new(&d).join(name).to_string_lossy().to_string()
}

/// sky gradient, a hill, and two cel-shaded blobs with dark antialiased outlines
fn picture(w: u32, h: u32) -> image::RgbaImage {
    let mut img = image::RgbaImage::new(w, h);
    let blobs = [(0.32f32, 0.58f32, 0.17f32, [0.95f32, 0.55, 0.62]), (0.7, 0.66, 0.13, [0.45, 0.7, 0.95])];
    for y in 0..h {
        for x in 0..w {
            let (fx, fy) = (x as f32 / w as f32, y as f32 / h as f32);
            // sky -> hill
            let sky = [0.55 + 0.4 * fy, 0.6 + 0.1 * fy, 0.95 - 0.3 * fy];
            let hill_y = 0.72 + 0.05 * (fx * 9.0).sin();
            let mut c = if fy > hill_y { [0.35, 0.62, 0.38] } else { sky };
            for &(bx, by, r, col) in &blobs {
                let (dx, dy) = ((fx - bx) * w as f32 / h as f32, fy - by);
                let d = (dx * dx + dy * dy).sqrt();
                let edge = (r - d) * h as f32; // px inside the blob
                if edge > -1.5 {
                    // cel shadow on the lower right, a highlight top left
                    let shade = if dx + dy > r * 0.25 { 0.68 } else { 1.0 };
                    let hl = if (dx + r * 0.4).powi(2) + (dy + r * 0.45).powi(2) < (r * 0.18).powi(2) { 0.6 } else { 0.0 };
                    let body = [col[0] * shade + (1.0 - col[0] * shade) * hl, col[1] * shade + (1.0 - col[1] * shade) * hl, col[2] * shade + (1.0 - col[2] * shade) * hl];
                    let inside = (edge + 0.5).clamp(0.0, 1.0);
                    for k in 0..3 {
                        c[k] = c[k] * (1.0 - inside) + body[k] * inside;
                    }
                    // 3 px dark outline, antialiased
                    let ink = (1.0 - ((edge - 1.5).abs() - 1.5).clamp(0.0, 1.0)).clamp(0.0, 1.0);
                    for k in 0..3 {
                        c[k] = c[k] * (1.0 - ink) + 0.08 * ink;
                    }
                }
            }
            img.put_pixel(x, y, image::Rgba([(c[0] * 255.0) as u8, (c[1] * 255.0) as u8, (c[2] * 255.0) as u8, 255]));
        }
    }
    img
}

fn run_split(st: &mut Studio, args: &[f64]) -> String {
    let a: Vec<Val> = args.iter().map(|&v| n(v)).collect();
    assert_eq!(num(call(st, "art_split_start", &a)), 1.0);
    let t = std::time::Instant::now();
    loop {
        let p = num(call(st, "art_split_poll", &[]));
        if p >= 2.0 {
            let msg = match call(st, "art_split_msg", &[]) {
                Val::Str(m) => m,
                _ => String::new(),
            };
            assert_eq!(p, 2.0, "split failed: {msg}");
            println!("split {args:?}: {msg} in {:.2}s", t.elapsed().as_secs_f32());
            return msg;
        }
        assert!(t.elapsed().as_secs() < 120, "split timed out");
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
}

/// mean and worst per-channel difference (0..255) between two pictures
fn diff(a: &str, b: &str) -> (f64, u8) {
    let a = image::open(a).unwrap().to_rgba8();
    let b = image::open(b).unwrap().to_rgba8();
    let mut sum = 0u64;
    let mut worst = 0u8;
    for (p, q) in a.pixels().zip(b.pixels()) {
        for k in 0..3 {
            let d = p[k].abs_diff(q[k]);
            sum += d as u64;
            worst = worst.max(d);
        }
    }
    (sum as f64 / (a.width() * a.height() * 3) as f64, worst)
}

#[test]
fn splits_pictures_into_layers_and_parallax_parts() {
    let (w, h) = (720u32, 450u32);
    let src = out("split_src.png");
    picture(w, h).save(&src).unwrap();

    // mode by mode: components, objects, both (with per-colour flats)
    for (tag, args) in [
        ("components", vec![0.0, 0.0, 1.0, 0.0, 8.0, 0.5, 4.0, 6.0, 0.0, 0.0]),
        ("objects", vec![0.0, 1.0, 0.0, 0.0, 8.0, 0.5, 4.0, 6.0, 0.0, 1.0]),
        ("both", vec![0.0, 1.0, 1.0, 1.0, 8.0, 0.5, 4.0, 6.0, 0.0, 0.0]),
    ] {
        let mut st = Studio::new();
        call(&mut st, "art_doc_new", &[n(w as f64), n(h as f64), n(144.0), s("Split"), n(16777215.0), n(0.0)]);
        call(&mut st, "art_layer_del", &[n(0.0)]);
        let li = num(call(&mut st, "art_import_image", &[s(&src), n(0.0)]));
        let mut args = args.clone();
        args[0] = li;
        let before = num(call(&mut st, "art_layer_count", &[]));
        run_split(&mut st, &args);
        let after = num(call(&mut st, "art_layer_count", &[]));
        assert!(after > before + 1.0, "{tag}: no layers added");
        let rec = out(&format!("split_{tag}_recomposed.png"));
        call(&mut st, "art_export_png", &[s(&rec), n(1.0), n(0.0)]);
        let (mean, worst) = diff(&src, &rec);
        println!("{tag}: {} layers, recomposed vs original: mean {mean:.3}, worst {worst}", after - before);
        assert!(mean < 1.5, "{tag}: the layers do not stack back to the picture (mean diff {mean})");
        let dir = out(&format!("split_{tag}_layers"));
        let _ = std::fs::remove_dir_all(&dir);
        call(&mut st, "art_export_layers", &[s(&dir)]);

        if tag == "objects" {
            // the filled background must exist and the stage must render with a turned camera
            assert_eq!(num(call(&mut st, "art_split_hole_select", &[])), 1.0);
            call(&mut st, "art_sc_render_png", &[s(&out("split_stage_rest.png")), n(0.0), n(1.0)]);
            call(&mut st, "art_sc_set", &[s("cam.proj"), n(1.0)]);
            call(&mut st, "art_sc_set", &[s("cam.yaw"), n(14.0)]);
            call(&mut st, "art_sc_render_png", &[s(&out("split_stage_yaw.png")), n(0.0), n(1.0)]);
        }
    }
}
