//! USB gamepads for the xHCI driver: a HID report-descriptor parser that
//! finds the sticks, triggers, hat and buttons of a generic HID gamepad or
//! joystick (DualShock 4 / DualSense, most "PC" pads), plus the fixed report
//! of a wired Xbox 360 controller (vendor class, not HID). Reports are decoded
//! into one [`PadState`] that apps read with the `pad_*` builtins.
//!
//! Pure data code: no bus access, so it is unit-testable on the host.

/// One field of an input report: `bits` wide at bit offset `off`, logical
/// range `min..=max`, what it is (`usage`: Generic Desktop usage id).
#[derive(Clone, Copy, Default)]
pub struct Field {
    pub off: u16,
    pub bits: u8,
    pub min: i32,
    pub max: i32,
    pub usage: u8,
}

pub const MAX_AXES: usize = 8;
pub const MAX_BUTTONS: usize = 32;

/// Where the interesting fields sit in a pad's input report.
#[derive(Clone, Copy)]
pub struct Layout {
    /// Report ID the fields belong to (0 = the device uses none).
    pub report_id: u8,
    pub axes: [Field; MAX_AXES],
    pub n_axes: u8,
    pub hat: Option<Field>,
    /// First button's bit offset and how many 1-bit buttons follow it.
    pub buttons_off: u16,
    pub n_buttons: u8,
    pub xbox360: bool,
}

impl Layout {
    pub const EMPTY: Layout = Layout { report_id: 0, axes: [Field { off: 0, bits: 0, min: 0, max: 0, usage: 0 }; MAX_AXES], n_axes: 0, hat: None, buttons_off: 0, n_buttons: 0, xbox360: false };

    /// The wired Xbox 360 controller's 20-byte report (not HID).
    pub const XBOX360: Layout = Layout { xbox360: true, ..Layout::EMPTY };

    pub fn usable(&self) -> bool {
        self.xbox360 || self.n_axes > 0 || self.n_buttons > 0
    }
}

/// Parse a HID report descriptor. `Some` only for a device whose application
/// collection is a Game Pad (0x05), Joystick (0x04) or Multi-axis Controller
/// (0x08) -- a keyboard's consumer-control interface is not one.
pub fn parse_descriptor(d: &[u8]) -> Option<Layout> {
    let mut lay = Layout::EMPTY;
    // Global items.
    let (mut usage_page, mut lmin, mut lmax, mut rsize, mut rcount, mut rid) = (0u32, 0i32, 0i32, 0u32, 0u32, 0u32);
    // Local items: the usages named before the next main item.
    let mut usages: [u32; 32] = [0; 32];
    let mut n_usages = 0usize;
    let (mut umin, mut umax): (Option<u32>, Option<u32>) = (None, None);
    let mut is_pad = false;
    let mut depth = 0u32;
    // Input bit offset per report id (the one we keep); reset when the id changes.
    let mut bit = 0u32;
    let mut cur_rid = 0u32;
    let mut kept_rid: Option<u32> = None;
    let mut i = 0usize;
    while i < d.len() {
        let b = d[i];
        i += 1;
        if b == 0xFE {
            // Long item: skip it.
            if i < d.len() {
                i += 2 + d[i] as usize;
            }
            continue;
        }
        let size = match b & 3 {
            3 => 4,
            n => n as usize,
        };
        if i + size > d.len() {
            break;
        }
        let mut val: u32 = 0;
        for k in 0..size {
            val |= (d[i + k] as u32) << (8 * k);
        }
        let sval: i32 = match size {
            1 => d[i] as i8 as i32,
            2 => i16::from_le_bytes([d[i], d[i + 1]]) as i32,
            4 => val as i32,
            _ => 0,
        };
        i += size;
        let (typ, tag) = ((b >> 2) & 3, b >> 4);
        match typ {
            1 => match tag {
                0 => usage_page = val,
                1 => lmin = sval,
                2 => lmax = if size == 4 { sval } else if lmin < 0 { sval } else { val as i32 },
                7 => rsize = val,
                8 => {
                    rid = val;
                    if cur_rid != rid {
                        cur_rid = rid;
                        if kept_rid.is_none() || kept_rid != Some(rid) {
                            bit = 0;
                        }
                    }
                },
                9 => rcount = val,
                _ => {},
            },
            2 => match tag {
                0 => {
                    if n_usages < usages.len() {
                        // A 32-bit usage carries its own page in the top half.
                        usages[n_usages] = if size == 4 { val } else { (usage_page << 16) | val };
                        n_usages += 1;
                    }
                },
                1 => umin = Some(if size == 4 { val } else { (usage_page << 16) | val }),
                2 => umax = Some(if size == 4 { val } else { (usage_page << 16) | val }),
                _ => {},
            },
            0 => {
                match tag {
                    10 => {
                        // Collection: the application one says what this is.
                        if depth == 0 && val == 1 && n_usages > 0 {
                            let u = usages[0];
                            if u >> 16 == 1 && matches!(u & 0xFFFF, 4 | 5 | 8) {
                                is_pad = true;
                            }
                        }
                        depth += 1;
                    },
                    12 => depth = depth.saturating_sub(1),
                    8 => {
                        // Input item.
                        let constant = val & 1 != 0;
                        let total = rsize * rcount;
                        // Fields of a report other than the one we keep are skipped
                        // (their offsets are counted in their own report).
                        let keep = is_pad && (kept_rid.is_none() || kept_rid == Some(cur_rid));
                        if !constant && keep {
                            for n in 0..rcount {
                                let usage = if n_usages > 0 {
                                    usages[(n as usize).min(n_usages - 1)]
                                } else if let (Some(a), Some(z)) = (umin, umax) {
                                    (a + n).min(z)
                                } else {
                                    0
                                };
                                let page = usage >> 16;
                                let id = usage & 0xFFFF;
                                let off = (bit + n * rsize) as u16;
                                if page == 1 && (0x30..=0x37).contains(&id) && (lay.n_axes as usize) < MAX_AXES && rsize <= 32 {
                                    lay.axes[lay.n_axes as usize] = Field { off, bits: rsize as u8, min: lmin, max: lmax, usage: id as u8 };
                                    lay.n_axes += 1;
                                    kept_rid = Some(cur_rid);
                                } else if page == 1 && id == 0x39 && lay.hat.is_none() {
                                    lay.hat = Some(Field { off, bits: rsize as u8, min: lmin, max: lmax, usage: 0x39 });
                                    kept_rid = Some(cur_rid);
                                } else if page == 9 && rsize == 1 && lay.n_buttons == 0 {
                                    lay.buttons_off = off;
                                    lay.n_buttons = rcount.min(MAX_BUTTONS as u32) as u8;
                                    kept_rid = Some(cur_rid);
                                    break;
                                }
                            }
                        }
                        if kept_rid.is_none() || kept_rid == Some(cur_rid) {
                            bit += total;
                        }
                    },
                    _ => {},
                }
                // Local items reset after every main item.
                n_usages = 0;
                umin = None;
                umax = None;
            },
            _ => {},
        }
    }
    if !is_pad || !lay.usable() {
        return None;
    }
    lay.report_id = kept_rid.unwrap_or(0) as u8;
    Some(lay)
}

/// What apps see.
#[derive(Clone, Copy, Default)]
pub struct PadState {
    pub connected: bool,
    /// LX, LY, RX, RY, LT, RT (then any further axes), -1..1 (triggers 0..1).
    pub axes: [f32; MAX_AXES],
    pub buttons: u32,
    /// 0..7 clockwise from up, -1 centred.
    pub hat: i8,
    pub xbox: bool,
}

fn bits_at(r: &[u8], off: u16, bits: u8) -> u32 {
    let mut v: u32 = 0;
    for k in 0..bits as u16 {
        let b = off + k;
        let byte = (b / 8) as usize;
        if byte < r.len() && r[byte] & (1 << (b % 8)) != 0 {
            v |= 1 << k;
        }
    }
    v
}

fn norm(f: &Field, raw: u32) -> f32 {
    if f.max <= f.min {
        return 0.0;
    }
    let v: i32 = if f.min < 0 {
        // Signed field: sign-extend from its width.
        let shift = 32 - f.bits as u32;
        ((raw << shift) as i32) >> shift
    } else {
        raw as i32
    };
    let span = (f.max - f.min) as f32;
    let centred = (v - f.min) as f32 / span * 2.0 - 1.0;
    centred.clamp(-1.0, 1.0)
}

/// Decode one report into `st` (`r` starts with the report ID byte when the
/// device uses IDs). Returns false when the report is for another ID.
pub fn decode(lay: &Layout, r: &[u8], st: &mut PadState) -> bool {
    st.connected = true;
    if lay.xbox360 {
        if r.len() < 14 || r[0] != 0x00 {
            return false;
        }
        st.xbox = true;
        let s = |i: usize| i16::from_le_bytes([r[i], r[i + 1]]) as f32 / 32767.0;
        st.axes[0] = s(6);
        st.axes[1] = -s(8);
        st.axes[2] = s(10);
        st.axes[3] = -s(12);
        st.axes[4] = r[4] as f32 / 255.0;
        st.axes[5] = r[5] as f32 / 255.0;
        // Buttons in the usual order: A B X Y LB RB Back Start LS RS Guide, then the d-pad as 12..15.
        let b2 = r[2];
        let b3 = r[3];
        let mut m = 0u32;
        let set = |m: &mut u32, cond: bool, bit: u32| {
            if cond {
                *m |= 1 << bit;
            }
        };
        set(&mut m, b3 & 0x10 != 0, 0);
        set(&mut m, b3 & 0x20 != 0, 1);
        set(&mut m, b3 & 0x40 != 0, 2);
        set(&mut m, b3 & 0x80 != 0, 3);
        set(&mut m, b3 & 0x01 != 0, 4);
        set(&mut m, b3 & 0x02 != 0, 5);
        set(&mut m, b2 & 0x20 != 0, 6);
        set(&mut m, b2 & 0x10 != 0, 7);
        set(&mut m, b2 & 0x40 != 0, 8);
        set(&mut m, b2 & 0x80 != 0, 9);
        set(&mut m, b3 & 0x04 != 0, 10);
        set(&mut m, b2 & 0x01 != 0, 12);
        set(&mut m, b2 & 0x02 != 0, 13);
        set(&mut m, b2 & 0x04 != 0, 14);
        set(&mut m, b2 & 0x08 != 0, 15);
        st.buttons = m;
        st.hat = match b2 & 0x0F {
            0x1 => 0,
            0x9 => 1,
            0x8 => 2,
            0xA => 3,
            0x2 => 4,
            0x6 => 5,
            0x4 => 6,
            0x5 => 7,
            _ => -1,
        };
        return true;
    }
    let body = if lay.report_id != 0 {
        if r.is_empty() || r[0] != lay.report_id {
            return false;
        }
        &r[1..]
    } else {
        r
    };
    // Axes in a stable order: X Y Z Rz Rx Ry -> LX LY RX RY LT RT on a DualShock,
    // anything else after.
    let mut slot_of = |usage: u8| -> usize {
        match usage {
            0x30 => 0,
            0x31 => 1,
            0x32 => 2,
            0x35 => 3,
            0x33 => 4,
            0x34 => 5,
            0x36 => 6,
            _ => 7,
        }
    };
    for a in lay.axes.iter().take(lay.n_axes as usize) {
        let v = norm(a, bits_at(body, a.off, a.bits));
        let slot = slot_of(a.usage);
        // Y axes point down in HID; apps expect up = positive.
        st.axes[slot] = if slot == 1 || slot == 3 { -v } else { v };
        if slot == 4 || slot == 5 {
            st.axes[slot] = (v + 1.0) / 2.0; // triggers 0..1
        }
    }
    if let Some(h) = lay.hat {
        let raw = bits_at(body, h.off, h.bits) as i32;
        let v = raw - h.min;
        st.hat = if v >= 0 && v <= h.max - h.min && h.max - h.min >= 7 { v as i8 } else { -1 };
    }
    let mut m = 0u32;
    for k in 0..lay.n_buttons as u16 {
        if bits_at(body, lay.buttons_off + k, 1) != 0 {
            m |= 1 << k;
        }
    }
    // The hat doubles as d-pad buttons 12..15 when the pad has 12 or fewer buttons.
    if lay.n_buttons <= 12 && st.hat >= 0 {
        let h = st.hat;
        if matches!(h, 7 | 0 | 1) {
            m |= 1 << 12;
        }
        if matches!(h, 3 | 4 | 5) {
            m |= 1 << 13;
        }
        if matches!(h, 5 | 6 | 7) {
            m |= 1 << 14;
        }
        if matches!(h, 1 | 2 | 3) {
            m |= 1 << 15;
        }
    }
    st.buttons = m;
    true
}
