# ling-emu

Native console-emulation host for [Ling](https://ling-lang.org)'s `emu_*`
builtins. **Not part of the default build**: the root crate pulls this in only
under the non-default `emu` cargo feature —

```sh
cargo build --release --bin ling --features emu
```

— so `cargo install ling-lang` ships no emulation code.

## SNES core — plays commercial games

65C816 (all 256 opcodes; native/emulation modes; decimal ADC/SBC; per-access
master-clock timing), scanline PPU (BG modes 0-4 + 7, all sprite size pairs,
window logic, main/sub-screen colour math with the hardware clip/prevent/half
rules, mosaic), general DMA + HDMA (direct and indirect), auto-joypad, H/V IRQ
timers, SPC700 executing the real IPL boot ROM, three timers, and a
line-by-line port of ares' cycle-accurate S-DSP pipeline (ISC licence).

Validated: PeterLemon's 23 CPU test ROMs show zero FAIL over 2,400 frames each
(`cargo test -p ling-emu --release --test snes_cpu`), and *A Link to the Past*
(PAL) plays from title through the intro into free movement.

Known gaps: hi-res modes 5/6, offset-per-tile, direct colour, interlace, and
the cartridge coprocessors (Super FX, SA-1, DSP-n). `Core::status()` names any
unsupported PPU feature a game uses.

## Audio

`audio.rs` is the shared sink. It pre-rolls ~70 ms, applies **dynamic rate
control** (±1.5 % slewed resample ratio keyed to buffer fill, so an emulator
that is 50 Hz, 60.0988 Hz or just jittery never drains or floods the buffer),
Catmull-Rom cubic interpolation, and fades out on underrun instead of cutting.
A simulation test pushes a bursty producer 1 % off the device clock for 60 s
and asserts no underruns, bounded latency and a smooth 440 Hz sine; with rate
control disabled the same test fails (buffer grows to ~650 ms).

## N64 core — current state (bring-up)

Working today:

- **MIPS R4300i interpreter** — 64-bit GPRs, delay slots + likely branches,
  CP0 (Count/Compare timer, interrupts, exceptions, ERET), CP1 floats with
  `Status.FR` pairing, minimal TLB, the unaligned load/store family.
- **HLE IPL3 boot** — byte-order normalization (.z64/.v64/.n64), CIC detection
  by IPL3 CRC (6101/6102/6103/6105/6106), libultra boot globals
  (osTvType/osMemSize/…), 1 MB code copy, boot registers.
- **Bus/MMIO** — RDRAM (8 MB), SP DMEM/IMEM + SP DMA, MI interrupts, VI
  scanout (16/32-bit framebuffers), AI (silent-drain), PI DMA, SI DMA,
  PIF joybus (controllers + pak-less probe), **16-kbit EEPROM**, and the
  **CIC-NUS-6105 anti-piracy challenge** (algorithm reimplemented from the
  ISC-licensed ares emulator).

Validated: PeterLemon's CPU-rendered bare-metal demos boot and display
(`cargo test -p ling-emu`, `examples/probe.rs`).

- **RSP LLE** — full scalar + vector unit (every VU op, 48-bit accumulators,
  VCO/VCC/VCE flags, RCP/RSQ tables, all vector load/store formats), lane
  semantics ported from the ISC-licensed ares serial implementations.
  Validated against PeterLemon's RSPTest CP2 ROMs (VADD/VADDC, VMACF/VMACU,
  VRCP/VRSQ, VCL/VCH all PASS) plus in-crate regression tests with
  hardware-verified vectors (`tests/rsp_vadd.rs`). Kicking the RSP runs it
  synchronously — hardware finishes small ucodes before the CPU's next MMIO
  access, and test ROMs rely on that without polling.

- **RDP software rasterizer** (`n64/rdp.rs`) — the full command set: edge-
  walked triangles (all 8 shade/texture/Z variants) with Gouraud shading,
  perspective-correct texturing and interpolated Z; fill and texture
  rectangles (incl. flip and copy mode); TMEM loads (tile, block with dxt
  line-swizzling, TLUT); tile wrap/mirror/clamp; RGBA16/32, CI4/8, IA4/8/16,
  I4/8 texels with TLUT; the two-cycle colour combiner; the blender (fog +
  alpha blend); alpha compare; the 14-bit floating Z format; scissor. The DPC
  FIFO assembles multi-word commands split across `DPC_END` updates.

**Ocarina of Time renders**: the title cinematic, logo, "PRESS START" and the
file-select menus, at ~13 ms of emulation per frame (≈52 fps inside the
arcade on a busy machine, a little under the 60 Hz target).

Deliberate RDP simplifications: one sample per pixel (no coverage/AA), no
dither, no LOD/mip-mapping, bilinear instead of the 3-point filter, no YUV.

Performance notes: an idle-loop detector fast-forwards branch-to-self loops
(Count/Compare stay exact — output was verified pixel-identical before and
after), and KSEG0/1 RDRAM accesses bypass the generic decoders. That took a
frame from 36.5 ms to 12.8 ms.

## Staged next

1. **Speed** — RSP vector registers stored as lanes instead of bytes; RDP
   inner-loop tightening. Target: a solid 60 fps in the arcade.
2. **RDP accuracy** — coverage/AA, LOD + mip-maps, dither.
3. **AI audio** — the N64 is still silent.
4. **Dynarec** — only if profiling ever points back at the CPU.

GameCube (PowerPC 750 + Flipper) is declared in `System` and honestly
unimplemented — that is Dolphin-scale work, not a weekend milestone.

## Frontend API

`load(bytes)` → boots the right core into a process-wide slot; `with_core`
drives it (`run_frame`, `framebuffer`, `set_button`, `set_stick`, `status`).
The Ling builtins (`emu_load`, `emu_frame`, `emu_blit`, `emu_button`,
`emu_stick`, `emu_status`, …) are thin wrappers; `emu_available()` exists in
every build so `.ling` programs can feature-probe safely.
