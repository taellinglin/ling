//! CPU driver: identification, topology, hardware P-states (HWP), thermal and
//! effective-frequency readouts, and per-core hardening -- tuned for Intel
//! Core (Rocket Lake i9-11900K and its relatives), harmless elsewhere.
//!
//! Every model-specific register is gated behind the CPUID bit that promises
//! it (and the Intel-only ones behind the vendor check too): an MSR the CPU
//! lacks raises #GP, which this kernel cannot recover from. Under QEMU's TCG
//! or on AMD most of this simply reports "not available".
//!
//! Performance: with HWP the core picks its own clock between the limits the
//! OS sets, reaching turbo (5.3 GHz on an i9-11900K) when busy and dropping to
//! ~800 MHz when idle. Without an OS request the firmware's default applies,
//! which often caps the core well below turbo. `cpu perf | balanced | save`
//! sets the energy-performance preference on every core.
use super::cpu;
use alloc::format;
use alloc::string::String;
use core::sync::atomic::{AtomicU64, Ordering};

const IA32_BIOS_SIGN_ID: u32 = 0x8B;
const IA32_MPERF: u32 = 0xE7;
const IA32_APERF: u32 = 0xE8;
const IA32_THERM_STATUS: u32 = 0x19C;
const MSR_TEMPERATURE_TARGET: u32 = 0x1A2;
const IA32_PACKAGE_THERM_STATUS: u32 = 0x1B1;
const IA32_PM_ENABLE: u32 = 0x770;
const IA32_HWP_CAPABILITIES: u32 = 0x771;
const IA32_HWP_REQUEST: u32 = 0x774;

#[derive(Clone, Copy, Default)]
pub struct Features {
    pub intel: bool,
    pub family: u32,
    pub model: u32,
    pub stepping: u32,
    pub sse42: bool,
    pub avx: bool,
    pub avx2: bool,
    pub avx512f: bool,
    pub x2apic: bool,
    pub smep: bool,
    pub monitor: bool,
    pub dts: bool,
    pub ptm: bool,
    pub turbo: bool,
    pub hwp: bool,
    pub hwp_epp: bool,
    pub aperf: bool,
    pub hypervisor: bool,
    /// CPUID 0x16: base / max MHz (0 when the leaf is absent).
    pub base_mhz: u32,
    pub max_mhz: u32,
}

static mut FEAT: Option<Features> = None;
/// The HWP request every core should run with (0 = HWP not in use).
static HWP_WANT: AtomicU64 = AtomicU64::new(0);
/// Per-core: the request it last wrote, and its last temperature (deg C, 0 = unknown).
static HWP_HAVE: [AtomicU64; super::smp::MAX_CPUS] = [const { AtomicU64::new(0) }; super::smp::MAX_CPUS];
static TEMP_C: [AtomicU64; super::smp::MAX_CPUS] = [const { AtomicU64::new(0) }; super::smp::MAX_CPUS];
static mut TJMAX: u32 = 0;
static mut EPP: u8 = 0x40;

pub fn features() -> Features {
    let f = unsafe { &mut *(&raw mut FEAT) };
    if let Some(x) = *f {
        return x;
    }
    let x = unsafe { detect() };
    *f = Some(x);
    x
}

unsafe fn detect() -> Features {
    let mut x = Features::default();
    let (max_leaf, b, c, d) = cpu::cpuid(0, 0);
    x.intel = b == 0x756E_6547 && d == 0x4965_6E69 && c == 0x6C65_746E; // "GenuineIntel"
    let (a1, _, c1, _) = cpu::cpuid(1, 0);
    let fam = (a1 >> 8) & 0xF;
    x.family = if fam == 0xF { fam + ((a1 >> 20) & 0xFF) } else { fam };
    x.model = if fam == 6 || fam == 0xF { ((a1 >> 4) & 0xF) | (((a1 >> 16) & 0xF) << 4) } else { (a1 >> 4) & 0xF };
    x.stepping = a1 & 0xF;
    x.sse42 = c1 & (1 << 20) != 0;
    x.x2apic = c1 & (1 << 21) != 0;
    x.monitor = c1 & (1 << 3) != 0;
    x.hypervisor = c1 & (1 << 31) != 0;
    // AVX needs the OS to have enabled it (OSXSAVE + XCR0); report the CPU's support
    x.avx = c1 & (1 << 28) != 0;
    if max_leaf >= 6 {
        let (a6, _, c6, _) = cpu::cpuid(6, 0);
        x.dts = a6 & 1 != 0;
        x.turbo = a6 & (1 << 1) != 0;
        x.ptm = a6 & (1 << 6) != 0;
        x.hwp = a6 & (1 << 7) != 0;
        x.hwp_epp = a6 & (1 << 10) != 0;
        x.aperf = c6 & 1 != 0;
    }
    if max_leaf >= 7 {
        let (_, b7, _, _) = cpu::cpuid(7, 0);
        x.smep = b7 & (1 << 7) != 0;
        x.avx2 = b7 & (1 << 5) != 0;
        x.avx512f = b7 & (1 << 16) != 0;
    }
    if max_leaf >= 0x16 {
        let (a16, b16, _, _) = cpu::cpuid(0x16, 0);
        x.base_mhz = a16 & 0xFFFF;
        x.max_mhz = b16 & 0xFFFF;
    }
    x
}

/// "11th Gen Intel(R) Core(TM) i9-11900K @ 3.50GHz"
pub fn brand() -> String {
    let mut out = [0u8; 48];
    unsafe {
        if cpu::cpuid(0x8000_0000, 0).0 < 0x8000_0004 {
            return String::from("(no brand string)");
        }
        for (k, leaf) in (0x8000_0002u32..=0x8000_0004).enumerate() {
            let (a, b, c, d) = cpu::cpuid(leaf, 0);
            for (j, r) in [a, b, c, d].iter().enumerate() {
                out[k * 16 + j * 4..k * 16 + j * 4 + 4].copy_from_slice(&r.to_le_bytes());
            }
        }
    }
    let end = out.iter().position(|&c| c == 0).unwrap_or(48);
    String::from(core::str::from_utf8(&out[..end]).unwrap_or("?").trim())
}

/// (logical processors per core, logical processors per package) from CPUID 0x0B.
pub fn topology() -> (u32, u32) {
    unsafe {
        if cpu::cpuid(0, 0).0 < 0x0B {
            return (1, 1);
        }
        let mut smt = 1;
        let mut pkg = 1;
        for level in 0..4 {
            let (_, b, c, _) = cpu::cpuid(0x0B, level);
            let kind = (c >> 8) & 0xFF;
            if kind == 0 {
                break;
            }
            match kind {
                1 => smt = (b & 0xFFFF).max(1),
                2 => pkg = (b & 0xFFFF).max(1),
                _ => {},
            }
        }
        (smt, pkg.max(smt))
    }
}

pub fn microcode() -> Option<u32> {
    let f = features();
    if !f.intel || f.hypervisor {
        return None;
    }
    unsafe {
        cpu::write_msr(IA32_BIOS_SIGN_ID, 0);
        cpu::cpuid(1, 0);
        Some((cpu::read_msr(IA32_BIOS_SIGN_ID) >> 32) as u32)
    }
}

fn thermal_ok(f: &Features) -> bool {
    f.intel && f.family == 6 && f.dts && !f.hypervisor
}

/// This core's temperature in degrees C (None without a digital sensor).
pub fn core_temp() -> Option<u32> {
    let f = features();
    if !thermal_ok(&f) {
        return None;
    }
    unsafe {
        let tj = TJMAX;
        let st = cpu::read_msr(IA32_THERM_STATUS);
        if tj == 0 || st & (1 << 31) == 0 {
            return None;
        }
        Some(tj.saturating_sub(((st >> 16) & 0x7F) as u32))
    }
}

/// The package temperature (hottest sensor) in degrees C.
pub fn package_temp() -> Option<u32> {
    let f = features();
    if !thermal_ok(&f) || !f.ptm {
        return None;
    }
    unsafe {
        let tj = TJMAX;
        if tj == 0 {
            return None;
        }
        Some(tj.saturating_sub(((cpu::read_msr(IA32_PACKAGE_THERM_STATUS) >> 16) & 0x7F) as u32))
    }
}

/// (APERF, MPERF) on this core: their ratio over an interval is the average
/// clock while running relative to the base clock.
pub fn aperf_mperf() -> Option<(u64, u64)> {
    let f = features();
    if !f.aperf || f.hypervisor {
        return None;
    }
    unsafe { Some((cpu::read_msr(IA32_APERF), cpu::read_msr(IA32_MPERF))) }
}

/// Base clock in MHz: CPUID 0x16, else the TSC rate (equal on Intel Core).
pub fn base_mhz() -> u32 {
    let f = features();
    if f.base_mhz > 0 {
        f.base_mhz
    } else {
        super::timer::tsc_mhz() as u32
    }
}

fn hwp_request(epp: u8) -> u64 {
    let caps = unsafe { cpu::read_msr(IA32_HWP_CAPABILITIES) };
    let highest = caps & 0xFF;
    let lowest = (caps >> 24) & 0xFF;
    // min = lowest, max = highest (turbo), desired = 0 (hardware picks), EPP
    lowest | (highest << 8) | ((epp as u64) << 24)
}

/// Per-core setup, on every core (the boot core from `init`, the others
/// from smp.rs): SMEP, and the HWP request.
pub fn per_core(me: usize) {
    let f = features();
    // the same memory types on every core (PAT entry 1 = write-combining)
    super::boot::set_pat_wc();
    unsafe {
        if f.smep {
            // the kernel never runs code from a user page: make that a fault
            let cr4: u64;
            core::arch::asm!("mov {}, cr4", out(reg) cr4, options(nomem, nostack));
            core::arch::asm!("mov cr4, {}", in(reg) cr4 | (1 << 20), options(nomem, nostack));
        }
    }
    enable_avx();
    apply_hwp(me);
}

/// XCR0 state components: x87, SSE, AVX (YMM upper halves), and the three
/// AVX-512 ones (opmask, ZMM_Hi256, Hi16_ZMM).
const XCR0_X87_SSE_AVX: u64 = 0b111;
const XCR0_AVX512: u64 = 0b1110_0000;

/// Turn on AVX (and AVX-512 state where the CPU has it) for this core:
/// CR4.OSXSAVE, then XCR0. Without this every VEX instruction is #UD. Safe
/// with the cooperative scheduler: a task only switches at a yield, and no
/// AVX code is live across one; interrupt handlers are built for SSE, whose
/// legacy encodings leave the upper YMM/ZMM halves alone. Kernel code is still
/// compiled for SSE -- only `#[target_feature(enable = "avx2,fma")]` functions
/// chosen at run time through `avx2()` use the wider units.
fn enable_avx() {
    unsafe {
        let (max_leaf, _, _, _) = cpu::cpuid(0, 0);
        let (_, _, c1, _) = cpu::cpuid(1, 0);
        let xsave = c1 & (1 << 26) != 0;
        let avx = c1 & (1 << 28) != 0;
        if !xsave || !avx || max_leaf < 0xD {
            return;
        }
        let cr4: u64;
        core::arch::asm!("mov {}, cr4", out(reg) cr4, options(nomem, nostack));
        if cr4 & (1 << 18) == 0 {
            core::arch::asm!("mov cr4, {}", in(reg) cr4 | (1 << 18), options(nomem, nostack));
        }
        // what this CPU can hold in XCR0
        let (sup_lo, _, _, sup_hi) = cpu::cpuid(0xD, 0);
        let supported = (sup_hi as u64) << 32 | sup_lo as u64;
        let mut want = XCR0_X87_SSE_AVX;
        if supported & XCR0_AVX512 == XCR0_AVX512 {
            want |= XCR0_AVX512;
        }
        want &= supported;
        let cur = xgetbv0();
        if cur & want != want {
            core::arch::asm!("xsetbv", in("ecx") 0u32, in("eax") (cur | want) as u32, in("edx") ((cur | want) >> 32) as u32, options(nomem, nostack));
        }
    }
}

unsafe fn xgetbv0() -> u64 {
    let (lo, hi): (u32, u32);
    core::arch::asm!("xgetbv", in("ecx") 0u32, out("eax") lo, out("edx") hi, options(nomem, nostack));
    (hi as u64) << 32 | lo as u64
}

/// AVX2 + FMA usable on this core right now (CPU support AND enabled by the
/// OS). Use it to choose `#[target_feature(enable = "avx2,fma")]` code paths.
pub fn avx2() -> bool {
    use core::sync::atomic::AtomicU8;
    static S: AtomicU8 = AtomicU8::new(0); // 0 unknown, 1 no, 2 yes
    match S.load(Ordering::Relaxed) {
        1 => return false,
        2 => return true,
        _ => {},
    }
    let ok = unsafe {
        let (max_leaf, _, _, _) = cpu::cpuid(0, 0);
        let (_, _, c1, _) = cpu::cpuid(1, 0);
        let fma = c1 & (1 << 12) != 0;
        let osxsave = c1 & (1 << 27) != 0; // reflects CR4.OSXSAVE
        let avx2 = max_leaf >= 7 && cpu::cpuid(7, 0).1 & (1 << 5) != 0;
        fma && osxsave && avx2 && xgetbv0() & 0b110 == 0b110
    };
    S.store(if ok { 2 } else { 1 }, Ordering::Relaxed);
    ok
}

/// AVX-512F usable (CPU + OS state enabled).
pub fn avx512() -> bool {
    unsafe {
        let (max_leaf, _, _, _) = cpu::cpuid(0, 0);
        let (_, _, c1, _) = cpu::cpuid(1, 0);
        max_leaf >= 7 && c1 & (1 << 27) != 0 && cpu::cpuid(7, 0).1 & (1 << 16) != 0 && xgetbv0() & 0xE6 == 0xE6
    }
}

/// Write the wanted HWP request on this core if it changed (workers call this
/// whenever they wake; cheap when nothing changed).
#[inline]
pub fn apply_hwp(me: usize) {
    let want = HWP_WANT.load(Ordering::Relaxed);
    if want == 0 || me >= HWP_HAVE.len() || HWP_HAVE[me].load(Ordering::Relaxed) == want {
        return;
    }
    unsafe { cpu::write_msr(IA32_HWP_REQUEST, want) };
    HWP_HAVE[me].store(want, Ordering::Relaxed);
}

/// Record this core's temperature for `cpu` (workers call this when they wake).
#[inline]
pub fn note_temp(me: usize) {
    if me < TEMP_C.len() {
        if let Some(t) = core_temp() {
            TEMP_C[me].store(t as u64, Ordering::Relaxed);
        }
    }
}

/// Boot core: detect, enable HWP when present, print one line.
pub fn init() {
    let f = features();
    unsafe {
        if thermal_ok(&f) {
            TJMAX = ((cpu::read_msr(MSR_TEMPERATURE_TARGET) >> 16) & 0xFF) as u32;
        }
        if f.hwp && f.intel && !f.hypervisor {
            // package-wide, and once on it stays on until reset
            cpu::write_msr(IA32_PM_ENABLE, 1);
            HWP_WANT.store(hwp_request(EPP), Ordering::Relaxed);
        }
    }
    per_core(0);
    crate::console_write(
        format!(
            "cpu: {} (family {} model 0x{:x} stepping {}){}{}{}\n",
            brand(),
            f.family,
            f.model,
            f.stepping,
            if HWP_WANT.load(Ordering::Relaxed) != 0 { ", HWP on" } else { "" },
            if f.smep { ", SMEP on" } else { "" },
            if avx512() { ", AVX2+FMA+AVX-512 on" } else if avx2() { ", AVX2+FMA on" } else { "" }
        )
        .as_bytes(),
    );
}

/// Set the energy-performance preference on every core (0 = fastest, 255 = most frugal).
pub fn set_epp(epp: u8) -> bool {
    if HWP_WANT.load(Ordering::Relaxed) == 0 {
        return false;
    }
    unsafe { EPP = epp };
    HWP_WANT.store(hwp_request(epp), Ordering::Relaxed);
    apply_hwp(0);
    super::smp::kick();
    true
}

/// Effective MHz of this core over `us` microseconds of busy work.
fn measure_mhz(us: u64) -> Option<u32> {
    let (a0, m0) = aperf_mperf()?;
    super::timer::delay_us(us);
    let (a1, m1) = aperf_mperf()?;
    let (da, dm) = (a1.wrapping_sub(a0), m1.wrapping_sub(m0));
    if dm == 0 {
        return None;
    }
    Some((base_mhz() as u64 * da / dm) as u32)
}

/// The `cpu` command (console / SSH): report, or `cpu perf|balanced|save`.
pub fn command(arg: &str) -> String {
    let a = arg.trim();
    let mode = match a {
        "perf" | "performance" => Some((0u8, "performance")),
        "balanced" | "balance" => Some((0x80, "balanced")),
        "save" | "powersave" => Some((0xC0, "power saving")),
        _ => None,
    };
    if let Some((epp, name)) = mode {
        return if set_epp(epp) {
            format!("cpu: every core now runs {name} (HWP energy preference {epp})\n")
        } else {
            String::from("cpu: this CPU has no hardware P-states (HWP) to tune\n")
        };
    }
    let f = features();
    let (smt, pkg) = topology();
    let mut s = format!("cpu: {}\n  family {} model 0x{:02x} stepping {}", brand(), f.family, f.model, f.stepping);
    if let Some(m) = microcode() {
        s.push_str(&format!(", microcode 0x{m:x}"));
    }
    s.push_str(&format!(
        "\n  {} logical processors per package, {} per core; {} online, {} doing pixel work; APIC {}\n",
        pkg,
        smt,
        super::smp::cores_online(),
        super::smp::cores(),
        if super::smp::x2apic() { "x2APIC" } else { "xAPIC" }
    ));
    let flag = |b: bool, n: &str| if b { format!(" {n}") } else { String::new() };
    s.push_str(&format!(
        "  features:{}{}{}{}{}{}{}{}{}{}\n",
        flag(f.sse42, "sse4.2"),
        flag(f.avx, "avx"),
        flag(f.avx2, "avx2"),
        flag(f.avx512f, "avx512f"),
        flag(f.x2apic, "x2apic"),
        flag(f.smep, "smep"),
        flag(f.monitor, "monitor/mwait"),
        flag(f.turbo, "turbo"),
        flag(f.hwp, "hwp"),
        flag(f.hypervisor, "(virtual machine)")
    ));
    if f.base_mhz > 0 {
        s.push_str(&format!("  base {} MHz, max turbo {} MHz\n", f.base_mhz, f.max_mhz));
    }
    let want = HWP_WANT.load(Ordering::Relaxed);
    if want != 0 {
        let caps = unsafe { cpu::read_msr(IA32_HWP_CAPABILITIES) };
        s.push_str(&format!(
            "  HWP on: performance range {}..{} (guaranteed {}, most efficient {}), preference {} ({})\n",
            (caps >> 24) & 0xFF,
            caps & 0xFF,
            (caps >> 8) & 0xFF,
            (caps >> 16) & 0xFF,
            (want >> 24) & 0xFF,
            match (want >> 24) & 0xFF {
                0 => "performance",
                0x80 => "balanced",
                0xC0 => "power saving",
                _ => "performance-leaning",
            }
        ));
    } else {
        s.push_str("  HWP: not available (the firmware's clock policy applies)\n");
    }
    if let Some(m) = measure_mhz(20_000) {
        s.push_str(&format!("  boot core under load: {m} MHz effective\n"));
    }
    match (package_temp(), core_temp()) {
        (Some(p), Some(c)) => {
            s.push_str(&format!("  temperature: package {p} C, boot core {c} C, TjMax {} C; other cores:", unsafe { TJMAX }));
            for (i, t) in TEMP_C.iter().enumerate().skip(1).take(super::smp::cores_online().saturating_sub(1)) {
                let v = t.load(Ordering::Relaxed);
                if v > 0 {
                    s.push_str(&format!(" #{i} {v}"));
                }
            }
            s.push('\n');
        },
        (None, Some(c)) => s.push_str(&format!("  temperature: boot core {c} C\n")),
        _ => s.push_str("  temperature: no sensor readable here\n"),
    }
    s.push_str("  cpu perf | balanced | save -- set every core's energy preference\n");
    s
}
