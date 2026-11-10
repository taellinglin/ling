//! MIPS R4300i interpreter. Interpreter-first by design: a cranelift dynarec
//! can replace `run` block-wise later (the CPU was never the N64 wall —
//! graphics HLE is), but a clean interpreter is the correctness baseline.

use super::mem::Bus;

// CP0 register indices
const CP0_INDEX: usize = 0;
const CP0_ENTRYLO0: usize = 2;
const CP0_ENTRYLO1: usize = 3;
const CP0_PAGEMASK: usize = 5;
const CP0_BADVADDR: usize = 8;
const CP0_COUNT: usize = 9;
const CP0_ENTRYHI: usize = 10;
const CP0_COMPARE: usize = 11;
const CP0_STATUS: usize = 12;
const CP0_CAUSE: usize = 13;
const CP0_EPC: usize = 14;
const CP0_PRID: usize = 15;

#[derive(Clone, Copy, Default)]
struct TlbEntry {
    hi: u64,
    lo0: u64,
    lo1: u64,
    mask: u32,
}

pub struct Cpu {
    pub gpr: [u64; 32],
    pub fgr: [u64; 32],
    pub hi: u64,
    pub lo: u64,
    pub pc: u32,
    next_pc: u32,
    pub cp0: [u64; 32],
    pub fcr31: u32,
    tlb: [TlbEntry; 32],
    ll_bit: bool,
    /// Set when the core hits something it can't honestly run.
    pub stuck: Option<String>,
    pub exc_count: u64,
    pub eret_count: u64,
    idle_loop: bool,
    pub idle_cycles: u64,
}

impl Cpu {
    pub fn new() -> Cpu {
        let mut cp0 = [0u64; 32];
        cp0[CP0_STATUS] = 0x3400_0000; // CU0|CU1|FR=0? (bit26 FR off), kernel
        cp0[CP0_PRID] = 0x0B22;
        Cpu {
            gpr: [0; 32],
            fgr: [0; 32],
            hi: 0,
            lo: 0,
            pc: 0,
            next_pc: 4,
            cp0,
            fcr31: 0,
            tlb: [TlbEntry::default(); 32],
            ll_bit: false,
            stuck: None,
            exc_count: 0,
            eret_count: 0,
            idle_loop: false,
            idle_cycles: 0,
        }
    }

    pub fn set_pc(&mut self, pc: u32) {
        self.pc = pc;
        self.next_pc = pc.wrapping_add(4);
    }

    #[inline]
    fn fr_bit(&self) -> bool {
        self.cp0[CP0_STATUS] & (1 << 26) != 0
    }

    // ── virtual → physical ───────────────────────────────────────────────────
    fn translate(&mut self, vaddr: u32) -> Option<u32> {
        match vaddr {
            0x8000_0000..=0x9FFF_FFFF => Some(vaddr & 0x1FFF_FFFF),
            0xA000_0000..=0xBFFF_FFFF => Some(vaddr & 0x1FFF_FFFF),
            _ => {
                // TLB-mapped (KUSEG / KSEG2) — OoT maps code through here.
                let v = vaddr as u64;
                for e in &self.tlb {
                    let mask = !(e.mask as u64 | 0x1FFF);
                    if (v & mask) == (e.hi & mask) {
                        let page = (e.mask as u64 >> 1) + 0xFFF; // odd/even page span - 1
                        let even = (v & (page + 1)) == 0;
                        let lo = if even { e.lo0 } else { e.lo1 };
                        if lo & 2 == 0 {
                            return None; // invalid
                        }
                        let pfn = ((lo >> 6) & 0xFF_FFFF) as u32;
                        return Some((pfn << 12) | (vaddr & (page as u32)));
                    }
                }
                None
            },
        }
    }

    fn read_v32(&mut self, bus: &mut Bus, vaddr: u32) -> u32 {
        if vaddr & 0xC000_0000 == 0x8000_0000 {
            let p = (vaddr & 0x1FFF_FFFF) as usize;
            if p + 3 < bus.rdram.len() {
                let r = &bus.rdram;
                return u32::from_be_bytes([r[p], r[p + 1], r[p + 2], r[p + 3]]);
            }
        }
        match self.translate(vaddr) {
            Some(p) => bus.read32(p),
            None => {
                self.tlb_miss(vaddr, false);
                0
            },
        }
    }

    fn read_v8(&mut self, bus: &mut Bus, vaddr: u32) -> u8 {
        if vaddr & 0xC000_0000 == 0x8000_0000 {
            let p = (vaddr & 0x1FFF_FFFF) as usize;
            if p < bus.rdram.len() {
                return bus.rdram[p];
            }
        }
        match self.translate(vaddr) {
            Some(p) => bus.read8(p),
            None => {
                self.tlb_miss(vaddr, false);
                0
            },
        }
    }

    fn write_v32(&mut self, bus: &mut Bus, vaddr: u32, v: u32) {
        if vaddr & 0xC000_0000 == 0x8000_0000 {
            let p = (vaddr & 0x1FFF_FFFF) as usize;
            if p + 3 < bus.rdram.len() {
                bus.rdram[p..p + 4].copy_from_slice(&v.to_be_bytes());
                return;
            }
        }
        match self.translate(vaddr) {
            Some(p) => bus.write32(p, v),
            None => self.tlb_miss(vaddr, true),
        }
    }

    fn write_v8(&mut self, bus: &mut Bus, vaddr: u32, v: u8) {
        if vaddr & 0xC000_0000 == 0x8000_0000 {
            let p = (vaddr & 0x1FFF_FFFF) as usize;
            if p < bus.rdram.len() {
                bus.rdram[p] = v;
                return;
            }
        }
        match self.translate(vaddr) {
            Some(p) => bus.write8(p, v),
            None => self.tlb_miss(vaddr, true),
        }
    }

    fn read_v16(&mut self, bus: &mut Bus, vaddr: u32) -> u16 {
        ((self.read_v8(bus, vaddr) as u16) << 8) | self.read_v8(bus, vaddr.wrapping_add(1)) as u16
    }

    fn write_v16(&mut self, bus: &mut Bus, vaddr: u32, v: u16) {
        self.write_v8(bus, vaddr, (v >> 8) as u8);
        self.write_v8(bus, vaddr.wrapping_add(1), v as u8);
    }

    fn read_v64(&mut self, bus: &mut Bus, vaddr: u32) -> u64 {
        ((self.read_v32(bus, vaddr) as u64) << 32) | self.read_v32(bus, vaddr.wrapping_add(4)) as u64
    }

    fn write_v64(&mut self, bus: &mut Bus, vaddr: u32, v: u64) {
        self.write_v32(bus, vaddr, (v >> 32) as u32);
        self.write_v32(bus, vaddr.wrapping_add(4), v as u32);
    }

    // ── exceptions ───────────────────────────────────────────────────────────
    fn exception(&mut self, code: u32, in_delay: bool) {
        let status = self.cp0[CP0_STATUS];
        if status & 2 == 0 {
            // not already EXL
            let epc = if in_delay { self.pc.wrapping_sub(4) } else { self.pc };
            self.cp0[CP0_EPC] = epc as i32 as u64;
            let mut cause = self.cp0[CP0_CAUSE] & !0xFF;
            cause |= (code << 2) as u64;
            if in_delay {
                cause |= 1 << 31;
            }
            self.cp0[CP0_CAUSE] = cause;
        }
        self.cp0[CP0_STATUS] |= 2; // EXL
        self.exc_count += 1;
        self.set_pc(0x8000_0180);
    }

    fn tlb_miss(&mut self, vaddr: u32, write: bool) {
        self.cp0[CP0_BADVADDR] = vaddr as i32 as u64;
        self.cp0[CP0_ENTRYHI] =
            (self.cp0[CP0_ENTRYHI] & 0x1FFF) | (vaddr as u64 & 0xFFFF_E000);
        // simplified: general vector (real chip uses 0x80000000 for refill)
        self.exception(if write { 3 } else { 2 }, false);
    }

    fn check_interrupts(&mut self, bus: &Bus) -> bool {
        // wire MI into IP2
        let mut cause = self.cp0[CP0_CAUSE];
        if bus.irq_asserted() {
            cause |= 1 << 10;
        } else {
            cause &= !(1 << 10);
        }
        self.cp0[CP0_CAUSE] = cause;
        let status = self.cp0[CP0_STATUS];
        let ie = status & 1 != 0 && status & 6 == 0; // IE set, not EXL/ERL
        if ie {
            let pending = ((cause >> 8) & 0xFF) as u32 & ((status >> 8) & 0xFF) as u32;
            if pending != 0 {
                self.exception(0, false);
                return true;
            }
        }
        false
    }

    // ── FPU register access (honours Status.FR) ──────────────────────────────
    fn fd(&self, r: usize) -> f64 {
        if self.fr_bit() {
            f64::from_bits(self.fgr[r])
        } else {
            f64::from_bits(self.fgr[r & !1])
        }
    }

    fn set_fd(&mut self, r: usize, v: f64) {
        if self.fr_bit() {
            self.fgr[r] = v.to_bits();
        } else {
            self.fgr[r & !1] = v.to_bits();
        }
    }

    /// Run approximately `cycles` CPU cycles (1 instruction ≈ 1 cycle here).
    pub fn run(&mut self, bus: &mut Bus, cycles: u64) {
        if self.stuck.is_some() {
            return;
        }
        let mut n = 0u64;
        while n < cycles {
            // timer: Count at half PClock
            let count = ((self.cp0[CP0_COUNT] as u32).wrapping_add(1)) as u64;
            self.cp0[CP0_COUNT] = count;
            if count as u32 == self.cp0[CP0_COMPARE] as u32 {
                self.cp0[CP0_CAUSE] |= 1 << 15; // IP7
            }
            if self.check_interrupts(bus) {
                n += 1;
                continue;
            }
            let pc = self.pc;
            self.step(bus);
            n += 1;
            // Idle loop (branch-to-self, empty delay slot): nothing can change
            // until an interrupt, and every interrupt source is raised either
            // by the CPU's own I/O or at a slice boundary. Fast-forward the
            // rest of this slice, advancing Count and catching Compare.
            if self.pc == pc && self.idle_loop {
                let rest = cycles - n;
                let now = self.cp0[CP0_COUNT] as u32;
                let cmp = self.cp0[CP0_COMPARE] as u32;
                let to_cmp = cmp.wrapping_sub(now);
                if to_cmp != 0 && (to_cmp as u64) <= rest {
                    self.cp0[CP0_CAUSE] |= 1 << 15;
                }
                self.cp0[CP0_COUNT] = now.wrapping_add(rest as u32) as u64;
                self.idle_cycles += rest;
                n = cycles;
            }
            self.idle_loop = false;
        }
    }

    fn step(&mut self, bus: &mut Bus) {
        let pc = self.pc;
        let instr = self.read_v32(bus, pc);
        self.pc = self.next_pc;
        self.next_pc = self.pc.wrapping_add(4);
        self.execute(bus, instr, pc, false);
        self.gpr[0] = 0;
    }

    fn execute(&mut self, bus: &mut Bus, instr: u32, pc: u32, in_delay: bool) {
        let op = instr >> 26;
        let rs = ((instr >> 21) & 31) as usize;
        let rt = ((instr >> 16) & 31) as usize;
        let rd = ((instr >> 11) & 31) as usize;
        let sa = (instr >> 6) & 31;
        let imm = instr & 0xFFFF;
        let simm = imm as i16 as i64;
        let addr = (self.gpr[rs] as u32).wrapping_add(simm as u32);

        macro_rules! take_branch {
            ($cond:expr, $likely:expr) => {{
                let taken = $cond;
                // delay slot executes unless (likely && !taken)
                if taken || !$likely {
                    let slot_pc = self.pc;
                    let slot = self.read_v32(bus, slot_pc);
                    self.execute(bus, slot, slot_pc, true);
                    self.gpr[0] = 0;
                }
                if taken {
                    let target = pc.wrapping_add(4).wrapping_add((simm as u32) << 2);
                    if target == pc && self.read_v32(bus, pc.wrapping_add(4)) == 0 {
                        self.idle_loop = true;
                    }
                    self.set_pc(target);
                } else {
                    let cont = pc.wrapping_add(8);
                    self.set_pc(cont);
                }
            }};
        }

        macro_rules! jump_to {
            ($target:expr) => {{
                let slot_pc = self.pc;
                let slot = self.read_v32(bus, slot_pc);
                self.execute(bus, slot, slot_pc, true);
                self.gpr[0] = 0;
                self.set_pc($target);
            }};
        }

        match op {
            0 => {
                // SPECIAL
                match instr & 0x3F {
                    0x00 => self.gpr[rd] = ((self.gpr[rt] as u32) << sa) as i32 as u64,
                    0x02 => self.gpr[rd] = ((self.gpr[rt] as u32) >> sa) as i32 as u64,
                    0x03 => self.gpr[rd] = ((self.gpr[rt] as u32 as i32) >> sa) as u64,
                    0x04 => {
                        self.gpr[rd] =
                            ((self.gpr[rt] as u32) << (self.gpr[rs] & 31)) as i32 as u64
                    },
                    0x06 => {
                        self.gpr[rd] =
                            ((self.gpr[rt] as u32) >> (self.gpr[rs] & 31)) as i32 as u64
                    },
                    0x07 => {
                        self.gpr[rd] =
                            ((self.gpr[rt] as u32 as i32) >> (self.gpr[rs] & 31)) as u64
                    },
                    0x08 => jump_to!(self.gpr[rs] as u32), // JR
                    0x09 => {
                        let ret = pc.wrapping_add(8) as i32 as u64;
                        let target = self.gpr[rs] as u32;
                        self.gpr[rd] = ret;
                        jump_to!(target); // JALR
                    },
                    0x0C => self.exception(8, in_delay), // SYSCALL
                    0x0D => self.exception(9, in_delay), // BREAK
                    0x0F => {},                          // SYNC
                    0x10 => self.gpr[rd] = self.hi,
                    0x11 => self.hi = self.gpr[rs],
                    0x12 => self.gpr[rd] = self.lo,
                    0x13 => self.lo = self.gpr[rs],
                    0x14 => self.gpr[rd] = self.gpr[rt] << (self.gpr[rs] & 63), // DSLLV
                    0x16 => self.gpr[rd] = self.gpr[rt] >> (self.gpr[rs] & 63), // DSRLV
                    0x17 => {
                        self.gpr[rd] = ((self.gpr[rt] as i64) >> (self.gpr[rs] & 63)) as u64
                    },
                    0x18 => {
                        // MULT
                        let r =
                            (self.gpr[rs] as u32 as i32 as i64) * (self.gpr[rt] as u32 as i32 as i64);
                        self.lo = (r as i32) as u64;
                        self.hi = ((r >> 32) as i32) as u64;
                    },
                    0x19 => {
                        // MULTU
                        let r = (self.gpr[rs] as u32 as u64) * (self.gpr[rt] as u32 as u64);
                        self.lo = (r as u32 as i32) as u64;
                        self.hi = ((r >> 32) as u32 as i32) as u64;
                    },
                    0x1A => {
                        // DIV
                        let a = self.gpr[rs] as u32 as i32;
                        let b = self.gpr[rt] as u32 as i32;
                        if b != 0 {
                            self.lo = a.wrapping_div(b) as u64;
                            self.hi = a.wrapping_rem(b) as u64;
                        } else {
                            self.lo = if a < 0 { 1 } else { -1i64 as u64 };
                            self.hi = a as u64;
                        }
                    },
                    0x1B => {
                        // DIVU
                        let a = self.gpr[rs] as u32;
                        let b = self.gpr[rt] as u32;
                        if b != 0 {
                            self.lo = (a / b) as i32 as u64;
                            self.hi = (a % b) as i32 as u64;
                        } else {
                            self.lo = u64::MAX;
                            self.hi = a as i32 as u64;
                        }
                    },
                    0x1C => {
                        // DMULT
                        let r = (self.gpr[rs] as i64 as i128) * (self.gpr[rt] as i64 as i128);
                        self.lo = r as u64;
                        self.hi = (r >> 64) as u64;
                    },
                    0x1D => {
                        // DMULTU
                        let r = (self.gpr[rs] as u128) * (self.gpr[rt] as u128);
                        self.lo = r as u64;
                        self.hi = (r >> 64) as u64;
                    },
                    0x1E => {
                        let a = self.gpr[rs] as i64;
                        let b = self.gpr[rt] as i64;
                        if b != 0 {
                            self.lo = a.wrapping_div(b) as u64;
                            self.hi = a.wrapping_rem(b) as u64;
                        }
                    },
                    0x1F => {
                        let a = self.gpr[rs];
                        let b = self.gpr[rt];
                        if b != 0 {
                            self.lo = a / b;
                            self.hi = a % b;
                        }
                    },
                    0x20 | 0x21 => {
                        self.gpr[rd] =
                            (self.gpr[rs] as u32).wrapping_add(self.gpr[rt] as u32) as i32 as u64
                    },
                    0x22 | 0x23 => {
                        self.gpr[rd] =
                            (self.gpr[rs] as u32).wrapping_sub(self.gpr[rt] as u32) as i32 as u64
                    },
                    0x24 => self.gpr[rd] = self.gpr[rs] & self.gpr[rt],
                    0x25 => self.gpr[rd] = self.gpr[rs] | self.gpr[rt],
                    0x26 => self.gpr[rd] = self.gpr[rs] ^ self.gpr[rt],
                    0x27 => self.gpr[rd] = !(self.gpr[rs] | self.gpr[rt]),
                    0x2A => {
                        self.gpr[rd] = ((self.gpr[rs] as i64) < (self.gpr[rt] as i64)) as u64
                    },
                    0x2B => self.gpr[rd] = (self.gpr[rs] < self.gpr[rt]) as u64,
                    0x2C | 0x2D => self.gpr[rd] = self.gpr[rs].wrapping_add(self.gpr[rt]),
                    0x2E | 0x2F => self.gpr[rd] = self.gpr[rs].wrapping_sub(self.gpr[rt]),
                    0x30 => {
                        if (self.gpr[rs] as i64) >= (self.gpr[rt] as i64) {
                            self.exception(13, in_delay)
                        }
                    },
                    0x31 => {
                        if self.gpr[rs] >= self.gpr[rt] {
                            self.exception(13, in_delay)
                        }
                    },
                    0x32 => {
                        if (self.gpr[rs] as i64) < (self.gpr[rt] as i64) {
                            self.exception(13, in_delay)
                        }
                    },
                    0x33 => {
                        if self.gpr[rs] < self.gpr[rt] {
                            self.exception(13, in_delay)
                        }
                    },
                    0x34 => {
                        if self.gpr[rs] == self.gpr[rt] {
                            self.exception(13, in_delay)
                        }
                    },
                    0x36 => {
                        if self.gpr[rs] != self.gpr[rt] {
                            self.exception(13, in_delay)
                        }
                    },
                    0x38 => self.gpr[rd] = self.gpr[rt] << sa,          // DSLL
                    0x3A => self.gpr[rd] = self.gpr[rt] >> sa,          // DSRL
                    0x3B => self.gpr[rd] = ((self.gpr[rt] as i64) >> sa) as u64, // DSRA
                    0x3C => self.gpr[rd] = self.gpr[rt] << (sa + 32),   // DSLL32
                    0x3E => self.gpr[rd] = self.gpr[rt] >> (sa + 32),   // DSRL32
                    0x3F => self.gpr[rd] = ((self.gpr[rt] as i64) >> (sa + 32)) as u64,
                    other => {
                        self.stuck =
                            Some(format!("unimplemented SPECIAL 0x{other:02X} at {pc:08X}"))
                    },
                }
            },
            1 => {
                // REGIMM
                let cond_lt = (self.gpr[rs] as i64) < 0;
                match rt {
                    0x00 => take_branch!(cond_lt, false),  // BLTZ
                    0x01 => take_branch!(!cond_lt, false), // BGEZ
                    0x02 => take_branch!(cond_lt, true),   // BLTZL
                    0x03 => take_branch!(!cond_lt, true),  // BGEZL
                    0x10 => {
                        self.gpr[31] = pc.wrapping_add(8) as i32 as u64;
                        take_branch!(cond_lt, false);
                    },
                    0x11 => {
                        self.gpr[31] = pc.wrapping_add(8) as i32 as u64;
                        take_branch!(!cond_lt, false);
                    },
                    _ => self.stuck = Some(format!("unimplemented REGIMM {rt} at {pc:08X}")),
                }
            },
            2 => {
                let target = (pc.wrapping_add(4) & 0xF000_0000) | ((instr & 0x03FF_FFFF) << 2);
                jump_to!(target);
            },
            3 => {
                self.gpr[31] = pc.wrapping_add(8) as i32 as u64;
                let target = (pc.wrapping_add(4) & 0xF000_0000) | ((instr & 0x03FF_FFFF) << 2);
                jump_to!(target);
            },
            4 => take_branch!(self.gpr[rs] == self.gpr[rt], false),
            5 => take_branch!(self.gpr[rs] != self.gpr[rt], false),
            6 => take_branch!((self.gpr[rs] as i64) <= 0, false),
            7 => take_branch!((self.gpr[rs] as i64) > 0, false),
            8 | 9 => self.gpr[rt] = (self.gpr[rs] as u32).wrapping_add(simm as u32) as i32 as u64,
            10 => self.gpr[rt] = ((self.gpr[rs] as i64) < simm) as u64,
            11 => self.gpr[rt] = (self.gpr[rs] < simm as u64) as u64,
            12 => self.gpr[rt] = self.gpr[rs] & imm as u64,
            13 => self.gpr[rt] = self.gpr[rs] | imm as u64,
            14 => self.gpr[rt] = self.gpr[rs] ^ imm as u64,
            15 => self.gpr[rt] = ((imm as u32) << 16) as i32 as u64,
            16 => {
                // COP0
                match rs {
                    0 => self.gpr[rt] = self.cp0[rd] as u32 as i32 as u64, // MFC0
                    4 => {
                        // MTC0
                        let v = self.gpr[rt] as u32;
                        match rd {
                            CP0_COMPARE => {
                                self.cp0[CP0_COMPARE] = v as u64;
                                self.cp0[CP0_CAUSE] &= !(1 << 15);
                            },
                            CP0_CAUSE => {
                                let keep = self.cp0[CP0_CAUSE] & !0x300;
                                self.cp0[CP0_CAUSE] = keep | (v as u64 & 0x300);
                            },
                            _ => self.cp0[rd] = v as i32 as u64,
                        }
                    },
                    1 => self.gpr[rt] = self.cp0[rd] as u32 as i32 as u64, // DMFC0 (approx)
                    5 => self.cp0[rd] = self.gpr[rt],                      // DMTC0
                    16..=31 => match instr & 0x3F {
                        0x02 => {
                            // TLBWI
                            let i = (self.cp0[CP0_INDEX] & 31) as usize;
                            self.tlb[i] = TlbEntry {
                                hi: self.cp0[CP0_ENTRYHI],
                                lo0: self.cp0[CP0_ENTRYLO0],
                                lo1: self.cp0[CP0_ENTRYLO1],
                                mask: self.cp0[CP0_PAGEMASK] as u32,
                            };
                        },
                        0x06 => {
                            // TLBWR → use a rotating index
                            let i = ((self.cp0[CP0_COUNT] >> 2) & 31) as usize;
                            self.tlb[i] = TlbEntry {
                                hi: self.cp0[CP0_ENTRYHI],
                                lo0: self.cp0[CP0_ENTRYLO0],
                                lo1: self.cp0[CP0_ENTRYLO1],
                                mask: self.cp0[CP0_PAGEMASK] as u32,
                            };
                        },
                        0x01 => {
                            // TLBR
                            let i = (self.cp0[CP0_INDEX] & 31) as usize;
                            self.cp0[CP0_ENTRYHI] = self.tlb[i].hi;
                            self.cp0[CP0_ENTRYLO0] = self.tlb[i].lo0;
                            self.cp0[CP0_ENTRYLO1] = self.tlb[i].lo1;
                            self.cp0[CP0_PAGEMASK] = self.tlb[i].mask as u64;
                        },
                        0x08 => {
                            // TLBP
                            self.cp0[CP0_INDEX] = 0x8000_0000;
                            for (i, e) in self.tlb.iter().enumerate() {
                                let mask = !(e.mask as u64 | 0x1FFF);
                                if (self.cp0[CP0_ENTRYHI] & mask) == (e.hi & mask) {
                                    self.cp0[CP0_INDEX] = i as u64;
                                    break;
                                }
                            }
                        },
                        0x18 => {
                            // ERET
                            self.cp0[CP0_STATUS] &= !2;
                            let epc = self.cp0[CP0_EPC] as u32;
                            self.set_pc(epc);
                            self.ll_bit = false;
                            self.eret_count += 1;
                        },
                        other => {
                            self.stuck =
                                Some(format!("unimplemented CO fn 0x{other:02X} at {pc:08X}"))
                        },
                    },
                    _ => self.stuck = Some(format!("unimplemented COP0 rs={rs} at {pc:08X}")),
                }
            },
            17 => self.cop1(bus, instr, pc),
            20 => take_branch!(self.gpr[rs] == self.gpr[rt], true), // BEQL
            21 => take_branch!(self.gpr[rs] != self.gpr[rt], true), // BNEL
            22 => take_branch!((self.gpr[rs] as i64) <= 0, true),   // BLEZL
            23 => take_branch!((self.gpr[rs] as i64) > 0, true),    // BGTZL
            24 | 25 => self.gpr[rt] = self.gpr[rs].wrapping_add(simm as u64), // DADDI(U)
            26 => {
                // LDL
                let shift = 8 * (addr & 7);
                let mem = self.read_v64(bus, addr & !7);
                let old = self.gpr[rt];
                self.gpr[rt] = (mem << shift) | (old & ((1u64 << shift).wrapping_sub(1)));
            },
            27 => {
                // LDR
                let shift = 8 * (7 - (addr & 7));
                let mem = self.read_v64(bus, addr & !7);
                let old = self.gpr[rt];
                let keep = if shift == 0 { 0 } else { old >> (64 - shift) << (64 - shift) };
                self.gpr[rt] = keep | (mem >> shift);
            },
            32 => self.gpr[rt] = self.read_v8(bus, addr) as i8 as u64,
            33 => self.gpr[rt] = self.read_v16(bus, addr) as i16 as u64,
            34 => {
                // LWL
                let shift = 8 * (addr & 3);
                let mem = self.read_v32(bus, addr & !3);
                let old = self.gpr[rt] as u32;
                let merged =
                    (mem << shift) | (old & ((1u32 << shift).wrapping_sub(1)));
                self.gpr[rt] = merged as i32 as u64;
            },
            35 => self.gpr[rt] = self.read_v32(bus, addr) as i32 as u64,
            36 => self.gpr[rt] = self.read_v8(bus, addr) as u64,
            37 => self.gpr[rt] = self.read_v16(bus, addr) as u64,
            38 => {
                // LWR
                let shift = 8 * (3 - (addr & 3));
                let mem = self.read_v32(bus, addr & !3);
                let old = self.gpr[rt] as u32;
                let keep = if shift == 0 { 0 } else { old >> (32 - shift) << (32 - shift) };
                self.gpr[rt] = (keep | (mem >> shift)) as i32 as u64;
            },
            39 => self.gpr[rt] = self.read_v32(bus, addr) as u64, // LWU
            40 => self.write_v8(bus, addr, self.gpr[rt] as u8),
            41 => self.write_v16(bus, addr, self.gpr[rt] as u16),
            42 => {
                // SWL
                let shift = 8 * (addr & 3);
                let mem = self.read_v32(bus, addr & !3);
                let v = self.gpr[rt] as u32;
                let merged = (mem & !(0xFFFF_FFFFu32 >> shift)) | (v >> shift);
                self.write_v32(bus, addr & !3, merged);
            },
            43 => self.write_v32(bus, addr, self.gpr[rt] as u32),
            44 => {
                // SDL
                let shift = 8 * (addr & 7);
                let mem = self.read_v64(bus, addr & !7);
                let v = self.gpr[rt];
                let merged = (mem & !(u64::MAX >> shift)) | (v >> shift);
                self.write_v64(bus, addr & !7, merged);
            },
            45 => {
                // SDR
                let shift = 8 * (7 - (addr & 7));
                let mem = self.read_v64(bus, addr & !7);
                let v = self.gpr[rt];
                let mask = if shift == 0 { u64::MAX } else { u64::MAX << shift };
                let merged = (mem & !mask) | (v << shift);
                self.write_v64(bus, addr & !7, merged);
            },
            46 => {
                // SWR
                let shift = 8 * (3 - (addr & 3));
                let mem = self.read_v32(bus, addr & !3);
                let v = self.gpr[rt] as u32;
                let mask = if shift == 0 { 0xFFFF_FFFFu32 } else { 0xFFFF_FFFFu32 << shift };
                let merged = (mem & !mask) | (v << shift);
                self.write_v32(bus, addr & !3, merged);
            },
            47 => {}, // CACHE
            48 => {
                self.gpr[rt] = self.read_v32(bus, addr) as i32 as u64;
                self.ll_bit = true;
            },
            49 => {
                // LWC1
                let v = self.read_v32(bus, addr);
                let r = rt;
                if self.fr_bit() || r & 1 == 0 {
                    let base = if self.fr_bit() { r } else { r & !1 };
                    self.fgr[base] = (self.fgr[base] & 0xFFFF_FFFF_0000_0000) | v as u64;
                } else {
                    let base = r & !1;
                    self.fgr[base] =
                        (self.fgr[base] & 0x0000_0000_FFFF_FFFF) | ((v as u64) << 32);
                }
            },
            53 => {
                // LDC1
                let v = self.read_v64(bus, addr);
                let base = if self.fr_bit() { rt } else { rt & !1 };
                self.fgr[base] = v;
            },
            55 => self.gpr[rt] = self.read_v64(bus, addr), // LD
            56 => {
                self.write_v32(bus, addr, self.gpr[rt] as u32);
                self.gpr[rt] = self.ll_bit as u64;
                self.ll_bit = false;
            },
            57 => {
                // SWC1
                let r = rt;
                let v = if self.fr_bit() || r & 1 == 0 {
                    let base = if self.fr_bit() { r } else { r & !1 };
                    self.fgr[base] as u32
                } else {
                    (self.fgr[r & !1] >> 32) as u32
                };
                self.write_v32(bus, addr, v);
            },
            61 => {
                let base = if self.fr_bit() { rt } else { rt & !1 };
                let v = self.fgr[base];
                self.write_v64(bus, addr, v);
            },
            63 => self.write_v64(bus, addr, self.gpr[rt]), // SD
            other => self.stuck = Some(format!("unimplemented opcode {other} at {pc:08X}")),
        }
    }

    fn cop1(&mut self, bus: &mut Bus, instr: u32, pc: u32) {
        let rs = ((instr >> 21) & 31) as usize;
        let rt = ((instr >> 16) & 31) as usize;
        let fs = ((instr >> 11) & 31) as usize;
        let ft = rt;
        let fdr = ((instr >> 6) & 31) as usize;
        match rs {
            0 => {
                // MFC1
                let v = if self.fr_bit() || fs & 1 == 0 {
                    self.fgr[if self.fr_bit() { fs } else { fs & !1 }] as u32
                } else {
                    (self.fgr[fs & !1] >> 32) as u32
                };
                self.gpr[rt] = v as i32 as u64;
            },
            1 => self.gpr[rt] = self.fgr[if self.fr_bit() { fs } else { fs & !1 }], // DMFC1
            2 => self.gpr[rt] = self.fcr31 as u64,                                  // CFC1
            4 => {
                // MTC1
                let v = self.gpr[rt] as u32;
                if self.fr_bit() || fs & 1 == 0 {
                    let b = if self.fr_bit() { fs } else { fs & !1 };
                    self.fgr[b] = (self.fgr[b] & 0xFFFF_FFFF_0000_0000) | v as u64;
                } else {
                    let b = fs & !1;
                    self.fgr[b] = (self.fgr[b] & 0x0000_0000_FFFF_FFFF) | ((v as u64) << 32);
                }
            },
            5 => {
                let b = if self.fr_bit() { fs } else { fs & !1 };
                self.fgr[b] = self.gpr[rt];
            },
            6 => self.fcr31 = self.gpr[rt] as u32, // CTC1
            8 => {
                // BC1
                let cond = self.fcr31 & (1 << 23) != 0;
                let want = rt & 1 == 1;
                let likely = rt & 2 == 2;
                let simm = (instr & 0xFFFF) as i16 as i32;
                let taken = cond == want;
                if taken || !likely {
                    let slot_pc = self.pc;
                    let slot = self.read_v32(bus, slot_pc);
                    self.execute(bus, slot, slot_pc, true);
                    self.gpr[0] = 0;
                }
                if taken {
                    let target = pc.wrapping_add(4).wrapping_add((simm as u32) << 2);
                    self.set_pc(target);
                } else {
                    self.set_pc(pc.wrapping_add(8));
                }
            },
            16 => {
                // single
                let a = self.fs_read(fs);
                let b = self.fs_read(ft);
                match instr & 0x3F {
                    0x00 => self.fs_write(fdr, a + b),
                    0x01 => self.fs_write(fdr, a - b),
                    0x02 => self.fs_write(fdr, a * b),
                    0x03 => self.fs_write(fdr, a / b),
                    0x04 => self.fs_write(fdr, a.sqrt()),
                    0x05 => self.fs_write(fdr, a.abs()),
                    0x06 => self.fs_write(fdr, a),
                    0x07 => self.fs_write(fdr, -a),
                    0x0C => self.w_write(fdr, a.round_ties_even() as i32),
                    0x0D => self.w_write(fdr, a.trunc() as i32),
                    0x0E => self.w_write(fdr, a.ceil() as i32),
                    0x0F => self.w_write(fdr, a.floor() as i32),
                    0x21 => self.fd_write(fdr, a as f64),
                    0x24 => self.w_write(fdr, a as i32),
                    0x25 => {
                        let b64 = if self.fr_bit() { fdr } else { fdr & !1 };
                        self.fgr[b64] = a as i64 as u64;
                    },
                    0x30..=0x3F => {
                        let c = instr & 0xF;
                        let res = fcmp(a as f64, b as f64, c);
                        if res {
                            self.fcr31 |= 1 << 23;
                        } else {
                            self.fcr31 &= !(1 << 23);
                        }
                    },
                    other => {
                        self.stuck =
                            Some(format!("unimplemented FPU.S 0x{other:02X} at {pc:08X}"))
                    },
                }
            },
            17 => {
                // double
                let a = self.fd(fs);
                let b = self.fd(ft);
                match instr & 0x3F {
                    0x00 => self.set_fd(fdr, a + b),
                    0x01 => self.set_fd(fdr, a - b),
                    0x02 => self.set_fd(fdr, a * b),
                    0x03 => self.set_fd(fdr, a / b),
                    0x04 => self.set_fd(fdr, a.sqrt()),
                    0x05 => self.set_fd(fdr, a.abs()),
                    0x06 => self.set_fd(fdr, a),
                    0x07 => self.set_fd(fdr, -a),
                    0x0C => self.w_write(fdr, a.round_ties_even() as i32),
                    0x0D => self.w_write(fdr, a.trunc() as i32),
                    0x0E => self.w_write(fdr, a.ceil() as i32),
                    0x0F => self.w_write(fdr, a.floor() as i32),
                    0x20 => self.fs_write(fdr, a as f32),
                    0x24 => self.w_write(fdr, a as i32),
                    0x25 => {
                        let b64 = if self.fr_bit() { fdr } else { fdr & !1 };
                        self.fgr[b64] = a as i64 as u64;
                    },
                    0x30..=0x3F => {
                        let c = instr & 0xF;
                        let res = fcmp(a, b, c);
                        if res {
                            self.fcr31 |= 1 << 23;
                        } else {
                            self.fcr31 &= !(1 << 23);
                        }
                    },
                    other => {
                        self.stuck =
                            Some(format!("unimplemented FPU.D 0x{other:02X} at {pc:08X}"))
                    },
                }
            },
            20 => {
                // W (32-bit int source)
                let a = self.w_read(fs);
                match instr & 0x3F {
                    0x20 => self.fs_write(fdr, a as f32),
                    0x21 => self.fd_write(fdr, a as f64),
                    other => {
                        self.stuck =
                            Some(format!("unimplemented FPU.W 0x{other:02X} at {pc:08X}"))
                    },
                }
            },
            21 => {
                // L (64-bit int source)
                let b64 = if self.fr_bit() { fs } else { fs & !1 };
                let a = self.fgr[b64] as i64;
                match instr & 0x3F {
                    0x20 => self.fs_write(fdr, a as f32),
                    0x21 => self.fd_write(fdr, a as f64),
                    other => {
                        self.stuck =
                            Some(format!("unimplemented FPU.L 0x{other:02X} at {pc:08X}"))
                    },
                }
            },
            _ => self.stuck = Some(format!("unimplemented COP1 rs={rs} at {pc:08X}")),
        }
    }

    // compact FPU single-precision helpers honoring FR
    fn fs_read(&self, r: usize) -> f32 {
        if self.fr_bit() || r & 1 == 0 {
            f32::from_bits(self.fgr[if self.fr_bit() { r } else { r & !1 }] as u32)
        } else {
            f32::from_bits((self.fgr[r & !1] >> 32) as u32)
        }
    }

    fn fs_write(&mut self, r: usize, v: f32) {
        if self.fr_bit() || r & 1 == 0 {
            let b = if self.fr_bit() { r } else { r & !1 };
            self.fgr[b] = (self.fgr[b] & 0xFFFF_FFFF_0000_0000) | v.to_bits() as u64;
        } else {
            let b = r & !1;
            self.fgr[b] = (self.fgr[b] & 0x0000_0000_FFFF_FFFF) | ((v.to_bits() as u64) << 32);
        }
    }

    fn fd_write(&mut self, r: usize, v: f64) {
        self.set_fd(r, v);
    }

    fn w_read(&self, r: usize) -> i32 {
        if self.fr_bit() || r & 1 == 0 {
            self.fgr[if self.fr_bit() { r } else { r & !1 }] as u32 as i32
        } else {
            (self.fgr[r & !1] >> 32) as u32 as i32
        }
    }

    fn w_write(&mut self, r: usize, v: i32) {
        if self.fr_bit() || r & 1 == 0 {
            let b = if self.fr_bit() { r } else { r & !1 };
            self.fgr[b] = (self.fgr[b] & 0xFFFF_FFFF_0000_0000) | (v as u32 as u64);
        } else {
            let b = r & !1;
            self.fgr[b] = (self.fgr[b] & 0x0000_0000_FFFF_FFFF) | ((v as u32 as u64) << 32);
        }
    }
}

fn fcmp(a: f64, b: f64, cond: u32) -> bool {
    let unordered = a.is_nan() || b.is_nan();
    let lt = !unordered && a < b;
    let eq = !unordered && a == b;
    match cond & 7 {
        0 => false,
        1 => unordered,
        2 => eq,
        3 => eq || unordered,
        4 => lt,
        5 => lt || unordered,
        6 => lt || eq,
        7 => lt || eq || unordered,
        _ => false,
    }
}
