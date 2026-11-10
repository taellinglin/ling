//! Paste from the real system clipboard. Ignored by default because it overwrites what you have copied:
//! `cargo test -p ling-paint --test clipboard -- --ignored`
use ling_paint::{Studio, Val};

fn n(v: f64) -> Val {
    Val::Num(v)
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

#[test]
#[ignore]
fn pastes_pictures_from_other_apps_and_copies_out() {
    let mut st = Studio::new();
    call(&mut st, "art_doc_new", &[n(400.0), n(300.0), n(144.0), Val::Str("Clip".into()), n(16777215.0), n(1.0)]);

    // "another app" copies a 120 x 80 half-transparent red picture
    let (w, h) = (120usize, 80usize);
    let px: Vec<u8> = (0..w * h).flat_map(|_| [255u8, 0, 0, 128]).collect();
    assert!(ling_paint::clipboard::set(w, h, px).is_some(), "could not write the clipboard");

    match call(&mut st, "art_clip_image_size", &[]) {
        Val::List(l) => assert_eq!(l.len(), 2),
        other => panic!("no clipboard size: {other:?}"),
    }
    let layers = num(call(&mut st, "art_layer_count", &[]));
    let li = num(call(&mut st, "art_paste", &[n(1.0)]));
    assert!(li >= 0.0, "paste failed");
    assert_eq!(num(call(&mut st, "art_layer_count", &[])), layers + 1.0);

    // our own copy goes out to the system clipboard as the selection's pixels
    call(&mut st, "art_copy", &[]);
    let img = ling_paint::clipboard::get().expect("our copy is not on the clipboard");
    assert_eq!((img.w, img.h), (w, h));
    assert!(img.px[0] > 240 && (img.px[3] as i32 - 128).abs() <= 2, "pixel {:?}", &img.px[..4]);

    // and pasting it back is recognised as ours (one more layer, no error)
    assert!(num(call(&mut st, "art_paste", &[n(1.0)])) >= 0.0);

    // stage part from the clipboard
    assert!(num(call(&mut st, "art_sc_add_clip", &[])) >= 0.0);
}
