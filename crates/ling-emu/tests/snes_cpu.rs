//! Runs PeterLemon's 22 SNES CPU test ROMs (every official opcode in every
//! addressing mode and register-width combination). Each ROM prints a PASS/FAIL
//! table per variant; we sample the BG1 text every few frames and fail the
//! test if the word FAIL ever appears.

use ling_emu::snes::Snes;
use ling_emu::Core;

const DIR: &str = "C:/Users/User/Programs/LingApps/ling-arcade/tools/emu-ref-roms/snes";
const TESTS: [&str; 23] = [
    "ADC", "AND", "ASL", "BIT", "BRA", "CMP", "DEC", "EOR", "INC", "JMP", "LDR", "LSR", "MOV",
    "MSC", "ORA", "PHL", "PSR", "RET", "ROL", "ROR", "SBC", "STR", "TRN",
];

fn run(name: &str, frames: u32) -> (u32, u32, Vec<String>) {
    let path = format!("{DIR}/CPU{name}.sfc");
    let Ok(bytes) = std::fs::read(&path) else {
        return (0, 0, vec![format!("{name}: ROM missing")]);
    };
    let mut snes = Snes::boot(&bytes).expect("boot");
    let (mut pass, mut fail) = (0u32, 0u32);
    let mut fails = Vec::new();
    let mut last_title = String::new();
    for f in 0..frames {
        snes.run_frame();
        if f % 4 == 3 {
            let text = snes.ppu().bg1_text();
            let lines: Vec<&str> = text.lines().collect();
            let title = lines.get(2).copied().unwrap_or("").trim().to_string();
            if title != last_title {
                last_title = title.clone();
            }
            for l in &lines {
                if l.contains("FAIL") {
                    fail += 1;
                    if fails.len() < 6 {
                        fails.push(format!("{name} [{title}] frame {f}: {}", l.trim()));
                    }
                }
                if l.contains("PASS") {
                    pass += 1;
                }
            }
        }
    }
    (pass, fail, fails)
}

#[test]
fn all_cpu_tests_pass() {
    let mut total_fail = 0;
    let mut report = String::new();
    for t in TESTS {
        let (pass, fail, fails) = run(t, 2400);
        report.push_str(&format!("{t:4}: {pass:6} PASS-lines  {fail:4} FAIL-lines\n"));
        for f in fails {
            report.push_str(&format!("      {f}\n"));
        }
        total_fail += fail;
    }
    println!("{report}");
    assert_eq!(total_fail, 0, "CPU test failures:\n{report}");
}
