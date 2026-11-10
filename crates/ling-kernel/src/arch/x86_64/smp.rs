//! Symmetric multiprocessing: wake the machine's other cores (the application
//! processors, APs) and use them as a worker pool for data-parallel work --
//! 3-D rasterizing in screen bands, scaling and copying frames to the screen.
//!
//! The kernel itself stays single-core: the scheduler, the Ling runtime, the
//! collector, every driver and the heap run on the boot core (BSP) only. A
//! worker only ever runs a job function handed to [`parallel_for`] while the
//! BSP waits inside that same call, so the job may borrow anything the caller
//! could. Job functions must not allocate, print, take locks or call into the
//! Ling runtime: the heap's lock only masks interrupts on its own core.
//!
//! Bring-up: a 16-bit trampoline copied to 0x8000 (below 1 MiB, clear of the
//! disk-boot E820 / LFB handoffs at 0x5000 / 0x6000) goes real -> protected ->
//! long mode on the boot page tables, takes an index and a stack, and calls
//! [`ap_main`], which switches to the kernel's own page tables, GDT (with a
//! TSS per core for its fault stacks) and IDT, enables its local APIC and
//! waits for work. Wake-up is INIT-SIPI-SIPI broadcast to all other cores;
//! idle workers sleep in `hlt` and are woken by a fixed IPI.
use super::{cpu, gdt, idt, paging, timer};
use core::arch::{asm, global_asm};
use core::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};

/// Cores including the boot core. Cores past this stay parked in `hlt`.
pub const MAX_CPUS: usize = 16;
pub const MAX_APS: usize = MAX_CPUS - 1;
/// The vector of the "work is ready" IPI (idt.rs installs its handler).
pub const WAKE_VECTOR: u8 = 0xF0;
const TRAMP: u64 = 0x8000;
const AP_STACK: usize = 256 * 1024;
const FAULT_STACK: usize = 16 * 1024;

const LAPIC_ID: u64 = 0x20;
const LAPIC_TPR: u64 = 0x80;
const LAPIC_EOI: u64 = 0xB0;
const LAPIC_SVR: u64 = 0xF0;
const LAPIC_ICR_LO: u64 = 0x300;
const LAPIC_ICR_HI: u64 = 0x310;
const LAPIC_LVT_TIMER: u64 = 0x320;
const LAPIC_LINT0: u64 = 0x350;
const LAPIC_LINT1: u64 = 0x360;
const LAPIC_LVT_ERR: u64 = 0x370;

global_asm!(
    r#"
.section .rodata
.set LAPT_COUNT, ling_ap_count - ling_ap_tramp_start
.set LAPT_CR3, ling_ap_cr3 - ling_ap_tramp_start
.set LAPT_ENTRY, ling_ap_entry - ling_ap_tramp_start
.set LAPT_GDTP, ling_ap_gdtp - ling_ap_tramp_start
.set LAPT_STACKS, ling_ap_stacks - ling_ap_tramp_start
.align 16
.global ling_ap_tramp_start
ling_ap_tramp_start:
.code16
    cli
    cld
    xor ax, ax
    mov ds, ax
    lgdt [0x8000 + LAPT_GDTP]
    mov eax, cr0
    or eax, 1
    mov cr0, eax
    // jmp dword 0x08:ap_pm (66 EA = far jump with a 32-bit offset in 16-bit code)
    .byte 0x66, 0xEA
    .long 0x8000 + (ling_ap_pm - ling_ap_tramp_start)
    .short 0x08
.code32
ling_ap_pm:
    mov ax, 0x10
    mov ds, ax
    mov es, ax
    mov fs, ax
    mov gs, ax
    mov ss, ax
    // PAE + OSFXSR + OSXMMEXCPT
    mov eax, cr4
    or eax, 0x620
    mov cr4, eax
    mov eax, [0x8000 + LAPT_CR3]
    mov cr3, eax
    // EFER: long mode + no-execute (the kernel's tables use NX bits)
    mov ecx, 0xC0000080
    rdmsr
    or eax, 0x900
    wrmsr
    // paging + write-protect + MP, no x87 emulation
    mov eax, cr0
    and eax, 0xFFFFFFFB
    or eax, 0x80010002
    mov cr0, eax
    .byte 0xEA
    .long 0x8000 + (ling_ap_lm - ling_ap_tramp_start)
    .short 0x18
.code64
ling_ap_lm:
    mov ax, 0x10
    mov ds, ax
    mov es, ax
    mov ss, ax
    mov eax, 1
    lock xadd dword ptr [0x8000 + LAPT_COUNT], eax
    cmp eax, 15
    jae 2f
    mov rsp, qword ptr [0x8000 + LAPT_STACKS + rax*8]
    mov edi, eax
    mov rax, qword ptr [0x8000 + LAPT_ENTRY]
    call rax
2:
    cli
    hlt
    jmp 2b
.align 8
ling_ap_gdt:
    .quad 0
    .quad 0x00CF9A000000FFFF
    .quad 0x00CF92000000FFFF
    .quad 0x00AF9A000000FFFF
ling_ap_gdtp:
    .short 31
    .long 0x8000 + (ling_ap_gdt - ling_ap_tramp_start)
.align 8
.global ling_ap_cr3
ling_ap_cr3:
    .quad 0
.global ling_ap_entry
ling_ap_entry:
    .quad 0
.global ling_ap_count
ling_ap_count:
    .quad 0
.global ling_ap_stacks
ling_ap_stacks:
    .skip 8 * 15
.global ling_ap_tramp_end
ling_ap_tramp_end:
.text
"#
);

extern "C" {
    static ling_ap_tramp_start: u8;
    static ling_ap_tramp_end: u8;
    static ling_ap_cr3: u8;
    static ling_ap_entry: u8;
    static ling_ap_count: u8;
    static ling_ap_stacks: u8;
    static ling_boot_pml4: u8;
}

static mut LAPIC: u64 = 0;
/// Application processors that reached `ap_main` and are waiting for work.
static ONLINE: AtomicUsize = AtomicUsize::new(0);
/// How many workers may take part (`cores N`); starts at every online AP.
static ACTIVE: AtomicUsize = AtomicUsize::new(0);
/// The current job: generation (24 bits) | task count (20) | next task (20).
static JOB: AtomicU64 = AtomicU64::new(0);
static JOB_FN: AtomicUsize = AtomicUsize::new(0);
static JOB_CTX: AtomicUsize = AtomicUsize::new(0);
static DONE: AtomicUsize = AtomicUsize::new(0);
static BUSY: AtomicBool = AtomicBool::new(false);
static mut GEN: u64 = 0;
/// Per-worker tasks done (index 0 = the boot core), for `cores` / the profiler.
static TASKS_BY: [AtomicU64; MAX_CPUS] = [const { AtomicU64::new(0) }; MAX_CPUS];
static JOBS: AtomicU64 = AtomicU64::new(0);
static JOB_US: AtomicU64 = AtomicU64::new(0);

const MASK20: u64 = (1 << 20) - 1;

#[inline]
fn abs_sym(s: &u8) -> u64 {
    s as *const u8 as u64
}

/// The local APICs run in x2APIC mode (registers are MSRs 0x800 + reg/16,
/// not memory). Firmware on many-core machines often leaves it on, and once on
/// it stays on until reset, so every core follows the boot core's mode.
static mut X2: bool = false;

unsafe fn lapic_read(reg: u64) -> u32 {
    if X2 {
        cpu::read_msr(0x800 + (reg >> 4) as u32) as u32
    } else {
        core::ptr::read_volatile((LAPIC + reg) as *const u32)
    }
}

unsafe fn lapic_write(reg: u64, v: u32) {
    if X2 {
        cpu::write_msr(0x800 + (reg >> 4) as u32, v as u64);
    } else {
        core::ptr::write_volatile((LAPIC + reg) as *mut u32, v);
    }
}

unsafe fn icr_wait() {
    if X2 {
        return; // x2APIC has no delivery-status bit: the MSR write is the send
    }
    let mut n = 0u32;
    while lapic_read(LAPIC_ICR_LO) & (1 << 12) != 0 && n < 1_000_000 {
        cpu::pause();
        n += 1;
    }
}

/// Send an IPI to every core but this one (destination shorthand 0b11).
unsafe fn ipi_others(low: u32) {
    let low = low | (0b11 << 18);
    if X2 {
        cpu::write_msr(0x830, low as u64);
        return;
    }
    icr_wait();
    lapic_write(LAPIC_ICR_HI, 0);
    lapic_write(LAPIC_ICR_LO, low);
    icr_wait();
}

/// Wake every sleeping worker (they re-read the HWP request and temperature).
pub fn kick() {
    unsafe {
        if X2 || LAPIC != 0 {
            ipi_others(WAKE_VECTOR as u32);
        }
    }
}

/// End-of-interrupt for the wake IPI (called from its handler in idt.rs).
pub fn eoi() {
    unsafe {
        if X2 || LAPIC != 0 {
            lapic_write(LAPIC_EOI, 0);
        }
    }
}

/// This core's local APIC id.
pub fn apic_id() -> u32 {
    unsafe {
        if X2 {
            lapic_read(LAPIC_ID)
        } else if LAPIC != 0 {
            lapic_read(LAPIC_ID) >> 24
        } else {
            cpu::cpuid(1, 0).1 >> 24
        }
    }
}

/// Whether the APICs run in x2APIC mode.
pub fn x2apic() -> bool {
    unsafe { X2 }
}

/// Cores in use for parallel work, the boot core included (1 = no SMP).
pub fn cores() -> usize {
    ACTIVE.load(Ordering::Relaxed) + 1
}

/// Cores that came up at boot, the boot core included.
pub fn cores_online() -> usize {
    ONLINE.load(Ordering::Relaxed) + 1
}

/// Limit parallel work to `n` cores (1 = everything on the boot core again).
pub fn set_cores(n: usize) -> usize {
    let n = n.clamp(1, cores_online());
    ACTIVE.store(n - 1, Ordering::Relaxed);
    n
}

/// (jobs run, microseconds inside them, tasks per core) since boot.
pub fn stats() -> (u64, u64, [u64; MAX_CPUS]) {
    let mut by = [0u64; MAX_CPUS];
    for (i, t) in TASKS_BY.iter().enumerate() {
        by[i] = t.load(Ordering::Relaxed);
    }
    (JOBS.load(Ordering::Relaxed), JOB_US.load(Ordering::Relaxed), by)
}

/// Wake the other cores. Call once, after paging, the GDT/IDT and the heap
/// are up. Returns the number of cores now available (1 when there is no
/// local APIC, x2APIC-only firmware, or no other core answered).
pub fn init() -> usize {
    super::cpudrv::init();
    unsafe {
        let (_, _, _, edx) = cpu::cpuid(1, 0);
        if edx & (1 << 9) == 0 {
            crate::console_write(b"smp: no local APIC, one core\n");
            return 1;
        }
        let base = cpu::read_msr(0x1B);
        if base & (1 << 11) == 0 {
            cpu::write_msr(0x1B, base | (1 << 11));
        }
        X2 = base & (1 << 10) != 0;
        LAPIC = base & 0xF_FFFF_F000;
        if !X2 && LAPIC >= 1 << 32 {
            crate::console_write(b"smp: local APIC above 4 GiB, one core\n");
            return 1;
        }
        // The boot core's APIC must be software-enabled to send IPIs. If the
        // firmware left it off, turn it on in the standard virtual-wire setup
        // so the 8259 PIC's interrupts keep arriving through LINT0.
        let svr = lapic_read(LAPIC_SVR);
        if svr & 0x100 == 0 {
            lapic_write(LAPIC_LINT0, 0x700); // ExtINT
            lapic_write(LAPIC_LINT1, 0x400); // NMI
            lapic_write(LAPIC_SVR, 0x1FF);
        }

        // The other cores' stacks and fault stacks: allocated here, on the
        // boot core, since the heap is not safe to use from another core.
        let start = abs_sym(&ling_ap_tramp_start);
        let len = (abs_sym(&ling_ap_tramp_end) - start) as usize;
        core::ptr::copy_nonoverlapping(start as *const u8, TRAMP as *mut u8, len);
        let at = |label: &u8| (TRAMP + abs_sym(label) - start) as *mut u64;
        core::ptr::write_volatile(at(&ling_ap_cr3), abs_sym(&ling_boot_pml4));
        core::ptr::write_volatile(at(&ling_ap_entry), ap_main as *const () as usize as u64);
        core::ptr::write_volatile(at(&ling_ap_count), 0);
        let stacks = at(&ling_ap_stacks);
        for i in 0..MAX_APS {
            let s: &'static mut [u8] = alloc::vec![0u8; AP_STACK].leak();
            let top = (s.as_ptr() as u64 + AP_STACK as u64) & !0xF;
            core::ptr::write_volatile(stacks.add(i), top);
            let f: &'static mut [u8] = alloc::vec![0u8; FAULT_STACK].leak();
            gdt::prepare_ap(i, (f.as_ptr() as u64 + FAULT_STACK as u64) & !0xF);
        }

        // INIT, then two STARTUPs at the trampoline's page (Intel MP spec).
        ipi_others(0x0000_4500);
        timer::delay_us(10_000);
        for _ in 0..2 {
            ipi_others(0x0000_4600 | (TRAMP >> 12) as u32);
            timer::delay_us(200);
        }
        // Wait until the count of started cores stops changing.
        let count = at(&ling_ap_count) as *const u32;
        let mut last = 0u32;
        let mut stable = 0u32;
        let t0 = timer::now_us();
        while timer::now_us() - t0 < 500_000 {
            timer::delay_us(5_000);
            let c = core::ptr::read_volatile(count);
            let online = ONLINE.load(Ordering::Acquire) as u32;
            if c == last && online >= c.min(MAX_APS as u32) {
                stable += 1;
                if stable >= 4 {
                    break;
                }
            } else {
                stable = 0;
            }
            last = c;
        }
        let started = core::ptr::read_volatile(count) as usize;
        let n = ONLINE.load(Ordering::Acquire);
        ACTIVE.store(n, Ordering::Relaxed);
        crate::console_write(b"smp: ");
        crate::print_decimal((n + 1) as u64);
        crate::console_write(b" cores working (boot core + ");
        crate::print_decimal(n as u64);
        crate::console_write(b" of ");
        crate::print_decimal(started as u64);
        crate::console_write(b" started); boot APIC id ");
        crate::print_decimal(apic_id() as u64);
        crate::console_write(if X2 { b" (x2APIC)\n".as_slice() } else { b" (xAPIC)\n".as_slice() });
        n + 1
    }
}

extern "C" fn ap_main(idx: u32) -> ! {
    unsafe {
        cpu::write_cr3(paging::kernel_pml4());
        if X2 {
            // follow the boot core into x2APIC mode (xAPIC first, then x2APIC)
            let b = cpu::read_msr(0x1B);
            if b & (1 << 10) == 0 {
                cpu::write_msr(0x1B, b | (1 << 11));
                cpu::write_msr(0x1B, b | (1 << 11) | (1 << 10));
            }
        }
        gdt::load_ap(idx as usize);
        idt::load();
        asm!("fninit", options(nomem, nostack));
        let mx: u32 = 0x1F80;
        asm!("ldmxcsr [{}]", in(reg) &mx, options(nostack, readonly));
        lapic_write(LAPIC_TPR, 0);
        lapic_write(LAPIC_LVT_TIMER, 0x1_0000);
        lapic_write(LAPIC_LINT0, 0x1_0000);
        lapic_write(LAPIC_LINT1, 0x1_0000);
        lapic_write(LAPIC_LVT_ERR, 0x1_0000);
        lapic_write(LAPIC_SVR, 0x1FF);
    }
    super::cpudrv::per_core(idx as usize + 1);
    super::cpudrv::note_temp(idx as usize + 1);
    ONLINE.fetch_add(1, Ordering::Release);
    worker(idx as usize + 1)
}

#[inline]
fn claimable(v: u64) -> bool {
    (v & MASK20) < ((v >> 20) & MASK20)
}

/// Take the next task of generation `gen`, or None when it has none left.
#[inline]
fn claim(gen: u64) -> Option<usize> {
    let mut v = JOB.load(Ordering::Acquire);
    loop {
        if v >> 40 != gen || !claimable(v) {
            return None;
        }
        match JOB.compare_exchange_weak(v, v + 1, Ordering::AcqRel, Ordering::Acquire) {
            Ok(_) => return Some((v & MASK20) as usize),
            Err(now) => v = now,
        }
    }
}

#[inline]
unsafe fn run_task(i: usize) {
    let f: unsafe fn(usize, usize) = core::mem::transmute(JOB_FN.load(Ordering::Acquire));
    f(JOB_CTX.load(Ordering::Acquire), i);
}

fn worker(me: usize) -> ! {
    loop {
        let mut spins = 0u32;
        let v = loop {
            let v = JOB.load(Ordering::Acquire);
            if claimable(v) && me <= ACTIVE.load(Ordering::Relaxed) {
                break v;
            }
            spins += 1;
            if spins < 50_000 {
                unsafe { cpu::pause() };
            } else {
                // Sleep until the next wake IPI. `sti; hlt` is atomic (the
                // interrupt shadow), so an IPI sent after the check still wakes us.
                unsafe {
                    asm!("cli", options(nomem, nostack));
                    let v2 = JOB.load(Ordering::Acquire);
                    if claimable(v2) && me <= ACTIVE.load(Ordering::Relaxed) {
                        asm!("sti", options(nomem, nostack));
                        continue;
                    }
                    asm!("sti; hlt", options(nomem, nostack));
                }
                spins = 0;
                // woken: take a new energy preference, note the temperature
                super::cpudrv::apply_hwp(me);
                super::cpudrv::note_temp(me);
            }
        };
        let gen = v >> 40;
        super::cpudrv::apply_hwp(me);
        while let Some(i) = claim(gen) {
            unsafe { run_task(i) };
            TASKS_BY[me].fetch_add(1, Ordering::Relaxed);
            DONE.fetch_add(1, Ordering::Release);
        }
    }
}

unsafe fn trampoline<F: Fn(usize) + Sync>(ctx: usize, i: usize) {
    (*(ctx as *const F))(i)
}

/// Run `f(0) .. f(n-1)` spread over every working core (the boot core takes
/// part) and return when all are done. Falls back to a plain loop with one
/// core, one task, or when called from inside another job. `f` must not
/// allocate, print or call into the Ling runtime (see the module doc).
pub fn parallel_for<F: Fn(usize) + Sync>(n: usize, f: &F) {
    if n == 0 {
        return;
    }
    let n = n.min(MASK20 as usize);
    if n == 1 || ACTIVE.load(Ordering::Relaxed) == 0 || BUSY.swap(true, Ordering::Acquire) {
        for i in 0..n {
            f(i);
        }
        return;
    }
    let t0 = timer::now_us();
    let gen = unsafe {
        GEN = (GEN + 1) & 0xFF_FFFF;
        GEN
    };
    JOB_FN.store(trampoline::<F> as unsafe fn(usize, usize) as usize, Ordering::Relaxed);
    JOB_CTX.store(f as *const F as usize, Ordering::Relaxed);
    DONE.store(0, Ordering::Relaxed);
    JOB.store((gen << 40) | ((n as u64) << 20), Ordering::Release);
    unsafe { ipi_others(WAKE_VECTOR as u32) };
    while let Some(i) = claim(gen) {
        f(i);
        TASKS_BY[0].fetch_add(1, Ordering::Relaxed);
        DONE.fetch_add(1, Ordering::Release);
    }
    while DONE.load(Ordering::Acquire) < n {
        unsafe { cpu::pause() };
    }
    BUSY.store(false, Ordering::Release);
    JOBS.fetch_add(1, Ordering::Relaxed);
    JOB_US.fetch_add(timer::now_us().saturating_sub(t0), Ordering::Relaxed);
}
