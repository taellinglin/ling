//! Pen tablets through WinTab (wintab32.dll, shipped by Wacom, Huion, XP-Pen, Gaomon drivers): pressure,
//! tilt (altitude / azimuth), barrel twist and the eraser end, at the pen's own packet rate. Positions come back
//! in screen pixels with sub-pixel precision; the host maps them into its window.
//! Without a WinTab driver `open` fails and painting falls back to the mouse with simulated pressure.

#[derive(Clone, Copy, Debug, Default)]
pub struct Packet {
    pub sx: f32,
    pub sy: f32,
    pub p: f32,
    /// 0 upright .. 1 flat
    pub tilt: f32,
    /// radians, clockwise from up
    pub az: f32,
    pub twist: f32,
    pub eraser: bool,
    pub buttons: u32,
}

#[cfg(all(windows, feature = "std"))]
mod imp {
    use super::Packet;
    use libloading::{Library, Symbol};

    const WTI_DEFSYSCTX: u32 = 4;
    const WTI_DEVICES: u32 = 100;
    const WTI_INTERFACE: u32 = 1;
    const IFC_WINTABID: u32 = 1;
    const DVC_NAME: u32 = 1;
    const DVC_NPRESSURE: u32 = 15;
    const DVC_ORIENTATION: u32 = 17;
    const PK_STATUS: u32 = 0x0002;
    const PK_CURSOR: u32 = 0x0020;
    const PK_BUTTONS: u32 = 0x0040;
    const PK_X: u32 = 0x0080;
    const PK_Y: u32 = 0x0100;
    const PK_NORMAL_PRESSURE: u32 = 0x0400;
    const PK_ORIENTATION: u32 = 0x1000;
    const CXO_SYSTEM: u32 = 0x0001;
    const OVERSAMPLE: i32 = 8;

    #[repr(C)]
    #[derive(Clone, Copy)]
    struct LogContextA {
        lc_name: [u8; 40],
        lc_options: u32,
        lc_status: u32,
        lc_locks: u32,
        lc_msg_base: u32,
        lc_device: u32,
        lc_pkt_rate: u32,
        lc_pkt_data: u32,
        lc_pkt_mode: u32,
        lc_move_mask: u32,
        lc_btn_dn_mask: u32,
        lc_btn_up_mask: u32,
        lc_in_org_x: i32,
        lc_in_org_y: i32,
        lc_in_org_z: i32,
        lc_in_ext_x: i32,
        lc_in_ext_y: i32,
        lc_in_ext_z: i32,
        lc_out_org_x: i32,
        lc_out_org_y: i32,
        lc_out_org_z: i32,
        lc_out_ext_x: i32,
        lc_out_ext_y: i32,
        lc_out_ext_z: i32,
        lc_sens_x: u32,
        lc_sens_y: u32,
        lc_sens_z: u32,
        lc_sys_mode: i32,
        lc_sys_org_x: i32,
        lc_sys_org_y: i32,
        lc_sys_ext_x: i32,
        lc_sys_ext_y: i32,
        lc_sys_sens_x: u32,
        lc_sys_sens_y: u32,
    }
    #[repr(C)]
    #[derive(Clone, Copy, Default)]
    struct Axis {
        min: i32,
        max: i32,
        units: u32,
        resolution: u32,
    }
    /// field order follows the bit order of the packet mask
    #[repr(C)]
    #[derive(Clone, Copy, Default)]
    struct Pkt {
        status: u32,
        cursor: u32,
        buttons: u32,
        x: i32,
        y: i32,
        pressure: u32,
        azimuth: i32,
        altitude: i32,
        twist: i32,
    }

    type WtInfo = unsafe extern "system" fn(u32, u32, *mut u8) -> u32;
    type WtOpen = unsafe extern "system" fn(isize, *mut LogContextA, i32) -> isize;
    type WtClose = unsafe extern "system" fn(isize) -> i32;
    type WtPacketsGet = unsafe extern "system" fn(isize, i32, *mut Pkt) -> i32;
    type WtQueueSizeSet = unsafe extern "system" fn(isize, i32) -> i32;
    type WtOverlap = unsafe extern "system" fn(isize, i32) -> i32;

    pub struct Tablet {
        lib: Option<Library>,
        ctx: isize,
        pmax: f32,
        alt_max: f32,
        org: (i32, i32),
        pub name: String,
    }
    impl Default for Tablet {
        fn default() -> Self {
            Tablet { lib: None, ctx: 0, pmax: 1023.0, alt_max: 900.0, org: (0, 0), name: String::new() }
        }
    }
    impl Tablet {
        pub fn is_open(&self) -> bool {
            self.ctx != 0
        }
        pub fn open(&mut self, hwnd: isize) -> Result<String, String> {
            if self.ctx != 0 {
                return Ok(self.name.clone());
            }
            unsafe {
                let lib = Library::new("wintab32.dll").map_err(|_| "no WinTab driver (wintab32.dll) - install your tablet driver, or keep using the mouse with simulated pressure".to_string())?;
                let info: Symbol<WtInfo> = lib.get(b"WTInfoA\0").map_err(|e| e.to_string())?;
                let open: Symbol<WtOpen> = lib.get(b"WTOpenA\0").map_err(|e| e.to_string())?;
                if info(0, 0, std::ptr::null_mut()) == 0 {
                    return Err("WinTab reports no tablet services".into());
                }
                let mut lc: LogContextA = std::mem::zeroed();
                if info(WTI_DEFSYSCTX, 0, &mut lc as *mut _ as *mut u8) == 0 {
                    return Err("WinTab has no default context".into());
                }
                let mut ax = Axis::default();
                if info(WTI_DEVICES, DVC_NPRESSURE, &mut ax as *mut _ as *mut u8) > 0 && ax.max > 0 {
                    self.pmax = ax.max as f32;
                }
                let mut ori = [Axis::default(); 3];
                if info(WTI_DEVICES, DVC_ORIENTATION, ori.as_mut_ptr() as *mut u8) > 0 && ori[1].max > 0 {
                    self.alt_max = ori[1].max as f32;
                }
                let mut nm = [0u8; 128];
                info(WTI_DEVICES, DVC_NAME, nm.as_mut_ptr());
                let mut dev = String::from_utf8_lossy(&nm).trim_end_matches('\0').trim().to_string();
                if dev.is_empty() {
                    info(WTI_INTERFACE, IFC_WINTABID, nm.as_mut_ptr());
                    dev = String::from_utf8_lossy(&nm).trim_end_matches('\0').trim().to_string();
                }
                let name = b"LingAnime\0";
                lc.lc_name = [0; 40];
                lc.lc_name[..name.len()].copy_from_slice(name);
                lc.lc_options |= CXO_SYSTEM;
                let mask = PK_STATUS | PK_CURSOR | PK_BUTTONS | PK_X | PK_Y | PK_NORMAL_PRESSURE | PK_ORIENTATION;
                lc.lc_pkt_data = mask;
                lc.lc_pkt_mode = 0;
                lc.lc_move_mask = mask;
                lc.lc_btn_up_mask = lc.lc_btn_dn_mask;
                // output in (oversampled) screen pixels over the area the driver maps the tablet to, y down
                self.org = (lc.lc_sys_org_x, lc.lc_sys_org_y);
                lc.lc_out_org_x = 0;
                lc.lc_out_org_y = 0;
                lc.lc_out_ext_x = lc.lc_sys_ext_x * OVERSAMPLE;
                lc.lc_out_ext_y = -(lc.lc_sys_ext_y * OVERSAMPLE);
                let ctx = open(hwnd, &mut lc, 1);
                if ctx == 0 {
                    return Err("WTOpen failed".into());
                }
                if let Ok(q) = lib.get::<WtQueueSizeSet>(b"WTQueueSizeSet\0") {
                    let mut n = 256;
                    while n >= 16 && q(ctx, n) == 0 {
                        n /= 2;
                    }
                }
                if let Ok(ov) = lib.get::<WtOverlap>(b"WTOverlap\0") {
                    ov(ctx, 1);
                }
                self.ctx = ctx;
                self.name = if dev.is_empty() { "WinTab tablet".into() } else { dev };
                self.lib = Some(lib);
            }
            Ok(self.name.clone())
        }
        pub fn close(&mut self) {
            if let (Some(lib), true) = (&self.lib, self.ctx != 0) {
                unsafe {
                    if let Ok(c) = lib.get::<WtClose>(b"WTClose\0") {
                        c(self.ctx);
                    }
                }
            }
            self.ctx = 0;
            self.lib = None;
        }
        pub fn poll(&mut self) -> Vec<Packet> {
            let mut out = Vec::new();
            let Some(lib) = &self.lib else { return out };
            if self.ctx == 0 {
                return out;
            }
            unsafe {
                let Ok(get) = lib.get::<WtPacketsGet>(b"WTPacketsGet\0") else { return out };
                let mut buf = [Pkt::default(); 128];
                loop {
                    let n = get(self.ctx, buf.len() as i32, buf.as_mut_ptr());
                    if n <= 0 {
                        break;
                    }
                    for p in &buf[..n as usize] {
                        let alt = (p.altitude.abs() as f32 / self.alt_max).clamp(0.0, 1.0);
                        out.push(Packet {
                            sx: self.org.0 as f32 + p.x as f32 / OVERSAMPLE as f32,
                            sy: self.org.1 as f32 + p.y as f32 / OVERSAMPLE as f32,
                            p: (p.pressure as f32 / self.pmax).clamp(0.0, 1.0),
                            tilt: 1.0 - alt,
                            az: (p.azimuth as f32 / 10.0).to_radians(),
                            twist: (p.twist as f32 / 10.0).to_radians(),
                            eraser: p.cursor % 3 == 2 || p.altitude < 0,
                            buttons: p.buttons,
                        });
                    }
                    if (n as usize) < buf.len() {
                        break;
                    }
                }
            }
            out
        }
    }
    impl Drop for Tablet {
        fn drop(&mut self) {
            self.close();
        }
    }
}

#[cfg(not(all(windows, feature = "std")))]
mod imp {
    use super::Packet;
    #[allow(unused_imports)]
    use crate::prelude::*;
    #[derive(Default)]
    pub struct Tablet {
        pub name: String,
    }
    impl Tablet {
        pub fn is_open(&self) -> bool {
            false
        }
        pub fn open(&mut self, _hwnd: isize) -> Result<String, String> {
            Err("pen tablets are read through WinTab on Windows; elsewhere the mouse with simulated pressure is used".into())
        }
        pub fn close(&mut self) {}
        pub fn poll(&mut self) -> Vec<Packet> {
            Vec::new()
        }
    }
}

pub use imp::Tablet;
