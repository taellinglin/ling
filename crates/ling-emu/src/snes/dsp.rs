//! S-DSP — the SNES sound chip: 8 BRR voices with Gaussian interpolation,
//! ADSR/GAIN envelopes, pitch modulation, noise, and the 8-tap FIR echo.
//!
//! A line-by-line port of the ares emulator's cycle-accurate 32-step pipeline
//! (ISC licence, ares/sfc/dsp/*.cpp). One call to [`Dsp::run_sample`] is one
//! 32 kHz output sample.

const COUNTER_RATE: [u16; 32] = [
    0, 2048, 1536, 1280, 1024, 768, 640, 512, 384, 320, 256, 192, 160, 128, 96, 80, 64, 48, 40, 32,
    24, 20, 16, 12, 10, 8, 6, 5, 4, 3, 2, 1,
];
const COUNTER_OFFSET: [u16; 32] = [
    0, 0, 1040, 536, 0, 1040, 536, 0, 1040, 536, 0, 1040, 536, 0, 1040, 536, 0, 1040, 536, 0, 1040,
    536, 0, 1040, 536, 0, 1040, 536, 0, 1040, 0, 0,
];

#[derive(Clone, Copy, PartialEq, Eq)]
enum Env {
    Release,
    Attack,
    Decay,
    Sustain,
}

#[derive(Clone, Copy)]
struct Voice {
    index: usize,
    volume: [i8; 2],
    pitch: u16, // 14 bit
    source: u8,
    adsr0: u8,
    adsr1: u8,
    gain: u8,
    envx: u8,
    keyon: bool,
    keyoff: bool,
    modulate: bool,
    noise: bool,
    echo: bool,
    buffer: [i16; 12],
    buffer_offset: usize,
    gaussian_offset: u16,
    brr_address: u16,
    brr_offset: u16,
    keyon_delay: u8,
    env_mode: Env,
    envelope: i32,
    // internal latches
    env_raw: i32,
    keylatch: bool,
    k_on: bool,
    k_off: bool,
    k_mod: bool,
    k_noise: bool,
    k_echo: bool,
    k_end: bool,
    k_looped: bool,
}

impl Voice {
    fn new(n: usize) -> Voice {
        Voice {
            index: n << 4,
            volume: [0; 2],
            pitch: 0,
            source: 0,
            adsr0: 0,
            adsr1: 0,
            gain: 0,
            envx: 0,
            keyon: false,
            keyoff: false,
            modulate: false,
            noise: false,
            echo: false,
            buffer: [0; 12],
            buffer_offset: 0,
            gaussian_offset: 0,
            brr_address: 0,
            brr_offset: 1,
            keyon_delay: 0,
            env_mode: Env::Release,
            envelope: 0,
            env_raw: 0,
            keylatch: false,
            k_on: false,
            k_off: false,
            k_mod: false,
            k_noise: false,
            k_echo: false,
            k_end: false,
            k_looped: false,
        }
    }
}

pub struct Dsp {
    pub regs: [u8; 128],
    voice: [Voice; 8],
    gauss: [i16; 512],
    counter: u16,
    sample_phase: bool,
    // main volume
    m_reset: bool,
    m_mute: bool,
    m_volume: [i8; 2],
    m_output: [i32; 2],
    // echo
    e_feedback: i8,
    e_volume: [i8; 2],
    e_fir: [i8; 8],
    e_history: [[i16; 8]; 2],
    e_page: u8,
    e_delay: u8,
    e_readonly: bool,
    e_input: [i32; 2],
    e_output: [i32; 2],
    e__page: u8,
    e__readonly: bool,
    e__address: u16,
    e__offset: u16,
    e__length: u16,
    e__hist: usize,
    // noise
    n_freq: u8,
    n_lfsr: u16,
    // brr
    b_bank: u8,
    b__bank: u8,
    b__source: u8,
    b__address: u16,
    b__next: u16,
    b__header: u8,
    b__byte: u8,
    // latch
    l_adsr0: u8,
    l_envx: u8,
    l_outx: u8,
    l_pitch: u16,
    l_output: i16,
    out: (i16, i16),
}

fn sclamp16(v: i32) -> i32 {
    v.clamp(-32768, 32767)
}

impl Dsp {
    pub fn new() -> Dsp {
        let mut d = Dsp {
            regs: [0; 128],
            voice: [
                Voice::new(0),
                Voice::new(1),
                Voice::new(2),
                Voice::new(3),
                Voice::new(4),
                Voice::new(5),
                Voice::new(6),
                Voice::new(7),
            ],
            gauss: [0; 512],
            counter: 0,
            sample_phase: true,
            m_reset: true,
            m_mute: true,
            m_volume: [0; 2],
            m_output: [0; 2],
            e_feedback: 0,
            e_volume: [0; 2],
            e_fir: [0; 8],
            e_history: [[0; 8]; 2],
            e_page: 0,
            e_delay: 0,
            e_readonly: true,
            e_input: [0; 2],
            e_output: [0; 2],
            e__page: 0,
            e__readonly: false,
            e__address: 0,
            e__offset: 0,
            e__length: 0,
            e__hist: 0,
            n_freq: 0,
            n_lfsr: 0x4000,
            b_bank: 0,
            b__bank: 0,
            b__source: 0,
            b__address: 0,
            b__next: 0,
            b__header: 0,
            b__byte: 0,
            l_adsr0: 0,
            l_envx: 0,
            l_outx: 0,
            l_pitch: 0,
            l_output: 0,
            out: (0, 0),
        };
        d.build_gaussian();
        d
    }

    fn build_gaussian(&mut self) {
        let mut table = [0f64; 512];
        for n in 0..512usize {
            let k = 0.5 + n as f64;
            let s = (std::f64::consts::PI * k * 1.280 / 1024.0).sin();
            let t = ((std::f64::consts::PI * k * 2.000 / 1023.0).cos() - 1.0) * 0.50;
            let u = ((std::f64::consts::PI * k * 4.000 / 1023.0).cos() - 1.0) * 0.08;
            let r = s * (t + u + 1.0) / k;
            table[511 - n] = r;
        }
        for phase in 0..128usize {
            let sum = table[phase] + table[phase + 256] + table[511 - phase] + table[255 - phase];
            let scale = 2048.0 / sum;
            self.gauss[phase] = (table[phase] * scale + 0.5) as i16;
            self.gauss[phase + 256] = (table[phase + 256] * scale + 0.5) as i16;
            self.gauss[511 - phase] = (table[511 - phase] * scale + 0.5) as i16;
            self.gauss[255 - phase] = (table[255 - phase] * scale + 0.5) as i16;
        }
    }

    pub fn read(&self, addr: u8) -> u8 {
        self.regs[(addr & 0x7F) as usize]
    }

    pub fn write(&mut self, addr: u8, data: u8) {
        let a = (addr & 0x7F) as usize;
        self.regs[a] = data;
        match a {
            0x0C => self.m_volume[0] = data as i8,
            0x1C => self.m_volume[1] = data as i8,
            0x2C => self.e_volume[0] = data as i8,
            0x3C => self.e_volume[1] = data as i8,
            0x4C => {
                for n in 0..8 {
                    let b = data >> n & 1 != 0;
                    self.voice[n].keyon = b;
                    self.voice[n].keylatch = b;
                }
            },
            0x5C => {
                for n in 0..8 {
                    self.voice[n].keyoff = data >> n & 1 != 0;
                }
            },
            0x6C => {
                self.n_freq = data & 0x1F;
                self.e_readonly = data & 0x20 != 0;
                self.m_mute = data & 0x40 != 0;
                self.m_reset = data & 0x80 != 0;
            },
            0x7C => {
                for n in 0..8 {
                    self.voice[n].k_end = false;
                }
                self.regs[0x7C] = 0;
            },
            0x0D => self.e_feedback = data as i8,
            0x2D => {
                for n in 0..8 {
                    self.voice[n].modulate = data >> n & 1 != 0;
                }
                self.voice[0].modulate = false;
            },
            0x3D => {
                for n in 0..8 {
                    self.voice[n].noise = data >> n & 1 != 0;
                }
            },
            0x4D => {
                for n in 0..8 {
                    self.voice[n].echo = data >> n & 1 != 0;
                }
            },
            0x5D => self.b_bank = data,
            0x6D => self.e_page = data,
            0x7D => self.e_delay = data & 15,
            _ => {},
        }
        let n = (a >> 4) & 7;
        match a & 15 {
            0x00 => self.voice[n].volume[0] = data as i8,
            0x01 => self.voice[n].volume[1] = data as i8,
            0x02 => self.voice[n].pitch = (self.voice[n].pitch & 0x3F00) | data as u16,
            0x03 => self.voice[n].pitch = (self.voice[n].pitch & 0x00FF) | ((data as u16 & 0x3F) << 8),
            0x04 => self.voice[n].source = data,
            0x05 => self.voice[n].adsr0 = data,
            0x06 => self.voice[n].adsr1 = data,
            0x07 => self.voice[n].gain = data,
            0x08 => self.l_envx = data,
            0x09 => self.l_outx = data,
            0x0F => self.e_fir[n] = data as i8,
            _ => {},
        }
    }

    // ── counters / envelopes ─────────────────────────────────────────────────
    fn counter_tick(&mut self) {
        if self.counter == 0 {
            self.counter = 30720;
        }
        self.counter -= 1;
    }

    fn counter_poll(&self, rate: u32) -> bool {
        if rate == 0 {
            return false;
        }
        (self.counter as u32 + COUNTER_OFFSET[rate as usize] as u32) % COUNTER_RATE[rate as usize] as u32 == 0
    }

    fn envelope_run(&mut self, vi: usize) {
        let mut envelope = self.voice[vi].envelope;
        if self.voice[vi].env_mode == Env::Release {
            envelope -= 8;
            if envelope < 0 {
                envelope = 0;
            }
            self.voice[vi].envelope = envelope;
            return;
        }
        let rate: u32;
        let mut env_data = self.voice[vi].adsr1 as i32;
        if self.l_adsr0 & 0x80 != 0 {
            if self.voice[vi].env_mode as u8 >= Env::Decay as u8 {
                envelope -= 1;
                envelope -= envelope >> 8;
                let mut r = (env_data & 0x1F) as u32;
                if self.voice[vi].env_mode == Env::Decay {
                    r = (((self.l_adsr0 >> 4) & 7) as u32) * 2 + 16;
                }
                rate = r;
            } else {
                rate = ((self.l_adsr0 & 15) as u32) * 2 + 1;
                envelope += if rate < 31 { 0x20 } else { 0x400 };
            }
        } else {
            env_data = self.voice[vi].gain as i32;
            let mode = env_data >> 5;
            if mode < 4 {
                envelope = env_data << 4;
                rate = 31;
            } else {
                rate = (env_data & 0x1F) as u32;
                if mode == 4 {
                    envelope -= 0x20;
                } else if mode < 6 {
                    envelope -= 1;
                    envelope -= envelope >> 8;
                } else {
                    envelope += 0x20;
                    if mode > 6 && (self.voice[vi].env_raw as u32) >= 0x600 {
                        envelope += 0x8 - 0x20;
                    }
                }
            }
        }
        if (envelope >> 8) == (env_data >> 5) && self.voice[vi].env_mode == Env::Decay {
            self.voice[vi].env_mode = Env::Sustain;
        }
        self.voice[vi].env_raw = envelope;
        if (envelope as u32) > 0x7FF {
            envelope = if envelope < 0 { 0 } else { 0x7FF };
            if self.voice[vi].env_mode == Env::Attack {
                self.voice[vi].env_mode = Env::Decay;
            }
        }
        if self.counter_poll(rate) {
            self.voice[vi].envelope = envelope;
        }
    }

    // ── BRR ──────────────────────────────────────────────────────────────────
    fn brr_decode(&mut self, vi: usize, ram: &[u8; 65536]) {
        let v = &mut self.voice[vi];
        let mut nybbles: i32 = ((self.b__byte as i32) << 8)
            | ram[(v.brr_address.wrapping_add(v.brr_offset).wrapping_add(1)) as usize] as i32;
        let filter = (self.b__header >> 2) & 3;
        let scale = self.b__header >> 4;
        for _ in 0..4 {
            let mut s: i32 = ((nybbles as i16) as i32) >> 12;
            nybbles <<= 4;
            if scale <= 12 {
                s <<= scale;
                s >>= 1;
            } else {
                s &= !0x7FF;
            }
            let mut off = v.buffer_offset as i32;
            off -= 1;
            if off < 0 {
                off = 11;
            }
            let p1 = v.buffer[off as usize] as i32;
            off -= 1;
            if off < 0 {
                off = 11;
            }
            let p2 = (v.buffer[off as usize] as i32) >> 1;
            match filter {
                1 => {
                    s += p1 >> 1;
                    s += (-p1) >> 5;
                },
                2 => {
                    s += p1;
                    s -= p2;
                    s += p2 >> 4;
                    s += (p1 * -3) >> 6;
                },
                3 => {
                    s += p1;
                    s -= p2;
                    s += (p1 * -13) >> 7;
                    s += (p2 * 3) >> 4;
                },
                _ => {},
            }
            s = sclamp16(s);
            s = ((s << 1) as i16) as i32;
            v.buffer[v.buffer_offset] = s as i16;
            v.buffer_offset += 1;
            if v.buffer_offset >= 12 {
                v.buffer_offset = 0;
            }
        }
    }

    fn gaussian_interpolate(&self, vi: usize) -> i32 {
        let v = &self.voice[vi];
        let offset = ((v.gaussian_offset >> 4) & 0xFF) as usize;
        let fwd = |k: usize| self.gauss[255 - offset + k] as i32;
        let rev = |k: usize| self.gauss[offset + k] as i32;
        let mut o = (v.buffer_offset + (v.gaussian_offset >> 12) as usize) % 12;
        let mut out;
        out = (fwd(0) * v.buffer[o] as i32) >> 11;
        o = (o + 1) % 12;
        out += (fwd(256) * v.buffer[o] as i32) >> 11;
        o = (o + 1) % 12;
        out += (rev(256) * v.buffer[o] as i32) >> 11;
        o = (o + 1) % 12;
        out = (out as i16) as i32;
        out += (rev(0) * v.buffer[o] as i32) >> 11;
        sclamp16(out) & !1
    }

    // ── voice pipeline stages ────────────────────────────────────────────────
    fn voice_output(&mut self, vi: usize, ch: usize) {
        let amp = (self.l_output as i32 * self.voice[vi].volume[ch] as i32) >> 7;
        self.m_output[ch] = sclamp16(self.m_output[ch] + amp);
        if self.voice[vi].k_echo {
            self.e_output[ch] = sclamp16(self.e_output[ch] + amp);
        }
    }

    fn voice1(&mut self, vi: usize) {
        self.b__address = ((self.b__bank as u16) << 8).wrapping_add((self.b__source as u16) << 2);
        self.b__source = self.voice[vi].source;
    }

    fn voice2(&mut self, vi: usize, ram: &[u8; 65536]) {
        let mut address = self.b__address;
        if self.voice[vi].keyon_delay == 0 {
            address = address.wrapping_add(2);
        }
        let lo = ram[address as usize] as u16;
        let hi = ram[address.wrapping_add(1) as usize] as u16;
        self.b__next = (hi << 8) | lo;
        self.l_adsr0 = self.voice[vi].adsr0;
        self.l_pitch = self.voice[vi].pitch & 0xFF;
    }

    fn voice3a(&mut self, vi: usize) {
        self.l_pitch |= self.voice[vi].pitch & !0xFF;
    }

    fn voice3b(&mut self, vi: usize, ram: &[u8; 65536]) {
        let v = &self.voice[vi];
        self.b__byte = ram[v.brr_address.wrapping_add(v.brr_offset) as usize];
        self.b__header = ram[v.brr_address as usize];
    }

    fn voice3c(&mut self, vi: usize) {
        if self.voice[vi].k_mod {
            let delta = ((self.l_output as i32 >> 5) * self.l_pitch as i32) >> 10;
            self.l_pitch = (self.l_pitch as i32 + delta) as u16 & 0x7FFF;
        }
        if self.voice[vi].keyon_delay != 0 {
            if self.voice[vi].keyon_delay == 5 {
                self.voice[vi].brr_address = self.b__next;
                self.voice[vi].brr_offset = 1;
                self.voice[vi].buffer_offset = 0;
                self.b__header = 0;
            }
            self.voice[vi].envelope = 0;
            self.voice[vi].env_raw = 0;
            self.voice[vi].gaussian_offset = 0;
            self.voice[vi].keyon_delay -= 1;
            if self.voice[vi].keyon_delay & 3 != 0 {
                self.voice[vi].gaussian_offset = 0x4000;
            }
            self.l_pitch = 0;
        }
        let mut output = self.gaussian_interpolate(vi);
        if self.voice[vi].k_noise {
            output = ((self.n_lfsr << 1) as i16) as i32;
        }
        self.l_output = ((output * self.voice[vi].envelope) >> 11 & !1) as i16;
        self.voice[vi].envx = (self.voice[vi].envelope >> 4) as u8;

        if self.m_reset || self.b__header & 3 == 1 {
            self.voice[vi].env_mode = Env::Release;
            self.voice[vi].envelope = 0;
        }
        if self.sample_phase {
            if self.voice[vi].k_off {
                self.voice[vi].env_mode = Env::Release;
            }
            if self.voice[vi].k_on {
                self.voice[vi].keyon_delay = 5;
                self.voice[vi].env_mode = Env::Attack;
            }
        }
        if self.voice[vi].keyon_delay == 0 {
            self.envelope_run(vi);
        }
    }

    fn voice4(&mut self, vi: usize, ram: &[u8; 65536]) {
        self.voice[vi].k_looped = false;
        if self.voice[vi].gaussian_offset >= 0x4000 {
            self.brr_decode(vi, ram);
            self.voice[vi].brr_offset += 2;
            if self.voice[vi].brr_offset >= 9 {
                self.voice[vi].brr_address = self.voice[vi].brr_address.wrapping_add(9);
                if self.b__header & 1 != 0 {
                    self.voice[vi].brr_address = self.b__next;
                    self.voice[vi].k_looped = true;
                }
                self.voice[vi].brr_offset = 1;
            }
        }
        let g = (self.voice[vi].gaussian_offset & 0x3FFF) as u32 + self.l_pitch as u32;
        self.voice[vi].gaussian_offset = g.min(0x7FFF) as u16;
        self.voice_output(vi, 0);
    }

    fn voice5(&mut self, vi: usize) {
        self.voice_output(vi, 1);
        self.voice[vi].k_end |= self.voice[vi].k_looped;
        if self.voice[vi].keyon_delay == 5 {
            self.voice[vi].k_end = false;
        }
    }

    fn voice6(&mut self, _vi: usize) {
        self.l_outx = (self.l_output >> 8) as u8;
    }

    fn voice7(&mut self, vi: usize) {
        let mut endx = 0u8;
        for n in 0..8 {
            if self.voice[n].k_end {
                endx |= 1 << n;
            }
        }
        self.regs[0x7C] = endx;
        self.l_envx = self.voice[vi].envx;
    }

    fn voice8(&mut self, vi: usize) {
        self.regs[self.voice[vi].index | 0x09] = self.l_outx;
    }

    fn voice9(&mut self, vi: usize) {
        self.regs[self.voice[vi].index | 0x08] = self.l_envx;
    }

    // ── echo ─────────────────────────────────────────────────────────────────
    fn fir(&self, ch: usize, index: usize) -> i32 {
        let s = self.e_history[ch][(self.e__hist + index + 1) & 7] as i32;
        (s * self.e_fir[index] as i32) >> 6
    }

    fn echo_output(&self, ch: usize) -> i32 {
        let main = (((self.m_output[ch] * self.m_volume[ch] as i32) >> 7) as i16) as i32;
        let echo = (((self.e_input[ch] * self.e_volume[ch] as i32) >> 7) as i16) as i32;
        sclamp16(main + echo)
    }

    fn echo_read(&mut self, ch: usize, ram: &[u8; 65536]) {
        let a = self.e__address.wrapping_add((ch * 2) as u16);
        let lo = ram[a as usize] as u16;
        let hi = ram[a.wrapping_add(1) as usize] as u16;
        let s = ((hi << 8) | lo) as i16 as i32;
        self.e_history[ch][self.e__hist] = (s >> 1) as i16;
    }

    fn echo_write(&mut self, ch: usize, ram: &mut [u8; 65536]) {
        if !self.e__readonly {
            let a = self.e__address.wrapping_add((ch * 2) as u16);
            let s = self.e_output[ch] as i16 as u16;
            ram[a as usize] = s as u8;
            ram[a.wrapping_add(1) as usize] = (s >> 8) as u8;
        }
        self.e_output[ch] = 0;
    }

    fn echo22(&mut self, ram: &[u8; 65536]) {
        self.e__hist = (self.e__hist + 1) & 7;
        self.e__address = ((self.e__page as u16) << 8).wrapping_add(self.e__offset);
        self.echo_read(0, ram);
        self.e_input[0] = self.fir(0, 0);
        self.e_input[1] = self.fir(1, 0);
    }

    fn echo23(&mut self, ram: &[u8; 65536]) {
        let l = self.fir(0, 1) + self.fir(0, 2);
        let r = self.fir(1, 1) + self.fir(1, 2);
        self.e_input[0] += l;
        self.e_input[1] += r;
        self.echo_read(1, ram);
    }

    fn echo24(&mut self) {
        let l = self.fir(0, 3) + self.fir(0, 4) + self.fir(0, 5);
        let r = self.fir(1, 3) + self.fir(1, 4) + self.fir(1, 5);
        self.e_input[0] += l;
        self.e_input[1] += r;
    }

    fn echo25(&mut self) {
        let mut l = self.e_input[0] + self.fir(0, 6);
        let mut r = self.e_input[1] + self.fir(1, 6);
        l = (l as i16) as i32;
        r = (r as i16) as i32;
        l += (self.fir(0, 7) as i16) as i32;
        r += (self.fir(1, 7) as i16) as i32;
        self.e_input[0] = sclamp16(l) & !1;
        self.e_input[1] = sclamp16(r) & !1;
    }

    fn echo26(&mut self) {
        self.m_output[0] = self.echo_output(0);
        let fb = self.e_feedback as i32;
        let l = self.e_output[0] + ((((self.e_input[0] * fb) >> 7) as i16) as i32);
        let r = self.e_output[1] + ((((self.e_input[1] * fb) >> 7) as i16) as i32);
        self.e_output[0] = sclamp16(l) & !1;
        self.e_output[1] = sclamp16(r) & !1;
    }

    fn echo27(&mut self) {
        let mut outl = self.m_output[0];
        let mut outr = self.echo_output(1);
        self.m_output[0] = 0;
        self.m_output[1] = 0;
        if self.m_mute {
            outl = 0;
            outr = 0;
        }
        self.out = (outl as i16, outr as i16);
    }

    fn echo28(&mut self) {
        self.e__readonly = self.e_readonly;
    }

    fn echo29(&mut self, ram: &mut [u8; 65536]) {
        self.e__page = self.e_page;
        if self.e__offset == 0 {
            self.e__length = (self.e_delay as u16) << 11;
        }
        self.e__offset = self.e__offset.wrapping_add(4);
        if self.e__offset >= self.e__length {
            self.e__offset = 0;
        }
        self.echo_write(0, ram);
        self.e__readonly = self.e_readonly;
    }

    fn echo30(&mut self, ram: &mut [u8; 65536]) {
        self.echo_write(1, ram);
    }

    // ── misc ─────────────────────────────────────────────────────────────────
    fn misc27(&mut self) {
        for v in self.voice.iter_mut() {
            v.k_mod = v.modulate;
        }
    }

    fn misc28(&mut self) {
        for v in self.voice.iter_mut() {
            v.k_noise = v.noise;
            v.k_echo = v.echo;
        }
        self.b__bank = self.b_bank;
    }

    fn misc29(&mut self) {
        self.sample_phase = !self.sample_phase;
        if self.sample_phase {
            for v in self.voice.iter_mut() {
                v.keylatch &= !v.k_on;
            }
        }
    }

    fn misc30(&mut self) {
        if self.sample_phase {
            for v in self.voice.iter_mut() {
                v.k_on = v.keylatch;
                v.k_off = v.keyoff;
            }
        }
        self.counter_tick();
        if self.counter_poll(self.n_freq as u32) {
            let feedback = (self.n_lfsr << 13) ^ (self.n_lfsr << 14);
            self.n_lfsr = (feedback & 0x4000) | (self.n_lfsr >> 1);
        }
    }

    /// One 32 kHz output sample (the full 32-step pipeline).
    pub fn run_sample(&mut self, ram: &mut [u8; 65536]) -> (i16, i16) {
        self.voice5(0);
        self.voice2(1, ram);
        self.voice6(0);
        self.voice3a(1);
        self.voice3b(1, ram);
        self.voice3c(1);
        self.voice7(0);
        self.voice4(1, ram);
        self.voice1(3);
        self.voice8(0);
        self.voice5(1);
        self.voice2(2, ram);
        self.voice9(0);
        self.voice6(1);
        self.voice3a(2);
        self.voice3b(2, ram);
        self.voice3c(2);
        self.voice7(1);
        self.voice4(2, ram);
        self.voice1(4);
        self.voice8(1);
        self.voice5(2);
        self.voice2(3, ram);
        self.voice9(1);
        self.voice6(2);
        self.voice3a(3);
        self.voice3b(3, ram);
        self.voice3c(3);
        self.voice7(2);
        self.voice4(3, ram);
        self.voice1(5);
        self.voice8(2);
        self.voice5(3);
        self.voice2(4, ram);
        self.voice9(2);
        self.voice6(3);
        self.voice3a(4);
        self.voice3b(4, ram);
        self.voice3c(4);
        self.voice7(3);
        self.voice4(4, ram);
        self.voice1(6);
        self.voice8(3);
        self.voice5(4);
        self.voice2(5, ram);
        self.voice9(3);
        self.voice6(4);
        self.voice3a(5);
        self.voice3b(5, ram);
        self.voice3c(5);
        self.voice7(4);
        self.voice4(5, ram);
        self.voice1(7);
        self.voice8(4);
        self.voice5(5);
        self.voice2(6, ram);
        self.voice9(4);
        self.voice6(5);
        self.voice3a(6);
        self.voice3b(6, ram);
        self.voice3c(6);
        self.voice1(0);
        self.voice7(5);
        self.voice4(6, ram);
        self.voice8(5);
        self.voice5(6);
        self.voice2(7, ram);
        self.voice9(5);
        self.voice6(6);
        self.voice3a(7);
        self.voice3b(7, ram);
        self.voice3c(7);
        self.voice1(1);
        self.voice7(6);
        self.voice4(7, ram);
        self.voice8(6);
        self.voice5(7);
        self.voice2(0, ram);
        self.voice3a(0);
        self.voice9(6);
        self.voice6(7);
        self.echo22(ram);
        self.voice7(7);
        self.echo23(ram);
        self.voice8(7);
        self.echo24();
        self.voice3b(0, ram);
        self.voice9(7);
        self.echo25();
        self.echo26();
        self.misc27();
        self.echo27();
        self.misc28();
        self.echo28();
        self.misc29();
        self.echo29(ram);
        self.misc30();
        self.voice3c(0);
        self.echo30(ram);
        self.voice4(0, ram);
        self.voice1(2);
        self.out
    }
}
