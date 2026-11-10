//! SNES PPU: register file, VRAM/OAM/CGRAM, and a scanline renderer.
//!
//! Renders BG modes 0-4 and 7, sprites (all 8 size pairs), the two windows
//! with per-layer logic, main/sub screen composition and colour math with
//! the hardware's clip/prevent/half rules. Not yet: hi-res modes 5/6,
//! offset-per-tile (2/4/6), direct colour, interlace — `unsupported()` names
//! whichever of those the game asks for.

pub const WIDTH: usize = 256;

const OBJ_SIZES: [((i32, i32), (i32, i32)); 8] = [
    ((8, 8), (16, 16)),
    ((8, 8), (32, 32)),
    ((8, 8), (64, 64)),
    ((16, 16), (32, 32)),
    ((16, 16), (64, 64)),
    ((32, 32), (64, 64)),
    ((16, 32), (32, 64)),
    ((16, 32), (32, 32)),
];

#[derive(Clone, Copy, PartialEq, Eq)]
enum L {
    Bg1,
    Bg2,
    Bg3,
    Bg4,
    Obj,
}

pub struct Ppu {
    pub vram: Vec<u16>,
    pub cgram: [u16; 256],
    pub oam: [u8; 544],

    pub inidisp: u8,
    obsel: u8,
    oamadd: u16,
    oam_addr: u16,
    oam_latch: u8,
    pub bgmode: u8,
    mosaic: u8,
    bgsc: [u8; 4],
    bg12nba: u8,
    bg34nba: u8,
    bghofs: [u16; 4],
    bgvofs: [u16; 4],
    bg_latch: u8,
    vmain: u8,
    vmadd: u16,
    vram_prefetch: u16,
    m7sel: u8,
    m7: [i16; 4],
    m7x: i16,
    m7y: i16,
    m7hofs: i16,
    m7vofs: i16,
    m7_latch: u8,
    cgadd: u8,
    cg_latch: u8,
    cg_second: bool,
    w12sel: u8,
    w34sel: u8,
    wobjsel: u8,
    wh: [u8; 4],
    wbglog: u8,
    wobjlog: u8,
    tm: u8,
    ts: u8,
    tmw: u8,
    tsw: u8,
    cgwsel: u8,
    cgadsub: u8,
    fixed: [u8; 3],
    pub setini: u8,
    // status / counters
    pub ophct: u16,
    pub opvct: u16,
    ophct_second: bool,
    opvct_second: bool,
    pub counter_latched: bool,
    pub pal: bool,
    pub fb: Vec<u32>,
    pub visible_lines: usize,
    pub frame_notes: u32, // bitmask of unsupported features seen
}

#[inline]
fn expand(c: u16) -> u32 {
    let r = (c & 31) as u32;
    let g = ((c >> 5) & 31) as u32;
    let b = ((c >> 10) & 31) as u32;
    let e = |v: u32| (v << 3) | (v >> 2);
    (e(r) << 16) | (e(g) << 8) | e(b)
}

impl Ppu {
    pub fn new(pal: bool) -> Ppu {
        Ppu {
            vram: vec![0; 32768],
            cgram: [0; 256],
            oam: [0; 544],
            inidisp: 0x80,
            obsel: 0,
            oamadd: 0,
            oam_addr: 0,
            oam_latch: 0,
            bgmode: 0,
            mosaic: 0,
            bgsc: [0; 4],
            bg12nba: 0,
            bg34nba: 0,
            bghofs: [0; 4],
            bgvofs: [0; 4],
            bg_latch: 0,
            vmain: 0,
            vmadd: 0,
            vram_prefetch: 0,
            m7sel: 0,
            m7: [0; 4],
            m7x: 0,
            m7y: 0,
            m7hofs: 0,
            m7vofs: 0,
            m7_latch: 0,
            cgadd: 0,
            cg_latch: 0,
            cg_second: false,
            w12sel: 0,
            w34sel: 0,
            wobjsel: 0,
            wh: [0; 4],
            wbglog: 0,
            wobjlog: 0,
            tm: 0,
            ts: 0,
            tmw: 0,
            tsw: 0,
            cgwsel: 0,
            cgadsub: 0,
            fixed: [0; 3],
            setini: 0,
            ophct: 0,
            opvct: 0,
            ophct_second: false,
            opvct_second: false,
            counter_latched: false,
            pal,
            fb: vec![0; WIDTH * 240],
            visible_lines: 224,
            frame_notes: 0,
        }
    }

    /// Debug: BG1's first 32x32 tilemap as text, assuming ASCII-indexed tiles
    /// (true of most homebrew test ROMs). Rows joined with newlines.
    pub fn bg1_text(&self) -> String {
        let base = ((self.bgsc[0] & 0xFC) as usize) << 8;
        let mut out = String::new();
        for row in 0..32 {
            for col in 0..32 {
                let e = self.vram[(base + row * 32 + col) & 0x7FFF];
                let c = (e & 0x3FF) as u8;
                out.push(if (32..127).contains(&c) { c as char } else { ' ' });
            }
            out.push('\n');
        }
        out
    }

    pub fn unsupported(&self) -> Option<String> {
        let mut v = Vec::new();
        if self.frame_notes & 1 != 0 {
            v.push("hi-res modes 5/6");
        }
        if self.frame_notes & 2 != 0 {
            v.push("offset-per-tile");
        }
        if self.frame_notes & 4 != 0 {
            v.push("direct colour");
        }
        if v.is_empty() {
            None
        } else {
            Some(v.join(", "))
        }
    }

    fn vram_inc(&self) -> u16 {
        match self.vmain & 3 {
            0 => 1,
            1 => 32,
            _ => 128,
        }
    }

    fn vram_addr(&self) -> usize {
        let a = self.vmadd;
        let r = match (self.vmain >> 2) & 3 {
            0 => a,
            1 => (a & 0xFF00) | ((a & 0x00E0) >> 5) | ((a & 0x001F) << 3),
            2 => (a & 0xFE00) | ((a & 0x01C0) >> 6) | ((a & 0x003F) << 3),
            _ => (a & 0xFC00) | ((a & 0x0380) >> 7) | ((a & 0x007F) << 3),
        };
        (r & 0x7FFF) as usize
    }

    pub fn oam_reload(&mut self) {
        self.oam_addr = (self.oamadd & 0x1FF) << 1;
    }

    // ── register writes ($2100-$2133) ────────────────────────────────────────
    pub fn write(&mut self, reg: u8, v: u8) {
        match reg {
            0x00 => self.inidisp = v,
            0x01 => self.obsel = v,
            0x02 => {
                self.oamadd = (self.oamadd & 0x8100) | v as u16;
                self.oam_reload();
            },
            0x03 => {
                self.oamadd = (self.oamadd & 0x00FF) | ((v as u16 & 1) << 8) | ((v as u16 & 0x80) << 8);
                self.oam_reload();
            },
            0x04 => {
                let a = self.oam_addr as usize;
                if a >= 512 {
                    self.oam[512 + (a & 31)] = v;
                } else if a & 1 == 0 {
                    self.oam_latch = v;
                } else {
                    self.oam[a - 1] = self.oam_latch;
                    self.oam[a] = v;
                }
                self.oam_addr = (self.oam_addr + 1) & 0x3FF;
            },
            0x05 => self.bgmode = v,
            0x06 => self.mosaic = v,
            0x07..=0x0A => self.bgsc[(reg - 7) as usize] = v,
            0x0B => self.bg12nba = v,
            0x0C => self.bg34nba = v,
            0x0D..=0x14 => {
                let bg = ((reg - 0x0D) >> 1) as usize;
                if reg & 1 == 1 {
                    // HOFS
                    self.bghofs[bg] =
                        ((v as u16) << 8) | (self.bg_latch as u16 & 0xF8) | ((self.bghofs[bg] >> 8) & 7);
                    self.bg_latch = v;
                    if bg == 0 {
                        self.m7hofs = (((v as i16) << 8) | self.m7_latch as i16) << 3 >> 3;
                        self.m7_latch = v;
                    }
                } else {
                    // VOFS
                    self.bgvofs[bg] = ((v as u16) << 8) | self.bg_latch as u16;
                    self.bg_latch = v;
                    if bg == 0 {
                        self.m7vofs = (((v as i16) << 8) | self.m7_latch as i16) << 3 >> 3;
                        self.m7_latch = v;
                    }
                }
            },
            0x15 => self.vmain = v,
            0x16 => {
                self.vmadd = (self.vmadd & 0xFF00) | v as u16;
                self.vram_prefetch = self.vram[self.vram_addr()];
            },
            0x17 => {
                self.vmadd = (self.vmadd & 0x00FF) | ((v as u16) << 8);
                self.vram_prefetch = self.vram[self.vram_addr()];
            },
            0x18 => {
                let a = self.vram_addr();
                self.vram[a] = (self.vram[a] & 0xFF00) | v as u16;
                if self.vmain & 0x80 == 0 {
                    self.vmadd = self.vmadd.wrapping_add(self.vram_inc());
                }
            },
            0x19 => {
                let a = self.vram_addr();
                self.vram[a] = (self.vram[a] & 0x00FF) | ((v as u16) << 8);
                if self.vmain & 0x80 != 0 {
                    self.vmadd = self.vmadd.wrapping_add(self.vram_inc());
                }
            },
            0x1A => self.m7sel = v,
            0x1B..=0x20 => {
                let val = ((v as i16) << 8) | self.m7_latch as i16;
                self.m7_latch = v;
                match reg {
                    0x1B => self.m7[0] = val,
                    0x1C => self.m7[1] = val,
                    0x1D => self.m7[2] = val,
                    0x1E => self.m7[3] = val,
                    0x1F => self.m7x = val << 3 >> 3,
                    _ => self.m7y = val << 3 >> 3,
                }
            },
            0x21 => {
                self.cgadd = v;
                self.cg_second = false;
            },
            0x22 => {
                if !self.cg_second {
                    self.cg_latch = v;
                } else {
                    self.cgram[self.cgadd as usize] = (((v & 0x7F) as u16) << 8) | self.cg_latch as u16;
                    self.cgadd = self.cgadd.wrapping_add(1);
                }
                self.cg_second = !self.cg_second;
            },
            0x23 => self.w12sel = v,
            0x24 => self.w34sel = v,
            0x25 => self.wobjsel = v,
            0x26..=0x29 => self.wh[(reg - 0x26) as usize] = v,
            0x2A => self.wbglog = v,
            0x2B => self.wobjlog = v,
            0x2C => self.tm = v,
            0x2D => self.ts = v,
            0x2E => self.tmw = v,
            0x2F => self.tsw = v,
            0x30 => self.cgwsel = v,
            0x31 => self.cgadsub = v,
            0x32 => {
                let val = v & 0x1F;
                if v & 0x20 != 0 {
                    self.fixed[0] = val;
                }
                if v & 0x40 != 0 {
                    self.fixed[1] = val;
                }
                if v & 0x80 != 0 {
                    self.fixed[2] = val;
                }
            },
            0x33 => {
                self.setini = v;
                self.visible_lines = if v & 4 != 0 { 239 } else { 224 };
            },
            _ => {},
        }
    }

    // ── register reads ($2134-$213F) ─────────────────────────────────────────
    pub fn read(&mut self, reg: u8, open: u8) -> u8 {
        match reg {
            0x34..=0x36 => {
                let r = (self.m7[0] as i32) * ((self.m7[1] >> 8) as i8 as i32);
                (r >> (8 * (reg - 0x34))) as u8
            },
            0x38 => {
                let a = self.oam_addr as usize;
                let v = if a >= 512 { self.oam[512 + (a & 31)] } else { self.oam[a] };
                self.oam_addr = (self.oam_addr + 1) & 0x3FF;
                v
            },
            0x39 => {
                let v = self.vram_prefetch as u8;
                if self.vmain & 0x80 == 0 {
                    self.vram_prefetch = self.vram[self.vram_addr()];
                    self.vmadd = self.vmadd.wrapping_add(self.vram_inc());
                }
                v
            },
            0x3A => {
                let v = (self.vram_prefetch >> 8) as u8;
                if self.vmain & 0x80 != 0 {
                    self.vram_prefetch = self.vram[self.vram_addr()];
                    self.vmadd = self.vmadd.wrapping_add(self.vram_inc());
                }
                v
            },
            0x3B => {
                let c = self.cgram[self.cgadd as usize];
                let v = if !self.cg_second { c as u8 } else { (c >> 8) as u8 & 0x7F };
                if self.cg_second {
                    self.cgadd = self.cgadd.wrapping_add(1);
                }
                self.cg_second = !self.cg_second;
                v
            },
            0x3C => {
                let v = if !self.ophct_second { self.ophct as u8 } else { (self.ophct >> 8) as u8 & 1 };
                self.ophct_second = !self.ophct_second;
                v
            },
            0x3D => {
                let v = if !self.opvct_second { self.opvct as u8 } else { (self.opvct >> 8) as u8 & 1 };
                self.opvct_second = !self.opvct_second;
                v
            },
            0x3E => 0x01 | (open & 0x10),            // PPU1 version 1
            0x3F => {
                let v = 0x02 | if self.pal { 0x10 } else { 0 } | if self.counter_latched { 0x40 } else { 0 } | (open & 0x20);
                self.counter_latched = false;
                self.ophct_second = false;
                self.opvct_second = false;
                v
            },
            _ => open,
        }
    }

    // ── scanline rendering ───────────────────────────────────────────────────
    fn window_masks(&self) -> [[bool; WIDTH]; 6] {
        let mut out = [[false; WIDTH]; 6];
        let sels = [
            self.w12sel & 0xF,
            self.w12sel >> 4,
            self.w34sel & 0xF,
            self.w34sel >> 4,
            self.wobjsel & 0xF,
            self.wobjsel >> 4,
        ];
        let logs = [
            self.wbglog & 3,
            (self.wbglog >> 2) & 3,
            (self.wbglog >> 4) & 3,
            (self.wbglog >> 6) & 3,
            self.wobjlog & 3,
            (self.wobjlog >> 2) & 3,
        ];
        for l in 0..6 {
            let s = sels[l];
            let (inv1, en1, inv2, en2) = (s & 1 != 0, s & 2 != 0, s & 4 != 0, s & 8 != 0);
            if !en1 && !en2 {
                continue;
            }
            for x in 0..WIDTH {
                let xv = x as u8;
                let w1 = (xv >= self.wh[0] && xv <= self.wh[1]) != inv1;
                let w2 = (xv >= self.wh[2] && xv <= self.wh[3]) != inv2;
                out[l][x] = match (en1, en2) {
                    (true, false) => w1,
                    (false, true) => w2,
                    _ => match logs[l] {
                        0 => w1 || w2,
                        1 => w1 && w2,
                        2 => w1 != w2,
                        _ => w1 == w2,
                    },
                };
            }
        }
        out
    }

    fn bg_depths(&self) -> [u32; 4] {
        match self.bgmode & 7 {
            0 => [2, 2, 2, 2],
            1 => [4, 4, 2, 0],
            2 => [4, 4, 0, 0],
            3 => [8, 4, 0, 0],
            4 => [8, 2, 0, 0],
            5 => [4, 2, 0, 0],
            6 => [4, 0, 0, 0],
            _ => [0, 0, 0, 0],
        }
    }

    fn render_bg(&self, bg: usize, bpp: u32, line: i32, out: &mut [u16; WIDTH]) {
        let sc = self.bgsc[bg];
        let map_base = ((sc & 0xFC) as usize) << 8;
        let size = sc & 3;
        let nba = if bg < 2 { self.bg12nba } else { self.bg34nba };
        let char_base = (((nba >> ((bg & 1) * 4)) & 0xF) as usize) << 12;
        let tile16 = (self.bgmode >> (4 + bg)) & 1 != 0;
        let tsz: i32 = if tile16 { 16 } else { 8 };
        let hofs = (self.bghofs[bg] & 0x3FF) as i32;
        let vofs = (self.bgvofs[bg] & 0x3FF) as i32;
        let msz = if self.mosaic & (1 << bg) != 0 { ((self.mosaic >> 4) + 1) as i32 } else { 1 };
        let pal_base: u16 = if self.bgmode & 7 == 0 { (bg as u16) * 32 } else { 0 };
        let wtiles = if size & 1 != 0 { 64 } else { 32 };
        let htiles = if size & 2 != 0 { 64 } else { 32 };
        let words_per_tile = (bpp * 4) as usize;

        let ey = line - ((line - 1).rem_euclid(msz));
        let py = ey + vofs;
        let mut last_tile_key = usize::MAX;
        let mut entry = 0u16;
        for x in 0..WIDTH as i32 {
            let ex = x - (x % msz);
            let px = ex + hofs;
            let tx = (px.rem_euclid(1024)) / tsz;
            let ty = (py.rem_euclid(1024)) / tsz;
            let mx = (tx & (wtiles - 1)) as usize;
            let my = (ty & (htiles - 1)) as usize;
            let screen = match size {
                0 => 0,
                1 => mx >> 5,
                2 => my >> 5,
                _ => ((my >> 5) << 1) | (mx >> 5),
            };
            let key = (screen << 12) | ((my & 31) << 5) | (mx & 31);
            if key != last_tile_key {
                entry = self.vram[(map_base + screen * 0x400 + (my & 31) * 32 + (mx & 31)) & 0x7FFF];
                last_tile_key = key;
            }
            let mut c = (entry & 0x3FF) as usize;
            let pal = ((entry >> 10) & 7) as u16;
            let prio = (entry >> 13) & 1;
            let hf = entry & 0x4000 != 0;
            let vf = entry & 0x8000 != 0;
            let mut fx = px.rem_euclid(tsz);
            let mut fy = py.rem_euclid(tsz);
            if hf {
                fx = tsz - 1 - fx;
            }
            if vf {
                fy = tsz - 1 - fy;
            }
            if tile16 {
                c += (fx / 8) as usize + (fy / 8) as usize * 16;
                fx %= 8;
                fy %= 8;
            }
            let base = (char_base + (c & 0x3FF) * words_per_tile + fy as usize) & 0x7FFF;
            let bit = 7 - fx as u32;
            let w0 = self.vram[base];
            let mut col = ((w0 >> bit) & 1) | (((w0 >> (8 + bit)) & 1) << 1);
            if bpp >= 4 {
                let w1 = self.vram[(base + 8) & 0x7FFF];
                col |= (((w1 >> bit) & 1) << 2) | (((w1 >> (8 + bit)) & 1) << 3);
            }
            if bpp == 8 {
                let w2 = self.vram[(base + 16) & 0x7FFF];
                let w3 = self.vram[(base + 24) & 0x7FFF];
                col |= (((w2 >> bit) & 1) << 4)
                    | (((w2 >> (8 + bit)) & 1) << 5)
                    | (((w3 >> bit) & 1) << 6)
                    | (((w3 >> (8 + bit)) & 1) << 7);
            }
            out[x as usize] = if col == 0 {
                0
            } else {
                let idx = match bpp {
                    2 => pal_base + pal * 4 + col,
                    4 => pal * 16 + col,
                    _ => col,
                };
                0x100 | (prio << 9) | (idx & 0xFF)
            };
        }
    }

    fn render_mode7(&self, line: i32, out: &mut [u16; WIDTH]) {
        let a = self.m7[0] as i32;
        let b = self.m7[1] as i32;
        let c = self.m7[2] as i32;
        let d = self.m7[3] as i32;
        let hflip = self.m7sel & 1 != 0;
        let vflip = self.m7sel & 2 != 0;
        let mode = self.m7sel >> 6;
        let clip = |n: i32| if n & 0x2000 != 0 { n | !1023 } else { n & 1023 };
        let cx = self.m7x as i32;
        let cy = self.m7y as i32;
        let ho = self.m7hofs as i32;
        let vo = self.m7vofs as i32;
        let y = if vflip { 255 - line } else { line };
        let origin_x = ((a * clip(ho - cx)) & !63)
            + ((b * clip(vo - cy)) & !63)
            + ((b * y) & !63)
            + (cx << 8);
        let origin_y = ((c * clip(ho - cx)) & !63)
            + ((d * clip(vo - cy)) & !63)
            + ((d * y) & !63)
            + (cy << 8);
        for x in 0..WIDTH as i32 {
            let mx = if hflip { 255 - x } else { x };
            let px = (origin_x + a * mx) >> 8;
            let py = (origin_y + c * mx) >> 8;
            let outside = (px | py) & !1023 != 0;
            let v = if outside && mode == 2 {
                0
            } else {
                let (tx, ty, fx, fy) = ((px >> 3) & 127, (py >> 3) & 127, px & 7, py & 7);
                let tile = if outside && mode == 3 {
                    0
                } else {
                    (self.vram[(ty * 128 + tx) as usize] & 0xFF) as usize
                };
                (self.vram[(tile * 64 + (fy * 8 + fx) as usize) & 0x7FFF] >> 8) as u16
            };
            out[x as usize] = if v == 0 { 0 } else { 0x100 | v };
        }
    }

    /// Sprites for `line` → u32 per pixel: 0 transparent, else
    /// 0x100 | cg index | prio<<9 | math_ok<<11.
    fn render_obj(&self, line: i32, out: &mut [u32; WIDTH]) {
        let (small, large) = OBJ_SIZES[(self.obsel >> 5) as usize];
        let name_base = ((self.obsel & 7) as usize) << 13;
        let name_gap = ((((self.obsel >> 3) & 3) as usize) + 1) << 12;
        let first = if self.oamadd & 0x8000 != 0 { ((self.oamadd >> 1) & 0x7F) as usize } else { 0 };

        let mut list: [usize; 32] = [0; 32];
        let mut n = 0;
        for k in 0..128 {
            let i = (first + k) & 0x7F;
            let hi = self.oam[512 + (i >> 2)] >> ((i & 3) * 2);
            let (w, h) = if hi & 2 != 0 { large } else { small };
            let y = self.oam[i * 4 + 1] as i32;
            let dy = (line - (y + 1)) & 0xFF;
            if dy < h {
                let x = self.oam[i * 4] as i32 | (((hi & 1) as i32) << 8);
                let x = if x >= 256 { x - 512 } else { x };
                if x > -w && x < 256 {
                    if n == 32 {
                        break;
                    }
                    list[n] = i;
                    n += 1;
                }
            }
        }
        for k in (0..n).rev() {
            let i = list[k];
            let hi = self.oam[512 + (i >> 2)] >> ((i & 3) * 2);
            let (w, h) = if hi & 2 != 0 { large } else { small };
            let x0 = {
                let x = self.oam[i * 4] as i32 | (((hi & 1) as i32) << 8);
                if x >= 256 { x - 512 } else { x }
            };
            let y = self.oam[i * 4 + 1] as i32;
            let tile = self.oam[i * 4 + 2] as usize;
            let attr = self.oam[i * 4 + 3];
            let pal = ((attr >> 1) & 7) as u32;
            let prio = ((attr >> 4) & 3) as u32;
            let hflip = attr & 0x40 != 0;
            let vflip = attr & 0x80 != 0;
            let dy = (line - (y + 1)) & 0xFF;
            let row = if vflip { h - 1 - dy } else { dy };
            let ty = (row / 8) as usize;
            let fy = (row % 8) as usize;
            let wt = (w / 8) as usize;
            for xt in 0..wt {
                let txc = if hflip { wt - 1 - xt } else { xt };
                let tile_index = ((tile & 0xF0) + (ty << 4)) & 0xF0 | (((tile & 0x0F) + txc) & 0x0F);
                let mut base = name_base + tile_index * 16;
                if attr & 1 != 0 {
                    base += name_gap;
                }
                let base = (base + fy) & 0x7FFF;
                let w0 = self.vram[base];
                let w1 = self.vram[(base + 8) & 0x7FFF];
                for pc in 0..8i32 {
                    let sx = x0 + (xt as i32) * 8 + pc;
                    if !(0..256).contains(&sx) {
                        continue;
                    }
                    let bit = if hflip { pc as u32 } else { 7 - pc as u32 };
                    let col = ((w0 >> bit) & 1)
                        | (((w0 >> (8 + bit)) & 1) << 1)
                        | (((w1 >> bit) & 1) << 2)
                        | (((w1 >> (8 + bit)) & 1) << 3);
                    if col != 0 {
                        let idx = 128 + pal * 16 + col as u32;
                        out[sx as usize] = 0x100 | idx | (prio << 9) | (((pal >= 4) as u32) << 11);
                    }
                }
            }
        }
    }

    fn priority_list(&self) -> &'static [(L, u8)] {
        use L::*;
        match self.bgmode & 7 {
            0 => &[
                (Obj, 3), (Bg1, 1), (Bg2, 1), (Obj, 2), (Bg1, 0), (Bg2, 0),
                (Obj, 1), (Bg3, 1), (Bg4, 1), (Obj, 0), (Bg3, 0), (Bg4, 0),
            ],
            1 => {
                if self.bgmode & 8 != 0 {
                    &[
                        (Bg3, 1), (Obj, 3), (Bg1, 1), (Bg2, 1), (Obj, 2), (Bg1, 0),
                        (Bg2, 0), (Obj, 1), (Obj, 0), (Bg3, 0),
                    ]
                } else {
                    &[
                        (Obj, 3), (Bg1, 1), (Bg2, 1), (Obj, 2), (Bg1, 0), (Bg2, 0),
                        (Obj, 1), (Bg3, 1), (Obj, 0), (Bg3, 0),
                    ]
                }
            },
            7 => &[(Obj, 3), (Obj, 2), (Obj, 1), (Bg1, 0), (Obj, 0)],
            _ => &[(Obj, 3), (Bg1, 1), (Obj, 2), (Bg2, 1), (Obj, 1), (Bg1, 0), (Obj, 0), (Bg2, 0)],
        }
    }

    /// Render visible line `line` (1-based) into the framebuffer.
    pub fn render_line(&mut self, line: usize) {
        if line == 0 || line > self.visible_lines {
            return;
        }
        let row = line - 1;
        let li = line as i32;
        if self.inidisp & 0x80 != 0 {
            for x in 0..WIDTH {
                self.fb[row * WIDTH + x] = 0;
            }
            return;
        }
        let mode = self.bgmode & 7;
        if mode == 5 || mode == 6 || self.setini & 8 != 0 {
            self.frame_notes |= 1;
        }
        if mode == 2 || mode == 4 || mode == 6 {
            self.frame_notes |= 2;
        }
        if self.cgwsel & 1 != 0 && (mode == 3 || mode == 4 || mode == 7) {
            self.frame_notes |= 4;
        }

        let depths = self.bg_depths();
        let mut bgpix = [[0u16; WIDTH]; 4];
        let enabled = self.tm | self.ts;
        if mode == 7 {
            if enabled & 1 != 0 {
                self.render_mode7(li, &mut bgpix[0]);
            }
        } else {
            for bg in 0..4 {
                if depths[bg] != 0 && enabled & (1 << bg) != 0 {
                    self.render_bg(bg, depths[bg], li, &mut bgpix[bg]);
                }
            }
        }
        let mut objpix = [0u32; WIDTH];
        if enabled & 0x10 != 0 {
            self.render_obj(li, &mut objpix);
        }
        let win = self.window_masks();
        let order = self.priority_list();
        let cgram = self.cgram;
        let backdrop = cgram[0];
        let fixed = (self.fixed[0] as u16) | ((self.fixed[1] as u16) << 5) | ((self.fixed[2] as u16) << 10);
        let bright = (self.inidisp & 15) as u32;

        // pick the topmost visible layer pixel for a screen
        let pick = |x: usize, layer_en: u8, win_en: u8| -> Option<(u16, u8, bool)> {
            for &(l, p) in order {
                let (bit, wl) = match l {
                    L::Bg1 => (1u8, 0),
                    L::Bg2 => (2, 1),
                    L::Bg3 => (4, 2),
                    L::Bg4 => (8, 3),
                    L::Obj => (0x10, 4),
                };
                if layer_en & bit == 0 {
                    continue;
                }
                if win_en & bit != 0 && win[wl][x] {
                    continue;
                }
                match l {
                    L::Obj => {
                        let o = objpix[x];
                        if o != 0 && ((o >> 9) & 3) as u8 == p {
                            return Some((cgram[(o & 0xFF) as usize], 4, o & 0x800 != 0));
                        }
                    },
                    _ => {
                        let idx = match l {
                            L::Bg1 => 0,
                            L::Bg2 => 1,
                            L::Bg3 => 2,
                            _ => 3,
                        };
                        let v = bgpix[idx][x];
                        if v != 0 && ((v >> 9) & 1) as u8 == p {
                            return Some((cgram[(v & 0xFF) as usize], idx as u8, true));
                        }
                    },
                }
            }
            None
        };

        for x in 0..WIDTH {
            let main = pick(x, self.tm, self.tmw);
            let (mut mc, mlayer, mobj_ok) = match main {
                Some((c, l, ok)) => (c, l, ok),
                None => (backdrop, 5, true),
            };
            // colour math
            let cwin = win[5][x];
            let region = |sel: u8| match sel {
                0 => false,
                1 => !cwin,
                2 => cwin,
                _ => true,
            };
            if region(self.cgwsel >> 6) {
                mc = 0;
            }
            let prevent = region((self.cgwsel >> 4) & 3);
            let layer_bit = match mlayer {
                0..=3 => 1u8 << mlayer,
                4 => 0x10,
                _ => 0x20,
            };
            let math_on = !prevent && self.cgadsub & layer_bit != 0 && (mlayer != 4 || mobj_ok);
            let mut out15 = mc;
            if math_on {
                let (operand, sub_is_layer) = if self.cgwsel & 2 != 0 {
                    match pick(x, self.ts, self.tsw) {
                        Some((c, _, _)) => (c, true),
                        None => (fixed, false),
                    }
                } else {
                    (fixed, false)
                };
                let halve = self.cgadsub & 0x40 != 0 && (self.cgwsel & 2 == 0 || sub_is_layer);
                let subtract = self.cgadsub & 0x80 != 0;
                let mut res = 0u16;
                for ch in 0..3 {
                    let a = ((mc >> (5 * ch)) & 31) as i32;
                    let b = ((operand >> (5 * ch)) & 31) as i32;
                    let mut r = if subtract { (a - b).max(0) } else { a + b };
                    if halve {
                        r >>= 1;
                    }
                    r = r.min(31);
                    res |= (r as u16) << (5 * ch);
                }
                out15 = res;
            }
            let mut rgb = expand(out15);
            if bright < 15 {
                let f = |v: u32| (v * bright) / 15;
                rgb = (f((rgb >> 16) & 255) << 16) | (f((rgb >> 8) & 255) << 8) | f(rgb & 255);
            }
            self.fb[row * WIDTH + x] = rgb;
        }
    }
}
