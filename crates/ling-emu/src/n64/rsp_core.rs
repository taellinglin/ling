//! RSP (Reality Signal Processor) LLE — scalar unit + full vector unit.
//!
//! Vector-lane, accumulator, clamp and flag semantics are ported from the
//! ares emulator's serial (SISD) implementations (ISC licence,
//! ares/n64/rsp/interpreter-vpu.cpp) — the de-facto hardware-verified
//! reference. Scalar unit is the usual MIPS subset: no mult/div, no
//! exceptions, 12-bit PC in IMEM, DMEM wraps at 4 KB per byte access.

use super::mem::Bus;

pub struct Rsp {
    pub gpr: [u32; 32],
    pub pc: u32, // 12-bit
    /// Vector registers as big-endian byte arrays (byte 2n = lane n high).
    pub vpr: [[u8; 16]; 32],
    pub acc_h: [u16; 8],
    pub acc_m: [u16; 8],
    pub acc_l: [u16; 8],
    pub vco_l: u8,
    pub vco_h: u8,
    pub vcc_l: u8,
    pub vcc_h: u8,
    pub vce: u8,
    div_in: u16,
    div_out: u16,
    div_dp: bool,
    reciprocals: [u16; 512],
    inv_sqrts: [u16; 512],
    /// Honest wall: set when an unimplemented instruction executes.
    pub stuck: Option<String>,
}

#[inline]
fn lane(v: &[u8; 16], n: usize) -> u16 {
    ((v[2 * n] as u16) << 8) | v[2 * n + 1] as u16
}

#[inline]
fn set_lane(v: &mut [u8; 16], n: usize, val: u16) {
    v[2 * n] = (val >> 8) as u8;
    v[2 * n + 1] = val as u8;
}

/// Broadcast/shuffle a source register by element specifier (ares case table).
fn vte(v: &[u8; 16], e: usize) -> [u16; 8] {
    let l = |n: usize| lane(v, n);
    match e {
        0 | 1 => [l(0), l(1), l(2), l(3), l(4), l(5), l(6), l(7)],
        2 => [l(0), l(0), l(2), l(2), l(4), l(4), l(6), l(6)],
        3 => [l(1), l(1), l(3), l(3), l(5), l(5), l(7), l(7)],
        4 => [l(0), l(0), l(0), l(0), l(4), l(4), l(4), l(4)],
        5 => [l(1), l(1), l(1), l(1), l(5), l(5), l(5), l(5)],
        6 => [l(2), l(2), l(2), l(2), l(6), l(6), l(6), l(6)],
        7 => [l(3), l(3), l(3), l(3), l(7), l(7), l(7), l(7)],
        n => {
            let x = l(n - 8);
            [x; 8]
        },
    }
}

#[inline]
fn sclamp16(v: i32) -> i16 {
    v.clamp(-32768, 32767) as i16
}

impl Rsp {
    pub fn new() -> Rsp {
        let mut reciprocals = [0u16; 512];
        reciprocals[0] = 0xFFFF;
        for i in 1..512u64 {
            let a = i + 512;
            let b = (1u64 << 34) / a;
            reciprocals[i as usize] = ((b + 1) >> 8) as u16;
        }
        let mut inv_sqrts = [0u16; 512];
        for i in 0..512u64 {
            let a = (i + 512) >> (i % 2);
            let mut b = 1u64 << 17;
            while a * (b + 1) * (b + 1) < (1u64 << 44) {
                b += 1;
            }
            inv_sqrts[i as usize] = (b >> 1) as u16;
        }
        Rsp {
            gpr: [0; 32],
            pc: 0,
            vpr: [[0; 16]; 32],
            acc_h: [0; 8],
            acc_m: [0; 8],
            acc_l: [0; 8],
            vco_l: 0,
            vco_h: 0,
            vcc_l: 0,
            vcc_h: 0,
            vce: 0,
            div_in: 0,
            div_out: 0,
            div_dp: false,
            reciprocals,
            inv_sqrts,
            stuck: None,
        }
    }

    #[inline]
    fn acc_get(&self, n: usize) -> u64 {
        ((self.acc_h[n] as u64) << 32) | ((self.acc_m[n] as u64) << 16) | self.acc_l[n] as u64
    }

    #[inline]
    fn acc_set(&mut self, n: usize, v: u64) {
        self.acc_h[n] = (v >> 32) as u16;
        self.acc_m[n] = (v >> 16) as u16;
        self.acc_l[n] = v as u16;
    }

    /// ares accumulatorSaturate: clamp by the 32-bit HI:MD view.
    fn acc_saturate(&self, n: usize, slice: bool, negative: u16, positive: u16) -> u16 {
        if (self.acc_h[n] as i16) < 0 {
            if self.acc_h[n] != 0xFFFF {
                return negative;
            }
            if (self.acc_m[n] as i16) >= 0 {
                return negative;
            }
        } else {
            if self.acc_h[n] != 0x0000 {
                return positive;
            }
            if (self.acc_m[n] as i16) < 0 {
                return positive;
            }
        }
        if slice {
            self.acc_m[n]
        } else {
            self.acc_l[n]
        }
    }

    #[inline]
    fn flag(mask: u8, n: usize) -> bool {
        mask & (1 << n) != 0
    }

    #[inline]
    fn set_flag(mask: &mut u8, n: usize, v: bool) -> bool {
        if v {
            *mask |= 1 << n;
        } else {
            *mask &= !(1 << n);
        }
        v
    }

    /// Run up to `max` instructions; stops on BREAK/halt. Returns steps taken.
    pub fn run(&mut self, bus: &mut Bus, max: u32) -> u32 {
        let mut n = 0;
        while n < max && !bus.sp_halt {
            if self.stuck.is_some() {
                bus.sp_halt = true;
                break;
            }
            self.step(bus);
            n += 1;
        }
        self.pc &= 0xFFF;
        n
    }

    #[inline]
    fn fetch(&self, bus: &Bus, pc: u32) -> u32 {
        let p = (pc & 0xFFF) as usize;
        u32::from_be_bytes([
            bus.imem[p],
            bus.imem[(p + 1) & 0xFFF],
            bus.imem[(p + 2) & 0xFFF],
            bus.imem[(p + 3) & 0xFFF],
        ])
    }

    fn step(&mut self, bus: &mut Bus) {
        let pc = self.pc & 0xFFC;
        let instr = self.fetch(bus, pc);
        self.pc = (pc + 4) & 0xFFF;
        self.execute(bus, instr, pc);
        self.gpr[0] = 0;
    }

    /// Execute the delay slot (one instruction at current pc), then jump.
    fn do_jump(&mut self, bus: &mut Bus, target: u32) {
        let slot_pc = self.pc & 0xFFC;
        let slot = self.fetch(bus, slot_pc);
        self.pc = (slot_pc + 4) & 0xFFF;
        self.execute(bus, slot, slot_pc);
        self.gpr[0] = 0;
        self.pc = target & 0xFFF;
    }

    fn execute(&mut self, bus: &mut Bus, instr: u32, pc: u32) {
        let op = instr >> 26;
        let rs = ((instr >> 21) & 31) as usize;
        let rt = ((instr >> 16) & 31) as usize;
        let rd = ((instr >> 11) & 31) as usize;
        let sa = (instr >> 6) & 31;
        let imm = (instr & 0xFFFF) as u16;
        let simm = imm as i16 as i32;
        let addr = self.gpr[rs].wrapping_add(simm as u32);

        macro_rules! branch {
            ($cond:expr) => {{
                let taken = $cond;
                let slot_pc = self.pc & 0xFFC;
                let slot = self.fetch(bus, slot_pc);
                self.pc = (slot_pc + 4) & 0xFFF;
                self.execute(bus, slot, slot_pc);
                self.gpr[0] = 0;
                if taken {
                    self.pc = pc.wrapping_add(4).wrapping_add((simm as u32) << 2) & 0xFFF;
                } else {
                    self.pc = pc.wrapping_add(8) & 0xFFF;
                }
            }};
        }

        match op {
            0 => match instr & 0x3F {
                0x00 => self.gpr[rd] = self.gpr[rt] << sa,
                0x02 => self.gpr[rd] = self.gpr[rt] >> sa,
                0x03 => self.gpr[rd] = ((self.gpr[rt] as i32) >> sa) as u32,
                0x04 => self.gpr[rd] = self.gpr[rt] << (self.gpr[rs] & 31),
                0x06 => self.gpr[rd] = self.gpr[rt] >> (self.gpr[rs] & 31),
                0x07 => self.gpr[rd] = ((self.gpr[rt] as i32) >> (self.gpr[rs] & 31)) as u32,
                0x08 => {
                    let t = self.gpr[rs];
                    self.do_jump(bus, t);
                },
                0x09 => {
                    let t = self.gpr[rs];
                    self.gpr[rd] = pc.wrapping_add(8) & 0xFFF;
                    self.do_jump(bus, t);
                },
                0x0D => {
                    // BREAK
                    bus.sp_halt = true;
                    bus.sp_broke = true;
                    if bus.sp_intr_break {
                        bus.raise(super::mem::MI_INTR_SP);
                    }
                },
                0x20 | 0x21 => self.gpr[rd] = self.gpr[rs].wrapping_add(self.gpr[rt]),
                0x22 | 0x23 => self.gpr[rd] = self.gpr[rs].wrapping_sub(self.gpr[rt]),
                0x24 => self.gpr[rd] = self.gpr[rs] & self.gpr[rt],
                0x25 => self.gpr[rd] = self.gpr[rs] | self.gpr[rt],
                0x26 => self.gpr[rd] = self.gpr[rs] ^ self.gpr[rt],
                0x27 => self.gpr[rd] = !(self.gpr[rs] | self.gpr[rt]),
                0x2A => self.gpr[rd] = ((self.gpr[rs] as i32) < (self.gpr[rt] as i32)) as u32,
                0x2B => self.gpr[rd] = (self.gpr[rs] < self.gpr[rt]) as u32,
                other => self.stuck = Some(format!("RSP SPECIAL {other:#04X} at {pc:03X}")),
            },
            1 => match rt {
                0x00 => branch!((self.gpr[rs] as i32) < 0),
                0x01 => branch!((self.gpr[rs] as i32) >= 0),
                0x10 => {
                    self.gpr[31] = pc.wrapping_add(8) & 0xFFF;
                    branch!((self.gpr[rs] as i32) < 0);
                },
                0x11 => {
                    self.gpr[31] = pc.wrapping_add(8) & 0xFFF;
                    branch!((self.gpr[rs] as i32) >= 0);
                },
                other => self.stuck = Some(format!("RSP REGIMM {other:#04X} at {pc:03X}")),
            },
            2 => self.do_jump(bus, (instr & 0x03FF_FFFF) << 2),
            3 => {
                self.gpr[31] = pc.wrapping_add(8) & 0xFFF;
                self.do_jump(bus, (instr & 0x03FF_FFFF) << 2);
            },
            4 => branch!(self.gpr[rs] == self.gpr[rt]),
            5 => branch!(self.gpr[rs] != self.gpr[rt]),
            6 => branch!((self.gpr[rs] as i32) <= 0),
            7 => branch!((self.gpr[rs] as i32) > 0),
            8 | 9 => self.gpr[rt] = self.gpr[rs].wrapping_add(simm as u32),
            10 => self.gpr[rt] = ((self.gpr[rs] as i32) < simm) as u32,
            11 => self.gpr[rt] = (self.gpr[rs] < simm as u32) as u32,
            12 => self.gpr[rt] = self.gpr[rs] & imm as u32,
            13 => self.gpr[rt] = self.gpr[rs] | imm as u32,
            14 => self.gpr[rt] = self.gpr[rs] ^ imm as u32,
            15 => self.gpr[rt] = (imm as u32) << 16,
            16 => match rs {
                0 => self.gpr[rt] = bus.rsp_cop0_read(rd),
                4 => bus.rsp_cop0_write(rd, self.gpr[rt]),
                other => self.stuck = Some(format!("RSP COP0 rs={other} at {pc:03X}")),
            },
            18 => self.cop2(bus, instr, pc),
            32 => self.gpr[rt] = bus.dmem[(addr & 0xFFF) as usize] as i8 as u32,
            33 => {
                let v = ((bus.dmem[(addr & 0xFFF) as usize] as u16) << 8)
                    | bus.dmem[(addr.wrapping_add(1) & 0xFFF) as usize] as u16;
                self.gpr[rt] = v as i16 as u32;
            },
            35 => {
                let mut v = 0u32;
                for k in 0..4 {
                    v = (v << 8) | bus.dmem[(addr.wrapping_add(k) & 0xFFF) as usize] as u32;
                }
                self.gpr[rt] = v;
            },
            36 => self.gpr[rt] = bus.dmem[(addr & 0xFFF) as usize] as u32,
            37 => {
                let v = ((bus.dmem[(addr & 0xFFF) as usize] as u16) << 8)
                    | bus.dmem[(addr.wrapping_add(1) & 0xFFF) as usize] as u16;
                self.gpr[rt] = v as u32;
            },
            40 => bus.dmem[(addr & 0xFFF) as usize] = self.gpr[rt] as u8,
            41 => {
                bus.dmem[(addr & 0xFFF) as usize] = (self.gpr[rt] >> 8) as u8;
                bus.dmem[(addr.wrapping_add(1) & 0xFFF) as usize] = self.gpr[rt] as u8;
            },
            43 => {
                for k in 0..4 {
                    bus.dmem[(addr.wrapping_add(k) & 0xFFF) as usize] =
                        (self.gpr[rt] >> (8 * (3 - k))) as u8;
                }
            },
            50 => self.lwc2(bus, instr, addr),
            58 => self.swc2(bus, instr, addr),
            other => self.stuck = Some(format!("RSP opcode {other} at {pc:03X}")),
        }
    }

    // ── vector loads/stores ──────────────────────────────────────────────────

    fn lwc2(&mut self, bus: &mut Bus, instr: u32, _a: u32) {
        let base = ((instr >> 21) & 31) as usize;
        let vt = ((instr >> 16) & 31) as usize;
        let funct = (instr >> 11) & 31;
        let e = ((instr >> 7) & 15) as usize;
        let off7 = ((instr & 0x7F) as i32).wrapping_shl(25).wrapping_shr(25); // sign-extend 7-bit
        let rsv = self.gpr[base];
        let rd8 = |bus: &Bus, a: u32| bus.dmem[(a & 0xFFF) as usize];
        match funct {
            0x00 => {
                let a = rsv.wrapping_add(off7 as u32);
                self.vpr[vt][e] = rd8(bus, a);
            },
            0x01 => {
                let mut a = rsv.wrapping_add((off7 * 2) as u32);
                for o in e..(e + 2).min(16) {
                    self.vpr[vt][o & 15] = rd8(bus, a);
                    a = a.wrapping_add(1);
                }
            },
            0x02 => {
                let mut a = rsv.wrapping_add((off7 * 4) as u32);
                for o in e..(e + 4).min(16) {
                    self.vpr[vt][o & 15] = rd8(bus, a);
                    a = a.wrapping_add(1);
                }
            },
            0x03 => {
                let mut a = rsv.wrapping_add((off7 * 8) as u32);
                for o in e..(e + 8).min(16) {
                    self.vpr[vt][o & 15] = rd8(bus, a);
                    a = a.wrapping_add(1);
                }
            },
            0x04 => {
                // LQV
                let mut a = rsv.wrapping_add((off7 * 16) as u32);
                let end = (16 + e as u32 - (a & 15)).min(16);
                for o in e as u32..end {
                    self.vpr[vt][(o & 15) as usize] = rd8(bus, a);
                    a = a.wrapping_add(1);
                }
            },
            0x05 => {
                // LRV
                let a0 = rsv.wrapping_add((off7 * 16) as u32);
                let start = 16i32 - ((a0 & 15) as i32 - e as i32);
                let mut a = a0 & !15;
                for o in start.max(0)..16 {
                    self.vpr[vt][(o & 15) as usize] = rd8(bus, a);
                    a = a.wrapping_add(1);
                }
            },
            0x06 => {
                // LPV
                let a0 = rsv.wrapping_add((off7 * 8) as u32);
                let index = (a0 & 7) as i32 - e as i32;
                let a = a0 & !7;
                for o in 0..8i32 {
                    let b = rd8(bus, a.wrapping_add(((index + o) & 15) as u32));
                    set_lane(&mut self.vpr[vt], o as usize, (b as u16) << 8);
                }
            },
            0x07 => {
                // LUV
                let a0 = rsv.wrapping_add((off7 * 8) as u32);
                let index = (a0 & 7) as i32 - e as i32;
                let a = a0 & !7;
                for o in 0..8i32 {
                    let b = rd8(bus, a.wrapping_add(((index + o) & 15) as u32));
                    set_lane(&mut self.vpr[vt], o as usize, (b as u16) << 7);
                }
            },
            0x08 => {
                // LHV
                let a0 = rsv.wrapping_add((off7 * 16) as u32);
                let index = (a0 & 7) as i32 - e as i32;
                let a = a0 & !7;
                for o in 0..8i32 {
                    let b = rd8(bus, a.wrapping_add(((index + o * 2) & 15) as u32));
                    set_lane(&mut self.vpr[vt], o as usize, (b as u16) << 7);
                }
            },
            0x09 => {
                // LFV
                let a0 = rsv.wrapping_add((off7 * 16) as u32);
                let index = (a0 & 7) as i32 - e as i32;
                let a = a0 & !7;
                let mut tmp = [0u8; 16];
                for o in 0..4i32 {
                    let b0 = rd8(bus, a.wrapping_add(((index + o * 4) & 15) as u32));
                    set_lane(&mut tmp, o as usize, (b0 as u16) << 7);
                    let b1 = rd8(bus, a.wrapping_add(((index + o * 4 + 8) & 15) as u32));
                    set_lane(&mut tmp, (o + 4) as usize, (b1 as u16) << 7);
                }
                for o in e..(e + 8).min(16) {
                    self.vpr[vt][o] = tmp[o];
                }
            },
            0x0B => {
                // LTV (transpose)
                let a0 = rsv.wrapping_add((off7 * 16) as u32);
                let begin = a0 & !7;
                let mut a = begin + ((e as u32 + (a0 & 8)) & 15);
                let vtbase = vt & !7;
                let mut vtoff = e >> 1;
                for i in 0..8 {
                    self.vpr[vtbase + vtoff][i * 2] = rd8(bus, a);
                    a += 1;
                    if a == begin + 16 {
                        a = begin;
                    }
                    self.vpr[vtbase + vtoff][i * 2 + 1] = rd8(bus, a);
                    a += 1;
                    if a == begin + 16 {
                        a = begin;
                    }
                    vtoff = (vtoff + 1) & 7;
                }
            },
            _ => {}, // LWV absent on hardware
        }
    }

    fn swc2(&mut self, bus: &mut Bus, instr: u32, _a: u32) {
        let base = ((instr >> 21) & 31) as usize;
        let vt = ((instr >> 16) & 31) as usize;
        let funct = (instr >> 11) & 31;
        let e = ((instr >> 7) & 15) as usize;
        let off7 = ((instr & 0x7F) as i32).wrapping_shl(25).wrapping_shr(25);
        let rsv = self.gpr[base];
        macro_rules! wr8 {
            ($a:expr, $v:expr) => {
                bus.dmem[($a & 0xFFF) as usize] = $v
            };
        }
        match funct {
            0x00 => {
                let a = rsv.wrapping_add(off7 as u32);
                wr8!(a, self.vpr[vt][e]);
            },
            0x01 => {
                let mut a = rsv.wrapping_add((off7 * 2) as u32);
                for o in e..e + 2 {
                    wr8!(a, self.vpr[vt][o & 15]);
                    a = a.wrapping_add(1);
                }
            },
            0x02 => {
                let mut a = rsv.wrapping_add((off7 * 4) as u32);
                for o in e..e + 4 {
                    wr8!(a, self.vpr[vt][o & 15]);
                    a = a.wrapping_add(1);
                }
            },
            0x03 => {
                let mut a = rsv.wrapping_add((off7 * 8) as u32);
                for o in e..e + 8 {
                    wr8!(a, self.vpr[vt][o & 15]);
                    a = a.wrapping_add(1);
                }
            },
            0x04 => {
                // SQV
                let mut a = rsv.wrapping_add((off7 * 16) as u32);
                let end = e as u32 + (16 - (a & 15));
                for o in e as u32..end {
                    wr8!(a, self.vpr[vt][(o & 15) as usize]);
                    a = a.wrapping_add(1);
                }
            },
            0x05 => {
                // SRV
                let a0 = rsv.wrapping_add((off7 * 16) as u32);
                let end = e as u32 + (a0 & 15);
                let base16 = 16 - (a0 & 15);
                let mut a = a0 & !15;
                for o in e as u32..end {
                    wr8!(a, self.vpr[vt][((o + base16) & 15) as usize]);
                    a = a.wrapping_add(1);
                }
            },
            0x06 => {
                // SPV
                let mut a = rsv.wrapping_add((off7 * 8) as u32);
                for o in e..e + 8 {
                    if (o & 15) < 8 {
                        wr8!(a, self.vpr[vt][(o & 7) << 1]);
                    } else {
                        wr8!(a, (lane(&self.vpr[vt], o & 7) >> 7) as u8);
                    }
                    a = a.wrapping_add(1);
                }
            },
            0x07 => {
                // SUV
                let mut a = rsv.wrapping_add((off7 * 8) as u32);
                for o in e..e + 8 {
                    if (o & 15) < 8 {
                        wr8!(a, (lane(&self.vpr[vt], o & 7) >> 7) as u8);
                    } else {
                        wr8!(a, self.vpr[vt][(o & 7) << 1]);
                    }
                    a = a.wrapping_add(1);
                }
            },
            0x08 => {
                // SHV
                let a0 = rsv.wrapping_add((off7 * 16) as u32);
                let index = a0 & 7;
                let a = a0 & !7;
                for o in 0..8usize {
                    let byte = e + o * 2;
                    let v = (self.vpr[vt][byte & 15] << 1) | (self.vpr[vt][(byte + 1) & 15] >> 7);
                    wr8!(a.wrapping_add((index + o as u32 * 2) & 15), v);
                }
            },
            0x09 => {
                // SFV
                let a0 = rsv.wrapping_add((off7 * 16) as u32);
                let b = a0 & 7;
                let a = a0 & !7;
                let lanes: [i32; 4] = match e {
                    0 | 15 => [0, 1, 2, 3],
                    1 => [6, 7, 4, 5],
                    4 => [1, 2, 3, 0],
                    5 => [7, 4, 5, 6],
                    8 => [4, 5, 6, 7],
                    11 => [3, 0, 1, 2],
                    12 => [5, 6, 7, 4],
                    _ => [-1, -1, -1, -1],
                };
                for (i, ln) in lanes.iter().enumerate() {
                    let v = if *ln < 0 { 0 } else { (lane(&self.vpr[vt], *ln as usize) >> 7) as u8 };
                    wr8!(a.wrapping_add((b + i as u32 * 4) & 15), v);
                }
            },
            0x0A => {
                // SWV
                let a0 = rsv.wrapping_add((off7 * 16) as u32);
                let mut b = a0 & 7;
                let a = a0 & !7;
                for o in e..e + 16 {
                    wr8!(a.wrapping_add(b & 15), self.vpr[vt][o & 15]);
                    b += 1;
                }
            },
            0x0B => {
                // STV
                let a0 = rsv.wrapping_add((off7 * 16) as u32);
                let start = vt & !7;
                let mut element = 16 - (e & !1);
                let mut b = (a0 & 7).wrapping_sub((e & !1) as u32);
                let a = a0 & !7;
                for reg in start..start + 8 {
                    wr8!(a.wrapping_add(b & 15), self.vpr[reg][element & 15]);
                    b = b.wrapping_add(1);
                    element += 1;
                    wr8!(a.wrapping_add(b & 15), self.vpr[reg][element & 15]);
                    b = b.wrapping_add(1);
                    element += 1;
                }
            },
            _ => {},
        }
    }

    // ── vector compute ───────────────────────────────────────────────────────

    fn cop2(&mut self, bus: &mut Bus, instr: u32, pc: u32) {
        if instr >> 25 & 1 == 0 {
            // scalar<->vector moves
            let rt = ((instr >> 16) & 31) as usize;
            let vs = ((instr >> 11) & 31) as usize;
            let e = ((instr >> 7) & 15) as usize;
            match (instr >> 21) & 31 {
                0x00 => {
                    // MFC2
                    let hi = self.vpr[vs][e & 15];
                    let lo = self.vpr[vs][(e + 1) & 15];
                    if rt != 0 {
                        self.gpr[rt] = (((hi as u16) << 8) | lo as u16) as i16 as u32;
                    }
                },
                0x02 => {
                    // CFC2
                    let (hi, lo) = match vs & 3 {
                        0 => (self.vco_h, self.vco_l),
                        1 => (self.vcc_h, self.vcc_l),
                        _ => (0, self.vce),
                    };
                    if rt != 0 {
                        self.gpr[rt] = (((hi as u16) << 8) | lo as u16) as i16 as u32;
                    }
                },
                0x04 => {
                    // MTC2
                    let v = self.gpr[rt];
                    self.vpr[vs][e] = (v >> 8) as u8;
                    if e != 15 {
                        self.vpr[vs][e + 1] = v as u8;
                    }
                },
                0x06 => {
                    // CTC2
                    let v = self.gpr[rt];
                    match vs & 3 {
                        0 => {
                            self.vco_l = v as u8;
                            self.vco_h = (v >> 8) as u8;
                        },
                        1 => {
                            self.vcc_l = v as u8;
                            self.vcc_h = (v >> 8) as u8;
                        },
                        _ => self.vce = v as u8,
                    }
                },
                other => self.stuck = Some(format!("RSP COP2 move rs={other} at {pc:03X}")),
            }
            return;
        }

        let e = ((instr >> 21) & 15) as usize;
        let vtn = ((instr >> 16) & 31) as usize;
        let vsn = ((instr >> 11) & 31) as usize;
        let vdn = ((instr >> 6) & 31) as usize;
        let de = ((instr >> 11) & 7) as usize;
        let vt = vte(&self.vpr[vtn], e);
        let vs: [u16; 8] = {
            let r = &self.vpr[vsn];
            [lane(r, 0), lane(r, 1), lane(r, 2), lane(r, 3), lane(r, 4), lane(r, 5), lane(r, 6), lane(r, 7)]
        };
        let mut vd = [0u16; 8];
        let funct = instr & 0x3F;
        let _ = bus;

        match funct {
            0x00 | 0x01 => {
                // VMULF / VMULU
                for n in 0..8 {
                    let p = (vs[n] as i16 as i64) * (vt[n] as i16 as i64) * 2 + 0x8000;
                    self.acc_set(n, p as u64 & 0xFFFF_FFFF_FFFF);
                    if funct == 0 {
                        vd[n] = self.acc_saturate(n, true, 0x8000, 0x7FFF);
                    } else {
                        vd[n] = if (self.acc_h[n] as i16) < 0 {
                            0x0000
                        } else if ((self.acc_h[n] ^ self.acc_m[n]) as i16) < 0 {
                            0xFFFF
                        } else {
                            self.acc_m[n]
                        };
                    }
                }
            },
            0x02 | 0x0A => {
                // VRNDP (0x02) / VRNDN (0x0A); vs register NUMBER selects shift
                for n in 0..8 {
                    let mut product = vt[n] as i16 as i64;
                    if vsn & 1 == 1 {
                        product <<= 16;
                    }
                    let mut acc = ((self.acc_get(n) << 16) as i64) >> 16;
                    let add = if funct == 0x02 { acc >= 0 } else { acc < 0 };
                    if add {
                        acc = ((acc + product) << 16) >> 16;
                    }
                    self.acc_set(n, acc as u64 & 0xFFFF_FFFF_FFFF);
                    vd[n] = sclamp16((acc >> 16) as i32) as u16;
                }
            },
            0x03 => {
                // VMULQ
                for n in 0..8 {
                    let mut product = (vs[n] as i16 as i32) * (vt[n] as i16 as i32);
                    if product < 0 {
                        product += 31;
                    }
                    self.acc_h[n] = (product >> 16) as u16;
                    self.acc_m[n] = product as u16;
                    self.acc_l[n] = 0;
                    vd[n] = (sclamp16(product >> 1) as u16) & !15;
                }
            },
            0x04 => {
                // VMUDL
                for n in 0..8 {
                    let p = ((vs[n] as u32 * vt[n] as u32) >> 16) as u16;
                    self.acc_set(n, p as u64);
                }
                vd = self.acc_l;
            },
            0x05 => {
                // VMUDM
                for n in 0..8 {
                    let p = (vs[n] as i16 as i32) * (vt[n] as u32 as i32);
                    self.acc_set(n, (p as i64 as u64) & 0xFFFF_FFFF_FFFF);
                }
                vd = self.acc_m;
            },
            0x06 => {
                // VMUDN
                for n in 0..8 {
                    let p = (vs[n] as u32 as i32).wrapping_mul(vt[n] as i16 as i32);
                    self.acc_set(n, (p as i64 as u64) & 0xFFFF_FFFF_FFFF);
                }
                vd = self.acc_l;
            },
            0x07 => {
                // VMUDH
                for n in 0..8 {
                    let p = ((vs[n] as i16 as i64) * (vt[n] as i16 as i64)) << 16;
                    self.acc_set(n, (p as u64) & 0xFFFF_FFFF_FFFF);
                    vd[n] = self.acc_saturate(n, true, 0x8000, 0x7FFF);
                }
            },
            0x08 | 0x09 => {
                // VMACF / VMACU
                for n in 0..8 {
                    let p = (vs[n] as i16 as i64) * (vt[n] as i16 as i64) * 2;
                    let acc = self.acc_get(n).wrapping_add(p as u64) & 0xFFFF_FFFF_FFFF;
                    self.acc_set(n, acc);
                    if funct == 0x08 {
                        vd[n] = self.acc_saturate(n, true, 0x8000, 0x7FFF);
                    } else {
                        vd[n] = if (self.acc_h[n] as i16) < 0 {
                            0x0000
                        } else if self.acc_h[n] != 0 || (self.acc_m[n] as i16) < 0 {
                            0xFFFF
                        } else {
                            self.acc_m[n]
                        };
                    }
                }
            },
            0x0B => {
                // VMACQ
                for n in 0..8 {
                    let mut product =
                        (((self.acc_h[n] as u32) << 16) | self.acc_m[n] as u32) as i32;
                    if product < 0 && product & 32 == 0 {
                        product += 32;
                    } else if product >= 32 && product & 32 == 0 {
                        product -= 32;
                    }
                    self.acc_h[n] = (product >> 16) as u16;
                    self.acc_m[n] = product as u16;
                    vd[n] = (sclamp16(product >> 1) as u16) & !15;
                }
            },
            0x0C => {
                // VMADL
                for n in 0..8 {
                    let p = ((vs[n] as u32 * vt[n] as u32) >> 16) as u64;
                    let acc = self.acc_get(n).wrapping_add(p) & 0xFFFF_FFFF_FFFF;
                    self.acc_set(n, acc);
                    vd[n] = self.acc_saturate(n, false, 0x0000, 0xFFFF);
                }
            },
            0x0D => {
                // VMADM
                for n in 0..8 {
                    let p = (vs[n] as i16 as i64) * (vt[n] as u64 as i64);
                    let acc = self.acc_get(n).wrapping_add(p as u64) & 0xFFFF_FFFF_FFFF;
                    self.acc_set(n, acc);
                    vd[n] = self.acc_saturate(n, true, 0x8000, 0x7FFF);
                }
            },
            0x0E => {
                // VMADN
                for n in 0..8 {
                    let p = (vs[n] as u64 as i64) * (vt[n] as i16 as i64);
                    let acc = self.acc_get(n).wrapping_add(p as u64) & 0xFFFF_FFFF_FFFF;
                    self.acc_set(n, acc);
                    vd[n] = self.acc_saturate(n, false, 0x0000, 0xFFFF);
                }
            },
            0x0F => {
                // VMADH: only HI:MD updated, ACCL untouched
                for n in 0..8 {
                    let hi_md = (self.acc_get(n) >> 16) as u32;
                    let result = hi_md
                        .wrapping_add(((vs[n] as i16 as i32).wrapping_mul(vt[n] as i16 as i32)) as u32);
                    self.acc_h[n] = (result >> 16) as u16;
                    self.acc_m[n] = result as u16;
                    vd[n] = self.acc_saturate(n, true, 0x8000, 0x7FFF);
                }
            },
            0x10 => {
                // VADD
                for n in 0..8 {
                    let r = (vs[n] as i16 as i32)
                        + (vt[n] as i16 as i32)
                        + Self::flag(self.vco_l, n) as i32;
                    self.acc_l[n] = r as u16;
                    vd[n] = sclamp16(r) as u16;
                }
                self.vco_l = 0;
                self.vco_h = 0;
            },
            0x11 => {
                // VSUB
                for n in 0..8 {
                    let r = (vs[n] as i16 as i32)
                        - (vt[n] as i16 as i32)
                        - Self::flag(self.vco_l, n) as i32;
                    self.acc_l[n] = r as u16;
                    vd[n] = sclamp16(r) as u16;
                }
                self.vco_l = 0;
                self.vco_h = 0;
            },
            0x13 => {
                // VABS
                for n in 0..8 {
                    let s = vs[n] as i16;
                    let t = vt[n] as i16;
                    if s < 0 {
                        if t == -32768 {
                            self.acc_l[n] = 0x8000;
                            vd[n] = 0x7FFF;
                        } else {
                            self.acc_l[n] = (-t) as u16;
                            vd[n] = (-t) as u16;
                        }
                    } else if s > 0 {
                        self.acc_l[n] = t as u16;
                        vd[n] = t as u16;
                    } else {
                        self.acc_l[n] = 0;
                        vd[n] = 0;
                    }
                }
            },
            0x14 => {
                // VADDC
                for n in 0..8 {
                    let r = vs[n] as u32 + vt[n] as u32;
                    self.acc_l[n] = r as u16;
                    Self::set_flag(&mut self.vco_l, n, r >> 16 != 0);
                }
                self.vco_h = 0;
                vd = self.acc_l;
            },
            0x15 => {
                // VSUBC
                for n in 0..8 {
                    let r = (vs[n] as u32).wrapping_sub(vt[n] as u32);
                    self.acc_l[n] = r as u16;
                    Self::set_flag(&mut self.vco_l, n, r >> 16 != 0);
                    Self::set_flag(&mut self.vco_h, n, r != 0);
                }
                vd = self.acc_l;
            },
            0x1D => {
                // VSAR
                vd = match e {
                    8 => self.acc_h,
                    9 => self.acc_m,
                    10 => self.acc_l,
                    _ => [0; 8],
                };
            },
            0x20 => {
                // VLT
                for n in 0..8 {
                    let cond = (vs[n] as i16) < (vt[n] as i16)
                        || ((vs[n] as i16) == (vt[n] as i16)
                            && Self::flag(self.vco_l, n)
                            && Self::flag(self.vco_h, n));
                    Self::set_flag(&mut self.vcc_l, n, cond);
                    self.acc_l[n] = if cond { vs[n] } else { vt[n] };
                }
                self.vcc_h = 0;
                self.vco_l = 0;
                self.vco_h = 0;
                vd = self.acc_l;
            },
            0x21 => {
                // VEQ
                for n in 0..8 {
                    let cond = !Self::flag(self.vco_h, n) && vs[n] == vt[n];
                    Self::set_flag(&mut self.vcc_l, n, cond);
                    self.acc_l[n] = if cond { vs[n] } else { vt[n] };
                }
                self.vcc_h = 0;
                self.vco_l = 0;
                self.vco_h = 0;
                vd = self.acc_l;
            },
            0x22 => {
                // VNE
                for n in 0..8 {
                    let cond = vs[n] != vt[n] || Self::flag(self.vco_h, n);
                    Self::set_flag(&mut self.vcc_l, n, cond);
                    self.acc_l[n] = if cond { vs[n] } else { vt[n] };
                }
                self.vcc_h = 0;
                self.vco_l = 0;
                self.vco_h = 0;
                vd = self.acc_l;
            },
            0x23 => {
                // VGE
                for n in 0..8 {
                    let cond = (vs[n] as i16) > (vt[n] as i16)
                        || ((vs[n] as i16) == (vt[n] as i16)
                            && (!Self::flag(self.vco_l, n) || !Self::flag(self.vco_h, n)));
                    Self::set_flag(&mut self.vcc_l, n, cond);
                    self.acc_l[n] = if cond { vs[n] } else { vt[n] };
                }
                self.vcc_h = 0;
                self.vco_l = 0;
                self.vco_h = 0;
                vd = self.acc_l;
            },
            0x24 => {
                // VCL
                for n in 0..8 {
                    if Self::flag(self.vco_l, n) {
                        if Self::flag(self.vco_h, n) {
                            self.acc_l[n] = if Self::flag(self.vcc_l, n) {
                                (vt[n] as i16).wrapping_neg() as u16
                            } else {
                                vs[n]
                            };
                        } else {
                            let sum = (vs[n] as u32).wrapping_add(vt[n] as u32);
                            let sum16 = sum as u16;
                            let carry = sum >> 16 != 0;
                            let cond = if Self::flag(self.vce, n) {
                                sum16 == 0 || !carry
                            } else {
                                sum16 == 0 && !carry
                            };
                            Self::set_flag(&mut self.vcc_l, n, cond);
                            self.acc_l[n] = if cond {
                                (vt[n] as i16).wrapping_neg() as u16
                            } else {
                                vs[n]
                            };
                        }
                    } else {
                        if Self::flag(self.vco_h, n) {
                            self.acc_l[n] =
                                if Self::flag(self.vcc_h, n) { vt[n] } else { vs[n] };
                        } else {
                            let cond = vs[n] as i32 - vt[n] as i32 >= 0;
                            Self::set_flag(&mut self.vcc_h, n, cond);
                            self.acc_l[n] = if cond { vt[n] } else { vs[n] };
                        }
                    }
                }
                self.vco_l = 0;
                self.vco_h = 0;
                self.vce = 0;
                vd = self.acc_l;
            },
            0x25 => {
                // VCH
                for n in 0..8 {
                    let s = vs[n] as i16;
                    let t = vt[n] as i16;
                    if (s ^ t) < 0 {
                        let result = s.wrapping_add(t);
                        self.acc_l[n] =
                            if result <= 0 { t.wrapping_neg() as u16 } else { s as u16 };
                        Self::set_flag(&mut self.vcc_l, n, result <= 0);
                        Self::set_flag(&mut self.vcc_h, n, t < 0);
                        Self::set_flag(&mut self.vco_l, n, true);
                        Self::set_flag(
                            &mut self.vco_h,
                            n,
                            result != 0 && vs[n] != (vt[n] ^ 0xFFFF),
                        );
                        Self::set_flag(&mut self.vce, n, result == -1);
                    } else {
                        let result = s.wrapping_sub(t);
                        self.acc_l[n] = if result >= 0 { t as u16 } else { s as u16 };
                        Self::set_flag(&mut self.vcc_l, n, t < 0);
                        Self::set_flag(&mut self.vcc_h, n, result >= 0);
                        Self::set_flag(&mut self.vco_l, n, false);
                        Self::set_flag(
                            &mut self.vco_h,
                            n,
                            result != 0 && vs[n] != (vt[n] ^ 0xFFFF),
                        );
                        Self::set_flag(&mut self.vce, n, false);
                    }
                }
                vd = self.acc_l;
            },
            0x26 => {
                // VCR
                for n in 0..8 {
                    let s = vs[n] as i16;
                    let t = vt[n] as i16;
                    if (s ^ t) < 0 {
                        Self::set_flag(&mut self.vcc_h, n, t < 0);
                        let cond = s as i32 + t as i32 + 1 <= 0;
                        Self::set_flag(&mut self.vcc_l, n, cond);
                        self.acc_l[n] = if cond { !(t as u16) } else { s as u16 };
                    } else {
                        Self::set_flag(&mut self.vcc_l, n, t < 0);
                        let cond = s as i32 - t as i32 >= 0;
                        Self::set_flag(&mut self.vcc_h, n, cond);
                        self.acc_l[n] = if cond { t as u16 } else { s as u16 };
                    }
                }
                self.vco_l = 0;
                self.vco_h = 0;
                self.vce = 0;
                vd = self.acc_l;
            },
            0x27 => {
                // VMRG
                for n in 0..8 {
                    self.acc_l[n] = if Self::flag(self.vcc_l, n) { vs[n] } else { vt[n] };
                }
                self.vco_l = 0;
                self.vco_h = 0;
                vd = self.acc_l;
            },
            0x28 => {
                for n in 0..8 {
                    self.acc_l[n] = vs[n] & vt[n];
                }
                vd = self.acc_l;
            },
            0x29 => {
                for n in 0..8 {
                    self.acc_l[n] = !(vs[n] & vt[n]);
                }
                vd = self.acc_l;
            },
            0x2A => {
                for n in 0..8 {
                    self.acc_l[n] = vs[n] | vt[n];
                }
                vd = self.acc_l;
            },
            0x2B => {
                for n in 0..8 {
                    self.acc_l[n] = !(vs[n] | vt[n]);
                }
                vd = self.acc_l;
            },
            0x2C => {
                for n in 0..8 {
                    self.acc_l[n] = vs[n] ^ vt[n];
                }
                vd = self.acc_l;
            },
            0x2D => {
                for n in 0..8 {
                    self.acc_l[n] = !(vs[n] ^ vt[n]);
                }
                vd = self.acc_l;
            },
            0x30 | 0x31 | 0x34 | 0x35 => {
                // VRCP / VRCPL / VRSQ / VRSQL
                let l = funct == 0x31 || funct == 0x35;
                let rsq = funct >= 0x34;
                let input: i32 = if l && self.div_dp {
                    (((self.div_in as u32) << 16) | vt[e & 7] as u32) as i32
                } else {
                    vt[e & 7] as i16 as i32
                };
                let mask = input >> 31;
                let mut data = input ^ mask;
                if input > -32768 {
                    data -= mask;
                }
                let result: i32 = if data == 0 {
                    0x7FFF_FFFF
                } else if input == -32768 {
                    -65536 // 0xFFFF0000
                } else {
                    let shift = (data as u32).leading_zeros();
                    let index = (((data as u64) << shift) & 0x7FC0_0000) >> 22;
                    let r = if rsq {
                        self.inv_sqrts[((index & 0x1FE) | (shift as u64 & 1)) as usize]
                    } else {
                        self.reciprocals[index as usize]
                    };
                    let r = ((0x10000 | r as u32) << 14) as i32;
                    let sh = if rsq { (31 - shift) >> 1 } else { 31 - shift };
                    ((r as u32) >> sh) as i32 ^ mask
                };
                self.div_dp = false;
                self.div_out = (result >> 16) as u16;
                self.acc_l = vt;
                let mut out = self.vpr[vdn];
                set_lane(&mut out, de, result as u16);
                self.vpr[vdn] = out;
                // write handled; skip generic vd store
                return;
            },
            0x32 | 0x36 => {
                // VRCPH / VRSQH
                self.acc_l = vt;
                self.div_dp = true;
                self.div_in = vt[e & 7];
                let mut out = self.vpr[vdn];
                set_lane(&mut out, de, self.div_out);
                self.vpr[vdn] = out;
                return;
            },
            0x33 => {
                // VMOV
                let mut out = self.vpr[vdn];
                set_lane(&mut out, de, vt[de]);
                self.vpr[vdn] = out;
                self.acc_l = vt;
                return;
            },
            0x37 | 0x3F => return, // VNOP / VNULL
            _ => {
                // VZERO family: acc = vs+vt, vd = 0
                for n in 0..8 {
                    self.acc_l[n] = (vs[n] as i16).wrapping_add(vt[n] as i16) as u16;
                }
                vd = [0; 8];
            },
        }

        let mut out = [0u8; 16];
        for n in 0..8 {
            out[2 * n] = (vd[n] >> 8) as u8;
            out[2 * n + 1] = vd[n] as u8;
        }
        self.vpr[vdn] = out;
    }
}
