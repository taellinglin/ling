//! Ling Nova's parameters: one flat list of named 0..1 values (selectors are `index / (n - 1)`, switches 0 / 1), built once. The
//! indices of every group are kept in `Layout` so the voice code reads `v[l.osc[0].pos]` instead of looking names up.
//!
//! Every continuous parameter that makes sense to move is also a modulation destination; a few virtual destinations (pitch,
//! oscillator pitch, amp) sit in front of them. Destinations of the effect rack are global: the matrix drives them from the newest
//! voice (and the macros, the wheel) once a block.
use super::tables;
use std::collections::HashMap;
use std::sync::OnceLock;

#[derive(Clone, Copy, PartialEq, Debug)]
pub enum PK {
    Cont,
    Sel,
    Sw,
}

pub struct Spec {
    pub name: &'static str,
    pub def: f32,
    pub kind: PK,
    /// the choices of a selector
    pub opts: &'static [&'static str],
}

pub const WARPS: [&str; 15] = ["off", "sync", "bend+", "bend-", "pwm", "asym", "mirror", "remap", "quantize", "crush", "fold", "fm", "pm", "am", "rm"];
pub const W_OFF: usize = 0;
pub const W_SYNC: usize = 1;
pub const W_BENDP: usize = 2;
pub const W_BENDM: usize = 3;
pub const W_PWM: usize = 4;
pub const W_ASYM: usize = 5;
pub const W_MIRROR: usize = 6;
pub const W_REMAP: usize = 7;
pub const W_QUANT: usize = 8;
pub const W_CRUSH: usize = 9;
pub const W_FOLD: usize = 10;
pub const W_FM: usize = 11;
pub const W_PM: usize = 12;
pub const W_AM: usize = 13;
pub const W_RM: usize = 14;
/// the source of an oscillator's FM / PM / AM / RM warps
pub const MOD_SRC: [&str; 5] = ["a", "b", "c", "sub", "noise"];
pub const ROUTES: [&str; 4] = ["f1", "f2", "f1+f2", "dry"];
pub const SUB_SHAPES: [&str; 4] = ["sine", "triangle", "square", "saw"];
pub const SUB_OCT: [&str; 3] = ["-2", "-1", "0"];
pub const NOISES: [&str; 6] = ["white", "pink", "brown", "crackle", "digital", "metallic"];
pub const FTYPES: [&str; 14] = ["lp 12", "lp 24", "hp 12", "hp 24", "bp 12", "bp 24", "notch", "ladder", "diode", "comb +", "comb -", "flange", "phaser", "formant"];
pub const FROUTE: [&str; 2] = ["serial", "parallel"];
pub const LFO_SHAPES: [&str; 9] = ["sine", "triangle", "saw up", "saw down", "square", "pulse", "s&h", "smooth", "custom"];
pub const LFO_MODES: [&str; 3] = ["trig", "free", "env"];
pub const DIVS: [&str; 18] = ["8 bar", "4 bar", "2 bar", "1 bar", "1/2", "1/2D", "1/2T", "1/4", "1/4D", "1/4T", "1/8", "1/8D", "1/8T", "1/16", "1/16D", "1/16T", "1/32", "1/32T"];
pub const DIV_BEATS: [f32; 18] = [32.0, 16.0, 8.0, 4.0, 2.0, 3.0, 4.0 / 3.0, 1.0, 1.5, 2.0 / 3.0, 0.5, 0.75, 1.0 / 3.0, 0.25, 0.375, 1.0 / 6.0, 0.125, 1.0 / 12.0];
pub const RND_MODES: [&str; 2] = ["step", "note"];
pub const VOICE_MODES: [&str; 3] = ["poly", "mono", "legato"];
pub const PORTA_MODES: [&str; 2] = ["always", "legato"];
pub const SOURCES: [&str; 25] = ["-", "e1", "e2", "e3", "e4", "l1", "l2", "l3", "l4", "r1", "r2", "vel", "note", "press", "timbre", "wheel", "gate", "m1", "m2", "m3", "m4", "m5", "m6", "m7", "m8"];
pub const NSRC: usize = SOURCES.len();
pub const S_E1: usize = 1;
pub const S_L1: usize = 5;
pub const S_R1: usize = 9;
pub const S_VEL: usize = 11;
pub const S_NOTE: usize = 12;
pub const S_PRESS: usize = 13;
pub const S_TIMBRE: usize = 14;
pub const S_WHEEL: usize = 15;
pub const S_GATE: usize = 16;
pub const S_M1: usize = 17;
pub const NSLOT: usize = 32;
pub const DIST_MODES: [&str; 9] = ["tube", "soft", "hard", "diode", "fold", "sine fold", "downsample", "crush", "asym"];
pub const DIST_FILT: [&str; 7] = ["off", "pre lp", "pre bp", "pre hp", "post lp", "post bp", "post hp"];
pub const FX_FTYPES: [&str; 4] = ["lp", "bp", "hp", "notch"];
/// the effect rack in its default order: (short name used in the parameter names, label)
pub const FX: [(&str, &str); 12] = [("dist", "distortion"), ("flt", "filter"), ("flg", "flanger"), ("phs", "phaser"), ("hyp", "hyper"), ("cho", "chorus"), ("ott", "multiband"), ("cmp", "compressor"), ("eq", "eq"), ("dly", "delay"), ("rev", "reverb"), ("img", "imager")];
pub const NFX: usize = FX.len();
/// virtual destinations, in front of the parameter destinations
pub const VDEST: [&str; 5] = ["pitch", "a pitch", "b pitch", "c pitch", "amp"];
pub const D_PITCH: usize = 0;
pub const D_APITCH: usize = 1;
pub const D_AMP: usize = 4;

#[derive(Clone, Copy, Default)]
pub struct OscIx {
    pub on: usize,
    pub table: usize,
    pub pos: usize,
    pub oct: usize,
    pub semi: usize,
    pub fine: usize,
    pub level: usize,
    pub pan: usize,
    pub phase: usize,
    pub rand: usize,
    pub uni: usize,
    pub det: usize,
    pub blend: usize,
    pub width: usize,
    pub curve: usize,
    pub wt: [usize; 2],
    pub wa: [usize; 2],
    pub src: usize,
    pub route: usize,
}
#[derive(Clone, Copy, Default)]
pub struct SubIx {
    pub on: usize,
    pub shape: usize,
    pub oct: usize,
    pub level: usize,
    pub pan: usize,
    pub route: usize,
}
#[derive(Clone, Copy, Default)]
pub struct NoiseIx {
    pub on: usize,
    pub kind: usize,
    pub level: usize,
    pub pan: usize,
    pub color: usize,
    pub track: usize,
    pub route: usize,
}
#[derive(Clone, Copy, Default)]
pub struct FiltIx {
    pub on: usize,
    pub kind: usize,
    pub cut: usize,
    pub res: usize,
    pub drive: usize,
    pub key: usize,
    pub fat: usize,
    pub var: usize,
    pub mix: usize,
}
#[derive(Clone, Copy, Default)]
pub struct EnvIx {
    pub a: usize,
    pub h: usize,
    pub d: usize,
    pub s: usize,
    pub r: usize,
    pub ac: usize,
    pub dc: usize,
    pub rc: usize,
}
#[derive(Clone, Copy, Default)]
pub struct LfoIx {
    pub shape: usize,
    pub rate: usize,
    pub sync: usize,
    pub div: usize,
    pub mode: usize,
    pub rise: usize,
    pub delay: usize,
    pub phase: usize,
    pub smooth: usize,
    pub pts: [usize; 8],
}
#[derive(Clone, Copy, Default)]
pub struct RndIx {
    pub rate: usize,
    pub sync: usize,
    pub div: usize,
    pub smooth: usize,
    pub mode: usize,
}
#[derive(Clone, Copy, Default)]
pub struct MatIx {
    pub src: usize,
    pub dst: usize,
    pub amt: usize,
    pub bi: usize,
    pub crv: usize,
    pub aux: usize,
}
#[derive(Clone, Copy, Default)]
pub struct FxIx {
    pub on: usize,
    pub mix: usize,
    pub pos: usize,
    /// the effect's own parameters, in the order `fx.rs` reads them
    pub p: [usize; 8],
}

pub struct Layout {
    pub specs: Vec<Spec>,
    pub index: HashMap<&'static str, usize>,
    pub osc: [OscIx; 3],
    pub sub: SubIx,
    pub noise: NoiseIx,
    pub filt: [FiltIx; 2],
    pub froute: usize,
    pub env: [EnvIx; 4],
    pub lfo: [LfoIx; 4],
    pub rnd: [RndIx; 2],
    pub voices: usize,
    pub vmode: usize,
    pub porta: usize,
    pub porta_mode: usize,
    pub porta_rate: usize,
    pub porta_curve: usize,
    pub bend_range: usize,
    pub bend: usize,
    pub wheel: usize,
    pub vel_amp: usize,
    pub level: usize,
    pub macros: [usize; 8],
    pub mat: [MatIx; NSLOT],
    pub fx: [FxIx; NFX],
    /// destination names (virtual first), the parameter each one moves (None = virtual) and whether it is global (the rack)
    pub dests: &'static [&'static str],
    pub dest_param: Vec<Option<usize>>,
    pub dest_global: Vec<bool>,
    /// parameter -> destination index (usize::MAX = not a destination)
    pub dst_of: Vec<usize>,
}

impl Layout {
    pub fn n(&self) -> usize {
        self.specs.len()
    }
    pub fn ndst(&self) -> usize {
        self.dests.len()
    }
}

fn leak(s: String) -> &'static str {
    Box::leak(s.into_boxed_str())
}

struct B {
    specs: Vec<Spec>,
    modable: Vec<(usize, bool)>,
}
impl B {
    fn p(&mut self, name: &str, def: f32) -> usize {
        self.specs.push(Spec { name: leak(name.to_string()), def, kind: PK::Cont, opts: &[] });
        self.specs.len() - 1
    }
    /// a continuous parameter that is also a destination
    fn m(&mut self, name: &str, def: f32) -> usize {
        let i = self.p(name, def);
        self.modable.push((i, false));
        i
    }
    /// a destination of the rack (global)
    fn g(&mut self, name: &str, def: f32) -> usize {
        let i = self.p(name, def);
        self.modable.push((i, true));
        i
    }
    fn s(&mut self, name: &str, opts: &'static [&'static str], def_idx: usize) -> usize {
        let def = if opts.len() > 1 { def_idx as f32 / (opts.len() - 1) as f32 } else { 0.0 };
        self.specs.push(Spec { name: leak(name.to_string()), def, kind: PK::Sel, opts });
        self.specs.len() - 1
    }
    fn sw(&mut self, name: &str, def: bool) -> usize {
        self.specs.push(Spec { name: leak(name.to_string()), def: if def { 1.0 } else { 0.0 }, kind: PK::Sw, opts: &[] });
        self.specs.len() - 1
    }
}

pub fn layout() -> &'static Layout {
    static L: OnceLock<Layout> = OnceLock::new();
    L.get_or_init(build)
}

fn build() -> Layout {
    let tables: &'static [&'static str] = Box::leak(tables::names_with_user().into_boxed_slice());
    let mut b = B { specs: vec![], modable: vec![] };
    let mut osc = [OscIx::default(); 3];
    for (k, o) in ["a", "b", "c"].iter().enumerate() {
        let x = &mut osc[k];
        x.on = b.sw(&format!("{o} on"), k == 0);
        x.table = b.s(&format!("{o} table"), tables, [0, 1, 4][k]);
        x.pos = b.m(&format!("{o} pos"), 0.0);
        x.oct = b.s(&format!("{o} oct"), &["-4", "-3", "-2", "-1", "0", "+1", "+2", "+3", "+4"], 4);
        x.semi = b.p(&format!("{o} semi"), 0.5);
        x.fine = b.m(&format!("{o} fine"), 0.5);
        x.level = b.m(&format!("{o} level"), 0.75);
        x.pan = b.m(&format!("{o} pan"), 0.5);
        x.phase = b.m(&format!("{o} phase"), 0.0);
        x.rand = b.p(&format!("{o} rand"), 1.0);
        x.uni = b.p(&format!("{o} uni"), 0.0);
        x.det = b.m(&format!("{o} det"), 0.25);
        x.blend = b.m(&format!("{o} blend"), 0.75);
        x.width = b.m(&format!("{o} width"), 0.8);
        x.curve = b.p(&format!("{o} curve"), 0.2);
        for w in 0..2 {
            x.wt[w] = b.s(&format!("{o} w{} type", w + 1), &WARPS, 0);
            x.wa[w] = b.m(&format!("{o} w{} amt", w + 1), 0.0);
        }
        x.src = b.s(&format!("{o} fm src"), &MOD_SRC, [1, 0, 0][k]);
        x.route = b.s(&format!("{o} route"), &ROUTES, 0);
    }
    let sub = SubIx {
        on: b.sw("sub on", false),
        shape: b.s("sub shape", &SUB_SHAPES, 0),
        oct: b.s("sub oct", &SUB_OCT, 1),
        level: b.m("sub level", 0.6),
        pan: b.m("sub pan", 0.5),
        route: b.s("sub route", &ROUTES, 3),
    };
    let noise = NoiseIx {
        on: b.sw("noise on", false),
        kind: b.s("noise type", &NOISES, 0),
        level: b.m("noise level", 0.3),
        pan: b.m("noise pan", 0.5),
        color: b.m("noise color", 0.6),
        track: b.p("noise track", 0.0),
        route: b.s("noise route", &ROUTES, 0),
    };
    let mut filt = [FiltIx::default(); 2];
    for (k, f) in ["f1", "f2"].iter().enumerate() {
        filt[k] = FiltIx {
            on: b.sw(&format!("{f} on"), k == 0),
            kind: b.s(&format!("{f} type"), &FTYPES, 1),
            cut: b.m(&format!("{f} cut"), 1.0),
            res: b.m(&format!("{f} res"), 0.1),
            drive: b.m(&format!("{f} drive"), 0.0),
            key: b.p(&format!("{f} key"), 0.0),
            fat: b.m(&format!("{f} fat"), 0.0),
            var: b.m(&format!("{f} var"), 0.5),
            mix: b.m(&format!("{f} mix"), 1.0),
        };
    }
    let froute = b.s("f route", &FROUTE, 0);
    let mut env = [EnvIx::default(); 4];
    for k in 0..4 {
        let e = format!("e{}", k + 1);
        env[k] = EnvIx {
            a: b.m(&format!("{e} a"), 0.05),
            h: b.p(&format!("{e} h"), 0.0),
            d: b.m(&format!("{e} d"), 0.5),
            s: b.m(&format!("{e} s"), if k == 0 { 1.0 } else { 0.0 }),
            r: b.m(&format!("{e} r"), 0.35),
            ac: b.p(&format!("{e} ac"), 0.4),
            dc: b.p(&format!("{e} dc"), 0.75),
            rc: b.p(&format!("{e} rc"), 0.75),
        };
    }
    let mut lfo = [LfoIx::default(); 4];
    for k in 0..4 {
        let l = format!("l{}", k + 1);
        let mut x = LfoIx {
            shape: b.s(&format!("{l} shape"), &LFO_SHAPES, 0),
            rate: b.m(&format!("{l} rate"), 0.5),
            sync: b.sw(&format!("{l} sync"), true),
            div: b.s(&format!("{l} div"), &DIVS, 10),
            mode: b.s(&format!("{l} mode"), &LFO_MODES, 0),
            rise: b.p(&format!("{l} rise"), 0.0),
            delay: b.p(&format!("{l} delay"), 0.0),
            phase: b.p(&format!("{l} phase"), 0.0),
            smooth: b.p(&format!("{l} smooth"), 0.5),
            pts: [0; 8],
        };
        // the custom shape's points start as a triangle
        const TRI: [f32; 8] = [0.0, 0.25, 0.5, 0.75, 1.0, 0.75, 0.5, 0.25];
        for (j, p) in x.pts.iter_mut().enumerate() {
            *p = b.p(&format!("{l} p{}", j + 1), TRI[j]);
        }
        lfo[k] = x;
    }
    let mut rnd = [RndIx::default(); 2];
    for k in 0..2 {
        let r = format!("r{}", k + 1);
        rnd[k] = RndIx {
            rate: b.m(&format!("{r} rate"), 0.5),
            sync: b.sw(&format!("{r} sync"), true),
            div: b.s(&format!("{r} div"), &DIVS, 13),
            smooth: b.p(&format!("{r} smooth"), 0.2),
            mode: b.s(&format!("{r} mode"), &RND_MODES, 0),
        };
    }
    let voices = b.p("voices", 7.0 / 15.0);
    let vmode = b.s("voice mode", &VOICE_MODES, 0);
    let porta = b.m("porta", 0.0);
    let porta_mode = b.s("porta mode", &PORTA_MODES, 0);
    let porta_rate = b.sw("porta rate", false);
    let porta_curve = b.p("porta curve", 0.5);
    let bend_range = b.p("bend range", 2.0 / 48.0);
    let bend = b.p("bend", 0.5);
    let wheel = b.p("wheel", 0.0);
    let vel_amp = b.p("vel amp", 0.5);
    let level = b.p("level", 0.7);
    let mut macros = [0usize; 8];
    for (k, m) in macros.iter_mut().enumerate() {
        *m = b.p(&format!("m{}", k + 1), 0.0);
    }
    // the rack: on, mix, position, then the effect's own
    let mut fx = [FxIx::default(); NFX];
    let fx_params: [&[(&str, f32)]; NFX] = [
        &[("mode", 0.0), ("drive", 0.3), ("bias", 0.0), ("freq", 0.6), ("q", 0.2), ("out", 0.5)],
        &[("cut", 0.7), ("res", 0.2), ("drive", 0.0)],
        &[("rate", 0.2), ("depth", 0.6), ("fb", 0.6), ("delay", 0.3)],
        &[("rate", 0.25), ("depth", 0.7), ("fb", 0.5), ("center", 0.45), ("stages", 0.5)],
        &[("voices", 0.5), ("detune", 0.4), ("rate", 0.3), ("spread", 0.8)],
        &[("rate", 0.25), ("depth", 0.5)],
        &[("depth", 0.5), ("time", 0.3), ("up", 0.6), ("down", 0.8), ("stages", 0.0), ("gain", 0.5), ("low", 0.5), ("high", 0.5)],
        &[("thresh", 0.6), ("ratio", 0.25), ("attack", 0.1), ("release", 0.2), ("gain", 0.55)],
        &[("low", 0.5), ("mid", 0.5), ("high", 0.5)],
        &[("time", 0.4), ("sync", 1.0), ("div", 10.0 / 17.0), ("fb", 0.35), ("pp", 1.0), ("lo", 0.15), ("hi", 0.7)],
        &[("size", 0.5), ("decay", 0.4), ("damp", 0.4), ("plate", 0.0), ("lo", 0.3)],
        &[("width", 0.5), ("mono", 0.35)],
    ];
    for (k, (pre, _)) in FX.iter().enumerate() {
        let on = b.sw(&format!("{pre} on"), false);
        let mix = b.g(&format!("{pre} mix"), 1.0);
        let pos = b.p(&format!("{pre} pos"), k as f32 / (NFX - 1) as f32);
        let mut p = [usize::MAX; 8];
        for (j, (n, d)) in fx_params[k].iter().enumerate() {
            let name = format!("{pre} {n}");
            p[j] = match (*pre, *n) {
                ("dist", "mode") => b.s(&name, &DIST_MODES, 0),
                ("dist", "freq") => b.g(&name, *d),
                ("flt", _) => b.g(&name, *d),
                ("dly", "sync") | ("dly", "pp") | ("rev", "plate") => b.sw(&name, *d > 0.5),
                ("dly", "div") => b.s(&name, &DIVS, 10),
                (_, "stages") => b.p(&name, *d),
                (_, "time") | (_, "lo") | (_, "high") | (_, "low") | (_, "mid") | (_, "mono") | (_, "out") | (_, "bias") | (_, "q") => b.p(&name, *d),
                _ => b.g(&name, *d),
            };
        }
        fx[k] = FxIx { on, mix, pos, p };
    }
    // the dist filter's type is a selector too
    let df = b.s("dist filt", &DIST_FILT, 0);
    fx[0].p[6] = df;
    let ft = b.s("flt type", &FX_FTYPES, 0);
    fx[1].p[3] = ft;
    // destinations
    let mut dnames: Vec<&'static str> = VDEST.to_vec();
    let mut dest_param: Vec<Option<usize>> = vec![None; VDEST.len()];
    let mut dest_global = vec![false; VDEST.len()];
    let mut dst_of = vec![usize::MAX; b.specs.len() + NSLOT * 6];
    for &(i, g) in &b.modable {
        dst_of[i] = dnames.len();
        dnames.push(b.specs[i].name);
        dest_param.push(Some(i));
        dest_global.push(g);
    }
    let dests: &'static [&'static str] = Box::leak(dnames.into_boxed_slice());
    let mut mat = [MatIx::default(); NSLOT];
    for (k, m) in mat.iter_mut().enumerate() {
        let x = format!("x{}", k + 1);
        m.src = b.s(&format!("{x} src"), &SOURCES, 0);
        m.dst = b.s(&format!("{x} dst"), dests, 0);
        m.amt = b.p(&format!("{x} amt"), 0.5);
        m.bi = b.sw(&format!("{x} bi"), false);
        m.crv = b.p(&format!("{x} crv"), 0.5);
        m.aux = b.s(&format!("{x} aux"), &SOURCES, 0);
    }
    dst_of.truncate(b.specs.len());
    let index = b.specs.iter().enumerate().map(|(i, s)| (s.name, i)).collect();
    Layout {
        specs: b.specs,
        index,
        osc,
        sub,
        noise,
        filt,
        froute,
        env,
        lfo,
        rnd,
        voices,
        vmode,
        porta,
        porta_mode,
        porta_rate,
        porta_curve,
        bend_range,
        bend,
        wheel,
        vel_amp,
        level,
        macros,
        mat,
        fx,
        dests,
        dest_param,
        dest_global,
        dst_of,
    }
}

/// a selector's index from its 0..1 value
#[inline]
pub fn sel(v: f32, n: usize) -> usize {
    ((v * (n.max(2) - 1) as f32).round().max(0.0) as usize).min(n.max(1) - 1)
}
