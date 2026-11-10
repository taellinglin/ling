//! RDP — the N64's rasterizer, in software.
//!
//! Consumes the command stream the RSP microcode emits (via the DPC FIFO) and
//! draws into RDRAM: edge-walked triangles with Gouraud shading, perspective
//! texturing and Z, texture/fill rectangles, TMEM loads (tile/block/TLUT),
//! the two-cycle colour combiner, the blender (fog + alpha blend), alpha
//! compare and the compressed 14-bit Z format.
//!
//! Image-quality enhancements over real hardware (deliberate):
//! * **24-bit colour shadow** — every 16-bit framebuffer write also stores
//!   the full 8-bit-per-channel colour in [`Rdp::hi`]. Blending reads it back
//!   and the VI scans it out whenever it still agrees with RDRAM, so the
//!   image has none of the 5-bit banding/dither of a real console. CPU writes
//!   to the framebuffer simply fall back to the RDRAM value.
//! * **Analytic edge coverage** — with AA enabled, edge pixels get an exact
//!   area coverage (4 sub-scanlines x continuous horizontal overlap,
//!   quantised to the hardware's 0..8). Coverage is stored beside the shadow
//!   colour, adjacent triangles of one surface sum their coverage (no seams),
//!   and the VI's anti-alias filter blends partially covered pixels with the
//!   background estimated from their neighbours, as the console does.
//!
//! Deliberate simplifications (named so nobody mistakes them for accuracy):
//! bilinear instead of the
//! 3-point triangle filter, YUV textures unsupported.

#[allow(dead_code)]
const FMT_RGBA: u8 = 0;
const FMT_CI: u8 = 2;
const FMT_IA: u8 = 3;
const FMT_I: u8 = 4;

#[derive(Clone, Copy, Default)]
struct Tile {
    fmt: u8,
    size: u8,
    line: u32,  // bytes per row in TMEM
    tmem: u32,  // byte address in TMEM
    palette: u8,
    ct: bool,
    mt: bool,
    mask_t: u32,
    shift_t: u32,
    cs: bool,
    ms: bool,
    mask_s: u32,
    shift_s: u32,
    sl: i32, // 10.2
    tl: i32,
    sh: i32,
    th: i32,
}

#[derive(Clone, Copy, Default)]
struct Img {
    fmt: u8,
    size: u8,
    width: u32,
    addr: u32,
}

#[derive(Clone, Copy, Default)]
struct Modes {
    cycle: u8,
    persp: bool,
    tlut_en: bool,
    tlut_ia: bool,
    bilerp: bool,
    blend_m1a: [u8; 2],
    blend_m1b: [u8; 2],
    blend_m2a: [u8; 2],
    blend_m2b: [u8; 2],
    force_blend: bool,
    alpha_cvg_sel: bool,
    z_mode: u8,
    image_read: bool,
    z_update: bool,
    z_compare: bool,
    z_prim: bool,
    alpha_compare: bool,
    aa_en: bool,
    tex_lod_en: bool,
    sharpen: bool,
    detail: bool,
}

#[derive(Clone, Copy, Default)]
struct Combine {
    rgb_a: [u8; 2],
    rgb_b: [u8; 2],
    rgb_c: [u8; 2],
    rgb_d: [u8; 2],
    a_a: [u8; 2],
    a_b: [u8; 2],
    a_c: [u8; 2],
    a_d: [u8; 2],
}

pub struct Rdp {
    tmem: Box<[u8; 4096]>,
    tiles: [Tile; 8],
    color_img: Img,
    tex_img: Img,
    z_addr: u32,
    modes: Modes,
    comb: Combine,
    fill: u32,
    fog: [i32; 4],
    blend: [i32; 4],
    prim: [i32; 4],
    env: [i32; 4],
    prim_lod: i32,
    /// LOD fraction of the pixel being shaded (combiner input).
    lod_frac: i32,
    /// Per-triangle: max mip level and major-edge slope (for d/dy).
    tri_level: usize,
    tri_dxh: f32,
    recent_cimg: [u32; 4],
    last_load_fb: bool,
    prim_z: u32,
    scissor: (i32, i32, i32, i32), // x0, y0, x1, y1 in pixels (exclusive end)
    pub triangles: u64,
    pub rects: u64,
    pub pixels: u64,
    /// Shadow of every 16-bit framebuffer pixel, indexed by RDRAM address/2:
    /// bit 31 valid, bits 27..24 coverage 0..8, bits 23..0 RGB888.
    pub hi: Vec<u32>,
    /// `LING_N64_NOAA` set: ignore AA_EN (hard-edged, for A/B comparison).
    no_aa: bool,
    /// `N64_NOFOG` (debug): fog-colour blends use factor 0.
    no_fog: bool,
    no_lod: bool,
}

const HI_VALID: u32 = 1 << 31;

/// Debug: texel-format usage histogram, index = tlut*32 + size*8 + fmt.
pub static TEX_STATS: [std::sync::atomic::AtomicU64; 64] = [const { std::sync::atomic::AtomicU64::new(0) }; 64];
/// Debug: fog factor (shade alpha >> 5) histogram for fog-blended pixels;
/// [8] = pixels whose blender reads fog colour in a cycle that never runs.
pub static FOG_STATS: [std::sync::atomic::AtomicU64; 9] = [const { std::sync::atomic::AtomicU64::new(0) }; 9];
/// Debug: [0] pixels force-blended with b=1.0 (additive), [1] force-blended
/// otherwise, [2] 2-cycle pixels whose 2nd combiner cycle reads TEXEL0/1.
pub static MISC_STATS: [std::sync::atomic::AtomicU64; 4] = [const { std::sync::atomic::AtomicU64::new(0) }; 4];
/// Debug: [0] TMEM loads sourced from a recent colour image (framebuffer
/// read-back: motion blur, transitions), [1] of those while 1-cycle force-blend.
pub static FB_STATS: [std::sync::atomic::AtomicU64; 2] = [const { std::sync::atomic::AtomicU64::new(0) }; 2];
pub static OP_STATS: [std::sync::atomic::AtomicU64; 64] = [const { std::sync::atomic::AtomicU64::new(0) }; 64];
/// Debug: textured pixels by LOD mode: [off, detail, sharpen, plain mip].
pub static LOD_STATS: [std::sync::atomic::AtomicU64; 4] = [const { std::sync::atomic::AtomicU64::new(0) }; 4];
pub static STATS_ON: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

#[inline]
fn q5551(rgb: u32) -> u16 {
    ((((rgb >> 19) & 31) << 11) | (((rgb >> 11) & 31) << 6) | (((rgb >> 3) & 31) << 1)) as u16
}

impl Rdp {
    /// Full-precision colour + coverage for the 16-bit pixel at RDRAM `addr`
    /// whose raw value is `raw`, if the shadow still matches it.
    #[inline]
    pub fn hi_lookup(&self, addr: usize, raw: u16) -> Option<(u32, u32)> {
        let h = *self.hi.get(addr >> 1)?;
        if h & HI_VALID != 0 && q5551(h & 0xFF_FFFF) == raw & 0xFFFE {
            Some((h & 0xFF_FFFF, (h >> 24) & 15))
        } else {
            None
        }
    }

    pub fn fog_color(&self) -> [i32; 4] {
        self.fog
    }

    #[inline]
    fn hi_store(&mut self, addr: usize, rgb: u32, cvg: u32) {
        if let Some(h) = self.hi.get_mut(addr >> 1) {
            *h = HI_VALID | (cvg.min(8) << 24) | (rgb & 0xFF_FFFF);
        }
    }
}

#[inline]
fn sext(v: u64, bits: u32) -> i32 {
    let s = 64 - bits;
    (((v << s) as i64) >> s) as i32
}

#[inline]
fn field(w: u64, hi: u32, lo: u32) -> u32 {
    ((w >> lo) & ((1u64 << (hi - lo + 1)) - 1)) as u32
}

/// 18-bit linear Z → the RDP's 14-bit floating Z (3-bit exponent, 11-bit mantissa).
fn z_compress(z: u32) -> u16 {
    let z = z.min(0x3FFFF);
    let (e, m) = if z < 0x20000 {
        (0, z >> 6)
    } else if z < 0x30000 {
        (1, z >> 5)
    } else if z < 0x38000 {
        (2, z >> 4)
    } else if z < 0x3C000 {
        (3, z >> 3)
    } else if z < 0x3E000 {
        (4, z >> 2)
    } else if z < 0x3F000 {
        (5, z >> 1)
    } else if z < 0x3F800 {
        (6, z)
    } else {
        (7, z)
    };
    ((e << 11) | (m & 0x7FF)) as u16
}

#[inline]
fn rgba5551(p: u16) -> [i32; 4] {
    let e = |v: u16| ((v << 3) | (v >> 2)) as i32;
    [e((p >> 11) & 31), e((p >> 6) & 31), e((p >> 1) & 31), if p & 1 != 0 { 255 } else { 0 }]
}

/// Interpolated attribute set at a pixel.
#[derive(Clone, Copy, Default)]
struct Attrs {
    shade: [f32; 4],
    s: f32,
    t: f32,
    w: f32,
    z: f32,
}

#[derive(Clone, Copy, Default)]
struct Grad {
    v: Attrs,
    dx: Attrs,
    de: Attrs,
}

impl Attrs {
    #[inline]
    fn madd(&self, d: &Attrs, k: f32) -> Attrs {
        Attrs {
            shade: [
                self.shade[0] + d.shade[0] * k,
                self.shade[1] + d.shade[1] * k,
                self.shade[2] + d.shade[2] * k,
                self.shade[3] + d.shade[3] * k,
            ],
            s: self.s + d.s * k,
            t: self.t + d.t * k,
            w: self.w + d.w * k,
            z: self.z + d.z * k,
        }
    }
}

impl Rdp {
    pub fn new() -> Rdp {
        Rdp {
            tmem: Box::new([0; 4096]),
            tiles: [Tile::default(); 8],
            color_img: Img { fmt: 0, size: 2, width: 320, addr: 0 },
            tex_img: Img::default(),
            z_addr: 0,
            modes: Modes::default(),
            comb: Combine::default(),
            fill: 0,
            fog: [0; 4],
            blend: [0; 4],
            prim: [0; 4],
            env: [0; 4],
            prim_lod: 0,
            lod_frac: 0,
            tri_level: 0,
            tri_dxh: 0.0,
            recent_cimg: [0; 4],
            last_load_fb: false,
            prim_z: 0x3FFFF,
            scissor: (0, 0, 320, 240),
            triangles: 0,
            rects: 0,
            pixels: 0,
            hi: vec![0; 4 * 1024 * 1024],
            no_aa: std::env::var_os("LING_N64_NOAA").is_some(),
            no_fog: std::env::var_os("N64_NOFOG").is_some(),
            no_lod: std::env::var_os("N64_NOLOD").is_some(),
        }
    }

    /// Length in 64-bit words of the command whose opcode is `op`.
    pub fn cmd_len(op: u8) -> usize {
        match op {
            0x08..=0x0F => {
                4 + if op & 4 != 0 { 8 } else { 0 } + if op & 2 != 0 { 8 } else { 0 } + if op & 1 != 0 { 2 } else { 0 }
            },
            0x24 | 0x25 => 2,
            _ => 1,
        }
    }

    /// Execute one complete command. Returns true for Sync Full.
    pub fn command(&mut self, w: &[u64], ram: &mut [u8]) -> bool {
        let w0 = w[0];
        let op = ((w0 >> 56) & 0x3F) as u8;
        if STATS_ON.load(std::sync::atomic::Ordering::Relaxed) {
            OP_STATS[op as usize].fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        }
        match op {
            0x08..=0x0F => self.triangle(op, w, ram),
            0x24 | 0x25 => self.tex_rect(w0, w[1], op == 0x25, ram),
            0x29 => return true,
            0x2D => {
                self.scissor = (
                    field(w0, 55, 44) as i32 >> 2,
                    field(w0, 43, 32) as i32 >> 2,
                    (field(w0, 23, 12) as i32 + 3) >> 2,
                    (field(w0, 11, 0) as i32 + 3) >> 2,
                );
            },
            0x2E => self.prim_z = field(w0, 31, 16) << 3,
            0x2F => {
                let m = &mut self.modes;
                m.cycle = field(w0, 53, 52) as u8;
                m.persp = w0 >> 51 & 1 != 0;
                m.detail = w0 >> 50 & 1 != 0;
                m.sharpen = w0 >> 49 & 1 != 0;
                m.tex_lod_en = w0 >> 48 & 1 != 0;
                m.tlut_en = w0 >> 47 & 1 != 0;
                m.tlut_ia = w0 >> 46 & 1 != 0;
                m.bilerp = w0 >> 45 & 1 != 0;
                m.blend_m1a = [field(w0, 31, 30) as u8, field(w0, 29, 28) as u8];
                m.blend_m1b = [field(w0, 27, 26) as u8, field(w0, 25, 24) as u8];
                m.blend_m2a = [field(w0, 23, 22) as u8, field(w0, 21, 20) as u8];
                m.blend_m2b = [field(w0, 19, 18) as u8, field(w0, 17, 16) as u8];
                m.force_blend = w0 >> 14 & 1 != 0;
                m.alpha_cvg_sel = w0 >> 13 & 1 != 0;
                m.z_mode = field(w0, 11, 10) as u8;
                m.image_read = w0 >> 6 & 1 != 0;
                m.z_update = w0 >> 5 & 1 != 0;
                m.z_compare = w0 >> 4 & 1 != 0;
                m.z_prim = w0 >> 2 & 1 != 0;
                m.alpha_compare = w0 & 1 != 0;
                m.aa_en = w0 >> 3 & 1 != 0 && !self.no_aa;
            },
            0x30 => self.load_tlut(w0, ram),
            0x32 => {
                let t = &mut self.tiles[field(w0, 26, 24) as usize];
                t.sl = field(w0, 55, 44) as i32;
                t.tl = field(w0, 43, 32) as i32;
                t.sh = field(w0, 23, 12) as i32;
                t.th = field(w0, 11, 0) as i32;
            },
            0x33 => self.load_block(w0, ram),
            0x34 => self.load_tile(w0, ram),
            0x35 => {
                let t = &mut self.tiles[field(w0, 26, 24) as usize];
                t.fmt = field(w0, 55, 53) as u8;
                t.size = field(w0, 52, 51) as u8;
                t.line = field(w0, 49, 41) * 8;
                t.tmem = field(w0, 40, 32) * 8;
                t.palette = field(w0, 23, 20) as u8;
                t.ct = w0 >> 19 & 1 != 0;
                t.mt = w0 >> 18 & 1 != 0;
                t.mask_t = field(w0, 17, 14);
                t.shift_t = field(w0, 13, 10);
                t.cs = w0 >> 9 & 1 != 0;
                t.ms = w0 >> 8 & 1 != 0;
                t.mask_s = field(w0, 7, 4);
                t.shift_s = field(w0, 3, 0);
            },
            0x36 => self.fill_rect(w0, ram),
            0x37 => self.fill = w0 as u32,
            0x38 => self.fog = rgba32(w0 as u32),
            0x39 => self.blend = rgba32(w0 as u32),
            0x3A => {
                self.prim = rgba32(w0 as u32);
                self.prim_lod = field(w0, 39, 32) as i32;
            },
            0x3B => self.env = rgba32(w0 as u32),
            0x3C => {
                let c = &mut self.comb;
                c.rgb_a = [field(w0, 55, 52) as u8, field(w0, 40, 37) as u8];
                c.rgb_c = [field(w0, 51, 47) as u8, field(w0, 36, 32) as u8];
                c.a_a = [field(w0, 46, 44) as u8, field(w0, 23, 21) as u8];
                c.a_c = [field(w0, 43, 41) as u8, field(w0, 20, 18) as u8];
                c.rgb_b = [field(w0, 31, 28) as u8, field(w0, 27, 24) as u8];
                c.rgb_d = [field(w0, 17, 15) as u8, field(w0, 8, 6) as u8];
                c.a_b = [field(w0, 14, 12) as u8, field(w0, 5, 3) as u8];
                c.a_d = [field(w0, 11, 9) as u8, field(w0, 2, 0) as u8];
            },
            0x3D => {
                self.tex_img = Img {
                    fmt: field(w0, 55, 53) as u8,
                    size: field(w0, 52, 51) as u8,
                    width: field(w0, 41, 32) + 1,
                    addr: field(w0, 23, 0),
                }
            },
            0x3E => self.z_addr = field(w0, 23, 0),
            0x3F => {
                let a = field(w0, 23, 0);
                if field(w0, 52, 51) == 2 && field(w0, 41, 32) + 1 == 320 && !self.recent_cimg.contains(&a) {
                    self.recent_cimg.rotate_right(1);
                    self.recent_cimg[0] = a;
                }
                self.color_img = Img {
                    fmt: field(w0, 55, 53) as u8,
                    size: field(w0, 52, 51) as u8,
                    width: field(w0, 41, 32) + 1,
                    addr: field(w0, 23, 0),
                }
            },
            _ => {},
        }
        false
    }

    // ── TMEM loads ───────────────────────────────────────────────────────────
    fn note_fb_load(&mut self) {
        if STATS_ON.load(std::sync::atomic::Ordering::Relaxed) {
            let a = self.tex_img.addr;
            self.last_load_fb = self.recent_cimg.iter().any(|&c| c != 0 && a >= c && a < c + 320 * 240 * 2);
            if self.last_load_fb {
                FB_STATS[0].fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            }
        }
    }

    fn load_tile(&mut self, w0: u64, ram: &[u8]) {
        self.note_fb_load();
        let ti = field(w0, 26, 24) as usize;
        let sl = field(w0, 55, 44) as i32 >> 2;
        let tl = field(w0, 43, 32) as i32 >> 2;
        let sh = field(w0, 23, 12) as i32 >> 2;
        let th = field(w0, 11, 0) as i32 >> 2;
        {
            let t = &mut self.tiles[ti];
            t.sl = field(w0, 55, 44) as i32;
            t.tl = field(w0, 43, 32) as i32;
            t.sh = field(w0, 23, 12) as i32;
            t.th = field(w0, 11, 0) as i32;
        }
        let tile = self.tiles[ti];
        let img = self.tex_img;
        if img.size == 3 {
            // RGBA32: split R,G → low half, B,A → high half
            for t in tl..=th {
                for s in sl..=sh {
                    let src = (img.addr as usize) + ((t as usize * img.width as usize + s as usize) * 4);
                    let xr = if (t - tl) & 1 == 1 { 4 } else { 0 };
                    let dst = ((tile.tmem + (t - tl) as u32 * tile.line + (s - sl) as u32 * 2) as usize ^ xr) & 0x7FF;
                    for k in 0..2 {
                        self.tmem[dst + k] = *ram.get(src + k).unwrap_or(&0);
                        self.tmem[(dst + k) | 0x800] = *ram.get(src + 2 + k).unwrap_or(&0);
                    }
                }
            }
            return;
        }
        // byte-granular copy: 4b images are loaded two texels per byte
        let (num, den) = match img.size {
            0 => (1usize, 2usize),
            1 => (1, 1),
            _ => (2, 1),
        };
        let row_bytes = (((sh - sl + 1) as usize) * num + den - 1) / den;
        for t in tl..=th {
            let src_row = img.addr as usize + (t as usize * img.width as usize * num) / den + (sl as usize * num) / den;
            let dst_row = tile.tmem as usize + (t - tl) as usize * tile.line as usize;
            let xr = if (t - tl) & 1 == 1 { 4 } else { 0 };
            for b in 0..row_bytes {
                let v = *ram.get(src_row + b).unwrap_or(&0);
                self.tmem[((dst_row + b) ^ xr) & 0xFFF] = v;
            }
        }
    }

    fn load_block(&mut self, w0: u64, ram: &[u8]) {
        self.note_fb_load();
        let ti = field(w0, 26, 24) as usize;
        let sl = field(w0, 55, 44) as usize;
        let tl = field(w0, 43, 32) as usize;
        let sh = field(w0, 23, 12) as usize;
        let dxt = field(w0, 11, 0);
        let tile = self.tiles[ti];
        let img = self.tex_img;
        let bpt_num = match img.size {
            0 => 1,
            1 => 2,
            2 => 4,
            _ => 8,
        }; // half-bytes per texel
        let texels = sh.saturating_sub(sl) + 1;
        let bytes = (texels * bpt_num + 1) / 2;
        let src = img.addr as usize + ((tl * img.width as usize + sl) * bpt_num) / 2;
        let words = (bytes + 7) / 8;
        let mut acc: u32 = 0;
        for wi in 0..words {
            let odd = (acc >> 11) & 1 == 1;
            acc = acc.wrapping_add(dxt);
            for k in 0..8usize {
                let sb = src + wi * 8 + k;
                let v = *ram.get(sb).unwrap_or(&0);
                if img.size == 3 {
                    // two RGBA32 texels per word: split halves
                    let texel = k / 4;
                    let comp = k % 4;
                    let base = (tile.tmem as usize + wi * 4 + texel * 2) ^ if odd { 4 } else { 0 };
                    let dst = if comp < 2 { (base + comp) & 0x7FF } else { ((base + comp - 2) & 0x7FF) | 0x800 };
                    self.tmem[dst] = v;
                } else {
                    let dst = ((tile.tmem as usize + wi * 8 + k) ^ if odd { 4 } else { 0 }) & 0xFFF;
                    self.tmem[dst] = v;
                }
            }
        }
        let t = &mut self.tiles[ti];
        t.sl = (sl as i32) << 2;
        t.tl = (tl as i32) << 2;
        t.sh = (sh as i32) << 2;
    }

    fn load_tlut(&mut self, w0: u64, ram: &[u8]) {
        let ti = field(w0, 26, 24) as usize;
        let sl = field(w0, 55, 44) as usize >> 2;
        let sh = field(w0, 23, 12) as usize >> 2;
        let tile = self.tiles[ti];
        let img = self.tex_img;
        for i in 0..=sh.saturating_sub(sl) {
            let src = img.addr as usize + (sl + i) * 2;
            let hi = *ram.get(src).unwrap_or(&0);
            let lo = *ram.get(src + 1).unwrap_or(&0);
            let dst = (tile.tmem as usize + i * 8) & 0xFFF;
            for q in 0..4 {
                self.tmem[(dst + q * 2) & 0xFFF] = hi;
                self.tmem[(dst + q * 2 + 1) & 0xFFF] = lo;
            }
        }
    }

    // ── texture sampling ─────────────────────────────────────────────────────
    fn wrap(&self, v: i32, tile: &Tile, is_s: bool) -> i32 {
        let (clamp, mirror, mask, lo, hi) = if is_s {
            (tile.cs, tile.ms, tile.mask_s, tile.sl, tile.sh)
        } else {
            (tile.ct, tile.mt, tile.mask_t, tile.tl, tile.th)
        };
        let mut v = v;
        if clamp || mask == 0 {
            let max = (hi - lo) >> 2;
            if v < 0 {
                v = 0;
            } else if v > max && max >= 0 {
                v = max;
            }
        }
        if mask > 0 {
            if mirror && (v >> mask) & 1 != 0 {
                v = !v;
            }
            v &= (1 << mask) - 1;
        }
        v
    }

    fn texel(&self, tile: &Tile, s: i32, t: i32) -> [i32; 4] {
        if STATS_ON.load(std::sync::atomic::Ordering::Relaxed) {
            let k = (self.modes.tlut_en as usize) * 32 + (tile.size as usize & 3) * 8 + (tile.fmt as usize & 7);
            TEX_STATS[k].fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        }
        let row = tile.tmem as usize + t as usize * tile.line as usize;
        let xr = if t & 1 == 1 { 4 } else { 0 };
        let tm = &self.tmem;
        let tlut = |idx: usize| -> [i32; 4] {
            let a = (0x800 + idx * 8) & 0xFFF;
            let v = ((tm[a] as u16) << 8) | tm[a + 1] as u16;
            if self.modes.tlut_ia {
                let i = (v >> 8) as i32;
                [i, i, i, (v & 0xFF) as i32]
            } else {
                rgba5551(v)
            }
        };
        match (tile.size, tile.fmt) {
            (3, _) => {
                let a = ((row + s as usize * 2) ^ xr) & 0x7FF;
                [tm[a] as i32, tm[a + 1] as i32, tm[a | 0x800] as i32, tm[(a + 1) | 0x800] as i32]
            },
            (2, f) => {
                let a = ((row + s as usize * 2) ^ xr) & 0xFFF;
                let v = ((tm[a] as u16) << 8) | tm[(a + 1) & 0xFFF] as u16;
                // TLUT on: every 16-bit texel indexes the palette by its high byte
                if self.modes.tlut_en {
                    tlut((v >> 8) as usize)
                } else if f == FMT_IA {
                    let i = (v >> 8) as i32;
                    [i, i, i, (v & 0xFF) as i32]
                } else {
                    rgba5551(v)
                }
            },
            (1, f) => {
                let b = tm[((row + s as usize) ^ xr) & 0xFFF];
                // TLUT on: any 8-bit format (CI, I, IA, RGBA) is a palette index
                if self.modes.tlut_en {
                    return tlut(b as usize);
                }
                match f {
                    FMT_IA => {
                        let i = ((b >> 4) * 17) as i32;
                        [i, i, i, ((b & 15) * 17) as i32]
                    },
                    _ => {
                        let i = b as i32;
                        [i, i, i, i]
                    },
                }
            },
            (_, f) => {
                let b = tm[((row + s as usize / 2) ^ xr) & 0xFFF];
                let n = if s & 1 == 0 { b >> 4 } else { b & 15 };
                if self.modes.tlut_en {
                    return tlut(((tile.palette as usize) << 4) | n as usize);
                }
                match f {
                    FMT_IA => {
                        let i = ((n >> 1) as i32 * 255) / 7;
                        [i, i, i, if n & 1 != 0 { 255 } else { 0 }]
                    },
                    FMT_I | _ => {
                        let i = (n * 17) as i32;
                        [i, i, i, i]
                    },
                }
            },
        }
    }

    /// Sample tile `ti` at texel coordinates in s10.5.
    fn sample(&self, ti: usize, s105: i32, t105: i32, bilerp: bool) -> [i32; 4] {
        let tile = self.tiles[ti & 7];
        let shift = |v: i32, sh: u32| {
            if sh == 0 {
                v
            } else if sh <= 10 {
                v >> sh
            } else {
                v << (16 - sh)
            }
        };
        let s = shift(s105, tile.shift_s) - (tile.sl << 3);
        let t = shift(t105, tile.shift_t) - (tile.tl << 3);
        if !bilerp {
            let si = self.wrap(s >> 5, &tile, true);
            let tj = self.wrap(t >> 5, &tile, false);
            return self.texel(&tile, si, tj);
        }
        let s = s - 16;
        let t = t - 16;
        let (s0, t0) = (s >> 5, t >> 5);
        let (fs, ft) = (s & 31, t & 31);
        let a = self.texel(&tile, self.wrap(s0, &tile, true), self.wrap(t0, &tile, false));
        let b = self.texel(&tile, self.wrap(s0 + 1, &tile, true), self.wrap(t0, &tile, false));
        let c = self.texel(&tile, self.wrap(s0, &tile, true), self.wrap(t0 + 1, &tile, false));
        let d = self.texel(&tile, self.wrap(s0 + 1, &tile, true), self.wrap(t0 + 1, &tile, false));
        let mut out = [0; 4];
        for i in 0..4 {
            let top = a[i] * (32 - fs) + b[i] * fs;
            let bot = c[i] * (32 - fs) + d[i] * fs;
            out[i] = (top * (32 - ft) + bot * ft) >> 10;
        }
        out
    }

    // ── colour combiner ──────────────────────────────────────────────────────
    fn combine(&self, cyc: usize, comb: [i32; 4], t0: [i32; 4], t1: [i32; 4], shade: [i32; 4]) -> [i32; 4] {
        let c = &self.comb;
        let pick = |sel: u8, k: usize, which: u8| -> i32 {
            // which: 0 = A, 1 = B, 2 = C, 3 = D (RGB channel k)
            match sel {
                0 => comb[k],
                1 => t0[k],
                2 => t1[k],
                3 => self.prim[k],
                4 => shade[k],
                5 => self.env[k],
                6 => match which {
                    0 | 3 => 255,
                    1 => 0, // key center
                    _ => 0, // key scale
                },
                7 => match which {
                    2 => comb[3],
                    _ => 0,
                },
                8 if which == 2 => t0[3],
                9 if which == 2 => t1[3],
                10 if which == 2 => self.prim[3],
                11 if which == 2 => shade[3],
                12 if which == 2 => self.env[3],
                13 if which == 2 => self.lod_frac,
                14 if which == 2 => self.prim_lod,
                _ => 0,
            }
        };
        let apick = |sel: u8, which: u8| -> i32 {
            match sel {
                0 => {
                    if which == 2 {
                        self.lod_frac
                    } else {
                        comb[3]
                    }
                },
                1 => t0[3],
                2 => t1[3],
                3 => self.prim[3],
                4 => shade[3],
                5 => self.env[3],
                6 => {
                    if which == 2 {
                        self.prim_lod
                    } else {
                        255
                    }
                },
                _ => 0,
            }
        };
        let mut out = [0i32; 4];
        for k in 0..3 {
            let a = pick(c.rgb_a[cyc], k, 0);
            let b = pick(c.rgb_b[cyc], k, 1);
            let cc = pick(c.rgb_c[cyc], k, 2);
            let d = pick(c.rgb_d[cyc], k, 3);
            let cc = if cc >= 255 { 256 } else { cc };
            out[k] = (((a - b) * cc + 0x80) >> 8) + d;
        }
        let a = apick(c.a_a[cyc], 0);
        let b = apick(c.a_b[cyc], 1);
        let cc = apick(c.a_c[cyc], 2);
        let d = apick(c.a_d[cyc], 3);
        let cc = if cc >= 255 { 256 } else { cc };
        out[3] = (((a - b) * cc + 0x80) >> 8) + d;
        for v in out.iter_mut() {
            *v = (*v).clamp(0, 255);
        }
        out
    }

    // ── framebuffer + blender ────────────────────────────────────────────────
    /// Memory colour (full precision when the shadow is valid) + coverage.
    fn read_pixel(&self, x: i32, y: i32, ram: &[u8]) -> ([i32; 4], u32) {
        let img = &self.color_img;
        let i = (y as usize) * img.width as usize + x as usize;
        if img.size == 3 {
            let a = img.addr as usize + i * 4;
            if a + 3 >= ram.len() {
                return ([0; 4], 8);
            }
            ([ram[a] as i32, ram[a + 1] as i32, ram[a + 2] as i32, ram[a + 3] as i32], 8)
        } else {
            let a = img.addr as usize + i * 2;
            if a + 1 >= ram.len() {
                return ([0; 4], 8);
            }
            let raw = ((ram[a] as u16) << 8) | ram[a + 1] as u16;
            match self.hi_lookup(a, raw) {
                Some((rgb, cvg)) => (
                    [(rgb >> 16) as i32 & 255, (rgb >> 8) as i32 & 255, rgb as i32 & 255, if raw & 1 != 0 { 255 } else { 0 }],
                    cvg,
                ),
                None => (rgba5551(raw), 8),
            }
        }
    }

    fn write_pixel(&mut self, x: i32, y: i32, c: [i32; 4], cvg: u32, ram: &mut [u8]) {
        let img = self.color_img;
        if STATS_ON.load(std::sync::atomic::Ordering::Relaxed) {
            MISC_STATS[3].fetch_add(1 << (16 * (img.size as u64 & 3)), std::sync::atomic::Ordering::Relaxed);
        }
        let i = (y as usize) * img.width as usize + x as usize;
        if img.size == 3 {
            let a = img.addr as usize + i * 4;
            if a + 3 < ram.len() {
                for k in 0..4 {
                    ram[a + k] = c[k] as u8;
                }
            }
        } else {
            let a = img.addr as usize + i * 2;
            if a + 1 < ram.len() {
                let rgb = ((c[0] as u32 & 255) << 16) | ((c[1] as u32 & 255) << 8) | (c[2] as u32 & 255);
                let p = q5551(rgb) | 1;
                ram[a] = (p >> 8) as u8;
                ram[a + 1] = p as u8;
                self.hi_store(a, rgb, cvg);
            }
        }
    }

    fn blend_cycle(&self, cyc: usize, pix: [i32; 4], mem: [i32; 4], shade_a: i32) -> [i32; 4] {
        let m = &self.modes;
        let psel = |sel: u8| match sel {
            0 => pix,
            1 => mem,
            2 => self.blend,
            _ => self.fog,
        };
        let p = psel(m.blend_m1a[cyc]);
        let mm = psel(m.blend_m2a[cyc]);
        let a = match m.blend_m1b[cyc] {
            0 => pix[3],
            1 => self.fog[3],
            2 => shade_a,
            _ => 0,
        };
        let a = if self.no_fog && (m.blend_m1a[cyc] == 3) { 0 } else { a };
        let b = match m.blend_m2b[cyc] {
            0 => 255 - a,
            1 => mem[3],
            2 => 255,
            _ => 0,
        };
        // The hardware blender is P*A + M*(B+1), NOT normalised by A+B when it
        // runs (2-cycle first pass, or FORCE_BLEND): with B = 1.0 that is a
        // true additive blend (glows, light shafts, fire). Hardware truncates
        // the alphas to 5 bits; full 8-bit precision is kept here.
        let mut out = pix;
        for k in 0..3 {
            out[k] = ((p[k] * a + mm[k] * (b + 1)) >> 8).clamp(0, 255);
        }
        out
    }

    /// Full per-pixel pipeline from combiner output to the framebuffer.
    #[allow(clippy::too_many_arguments)]
    fn shade_pixel(&mut self, x: i32, y: i32, comb: [i32; 4], shade_a: i32, z18: u32, cvg: u32, ram: &mut [u8]) {
        let m = self.modes;
        // alpha compare / coverage-from-alpha cut-outs
        if m.alpha_compare && comb[3] < self.blend[3] {
            return;
        }
        if m.alpha_cvg_sel && !m.force_blend && comb[3] < 0x20 {
            return;
        }
        // depth
        let zi = (y as usize) * self.color_img.width as usize + x as usize;
        let za = self.z_addr as usize + zi * 2;
        let z18 = if m.z_prim { self.prim_z } else { z18 };
        let newz = z_compress(z18);
        // AA: an edge pixel within this tolerance of the stored depth is the
        // same surface (a neighbouring triangle), so coverages combine.
        const SAME_Z: i32 = 32;
        let mut same = false;
        if m.z_compare && za + 1 < ram.len() {
            let old = ((((ram[za] as u16) << 8) | ram[za + 1] as u16) >> 2) as u16;
            let pass = match m.z_mode {
                3 => newz <= old.saturating_add(16), // decal: coplanar passes
                _ => newz <= old || (m.aa_en && cvg < 8 && (newz as i32) <= old as i32 + SAME_Z),
            };
            if !pass {
                return;
            }
            same = m.aa_en && (newz as i32 - old as i32).abs() <= SAME_Z;
        }
        let (mem, mcvg) = if m.image_read || m.force_blend || m.cycle == 1 || m.aa_en {
            self.read_pixel(x, y, ram)
        } else {
            ([0, 0, 0, 255], 8)
        };
        if STATS_ON.load(std::sync::atomic::Ordering::Relaxed) {
            let fog0 = m.blend_m1a[0] == 3 || m.blend_m2a[0] == 3;
            let fog1 = m.blend_m1a[1] == 3 || m.blend_m2a[1] == 3;
            if (m.cycle == 1 && fog0) || ((m.cycle == 0 || m.force_blend) && fog1 && m.cycle != 1) {
                FOG_STATS[(shade_a.clamp(0, 255) >> 5) as usize].fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            } else if fog0 || fog1 {
                FOG_STATS[8].fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            }
            let cyc = if m.cycle == 1 { 1 } else { 0 };
            if m.force_blend {
                let k = if m.blend_m2b[cyc] == 2 { 0 } else { 1 };
                MISC_STATS[k].fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            }
            if m.cycle == 1 {
                let c = &self.comb;
                let uses = |v: u8| v == 1 || v == 2;
                if uses(c.rgb_a[1]) || uses(c.rgb_b[1]) || uses(c.rgb_c[1]) || uses(c.rgb_d[1])
                    || uses(c.a_a[1]) || uses(c.a_b[1]) || uses(c.a_c[1]) || uses(c.a_d[1])
                {
                    MISC_STATS[2].fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                }
            }
        }
        let out = if m.cycle == 1 {
            let c0 = self.blend_cycle(0, comb, mem, shade_a);
            if m.force_blend {
                self.blend_cycle(1, [c0[0], c0[1], c0[2], comb[3]], mem, shade_a)
            } else {
                c0
            }
        } else if m.force_blend {
            self.blend_cycle(0, comb, mem, shade_a)
        } else {
            comb
        };
        let (out, stored) = if !m.aa_en {
            (out, 8)
        } else if m.force_blend {
            // translucent: coverage scales opacity; memory coverage is kept
            let mut o = out;
            for k in 0..3 {
                o[k] = (out[k] * cvg as i32 + mem[k] * (8 - cvg as i32)) / 8;
            }
            (o, mcvg)
        } else if same {
            let mc = mcvg.min(8 - cvg) as i32;
            let tot = (cvg as i32 + mc).max(1);
            let mut o = out;
            for k in 0..3 {
                o[k] = (out[k] * cvg as i32 + mem[k] * mc) / tot;
            }
            (o, (cvg + mcvg).min(8))
        } else {
            (out, cvg)
        };
        self.write_pixel(x, y, out, stored, ram);
        if m.z_update && za + 1 < ram.len() {
            let v = newz << 2;
            ram[za] = (v >> 8) as u8;
            ram[za + 1] = v as u8;
        }
        self.pixels += 1;
    }

    /// Mip-map level selection (RDP LOD): the largest texel step to the next
    /// pixel or the next line picks tiles `base+level` / `base+level+1` and
    /// sets the combiner's LOD fraction. Returns (tile for TEXEL0, TEXEL1).
    fn lod(&mut self, a: &Attrs, g: &Grad, s: i32, t: i32, base: usize) -> (usize, usize) {
        let ax = a.madd(&g.dx, 1.0);
        // one line down at the same x: along the edge, minus the edge's slope
        let dy = g.de.madd(&g.dx, -self.tri_dxh);
        let ay = a.madd(&dy, 1.0);
        let (sx, tx) = self.texcoords(&ax);
        let (sy, ty) = self.texcoords(&ay);
        let lod = (sx - s).abs().max((tx - t).abs()).max((sy - s).abs()).max((ty - t).abs()).min(0x7FFF);
        let max_level = self.tri_level;
        let magnify = lod < 32;
        let level = if magnify { 0 } else { (31 - ((lod >> 5) as u32).leading_zeros()) as usize };
        let m = self.modes;
        if level >= max_level && !magnify {
            // beyond the smallest mip: clamp to it
            self.lod_frac = 0xFF;
            let t = base + max_level;
            return (t & 7, t & 7);
        }
        self.lod_frac = if magnify {
            if m.sharpen || m.detail { (lod << 3) & 0xFF } else { 0 }
        } else {
            ((lod << 3) >> level) & 0xFF
        };
        let t = base + level;
        (t & 7, (t + 1) & 7)
    }

    fn texcoords(&self, a: &Attrs) -> (i32, i32) {
        if self.modes.persp && a.w.abs() > 1e-6 {
            // S,T arrive pre-multiplied by W (0x8000 ≈ 1.0 in the int part)
            let s = a.s / a.w * 32768.0;
            let t = a.t / a.w * 32768.0;
            (s as i32, t as i32)
        } else {
            (a.s as i32, a.t as i32)
        }
    }

    // ── triangles ────────────────────────────────────────────────────────────
    fn triangle(&mut self, op: u8, w: &[u64], ram: &mut [u8]) {
        self.triangles += 1;
        let has_shade = op & 4 != 0;
        let has_tex = op & 2 != 0;
        let has_z = op & 1 != 0;
        let w0 = w[0];
        let lft = w0 >> 55 & 1 != 0;
        let tile = field(w0, 50, 48) as usize;
        self.tri_level = field(w0, 53, 51) as usize;
        let yl = sext(field(w0, 45, 32) as u64, 14) as f32 / 4.0;
        let ym = sext(field(w0, 29, 16) as u64, 14) as f32 / 4.0;
        let yh = sext(field(w0, 13, 0) as u64, 14) as f32 / 4.0;
        let edge = |wd: u64| -> (f32, f32) {
            let x = sext(wd >> 32, 28) as f32 / 65536.0;
            let d = sext(wd & 0xFFFF_FFFF, 30) as f32 / 65536.0;
            (x, d)
        };
        let (xl, dxl) = edge(w[1]);
        let (xh, dxh) = edge(w[2]);
        self.tri_dxh = dxh;
        let (xm, dxm) = edge(w[3]);

        // attribute gradients
        let mut g = Grad::default();
        let lane = |words: &[u64], word_i: usize, word_f: usize, lane: u32| -> f32 {
            let sh = 48 - lane * 16;
            let i = ((words[word_i] >> sh) & 0xFFFF) as u16 as i16 as i32;
            let f = ((words[word_f] >> sh) & 0xFFFF) as u32;
            ((i << 16) as i64 | f as i64) as f32 / 65536.0
        };
        let mut k = 4;
        if has_shade {
            let sw = &w[k..k + 8];
            for c in 0..4 {
                g.v.shade[c] = lane(sw, 0, 2, c as u32);
                g.dx.shade[c] = lane(sw, 1, 3, c as u32);
                g.de.shade[c] = lane(sw, 4, 6, c as u32);
            }
            k += 8;
        }
        if has_tex {
            let tw = &w[k..k + 8];
            g.v.s = lane(tw, 0, 2, 0);
            g.v.t = lane(tw, 0, 2, 1);
            g.v.w = lane(tw, 0, 2, 2);
            g.dx.s = lane(tw, 1, 3, 0);
            g.dx.t = lane(tw, 1, 3, 1);
            g.dx.w = lane(tw, 1, 3, 2);
            g.de.s = lane(tw, 4, 6, 0);
            g.de.t = lane(tw, 4, 6, 1);
            g.de.w = lane(tw, 4, 6, 2);
            k += 8;
        }
        if has_z {
            let zw = &w[k..k + 2];
            g.v.z = (zw[0] >> 32) as i32 as f32 / 65536.0;
            g.dx.z = zw[0] as u32 as i32 as f32 / 65536.0;
            g.de.z = (zw[1] >> 32) as i32 as f32 / 65536.0;
        }

        let ystart_line = yh.floor();
        let (sx0, sy0, sx1, sy1) = self.scissor;
        let y_first = (yh.ceil() as i32).max(sy0);
        let y_last = ((yl - 0.001).floor() as i32).min(sy1 - 1);
        if y_last < y_first {
            return;
        }
        let span = |yc: f32| -> (f32, f32, f32) {
            let dy = yc - ystart_line;
            let xmaj = xh + dxh * dy;
            let xmin = if yc < ym { xm + dxm * dy } else { xl + dxl * (yc - ym) };
            let (x0, x1) = if lft { (xmaj, xmin) } else { (xmin, xmaj) };
            (x0, x1, xmaj)
        };
        if self.modes.aa_en {
            // coverage rasterization: every pixel the triangle touches
            let ya = (yh.floor() as i32).max(sy0);
            let yb = ((yl.ceil() as i32) - 1).min(sy1 - 1);
            for y in ya..=yb {
                let mut sub = [(0f32, 0f32); 4];
                let mut nsub = 0;
                let (mut lo, mut hi) = (f32::MAX, f32::MIN);
                let (mut in_lo, mut in_hi) = (f32::MIN, f32::MAX);
                for (k, sk) in sub.iter_mut().enumerate() {
                    let ys = y as f32 + 0.125 + k as f32 * 0.25;
                    let (x0, x1) = if ys >= yh && ys < yl {
                        let (a, b, _) = span(ys);
                        (a.max(sx0 as f32), b.min(sx1 as f32))
                    } else {
                        (0.0, 0.0)
                    };
                    if x1 > x0 {
                        nsub += 1;
                        lo = lo.min(x0);
                        hi = hi.max(x1);
                    }
                    in_lo = in_lo.max(x0);
                    in_hi = in_hi.min(x1);
                    *sk = (x0, x1);
                }
                if nsub == 0 {
                    continue;
                }
                let yc = y as f32 + 0.5;
                let dy = yc - ystart_line;
                let (_, _, xmaj) = span(yc);
                let at_edge = g.v.madd(&g.de, dy);
                let full_lo = if nsub == 4 { in_lo.ceil() as i32 } else { i32::MAX };
                let full_hi = if nsub == 4 { in_hi.floor() as i32 } else { i32::MIN };
                for x in (lo.floor() as i32).max(sx0)..(hi.ceil() as i32).min(sx1) {
                    let cvg = if x >= full_lo && x < full_hi {
                        8
                    } else {
                        let xf = x as f32;
                        let mut c = 0.0;
                        for &(x0, x1) in &sub {
                            c += ((xf + 1.0).min(x1) - xf.max(x0)).max(0.0);
                        }
                        ((c * 2.0 + 0.5) as u32).min(8)
                    };
                    if cvg == 0 {
                        continue;
                    }
                    self.tri_pixel(&at_edge, &g, xmaj, x, y, tile, has_shade, has_tex, has_z, cvg, ram);
                }
            }
            return;
        }
        for y in y_first..=y_last {
            let yc = y as f32 + 0.5;
            if yc < yh || yc >= yl {
                continue;
            }
            let dy = yc - ystart_line;
            let (x0, x1, xmaj) = span(yc);
            let ix0 = ((x0 - 0.5).ceil() as i32).max(sx0);
            let ix1 = ((x1 - 0.5).ceil() as i32).min(sx1);
            if ix1 <= ix0 {
                continue;
            }
            let at_edge = g.v.madd(&g.de, dy);
            for x in ix0..ix1 {
                self.tri_pixel(&at_edge, &g, xmaj, x, y, tile, has_shade, has_tex, has_z, 8, ram);
            }
        }
    }

    #[allow(clippy::too_many_arguments)]
    #[inline]
    fn tri_pixel(
        &mut self,
        at_edge: &Attrs,
        g: &Grad,
        xmaj: f32,
        x: i32,
        y: i32,
        tile: usize,
        shade_on: bool,
        has_tex: bool,
        has_z: bool,
        cvg: u32,
        ram: &mut [u8],
    ) {
        {
            {
                let a = at_edge.madd(&g.dx, x as f32 + 0.5 - xmaj);
                let shade = if shade_on {
                    [
                        (a.shade[0] as i32).clamp(0, 255),
                        (a.shade[1] as i32).clamp(0, 255),
                        (a.shade[2] as i32).clamp(0, 255),
                        (a.shade[3] as i32).clamp(0, 255),
                    ]
                } else {
                    [0; 4]
                };
                let (t0, t1) = if has_tex {
                    let (s, t) = self.texcoords(&a);
                    if STATS_ON.load(std::sync::atomic::Ordering::Relaxed) {
                        let k = if !self.modes.tex_lod_en { 0 } else if self.modes.detail { 1 } else if self.modes.sharpen { 2 } else { 3 };
                        LOD_STATS[k].fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                    }
                    let (tile0, tile1) = if self.modes.tex_lod_en && !self.no_lod { self.lod(&a, g, s, t, tile) } else { (tile, tile + 1) };
                    let bl = self.modes.bilerp;
                    let t0 = self.sample(tile0, s, t, bl);
                    let t1 = if self.modes.cycle == 1 { self.sample(tile1, s, t, bl) } else { t0 };
                    (t0, t1)
                } else {
                    ([0; 4], [0; 4])
                };
                let comb = if self.modes.cycle == 1 {
                    let c0 = self.combine(0, [0; 4], t0, t1, shade);
                    self.combine(1, c0, t0, t1, shade)
                } else {
                    self.combine(1, [0; 4], t0, t1, shade)
                };
                let z18 = if has_z { ((a.z * 8.0) as i64).clamp(0, 0x3FFFF) as u32 } else { 0x3FFFF };
                self.shade_pixel(x, y, comb, shade[3], z18, cvg, ram);
            }
        }
    }

    // ── rectangles ───────────────────────────────────────────────────────────
    fn fill_rect(&mut self, w0: u64, ram: &mut [u8]) {
        self.rects += 1;
        let xl = field(w0, 55, 44) as i32;
        let yl = field(w0, 43, 32) as i32;
        let xh = field(w0, 23, 12) as i32;
        let yh = field(w0, 11, 0) as i32;
        let (sx0, sy0, sx1, sy1) = self.scissor;
        let fillmode = self.modes.cycle == 3;
        let copymode = self.modes.cycle == 2;
        let x0 = (xh >> 2).max(sx0);
        let y0 = (yh >> 2).max(sy0);
        let (x1, y1) = if fillmode || copymode {
            (((xl >> 2) + 1).min(sx1), ((yl >> 2) + 1).min(sy1))
        } else {
            (((xl + 3) >> 2).min(sx1), ((yl + 3) >> 2).min(sy1))
        };
        if fillmode {
            let img = self.color_img;
            for y in y0..y1 {
                for x in x0..x1 {
                    let i = y as usize * img.width as usize + x as usize;
                    if img.size == 3 {
                        let a = img.addr as usize + i * 4;
                        if a + 3 < ram.len() {
                            ram[a..a + 4].copy_from_slice(&self.fill.to_be_bytes());
                        }
                    } else {
                        let a = img.addr as usize + i * 2;
                        let v = if x & 1 == 0 { (self.fill >> 16) as u16 } else { self.fill as u16 };
                        if a + 1 < ram.len() {
                            ram[a] = (v >> 8) as u8;
                            ram[a + 1] = v as u8;
                            let c = rgba5551(v);
                            self.hi_store(a, ((c[0] as u32) << 16) | ((c[1] as u32) << 8) | c[2] as u32, 8);
                        }
                    }
                }
            }
            return;
        }
        for y in y0..y1 {
            for x in x0..x1 {
                let comb = if self.modes.cycle == 1 {
                    let c0 = self.combine(0, [0; 4], [0; 4], [0; 4], [0; 4]);
                    self.combine(1, c0, [0; 4], [0; 4], [0; 4])
                } else {
                    self.combine(1, [0; 4], [0; 4], [0; 4], [0; 4])
                };
                let z = self.prim_z;
                self.shade_pixel(x, y, comb, 0, z, 8, ram);
            }
        }
    }

    fn tex_rect(&mut self, w0: u64, w1: u64, flip: bool, ram: &mut [u8]) {
        self.rects += 1;
        let xl = field(w0, 55, 44) as i32;
        let yl = field(w0, 43, 32) as i32;
        let tile = field(w0, 26, 24) as usize;
        let xh = field(w0, 23, 12) as i32;
        let yh = field(w0, 11, 0) as i32;
        let s0 = (w1 >> 48) as u16 as i16 as i32; // s10.5
        let t0 = (w1 >> 32) as u16 as i16 as i32;
        let mut dsdx = (w1 >> 16) as u16 as i16 as i32; // s5.10
        let dtdy = w1 as u16 as i16 as i32;
        let copymode = self.modes.cycle == 2;
        if copymode {
            dsdx >>= 2; // copy mode steps four texels per clock
        }
        let (sx0, sy0, sx1, sy1) = self.scissor;
        let (x1, y1) = if copymode || self.modes.cycle == 3 {
            (((xl >> 2) + 1).min(sx1), ((yl >> 2) + 1).min(sy1))
        } else {
            (((xl + 3) >> 2).min(sx1), ((yl + 3) >> 2).min(sy1))
        };
        let x0 = (xh >> 2).max(sx0);
        let y0 = (yh >> 2).max(sy0);
        for y in y0..y1 {
            for x in x0..x1 {
                let dx = (x - (xh >> 2)) as i32;
                let dy = (y - (yh >> 2)) as i32;
                let (u, v) = if flip { (dy, dx) } else { (dx, dy) };
                // s10.5 + s5.10 * n → s10.5 via >>5
                let s = s0 + ((dsdx * u) >> 5);
                let t = t0 + ((dtdy * v) >> 5);
                if copymode {
                    let tex = self.sample(tile, s, t, false);
                    if self.modes.alpha_compare && tex[3] == 0 {
                        continue;
                    }
                    self.write_pixel(x, y, tex, 8, ram);
                    self.pixels += 1;
                    continue;
                }
                let bl = self.modes.bilerp;
                let tx0 = self.sample(tile, s, t, bl);
                let tx1 = if self.modes.cycle == 1 { self.sample(tile + 1, s, t, bl) } else { tx0 };
                let comb = if self.modes.cycle == 1 {
                    let c0 = self.combine(0, [0; 4], tx0, tx1, [0; 4]);
                    self.combine(1, c0, tx0, tx1, [0; 4])
                } else {
                    self.combine(1, [0; 4], tx0, tx1, [0; 4])
                };
                let z = self.prim_z;
                self.shade_pixel(x, y, comb, 0, z, 8, ram);
            }
        }
    }
}

fn rgba32(v: u32) -> [i32; 4] {
    [(v >> 24) as i32 & 255, (v >> 16) as i32 & 255, (v >> 8) as i32 & 255, v as i32 & 255]
}
