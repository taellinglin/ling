use phf::phf_map;

/// Maps polyglot keywords to their canonical English equivalents.
/// Duplicate Unicode code points are deduplicated (e.g. CJK shared chars).
static KEYWORD_MAP: phf::Map<&'static str, &'static str> = phf_map! {
    // Chinese (Simplified)
    "令" => "bind",
    "启动" => "start",
    "执行" => "do",
    "印" => "print",
    "如果" => "if",
    "否则" => "else",
    "当" => "while",
    "对于" => "for",
    "停止" => "stop",
    "继续" => "again",
    "尝试" => "try",
    "确定" => "sure",
    "可能" => "maybe",
    "结果" => "result",
    "好" => "ok",
    "坏" => "bad",
    "无" => "none",
    "纯" => "pure",
    "异步" => "async",
    "等待" => "wait",
    "生成" => "spawn",
    "借" => "lend",
    "共享" => "share",
    "拥有" => "own",
    "改变" => "change",
    "移动" => "move",
    "复制" => "copy",
    "选择" => "choose",
    "能" => "can",
    "形式" => "form",
    "适合" => "fit",
    "给" => "give",
    "发布" => "post",
    // Japanese
    "束縛" => "bind",
    "開始" => "start",
    "実行" => "do",
    "印刷" => "print",
    "もし" => "if",
    "一方" => "while",
    "ために" => "for",
    "続く" => "again",
    "試す" => "try",
    "待つ" => "wait",
    "生む" => "spawn",
    // Korean
    "바인드" => "bind",
    "시작" => "start",
    "실행" => "do",
    "인쇄" => "print",
    "만약" => "if",
    "동안" => "while",
    // Russian
    "связать" => "bind",
    "начать" => "start",
    "сделать" => "do",
    "печать" => "print",
    "если" => "if",
    "иначе" => "else",
    "пока" => "while",
    "для" => "for",
    "стоп" => "stop",
    "ждать" => "wait",
    // Spanish
    "atar" => "bind",
    "inicio" => "start",
    "hacer" => "do",
    "imprimir" => "print",
    "mientras" => "while",
    "para" => "for",
    // French
    "lier" => "bind",
    "début" => "start",
    "faire" => "do",
    "imprimer" => "print",
    "tant_que" => "while",
    "pour" => "for",
    "sinon" => "else",
    // German
    "binden" => "bind",
    "anfang" => "start",
    "machen" => "do",
    "drucken" => "print",
    "wenn" => "if",
    "sonst" => "else",
    "während" => "while",
    // Arabic
    "ربط" => "bind",
    "بداية" => "start",
    "افعل" => "do",
    "اطبع" => "print",
    // Hindi
    "बांधना" => "bind",
    "शुरू" => "start",
    "करना" => "do",
    "प्रिंट" => "print",
    // Thai
    "ผูก" => "bind",
    "เริ่ม" => "start",
    "ทำ" => "do",
    "พิมพ์" => "print",
    // Greek
    "δέσιμο" => "bind",
    "αρχή" => "start",
    "κάνω" => "do",
    "εκτύπωση" => "print",
    // Hebrew
    "קשור" => "bind",
    "התחלה" => "start",
    "עשה" => "do",
    "הדפס" => "print",
    // Vietnamese
    "ràng_buộc" => "bind",
    "bắt_đầu" => "start",
    "làm" => "do",
    // Turkish
    "bağla" => "bind",
    "başlangıç" => "start",
    "yap" => "do",
    "yazdır" => "print",
    // Polish
    "powiąż" => "bind",
    "początek" => "start",
    "zrób" => "do",
    "drukuj" => "print",
};

/// Normalize polyglot source by replacing non-English keywords with their
/// canonical English equivalents before tokenization.
/// Note: replacement is global and does not skip string literals (MVP).
pub fn normalize(source: &str) -> String {
    let mut result = source.to_string();
    for (foreign, canonical) in KEYWORD_MAP.entries() {
        result = result.replace(foreign, canonical);
    }
    result
}

/// Return the canonical English form of a single keyword, if known.
pub fn canonical(keyword: &str) -> Option<&'static str> {
    KEYWORD_MAP.get(keyword).copied()
}
