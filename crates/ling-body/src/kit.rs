//! `BodyKit` is the stateful façade the runtime exposes to Ling programs.
//! Everything crossing the boundary is a number, a string or a flat float list.

use crate::analysis;
use crate::codec;
use crate::curves::{self, CreaseCfg, Curve};
use crate::detail::{self, DetailCfg};
use crate::export;
use crate::humanoid::Humanoid;
use crate::mesh::{Mesh, Tris};
use crate::mesher;
use crate::model::{self, Model};
use crate::params::{Params, Source, Waveform, DEFS};
use crate::signature::Signature;
use crate::sim::{build_rest, Rest, Sim};
use serde_json::json;

type Joint = (String, [f32; 3]);

/// Everything detail synthesis needs from the simulation that produced the mesh.
struct DetailCtx {
    base: Mesh,
    scalar: Vec<f32>,
    phi: Vec<f32>,
    grid: crate::Grid,
}

#[derive(Default)]
pub struct BodyKit {
    pub params: Params,
    models: Vec<Model>,
    active_model: Option<usize>,
    rest: Option<Rest>,
    sim: Option<Sim>,
    mesh: Option<Mesh>,
    /// Outward skin displacement per mesh vertex in millimetres; empty unless the mesh came from a simulation.
    push: Vec<f32>,
    /// Mesh recovered verbatim from an imported WAV.
    recovered: bool,
    audio: Option<(Vec<f32>, Vec<f32>, u32)>,
    notes: Vec<String>,
    creases: Vec<Curve>,
    detail: Option<DetailCtx>,
    listen: Option<Signature>,
    listen_spectrum: Vec<f32>,
}

impl BodyKit {
    pub fn new() -> BodyKit {
        BodyKit::default()
    }

    pub fn param_count(&self) -> usize {
        DEFS.len()
    }

    pub fn param_def(&self, i: usize) -> Option<(&'static str, f64, f64, f64, &'static str)> {
        DEFS.get(i).copied()
    }

    /// Forgets crease curves and the simulation snapshot they were found on.
    fn drop_detail(&mut self) {
        self.creases.clear();
        self.detail = None;
    }

    fn invalidate(&mut self) {
        self.rest = None;
        self.sim = None;
    }

    pub fn param_set(&mut self, name: &str, v: f64) -> bool {
        let ok = self.params.set(name, v);
        if ok {
            match Params::def(name).map(|d| d.4) {
                Some("body") | Some("sim") => self.invalidate(),
                Some("medium") | Some("mic") | Some("membrane") => {
                    if matches!(name, "c_air" | "c_body") {
                        self.invalidate();
                    } else {
                        self.sim = None;
                    }
                },
                _ => {},
            }
        }
        ok
    }

    pub fn notes(&self) -> &[String] {
        &self.notes
    }

    pub fn models_loaded(&self) -> usize {
        self.models.len()
    }

    pub fn load_model(&mut self, path: &str) -> Result<usize, String> {
        let m = Model::load(path)?;
        self.models.push(m);
        Ok(self.models.len() - 1)
    }

    pub fn model_parts(&self, h: usize) -> Vec<(String, usize, bool)> {
        self.models
            .get(h)
            .map(|m| m.parts.iter().map(|p| (p.name.clone(), p.tris.len(), p.enabled)).collect())
            .unwrap_or_default()
    }

    pub fn set_part(&mut self, h: usize, part: usize, on: bool) {
        if let Some(p) = self.models.get_mut(h).and_then(|m| m.parts.get_mut(part)) {
            p.enabled = on;
            if self.active_model == Some(h) {
                self.invalidate();
            }
        }
    }

    /// Selects the loaded model as the body, or the parametric scaffold for `None`.
    pub fn use_model(&mut self, h: Option<usize>) {
        self.active_model = h.filter(|&i| i < self.models.len());
        self.invalidate();
        self.drop_detail();
        self.recovered = false;
    }

    pub fn active_model(&self) -> Option<usize> {
        self.active_model
    }

    fn body_tris(&self) -> Option<(Tris, Vec<Joint>)> {
        let m = self.models.get(self.active_model?)?;
        let mut t = m.tris();
        let mut j = m.joints.clone();
        model::normalize(&mut t, &mut j, Some(self.params.getf("height")));
        Some((t, j))
    }

    pub fn build_rest(&mut self) -> Result<(), String> {
        if self.rest.is_some() {
            return Ok(());
        }
        let tris = self.body_tris().map(|b| b.0);
        self.rest = Some(build_rest(&self.params, tris.as_ref())?);
        Ok(())
    }

    /// Joint hierarchy of the scaffold fitted to the current body.
    pub fn joints(&self) -> Vec<(String, [f32; 3])> {
        let rig = match &self.rest {
            Some(r) => r.rig.clone(),
            None => match self.body_tris().and_then(|b| b.0.bounds()) {
                Some((lo, hi)) => Humanoid::fit(lo, hi),
                None => Humanoid::new(
                    self.params.getf("height"),
                    self.params.getf("build"),
                    self.params.getf("arm_span"),
                    self.params.getf("head_scale"),
                ),
            },
        };
        rig.joints.iter().map(|j| (j.name.clone(), j.pos)).collect()
    }

    pub fn joint_parent(&self, i: usize) -> i64 {
        let rig = Humanoid::new(1.0, 1.0, 1.0, 1.0);
        rig.joints.get(i).and_then(|j| j.parent).map_or(-1, |p| p as i64)
    }

    pub fn grid_info(&self) -> Option<String> {
        let r = self.rest.as_ref()?;
        let g = r.grid;
        Some(format!("{}x{}x{} @ {:.1} mm", g.nx, g.ny, g.nz, g.dx * 1000.0))
    }

    pub fn source_count(&self) -> usize {
        self.params.sources.len()
    }

    pub fn source_add(&mut self, pos: [f32; 3], wave: i64, freq: f32) -> usize {
        let n = self.params.sources.len();
        self.params.sources.push(Source::new(&format!("src{n}"), pos, Waveform::from_index(wave), freq.max(1.0)));
        self.sim = None;
        n
    }

    pub fn source_rename(&mut self, i: usize, name: &str) {
        if let Some(s) = self.params.sources.get_mut(i) {
            s.name = name.to_string();
        }
    }

    pub fn source_remove(&mut self, i: usize) {
        if i < self.params.sources.len() {
            self.params.sources.remove(i);
            self.sim = None;
        }
    }

    pub fn source_clear(&mut self) {
        self.params.sources.clear();
        self.sim = None;
    }

    pub fn source_get(&self, i: usize, field: &str) -> f64 {
        let Some(s) = self.params.sources.get(i) else { return 0.0 };
        match field {
            "x" => s.pos[0] as f64,
            "y" => s.pos[1] as f64,
            "z" => s.pos[2] as f64,
            "wave" => s.wave.index() as f64,
            "freq" => s.freq as f64,
            "amp" => s.amp as f64,
            "phase" => s.phase as f64,
            "start" => s.start as f64,
            "dur" => s.dur as f64,
            _ => 0.0,
        }
    }

    pub fn source_set(&mut self, i: usize, field: &str, v: f64) -> bool {
        let Some(s) = self.params.sources.get_mut(i) else { return false };
        let v = if v.is_finite() { v as f32 } else { 0.0 };
        match field {
            "x" => s.pos[0] = v,
            "y" => s.pos[1] = v,
            "z" => s.pos[2] = v,
            "wave" => s.wave = Waveform::from_index(v as i64),
            "freq" => s.freq = v.clamp(1.0, 40000.0),
            "amp" => s.amp = v,
            "phase" => s.phase = v,
            "start" => s.start = v.max(0.0),
            "dur" => s.dur = v.max(0.0),
            _ => return false,
        }
        self.sim = None;
        true
    }

    pub fn source_name(&self, i: usize) -> String {
        self.params.sources.get(i).map(|s| s.name.clone()).unwrap_or_default()
    }

    /// Voxelises the body and extracts its rest shape.
    pub fn preview_rest(&mut self) -> Result<(), String> {
        self.build_rest()?;
        let r = self.rest.as_ref().ok_or("rest body missing")?;
        self.mesh = Some(self.build_mesh(&r.phi0, &r.grid));
        self.push.clear();
        self.drop_detail();
        self.recovered = false;
        Ok(())
    }

    fn build_mesh(&self, phi: &[f32], g: &crate::Grid) -> Mesh {
        mesher::build(phi, g, self.params.getf("iso"), self.params.get("smooth") as usize, self.params.getf("decimate"))
    }

    pub fn sim_begin(&mut self) -> Result<usize, String> {
        self.build_rest()?;
        let rest = self.rest.as_ref().ok_or("rest body missing")?;
        let s = Sim::begin(&self.params, rest)?;
        let total = s.total;
        self.sim = Some(s);
        self.audio = None;
        self.recovered = false;
        Ok(total)
    }

    /// Runs the simulation for about `budget_ms`; returns `(steps_done, total)`.
    pub fn sim_advance(&mut self, budget_ms: f32) -> (usize, usize) {
        match self.sim.as_mut() {
            Some(s) => {
                s.advance(budget_ms);
                (s.steps_done(), s.total)
            },
            None => (0, 0),
        }
    }

    pub fn sim_active(&self) -> bool {
        self.sim.is_some()
    }

    pub fn sim_done(&self) -> bool {
        self.sim.as_ref().is_some_and(Sim::done)
    }

    /// Extracts the membrane's current zero level set as the working mesh.
    pub fn mesh_from_sim(&mut self) -> Result<(), String> {
        let s = self.sim.as_ref().ok_or("no simulation running")?;
        let m = self.build_mesh(&s.membrane.phi, &s.membrane.grid);
        let g = s.membrane.grid;
        let (rest, now) = (s.membrane.rest(), &s.membrane.phi);
        self.push = m
            .pos
            .as_chunks::<3>().0.iter()
            .map(|p| {
                let l = g.lattice([p[0], p[1], p[2]]);
                (g.sample(rest, l) - g.sample(now, l)) * 1000.0
            })
            .collect();
        let done = s.done();
        self.mesh = Some(m);
        self.drop_detail();
        self.recovered = false;
        if done {
            self.capture_audio();
        }
        Ok(())
    }

    fn capture_audio(&mut self) {
        if let Some(s) = &self.sim {
            if s.wave.mic_signal.len() == 2 {
                let rate = self.params.get("sr").round() as u32;
                self.audio = Some((s.wave.mic_signal[0].clone(), s.wave.mic_signal[1].clone(), rate));
            }
        }
    }

    pub fn displacement_mm(&self) -> (f32, f32) {
        self.sim.as_ref().map_or((0.0, 0.0), |s| {
            let (a, b) = s.membrane.displacement_range();
            (a * 1000.0, b * 1000.0)
        })
    }

    pub fn slice(&self, axis: usize, frac: f32) -> Option<(usize, usize, Vec<f32>)> {
        self.sim.as_ref().map(|s| s.slice(axis, frac))
    }

    pub fn mic_tail(&self, count: usize) -> Vec<f32> {
        self.sim.as_ref().and_then(|s| s.wave.mic_signal.first()).map_or_else(Vec::new, |m| {
            m[m.len().saturating_sub(count)..].to_vec()
        })
    }

    /// Finds crease curves on the current mesh from the simulated acoustic field and
    /// stores the snapshot needed to carve them later.
    pub fn crease_detect(&mut self) -> Result<String, String> {
        let s = self.sim.as_ref().ok_or("simulate first: creases come from the pressure field")?;
        let base = match &self.detail {
            Some(d) => d.base.clone(),
            None => self.mesh.clone().ok_or("no mesh; simulate first")?,
        };
        let pts: Vec<[f32; 3]> = base.pos.as_chunks::<3>().0.to_vec();
        let nodal = self.params.get("crease_mode") >= 0.5;
        let mut field = s.surface_field(&pts, !nodal);
        smooth_scalar(&base, &mut field, 4);
        let cfg = CreaseCfg {
            levels: self.params.get("crease_levels").round() as usize,
            tolerance: self.params.getf("crease_tol") / 1000.0,
            min_length: self.params.getf("crease_min") / 1000.0,
        };
        self.creases = curves::detect(&base, &field, &cfg, nodal);
        self.detail = Some(DetailCtx { base, scalar: field, phi: s.membrane.phi.clone(), grid: s.membrane.grid });
        let segs: usize = self.creases.iter().map(|c| c.segs.len()).sum();
        Ok(format!("{} crease curves, {} Bezier segments", self.creases.len(), segs))
    }

    /// Subdivides the mesh and carves grooves and noise along the detected creases.
    pub fn crease_apply(&mut self) -> Result<String, String> {
        let ctx = self.detail.as_ref().ok_or("find creases first")?;
        let cfg = DetailCfg {
            levels: self.params.get("detail_level").round() as usize,
            depth: self.params.getf("crease_depth") / 1000.0,
            width: self.params.getf("crease_width") / 1000.0,
            noise_amp: self.params.getf("noise_amp") / 1000.0,
            noise_freq: self.params.getf("noise_freq"),
        };
        let m = detail::build(&ctx.base, &ctx.scalar, &ctx.phi, &ctx.grid, &self.creases, &cfg);
        let tris = m.tri_count();
        self.mesh = Some(m);
        self.push.clear();
        self.recovered = false;
        Ok(format!("detail added: {tris} triangles"))
    }

    /// Returns to the smooth mesh the creases were found on.
    pub fn detail_reset(&mut self) -> Result<String, String> {
        let ctx = self.detail.as_ref().ok_or("no detail to remove")?;
        self.mesh = Some(ctx.base.clone());
        self.push.clear();
        Ok("smooth mesh restored".into())
    }

    pub fn crease_clear(&mut self) {
        if self.detail.is_some() {
            let _ = self.detail_reset();
        }
        self.drop_detail();
    }

    pub fn creases(&self) -> &[Curve] {
        &self.creases
    }

    pub fn crease_export(&self, path: &str) -> Result<String, String> {
        if self.creases.is_empty() {
            return Err("no creases to export; find creases first".into());
        }
        let lower = path.to_lowercase();
        let text = if lower.ends_with(".py") {
            curves::to_blender_script(&self.creases)
        } else if lower.ends_with(".json") {
            curves::to_json(&self.creases)
        } else {
            return Err("crease export must be .json or .py".into());
        };
        export::write(path, text.as_bytes())?;
        Ok(format!("{} curves → {path}", self.creases.len()))
    }

    pub fn listen_begin(&mut self) -> Result<usize, String> {
        let mesh = self.mesh.as_ref().ok_or("no mesh to listen to; preview a body or simulate first")?;
        let s = Signature::begin(mesh, &self.params)?;
        let total = s.total();
        self.listen = Some(s);
        Ok(total)
    }

    /// Returns `(steps_done, total)`.
    pub fn listen_advance(&mut self, budget_ms: f32) -> (usize, usize) {
        match self.listen.as_mut() {
            Some(s) => {
                s.advance(budget_ms);
                (s.steps_done(), s.total())
            },
            None => (0, 0),
        }
    }

    pub fn listen_done(&self) -> bool {
        self.listen.as_ref().is_some_and(Signature::done)
    }

    /// Renders the finished signature to a WAV and keeps its spectrum and resonance notes.
    pub fn listen_finish(&mut self, path: &str) -> Result<String, String> {
        let s = self.listen.as_ref().ok_or("start listening first")?;
        let out = s.render(path)?;
        self.listen_spectrum = out.spectrum;
        self.notes = out.notes;
        Ok(format!("{:.2} s signature → {path}", out.left.len() as f32 / out.rate as f32))
    }

    pub fn listen_spectrum(&self) -> &[f32] {
        &self.listen_spectrum
    }

    pub fn mesh(&self) -> Option<&Mesh> {
        self.mesh.as_ref()
    }

    /// Per-vertex outward skin displacement in millimetres, or an empty slice.
    pub fn mesh_push(&self) -> &[f32] {
        match &self.mesh {
            Some(m) if self.push.len() == m.vert_count() => &self.push,
            _ => &[],
        }
    }

    pub fn mesh_info(&self) -> (usize, usize, bool) {
        self.mesh.as_ref().map_or((0, 0, false), |m| (m.vert_count(), m.tri_count(), m.is_watertight()))
    }

    pub fn recovered_from_wav(&self) -> bool {
        self.recovered
    }

    pub fn body_bounds(&self) -> Option<([f32; 3], [f32; 3])> {
        self.mesh.as_ref().and_then(Mesh::bounds)
    }

    pub fn export_mesh(&self, path: &str) -> Result<String, String> {
        let m = self.mesh.as_ref().ok_or("no mesh to export; simulate or preview first")?;
        let lower = path.to_lowercase();
        if lower.ends_with(".obj") {
            export::write(path, export::obj_text(m).as_bytes())?;
        } else if lower.ends_with(".stl") {
            export::write(path, &export::stl_bytes(m))?;
        } else if lower.ends_with(".glb") {
            let rig = self.fitted_rig(m);
            export::write(path, &export::glb_bytes(m, Some(&rig)))?;
        } else {
            return Err("export format must be .obj, .stl or .glb".into());
        }
        Ok(format!("{} triangles → {path}", m.tri_count()))
    }

    fn fitted_rig(&self, m: &Mesh) -> Humanoid {
        match m.bounds() {
            Some((lo, hi)) => Humanoid::fit(lo, hi),
            None => Humanoid::new(1.7, 1.0, 1.0, 1.0),
        }
    }

    pub fn wav_export(&self, path: &str) -> Result<String, String> {
        let (l, r, rate) = self.audio.as_ref().ok_or("run the simulation to completion first")?;
        let repeat = self.params.get("wav_repeat").round().max(1.0) as usize;
        let period = l.len() + (self.params.get("wav_gap") * *rate as f64).round() as usize;
        let tile = |x: &[f32]| {
            let mut out = Vec::with_capacity(period * repeat);
            for _ in 0..repeat {
                out.extend_from_slice(x);
                out.resize(out.len() + period - x.len(), 0.0);
            }
            out
        };
        let (l, r) = (tile(l), tile(r));
        let bytes = codec::encode_body(
            &self.params,
            self.mesh.as_ref(),
            &l,
            &r,
            *rate,
            self.params.get("wav_bits").round() as u32,
            self.params.getf("wav_gain"),
        );
        export::write(path, &bytes)?;
        Ok(format!("{:.2} s, {} KB → {path}", l.len() as f32 / *rate as f32, bytes.len() / 1024))
    }

    /// Loads a WAV. Embedded parameters and mesh are used verbatim unless
    /// `analyze_only` is set, in which case the audio alone is analysed.
    pub fn wav_import(&mut self, path: &str, analyze_only: bool) -> Result<String, String> {
        let bytes = std::fs::read(path).map_err(|e| format!("{path}: {e}"))?;
        let d = codec::decode_body(&bytes)?;
        self.notes.clear();
        self.invalidate();
        let seconds = d.left.len() as f32 / d.rate.max(1) as f32;
        self.audio = Some((d.left.clone(), d.right.clone(), d.rate));
        self.drop_detail();
        if let (Some(p), false) = (&d.params, analyze_only) {
            self.params = p.clone();
            self.recovered = d.mesh.is_some();
            self.mesh = d.mesh;
            self.notes.push("parameters read from the file's LBDY chunk".into());
            if self.recovered {
                self.notes.push("mesh restored from the LBMS chunk".into());
            }
            return Ok(format!("exact recipe, {} sources, {:.2} s", self.params.sources.len(), seconds));
        }
        let a = analysis::analyze(&d.left, &d.right, d.rate, &self.params)?;
        self.params = a.params;
        self.notes = a.notes;
        self.recovered = false;
        self.mesh = None;
        Ok(format!("estimated from audio, {} sources, {:.2} s", self.params.sources.len(), seconds))
    }

    pub fn project_json(&self) -> String {
        let mut v = self.params.to_json();
        let model = self.active_model.and_then(|i| self.models.get(i));
        v["model"] = json!(model.map(|m| m.path.clone()));
        v["parts"] = json!(model.map(|m| m.parts.iter().map(|p| p.enabled).collect::<Vec<_>>()));
        serde_json::to_string_pretty(&v).unwrap_or_default()
    }

    pub fn project_load(&mut self, text: &str) -> Result<(), String> {
        let v: serde_json::Value = serde_json::from_str(text).map_err(|e| e.to_string())?;
        self.params = Params::from_json(&v)?;
        self.invalidate();
        self.mesh = None;
        self.active_model = None;
        if let Some(path) = v.get("model").and_then(|m| m.as_str()) {
            let h = self.load_model(path)?;
            if let Some(flags) = v.get("parts").and_then(|p| p.as_array()) {
                for (i, f) in flags.iter().enumerate() {
                    self.set_part(h, i, f.as_bool().unwrap_or(true));
                }
            }
            self.use_model(Some(h));
        }
        Ok(())
    }
}

/// Laplacian smoothing of a per-vertex scalar over the mesh edges.
fn smooth_scalar(mesh: &Mesh, s: &mut [f32], passes: usize) {
    let (start, nbr) = mesher::adjacency(mesh.vert_count(), &mesh.idx);
    for _ in 0..passes {
        let src = s.to_vec();
        for (v, out) in s.iter_mut().enumerate() {
            let ring = &nbr[start[v]..start[v + 1]];
            if !ring.is_empty() {
                let avg = ring.iter().map(|&u| src[u as usize]).sum::<f32>() / ring.len() as f32;
                *out = 0.5 * src[v] + 0.5 * avg;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn end_to_end_wav_round_trip() {
        let mut kit = BodyKit::new();
        kit.param_set("sr", 11025.0);
        kit.param_set("steps", 240.0);
        kit.param_set("margin", 10.0);
        kit.param_set("sponge", 6.0);
        kit.source_add([0.0, 1.25, 0.0], 0, 500.0);
        kit.source_add([0.5, 1.4, 0.0], 1, 900.0);
        kit.sim_begin().unwrap();
        while !kit.sim_done() {
            kit.sim_advance(1000.0);
        }
        kit.mesh_from_sim().unwrap();
        let (_, tris, wt) = kit.mesh_info();
        assert!(tris > 1000 && wt);

        let path = std::env::temp_dir().join("ling_body_kit_test.wav");
        let path = path.to_str().unwrap();
        kit.wav_export(path).unwrap();
        let wav = codec::decode_body(&std::fs::read(path).unwrap()).unwrap();
        let repeat = kit.params.get("wav_repeat") as usize;
        let period = 240 + (kit.params.get("wav_gap") * 11025.0).round() as usize;
        assert_eq!(wav.left.len(), repeat * period);
        assert!(!kit.mesh_push().is_empty(), "a simulated mesh carries per-vertex skin displacement");

        let mut other = BodyKit::new();
        let msg = other.wav_import(path, false).unwrap();
        assert!(msg.starts_with("exact recipe"), "{msg}");
        assert_eq!(other.mesh_info().1, tris);
        assert_eq!(other.params.sources.len(), 2);

        let mut rough = BodyKit::new();
        let msg = rough.wav_import(path, true).unwrap();
        assert!(msg.starts_with("estimated"), "{msg}");
        assert!(!rough.params.sources.is_empty());
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn creases_detail_and_listening_work_on_a_simulated_body() {
        let mut kit = BodyKit::new();
        kit.param_set("sr", 11025.0);
        kit.param_set("steps", 300.0);
        kit.param_set("margin", 10.0);
        kit.param_set("sponge", 6.0);
        kit.param_set("sig_dur", 0.1);
        kit.param_set("sig_sr", 12000.0);
        kit.source_add([0.0, 1.25, 0.0], 0, 500.0);
        kit.source_add([0.4, 1.4, 0.0], 1, 800.0);
        assert!(kit.crease_detect().is_err(), "creases need a simulation");
        kit.sim_begin().unwrap();
        while !kit.sim_done() {
            kit.sim_advance(1000.0);
        }
        kit.mesh_from_sim().unwrap();
        let smooth_tris = kit.mesh_info().1;

        let msg = kit.crease_detect().unwrap();
        assert!(!kit.creases().is_empty(), "{msg}");
        kit.crease_apply().unwrap();
        let (_, tris, watertight) = kit.mesh_info();
        assert!(tris >= smooth_tris * 4 && watertight, "{tris} triangles, watertight {watertight}");
        kit.detail_reset().unwrap();
        assert_eq!(kit.mesh_info().1, smooth_tris);

        let dir = std::env::temp_dir();
        let json = dir.join("ling_body_creases.json");
        let py = dir.join("ling_body_creases.py");
        kit.crease_export(json.to_str().unwrap()).unwrap();
        kit.crease_export(py.to_str().unwrap()).unwrap();
        assert!(std::fs::read_to_string(&json).unwrap().contains("ling-body/creases"));
        assert!(std::fs::read_to_string(&py).unwrap().contains("bpy.data.curves.new"));
        assert!(kit.crease_export("x.txt").is_err());

        let wav = dir.join("ling_body_listen.wav");
        let total = kit.listen_begin().unwrap();
        assert_eq!(total, 1200);
        while !kit.listen_done() {
            kit.listen_advance(1000.0);
        }
        kit.listen_finish(wav.to_str().unwrap()).unwrap();
        assert_eq!(kit.listen_spectrum().len(), 48);
        assert!(!kit.notes().is_empty());
        let heard = codec::decode_body(&std::fs::read(&wav).unwrap()).unwrap();
        assert!(heard.left.iter().any(|v| v.abs() > 0.1));
        for f in [json, py, wav] {
            let _ = std::fs::remove_file(f);
        }
    }
}
