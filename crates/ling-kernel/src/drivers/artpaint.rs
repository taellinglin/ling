//! LingAnime's `art_*` builtins on LingOS: the desktop's paint / 2.5D engine (`ling-paint`, built no_std) bound
//! to compiled Ling, the way `ling/src/runtime/paint.rs` binds it to the interpreter.
//!
//! One `Studio` lives while LingAnime runs (created on its first `art_*` call, dropped when the app ends).
//! Arguments are copied into the engine's own values (`Val`) and results copied back, so the engine never holds
//! a Ling value across frames (nothing for the GC to root). The calls that paint pixels (`art_view_draw`,
//! `art_sc_draw`, thumbnails, colour pickers, the brush preview) draw straight into the app window's back buffer,
//! clipped by `px_clip`. Files go through `crate::vfs` (the app's relative paths land in its data / Projects
//! folders), the clock is the kernel timer and the view's row loop and the 2.5D renderer's bands run on the
//! worker cores (`crate::par`). Not on LingOS: pen tablets (WinTab), Stable Diffusion and ffmpeg video export
//! (the engine reports those in `art_error` / the export message).

use alloc::boxed::Box;
use alloc::string::String;
use alloc::vec::Vec;

use ling_paint::os::Host;
use ling_paint::{Fb, Studio, Val};

use crate::lingrt::{is_list, is_num, is_str, list_from, list_items, mk_str, num, str_of, to_string, FALSE, TRUE, UNIT};

static mut STUDIO: Option<Box<Studio>> = None;
/// the app that made the studio (it goes when that app ends)
static mut OWNER: Option<String> = None;

const DRAW_CALLS: &[&str] = &["art_view_draw", "art_sc_draw", "art_layer_thumb", "art_part_thumb", "art_draw_sv", "art_draw_hue", "art_draw_grad", "art_draw_brush"];

fn h_now() -> f64 {
    crate::arch::timer::now_us() as f64 / 1_000_000.0
}
fn h_path(p: &str) -> String {
    p.replace('\\', "/")
}
fn h_read(p: &str) -> Option<Vec<u8>> {
    crate::vfs::read(&h_path(p))
}
fn h_write(p: &str, data: &[u8]) -> bool {
    crate::vfs::write(&h_path(p), data)
}
fn h_mkdir(p: &str) -> bool {
    crate::vfs::mkdir(&h_path(p))
}
fn h_par(n: usize, f: &(dyn Fn(usize) + Sync)) {
    crate::par::for_each(n, &|i| f(i));
}
/// BMP / SVG pictures through the kernel's decoders, as straight RGBA8
fn h_decode(data: &[u8]) -> Option<(usize, usize, Vec<u8>)> {
    use crate::drivers::image;
    let img = if image::looks_svg(data) { image::decode_svg_alpha(data, 2048) } else { image::decode_bmp_rgba(data).or_else(|| image::decode_png_rgba(data)) }?;
    let mut px = Vec::with_capacity(img.px.len() * 4);
    for &p in &img.px {
        px.extend_from_slice(&[(p >> 16) as u8, (p >> 8) as u8, p as u8, (p >> 24) as u8]);
    }
    Some((img.w as usize, img.h as usize, px))
}

static HOST: Host = Host { now: h_now, read: h_read, write: h_write, mkdir: h_mkdir, par_for: h_par, decode: h_decode };

fn to_val(v: u64) -> Val {
    if is_num(v) {
        Val::Num(f64::from_bits(v))
    } else if v == TRUE {
        Val::Num(1.0)
    } else if v == FALSE {
        Val::Num(0.0)
    } else if v == UNIT {
        Val::Unit
    } else if is_str(v) {
        Val::Str(String::from(str_of(v)))
    } else if is_list(v) {
        Val::List(list_items(v).iter().map(|&x| to_val(x)).collect())
    } else {
        Val::Str(to_string(v))
    }
}

fn from_val(v: Val) -> u64 {
    match v {
        Val::Num(n) => num(n),
        Val::Str(s) => mk_str(&s),
        Val::List(l) => list_from(l.into_iter().map(from_val).collect()),
        Val::Unit => UNIT,
    }
}

fn studio() -> &'static mut Studio {
    unsafe {
        let s = &mut *(&raw mut STUDIO);
        if s.is_none() {
            ling_paint::os::set_host(&HOST);
            // the system picture clipboard: Copy here pastes in Files / LingDiffusion and theirs here
            ling_paint::clipboard::set_hooks(&crate::drivers::clipboard::PAINT_HOOKS);
            *s = Some(Box::new(Studio::new()));
            *(&raw mut OWNER) = Some(crate::lingapp::active_app_name());
            crate::console_write(b"[art] LingAnime paint engine ready\n");
        }
        s.as_mut().unwrap()
    }
}

/// An app ended: free the studio if it was that app's (a 1080p document holds tens of MiB).
pub fn app_ended(name: &str) {
    unsafe {
        if (*(&raw const OWNER)).as_deref() == Some(name) {
            *(&raw mut STUDIO) = None;
            *(&raw mut OWNER) = None;
        }
    }
}

/// The `art_*` builtins; `None` for any other name.
pub fn dispatch(name: &[u8], args: &[u64]) -> Option<u64> {
    if !name.starts_with(b"art_") {
        return None;
    }
    let name = core::str::from_utf8(name).ok()?;
    match name {
        // WinTab pens are a Windows driver API: the mouse with simulated pressure paints instead
        "art_tablet_open" | "art_pen_pump" => return Some(num(0.0)),
        _ => {},
    }
    let vals: Vec<Val> = args.iter().map(|&v| to_val(v)).collect();
    let st = studio();
    let r = if DRAW_CALLS.contains(&name) {
        let Some((buf, w, h)) = crate::lingapp::surface() else { return Some(UNIT) };
        let clip = crate::lingapp::surface_clip();
        let len = buf.len().min(w * h);
        let mut fb = Fb { buf: &mut buf[..len], w, h: len / w.max(1), clip, tag: 0 };
        st.draw(name, &vals, &mut fb)
    } else {
        st.call(name, &vals)
    };
    match r {
        Some(Ok(v)) => Some(from_val(v)),
        Some(Err(e)) => {
            crate::console_write(b"[art] ");
            crate::console_write(name.as_bytes());
            crate::console_write(b": ");
            crate::console_write(e.as_bytes());
            crate::console_write(b"\n");
            Some(UNIT)
        },
        None => None,
    }
}
