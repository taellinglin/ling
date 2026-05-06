use std::fs;
use std::io;
use std::path::{Path, PathBuf};

pub struct ProjectScaffold {
    pub name: String,
    pub kind: ProjectKind,
    pub language: String,
    pub spirit_level: String,
}

#[derive(Debug, Clone)]
pub enum ProjectKind {
    Bin,      // 独行 - Standalone Binary
    Lib,      // 共修 - Library
    Game,     // 游灵 - Game
    Ui,       // 显灵 - UI Application
    Ai,       // 智灵 - AI/ML Project
    Crypto,   // 密灵 - Cryptography
    Web,      // 网灵 - Web/WASM
    Polyglot, // 万言 - Polyglot Lexicon
}

impl ProjectKind {
    pub fn from_str(s: &str) -> Self {
        match s {
            "bin" | "独行" => ProjectKind::Bin,
            "lib" | "共修" => ProjectKind::Lib,
            "game" | "游灵" => ProjectKind::Game,
            "ui" | "显灵" => ProjectKind::Ui,
            "ai" | "智灵" => ProjectKind::Ai,
            "crypto" | "密灵" => ProjectKind::Crypto,
            "web" | "网灵" => ProjectKind::Web,
            "polyglot" | "万言" => ProjectKind::Polyglot,
            _ => ProjectKind::Bin,
        }
    }
    
    pub fn to_chinese(&self) -> &'static str {
        match self {
            ProjectKind::Bin => "独行",
            ProjectKind::Lib => "共修",
            ProjectKind::Game => "游灵",
            ProjectKind::Ui => "显灵",
            ProjectKind::Ai => "智灵",
            ProjectKind::Crypto => "密灵",
            ProjectKind::Web => "网灵",
            ProjectKind::Polyglot => "万言",
        }
    }
    
    pub fn to_english(&self) -> &'static str {
        match self {
            ProjectKind::Bin => "bin",
            ProjectKind::Lib => "lib",
            ProjectKind::Game => "game",
            ProjectKind::Ui => "ui",
            ProjectKind::Ai => "ai",
            ProjectKind::Crypto => "crypto",
            ProjectKind::Web => "web",
            ProjectKind::Polyglot => "polyglot",
        }
    }
}

pub fn scaffold_project(project: &ProjectScaffold) -> anyhow::Result<()> {
    let cwd = std::env::current_dir()?;
    let project_dir = cwd.join(&project.name);

    if project_dir.exists() {
        anyhow::bail!("Target directory already exists: {}", project_dir.display());
    }

    // Create directory structure
    fs::create_dir_all(&project_dir)?;
    fs::create_dir_all(project_dir.join("灵源"))?;      // Ling Source
    fs::create_dir_all(project_dir.join("灵言"))?;      // Lexicons
    fs::create_dir_all(project_dir.join("灵镜"))?;      // Tests
    fs::create_dir_all(project_dir.join("灵碑"))?;      // Build artifacts
    fs::create_dir_all(project_dir.join("灵符锁"))?;    // Lock files
    
    if matches!(project.kind, ProjectKind::Lib) {
        fs::create_dir_all(project_dir.join("灵源").join("本"))?;  // Core module
    }
    
    // Create README
    create_readme(&project_dir, project)?;
    
    // Create 灵符.toml manifest
    create_manifest(&project_dir, project)?;
    
    // Create entry point based on project type
    create_entry_point(&project_dir, project)?;
    
    // Create lexicon files
    create_lexicon_files(&project_dir, &project.language)?;
    
    // Create type-specific files
    match project.kind {
        ProjectKind::Game => create_game_files(&project_dir, project)?,
        ProjectKind::Ui => create_ui_files(&project_dir, project)?,
        ProjectKind::Ai => create_ai_files(&project_dir, project)?,
        ProjectKind::Crypto => create_crypto_files(&project_dir, project)?,
        ProjectKind::Web => create_web_files(&project_dir, project)?,
        ProjectKind::Polyglot => create_polyglot_files(&project_dir, project)?,
        _ => {}
    }
    
    // Create test file
    create_test_file(&project_dir, project)?;
    
    Ok(())
}

fn create_readme(dir: &Path, project: &ProjectScaffold) -> anyhow::Result<()> {
    let content = format!(r#"# {}

## 灵境信息 (Spirit Realm Info)

- **类型 (Type)**: {}
- **灵级 (Spirit Level)**: {}
- **语言 (Language)**: {}

## 灵境之门 (Getting Started)

```bash
# 筑造灵境 (Build)
{} build

# 行灵启程 (Run)
{} run

# 试灵照心 (Test)
{} test

灵境结构 (Structure)
text

{}/                        
├── 灵符.toml              # 灵境符箓 (Project manifest)
├── 灵源/                   # 灵源之地 (Source code)
│   └── 启.灵               # 灵境之门 (Entry point)
├── 灵言/                   # 万言词典 (Lexicons)
├── 灵镜/                   # 照心之镜 (Tests)
└── 灵碑/                   # 筑造之碑 (Build artifacts)

灵境咒语 (License)

This project is under the Ling Harmony License.
"#,
project.name,
project.kind.to_chinese(),
project.spirit_level,
project.language,
project.name, project.name, project.name,
project.name,
);

fs::write(dir.join("README.md"), content)?;
Ok(())
}

fn create_manifest(dir: &Path, project: &ProjectScaffold) -> anyhow::Result<()> {
let content = format!(r#"
[灵符]
名 = "{}"
型 = "{}"
版 = "2030.0.0"
道 = "2030"

[灵者]
灵林 = {{ 邮 = "taellinglin@gmail.com" }}

[灵境]
语言 = "{}"
灵级 = "{}"

[灵根]
灵核 = {{ 版 = "2030.0" }}
"#,
project.name,
project.kind.to_chinese(),
project.language,
project.spirit_level
);

fs::write(dir.join("灵符.toml"), content)?;
Ok(())
}

fn create_entry_point(dir: &Path, project: &ProjectScaffold) -> anyhow::Result<()> {
let entry_path = dir.join("灵源").join("启.灵");

let content = match project.language.as_str() {
"zh" => format!(r#"// 灵源/启.灵 - 灵境之门

灵符 启 = 执 {{
印("🌊 欢迎来到 {} 灵境 🌊")
印("灵级: {}")

// 你的灵境代码从此觉醒
}}
"#, project.name, project.spirit_level),

"ja" => format!(r#"// 霊源/啓.霊 - 霊境への扉

霊符 啓 = 実行 {{
印刷("🌊 ようこそ {} 霊境へ 🌊")
印刷("霊級: {}")

// あなたの霊境コードがここから目覚める
}}
"#, project.name, project.spirit_level),

"ko" => format!(r#"// 령원/계.령 - 령경으로의 문

령부 계 = 집행 {{
인쇄("🌊 {} 령경에 오신 것을 환영합니다 🌊")
인쇄("령급: {}")

// 당신의 령경 코드가 여기서 깨어납니다
}}
"#, project.name, project.spirit_level),

_ => format!(r#"// Ling Source/Start.ling - Gateway to the Spirit Realm

bind start = do {{
print("🌊 Welcome to {} Spirit Realm 🌊")
print("Spirit Level: {}")

// Your Ling code awakens here
}}
"#, project.name, project.spirit_level),
};

fs::write(entry_path, content)?;
Ok(())
}

fn create_lexicon_files(dir: &Path, language: &str) -> anyhow::Result<()> {
// Always create English lexicon (canonical)
let en_lexicon = r#"
name = "English"
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
"#;
fs::write(dir.join("灵言").join("en.ling"), en_lexicon)?;

// Create user's preferred language lexicon
if language == "zh" {
let zh_lexicon = r#"
name = "中文"
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
while = "当"
"#;
fs::write(dir.join("灵言").join("zh.ling"), zh_lexicon)?;
}

Ok(())
}

fn create_test_file(dir: &Path, project: &ProjectScaffold) -> anyhow::Result<()> {
let content = match project.language.as_str() {
"zh" => r#"// 灵镜/试.灵 - 灵境之镜

灵符 试 = 执 {
灵镜::断言(1 + 1 == 2, "道法自然")
印("✨ 万镜皆明 ✨")
}
"#,
_ => r#"// Spirit Mirror/Test.ling - Mirror of the Realm

bind test = do {
spirit::assert(1 + 1 == 2, "The Dao flows")
print("✨ All mirrors are clear ✨")
}
"#
};

fs::write(dir.join("灵镜").join("试.灵"), content)?;
Ok(())
}

fn create_game_files(dir: &Path, project: &ProjectScaffold) -> anyhow::Result<()> {
let content = r#"// Game configuration - 游灵配置
// Add your game entities, physics, and rendering logic here

form 玩家 {
x: 数,
y: 数,
生命: 数,
分数: 数,
}

灵符 启 = 执 {
游灵::初醒()

令 主角 = 玩家 {
x: 0,
y: 0,
生命: 100,
分数: 0,
}

游灵::启动()
}"#;

fs::write(dir.join("灵源").join("游戏.灵"), content)?;
Ok(())
}

fn create_ui_files(dir: &Path, project: &ProjectScaffold) -> anyhow::Result<()> {
let content = r#"// UI Application - 显灵应用

灵符 启 = 执 {
显灵::初现()

令 窗口 = 显灵::窗()
.名("我的灵境")
宽(800)
高(600)

窗口.渲染(|| {
行 {
文本("欢迎来到灵境")
按钮("点我").点按(|| {
印("灵触觉醒!")
})
}
})
}"#;

fs::write(dir.join("灵源").join("界面.灵"), content)?;
Ok(())
}

fn create_ai_files(dir: &Path, project: &ProjectScaffold) -> anyhow::Result<()> {
let content = r#"// AI Model - 智灵模型

智灵::初醒()

令 模型 = 智灵::灵络::新()
.层(智灵::全联(784, 256))
.层(智灵::激(智灵::修正))
.层(智灵::全联(256, 10))
.就()

灵符 启 = 执 {
印("智灵模型已就绪")
印("输入层: 784, 输出层: 10")

// 训练或推理代码
}"#;

fs::write(dir.join("灵源").join("智灵.灵"), content)?;
Ok(())
}

fn create_crypto_files(dir: &Path, project: &ProjectScaffold) -> anyhow::Result<()> {
let content = r#"// Cryptography Module - 密灵模块

密灵::启轮()

灵符 启 = 执 {
令 钥 = 密灵::钥::生(密灵::式::灵锁)
令 原文 = "灵言秘语"
令 密文 = 密灵::封(钥, 原文.转字节())

印("原文: ", 原文)
印("密文: ", 密文.转十六())
印("解密: ", 密灵::开(钥, 密文).转文())

印("🔒 密灵护法")
}"#;

fs::write(dir.join("灵源").join("密灵.灵"), content)?;
Ok(())
}

fn create_web_files(dir: &Path, project: &ProjectScaffold) -> anyhow::Result<()> {
let content = r#"// Web/WASM Module - 网灵模块

灵符 启 = 执 {
网灵::初醒()

令 服务 = 网灵::服务()
.路("/", |_| "🌊 灵境之门 🌊")
.路("/api", |req| {
印("收到请求: ", req)
"{"status": "ok"}"
})

服务.听("0.0.0.0:8080").等待()
}"#;

fs::write(dir.join("灵源").join("网.灵"), content)?;
Ok(())
}

fn create_polyglot_files(dir: &Path, project: &ProjectScaffold) -> anyhow::Result<()> {
// Create multiple lexicon examples
let lexicons = [
("en", "English", "bind", "start", "print"),
("zh", "中文", "令", "启", "印"),
("ja", "日本語", "変数", "開始", "印刷"),
("ko", "한국어", "변수", "시작", "출력"),
];

for (code, name, bind_word, start_word, print_word) in lexicons {
let content = format!(r#"
name = "{}"
code = "{}"
canonical = false

[keywords]
bind = "{}"
start = "{}"
print = "{}"
"#, name, code, bind_word, start_word, print_word);

fs::write(dir.join("灵言").join(format!("{}.ling", code)), content)?;
}

// Create polyglot example
let content = r#"// Polyglot Example - 万言示例
// This file demonstrates multiple lexicon support

bind message = "Hello from all languages!"
print(message)

// You can write the same code in:
// 令 消息 = "来自所有语言的问候！"
// 印(消息)
"#;

fs::write(dir.join("灵源").join("万言.灵"), content)?;
Ok(())
}

pub fn sanitize_project_kind(kind: &str) -> String {
kind.trim().replace(' ', "_")
}