//! S-SMP — the SPC700 audio CPU with its 64 KB RAM, the real 64-byte boot ROM
//! (verified byte-for-byte against snes9x), three timers, four CPU↔APU ports,
//! and the S-DSP clocked once per 32 SMP cycles.

use super::dsp::Dsp;

pub const IPL: [u8; 64] = [
    0xCD, 0xEF, 0xBD, 0xE8, 0x00, 0xC6, 0x1D, 0xD0, 0xFC, 0x8F, 0xAA, 0xF4, 0x8F, 0xBB, 0xF5, 0x78,
    0xCC, 0xF4, 0xD0, 0xFB, 0x2F, 0x19, 0xEB, 0xF4, 0xD0, 0xFC, 0x7E, 0xF4, 0xD0, 0x0B, 0xE4, 0xF5,
    0xCB, 0xF4, 0xD7, 0x00, 0xFC, 0xD0, 0xF3, 0xAB, 0x01, 0x10, 0xEF, 0x7E, 0xF4, 0x10, 0xEB, 0xBA,
    0xF6, 0xDA, 0x00, 0xBA, 0xF4, 0xC4, 0xF4, 0xDD, 0x5D, 0xD0, 0xDB, 0x1F, 0x00, 0x00, 0xC0, 0xFF,
];

#[rustfmt::skip]
const CYCLES: [u8; 256] = [
    2,8,4,5,3,4,3,6,2,6,5,4,5,4,6,8,
    2,8,4,5,4,5,5,6,5,5,6,5,2,2,4,6,
    2,8,4,5,3,4,3,6,2,6,5,4,5,4,5,4,
    2,8,4,5,4,5,5,6,5,5,6,5,2,2,3,8,
    2,8,4,5,3,4,3,6,2,6,4,4,5,4,6,6,
    2,8,4,5,4,5,5,6,5,5,4,5,2,2,4,3,
    2,8,4,5,3,4,3,6,2,6,4,4,5,4,5,5,
    2,8,4,5,4,5,5,6,5,5,5,5,2,2,3,6,
    2,8,4,5,3,4,3,6,2,6,5,4,5,2,4,5,
    2,8,4,5,4,5,5,6,5,5,5,5,2,2,12,5,
    3,8,4,5,3,4,3,6,2,6,4,4,5,2,4,4,
    2,8,4,5,4,5,5,6,5,5,5,5,2,2,3,4,
    3,8,4,5,4,5,4,7,2,5,6,4,5,2,4,9,
    2,8,4,5,5,6,6,7,4,5,5,5,2,2,6,3,
    2,8,4,5,3,4,3,6,2,4,5,3,4,3,4,3,
    2,8,4,5,4,5,5,6,3,4,5,4,2,2,4,3,
];

pub struct Smp {
    pub ram: Box<[u8; 65536]>,
    pub dsp: Dsp,
    a: u8,
    x: u8,
    y: u8,
    sp: u8,
    pc: u16,
    n: bool,
    v: bool,
    p: bool,
    b: bool,
    h: bool,
    i: bool,
    z: bool,
    c: bool,
    ctrl: u8,
    dsp_addr: u8,
    pub cpu_in: [u8; 4],
    pub smp_out: [u8; 4],
    t_en: [bool; 3],
    t_target: [u8; 3],
    t_stage: [u8; 3],
    t_out: [u8; 3],
    clk: u64,
    stopped: bool,
    pub samples: Vec<(i16, i16)>,
}

impl Smp {
    pub fn new() -> Smp {
        let mut s = Smp {
            ram: Box::new([0; 65536]),
            dsp: Dsp::new(),
            a: 0,
            x: 0,
            y: 0,
            sp: 0xEF,
            pc: 0xFFC0,
            n: false,
            v: false,
            p: false,
            b: false,
            h: false,
            i: false,
            z: false,
            c: false,
            ctrl: 0x80,
            dsp_addr: 0,
            cpu_in: [0; 4],
            smp_out: [0; 4],
            t_en: [false; 3],
            t_target: [0; 3],
            t_stage: [0; 3],
            t_out: [0; 3],
            clk: 0,
            stopped: false,
            samples: Vec::new(),
        };
        s.pc = u16::from_le_bytes([IPL[62], IPL[63]]);
        s
    }

    // ── bus ──────────────────────────────────────────────────────────────────
    fn read(&mut self, addr: u16) -> u8 {
        match addr {
            0xF0 | 0xF1 => 0,
            0xF2 => self.dsp_addr,
            0xF3 => self.dsp.read(self.dsp_addr & 0x7F),
            0xF4..=0xF7 => self.cpu_in[(addr - 0xF4) as usize],
            0xFA..=0xFC => 0,
            0xFD..=0xFF => {
                let i = (addr - 0xFD) as usize;
                let v = self.t_out[i];
                self.t_out[i] = 0;
                v
            },
            0xFFC0..=0xFFFF if self.ctrl & 0x80 != 0 => IPL[(addr - 0xFFC0) as usize],
            _ => self.ram[addr as usize],
        }
    }

    fn write(&mut self, addr: u16, v: u8) {
        match addr {
            0xF0 => {},
            0xF1 => {
                for i in 0..3 {
                    let on = v & (1 << i) != 0;
                    if !self.t_en[i] && on {
                        self.t_stage[i] = 0;
                        self.t_out[i] = 0;
                    }
                    self.t_en[i] = on;
                }
                if v & 0x10 != 0 {
                    self.cpu_in[0] = 0;
                    self.cpu_in[1] = 0;
                }
                if v & 0x20 != 0 {
                    self.cpu_in[2] = 0;
                    self.cpu_in[3] = 0;
                }
                self.ctrl = v;
            },
            0xF2 => self.dsp_addr = v,
            0xF3 => {
                if self.dsp_addr < 0x80 {
                    self.dsp.write(self.dsp_addr, v);
                }
            },
            0xF4..=0xF7 => self.smp_out[(addr - 0xF4) as usize] = v,
            0xFA..=0xFC => self.t_target[(addr - 0xFA) as usize] = v,
            0xFD..=0xFF => {},
            _ => {},
        }
        // writes always land in RAM too (IPL/regs overlay only affects reads)
        self.ram[addr as usize] = v;
    }

    fn fetch(&mut self) -> u8 {
        let v = self.read(self.pc);
        self.pc = self.pc.wrapping_add(1);
        v
    }
    fn fetch16(&mut self) -> u16 {
        let lo = self.fetch() as u16;
        let hi = self.fetch() as u16;
        (hi << 8) | lo
    }
    fn read16(&mut self, a: u16) -> u16 {
        let lo = self.read(a) as u16;
        let hi = self.read(a.wrapping_add(1)) as u16;
        (hi << 8) | lo
    }
    fn dp(&self, off: u8) -> u16 {
        (if self.p { 0x100 } else { 0 }) | off as u16
    }
    fn dp_read16(&mut self, off: u8) -> u16 {
        let lo = self.read(self.dp(off)) as u16;
        let hi = self.read(self.dp(off.wrapping_add(1))) as u16;
        (hi << 8) | lo
    }
    fn push(&mut self, v: u8) {
        self.write(0x100 | self.sp as u16, v);
        self.sp = self.sp.wrapping_sub(1);
    }
    fn pop(&mut self) -> u8 {
        self.sp = self.sp.wrapping_add(1);
        self.read(0x100 | self.sp as u16)
    }
    fn push16(&mut self, v: u16) {
        self.push((v >> 8) as u8);
        self.push(v as u8);
    }
    fn pop16(&mut self) -> u16 {
        let lo = self.pop() as u16;
        let hi = self.pop() as u16;
        (hi << 8) | lo
    }

    fn psw(&self) -> u8 {
        (self.n as u8) << 7
            | (self.v as u8) << 6
            | (self.p as u8) << 5
            | (self.b as u8) << 4
            | (self.h as u8) << 3
            | (self.i as u8) << 2
            | (self.z as u8) << 1
            | self.c as u8
    }
    fn set_psw(&mut self, v: u8) {
        self.n = v & 0x80 != 0;
        self.v = v & 0x40 != 0;
        self.p = v & 0x20 != 0;
        self.b = v & 0x10 != 0;
        self.h = v & 0x08 != 0;
        self.i = v & 0x04 != 0;
        self.z = v & 0x02 != 0;
        self.c = v & 0x01 != 0;
    }
    fn nz(&mut self, v: u8) -> u8 {
        self.n = v & 0x80 != 0;
        self.z = v == 0;
        v
    }

    // ── ALU ──────────────────────────────────────────────────────────────────
    fn adc8(&mut self, a: u8, b: u8) -> u8 {
        let c = self.c as u32;
        let r = a as u32 + b as u32 + c;
        self.h = (a as u32 & 15) + (b as u32 & 15) + c > 15;
        self.v = (!(a ^ b) & (a ^ r as u8) & 0x80) != 0;
        self.c = r > 0xFF;
        self.nz(r as u8)
    }
    fn sbc8(&mut self, a: u8, b: u8) -> u8 {
        self.adc8(a, !b)
    }
    fn cmp8(&mut self, a: u8, b: u8) {
        let r = a as i32 - b as i32;
        self.c = r >= 0;
        self.nz(r as u8);
    }
    /// kind: 0 OR 1 AND 2 EOR 3 CMP 4 ADC 5 SBC → result (None for CMP)
    fn alu(&mut self, kind: u8, a: u8, b: u8) -> Option<u8> {
        Some(match kind {
            0 => self.nz(a | b),
            1 => self.nz(a & b),
            2 => self.nz(a ^ b),
            3 => {
                self.cmp8(a, b);
                return None;
            },
            4 => self.adc8(a, b),
            _ => self.sbc8(a, b),
        })
    }

    fn add16(&mut self, ya: u16, d: u16, carry: u32) -> u16 {
        let r = ya as u32 + d as u32 + carry;
        self.h = ((ya ^ d ^ r as u16) & 0x1000) != 0;
        self.v = (!(ya ^ d) & (ya ^ r as u16) & 0x8000) != 0;
        self.c = r > 0xFFFF;
        let r = r as u16;
        self.z = r == 0;
        self.n = r & 0x8000 != 0;
        r
    }

    fn asl(&mut self, v: u8) -> u8 {
        self.c = v & 0x80 != 0;
        self.nz(v << 1)
    }
    fn lsr(&mut self, v: u8) -> u8 {
        self.c = v & 1 != 0;
        self.nz(v >> 1)
    }
    fn rol(&mut self, v: u8) -> u8 {
        let c = self.c as u8;
        self.c = v & 0x80 != 0;
        self.nz((v << 1) | c)
    }
    fn ror(&mut self, v: u8) -> u8 {
        let c = (self.c as u8) << 7;
        self.c = v & 1 != 0;
        self.nz((v >> 1) | c)
    }

    // ── addressing ───────────────────────────────────────────────────────────
    fn a_dp(&mut self) -> u16 {
        let o = self.fetch();
        self.dp(o)
    }
    fn a_dpx(&mut self) -> u16 {
        let o = self.fetch().wrapping_add(self.x);
        self.dp(o)
    }
    fn a_dpy(&mut self) -> u16 {
        let o = self.fetch().wrapping_add(self.y);
        self.dp(o)
    }
    fn a_abs(&mut self) -> u16 {
        self.fetch16()
    }
    fn a_absx(&mut self) -> u16 {
        self.fetch16().wrapping_add(self.x as u16)
    }
    fn a_absy(&mut self) -> u16 {
        self.fetch16().wrapping_add(self.y as u16)
    }
    fn a_ix(&mut self) -> u16 {
        let o = self.fetch().wrapping_add(self.x);
        self.dp_read16(o)
    }
    fn a_iy(&mut self) -> u16 {
        let o = self.fetch();
        self.dp_read16(o).wrapping_add(self.y as u16)
    }

    fn branch(&mut self, cond: bool, extra: &mut u32) {
        let off = self.fetch() as i8;
        if cond {
            self.pc = self.pc.wrapping_add(off as u16);
            *extra += 2;
        }
    }

    /// Execute one instruction; returns SMP cycles consumed.
    fn exec(&mut self) -> u32 {
        let op = self.fetch();
        let mut extra = 0u32;
        match op {
            0x00 => {},
            0xEF | 0xFF => self.stopped = true,
            // ── ALU families: low nibble selects mode, high part selects op ──
            0x08 | 0x28 | 0x48 | 0x68 | 0x88 | 0xA8 => {
                let kind = op >> 5;
                let b = self.fetch();
                let a = self.a;
                if let Some(r) = self.alu(kind, a, b) {
                    self.a = r;
                }
            },
            0x06 | 0x26 | 0x46 | 0x66 | 0x86 | 0xA6 | 0x04 | 0x24 | 0x44 | 0x64 | 0x84 | 0xA4
            | 0x14 | 0x34 | 0x54 | 0x74 | 0x94 | 0xB4 | 0x05 | 0x25 | 0x45 | 0x65 | 0x85 | 0xA5
            | 0x15 | 0x35 | 0x55 | 0x75 | 0x95 | 0xB5 | 0x16 | 0x36 | 0x56 | 0x76 | 0x96 | 0xB6
            | 0x07 | 0x27 | 0x47 | 0x67 | 0x87 | 0xA7 | 0x17 | 0x37 | 0x57 | 0x77 | 0x97 | 0xB7 => {
                let kind = (op >> 5) as u8;
                let addr = match op & 0x1F {
                    0x06 => self.dp_x_ptr(),
                    0x04 => self.a_dp(),
                    0x14 => self.a_dpx(),
                    0x05 => self.a_abs(),
                    0x15 => self.a_absx(),
                    0x16 => self.a_absy(),
                    0x07 => self.a_ix(),
                    _ => self.a_iy(),
                };
                let b = self.read(addr);
                let a = self.a;
                if let Some(r) = self.alu(kind, a, b) {
                    self.a = r;
                }
            },
            // dp,dp  |  (X),(Y)  |  dp,#imm
            0x09 | 0x29 | 0x49 | 0x69 | 0x89 | 0xA9 => {
                let kind = op >> 5;
                let s = self.fetch();
                let d = self.fetch();
                let src = self.read(self.dp(s));
                let da = self.dp(d);
                let dst = self.read(da);
                if let Some(r) = self.alu(kind, dst, src) {
                    self.write(da, r);
                }
            },
            0x19 | 0x39 | 0x59 | 0x79 | 0x99 | 0xB9 => {
                let kind = (op >> 5) as u8;
                let src = self.read(self.dp(self.y));
                let da = self.dp(self.x);
                let dst = self.read(da);
                if let Some(r) = self.alu(kind, dst, src) {
                    self.write(da, r);
                }
            },
            0x18 | 0x38 | 0x58 | 0x78 | 0x98 | 0xB8 => {
                let kind = (op >> 5) as u8;
                let imm = self.fetch();
                let d = self.fetch();
                let da = self.dp(d);
                let dst = self.read(da);
                if let Some(r) = self.alu(kind, dst, imm) {
                    self.write(da, r);
                }
            },
            // ── MOV A, ... ───────────────────────────────────────────────────
            0xE8 => {
                let v = self.fetch();
                self.a = self.nz(v);
            },
            0xE6 | 0xE4 | 0xF4 | 0xE5 | 0xF5 | 0xF6 | 0xE7 | 0xF7 => {
                let addr = match op {
                    0xE6 => self.dp(self.x),
                    0xE4 => self.a_dp(),
                    0xF4 => self.a_dpx(),
                    0xE5 => self.a_abs(),
                    0xF5 => self.a_absx(),
                    0xF6 => self.a_absy(),
                    0xE7 => self.a_ix(),
                    _ => self.a_iy(),
                };
                let v = self.read(addr);
                self.a = self.nz(v);
            },
            0xBF => {
                let v = self.read(self.dp(self.x));
                self.x = self.x.wrapping_add(1);
                self.a = self.nz(v);
            },
            // ── MOV mem, A ───────────────────────────────────────────────────
            0xC4 | 0xD4 | 0xC5 | 0xD5 | 0xD6 | 0xC7 | 0xD7 | 0xC6 => {
                let addr = match op {
                    0xC4 => self.a_dp(),
                    0xD4 => self.a_dpx(),
                    0xC5 => self.a_abs(),
                    0xD5 => self.a_absx(),
                    0xD6 => self.a_absy(),
                    0xC7 => self.a_ix(),
                    0xD7 => self.a_iy(),
                    _ => self.dp(self.x),
                };
                let a = self.a;
                self.write(addr, a);
            },
            0xAF => {
                let a = self.a;
                let addr = self.dp(self.x);
                self.write(addr, a);
                self.x = self.x.wrapping_add(1);
            },
            // ── MOV X / Y ────────────────────────────────────────────────────
            0xCD => {
                let v = self.fetch();
                self.x = self.nz(v);
            },
            0xF8 | 0xF9 | 0xE9 => {
                let addr = match op {
                    0xF8 => self.a_dp(),
                    0xF9 => self.a_dpy(),
                    _ => self.a_abs(),
                };
                let v = self.read(addr);
                self.x = self.nz(v);
            },
            0x8D => {
                let v = self.fetch();
                self.y = self.nz(v);
            },
            0xEB | 0xFB | 0xEC => {
                let addr = match op {
                    0xEB => self.a_dp(),
                    0xFB => self.a_dpx(),
                    _ => self.a_abs(),
                };
                let v = self.read(addr);
                self.y = self.nz(v);
            },
            0xD8 | 0xD9 | 0xC9 => {
                let addr = match op {
                    0xD8 => self.a_dp(),
                    0xD9 => self.a_dpy(),
                    _ => self.a_abs(),
                };
                let x = self.x;
                self.write(addr, x);
            },
            0xCB | 0xDB | 0xCC => {
                let addr = match op {
                    0xCB => self.a_dp(),
                    0xDB => self.a_dpx(),
                    _ => self.a_abs(),
                };
                let y = self.y;
                self.write(addr, y);
            },
            0x8F => {
                let imm = self.fetch();
                let d = self.fetch();
                let da = self.dp(d);
                self.write(da, imm);
            },
            0xFA => {
                let s = self.fetch();
                let d = self.fetch();
                let v = self.read(self.dp(s));
                let da = self.dp(d);
                self.write(da, v);
            },
            0x7D => {
                self.a = self.nz(self.x);
            },
            0xDD => {
                self.a = self.nz(self.y);
            },
            0x5D => {
                self.x = self.nz(self.a);
            },
            0xFD => {
                self.y = self.nz(self.a);
            },
            0x9D => {
                self.x = self.nz(self.sp);
            },
            0xBD => self.sp = self.x,
            // ── CMP X / Y ────────────────────────────────────────────────────
            0xC8 | 0x3E | 0x1E => {
                let b = match op {
                    0xC8 => self.fetch(),
                    0x3E => {
                        let a = self.a_dp();
                        self.read(a)
                    },
                    _ => {
                        let a = self.a_abs();
                        self.read(a)
                    },
                };
                let x = self.x;
                self.cmp8(x, b);
            },
            0xAD | 0x7E | 0x5E => {
                let b = match op {
                    0xAD => self.fetch(),
                    0x7E => {
                        let a = self.a_dp();
                        self.read(a)
                    },
                    _ => {
                        let a = self.a_abs();
                        self.read(a)
                    },
                };
                let y = self.y;
                self.cmp8(y, b);
            },
            // ── INC / DEC ────────────────────────────────────────────────────
            0xBC => self.a = self.nz(self.a.wrapping_add(1)),
            0x3D => self.x = self.nz(self.x.wrapping_add(1)),
            0xFC => self.y = self.nz(self.y.wrapping_add(1)),
            0x9C => self.a = self.nz(self.a.wrapping_sub(1)),
            0x1D => self.x = self.nz(self.x.wrapping_sub(1)),
            0xDC => self.y = self.nz(self.y.wrapping_sub(1)),
            0xAB | 0xBB | 0xAC | 0x8B | 0x9B | 0x8C => {
                let addr = match op {
                    0xAB | 0x8B => self.a_dp(),
                    0xBB | 0x9B => self.a_dpx(),
                    _ => self.a_abs(),
                };
                let v = self.read(addr);
                let r = if matches!(op, 0xAB | 0xBB | 0xAC) {
                    v.wrapping_add(1)
                } else {
                    v.wrapping_sub(1)
                };
                let r = self.nz(r);
                self.write(addr, r);
            },
            // ── shifts ───────────────────────────────────────────────────────
            0x1C => self.a = self.asl(self.a),
            0x5C => self.a = self.lsr(self.a),
            0x3C => self.a = self.rol(self.a),
            0x7C => self.a = self.ror(self.a),
            0x0B | 0x1B | 0x0C | 0x4B | 0x5B | 0x4C | 0x2B | 0x3B | 0x2C | 0x6B | 0x7B | 0x6C => {
                let addr = match op {
                    0x0B | 0x4B | 0x2B | 0x6B => self.a_dp(),
                    0x0C | 0x4C | 0x2C | 0x6C => self.a_abs(),
                    _ => self.a_dpx(),
                };
                let v = self.read(addr);
                let r = match op >> 4 {
                    0x0 | 0x1 => self.asl(v),
                    0x4 | 0x5 => self.lsr(v),
                    0x2 | 0x3 => self.rol(v),
                    _ => self.ror(v),
                };
                self.write(addr, r);
            },
            // ── 16-bit ───────────────────────────────────────────────────────
            0xBA => {
                let o = self.fetch();
                let v = self.dp_read16(o);
                self.a = v as u8;
                self.y = (v >> 8) as u8;
                self.n = v & 0x8000 != 0;
                self.z = v == 0;
            },
            0xDA => {
                let o = self.fetch();
                let a = self.a;
                let y = self.y;
                let da = self.dp(o);
                let _ = self.read(da);
                self.write(da, a);
                let da2 = self.dp(o.wrapping_add(1));
                self.write(da2, y);
            },
            0x3A | 0x1A => {
                let o = self.fetch();
                let v = self.dp_read16(o);
                let r = if op == 0x3A { v.wrapping_add(1) } else { v.wrapping_sub(1) };
                self.write(self.dp(o), r as u8);
                self.write(self.dp(o.wrapping_add(1)), (r >> 8) as u8);
                self.n = r & 0x8000 != 0;
                self.z = r == 0;
            },
            0x7A => {
                let o = self.fetch();
                let d = self.dp_read16(o);
                let ya = ((self.y as u16) << 8) | self.a as u16;
                let r = self.add16(ya, d, 0);
                self.a = r as u8;
                self.y = (r >> 8) as u8;
            },
            0x9A => {
                let o = self.fetch();
                let d = self.dp_read16(o);
                let ya = ((self.y as u16) << 8) | self.a as u16;
                let r = self.add16(ya, !d, 1);
                self.a = r as u8;
                self.y = (r >> 8) as u8;
            },
            0x5A => {
                let o = self.fetch();
                let d = self.dp_read16(o);
                let ya = ((self.y as u16) << 8) | self.a as u16;
                let r = ya as i32 - d as i32;
                self.c = r >= 0;
                self.z = r as u16 == 0;
                self.n = (r as u16) & 0x8000 != 0;
            },
            0xCF => {
                let r = self.y as u16 * self.a as u16;
                self.a = r as u8;
                self.y = (r >> 8) as u8;
                self.nz(self.y);
            },
            0x9E => {
                let ya = ((self.y as u32) << 8) | self.a as u32;
                self.h = (self.y & 15) >= (self.x & 15);
                self.v = self.y >= self.x;
                let x = self.x as u32;
                if (self.y as u32) < (x << 1) {
                    self.a = (ya / x.max(1)) as u8;
                    self.y = (ya % x.max(1)) as u8;
                } else {
                    let t = ya - (x << 9);
                    let d = 256 - x;
                    self.a = (255 - t / d.max(1)) as u8;
                    self.y = (x + t % d.max(1)) as u8;
                }
                self.nz(self.a);
            },
            0x9F => {
                self.a = self.nz(self.a.rotate_left(4));
            },
            0xDF => {
                if self.c || self.a > 0x99 {
                    self.a = self.a.wrapping_add(0x60);
                    self.c = true;
                }
                if self.h || (self.a & 15) > 9 {
                    self.a = self.a.wrapping_add(6);
                }
                self.nz(self.a);
            },
            0xBE => {
                if !self.c || self.a > 0x99 {
                    self.a = self.a.wrapping_sub(0x60);
                    self.c = false;
                }
                if !self.h || (self.a & 15) > 9 {
                    self.a = self.a.wrapping_sub(6);
                }
                self.nz(self.a);
            },
            // ── stack ────────────────────────────────────────────────────────
            0x2D => self.push(self.a),
            0x4D => self.push(self.x),
            0x6D => self.push(self.y),
            0x0D => {
                let p = self.psw();
                self.push(p);
            },
            0xAE => {
                let v = self.pop();
                self.a = v;
            },
            0xCE => {
                let v = self.pop();
                self.x = v;
            },
            0xEE => {
                let v = self.pop();
                self.y = v;
            },
            0x8E => {
                let v = self.pop();
                self.set_psw(v);
            },
            // ── flags ────────────────────────────────────────────────────────
            0x60 => self.c = false,
            0x80 => self.c = true,
            0xED => self.c = !self.c,
            0xE0 => {
                self.v = false;
                self.h = false;
            },
            0x20 => self.p = false,
            0x40 => self.p = true,
            0xA0 => self.i = true,
            0xC0 => self.i = false,
            // ── branches ─────────────────────────────────────────────────────
            0x10 => self.branch(!self.n, &mut extra),
            0x30 => self.branch(self.n, &mut extra),
            0x50 => self.branch(!self.v, &mut extra),
            0x70 => self.branch(self.v, &mut extra),
            0x90 => self.branch(!self.c, &mut extra),
            0xB0 => self.branch(self.c, &mut extra),
            0xD0 => self.branch(!self.z, &mut extra),
            0xF0 => self.branch(self.z, &mut extra),
            0x2F => self.branch(true, &mut extra),
            0x03 | 0x23 | 0x43 | 0x63 | 0x83 | 0xA3 | 0xC3 | 0xE3 | 0x13 | 0x33 | 0x53 | 0x73
            | 0x93 | 0xB3 | 0xD3 | 0xF3 => {
                let bit = op >> 5;
                let set = op & 0x10 == 0;
                let a = self.a_dp();
                let v = self.read(a);
                let is_set = v & (1 << bit) != 0;
                self.branch(is_set == set, &mut extra);
            },
            0x02 | 0x22 | 0x42 | 0x62 | 0x82 | 0xA2 | 0xC2 | 0xE2 | 0x12 | 0x32 | 0x52 | 0x72
            | 0x92 | 0xB2 | 0xD2 | 0xF2 => {
                let bit = op >> 5;
                let a = self.a_dp();
                let v = self.read(a);
                let r = if op & 0x10 == 0 { v | (1 << bit) } else { v & !(1 << bit) };
                self.write(a, r);
            },
            0x2E => {
                let a = self.a_dp();
                let v = self.read(a);
                let c = self.a != v;
                self.branch(c, &mut extra);
            },
            0xDE => {
                let a = self.a_dpx();
                let v = self.read(a);
                let c = self.a != v;
                self.branch(c, &mut extra);
            },
            0x6E => {
                let a = self.a_dp();
                let v = self.read(a).wrapping_sub(1);
                self.write(a, v);
                self.branch(v != 0, &mut extra);
            },
            0xFE => {
                self.y = self.y.wrapping_sub(1);
                let c = self.y != 0;
                self.branch(c, &mut extra);
            },
            // ── jumps / calls ────────────────────────────────────────────────
            0x5F => self.pc = self.fetch16(),
            0x1F => {
                let a = self.a_absx();
                self.pc = self.read16(a);
            },
            0x3F => {
                let t = self.fetch16();
                self.push16(self.pc);
                self.pc = t;
            },
            0x4F => {
                let u = self.fetch();
                self.push16(self.pc);
                self.pc = 0xFF00 | u as u16;
            },
            0x6F => self.pc = self.pop16(),
            0x7F => {
                let p = self.pop();
                self.set_psw(p);
                self.pc = self.pop16();
            },
            0x0F => {
                self.push16(self.pc);
                let p = self.psw();
                self.push(p);
                self.b = true;
                self.i = false;
                self.pc = self.read16(0xFFDE);
            },
            0x01 | 0x11 | 0x21 | 0x31 | 0x41 | 0x51 | 0x61 | 0x71 | 0x81 | 0x91 | 0xA1 | 0xB1
            | 0xC1 | 0xD1 | 0xE1 | 0xF1 => {
                let n = (op >> 4) as u16;
                self.push16(self.pc);
                self.pc = self.read16(0xFFDE - n * 2);
            },
            // ── bit ops ──────────────────────────────────────────────────────
            0x0A | 0x2A | 0x4A | 0x6A | 0x8A | 0xAA | 0xCA | 0xEA => {
                let m = self.fetch16();
                let addr = m & 0x1FFF;
                let bit = (m >> 13) as u8;
                let v = self.read(addr);
                let b = v & (1 << bit) != 0;
                match op {
                    0x0A => self.c |= b,
                    0x2A => self.c |= !b,
                    0x4A => self.c &= b,
                    0x6A => self.c &= !b,
                    0x8A => self.c ^= b,
                    0xAA => self.c = b,
                    0xCA => {
                        let r = if self.c { v | (1 << bit) } else { v & !(1 << bit) };
                        self.write(addr, r);
                    },
                    _ => self.write(addr, v ^ (1 << bit)),
                }
            },
            0x0E | 0x4E => {
                let a = self.a_abs();
                let v = self.read(a);
                self.nz(self.a.wrapping_sub(v));
                let r = if op == 0x0E { v | self.a } else { v & !self.a };
                self.write(a, r);
            },
            _ => {},
        }
        CYCLES[op as usize] as u32 + extra
    }

    fn dp_x_ptr(&mut self) -> u16 {
        self.dp(self.x)
    }

    /// Run one instruction and advance timers + DSP. Returns cycles used.
    pub fn step(&mut self) -> u32 {
        let c = if self.stopped { 2 } else { self.exec() };
        for _ in 0..c {
            self.clk += 1;
            if self.clk & 15 == 0 {
                self.tick_timer(2);
            }
            if self.clk & 127 == 0 {
                self.tick_timer(0);
                self.tick_timer(1);
            }
            if self.clk & 31 == 0 {
                let s = self.dsp.run_sample(&mut self.ram);
                self.samples.push(s);
            }
        }
        c
    }

    fn tick_timer(&mut self, i: usize) {
        if !self.t_en[i] {
            return;
        }
        self.t_stage[i] = self.t_stage[i].wrapping_add(1);
        if self.t_stage[i] == self.t_target[i] {
            self.t_stage[i] = 0;
            self.t_out[i] = (self.t_out[i] + 1) & 15;
        }
    }
}
