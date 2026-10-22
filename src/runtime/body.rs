//! `body_*` builtins: the Ling-facing surface of `ling-body`. Values crossing the
//! boundary are numbers, strings and flat lists. Operations that can fail return
//! 1 or 0 and leave their message for `body_message()`.
use super::{EvalResult, Interpreter, Value};
use ling_body::BodyKit;
use std::rc::Rc;

#[derive(Default)]
pub(super) struct BodyState {
    kit: BodyKit,
    message: String,
}

fn num(args: &[Value], i: usize, default: f64) -> f64 {
    match args.get(i) {
        Some(Value::Number(n)) => *n,
        Some(Value::Bool(b)) => *b as i32 as f64,
        Some(Value::Str(s)) => s.trim().parse().unwrap_or(default),
        _ => default,
    }
}

fn index(args: &[Value], i: usize) -> Option<usize> {
    let n = num(args, i, -1.0);
    (n >= 0.0).then_some(n as usize)
}

fn text(args: &[Value], i: usize) -> String {
    args.get(i).map(|v| v.to_string()).unwrap_or_default()
}

fn floats(v: &[f32]) -> Value {
    Value::List(Rc::new(v.iter().map(|&x| Value::Number(x as f64)).collect()))
}

fn numbers(v: &[f64]) -> Value {
    Value::List(Rc::new(v.iter().map(|&x| Value::Number(x)).collect()))
}

fn flag(b: bool) -> Value {
    Value::Number(b as i32 as f64)
}

impl BodyState {
    fn done(&mut self, r: Result<String, String>) -> Value {
        match r {
            Ok(m) => {
                self.message = m;
                flag(true)
            },
            Err(e) => {
                self.message = e;
                flag(false)
            },
        }
    }
}

impl Interpreter {
    /// `body_draw(mode, yaw, ox, oy, oz, scale)`: draws the kit's current mesh with the
    /// pen colour. The mesh is Y-up in metres; the renderer is Y-down, so it is flipped
    /// here. Mode 0 shades solid triangles with smoothed normals, mode 1 draws the edges of
    /// every `stride`-th triangle (argument 6, default 1), and mode 2 is mode 0 tinted by
    /// the skin displacement, with argument 6 as the full-scale push in millimetres (default 10),
    /// mode 3 colours each triangle by its surface normal and mode 4 is hard single-light clay
    /// shading; both make fine relief easy to judge.
    fn body_draw(&mut self, args: &[Value]) -> EvalResult {
        let Some(mesh) = self.body.kit.mesh() else { return Ok(Value::Unit) };
        let mode = num(args, 0, 0.0) as i64;
        let yaw = num(args, 1, 0.0) as f32;
        let (ox, oy, oz) = (num(args, 2, 0.0) as f32, num(args, 3, 0.0) as f32, num(args, 4, 0.0) as f32);
        let s = num(args, 5, 1.0) as f32;
        let (c, sn) = (yaw.cos(), yaw.sin());
        let place = |p: &[f32]| [ox + s * (p[0] * c + p[2] * sn), oy - s * p[1], oz + s * (-p[0] * sn + p[2] * c)];
        let world: Vec<[f32; 3]> = mesh.pos.chunks_exact(3).map(place).collect();
        let turn = |n: &[f32]| [n[0] * c + n[2] * sn, -n[1], -n[0] * sn + n[2] * c];
        let normals: Vec<[f32; 3]> = mesh.nrm.chunks_exact(3).map(turn).collect();
        let smooth = normals.len() == world.len();

        let mut gfx = self.gfx.borrow_mut();
        if mode == 1 {
            let near = -gfx.camera.zdist + 0.05;
            let color = gfx.color;
            let stride = num(args, 6, 1.0).max(1.0) as usize;
            for t in mesh.idx.chunks_exact(3).step_by(stride) {
                for k in 0..3 {
                    let (a, b) = (world[t[k] as usize], world[t[(k + 1) % 3] as usize]);
                    if gfx.camera.depth(a[0], a[1], a[2]) <= near || gfx.camera.depth(b[0], b[1], b[2]) <= near {
                        continue;
                    }
                    if !gfx.edge_set.try_insert(a[0], a[1], a[2], b[0], b[1], b[2]) {
                        continue;
                    }
                    let (sax, say, da) = gfx.camera.project(a[0], a[1], a[2]);
                    let (sbx, sby, db) = gfx.camera.project(b[0], b[1], b[2]);
                    let depth = (da + db) * 0.5;
                    let col = gfx.fog_apply(color, depth);
                    gfx.depth_queue.push_line(depth, col, sax, say, sbx, sby);
                }
            }
            return Ok(Value::Unit);
        }
        let push = self.body.kit.mesh_push();
        let heat = mode == 2 && push.len() == world.len();
        let full = num(args, 6, 10.0).max(0.01) as f32;
        let base = gfx.color;
        let relief_view = (mode == 3 || mode == 4) && smooth;
        let lit_before = gfx.flat_shade;
        gfx.flat_shade = gfx.flat_shade || relief_view;
        let towards_light = {
            let l = [0.4f32, -0.7, -0.6];
            let n = (l[0] * l[0] + l[1] * l[1] + l[2] * l[2]).sqrt();
            [l[0] / n, l[1] / n, l[2] / n]
        };
        let mix = |t: f32, to: [f32; 3]| -> u32 {
            let ch = |s: u32, d: f32| ((s as f32) + (d - s as f32) * t) as u32;
            (ch((base >> 16) & 255, to[0]) << 16) | (ch((base >> 8) & 255, to[1]) << 8) | ch(base & 255, to[2])
        };
        for t in mesh.idx.chunks_exact(3) {
            let (a, b, cc) = (world[t[0] as usize], world[t[1] as usize], world[t[2] as usize]);
            if heat {
                let p = (push[t[0] as usize] + push[t[1] as usize] + push[t[2] as usize]) / (3.0 * full);
                gfx.color = if p >= 0.0 { mix(p.min(1.0), [255.0, 96.0, 40.0]) } else { mix((-p).min(1.0), [60.0, 120.0, 255.0]) };
            }
            if relief_view {
                let mut n = [0.0f32; 3];
                for v in t {
                    for (k, c) in n.iter_mut().enumerate() {
                        *c += normals[*v as usize][k];
                    }
                }
                let len = (n[0] * n[0] + n[1] * n[1] + n[2] * n[2]).sqrt().max(1e-9);
                let n = [n[0] / len, n[1] / len, n[2] / len];
                gfx.color = if mode == 3 {
                    let ch = |v: f32| ((v * 0.5 + 0.5).clamp(0.0, 1.0) * 255.0) as u32;
                    (ch(n[0]) << 16) | (ch(n[1]) << 8) | ch(n[2])
                } else {
                    let lit = n[0] * towards_light[0] + n[1] * towards_light[1] + n[2] * towards_light[2];
                    let k = 0.14 + 0.86 * lit.max(0.0).powf(0.9);
                    let ch = |s: u32| (((s as f32) * k).min(255.0)) as u32;
                    (ch((base >> 16) & 255) << 16) | (ch((base >> 8) & 255) << 8) | ch(base & 255)
                };
            }
            if smooth {
                let n: Vec<f32> = (0..3)
                    .map(|k| normals[t[0] as usize][k] + normals[t[1] as usize][k] + normals[t[2] as usize][k])
                    .collect();
                gfx.normal_override = Some([n[0], n[1], n[2]]);
            }
            gfx.submit_triangle(a[0], a[1], a[2], cc[0], cc[1], cc[2], b[0], b[1], b[2]);
        }
        gfx.normal_override = None;
        gfx.color = base;
        gfx.flat_shade = lit_before;
        Ok(Value::Unit)
    }

    /// `body_draw_creases(yaw, ox, oy, oz, scale)`: draws the crease curves with the pen colour,
    /// pulled slightly toward the camera so they stay visible on the surface.
    fn body_draw_creases(&mut self, args: &[Value]) -> EvalResult {
        let yaw = num(args, 0, 0.0) as f32;
        let (ox, oy, oz) = (num(args, 1, 0.0) as f32, num(args, 2, 0.0) as f32, num(args, 3, 0.0) as f32);
        let s = num(args, 4, 1.0) as f32;
        let (c, sn) = (yaw.cos(), yaw.sin());
        let place = |p: [f32; 3]| [ox + s * (p[0] * c + p[2] * sn), oy - s * p[1], oz + s * (-p[0] * sn + p[2] * c)];
        let mut gfx = self.gfx.borrow_mut();
        let near = -gfx.camera.zdist + 0.05;
        let color = gfx.color;
        for curve in self.body.kit.creases() {
            let line: Vec<[f32; 3]> = ling_body::curves::flatten(curve, 8).into_iter().map(place).collect();
            for w in line.windows(2) {
                let (a, b) = (w[0], w[1]);
                if gfx.camera.depth(a[0], a[1], a[2]) <= near || gfx.camera.depth(b[0], b[1], b[2]) <= near {
                    continue;
                }
                let (sax, say, da) = gfx.camera.project(a[0], a[1], a[2]);
                let (sbx, sby, db) = gfx.camera.project(b[0], b[1], b[2]);
                let depth = (da + db) * 0.5 - 0.03;
                let col = gfx.fog_apply(color, depth);
                gfx.depth_queue.push_line(depth, col, sax, say, sbx, sby);
            }
        }
        Ok(Value::Unit)
    }

    /// `None` when `name` is not a `body_*` builtin.
    pub(super) fn body_builtin(&mut self, name: &str, args: &[Value]) -> Option<EvalResult> {
        let rest = name.strip_prefix("body_")?;
        if rest == "draw" {
            return Some(self.body_draw(args));
        }
        if rest == "draw_creases" {
            return Some(self.body_draw_creases(args));
        }
        let st = &mut self.body;
        let v = match rest {
            "message" => Value::Str(st.message.clone()),
            "grid_info" => Value::Str(st.kit.grid_info().unwrap_or_default()),
            "notes" => Value::List(Rc::new(st.kit.notes().iter().map(|n| Value::Str(n.clone())).collect())),

            "param_count" => Value::Number(st.kit.param_count() as f64),
            "param_name" | "param_group" | "param_min" | "param_max" | "param_default" => {
                let def = match args.first() {
                    Some(Value::Str(s)) => ling_body::Params::def(s).copied(),
                    _ => index(args, 0).and_then(|i| st.kit.param_def(i)),
                };
                match def {
                    Some(d) => match rest {
                        "param_name" => Value::Str(d.0.to_string()),
                        "param_group" => Value::Str(d.4.to_string()),
                        "param_min" => Value::Number(d.2),
                        "param_max" => Value::Number(d.3),
                        _ => Value::Number(d.1),
                    },
                    None => Value::Unit,
                }
            },
            "param_get" => Value::Number(st.kit.params.get(&text(args, 0))),
            "param_set" => flag(st.kit.param_set(&text(args, 0), num(args, 1, 0.0))),
            "param_reset" => {
                st.kit.params.reset();
                st.kit.source_clear();
                Value::Unit
            },

            "src_count" => Value::Number(st.kit.source_count() as f64),
            "src_add" => Value::Number(st.kit.source_add(
                [num(args, 0, 0.0) as f32, num(args, 1, 1.0) as f32, num(args, 2, 0.0) as f32],
                num(args, 3, 0.0) as i64,
                num(args, 4, 220.0) as f32,
            ) as f64),
            "src_at_joint" => {
                let joint = text(args, 0);
                match st.kit.joints().into_iter().find(|j| j.0 == joint) {
                    Some((_, p)) => {
                        let i = st.kit.source_add(p, num(args, 1, 0.0) as i64, num(args, 2, 220.0) as f32);
                        st.kit.source_rename(i, &joint);
                        Value::Number(i as f64)
                    },
                    None => Value::Number(-1.0),
                }
            },
            "src_del" => {
                if let Some(i) = index(args, 0) {
                    st.kit.source_remove(i);
                }
                Value::Unit
            },
            "src_clear" => {
                st.kit.source_clear();
                Value::Unit
            },
            "src_get" => Value::Number(index(args, 0).map_or(0.0, |i| st.kit.source_get(i, &text(args, 1)))),
            "src_set" => flag(index(args, 0).is_some_and(|i| st.kit.source_set(i, &text(args, 1), num(args, 2, 0.0)))),
            "src_name" => Value::Str(index(args, 0).map(|i| st.kit.source_name(i)).unwrap_or_default()),

            "joint_count" => Value::Number(st.kit.joints().len() as f64),
            "joint_name" => Value::Str(index(args, 0).and_then(|i| st.kit.joints().get(i).map(|j| j.0.clone())).unwrap_or_default()),
            "joint_pos" => match index(args, 0).and_then(|i| st.kit.joints().get(i).map(|j| j.1)) {
                Some(p) => floats(&p),
                None => floats(&[0.0; 3]),
            },
            "joint_parent" => Value::Number(index(args, 0).map_or(-1, |i| st.kit.joint_parent(i)) as f64),

            "model_load" => match st.kit.load_model(&text(args, 0)) {
                Ok(h) => {
                    st.message = format!("{} parts", st.kit.model_parts(h).len());
                    Value::Number(h as f64)
                },
                Err(e) => {
                    st.message = e;
                    Value::Number(-1.0)
                },
            },
            "model_use" => {
                st.kit.use_model(index(args, 0));
                Value::Unit
            },
            "model_active" => Value::Number(st.kit.active_model().map_or(-1.0, |h| h as f64)),
            "model_part_count" => Value::Number(index(args, 0).map_or(0, |h| st.kit.model_parts(h).len()) as f64),
            "model_part_name" | "model_part_tris" | "model_part_enabled" => {
                let part = index(args, 0).zip(index(args, 1)).and_then(|(h, i)| st.kit.model_parts(h).into_iter().nth(i));
                match (rest, part) {
                    ("model_part_name", Some(p)) => Value::Str(p.0),
                    ("model_part_tris", Some(p)) => Value::Number(p.1 as f64),
                    ("model_part_enabled", Some(p)) => flag(p.2),
                    ("model_part_name", None) => Value::Str(String::new()),
                    _ => Value::Number(0.0),
                }
            },
            "model_part_set" => {
                if let (Some(h), Some(i)) = (index(args, 0), index(args, 1)) {
                    st.kit.set_part(h, i, num(args, 2, 1.0) != 0.0);
                }
                Value::Unit
            },

            "preview" => {
                let r = st.kit.preview_rest().map(|_| "rest shape ready".to_string());
                st.done(r)
            },
            "sim_begin" => match st.kit.sim_begin() {
                Ok(total) => {
                    st.message = format!("{total} steps");
                    Value::Number(total as f64)
                },
                Err(e) => {
                    st.message = e;
                    Value::Number(-1.0)
                },
            },
            "sim_step" => {
                let (done, total) = st.kit.sim_advance(num(args, 0, 30.0) as f32);
                numbers(&[done as f64, total as f64])
            },
            "sim_active" => flag(st.kit.sim_active()),
            "sim_done" => flag(st.kit.sim_done()),
            "mesh_update" => {
                let r = st.kit.mesh_from_sim().map(|_| "mesh updated".to_string());
                st.done(r)
            },
            "displacement" => {
                let (inward, outward) = st.kit.displacement_mm();
                floats(&[inward, outward])
            },
            "slice" => match st.kit.slice(num(args, 0, 2.0) as usize, num(args, 1, 0.5) as f32) {
                Some((w, h, vals)) => {
                    let peak = vals.iter().fold(1e-30f32, |m, v| m.max(v.abs()));
                    let mut out = Vec::with_capacity(vals.len() + 2);
                    out.push(Value::Number(w as f64));
                    out.push(Value::Number(h as f64));
                    out.extend(vals.iter().map(|v| Value::Number((v / peak) as f64)));
                    Value::List(Rc::new(out))
                },
                None => numbers(&[0.0, 0.0]),
            },
            "mic_tail" => floats(&st.kit.mic_tail(num(args, 0, 256.0).max(1.0) as usize)),

            "mesh_counts" => {
                let (v, t, wt) = st.kit.mesh_info();
                numbers(&[v as f64, t as f64, wt as i32 as f64, st.kit.recovered_from_wav() as i32 as f64])
            },
            "mesh_bounds" => match st.kit.body_bounds() {
                Some((lo, hi)) => floats(&[lo[0], lo[1], lo[2], hi[0], hi[1], hi[2]]),
                None => floats(&[0.0; 6]),
            },
            "mesh_positions" => st.kit.mesh().map_or_else(|| floats(&[]), |m| floats(&m.pos)),
            "mesh_normals" => st.kit.mesh().map_or_else(|| floats(&[]), |m| floats(&m.nrm)),
            "mesh_indices" => match st.kit.mesh() {
                Some(m) => Value::List(Rc::new(m.idx.iter().map(|&i| Value::Number(i as f64)).collect())),
                None => floats(&[]),
            },

            "crease_detect" => {
                let r = st.kit.crease_detect();
                st.done(r)
            },
            "crease_apply" => {
                let r = st.kit.crease_apply();
                st.done(r)
            },
            "detail_reset" => {
                let r = st.kit.detail_reset();
                st.done(r)
            },
            "crease_clear" => {
                st.kit.crease_clear();
                Value::Unit
            },
            "crease_count" => Value::Number(st.kit.creases().len() as f64),
            "crease_segments" => Value::Number(st.kit.creases().iter().map(|c| c.segs.len()).sum::<usize>() as f64),
            "crease_export" => {
                let r = st.kit.crease_export(&text(args, 0));
                st.done(r)
            },
            "listen_begin" => match st.kit.listen_begin() {
                Ok(total) => {
                    st.message = format!("{total} steps");
                    Value::Number(total as f64)
                },
                Err(e) => {
                    st.message = e;
                    Value::Number(-1.0)
                },
            },
            "listen_step" => {
                let (done, total) = st.kit.listen_advance(num(args, 0, 30.0) as f32);
                numbers(&[done as f64, total as f64])
            },
            "listen_done" => flag(st.kit.listen_done()),
            "listen_finish" => {
                let r = st.kit.listen_finish(&text(args, 0));
                st.done(r)
            },
            "listen_spectrum" => floats(st.kit.listen_spectrum()),
            "export_mesh" => {
                let r = st.kit.export_mesh(&text(args, 0));
                st.done(r)
            },
            "export_wav" => {
                let r = st.kit.wav_export(&text(args, 0));
                st.done(r)
            },
            "import_wav" => {
                let r = st.kit.wav_import(&text(args, 0), num(args, 1, 0.0) != 0.0);
                st.done(r)
            },
            "project_save" => {
                let path = text(args, 0);
                let r = std::fs::write(&path, st.kit.project_json()).map(|_| format!("saved {path}")).map_err(|e| format!("{path}: {e}"));
                st.done(r)
            },
            "project_load" => {
                let path = text(args, 0);
                let r = std::fs::read_to_string(&path)
                    .map_err(|e| format!("{path}: {e}"))
                    .and_then(|t| st.kit.project_load(&t))
                    .map(|_| format!("loaded {path}"));
                st.done(r)
            },
            _ => return None,
        };
        Some(Ok(v))
    }
}
