//! Threads: isolates + channels (ling-wallet LING_API.md §1).
//!
//! Design
//! ------
//! * **Isolates.** `thread_spawn` starts a fresh, lightweight [`Interpreter`]
//!   (`Interpreter::new_isolate`: no audio device, no window, nothing that has
//!   to live on the main thread) on its own OS thread. Interpreters are full of
//!   `Rc`/`RefCell` and are never shared; the only things that cross threads
//!   are [`SendValue`] trees (owned, `Send + Sync`), channel endpoints and the
//!   write-once shared store. Isolates always run on the tree-walker.
//! * **Values.** [`SendValue`] is a deep, frozen copy of a Ling value. Strings
//!   are *moved* (not copied) out of a `SendValue` when it is delivered, so a
//!   channel hop costs one copy (at send). Functions are rejected.
//! * **Handles are capabilities.** Every channel endpoint and thread has a
//!   128-bit random (OS RNG) lowercase-hex handle. Each isolate owns a handle
//!   table; an operation on a string that is not in the caller's table is a
//!   runtime error, so handles cannot be forged or guessed. When a value that
//!   contains a channel handle (that the sender holds) is sent — as a message,
//!   a spawn argument, or a thread's return value — the endpoint travels with
//!   it and is installed in the receiver's table under the same string.
//! * **Channels** are bounded MPMC queues (Mutex + Condvars, one shared
//!   `Signal` per waiting `chan_select`). Endpoints are reference counted per
//!   holder: when every send endpoint is gone (all holders exited) the channel
//!   ends once drained; when every receive endpoint is gone, sends fail with
//!   `closed` and queued messages are dropped. Blocking waits sleep in short
//!   slices so a cancelled isolate wakes promptly.
//! * **Modules** are parsed once per process: [`parse_cached`] keeps an
//!   `Arc<Program>` per canonical path (the AST is plain `Box`/`Vec`/`String`,
//!   so it is `Send + Sync`), revalidated against the file's current source
//!   text. `load_module` (`use`) goes through the same cache.
//! * **Capabilities.** An isolate's `caps` is `Some(set)`; the main thread's is
//!   `None` (everything). `call_named_inner` calls [`Interpreter::isolate_guard`]
//!   for every builtin call made inside an isolate; it maps the builtin name
//!   (English names by prefix, plus every multilingual alias of those arms —
//!   the alias tables below were generated from the dispatch `match` arms) to
//!   the capability it needs, or rejects GUI/input builtins outright.

use super::{new_env, EvalErr, EvalResult, Interpreter, Value};
use crate::parser::ast::Program;
use std::collections::{HashMap, HashSet, VecDeque};
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Condvar, Mutex, MutexGuard, OnceLock, RwLock};
use std::time::{Duration, Instant};

// ─── Limits ──────────────────────────────────────────────────────────────────

/// Approximate maximum size of one message / argument list / return value.
pub(crate) const MAX_MSG_BYTES: usize = 64 << 20;
/// Maximum number of live isolates in the process.
pub(crate) const MAX_THREADS: usize = 1024;
/// Maximum channel capacity (messages queued).
pub(crate) const MAX_CAPACITY: usize = 65_536;
/// Every capability a script can name.
pub(crate) const ALL_CAPS: &[&str] = &["fs", "db", "net", "p2p", "proc", "keys", "clipboard"];
/// Blocking waits wake at least this often to check for cancellation.
const SLICE: Duration = Duration::from_millis(20);
/// OS stack per isolate (reserved, committed lazily): the tree-walker recurses.
const ISOLATE_STACK: usize = 32 << 20;

static LIVE_THREADS: AtomicUsize = AtomicUsize::new(0);

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    // A panicking isolate must never wedge everyone else: ignore poisoning
    // (all state guarded here stays consistent across a panic).
    m.lock().unwrap_or_else(|e| e.into_inner())
}

fn new_handle() -> String {
    use rand::RngCore;
    let mut b = [0u8; 16];
    rand::rngs::OsRng.fill_bytes(&mut b);
    let mut s = String::with_capacity(32);
    for x in b {
        s.push_str(&format!("{x:02x}"));
    }
    s
}

// ─── Frozen, thread-safe values ──────────────────────────────────────────────

/// A deep-frozen Ling value that may cross isolates. Owned tree; the shared
/// store wraps it in an `Arc` so reads are lock-free clones of the pointer.
#[derive(Debug, Clone)]
pub(crate) enum SendValue {
    Str(String),
    Number(f64),
    Bool(bool),
    Unit,
    List(Vec<SendValue>),
    Ok(Box<SendValue>),
    Err(Box<SendValue>),
    Struct { name: String, fields: Vec<(String, SendValue)> },
    Variant { enum_name: String, variant: String, payload: Vec<SendValue> },
    /// A channel send endpoint travelling with its handle string.
    Tx(String, SendEnd),
    /// A channel receive endpoint travelling with its handle string.
    Rx(String, RecvEnd),
}

fn wipe_string(s: &mut String) {
    // SAFETY: we only write zero bytes (valid UTF-8) and keep the length.
    let bytes = unsafe { s.as_mut_vec() };
    for b in bytes.iter_mut() {
        unsafe { std::ptr::write_volatile(b, 0) };
    }
    std::sync::atomic::compiler_fence(Ordering::SeqCst);
}

impl SendValue {
    /// Overwrite every string payload byte with zeros (secure channels).
    fn wipe(&mut self) {
        match self {
            SendValue::Str(s) => wipe_string(s),
            SendValue::List(v) | SendValue::Variant { payload: v, .. } => v.iter_mut().for_each(|x| x.wipe()),
            SendValue::Ok(b) | SendValue::Err(b) => b.wipe(),
            SendValue::Struct { fields, .. } => fields.iter_mut().for_each(|(_, x)| x.wipe()),
            _ => {},
        }
    }
}

// ─── Channels ────────────────────────────────────────────────────────────────

#[derive(Default)]
struct Signal {
    flag: Mutex<bool>,
    cv: Condvar,
}

impl Signal {
    fn raise(&self) {
        *lock(&self.flag) = true;
        self.cv.notify_one();
    }
    /// Wait up to `d` for a raise; clears the flag.
    fn wait(&self, d: Duration) {
        let mut f = lock(&self.flag);
        if !*f {
            f = self.cv.wait_timeout(f, d).map(|r| r.0).unwrap_or_else(|e| e.into_inner().0);
        }
        *f = false;
    }
}

#[derive(Default)]
struct ChanState {
    q: VecDeque<SendValue>,
    /// Send side closed (explicitly, or every send endpoint dropped).
    tx_closed: bool,
    /// Receive side closed (explicitly, or every receive endpoint dropped).
    rx_closed: bool,
    senders: usize,
    receivers: usize,
    watchers: Vec<Arc<Signal>>,
}

pub(crate) struct Chan {
    cap: usize,
    secure: bool,
    st: Mutex<ChanState>,
    not_empty: Condvar,
    not_full: Condvar,
}

impl std::fmt::Debug for Chan {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "<chan cap={} secure={}>", self.cap, self.secure)
    }
}

impl Drop for Chan {
    fn drop(&mut self) {
        if self.secure {
            let st = self.st.get_mut().unwrap_or_else(|e| e.into_inner());
            st.q.iter_mut().for_each(|v| v.wipe());
        }
    }
}

enum SendErr {
    Full,
    Closed,
    Cancelled,
}
enum RecvErr {
    Empty,
    Timeout,
    Closed,
    Cancelled,
}

fn is_cancelled(c: Option<&AtomicBool>) -> bool {
    c.map(|c| c.load(Ordering::Relaxed)).unwrap_or(false)
}

impl Chan {
    fn new(cap: usize, secure: bool) -> Arc<Chan> {
        Arc::new(Chan {
            cap,
            secure,
            st: Mutex::new(ChanState::default()),
            not_empty: Condvar::new(),
            not_full: Condvar::new(),
        })
    }

    fn discard(&self, mut v: SendValue) {
        if self.secure {
            v.wipe();
        }
        drop(v);
    }

    fn wake_all(&self, watchers: Vec<Arc<Signal>>) {
        self.not_empty.notify_all();
        self.not_full.notify_all();
        watchers.iter().for_each(|w| w.raise());
    }

    /// `block = false` → `Full` instead of waiting.
    fn send(&self, v: SendValue, block: bool, cancel: Option<&AtomicBool>) -> Result<(), SendErr> {
        let mut st = lock(&self.st);
        loop {
            if st.tx_closed || st.rx_closed {
                drop(st);
                self.discard(v);
                return Err(SendErr::Closed);
            }
            if st.q.len() < self.cap {
                st.q.push_back(v);
                let watchers = if st.watchers.is_empty() { Vec::new() } else { st.watchers.clone() };
                drop(st);
                self.not_empty.notify_one();
                watchers.iter().for_each(|w| w.raise());
                return Ok(());
            }
            if !block {
                drop(st);
                self.discard(v);
                return Err(SendErr::Full);
            }
            if is_cancelled(cancel) {
                drop(st);
                self.discard(v);
                return Err(SendErr::Cancelled);
            }
            st = self.not_full.wait_timeout(st, SLICE).map(|r| r.0).unwrap_or_else(|e| e.into_inner().0);
        }
    }

    /// `deadline = None` with `block` → wait forever; `!block` → poll once.
    fn recv(&self, block: bool, deadline: Option<Instant>, cancel: Option<&AtomicBool>) -> Result<SendValue, RecvErr> {
        let mut st = lock(&self.st);
        loop {
            if let Some(v) = st.q.pop_front() {
                drop(st);
                self.not_full.notify_one();
                return Ok(v);
            }
            if st.tx_closed || st.rx_closed {
                return Err(RecvErr::Closed);
            }
            if !block {
                return Err(RecvErr::Empty);
            }
            if is_cancelled(cancel) {
                return Err(RecvErr::Cancelled);
            }
            let wait = match deadline {
                Some(d) => {
                    let now = Instant::now();
                    if now >= d {
                        return Err(RecvErr::Timeout);
                    }
                    (d - now).min(SLICE)
                },
                None => SLICE,
            };
            st = self.not_empty.wait_timeout(st, wait).map(|r| r.0).unwrap_or_else(|e| e.into_inner().0);
        }
    }

    /// Returns true if the side was open (state changed).
    fn close(&self, send_side: bool) -> bool {
        let mut st = lock(&self.st);
        let changed;
        let mut drained = VecDeque::new();
        if send_side {
            changed = !st.tx_closed;
            st.tx_closed = true;
        } else {
            changed = !st.rx_closed;
            st.rx_closed = true;
            drained = std::mem::take(&mut st.q);
        }
        let watchers = st.watchers.clone();
        drop(st);
        // Drop drained messages outside the lock: they may hold endpoints
        // (possibly of this very channel) whose Drop locks a channel.
        drained.into_iter().for_each(|v| self.discard(v));
        self.wake_all(watchers);
        changed
    }

    fn len(&self) -> usize {
        lock(&self.st).q.len()
    }
}

/// Counted send endpoint: one per holder (isolate table entry or in-flight
/// message). The last one dropped ends the channel (after draining).
#[derive(Debug)]
pub(crate) struct SendEnd(Arc<Chan>);
/// Counted receive endpoint; the last one dropped closes the channel.
#[derive(Debug)]
pub(crate) struct RecvEnd(Arc<Chan>);

impl SendEnd {
    fn new(c: Arc<Chan>) -> Self {
        lock(&c.st).senders += 1;
        SendEnd(c)
    }
}
impl Clone for SendEnd {
    fn clone(&self) -> Self {
        SendEnd::new(self.0.clone())
    }
}
impl Drop for SendEnd {
    fn drop(&mut self) {
        let mut st = lock(&self.0.st);
        st.senders -= 1;
        if st.senders == 0 && !st.tx_closed {
            st.tx_closed = true;
            let w = st.watchers.clone();
            drop(st);
            self.0.wake_all(w);
        }
    }
}
impl RecvEnd {
    fn new(c: Arc<Chan>) -> Self {
        lock(&c.st).receivers += 1;
        RecvEnd(c)
    }
}
impl Clone for RecvEnd {
    fn clone(&self) -> Self {
        RecvEnd::new(self.0.clone())
    }
}
impl Drop for RecvEnd {
    fn drop(&mut self) {
        let last = {
            let mut st = lock(&self.0.st);
            st.receivers -= 1;
            st.receivers == 0
        };
        if last {
            self.0.close(false);
        }
    }
}

// ─── Threads ─────────────────────────────────────────────────────────────────

type ThreadResult = Result<SendValue, String>;

pub(crate) struct ThreadRec {
    cancel: Arc<AtomicBool>,
    done: Mutex<Option<ThreadResult>>,
    cv: Condvar,
    join: Mutex<Option<std::thread::JoinHandle<()>>>,
}

impl ThreadRec {
    fn finish(&self, r: ThreadResult) {
        *lock(&self.done) = Some(r);
        self.cv.notify_all();
    }
    /// Wait for the result (`None` deadline = forever). `None` on timeout or
    /// when the *waiting* isolate is cancelled (`Err("cancelled")`).
    fn wait(&self, deadline: Option<Instant>, cancel: Option<&AtomicBool>) -> Option<Result<ThreadResult, ()>> {
        let mut d = lock(&self.done);
        loop {
            if let Some(r) = d.as_ref() {
                let r = r.clone();
                drop(d);
                if let Some(j) = lock(&self.join).take() {
                    let _ = j.join();
                }
                return Some(Ok(r));
            }
            if is_cancelled(cancel) {
                return Some(Err(()));
            }
            let wait = match deadline {
                Some(dl) => {
                    let now = Instant::now();
                    if now >= dl {
                        return None;
                    }
                    (dl - now).min(SLICE)
                },
                None => SLICE,
            };
            d = self.cv.wait_timeout(d, wait).map(|r| r.0).unwrap_or_else(|e| e.into_inner().0);
        }
    }
}

// ─── Per-isolate state ───────────────────────────────────────────────────────

pub(crate) enum Entry {
    Tx(SendEnd),
    Rx(RecvEnd),
    Thread(Arc<ThreadRec>),
}

/// Per-isolate state for this builtin family.
#[derive(Default)]
pub(crate) struct ThreadsState {
    /// handle string → capability held by this isolate.
    table: HashMap<String, Entry>,
    /// Set by the parent's `thread_cancel`; `None` on the main thread.
    pub(crate) cancel: Option<Arc<AtomicBool>>,
    /// This isolate's own handle (lazily invented on the main thread).
    self_handle: Option<String>,
    /// Rotating start index so `chan_select` is fair.
    select_rr: usize,
}

// ─── Process-wide caches ─────────────────────────────────────────────────────

struct CachedModule {
    source: String,
    program: Arc<Program>,
}

fn module_cache() -> &'static Mutex<HashMap<String, CachedModule>> {
    static C: OnceLock<Mutex<HashMap<String, CachedModule>>> = OnceLock::new();
    C.get_or_init(Default::default)
}

/// Parse `source` (the current text of the file whose canonical path is
/// `key`), reusing the process-wide parsed AST when the text is unchanged.
pub(crate) fn parse_cached_source(key: &str, source: String) -> Result<Arc<Program>, String> {
    if let Some(m) = lock(module_cache()).get(key) {
        if m.source == source {
            return Ok(m.program.clone());
        }
    }
    let program = Arc::new(crate::parser::parse(&source).map_err(|e| e.to_string())?);
    lock(module_cache()).insert(key.to_string(), CachedModule { source, program: program.clone() });
    Ok(program)
}

/// Read + parse a file through the cache. Returns (canonical path, program).
pub(crate) fn parse_cached(path: &Path) -> Result<(String, Arc<Program>), String> {
    let canonical = path.canonicalize().unwrap_or_else(|_| path.to_path_buf()).to_string_lossy().to_string();
    let src = std::fs::read_to_string(path).map_err(|e| format!("cannot read '{}': {e}", path.display()))?;
    let prog = parse_cached_source(&canonical, src).map_err(|e| format!("parse error in '{}': {e}", path.display()))?;
    Ok((canonical, prog))
}

fn shared_store() -> &'static RwLock<HashMap<String, Arc<SendValue>>> {
    static S: OnceLock<RwLock<HashMap<String, Arc<SendValue>>>> = OnceLock::new();
    S.get_or_init(Default::default)
}

// ─── Helpers ─────────────────────────────────────────────────────────────────

fn ok(v: Value) -> EvalResult {
    Ok(Value::Ok(Box::new(v)))
}
fn bad(msg: impl Into<String>) -> EvalResult {
    Ok(Value::Err(Box::new(Value::Str(msg.into()))))
}
fn rt(msg: impl Into<String>) -> EvalErr {
    EvalErr::Runtime(msg.into())
}
fn arg_str<'a>(name: &str, args: &'a [Value], i: usize) -> Result<&'a str, EvalErr> {
    match args.get(i) {
        Some(Value::Str(s)) => Ok(s),
        _ => Err(rt(format!("{name}: argument {} must be a string", i + 1))),
    }
}
fn arg_num(name: &str, args: &[Value], i: usize) -> Result<f64, EvalErr> {
    match args.get(i) {
        Some(Value::Number(n)) => Ok(*n),
        _ => Err(rt(format!("{name}: argument {} must be a number", i + 1))),
    }
}
fn deadline_ms(ms: f64) -> Option<Instant> {
    if ms < 0.0 || !ms.is_finite() {
        None
    } else {
        Some(Instant::now() + Duration::from_micros((ms * 1000.0) as u64))
    }
}

/// Builtins an isolate may not call without a capability; `"gui"` means
/// main-thread only. English names by prefix; aliases from the dispatch arms.
fn builtin_cap(name: &str) -> Option<&'static str> {
    const PREFIX: &[(&str, &str)] = &[
        ("file_", "fs"), ("remove_", "fs"), ("tar_", "fs"), ("zip_", "fs"), ("fs_", "fs"), ("dlg_", "fs"),
        ("db_", "db"),
        ("net_", "net"), ("http_", "net"), ("lingtp_", "net"), ("ssh_", "net"), ("oauth_", "net"),
        ("daw_clipboard", "clipboard"),
        ("px_", "gui"), ("key_", "gui"), ("mouse_", "gui"), ("pad_", "gui"), ("gamepad_", "gui"),
        ("cursor_", "gui"), ("window_", "gui"), ("mic_", "gui"),
    ];
    for (p, c) in PREFIX {
        if name.starts_with(p) {
            return Some(c);
        }
    }
    Some(match name {
        "read_file" | "write_file" | "append_file" | "list_dir" | "rename_file" | "copy_file" | "make_dir"
        | "script_load" | "icon_convert_to_png" | "base64_decode_to_file" | "image_load" | "image_replace"
        | "image_save" | "music_lrc" | "music_load" | "music_midi_load" | "mesh_load" | "font_load"
        | "record_frame" | "screenshot" | "tm_tex_load" | "svg_begin" | "svg_end" | "audio_sample_load"
        | "llm_load" | "vision_load" | "stt_start" | "stt_align" | "stt_align_srt" => "fs",
        "run_process" => "proc",
        "open_window" | "present" => "gui",
        // db aliases (generated from the dispatch arms)
        "בצע_במסד_נתונים" | "פתח_מסד_נתונים" | "שאילתת_מסד_נתונים" | "اجرای_پایگاه_داده" |
        "استعلم_قاعدة_البيانات" | "افتح_قاعدة_البيانات" | "باز_کردن_پایگاه_داده" |
        "نفّذ_في_قاعدة_البيانات" | "پرسوجوی_پایگاه_داده" | "ڈیٹا_بیس_سوال" | "ڈیٹا_بیس_عمل" |
        "ڈیٹا_بیس_کھولو" | "ฐานข้อมูลถาม" | "ฐานข้อมูลรัน" | "ฐานข้อมูลเปิด" => "db",
        // fs aliases (generated from the dispatch arms)
        "MIDI_لوڈ" | "MIDI読込" | "SVG_ختم" | "SVG_شروع" | "TAR_GZ_فہرست" | "TAR_GZ_پڑھو" |
        "base64파일로저장" | "charger_midi_musique" | "charger_musique" | "charger_police" |
        "charger_échantillon" | "commencer_svg" | "lrc_musique" | "lrc_музыки" | "music_patch" |
        "musik_laden" | "musik_lrc" | "musik_midi_laden" | "sample_laden" | "schriftart_laden" |
        "svg_beenden" | "svg_beginnen" | "terminer_svg" | "загрузить_midi_музыки" |
        "загрузить_музыку" | "загрузить_семпл" | "загрузить_шрифт" | "закончить_svg" |
        "начать_svg" | "גודל_קובץ" | "גיבוב_קובץ" | "העתק_קובץ" | "התחל_SVG" | "זמן_יצירת_קובץ" |
        "זמן_עדכון_קובץ" | "חתימת_קובץ" | "טעינת_MIDI" | "טעינת_גופן" | "טעינת_דגימת_קול" |
        "טעינת_מוזיקה" | "טעינת_מילות_שיר" | "טען_מש" | "טען_רשת" | "כתוב_קובץ" | "סיים_SVG" |
        "צור_תיקייה" | "צילום_מסך" | "קרא_TAR_GZ" | "קרא_קובץ" | "רשום_TAR_GZ" | "רשום_תיקייה" |
        "آواز_نمونہ_لوڈ" | "أنشئ_مجلدا" | "أنهِ_SVG" | "ابدأ_SVG" | "اسرد_TAR_GZ" | "اسرد_المجلد" |
        "اسکرین_شاٹ" | "اقرأ_TAR_GZ" | "اقرأ_الملف" | "اكتب_الملف" | "امضای_فایل" | "اندازه_فایل" |
        "انسخ_الملف" | "بارگذاری_MIDI" | "بارگذاری_فونت" | "بارگذاری_متن_ترانه" | "بارگذاری_مش" |
        "بارگذاری_موسیقی" | "بارگذاری_نمونه_صدا" | "بصمة_الملف" | "تحميل_MIDI" | "تحميل_الخط" |
        "تحميل_الموسيقى" | "تحميل_عينة_صوتية" | "تحميل_كلمات_الأغنية" | "توقيع_الملف" |
        "حجم_الملف" | "حمّل_شبكة" | "خواندن_TAR_GZ" | "خواندن_فایل" | "درهم_فایل" |
        "زمان_ایجاد_فایل" | "زمان_تغییر_فایل" | "ساخت_پوشه" | "شروع_SVG" | "عکس‌صفحه" |
        "فائل_تخلیق_وقت" | "فائل_ترمیم_وقت" | "فائل_سائز" | "فائل_لکھو" | "فائل_میجک" |
        "فائل_پڑھو" | "فائل_کاپی" | "فائل_ہیش" | "فهرست_TAR_GZ" | "فهرست_پوشه" | "فولڈر_بناؤ" |
        "فولڈر_فہرست" | "فونٹ_لوڈ" | "لقطة_شاشة" | "موسیقی_لوڈ" | "میش_لوڈ" | "نوشتن_فایل" |
        "وقت_إنشاء_الملف" | "وقت_تعديل_الملف" | "پایان_SVG" | "کپی_فایل" | "گیت_متن_لوڈ" |
        "ขนาดไฟล์" | "คัดลอกไฟล์" | "จบSVG" | "บันทึกภาพ" | "บีบอัดโฟลเดอร์" | "มายาไฟล์" |
        "รายการทาร์" | "รายการไดเรกทอรี" | "ลบไดเรกทอรีทั้งหมด" | "ลบไฟล์" | "สร้างไดเรกทอรี" |
        "อ่านทาร์" | "อ่านไฟล์" | "เขียนไฟล์" | "เปลี่ยนชื่อไฟล์" | "เริ่มSVG" | "เวลาที่สร้าง" |
        "เวลาที่แก้ไข" | "แตกทาร์ทั้งหมด" | "แตกไฟล์ทาร์" | "แปลงไอคอนเป็นพีเอ็นจี" |
        "แพตช์เครื่องดนตรี" | "แฮชไฟล์" | "โหลดตัวอย่างเสียง" | "โหลดฟอนต์" | "โหลดมิดี" |
        "โหลดเนื้อเพลง" | "โหลดเพลง" | "โหลดเมช" | "サンプル読込" | "ファイル削除" | "ファイル改名" | "フォント読込" |
        "メッシュ読込" | "乐器音色" | "删除文件" | "加载字体" | "开始SVG" | "文件存在" | "歌詞読込" | "结束SVG" | "载入MIDI" |
        "载入歌词" | "载入网格" | "载入采样" | "载入音乐" | "重命名文件" | "音楽読込" | "音色読込" | "가사로드" | "글꼴로드" | "메시로드" |
        "미디로드" | "샘플로드" | "악기패치" | "압축파일" | "음악로드" | "파일삭제" | "파일이름변경" => "fs",
        // gui aliases (generated from the dispatch arms)
        "FFT_מיקרופון" | "FFT_الميكروفون" | "FFT_میکروفون" | "RMS_מיקרופון" | "RMS_الميكروفون" |
        "RMS_میکروفون" | "bouton_manette" | "crête_micro" | "fft_micro" | "fft_микрофона" |
        "gfx_is_open" | "gfx_present" | "gfx_window" | "interroger_manette" | "is_open" |
        "manette_appuyée" | "manette_axe_droit_x" | "manette_axe_droit_y" |
        "manette_axe_gauche_x" | "manette_axe_gauche_y" | "manette_connectée" |
        "manette_gâchette_droite" | "manette_gâchette_gauche" | "maus_dx" | "maus_dy" |
        "mikrofon_fft" | "mikrofon_rms" | "mikrofon_spitze" | "mikrofon_öffnen" |
        "nombre_manettes" | "ouvrir_micro" | "rms_micro" | "rms_микрофона" | "show" | "souris_dx" |
        "souris_dy" | "taste_gedrückt" | "taste_getippt" | "touche_appuyée" | "touche_enfoncée" |
        "vibration_manette" | "вибрация_геймпада" | "геймпад_нажат" | "геймпад_ось_лево_x" |
        "геймпад_ось_лево_y" | "геймпад_ось_право_x" | "геймпад_ось_право_y" |
        "геймпад_подключён" | "геймпад_триггер_лево" | "геймпад_триггер_право" | "клавиша_нажата" |
        "клавиша_нажатие" | "кнопка_геймпада" | "мышь_dx" | "мышь_dy" | "опросить_геймпад" |
        "открыть_микрофон" | "пик_микрофона" | "число_геймпадов" | "בדוק_בקר" | "בדוק_בקר_משחק" |
        "בקר_כלשהו" | "בקר_מחובר" | "ג'ויסטיק_ימין_X" | "ג'ויסטיק_ימין_Y" | "ג'ויסטיק_שמאל_X" |
        "ג'ויסטיק_שמאל_Y" | "גלגלת_עכבר" | "דלתא_עכבר_X" | "דלתא_עכבר_Y" | "הדק_ימין" |
        "הדק_שמאל" | "החלון_פתוח" | "הצג" | "כפתור_בקר" | "כפתור_בקר_לחוץ" | "כפתור_בקר_משחק" |
        "לחיצת_מקש" | "מספר_בקרים" | "מקש_לחוץ" | "עכבר_אמצעי_לחוץ" | "עכבר_ימני_לחוץ" |
        "פתח_חלון" | "פתח_מיקרופון" | "ציר_בקר_משחק" | "רטט_בקר" | "רטט_בקר_משחק" |
        "רשימת_בקרי_משחק" | "שיא_מיקרופון" | "آنالوگ_راست_ایکس" | "آنالوگ_راست_ایگرگ" |
        "آنالوگ_چپ_ایکس" | "آنالوگ_چپ_ایگرگ" | "أي_يد_لعب" | "استطلع_اليد" | "استطلع_يد_اللعب" |
        "اعرض" | "افتح_الميكروفون" | "افتح_نافذة" | "الفأرة_الوسطى_مضغوطة" |
        "الفأرة_اليمنى_مضغوطة" | "المفتاح_مضغوط" | "النافذة_مفتوحة" | "اهتزاز_اليد" |
        "اهتزاز_يد_اللعب" | "اوج_میکروفون" | "بائیں_اسٹک_ایکس" | "بائیں_اسٹک_وائی" |
        "بائیں_ٹریگر" | "باز_کردن_میکروفون" | "باز_کردن_پنجره" | "بررسی_دسته" | "بررسی_دسته_بازی" |
        "بٹن_دبا_ہوا" | "بٹن_دبانا" | "تعداد_دسته" | "دائیں_اسٹک_ایکس" | "دائیں_اسٹک_وائی" |
        "دائیں_ٹریگر" | "دایاں_ماؤس_دبا_ہوا" | "درمیانی_ماؤس_دبا_ہوا" | "دسته_متصل" |
        "دلتا_ماوس_ایکس" | "دلتا_ماوس_ایگرگ" | "دکمه_دسته" | "دکمه_دسته_بازی" | "دکمه_دسته_فشرده" |
        "دکھاؤ" | "ذروة_الميكروفون" | "زر_اليد" | "زر_اليد_مضغوط" | "زر_يد_اللعب" | "زناد_اليسار" |
        "زناد_اليمين" | "ضغط_المفتاح" | "عجلة_الفأرة" | "عدد_أيدي_التحكم" | "عصا_اليسرى_س" |
        "عصا_اليسرى_ص" | "عصا_اليمنى_س" | "عصا_اليمنى_ص" | "غلتک_ماوس" | "فارق_الفأرة_س" |
        "فارق_الفأرة_ص" | "فشردن_کلید" | "فهرست_دسته‌های_بازی" | "قائمة_أيدي_اللعب" | "لرزش_دسته" |
        "لرزش_دسته_بازی" | "ماؤس_اسکرول" | "ماؤس_ڈیلٹا_ایکس" | "ماؤس_ڈیلٹا_وائی" |
        "مائیکروفون_FFT" | "مائیکروفون_RMS" | "مائیکروفون_چوٹی" | "مائیکروفون_کھولو" |
        "ماشه_راست" | "ماشه_چپ" | "ماوس_راست_فشرده" | "ماوس_وسط_فشرده" | "محور_دسته_بازی" |
        "محور_يد_اللعب" | "نمایش" | "هر_دسته_بازی" | "ونڈو_کھلی_ہے" | "ونڈو_کھولو" |
        "يد_التحكم_متصلة" | "پنجره_باز_است" | "کلید_فشرده" | "کوئی_بھی_گیم_پیڈ" | "گیم_پیڈ_بٹن" |
        "گیم_پیڈ_بٹن_دبایا" | "گیم_پیڈ_تعداد" | "گیم_پیڈ_تھرتھراہٹ" | "گیم_پیڈ_دبایا" |
        "گیم_پیڈ_فہرست" | "گیم_پیڈ_لرزش" | "گیم_پیڈ_محور" | "گیم_پیڈ_منسلک" | "گیم_پیڈ_پول" |
        "گیم_پیڈ_پول_کرو" | "กดค้าง" | "กดปุ่ม" | "จอยปุ่ม" | "จอยรายการ" | "จอยสั่น" | "จอยแกน" |
        "จอยโพล" | "จอยใดๆ" | "จำนวนแพด" | "ปุ่มแพด" | "ล้อเมาส์" | "วิเคราะห์เสียงสด" |
        "หน้าต่างเปิดอยู่" | "อัปเดตแพด" | "เปิดหน้าต่าง" | "เปิดไมค์" | "เมาส์X" | "เมาส์Y" |
        "เมาส์กลาง" | "เมาส์ขวา" | "เสียงRMS" | "เสียงพีค" | "แพดกด" | "แพดขวาX" | "แพดขวาY" |
        "แพดซ้ายX" | "แพดซ้ายY" | "แพดสั่น" | "แพดเชื่อม" | "แสดงผล" | "ไกแพดขวา" | "ไกแพดซ้าย" |
        "ウィンドウ開く" | "キー押した" | "キー押す" | "パッドボタン" | "パッド右X" | "パッド右Y" | "パッド右トリガー" | "パッド左X" |
        "パッド左Y" | "パッド左トリガー" | "パッド押下" | "パッド振動" | "パッド接続" | "パッド数" | "パッド更新" | "ホイール" | "マイクRMS" |
        "マイクピーク" | "マイク開く" | "マウスΔX" | "マウスΔY" | "リアルタイムFFT" | "呈现" | "实时频谱" | "开窗" | "开麦克风" |
        "手柄右X" | "手柄右Y" | "手柄右扳机" | "手柄左X" | "手柄左Y" | "手柄左扳机" | "手柄按下" | "手柄按键" | "手柄数" | "手柄轮询" |
        "手柄连接" | "手柄震动" | "按键" | "显" | "滚轮" | "窗开" | "表示" | "键按" | "開いている" | "麦克风峰值" | "麦克风音量" |
        "鼠ΔX" | "鼠ΔY" | "마우스ΔX" | "마우스ΔY" | "마이크RMS" | "마이크열기" | "마이크피크" | "스크롤" | "실시간FFT" |
        "창열기" | "창열림" | "키누름" | "키눌림" | "패드눌림" | "패드버튼" | "패드수" | "패드연결" | "패드오X" | "패드오Y" |
        "패드오트리거" | "패드왼X" | "패드왼Y" | "패드왼트리거" | "패드진동" | "패드폴링" | "표시" => "gui",
        // net aliases (generated from the dispatch arms)
        "HTTP_حاصل_کرو" | "HTTP_روٹ" | "HTTP_سرو" | "HTTP_غیر_ہمزمان_بھیجو" | "HTTP_مستقل_فائل" |
        "HTTP_کام_پول" | "ארח_רשת" | "בדוק_משימת_HTTP" | "בדוק_רשת" | "גלה_רשת" | "הגש_HTTP" |
        "הכרז_ברשת" | "הצטרף_לרשת" | "התחבר_לרשת" | "נתיב_HTTP" | "סגור_רשת" | "סטטוס_רשת" |
        "עצור_הכרזה" | "קבל_HTTP" | "קבל_מרשת" | "קבצים_סטטיים_HTTP" | "שלח_HTTP_אסינכרוני" |
        "שלח_ברשת" | "آزمون_شبکه" | "أرسل_HTTP_غير_متزامن" | "أرسل_عبر_الشبكة" | "أعلن_في_الشبكة" |
        "أغلق_الشبكة" | "أوقف_الإعلان" | "اتصال_شبکه" | "اتصل_بالشبكة" | "اجلب_HTTP" |
        "اختبر_الشبكة" | "ارسال_شبکه" | "ارسال_ناهمگام_HTTP" | "استضف_الشبكة" |
        "استطلع_مهمة_HTTP" | "استقبل_من_الشبكة" | "اعلام_شبکه" | "اعلان_روکو" | "اكتشف_الشبكة" |
        "انضم_للشبكة" | "بررسی_وظیفه_HTTP" | "بستن_شبکه" | "توقف_اعلام" | "حالة_الشبكة" |
        "دریافت_HTTP" | "دریافت_شبکه" | "سرویس_HTTP" | "فایل_ایستای_HTTP" | "قدّم_HTTP" |
        "مسار_HTTP" | "مسیر_HTTP" | "ملفات_HTTP_ثابتة" | "میزبانی_شبکه" | "نیٹ_اعلان" | "نیٹ_بند" |
        "نیٹ_بھیجو" | "نیٹ_حالت" | "نیٹ_دریافت" | "نیٹ_شمولیت" | "نیٹ_وصول" | "نیٹ_ٹیسٹ" |
        "نیٹ_کنیکٹ" | "نیٹ_ہوسٹ" | "وضعیت_شبکه" | "پیوستن_شبکه" | "کشف_شبکه" | "ค้นหาเครือข่าย" |
        "ตัดเชื่อม" | "ทดสอบเน็ต" | "รอรับ" | "รับ" | "สถานะการเชื่อม" | "ส่ง" | "เชื่อมเน็ต" |
        "เน็ตค้นหา" | "เน็ตจอย" | "เน็ตทดสอบ" | "เน็ตประกาศ" | "เน็ตรับ" | "เน็ตสถานะ" |
        "เน็ตส่ง" | "เน็ตหยุดประกาศ" | "เน็ตโฮสต์" | "เว็บงานสำรวจ" | "เว็บรับ" | "เว็บสแตติก" |
        "เว็บเสิร์ฟ" | "เว็บเส้นทาง" | "เว็บโพสต์ไม่บล็อก" | "ネット接続" | "切断" | "发现" | "发送" | "受信" |
        "待機" | "探索" | "接收" | "接続テスト" | "接続状態" | "断开" | "测连接" | "监听" | "联网" | "连接状态" | "送信" | "검색" |
        "네트연결" | "리슨" | "수신" | "연결상태" | "연결종료" | "연결테스트" | "전송" => "net",
        // proc aliases (generated from the dispatch arms)
        "รันโปรเซส" => "proc",
        _ => return None,
    })
}

// ─── Interpreter integration ─────────────────────────────────────────────────

impl Interpreter {
    /// Cooperative cancellation point (loop iterations, function calls).
    #[inline]
    pub(crate) fn check_cancel(&self) -> Result<(), EvalErr> {
        match &self.threads_state.cancel {
            Some(c) if c.load(Ordering::Relaxed) => Err(rt("cancelled")),
            _ => Ok(()),
        }
    }

    /// Capability gate for builtin calls made inside an isolate.
    pub(crate) fn isolate_guard(&self, name: &str) -> Result<(), EvalErr> {
        match builtin_cap(name) {
            None => Ok(()),
            Some("gui") => Err(rt(format!("'{name}' is main-thread only (no GUI/input in isolates)"))),
            Some(c) => self.require_cap(c),
        }
    }

    fn cancel_flag(&self) -> Option<Arc<AtomicBool>> {
        self.threads_state.cancel.clone()
    }

    // ── Value transfer ──

    /// Deep-freeze `v`. With `translate`, strings naming channel endpoints in
    /// this isolate's table carry the endpoint along.
    pub(crate) fn freeze(&self, v: &Value, translate: bool, budget: &mut usize) -> Result<SendValue, String> {
        let charge = |budget: &mut usize, n: usize| -> Result<(), String> {
            *budget = budget.saturating_add(n);
            if *budget > MAX_MSG_BYTES {
                Err(format!("message too large (limit {} MiB)", MAX_MSG_BYTES >> 20))
            } else {
                Ok(())
            }
        };
        charge(budget, 16)?;
        Ok(match v {
            Value::Str(s) => {
                charge(budget, s.len())?;
                if translate && s.len() == 32 {
                    match self.threads_state.table.get(s.as_str()) {
                        Some(Entry::Tx(e)) => return Ok(SendValue::Tx(s.clone(), e.clone())),
                        Some(Entry::Rx(e)) => return Ok(SendValue::Rx(s.clone(), e.clone())),
                        _ => {},
                    }
                }
                SendValue::Str(s.clone())
            },
            Value::Number(n) => SendValue::Number(*n),
            Value::Bool(b) => SendValue::Bool(*b),
            Value::Unit => SendValue::Unit,
            Value::List(l) => SendValue::List(l.iter().map(|x| self.freeze(x, translate, budget)).collect::<Result<_, _>>()?),
            Value::Ok(b) => SendValue::Ok(Box::new(self.freeze(b, translate, budget)?)),
            Value::Err(b) => SendValue::Err(Box::new(self.freeze(b, translate, budget)?)),
            Value::Fn(..) => return Err("functions cannot be sent between threads".into()),
            Value::Struct { name, fields } => {
                charge(budget, name.len())?;
                SendValue::Struct {
                    name: name.clone(),
                    fields: fields
                        .iter()
                        .map(|(k, x)| Ok((k.clone(), self.freeze(x, translate, budget)?)))
                        .collect::<Result<_, String>>()?,
                }
            },
            Value::Variant { enum_name, variant, payload } => SendValue::Variant {
                enum_name: enum_name.clone(),
                variant: variant.clone(),
                payload: payload.iter().map(|x| self.freeze(x, translate, budget)).collect::<Result<_, _>>()?,
            },
        })
    }

    fn freeze_msg(&self, v: &Value) -> Result<SendValue, String> {
        let mut budget = 0;
        self.freeze(v, true, &mut budget)
    }

    fn adopt(&mut self, handle: String, e: Entry) {
        // Already holding it: the extra endpoint is simply dropped.
        self.threads_state.table.entry(handle).or_insert(e);
    }

    /// Thaw a delivered value (consumes it: strings are moved, not copied);
    /// endpoints are installed in this isolate's handle table.
    pub(crate) fn thaw(&mut self, v: SendValue) -> Value {
        match v {
            SendValue::Str(s) => Value::Str(s),
            SendValue::Number(n) => Value::Number(n),
            SendValue::Bool(b) => Value::Bool(b),
            SendValue::Unit => Value::Unit,
            SendValue::List(l) => Value::List(Rc::new(l.into_iter().map(|x| self.thaw(x)).collect())),
            SendValue::Ok(b) => Value::Ok(Box::new(self.thaw(*b))),
            SendValue::Err(b) => Value::Err(Box::new(self.thaw(*b))),
            SendValue::Struct { name, fields } => Value::Struct {
                name,
                fields: fields.into_iter().map(|(k, x)| (k, self.thaw(x))).collect(),
            },
            SendValue::Variant { enum_name, variant, payload } => Value::Variant {
                enum_name,
                variant,
                payload: payload.into_iter().map(|x| self.thaw(x)).collect(),
            },
            SendValue::Tx(h, e) => {
                self.adopt(h.clone(), Entry::Tx(e));
                Value::Str(h)
            },
            SendValue::Rx(h, e) => {
                self.adopt(h.clone(), Entry::Rx(e));
                Value::Str(h)
            },
        }
    }

    /// Thaw without consuming (shared store / repeated joins).
    fn thaw_ref(&mut self, v: &SendValue) -> Value {
        self.thaw(v.clone())
    }

    // ── Handle lookup ──

    fn handle_str<'a>(&self, name: &str, args: &'a [Value], i: usize) -> Result<&'a str, EvalErr> {
        let h = arg_str(name, args, i)?;
        if self.threads_state.table.contains_key(h) {
            Ok(h)
        } else {
            Err(rt(format!("{name}: unknown handle (not held by this thread)")))
        }
    }

    fn chan_tx(&self, name: &str, args: &[Value], i: usize) -> Result<Arc<Chan>, EvalErr> {
        let h = self.handle_str(name, args, i)?;
        match &self.threads_state.table[h] {
            Entry::Tx(e) => Ok(e.0.clone()),
            _ => Err(rt(format!("{name}: not a channel send handle"))),
        }
    }

    fn chan_rx(&self, name: &str, args: &[Value], i: usize) -> Result<Arc<Chan>, EvalErr> {
        let h = self.handle_str(name, args, i)?;
        match &self.threads_state.table[h] {
            Entry::Rx(e) => Ok(e.0.clone()),
            _ => Err(rt(format!("{name}: not a channel receive handle"))),
        }
    }

    fn thread_rec(&self, name: &str, args: &[Value]) -> Result<Arc<ThreadRec>, EvalErr> {
        let h = self.handle_str(name, args, 0)?;
        match &self.threads_state.table[h] {
            Entry::Thread(t) => Ok(t.clone()),
            _ => Err(rt(format!("{name}: not a thread handle"))),
        }
    }

    fn deliver(&mut self, v: SendValue) -> Value {
        self.thaw(v)
    }

    fn join_result(&mut self, r: ThreadResult) -> Value {
        match r {
            Ok(v) => Value::Ok(Box::new(self.deliver(v))),
            Err(e) => Value::Err(Box::new(Value::Str(e))),
        }
    }

    // ── Spawning ──

    fn resolve_spawn_path(&self, file: &str) -> Option<PathBuf> {
        let mut cands = Vec::new();
        let p = Path::new(file);
        if p.is_absolute() {
            cands.push(p.to_path_buf());
            cands.push(PathBuf::from(format!("{file}.ling")));
        } else {
            if let Some(d) = &self.source_dir {
                cands.push(d.join(file));
                cands.push(d.join(format!("{file}.ling")));
            }
            cands.push(PathBuf::from(file));
            cands.push(PathBuf::from(format!("{file}.ling")));
        }
        cands.into_iter().find(|c| c.is_file())
    }

    fn thread_spawn(&mut self, args: &[Value]) -> EvalResult {
        const N: &str = "thread_spawn";
        let file = arg_str(N, args, 0)?.to_string();
        let fn_name = arg_str(N, args, 1)?.to_string();
        let argv: Vec<Value> = match args.get(2) {
            None | Some(Value::Unit) => Vec::new(),
            Some(Value::List(l)) => l.as_ref().clone(),
            _ => return Err(rt("thread_spawn: args must be a list")),
        };
        let mut caps = HashSet::new();
        match args.get(3) {
            None | Some(Value::Unit) => {},
            Some(Value::List(l)) => {
                for c in l.iter() {
                    let Value::Str(c) = c else { return Err(rt("thread_spawn: caps must be a list of strings")) };
                    if !ALL_CAPS.contains(&c.as_str()) {
                        return bad(format!("unknown capability \"{c}\""));
                    }
                    if self.require_cap(c).is_err() {
                        return bad(format!("capability \"{c}\" not granted"));
                    }
                    caps.insert(c.clone());
                }
            },
            _ => return Err(rt("thread_spawn: caps must be a list")),
        }
        let Some(path) = self.resolve_spawn_path(&file) else {
            return bad(format!("cannot find '{file}'"));
        };
        let (canonical, program) = match parse_cached(&path) {
            Ok(x) => x,
            Err(e) => return bad(e),
        };
        let mut budget = 0;
        let frozen: Vec<SendValue> = match argv.iter().map(|v| self.freeze(v, true, &mut budget)).collect() {
            Ok(v) => v,
            Err(e) => return bad(e),
        };
        if LIVE_THREADS.fetch_add(1, Ordering::SeqCst) >= MAX_THREADS {
            LIVE_THREADS.fetch_sub(1, Ordering::SeqCst);
            return bad(format!("too many threads (limit {MAX_THREADS})"));
        }
        let handle = new_handle();
        let cancel = Arc::new(AtomicBool::new(false));
        let rec = Arc::new(ThreadRec {
            cancel: cancel.clone(),
            done: Mutex::new(None),
            cv: Condvar::new(),
            join: Mutex::new(None),
        });
        let rec2 = rec.clone();
        let self_h = handle.clone();
        let spawned = std::thread::Builder::new()
            .name(format!("ling-isolate-{}", &handle[..8]))
            .stack_size(ISOLATE_STACK)
            .spawn(move || {
                let flag = cancel.clone();
                let r = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                    run_isolate(program, path, canonical, fn_name, frozen, caps, cancel, self_h)
                }));
                let r = match r {
                    Ok(r) => r,
                    Err(p) => {
                        let msg = p
                            .downcast_ref::<&str>()
                            .map(|s| s.to_string())
                            .or_else(|| p.downcast_ref::<String>().cloned())
                            .unwrap_or_else(|| "unknown".into());
                        Err(format!("panic: {msg}"))
                    },
                };
                let r = if r.is_err() && flag.load(Ordering::Relaxed) { Err("cancelled".to_string()) } else { r };
                rec2.finish(r);
                LIVE_THREADS.fetch_sub(1, Ordering::SeqCst);
            });
        match spawned {
            Ok(j) => {
                *lock(&rec.join) = Some(j);
                self.threads_state.table.insert(handle.clone(), Entry::Thread(rec));
                ok(Value::Str(handle))
            },
            Err(e) => {
                LIVE_THREADS.fetch_sub(1, Ordering::SeqCst);
                bad(format!("spawn failed: {e}"))
            },
        }
    }

    // ── Dispatch ──

    pub(super) fn threads_builtin(&mut self, name: &str, args: &[Value]) -> Option<EvalResult> {
        if !(name.starts_with("thread_") || name.starts_with("chan_") || name.starts_with("shared_"))
            && !(self.threads_state.cancel.is_some() && matches!(name, "sleep" | "sleep_ms" | "__thread_panic"))
        {
            return None;
        }
        Some(self.threads_dispatch(name, args))
    }

    fn threads_dispatch(&mut self, name: &str, args: &[Value]) -> EvalResult {
        let cancel = self.cancel_flag();
        let cancel = cancel.as_deref();
        match name {
            "thread_spawn" => self.thread_spawn(args),
            "thread_join" | "thread_join_timeout" => {
                let rec = self.thread_rec(name, args)?;
                let dl = if name == "thread_join" { None } else { deadline_ms(arg_num(name, args, 1)?) };
                match rec.wait(dl, cancel) {
                    None => bad("timeout"),
                    Some(Err(())) => Err(rt("cancelled")),
                    Some(Ok(r)) => Ok(self.join_result(r)),
                }
            },
            "thread_alive" => Ok(Value::Bool(lock(&self.thread_rec(name, args)?.done).is_none())),
            "thread_cancel" => {
                let rec = self.thread_rec(name, args)?;
                let alive = lock(&rec.done).is_none();
                let was = rec.cancel.swap(true, Ordering::SeqCst);
                Ok(Value::Bool(alive && !was))
            },
            "thread_self" => {
                let h = self.threads_state.self_handle.get_or_insert_with(new_handle).clone();
                Ok(Value::Str(h))
            },
            "thread_cpus" => Ok(Value::Number(
                std::thread::available_parallelism().map(|n| n.get()).unwrap_or(1) as f64,
            )),
            "thread_caps" => {
                let mut v: Vec<String> = match &self.caps {
                    None => ALL_CAPS.iter().map(|s| s.to_string()).collect(),
                    Some(s) => s.iter().cloned().collect(),
                };
                v.sort();
                Ok(Value::List(Rc::new(v.into_iter().map(Value::Str).collect())))
            },

            "chan_new" | "chan_new_secure" => {
                let n = arg_num(name, args, 0)?;
                if !(n >= 1.0 && n <= MAX_CAPACITY as f64) || n.fract() != 0.0 {
                    return Err(rt(format!("{name}: capacity must be an integer in 1..={MAX_CAPACITY}")));
                }
                let c = Chan::new(n as usize, name == "chan_new_secure");
                let (th, rh) = (new_handle(), new_handle());
                self.threads_state.table.insert(th.clone(), Entry::Tx(SendEnd::new(c.clone())));
                self.threads_state.table.insert(rh.clone(), Entry::Rx(RecvEnd::new(c)));
                Ok(Value::List(Rc::new(vec![Value::Str(th), Value::Str(rh)])))
            },
            "chan_send" | "chan_try_send" => {
                let c = self.chan_tx(name, args, 0)?;
                let v = args.get(1).cloned().unwrap_or(Value::Unit);
                let sv = self.freeze_msg(&v).map_err(|e| rt(format!("{name}: {e}")))?;
                match c.send(sv, name == "chan_send", cancel) {
                    Ok(()) => ok(Value::Unit),
                    Err(SendErr::Full) => bad("full"),
                    Err(SendErr::Closed) => bad("closed"),
                    Err(SendErr::Cancelled) => Err(rt("cancelled")),
                }
            },
            "chan_recv" | "chan_try_recv" | "chan_recv_timeout" => {
                let c = self.chan_rx(name, args, 0)?;
                let r = match name {
                    "chan_recv" => c.recv(true, None, cancel),
                    "chan_try_recv" => c.recv(false, None, cancel),
                    _ => c.recv(true, Some(deadline_ms(arg_num(name, args, 1)?.max(0.0)).unwrap()), cancel),
                };
                match r {
                    Ok(v) => ok(self.deliver(v)),
                    Err(RecvErr::Empty) => bad("empty"),
                    Err(RecvErr::Timeout) => bad("timeout"),
                    Err(RecvErr::Closed) => bad("closed"),
                    Err(RecvErr::Cancelled) => Err(rt("cancelled")),
                }
            },
            "chan_select" => self.chan_select(args, cancel),
            "chan_close" => {
                let h = self.handle_str(name, args, 0)?;
                Ok(Value::Bool(match &self.threads_state.table[h] {
                    Entry::Tx(e) => e.0.close(true),
                    Entry::Rx(e) => e.0.close(false),
                    Entry::Thread(_) => return Err(rt("chan_close: not a channel handle")),
                }))
            },
            "chan_len" => {
                let h = self.handle_str(name, args, 0)?;
                Ok(Value::Number(match &self.threads_state.table[h] {
                    Entry::Tx(e) => e.0.len(),
                    Entry::Rx(e) => e.0.len(),
                    Entry::Thread(_) => return Err(rt("chan_len: not a channel handle")),
                } as f64))
            },

            "shared_put" => {
                let key = arg_str(name, args, 0)?.to_string();
                let v = args.get(1).cloned().unwrap_or(Value::Unit);
                let mut budget = 0;
                // Handles are NOT translated: the store is readable by every
                // isolate, so it must never leak a capability.
                let sv = self.freeze(&v, false, &mut budget).map_err(|e| rt(format!("shared_put: {e}")))?;
                let mut s = shared_store().write().unwrap_or_else(|e| e.into_inner());
                if s.contains_key(&key) {
                    return bad("exists");
                }
                s.insert(key, Arc::new(sv));
                ok(Value::Unit)
            },
            "shared_get" => {
                let key = arg_str(name, args, 0)?;
                let got = shared_store().read().unwrap_or_else(|e| e.into_inner()).get(key).cloned();
                Ok(match got {
                    Some(v) => self.thaw_ref(&v),
                    None => args.get(1).cloned().unwrap_or(Value::Unit),
                })
            },

            // Inside an isolate only: cancellable sleep (sliced).
            "sleep" | "sleep_ms" => {
                let ms = match args.first() {
                    Some(Value::Number(n)) => n.max(0.0),
                    _ => 0.0,
                };
                let end = Instant::now() + Duration::from_micros((ms * 1000.0) as u64);
                loop {
                    self.check_cancel()?;
                    let now = Instant::now();
                    if now >= end {
                        return Ok(Value::Unit);
                    }
                    std::thread::sleep((end - now).min(SLICE));
                }
            },
            // Inside an isolate only: deliberately panic (tests the
            // catch_unwind supervision path).
            "__thread_panic" => panic!("{}", args.first().map(|v| v.to_string()).unwrap_or_default()),

            _ => Err(rt(format!("unknown builtin '{name}'"))),
        }
    }

    fn chan_select(&mut self, args: &[Value], cancel: Option<&AtomicBool>) -> EvalResult {
        const N: &str = "chan_select";
        let list = match args.first() {
            Some(Value::List(l)) => l.clone(),
            _ => return Err(rt("chan_select: first argument must be a list of receive handles")),
        };
        if list.is_empty() {
            return Err(rt("chan_select: empty handle list"));
        }
        let chans: Vec<Arc<Chan>> = (0..list.len())
            .map(|i| self.chan_rx(N, list.as_slice(), i))
            .collect::<Result<_, _>>()?;
        let deadline = deadline_ms(match args.get(1) {
            Some(Value::Number(n)) => *n,
            _ => -1.0,
        });
        let sig = Arc::new(Signal::default());
        for c in &chans {
            lock(&c.st).watchers.push(sig.clone());
        }
        let unregister = |chans: &[Arc<Chan>]| {
            for c in chans {
                lock(&c.st).watchers.retain(|w| !Arc::ptr_eq(w, &sig));
            }
        };
        let n = chans.len();
        let start = self.threads_state.select_rr % n;
        self.threads_state.select_rr = self.threads_state.select_rr.wrapping_add(1);
        loop {
            let mut closed = 0;
            for k in 0..n {
                let i = (start + k) % n;
                match chans[i].recv(false, None, None) {
                    Ok(v) => {
                        unregister(&chans);
                        let v = self.deliver(v);
                        return ok(Value::List(Rc::new(vec![Value::Number(i as f64), v])));
                    },
                    Err(RecvErr::Closed) => closed += 1,
                    Err(_) => {},
                }
            }
            if closed == n {
                unregister(&chans);
                return bad("closed");
            }
            if is_cancelled(cancel) {
                unregister(&chans);
                return Err(rt("cancelled"));
            }
            let wait = match deadline {
                Some(d) => {
                    let now = Instant::now();
                    if now >= d {
                        unregister(&chans);
                        return bad("timeout");
                    }
                    (d - now).min(SLICE)
                },
                None => SLICE,
            };
            sig.wait(wait);
        }
    }
}

/// Body of an isolate's OS thread.
#[allow(clippy::too_many_arguments)]
fn run_isolate(
    program: Arc<Program>,
    path: PathBuf,
    canonical: String,
    fn_name: String,
    args: Vec<SendValue>,
    caps: HashSet<String>,
    cancel: Arc<AtomicBool>,
    self_handle: String,
) -> ThreadResult {
    let mut it = Interpreter::new_isolate(caps);
    it.threads_state.cancel = Some(cancel);
    it.threads_state.self_handle = Some(self_handle);
    it.source_dir = path.parent().map(|p| p.to_path_buf());
    it.loaded_files.insert(canonical);
    it.register_program(&program)?;
    if !it.functions.contains_key(fn_name.as_str()) {
        return Err(format!("thread_spawn: no function '{fn_name}' in '{}'", path.display()));
    }
    let argv: Vec<Value> = args.into_iter().map(|a| it.thaw(a)).collect();
    let r = match it.call_named(&fn_name, argv, &new_env()) {
        Ok(v) | Err(EvalErr::Return(v)) => v,
        Err(EvalErr::Runtime(s)) => return Err(s),
        Err(EvalErr::Break) => return Err("unexpected break".into()),
    };
    let mut budget = 0;
    it.freeze(&r, true, &mut budget)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn iso() -> Interpreter {
        Interpreter::new_isolate(HashSet::new())
    }

    #[test]
    fn sendvalue_roundtrip() {
        let mut it = iso();
        let v = Value::List(Rc::new(vec![
            Value::Number(1.5),
            Value::Str("héllo".into()),
            Value::Bool(true),
            Value::Unit,
            Value::Ok(Box::new(Value::Err(Box::new(Value::Str("x".into()))))),
            Value::Struct { name: "P".into(), fields: vec![("a".into(), Value::Number(2.0))] },
            Value::Variant { enum_name: "E".into(), variant: "V".into(), payload: vec![Value::Number(3.0)] },
        ]));
        let mut b = 0;
        let s = it.freeze(&v, true, &mut b).unwrap();
        let back = it.thaw(s);
        assert_eq!(back.to_string(), v.to_string());
    }

    #[test]
    fn rejects_functions_and_huge() {
        let it = iso();
        let f = Value::Fn(vec![], vec![], new_env());
        assert!(it.freeze(&f, true, &mut 0).is_err());
        let big = Value::Str("x".repeat(MAX_MSG_BYTES + 1));
        assert!(it.freeze(&big, true, &mut 0).is_err());
    }

    #[test]
    fn handles_travel_and_unknown_rejected() {
        let mut a = iso();
        let l = a.threads_dispatch("chan_new", &[Value::Number(2.0)]).unwrap();
        let Value::List(l) = l else { panic!() };
        let (tx, rx) = (l[0].clone(), l[1].clone());
        // Send the receive handle through its own channel to b.
        let sv = a.freeze_msg(&Value::List(Rc::new(vec![rx.clone()]))).unwrap();
        let mut b = iso();
        assert!(b.threads_dispatch("chan_try_recv", &[rx.clone()]).is_err());
        b.thaw(sv);
        assert!(a.threads_dispatch("chan_send", &[tx, Value::Number(7.0)]).is_ok());
        let got = b.threads_dispatch("chan_try_recv", &[rx]).unwrap();
        assert_eq!(got.to_string(), "Ok(7)");
        assert!(b.threads_dispatch("chan_recv", &[Value::Str("0".repeat(32))]).is_err());
    }
}
