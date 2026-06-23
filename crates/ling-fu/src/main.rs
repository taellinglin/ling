use colored::*;
use dialoguer::{Input, Select};
use std::env;

mod normalize;
mod scaffold;
use scaffold::{ProjectKind, ProjectScaffold, ScaffoldLang, dir_names};

// ─── Invocation language detection ───────────────────────────────────────────

fn detect_invocation_name() -> InvocationLanguage {
    let args: Vec<String> = env::args().collect();
    let exec_name = args[0].as_str();
    let base_name = std::path::Path::new(exec_name)
        .file_stem()
        .map(|s| s.to_string_lossy().to_string())
        .unwrap_or_else(|| exec_name.to_string());

    match base_name.as_str() {
        "灵符" | "靈符" => InvocationLanguage::Chinese,
        "霊符" | "リンゴフー" | "れいふ" => InvocationLanguage::Japanese,
        "영부" | "링푸" => InvocationLanguage::Korean,
        "линфу" => InvocationLanguage::Russian,
        "لينغفو" => InvocationLanguage::Arabic,
        "लिंगफू" => InvocationLanguage::Hindi,
        "ลิงฟู" => InvocationLanguage::Thai,
        "linhphu" | "linh-phu" => InvocationLanguage::Vietnamese,
        "λινγφυ" => InvocationLanguage::Greek,
        "לינגפו" => InvocationLanguage::Hebrew,
        "lingfu-de" => InvocationLanguage::German,
        "lingfu-fr" => InvocationLanguage::French,
        "lingfu-es" => InvocationLanguage::Spanish,
        "lingfu-tr" => InvocationLanguage::Turkish,
        "lingfu-pl" => InvocationLanguage::Polish,
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

    fn commands(&self) -> &'static CommandTranslations {
        match self {
            InvocationLanguage::English => &COMMANDS_EN,
            InvocationLanguage::Chinese => &COMMANDS_ZH,
            InvocationLanguage::Japanese => &COMMANDS_JA,
            InvocationLanguage::Korean => &COMMANDS_KO,
            InvocationLanguage::Russian => &COMMANDS_RU,
            InvocationLanguage::Arabic => &COMMANDS_AR,
            InvocationLanguage::Hindi => &COMMANDS_HI,
            InvocationLanguage::Thai => &COMMANDS_TH,
            InvocationLanguage::Vietnamese => &COMMANDS_VI,
            InvocationLanguage::Greek => &COMMANDS_EL,
            InvocationLanguage::Hebrew => &COMMANDS_HE,
            InvocationLanguage::German => &COMMANDS_DE,
            InvocationLanguage::French => &COMMANDS_FR,
            InvocationLanguage::Spanish => &COMMANDS_ES,
            InvocationLanguage::Turkish => &COMMANDS_TR,
            InvocationLanguage::Polish => &COMMANDS_PL,
        }
    }

    fn welcome_message(&self) -> &'static str {
        match self {
            InvocationLanguage::English => "~ Ling Manifest — Spirit Token Descends ~",
            InvocationLanguage::Chinese => "~ 灵符降世 — 灵境之门已开 ~",
            InvocationLanguage::Japanese => "~ 霊符降臨 — 霊境の門開く ~",
            InvocationLanguage::Korean => "~ 영부강세 — 영경의 문 열림 ~",
            InvocationLanguage::Russian => "~ Линфу Нисходит — Врата Духа Открыты ~",
            InvocationLanguage::Arabic => "~ لينغفو ينزل — بوابة الروح مفتوحة ~",
            InvocationLanguage::Hindi => "~ लिंगफू उतरा — आत्मा का द्वार खुला ~",
            InvocationLanguage::Thai => "~ ลิงฟูเสด็จลง — ประตูวิญญาณเปิดแล้ว ~",
            InvocationLanguage::Vietnamese => "~ Linh Phù Giáng Thế — Cổng Linh Mở ~",
            InvocationLanguage::Greek => "~ Λινγφυ Κατεβαίνει — Η Πύλη του Πνεύματος Ανοίγει ~",
            InvocationLanguage::Hebrew => "~ לינגפו יורד — שער הרוח נפתח ~",
            InvocationLanguage::German => "~ Lingfu Steigt Hinab — Das Geistertor Öffnet Sich ~",
            InvocationLanguage::French => "~ Lingfu Descend — La Porte Spirituelle S'ouvre ~",
            InvocationLanguage::Spanish => "~ Lingfu Desciende — La Puerta Espiritual se Abre ~",
            InvocationLanguage::Turkish => "~ Lingfu İniyor — Ruh Kapısı Açılıyor ~",
            InvocationLanguage::Polish => "~ Lingfu Zstępuje — Brama Ducha Otwarta ~",
        }
    }

    fn help_text(&self) -> &'static str {
        match self {
            InvocationLanguage::English => {
                "Ling Project Manifest Tool — Spirit Token for Your Code"
            },
            InvocationLanguage::Chinese => "灵符 — 灵境项目管理工具",
            InvocationLanguage::Japanese => "霊符 — 霊境プロジェクト管理ツール",
            InvocationLanguage::Korean => "영부 — 영경 프로젝트 관리 도구",
            InvocationLanguage::Russian => "Линфу — Инструмент управления проектами Лин",
            InvocationLanguage::Arabic => "لينغفو — أداة إدارة مشاريع لينغ",
            InvocationLanguage::Hindi => "लिंगफू — लिंग परियोजना प्रबंधन उपकरण",
            InvocationLanguage::Thai => "ลิงฟู — เครื่องมือจัดการโปรเจคลิง",
            InvocationLanguage::Vietnamese => "Linh Phù — Công Cụ Quản Lý Dự Án Linh",
            InvocationLanguage::Greek => "Λινγφυ — Εργαλείο Διαχείρισης Έργων Ling",
            InvocationLanguage::Hebrew => "לינגפו — כלי ניהול פרויקטים של לינג",
            InvocationLanguage::German => "Lingfu — Ling Projektmanagement-Tool",
            InvocationLanguage::French => "Lingfu — Outil de Gestion de Projets Ling",
            InvocationLanguage::Spanish => "Lingfu — Herramienta de Gestión de Proyectos Ling",
            InvocationLanguage::Turkish => "Lingfu — Ling Proje Yönetim Aracı",
            InvocationLanguage::Polish => "Lingfu — Narzędzie do Zarządzania Projektami Ling",
        }
    }

    fn version_text(&self) -> String {
        format!("{} v2030.0.0", self.cli_name())
    }
}

// ─── Command translation tables ───────────────────────────────────────────────

struct CommandTranslations {
    new: &'static [&'static str],
    init: &'static [&'static str],
    build: &'static [&'static str],
    run: &'static [&'static str],
    test: &'static [&'static str],
    check: &'static [&'static str],
    clean: &'static [&'static str],
    doc: &'static [&'static str],
    fmt: &'static [&'static str],
    tree: &'static [&'static str],
    bench: &'static [&'static str],
    update: &'static [&'static str],
    add: &'static [&'static str],
    search: &'static [&'static str],
    publish: &'static [&'static str],
    login: &'static [&'static str],
    logout: &'static [&'static str],
    translate: &'static [&'static str],
    manifest: &'static [&'static str],
    wizard: &'static [&'static str],
    normalize: &'static [&'static str],
}

// English
const COMMANDS_EN: CommandTranslations = CommandTranslations {
    new: &["new", "create"],
    init: &["init", "initialize"],
    build: &["build", "compile", "make"],
    run: &["run", "execute", "start"],
    test: &["test", "verify"],
    check: &["check", "lint", "validate"],
    clean: &["clean", "purge", "clear"],
    doc: &["doc", "docs", "document"],
    fmt: &["fmt", "format", "style"],
    tree: &["tree", "structure", "ls"],
    bench: &["bench", "benchmark", "measure"],
    update: &["update", "upgrade", "refresh"],
    add: &["add", "install", "include"],
    search: &["search", "find", "seek"],
    publish: &["publish", "release", "share"],
    login: &["login", "sign-in", "auth"],
    logout: &["logout", "sign-out", "deauth"],
    translate: &["translate", "trans", "convert"],
    manifest: &["manifest", "show", "info"],
    wizard: &["wizard", "guide", "create-wizard"],
    normalize: &["normalize", "norm", "localize"],
};

// Chinese (中文)
const COMMANDS_ZH: CommandTranslations = CommandTranslations {
    new: &["新", "新生", "创", "创建"],
    init: &["初", "初始", "初始化"],
    build: &["筑", "建", "编译", "构建"],
    run: &["行", "跑", "运行", "执行"],
    test: &["试", "测", "检验", "验证"],
    check: &["查", "核查", "检查", "验"],
    clean: &["清", "净化", "清洁", "扫"],
    doc: &["文", "文档", "说明", "典"],
    fmt: &["整", "格式", "整理", "化"],
    tree: &["树", "结构", "列"],
    bench: &["衡", "基准", "测速", "量"],
    update: &["新", "更新", "升级", "刷新"],
    add: &["添", "加", "安装", "引入"],
    search: &["寻", "搜", "查找", "探"],
    publish: &["发", "发布", "传世", "出版"],
    login: &["入", "登入", "认证"],
    logout: &["出", "退出", "登出"],
    translate: &["译", "翻", "转换", "翻译"],
    manifest: &["显", "现", "显示", "信息"],
    wizard: &["创灵", "向导", "引导"],
    normalize: &["规范", "规范化", "本地化"],
};

// Japanese (日本語)
const COMMANDS_JA: CommandTranslations = CommandTranslations {
    new: &["新", "新生", "作成", "新規"],
    init: &["初", "初期化", "はじめ"],
    build: &["築", "建", "ビルド", "コンパイル"],
    run: &["行", "実行", "起動", "ラン"],
    test: &["試", "テスト", "検証", "確認"],
    check: &["査", "チェック", "検査", "確"],
    clean: &["清", "浄化", "クリーン", "掃"],
    doc: &["文", "ドキュメント", "説明", "典"],
    fmt: &["整", "フォーマット", "整理"],
    tree: &["木", "構造", "ツリー", "一覧"],
    bench: &["測", "ベンチ", "計測", "速"],
    update: &["更", "アップデート", "更新", "刷"],
    add: &["添", "追加", "インストール", "導入"],
    search: &["探", "検索", "求", "サーチ"],
    publish: &["発", "公開", "リリース", "発布"],
    login: &["入", "ログイン", "認証"],
    logout: &["出", "ログアウト", "退出"],
    translate: &["訳", "翻訳", "変換", "トランスレート"],
    manifest: &["示", "表示", "情報", "マニフェスト"],
    wizard: &["創霊", "ウィザード", "作成案内"],
    normalize: &["正規化", "言語統一", "ローカライズ"],
};

// Korean (한국어)
const COMMANDS_KO: CommandTranslations = CommandTranslations {
    new: &["새", "생성", "만들기", "신규"],
    init: &["초기화", "초기", "시작"],
    build: &["짓", "빌드", "컴파일", "구축"],
    run: &["달", "실행", "시작", "런"],
    test: &["시", "테스트", "검증", "확인"],
    check: &["검", "체크", "검사", "확"],
    clean: &["청", "정화", "클린", "청소"],
    doc: &["문", "문서", "설명", "전"],
    fmt: &["정", "형식", "정리"],
    tree: &["트리", "구조", "목록"],
    bench: &["측", "벤치", "측정", "속"],
    update: &["갱신", "업데이트", "새로고침"],
    add: &["더", "추가", "설치", "포함"],
    search: &["찾", "검색", "탐", "서치"],
    publish: &["발", "게시", "배포", "공개"],
    login: &["로그인", "인증", "입"],
    logout: &["로그아웃", "퇴장", "출"],
    translate: &["번", "번역", "변환", "트랜슬레이션"],
    manifest: &["보", "표시", "정보", "매니페스트"],
    wizard: &["창령", "마법사", "생성도우미"],
    normalize: &["정규화", "언어통일", "로컬라이즈"],
};

// Russian (русский)
const COMMANDS_RU: CommandTranslations = CommandTranslations {
    new: &["нов", "созд", "создать"],
    init: &["иниц", "инициализировать", "начать"],
    build: &["сбор", "комп", "билд"],
    run: &["зап", "вып", "старт"],
    test: &["тест", "про", "пров"],
    check: &["проверить", "проверка", "провер"],
    clean: &["очист", "чист", "убрать"],
    doc: &["докум", "документация", "доки"],
    fmt: &["форм", "формат", "оформить"],
    tree: &["дер", "структура", "дерево"],
    bench: &["тест_скор", "замер", "бенч"],
    update: &["обнов", "обновить", "апдейт"],
    add: &["доб", "уст", "включ"],
    search: &["поиск", "искать", "найти"],
    publish: &["публ", "опубл", "издать"],
    login: &["вход", "войти", "авт"],
    logout: &["выход", "выйти"],
    translate: &["пер", "перев", "конв"],
    manifest: &["показ", "инфо", "маниф"],
    wizard: &["создДух", "мастер", "гид"],
    normalize: &["нормализовать", "норм", "локализовать"],
};

// Arabic (العربية)
const COMMANDS_AR: CommandTranslations = CommandTranslations {
    new: &["جديد", "إنشاء", "بدء"],
    init: &["تهيئة", "مبادئ", "ابدأ"],
    build: &["بناء", "تجميع", "صنع"],
    run: &["تشغيل", "تنفيذ", "بداية"],
    test: &["اختبار", "تحقق", "تأكيد"],
    check: &["فحص", "مراجعة", "تدقيق"],
    clean: &["تنظيف", "إزالة", "مسح"],
    doc: &["توثيق", "وثائق", "شرح"],
    fmt: &["تنسيق", "ترتيب", "صياغة"],
    tree: &["شجرة", "هيكل", "عرض"],
    bench: &["قياس", "سرعة", "أداء"],
    update: &["تحديث", "تطوير", "تحديث"],
    add: &["إضافة", "تثبيت", "ضمن"],
    search: &["بحث", "إيجاد", "استعلام"],
    publish: &["نشر", "إصدار", "توزيع"],
    login: &["تسجيل_دخول", "دخول", "مصادقة"],
    logout: &["تسجيل_خروج", "خروج"],
    translate: &["ترجمة", "تحويل", "نقل"],
    manifest: &["إظهار", "معلومات", "بيان"],
    wizard: &["ساحر", "دليل", "منشئ"],
    normalize: &["توحيد", "تطبيع", "توطين"],
};

// Hindi (हिन्दी)
const COMMANDS_HI: CommandTranslations = CommandTranslations {
    new: &["नया", "बनाएं", "शुरू"],
    init: &["प्रारंभ", "शुरुआत", "आरंभ"],
    build: &["बनाएं", "संकलन", "निर्माण"],
    run: &["चलाएं", "दौड़ाएं", "प्रारंभ"],
    test: &["परीक्षण", "जांचें", "सत्यापित"],
    check: &["जांच", "परख", "सत्यापन"],
    clean: &["साफ", "शुद्ध", "सफाई"],
    doc: &["दस्तावेज", "प्रलेखन", "टिप्पणी"],
    fmt: &["स्वरूप", "प्रारूप", "सजाएं"],
    tree: &["वृक्ष", "संरचना", "सूची"],
    bench: &["माप", "गति", "बेंचमार्क"],
    update: &["अद्यतन", "अपडेट", "ताजा"],
    add: &["जोड़ें", "स्थापित", "शामिल"],
    search: &["खोजें", "ढूंढें", "तलाश"],
    publish: &["प्रकाशित", "जारी", "वितरित"],
    login: &["लॉगिन", "प्रवेश", "सत्यापित"],
    logout: &["लॉगआउट", "निकास"],
    translate: &["अनुवाद", "रूपांतरित", "बदलें"],
    manifest: &["दिखाएं", "जानकारी", "प्रकट"],
    wizard: &["जादूगर", "मार्गदर्शक", "सृजक"],
    normalize: &["सामान्यीकरण", "नॉर्म", "स्थानीयकरण"],
};

// Thai (ภาษาไทย)
const COMMANDS_TH: CommandTranslations = CommandTranslations {
    new: &["ใหม่", "สร้าง", "เริ่ม"],
    init: &["เริ่มต้น", "กำหนดค่า", "ตั้งค่า"],
    build: &["สร้าง", "คอมไพล์", "ทำ"],
    run: &["เรียก", "รัน", "เริ่มต้น"],
    test: &["ทดสอบ", "ตรวจ", "ยืนยัน"],
    check: &["ตรวจสอบ", "ตรวจ", "ยืนยัน"],
    clean: &["ล้าง", "ทำความสะอาด", "ลบ"],
    doc: &["เอกสาร", "คำอธิบาย", "ไฟล์ช่วย"],
    fmt: &["จัดรูปแบบ", "จัด", "ปรับ"],
    tree: &["ต้นไม้", "โครงสร้าง", "แสดง"],
    bench: &["วัดความเร็ว", "ทดสอบ", "วัด"],
    update: &["อัปเดต", "อัพเกรด", "ปรับปรุง"],
    add: &["เพิ่ม", "ติดตั้ง", "รวม"],
    search: &["ค้นหา", "หา", "สืบค้น"],
    publish: &["เผยแพร่", "ปล่อย", "แจกจ่าย"],
    login: &["เข้าสู่ระบบ", "ลงชื่อเข้า", "ยืนยัน"],
    logout: &["ออกจากระบบ", "ลงชื่อออก"],
    translate: &["แปล", "แปลง", "เปลี่ยน"],
    manifest: &["แสดง", "ข้อมูล", "แจ้ง"],
    wizard: &["ตัวช่วย", "แนะนำ", "สร้าง"],
    normalize: &["ปรับภาษา", "ปรับ", "แปลงภาษา"],
};

// Vietnamese (Tiếng Việt)
const COMMANDS_VI: CommandTranslations = CommandTranslations {
    new: &["mới", "tạo", "khởi_tạo"],
    init: &["khởi_động", "khởi", "bắt_đầu"],
    build: &["xây", "biên_dịch", "dựng"],
    run: &["chạy", "thực_thi", "bắt_đầu"],
    test: &["kiểm_tra", "thử", "xác_nhận"],
    check: &["kiểm", "xác", "tra"],
    clean: &["dọn", "xóa", "sạch"],
    doc: &["tài_liệu", "giải_thích", "hướng_dẫn"],
    fmt: &["định_dạng", "sắp_xếp", "chuẩn"],
    tree: &["cây", "cấu_trúc", "danh_sách"],
    bench: &["đo", "thử_tốc_độ", "hiệu_suất"],
    update: &["cập_nhật", "nâng_cấp", "làm_mới"],
    add: &["thêm", "cài", "bao_gồm"],
    search: &["tìm", "tìm_kiếm", "tra_cứu"],
    publish: &["xuất_bản", "phát_hành", "chia_sẻ"],
    login: &["đăng_nhập", "vào", "xác_thực"],
    logout: &["đăng_xuất", "ra"],
    translate: &["dịch", "chuyển_đổi", "biến_đổi"],
    manifest: &["hiển_thị", "thông_tin", "bày"],
    wizard: &["phù_thủy", "hướng_dẫn", "tạo"],
    normalize: &["chuẩn_hóa", "chuẩn", "bản_địa_hóa"],
};

// Greek (Ελληνικά)
const COMMANDS_EL: CommandTranslations = CommandTranslations {
    new: &["νέο", "δημιουργία", "αρχή"],
    init: &["αρχικοποίηση", "εκκίνηση", "έναρξη"],
    build: &["χτίσε", "μεταγλώττιση", "κάνε"],
    run: &["τρέξε", "εκτέλεσε", "ξεκίνα"],
    test: &["δοκίμασε", "έλεγξε", "επιβεβαίωσε"],
    check: &["έλεγχος", "σκανάρισε", "επαλήθευση"],
    clean: &["καθάρισε", "διέγραψε", "σκούπισε"],
    doc: &["τεκμηρίωση", "εγγραφή", "βοήθεια"],
    fmt: &["μορφοποίηση", "τακτοποίηση", "στυλ"],
    tree: &["δέντρο", "δομή", "λίστα"],
    bench: &["μέτρηση", "ταχύτητα", "απόδοση"],
    update: &["ενημέρωση", "αναβάθμιση", "ανανέωση"],
    add: &["πρόσθεσε", "εγκατάστησε", "συμπεριέλαβε"],
    search: &["αναζήτηση", "βρες", "ανακάλυψε"],
    publish: &["δημοσίευσε", "κυκλοφόρησε", "μοιράσου"],
    login: &["σύνδεση", "είσοδος", "πιστοποίηση"],
    logout: &["αποσύνδεση", "έξοδος"],
    translate: &["μετάφρασε", "μετέτρεψε", "άλλαξε"],
    manifest: &["δείξε", "πληροφορίες", "φανέρωσε"],
    wizard: &["μάγο", "οδηγό", "δημιουργέ"],
    normalize: &["κανονικοποίηση", "νόρμ", "τοπικοποίηση"],
};

// Hebrew (עברית)
const COMMANDS_HE: CommandTranslations = CommandTranslations {
    new: &["חדש", "צור", "התחל"],
    init: &["אתחל", "הגדר", "הכן"],
    build: &["בנה", "קמפל", "עשה"],
    run: &["הרץ", "בצע", "התחל"],
    test: &["בדוק", "אמת", "בחן"],
    check: &["בדיקה", "סריקה", "אימות"],
    clean: &["נקה", "מחק", "טהר"],
    doc: &["תיעוד", "הסבר", "עזרה"],
    fmt: &["עיצוב", "סידור", "פרמט"],
    tree: &["עץ", "מבנה", "רשימה"],
    bench: &["מדידה", "מהירות", "ביצועים"],
    update: &["עדכן", "שדרג", "רענן"],
    add: &["הוסף", "התקן", "כלול"],
    search: &["חפש", "מצא", "חקור"],
    publish: &["פרסם", "שחרר", "הפץ"],
    login: &["התחבר", "כניסה", "אמת"],
    logout: &["התנתק", "יציאה"],
    translate: &["תרגם", "המר", "שנה"],
    manifest: &["הצג", "מידע", "גלה"],
    wizard: &["אשף", "מדריך", "יוצר"],
    normalize: &["נרמול", "תקנן", "לוקליזציה"],
};

// German (Deutsch)
const COMMANDS_DE: CommandTranslations = CommandTranslations {
    new: &["neu", "erstellen", "anlegen"],
    init: &["init", "initialisieren", "einrichten"],
    build: &["bauen", "kompilieren", "machen"],
    run: &["ausführen", "starten", "laufen"],
    test: &["testen", "prüfen", "überprüfen"],
    check: &["prüfen", "checken", "validieren"],
    clean: &["bereinigen", "säubern", "löschen"],
    doc: &["dokumentieren", "dokumente", "doku"],
    fmt: &["formatieren", "ordnen", "stilisieren"],
    tree: &["baum", "struktur", "liste"],
    bench: &["messen", "benchmark", "leistung"],
    update: &["aktualisieren", "erneuern", "upgraden"],
    add: &["hinzufügen", "installieren", "einschließen"],
    search: &["suchen", "finden", "erkunden"],
    publish: &["veröffentlichen", "freigeben", "teilen"],
    login: &["einloggen", "anmelden", "auth"],
    logout: &["ausloggen", "abmelden"],
    translate: &["übersetzen", "konvertieren", "trans"],
    manifest: &["zeigen", "info", "manifest"],
    wizard: &["assistent", "führer", "ersteller"],
    normalize: &["normalisieren", "norm", "lokalisieren"],
};

// French (Français)
const COMMANDS_FR: CommandTranslations = CommandTranslations {
    new: &["nouveau", "créer", "init"],
    init: &["initialiser", "configurer", "préparer"],
    build: &["construire", "compiler", "fabriquer"],
    run: &["exécuter", "lancer", "démarrer"],
    test: &["tester", "vérifier", "contrôler"],
    check: &["vérifier", "valider", "analyser"],
    clean: &["nettoyer", "purger", "effacer"],
    doc: &["documenter", "documentation", "docs"],
    fmt: &["formater", "ordonner", "styliser"],
    tree: &["arbre", "structure", "liste"],
    bench: &["mesurer", "benchmark", "performance"],
    update: &["mettre_a_jour", "actualiser", "upgrader"],
    add: &["ajouter", "installer", "inclure"],
    search: &["rechercher", "trouver", "chercher"],
    publish: &["publier", "diffuser", "partager"],
    login: &["connexion", "authentification", "entrer"],
    logout: &["déconnexion", "sortir"],
    translate: &["traduire", "convertir", "trans"],
    manifest: &["afficher", "info", "manifeste"],
    wizard: &["assistant", "guide", "créateur"],
    normalize: &["normaliser", "norm", "localiser"],
};

// Spanish (Español)
const COMMANDS_ES: CommandTranslations = CommandTranslations {
    new: &["nuevo", "crear", "iniciar"],
    init: &["inicializar", "configurar", "preparar"],
    build: &["construir", "compilar", "hacer"],
    run: &["ejecutar", "correr", "iniciar"],
    test: &["probar", "verificar", "testear"],
    check: &["verificar", "validar", "analizar"],
    clean: &["limpiar", "purgar", "borrar"],
    doc: &["documentar", "documentación", "docs"],
    fmt: &["formatear", "ordenar", "estilizar"],
    tree: &["árbol", "estructura", "lista"],
    bench: &["medir", "benchmark", "rendimiento"],
    update: &["actualizar", "renovar", "mejorar"],
    add: &["agregar", "instalar", "incluir"],
    search: &["buscar", "encontrar", "explorar"],
    publish: &["publicar", "lanzar", "compartir"],
    login: &["iniciar_sesión", "autenticar", "entrar"],
    logout: &["cerrar_sesión", "salir"],
    translate: &["traducir", "convertir", "trans"],
    manifest: &["mostrar", "información", "manifiesto"],
    wizard: &["asistente", "crear", "guía"],
    normalize: &["normalizar", "norm", "localizar"],
};

// Turkish (Türkçe)
const COMMANDS_TR: CommandTranslations = CommandTranslations {
    new: &["yeni", "oluştur", "başlat"],
    init: &["başlat", "hazırla", "kur"],
    build: &["inşa", "derle", "yap"],
    run: &["çalıştır", "başlat", "koş"],
    test: &["test", "kontrol", "doğrula"],
    check: &["denetle", "kontrol_et", "doğrula"],
    clean: &["temizle", "sil", "arıt"],
    doc: &["belgele", "belge", "yardım"],
    fmt: &["biçimlendir", "düzenle", "stilize"],
    tree: &["ağaç", "yapı", "liste"],
    bench: &["ölç", "kıyasla", "hız"],
    update: &["güncelle", "yükselt", "tazele"],
    add: &["ekle", "kur", "dahil_et"],
    search: &["ara", "bul", "keşfet"],
    publish: &["yayınla", "dağıt", "paylaş"],
    login: &["giriş", "oturum_aç", "doğrula"],
    logout: &["çıkış", "oturumu_kapat"],
    translate: &["çevir", "dönüştür", "tercüme_et"],
    manifest: &["göster", "bilgi", "manifest"],
    wizard: &["sihirbaz", "rehber", "oluşturucu"],
    normalize: &["normalleştir", "norm", "yerelleştir"],
};

// Polish (Polski)
const COMMANDS_PL: CommandTranslations = CommandTranslations {
    new: &["nowy", "utwórz", "inicjuj"],
    init: &["inicjalizuj", "skonfiguruj", "przygotuj"],
    build: &["zbuduj", "skompiluj", "zrób"],
    run: &["uruchom", "wykonaj", "start"],
    test: &["testuj", "sprawdź", "zweryfikuj"],
    check: &["sprawdź", "zweryfikuj", "zbadaj"],
    clean: &["wyczyść", "usuń", "oczyść"],
    doc: &["dokumentuj", "dokumentacja", "pomoc"],
    fmt: &["formatuj", "ułóż", "ostyluj"],
    tree: &["drzewo", "struktura", "lista"],
    bench: &["mierz", "benchmark", "wydajność"],
    update: &["aktualizuj", "uaktualnij", "odśwież"],
    add: &["dodaj", "zainstaluj", "dołącz"],
    search: &["szukaj", "znajdź", "przeszukaj"],
    publish: &["opublikuj", "wydaj", "udostępnij"],
    login: &["zaloguj", "wejdź", "uwierzytelnij"],
    logout: &["wyloguj", "wyjdź"],
    translate: &["tłumacz", "konwertuj", "przekształć"],
    manifest: &["pokaż", "informacje", "manifest"],
    wizard: &["kreator", "przewodnik", "twórca"],
    normalize: &["normalizuj", "norm", "lokalizuj"],
};

// ─── Localized string helpers ─────────────────────────────────────────────────

fn t<'a>(
    lang: &InvocationLanguage,
    en: &'a str,
    zh: &'a str,
    ja: &'a str,
    ko: &'a str,
    th: &'a str,
    other: &'a str,
) -> &'a str {
    match lang {
        InvocationLanguage::English => en,
        InvocationLanguage::Chinese => zh,
        InvocationLanguage::Japanese => ja,
        InvocationLanguage::Korean => ko,
        InvocationLanguage::Thai => th,
        _ => other,
    }
}

fn say(lang: &InvocationLanguage, en: &str, zh: &str, ja: &str, ko: &str) {
    match lang {
        InvocationLanguage::English => println!("{}", en),
        InvocationLanguage::Chinese => println!("{}", zh),
        InvocationLanguage::Japanese => println!("{}", ja),
        InvocationLanguage::Korean => println!("{}", ko),
        _ => println!("{}", en),
    }
}

// ─── Manifest discovery ───────────────────────────────────────────────────────

const MANIFEST_NAMES: &[&str] = &[
    "Ling.toml",
    "灵符.toml",
    "霊符.toml",
    "영부.toml",
    "ลิงฟู.toml",
];

/// Walk up the directory tree to find a Ling manifest file.
fn find_manifest() -> Option<std::path::PathBuf> {
    let mut dir = std::env::current_dir().ok()?;
    loop {
        for name in MANIFEST_NAMES {
            let candidate = dir.join(name);
            if candidate.exists() {
                return Some(candidate);
            }
        }
        if !dir.pop() {
            break;
        }
    }
    None
}

/// Detect which ScaffoldLang a manifest filename implies.
fn lang_from_manifest(path: &std::path::Path) -> ScaffoldLang {
    let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("");
    match name {
        "灵符.toml" => ScaffoldLang::Chinese,
        "霊符.toml" => ScaffoldLang::Japanese,
        "영부.toml" => ScaffoldLang::Korean,
        "ลิงฟู.toml" => ScaffoldLang::Thai,
        _ => ScaffoldLang::English,
    }
}

// ─── main ─────────────────────────────────────────────────────────────────────

fn main() -> anyhow::Result<()> {
    let invocation_lang = detect_invocation_name();
    let tr = invocation_lang.commands();

    let args: Vec<String> = env::args().collect();

    if args.len() < 2 {
        print_help(&invocation_lang, tr);
        return Ok(());
    }

    let cmd = &args[1];
    let rest = &args[2..];

    macro_rules! matches_cmd {
        ($field:expr) => {
            $field.contains(&cmd.as_str())
        };
    }

    if cmd == "convert" || cmd == "转换" || cmd == "変換" || cmd == "변환" || cmd == "แปลง"
    {
        // Asset → .ling transcoding lives in the `ling` binary (it owns the
        // glTF/audio/MIDI parsers); forward verbatim. Checked before `translate`
        // because "convert" is also a translate alias.
        let ling_bin = find_ling_binary();
        let status = std::process::Command::new(&ling_bin)
            .arg("convert")
            .args(rest)
            .status()
            .map_err(|e| anyhow::anyhow!("ling: {e}"))?;
        std::process::exit(status.code().unwrap_or(1));
    } else if cmd == "ast" || cmd == "图谱" || cmd == "図譜" || cmd == "도표" || cmd == "ผังต้นไม้"
    {
        // Project-wide AST → SVG. The renderer lives in the `ling` binary (it owns
        // the parser). Treat the whole project as one program: pass the project
        // root (manifest parent, else cwd) unless the user supplied a path.
        let ling_bin = find_ling_binary();
        let has_path = {
            let mut i = 0;
            let mut found = false;
            while i < rest.len() {
                if rest[i] == "--out" {
                    i += 2;
                    continue;
                }
                if rest[i].starts_with('-') {
                    i += 1;
                    continue;
                }
                found = true;
                break;
            }
            found
        };
        let root = find_manifest()
            .and_then(|m| m.parent().map(|p| p.to_path_buf()))
            .unwrap_or_else(|| std::path::PathBuf::from("."));
        let mut command = std::process::Command::new(&ling_bin);
        command.arg("ast");
        if !has_path {
            command.arg(&root);
        }
        let status = command
            .args(rest)
            .status()
            .map_err(|e| anyhow::anyhow!("ling: {e}"))?;
        std::process::exit(status.code().unwrap_or(1));
    } else if matches_cmd!(tr.new) {
        println!("{}", invocation_lang.welcome_message().cyan().bold());
        cmd_new(rest, &invocation_lang)?;
    } else if matches_cmd!(tr.init) {
        println!("{}", invocation_lang.welcome_message().cyan().bold());
        cmd_init(rest, &invocation_lang)?;
    } else if matches_cmd!(tr.build) {
        cmd_build(rest, &invocation_lang)?;
    } else if matches_cmd!(tr.run) {
        cmd_run(rest, &invocation_lang)?;
    } else if matches_cmd!(tr.test) {
        cmd_test(rest, &invocation_lang)?;
    } else if matches_cmd!(tr.check) {
        cmd_check(rest, &invocation_lang)?;
    } else if matches_cmd!(tr.clean) {
        cmd_clean(&invocation_lang)?;
    } else if matches_cmd!(tr.doc) {
        cmd_doc(&invocation_lang)?;
    } else if matches_cmd!(tr.fmt) {
        cmd_fmt(rest, &invocation_lang)?;
    } else if matches_cmd!(tr.tree) {
        cmd_tree(&invocation_lang)?;
    } else if matches_cmd!(tr.bench) {
        cmd_bench(rest, &invocation_lang)?;
    } else if matches_cmd!(tr.update) {
        cmd_update(rest, &invocation_lang)?;
    } else if matches_cmd!(tr.add) {
        cmd_add(rest, &invocation_lang)?;
    } else if matches_cmd!(tr.search) {
        cmd_search(rest, &invocation_lang)?;
    } else if matches_cmd!(tr.publish) {
        cmd_publish(rest, &invocation_lang)?;
    } else if matches_cmd!(tr.login) {
        cmd_login(rest, &invocation_lang)?;
    } else if matches_cmd!(tr.logout) {
        cmd_logout(&invocation_lang)?;
    } else if matches_cmd!(tr.translate) {
        cmd_translate(rest, &invocation_lang)?;
    } else if matches_cmd!(tr.manifest) {
        cmd_manifest(&invocation_lang)?;
    } else if matches_cmd!(tr.wizard) {
        cmd_wizard(&invocation_lang)?;
    } else if matches_cmd!(tr.normalize) {
        cmd_normalize(rest, &invocation_lang)?;
    } else if cmd == "--version" || cmd == "-V" || cmd == "version" {
        println!("{}", invocation_lang.version_text());
    } else if cmd == "--help" || cmd == "-h" || cmd == "help" {
        print_help(&invocation_lang, tr);
    } else {
        eprintln!(
            "{}: {}",
            t(
                &invocation_lang,
                "unknown command",
                "未知命令",
                "不明なコマンド",
                "알 수 없는 명령",
                "คำสั่งที่ไม่รู้จัก",
                "unknown command"
            ),
            cmd.red()
        );
        eprintln!("  {} --help", invocation_lang.cli_name());
        std::process::exit(1);
    }

    Ok(())
}

// ─── print_help ───────────────────────────────────────────────────────────────

fn print_help(lang: &InvocationLanguage, tr: &CommandTranslations) {
    let bin = lang.cli_name();

    println!("\n{}", lang.help_text().cyan().bold());

    let usage_label = t(
        lang,
        "Usage:",
        "用法:",
        "使用法:",
        "사용법:",
        "การใช้งาน:",
        "Usage:",
    );
    println!("\n{}", usage_label.yellow());

    let name_ph = t(lang, "NAME", "名称", "名前", "이름", "ชื่อ", "NAME");
    let pkg_ph = t(
        lang,
        "PACKAGE",
        "包",
        "パッケージ",
        "패키지",
        "แพ็คเกจ",
        "PACKAGE",
    );
    let query_ph = t(lang, "QUERY", "查询", "クエリ", "쿼리", "คิวรี", "QUERY");
    let file_ph = t(lang, "FILE", "文件", "ファイル", "파일", "ไฟล์", "FILE");

    let cmds: &[(&str, &str)] = &[
        (tr.new[0], name_ph),
        (tr.init[0], ""),
        (tr.build[0], ""),
        (tr.run[0], ""),
        (tr.test[0], ""),
        (tr.check[0], ""),
        (tr.clean[0], ""),
        (tr.doc[0], ""),
        (tr.fmt[0], ""),
        (tr.tree[0], ""),
        (tr.bench[0], ""),
        (tr.update[0], ""),
        (tr.add[0], pkg_ph),
        (tr.search[0], query_ph),
        (tr.publish[0], ""),
        (tr.login[0], ""),
        (tr.logout[0], ""),
        (tr.translate[0], file_ph),
        (tr.manifest[0], ""),
        (tr.wizard[0], ""),
        (
            tr.normalize[0],
            t(lang, "LANG", "语言", "言語", "언어", "ภาษา", "LANG"),
        ),
    ];

    for (cmd, arg) in cmds {
        if arg.is_empty() {
            println!("  {} {}", bin, cmd);
        } else {
            println!("  {} {} <{}>", bin, cmd, arg);
        }
    }

    let opts_label = t(
        lang,
        "Options:",
        "选项:",
        "オプション:",
        "옵션:",
        "ตัวเลือก:",
        "Options:",
    );
    println!("\n{}", opts_label.yellow());
    println!(
        "  --lang, -l <LANG>      {}",
        t(
            lang,
            "Scaffolding language (en/zh/ja/ko/th/ru)",
            "支架语言 (en/zh/ja/ko/th/ru)",
            "足場言語 (en/zh/ja/ko/th/ru)",
            "발판 언어 (en/zh/ja/ko/th/ru)",
            "ภาษาโครงสร้าง (en/zh/ja/ko/th/ru)",
            "Scaffolding language (en/zh/ja/ko/th/ru)",
        )
    );
    println!(
        "  --type <TYPE>          {}",
        t(
            lang,
            "Project type (bin/lib/game/ui/ai/crypto/web/polyglot)",
            "项目类型",
            "プロジェクトタイプ",
            "프로젝트 유형",
            "ประเภทโปรเจค",
            "Project type",
        )
    );
    println!(
        "  --version, -V          {}",
        t(
            lang,
            "Show version",
            "显示版本",
            "バージョン表示",
            "버전 표시",
            "แสดงเวอร์ชัน",
            "Show version"
        )
    );
    println!(
        "  --help, -h             {}",
        t(
            lang,
            "Show help",
            "显示帮助",
            "ヘルプ表示",
            "도움말 표시",
            "แสดงความช่วยเหลือ",
            "Show help"
        )
    );
    println!();
}

// ─── cmd_new ─────────────────────────────────────────────────────────────────

fn cmd_new(args: &[String], lang: &InvocationLanguage) -> anyhow::Result<()> {
    if args.is_empty() {
        say(
            lang,
            "Please provide a project name",
            "请提供项目名称",
            "プロジェクト名を指定してください",
            "프로젝트 이름을 입력하세요",
        );
        return Ok(());
    }

    let name = &args[0];
    let (kind, primary_lang, root_lang) = parse_new_flags(&args[1..]);
    let root_language = root_lang.unwrap_or_else(|| primary_lang.clone());

    println!(
        "{}",
        format!(
            "{} {}",
            t(
                lang,
                "Creating project:",
                "正在创建项目:",
                "プロジェクトを作成中:",
                "프로젝트 생성 중:",
                "กำลังสร้างโปรเจค:",
                "Creating project:"
            ),
            name
        )
        .green()
    );

    let project = ProjectScaffold {
        name: name.clone(),
        kind,
        language: primary_lang,
        root_language,
        spirit_level: "铁".into(),
    };
    scaffold::scaffold_project(&project)?;
    print_post_create(lang, name);
    Ok(())
}

// ─── cmd_init ────────────────────────────────────────────────────────────────

fn cmd_init(args: &[String], lang: &InvocationLanguage) -> anyhow::Result<()> {
    let cwd = std::env::current_dir()?;
    let name = cwd
        .file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_else(|| "project".into());

    let (kind, primary_lang, root_lang) = parse_new_flags(args);
    let root_language = root_lang.unwrap_or_else(|| primary_lang.clone());

    say(
        lang,
        "Initializing Ling project in current directory...",
        "正在当前目录初始化灵境...",
        "現在のディレクトリに霊境を初期化中...",
        "현재 디렉토리에 영경을 초기화 중...",
    );

    let project = ProjectScaffold {
        name,
        kind,
        language: primary_lang,
        root_language,
        spirit_level: "铁".into(),
    };
    scaffold::scaffold_in_dir(&project, &cwd)?;

    println!(
        "{}",
        t(
            lang,
            "✓ Initialized!",
            "✓ 初始完成！",
            "✓ 初期化完了！",
            "✓ 초기화 완료！",
            "✓ เสร็จสิ้น！",
            "✓ Initialized!"
        )
        .green()
        .bold()
    );
    Ok(())
}

// ─── cmd_build ───────────────────────────────────────────────────────────────

/// First value following `flag` in `args` (e.g. `--path ./app` → `./app`).
fn flag_value(args: &[String], flag: &str) -> Option<String> {
    args.windows(2).find(|w| w[0] == flag).map(|w| w[1].clone())
}

/// Copy of `args` with `flag` and the value after it removed, so wrapper-only
/// flags (like `--path`) are never forwarded to `cargo` / `ling`.
fn strip_flag(args: &[String], flag: &str) -> Vec<String> {
    let mut out = Vec::with_capacity(args.len());
    let mut skip_value = false;
    for a in args {
        if skip_value {
            skip_value = false;
            continue;
        }
        if a == flag {
            skip_value = true; // also drop the value that follows
            continue;
        }
        out.push(a.clone());
    }
    out
}

fn cmd_build(args: &[String], lang: &InvocationLanguage) -> anyhow::Result<()> {
    say(
        lang,
        "Building Ling project...",
        "正在筑造灵境...",
        "霊境を築造中...",
        "영경 건설 중...",
    );

    // `--path <dir>` pins the project root explicitly. Otherwise we walk up for a
    // manifest, and finally fall back to the current directory so a bare folder of
    // `.ling` files still builds. The flag is consumed here, never forwarded.
    let path_override = flag_value(args, "--path").map(std::path::PathBuf::from);
    let forwarded = strip_flag(args, "--path");

    // Resolve the project root and its scaffolding language (used for entry-file
    // candidates). An explicit --path looks for a manifest inside that folder; a
    // bare folder with none just builds as an English-layout .ling project.
    let (root, slang) = if let Some(p) = path_override {
        let slang = MANIFEST_NAMES
            .iter()
            .map(|n| p.join(n))
            .find(|m| m.exists())
            .as_deref()
            .map(lang_from_manifest)
            .unwrap_or(ScaffoldLang::English);
        (p, slang)
    } else if let Some(manifest) = find_manifest() {
        let slang = lang_from_manifest(&manifest);
        let root = manifest
            .parent()
            .map(std::path::Path::to_path_buf)
            .unwrap_or_else(|| std::path::PathBuf::from("."));
        (root, slang)
    } else {
        (std::path::PathBuf::from("."), ScaffoldLang::English)
    };

    // Resolve the entry file (same candidate order as `cmd_run`) so the native
    // build targets the real entry regardless of project layout.
    let dirs = dir_names(&slang);
    let src_dir = root.join(dirs.src);
    let entry_candidates = [
        src_dir.join(dirs.entry),
        root.join(dirs.entry),
        src_dir.join("main.ling"),
        root.join("main.ling"),
        root.join("start.ling"),
    ];
    let entry = entry_candidates.into_iter().find(|p| p.exists());
    if let Some(e) = &entry {
        say(
            lang,
            &format!("Entry: {}", e.display()),
            &format!("灵源: {}", e.display()),
            &format!("霊源: {}", e.display()),
            &format!("령원: {}", e.display()),
        );
    }

    if root.join("Cargo.toml").exists() {
        // A Cargo project sits alongside the .ling sources — defer to cargo.
        let mut cmd = std::process::Command::new("cargo");
        cmd.arg("build").args(&forwarded).current_dir(&root);
        let status = cmd.status().map_err(|e| anyhow::anyhow!("cargo: {e}"))?;
        if !status.success() {
            anyhow::bail!("build failed");
        }
    } else {
        // Pure .ling project (no Cargo.toml): AOT-compile to a native executable
        // by default (pass --no-aot to embed the interpreter). Target the resolved
        // entry file — `ling build <file>` flattens imports from there — otherwise
        // hand `ling build` the directory and let it auto-detect the entry.
        let target = entry.as_deref().unwrap_or(root.as_path());
        run_aot_build(target, &forwarded, lang)?;
    }
    Ok(())
}

/// The current host's `ling build` platform token.
fn native_platform_name() -> &'static str {
    if cfg!(target_os = "windows") {
        "win"
    } else if cfg!(target_os = "macos") {
        "mac"
    } else {
        "lin"
    }
}

/// Build a native `.ling` project by delegating to the `ling` binary's AOT
/// compiler (`ling build <root> --aot`). AOT is the default; pass `--no-aot` to
/// fall back to the interpreter-embedding build. Extra flags (`--out`, `--icon`,
/// `--pack`, `--platform`) pass straight through; with no `--platform` we default
/// to a native-only build (no web bundle), matching `lingfu build`'s old scope.
fn run_aot_build(
    target: &std::path::Path,
    args: &[String],
    lang: &InvocationLanguage,
) -> anyhow::Result<()> {
    let aot = !args.iter().any(|a| a == "--no-aot");
    say(
        lang,
        if aot {
            "→ AOT-compiling to a native executable…"
        } else {
            "→ building native executable (interpreter-embedded)…"
        },
        if aot {
            "→ 正在 AOT 编译为本地可执行文件…"
        } else {
            "→ 正在构建本地可执行文件（内嵌解释器）…"
        },
        if aot {
            "→ ネイティブ実行ファイルへ AOT コンパイル中…"
        } else {
            "→ ネイティブ実行ファイルを構築中（インタプリタ埋め込み）…"
        },
        if aot {
            "→ 네이티브 실행 파일로 AOT 컴파일 중…"
        } else {
            "→ 네이티브 실행 파일 빌드 중(인터프리터 내장)…"
        },
    );

    let ling_bin = find_ling_binary();
    let mut cmd = std::process::Command::new(&ling_bin);
    cmd.arg("build").arg(target);
    if aot {
        cmd.arg("--aot");
    }
    if !args.iter().any(|a| a == "--platform") {
        cmd.arg("--platform").arg(native_platform_name());
    }
    // Forward every build flag except our own --no-aot opt-out.
    for a in args.iter().filter(|a| a.as_str() != "--no-aot") {
        cmd.arg(a);
    }

    let status = cmd.status().map_err(|e| anyhow::anyhow!("ling: {e}"))?;
    if !status.success() {
        anyhow::bail!("build failed");
    }
    Ok(())
}

// ─── cmd_run ─────────────────────────────────────────────────────────────────

fn cmd_run(args: &[String], lang: &InvocationLanguage) -> anyhow::Result<()> {
    if let Some(manifest) = find_manifest() {
        let slang = lang_from_manifest(&manifest);
        let dirs = dir_names(&slang);
        let root = manifest.parent().unwrap_or(std::path::Path::new("."));
        let src_dir = root.join(dirs.src);

        // Try entry file candidates in order
        let entry_candidates = [
            src_dir.join(dirs.entry),
            root.join(dirs.entry),
            src_dir.join("main.ling"),
            root.join("main.ling"),
        ];

        if let Some(entry) = entry_candidates.iter().find(|p| p.exists()) {
            say(
                lang,
                &format!("Running {}...", entry.display()),
                &format!("正在行灵 {}...", entry.display()),
                &format!("{}を実行中...", entry.display()),
                &format!("{} 실행 중...", entry.display()),
            );
            let ling_bin = find_ling_binary();
            let status = std::process::Command::new(ling_bin)
                .arg("run")
                .arg(entry)
                .args(args)
                .status()
                .map_err(|e| anyhow::anyhow!("ling: {e}"))?;
            if !status.success() {
                anyhow::bail!("run failed");
            }
            return Ok(());
        }
    }

    // Fallback: search cwd for any .ling file
    if let Some(path) = find_ling_entry() {
        say(
            lang,
            &format!("Running {}...", path.display()),
            &format!("正在行灵 {}...", path.display()),
            &format!("{}を実行中...", path.display()),
            &format!("{} 실행 중...", path.display()),
        );
        let ling_bin = find_ling_binary();
        let status = std::process::Command::new(ling_bin)
            .arg("run")
            .arg(&path)
            .args(args)
            .status()
            .map_err(|e| anyhow::anyhow!("ling: {e}"))?;
        if !status.success() {
            anyhow::bail!("run failed");
        }
        return Ok(());
    }

    // Last resort: cargo run
    say(
        lang,
        "Running Ling project...",
        "正在行灵...",
        "霊を実行中...",
        "영 실행 중...",
    );
    let status = std::process::Command::new("cargo")
        .arg("run")
        .args(args)
        .status()
        .map_err(|e| anyhow::anyhow!("cargo: {e}"))?;
    if !status.success() {
        anyhow::bail!("run failed");
    }
    Ok(())
}

// ─── cmd_test ────────────────────────────────────────────────────────────────

fn cmd_test(args: &[String], lang: &InvocationLanguage) -> anyhow::Result<()> {
    say(
        lang,
        "Testing Ling project...",
        "正在试灵...",
        "霊をテスト中...",
        "영 테스트 중...",
    );

    if let Some(manifest) = find_manifest() {
        let slang = lang_from_manifest(&manifest);
        let dirs = dir_names(&slang);
        let root = manifest.parent().unwrap_or(std::path::Path::new("."));
        let tests_dir = root.join(dirs.tests);
        let test_file = root.join(dirs.src).join(dirs.test_file);

        let ling_bin = find_ling_binary();
        let mut ran_any = false;

        // Run dedicated test file if it exists
        if test_file.exists() {
            let status = std::process::Command::new(&ling_bin)
                .arg("run")
                .arg(&test_file)
                .status()
                .map_err(|e| anyhow::anyhow!("ling: {e}"))?;
            if !status.success() {
                anyhow::bail!("tests failed");
            }
            ran_any = true;
        }

        // Run all .ling files under tests directory
        if tests_dir.exists() {
            for entry in walkdir::WalkDir::new(&tests_dir)
                .into_iter()
                .filter_map(|e| e.ok())
                .filter(|e| e.path().is_file())
                .filter(|e| has_ling_ext(e.path()))
            {
                say(
                    lang,
                    &format!("  test: {}", entry.path().display()),
                    &format!("  试验: {}", entry.path().display()),
                    &format!("  テスト: {}", entry.path().display()),
                    &format!("  시험: {}", entry.path().display()),
                );
                let status = std::process::Command::new(&ling_bin)
                    .arg("run")
                    .arg(entry.path())
                    .status()
                    .map_err(|e| anyhow::anyhow!("ling: {e}"))?;
                if !status.success() {
                    anyhow::bail!("test failed: {}", entry.path().display());
                }
                ran_any = true;
            }
        }

        if !ran_any && root.join("Cargo.toml").exists() {
            let status = std::process::Command::new("cargo")
                .arg("test")
                .args(args)
                .current_dir(root)
                .status()
                .map_err(|e| anyhow::anyhow!("cargo: {e}"))?;
            if !status.success() {
                anyhow::bail!("tests failed");
            }
        }
    } else {
        let status = std::process::Command::new("cargo")
            .arg("test")
            .args(args)
            .status()
            .map_err(|e| anyhow::anyhow!("cargo: {e}"))?;
        if !status.success() {
            anyhow::bail!("tests failed");
        }
    }

    println!(
        "{}",
        t(
            lang,
            "✓ All tests passed!",
            "✓ 所有试验通过！",
            "✓ 全テスト通過！",
            "✓ 모든 테스트 통과！",
            "✓ ทดสอบทั้งหมดผ่าน！",
            "✓ All tests passed!"
        )
        .green()
        .bold()
    );
    Ok(())
}

// ─── cmd_check ───────────────────────────────────────────────────────────────

fn cmd_check(args: &[String], lang: &InvocationLanguage) -> anyhow::Result<()> {
    say(
        lang,
        "Checking Ling source...",
        "正在核查灵源...",
        "霊源を確認中...",
        "령원 확인 중...",
    );

    let ling_bin = find_ling_binary();
    let mut checked = 0u32;

    // Check all .ling files we can find under src/ or current dir
    let src_roots = if let Some(manifest) = find_manifest() {
        let slang = lang_from_manifest(&manifest);
        let dirs = dir_names(&slang);
        let root = manifest
            .parent()
            .unwrap_or(std::path::Path::new("."))
            .to_path_buf();
        vec![root.join(dirs.src), root.join(dirs.tests)]
    } else {
        vec![
            std::path::PathBuf::from("src"),
            std::path::PathBuf::from("."),
        ]
    };

    for root in &src_roots {
        if !root.exists() {
            continue;
        }
        for entry in walkdir::WalkDir::new(root)
            .into_iter()
            .filter_map(|e| e.ok())
            .filter(|e| e.path().is_file())
            .filter(|e| has_ling_ext(e.path()))
        {
            let status = std::process::Command::new(&ling_bin)
                .arg("check")
                .arg(entry.path())
                .args(args)
                .status()
                .map_err(|e| anyhow::anyhow!("ling: {e}"))?;
            if !status.success() {
                anyhow::bail!("check failed: {}", entry.path().display());
            }
            checked += 1;
        }
    }

    if checked == 0 {
        // Try cargo check as fallback
        let status = std::process::Command::new("cargo")
            .arg("check")
            .args(args)
            .status()
            .map_err(|e| anyhow::anyhow!("cargo: {e}"))?;
        if !status.success() {
            anyhow::bail!("check failed");
        }
    } else {
        println!(
            "{}",
            format!(
                "{} {}",
                t(
                    lang,
                    "✓ Checked",
                    "✓ 核查",
                    "✓ 確認",
                    "✓ 확인",
                    "✓ ตรวจสอบ",
                    "✓ Checked"
                ),
                checked
            )
            .green()
            .bold()
        );
    }
    Ok(())
}

// ─── cmd_clean ───────────────────────────────────────────────────────────────

fn cmd_clean(lang: &InvocationLanguage) -> anyhow::Result<()> {
    say(
        lang,
        "Cleaning build artifacts...",
        "正在清洁灵碑...",
        "霊碑を清浄化中...",
        "령비 정화 중...",
    );

    let mut cleaned = false;

    if let Some(manifest) = find_manifest() {
        let slang = lang_from_manifest(&manifest);
        let dirs = dir_names(&slang);
        let root = manifest.parent().unwrap_or(std::path::Path::new("."));

        for dir_name in &[dirs.build, dirs.lock] {
            let dir = root.join(dir_name);
            if dir.exists() {
                std::fs::remove_dir_all(&dir)?;
                println!(
                    "  {} {}",
                    t(lang, "removed", "已清", "削除", "삭제", "ลบ", "removed").red(),
                    dir.display()
                );
                cleaned = true;
            }
        }
    }

    // Also clean target/ if Cargo.toml is present
    if std::path::Path::new("Cargo.toml").exists() || std::path::Path::new("target").exists() {
        let status = std::process::Command::new("cargo")
            .arg("clean")
            .status()
            .map_err(|e| anyhow::anyhow!("cargo: {e}"))?;
        if !status.success() {
            anyhow::bail!("clean failed");
        }
        cleaned = true;
    }

    if cleaned {
        println!(
            "{}",
            t(
                lang,
                "✓ Clean!",
                "✓ 已清洁！",
                "✓ 清浄完了！",
                "✓ 정화 완료！",
                "✓ ล้างเสร็จ！",
                "✓ Clean!"
            )
            .green()
            .bold()
        );
    } else {
        say(
            lang,
            "Nothing to clean.",
            "无需清洁。",
            "清浄するものはありません。",
            "정화할 것이 없습니다.",
        );
    }
    Ok(())
}

// ─── cmd_doc ─────────────────────────────────────────────────────────────────

fn cmd_doc(lang: &InvocationLanguage) -> anyhow::Result<()> {
    say(
        lang,
        "Generating documentation...",
        "正在生成文典...",
        "文典を生成中...",
        "문전 생성 중...",
    );

    // Try mdBook if docs/ or book/ exists
    for docs_dir in &["docs", "book"] {
        if std::path::Path::new(docs_dir).join("book.toml").exists() {
            let status = std::process::Command::new("mdbook")
                .arg("build")
                .arg(docs_dir)
                .status()
                .map_err(|e| anyhow::anyhow!("mdbook: {e}"))?;
            if !status.success() {
                anyhow::bail!("doc failed");
            }
            println!(
                "{}",
                t(
                    lang,
                    "✓ Docs built!",
                    "✓ 文典已生！",
                    "✓ 文典完成！",
                    "✓ 문전 완성！",
                    "✓ เอกสารเสร็จ！",
                    "✓ Docs built!"
                )
                .green()
                .bold()
            );
            return Ok(());
        }
    }

    // Fallback to cargo doc
    let status = std::process::Command::new("cargo")
        .arg("doc")
        .arg("--no-deps")
        .status()
        .map_err(|e| anyhow::anyhow!("cargo: {e}"))?;
    if !status.success() {
        anyhow::bail!("doc failed");
    }
    println!(
        "{}",
        t(
            lang,
            "✓ Docs built!",
            "✓ 文典已生！",
            "✓ 文典完成！",
            "✓ 문전 완성！",
            "✓ เอกสารเสร็จ！",
            "✓ Docs built!"
        )
        .green()
        .bold()
    );
    Ok(())
}

// ─── cmd_fmt ─────────────────────────────────────────────────────────────────

fn cmd_fmt(args: &[String], lang: &InvocationLanguage) -> anyhow::Result<()> {
    say(
        lang,
        "Formatting Ling sources...",
        "正在整理灵源...",
        "霊源を整形中...",
        "령원 정형 중...",
    );

    let ling_bin = find_ling_binary();
    let mut found = false;

    let roots = if let Some(manifest) = find_manifest() {
        let slang = lang_from_manifest(&manifest);
        let dirs = dir_names(&slang);
        let root = manifest
            .parent()
            .unwrap_or(std::path::Path::new("."))
            .to_path_buf();
        vec![root.join(dirs.src)]
    } else {
        vec![
            std::path::PathBuf::from("src"),
            std::path::PathBuf::from("."),
        ]
    };

    for root in &roots {
        if !root.exists() {
            continue;
        }
        for entry in walkdir::WalkDir::new(root)
            .into_iter()
            .filter_map(|e| e.ok())
            .filter(|e| e.path().is_file())
            .filter(|e| has_ling_ext(e.path()))
        {
            let status = std::process::Command::new(&ling_bin)
                .arg("fmt")
                .arg(entry.path())
                .args(args)
                .status()
                .map_err(|e| anyhow::anyhow!("ling: {e}"))?;
            if !status.success() {
                eprintln!("  fmt skipped: {}", entry.path().display());
            }
            found = true;
        }
    }

    if !found {
        // Fallback: rustfmt
        let _ = std::process::Command::new("cargo")
            .arg("fmt")
            .args(args)
            .status();
    }

    println!(
        "{}",
        t(
            lang,
            "✓ Formatted!",
            "✓ 已整理！",
            "✓ 整形完了！",
            "✓ 정형 완료！",
            "✓ จัดรูปแบบเสร็จ！",
            "✓ Formatted!"
        )
        .green()
        .bold()
    );
    Ok(())
}

// ─── cmd_tree ────────────────────────────────────────────────────────────────

fn cmd_tree(lang: &InvocationLanguage) -> anyhow::Result<()> {
    let root_dir = if let Some(manifest) = find_manifest() {
        manifest
            .parent()
            .unwrap_or(std::path::Path::new("."))
            .to_path_buf()
    } else {
        std::env::current_dir()?
    };

    println!(
        "{}",
        t(
            lang,
            "Project tree:",
            "项目结构:",
            "プロジェクト構造:",
            "프로젝트 구조:",
            "โครงสร้างโปรเจค:",
            "Project tree:"
        )
        .yellow()
    );
    print_tree(&root_dir, "", true)?;
    Ok(())
}

fn print_tree(dir: &std::path::Path, prefix: &str, is_root: bool) -> anyhow::Result<()> {
    let name = if is_root {
        dir.file_name()
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_else(|| ".".into())
    } else {
        dir.file_name()
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_default()
    };

    println!("{}{}", prefix, name.cyan());

    let skip = [".git", "target", "node_modules"];
    let mut entries: Vec<_> = std::fs::read_dir(dir)?
        .filter_map(|e| e.ok())
        .filter(|e| !skip.contains(&e.file_name().to_string_lossy().as_ref()))
        .collect();
    entries.sort_by_key(|e| e.file_name());

    for (i, entry) in entries.iter().enumerate() {
        let is_last = i == entries.len() - 1;
        let connector = if is_last { "└── " } else { "├── " };
        let child_prefix = if is_last { "    " } else { "│   " };
        let path = entry.path();

        if path.is_dir() {
            print_tree(&path, &format!("{}{}", prefix, connector), false)?;
        } else {
            let fname = entry.file_name().to_string_lossy().to_string();
            let colored_name = if has_ling_ext(&path) {
                fname.green().to_string()
            } else if fname.ends_with(".toml") {
                fname.yellow().to_string()
            } else {
                fname
            };
            println!("{}{}{}", prefix, connector, colored_name);
        }

        let _ = child_prefix; // used only for recursion
    }
    Ok(())
}

// ─── cmd_bench ───────────────────────────────────────────────────────────────

fn cmd_bench(args: &[String], lang: &InvocationLanguage) -> anyhow::Result<()> {
    say(
        lang,
        "Running benchmarks...",
        "正在运行基准测试...",
        "ベンチマーク実行中...",
        "벤치마크 실행 중...",
    );
    let status = std::process::Command::new("cargo")
        .arg("bench")
        .args(args)
        .status()
        .map_err(|e| anyhow::anyhow!("cargo: {e}"))?;
    if !status.success() {
        anyhow::bail!("bench failed");
    }
    Ok(())
}

// ─── cmd_update ──────────────────────────────────────────────────────────────

fn cmd_update(args: &[String], lang: &InvocationLanguage) -> anyhow::Result<()> {
    say(
        lang,
        "Updating dependencies...",
        "正在更新依赖...",
        "依存関係を更新中...",
        "의존성 업데이트 중...",
    );
    let status = std::process::Command::new("cargo")
        .arg("update")
        .args(args)
        .status()
        .map_err(|e| anyhow::anyhow!("cargo: {e}"))?;
    if !status.success() {
        anyhow::bail!("update failed");
    }
    println!(
        "{}",
        t(
            lang,
            "✓ Updated!",
            "✓ 已更新！",
            "✓ 更新完了！",
            "✓ 업데이트 완료！",
            "✓ อัปเดตเสร็จ！",
            "✓ Updated!"
        )
        .green()
        .bold()
    );
    Ok(())
}

// ─── cmd_add ─────────────────────────────────────────────────────────────────

fn cmd_add(args: &[String], lang: &InvocationLanguage) -> anyhow::Result<()> {
    if args.is_empty() {
        say(
            lang,
            "Please specify a package to add",
            "请指定要添加的包",
            "追加するパッケージを指定してください",
            "추가할 패키지를 지정하세요",
        );
        return Ok(());
    }
    let package = &args[0];
    say(
        lang,
        &format!("Adding package: {}", package),
        &format!("正在添包: {}", package),
        &format!("パッケージを追加中: {}", package),
        &format!("패키지 추가 중: {}", package),
    );
    let status = std::process::Command::new("cargo")
        .arg("add")
        .args(args)
        .status()
        .map_err(|e| anyhow::anyhow!("cargo: {e}"))?;
    if !status.success() {
        anyhow::bail!("add failed");
    }
    println!(
        "{}",
        t(
            lang,
            "✓ Added!",
            "✓ 已添！",
            "✓ 追加完了！",
            "✓ 추가 완료！",
            "✓ เพิ่มเสร็จ！",
            "✓ Added!"
        )
        .green()
        .bold()
    );
    Ok(())
}

// ─── cmd_search ──────────────────────────────────────────────────────────────

fn cmd_search(args: &[String], lang: &InvocationLanguage) -> anyhow::Result<()> {
    if args.is_empty() {
        say(
            lang,
            "Please provide a search query",
            "请提供搜索词",
            "検索クエリを入力してください",
            "검색어를 입력하세요",
        );
        return Ok(());
    }
    say(
        lang,
        &format!("Searching: {}", args[0]),
        &format!("正在寻找: {}", args[0]),
        &format!("検索中: {}", args[0]),
        &format!("검색 중: {}", args[0]),
    );
    let status = std::process::Command::new("cargo")
        .arg("search")
        .args(args)
        .status()
        .map_err(|e| anyhow::anyhow!("cargo: {e}"))?;
    if !status.success() {
        anyhow::bail!("search failed");
    }
    Ok(())
}

// ─── cmd_publish ─────────────────────────────────────────────────────────────

fn cmd_publish(args: &[String], lang: &InvocationLanguage) -> anyhow::Result<()> {
    say(
        lang,
        "Publishing to registry...",
        "正在发布到注册处...",
        "レジストリに公開中...",
        "레지스트리에 게시 중...",
    );
    let status = std::process::Command::new("cargo")
        .arg("publish")
        .args(args)
        .status()
        .map_err(|e| anyhow::anyhow!("cargo: {e}"))?;
    if !status.success() {
        anyhow::bail!("publish failed");
    }
    println!(
        "{}",
        t(
            lang,
            "✓ Published!",
            "✓ 已发布！",
            "✓ 公開完了！",
            "✓ 게시 완료！",
            "✓ เผยแพร่เสร็จ！",
            "✓ Published!"
        )
        .green()
        .bold()
    );
    Ok(())
}

// ─── cmd_login / logout ───────────────────────────────────────────────────────

fn cmd_login(args: &[String], lang: &InvocationLanguage) -> anyhow::Result<()> {
    say(
        lang,
        "Logging in to registry...",
        "正在登入注册处...",
        "レジストリにログイン中...",
        "레지스트리 로그인 중...",
    );
    let status = std::process::Command::new("cargo")
        .arg("login")
        .args(args)
        .status()
        .map_err(|e| anyhow::anyhow!("cargo: {e}"))?;
    if !status.success() {
        anyhow::bail!("login failed");
    }
    println!(
        "{}",
        t(
            lang,
            "✓ Logged in!",
            "✓ 已登入！",
            "✓ ログイン完了！",
            "✓ 로그인 완료！",
            "✓ เข้าสู่ระบบเสร็จ！",
            "✓ Logged in!"
        )
        .green()
        .bold()
    );
    Ok(())
}

fn cmd_logout(lang: &InvocationLanguage) -> anyhow::Result<()> {
    say(
        lang,
        "Logging out from registry...",
        "正在退出注册处...",
        "レジストリからログアウト中...",
        "레지스트리 로그아웃 중...",
    );
    let status = std::process::Command::new("cargo")
        .arg("logout")
        .status()
        .map_err(|e| anyhow::anyhow!("cargo: {e}"))?;
    if !status.success() {
        anyhow::bail!("logout failed");
    }
    println!(
        "{}",
        t(
            lang,
            "✓ Logged out!",
            "✓ 已退出！",
            "✓ ログアウト完了！",
            "✓ 로그아웃 완료！",
            "✓ ออกจากระบบเสร็จ！",
            "✓ Logged out!"
        )
        .green()
        .bold()
    );
    Ok(())
}

// ─── cmd_translate ───────────────────────────────────────────────────────────

fn cmd_translate(args: &[String], lang: &InvocationLanguage) -> anyhow::Result<()> {
    say(
        lang,
        "Translate source keywords between lexicons (use --from <lang> --to <lang> <file>)",
        "翻译源文件关键字 (使用 --from <语言> --to <语言> <文件>)",
        "ソースキーワードを翻訳 (--from <言語> --to <言語> <ファイル>)",
        "소스 키워드 번역 (--from <언어> --to <언어> <파일>)",
    );

    let mut from_lang = String::new();
    let mut to_lang = String::new();
    let mut file_path = String::new();

    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "--from" => {
                if i + 1 < args.len() {
                    from_lang = args[i + 1].clone();
                    i += 1;
                }
            },
            "--to" => {
                if i + 1 < args.len() {
                    to_lang = args[i + 1].clone();
                    i += 1;
                }
            },
            s if !s.starts_with('-') && file_path.is_empty() => {
                file_path = s.to_string();
            },
            _ => {},
        }
        i += 1;
    }

    if file_path.is_empty() || from_lang.is_empty() || to_lang.is_empty() {
        say(
            lang,
            "Usage: translate --from <lang> --to <lang> <file>",
            "用法: 译 --from <语言> --to <语言> <文件>",
            "使用法: 訳 --from <言語> --to <言語> <ファイル>",
            "사용법: 번 --from <언어> --to <언어> <파일>",
        );
        return Ok(());
    }

    say(
        lang,
        &format!(
            "Translating {} from {} to {}...",
            file_path, from_lang, to_lang
        ),
        &format!("正在翻译 {} 从 {} 到 {}...", file_path, from_lang, to_lang),
        &format!(
            "{} を {} から {} へ翻訳中...",
            file_path, from_lang, to_lang
        ),
        &format!(
            "{} 를 {} 에서 {} 로 번역 중...",
            file_path, from_lang, to_lang
        ),
    );

    // Invoke ling binary with translate subcommand
    let ling_bin = find_ling_binary();
    let status = std::process::Command::new(&ling_bin)
        .args([
            "translate",
            "--from",
            &from_lang,
            "--to",
            &to_lang,
            &file_path,
        ])
        .status()
        .map_err(|e| anyhow::anyhow!("ling: {e}"))?;
    if !status.success() {
        anyhow::bail!("translate failed");
    }
    Ok(())
}

// ─── cmd_manifest ────────────────────────────────────────────────────────────

fn cmd_manifest(lang: &InvocationLanguage) -> anyhow::Result<()> {
    if let Some(path) = find_manifest() {
        say(
            lang,
            &format!("Manifest: {}", path.display()),
            &format!("灵符清单: {}", path.display()),
            &format!("マニフェスト: {}", path.display()),
            &format!("매니페스트: {}", path.display()),
        );
        let content = std::fs::read_to_string(&path)?;
        println!("{}", content);
    } else if std::path::Path::new("Cargo.toml").exists() {
        println!("{}", std::fs::read_to_string("Cargo.toml")?);
    } else {
        say(
            lang,
            "No manifest found (Ling.toml / 灵符.toml / 霊符.toml / 영부.toml / ลิงฟู.toml)",
            "未找到灵符清单",
            "マニフェストが見つかりません",
            "매니페스트를 찾을 수 없습니다",
        );
    }
    Ok(())
}

// ─── cmd_wizard ──────────────────────────────────────────────────────────────

fn cmd_wizard(lang: &InvocationLanguage) -> anyhow::Result<()> {
    println!("{}", lang.welcome_message().magenta().bold());

    let name_prompt = t(
        lang,
        "Project Name:",
        "项目名称:",
        "プロジェクト名:",
        "프로젝트 이름:",
        "ชื่อโปรเจค:",
        "Project Name:",
    );
    let name: String = Input::new().with_prompt(name_prompt).interact_text()?;

    let type_options: Vec<&str> = match lang {
        InvocationLanguage::Chinese => vec![
            "独行 (Binary)",
            "共修 (Library)",
            "游灵 (Game)",
            "显灵 (UI)",
            "智灵 (AI/ML)",
            "密灵 (Crypto)",
            "网灵 (Web/WASM)",
            "万言 (Polyglot)",
        ],
        InvocationLanguage::Japanese => vec![
            "単独 (Binary)",
            "共修 (Library)",
            "游霊 (Game)",
            "表霊 (UI)",
            "智霊 (AI/ML)",
            "密霊 (Crypto)",
            "網霊 (Web)",
            "万言 (Polyglot)",
        ],
        InvocationLanguage::Korean => vec![
            "독립 (Binary)",
            "라이브러리 (Library)",
            "게임 (Game)",
            "인터페이스 (UI)",
            "인공지능 (AI)",
            "암호 (Crypto)",
            "웹 (Web)",
            "다국어 (Polyglot)",
        ],
        InvocationLanguage::Thai => vec![
            "โปรแกรม (Binary)",
            "ไลบรารี (Library)",
            "เกม (Game)",
            "ส่วนติดต่อ (UI)",
            "ปัญญา (AI)",
            "การเข้ารหัส (Crypto)",
            "เว็บ (Web)",
            "หลายภาษา (Polyglot)",
        ],
        _ => vec![
            "Binary", "Library", "Game", "UI", "AI/ML", "Crypto", "Web/WASM", "Polyglot",
        ],
    };

    let type_prompt = t(
        lang,
        "Spirit Realm Type:",
        "灵境类型:",
        "霊境タイプ:",
        "영경 유형:",
        "ประเภทวิญญาณ:",
        "Project Type:",
    );
    let selection = Select::new()
        .with_prompt(type_prompt)
        .items(&type_options)
        .default(0)
        .interact()?;

    let project_kind = match selection {
        0 => ProjectKind::Bin,
        1 => ProjectKind::Lib,
        2 => ProjectKind::Game,
        3 => ProjectKind::Ui,
        4 => ProjectKind::Ai,
        5 => ProjectKind::Crypto,
        6 => ProjectKind::Web,
        _ => ProjectKind::Polyglot,
    };

    let lang_options = &["en", "zh", "ja", "ko", "th", "ru"];
    let lang_prompt = t(
        lang,
        "Scaffolding language:",
        "支架语言:",
        "足場言語:",
        "발판 언어:",
        "ภาษาโครงสร้าง:",
        "Scaffolding language:",
    );
    let lang_sel = Select::new()
        .with_prompt(lang_prompt)
        .items(lang_options)
        .default(0)
        .interact()?;
    let primary_lang = lang_options[lang_sel].to_string();

    println!(
        "{}",
        format!(
            "{} {}",
            t(
                lang,
                "Creating project:",
                "正在创建项目:",
                "プロジェクトを作成中:",
                "프로젝트 생성 중:",
                "กำลังสร้างโปรเจค:",
                "Creating project:"
            ),
            name
        )
        .green()
    );

    let project = ProjectScaffold {
        name: name.clone(),
        kind: project_kind,
        language: primary_lang.clone(),
        root_language: primary_lang,
        spirit_level: "铁".into(),
    };
    scaffold::scaffold_project(&project)?;
    print_post_create(lang, &name);
    Ok(())
}

// ─── Helpers ──────────────────────────────────────────────────────────────────

fn parse_new_flags(args: &[String]) -> (ProjectKind, String, Option<String>) {
    let mut kind = ProjectKind::Bin;
    let mut primary_lang = "en".to_string();
    let mut root_lang: Option<String> = None;
    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "--type" | "-t" if i + 1 < args.len() => {
                kind = ProjectKind::from_str(&args[i + 1]);
                i += 1;
            },
            "--lang" | "-l" if i + 1 < args.len() => {
                primary_lang = args[i + 1].clone();
                i += 1;
            },
            "--root-lang" if i + 1 < args.len() => {
                root_lang = Some(args[i + 1].clone());
                i += 1;
            },
            _ => {},
        }
        i += 1;
    }
    (kind, primary_lang, root_lang)
}

fn print_post_create(lang: &InvocationLanguage, name: &str) {
    println!(
        "\n{}",
        t(
            lang,
            "✓ Spirit Realm created!",
            "✓ 灵境已成！",
            "✓ 霊境が完成しました！",
            "✓ 영경이 완성되었습니다！",
            "✓ วิญญาณเสร็จสิ้น！",
            "✓ Spirit Realm created!"
        )
        .green()
        .bold()
    );
    println!("  cd {}", name);
    println!("  {} build", "lingfu".cyan());
    println!("  {} run", "lingfu".cyan());
}

fn find_ling_entry() -> Option<std::path::PathBuf> {
    let candidates = [
        "main.ling",
        "src/main.ling",
        "src/start.ling",
        "start.ling",
        "启.灵",
        "灵源/启.灵",
        "啓.霊",
        "霊源/啓.霊",
        "시작.령",
        "령원/시작.령",
        "เริ่ม.ลิง",
    ];
    for c in &candidates {
        let p = std::path::Path::new(c);
        if p.exists() {
            return Some(p.to_path_buf());
        }
    }
    if let Ok(entries) = std::fs::read_dir(".") {
        for entry in entries.flatten() {
            if has_ling_ext(&entry.path()) {
                return Some(entry.path());
            }
        }
    }
    None
}

fn find_ling_binary() -> std::path::PathBuf {
    if let Ok(exe) = std::env::current_exe() {
        let parent = exe.parent().unwrap_or(std::path::Path::new("."));
        for name in &["ling", "ling.exe"] {
            let p = parent.join(name);
            if p.exists() {
                return p;
            }
        }
    }
    std::path::PathBuf::from("ling")
}

fn has_ling_ext(path: &std::path::Path) -> bool {
    match path.extension().and_then(|e| e.to_str()) {
        Some("ling" | "灵" | "霊" | "령" | "ลิง") => true,
        _ => false,
    }
}

// ─── cmd_normalize ────────────────────────────────────────────────────────────

fn cmd_normalize(args: &[String], lang: &InvocationLanguage) -> anyhow::Result<()> {
    // Parse flags
    let dry_run = args.iter().any(|a| a == "--dry-run" || a == "-n");
    let content_only = args.iter().any(|a| a == "--content-only");
    let files_only = args.iter().any(|a| a == "--files-only");

    // Find target language: positional or --lang / -l flag
    let mut target_lang_str: Option<&str> = None;
    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "--lang" | "-l" if i + 1 < args.len() => {
                target_lang_str = Some(&args[i + 1]);
                i += 2;
            },
            s if !s.starts_with('-') => {
                target_lang_str = Some(s);
                i += 1;
            },
            _ => {
                i += 1;
            },
        }
    }

    let target = match target_lang_str.and_then(normalize::Lang::from_str) {
        Some(l) => l,
        None => {
            // Interactive prompt if no language specified
            let opts = &[
                "en  — English",
                "zh  — 中文",
                "ja  — 日本語",
                "ko  — 한국어",
                "th  — ภาษาไทย",
            ];
            let prompt = t(
                lang,
                "Target language:",
                "目标语言:",
                "対象言語:",
                "대상 언어:",
                "ภาษาเป้าหมาย:",
                "Target language:",
            );
            let sel = dialoguer::Select::new()
                .with_prompt(prompt)
                .items(opts)
                .default(0)
                .interact()?;
            [
                normalize::Lang::En,
                normalize::Lang::Zh,
                normalize::Lang::Ja,
                normalize::Lang::Ko,
                normalize::Lang::Th,
            ][sel]
        },
    };

    println!(
        "{} {} {}{}",
        t(
            lang,
            "Normalizing to",
            "规范化至",
            "正規化:",
            "정규화:",
            "ปรับภาษาเป็น:",
            "Normalizing to"
        ),
        target.name().cyan().bold(),
        if dry_run {
            " [dry-run]".yellow().to_string()
        } else {
            String::new()
        },
        if content_only {
            " [content only]".dimmed().to_string()
        } else if files_only {
            " [files only]".dimmed().to_string()
        } else {
            String::new()
        },
    );

    let root = std::path::Path::new(".");
    match normalize::normalize_project(root, target, dry_run, content_only, files_only) {
        Ok(stats) => {
            println!();
            if dry_run {
                println!(
                    "{}",
                    t(
                        lang,
                        "Dry run complete — no files changed.",
                        "模拟完成 — 未修改任何文件。",
                        "ドライランが完了しました — 変更なし。",
                        "드라이 런 완료 — 변경 없음.",
                        "ทดลองเสร็จสิ้น — ไม่มีไฟล์ถูกเปลี่ยน",
                        "Dry run complete — no files changed.",
                    )
                    .dimmed()
                );
            }
            println!(
                "  {} {} {}",
                "✓".green(),
                stats.files_rewritten,
                t(
                    lang,
                    "file(s) rewritten",
                    "文件已重写",
                    "ファイル書き換え済み",
                    "파일 재작성됨",
                    "ไฟล์ถูกเขียนใหม่",
                    "file(s) rewritten"
                ),
            );
            println!(
                "  {} {} {}",
                "~".yellow(),
                stats.files_renamed,
                t(
                    lang,
                    "file(s) renamed",
                    "文件已重命名",
                    "ファイル名変更済み",
                    "파일 이름 변경됨",
                    "ไฟล์ถูกเปลี่ยนชื่อ",
                    "file(s) renamed"
                ),
            );
            println!(
                "  {} {} {}",
                "↕".magenta(),
                stats.dirs_renamed,
                t(
                    lang,
                    "folder(s) renamed",
                    "文件夹已重命名",
                    "フォルダ名変更済み",
                    "폴더 이름 변경됨",
                    "โฟลเดอร์ถูกเปลี่ยนชื่อ",
                    "folder(s) renamed"
                ),
            );
        },
        Err(e) => {
            eprintln!("{}: {}", "error".red(), e);
            std::process::exit(1);
        },
    }
    Ok(())
}
