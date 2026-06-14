//! lingfu normalizer end-to-end tests.
//!
//! Drives the real `lingfu` binary: writes an English Ling program into a temp
//! directory, runs `lingfu normalize <lang>`, and asserts the translated
//! keywords and builtins appear. This guards the multilingual guarantee — a
//! program normalized to any supported language must use that language's tokens.

use std::fs;
use std::path::PathBuf;
use std::process::Command;

const LINGFU: &str = env!("CARGO_BIN_EXE_lingfu");

const PROGRAM: &str = "bind start = do {\n  print(sin(0.0))\n  print(sqrt(16.0))\n  print(max(3.0, 7.0))\n  fill(0, 0, 0)\n  present()\n}\n";

/// Normalize `PROGRAM` to `lang` in an isolated temp dir and return the result.
fn normalize_to(lang: &str) -> String {
    let dir = std::env::temp_dir().join(format!("lingfu_norm_{lang}_{}", std::process::id()));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).expect("create temp dir");
    let file = dir.join("prog.ling");
    fs::write(&file, PROGRAM).expect("write program");

    let status = Command::new(LINGFU)
        .arg("normalize")
        .arg(lang)
        .arg("--content-only")
        .current_dir(&dir)
        .status()
        .expect("run lingfu");
    assert!(status.success(), "lingfu normalize {lang} exited with failure");

    let out = fs::read_to_string(&file).expect("read normalized program");
    let _ = fs::remove_dir_all(&dir);
    out
}

#[test]
fn normalize_to_chinese() {
    let out = normalize_to("zh");
    assert!(out.contains('令'), "bind → 令\n{out}");
    assert!(out.contains("正弦"), "sin → 正弦\n{out}");
    assert!(out.contains("平方根"), "sqrt → 平方根\n{out}");
    assert!(out.contains("最大"), "max → 最大\n{out}");
}

#[test]
fn normalize_to_japanese() {
    let out = normalize_to("ja");
    assert!(out.contains("束縛"), "bind → 束縛\n{out}");
    assert!(out.contains("サイン"), "sin → サイン\n{out}");
    assert!(out.contains("平方根"), "sqrt → 平方根\n{out}");
}

#[test]
fn normalize_to_korean() {
    let out = normalize_to("ko");
    assert!(out.contains("바인드"), "bind → 바인드\n{out}");
    assert!(out.contains("사인"), "sin → 사인\n{out}");
    assert!(out.contains("제곱근"), "sqrt → 제곱근\n{out}");
}

#[test]
fn normalize_to_thai() {
    let out = normalize_to("th");
    assert!(out.contains("ผูก"), "bind → ผูก\n{out}");
    assert!(out.contains("ไซน์"), "sin → ไซน์\n{out}");
    assert!(out.contains("รากที่สอง"), "sqrt → รากที่สอง\n{out}");
}

#[test]
fn normalize_input_builtins() {
    // Gamepad/joystick builtins must normalize to each language's *runtime*
    // aliases (the forms src/runtime/mod.rs actually dispatches on).
    let prog = "bind start = do {\n  pad_poll()\n  print(pad_button(0, \"a\"))\n  print(pad_lx(0))\n  pad_rumble(0, 1.0, 1.0)\n}\n";
    let cases: [(&str, [&str; 4]); 4] = [
        ("zh", ["手柄轮询", "手柄按键", "手柄左X", "手柄震动"]),
        ("ja", ["パッド更新", "パッドボタン", "パッド左X", "パッド振動"]),
        ("ko", ["패드폴링", "패드버튼", "패드왼X", "패드진동"]),
        ("th", ["อัปเดตแพด", "ปุ่มแพด", "แพดซ้ายX", "แพดสั่น"]),
    ];
    for (lang, expect) in cases {
        let dir = std::env::temp_dir().join(format!("lingfu_pad_{lang}_{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        let file = dir.join("prog.ling");
        fs::write(&file, prog).unwrap();
        let status = Command::new(LINGFU)
            .arg("normalize").arg(lang).arg("--content-only")
            .current_dir(&dir).status().expect("run lingfu");
        assert!(status.success(), "normalize {lang} failed");
        let out = fs::read_to_string(&file).unwrap();
        let _ = fs::remove_dir_all(&dir);
        for token in expect {
            assert!(out.contains(token), "{lang}: expected `{token}`\n{out}");
        }
    }
}

#[test]
fn normalize_to_english_uses_bind_not_let() {
    // Round-trip a Chinese program back to English; the canonical bind keyword
    // must be `bind` (never `let`, which the runtime does not accept).
    let dir = std::env::temp_dir().join(format!("lingfu_en_{}", std::process::id()));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).unwrap();
    let file: PathBuf = dir.join("prog.ling");
    fs::write(&file, "令 start = 执 { 印(正弦(0.0)) }\n").unwrap();
    let status = Command::new(LINGFU)
        .arg("normalize").arg("en").arg("--content-only")
        .current_dir(&dir)
        .status()
        .expect("run lingfu");
    assert!(status.success());
    let out = fs::read_to_string(&file).unwrap();
    let _ = fs::remove_dir_all(&dir);
    assert!(out.contains("bind"), "令 → bind\n{out}");
    assert!(!out.contains("let "), "must not emit `let`\n{out}");
}
