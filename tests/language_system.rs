//! Language-system tests for ling-lang.
//!
//! These exercise the full pipeline — lexer → parser → runtime — and the
//! central multilingual guarantee: the SAME program written in English, Chinese,
//! Japanese, Korean, or Thai must parse and execute identically.
//!
//! Run with: `cargo test --test language_system`

use ling::run;

/// A program that runs cleanly returns Ok(()).
fn assert_runs(label: &str, src: &str) {
    match run(src) {
        Ok(()) => {},
        Err(e) => {
            panic!("[{label}] expected program to run, got error: {e}\n--- source ---\n{src}")
        },
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Core execution
// ─────────────────────────────────────────────────────────────────────────────

#[test]
fn runs_minimal_program() {
    assert_runs("minimal", r#"bind start = do { print("hello") }"#);
}

#[test]
fn arithmetic_and_bind_locals() {
    assert_runs(
        "arith",
        r#"
        bind start = do {
            bind a = 2
            bind b = 40
            print(a + b)
        }
    "#,
    );
}

#[test]
fn if_else_and_while() {
    assert_runs(
        "control-flow",
        r#"
        bind start = do {
            bind n = 0
            while n < 3 {
                print(n)
                bind n = n + 1
            }
            if n > 2 { print("done") } else { print("no") }
        }
    "#,
    );
}

// ─────────────────────────────────────────────────────────────────────────────
// Multilingual keywords — the same hello-world in five languages
// ─────────────────────────────────────────────────────────────────────────────

#[test]
fn hello_world_english() {
    assert_runs("en", r#"bind start = do { print("hi") }"#);
}

#[test]
fn hello_world_chinese() {
    assert_runs("zh", r#"令 启动 = 执 { 印("你好") }"#);
}

#[test]
fn hello_world_japanese() {
    assert_runs("ja", r#"束縛 開始 = 実行 { 印刷("こんにちは") }"#);
}

#[test]
fn hello_world_korean() {
    assert_runs("ko", r#"바인드 시작 = 실행 { 출력("안녕") }"#);
}

#[test]
fn hello_world_thai() {
    assert_runs("th", r#"ผูก เริ่ม = ทำ { พิมพ์("สวัสดี") }"#);
}

// ─────────────────────────────────────────────────────────────────────────────
// Multilingual control flow — for/in + if/else + fn must parse in every
// language. (Regression guard: the Korean `for` alias was once mis-mapped to
// `while` in the lexicon, which only a running for-loop catches.)
// ─────────────────────────────────────────────────────────────────────────────

#[test]
fn for_loop_english() {
    assert_runs(
        "for-en",
        r#"bind start = do { for i in 0..3 { print(i) } }"#,
    );
}
#[test]
fn for_loop_chinese() {
    assert_runs("for-zh", r#"令 启动 = 执 { 历 i 于 0..3 { 印(i) } }"#);
}
#[test]
fn for_loop_japanese() {
    assert_runs(
        "for-ja",
        r#"束縛 開始 = 実行 { 繰 i の中 0..3 { 印刷(i) } }"#,
    );
}
#[test]
fn for_loop_korean() {
    assert_runs(
        "for-ko",
        r#"바인드 시작 = 실행 { 위해 i 안에 0..3 { 출력(i) } }"#,
    );
}
#[test]
fn for_loop_thai() {
    assert_runs("for-th", r#"ผูก เริ่ม = ทำ { สำหรับ i ใน 0..3 { พิมพ์(i) } }"#);
}

/// Recursive `fn` + if/else implicit-return — the canonical fib, in Korean.
#[test]
fn fib_recursive_korean() {
    assert_runs(
        "fib-ko",
        r#"
        함수 피보나치(n: 숫자) -> 숫자 {
            만약 n <= 1 { n } 아니면 { 피보나치(n - 1) + 피보나치(n - 2) }
        }
        바인드 시작 = 실행 { 위해 i 안에 0..8 { 출력(피보나치(i)) } }
    "#,
    );
}

#[test]
fn fib_recursive_chinese() {
    assert_runs(
        "fib-zh",
        r#"
        函 fib(n: 数字) -> 数字 {
            若 n <= 1 { n } 否则 { fib(n - 1) + fib(n - 2) }
        }
        令 启动 = 执 { 历 i 于 0..8 { 印(fib(i)) } }
    "#,
    );
}

// ─────────────────────────────────────────────────────────────────────────────
// Multilingual math builtins (Batch 1 parity) — every language must resolve.
// Each program computes the same values; success means all aliases resolved.
// ─────────────────────────────────────────────────────────────────────────────

const MATH_EN: &str = r#"bind start = do {
    print(sin(0.0)) print(cos(0.0)) print(sqrt(16.0))
    print(max(3.0, 7.0)) print(min(3.0, 7.0)) print(floor(3.9))
    print(round(2.5)) print(clamp(5.0, 0.0, 1.0)) print(pow(2.0, 8.0))
}"#;

const MATH_ZH: &str = r#"令 启动 = 执 {
    印(正弦(0.0)) 印(余弦(0.0)) 印(平方根(16.0))
    印(最大(3.0, 7.0)) 印(最小(3.0, 7.0)) 印(向下取整(3.9))
    印(四舍五入(2.5)) 印(截取(5.0, 0.0, 1.0)) 印(幂(2.0, 8.0))
}"#;

const MATH_JA: &str = r#"束縛 開始 = 実行 {
    印刷(サイン(0.0)) 印刷(コサイン(0.0)) 印刷(平方根(16.0))
    印刷(最大(3.0, 7.0)) 印刷(最小(3.0, 7.0)) 印刷(床関数(3.9))
    印刷(四捨五入(2.5)) 印刷(範囲制限(5.0, 0.0, 1.0)) 印刷(べき乗(2.0, 8.0))
}"#;

const MATH_KO: &str = r#"바인드 시작 = 실행 {
    출력(사인(0.0)) 출력(코사인(0.0)) 출력(제곱근(16.0))
    출력(최댓값(3.0, 7.0)) 출력(최솟값(3.0, 7.0)) 출력(내림(3.9))
    출력(반올림(2.5)) 출력(범위제한(5.0, 0.0, 1.0)) 출력(거듭제곱(2.0, 8.0))
}"#;

const MATH_TH: &str = r#"ผูก เริ่ม = ทำ {
    พิมพ์(ไซน์(0.0)) พิมพ์(โคไซน์(0.0)) พิมพ์(รากที่สอง(16.0))
    พิมพ์(สูงสุด(3.0, 7.0)) พิมพ์(ต่ำสุด(3.0, 7.0)) พิมพ์(ปัดลง(3.9))
    พิมพ์(ปัดเศษ(2.5)) พิมพ์(จำกัด(5.0, 0.0, 1.0)) พิมพ์(ยกกำลัง(2.0, 8.0))
}"#;

#[test]
fn math_builtins_english() {
    assert_runs("math-en", MATH_EN);
}
#[test]
fn math_builtins_chinese() {
    assert_runs("math-zh", MATH_ZH);
}
#[test]
fn math_builtins_japanese() {
    assert_runs("math-ja", MATH_JA);
}
#[test]
fn math_builtins_korean() {
    assert_runs("math-ko", MATH_KO);
}
#[test]
fn math_builtins_thai() {
    assert_runs("math-th", MATH_TH);
}

// ─────────────────────────────────────────────────────────────────────────────
// Mixed-language source — the killer feature: five languages in one file.
// ─────────────────────────────────────────────────────────────────────────────

#[test]
fn mixed_language_single_file() {
    assert_runs(
        "mixed",
        r#"bind start = do {
        bind x = 正弦(0.0)
        bind y = 余弦(0.0)
        print(ปัดลง(3.7))
        출력(제곱근(9.0))
        印刷(べき乗(2.0, 3.0))
    }"#,
    );
}

// ─────────────────────────────────────────────────────────────────────────────
// Phase 3/4: audio (fft) + collection builtins resolve in every language.
// ─────────────────────────────────────────────────────────────────────────────

#[test]
fn collections_and_fft_english() {
    assert_runs(
        "coll-en",
        r#"bind start = do {
        bind xs = list_new()
        bind xs2 = list_push(xs, 1.0)
        print(len(xs2))
        fft_push(0.1)
        print(fft_rms())
    }"#,
    );
}

#[test]
fn collections_and_fft_chinese() {
    assert_runs(
        "coll-zh",
        r#"令 启动 = 执 {
        令 xs = 新建列表()
        令 xs2 = 列表添加(xs, 1.0)
        印(长度(xs2))
        频谱输入(0.1)
        印(均方根())
    }"#,
    );
}

#[test]
fn collections_and_fft_korean() {
    assert_runs(
        "coll-ko",
        r#"바인드 시작 = 실행 {
        바인드 xs = 새목록()
        바인드 xs2 = 목록추가(xs, 1.0)
        출력(길이(xs2))
        FFT입력(0.1)
        출력(RMS레벨())
    }"#,
    );
}

// ─────────────────────────────────────────────────────────────────────────────
// Crypto builtins: hybrid PQ KEM round-trip + seal/open, callable from Ling.
// ─────────────────────────────────────────────────────────────────────────────

#[cfg(not(target_arch = "wasm32"))]
#[test]
fn crypto_kem_and_seal_round_trip() {
    assert_runs(
        "crypto-en",
        r#"bind start = do {
        bind id = knot_keygen()
        bind pk = knot_public(id)
        bind enc = knot_encapsulate(pk)
        bind ss = knot_decapsulate(id, enc[0])
        bind sealed = crypto_seal(enc[1], "temple at dusk")
        print(crypto_open(ss, sealed))
        print(len(knot_points(pk)))
        print(crypto_hash("ling"))
    }"#,
    );
}

#[cfg(not(target_arch = "wasm32"))]
#[test]
fn crypto_builtins_chinese() {
    assert_runs(
        "crypto-zh",
        r#"令 启动 = 执 {
        令 id = 生成密钥()
        令 pk = 公钥(id)
        令 enc = 封装密钥(pk)
        令 ss = 解封装密钥(id, enc[0])
        印(解封(ss, 封印(enc[1], "你好")))
    }"#,
    );
}

// ─────────────────────────────────────────────────────────────────────────────
// Polyglot language detection
// ─────────────────────────────────────────────────────────────────────────────

#[test]
fn detects_languages() {
    // Detection is heuristic; just assert it returns a non-empty label and does
    // not panic for each script.
    for (label, src) in [
        ("en", "bind start = do { print(1) }"),
        ("zh", "令 启动 = 执 { 印(1) }"),
        ("th", "ผูก เริ่ม = ทำ { พิมพ์(1) }"),
    ] {
        let lang = ling::detect_language(src);
        assert!(!lang.is_empty(), "[{label}] detect_language returned empty");
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Error handling — unknown functions must error, not panic.
// ─────────────────────────────────────────────────────────────────────────────

#[test]
fn unknown_function_is_error_not_panic() {
    let res = run(r#"bind start = do { this_is_not_a_builtin(1) }"#);
    assert!(
        res.is_err(),
        "calling an unknown builtin should be an error"
    );
}
