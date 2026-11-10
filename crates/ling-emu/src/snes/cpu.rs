//! WDC 65C816 — the SNES main CPU. Full instruction set, emulation/native
//! modes, 24-bit addressing, direct-page/stack-relative modes, decimal ADC/SBC
//! (8 and 16-bit), block moves, WAI/STP, NMI/IRQ/BRK/COP. Timing is counted in
//! master-clock cycles through the bus (6 fast / 8 slow / 12 xslow per access,
//! 6 per internal cycle).

pub const C: u8 = 0x01;
pub const Z: u8 = 0x02;
pub const I: u8 = 0x04;
pub const D: u8 = 0x08;
pub const X: u8 = 0x10;
pub const M: u8 = 0x20;
pub const V: u8 = 0x40;
pub const N: u8 = 0x80;

pub trait CpuBus {
    fn read(&mut self, addr: u32) -> u8;
    fn write(&mut self, addr: u32, v: u8);
    /// One internal (non-bus) cycle.
    fn idle(&mut self);
    /// A dummy bus cycle at `addr` (costs that region's access time).
    fn cycle_at(&mut self, addr: u32);
    /// Consume a latched NMI edge.
    fn take_nmi(&mut self) -> bool;
    fn irq(&self) -> bool;
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Mode {
    Dp,
    DpX,
    DpY,
    Abs,
    AbsX,
    AbsY,
    Long,
    LongX,
    DpInd,
    DpXInd,
    DpIndY,
    DpIndL,
    DpIndLY,
    Sr,
    SrIndY,
}

pub struct Cpu {
    pub a: u16,
    pub x: u16,
    pub y: u16,
    pub s: u16,
    pub d: u16,
    pub db: u8,
    pub pb: u8,
    pub pc: u16,
    pub p: u8,
    pub e: bool,
    pub waiting: bool,
    pub stopped: bool,
}

impl Cpu {
    pub fn new() -> Cpu {
        Cpu {
            a: 0,
            x: 0,
            y: 0,
            s: 0x01FF,
            d: 0,
            db: 0,
            pb: 0,
            pc: 0,
            p: M | X | I,
            e: true,
            waiting: false,
            stopped: false,
        }
    }

    pub fn reset<B: CpuBus>(&mut self, b: &mut B) {
        self.e = true;
        self.p = M | X | I;
        self.s = 0x01FF;
        self.d = 0;
        self.db = 0;
        self.pb = 0;
        self.x &= 0xFF;
        self.y &= 0xFF;
        self.waiting = false;
        self.stopped = false;
        let lo = b.read(0xFFFC) as u16;
        let hi = b.read(0xFFFD) as u16;
        self.pc = (hi << 8) | lo;
    }

    #[inline]
    fn m8(&self) -> bool {
        self.e || self.p & M != 0
    }
    #[inline]
    fn x8(&self) -> bool {
        self.e || self.p & X != 0
    }

    // ── fetch ────────────────────────────────────────────────────────────────
    #[inline]
    fn fetch8<B: CpuBus>(&mut self, b: &mut B) -> u8 {
        let v = b.read(((self.pb as u32) << 16) | self.pc as u32);
        self.pc = self.pc.wrapping_add(1);
        v
    }
    #[inline]
    fn fetch16<B: CpuBus>(&mut self, b: &mut B) -> u16 {
        let lo = self.fetch8(b) as u16;
        let hi = self.fetch8(b) as u16;
        (hi << 8) | lo
    }
    #[inline]
    fn fetch24<B: CpuBus>(&mut self, b: &mut B) -> u32 {
        let lo = self.fetch16(b) as u32;
        let hi = self.fetch8(b) as u32;
        (hi << 16) | lo
    }

    // ── flags ────────────────────────────────────────────────────────────────
    #[inline]
    fn set(&mut self, flag: u8, on: bool) {
        if on {
            self.p |= flag;
        } else {
            self.p &= !flag;
        }
    }
    #[inline]
    fn nz8(&mut self, v: u8) {
        self.set(Z, v == 0);
        self.set(N, v & 0x80 != 0);
    }
    #[inline]
    fn nz16(&mut self, v: u16) {
        self.set(Z, v == 0);
        self.set(N, v & 0x8000 != 0);
    }
    #[inline]
    fn nz_m(&mut self, v: u16) {
        if self.m8() {
            self.nz8(v as u8)
        } else {
            self.nz16(v)
        }
    }
    #[inline]
    fn nz_x(&mut self, v: u16) {
        if self.x8() {
            self.nz8(v as u8)
        } else {
            self.nz16(v)
        }
    }

    // ── stack ────────────────────────────────────────────────────────────────
    fn push8<B: CpuBus>(&mut self, b: &mut B, v: u8) {
        b.write(self.s as u32, v);
        self.s = if self.e {
            0x0100 | (self.s.wrapping_sub(1) & 0xFF)
        } else {
            self.s.wrapping_sub(1)
        };
    }
    fn pull8<B: CpuBus>(&mut self, b: &mut B) -> u8 {
        self.s = if self.e {
            0x0100 | (self.s.wrapping_add(1) & 0xFF)
        } else {
            self.s.wrapping_add(1)
        };
        b.read(self.s as u32)
    }
    fn push16<B: CpuBus>(&mut self, b: &mut B, v: u16) {
        self.push8(b, (v >> 8) as u8);
        self.push8(b, v as u8);
    }
    fn pull16<B: CpuBus>(&mut self, b: &mut B) -> u16 {
        let lo = self.pull8(b) as u16;
        let hi = self.pull8(b) as u16;
        (hi << 8) | lo
    }

    // ── memory helpers ───────────────────────────────────────────────────────
    /// `bank0`: the effective address lives in bank 0 and its +1 wraps at 64K.
    #[inline]
    fn rd<B: CpuBus>(&mut self, b: &mut B, addr: u32, bank0: bool, wide: bool) -> u16 {
        let lo = b.read(addr) as u16;
        if !wide {
            return lo;
        }
        let a2 = if bank0 { (addr.wrapping_add(1)) & 0xFFFF } else { addr.wrapping_add(1) & 0xFF_FFFF };
        let hi = b.read(a2) as u16;
        (hi << 8) | lo
    }
    #[inline]
    fn wr<B: CpuBus>(&mut self, b: &mut B, addr: u32, bank0: bool, wide: bool, v: u16) {
        b.write(addr, v as u8);
        if wide {
            let a2 =
                if bank0 { (addr.wrapping_add(1)) & 0xFFFF } else { addr.wrapping_add(1) & 0xFF_FFFF };
            b.write(a2, (v >> 8) as u8);
        }
    }
    /// 16-bit read within bank 0 (pointers).
    fn rd16_b0<B: CpuBus>(&mut self, b: &mut B, ptr: u16) -> u16 {
        let lo = b.read(ptr as u32) as u16;
        let hi = b.read(ptr.wrapping_add(1) as u32) as u16;
        (hi << 8) | lo
    }

    fn dp_penalty<B: CpuBus>(&mut self, b: &mut B) {
        if self.d & 0xFF != 0 {
            b.idle();
        }
    }

    /// Resolve an effective address (consuming operand bytes).
    fn ea<B: CpuBus>(&mut self, b: &mut B, mode: Mode, write: bool) -> (u32, bool) {
        match mode {
            Mode::Dp => {
                let d = self.fetch8(b) as u16;
                self.dp_penalty(b);
                (self.d.wrapping_add(d) as u32, true)
            },
            Mode::DpX | Mode::DpY => {
                let d = self.fetch8(b) as u16;
                self.dp_penalty(b);
                b.idle();
                let idx = if mode == Mode::DpX { self.x } else { self.y };
                let addr = if self.e && self.d & 0xFF == 0 {
                    self.d | ((d.wrapping_add(idx)) & 0xFF)
                } else {
                    self.d.wrapping_add(d).wrapping_add(idx)
                };
                (addr as u32, true)
            },
            Mode::Abs => {
                let a = self.fetch16(b) as u32;
                (((self.db as u32) << 16) | a, false)
            },
            Mode::AbsX | Mode::AbsY => {
                let a = self.fetch16(b) as u32;
                let base = ((self.db as u32) << 16) | a;
                let idx = if mode == Mode::AbsX { self.x } else { self.y } as u32;
                let addr = base.wrapping_add(idx) & 0xFF_FFFF;
                if write || !self.x8() || (base ^ addr) & 0xFF00 != 0 {
                    b.idle();
                }
                (addr, false)
            },
            Mode::Long => (self.fetch24(b), false),
            Mode::LongX => {
                let a = self.fetch24(b);
                (a.wrapping_add(self.x as u32) & 0xFF_FFFF, false)
            },
            Mode::DpInd => {
                let d = self.fetch8(b) as u16;
                self.dp_penalty(b);
                let ptr = self.d.wrapping_add(d);
                let v = self.rd16_b0(b, ptr) as u32;
                (((self.db as u32) << 16) | v, false)
            },
            Mode::DpXInd => {
                let d = self.fetch8(b) as u16;
                self.dp_penalty(b);
                b.idle();
                let ptr = if self.e && self.d & 0xFF == 0 {
                    self.d | ((d.wrapping_add(self.x)) & 0xFF)
                } else {
                    self.d.wrapping_add(d).wrapping_add(self.x)
                };
                let v = self.rd16_b0(b, ptr) as u32;
                (((self.db as u32) << 16) | v, false)
            },
            Mode::DpIndY => {
                let d = self.fetch8(b) as u16;
                self.dp_penalty(b);
                let ptr = self.d.wrapping_add(d);
                let v = self.rd16_b0(b, ptr) as u32;
                let base = ((self.db as u32) << 16) | v;
                let addr = base.wrapping_add(self.y as u32) & 0xFF_FFFF;
                if write || !self.x8() || (base ^ addr) & 0xFF00 != 0 {
                    b.idle();
                }
                (addr, false)
            },
            Mode::DpIndL | Mode::DpIndLY => {
                let d = self.fetch8(b) as u16;
                self.dp_penalty(b);
                let ptr = self.d.wrapping_add(d);
                let lo = self.rd16_b0(b, ptr) as u32;
                let hi = b.read(ptr.wrapping_add(2) as u32) as u32;
                let base = (hi << 16) | lo;
                if mode == Mode::DpIndLY {
                    (base.wrapping_add(self.y as u32) & 0xFF_FFFF, false)
                } else {
                    (base, false)
                }
            },
            Mode::Sr => {
                let d = self.fetch8(b) as u16;
                b.idle();
                (self.s.wrapping_add(d) as u32, true)
            },
            Mode::SrIndY => {
                let d = self.fetch8(b) as u16;
                b.idle();
                let ptr = self.s.wrapping_add(d);
                let v = self.rd16_b0(b, ptr) as u32;
                b.idle();
                let base = ((self.db as u32) << 16) | v;
                (base.wrapping_add(self.y as u32) & 0xFF_FFFF, false)
            },
        }
    }

    // ── arithmetic ───────────────────────────────────────────────────────────
    fn adc(&mut self, v: u16) {
        let decimal = self.p & D != 0;
        let c = (self.p & C) as u32;
        if self.m8() {
            let a = self.a as u32 & 0xFF;
            let v = v as u32 & 0xFF;
            let mut r;
            if !decimal {
                r = a + v + c;
            } else {
                r = (a & 0x0F) + (v & 0x0F) + c;
                if r > 0x09 {
                    r += 0x06;
                }
                let cc = (r > 0x0F) as u32;
                r = (a & 0xF0) + (v & 0xF0) + (r & 0x0F) + cc * 0x10;
            }
            self.set(V, (!(a ^ v) & (a ^ r) & 0x80) != 0);
            if decimal && r > 0x9F {
                r += 0x60;
            }
            self.set(C, r > 0xFF);
            self.a = (self.a & 0xFF00) | (r as u16 & 0xFF);
            self.nz8(r as u8);
        } else {
            let a = self.a as u32;
            let v = v as u32;
            let mut r;
            if !decimal {
                r = a + v + c;
            } else {
                r = (a & 0x000F) + (v & 0x000F) + c;
                if r > 0x0009 {
                    r += 0x0006;
                }
                let mut cc = (r > 0x000F) as u32;
                r = (a & 0x00F0) + (v & 0x00F0) + (r & 0x000F) + cc * 0x10;
                if r > 0x009F {
                    r += 0x0060;
                }
                cc = (r > 0x00FF) as u32;
                r = (a & 0x0F00) + (v & 0x0F00) + (r & 0x00FF) + cc * 0x100;
                if r > 0x09FF {
                    r += 0x0600;
                }
                cc = (r > 0x0FFF) as u32;
                r = (a & 0xF000) + (v & 0xF000) + (r & 0x0FFF) + cc * 0x1000;
            }
            self.set(V, (!(a ^ v) & (a ^ r) & 0x8000) != 0);
            if decimal && r > 0x9FFF {
                r += 0x6000;
            }
            self.set(C, r > 0xFFFF);
            self.a = r as u16;
            self.nz16(r as u16);
        }
    }

    fn sbc(&mut self, v: u16) {
        let decimal = self.p & D != 0;
        let c = (self.p & C) as i32;
        if self.m8() {
            let a = self.a as i32 & 0xFF;
            let v = (!v) as i32 & 0xFF;
            let mut r;
            if !decimal {
                r = a + v + c;
            } else {
                r = (a & 0x0F) + (v & 0x0F) + c;
                if r <= 0x0F {
                    r -= 0x06;
                }
                let cc = (r > 0x0F) as i32;
                r = (a & 0xF0) + (v & 0xF0) + (r & 0x0F) + cc * 0x10;
            }
            self.set(V, (!(a ^ v) & (a ^ r) & 0x80) != 0);
            if decimal && r <= 0xFF {
                r -= 0x60;
            }
            self.set(C, r > 0xFF);
            self.a = (self.a & 0xFF00) | (r as u16 & 0xFF);
            self.nz8(r as u8);
        } else {
            let a = self.a as i32;
            let v = (!v) as i32 & 0xFFFF;
            let mut r;
            if !decimal {
                r = a + v + c;
            } else {
                r = (a & 0x000F) + (v & 0x000F) + c;
                if r <= 0x000F {
                    r -= 0x0006;
                }
                let mut cc = (r > 0x000F) as i32;
                r = (a & 0x00F0) + (v & 0x00F0) + (r & 0x000F) + cc * 0x10;
                if r <= 0x00FF {
                    r -= 0x0060;
                }
                cc = (r > 0x00FF) as i32;
                r = (a & 0x0F00) + (v & 0x0F00) + (r & 0x00FF) + cc * 0x100;
                if r <= 0x0FFF {
                    r -= 0x0600;
                }
                cc = (r > 0x0FFF) as i32;
                r = (a & 0xF000) + (v & 0xF000) + (r & 0x0FFF) + cc * 0x1000;
            }
            self.set(V, (!(a ^ v) & (a ^ r) & 0x8000) != 0);
            if decimal && r <= 0xFFFF {
                r -= 0x6000;
            }
            self.set(C, r > 0xFFFF);
            self.a = r as u16;
            self.nz16(r as u16);
        }
    }

    fn cmp_reg(&mut self, reg: u16, v: u16, wide_flag_8: bool) {
        if wide_flag_8 {
            let r = (reg as u8).wrapping_sub(v as u8);
            self.set(C, (reg as u8) >= (v as u8));
            self.nz8(r);
        } else {
            let r = reg.wrapping_sub(v);
            self.set(C, reg >= v);
            self.nz16(r);
        }
    }

    /// Apply one of ORA/AND/EOR/ADC/CMP/SBC with operand `v`.
    fn alu(&mut self, kind: u8, v: u16) {
        let m8 = self.m8();
        match kind {
            0 => {
                if m8 {
                    let r = (self.a as u8) | v as u8;
                    self.a = (self.a & 0xFF00) | r as u16;
                    self.nz8(r);
                } else {
                    self.a |= v;
                    self.nz16(self.a);
                }
            },
            1 => {
                if m8 {
                    let r = (self.a as u8) & v as u8;
                    self.a = (self.a & 0xFF00) | r as u16;
                    self.nz8(r);
                } else {
                    self.a &= v;
                    self.nz16(self.a);
                }
            },
            2 => {
                if m8 {
                    let r = (self.a as u8) ^ v as u8;
                    self.a = (self.a & 0xFF00) | r as u16;
                    self.nz8(r);
                } else {
                    self.a ^= v;
                    self.nz16(self.a);
                }
            },
            3 => self.adc(v),
            6 => self.cmp_reg(self.a, v, m8),
            7 => self.sbc(v),
            _ => {},
        }
    }

    // ── shifts / rotates / inc / dec on a value of width M ───────────────────
    fn shift(&mut self, kind: u8, v: u16) -> u16 {
        let m8 = self.m8();
        let (top, mask) = if m8 { (0x80u32, 0xFFu32) } else { (0x8000u32, 0xFFFFu32) };
        let v = v as u32;
        let cin = (self.p & C) as u32;
        let r = match kind {
            0 => {
                // ASL
                self.set(C, v & top != 0);
                (v << 1) & mask
            },
            1 => {
                // ROL
                self.set(C, v & top != 0);
                ((v << 1) | cin) & mask
            },
            2 => {
                // LSR
                self.set(C, v & 1 != 0);
                v >> 1
            },
            _ => {
                // ROR
                self.set(C, v & 1 != 0);
                (v >> 1) | if cin != 0 { top } else { 0 }
            },
        };
        self.nz_m(r as u16);
        r as u16
    }

    // ── interrupts ───────────────────────────────────────────────────────────
    fn interrupt<B: CpuBus>(&mut self, b: &mut B, vec_native: u16, vec_emu: u16, brk: bool) {
        b.idle();
        if !self.e {
            let pb = self.pb;
            self.push8(b, pb);
        }
        let pc = self.pc;
        self.push16(b, pc);
        let mut p = self.p;
        if self.e {
            p = (p & !0x10) | if brk { 0x10 } else { 0 };
            p |= 0x20;
        }
        self.push8(b, p);
        self.set(I, true);
        self.set(D, false);
        self.pb = 0;
        let vec = if self.e { vec_emu } else { vec_native } as u32;
        let lo = b.read(vec) as u16;
        let hi = b.read(vec + 1) as u16;
        self.pc = (hi << 8) | lo;
    }

    // ── one step ─────────────────────────────────────────────────────────────
    pub fn step<B: CpuBus>(&mut self, b: &mut B) {
        if self.stopped {
            b.idle();
            return;
        }
        if b.take_nmi() {
            self.waiting = false;
            self.interrupt(b, 0xFFEA, 0xFFFA, false);
            return;
        }
        let irq = b.irq();
        if self.waiting {
            if irq {
                self.waiting = false;
                if self.p & I == 0 {
                    self.interrupt(b, 0xFFEE, 0xFFFE, false);
                    return;
                }
            } else {
                b.idle();
                return;
            }
        } else if irq && self.p & I == 0 {
            self.interrupt(b, 0xFFEE, 0xFFFE, false);
            return;
        }

        let op = self.fetch8(b);
        self.exec(b, op);
    }

    fn implied<B: CpuBus>(&mut self, b: &mut B) {
        b.cycle_at(((self.pb as u32) << 16) | self.pc as u32);
    }

    fn branch<B: CpuBus>(&mut self, b: &mut B, cond: bool) {
        let off = self.fetch8(b) as i8;
        if cond {
            b.idle();
            let new = self.pc.wrapping_add(off as i16 as u16);
            if self.e && (new ^ self.pc) & 0xFF00 != 0 {
                b.idle();
            }
            self.pc = new;
        }
    }

    fn exec<B: CpuBus>(&mut self, b: &mut B, op: u8) {
        // ── ALU group: ORA AND EOR ADC STA LDA CMP SBC ───────────────────────
        let cc = op & 3;
        let bbb = (op >> 2) & 7;
        let aaa = op >> 5;
        let is_alu = (cc == 1 && op != 0x89)
            || (cc == 3 && bbb != 2 && bbb != 6)
            || (op & 0x1F) == 0x12;
        if is_alu {
            let mode = match (cc, bbb) {
                (1, 0) => Mode::DpXInd,
                (1, 1) => Mode::Dp,
                (1, 3) => Mode::Abs,
                (1, 4) => Mode::DpIndY,
                (1, 5) => Mode::DpX,
                (1, 6) => Mode::AbsY,
                (1, 7) => Mode::AbsX,
                (3, 0) => Mode::Sr,
                (3, 1) => Mode::DpIndL,
                (3, 3) => Mode::Long,
                (3, 4) => Mode::SrIndY,
                (3, 5) => Mode::DpIndLY,
                (3, 7) => Mode::LongX,
                (2, 4) => Mode::DpInd,
                _ => Mode::Dp, // (1,2) immediate handled below
            };
            let imm = cc == 1 && bbb == 2;
            let wide = !self.m8();
            match aaa {
                4 => {
                    // STA
                    let (addr, b0) = self.ea(b, mode, true);
                    let a = self.a;
                    self.wr(b, addr, b0, wide, a);
                },
                5 => {
                    // LDA
                    let v = if imm {
                        if wide {
                            self.fetch16(b)
                        } else {
                            self.fetch8(b) as u16
                        }
                    } else {
                        let (addr, b0) = self.ea(b, mode, false);
                        self.rd(b, addr, b0, wide)
                    };
                    if wide {
                        self.a = v;
                        self.nz16(v);
                    } else {
                        self.a = (self.a & 0xFF00) | (v & 0xFF);
                        self.nz8(v as u8);
                    }
                },
                _ => {
                    let v = if imm {
                        if wide {
                            self.fetch16(b)
                        } else {
                            self.fetch8(b) as u16
                        }
                    } else {
                        let (addr, b0) = self.ea(b, mode, false);
                        self.rd(b, addr, b0, wide)
                    };
                    self.alu(aaa, v);
                },
            }
            return;
        }

        // ── RMW shifts / INC / DEC on A or memory ────────────────────────────
        if cc == 2 && matches!(aaa, 0..=3) && matches!(bbb, 1 | 2 | 3 | 5 | 7) {
            if bbb == 2 {
                self.implied(b);
                let a = self.a;
                let r = self.shift(aaa, if self.m8() { a & 0xFF } else { a });
                if self.m8() {
                    self.a = (self.a & 0xFF00) | (r & 0xFF);
                } else {
                    self.a = r;
                }
            } else {
                let mode = match bbb {
                    1 => Mode::Dp,
                    3 => Mode::Abs,
                    5 => Mode::DpX,
                    _ => Mode::AbsX,
                };
                let wide = !self.m8();
                let (addr, b0) = self.ea(b, mode, true);
                let v = self.rd(b, addr, b0, wide);
                b.idle();
                let r = self.shift(aaa, v);
                self.wr_rmw(b, addr, b0, wide, r);
            }
            return;
        }
        if cc == 2 && matches!(aaa, 6 | 7) && matches!(bbb, 1 | 3 | 5 | 7) {
            let mode = match bbb {
                1 => Mode::Dp,
                3 => Mode::Abs,
                5 => Mode::DpX,
                _ => Mode::AbsX,
            };
            let wide = !self.m8();
            let (addr, b0) = self.ea(b, mode, true);
            let v = self.rd(b, addr, b0, wide);
            b.idle();
            let r = if aaa == 6 { v.wrapping_sub(1) } else { v.wrapping_add(1) };
            let r = if wide { r } else { r & 0xFF };
            self.nz_m(r);
            self.wr_rmw(b, addr, b0, wide, r);
            return;
        }

        match op {
            // ── control ──────────────────────────────────────────────────────
            0x00 | 0x02 => {
                // BRK / COP
                let _sig = self.fetch8(b);
                if op == 0x00 {
                    self.interrupt(b, 0xFFE6, 0xFFFE, true);
                } else {
                    self.interrupt(b, 0xFFE4, 0xFFF4, false);
                }
            },
            0x40 => {
                // RTI
                self.implied(b);
                b.idle();
                let p = self.pull8(b);
                self.p = p;
                if self.e {
                    self.p |= M | X;
                }
                self.pc = self.pull16(b);
                if !self.e {
                    self.pb = self.pull8(b);
                }
                self.fix_index_width();
            },
            0x60 => {
                // RTS
                self.implied(b);
                b.idle();
                let pc = self.pull16(b);
                b.idle();
                self.pc = pc.wrapping_add(1);
            },
            0x6B => {
                // RTL
                self.implied(b);
                b.idle();
                let pc = self.pull16(b);
                self.pb = self.pull8(b);
                self.pc = pc.wrapping_add(1);
            },
            0x20 => {
                // JSR abs
                let a = self.fetch16(b);
                b.idle();
                let ret = self.pc.wrapping_sub(1);
                self.push16(b, ret);
                self.pc = a;
            },
            0x22 => {
                // JSL long
                let lo = self.fetch16(b);
                let pb = self.pb;
                self.push8(b, pb);
                b.idle();
                let bank = self.fetch8(b);
                let ret = self.pc.wrapping_sub(1);
                self.push16(b, ret);
                self.pc = lo;
                self.pb = bank;
            },
            0xFC => {
                // JSR (abs,x)
                let lo = self.fetch8(b);
                let ret = self.pc;
                self.push16(b, ret);
                let hi = self.fetch8(b);
                b.idle();
                let ptr = (((hi as u16) << 8) | lo as u16).wrapping_add(self.x);
                let base = (self.pb as u32) << 16;
                let l = b.read(base | ptr as u32) as u16;
                let h = b.read(base | ptr.wrapping_add(1) as u32) as u16;
                self.pc = (h << 8) | l;
            },
            0x4C => self.pc = self.fetch16(b),
            0x5C => {
                let lo = self.fetch16(b);
                let bank = self.fetch8(b);
                self.pc = lo;
                self.pb = bank;
            },
            0x6C => {
                let ptr = self.fetch16(b);
                self.pc = self.rd16_b0(b, ptr);
            },
            0x7C => {
                let a = self.fetch16(b);
                b.idle();
                let ptr = a.wrapping_add(self.x);
                let base = (self.pb as u32) << 16;
                let l = b.read(base | ptr as u32) as u16;
                let h = b.read(base | ptr.wrapping_add(1) as u32) as u16;
                self.pc = (h << 8) | l;
            },
            0xDC => {
                let ptr = self.fetch16(b);
                let lo = self.rd16_b0(b, ptr);
                let bank = b.read(ptr.wrapping_add(2) as u32);
                self.pc = lo;
                self.pb = bank;
            },
            // ── branches ─────────────────────────────────────────────────────
            0x10 => {
                let c = self.p & N == 0;
                self.branch(b, c)
            },
            0x30 => {
                let c = self.p & N != 0;
                self.branch(b, c)
            },
            0x50 => {
                let c = self.p & V == 0;
                self.branch(b, c)
            },
            0x70 => {
                let c = self.p & V != 0;
                self.branch(b, c)
            },
            0x80 => self.branch(b, true),
            0x90 => {
                let c = self.p & C == 0;
                self.branch(b, c)
            },
            0xB0 => {
                let c = self.p & C != 0;
                self.branch(b, c)
            },
            0xD0 => {
                let c = self.p & Z == 0;
                self.branch(b, c)
            },
            0xF0 => {
                let c = self.p & Z != 0;
                self.branch(b, c)
            },
            0x82 => {
                let off = self.fetch16(b);
                b.idle();
                self.pc = self.pc.wrapping_add(off);
            },
            // ── flag ops ─────────────────────────────────────────────────────
            0x18 => {
                self.implied(b);
                self.set(C, false)
            },
            0x38 => {
                self.implied(b);
                self.set(C, true)
            },
            0x58 => {
                self.implied(b);
                self.set(I, false)
            },
            0x78 => {
                self.implied(b);
                self.set(I, true)
            },
            0xB8 => {
                self.implied(b);
                self.set(V, false)
            },
            0xD8 => {
                self.implied(b);
                self.set(D, false)
            },
            0xF8 => {
                self.implied(b);
                self.set(D, true)
            },
            0xC2 => {
                // REP
                let m = self.fetch8(b);
                b.idle();
                self.p &= !m;
                if self.e {
                    self.p |= M | X;
                }
            },
            0xE2 => {
                // SEP
                let m = self.fetch8(b);
                b.idle();
                self.p |= m;
                if self.e {
                    self.p |= M | X;
                }
                self.fix_index_width();
            },
            0xFB => {
                // XCE
                self.implied(b);
                let c = self.p & C != 0;
                let e = self.e;
                self.set(C, e);
                self.e = c;
                if self.e {
                    self.p |= M | X;
                    self.x &= 0xFF;
                    self.y &= 0xFF;
                    self.s = 0x0100 | (self.s & 0xFF);
                }
            },
            0xEB => {
                // XBA
                self.implied(b);
                b.idle();
                self.a = self.a.rotate_left(8);
                self.nz8(self.a as u8);
            },
            0xEA => self.implied(b),
            0x42 => {
                let _ = self.fetch8(b); // WDM
            },
            0xCB => {
                self.implied(b);
                b.idle();
                self.waiting = true;
            },
            0xDB => {
                self.implied(b);
                b.idle();
                self.stopped = true;
            },
            // ── register inc/dec ─────────────────────────────────────────────
            0x1A | 0x3A => {
                self.implied(b);
                let wide = !self.m8();
                let r = if op == 0x1A { self.a.wrapping_add(1) } else { self.a.wrapping_sub(1) };
                if wide {
                    self.a = r;
                    self.nz16(r);
                } else {
                    self.a = (self.a & 0xFF00) | (r & 0xFF);
                    self.nz8(r as u8);
                }
            },
            0xE8 | 0xC8 | 0xCA | 0x88 => {
                self.implied(b);
                let x8 = self.x8();
                let (reg, inc) = match op {
                    0xE8 => (self.x, true),
                    0xC8 => (self.y, true),
                    0xCA => (self.x, false),
                    _ => (self.y, false),
                };
                let mut r = if inc { reg.wrapping_add(1) } else { reg.wrapping_sub(1) };
                if x8 {
                    r &= 0xFF;
                }
                self.nz_x(r);
                if op == 0xE8 || op == 0xCA {
                    self.x = r;
                } else {
                    self.y = r;
                }
            },
            // ── loads / stores of X, Y, Z ────────────────────────────────────
            0xA2 | 0xA6 | 0xAE | 0xB6 | 0xBE => {
                let mode = match op {
                    0xA6 => Mode::Dp,
                    0xAE => Mode::Abs,
                    0xB6 => Mode::DpY,
                    _ => Mode::AbsY,
                };
                let wide = !self.x8();
                let v = if op == 0xA2 {
                    if wide {
                        self.fetch16(b)
                    } else {
                        self.fetch8(b) as u16
                    }
                } else {
                    let (addr, b0) = self.ea(b, mode, false);
                    self.rd(b, addr, b0, wide)
                };
                self.x = v;
                self.nz_x(v);
            },
            0xA0 | 0xA4 | 0xAC | 0xB4 | 0xBC => {
                let mode = match op {
                    0xA4 => Mode::Dp,
                    0xAC => Mode::Abs,
                    0xB4 => Mode::DpX,
                    _ => Mode::AbsX,
                };
                let wide = !self.x8();
                let v = if op == 0xA0 {
                    if wide {
                        self.fetch16(b)
                    } else {
                        self.fetch8(b) as u16
                    }
                } else {
                    let (addr, b0) = self.ea(b, mode, false);
                    self.rd(b, addr, b0, wide)
                };
                self.y = v;
                self.nz_x(v);
            },
            0x86 | 0x8E | 0x96 => {
                let mode = match op {
                    0x86 => Mode::Dp,
                    0x8E => Mode::Abs,
                    _ => Mode::DpY,
                };
                let wide = !self.x8();
                let (addr, b0) = self.ea(b, mode, true);
                let x = self.x;
                self.wr(b, addr, b0, wide, x);
            },
            0x84 | 0x8C | 0x94 => {
                let mode = match op {
                    0x84 => Mode::Dp,
                    0x8C => Mode::Abs,
                    _ => Mode::DpX,
                };
                let wide = !self.x8();
                let (addr, b0) = self.ea(b, mode, true);
                let y = self.y;
                self.wr(b, addr, b0, wide, y);
            },
            0x64 | 0x74 | 0x9C | 0x9E => {
                let mode = match op {
                    0x64 => Mode::Dp,
                    0x74 => Mode::DpX,
                    0x9C => Mode::Abs,
                    _ => Mode::AbsX,
                };
                let wide = !self.m8();
                let (addr, b0) = self.ea(b, mode, true);
                self.wr(b, addr, b0, wide, 0);
            },
            // ── compares of X / Y ────────────────────────────────────────────
            0xE0 | 0xE4 | 0xEC | 0xC0 | 0xC4 | 0xCC => {
                let is_x = op & 0x20 != 0;
                let wide = !self.x8();
                let v = if op == 0xE0 || op == 0xC0 {
                    if wide {
                        self.fetch16(b)
                    } else {
                        self.fetch8(b) as u16
                    }
                } else {
                    let mode = if op & 0x0C == 0x04 { Mode::Dp } else { Mode::Abs };
                    let (addr, b0) = self.ea(b, mode, false);
                    self.rd(b, addr, b0, wide)
                };
                let reg = if is_x { self.x } else { self.y };
                self.cmp_reg(reg, v, !wide);
            },
            // ── BIT / TSB / TRB ──────────────────────────────────────────────
            0x89 => {
                let wide = !self.m8();
                let v = if wide { self.fetch16(b) } else { self.fetch8(b) as u16 };
                let r = if wide { self.a & v } else { (self.a & v) & 0xFF };
                self.set(Z, r == 0);
            },
            0x24 | 0x2C | 0x34 | 0x3C => {
                let mode = match op {
                    0x24 => Mode::Dp,
                    0x2C => Mode::Abs,
                    0x34 => Mode::DpX,
                    _ => Mode::AbsX,
                };
                let wide = !self.m8();
                let (addr, b0) = self.ea(b, mode, false);
                let v = self.rd(b, addr, b0, wide);
                let r = if wide { self.a & v } else { (self.a & v) & 0xFF };
                self.set(Z, r == 0);
                if wide {
                    self.set(N, v & 0x8000 != 0);
                    self.set(V, v & 0x4000 != 0);
                } else {
                    self.set(N, v & 0x80 != 0);
                    self.set(V, v & 0x40 != 0);
                }
            },
            0x04 | 0x0C | 0x14 | 0x1C => {
                let mode = if op & 0x08 == 0 { Mode::Dp } else { Mode::Abs };
                let wide = !self.m8();
                let (addr, b0) = self.ea(b, mode, true);
                let v = self.rd(b, addr, b0, wide);
                b.idle();
                let a = if wide { self.a } else { self.a & 0xFF };
                self.set(Z, (a & v) == 0);
                let r = if op & 0x10 == 0 { v | a } else { v & !a };
                self.wr_rmw(b, addr, b0, wide, r);
            },
            // ── transfers ────────────────────────────────────────────────────
            0xAA => {
                self.implied(b);
                self.x = if self.x8() { self.a & 0xFF } else { self.a };
                self.nz_x(self.x);
            },
            0xA8 => {
                self.implied(b);
                self.y = if self.x8() { self.a & 0xFF } else { self.a };
                self.nz_x(self.y);
            },
            0x8A => {
                self.implied(b);
                if self.m8() {
                    self.a = (self.a & 0xFF00) | (self.x & 0xFF);
                    self.nz8(self.a as u8);
                } else {
                    self.a = self.x;
                    self.nz16(self.a);
                }
            },
            0x98 => {
                self.implied(b);
                if self.m8() {
                    self.a = (self.a & 0xFF00) | (self.y & 0xFF);
                    self.nz8(self.a as u8);
                } else {
                    self.a = self.y;
                    self.nz16(self.a);
                }
            },
            0x9A => {
                self.implied(b);
                self.s = if self.e { 0x0100 | (self.x & 0xFF) } else { self.x };
            },
            0xBA => {
                self.implied(b);
                self.x = if self.x8() { self.s & 0xFF } else { self.s };
                self.nz_x(self.x);
            },
            0x9B => {
                self.implied(b);
                self.y = if self.x8() { self.x & 0xFF } else { self.x };
                self.nz_x(self.y);
            },
            0xBB => {
                self.implied(b);
                self.x = if self.x8() { self.y & 0xFF } else { self.y };
                self.nz_x(self.x);
            },
            0x5B => {
                self.implied(b);
                self.d = self.a;
                self.nz16(self.d);
            },
            0x7B => {
                self.implied(b);
                self.a = self.d;
                self.nz16(self.a);
            },
            0x1B => {
                self.implied(b);
                self.s = if self.e { 0x0100 | (self.a & 0xFF) } else { self.a };
            },
            0x3B => {
                self.implied(b);
                self.a = self.s;
                self.nz16(self.a);
            },
            // ── stack pushes / pulls ─────────────────────────────────────────
            0x48 => {
                self.implied(b);
                if self.m8() {
                    let a = self.a as u8;
                    self.push8(b, a);
                } else {
                    let a = self.a;
                    self.push16(b, a);
                }
            },
            0x68 => {
                self.implied(b);
                b.idle();
                if self.m8() {
                    let v = self.pull8(b) as u16;
                    self.a = (self.a & 0xFF00) | v;
                    self.nz8(v as u8);
                } else {
                    let v = self.pull16(b);
                    self.a = v;
                    self.nz16(v);
                }
            },
            0xDA | 0x5A => {
                self.implied(b);
                let v = if op == 0xDA { self.x } else { self.y };
                if self.x8() {
                    self.push8(b, v as u8);
                } else {
                    self.push16(b, v);
                }
            },
            0xFA | 0x7A => {
                self.implied(b);
                b.idle();
                let v = if self.x8() { self.pull8(b) as u16 } else { self.pull16(b) };
                self.nz_x(v);
                if op == 0xFA {
                    self.x = v;
                } else {
                    self.y = v;
                }
            },
            0x08 => {
                self.implied(b);
                let p = if self.e { self.p | 0x30 } else { self.p };
                self.push8(b, p);
            },
            0x28 => {
                self.implied(b);
                b.idle();
                let p = self.pull8(b);
                self.p = p;
                if self.e {
                    self.p |= M | X;
                }
                self.fix_index_width();
            },
            0x8B => {
                self.implied(b);
                let db = self.db;
                self.push8(b, db);
            },
            0xAB => {
                self.implied(b);
                b.idle();
                self.db = self.pull8(b);
                self.nz8(self.db);
            },
            0x0B => {
                self.implied(b);
                let d = self.d;
                self.push16(b, d);
            },
            0x2B => {
                self.implied(b);
                b.idle();
                self.d = self.pull16(b);
                self.nz16(self.d);
            },
            0x4B => {
                self.implied(b);
                let pb = self.pb;
                self.push8(b, pb);
            },
            0xF4 => {
                // PEA
                let v = self.fetch16(b);
                self.push16(b, v);
            },
            0xD4 => {
                // PEI
                let d = self.fetch8(b) as u16;
                self.dp_penalty(b);
                let ptr = self.d.wrapping_add(d);
                let v = self.rd16_b0(b, ptr);
                self.push16(b, v);
            },
            0x62 => {
                // PER
                let off = self.fetch16(b);
                b.idle();
                let v = self.pc.wrapping_add(off);
                self.push16(b, v);
            },
            // ── block moves ──────────────────────────────────────────────────
            0x44 | 0x54 => {
                let dst = self.fetch8(b);
                let src = self.fetch8(b);
                self.db = dst;
                let v = b.read(((src as u32) << 16) | self.x as u32);
                b.write(((dst as u32) << 16) | self.y as u32, v);
                b.idle();
                b.idle();
                let step: u16 = if op == 0x54 { 1 } else { 0xFFFF };
                self.x = self.x.wrapping_add(step);
                self.y = self.y.wrapping_add(step);
                if self.x8() {
                    self.x &= 0xFF;
                    self.y &= 0xFF;
                }
                self.a = self.a.wrapping_sub(1);
                if self.a != 0xFFFF {
                    self.pc = self.pc.wrapping_sub(3);
                }
            },
            other => {
                // every opcode is handled above; keep the CPU alive regardless
                let _ = other;
            },
        }
    }

    /// RMW writeback: 16-bit writes high byte first.
    fn wr_rmw<B: CpuBus>(&mut self, b: &mut B, addr: u32, bank0: bool, wide: bool, v: u16) {
        if wide {
            let a2 =
                if bank0 { (addr.wrapping_add(1)) & 0xFFFF } else { addr.wrapping_add(1) & 0xFF_FFFF };
            b.write(a2, (v >> 8) as u8);
        }
        b.write(addr, v as u8);
    }

    fn fix_index_width(&mut self) {
        if self.p & X != 0 || self.e {
            self.x &= 0xFF;
            self.y &= 0xFF;
        }
    }
}
