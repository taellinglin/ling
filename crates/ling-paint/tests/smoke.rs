//! End-to-end checks of the engine through its named-call API; pictures land in $LINGPAINT_OUT (or the temp dir).
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
fn out(name: &str) -> String {
    let d = std::env::var("LINGPAINT_OUT").unwrap_or_else(|_| std::env::temp_dir().to_string_lossy().to_string());
    std::path::Path::new(&d).join(name).to_string_lossy().to_string()
}
fn stroke(st: &mut Studio, pts: &[(f32, f32)]) {
    // screen == canvas when the view is at zoom 1 centred on the canvas
    call(st, "art_stroke_begin", &[n(pts[0].0 as f64), n(pts[0].1 as f64), n(0.0)]);
    for &(x, y) in &pts[1..] {
        call(st, "art_stroke_to", &[n(x as f64), n(y as f64)]);
    }
    call(st, "art_stroke_end", &[]);
}

#[test]
fn paints_filters_scenes_and_saves() {
    let mut st = Studio::new();
    call(&mut st, "art_doc_new", &[n(1200.0), n(800.0), n(300.0), s("Smoke"), n(16777215.0), n(1.0)]);
    call(&mut st, "art_view_set", &[n(0.0), n(0.0), n(1200.0), n(800.0)]);
    call(&mut st, "art_view_set_zoom", &[n(1.0)]);
    call(&mut st, "art_view_center", &[]);
    call(&mut st, "art_sim_set", &[s("mode"), n(2.0)]);
    let presets = match call(&mut st, "art_brush_presets", &[]) {
        Val::List(l) => l.len(),
        _ => 0,
    };
    assert!(presets >= 18);
    // one wavy stroke per preset, in a different colour each
    for i in 0..presets {
        call(&mut st, "art_brush_load", &[n(i as f64)]);
        let hue = i as f64 / presets as f64;
        let c = call(&mut st, "art_hsv_to", &[n(hue), n(0.8), n(0.75)]);
        call(&mut st, "art_color", &[c]);
        let row = (i % 10) as f32;
        let col = (i / 10) as f32;
        let y0 = 40.0 + row * 38.0;
        let x0 = 40.0 + col * 560.0;
        let pts: Vec<(f32, f32)> = (0..80).map(|k| {
            let t = k as f32 / 79.0;
            (x0 + t * 480.0, y0 + (t * 12.0).sin() * 10.0)
        }).collect();
        stroke(&mut st, &pts);
    }
    // gradient + halftone + dither on their own layers
    call(&mut st, "art_layer_add", &[n(0.0), s("Grad")]);
    call(&mut st, "art_sel_rect", &[n(40.0), n(440.0), n(380.0), n(760.0), n(0.0)]);
    call(&mut st, "art_grad_preset", &[s("Anime Sky (dusk)")]);
    call(&mut st, "art_grad_apply", &[n(40.0), n(440.0), n(40.0), n(760.0), n(0.0), n(0.0), n(1.0), n(1.0)]);
    call(&mut st, "art_layer_add", &[n(0.0), s("Halftone")]);
    call(&mut st, "art_sel_ellipse", &[n(420.0), n(440.0), n(760.0), n(760.0), n(0.0)]);
    call(&mut st, "art_color", &[n(0.0)]);
    call(&mut st, "art_grad_preset", &[s("FG to BG")]);
    call(&mut st, "art_grad_apply", &[n(590.0), n(600.0), n(760.0), n(760.0), n(1.0), n(0.0), n(1.0), n(0.0)]);
    call(&mut st, "art_filter_apply", &[s("halftone"), Val::List(vec![n(10.0), n(45.0), n(0.0), n(0.0), n(1.0)]), Val::List(vec![])]);
    call(&mut st, "art_layer_add", &[n(0.0), s("Dither")]);
    call(&mut st, "art_sel_rect", &[n(800.0), n(440.0), n(1160.0), n(760.0), n(0.0)]);
    call(&mut st, "art_grad_preset", &[s("Rainbow")]);
    call(&mut st, "art_grad_apply", &[n(800.0), n(440.0), n(1160.0), n(760.0), n(0.0), n(0.0), n(1.0), n(0.0)]);
    call(&mut st, "art_filter_apply", &[s("dither"), Val::List(vec![n(2.0), n(2.0), n(0.0), n(1.0), n(0.0), n(3.0)]), Val::List(vec![])]);
    call(&mut st, "art_sel_none", &[]);
    // a vector shape with a gradient fill and a freehand vector stroke
    call(&mut st, "art_layer_add", &[n(1.0), s("Vectors")]);
    call(&mut st, "art_brush_load", &[n(0.0)]);
    call(&mut st, "art_brush_set", &[s("size"), n(6.0)]);
    let pi = call(&mut st, "art_vec_shape", &[n(4.0), n(600.0), n(470.0), n(740.0), n(610.0), n(5.0), n(1.0), n(1.0)]);
    let pi = if let Val::Num(v) = pi { v } else { -1.0 };
    assert!(pi >= 0.0);
    call(&mut st, "art_grad_preset", &[s("Sakura")]);
    call(&mut st, "art_vec_grad_fill", &[n(pi), n(600.0), n(470.0), n(740.0), n(610.0), n(1.0), n(0.0)]);
    stroke(&mut st, &(0..60).map(|k| (900.0 + k as f32 * 4.0, 600.0 + (k as f32 * 0.2).cos() * 40.0)).collect::<Vec<_>>());
    // tone layer: grey paint shown as screentone
    call(&mut st, "art_layer_add", &[n(0.0), s("Tone")]);
    call(&mut st, "art_color", &[n(9474192.0)]);
    call(&mut st, "art_sel_rect", &[n(420.0), n(40.0), n(560.0), n(420.0), n(0.0)]);
    call(&mut st, "art_layer_fill", &[n(-1.0 + 0.0 + 6.0)]);
    call(&mut st, "art_sel_none", &[]);
    let li = match call(&mut st, "art_layer_active", &[]) {
        Val::Num(v) => v,
        _ => 0.0,
    };
    call(&mut st, "art_layer_set", &[n(li), s("tone"), n(1.0)]);
    call(&mut st, "art_layer_set", &[n(li), s("tone_lpi"), n(40.0)]);
    let png = out("lingpaint_smoke.png");
    assert!(matches!(call(&mut st, "art_export_png", &[s(&png), n(1.0), n(1.0)]), Val::Num(v) if v > 0.5), "{}", st.last_error());

    // save / open round trip
    let proj = out("lingpaint_smoke.lanime");
    assert!(matches!(call(&mut st, "art_doc_save", &[s(&proj)]), Val::Num(v) if v > 0.5), "{}", st.last_error());
    let mut st2 = Studio::new();
    assert!(matches!(call(&mut st2, "art_doc_open", &[s(&proj)]), Val::Num(v) if v > 0.5), "{}", st2.last_error());
    let info = call(&mut st2, "art_doc_info", &[]);
    if let Val::List(l) = info {
        assert!(matches!(l[3], Val::Num(v) if v >= 6.0));
    }
    call(&mut st2, "art_export_png", &[s(&out("lingpaint_smoke_reopened.png")), n(0.5), n(1.0)]);

    // 2.5D: layers to parts, a bone, each projection
    let made = call(&mut st, "art_sc_layers_to_parts", &[n(150.0), n(1.0)]);
    assert!(matches!(made, Val::Num(v) if v >= 3.0));
    let b = call(&mut st, "art_bone_add", &[n(-1.0), n(600.0), n(400.0), n(600.0), n(250.0)]);
    let _ = b;
    call(&mut st, "art_sc_bind", &[n(0.0), Val::List(vec![n(0.0)]), n(0.0)]);
    call(&mut st, "art_key_set", &[s("b0.rot"), n(0.0), n(0.0), n(2.0)]);
    call(&mut st, "art_key_set", &[s("b0.rot"), n(1.0), n(25.0), n(2.0)]);
    for (pi, nm) in [(0.0, "ortho"), (1.0, "persp"), (2.0, "fisheye"), (3.0, "hyper")] {
        call(&mut st, "art_sc_set", &[s("cam.proj"), n(pi)]);
        call(&mut st, "art_sc_set", &[s("cam.yaw"), n(18.0)]);
        call(&mut st, "art_sc_set", &[s("cam.pitch"), n(-8.0)]);
        call(&mut st, "art_sc_set", &[s("cam.curv"), n(0.7)]);
        let p = out(&format!("lingpaint_scene_{nm}.png"));
        assert!(matches!(call(&mut st, "art_sc_render_png", &[s(&p), n(0.5), n(0.5)]), Val::Num(v) if v > 0.5), "{}", st.last_error());
    }
    // unproject inverts project
    call(&mut st, "art_sc_set", &[s("cam.proj"), n(2.0)]);
    let pr = call(&mut st, "art_sc_project", &[n(500.0), n(300.0), n(0.0)]);
    if let Val::List(l) = pr {
        let (x, y) = (if let Val::Num(v) = l[0] { v } else { 0.0 }, if let Val::Num(v) = l[1] { v } else { 0.0 });
        let back = call(&mut st, "art_sc_unproject", &[n(x), n(y), n(0.0)]);
        if let Val::List(b) = back {
            let bx = if let Val::Num(v) = b[0] { v } else { 0.0 };
            let by = if let Val::Num(v) = b[1] { v } else { 0.0 };
            assert!((bx - 500.0).abs() < 1.0 && (by - 300.0).abs() < 1.0, "unproject {bx},{by}");
        }
    }
    // undo walks back
    let u = call(&mut st, "art_undo", &[]);
    assert!(matches!(u, Val::Num(v) if v > 0.5));
}
