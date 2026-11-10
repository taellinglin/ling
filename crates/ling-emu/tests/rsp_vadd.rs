//! RSP vector-unit regression: replicates PeterLemon's RSPCP2VADD test
//! (hardware-verified expected values) against the LLE core directly.

use ling_emu::n64::mem::Bus;
use ling_emu::n64::rsp_core::Rsp;

const QUAD_A: [u16; 8] = [0; 8];
const QUAD_B: [u16; 8] = [0x0011, 0x2233, 0x4455, 0x6677, 0x8899, 0xAABB, 0xCCDD, 0xEEFF];
const QUAD_C: [u16; 8] = [0xFFEE, 0xDDCC, 0xBBAA, 0x9988, 0x7766, 0x5544, 0x3322, 0x1100];

/// The RSP ucode from the test ROM, hand-assembled:
/// lqv v0,$00 · lqv v1,$10 · OP v0,v0,v1 · sqv v0,$00 · vsar hi/md/lo →
/// $10/$20/$30 · cfc2 vco/vcc/vce → $40/$42/$44 · break
fn ucode(op_word: u32) -> Vec<u32> {
    vec![
        0xC800_2000, // lqv v0[e0],$00(r0)
        0xC801_2001, // lqv v1[e0],$10(r0)
        op_word,
        0xE800_2000, // sqv v0[e0],$00(r0)
        0x4B00_001D, // vsar v0[e8]  (ACC HI)
        0xE800_2001, // sqv v0[e0],$10(r0)
        0x4B20_001D, // vsar v0[e9]  (ACC MD)
        0xE800_2002, // sqv v0[e0],$20(r0)
        0x4B40_001D, // vsar v0[e10] (ACC LO)
        0xE800_2003, // sqv v0[e0],$30(r0)
        0x4848_0000, // cfc2 t0,vco
        0xA408_0040, // sh t0,$40(r0)
        0x4848_0800, // cfc2 t0,vcc
        0xA408_0042, // sh t0,$42(r0)
        0x4848_1000, // cfc2 t0,vce
        0xA008_0044, // sb t0,$44(r0)
        0x0000_000D, // break
    ]
}

struct Out {
    vd: [u16; 8],
    acc_hi: [u16; 8],
    acc_md: [u16; 8],
    acc_lo: [u16; 8],
    vco: u16,
    vcc: u16,
    vce: u8,
}

fn run(op_word: u32, vs: [u16; 8], vt: [u16; 8]) -> Out {
    let mut bus = Bus::new(vec![0; 0x1000]);
    let mut rsp = Rsp::new();
    for n in 0..8 {
        bus.dmem[2 * n] = (vs[n] >> 8) as u8;
        bus.dmem[2 * n + 1] = vs[n] as u8;
        bus.dmem[16 + 2 * n] = (vt[n] >> 8) as u8;
        bus.dmem[16 + 2 * n + 1] = vt[n] as u8;
    }
    for (i, w) in ucode(op_word).iter().enumerate() {
        bus.imem[4 * i..4 * i + 4].copy_from_slice(&w.to_be_bytes());
    }
    bus.sp_halt = false;
    rsp.pc = 0;
    rsp.run(&mut bus, 10_000);
    assert!(bus.sp_halt && bus.sp_broke, "ucode must BREAK");
    assert!(rsp.stuck.is_none(), "rsp stuck: {:?}", rsp.stuck);
    let rd16 = |b: &Bus, a: usize| ((b.dmem[a] as u16) << 8) | b.dmem[a + 1] as u16;
    let quad = |b: &Bus, a: usize| {
        let mut q = [0u16; 8];
        for n in 0..8 {
            q[n] = rd16(b, a + 2 * n);
        }
        q
    };
    Out {
        vd: quad(&bus, 0x00),
        acc_hi: quad(&bus, 0x10),
        acc_md: quad(&bus, 0x20),
        acc_lo: quad(&bus, 0x30),
        vco: rd16(&bus, 0x40),
        vcc: rd16(&bus, 0x42),
        vce: bus.dmem[0x44],
    }
}

const VADD: u32 = 0x4A01_0010; // vadd v0,v0,v1[e0]
const VADDC: u32 = 0x4A01_0014; // vaddc v0,v0,v1[e0]

#[test]
fn vadd_matches_hardware() {
    // (vs, vt, expected vd, expected acc_lo, expected vco)
    let cases = [
        (QUAD_A, QUAD_B, QUAD_B, QUAD_B, 0u16),
        (
            QUAD_B,
            QUAD_B,
            [0x0022, 0x4466, 0x7FFF, 0x7FFF, 0x8000, 0x8000, 0x99BA, 0xDDFE],
            [0x0022, 0x4466, 0x88AA, 0xCCEE, 0x1132, 0x5576, 0x99BA, 0xDDFE],
            0,
        ),
        (QUAD_B, QUAD_C, [0xFFFF; 8], [0xFFFF; 8], 0),
        (
            QUAD_C,
            QUAD_C,
            [0xFFDC, 0xBB98, 0x8000, 0x8000, 0x7FFF, 0x7FFF, 0x6644, 0x2200],
            [0xFFDC, 0xBB98, 0x7754, 0x3310, 0xEECC, 0xAA88, 0x6644, 0x2200],
            0,
        ),
    ];
    for (i, (vs, vt, vd, lo, vco)) in cases.iter().enumerate() {
        let o = run(VADD, *vs, *vt);
        assert_eq!(o.vd, *vd, "VADD case {i}: VD");
        assert_eq!(o.acc_lo, *lo, "VADD case {i}: ACC LO");
        assert_eq!(o.acc_hi, [0; 8], "VADD case {i}: ACC HI");
        assert_eq!(o.acc_md, [0; 8], "VADD case {i}: ACC MD");
        assert_eq!(o.vco, *vco, "VADD case {i}: VCO");
        assert_eq!(o.vcc, 0, "VADD case {i}: VCC");
        assert_eq!(o.vce, 0, "VADD case {i}: VCE");
    }
}

#[test]
fn vaddc_matches_hardware() {
    let cases = [
        (QUAD_A, QUAD_B, QUAD_B, 0u16),
        (
            QUAD_B,
            QUAD_B,
            [0x0022, 0x4466, 0x88AA, 0xCCEE, 0x1132, 0x5576, 0x99BA, 0xDDFE],
            0x00F0,
        ),
        (QUAD_B, QUAD_C, [0xFFFF; 8], 0x0000),
        (
            QUAD_C,
            QUAD_C,
            [0xFFDC, 0xBB98, 0x7754, 0x3310, 0xEECC, 0xAA88, 0x6644, 0x2200],
            0x000F,
        ),
    ];
    for (i, (vs, vt, vd, vco)) in cases.iter().enumerate() {
        let o = run(VADDC, *vs, *vt);
        assert_eq!(o.vd, *vd, "VADDC case {i}: VD");
        assert_eq!(o.acc_lo, *vd, "VADDC case {i}: ACC LO");
        assert_eq!(o.vco, *vco, "VADDC case {i}: VCO");
        assert_eq!(o.vce, 0, "VADDC case {i}: VCE");
    }
}

/// The ROM reruns one ucode on one RSP; replicate that exact flow:
/// same Rsp + Bus across all 8 sub-tests, reload DMEM + PC each time.
#[test]
fn vadd_sequential_like_rom() {
    let mut bus = Bus::new(vec![0; 0x1000]);
    let mut rsp = Rsp::new();
    let cases: Vec<(u32, [u16; 8], [u16; 8])> = vec![
        (VADD, QUAD_A, QUAD_B),
        (VADD, QUAD_B, QUAD_B),
        (VADD, QUAD_B, QUAD_C),
        (VADD, QUAD_C, QUAD_C),
        (VADDC, QUAD_A, QUAD_B),
        (VADDC, QUAD_B, QUAD_B),
        (VADDC, QUAD_B, QUAD_C),
        (VADDC, QUAD_C, QUAD_C),
    ];
    for (i, (opw, vs, vt)) in cases.iter().enumerate() {
        for n in 0..8 {
            bus.dmem[2 * n] = (vs[n] >> 8) as u8;
            bus.dmem[2 * n + 1] = vs[n] as u8;
            bus.dmem[16 + 2 * n] = (vt[n] >> 8) as u8;
            bus.dmem[16 + 2 * n + 1] = vt[n] as u8;
        }
        for (k, w) in ucode(*opw).iter().enumerate() {
            bus.imem[4 * k..4 * k + 4].copy_from_slice(&w.to_be_bytes());
        }
        bus.sp_broke = false;
        bus.sp_halt = false;
        rsp.pc = 0;
        rsp.run(&mut bus, 10_000);
        assert!(bus.sp_halt && bus.sp_broke, "case {i}: must BREAK");
        let rd16 = |b: &Bus, a: usize| ((b.dmem[a] as u16) << 8) | b.dmem[a + 1] as u16;
        let quad = |b: &Bus, a: usize| {
            let mut q = [0u16; 8];
            for n in 0..8 {
                q[n] = rd16(b, a + 2 * n);
            }
            q
        };
        let hi = quad(&bus, 0x10);
        let md = quad(&bus, 0x20);
        assert_eq!(hi, [0; 8], "case {i}: ACC HI leaked state");
        assert_eq!(md, [0; 8], "case {i}: ACC MD leaked state");
    }
}
