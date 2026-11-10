//! N64 physical bus: RDRAM, SP memories, MMIO (MI/VI/AI/PI/RI/SI), cart, PIF.
//! The ROM is normalized to big-endian (.z64) order at load.

pub const MI_INTR_SP: u32 = 1 << 0;
pub const MI_INTR_SI: u32 = 1 << 1;
pub const MI_INTR_AI: u32 = 1 << 2;
pub const MI_INTR_VI: u32 = 1 << 3;
pub const MI_INTR_PI: u32 = 1 << 4;
pub const MI_INTR_DP: u32 = 1 << 5;

#[derive(Default, Clone, Copy)]
pub struct Pad {
    pub buttons: u16,
    pub stick_x: i8,
    pub stick_y: i8,
    pub present: bool,
}

pub struct Bus {
    pub rdram: Vec<u8>, // 8 MB (Expansion Pak size)
    pub dmem: Vec<u8>,  // 4 KB
    pub imem: Vec<u8>,  // 4 KB
    pub rom: Vec<u8>,
    pub pif_ram: [u8; 64],

    pub mi_intr: u32,
    pub mi_mask: u32,

    // VI
    pub vi_ctrl: u32,
    pub vi_origin: u32,
    pub vi_width: u32,
    /// Raw VI register file (timing/scale regs 0x14..0x34 kept for scanout).
    pub vi_regs: [u32; 14],
    pub vi_intr_line: u32,
    pub vi_current: u32,

    // PI
    pub pi_dram_addr: u32,
    pub pi_cart_addr: u32,

    // SP
    pub sp_mem_addr: u32,
    pub sp_dram_addr: u32,
    pub sp_halt: bool,
    pub sp_broke: bool,
    pub sp_intr_break: bool,
    pub sp_sstep: bool,
    pub sp_sig: u8,
    pub sp_semaphore: bool,
    /// Set when the CPU clears halt — the frontend sniffs the task once.
    pub sp_kicked: bool,
    pub sp_pc: u32,
    /// The RSP core itself: owned by the bus so clearing halt can run the
    /// task synchronously (hardware finishes small ucodes before the CPU's
    /// next MMIO access — test ROMs rely on it and don't poll).
    pub rsp: Option<Box<super::rsp_core::Rsp>>,
    in_rsp: bool,

    // DPC (RDP command interface)
    pub dpc_start: u32,
    pub dpc_end: u32,
    pub dpc_current: u32,
    pub dpc_xbus: bool,
    pub dpc_freeze: bool,
    pub rdp_cmds: u64,
    pub rdp: super::rdp::Rdp,
    pub prof_rdp_ns: u128,
    pub prof_rsp_ns: u128,

    // SI / AI
    pub si_dram_addr: u32,
    pub ai_dram_addr: u32,
    pub ai_len: u32,
    pub ai_dacrate: u32,
    /// Two-entry DMA FIFO (addr, len); [0] is playing while `ai_count` > 0.
    pub ai_fifo: [(u32, u32); 2],
    pub ai_count: usize,
    /// CPU cycles left until the playing buffer drains.
    pub ai_remain: f64,
    /// Samples DMA'd since the host last drained them.
    pub ai_out: Vec<(i16, i16)>,

    /// Latest microcode identification (for honest status reporting).
    pub last_ucode: Option<String>,

    pub pads: [Pad; 4],
    /// 16-kbit cartridge EEPROM (Zelda-class saves) + dirty flag for the host.
    pub eeprom: Vec<u8>,
    pub eeprom_dirty: bool,
    pub vi_clear_count: u64,
    pub pi_dma_count: u64,
    pub si_rd_count: u64,
    pub si_wr_count: u64,
    pub challenge_count: u64,
}

impl Bus {
    pub fn new(rom_be: Vec<u8>) -> Bus {
        let mut pads = [Pad::default(); 4];
        pads[0].present = true;
        Bus {
            rdram: vec![0; 8 * 1024 * 1024],
            dmem: vec![0; 4096],
            imem: vec![0; 4096],
            rom: rom_be,
            pif_ram: [0; 64],
            mi_intr: 0,
            mi_mask: 0,
            vi_ctrl: 0,
            vi_origin: 0,
            vi_width: 320,
            vi_regs: [0; 14],
            vi_intr_line: 0x3FF,
            vi_current: 0,
            pi_dram_addr: 0,
            pi_cart_addr: 0,
            sp_mem_addr: 0,
            sp_dram_addr: 0,
            sp_halt: true,
            sp_broke: false,
            sp_intr_break: false,
            sp_sstep: false,
            sp_sig: 0,
            sp_semaphore: false,
            sp_kicked: false,
            sp_pc: 0,
            rsp: Some(Box::new(super::rsp_core::Rsp::new())),
            in_rsp: false,
            dpc_start: 0,
            dpc_end: 0,
            dpc_current: 0,
            dpc_xbus: false,
            dpc_freeze: false,
            rdp_cmds: 0,
            rdp: super::rdp::Rdp::new(),
            prof_rdp_ns: 0,
            prof_rsp_ns: 0,
            si_dram_addr: 0,
            ai_dram_addr: 0,
            ai_len: 0,
            ai_dacrate: 1520,
            ai_fifo: [(0, 0); 2],
            ai_count: 0,
            ai_remain: 0.0,
            ai_out: Vec::with_capacity(4096),
            last_ucode: None,
            pads,
            eeprom: vec![0; 2048],
            eeprom_dirty: false,
            vi_clear_count: 0,
            pi_dma_count: 0,
            si_rd_count: 0,
            si_wr_count: 0,
            challenge_count: 0,
        }
    }

    /// DAC output rate: the NTSC VI clock divided by (DACRATE + 1).
    pub fn ai_rate(&self) -> f64 {
        48_681_812.0 / (self.ai_dacrate as f64 + 1.0)
    }

    fn ai_cycles_per_sample(&self) -> f64 {
        93_750_000.0 / self.ai_rate()
    }

    /// Begin playing FIFO slot 0: DMA its samples out and time its drain.
    fn ai_start(&mut self) {
        let (addr, len) = self.ai_fifo[0];
        for i in (0..len as usize).step_by(4) {
            let a = addr as usize + i;
            if a + 3 >= self.rdram.len() {
                break;
            }
            let l = i16::from_be_bytes([self.rdram[a], self.rdram[a + 1]]);
            let r = i16::from_be_bytes([self.rdram[a + 2], self.rdram[a + 3]]);
            self.ai_out.push((l, r));
        }
        self.ai_remain = (len / 4) as f64 * self.ai_cycles_per_sample();
        self.raise(MI_INTR_AI);
    }

    /// Advance the DAC by `cycles` CPU cycles.
    pub fn ai_tick(&mut self, cycles: u64) {
        let mut c = cycles as f64;
        while self.ai_count > 0 && c > 0.0 {
            if c < self.ai_remain {
                self.ai_remain -= c;
                return;
            }
            c -= self.ai_remain;
            self.ai_remain = 0.0;
            self.ai_fifo[0] = self.ai_fifo[1];
            self.ai_count -= 1;
            if self.ai_count > 0 {
                self.ai_start();
            } else {
                self.raise(MI_INTR_AI);
            }
        }
    }

    #[inline]
    pub fn irq_asserted(&self) -> bool {
        self.mi_intr & self.mi_mask != 0
    }

    pub fn raise(&mut self, bit: u32) {
        self.mi_intr |= bit;
    }

    // ── byte-level physical access (RAM-like regions) ────────────────────────
    pub fn read8(&mut self, phys: u32) -> u8 {
        match phys {
            0x0000_0000..=0x007F_FFFF => self.rdram[phys as usize],
            0x0400_0000..=0x0400_0FFF => self.dmem[(phys & 0xFFF) as usize],
            0x0400_1000..=0x0400_1FFF => self.imem[(phys & 0xFFF) as usize],
            0x1000_0000..=0x1FBF_FFFF => {
                let off = (phys - 0x1000_0000) as usize;
                if off < self.rom.len() {
                    self.rom[off]
                } else {
                    0
                }
            },
            0x1FC0_07C0..=0x1FC0_07FF => self.pif_ram[(phys - 0x1FC0_07C0) as usize],
            // MMIO through the word path
            _ => {
                let w = self.read32(phys & !3);
                (w >> (8 * (3 - (phys & 3)))) as u8
            },
        }
    }

    pub fn write8(&mut self, phys: u32, v: u8) {
        match phys {
            0x0000_0000..=0x007F_FFFF => self.rdram[phys as usize] = v,
            0x0400_0000..=0x0400_0FFF => self.dmem[(phys & 0xFFF) as usize] = v,
            0x0400_1000..=0x0400_1FFF => self.imem[(phys & 0xFFF) as usize] = v,
            0x1FC0_07C0..=0x1FC0_07FF => self.pif_ram[(phys - 0x1FC0_07C0) as usize] = v,
            _ => {
                // byte write into MMIO: merge into the word
                let aligned = phys & !3;
                let shift = 8 * (3 - (phys & 3));
                let old = self.read32(aligned);
                let w = (old & !(0xFF << shift)) | ((v as u32) << shift);
                self.write32(aligned, w);
            },
        }
    }

    pub fn read32(&mut self, phys: u32) -> u32 {
        match phys {
            0x0000_0000..=0x007F_FFFF => be32(&self.rdram, phys as usize),
            0x03F0_0000..=0x03FF_FFFF => 0, // RDRAM interface regs
            0x0400_0000..=0x0400_0FFF => be32(&self.dmem, (phys & 0xFFF) as usize),
            0x0400_1000..=0x0400_1FFF => be32(&self.imem, (phys & 0xFFF) as usize),
            0x0404_0000..=0x040F_FFFF => {
                if phys >= 0x0408_0000 {
                    self.sp_pc
                } else {
                    match phys & 0x1F {
                        0x00 => self.sp_mem_addr,
                        0x04 => self.sp_dram_addr,
                        0x10 => self.sp_status_read(),
                        0x14 => 0, // SP_DMA_FULL
                        0x18 => 0, // SP_DMA_BUSY
                        0x1C => self.sp_semaphore_read(),
                        _ => 0,
                    }
                }
            },
            0x0410_0000..=0x041F_FFFF => match phys & 0x1F {
                0x00 => self.dpc_start,
                0x04 => self.dpc_end,
                0x08 => self.dpc_current,
                0x0C => self.dpc_status_read(),
                _ => 0,
            },
            0x0430_0000..=0x043F_FFFF => match phys & 0xF {
                0x0 => 0,          // MI MODE
                0x4 => 0x0202_0102, // MI VERSION
                0x8 => self.mi_intr,
                0xC => self.mi_mask,
                _ => 0,
            },
            0x0440_0000..=0x044F_FFFF => match phys & 0x3F {
                0x00 => self.vi_ctrl,
                0x04 => self.vi_origin,
                0x08 => self.vi_width,
                0x0C => self.vi_intr_line,
                r @ 0x14..=0x34 => self.vi_regs[(r >> 2) as usize],
                0x10 => {
                    // VI_CURRENT advances so vblank-waiters make progress
                    self.vi_current = (self.vi_current + 2) % 525;
                    self.vi_current
                },
                _ => 0,
            },
            0x0450_0000..=0x045F_FFFF => match phys & 0x1F {
                0x04 => {
                    if self.ai_count == 0 {
                        0
                    } else {
                        ((self.ai_remain / self.ai_cycles_per_sample()) as u32 * 4).min(self.ai_fifo[0].1)
                    }
                },
                0x0C => {
                    let full = (self.ai_count >= 2) as u32;
                    let busy = (self.ai_count >= 1) as u32;
                    (full << 31) | (busy << 30) | full
                },
                _ => 0,
            },
            0x0460_0000..=0x046F_FFFF => match phys & 0x3F {
                0x00 => self.pi_dram_addr,
                0x04 => self.pi_cart_addr,
                0x10 => 0, // PI_STATUS idle
                _ => 0,
            },
            0x0470_0000..=0x047F_FFFF => match phys & 0x1F {
                0x0C => 0x14, // RI_SELECT: initialized
                _ => 0,
            },
            0x0480_0000..=0x048F_FFFF => match phys & 0x1F {
                0x00 => self.si_dram_addr,
                0x18 => 0, // SI_STATUS idle
                _ => 0,
            },
            0x1000_0000..=0x1FBF_FFFF => {
                let off = (phys - 0x1000_0000) as usize;
                if off + 4 <= self.rom.len() {
                    be32(&self.rom, off)
                } else {
                    0
                }
            },
            0x1FC0_07C0..=0x1FC0_07FF => {
                let i = (phys - 0x1FC0_07C0) as usize;
                u32::from_be_bytes([
                    self.pif_ram[i],
                    self.pif_ram[i + 1],
                    self.pif_ram[i + 2],
                    self.pif_ram[i + 3],
                ])
            },
            _ => 0,
        }
    }

    pub fn write32(&mut self, phys: u32, v: u32) {
        match phys {
            0x0000_0000..=0x007F_FFFF => set_be32(&mut self.rdram, phys as usize, v),
            0x0400_0000..=0x0400_0FFF => set_be32(&mut self.dmem, (phys & 0xFFF) as usize, v),
            0x0400_1000..=0x0400_1FFF => set_be32(&mut self.imem, (phys & 0xFFF) as usize, v),
            0x0404_0000..=0x040F_FFFF => self.sp_write(phys, v),
            0x0410_0000..=0x041F_FFFF => match phys & 0x1F {
                0x00 => {
                    self.dpc_start = v & 0x00FF_FFF8;
                    self.dpc_current = self.dpc_start;
                },
                0x04 => {
                    self.dpc_end = v & 0x00FF_FFF8;
                    self.dpc_run();
                },
                0x0C => self.dpc_status_write(v),
                _ => {},
            },
            0x0430_0000..=0x043F_FFFF => match phys & 0xF {
                0x0 => {
                    // MI MODE: writing bit11 clears DP interrupt
                    if v & 0x0800 != 0 {
                        self.mi_intr &= !MI_INTR_DP;
                    }
                },
                0xC => {
                    // interrupt mask: pairs of clear/set bits
                    for i in 0..6 {
                        if v & (1 << (2 * i)) != 0 {
                            self.mi_mask &= !(1 << i);
                        }
                        if v & (1 << (2 * i + 1)) != 0 {
                            self.mi_mask |= 1 << i;
                        }
                    }
                },
                _ => {},
            },
            0x0440_0000..=0x044F_FFFF => match phys & 0x3F {
                0x00 => self.vi_ctrl = v,
                0x04 => self.vi_origin = v & 0x00FF_FFFF,
                0x08 => self.vi_width = v & 0xFFF,
                0x0C => self.vi_intr_line = v & 0x3FF,
                r @ 0x14..=0x34 => self.vi_regs[(r >> 2) as usize] = v,
                0x10 => {
                    self.mi_intr &= !MI_INTR_VI; // writing CURRENT clears VI intr
                    self.vi_clear_count += 1;
                },
                _ => {},
            },
            0x0450_0000..=0x045F_FFFF => match phys & 0x1F {
                0x00 => self.ai_dram_addr = v,
                0x04 => {
                    self.ai_len = v & 0x3FFF8;
                    if self.ai_len != 0 && self.ai_count < 2 {
                        self.ai_fifo[self.ai_count] = (self.ai_dram_addr & 0x00FF_FFF8, self.ai_len);
                        self.ai_count += 1;
                        if self.ai_count == 1 {
                            self.ai_start();
                        }
                    }
                },
                0x0C => self.mi_intr &= !MI_INTR_AI, // AI_STATUS write clears
                0x10 => self.ai_dacrate = v & 0x3FFF,
                _ => {},
            },
            0x0460_0000..=0x046F_FFFF => match phys & 0x3F {
                0x00 => self.pi_dram_addr = v & 0x00FF_FFFE,
                0x04 => self.pi_cart_addr = v,
                0x08 => {
                    // PI RD: RDRAM → cart (writeback) — ignore, complete
                    self.raise(MI_INTR_PI);
                },
                0x0C => {
                    // PI WR: cart → RDRAM, v = len-1
                    let len = (v as usize & 0x00FF_FFFF) + 1;
                    let src = (self.pi_cart_addr as usize).wrapping_sub(0x1000_0000);
                    let dst = self.pi_dram_addr as usize;
                    for i in 0..len {
                        let b = if src + i < self.rom.len() { self.rom[src + i] } else { 0 };
                        if dst + i < self.rdram.len() {
                            self.rdram[dst + i] = b;
                        }
                    }
                    self.pi_dma_count += 1;
                    self.raise(MI_INTR_PI);
                },
                0x10 => self.mi_intr &= !MI_INTR_PI, // PI_STATUS write clears
                _ => {},
            },
            0x0470_0000..=0x047F_FFFF => {},
            0x0480_0000..=0x048F_FFFF => match phys & 0x1F {
                0x00 => self.si_dram_addr = v & 0x00FF_FFFF,
                0x04 => {
                    // SI PIF→RDRAM 64B. Command byte 0x3F routes: bit1 = CIC
                    // challenge (CIC-6105 anti-piracy — OoT blocks on this),
                    // else run the joybus protocol.
                    self.si_rd_count += 1;
                    if self.pif_ram[0x3F] & 0x02 != 0 {
                        self.challenge_count += 1;
                        self.challenge_6105();
                    } else {
                        self.pif_process();
                    }
                    for i in 0..64 {
                        let b = self.pif_ram[i];
                        let d = self.si_dram_addr as usize + i;
                        if d < self.rdram.len() {
                            self.rdram[d] = b;
                        }
                    }
                    self.raise(MI_INTR_SI);
                },
                0x10 => {
                    // SI RDRAM→PIF 64B; bit0 of the command byte requests
                    // joybus formatting — acknowledge by clearing it.
                    for i in 0..64 {
                        let s = self.si_dram_addr as usize + i;
                        self.pif_ram[i] = if s < self.rdram.len() { self.rdram[s] } else { 0 };
                    }
                    self.si_wr_count += 1;
                    if self.pif_ram[0x3F] & 0x01 != 0 {
                        self.pif_ram[0x3F] &= !0x01;
                    }
                    self.raise(MI_INTR_SI);
                },
                0x18 => self.mi_intr &= !MI_INTR_SI,
                _ => {},
            },
            0x1FC0_07C0..=0x1FC0_07FF => {
                let i = (phys - 0x1FC0_07C0) as usize;
                self.pif_ram[i..i + 4].copy_from_slice(&v.to_be_bytes());
            },
            _ => {},
        }
    }

    fn sp_write(&mut self, phys: u32, v: u32) {
        if phys >= 0x0408_0000 {
            self.sp_pc = v & 0xFFC;
            return;
        }
        match phys & 0x1F {
            0x00 => self.sp_mem_addr = v & 0x1FFF,
            0x04 => self.sp_dram_addr = v & 0x00FF_FFFF,
            0x08 => self.sp_dma(false, v),
            0x0C => self.sp_dma(true, v),
            0x10 => self.sp_status_write(v),
            0x1C => self.sp_semaphore = false,
            _ => {},
        }
    }

    /// SP DMA with the hardware's count/skip line format.
    /// `to_dram` false: RDRAM → DMEM/IMEM (RD_LEN). true: WR_LEN.
    pub fn sp_dma(&mut self, to_dram: bool, lenreg: u32) {
        let len = ((lenreg as usize & 0xFFF) | 7) + 1;
        let count = ((lenreg >> 12) & 0xFF) as usize + 1;
        let skip = ((lenreg >> 20) & 0xFFF) as usize;
        let imem = self.sp_mem_addr & 0x1000 != 0;
        let mut m = (self.sp_mem_addr & 0xFF8) as usize;
        let mut d = (self.sp_dram_addr & 0x00FF_FFF8) as usize;
        for _ in 0..count {
            for i in 0..len {
                if to_dram {
                    let src = if imem { &self.imem } else { &self.dmem };
                    let b = src[(m + i) & 0xFFF];
                    if d + i < self.rdram.len() {
                        self.rdram[d + i] = b;
                    }
                } else {
                    let b = if d + i < self.rdram.len() { self.rdram[d + i] } else { 0 };
                    let t = if imem { &mut self.imem } else { &mut self.dmem };
                    t[(m + i) & 0xFFF] = b;
                }
            }
            m = (m + len) & 0xFFF;
            d += len + skip;
        }
        self.sp_mem_addr = (self.sp_mem_addr & 0x1000) | (m as u32 & 0xFFF);
        self.sp_dram_addr = d as u32 & 0x00FF_FFFF;
    }

    pub fn sp_status_read(&self) -> u32 {
        (self.sp_halt as u32)
            | (self.sp_broke as u32) << 1
            | (self.sp_sstep as u32) << 5
            | (self.sp_intr_break as u32) << 6
            | (self.sp_sig as u32) << 7
    }

    pub fn sp_status_write(&mut self, v: u32) {
        if v & (1 << 0) != 0 {
            if self.sp_halt {
                self.sp_kicked = true;
            }
            self.sp_halt = false;
            self.run_rsp(2_000_000);
        }
        if v & (1 << 1) != 0 {
            self.sp_halt = true;
        }
        if v & (1 << 2) != 0 {
            self.sp_broke = false;
        }
        if v & (1 << 3) != 0 {
            self.mi_intr &= !MI_INTR_SP;
        }
        if v & (1 << 4) != 0 {
            self.raise(MI_INTR_SP);
        }
        if v & (1 << 5) != 0 {
            self.sp_sstep = false;
        }
        if v & (1 << 6) != 0 {
            self.sp_sstep = true;
        }
        if v & (1 << 7) != 0 {
            self.sp_intr_break = false;
        }
        if v & (1 << 8) != 0 {
            self.sp_intr_break = true;
        }
        for k in 0..8 {
            if v & (1 << (9 + 2 * k)) != 0 {
                self.sp_sig &= !(1 << k);
            }
            if v & (1 << (10 + 2 * k)) != 0 {
                self.sp_sig |= 1 << k;
            }
        }
    }

    /// Execute the RSP until BREAK/halt or the step budget runs out.
    /// No-op when called from inside RSP execution itself.
    pub fn run_rsp(&mut self, budget: u32) {
        if self.in_rsp || self.sp_halt {
            return;
        }
        if let Some(mut rsp) = self.rsp.take() {
            self.in_rsp = true;
            rsp.pc = self.sp_pc & 0xFFC;
            let t0 = std::time::Instant::now();
            rsp.run(self, budget);
            self.prof_rsp_ns += t0.elapsed().as_nanos();
            self.sp_pc = rsp.pc & 0xFFF;
            if rsp.stuck.is_some() {
                self.sp_halt = true;
            }
            self.in_rsp = false;
            self.rsp = Some(rsp);
        }
    }

    pub fn sp_semaphore_read(&mut self) -> u32 {
        if self.sp_semaphore {
            1
        } else {
            self.sp_semaphore = true;
            0
        }
    }

    pub fn dpc_status_read(&self) -> u32 {
        (self.dpc_xbus as u32) | (self.dpc_freeze as u32) << 1
    }

    pub fn dpc_status_write(&mut self, v: u32) {
        if v & 1 != 0 {
            self.dpc_xbus = false;
        }
        if v & 2 != 0 {
            self.dpc_xbus = true;
        }
        if v & 4 != 0 {
            self.dpc_freeze = false;
        }
        if v & 8 != 0 {
            self.dpc_freeze = true;
        }
    }

    /// RDP command consumption stub: count commands, detect Sync_Full (0x29)
    /// to raise the DP interrupt the game waits on. Commands come from DMEM
    /// (XBUS) or RDRAM depending on DPC status.
    fn dpc_word(&self, addr: u32) -> u64 {
        let mut v = 0u64;
        for k in 0..8u32 {
            let a = addr + k;
            let b = if self.dpc_xbus {
                self.dmem[(a & 0xFFF) as usize]
            } else {
                *self.rdram.get(a as usize).unwrap_or(&0)
            };
            v = (v << 8) | b as u64;
        }
        v
    }

    /// Execute every complete RDP command between DPC_CURRENT and DPC_END.
    /// A command split across two DPC_END updates waits for the rest.
    pub fn dpc_run(&mut self) {
        let mut cur = self.dpc_current & 0x00FF_FFF8;
        let end = self.dpc_end & 0x00FF_FFF8;
        let mut words: Vec<u64> = Vec::with_capacity(22);
        while cur < end {
            let w0 = self.dpc_word(cur);
            let op = ((w0 >> 56) & 0x3F) as u8;
            let len = super::rdp::Rdp::cmd_len(op) as u32;
            if cur + len * 8 > end {
                break;
            }
            words.clear();
            for k in 0..len {
                words.push(self.dpc_word(cur + k * 8));
            }
            self.rdp_cmds += 1;
            let t0 = std::time::Instant::now();
            let rdp = &mut self.rdp;
            let full = rdp.command(&words, &mut self.rdram);
            self.prof_rdp_ns += t0.elapsed().as_nanos();
            if full {
                self.raise(MI_INTR_DP);
            }
            cur += len * 8;
        }
        self.dpc_current = cur;
    }

    /// RSP-side COP0 registers 0-15.
    pub fn rsp_cop0_read(&mut self, rd: usize) -> u32 {
        match rd {
            0 => self.sp_mem_addr,
            1 => self.sp_dram_addr,
            2 | 3 => 0,
            4 => self.sp_status_read(),
            5 => 0, // DMA_FULL
            6 => 0, // DMA_BUSY
            7 => self.sp_semaphore_read(),
            8 => self.dpc_start,
            9 => self.dpc_end,
            10 => self.dpc_current,
            11 => self.dpc_status_read(),
            _ => 0,
        }
    }

    pub fn rsp_cop0_write(&mut self, rd: usize, v: u32) {
        match rd {
            0 => self.sp_mem_addr = v & 0x1FFF,
            1 => self.sp_dram_addr = v & 0x00FF_FFFF,
            2 => self.sp_dma(false, v),
            3 => self.sp_dma(true, v),
            4 => self.sp_status_write(v),
            7 => self.sp_semaphore = false,
            8 => {
                self.dpc_start = v & 0x00FF_FFF8;
                self.dpc_current = self.dpc_start;
            },
            9 => {
                self.dpc_end = v & 0x00FF_FFF8;
                self.dpc_run();
            },
            11 => self.dpc_status_write(v),
            _ => {},
        }
    }

    /// CIC-NUS-6105 anti-piracy challenge: transform the 30 nibbles at PIF RAM
    /// 0x30-0x3E in place. Algorithm reimplemented from the ares emulator
    /// (ISC licence, ares/n64/cic/commands.cpp).
    fn challenge_6105(&mut self) {
        const LUT: [u8; 32] = [
            0x4, 0x7, 0xA, 0x7, 0xE, 0x5, 0xE, 0x1, 0xC, 0xF, 0x8, 0xF, 0x6, 0x3, 0x6, 0x9,
            0x4, 0x1, 0xA, 0x7, 0xE, 0x5, 0xE, 0x1, 0xC, 0x9, 0x8, 0x5, 0x6, 0x3, 0xC, 0x9,
        ];
        let mut key: u8 = 0xB;
        let mut sel: u8 = 0;
        for i in 0..30 {
            let bi = 0x30 + i / 2;
            let nib = if i % 2 == 0 { self.pif_ram[bi] >> 4 } else { self.pif_ram[bi] & 0xF };
            let data = key.wrapping_add(nib.wrapping_mul(5)) & 0xF;
            if i % 2 == 0 {
                self.pif_ram[bi] = (self.pif_ram[bi] & 0x0F) | (data << 4);
            } else {
                self.pif_ram[bi] = (self.pif_ram[bi] & 0xF0) | data;
            }
            key = LUT[((sel as usize) << 4) | data as usize];
            let mut m = data >> 3;
            let mut mag = data & 7;
            if m == 1 {
                mag = (!mag) & 7;
            }
            if mag % 3 != 1 {
                m = 1 - m;
            }
            if sel == 1 {
                if data == 0x1 || data == 0x9 {
                    m = 1;
                }
                if data == 0xB || data == 0xE {
                    m = 0;
                }
            }
            sel = m;
        }
    }

    // ── joybus / controller protocol over PIF RAM ────────────────────────────
    fn pif_process(&mut self) {
        let mut i = 0usize;
        let mut channel = 0usize;
        while i < 63 {
            let t = self.pif_ram[i];
            match t {
                0x00 => {
                    channel += 1;
                    i += 1;
                },
                0xFD | 0xFE => break, // end of commands
                0xFF => i += 1,       // channel reset/pad
                _ => {
                    let tx = (t & 0x3F) as usize;
                    if i + 1 >= 64 {
                        break;
                    }
                    let rx = (self.pif_ram[i + 1] & 0x3F) as usize;
                    let cmd_at = i + 2;
                    if cmd_at >= 64 {
                        break;
                    }
                    let cmd = self.pif_ram[cmd_at];
                    let resp_at = cmd_at + tx;
                    if channel >= 4 {
                        // cartridge accessory channel: 16-kbit EEPROM
                        match cmd {
                            0x00 | 0xFF => {
                                if resp_at + 2 < 64 {
                                    self.pif_ram[resp_at] = 0x00;
                                    self.pif_ram[resp_at + 1] = 0xC0; // 16Kb EEPROM
                                    self.pif_ram[resp_at + 2] = 0x00;
                                }
                            },
                            0x04 => {
                                // read 8-byte block
                                let block = self.pif_ram[cmd_at + 1] as usize * 8;
                                for k in 0..8 {
                                    if resp_at + k < 64 && block + k < self.eeprom.len() {
                                        self.pif_ram[resp_at + k] = self.eeprom[block + k];
                                    }
                                }
                            },
                            0x05 => {
                                // write 8-byte block
                                let block = self.pif_ram[cmd_at + 1] as usize * 8;
                                for k in 0..8 {
                                    if cmd_at + 2 + k < 64 && block + k < self.eeprom.len() {
                                        self.eeprom[block + k] = self.pif_ram[cmd_at + 2 + k];
                                    }
                                }
                                self.eeprom_dirty = true;
                                if resp_at < 64 {
                                    self.pif_ram[resp_at] = 0x00;
                                }
                            },
                            _ => {
                                self.pif_ram[i + 1] |= 0x80;
                            },
                        }
                        i = resp_at + rx;
                        channel += 1;
                        continue;
                    }
                    let pad = self.pads[channel];
                    match cmd {
                        0x00 | 0xFF => {
                            // info / reset: standard controller, no pak
                            if pad.present && resp_at + 2 < 64 {
                                self.pif_ram[resp_at] = 0x05;
                                self.pif_ram[resp_at + 1] = 0x00;
                                self.pif_ram[resp_at + 2] = 0x02;
                            } else if resp_at + 1 < 64 {
                                // mark channel absent
                                self.pif_ram[i + 1] |= 0x80;
                            }
                        },
                        0x01 => {
                            // controller state: buttons + stick
                            if resp_at + 3 < 64 {
                                let b = pad.buttons;
                                self.pif_ram[resp_at] = (b >> 8) as u8;
                                self.pif_ram[resp_at + 1] = b as u8;
                                self.pif_ram[resp_at + 2] = pad.stick_x as u8;
                                self.pif_ram[resp_at + 3] = pad.stick_y as u8;
                            }
                        },
                        _ => {
                            // unimplemented subcommand (pak reads etc.): error-flag it
                            self.pif_ram[i + 1] |= 0x80;
                        },
                    }
                    i = resp_at + rx;
                    channel += 1;
                },
            }
        }
    }
}

#[inline]
fn be32(b: &[u8], i: usize) -> u32 {
    u32::from_be_bytes([b[i], b[i + 1], b[i + 2], b[i + 3]])
}

#[inline]
fn set_be32(b: &mut [u8], i: usize, v: u32) {
    b[i..i + 4].copy_from_slice(&v.to_be_bytes());
}
