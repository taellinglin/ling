//! Physically-modeled drum synth ("Ling Membrane").
//!
//! * Membranes (kick, toms, snare head): circular-membrane modal synthesis —
//!   Bessel-zero mode ratios, each mode a damped resonator, excited by a stick
//!   impulse whose brightness follows `hardness`; tension modulation gives the
//!   characteristic pitch drop. `purity` fades the upper modes away for analog-style
//!   sine drums, `damp` mutes the shell, `sweep` sets the pitch-drop time.
//! * Snare wires: noise band-passed and driven by the head's motion (sympathetic);
//!   `analog` adds the two tuned oscillators of electronic snares, `swell` fades the
//!   wires in (brush sweeps).
//! * Cymbals / hats: inharmonic plate modes + noise, or (`analog`) the six detuned
//!   square oscillators of classic drum machines; `gate` cuts any piece like a gated reverb.
//! * Clap: several staggered noise bursts through a band filter.
//! * Every GM key has its own voicing (tom pitches, side stick, cowbell, conga, ...).
//! * Where and how it is played: `strike` (centre .. edge of the head), `shell` (body resonance), `wires` (noise level), `tail` (noise length),
//!   `human` (random pitch per hit), `veltune` (velocity raises pitch) and `dynamics` (velocity response).
//! * Every piece has its own insert rack (`dsp/insert.rs`): filter, drive, crusher, punch, squash, echo, room and width, the `fx ...` knobs.
//! * Character: `sub` (a sine at the head's fundamental that follows its pitch drop: 808 boom under an acoustic kick), `snap` (a tonal
//!   chirp transient, the modern kick / tight snare attack), `ring` (ring modulation of the head: clang, rimshot, industrial metal), `fold`
//!   (a wavefolder after the voice: hard techno kicks) and `vary` (every hit a little different in length, brightness and level).
//! * The kit bus (`bus ...` knobs, the same on every piece): `duck` (the kick pushes the rest of the kit down), `room` (one shared room),
//!   `glue` (bus compression), `smash` (parallel compression), `tape` (saturation with a soft top), `boom` and `air` (low / high shelves,
//!   0.5 = flat). All neutral = bypassed.
use crate::dsp::fx::{FilterMode, Svf};
use crate::dsp::insert::{Insert, InsertParams};
use crate::dsp::{Instrument, Rng, TAU};

/// Bessel-zero ratios of an ideal circular membrane (modes 01,11,21,02,31,12,41,22).
const MEMBRANE: [f32; 8] = [1.0, 1.593, 2.136, 2.296, 2.653, 2.918, 3.156, 3.500];
/// Inharmonic plate-ish ratios for metal.
const PLATE: [f32; 10] = [1.0, 1.4471, 1.6170, 1.9265, 2.5028, 2.6637, 3.1585, 3.6152, 4.2127, 5.0416];
/// The six square-wave oscillators of the classic analog hat circuit (Hz).
const SQUARES: [f32; 6] = [205.3, 304.4, 369.6, 522.7, 540.0, 800.0];

#[derive(Clone, Copy, PartialEq, Debug)]
pub enum Kind {
    Kick,
    Snare,
    Tom,
    ClosedHat,
    OpenHat,
    Crash,
    Ride,
    Clap,
}

/// What a particular key does with its piece: a voicing on top of the piece's parameters.
#[derive(Clone, Copy, PartialEq, Debug)]
pub enum Flavor {
    Normal,
    SideStick,
    Cowbell,
    Bell,
}

#[derive(Clone, Copy, Debug)]
pub struct Var {
    pub semis: f32,
    pub decay: f32,
    pub noise: f32,
    pub click: f32,
    pub level: f32,
    pub hard: f32,
    pub flavor: Flavor,
}
impl Var {
    const N: Var = Var { semis: 0.0, decay: 1.0, noise: 1.0, click: 1.0, level: 1.0, hard: 0.0, flavor: Flavor::Normal };
}

#[derive(Clone)]
struct Mode {
    x: f32,
    y: f32,
    ratio: f32,
    amp: f32,
    decay: f32, // T60 seconds
    r: f32,
    c: f32,
    s: f32,
}

#[derive(Clone)]
pub struct DrumParams {
    pub tune: f32,     // 0..1 -> pitch scale
    pub decay: f32,    // 0..1 -> length scale
    pub tension: f32,  // 0..1 -> pitch-drop amount
    pub hardness: f32, // 0..1 -> exciter brightness
    pub body: f32,     // 0..1 -> balance shell vs noise/wires
    pub level: f32,
    pub click: f32,  // beater / stick transient
    pub tone: f32,   // brightness of the noise / cymbal filters
    pub damp: f32,   // shell damping (upper modes die faster)
    pub purity: f32, // 0 modal membrane .. 1 pure sine (808)
    pub sweep: f32,  // pitch-drop time
    pub drive: f32,  // saturation
    pub gate: f32,   // 0 off .. hard gate length (gated reverb / tight metal)
    pub analog: f32, // 0 physical .. 1 drum-machine oscillators
    pub swell: f32,  // noise fade-in (brush sweeps)
    pub pan: f32,    // 0..1, 0.5 centre
    pub strike: f32, // 0..1 where the stick lands: centre (dark, fundamental) .. edge (bright, upper modes), 0.5 neutral
    pub shell: f32,  // 0..1 resonance of the shell / body under the head
    pub wires: f32,  // 0..1 level of the noise part (snare wires, hat sizzle, clap bursts), 0.5 neutral
    pub tail: f32,   // 0..1 length of the noise tail against the membrane, 0.5 neutral
    pub human: f32,  // 0..1 random pitch change per hit (0.15 = the classic 1.5 %)
    pub veltune: f32, // 0..1 how much velocity raises the pitch
    pub dynamics: f32, // 0..1 velocity response: low = soft hits stay loud, high = wide dynamic range (0.5 neutral)
    pub sub: f32,    // 0..1 sine sub layer at the fundamental, following the pitch drop
    pub snap: f32,   // 0..1 tonal chirp transient
    pub ring: f32,   // 0..1 ring modulation of the head (clang)
    pub fold: f32,   // 0..1 wavefolder
    pub vary: f32,   // 0..1 random length / brightness / level per hit
}
impl DrumParams {
    fn new(level: f32) -> Self {
        DrumParams { tune: 0.5, decay: 0.5, tension: 0.5, hardness: 0.5, body: 0.5, level, click: 0.5, tone: 0.5, damp: 0.0, purity: 0.0, sweep: 0.5, drive: 0.0, gate: 0.0, analog: 0.0, swell: 0.0, pan: 0.5, strike: 0.5, shell: 0.0, wires: 0.5, tail: 0.5, human: 0.15, veltune: 0.0, dynamics: 0.5, sub: 0.0, snap: 0.0, ring: 0.0, fold: 0.0, vary: 0.0 }
    }
    fn list(&self) -> [(&'static str, f32); 28] {
        [
            ("tune", self.tune),
            ("decay", self.decay),
            ("tension", self.tension),
            ("hardness", self.hardness),
            ("body", self.body),
            ("level", self.level),
            ("click", self.click),
            ("tone", self.tone),
            ("damp", self.damp),
            ("purity", self.purity),
            ("sweep", self.sweep),
            ("drive", self.drive),
            ("gate", self.gate),
            ("analog", self.analog),
            ("swell", self.swell),
            ("pan", self.pan),
            ("strike", self.strike),
            ("shell", self.shell),
            ("wires", self.wires),
            ("tail", self.tail),
            ("human", self.human),
            ("veltune", self.veltune),
            ("dynamics", self.dynamics),
            ("sub", self.sub),
            ("snap", self.snap),
            ("ring", self.ring),
            ("fold", self.fold),
            ("vary", self.vary),
        ]
    }
    fn set(&mut self, n: &str, v: f32) {
        match n {
            "tune" => self.tune = v,
            "decay" => self.decay = v,
            "tension" => self.tension = v,
            "hardness" => self.hardness = v,
            "body" => self.body = v,
            "level" => self.level = v,
            "click" => self.click = v,
            "tone" => self.tone = v,
            "damp" => self.damp = v,
            "purity" => self.purity = v,
            "sweep" => self.sweep = v,
            "drive" => self.drive = v,
            "gate" => self.gate = v,
            "analog" => self.analog = v,
            "swell" => self.swell = v,
            "pan" => self.pan = v,
            "strike" => self.strike = v,
            "shell" => self.shell = v,
            "wires" => self.wires = v,
            "tail" => self.tail = v,
            "human" => self.human = v,
            "veltune" => self.veltune = v,
            "dynamics" => self.dynamics = v,
            "sub" => self.sub = v,
            "snap" => self.snap = v,
            "ring" => self.ring = v,
            "fold" => self.fold = v,
            "vary" => self.vary = v,
            _ => {}
        }
    }
}

struct DrumVoice {
    kind: Kind,
    var: Var,
    active: bool,
    modes: Vec<Mode>,
    t: f32,
    vel: f32,
    base: f32,
    pitch_drop: f32,
    pitch_tau: f32,
    exc_lp: f32,
    exc_a: f32,
    exc_left: u32,
    wires: Svf,
    wire_env: f32,
    wire_t60: f32,
    noise_hp: Svf,
    noise_bp: Svf,
    noise_env: f32,
    noise_t60: f32,
    swell_g: f32,
    swell_t: f32,
    sq_ph: [f32; 6],
    tone_ph: [f32; 2],
    tick: u32,
    pan: f32,
    piece: usize,
    gate_t: f32,
    hard: f32,
    pure_gain: f32,
    sub_ph: f32,
    sub_t60: f32,
    snap_ph: f32,
    ring_ph: f32,
    p: DrumParams,
}

impl DrumVoice {
    fn new(kind: Kind) -> Self {
        DrumVoice {
            kind,
            var: Var::N,
            active: false,
            modes: vec![],
            t: 0.0,
            vel: 0.0,
            base: 100.0,
            pitch_drop: 0.0,
            pitch_tau: 0.03,
            exc_lp: 0.0,
            exc_a: 0.5,
            exc_left: 0,
            wires: Svf::new(FilterMode::Band),
            wire_env: 0.0,
            wire_t60: 0.3,
            noise_hp: Svf::new(FilterMode::High),
            noise_bp: Svf::new(FilterMode::Band),
            noise_env: 0.0,
            noise_t60: 0.1,
            swell_g: 1.0,
            swell_t: 0.0,
            sq_ph: [0.0; 6],
            tone_ph: [0.0; 2],
            tick: 0,
            pan: 0.0,
            piece: 0,
            gate_t: 0.0,
            hard: 0.5,
            pure_gain: 1.0,
            sub_ph: 0.0,
            sub_t60: 0.5,
            snap_ph: 0.0,
            ring_ph: 0.0,
            p: DrumParams::new(0.9),
        }
    }

    fn trigger(&mut self, vel: f32, sr: f32, p: &DrumParams, var: Var, human: f32, jit: [f32; 3]) {
        // vary: every hit a little different in length, brightness and level
        let mut pv = p.clone();
        if p.vary > 0.0 {
            pv.decay = (pv.decay * (1.0 + p.vary * 0.3 * jit[0])).clamp(0.0, 1.0);
            pv.hardness = (pv.hardness + p.vary * 0.25 * jit[1]).clamp(0.0, 1.0);
            pv.level *= 1.0 + p.vary * 0.18 * jit[2];
        }
        let p = &pv;
        self.p = p.clone();
        self.var = var;
        self.sub_ph = 0.0;
        self.snap_ph = 0.0;
        self.ring_ph = 0.0;
        // the noise part follows `wires`, the velocity response follows `dynamics`
        self.var.noise *= 2f32.powf((p.wires - 0.5) * 2.0);
        let vel = vel.clamp(0.0, 1.0).powf(2f32.powf((p.dynamics - 0.5) * 2.0));
        self.vel = vel;
        self.t = 0.0;
        self.active = true;
        self.exc_lp = 0.0;
        self.tick = 0;
        self.sq_ph = [0.0; 6];
        self.tone_ph = [0.0; 2];
        // harder playing = brighter; the variant can push it further
        let hard = (p.hardness * 0.75 + vel * 0.25 + var.hard).clamp(0.0, 1.0);
        self.hard = hard;
        // stick impulse: harder = shorter/brighter (less lowpass smoothing)
        self.exc_a = 0.98 - hard * 0.93;
        self.exc_left = 2 + ((1.0 - hard) * 24.0) as u32;
        let tune = 2f32.powf((p.tune - 0.5) * 1.6) * 2f32.powf(var.semis / 12.0) * human * 2f32.powf((vel - 0.5) * p.veltune * 0.5);
        let tailk = 2f32.powf((p.tail - 0.5) * 2.0);
        let dscale = (0.35 + p.decay * 1.9) * var.decay;
        self.modes.clear();
        let (n_modes, base, decay0, drop, tau) = match self.kind {
            Kind::Kick => (5, 52.0 * tune, 0.45 * dscale, 2.4 * p.tension + 0.3, 0.035),
            Kind::Tom => (7, 120.0 * tune, 0.5 * dscale, 0.7 * p.tension + 0.1, 0.06),
            Kind::Snare => (8, 185.0 * tune, 0.16 * dscale, 0.35 * p.tension, 0.02),
            Kind::ClosedHat => (10, 320.0 * tune, 0.05 * dscale, 0.0, 0.01),
            Kind::OpenHat => (10, 320.0 * tune, 0.5 * dscale, 0.0, 0.01),
            Kind::Crash => (10, 280.0 * tune, 1.4 * dscale, 0.0, 0.01),
            Kind::Ride => (10, 400.0 * tune, 1.0 * dscale, 0.0, 0.01),
            Kind::Clap => (0, 1000.0, 0.0, 0.0, 0.01),
        };
        self.base = base;
        self.sub_t60 = (decay0 * 1.5).max(0.05);
        self.pitch_drop = drop;
        self.pitch_tau = tau * (0.2 + p.sweep * 1.6);
        let metal = matches!(self.kind, Kind::ClosedHat | Kind::OpenHat | Kind::Crash | Kind::Ride);
        // purity: fade the membrane's upper modes out for analog-style sine drums
        let keep = 1.0 - p.purity;
        self.pure_gain = 1.0 + p.purity * 1.3;
        for i in 0..n_modes {
            let ratio = if metal { PLATE[i] * 2.0 } else { MEMBRANE[i] };
            // higher modes die faster; shell partials on kick fade quickest; damp shortens them further
            let mut amp = if metal { 0.6 / (1.0 + i as f32 * 0.25) } else { 1.0 / (1.0 + i as f32 * 0.9) };
            // where the stick lands: the edge excites the upper modes, the centre mostly the fundamental
            amp *= (1.0 + (p.strike - 0.5) * 1.2 * i as f32).max(0.05);
            if !metal && i > 0 {
                amp *= keep;
            }
            let decay = decay0 / (1.0 + i as f32 * if metal { 0.15 } else { 0.7 + p.damp * 2.5 });
            self.modes.push(Mode { x: 0.0, y: 0.0, ratio, amp, decay, r: 0.0, c: 1.0, s: 0.0 });
        }
        // the shell: a resonance below the head's fundamental that rings a little longer
        if p.shell > 0.0 && !metal && n_modes > 0 {
            self.modes.push(Mode { x: 0.0, y: 0.0, ratio: 0.62, amp: p.shell * 0.9, decay: decay0 * 1.3, r: 0.0, c: 1.0, s: 0.0 });
        }
        self.update_coeffs(sr, 0.0);
        // wire / noise tails are T60 times (time to -60 dB)
        self.wire_env = 1.0;
        self.wire_t60 = (0.10 + 0.55 * p.decay) * var.decay * tailk;
        self.noise_env = 1.0;
        self.noise_t60 = match self.kind {
            Kind::ClosedHat => 0.07 * dscale,
            Kind::OpenHat => 0.45 * dscale,
            Kind::Crash => 1.4 * dscale,
            Kind::Ride => 1.0 * dscale,
            Kind::Clap => 0.14 + 0.2 * p.decay,
            _ => 0.05,
        } * tailk;
        self.swell_t = p.swell * p.swell * 0.12;
        self.swell_g = if self.swell_t > 0.0 { 0.0 } else { 1.0 };
        // gate: seconds until the piece is cut
        self.gate_t = if p.gate > 0.001 { 0.03 + p.gate * 0.4 } else { 0.0 };
        self.pan = (p.pan * 2.0 - 1.0).clamp(-1.0, 1.0);
        self.wires = Svf::new(FilterMode::Band);
        self.noise_hp = Svf::new(FilterMode::High);
        self.noise_bp = Svf::new(FilterMode::Band);
    }

    fn update_coeffs(&mut self, sr: f32, t: f32) {
        let bend = 1.0 + self.pitch_drop * (-t / self.pitch_tau).exp();
        for m in self.modes.iter_mut() {
            let w = TAU * (self.base * m.ratio * bend).min(sr * 0.45) / sr;
            m.c = w.cos();
            m.s = w.sin();
            m.r = (-6.9 / (m.decay.max(0.005) * sr)).exp();
        }
    }

    #[inline]
    fn sample(&mut self, sr: f32, rng: &mut Rng) -> f32 {
        if !self.active {
            return 0.0;
        }
        let dt = 1.0 / sr;
        let p = &self.p;
        let var = self.var;
        // exciter: short noise/impulse burst, one-pole smoothed by hardness
        let mut exc = 0.0;
        if self.exc_left > 0 {
            self.exc_left -= 1;
            let raw = if matches!(self.kind, Kind::Kick | Kind::Tom | Kind::Snare) { 1.0 } else { rng.next() };
            self.exc_lp += (raw - self.exc_lp) * (1.0 - self.exc_a);
            exc = self.exc_lp;
        } else {
            self.exc_lp *= 0.9;
        }
        self.tick += 1;
        if self.tick % 16 == 0 && self.pitch_drop > 0.0 {
            let t = self.t;
            self.update_coeffs(sr, t);
        }
        let mut head = 0.0;
        for m in self.modes.iter_mut() {
            let nx = m.r * (m.c * m.x - m.s * m.y) + exc * m.amp * 0.02;
            let ny = m.r * (m.s * m.x + m.c * m.y);
            m.x = nx;
            m.y = ny;
            head += ny * m.amp * 8.0;
        }
        head *= self.pure_gain;
        let membrane = matches!(self.kind, Kind::Kick | Kind::Tom | Kind::Snare);
        let bend = if self.pitch_drop > 0.0 { 1.0 + self.pitch_drop * (-self.t / self.pitch_tau).exp() } else { 1.0 };
        // ring: the head times a sine at an inharmonic ratio of its pitch (sidebands = clang)
        if self.p.ring > 0.0 && self.kind != Kind::Clap {
            self.ring_ph = (self.ring_ph + self.base * 1.47 * bend * dt).fract();
            head *= 1.0 - self.p.ring + self.p.ring * 1.6 * (self.ring_ph * TAU).sin();
        }
        // sub: a pure sine at the fundamental with the head's pitch drop, ringing a little longer than the head
        let mut extra = 0.0;
        if self.p.sub > 0.0 && membrane {
            self.sub_ph = (self.sub_ph + self.base * bend * dt).fract();
            let env = (-self.t * 6.9 / self.sub_t60).exp() * (self.t / 0.002).min(1.0);
            extra += (self.sub_ph * TAU).sin() * env * self.p.sub * 1.1;
        }
        // snap: a sine chirp from far above the pitch down onto it in a few milliseconds
        if self.p.snap > 0.0 && self.t < 0.03 {
            let f = self.base * (1.0 + 40.0 * (-self.t / 0.002).exp());
            self.snap_ph = (self.snap_ph + f.min(sr * 0.45) * dt).fract();
            extra += (self.snap_ph * TAU).sin() * (-self.t / 0.004).exp() * self.p.snap * 2.2;
        }
        let p = &self.p;
        let (body_w, noise_w) = (p.body, 1.0 - p.body);
        let tone_k = 0.6 + p.tone * 0.8;
        // noise fade-in (brush sweeps)
        if self.swell_g < 1.0 {
            self.swell_g = (self.swell_g + dt / self.swell_t.max(1e-4)).min(1.0);
        }
        let click_g = p.click * var.click;
        let out = extra + match self.kind {
            Kind::Kick | Kind::Tom => {
                let mut o = head * (0.6 + body_w * 0.6);
                // beater click
                let ct = 0.004;
                if self.t < ct {
                    o += rng.next() * 0.5 * click_g * (0.4 + self.hard) * (1.0 - self.t / ct);
                }
                o
            }
            Kind::Snare => {
                self.wire_env *= (-6.9 * dt / self.wire_t60.max(0.01)).exp();
                let drive = head.abs().min(1.0) * 0.6 + 0.4;
                let side = var.flavor == Flavor::SideStick;
                let w = self.wires.tick(rng.next() * drive, (4200.0 + self.hard * 3000.0) * tone_k, 0.35, sr);
                let mut o = head * (0.5 + body_w * 0.5) + w * self.wire_env * self.swell_g * (0.4 + noise_w * 1.2) * var.noise;
                // stick crack
                if self.t < 0.003 {
                    o += rng.next() * 0.55 * click_g * (1.0 - self.t / 0.003);
                }
                if side {
                    // side stick: a short wooden tick, no wires
                    o = head * 0.6 + if self.t < 0.004 { rng.next() * 0.9 * click_g * (1.0 - self.t / 0.004) } else { 0.0 };
                }
                // analog (drum machine) snare: two tuned oscillators under the noise
                if p.analog > 0.0 {
                    let e1 = (-self.t * 6.9 / 0.11).exp();
                    let e2 = (-self.t * 6.9 / 0.07).exp();
                    self.tone_ph[0] = (self.tone_ph[0] + self.base * dt * (1.0 + 0.6 * (-self.t * 60.0).exp())).fract();
                    self.tone_ph[1] = (self.tone_ph[1] + self.base * 1.83 * dt).fract();
                    let t1 = (self.tone_ph[0] * TAU).sin() * e1 + (self.tone_ph[1] * TAU).sin() * e2 * 0.6;
                    o = o * (1.0 - p.analog * 0.35) + t1 * p.analog * 0.9;
                }
                o
            }
            Kind::ClosedHat | Kind::OpenHat | Kind::Crash | Kind::Ride => {
                self.noise_env *= (-6.9 * dt / self.noise_t60.max(0.01)).exp();
                let n = self.noise_hp.tick(rng.next(), (6000.0 + self.hard * 3000.0) * tone_k, 0.2, sr);
                let plate = head * 0.9 * body_w.max(0.3) + n * (1.0 + noise_w * 1.4) * var.noise * self.swell_g;
                let mut o = plate;
                let tk = 2f32.powf((p.tune - 0.5) * 1.6) * 2f32.powf(var.semis / 12.0);
                if var.flavor == Flavor::Cowbell {
                    // two square waves through a band filter (808 cowbell)
                    self.sq_ph[0] = (self.sq_ph[0] + 540.0 * tk * dt).fract();
                    self.sq_ph[1] = (self.sq_ph[1] + 800.0 * tk * dt).fract();
                    let s = (if self.sq_ph[0] < 0.5 { 1.0 } else { -1.0 }) + (if self.sq_ph[1] < 0.5 { 1.0 } else { -1.0 });
                    let f = self.noise_bp.tick(s * 0.5, 1500.0 * tk * tone_k, 0.55, sr);
                    let fast = (-self.t * 6.9 / 0.04).exp();
                    o = f * (0.35 + 0.65 * fast) * 2.4 + head * 0.15;
                } else if p.analog > 0.0 {
                    let mut s = 0.0;
                    for (i, f) in SQUARES.iter().enumerate() {
                        self.sq_ph[i] = (self.sq_ph[i] + f * tk * 2.0 * dt).fract();
                        s += if self.sq_ph[i] < 0.5 { 1.0 } else { -1.0 };
                    }
                    let a = self.noise_hp.tick(s * 0.16, 7000.0 * tone_k, 0.3, sr) * 1.5;
                    o = plate * (1.0 - p.analog) + a * p.analog * 1.6 * self.swell_g;
                }
                if var.flavor == Flavor::Bell {
                    o = head * 1.6 + n * 0.25;
                }
                o * self.noise_env
            }
            Kind::Clap => {
                // three staggered bursts then a tail
                let t = self.t;
                let burst = |t0: f32| if t >= t0 && t < t0 + 0.012 { 1.0 - (t - t0) / 0.012 } else { 0.0 };
                let tail = (-((t - 0.03).max(0.0)) * 6.9 / self.noise_t60.max(0.05)).exp() * if t > 0.03 { 0.6 } else { 0.0 };
                let env = burst(0.0).max(burst(0.011)).max(burst(0.023)) * 0.8 + tail;
                self.wires.tick(rng.next(), (1100.0 + p.tune * 1800.0) * tone_k, 0.5, sr) * env * 2.0 * self.swell_g
            }
        };
        self.t += dt;
        // a hard gate (gated-reverb snares, tight metal kicks)
        let mut gate_g = 1.0;
        if self.gate_t > 0.0 {
            if self.t > self.gate_t + 0.015 {
                self.active = false;
                return 0.0;
            }
            if self.t > self.gate_t {
                gate_g = 1.0 - (self.t - self.gate_t) / 0.015;
            }
        }
        let limit = match self.kind {
            Kind::Crash => 4.0,
            Kind::Ride | Kind::OpenHat => 2.5,
            _ => 1.5,
        };
        if self.t > limit || (self.t > 0.05 && out.abs() < 1e-5 && self.modes.iter().all(|m| m.x.abs() + m.y.abs() < 1e-5) && self.noise_env < 1e-3 && self.wire_env < 1e-3) {
            self.active = false;
        }
        let d = self.p.drive;
        let mut out = out;
        if self.p.fold > 0.0 {
            // sine wavefolder: louder hits fold over into brighter, harder shapes
            let k = 1.0 + self.p.fold * 5.0;
            out = out * (1.0 - self.p.fold) + (out * k * 1.3).sin() * self.p.fold * 0.85;
        }
        let y = out * self.vel * self.p.level * var.level * gate_g;
        if d > 0.0 {
            (y * (1.0 + d * 5.0)).tanh() / (1.0 + d * 1.5)
        } else {
            y.tanh()
        }
    }
}

/// Map GM drum keys to a piece + voicing.
pub fn variant_for(key: u8) -> Option<(Kind, Var)> {
    let n = Var::N;
    Some(match key {
        35 => (Kind::Kick, Var { semis: -1.0, decay: 0.85, level: 0.9, ..n }),
        36 => (Kind::Kick, n),
        37 => (Kind::Snare, Var { semis: 5.0, decay: 0.3, noise: 0.0, click: 2.0, level: 0.9, flavor: Flavor::SideStick, ..n }),
        38 => (Kind::Snare, n),
        40 => (Kind::Snare, Var { semis: 1.0, noise: 1.15, decay: 0.9, hard: 0.1, ..n }),
        39 => (Kind::Clap, n),
        41 | 43 | 45 | 47 | 48 | 50 => (Kind::Tom, Var { semis: (key as f32 - 45.0) * 1.3, ..n }),
        42 => (Kind::ClosedHat, n),
        44 => (Kind::ClosedHat, Var { decay: 0.6, level: 0.75, noise: 0.85, ..n }),
        46 => (Kind::OpenHat, n),
        49 => (Kind::Crash, n),
        57 => (Kind::Crash, Var { semis: 2.0, decay: 0.85, ..n }),
        55 => (Kind::Crash, Var { semis: 6.0, decay: 0.3, level: 0.9, ..n }),
        52 => (Kind::Crash, Var { semis: -1.0, decay: 0.55, noise: 1.4, hard: 0.15, ..n }),
        51 => (Kind::Ride, n),
        59 => (Kind::Ride, Var { semis: 1.0, decay: 0.9, ..n }),
        53 => (Kind::Ride, Var { semis: 5.0, decay: 0.5, flavor: Flavor::Bell, ..n }),
        54 => (Kind::ClosedHat, Var { semis: 4.0, decay: 1.8, noise: 1.3, hard: 0.1, ..n }), // tambourine
        69 => (Kind::ClosedHat, Var { semis: 6.0, decay: 0.8, noise: 1.2, level: 0.8, ..n }), // cabasa
        70 => (Kind::ClosedHat, Var { semis: 8.0, decay: 0.7, noise: 1.2, level: 0.7, ..n }), // maracas
        82 => (Kind::ClosedHat, Var { semis: 3.0, decay: 1.0, noise: 1.2, level: 0.8, ..n }), // shaker
        56 => (Kind::ClosedHat, Var { decay: 4.0, flavor: Flavor::Cowbell, level: 0.8, ..n }),
        58 => (Kind::OpenHat, Var { semis: -3.0, decay: 1.6, noise: 1.2, ..n }), // vibraslap
        60 => (Kind::Tom, Var { semis: 19.0, decay: 0.3, hard: 0.2, ..n }),
        61 => (Kind::Tom, Var { semis: 15.0, decay: 0.3, hard: 0.2, ..n }),
        62 => (Kind::Tom, Var { semis: 13.0, decay: 0.25, hard: 0.2, ..n }),
        63 => (Kind::Tom, Var { semis: 12.0, decay: 0.4, hard: 0.15, ..n }),
        64 => (Kind::Tom, Var { semis: 7.0, decay: 0.45, hard: 0.15, ..n }),
        65 => (Kind::Tom, Var { semis: 17.0, decay: 0.4, hard: 0.35, ..n }),
        66 => (Kind::Tom, Var { semis: 13.0, decay: 0.4, hard: 0.35, ..n }),
        67 => (Kind::ClosedHat, Var { semis: 5.0, decay: 3.0, flavor: Flavor::Cowbell, level: 0.7, ..n }),
        68 => (Kind::ClosedHat, Var { semis: 1.0, decay: 3.0, flavor: Flavor::Cowbell, level: 0.7, ..n }),
        71 | 72 => (Kind::OpenHat, Var { semis: 14.0, decay: 0.25, noise: 0.6, level: 0.6, ..n }),
        73 | 74 => (Kind::OpenHat, Var { semis: 4.0, decay: 0.3, noise: 1.1, level: 0.7, ..n }),
        75 => (Kind::Tom, Var { semis: 30.0, decay: 0.07, click: 1.5, hard: 0.4, level: 0.8, ..n }),
        76 | 77 => (Kind::Tom, Var { semis: 21.0, decay: 0.1, click: 1.5, hard: 0.3, level: 0.8, ..n }),
        78 | 79 => (Kind::Tom, Var { semis: 10.0, decay: 0.3, ..n }),
        80 | 81 => (Kind::Ride, Var { semis: 16.0, decay: 0.4, noise: 0.15, flavor: Flavor::Bell, level: 0.6, ..n }),
        _ => return None,
    })
}

pub struct DrumSynth {
    voices: Vec<DrumVoice>,
    pub params: [DrumParams; 8],
    sr: f32,
    rng: Rng,
    selected: usize,
    steal: usize,
    /// one insert-effect rack per piece (filter, drive, crush, punch, squash, echo, room, width)
    ins: Vec<Insert>,
    /// the kit bus: duck, room, glue, smash, tape, boom, air
    pub bus: BusParams,
    bs: BusState,
}

pub const BUS_NAMES: [&str; 7] = ["bus duck", "bus room", "bus glue", "bus smash", "bus tape", "bus boom", "bus air"];

/// The kit bus knobs, 0..1. boom and air are shelves with 0.5 = flat; the rest are off at 0.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct BusParams {
    pub duck: f32,
    pub room: f32,
    pub glue: f32,
    pub smash: f32,
    pub tape: f32,
    pub boom: f32,
    pub air: f32,
}
impl Default for BusParams {
    fn default() -> Self {
        BusParams { duck: 0.0, room: 0.0, glue: 0.0, smash: 0.0, tape: 0.0, boom: 0.5, air: 0.5 }
    }
}
impl BusParams {
    pub fn is_off(&self) -> bool {
        self.duck <= 0.0 && self.room <= 0.0 && self.glue <= 0.0 && self.smash <= 0.0 && self.tape <= 0.0 && (self.boom - 0.5).abs() < 0.005 && (self.air - 0.5).abs() < 0.005
    }
    pub fn list(&self) -> [(&'static str, f32); 7] {
        let v = [self.duck, self.room, self.glue, self.smash, self.tape, self.boom, self.air];
        std::array::from_fn(|i| (BUS_NAMES[i], v[i]))
    }
    /// false when `name` is not a bus knob
    pub fn set(&mut self, name: &str, v: f32) -> bool {
        let v = v.clamp(0.0, 1.0);
        match name {
            "bus duck" => self.duck = v,
            "bus room" => self.room = v,
            "bus glue" => self.glue = v,
            "bus smash" => self.smash = v,
            "bus tape" => self.tape = v,
            "bus boom" => self.boom = v,
            "bus air" => self.air = v,
            _ => return false,
        }
        true
    }
}

struct BusState {
    duck_env: f32,
    glue_env: f32,
    smash_env: f32,
    boom_lp: [f32; 2],
    air_lp: [f32; 2],
    tape_lp: [f32; 2],
    room: Insert,
    room_set: f32,
}
impl BusState {
    fn new(sr: f32) -> Self {
        BusState { duck_env: 0.0, glue_env: 0.0, smash_env: 0.0, boom_lp: [0.0; 2], air_lp: [0.0; 2], tape_lp: [0.0; 2], room: Insert::new(sr), room_set: -1.0 }
    }
}

#[inline]
fn db(x: f32) -> f32 {
    20.0 * x.max(1e-9).log10()
}
#[inline]
fn undb(d: f32) -> f32 {
    10f32.powf(d / 20.0)
}
/// one-pole low-pass coefficient for a cutoff
#[inline]
fn onepole(fc: f32, sr: f32) -> f32 {
    1.0 - (-TAU * fc / sr).exp()
}
#[inline]
fn follow(env: &mut f32, x: f32, att: f32, rel: f32, sr: f32) {
    let a = if x > *env { 1.0 - (-1.0 / (att * sr)).exp() } else { 1.0 - (-1.0 / (rel * sr)).exp() };
    *env += a * (x - *env);
}
/// gain (linear) of a compressor with threshold `thr_db` and `ratio` for the envelope `env`
#[inline]
fn comp_gain(env: f32, thr_db: f32, ratio: f32) -> f32 {
    let over = db(env) - thr_db;
    if over <= 0.0 { 1.0 } else { undb(-over * (1.0 - 1.0 / ratio)) }
}

impl DrumSynth {
    /// the kit bus on one frame: `kick` is the kick piece after its rack, `rest` everything else
    fn bus_frame(&mut self, kick: (f32, f32), rest: (f32, f32)) -> (f32, f32) {
        let b = self.bus;
        let sr = self.sr;
        let st = &mut self.bs;
        let (mut l, mut r) = (kick.0, kick.1);
        // duck: the kick's level pushes the rest of the kit down (the pump of a sidechained bus)
        let mut g = 1.0;
        if b.duck > 0.0 {
            follow(&mut st.duck_env, kick.0.abs().max(kick.1.abs()), 0.001, 0.14, sr);
            g = 1.0 - b.duck * (st.duck_env * 2.5).min(1.0) * 0.85;
        }
        l += rest.0 * g;
        r += rest.1 * g;
        // one shared room for the whole kit
        if b.room > 0.0 {
            if (st.room_set - b.room).abs() > 1e-4 {
                let mut ip = InsertParams::default();
                ip.verb = (b.room * 0.75).min(1.0);
                ip.size = 0.22 + b.room * 0.2;
                st.room.set_params(ip);
                st.room_set = b.room;
            }
            let (a, c) = st.room.process(l, r);
            l = a;
            r = c;
        }
        // glue: gentle, slow bus compression with make-up
        if b.glue > 0.0 {
            follow(&mut st.glue_env, l.abs().max(r.abs()), 0.01, 0.15, sr);
            let gg = comp_gain(st.glue_env, -8.0 - b.glue * 16.0, 1.5 + b.glue * 2.5) * undb(b.glue * 5.0);
            l *= gg;
            r *= gg;
        }
        // smash: a crushed copy mixed under the dry kit (parallel / New York compression)
        if b.smash > 0.0 {
            follow(&mut st.smash_env, l.abs().max(r.abs()), 0.0008, 0.06, sr);
            let gs = comp_gain(st.smash_env, -32.0, 12.0) * undb(18.0);
            let (dl, dr) = (l * (1.0 - 0.3 * b.smash), r * (1.0 - 0.3 * b.smash));
            l = dl + (l * gs).tanh() * b.smash * 0.55;
            r = dr + (r * gs).tanh() * b.smash * 0.55;
        }
        // tape: saturation with a softened top
        if b.tape > 0.0 {
            let k = 1.0 + b.tape * 3.0;
            let a = onepole(18000.0 - b.tape * 9000.0, sr);
            st.tape_lp[0] += a * ((l * k).tanh() / k.sqrt() - st.tape_lp[0]);
            st.tape_lp[1] += a * ((r * k).tanh() / k.sqrt() - st.tape_lp[1]);
            l = l * (1.0 - b.tape) + st.tape_lp[0] * b.tape * 1.25;
            r = r * (1.0 - b.tape) + st.tape_lp[1] * b.tape * 1.25;
        }
        // boom (low shelf at 80 Hz, +-9 dB) and air (high shelf above 8 kHz, +-8 dB)
        if (b.boom - 0.5).abs() >= 0.005 {
            let a = onepole(80.0, sr);
            let k = undb((b.boom - 0.5) * 18.0) - 1.0;
            st.boom_lp[0] += a * (l - st.boom_lp[0]);
            st.boom_lp[1] += a * (r - st.boom_lp[1]);
            l += st.boom_lp[0] * k;
            r += st.boom_lp[1] * k;
        }
        if (b.air - 0.5).abs() >= 0.005 {
            let a = onepole(8000.0, sr);
            let k = undb((b.air - 0.5) * 16.0) - 1.0;
            st.air_lp[0] += a * (l - st.air_lp[0]);
            st.air_lp[1] += a * (r - st.air_lp[1]);
            l += (l - st.air_lp[0]) * k;
            r += (r - st.air_lp[1]) * k;
        }
        (l, r)
    }
}

pub const KIND_NAMES: [&str; 8] = ["Kick", "Snare", "Tom", "Closed Hat", "Open Hat", "Crash", "Ride", "Clap"];
const KINDS: [Kind; 8] = [Kind::Kick, Kind::Snare, Kind::Tom, Kind::ClosedHat, Kind::OpenHat, Kind::Crash, Kind::Ride, Kind::Clap];

impl DrumSynth {
    pub fn new(sr: f32) -> Self {
        let mut params: [DrumParams; 8] = std::array::from_fn(|_| DrumParams::new(0.9));
        params[0].tune = 0.45;
        params[1].body = 0.55;
        params[3].hardness = 0.8;
        // a little stage width
        params[2].pan = 0.42;
        params[3].pan = 0.62;
        params[5].pan = 0.35;
        params[6].pan = 0.66;
        DrumSynth { voices: (0..24).map(|_| DrumVoice::new(Kind::Kick)).collect(), params, sr, rng: Rng::new(99), selected: 0, steal: 0, ins: (0..8).map(|_| Insert::new(sr)).collect(), bus: BusParams::default(), bs: BusState::new(sr) }
    }
    pub fn select(&mut self, i: usize) {
        self.selected = i.min(7);
    }
    pub fn selected(&self) -> usize {
        self.selected
    }
    pub fn trigger(&mut self, kind: Kind, vel: f32) {
        self.trigger_var(kind, vel, Var::N);
    }
    fn trigger_var(&mut self, kind: Kind, vel: f32, var: Var) {
        let idx = KINDS.iter().position(|k| *k == kind).unwrap();
        // choke: closed hat cuts open hat
        if kind == Kind::ClosedHat && var.flavor == Flavor::Normal {
            for v in self.voices.iter_mut().filter(|v| v.kind == Kind::OpenHat && v.var.flavor == Flavor::Normal) {
                v.active = false;
            }
        }
        let slot = match self.voices.iter().position(|v| !v.active) {
            Some(s) => s,
            None => {
                self.steal = (self.steal + 1) % self.voices.len();
                self.steal
            }
        };
        let p = self.params[idx].clone();
        let sr = self.sr;
        // every hit is a little different (+-1.5 % pitch)
        let human = 1.0 + (self.rng.next() * p.human * 0.1);
        // vary draws its own numbers only when it is on, so a kit without it plays exactly as before
        let jit = if p.vary > 0.0 { [self.rng.next(), self.rng.next(), self.rng.next()] } else { [0.0; 3] };
        let v = &mut self.voices[slot];
        v.kind = kind;
        v.piece = idx;
        v.trigger(vel, sr, &p, var, human, jit);
    }
}

impl Instrument for DrumSynth {
    fn note_on(&mut self, key: u8, vel: f32) {
        if let Some((k, var)) = variant_for(key) {
            self.trigger_var(k, vel, var);
        }
    }
    fn note_off(&mut self, _key: u8) {}
    fn all_off(&mut self) {
        for v in self.voices.iter_mut() {
            v.active = false;
        }
    }
    fn render(&mut self, out: &mut [f32]) {
        let sr = self.sr;
        for fr in out.chunks_exact_mut(2) {
            let (mut l, mut r) = (0.0, 0.0);
            let (mut pl, mut pr) = ([0.0f32; 8], [0.0f32; 8]);
            for v in self.voices.iter_mut() {
                if !v.active {
                    continue;
                }
                let s = v.sample(sr, &mut self.rng);
                let ang = (v.pan + 1.0) * std::f32::consts::FRAC_PI_4;
                pl[v.piece] += s * ang.cos();
                pr[v.piece] += s * ang.sin();
            }
            // each piece through its own insert rack, then the kit sum (through the bus when one of its knobs is moved)
            let bus_on = !self.bus.is_off();
            let mut kick = (0.0, 0.0);
            for k in 0..8 {
                let (a, b) = if self.ins[k].active() { self.ins[k].process(pl[k], pr[k]) } else { (pl[k], pr[k]) };
                if bus_on && k == 0 {
                    kick = (a, b);
                    continue;
                }
                l += a;
                r += b;
            }
            if bus_on {
                let (a, b) = self.bus_frame(kick, (l, r));
                l = a;
                r = b;
            }
            // equal-power pan: centre = 0.707 per side, scaled so centred material matches the old mono mix
            fr[0] += l * 0.7;
            fr[1] += r * 0.7;
        }
    }
    fn params(&self) -> Vec<(&'static str, f32)> {
        let mut v = self.params[self.selected].list().to_vec();
        v.extend(self.ins[self.selected].p.list());
        v.extend(self.bus.list());
        v
    }
    fn set_param(&mut self, n: &str, v: f32) {
        if !v.is_finite() {
            return;
        }
        let v = v.clamp(0.0, 1.0);
        if self.bus.set(n, v) {
            return;
        }
        if self.ins[self.selected].p.set(n, v) {
            return;
        }
        self.params[self.selected].set(n, v);
    }
    fn name(&self) -> &'static str {
        "Ling Membrane"
    }
    fn state(&self) -> Vec<(String, f32)> {
        let mut out = vec![];
        for (i, name) in KIND_NAMES.iter().enumerate() {
            let key = name.to_lowercase().replace(' ', "_");
            for (n, v) in self.params[i].list() {
                out.push((format!("{key}.{n}"), v));
            }
            // the insert rack: only the knobs that were moved, so old projects stay small
            let def = InsertParams::default().list();
            for (k, (n, v)) in self.ins[i].p.list().into_iter().enumerate() {
                if (v - def[k].1).abs() > 1e-6 {
                    out.push((format!("{key}.{n}"), v));
                }
            }
        }
        // the kit bus, always (so switching A / B or loading a preset puts every bus knob back)
        for (n, v) in self.bus.list() {
            out.push((format!("bus.{}", &n[4..]), v));
        }
        out
    }
    fn load_state(&mut self, st: &[(String, f32)]) {
        let keep = self.selected;
        for (k, v) in st {
            if let Some((part, n)) = k.split_once('.') {
                if part == "bus" {
                    self.bus.set(&format!("bus {n}"), *v);
                    continue;
                }
                if let Some(i) = KIND_NAMES.iter().position(|x| x.to_lowercase().replace(' ', "_") == part) {
                    self.selected = i;
                    self.set_param(n, *v);
                }
            }
        }
        self.selected = keep;
    }
    fn parts(&self) -> Vec<&'static str> {
        KIND_NAMES.to_vec()
    }
    fn part(&self) -> usize {
        self.selected
    }
    fn select_part(&mut self, i: usize) {
        self.select(i);
    }
}
