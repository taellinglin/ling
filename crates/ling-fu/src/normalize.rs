// crates/ling-fu/src/normalize.rs
//
// `lingfu normalize <lang>` — translate all keywords, builtin names,
// and file/folder names in a Ling project to a single target language.
//
// Usage:
//   lingfu normalize thai          # normalize everything to Thai
//   lingfu normalize zh            # Chinese
//   lingfu normalize --dry-run en  # preview English normalization
//   lingfu normalize ja --content-only  # only rewrite file contents, no renames
//   lingfu normalize ko --files-only    # only rename files/folders

use colored::*;
use std::path::{Path, PathBuf};
use std::{fs, io};

// ─── Target language ──────────────────────────────────────────────────────────

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Lang { En, Zh, Ja, Ko, Th }

impl Lang {
    pub fn from_str(s: &str) -> Option<Self> {
        match s.to_lowercase().as_str() {
            "en" | "english" | "英语" | "英語" | "영어" | "อังกฤษ" => Some(Lang::En),
            "zh" | "chinese" | "中文" | "汉语" | "中国語" | "중국어" | "จีน" => Some(Lang::Zh),
            "ja" | "japanese" | "日本語" | "日语" | "일본어" | "ญี่ปุ่น" => Some(Lang::Ja),
            "ko" | "korean" | "한국어" | "韩语" | "韓国語" | "เกาหลี" => Some(Lang::Ko),
            "th" | "thai" | "ภาษาไทย" | "ไทย" | "泰语" | "タイ語" | "태국어" => Some(Lang::Th),
            _ => None,
        }
    }

    pub fn name(self) -> &'static str {
        match self { Lang::En => "English", Lang::Zh => "Chinese", Lang::Ja => "Japanese", Lang::Ko => "Korean", Lang::Th => "Thai" }
    }

    fn idx(self) -> usize {
        match self { Lang::En => 0, Lang::Zh => 1, Lang::Ja => 2, Lang::Ko => 3, Lang::Th => 4 }
    }
}

// ─── Translation table ────────────────────────────────────────────────────────
//
// Each entry: (aliases_in_all_langs, [en, zh, ja, ko, th])
// The canonical form index matches Lang::idx().

type Entry = (&'static [&'static str], [&'static str; 5]);

// Keywords
static KEYWORDS: &[Entry] = &[
    (&["fn","函","関数","関","함수","ฟังก์ชัน"],        ["fn","函","関数","함수","ฟังก์ชัน"]),
    (&["bind","令","束縛","バ","바인드","묶","ผูก"],     ["let","令","束縛","바인드","ผูก"]),
    (&["let"],                                          ["let","令","束縛","바인드","ผูก"]),
    (&["do","执","実行","執","실행","ทำ"],               ["do","执","実行","실행","ทำ"]),
    (&["mod","核","モジュール","模","모듈","โมดูล"],     ["mod","核","モジュール","모듈","โมดูล"]),
    (&["use","载","引","사용","使う","ใช้","นำเข้า"],   ["use","载","使う","사용","ใช้"]),
    (&["if","若","如","もし","만약","조건","ถ้า"],       ["if","若","もし","만약","ถ้า"]),
    (&["else","否则","否","他","아니면","มิฉะนั้น"],     ["else","否则","他","아니면","มิฉะนั้น"]),
    (&["while","循","当","間","一方","동안","반복","ขณะที่"],["while","循","間","동안","ขณะที่"]),
    (&["for","历","繰","ために","위해","สำหรับ"],        ["for","历","繰","위해","สำหรับ"]),
    (&["in","于","の中","안에","ใน"],                   ["in","于","の中","안에","ใน"]),
    (&["match","配","一致","매치","จับคู่"],            ["match","配","一致","매치","จับคู่"]),
    (&["return","归","戻る","帰る","반환","귀환","คืน"], ["return","归","戻る","반환","คืน"]),
    (&["async","异步","异","非同期","비동기","ไม่พร้อมกัน"],["async","异步","非同期","비동기","ไม่พร้อมกัน"]),
    (&["wait","等待","待","待つ","기다려","รอ"],         ["wait","等待","待つ","기다려","รอ"]),
    (&["try","尝试","试","試す","시도"],                 ["try","尝试","試す","시도","ลอง"]),
    (&["stop","停止","止","止まれ","停め","멈춤","หยุด"],["stop","停止","止まれ","멈춤","หยุด"]),
    (&["again","继续","継続","계속","อีกครั้ง"],         ["again","继续","継続","계속","อีกครั้ง"]),
    (&["spawn","生成","启","起動","생성","สร้าง"],       ["spawn","生成","起動","생성","สร้าง"]),
    (&["own","拥有","独","所有","소유","เป็นเจ้าของ"],   ["own","拥有","所有","소유","เป็นเจ้าของ"]),
    (&["lend","借","借りる","빌려","ยืม"],               ["lend","借","借りる","빌려","ยืม"]),
    (&["share","共享","共","共有","공유","แชร์"],         ["share","共享","共有","공유","แชร์"]),
    (&["move","移动","移","移動","이동","ย้าย"],         ["move","移动","移動","이동","ย้าย"]),
    (&["copy","复制","复","コピー","복사","คัดลอก"],     ["copy","复制","コピー","복사","คัดลอก"]),
    (&["as","为","として","로서","เป็น"],                ["as","为","として","로서","เป็น"]),
    (&["pure","纯","純粋","순수","บริสุทธิ์"],           ["pure","纯","純粋","순수","บริสุทธิ์"]),
    (&["ok","好","可","良し","좋아","โอเค"],             ["ok","好","良し","좋아","โอเค"]),
    (&["bad","坏","误","悪い","나쁨","ผิด"],             ["bad","坏","悪い","나쁨","ผิด"]),
    (&["none","无","なし","없음","ไม่มี"],               ["none","无","なし","없음","ไม่มี"]),
    (&["true","真","참","จริง"],                        ["true","真","真","참","จริง"]),
    (&["false","假","偽","거짓","เท็จ"],                 ["false","假","偽","거짓","เท็จ"]),
];

// Builtins — vtex_* geometry functions
static BUILTINS_VTEX: &[Entry] = &[
    (&["vtex_grid","ลายตาราง","纹格","格子模様","격자무늬"],         ["vtex_grid","纹格","格子模様","격자무늬","ลายตาราง"]),
    (&["vtex_rings","ลายวงแหวน","纹环","リング模様","링무늬"],        ["vtex_rings","纹环","リング模様","링무늬","ลายวงแหวน"]),
    (&["vtex_spiral","ลายก้นหอย","纹螺","螺旋模様","나선무늬"],       ["vtex_spiral","纹螺","螺旋模様","나선무늬","ลายก้นหอย"]),
    (&["vtex_star","ลายดาว","纹星","星模様","별무늬"],               ["vtex_star","纹星","星模様","별무늬","ลายดาว"]),
    (&["vtex_flower","ลายดอกไม้","纹花","花模様","꽃무늬"],          ["vtex_flower","纹花","花模様","꽃무늬","ลายดอกไม้"]),
    (&["vtex_lotus","ลายดอกบัว","纹莲","蓮模様","연꽃무늬"],         ["vtex_lotus","纹莲","蓮模様","연꽃무늬","ลายดอกบัว"]),
    (&["vtex_chakra","ลายจักร","纹轮","輪模様","바퀴무늬"],          ["vtex_chakra","纹轮","輪模様","바퀴무늬","ลายจักร"]),
    (&["vtex_yantra","ลายยันต์","纹扬特拉","ヤントラ模様","얀트라무늬"],["vtex_yantra","纹扬特拉","ヤントラ模様","얀트라무늬","ลายยันต์"]),
    (&["vtex_hyper","ลายไฮเปอร์","纹超","超次元模様","초차원무늬"],  ["vtex_hyper","纹超","超次元模様","초차원무늬","ลายไฮเปอร์"]),
    (&["vtex_tess","ลายเทสเซล","纹镶","テッセレーション","테셀무늬"], ["vtex_tess","纹镶","テッセレーション","테셀무늬","ลายเทสเซล"]),
    (&["vtex_rain","ลายฝน","纹雨","雨模様","비무늬"],               ["vtex_rain","纹雨","雨模様","비무늬","ลายฝน"]),
    (&["vtex_halftone","ลายจุด","纹半调","ハーフトーン","하프톤무늬"],["vtex_halftone","纹半调","ハーフトーン","하프톤무늬","ลายจุด"]),
    (&["vtex_spiked_cog","ลายเฟืองหนาม","纹刺轮","スパイクコグ","스파이크톱니무늬","ฟันเฟืองหนาม"],
                                                                    ["vtex_spiked_cog","纹刺轮","スパイクコグ","스파이크톱니무늬","ฟันเฟืองหนาม"]),
    (&["vtex_torii","ลายโทริอิ","纹鸟居","鳥居","도리이","ประตูโทริอิ"],["vtex_torii","纹鸟居","鳥居","도리이","ประตูโทริอิ"]),
    (&["vtex_pagoda","ลายเจดีย์","纹塔","塔","탑","เจดีย์"],       ["vtex_pagoda","纹塔","塔","탑","เจดีย์"]),
];

// Builtins — draw / camera / audio / misc
static BUILTINS_OTHER: &[Entry] = &[
    (&["present","呈现","表示","표시","แสดง"],           ["present","呈现","表示","표시","แสดง"]),
    (&["set_camera","设置摄像机","カメラ設定","카메라설정","ตั้งกล้อง"],["set_camera","设置摄像机","カメラ設定","카메라설정","ตั้งกล้อง"]),
    (&["set_camera_pos","摄像机位置","カメラ位置","카메라위치","ตำแหน่งกล้อง"],["set_camera_pos","摄像机位置","カメラ位置","카메라위치","ตำแหน่งกล้อง"]),
    (&["set_zdist","设置深度","奥行き設定","깊이설정","ตั้งความลึก"],["set_zdist","设置深度","奥行き設定","깊이설정","ตั้งความลึก"]),
    (&["set_projection","设置投影","投影設定","투영설정","ตั้งการฉาย"],["set_projection","设置投影","投影設定","투영설정","ตั้งการฉาย"]),
    (&["set_ambient","设置环境光","環境光設定","환경광설정","ตั้งแสงรอบข้าง"],["set_ambient","设置环境光","環境光設定","환경광설정","ตั้งแสงรอบข้าง"]),
    (&["add_light","添加灯光","ライト追加","조명추가","เพิ่มแสง"],   ["add_light","添加灯光","ライト追加","조명추가","เพิ่มแสง"]),
    (&["clear_lights","清除灯光","ライトクリア","조명초기화","ลบแสง"],["clear_lights","清除灯光","ライトクリア","조명초기화","ลบแสง"]),
    (&["draw_triangle_3d","绘制三角形","三角形描画","삼각형그리기","วาดสามเหลี่ยม"],["draw_triangle_3d","绘制三角形","三角形描画","삼각형그리기","วาดสามเหลี่ยม"]),
    (&["draw_line_3d","绘制线条","線描画","선그리기","วาดเส้น"],     ["draw_line_3d","绘制线条","線描画","선그리기","วาดเส้น"]),
    (&["audio_tone","音调","音調","음조","เสียง"],                   ["audio_tone","音调","音調","음조","เสียง"]),
    (&["audio_volume","音量","音量","음량","ระดับเสียง"],            ["audio_volume","音量","音量","음량","ระดับเสียง"]),
    (&["audio_listener","音频监听","音声リスナー","오디오리스너","ตัวฟังเสียง"],["audio_listener","音频监听","音声リスナー","오디오리스너","ตัวฟังเสียง"]),
    (&["audio_bgm","背景乐","BGM","배경음악","เพลงประกอบ"],          ["audio_bgm","背景乐","BGM","배경음악","เพลงประกอบ"]),
    (&["audio_bgm_volume","背景乐音量","BGM音量","배경음악음량","ระดับเพลงประกอบ"],["audio_bgm_volume","背景乐音量","BGM音量","배경음악음량","ระดับเพลงประกอบ"]),
    (&["hsl_color","色相色","HSL色","HSL색","สีHSL"],               ["hsl_color","色相色","HSL色","HSL색","สีHSL"]),
    (&["print","打印","印刷","출력","พิมพ์"],                        ["print","打印","印刷","출력","พิมพ์"]),
    (&["sqrt","平方根","平方根","제곱근","รากที่สอง"],                ["sqrt","平方根","平方根","제곱근","รากที่สอง"]),
    (&["abs","绝对值","絶対値","절댓값","ค่าสัมบูรณ์"],              ["abs","绝对值","絶対値","절댓값","ค่าสัมบูรณ์"]),
    (&["sin","正弦","サイン","사인","ไซน์"],                        ["sin","正弦","サイン","사인","ไซน์"]),
    (&["cos","余弦","コサイン","코사인","โคไซน์"],                  ["cos","余弦","コサイン","코사인","โคไซน์"]),
    (&["int","取整","整数","정수","ตัดทศนิยม"],                     ["int","取整","整数","정수","ตัดทศนิยม"]),
    (&["trunc","截整","切り捨て","버림","ตัดทศนิยม"],               ["trunc","截整","切り捨て","버림","ตัดทศนิยม"]),
    (&["len","长度","長さ","길이","ความยาว"],                        ["len","长度","長さ","길이","ความยาว"]),
    (&["push","添加","追加","추가","เพิ่ม"],                         ["push","添加","追加","추가","เพิ่ม"]),
    (&["pop","删除","削除","제거","ลบ"],                             ["pop","删除","削除","제거","ลบ"]),
    (&["keys","键","キー","키","คีย์"],                              ["keys","键","キー","키","คีย์"]),
    (&["values","值","値","값","ค่า"],                               ["values","值","値","값","ค่า"]),
    (&["floor","向下取整","床関数","내림","ปัดลง"],                  ["floor","向下取整","床関数","내림","ปัดลง"]),
    (&["ceil","向上取整","天井関数","올림","ปัดขึ้น"],               ["ceil","向上取整","天井関数","올림","ปัดขึ้น"]),
    (&["rand","随机","乱数","무작위","สุ่ม"],                        ["rand","随机","乱数","무작위","สุ่ม"]),
    (&["now","当前时间","現在時刻","현재시간","เวลาปัจจุบัน"],      ["now","当前时间","現在時刻","현재시간","เวลาปัจจุบัน"]),
];

// Folder name translations: [en, zh, ja, ko, th]
static FOLDER_NAMES: &[([&str; 5], &[&str])] = &[
    (["src","源码","ソース","소스","ต้นฉบับ"],     &["src","源码","ソース","소스","ต้นฉบับ","source","sources"]),
    (["lib","库","ライブラリ","라이브러리","ไลบรารี"],&["lib","库","ライブラリ","라이브러리","ไลบรารี","library"]),
    (["tests","测试","テスト","테스트","ทดสอบ"],   &["tests","test","测试","テスト","테스트","ทดสอบ"]),
    (["examples","示例","サンプル","예제","ตัวอย่าง"],&["examples","example","示例","サンプル","예제","ตัวอย่าง"]),
    (["docs","文档","ドキュメント","문서","เอกสาร"], &["docs","doc","文档","ドキュメント","문서","เอกสาร"]),
    (["assets","资源","アセット","에셋","ทรัพยากร"], &["assets","asset","资源","アセット","에셋","ทรัพยากร"]),
    (["scenes","场景","シーン","장면","ฉาก"],       &["scenes","scene","场景","シーン","장면","ฉาก"]),
    (["rooms","房间","部屋","방","ห้อง"],           &["rooms","room","房间","部屋","방","ห้อง"]),
    (["garden","花园","庭園","정원","สวน"],         &["garden","花园","庭園","정원","สวน"]),
    (["gallery","画廊","ギャラリー","갤러리","แกลเลอรี"],&["gallery","画廊","ギャラリー","갤러리","แกลเลอรี"]),
];

// ─── Content normalization ────────────────────────────────────────────────────

/// Collect all entries into a sorted replacement map: alias → target_form.
/// Sorted longest-first to prevent shorter aliases shadowing longer ones.
fn build_replacement_map(target: Lang) -> Vec<(String, String)> {
    let idx = target.idx();
    let mut map: Vec<(String, String)> = Vec::new();

    for entries in [KEYWORDS, BUILTINS_VTEX, BUILTINS_OTHER] {
        for (aliases, forms) in entries.iter() {
            let target_form = forms[idx];
            for alias in *aliases {
                if *alias != target_form {
                    map.push((alias.to_string(), target_form.to_string()));
                }
            }
        }
    }

    // Longest alias first — prevents "fn" matching inside "ฟังก์ชัน"
    map.sort_by(|a, b| b.0.len().cmp(&a.0.len()));
    map.dedup_by(|a, b| a.0 == b.0);
    map
}

/// True if character is a word constituent (can appear inside an identifier).
fn is_word_char(c: char) -> bool {
    c.is_alphanumeric() || c == '_'
}

/// Normalize the content of a single .ling source file.
pub fn normalize_content(source: &str, target: Lang) -> String {
    let replacements = build_replacement_map(target);
    let chars: Vec<char> = source.chars().collect();
    let mut out = String::with_capacity(source.len());
    let mut i = 0;
    let n = chars.len();

    while i < n {
        // Skip string literals verbatim
        if chars[i] == '"' {
            out.push(chars[i]);
            i += 1;
            while i < n {
                if chars[i] == '\\' && i + 1 < n {
                    out.push(chars[i]); out.push(chars[i+1]); i += 2;
                } else if chars[i] == '"' {
                    out.push(chars[i]); i += 1; break;
                } else {
                    out.push(chars[i]); i += 1;
                }
            }
            continue;
        }
        // Skip line comments verbatim
        if chars[i] == '/' && i + 1 < n && chars[i+1] == '/' {
            while i < n && chars[i] != '\n' { out.push(chars[i]); i += 1; }
            continue;
        }
        // Skip # comments verbatim
        if chars[i] == '#' {
            while i < n && chars[i] != '\n' { out.push(chars[i]); i += 1; }
            continue;
        }

        // Try to match a word starting at i
        if is_word_char(chars[i]) {
            // Collect the full word
            let word_start = i;
            while i < n && is_word_char(chars[i]) { i += 1; }
            let word: String = chars[word_start..i].iter().collect();

            // Check left boundary (char before word_start must be non-word or SOF)
            let left_ok = word_start == 0 || !is_word_char(chars[word_start - 1]);
            // Right boundary already guaranteed (chars[i] is non-word or EOF)
            let right_ok = i == n || !is_word_char(chars[i]);

            if left_ok && right_ok {
                // Try to match any replacement (longest-first already sorted)
                let mut replaced = false;
                for (alias, target_form) in &replacements {
                    if word == *alias {
                        out.push_str(target_form);
                        replaced = true;
                        break;
                    }
                }
                if !replaced { out.push_str(&word); }
            } else {
                out.push_str(&word);
            }
            continue;
        }

        out.push(chars[i]);
        i += 1;
    }

    out
}

// ─── File / folder renaming ───────────────────────────────────────────────────

fn translate_folder_name(name: &str, target: Lang) -> Option<String> {
    let idx = target.idx();
    for (forms, aliases) in FOLDER_NAMES {
        if aliases.contains(&name) {
            let form = forms[idx];
            if form != name { return Some(form.to_string()); }
        }
    }
    None
}

/// Translate a .ling filename stem to target lang.
/// e.g. "main" → "หลัก" (Thai), "garden" → "庭園" (Japanese)
fn translate_file_stem(stem: &str, target: Lang) -> Option<String> {
    // Filename stems reuse the folder table (most overlap)
    let idx = target.idx();
    // Extra per-file stems
    static STEMS: &[([&str; 5], &[&str])] = &[
        (["main","主","メイン","메인","หลัก"],      &["main","主","メイン","메인","หลัก"]),
        (["hello","你好","こんにちは","안녕","สวัสดี"],&["hello","you_good","你好","こんにちは","안녕","สวัสดี"]),
        (["room","房间","部屋","방","ห้อง"],         &["room","房间","部屋","방","ห้อง"]),
        (["garden","花园","庭園","정원","สวน"],      &["garden","花园","庭園","정원","สวน"]),
        (["lounge","休息室","ラウンジ","라운지","ห้องพัก"],&["lounge","休息室","ラウンジ","라운지","ห้องพัก"]),
        (["scene","场景","シーン","장면","ฉาก"],     &["scene","场景","シーン","장면","ฉาก"]),
        (["lib","库","ライブラリ","라이브러리","ไลบรารี"],&["lib","库","ライブラリ","라이브러리","ไลบรารี"]),
        (["test","测试","テスト","테스트","ทดสอบ"],  &["test","测试","テスト","테스트","ทดสอบ"]),
        (["util","工具","ユーティリティ","유틸","ยูทิลิตี้"],&["util","utils","工具","ユーティリティ","유틸","ยูทิลิตี้"]),
    ];
    for (forms, aliases) in STEMS {
        if aliases.contains(&stem) {
            let form = forms[idx];
            if form != stem { return Some(form.to_string()); }
        }
    }
    // Also try the folder table
    for (forms, aliases) in FOLDER_NAMES {
        if aliases.contains(&stem) {
            let form = forms[idx];
            if form != stem { return Some(form.to_string()); }
        }
    }
    None
}

fn has_ling_ext(name: &str) -> bool {
    name.ends_with(".ling") || name.ends_with(".灵") || name.ends_with(".霊")
        || name.ends_with(".령") || name.ends_with(".ลิง")
}

// ─── Project walker ───────────────────────────────────────────────────────────

#[derive(Default)]
pub struct NormalizeStats {
    pub files_rewritten: usize,
    pub files_renamed: usize,
    pub dirs_renamed: usize,
    pub unchanged: usize,
}

pub fn normalize_project(
    root: &Path,
    target: Lang,
    dry_run: bool,
    content_only: bool,
    files_only: bool,
) -> io::Result<NormalizeStats> {
    let mut stats = NormalizeStats::default();

    // Collect all .ling files and directories first (before any renaming)
    let mut ling_files: Vec<PathBuf> = Vec::new();
    let mut dirs: Vec<PathBuf> = Vec::new();
    collect_ling_paths(root, &mut ling_files, &mut dirs)?;

    // ── 1. Normalize file contents ──────────────────────────────────────────
    if !files_only {
        for path in &ling_files {
            let source = fs::read_to_string(path)?;
            let normalized = normalize_content(&source, target);
            if normalized == source {
                stats.unchanged += 1;
                continue;
            }
            let rel = path.strip_prefix(root).unwrap_or(path);
            if dry_run {
                println!("  {} {}", "~".cyan(), rel.display());
            } else {
                fs::write(path, &normalized)?;
                println!("  {} {}", "✓".green(), rel.display());
            }
            stats.files_rewritten += 1;
        }
    }

    // ── 2. Rename .ling files ───────────────────────────────────────────────
    if !content_only {
        // Sort deepest paths first so renames don't invalidate parent paths
        let mut rename_files: Vec<(PathBuf, PathBuf)> = Vec::new();
        for path in &ling_files {
            if let Some(file_name) = path.file_name().and_then(|n| n.to_str()) {
                let (stem, ext) = split_ling_ext(file_name);
                if let Some(new_stem) = translate_file_stem(stem, target) {
                    let new_name = format!("{}{}", new_stem, ext);
                    let new_path = path.with_file_name(&new_name);
                    rename_files.push((path.clone(), new_path));
                }
            }
        }
        for (from, to) in rename_files {
            let rel_from = from.strip_prefix(root).unwrap_or(&from);
            let rel_to   = to.strip_prefix(root).unwrap_or(&to);
            if dry_run {
                println!("  {} {} → {}", "mv".yellow(), rel_from.display(), rel_to.display());
            } else {
                fs::rename(&from, &to)?;
                println!("  {} {} → {}", "mv".yellow(), rel_from.display(), rel_to.display());
            }
            stats.files_renamed += 1;
        }

        // ── 3. Rename directories ─────────────────────────────────────────
        // Sort by depth (deepest first) to rename leaves before parents
        let mut dirs_sorted = dirs;
        dirs_sorted.sort_by(|a, b| b.components().count().cmp(&a.components().count()));

        for dir in dirs_sorted {
            if let Some(dir_name) = dir.file_name().and_then(|n| n.to_str()) {
                if let Some(new_name) = translate_folder_name(dir_name, target) {
                    let new_path = dir.with_file_name(&new_name);
                    let rel_from = dir.strip_prefix(root).unwrap_or(&dir);
                    let rel_to   = new_path.strip_prefix(root).unwrap_or(&new_path);
                    if dry_run {
                        println!("  {} {} → {}", "mv".magenta(), rel_from.display(), rel_to.display());
                    } else {
                        fs::rename(&dir, &new_path)?;
                        println!("  {} {} → {}", "mv".magenta(), rel_from.display(), rel_to.display());
                    }
                    stats.dirs_renamed += 1;
                }
            }
        }
    }

    Ok(stats)
}

fn collect_ling_paths(root: &Path, files: &mut Vec<PathBuf>, dirs: &mut Vec<PathBuf>) -> io::Result<()> {
    for entry in fs::read_dir(root)? {
        let entry = entry?;
        let path = entry.path();
        let name = entry.file_name().into_string().unwrap_or_default();
        // Skip hidden dirs and target/
        if name.starts_with('.') || name == "target" || name == ".ling-build" { continue; }
        if path.is_dir() {
            dirs.push(path.clone());
            collect_ling_paths(&path, files, dirs)?;
        } else if has_ling_ext(&name) {
            files.push(path);
        }
    }
    Ok(())
}

/// Split "hello.ling" → ("hello", ".ling")
fn split_ling_ext(name: &str) -> (&str, &str) {
    for ext in &[".ling", ".灵", ".霊", ".령", ".ลิง"] {
        if name.ends_with(ext) {
            return (&name[..name.len() - ext.len()], ext);
        }
    }
    (name, "")
}
