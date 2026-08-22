//! Language-system tests for ling-lang.
//!
//! These exercise the full pipeline — lexer → parser → runtime — and the
//! central multilingual guarantee: the SAME program written in English, Chinese,
//! Japanese, Korean, or Thai must parse and execute identically.
//!
//! Run with: `cargo test --test language_system`

use ling::lexer::{Lexer, Token};
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

// RTL languages (Arabic, Persian, Hebrew, Urdu) — grammar keywords are fully
// wired (lexer `classify_word`); builtin-function-name aliases (print, math,
// ...) land in a later batch, so these use the native entry-point/bind/do
// keywords with the English `print` builtin, proving the RTL keyword set
// itself lexes and parses identically to every other language.
#[test]
fn hello_world_arabic() {
    assert_runs("ar", r#"ربط ابدأ = افعل { print("أهلا") }"#);
}

#[test]
fn hello_world_persian() {
    assert_runs("fa", r#"پیوند شروع = انجام { print("سلام") }"#);
}

#[test]
fn hello_world_hebrew() {
    assert_runs("he", r#"קשר התחל = בצע { print("שלום") }"#);
}

#[test]
fn hello_world_urdu() {
    assert_runs("ur", r#"باندھو شروع = کرو { print("سلام") }"#);
}

// French and German — full peers of en/zh/ja/ko/th: grammar keywords AND
// builtin-function-name aliases are both wired (lexer + normalize.rs +
// runtime dispatch), so these get the same full for-loop/math coverage as
// the top-5 languages below, not the print-only RTL treatment above.
#[test]
fn hello_world_french() {
    assert_runs("fr", r#"lier début = faire { afficher("bonjour") }"#);
}

#[test]
fn hello_world_german() {
    assert_runs("de", r#"binden anfang = machen { drucken("hallo") }"#);
}

#[test]
fn hello_world_russian() {
    assert_runs("ru", r#"связать начать = сделать { печать("привет") }"#);
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
#[test]
fn for_loop_arabic() {
    assert_runs(
        "for-ar",
        r#"ربط ابدأ = افعل { لأجل i في 0..3 { print(i) } }"#,
    );
}
#[test]
fn for_loop_persian() {
    assert_runs(
        "for-fa",
        r#"پیوند شروع = انجام { برای i در 0..3 { print(i) } }"#,
    );
}
#[test]
fn for_loop_hebrew() {
    assert_runs(
        "for-he",
        r#"קשר התחל = בצע { עבור i בתוך 0..3 { print(i) } }"#,
    );
}
#[test]
fn for_loop_urdu() {
    assert_runs(
        "for-ur",
        r#"باندھو شروع = کرو { کے_لیے i میں 0..3 { print(i) } }"#,
    );
}
#[test]
fn for_loop_french() {
    assert_runs(
        "for-fr",
        r#"lier début = faire { pour i dans 0..3 { afficher(i) } }"#,
    );
}
#[test]
fn for_loop_german() {
    assert_runs(
        "for-de",
        r#"binden anfang = machen { für i in 0..3 { drucken(i) } }"#,
    );
}
#[test]
fn for_loop_russian() {
    assert_runs(
        "for-ru",
        r#"связать начать = сделать { для i в 0..3 { печать(i) } }"#,
    );
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

const MATH_FR: &str = r#"lier début = faire {
    afficher(sinus(0.0)) afficher(cosinus(0.0)) afficher(racine_carrée(16.0))
    afficher(maximum(3.0, 7.0)) afficher(minimum(3.0, 7.0)) afficher(plancher(3.9))
    afficher(arrondir(2.5)) afficher(limiter(5.0, 0.0, 1.0)) afficher(puissance(2.0, 8.0))
}"#;

const MATH_DE: &str = r#"binden anfang = machen {
    drucken(sinus(0.0)) drucken(kosinus(0.0)) drucken(quadratwurzel(16.0))
    drucken(maximum(3.0, 7.0)) drucken(minimum(3.0, 7.0)) drucken(abrunden(3.9))
    drucken(runden(2.5)) drucken(begrenzen(5.0, 0.0, 1.0)) drucken(potenz(2.0, 8.0))
}"#;

const MATH_RU: &str = r#"связать начать = сделать {
    печать(синус(0.0)) печать(косинус(0.0)) печать(корень(16.0))
    печать(максимум(3.0, 7.0)) печать(минимум(3.0, 7.0)) печать(вниз(3.9))
    печать(округлить(2.5)) печать(ограничить(5.0, 0.0, 1.0)) печать(степень(2.0, 8.0))
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
#[test]
fn math_builtins_french() {
    assert_runs("math-fr", MATH_FR);
}
#[test]
fn math_builtins_german() {
    assert_runs("math-de", MATH_DE);
}
#[test]
fn math_builtins_russian() {
    assert_runs("math-ru", MATH_RU);
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
// Keyword coverage — every core keyword must resolve in en/zh/ja/ko/th.
//
// Most of these keywords (post/give/fit/can/change/stop/again/try/sure/maybe/
// pure/ok/bad/none) have no dedicated grammar production yet — `parser::mod`
// only ever consumes them via its "keyword usable as a bind name" fallback —
// so a program using one still runs identically whether the word tokenized as
// the intended keyword or fell through to a plain identifier. That makes
// `assert_runs` unable to catch a missing/wrong mapping for them, so this
// checks the lexer directly instead: the one thing that actually
// distinguishes "recognized" from "silently became an Ident".
fn assert_lexes_as(word: &str, expected: Token) {
    let got = Lexer::new(word).next_token();
    assert_eq!(
        got,
        Some(expected.clone()),
        "{word:?} should lex as {expected:?}, got {got:?}"
    );
}

#[test]
fn keyword_coverage_all_five_languages() {
    // (word, expected token) — one row per newly-completed language, for the
    // keywords that were previously en/zh-only (or, for stop/again/try/spawn,
    // missing only Thai).
    let cases: &[(&str, Token)] = &[
        // type
        ("型", Token::Type),
        ("타입", Token::Type),
        ("ชนิด", Token::Type),
        // own / lend / share / move / copy
        ("所有", Token::Own),
        ("소유", Token::Own),
        ("เป็นเจ้าของ", Token::Own),
        ("貸す", Token::Lend),
        ("빌려", Token::Lend),
        ("ให้ยืม", Token::Lend),
        ("共有", Token::Share),
        ("공유", Token::Share),
        ("แบ่งปัน", Token::Share),
        ("移動", Token::Move),
        ("이동", Token::Move),
        ("ย้าย", Token::Move),
        ("複製", Token::Copy),
        ("복사", Token::Copy),
        ("คัดลอก", Token::Copy),
        // as / where
        ("として", Token::As),
        ("로서", Token::As),
        ("เป็น", Token::As),
        ("但し", Token::Where),
        ("단", Token::Where),
        ("โดยที่", Token::Where),
        // post / give / fit
        ("投稿", Token::Post),
        ("게시", Token::Post),
        ("ส่ง", Token::Post),
        ("渡す", Token::Give),
        ("전달", Token::Give),
        ("ให้", Token::Give),
        ("適合", Token::Fit),
        ("적합", Token::Fit),
        ("เหมาะสม", Token::Fit),
        // can / change
        ("できる", Token::Can),
        ("가능", Token::Can),
        ("สามารถ", Token::Can),
        ("変える", Token::Change),
        ("변경", Token::Change),
        ("เปลี่ยนแปลง", Token::Change),
        // stop / again / try / spawn — only Thai was missing (ja/ko already existed)
        ("หยุด", Token::Stop),
        ("ทำอีก", Token::Again),
        ("ลอง", Token::Try),
        ("สร้าง", Token::Spawn),
        // sure / maybe / pure
        ("確か", Token::Sure),
        ("확실", Token::Sure),
        ("แน่นอน", Token::Sure),
        ("多分", Token::Maybe),
        ("아마도", Token::Maybe),
        ("อาจจะ", Token::Maybe),
        ("純粋", Token::Pure),
        ("순수", Token::Pure),
        ("บริสุทธิ์", Token::Pure),
        // ok / bad / none
        ("良い", Token::Ok),
        ("좋아", Token::Ok),
        ("ตกลง", Token::Ok),
        ("悪い", Token::Bad),
        ("나쁨", Token::Bad),
        ("ผิดพลาด", Token::Bad),
        ("なし", Token::None),
        ("없음", Token::None),
        ("ไม่มี", Token::None),
    ];
    for (word, expected) in cases {
        assert_lexes_as(word, expected.clone());
    }
}

/// Keyword coverage for the RTL languages (Arabic, Persian, Hebrew, Urdu),
/// added as full peers of en/zh/ja/ko/th. Mirrors
/// `keyword_coverage_all_five_languages` above: only a handful of these
/// tokens (own/lend/share/move/copy, type+as) have dedicated grammar
/// productions, so the rest are checked at the lexer level directly.
#[test]
fn keyword_coverage_rtl_languages() {
    let cases: &[(&str, Token)] = &[
        // Arabic
        ("دالة", Token::Fn),
        ("وحدة", Token::Mod),
        ("نوع", Token::Type),
        ("استخدم", Token::Use),
        ("طابق", Token::Match),
        ("امتلك", Token::Own),
        ("أقرض", Token::Lend),
        ("شارك", Token::Share),
        ("انقل", Token::Move),
        ("انسخ", Token::Copy),
        ("غير_متزامن", Token::Async),
        ("انتظر", Token::Wait),
        ("بصفة", Token::As),
        ("حيث", Token::Where),
        ("توقف", Token::Stop),
        ("حاول", Token::Try),
        ("أنشئ", Token::Spawn),
        ("تمام", Token::Ok),
        ("سيء", Token::Bad),
        ("لا_شيء", Token::None),
        // Persian
        ("تابع", Token::Fn),
        ("ماژول", Token::Mod),
        ("استفاده", Token::Use),
        ("اگر", Token::If),
        ("تطبیق", Token::Match),
        ("مالک", Token::Own),
        ("قرض", Token::Lend),
        ("اشتراک", Token::Share),
        ("انتقال", Token::Move),
        ("کپی", Token::Copy),
        ("ناهمگام", Token::Async),
        ("انتظار", Token::Wait),
        ("به_عنوان", Token::As),
        ("تلاش", Token::Try),
        ("ایجاد", Token::Spawn),
        ("تایید", Token::Ok),
        ("هیچ", Token::None),
        ("درست", Token::Bool(true)),
        ("نادرست", Token::Bool(false)),
        // Hebrew
        ("פונקציה", Token::Fn),
        ("מודול", Token::Mod),
        ("סוג", Token::Type),
        ("השתמש", Token::Use),
        ("התאמה", Token::Match),
        ("בעל", Token::Own),
        ("השאלה", Token::Lend),
        ("שתף", Token::Share),
        ("הזז", Token::Move),
        ("העתק", Token::Copy),
        ("אסינכרוני", Token::Async),
        ("המתן", Token::Wait),
        ("בתור", Token::As),
        ("עצור", Token::Stop),
        ("נסה", Token::Try),
        ("צור", Token::Spawn),
        ("בסדר", Token::Ok),
        ("רע", Token::Bad),
        ("כלום", Token::None),
        ("אמת", Token::Bool(true)),
        ("שקר", Token::Bool(false)),
        // Urdu
        ("تفاعل", Token::Fn),
        ("ماڈیول", Token::Mod),
        ("قسم", Token::Type),
        ("استعمال", Token::Use),
        ("مطابقت", Token::Match),
        ("ادھار", Token::Lend),
        ("شراکت", Token::Share),
        ("منتقل", Token::Move),
        ("نقل", Token::Copy),
        ("غیر_ہمزمان", Token::Async),
        ("بطور", Token::As),
        ("رکو", Token::Stop),
        ("کوشش", Token::Try),
        ("پیدا", Token::Spawn),
        ("ٹھیک", Token::Ok),
        ("برا", Token::Bad),
        ("کچھ_نہیں", Token::None),
        ("سچ", Token::Bool(true)),
        ("جھوٹ", Token::Bool(false)),
    ];
    for (word, expected) in cases {
        assert_lexes_as(word, expected.clone());
    }
}

/// Keyword coverage for French and German — full peers of en/zh/ja/ko/th.
/// Mirrors `keyword_coverage_all_five_languages`: only a handful of these
/// tokens have dedicated grammar productions, so the rest are checked at the
/// lexer level directly.
#[test]
fn keyword_coverage_french_german() {
    let cases: &[(&str, Token)] = &[
        // French
        ("fonction", Token::Fn),
        ("module", Token::Mod),
        ("type", Token::Type),
        ("utiliser", Token::Use),
        ("si", Token::If),
        ("correspondre", Token::Match),
        ("posséder", Token::Own),
        ("prêter", Token::Lend),
        ("partager", Token::Share),
        ("déplacer", Token::Move),
        ("copier", Token::Copy),
        ("asynchrone", Token::Async),
        ("attendre", Token::Wait),
        ("comme", Token::As),
        ("où", Token::Where),
        ("arrêter", Token::Stop),
        ("essayer", Token::Try),
        ("engendrer", Token::Spawn),
        ("bon", Token::Ok),
        ("mauvais", Token::Bad),
        ("rien", Token::None),
        ("vrai", Token::Bool(true)),
        ("faux", Token::Bool(false)),
        // German
        ("funktion", Token::Fn),
        ("modul", Token::Mod),
        ("typ", Token::Type),
        ("verwenden", Token::Use),
        ("wenn", Token::If),
        ("abgleichen", Token::Match),
        ("besitzen", Token::Own),
        ("leihen", Token::Lend),
        ("teilen", Token::Share),
        ("bewegen", Token::Move),
        ("kopieren", Token::Copy),
        ("asynchron", Token::Async),
        ("warten", Token::Wait),
        ("als", Token::As),
        ("wobei", Token::Where),
        ("stoppen", Token::Stop),
        ("versuchen", Token::Try),
        ("erzeugen", Token::Spawn),
        ("gut", Token::Ok),
        ("schlecht", Token::Bad),
        ("nichts", Token::None),
        ("wahr", Token::Bool(true)),
        ("falsch", Token::Bool(false)),
    ];
    for (word, expected) in cases {
        assert_lexes_as(word, expected.clone());
    }
}

/// Keyword coverage for Russian — full peer of en/zh/ja/ko/th/fr/de.
#[test]
fn keyword_coverage_russian() {
    let cases: &[(&str, Token)] = &[
        ("функция", Token::Fn),
        ("модуль", Token::Mod),
        ("тип", Token::Type),
        ("использовать", Token::Use),
        ("в", Token::In),
        ("сопоставить", Token::Match),
        ("владеть", Token::Own),
        ("одолжить", Token::Lend),
        ("делиться", Token::Share),
        ("переместить", Token::Move),
        ("копировать", Token::Copy),
        ("асинхронно", Token::Async),
        ("ждать", Token::Wait),
        ("как", Token::As),
        ("где", Token::Where),
        ("стоп", Token::Stop),
        ("пробовать", Token::Try),
        ("создать", Token::Spawn),
        ("хорошо", Token::Ok),
        ("плохо", Token::Bad),
        ("ничего", Token::None),
        ("истина", Token::Bool(true)),
        ("ложь", Token::Bool(false)),
    ];
    for (word, expected) in cases {
        assert_lexes_as(word, expected.clone());
    }
}

/// Ownership-hint keywords (own/lend/share/move/copy) are the one part of this
/// batch that *does* have a dedicated grammar rule (`parse_unary_expr`
/// evaluates straight through them) — so unlike the rest of
/// `keyword_coverage_all_five_languages`, this can be proven end-to-end: if
/// the word didn't lex as the intended keyword, the leftover number token
/// would break parsing inside the `print(...)` call.
#[test]
fn ownership_hints_japanese_korean_thai() {
    assert_runs(
        "own-ja",
        r#"束縛 開始 = 実行 { 印刷(所有 1) 印刷(貸す 2) 印刷(共有 3) 印刷(移動 4) 印刷(複製 5) }"#,
    );
    assert_runs(
        "own-ko",
        r#"바인드 시작 = 실행 { 출력(소유 1) 출력(빌려 2) 출력(공유 3) 출력(이동 4) 출력(복사 5) }"#,
    );
    assert_runs(
        "own-th",
        r#"ผูก เริ่ม = ทำ { พิมพ์(เป็นเจ้าของ 1) พิมพ์(ให้ยืม 2) พิมพ์(แบ่งปัน 3) พิมพ์(ย้าย 4) พิมพ์(คัดลอก 5) }"#,
    );
    assert_runs(
        "own-fr",
        r#"lier début = faire { afficher(posséder 1) afficher(prêter 2) afficher(partager 3) afficher(déplacer 4) afficher(copier 5) }"#,
    );
    assert_runs(
        "own-de",
        r#"binden anfang = machen { drucken(besitzen 1) drucken(leihen 2) drucken(teilen 3) drucken(bewegen 4) drucken(kopieren 5) }"#,
    );
    assert_runs(
        "own-ru",
        r#"связать начать = сделать { печать(владеть 1) печать(одолжить 2) печать(делиться 3) печать(переместить 4) печать(копировать 5) }"#,
    );
}

/// `type X as Y` exercises both the newly-added `type` and `as` keywords
/// together, in the one grammar rule that actually consumes them
/// (`parse_item`'s `Token::Type` branch expects `Token::As` right after the
/// name). Checked via `parser::parse` rather than `assert_runs`/a second
/// top-level item: `parse_type_str` only stops at `{`/`,`/`;`/`->`/EOF, so a
/// standalone type alias followed by *anything* else greedily swallows it —
/// a pre-existing limitation independent of language (confirmed: plain
/// English `type Foo as num` followed by `bind start = do {...}` hits the
/// exact same "unexpected token at top level: LBrace"), not something to
/// paper over in a keyword-coverage test.
#[test]
fn type_alias_japanese_korean_thai() {
    for (label, src) in [
        ("type-ja", "型 数 として num"),
        ("type-ko", "타입 숫자 로서 num"),
        ("type-th", "ชนิด เลข เป็น num"),
    ] {
        ling::parser::parse(src)
            .unwrap_or_else(|e| panic!("[{label}] expected `type ... as ...` to parse, got: {e}"));
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
