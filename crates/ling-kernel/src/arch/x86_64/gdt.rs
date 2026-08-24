//! Global Descriptor Table + Task State Segment.
//!
//! Long mode ignores segment base/limit for code/data (paging does all
//! address translation), so this GDT exists only to select ring 0 vs. ring 3
//! and to give the CPU a TSS to find IST stacks in — not for classic x86
//! segmentation. The temporary 1-entry GDT `boot.rs` builds to get into long
//! mode is replaced by this one before interrupts are enabled.
//!
//! Selector layout is fixed now, ahead of Phase 3's `syscall`/`sysret`,
//! because `sysret`'s segment-loading math is selector-order-dependent: for
//! `STAR[63:48] = base`, `sysret` sets `SS = base + 8` and `CS = base + 16`.
//! That only lands on the right descriptors if user data sits immediately
//! before user code in the table, with an (unused) placeholder before both —
//! see the selector constants below. Getting this wrong is a Phase 3 bug
//! that's cheaper to avoid now than to debug later, so the ordering is
//! correct from the first commit even though nothing runs in ring 3 yet.
use core::arch::asm;
use core::mem::size_of;

#[repr(C, packed)]
struct Tss {
    reserved0: u32,
    rsp0: u64,
    rsp1: u64,
    rsp2: u64,
    reserved1: u64,
    ist: [u64; 7],
    reserved2: u64,
    reserved3: u16,
    iomap_base: u16,
}

const DOUBLE_FAULT_STACK_SIZE: usize = 16 * 1024;
const TIMER_STACK_SIZE: usize = 16 * 1024;
const KERNEL_BOOT_STACK_TOP_PLACEHOLDER: u64 = 0;

static mut DOUBLE_FAULT_STACK: [u8; DOUBLE_FAULT_STACK_SIZE] = [0; DOUBLE_FAULT_STACK_SIZE];
static mut TIMER_STACK: [u8; TIMER_STACK_SIZE] = [0; TIMER_STACK_SIZE];
static mut TSS: Tss = Tss {
    reserved0: 0,
    rsp0: KERNEL_BOOT_STACK_TOP_PLACEHOLDER,
    rsp1: 0,
    rsp2: 0,
    reserved1: 0,
    ist: [0; 7],
    reserved2: 0,
    reserved3: 0,
    iomap_base: size_of::<Tss>() as u16,
};

const GDT_ENTRIES: usize = 9;
static mut GDT: [u64; GDT_ENTRIES] = [0; GDT_ENTRIES];

pub const KERNEL_CODE_SEL: u16 = 1 * 8;
pub const KERNEL_DATA_SEL: u16 = 2 * 8;
/// Unused as an actual segment — exists only so `USER_DATA_SEL`/`USER_CODE_SEL`
/// land at `base+8`/`base+16` for `sysret` (see the module doc). Its GDT slot
/// (index 3) is populated in `init()`; the selector constant itself has no
/// reader until Phase 3 wires up `syscall`/`sysret`.
#[allow(dead_code)]
const USER_PLACEHOLDER_SEL: u16 = 3 * 8;
pub const USER_DATA_SEL: u16 = (4 * 8) | 3;
pub const USER_CODE_SEL: u16 = (5 * 8) | 3;
const TSS_SEL: u16 = 6 * 8;

/// IST index (1-based, matching `Tss::ist`'s hardware numbering) reserved for
/// double-fault/NMI handlers, which must never run on a stack that might
/// itself be the reason a fault happened (a blown kernel stack overflowing
/// into a guard page, once `mm/` adds one, would otherwise turn a single
/// page fault into an unrecoverable triple fault via infinite re-entry).
pub const DOUBLE_FAULT_IST: u8 = 1;

/// IST index for the timer vector. Not for stack *safety* like IST1 — it's
/// there so the hardware unconditionally pushes the full `ss:rsp` pair on
/// every timer interrupt, even one that happens to interrupt ring-0 code
/// where a privilege level change (the normal trigger for that push) never
/// occurs. Without this, the same vector's exception frame is 3 qwords when
/// it interrupts the kernel and 5 when it interrupts ring 3 — a shape that
/// depends on what got interrupted is exactly the kind of asymmetry Phase
/// 3's preemptive scheduler can't afford to special-case; the SDM documents
/// IST-based stack switching as unconditional on privilege level specifically
/// to make that frame shape uniform.
pub const TIMER_IST: u8 = 2;

fn descriptor(base: u32, limit: u32, access: u8, flags: u8) -> u64 {
    let limit_low = (limit & 0xFFFF) as u64;
    let base_low = (base & 0xFFFFFF) as u64;
    let access = access as u64;
    let limit_high_flags = (((limit >> 16) & 0xF) as u64) | (((flags & 0xF) as u64) << 4);
    let base_high = ((base >> 24) & 0xFF) as u64;
    limit_low | (base_low << 16) | (access << 40) | (limit_high_flags << 48) | (base_high << 56)
}

fn tss_descriptors(base: u64, limit: u32) -> (u64, u64) {
    let low = descriptor((base & 0xFFFF_FFFF) as u32, limit, 0x89, 0x0);
    let high = (base >> 32) & 0xFFFF_FFFF;
    (low, high)
}

/// Build the GDT/TSS, load them, and reload every segment register — must
/// run once, early, before `idt::init()` (interrupt gates reference
/// `KERNEL_CODE_SEL`) and before the first `cpu::sti()`.
pub fn init(boot_stack_top: u64) {
    unsafe {
        TSS.rsp0 = boot_stack_top;
        let df_stack_top = (&raw const DOUBLE_FAULT_STACK) as u64 + DOUBLE_FAULT_STACK_SIZE as u64;
        TSS.ist[(DOUBLE_FAULT_IST - 1) as usize] = df_stack_top & !0xF;
        let timer_stack_top = (&raw const TIMER_STACK) as u64 + TIMER_STACK_SIZE as u64;
        TSS.ist[(TIMER_IST - 1) as usize] = timer_stack_top & !0xF;

        GDT[0] = 0;
        // access=0x9A: present, ring0, code, executable+readable.
        // flags=0xA: G=1 (limit in 4KiB units, irrelevant in long mode), L=1
        // (this is a 64-bit code segment — the bit LLVM's own `retf` trick in
        // `boot.rs` set on its temporary GDT the same way).
        GDT[1] = descriptor(0, 0xFFFFF, 0x9A, 0xA);
        // access=0x92: present, ring0, data, writable. flags=0xC: G=1, D/B=1
        // (ignored for data in long mode, kept for consistency with the
        // standard flat-descriptor idiom every x86 bring-up uses).
        GDT[2] = descriptor(0, 0xFFFFF, 0x92, 0xC);
        GDT[3] = descriptor(0, 0xFFFFF, 0xFA, 0xC); // placeholder, never selected
        GDT[4] = descriptor(0, 0xFFFFF, 0xF2, 0xC); // user data, ring3
        GDT[5] = descriptor(0, 0xFFFFF, 0xFA, 0xA); // user code, ring3
        let (tss_low, tss_high) =
            tss_descriptors((&raw const TSS) as u64, (size_of::<Tss>() - 1) as u32);
        GDT[6] = tss_low;
        GDT[7] = tss_high;

        #[repr(C, packed)]
        struct GdtPointer {
            limit: u16,
            base: u64,
        }
        let pointer = GdtPointer {
            limit: (size_of::<[u64; GDT_ENTRIES]>() - 1) as u16,
            base: (&raw const GDT) as u64,
        };

        asm!("lgdt [{}]", in(reg) &pointer, options(readonly, nostack, preserves_flags));

        // Reload CS via a far return (the standard trick — there's no direct
        // `mov cs, imm` on x86), then every other segment register directly.
        asm!(
            "lea {tmp}, [rip + 55f]",
            "push {code_sel}",
            "push {tmp}",
            "retfq",
            "55:",
            tmp = out(reg) _,
            code_sel = in(reg) KERNEL_CODE_SEL as u64,
            options(nostack),
        );
        asm!(
            "mov ds, {sel:x}",
            "mov es, {sel:x}",
            "mov fs, {sel:x}",
            "mov gs, {sel:x}",
            "mov ss, {sel:x}",
            sel = in(reg) KERNEL_DATA_SEL,
            options(nostack, preserves_flags),
        );
        asm!("ltr {sel:x}", sel = in(reg) TSS_SEL, options(nostack, preserves_flags));
    }
}

/// Point `TSS.rsp0` at `stack_top` — the kernel stack the CPU switches to on
/// any ring3→ring0 transition via an IDT gate without its own IST (page
/// faults, GPFs, syscalls-via-int, ...). The scheduler calls this on every
/// switch to a different process, so a fault taken while running process A
/// always lands on A's own kernel stack, never a stale one left over from
/// whichever process ran last.
pub fn set_kernel_stack(stack_top: u64) {
    unsafe { TSS.rsp0 = stack_top };
}
