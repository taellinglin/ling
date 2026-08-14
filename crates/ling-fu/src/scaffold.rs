use std::fs;
use std::path::Path;

// ─── Project kind ─────────────────────────────────────────────────────────────

#[derive(Debug, Clone, PartialEq)]
pub enum ProjectKind {
    Bin,      // 独行 — standalone binary
    Lib,      // 共修 — library
    Game,     // 游灵 — game
    Ui,       // 显灵 — UI application
    Ai,       // 智灵 — AI / ML
    Crypto,   // 密灵 — cryptography
    Web,      // 网灵 — Web / WASM
    Polyglot, // 万言 — polyglot lexicon example
}

impl ProjectKind {
    pub fn from_name(s: &str) -> Self {
        match s {
            "bin" | "独行" | "독립" | "単独" | "โปรแกรม" => ProjectKind::Bin,
            "lib" | "共修" | "라이브러리" | "ライブラリ" | "ไลบรารี" => {
                ProjectKind::Lib
            },
            "game" | "游灵" | "게임" | "ゲーム" | "เกม" => ProjectKind::Game,
            "ui" | "显灵" | "UI" | "인터페이스" | "インターフェース" => {
                ProjectKind::Ui
            },
            "ai" | "智灵" | "AI" | "인공지능" | "人工知能" | "ปัญญา" => {
                ProjectKind::Ai
            },
            "crypto" | "密灵" | "암호" | "暗号" | "การเข้ารหัส" => {
                ProjectKind::Crypto
            },
            "web" | "网灵" | "웹" | "ウェブ" | "เว็บ" => ProjectKind::Web,
            "polyglot" | "万言" | "다국어" | "多言語" | "หลายภาษา" => {
                ProjectKind::Polyglot
            },
            _ => ProjectKind::Bin,
        }
    }

    pub fn name_in(&self, lang: &ScaffoldLang) -> &'static str {
        match (self, lang) {
            (ProjectKind::Bin, ScaffoldLang::Chinese) => "独行",
            (ProjectKind::Lib, ScaffoldLang::Chinese) => "共修",
            (ProjectKind::Game, ScaffoldLang::Chinese) => "游灵",
            (ProjectKind::Ui, ScaffoldLang::Chinese) => "显灵",
            (ProjectKind::Ai, ScaffoldLang::Chinese) => "智灵",
            (ProjectKind::Crypto, ScaffoldLang::Chinese) => "密灵",
            (ProjectKind::Web, ScaffoldLang::Chinese) => "网灵",
            (ProjectKind::Polyglot, ScaffoldLang::Chinese) => "万言",
            (ProjectKind::Bin, ScaffoldLang::Japanese) => "単独",
            (ProjectKind::Lib, ScaffoldLang::Japanese) => "共修",
            (ProjectKind::Game, ScaffoldLang::Japanese) => "游霊",
            (ProjectKind::Ui, ScaffoldLang::Japanese) => "表霊",
            (ProjectKind::Ai, ScaffoldLang::Japanese) => "智霊",
            (ProjectKind::Crypto, ScaffoldLang::Japanese) => "密霊",
            (ProjectKind::Web, ScaffoldLang::Japanese) => "網霊",
            (ProjectKind::Polyglot, ScaffoldLang::Japanese) => "万言",
            (ProjectKind::Bin, ScaffoldLang::Korean) => "독립",
            (ProjectKind::Lib, ScaffoldLang::Korean) => "라이브러리",
            (ProjectKind::Game, ScaffoldLang::Korean) => "게임",
            (ProjectKind::Ui, ScaffoldLang::Korean) => "인터페이스",
            (ProjectKind::Ai, ScaffoldLang::Korean) => "인공지능",
            (ProjectKind::Crypto, ScaffoldLang::Korean) => "암호",
            (ProjectKind::Web, ScaffoldLang::Korean) => "웹",
            (ProjectKind::Polyglot, ScaffoldLang::Korean) => "다국어",
            (ProjectKind::Bin, ScaffoldLang::Thai) => "โปรแกรม",
            (ProjectKind::Lib, ScaffoldLang::Thai) => "ไลบรารี",
            (ProjectKind::Game, ScaffoldLang::Thai) => "เกม",
            (ProjectKind::Ui, ScaffoldLang::Thai) => "ส่วนติดต่อ",
            (ProjectKind::Ai, ScaffoldLang::Thai) => "ปัญญา",
            (ProjectKind::Crypto, ScaffoldLang::Thai) => "การเข้ารหัส",
            (ProjectKind::Web, ScaffoldLang::Thai) => "เว็บ",
            (ProjectKind::Polyglot, ScaffoldLang::Thai) => "หลายภาษา",
            (ProjectKind::Bin, _) => "bin",
            (ProjectKind::Lib, _) => "lib",
            (ProjectKind::Game, _) => "game",
            (ProjectKind::Ui, _) => "ui",
            (ProjectKind::Ai, _) => "ai",
            (ProjectKind::Crypto, _) => "crypto",
            (ProjectKind::Web, _) => "web",
            (ProjectKind::Polyglot, _) => "polyglot",
        }
    }

    pub fn to_chinese(&self) -> &'static str {
        self.name_in(&ScaffoldLang::Chinese)
    }

    pub fn to_english(&self) -> &'static str {
        self.name_in(&ScaffoldLang::English)
    }
}

// ─── Scaffolding language ─────────────────────────────────────────────────────

/// The language that determines folder/file names and template content.
#[derive(Debug, Clone, PartialEq)]
pub enum ScaffoldLang {
    English,
    Chinese,
    Japanese,
    Korean,
    Thai,
    Russian,
    Persian,
    Arabic,
    Hebrew,
    Urdu,
    Other(String), // falls back to English for file names
}

impl ScaffoldLang {
    pub fn from_code(code: &str) -> Self {
        match code {
            "en" | "english" => ScaffoldLang::English,
            "zh" | "cn" | "chinese" | "中文" => ScaffoldLang::Chinese,
            "ja" | "jp" | "japanese" | "日本語" => ScaffoldLang::Japanese,
            "ko" | "kr" | "korean" | "한국어" => ScaffoldLang::Korean,
            "th" | "thai" | "ภาษาไทย" => ScaffoldLang::Thai,
            "ru" | "russian" | "русский" => ScaffoldLang::Russian,
            "fa" | "persian" | "farsi" | "فارسی" => ScaffoldLang::Persian,
            "ar" | "arabic" | "العربية" => ScaffoldLang::Arabic,
            "he" | "hebrew" | "עברית" => ScaffoldLang::Hebrew,
            "ur" | "urdu" | "اردو" => ScaffoldLang::Urdu,
            other => ScaffoldLang::Other(other.to_string()),
        }
    }
}

/// All the file/directory names for a given scaffold language.
pub struct DirNames {
    /// Source directory  (灵源 / 霊源 / 령원 / ต้นกำเนิด / src)
    pub src: &'static str,
    /// Lexicon directory (灵言 / 霊語 / 령어 / คำศัพท์  / lexicons)
    pub lexicons: &'static str,
    /// Test directory    (灵镜 / 霊鏡 / 령경 / ทดสอบ     / tests)
    pub tests: &'static str,
    /// Build directory   (灵碑 / 霊碑 / 령비 / สร้าง     / build)
    pub build: &'static str,
    /// Lock directory    (灵符锁 / 霊符錠 / 령잠금 / ล็อก / .lock)
    pub lock: &'static str,
    /// Manifest filename (灵符.toml / 霊符.toml / 영부.toml / ลิงฟู.toml / Ling.toml)
    pub manifest: &'static str,
    /// Entry-point file  (启.灵 / 啓.霊 / 시작.령 / เริ่ม.ลิง / start.ling)
    pub entry: &'static str,
    /// Test file         (试.灵 / 試.霊 / 시험.령 / ทดสอบ.ลิง / test.ling)
    pub test_file: &'static str,
    /// Library core dir  (本 / 本 / 핵심 / แกน / core)
    pub lib_core: &'static str,
    /// "Spirit source" comment prefix for generated files
    #[allow(dead_code)] // emitted by templates; not read by the CLI itself
    pub src_label: &'static str,
}

pub fn dir_names(lang: &ScaffoldLang) -> DirNames {
    match lang {
        ScaffoldLang::Chinese => DirNames {
            src: "灵源",
            lexicons: "灵言",
            tests: "灵镜",
            build: "灵碑",
            lock: "灵符锁",
            manifest: "灵符.toml",
            entry: "启.灵",
            test_file: "试.灵",
            lib_core: "本",
            src_label: "灵源",
        },
        ScaffoldLang::Japanese => DirNames {
            src: "霊源",
            lexicons: "霊語",
            tests: "霊鏡",
            build: "霊碑",
            lock: "霊符錠",
            manifest: "霊符.toml",
            entry: "啓.霊",
            test_file: "試.霊",
            lib_core: "本",
            src_label: "霊源",
        },
        ScaffoldLang::Korean => DirNames {
            src: "령원",
            lexicons: "령어",
            tests: "령경",
            build: "령비",
            lock: "령잠금",
            manifest: "영부.toml",
            entry: "시작.령",
            test_file: "시험.령",
            lib_core: "핵심",
            src_label: "령원",
        },
        ScaffoldLang::Thai => DirNames {
            src: "ต้นกำเนิด",
            lexicons: "คำศัพท์",
            tests: "ทดสอบ",
            build: "สร้าง",
            lock: "ล็อก",
            manifest: "ลิงฟู.toml",
            entry: "เริ่ม.ลิง",
            test_file: "ทดสอบ.ลิง",
            lib_core: "แกน",
            src_label: "ต้นกำเนิด",
        },
        ScaffoldLang::Persian => DirNames {
            src: "منبع",
            lexicons: "واژگان",
            tests: "آزمونها",
            build: "ساخت",
            lock: "قفل",
            manifest: "لینگفو.toml",
            entry: "شروع.لینگ",
            test_file: "آزمون.لینگ",
            lib_core: "هسته",
            src_label: "منبع",
        },
        ScaffoldLang::Arabic => DirNames {
            src: "مصدر",
            lexicons: "معجم",
            tests: "اختبارات",
            build: "بناء",
            lock: "قفل",
            manifest: "لينغفو.toml",
            entry: "ابدأ.لينغ",
            test_file: "اختبار.لينغ",
            lib_core: "نواة",
            src_label: "مصدر",
        },
        ScaffoldLang::Hebrew => DirNames {
            src: "מקור",
            lexicons: "מילון",
            tests: "בדיקות",
            build: "בנייה",
            lock: "מנעול",
            manifest: "לינגפו.toml",
            entry: "התחל.לינג",
            test_file: "בדיקה.לינג",
            lib_core: "גרעין",
            src_label: "מקור",
        },
        ScaffoldLang::Urdu => DirNames {
            src: "ماخذ",
            lexicons: "لغت",
            tests: "آزمائش",
            build: "تعمیر",
            lock: "تالا",
            manifest: "لنگفو.toml",
            entry: "شروع.لنگ",
            test_file: "آزمائش.لنگ",
            lib_core: "مرکز",
            src_label: "ماخذ",
        },
        // English, Russian, and everything else
        _ => DirNames {
            src: "src",
            lexicons: "lexicons",
            tests: "tests",
            build: "build",
            lock: ".lock",
            manifest: "Ling.toml",
            entry: "start.ling",
            test_file: "test.ling",
            lib_core: "core",
            src_label: "src",
        },
    }
}

// ─── ProjectScaffold ──────────────────────────────────────────────────────────

pub struct ProjectScaffold {
    pub name: String,
    pub kind: ProjectKind,
    /// Language code used to choose folder/file names (zh / ja / ko / th / en …)
    pub language: String,
    /// Language code stored in the manifest's `[root]` / `[灵根]` section
    pub root_language: String,
    pub spirit_level: String,
}

// ─── Main scaffold entry point ────────────────────────────────────────────────

pub fn scaffold_project(project: &ProjectScaffold) -> anyhow::Result<()> {
    scaffold_in_dir(project, &std::env::current_dir()?.join(&project.name))
}

/// Scaffold into an existing empty directory (used by `init`).
pub fn scaffold_in_dir(project: &ProjectScaffold, project_dir: &Path) -> anyhow::Result<()> {
    if project_dir.exists() && project_dir.read_dir()?.next().is_some() {
        anyhow::bail!("directory is not empty: {}", project_dir.display());
    }

    let slang = ScaffoldLang::from_code(&project.language);
    let dirs = dir_names(&slang);

    // ── Create directory tree ─────────────────────────────────────────────────
    fs::create_dir_all(project_dir)?;
    fs::create_dir_all(project_dir.join(dirs.src))?;
    fs::create_dir_all(project_dir.join(dirs.lexicons))?;
    fs::create_dir_all(project_dir.join(dirs.tests))?;
    fs::create_dir_all(project_dir.join(dirs.build))?;
    fs::create_dir_all(project_dir.join(dirs.lock))?;
    if project.kind == ProjectKind::Lib {
        fs::create_dir_all(project_dir.join(dirs.src).join(dirs.lib_core))?;
    }

    // ── Files ─────────────────────────────────────────────────────────────────
    write_manifest(project_dir, project, &slang, &dirs)?;
    write_readme(project_dir, project, &slang, &dirs)?;
    write_entry(project_dir, project, &slang, &dirs)?;
    write_lexicons(project_dir, &slang, &dirs)?;
    write_test_file(project_dir, project, &slang, &dirs)?;
    write_gitignore(project_dir, &dirs)?;
    write_ling_ignore(project_dir)?;

    match project.kind {
        ProjectKind::Game => write_game_files(project_dir, project, &slang, &dirs)?,
        ProjectKind::Ui => write_ui_files(project_dir, project, &slang, &dirs)?,
        ProjectKind::Ai => write_ai_files(project_dir, project, &slang, &dirs)?,
        ProjectKind::Crypto => write_crypto_files(project_dir, project, &slang, &dirs)?,
        ProjectKind::Web => write_web_files(project_dir, project, &slang, &dirs)?,
        ProjectKind::Polyglot => write_polyglot_files(project_dir, &dirs)?,
        ProjectKind::Lib => write_lib_files(project_dir, project, &slang, &dirs)?,
        ProjectKind::Bin => {},
    }

    Ok(())
}

// ─── Manifest ────────────────────────────────────────────────────────────────

fn write_manifest(
    dir: &Path,
    p: &ProjectScaffold,
    slang: &ScaffoldLang,
    dirs: &DirNames,
) -> anyhow::Result<()> {
    let content = match slang {
        ScaffoldLang::Chinese => format!(
            r#"
[灵符]
名 = "{name}"
型 = "{kind}"
版 = "2030.0.0"
道 = "2030"

[灵者]
灵林 = {{ 邮 = "taellinglin@gmail.com" }}

[灵境]
语言 = "{lang}"
灵级 = "{level}"
源 = "{src}"
词典 = "{lex}"

[灵根]
灵核 = {{ 版本 = "2030.0", 语言 = "{root}" }}

[灵依]
# 添加依赖:  灵依名 = {{ 版本 = "x.y.z" }}
"#,
            name = p.name,
            kind = p.kind.to_chinese(),
            lang = p.language,
            level = p.spirit_level,
            src = dirs.src,
            lex = dirs.lexicons,
            root = p.root_language
        ),
        ScaffoldLang::Japanese => format!(
            r#"
[霊符]
名 = "{name}"
型 = "{kind}"
版 = "2030.0.0"

[霊者]
霊林 = {{ 邮 = "taellinglin@gmail.com" }}

[霊境]
言語 = "{lang}"
霊級 = "{level}"
源 = "{src}"

[霊根]
霊核 = {{ バージョン = "2030.0", 言語 = "{root}" }}

[霊依]
# 依存関係を追加: 霊依名 = {{ バージョン = "x.y.z" }}
"#,
            name = p.name,
            kind = p.kind.name_in(slang),
            lang = p.language,
            level = p.spirit_level,
            src = dirs.src,
            root = p.root_language
        ),
        ScaffoldLang::Korean => format!(
            r#"
[영부]
이름 = "{name}"
종류 = "{kind}"
버전 = "2030.0.0"

[영자]
영림 = {{ 이메일 = "taellinglin@gmail.com" }}

[영경]
언어 = "{lang}"
영급 = "{level}"
소스 = "{src}"

[영근]
영핵 = {{ 버전 = "2030.0", 언어 = "{root}" }}

[영의]
# 의존성 추가: 패키지명 = {{ 버전 = "x.y.z" }}
"#,
            name = p.name,
            kind = p.kind.name_in(slang),
            lang = p.language,
            level = p.spirit_level,
            src = dirs.src,
            root = p.root_language
        ),
        ScaffoldLang::Thai => format!(
            r#"
[ลิงฟู]
ชื่อ = "{name}"
ประเภท = "{kind}"
เวอร์ชัน = "2030.0.0"

[ผู้สร้าง]
ลิงลิน = {{ อีเมล = "taellinglin@gmail.com" }}

[สภาพแวดล้อม]
ภาษา = "{lang}"
ระดับ = "{level}"
แหล่งที่มา = "{src}"

[รากฐาน]
แกนกลาง = {{ เวอร์ชัน = "2030.0", ภาษา = "{root}" }}

[ขึ้นอยู่กับ]
# เพิ่มการพึ่งพา: ชื่อแพ็กเกจ = {{ เวอร์ชัน = "x.y.z" }}
"#,
            name = p.name,
            kind = p.kind.name_in(slang),
            lang = p.language,
            level = p.spirit_level,
            src = dirs.src,
            root = p.root_language
        ),
        _ => format!(
            r#"
[spirit]
name = "{name}"
kind = "{kind}"
version = "2030.0.0"

[author]
ling-lin = {{ email = "taellinglin@gmail.com" }}

[realm]
language = "{lang}"
level    = "{level}"
source   = "{src}"

[root]
core = {{ version = "2030.0", language = "{root}" }}

[depends]
# Add dependencies: spirit-name = {{ version = "x.y.z" }}
"#,
            name = p.name,
            kind = p.kind.to_english(),
            lang = p.language,
            level = p.spirit_level,
            src = dirs.src,
            root = p.root_language
        ),
    };
    fs::write(dir.join(dirs.manifest), content.trim_start())?;
    Ok(())
}

// ─── README ───────────────────────────────────────────────────────────────────

fn write_readme(
    dir: &Path,
    p: &ProjectScaffold,
    slang: &ScaffoldLang,
    dirs: &DirNames,
) -> anyhow::Result<()> {
    let (cli, build_cmd, run_cmd, test_cmd, src_dir, lex_dir, test_dir, build_dir) = match slang {
        ScaffoldLang::Chinese => ("灵符", "筑", "行", "试", "灵源/", "灵言/", "灵镜/", "灵碑/"),
        ScaffoldLang::Japanese => ("霊符", "築", "行", "試", "霊源/", "霊語/", "霊鏡/", "霊碑/"),
        ScaffoldLang::Korean => ("영부", "짓", "달", "시", "령원/", "령어/", "령경/", "령비/"),
        ScaffoldLang::Thai => (
            "ลิงฟู",
            "สร้าง",
            "เรียก",
            "ทดสอบ",
            "ต้นกำเนิด/",
            "คำศัพท์/",
            "ทดสอบ/",
            "สร้าง/",
        ),
        _ => (
            "lingfu",
            "build",
            "run",
            "test",
            "src/",
            "lexicons/",
            "tests/",
            "build/",
        ),
    };
    let content = format!(
        r#"# {name}

**Kind:** {kind}  ·  **Level:** {level}  ·  **Language:** {lang}

## Quick Start

```sh
cd {name}
{cli} {build_cmd}
{cli} {run_cmd}
{cli} {test_cmd}
```

## Structure

```
{name}/
├── {manifest}        # Project manifest
├── {src_dir}         # Source files
│   └── {entry}       # Entry point
├── {lex_dir}         # Lexicon / keyword files
├── {test_dir}        # Tests
└── {build_dir}       # Build artefacts
```

## Module Imports

```ling
# Load another .ling file
use "path/to/module"
载 "数学/运算"              # Chinese
載 "数学/演算"              # Japanese
사용 "수학/연산"            # Korean
ใช้ "คณิตศาสตร์/การดำเนินการ"  # Thai

# Import with namespace alias
use "utils" as u
载 "工具" 为 工             # Chinese
```

---
*Generated by lingfu for the Goddess Ling* 🌊
"#,
        name = p.name,
        kind = p.kind.name_in(slang),
        level = p.spirit_level,
        lang = p.language,
        cli = cli,
        build_cmd = build_cmd,
        run_cmd = run_cmd,
        test_cmd = test_cmd,
        manifest = dirs.manifest,
        src_dir = src_dir,
        entry = dirs.entry,
        lex_dir = lex_dir,
        test_dir = test_dir,
        build_dir = build_dir,
    );
    fs::write(dir.join("README.md"), content)?;
    Ok(())
}

// ─── Entry point ─────────────────────────────────────────────────────────────

fn write_entry(
    dir: &Path,
    p: &ProjectScaffold,
    slang: &ScaffoldLang,
    dirs: &DirNames,
) -> anyhow::Result<()> {
    let path = dir.join(dirs.src).join(dirs.entry);
    let content = match slang {
        ScaffoldLang::Chinese => format!(
            "# {src}/{entry} — 灵境之门\n\n令 启 = 执 {{\n    印(\"🌊 欢迎来到 {name} 灵境 🌊\")\n    印(\"灵级: {level}\")\n\n    # 你的灵境代码从此觉醒\n}}\n",
            src = dirs.src,
            entry = dirs.entry,
            name = p.name,
            level = p.spirit_level
        ),
        ScaffoldLang::Japanese => format!(
            "# {src}/{entry} — 霊境への扉\n\n束縛 啓 = 実行 {{\n    印刷(\"🌊 ようこそ {name} 霊境へ 🌊\")\n    印刷(\"霊級: {level}\")\n\n    # あなたの霊境コードがここから目覚める\n}}\n",
            src = dirs.src,
            entry = dirs.entry,
            name = p.name,
            level = p.spirit_level
        ),
        ScaffoldLang::Korean => format!(
            "# {src}/{entry} — 령경으로의 문\n\n바인드 시작 = 실행 {{\n    print(\"🌊 {name} 령경에 오신 것을 환영합니다 🌊\")\n    print(\"령급: {level}\")\n\n    # 당신의 령경 코드가 여기서 깨어납니다\n}}\n",
            src = dirs.src,
            entry = dirs.entry,
            name = p.name,
            level = p.spirit_level
        ),
        ScaffoldLang::Thai => format!(
            "# {src}/{entry} — ประตูสู่อาณาจักรลิง\n\nผูก เริ่ม = ทำ {{\n    พิมพ์(\"🌊 ยินดีต้อนรับสู่ {name} อาณาจักรลิง 🌊\")\n    พิมพ์(\"ระดับ: {level}\")\n\n    # โค้ดของคุณตื่นขึ้นที่นี่\n}}\n",
            src = dirs.src,
            entry = dirs.entry,
            name = p.name,
            level = p.spirit_level
        ),
        _ => format!(
            "# {src}/{entry} — gateway to the Spirit Realm\n\nbind start = do {{\n    print(\"🌊 Welcome to {name} Spirit Realm 🌊\")\n    print(\"Spirit Level: {level}\")\n\n    # Your Ling code awakens here\n}}\n",
            src = dirs.src,
            entry = dirs.entry,
            name = p.name,
            level = p.spirit_level
        ),
    };
    fs::write(path, content)?;
    Ok(())
}

// ─── Lexicons ─────────────────────────────────────────────────────────────────

fn write_lexicons(dir: &Path, slang: &ScaffoldLang, dirs: &DirNames) -> anyhow::Result<()> {
    let lex_dir = dir.join(dirs.lexicons);

    // Always write the English canonical lexicon
    fs::write(
        lex_dir.join("en.ling"),
        r#"name = "English"
code = "en"
canonical = true

[keywords]
bind = "bind"
start = "start"
print = "print"
if = "if"
else = "else"
for = "for"
while = "while"
use = "use"
"#,
    )?;

    // Write the project's preferred language lexicon
    match slang {
        ScaffoldLang::Chinese => {
            fs::write(
                lex_dir.join("zh.ling"),
                r#"name = "中文"
code = "zh"
canonical = false
base = "en"

[keywords]
bind = "令"
start = "启"
print = "印"
if = "若"
else = "否则"
for = "历"
while = "循"
use = "载"
use_as = "为"
"#,
            )?;
        },
        ScaffoldLang::Japanese => {
            fs::write(
                lex_dir.join("ja.ling"),
                r#"name = "日本語"
code = "ja"
canonical = false
base = "en"

[keywords]
bind = "束縛"
start = "啓"
print = "印刷"
if = "もし"
else = "他"
for = "繰"
while = "間"
use = "使う"
use_as = "として"
"#,
            )?;
        },
        ScaffoldLang::Korean => {
            fs::write(
                lex_dir.join("ko.ling"),
                r#"name = "한국어"
code = "ko"
canonical = false
base = "en"

[keywords]
bind = "바인드"
start = "시작"
print = "출력"
if = "만약"
else = "아니면"
for = "위해"
while = "동안"
use = "사용"
use_as = "로서"
"#,
            )?;
        },
        ScaffoldLang::Thai => {
            fs::write(
                lex_dir.join("th.ling"),
                r#"name = "ภาษาไทย"
code = "th"
canonical = false
base = "en"

[keywords]
bind = "ผูก"
start = "เริ่ม"
print = "พิมพ์"
if = "ถ้า"
else = "มิฉะนั้น"
for = "สำหรับ"
while = "ขณะที่"
use = "ใช้"
use_as = "เป็น"
"#,
            )?;
        },
        _ => {},
    }
    Ok(())
}

// ─── Test file ────────────────────────────────────────────────────────────────

fn write_test_file(
    dir: &Path,
    p: &ProjectScaffold,
    slang: &ScaffoldLang,
    dirs: &DirNames,
) -> anyhow::Result<()> {
    let path = dir.join(dirs.tests).join(dirs.test_file);
    let content = match slang {
        ScaffoldLang::Chinese => format!(
            "# {tests}/{file} — 灵境之镜\n\n令 试 = 执 {{\n    # 验证基本算术 (Verify basic arithmetic)\n    若 1 + 1 == 2 {{\n        印(\"✨ 万镜皆明：算术正确 ✨\")\n    }}\n    印(\"🔮 试灵完成 for {name}\")\n}}\n",
            tests = dirs.tests,
            file = dirs.test_file,
            name = p.name
        ),
        ScaffoldLang::Japanese => format!(
            "# {tests}/{file} — 霊境の鏡\n\n束縛 試 = 実行 {{\n    もし 1 + 1 == 2 {{\n        印刷(\"✨ 全ての鏡が明らか ✨\")\n    }}\n    印刷(\"🔮 霊テスト完了: {name}\")\n}}\n",
            tests = dirs.tests,
            file = dirs.test_file,
            name = p.name
        ),
        ScaffoldLang::Korean => format!(
            "# {tests}/{file} — 령경의 거울\n\n바인드 시험 = 실행 {{\n    만약 1 + 1 == 2 {{\n        print(\"✨ 모든 거울이 맑다 ✨\")\n    }}\n    print(\"🔮 령 테스트 완료: {name}\")\n}}\n",
            tests = dirs.tests,
            file = dirs.test_file,
            name = p.name
        ),
        ScaffoldLang::Thai => format!(
            "# {tests}/{file} — กระจกแห่งอาณาจักร\n\nผูก ทดสอบ = ทำ {{\n    ถ้า 1 + 1 == 2 {{\n        พิมพ์(\"✨ กระจกทุกบานใส ✨\")\n    }}\n    พิมพ์(\"🔮 ทดสอบลิงเสร็จ: {name}\")\n}}\n",
            tests = dirs.tests,
            file = dirs.test_file,
            name = p.name
        ),
        _ => format!(
            "# {tests}/{file} — Spirit Mirror\n\nbind test = do {{\n    if 1 + 1 == 2 {{\n        print(\"✨ All mirrors are clear ✨\")\n    }}\n    print(\"🔮 Ling tests passed for {name}\")\n}}\n",
            tests = dirs.tests,
            file = dirs.test_file,
            name = p.name
        ),
    };
    fs::write(path, content)?;
    Ok(())
}

// ─── .gitignore ───────────────────────────────────────────────────────────────

fn write_gitignore(dir: &Path, dirs: &DirNames) -> anyhow::Result<()> {
    let content = format!(
        "{}/\n{}/\n*.ling-build/\n.ling-build/\ndist/\n",
        dirs.build, dirs.lock
    );
    fs::write(dir.join(".gitignore"), content)?;
    Ok(())
}

/// `ling.ignore` -- consulted by `lingfu seed` (on top of `git ls-files`)
/// and `lingfu publish` (which otherwise isn't `.gitignore`-aware at all),
/// so a keyfile, recovery share, or oversized local asset has one place to
/// be excluded from every whole-project operation, not one per tool.
fn write_ling_ignore(dir: &Path) -> anyhow::Result<()> {
    let content = "\
# Consulted by `lingfu seed` and `lingfu publish` -- keep keyfiles,
# recovery shares, and other sensitive or oversized local-only files
# out of both, in one place. One glob pattern per line.
seed.wav
*.share
*.key
*.pem
";
    fs::write(dir.join(IGNORE_FILE_NAME), content)?;
    Ok(())
}

const IGNORE_FILE_NAME: &str = "ling.ignore";

// ─── Type-specific extra files ────────────────────────────────────────────────

fn write_lib_files(
    dir: &Path,
    p: &ProjectScaffold,
    slang: &ScaffoldLang,
    dirs: &DirNames,
) -> anyhow::Result<()> {
    let core_dir = dir.join(dirs.src).join(dirs.lib_core);
    let lib_file = core_dir.join(dirs.entry);
    let content = match slang {
        ScaffoldLang::Chinese => format!(
            "# {src}/{core}/{entry} — 共修核心模块\n\n函 问好(名字) {{\n    归 格式(\"🌸 你好，{{}}！来自 {} 灵库\", 名字)\n}}\n",
            p.name,
            src = dirs.src,
            core = dirs.lib_core,
            entry = dirs.entry
        ),
        ScaffoldLang::Japanese => format!(
            "# {src}/{core}/{entry} — 共修コアモジュール\n\n関数 挨拶(名前) {{\n    帰る 格式(\"🌸 こんにちは、{{}}！{} 霊ライブラリより\", 名前)\n}}\n",
            p.name,
            src = dirs.src,
            core = dirs.lib_core,
            entry = dirs.entry
        ),
        ScaffoldLang::Korean => format!(
            "# {src}/{core}/{entry} — 공수 핵심 모듈\n\n함수 인사(이름) {{\n    반환 format(\"🌸 안녕하세요, {{}}! {} 령 라이브러리에서\", 이름)\n}}\n",
            p.name,
            src = dirs.src,
            core = dirs.lib_core,
            entry = dirs.entry
        ),
        ScaffoldLang::Thai => format!(
            "# {src}/{core}/{entry} — โมดูลหลักของห้องสมุด\n\nฟังก์ชัน ทักทาย(ชื่อ) {{\n    คืน format(\"🌸 สวัสดี, {{}}! จากไลบรารี {} ลิง\", ชื่อ)\n}}\n",
            p.name,
            src = dirs.src,
            core = dirs.lib_core,
            entry = dirs.entry
        ),
        _ => format!(
            "# {src}/{core}/{entry} — library core module\n\nfn greet(name) {{\n    return format(\"🌸 Hello, {{}}! from {} spirit library\", name)\n}}\n",
            p.name,
            src = dirs.src,
            core = dirs.lib_core,
            entry = dirs.entry
        ),
    };
    fs::write(lib_file, content)?;
    Ok(())
}

fn write_game_files(
    dir: &Path,
    p: &ProjectScaffold,
    slang: &ScaffoldLang,
    dirs: &DirNames,
) -> anyhow::Result<()> {
    let content = match slang {
        ScaffoldLang::Chinese => format!(
            "# 游戏实体 — {name}\n\n令 启 = 执 {{\n    令 宽 = get_width()\n    令 高 = get_height()\n    open_window(\"{name}\")\n    循 window_is_open() {{\n        fill(0, 0, 0)\n        vtex_chakra(0, 0, 8,  1,0,0,  0,1,0,  2.0, 8,  0.001, 1.57)\n        display()\n    }}\n}}\n",
            name = p.name
        ),
        ScaffoldLang::Thai => format!(
            "# เกม — {name}\n\nผูก เริ่ม = ทำ {{\n    เปิดหน้าต่าง(\"{name}\")\n    循 window_is_open() {{\n        เติม(0, 0, 0)\n        vtex_chakra(0, 0, 8,  1,0,0,  0,1,0,  2.0, 8,  0.001, 3.50)\n        แสดงผล()\n    }}\n}}\n",
            name = p.name
        ),
        _ => format!(
            "# Game — {name}\n\nbind start = do {{\n    open_window(\"{name}\")\n    while window_is_open() {{\n        fill(0, 0, 0)\n        vtex_chakra(0, 0, 8,  1,0,0,  0,1,0,  2.0, 8,  0.001, 1.57)\n        display()\n    }}\n}}\n",
            name = p.name
        ),
    };
    fs::write(dir.join(dirs.src).join("game.ling"), content)?;
    Ok(())
}

fn write_ui_files(
    dir: &Path,
    p: &ProjectScaffold,
    slang: &ScaffoldLang,
    dirs: &DirNames,
) -> anyhow::Result<()> {
    let content = match slang {
        ScaffoldLang::Chinese => format!(
            "# UI 应用 — {name}\n\n令 启 = 执 {{\n    open_fullscreen(\"{name} — 显灵\")\n    循 window_is_open() {{\n        fill(12, 8, 20)\n        vtex_star(0, 0, 5,  1,0,0,  0,1,0,  8, 2.0, 0.8, 0.0003, 0, 1.57)\n        display()\n    }}\n}}\n",
            name = p.name
        ),
        _ => format!(
            "# UI app — {name}\n\nbind start = do {{\n    open_fullscreen(\"{name} — Spirit UI\")\n    while window_is_open() {{\n        fill(12, 8, 20)\n        vtex_star(0, 0, 5,  1,0,0,  0,1,0,  8, 2.0, 0.8, 0.0003, 0, 1.57)\n        display()\n    }}\n}}\n",
            name = p.name
        ),
    };
    fs::write(dir.join(dirs.src).join("ui.ling"), content)?;
    Ok(())
}

fn write_ai_files(
    dir: &Path,
    p: &ProjectScaffold,
    slang: &ScaffoldLang,
    dirs: &DirNames,
) -> anyhow::Result<()> {
    let content = match slang {
        ScaffoldLang::Chinese => format!(
            "# AI 模型 — {name}\n\n令 启 = 执 {{\n    印(\"🧠 智灵模型 {name} 已就绪\")\n    # 训练 / 推理代码放在这里\n}}\n",
            name = p.name
        ),
        _ => format!(
            "# AI model — {name}\n\nbind start = do {{\n    print(\"🧠 AI Spirit {name} ready\")\n    # Training / inference code goes here\n}}\n",
            name = p.name
        ),
    };
    fs::write(dir.join(dirs.src).join("ai.ling"), content)?;
    Ok(())
}

fn write_crypto_files(
    dir: &Path,
    p: &ProjectScaffold,
    slang: &ScaffoldLang,
    dirs: &DirNames,
) -> anyhow::Result<()> {
    let content = match slang {
        ScaffoldLang::Chinese => format!(
            "# 密灵模块 — {name}\n\n令 启 = 执 {{\n    印(\"🔒 密灵 {name} 护法\")\n    # 加密 / 解密代码放在这里\n}}\n",
            name = p.name
        ),
        _ => format!(
            "# Crypto module — {name}\n\nbind start = do {{\n    print(\"🔒 Spirit Crypto {name}\")\n    # Encryption / hashing code goes here\n}}\n",
            name = p.name
        ),
    };
    fs::write(dir.join(dirs.src).join("crypto.ling"), content)?;
    Ok(())
}

fn write_web_files(
    dir: &Path,
    p: &ProjectScaffold,
    slang: &ScaffoldLang,
    dirs: &DirNames,
) -> anyhow::Result<()> {
    let content = match slang {
        ScaffoldLang::Chinese => format!(
            "# 网灵服务 — {name}\n\n令 启 = 执 {{\n    印(\"🌐 网灵 {name} 启航\")\n    # Web / WASM 代码放在这里\n}}\n",
            name = p.name
        ),
        _ => format!(
            "# Web / WASM service — {name}\n\nbind start = do {{\n    print(\"🌐 Web Spirit {name} launched\")\n    # Web / WASM code goes here\n}}\n",
            name = p.name
        ),
    };
    fs::write(dir.join(dirs.src).join("web.ling"), content)?;
    Ok(())
}

fn write_polyglot_files(dir: &Path, dirs: &DirNames) -> anyhow::Result<()> {
    let lex_dir = dir.join(dirs.lexicons);
    for (code, name, bind_kw, start_kw, print_kw, use_kw) in [
        ("en", "English", "bind", "start", "print", "use"),
        ("zh", "中文", "令", "启", "印", "载"),
        ("ja", "日本語", "束縛", "啓", "印刷", "使う"),
        ("ko", "한국어", "바인드", "시작", "출력", "사용"),
        ("th", "ภาษาไทย", "ผูก", "เริ่ม", "พิมพ์", "ใช้"),
    ] {
        let content = format!(
            "name = \"{name}\"\ncode = \"{code}\"\ncanonical = {canonical}\n\n[keywords]\nbind = \"{bind_kw}\"\nstart = \"{start_kw}\"\nprint = \"{print_kw}\"\nuse = \"{use_kw}\"\n",
            name = name,
            code = code,
            canonical = if code == "en" { "true" } else { "false" },
            bind_kw = bind_kw,
            start_kw = start_kw,
            print_kw = print_kw,
            use_kw = use_kw,
        );
        fs::write(lex_dir.join(format!("{code}.ling")), content)?;
    }

    let polyglot_demo = r#"# Polyglot demo — all languages load the same file
# This file works whether you write in Chinese, Japanese, Korean, Thai, or English.

# English
use "shared/greetings"

# Chinese
载 "shared/greetings" 为 问候

# Japanese
使う "shared/greetings"

bind start = do {
    print("🌍 Hello from all languages!")
    # 令 启 = 执 { 印("🌍 来自所有语言的问候！") }
}
"#;
    fs::write(dir.join(dirs.src).join("polyglot_demo.ling"), polyglot_demo)?;

    // shared greetings module
    fs::create_dir_all(dir.join(dirs.src).join("shared"))?;
    let greet = r#"fn hello(name) {
    return format("Hello / 你好 / こんにちは / 안녕 / สวัสดี, {}!", name)
}
"#;
    fs::write(
        dir.join(dirs.src).join("shared").join("greetings.ling"),
        greet,
    )?;
    Ok(())
}

#[allow(dead_code)] // public helper retained for external callers / future use
pub fn sanitize_project_kind(kind: &str) -> String {
    kind.trim().replace(' ', "_")
}
