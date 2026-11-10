//! Which GPU drives the monitors, and how full each card really is, from NVIDIA's NVML (nvml.dll /
//! libnvidia-ml.so.1, part of the driver; loaded at run time, optional). CUDA alone cannot tell:
//! on Windows cuMemGetInfo only counts this process's own allocations.

use std::ffi::{c_char, c_int, c_uint, c_void};

#[repr(C)]
#[derive(Default)]
struct NvmlMemory {
    total: u64,
    free: u64,
    used: u64,
}

pub struct GpuState {
    /// a monitor is connected to it and the desktop is drawn on it
    pub display: bool,
    /// bytes in use by every process
    pub used: u64,
    pub total: u64,
}

/// NVML's view of the GPU at this PCI address ("00000000:01:00.0"), if NVML is available.
pub fn query(pci: &str) -> Option<GpuState> {
    unsafe {
        let names: &[&str] = if cfg!(windows) { &["nvml.dll", "C:\\Windows\\System32\\nvml.dll"] } else { &["libnvidia-ml.so.1", "libnvidia-ml.so"] };
        let lib = names.iter().find_map(|n| libloading::Library::new(n).ok())?;
        let init: libloading::Symbol<unsafe extern "C" fn() -> c_int> = lib.get(b"nvmlInit_v2\0").ok()?;
        if init() != 0 {
            return None;
        }
        let by_pci: libloading::Symbol<unsafe extern "C" fn(*const c_char, *mut *mut c_void) -> c_int> = lib.get(b"nvmlDeviceGetHandleByPciBusId_v2\0").ok()?;
        let disp: libloading::Symbol<unsafe extern "C" fn(*mut c_void, *mut c_uint) -> c_int> = lib.get(b"nvmlDeviceGetDisplayActive\0").ok()?;
        let mem: libloading::Symbol<unsafe extern "C" fn(*mut c_void, *mut NvmlMemory) -> c_int> = lib.get(b"nvmlDeviceGetMemoryInfo\0").ok()?;
        let c = std::ffi::CString::new(pci).ok()?;
        let mut dev: *mut c_void = std::ptr::null_mut();
        if by_pci(c.as_ptr(), &mut dev) != 0 {
            return None;
        }
        let mut active: c_uint = 0;
        let display = disp(dev, &mut active) == 0 && active != 0;
        let mut m = NvmlMemory::default();
        let _ = mem(dev, &mut m);
        // nvmlShutdown is skipped on purpose: NVML is reference counted and cheap to keep
        Some(GpuState { display, used: m.used, total: m.total })
    }
}
