use core::arch::asm;

pub unsafe fn halt() {
    asm!("hlt", options(nomem, nostack));
}

pub unsafe fn cli() {
    asm!("cli", options(nomem, nostack));
}

pub unsafe fn sti() {
    asm!("sti", options(nomem, nostack));
}

pub unsafe fn pause() {
    asm!("pause", options(nomem, nostack));
}

pub unsafe fn cpuid(eax: u32, ecx: u32) -> (u32, u32, u32, u32) {
    let a: u32;
    let bval: u32;
    let c: u32;
    let d: u32;
    asm!(
        "push rbx",
        "cpuid",
        "mov {b:e}, ebx",
        "pop rbx",
        inout("eax") eax => a,
        inout("ecx") ecx => c,
        out("edx") d,
        b = out(reg) bval,
        options(nomem, nostack)
    );
    (a, bval, c, d)
}

pub unsafe fn rdtsc() -> u64 {
    let lo: u32;
    let hi: u32;
    asm!("rdtsc", out("eax") lo, out("edx") hi, options(nomem, nostack));
    (lo as u64) | ((hi as u64) << 32)
}

pub unsafe fn read_cr2() -> u64 {
    let val: u64;
    asm!("mov {}, cr2", out(reg) val, options(nomem, nostack));
    val
}

pub unsafe fn read_cr3() -> u64 {
    let val: u64;
    asm!("mov {}, cr3", out(reg) val, options(nomem, nostack));
    val
}

pub unsafe fn write_cr3(val: u64) {
    asm!("mov cr3, {}", in(reg) val, options(nomem, nostack));
}

pub unsafe fn int3() {
    asm!("int3", options(nomem, nostack));
}

pub unsafe fn read_msr(msr: u32) -> u64 {
    let lo: u32;
    let hi: u32;
    asm!("rdmsr", out("eax") lo, out("edx") hi, in("ecx") msr, options(nomem, nostack));
    (lo as u64) | ((hi as u64) << 32)
}

pub unsafe fn write_msr(msr: u32, val: u64) {
    let lo = val as u32;
    let hi = (val >> 32) as u32;
    asm!("wrmsr", in("eax") lo, in("edx") hi, in("ecx") msr, options(nomem, nostack));
}

pub unsafe fn read_flags() -> u64 {
    let val: u64;
    asm!("pushfq; pop {}", out(reg) val, options(nomem, nostack));
    val
}
