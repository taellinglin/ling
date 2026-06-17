// src/diag.rs — colored, Rust-style error reporting with localized labels.
//
// Output language defaults to English and can be overridden with the `LING_LANG`
// environment variable (en | th | ko | ja | zh). Colors auto-disable when stderr
// is not a terminal or when `NO_COLOR` is set; `CLICOLOR_FORCE` forces them on.
//
// Default palette: navy blue, teal, rose red, grey, vine green.

use std::io::IsTerminal;

// ─── Palette (truecolor RGB) ───────────────────────────────────────────────────

pub const NAVY: (u8, u8, u8) = (59, 110, 165); // #3B6EA5 — notes / secondary
pub const TEAL: (u8, u8, u8) = (42, 157, 143); // #2A9D8F — frames / structure
pub const ROSE: (u8, u8, u8) = (232, 74, 111); // #E84A6F — errors
pub const GREY: (u8, u8, u8) = (141, 153, 174); // #8D99AE — locations / dim
pub const VINE: (u8, u8, u8) = (127, 176, 105); // #7FB069 — hints / success

// ─── Output language ────────────────────────────────────────────────────────────

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum OutputLang {
    English,
    Thai,
    Korean,
    Japanese,
    Chinese,
}

impl OutputLang {
    /// Resolve from the `LING_LANG` env var; defaults to English.
    pub fn from_env() -> Self {
        match std::env::var("LING_LANG")
            .unwrap_or_default()
            .trim()
            .to_lowercase()
            .as_str()
        {
            "th" | "thai" | "ภาษาไทย" => OutputLang::Thai,
            "ko" | "korean" | "한국어" => OutputLang::Korean,
            "ja" | "japanese" | "日本語" => OutputLang::Japanese,
            "zh" | "chinese" | "中文" => OutputLang::Chinese,
            _ => OutputLang::English,
        }
    }
}

/// Localized label lookup. `key` is one of the keys handled below.
fn t(lang: OutputLang, key: &str) -> &'static str {
    use OutputLang::*;
    match (key, lang) {
        ("error", English) => "error",
        ("error", Thai) => "ข้อผิดพลาด",
        ("error", Korean) => "오류",
        ("error", Japanese) => "エラー",
        ("error", Chinese) => "错误",

        ("parse", English) => "parse",
        ("parse", Thai) => "แยกวิเคราะห์",
        ("parse", Korean) => "구문",
        ("parse", Japanese) => "構文",
        ("parse", Chinese) => "解析",

        ("runtime", English) => "runtime",
        ("runtime", Thai) => "ขณะทำงาน",
        ("runtime", Korean) => "런타임",
        ("runtime", Japanese) => "実行時",
        ("runtime", Chinese) => "运行时",

        ("traceback", English) => "traceback (deepest call last)",
        ("traceback", Thai) => "การย้อนรอย (เรียกล่าสุดอยู่ท้าย)",
        ("traceback", Korean) => "역추적 (최근 호출이 마지막)",
        ("traceback", Japanese) => "トレースバック (最新の呼び出しが最後)",
        ("traceback", Chinese) => "回溯（最近的调用在最后）",

        ("in", English) => "in",
        ("in", Thai) => "ใน",
        ("in", Korean) => "위치",
        ("in", Japanese) => "内",
        ("in", Chinese) => "于",

        ("hint", English) => "hint",
        ("hint", Thai) => "คำแนะนำ",
        ("hint", Korean) => "힌트",
        ("hint", Japanese) => "ヒント",
        ("hint", Chinese) => "提示",

        // Fallback to English for any unmapped key.
        (_, _) => "error",
    }
}

// ─── Color gating ────────────────────────────────────────────────────────────────

fn colors_enabled() -> bool {
    if std::env::var_os("NO_COLOR").is_some() {
        return false;
    }
    if std::env::var_os("CLICOLOR_FORCE").is_some() {
        enable_windows_vt();
        return true;
    }
    let on = std::io::stderr().is_terminal();
    if on {
        enable_windows_vt();
    }
    on
}

/// On Windows, enable ANSI escape (virtual terminal) processing on the legacy
/// console. No-op on terminals that already support it (Windows Terminal, VSCode).
#[cfg(windows)]
fn enable_windows_vt() {
    use std::sync::Once;
    static ONCE: Once = Once::new();
    ONCE.call_once(|| unsafe {
        extern "system" {
            fn GetStdHandle(n: u32) -> *mut core::ffi::c_void;
            fn GetConsoleMode(h: *mut core::ffi::c_void, m: *mut u32) -> i32;
            fn SetConsoleMode(h: *mut core::ffi::c_void, m: u32) -> i32;
        }
        const STD_ERROR_HANDLE: u32 = -12i32 as u32;
        const ENABLE_VIRTUAL_TERMINAL_PROCESSING: u32 = 0x0004;
        let h = GetStdHandle(STD_ERROR_HANDLE);
        let mut mode = 0u32;
        if GetConsoleMode(h, &mut mode) != 0 {
            let _ = SetConsoleMode(h, mode | ENABLE_VIRTUAL_TERMINAL_PROCESSING);
        }
    });
}

#[cfg(not(windows))]
fn enable_windows_vt() {}

// ─── Painter ─────────────────────────────────────────────────────────────────────

struct Paint {
    on: bool,
}

impl Paint {
    fn new() -> Self {
        Paint { on: colors_enabled() }
    }

    fn color(&self, rgb: (u8, u8, u8), bold: bool, s: &str) -> String {
        if !self.on {
            return s.to_string();
        }
        let (r, g, bl) = rgb;
        let bold_seq = if bold { "\x1b[1m" } else { "" };
        format!("\x1b[38;2;{};{};{}m{}{}\x1b[0m", r, g, bl, bold_seq, s)
    }
}

// ─── Rendering ─────────────────────────────────────────────────────────────────

fn header(p: &Paint, lang: OutputLang, kind_key: &str, message: &str) -> String {
    let error_word = t(lang, "error");
    let kind = t(lang, kind_key);
    format!(
        "{}{}{} {}",
        p.color(ROSE, true, error_word),
        p.color(GREY, false, &format!("[{kind}]")),
        p.color(ROSE, true, ":"),
        p.color((230, 230, 235), true, message),
    )
}

fn location(p: &Paint, file: Option<&str>) -> String {
    match file {
        Some(f) => format!("\n {}", p.color(GREY, false, &format!("--> {f}"))),
        None => String::new(),
    }
}

/// Render a runtime error with an optional call-stack traceback.
pub fn render_runtime(
    message: &str,
    _source: &str,
    file: Option<&str>,
    trace: &[String],
    lang: OutputLang,
) -> String {
    let p = Paint::new();
    let localized = localize_message(message, lang);
    let mut out = header(&p, lang, "runtime", &localized);
    out.push_str(&location(&p, file));

    if !trace.is_empty() {
        out.push('\n');
        out.push_str(&format!("  {}", p.color(TEAL, true, t(lang, "traceback"))));
        out.push(':');
        for (i, frame) in trace.iter().enumerate() {
            out.push('\n');
            out.push_str(&format!(
                "    {} {}",
                p.color(GREY, false, &format!("{i}:")),
                p.color(TEAL, false, frame),
            ));
        }
    }

    if let Some(hint) = hint_for(message, lang) {
        out.push('\n');
        out.push_str(&format!(
            "  {}{} {}",
            p.color(VINE, true, t(lang, "hint")),
            p.color(VINE, true, ":"),
            p.color(GREY, false, &hint),
        ));
    }
    out
}

/// Render a parse error.
pub fn render_parse(message: &str, _source: &str, file: Option<&str>, lang: OutputLang) -> String {
    let p = Paint::new();
    let mut out = header(&p, lang, "parse", message);
    out.push_str(&location(&p, file));
    out
}

/// Localized, best-effort hint for common runtime errors.
fn hint_for(message: &str, lang: OutputLang) -> Option<String> {
    use OutputLang::*;
    if message.contains("unknown function") || message.contains("undefined") {
        Some(
            match lang {
                English => "check the spelling, or `use` the module that defines it",
                Thai => "ตรวจการสะกด หรือ `use` โมดูลที่กำหนดมัน",
                Korean => "철자를 확인하거나, 정의한 모듈을 `use` 하세요",
                Japanese => "綴りを確認するか、定義しているモジュールを `use` してください",
                Chinese => "检查拼写，或 `use` 定义它的模块",
            }
            .to_string(),
        )
    } else if message.contains("no entry point") {
        Some(
            match lang {
                English => "add `bind start = do { ... }`",
                Thai => "เพิ่ม `bind start = do { ... }`",
                Korean => "`bind start = do { ... }` 를 추가하세요",
                Japanese => "`bind start = do { ... }` を追加してください",
                Chinese => "添加 `bind start = do { ... }`",
            }
            .to_string(),
        )
    } else {
        None
    }
}

/// Translate the *body* of the most common runtime errors, preserving any
/// dynamic suffix (e.g. the quoted identifier). English passes through. This
/// lets diagnostics read fully in the chosen language without touching the
/// thousands of `format!`-built messages in the runtime.
fn localize_message(msg: &str, lang: OutputLang) -> String {
    use OutputLang::*;
    if lang == English {
        return msg.to_string();
    }

    // (english_prefix, [th, ko, ja, zh])
    let prefixes: &[(&str, [&str; 4])] = &[
        (
            "unknown function ",
            [
                "ฟังก์ชันที่ไม่รู้จัก ",
                "알 수 없는 함수 ",
                "不明な関数 ",
                "未知函数 ",
            ],
        ),
        (
            "undefined: ",
            ["ไม่ได้กำหนด: ", "정의되지 않음: ", "未定義: ", "未定义： "],
        ),
        (
            "cannot call ",
            [
                "เรียกใช้ไม่ได้ ",
                "호출할 수 없음 ",
                "呼び出せません ",
                "无法调用 ",
            ],
        ),
        (
            "division by zero",
            ["หารด้วยศูนย์", "0으로 나눔", "ゼロ除算", "除以零"],
        ),
        (
            "index out of",
            [
                "ดัชนีเกินขอบเขต",
                "인덱스 범위 초과",
                "範囲外インデックス",
                "索引越界",
            ],
        ),
    ];
    let idx = match lang {
        Thai => 0,
        Korean => 1,
        Japanese => 2,
        Chinese => 3,
        English => return msg.to_string(),
    };
    for (en, tr) in prefixes {
        if let Some(rest) = msg.strip_prefix(en) {
            return format!("{}{}", tr[idx], rest);
        }
    }
    // Whole-message special cases.
    if msg.starts_with("no entry point") {
        return match lang {
            Thai => "ไม่มีจุดเริ่มต้น — ต้องมี `bind เริ่ม = ทำ {...}`",
            Korean => "진입점 없음 — `bind 시작 = do {...}` 가 필요합니다",
            Japanese => "エントリポイントがありません — `bind 始め = do {...}` が必要です",
            Chinese => "没有入口点 — 需要 `bind 始 = do {...}`",
            English => msg,
        }
        .to_string();
    }
    msg.to_string()
}
