//! Host-engine builtins that bigger Ling programs (FLY for FUN first) call and
//! the kernel didn't have: the animation scalar drivers (`tween`,
//! `tween_ease`, `wobble`, `gait_*`), a few string helpers, `crypto_hash`, the
//! display queries, and no-op stand-ins for desktop-window knobs that mean
//! nothing on a kernel framebuffer (vsync, fullscreen, cursor hiding, font AA).
//! Semantics follow `ling/src/runtime/mod.rs` + `crates/ling-animation`.
//!
//! [`canon`] maps the Korean (and other localized) names those programs use to
//! the canonical builtin each stands for; `lingrt::ling_builtin` applies it
//! before dispatching, so every builtin table answers both spellings.

use crate::lingrt::{list_from, mk_str, num, to_f64, to_string, FALSE, TRUE, UNIT};
use alloc::vec;

/// A localized builtin name -> its canonical name (names the kernel's tables
/// use). Only consulted for non-ASCII names, so ASCII calls pay nothing.
pub fn canon(name: &[u8]) -> &[u8] {
    if name.iter().all(|b| *b < 0x80) {
        return name;
    }
    let s = core::str::from_utf8(name).unwrap_or("");
    match s {
        "색설정" => b"set_color",
        "내림" => b"floor",
        "올림" => b"ceil",
        "반올림" => b"round",
        "사인" => b"sin",
        "코사인" => b"cos",
        "탄젠트" => b"tan",
        "제곱근" => b"sqrt",
        "절댓값" => b"abs",
        "거듭제곱" => b"pow",
        "최소" => b"min",
        "최대" => b"max",
        "키눌림" => b"key_pressed",
        "키누름" => b"key_down",
        "트윈" => b"tween",
        "이징트윈" => b"tween_ease",
        "블렌드모드" => b"set_blend",
        "걸음흔들" => b"gait_swing",
        "걸음위상" => b"gait_phase",
        "발들기" => b"gait_lift",
        "흔들림" => b"wobble",
        "카메라설정" => b"set_camera",
        "카메라좌표" => b"set_camera_pos",
        "Z거리설정" => b"set_zdist",
        "조명추가" => b"add_light",
        "조명초기화" => b"clear_lights",
        "환경광설정" => b"set_ambient",
        "안개설정" => b"set_fog",
        "톤스톱" => b"tone_stop",
        "톤램프클리어" => b"tone_ramp_clear",
        "출력" => b"print",
        "채우기" => b"fill",
        "현재시간" => b"time_now",
        "창열기" => b"open_window",
        "창열림" => b"window_is_open",
        "표시" => b"present",
        "인터페이스테마" => b"ui_theme",
        "체력바" => b"ui_healthbar",
        "카운터" => b"ui_counter",
        "레이더" => b"ui_radar",
        "나침반" => b"ui_compass",
        "슬롯격자" => b"ui_slotgrid",
        "투명도설정" => b"set_alpha",
        "문자열포함" => b"str_contains",
        "기하해시" => b"crypto_hash",
        "연결종료" => b"net_close",
        _ => name,
    }
}

fn ease(kind: &str, t: f32) -> f32 {
    use core::f32::consts::{FRAC_PI_2, PI};
    let t = t.clamp(0.0, 1.0);
    let k: alloc::string::String = kind.chars().filter(|c| c.is_ascii_alphanumeric()).map(|c| c.to_ascii_lowercase()).collect();
    let pow = |b: f32, e: f32| libm::powf(b, e);
    match k.as_str() {
        "step" => {
            if t < 1.0 {
                0.0
            } else {
                1.0
            }
        },
        "quadin" => t * t,
        "quadout" => 1.0 - (1.0 - t) * (1.0 - t),
        "quadinout" => {
            if t < 0.5 {
                2.0 * t * t
            } else {
                1.0 - pow(-2.0 * t + 2.0, 2.0) / 2.0
            }
        },
        "cubicin" => t * t * t,
        "cubicout" => 1.0 - pow(1.0 - t, 3.0),
        "cubicinout" | "smooth" => {
            if t < 0.5 {
                4.0 * t * t * t
            } else {
                1.0 - pow(-2.0 * t + 2.0, 3.0) / 2.0
            }
        },
        "sinein" => 1.0 - libm::cosf(t * FRAC_PI_2),
        "sineout" => libm::sinf(t * FRAC_PI_2),
        "sineinout" => -(libm::cosf(PI * t) - 1.0) / 2.0,
        "expoin" => {
            if t == 0.0 {
                0.0
            } else {
                pow(2.0, 10.0 * t - 10.0)
            }
        },
        "expoout" => {
            if t == 1.0 {
                1.0
            } else {
                1.0 - pow(2.0, -10.0 * t)
            }
        },
        "backout" | "overshoot" => {
            let (c1, c3) = (1.70158, 2.70158);
            1.0 + c3 * pow(t - 1.0, 3.0) + c1 * pow(t - 1.0, 2.0)
        },
        "bounceout" | "bounce" => {
            let (n1, d1) = (7.5625, 2.75);
            if t < 1.0 / d1 {
                n1 * t * t
            } else if t < 2.0 / d1 {
                let t = t - 1.5 / d1;
                n1 * t * t + 0.75
            } else if t < 2.5 / d1 {
                let t = t - 2.25 / d1;
                n1 * t * t + 0.9375
            } else {
                let t = t - 2.625 / d1;
                n1 * t * t + 0.984375
            }
        },
        _ => t,
    }
}

/// The background track: (path, volume, every path loaded so far -> sample id).
static mut BGM: (alloc::string::String, f32, alloc::vec::Vec<(alloc::string::String, usize)>) =
    (alloc::string::String::new(), 0.5, alloc::vec::Vec::new());
/// Loaded images by path (`image_load`), `0xAARRGGBB`.
static mut IMAGES: alloc::vec::Vec<(alloc::string::String, crate::drivers::image::Image)> = alloc::vec::Vec::new();
/// The ui_* theme: primary, accent, track, warn, text, bg (r, g, b each).
static mut THEME: [f64; 18] = [76.0, 110.0, 200.0, 214.0, 52.0, 64.0, 40.0, 44.0, 60.0, 240.0, 72.0, 80.0, 234.0, 244.0, 255.0, 16.0, 18.0, 28.0];

fn image(id: f64) -> Option<&'static crate::drivers::image::Image> {
    if id < 0.0 {
        return None;
    }
    let imgs = unsafe { &*(&raw const IMAGES) };
    imgs.get(id as usize).map(|(_, i)| i)
}

fn blend_rgb(d: u32, c: u32, a: u32) -> u32 {
    let m = |sh: u32| ((((d >> sh) & 0xff) * (255 - a) + ((c >> sh) & 0xff) * a) / 255) << sh;
    m(16) | m(8) | m(0)
}

/// Synthesized blips by (freq*10, ms, wave, amp*100) -> sample id.
static mut BLIPS: alloc::vec::Vec<((u64, u64, u64, u64), usize)> = alloc::vec::Vec::new();

fn gait_phase(t: f32, speed: f32) -> f32 {
    let x = t * speed;
    x - libm::floorf(x)
}

fn hex(bytes: &[u8]) -> alloc::string::String {
    const D: &[u8; 16] = b"0123456789abcdef";
    let mut s = alloc::string::String::with_capacity(bytes.len() * 2);
    for &b in bytes {
        s.push(D[(b >> 4) as usize] as char);
        s.push(D[(b & 15) as usize] as char);
    }
    s
}

/// The builtins this module implements; `None` for any other name.
pub fn dispatch(name: &[u8], args: &[u64]) -> Option<u64> {
    let n = |i: usize, d: f64| args.get(i).map(|&v| to_f64(v)).unwrap_or(d);
    let s = |i: usize| args.get(i).map(|&v| to_string(v)).unwrap_or_default();
    const TAU: f32 = core::f32::consts::TAU;
    Some(match name {
        b"tween" => {
            let (a, b) = (n(0, 0.0), n(1, 0.0));
            num(a + (b - a) * n(2, 0.0).clamp(0.0, 1.0))
        },
        b"tween_ease" => {
            let (a, b) = (n(0, 0.0) as f32, n(1, 0.0) as f32);
            let kind = if args.len() > 3 { s(3) } else { alloc::string::String::from("linear") };
            num((a + (b - a) * ease(&kind, n(2, 0.0) as f32)) as f64)
        },
        b"ease" => {
            let kind = if args.len() > 1 { s(1) } else { alloc::string::String::from("linear") };
            num(ease(&kind, n(0, 0.0) as f32) as f64)
        },
        b"wobble" => {
            let (t, f, a, p) = (n(0, 0.0) as f32, n(1, 1.0) as f32, n(2, 1.0) as f32, n(3, 0.0) as f32);
            num((a * libm::sinf(TAU * (f * t + p))) as f64)
        },
        b"gait_phase" => num(gait_phase(n(0, 0.0) as f32, n(1, 1.0) as f32) as f64),
        b"gait_swing" => {
            let ph = gait_phase(n(0, 0.0) as f32, n(1, 1.0) as f32);
            num((n(2, 1.0) as f32 * libm::sinf(TAU * ph)) as f64)
        },
        b"gait_lift" => {
            let ph = gait_phase(n(0, 0.0) as f32, n(1, 1.0) as f32);
            num((n(2, 1.0) as f32 * libm::sinf(TAU * ph).max(0.0)) as f64)
        },
        b"str" | b"to_str" => mk_str(&to_string(args.first().copied().unwrap_or(UNIT))),
        // the host's `split(s, sep)` (= str_split): a list of the pieces; sep "" = newline
        b"split" => {
            let sep = s(1);
            let sep = if args.len() < 2 || sep.is_empty() { alloc::string::String::from("\n") } else { sep };
            list_from(s(0).split(sep.as_str()).map(mk_str).collect())
        },
        b"str_contains" => {
            if s(0).contains(s(1).as_str()) {
                TRUE
            } else {
                FALSE
            }
        },
        b"hash_str" => {
            let mut h: u64 = 14695981039346656037;
            for b in s(0).bytes() {
                h ^= b as u64;
                h = h.wrapping_mul(1099511628211);
            }
            num((h & 0xFFFFFF) as f64 / 16777215.0)
        },
        b"crypto_hash" => mk_str(&hex(&ling_crypto::geo::holo_hash(s(0).as_bytes()))),
        b"display_info" => {
            let (w, h) = crate::drivers::framebuffer::mode_dims();
            list_from(vec![num(w as f64), num(h as f64), num(60.0), num(96.0), num(32.0)])
        },
        // background music from a WAV (or Ogg/FLAC) path, looped; the same path
        // again keeps it playing, a new one replaces it (host `audio_bgm(path, vol)`)
        b"audio_bgm" => {
            let path = s(0);
            let vol = n(1, 0.5) as f32;
            let cur = unsafe { &mut *(&raw mut BGM) };
            if cur.0 != path || !crate::drivers::sfx::music_playing() {
                crate::drivers::sfx::stop_music();
                let id = match cur.2.iter().find(|(p, _)| *p == path) {
                    Some(&(_, id)) => Some(id),
                    None => crate::vfs::read(&path).and_then(|d| crate::drivers::sfx::load_wav(&d)).inspect(|&id| cur.2.push((path.clone(), id))),
                };
                if let Some(id) = id {
                    let _ = crate::drivers::sfx::play(id, 1.0, 0.0, true, 1.0, true);
                }
                cur.0 = path;
            }
            cur.1 = vol;
            crate::drivers::sfx::set_music_volume(vol);
            UNIT
        },
        b"audio_bgm_volume" => {
            crate::drivers::sfx::set_music_volume(n(0, 0.5) as f32);
            UNIT
        },
        b"audio_bgm_stop" => {
            crate::drivers::sfx::stop_music();
            unsafe { (*(&raw mut BGM)).0.clear() };
            UNIT
        },
        // a synthesized UI blip (freq, dur, wave name, amp); one sample per distinct sound
        b"audio_blip" => {
            let (freq, dur, amp) = (n(0, 660.0), n(1, 0.08), n(3, 0.25));
            let wave = match s(2).as_str() {
                "square" => 1,
                "saw" | "sawtooth" => 2,
                "triangle" | "tri" => 3,
                "noise" => 4,
                _ => 0,
            };
            let key = ((freq * 10.0) as u64, (dur * 1000.0) as u64, wave as u64, (amp * 100.0) as u64);
            let cache = unsafe { &mut *(&raw mut BLIPS) };
            let id = match cache.iter().find(|(k, _)| *k == key) {
                Some(&(_, id)) => id,
                None => {
                    let id = crate::drivers::sfx::synth(freq, amp, dur, wave);
                    if cache.len() < 256 {
                        cache.push((key, id));
                    }
                    id
                },
            };
            let _ = crate::drivers::sfx::play(id, 1.0, 0.0, false, 1.0, false);
            UNIT
        },
        // toon-shading ramp knobs of the host's 3-D look; the kernel renderer
        // has its own fixed shading
        b"tone_soft" | b"tone_stop" | b"tone_ramp_clear" => UNIT,
        // desktop-window knobs with no meaning on the kernel's framebuffer
        b"set_vsync" | b"cursor_hide" | b"cursor_show" | b"set_font_antialias" | b"set_shadow_color" => UNIT,
        // ── images: PNG / BMP decoded once per path, drawn scaled into the window ──
        b"image_load" => {
            let path = s(0).replace('\\', "/");
            let imgs = unsafe { &mut *(&raw mut IMAGES) };
            if let Some(i) = imgs.iter().position(|(p, _)| *p == path) {
                return Some(num(i as f64));
            }
            let img = crate::vfs::read(&path).and_then(|d| {
                if path.to_ascii_lowercase().ends_with(".bmp") {
                    crate::drivers::image::decode_bmp_rgba(&d)
                } else {
                    crate::drivers::image::decode_png_rgba(&d)
                }
            });
            match img {
                Some(im) => {
                    imgs.push((path, im));
                    num((imgs.len() - 1) as f64)
                },
                None => num(-1.0),
            }
        },
        b"image_w" | b"image_width" => num(image(n(0, -1.0)).map(|i| i.w).unwrap_or(0) as f64),
        b"image_h" | b"image_height" => num(image(n(0, -1.0)).map(|i| i.h).unwrap_or(0) as f64),
        b"image_pixel_r" | b"image_pixel_g" | b"image_pixel_b" | b"image_pixel_a" => {
            let (x, y) = (n(1, 0.0), n(2, 0.0));
            let v = image(n(0, -1.0)).and_then(|i| {
                if x < 0.0 || y < 0.0 || x as u32 >= i.w || y as u32 >= i.h {
                    return None;
                }
                let p = i.px[y as usize * i.w as usize + x as usize];
                Some(match name {
                    b"image_pixel_r" => (p >> 16) & 0xff,
                    b"image_pixel_g" => (p >> 8) & 0xff,
                    b"image_pixel_b" => p & 0xff,
                    _ => p >> 24,
                })
            });
            num(v.unwrap_or(0) as f64)
        },
        // image_pixel_u16(id, x, y): r*256 + g (a 16-bit height map) in one call; 0 off the image
        b"image_pixel_u16" => {
            let (x, y) = (n(1, 0.0), n(2, 0.0));
            let v = image(n(0, -1.0)).and_then(|i| {
                if x < 0.0 || y < 0.0 || x as u32 >= i.w || y as u32 >= i.h {
                    return None;
                }
                let p = i.px[y as usize * i.w as usize + x as usize];
                Some(((p >> 16) & 0xff) * 256 + ((p >> 8) & 0xff))
            });
            num(v.unwrap_or(0) as f64)
        },
        // image_height_tri(id, fx, fz, step): the host's node-grid terrain sample -- the cell of
        // `step` pixels holding (fx, fz) split along its (qx+step, qz)-(qx, qz+step) diagonal, the
        // height interpolated on the triangle holding the point (0..65535); -1 where the cell's
        // corner has no data (blue < 128) or the id is bad
        b"image_height_tri" => {
            let (fx, fz, ms) = (n(1, 0.0), n(2, 0.0), n(3, 1.0));
            let Some(i) = image(n(0, -1.0)) else { return Some(num(-1.0)) };
            if !(ms > 0.0) || !fx.is_finite() || !fz.is_finite() {
                return Some(num(-1.0));
            }
            let (w, h) = (i.w as i64, i.h as i64);
            let px = |x: f64, y: f64| -> u32 {
                let (xi, yi) = (x as i64, y as i64);
                if xi >= 0 && yi >= 0 && xi < w && yi < h { i.px[yi as usize * w as usize + xi as usize] } else { 0 }
            };
            let u16v = |x: f64, y: f64| {
                let p = px(x, y);
                (((p >> 16) & 0xff) * 256 + ((p >> 8) & 0xff)) as f64
            };
            let qx = libm::floor(fx / ms) * ms;
            let qz = libm::floor(fz / ms) * ms;
            if px(qx, qz) & 0xff < 128 {
                return Some(num(-1.0));
            }
            let (tx, tz) = ((fx - qx) / ms, (fz - qz) / ms);
            let (h10, h01) = (u16v(qx + ms, qz), u16v(qx, qz + ms));
            let v = if tx + tz <= 1.0 {
                let h00 = u16v(qx, qz);
                h00 + (h10 - h00) * tx + (h01 - h00) * tz
            } else {
                let h11 = u16v(qx + ms, qz + ms);
                h11 + (h01 - h11) * (1.0 - tx) + (h10 - h11) * (1.0 - tz)
            };
            num(v)
        },
        b"tanh" | b"tanhf" => num(libm::tanh(n(0, 0.0))),
        // image_draw(id, x, y, w, h [, cx, cy, cw, ch [, opacity]]): stretched, alpha-blended, clipped
        b"image_draw" => {
            let Some(img) = image(n(0, -1.0)) else { return Some(UNIT) };
            let (dx, dy, dw, dh) = (n(1, 0.0) as i64, n(2, 0.0) as i64, n(3, 0.0).max(0.0) as i64, n(4, 0.0).max(0.0) as i64);
            let (kx0, ky0, kx1, ky1) = if args.len() >= 9 {
                let (a, b) = (n(5, 0.0) as i64, n(6, 0.0) as i64);
                (a, b, a + n(7, 0.0) as i64, b + n(8, 0.0) as i64)
            } else {
                (i64::MIN, i64::MIN, i64::MAX, i64::MAX)
            };
            let op = if args.len() >= 10 { (n(9, 1.0).clamp(0.0, 1.0) * 255.0) as u32 } else { 255 };
            let Some((buf, fw, fh)) = crate::lingapp::surface() else { return Some(UNIT) };
            if dw == 0 || dh == 0 || op == 0 || img.w == 0 || img.h == 0 {
                return Some(UNIT);
            }
            let (x0, x1) = (dx.max(0).max(kx0), (dx + dw).min(fw as i64).min(kx1));
            let (y0, y1) = (dy.max(0).max(ky0), (dy + dh).min(fh as i64).min(ky1));
            for ty in y0..y1 {
                let sy = (((ty - dy) * img.h as i64 / dh) as u32).min(img.h - 1);
                for tx in x0..x1 {
                    let sx = (((tx - dx) * img.w as i64 / dw) as u32).min(img.w - 1);
                    let p = img.px[(sy * img.w + sx) as usize];
                    let a = (p >> 24) * op / 255;
                    if a == 0 {
                        continue;
                    }
                    let i = ty as usize * fw + tx as usize;
                    buf[i] = if a >= 255 { p & 0xFFFFFF } else { blend_rgb(buf[i], p & 0xFFFFFF, a) };
                }
            }
            UNIT
        },
        b"image_free" => UNIT,
        // ── the ui_* theme the game's own widgets read back ──
        b"ui_theme" => {
            let th = unsafe { &mut *(&raw mut THEME) };
            for (i, slot) in th.iter_mut().enumerate() {
                if let Some(&v) = args.get(i) {
                    *slot = to_f64(v).clamp(0.0, 255.0);
                }
            }
            UNIT
        },
        b"ui_theme_colors" => list_from(unsafe { &*(&raw const THEME) }.iter().map(|&v| num(v)).collect()),
        // the UI scale layer: the HUD is laid out for 720 px, the render height here
        b"ui_begin" | b"ui_end" => UNIT,
        // cosmetic effects not drawn on LingOS yet: the sky-ink fluid, the ground sigil
        b"liquid_new" => num(0.0),
        b"liquid_set_colors" | b"liquid_gravity" | b"liquid_splat" | b"liquid_step" | b"liquid_draw_surface" | b"liquid_draw" | b"vtex_spiral" | b"knot_keygen" | b"knot_public" => UNIT,
        _ => return None,
    })
}
