//! Bridge between Ling values and the `ling-paint` engine (`art_*` builtins): LingAnime's layers, brushes,
//! vectors, filters, 2.5D scenes, Stable Diffusion and pen tablets. The calls that paint pixels draw straight
//! into the window's framebuffer (clipped by px_clip); `art_pen_pump` reads WinTab packets and maps them from
//! screen pixels into the same buffer coordinates `mouse_x` / `mouse_y` report.
use super::{EvalErr, EvalResult, Interpreter, Value};
#[cfg(feature = "art")]
use ling_paint::{Fb, PenSample, Studio, Val};
#[cfg(feature = "art")]
use std::rc::Rc;

#[cfg(feature = "art")]
fn to_val(v: &Value) -> Val {
    match v {
        Value::Number(n) => Val::Num(*n),
        Value::Bool(b) => Val::Num(if *b { 1.0 } else { 0.0 }),
        Value::Str(s) => Val::Str(s.clone()),
        Value::List(l) => Val::List(l.iter().map(to_val).collect()),
        Value::Unit => Val::Unit,
        other => Val::Str(other.to_string()),
    }
}

#[cfg(feature = "art")]
fn from_val(v: Val) -> Value {
    match v {
        Val::Num(n) => Value::Number(n),
        Val::Str(s) => Value::Str(s),
        Val::List(l) => Value::List(Rc::new(l.into_iter().map(from_val).collect())),
        Val::Unit => Value::Unit,
    }
}

#[cfg(feature = "art")]
const DRAW_CALLS: &[&str] = &["art_view_draw", "art_sc_draw", "art_layer_thumb", "art_part_thumb", "art_draw_sv", "art_draw_hue", "art_draw_grad", "art_draw_brush"];

/// screen pixel -> framebuffer pixel of `win` (the maths of buffer_mouse_pos, from a screen position)
#[cfg(all(feature = "art", windows))]
fn screen_to_buffer(win: &minifb::Window, buf_w: usize, buf_h: usize, sx: f32, sy: f32) -> Option<(f32, f32)> {
    #[repr(C)]
    struct Point {
        x: i32,
        y: i32,
    }
    #[link(name = "user32")]
    extern "system" {
        fn ClientToScreen(hwnd: isize, p: *mut Point) -> i32;
    }
    let hwnd = win.get_window_handle() as isize;
    if hwnd == 0 {
        return None;
    }
    let mut o = Point { x: 0, y: 0 };
    // the client origin on screen; sub-pixel precision is kept by offsetting the float position
    if unsafe { ClientToScreen(hwnd, &mut o) } == 0 {
        return None;
    }
    let (mx, my) = (sx - o.x as f32, sy - o.y as f32);
    let (cw, ch) = win.get_size();
    let (cw, ch) = (cw as f32, ch as f32);
    if cw < 1.0 || ch < 1.0 {
        return None;
    }
    let v = super::SCALED_BUF.load(std::sync::atomic::Ordering::Relaxed);
    let (x, y) = if v == 0 {
        (mx * buf_w as f32 / cw, my * buf_h as f32 / ch)
    } else {
        let (bw, bh) = ((v >> 32) as f32, (v & 0xFFFF_FFFF) as f32);
        let (ba, wa) = (bw / bh, cw / ch);
        let (dw, dh, ox, oy) = if ba > wa {
            let dh = cw / ba;
            (cw, dh, 0.0, (ch - dh) / 2.0)
        } else {
            let dw = ch * ba;
            (dw, ch, (cw - dw) / 2.0, 0.0)
        };
        ((mx - ox) * bw / dw, (my - oy) * bh / dh)
    };
    let k = super::ui_scale_now();
    Some((x / k, y / k))
}

impl Interpreter {
    /// `None` when `name` is not one of the LingAnime builtins.
    pub(super) fn paint_builtin(&mut self, name: &str, args: &[Value]) -> Option<EvalResult> {
        if !name.starts_with("art_") {
            return None;
        }
        #[cfg(not(feature = "art"))]
        {
            let _ = args;
            return Some(Err(EvalErr::Runtime(format!(
                "'{name}' needs the `art` profile, which this build of ling leaves out (reinstall with: cargo install ling-lang --features art, or the default full install)"
            ))));
        }
        #[cfg(feature = "art")]
        {
            let vals: Vec<Val> = args.iter().map(to_val).collect();
            let st = self.paint.get_or_insert_with(|| Box::new(Studio::new()));
            match name {
                "art_tablet_open" => {
                    let hwnd = {
                        let gfx = self.gfx.borrow();
                        gfx.window.as_ref().map(|w| w.get_window_handle() as isize).unwrap_or(0)
                    };
                    if hwnd == 0 {
                        return Some(Ok(Value::Number(0.0)));
                    }
                    return Some(Ok(from_val(st.tablet_open(hwnd))));
                }
                "art_pen_pump" => {
                    let pk = st.tablet_poll();
                    if pk.is_empty() {
                        return Some(Ok(Value::Number(0.0)));
                    }
                    #[cfg(windows)]
                    {
                        let gfx = self.gfx.borrow();
                        let (bw, bh) = (gfx.width, gfx.height);
                        if let Some(win) = gfx.window.as_ref() {
                            let mapped: Vec<PenSample> = pk
                                .iter()
                                .filter_map(|p| screen_to_buffer(win, bw, bh, p.sx, p.sy).map(|(x, y)| PenSample { x, y, p: p.p, tilt: p.tilt, az: p.az, twist: p.twist, eraser: p.eraser }))
                                .collect();
                            let n = mapped.len();
                            st.pen_feed(mapped);
                            return Some(Ok(Value::Number(n as f64)));
                        }
                    }
                    return Some(Ok(Value::Number(0.0)));
                }
                _ => {}
            }
            if DRAW_CALLS.contains(&name) {
                let mut gfx = self.gfx.borrow_mut();
                let (w, h) = (gfx.width, gfx.height);
                let clip = self.px_clip.unwrap_or([0, 0, w as i32, h as i32]);
                let len = gfx.buffer.len().min(w * h);
                let mut fb = Fb { buf: &mut gfx.buffer[..len], w, h: len / w.max(1), clip, tag: crate::gfx::UNLIT };
                return st.draw(name, &vals, &mut fb).map(|r| r.map(from_val).map_err(EvalErr::Runtime));
            }
            let r = st.call(name, &vals)?;
            Some(r.map(from_val).map_err(EvalErr::Runtime))
        }
    }
}
