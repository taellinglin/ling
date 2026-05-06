use clap::{Parser, Subcommand};
use colored::*;
use dialoguer::{Confirm, Input, Select};
use std::env;
use std::fs;
use std::path::PathBuf;
use unicode_normalization::UnicodeNormalization;

// Detect which name was used to call this binary
fn detect_invocation_name() -> InvocationLanguage {
    let args: Vec<String> = env::args().collect();
    let exec_name = args[0].as_str();
    
    // Extract base filename without path
    let base_name = std::path::Path::new(exec_name)
        .file_stem()
        .map(|s| s.to_string_lossy().to_string())
        .unwrap_or_else(|| exec_name.to_string());
    
    // Normalize Unicode for comparison
    let normalized = base_name.nfc().collect::<String>();
    
    match normalized.as_str() {
        // Chinese variants
        "灵符" | "靈符" => InvocationLanguage::Chinese,
        
        // Japanese variants
        "霊符" | "リンゴフー" | "れいふ" => InvocationLanguage::Japanese,
        
        // Korean variants
        "영부" | "링푸" => InvocationLanguage::Korean,
        
        // Russian
        "линфу" => InvocationLanguage::Russian,
        
        // Arabic
        "لينغفو" => InvocationLanguage::Arabic,
        
        // Hindi
        "लिंगफू" => InvocationLanguage::Hindi,
        
        // Thai
        "ลิงฟู" => InvocationLanguage::Thai,
        
        // Vietnamese
        "linhphu" | "linh-phu" => InvocationLanguage::Vietnamese,
        
        // Greek
        "λινγφυ" => InvocationLanguage::Greek,
        
        // Hebrew
        "לינגפו" => InvocationLanguage::Hebrew,
        
        // German
        "lingfu-de" => InvocationLanguage::German,
        
        // French
        "lingfu-fr" => InvocationLanguage::French,
        
        // Spanish
        "lingfu-es" => InvocationLanguage::Spanish,
        
        // Turkish
        "lingfu-tr" => InvocationLanguage::Turkish,
        
        // Polish
        "lingfu-pl" => InvocationLanguage::Polish,
        
        // Default English
        _ => InvocationLanguage::English,
    }
}

#[derive(Debug, Clone, PartialEq)]
enum InvocationLanguage {
    English,
    Chinese,
    Japanese,
    Korean,
    Russian,
    Arabic,
    Hindi,
    Thai,
    Vietnamese,
    Greek,
    Hebrew,
    German,
    French,
    Spanish,
    Turkish,
    Polish,
}

impl InvocationLanguage {
    fn cli_name(&self) -> &'static str {
        match self {
            InvocationLanguage::English => "lingfu",
            InvocationLanguage::Chinese => "灵符",
            InvocationLanguage::Japanese => "霊符",
            InvocationLanguage::Korean => "영부",
            InvocationLanguage::Russian => "линфу",
            InvocationLanguage::Arabic => "لينغفو",
            InvocationLanguage::Hindi => "लिंगफू",
            InvocationLanguage::Thai => "ลิงฟู",
            InvocationLanguage::Vietnamese => "linh phù",
            InvocationLanguage::Greek => "λινγφυ",
            InvocationLanguage::Hebrew => "לינגפו",
            InvocationLanguage::German => "lingfu",
            InvocationLanguage::French => "lingfu",
            InvocationLanguage::Spanish => "lingfu",
            InvocationLanguage::Turkish => "lingfu",
            InvocationLanguage::Polish => "lingfu",
        }
    }
    
    fn commands(&self) -> CommandTranslations {
        match self {
            InvocationLanguage::English => COMMANDS_EN,
            InvocationLanguage::Chinese => COMMANDS_ZH,
            InvocationLanguage::Japanese => COMMANDS_JA,
            InvocationLanguage::Korean => COMMANDS_KO,
            InvocationLanguage::Russian => COMMANDS_RU,
            InvocationLanguage::Arabic => COMMANDS_AR,
            InvocationLanguage::Hindi => COMMANDS_HI,
            InvocationLanguage::Thai => COMMANDS_TH,
            InvocationLanguage::Vietnamese => COMMANDS_VI,
            InvocationLanguage::Greek => COMMANDS_EL,
            InvocationLanguage::Hebrew => COMMANDS_HE,
            InvocationLanguage::German => COMMANDS_DE,
            InvocationLanguage::French => COMMANDS_FR,
            InvocationLanguage::Spanish => COMMANDS_ES,
            InvocationLanguage::Turkish => COMMANDS_TR,
            InvocationLanguage::Polish => COMMANDS_PL,
        }
    }
    
    fn welcome_message(&self) -> &'static str {
        match self {
            InvocationLanguage::English => "🌊 Ling Manifest - Spirit Token Descends 🌊",
            InvocationLanguage::Chinese => "🌊 灵符降世 - 灵境之门已开 🌊",
            InvocationLanguage::Japanese => "🌊 霊符降臨 - 霊境の門開く 🌊",
            InvocationLanguage::Korean => "🌊 영부강세 - 영경의 문 열림 🌊",
            InvocationLanguage::Russian => "🌊 Линфу Нисходит - Врата Духа Открыты 🌊",
            InvocationLanguage::Arabic => "🌊 لينغفو ينزل - بوابة الروح مفتوحة 🌊",
            InvocationLanguage::Hindi => "🌊 लिंगफू उतरा - आत्मा का द्वार खुला 🌊",
            InvocationLanguage::Thai => "🌊 ลิงฟูเสด็จลง - ประตูวิญญาณเปิดแล้ว 🌊",
            InvocationLanguage::Vietnamese => "🌊 Linh Phù Giáng Thế - Cổng Linh Mở 🌊",
            InvocationLanguage::Greek => "🌊 Λινγφυ Κατεβαίνει - Η Πύλη του Πνεύματος Ανοίγει 🌊",
            InvocationLanguage::Hebrew => "🌊 לינגפו יורד - שער הרוח נפתח 🌊",
            InvocationLanguage::German => "🌊 Lingfu Steigt Hinab - Das Geistertor Öffnet Sich 🌊",
            InvocationLanguage::French => "🌊 Lingfu Descend - La Porte Spirituelle S'ouvre 🌊",
            InvocationLanguage::Spanish => "🌊 Lingfu Desciende - La Puerta Espiritual se Abre 🌊",
            InvocationLanguage::Turkish => "🌊 Lingfu İniyor - Ruh Kapısı Açılıyor 🌊",
            InvocationLanguage::Polish => "🌊 Lingfu Zstępuje - Brama Ducha Otwarta 🌊",
        }
    }
    
    fn help_text(&self) -> &'static str {
        match self {
            InvocationLanguage::English => "Ling Project Manifest Tool - Spirit Token for Your Code",
            InvocationLanguage::Chinese => "灵符 - 灵境项目管理工具",
            InvocationLanguage::Japanese => "霊符 - 霊境プロジェクト管理ツール",
            InvocationLanguage::Korean => "영부 - 영경 프로젝트 관리 도구",
            InvocationLanguage::Russian => "Линфу - Инструмент управления проектами Лин",
            InvocationLanguage::Arabic => "لينغفو - أداة إدارة مشاريع لينغ",
            InvocationLanguage::Hindi => "लिंगफू - लिंग परियोजना प्रबंधन उपकरण",
            InvocationLanguage::Thai => "ลิงฟู - เครื่องมือจัดการโปรเจคลิง",
            InvocationLanguage::Vietnamese => "Linh Phù - Công Cụ Quản Lý Dự Án Linh",
            InvocationLanguage::Greek => "Λινγφυ - Εργαλείο Διαχείρισης Έργων Ling",
            InvocationLanguage::Hebrew => "לינגפו - כלי ניהול פרויקטים של לינג",
            InvocationLanguage::German => "Lingfu - Ling Projektmanagement-Tool",
            InvocationLanguage::French => "Lingfu - Outil de Gestion de Projets Ling",
            InvocationLanguage::Spanish => "Lingfu - Herramienta de Gestión de Proyectos Ling",
            InvocationLanguage::Turkish => "Lingfu - Ling Proje Yönetim Aracı",
            InvocationLanguage::Polish => "Lingfu - Narzędzie do Zarządzania Projektami Ling",
        }
    }
    
    fn version_text(&self) -> String {
        format!("{} v2030.0.0", self.cli_name())
    }
}

// Command translations for each language (same as previous but expanded)
const COMMANDS_EN: CommandTranslations = CommandTranslations {
    new: &["new", "create", "init"],
    build: &["build", "compile", "make"],
    run: &["run", "execute", "start"],
    test: &["test", "check", "verify"],
    add: &["add", "install", "include"],
    translate: &["translate", "trans", "convert"],
    manifest: &["manifest", "show", "info"],
    wizard: &["wizard", "create", "init-wizard"],
};


const COMMANDS_ZH: CommandTranslations = CommandTranslations {
    new: &["新", "新生", "创", "创建"],
    build: &["筑", "建", "编译", "构建"],
    run: &["行", "跑", "运行", "执行"],
    test: &["试", "测", "检验", "验证"],
    add: &["添", "加", "安装", "引入"],
    translate: &["译", "翻", "转换", "翻译"],
    manifest: &["显", "现", "显示", "信息"],
    wizard: &["创灵", "向导", "引导"],
};


const COMMANDS_JA: CommandTranslations = CommandTranslations {
    new: &["新", "新生", "作成", "新規"],
    build: &["築", "建", "ビルド", "コンパイル"],
    run: &["行", "実行", "起動", "ラン"],
    test: &["試", "テスト", "検証", "確認"],
    add: &["添", "追加", "インストール", "導入"],
    translate: &["訳", "翻訳", "変換", "トランスレート"],
    manifest: &["示", "表示", "情報", "マニフェスト"],
    wizard: &["創霊", "ウィザード", "作成案内"],
};


const COMMANDS_KO: CommandTranslations = CommandTranslations {
    new: &["새", "생성", "만들기", "신규"],
    build: &["짓", "빌드", "컴파일", "구축"],
    run: &["달", "실행", "시작", "런"],
    test: &["시", "테스트", "검증", "확인"],
    add: &["더", "추가", "설치", "포함"],
    translate: &["번", "번역", "변환", "트랜슬레이트"],
    manifest: &["보", "표시", "정보", "매니페스트"],
    wizard: &["창령", "마법사", "생성도우미"],
};


const COMMANDS_RU: CommandTranslations = CommandTranslations {
    new: &["нов", "созд", "иниц"],
    build: &["сбор", "комп", "билд"],
    run: &["зап", "вып", "старт"],
    test: &["тест", "про", "пров"],
    add: &["доб", "уст", "включ"],
    translate: &["пер", "перев", "конв"],
    manifest: &["показ", "инфо", "маниф"],
    wizard: &["создДух", "мастер", "гид"],
};

const COMMANDS_AR: CommandTranslations = CommandTranslations {
    new: &["جديد", "إنشاء", "بدء"],
    build: &["بناء", "تجميع", "صنع"],
    run: &["تشغيل", "تنفيذ", "بداية"],
    test: &["اختبار", "تحقق", "تأكيد"],
    add: &["إضافة", "تثبيت", "ضمن"],
    translate: &["ترجمة", "تحويل", "نقل"],
    manifest: &["إظهار", "معلومات", "بيان"],
    wizard: &["ساحر", "دليل", "منشئ"],
};

const COMMANDS_HI: CommandTranslations = CommandTranslations {
    new: &["नया", "बनाएं", "शुरू"],
    build: &["बनाएं", "संकलन", "निर्माण"],
    run: &["चलाएं", "दौड़ाएं", "प्रारंभ"],
    test: &["परीक्षण", "जांचें", "सत्यापित"],
    add: &["जोड़ें", "स्थापित", "शामिल"],
    translate: &["अनुवाद", "रूपांतरित", "बदलें"],
    manifest: &["दिखाएं", "जानकारी", "प्रकट"],
    wizard: &["जादूगर", "मार्गदर्शक", "सृजक"],
};

const COMMANDS_TH: CommandTranslations = CommandTranslations {
    new: &["ใหม่", "สร้าง", "เริ่ม"],
    build: &["สร้าง", "คอมไพล์", "ทำ"],
    run: &["เรียก", "รัน", "เริ่มต้น"],
    test: &["ทดสอบ", "ตรวจ", "ยืนยัน"],
    add: &["เพิ่ม", "ติดตั้ง", "รวม"],
    translate: &["แปล", "แปลง", "เปลี่ยน"],
    manifest: &["แสดง", "ข้อมูล", "แจ้ง"],
    wizard: &["ตัวช่วย", "แนะนำ", "สร้าง"],
};

const COMMANDS_VI: CommandTranslations = CommandTranslations {
    new: &["mới", "tạo", "khởi_tạo"],
    build: &["xây", "biên_dịch", "dựng"],
    run: &["chạy", "thực_thi", "bắt_đầu"],
    test: &["kiểm_tra", "thử", "xác_nhận"],
    add: &["thêm", "cài", "bao_gồm"],
    translate: &["dịch", "chuyển_đổi", "biến_đổi"],
    manifest: &["hiển_thị", "thông_tin", "bày"],
    wizard: &["phù_thủy", "hướng_dẫn", "tạo"],
};

const COMMANDS_EL: CommandTranslations = CommandTranslations {
    new: &["νέο", "δημιουργία", "αρχή"],
    build: &["χτίσε", "μεταγλώττιση", "κάνε"],
    run: &["τρέξε", "εκτέλεσε", "ξεκίνα"],
    test: &["δοκίμασε", "έλεγξε", "επιβεβαίωσε"],
    add: &["πρόσθεσε", "εγκατάστησε", "συμπεριέλαβε"],
    translate: &["μετάφρασε", "μετέτρεψε", "άλλαξε"],
    manifest: &["δείξε", "πληροφορίες", "φανέρωσε"],
    wizard: &["μάγο", "οδηγό", "δημιουργέ"],
};

const COMMANDS_HE: CommandTranslations = CommandTranslations {
    new: &["חדש", "צור", "התחל"],
    build: &["בנה", "קמפל", "עשה"],
    run: &["הרץ", "בצע", "התחל"],
    test: &["בדוק", "אמת", "בחן"],
    add: &["הוסף", "התקן", "כלול"],
    translate: &["תרגם", "המר", "שנה"],
    manifest: &["הצג", "מידע", "גלה"],
    wizard: &["אשף", "מדריך", "יוצר"],
};

const COMMANDS_DE: CommandTranslations = CommandTranslations {
    new: &["neu", "erstellen", "init"],
    build: &["bauen", "kompilieren", "machen"],
    run: &["ausführen", "starten", "laufen"],
    test: &["testen", "prüfen", "überprüfen"],
    add: &["hinzufügen", "installieren", "einschließen"],
    translate: &["übersetzen", "konvertieren", "trans"],
    manifest: &["zeigen", "info", "manifest"],
    wizard: &["assistent", "führer", "ersteller"],
};

const COMMANDS_FR: CommandTranslations = CommandTranslations {
    new: &["nouveau", "créer", "init"],
    build: &["construire", "compiler", "fabriquer"],
    run: &["exécuter", "lancer", "démarrer"],
    test: &["tester", "vérifier", "contrôler"],
    add: &["ajouter", "installer", "inclure"],
    translate: &["traduire", "convertir", "trans"],
    manifest: &["afficher", "info", "manifeste"],
    wizard: &["assistant", "guide", "créateur"],
};

const COMMANDS_ES: CommandTranslations = CommandTranslations {
    new: &["nuevo", "crear", "iniciar"],
    build: &["construir", "compilar", "hacer"],
    run: &["ejecutar", "correr", "iniciar"],
    test: &["probar", "verificar", "testear"],
    add: &["agregar", "instalar", "incluir"],
    translate: &["traducir", "convertir", "trans"],
    manifest: &["mostrar", "información", "manifiesto"],
    wizard: &["asistente", "crear", "guía"],
};

const COMMANDS_TR: CommandTranslations = CommandTranslations {
    new: &["yeni", "oluştur", "başlat"],
    build: &["inşa", "derle", "yap"],
    run: &["çalıştır", "başlat", "koş"],
    test: &["test", "kontrol", "doğrula"],
    add: &["ekle", "kur", "dahil et"],
    translate: &["çevir", "dönüştür", "tercüme et"],
    manifest: &["göster", "bilgi", "manifest"],
    wizard: &["sihirbaz", "rehber", "oluşturucu"],
};

const COMMANDS_PL: CommandTranslations = CommandTranslations {
    new: &["nowy", "utwórz", "inicjuj"],
    build: &["zbuduj", "skompiluj", "zrób"],
    run: &["uruchom", "wykonaj", "start"],
    test: &["testuj", "sprawdź", "zweryfikuj"],
    add: &["dodaj", "zainstaluj", "dołącz"],
    translate: &["tłumacz", "konwertuj", "przekształć"],
    manifest: &["pokaż", "informacje", "manifest"],
    wizard: &["kreator", "przewodnik", "twórca"],
};

struct CommandTranslations {
    new: &'static [&'static str],
    build: &'static [&'static str],
    run: &'static [&'static str],
    test: &'static [&'static str],
    add: &'static [&'static str],
    translate: &'static [&'static str],
    manifest: &'static [&'static str],
    wizard: &'static [&'static str],
}


// Helper function to display in current language
fn print_in_lang(lang: &InvocationLanguage, en: &str, zh: &str, ja: &str, ko: &str) {
    match lang {
        InvocationLanguage::English => println!("{}", en),
        InvocationLanguage::Chinese => println!("{}", zh),
        InvocationLanguage::Japanese => println!("{}", ja),
        InvocationLanguage::Korean => println!("{}", ko),
        _ => println!("{}", en), // Fallback to English
    }
}

fn main() -> anyhow::Result<()> {
    let invocation_lang = detect_invocation_name();
    let translations = invocation_lang.commands();
    
    let args: Vec<String> = env::args().collect();
    
    if args.len() < 2 {
        print_help(&invocation_lang, &translations);
        return Ok(());
    }
    
    let command_str = &args[1];
    let command_args = &args[2..];
    
    // Match command across all language variants
    if translations.new.contains(&command_str.as_str()) {
        println!("{}", invocation_lang.welcome_message().cyan().bold());
        cmd_new(command_args, &invocation_lang)?;
    } else if translations.build.contains(&command_str.as_str()) {
        cmd_build(&invocation_lang)?;
    } else if translations.run.contains(&command_str.as_str()) {
        cmd_run(&invocation_lang)?;
    } else if translations.test.contains(&command_str.as_str()) {
        cmd_test(&invocation_lang)?;
    } else if translations.add.contains(&command_str.as_str()) {
        cmd_add(command_args, &invocation_lang)?;
    } else if translations.translate.contains(&command_str.as_str()) {
        cmd_translate(command_args, &invocation_lang)?;
    } else if translations.manifest.contains(&command_str.as_str()) {
        cmd_manifest(&invocation_lang)?;
    } else if translations.wizard.contains(&command_str.as_str()) {
        cmd_wizard(&invocation_lang)?;
    } else if command_str == "--version" || command_str == "-V" {
        println!("{}", invocation_lang.version_text());
    } else if command_str == "--help" || command_str == "-h" {
        print_help(&invocation_lang, &translations);
    } else {
        print_in_lang(
            &invocation_lang,
            &format!("unknown command: {}", command_str),
            &format!("未知命令: {}", command_str),
            &format!("不明なコマンド: {}", command_str),
            &format!("알 수 없는 명령: {}", command_str),
        );
    }
    
    Ok(())
}

fn print_help(lang: &InvocationLanguage, translations: &CommandTranslations) {
    let exec_name = lang.cli_name();
    
    println!("\n{}", lang.help_text().cyan().bold());
    println!("\n{}", 
        match lang {
            InvocationLanguage::English => "Usage:",
            InvocationLanguage::Chinese => "用法:",
            InvocationLanguage::Japanese => "使用法:",
            InvocationLanguage::Korean => "사용법:",
            _ => "Usage:",
        }.yellow()
    );
    
    println!("  {} {} <{}>", exec_name, translations.new[0], 
        match lang {
            InvocationLanguage::English => "NAME",
            InvocationLanguage::Chinese => "名称",
            InvocationLanguage::Japanese => "名前",
            InvocationLanguage::Korean => "이름",
            _ => "NAME",
        }
    );
    println!("  {} {}", exec_name, translations.build[0]);
    println!("  {} {}", exec_name, translations.run[0]);
    println!("  {} {}", exec_name, translations.test[0]);
    println!("  {} {} <{}>", exec_name, translations.add[0],
        match lang {
            InvocationLanguage::English => "PACKAGE",
            InvocationLanguage::Chinese => "包",
            InvocationLanguage::Japanese => "パッケージ",
            InvocationLanguage::Korean => "패키지",
            _ => "PACKAGE",
        }
    );
    println!("  {} {} --from EN --to ZH <{}>", exec_name, translations.translate[0],
        match lang {
            InvocationLanguage::English => "FILE",
            InvocationLanguage::Chinese => "文件",
            InvocationLanguage::Japanese => "ファイル",
            InvocationLanguage::Korean => "파일",
            _ => "FILE",
        }
    );
    println!("  {} {}", exec_name, translations.manifest[0]);
    println!("  {} {}", exec_name, translations.wizard[0]);
    
    println!("\n{}", 
        match lang {
            InvocationLanguage::English => "Options:",
            InvocationLanguage::Chinese => "选项:",
            InvocationLanguage::Japanese => "オプション:",
            InvocationLanguage::Korean => "옵션:",
            _ => "Options:",
        }.yellow()
    );
    println!("  --lang, -l <LANG>  Set language (en, zh, ja, ko, ru, ar, hi, th, vi, el, he, de, fr, es, tr, pl)");
    println!("  --help, -h         Show this help");
    println!("  --version, -V      Show version");
}

// Command implementations remain the same as before...
fn cmd_new(args: &[String], lang: &InvocationLanguage) -> anyhow::Result<()> {
    if args.is_empty() {
        print_in_lang(lang,
            "Please provide a project name",
            "请提供项目名称",
            "プロジェクト名を指定してください",
            "프로젝트 이름을 입력하세요",
        );
        return Ok(());
    }
    
    let name = &args[0];
    println!("{}", 
        match lang {
            InvocationLanguage::English => format!("Creating project: {}", name),
            InvocationLanguage::Chinese => format!("正在创建项目: {}", name),
            InvocationLanguage::Japanese => format!("プロジェクトを作成中: {}", name),
            InvocationLanguage::Korean => format!("프로젝트 생성 중: {}", name),
            _ => format!("Creating project: {}", name),
        }.green()
    );
    
    // Actual creation logic here
    Ok(())
}

fn cmd_build(lang: &InvocationLanguage) -> anyhow::Result<()> {
    print_in_lang(lang,
        "Building Ling project...",
        "正在筑造灵境...",
        "霊境を築造中...",
        "영경 건설 중...",
    );
    Ok(())
}

fn cmd_run(lang: &InvocationLanguage) -> anyhow::Result<()> {
    print_in_lang(lang,
        "Running Ling project...",
        "正在行灵...",
        "霊を実行中...",
        "영 실행 중...",
    );
    Ok(())
}

fn cmd_test(lang: &InvocationLanguage) -> anyhow::Result<()> {
    print_in_lang(lang,
        "Testing Ling project...",
        "正在试灵...",
        "霊をテスト中...",
        "영 테스트 중...",
    );
    Ok(())
}

fn cmd_add(args: &[String], lang: &InvocationLanguage) -> anyhow::Result<()> {
    if args.is_empty() {
        print_in_lang(lang,
            "Please specify a package to add",
            "请指定要添加的包",
            "追加するパッケージを指定してください",
            "추가할 패키지를 지정하세요",
        );
        return Ok(());
    }
    
    let package = &args[0];
    print_in_lang(lang,
        &format!("Adding package: {}", package),
        &format!("正在添包: {}", package),
        &format!("パッケージを追加中: {}", package),
        &format!("패키지 추가 중: {}", package),
    );
    Ok(())
}

fn cmd_translate(args: &[String], lang: &InvocationLanguage) -> anyhow::Result<()> {
    print_in_lang(lang,
        "Translating...",
        "正在翻译...",
        "翻訳中...",
        "번역 중...",
    );
    Ok(())
}

fn cmd_manifest(lang: &InvocationLanguage) -> anyhow::Result<()> {
    print_in_lang(lang,
        "Showing manifest...",
        "正在显示灵符...",
        "霊符を表示中...",
        "영부 표시 중...",
    );
    Ok(())
}

fn cmd_wizard(lang: &InvocationLanguage) -> anyhow::Result<()> {
    println!("{}", lang.welcome_message().magenta().bold());
    
    let name_prompt = match lang {
        InvocationLanguage::English => "Project Name:",
        InvocationLanguage::Chinese => "项目名称:",
        InvocationLanguage::Japanese => "プロジェクト名:",
        InvocationLanguage::Korean => "프로젝트 이름:",
        _ => "Project Name:",
    };
    
    let name: String = Input::new().with_prompt(name_prompt).interact_text()?;
    
    let type_options = match lang {
        InvocationLanguage::English => vec![
            "Dú Xíng - Standalone Binary",
            "Gòng Xiū - Library", 
            "Yóu Líng - Game",
            "Xiǎn Líng - UI Application",
            "Zhì Líng - AI/ML Project",
            "Mì Líng - Cryptography",
            "Wǎng Líng - Web/WASM",
            "Wàn Yán - Polyglot Lexicon",
        ],
        InvocationLanguage::Chinese => vec![
            "独行 - 独立可执行程序",
            "共修 - 库",
            "游灵 - 游戏",
            "显灵 - UI应用",
            "智灵 - AI/ML项目",
            "密灵 - 密码学",
            "网灵 - Web/WASM",
            "万言 - 多语言词典",
        ],
        _ => vec![
            "Binary", "Library", "Game", "UI", "AI/ML", "Crypto", "Web/WASM", "Polyglot",
        ],
    };
    
    let selection = Select::new()
        .with_prompt(match lang {
            InvocationLanguage::English => "Spirit Realm Type:",
            InvocationLanguage::Chinese => "灵境类型:",
            InvocationLanguage::Japanese => "霊境タイプ:",
            InvocationLanguage::Korean => "영경 유형:",
            _ => "Project Type:",
        })
        .items(&type_options)
        .default(0)
        .interact()?;
    
    println!("{}", 
        match lang {
            InvocationLanguage::English => format!("Creating {} project: {}", type_options[selection], name),
            InvocationLanguage::Chinese => format!("正在创建{}项目: {}", type_options[selection], name),
            _ => format!("Creating project: {}", name),
        }.green()
    );
    
    Ok(())
}