# Ling Kernel Example

Minimal x86_64 kernel written in Ling. Prints to VGA text mode, then halts.

## Requirements

```bash
rustup toolchain install nightly
rustup target add x86_64-unknown-none --toolchain nightly
```

## Build

```bash
# From ling-lang repo root
ling build examples/kernel --platform kernel
```

Output: `dist/kernel/ling-kernel-example` (ELF binary, loadable by GRUB/multiboot2)

## Run in QEMU

```bash
qemu-system-x86_64 -kernel dist/kernel/ling-kernel-example
```

## Kernel Intrinsics

| Function | Signature | Description |
|----------|-----------|-------------|
| `ling_kernel_vga_clear()` | `() -> u64` | Clear VGA text buffer |
| `ling_kernel_vga_write_str(s)` | `(u64) -> u64` | Write null-terminated string to VGA |
| `ling_kernel_vga_write_char(c)` | `(u64) -> u64` | Write character to VGA |
| `ling_kernel_serial_write(ptr, len)` | `(u64, u64) -> u64` | Write bytes to serial (COM1) |
| `ling_kernel_halt()` | `() -> u64` | Halt CPU (infinite loop) |
| `ling_kernel_cli()` | `() -> u64` | Clear interrupts |
| `ling_kernel_sti()` | `() -> u64` | Set interrupts |
| `ling_kernel_inb(port)` | `(u64) -> u64` | Read byte from I/O port |
| `ling_kernel_outb(port, val)` | `(u64, u64) -> u64` | Write byte to I/O port |
| `ling_kernel_panic(msg)` | `(u64) -> !` | Kernel panic |
| `ling_kernel_init()` | `() -> u64` | Initialize kernel (serial, VGA) |
