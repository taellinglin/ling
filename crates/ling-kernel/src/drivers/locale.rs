//! Locale/timezone table: real Earth regions grounded in actual UTC offsets,
//! the Moon grounded in the real synodic month (its actual day/night cycle
//! length, tidally locked so it differs from its ~27.3-day sidereal
//! rotation), and a couple of invented celestial/mythological locales named
//! after real figures from Chinese folk religion (月宫/Moon Palace — Chang'e
//! 嫦娥's home; 龙宫/Dragon Palace — the undersea court of the Dragon Kings,
//! from Journey to the West and wider folklore) rather than made up whole
//! cloth, per the request this was built for.
//!
//! Honest limits: there is no calendar/date math here beyond `arch::rtc`'s
//! Unix timestamp — a fictional planet's "timezone" is necessarily symbolic
//! (an offset applied to Earth-UTC, not a real independent calendar), and is
//! disclosed as such via `Kind::Celestial` rather than presented as if it
//! were a real astronomical authority. The Moon's day-cycle fraction is a
//! real computation (`moon_day_progress`), not decoration.

use crate::arch::rtc;

#[derive(Clone, Copy, PartialEq)]
pub enum Kind {
    /// A real Earth timezone — `utc_offset_min` is a genuine UTC offset.
    Earth,
    /// A locale with no real independent clock of its own — `utc_offset_min`
    /// is a symbolic offset from Earth-UTC for flavor/sorting, not a claim
    /// about how time actually works there.
    Celestial,
}

pub struct Locale {
    /// Stable short id, e.g. "zh-CN" / "moon" / "ling-country".
    pub id: &'static str,
    /// Display name in the locale's own script (UTF-8) — rendered via
    /// `drivers::font_unicode` when non-ASCII, `font8x8` otherwise.
    pub native_name: &'static str,
    /// Latin/English name — always renderable, used in text-mode contexts
    /// that can't show the native script (see this module's doc + the
    /// installer's text-mode picker).
    pub latin_name: &'static str,
    pub utc_offset_min: i32,
    pub kind: Kind,
    /// Whether this locale's UI text should render through the Daemon
    /// glyph reskin (see `font_unicode`'s doc) instead of plain Latin.
    pub uses_daemon_script: bool,
    pub flag_id: u32,
}

pub static LOCALES: &[Locale] = &[
    Locale {
        id: "en-US",
        native_name: "English (US)",
        latin_name: "English (US)",
        utc_offset_min: -5 * 60,
        kind: Kind::Earth,
        uses_daemon_script: false,
        flag_id: 0,
    },
    Locale {
        id: "en-GB",
        native_name: "English (UK)",
        latin_name: "English (UK)",
        utc_offset_min: 0,
        kind: Kind::Earth,
        uses_daemon_script: false,
        flag_id: 1,
    },
    Locale {
        id: "zh-CN",
        native_name: "简体中文",
        latin_name: "Chinese (Simplified)",
        utc_offset_min: 8 * 60,
        kind: Kind::Earth,
        uses_daemon_script: false,
        flag_id: 2,
    },
    Locale {
        id: "ko-KR",
        native_name: "한국어",
        latin_name: "Korean",
        utc_offset_min: 9 * 60,
        kind: Kind::Earth,
        uses_daemon_script: false,
        flag_id: 3,
    },
    Locale {
        id: "th-TH",
        native_name: "ภาษาไทย",
        latin_name: "Thai",
        utc_offset_min: 7 * 60,
        kind: Kind::Earth,
        uses_daemon_script: false,
        flag_id: 4,
    },
    // -- Fictional / celestial locales -----------------------------------
    Locale {
        id: "ling-country",
        native_name: "Ling Country",
        latin_name: "Ling Country",
        // 8 is a lucky number in Chinese culture (sounds like "prosper") --
        // an offset no real timezone uses, deliberately: this locale isn't
        // claiming to be a real place.
        utc_offset_min: 8 * 60 + 8,
        kind: Kind::Celestial,
        uses_daemon_script: true,
        flag_id: 5,
    },
    Locale {
        id: "new-thailand",
        native_name: "New Thailand",
        latin_name: "New Thailand",
        utc_offset_min: 7 * 60 + 30,
        kind: Kind::Celestial,
        uses_daemon_script: false,
        flag_id: 6,
    },
    Locale {
        id: "moon-palace",
        native_name: "月宫",
        latin_name: "Moon Palace (Chang'e, 嫦娥)",
        // Symbolic sort key only -- see `moon_day_progress` for the real
        // computation this locale's clock actually runs on.
        utc_offset_min: 0,
        kind: Kind::Celestial,
        uses_daemon_script: false,
        flag_id: 7,
    },
    Locale {
        id: "dragon-palace",
        native_name: "龙宫",
        latin_name: "Dragon Palace (water world, ~3x Earth radius)",
        // Invented rotation period for this water world: 34h, expressed as
        // an offset purely for display ordering, not a real clock model.
        utc_offset_min: 34 * 60,
        kind: Kind::Celestial,
        uses_daemon_script: false,
        flag_id: 8,
    },
];

pub fn count() -> usize {
    LOCALES.len()
}

pub fn get(i: usize) -> Option<&'static Locale> {
    LOCALES.get(i)
}

/// Index of the locale with this `id` (case-insensitive -- a typed prompt
/// shouldn't reject "ZH-CN"/"zh-cn" just because the table's own id is
/// mixed-case), or 0 (`en-US`, the table's first/default entry) if `id` is
/// empty or matches nothing, so a blank/mistyped entry falls back to a sane
/// default rather than an error mid-setup.
pub fn index_of_id(id: &str) -> usize {
    LOCALES
        .iter()
        .position(|l| l.id.eq_ignore_ascii_case(id))
        .unwrap_or(0)
}

static mut SELECTED: Option<usize> = None;

/// Record the user's locale choice — kernel-side state for the same reason
/// `wm_liquid`'s spring simulation is (see that module's doc): `.ling`'s
/// AOT kernel path has no mutable-reassignment construct, so a selection
/// made on one frame of an infinite event loop can't be remembered by a
/// `bind` on the next frame. The picker screen polls [`selected`] each
/// frame instead.
pub fn select(i: usize) {
    unsafe { SELECTED = Some(i) };
}

pub fn selected() -> Option<usize> {
    unsafe { SELECTED }
}

/// Minimal UI translation: map an English source string to the currently
/// selected locale, falling back to the English key itself when a locale has
/// no entry (so untranslated call sites still read correctly). Keyed by the
/// English string so the code stays readable at the call site.
///
/// Curated, not exhaustive: only the app vocabulary whose glyphs are baked
/// into the Unicode atlas (see `ling`'s `ZH_CHARS`/`KO_CHARS` + this repo's
/// `font_unicode` doc). Translated strings contain non-ASCII, so callers MUST
/// draw the result with `font_unicode::draw_utf8_str`, not the ASCII-only
/// `font8x8`. Thai and the fictional locales fall back to English for now.
pub fn tr(key: &'static str) -> &'static str {
    tr_lookup(key).unwrap_or(key)
}

/// Translation lookup by runtime string: `Some(translated)` for a known key in
/// the current locale, `None` otherwise. The FFI (`ling_kernel_tr`) uses this
/// to echo the original English when there's no translation, since the input
/// isn't `'static`. Kernel-internal callers use `tr()` (which falls back to
/// the `'static` key literal).
pub fn tr_lookup(key: &str) -> Option<&'static str> {
    let id = selected().and_then(|i| LOCALES.get(i)).map(|l| l.id).unwrap_or("en-US");
    match id {
        // The two celestial Chinese-culture locales (月宫 / 龙宫) speak Chinese.
        "zh-CN" | "moon-palace" | "dragon-palace" => tr_zh(key),
        "ko-KR" => tr_ko(key),
        "th-TH" | "new-thailand" => tr_th(key),
        // en-US / en-GB use English. Ling Country also returns English strings
        // -- its constructed script is a *reskin* of Latin, so the words stay
        // English and `render_daemon()` tells the drawing code to paint them in
        // the Daemon glyphs (the daemon atlas is keyed by ASCII).
        _ => None,
    }
}

/// Should UI text be painted through the Daemon (constructed-script) reskin?
/// True only for the Ling Country locale; callers pass this as the `daemon`
/// flag to `font_unicode::draw_utf8_str` so localized strings render in-script.
pub fn render_daemon() -> bool {
    selected().and_then(|i| LOCALES.get(i)).map(|l| l.uses_daemon_script).unwrap_or(false)
}

fn tr_zh(key: &str) -> Option<&'static str> {
    Some(match key {
        // Window / app titles.
        "Files" => "文件",
        "Settings" => "设置",
        "Terminal" => "终端",
        "Editor" => "编辑器",
        "Messenger" => "信使",
        "Gallery" => "图库",
        "Media Player" => "媒体播放器",
        "Packages" => "软件包",
        "About LingOS" => "关于 LingOS",
        // File browser.
        "(empty)" => "(空)",
        "(unreadable)" => "(无法读取)",
        "enter: open   backspace: up" => "回车打开  退格上级",
        "backspace: back" => "退格返回",
        // Shared buttons.
        "Apply" => "应用",
        "OK" => "确定",
        // Installer step names + prompts.
        "Keyboard" => "键盘",
        "Language" => "语言",
        "Locale" => "区域",
        "Disk" => "磁盘",
        "Root" => "管理员",
        "User" => "用户",
        "Network" => "网络",
        "Finish" => "完成",
        "Choose your keyboard layout" => "选择键盘布局",
        "Choose your system language" => "选择系统语言",
        "Choose your locale and timezone" => "选择区域和时区",
        "up/down to move, enter to select" => "上下移动，回车选择",
        "Install" => "安装",
        "Next" => "下一步",
        "Back" => "上一步",
        "Erase disk and install LingOS" => "清除磁盘并安装 LingOS",
        "Installing..." => "正在安装...",
        "Welcome to LingOS" => "欢迎使用 LingOS",
        // Installer step titles.
        "Confirm erase" => "确认清除",
        "Choose the installation disk" => "选择安装磁盘",
        "Root account" => "管理员账户",
        "User account" => "用户账户",
        "Finishing up" => "正在完成",
        "Setup" => "安装设置",
        "Erasing disk" => "正在清除",
        // Installer prompts + instructions.
        "Password:" => "密码:",
        "Confirm password:" => "确认密码:",
        "Type ERASE (all caps) to wipe this disk and install:" => "输入 ERASE 清除此磁盘并安装:",
        "Hostname for this system:" => "此系统的主机名:",
        "Create a user account (username, blank to skip):" => "创建用户 (留空跳过):",
        "Password for this account:" => "此账户的密码:",
        "Set a password for the root account:" => "设置管理员密码:",
        "Start the SSH server at boot? (yes/no):" => "开机启动 SSH 服务? (yes/no):",
        "Package set (1-7):" => "软件包组 (1-7):",
        "Package name from the registry (blank to skip):" => "软件包名 (留空跳过):",
        "Choose a package set to install now:" => "选择要安装的软件包组:",
        "Detected disk (the only one this kernel drives today):" => "检测到的磁盘 (目前仅支持一个):",
        "Installing ERASES this disk: bootloader, filesystem, everything." => "安装将清除此磁盘的一切.",
        // Installer status lines.
        "Erasing boot sectors and filesystem headers..." => "正在清除引导扇区和文件系统...",
        "Mounting disk..." => "正在挂载磁盘...",
        "Setting up network..." => "正在设置网络...",
        "Installing bootloader..." => "正在安装引导程序...",
        "Done. Remove the installation media and reboot." => "完成. 请移除安装介质并重启.",
        "Package set installed." => "软件包已安装.",
        _ => return None,
    })
}

fn tr_ko(key: &str) -> Option<&'static str> {
    Some(match key {
        "Files" => "파일",
        "Settings" => "설정",
        "Terminal" => "터미널",
        "Editor" => "편집기",
        "Messenger" => "메신저",
        "Gallery" => "갤러리",
        "Media Player" => "미디어 플레이어",
        "Packages" => "패키지",
        "About LingOS" => "LingOS 정보",
        "(empty)" => "(비었음)",
        "(unreadable)" => "(읽기 실패)",
        "enter: open   backspace: up" => "엔터 열기  백스페이스 위로",
        "backspace: back" => "백스페이스 뒤로",
        "Apply" => "적용",
        "OK" => "확인",
        "Keyboard" => "키보드",
        "Language" => "언어",
        "Locale" => "지역",
        "Disk" => "디스크",
        "Root" => "루트",
        "User" => "사용자",
        "Network" => "네트워크",
        "Finish" => "완료",
        "Choose your keyboard layout" => "키보드 배열 선택",
        "Choose your system language" => "시스템 언어 선택",
        "Choose your locale and timezone" => "지역 및 시간대 선택",
        "up/down to move, enter to select" => "위아래 이동, 엔터 선택",
        "Install" => "설치",
        "Next" => "다음",
        "Back" => "뒤로",
        "Erase disk and install LingOS" => "디스크를 지우고 LingOS 설치",
        "Installing..." => "설치 중...",
        "Welcome to LingOS" => "LingOS 에 오신 것을 환영합니다",
        "Confirm erase" => "지우기 확인",
        "Choose the installation disk" => "설치 디스크 선택",
        "Root account" => "루트 계정",
        "User account" => "사용자 계정",
        "Finishing up" => "마무리",
        "Setup" => "설정",
        "Erasing disk" => "디스크 지우는 중",
        "Password:" => "비밀번호:",
        "Confirm password:" => "비밀번호 확인:",
        "Type ERASE (all caps) to wipe this disk and install:" => "ERASE 입력하여 디스크 지우고 설치:",
        "Hostname for this system:" => "호스트 이름:",
        "Create a user account (username, blank to skip):" => "사용자 만들기 (비우면 건너뜀):",
        "Password for this account:" => "계정 비밀번호:",
        "Set a password for the root account:" => "루트 비밀번호 설정:",
        "Start the SSH server at boot? (yes/no):" => "부팅 시 SSH 시작? (yes/no):",
        "Package set (1-7):" => "패키지 세트 (1-7):",
        "Package name from the registry (blank to skip):" => "패키지 이름 (비우면 건너뜀):",
        "Choose a package set to install now:" => "설치할 패키지 세트 선택:",
        "Detected disk (the only one this kernel drives today):" => "감지된 디스크 (현재 하나만 지원):",
        "Installing ERASES this disk: bootloader, filesystem, everything." => "설치는 이 디스크를 모두 지웁니다.",
        "Erasing boot sectors and filesystem headers..." => "부트 섹터와 파일시스템 지우는 중...",
        "Mounting disk..." => "디스크 마운트 중...",
        "Setting up network..." => "네트워크 설정 중...",
        "Installing bootloader..." => "부트로더 설치 중...",
        "Done. Remove the installation media and reboot." => "완료. 미디어를 빼고 재부팅.",
        "Package set installed." => "패키지 설치됨.",
        _ => return None,
    })
}

// Thai (also used for the fictional "New Thailand"). Note: `font_unicode` does
// not do Thai shaping, so combining vowel/tone marks render as their own cells
// -- readable, not typographically perfect (disclosed in that module's doc).
fn tr_th(key: &str) -> Option<&'static str> {
    Some(match key {
        "Files" => "ไฟล์",
        "Settings" => "ตั้งค่า",
        "Terminal" => "เทอร์มินัล",
        "Editor" => "แก้ไข",
        "Messenger" => "แชท",
        "Gallery" => "แกลเลอรี",
        "Media Player" => "มีเดีย",
        "Packages" => "แพ็กเกจ",
        "About LingOS" => "เกี่ยวกับ LingOS",
        "(empty)" => "(ว่าง)",
        "(unreadable)" => "(อ่านไม่ได้)",
        "enter: open   backspace: up" => "Enter เปิด  Backspace ขึ้น",
        "backspace: back" => "Backspace ย้อน",
        "Apply" => "ใช้",
        "OK" => "ตกลง",
        "Keyboard" => "แป้นพิมพ์",
        "Language" => "ภาษา",
        "Locale" => "ภูมิภาค",
        "Disk" => "ดิสก์",
        "Root" => "รูท",
        "User" => "ผู้ใช้",
        "Network" => "เครือข่าย",
        "Finish" => "เสร็จ",
        "Choose your keyboard layout" => "เลือกแป้นพิมพ์",
        "Choose your system language" => "เลือกภาษาระบบ",
        "Choose your locale and timezone" => "เลือกภูมิภาคและเขตเวลา",
        "up/down to move, enter to select" => "ขึ้นลงเลื่อน กด Enter เลือก",
        "Install" => "ติดตั้ง",
        "Next" => "ถัดไป",
        "Back" => "ย้อน",
        "Erase disk and install LingOS" => "ล้างดิสก์และติดตั้ง LingOS",
        "Installing..." => "กำลังติดตั้ง...",
        "Welcome to LingOS" => "ยินดีต้อนรับสู่ LingOS",
        "Confirm erase" => "ยืนยันการล้าง",
        "Choose the installation disk" => "เลือกดิสก์ติดตั้ง",
        "Root account" => "บัญชีรูท",
        "User account" => "บัญชีผู้ใช้",
        "Finishing up" => "กำลังเสร็จ",
        "Setup" => "ตั้งค่า",
        "Erasing disk" => "กำลังล้างดิสก์",
        "Password:" => "รหัสผ่าน:",
        "Confirm password:" => "ยืนยันรหัสผ่าน:",
        "Type ERASE (all caps) to wipe this disk and install:" => "พิมพ์ ERASE เพื่อล้างดิสก์และติดตั้ง:",
        "Hostname for this system:" => "ชื่อโฮสต์:",
        "Create a user account (username, blank to skip):" => "สร้างผู้ใช้ (เว้นว่างข้าม):",
        "Password for this account:" => "รหัสผ่านบัญชี:",
        "Set a password for the root account:" => "ตั้งรหัสผ่านรูท:",
        "Start the SSH server at boot? (yes/no):" => "เริ่ม SSH เมื่อบูต? (yes/no):",
        "Package set (1-7):" => "ชุดแพ็กเกจ (1-7):",
        "Package name from the registry (blank to skip):" => "ชื่อแพ็กเกจ (เว้นว่างข้าม):",
        "Choose a package set to install now:" => "เลือกชุดแพ็กเกจติดตั้ง:",
        "Detected disk (the only one this kernel drives today):" => "ดิสก์ที่พบ (รองรับหนึ่งตัว):",
        "Installing ERASES this disk: bootloader, filesystem, everything." => "การติดตั้งจะล้างดิสก์นี้ทั้งหมด.",
        "Erasing boot sectors and filesystem headers..." => "กำลังล้างบูตเซกเตอร์...",
        "Mounting disk..." => "กำลังเมานต์ดิสก์...",
        "Setting up network..." => "กำลังตั้งค่าเครือข่าย...",
        "Installing bootloader..." => "กำลังติดตั้งบูตโหลดเดอร์...",
        "Done. Remove the installation media and reboot." => "เสร็จ. นำสื่อออกแล้วรีบูต.",
        "Package set installed." => "ติดตั้งแพ็กเกจแล้ว.",
        _ => return None,
    })
}

/// Restore the locale the installer persisted to `/locale` (its stable id,
/// e.g. "zh-CN") and select the matching entry. Returns true if a saved id was
/// found and matched. Called at installed-disk boot so the login screen and
/// desktop come up in the language chosen during install, instead of defaulting
/// back to English.
pub fn restore() -> bool {
    let mut buf = [0u8; 64];
    let Ok(Some(n)) = crate::fs::lingfs::read_file_all("/locale", &mut buf) else {
        return false;
    };
    let Ok(id) = core::str::from_utf8(&buf[..n]) else { return false };
    let id = id.trim();
    if id.is_empty() {
        return false;
    }
    for (i, loc) in LOCALES.iter().enumerate() {
        if loc.id == id {
            select(i);
            return true;
        }
    }
    false
}

/// Clear the current pick, so [`selected`] reports "unselected" again --
/// lets a multi-step wizard (language, then a separate locale/timezone
/// step) run the same picker screen twice over the same table for two
/// independent choices, rather than the second call just re-confirming the
/// first pick instantly.
pub fn reset() {
    unsafe { SELECTED = None };
    unsafe { CURSOR = 0 };
}

static mut CURSOR: usize = 0;

/// The picker's currently-highlighted row -- separate from [`selected`]
/// (which only becomes `Some` once the user actually confirms with
/// Enter), for real arrow-key/Enter navigation instead of typing a raw
/// digit for the row you want. Kernel-side for the same reason as
/// `selected`: no persistent mutable state on the `.ling` side of an
/// infinite per-frame loop.
pub fn cursor() -> usize {
    unsafe { CURSOR }
}

pub fn cursor_up() {
    unsafe {
        CURSOR = if CURSOR == 0 { LOCALES.len() - 1 } else { CURSOR - 1 };
    }
}

pub fn cursor_down() {
    unsafe {
        CURSOR = (CURSOR + 1) % LOCALES.len();
    }
}

/// Confirm whatever row is currently highlighted -- the Enter-key action.
pub fn confirm_cursor() {
    unsafe { SELECTED = Some(CURSOR) };
}

/// How far through its real day/night cycle the Moon currently is, as a
/// permille (0..1000) of a full cycle. Grounded in the actual synodic month
/// (29.530589 days — the Moon's tidally-locked day length, distinct from
/// its ~27.3-day sidereal rotation) applied to the real RTC-derived Unix
/// timestamp, not an invented number.
pub fn moon_day_progress_permille() -> u32 {
    const SYNODIC_MONTH_SECS: i64 = 2_551_443; // 29.530589 days
    // A fixed reference new-moon epoch (2000-01-06 18:14 UTC, a commonly
    // cited J2000-era reference new moon) so the phase is at least
    // approximately right rather than arbitrary from an epoch-zero start.
    const REFERENCE_NEW_MOON_UNIX: i64 = 947_182_440;
    let ts = rtc::unix_timestamp();
    let elapsed = (ts - REFERENCE_NEW_MOON_UNIX).rem_euclid(SYNODIC_MONTH_SECS);
    (elapsed * 1000 / SYNODIC_MONTH_SECS) as u32
}
