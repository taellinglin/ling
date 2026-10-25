// The aarch64 counterpart of ring3_selftest.asm: an EL0 process that writes a
// line through SYS_WRITE and exits with code 42 through SYS_EXIT. Built with
//   clang --target=aarch64-unknown-none -nostdlib -static -fuse-ld=lld \
//         -Wl,-T,ring3_selftest.ld -o hello_aarch64.elf hello_aarch64.S
// (the linker script places .text at 0x80_0000_0000, uproc's USER_CODE_BASE).
    .text
    .global _start
_start:
    mov x8, #1              // SYS_WRITE
    mov x0, #1              // fd = stdout
    adr x1, msg
    mov x2, #msg_len
    svc #0

    mov x8, #0              // SYS_EXIT
    mov x0, #42
    svc #0

1:  b 1b                    // unreachable: exit never returns to EL0

    .section .rodata
msg:
    .ascii "hello from EL0 on the Pi\n"
    .set msg_len, . - msg
