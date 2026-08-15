// src/runtime/mod.rs — tree-walking interpreter with graphics support
#[cfg(not(target_arch = "wasm32"))]
mod ai;
#[cfg(not(target_arch = "wasm32"))]
mod gamepad;
#[cfg(target_arch = "wasm32")]
mod input_web;
#[cfg(not(target_arch = "wasm32"))]
pub(crate) mod jit_abi;

/// Initialize the AOT/JIT runtime. Must be called before any AOT-compiled code.
/// Creates a new interpreter instance for runtime function dispatch.
#[cfg(not(target_arch = "wasm32"))]
pub fn init_aot_runtime() {
    let interp = Interpreter::new();
    jit_abi::init(interp);
}

/// Returns seconds since Unix epoch. On wasm32 uses `js_sys::Date::now()`
/// (milliseconds / 1000); on native uses `SystemTime`.
pub fn now_secs() -> f64 {
    #[cfg(target_arch = "wasm32")]
    {
        js_sys::Date::now() / 1000.0
    }
    #[cfg(not(target_arch = "wasm32"))]
    {
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs_f64())
            .unwrap_or(0.0)
    }
}

/// Global hue-cycle for wireframe line strokes (enabled via `set_line_hue_cycle`).
/// Stored as f64 bits: the cycle rate (radians/sec) and the epoch-seconds baseline
/// captured when it was last (re)enabled. A rate of 0 disables the effect.
static LINE_HUE_RATE: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
static LINE_HUE_START: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

/// Enable/disable the wireframe line hue-cycle. `rate` is in radians/sec (0 = off).
pub fn set_line_hue_rate(rate: f64) {
    LINE_HUE_RATE.store(rate.to_bits(), std::sync::atomic::Ordering::Relaxed);
    LINE_HUE_START.store(now_secs().to_bits(), std::sync::atomic::Ordering::Relaxed);
}

/// Current hue phase (radians) for line strokes, or `None` when the cycle is off.
/// Elapsed is computed in f64 (epoch seconds are huge) before the caller casts to
/// f32, so `sin` keeps precision across a long session.
pub fn line_hue_phase() -> Option<f64> {
    let rate = f64::from_bits(LINE_HUE_RATE.load(std::sync::atomic::Ordering::Relaxed));
    if rate <= 0.0 {
        return None;
    }
    let start = f64::from_bits(LINE_HUE_START.load(std::sync::atomic::Ordering::Relaxed));
    Some((now_secs() - start) * rate)
}

// Wasm-only module registry: seeded by JS before `run_program` is called so
// that `use "path"` statements resolve without a real filesystem.
#[cfg(target_arch = "wasm32")]
thread_local! {
    static WASM_MODULES: std::cell::RefCell<std::collections::HashMap<String, String>> =
        std::cell::RefCell::new(std::collections::HashMap::new());
}

/// Register a module source for wasm32 `use` resolution.
/// Called from JS via `wasm_bindgen` before `run_program`.
#[cfg(target_arch = "wasm32")]
pub fn register_wasm_module(path: &str, source: &str) {
    WASM_MODULES.with(|m| m.borrow_mut().insert(path.to_string(), source.to_string()));
}

/// Look up a registered module source on wasm32.
#[cfg(target_arch = "wasm32")]
pub(crate) fn get_wasm_module(path: &str) -> Option<String> {
    WASM_MODULES.with(|m| m.borrow().get(path).cloned())
}

#[cfg(target_arch = "wasm32")]
#[inline]
fn wasm_sleep_ms(ms: i32) {
    if ms <= 0 {
        return;
    }

    // Keep this in Rust/js_sys so wasm-bindgen doesn't emit `require(...)`
    // snippets, which break worker/no-modules output.
    let global = js_sys::global();
    let has_sab = js_sys::Reflect::has(
        &global,
        &wasm_bindgen::JsValue::from_str("SharedArrayBuffer"),
    )
    .unwrap_or(false);
    let has_atomics =
        js_sys::Reflect::has(&global, &wasm_bindgen::JsValue::from_str("Atomics")).unwrap_or(false);
    if has_sab && has_atomics {
        let sab = js_sys::SharedArrayBuffer::new(4);
        let i32a = js_sys::Int32Array::new(&sab);
        if js_sys::Atomics::wait_with_timeout(&i32a, 0, 0, ms as f64).is_ok() {
            return;
        }
    }

    let end = js_sys::Date::now() + ms as f64;
    while js_sys::Date::now() < end {}
}

#[cfg(target_arch = "wasm32")]
fn wasm_fetch_sync(
    path: &str,
    response_type: &str,
    return_expr: &str,
) -> Result<wasm_bindgen::JsValue, String> {
    let quoted = js_sys::JSON::stringify(&wasm_bindgen::JsValue::from_str(path))
        .ok()
        .and_then(|s| s.as_string())
        .unwrap_or_else(|| "\"\"".to_string());

    let script = format!(
        "(function(){{\n  var xhr = new XMLHttpRequest();\n  xhr.open('GET', {quoted}, false);\n  xhr.responseType = '{response_type}';\n  xhr.send(null);\n  if ((xhr.status|0) !== 200 && (xhr.status|0) !== 0) {{ throw new Error('HTTP ' + xhr.status + ' for ' + {quoted}); }}\n  return {return_expr};\n}})()"
    );

    js_sys::eval(&script).map_err(|e| {
        e.as_string()
            .unwrap_or_else(|| format!("JS eval failed: {:?}", e))
    })
}

#[cfg(target_arch = "wasm32")]
fn wasm_fetch_bytes(path: &str) -> Result<Vec<u8>, String> {
    let value = wasm_fetch_sync(
        path,
        "arraybuffer",
        "new Uint8Array(xhr.response || new ArrayBuffer(0))",
    )?;
    let arr = js_sys::Uint8Array::new(&value);
    let mut out = vec![0u8; arr.length() as usize];
    arr.copy_to(&mut out);
    Ok(out)
}

#[cfg(target_arch = "wasm32")]
fn wasm_fetch_text(path: &str) -> Result<String, String> {
    let value = wasm_fetch_sync(path, "text", "String(xhr.responseText || '')")?;
    Ok(value.as_string().unwrap_or_default())
}

#[cfg(not(target_arch = "wasm32"))]
mod net;
#[cfg(all(not(target_arch = "wasm32"), feature = "web"))]
pub mod web;
use crate::gfx::{GfxState, Light};
use crate::parser::ast::*;
#[cfg(target_arch = "wasm32")]
use js_sys;
use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;
// `raster` is wasm-safe (pure CPU framebuffer), so `draw_line` is available on
// web too; `fill_triangle` is only reached from native-gated 3-D fill paths.
use crate::gfx::raster::draw_line;
#[cfg(not(target_arch = "wasm32"))]
use crate::gfx::raster::fill_triangle;
#[cfg(not(target_arch = "wasm32"))]
use ling_audio::{AudioEngine, ToneParams, Wave};

#[cfg(not(target_arch = "wasm32"))]
use ling_audio::FftAnalyzer;

#[cfg(not(target_arch = "wasm32"))]
use ling_mic;

// ─── Values ──────────────────────────────────────────────────────────────────

#[derive(Debug, Clone)]
pub enum Value {
    Str(String),
    Number(f64),
    Bool(bool),
    Unit,
    List(Rc<Vec<Value>>),
    Ok(Box<Value>),
    Err(Box<Value>),
    Fn(Vec<String>, Vec<Stmt>, Env),
    /// `form` record instance — ordered named fields.
    Struct {
        name: String,
        fields: Vec<(String, Value)>,
    },
    /// `choose` enum instance — variant tag plus ordered payload.
    Variant {
        enum_name: String,
        variant: String,
        payload: Vec<Value>,
    },
}

// Interpreter-hot maps use a fast non-crypto hasher: short Thai identifier keys
// are hashed on every variable access and builtin dispatch, where SipHash dominates.
use rustc_hash::FxHashMap;
type Env = FxHashMap<String, Value>;

#[inline]
fn new_env() -> Env {
    FxHashMap::default()
}

impl std::fmt::Display for Value {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Value::Str(s) => write!(f, "{s}"),
            Value::Number(n) => {
                if n.fract() == 0.0 && n.abs() < 1e15 {
                    write!(f, "{}", *n as i64)
                } else {
                    write!(f, "{n}")
                }
            },
            Value::Bool(b) => write!(f, "{b}"),
            Value::Unit => write!(f, "()"),
            Value::List(v) => {
                write!(f, "[")?;
                for (i, x) in v.iter().enumerate() {
                    if i > 0 {
                        write!(f, ", ")?;
                    }
                    write!(f, "{x}")?;
                }
                write!(f, "]")
            },
            Value::Ok(v) => write!(f, "Ok({v})"),
            Value::Err(v) => write!(f, "Err({v})"),
            Value::Fn(_, _, _) => write!(f, "<fn>"),
            Value::Struct { name, fields } => {
                write!(f, "{name} {{ ")?;
                for (i, (k, v)) in fields.iter().enumerate() {
                    if i > 0 {
                        write!(f, ", ")?;
                    }
                    write!(f, "{k}: {v}")?;
                }
                write!(f, " }}")
            },
            Value::Variant { variant, payload, .. } => {
                write!(f, "{variant}")?;
                if !payload.is_empty() {
                    write!(f, "(")?;
                    for (i, v) in payload.iter().enumerate() {
                        if i > 0 {
                            write!(f, ", ")?;
                        }
                        write!(f, "{v}")?;
                    }
                    write!(f, ")")?;
                }
                Ok(())
            },
        }
    }
}

#[cfg(target_arch = "wasm32")]
fn wasm_unsupported_builtin(name: &str) -> Option<Value> {
    Some(match name {
        // interface blips (native-only today)
        "audio_blip"
        | "提示音"
        | "ビープ音"
        | "효과음"
        | "เสียงบี๊บ"
        | "ui_sound"
        | "界面音"
        | "UI音"
        | "인터페이스음"
        | "เสียงปุ่ม"
        | "audio_stop_sfx"
        | "停止音效"
        | "効果音停止"
        | "효과음정지"
        | "หยุดเอฟเฟกต์ทั้งหมด" | "بوق_کوتاه" | "نغمة_قصيرة" | "ביפ" | "بلپ_آواز" => Value::Unit,

        // music loading / analysis / playback / midi (native-only today)
        "music_load"
        | "载入音乐"
        | "音楽読込"
        | "음악로드"
        | "โหลดเพลง"
        | "music_patch"
        | "乐器音色"
        | "音色読込"
        | "악기패치"
        | "แพตช์เครื่องดนตรี"
        | "music_lrc"
        | "载入歌词"
        | "歌詞読込"
        | "가사로드"
        | "โหลดเนื้อเพลง"
        | "music_midi_load"
        | "载入MIDI"
        | "MIDI読込"
        | "미디로드"
        | "โหลดมิดี" | "بارگذاری_موسیقی" | "تحميل_الموسيقى" | "טעינת_מוזיקה" | "موسیقی_لوڈ" => Value::Number(-1.0),

        "music_duration"
        | "音乐时长"
        | "音楽長さ"
        | "음악길이"
        | "ความยาวเพลง"
        | "music_bpm"
        | "节拍速度"
        | "テンポ"
        | "템포"
        | "จังหวะต่อนาที"
        | "music_pos"
        | "音乐位置"
        | "音楽位置"
        | "음악위치"
        | "ตำแหน่งเพลง"
        | "music_mic_pitch"
        | "麦克风音高"
        | "マイク音程"
        | "마이크음정"
        | "ระดับเสียงไมค์"
        | "music_hz"
        | "音符频率"
        | "音符周波数"
        | "음표주파수"
        | "ความถี่โน้ต"
        | "music_pitch_score"
        | "音准评分"
        | "音程スコア"
        | "음정점수"
        | "คะแนนเสียง"
        | "music_judge"
        | "判定"
        | "判定する"
        | "판정"
        | "ตัดสินจังหวะ"
        | "music_midi_count"
        | "MIDI数量"
        | "MIDI数"
        | "미디수"
        | "จำนวนมิดี" | "مدت_موسیقی" | "مدة_الموسيقى" | "משך_מוזיקה" | "موسیقی_دورانیہ" => Value::Number(0.0),

        "music_key"
        | "调性"
        | "調性"
        | "조성"
        | "คีย์เพลง"
        | "music_lyric"
        | "当前歌词"
        | "現在歌詞"
        | "현재가사"
        | "เนื้อเพลงปัจจุบัน"
        | "music_note_name"
        | "音名"
        | "音名称"
        | "음이름"
        | "ชื่อโน้ต"
        | "music_grade_name"
        | "判定名"
        | "判定名称"
        | "판정이름"
        | "ชื่อการตัดสิน" | "گام_موسیقی" | "مقام_الموسيقى" | "סולם_מוזיקלי" | "موسیقی_کلید" => Value::Str(String::new()),

        "music_onsets"
        | "音符起点"
        | "オンセット"
        | "온셋"
        | "จุดเริ่มเสียง"
        | "music_beat_grid"
        | "节拍网格"
        | "ビートグリッド"
        | "비트그리드"
        | "กริดจังหวะ"
        | "music_midi_notes"
        | "MIDI音符"
        | "MIDIノート"
        | "미디음표"
        | "โน้ตมิดี"
        | "music_midi_bars"
        | "MIDI音条"
        | "MIDIバー"
        | "미디바"
        | "แท่งมิดี"
        | "music_fft"
        | "音乐频谱"
        | "音楽スペクトル"
        | "음악스펙트럼"
        | "สเปกตรัมเพลง" | "آغازهای_نت" | "بدايات_النغمات" | "התחלות_תווים" | "نوٹ_شروعات" => Value::List(Vec::new().into()),

        "music_play"
        | "播放音乐"
        | "音楽再生"
        | "음악재생"
        | "เล่นเพลง"
        | "music_pause"
        | "暂停音乐"
        | "音楽一時停止"
        | "음악일시정지"
        | "หยุดเพลงชั่วคราว"
        | "music_stop"
        | "停止音乐"
        | "音楽停止"
        | "음악정지"
        | "หยุดเพลง"
        | "music_seek"
        | "定位音乐"
        | "音楽シーク"
        | "음악탐색"
        | "ค้นหาเพลง"
        | "music_volume"
        | "音乐音量"
        | "音楽音量"
        | "음악음량"
        | "ระดับเพลง"
        | "music_note"
        | "弹音符"
        | "音符演奏"
        | "음표연주"
        | "เล่นโน้ต"
        | "music_note_on"
        | "音符开始"
        | "音符オン"
        | "음표켜기"
        | "โน้ตเริ่ม"
        | "music_note_off"
        | "音符结束"
        | "音符オフ"
        | "음표끄기"
        | "โน้ตจบ" | "پخش_موسیقی" | "شغّل_الموسيقى" | "נגן_מוזיקה" | "موسیقی_چلاؤ" => Value::Unit,

        // liquid sim — return handle 0 for new, Unit for everything else
        "liquid_new" | "新建液体" | "液体新規" | "액체생성" | "สร้างของเหลว" | "مایع_جدید" | "سائل_جديد" | "נוזל_חדש" | "نیا_مائع" => {
            Value::Number(0.0)
        },
        "liquid_mix" | "液体混合" | "液体混合度" | "액체혼합" | "การผสมของเหลว" | "ترکیب_مایع" | "مزج_سائل" | "ערבוב_נוזל" | "مائع_ملاؤ" => {
            Value::Number(0.0)
        },
        "liquid_set_colors"
        | "液体颜色"
        | "液体配色"
        | "액체색상"
        | "สีของเหลว"
        | "liquid_splat"
        | "液体注入"
        | "液体追加"
        | "액체분사"
        | "หยดของเหลว"
        | "liquid_gravity"
        | "液体重力"
        | "液体重力ベクトル"
        | "액체중력"
        | "แรงโน้มถ่วงเหลว"
        | "liquid_step"
        | "液体步进"
        | "液体更新"
        | "액체스텝"
        | "ก้าวของเหลว"
        | "liquid_step_all"
        | "液体全步进"
        | "液体全更新"
        | "전체액체스텝"
        | "ก้าวของเหลวทั้งหมด"
        | "liquid_rainbow"
        | "液体彩虹"
        | "液体虹"
        | "액체무지개"
        | "ของเหลวสายรุ้ง"
        | "liquid_draw"
        | "绘制液体"
        | "液体描画"
        | "액체그리기"
        | "วาดของเหลว"
        | "liquid_draw_surface"
        | "液体贴面"
        | "液体曲面"
        | "액체곡면"
        | "ของเหลวบนพื้นผิว" | "تنظیم_رنگ_مایع" | "عيّن_ألوان_السائل" | "קבע_צבעי_נוזל" | "مائع_رنگ_مقرر_کرو" => Value::Unit,

        // ── game AI: neural networks ─────────────────────────────────────────
        "nn_new"
        | "建神经网"
        | "ニューラル作成"
        | "신경망생성"
        | "สร้างโครงข่าย"
        | "nn_load"
        | "载入网"
        | "網読込"
        | "신경망불러오기"
        | "โหลดโครงข่าย" | "شبکه_جدید" | "شبكة_جديدة" | "רשת_חדשה" | "نئی_نیورل_نیٹ" => Value::Number(-1.0),
        "nn_forward" | "神经前向" | "順伝播" | "순전파" | "ส่งต่อโครงข่าย" | "پیش‌روی_شبکه" | "تمرير_أمامي" | "העברה_קדימה" | "فارورڈ_پاس" => {
            Value::List(Vec::new().into())
        },
        "nn_train"
        | "训练网"
        | "ニューラル学習"
        | "신경망학습"
        | "ฝึกโครงข่าย"
        | "nn_dense"
        | "密集层"
        | "密層追加"
        | "밀집층"
        | "ชั้นหนาแน่น" | "آموزش_شبکه" | "درّب_الشبكة" | "אמן_רשת" | "نیٹ_ٹریننگ" => Value::Number(0.0),
        "nn_save" | "保存网" | "網保存" | "신경망저장" | "บันทึกโครงข่าย" | "ذخیره_شبکه" | "احفظ_الشبكة" | "שמור_רשת" | "نیٹ_محفوظ_کرو" => {
            Value::Bool(false)
        },

        // ── game AI: behavior trees ─────────────────────────────────────────
        "bt_build" | "建行为树" | "行動木構築" | "행동트리구성" | "สร้างต้นไม้พฤติกรรม" | "ساخت_درخت_رفتار" | "ابنِ_شجرة_السلوك" | "בנה_עץ_התנהגות" | "بی_ٹی_تعمیر" => {
            Value::Number(-1.0)
        },
        "bt_tick" | "行为树滴答" | "行動木更新" | "행동트리틱" | "เดินต้นไม้พฤติกรรม" | "تیک_درخت_رفتار" | "نبضة_شجرة_السلوك" | "טיק_עץ_התנהגות" | "بی_ٹی_ٹک" => {
            Value::Str(String::new())
        },
        "bt_status" | "行为树状态" | "行動木状態" | "행동트리상태" | "สถานะต้นไม้พฤติกรรม" | "وضعیت_درخت_رفتار" | "حالة_شجرة_السلوك" | "סטטוס_עץ_התנהגות" | "بی_ٹی_حالت" => {
            Value::Number(0.0)
        },
        "bt_set" | "设事实" | "事実設定" | "사실설정" | "ตั้งข้อเท็จจริง" | "تنظیم_واقعیت" | "عيّن_حقيقة" | "קבע_עובדה" | "بی_ٹی_سیٹ" => {
            Value::Unit
        },

        // ── game AI: dialog LLM ─────────────────────────────────────────────
        "dialog_new"
        | "建对话模型"
        | "対話モデル作成"
        | "대화모델생성"
        | "สร้างโมเดลสนทนา"
        | "dialog_load_model"
        | "对话载模"
        | "対話モデル読込"
        | "대화모델불러오기"
        | "โหลดโมเดลสนทนา"
        | "dialog_train"
        | "对话训练"
        | "対話訓練"
        | "대화훈련"
        | "ฝึกสนทนา"
        | "dialog_load"
        | "对话载入"
        | "対話読込"
        | "대화불러오기"
        | "โหลดชุดสนทนา" | "مدل_گفتگوی_جدید" | "نموذج_حوار_جديد" | "מודל_דיאלוג_חדש" | "نیا_مکالمہ_ماڈل" => Value::Number(-1.0),
        "dialog_say" | "对话生成" | "対話生成" | "대화생성" | "พูดสนทนา" | "بگو" | "قل" | "אמור" | "کہو" => {
            Value::Str(String::new())
        },
        "dialog_save" | "对话存模" | "対話モデル保存" | "대화모델저장" | "บันทึกโมเดลสนทนา" | "ذخیره_مدل_گفتگو" | "احفظ_نموذج_الحوار" | "שמור_מודל_דיאלוג" | "مکالمہ_ماڈل_محفوظ" => {
            Value::Bool(false)
        },
        "dialog_learn" | "对话学习" | "対話学習" | "대화학습" | "เรียนรู้สนทนา" | "یادگیری_گفتگو" | "تعلّم_الحوار" | "למד_דיאלוג" | "مکالمہ_سیکھو" => {
            Value::Unit
        },

        // ── networking ──────────────────────────────────────────────────────
        "net_connect"
        | "联网"
        | "ネット接続"
        | "네트연결"
        | "เชื่อมเน็ต"
        | "net_listen"
        | "监听"
        | "待機"
        | "리슨"
        | "รอรับ"
        | "net_send"
        | "发送"
        | "送信"
        | "전송"
        | "ส่ง" | "اتصال_شبکه" | "اتصل_بالشبكة" | "התחבר_לרשת" | "نیٹ_کنیکٹ" => Value::Number(-1.0),
        "net_recv"
        | "接收"
        | "受信"
        | "수신"
        | "รับ"
        | "net_status"
        | "连接状态"
        | "接続状態"
        | "연결상태"
        | "สถานะการเชื่อม" | "دریافت_شبکه" | "استقبل_من_الشبكة" | "קבל_מרשת" | "نیٹ_وصول" => Value::Str(String::new()),
        "net_discover" | "发现" | "探索" | "검색" | "ค้นหาเครือข่าย" | "کشف_شبکه" | "اكتشف_الشبكة" | "גלה_רשת" | "نیٹ_دریافت" => {
            Value::List(Vec::new().into())
        },
        "net_close"
        | "断开"
        | "切断"
        | "연결종료"
        | "ตัดเชื่อม"
        | "net_test"
        | "测连接"
        | "接続テスト"
        | "연결테스트"
        | "ทดสอบเน็ต" | "بستن_شبکه" | "أغلق_الشبكة" | "סגור_רשת" | "نیٹ_بند" => Value::Number(0.0),

        // catch-all: any other native-only builtin silently no-ops on wasm32
        _ => Value::Unit,
    })
}

// ─── Control flow ────────────────────────────────────────────────────────────

#[derive(Debug)]
pub(crate) enum EvalErr {
    Runtime(String),
    Return(Value),
    #[allow(dead_code)] // reserved for future `break` statement support
    Break,
}

impl From<String> for EvalErr {
    fn from(s: String) -> Self {
        EvalErr::Runtime(s)
    }
}

type EvalResult = Result<Value, EvalErr>;

/// RFC 4648 base32 encode (no padding) — used for TOTP secrets.
#[cfg(all(not(target_arch = "wasm32"), feature = "web"))]
fn base32_encode(data: &[u8]) -> String {
    const ALPHABET: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZ234567";
    let mut out = String::new();
    let mut buffer: u32 = 0;
    let mut bits = 0u32;
    for &b in data {
        buffer = (buffer << 8) | b as u32;
        bits += 8;
        while bits >= 5 {
            bits -= 5;
            out.push(ALPHABET[((buffer >> bits) & 0x1f) as usize] as char);
        }
    }
    if bits > 0 {
        out.push(ALPHABET[((buffer << (5 - bits)) & 0x1f) as usize] as char);
    }
    out
}

/// RFC 4648 base32 decode (case-insensitive, ignores padding/whitespace).
#[cfg(all(not(target_arch = "wasm32"), feature = "web"))]
fn base32_decode(s: &str) -> Option<Vec<u8>> {
    let mut buffer: u32 = 0;
    let mut bits = 0u32;
    let mut out = Vec::new();
    for c in s.chars() {
        if c == '=' || c.is_whitespace() {
            continue;
        }
        let v = match c.to_ascii_uppercase() {
            'A'..='Z' => c.to_ascii_uppercase() as u32 - 'A' as u32,
            '2'..='7' => c as u32 - '2' as u32 + 26,
            _ => return None,
        };
        buffer = (buffer << 5) | v;
        bits += 5;
        if bits >= 8 {
            bits -= 8;
            out.push((buffer >> bits) as u8);
        }
    }
    Some(out)
}

/// One RFC 6238 TOTP code (HMAC-SHA1, 6 digits) for the given 30s time step.
#[cfg(all(not(target_arch = "wasm32"), feature = "web"))]
fn totp_code(secret_b32: &str, step: u64) -> Option<String> {
    let key = base32_decode(secret_b32)?;
    let msg = step.to_be_bytes();
    let mac = hmac_sha1(&key, &msg);
    let offset = (mac[19] & 0x0f) as usize;
    let bin = ((mac[offset] as u32 & 0x7f) << 24)
        | ((mac[offset + 1] as u32) << 16)
        | ((mac[offset + 2] as u32) << 8)
        | (mac[offset + 3] as u32);
    Some(format!("{:06}", bin % 1_000_000))
}

/// TOTP verify with a ±1 step window (tolerates minor clock skew).
#[cfg(all(not(target_arch = "wasm32"), feature = "web"))]
fn totp_check(secret_b32: &str, code: &str) -> bool {
    if code.len() != 6 || !code.bytes().all(|b| b.is_ascii_digit()) {
        return false;
    }
    let now_step = (crate::runtime::now_secs() as u64) / 30;
    for delta in [-1i64, 0, 1] {
        let step = (now_step as i64 + delta) as u64;
        if let Some(expected) = totp_code(secret_b32, step) {
            // constant-time-ish compare (fixed 6-char length)
            if expected.as_bytes().ct_eq_str(code.as_bytes()) {
                return true;
            }
        }
    }
    false
}

/// HMAC-SHA1 built on the `hmac`+`sha1` crates (both via the web feature).
#[cfg(all(not(target_arch = "wasm32"), feature = "web"))]
fn hmac_sha1(key: &[u8], msg: &[u8]) -> [u8; 20] {
    use hmac::{Mac, SimpleHmac};
    let mut mac = SimpleHmac::<sha1::Sha1>::new_from_slice(key).expect("hmac key");
    mac.update(msg);
    let out = mac.finalize().into_bytes();
    let mut arr = [0u8; 20];
    arr.copy_from_slice(&out);
    arr
}

/// Tiny fixed-length constant-time byte compare helper for TOTP codes.
#[cfg(all(not(target_arch = "wasm32"), feature = "web"))]
trait CtEqStr {
    fn ct_eq_str(&self, other: &[u8]) -> bool;
}
#[cfg(all(not(target_arch = "wasm32"), feature = "web"))]
impl CtEqStr for [u8] {
    fn ct_eq_str(&self, other: &[u8]) -> bool {
        if self.len() != other.len() {
            return false;
        }
        let mut diff = 0u8;
        for (a, b) in self.iter().zip(other.iter()) {
            diff |= a ^ b;
        }
        diff == 0
    }
}

/// Percent-decodes a URL query component (`+` → space, `%41` → `A`).
fn url_decode(s: &str) -> String {
    let bytes = s.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        match bytes[i] {
            b'+' => {
                out.push(b' ');
                i += 1;
            },
            b'%' if i + 2 < bytes.len() => {
                let hi = (bytes[i + 1] as char).to_digit(16);
                let lo = (bytes[i + 2] as char).to_digit(16);
                if let (Some(h), Some(l)) = (hi, lo) {
                    out.push((h * 16 + l) as u8);
                    i += 3;
                } else {
                    out.push(bytes[i]);
                    i += 1;
                }
            },
            b => {
                out.push(b);
                i += 1;
            },
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// Maps Ling values to owned rusqlite parameter values for `db_exec`/`db_query`.
#[cfg(all(not(target_arch = "wasm32"), feature = "web"))]
fn values_to_sql_params(args: &[Value]) -> Vec<ling_http::rusqlite::types::Value> {
    use ling_http::rusqlite::types::Value as Sql;
    args.iter()
        .map(|v| match v {
            Value::Number(n) if n.fract() == 0.0 && n.abs() < 9e15 => Sql::Integer(*n as i64),
            Value::Number(n) => Sql::Real(*n),
            Value::Bool(b) => Sql::Integer(*b as i64),
            other => Sql::Text(other.to_string()),
        })
        .collect()
}

// GfxState is now defined in crate::gfx — see src/gfx/mod.rs.

// ─── SVG writer ───────────────────────────────────────────────────────────────

struct SvgWriter {
    path: String,
    width: f64,
    height: f64,
    elements: Vec<String>,
}

impl SvgWriter {
    fn new(path: String, width: f64, height: f64) -> Self {
        let bg = format!("<rect width=\"{width}\" height=\"{height}\" fill=\"#0a0a0a\"/>");
        Self { path, width, height, elements: vec![bg] }
    }

    fn save(&self) -> std::io::Result<()> {
        let w = self.width;
        let h = self.height;
        let mut out = format!(
            "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n\
             <svg xmlns=\"http://www.w3.org/2000/svg\" \
             width=\"{w}\" height=\"{h}\" viewBox=\"0 0 {w} {h}\">\n"
        );
        for elem in &self.elements {
            out.push_str("  ");
            out.push_str(elem);
            out.push('\n');
        }
        out.push_str("</svg>\n");
        // Create parent directory if it doesn't exist.
        if let Some(parent) = std::path::Path::new(&self.path).parent() {
            if !parent.as_os_str().is_empty() {
                let _ = std::fs::create_dir_all(parent);
            }
        }
        std::fs::write(&self.path, out.as_bytes())
    }
}

fn hsl_to_hex(h: f64, s: f64, l: f64) -> String {
    let s = s / 100.0;
    let l = l / 100.0;
    let c = (1.0 - (2.0 * l - 1.0).abs()) * s;
    let x = c * (1.0 - ((h / 60.0) % 2.0 - 1.0).abs());
    let m = l - c / 2.0;
    let (r1, g1, b1) = if h < 60.0 {
        (c, x, 0.0)
    } else if h < 120.0 {
        (x, c, 0.0)
    } else if h < 180.0 {
        (0.0, c, x)
    } else if h < 240.0 {
        (0.0, x, c)
    } else if h < 300.0 {
        (x, 0.0, c)
    } else {
        (c, 0.0, x)
    };
    let r = ((r1 + m) * 255.0).round() as u8;
    let g = ((g1 + m) * 255.0).round() as u8;
    let b = ((b1 + m) * 255.0).round() as u8;
    format!("#{r:02x}{g:02x}{b:02x}")
}

// ─── Procedural texture helpers ───────────────────────────────────────────────

fn tex_hash(x: i32, y: i32, seed: u32) -> f32 {
    let mut h = (x as u32)
        .wrapping_add((y as u32).wrapping_mul(2654435769))
        .wrapping_add(seed.wrapping_mul(1234567891));
    h ^= h >> 16;
    h = h.wrapping_mul(0x45d9f3b);
    h ^= h >> 16;
    h as f32 / u32::MAX as f32
}

fn tex_vnoise(x: f32, y: f32, seed: u32) -> f32 {
    let xi = x.floor() as i32;
    let yi = y.floor() as i32;
    let sm = |t: f32| t * t * (3.0 - 2.0 * t);
    let xf = sm(x - xi as f32);
    let yf = sm(y - yi as f32);
    let a = tex_hash(xi, yi, seed);
    let b = tex_hash(xi + 1, yi, seed);
    let c = tex_hash(xi, yi + 1, seed);
    let d = tex_hash(xi + 1, yi + 1, seed);
    a + (b - a) * xf + (c - a) * yf + (a - b - c + d) * xf * yf
}

fn tex_fbm(x: f32, y: f32, octaves: u32, seed: u32) -> f32 {
    let mut v = 0.0f32;
    let mut amp = 0.5f32;
    let mut f = 1.0f32;
    for i in 0..octaves {
        v += tex_vnoise(x * f, y * f, seed.wrapping_add(i * 7919)) * amp;
        amp *= 0.5;
        f *= 2.0;
    }
    v
}

fn tex_palette(name: &str, t: f32) -> [f32; 3] {
    let (a, b, c, d): ([f32; 3], [f32; 3], [f32; 3], [f32; 3]) = match name {
        "fire" => (
            [0.8, 0.4, 0.1],
            [0.7, 0.3, 0.1],
            [1.0, 0.5, 0.3],
            [0.0, 0.5, 0.8],
        ),
        "ocean" => (
            [0.1, 0.4, 0.7],
            [0.3, 0.3, 0.4],
            [0.8, 1.0, 0.5],
            [0.3, 0.0, 0.6],
        ),
        "psychedelic" => (
            [0.5, 0.5, 0.5],
            [0.8, 0.8, 0.8],
            [1.0, 1.3, 0.7],
            [0.0, 0.15, 0.3],
        ),
        "neon" => (
            [0.5, 0.5, 0.5],
            [0.5, 0.5, 0.5],
            [2.0, 1.0, 0.0],
            [0.5, 0.2, 0.25],
        ),
        "forest" => (
            [0.3, 0.5, 0.2],
            [0.2, 0.3, 0.1],
            [1.0, 0.5, 0.8],
            [0.1, 0.3, 0.6],
        ),
        _ => (
            [0.5, 0.5, 0.5],
            [0.5, 0.5, 0.5],
            [1.0, 1.0, 1.0],
            [0.0, 0.333, 0.667],
        ),
    };
    [0, 1, 2]
        .map(|i| (a[i] + b[i] * (std::f32::consts::TAU * (c[i] * t + d[i])).cos()).clamp(0.0, 1.0))
}

/// Map a physical key to a typed character for ling-ui text input (lowercase).
#[cfg(not(target_arch = "wasm32"))]
// Full US-QWERTY keyboard → char, shift-aware. `key_char` only ever emits
// printable ASCII (never Tab/Enter/control bytes — those keys have no case
// here at all) so anything read through `text_poll` is safe by construction
// to drop straight into the game's tab/comma/semicolon/pipe-framed wire
// protocols without needing per-keystroke sanitization.
fn key_char(k: minifb::Key, shift: bool) -> Option<char> {
    use minifb::Key::*;
    let base = match k {
        A => 'a',
        B => 'b',
        C => 'c',
        D => 'd',
        E => 'e',
        F => 'f',
        G => 'g',
        H => 'h',
        I => 'i',
        J => 'j',
        K => 'k',
        L => 'l',
        M => 'm',
        N => 'n',
        O => 'o',
        P => 'p',
        Q => 'q',
        R => 'r',
        S => 's',
        T => 't',
        U => 'u',
        V => 'v',
        W => 'w',
        X => 'x',
        Y => 'y',
        Z => 'z',
        Key0 => '0',
        Key1 => '1',
        Key2 => '2',
        Key3 => '3',
        Key4 => '4',
        Key5 => '5',
        Key6 => '6',
        Key7 => '7',
        Key8 => '8',
        Key9 => '9',
        Space => ' ',
        Minus => '-',
        Equal => '=',
        Period => '.',
        Comma => ',',
        Slash => '/',
        Backslash => '\\',
        Semicolon => ';',
        Apostrophe => '\'',
        LeftBracket => '[',
        RightBracket => ']',
        Backquote => '`',
        _ => return None,
    };
    if !shift {
        return Some(base);
    }
    Some(match base {
        'a'..='z' => base.to_ascii_uppercase(),
        '0' => ')',
        '1' => '!',
        '2' => '@',
        '3' => '#',
        '4' => '$',
        '5' => '%',
        '6' => '^',
        '7' => '&',
        '8' => '*',
        '9' => '(',
        '-' => '_',
        '=' => '+',
        '.' => '>',
        ',' => '<',
        '/' => '?',
        '\\' => '|',
        ';' => ':',
        '\'' => '"',
        '[' => '{',
        ']' => '}',
        '`' => '~',
        other => other,
    })
}

/// Win32 virtual-key code → char, mirroring `key_char` exactly but keyed by
/// VK code instead of `minifb::Key` — used by the `GetAsyncKeyState` input
/// path (see `os_key_down` / the `topmost_window` fallback in `text_poll`).
#[cfg(all(not(target_arch = "wasm32"), windows))]
fn vk_char(vk: i32, shift: bool) -> Option<char> {
    let base = match vk {
        0x41..=0x5A => (b'a' + (vk - 0x41) as u8) as char, // 'A'..'Z'
        0x30..=0x39 => (b'0' + (vk - 0x30) as u8) as char, // '0'..'9'
        0x20 => ' ',  // VK_SPACE
        0xBD => '-',  // VK_OEM_MINUS
        0xBB => '=',  // VK_OEM_PLUS (unshifted '=')
        0xBE => '.',  // VK_OEM_PERIOD
        0xBC => ',',  // VK_OEM_COMMA
        0xBF => '/',  // VK_OEM_2
        0xDC => '\\', // VK_OEM_5
        0xBA => ';',  // VK_OEM_1
        0xDE => '\'', // VK_OEM_7
        0xDB => '[',  // VK_OEM_4
        0xDD => ']',  // VK_OEM_6
        0xC0 => '`',  // VK_OEM_3
        _ => return None,
    };
    if !shift {
        return Some(base);
    }
    Some(match base {
        'a'..='z' => base.to_ascii_uppercase(),
        '0' => ')',
        '1' => '!',
        '2' => '@',
        '3' => '#',
        '4' => '$',
        '5' => '%',
        '6' => '^',
        '7' => '&',
        '8' => '*',
        '9' => '(',
        '-' => '_',
        '=' => '+',
        '.' => '>',
        ',' => '<',
        '/' => '?',
        '\\' => '|',
        ';' => ':',
        '\'' => '"',
        '[' => '{',
        ']' => '}',
        '`' => '~',
        other => other,
    })
}

/// The VK codes `text_poll`'s `GetAsyncKeyState` fallback scans each frame —
/// every key `vk_char` can turn into a character.
#[cfg(all(not(target_arch = "wasm32"), windows))]
const TEXT_POLL_VKS: &[i32] = &[
    0x41, 0x42, 0x43, 0x44, 0x45, 0x46, 0x47, 0x48, 0x49, 0x4A, 0x4B, 0x4C, 0x4D, 0x4E, 0x4F,
    0x50, 0x51, 0x52, 0x53, 0x54, 0x55, 0x56, 0x57, 0x58, 0x59, 0x5A, 0x30, 0x31, 0x32, 0x33,
    0x34, 0x35, 0x36, 0x37, 0x38, 0x39, 0x20, 0xBD, 0xBB, 0xBE, 0xBC, 0xBF, 0xDC, 0xBA, 0xDE,
    0xDB, 0xDD, 0xC0,
];
/// VK_BACK — polled separately from `TEXT_POLL_VKS` since it edits (pops)
/// rather than appends.
#[cfg(all(not(target_arch = "wasm32"), windows))]
const VK_BACK: i32 = 0x08;
#[cfg(all(not(target_arch = "wasm32"), windows))]
const VK_SHIFT: i32 = 0x10;

/// Read the OS's live key-state table directly — unlike minifb's
/// `is_key_down`/`get_keys_pressed` (populated from `WM_KEYDOWN`, which only
/// arrives at a window holding real Win32 keyboard focus), this works even
/// when the borderless-fullscreen window is topmost/visually in front but
/// didn't actually win the OS focus fight (Windows' foreground-lock — see
/// `force_window_focus`). High bit of `GetAsyncKeyState` = currently down.
#[cfg(all(not(target_arch = "wasm32"), windows))]
fn os_key_down(vk: i32) -> bool {
    unsafe {
        extern "system" {
            fn GetAsyncKeyState(vkey: i32) -> i16;
        }
        (GetAsyncKeyState(vk) as u16 & 0x8000) != 0
    }
}

/// True when `hwnd` is the actual OS foreground window. `GetAsyncKeyState`
/// (see `os_key_down`) reads the global key-state table regardless of which
/// window is focused, so the `topmost_window` input fallback must check this
/// before trusting it — otherwise alt-tabbing away from the topmost/
/// fullscreen window to type in some other app would still feed keystrokes
/// into the game sitting behind it, since nothing about GetAsyncKeyState
/// itself would notice the window lost focus.
#[cfg(all(not(target_arch = "wasm32"), windows))]
fn window_is_foreground(hwnd: isize) -> bool {
    unsafe {
        extern "system" {
            fn GetForegroundWindow() -> isize;
        }
        hwnd != 0 && GetForegroundWindow() == hwnd
    }
}

/// Delay (seconds) a key must stay held before it starts auto-repeating —
/// matches the initial "hesitation" of a normal OS text field.
#[cfg(all(not(target_arch = "wasm32"), windows))]
const KEY_REPEAT_DELAY: f64 = 0.45;
/// Interval (seconds) between repeats once a held key is auto-repeating.
#[cfg(all(not(target_arch = "wasm32"), windows))]
const KEY_REPEAT_RATE: f64 = 0.045;

/// Edge/repeat detector for the `GetAsyncKeyState` text-input fallback:
/// fires true on the initial press, then again every `KEY_REPEAT_RATE`
/// seconds once the key has been held past `KEY_REPEAT_DELAY` — the same
/// press-then-hold-to-repeat behavior minifb's `KeyRepeat::Yes` gives the
/// normal (focused-window) path, which this fallback otherwise lacks since
/// it does its own from-scratch edge detection per VK code.
#[cfg(all(not(target_arch = "wasm32"), windows))]
fn key_repeat_fire(now: f64, down: bool, was_down: bool, down_since: &mut f64, last_fire: &mut f64) -> bool {
    if !down {
        return false;
    }
    if !was_down {
        *down_since = now;
        *last_fire = now;
        return true;
    }
    if now - *down_since >= KEY_REPEAT_DELAY && now - *last_fire >= KEY_REPEAT_RATE {
        *last_fire = now;
        return true;
    }
    false
}

/// Map the same key-name strings `key_down`/`key_pressed` already accept
/// (see `str_to_minifb_key`) to Win32 virtual-key codes, for the
/// `GetAsyncKeyState` fallback path.
#[cfg(all(not(target_arch = "wasm32"), windows))]
fn str_to_vk(name: &str) -> Option<i32> {
    Some(match name {
        "numpad0" | "kp0" => 0x60,
        "numpad1" | "kp1" => 0x61,
        "numpad2" | "kp2" => 0x62,
        "numpad3" | "kp3" => 0x63,
        "numpad4" | "kp4" => 0x64,
        "numpad5" | "kp5" => 0x65,
        "numpad6" | "kp6" => 0x66,
        "numpad7" | "kp7" => 0x67,
        "numpad8" | "kp8" => 0x68,
        "numpad9" | "kp9" => 0x69,
        "numpad+" | "kp+" => 0x6B,
        "numpad-" | "kp-" => 0x6D,
        "numpad*" | "kp*" => 0x6A,
        "numpad/" | "kp/" => 0x6F,
        "left" => 0x25,
        "up" => 0x26,
        "right" => 0x27,
        "down" => 0x28,
        "space" => 0x20,
        "enter" => 0x0D,
        "escape" => 0x1B,
        "pageup" => 0x21,
        "pagedown" => 0x22,
        "lshift" | "leftshift" => 0xA0,
        "rshift" | "rightshift" => 0xA1,
        "lctrl" | "leftctrl" => 0xA2,
        "rctrl" | "rightctrl" => 0xA3,
        "lalt" | "leftalt" => 0xA4,
        "ralt" | "rightalt" => 0xA5,
        "tab" => 0x09,
        "backspace" => VK_BACK,
        "delete" => 0x2E,
        "insert" => 0x2D,
        "home" => 0x24,
        "end" => 0x23,
        "a" => 0x41,
        "b" => 0x42,
        "c" => 0x43,
        "d" => 0x44,
        "e" => 0x45,
        "f" => 0x46,
        "g" => 0x47,
        "h" => 0x48,
        "i" => 0x49,
        "j" => 0x4A,
        "k" => 0x4B,
        "l" => 0x4C,
        "m" => 0x4D,
        "n" => 0x4E,
        "o" => 0x4F,
        "p" => 0x50,
        "q" => 0x51,
        "r" => 0x52,
        "s" => 0x53,
        "t" => 0x54,
        "u" => 0x55,
        "v" => 0x56,
        "w" => 0x57,
        "x" => 0x58,
        "y" => 0x59,
        "z" => 0x5A,
        "0" => 0x30,
        "1" => 0x31,
        "2" => 0x32,
        "3" => 0x33,
        "4" => 0x34,
        "5" => 0x35,
        "6" => 0x36,
        "7" => 0x37,
        "8" => 0x38,
        "9" => 0x39,
        _ => return None,
    })
}

/// Lowercase-hex encode bytes (the wire format for crypto values in Ling).
fn hex_encode(bytes: &[u8]) -> String {
    let mut s = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        s.push_str(&format!("{b:02x}"));
    }
    s
}

/// Decode a `ling convert` blob: base64 → zlib-inflate → raw little-endian bytes.
#[cfg(not(target_arch = "wasm32"))]
fn decode_blob(s: &str) -> Result<Vec<u8>, String> {
    use base64::Engine as _;
    use std::io::Read as _;
    let comp = base64::engine::general_purpose::STANDARD
        .decode(s.trim())
        .map_err(|e| format!("base64: {e}"))?;
    let mut out = Vec::new();
    flate2::read::ZlibDecoder::new(&comp[..])
        .read_to_end(&mut out)
        .map_err(|e| format!("inflate: {e}"))?;
    Ok(out)
}

/// Decode a lowercase/uppercase hex string to bytes (ignores malformed tail).
#[cfg(not(target_arch = "wasm32"))]
fn hex_decode(s: &str) -> Vec<u8> {
    let s = s.trim();
    (0..s.len() / 2)
        .filter_map(|i| u8::from_str_radix(s.get(i * 2..i * 2 + 2)?, 16).ok())
        .collect()
}

/// Decode a hex string into a fixed 32-byte key (zero-padded / truncated).
#[cfg(not(target_arch = "wasm32"))]
fn hex_to_32(s: &str) -> [u8; 32] {
    let v = hex_decode(s);
    let mut out = [0u8; 32];
    let n = v.len().min(32);
    out[..n].copy_from_slice(&v[..n]);
    out
}

/// Builds a minimal, valid PDF with one page per input image (decoded via
/// `image`, page sized to the image's own pixel dimensions), backing the
/// `pdf_from_images` builtin. No PDF crate: each page is three objects
/// (Page / Contents / Image XObject) hand-written directly, with the image
/// stream flate-compressed raw RGB8 (`/Filter /FlateDecode /ColorSpace
/// /DeviceRGB /BitsPerComponent 8`) — no separate re-encoding step beyond
/// what `flate2` (already a dependency) does for the stream itself.
#[cfg(all(not(target_arch = "wasm32"), feature = "web"))]
fn build_pdf_from_images(paths: &[String], out_path: &str) -> Result<(), Box<dyn std::error::Error>> {
    use std::io::Write as _;

    struct PageImg {
        w: u32,
        h: u32,
        compressed: Vec<u8>,
    }

    let mut pages = Vec::with_capacity(paths.len());
    for p in paths {
        let img = image::open(p)?.to_rgb8();
        let (w, h) = (img.width(), img.height());
        let mut enc = flate2::write::ZlibEncoder::new(Vec::new(), flate2::Compression::default());
        enc.write_all(img.as_raw())?;
        pages.push(PageImg { w, h, compressed: enc.finish()? });
    }

    let n = pages.len();
    // Object numbers: 1=Catalog, 2=Pages, then per page i (0-indexed):
    // 3+3i=Page, 4+3i=Contents, 5+3i=Image XObject.
    let page_nums: Vec<u32> = (0..n).map(|i| 3 + (i as u32) * 3).collect();
    let total_objs = 2 + n * 3;
    let mut off = vec![0usize; total_objs + 1]; // 1-based; off[0] unused

    let mut buf: Vec<u8> = Vec::new();
    buf.extend_from_slice(b"%PDF-1.4\n%\xE2\xE3\xCF\xD3\n");

    off[1] = buf.len();
    buf.extend_from_slice(b"1 0 obj\n<< /Type /Catalog /Pages 2 0 R >>\nendobj\n");

    off[2] = buf.len();
    let kids = page_nums.iter().map(|n| format!("{n} 0 R")).collect::<Vec<_>>().join(" ");
    buf.extend_from_slice(format!("2 0 obj\n<< /Type /Pages /Kids [{kids}] /Count {n} >>\nendobj\n").as_bytes());

    for (i, page) in pages.iter().enumerate() {
        let page_num = page_nums[i];
        let content_num = page_num + 1;
        let image_num = page_num + 2;
        let (w, h) = (page.w, page.h);

        off[page_num as usize] = buf.len();
        buf.extend_from_slice(format!(
            "{page_num} 0 obj\n<< /Type /Page /Parent 2 0 R /MediaBox [0 0 {w} {h}] /Resources << /XObject << /Im0 {image_num} 0 R >> >> /Contents {content_num} 0 R >>\nendobj\n"
        ).as_bytes());

        let content = format!("q {w} 0 0 {h} 0 0 cm /Im0 Do Q");
        off[content_num as usize] = buf.len();
        buf.extend_from_slice(format!(
            "{content_num} 0 obj\n<< /Length {} >>\nstream\n{content}\nendstream\nendobj\n",
            content.len()
        ).as_bytes());

        off[image_num as usize] = buf.len();
        buf.extend_from_slice(format!(
            "{image_num} 0 obj\n<< /Type /XObject /Subtype /Image /Width {w} /Height {h} /ColorSpace /DeviceRGB /BitsPerComponent 8 /Filter /FlateDecode /Length {} >>\nstream\n",
            page.compressed.len()
        ).as_bytes());
        buf.extend_from_slice(&page.compressed);
        buf.extend_from_slice(b"\nendstream\nendobj\n");
    }

    let xref_offset = buf.len();
    buf.extend_from_slice(format!("xref\n0 {}\n", total_objs + 1).as_bytes());
    buf.extend_from_slice(b"0000000000 65535 f \n");
    for entry in off.iter().skip(1) {
        buf.extend_from_slice(format!("{entry:010} 00000 n \n").as_bytes());
    }
    buf.extend_from_slice(
        format!("trailer\n<< /Size {} /Root 1 0 R >>\nstartxref\n{xref_offset}\n%%EOF", total_objs + 1).as_bytes(),
    );

    std::fs::write(out_path, buf)?;
    Ok(())
}

fn tex_rgb(r: f32, g: f32, b: f32) -> u32 {
    ((r * 255.0) as u32) << 16 | ((g * 255.0) as u32) << 8 | (b * 255.0) as u32
}

// ─── 3D Perlin Noise (Improved Perlin 2002) ───────────────────────────────────

const PERM: [u8; 512] = [
    151, 160, 137, 91, 90, 15, 131, 13, 201, 95, 96, 53, 194, 233, 7, 225, 140, 36, 103, 30, 69,
    142, 8, 99, 37, 240, 21, 10, 23, 190, 6, 148, 247, 120, 234, 75, 0, 26, 197, 62, 94, 252, 219,
    203, 117, 35, 11, 32, 57, 177, 33, 88, 237, 149, 56, 87, 174, 35, 63, 189, 114, 56, 42, 123,
    165, 38, 72, 93, 69, 139, 138, 78, 149, 159, 56, 89, 152, 78, 61, 140, 63, 26, 142, 76, 124,
    132, 72, 11, 90, 44, 82, 59, 96, 41, 148, 126, 157, 13, 49, 27, 176, 33, 47, 14, 97, 78, 71,
    40, 87, 183, 4, 122, 92, 7, 72, 3, 246, 17, 225, 87, 91, 106, 203, 190, 57, 74, 76, 88, 207,
    208, 239, 170, 251, 67, 77, 51, 133, 69, 249, 2, 127, 80, 60, 159, 168, 81, 163, 64, 143, 146,
    157, 56, 245, 188, 182, 218, 33, 16, 255, 243, 210, 205, 12, 19, 236, 95, 151, 68, 23, 196,
    167, 126, 61, 100, 93, 25, 115, 96, 129, 79, 220, 34, 42, 144, 136, 70, 238, 184, 20, 222, 94,
    11, 219, 224, 50, 58, 10, 73, 6, 36, 92, 194, 211, 172, 98, 145, 149, 228, 121, 231, 200, 55,
    109, 141, 213, 78, 169, 108, 86, 244, 234, 101, 122, 174, 8, 186, 120, 37, 46, 28, 166, 180,
    198, 232, 221, 116, 31, 75, 189, 139, 138, 112, 62, 181, 102, 72, 3, 246, 14, 97, 53, 87, 185,
    134, 193, 29, 158, 225, 248, 152, 17, 105, 217, 142, 148, 155, 30, 135, 233, 206, 85, 40, 223,
    140, 161, 137, 13, 191, 230, 66, 104, 153, 199, 167, 147, 99, 179, 92,
    // Duplicate for wrap-around indexing
    151, 160, 137, 91, 90, 15, 131, 13, 201, 95, 96, 53, 194, 233, 7, 225, 140, 36, 103, 30, 69,
    142, 8, 99, 37, 240, 21, 10, 23, 190, 6, 148, 247, 120, 234, 75, 0, 26, 197, 62, 94, 252, 219,
    203, 117, 35, 11, 32, 57, 177, 33, 88, 237, 149, 56, 87, 174, 35, 63, 189, 114, 56, 42, 123,
    165, 38, 72, 93, 69, 139, 138, 78, 149, 159, 56, 89, 152, 78, 61, 140, 63, 26, 142, 76, 124,
    132, 72, 11, 90, 44, 82, 59, 96, 41, 148, 126, 157, 13, 49, 27, 176, 33, 47, 14, 97, 78, 71,
    40, 87, 183, 4, 122, 92, 7, 72, 3, 246, 17, 225, 87, 91, 106, 203, 190, 57, 74, 76, 88, 207,
    208, 239, 170, 251, 67, 77, 51, 133, 69, 249, 2, 127, 80, 60, 159, 168, 81, 163, 64, 143, 146,
    157, 56, 245, 188, 182, 218, 33, 16, 255, 243, 210, 205, 12, 19, 236, 95, 151, 68, 23, 196,
    167, 126, 61, 100, 93, 25, 115, 96, 129, 79, 220, 34, 42, 144, 136, 70, 238, 184, 20, 222, 94,
    11, 219, 224, 50, 58, 10, 73, 6, 36, 92, 194, 211, 172, 98, 145, 149, 228, 121, 231, 200, 55,
    109, 141, 213, 78, 169, 108, 86, 244, 234, 101, 122, 174,
];

fn fade(t: f32) -> f32 {
    t * t * t * (t * (t * 6.0 - 15.0) + 10.0)
}

fn grad(hash: u8, x: f32, y: f32, z: f32) -> f32 {
    let h = hash & 15;
    let u = if h < 8 { x } else { y };
    let v = if h < 8 { y } else { z };
    (if (h & 1) == 0 { u } else { -u }) + (if (h & 2) == 0 { v } else { -v })
}

fn perlin3(x: f32, y: f32, z: f32) -> f32 {
    let xi = (x.floor() as i32) & 255;
    let yi = (y.floor() as i32) & 255;
    let zi = (z.floor() as i32) & 255;

    let xf = x - x.floor();
    let yf = y - y.floor();
    let zf = z - z.floor();

    let u = fade(xf);
    let v = fade(yf);
    let w = fade(zf);

    let p0 = PERM[xi as usize] as usize;
    let p1 = PERM[((xi + 1) & 255) as usize] as usize;
    let pa = PERM[(p0 + yi as usize) & 255] as usize;
    let pb = PERM[(p0 + ((yi + 1) & 255) as usize) & 255] as usize;
    let pc = PERM[(p1 + yi as usize) & 255] as usize;
    let pd = PERM[(p1 + ((yi + 1) & 255) as usize) & 255] as usize;

    let g000 = grad(PERM[(pa + zi as usize) & 255], xf, yf, zf);
    let g001 = grad(
        PERM[(pa + ((zi + 1) & 255) as usize) & 255],
        xf,
        yf,
        zf - 1.0,
    );
    let g010 = grad(PERM[(pb + zi as usize) & 255], xf, yf - 1.0, zf);
    let g011 = grad(
        PERM[(pb + ((zi + 1) & 255) as usize) & 255],
        xf,
        yf - 1.0,
        zf - 1.0,
    );
    let g100 = grad(PERM[(pc + zi as usize) & 255], xf - 1.0, yf, zf);
    let g101 = grad(
        PERM[(pc + ((zi + 1) & 255) as usize) & 255],
        xf - 1.0,
        yf,
        zf - 1.0,
    );
    let g110 = grad(PERM[(pd + zi as usize) & 255], xf - 1.0, yf - 1.0, zf);
    let g111 = grad(
        PERM[(pd + ((zi + 1) & 255) as usize) & 255],
        xf - 1.0,
        yf - 1.0,
        zf - 1.0,
    );

    let l00 = g000 + u * (g100 - g000);
    let l01 = g001 + u * (g101 - g001);
    let l10 = g010 + u * (g110 - g010);
    let l11 = g011 + u * (g111 - g011);

    let l0 = l00 + v * (l10 - l00);
    let l1 = l01 + v * (l11 - l01);

    l0 + w * (l1 - l0)
}

fn fast_rand_f64(state: &mut u64) -> f64 {
    *state = state
        .wrapping_mul(6364136223846793005)
        .wrapping_add(1442695040888963407);
    ((*state >> 32) as u32) as f64 / 4294967296.0
}

// ─── Circle Drawing Primitives ────────────────────────────────────────────────

/// Write one pixel into the framebuffer (normal or additive blend).
#[inline]
fn put_px(buf: &mut [u32], idx: usize, color: u32, blend: u8) {
    if idx >= buf.len() {
        return;
    }
    if blend == 0 {
        buf[idx] = color;
    } else {
        let old = buf[idx];
        let r = (((old >> 16) & 255) + ((color >> 16) & 255)).min(255);
        let g = (((old >> 8) & 255) + ((color >> 8) & 255)).min(255);
        let b = ((old & 255) + (color & 255)).min(255);
        buf[idx] = (r << 16) | (g << 8) | b;
    }
}

/// Pack three float colour channels (0..255) into a 0x00RRGGBB word, clamping.
#[inline]
fn rgb(r: f64, g: f64, b: f64) -> u32 {
    let r = (r as i64).clamp(0, 255) as u32;
    let g = (g as i64).clamp(0, 255) as u32;
    let b = (b as i64).clamp(0, 255) as u32;
    (r << 16) | (g << 8) | b
}

#[allow(clippy::too_many_arguments)]
fn draw_circle_outline(
    buf: &mut [u32],
    w: i32,
    h: i32,
    cx: i32,
    cy: i32,
    r: i32,
    color: u32,
    blend: u8,
) {
    let r = r.clamp(0, 20000); // guard against overflow / runaway from tiny depths
    if r == 0 {
        return;
    }
    let mut x = 0;
    let mut y = r;
    let mut d = 3 - 2 * r;
    while x <= y {
        plot_circle_points(buf, w, h, cx, cy, x, y, color, blend);
        if d < 0 {
            d += 4 * x + 6;
        } else {
            d += 4 * (x - y) + 10;
            y -= 1;
        }
        x += 1;
    }
}

#[allow(clippy::too_many_arguments)]
fn plot_circle_points(
    buf: &mut [u32],
    w: i32,
    h: i32,
    cx: i32,
    cy: i32,
    x: i32,
    y: i32,
    color: u32,
    blend: u8,
) {
    let points = [
        (cx + x, cy + y),
        (cx - x, cy + y),
        (cx + x, cy - y),
        (cx - x, cy - y),
        (cx + y, cy + x),
        (cx - y, cy + x),
        (cx + y, cy - x),
        (cx - y, cy - x),
    ];
    for &(px, py) in &points {
        if px >= 0 && px < w && py >= 0 && py < h {
            put_px(buf, (py * w + px) as usize, color, blend);
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn draw_circle_filled(
    buf: &mut [u32],
    w: i32,
    h: i32,
    cx: i32,
    cy: i32,
    r: i32,
    color: u32,
    blend: u8,
) {
    if r <= 0 {
        return;
    }
    for dy in -r..=r {
        let dx_max = ((r * r - dy * dy) as f64).sqrt() as i32;
        let py = cy + dy;
        if py < 0 || py >= h {
            continue;
        }
        for dx in -dx_max..=dx_max {
            let px = cx + dx;
            if px >= 0 && px < w {
                put_px(buf, (py * w + px) as usize, color, blend);
            }
        }
    }
}

#[cfg(test)]
mod draw_tests {
    use super::*;

    #[test]
    fn filled_circle_actually_writes_pixels() {
        let mut buf = vec![0u32; 100 * 100];
        draw_circle_filled(&mut buf, 100, 100, 50, 50, 10, 0xFF00FF, 0);
        assert_eq!(buf[50 * 100 + 50], 0xFF00FF, "centre pixel must be filled");
        assert_eq!(buf[0], 0, "far corner must stay clear");
        let n = buf.iter().filter(|&&p| p != 0).count();
        assert!(n > 200 && n < 500, "r=10 disc area ≈ 314, got {n}");
    }

    #[test]
    fn circle_outline_writes_a_ring() {
        let mut buf = vec![0u32; 100 * 100];
        draw_circle_outline(&mut buf, 100, 100, 50, 50, 20, 0x00FF00, 0);
        assert_eq!(buf[50 * 100 + 50], 0, "outline must NOT fill the centre");
        assert!(
            buf.iter().any(|&p| p == 0x00FF00),
            "outline must draw a ring"
        );
    }

    #[test]
    fn additive_blend_accumulates_channels() {
        let mut buf = vec![0x202020u32; 1];
        put_px(&mut buf, 0, 0x404040, 1);
        assert_eq!(buf[0], 0x606060);
    }
}

// ─── Interpreter ─────────────────────────────────────────────────────────────

/// Customizable colour palette for the vector UI toolkit (packed 0x00RRGGBB).
/// `ui_theme(...)` sets it; every widget falls back to these and accepts a
/// trailing `r,g,b` override.
#[derive(Clone, Copy)]
pub struct UiTheme {
    pub primary: u32,
    pub accent: u32,
    pub track: u32,
    pub warn: u32,
    pub text: u32,
    pub bg: u32,
}

impl Default for UiTheme {
    fn default() -> Self {
        Self {
            primary: 0x00D2FF, // holographic cyan
            accent: 0x28FFB4,  // mint
            track: 0x2C3E64,   // dim slate
            warn: 0xFF5A5A,    // alert red
            text: 0xBEEBFF,    // pale cyan
            bg: 0x0A1018,      // near-black panel
        }
    }
}

#[cfg_attr(target_arch = "wasm32", allow(dead_code))]
pub struct Interpreter {
    globals: HashMap<String, Expr>,
    /// Globals evaluated ONCE at program start (immutable after load).
    /// call_named clones this instead of re-evaluating every global per call.
    global_seed: Env,
    functions: FxHashMap<String, Rc<FnDef>>,
    /// `form` definitions: struct name → ordered field names.
    pub(crate) structs: HashMap<String, Vec<String>>,
    /// `choose` variants: variant name (bare and `Enum::Variant`) → (enum name, arity).
    enum_variants: HashMap<String, (String, usize)>,
    _modules: HashMap<String, Vec<FnDef>>,
    gfx: RefCell<GfxState>,
    svg: RefCell<Option<SvgWriter>>,
    /// Directory of the primary source file, for relative `use` resolution.
    pub source_dir: Option<std::path::PathBuf>,
    /// Files already loaded — prevents circular imports.
    loaded_files: std::collections::HashSet<String>,
    /// Optional audio engine — `None` if no audio device is available.
    #[cfg(not(target_arch = "wasm32"))]
    audio: Option<AudioEngine>,
    #[cfg(not(target_arch = "wasm32"))]
    fft: RefCell<FftAnalyzer>,
    fft_bands_cache: RefCell<Vec<f32>>,
    /// Real-time clock — seconds since Unix epoch at startup (f64 works on both
    /// native and wasm32; Instant is not available on wasm32).
    start_time_secs: f64,
    /// Frame counter — incremented at each present()
    frame_num: u64,
    /// Target framerate used to pace `present()` on wasm32.
    #[cfg(target_arch = "wasm32")]
    wasm_target_fps: f64,
    /// Next frame deadline (ms since epoch) for wasm frame pacing.
    #[cfg(target_arch = "wasm32")]
    wasm_next_present_ms: f64,
    /// Random state for rand() builtin (xorshift)
    rand_state: u64,
    /// Microphone input (Phase 1 audio reactivity)
    #[cfg(not(target_arch = "wasm32"))]
    mic: Option<ling_mic::MicInput>,
    /// Persistent KEM keypairs (knot / hybrid identities), referenced by handle.
    #[cfg(not(target_arch = "wasm32"))]
    crypto_ids: Vec<ling_crypto::KnotIdentity>,
    /// Persistent Ed25519 signing keypairs, referenced by handle.
    #[cfg(not(target_arch = "wasm32"))]
    ed25519_ids: Vec<ling_crypto::Ed25519Keypair>,
    /// Editable text-input buffer (ling-ui text fields).
    text_buffer: String,
    /// Frame counter for record_frame().
    record_n: u32,
    /// Accumulated microphone samples (for turning sound into crypto donuts).
    #[cfg(not(target_arch = "wasm32"))]
    mic_buffer: Vec<f32>,
    /// Loaded vector UI fonts, referenced by handle (index) from `font_load`.
    #[cfg(not(target_arch = "wasm32"))]
    fonts: Vec<ling_graphics::VectorFont>,
    /// Loaded raster images (PNG/etc.), referenced by handle (index) from
    /// `image_load` — read pixel-by-pixel via `image_pixel_r/g/b/a`.
    images: Vec<image::RgbaImage>,
    /// Customizable UI colour palette (set via `ui_theme`).
    ui_theme: UiTheme,
    /// Left-mouse state on the previous frame — for widget click-edge detection.
    mouse_was_down: bool,
    /// Live music engine (decode playback + GM synth) — lazily initialised.
    #[cfg(not(target_arch = "wasm32"))]
    music: Option<ling_music::MusicEngine>,
    #[cfg(not(target_arch = "wasm32"))]
    music_init: bool,
    /// Decoded tracks (for analysis + playback), by `music_load` handle.
    tracks: Vec<ling_music::DecodedAudio>,
    /// Parsed `.lrc` lyrics, by `music_lrc` handle.
    lyrics: Vec<ling_music::Lyrics>,
    /// Parsed MIDI songs, by `music_midi_load` handle.
    midis: Vec<ling_music::MidiSong>,
    /// Soft bodies (deformable balls), by `soft_ball` handle.
    soft_bodies: Vec<ling_physics::soft::SoftBody>,
    /// Rigid-body world (angular dynamics), shared by `rb_*`.
    rigid_world: ling_physics::rigid::PhysicsWorld,
    /// Liquid grids (water/oil), by `liquid_new` handle.
    liquids: Vec<ling_physics::liquid::LiquidGrid>,
    meshes: Vec<crate::gfx::shapes::ColorMesh>,
    /// Loaded glTF models (skeleton + skin weights + animations), by `mesh_load` handle.
    gltf_models: std::cell::RefCell<Vec<ling_physics::gltf::GltfModel>>,
    /// Active cinematic dialog box (Ocarina/Majora-style), if any.
    dialog: Option<ling_game::dialog::Dialog>,
    /// Dialog highlight colours by role: text, name, place, item (0x00RRGGBB).
    dialog_colors: [u32; 4],
    /// Active user-function call frames (names), for error tracebacks.
    frames: Vec<String>,
    /// Snapshot of `frames` captured the moment a runtime error first arose
    /// (the deepest call). Consumed by `take_error_trace`.
    error_trace: Option<Vec<String>>,
    /// Unified input (gamepads/joysticks/VR/touch via the ling-input
    /// "Sensorium"). Lazily initialised on the first `pad_*` builtin call;
    /// `None` if no native input backend is available.
    #[cfg(not(target_arch = "wasm32"))]
    input: RefCell<Option<InputState>>,
    /// Routes registered by `http_route(method, path, handler)`, consumed
    /// by `http_serve`. Lives here (not a global, unlike `net`) because the
    /// handler is a `Value::Fn` closure — `Value` holds `Rc` and so can't
    /// safely live in a `static`.
    #[cfg(all(not(target_arch = "wasm32"), feature = "web"))]
    http_routes: Vec<(String, String, Value)>,
    /// SQLite handle opened by `db_open` — plain rusqlite Connection, no
    /// pool: the interpreter is single-threaded, so one connection is both
    /// sufficient and contention-free.
    #[cfg(all(not(target_arch = "wasm32"), feature = "web"))]
    db: Option<ling_http::rusqlite::Connection>,
    /// `(url_prefix, disk_dir)` pairs registered by `http_static`, consumed by
    /// `http_serve` — served as raw bytes, bypassing the String-only Value bridge.
    #[cfg(all(not(target_arch = "wasm32"), feature = "web"))]
    http_static_dirs: Vec<(String, String)>,
    /// Background jobs started by `http_post_async`, polled by `http_job_poll`.
    /// A plain `Arc<Mutex<..>>` handle (not `Value`), so it's fine to touch from
    /// the background tokio task that fills in each job's result.
    #[cfg(all(not(target_arch = "wasm32"), feature = "web"))]
    async_jobs: web::AsyncJobs,
}

/// Live gamepad input state: a ling-input hub fed by the native `gilrs` backend.
#[cfg(not(target_arch = "wasm32"))]
struct InputState {
    sensorium: ling_input::Sensorium,
    backend: ling_input::backend::GilrsBackend,
}

impl Default for Interpreter {
    fn default() -> Self {
        Self::new()
    }
}

impl Interpreter {
    pub fn new() -> Self {
        #[cfg(not(target_arch = "wasm32"))]
        let audio = AudioEngine::new()
            .map_err(|e| eprintln!("audio init failed (no sound): {e}"))
            .ok();
        Self {
            globals: HashMap::new(),
            global_seed: new_env(),
            functions: FxHashMap::default(),
            structs: HashMap::new(),
            enum_variants: HashMap::new(),
            _modules: HashMap::new(),
            gfx: RefCell::new(GfxState::new()),
            svg: RefCell::new(None),
            source_dir: None,
            loaded_files: std::collections::HashSet::new(),
            #[cfg(not(target_arch = "wasm32"))]
            audio,
            #[cfg(not(target_arch = "wasm32"))]
            fft: RefCell::new(FftAnalyzer::new(2048, 44100)),
            fft_bands_cache: RefCell::new(vec![]),
            start_time_secs: crate::runtime::now_secs(),
            frame_num: 0,
            #[cfg(target_arch = "wasm32")]
            wasm_target_fps: 60.0,
            #[cfg(target_arch = "wasm32")]
            wasm_next_present_ms: 0.0,
            rand_state: 0x123456789ABCDEF,
            #[cfg(not(target_arch = "wasm32"))]
            mic: None,
            #[cfg(not(target_arch = "wasm32"))]
            crypto_ids: Vec::new(),
            #[cfg(not(target_arch = "wasm32"))]
            ed25519_ids: Vec::new(),
            text_buffer: String::new(),
            record_n: 0,
            #[cfg(not(target_arch = "wasm32"))]
            mic_buffer: Vec::new(),
            #[cfg(not(target_arch = "wasm32"))]
            fonts: Vec::new(),
            images: Vec::new(),
            ui_theme: UiTheme::default(),
            mouse_was_down: false,
            #[cfg(not(target_arch = "wasm32"))]
            music: None,
            #[cfg(not(target_arch = "wasm32"))]
            music_init: false,
            tracks: Vec::new(),
            lyrics: Vec::new(),
            midis: Vec::new(),
            soft_bodies: Vec::new(),
            rigid_world: ling_physics::rigid::PhysicsWorld::new(),
            liquids: Vec::new(),
            meshes: Vec::new(),
            gltf_models: std::cell::RefCell::new(Vec::new()),
            dialog: None,
            dialog_colors: [0xE6F2FF, 0xFFD24A, 0x4AD2FF, 0x6CFF8C], // text · name · place · item
            frames: Vec::new(),
            error_trace: None,
            #[cfg(not(target_arch = "wasm32"))]
            input: RefCell::new(None),
            #[cfg(all(not(target_arch = "wasm32"), feature = "web"))]
            http_routes: Vec::new(),
            #[cfg(all(not(target_arch = "wasm32"), feature = "web"))]
            db: None,
            #[cfg(all(not(target_arch = "wasm32"), feature = "web"))]
            http_static_dirs: Vec::new(),
            #[cfg(all(not(target_arch = "wasm32"), feature = "web"))]
            async_jobs: web::AsyncJobs::new(),
        }
    }

    /// Lazily initialise the input system and advance it one frame; returns the
    /// number of connected gamepads. Call once per game-loop iteration (like a
    /// window update) before reading `pad_*` state.
    #[cfg(not(target_arch = "wasm32"))]
    fn pad_poll(&self) -> usize {
        let mut slot = self.input.borrow_mut();
        if slot.is_none() {
            match ling_input::backend::GilrsBackend::new() {
                Ok(backend) => {
                    *slot = Some(InputState { sensorium: ling_input::Sensorium::new(4), backend });
                },
                Err(_) => return 0,
            }
        }
        let st = slot.as_mut().unwrap();
        st.sensorium.begin_frame();
        st.sensorium.pump(&mut st.backend);
        st.sensorium.update(1.0 / 60.0);
        st.sensorium.devices.count()
    }

    /// Read player `slot`'s gamepad with `f`, or return `default` if there is no
    /// input system / no such pad.
    #[cfg(not(target_arch = "wasm32"))]
    fn with_pad<T>(&self, slot: usize, default: T, f: impl FnOnce(&ling_input::Gamepad) -> T) -> T {
        let inp = self.input.borrow();
        match inp.as_ref().and_then(|s| s.sensorium.player(slot)) {
            Some(p) => f(p),
            None => default,
        }
    }

    /// Take the call-stack snapshot captured at the deepest runtime error, if any.
    /// Frames are ordered outermost-first (entry point first, failing call last).
    pub fn take_error_trace(&mut self) -> Vec<String> {
        self.error_trace.take().unwrap_or_default()
    }

    #[cfg(target_arch = "wasm32")]
    fn wasm_pace_frame(&mut self) {
        let fps = self.wasm_target_fps.max(1.0);
        let frame_ms = 1000.0 / fps;
        let now = js_sys::Date::now();
        if self.wasm_next_present_ms <= 0.0 {
            self.wasm_next_present_ms = now + frame_ms;
            return;
        }

        let wait_ms = (self.wasm_next_present_ms - now).floor() as i32;
        if wait_ms > 0 {
            wasm_sleep_ms(wait_ms);
        }

        let after = js_sys::Date::now();
        if after > self.wasm_next_present_ms + frame_ms * 3.0 {
            self.wasm_next_present_ms = after + frame_ms;
        } else {
            self.wasm_next_present_ms += frame_ms;
        }
    }

    /// Run `body`, recording `name` as a call frame and snapshotting the stack on
    /// the first runtime error so a traceback can be reported.
    fn framed<T, F>(&mut self, name: &str, body: F) -> Result<T, EvalErr>
    where
        F: FnOnce(&mut Self) -> Result<T, EvalErr>,
    {
        self.frames.push(name.to_string());
        let result = body(self);
        if matches!(result, Err(EvalErr::Runtime(_))) && self.error_trace.is_none() {
            self.error_trace = Some(self.frames.clone());
        }
        self.frames.pop();
        result
    }

    /// Render the active dialog box: beveled frame + dark fill, then the visible
    /// (typewriter-revealed) text word-wrapped with colour-coded runs, plus a
    /// blinking advance arrow once the page is fully typed.
    #[cfg(not(target_arch = "wasm32"))]
    fn render_dialog(&mut self, x: f32, y: f32, w: f32, h: f32, font: i64, t: f32) {
        let (runs, typing) = match &self.dialog {
            Some(d) if !d.is_closed() => {
                let runs: Vec<(String, usize, bool)> = d
                    .visible_runs()
                    .into_iter()
                    .map(|r| (r.text, r.role.index(), r.newline_before))
                    .collect();
                (runs, d.is_typing())
            },
            _ => return,
        };
        let colors = self.dialog_colors;
        // ── frame + fill ──
        let b = 12.0;
        let corners: Vec<[f32; 2]> = vec![
            [x + b, y],
            [x + w - b, y],
            [x + w, y + b],
            [x + w, y + h - b],
            [x + w - b, y + h],
            [x + b, y + h],
            [x, y + h - b],
            [x, y + b],
            [x + b, y],
        ];
        {
            let mut gfx = self.gfx.borrow_mut();
            let (bw, bh) = (gfx.width, gfx.height);
            crate::gfx::raster::fill_contours_aa(
                &mut gfx.buffer,
                bw,
                bh,
                0x0A1018,
                false,
                std::slice::from_ref(&corners),
            );
            for seg in corners.windows(2) {
                crate::gfx::raster::draw_line_aa(
                    &mut gfx.buffer,
                    bw,
                    bh,
                    0x00D2FF,
                    false,
                    seg[0][0],
                    seg[0][1],
                    seg[1][0],
                    seg[1][1],
                );
            }
        }
        // ── word-wrapped, colour-coded text ──
        let px = 22.0f32;
        let pad = 20.0f32;
        let line_h = px * 1.45;
        let mut cx = x + pad;
        let mut cy = y + pad;
        let use_font = font >= 0 && (font as usize) < self.fonts.len();
        for (text, role, nl) in &runs {
            if *nl {
                cx = x + pad;
                cy += line_h;
            }
            for word in text.split_inclusive(' ') {
                let wpx = if use_font {
                    self.fonts[font as usize].measure(word, px)
                } else {
                    ling_ui::holo::text_width(word, px * 0.6, px * 0.24)
                };
                if cx + wpx > x + w - pad && cx > x + pad + 1.0 {
                    cx = x + pad;
                    cy += line_h;
                }
                if cy + line_h > y + h {
                    break;
                }
                let col = colors[(*role).min(3)];
                if use_font {
                    let glyphs = self.font_layout_2d_glyphs(font as usize, cx, cy, px, word);
                    let mut gfx = self.gfx.borrow_mut();
                    let (bw, bh, add) = (gfx.width, gfx.height, gfx.blend == 1);
                    for contours in &glyphs {
                        crate::gfx::raster::fill_contours_aa(
                            &mut gfx.buffer,
                            bw,
                            bh,
                            col,
                            add,
                            contours,
                        );
                    }
                } else {
                    let segs = ling_ui::holo::text_lines(word, cx, cy, px * 0.6, px, px * 0.24);
                    let mut gfx = self.gfx.borrow_mut();
                    let (bw, bh) = (gfx.width, gfx.height);
                    for s in segs {
                        draw_line(&mut gfx.buffer, bw, bh, col, s[0], s[1], s[2], s[3]);
                    }
                }
                cx += wpx;
            }
        }
        // ── blinking advance arrow when fully typed ──
        if !typing && (t * 3.0).sin() > 0.0 {
            let ax = x + w - 26.0;
            let ay = y + h - 22.0;
            let mut gfx = self.gfx.borrow_mut();
            let (bw, bh) = (gfx.width, gfx.height);
            crate::gfx::raster::fill_contours_aa(
                &mut gfx.buffer,
                bw,
                bh,
                0x00D2FF,
                false,
                std::slice::from_ref(&vec![
                    [ax - 7.0, ay],
                    [ax + 7.0, ay],
                    [ax, ay + 9.0],
                    [ax - 7.0, ay],
                ]),
            );
        }
    }

    /// Lazily start the music engine on first use (playback/synth need a device;
    /// analysis/decoding do not). Returns `false` if no audio device is available.
    #[cfg(not(target_arch = "wasm32"))]
    fn ensure_music(&mut self) -> bool {
        if self.music.is_some() {
            return true;
        }
        if self.music_init {
            return false;
        }
        self.music_init = true;
        match ling_music::MusicEngine::new() {
            Ok(m) => {
                self.music = Some(m);
                true
            },
            Err(e) => {
                eprintln!("music engine init failed (no music playback): {e}");
                false
            },
        }
    }

    #[cfg(target_arch = "wasm32")]
    fn wasm_resolve_source_path(&self, path: &str) -> String {
        let p = path.trim();
        if p.is_empty() {
            return String::new();
        }
        if p.contains("://") || p.starts_with('/') || p.starts_with("./") || p.starts_with("../") {
            return p.to_string();
        }
        if let Some(d) = &self.source_dir {
            let base = d.to_string_lossy().replace('\\', "/");
            if !base.is_empty() {
                return format!(
                    "{}/{}",
                    base.trim_end_matches('/'),
                    p.trim_start_matches("./")
                );
            }
        }
        p.to_string()
    }

    #[cfg(target_arch = "wasm32")]
    fn wasm_music_builtin(&mut self, name: &str, args: &[Value]) -> Result<Option<Value>, EvalErr> {
        match name {
            // music_load(path) -> track handle (decode from fetched bytes)
            "music_load" | "载入音乐" | "音楽読込" | "음악로드" | "โหลดเพลง" | "بارگذاری_موسیقی" | "تحميل_الموسيقى" | "טעינת_מוזיקה" | "موسیقی_لوڈ" | "charger_musique" | "musik_laden" | "загрузить_музыку" =>
            {
                let path = self.arg_str(args, 0, "");
                let resolved = self.wasm_resolve_source_path(&path);
                match wasm_fetch_bytes(&resolved)
                    .and_then(|bytes| ling_music::from_bytes(&bytes).map_err(|e| e.to_string()))
                {
                    Ok(t) => {
                        let id = self.tracks.len();
                        self.tracks.push(t);
                        return Ok(Some(Value::Number(id as f64)));
                    },
                    Err(e) => {
                        eprintln!("music_load failed ({path}): {e}");
                        return Ok(Some(Value::Number(-1.0)));
                    },
                }
            },
            "music_duration" | "音乐时长" | "音楽長さ" | "음악길이" | "ความยาวเพลง" | "مدت_موسیقی" | "مدة_الموسيقى" | "משך_מוזיקה" | "موسیقی_دورانیہ" | "durée_musique" | "musik_dauer" | "длительность_музыки" =>
            {
                let id = self.arg_num(args, 0, 0.0)? as i64;
                let d = self
                    .tracks
                    .get(id as usize)
                    .map(|t| t.duration)
                    .unwrap_or(0.0);
                return Ok(Some(Value::Number(d as f64)));
            },
            "music_bpm" | "节拍速度" | "テンポ" | "템포" | "จังหวะต่อนาที" | "ضربان_در_دقیقه" | "نبضات_بالدقيقة" | "פעימות_לדקה" | "بی_پی_ایم" | "bpm_musique" | "musik_bpm" | "музыка_bpm" =>
            {
                let id = self.arg_num(args, 0, 0.0)? as i64;
                let b = self
                    .tracks
                    .get(id as usize)
                    .map(|t| ling_music::analysis::bpm(&t.mono, t.rate))
                    .unwrap_or(0.0);
                return Ok(Some(Value::Number(b as f64)));
            },
            "music_key" | "调性" | "調性" | "조성" | "คีย์เพลง" | "گام_موسیقی" | "مقام_الموسيقى" | "סולם_מוזיקלי" | "موسیقی_کلید" | "tonalité_musique" | "musik_tonart" | "тональность_музыки" => {
                let id = self.arg_num(args, 0, 0.0)? as i64;
                let k = self
                    .tracks
                    .get(id as usize)
                    .map(|t| ling_music::analysis::key_name(&t.mono, t.rate))
                    .unwrap_or_default();
                return Ok(Some(Value::Str(k)));
            },
            "music_onsets" | "音符起点" | "オンセット" | "온셋" | "จุดเริ่มเสียง" | "آغازهای_نت" | "بدايات_النغمات" | "התחלות_תווים" | "نوٹ_شروعات" | "attaques_musique" | "musik_einsätze" | "атаки_музыки" =>
            {
                let id = self.arg_num(args, 0, 0.0)? as i64;
                let v = self
                    .tracks
                    .get(id as usize)
                    .map(|t| ling_music::analysis::onsets(&t.mono, t.rate))
                    .unwrap_or_default();
                return Ok(Some(Value::List(
                    v.into_iter().map(|x| Value::Number(x as f64)).collect::<Vec<_>>().into(),
                )));
            },
            "music_beat_grid" | "节拍网格" | "ビートグリッド" | "비트그리드" | "กริดจังหวะ" | "شبکه_ضرب" | "شبكة_الإيقاع" | "רשת_פעימות" | "بیٹ_گرڈ" | "grille_temps_musique" | "musik_taktraster" | "сетка_ритма_музыки" =>
            {
                let id = self.arg_num(args, 0, 0.0)? as i64;
                let beats = self
                    .tracks
                    .get(id as usize)
                    .map(|t| {
                        let b = ling_music::analysis::bpm(&t.mono, t.rate);
                        ling_music::analysis::beat_grid(&t.mono, t.rate, b)
                    })
                    .unwrap_or_default();
                return Ok(Some(Value::List(
                    beats.into_iter().map(|x| Value::Number(x as f64)).collect::<Vec<_>>().into(),
                )));
            },
            "music_lrc" | "载入歌词" | "歌詞読込" | "가사로드" | "โหลดเนื้อเพลง" | "بارگذاری_متن_ترانه" | "تحميل_كلمات_الأغنية" | "טעינת_מילות_שיר" | "گیت_متن_لوڈ" | "lrc_musique" | "musik_lrc" | "lrc_музыки" =>
            {
                let path = self.arg_str(args, 0, "");
                let resolved = self.wasm_resolve_source_path(&path);
                match wasm_fetch_text(&resolved) {
                    Ok(text) => {
                        let id = self.lyrics.len();
                        self.lyrics.push(ling_music::Lyrics::parse(&text));
                        return Ok(Some(Value::Number(id as f64)));
                    },
                    Err(e) => {
                        eprintln!("music_lrc failed ({path}): {e}");
                        return Ok(Some(Value::Number(-1.0)));
                    },
                }
            },
            "music_lyric" | "当前歌词" | "現在歌詞" | "현재가사" | "เนื้อเพลงปัจจุบัน" | "متن_ترانه_فعلی" | "كلمات_الأغنية_الحالية" | "מילות_שיר_נוכחיות" | "موجودہ_گیت_متن" | "paroles_musique" | "musik_liedtext" | "текст_песни" =>
            {
                let id = self.arg_num(args, 0, 0.0)? as i64;
                let t = self.arg_num(args, 1, 0.0)? as f32;
                let line = self
                    .lyrics
                    .get(id as usize)
                    .map(|l| l.line_at(t).to_string())
                    .unwrap_or_default();
                return Ok(Some(Value::Str(line)));
            },
            "music_midi_load" | "载入MIDI" | "MIDI読込" | "미디로드" | "โหลดมิดี" | "بارگذاری_MIDI" | "تحميل_MIDI" | "טעינת_MIDI" | "MIDI_لوڈ" | "charger_midi_musique" | "musik_midi_laden" | "загрузить_midi_музыки" =>
            {
                let path = self.arg_str(args, 0, "");
                let resolved = self.wasm_resolve_source_path(&path);
                match wasm_fetch_bytes(&resolved).and_then(|bytes| {
                    ling_music::midi::from_bytes(&bytes).map_err(|e| e.to_string())
                }) {
                    Ok(m) => {
                        let id = self.midis.len();
                        self.midis.push(m);
                        return Ok(Some(Value::Number(id as f64)));
                    },
                    Err(e) => {
                        eprintln!("music_midi_load failed ({path}): {e}");
                        return Ok(Some(Value::Number(-1.0)));
                    },
                }
            },
            "music_midi_count" | "MIDI数量" | "MIDI数" | "미디수" | "จำนวนมิดี" | "تعداد_MIDI" | "عدد_MIDI" | "מספר_MIDI" | "MIDI_تعداد" | "nombre_midi_musique" | "musik_midi_anzahl" | "число_midi_музыки" =>
            {
                let id = self.arg_num(args, 0, 0.0)? as i64;
                let n = self
                    .midis
                    .get(id as usize)
                    .map(|m| m.notes.len())
                    .unwrap_or(0);
                return Ok(Some(Value::Number(n as f64)));
            },
            "music_midi_notes" | "MIDI音符" | "MIDIノート" | "미디음표" | "โน้ตมิดี" | "نت‌های_MIDI" | "نغمات_MIDI" | "תווי_MIDI" | "MIDI_نوٹس" | "notes_midi_musique" | "musik_midi_noten" | "ноты_midi_музыки" =>
            {
                let id = self.arg_num(args, 0, 0.0)? as i64;
                let mut out = Vec::new();
                if let Some(m) = self.midis.get(id as usize) {
                    for n in &m.notes {
                        out.push(Value::Number(n.time as f64));
                        out.push(Value::Number(n.midi as f64));
                    }
                }
                return Ok(Some(Value::List(out.into())));
            },
            "music_midi_bars" | "MIDI音条" | "MIDIバー" | "미디바" | "แท่งมิดี" | "میله‌های_MIDI" | "أعمدة_MIDI" | "עמודות_MIDI" | "MIDI_بارز" | "mesures_midi_musique" | "musik_midi_takte" | "такты_midi_музыки" =>
            {
                let id = self.arg_num(args, 0, 0.0)? as i64;
                let mut out = Vec::new();
                if let Some(m) = self.midis.get(id as usize) {
                    for n in &m.notes {
                        out.push(Value::Number(n.time as f64));
                        out.push(Value::Number(n.midi as f64));
                        out.push(Value::Number(n.dur as f64));
                    }
                }
                return Ok(Some(Value::List(out.into())));
            },
            "music_judge" | "判定" | "判定する" | "판정" | "ตัดสินจังหวะ" | "داوری_ضرب" | "حكم_الإيقاع" | "שיפוט_קצב" | "بیٹ_فیصلہ" | "juger_musique" | "musik_bewerten" | "оценить_музыку" =>
            {
                let delta_ms = self.arg_num(args, 0, 9999.0)? as f32;
                return Ok(Some(Value::Number(
                    ling_music::Grade::judge(delta_ms).index() as f64,
                )));
            },
            "music_grade_name" | "判定名" | "判定名称" | "판정이름" | "ชื่อการตัดสิน" | "نام_رتبه" | "اسم_التقييم" | "שם_דירוג" | "گریڈ_نام" | "nom_grade_musique" | "musik_bewertungsname" | "имя_оценки_музыки" =>
            {
                let idx = self.arg_num(args, 0, 4.0)? as i32;
                return Ok(Some(Value::Str(
                    ling_music::Grade::from_index(idx).name().to_string(),
                )));
            },
            "music_note_name" | "音名" | "音名称" | "음이름" | "ชื่อโน้ต" | "نام_نت" | "اسم_النغمة" | "שם_תו" | "نوٹ_نام" | "nom_note_musique" | "musik_notenname" | "имя_ноты_музыки" =>
            {
                let hz = self.arg_num(args, 0, 0.0)? as f32;
                return Ok(Some(Value::Str(ling_music::note::hz_to_name(hz))));
            },
            "music_hz" | "音符频率" | "音符周波数" | "음표주파수" | "ความถี่โน้ต" | "فرکانس_نت" | "تردد_النغمة" | "תדר_תו" | "نوٹ_ہرٹز" | "hz_musique" | "musik_hz" | "музыка_гц" =>
            {
                let midi = match args.get(0) {
                    Some(Value::Str(s)) => ling_music::note::parse_pitch(s).unwrap_or(69),
                    Some(Value::Number(n)) => *n as i32,
                    _ => 69,
                };
                return Ok(Some(Value::Number(
                    ling_music::note::midi_to_hz(midi as f32) as f64,
                )));
            },
            "music_pitch_score" | "音准评分" | "音程スコア" | "음정점수" | "คะแนนเสียง" | "امتیاز_زیروبمی" | "درجة_طبقة_الصوت" | "ציון_גובה_צליל" | "پچ_اسکور" | "score_hauteur_musique" | "musik_tonhöhen_punktzahl" | "счёт_высоты_тона" =>
            {
                let hz = self.arg_num(args, 0, 0.0)? as f32;
                let target = self.arg_num(args, 1, 0.0)? as f32;
                return Ok(Some(Value::Number(
                    ling_music::karaoke::pitch_score(hz, target) as f64,
                )));
            },

            // ── Playback ──────────────────────────────────────────────────────
            "music_play" | "播放音乐" | "音楽再生" | "음악재생" | "เล่นเพลง" | "پخش_موسیقی" | "شغّل_الموسيقى" | "נגן_מוזיקה" | "موسیقی_چلاؤ" | "jouer_musique" | "musik_abspielen" | "играть_музыку" =>
            {
                let id = self.arg_num(args, 0, 0.0)? as usize;
                if let Some(t) = self.tracks.get(id) {
                    crate::gfx::audio_web::play_music(id, &t.stereo, t.channels, t.rate, 1.0);
                }
                return Ok(Some(Value::Unit));
            },
            "music_pause"
            | "暂停音乐"
            | "音楽一時停止"
            | "음악일시정지"
            | "หยุดเพลงชั่วคราว"
            | "music_stop"
            | "停止音乐"
            | "音楽停止"
            | "음악정지"
            | "หยุดเพลง" | "مکث_موسیقی" | "ألبث_الموسيقى" | "השהה_מוזיקה" | "موسیقی_روکو_مؤقت" | "pause_musique" | "musik_pausieren" | "пауза_музыки" => {
                let id = self.arg_num(args, 0, 0.0)? as usize;
                crate::gfx::audio_web::stop_music(id);
                return Ok(Some(Value::Unit));
            },
            "music_seek" | "定位音乐" | "音楽シーク" | "음악탐색" | "ค้นหาเพลง" | "جستجوی_موسیقی" | "ابحث_في_الموسيقى" | "חפש_במוזיקה" | "موسیقی_تلاش" | "chercher_musique" | "musik_suchen" | "перемотать_музыку" =>
            {
                // Seek is not straightforward on AudioBufferSourceNode; no-op for now.
                return Ok(Some(Value::Unit));
            },
            "music_pos" | "音乐位置" | "音楽位置" | "음악위치" | "ตำแหน่งเพลง" | "موقعیت_موسیقی" | "موضع_الموسيقى" | "מיקום_מוזיקה" | "موسیقی_مقام" | "position_musique" | "musik_position" | "позиция_музыки" =>
            {
                return Ok(Some(Value::Number(
                    crate::gfx::audio_web::current_music_position(),
                )));
            },
            "music_volume" | "音乐音量" | "音楽音量" | "음악음량" | "ระดับเพลง" | "بلندی_موسیقی" | "مستوى_الموسيقى" | "עוצמת_מוזיקה" | "موسیقی_شدت" | "volume_musique" | "musik_lautstärke" | "громкость_музыки" =>
            {
                let vol = self.arg_num(args, 0, 0.8)? as f32;
                // Apply to the most-recently started slot (slot 0 is typical).
                crate::gfx::audio_web::set_music_volume(0, vol);
                return Ok(Some(Value::Unit));
            },

            // ── FFT bands at current playback position ─────────────────────
            "music_fft" | "音乐频谱" | "音楽スペクトル" | "음악스펙트럼" | "สเปกตรัมเพลง" | "طیف_موسیقی" | "طيف_الموسيقى" | "ספקטרום_מוזיקה" | "میوزک_اسپیکٹرم" | "fft_musique" | "musik_fft" | "fft_музыки" =>
            {
                let id = self.arg_num(args, 0, 0.0)? as usize;
                let nbands = self.arg_num(args, 1, 16.0)? as usize;
                let pos = crate::gfx::audio_web::current_music_position() as f32;
                let bands = if let Some(t) = self.tracks.get(id) {
                    ling_music::analysis::fft_bands_at_pos(&t.mono, t.rate, pos, nbands)
                } else {
                    vec![0.0f32; nbands]
                };
                return Ok(Some(Value::List(
                    bands.into_iter().map(|x| Value::Number(x as f64)).collect::<Vec<_>>().into(),
                )));
            },

            _ => {},
        }
        Ok(None)
    }

    /// Lay out `text` for font `id` at size `px`, returning every glyph contour as
    /// a screen-space polyline (x→right, y→down). `(x, y)` is the text box top-left;
    /// the baseline is placed `ascent*px` below it. Curves are flattened to 0.3 px.
    #[cfg(not(target_arch = "wasm32"))]
    fn font_layout_2d(
        &mut self,
        id: usize,
        x: f32,
        y: f32,
        px: f32,
        text: &str,
    ) -> Vec<Vec<[f32; 2]>> {
        let mut out = Vec::new();
        for g in self.font_layout_2d_glyphs(id, x, y, px, text) {
            out.extend(g);
        }
        out
    }

    /// Same as [`font_layout_2d`] but grouped per glyph (so a fill can apply the
    /// non-zero winding rule within each glyph, preserving interior holes).
    #[cfg(not(target_arch = "wasm32"))]
    fn font_layout_2d_glyphs(
        &mut self,
        id: usize,
        x: f32,
        y: f32,
        px: f32,
        text: &str,
    ) -> Vec<Vec<Vec<[f32; 2]>>> {
        let font = &mut self.fonts[id];
        let asc = font.ascent();
        let tol = 0.3 / px;
        let mut pen = 0.0f32;
        let mut glyphs = Vec::new();
        for ch in text.chars() {
            let go = font.glyph_outline(ch, tol);
            let mut contours = Vec::with_capacity(go.polylines.len());
            for pl in &go.polylines {
                let mapped: Vec<[f32; 2]> = pl
                    .iter()
                    .map(|p| [x + (pen + p[0]) * px, y + (asc - p[1]) * px])
                    .collect();
                contours.push(mapped);
            }
            glyphs.push(contours);
            pen += go.advance;
        }
        glyphs
    }

    /// Register every item (functions, structs, globals) and evaluate the
    /// non-`do` globals into `global_seed`, WITHOUT running the entry. Used to
    /// prime the JIT's fallback interpreter so cranelift-skipped (oversized)
    /// functions can still be interpreted with full access to globals + peers.
    pub fn register_program(&mut self, program: &Program) -> Result<(), String> {
        for item in &program.items {
            self.register_item("", item)?;
        }
        let mut env = new_env();
        let non_do: Vec<_> = self
            .globals
            .iter()
            .filter(|(_, e)| !matches!(e, Expr::Do(_)))
            .map(|(k, e)| (k.clone(), e.clone()))
            .collect();
        let mut pending: Vec<(String, Expr)> = Vec::new();
        for (k, expr) in &non_do {
            let mut tmp = new_env();
            if let Ok(v) = self.eval_expr(expr, &mut tmp) {
                env.insert(k.clone(), v);
            } else {
                pending.push((k.clone(), expr.clone()));
            }
        }
        for (k, expr) in &pending {
            let mut tmp = env.clone();
            if let Ok(v) = self.eval_expr(expr, &mut tmp) {
                env.insert(k.clone(), v);
            }
        }
        self.global_seed = env;
        Ok(())
    }

    pub fn run_program(&mut self, program: &Program) -> Result<(), String> {
        self.register_program(program)?;
        let entry = self
            .find_entry()
            .ok_or("no entry point — need `bind start = do {...}` or `ผูก เริ่ม = ทำ {...}`")?;
        let mut env = self.global_seed.clone();
        self.framed("start", |me| me.eval_expr(&entry, &mut env))
            .map(|_| ())
            .map_err(|e| match e {
                EvalErr::Runtime(s) => s,
                EvalErr::Return(_) => "unexpected top-level return".to_string(),
                EvalErr::Break => "unexpected break at top level".to_string(),
            })
    }

    fn register_item(&mut self, ns: &str, item: &Item) -> Result<(), String> {
        match item {
            Item::Bind(name, expr) => {
                let key = if ns.is_empty() {
                    name.clone()
                } else {
                    format!("{ns}::{name}")
                };
                self.globals.insert(key, expr.clone());
            },
            Item::Fn(def) => {
                let key = if ns.is_empty() {
                    def.name.clone()
                } else {
                    format!("{ns}::{}", def.name)
                };
                self.functions.insert(key, Rc::new(def.clone()));
            },
            Item::Mod(name, body) => {
                let child_ns = if ns.is_empty() {
                    name.clone()
                } else {
                    format!("{ns}::{name}")
                };
                for child in body {
                    self.register_item(&child_ns, child)?;
                }
            },
            Item::TypeAlias(_, _) => {},
            Item::Struct(name, fields) => {
                self.structs.insert(name.clone(), fields.clone());
                if !ns.is_empty() {
                    self.structs.insert(format!("{ns}::{name}"), fields.clone());
                }
            },
            Item::Enum(name, variants) => {
                for v in variants {
                    self.enum_variants
                        .insert(v.name.clone(), (name.clone(), v.arity));
                    self.enum_variants
                        .insert(format!("{name}::{}", v.name), (name.clone(), v.arity));
                    if !ns.is_empty() {
                        self.enum_variants
                            .insert(format!("{ns}::{name}::{}", v.name), (name.clone(), v.arity));
                    }
                }
            },
            Item::Use { path, alias } => {
                self.load_module(path, alias.as_deref(), ns)?;
            },
        }
        Ok(())
    }

    /// Resolve `path` relative to `source_dir`, load and parse it, then
    /// register all its definitions.  If `alias` is given, every name is
    /// prefixed with `<parent_ns>::<alias>`.  Circular imports are silently
    /// skipped.
    fn load_module(
        &mut self,
        path: &str,
        alias: Option<&str>,
        parent_ns: &str,
    ) -> Result<(), String> {
        // ── Wasm32: no filesystem — use the pre-registered module registry ──
        #[cfg(target_arch = "wasm32")]
        let (source, sub_dir) = {
            // Skip if already loaded (circular import guard)
            if self.loaded_files.contains(path) {
                return Ok(());
            }
            self.loaded_files.insert(path.to_string());

            let src = crate::runtime::get_wasm_module(path)
                .or_else(|| crate::runtime::get_wasm_module(&format!("{}.ling", path)))
                .ok_or_else(|| format!("use: cannot find module '{path}'"))?;
            (src, None::<std::path::PathBuf>)
        };

        // ── Native: resolve against filesystem ──
        #[cfg(not(target_arch = "wasm32"))]
        let (source, sub_dir) = {
            let base_dir = self
                .source_dir
                .clone()
                .unwrap_or_else(|| std::path::PathBuf::from("."));
            let raw = std::path::Path::new(path);
            let candidates: Vec<std::path::PathBuf> = vec![
                base_dir.join(format!("{}.ling", path)),
                base_dir.join(format!("{}.灵", path)),
                base_dir.join(format!("{}.령", path)),
                base_dir.join(format!("{}.霊", path)),
                base_dir.join(format!("{}.ลิง", path)),
                base_dir.join(raw),
                std::path::PathBuf::from(format!("{}.ling", path)),
                std::path::PathBuf::from(path),
            ];

            let resolved = candidates
                .into_iter()
                .find(|p| p.exists())
                .ok_or_else(|| format!("use: cannot find module '{path}'"))?;

            let canonical = resolved
                .canonicalize()
                .unwrap_or_else(|_| resolved.clone())
                .to_string_lossy()
                .to_string();

            // Skip if already loaded (circular import guard)
            if self.loaded_files.contains(&canonical) {
                return Ok(());
            }
            self.loaded_files.insert(canonical.clone());

            let src = std::fs::read_to_string(&resolved)
                .map_err(|e| format!("use: failed to read '{path}': {e}"))?;
            let dir = resolved.parent().map(|p| p.to_path_buf());
            (src, dir)
        };

        let program = crate::parser::parse(&source)
            .map_err(|e| format!("use: parse error in '{path}': {e}"))?;

        // Compute target namespace: parent_ns :: alias (or just alias, or just parent_ns)
        let target_ns = match (parent_ns.is_empty(), alias) {
            (_, Some(a)) if !parent_ns.is_empty() => format!("{parent_ns}::{a}"),
            (_, Some(a)) => a.to_string(),
            (false, None) => parent_ns.to_string(),
            (true, None) => String::new(),
        };

        // Save/restore source_dir for nested relative imports
        let prev_dir = self.source_dir.clone();
        self.source_dir = sub_dir;

        for item in &program.items {
            self.register_item(&target_ns, item)?;
        }

        self.source_dir = prev_dir;
        Ok(())
    }

    fn find_entry(&self) -> Option<Expr> {
        // Known entry-point names across supported human languages.
        for key in crate::entry::ENTRY_NAMES {
            if let Some(e) = self.globals.get(*key) {
                return Some(e.clone());
            }
        }
        self.globals
            .values()
            .find(|e| matches!(e, Expr::Do(_)))
            .cloned()
    }

    // ─── Expression evaluation ────────────────────────────────────────────────

    fn eval_expr(&mut self, expr: &Expr, env: &mut Env) -> EvalResult {
        match expr {
            Expr::Str(s) => Ok(Value::Str(s.clone())),
            Expr::Number(n) => Ok(Value::Number(*n)),
            Expr::Bool(b) => Ok(Value::Bool(*b)),
            Expr::Unit => Ok(Value::Unit),
            Expr::Array(elems) => {
                let vs: Vec<_> = elems
                    .iter()
                    .map(|e| self.eval_expr(e, env))
                    .collect::<Result<_, _>>()?;
                Ok(Value::List(Rc::new(vs)))
            },

            Expr::Ident(name) => self.lookup(name, env),

            Expr::Path(segs) => {
                if segs.len() == 1 {
                    return self.lookup(&segs[0], env);
                }
                Ok(Value::Str(segs.join("::")))
            },

            Expr::Ref(inner) => self.eval_expr(inner, env),
            Expr::Await(inner) => self.eval_expr(inner, env),

            Expr::Do(stmts) => {
                let mut local = env.clone();
                Ok(self.exec_block(stmts, &mut local)?.unwrap_or(Value::Unit))
            },

            Expr::BinOp(op, lhs, rhs) => {
                let l = self.eval_expr(lhs, env)?;
                let r = self.eval_expr(rhs, env)?;
                self.apply_binop(op, l, r)
            },

            Expr::If { cond, then, elseifs, else_body } => {
                let cond_val = self.eval_expr(cond, env)?;
                if self.is_truthy(&cond_val) {
                    return Ok(self.exec_block(then, env)?.unwrap_or(Value::Unit));
                }
                for (ei_cond, ei_body) in elseifs {
                    let ei_cond_val = self.eval_expr(ei_cond, env)?;
                    if self.is_truthy(&ei_cond_val) {
                        return Ok(self.exec_block(ei_body, env)?.unwrap_or(Value::Unit));
                    }
                }
                if let Some(eb) = else_body {
                    return Ok(self.exec_block(eb, env)?.unwrap_or(Value::Unit));
                }
                Ok(Value::Unit)
            },

            Expr::While { cond, body } => {
                // Run the body directly in the *outer* env so that
                // `bind counter = counter + 1` persists across iterations,
                // which is the expected behaviour in a scripting language.
                loop {
                    let cv = self.eval_expr(cond, env)?;
                    if !self.is_truthy(&cv) {
                        break;
                    }
                    match self.exec_block(body, env) {
                        Ok(_) => {},
                        Err(EvalErr::Break) => break,
                        Err(e) => return Err(e),
                    }
                }
                Ok(Value::Unit)
            },

            Expr::For { var, iter, body } => {
                let iter_val = self.eval_expr(iter, env)?;
                let items = self.value_to_iter(iter_val)?;
                for item in items {
                    let mut local = env.clone();
                    local.insert(var.clone(), item);
                    match self.exec_block(body, &mut local) {
                        Ok(_) => {},
                        Err(EvalErr::Break) => break,
                        Err(e) => return Err(e),
                    }
                }
                Ok(Value::Unit)
            },

            Expr::Match(subject, arms) => {
                let subj = self.eval_expr(subject, env)?;
                for arm in arms {
                    if let Some(bindings) = self.match_pattern(&arm.pattern, &subj) {
                        let mut local = env.clone();
                        local.extend(bindings);
                        return self.eval_expr(&arm.body, &mut local);
                    }
                }
                Ok(Value::Unit)
            },

            Expr::Range(lo, hi) => {
                let lo_v = self.eval_expr(lo, env)?;
                let hi_v = self.eval_expr(hi, env)?;
                let lo_n = self.to_number(&lo_v)? as i64;
                let hi_n = self.to_number(&hi_v)? as i64;
                Ok(Value::List(Rc::new(
                    (lo_n..hi_n).map(|i| Value::Number(i as f64)).collect(),
                )))
            },

            Expr::Index(base, idx) => {
                let b = self.eval_expr(base, env)?;
                let i = self.eval_expr(idx, env)?;
                let n = self.to_number(&i)? as usize;
                match b {
                    Value::List(v) => v
                        .get(n)
                        .cloned()
                        .ok_or_else(|| EvalErr::from(format!("index {n} out of bounds"))),
                    Value::Str(s) => s
                        .chars()
                        .nth(n)
                        .map(|c| Value::Str(c.to_string()))
                        .ok_or_else(|| EvalErr::from(format!("index {n} out of bounds"))),
                    other => Err(EvalErr::from(format!("cannot index {:?}", other))),
                }
            },

            Expr::Call(callee, args) => {
                let arg_vals: Vec<Value> = args
                    .iter()
                    .map(|a| self.eval_expr(a, env))
                    .collect::<Result<_, _>>()?;
                match callee.as_ref() {
                    Expr::Ident(name) => self.call_named(name, arg_vals, env),
                    Expr::Path(segs) => self.call_named(&segs.join("::"), arg_vals, env),
                    _ => {
                        let v = self.eval_expr(callee, env)?;
                        self.call_value(v, arg_vals)
                    },
                }
            },

            Expr::MethodCall { receiver, method, args } => {
                let recv = self.eval_expr(receiver, env)?;
                let arg_vals: Vec<Value> = args
                    .iter()
                    .map(|a| self.eval_expr(a, env))
                    .collect::<Result<_, _>>()?;
                self.call_method(recv, method, arg_vals)
            },

            Expr::Closure(params, body) => Ok(Value::Fn(
                params.clone(),
                vec![Stmt::Expr(*body.clone())],
                env.clone(),
            )),

            Expr::Asm(_) => Ok(Value::Unit),
        }
    }

    // ─── Block execution ─────────────────────────────────────────────────────

    fn exec_block(&mut self, stmts: &[Stmt], env: &mut Env) -> Result<Option<Value>, EvalErr> {
        let mut last: Option<Value> = None;
        for stmt in stmts {
            match stmt {
                Stmt::Bind(name, expr) => {
                    match self.try_inplace_list_update(name, expr, env)? {
                        Some(v) => env.insert(name.clone(), v),
                        None => {
                            let v = self.eval_expr(expr, env)?;
                            env.insert(name.clone(), v)
                        },
                    };
                    last = None;
                },
                Stmt::Return(expr) => {
                    let v = self.eval_expr(expr, env)?;
                    return Err(EvalErr::Return(v));
                },
                Stmt::Expr(expr) => {
                    last = Some(self.eval_expr(expr, env)?);
                },
            }
        }
        Ok(last)
    }

    /// Fast path for `bind v = list_push(v, x)` / `bind v = list_set(v, i, x)`:
    /// the binding aliases the same list being rebuilt, so the env copy keeps the
    /// `Rc` shared and `make_mut` copies the whole vector every call. Taking the
    /// value out of env first leaves the `Rc` unique (unless truly aliased
    /// elsewhere, where copy-on-write still applies), turning O(n) into O(1).
    /// Returns `None` to fall back to normal evaluation.
    fn try_inplace_list_update(
        &mut self,
        name: &str,
        expr: &Expr,
        env: &mut Env,
    ) -> Result<Option<Value>, EvalErr> {
        let Expr::Call(callee, args) = expr else { return Ok(None) };
        let Expr::Ident(fname) = callee.as_ref() else { return Ok(None) };
        let is_push = matches!(
            fname.as_str(),
            "list_push" | "เพิ่มรายการ" | "列表添加" | "リスト追加" | "목록추가"
        );
        let is_set = matches!(
            fname.as_str(),
            "list_set" | "ตั้งรายการ" | "设元素" | "要素設定" | "요소설정"
        );
        if !is_push && !is_set {
            return Ok(None);
        }
        // First arg must be the same variable we are binding, and the builtin
        // must not be shadowed by a user function.
        match args.first() {
            Some(Expr::Ident(a0)) if a0 == name => {},
            _ => return Ok(None),
        }
        if self.functions.contains_key(fname.as_str()) {
            return Ok(None);
        }
        if is_push {
            if args.len() != 2 {
                return Ok(None);
            }
            let val = self.eval_expr(&args[1], env)?;
            match env.remove(name) {
                Some(Value::List(mut v)) => {
                    Rc::make_mut(&mut v).push(val);
                    Ok(Some(Value::List(v)))
                },
                other => {
                    if let Some(o) = other {
                        env.insert(name.to_string(), o);
                    }
                    Ok(None)
                },
            }
        } else {
            if args.len() != 3 {
                return Ok(None);
            }
            let idx_v = self.eval_expr(&args[1], env)?;
            let idx = self.to_number(&idx_v).unwrap_or(0.0) as usize;
            let val = self.eval_expr(&args[2], env)?;
            match env.remove(name) {
                Some(Value::List(mut v)) => {
                    if idx < v.len() {
                        Rc::make_mut(&mut v)[idx] = val;
                    }
                    Ok(Some(Value::List(v)))
                },
                other => {
                    if let Some(o) = other {
                        env.insert(name.to_string(), o);
                    }
                    Ok(None)
                },
            }
        }
    }

    // ─── Dispatch helpers ─────────────────────────────────────────────────────

    fn lookup(&self, name: &str, env: &Env) -> EvalResult {
        if let Some(v) = env.get(name) {
            return Ok(v.clone());
        }
        // Globals are an immutable load-time snapshot shared by every call frame;
        // a function reads them here instead of receiving a per-call clone.
        if let Some(v) = self.global_seed.get(name) {
            return Ok(v.clone());
        }
        if self.functions.contains_key(name) {
            let def = &self.functions[name];
            return Ok(Value::Fn(def.params.clone(), def.body.clone(), new_env()));
        }
        // Bare nullary enum variant used as a value (e.g. `bind p = Origin`).
        if let Some((enum_name, 0)) = self.enum_variants.get(name).cloned() {
            let variant = name.rsplit("::").next().unwrap_or(name).to_string();
            return Ok(Value::Variant { enum_name, variant, payload: Vec::new() });
        }
        // Math constants usable as plain identifiers (e.g. `sin(pi)`)
        match name {
            "pi" | "π" | "พาย" | "圆周率" | "円周率" | "파이" | "پی" | "باي" | "פאי" | "پائی" | "пи" => {
                return Ok(Value::Number(std::f64::consts::PI))
            },
            "tau" | "τ" | "双周率" | "タウ" | "타우" | "ทาว" | "تاو" | "טאו" | "ٹاؤ" | "тау" => {
                return Ok(Value::Number(std::f64::consts::TAU))
            },
            _ => {},
        }
        Err(EvalErr::from(format!("undefined: '{name}'")))
    }

    /// Profiling wrapper around the real dispatch. Zero overhead unless
    /// `LING_PROFILE` is set (one thread-local bool check per call). When on,
    /// it tallies per-name call count + inclusive time and, on each frame
    /// boundary (`present`), prints a sorted top-down report every
    /// `LING_PROFILE_EVERY` frames (default 240). Both the JIT (`ling_builtin` →
    /// here) and the tree-walker route through this, so it sees every builtin —
    /// in JIT mode user fns are native, so it's a clean builtin/render/physics
    /// profile with no nesting double-count.
    pub(crate) fn call_named(&mut self, name: &str, args: Vec<Value>, env: &Env) -> EvalResult {
        if !ling_profile_enabled() {
            return self.call_named_inner(name, args, env);
        }
        let t0 = crate::runtime::now_secs();
        let r = self.call_named_inner(name, args, env);
        ling_profile_record(
            name,
            ((crate::runtime::now_secs() - t0) * 1_000_000_000.0) as u128,
        );
        r
    }

    fn call_named_inner(&mut self, name: &str, args: Vec<Value>, env: &Env) -> EvalResult {
        // A user-defined function shadows any builtin of the same name, matching
        // the JIT/AOT backends (which always resolve a defined function first).
        if let Some(def) = self.functions.get(name).cloned() {
            let mut call_env =
                FxHashMap::with_capacity_and_hasher(def.params.len(), Default::default());
            let _ = env; // call-site locals are intentionally NOT visible to fns
            for (param, arg) in def.params.iter().zip(args) {
                call_env.insert(param.clone(), arg);
            }
            return match self.framed(name, |me| me.exec_block(&def.body, &mut call_env)) {
                Ok(v) => Ok(v.unwrap_or(Value::Unit)),
                Err(EvalErr::Return(v)) => Ok(v),
                Err(e) => Err(e),
            };
        }

        #[cfg(target_arch = "wasm32")]
        if let Some(v) = self.wasm_music_builtin(name, &args)? {
            return Ok(v);
        }

        match name {
            // Module global read emitted by the MIR backend: resolve against the
            // evaluated global snapshot (functions see globals read-only).
            "__ling_global" => {
                if let Some(Value::Str(g)) = args.first() {
                    if let Some(v) = self.global_seed.get(g.as_str()) {
                        return Ok(v.clone());
                    }
                }
                return Ok(Value::Unit);
            },
            // ── Print ──
            "print" | "println" | "印" | "打印" | "印刷" | "พิมพ์" | "출력" | "вывести"
            | "imprimir" | "afficher" | "چاپ" | "اطبع" | "הדפס" | "چھاپو" | "drucken" | "печать" => {
                let s = args
                    .iter()
                    .map(|v| v.to_string())
                    .collect::<Vec<_>>()
                    .join("");
                println!("{s}");
                return Ok(Value::Unit);
            },
            // print_color(colorIdx, text...) — ANSI-coloured console line.
            //   colorIdx 0..7 → bright fg (90+idx): 1=red 2=green 3=yellow 4=blue 6=cyan 7=white.
            "print_color" | "พิมพ์สี" | "چاپ_رنگی" | "اطبع_بلون" | "הדפס_בצבע" | "رنگین_چھاپو" => {
                #[cfg(windows)]
                {
                    use std::sync::Once;
                    static VT: Once = Once::new();
                    VT.call_once(|| {
                        extern "system" {
                            fn GetStdHandle(n: u32) -> *mut std::ffi::c_void;
                            fn GetConsoleMode(h: *mut std::ffi::c_void, m: *mut u32) -> i32;
                            fn SetConsoleMode(h: *mut std::ffi::c_void, m: u32) -> i32;
                        }
                        unsafe {
                            let h = GetStdHandle(0xFFFF_FFF5u32); // STD_OUTPUT_HANDLE (-11)
                            let mut mode = 0u32;
                            if GetConsoleMode(h, &mut mode) != 0 {
                                SetConsoleMode(h, mode | 0x0004); // ENABLE_VIRTUAL_TERMINAL_PROCESSING
                            }
                        }
                    });
                }
                let col = self.arg_num(&args, 0, 7.0)? as i64;
                let s = args
                    .iter()
                    .skip(1)
                    .map(|v| v.to_string())
                    .collect::<Vec<_>>()
                    .join("");
                let code = 90 + col.clamp(0, 7);
                println!("\x1b[1;{code}m{s}\x1b[0m");
                return Ok(Value::Unit);
            },
            // ── Format ──
            "format"
            | "格式"
            | "フォーマット"
            | "서식"
            | "รูปแบบ"
            | "форматировать"
            | "formatear"
            | "formater" | "قالب‌بندی" | "نسّق" | "פרמט" | "فارمیٹ" | "formatieren" => {
                return Ok(Value::Str(self.builtin_format(&args)?));
            },
            // ── String join / concatenation ──
            "格式::拼接" | "format::join" => match args.first() {
                Some(Value::List(items)) => {
                    return Ok(Value::Str(items.iter().map(|v| v.to_string()).collect()));
                },
                _ => return Ok(Value::Str(self.builtin_format(&args)?)),
            },
            // ── Result constructors ──
            "ok" | "好" | "良し" | "좋아" | "โอเค" | "تایید" | "تمام" | "בסדר" | "ٹھیک" | "bon" | "gut" | "хорошо" => {
                let val = args.into_iter().next().unwrap_or(Value::Unit);
                return Ok(Value::Ok(Box::new(val)));
            },
            "bad" | "坏" | "err" | "悪い" | "나쁨" | "ผิด" | "بد" | "سيء" | "רע" | "برا" | "mauvais" | "schlecht" | "плохо" => {
                let val = args.into_iter().next().unwrap_or(Value::Unit);
                return Ok(Value::Err(Box::new(val)));
            },
            // ── Vec constructors ──
            "向量::从" | "Vec::from" => {
                if let Some(Value::List(v)) = args.first() {
                    return Ok(Value::List(v.clone()));
                }
                return Ok(Value::List(Rc::new(args)));
            },
            "向量::有容量" | "Vec::with_capacity" => {
                return Ok(Value::List(Rc::new(Vec::new())))
            },
            // ── Timer stubs ──
            "计时::获取当前小时" | "Timer::hour" => return Ok(Value::Number(14.0)),
            "计时::现在" | "Timer::now" => return Ok(Value::Number(1000.0)),
            // ── Sleep ──
            "sleep" | "หยุด" | "นอน" | "sleep_ms" | "睡眠" | "眠る" | "スリープ" | "잠자기"
            | "잠" | "流水::睡眠" | "Flow::sleep" | "خواب" | "نم" | "שינה" | "سو_جاؤ" | "dormir" | "schlafen" | "спать" => {
                if let Some(ms_val) = args.first() {
                    if let Ok(ms) = self.to_number(ms_val) {
                        #[cfg(target_arch = "wasm32")]
                        wasm_sleep_ms(ms.max(0.0) as i32);
                        #[cfg(not(target_arch = "wasm32"))]
                        std::thread::sleep(std::time::Duration::from_millis(ms as u64));
                    }
                }
                return Ok(Value::Unit);
            },
            // ── Flow::parallel stub ──
            "流水::并行" | "Flow::parallel" => {
                if let Some(Value::Fn(params, body, mut cap)) = args.first().cloned() {
                    let _ = params;
                    match self.exec_block(&body, &mut cap) {
                        Ok(Some(v)) => return Ok(v),
                        Ok(None) => return Ok(Value::Unit),
                        Err(EvalErr::Return(v)) => return Ok(v),
                        Err(e) => return Err(e),
                    }
                }
                return Ok(Value::Unit);
            },

            // ══════════════════════════════════════════════════════════════════
            // MATH BUILTINS  (all args and results are f64)
            // Thai aliases: ไซน์ โคไซน์ แทนเจนต์ รากที่สอง ค่าสัมบูรณ์
            //               ปัดลง ปัดขึ้น ปัดเศษ ตัดทศนิยม ต่ำสุด สูงสุด
            //               จำกัด ยกกำลัง ลอการิทึม พาย
            // ══════════════════════════════════════════════════════════════════

            // ── Trigonometry (input in radians) ──
            "sin" | "ไซน์" | "正弦" | "サイン" | "사인" | "سینوس" | "جا" | "סינוס" | "سائن" | "sinus" | "синус" => {
                return Ok(Value::Number(self.arg_num(&args, 0, 0.0)?.sin()));
            },
            "cos" | "โคไซน์" | "余弦" | "コサイン" | "코사인" | "کسینوس" | "جتا" | "קוסינוס" | "کوسائن" | "cosinus" | "kosinus" | "косинус" => {
                return Ok(Value::Number(self.arg_num(&args, 0, 0.0)?.cos()));
            },

            // ── Hyperbolic functions ──
            // Hyperbolic tangent
            "tanh" | "tanhf" | "双曲正切" | "双曲線正接" | "쌍곡탄젠트" => {
                return Ok(Value::Number(self.arg_num(&args, 0, 0.0)?.tanh()));
            },

            "tan" | "แทนเจนต์" | "正切" | "タンジェント" | "탄젠트" | "تانژانت" | "ظا" | "טנגנס" | "ٹینجنٹ" | "tangente" | "tangens" | "тангенс" => {
                return Ok(Value::Number(self.arg_num(&args, 0, 0.0)?.tan()));
            },
            "asin" | "arcsin" | "反正弦" | "アークサイン" | "아크사인" | "อาร์กไซน์" | "آرک‌سینوس" | "قوس_جا" | "ארקסינוס" | "آرک_سائن" | "arcsinus" | "arkussinus" | "арксинус" =>
            {
                return Ok(Value::Number(self.arg_num(&args, 0, 0.0)?.asin()));
            },
            "acos" | "arccos" | "反余弦" | "アークコサイン" | "아크코사인" | "อาร์กโคไซน์" | "آرک‌کسینوس" | "قوس_جتا" | "ארקוקוסינוס" | "آرک_کوسائن" | "arccosinus" | "arkuskosinus" | "арккосинус" =>
            {
                return Ok(Value::Number(self.arg_num(&args, 0, 0.0)?.acos()));
            },
            "atan" | "arctan" | "反正切" | "アークタンジェント" | "아크탄젠트" | "อาร์กแทนเจนต์" | "آرک‌تانژانت" | "قوس_ظا" | "ארקטנגנס" | "آرک_ٹینجنٹ" | "arctangente" | "arkustangens" | "арктангенс" =>
            {
                return Ok(Value::Number(self.arg_num(&args, 0, 0.0)?.atan()));
            },
            "atan2" | "arctan2" | "反正切2" | "アークタンジェント2" | "아크탄젠트2" =>
            {
                let y = self.arg_num(&args, 0, 0.0)?;
                let x = self.arg_num(&args, 1, 1.0)?;
                return Ok(Value::Number(y.atan2(x)));
            },

            // ── Roots / powers ──
            "sqrt" | "รากที่สอง" | "平方根" | "根" | "제곱근" | "جذر" | "שורש" | "racine_carrée" | "quadratwurzel" | "корень" => {
                return Ok(Value::Number(self.arg_num(&args, 0, 0.0)?.sqrt()));
            },
            "cbrt" | "立方根" | "세제곱근" | "รากที่สาม" | "ریشه_سوم" | "جذر_تكعيبي" | "שורש_שלישי" | "مکعب_جذر" | "racine_cubique" | "kubikwurzel" | "кубический_корень" => {
                return Ok(Value::Number(self.arg_num(&args, 0, 0.0)?.cbrt()));
            },
            "pow" | "ยกกำลัง" | "幂" | "べき乗" | "거듭제곱" | "توان" | "أس" | "חזקה" | "قوت" | "puissance" | "potenz" | "степень" => {
                let base = self.arg_num(&args, 0, 0.0)?;
                let exp = self.arg_num(&args, 1, 1.0)?;
                return Ok(Value::Number(base.powf(exp)));
            },
            "exp" | "指数" | "指数関数" | "지수" => {
                return Ok(Value::Number(self.arg_num(&args, 0, 0.0)?.exp()));
            },
            "hypot" | "斜边" | "斜辺" | "빗변" => {
                let x = self.arg_num(&args, 0, 0.0)?;
                let y = self.arg_num(&args, 1, 0.0)?;
                return Ok(Value::Number(x.hypot(y)));
            },

            // ── Logarithms ──
            "ln" | "log" | "ลอการิทึม" | "对数" | "対数" | "로그" | "لگاریتم_طبیعی" | "لوغاريتم_طبيعي" | "לוגריתם_טבעי" | "فطری_لوگ" => {
                return Ok(Value::Number(self.arg_num(&args, 0, 1.0)?.ln()));
            },
            "log2" | "对数2" | "対数2" | "로그2" => {
                return Ok(Value::Number(self.arg_num(&args, 0, 1.0)?.log2()));
            },
            "log10" | "对数10" | "対数10" | "로그10" => {
                return Ok(Value::Number(self.arg_num(&args, 0, 1.0)?.log10()));
            },

            // ── Rounding / truncation ──
            "abs" | "ค่าสัมบูรณ์" | "绝对值" | "绝对" | "絶対値" | "절댓값" | "절대값" | "قدرمطلق" | "مطلق" | "ערך_מוחלט" | "مطلق_قدر" | "valeur_absolue" | "betrag" | "модуль_числа" =>
            {
                return Ok(Value::Number(self.arg_num(&args, 0, 0.0)?.abs()));
            },
            "floor" | "ปัดลง" | "向下取整" | "下整" | "床関数" | "내림" | "کف" | "أرضية" | "רצפה" | "فرش" | "plancher" | "abrunden" | "вниз" => {
                return Ok(Value::Number(self.arg_num(&args, 0, 0.0)?.floor()));
            },
            "ceil" | "ปัดขึ้น" | "向上取整" | "上整" | "天井関数" | "올림" | "سقف" | "תקרה" | "چھت" | "plafond" | "aufrunden" | "вверх" =>
            {
                return Ok(Value::Number(self.arg_num(&args, 0, 0.0)?.ceil()));
            },
            "round" | "ปัดเศษ" | "四舍五入" | "四舍" | "四捨五入" | "반올림" | "گرد_کردن" | "تقريب" | "עיגול" | "گول" | "arrondir" | "runden" | "округлить" =>
            {
                return Ok(Value::Number(self.arg_num(&args, 0, 0.0)?.round()));
            },
            "trunc"
            | "int"
            | "ตัดทศนิยม"
            | "取整"
            | "整数化"
            | "整数"
            | "截整"
            | "정수화"
            | "정수"
            | "切り捨て"
            | "버림" | "برش" | "اقتطاع" | "קיטום" | "کٹائی" | "tronquer" | "abschneiden" | "усечь" => {
                return Ok(Value::Number(self.arg_num(&args, 0, 0.0)?.trunc()));
            },
            "fract" | "小数部分" | "小数部" | "소수부" => {
                return Ok(Value::Number(self.arg_num(&args, 0, 0.0)?.fract()));
            },

            // ── min / max / clamp ──
            "min" | "ต่ำสุด" | "最小" | "최솟값" | "کمینه" | "أصغر" | "מינימום" | "کم_ترین" | "minimum" | "минимум" => {
                let a = self.arg_num(&args, 0, 0.0)?;
                let b = self.arg_num(&args, 1, 0.0)?;
                return Ok(Value::Number(a.min(b)));
            },
            "max" | "สูงสุด" | "最大" | "최댓값" | "بیشینه" | "أكبر" | "מקסימום" | "زیادہ_ترین" | "maximum" | "максимум" => {
                let a = self.arg_num(&args, 0, 0.0)?;
                let b = self.arg_num(&args, 1, 0.0)?;
                return Ok(Value::Number(a.max(b)));
            },
            "clamp" | "จำกัด" | "截取" | "範囲制限" | "범위제한" | "محدود" | "قيّد" | "הגבל" | "limiter" | "begrenzen" | "ограничить" => {
                let x = self.arg_num(&args, 0, 0.0)?;
                let lo = self.arg_num(&args, 1, 0.0)?;
                let hi = self.arg_num(&args, 2, 1.0)?;
                return Ok(Value::Number(x.clamp(lo, hi)));
            },

            // ── Constants (also accessible as plain identifiers via lookup) ──
            "pi" | "π" | "พาย" | "圆周率" | "円周率" | "파이" | "پی" | "باي" | "פאי" | "پائی" | "пи" => {
                return Ok(Value::Number(std::f64::consts::PI))
            },
            "tau" | "τ" | "双周率" | "タウ" | "타우" | "ทาว" | "تاو" | "טאו" | "ٹاؤ" | "тау" => {
                return Ok(Value::Number(std::f64::consts::TAU))
            },

            // ══════════════════════════════════════════════════════════════════
            // PHASE 1: DMT TRIP CODER FEATURES
            // ══════════════════════════════════════════════════════════════════

            // ── Step 1: Noise Functions ──
            "vnoise" | "noise2" | "นอยส์2ดี" | "柏林噪声2D" | "バリューノイズ2D" | "값노이즈2D" | "نویز_برداری" | "ضجيج_متجه" | "רעש_וקטורי" | "ویکٹر_نوائز" | "bruit_v" | "v_rauschen" | "шум_v" =>
            {
                let x = self.arg_num(&args, 0, 0.0)? as f32;
                let y = self.arg_num(&args, 1, 0.0)? as f32;
                let seed = self.arg_num(&args, 2, 0.0)? as u32;
                return Ok(Value::Number(tex_vnoise(x, y, seed) as f64));
            },

            "fbm" | "นอยส์ออร์แกนิก" | "分形噪声" | "フラクタルノイズ" | "프랙탈노이즈" | "نویز_فراکتالی" | "ضجيج_عضوي" | "רעש_פרקטלי" | "فریکٹل_نوائز" =>
            {
                let x = self.arg_num(&args, 0, 0.0)? as f32;
                let y = self.arg_num(&args, 1, 0.0)? as f32;
                let octaves = self.arg_num(&args, 2, 4.0)? as u32;
                let seed = self.arg_num(&args, 3, 0.0)? as u32;
                return Ok(Value::Number(tex_fbm(x, y, octaves, seed) as f64));
            },

            "perlin"
            | "perlin3"
            | "เพอร์ลิน3ดี"
            | "柏林噪声3D"
            | "パーリンノイズ3D"
            | "펄린노이즈3D" | "نویز_پرلین" | "بيرلين" | "רעש_פרלין" | "پرلن_نوائز" | "перлин" => {
                let x = self.arg_num(&args, 0, 0.0)? as f32;
                let y = self.arg_num(&args, 1, 0.0)? as f32;
                let z = self.arg_num(&args, 2, 0.0)? as f32;
                return Ok(Value::Number(perlin3(x, y, z) as f64));
            },

            // ── Step 2: Math Ergonomics ──
            "lerp" | "ค่าระหว่าง" | "线性插值" | "線形補間" | "선형보간" | "میان‌یابی" | "استيفاء" | "אינטרפולציה" | "درمیانی_قدر" | "interpoler" | "interpolieren" | "интерполировать" =>
            {
                let a = self.arg_num(&args, 0, 0.0)?;
                let b = self.arg_num(&args, 1, 1.0)?;
                let t = self.arg_num(&args, 2, 0.0)?;
                return Ok(Value::Number(a + (b - a) * t));
            },

            "smoothstep" | "เปลี่ยนแบบนุ่ม" | "平滑步进" | "スムーズステップ" | "스무스스텝" | "گام_نرم" | "تدرج_ناعم" | "מדרגה_חלקה" | "ہموار_قدم" | "lissage" | "glättung" | "сглаживание" =>
            {
                let lo = self.arg_num(&args, 0, 0.0)?;
                let hi = self.arg_num(&args, 1, 1.0)?;
                let x = self.arg_num(&args, 2, 0.5)?;
                let t = ((x - lo) / (hi - lo)).clamp(0.0, 1.0);
                return Ok(Value::Number(t * t * (3.0 - 2.0 * t)));
            },

            "rand" | "สุ่ม" | "随机" | "乱数" | "난수" | "تصادفی" | "عشوائي" | "אקראי" | "بے_ترتیب" | "aléatoire" | "zufall" | "случайное" => {
                let val = fast_rand_f64(&mut self.rand_state);
                return Ok(Value::Number(val));
            },

            "sign" | "เครื่องหมาย" | "符号" | "符号関数" | "부호" | "علامت" | "إشارة" | "סימן" | "نشان" | "signe" | "vorzeichen" | "знак" => {
                let x = self.arg_num(&args, 0, 0.0)?;
                return Ok(Value::Number(x.signum()));
            },

            "hsv_to_rgb" | "เอชเอสวีเป็นRGB" | "HSV转RGB" | "HSV変換RGB" | "HSV변환RGB" | "HSV_به_RGB" | "HSV_إلى_RGB" | "HSV_ל_RGB" | "HSV_سے_RGB" | "hsv_vers_rgb" | "hsv_zu_rgb" | "hsv_в_rgb" =>
            {
                let h = self.arg_num(&args, 0, 0.0)?; // 0-360
                let s = self.arg_num(&args, 1, 1.0)?; // 0-1
                let v = self.arg_num(&args, 2, 1.0)?; // 0-1
                let c = v * s;
                let x = c * (1.0 - (((h / 60.0) % 2.0) - 1.0).abs());
                let m = v - c;
                let (r1, g1, b1) = if h < 60.0 {
                    (c, x, 0.0)
                } else if h < 120.0 {
                    (x, c, 0.0)
                } else if h < 180.0 {
                    (0.0, c, x)
                } else if h < 240.0 {
                    (0.0, x, c)
                } else if h < 300.0 {
                    (x, 0.0, c)
                } else {
                    (c, 0.0, x)
                };
                let r = ((r1 + m) * 255.0).round();
                let g = ((g1 + m) * 255.0).round();
                let b = ((b1 + m) * 255.0).round();
                return Ok(Value::List(Rc::new(vec![
                    Value::Number(r),
                    Value::Number(g),
                    Value::Number(b),
                ])));
            },

            "lerp_color" | "ไล่สี" | "颜色插值" | "色補間" | "색보간" | "میان‌یابی_رنگ" | "استيفاء_اللون" | "אינטרפולציית_צבע" | "رنگ_درمیانی_قدر" | "interpoler_couleur" | "farbe_interpolieren" | "интерполировать_цвет" => {
                let r1 = self.arg_num(&args, 0, 0.0)?;
                let g1 = self.arg_num(&args, 1, 0.0)?;
                let b1 = self.arg_num(&args, 2, 0.0)?;
                let r2 = self.arg_num(&args, 3, 255.0)?;
                let g2 = self.arg_num(&args, 4, 255.0)?;
                let b2 = self.arg_num(&args, 5, 255.0)?;
                let t = self.arg_num(&args, 6, 0.0)?;
                let r = r1 + (r2 - r1) * t;
                let g = g1 + (g2 - g1) * t;
                let b = b1 + (b2 - b1) * t;
                let c = ((r as u32) << 16) | ((g as u32) << 8) | (b as u32);
                self.gfx.borrow_mut().color = c;
                return Ok(Value::Unit);
            },

            // ── Step 3: Real-Time Clock ──
            "time_now" | "เวลาปัจจุบัน" | "当前时间" | "経過時間" | "현재시간" | "زمان_اکنون" | "الوقت_الآن" | "הזמן_עכשיו" | "ابھی_کا_وقت" | "temps_actuel" | "aktuelle_zeit" | "текущее_время" =>
            {
                return Ok(Value::Number(
                    crate::runtime::now_secs() - self.start_time_secs,
                ));
            },

            // Wall-clock seconds since the Unix epoch (real date/time). Lets a
            // program defer deterministic-yet-evolving generation to the actual
            // datetime — same clock → same world, advancing as real time passes.
            "epoch_now" | "เวลาโลก" | "datetime" | "现在时刻" | "現在時刻" | "현재시각" | "مهر_زمانی_اکنون" | "طابع_الوقت_الآن" | "חותמת_זמן_עכשיו" | "ایپاک_وقت" =>
            {
                return Ok(Value::Number(crate::runtime::now_secs()));
            },

            "frame_count" | "เฟรม" | "帧数" | "フレーム数" | "프레임수" | "شمار_فریم" | "عدد_الإطارات" | "ספירת_פריימים" | "فریم_شمار" | "nombre_images" | "bildanzahl" | "число_кадров" => {
                return Ok(Value::Number(self.frame_num as f64));
            },

            // ── Step 4: Microphone Input ──
            "mic_open" | "เปิดไมค์" | "开麦克风" | "マイク開く" | "마이크열기" | "باز_کردن_میکروفون" | "افتح_الميكروفون" | "פתח_מיקרופון" | "مائیکروفون_کھولو" | "ouvrir_micro" | "mikrofon_öffnen" | "открыть_микрофон" =>
            {
                #[cfg(not(target_arch = "wasm32"))]
                {
                    match ling_mic::MicInput::open(Default::default()) {
                        Ok(mic) => {
                            let _ = mic.start(|_samples: &[f32]| {}); // No-op callback
                            self.mic = Some(mic);
                            return Ok(Value::Number(1.0)); // opened
                        },
                        // No device / permission denied → graceful: don't crash the game loop.
                        // Returns 0.0; mic_rms/mic_peak return 0.0 while self.mic is None.
                        Err(_e) => {
                            self.mic = None;
                            return Ok(Value::Number(0.0));
                        },
                    }
                }
                #[cfg(target_arch = "wasm32")]
                return Ok(Value::Unit);
            },

            "mic_rms" | "เสียงRMS" | "麦克风音量" | "マイクRMS" | "마이크RMS" | "RMS_میکروفون" | "RMS_الميكروفون" | "RMS_מיקרופון" | "مائیکروفون_RMS" | "rms_micro" | "mikrofon_rms" | "rms_микрофона" =>
            {
                #[cfg(not(target_arch = "wasm32"))]
                {
                    let rms = self
                        .mic
                        .as_ref()
                        .map(|m: &ling_mic::MicInput| m.rms())
                        .unwrap_or(0.0);
                    return Ok(Value::Number(rms as f64));
                }
                #[cfg(target_arch = "wasm32")]
                return Ok(Value::Number(0.0));
            },

            "mic_peak" | "เสียงพีค" | "麦克风峰值" | "マイクピーク" | "마이크피크" | "اوج_میکروفون" | "ذروة_الميكروفون" | "שיא_מיקרופון" | "مائیکروفون_چوٹی" | "crête_micro" | "mikrofon_spitze" | "пик_микрофона" =>
            {
                #[cfg(not(target_arch = "wasm32"))]
                {
                    let peak = self
                        .mic
                        .as_ref()
                        .map(|m: &ling_mic::MicInput| m.peak())
                        .unwrap_or(0.0);
                    return Ok(Value::Number(peak as f64));
                }
                #[cfg(target_arch = "wasm32")]
                return Ok(Value::Number(0.0));
            },

            "mic_fft" | "วิเคราะห์เสียงสด" | "实时频谱" | "リアルタイムFFT" | "실시간FFT" | "FFT_میکروفون" | "FFT_الميكروفون" | "FFT_מיקרופון" | "مائیکروفون_FFT" | "fft_micro" | "mikrofon_fft" | "fft_микрофона" =>
            {
                #[cfg(not(target_arch = "wasm32"))]
                {
                    let n = self.arg_num(&args, 0, 8.0)? as usize;
                    if let Some(mic) = self.mic.as_ref() {
                        let samples = mic.latest_samples();
                        self.fft.borrow_mut().push_samples(&samples);
                    }
                    let bands = self.fft.borrow().freq_bands(n);
                    let result: Vec<Value> =
                        bands.iter().map(|&v| Value::Number(v as f64)).collect();
                    return Ok(Value::List(Rc::new(result)));
                }
                #[cfg(target_arch = "wasm32")]
                return Ok(Value::List(Vec::new().into()));
            },

            // ── Step 5: Additive Blend Mode ──
            "set_blend" | "โหมดผสม" | "混合模式" | "ブレンドモード" | "블렌드모드" | "تنظیم_ترکیب" | "عيّن_المزج" | "קבע_מיזוג" | "بلینڈ_مقرر_کرو" | "définir_mélange" | "mischmodus_setzen" | "задать_смешивание" =>
            {
                let mode = self.arg_num(&args, 0, 0.0)? as u8;
                let mut gfx = self.gfx.borrow_mut();
                gfx.blend = mode;
                let a = gfx.alpha;
                gfx.depth_queue.set_state(mode, a); // 3-D queue captures blend for subsequent pushes
                return Ok(Value::Unit);
            },

            // set_antialias(on) — smooth wireframe strokes (lines / edges / arcs /
            // circle outlines) via Xiaolin-Wu coverage. Default OFF = crisp,
            // opaque, aliased pixels; pass 1 to opt into smooth edges.
            "set_antialias" | "ตั้งลบรอยหยัก" | "抗锯齿" | "アンチエイリアス" | "안티에일리어싱" | "تنظیم_ضدلبه‌دندانه" | "عيّن_مضاد_التسنن" | "קבע_החלקת_קצוות" | "اینٹی_الائیسنگ_مقرر_کرو" =>
            {
                let on = self.arg_num(&args, 0, 1.0)? > 0.5;
                self.gfx.borrow_mut().antialias = on;
                return Ok(Value::Unit);
            },
            // get_antialias() -> bool — current wireframe anti-aliasing state.
            "get_antialias"
            | "อ่านลบรอยหยัก"
            | "读取抗锯齿"
            | "アンチエイリアス取得"
            | "안티에일리어싱상태" | "خواندن_ضدلبه‌دندانه" | "اقرأ_مضاد_التسنن" | "קרא_החלקת_קצוות" | "اینٹی_الائیسنگ_پڑھو" => {
                return Ok(Value::Bool(self.gfx.borrow().antialias));
            },

            // set_font_antialias(on) — smooth `font_text`/`font_text_fill` glyph
            // edges, independent of `set_antialias` (which only covers wireframe
            // strokes). Default OFF = crisp, hard-edged text; pass 1 to opt in.
            "set_font_antialias" | "글꼴안티에일리어싱" => {
                let on = self.arg_num(&args, 0, 1.0)? > 0.5;
                self.gfx.borrow_mut().font_antialias = on;
                return Ok(Value::Unit);
            },
            // get_font_antialias() -> bool — current font anti-aliasing state.
            "get_font_antialias" | "글꼴안티에일리어싱상태" => {
                return Ok(Value::Bool(self.gfx.borrow().font_antialias));
            },

            // ── Step 6: Circle Primitives ──
            "draw_circle" | "วาดวงกลม" | "画圆" | "円描画" | "원그리기" | "رسم_دایره" | "ارسم_دائرة" | "צייר_עיגול" | "دائرہ_کھینچو" | "dessiner_cercle" | "kreis_zeichnen" | "рисовать_круг" =>
            {
                let cx = self.arg_num(&args, 0, 0.0)? as i32;
                let cy = self.arg_num(&args, 1, 0.0)? as i32;
                let r = self.arg_num(&args, 2, 10.0)? as i32;
                let mut gfx = self.gfx.borrow_mut();
                let (w, h, color, blend) =
                    (gfx.width as i32, gfx.height as i32, gfx.color, gfx.blend);
                if gfx.antialias {
                    let (uw, uh) = (gfx.width, gfx.height);
                    let segs = ((r.max(1) as u32) * 4).clamp(24, 512);
                    crate::gfx::raster::draw_arc(
                        &mut gfx.buffer,
                        uw,
                        uh,
                        color,
                        true,
                        blend == 1,
                        cx as f32,
                        cy as f32,
                        r as f32,
                        0.0,
                        std::f32::consts::TAU,
                        segs,
                    );
                } else {
                    draw_circle_outline(&mut gfx.buffer, w, h, cx, cy, r, color, blend);
                }
                return Ok(Value::Unit);
            },

            "draw_filled_circle"
            | "draw_disc"
            | "วาดวงกลมทึบ"
            | "画实心圆"
            | "塗りつぶし円"
            | "원채우기" | "رسم_دایره_توپر" | "ارسم_دائرة_ممتلئة" | "צייר_עיגול_מלא" | "بھرا_دائرہ_کھینچو" => {
                let cx = self.arg_num(&args, 0, 0.0)? as i32;
                let cy = self.arg_num(&args, 1, 0.0)? as i32;
                let r = self.arg_num(&args, 2, 10.0)? as i32;
                let mut gfx = self.gfx.borrow_mut();
                let (w, h, color, blend) =
                    (gfx.width as i32, gfx.height as i32, gfx.color, gfx.blend);
                draw_circle_filled(&mut gfx.buffer, w, h, cx, cy, r, color, blend);
                return Ok(Value::Unit);
            },

            // draw_arc(cx, cy, r, a0, a1 [, segments]) — stroke a circular arc in
            // the pen colour (full circle when a1-a0 = TAU). Honors the antialias
            // flag; opaque by default (additive when blend = 1).
            "draw_arc" | "arc" | "วาดส่วนโค้ง" | "画弧" | "円弧描画" | "호그리기" | "رسم_کمان" | "ارسم_قوسا" | "צייר_קשת" | "آرک_کھینچو" =>
            {
                let cx = self.arg_num(&args, 0, 0.0)? as f32;
                let cy = self.arg_num(&args, 1, 0.0)? as f32;
                let r = self.arg_num(&args, 2, 10.0)? as f32;
                let a0 = self.arg_num(&args, 3, 0.0)? as f32;
                let a1 = self.arg_num(&args, 4, std::f64::consts::TAU)? as f32;
                let default_segs = ((r.abs() * (a1 - a0).abs()).ceil() as u32).clamp(8, 1024);
                let segs = self.arg_num(&args, 5, default_segs as f64)? as u32;
                let mut gfx = self.gfx.borrow_mut();
                let color = gfx.color;
                #[cfg(not(target_arch = "wasm32"))]
                {
                    let (uw, uh, aa, add) = (gfx.width, gfx.height, gfx.antialias, gfx.blend == 1);
                    crate::gfx::raster::draw_arc(
                        &mut gfx.buffer,
                        uw,
                        uh,
                        color,
                        aa,
                        add,
                        cx,
                        cy,
                        r,
                        a0,
                        a1,
                        segs,
                    );
                }
                #[cfg(target_arch = "wasm32")]
                {
                    let segs_f = segs.max(1);
                    let step = (a1 - a0) / segs_f as f32;
                    let mut px = cx + r * a0.cos();
                    let mut py = cy + r * a0.sin();
                    let mut i = 1u32;
                    while i <= segs_f {
                        let a = a0 + step * i as f32;
                        let nx = cx + r * a.cos();
                        let ny = cy + r * a.sin();
                        gfx.depth_queue.push_line(0.0, color, px, py, nx, ny);
                        px = nx;
                        py = ny;
                        i += 1;
                    }
                }
                return Ok(Value::Unit);
            },

            // ── Step 7: Transparent fills, gradient surfaces & colored shadows ──
            // These all write straight into the software framebuffer (gfx.buffer)
            // on both native and web, so no target gating is needed.

            // set_alpha(a) — pen opacity 0..1 for the alpha-blended fills below.
            "set_alpha" | "ตั้งความโปร่งใส" | "设透明" | "アルファ設定" | "투명도설정" | "تنظیم_شفافیت" | "عيّن_الشفافية" | "קבע_שקיפות" | "شفافیت_مقرر_کرو" | "définir_alpha" | "alpha_setzen" | "задать_альфа" =>
            {
                let a = self.arg_num(&args, 0, 1.0)? as f32;
                let mut gfx = self.gfx.borrow_mut();
                gfx.alpha = a.clamp(0.0, 1.0);
                let (m, al) = (gfx.blend, gfx.alpha);
                gfx.depth_queue.set_state(m, al); // 3-D queue captures alpha for subsequent pushes
                return Ok(Value::Unit);
            },

            // mesh_hue(radians) — hue-rotate the baked per-tri colours of every
            // subsequent mesh_draw (.lmesh). 0 resets. Cheap: one matrix per call.
            "mesh_hue" | "หมุนสีเมช" | "فام_مش" | "صبغة_الشبكة" | "גוון_רשת" | "میش_ہیو" =>
            {
                let h = self.arg_num(&args, 0, 0.0)? as f32;
                let g = self.arg_num(&args, 1, 1.0)? as f32;
                let mut gfx = self.gfx.borrow_mut();
                gfx.mesh_hue = h;
                gfx.mesh_hue_gain = g.max(0.0);
                return Ok(Value::Unit);
            },

            // set_frame_blur(amount 0..0.95) — afterimage trails: blend the previous
            // presented frame into each new one. 0 = off (also frees the ghost buffer).
            "set_frame_blur" | "frame_blur" | "เบลอเฟรม" | "تنظیم_تاری_فریم" | "عيّن_ضبابية_الإطار" | "קבע_טשטוש_פריים" | "فریم_بلر_مقرر_کرو" =>
            {
                let a = self.arg_num(&args, 0, 0.0)? as f32;
                let mut gfx = self.gfx.borrow_mut();
                gfx.frame_blur = a.clamp(0.0, 0.95);
                if gfx.frame_blur <= 0.0 {
                    gfx.prev_frame = Vec::new();
                }
                return Ok(Value::Unit);
            },

            // set_line_hue_cycle(rate) — rapidly cycle the hue of ALL wireframe line
            // strokes (draw_line / draw_line_3d). `rate` in radians/sec; 0 = off.
            // Process-global so a single call covers every stroke, every frame.
            "set_line_hue_cycle" | "ตั้งวนสีเส้น" | "تنظیم_چرخه_فام_خط" | "عيّن_دورة_صبغة_الخط" | "קבע_מחזור_גוון_קו" | "لائن_ہیو_سائیکل_مقرر_کرو" => {
                let rate = self.arg_num(&args, 0, 0.0)?;
                crate::runtime::set_line_hue_rate(rate);
                return Ok(Value::Unit);
            },

            // set_color_space(mode) — 0 = legacy sRGB compositing (default),
            // 1 = gamma-correct linear-light compositing (blend in linear, store
            // sRGB) so alpha and gradients don't darken/shift hue.
            "set_color_space" | "ปริภูมิสี" | "色彩空间" | "色空間" | "색공간" | "تنظیم_فضای_رنگ" | "عيّن_فضاء_اللون" | "קבע_מרחב_צבע" | "کلر_اسپیس_مقرر_کرو" | "définir_espace_couleur" | "farbraum_setzen" | "задать_цветовое_пространство" =>
            {
                let m = self.arg_num(&args, 0, 0.0)? as i64;
                self.gfx.borrow_mut().linear_blend = m != 0;
                return Ok(Value::Unit);
            },

            // set_gradient_space(mode) — 1 = perceptual OkLab gradient interp
            // (default), 0 = legacy sRGB. Affects grad_triangle / grad_rect.
            "set_gradient_space" | "ปริภูมิไล่สี" | "渐变空间" | "グラデ空間" | "그라데이션공간" | "تنظیم_فضای_گرادیان" | "عيّن_فضاء_التدرج" | "קבע_מרחב_גרדיאנט" | "گریڈینٹ_اسپیس_مقرر_کرو" | "définir_espace_dégradé" | "verlaufsraum_setzen" | "задать_пространство_градиента" =>
            {
                let m = self.arg_num(&args, 0, 1.0)? as i64;
                self.gfx.borrow_mut().grad_oklab = m != 0;
                return Ok(Value::Unit);
            },

            // mix_color(r0,g0,b0, r1,g1,b1, t) — set the pen colour to the
            // perceptual OkLab blend of two colours (t in 0..1). Far nicer
            // mid-tones than a raw RGB lerp.
            "mix_color" | "ผสมสี" | "混合颜色" | "色混合" | "색혼합" | "ترکیب_رنگ" | "امزج_اللون" | "ערבב_צבע" | "رنگ_ملاؤ" | "mélanger_couleur" | "farbe_mischen" | "смешать_цвет" => {
                let c0 = rgb(
                    self.arg_num(&args, 0, 0.0)?,
                    self.arg_num(&args, 1, 0.0)?,
                    self.arg_num(&args, 2, 0.0)?,
                );
                let c1 = rgb(
                    self.arg_num(&args, 3, 255.0)?,
                    self.arg_num(&args, 4, 255.0)?,
                    self.arg_num(&args, 5, 255.0)?,
                );
                let t = self.arg_num(&args, 6, 0.5)? as f32;
                self.gfx.borrow_mut().color = crate::gfx::color::mix_oklab(c0, c1, t);
                return Ok(Value::Unit);
            },

            // set_depth_test(on) — enable the per-pixel z-buffer for the deferred
            // 3-D/queued draws (correct interpenetration) instead of painter's-
            // only sort. 0 = off (default), non-zero = on.
            "set_depth_test" | "ทดสอบความลึก" | "深度测试" | "深度テスト" | "깊이테스트" | "تنظیم_آزمون_عمق" | "عيّن_اختبار_العمق" | "קבע_בדיקת_עומק" | "ڈیپتھ_ٹیسٹ_مقرر_کرو" | "définir_test_profondeur" | "tiefentest_setzen" | "задать_тест_глубины" =>
            {
                let on = self.arg_num(&args, 0, 1.0)? as i64 != 0;
                self.gfx.borrow_mut().depth_test = on;
                return Ok(Value::Unit);
            },

            // set_flat_shade(on) / ตั้งแฟลตเชด — perf test: skip all per-triangle/mesh
            // lighting (compute_lit_color) and draw with the raw pen colour.
            "set_flat_shade" | "ตั้งแฟลตเชด" | "平面着色" | "フラット着色" | "평면음영" | "تنظیم_سایه‌پردازی_تخت" | "عيّن_تظليلا_مسطحا" | "קבע_הצללה_שטוחה" | "فلیٹ_شیڈ_مقرر_کرو" =>
            {
                let on = self.arg_num(&args, 0, 1.0)? as i64 != 0;
                self.gfx.borrow_mut().flat_shade = on;
                return Ok(Value::Unit);
            },

            // set_normal_override(x,y,z) - force subsequent triangle/mesh lighting
            // to use a stylized world-space normal until reset_normal_override().
            "set_normal_override" =>
            {
                let x = self.arg_num(&args, 0, 0.0)? as f32;
                let y = self.arg_num(&args, 1, -1.0)? as f32;
                let z = self.arg_num(&args, 2, 0.0)? as f32;
                self.gfx.borrow_mut().normal_override = Some([x, y, z]);
                return Ok(Value::Unit);
            },

            "reset_normal_override" =>
            {
                self.gfx.borrow_mut().normal_override = None;
                return Ok(Value::Unit);
            },

            // clear_depth() / ล้างความลึก — force the z-buffer to clear on the next
            // flush. `เติม` already does this; call explicitly to start a fresh
            // depth pass mid-frame (e.g. a separate overlay scene).
            "clear_depth" | "ล้างความลึก" | "清深度" | "深度クリア" | "깊이지우기" | "پاک‌کردن_عمق" | "امسح_العمق" | "נקה_עומק" | "گہرائی_صاف_کرو" =>
            {
                self.gfx.borrow_mut().zbuf_needs_clear = true;
                return Ok(Value::Unit);
            },

            // depth_blur(focus, range, radius) / เบลอความลึก — depth-of-field post
            // pass over the framebuffer using the z-buffer: sharp at camera-space
            // depth `focus`, blurred up to `radius` px as depth departs by `range`.
            // Background (no geometry) blurs fully. Call AFTER `flush_3d` (so the
            // z-buffer is populated) and BEFORE `present`. Needs `set_depth_test(1)`.
            "depth_blur" | "เบลอความลึก" | "dof" | "depth_of_field" | "景深" | "تاری_عمق" | "ضبابية_العمق" | "טשטוש_עומק" | "ڈیپتھ_بلر" =>
            {
                let focus = self.arg_num(&args, 0, 30.0)? as f32;
                let range = self.arg_num(&args, 1, 60.0)? as f32;
                let radius = self.arg_num(&args, 2, 3.0)?.max(0.0) as usize;
                // oil [0..1] — oil-slick treatment of the blurred zone:
                // iridescent chroma fringe + hue swirl (water / heat haze).
                let oil = self.arg_num(&args, 3, 0.0)? as f32;
                let mut gfx = self.gfx.borrow_mut();
                let w = gfx.width;
                let h = gfx.height;
                if gfx.depth_buf.len() == w * h {
                    let g = &mut *gfx;
                    crate::gfx::raster::depth_of_field(
                        &mut g.buffer,
                        &g.depth_buf,
                        w,
                        h,
                        focus,
                        range,
                        radius,
                        oil,
                    );
                }
                return Ok(Value::Unit);
            },

            // light_pool(x, y, z, radius, r, g, b, intensity) / แอ่งแสง —
            // volumetric light splash: a soft additive radial vector gradient on
            // the floor at height y — the coloured pool a light throws on the
            // ground (underwater-light look). Smooth transparent edge, distance-
            // fog aware. Colours 0-255; intensity ~0.2-1.5.
            "light_pool" | "แอ่งแสง" | "光池" | "ライトプール" | "빛웅덩이" | "برکه_نور" | "بركة_ضوء" | "בריכת_אור" | "روشنی_تالاب" | "bassin_lumière" | "lichtpfütze" | "лужа_света" =>
            {
                let x = self.arg_num(&args, 0, 0.0)? as f32;
                let y = self.arg_num(&args, 1, 0.0)? as f32;
                let z = self.arg_num(&args, 2, 0.0)? as f32;
                let radius = self.arg_num(&args, 3, 20.0)? as f32;
                let r = self.arg_num(&args, 4, 255.0)? as f32 / 255.0;
                let g = self.arg_num(&args, 5, 255.0)? as f32 / 255.0;
                let b = self.arg_num(&args, 6, 255.0)? as f32 / 255.0;
                let inten = self.arg_num(&args, 7, 1.0)? as f32;
                self.gfx
                    .borrow_mut()
                    .emit_light_pool(x, y, z, radius, [r, g, b], inten);
                return Ok(Value::Unit);
            },

            // light_beam(x, y, z, floor_y, radius, r, g, b, intensity) / ลำแสงไฟ —
            // volumetric god-ray shaft: a soft additive double-cone from the
            // light position down to the floor plane, spreading to `radius`.
            // Pair with light_pool at the base. Colours 0-255.
            "light_beam" | "ลำแสงไฟ" | "光柱" | "ライトビーム" | "빛기둥" | "پرتو_نور" | "شعاع_ضوء" | "קרן_אור" | "روشنی_شعاع" | "faisceau_lumière" | "lichtstrahl" | "луч_света" =>
            {
                let x = self.arg_num(&args, 0, 0.0)? as f32;
                let y = self.arg_num(&args, 1, 0.0)? as f32;
                let z = self.arg_num(&args, 2, 0.0)? as f32;
                let fy = self.arg_num(&args, 3, 0.0)? as f32;
                let radius = self.arg_num(&args, 4, 14.0)? as f32;
                let r = self.arg_num(&args, 5, 255.0)? as f32 / 255.0;
                let g = self.arg_num(&args, 6, 255.0)? as f32 / 255.0;
                let b = self.arg_num(&args, 7, 255.0)? as f32 / 255.0;
                let inten = self.arg_num(&args, 8, 1.0)? as f32;
                self.gfx
                    .borrow_mut()
                    .emit_light_beam(x, y, z, fy, radius, [r, g, b], inten);
                return Ok(Value::Unit);
            },

            // grad_triangle(x0,y0,r0,g0,b0, x1,y1,r1,g1,b1, x2,y2,r2,g2,b2)
            // Smooth per-vertex gradient triangle — a cheap lit surface: put the
            // bright colour on the vertex facing the light. Honours set_alpha.
            "grad_triangle" | "สามเหลี่ยมไล่สี" | "渐变三角" | "グラデ三角" | "그라데삼각" | "مثلث_گرادیان" | "مثلث_متدرج" | "משולש_גרדיאנט" | "گریڈینٹ_مثلث" | "triangle_dégradé" | "dreieck_verlauf" | "градиент_треугольник" =>
            {
                let x0 = self.arg_num(&args, 0, 0.0)? as f32;
                let y0 = self.arg_num(&args, 1, 0.0)? as f32;
                let c0 = rgb(
                    self.arg_num(&args, 2, 255.0)?,
                    self.arg_num(&args, 3, 255.0)?,
                    self.arg_num(&args, 4, 255.0)?,
                );
                let x1 = self.arg_num(&args, 5, 0.0)? as f32;
                let y1 = self.arg_num(&args, 6, 0.0)? as f32;
                let c1 = rgb(
                    self.arg_num(&args, 7, 255.0)?,
                    self.arg_num(&args, 8, 255.0)?,
                    self.arg_num(&args, 9, 255.0)?,
                );
                let x2 = self.arg_num(&args, 10, 0.0)? as f32;
                let y2 = self.arg_num(&args, 11, 0.0)? as f32;
                let c2 = rgb(
                    self.arg_num(&args, 12, 255.0)?,
                    self.arg_num(&args, 13, 255.0)?,
                    self.arg_num(&args, 14, 255.0)?,
                );
                let mut gfx = self.gfx.borrow_mut();
                let (w, h, alpha, mode, lin, ok) = (
                    gfx.width,
                    gfx.height,
                    gfx.alpha,
                    gfx.blend,
                    gfx.linear_blend,
                    gfx.grad_oklab,
                );
                crate::gfx::raster::fill_triangle_grad(
                    &mut gfx.buffer,
                    w,
                    h,
                    alpha,
                    mode,
                    lin,
                    ok,
                    x0,
                    y0,
                    c0,
                    x1,
                    y1,
                    c1,
                    x2,
                    y2,
                    c2,
                );
                return Ok(Value::Unit);
            },

            // grad_rect(x,y,w,h, r0,g0,b0, r1,g1,b1, dir) — linear-gradient rect.
            // dir 0 = horizontal (left→right), else vertical (top→bottom).
            "grad_rect" | "สี่เหลี่ยมไล่สี" | "渐变矩形" | "グラデ矩形" | "그라데사각" | "مستطیل_گرادیان" | "مستطيل_متدرج" | "מלבן_גרדיאנט" | "گریڈینٹ_مستطیل" | "rectangle_dégradé" | "rechteck_verlauf" | "градиент_прямоугольник" =>
            {
                let x = self.arg_num(&args, 0, 0.0)? as f32;
                let y = self.arg_num(&args, 1, 0.0)? as f32;
                let rw = self.arg_num(&args, 2, 0.0)? as f32;
                let rh = self.arg_num(&args, 3, 0.0)? as f32;
                let c0 = rgb(
                    self.arg_num(&args, 4, 255.0)?,
                    self.arg_num(&args, 5, 255.0)?,
                    self.arg_num(&args, 6, 255.0)?,
                );
                let c1 = rgb(
                    self.arg_num(&args, 7, 0.0)?,
                    self.arg_num(&args, 8, 0.0)?,
                    self.arg_num(&args, 9, 0.0)?,
                );
                let dir = self.arg_num(&args, 10, 1.0)? as u8;
                let mut gfx = self.gfx.borrow_mut();
                let (w, h, alpha, mode, lin, ok) = (
                    gfx.width,
                    gfx.height,
                    gfx.alpha,
                    gfx.blend,
                    gfx.linear_blend,
                    gfx.grad_oklab,
                );
                crate::gfx::raster::fill_rect_grad(
                    &mut gfx.buffer,
                    w,
                    h,
                    alpha,
                    mode,
                    lin,
                    ok,
                    x,
                    y,
                    rw,
                    rh,
                    c0,
                    c1,
                    dir,
                );
                return Ok(Value::Unit);
            },

            // shadow_blob(cx,cy, rx,ry, alpha) — soft colored shadow ellipse in
            // the current pen colour. Dark colour = normal shadow; any hue = a
            // tinted/coloured shadow. Edge softness comes from shadow_params.
            "shadow_blob" | "เงาวงรี" | "阴影斑" | "影ブロブ" | "그림자블롭" | "لکه_سایه" | "بقعة_ظل" | "כתם_צל" | "سایہ_دھبہ" | "tache_ombre" | "schattenklecks" | "пятно_тени" =>
            {
                let cx = self.arg_num(&args, 0, 0.0)? as f32;
                let cy = self.arg_num(&args, 1, 0.0)? as f32;
                let rx = self.arg_num(&args, 2, 16.0)? as f32;
                let ry = self.arg_num(&args, 3, 8.0)? as f32;
                let a = self.arg_num(&args, 4, 0.5)? as f32;
                let mut gfx = self.gfx.borrow_mut();
                let (w, h, color, soft, mode, lin) = (
                    gfx.width,
                    gfx.height,
                    gfx.color,
                    gfx.shadow.soft,
                    gfx.blend,
                    gfx.linear_blend,
                );
                crate::gfx::raster::fill_disc_soft(
                    &mut gfx.buffer,
                    w,
                    h,
                    cx,
                    cy,
                    rx,
                    ry,
                    color,
                    a,
                    soft,
                    mode,
                    lin,
                );
                return Ok(Value::Unit);
            },

            // cast_shadow(cx,cy, height) — height-driven contact shadow in the
            // current pen colour. Closer to the surface (small height) = smaller,
            // darker, sharper; farther (large height) = bigger, fainter, softer.
            // Tune the ramp with shadow_params.
            "cast_shadow" | "ทอดเงา" | "投射阴影" | "影を落とす" | "그림자드리우기" | "افکندن_سایه" | "ألقِ_ظلا" | "הטל_צל" | "سایہ_ڈالو" | "projeter_ombre" | "schatten_werfen" | "отбросить_тень" =>
            {
                let cx = self.arg_num(&args, 0, 0.0)? as f32;
                let cy = self.arg_num(&args, 1, 0.0)? as f32;
                let height = (self.arg_num(&args, 2, 0.0)? as f32).max(0.0);
                let mut gfx = self.gfx.borrow_mut();
                let sp = gfx.shadow;
                let radius = (sp.base + sp.grow * height).max(0.5);
                let alpha = (sp.alpha - sp.fade * height).clamp(0.04, 1.0);
                let soft = (sp.soft + height * 0.004).clamp(0.0, 0.95);
                let (w, h, color, mode, lin) = (
                    gfx.width,
                    gfx.height,
                    gfx.color,
                    gfx.blend,
                    gfx.linear_blend,
                );
                crate::gfx::raster::fill_disc_soft(
                    &mut gfx.buffer,
                    w,
                    h,
                    cx,
                    cy,
                    radius,
                    radius * 0.62,
                    color,
                    alpha,
                    soft,
                    mode,
                    lin,
                );
                return Ok(Value::Unit);
            },

            // shadow_params(base, grow, alpha, fade, soft) — tune cast_shadow.
            // Each arg defaults to the current value, so you can set just one.
            "shadow_params" | "ตั้งค่าเงา" | "阴影参数" | "影設定" | "그림자설정" | "پارامترهای_سایه" | "معاملات_الظل" | "פרמטרי_צל" | "سایہ_پیرامیٹرز" | "paramètres_ombre" | "schattenparameter" | "параметры_тени" =>
            {
                let cur = self.gfx.borrow().shadow;
                let base = self.arg_num(&args, 0, cur.base as f64)? as f32;
                let grow = self.arg_num(&args, 1, cur.grow as f64)? as f32;
                let alpha = self.arg_num(&args, 2, cur.alpha as f64)? as f32;
                let fade = self.arg_num(&args, 3, cur.fade as f64)? as f32;
                let soft = self.arg_num(&args, 4, cur.soft as f64)? as f32;
                self.gfx.borrow_mut().shadow =
                    crate::gfx::ShadowParams { base, grow, alpha, fade, soft };
                return Ok(Value::Unit);
            },

            // depth_triangle(x0,y0, x1,y1, x2,y2, z) — queue a depth-sorted tri in
            // the current colour. Drawn back-to-front (painter's algorithm) at
            // present(); larger z = farther away. Lets 2-D sprites/quads sort by
            // depth the same way 3-D faces do.
            "depth_triangle" | "สามเหลี่ยมเรียงลึก" | "深度三角" | "深度三角形" | "깊이삼각" | "مثلث_عمق" | "مثلث_العمق" | "משולש_עומק" | "گہرائی_مثلث" | "triangle_profondeur" | "tiefendreieck" | "глубина_треугольник" =>
            {
                let x0 = self.arg_num(&args, 0, 0.0)? as f32;
                let y0 = self.arg_num(&args, 1, 0.0)? as f32;
                let x1 = self.arg_num(&args, 2, 0.0)? as f32;
                let y1 = self.arg_num(&args, 3, 0.0)? as f32;
                let x2 = self.arg_num(&args, 4, 0.0)? as f32;
                let y2 = self.arg_num(&args, 5, 0.0)? as f32;
                let z = self.arg_num(&args, 6, 0.0)? as f32;
                let mut gfx = self.gfx.borrow_mut();
                let color = gfx.color;
                gfx.depth_queue
                    .push_triangle(z, color, x0, y0, x1, y1, x2, y2);
                return Ok(Value::Unit);
            },

            // depth_line(x0,y0, x1,y1, z) — queue a depth-sorted line in the
            // current colour (same painter's queue as depth_triangle).
            "depth_line" | "เส้นเรียงลึก" | "深度线" | "深度線" | "깊이선" | "خط_عمق" | "خط_العمق" | "קו_עומק" | "گہرائی_لکیر" | "ligne_profondeur" | "tiefenlinie" | "глубина_линия" =>
            {
                let x0 = self.arg_num(&args, 0, 0.0)? as f32;
                let y0 = self.arg_num(&args, 1, 0.0)? as f32;
                let x1 = self.arg_num(&args, 2, 0.0)? as f32;
                let y1 = self.arg_num(&args, 3, 0.0)? as f32;
                let z = self.arg_num(&args, 4, 0.0)? as f32;
                let mut gfx = self.gfx.borrow_mut();
                let color = gfx.color;
                gfx.depth_queue.push_line(z, color, x0, y0, x1, y1);
                return Ok(Value::Unit);
            },

            // ══════════════════════════════════════════════════════════════════
            // GRAPHICS BUILTINS
            // Thai names first, then English aliases.
            // ══════════════════════════════════════════════════════════════════

            // ── เปิดหน้าต่าง(width, height, title) — open_window ──
            "เปิดหน้าต่าง" | "open_window" | "gfx_window" | "开窗" | "ウィンドウ開く" | "창열기" | "باز_کردن_پنجره" | "افتح_نافذة" | "פתח_חלון" | "ونڈو_کھولو" =>
            {
                let w = self.arg_num(&args, 0, 800.0)? as usize;
                let h = self.arg_num(&args, 1, 600.0)? as usize;
                #[cfg(not(target_arch = "wasm32"))]
                {
                    let title = args
                        .get(2)
                        .map(|v| v.to_string())
                        .unwrap_or_else(|| "Ling".into());
                    let mut gfx = self.gfx.borrow_mut();
                    let mut win = minifb::Window::new(
                        &title,
                        w,
                        h,
                        minifb::WindowOptions {
                            resize: false,
                            scale: minifb::Scale::X1,
                            ..Default::default()
                        },
                    )
                    .map_err(|e| EvalErr::from(format!("cannot open window: {e}")))?;
                    apply_frame_pacing(&mut win, gfx.vsync);
                    gfx.buffer = vec![0u32; w * h];
                    gfx.width = w;
                    gfx.height = h;
                    gfx.window = Some(win);
                    gfx.topmost_window = false;
                    gfx.sync_projection();
                }
                #[cfg(target_arch = "wasm32")]
                {
                    let mut gfx = self.gfx.borrow_mut();
                    gfx.width = w;
                    gfx.height = h;
                    gfx.buffer.resize(w * h, 0); // keep the CPU framebuffer in sync
                    gfx.sync_projection();
                    crate::gfx::webgl::resize(w as u32, h as u32);
                }
                return Ok(Value::Unit);
            },

            // ── เติม(r, g, b) — fill / clear screen with colour ──
            "เติม" | "fill" | "gfx_fill" | "clear" | "填" | "塗り潰し" | "채우기" | "清"
            | "消去" | "지우기" | "پر_کن" | "املأ" | "מלא" | "بھرو" => {
                let r = self.arg_num(&args, 0, 0.0)? as u32;
                let g = self.arg_num(&args, 1, 0.0)? as u32;
                let b = self.arg_num(&args, 2, 0.0)? as u32;
                #[cfg(not(target_arch = "wasm32"))]
                {
                    let c = (r << 16) | (g << 8) | b;
                    let mut gfx = self.gfx.borrow_mut();
                    gfx.buffer.fill(c);
                    gfx.zbuf_needs_clear = true; // clear color ⇒ clear depth next flush
                    gfx.edge_set.clear(); // reset shared-edge dedup for new frame
                }
                #[cfg(target_arch = "wasm32")]
                {
                    let mut gfx = self.gfx.borrow_mut();
                    gfx.fill_r = r as f32 / 255.0;
                    gfx.fill_g = g as f32 / 255.0;
                    gfx.fill_b = b as f32 / 255.0;
                    let c = (r << 16) | (g << 8) | b;
                    gfx.buffer.fill(c);
                    gfx.zbuf_needs_clear = true;
                    gfx.edge_set.clear();
                }
                return Ok(Value::Unit);
            },

            // ── set_color_hsl(h, s, l) — set drawing colour from HSL ──
            // h: 0–360 degrees, s: 0–100 saturation, l: 0–100 lightness
            "set_color_hsl" | "颜色HSL" | "色相" | "HSL色" | "HSL색설정" | "สีHSLวาด" | "تنظیم_رنگ_HSL" | "عيّن_اللون_HSL" | "קבע_צבע_HSL" | "HSL_رنگ_مقرر_کرو" | "définir_couleur_hsl" | "farbe_hsl_setzen" | "задать_цвет_hsl" =>
            {
                let h = self.arg_num(&args, 0, 0.0)?;
                let s = self.arg_num(&args, 1, 70.0)?;
                let l = self.arg_num(&args, 2, 50.0)?;
                let hex = hsl_to_hex(h, s, l);
                let r = u32::from_str_radix(&hex[1..3], 16).unwrap_or(255);
                let g = u32::from_str_radix(&hex[3..5], 16).unwrap_or(255);
                let b = u32::from_str_radix(&hex[5..7], 16).unwrap_or(255);
                self.gfx.borrow_mut().color = (r << 16) | (g << 8) | b;
                return Ok(Value::Unit);
            },

            // ── สีดินสอ(r, g, b) — set drawing colour ──
            "สีดินสอ" | "set_color" | "gfx_color" | "color" | "设色" | "色設定" | "색설정" | "تنظیم_رنگ" | "عيّن_اللون" | "קבע_צבע" | "رنگ_مقرر_کرو" =>
            {
                let r = self.arg_num(&args, 0, 255.0)? as u32;
                let g = self.arg_num(&args, 1, 255.0)? as u32;
                let b = self.arg_num(&args, 2, 255.0)? as u32;
                self.gfx.borrow_mut().color = (r << 16) | (g << 8) | b;
                return Ok(Value::Unit);
            },

            // ── วาดสามเหลี่ยม(x1,y1, x2,y2, x3,y3) — draw filled triangle ──
            "วาดสามเหลี่ยม"
            | "draw_triangle"
            | "gfx_triangle"
            | "triangle"
            | "画三角"
            | "三角形描画"
            | "삼각형그리기" | "رسم_مثلث" | "ارسم_مثلثا" | "צייר_משולש" | "مثلث_کھینچو" => {
                let x0 = self.arg_num(&args, 0, 0.0)? as f32;
                let y0 = self.arg_num(&args, 1, 0.0)? as f32;
                let x1 = self.arg_num(&args, 2, 0.0)? as f32;
                let y1 = self.arg_num(&args, 3, 0.0)? as f32;
                let x2 = self.arg_num(&args, 4, 0.0)? as f32;
                let y2 = self.arg_num(&args, 5, 0.0)? as f32;
                let mut gfx = self.gfx.borrow_mut();
                let color = gfx.color;
                #[cfg(not(target_arch = "wasm32"))]
                {
                    let w = gfx.width;
                    let h = gfx.height;
                    fill_triangle(&mut gfx.buffer, w, h, color, x0, y0, x1, y1, x2, y2);
                }
                #[cfg(target_arch = "wasm32")]
                gfx.depth_queue
                    .push_triangle(0.0, color, x0, y0, x1, y1, x2, y2);
                return Ok(Value::Unit);
            },

            // ── วาดเส้น(x1,y1, x2,y2) — draw line ──
            "วาดเส้น" | "draw_line" | "gfx_line" | "line" | "画线" | "線描く" | "선그리기" | "رسم_خط" | "ارسم_خط" | "צייר_קו" | "لکیر_کھینچو" =>
            {
                let x0 = self.arg_num(&args, 0, 0.0)? as f32;
                let y0 = self.arg_num(&args, 1, 0.0)? as f32;
                let x1 = self.arg_num(&args, 2, 0.0)? as f32;
                let y1 = self.arg_num(&args, 3, 0.0)? as f32;
                let mut gfx = self.gfx.borrow_mut();
                let color = gfx.color;
                #[cfg(not(target_arch = "wasm32"))]
                {
                    let w = gfx.width;
                    let h = gfx.height;
                    let aa = gfx.antialias;
                    let add = gfx.blend == 1;
                    if aa {
                        crate::gfx::raster::draw_line_aa(
                            &mut gfx.buffer,
                            w,
                            h,
                            color,
                            add,
                            x0,
                            y0,
                            x1,
                            y1,
                        );
                    } else {
                        draw_line(&mut gfx.buffer, w, h, color, x0, y0, x1, y1);
                    }
                }
                #[cfg(target_arch = "wasm32")]
                gfx.depth_queue.push_line(0.0, color, x0, y0, x1, y1);
                return Ok(Value::Unit);
            },

            // ── วาดจุด(x, y) — plot a single pixel ──
            "วาดจุด" | "draw_pixel" | "gfx_pixel" | "pixel" | "画点" | "点描く" | "점그리기" | "رسم_نقطه" | "ارسم_نقطة" | "צייר_פיקסל" | "پکسل_کھینچو" =>
            {
                let px = self.arg_num(&args, 0, 0.0)? as i32;
                let py = self.arg_num(&args, 1, 0.0)? as i32;
                #[cfg(not(target_arch = "wasm32"))]
                {
                    let mut gfx = self.gfx.borrow_mut();
                    let color = gfx.color;
                    let w = gfx.width;
                    let h = gfx.height;
                    if px >= 0 && py >= 0 && (px as usize) < w && (py as usize) < h {
                        gfx.buffer[py as usize * w + px as usize] = color;
                    }
                }
                #[cfg(target_arch = "wasm32")]
                {
                    // Render pixel as a 1×1 square via two triangles.
                    let mut gfx = self.gfx.borrow_mut();
                    let color = gfx.color;
                    let x = px as f32;
                    let y = py as f32;
                    gfx.depth_queue
                        .push_triangle(0.0, color, x, y, x + 1.0, y, x + 1.0, y + 1.0);
                    gfx.depth_queue
                        .push_triangle(0.0, color, x, y, x + 1.0, y + 1.0, x, y + 1.0);
                }
                return Ok(Value::Unit);
            },

            // ── แสดงผล() — flush depth queue, then present frame to screen ──
            "แสดงผล" | "present" | "gfx_present" | "show" | "显" | "呈现" | "表示" | "표시" | "نمایش" | "اعرض" | "הצג" | "دکھاؤ" =>
            {
                // Click-edge widgets (ui_button etc.) compare THIS frame's
                // mouse_now() against `mouse_was_down` to detect a fresh
                // press. That comparison only works if mouse_was_down
                // reflects what the script itself observed this frame — i.e.
                // the state from BEFORE update_with_buffer below pulls in new
                // OS events. Capturing it after (as the "freshest" read)
                // would mean a just-arrived click is already baked into
                // mouse_was_down by the time next frame's ui_button compares
                // against it, so `down && !mouse_was_down` is never true and
                // clicks never register at all. Declared at the top of the
                // match arm (not inside the block below) so it survives past
                // the wasm32/non-wasm32 split further down.
                #[cfg(not(target_arch = "wasm32"))]
                let pre_update_mouse_down = self
                    .gfx
                    .borrow()
                    .window
                    .as_ref()
                    .map(|w| w.get_mouse_down(minifb::MouseButton::Left))
                    .unwrap_or(false);
                #[cfg(not(target_arch = "wasm32"))]
                {
                    ling_fps_tick();
                    ling_phase_frame();
                    // Flush depth queue and present — release borrow before reading mouse.
                    {
                        let mut gfx = self.gfx.borrow_mut();
                        if !gfx.depth_queue.is_empty() {
                            let w = gfx.width;
                            let h = gfx.height;
                            let dt = gfx.depth_test;
                            let reset_z = gfx.zbuf_needs_clear;
                            let (bm, ba) = (gfx.blend, gfx.alpha);
                            let aa = gfx.antialias;
                            let queue = std::mem::take(&mut gfx.depth_queue);
                            {
                                let g = &mut *gfx;
                                let z = if dt { Some(&mut g.depth_buf) } else { None };
                                queue.flush(&mut g.buffer, z, reset_z, w, h, aa);
                            }
                            gfx.depth_queue.set_state(bm, ba);
                            gfx.zbuf_needs_clear = false;
                        }
                        let _t = std::time::Instant::now();
                        if !gfx.post_done {
                            gfx.toon_post_process();
                        }
                        gfx.post_done = false;
                        ling_phase_add(phase::TOON, _t.elapsed().as_nanos());
                        let w = gfx.width;
                        let h = gfx.height;
                        let g = &mut *gfx;
                        if g.frame_blur > 0.0 {
                            // Afterimage trails: previous frame decays by `frame_blur`
                            // per frame and composites with MAX — fresh content stays
                            // full-brightness, ghosts fade out over time.
                            // retention 0.98 @60fps ≈ trails last ~2.6 s.
                            let a = (g.frame_blur.clamp(0.0, 0.995) * 256.0) as u32;
                            if g.prev_frame.len() != g.buffer.len() {
                                g.prev_frame = g.buffer.clone();
                            }
                            for (dst, prev) in g.buffer.iter_mut().zip(g.prev_frame.iter_mut()) {
                                let c = *dst;
                                let pv = *prev;
                                let pr = (((pv >> 16) & 0xFF) * a) >> 8;
                                let pg = (((pv >> 8) & 0xFF) * a) >> 8;
                                let pb = ((pv & 0xFF) * a) >> 8;
                                let cr = (c >> 16) & 0xFF;
                                let cg = (c >> 8) & 0xFF;
                                let cb = c & 0xFF;
                                let outp = (cr.max(pr) << 16) | (cg.max(pg) << 8) | cb.max(pb);
                                *dst = outp;
                                *prev = outp;
                            }
                        }
                        if let Some(win) = g.window.as_mut() {
                            let _b = std::time::Instant::now();
                            win.update_with_buffer(&g.buffer, w, h)
                                .map_err(|e| EvalErr::from(format!("present error: {e}")))?;
                            ling_phase_add(phase::BLIT, _b.elapsed().as_nanos());
                        }
                    }
                    // Read mouse AFTER update_with_buffer so events are processed.
                    let mouse_pos = {
                        let gfx = self.gfx.borrow();
                        gfx.window
                            .as_ref()
                            .and_then(|w| w.get_mouse_pos(minifb::MouseMode::Clamp))
                    };
                    let mut gfx = self.gfx.borrow_mut();
                    if gfx.mouse_captured {
                        let w = gfx.width as f32;
                        let h = gfx.height as f32;
                        if let Some((mx, my)) = mouse_pos {
                            if gfx.last_mx.is_nan() {
                                gfx.mouse_dx = 0.0;
                                gfx.mouse_dy = 0.0;
                                gfx.last_mx = mx;
                                gfx.last_my = my;
                            } else {
                                gfx.mouse_dx = mx - gfx.last_mx;
                                gfx.mouse_dy = my - gfx.last_my;
                                // Wrap the cursor at every edge (L/R/U/D) → infinite look
                                // on both axes, and the cursor is NOT trapped (alt-tab works).
                                let margin = 6.0;
                                let (mut nx, mut ny, mut warp) = (mx, my, false);
                                if mx < margin {
                                    nx = w - margin - 2.0;
                                    warp = true;
                                } else if mx > w - margin {
                                    nx = margin + 2.0;
                                    warp = true;
                                }
                                if my < margin {
                                    ny = h - margin - 2.0;
                                    warp = true;
                                } else if my > h - margin {
                                    ny = margin + 2.0;
                                    warp = true;
                                }
                                if warp {
                                    #[cfg(windows)]
                                    unsafe {
                                        #[repr(C)]
                                        struct RECT {
                                            left: i32,
                                            top: i32,
                                            right: i32,
                                            bottom: i32,
                                        }
                                        extern "system" {
                                            fn GetForegroundWindow() -> isize;
                                            fn GetWindowRect(hwnd: isize, lpRect: *mut RECT)
                                                -> i32;
                                            fn SetCursorPos(x: i32, y: i32) -> i32;
                                        }
                                        let hwnd = GetForegroundWindow();
                                        let mut rect =
                                            RECT { left: 0, top: 0, right: 0, bottom: 0 };
                                        if GetWindowRect(hwnd, &mut rect) != 0 {
                                            SetCursorPos(
                                                rect.left + nx as i32,
                                                rect.top + ny as i32,
                                            );
                                        }
                                    }
                                    gfx.last_mx = nx;
                                    gfx.last_my = ny;
                                } else {
                                    gfx.last_mx = mx;
                                    gfx.last_my = my;
                                }
                            }
                        } else {
                            gfx.mouse_dx = 0.0;
                            gfx.mouse_dy = 0.0;
                        }
                    } else if let Some((mx, my)) = mouse_pos {
                        if gfx.last_mx.is_nan() {
                            gfx.mouse_dx = 0.0;
                            gfx.mouse_dy = 0.0;
                        } else {
                            gfx.mouse_dx = mx - gfx.last_mx;
                            gfx.mouse_dy = my - gfx.last_my;
                        }
                        gfx.last_mx = mx;
                        gfx.last_my = my;
                    } else {
                        gfx.mouse_dx = 0.0;
                        gfx.mouse_dy = 0.0;
                    }

                    // Alt-tab support: minifb has no WM_KILLFOCUS handler on Windows,
                    // so a key/button released while another window was focused can
                    // still read as "down" for one stale frame right after the user
                    // alt-tabs back. Detect the unfocused→focused transition and
                    // swallow raw input for a short grace window afterward instead of
                    // letting a phantom held key jerk the camera. See key_down /
                    // mouse_down* below (they early-out on gfx.input_suppressed()).
                    let is_active = gfx.window.as_mut().map(|w| w.is_active()).unwrap_or(true);
                    if is_active && !gfx.was_active {
                        gfx.focus_grace_frames = 5;
                        // Regained focus: restore HWND_TOPMOST so the borderless-
                        // fullscreen window covers the taskbar again.
                        #[cfg(windows)]
                        if gfx.topmost_window {
                            if let Some(w) = gfx.window.as_ref() {
                                set_window_topmost(w.get_window_handle() as isize, true);
                            }
                        }
                    } else if !is_active && gfx.was_active {
                        // Lost focus (alt-tab): drop topmost so the game stops
                        // covering whatever window the user just switched to —
                        // otherwise a topmost borderless window visually "wins"
                        // even though it's no longer focused, making alt-tab
                        // look broken.
                        #[cfg(windows)]
                        if gfx.topmost_window {
                            if let Some(w) = gfx.window.as_ref() {
                                set_window_topmost(w.get_window_handle() as isize, false);
                            }
                        }
                    }
                    gfx.was_active = is_active;
                    if gfx.focus_grace_frames > 0 {
                        gfx.focus_grace_frames -= 1;
                    }
                }
                #[cfg(target_arch = "wasm32")]
                {
                    {
                        // Software-render everything (3-D depth queue + 2-D vtex/ui that
                        // already wrote into the buffer) into the framebuffer, exactly
                        // like native, then upload that buffer to the canvas in one blit.
                        let mut gfx = self.gfx.borrow_mut();
                        let w = gfx.width;
                        let h = gfx.height;
                        if gfx.buffer.len() != w * h {
                            gfx.buffer.resize(w * h, 0);
                        }
                        if !gfx.depth_queue.is_empty() {
                            let dt = gfx.depth_test;
                            let reset_z = gfx.zbuf_needs_clear;
                            let aa = gfx.antialias;
                            let queue = std::mem::take(&mut gfx.depth_queue);
                            {
                                let g = &mut *gfx;
                                let z = if dt { Some(&mut g.depth_buf) } else { None };
                                queue.flush(&mut g.buffer, z, reset_z, w, h, aa);
                            }
                            gfx.zbuf_needs_clear = false;
                        }
                        if !gfx.post_done {
                            gfx.toon_post_process();
                        }
                        gfx.post_done = false;
                        crate::gfx::webgl::blit_rgb(&gfx.buffer, w, h);
                    }
                    self.wasm_pace_frame();
                }
                // Update the click-edge latch for interactive UI widgets —
                // using the PRE-update_with_buffer snapshot captured at the
                // top of this function (see the comment there for why).
                #[cfg(not(target_arch = "wasm32"))]
                {
                    self.mouse_was_down = pre_update_mouse_down;
                }
                // Increment frame counter
                self.frame_num += 1;
                return Ok(Value::Unit);
            },

            // ── เปิดหน้าต่างเต็มจอ(title) — true native-res fullscreen window ──
            "เปิดหน้าต่างเต็มจอ"
            | "open_fullscreen"
            | "fullscreen"
            | "全屏"
            | "全画面"
            | "전체화면" | "باز_کردن_تمام‌صفحه" | "افتح_ملء_الشاشة" | "פתח_מסך_מלא" | "فل_سکرین_کھولو" => {
                // In WASM the canvas defines the viewport; use its current size
                // as the default so the projection matches what's actually visible.
                #[cfg(target_arch = "wasm32")]
                let (default_w, default_h) = {
                    let (cw, ch) = crate::gfx::webgl::canvas_size();
                    (cw as f64, ch as f64)
                };
                // On native: query the actual primary monitor resolution.
                #[cfg(all(not(target_arch = "wasm32"), windows))]
                let (default_w, default_h) = unsafe {
                    extern "system" {
                        fn GetSystemMetrics(nIndex: i32) -> i32;
                    }
                    (GetSystemMetrics(0) as f64, GetSystemMetrics(1) as f64)
                };
                #[cfg(all(not(target_arch = "wasm32"), not(windows)))]
                let (default_w, default_h) = native_screen_size();

                let w = args
                    .get(1)
                    .map(|v| self.to_number(v).unwrap_or(default_w) as usize)
                    .unwrap_or(default_w as usize);
                let h = args
                    .get(2)
                    .map(|v| self.to_number(v).unwrap_or(default_h) as usize)
                    .unwrap_or(default_h as usize);
                #[cfg(not(target_arch = "wasm32"))]
                {
                    let title = args
                        .first()
                        .map(|v| v.to_string())
                        .unwrap_or_else(|| "Ling".into());
                    let mut gfx = self.gfx.borrow_mut();
                    let mut win = minifb::Window::new(
                        &title,
                        w,
                        h,
                        minifb::WindowOptions {
                            borderless: true,
                            title: false,
                            resize: false,
                            topmost: true,
                            scale: minifb::Scale::X1,
                            ..Default::default()
                        },
                    )
                    .map_err(|e| EvalErr::from(format!("cannot open fullscreen: {e}")))?;
                    apply_frame_pacing(&mut win, gfx.vsync);
                    // Grab the native handle *before* moving the window into gfx.
                    #[cfg(windows)]
                    let hwnd = win.get_window_handle() as isize;
                    gfx.buffer = vec![0u32; w * h];
                    gfx.width = w;
                    gfx.height = h;
                    gfx.window = Some(win);
                    gfx.topmost_window = true;
                    #[cfg(windows)]
                    {
                        gfx.hwnd = hwnd;
                    }
                    gfx.sync_projection();
                    // Strip all chrome and cover the full screen, above the taskbar.
                    #[cfg(windows)]
                    make_borderless_fullscreen(hwnd, w as i32, h as i32);
                    #[cfg(windows)]
                    force_window_focus(hwnd);
                }
                #[cfg(target_arch = "wasm32")]
                {
                    let mut gfx = self.gfx.borrow_mut();
                    gfx.width = w;
                    gfx.height = h;
                    gfx.buffer.resize(w * h, 0); // keep the CPU framebuffer in sync
                    gfx.sync_projection();
                    crate::gfx::webgl::resize(w as u32, h as u32);
                }
                return Ok(Value::Unit);
            },

            // ── ความกว้าง() / ความสูง() — current framebuffer size ──
            "get_width" | "ความกว้าง" | "宽" | "幅取得" | "너비" | "عرض" | "العرض" | "רוחב" | "چوڑائی" | "obtenir_largeur" | "breite_abrufen" | "получить_ширину" => {
                return Ok(Value::Number(self.gfx.borrow().width as f64));
            },
            "get_height" | "ความสูง" | "高" | "高取得" | "높이" | "ارتفاع" | "الارتفاع" | "גובה" | "اونچائی" | "obtenir_hauteur" | "höhe_abrufen" | "получить_высоту" => {
                return Ok(Value::Number(self.gfx.borrow().height as f64));
            },

            // ── monitor detection: physical display, not the framebuffer ──────
            // monitor_width() → primary-monitor pixel width
            "monitor_width" | "screen_width" | "屏宽" | "画面幅" | "화면너비" | "ความกว้างจอ" | "عرض_مانیتور" | "عرض_الشاشة" | "רוחב_צג" | "مانیٹر_چوڑائی" | "largeur_moniteur" | "monitor_breite" | "ширина_монитора" =>
            {
                return Ok(Value::Number(monitor_info().0 as f64));
            },
            // monitor_height() → primary-monitor pixel height
            "monitor_height" | "screen_height" | "屏高" | "画面高" | "화면높이" | "ความสูงจอ" | "ارتفاع_مانیتور" | "ارتفاع_الشاشة" | "גובה_צג" | "مانیٹر_اونچائی" | "hauteur_moniteur" | "monitor_höhe" | "высота_монитора" =>
            {
                return Ok(Value::Number(monitor_info().1 as f64));
            },
            // monitor_refresh() → refresh rate in Hz (a.k.a. the monitor framerate)
            "monitor_refresh"
            | "monitor_hz"
            | "monitor_fps"
            | "refresh_rate"
            | "刷新率"
            | "リフレッシュレート"
            | "주사율"
            | "อัตรารีเฟรช" | "نرخ_بروزرسانی_مانیتور" | "معدل_تحديث_الشاشة" | "קצב_רענון_צג" | "مانیٹر_ریفریش_ریٹ" | "fréquence_moniteur" | "monitor_bildwiederholrate" | "частота_монитора" => {
                return Ok(Value::Number(monitor_info().2 as f64));
            },
            // monitor_info() → [width, height, refresh_hz]
            "monitor_info" | "screen_info" | "屏幕信息" | "画面情報" | "화면정보" | "ข้อมูลจอ" | "اطلاعات_مانیتور" | "معلومات_الشاشة" | "מידע_צג" | "مانیٹر_معلومات" | "info_moniteur" | "bildschirminfo" | "инфо_монитора" =>
            {
                let (w, h, hz) = monitor_info();
                return Ok(Value::List(Rc::new(vec![
                    Value::Number(w as f64),
                    Value::Number(h as f64),
                    Value::Number(hz as f64),
                ])));
            },
            // set_fps(n) → cap the render loop at n frames per second
            "set_fps"
            | "set_target_fps"
            | "target_fps"
            | "设帧率"
            | "フレームレート設定"
            | "프레임설정"
            | "ตั้งเฟรมเรต" | "تنظیم_نرخ_فریم" | "عيّن_معدل_الإطارات" | "קבע_קצב_פריימים" | "ایف_پی_ایس_مقرر_کرو" | "définir_fps" | "fps_setzen" | "задать_fps" => {
                #[cfg(not(target_arch = "wasm32"))]
                {
                    let fps = self.arg_num(&args, 0, 60.0)?.max(1.0) as usize;
                    let mut gfx = self.gfx.borrow_mut();
                    if let Some(win) = gfx.window.as_mut() {
                        win.set_target_fps(fps);
                    }
                }
                #[cfg(target_arch = "wasm32")]
                {
                    self.wasm_target_fps = self.arg_num(&args, 0, 60.0)?.max(1.0);
                    self.wasm_next_present_ms = 0.0;
                }
                return Ok(Value::Unit);
            },

            // set_vsync(on) → pace the window to the monitor's refresh rate.
            // Frame-rate pacing (minifb has no swap-interval), not tear-free
            // vsync; `LING_FPS_CAP` and an explicit `set_fps` call still win.
            "set_vsync" | "vsync" | "垂直同步" | "垂直同期" | "수직동기" | "ตั้งวีซิงก์" | "تنظیم_وی‌سینک" | "عيّن_تزامن_رأسي" | "קבע_וי_סינק" | "وی_سینک_مقرر_کرو" | "définir_vsync" | "vsync_setzen" | "задать_vsync" =>
            {
                let on = self.arg_num(&args, 0, 1.0)? as i64 != 0;
                #[cfg(not(target_arch = "wasm32"))]
                {
                    let mut gfx = self.gfx.borrow_mut();
                    gfx.vsync = on;
                    if let Some(win) = gfx.window.as_mut() {
                        apply_frame_pacing(win, on);
                    }
                }
                #[cfg(target_arch = "wasm32")]
                {
                    self.wasm_target_fps = if on { monitor_info().2 as f64 } else { 240.0 };
                    self.wasm_next_present_ms = 0.0;
                }
                return Ok(Value::Unit);
            },

            // ── หน้าต่างเปิดอยู่() → bool — is the window still open? ──
            "หน้าต่างเปิดอยู่"
            | "window_is_open"
            | "gfx_is_open"
            | "is_open"
            | "窗开"
            | "開いている"
            | "창열림" | "پنجره_باز_است" | "النافذة_مفتوحة" | "החלון_פתוח" | "ونڈو_کھلی_ہے" => {
                #[cfg(not(target_arch = "wasm32"))]
                {
                    let gfx = self.gfx.borrow();
                    if gfx.want_quit {
                        return Ok(Value::Bool(false));
                    }
                    // Escape-to-quit needs the same GetAsyncKeyState fallback
                    // as key_down/key_pressed/text_poll (see those) — raw
                    // w.is_key_down(Escape) is WM_KEYDOWN-based and silently
                    // never fires if this topmost window didn't actually win
                    // real Win32 keyboard focus.
                    #[cfg(windows)]
                    let escape_down = if gfx.topmost_window {
                        window_is_foreground(gfx.hwnd) && os_key_down(0x1B) // VK_ESCAPE
                    } else {
                        gfx.window
                            .as_ref()
                            .map(|w| w.is_key_down(minifb::Key::Escape))
                            .unwrap_or(false)
                    };
                    #[cfg(not(windows))]
                    let escape_down = gfx
                        .window
                        .as_ref()
                        .map(|w| w.is_key_down(minifb::Key::Escape))
                        .unwrap_or(false);
                    let open = gfx.window.as_ref().map(|w| w.is_open()).unwrap_or(false)
                        && !escape_down;
                    return Ok(Value::Bool(open));
                }
                #[cfg(target_arch = "wasm32")]
                return Ok(Value::Bool(true));
            },

            // quit() — close the window the same way Escape does, for a
            // script-drawn UI element (an exit button) to call.
            "quit" | "exit_game" | "close_window" => {
                #[cfg(not(target_arch = "wasm32"))]
                {
                    self.gfx.borrow_mut().want_quit = true;
                }
                return Ok(Value::Unit);
            },

            // ── key_down(name) → bool — is a key held? ──
            "key_down" | "กดค้าง" | "按键" | "キー押す" | "키누름" | "کلید_فشرده" | "المفتاح_مضغوط" | "מקש_לחוץ" | "بٹن_دبا_ہوا" | "touche_enfoncée" | "taste_gedrückt" | "клавиша_нажата" => {
                #[cfg(not(target_arch = "wasm32"))]
                {
                    let name = self.arg_str(&args, 0, "");
                    let mut gfx = self.gfx.borrow_mut();
                    // The borderless-fullscreen/topmost window can be
                    // visually in front without ever winning real Win32
                    // keyboard focus (Windows' foreground-lock) — minifb's
                    // is_key_down is populated from WM_KEYDOWN, which then
                    // never arrives. GetAsyncKeyState reads the OS key-state
                    // table directly and doesn't need focus, so use it
                    // whenever this is that window (see force_window_focus).
                    #[cfg(windows)]
                    if gfx.topmost_window {
                        if !window_is_foreground(gfx.hwnd) {
                            return Ok(Value::Bool(false));
                        }
                        return Ok(Value::Bool(
                            str_to_vk(&name).map(os_key_down).unwrap_or(false),
                        ));
                    }
                    if gfx.input_suppressed() {
                        return Ok(Value::Bool(false));
                    }
                    let down = gfx
                        .window
                        .as_ref()
                        .and_then(|w| str_to_minifb_key(&name).map(|k| w.is_key_down(k)))
                        .unwrap_or(false);
                    return Ok(Value::Bool(down));
                }
                #[cfg(target_arch = "wasm32")]
                {
                    let name = self.arg_str(&args, 0, "");
                    return Ok(Value::Bool(crate::gfx::wasm_is_key_down(&name)));
                }
            },

            // ── key_pressed(name) → bool — was a key pressed this frame? ──
            "key_pressed" | "กดปุ่ม" | "键按" | "キー押した" | "키눌림" | "فشردن_کلید" | "ضغط_المفتاح" | "לחיצת_מקש" | "بٹن_دبانا" | "touche_appuyée" | "taste_getippt" | "клавиша_нажатие" => {
                #[cfg(not(target_arch = "wasm32"))]
                {
                    let name = self.arg_str(&args, 0, "");
                    let pressed = {
                        let mut gfx = self.gfx.borrow_mut();
                        #[cfg(windows)]
                        let topmost = gfx.topmost_window;
                        #[cfg(not(windows))]
                        let topmost = false;
                        if topmost {
                            #[cfg(windows)]
                            {
                                if !window_is_foreground(gfx.hwnd) {
                                    false
                                } else {
                                    match str_to_vk(&name) {
                                        Some(vk) => {
                                            let idx = (vk as usize) & 0xFF;
                                            let down = os_key_down(vk);
                                            let was = gfx.raw_keys_prev[idx];
                                            gfx.raw_keys_prev[idx] = down;
                                            down && !was
                                        },
                                        None => false,
                                    }
                                }
                            }
                            #[cfg(not(windows))]
                            {
                                false
                            }
                        } else if gfx.input_suppressed() {
                            false
                        } else {
                            gfx.window
                                .as_ref()
                                .and_then(|w| {
                                    str_to_minifb_key(&name)
                                        .map(|k| w.is_key_pressed(k, minifb::KeyRepeat::No))
                                })
                                .unwrap_or(false)
                        }
                    };
                    // gamepad Start behaves like Enter everywhere
                    let pressed =
                        pressed || ((name == "enter" || name == "return") && gamepad::start_edge());
                    return Ok(Value::Bool(pressed));
                }
                #[cfg(target_arch = "wasm32")]
                {
                    let name = self.arg_str(&args, 0, "");
                    let pressed = crate::gfx::wasm_is_key_pressed(&name);
                    return Ok(Value::Bool(pressed));
                }
            },

            // ── mouse_dx() / mouse_dy() → f64 — delta since last frame ──
            "mouse_dx" | "เมาส์X" | "鼠ΔX" | "マウスΔX" | "마우스ΔX" | "دلتا_ماوس_ایکس" | "فارق_الفأرة_س" | "דלתא_עכבר_X" | "ماؤس_ڈیلٹا_ایکس" | "souris_dx" | "maus_dx" | "мышь_dx" => {
                #[cfg(not(target_arch = "wasm32"))]
                return Ok(Value::Number(self.gfx.borrow().mouse_dx as f64));
                #[cfg(target_arch = "wasm32")]
                return Ok(Value::Number(crate::gfx::wasm_mouse_dx() as f64));
            },
            // ── mouse_scroll() → f64 — vertical scroll-wheel delta this frame ──
            #[cfg(not(target_arch = "wasm32"))]
            "mouse_scroll" | "ล้อเมาส์" | "滚轮" | "ホイール" | "스크롤" | "غلتک_ماوس" | "عجلة_الفأرة" | "גלגלת_עכבר" | "ماؤس_اسکرول" =>
            {
                let gfx = self.gfx.borrow();
                let s = gfx
                    .window
                    .as_ref()
                    .and_then(|w| w.get_scroll_wheel())
                    .map(|(_, y)| y as f64)
                    .unwrap_or(0.0);
                return Ok(Value::Number(s));
            },
            #[cfg(target_arch = "wasm32")]
            "mouse_scroll" | "ล้อเมาส์" | "滚轮" | "ホイール" | "스크롤" | "غلتک_ماوس" | "عجلة_الفأرة" | "גלגלת_עכבר" | "ماؤس_اسکرول" =>
            {
                return Ok(Value::Number(0.0));
            },
            "mouse_dy" | "เมาส์Y" | "鼠ΔY" | "マウスΔY" | "마우스ΔY" | "دلتا_ماوس_ایگرگ" | "فارق_الفأرة_ص" | "דלתא_עכבר_Y" | "ماؤس_ڈیلٹا_وائی" | "souris_dy" | "maus_dy" | "мышь_dy" => {
                #[cfg(not(target_arch = "wasm32"))]
                return Ok(Value::Number(self.gfx.borrow().mouse_dy as f64));
                #[cfg(target_arch = "wasm32")]
                return Ok(Value::Number(crate::gfx::wasm_mouse_dy() as f64));
            },

            // ── Gamepad / joystick input (ling-input "Sensorium" + gilrs) ──
            // pad_poll() → number — advance input one frame; returns # connected pads.
            "pad_poll" | "手柄轮询" | "パッド更新" | "패드폴링" | "อัปเดตแพด" | "بررسی_دسته" | "استطلع_اليد" | "בדוק_בקר" | "گیم_پیڈ_پول" | "interroger_manette" | "gamepad_abfragen" | "опросить_геймпад" =>
            {
                #[cfg(not(target_arch = "wasm32"))]
                return Ok(Value::Number(self.pad_poll() as f64));
                #[cfg(target_arch = "wasm32")]
                return Ok(Value::Number(input_web::poll() as f64));
            },
            // pad_count() → number — connected gamepads.
            "pad_count" | "手柄数" | "パッド数" | "패드수" | "จำนวนแพด" | "تعداد_دسته" | "عدد_أيدي_التحكم" | "מספר_בקרים" | "گیم_پیڈ_تعداد" | "nombre_manettes" | "gamepad_anzahl" | "число_геймпадов" =>
            {
                #[cfg(not(target_arch = "wasm32"))]
                {
                    let inp = self.input.borrow();
                    let n = inp.as_ref().map_or(0, |s| s.sensorium.devices.count());
                    return Ok(Value::Number(n as f64));
                }
                #[cfg(target_arch = "wasm32")]
                return Ok(Value::Number(input_web::count() as f64));
            },
            // pad_connected(i) → bool.
            "pad_connected" | "手柄连接" | "パッド接続" | "패드연결" | "แพดเชื่อม" | "دسته_متصل" | "يد_التحكم_متصلة" | "בקר_מחובר" | "گیم_پیڈ_منسلک" | "manette_connectée" | "gamepad_verbunden" | "геймпад_подключён" =>
            {
                #[cfg(not(target_arch = "wasm32"))]
                {
                    let i = self.arg_num(&args, 0, 0.0)? as usize;
                    let inp = self.input.borrow();
                    let c = inp
                        .as_ref()
                        .is_some_and(|s| s.sensorium.devices.for_player(i as u8).is_some());
                    return Ok(Value::Bool(c));
                }
                #[cfg(target_arch = "wasm32")]
                {
                    let i = self.arg_num(&args, 0, 0.0)? as usize;
                    return Ok(Value::Bool(input_web::is_connected(i)));
                }
            },
            // pad_button(i, name) → bool — is the button held?
            "pad_button" | "手柄按键" | "パッドボタン" | "패드버튼" | "ปุ่มแพด" | "دکمه_دسته" | "زر_اليد" | "כפתור_בקר" | "گیم_پیڈ_بٹن" | "bouton_manette" | "gamepad_taste" | "кнопка_геймпада" =>
            {
                #[cfg(not(target_arch = "wasm32"))]
                {
                    let i = self.arg_num(&args, 0, 0.0)? as usize;
                    let name = self.arg_str(&args, 1, "");
                    let down = parse_pad_button(&name)
                        .is_some_and(|b| self.with_pad(i, false, |p| p.is_down(b)));
                    return Ok(Value::Bool(down));
                }
                #[cfg(target_arch = "wasm32")]
                {
                    let i = self.arg_num(&args, 0, 0.0)? as usize;
                    let name = self.arg_str(&args, 1, "");
                    return Ok(Value::Bool(input_web::button_down(i, &name)));
                }
            },
            // pad_pressed(i, name) → bool — pressed this frame?
            // On WASM we only have the current snapshot, so treat as button_down.
            "pad_pressed" | "手柄按下" | "パッド押下" | "패드눌림" | "แพดกด" | "دکمه_دسته_فشرده" | "زر_اليد_مضغوط" | "כפתור_בקר_לחוץ" | "گیم_پیڈ_دبایا" | "manette_appuyée" | "gamepad_gedrückt" | "геймпад_нажат" =>
            {
                #[cfg(not(target_arch = "wasm32"))]
                {
                    let i = self.arg_num(&args, 0, 0.0)? as usize;
                    let name = self.arg_str(&args, 1, "");
                    let p = parse_pad_button(&name)
                        .is_some_and(|b| self.with_pad(i, false, |g| g.just_pressed(b)));
                    return Ok(Value::Bool(p));
                }
                #[cfg(target_arch = "wasm32")]
                {
                    let i = self.arg_num(&args, 0, 0.0)? as usize;
                    let name = self.arg_str(&args, 1, "");
                    return Ok(Value::Bool(input_web::button_down(i, &name)));
                }
            },
            // pad_lx(i)/pad_ly(i)/pad_rx(i)/pad_ry(i) → number — stick axes (−1..=1).
            "pad_lx" | "手柄左X" | "パッド左X" | "패드왼X" | "แพดซ้ายX" | "آنالوگ_چپ_ایکس" | "عصا_اليسرى_س" | "ג'ויסטיק_שמאל_X" | "بائیں_اسٹک_ایکس" | "manette_axe_gauche_x" | "gamepad_lx" | "геймпад_ось_лево_x" => {
                #[cfg(not(target_arch = "wasm32"))]
                {
                    let i = self.arg_num(&args, 0, 0.0)? as usize;
                    return Ok(Value::Number(
                        self.with_pad(i, 0.0, |p| p.left_stick.x as f64),
                    ));
                }
                #[cfg(target_arch = "wasm32")]
                {
                    let i = self.arg_num(&args, 0, 0.0)? as usize;
                    return Ok(Value::Number(input_web::axis_lx(i) as f64));
                }
            },
            "pad_ly" | "手柄左Y" | "パッド左Y" | "패드왼Y" | "แพดซ้ายY" | "آنالوگ_چپ_ایگرگ" | "عصا_اليسرى_ص" | "ג'ויסטיק_שמאל_Y" | "بائیں_اسٹک_وائی" | "manette_axe_gauche_y" | "gamepad_ly" | "геймпад_ось_лево_y" => {
                #[cfg(not(target_arch = "wasm32"))]
                {
                    let i = self.arg_num(&args, 0, 0.0)? as usize;
                    return Ok(Value::Number(
                        self.with_pad(i, 0.0, |p| p.left_stick.y as f64),
                    ));
                }
                #[cfg(target_arch = "wasm32")]
                {
                    let i = self.arg_num(&args, 0, 0.0)? as usize;
                    return Ok(Value::Number(input_web::axis_ly(i) as f64));
                }
            },
            "pad_rx" | "手柄右X" | "パッド右X" | "패드오X" | "แพดขวาX" | "آنالوگ_راست_ایکس" | "عصا_اليمنى_س" | "ג'ויסטיק_ימין_X" | "دائیں_اسٹک_ایکس" | "manette_axe_droit_x" | "gamepad_rx" | "геймпад_ось_право_x" => {
                #[cfg(not(target_arch = "wasm32"))]
                {
                    let i = self.arg_num(&args, 0, 0.0)? as usize;
                    return Ok(Value::Number(
                        self.with_pad(i, 0.0, |p| p.right_stick.x as f64),
                    ));
                }
                #[cfg(target_arch = "wasm32")]
                {
                    let i = self.arg_num(&args, 0, 0.0)? as usize;
                    return Ok(Value::Number(input_web::axis_rx(i) as f64));
                }
            },
            "pad_ry" | "手柄右Y" | "パッド右Y" | "패드오Y" | "แพดขวาY" | "آنالوگ_راست_ایگرگ" | "عصا_اليمنى_ص" | "ג'ויסטיק_ימין_Y" | "دائیں_اسٹک_وائی" | "manette_axe_droit_y" | "gamepad_ry" | "геймпад_ось_право_y" => {
                #[cfg(not(target_arch = "wasm32"))]
                {
                    let i = self.arg_num(&args, 0, 0.0)? as usize;
                    return Ok(Value::Number(
                        self.with_pad(i, 0.0, |p| p.right_stick.y as f64),
                    ));
                }
                #[cfg(target_arch = "wasm32")]
                {
                    let i = self.arg_num(&args, 0, 0.0)? as usize;
                    return Ok(Value::Number(input_web::axis_ry(i) as f64));
                }
            },
            // pad_lt(i)/pad_rt(i) → number — analog triggers (0..=1).
            "pad_lt" | "手柄左扳机" | "パッド左トリガー" | "패드왼트리거" | "ไกแพดซ้าย" | "ماشه_چپ" | "زناد_اليسار" | "הדק_שמאל" | "بائیں_ٹریگر" | "manette_gâchette_gauche" | "gamepad_lt" | "геймпад_триггер_лево" =>
            {
                #[cfg(not(target_arch = "wasm32"))]
                {
                    let i = self.arg_num(&args, 0, 0.0)? as usize;
                    return Ok(Value::Number(
                        self.with_pad(i, 0.0, |p| p.left_trigger as f64),
                    ));
                }
                #[cfg(target_arch = "wasm32")]
                {
                    let i = self.arg_num(&args, 0, 0.0)? as usize;
                    return Ok(Value::Number(input_web::trigger_lt(i) as f64));
                }
            },
            "pad_rt" | "手柄右扳机" | "パッド右トリガー" | "패드오트리거" | "ไกแพดขวา" | "ماشه_راست" | "زناد_اليمين" | "הדק_ימין" | "دائیں_ٹریگر" | "manette_gâchette_droite" | "gamepad_rt" | "геймпад_триггер_право" =>
            {
                #[cfg(not(target_arch = "wasm32"))]
                {
                    let i = self.arg_num(&args, 0, 0.0)? as usize;
                    return Ok(Value::Number(
                        self.with_pad(i, 0.0, |p| p.right_trigger as f64),
                    ));
                }
                #[cfg(target_arch = "wasm32")]
                {
                    let i = self.arg_num(&args, 0, 0.0)? as usize;
                    return Ok(Value::Number(input_web::trigger_rt(i) as f64));
                }
            },
            // pad_rumble(i, lo, hi) → unit — set rumble motor amplitudes (0..=1).
            "pad_rumble" | "手柄震动" | "パッド振動" | "패드진동" | "แพดสั่น" | "لرزش_دسته" | "اهتزاز_اليد" | "רטט_בקר" | "گیم_پیڈ_تھرتھراہٹ" | "vibration_manette" | "gamepad_vibration" | "вибрация_геймпада" =>
            {
                #[cfg(not(target_arch = "wasm32"))]
                {
                    use ling_input::backend::InputBackend;
                    let i = self.arg_num(&args, 0, 0.0)? as usize;
                    let lo = self.arg_num(&args, 1, 0.0)? as f32;
                    let hi = self.arg_num(&args, 2, lo as f64)? as f32;
                    let mut inp = self.input.borrow_mut();
                    if let Some(s) = inp.as_mut() {
                        if let Some(dev) = s.sensorium.devices.for_player(i as u8).map(|d| d.id) {
                            s.backend.set_rumble(
                                dev,
                                ling_input::Rumble { low: lo, high: hi, ..Default::default() },
                            );
                        }
                    }
                    return Ok(Value::Unit);
                }
                #[cfg(target_arch = "wasm32")]
                return Ok(Value::Unit);
            },

            // ── set_camera_pos(x, y, z) — move camera to world position ──
            "set_camera_pos" | "ตั้งตำแหน่งกล้อง" | "镜坐标" | "カメラ座標" | "카메라좌표" | "تنظیم_موقعیت_دوربین" | "عيّن_موضع_الكاميرا" | "קבע_מיקום_מצלמה" | "کیمرہ_مقام_مقرر_کرو" | "définir_position_caméra" | "kameraposition_setzen" | "задать_позицию_камеры" =>
            {
                let x = self.arg_num(&args, 0, 0.0)? as f32;
                let y = self.arg_num(&args, 1, 0.0)? as f32;
                let z = self.arg_num(&args, 2, 0.0)? as f32;
                {
                    let mut gfx = self.gfx.borrow_mut();
                    gfx.camera.tx = x;
                    gfx.camera.ty = y;
                    gfx.camera.tz = z;
                }
                #[cfg(not(target_arch = "wasm32"))]
                if let Some(audio) = &self.audio {
                    audio.set_listener_pos(x, y, z);
                }
                return Ok(Value::Unit);
            },

            // ── move_camera(dx, dy, dz) — translate camera by delta ──
            "move_camera" => {
                let dx = self.arg_num(&args, 0, 0.0)? as f32;
                let dy = self.arg_num(&args, 1, 0.0)? as f32;
                let dz = self.arg_num(&args, 2, 0.0)? as f32;
                let mut gfx = self.gfx.borrow_mut();
                gfx.camera.tx += dx;
                gfx.camera.ty += dy;
                gfx.camera.tz += dz;
                return Ok(Value::Unit);
            },

            // ── set_zdist(d) — set perspective z-offset (field-of-view taper) ──
            "set_zdist" | "ตั้งระยะห่าง" | "镜距" | "Z距離設定" | "Z거리설정" | "تنظیم_فاصله_عمق" | "عيّن_مسافة_العمق" | "קבע_מרחק_עומק" | "گہرائی_فاصلہ_مقرر_کرو" | "définir_distance_z" | "z_abstand_setzen" | "задать_дистанцию_z" =>
            {
                let d = self.arg_num(&args, 0, 5.0)? as f32;
                self.gfx.borrow_mut().camera.zdist = d;
                return Ok(Value::Unit);
            },

            // ── capture_mouse() — hide cursor and warp to centre each frame ──
            "capture_mouse" | "จับเมาส์" | "捕鼠" | "マウス捕捉" | "마우스잡기" | "ضبط_ماوس" | "امسك_الفأرة" | "לכוד_עכבר" | "ماؤس_پکڑو" | "capturer_souris" | "maus_erfassen" | "захватить_мышь" =>
            {
                #[cfg(not(target_arch = "wasm32"))]
                {
                    let mut gfx = self.gfx.borrow_mut();
                    gfx.mouse_captured = true;
                    gfx.last_mx = f32::NAN;
                    if let Some(win) = gfx.window.as_mut() {
                        win.set_cursor_visibility(false);
                    }
                }
                return Ok(Value::Unit);
            },

            // ── release_mouse() — restore cursor and remove clip region ──
            "release_mouse" => {
                #[cfg(not(target_arch = "wasm32"))]
                {
                    let mut gfx = self.gfx.borrow_mut();
                    gfx.mouse_captured = false;
                    gfx.last_mx = f32::NAN;
                    if let Some(win) = gfx.window.as_mut() {
                        win.set_cursor_visibility(true);
                    }
                    #[cfg(windows)]
                    unsafe {
                        // Null releases the clip; reuse the RECT-typed declaration above.
                        extern "system" {
                            fn ClipCursor(lpRect: *const std::ffi::c_void) -> i32;
                        }
                        ClipCursor(std::ptr::null());
                    }
                }
                return Ok(Value::Unit);
            },

            // ── cursor_hide() / cursor_show() — just the OS cursor's visibility,
            // no warp-to-centre or clip region (unlike capture_mouse/release_mouse,
            // which are for FPS-style look-around). For point-and-click play where
            // the cursor still needs to move freely and mouse_x()/mouse_y() still
            // need to track real position, just hide the system pointer glyph.
            "cursor_hide" => {
                #[cfg(not(target_arch = "wasm32"))]
                if let Some(win) = self.gfx.borrow_mut().window.as_mut() {
                    win.set_cursor_visibility(false);
                }
                return Ok(Value::Unit);
            },
            "cursor_show" => {
                #[cfg(not(target_arch = "wasm32"))]
                if let Some(win) = self.gfx.borrow_mut().window.as_mut() {
                    win.set_cursor_visibility(true);
                }
                return Ok(Value::Unit);
            },

            // ══════════════════════════════════════════════════════════════════
            // 3-D / 4-D DRAWING — camera, lights, depth-sorted geometry
            // ══════════════════════════════════════════════════════════════════

            // ── set_camera(cry, sry, crx, srx) — store precomputed camera trig ──
            // Call once per frame after computing cos/sin of your rotation angles.
            "set_camera" | "ตั้งกล้อง" | "设镜" | "设置摄像机" | "カメラ設定" | "카메라설정" | "تنظیم_دوربین" | "عيّن_الكاميرا" | "קבע_מצלמה" | "کیمرہ_مقرر_کرو" | "définir_caméra" | "kamera_setzen" | "задать_камеру" =>
            {
                let cry = self.arg_num(&args, 0, 1.0)? as f32;
                let sry = self.arg_num(&args, 1, 0.0)? as f32;
                let crx = self.arg_num(&args, 2, 1.0)? as f32;
                let srx = self.arg_num(&args, 3, 0.0)? as f32;
                let mut gfx = self.gfx.borrow_mut();
                gfx.camera.cry = cry;
                gfx.camera.sry = sry;
                gfx.camera.crx = crx;
                gfx.camera.srx = srx;
                return Ok(Value::Unit);
            },

            // ── set_projection(cx, cy, focal, zdist) — override projection params ──
            // Automatically set when the window opens; override only if needed.
            "set_projection" | "ตั้งโปรเจกชัน" | "投影" | "投影設定" | "투영설정" | "تنظیم_فرافکنی" | "عيّن_الإسقاط" | "קבע_הטלה" | "پروجیکشن_مقرر_کرو" | "définir_projection" | "projektion_setzen" | "задать_проекцию" =>
            {
                let cx = self.arg_num(&args, 0, 960.0)? as f32;
                let cy = self.arg_num(&args, 1, 540.0)? as f32;
                let focal = self.arg_num(&args, 2, 1080.0)? as f32;
                let zdist = self.arg_num(&args, 3, 5.0)? as f32;
                let mut gfx = self.gfx.borrow_mut();
                gfx.camera.cx = cx;
                gfx.camera.cy = cy;
                gfx.camera.focal = focal;
                gfx.camera.zdist = zdist;
                return Ok(Value::Unit);
            },

            // ── mesh_load(path) → handle · loads a glb/gltf (skeleton + skin + animation) ──
            "gltf_load" => {
                let path = self.arg_str(&args, 0, "");
                match ling_physics::gltf::GltfModel::load(&path) {
                    Ok(m) => {
                        self.gltf_models.borrow_mut().push(m);
                        let h = self.gltf_models.borrow().len() - 1;
                        return Ok(Value::Number(h as f64));
                    }
                    Err(e) => {
                        eprintln!("mesh_load failed ({path}): {e}");
                        return Ok(Value::Number(-1.0));
                    }
                }
            },
            // mesh_anim_count(handle) → number of animation clips
            "gltf_anim_count" => {
                let h = self.arg_num(&args, 0, -1.0)? as i64;
                let n = self
                    .gltf_models
                    .borrow()
                    .get(h as usize)
                    .map(|m| m.animations.len())
                    .unwrap_or(0);
                return Ok(Value::Number(n as f64));
            },
            // mesh_anim_name(handle, i) → clip name
            "gltf_anim_name" => {
                let h = self.arg_num(&args, 0, -1.0)? as i64;
                let i = self.arg_num(&args, 1, 0.0)? as usize;
                let s = self
                    .gltf_models
                    .borrow()
                    .get(h as usize)
                    .and_then(|m| m.animations.get(i))
                    .map(|a| a.name.clone())
                    .unwrap_or_default();
                return Ok(Value::Str(s));
            },
            // mesh_anim_dur(handle, i) → clip duration (seconds)
            "gltf_anim_dur" => {
                let h = self.arg_num(&args, 0, -1.0)? as i64;
                let i = self.arg_num(&args, 1, 0.0)? as usize;
                let d = self
                    .gltf_models
                    .borrow()
                    .get(h as usize)
                    .and_then(|m| m.animations.get(i))
                    .map(|a| a.duration)
                    .unwrap_or(0.0);
                return Ok(Value::Number(d as f64));
            },
            // mesh_tris(handle) → total triangle count (perf sanity check)
            "gltf_tris" => {
                let h = self.arg_num(&args, 0, -1.0)? as i64;
                let n: usize = self
                    .gltf_models
                    .borrow()
                    .get(h as usize)
                    .map(|m| m.meshes.iter().map(|mm| mm.indices.len()).sum::<usize>() / 3)
                    .unwrap_or(0);
                return Ok(Value::Number(n as f64));
            },

            // gltf_joint_count(handle) → number of skin joints (bones)
            "gltf_joint_count" => {
                let h = self.arg_num(&args, 0, -1.0)? as i64;
                let n = self
                    .gltf_models
                    .borrow()
                    .get(h as usize)
                    .and_then(|m| m.skins.first())
                    .map(|s| s.joints.len())
                    .unwrap_or(0);
                return Ok(Value::Number(n as f64));
            },
            // gltf_joint_name(handle, j) → bone name (its node's name)
            "gltf_joint_name" => {
                let h = self.arg_num(&args, 0, -1.0)? as i64;
                let j = self.arg_num(&args, 1, 0.0)? as usize;
                let models = self.gltf_models.borrow();
                let s = models
                    .get(h as usize)
                    .and_then(|m| {
                        m.skins
                            .first()
                            .and_then(|sk| sk.joints.get(j))
                            .and_then(|jt| m.nodes.get(jt.node_idx))
                            .map(|n| n.name.clone())
                    })
                    .unwrap_or_default();
                return Ok(Value::Str(s));
            },

            // ── gltf_draw(handle, ox,oy,oz, scale, yaw) — filled render of a loaded model ──
            //   glTF is Y-up / -Z-forward; the engine is Y-down, so we flip Y and Z, then
            //   yaw about Y, scale, translate. Per-part colour by mesh name. Lit + depth-queued
            //   exactly like draw_mesh, so it shares the camera + z-buffer.
            "gltf_draw" => {
                let hh = self.arg_num(&args, 0, -1.0)? as i64;
                let ox = self.arg_num(&args, 1, 0.0)? as f32;
                let oy = self.arg_num(&args, 2, 0.0)? as f32;
                let oz = self.arg_num(&args, 3, 0.0)? as f32;
                let scale = self.arg_num(&args, 4, 1.0)? as f32;
                let yaw = self.arg_num(&args, 5, 0.0)? as f32;
                let (sy, cyy) = yaw.sin_cos();
                let models = self.gltf_models.borrow();
                let model = match models.get(hh as usize) {
                    Some(m) => m,
                    None => return Ok(Value::Unit),
                };
                let mut gfx = self.gfx.borrow_mut();
                let cp = {
                    let c = &gfx.camera;
                    ling_gpu::CameraParams {
                        cry: c.cry, sry: c.sry, crx: c.crx, srx: c.srx,
                        cx: c.cx, cy: c.cy, focal: c.focal, zdist: c.zdist,
                        tx: c.tx, ty: c.ty, tz: c.tz,
                    }
                };
                let near = -gfx.camera.zdist + 0.02;
                let ambient = gfx.ambient;
                for mesh in &model.meshes {
                    let nlow = mesh.name.to_lowercase();
                    let base: u32 = if nlow.contains("hair") {
                        0x7a4a28
                    } else if nlow.contains("cloth") || nlow.contains("top") {
                        0x4a86e0
                    } else if nlow.contains("wing") {
                        0xe6ecf5
                    } else if nlow.contains("star") {
                        0xffd24d
                    } else {
                        0xf2d6b8
                    };
                    let nv = mesh.verts.len();
                    if nv == 0 {
                        continue;
                    }
                    let mut world = vec![0.0f32; nv * 3];
                    for (i, v) in mesh.verts.iter().enumerate() {
                        let gx = v.pos.x * scale;
                        let gy = -v.pos.y * scale;
                        let gz = -v.pos.z * scale;
                        let rx = gx * cyy + gz * sy;
                        let rz = -gx * sy + gz * cyy;
                        world[i * 3] = ox + rx;
                        world[i * 3 + 1] = oy + gy;
                        world[i * 3 + 2] = oz + rz;
                    }
                    let mut proj = vec![0.0f32; nv * 3];
                    ling_gpu::backend().project_points(&world, &cp, &mut proj);
                    let idx = &mesh.indices;
                    let nt = idx.len() / 3;
                    for t in 0..nt {
                        let ia = idx[t * 3] as usize;
                        let ib = idx[t * 3 + 1] as usize;
                        let ic = idx[t * 3 + 2] as usize;
                        if ia >= nv || ib >= nv || ic >= nv {
                            continue;
                        }
                        let (da, db, dc) = (proj[ia * 3 + 2], proj[ib * 3 + 2], proj[ic * 3 + 2]);
                        if (da + db + dc) / 3.0 <= near {
                            continue;
                        }
                        let col = {
                            let (ax, ay, az) = (world[ia * 3], world[ia * 3 + 1], world[ia * 3 + 2]);
                            let (bx, by, bz) = (world[ib * 3], world[ib * 3 + 1], world[ib * 3 + 2]);
                            let (px, py, pz) = (world[ic * 3], world[ic * 3 + 1], world[ic * 3 + 2]);
                            let (ux, uy, uz) = (bx - ax, by - ay, bz - az);
                            let (vx, vy, vz) = (px - ax, py - ay, pz - az);
                            let normal = [uy * vz - uz * vy, uz * vx - ux * vz, ux * vy - uy * vx];
                            let centroid =
                                [(ax + bx + px) / 3.0, (ay + by + py) / 3.0, (az + bz + pz) / 3.0];
                            if gfx.flat_shade {
                                base
                            } else {
                                crate::gfx::light::compute_lit_color(
                                    base, normal, centroid, &gfx.lights, ambient,
                                )
                            }
                        };
                        let depth = (da + db + dc) / 3.0;
                        let col = gfx.fog_apply(col, depth);
                        gfx.depth_queue.push_triangle_zv(
                            col,
                            proj[ia * 3], proj[ia * 3 + 1], da,
                            proj[ib * 3], proj[ib * 3 + 1], db,
                            proj[ic * 3], proj[ic * 3 + 1], dc,
                        );
                    }
                }
                return Ok(Value::Unit);
            },

            // ── gltf_autorig(handle) → synthesize a humanoid skeleton + skin weights ──
            "gltf_autorig" => {
                let hh = self.arg_num(&args, 0, -1.0)? as i64;
                let mut models = self.gltf_models.borrow_mut();
                if let Some(m) = models.get_mut(hh as usize) {
                    return Ok(Value::Number(m.autorig() as f64));
                }
                return Ok(Value::Number(0.0));
            },

            // ── gltf_pose_draw(handle, ox,oy,oz, scale, yaw, poseList) ──
            //   Like gltf_draw, but linear-blend-skins the mesh by `poseList` first.
            //   poseList = flat XYZ-euler radians, 3 per bone (12 bones → 36 values).
            "gltf_pose_draw" => {
                let hh = self.arg_num(&args, 0, -1.0)? as i64;
                let ox = self.arg_num(&args, 1, 0.0)? as f32;
                let oy = self.arg_num(&args, 2, 0.0)? as f32;
                let oz = self.arg_num(&args, 3, 0.0)? as f32;
                let scale = self.arg_num(&args, 4, 1.0)? as f32;
                let yaw = self.arg_num(&args, 5, 0.0)? as f32;
                let euler: Vec<f32> = match args.get(6) {
                    Some(Value::List(v)) => {
                        v.iter().map(|x| self.to_number(x).unwrap_or(0.0) as f32).collect()
                    },
                    _ => Vec::new(),
                };
                let (sy, cyy) = yaw.sin_cos();
                let models = self.gltf_models.borrow();
                let model = match models.get(hh as usize) {
                    Some(m) => m,
                    None => return Ok(Value::Unit),
                };
                let skinned = model.skin_local(&euler);
                let mut gfx = self.gfx.borrow_mut();
                let cp = {
                    let c = &gfx.camera;
                    ling_gpu::CameraParams {
                        cry: c.cry, sry: c.sry, crx: c.crx, srx: c.srx,
                        cx: c.cx, cy: c.cy, focal: c.focal, zdist: c.zdist,
                        tx: c.tx, ty: c.ty, tz: c.tz,
                    }
                };
                let near = -gfx.camera.zdist + 0.02;
                let ambient = gfx.ambient;
                for (mi, mesh) in model.meshes.iter().enumerate() {
                    let nlow = mesh.name.to_lowercase();
                    let base: u32 = if nlow.contains("hair") {
                        0x7a4a28
                    } else if nlow.contains("cloth") || nlow.contains("top") {
                        0x4a86e0
                    } else if nlow.contains("wing") {
                        0xe6ecf5
                    } else if nlow.contains("star") {
                        0xffd24d
                    } else {
                        0xf2d6b8
                    };
                    let sk = match skinned.get(mi) {
                        Some(s) => s,
                        None => continue,
                    };
                    let nv = sk.len();
                    if nv == 0 {
                        continue;
                    }
                    let mut world = vec![0.0f32; nv * 3];
                    for i in 0..nv {
                        let gx = sk[i][0] * scale;
                        let gy = -sk[i][1] * scale;
                        let gz = -sk[i][2] * scale;
                        let rx = gx * cyy + gz * sy;
                        let rz = -gx * sy + gz * cyy;
                        world[i * 3] = ox + rx;
                        world[i * 3 + 1] = oy + gy;
                        world[i * 3 + 2] = oz + rz;
                    }
                    let mut proj = vec![0.0f32; nv * 3];
                    ling_gpu::backend().project_points(&world, &cp, &mut proj);
                    let idx = &mesh.indices;
                    let nt = idx.len() / 3;
                    for t in 0..nt {
                        let ia = idx[t * 3] as usize;
                        let ib = idx[t * 3 + 1] as usize;
                        let ic = idx[t * 3 + 2] as usize;
                        if ia >= nv || ib >= nv || ic >= nv {
                            continue;
                        }
                        let (da, db, dc) = (proj[ia * 3 + 2], proj[ib * 3 + 2], proj[ic * 3 + 2]);
                        if (da + db + dc) / 3.0 <= near {
                            continue;
                        }
                        let col = {
                            let (ax, ay, az) = (world[ia * 3], world[ia * 3 + 1], world[ia * 3 + 2]);
                            let (bx, by, bz) = (world[ib * 3], world[ib * 3 + 1], world[ib * 3 + 2]);
                            let (px, py, pz) = (world[ic * 3], world[ic * 3 + 1], world[ic * 3 + 2]);
                            let (ux, uy, uz) = (bx - ax, by - ay, bz - az);
                            let (vx, vy, vz) = (px - ax, py - ay, pz - az);
                            let normal = [uy * vz - uz * vy, uz * vx - ux * vz, ux * vy - uy * vx];
                            let centroid =
                                [(ax + bx + px) / 3.0, (ay + by + py) / 3.0, (az + bz + pz) / 3.0];
                            if gfx.flat_shade {
                                base
                            } else {
                                crate::gfx::light::compute_lit_color(
                                    base, normal, centroid, &gfx.lights, ambient,
                                )
                            }
                        };
                        let depth = (da + db + dc) / 3.0;
                        let col = gfx.fog_apply(col, depth);
                        gfx.depth_queue.push_triangle_zv(
                            col,
                            proj[ia * 3], proj[ia * 3 + 1], da,
                            proj[ib * 3], proj[ib * 3 + 1], db,
                            proj[ic * 3], proj[ic * 3 + 1], dc,
                        );
                    }
                }
                return Ok(Value::Unit);
            },

            // ── draw_mesh(pos, idx, ox, oy, oz, scale, mode) ──
            //   Native batched triangle mesh. pos = flat [x,y,z,…], idx = flat tri indices.
            //   mode 0 = lit with current pen colour; 1 = per-face hue cycle.
            //   Vertices are batch-projected via ling-gpu (CPU fallback, or CUDA when the
            //   `cuda` feature is on); the per-triangle loop runs natively (not in the
            //   interpreter) so dense meshes (imported glTF, grids) stay fast.
            "draw_mesh" | "วาดเมช" | "رسم_مش" | "ارسم_شبكة" | "צייר_רשת" | "میش_کھینچو" | "dessiner_maillage" | "netz_zeichnen" | "рисовать_меш" => {
                let pos = match args.first() {
                    Some(Value::List(v)) => v,
                    _ => return Ok(Value::Unit),
                };
                let idx = match args.get(1) {
                    Some(Value::List(v)) => v,
                    _ => return Ok(Value::Unit),
                };
                let ox = self.arg_num(&args, 2, 0.0)? as f32;
                let oy = self.arg_num(&args, 3, 0.0)? as f32;
                let oz = self.arg_num(&args, 4, 0.0)? as f32;
                let scale = self.arg_num(&args, 5, 1.0)? as f32;
                let mode = self.arg_num(&args, 6, 0.0)? as i64;
                let nv = pos.len() / 3;
                if nv == 0 {
                    return Ok(Value::Unit);
                }
                let mut world = vec![0.0f32; nv * 3];
                for i in 0..nv {
                    world[i * 3] = ox + self.to_number(&pos[i * 3]).unwrap_or(0.0) as f32 * scale;
                    world[i * 3 + 1] =
                        oy + self.to_number(&pos[i * 3 + 1]).unwrap_or(0.0) as f32 * scale;
                    world[i * 3 + 2] =
                        oz + self.to_number(&pos[i * 3 + 2]).unwrap_or(0.0) as f32 * scale;
                }
                let mut gfx = self.gfx.borrow_mut();
                let cp = {
                    let c = &gfx.camera;
                    ling_gpu::CameraParams {
                        cry: c.cry,
                        sry: c.sry,
                        crx: c.crx,
                        srx: c.srx,
                        cx: c.cx,
                        cy: c.cy,
                        focal: c.focal,
                        zdist: c.zdist,
                        tx: c.tx,
                        ty: c.ty,
                        tz: c.tz,
                    }
                };
                let near = -gfx.camera.zdist + 0.02;
                let base = gfx.color;
                let ambient = gfx.ambient;
                let mut proj = vec![0.0f32; nv * 3]; // (sx, sy, depth) per vertex
                ling_gpu::backend().project_points(&world, &cp, &mut proj);
                let nt = idx.len() / 3;
                for t in 0..nt {
                    let ia = self.to_number(&idx[t * 3]).unwrap_or(0.0) as usize;
                    let ib = self.to_number(&idx[t * 3 + 1]).unwrap_or(0.0) as usize;
                    let ic = self.to_number(&idx[t * 3 + 2]).unwrap_or(0.0) as usize;
                    if ia >= nv || ib >= nv || ic >= nv {
                        continue;
                    }
                    let (da, db, dc) = (proj[ia * 3 + 2], proj[ib * 3 + 2], proj[ic * 3 + 2]);
                    if (da + db + dc) / 3.0 <= near {
                        continue;
                    } // near-plane cull (centroid)
                    let col = if mode == 1 {
                        let h = t as f32 * 0.6;
                        let r = ((h.sin() * 0.5 + 0.5) * 150.0 + 55.0) as u32;
                        let g = (((h + 2.094).sin() * 0.5 + 0.5) * 150.0 + 55.0) as u32;
                        let b = (((h + 4.189).sin() * 0.5 + 0.5) * 150.0 + 55.0) as u32;
                        (r << 16) | (g << 8) | b
                    } else {
                        let (ax, ay, az) = (world[ia * 3], world[ia * 3 + 1], world[ia * 3 + 2]);
                        let (bx, by, bz) = (world[ib * 3], world[ib * 3 + 1], world[ib * 3 + 2]);
                        let (px, py, pz) = (world[ic * 3], world[ic * 3 + 1], world[ic * 3 + 2]);
                        let (ux, uy, uz) = (bx - ax, by - ay, bz - az);
                        let (vx, vy, vz) = (px - ax, py - ay, pz - az);
                        let normal = [uy * vz - uz * vy, uz * vx - ux * vz, ux * vy - uy * vx];
                        let centroid = [
                            (ax + bx + px) / 3.0,
                            (ay + by + py) / 3.0,
                            (az + bz + pz) / 3.0,
                        ];
                        if gfx.flat_shade {
                            base
                        } else {
                            crate::gfx::light::compute_lit_color(
                                base,
                                normal,
                                centroid,
                                &gfx.lights,
                                ambient,
                            )
                        }
                    };
                    let depth = (da + db + dc) / 3.0;
                    let col = gfx.fog_apply(col, depth);
                    // True per-vertex depth so the z-buffer resolves mesh
                    // self-occlusion (when depth_test is off, the flush ignores
                    // z and uses the screen x/y exactly as before).
                    gfx.depth_queue.push_triangle_zv(
                        col,
                        proj[ia * 3],
                        proj[ia * 3 + 1],
                        da,
                        proj[ib * 3],
                        proj[ib * 3 + 1],
                        db,
                        proj[ic * 3],
                        proj[ic * 3 + 1],
                        dc,
                    );
                }
                return Ok(Value::Unit);
            },

            // ── add_light(x, y, z, r, g, b, intensity, radius) ──
            // Adds a point light in world space.  r/g/b in [0..1].
            // radius == 0 → no distance falloff.
            "add_light" | "เพิ่มแสง" | "加灯" | "ライト追加" | "조명추가" | "افزودن_نور" | "أضف_ضوء" | "הוסף_אור" | "روشنی_شامل_کرو" | "ajouter_lumière" | "licht_hinzufügen" | "добавить_свет" =>
            {
                let x = self.arg_num(&args, 0, 0.0)? as f32;
                let y = self.arg_num(&args, 1, -3.0)? as f32;
                let z = self.arg_num(&args, 2, 3.0)? as f32;
                let mut r = self.arg_num(&args, 3, 1.0)? as f32;
                let mut g = self.arg_num(&args, 4, 1.0)? as f32;
                let mut b = self.arg_num(&args, 5, 1.0)? as f32;
                // Forgive 0-255 colour values: if any channel is clearly > 1,
                // treat the triple as 0-255 and normalise. Keeps 0-1 callers exact.
                if r > 1.5 || g > 1.5 || b > 1.5 {
                    r /= 255.0;
                    g /= 255.0;
                    b /= 255.0;
                }
                let intensity = self.arg_num(&args, 6, 1.0)? as f32;
                let radius = self.arg_num(&args, 7, 0.0)? as f32;
                self.gfx
                    .borrow_mut()
                    .lights
                    .push(Light { x, y, z, r, g, b, intensity, radius });
                return Ok(Value::Unit);
            },

            // ── clear_lights() — remove all lights ──
            "clear_lights" | "ล้างแสง" | "清灯" | "ライト消去" | "조명초기화" | "پاک‌کردن_نورها" | "امسح_الأضواء" | "נקה_אורות" | "روشنیاں_صاف_کرو" | "effacer_lumières" | "lichter_löschen" | "очистить_свет" =>
            {
                self.gfx.borrow_mut().lights.clear();
                return Ok(Value::Unit);
            },

            // ── set_material(key, value) — configure LingMaterial field ──
            // Activates the material BSDF for subsequent polygon/triangle draws.
            // Keys (string): "albedo" "roughness" "metallic" "emission"
            //   "emission_strength" "specular" "specular_tint" "subsurface"
            //   "subsurface_color" "clearcoat" "clearcoat_roughness"
            //   "transmission" "ior" "iridescence" "sheen" "anisotropy"
            //   "anisotropy_angle" "toon_bands" "shadow_softness"
            //   "outline_px" "outline_color" "highlight_color"
            // Value: number (or packed 0xRRGGBB for colour fields)
            "set_material" | "ตั้งวัสดุ" | "设置材质" | "マテリアル設定" | "재질설정" | "تنظیم_متریال" | "عيّن_المادة" | "קבע_חומר" | "میٹریل_مقرر_کرو" =>
            {
                let key = self.arg_str(&args, 0, "");
                let val = self.arg_num(&args, 1, 0.0)?;
                let mut gfx = self.gfx.borrow_mut();
                let mat = gfx
                    .material
                    .get_or_insert_with(crate::gfx::LingMaterial::default);
                match key.as_str() {
                    "albedo" => mat.albedo = val as u32,
                    "roughness" => mat.roughness = val as f32,
                    "metallic" => mat.metallic = val as f32,
                    "emission" => mat.emission = val as u32,
                    "emission_strength" => mat.emission_strength = val as f32,
                    "specular" => mat.specular = val as f32,
                    "specular_tint" => mat.specular_tint = val as f32,
                    "subsurface" => mat.subsurface = val as f32,
                    "subsurface_color" => mat.subsurface_color = val as u32,
                    "clearcoat" => mat.clearcoat = val as f32,
                    "clearcoat_roughness" => mat.clearcoat_roughness = val as f32,
                    "transmission" => mat.transmission = val as f32,
                    "ior" => mat.ior = val as f32,
                    "iridescence" => mat.iridescence = val as f32,
                    "sheen" => mat.sheen = val as f32,
                    "anisotropy" => mat.anisotropy = val as f32,
                    "anisotropy_angle" => mat.anisotropy_angle = val as f32,
                    "toon_bands" => mat.toon_bands = val as u32,
                    "shadow_softness" => mat.shadow_softness = val as f32,
                    "outline_px" => mat.outline_px = val as f32,
                    "outline_color" => mat.outline_color = val as u32,
                    "highlight_color" => mat.highlight_color = val as u32,
                    _ => {},
                }
                return Ok(Value::Unit);
            },

            // ── reset_material() — disable material override ──
            // After this call, draws use the legacy compute_lit_color_linear path.
            "reset_material" | "รีเซ็ตวัสดุ" | "重置材质" | "マテリアルリセット" | "재질초기화" | "بازنشانی_متریال" | "أعد_ضبط_المادة" | "אפס_חומר" | "میٹریل_ری_سیٹ" =>
            {
                self.gfx.borrow_mut().material = None;
                return Ok(Value::Unit);
            },

            // ── toon_outlines(thickness, color, threshold) ──
            // Enable vector-smooth ink outlines on depth discontinuities.
            //   thickness  — ink-line half-width in pixels (0 = off, 1.5 = anime default)
            //   color      — 0xRRGGBB ink colour (default black = 0)
            //   threshold  — depth delta that triggers an edge (0.05 recommended)
            "toon_outlines"
            | "ตั้งเส้นขอบการ์ตูน"
            | "卡通轮廓"
            | "トゥーンアウトライン"
            | "툰아웃라인" | "خطوط_کارتونی" | "حدود_كرتونية" | "קווי_מתאר_מצוירים" | "ٹون_آؤٹ_لائنز" => {
                let px = self.arg_num(&args, 0, 0.0)? as f32;
                let color = self.arg_num(&args, 1, 0.0)? as u32;
                let thresh = self.arg_num(&args, 2, 0.05)? as f32;
                let mut gfx = self.gfx.borrow_mut();
                gfx.toon.outline_px = px;
                gfx.toon.outline_color = color;
                gfx.toon.outline_thresh = thresh;
                return Ok(Value::Unit);
            },

            // ── tone_stop(t, value) ──
            // Add a stop to the tone ramp.
            //   t      — input luminance position [0..1]
            //   value  — output brightness [0..1]
            // Stops are automatically sorted; call tone_ramp_reset() first to clear.
            "tone_stop" | "ตั้งจุดโทน" | "色调停止" | "トーンストップ" | "톤스톱" | "نقطه_توقف_تن_رنگ" | "نقطة_توقف_اللون" | "נקודת_עצירת_גוון" | "ٹون_اسٹاپ" =>
            {
                let t = self.arg_num(&args, 0, 0.0)? as f32;
                let val = self.arg_num(&args, 1, 1.0)? as f32;
                let mut gfx = self.gfx.borrow_mut();
                gfx.toon.ramp.stops.push(crate::gfx::toon::ToneStop {
                    t: t.clamp(0.0, 1.0),
                    value: val.clamp(0.0, 1.0),
                });
                gfx.toon
                    .ramp
                    .stops
                    .sort_by(|a, b| a.t.partial_cmp(&b.t).unwrap_or(std::cmp::Ordering::Equal));
                return Ok(Value::Unit);
            },

            // ── tone_smooth(enabled) ──
            // 0 = hard cel snap between stops (default); 1 = smooth gradient lerp.
            "tone_smooth" | "ตั้งโทนนุ่ม" | "色调平滑" | "トーンスムーズ" | "톤스무스" | "تن_رنگ_نرم" | "تدرج_لون_ناعم" | "גוון_חלק" | "ٹون_ہموار" =>
            {
                let v = self.arg_num(&args, 0, 0.0)? as f32;
                self.gfx.borrow_mut().toon.ramp.smooth = v > 0.5;
                return Ok(Value::Unit);
            },

            // ── tone_bezier(y1, y2) ──
            // Apply a cubic Bézier remap to the input luminance before stop lookup.
            //   y1, y2 — control-point y-values (identity: y1=0.333 y2=0.667)
            //   0 args or tone_bezier(0, 0)  → ease-in (shadow-heavy)
            //   tone_bezier(1, 1)            → ease-out (highlight-heavy)
            //   tone_bezier(0.1, 0.9)        → S-curve (smooth both ends)
            //   tone_bezier_off()            → disable (back to linear)
            "tone_bezier" | "ตั้งโทนเบซิเยร์" | "色调贝塞尔" | "トーンベジェ" | "톤베지어" | "تن_رنگ_بزیه" | "تدرج_لون_بيزيه" | "גוון_בזייה" | "بیزیئر_ٹون" =>
            {
                let y1 = self.arg_num(&args, 0, 1.0 / 3.0)? as f32;
                let y2 = self.arg_num(&args, 1, 2.0 / 3.0)? as f32;
                self.gfx.borrow_mut().toon.ramp.bezier = Some([y1, y2]);
                return Ok(Value::Unit);
            },

            // ── tone_bezier_off() — disable Bézier remap ──
            "tone_bezier_off"
            | "ปิดโทนเบซิเยร์"
            | "关闭色调贝塞尔"
            | "トーンベジェオフ"
            | "톤베지어끄기" | "خاموش‌کردن_بزیه" | "إيقاف_تدرج_بيزيه" | "כבה_גוון_בזייה" | "بیزیئر_ٹون_بند" => {
                self.gfx.borrow_mut().toon.ramp.bezier = None;
                return Ok(Value::Unit);
            },

            // ── tone_ramp_reset() — restore default 3-band cel ramp ──
            "tone_ramp_reset"
            | "รีเซ็ตการไล่โทน"
            | "重置色调渐变"
            | "トーンランプリセット"
            | "톤램프리셋" | "بازنشانی_شیب_تن_رنگ" | "أعد_ضبط_تدرج_اللون" | "אפס_שיפוע_גוון" | "ٹون_ریمپ_ری_سیٹ" => {
                self.gfx.borrow_mut().toon.ramp = crate::gfx::toon::ToneRamp::default();
                return Ok(Value::Unit);
            },

            // ── tone_ramp_clear() — clear all stops (build your own ramp) ──
            "tone_ramp_clear"
            | "ล้างการไล่โทน"
            | "清除色调渐变"
            | "トーンランプクリア"
            | "톤램프클리어" | "پاک‌کردن_شیب_تن_رنگ" | "امسح_تدرج_اللون" | "נקה_שיפוע_גוון" | "ٹون_ریمپ_صاف" => {
                self.gfx.borrow_mut().toon.ramp.stops.clear();
                return Ok(Value::Unit);
            },

            // ── tone_soft(soft, sheen) — band-edge softness + highlight sheen ──
            //   soft  [0..1] — fraction of each band gap that blends smoothly
            //                  across the boundary (0 = crisp cel, ~0.3 = soft
            //                  Wind Waker shadow edges). Default 0.32.
            //   sheen [0..1] — bright pixels keep their smooth gradient instead
            //                  of being quantised (clean specular/rim sheen
            //                  rather than scratchy banded highlights). 0.65.
            "tone_soft" | "โทนขอบนุ่ม" | "色调柔边" | "トーンソフト" | "톤소프트" | "تن_رنگ_لبه‌نرم" | "تدرج_ناعم_الحواف" | "גוון_קצה_רך" | "نرم_کنارہ_ٹون" | "tonalité_douce" | "weicher_ton" | "мягкий_тон" => {
                let s = self.arg_num(&args, 0, 0.32)? as f32;
                let sh = self.arg_num(&args, 1, 0.65)? as f32;
                let mut gfx = self.gfx.borrow_mut();
                gfx.toon.ramp.soft = s.clamp(0.0, 1.0);
                gfx.toon.ramp.sheen = sh.clamp(0.0, 1.0);
                return Ok(Value::Unit);
            },

            // ── set_ssao(strength, radius_px, zrange) — ambient occlusion ──
            // Depth-buffer contact shading: soft darkening in corners/under
            // objects, computed half-res + smoothed (no grain). Needs
            // set_depth_test(1). strength 0 disables. Defaults (0.35, 6, 12).
            "set_ssao" | "ตั้งเงาสัมผัส" | "环境光遮蔽" | "アンビエントオクルージョン"
            | "앰비언트오클루전" | "تنظیم_انسداد_محیطی" | "عيّن_تظليل_محيطي" | "קבע_הצללה_סביבתית" | "ایس_ایس_اے_او_مقرر_کرو" | "définir_ssao" | "ssao_setzen" | "задать_ssao" => {
                let s = self.arg_num(&args, 0, 0.35)? as f32;
                let r = self.arg_num(&args, 1, 6.0)? as f32;
                let z = self.arg_num(&args, 2, 12.0)? as f32;
                let mut gfx = self.gfx.borrow_mut();
                gfx.toon.ao_strength = s.clamp(0.0, 1.0);
                gfx.toon.ao_radius = r.max(1.0);
                gfx.toon.ao_range = z.max(0.01);
                return Ok(Value::Unit);
            },

            // ── set_fxaa(on) — FXAA-lite screen-space edge anti-aliasing ──
            // Softens polygon stair-steps and ink-line jaggies over the whole
            // frame; flat fills are untouched. Applied last in the present
            // post-chain. (set_antialias smooths wireframe STROKES; this pass
            // smooths the composited IMAGE.)
            "set_fxaa" | "ลบรอยหยัก" | "屏幕抗锯齿" | "画面アンチエイリアス" | "화면안티앨리어싱" | "تنظیم_ضدلبه‌دندانه_سریع" | "عيّن_مضاد_التسنن_السريع" | "קבע_החלקת_מסך" | "ایف_ایکس_اے_اے_مقرر_کرو" | "définir_fxaa" | "fxaa_setzen" | "задать_fxaa" => {
                let on = self.arg_num(&args, 0, 1.0)? as i64 != 0;
                self.gfx.borrow_mut().toon.fxaa = on;
                return Ok(Value::Unit);
            },

            // ── set_bloom(strength, threshold) — soft HDR-style glow ──
            // Bright pixels (rim sheen, emissive, additive FX) bleed a soft
            // quarter-res glow — the "HDR material" feel for toon/vector art.
            // strength 0 disables; threshold = luminance cutoff [0..1].
            "set_bloom" | "ตั้งบลูม" | "泛光" | "ブルーム" | "블룸" | "تنظیم_درخشش" | "عيّن_التوهج" | "קבע_זוהר" | "بلوم_مقرر_کرو" | "définir_bloom" | "bloom_setzen" | "задать_блум" => {
                let s = self.arg_num(&args, 0, 0.45)? as f32;
                let t = self.arg_num(&args, 1, 0.74)? as f32;
                let mut gfx = self.gfx.borrow_mut();
                gfx.toon.bloom_strength = s.max(0.0);
                gfx.toon.bloom_thresh = t.clamp(0.0, 0.99);
                return Ok(Value::Unit);
            },

            // ── shadow_smooth(softness) [compat] → tone_smooth + tone_bezier ──
            // Deprecated: use tone_smooth + tone_bezier instead.
            "shadow_smooth" | "ตั้งเงานุ่ม" | "柔化阴影" | "影ソフト" | "그림자부드럽게" | "سایه_نرم" | "ظل_ناعم" | "צל_חלק" | "نرم_سایہ" =>
            {
                let s = self.arg_num(&args, 0, 0.0)? as f32;
                let mut gfx = self.gfx.borrow_mut();
                gfx.toon.ramp.smooth = s > 0.05;
                if s > 0.05 {
                    let y1 = (0.333 + s * 0.2).clamp(0.0, 1.0);
                    let y2 = (0.667 - s * 0.2).clamp(0.0, 1.0);
                    gfx.toon.ramp.bezier = Some([y1, y2]);
                } else {
                    gfx.toon.ramp.bezier = None;
                }
                return Ok(Value::Unit);
            },

            // ── toon_highlight [compat] — no-op, use tone_stop instead ──
            "toon_highlight"
            | "ตั้งไฮไลท์การ์ตูน"
            | "卡通高光"
            | "トゥーンハイライト"
            | "툰하이라이트" | "هایلایت_کارتونی" | "إبراز_كرتوني" | "הדגשה_מצוירת" | "ٹون_ہائی_لائٹ" => {
                // Remap as a lit-band brightness boost: adds a stop near the highlight threshold.
                let _strength = self.arg_num(&args, 0, 0.0)? as f32;
                let _thresh = self.arg_num(&args, 2, 0.78)? as f32;
                // No-op: configure via tone_stop() for precise control.
                return Ok(Value::Unit);
            },

            // ── set_ambient(v) — ambient light level [0..1] ──
            "set_ambient" | "ตั้งแสงรอบข้าง" | "环境光" | "環境光設定" | "환경광설정" | "تنظیم_نور_محیطی" | "عيّن_الإضاءة_المحيطة" | "קבע_תאורה_סביבתית" | "ماحولیاتی_روشنی_مقرر_کرو" | "définir_ambiante" | "umgebungslicht_setzen" | "задать_фон" =>
            {
                let v = self.arg_num(&args, 0, 0.15)? as f32;
                self.gfx.borrow_mut().ambient = v;
                return Ok(Value::Unit);
            },

            // ── set_fog(r,g,b, start, end) — distance fog toward (r,g,b).
            //    triangles/lines fade from `start`..`end` camera depth. end<=0 = off.
            "set_fog" | "ตั้งหมอก" | "雾" | "霧設定" | "안개설정" | "تنظیم_مه" | "عيّن_الضباب" | "קבע_ערפל" | "دھند_مقرر_کرو" => {
                let r = self.arg_num(&args, 0, 0.0)?.clamp(0.0, 255.0) as u32;
                let g = self.arg_num(&args, 1, 0.0)?.clamp(0.0, 255.0) as u32;
                let b = self.arg_num(&args, 2, 0.0)?.clamp(0.0, 255.0) as u32;
                let start = self.arg_num(&args, 3, 0.0)? as f32;
                let end = self.arg_num(&args, 4, 0.0)? as f32;
                let mut gfx = self.gfx.borrow_mut();
                gfx.fog_color = (r << 16) | (g << 8) | b;
                gfx.fog_start = start;
                gfx.fog_end = end;
                return Ok(Value::Unit);
            },

            // ── วาดสามเหลี่ยม3มิติ(ax,ay,az, bx,by,bz, cx,cy,cz) ──
            // Computes lighting from world-space normal + active lights (cel shading),
            // projects via the stored camera, and pushes to the depth queue.
            "วาดสามเหลี่ยม3มิติ" | "draw_triangle_3d" | "triangle3d" | "رسم_مثلث_سه‌بعدی" | "ارسم_مثلثا_ثلاثي_الأبعاد" | "צייר_משולש_תלת_ממדי" | "تھری_ڈی_مثلث_کھینچو" =>
            {
                let ax = self.arg_num(&args, 0, 0.0)? as f32;
                let ay = self.arg_num(&args, 1, 0.0)? as f32;
                let az = self.arg_num(&args, 2, 0.0)? as f32;
                let bx = self.arg_num(&args, 3, 0.0)? as f32;
                let by = self.arg_num(&args, 4, 0.0)? as f32;
                let bz = self.arg_num(&args, 5, 0.0)? as f32;
                let cx = self.arg_num(&args, 6, 0.0)? as f32;
                let cy = self.arg_num(&args, 7, 0.0)? as f32;
                let cz = self.arg_num(&args, 8, 0.0)? as f32;

                let mut gfx = self.gfx.borrow_mut();

                // Mesh capture: record raw local coords + pen colour, skip submit.
                if gfx.mesh_capture.is_some() {
                    let col = gfx.color;
                    gfx.mesh_capture
                        .as_mut()
                        .unwrap()
                        .push(([ax, ay, az, bx, by, bz, cx, cy, cz], col));
                    return Ok(Value::Unit);
                }

                gfx.submit_triangle(ax, ay, az, bx, by, bz, cx, cy, cz);
                return Ok(Value::Unit);
            },

            // ── เริ่มอบเมช() — begin capturing 3-D triangles into a display list ──
            "เริ่มอบเมช" | "mesh_bake_begin" | "شروع_پخت_مش" | "ابدأ_خبز_الشبكة" | "התחל_אפיית_רשת" | "میش_بیک_شروع" => {
                self.gfx.borrow_mut().mesh_capture = Some(Vec::new());
                return Ok(Value::Unit);
            },

            // ── เมชแคชรับ(key) — keyed display-list cache lookup (-1 = miss) ──
            "เมชแคชรับ" | "mesh_cache_get" | "دریافت_کش_مش" | "اجلب_مخبأ_الشبكة" | "קבל_מטמון_רשת" | "میش_کیش_حاصل_کرو" => {
                let key = self.arg_num(&args, 0, 0.0)? as i64;
                let h = self.gfx.borrow().mesh_cache.get(&key).copied();
                return Ok(Value::Number(h.map(|x| x as f64).unwrap_or(-1.0)));
            },

            // ── เมชแคชตั้ง(key, handle) — store a baked mesh under key (bounded) ──
            "เมชแคชตั้ง" | "mesh_cache_put" | "ذخیره_در_کش_مش" | "ضع_في_مخبأ_الشبكة" | "שמור_במטמון_רשת" | "میش_کیش_رکھو" => {
                let key = self.arg_num(&args, 0, 0.0)? as i64;
                let h = self.arg_num(&args, 1, 0.0)? as usize;
                let mut gfx = self.gfx.borrow_mut();
                const CAP: usize = 256;
                if gfx.mesh_cache.len() >= CAP {
                    let evict: Vec<usize> = gfx.mesh_cache.values().copied().collect();
                    gfx.mesh_cache.clear();
                    for id in evict {
                        if id < gfx.meshes.len() {
                            gfx.meshes[id].clear();
                            gfx.mesh_free.push(id);
                        }
                    }
                }
                gfx.mesh_cache.insert(key, h);
                return Ok(Value::Unit);
            },

            // ── เมชแคชล้าง() — drop the keyed cache (e.g. on level change) ──
            "เมชแคชล้าง" | "mesh_cache_clear" | "پاک‌کردن_کش_مش" | "امسح_مخبأ_الشبكة" | "נקה_מטמון_רשת" | "میش_کیش_صاف" => {
                let mut gfx = self.gfx.borrow_mut();
                let evict: Vec<usize> = gfx.mesh_cache.values().copied().collect();
                gfx.mesh_cache.clear();
                for id in evict {
                    if id < gfx.meshes.len() {
                        gfx.meshes[id].clear();
                        gfx.mesh_free.push(id);
                    }
                }
                return Ok(Value::Unit);
            },

            // ── จบอบเมช() — bake captured triangles, return mesh handle ──
            "จบอบเมช" | "mesh_bake_end" | "پایان_پخت_مش" | "أنهِ_خبز_الشبكة" | "סיים_אפיית_רשת" | "میش_بیک_ختم" => {
                let mut gfx = self.gfx.borrow_mut();
                let tris = gfx.mesh_capture.take().unwrap_or_default();
                let id = gfx.mesh_register(tris);
                return Ok(Value::Number(id as f64));
            },

            // ── วาดอบเมช[สี](id, ox,oy,oz, rx,ry,rz, ux,uy,uz, s) — draw a baked mesh ──
            //   วาดอบเมช: current pen colour (tinted glyphs)
            //   วาดอบเมชสี: per-triangle baked colour (multi-colour models)
            "วาดอบเมช" | "mesh_bake_draw" | "วาดอบเมชสี" | "mesh_bake_draw_col" | "رسم_مش_پخته" | "ارسم_شبكة_مخبوزة" | "צייר_רשת_אפויה" | "بیکڈ_میش_کھینچو" =>
            {
                let baked_col = matches!(name, "วาดอบเมชสี" | "mesh_bake_draw_col");
                let id = self.arg_num(&args, 0, 0.0)? as usize;
                let ox = self.arg_num(&args, 1, 0.0)? as f32;
                let oy = self.arg_num(&args, 2, 0.0)? as f32;
                let oz = self.arg_num(&args, 3, 0.0)? as f32;
                let rx = self.arg_num(&args, 4, 1.0)? as f32;
                let ry = self.arg_num(&args, 5, 0.0)? as f32;
                let rz = self.arg_num(&args, 6, 0.0)? as f32;
                let ux = self.arg_num(&args, 7, 0.0)? as f32;
                let uy = self.arg_num(&args, 8, 1.0)? as f32;
                let uz = self.arg_num(&args, 9, 0.0)? as f32;
                let s = self.arg_num(&args, 10, 1.0)? as f32;
                self.gfx
                    .borrow_mut()
                    .mesh_draw(id, ox, oy, oz, rx, ry, rz, ux, uy, uz, s, baked_col);
                return Ok(Value::Unit);
            },

            // ── draw_quad_3d / draw_pent_3d / draw_hex_3d / draw_polygon_3d ──
            // Fan-triangulate convex n-gons.  Lighting, near-plane clip, fog, and
            // Gouraud shading all mirror draw_triangle_3d exactly.
            "draw_quad_3d"
            | "quad3d"
            | "วาดสี่เหลี่ยม3มิติ"
            | "draw_pent_3d"
            | "pent3d"
            | "วาดห้าเหลี่ยม3มิติ"
            | "draw_hex_3d"
            | "hex3d"
            | "วาดหกเหลี่ยม3มิติ"
            | "draw_polygon_3d"
            | "polygon3d"
            | "วาดรูปหลายเหลี่ยม3มิติ" | "رسم_چهارضلعی_سه‌بعدی" | "ارسم_رباعيا_ثلاثي_الأبعاد" | "צייר_מרובע_תלת_ממדי" | "تھری_ڈی_چوکور_کھینچو" => {
                // Collect (wx, wy, wz) triples from args or list
                let mut wxs: [f32; 8] = [0.0; 8];
                let mut wys: [f32; 8] = [0.0; 8];
                let mut wzs: [f32; 8] = [0.0; 8];
                let n_verts;

                if args.len() == 1 {
                    // draw_polygon_3d([x0,y0,z0, x1,y1,z1, ...])
                    let list = match &args[0] {
                        Value::List(l) => l.clone(),
                        _ => {
                            return Err(EvalErr::from("draw_polygon_3d: expected list".to_string()))
                        },
                    };
                    let coords: Vec<f32> = list
                        .iter()
                        .map(|v| match v {
                            Value::Number(n) => *n as f32,
                            _ => 0.0,
                        })
                        .collect();
                    n_verts = (coords.len() / 3).min(8);
                    for i in 0..n_verts {
                        wxs[i] = coords[i * 3];
                        wys[i] = coords[i * 3 + 1];
                        wzs[i] = coords[i * 3 + 2];
                    }
                } else {
                    // draw_quad/pent/hex_3d(x0,y0,z0, x1,y1,z1, ...)
                    n_verts = (args.len() / 3).min(8);
                    for i in 0..n_verts {
                        wxs[i] = self.arg_num(&args, i * 3, 0.0)? as f32;
                        wys[i] = self.arg_num(&args, i * 3 + 1, 0.0)? as f32;
                        wzs[i] = self.arg_num(&args, i * 3 + 2, 0.0)? as f32;
                    }
                }
                if n_verts < 3 {
                    return Ok(Value::Unit);
                }

                let mut gfx = self.gfx.borrow_mut();

                // Mesh capture: fan-triangulate and record raw local coords +
                // pen colour, exactly like วาดสามเหลี่ยม3มิติ. Quads used to
                // fall through here and bake EMPTY display lists — the 3-D
                // glyph fonts (letter pickups) are built from draw_quad_3d,
                // which made every baked glyph invisible.
                if gfx.mesh_capture.is_some() {
                    let col = gfx.color;
                    let cap = gfx.mesh_capture.as_mut().unwrap();
                    for i in 1..n_verts - 1 {
                        cap.push((
                            [
                                wxs[0], wys[0], wzs[0],
                                wxs[i], wys[i], wzs[i],
                                wxs[i + 1], wys[i + 1], wzs[i + 1],
                            ],
                            col,
                        ));
                    }
                    return Ok(Value::Unit);
                }

                // Face normal from first triangle of the fan
                let normal = crate::gfx::poly::face_normal(
                    wxs[0], wys[0], wzs[0], wxs[1], wys[1], wzs[1], wxs[2], wys[2], wzs[2],
                );

                // Per-vertex lit colours
                let mut wcs: [u32; 8] = [0; 8];
                if gfx.flat_shade {
                    let c = gfx.color;
                    for wc in wcs.iter_mut().take(n_verts) {
                        *wc = c;
                    }
                } else if let Some(ref mat) = gfx.material.clone() {
                    let cam = [gfx.camera.cx, gfx.camera.cy, gfx.camera.zdist];
                    let lights: Vec<_> = gfx.lights.clone();
                    let ambient = gfx.ambient;
                    for i in 0..n_verts {
                        let v = [wxs[i], wys[i], wzs[i]];
                        let vd = [cam[0] - v[0], cam[1] - v[1], cam[2] - v[2]];
                        wcs[i] = crate::gfx::material::shade(mat, normal, vd, v, &lights, ambient);
                    }
                } else {
                    let base = gfx.color;
                    let lights: Vec<_> = gfx.lights.clone();
                    let ambient = gfx.ambient;
                    for i in 0..n_verts {
                        wcs[i] = crate::gfx::light::compute_lit_color_linear(
                            base,
                            normal,
                            [wxs[i], wys[i], wzs[i]],
                            &lights,
                            ambient,
                        );
                    }
                }

                // Near-plane clip (Sutherland-Hodgman per vertex)
                let near = -gfx.camera.zdist + 0.05;
                let mut clip_in: [(f32, f32, f32, f32, u32); crate::gfx::poly::MAX_CLIP_VERTS] =
                    [(0.0, 0.0, 0.0, 0.0, 0); crate::gfx::poly::MAX_CLIP_VERTS];
                for i in 0..n_verts {
                    let d = gfx.camera.depth(wxs[i], wys[i], wzs[i]);
                    clip_in[i] = (wxs[i], wys[i], wzs[i], d, wcs[i]);
                }
                let mut clip_out: [(f32, f32, f32, f32, u32); crate::gfx::poly::MAX_CLIP_VERTS] =
                    [(0.0, 0.0, 0.0, 0.0, 0); crate::gfx::poly::MAX_CLIP_VERTS];
                let pn = crate::gfx::poly::clip_near(&clip_in, n_verts, near, &mut clip_out);
                if pn < 3 {
                    return Ok(Value::Unit);
                }

                // Project + fog
                let mut proj: [(f32, f32, f32, u32); crate::gfx::poly::MAX_CLIP_VERTS] =
                    [(0.0, 0.0, 0.0, 0); crate::gfx::poly::MAX_CLIP_VERTS];
                for i in 0..pn {
                    let (sx, sy, sz) =
                        gfx.camera
                            .project(clip_out[i].0, clip_out[i].1, clip_out[i].2);
                    let fc = gfx.fog_apply(clip_out[i].4, sz);
                    proj[i] = (sx, sy, sz, fc);
                }

                // Fan-triangulate and push
                let unlit = gfx.flat_shade;
                crate::gfx::poly::fan_emit_proj(
                    &proj,
                    pn,
                    |x0, y0, z0, c0, x1, y1, z1, c1, x2, y2, z2, c2| {
                        gfx.depth_queue.push_triangle_g_zv(
                            x0, y0, z0, c0, x1, y1, z1, c1, x2, y2, z2, c2, 3, unlit,
                        );
                    },
                );
                return Ok(Value::Unit);
            },

            // ── วาดเส้น3มิติ(ax,ay,az, bx,by,bz) ──
            // Projects two world-space points via the stored camera and pushes
            // a line to the depth queue.
            "วาดเส้น3มิติ" | "draw_line_3d" | "line3d" | "画3D线" | "3D線描く" | "3D선그리기" | "رسم_خط_سه‌بعدی" | "ارسم_خطا_ثلاثي_الأبعاد" | "צייר_קו_תלת_ממדי" | "تھری_ڈی_لکیر_کھینچو" =>
            {
                let ax = self.arg_num(&args, 0, 0.0)? as f32;
                let ay = self.arg_num(&args, 1, 0.0)? as f32;
                let az = self.arg_num(&args, 2, 0.0)? as f32;
                let bx = self.arg_num(&args, 3, 0.0)? as f32;
                let by = self.arg_num(&args, 4, 0.0)? as f32;
                let bz = self.arg_num(&args, 5, 0.0)? as f32;

                let mut gfx = self.gfx.borrow_mut();
                let color = gfx.color;
                // Near-plane clip in 3-D before perspective divide
                let near = -gfx.camera.zdist + 0.05;
                let mut lax = ax;
                let mut lay = ay;
                let mut laz = az;
                let mut lbx = bx;
                let mut lby = by;
                let mut lbz = bz;
                let da_raw = gfx.camera.depth(lax, lay, laz);
                let db_raw = gfx.camera.depth(lbx, lby, lbz);
                if da_raw <= near && db_raw <= near {
                    return Ok(Value::Unit);
                }
                if da_raw <= near {
                    let t = (near - da_raw) / (db_raw - da_raw);
                    lax += t * (lbx - lax);
                    lay += t * (lby - lay);
                    laz += t * (lbz - laz);
                } else if db_raw <= near {
                    let t = (near - da_raw) / (db_raw - da_raw);
                    lbx = lax + t * (lbx - lax);
                    lby = lay + t * (lby - lay);
                    lbz = laz + t * (lbz - laz);
                }
                // Shared-edge dedup: skip if this world-space edge was already queued.
                if !gfx.edge_set.try_insert(lax, lay, laz, lbx, lby, lbz) {
                    return Ok(Value::Unit);
                }
                let (sax, say, da) = gfx.camera.project(lax, lay, laz);
                let (sbx, sby, db) = gfx.camera.project(lbx, lby, lbz);
                let depth = (da + db) / 2.0;
                let color = gfx.fog_apply(color, depth);
                gfx.depth_queue.push_line(depth, color, sax, say, sbx, sby);
                return Ok(Value::Unit);
            },

            // orb_shell(cx,cy,cz, radius, rot_y, rot_x, density, r,g,b)
            //   A single trippy, grayscale, depth-faded vector pattern wound around
            //   a sphere — two families of interleaved spherical spirals (a guilloché
            //   weave), NOT a lat/long cage. Each segment's brightness follows its
            //   facing (front bright, back dim), so it reads as a translucent
            //   grayscale "texture" with alpha rather than a hard wireframe; the
            //   inner marble shows through. `rot_y`/`rot_x` roll the texture around
            //   the orb; `density` = spirals per winding direction. r,g,b tint it
            //   (pass a gray like 230,230,230 for pure grayscale).
            #[cfg(not(target_arch = "wasm32"))]
            "orb_shell" | "球壳" | "オーブ殻" | "오브껍질" | "เปลือกทรงกลม" | "پوسته_کروی" | "قشرة_كروية" | "קליפת_כדור" | "کروی_خول" | "coque_orbe" | "orb_hülle" | "оболочка_сферы" =>
            {
                let cx = self.arg_num(&args, 0, 0.)? as f32;
                let cy = self.arg_num(&args, 1, 0.)? as f32;
                let cz = self.arg_num(&args, 2, 0.)? as f32;
                let radius = self.arg_num(&args, 3, 1.0)? as f32;
                let ry = self.arg_num(&args, 4, 0.)? as f32;
                let rx = self.arg_num(&args, 5, 0.)? as f32;
                let density = (self.arg_num(&args, 6, 10.)? as i32).clamp(1, 48);
                let tr = (self.arg_num(&args, 7, 230.)? as f32).clamp(0., 255.);
                let tg = (self.arg_num(&args, 8, 230.)? as f32).clamp(0., 255.);
                let tb = (self.arg_num(&args, 9, 235.)? as f32).clamp(0., 255.);
                let (cyr, syr) = (ry.cos(), ry.sin());
                let (cxr, sxr) = (rx.cos(), rx.sin());
                let tau = std::f32::consts::TAU;
                let pi = std::f32::consts::PI;
                let turns = 6.0_f32; // how many times each spiral wraps pole→pole
                let nseg = 96; // segments per spiral (smoothness)
                let inv_r = if radius.abs() > 1e-5 {
                    1.0 / radius
                } else {
                    0.0
                };
                // a point along a spiral (param u 0..1, start angle theta0, winding dir),
                // spun by ry/rx — returns (world point, facing 0..1 where 1 = toward camera)
                let pt = |u: f32, theta0: f32, dir: f32| -> ([f32; 3], f32) {
                    let phi = pi * u; // 0..pi  (north → south)
                    let th = dir * turns * tau * u + theta0;
                    let (mut x, y, mut z) = (
                        phi.sin() * th.cos() * radius,
                        phi.cos() * radius,
                        phi.sin() * th.sin() * radius,
                    );
                    let x1 = x * cyr + z * syr; // yaw about Y
                    let z1 = -x * syr + z * cyr;
                    x = x1;
                    z = z1;
                    let y2 = y * cxr - z * sxr; // pitch about X
                    let z2 = y * sxr + z * cxr;
                    // facing: camera sits at -zdist looking +z, so smaller z2 = nearer = brighter
                    let facing = (0.5 - 0.5 * z2 * inv_r).clamp(0.0, 1.0);
                    ([cx + x, cy + y2, cz + z2], facing)
                };
                let mut gfx = self.gfx.borrow_mut();
                let near = -gfx.camera.zdist + 0.05;
                // draw one segment (near-clipped) in a grayscale tint scaled by `lum`
                let seg = |gfx: &mut crate::gfx::GfxState, a: [f32; 3], b: [f32; 3], lum: f32| {
                    let (mut lax, mut lay, mut laz) = (a[0], a[1], a[2]);
                    let (mut lbx, mut lby, mut lbz) = (b[0], b[1], b[2]);
                    let da = gfx.camera.depth(lax, lay, laz);
                    let db = gfx.camera.depth(lbx, lby, lbz);
                    if da <= near && db <= near {
                        return;
                    }
                    if da <= near {
                        let t = (near - da) / (db - da);
                        lax += t * (lbx - lax);
                        lay += t * (lby - lay);
                        laz += t * (lbz - laz);
                    } else if db <= near {
                        let t = (near - da) / (db - da);
                        lbx = lax + t * (lbx - lax);
                        lby = lay + t * (lby - lay);
                        lbz = laz + t * (lbz - laz);
                    }
                    let (sax, say, da2) = gfx.camera.project(lax, lay, laz);
                    let (sbx, sby, db2) = gfx.camera.project(lbx, lby, lbz);
                    // grayscale-alpha: front-facing bright, back faded toward black
                    let l = (0.12 + 0.88 * lum).clamp(0.0, 1.0);
                    let cr = (tr * l) as u32;
                    let cg = (tg * l) as u32;
                    let cb = (tb * l) as u32;
                    let color = (cr << 16) | (cg << 8) | cb;
                    gfx.depth_queue
                        .push_line((da2 + db2) * 0.5, color, sax, say, sbx, sby);
                };
                // two opposite winding directions → a soft guilloché weave (not a cage)
                for &dir in &[1.0_f32, -1.0_f32] {
                    for s in 0..density {
                        let theta0 = s as f32 * tau / density as f32;
                        let mut prev = pt(0.0, theta0, dir);
                        for k in 1..=nseg {
                            let cur = pt(k as f32 / nseg as f32, theta0, dir);
                            seg(&mut gfx, prev.0, cur.0, (prev.1 + cur.1) * 0.5);
                            prev = cur;
                        }
                    }
                }
                return Ok(Value::Unit);
            },

            // orb_particles(cx,cy,cz, radius, count, t, r,g,b)
            //   Fills the VOLUME of a sphere with `count` swirling vector points —
            //   like motes suspended inside a snow-globe orb. Points are distributed
            //   uniformly through the ball, slowly tumble as a cloud + wobble
            //   individually over time `t`, and are depth-shaded (near = bright,
            //   far = dim) so the cloud has real volume. Additive, so it layers under
            //   a shell / over a liquid marble.
            #[cfg(not(target_arch = "wasm32"))]
            "orb_particles" | "球内粒子" | "オーブ粒子" | "오브입자" | "อนุภาคทรงกลม" | "ذرات_کروی" | "جسيمات_كروية" | "חלקיקי_כדור" | "کروی_ذرات" | "particules_orbe" | "orb_partikel" | "частицы_сферы" =>
            {
                let cx = self.arg_num(&args, 0, 0.)? as f32;
                let cy = self.arg_num(&args, 1, 0.)? as f32;
                let cz = self.arg_num(&args, 2, 0.)? as f32;
                let radius = self.arg_num(&args, 3, 1.0)? as f32;
                let count = (self.arg_num(&args, 4, 160.)? as i32).clamp(1, 4000);
                let t = self.arg_num(&args, 5, 0.)? as f32;
                let tr = (self.arg_num(&args, 6, 255.)? as f32).clamp(0., 255.);
                let tg = (self.arg_num(&args, 7, 255.)? as f32).clamp(0., 255.);
                let tb = (self.arg_num(&args, 8, 255.)? as f32).clamp(0., 255.);
                let inv_r = if radius.abs() > 1e-5 {
                    1.0 / radius
                } else {
                    0.0
                };
                // cheap deterministic hash → [0,1)
                let h = |mut x: u32| -> f32 {
                    x = x.wrapping_mul(747796405).wrapping_add(2891336453);
                    x = ((x >> ((x >> 28).wrapping_add(4))) ^ x).wrapping_mul(277803737);
                    (((x >> 22) ^ x) & 0xFFFFFF) as f32 / 16_777_216.0
                };
                let tau = std::f32::consts::TAU;
                // slow tumble of the whole cloud
                let (cyr, syr) = ((t * 0.5).cos(), (t * 0.5).sin());
                let (cxr, sxr) = ((t * 0.23).cos(), (t * 0.23).sin());
                let mut gfx = self.gfx.borrow_mut();
                let near = -gfx.camera.zdist + 0.05;
                let (sw, sh) = (gfx.width as i32, gfx.height as i32);
                for i in 0..count {
                    let i = i as u32;
                    // uniform-in-volume: r = cbrt(u) * radius; direction from two hashes
                    let u = h(i.wrapping_mul(3) + 1);
                    let rr = u.cbrt() * radius * (0.85 + 0.15 * (t * 1.3 + i as f32).sin()); // gentle pulse
                    let th = h(i.wrapping_mul(3) + 2) * tau + t * (0.3 + 0.5 * h(i * 7 + 5)); // per-mote orbit
                    let ph = (h(i.wrapping_mul(3) + 3) * 2.0 - 1.0).acos(); // uniform cos(phi)
                    let (mut x, y, mut z) = (
                        rr * ph.sin() * th.cos(),
                        rr * ph.cos(),
                        rr * ph.sin() * th.sin(),
                    );
                    // tumble the cloud (yaw then pitch)
                    let x1 = x * cyr + z * syr;
                    let z1 = -x * syr + z * cyr;
                    x = x1;
                    z = z1;
                    let y2 = y * cxr - z * sxr;
                    let z2 = y * sxr + z * cxr;
                    let (wx, wy, wz) = (cx + x, cy + y2, cz + z2);
                    if gfx.camera.depth(wx, wy, wz) <= near {
                        continue;
                    }
                    let (sx, sy, dep) = gfx.camera.project(wx, wy, wz);
                    let sxi = sx as i32;
                    let syi = sy as i32;
                    if sxi < 0 || syi < 0 || sxi >= sw || syi >= sh {
                        continue;
                    }
                    // depth-shade: nearer (smaller z2) = brighter
                    let facing = (0.5 - 0.5 * z2 * inv_r).clamp(0.15, 1.0);
                    let l = facing;
                    let cr = (tr * l) as u32;
                    let cg = (tg * l) as u32;
                    let cb = (tb * l) as u32;
                    let color = (cr << 16) | (cg << 8) | cb;
                    // a 1–2px dot (bigger when near) as a short segment in the depth queue
                    let len = if facing > 0.7 { 1.0 } else { 0.0 };
                    gfx.depth_queue.push_line(dep, color, sx, sy, sx + len, sy);
                }
                return Ok(Value::Unit);
            },

            // project_3d(x,y,z) -> [screen_x, screen_y, depth]; behind the camera
            // returns a sentinel ([-99999,-99999, depth]) so scripts can skip it.
            // Lets scripts place 2-D overlays (e.g. filled teardrop flames) onto 3-D points.
            "project_3d" | "投影3D" | "3D投影" | "3D투영" | "ฉาย3มิติ" | "فرافکنی_سه‌بعدی" | "إسقاط_ثلاثي_الأبعاد" | "הטלה_תלת_ממדית" | "تھری_ڈی_پروجیکشن" | "projeter_3d" | "projizieren_3d" | "проекция_3d" => {
                let x = self.arg_num(&args, 0, 0.0)? as f32;
                let y = self.arg_num(&args, 1, 0.0)? as f32;
                let z = self.arg_num(&args, 2, 0.0)? as f32;
                let gfx = self.gfx.borrow();
                let near = -gfx.camera.zdist + 0.05;
                let d = gfx.camera.depth(x, y, z);
                if d <= near {
                    return Ok(Value::List(Rc::new(vec![
                        Value::Number(-99999.0),
                        Value::Number(-99999.0),
                        Value::Number(d as f64),
                    ])));
                }
                let (sx, sy, depth) = gfx.camera.project(x, y, z);
                return Ok(Value::List(Rc::new(vec![
                    Value::Number(sx as f64),
                    Value::Number(sy as f64),
                    Value::Number(depth as f64),
                ])));
            },

            // mouse_ray() -> [ox,oy,oz, dx,dy,dz] — world-space ray from the eye
            // through the actual mouse cursor pixel, exact inverse of project_3d's
            // pipeline (translate → Y-rotate → X-rotate → perspective divide by
            // rz+zdist). Scripts previously marched a ray along the CENTRE-SCREEN
            // forward vector regardless of where the cursor was — accurate only by
            // coincidence when the cursor happened to sit near the crosshair.
            #[cfg(not(target_arch = "wasm32"))]
            "mouse_ray" => {
                let gfx = self.gfx.borrow();
                let (mx, my) = gfx
                    .window
                    .as_ref()
                    .and_then(|w| w.get_mouse_pos(minifb::MouseMode::Clamp))
                    .unwrap_or((gfx.camera.cx, gfx.camera.cy));
                let cam = &gfx.camera;
                // Eye-relative pinhole direction for this pixel, in rotation-space
                // (before undoing the Y-then-X rotation project() applied).
                let dcx = (mx - cam.cx) / cam.focal;
                let dcy = (my - cam.cy) / cam.focal;
                // Undo the X-rotation, then the Y-rotation (reverse of project()'s
                // forward order), on the direction vector (dcx, dcy, 1.0).
                let a_x = dcx;
                let a_y = cam.crx * dcy + cam.srx * 1.0;
                let a_z = 0.0 - cam.srx * dcy + cam.crx * 1.0;
                let dir_x = cam.cry * a_x + cam.sry * a_z;
                let dir_y = a_y;
                let dir_z = 0.0 - cam.sry * a_x + cam.cry * a_z;
                let dlen = (dir_x * dir_x + dir_y * dir_y + dir_z * dir_z)
                    .sqrt()
                    .max(1e-6);
                let (dir_x, dir_y, dir_z) = (dir_x / dlen, dir_y / dlen, dir_z / dlen);
                // Origin: the camera's rotation pivot (tx,ty,tz) — i.e. wherever
                // the script last put it with set_camera_pos, NOT the "true"
                // pinhole eye zdist further back. The pivot is what scripts
                // already keep clear of the ground (a ground-collision pull-in
                // loop is standard practice for orbit cameras); the true eye
                // would need its own separate ground clearance since zdist is
                // often large relative to a close-in camera distance, and
                // starting a ray underground makes it hit "ground" instantly
                // regardless of aim. The zdist offset only matters for the
                // near-field parallax, which a click-to-move ray (aimed at
                // terrain many units out) doesn't need.
                let ox = cam.tx;
                let oy = cam.ty;
                let oz = cam.tz;
                return Ok(Value::List(Rc::new(vec![
                    Value::Number(ox as f64),
                    Value::Number(oy as f64),
                    Value::Number(oz as f64),
                    Value::Number(dir_x as f64),
                    Value::Number(dir_y as f64),
                    Value::Number(dir_z as f64),
                ])));
            },
            #[cfg(target_arch = "wasm32")]
            "mouse_ray" => {
                let gfx = self.gfx.borrow();
                let mx = crate::gfx::wasm_mouse_x();
                let my = crate::gfx::wasm_mouse_y();
                let cam = &gfx.camera;
                let dcx = (mx - cam.cx) / cam.focal;
                let dcy = (my - cam.cy) / cam.focal;
                let a_x = dcx;
                let a_y = cam.crx * dcy + cam.srx * 1.0;
                let a_z = 0.0 - cam.srx * dcy + cam.crx * 1.0;
                let dir_x = cam.cry * a_x + cam.sry * a_z;
                let dir_y = a_y;
                let dir_z = 0.0 - cam.sry * a_x + cam.cry * a_z;
                let dlen = (dir_x * dir_x + dir_y * dir_y + dir_z * dir_z)
                    .sqrt()
                    .max(1e-6);
                let (dir_x, dir_y, dir_z) = (dir_x / dlen, dir_y / dlen, dir_z / dlen);
                let ox = cam.tx;
                let oy = cam.ty;
                let oz = cam.tz;
                return Ok(Value::List(Rc::new(vec![
                    Value::Number(ox as f64),
                    Value::Number(oy as f64),
                    Value::Number(oz as f64),
                    Value::Number(dir_x as f64),
                    Value::Number(dir_y as f64),
                    Value::Number(dir_z as f64),
                ])));
            },
            // draw_poly([x0,y0,x1,y1,…]) — filled 2-D polygon in the current colour,
            // honouring the blend mode (additive → translucent glow). Auto-closes.
            #[cfg(not(target_arch = "wasm32"))]
            "draw_poly" | "填充多边形" | "ポリゴン塗り" | "다각형채우기" | "เติมรูปหลายเหลี่ยม" | "رسم_چندضلعی" | "ارسم_مضلع" | "צייר_מצולע" | "کثیر_الاضلاع_کھینچو" | "dessiner_polygone" | "polygon_zeichnen" | "рисовать_полигон" =>
            {
                let mut pts: Vec<[f32; 2]> = Vec::new();
                if let Some(Value::List(v)) = args.first() {
                    let mut i = 0;
                    while i + 1 < v.len() {
                        let x = self.to_number(&v[i]).unwrap_or(0.0) as f32;
                        let y = self.to_number(&v[i + 1]).unwrap_or(0.0) as f32;
                        pts.push([x, y]);
                        i += 2;
                    }
                }
                if pts.len() >= 3 {
                    if pts[0] != pts[pts.len() - 1] {
                        let p0 = pts[0];
                        pts.push(p0);
                    } // close
                    let mut gfx = self.gfx.borrow_mut();
                    let (w, h, color, add) = (gfx.width, gfx.height, gfx.color, gfx.blend == 1);
                    crate::gfx::raster::fill_contours_aa(
                        &mut gfx.buffer,
                        w,
                        h,
                        color,
                        add,
                        std::slice::from_ref(&pts),
                    );
                }
                return Ok(Value::Unit);
            },

            // ══════════════════════════════════════════════════════════════════
            // VECTOR TEXTURE BUILTINS  (src/gfx/vtex.rs)
            // All patterns are depth-biased so they appear on top of surfaces.
            // Plane defined by: centre (cx,cy,cz) + U tangent + V tangent.
            // Last two args always: fr (frame f32), hue (phase offset f32).
            // ══════════════════════════════════════════════════════════════════

            // vtex_grid(cx,cy,cz, ux,uy,uz, vx,vy,vz, cols,rows, cw,ch, fr,hue)
            "vtex_grid" | "ลายตาราง" | "纹格" | "格子模様" | "격자무늬" | "الگوی_شبکه" | "نقش_شبكة" | "דוגמת_רשת" | "نقش_جالی" | "motif_grille" | "muster_gitter" | "узор_сетка" =>
            {
                let cx = self.arg_num(&args, 0, 0.)? as f32;
                let cy = self.arg_num(&args, 1, 0.)? as f32;
                let cz = self.arg_num(&args, 2, 0.)? as f32;
                let ux = self.arg_num(&args, 3, 1.)? as f32;
                let uy = self.arg_num(&args, 4, 0.)? as f32;
                let uz = self.arg_num(&args, 5, 0.)? as f32;
                let vx = self.arg_num(&args, 6, 0.)? as f32;
                let vy = self.arg_num(&args, 7, 0.)? as f32;
                let vz = self.arg_num(&args, 8, 1.)? as f32;
                let cols = self.arg_num(&args, 9, 10.)? as usize;
                let rows = self.arg_num(&args, 10, 10.)? as usize;
                let cw = self.arg_num(&args, 11, 1.)? as f32;
                let ch = self.arg_num(&args, 12, 1.)? as f32;
                let fr = self.arg_num(&args, 13, 0.)? as f32;
                let hue = self.arg_num(&args, 14, 0.)? as f32;
                let mut gfx = self.gfx.borrow_mut();
                let cam = gfx.camera.clone();
                crate::gfx::vtex::draw_grid(
                    &mut gfx.depth_queue,
                    &cam,
                    cx,
                    cy,
                    cz,
                    ux,
                    uy,
                    uz,
                    vx,
                    vy,
                    vz,
                    cols,
                    rows,
                    cw,
                    ch,
                    fr,
                    hue,
                );
                return Ok(Value::Unit);
            },

            // vtex_rings(cx,cy,cz, ux,uy,uz, vx,vy,vz, n_rings,n_sides, max_r,twist, fr,hue)
            "vtex_rings" | "ลายวงซ้อน" | "纹环" | "同心円" | "동심원" | "الگوی_حلقه" | "نقش_حلقات" | "דוגמת_טבעות" | "نقش_حلقے" | "motif_anneaux" | "muster_ringe" | "узор_кольца" => {
                let cx = self.arg_num(&args, 0, 0.)? as f32;
                let cy = self.arg_num(&args, 1, 0.)? as f32;
                let cz = self.arg_num(&args, 2, 0.)? as f32;
                let ux = self.arg_num(&args, 3, 1.)? as f32;
                let uy = self.arg_num(&args, 4, 0.)? as f32;
                let uz = self.arg_num(&args, 5, 0.)? as f32;
                let vx = self.arg_num(&args, 6, 0.)? as f32;
                let vy = self.arg_num(&args, 7, 0.)? as f32;
                let vz = self.arg_num(&args, 8, 1.)? as f32;
                let nr = self.arg_num(&args, 9, 6.)? as usize;
                let ns = self.arg_num(&args, 10, 6.)? as usize;
                let mr = self.arg_num(&args, 11, 3.)? as f32;
                let tw = self.arg_num(&args, 12, 0.)? as f32;
                let fr = self.arg_num(&args, 13, 0.)? as f32;
                let hue = self.arg_num(&args, 14, 0.)? as f32;
                let mut gfx = self.gfx.borrow_mut();
                let cam = gfx.camera.clone();
                crate::gfx::vtex::draw_rings(
                    &mut gfx.depth_queue,
                    &cam,
                    cx,
                    cy,
                    cz,
                    ux,
                    uy,
                    uz,
                    vx,
                    vy,
                    vz,
                    nr,
                    ns,
                    mr,
                    tw,
                    fr,
                    hue,
                );
                return Ok(Value::Unit);
            },

            // vtex_star(cx,cy,cz, ux,uy,uz, vx,vy,vz, n_pts,r_out,r_in, rot_speed, fr,hue)
            "vtex_star" | "ลายดาว" | "纹星" | "星模様" | "별무늬" | "الگوی_ستاره" | "نقش_نجمة" | "דוגמת_כוכב" | "نقش_ستارہ" | "motif_étoile" | "muster_stern" | "узор_звезда" => {
                let cx = self.arg_num(&args, 0, 0.)? as f32;
                let cy = self.arg_num(&args, 1, 0.)? as f32;
                let cz = self.arg_num(&args, 2, 0.)? as f32;
                let ux = self.arg_num(&args, 3, 1.)? as f32;
                let uy = self.arg_num(&args, 4, 0.)? as f32;
                let uz = self.arg_num(&args, 5, 0.)? as f32;
                let vx = self.arg_num(&args, 6, 0.)? as f32;
                let vy = self.arg_num(&args, 7, 0.)? as f32;
                let vz = self.arg_num(&args, 8, 1.)? as f32;
                let np = self.arg_num(&args, 9, 6.)? as usize;
                let ro = self.arg_num(&args, 10, 2.)? as f32;
                let ri = self.arg_num(&args, 11, 1.)? as f32;
                let rs = self.arg_num(&args, 12, 0.01)? as f32;
                let fr = self.arg_num(&args, 13, 0.)? as f32;
                let hue = self.arg_num(&args, 14, 0.)? as f32;
                let mut gfx = self.gfx.borrow_mut();
                let cam = gfx.camera.clone();
                crate::gfx::vtex::draw_star(
                    &mut gfx.depth_queue,
                    &cam,
                    cx,
                    cy,
                    cz,
                    ux,
                    uy,
                    uz,
                    vx,
                    vy,
                    vz,
                    np,
                    ro,
                    ri,
                    rs,
                    fr,
                    hue,
                );
                return Ok(Value::Unit);
            },

            // vtex_spiral(cx,cy,cz, ux,uy,uz, vx,vy,vz, n_turns,max_r,steps, fr,hue)
            "vtex_spiral" | "ลายเกลียว" | "纹螺" | "螺旋" | "나선" | "الگوی_مارپیچ" | "نقش_حلزوني" | "דוגמת_ספירלה" | "نقش_سرپیچ" | "motif_spirale" | "muster_spirale" | "узор_спираль" => {
                let cx = self.arg_num(&args, 0, 0.)? as f32;
                let cy = self.arg_num(&args, 1, 0.)? as f32;
                let cz = self.arg_num(&args, 2, 0.)? as f32;
                let ux = self.arg_num(&args, 3, 1.)? as f32;
                let uy = self.arg_num(&args, 4, 0.)? as f32;
                let uz = self.arg_num(&args, 5, 0.)? as f32;
                let vx = self.arg_num(&args, 6, 0.)? as f32;
                let vy = self.arg_num(&args, 7, 0.)? as f32;
                let vz = self.arg_num(&args, 8, 1.)? as f32;
                let nt = self.arg_num(&args, 9, 3.)? as f32;
                let mr = self.arg_num(&args, 10, 3.)? as f32;
                let st = self.arg_num(&args, 11, 120.)? as usize;
                let fr = self.arg_num(&args, 12, 0.)? as f32;
                let hue = self.arg_num(&args, 13, 0.)? as f32;
                let mut gfx = self.gfx.borrow_mut();
                let cam = gfx.camera.clone();
                crate::gfx::vtex::draw_spiral(
                    &mut gfx.depth_queue,
                    &cam,
                    cx,
                    cy,
                    cz,
                    ux,
                    uy,
                    uz,
                    vx,
                    vy,
                    vz,
                    nt,
                    mr,
                    st,
                    fr,
                    hue,
                );
                return Ok(Value::Unit);
            },

            // vtex_flower(cx,cy,cz, ux,uy,uz, vx,vy,vz, radius,n_sides, fr,hue)
            "vtex_flower" | "ลายดอก" | "纹花" | "花模様" | "꽃무늬" | "الگوی_گل" | "نقش_زهرة" | "דוגמת_פרח" | "نقش_پھول" | "motif_fleur" | "muster_blume" | "узор_цветок" => {
                let cx = self.arg_num(&args, 0, 0.)? as f32;
                let cy = self.arg_num(&args, 1, 0.)? as f32;
                let cz = self.arg_num(&args, 2, 0.)? as f32;
                let ux = self.arg_num(&args, 3, 1.)? as f32;
                let uy = self.arg_num(&args, 4, 0.)? as f32;
                let uz = self.arg_num(&args, 5, 0.)? as f32;
                let vx = self.arg_num(&args, 6, 0.)? as f32;
                let vy = self.arg_num(&args, 7, 0.)? as f32;
                let vz = self.arg_num(&args, 8, 1.)? as f32;
                let r = self.arg_num(&args, 9, 1.)? as f32;
                let ns = self.arg_num(&args, 10, 24.)? as usize;
                let fr = self.arg_num(&args, 11, 0.)? as f32;
                let hue = self.arg_num(&args, 12, 0.)? as f32;
                let mut gfx = self.gfx.borrow_mut();
                let cam = gfx.camera.clone();
                crate::gfx::vtex::draw_flower(
                    &mut gfx.depth_queue,
                    &cam,
                    cx,
                    cy,
                    cz,
                    ux,
                    uy,
                    uz,
                    vx,
                    vy,
                    vz,
                    r,
                    ns,
                    fr,
                    hue,
                );
                return Ok(Value::Unit);
            },

            // vtex_letter_rain(cx,cy,cz, ux,uy,uz, vx,vy,vz, n_cols,n_vis, col_w,row_h, speed, fr,hue)
            "vtex_letter_rain" | "ลายอักษรไหล" | "纹字雨" | "文字雨" | "글자비" | "الگوی_باران_حروف" | "نقش_مطر_الحروف" | "דוגמת_גשם_אותיות" | "نقش_حروف_بارش" | "motif_pluie_lettres" | "muster_buchstabenregen" | "узор_дождь_букв" =>
            {
                let cx = self.arg_num(&args, 0, 0.)? as f32;
                let cy = self.arg_num(&args, 1, 0.)? as f32;
                let cz = self.arg_num(&args, 2, 0.)? as f32;
                let ux = self.arg_num(&args, 3, 1.)? as f32;
                let uy = self.arg_num(&args, 4, 0.)? as f32;
                let uz = self.arg_num(&args, 5, 0.)? as f32;
                let vx = self.arg_num(&args, 6, 0.)? as f32;
                let vy = self.arg_num(&args, 7, 0.)? as f32;
                let vz = self.arg_num(&args, 8, 1.)? as f32;
                let nc = self.arg_num(&args, 9, 16.)? as usize;
                let nv = self.arg_num(&args, 10, 14.)? as usize;
                let cw = self.arg_num(&args, 11, 0.65)? as f32;
                let rh = self.arg_num(&args, 12, 0.60)? as f32;
                let sp = self.arg_num(&args, 13, 0.025)? as f32;
                let fr = self.arg_num(&args, 14, 0.)? as f32;
                let hue = self.arg_num(&args, 15, 0.)? as f32;
                let mut gfx = self.gfx.borrow_mut();
                let cam = gfx.camera.clone();
                crate::gfx::vtex::draw_letter_rain(
                    &mut gfx.depth_queue,
                    &cam,
                    cx,
                    cy,
                    cz,
                    ux,
                    uy,
                    uz,
                    vx,
                    vy,
                    vz,
                    nc,
                    nv,
                    cw,
                    rh,
                    sp,
                    fr,
                    hue,
                );
                return Ok(Value::Unit);
            },

            // vtex_hyperbolic_uv(cx,cy,cz, ux,uy,uz, vx,vy,vz, max_r,n_circles,n_rays, fr,hue)
            "vtex_hyperbolic_uv" | "ลายไฮเพอร์โบลิก" | "纹曲面" | "双曲線" | "쌍곡선" | "الگوی_هذلولی" | "نقش_زائدي" | "דוגמת_היפרבולית" | "نقش_ہائپربولک" | "motif_uv_hyperbolique" | "muster_hyperbolische_uv" | "узор_гиперболический_uv" =>
            {
                let cx = self.arg_num(&args, 0, 0.)? as f32;
                let cy = self.arg_num(&args, 1, 0.)? as f32;
                let cz = self.arg_num(&args, 2, 0.)? as f32;
                let ux = self.arg_num(&args, 3, 1.)? as f32;
                let uy = self.arg_num(&args, 4, 0.)? as f32;
                let uz = self.arg_num(&args, 5, 0.)? as f32;
                let vx = self.arg_num(&args, 6, 0.)? as f32;
                let vy = self.arg_num(&args, 7, 0.)? as f32;
                let vz = self.arg_num(&args, 8, 1.)? as f32;
                let mr = self.arg_num(&args, 9, 5.)? as f32;
                let nc = self.arg_num(&args, 10, 12.)? as usize;
                let nr = self.arg_num(&args, 11, 18.)? as usize;
                let fr = self.arg_num(&args, 12, 0.)? as f32;
                let hue = self.arg_num(&args, 13, 0.)? as f32;
                let mut gfx = self.gfx.borrow_mut();
                let cam = gfx.camera.clone();
                crate::gfx::vtex::draw_hyperbolic_uv(
                    &mut gfx.depth_queue,
                    &cam,
                    cx,
                    cy,
                    cz,
                    ux,
                    uy,
                    uz,
                    vx,
                    vy,
                    vz,
                    mr,
                    nc,
                    nr,
                    fr,
                    hue,
                );
                return Ok(Value::Unit);
            },

            // vtex_halftone(cx,cy,cz, ux,uy,uz, vx,vy,vz, cols,rows, cell_w,cell_h, density, fr,hue)
            "vtex_halftone" | "ลายจุด" | "纹半调" | "網点模様" | "망점" | "الگوی_نیم‌تن" | "نقش_نصفي" | "דוגמת_חצי_גוון" | "نقش_ہاف_ٹون" | "motif_demi_ton" | "muster_halbton" | "узор_растр" => {
                let cx = self.arg_num(&args, 0, 0.)? as f32;
                let cy = self.arg_num(&args, 1, 0.)? as f32;
                let cz = self.arg_num(&args, 2, 0.)? as f32;
                let ux = self.arg_num(&args, 3, 1.)? as f32;
                let uy = self.arg_num(&args, 4, 0.)? as f32;
                let uz = self.arg_num(&args, 5, 0.)? as f32;
                let vx = self.arg_num(&args, 6, 0.)? as f32;
                let vy = self.arg_num(&args, 7, 0.)? as f32;
                let vz = self.arg_num(&args, 8, 1.)? as f32;
                let cols = self.arg_num(&args, 9, 16.)? as usize;
                let rows = self.arg_num(&args, 10, 12.)? as usize;
                let cw = self.arg_num(&args, 11, 0.5)? as f32;
                let ch = self.arg_num(&args, 12, 0.5)? as f32;
                let dens = self.arg_num(&args, 13, 0.4)? as f32;
                let fr = self.arg_num(&args, 14, 0.)? as f32;
                let hue = self.arg_num(&args, 15, 0.)? as f32;
                let mut gfx = self.gfx.borrow_mut();
                let cam = gfx.camera.clone();
                crate::gfx::vtex::draw_halftone(
                    &mut gfx.depth_queue,
                    &cam,
                    cx,
                    cy,
                    cz,
                    ux,
                    uy,
                    uz,
                    vx,
                    vy,
                    vz,
                    cols,
                    rows,
                    cw,
                    ch,
                    dens,
                    fr,
                    hue,
                );
                return Ok(Value::Unit);
            },

            // vtex_tessellated(cx,cy,cz, ux,uy,uz, vx,vy,vz, cols,rows, cell, amplitude,freq, fr,hue)
            "vtex_tessellated" | "ลายตาข่าย" | "纹镶嵌" | "網目模様" | "격자망" | "الگوی_کاشی‌کاری" | "نقش_مرصوف_متكرر" | "דוגמת_ריצוף_חוזר" | "نقش_ٹائلنگ" | "motif_tesselle" | "muster_tessellation" | "узор_мозаика" =>
            {
                let cx = self.arg_num(&args, 0, 0.)? as f32;
                let cy = self.arg_num(&args, 1, 0.)? as f32;
                let cz = self.arg_num(&args, 2, 0.)? as f32;
                let ux = self.arg_num(&args, 3, 1.)? as f32;
                let uy = self.arg_num(&args, 4, 0.)? as f32;
                let uz = self.arg_num(&args, 5, 0.)? as f32;
                let vx = self.arg_num(&args, 6, 0.)? as f32;
                let vy = self.arg_num(&args, 7, 0.)? as f32;
                let vz = self.arg_num(&args, 8, 1.)? as f32;
                let cols = self.arg_num(&args, 9, 14.)? as usize;
                let rows = self.arg_num(&args, 10, 10.)? as usize;
                let cell = self.arg_num(&args, 11, 0.6)? as f32;
                let amp = self.arg_num(&args, 12, 0.25)? as f32;
                let freq = self.arg_num(&args, 13, 4.)? as f32;
                let fr = self.arg_num(&args, 14, 0.)? as f32;
                let hue = self.arg_num(&args, 15, 0.)? as f32;
                let mut gfx = self.gfx.borrow_mut();
                let cam = gfx.camera.clone();
                crate::gfx::vtex::draw_tessellated(
                    &mut gfx.depth_queue,
                    &cam,
                    cx,
                    cy,
                    cz,
                    ux,
                    uy,
                    uz,
                    vx,
                    vy,
                    vz,
                    cols,
                    rows,
                    cell,
                    amp,
                    freq,
                    fr,
                    hue,
                );
                return Ok(Value::Unit);
            },

            // vtex_lotus(cx,cy,cz, ux,uy,uz, vx,vy,vz, r_inner,r_outer,n_petals, fr,hue)
            "vtex_lotus" | "ลายดอกบัว" | "纹莲" | "蓮模様" | "연꽃무늬" | "الگوی_لوتوس" | "نقش_لوتس" | "דוגמת_לוטוס" | "نقش_کنول" | "motif_lotus" | "muster_lotus" | "узор_лотос" =>
            {
                let cx = self.arg_num(&args, 0, 0.)? as f32;
                let cy = self.arg_num(&args, 1, 0.)? as f32;
                let cz = self.arg_num(&args, 2, 0.)? as f32;
                let ux = self.arg_num(&args, 3, 1.)? as f32;
                let uy = self.arg_num(&args, 4, 0.)? as f32;
                let uz = self.arg_num(&args, 5, 0.)? as f32;
                let vx = self.arg_num(&args, 6, 0.)? as f32;
                let vy = self.arg_num(&args, 7, 0.)? as f32;
                let vz = self.arg_num(&args, 8, 1.)? as f32;
                let ri = self.arg_num(&args, 9, 1.)? as f32;
                let ro = self.arg_num(&args, 10, 2.)? as f32;
                let np = self.arg_num(&args, 11, 12.)? as usize;
                let fr = self.arg_num(&args, 12, 0.)? as f32;
                let hue = self.arg_num(&args, 13, 0.)? as f32;
                let mut gfx = self.gfx.borrow_mut();
                let cam = gfx.camera.clone();
                crate::gfx::vtex::draw_lotus(
                    &mut gfx.depth_queue,
                    &cam,
                    cx,
                    cy,
                    cz,
                    ux,
                    uy,
                    uz,
                    vx,
                    vy,
                    vz,
                    ri,
                    ro,
                    np,
                    fr,
                    hue,
                );
                return Ok(Value::Unit);
            },

            // vtex_chakra(cx,cy,cz, ux,uy,uz, vx,vy,vz, r,n_spokes, fr,hue)
            "vtex_chakra" | "ลายจักร" | "纹轮" | "輪模様" | "바퀴무늬" | "الگوی_چاکرا" | "نقش_تشاكرا" | "דוגמת_צ'אקרה" | "نقش_چکر" | "motif_chakra" | "muster_chakra" | "узор_чакра" => {
                let cx = self.arg_num(&args, 0, 0.)? as f32;
                let cy = self.arg_num(&args, 1, 0.)? as f32;
                let cz = self.arg_num(&args, 2, 0.)? as f32;
                let ux = self.arg_num(&args, 3, 1.)? as f32;
                let uy = self.arg_num(&args, 4, 0.)? as f32;
                let uz = self.arg_num(&args, 5, 0.)? as f32;
                let vx = self.arg_num(&args, 6, 0.)? as f32;
                let vy = self.arg_num(&args, 7, 0.)? as f32;
                let vz = self.arg_num(&args, 8, 1.)? as f32;
                let r = self.arg_num(&args, 9, 2.)? as f32;
                let ns = self.arg_num(&args, 10, 8.)? as usize;
                let fr = self.arg_num(&args, 11, 0.)? as f32;
                let hue = self.arg_num(&args, 12, 0.)? as f32;
                let mut gfx = self.gfx.borrow_mut();
                let cam = gfx.camera.clone();
                crate::gfx::vtex::draw_chakra(
                    &mut gfx.depth_queue,
                    &cam,
                    cx,
                    cy,
                    cz,
                    ux,
                    uy,
                    uz,
                    vx,
                    vy,
                    vz,
                    r,
                    ns,
                    fr,
                    hue,
                );
                return Ok(Value::Unit);
            },

            // vtex_yantra(cx,cy,cz, ux,uy,uz, vx,vy,vz, n_layers,max_r, fr,hue)
            "vtex_yantra" | "ลายยันต์" | "纹咒" | "護符模様" | "부적무늬" | "الگوی_یانترا" | "نقش_يانترا" | "דוגמת_יאנטרה" | "نقش_ینترا" | "motif_yantra" | "muster_yantra" | "узор_янтра" =>
            {
                let cx = self.arg_num(&args, 0, 0.)? as f32;
                let cy = self.arg_num(&args, 1, 0.)? as f32;
                let cz = self.arg_num(&args, 2, 0.)? as f32;
                let ux = self.arg_num(&args, 3, 1.)? as f32;
                let uy = self.arg_num(&args, 4, 0.)? as f32;
                let uz = self.arg_num(&args, 5, 0.)? as f32;
                let vx = self.arg_num(&args, 6, 0.)? as f32;
                let vy = self.arg_num(&args, 7, 0.)? as f32;
                let vz = self.arg_num(&args, 8, 1.)? as f32;
                let nl = self.arg_num(&args, 9, 4.)? as usize;
                let mr = self.arg_num(&args, 10, 3.)? as f32;
                let fr = self.arg_num(&args, 11, 0.)? as f32;
                let hue = self.arg_num(&args, 12, 0.)? as f32;
                let mut gfx = self.gfx.borrow_mut();
                let cam = gfx.camera.clone();
                crate::gfx::vtex::draw_yantra(
                    &mut gfx.depth_queue,
                    &cam,
                    cx,
                    cy,
                    cz,
                    ux,
                    uy,
                    uz,
                    vx,
                    vy,
                    vz,
                    nl,
                    mr,
                    fr,
                    hue,
                );
                return Ok(Value::Unit);
            },

            // vtex_spiked_cog(cx,cy,cz, ux,uy,uz, vx,vy,vz, n_teeth,r_body,r_spike,r_hub,n_spokes, fr,hue)
            "vtex_spiked_cog" | "ฟันเฟืองหนาม" | "纹棘轮" | "歯車模様" | "톱니바퀴" | "الگوی_چرخ‌دنده_خاردار" | "نقش_ترس_شائك" | "דוגמת_גלגל_קוצני" | "نقش_خاردار_گیئر" | "motif_engrenage_pointes" | "muster_stachelzahnrad" | "узор_шестерня_шипы" =>
            {
                let cx = self.arg_num(&args, 0, 0.)? as f32;
                let cy = self.arg_num(&args, 1, 0.)? as f32;
                let cz = self.arg_num(&args, 2, 0.)? as f32;
                let ux = self.arg_num(&args, 3, 1.)? as f32;
                let uy = self.arg_num(&args, 4, 0.)? as f32;
                let uz = self.arg_num(&args, 5, 0.)? as f32;
                let vx = self.arg_num(&args, 6, 0.)? as f32;
                let vy = self.arg_num(&args, 7, 0.)? as f32;
                let vz = self.arg_num(&args, 8, 1.)? as f32;
                let nt = self.arg_num(&args, 9, 12.)? as usize;
                let rb = self.arg_num(&args, 10, 1.)? as f32;
                let rs = self.arg_num(&args, 11, 1.3)? as f32;
                let rh = self.arg_num(&args, 12, 0.2)? as f32;
                let ns = self.arg_num(&args, 13, 6.)? as usize;
                let fr = self.arg_num(&args, 14, 0.)? as f32;
                let hue = self.arg_num(&args, 15, 0.)? as f32;
                let mut gfx = self.gfx.borrow_mut();
                let cam = gfx.camera.clone();
                crate::gfx::vtex::draw_spiked_cog(
                    &mut gfx.depth_queue,
                    &cam,
                    cx,
                    cy,
                    cz,
                    ux,
                    uy,
                    uz,
                    vx,
                    vy,
                    vz,
                    nt,
                    rb,
                    rs,
                    rh,
                    ns,
                    fr,
                    hue,
                );
                return Ok(Value::Unit);
            },

            // vtex_torii(cx,cy,cz, ux,uy,uz, vx,vy,vz, width,height, fr,hue)
            "vtex_torii" | "ประตูโทริอิ" | "纹鸟居" | "鳥居" | "도리이" | "الگوی_توری_ژاپنی" | "نقش_توري" | "דוגמת_טוריי" | "نقش_توری_گیٹ" | "motif_torii" | "muster_torii" | "узор_тории" =>
            {
                let cx = self.arg_num(&args, 0, 0.)? as f32;
                let cy = self.arg_num(&args, 1, 0.)? as f32;
                let cz = self.arg_num(&args, 2, 0.)? as f32;
                let ux = self.arg_num(&args, 3, 1.)? as f32;
                let uy = self.arg_num(&args, 4, 0.)? as f32;
                let uz = self.arg_num(&args, 5, 0.)? as f32;
                let vx = self.arg_num(&args, 6, 0.)? as f32;
                let vy = self.arg_num(&args, 7, 0.)? as f32;
                let vz = self.arg_num(&args, 8, 1.)? as f32;
                let w = self.arg_num(&args, 9, 4.)? as f32;
                let h = self.arg_num(&args, 10, 5.)? as f32;
                let fr = self.arg_num(&args, 11, 0.)? as f32;
                let hue = self.arg_num(&args, 12, 0.)? as f32;
                let mut gfx = self.gfx.borrow_mut();
                let cam = gfx.camera.clone();
                crate::gfx::vtex::draw_torii(
                    &mut gfx.depth_queue,
                    &cam,
                    cx,
                    cy,
                    cz,
                    ux,
                    uy,
                    uz,
                    vx,
                    vy,
                    vz,
                    w,
                    h,
                    fr,
                    hue,
                );
                return Ok(Value::Unit);
            },

            // vtex_pagoda(cx,cy,cz, ux,uy,uz, vx,vy,vz, n_tiers,base_w,tier_h,taper,eave_out, fr,hue)
            "vtex_pagoda" | "เจดีย์" | "纹塔" | "塔" | "탑" | "الگوی_پاگودا" | "نقش_باغودا" | "דוגמת_פגודה" | "نقش_پگوڈا" | "motif_pagode" | "muster_pagode" | "узор_пагода" => {
                let cx = self.arg_num(&args, 0, 0.)? as f32;
                let cy = self.arg_num(&args, 1, 0.)? as f32;
                let cz = self.arg_num(&args, 2, 0.)? as f32;
                let ux = self.arg_num(&args, 3, 1.)? as f32;
                let uy = self.arg_num(&args, 4, 0.)? as f32;
                let uz = self.arg_num(&args, 5, 0.)? as f32;
                let vx = self.arg_num(&args, 6, 0.)? as f32;
                let vy = self.arg_num(&args, 7, 0.)? as f32;
                let vz = self.arg_num(&args, 8, 1.)? as f32;
                let nt = self.arg_num(&args, 9, 5.)? as usize;
                let bw = self.arg_num(&args, 10, 2.)? as f32;
                let th = self.arg_num(&args, 11, 1.)? as f32;
                let tp = self.arg_num(&args, 12, 0.72)? as f32;
                let eo = self.arg_num(&args, 13, 0.28)? as f32;
                let fr = self.arg_num(&args, 14, 0.)? as f32;
                let hue = self.arg_num(&args, 15, 0.)? as f32;
                let mut gfx = self.gfx.borrow_mut();
                let cam = gfx.camera.clone();
                crate::gfx::vtex::draw_pagoda(
                    &mut gfx.depth_queue,
                    &cam,
                    cx,
                    cy,
                    cz,
                    ux,
                    uy,
                    uz,
                    vx,
                    vy,
                    vz,
                    nt,
                    bw,
                    th,
                    tp,
                    eo,
                    fr,
                    hue,
                );
                return Ok(Value::Unit);
            },

            // ══════════════════════════════════════════════════════════════════
            // AUDIO BUILTINS
            // ══════════════════════════════════════════════════════════════════

            // audio_tone(idx, x, y, z, w, freq, amp, lfo_rate, lfo_depth)
            #[cfg(not(target_arch = "wasm32"))]
            "audio_tone"
            | "เสียงโทน"
            | "音调"
            | "音調"
            | "음조"
            | "空间音"
            | "空間音"
            | "공간음" | "تن_صدا" | "نغمة" | "צליל" | "آواز_کا_سر" | "tonalité_audio" | "audioton" | "звук_тон" => {
                let idx = self.arg_num(&args, 0, 0.0)? as usize;
                let x = self.arg_num(&args, 1, 0.0)? as f32;
                let y = self.arg_num(&args, 2, 0.0)? as f32;
                let z = self.arg_num(&args, 3, 0.0)? as f32;
                let w = self.arg_num(&args, 4, 1.0)? as f32;
                let freq = self.arg_num(&args, 5, 220.0)? as f32;
                let amp = self.arg_num(&args, 6, 0.15)? as f32;
                let lfo_rate = self.arg_num(&args, 7, 0.5)? as f32;
                let lfo_depth = self.arg_num(&args, 8, 0.02)? as f32;
                if let Some(audio) = &self.audio {
                    audio.set_tone(
                        idx,
                        ToneParams { x, y, z, w, freq, amp, lfo_rate, lfo_depth },
                    );
                }
                return Ok(Value::Unit);
            },

            #[cfg(not(target_arch = "wasm32"))]
            "audio_listener" | "ผู้ฟัง" | "音频监听" | "音声リスナー" | "오디오리스너" | "شنونده_صدا" | "مستمع_الصوت" | "מאזין_קול" | "آواز_سننے_والا" | "auditeur_audio" | "audiohörer" | "звук_слушатель" =>
            {
                let cry = self.arg_num(&args, 0, 1.0)? as f32;
                let sry = self.arg_num(&args, 1, 0.0)? as f32;
                let crx = self.arg_num(&args, 2, 1.0)? as f32;
                let srx = self.arg_num(&args, 3, 0.0)? as f32;
                if let Some(audio) = &self.audio {
                    audio.set_listener(cry, sry, crx, srx);
                }
                return Ok(Value::Unit);
            },

            #[cfg(not(target_arch = "wasm32"))]
            "audio_bgm" | "เพลงพื้นหลัง" | "เพลงประกอบ" | "背景乐" | "BGM" | "배경음악" | "موسیقی_پس‌زمینه" | "موسيقى_خلفية" | "מוזיקת_רקע" | "پس_منظر_موسیقی" | "musique_fond" | "hintergrundmusik" | "фоновая_музыка" =>
            {
                let path = match args.first() {
                    Some(Value::Str(s)) => s.clone(),
                    _ => return Ok(Value::Unit),
                };
                let vol = self.arg_num(&args, 1, 0.5)? as f32;
                if let Some(audio) = &self.audio {
                    audio.load_bgm(&path, vol);
                }
                return Ok(Value::Unit);
            },

            #[cfg(not(target_arch = "wasm32"))]
            "audio_bgm_volume"
            | "ระดับเสียงพื้นหลัง"
            | "ระดับเพลงประกอบ"
            | "背景乐音量"
            | "BGM音量"
            | "배경음악음량" | "بلندی_موسیقی_پس‌زمینه" | "مستوى_موسيقى_الخلفية" | "עוצמת_מוזיקת_רקע" | "پس_منظر_موسیقی_شدت" | "volume_musique_fond" | "hintergrundmusiklautstärke" | "громкость_фоновой_музыки" => {
                let vol = self.arg_num(&args, 0, 0.5)? as f32;
                if let Some(audio) = &self.audio {
                    audio.set_bgm_volume(vol);
                }
                return Ok(Value::Unit);
            },

            #[cfg(not(target_arch = "wasm32"))]
            "audio_volume" | "ระดับเสียง" | "音量" | "음량" | "بلندی_صدا" | "مستوى_الصوت" | "עוצמת_קול" | "آواز_کی_شدت" | "volume_audio" | "audiolautstärke" | "звук_громкость" => {
                let vol = self.arg_num(&args, 0, 0.7)? as f32;
                if let Some(audio) = &self.audio {
                    audio.set_master_volume(vol);
                }
                return Ok(Value::Unit);
            },

            // WASM audio builtins — delegate to Web Audio API
            #[cfg(target_arch = "wasm32")]
            "audio_tone"
            | "เสียงโทน"
            | "音调"
            | "音調"
            | "음조"
            | "空间音"
            | "空間音"
            | "공간음" | "تن_صدا" | "نغمة" | "צליל" | "آواز_کا_سر" | "tonalité_audio" | "audioton" | "звук_тон" => {
                let idx = self.arg_num(&args, 0, 0.0)? as usize;
                let x = self.arg_num(&args, 1, 0.0)? as f32;
                let y = self.arg_num(&args, 2, 0.0)? as f32;
                let z = self.arg_num(&args, 3, 0.0)? as f32;
                let w = self.arg_num(&args, 4, 1.0)? as f32;
                let freq = self.arg_num(&args, 5, 220.0)? as f32;
                let amp = self.arg_num(&args, 6, 0.15)? as f32;
                let lfo_rate = self.arg_num(&args, 7, 0.5)? as f32;
                let lfo_depth = self.arg_num(&args, 8, 0.02)? as f32;
                crate::gfx::audio_web::set_tone(idx, x, y, z, w, freq, amp, lfo_rate, lfo_depth);
                return Ok(Value::Unit);
            },

            #[cfg(target_arch = "wasm32")]
            "audio_listener" | "ผู้ฟัง" | "音频监听" | "音声リスナー" | "오디오리스너" | "شنونده_صدا" | "مستمع_الصوت" | "מאזין_קול" | "آواز_سننے_والا" | "auditeur_audio" | "audiohörer" | "звук_слушатель" =>
            {
                let cry = self.arg_num(&args, 0, 1.0)? as f32;
                let sry = self.arg_num(&args, 1, 0.0)? as f32;
                let crx = self.arg_num(&args, 2, 1.0)? as f32;
                let srx = self.arg_num(&args, 3, 0.0)? as f32;
                crate::gfx::audio_web::set_listener(cry, sry, crx, srx);
                return Ok(Value::Unit);
            },

            #[cfg(target_arch = "wasm32")]
            "audio_bgm" | "เพลงพื้นหลัง" | "เพลงประกอบ" | "背景乐" | "BGM" | "배경음악" | "موسیقی_پس‌زمینه" | "موسيقى_خلفية" | "מוזיקת_רקע" | "پس_منظر_موسیقی" | "musique_fond" | "hintergrundmusik" | "фоновая_музыка" =>
            {
                let path = self.arg_str(&args, 0, "");
                let vol = self.arg_num(&args, 1, 0.5)? as f32;
                crate::gfx::audio_web::load_bgm(&path, vol);
                return Ok(Value::Unit);
            },

            #[cfg(target_arch = "wasm32")]
            "audio_bgm_volume"
            | "ระดับเสียงพื้นหลัง"
            | "ระดับเพลงประกอบ"
            | "背景乐音量"
            | "BGM音量"
            | "배경음악음량" | "بلندی_موسیقی_پس‌زمینه" | "مستوى_موسيقى_الخلفية" | "עוצמת_מוזיקת_רקע" | "پس_منظر_موسیقی_شدت" | "volume_musique_fond" | "hintergrundmusiklautstärke" | "громкость_фоновой_музыки" => {
                let vol = self.arg_num(&args, 0, 0.5)? as f32;
                crate::gfx::audio_web::set_bgm_volume(vol);
                return Ok(Value::Unit);
            },

            #[cfg(target_arch = "wasm32")]
            "audio_volume" | "ระดับเสียง" | "音量" | "음량" | "بلندی_صدا" | "مستوى_الصوت" | "עוצמת_קול" | "آواز_کی_شدت" | "volume_audio" | "audiolautstärke" | "звук_громкость" => {
                let vol = self.arg_num(&args, 0, 0.7)? as f32;
                crate::gfx::audio_web::set_master_volume(vol);
                return Ok(Value::Unit);
            },

            // ── WASM sample load / play / stop / FX (Web Audio pool) ─────────
            #[cfg(target_arch = "wasm32")]
            "audio_sample_load" | "载入采样" | "サンプル読込" | "샘플로드" | "โหลดตัวอย่างเสียง" | "بارگذاری_نمونه_صدا" | "تحميل_عينة_صوتية" | "טעינת_דגימת_קול" | "آواز_نمونہ_لوڈ" | "charger_échantillon" | "sample_laden" | "загрузить_семпл" =>
            {
                let path = self.arg_str(&args, 0, "");
                let resolved = self.wasm_resolve_source_path(&path);
                match wasm_fetch_bytes(&resolved)
                    .and_then(|bytes| ling_music::from_bytes(&bytes).map_err(|e| e.to_string()))
                {
                    Ok(t) => {
                        let id = crate::gfx::audio_web::add_sample(&t.stereo, t.channels, t.rate);
                        return Ok(Value::Number(id as f64));
                    },
                    Err(e) => {
                        eprintln!("audio_sample_load failed ({path}): {e}");
                        return Ok(Value::Number(-1.0));
                    },
                }
            },
            #[cfg(target_arch = "wasm32")]
            "audio_sample_play" | "播放采样" | "サンプル再生" | "샘플재생" | "เล่นตัวอย่างเสียง" | "پخش_نمونه_صدا" | "تشغيل_عينة_صوتية" | "נגינת_דגימת_קול" | "آواز_نمونہ_چلاؤ" | "jouer_échantillon" | "sample_abspielen" | "играть_семпл" =>
            {
                let id = self.arg_num(&args, 0, 0.0)? as usize;
                let x = self.arg_num(&args, 1, 0.0)? as f32;
                let y = self.arg_num(&args, 2, 0.0)? as f32;
                let z = self.arg_num(&args, 3, 0.0)? as f32;
                // arg 4 is w (4th spatial dim) — ignored for 3-D panner
                let vol = self.arg_num(&args, 5, 1.0)? as f32;
                let looping = self.arg_num(&args, 6, 0.0)? > 0.5;
                crate::gfx::audio_web::play_sample(id, x, y, z, vol, looping);
                return Ok(Value::Number(0.0));
            },
            #[cfg(target_arch = "wasm32")]
            "audio_sample_stop"
            | "停止采样"
            | "サンプル停止"
            | "샘플정지"
            | "หยุดตัวอย่างเสียง"
            | "audio_fx_reverb"
            | "混响"
            | "リバーブ"
            | "리버브"
            | "เสียงก้อง"
            | "audio_fx_delay"
            | "回声"
            | "ディレイ効果"
            | "딜레이"
            | "เสียงสะท้อน"
            | "audio_fx_lowpass"
            | "低通滤波"
            | "ローパス"
            | "저역통과"
            | "กรองความถี่ต่ำ" | "توقف_نمونه_صدا" | "إيقاف_عينة_صوتية" | "עצירת_דגימת_קול" | "آواز_نمونہ_روکو" | "arrêter_échantillon" | "sample_stoppen" | "остановить_семпл" => {
                return Ok(Value::Unit);
            },

            // ── รอหน้าต่าง() — block until window closed / Escape ──
            "รอหน้าต่าง" | "wait_window" | "gfx_wait" | "انتظار_پنجره" | "انتظر_النافذة" | "המתן_לחלון" | "ونڈو_انتظار" => {
                #[cfg(not(target_arch = "wasm32"))]
                loop {
                    let still_open = {
                        let gfx = self.gfx.borrow();
                        gfx.window
                            .as_ref()
                            .map(|w| w.is_open() && !w.is_key_down(minifb::Key::Escape))
                            .unwrap_or(false)
                    };
                    if !still_open {
                        break;
                    }
                    let (buf, w, h) = {
                        let gfx = self.gfx.borrow();
                        (gfx.buffer.clone(), gfx.width, gfx.height)
                    };
                    let mut gfx = self.gfx.borrow_mut();
                    if let Some(win) = gfx.window.as_mut() {
                        if win.update_with_buffer(&buf, w, h).is_err() {
                            break;
                        }
                    }
                }
                return Ok(Value::Unit);
            },

            // ── File I/O ──────────────────────────────────────────────────────
            "read_file" | "อ่านไฟล์" | "خواندن_فایل" | "اقرأ_الملف" | "קרא_קובץ" | "فائل_پڑھو" => {
                #[cfg(target_arch = "wasm32")]
                return Ok(Value::Str(String::new()));
                #[cfg(not(target_arch = "wasm32"))]
                {
                    let path = self.arg_str(&args, 0, "").replace('\\', "/");
                    return std::fs::read_to_string(&path)
                        .map(Value::Str)
                        .map_err(|e| EvalErr::from(format!("read_file '{path}': {e}")));
                }
            },
            // ── networking (TCP, 2-peer co-op) ───────────────────────────────
            #[cfg(not(target_arch = "wasm32"))]
            "net_host" | "เน็ตโฮสต์" | "میزبانی_شبکه" | "استضف_الشبكة" | "ארח_רשת" | "نیٹ_ہوسٹ" => {
                let port = self.arg_num(&args, 0, 7777.0)? as u16;
                net::host(port);
                return Ok(Value::Unit);
            },
            #[cfg(not(target_arch = "wasm32"))]
            "net_join" | "เน็ตจอย" | "پیوستن_شبکه" | "انضم_للشبكة" | "הצטרף_לרשת" | "نیٹ_شمولیت" => {
                let ip = self.arg_str(&args, 0, "127.0.0.1");
                let port = self.arg_num(&args, 1, 7777.0)? as u16;
                net::join(&ip, port);
                return Ok(Value::Unit);
            },
            #[cfg(not(target_arch = "wasm32"))]
            "net_send" | "เน็ตส่ง" | "ارسال_شبکه" | "أرسل_عبر_الشبكة" | "שלח_ברשת" | "نیٹ_بھیجو" => {
                let s = self.arg_str(&args, 0, "");
                net::send(&s);
                return Ok(Value::Unit);
            },
            #[cfg(not(target_arch = "wasm32"))]
            "net_recv" | "เน็ตรับ" | "دریافت_شبکه" | "استقبل_من_الشبكة" | "קבל_מרשת" | "نیٹ_وصول" => {
                return Ok(Value::Str(net::recv()));
            },
            #[cfg(not(target_arch = "wasm32"))]
            "net_status" | "เน็ตสถานะ" | "وضعیت_شبکه" | "حالة_الشبكة" | "סטטוס_רשת" | "نیٹ_حالت" => {
                return Ok(Value::Number(net::status() as f64));
            },
            #[cfg(not(target_arch = "wasm32"))]
            "net_recv_from" => {
                return Ok(Value::Str(net::recv_from()));
            },
            #[cfg(not(target_arch = "wasm32"))]
            "net_send_to" => {
                let id = self.arg_num(&args, 0, 0.0)? as u64;
                let s = self.arg_str(&args, 1, "");
                net::send_to(id, &s);
                return Ok(Value::Unit);
            },
            #[cfg(not(target_arch = "wasm32"))]
            "net_close" | "연결종료" => {
                net::close();
                return Ok(Value::Unit);
            },
            // ── LAN lobby discovery (UDP broadcast) ──
            #[cfg(not(target_arch = "wasm32"))]
            "net_announce" | "เน็ตประกาศ" | "اعلام_شبکه" | "أعلن_في_الشبكة" | "הכרז_ברשת" | "نیٹ_اعلان" => {
                let port = self.arg_num(&args, 0, 7778.0)? as u16;
                let info = self.arg_str(&args, 1, "");
                net::announce(port, &info);
                return Ok(Value::Unit);
            },
            #[cfg(not(target_arch = "wasm32"))]
            "net_announce_stop" | "เน็ตหยุดประกาศ" | "توقف_اعلام" | "أوقف_الإعلان" | "עצור_הכרזה" | "اعلان_روکو" => {
                net::announce_stop();
                return Ok(Value::Unit);
            },
            #[cfg(not(target_arch = "wasm32"))]
            "net_discover" | "เน็ตค้นหา" | "کشف_شبکه" | "اكتشف_الشبكة" | "גלה_רשת" | "نیٹ_دریافت" => {
                let port = self.arg_num(&args, 0, 7778.0)? as u16;
                return Ok(Value::Str(net::discover(port)));
            },
            #[cfg(not(target_arch = "wasm32"))]
            "net_test" | "เน็ตทดสอบ" | "آزمون_شبکه" | "اختبر_الشبكة" | "בדוק_רשת" | "نیٹ_ٹیسٹ" => {
                let port = self.arg_num(&args, 0, 7777.0)? as u16;
                return Ok(Value::Str(net::test_bind(port)));
            },
            // ── HTTP server (interpreter <-> async bridge, see runtime::web) ──
            #[cfg(all(not(target_arch = "wasm32"), feature = "web"))]
            "http_route" | "เว็บเส้นทาง" | "مسیر_HTTP" | "مسار_HTTP" | "נתיב_HTTP" | "HTTP_روٹ" => {
                let method = self.arg_str(&args, 0, "GET").to_uppercase();
                let path = self.arg_str(&args, 1, "/");
                let handler = args.get(2).cloned().unwrap_or(Value::Unit);
                self.http_routes.push((method, path, handler));
                return Ok(Value::Unit);
            },
            // Registers a directory to be served as raw bytes at `prefix` (fonts,
            // images, generated zips/PDFs) — bypasses the String-only Request/
            // Response bridge entirely, so binary files come through intact.
            #[cfg(all(not(target_arch = "wasm32"), feature = "web"))]
            "http_static" | "เว็บสแตติก" | "فایل_ایستای_HTTP" | "ملفات_HTTP_ثابتة" | "קבצים_סטטיים_HTTP" | "HTTP_مستقل_فائل" => {
                let prefix = self.arg_str(&args, 0, "/static");
                let dir = self.arg_str(&args, 1, "static");
                self.http_static_dirs.push((prefix, dir));
                return Ok(Value::Unit);
            },
            #[cfg(all(not(target_arch = "wasm32"), feature = "web"))]
            "http_serve" | "เว็บเสิร์ฟ" | "سرویس_HTTP" | "قدّم_HTTP" | "הגש_HTTP" | "HTTP_سرو" => {
                let host = self.arg_str(&args, 0, "127.0.0.1");
                let port = self.arg_num(&args, 1, 8080.0)? as u16;
                let routes = std::mem::take(&mut self.http_routes);
                let static_dirs = std::mem::take(&mut self.http_static_dirs);
                // No premature "listening" print here: ling_http::serve_http
                // (called from spawn_server's background thread) now prints
                // its own banner, but only after the socket is actually
                // bound — a more honest signal than printing right after
                // requesting the background thread be spawned.
                let rx = web::spawn_server(host.clone(), port, static_dirs);
                for pending in rx {
                    let matched = routes
                        .iter()
                        .find(|(m, p, _)| m == &pending.method && p == &pending.path);
                    let response = match matched {
                        Some((_, _, handler)) => {
                            let req_value = Value::Struct {
                                name: "Request".to_string(),
                                fields: vec![
                                    ("method".to_string(), Value::Str(pending.method.clone())),
                                    ("path".to_string(), Value::Str(pending.path.clone())),
                                    ("query".to_string(), Value::Str(pending.query.clone())),
                                    ("body".to_string(), Value::Str(pending.body.clone())),
                                    ("cookie".to_string(), Value::Str(pending.cookie.clone())),
                                    (
                                        "authorization".to_string(),
                                        Value::Str(pending.authorization.clone()),
                                    ),
                                    (
                                        "client_ip".to_string(),
                                        Value::Str(pending.client_ip.clone()),
                                    ),
                                ],
                            };
                            match self.call_value(handler.clone(), vec![req_value]) {
                                Ok(v) => web::value_to_response(&v),
                                Err(e) => web::HttpResponse {
                                    status: 500,
                                    content_type: "text/plain; charset=utf-8".to_string(),
                                    body: format!("handler error: {e:?}"),
                                    set_cookie: None,
                                    location: None,
                                },
                            }
                        },
                        None => web::HttpResponse {
                            status: 404,
                            content_type: "text/plain; charset=utf-8".to_string(),
                            body: "not found".to_string(),
                            set_cookie: None,
                            location: None,
                        },
                    };
                    let _ = pending.respond_to.send(response);
                }
                return Ok(Value::Unit);
            },
            // Fires a POST request on a background async runtime and returns a job
            // id immediately — for slow external calls (e.g. local Stable Diffusion
            // generation) that must not block http_serve's single-threaded loop.
            #[cfg(all(not(target_arch = "wasm32"), feature = "web"))]
            "http_post_async" | "เว็บโพสต์ไม่บล็อก" | "ارسال_ناهمگام_HTTP" | "أرسل_HTTP_غير_متزامن" | "שלח_HTTP_אסינכרוני" | "HTTP_غیر_ہمزمان_بھیجو" => {
                let url = self.arg_str(&args, 0, "");
                let body = self.arg_str(&args, 1, "");
                let content_type = self.arg_str(&args, 2, "application/json");
                let id = self.async_jobs.start_post(url, content_type, body);
                return Ok(Value::Str(id));
            },
            // Non-blocking poll: "" while the job named by http_post_async (or
            // sdai_generate_start) is still running, the result once it
            // completes — same job table, same builtin polls both.
            #[cfg(all(not(target_arch = "wasm32"), feature = "web"))]
            "http_job_poll" | "เว็บงานสำรวจ" | "بررسی_وظیفه_HTTP" | "استطلع_مهمة_HTTP" | "בדוק_משימת_HTTP" | "HTTP_کام_پول" => {
                let id = self.arg_str(&args, 0, "");
                return Ok(Value::Str(self.async_jobs.poll(&id).unwrap_or_default()));
            },
            // Starts an AUTOMATIC1111-compatible txt2img generation in the
            // background against `base_url` (e.g. "http://127.0.0.1:1342").
            // Poll with http_job_poll: the result is the plain base64 PNG
            // once ready, or a string starting with "ERROR:" on failure —
            // the JSON response itself is parsed in Rust (see
            // AsyncJobs::start_sdai_txt2img), since `.ling` has no JSON parser.
            #[cfg(all(not(target_arch = "wasm32"), feature = "web"))]
            "sdai_generate_start" | "เอสดีเอไอเริ่มสร้าง" | "شروع_تولید_هوش" | "ابدأ_توليد_الذكاء" | "התחל_יצירת_בינה" | "اے_آئی_تخلیق_شروع" => {
                let base_url = self.arg_str(&args, 0, "http://127.0.0.1:1342");
                let prompt = self.arg_str(&args, 1, "");
                let width = self.arg_num(&args, 2, 512.0)? as u32;
                let height = self.arg_num(&args, 3, 512.0)? as u32;
                let id = self.async_jobs.start_sdai_txt2img(base_url, prompt, width, height);
                return Ok(Value::Str(id));
            },
            // ── query_param("q=a&page=2", "q", "") → "a" (URL-decoded) ──
            "query_param" | "พารามิเตอร์" | "پارامتر_پرسوجو" | "معامل_الاستعلام" | "פרמטר_שאילתה" | "کوئری_پیرامیٹر" => {
                let qs = self.arg_str(&args, 0, "");
                let name = self.arg_str(&args, 1, "");
                let default = self.arg_str(&args, 2, "");
                let mut found = default;
                for pair in qs.split('&') {
                    let mut it = pair.splitn(2, '=');
                    if it.next().unwrap_or("") == name {
                        found = url_decode(it.next().unwrap_or(""));
                        break;
                    }
                }
                return Ok(Value::Str(found));
            },
            // ── cookie_get("sid=abc; x=1", "sid", "") → "abc" ──
            "cookie_get" | "รับคุกกี้" | "دریافت_کوکی" | "اجلب_الكعكة" | "קבל_עוגייה" | "کوکی_حاصل_کرو" => {
                let header = self.arg_str(&args, 0, "");
                let name = self.arg_str(&args, 1, "");
                let default = self.arg_str(&args, 2, "");
                let mut found = default;
                for pair in header.split(';') {
                    let p = pair.trim();
                    let mut it = p.splitn(2, '=');
                    if it.next().unwrap_or("") == name {
                        found = it.next().unwrap_or("").to_string();
                        break;
                    }
                }
                return Ok(Value::Str(found));
            },
            // ── html_escape(s) — & < > " ' → entities, for echoing user input ──
            "html_escape" | "กันเอชทีเอ็มแอล" | "فرار_HTML" | "أفلت_HTML" | "בריחת_HTML" | "HTML_ایسکیپ" => {
                let s = self.arg_str(&args, 0, "");
                return Ok(Value::Str(
                    s.replace('&', "&amp;")
                        .replace('<', "&lt;")
                        .replace('>', "&gt;")
                        .replace('"', "&quot;")
                        .replace('\'', "&#39;"),
                ));
            },
            // json_escape(s) — escape a string for embedding inside a JSON
            // string literal (", \, and control chars). Needed because the
            // registry builds JSON API responses by concatenation; without
            // this a value containing " or \ breaks or injects into the JSON.
            "json_escape" | "หนีเจสัน" | "فرار_JSON" | "أفلت_JSON" | "בריחת_JSON" | "JSON_ایسکیپ" => {
                let s = self.arg_str(&args, 0, "");
                let mut out = String::with_capacity(s.len() + 8);
                for c in s.chars() {
                    match c {
                        '"' => out.push_str("\\\""),
                        '\\' => out.push_str("\\\\"),
                        '\n' => out.push_str("\\n"),
                        '\r' => out.push_str("\\r"),
                        '\t' => out.push_str("\\t"),
                        c if (c as u32) < 0x20 => {
                            out.push_str(&format!("\\u{:04x}", c as u32))
                        },
                        c => out.push(c),
                    }
                }
                return Ok(Value::Str(out));
            },
            // ── CLI arguments: cli_arg("port", "8080") reads `--port 6688` ──
            "cli_arg" | "อาร์กิวเมนต์" | "آرگومان_خط‌فرمان" | "معامل_سطر_الأوامر" | "ארגומנט_שורת_פקודה" | "سی_ایل_آئی_دلیل" => {
                let name = self.arg_str(&args, 0, "");
                let default = self.arg_str(&args, 1, "");
                let flag = format!("--{name}");
                let argv: Vec<String> = std::env::args().collect();
                let found = argv
                    .iter()
                    .position(|a| a == &flag)
                    .and_then(|i| argv.get(i + 1))
                    .cloned()
                    .unwrap_or(default);
                return Ok(Value::Str(found));
            },
            // ── SQLite (rusqlite, synchronous — matches the interpreter) ──
            #[cfg(all(not(target_arch = "wasm32"), feature = "web"))]
            "db_open" | "ฐานข้อมูลเปิด" | "باز_کردن_پایگاه_داده" | "افتح_قاعدة_البيانات" | "פתח_מסד_נתונים" | "ڈیٹا_بیس_کھولو" => {
                let path = self.arg_str(&args, 0, "app.db");
                let conn = ling_http::rusqlite::Connection::open(&path)
                    .map_err(|e| EvalErr::from(format!("db_open '{path}': {e}")))?;
                let _ = conn.execute_batch("PRAGMA foreign_keys = ON; PRAGMA journal_mode = WAL;");
                self.db = Some(conn);
                return Ok(Value::Unit);
            },
            // db_exec(sql, ...params) → rows affected. Params bind positionally
            // (?1, ?2, ...): numbers as REAL, bools as 0/1, everything else TEXT.
            #[cfg(all(not(target_arch = "wasm32"), feature = "web"))]
            "db_exec" | "ฐานข้อมูลรัน" | "اجرای_پایگاه_داده" | "نفّذ_في_قاعدة_البيانات" | "בצע_במסד_נתונים" | "ڈیٹا_بیس_عمل" => {
                let sql = self.arg_str(&args, 0, "");
                let params = values_to_sql_params(&args[1.min(args.len())..]);
                let conn = self
                    .db
                    .as_ref()
                    .ok_or_else(|| EvalErr::from("db_exec: call db_open first".to_string()))?;
                let n = conn
                    .execute(
                        &sql,
                        ling_http::rusqlite::params_from_iter(params.iter()),
                    )
                    .map_err(|e| EvalErr::from(format!("db_exec: {e}\n  sql: {sql}")))?;
                return Ok(Value::Number(n as f64));
            },
            // db_query(sql, ...params) → List of Row structs (row.column_name).
            #[cfg(all(not(target_arch = "wasm32"), feature = "web"))]
            "db_query" | "ฐานข้อมูลถาม" | "پرسوجوی_پایگاه_داده" | "استعلم_قاعدة_البيانات" | "שאילתת_מסד_נתונים" | "ڈیٹا_بیس_سوال" => {
                let sql = self.arg_str(&args, 0, "");
                let params = values_to_sql_params(&args[1.min(args.len())..]);
                let conn = self
                    .db
                    .as_ref()
                    .ok_or_else(|| EvalErr::from("db_query: call db_open first".to_string()))?;
                let mut stmt = conn
                    .prepare(&sql)
                    .map_err(|e| EvalErr::from(format!("db_query: {e}\n  sql: {sql}")))?;
                let col_names: Vec<String> =
                    stmt.column_names().iter().map(|s| s.to_string()).collect();
                let mut rows = stmt
                    .query(ling_http::rusqlite::params_from_iter(params.iter()))
                    .map_err(|e| EvalErr::from(format!("db_query: {e}")))?;
                let mut out = Vec::new();
                while let Some(row) = rows
                    .next()
                    .map_err(|e| EvalErr::from(format!("db_query row: {e}")))?
                {
                    let mut fields = Vec::with_capacity(col_names.len());
                    for (i, col) in col_names.iter().enumerate() {
                        use ling_http::rusqlite::types::ValueRef;
                        let v = match row.get_ref(i) {
                            Ok(ValueRef::Null) => Value::Str(String::new()),
                            Ok(ValueRef::Integer(n)) => Value::Number(n as f64),
                            Ok(ValueRef::Real(n)) => Value::Number(n),
                            Ok(ValueRef::Text(t)) => {
                                Value::Str(String::from_utf8_lossy(t).into_owned())
                            },
                            Ok(ValueRef::Blob(b)) =>

                            {
                                use base64::Engine as _;
                                Value::Str(base64::engine::general_purpose::STANDARD.encode(b))
                            },
                            Err(_) => Value::Str(String::new()),
                        };
                        fields.push((col.clone(), v));
                    }
                    out.push(Value::Struct { name: "Row".to_string(), fields });
                }
                return Ok(Value::List(Rc::new(out)));
            },
            // ── gamepad (gilrs) ──
            #[cfg(not(target_arch = "wasm32"))]
            "gamepad_poll" | "จอยโพล" | "بررسی_دسته_بازی" | "استطلع_يد_اللعب" | "בדוק_בקר_משחק" | "گیم_پیڈ_پول_کرو" => {
                gamepad::poll();
                return Ok(Value::Unit);
            },
            #[cfg(not(target_arch = "wasm32"))]
            "gamepad_button" | "จอยปุ่ม" | "دکمه_دسته_بازی" | "زر_يد_اللعب" | "כפתור_בקר_משחק" | "گیم_پیڈ_بٹن_دبایا" => {
                let name = self.arg_str(&args, 0, "");
                return Ok(Value::Number(if gamepad::button(&name) {
                    1.0
                } else {
                    0.0
                }));
            },
            #[cfg(not(target_arch = "wasm32"))]
            "gamepad_axis" | "จอยแกน" | "محور_دسته_بازی" | "محور_يد_اللعب" | "ציר_בקר_משחק" | "گیم_پیڈ_محور" => {
                let name = self.arg_str(&args, 0, "");
                return Ok(Value::Number(gamepad::axis(&name) as f64));
            },
            #[cfg(not(target_arch = "wasm32"))]
            "gamepad_rumble" | "จอยสั่น" | "لرزش_دسته_بازی" | "اهتزاز_يد_اللعب" | "רטט_בקר_משחק" | "گیم_پیڈ_لرزش" => {
                let low = self.arg_num(&args, 0, 0.0)? as f32;
                let high = self.arg_num(&args, 1, 0.0)? as f32;
                let ms = self.arg_num(&args, 2, 200.0)? as u32;
                gamepad::rumble(low, high, ms);
                return Ok(Value::Unit);
            },
            #[cfg(not(target_arch = "wasm32"))]
            "gamepad_list" | "จอยรายการ" | "فهرست_دسته‌های_بازی" | "قائمة_أيدي_اللعب" | "רשימת_בקרי_משחק" | "گیم_پیڈ_فہرست" => {
                return Ok(Value::Str(gamepad::list()));
            },
            #[cfg(not(target_arch = "wasm32"))]
            "gamepad_any" | "จอยใดๆ" | "هر_دسته_بازی" | "أي_يد_لعب" | "בקר_כלשהו" | "کوئی_بھی_گیم_پیڈ" => {
                return Ok(Value::Number(if gamepad::any_button() { 1.0 } else { 0.0 }));
            },
            // wasm32: gamepad not available — return safe no-op values
            #[cfg(target_arch = "wasm32")]
            "gamepad_poll" | "จอยโพล" | "gamepad_rumble" | "จอยสั่น" | "بررسی_دسته_بازی" | "استطلع_يد_اللعب" | "בדוק_בקר_משחק" | "گیم_پیڈ_پول_کرو" => {
                return Ok(Value::Unit);
            },
            #[cfg(target_arch = "wasm32")]
            "gamepad_button" | "จอยปุ่ม" | "gamepad_axis" | "จอยแกน" | "gamepad_any" | "จอยใดๆ" | "دکمه_دسته_بازی" | "زر_يد_اللعب" | "כפתור_בקר_משחק" | "گیم_پیڈ_بٹن" =>
            {
                return Ok(Value::Number(0.0));
            },
            #[cfg(target_arch = "wasm32")]
            "gamepad_list" | "จอยรายการ" | "فهرست_دسته‌های_بازی" | "قائمة_أيدي_اللعب" | "רשימת_בקרי_משחק" | "گیم_پیڈ_فہرست" => {
                return Ok(Value::Str(String::new()));
            },

            // ── game AI: neural networks ─────────────────────────────────────
            // nn_new(inputs[, seed]) → handle
            #[cfg(not(target_arch = "wasm32"))]
            "nn_new" | "建神经网" | "ニューラル作成" | "신경망생성" | "สร้างโครงข่าย" | "شبکه_جدید" | "شبكة_جديدة" | "רשת_חדשה" | "نئی_نیورل_نیٹ" | "nouveau_réseau" | "neues_netz" | "новая_сеть" =>
            {
                let n_in = self.arg_num(&args, 0, 1.0)?.max(0.0) as usize;
                let seed = self.arg_num(&args, 1, 1.0)? as u64;
                return Ok(Value::Number(ai::nn_new(n_in, seed) as f64));
            },
            // nn_dense(handle, units[, activation]) — append a layer
            #[cfg(not(target_arch = "wasm32"))]
            "nn_dense" | "密集层" | "密層追加" | "밀집층" | "ชั้นหนาแน่น" | "لایه_متراکم" | "طبقة_كثيفة" | "שכבה_צפופה" | "ڈینس_لیئر" | "réseau_dense" | "netz_dicht" | "плотная_сеть" =>
            {
                let id = self.arg_num(&args, 0, -1.0)? as i64;
                let units = self.arg_num(&args, 1, 1.0)?.max(1.0) as usize;
                let act = self.arg_str(&args, 2, "relu");
                ai::nn_dense(id, units, &act);
                return Ok(Value::Unit);
            },
            // nn_forward(handle, [inputs]) → [outputs]
            #[cfg(not(target_arch = "wasm32"))]
            "nn_forward" | "神经前向" | "順伝播" | "순전파" | "ส่งต่อโครงข่าย" | "پیش‌روی_شبکه" | "تمرير_أمامي" | "העברה_קדימה" | "فارورڈ_پاس" | "propager_réseau" | "netz_vorwärts" | "прямой_проход_сети" =>
            {
                let id = self.arg_num(&args, 0, -1.0)? as i64;
                let input = self.arg_list_f32(&args, 1);
                let out = ai::nn_forward(id, &input);
                return Ok(Value::List(Rc::new(
                    out.into_iter().map(|v| Value::Number(v as f64)).collect(),
                )));
            },
            // nn_train(handle, [inputs], [targets][, lr]) → loss
            #[cfg(not(target_arch = "wasm32"))]
            "nn_train" | "训练网" | "ニューラル学習" | "신경망학습" | "ฝึกโครงข่าย" | "آموزش_شبکه" | "درّب_الشبكة" | "אמן_רשת" | "نیٹ_ٹریننگ" | "entraîner_réseau" | "netz_trainieren" | "обучить_сеть" =>
            {
                let id = self.arg_num(&args, 0, -1.0)? as i64;
                let input = self.arg_list_f32(&args, 1);
                let target = self.arg_list_f32(&args, 2);
                let lr = self.arg_num(&args, 3, 0.01)? as f32;
                return Ok(Value::Number(ai::nn_train(id, &input, &target, lr) as f64));
            },
            // nn_save(handle, path) → bool
            #[cfg(not(target_arch = "wasm32"))]
            "nn_save" | "保存网" | "網保存" | "신경망저장" | "บันทึกโครงข่าย" | "ذخیره_شبکه" | "احفظ_الشبكة" | "שמור_רשת" | "نیٹ_محفوظ_کرو" | "sauvegarder_réseau" | "netz_speichern" | "сохранить_сеть" =>
            {
                let id = self.arg_num(&args, 0, -1.0)? as i64;
                let path = self.arg_str(&args, 1, "model.lnn");
                return Ok(Value::Bool(ai::nn_save(id, &path)));
            },
            // nn_load(path) → handle (-1 on failure)
            #[cfg(not(target_arch = "wasm32"))]
            "nn_load" | "载入网" | "網読込" | "신경망불러오기" | "โหลดโครงข่าย" | "بارگذاری_شبکه" | "حمّل_الشبكة" | "טען_רשת" | "نیٹ_لوڈ" | "charger_réseau" | "netz_laden" | "загрузить_сеть" =>
            {
                let path = self.arg_str(&args, 0, "model.lnn");
                return Ok(Value::Number(ai::nn_load(&path) as f64));
            },

            // ── game AI: behavior trees ──────────────────────────────────────
            // bt_build(dsl_string) → handle
            #[cfg(not(target_arch = "wasm32"))]
            "bt_build" | "建行为树" | "行動木構築" | "행동트리구성" | "สร้างต้นไม้พฤติกรรม" | "ساخت_درخت_رفتار" | "ابنِ_شجرة_السلوك" | "בנה_עץ_התנהגות" | "بی_ٹی_تعمیر" | "construire_arbre_comportement" | "verhaltensbaum_bauen" | "построить_дерево_поведения" =>
            {
                let spec = self.arg_str(&args, 0, "");
                return Ok(Value::Number(ai::bt_build(&spec) as f64));
            },
            // bt_set(handle, key, value) — set a blackboard fact
            #[cfg(not(target_arch = "wasm32"))]
            "bt_set" | "设事实" | "事実設定" | "사실설정" | "ตั้งข้อเท็จจริง" | "تنظیم_واقعیت" | "عيّن_حقيقة" | "קבע_עובדה" | "بی_ٹی_سیٹ" | "définir_arbre_comportement" | "verhaltensbaum_setzen" | "задать_дерево_поведения" =>
            {
                let id = self.arg_num(&args, 0, -1.0)? as i64;
                let key = self.arg_str(&args, 1, "");
                let val = self.arg_num(&args, 2, 0.0)? as f32;
                ai::bt_set(id, &key, val);
                return Ok(Value::Unit);
            },
            // bt_tick(handle) → chosen action name ("" if none)
            #[cfg(not(target_arch = "wasm32"))]
            "bt_tick" | "行为树滴答" | "行動木更新" | "행동트리틱" | "เดินต้นไม้พฤติกรรม" | "تیک_درخت_رفتار" | "نبضة_شجرة_السلوك" | "טיק_עץ_התנהגות" | "بی_ٹی_ٹک" | "tick_arbre_comportement" | "verhaltensbaum_tick" | "тик_дерева_поведения" =>
            {
                let id = self.arg_num(&args, 0, -1.0)? as i64;
                return Ok(Value::Str(ai::bt_tick(id)));
            },
            // bt_status(handle) → 0 fail / 1 success / 2 running
            #[cfg(not(target_arch = "wasm32"))]
            "bt_status" | "行为树状态" | "行動木状態" | "행동트리상태" | "สถานะต้นไม้พฤติกรรม" | "وضعیت_درخت_رفتار" | "حالة_شجرة_السلوك" | "סטטוס_עץ_התנהגות" | "بی_ٹی_حالت" | "statut_arbre_comportement" | "verhaltensbaum_status" | "статус_дерева_поведения" =>
            {
                let id = self.arg_num(&args, 0, -1.0)? as i64;
                return Ok(Value::Number(ai::bt_status(id) as f64));
            },

            // ── game AI: miniature dialog LLM ────────────────────────────────
            // dialog_new([ctx, embed, hidden, seed]) → handle
            #[cfg(not(target_arch = "wasm32"))]
            "dialog_new" | "建对话模型" | "対話モデル作成" | "대화모델생성" | "สร้างโมเดลสนทนา" | "مدل_گفتگوی_جدید" | "نموذج_حوار_جديد" | "מודל_דיאלוג_חדש" | "نیا_مکالمہ_ماڈل" | "nouveau_dialogue" | "neuer_dialog" | "новый_диалог" =>
            {
                let ctx = self.arg_num(&args, 0, 3.0)?.max(1.0) as usize;
                let embed = self.arg_num(&args, 1, 32.0)?.max(1.0) as usize;
                let hidden = self.arg_num(&args, 2, 64.0)?.max(1.0) as usize;
                let seed = self.arg_num(&args, 3, 1.0)? as u64;
                return Ok(Value::Number(
                    ai::dialog_new(ctx, embed, hidden, seed) as f64
                ));
            },
            // dialog_learn(handle, text) — add one utterance to the corpus
            #[cfg(not(target_arch = "wasm32"))]
            "dialog_learn" | "对话学习" | "対話学習" | "대화학습" | "เรียนรู้สนทนา" | "یادگیری_گفتگو" | "تعلّم_الحوار" | "למד_דיאלוג" | "مکالمہ_سیکھو" | "apprendre_dialogue" | "dialog_lernen" | "обучить_диалог" =>
            {
                let id = self.arg_num(&args, 0, -1.0)? as i64;
                let text = self.arg_str(&args, 1, "");
                ai::dialog_learn(id, &text);
                return Ok(Value::Unit);
            },
            // dialog_load(handle, path) → lines added (-1 on error)
            #[cfg(not(target_arch = "wasm32"))]
            "dialog_load" | "对话载入" | "対話読込" | "대화불러오기" | "โหลดชุดสนทนา" | "بارگذاری_مجموعه_گفتگو" | "حمّل_مجموعة_الحوار" | "טען_מערך_דיאלוג" | "مکالمہ_مجموعہ_لوڈ" | "charger_dialogue" | "dialog_laden" | "загрузить_диалог" =>
            {
                let id = self.arg_num(&args, 0, -1.0)? as i64;
                let path = self.arg_str(&args, 1, "");
                return Ok(Value::Number(ai::dialog_load(id, &path) as f64));
            },
            // dialog_train(handle[, epochs, lr]) → loss
            #[cfg(not(target_arch = "wasm32"))]
            "dialog_train" | "对话训练" | "対話訓練" | "대화훈련" | "ฝึกสนทนา" | "آموزش_گفتگو" | "درّب_الحوار" | "אמן_דיאלוג" | "مکالمہ_ٹریننگ" | "entraîner_dialogue" | "dialog_trainieren" | "тренировать_диалог" =>
            {
                let id = self.arg_num(&args, 0, -1.0)? as i64;
                let epochs = self.arg_num(&args, 1, 20.0)?.max(1.0) as usize;
                let lr = self.arg_num(&args, 2, 0.1)? as f32;
                return Ok(Value::Number(ai::dialog_train(id, epochs, lr) as f64));
            },
            // dialog_say(handle, prompt[, max_tokens, temperature]) → reply text
            #[cfg(not(target_arch = "wasm32"))]
            "dialog_say" | "对话生成" | "対話生成" | "대화생성" | "พูดสนทนา" | "بگو" | "قل" | "אמור" | "کہو" | "dire_dialogue" | "dialog_sagen" | "сказать_диалог" =>
            {
                let id = self.arg_num(&args, 0, -1.0)? as i64;
                let prompt = self.arg_str(&args, 1, "");
                let max = self.arg_num(&args, 2, 24.0)?.max(1.0) as usize;
                let temp = self.arg_num(&args, 3, 0.8)? as f32;
                return Ok(Value::Str(ai::dialog_say(id, &prompt, max, temp)));
            },
            // dialog_save(handle, path) → bool
            #[cfg(not(target_arch = "wasm32"))]
            "dialog_save" | "对话存模" | "対話モデル保存" | "대화모델저장" | "บันทึกโมเดลสนทนา" | "ذخیره_مدل_گفتگو" | "احفظ_نموذج_الحوار" | "שמור_מודל_דיאלוג" | "مکالمہ_ماڈل_محفوظ" | "sauvegarder_dialogue" | "dialog_speichern" | "сохранить_диалог" =>
            {
                let id = self.arg_num(&args, 0, -1.0)? as i64;
                let path = self.arg_str(&args, 1, "model.llm");
                return Ok(Value::Bool(ai::dialog_save(id, &path)));
            },
            // dialog_load_model(path) → handle (-1 on failure)
            #[cfg(not(target_arch = "wasm32"))]
            "dialog_load_model"
            | "对话载模"
            | "対話モデル読込"
            | "대화모델불러오기"
            | "โหลดโมเดลสนทนา" | "بارگذاری_مدل_گفتگو" | "حمّل_نموذج_الحوار" | "טען_מודל_דיאלוג" | "مکالمہ_ماڈل_لوڈ" | "charger_modèle_dialogue" | "dialog_modell_laden" | "загрузить_модель_диалога" => {
                let path = self.arg_str(&args, 0, "model.llm");
                return Ok(Value::Number(ai::dialog_load_model(&path) as f64));
            },

            // Decodes `application/x-www-form-urlencoded` text: '+' -> space,
            // '%XX' -> byte. Needed to read plain HTML `<form>` POST bodies.
            "url_decode" | "网址解码" => {
                let s = self.arg_str(&args, 0, "");
                let bytes = s.as_bytes();
                let mut out = Vec::with_capacity(bytes.len());
                let mut i = 0;
                while i < bytes.len() {
                    match bytes[i] {
                        b'+' => {
                            out.push(b' ');
                            i += 1;
                        },
                        b'%' if i + 2 < bytes.len() => {
                            let hex = std::str::from_utf8(&bytes[i + 1..i + 3]).unwrap_or("");
                            match u8::from_str_radix(hex, 16) {
                                Ok(b) => {
                                    out.push(b);
                                    i += 3;
                                },
                                Err(_) => {
                                    out.push(bytes[i]);
                                    i += 1;
                                },
                            }
                        },
                        b => {
                            out.push(b);
                            i += 1;
                        },
                    }
                }
                return Ok(Value::Str(String::from_utf8_lossy(&out).into_owned()));
            },
            // Seconds since Unix epoch (float — sub-second precision). No ISO/date
            // formatting builtin exists yet; `.ling` code that wants a display
            // string currently just uses the raw number.
            "now_unix" | "现在时间" => {
                return Ok(Value::Number(now_secs()));
            },
            "file_exists" | "文件存在" => {
                #[cfg(target_arch = "wasm32")]
                return Ok(Value::Bool(false));
                #[cfg(not(target_arch = "wasm32"))]
                {
                    let path = self.arg_str(&args, 0, "").replace('\\', "/");
                    return Ok(Value::Bool(std::path::Path::new(&path).exists()));
                }
            },
            "write_file" | "เขียนไฟล์" | "نوشتن_فایل" | "اكتب_الملف" | "כתוב_קובץ" | "فائل_لکھو" => {
                #[cfg(target_arch = "wasm32")]
                return Ok(Value::Unit);
                #[cfg(not(target_arch = "wasm32"))]
                {
                    let path = self.arg_str(&args, 0, "").replace('\\', "/");
                    let content = self.arg_str(&args, 1, "");
                    if let Some(parent) = std::path::Path::new(&path).parent() {
                        if !parent.as_os_str().is_empty() {
                            let _ = std::fs::create_dir_all(parent);
                        }
                    }
                    std::fs::write(&path, content.as_bytes())
                        .map_err(|e| EvalErr::from(format!("write_file '{path}': {e}")))?;
                    return Ok(Value::Unit);
                }
            },
            "print_file" | "พิมพ์ไฟล์" | "چاپ_فایل" | "اطبع_الملف" | "הדפס_קובץ" | "فائل_چھاپو" => {
                let content = self.arg_str(&args, 0, "");
                print!("{content}");
                return Ok(Value::Unit);
            },

            // ── CLI arguments ─────────────────────────────────────────────────
            "get_args" | "รับอาร์กิวเมนต์" | "دریافت_آرگومان‌ها" | "اجلب_المعاملات" | "קבל_ארגומנטים" | "دلائل_حاصل_کرو" => {
                let v: Vec<Value> = std::env::args().map(Value::Str).collect();
                return Ok(Value::List(Rc::new(v)));
            },

            // ── Filesystem: directory walking, stat, content hashing (native) ──
            // These power headless batch tools (asset pipelines, indexers). Errors
            // degrade gracefully (empty list / 0 / "") so a walk never aborts on one
            // unreadable entry.
            #[cfg(not(target_arch = "wasm32"))]
            "list_dir" | "รายการไดเรกทอรี" | "فهرست_پوشه" | "اسرد_المجلد" | "רשום_תיקייה" | "فولڈر_فہرست" => {
                let path = self.arg_str(&args, 0, ".").replace('\\', "/");
                let mut paths: Vec<String> = Vec::new();
                if let Ok(rd) = std::fs::read_dir(&path) {
                    for e in rd.flatten() {
                        paths.push(e.path().to_string_lossy().replace('\\', "/"));
                    }
                }
                paths.sort();
                let out: Vec<Value> = paths.into_iter().map(Value::Str).collect();
                return Ok(Value::List(Rc::new(out)));
            },
            #[cfg(not(target_arch = "wasm32"))]
            "is_dir" | "เป็นไดเรกทอรี" | "آیا_پوشه_است" | "هل_مجلد" | "האם_תיקייה" | "کیا_فولڈر_ہے" => {
                let path = self.arg_str(&args, 0, "").replace('\\', "/");
                return Ok(Value::Bool(std::path::Path::new(&path).is_dir()));
            },
            #[cfg(not(target_arch = "wasm32"))]
            "is_file" | "เป็นไฟล์" | "آیا_فایل_است" | "هل_ملف" | "האם_קובץ" | "کیا_فائل_ہے" => {
                let path = self.arg_str(&args, 0, "").replace('\\', "/");
                return Ok(Value::Bool(std::path::Path::new(&path).is_file()));
            },
            #[cfg(not(target_arch = "wasm32"))]
            "path_name" | "ชื่อไฟล์" | "نام_مسیر" | "اسم_المسار" | "שם_נתיב" | "پاتھ_نام" => {
                let path = self.arg_str(&args, 0, "").replace('\\', "/");
                let name = std::path::Path::new(&path)
                    .file_name()
                    .map(|s| s.to_string_lossy().into_owned())
                    .unwrap_or_default();
                return Ok(Value::Str(name));
            },
            #[cfg(not(target_arch = "wasm32"))]
            "path_ext" | "นามสกุลไฟล์" | "پسوند_مسیر" | "امتداد_المسار" | "סיומת_נתיב" | "پاتھ_ایکسٹینشن" => {
                let path = self.arg_str(&args, 0, "").replace('\\', "/");
                let ext = std::path::Path::new(&path)
                    .extension()
                    .map(|s| s.to_string_lossy().to_lowercase())
                    .unwrap_or_default();
                return Ok(Value::Str(ext));
            },
            #[cfg(not(target_arch = "wasm32"))]
            "file_size" | "ขนาดไฟล์" | "اندازه_فایل" | "حجم_الملف" | "גודל_קובץ" | "فائل_سائز" => {
                let path = self.arg_str(&args, 0, "");
                let sz = std::fs::metadata(&path).map(|m| m.len()).unwrap_or(0);
                return Ok(Value::Number(sz as f64));
            },
            #[cfg(not(target_arch = "wasm32"))]
            "file_modified" | "เวลาที่แก้ไข" | "زمان_تغییر_فایل" | "وقت_تعديل_الملف" | "זמן_עדכון_קובץ" | "فائل_ترمیم_وقت" => {
                let path = self.arg_str(&args, 0, "");
                let secs = std::fs::metadata(&path)
                    .and_then(|m| m.modified())
                    .ok()
                    .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
                    .map(|d| d.as_secs_f64())
                    .unwrap_or(0.0);
                return Ok(Value::Number(secs));
            },
            #[cfg(not(target_arch = "wasm32"))]
            "file_created" | "เวลาที่สร้าง" | "زمان_ایجاد_فایل" | "وقت_إنشاء_الملف" | "זמן_יצירת_קובץ" | "فائل_تخلیق_وقت" => {
                let path = self.arg_str(&args, 0, "");
                let secs = std::fs::metadata(&path)
                    .ok()
                    .and_then(|m| m.created().or_else(|_| m.modified()).ok())
                    .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
                    .map(|d| d.as_secs_f64())
                    .unwrap_or(0.0);
                return Ok(Value::Number(secs));
            },
            #[cfg(not(target_arch = "wasm32"))]
            "make_dir" | "สร้างไดเรกทอรี" | "ساخت_پوشه" | "أنشئ_مجلدا" | "צור_תיקייה" | "فولڈر_بناؤ" => {
                let path = self.arg_str(&args, 0, "");
                return Ok(Value::Bool(std::fs::create_dir_all(&path).is_ok()));
            },
            // str_strip_prefix("Bearer x", "Bearer ") → "x" (unchanged if absent).
            "str_strip_prefix" | "ตัดคำนำหน้า" | "حذف_پیشوند" | "أزل_البادئة" | "הסר_קידומת" | "سابقہ_ہٹاؤ" => {
                let s = self.arg_str(&args, 0, "");
                let prefix = self.arg_str(&args, 1, "");
                return Ok(Value::Str(
                    s.strip_prefix(&prefix).unwrap_or(&s).to_string(),
                ));
            },
            // Classify a file by magic bytes: "gzip" | "zip" | "other" | "missing".
            // The build-verification gate: only real built archives may publish.
            #[cfg(not(target_arch = "wasm32"))]
            "file_magic" | "มายาไฟล์" | "امضای_فایل" | "توقيع_الملف" | "חתימת_קובץ" | "فائل_میجک" => {
                let path = self.arg_str(&args, 0, "");
                let kind = match std::fs::File::open(&path) {
                    Ok(mut f) => {
                        use std::io::Read;
                        let mut buf = [0u8; 4];
                        let n = f.read(&mut buf).unwrap_or(0);
                        if n >= 2 && buf[0] == 0x1f && buf[1] == 0x8b {
                            "gzip"
                        } else if n >= 4 && &buf[0..2] == b"PK" {
                            "zip"
                        } else {
                            "other"
                        }
                    },
                    Err(_) => "missing",
                };
                return Ok(Value::Str(kind.to_string()));
            },
            // Binary-safe file copy (backups): copy_file(src, dst) → bool.
            #[cfg(not(target_arch = "wasm32"))]
            "copy_file" | "คัดลอกไฟล์" | "کپی_فایل" | "انسخ_الملف" | "העתק_קובץ" | "فائل_کاپی" => {
                let src = self.arg_str(&args, 0, "");
                let dst = self.arg_str(&args, 1, "");
                if let Some(parent) = std::path::Path::new(&dst).parent() {
                    let _ = std::fs::create_dir_all(parent);
                }
                return Ok(Value::Bool(std::fs::copy(&src, &dst).is_ok()));
            },
            // ── Read a .tgz (gzip tarball): list file entries / read one file ──
            // Powers the GitHub-style "Code / Files" browser: tar_gz_list gives
            // the file tree, tar_gz_read pulls one file's text for the viewer.
            #[cfg(all(not(target_arch = "wasm32"), feature = "web"))]
            "tar_gz_list" | "รายการทาร์" | "فهرست_TAR_GZ" | "اسرد_TAR_GZ" | "רשום_TAR_GZ" | "TAR_GZ_فہرست" => {
                let path = self.arg_str(&args, 0, "");
                let mut names: Vec<String> = Vec::new();
                if let Ok(file) = std::fs::File::open(&path) {
                    let gz = flate2::read::GzDecoder::new(file);
                    let mut ar = tar::Archive::new(gz);
                    if let Ok(entries) = ar.entries() {
                        for entry in entries.flatten() {
                            if entry.header().entry_type().is_file() {
                                if let Ok(p) = entry.path() {
                                    names.push(
                                        p.to_string_lossy()
                                            .trim_start_matches("./")
                                            .replace('\\', "/"),
                                    );
                                }
                            }
                        }
                    }
                }
                names.sort();
                names.dedup();
                let out: Vec<Value> = names.into_iter().map(Value::Str).collect();
                return Ok(Value::List(Rc::new(out)));
            },
            // tar_gz_read(archive, entry) → that file's text (utf-8 lossy,
            // capped at 256 KiB). Only reads entries that exist in the archive,
            // so a caller can't traverse outside it. "" if not found/unreadable.
            #[cfg(all(not(target_arch = "wasm32"), feature = "web"))]
            "tar_gz_read" | "อ่านทาร์" | "خواندن_TAR_GZ" | "اقرأ_TAR_GZ" | "קרא_TAR_GZ" | "TAR_GZ_پڑھو" => {
                let path = self.arg_str(&args, 0, "");
                let want = self.arg_str(&args, 1, "");
                let want = want.trim_start_matches("./").replace('\\', "/");
                let mut content = String::new();
                if let Ok(file) = std::fs::File::open(&path) {
                    let gz = flate2::read::GzDecoder::new(file);
                    let mut ar = tar::Archive::new(gz);
                    if let Ok(entries) = ar.entries() {
                        for entry in entries.flatten() {
                            let mut entry = entry;
                            let name = match entry.path() {
                                Ok(p) => p
                                    .to_string_lossy()
                                    .trim_start_matches("./")
                                    .replace('\\', "/"),
                                Err(_) => continue,
                            };
                            if name == want {
                                use std::io::Read;
                                let mut buf = Vec::new();
                                let cap = 256 * 1024;
                                if entry.take(cap as u64 + 1).read_to_end(&mut buf).is_ok() {
                                    let slice = if buf.len() > cap { &buf[..cap] } else { &buf[..] };
                                    content = String::from_utf8_lossy(slice).into_owned();
                                }
                                break;
                            }
                        }
                    }
                }
                return Ok(Value::Str(content));
            },
            // ── TOTP (RFC 6238, HMAC-SHA1, 6 digits, 30s) for 2FA ──
            // Base32 secret compatible with Google Authenticator / Authy etc.
            // `base32_encode`/`totp_code`/`totp_check` only exist under this
            // same `feature = "web"` gate (see their definitions above) — a
            // build without it must skip these arms too, not just fail to
            // link; matches how `file_hash` etc. gate their own arms below.
            #[cfg(all(not(target_arch = "wasm32"), feature = "web"))]
            "totp_secret" | "โทเทนลับ" | "راز_TOTP" | "سر_TOTP" | "סוד_TOTP" | "TOTP_راز" => {
                let mut bytes = [0u8; 20];
                rand::RngCore::fill_bytes(&mut rand::rngs::OsRng, &mut bytes);
                return Ok(Value::Str(base32_encode(&bytes)));
            },
            // otpauth:// URI to paste into an authenticator app (or make a QR of).
            // No cfg gate needed — pure string formatting, no dependency on
            // the web-only TOTP helpers.
            "totp_uri" | "โทเทนยูอาร์ไอ" | "آدرس_TOTP" | "رابط_TOTP" | "כתובת_TOTP" | "TOTP_یو_آر_آئی" => {
                let secret = self.arg_str(&args, 0, "");
                let account = self.arg_str(&args, 1, "user");
                let issuer = self.arg_str(&args, 2, "lingfu");
                return Ok(Value::Str(format!(
                    "otpauth://totp/{issuer}:{account}?secret={secret}&issuer={issuer}&algorithm=SHA1&digits=6&period=30"
                )));
            },
            // Verify a 6-digit code against the secret, allowing ±1 time step.
            #[cfg(all(not(target_arch = "wasm32"), feature = "web"))]
            "totp_verify" | "โทเทนตรวจ" | "تایید_TOTP" | "تحقق_TOTP" | "אמת_TOTP" | "TOTP_تصدیق" => {
                let secret = self.arg_str(&args, 0, "");
                let code = self.arg_str(&args, 1, "");
                let ok = totp_check(&secret, code.trim());
                return Ok(Value::Bool(ok));
            },
            // The current valid code, for tests/tools.
            #[cfg(all(not(target_arch = "wasm32"), feature = "web"))]
            "totp_now" | "โทเทนตอนนี้" | "TOTP_اکنون" | "TOTP_الآن" | "TOTP_עכשיו" | "TOTP_ابھی" => {
                let secret = self.arg_str(&args, 0, "");
                let step = (crate::runtime::now_secs() as u64) / 30;
                return Ok(Value::Str(
                    totp_code(&secret, step).unwrap_or_default(),
                ));
            },
            // BLAKE3 hex of a file's bytes (binary-safe content fingerprint).
            #[cfg(not(target_arch = "wasm32"))]
            "file_hash" | "แฮชไฟล์" | "درهم_فایل" | "بصمة_الملف" | "גיבוב_קובץ" | "فائل_ہیش" => {
                let path = self.arg_str(&args, 0, "");
                match std::fs::read(&path) {
                    Ok(bytes) => {
                        return Ok(Value::Str(hex_encode(&ling_crypto::Blake3::hash(&bytes))))
                    },
                    Err(_) => return Ok(Value::Str(String::new())),
                }
            },
            // BLAKE3 hex of an arbitrary string (deterministic id/colour/role seed).
            "hash_hex" | "แฮชสตริง" | "درهم_هگزادسیمال" | "بصمة_سداسية" | "גיבוב_הקסדצימלי" | "ہیکس_ہیش" => {
                let s = self.arg_str(&args, 0, "");
                return Ok(Value::Str(hex_encode(&ling_crypto::Blake3::hash(
                    s.as_bytes(),
                ))));
            },
            // Read an environment variable, falling back to a default.
            #[cfg(not(target_arch = "wasm32"))]
            "env_get" | "รับตัวแปรแวดล้อม" | "دریافت_متغیر_محیطی" | "اجلب_متغير_البيئة" | "קבל_משתנה_סביבה" | "ماحولیاتی_متغیر_حاصل_کرو" => {
                let name = self.arg_str(&args, 0, "");
                let dflt = self.arg_str(&args, 1, "");
                return Ok(Value::Str(std::env::var(&name).unwrap_or(dflt)));
            },

            // ── String utilities ──────────────────────────────────────────────
            // Parses a string to a number (0 on failure — degrades gracefully,
            // like the other filesystem/parsing builtins in this file).
            "to_number" | "转数字" => {
                let s = self.arg_str(&args, 0, "");
                return Ok(Value::Number(s.trim().parse().unwrap_or(0.0)));
            },
            // Plain SHA-256 hex — matches the browser's native SubtleCrypto
            // digest("SHA-256", ...), which is what proof-of-work mining uses
            // client-side (Web Crypto has no Blake3/SHA-3, so this is the one
            // hash both sides can compute natively and fast).
            "sha256_hex" | "SHA256哈希" => {
                use sha2::Digest;
                let s = self.arg_str(&args, 0, "");
                let mut h = sha2::Sha256::new();
                h.update(s.as_bytes());
                return Ok(Value::Str(hex_encode(&h.finalize())));
            },
            // Parses a hex string (no "0x" prefix) to a number — `to_number`
            // uses Rust's plain f64 parser, which doesn't understand hex.
            "hex_to_number" | "十六进制转数字" => {
                let s = self.arg_str(&args, 0, "");
                let v = u64::from_str_radix(s.trim(), 16).unwrap_or(0);
                return Ok(Value::Number(v as f64));
            },
            "split" | "str_split" | "แยก" | "جداسازی" | "قسّم" | "פצל" | "تقسیم_کرو" => {
                let s = self.arg_str(&args, 0, "");
                let sep = self.arg_str(&args, 1, "\n");
                let sep = if sep.is_empty() { "\n".into() } else { sep };
                let parts: Vec<Value> = s
                    .split(sep.as_str())
                    .map(|p| Value::Str(p.to_string()))
                    .collect();
                return Ok(Value::List(Rc::new(parts)));
            },
            "trim" | "str_trim" | "ตัดช่องว่าง" | "حذف_فاصله" | "اقتطع_الفراغات" | "חתוך_רווחים" | "خالی_جگہ_کاٹو" => {
                let s = self.arg_str(&args, 0, "");
                return Ok(Value::Str(s.trim().to_string()));
            },
            "starts_with" | "str_starts_with" | "เริ่มด้วย" | "شروع_می‌شود_با" | "يبدأ_بـ" | "מתחיל_ב" | "شروع_ہوتا_ہے" => {
                let s = self.arg_str(&args, 0, "");
                let prefix = self.arg_str(&args, 1, "");
                return Ok(Value::Bool(s.starts_with(prefix.as_str())));
            },
            "ends_with" | "str_ends_with" | "ลงท้ายด้วย" | "پایان_می‌یابد_با" | "ينتهي_بـ" | "מסתיים_ב" | "ختم_ہوتا_ہے" => {
                let s = self.arg_str(&args, 0, "");
                let suffix = self.arg_str(&args, 1, "");
                return Ok(Value::Bool(s.ends_with(suffix.as_str())));
            },
            "str_replace" | "แทนสตริง" | "جایگزینی_رشته" | "استبدل_النص" | "החלף_מחרוזת" | "اسٹرنگ_تبدیل" => {
                let s = self.arg_str(&args, 0, "");
                let from = self.arg_str(&args, 1, "");
                let to = self.arg_str(&args, 2, "");
                return Ok(Value::Str(s.replace(from.as_str(), to.as_str())));
            },
            "str_find" | "หาในสตริง" | "جستجوی_رشته" | "ابحث_في_النص" | "חפש_מחרוזת" | "اسٹرنگ_تلاش" => {
                let s = self.arg_str(&args, 0, "");
                let needle = self.arg_str(&args, 1, "");
                // Return char index (not byte index) for consistency with substr
                let pos = s
                    .find(needle.as_str())
                    .map(|byte_i| s[..byte_i].chars().count() as f64)
                    .unwrap_or(-1.0);
                return Ok(Value::Number(pos));
            },
            "substr" | "str_slice" | "ส่วนสตริง" | "زیررشته" | "جزء_النص" | "תת_מחרוזת" | "ذیلی_اسٹرنگ" => {
                let s = self.arg_str(&args, 0, "");
                let start = self.arg_num(&args, 1, 0.0)? as usize;
                let len = args
                    .get(2)
                    .map(|v| self.to_number(v).unwrap_or(999999.0) as usize)
                    .unwrap_or_else(|| s.chars().count().saturating_sub(start));
                let chars: Vec<char> = s.chars().collect();
                let end = (start + len).min(chars.len());
                let slice: String = chars.get(start..end).unwrap_or(&[]).iter().collect();
                return Ok(Value::Str(slice));
            },
            "to_str" | "str" | "num_str" | "แปลงสตริง" | "تبدیل_به_رشته" | "حوّل_لنص" | "המר_למחרוזת" | "اسٹرنگ_میں_بدلو" => {
                let v = args.into_iter().next().unwrap_or(Value::Unit);
                return Ok(Value::Str(v.to_string()));
            },
            "str_repeat" | "ทำซ้ำสตริง" | "تکرار_رشته" | "كرّر_النص" | "חזור_על_מחרוזת" | "اسٹرنگ_دہراؤ" => {
                let s = self.arg_str(&args, 0, "");
                let n = self.arg_num(&args, 1, 1.0)? as usize;
                return Ok(Value::Str(s.repeat(n)));
            },
            "str_upper" => {
                let s = self.arg_str(&args, 0, "");
                return Ok(Value::Str(s.to_uppercase()));
            },
            "str_lower" => {
                let s = self.arg_str(&args, 0, "");
                return Ok(Value::Str(s.to_lowercase()));
            },
            "str_len" | "len" | "ความยาว" | "长度" | "長さ" | "길이" | "طول_رشته" | "طول_النص" | "אורך_מחרוזת" | "اسٹرنگ_لمبائی" => {
                match args.first() {
                    Some(Value::Str(s)) => return Ok(Value::Number(s.chars().count() as f64)),
                    Some(Value::List(v)) => return Ok(Value::Number(v.len() as f64)),
                    _ => return Ok(Value::Number(0.0)),
                }
            },

            // ── FNV-1a hash (deterministic, normalized 0.0–1.0) ──────────────
            "hash_str" | "แฮช" | "درهم_رشته" | "بصمة_نص" | "גיבוב_מחרוזת" | "اسٹرنگ_ہیش" => {
                let s = self.arg_str(&args, 0, "");
                let mut h: u64 = 14695981039346656037_u64;
                for b in s.bytes() {
                    h ^= b as u64;
                    h = h.wrapping_mul(1099511628211);
                }
                return Ok(Value::Number((h & 0xFFFFFF) as f64 / 16777215.0));
            },
            "hash_int" | "แฮชจำนวน" | "درهم_عدد" | "بصمة_عدد" | "גיבוב_מספר" | "نمبر_ہیش" => {
                let s = self.arg_str(&args, 0, "");
                let n = self.arg_num(&args, 1, 100.0)? as u64;
                let mut h: u64 = 14695981039346656037_u64;
                for b in s.bytes() {
                    h ^= b as u64;
                    h = h.wrapping_mul(1099511628211);
                }
                return Ok(Value::Number((h % n.max(1)) as f64));
            },

            // ── List utilities ────────────────────────────────────────────────
            "list_new" | "รายการใหม่" | "新建列表" | "新規リスト" | "새목록" | "فهرست_جدید" | "قائمة_جديدة" | "רשימה_חדשה" | "نئی_فہرست" | "nouvelle_liste" | "neue_liste" | "новый_список" =>
            {
                return Ok(Value::List(Rc::new(Vec::new())));
            },
            "list_push" | "เพิ่มรายการ" | "列表添加" | "リスト追加" | "목록추가" | "افزودن_به_فهرست" | "أضف_للقائمة" | "הוסף_לרשימה" | "فہرست_میں_شامل_کرو" | "ajouter_liste" | "liste_anhängen" | "добавить_в_список" =>
            {
                let lst = args
                    .first()
                    .cloned()
                    .unwrap_or(Value::List(Rc::new(vec![])));
                let val = args.get(1).cloned().unwrap_or(Value::Unit);
                if let Value::List(mut v) = lst {
                    Rc::make_mut(&mut v).push(val);
                    return Ok(Value::List(v));
                }
                return Ok(Value::List(Rc::new(vec![val])));
            },
            "list_get" | "รับรายการ" | "取元素" | "要素取得" | "요소가져오기" | "دریافت_از_فهرست" | "اجلب_من_القائمة" | "קבל_מרשימה" | "فہرست_سے_حاصل_کرو" | "obtenir_liste" | "liste_abrufen" | "получить_из_списка" =>
            {
                // Borrow the list; clone only the element (was cloning the whole list).
                let i = self.arg_num(&args, 1, 0.0)? as usize;
                if let Some(Value::List(v)) = args.first() {
                    return Ok(v.get(i).cloned().unwrap_or(Value::Str(String::new())));
                }
                return Ok(Value::Str(String::new()));
            },
            // list_max(numbers, default) / list_min(numbers, default) — `default`
            // is returned for an empty list (there's no numeric identity element
            // to fall back to otherwise).
            "list_max" | "列表最大值" => {
                let lst = args.first().cloned().unwrap_or(Value::List(Rc::new(vec![])));
                let default = self.arg_num(&args, 1, 0.0)?;
                if let Value::List(v) = lst {
                    let mut best = default;
                    let mut any = false;
                    for item in v.iter() {
                        if let Value::Number(n) = item {
                            if !any || *n > best {
                                best = *n;
                                any = true;
                            }
                        }
                    }
                    return Ok(Value::Number(best));
                }
                return Ok(Value::Number(default));
            },
            "list_min" | "列表最小值" => {
                let lst = args.first().cloned().unwrap_or(Value::List(Rc::new(vec![])));
                let default = self.arg_num(&args, 1, 0.0)?;
                if let Value::List(v) = lst {
                    let mut best = default;
                    let mut any = false;
                    for item in v.iter() {
                        if let Value::Number(n) = item {
                            if !any || *n < best {
                                best = *n;
                                any = true;
                            }
                        }
                    }
                    return Ok(Value::Number(best));
                }
                return Ok(Value::Number(default));
            },
            // list_set(lst, idx, val) → new list with index replaced. Engine builtin
            // (O(n) one copy) to replace the O(n²) ling `ตั้งรายการ` that looped
            // list_push + list_get (each of which copied the whole list).
            "list_set" | "ตั้งรายการ" | "设元素" | "要素設定" | "요소설정" | "تنظیم_عنصر_فهرست" | "عيّن_عنصر_القائمة" | "קבע_איבר_רשימה" | "فہرست_سیٹ" =>
            {
                let idx = self.arg_num(&args, 1, 0.0)? as usize;
                let mut ai = args.into_iter();
                let lst = ai.next().unwrap_or(Value::List(Rc::new(vec![])));
                ai.next(); // skip idx
                let val = ai.next().unwrap_or(Value::Unit);
                if let Value::List(mut v) = lst {
                    if idx < v.len() {
                        Rc::make_mut(&mut v)[idx] = val;
                    }
                    return Ok(Value::List(v));
                }
                return Ok(Value::List(Rc::new(vec![])));
            },
            "list_join" | "join" | "รวมรายการ" | "连接" | "連結" | "연결" | "پیوستن_فهرست" | "اربط_القائمة" | "חבר_רשימה" | "فہرست_جوڑو" =>
            {
                let lst = args
                    .first()
                    .cloned()
                    .unwrap_or(Value::List(Rc::new(vec![])));
                let sep = args.get(1).map(|v| v.to_string()).unwrap_or_default();
                if let Value::List(v) = lst {
                    return Ok(Value::Str(
                        v.iter()
                            .map(|x| x.to_string())
                            .collect::<Vec<_>>()
                            .join(&sep),
                    ));
                }
                return Ok(Value::Str(String::new()));
            },
            // list_map/list_filter/list_find — take a closure. Necessary as real
            // builtins (not expressible in `.ling` itself): a bare-identifier call
            // `f(x)` where `f` is a local variable always resolves through
            // `call_named`, which only looks at top-level `fn` definitions by
            // design ("call-site locals are intentionally NOT visible to fns") —
            // so a closure held in a variable/parameter can't be invoked from
            // `.ling` source directly. These call it from the Rust side instead,
            // the same way `http_serve` already invokes route-handler closures.
            "list_map" | "映射列表" => {
                let lst = args.first().cloned().unwrap_or(Value::List(Rc::new(vec![])));
                let f = args.get(1).cloned().unwrap_or(Value::Unit);
                if let Value::List(v) = lst {
                    let mut out = Vec::with_capacity(v.len());
                    for item in v.iter() {
                        out.push(self.call_value(f.clone(), vec![item.clone()])?);
                    }
                    return Ok(Value::List(Rc::new(out)));
                }
                return Ok(Value::List(Rc::new(vec![])));
            },
            "list_filter" | "过滤列表" => {
                let lst = args.first().cloned().unwrap_or(Value::List(Rc::new(vec![])));
                let f = args.get(1).cloned().unwrap_or(Value::Unit);
                if let Value::List(v) = lst {
                    let mut out = Vec::new();
                    for item in v.iter() {
                        if matches!(self.call_value(f.clone(), vec![item.clone()])?, Value::Bool(true)) {
                            out.push(item.clone());
                        }
                    }
                    return Ok(Value::List(Rc::new(out)));
                }
                return Ok(Value::List(Rc::new(vec![])));
            },
            // First element for which `f` returns true, or Unit if none match.
            "list_find" | "查找列表" => {
                let lst = args.first().cloned().unwrap_or(Value::List(Rc::new(vec![])));
                let f = args.get(1).cloned().unwrap_or(Value::Unit);
                if let Value::List(v) = lst {
                    for item in v.iter() {
                        if matches!(self.call_value(f.clone(), vec![item.clone()])?, Value::Bool(true)) {
                            return Ok(item.clone());
                        }
                    }
                }
                return Ok(Value::Unit);
            },
            // blob_f32("<deflate+base64>") / blob_i32(...) — decode an embedded,
            // losslessly-compressed numeric blob into a list. Produced by
            // `ling convert`; lets converted assets carry geometry/PCM/etc. compactly.
            #[cfg(not(target_arch = "wasm32"))]
            "blob_f32" | "blob_i32" => {
                let s = self.arg_str(&args, 0, "");
                let is_i32 = name == "blob_i32";
                match decode_blob(&s) {
                    Ok(bytes) => {
                        let mut out = Vec::with_capacity(bytes.len() / 4);
                        for ch in bytes.chunks_exact(4) {
                            let arr = [ch[0], ch[1], ch[2], ch[3]];
                            let n = if is_i32 {
                                i32::from_le_bytes(arr) as f64
                            } else {
                                f32::from_le_bytes(arr) as f64
                            };
                            out.push(Value::Number(n));
                        }
                        return Ok(Value::List(Rc::new(out)));
                    },
                    Err(e) => {
                        eprintln!("blob decode failed: {e}");
                        return Ok(Value::List(Rc::new(vec![])));
                    },
                }
            },

            // ══════════════════════════════════════════════════════════════════
            // SVG EXPORT  (svg_begin / svg_rect / svg_circle / svg_line /
            //              svg_polyline / svg_text / svg_end / hsl_color)
            // Chinese aliases: 开始SVG 结束SVG SVG矩形 SVG圆形 SVG线段 SVG折线 SVG文本 HSL颜色
            // Thai aliases:    เริ่มSVG จบSVG SVGสี่เหลี่ยม SVGวงกลม SVGเส้น SVGเส้นหัก SVGข้อความ สีHSL
            // ══════════════════════════════════════════════════════════════════
            "svg_begin" | "开始SVG" | "เริ่มSVG" | "شروع_SVG" | "ابدأ_SVG" | "התחל_SVG" | "SVG_شروع" | "commencer_svg" | "svg_beginnen" | "начать_svg" => {
                let path = self.arg_str(&args, 0, "output.svg");
                let width = self.arg_num(&args, 1, 800.0)?;
                let height = self.arg_num(&args, 2, 600.0)?;
                *self.svg.borrow_mut() = Some(SvgWriter::new(path, width, height));
                return Ok(Value::Unit);
            },

            "svg_rect" | "SVG矩形" | "SVGสี่เหลี่ยม" | "مستطیل_SVG" | "مستطيل_SVG" | "מלבן_SVG" | "SVG_مستطیل" | "rectangle_svg" | "svg_rechteck" | "прямоугольник_svg" => {
                let x = self.arg_num(&args, 0, 0.0)?;
                let y = self.arg_num(&args, 1, 0.0)?;
                let w = self.arg_num(&args, 2, 10.0)?;
                let h = self.arg_num(&args, 3, 10.0)?;
                let fill = self.arg_str(&args, 4, "#ffffff");
                if let Some(svg) = self.svg.borrow_mut().as_mut() {
                    svg.elements.push(format!(
                        "<rect x=\"{x:.1}\" y=\"{y:.1}\" width=\"{w:.1}\" \
                         height=\"{h:.1}\" fill=\"{fill}\"/>"
                    ));
                }
                return Ok(Value::Unit);
            },

            "svg_circle" | "SVG圆形" | "SVGวงกลม" | "دایره_SVG" | "دائرة_SVG" | "עיגול_SVG" | "SVG_دائرہ" | "cercle_svg" | "svg_kreis" | "круг_svg" => {
                let cx = self.arg_num(&args, 0, 0.0)?;
                let cy = self.arg_num(&args, 1, 0.0)?;
                let r = self.arg_num(&args, 2, 5.0)?;
                let fill = self.arg_str(&args, 3, "#ffffff");
                if let Some(svg) = self.svg.borrow_mut().as_mut() {
                    svg.elements.push(format!(
                        "<circle cx=\"{cx:.1}\" cy=\"{cy:.1}\" r=\"{r:.1}\" fill=\"{fill}\"/>"
                    ));
                }
                return Ok(Value::Unit);
            },

            "svg_line" | "SVG线段" | "SVGเส้น" | "خط_SVG" | "קו_SVG" | "SVG_لکیر" | "ligne_svg" | "svg_linie" | "линия_svg" => {
                let x1 = self.arg_num(&args, 0, 0.0)?;
                let y1 = self.arg_num(&args, 1, 0.0)?;
                let x2 = self.arg_num(&args, 2, 0.0)?;
                let y2 = self.arg_num(&args, 3, 0.0)?;
                let stroke = self.arg_str(&args, 4, "#ffffff");
                let sw = self.arg_num(&args, 5, 1.0)?;
                if let Some(svg) = self.svg.borrow_mut().as_mut() {
                    svg.elements.push(format!(
                        "<line x1=\"{x1:.1}\" y1=\"{y1:.1}\" x2=\"{x2:.1}\" y2=\"{y2:.1}\" \
                         stroke=\"{stroke}\" stroke-width=\"{sw:.1}\"/>"
                    ));
                }
                return Ok(Value::Unit);
            },

            "svg_polyline" | "SVG折线" | "SVGเส้นหัก" | "چندخطی_SVG" | "خط_متعدد_SVG" | "קו_שבור_SVG" | "SVG_پولی_لائن" | "polyligne_svg" | "svg_polylinie" | "ломаная_svg" => {
                let pts = self.arg_str(&args, 0, "");
                let stroke = self.arg_str(&args, 1, "#ffffff");
                let sw = self.arg_num(&args, 2, 1.0)?;
                if let Some(svg) = self.svg.borrow_mut().as_mut() {
                    svg.elements.push(format!(
                        "<polyline points=\"{pts}\" fill=\"none\" \
                         stroke=\"{stroke}\" stroke-width=\"{sw:.1}\"/>"
                    ));
                }
                return Ok(Value::Unit);
            },

            "svg_text" | "SVG文本" | "SVGข้อความ" | "متن_SVG" | "نص_SVG" | "טקסט_SVG" | "SVG_متن" | "texte_svg" | "текст_svg" => {
                let x = self.arg_num(&args, 0, 0.0)?;
                let y = self.arg_num(&args, 1, 0.0)?;
                let text = self.arg_str(&args, 2, "");
                let fill = self.arg_str(&args, 3, "#ffffff");
                let size = self.arg_num(&args, 4, 12.0)?;
                if let Some(svg) = self.svg.borrow_mut().as_mut() {
                    let safe = text
                        .replace('&', "&amp;")
                        .replace('<', "&lt;")
                        .replace('>', "&gt;");
                    svg.elements.push(format!(
                        "<text x=\"{x:.1}\" y=\"{y:.1}\" fill=\"{fill}\" \
                         font-family=\"monospace\" font-size=\"{size:.0}\">{safe}</text>"
                    ));
                }
                return Ok(Value::Unit);
            },

            "svg_end" | "结束SVG" | "จบSVG" | "پایان_SVG" | "أنهِ_SVG" | "סיים_SVG" | "SVG_ختم" | "terminer_svg" | "svg_beenden" | "закончить_svg" => {
                {
                    let borrow = self.svg.borrow();
                    if let Some(svg) = borrow.as_ref() {
                        svg.save()
                            .map_err(|e| EvalErr::from(format!("svg_end: {e}")))?;
                    }
                }
                *self.svg.borrow_mut() = None;
                return Ok(Value::Unit);
            },

            "hsl_color" | "HSL颜色" | "สีHSL" | "رنگ_HSL" | "لون_HSL" | "צבע_HSL" | "HSL_رنگ" | "couleur_hsl" | "hsl_farbe" | "цвет_hsl" => {
                let h = self.arg_num(&args, 0, 0.0)?;
                let s = self.arg_num(&args, 1, 70.0)?;
                let l = self.arg_num(&args, 2, 50.0)?;
                return Ok(Value::Str(hsl_to_hex(h, s, l)));
            },

            // ══════════════════════════════════════════════════════════════════
            // FFT / AUDIO ANALYSIS BUILTINS  (native only)
            // ══════════════════════════════════════════════════════════════════

            // fft_push(samples_list) — feed raw audio samples and run FFT
            #[cfg(not(target_arch = "wasm32"))]
            "fft_push" | "วิเคราะห์เสียง" | "频谱输入" | "FFT入力" | "FFT입력" | "ورودی_FFT" | "أدخل_FFT" | "הזנת_FFT" | "FFT_ان_پٹ" | "fft_entrée" | "fft_eingabe" | "fft_вход" =>
            {
                if let Some(Value::List(v)) = args.first() {
                    let samples: Vec<f32> = v
                        .iter()
                        .filter_map(|x| {
                            if let Value::Number(n) = x {
                                Some(*n as f32)
                            } else {
                                None
                            }
                        })
                        .collect();
                    self.fft.borrow_mut().push_samples(&samples);
                }
                return Ok(Value::Unit);
            },

            // fft_bands(n) → list of n log-spaced magnitude bands (0..1)
            #[cfg(not(target_arch = "wasm32"))]
            "fft_bands" | "แถบความถี่" | "频段" | "周波数帯" | "주파수대" | "باندهای_FFT" | "نطاقات_FFT" | "פסי_FFT" | "FFT_بینڈز" | "fft_bandes" | "fft_bänder" | "fft_полосы" =>
            {
                let n = self.arg_num(&args, 0, 32.0)? as usize;
                let bands = self.fft.borrow().freq_bands(n);
                *self.fft_bands_cache.borrow_mut() = bands.clone();
                return Ok(Value::List(Rc::new(
                    bands.into_iter().map(|v| Value::Number(v as f64)).collect(),
                )));
            },

            // fft_beat() → bool
            #[cfg(not(target_arch = "wasm32"))]
            "fft_beat" | "จังหวะเสียง" | "节拍检测" | "ビート検出" | "비트" | "ضرب_FFT" | "نبضة_FFT" | "פעימת_FFT" | "FFT_دھڑکن" | "fft_battement" | "fft_takt" | "fft_удар" =>
            {
                return Ok(Value::Bool(self.fft.borrow().is_beat()));
            },

            // fft_beat_ratio() → f64  (1.0 = at threshold, >1 = strong beat)
            #[cfg(not(target_arch = "wasm32"))]
            "fft_beat_ratio" | "อัตราจังหวะ" | "节拍比" | "ビート比" | "비트비율" | "نسبت_ضرب_FFT" | "نسبة_نبضة_FFT" | "יחס_פעימת_FFT" | "FFT_بیٹ_تناسب" =>
            {
                return Ok(Value::Number(self.fft.borrow().beat_ratio() as f64));
            },

            // fft_rms() → f64
            #[cfg(not(target_arch = "wasm32"))]
            "fft_rms" | "ระดับRMS" | "均方根" | "二乗平均" | "RMS레벨" | "RMS_صدا" | "جذر_متوسط_مربع_FFT" | "RMS_של_FFT" | "FFT_RMS" => {
                return Ok(Value::Number(self.fft.borrow().rms() as f64));
            },

            // fft_dominant_freq() → f64  in Hz
            #[cfg(not(target_arch = "wasm32"))]
            "fft_dominant_freq" | "ความถี่หลัก" | "主频" | "主要周波数" | "주파수" | "فرکانس_غالب" | "التردد_السائد" | "תדר_דומיננטי" | "غالب_فریکوئنسی" | "fft_fréquence_dominante" | "fft_dominante_frequenz" | "fft_доминирующая_частота" =>
            {
                return Ok(Value::Number(self.fft.borrow().dominant_freq() as f64));
            },

            // ── wasm32 stubs: fft builtins are no-ops on web ───────────────
            #[cfg(target_arch = "wasm32")]
            "fft_push" | "วิเคราะห์เสียง" | "频谱输入" | "FFT入力" | "FFT입력" | "ورودی_FFT" | "أدخل_FFT" | "הזנת_FFT" | "FFT_ان_پٹ" | "fft_entrée" | "fft_eingabe" | "fft_вход" =>
            {
                return Ok(Value::Unit);
            },
            #[cfg(target_arch = "wasm32")]
            "fft_bands" | "แถบความถี่" | "频段" | "周波数帯" | "주파수대" | "باندهای_FFT" | "نطاقات_FFT" | "פסי_FFT" | "FFT_بینڈز" | "fft_bandes" | "fft_bänder" | "fft_полосы" =>
            {
                let n = self.arg_num(&args, 0, 32.0)? as usize;
                return Ok(Value::List(vec![Value::Number(0.0); n].into()));
            },
            #[cfg(target_arch = "wasm32")]
            "fft_beat" | "จังหวะเสียง" | "节拍检测" | "ビート検出" | "비트" | "ضرب_FFT" | "نبضة_FFT" | "פעימת_FFT" | "FFT_دھڑکن" | "fft_battement" | "fft_takt" | "fft_удар" =>
            {
                return Ok(Value::Bool(false));
            },
            #[cfg(target_arch = "wasm32")]
            "fft_beat_ratio" | "อัตราจังหวะ" | "节拍比" | "ビート比" | "비트비율" | "نسبت_ضرب_FFT" | "نسبة_نبضة_FFT" | "יחס_פעימת_FFT" | "FFT_بیٹ_تناسب" =>
            {
                return Ok(Value::Number(1.0));
            },
            #[cfg(target_arch = "wasm32")]
            "fft_rms" | "ระดับRMS" | "均方根" | "二乗平均" | "RMS레벨" | "RMS_صدا" | "جذر_متوسط_مربع_FFT" | "RMS_של_FFT" | "FFT_RMS" => {
                return Ok(Value::Number(0.0));
            },
            #[cfg(target_arch = "wasm32")]
            "fft_dominant_freq" | "ความถี่หลัก" | "主频" | "主要周波数" | "주파수" | "فرکانس_غالب" | "التردد_السائد" | "תדר_דומיננטי" | "غالب_فریکوئنسی" | "fft_fréquence_dominante" | "fft_dominante_frequenz" | "fft_доминирующая_частота" =>
            {
                return Ok(Value::Number(0.0));
            },

            // ══════════════════════════════════════════════════════════════════
            // PROCEDURAL TEXTURE BLIT BUILTINS  (screen-space)
            // All: name(dst_x, dst_y, width, height, ...params, palette)
            // palette: "rainbow" | "fire" | "ocean" | "psychedelic" | "neon" | "forest"
            // ══════════════════════════════════════════════════════════════════

            // tex_checkerboard(x, y, w, h, tiles, r1,g1,b1, r2,g2,b2)
            "tex_checkerboard" | "ลายตารางหมากรุก" | "بافت_شطرنجی" | "نسيج_رقعة_الشطرنج" | "מרקם_שחמט" | "شطرنج_ٹیکسچر" => {
                let (tx, ty, tw, th) = self.tex_rect(&args)?;
                let tiles = self.arg_num(&args, 4, 8.0)? as u32;
                let (r1, g1, b1) = (
                    self.arg_num(&args, 5, 255.)? as u32,
                    self.arg_num(&args, 6, 255.)? as u32,
                    self.arg_num(&args, 7, 255.)? as u32,
                );
                let (r2, g2, b2) = (
                    self.arg_num(&args, 8, 0.)? as u32,
                    self.arg_num(&args, 9, 0.)? as u32,
                    self.arg_num(&args, 10, 0.)? as u32,
                );
                let c1 = (r1 << 16) | (g1 << 8) | b1;
                let c2 = (r2 << 16) | (g2 << 8) | b2;
                let mut gfx = self.gfx.borrow_mut();
                let (bw, bh) = (gfx.width, gfx.height);
                for row in 0..th {
                    for col in 0..tw {
                        let cx = col as u32 * tiles / tw as u32;
                        let cy = row as u32 * tiles / th as u32;
                        let (dx, dy) = (tx + col, ty + row);
                        if dx < bw && dy < bh {
                            gfx.buffer[dy * bw + dx] = if (cx + cy) % 2 == 0 { c1 } else { c2 };
                        }
                    }
                }
                return Ok(Value::Unit);
            },

            // tex_gradient(x, y, w, h, angle_deg, r1,g1,b1, r2,g2,b2)
            "tex_gradient" | "ลายไล่สี" | "بافت_گرادیان" | "نسيج_متدرج" | "מרקם_גרדיאנט" | "گریڈینٹ_ٹیکسچر" => {
                let (tx, ty, tw, th) = self.tex_rect(&args)?;
                let angle = self.arg_num(&args, 4, 0.0)? as f32;
                let (r1, g1, b1) = (
                    self.arg_num(&args, 5, 0.)? as f32 / 255.,
                    self.arg_num(&args, 6, 0.)? as f32 / 255.,
                    self.arg_num(&args, 7, 0.)? as f32 / 255.,
                );
                let (r2, g2, b2) = (
                    self.arg_num(&args, 8, 255.)? as f32 / 255.,
                    self.arg_num(&args, 9, 255.)? as f32 / 255.,
                    self.arg_num(&args, 10, 255.)? as f32 / 255.,
                );
                let (ca, sa) = (angle.to_radians().cos(), angle.to_radians().sin());
                let mut gfx = self.gfx.borrow_mut();
                let (bw, bh) = (gfx.width, gfx.height);
                for row in 0..th {
                    for col in 0..tw {
                        let nx = col as f32 / tw as f32 - 0.5;
                        let ny = row as f32 / th as f32 - 0.5;
                        let t = ((nx * ca + ny * sa + 0.707) / 1.414).clamp(0., 1.);
                        let (dx, dy) = (tx + col, ty + row);
                        if dx < bw && dy < bh {
                            gfx.buffer[dy * bw + dx] =
                                tex_rgb(r1 + (r2 - r1) * t, g1 + (g2 - g1) * t, b1 + (b2 - b1) * t);
                        }
                    }
                }
                return Ok(Value::Unit);
            },

            // tex_noise(x, y, w, h, scale, octaves, seed, palette)
            "tex_noise" | "ลายนอยส์" | "بافت_نویز" | "نسيج_ضجيج" | "מרקם_רעש" | "نوائز_ٹیکسچر" => {
                let (tx, ty, tw, th) = self.tex_rect(&args)?;
                let scale = self.arg_num(&args, 4, 4.0)? as f32;
                let octaves = self.arg_num(&args, 5, 4.0)? as u32;
                let seed = self.arg_num(&args, 6, 0.0)? as u32;
                let palette = self.arg_str(&args, 7, "rainbow");
                let mut gfx = self.gfx.borrow_mut();
                let (bw, bh) = (gfx.width, gfx.height);
                for row in 0..th {
                    for col in 0..tw {
                        let v = tex_fbm(
                            col as f32 * scale / tw as f32,
                            row as f32 * scale / th as f32,
                            octaves,
                            seed,
                        );
                        let [r, g, b] = tex_palette(&palette, v);
                        let (dx, dy) = (tx + col, ty + row);
                        if dx < bw && dy < bh {
                            gfx.buffer[dy * bw + dx] = tex_rgb(r, g, b);
                        }
                    }
                }
                return Ok(Value::Unit);
            },

            // tex_freq_map(x, y, w, h, time, speed, palette)
            // Uses bands written by the last fft_bands() call.
            "tex_freq_map" | "ลายความถี่" | "نقشه_فرکانس_بافت" | "خريطة_تردد_النسيج" | "מפת_תדר_מרקם" | "فریکوئنسی_میپ_ٹیکسچر" => {
                let (tx, ty, tw, th) = self.tex_rect(&args)?;
                let time = self.arg_num(&args, 4, 0.0)? as f32;
                let speed = self.arg_num(&args, 5, 0.3)? as f32;
                let palette = self.arg_str(&args, 6, "rainbow");
                let bands: Vec<f32> = {
                    let c = self.fft_bands_cache.borrow();
                    if c.is_empty() {
                        vec![0.0; 32]
                    } else {
                        c.clone()
                    }
                };
                let n = bands.len().max(1);
                let mut gfx = self.gfx.borrow_mut();
                let (bw, bh) = (gfx.width, gfx.height);
                for row in 0..th {
                    for col in 0..tw {
                        let band_idx = (col * n / tw.max(1)).min(n - 1);
                        let mag = bands[band_idx].clamp(0., 1.);
                        let fill_y = (mag * th as f32) as usize;
                        if row >= th.saturating_sub(fill_y) {
                            let t = (col as f32 / tw as f32 + time * speed) % 1.0;
                            let [r, g, b] = tex_palette(&palette, t);
                            let bright = mag * (1.0 - row as f32 / th as f32 * 0.5);
                            let (dx, dy) = (tx + col, ty + row);
                            if dx < bw && dy < bh {
                                gfx.buffer[dy * bw + dx] =
                                    tex_rgb(r * bright, g * bright, b * bright);
                            }
                        }
                    }
                }
                return Ok(Value::Unit);
            },

            // tex_spiral(x, y, w, h, freq, bands, time, palette)
            "tex_spiral" | "ลายเกลียวหมุน" | "بافت_مارپیچ" | "نسيج_حلزوني" | "מרקם_ספירלה" | "سرپیچ_ٹیکسچر" => {
                let (tx, ty, tw, th) = self.tex_rect(&args)?;
                let freq = self.arg_num(&args, 4, 5.0)? as f32;
                let n_bands = self.arg_num(&args, 5, 8.0)? as f32;
                let time = self.arg_num(&args, 6, 0.0)? as f32;
                let palette = self.arg_str(&args, 7, "rainbow");
                let mut gfx = self.gfx.borrow_mut();
                let (bw, bh) = (gfx.width, gfx.height);
                for row in 0..th {
                    for col in 0..tw {
                        let nx = col as f32 / tw as f32 - 0.5;
                        let ny = row as f32 / th as f32 - 0.5;
                        let r = (nx * nx + ny * ny).sqrt();
                        let theta = ny.atan2(nx);
                        let t = ((r * freq - theta / std::f32::consts::TAU + time * 0.5) * n_bands
                            % 1.0)
                            .abs();
                        let [cr, cg, cb] = tex_palette(&palette, t);
                        let (dx, dy) = (tx + col, ty + row);
                        if dx < bw && dy < bh {
                            gfx.buffer[dy * bw + dx] = tex_rgb(cr, cg, cb);
                        }
                    }
                }
                return Ok(Value::Unit);
            },

            // tex_ripple(x, y, w, h, freq, cx, cy, time, palette)
            "tex_ripple" | "ลายระลอก" | "بافت_موج" | "نسيج_تموج" | "מרקם_אדווה" | "ریپل_ٹیکسچر" => {
                let (tx, ty, tw, th) = self.tex_rect(&args)?;
                let freq = self.arg_num(&args, 4, 10.0)? as f32;
                let rcx = self.arg_num(&args, 5, 0.5)? as f32;
                let rcy = self.arg_num(&args, 6, 0.5)? as f32;
                let time = self.arg_num(&args, 7, 0.0)? as f32;
                let palette = self.arg_str(&args, 8, "ocean");
                let mut gfx = self.gfx.borrow_mut();
                let (bw, bh) = (gfx.width, gfx.height);
                for row in 0..th {
                    for col in 0..tw {
                        let nx = col as f32 / tw as f32 - rcx;
                        let ny = row as f32 / th as f32 - rcy;
                        let r = (nx * nx + ny * ny).sqrt();
                        let t = ((r * freq - time) % 1.0).abs();
                        let [cr, cg, cb] = tex_palette(&palette, t);
                        let (dx, dy) = (tx + col, ty + row);
                        if dx < bw && dy < bh {
                            gfx.buffer[dy * bw + dx] = tex_rgb(cr, cg, cb);
                        }
                    }
                }
                return Ok(Value::Unit);
            },

            // tex_mandelbrot(x, y, w, h, zoom, cx, cy, max_iter, palette)
            "tex_mandelbrot" | "ลายแมนเดลบรอต" | "بافت_ماندلبرو" | "نسيج_مانديلبروت" | "מרקם_מנדלברוט" | "مینڈل_بروٹ_ٹیکسچر" => {
                let (tx, ty, tw, th) = self.tex_rect(&args)?;
                let zoom = self.arg_num(&args, 4, 1.0)?;
                let mcx = self.arg_num(&args, 5, -0.5)?;
                let mcy = self.arg_num(&args, 6, 0.0)?;
                let max_iter = self.arg_num(&args, 7, 64.0)? as u32;
                let palette = self.arg_str(&args, 8, "psychedelic");
                let mut gfx = self.gfx.borrow_mut();
                let (bw, bh) = (gfx.width, gfx.height);
                for row in 0..th {
                    for col in 0..tw {
                        let zx0 = (col as f64 / tw as f64 - 0.5) / zoom + mcx;
                        let zy0 = (row as f64 / th as f64 - 0.5) / zoom + mcy;
                        let mut x = 0.0f64;
                        let mut y = 0.0f64;
                        let mut i = 0u32;
                        while i < max_iter && x * x + y * y < 4.0 {
                            let t = x * x - y * y + zx0;
                            y = 2.0 * x * y + zy0;
                            x = t;
                            i += 1;
                        }
                        let t = if i == max_iter {
                            0.0f32
                        } else {
                            (i as f32
                                - (x as f32 * x as f32 + y as f32 * y as f32).ln().ln()
                                    / 2.0f32.ln())
                                / max_iter as f32
                        };
                        let [cr, cg, cb] = tex_palette(&palette, t.clamp(0., 1.));
                        let (dx, dy) = (tx + col, ty + row);
                        if dx < bw && dy < bh {
                            gfx.buffer[dy * bw + dx] = tex_rgb(cr, cg, cb);
                        }
                    }
                }
                return Ok(Value::Unit);
            },

            // tex_julia(x, y, w, h, c_re, c_im, max_iter, palette)
            "tex_julia" | "ลายจูเลีย" | "بافت_ژولیا" | "نسيج_جوليا" | "מרקם_ג'וליה" | "جولیا_ٹیکسچر" => {
                let (tx, ty, tw, th) = self.tex_rect(&args)?;
                let c_re = self.arg_num(&args, 4, -0.7)?;
                let c_im = self.arg_num(&args, 5, 0.27)?;
                let max_iter = self.arg_num(&args, 6, 64.0)? as u32;
                let palette = self.arg_str(&args, 7, "neon");
                let mut gfx = self.gfx.borrow_mut();
                let (bw, bh) = (gfx.width, gfx.height);
                for row in 0..th {
                    for col in 0..tw {
                        let mut zx = (col as f64 / tw as f64 - 0.5) * 3.5;
                        let mut zy = (row as f64 / th as f64 - 0.5) * 3.5;
                        let mut i = 0u32;
                        while i < max_iter && zx * zx + zy * zy < 4.0 {
                            let t = zx * zx - zy * zy + c_re;
                            zy = 2.0 * zx * zy + c_im;
                            zx = t;
                            i += 1;
                        }
                        let t = i as f32 / max_iter as f32;
                        let [cr, cg, cb] = tex_palette(&palette, t);
                        let (dx, dy) = (tx + col, ty + row);
                        if dx < bw && dy < bh {
                            gfx.buffer[dy * bw + dx] = tex_rgb(cr, cg, cb);
                        }
                    }
                }
                return Ok(Value::Unit);
            },

            // tex_voronoi(x, y, w, h, cells, seed, palette)
            "tex_voronoi" | "ลายโวโรนอย" | "بافت_ورونوی" | "نسيج_فورونوي" | "מרקם_וורונוי" | "ورونوئی_ٹیکسچر" => {
                let (tx, ty, tw, th) = self.tex_rect(&args)?;
                let cells = self.arg_num(&args, 4, 16.0)? as u32;
                let seed = self.arg_num(&args, 5, 42.0)? as u32;
                let palette = self.arg_str(&args, 6, "rainbow");
                let pts: Vec<[f32; 2]> = (0..cells)
                    .map(|i| {
                        [
                            tex_hash(i as i32, 0, seed),
                            tex_hash(i as i32, 1, seed + 999),
                        ]
                    })
                    .collect();
                let mut gfx = self.gfx.borrow_mut();
                let (bw, bh) = (gfx.width, gfx.height);
                for row in 0..th {
                    for col in 0..tw {
                        let (fx, fy) = (col as f32 / tw as f32, row as f32 / th as f32);
                        let (min_d, nearest) = pts.iter().enumerate().fold(
                            (f32::MAX, 0usize),
                            |(d, idx), (i, &[cx, cy])| {
                                let dd = (fx - cx).powi(2) + (fy - cy).powi(2);
                                if dd < d {
                                    (dd, i)
                                } else {
                                    (d, idx)
                                }
                            },
                        );
                        let t = (nearest as f32 / cells as f32 + min_d * 4.0) % 1.0;
                        let [cr, cg, cb] = tex_palette(&palette, t);
                        let (dx, dy) = (tx + col, ty + row);
                        if dx < bw && dy < bh {
                            gfx.buffer[dy * bw + dx] = tex_rgb(cr, cg, cb);
                        }
                    }
                }
                return Ok(Value::Unit);
            },

            // tex_halftone(x, y, w, h, dot_size, time, palette)
            "tex_halftone" | "ลายฮาล์ฟโทน" | "بافت_نیم‌تن" | "نسيج_نصفي" | "מרקם_חצי_גוון" | "ہاف_ٹون_ٹیکسچر" => {
                let (tx, ty, tw, th) = self.tex_rect(&args)?;
                let dot_size = self.arg_num(&args, 4, 0.05)? as f32;
                let time = self.arg_num(&args, 5, 0.0)? as f32;
                let palette = self.arg_str(&args, 6, "rainbow");
                let mut gfx = self.gfx.borrow_mut();
                let (bw, bh) = (gfx.width, gfx.height);
                for row in 0..th {
                    for col in 0..tw {
                        let (fx, fy) = (col as f32 / tw as f32, row as f32 / th as f32);
                        let gx = (fx / dot_size).floor();
                        let gy = (fy / dot_size).floor();
                        let lx = (fx / dot_size - gx - 0.5) * 2.0;
                        let ly = (fy / dot_size - gy - 0.5) * 2.0;
                        let r = (lx * lx + ly * ly).sqrt();
                        let t = (gx / (1.0 / dot_size) + time * 0.1) % 1.0;
                        let a = if r < 0.7 {
                            ((0.7 - r) / 0.7).clamp(0., 1.)
                        } else {
                            0.0
                        };
                        if a > 0.0 {
                            let [cr, cg, cb] = tex_palette(&palette, t);
                            let (dx, dy) = (tx + col, ty + row);
                            if dx < bw && dy < bh {
                                gfx.buffer[dy * bw + dx] = tex_rgb(cr, cg, cb);
                            }
                        }
                    }
                }
                return Ok(Value::Unit);
            },

            // ══════════════════════════════════════════════════════════════════
            // RENDER / LIGHTING MODES  (holographic cel shading)
            // ══════════════════════════════════════════════════════════════════
            // set_shade_mode(m) — 0 flat · 1 cel · 2 holo (default)
            "set_shade_mode" | "设置着色" | "シェード設定" | "셰이드모드" | "ตั้งการแรเงา" | "تنظیم_حالت_سایه‌پردازی" | "عيّن_نمط_التظليل" | "קבע_מצב_הצללה" | "شیڈ_موڈ_مقرر_کرو" | "définir_mode_ombrage" | "schattierungsmodus_setzen" | "задать_режим_затенения" =>
            {
                let m = self.arg_num(&args, 0, 2.0)? as u8;
                self.gfx.borrow_mut().shade_mode = m;
                return Ok(Value::Unit);
            },
            // set_cel_bands(n) — number of posterisation bands (>=2)
            "set_cel_bands" | "设置色阶" | "セル段数" | "셀밴드" | "ตั้งระดับสี" | "تنظیم_باندهای_سل" | "عيّن_نطاقات_التظليل" | "קבע_רצועות_הצללה" | "سیل_بینڈز_مقرر_کرو" | "définir_bandes_cel" | "cel_bänder_setzen" | "задать_полосы_cel" =>
            {
                let n = (self.arg_num(&args, 0, 4.0)? as u32).max(2);
                self.gfx.borrow_mut().shade.bands = n;
                return Ok(Value::Unit);
            },
            // set_shadow_color(r,g,b) — coloured-shadow tint, 0-255
            "set_shadow_color" | "设置阴影色" | "影の色" | "그림자색" | "ตั้งสีเงา" | "تنظیم_رنگ_سایه" | "عيّن_لون_الظل" | "קבע_צבע_צל" | "سایہ_رنگ_مقرر_کرو" | "définir_couleur_ombre" | "schattenfarbe_setzen" | "задать_цвет_тени" =>
            {
                let r = self.arg_num(&args, 0, 26.)? as f32 / 255.0;
                let g = self.arg_num(&args, 1, 33.)? as f32 / 255.0;
                let b = self.arg_num(&args, 2, 77.)? as f32 / 255.0;
                self.gfx.borrow_mut().shade.shadow = [r, g, b];
                return Ok(Value::Unit);
            },
            // set_rim(strength, r,g,b) — holographic fresnel edge glow
            // ══════════════════════════════════════════════════════════════════
            // CRYPTOGRAPHY (ling-crypto) — geo suite, hybrid PQ KEM, holographic
            // Bytes cross the language boundary as lowercase hex strings.
            // ══════════════════════════════════════════════════════════════════
            #[cfg(not(target_arch = "wasm32"))]
            "crypto_hash" | "แฮชเข้ารหัส" | "几何哈希" | "幾何ハッシュ" | "기하해시" | "درهم_رمزنگاری" | "بصمة_تشفير" | "גיבוב_הצפנה" | "خفیہ_ہیش" | "hachage_crypto" | "krypto_hash" | "крипто_хеш" =>
            {
                let s = self.arg_str(&args, 0, "");
                return Ok(Value::Str(hex_encode(&ling_crypto::geo::holo_hash(
                    s.as_bytes(),
                ))));
            },
            #[cfg(target_arch = "wasm32")]
            "crypto_hash" | "แฮชเข้ารหัส" | "几何哈希" | "幾何ハッシュ" | "기하해시" | "درهم_رمزنگاری" | "بصمة_تشفير" | "גיבוב_הצפנה" | "خفیہ_ہیش" | "hachage_crypto" | "krypto_hash" | "крипто_хеш" =>
            {
                let s = self.arg_str(&args, 0, "");
                return Ok(Value::Str(hex_encode(&ling_crypto::geo::holo_hash(
                    s.as_bytes(),
                ))));
            },
            // 3-D torus-knot fingerprint of any text/key → flat [x,y,z, x,y,z, …]
            #[cfg(not(target_arch = "wasm32"))]
            "knot_points" | "จุดปม" | "结点坐标" | "結び目点" | "매듭점" | "نقاط_گره" | "نقاط_العقدة" | "נקודות_קשר" | "گرہ_پوائنٹس" | "points_nœud" | "knotenpunkte" | "точки_узла" => {
                let s = self.arg_str(&args, 0, "");
                let shape = ling_crypto::geo::KnotShape::from_bytes(s.as_bytes());
                let mut out = Vec::with_capacity(shape.points.len() * 3);
                for p in &shape.points {
                    out.push(Value::Number(p[0] as f64));
                    out.push(Value::Number(p[1] as f64));
                    out.push(Value::Number(p[2] as f64));
                }
                return Ok(Value::List(Rc::new(out)));
            },
            #[cfg(target_arch = "wasm32")]
            "knot_points" | "จุดปม" | "结点坐标" | "結び目点" | "매듭점" | "نقاط_گره" | "نقاط_العقدة" | "נקודות_קשר" | "گرہ_پوائنٹس" | "points_nœud" | "knotenpunkte" | "точки_узла" => {
                let s = self.arg_str(&args, 0, "");
                let shape = ling_crypto::geo::KnotShape::from_bytes(s.as_bytes());
                let mut out = Vec::with_capacity(shape.points.len() * 3);
                for p in &shape.points {
                    out.push(Value::Number(p[0] as f64));
                    out.push(Value::Number(p[1] as f64));
                    out.push(Value::Number(p[2] as f64));
                }
                return Ok(Value::List(out.into()));
            },
            #[cfg(not(target_arch = "wasm32"))]
            "knot_label" | "ป้ายปม" | "结点标签" | "結び目ラベル" | "매듭라벨" | "برچسب_گره" | "تسمية_العقدة" | "תווית_קשר" | "گرہ_لیبل" | "étiquette_nœud" | "knotenbezeichnung" | "метка_узла" =>
            {
                let s = self.arg_str(&args, 0, "");
                return Ok(Value::Str(
                    ling_crypto::geo::KnotShape::from_bytes(s.as_bytes()).label(),
                ));
            },
            #[cfg(target_arch = "wasm32")]
            "knot_label" | "ป้ายปม" | "结点标签" | "結び目ラベル" | "매듭라벨" | "برچسب_گره" | "تسمية_العقدة" | "תווית_קשר" | "گرہ_لیبل" | "étiquette_nœud" | "knotenbezeichnung" | "метка_узла" =>
            {
                let s = self.arg_str(&args, 0, "");
                return Ok(Value::Str(
                    ling_crypto::geo::KnotShape::from_bytes(s.as_bytes()).label(),
                ));
            },
            // KEM keypair (hybrid X25519+ML-KEM-768) → integer handle
            #[cfg(not(target_arch = "wasm32"))]
            "knot_keygen" | "hybrid_keygen" | "สร้างกุญแจปม" | "生成密钥" | "鍵生成" | "키생성" | "تولید_کلید_گره" | "توليد_مفتاح_العقدة" | "יצירת_מפתח_קשר" | "گرہ_کلید_تخلیق" | "génération_clé_nœud" | "knotenschlüsselerzeugung" | "генерация_ключа_узла" =>
            {
                self.crypto_ids.push(ling_crypto::KnotIdentity::generate());
                return Ok(Value::Number((self.crypto_ids.len() - 1) as f64));
            },
            #[cfg(not(target_arch = "wasm32"))]
            "knot_public" | "hybrid_public" | "กุญแจสาธารณะปม" | "公钥" | "公開鍵" | "공개키" | "کلید_عمومی_گره" | "مفتاح_العقدة_العام" | "מפתח_ציבורי_קשר" | "گرہ_عوامی_کلید" | "clé_publique_nœud" | "knoten_öffentlicher_schlüssel" | "публичный_ключ_узла" =>
            {
                let h = self.arg_num(&args, 0, 0.0)? as usize;
                let pk = self
                    .crypto_ids
                    .get(h)
                    .map(|id| hex_encode(id.public_key()))
                    .unwrap_or_default();
                return Ok(Value::Str(pk));
            },
            // encapsulate(pubkey_hex) → [ciphertext_hex, shared_secret_hex]
            #[cfg(not(target_arch = "wasm32"))]
            "knot_encapsulate"
            | "hybrid_encapsulate"
            | "ห่อกุญแจปม"
            | "封装密钥"
            | "カプセル化"
            | "캡슐화" | "کپسوله‌سازی_گره" | "تغليف_مفتاح_العقدة" | "עטיפת_קשר" | "گرہ_احاطہ" | "encapsuler_nœud" | "knoten_kapseln" | "инкапсулировать_узел" => {
                let pk = hex_decode(&self.arg_str(&args, 0, ""));
                match ling_crypto::geo::knot_encapsulate(&pk) {
                    Ok((ct, ss)) => {
                        return Ok(Value::List(Rc::new(vec![
                            Value::Str(hex_encode(&ct)),
                            Value::Str(hex_encode(&ss)),
                        ])))
                    },
                    Err(e) => return Ok(Value::Err(Box::new(Value::Str(e.to_string())))),
                }
            },
            // decapsulate(handle, ciphertext_hex) → shared_secret_hex
            #[cfg(not(target_arch = "wasm32"))]
            "knot_decapsulate"
            | "hybrid_decapsulate"
            | "แกะกุญแจปม"
            | "解封装密钥"
            | "カプセル解除"
            | "캡슐해제" | "بازکردن_کپسوله_گره" | "فك_تغليف_مفتاح_العقدة" | "פתיחת_עטיפת_קשר" | "گرہ_احاطہ_کھولو" | "décapsuler_nœud" | "knoten_entkapseln" | "декапсулировать_узел" => {
                let h = self.arg_num(&args, 0, 0.0)? as usize;
                let ct = hex_decode(&self.arg_str(&args, 1, ""));
                let ss = self
                    .crypto_ids
                    .get(h)
                    .and_then(|id| id.decapsulate(&ct).ok())
                    .map(|s| hex_encode(&s))
                    .unwrap_or_default();
                return Ok(Value::Str(ss));
            },
            // Authenticated encryption (XChaCha20-Poly1305) — seal(key_hex, text) → ct_hex
            #[cfg(not(target_arch = "wasm32"))]
            "crypto_seal" | "ผนึก" | "封印" | "封印する" | "봉인" | "مهر_رمزنگاری" | "ختم_تشفير" | "חתימת_הצפנה" | "خفیہ_مہر" | "sceller_crypto" | "krypto_versiegeln" | "запечатать_крипто" => {
                let key = hex_to_32(&self.arg_str(&args, 0, ""));
                let pt = self.arg_str(&args, 1, "");
                match ling_crypto::geo::holo_seal(key, pt.as_bytes()) {
                    Ok(ct) => return Ok(Value::Str(hex_encode(&ct))),
                    Err(e) => return Ok(Value::Err(Box::new(Value::Str(e.to_string())))),
                }
            },
            #[cfg(not(target_arch = "wasm32"))]
            "crypto_open" | "เปิดผนึก" | "解封" | "封印解除" | "봉인해제" | "بازکردن_مهر" | "فتح_الختم" | "פתיחת_חתימה" | "مہر_کھولو" | "ouvrir_crypto" | "krypto_öffnen" | "открыть_крипто" =>
            {
                let key = hex_to_32(&self.arg_str(&args, 0, ""));
                let ct = hex_decode(&self.arg_str(&args, 1, ""));
                match ling_crypto::geo::holo_open(key, &ct) {
                    Ok(pt) => return Ok(Value::Str(String::from_utf8_lossy(&pt).into_owned())),
                    Err(e) => return Ok(Value::Err(Box::new(Value::Str(e.to_string())))),
                }
            },
            // Holographic all-or-nothing transform — 4-D fragment coords [a,b,c,d, …]
            #[cfg(not(target_arch = "wasm32"))]
            "holo_points" | "จุดโฮโลแกรม" | "全息点" | "ホログラム点" | "홀로그램점" | "نقاط_هولوگرام" | "نقاط_الهولوغرام" | "נקודות_הולוגרמה" | "ہولوگرام_پوائنٹس" | "points_holo" | "holo_punkte" | "точки_голо" =>
            {
                let s = self.arg_str(&args, 0, "");
                let frags = ling_crypto::geo::scatter(s.as_bytes());
                let mut out = Vec::with_capacity(frags.len() * 4);
                for f in &frags {
                    for c in f.coord {
                        out.push(Value::Number(c as f64));
                    }
                }
                return Ok(Value::List(Rc::new(out)));
            },
            #[cfg(not(target_arch = "wasm32"))]
            "holo_fragment_count"
            | "จำนวนชิ้นโฮโลแกรม"
            | "全息碎片数"
            | "ホログラム断片数"
            | "홀로그램조각수" | "تعداد_قطعات_هولوگرام" | "عدد_شظايا_الهولوغرام" | "מספר_שברי_הולוגרמה" | "ہولوگرام_ٹکڑے_تعداد" | "nombre_fragments_holo" | "holo_fragmentanzahl" | "число_фрагментов_голо" => {
                let s = self.arg_str(&args, 0, "");
                return Ok(Value::Number(
                    ling_crypto::geo::scatter(s.as_bytes()).len() as f64
                ));
            },
            // SHAKE-256 XOF, squeezed to an arbitrary output length in bytes
            // (`shake_hex(s, 128)` = a 1024-bit seal digest).
            #[cfg(not(target_arch = "wasm32"))]
            "shake_hex" | "SHAKE哈希" => {
                let s = self.arg_str(&args, 0, "");
                let len = self.arg_num(&args, 1, 32.0)?.max(0.0) as usize;
                return Ok(Value::Str(hex_encode(&ling_crypto::Shake256::hash(s.as_bytes(), len))));
            },
            // Ed25519 signing keypair (issuer identity) → integer handle.
            #[cfg(not(target_arch = "wasm32"))]
            "ed25519_keygen" | "생성서명키" => {
                self.ed25519_ids.push(ling_crypto::Ed25519Keypair::generate());
                return Ok(Value::Number((self.ed25519_ids.len() - 1) as f64));
            },
            // Deterministic keypair from a 32-byte hex seed — the same seed
            // always yields the same keypair, so a program can persist just the
            // seed (e.g. a bank's issuer identity) and rederive identical keys
            // across restarts instead of every run minting a fresh, unrelated one.
            #[cfg(not(target_arch = "wasm32"))]
            "ed25519_keygen_from_seed" | "씨앗에서생성서명키" => {
                let seed = hex_to_32(&self.arg_str(&args, 0, ""));
                self.ed25519_ids.push(ling_crypto::Ed25519Keypair::from_seed(seed));
                return Ok(Value::Number((self.ed25519_ids.len() - 1) as f64));
            },
            #[cfg(not(target_arch = "wasm32"))]
            "ed25519_public" | "서명공개키" => {
                let h = self.arg_num(&args, 0, 0.0)? as usize;
                let pk = self
                    .ed25519_ids
                    .get(h)
                    .map(|kp| hex_encode(&kp.public_key()))
                    .unwrap_or_default();
                return Ok(Value::Str(pk));
            },
            // ed25519_sign(handle, message) → signature hex (64 bytes)
            #[cfg(not(target_arch = "wasm32"))]
            "ed25519_sign" | "서명하다" => {
                let h = self.arg_num(&args, 0, 0.0)? as usize;
                let msg = self.arg_str(&args, 1, "");
                let sig = self
                    .ed25519_ids
                    .get(h)
                    .map(|kp| hex_encode(&kp.sign(msg.as_bytes())))
                    .unwrap_or_default();
                return Ok(Value::Str(sig));
            },
            // ed25519_verify(pubkey_hex, message, signature_hex) → bool
            #[cfg(not(target_arch = "wasm32"))]
            "ed25519_verify" | "서명확인" => {
                let pk_hex = self.arg_str(&args, 0, "");
                let msg = self.arg_str(&args, 1, "");
                let sig_hex = self.arg_str(&args, 2, "");
                let pk_bytes = hex_decode(&pk_hex);
                let sig_bytes = hex_decode(&sig_hex);
                let ok = (|| {
                    let pk: [u8; 32] = pk_bytes.try_into().ok()?;
                    let sig: [u8; 64] = sig_bytes.try_into().ok()?;
                    Some(ling_crypto::Ed25519Keypair::verify(&pk, msg.as_bytes(), &sig).is_ok())
                })()
                .unwrap_or(false);
                return Ok(Value::Bool(ok));
            },
            // Argon2id password hashing — password_hash(pw) → PHC string,
            // password_verify(pw, phc_string) → bool.
            #[cfg(not(target_arch = "wasm32"))]
            "password_hash" | "비밀번호해시" => {
                let pw = self.arg_str(&args, 0, "");
                let hash = ling_crypto::Argon2idParams::default()
                    .hash_password(pw.as_bytes())
                    .unwrap_or_default();
                return Ok(Value::Str(hash));
            },
            #[cfg(not(target_arch = "wasm32"))]
            "password_verify" | "비밀번호확인" => {
                let pw = self.arg_str(&args, 0, "");
                let hash = self.arg_str(&args, 1, "");
                let ok = ling_crypto::Argon2idParams::verify_password(pw.as_bytes(), &hash).is_ok();
                return Ok(Value::Bool(ok));
            },
            // OS-CSPRNG random bytes as hex — session ids / nonces (not the
            // xorshift `rand` builtin, which is for game logic, not security).
            #[cfg(not(target_arch = "wasm32"))]
            "random_hex" | "무작위16진수" => {
                use rand::RngCore;
                let n = self.arg_num(&args, 0, 16.0)?.max(0.0) as usize;
                let mut buf = vec![0u8; n];
                rand::rngs::OsRng.fill_bytes(&mut buf);
                return Ok(Value::Str(hex_encode(&buf)));
            },
            // base64_encode(s) — text -> base64 (matches what canvas.toDataURL()
            // already produces client-side, so PNG uploads never need a binary
            // request body).
            #[cfg(not(target_arch = "wasm32"))]
            "base64_encode" | "base64인코딩" => {
                use base64::Engine as _;
                let s = self.arg_str(&args, 0, "");
                return Ok(Value::Str(
                    base64::engine::general_purpose::STANDARD.encode(s.as_bytes()),
                ));
            },
            // base64_decode_to_file(b64, path) — writes decoded bytes straight to
            // disk; returns true on success. The only way binary data (an
            // uploaded/rendered PNG) reaches the filesystem from `.ling` source.
            #[cfg(not(target_arch = "wasm32"))]
            "base64_decode_to_file" | "base64파일로저장" => {
                use base64::Engine as _;
                let b64 = self.arg_str(&args, 0, "");
                let path = self.arg_str(&args, 1, "");
                let b64 = b64
                    .split(',')
                    .next_back()
                    .unwrap_or(&b64); // tolerate a "data:image/png;base64,..." prefix
                let ok = base64::engine::general_purpose::STANDARD
                    .decode(b64)
                    .ok()
                    .and_then(|bytes| std::fs::write(&path, bytes).ok())
                    .is_some();
                return Ok(Value::Bool(ok));
            },
            // qr_svg(text) — a scannable QR code as an inline <svg>...</svg>
            // string (e.g. for an otpauth:// 2FA enrollment URI). Kept as SVG
            // rather than a rasterized image so it fits the same "everything
            // stays vector" theme as the banknote seal art.
            #[cfg(not(target_arch = "wasm32"))]
            "qr_svg" | "QR코드" => {
                let text = self.arg_str(&args, 0, "");
                let svg = qrcode::QrCode::new(text.as_bytes())
                    .map(|code| {
                        code.render::<qrcode::render::svg::Color>()
                            .min_dimensions(240, 240)
                            .dark_color(qrcode::render::svg::Color("#1a0f3d"))
                            .light_color(qrcode::render::svg::Color("#ffffff"))
                            .build()
                    })
                    .unwrap_or_default();
                return Ok(Value::Str(svg));
            },
            // zip_files(paths_list, out_path) — bundles files into a zip archive
            // (used by the "render" step to package a note's PNG/SVG/PDF).
            #[cfg(all(not(target_arch = "wasm32"), feature = "web"))]
            "zip_files" | "압축파일" => {
                let paths = match args.first() {
                    Some(Value::List(l)) => l.iter().map(|v| v.to_string()).collect::<Vec<_>>(),
                    _ => Vec::new(),
                };
                let out_path = self.arg_str(&args, 1, "out.zip");
                let ok = (|| -> std::io::Result<()> {
                    let file = std::fs::File::create(&out_path)?;
                    let mut writer = zip::ZipWriter::new(file);
                    let options: zip::write::FileOptions<'_, ()> = zip::write::FileOptions::default()
                        .compression_method(zip::CompressionMethod::Deflated);
                    for p in &paths {
                        let name = std::path::Path::new(p)
                            .file_name()
                            .map(|n| n.to_string_lossy().into_owned())
                            .unwrap_or_else(|| p.clone());
                        let bytes = std::fs::read(p)?;
                        writer.start_file(name, options)?;
                        std::io::Write::write_all(&mut writer, &bytes)?;
                    }
                    writer.finish()?;
                    Ok(())
                })()
                .is_ok();
                return Ok(Value::Bool(ok));
            },
            // pdf_from_images(png_paths_list, out_path) — one page per image,
            // sized to its pixel dimensions. No PDF crate dependency: `image`
            // (decode) and `flate2` (deflate the page's raw RGB stream) are
            // already unconditional deps, so this hand-writes the handful of
            // PDF objects (Catalog/Pages/Page/Contents/Image XObject) directly.
            // `build_pdf_from_images` itself only exists under this same gate.
            #[cfg(all(not(target_arch = "wasm32"), feature = "web"))]
            "pdf_from_images" | "PDF来自图片" => {
                let paths = match args.first() {
                    Some(Value::List(l)) => l.iter().map(|v| v.to_string()).collect::<Vec<_>>(),
                    _ => Vec::new(),
                };
                let out_path = self.arg_str(&args, 1, "out.pdf");
                let ok = build_pdf_from_images(&paths, &out_path).is_ok();
                return Ok(Value::Bool(ok));
            },

            // ══════════════════════════════════════════════════════════════════
            // ling-ui — animation easings + holographic vector widgets + text I/O
            // ══════════════════════════════════════════════════════════════════
            "ease" => {
                let name = self.arg_str(&args, 0, "ease");
                let t = self.arg_num(&args, 1, 0.0)? as f32;
                return Ok(Value::Number(
                    ling_ui::Easing::from_name(&name).apply(t) as f64
                ));
            },

            // ══════════════════════════════════════════════════════════════════
            // Anima — unified animation drivers (ling-animation). Organic 灵 +
            // mechanical 机 scalar drivers, callable per frame from a script.
            // ══════════════════════════════════════════════════════════════════
            "tween" | "补间" | "補間" | "트윈" | "แทรกค่า" | "میان‌فریم" | "تدرج_حركي" | "טווין" | "ٹوئین" | "твин" => {
                let a = self.arg_num(&args, 0, 0.0)?;
                let b = self.arg_num(&args, 1, 0.0)?;
                let t = self.arg_num(&args, 2, 0.0)?.clamp(0.0, 1.0);
                return Ok(Value::Number(a + (b - a) * t));
            },
            "tween_ease" | "缓动补间" | "緩和補間" | "이징트윈" | "แทรกนุ่ม" | "میان‌فریم_نرم" | "تدرج_ناعم_حركي" | "טווין_חלק" | "ٹوئین_ایز" | "tween_lisse" | "tween_glättung" | "твин_плавность" =>
            {
                let a = self.arg_num(&args, 0, 0.0)? as f32;
                let b = self.arg_num(&args, 1, 0.0)? as f32;
                let t = self.arg_num(&args, 2, 0.0)? as f32;
                let kind = self.arg_str(&args, 3, "linear");
                let e = ling_animation::EaseFunction::from_name(&kind);
                return Ok(Value::Number(
                    ling_animation::ease::tween_ease(&a, &b, t, e) as f64,
                ));
            },
            // ── Organic 灵 ──
            "breathe" | "呼吸" | "호흡" | "หายใจ" | "تنفس" | "נשימה" | "سانس" | "respirer" | "atmen" | "дышать" => {
                let t = self.arg_num(&args, 0, 0.0)? as f32;
                let rate = self.arg_num(&args, 1, 1.0)? as f32;
                let depth = self.arg_num(&args, 2, 0.1)? as f32;
                return Ok(Value::Number(
                    ling_animation::scalar::breathe(t, rate, depth) as f64,
                ));
            },
            "wobble" | "摆动" | "揺れ" | "흔들림" | "โยก" | "نوسان" | "تذبذب" | "תנודה" | "لرزش" | "osciller" | "wackeln" | "покачивание" => {
                let t = self.arg_num(&args, 0, 0.0)? as f32;
                let freq = self.arg_num(&args, 1, 1.0)? as f32;
                let amp = self.arg_num(&args, 2, 1.0)? as f32;
                let phase = self.arg_num(&args, 3, 0.0)? as f32;
                return Ok(Value::Number(
                    ling_animation::scalar::wobble(t, freq, amp, phase) as f64,
                ));
            },
            "gait_phase" | "步相" | "歩相" | "걸음위상" | "เฟสก้าว" | "فاز_گام" | "طور_المشية" | "שלב_הליכה" | "چال_مرحلہ" | "phase_démarche" | "gangphase" | "фаза_походки" => {
                let t = self.arg_num(&args, 0, 0.0)? as f32;
                let speed = self.arg_num(&args, 1, 1.0)? as f32;
                return Ok(Value::Number(
                    ling_animation::scalar::gait_phase(t, speed) as f64
                ));
            },
            "gait_swing" | "步摆" | "歩振り" | "걸음흔들" | "ก้าวแกว่ง" | "نوسان_گام" | "أرجحة_المشية" | "נדנוד_הליכה" | "چال_جھولا" | "balancement_démarche" | "gangschwung" | "мах_походки" =>
            {
                let t = self.arg_num(&args, 0, 0.0)? as f32;
                let speed = self.arg_num(&args, 1, 1.0)? as f32;
                let stride = self.arg_num(&args, 2, 1.0)? as f32;
                return Ok(Value::Number(
                    ling_animation::scalar::gait_swing(t, speed, stride) as f64,
                ));
            },
            "gait_lift" | "抬脚" | "足上げ" | "발들기" | "ยกเท้า" | "بلندشدن_گام" | "رفع_المشية" | "הרמת_הליכה" | "چال_اٹھاؤ" | "levée_démarche" | "ganghub" | "подъём_походки" => {
                let t = self.arg_num(&args, 0, 0.0)? as f32;
                let speed = self.arg_num(&args, 1, 1.0)? as f32;
                let height = self.arg_num(&args, 2, 1.0)? as f32;
                return Ok(Value::Number(
                    ling_animation::scalar::gait_lift(t, speed, height) as f64,
                ));
            },
            "spring_to" | "弹向" | "バネ寄せ" | "스프링이동" | "สปริงไป" | "فنر_به‌سوی" | "نابض_إلى" | "קפיץ_אל" | "اسپرنگ_تک" | "ressort_vers" | "feder_zu" | "пружина_к" =>
            {
                let pos = self.arg_num(&args, 0, 0.0)? as f32;
                let vel = self.arg_num(&args, 1, 0.0)? as f32;
                let target = self.arg_num(&args, 2, 0.0)? as f32;
                let stiffness = self.arg_num(&args, 3, 120.0)? as f32;
                let damping = self.arg_num(&args, 4, 14.0)? as f32;
                let dt = self.arg_num(&args, 5, 1.0 / 60.0)? as f32;
                let (np, nv) =
                    ling_animation::scalar::spring_step(pos, vel, target, stiffness, damping, dt);
                return Ok(Value::List(Rc::new(vec![
                    Value::Number(np as f64),
                    Value::Number(nv as f64),
                ])));
            },
            "ik2" | "反解" | "逆運動" | "역운동" | "ไอเค2" | "سینماتیک_معکوس2" | "حركية_عكسية2" | "קינמטיקה_הפוכה2" | "آئی_کے2" | "cinématique_inverse2" | "inverse_kinematik2" | "обратная_кинематика2" => {
                let l1 = self.arg_num(&args, 0, 1.0)? as f32;
                let l2 = self.arg_num(&args, 1, 1.0)? as f32;
                let tx = self.arg_num(&args, 2, 0.0)? as f32;
                let ty = self.arg_num(&args, 3, 0.0)? as f32;
                let (sh, el) = ling_animation::scalar::two_bone_ik(l1, l2, tx, ty);
                return Ok(Value::List(Rc::new(vec![
                    Value::Number(sh as f64),
                    Value::Number(el as f64),
                ])));
            },
            // ── Mechanical 机 ──
            "gear_couple" | "齿轮联动" | "歯車連動" | "기어연동" | "เฟืองทด" | "جفت_چرخ‌دنده" | "اقتران_التروس" | "צימוד_גלגלי_שיניים" | "گیئر_جوڑا" | "accoupler_engrenage" | "zahnrad_koppeln" | "сцепить_шестерни" =>
            {
                let angle = self.arg_num(&args, 0, 0.0)? as f32;
                let ti = self.arg_num(&args, 1, 1.0)? as f32;
                let to = self.arg_num(&args, 2, 1.0)? as f32;
                return Ok(Value::Number(
                    ling_animation::scalar::gear(angle, ti, to) as f64
                ));
            },
            "gear_train" | "齿轮组" | "歯車列" | "기어열" | "ชุดเฟือง" | "مجموعه_چرخ‌دنده" | "قطار_التروس" | "שרשרת_גלגלי_שיניים" | "گیئر_ٹرین" | "train_engrenages" | "zahnradgetriebe" | "передача_шестерён" => {
                let angle = self.arg_num(&args, 0, 0.0)? as f32;
                let teeth: Vec<f32> = match args.get(1) {
                    Some(Value::List(items)) => items
                        .iter()
                        .filter_map(|v| {
                            if let Value::Number(n) = v {
                                Some(*n as f32)
                            } else {
                                None
                            }
                        })
                        .collect(),
                    _ => Vec::new(),
                };
                let out = ling_animation::mechanism::gear_train(angle, &teeth);
                return Ok(Value::List(Rc::new(
                    out.into_iter().map(|a| Value::Number(a as f64)).collect(),
                )));
            },
            "cam_lift" | "凸轮升程" | "カム揚程" | "캠리프트" | "ยกลูกเบี้ยว" | "بلندشدن_بادامک" | "رفع_الكامة" | "הרמת_קאם" | "کیم_اٹھاؤ" | "levée_came" | "nockenhub" | "подъём_кулачка" =>
            {
                let angle = self.arg_num(&args, 0, 0.0)? as f32;
                let lift = self.arg_num(&args, 1, 1.0)? as f32;
                return Ok(Value::Number(
                    ling_animation::scalar::cam_lift(angle, lift) as f64
                ));
            },
            "piston" | "活塞" | "ピストン" | "피스톤" | "ลูกสูบ" | "پیستون" | "مكبس" | "בוכנה" | "پسٹن" | "kolben" | "поршень" => {
                let angle = self.arg_num(&args, 0, 0.0)? as f32;
                let crank = self.arg_num(&args, 1, 1.0)? as f32;
                let rod = self.arg_num(&args, 2, 2.0)? as f32;
                return Ok(Value::Number(
                    ling_animation::scalar::piston(angle, crank, rod) as f64,
                ));
            },
            "rack" | "齿条" | "ラック" | "랙" | "แร็ค" | "زبانه‌دنده" | "سكة_مسننة" | "מוט_שיניים" | "ریک" | "crémaillère" | "zahnstange" | "рейка" => {
                let angle = self.arg_num(&args, 0, 0.0)? as f32;
                let radius = self.arg_num(&args, 1, 1.0)? as f32;
                return Ok(Value::Number(
                    ling_animation::scalar::rack(angle, radius) as f64
                ));
            },
            #[cfg(not(target_arch = "wasm32"))]
            "mouse_x" => {
                let gfx = self.gfx.borrow();
                let v = gfx
                    .window
                    .as_ref()
                    .and_then(|w| w.get_mouse_pos(minifb::MouseMode::Clamp))
                    .map(|p| p.0 as f64)
                    .unwrap_or(0.0);
                return Ok(Value::Number(v));
            },
            #[cfg(target_arch = "wasm32")]
            "mouse_x" => {
                return Ok(Value::Number(crate::gfx::wasm_mouse_x() as f64));
            },
            #[cfg(not(target_arch = "wasm32"))]
            "mouse_y" => {
                let gfx = self.gfx.borrow();
                let v = gfx
                    .window
                    .as_ref()
                    .and_then(|w| w.get_mouse_pos(minifb::MouseMode::Clamp))
                    .map(|p| p.1 as f64)
                    .unwrap_or(0.0);
                return Ok(Value::Number(v));
            },
            #[cfg(target_arch = "wasm32")]
            "mouse_y" => {
                return Ok(Value::Number(crate::gfx::wasm_mouse_y() as f64));
            },
            #[cfg(not(target_arch = "wasm32"))]
            "mouse_down" => {
                let mut gfx = self.gfx.borrow_mut();
                let d = !gfx.input_suppressed()
                    && gfx
                        .window
                        .as_ref()
                        .map(|w| w.get_mouse_down(minifb::MouseButton::Left))
                        .unwrap_or(false);
                return Ok(Value::Bool(d));
            },
            #[cfg(target_arch = "wasm32")]
            "mouse_down" => {
                return Ok(Value::Bool(crate::gfx::wasm_mouse_down()));
            },
            #[cfg(not(target_arch = "wasm32"))]
            "mouse_down_right" | "เมาส์ขวา" | "ماوس_راست_فشرده" | "الفأرة_اليمنى_مضغوطة" | "עכבר_ימני_לחוץ" | "دایاں_ماؤس_دبا_ہوا" => {
                let mut gfx = self.gfx.borrow_mut();
                let d = !gfx.input_suppressed()
                    && gfx
                        .window
                        .as_ref()
                        .map(|w| w.get_mouse_down(minifb::MouseButton::Right))
                        .unwrap_or(false);
                return Ok(Value::Bool(d));
            },
            #[cfg(target_arch = "wasm32")]
            "mouse_down_right" | "เมาส์ขวา" | "ماوس_راست_فشرده" | "الفأرة_اليمنى_مضغوطة" | "עכבר_ימני_לחוץ" | "دایاں_ماؤس_دبا_ہوا" => {
                return Ok(Value::Bool(crate::gfx::wasm_mouse_down_right()));
            },
            #[cfg(not(target_arch = "wasm32"))]
            "mouse_down_middle" | "เมาส์กลาง" | "ماوس_وسط_فشرده" | "الفأرة_الوسطى_مضغوطة" | "עכבר_אמצעי_לחוץ" | "درمیانی_ماؤس_دبا_ہوا" => {
                let mut gfx = self.gfx.borrow_mut();
                let d = !gfx.input_suppressed()
                    && gfx
                        .window
                        .as_ref()
                        .map(|w| w.get_mouse_down(minifb::MouseButton::Middle))
                        .unwrap_or(false);
                return Ok(Value::Bool(d));
            },
            #[cfg(target_arch = "wasm32")]
            "mouse_down_middle" | "เมาส์กลาง" | "ماوس_وسط_فشرده" | "الفأرة_الوسطى_مضغوطة" | "עכבר_אמצעי_לחוץ" | "درمیانی_ماؤس_دبا_ہوا" => {
                return Ok(Value::Bool(crate::gfx::wasm_mouse_down_middle()));
            },
            #[cfg(not(target_arch = "wasm32"))]
            "ui_hot" | "热区" | "ホットエリア" | "핫존" | "พื้นที่สัมผัส" | "ناحیه_فعال" | "منطقة_ساخنة" | "אזור_חם" | "ہاٹ_زون" | "survol_ui" | "ui_hover" | "ui_наведение" =>
            {
                let x = self.arg_num(&args, 0, 0.0)? as f32;
                let y = self.arg_num(&args, 1, 0.0)? as f32;
                let w = self.arg_num(&args, 2, 0.0)? as f32;
                let h = self.arg_num(&args, 3, 0.0)? as f32;
                let gfx = self.gfx.borrow();
                let (mx, my) = gfx
                    .window
                    .as_ref()
                    .and_then(|win| win.get_mouse_pos(minifb::MouseMode::Clamp))
                    .unwrap_or((0.0, 0.0));
                return Ok(Value::Bool(ling_ui::holo::hit_rect(mx, my, x, y, w, h)));
            },
            #[cfg(target_arch = "wasm32")]
            "ui_hot" | "热区" | "ホットエリア" | "핫존" | "พื้นที่สัมผัส" | "ناحیه_فعال" | "منطقة_ساخنة" | "אזור_חם" | "ہاٹ_زون" | "survol_ui" | "ui_hover" | "ui_наведение" =>
            {
                return Ok(Value::Bool(false));
            },
            // ui_text(x, y, scale, "string") — holographic vector text
            "ui_text" | "界面文字" | "UI文字" | "UI텍스트" | "ข้อความหน้าจอ" | "متن_رابط" | "نص_الواجهة" | "טקסט_ממשק" | "یو_آئی_متن" | "texte_ui" | "ui_beschriftung" | "ui_текст" =>
            {
                let x = self.arg_num(&args, 0, 0.0)? as f32;
                let y = self.arg_num(&args, 1, 0.0)? as f32;
                let scale = self.arg_num(&args, 2, 16.0)? as f32;
                let s = self.arg_str(&args, 3, "");
                let segs = ling_ui::holo::text_lines(&s, x, y, scale * 0.62, scale, scale * 0.24);
                let mut gfx = self.gfx.borrow_mut();
                let (w, h, color) = (gfx.width, gfx.height, gfx.color);
                for sg in segs {
                    draw_line(&mut gfx.buffer, w, h, color, sg[0], sg[1], sg[2], sg[3]);
                }
                return Ok(Value::Unit);
            },
            // font_load("path.ttf") — load a vector font (outlines cached lazily as
            // cache/fonts/<stem>/<codepoint>.ling). Returns a handle, or -1 on failure.
            #[cfg(not(target_arch = "wasm32"))]
            "font_load" | "โหลดฟอนต์" | "加载字体" | "フォント読込" | "글꼴로드" | "بارگذاری_فونت" | "تحميل_الخط" | "טעינת_גופן" | "فونٹ_لوڈ" | "charger_police" | "schriftart_laden" | "загрузить_шрифт" =>
            {
                let path = self.arg_str(&args, 0, "");
                // Optional 2nd arg: variable-font weight (e.g. 600 for a solid, bold UI).
                let weight = match self.arg_num(&args, 1, 0.0)? {
                    w if w > 0.0 => Some(w as f32),
                    _ => None,
                };
                // Try the path as given, then relative to the script's directory.
                let mut loaded = ling_graphics::VectorFont::from_path_weight(&path, weight);
                if loaded.is_err() {
                    if let Some(dir) = &self.source_dir {
                        let joined = dir.join(&path);
                        loaded = ling_graphics::VectorFont::from_path_weight(
                            &joined.to_string_lossy(),
                            weight,
                        );
                    }
                }
                match loaded {
                    Ok(f) => {
                        let id = self.fonts.len();
                        self.fonts.push(f);
                        return Ok(Value::Number(id as f64));
                    },
                    Err(e) => {
                        eprintln!("font_load failed ({path}): {e}");
                        return Ok(Value::Number(-1.0));
                    },
                }
            },
            #[cfg(target_arch = "wasm32")]
            "font_load" | "โหลดฟอนต์" | "加载字体" | "フォント読込" | "글꼴로드" | "بارگذاری_فونت" | "تحميل_الخط" | "טעינת_גופן" | "فونٹ_لوڈ" | "charger_police" | "schriftart_laden" | "загрузить_шрифт" =>
            {
                // Web runtime does not load host TTF/OTF files yet.
                // Return -1 so scripts can fall back to ui_text.
                return Ok(Value::Number(-1.0));
            },
            // image_load("path.png") — decode a raster image (via the `image` crate)
            // for pixel sampling (image_width/image_height/image_pixel_r/g/b/a) —
            // used by the coin-stamp mosaic tool to read a source photo's
            // colour/darkness. Returns a handle, or -1 on failure.
            #[cfg(not(target_arch = "wasm32"))]
            "image_load" =>
            {
                let path = self.arg_str(&args, 0, "").replace('\\', "/");
                let mut loaded = image::open(&path);
                if loaded.is_err() {
                    if let Some(dir) = &self.source_dir {
                        let joined = dir.join(&path);
                        loaded = image::open(&joined);
                    }
                }
                match loaded {
                    Ok(img) => {
                        let id = self.images.len();
                        self.images.push(img.to_rgba8());
                        return Ok(Value::Number(id as f64));
                    },
                    Err(e) => {
                        eprintln!("image_load failed ({path}): {e}");
                        return Ok(Value::Number(-1.0));
                    },
                }
            },
            #[cfg(target_arch = "wasm32")]
            "image_load" =>
            {
                // Web runtime does not load host image files yet.
                return Ok(Value::Number(-1.0));
            },
            "image_width" =>
            {
                let id = self.arg_num(&args, 0, -1.0)? as i64;
                if id >= 0 && (id as usize) < self.images.len() {
                    return Ok(Value::Number(self.images[id as usize].width() as f64));
                }
                return Ok(Value::Number(0.0));
            },
            "image_height" =>
            {
                let id = self.arg_num(&args, 0, -1.0)? as i64;
                if id >= 0 && (id as usize) < self.images.len() {
                    return Ok(Value::Number(self.images[id as usize].height() as f64));
                }
                return Ok(Value::Number(0.0));
            },
            "image_pixel_r" | "image_pixel_g" | "image_pixel_b" | "image_pixel_a" =>
            {
                let id = self.arg_num(&args, 0, -1.0)? as i64;
                let px = self.arg_num(&args, 1, 0.0)? as i64;
                let py = self.arg_num(&args, 2, 0.0)? as i64;
                if id >= 0 && (id as usize) < self.images.len() {
                    let img = &self.images[id as usize];
                    if px >= 0 && py >= 0 && (px as u32) < img.width() && (py as u32) < img.height() {
                        let p = img.get_pixel(px as u32, py as u32);
                        let ch = match name {
                            "image_pixel_r" => p[0],
                            "image_pixel_g" => p[1],
                            "image_pixel_b" => p[2],
                            _ => p[3],
                        };
                        return Ok(Value::Number(ch as f64));
                    }
                }
                return Ok(Value::Number(0.0));
            },
            // image_new(w, h) — a new blank (fully transparent) RGBA image the
            // script can paint into with image_set_pixel and write out with
            // image_save. Lives in the same self.images table as image_load,
            // so image_width/image_height/image_pixel_* all work on it too.
            // Used by the coin-stamp tool to build cropped, physically-sized
            // (mm x DPI) PNG exports — something a raw window screenshot()
            // can't do, since it always captures the whole on-screen
            // framebuffer at whatever size the window happens to be.
            "image_new" =>
            {
                let w = self.arg_num(&args, 0, 1.0)?.max(1.0) as u32;
                let h = self.arg_num(&args, 1, 1.0)?.max(1.0) as u32;
                let id = self.images.len();
                self.images.push(image::RgbaImage::new(w, h));
                return Ok(Value::Number(id as f64));
            },
            // image_set_pixel(id, x, y, r, g, b, a) — paint one pixel of an
            // image created with image_new (0..255 channels; out-of-bounds is
            // a silent no-op, matching image_pixel_*'s own out-of-bounds
            // behaviour).
            "image_set_pixel" =>
            {
                let id = self.arg_num(&args, 0, -1.0)? as i64;
                let px = self.arg_num(&args, 1, 0.0)? as i64;
                let py = self.arg_num(&args, 2, 0.0)? as i64;
                let r = self.arg_num(&args, 3, 0.0)?.clamp(0.0, 255.0) as u8;
                let g = self.arg_num(&args, 4, 0.0)?.clamp(0.0, 255.0) as u8;
                let b = self.arg_num(&args, 5, 0.0)?.clamp(0.0, 255.0) as u8;
                let a = self.arg_num(&args, 6, 255.0)?.clamp(0.0, 255.0) as u8;
                if id >= 0 && (id as usize) < self.images.len() {
                    let img = &mut self.images[id as usize];
                    if px >= 0 && py >= 0 && (px as u32) < img.width() && (py as u32) < img.height() {
                        img.put_pixel(px as u32, py as u32, image::Rgba([r, g, b, a]));
                    }
                }
                return Ok(Value::Unit);
            },
            // image_save(id, "path.png") — encode an image (from image_new or
            // image_load) to disk, alpha preserved. Returns 1 on success, -1
            // on failure (bad id or write error), mirroring image_load's own
            // -1-on-failure convention. Path resolves the same way
            // write_file/copy_file's outputs do: relative to the script's own
            // working directory (typically the app dir the launcher cd's
            // into), not source_dir.
            #[cfg(not(target_arch = "wasm32"))]
            "image_save" =>
            {
                let id = self.arg_num(&args, 0, -1.0)? as i64;
                let path = self.arg_str(&args, 1, "");
                if id >= 0 && (id as usize) < self.images.len() {
                    if let Some(parent) = std::path::Path::new(&path).parent() {
                        if !parent.as_os_str().is_empty() {
                            let _ = std::fs::create_dir_all(parent);
                        }
                    }
                    if self.images[id as usize].save(&path).is_ok() {
                        return Ok(Value::Number(1.0));
                    }
                }
                return Ok(Value::Number(-1.0));
            },
            #[cfg(target_arch = "wasm32")]
            "image_save" =>
            {
                return Ok(Value::Number(-1.0));
            },
            // image_draw(id, x, y, w, h) — blit an image (nearest-neighbour
            // scaled to w x h, alpha-blended against whatever's already in
            // the framebuffer) into the current frame. A native pixel loop,
            // not a .ling-level per-pixel image_pixel_*+pixel() loop: doing
            // this from script for even a modest thumbnail grid re-incurs
            // the exact per-frame interpreted-call-volume cost that made
            // small mosaic tiles hang the UI (see mosaic.ling's
            // xform_glyph_pts_fit fix) — this is the "read the framebuffer
            // out" direction's counterpart to screenshot().
            #[cfg(not(target_arch = "wasm32"))]
            "image_draw" =>
            {
                let id = self.arg_num(&args, 0, -1.0)? as i64;
                let dx = self.arg_num(&args, 1, 0.0)? as i32;
                let dy = self.arg_num(&args, 2, 0.0)? as i32;
                let dw = self.arg_num(&args, 3, 0.0)?.max(0.0) as i32;
                let dh = self.arg_num(&args, 4, 0.0)?.max(0.0) as i32;
                if id >= 0 && (id as usize) < self.images.len() && dw > 0 && dh > 0 {
                    let img = &self.images[id as usize];
                    let sw = img.width() as i32;
                    let sh = img.height() as i32;
                    if sw > 0 && sh > 0 {
                        let mut gfx = self.gfx.borrow_mut();
                        let (fw, fh) = (gfx.width as i32, gfx.height as i32);
                        for py in 0..dh {
                            let ty = dy + py;
                            if ty < 0 || ty >= fh {
                                continue;
                            }
                            let sy = (py * sh / dh).clamp(0, sh - 1) as u32;
                            for px in 0..dw {
                                let tx = dx + px;
                                if tx < 0 || tx >= fw {
                                    continue;
                                }
                                let sx = (px * sw / dw).clamp(0, sw - 1) as u32;
                                let p = img.get_pixel(sx, sy);
                                let a = p[3] as u32;
                                if a == 0 {
                                    continue;
                                }
                                let idx = ty as usize * gfx.width + tx as usize;
                                if a >= 255 {
                                    gfx.buffer[idx] =
                                        ((p[0] as u32) << 16) | ((p[1] as u32) << 8) | (p[2] as u32);
                                } else {
                                    let bg = gfx.buffer[idx];
                                    let br = (bg >> 16) & 0xff;
                                    let bg_g = (bg >> 8) & 0xff;
                                    let bb = bg & 0xff;
                                    let r = (p[0] as u32 * a + br * (255 - a)) / 255;
                                    let g = (p[1] as u32 * a + bg_g * (255 - a)) / 255;
                                    let b = (p[2] as u32 * a + bb * (255 - a)) / 255;
                                    gfx.buffer[idx] = (r << 16) | (g << 8) | b;
                                }
                            }
                        }
                    }
                }
                return Ok(Value::Unit);
            },
            #[cfg(target_arch = "wasm32")]
            "image_draw" =>
            {
                return Ok(Value::Unit);
            },
            // font_text(handle, x, y, px, "string") — anti-aliased *stroked* vector outline
            // in the current set_color / set_blend. (x,y) is the text box top-left.
            #[cfg(not(target_arch = "wasm32"))]
            "font_text" | "ข้อความฟอนต์" | "字体文本" | "フォント文字" | "글꼴텍스트" | "متن_فونت" | "نص_الخط" | "טקסט_גופן" | "فونٹ_متن" | "texte_police" | "schriftart_text" | "текст_шрифт" =>
            {
                let id = self.arg_num(&args, 0, 0.0)? as i64;
                let x = self.arg_num(&args, 1, 0.0)? as f32;
                let y = self.arg_num(&args, 2, 0.0)? as f32;
                let px = self.arg_num(&args, 3, 16.0)? as f32;
                let s = self.arg_str(&args, 4, "");
                if id >= 0 && (id as usize) < self.fonts.len() && px > 0.0 {
                    let strokes = self.font_layout_2d(id as usize, x, y, px, &s);
                    let mut gfx = self.gfx.borrow_mut();
                    let (w, h, color, add, aa) =
                        (gfx.width, gfx.height, gfx.color, gfx.blend == 1, gfx.font_antialias);
                    for pl in &strokes {
                        for seg in pl.windows(2) {
                            if aa {
                                crate::gfx::raster::draw_line_aa(
                                    &mut gfx.buffer,
                                    w,
                                    h,
                                    color,
                                    add,
                                    seg[0][0],
                                    seg[0][1],
                                    seg[1][0],
                                    seg[1][1],
                                );
                            } else {
                                crate::gfx::raster::draw_line(
                                    &mut gfx.buffer,
                                    w,
                                    h,
                                    color,
                                    seg[0][0],
                                    seg[0][1],
                                    seg[1][0],
                                    seg[1][1],
                                );
                            }
                        }
                    }
                }
                return Ok(Value::Unit);
            },
            #[cfg(target_arch = "wasm32")]
            "font_text" | "ข้อความฟอนต์" | "字体文本" | "フォント文字" | "글꼴텍스트" | "متن_فونت" | "نص_الخط" | "טקסט_גופן" | "فونٹ_متن" | "texte_police" | "schriftart_text" | "текст_шрифт" =>
            {
                return Ok(Value::Unit);
            },
            // font_text_fill(handle, x, y, px, "string") — filled vector glyphs;
            // anti-aliased when `set_font_antialias(1)` is on (default off = crisp).
            #[cfg(not(target_arch = "wasm32"))]
            "font_text_fill" | "เติมฟอนต์" | "填充字体" | "フォント塗り" | "글꼴채움" | "پرکردن_متن_فونت" | "تعبئة_نص_الخط" | "מילוי_טקסט_גופן" | "فونٹ_متن_بھرو" | "remplir_texte_police" | "schriftart_text_füllen" | "заполнить_текст_шрифт" =>
            {
                let id = self.arg_num(&args, 0, 0.0)? as i64;
                let x = self.arg_num(&args, 1, 0.0)? as f32;
                let y = self.arg_num(&args, 2, 0.0)? as f32;
                let px = self.arg_num(&args, 3, 16.0)? as f32;
                let s = self.arg_str(&args, 4, "");
                if id >= 0 && (id as usize) < self.fonts.len() && px > 0.0 {
                    // fill each glyph independently so interior holes (winding) stay correct
                    let glyphs = self.font_layout_2d_glyphs(id as usize, x, y, px, &s);
                    let mut gfx = self.gfx.borrow_mut();
                    let (w, h, color, add, aa) =
                        (gfx.width, gfx.height, gfx.color, gfx.blend == 1, gfx.font_antialias);
                    for contours in &glyphs {
                        if aa {
                            crate::gfx::raster::fill_contours_aa(
                                &mut gfx.buffer,
                                w,
                                h,
                                color,
                                add,
                                contours,
                            );
                        } else {
                            crate::gfx::raster::fill_contours(
                                &mut gfx.buffer,
                                w,
                                h,
                                color,
                                add,
                                contours,
                            );
                        }
                    }
                }
                return Ok(Value::Unit);
            },
            #[cfg(target_arch = "wasm32")]
            "font_text_fill" | "เติมฟอนต์" | "填充字体" | "フォント塗り" | "글꼴채움" | "پرکردن_متن_فونت" | "تعبئة_نص_الخط" | "מילוי_טקסט_גופן" | "فونٹ_متن_بھرو" | "remplir_texte_police" | "schriftart_text_füllen" | "заполнить_текст_шрифт" =>
            {
                return Ok(Value::Unit);
            },
            // font_text_3d(handle, cx,cy,cz, ux,uy,uz, vx,vy,vz, size, "string")
            // — stroked vector text on a 3D plane: u = advance dir, v = up dir, size = world/em.
            //   Flows through the depth-sorted line pipeline, so it rotates with the camera (and 4D).
            #[cfg(not(target_arch = "wasm32"))]
            "font_text_3d" | "ข้อความฟอนต์3มิติ" | "字体3D" | "フォント3D" | "글꼴3D" | "متن_فونت_سه‌بعدی" | "نص_خط_ثلاثي_الأبعاد" | "טקסט_גופן_תלת_ממדי" | "تھری_ڈی_فونٹ_متن" | "texte_police_3d" | "schriftart_text_3d" | "текст_шрифт_3d" =>
            {
                let id = self.arg_num(&args, 0, 0.0)? as i64;
                let cx = self.arg_num(&args, 1, 0.0)? as f32;
                let cy = self.arg_num(&args, 2, 0.0)? as f32;
                let cz = self.arg_num(&args, 3, 0.0)? as f32;
                let ux = self.arg_num(&args, 4, 1.0)? as f32;
                let uy = self.arg_num(&args, 5, 0.0)? as f32;
                let uz = self.arg_num(&args, 6, 0.0)? as f32;
                let vx = self.arg_num(&args, 7, 0.0)? as f32;
                let vy = self.arg_num(&args, 8, 1.0)? as f32;
                let vz = self.arg_num(&args, 9, 0.0)? as f32;
                let size = self.arg_num(&args, 10, 1.0)? as f32;
                let s = self.arg_str(&args, 11, "");
                // Optional arg 12: fill_rows — when > 0, each glyph interior is
                // filled with that many even-odd scanline spans (true filled
                // letterforms, not a bounding box). 0/omitted = outline only.
                let fill_rows = self.arg_num(&args, 12, 0.0)? as i32;
                if id >= 0 && (id as usize) < self.fonts.len() && size > 0.0 {
                    // Build world-space polylines: world = C + (pen+ex)*size*U + ey*size*V
                    let font = &mut self.fonts[id as usize];
                    let asc = font.ascent();
                    let mut pen = 0.0f32;
                    let mut lines: Vec<[f32; 6]> = Vec::new();
                    for ch in s.chars() {
                        let go = font.glyph_outline(ch, 0.01);
                        let map = |p: [f32; 2], pen: f32| {
                            let a = pen + p[0];
                            let b = p[1] - asc; // shift so the top of the cap sits near C
                            [
                                cx + a * size * ux + b * size * vx,
                                cy + a * size * uy + b * size * vy,
                                cz + a * size * uz + b * size * vz,
                            ]
                        };
                        for pl in &go.polylines {
                            for seg in pl.windows(2) {
                                let p0 = map(seg[0], pen);
                                let p1 = map(seg[1], pen);
                                lines.push([p0[0], p0[1], p0[2], p1[0], p1[1], p1[2]]);
                            }
                        }
                        if fill_rows > 0 {
                            // Even-odd scanline fill in glyph space. Contours may
                            // omit their closing edge, so the implicit last→first
                            // segment is scanned too (skipped when degenerate).
                            let (mut ymin, mut ymax) = (f32::MAX, f32::MIN);
                            for pl in &go.polylines {
                                for p in pl {
                                    ymin = ymin.min(p[1]);
                                    ymax = ymax.max(p[1]);
                                }
                            }
                            if ymax > ymin {
                                for r in 0..fill_rows {
                                    let y =
                                        ymin + (r as f32 + 0.5) * (ymax - ymin) / fill_rows as f32;
                                    let mut xs: Vec<f32> = Vec::new();
                                    for pl in &go.polylines {
                                        let n = pl.len();
                                        if n < 2 {
                                            continue;
                                        }
                                        for k in 0..n {
                                            let p0 = pl[k];
                                            let p1 = pl[(k + 1) % n];
                                            if k + 1 == n
                                                && (p1[0] - p0[0]).abs() < 1e-6
                                                && (p1[1] - p0[1]).abs() < 1e-6
                                            {
                                                continue; // contour already closed
                                            }
                                            let (y0, y1) = (p0[1], p1[1]);
                                            if (y0 <= y && y1 > y) || (y1 <= y && y0 > y) {
                                                let t = (y - y0) / (y1 - y0);
                                                xs.push(p0[0] + t * (p1[0] - p0[0]));
                                            }
                                        }
                                    }
                                    xs.sort_by(|a, b| {
                                        a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal)
                                    });
                                    let mut k = 0;
                                    while k + 1 < xs.len() {
                                        let a = map([xs[k], y], pen);
                                        let b = map([xs[k + 1], y], pen);
                                        lines.push([a[0], a[1], a[2], b[0], b[1], b[2]]);
                                        k += 2;
                                    }
                                }
                            }
                        }
                        pen += go.advance;
                    }
                    let mut gfx = self.gfx.borrow_mut();
                    let color = gfx.color;
                    let near = -gfx.camera.zdist + 0.05;
                    for l in &lines {
                        let (mut ax, mut ay, mut az) = (l[0], l[1], l[2]);
                        let (mut bx, mut by, mut bz) = (l[3], l[4], l[5]);
                        let da = gfx.camera.depth(ax, ay, az);
                        let db = gfx.camera.depth(bx, by, bz);
                        if da <= near && db <= near {
                            continue;
                        }
                        if da <= near {
                            let t = (near - da) / (db - da);
                            ax += t * (bx - ax);
                            ay += t * (by - ay);
                            az += t * (bz - az);
                        } else if db <= near {
                            let t = (near - da) / (db - da);
                            bx = ax + t * (bx - ax);
                            by = ay + t * (by - ay);
                            bz = az + t * (bz - az);
                        }
                        let (sax, say, da2) = gfx.camera.project(ax, ay, az);
                        let (sbx, sby, db2) = gfx.camera.project(bx, by, bz);
                        let depth = (da2 + db2) / 2.0;
                        gfx.depth_queue.push_line(depth, color, sax, say, sbx, sby);
                    }
                }
                return Ok(Value::Unit);
            },
            #[cfg(target_arch = "wasm32")]
            "font_text_3d" | "ข้อความฟอนต์3มิติ" | "字体3D" | "フォント3D" | "글꼴3D" | "متن_فونت_سه‌بعدی" | "نص_خط_ثلاثي_الأبعاد" | "טקסט_גופן_תלת_ממדי" | "تھری_ڈی_فونٹ_متن" | "texte_police_3d" | "schriftart_text_3d" | "текст_шрифт_3d" =>
            {
                return Ok(Value::Unit);
            },
            // font_width(handle, px, "string") — pixel width of a string in a loaded font.
            #[cfg(not(target_arch = "wasm32"))]
            "font_width" | "ความกว้างฟอนต์" | "字体宽度" | "フォント幅" | "글꼴너비" | "عرض_فونت" | "عرض_الخط" | "רוחב_גופן" | "فونٹ_چوڑائی" | "largeur_police" | "schriftart_breite" | "ширина_шрифта" =>
            {
                let id = self.arg_num(&args, 0, 0.0)? as i64;
                let px = self.arg_num(&args, 1, 16.0)? as f32;
                let s = self.arg_str(&args, 2, "");
                if id >= 0 && (id as usize) < self.fonts.len() {
                    return Ok(Value::Number(self.fonts[id as usize].measure(&s, px) as f64));
                }
                return Ok(Value::Number(0.0));
            },
            #[cfg(target_arch = "wasm32")]
            "font_width" | "ความกว้างฟอนต์" | "字体宽度" | "フォント幅" | "글꼴너비" | "عرض_فونت" | "عرض_الخط" | "רוחב_גופן" | "فونٹ_چوڑائی" | "largeur_police" | "schriftart_breite" | "ширина_шрифта" =>
            {
                return Ok(Value::Number(0.0));
            },
            // font_glyph_outline(handle, "char", tol_em) — flattened vector outline of
            // ONE glyph in normalized em space (x→right, y→up, baseline at 0). Returns a
            // list of contours; each contour is a flat list [x0,y0,x1,y1,…]. Curves are
            // subdivided so deviation stays under tol_em (default 0.01). Empty on failure.
            #[cfg(not(target_arch = "wasm32"))]
            "font_glyph_outline" | "font_outline" | "เส้นขอบฟอนต์" | "字体轮廓"
            | "フォント輪郭" | "글꼴윤곽" | "خط‌دور_نویسه_فونت" | "حدود_حرف_الخط" | "קו_מתאר_גליף" | "فونٹ_گلف_آؤٹ_لائن" => {
                let id = self.arg_num(&args, 0, 0.0)? as i64;
                let s = self.arg_str(&args, 1, "");
                let tol = self.arg_num(&args, 2, 0.01)? as f32;
                let ch = s.chars().next().unwrap_or(' ');
                if id >= 0 && (id as usize) < self.fonts.len() {
                    let go = self.fonts[id as usize].glyph_outline(ch, tol.max(1e-4));
                    let mut contours: Vec<Value> = Vec::with_capacity(go.polylines.len());
                    for pl in &go.polylines {
                        let mut flat: Vec<Value> = Vec::with_capacity(pl.len() * 2);
                        for p in pl {
                            flat.push(Value::Number(p[0] as f64));
                            flat.push(Value::Number(p[1] as f64));
                        }
                        contours.push(Value::List(Rc::new(flat)));
                    }
                    return Ok(Value::List(Rc::new(contours)));
                }
                return Ok(Value::List(Rc::new(vec![])));
            },
            #[cfg(target_arch = "wasm32")]
            "font_glyph_outline" | "font_outline" | "เส้นขอบฟอนต์" | "字体轮廓"
            | "フォント輪郭" | "글꼴윤곽" | "خط‌دور_نویسه_فونت" | "حدود_حرف_الخط" | "קו_מתאר_גליף" | "فونٹ_گلف_آؤٹ_لائن" => {
                return Ok(Value::List(Rc::new(vec![])));
            },
            // font_advance(handle, "char") — normalized em advance width of ONE glyph
            // (baseline metric, ignores side bearings). Multiply by px for pixels.
            #[cfg(not(target_arch = "wasm32"))]
            "font_advance" | "ระยะฟอนต์" | "字体步进" | "フォント送り" | "글꼴전진" | "پیشروی_فونت" | "تقدم_الخط" | "קידום_גופן" | "فونٹ_ایڈوانس" => {
                let id = self.arg_num(&args, 0, 0.0)? as i64;
                let s = self.arg_str(&args, 1, "");
                let ch = s.chars().next().unwrap_or(' ');
                if id >= 0 && (id as usize) < self.fonts.len() {
                    return Ok(Value::Number(self.fonts[id as usize].advance(ch) as f64));
                }
                return Ok(Value::Number(0.0));
            },
            #[cfg(target_arch = "wasm32")]
            "font_advance" | "ระยะฟอนต์" | "字体步进" | "フォント送り" | "글꼴전진" | "پیشروی_فونت" | "تقدم_الخط" | "קידום_גופן" | "فونٹ_ایڈوانس" => {
                return Ok(Value::Number(0.0));
            },

            // ui_frame(x,y,w,h, bracketLen) — sci-fi corner brackets
            "ui_frame" | "边框" | "フレーム枠" | "프레임틀" | "กรอบ" | "قاب_رابط" | "إطار_الواجهة" | "מסגרת_ממשק" | "یو_آئی_فریم" | "cadre_ui" | "ui_rahmen" | "ui_рамка" => {
                let x = self.arg_num(&args, 0, 0.0)? as f32;
                let y = self.arg_num(&args, 1, 0.0)? as f32;
                let w0 = self.arg_num(&args, 2, 0.0)? as f32;
                let h0 = self.arg_num(&args, 3, 0.0)? as f32;
                let l = self.arg_num(&args, 4, 14.0)? as f32;
                let segs = ling_ui::holo::corner_brackets(x, y, w0, h0, l);
                let mut gfx = self.gfx.borrow_mut();
                let (w, h, color) = (gfx.width, gfx.height, gfx.color);
                for sg in segs {
                    draw_line(&mut gfx.buffer, w, h, color, sg[0], sg[1], sg[2], sg[3]);
                }
                return Ok(Value::Unit);
            },
            // ui_bevel(x,y,w,h, bevel) — beveled holographic panel outline
            "ui_bevel" | "斜角框" | "ベベル枠" | "베벨틀" | "กรอบเฉียง" | "لبه_شیبدار" | "حافة_مشطوفة" | "מסגרת_משופעת" | "یو_آئی_بیول" | "biseau_ui" | "ui_fase" | "ui_фаска" =>
            {
                let x = self.arg_num(&args, 0, 0.0)? as f32;
                let y = self.arg_num(&args, 1, 0.0)? as f32;
                let w0 = self.arg_num(&args, 2, 0.0)? as f32;
                let h0 = self.arg_num(&args, 3, 0.0)? as f32;
                let bv = self.arg_num(&args, 4, 10.0)? as f32;
                let segs = ling_ui::holo::beveled_rect(x, y, w0, h0, bv);
                let mut gfx = self.gfx.borrow_mut();
                let (w, h, color) = (gfx.width, gfx.height, gfx.color);
                for sg in segs {
                    draw_line(&mut gfx.buffer, w, h, color, sg[0], sg[1], sg[2], sg[3]);
                }
                return Ok(Value::Unit);
            },

            // ══════════════════════════════════════════════════════════════════
            // VECTOR UI TOOLKIT  (crates/ling-ui/src/widgets.rs)
            // All widgets are vector + theme-coloured with an optional trailing
            // r,g,b override; interactive ones read the mouse and return state.
            // ══════════════════════════════════════════════════════════════════
            #[cfg(not(target_arch = "wasm32"))]
            "ui_theme" | "界面主题" | "UIテーマ" | "인터페이스테마" | "ธีมส่วนติดต่อ" | "پوسته_رابط" | "سمة_الواجهة" | "ערכת_נושא" | "یو_آئی_تھیم" | "thème_ui" | "ui_thema" | "ui_тема" =>
            {
                let cur = self.ui_theme;
                let primary = self.color_at(&args, 0, cur.primary);
                let accent = self.color_at(&args, 3, cur.accent);
                let track = self.color_at(&args, 6, cur.track);
                let warn = self.color_at(&args, 9, cur.warn);
                let text = self.color_at(&args, 12, cur.text);
                let bg = self.color_at(&args, 15, cur.bg);
                self.ui_theme = UiTheme { primary, accent, track, warn, text, bg };
                return Ok(Value::Unit);
            },

            // ui_theme_colors() -> [pr,pg,pb, ar,ag,ab, tr,tg,tb, wr,wg,wb,
            // xr,xg,xb, br,bg,bb] — the live theme every ui_* widget already
            // draws from (primary/accent/track/warn/text/bg, each 0-255),
            // so script-drawn UI (e.g. a hand-rolled text field) can match it
            // instead of guessing its own colours.
            "ui_theme_colors" | "인터페이스테마색상" => {
                let th = self.ui_theme;
                let mut out = Vec::with_capacity(18);
                for c in [th.primary, th.accent, th.track, th.warn, th.text, th.bg] {
                    out.push(Value::Number(((c >> 16) & 0xFF) as f64));
                    out.push(Value::Number(((c >> 8) & 0xFF) as f64));
                    out.push(Value::Number((c & 0xFF) as f64));
                }
                return Ok(Value::List(Rc::new(out)));
            },

            // ── HUD ──────────────────────────────────────────────────────────
            #[cfg(not(target_arch = "wasm32"))]
            "ui_radar" | "雷达" | "レーダー" | "레이더" | "เรดาร์" | "رادار_رابط" | "رادار_الواجهة" | "מכ״ם_ממשק" | "یو_آئی_ریڈار" | "radar_ui" | "ui_радар" => {
                let cx = self.arg_num(&args, 0, 0.)? as f32;
                let cy = self.arg_num(&args, 1, 0.)? as f32;
                let r = self.arg_num(&args, 2, 60.)? as f32;
                let sweep = self.arg_num(&args, 3, 0.)? as f32;
                let th = self.ui_theme;
                let prim = self.color_at(&args, 4, th.primary);
                self.draw_ui(&ling_ui::widgets::radar(
                    cx, cy, r, sweep, prim, th.accent, th.track,
                ));
                return Ok(Value::Unit);
            },
            #[cfg(not(target_arch = "wasm32"))]
            "ui_compass" | "罗盘" | "コンパス" | "나침반" | "เข็มทิศ" | "قطب‌نمای_رابط" | "بوصلة_الواجهة" | "מצפן_ממשק" | "یو_آئی_قطب_نما" | "boussole_ui" | "ui_kompass" | "ui_компас" => {
                let x = self.arg_num(&args, 0, 0.)? as f32;
                let y = self.arg_num(&args, 1, 0.)? as f32;
                let w0 = self.arg_num(&args, 2, 300.)? as f32;
                let h0 = self.arg_num(&args, 3, 24.)? as f32;
                let head = self.arg_num(&args, 4, 0.)? as f32;
                let th = self.ui_theme;
                let prim = self.color_at(&args, 5, th.primary);
                self.draw_ui(&ling_ui::widgets::compass(
                    x, y, w0, h0, head, prim, th.track,
                ));
                return Ok(Value::Unit);
            },
            #[cfg(not(target_arch = "wasm32"))]
            "ui_reticle" | "准星" | "照準" | "조준선" | "เป้าเล็ง" | "نشانه_رابط" | "علامة_تصويب" | "כוונת" | "نشانہ" | "réticule_ui" | "ui_fadenkreuz" | "ui_прицел" => {
                let cx = self.arg_num(&args, 0, 0.)? as f32;
                let cy = self.arg_num(&args, 1, 0.)? as f32;
                let r = self.arg_num(&args, 2, 30.)? as f32;
                let spread = self.arg_num(&args, 3, 0.)? as f32;
                let th = self.ui_theme;
                let prim = self.color_at(&args, 4, th.primary);
                self.draw_ui(&ling_ui::widgets::reticle(cx, cy, r, spread, prim));
                return Ok(Value::Unit);
            },
            #[cfg(not(target_arch = "wasm32"))]
            "ui_target" | "锁定框" | "ターゲット" | "표적" | "กรอบเป้า" | "قاب_هدف" | "إطار_الهدف" | "מסגרת_מטרה" | "ہدف_فریم" | "cible_ui" | "ui_ziel" | "ui_цель" =>
            {
                let x = self.arg_num(&args, 0, 0.)? as f32;
                let y = self.arg_num(&args, 1, 0.)? as f32;
                let w0 = self.arg_num(&args, 2, 80.)? as f32;
                let h0 = self.arg_num(&args, 3, 80.)? as f32;
                let lock = self.arg_num(&args, 4, 0.)? as f32;
                let th = self.ui_theme;
                let prim = self.color_at(&args, 5, th.primary);
                self.draw_ui(&ling_ui::widgets::target(
                    x, y, w0, h0, lock, prim, th.accent,
                ));
                return Ok(Value::Unit);
            },
            #[cfg(not(target_arch = "wasm32"))]
            "ui_panel" | "面板" | "パネル" | "패널" | "แผง" | "پنل_رابط" | "لوحة_الواجهة" | "לוח_ממשק" | "یو_آئی_پینل" | "panneau_ui" | "ui_feld" | "ui_панель" => {
                let x = self.arg_num(&args, 0, 0.)? as f32;
                let y = self.arg_num(&args, 1, 0.)? as f32;
                let w0 = self.arg_num(&args, 2, 200.)? as f32;
                let h0 = self.arg_num(&args, 3, 120.)? as f32;
                let bv = self.arg_num(&args, 4, 12.)? as f32;
                let th = self.ui_theme;
                let prim = self.color_at(&args, 5, th.primary);
                self.draw_ui(&ling_ui::widgets::panel(x, y, w0, h0, bv, prim, th.bg));
                return Ok(Value::Unit);
            },
            #[cfg(not(target_arch = "wasm32"))]
            "ui_scanlines" | "扫描线" | "走査線" | "스캔라인" | "เส้นสแกน" | "خطوط_اسکن" | "خطوط_المسح" | "קווי_סריקה" | "اسکین_لائنز" | "lignes_balayage_ui" | "ui_abtastzeilen" | "ui_линии_развёртки" =>
            {
                let x = self.arg_num(&args, 0, 0.)? as f32;
                let y = self.arg_num(&args, 1, 0.)? as f32;
                let w0 = self.arg_num(&args, 2, 200.)? as f32;
                let h0 = self.arg_num(&args, 3, 120.)? as f32;
                let dens = self.arg_num(&args, 4, 24.)? as usize;
                let th = self.ui_theme;
                let line = self.color_at(&args, 5, th.track);
                self.draw_ui(&ling_ui::widgets::scanlines(x, y, w0, h0, dens, line));
                return Ok(Value::Unit);
            },

            // ── Meters ───────────────────────────────────────────────────────
            #[cfg(not(target_arch = "wasm32"))]
            "ui_bar" | "进度条" | "バー" | "막대" | "แถบ" | "نوار_رابط" | "شريط_الواجهة" | "סרגל_ממשק" | "یو_آئی_بار" | "barre_ui" | "ui_leiste" | "ui_полоса" => {
                let x = self.arg_num(&args, 0, 0.)? as f32;
                let y = self.arg_num(&args, 1, 0.)? as f32;
                let w0 = self.arg_num(&args, 2, 160.)? as f32;
                let h0 = self.arg_num(&args, 3, 16.)? as f32;
                let val = self.arg_num(&args, 4, 0.)? as f32;
                let max = self.arg_num(&args, 5, 1.)? as f32;
                let th = self.ui_theme;
                let fill = self.color_at(&args, 6, th.primary);
                self.draw_ui(&ling_ui::widgets::bar(
                    x,
                    y,
                    w0,
                    h0,
                    val / max.max(1e-6),
                    fill,
                    th.track,
                ));
                return Ok(Value::Unit);
            },
            #[cfg(not(target_arch = "wasm32"))]
            "ui_segbar" | "分段条" | "分割バー" | "분할막대" | "แถบแบ่ง" | "نوار_قطعه‌ای" | "شريط_مقسم" | "סרגל_מקוטע" | "سیگمنٹ_بار" | "barre_segmentée_ui" | "ui_segmentleiste" | "ui_сегментная_полоса" =>
            {
                let x = self.arg_num(&args, 0, 0.)? as f32;
                let y = self.arg_num(&args, 1, 0.)? as f32;
                let w0 = self.arg_num(&args, 2, 160.)? as f32;
                let h0 = self.arg_num(&args, 3, 16.)? as f32;
                let val = self.arg_num(&args, 4, 0.)? as f32;
                let max = self.arg_num(&args, 5, 1.)? as f32;
                let segs = self.arg_num(&args, 6, 10.)? as usize;
                let th = self.ui_theme;
                let fill = self.color_at(&args, 7, th.primary);
                self.draw_ui(&ling_ui::widgets::segbar(
                    x,
                    y,
                    w0,
                    h0,
                    val / max.max(1e-6),
                    segs,
                    fill,
                    th.track,
                ));
                return Ok(Value::Unit);
            },
            #[cfg(not(target_arch = "wasm32"))]
            "ui_gauge" | "仪表" | "ゲージ" | "게이지" | "มาตรวัด" | "گیج_رابط" | "مقياس_الواجهة" | "מד_ממשק" | "یو_آئی_گیج" | "jauge_ui" | "ui_anzeige" | "ui_индикатор" => {
                let cx = self.arg_num(&args, 0, 0.)? as f32;
                let cy = self.arg_num(&args, 1, 0.)? as f32;
                let r = self.arg_num(&args, 2, 50.)? as f32;
                let val = self.arg_num(&args, 3, 0.)? as f32;
                let max = self.arg_num(&args, 4, 1.)? as f32;
                let th = self.ui_theme;
                let needle = self.color_at(&args, 5, th.warn);
                self.draw_ui(&ling_ui::widgets::gauge(
                    cx,
                    cy,
                    r,
                    val / max.max(1e-6),
                    needle,
                    th.accent,
                    th.track,
                ));
                return Ok(Value::Unit);
            },
            #[cfg(not(target_arch = "wasm32"))]
            "ui_ring" | "环表" | "リングメーター" | "링미터" | "วงแหวนวัด" | "حلقه_گیج" | "حلقة_قياس" | "טבעת_מד" | "رنگ_گیج" | "anneau_ui" | "ui_кольцо" =>
            {
                let cx = self.arg_num(&args, 0, 0.)? as f32;
                let cy = self.arg_num(&args, 1, 0.)? as f32;
                let r = self.arg_num(&args, 2, 40.)? as f32;
                let val = self.arg_num(&args, 3, 0.)? as f32;
                let max = self.arg_num(&args, 4, 1.)? as f32;
                let th = self.ui_theme;
                let fill = self.color_at(&args, 5, th.primary);
                self.draw_ui(&ling_ui::widgets::ring(
                    cx,
                    cy,
                    r,
                    val / max.max(1e-6),
                    fill,
                    th.track,
                ));
                return Ok(Value::Unit);
            },
            #[cfg(not(target_arch = "wasm32"))]
            "ui_vu" | "音量条" | "VUメーター" | "음량막대" | "มาตรเสียง" | "گیج_صدا" | "مقياس_مستوى_الصوت" | "מד_עוצמה" | "وی_یو_میٹر" | "vumètre_ui" | "ui_vumeter" | "ui_вю_метр" =>
            {
                let x = self.arg_num(&args, 0, 0.)? as f32;
                let y = self.arg_num(&args, 1, 0.)? as f32;
                let w0 = self.arg_num(&args, 2, 160.)? as f32;
                let h0 = self.arg_num(&args, 3, 60.)? as f32;
                let levels = self.arg_list_f32(&args, 4);
                let th = self.ui_theme;
                let fill = self.color_at(&args, 5, th.primary);
                self.draw_ui(&ling_ui::widgets::vu(x, y, w0, h0, &levels, fill, th.warn));
                return Ok(Value::Unit);
            },
            #[cfg(not(target_arch = "wasm32"))]
            "ui_spark" | "迷你图" | "スパークライン" | "스파크라인" | "กราฟจิ๋ว" | "نمودار_ریز" | "رسم_مصغر" | "גרף_זעיר" | "اسپارک_لائن" | "mini_graphe_ui" | "ui_sparkline" | "ui_мини_график" =>
            {
                let x = self.arg_num(&args, 0, 0.)? as f32;
                let y = self.arg_num(&args, 1, 0.)? as f32;
                let w0 = self.arg_num(&args, 2, 160.)? as f32;
                let h0 = self.arg_num(&args, 3, 40.)? as f32;
                let vals = self.arg_list_f32(&args, 4);
                let th = self.ui_theme;
                let line = self.color_at(&args, 5, th.accent);
                self.draw_ui(&ling_ui::widgets::spark(x, y, w0, h0, &vals, line));
                return Ok(Value::Unit);
            },
            #[cfg(not(target_arch = "wasm32"))]
            "ui_battery" | "电池" | "バッテリー" | "배터리" | "แบตเตอรี่" | "نشانگر_باتری" | "مؤشر_البطارية" | "מחוון_סוללה" | "بیٹری_انڈیکیٹر" | "batterie_ui" | "ui_batterie" | "ui_батарея" =>
            {
                let x = self.arg_num(&args, 0, 0.)? as f32;
                let y = self.arg_num(&args, 1, 0.)? as f32;
                let w0 = self.arg_num(&args, 2, 50.)? as f32;
                let h0 = self.arg_num(&args, 3, 22.)? as f32;
                let val = self.arg_num(&args, 4, 1.)? as f32;
                let max = self.arg_num(&args, 5, 1.)? as f32;
                let th = self.ui_theme;
                let fill = self.color_at(&args, 6, th.accent);
                self.draw_ui(&ling_ui::widgets::battery(
                    x,
                    y,
                    w0,
                    h0,
                    val / max.max(1e-6),
                    fill,
                    th.track,
                    th.warn,
                ));
                return Ok(Value::Unit);
            },

            // ── Interface controls (interactive → return state) ──────────────
            #[cfg(not(target_arch = "wasm32"))]
            "ui_button" | "按钮" | "ボタン" | "버튼" | "ปุ่ม" | "دکمه_رابط" | "زر_الواجهة" | "כפתור_ממשק" | "یو_آئی_بٹن" | "bouton_ui" | "ui_knopf" | "ui_кнопка" => {
                let x = self.arg_num(&args, 0, 0.)? as f32;
                let y = self.arg_num(&args, 1, 0.)? as f32;
                let w0 = self.arg_num(&args, 2, 120.)? as f32;
                let h0 = self.arg_num(&args, 3, 40.)? as f32;
                let (mx, my, down) = self.mouse_now();
                let hover = ling_ui::holo::hit_rect(mx, my, x, y, w0, h0);
                let clicked = hover && down && !self.mouse_was_down;
                let th = self.ui_theme;
                let prim = self.color_at(&args, 4, th.primary);
                self.draw_ui(&ling_ui::widgets::button(
                    x,
                    y,
                    w0,
                    h0,
                    hover,
                    down && hover,
                    prim,
                    th.bg,
                ));
                return Ok(Value::Number(if clicked { 1.0 } else { 0.0 }));
            },
            #[cfg(not(target_arch = "wasm32"))]
            "ui_toggle" | "开关" | "トグル" | "토글" | "สวิตช์" | "کلید_ضامن" | "مفتاح_تبديل" | "מתג" | "ٹوگل" | "bascule_ui" | "ui_schalter" | "ui_переключатель" => {
                let x = self.arg_num(&args, 0, 0.)? as f32;
                let y = self.arg_num(&args, 1, 0.)? as f32;
                let w0 = self.arg_num(&args, 2, 52.)? as f32;
                let h0 = self.arg_num(&args, 3, 24.)? as f32;
                let mut state = self.arg_num(&args, 4, 0.)? > 0.5;
                let (mx, my, down) = self.mouse_now();
                let hover = ling_ui::holo::hit_rect(mx, my, x, y, w0, h0);
                if hover && down && !self.mouse_was_down {
                    state = !state;
                }
                let th = self.ui_theme;
                let on = self.color_at(&args, 5, th.accent);
                self.draw_ui(&ling_ui::widgets::toggle(x, y, w0, h0, state, on, th.track));
                return Ok(Value::Number(if state { 1.0 } else { 0.0 }));
            },
            #[cfg(not(target_arch = "wasm32"))]
            "ui_slider" | "滑块" | "スライダー" | "슬라이더" | "แถบเลื่อน" | "لغزنده" | "شريط_انزلاق" | "מחוון_החלקה" | "سلائیڈر" | "curseur_ui" | "ui_schieberegler" | "ui_ползунок" =>
            {
                let x = self.arg_num(&args, 0, 0.)? as f32;
                let y = self.arg_num(&args, 1, 0.)? as f32;
                let w0 = self.arg_num(&args, 2, 160.)? as f32;
                let mut val = self.arg_num(&args, 3, 0.)? as f32;
                let mn = self.arg_num(&args, 4, 0.)? as f32;
                let mx_ = self.arg_num(&args, 5, 1.)? as f32;
                let (mx, my, down) = self.mouse_now();
                let hover = ling_ui::holo::hit_rect(mx, my, x - 8.0, y - 10.0, w0 + 16.0, 20.0);
                if hover && down {
                    let frac = ((mx - x) / w0).clamp(0.0, 1.0);
                    val = mn + (mx_ - mn) * frac;
                }
                let frac = ((val - mn) / (mx_ - mn).abs().max(1e-6)).clamp(0.0, 1.0);
                let th = self.ui_theme;
                let fill = self.color_at(&args, 6, th.primary);
                self.draw_ui(&ling_ui::widgets::slider(
                    x, y, w0, frac, hover, fill, th.track,
                ));
                return Ok(Value::Number(val as f64));
            },
            #[cfg(not(target_arch = "wasm32"))]
            "ui_checkbox" | "复选框" | "チェックボックス" | "체크박스" | "ช่องเลือก" | "جعبه_علامت" | "مربع_اختيار" | "תיבת_סימון" | "چیک_باکس" | "case_cocher_ui" | "ui_kontrollkästchen" | "ui_флажок" =>
            {
                let x = self.arg_num(&args, 0, 0.)? as f32;
                let y = self.arg_num(&args, 1, 0.)? as f32;
                let s = self.arg_num(&args, 2, 20.)? as f32;
                let mut checked = self.arg_num(&args, 3, 0.)? > 0.5;
                let (mx, my, down) = self.mouse_now();
                let hover = ling_ui::holo::hit_rect(mx, my, x, y, s, s);
                if hover && down && !self.mouse_was_down {
                    checked = !checked;
                }
                let th = self.ui_theme;
                let prim = self.color_at(&args, 4, th.primary);
                self.draw_ui(&ling_ui::widgets::checkbox(
                    x, y, s, checked, hover, prim, th.track,
                ));
                return Ok(Value::Number(if checked { 1.0 } else { 0.0 }));
            },
            #[cfg(not(target_arch = "wasm32"))]
            "ui_tabs" | "标签页" | "タブ" | "탭" | "แท็บ" | "برگه‌ها" | "ألسنة_الواجهة" | "לשוניות" | "ٹیبز" | "onglets_ui" | "ui_reiter" | "ui_вкладки" => {
                let x = self.arg_num(&args, 0, 0.)? as f32;
                let y = self.arg_num(&args, 1, 0.)? as f32;
                let w0 = self.arg_num(&args, 2, 240.)? as f32;
                let h0 = self.arg_num(&args, 3, 28.)? as f32;
                let count = self.arg_num(&args, 4, 3.)? as usize;
                let mut active = self.arg_num(&args, 5, 0.)? as i32;
                let (mx, my, down) = self.mouse_now();
                let mut hover = -1;
                if my >= y && my <= y + h0 && mx >= x && mx <= x + w0 && count > 0 {
                    hover = (((mx - x) / (w0 / count as f32)) as i32)
                        .max(0)
                        .min(count as i32 - 1);
                    if down && !self.mouse_was_down {
                        active = hover;
                    }
                }
                let th = self.ui_theme;
                let prim = self.color_at(&args, 6, th.primary);
                self.draw_ui(&ling_ui::widgets::tabs(
                    x,
                    y,
                    w0,
                    h0,
                    count,
                    active as usize,
                    hover,
                    prim,
                    th.track,
                ));
                return Ok(Value::Number(active as f64));
            },
            #[cfg(not(target_arch = "wasm32"))]
            "ui_progress" | "进度" | "プログレス" | "진행바" | "ความคืบหน้า" | "نوار_پیشرفت" | "شريط_التقدم" | "פס_התקדמות" | "پیش_رفت_بار" | "progression_ui" | "ui_fortschritt" | "ui_прогресс" =>
            {
                let x = self.arg_num(&args, 0, 0.)? as f32;
                let y = self.arg_num(&args, 1, 0.)? as f32;
                let w0 = self.arg_num(&args, 2, 200.)? as f32;
                let h0 = self.arg_num(&args, 3, 12.)? as f32;
                let frac = self.arg_num(&args, 4, 0.)? as f32;
                let th = self.ui_theme;
                let fill = self.color_at(&args, 5, th.accent);
                self.draw_ui(&ling_ui::widgets::progress(
                    x, y, w0, h0, frac, fill, th.track,
                ));
                return Ok(Value::Unit);
            },
            #[cfg(not(target_arch = "wasm32"))]
            "ui_tooltip" | "提示框" | "ツールチップ" | "툴팁" | "คำแนะนำ" | "راهنمای_شناور" | "تلميح_الواجهة" | "חלונית_עזרה" | "ٹول_ٹپ" | "infobulle_ui" | "ui_подсказка" =>
            {
                let x = self.arg_num(&args, 0, 0.)? as f32;
                let y = self.arg_num(&args, 1, 0.)? as f32;
                let w0 = self.arg_num(&args, 2, 120.)? as f32;
                let h0 = self.arg_num(&args, 3, 28.)? as f32;
                let th = self.ui_theme;
                let prim = self.color_at(&args, 4, th.primary);
                self.draw_ui(&ling_ui::widgets::tooltip(x, y, w0, h0, prim, th.bg));
                return Ok(Value::Unit);
            },
            #[cfg(not(target_arch = "wasm32"))]
            "ui_stepper" | "步进器" | "ステッパー" | "스테퍼" | "ตัวปรับค่า" | "پله‌گر" | "زر_خطوات" | "בורר_מדורג" | "اسٹیپر" | "pas_à_pas_ui" | "ui_schrittsteuerung" | "ui_степпер" =>
            {
                let x = self.arg_num(&args, 0, 0.)? as f32;
                let y = self.arg_num(&args, 1, 0.)? as f32;
                let w0 = self.arg_num(&args, 2, 120.)? as f32;
                let h0 = self.arg_num(&args, 3, 28.)? as f32;
                let mut val = self.arg_num(&args, 4, 0.)? as f32;
                let step = self.arg_num(&args, 5, 1.)? as f32;
                let (mx, my, down) = self.mouse_now();
                let hm = ling_ui::holo::hit_rect(mx, my, x, y, h0, h0);
                let hp = ling_ui::holo::hit_rect(mx, my, x + w0 - h0, y, h0, h0);
                if down && !self.mouse_was_down {
                    if hm {
                        val -= step;
                    }
                    if hp {
                        val += step;
                    }
                }
                let th = self.ui_theme;
                let prim = self.color_at(&args, 6, th.primary);
                self.draw_ui(&ling_ui::widgets::stepper(
                    x, y, w0, h0, hm, hp, prim, th.track,
                ));
                return Ok(Value::Number(val as f64));
            },

            // ── Game UI ──────────────────────────────────────────────────────
            #[cfg(not(target_arch = "wasm32"))]
            "ui_healthbar" | "血条" | "体力バー" | "체력바" | "แถบพลังชีวิต" | "نوار_سلامتی" | "شريط_الصحة" | "פס_בריאות" | "ہیلتھ_بار" | "barre_vie_ui" | "ui_lebensbalken" | "ui_полоса_здоровья" =>
            {
                let x = self.arg_num(&args, 0, 0.)? as f32;
                let y = self.arg_num(&args, 1, 0.)? as f32;
                let w0 = self.arg_num(&args, 2, 180.)? as f32;
                let h0 = self.arg_num(&args, 3, 16.)? as f32;
                let val = self.arg_num(&args, 4, 1.)? as f32;
                let max = self.arg_num(&args, 5, 1.)? as f32;
                let pulse = self.arg_num(&args, 6, 0.)? as f32;
                let th = self.ui_theme;
                let full = self.color_at(&args, 7, th.accent);
                self.draw_ui(&ling_ui::widgets::healthbar(
                    x,
                    y,
                    w0,
                    h0,
                    val / max.max(1e-6),
                    pulse,
                    full,
                    th.warn,
                    th.track,
                ));
                return Ok(Value::Unit);
            },
            #[cfg(not(target_arch = "wasm32"))]
            "ui_cooldown" | "冷却" | "クールダウン" | "쿨다운" | "คูลดาวน์" | "زمان_خنک‌سازی" | "مؤقت_التهدئة" | "זמן_קירור" | "کول_ڈاؤن" | "recharge_ui" | "ui_abklingzeit" | "ui_перезарядка" =>
            {
                let cx = self.arg_num(&args, 0, 0.)? as f32;
                let cy = self.arg_num(&args, 1, 0.)? as f32;
                let r = self.arg_num(&args, 2, 28.)? as f32;
                let frac = self.arg_num(&args, 3, 0.)? as f32;
                let th = self.ui_theme;
                let fill = self.color_at(&args, 4, th.primary);
                self.draw_ui(&ling_ui::widgets::cooldown(cx, cy, r, frac, fill, th.track));
                return Ok(Value::Unit);
            },
            #[cfg(not(target_arch = "wasm32"))]
            "ui_counter" | "计数器" | "カウンター" | "카운터" | "ตัวนับ" | "شمارشگر" | "عداد_الواجهة" | "מונה_ממשק" | "کاؤنٹر" | "compteur_ui" | "ui_zähler" | "ui_счётчик" => {
                let x = self.arg_num(&args, 0, 0.)? as f32;
                let y = self.arg_num(&args, 1, 0.)? as f32;
                let dw = self.arg_num(&args, 2, 14.)? as f32;
                let dh = self.arg_num(&args, 3, 24.)? as f32;
                let val = self.arg_num(&args, 4, 0.)? as i64;
                let digits = self.arg_num(&args, 5, 4.)? as usize;
                let th = self.ui_theme;
                let on = self.color_at(&args, 6, th.primary);
                let off = ling_ui::widgets::shade(th.track, 0.5);
                self.draw_ui(&ling_ui::widgets::counter(
                    x, y, dw, dh, val, digits, on, off,
                ));
                return Ok(Value::Unit);
            },
            #[cfg(not(target_arch = "wasm32"))]
            "ui_minimap" | "小地图" | "ミニマップ" | "미니맵" | "แผนที่ย่อ" | "نقشه_کوچک" | "خريطة_مصغرة" | "מפה_מוקטנת" | "منی_میپ" | "minicarte_ui" | "ui_minikarte" | "ui_миникарта" =>
            {
                let x = self.arg_num(&args, 0, 0.)? as f32;
                let y = self.arg_num(&args, 1, 0.)? as f32;
                let w0 = self.arg_num(&args, 2, 140.)? as f32;
                let h0 = self.arg_num(&args, 3, 140.)? as f32;
                let th = self.ui_theme;
                let prim = self.color_at(&args, 4, th.primary);
                self.draw_ui(&ling_ui::widgets::minimap(x, y, w0, h0, prim, th.bg));
                return Ok(Value::Unit);
            },
            #[cfg(not(target_arch = "wasm32"))]
            "ui_dpad" | "方向键" | "方向パッド" | "방향패드" | "ปุ่มทิศทาง" | "دسته_جهت‌دار" | "لوحة_الاتجاهات" | "לוח_כיוונים" | "ڈی_پیڈ" | "croix_direction_ui" | "ui_steuerkreuz" | "ui_крестовина" =>
            {
                let cx = self.arg_num(&args, 0, 0.)? as f32;
                let cy = self.arg_num(&args, 1, 0.)? as f32;
                let r = self.arg_num(&args, 2, 50.)? as f32;
                let (mx, my, down) = self.mouse_now();
                let mut dir = 0;
                if down {
                    let (dx, dy) = (mx - cx, my - cy);
                    if dx * dx + dy * dy <= r * r {
                        if dx.abs() > dy.abs() {
                            dir = if dx > 0.0 { 2 } else { 4 };
                        } else {
                            dir = if dy > 0.0 { 3 } else { 1 };
                        }
                    }
                }
                let th = self.ui_theme;
                let prim = self.color_at(&args, 3, th.primary);
                self.draw_ui(&ling_ui::widgets::dpad(cx, cy, r, dir, prim, th.track));
                return Ok(Value::Number(dir as f64));
            },
            #[cfg(not(target_arch = "wasm32"))]
            "ui_slotgrid" | "物品格" | "スロットグリッド" | "슬롯격자" | "ช่องไอเทม" | "شبکه_شیار" | "شبكة_الفتحات" | "רשת_חריצים" | "سلاٹ_گرڈ" | "grille_emplacements_ui" | "ui_slotraster" | "ui_сетка_слотов" =>
            {
                let x = self.arg_num(&args, 0, 0.)? as f32;
                let y = self.arg_num(&args, 1, 0.)? as f32;
                let cols = self.arg_num(&args, 2, 4.)? as usize;
                let rows = self.arg_num(&args, 3, 1.)? as usize;
                let cell = self.arg_num(&args, 4, 36.)? as f32;
                let sel = self.arg_num(&args, 5, -1.)? as i32;
                let th = self.ui_theme;
                let prim = self.color_at(&args, 6, th.primary);
                self.draw_ui(&ling_ui::widgets::slotgrid(
                    x, y, cols, rows, cell, sel, prim, th.track,
                ));
                return Ok(Value::Unit);
            },
            #[cfg(not(target_arch = "wasm32"))]
            "ui_vignette" | "暗角" | "ビネット" | "비네트" | "ขอบมืด" | "سایه‌گرد_کادر" | "تظليل_الحواف" | "הצללת_מסגרת" | "ویگنیٹ" | "vignette_ui" | "ui_виньетка" => {
                let intensity = self.arg_num(&args, 0, 0.5)? as f32;
                let (w, h) = {
                    let g = self.gfx.borrow();
                    (g.width as f32, g.height as f32)
                };
                let th = self.ui_theme;
                let col = self.color_at(&args, 1, th.warn);
                self.draw_ui(&ling_ui::widgets::vignette(w, h, intensity, col));
                return Ok(Value::Unit);
            },

            // ── Faux-3D in 2D space ──────────────────────────────────────────
            #[cfg(not(target_arch = "wasm32"))]
            "ui_gauge3d" | "立体仪表" | "立体ゲージ" | "입체게이지" | "มาตรวัด3มิติ" | "گیج_سه‌بعدی" | "مقياس_ثلاثي_الأبعاد" | "מד_תלת_ממדי" | "تھری_ڈی_گیج" | "jauge_3d_ui" | "ui_anzeige_3d" | "ui_индикатор_3d" =>
            {
                let cx = self.arg_num(&args, 0, 0.)? as f32;
                let cy = self.arg_num(&args, 1, 0.)? as f32;
                let r = self.arg_num(&args, 2, 50.)? as f32;
                let val = self.arg_num(&args, 3, 0.)? as f32;
                let max = self.arg_num(&args, 4, 1.)? as f32;
                let spin = self.arg_num(&args, 5, 0.)? as f32;
                let th = self.ui_theme;
                let fill = self.color_at(&args, 6, th.primary);
                self.draw_ui(&ling_ui::widgets::gauge3d(
                    cx,
                    cy,
                    r,
                    val / max.max(1e-6),
                    spin,
                    fill,
                    th.track,
                ));
                return Ok(Value::Unit);
            },
            #[cfg(not(target_arch = "wasm32"))]
            "ui_panel3d" | "立体面板" | "立体パネル" | "입체패널" | "แผง3มิติ" | "پنل_سه‌بعدی" | "لوحة_ثلاثية_الأبعاد" | "לוח_תלת_ממדי" | "تھری_ڈی_پینل" | "panneau_3d_ui" | "ui_feld_3d" | "ui_панель_3d" =>
            {
                let x = self.arg_num(&args, 0, 0.)? as f32;
                let y = self.arg_num(&args, 1, 0.)? as f32;
                let w0 = self.arg_num(&args, 2, 200.)? as f32;
                let h0 = self.arg_num(&args, 3, 120.)? as f32;
                let depth = self.arg_num(&args, 4, 14.)? as f32;
                let th = self.ui_theme;
                let prim = self.color_at(&args, 5, th.primary);
                self.draw_ui(&ling_ui::widgets::panel3d(x, y, w0, h0, depth, prim, th.bg));
                return Ok(Value::Unit);
            },
            #[cfg(not(target_arch = "wasm32"))]
            "ui_radar3d" | "立体雷达" | "立体レーダー" | "입체레이더" | "เรดาร์3มิติ" | "رادار_سه‌بعدی" | "رادار_ثلاثي_الأبعاد" | "מכ״ם_תלת_ממדי" | "تھری_ڈی_ریڈار" | "radar_3d_ui" | "ui_radar_3d" | "ui_радар_3d" =>
            {
                let cx = self.arg_num(&args, 0, 0.)? as f32;
                let cy = self.arg_num(&args, 1, 0.)? as f32;
                let r = self.arg_num(&args, 2, 60.)? as f32;
                let tilt = self.arg_num(&args, 3, 0.9)? as f32;
                let sweep = self.arg_num(&args, 4, 0.)? as f32;
                let th = self.ui_theme;
                let prim = self.color_at(&args, 5, th.primary);
                self.draw_ui(&ling_ui::widgets::radar3d(
                    cx, cy, r, tilt, sweep, prim, th.track,
                ));
                return Ok(Value::Unit);
            },

            // ── Interface sounds ─────────────────────────────────────────────
            #[cfg(not(target_arch = "wasm32"))]
            "audio_blip" | "提示音" | "ビープ音" | "효과음" | "เสียงบี๊บ" | "بوق_کوتاه" | "نغمة_قصيرة" | "ביפ" | "بلپ_آواز" | "bip_audio" | "звук_бип" =>
            {
                let freq = self.arg_num(&args, 0, 660.)? as f32;
                let dur = self.arg_num(&args, 1, 0.08)? as f32;
                let wave = Wave::from_name(&self.arg_str(&args, 2, "sine"));
                let amp = self.arg_num(&args, 3, 0.25)? as f32;
                if let Some(audio) = &self.audio {
                    audio.blip(freq, amp, dur, wave);
                }
                return Ok(Value::Unit);
            },
            #[cfg(not(target_arch = "wasm32"))]
            "ui_sound" | "界面音" | "UI音" | "인터페이스음" | "เสียงปุ่ม" | "صدای_رابط" | "صوت_الواجهة" | "צליל_ממשק" | "یو_آئی_آواز" | "son_ui" | "ui_klang" | "ui_звук" =>
            {
                let name = self.arg_str(&args, 0, "click");
                if let Some(audio) = &self.audio {
                    match name.as_str() {
                        "hover" => audio.blip(880.0, 0.10, 0.04, Wave::Sine),
                        "confirm" => {
                            audio.blip(660.0, 0.22, 0.07, Wave::Square);
                            audio.blip(990.0, 0.18, 0.10, Wave::Square);
                        },
                        "error" => {
                            audio.blip(180.0, 0.30, 0.16, Wave::Saw);
                            audio.blip(140.0, 0.30, 0.18, Wave::Saw);
                        },
                        "toggle" => audio.blip(520.0, 0.22, 0.05, Wave::Triangle),
                        "tick" => audio.blip(1500.0, 0.12, 0.02, Wave::Square),
                        _ => audio.blip(720.0, 0.26, 0.05, Wave::Square), // "click"
                    }
                }
                return Ok(Value::Unit);
            },

            // ══════════════════════════════════════════════════════════════════
            // MUSIC TOOLKIT  (crates/ling-music) — decode · analysis · GM synth ·
            // rhythm · karaoke. Analysis/decoding need no audio device; playback
            // and synthesis lazily start a dedicated music engine.
            // ══════════════════════════════════════════════════════════════════

            // music_load(path) -> track handle (decodes WAV/FLAC/OGG/MP3/AAC)
            #[cfg(not(target_arch = "wasm32"))]
            "music_load" | "载入音乐" | "音楽読込" | "음악로드" | "โหลดเพลง" | "بارگذاری_موسیقی" | "تحميل_الموسيقى" | "טעינת_מוזיקה" | "موسیقی_لوڈ" | "charger_musique" | "musik_laden" | "загрузить_музыку" =>
            {
                let path = self.arg_str(&args, 0, "");
                let resolved = if std::path::Path::new(&path).exists() {
                    path.clone()
                } else if let Some(d) = &self.source_dir {
                    d.join(&path).to_string_lossy().into_owned()
                } else {
                    path.clone()
                };
                match ling_music::load(&resolved) {
                    Ok(t) => {
                        let id = self.tracks.len();
                        self.tracks.push(t);
                        return Ok(Value::Number(id as f64));
                    },
                    Err(e) => {
                        eprintln!("music_load failed ({path}): {e}");
                        return Ok(Value::Number(-1.0));
                    },
                }
            },
            #[cfg(not(target_arch = "wasm32"))]
            "music_duration" | "音乐时长" | "音楽長さ" | "음악길이" | "ความยาวเพลง" | "مدت_موسیقی" | "مدة_الموسيقى" | "משך_מוזיקה" | "موسیقی_دورانیہ" | "durée_musique" | "musik_dauer" | "длительность_музыки" =>
            {
                let id = self.arg_num(&args, 0, 0.0)? as i64;
                let d = self
                    .tracks
                    .get(id as usize)
                    .map(|t| t.duration)
                    .unwrap_or(0.0);
                return Ok(Value::Number(d as f64));
            },
            #[cfg(not(target_arch = "wasm32"))]
            "music_bpm" | "节拍速度" | "テンポ" | "템포" | "จังหวะต่อนาที" | "ضربان_در_دقیقه" | "نبضات_بالدقيقة" | "פעימות_לדקה" | "بی_پی_ایم" | "bpm_musique" | "musik_bpm" | "музыка_bpm" =>
            {
                let id = self.arg_num(&args, 0, 0.0)? as i64;
                let b = self
                    .tracks
                    .get(id as usize)
                    .map(|t| ling_music::analysis::bpm(&t.mono, t.rate))
                    .unwrap_or(0.0);
                return Ok(Value::Number(b as f64));
            },
            #[cfg(not(target_arch = "wasm32"))]
            "music_key" | "调性" | "調性" | "조성" | "คีย์เพลง" | "گام_موسیقی" | "مقام_الموسيقى" | "סולם_מוזיקלי" | "موسیقی_کلید" | "tonalité_musique" | "musik_tonart" | "тональность_музыки" => {
                let id = self.arg_num(&args, 0, 0.0)? as i64;
                let k = self
                    .tracks
                    .get(id as usize)
                    .map(|t| ling_music::analysis::key_name(&t.mono, t.rate))
                    .unwrap_or_default();
                return Ok(Value::Str(k));
            },
            #[cfg(not(target_arch = "wasm32"))]
            "music_onsets" | "音符起点" | "オンセット" | "온셋" | "จุดเริ่มเสียง" | "آغازهای_نت" | "بدايات_النغمات" | "התחלות_תווים" | "نوٹ_شروعات" | "attaques_musique" | "musik_einsätze" | "атаки_музыки" =>
            {
                let id = self.arg_num(&args, 0, 0.0)? as i64;
                let v = self
                    .tracks
                    .get(id as usize)
                    .map(|t| ling_music::analysis::onsets(&t.mono, t.rate))
                    .unwrap_or_default();
                return Ok(Value::List(Rc::new(
                    v.into_iter().map(|x| Value::Number(x as f64)).collect(),
                )));
            },
            #[cfg(not(target_arch = "wasm32"))]
            "music_beat_grid" | "节拍网格" | "ビートグリッド" | "비트그리드" | "กริดจังหวะ" | "شبکه_ضرب" | "شبكة_الإيقاع" | "רשת_פעימות" | "بیٹ_گرڈ" | "grille_temps_musique" | "musik_taktraster" | "сетка_ритма_музыки" =>
            {
                let id = self.arg_num(&args, 0, 0.0)? as i64;
                let beats = self
                    .tracks
                    .get(id as usize)
                    .map(|t| {
                        let b = ling_music::analysis::bpm(&t.mono, t.rate);
                        ling_music::analysis::beat_grid(&t.mono, t.rate, b)
                    })
                    .unwrap_or_default();
                return Ok(Value::List(Rc::new(
                    beats.into_iter().map(|x| Value::Number(x as f64)).collect(),
                )));
            },

            // ── playback ──
            #[cfg(not(target_arch = "wasm32"))]
            "music_play" | "播放音乐" | "音楽再生" | "음악재생" | "เล่นเพลง" | "پخش_موسیقی" | "شغّل_الموسيقى" | "נגן_מוזיקה" | "موسیقی_چلاؤ" | "jouer_musique" | "musik_abspielen" | "играть_музыку" =>
            {
                let id = self.arg_num(&args, 0, 0.0)? as i64;
                if self.ensure_music() {
                    let track = self
                        .tracks
                        .get(id as usize)
                        .map(|t| (t.stereo.clone(), t.rate));
                    if let (Some((st, rate)), Some(m)) = (track, &self.music) {
                        m.set_track(st, rate);
                        m.play();
                    } else if let Some(m) = &self.music {
                        m.play();
                    }
                }
                return Ok(Value::Unit);
            },
            #[cfg(not(target_arch = "wasm32"))]
            "music_pause" | "暂停音乐" | "音楽一時停止" | "음악일시정지" | "หยุดเพลงชั่วคราว" | "مکث_موسیقی" | "ألبث_الموسيقى" | "השהה_מוזיקה" | "موسیقی_روکو_مؤقت" | "pause_musique" | "musik_pausieren" | "пауза_музыки" =>
            {
                if let Some(m) = &self.music {
                    m.pause();
                }
                return Ok(Value::Unit);
            },
            #[cfg(not(target_arch = "wasm32"))]
            "music_stop" | "停止音乐" | "音楽停止" | "음악정지" | "หยุดเพลง" | "توقف_موسیقی" | "أوقف_الموسيقى" | "עצור_מוזיקה" | "موسیقی_روکو" | "arrêter_musique" | "musik_stoppen" | "остановить_музыку" =>
            {
                if let Some(m) = &self.music {
                    m.stop();
                }
                return Ok(Value::Unit);
            },
            #[cfg(not(target_arch = "wasm32"))]
            "music_seek" | "定位音乐" | "音楽シーク" | "음악탐색" | "ค้นหาเพลง" | "جستجوی_موسیقی" | "ابحث_في_الموسيقى" | "חפש_במוזיקה" | "موسیقی_تلاش" | "chercher_musique" | "musik_suchen" | "перемотать_музыку" =>
            {
                let sec = self.arg_num(&args, 0, 0.0)? as f32;
                if let Some(m) = &self.music {
                    m.seek(sec);
                }
                return Ok(Value::Unit);
            },
            #[cfg(not(target_arch = "wasm32"))]
            "music_pos" | "音乐位置" | "音楽位置" | "음악위치" | "ตำแหน่งเพลง" | "موقعیت_موسیقی" | "موضع_الموسيقى" | "מיקום_מוזיקה" | "موسیقی_مقام" | "position_musique" | "musik_position" | "позиция_музыки" =>
            {
                let p = self.music.as_ref().map(|m| m.position()).unwrap_or(0.0);
                return Ok(Value::Number(p as f64));
            },
            #[cfg(not(target_arch = "wasm32"))]
            "music_volume" | "音乐音量" | "音楽音量" | "음악음량" | "ระดับเพลง" | "بلندی_موسیقی" | "مستوى_الموسيقى" | "עוצמת_מוזיקה" | "موسیقی_شدت" | "volume_musique" | "musik_lautstärke" | "громкость_музыки" =>
            {
                let v = self.arg_num(&args, 0, 0.8)? as f32;
                if self.ensure_music() {
                    if let Some(m) = &self.music {
                        m.set_volume(v);
                    }
                }
                return Ok(Value::Unit);
            },

            // ── synthesis (GM-capable, patches from .ling files) ──
            #[cfg(not(target_arch = "wasm32"))]
            "music_patch" | "乐器音色" | "音色読込" | "악기패치" | "แพตช์เครื่องดนตรี" | "پچ_موسیقی" | "آلة_الموسيقى" | "תיקון_כלי_נגינה" | "میوزک_پیچ" | "patch_musique" | "musik_patch" | "патч_музыки" =>
            {
                let path = self.arg_str(&args, 0, "");
                let resolved = if std::path::Path::new(&path).exists() {
                    path.clone()
                } else if let Some(d) = &self.source_dir {
                    d.join(&path).to_string_lossy().into_owned()
                } else {
                    path.clone()
                };
                if !self.ensure_music() {
                    return Ok(Value::Number(-1.0));
                }
                match ling_music::patch::from_path(&resolved) {
                    Ok(p) => {
                        let id = self.music.as_ref().unwrap().add_patch(p);
                        return Ok(Value::Number(id as f64));
                    },
                    Err(e) => {
                        eprintln!("music_patch failed ({path}): {e}");
                        return Ok(Value::Number(-1.0));
                    },
                }
            },
            #[cfg(not(target_arch = "wasm32"))]
            "music_note" | "弹音符" | "音符演奏" | "음표연주" | "เล่นโน้ต" | "نواختن_نت" | "عزف_نغمة" | "נגן_תו" | "نوٹ_بجاؤ" | "note_musique" | "musik_note" | "нота_музыки" =>
            {
                let inst = self.arg_num(&args, 0, 0.0)? as usize;
                let midi = self.pitch_arg(&args, 1, 60);
                let dur = self.arg_num(&args, 2, 0.5)? as f32;
                let vel = self.arg_num(&args, 3, 0.9)? as f32;
                if self.ensure_music() {
                    if let Some(m) = &self.music {
                        m.note(inst, midi, vel, dur);
                    }
                }
                return Ok(Value::Unit);
            },
            #[cfg(not(target_arch = "wasm32"))]
            "music_note_on" | "音符开始" | "音符オン" | "음표켜기" | "โน้ตเริ่ม" | "شروع_نت" | "بدء_النغمة" | "התחלת_תו" | "نوٹ_شروع" | "note_musique_on" | "musik_note_an" | "нота_музыки_вкл" =>
            {
                let inst = self.arg_num(&args, 0, 0.0)? as usize;
                let midi = self.pitch_arg(&args, 1, 60);
                let vel = self.arg_num(&args, 2, 0.9)? as f32;
                if self.ensure_music() {
                    if let Some(m) = &self.music {
                        m.note_on(inst, midi, vel);
                    }
                }
                return Ok(Value::Unit);
            },
            #[cfg(not(target_arch = "wasm32"))]
            "music_note_off" | "音符结束" | "音符オフ" | "음표끄기" | "โน้ตจบ" | "پایان_نت" | "إيقاف_النغمة" | "סיום_תו" | "نوٹ_ختم" | "note_musique_off" | "musik_note_aus" | "нота_музыки_выкл" =>
            {
                let inst = self.arg_num(&args, 0, 0.0)? as usize;
                let midi = self.pitch_arg(&args, 1, 60);
                if let Some(m) = &self.music {
                    m.note_off(inst, midi);
                }
                return Ok(Value::Unit);
            },

            // ── rhythm-game judging ──
            #[cfg(not(target_arch = "wasm32"))]
            "music_judge" | "判定" | "判定する" | "판정" | "ตัดสินจังหวะ" | "داوری_ضرب" | "حكم_الإيقاع" | "שיפוט_קצב" | "بیٹ_فیصلہ" | "juger_musique" | "musik_bewerten" | "оценить_музыку" =>
            {
                let delta_ms = self.arg_num(&args, 0, 9999.0)? as f32;
                return Ok(Value::Number(
                    ling_music::Grade::judge(delta_ms).index() as f64
                ));
            },
            #[cfg(not(target_arch = "wasm32"))]
            "music_grade_name" | "判定名" | "判定名称" | "판정이름" | "ชื่อการตัดสิน" | "نام_رتبه" | "اسم_التقييم" | "שם_דירוג" | "گریڈ_نام" | "nom_grade_musique" | "musik_bewertungsname" | "имя_оценки_музыки" =>
            {
                let idx = self.arg_num(&args, 0, 4.0)? as i32;
                return Ok(Value::Str(
                    ling_music::Grade::from_index(idx).name().to_string(),
                ));
            },

            // ── karaoke ──
            #[cfg(not(target_arch = "wasm32"))]
            "music_lrc" | "载入歌词" | "歌詞読込" | "가사로드" | "โหลดเนื้อเพลง" | "بارگذاری_متن_ترانه" | "تحميل_كلمات_الأغنية" | "טעינת_מילות_שיר" | "گیت_متن_لوڈ" | "lrc_musique" | "musik_lrc" | "lrc_музыки" =>
            {
                let path = self.arg_str(&args, 0, "");
                let resolved = if std::path::Path::new(&path).exists() {
                    path.clone()
                } else if let Some(d) = &self.source_dir {
                    d.join(&path).to_string_lossy().into_owned()
                } else {
                    path.clone()
                };
                match std::fs::read_to_string(&resolved) {
                    Ok(text) => {
                        let id = self.lyrics.len();
                        self.lyrics.push(ling_music::Lyrics::parse(&text));
                        return Ok(Value::Number(id as f64));
                    },
                    Err(e) => {
                        eprintln!("music_lrc failed ({path}): {e}");
                        return Ok(Value::Number(-1.0));
                    },
                }
            },
            #[cfg(not(target_arch = "wasm32"))]
            "music_lyric" | "当前歌词" | "現在歌詞" | "현재가사" | "เนื้อเพลงปัจจุบัน" | "متن_ترانه_فعلی" | "كلمات_الأغنية_الحالية" | "מילות_שיר_נוכחיות" | "موجودہ_گیت_متن" | "paroles_musique" | "musik_liedtext" | "текст_песни" =>
            {
                let id = self.arg_num(&args, 0, 0.0)? as i64;
                let t = self.arg_num(&args, 1, 0.0)? as f32;
                let line = self
                    .lyrics
                    .get(id as usize)
                    .map(|l| l.line_at(t).to_string())
                    .unwrap_or_default();
                return Ok(Value::Str(line));
            },
            #[cfg(not(target_arch = "wasm32"))]
            "music_mic_pitch" | "麦克风音高" | "マイク音程" | "마이크음정" | "ระดับเสียงไมค์" | "زیروبمی_میکروفون" | "طبقة_صوت_الميكروفون" | "גובה_צליל_מיקרופון" | "مائیکروفون_پچ" | "hauteur_micro_musique" | "musik_mikrofon_tonhöhe" | "высота_тона_микрофона" =>
            {
                let hz = if let Some(mic) = self.mic.as_ref() {
                    let s = mic.latest_samples();
                    let rate = mic.sample_rate();
                    ling_music::pitch::detect(&s, rate).unwrap_or(0.0)
                } else {
                    0.0
                };
                return Ok(Value::Number(hz as f64));
            },
            #[cfg(not(target_arch = "wasm32"))]
            "music_note_name" | "音名" | "音名称" | "음이름" | "ชื่อโน้ต" | "نام_نت" | "اسم_النغمة" | "שם_תו" | "نوٹ_نام" | "nom_note_musique" | "musik_notenname" | "имя_ноты_музыки" =>
            {
                let hz = self.arg_num(&args, 0, 0.0)? as f32;
                return Ok(Value::Str(ling_music::note::hz_to_name(hz)));
            },
            #[cfg(not(target_arch = "wasm32"))]
            "music_hz" | "音符频率" | "音符周波数" | "음표주파수" | "ความถี่โน้ต" | "فرکانس_نت" | "تردد_النغمة" | "תדר_תו" | "نوٹ_ہرٹز" | "hz_musique" | "musik_hz" | "музыка_гц" =>
            {
                let midi = self.pitch_arg(&args, 0, 69);
                return Ok(Value::Number(
                    ling_music::note::midi_to_hz(midi as f32) as f64
                ));
            },
            #[cfg(not(target_arch = "wasm32"))]
            "music_pitch_score" | "音准评分" | "音程スコア" | "음정점수" | "คะแนนเสียง" | "امتیاز_زیروبمی" | "درجة_طبقة_الصوت" | "ציון_גובה_צליל" | "پچ_اسکور" | "score_hauteur_musique" | "musik_tonhöhen_punktzahl" | "счёт_высоты_тона" =>
            {
                let hz = self.arg_num(&args, 0, 0.0)? as f32;
                let target = self.arg_num(&args, 1, 0.0)? as f32;
                return Ok(Value::Number(
                    ling_music::karaoke::pitch_score(hz, target) as f64
                ));
            },

            // ── MIDI (inaudible note source: drive coins, cues, etc.) ──
            #[cfg(not(target_arch = "wasm32"))]
            "music_midi_load" | "载入MIDI" | "MIDI読込" | "미디로드" | "โหลดมิดี" | "بارگذاری_MIDI" | "تحميل_MIDI" | "טעינת_MIDI" | "MIDI_لوڈ" | "charger_midi_musique" | "musik_midi_laden" | "загрузить_midi_музыки" =>
            {
                let path = self.arg_str(&args, 0, "");
                let resolved = if std::path::Path::new(&path).exists() {
                    path.clone()
                } else if let Some(d) = &self.source_dir {
                    d.join(&path).to_string_lossy().into_owned()
                } else {
                    path.clone()
                };
                match ling_music::midi::load(&resolved) {
                    Ok(m) => {
                        let id = self.midis.len();
                        self.midis.push(m);
                        return Ok(Value::Number(id as f64));
                    },
                    Err(e) => {
                        eprintln!("music_midi_load failed ({path}): {e}");
                        return Ok(Value::Number(-1.0));
                    },
                }
            },
            #[cfg(not(target_arch = "wasm32"))]
            "music_midi_count" | "MIDI数量" | "MIDI数" | "미디수" | "จำนวนมิดี" | "تعداد_MIDI" | "عدد_MIDI" | "מספר_MIDI" | "MIDI_تعداد" | "nombre_midi_musique" | "musik_midi_anzahl" | "число_midi_музыки" =>
            {
                let id = self.arg_num(&args, 0, 0.0)? as i64;
                let n = self
                    .midis
                    .get(id as usize)
                    .map(|m| m.notes.len())
                    .unwrap_or(0);
                return Ok(Value::Number(n as f64));
            },
            // music_midi_notes(id) -> flat [time, midi, time, midi, …]
            #[cfg(not(target_arch = "wasm32"))]
            "music_midi_notes" | "MIDI音符" | "MIDIノート" | "미디음표" | "โน้ตมิดี" | "نت‌های_MIDI" | "نغمات_MIDI" | "תווי_MIDI" | "MIDI_نوٹس" | "notes_midi_musique" | "musik_midi_noten" | "ноты_midi_музыки" =>
            {
                let id = self.arg_num(&args, 0, 0.0)? as i64;
                let mut out = Vec::new();
                if let Some(m) = self.midis.get(id as usize) {
                    for n in &m.notes {
                        out.push(Value::Number(n.time as f64));
                        out.push(Value::Number(n.midi as f64));
                    }
                }
                return Ok(Value::List(Rc::new(out)));
            },
            // music_midi_bars(id) -> flat [time, midi, dur, …] (for karaoke note bars)
            #[cfg(not(target_arch = "wasm32"))]
            "music_midi_bars" | "MIDI音条" | "MIDIバー" | "미디바" | "แท่งมิดี" | "میله‌های_MIDI" | "أعمدة_MIDI" | "עמודות_MIDI" | "MIDI_بارز" | "mesures_midi_musique" | "musik_midi_takte" | "такты_midi_музыки" =>
            {
                let id = self.arg_num(&args, 0, 0.0)? as i64;
                let mut out = Vec::new();
                if let Some(m) = self.midis.get(id as usize) {
                    for n in &m.notes {
                        out.push(Value::Number(n.time as f64));
                        out.push(Value::Number(n.midi as f64));
                        out.push(Value::Number(n.dur as f64));
                    }
                }
                return Ok(Value::List(Rc::new(out)));
            },

            // music_fft(track_id, nbands) -> spectrum at the current playback position
            #[cfg(not(target_arch = "wasm32"))]
            "music_fft" | "音乐频谱" | "音楽スペクトル" | "음악스펙트럼" | "สเปกตรัมเพลง" | "طیف_موسیقی" | "طيف_الموسيقى" | "ספקטרום_מוזיקה" | "میوزک_اسپیکٹرم" | "fft_musique" | "musik_fft" | "fft_музыки" =>
            {
                let id = self.arg_num(&args, 0, 0.0)? as i64;
                let nbands = self.arg_num(&args, 1, 16.0)? as usize;
                let pos = self.music.as_ref().map(|m| m.position()).unwrap_or(0.0);
                if let Some(t) = self.tracks.get(id as usize) {
                    let idx = (pos * t.rate as f32) as usize;
                    let end = (idx + 2048).min(t.mono.len());
                    if end > idx + 64 {
                        self.fft.borrow_mut().push_samples(&t.mono[idx..end]);
                    }
                }
                let bands = self.fft.borrow().freq_bands(nbands);
                return Ok(Value::List(Rc::new(
                    bands.into_iter().map(|x| Value::Number(x as f64)).collect(),
                )));
            },

            // ── stop every one-shot SFX/morph/sample voice (scene cleanup) ──
            #[cfg(not(target_arch = "wasm32"))]
            "audio_stop_sfx" | "停止音效" | "効果音停止" | "효과음정지" | "หยุดเอฟเฟกต์ทั้งหมด" | "توقف_همه_جلوه‌ها" | "أوقف_كل_المؤثرات" | "עצור_כל_האפקטים" | "تمام_ایفیکٹ_روکو" =>
            {
                if let Some(a) = &self.audio {
                    a.stop_all_sfx();
                }
                return Ok(Value::Unit);
            },
            // ── spatial (2D/3D/4D) one-shot SFX ──
            #[cfg(not(target_arch = "wasm32"))]
            "audio_sfx" | "音效" | "空間効果音" | "공간효과음" | "เสียงเอฟเฟกต์" | "جلوه_صوتی" | "مؤثرات_صوتية" | "אפקט_קול" | "آواز_ایفیکٹ" | "effet_sonore" | "klangeffekt" | "звуковой_эффект" =>
            {
                let x = self.arg_num(&args, 0, 0.0)? as f32;
                let y = self.arg_num(&args, 1, 0.0)? as f32;
                let z = self.arg_num(&args, 2, 0.0)? as f32;
                let w = self.arg_num(&args, 3, 1.0)? as f32;
                let freq = self.arg_num(&args, 4, 440.0)? as f32;
                let amp = self.arg_num(&args, 5, 0.3)? as f32;
                let dur = self.arg_num(&args, 6, 0.15)? as f32;
                let wave = Wave::from_name(&self.arg_str(&args, 7, "sine"));
                if let Some(a) = &self.audio {
                    a.sfx(x, y, z, w, freq, amp, dur, wave);
                }
                return Ok(Value::Unit);
            },
            // ── YIN-YANG morph synth note: physical-model(light) ↔ FM/crush(dark) ──
            // โน้ตมอร์ฟ(x,y,z,w, freq, amp, dur, material, morph)
            //   material: 0 bowed-string · 1 plucked · 2 blown · 3 struck-metal
            //   morph:    0.0 light/acoustic .. 1.0 dark/digital
            #[cfg(not(target_arch = "wasm32"))]
            "morph_note" | "โน้ตมอร์ฟ" | "变形音" | "モーフ音" | "모프음" | "نت_مورف" | "نغمة_متحولة" | "תו_מורף" | "مورف_نوٹ" =>
            {
                let x = self.arg_num(&args, 0, 0.0)? as f32;
                let y = self.arg_num(&args, 1, 0.0)? as f32;
                let z = self.arg_num(&args, 2, 0.0)? as f32;
                let w = self.arg_num(&args, 3, 1.0)? as f32;
                let freq = self.arg_num(&args, 4, 220.0)? as f32;
                let amp = self.arg_num(&args, 5, 0.3)? as f32;
                let dur = self.arg_num(&args, 6, 0.6)? as f32;
                let material = self.arg_num(&args, 7, 0.0)?.clamp(0.0, 3.0) as u8;
                let morph = self.arg_num(&args, 8, 0.0)? as f32;
                if let Some(a) = &self.audio {
                    a.morph_note(x, y, z, w, freq, amp, dur, material, morph);
                }
                return Ok(Value::Unit);
            },
            // ── sample load / positional play / loop / stop ──
            #[cfg(not(target_arch = "wasm32"))]
            "audio_sample_load" | "载入采样" | "サンプル読込" | "샘플로드" | "โหลดตัวอย่างเสียง" | "بارگذاری_نمونه_صدا" | "تحميل_عينة_صوتية" | "טעינת_דגימת_קול" | "آواز_نمونہ_لوڈ" | "charger_échantillon" | "sample_laden" | "загрузить_семпл" =>
            {
                let path = self.arg_str(&args, 0, "");
                let resolved = if std::path::Path::new(&path).exists() {
                    path.clone()
                } else if let Some(d) = &self.source_dir {
                    d.join(&path).to_string_lossy().into_owned()
                } else {
                    path.clone()
                };
                match ling_music::load(&resolved) {
                    Ok(t) => {
                        if let Some(a) = &self.audio {
                            return Ok(Value::Number(a.add_sample(t.mono, t.rate) as f64));
                        }
                        return Ok(Value::Number(-1.0));
                    },
                    Err(e) => {
                        eprintln!("audio_sample_load failed ({path}): {e}");
                        return Ok(Value::Number(-1.0));
                    },
                }
            },
            #[cfg(not(target_arch = "wasm32"))]
            "audio_sample_play" | "播放采样" | "サンプル再生" | "샘플재생" | "เล่นตัวอย่างเสียง" | "پخش_نمونه_صدا" | "تشغيل_عينة_صوتية" | "נגינת_דגימת_קול" | "آواز_نمونہ_چلاؤ" | "jouer_échantillon" | "sample_abspielen" | "играть_семпл" =>
            {
                let id = self.arg_num(&args, 0, 0.0)? as usize;
                let x = self.arg_num(&args, 1, 0.0)? as f32;
                let y = self.arg_num(&args, 2, 0.0)? as f32;
                let z = self.arg_num(&args, 3, 0.0)? as f32;
                let w = self.arg_num(&args, 4, 1.0)? as f32;
                let vol = self.arg_num(&args, 5, 1.0)? as f32;
                let looping = self.arg_num(&args, 6, 0.0)? > 0.5;
                let v = self
                    .audio
                    .as_ref()
                    .map(|a| a.play_sample(id, x, y, z, w, vol, looping))
                    .unwrap_or(0);
                return Ok(Value::Number(v as f64));
            },
            #[cfg(not(target_arch = "wasm32"))]
            "audio_sample_stop" | "停止采样" | "サンプル停止" | "샘플정지" | "หยุดตัวอย่างเสียง" | "توقف_نمونه_صدا" | "إيقاف_عينة_صوتية" | "עצירת_דגימת_קול" | "آواز_نمونہ_روکو" | "arrêter_échantillon" | "sample_stoppen" | "остановить_семпл" =>
            {
                let v = self.arg_num(&args, 0, 0.0)? as u32;
                if let Some(a) = &self.audio {
                    a.stop_sample(v);
                }
                return Ok(Value::Unit);
            },
            // ── master FX: delay / reverb / low-pass (underwater) ──
            #[cfg(not(target_arch = "wasm32"))]
            "audio_fx_delay" | "回声" | "ディレイ効果" | "딜레이" | "เสียงสะท้อน" | "افکت_تاخیر" | "صدى_تأخير" | "אפקט_עיכוב" | "تاخیر_ایفیکٹ" | "délai_audio" | "audio_verzögerung" | "звук_задержка" =>
            {
                let time = self.arg_num(&args, 0, 0.3)? as f32;
                let fb = self.arg_num(&args, 1, 0.3)? as f32;
                let mix = self.arg_num(&args, 2, 0.3)? as f32;
                if let Some(a) = &self.audio {
                    a.fx_delay(time, fb, mix);
                }
                return Ok(Value::Unit);
            },
            #[cfg(not(target_arch = "wasm32"))]
            "audio_fx_reverb" | "混响" | "リバーブ" | "리버브" | "เสียงก้อง" | "افکت_پژواک" | "صدى_ارتداد" | "אפקט_הדהוד" | "بازگشت_آواز_ایفیکٹ" | "réverbération_audio" | "audio_nachhall" | "звук_реверберация" =>
            {
                let mix = self.arg_num(&args, 0, 0.3)? as f32;
                if let Some(a) = &self.audio {
                    a.fx_reverb(mix);
                }
                return Ok(Value::Unit);
            },
            #[cfg(not(target_arch = "wasm32"))]
            "audio_fx_lowpass" | "低通滤波" | "ローパス" | "저역통과" | "กรองความถี่ต่ำ" | "فیلتر_پایین‌گذر" | "مرشح_تمرير_منخفض" | "מסנן_תדר_נמוך" | "لو_پاس_فلٹر" | "passe_bas_audio" | "audio_tiefpass" | "звук_фнч" =>
            {
                let cutoff = self.arg_num(&args, 0, 1.0)? as f32;
                if let Some(a) = &self.audio {
                    a.fx_lowpass(cutoff);
                }
                return Ok(Value::Unit);
            },

            // ══════════════════════════════════════════════════════════════════
            // PHYSICS BUILTINS  (crates/ling-physics) — soft bodies, rigid+angular,
            // and a fast 2-D water/oil liquid sim mappable onto 3-D surfaces.
            // ══════════════════════════════════════════════════════════════════

            // ── soft bodies (deformable bouncy balls) ──
            #[cfg(not(target_arch = "wasm32"))]
            "soft_ball" | "软球" | "ソフトボール" | "소프트볼" | "ลูกบอลนุ่ม" | "توپ_نرم" | "كرة_ناعمة" | "כדור_רך" | "نرم_گیند" | "balle_molle" | "weicher_ball" | "мягкий_шар" =>
            {
                let x = self.arg_num(&args, 0, 0.)? as f32;
                let y = self.arg_num(&args, 1, 0.)? as f32;
                let z = self.arg_num(&args, 2, 0.)? as f32;
                let r = self.arg_num(&args, 3, 1.0)? as f32;
                let b = ling_physics::soft::SoftBody::sphere(
                    ling_physics::Vec3::new(x, y, z),
                    r,
                    8,
                    12,
                    1.0,
                );
                let id = self.soft_bodies.len();
                self.soft_bodies.push(b);
                return Ok(Value::Number(id as f64));
            },
            #[cfg(not(target_arch = "wasm32"))]
            "soft_step" | "软体步进" | "ソフト更新" | "소프트스텝" | "ก้าวนุ่ม" | "گام_نرم_جسم" | "خطوة_ناعمة" | "צעד_רך" | "نرم_قدم" | "pas_mou" | "weicher_schritt" | "мягкий_шаг" =>
            {
                let id = self.arg_num(&args, 0, 0.)? as usize;
                let dt = self.arg_num(&args, 1, 0.016)? as f32;
                let gy = self.arg_num(&args, 2, 15.0)? as f32;
                if let Some(b) = self.soft_bodies.get_mut(id) {
                    b.integrate(dt, ling_physics::Vec3::new(0.0, gy, 0.0), 4);
                }
                return Ok(Value::Unit);
            },
            #[cfg(not(target_arch = "wasm32"))]
            "soft_bounce" | "软体落地" | "ソフト着地" | "소프트바운스" | "เด้งนุ่ม" | "جهش_نرم" | "ارتداد_ناعم" | "קפיצה_רכה" | "نرم_اچھال" | "rebond_mou" | "weicher_abprall" | "мягкий_отскок" =>
            {
                let id = self.arg_num(&args, 0, 0.)? as usize;
                let fy = self.arg_num(&args, 1, 0.)? as f32;
                let rest = self.arg_num(&args, 2, 0.5)? as f32;
                if let Some(b) = self.soft_bodies.get_mut(id) {
                    b.floor_collision(fy, rest);
                }
                return Ok(Value::Unit);
            },
            #[cfg(not(target_arch = "wasm32"))]
            "soft_contain" | "软体边界" | "ソフト箱" | "소프트경계" | "กล่องนุ่ม" | "محفظه_نرم" | "احتواء_ناعم" | "הכלה_רכה" | "نرم_احاطہ" | "contenir_mou" | "weiche_eindämmung" | "мягкое_сдерживание" =>
            {
                let id = self.arg_num(&args, 0, 0.)? as usize;
                let nx = self.arg_num(&args, 1, -5.)? as f32;
                let ny = self.arg_num(&args, 2, -5.)? as f32;
                let nz = self.arg_num(&args, 3, -5.)? as f32;
                let mx = self.arg_num(&args, 4, 5.)? as f32;
                let my = self.arg_num(&args, 5, 5.)? as f32;
                let mz = self.arg_num(&args, 6, 5.)? as f32;
                let rest = self.arg_num(&args, 7, 0.6)? as f32;
                if let Some(b) = self.soft_bodies.get_mut(id) {
                    b.contain(
                        ling_physics::Vec3::new(nx, ny, nz),
                        ling_physics::Vec3::new(mx, my, mz),
                        rest,
                    );
                }
                return Ok(Value::Unit);
            },
            #[cfg(not(target_arch = "wasm32"))]
            "soft_kick" | "软体踢" | "ソフト衝撃" | "소프트킥" | "เตะนุ่ม" | "ضربه_نرم" | "ركلة_ناعمة" | "בעיטה_רכה" | "نرم_ٹھوکر" | "coup_mou" | "weicher_stoß" | "мягкий_удар" =>
            {
                let id = self.arg_num(&args, 0, 0.)? as usize;
                let dx = self.arg_num(&args, 1, 0.)? as f32;
                let dy = self.arg_num(&args, 2, 0.)? as f32;
                let dz = self.arg_num(&args, 3, 0.)? as f32;
                let s = self.arg_num(&args, 4, 0.1)? as f32;
                if let Some(b) = self.soft_bodies.get_mut(id) {
                    b.kick(ling_physics::Vec3::new(dx, dy, dz), s);
                }
                return Ok(Value::Unit);
            },
            // soft_spin(id, ax, ay, az, rate) — add angular velocity about the axis
            // through the centroid (rate = rad/step; ≈ surface_speed / radius to roll)
            #[cfg(not(target_arch = "wasm32"))]
            "soft_spin" | "软体自旋" | "ソフト回転" | "소프트회전" | "หมุนนุ่ม" | "چرخش_نرم" | "دوران_ناعم" | "סיבוב_רך" | "نرم_گھماؤ" | "rotation_molle" | "weicher_spin" | "мягкое_вращение" =>
            {
                let id = self.arg_num(&args, 0, 0.)? as usize;
                let ax = self.arg_num(&args, 1, 0.)? as f32;
                let ay = self.arg_num(&args, 2, 0.)? as f32;
                let az = self.arg_num(&args, 3, 0.)? as f32;
                let rate = self.arg_num(&args, 4, 0.1)? as f32;
                if let Some(b) = self.soft_bodies.get_mut(id) {
                    b.spin(ling_physics::Vec3::new(ax, ay, az), rate);
                }
                return Ok(Value::Unit);
            },
            #[cfg(not(target_arch = "wasm32"))]
            "soft_deform" | "形变量" | "変形量" | "변형량" | "ความบิดเบี้ยว" | "تغییرشکل_نرم" | "تشوه_ناعم" | "עיוות_רך" | "نرم_بگاڑ" | "déformer_mou" | "weiches_verformen" | "мягкая_деформация" =>
            {
                let id = self.arg_num(&args, 0, 0.)? as usize;
                let d = self
                    .soft_bodies
                    .get(id)
                    .map(|b| b.deformation())
                    .unwrap_or(0.0);
                return Ok(Value::Number(d as f64));
            },
            // soft_angular_speed(id) -> magnitude of the body's angular velocity
            // (how fast it is tumbling/rolling), derived from its node velocities.
            #[cfg(not(target_arch = "wasm32"))]
            "soft_angular_speed"
            | "软体角速"
            | "ソフト角速度"
            | "소프트각속도"
            | "ความเร็วเชิงมุมนุ่ม" | "سرعت_زاویه‌ای_نرم" | "سرعة_زاوية_ناعمة" | "מהירות_זוויתית_רכה" | "نرم_زاویائی_رفتار" | "vitesse_angulaire_molle" | "weiche_winkelgeschwindigkeit" | "мягкая_угловая_скорость" => {
                let id = self.arg_num(&args, 0, 0.)? as usize;
                let w = self
                    .soft_bodies
                    .get(id)
                    .map(|b| b.angular_speed())
                    .unwrap_or(0.0);
                return Ok(Value::Number(w as f64));
            },
            #[cfg(not(target_arch = "wasm32"))]
            "soft_centroid" | "软体质心" | "ソフト重心" | "소프트중심" | "จุดศูนย์กลางนุ่ม" | "مرکز_جرم_نرم" | "مركز_ثقل_ناعم" | "מרכז_כובד_רך" | "نرم_مرکز_ثقل" | "centroïde_mou" | "weicher_schwerpunkt" | "мягкий_центроид" =>
            {
                let id = self.arg_num(&args, 0, 0.)? as usize;
                let c = self
                    .soft_bodies
                    .get(id)
                    .map(|b| b.centroid())
                    .unwrap_or(ling_physics::Vec3::ZERO);
                return Ok(Value::List(Rc::new(vec![
                    Value::Number(c.x as f64),
                    Value::Number(c.y as f64),
                    Value::Number(c.z as f64),
                ])));
            },
            // soft_nodes(id) -> flat [x,y,z, x,y,z, …] for rendering the deformed mesh
            #[cfg(not(target_arch = "wasm32"))]
            "soft_nodes" | "软体节点" | "ソフト節点" | "소프트노드" | "จุดนุ่ม" | "گره‌های_نرم" | "عقد_ناعمة" | "צמתי_רך" | "نرم_نوڈز" | "nœuds_mous" | "weiche_knoten" | "мягкие_узлы" =>
            {
                let id = self.arg_num(&args, 0, 0.)? as usize;
                let mut out = Vec::new();
                if let Some(b) = self.soft_bodies.get(id) {
                    for n in &b.nodes {
                        out.push(Value::Number(n.pos.x as f64));
                        out.push(Value::Number(n.pos.y as f64));
                        out.push(Value::Number(n.pos.z as f64));
                    }
                }
                return Ok(Value::List(Rc::new(out)));
            },

            // ── rigid bodies with angular dynamics ──
            #[cfg(not(target_arch = "wasm32"))]
            "rb_add" | "刚体添加" | "剛体追加" | "강체추가" | "เพิ่มวัตถุแข็ง" | "افزودن_جسم_صلب" | "أضف_جسما_صلبا" | "הוסף_גוף_קשיח" | "سخت_جسم_شامل_کرو" | "ajouter_corps_rigide" | "starrkörper_hinzufügen" | "добавить_твёрдое_тело" =>
            {
                let x = self.arg_num(&args, 0, 0.)? as f32;
                let y = self.arg_num(&args, 1, 0.)? as f32;
                let z = self.arg_num(&args, 2, 0.)? as f32;
                let mass = self.arg_num(&args, 3, 1.0)? as f32;
                let mut b =
                    ling_physics::rigid::RigidBody::new(ling_physics::Vec3::new(x, y, z), mass);
                b.restitution = 0.6;
                return Ok(Value::Number(self.rigid_world.add(b) as f64));
            },
            #[cfg(not(target_arch = "wasm32"))]
            "rb_torque" | "扭矩" | "トルク" | "토크" | "แรงบิด" | "گشتاور" | "عزم_دوران" | "מומנט" | "ٹارک" | "couple_corps_rigide" | "starrkörper_drehmoment" | "крутящий_момент_твёрдого_тела" => {
                let i = self.arg_num(&args, 0, 0.)? as usize;
                let tx = self.arg_num(&args, 1, 0.)? as f32;
                let ty = self.arg_num(&args, 2, 0.)? as f32;
                let tz = self.arg_num(&args, 3, 0.)? as f32;
                if let Some(b) = self.rigid_world.bodies.get_mut(i) {
                    b.apply_torque(ling_physics::Vec3::new(tx, ty, tz));
                }
                return Ok(Value::Unit);
            },
            #[cfg(not(target_arch = "wasm32"))]
            "rb_spin" | "自旋" | "スピン" | "스핀" | "หมุน" | "چرخش_جسم_صلب" | "دوران_جسم_صلب" | "סיבוב_גוף_קשיח" | "سخت_جسم_گھماؤ" | "spin_corps_rigide" | "starrkörper_spin" | "вращение_твёрдого_тела" => {
                let i = self.arg_num(&args, 0, 0.)? as usize;
                let wx = self.arg_num(&args, 1, 0.)? as f32;
                let wy = self.arg_num(&args, 2, 0.)? as f32;
                let wz = self.arg_num(&args, 3, 0.)? as f32;
                if let Some(b) = self.rigid_world.bodies.get_mut(i) {
                    b.apply_spin(ling_physics::Vec3::new(wx, wy, wz));
                }
                return Ok(Value::Unit);
            },
            #[cfg(not(target_arch = "wasm32"))]
            "rb_impulse" | "刚体冲量" | "剛体インパルス" | "강체충격" | "แรงดลแข็ง" | "ضربه_جسم_صلب" | "دفعة_جسم_صلب" | "דחף_גוף_קשיח" | "سخت_جسم_دھکا" | "impulsion_corps_rigide" | "starrkörper_impuls" | "импульс_твёрдого_тела" =>
            {
                let i = self.arg_num(&args, 0, 0.)? as usize;
                let ix = self.arg_num(&args, 1, 0.)? as f32;
                let iy = self.arg_num(&args, 2, 0.)? as f32;
                let iz = self.arg_num(&args, 3, 0.)? as f32;
                if let Some(b) = self.rigid_world.bodies.get_mut(i) {
                    b.apply_impulse(ling_physics::Vec3::new(ix, iy, iz));
                }
                return Ok(Value::Unit);
            },
            #[cfg(not(target_arch = "wasm32"))]
            "rb_floor" | "刚体落地" | "剛体着地" | "강체바닥" | "พื้นแข็ง" | "کف_جسم_صلب" | "أرضية_جسم_صلب" | "רצפת_גוף_קשיח" | "سخت_جسم_فرش" | "sol_corps_rigide" | "starrkörper_boden" | "пол_твёрдого_тела" =>
            {
                let i = self.arg_num(&args, 0, 0.)? as usize;
                let fy = self.arg_num(&args, 1, 0.)? as f32;
                let rest = self.arg_num(&args, 2, 0.6)? as f32;
                let fric = self.arg_num(&args, 3, 0.6)? as f32;
                if let Some(b) = self.rigid_world.bodies.get_mut(i) {
                    b.bounce_floor(fy, rest, fric);
                }
                return Ok(Value::Unit);
            },
            #[cfg(not(target_arch = "wasm32"))]
            "rb_gravity" | "刚体重力" | "剛体重力" | "강체중력" | "แรงโน้มถ่วงแข็ง" | "گرانش_جسم_صلب" | "جاذبية_جسم_صلب" | "כבידת_גוף_קשיח" | "سخت_جسم_کشش_ثقل" | "gravité_corps_rigide" | "starrkörper_schwerkraft" | "гравитация_твёрдого_тела" =>
            {
                let gx = self.arg_num(&args, 0, 0.)? as f32;
                let gy = self.arg_num(&args, 1, 9.81)? as f32;
                let gz = self.arg_num(&args, 2, 0.)? as f32;
                self.rigid_world.gravity = ling_physics::Vec3::new(gx, gy, gz);
                return Ok(Value::Unit);
            },
            #[cfg(not(target_arch = "wasm32"))]
            "rb_step" | "刚体步进" | "剛体更新" | "강체스텝" | "ก้าวแข็ง" | "گام_جسم_صلب" | "خطوة_جسم_صلب" | "צעד_גוף_קשיח" | "سخت_جسم_قدم" | "pas_corps_rigide" | "starrkörper_schritt" | "шаг_твёрдого_тела" =>
            {
                let dt = self.arg_num(&args, 0, 0.016)? as f32;
                self.rigid_world.step(dt);
                return Ok(Value::Unit);
            },
            #[cfg(not(target_arch = "wasm32"))]
            "rb_pos" | "刚体位置" | "剛体位置" | "강체위치" | "ตำแหน่งแข็ง" | "موقعیت_جسم_صلب" | "موضع_جسم_صلب" | "מיקום_גוף_קשיח" | "سخت_جسم_مقام" | "position_corps_rigide" | "starrkörper_position" | "позиция_твёрдого_тела" =>
            {
                let i = self.arg_num(&args, 0, 0.)? as usize;
                let p = self
                    .rigid_world
                    .bodies
                    .get(i)
                    .map(|b| b.pos)
                    .unwrap_or(ling_physics::Vec3::ZERO);
                return Ok(Value::List(Rc::new(vec![
                    Value::Number(p.x as f64),
                    Value::Number(p.y as f64),
                    Value::Number(p.z as f64),
                ])));
            },
            #[cfg(not(target_arch = "wasm32"))]
            "rb_rot" | "刚体旋转" | "剛体回転" | "강체회전" | "การหมุนแข็ง" | "چرخش_وضعية_جسم_صلب" | "دوران_وضعية_جسم_صلب" | "סיבוב_זווית_גוף_קשיח" | "سخت_جسم_گردش" | "rotation_corps_rigide" | "starrkörper_rotation" | "поворот_твёрдого_тела" =>
            {
                let i = self.arg_num(&args, 0, 0.)? as usize;
                let q = self
                    .rigid_world
                    .bodies
                    .get(i)
                    .map(|b| b.orientation)
                    .unwrap_or(ling_physics::Quat::IDENTITY);
                return Ok(Value::List(Rc::new(vec![
                    Value::Number(q.x as f64),
                    Value::Number(q.y as f64),
                    Value::Number(q.z as f64),
                    Value::Number(q.w as f64),
                ])));
            },

            // ── native-res mesh (.lmesh): load once, draw fast (unlit, per-tri colour) ──
            #[cfg(not(target_arch = "wasm32"))]
            "mesh_load" | "โหลดเมช" | "载入网格" | "メッシュ読込" | "메시로드" | "بارگذاری_مش" | "حمّل_شبكة" | "טען_מש" | "میش_لوڈ" =>
            {
                let path = self.arg_str(&args, 0, "");
                let resolved = if std::path::Path::new(&path).exists() {
                    path.clone()
                } else if let Some(d) = &self.source_dir {
                    d.join(&path).to_string_lossy().into_owned()
                } else {
                    path.clone()
                };
                let bytes = match std::fs::read(&resolved) {
                    Ok(b) => b,
                    Err(e) => {
                        eprintln!("mesh_load failed ({path}): {e}");
                        return Ok(Value::Number(-1.0));
                    },
                };
                if bytes.len() < 16 || &bytes[0..4] != b"LMSH" {
                    eprintln!("mesh_load: bad header ({path})");
                    return Ok(Value::Number(-1.0));
                }
                let rd4 =
                    |o: usize| -> [u8; 4] { [bytes[o], bytes[o + 1], bytes[o + 2], bytes[o + 3]] };
                let height = f32::from_le_bytes(rd4(8));
                let ntri = u32::from_le_bytes(rd4(12)) as usize;
                let need = 16usize.saturating_add(ntri.saturating_mul(9 * 4 + 3));
                if bytes.len() < need {
                    eprintln!("mesh_load: truncated ({path})");
                    return Ok(Value::Number(-1.0));
                }
                let mut pos = Vec::with_capacity(ntri * 3);
                let mut col = Vec::with_capacity(ntri);
                let mut off = 16usize;
                for _ in 0..ntri {
                    for _k in 0..3 {
                        let x = f32::from_le_bytes(rd4(off));
                        let y = f32::from_le_bytes(rd4(off + 4));
                        let z = f32::from_le_bytes(rd4(off + 8));
                        off += 12;
                        pos.push([x, y, z]);
                    }
                    col.push([bytes[off], bytes[off + 1], bytes[off + 2]]);
                    off += 3;
                }
                eprintln!("mesh_load: {} ({} tris, h={:.2})", path, ntri, height);
                let id = self.meshes.len();
                self.meshes
                    .push(crate::gfx::shapes::ColorMesh { pos, col, height });
                return Ok(Value::Number(id as f64));
            },
            #[cfg(target_arch = "wasm32")]
            "mesh_load" | "โหลดเมช" | "载入网格" | "メッシュ読込" | "메시로드" | "بارگذاری_مش" | "حمّل_شبكة" | "טען_רשת" | "میش_لوڈ" =>
            {
                // Native .lmesh loading is file-system based and not wired for wasm yet.
                // Return an invalid handle so scripts can choose a fallback path.
                return Ok(Value::Number(-1.0));
            },
            #[cfg(not(target_arch = "wasm32"))]
            "mesh_draw" | "วาดเมชสี" | "绘制网格" | "メッシュ描画" | "메시그리기" | "رسم_مش_رنگی" | "ارسم_شبكة_ملونة" | "צייר_רשת_צבעונית" | "رنگین_میش_کھینچو" =>
            {
                // ('วาดเมช' is taken by draw_mesh — use a distinct Thai alias)
                let id = self.arg_num(&args, 0, 0.)? as usize;
                let cx = self.arg_num(&args, 1, 0.)? as f32;
                let cy = self.arg_num(&args, 2, 0.)? as f32;
                let cz = self.arg_num(&args, 3, 0.)? as f32;
                let sc = self.arg_num(&args, 4, 1.)? as f32;
                let yaw = self.arg_num(&args, 5, 0.)? as f32;
                let sway = self.arg_num(&args, 6, 0.)? as f32;
                let arm = self.arg_num(&args, 7, 0.)? as f32;
                let lean = self.arg_num(&args, 8, 0.)? as f32;
                let leg = self.arg_num(&args, 9, 0.)? as f32;
                let tuck = self.arg_num(&args, 10, 0.)? as f32;
                if id < self.meshes.len() {
                    let m = &self.meshes[id];
                    let mut gfx = self.gfx.borrow_mut();
                    gfx.draw_color_mesh(m, cx, cy, cz, sc, yaw, sway, arm, lean, leg, tuck);
                }
                return Ok(Value::Unit);
            },
            #[cfg(target_arch = "wasm32")]
            "mesh_draw" | "วาดเมชสี" | "绘制网格" | "メッシュ描画" | "메시그리기" | "رسم_مش_رنگی" | "ارسم_شبكة_ملونة" | "צייר_רשת_צבעונית" | "رنگین_میش_کھینچو" =>
            {
                return Ok(Value::Unit);
            },

            // ── liquid sim (water + oil, immiscible) ──
            "liquid_new" | "新建液体" | "液体新規" | "액체생성" | "สร้างของเหลว" | "مایع_جدید" | "سائل_جديد" | "נוזל_חדש" | "نیا_مائع" | "nouveau_liquide" | "neue_flüssigkeit" | "новая_жидкость" =>
            {
                let w = self.arg_num(&args, 0, 64.)? as usize;
                let h = self.arg_num(&args, 1, 64.)? as usize;
                let id = self.liquids.len();
                self.liquids
                    .push(ling_physics::liquid::LiquidGrid::new(w, h));
                return Ok(Value::Number(id as f64));
            },
            "liquid_set_colors" | "液体颜色" | "液体配色" | "액체색상" | "สีของเหลว" | "تنظیم_رنگ_مایع" | "عيّن_ألوان_السائل" | "קבע_צבעי_נוזל" | "مائع_رنگ_مقرر_کرو" =>
            {
                let id = self.arg_num(&args, 0, 0.)? as usize;
                let wr = self.arg_num(&args, 1, 40.)? as f32;
                let wg = self.arg_num(&args, 2, 110.)? as f32;
                let wb = self.arg_num(&args, 3, 235.)? as f32;
                let or_ = self.arg_num(&args, 4, 240.)? as f32;
                let og = self.arg_num(&args, 5, 175.)? as f32;
                let ob = self.arg_num(&args, 6, 45.)? as f32;
                if let Some(g) = self.liquids.get_mut(id) {
                    g.set_colors(wr, wg, wb, or_, og, ob);
                }
                return Ok(Value::Unit);
            },
            "liquid_splat" | "液体注入" | "液体追加" | "액체분사" | "หยดของเหลว" | "پاشش_مایع" | "بقعة_سائل" | "התזת_נוזל" | "مائع_چھینٹا" | "éclaboussure_liquide" | "flüssigkeit_spritzer" | "брызги_жидкости" =>
            {
                let id = self.arg_num(&args, 0, 0.)? as usize;
                let x = self.arg_num(&args, 1, 0.)? as f32;
                let y = self.arg_num(&args, 2, 0.)? as f32;
                let kind = self.arg_num(&args, 3, 0.)? as i32;
                let amt = self.arg_num(&args, 4, 1.0)? as f32;
                let rad = self.arg_num(&args, 5, 4.0)? as f32;
                if let Some(g) = self.liquids.get_mut(id) {
                    g.splat(x, y, kind, amt, rad);
                }
                return Ok(Value::Unit);
            },
            "liquid_gravity" | "液体重力" | "液体重力ベクトル" | "액체중력" | "แรงโน้มถ่วงเหลว" | "گرانش_مایع" | "جاذبية_السائل" | "כבידת_נוזל" | "مائع_کشش_ثقل" | "gravité_liquide" | "flüssigkeit_schwerkraft" | "гравитация_жидкости" =>
            {
                let id = self.arg_num(&args, 0, 0.)? as usize;
                let gx = self.arg_num(&args, 1, 0.)? as f32;
                let gy = self.arg_num(&args, 2, 60.)? as f32;
                if let Some(g) = self.liquids.get_mut(id) {
                    g.set_gravity(gx, gy);
                }
                return Ok(Value::Unit);
            },
            "liquid_step" | "液体步进" | "液体更新" | "액체스텝" | "ก้าวของเหลว" | "گام_مایع" | "خطوة_السائل" | "צעד_נוזל" | "مائع_قدم" | "pas_liquide" | "flüssigkeit_schritt" | "шаг_жидкости" =>
            {
                let id = self.arg_num(&args, 0, 0.)? as usize;
                let dt = self.arg_num(&args, 1, 0.016)? as f32;
                if let Some(g) = self.liquids.get_mut(id) {
                    g.step(dt);
                }
                return Ok(Value::Unit);
            },
            // liquid_step_all(dt) — advance EVERY liquid grid one tick, in parallel
            // across instances (rayon). Independent grids share no state, so this is
            // an embarrassingly-parallel batch: a scene with many liquid surfaces
            // steps in one call that scales across cores instead of N serial
            // `liquid_step` calls.
            "liquid_step_all"
            | "液体全步进"
            | "液体全更新"
            | "전체액체스텝"
            | "ก้าวของเหลวทั้งหมด" | "گام_همه_مایعات" | "خطوة_كل_السوائل" | "צעד_כל_הנוזלים" | "تمام_مائع_قدم" => {
                let dt = self.arg_num(&args, 0, 0.016)? as f32;
                ling_physics::liquid::step_all(&mut self.liquids, dt);
                return Ok(Value::Unit);
            },
            // liquid_rainbow(id, on) — colour the fluid as a flowing ROYGBIV marble
            "liquid_rainbow" | "液体彩虹" | "液体虹" | "액체무지개" | "ของเหลวสายรุ้ง" | "مایع_رنگین‌کمان" | "سائل_قوس_قزح" | "נוזל_קשת" | "قوس_قزح_مائع" | "arc_en_ciel_liquide" | "flüssigkeit_regenbogen" | "радуга_жидкости" =>
            {
                let id = self.arg_num(&args, 0, 0.)? as usize;
                let on = self.arg_num(&args, 1, 1.0)? > 0.5;
                if let Some(g) = self.liquids.get_mut(id) {
                    g.rainbow = on;
                }
                return Ok(Value::Unit);
            },
            // liquid_mix(id) -> 0 (oil/water separated) .. 1 (fully intermixed)
            "liquid_mix" | "液体混合" | "液体混合度" | "액체혼합" | "การผสมของเหลว" | "ترکیب_مایع" | "مزج_سائل" | "ערבוב_נוזל" | "مائع_ملاؤ" | "mélanger_liquide" | "flüssigkeit_mischen" | "смешать_жидкость" =>
            {
                let id = self.arg_num(&args, 0, 0.)? as usize;
                let m = self.liquids.get(id).map(|g| g.mix_amount()).unwrap_or(0.0);
                return Ok(Value::Number(m as f64));
            },
            // liquid_draw(id, sx, sy, scale) — fast flat 2-D blit of the colour field
            #[cfg(not(target_arch = "wasm32"))]
            "liquid_draw" | "绘制液体" | "液体描画" | "액체그리기" | "วาดของเหลว" | "رسم_مایع" | "ارسم_سائلا" | "צייר_נוזל" | "مائع_کھینچو" | "dessiner_liquide" | "flüssigkeit_zeichnen" | "рисовать_жидкость" =>
            {
                let id = self.arg_num(&args, 0, 0.)? as usize;
                let sx = self.arg_num(&args, 1, 0.)? as i32;
                let sy = self.arg_num(&args, 2, 0.)? as i32;
                let scale = (self.arg_num(&args, 3, 4.)? as i32).max(1);
                if id < self.liquids.len() {
                    let (gw, gh) = {
                        let g = &self.liquids[id];
                        (g.w, g.h)
                    };
                    let mut gfx = self.gfx.borrow_mut();
                    let (w, h) = (gfx.width as i32, gfx.height as i32);
                    let g = &self.liquids[id];
                    for cy in 0..gh {
                        for cx in 0..gw {
                            let col = g.sample_rgb(cx, cy);
                            let bx = sx + cx as i32 * scale;
                            let by = sy + cy as i32 * scale;
                            for dy in 0..scale {
                                for dx in 0..scale {
                                    let px = bx + dx;
                                    let py = by + dy;
                                    if px >= 0 && py >= 0 && px < w && py < h {
                                        gfx.buffer[(py * w + px) as usize] = col;
                                    }
                                }
                            }
                        }
                    }
                }
                return Ok(Value::Unit);
            },
            // liquid_draw_surface(id, kind, cx,cy,cz, radius, height)
            //   kind: 0 plane · 1 sphere · 2 cylinder · 3 cone · 4 dome
            "liquid_draw_surface" | "液体贴面" | "液体曲面" | "액체곡면" | "ของเหลวบนพื้นผิว" | "رسم_سطح_مایع" | "ارسم_سطح_السائل" | "צייר_משטח_נוזל" | "مائع_سطح_کھینچو" | "dessiner_surface_liquide" | "flüssigkeit_oberfläche_zeichnen" | "рисовать_поверхность_жидкости" =>
            {
                #[cfg(not(target_arch = "wasm32"))]
                {
                    let id = self.arg_num(&args, 0, 0.)? as usize;
                    let kind = self.arg_num(&args, 1, 1.)? as i32;
                    let cx = self.arg_num(&args, 2, 0.)? as f32;
                    let cy = self.arg_num(&args, 3, 0.)? as f32;
                    let cz = self.arg_num(&args, 4, 0.)? as f32;
                    let radius = self.arg_num(&args, 5, 2.0)? as f32;
                    let height = self.arg_num(&args, 6, 3.0)? as f32;
                    if id < self.liquids.len() {
                        let (gw, gh) = {
                            let g = &self.liquids[id];
                            (g.w, g.h)
                        };
                        let mut gfx = self.gfx.borrow_mut();
                        let (w, h, add) = (gfx.width, gfx.height, gfx.blend == 1);
                        let cam = gfx.camera.clone();
                        let near = -cam.zdist + 0.05;
                        let g = &self.liquids[id];
                        let tau = std::f32::consts::TAU;
                        let pi = std::f32::consts::PI;
                        // surface point for a (u,v) in [0,1] on the chosen primitive
                        let sp = |u: f32, v: f32| -> [f32; 3] {
                            if kind == 0 {
                                [
                                    cx + (u - 0.5) * 2.0 * radius,
                                    cy,
                                    cz + (v - 0.5) * 2.0 * radius,
                                ]
                            } else if kind == 2 {
                                let th = u * tau;
                                [
                                    cx + th.cos() * radius,
                                    cy + (v - 0.5) * height,
                                    cz + th.sin() * radius,
                                ]
                            } else if kind == 3 {
                                let th = u * tau;
                                let rr = radius * (1.0 - v);
                                [
                                    cx + th.cos() * rr,
                                    cy + (v - 0.5) * height,
                                    cz + th.sin() * rr,
                                ]
                            } else if kind == 4 {
                                let th = u * tau;
                                let ph = v * pi * 0.5;
                                [
                                    cx + ph.sin() * th.cos() * radius,
                                    cy - ph.cos() * radius,
                                    cz + ph.sin() * th.sin() * radius,
                                ]
                            } else {
                                let th = u * tau;
                                let ph = v * pi;
                                [
                                    cx + ph.sin() * th.cos() * radius,
                                    cy + ph.cos() * radius,
                                    cz + ph.sin() * th.sin() * radius,
                                ]
                            }
                        };
                        let nrm = |u: f32, v: f32| -> [f32; 3] {
                            if kind == 0 {
                                [0.0, -1.0, 0.0]
                            } else if kind == 2 {
                                let th = u * tau;
                                [th.cos(), 0.0, th.sin()]
                            } else if kind == 3 {
                                let th = u * tau;
                                let s = (radius / height.max(0.01)).atan();
                                [th.cos() * s.cos(), s.sin(), th.sin() * s.cos()]
                            } else if kind == 4 {
                                let th = u * tau;
                                let ph = v * pi * 0.5;
                                [ph.sin() * th.cos(), -ph.cos(), ph.sin() * th.sin()]
                            } else {
                                let th = u * tau;
                                let ph = v * pi;
                                [ph.sin() * th.cos(), ph.cos(), ph.sin() * th.sin()]
                            }
                        };
                        let gwf = gw as f32;
                        let ghf = gh as f32;
                        let mut cyc = 0usize;
                        while cyc < gh {
                            let mut cxc = 0usize;
                            while cxc < gw {
                                // cull by the cell centre's outward normal
                                let uc = (cxc as f32 + 0.5) / gwf;
                                let vc = (cyc as f32 + 0.5) / ghf;
                                let c = sp(uc, vc);
                                let n = nrm(uc, vc);
                                let dc = cam.depth(c[0], c[1], c[2]);
                                if dc > near {
                                    let cull = kind != 0
                                        && cam.depth(
                                            c[0] + n[0] * 0.06,
                                            c[1] + n[1] * 0.06,
                                            c[2] + n[2] * 0.06,
                                        ) > dc;
                                    if !cull {
                                        // project the 4 cell corners → a filled AA vector quad
                                        let u0 = cxc as f32 / gwf;
                                        let u1 = (cxc + 1) as f32 / gwf;
                                        let v0 = cyc as f32 / ghf;
                                        let v1 = (cyc + 1) as f32 / ghf;
                                        let q = [sp(u0, v0), sp(u1, v0), sp(u1, v1), sp(u0, v1)];
                                        let mut poly: Vec<[f32; 2]> = Vec::with_capacity(5);
                                        let mut ok = true;
                                        for p in &q {
                                            if cam.depth(p[0], p[1], p[2]) <= near {
                                                ok = false;
                                                break;
                                            }
                                            let (sx, sy, _) = cam.project(p[0], p[1], p[2]);
                                            poly.push([sx, sy]);
                                        }
                                        if ok {
                                            let p0 = poly[0];
                                            poly.push(p0);
                                            let col = g.sample_rgb(cxc, cyc);
                                            crate::gfx::raster::fill_contours_aa(
                                                &mut gfx.buffer,
                                                w,
                                                h,
                                                col,
                                                add,
                                                std::slice::from_ref(&poly),
                                            );
                                        }
                                    }
                                }
                                cxc += 1;
                            }
                            cyc += 1;
                        }
                    }
                }
                #[cfg(target_arch = "wasm32")]
                {
                    // WASM: liquid_draw_surface is a no-op for now (would need WebGL shader)
                    // The liquid simulation still runs, just not rendered to 3D surfaces
                }
                return Ok(Value::Unit);
            },
            // sparkle(x, y, w, h, count [, t]) — scatter twinkling vector star-sparkles
            // in a rect (snowglobe effect) in the current colour + blend mode.
            #[cfg(not(target_arch = "wasm32"))]
            "sparkle" | "闪光" | "きらめき" | "반짝임" | "ประกาย" | "درخشش" | "بريق" | "נצנוץ" | "چمک" | "scintillement" | "funkeln" | "искриться" => {
                let x = self.arg_num(&args, 0, 0.)? as f32;
                let y = self.arg_num(&args, 1, 0.)? as f32;
                let ww = self.arg_num(&args, 2, 200.)? as f32;
                let hh = self.arg_num(&args, 3, 200.)? as f32;
                let count = self.arg_num(&args, 4, 40.)? as i32;
                let t = self.arg_num(&args, 5, 0.)? as f32;
                let mut gfx = self.gfx.borrow_mut();
                let (w, h, add, color) = (gfx.width, gfx.height, gfx.blend == 1, gfx.color);
                let (cr, cg, cb) = (
                    (color >> 16 & 0xFF) as f32,
                    (color >> 8 & 0xFF) as f32,
                    (color & 0xFF) as f32,
                );
                let mut n = 0i32;
                while n < count {
                    let hsh = (n as u32).wrapping_mul(2654435761).wrapping_add(0x9E3779B9);
                    let u = ((hsh >> 8) & 1023) as f32 / 1023.0;
                    let v = ((hsh >> 18) & 1023) as f32 / 1023.0;
                    let phase = (hsh & 255) as f32 / 255.0;
                    let tw = (t * 3.0 + phase * std::f32::consts::TAU + n as f32).sin() * 0.5 + 0.5;
                    let sz = 1.5 + tw * 5.0;
                    let px = x + u * ww;
                    let py = y + v * hh;
                    let b = tw * tw; // sharp twinkle
                    let col =
                        (((cr * b) as u32) << 16) | (((cg * b) as u32) << 8) | ((cb * b) as u32);
                    crate::gfx::raster::draw_line_aa(
                        &mut gfx.buffer,
                        w,
                        h,
                        col,
                        add,
                        px - sz,
                        py,
                        px + sz,
                        py,
                    );
                    crate::gfx::raster::draw_line_aa(
                        &mut gfx.buffer,
                        w,
                        h,
                        col,
                        add,
                        px,
                        py - sz,
                        px,
                        py + sz,
                    );
                    let d = sz * 0.55;
                    crate::gfx::raster::draw_line_aa(
                        &mut gfx.buffer,
                        w,
                        h,
                        col,
                        add,
                        px - d,
                        py - d,
                        px + d,
                        py + d,
                    );
                    crate::gfx::raster::draw_line_aa(
                        &mut gfx.buffer,
                        w,
                        h,
                        col,
                        add,
                        px - d,
                        py + d,
                        px + d,
                        py - d,
                    );
                    n += 1;
                }
                return Ok(Value::Unit);
            },

            // ══════════════════════════════════════════════════════════════════
            // DIALOG BUILTINS  (crates/ling-game/src/dialog.rs) — cinematic,
            // typed-out, colour-coded text boxes. Markup: {n}name{/} {p}place{/}
            // {i}item{/}, \n newline, || page break.
            // ══════════════════════════════════════════════════════════════════
            #[cfg(not(target_arch = "wasm32"))]
            "dialog_show" | "对话显示" | "会話表示" | "대화표시" | "แสดงบทสนทนา" | "نمایش_گفتگو" | "اعرض_الحوار" | "הצג_דיאלוג" | "مکالمہ_دکھاؤ" | "afficher_dialogue" | "dialog_anzeigen" | "показать_диалог" =>
            {
                let text = self.arg_str(&args, 0, "");
                let cps = self.arg_num(&args, 1, 32.0)? as f32;
                self.dialog = Some(ling_game::dialog::Dialog::new(&text, cps));
                return Ok(Value::Unit);
            },
            #[cfg(not(target_arch = "wasm32"))]
            "dialog_step" | "对话步进" | "会話更新" | "대화스텝" | "ก้าวบทสนทนา" | "گام_گفتگو" | "خطوة_الحوار" | "צעד_דיאלוג" | "مکالمہ_قدم" | "pas_dialogue" | "dialog_schritt" | "шаг_диалога" =>
            {
                let dt = self.arg_num(&args, 0, 0.016)? as f32;
                if let Some(d) = self.dialog.as_mut() {
                    d.update(dt);
                }
                return Ok(Value::Unit);
            },
            #[cfg(not(target_arch = "wasm32"))]
            "dialog_advance" | "对话推进" | "会話送り" | "대화진행" | "เลื่อนบทสนทนา" | "پیشروی_گفتگو" | "تقدّم_الحوار" | "קדם_דיאלוג" | "مکالمہ_آگے_بڑھاؤ" | "avancer_dialogue" | "dialog_weiter" | "продолжить_диалог" =>
            {
                if let Some(d) = self.dialog.as_mut() {
                    d.advance();
                }
                return Ok(Value::Unit);
            },
            #[cfg(not(target_arch = "wasm32"))]
            "dialog_active" | "对话激活" | "会話中" | "대화중" | "บทสนทนาทำงาน" | "گفتگو_فعال" | "الحوار_نشط" | "דיאלוג_פעיל" | "مکالمہ_فعال" | "dialogue_actif" | "dialog_aktiv" | "диалог_активен" =>
            {
                let a = self
                    .dialog
                    .as_ref()
                    .map(|d| !d.is_closed())
                    .unwrap_or(false);
                return Ok(Value::Bool(a));
            },
            #[cfg(not(target_arch = "wasm32"))]
            "dialog_typing" | "对话打字" | "会話タイプ中" | "대화타이핑" | "กำลังพิมพ์บทสนทนา" | "گفتگو_در_حال_تایپ" | "الحوار_يكتب" | "דיאלוג_מקליד" | "مکالمہ_ٹائپنگ" | "dialogue_frappe" | "dialog_tippen" | "диалог_печатает" =>
            {
                use ling_game::dialog::Dialog;

                let a = self
                    .dialog
                    .as_ref()
                    .map(|d: &Dialog| !d.is_closed() && d.is_typing())
                    .unwrap_or(false);
                return Ok(Value::Bool(a));
            },
            #[cfg(not(target_arch = "wasm32"))]
            "dialog_close" | "对话关闭" | "会話閉じる" | "대화닫기" | "ปิดบทสนทนา" | "بستن_گفتگو" | "أغلق_الحوار" | "סגור_דיאלוג" | "مکالمہ_بند" | "fermer_dialogue" | "dialog_schließen" | "закрыть_диалог" =>
            {
                self.dialog = None;
                return Ok(Value::Unit);
            },
            // dialog_color(role, r, g, b) — role: 0 text · 1 name · 2 place · 3 item
            #[cfg(not(target_arch = "wasm32"))]
            "dialog_color" | "对话颜色" | "会話色" | "대화색" | "สีบทสนทนา" | "رنگ_گفتگو" | "لون_الحوار" | "צבע_דיאלוג" | "مکالمہ_رنگ" | "couleur_dialogue" | "dialog_farbe" | "цвет_диалога" =>
            {
                let role = (self.arg_num(&args, 0, 0.0)? as usize).min(3);
                let r = self.arg_num(&args, 1, 255.0)? as u32 & 0xFF;
                let g = self.arg_num(&args, 2, 255.0)? as u32 & 0xFF;
                let b = self.arg_num(&args, 3, 255.0)? as u32 & 0xFF;
                self.dialog_colors[role] = (r << 16) | (g << 8) | b;
                return Ok(Value::Unit);
            },
            // dialog_draw(x, y, w, h [, font_handle]) — draw the box + typed text
            #[cfg(not(target_arch = "wasm32"))]
            "dialog_draw" | "对话绘制" | "会話描画" | "대화그리기" | "วาดบทสนทนา" | "رسم_گفتگو" | "ارسم_الحوار" | "צייר_דיאלוג" | "مکالمہ_کھینچو" | "dessiner_dialogue" | "dialog_zeichnen" | "рисовать_диалог" =>
            {
                let x = self.arg_num(&args, 0, 40.0)? as f32;
                let y = self.arg_num(&args, 1, 0.0)? as f32;
                let ww = self.arg_num(&args, 2, 720.0)? as f32;
                let hh = self.arg_num(&args, 3, 150.0)? as f32;
                let font = self.arg_num(&args, 4, -1.0)? as i64;
                let t = (crate::runtime::now_secs() - self.start_time_secs) as f32;
                self.render_dialog(x, y, ww, hh, font, t);
                return Ok(Value::Unit);
            },

            // text_poll() — fold newly-typed keys into the input buffer, return it.
            // Repeat is enabled (KeyRepeat::Yes) so holding a key/Backspace behaves
            // like a normal text field; length is capped so a stuck key or a runaway
            // script can't grow the buffer without bound.
            #[cfg(not(target_arch = "wasm32"))]
            "text_poll" => {
                const TEXT_BUFFER_MAX: usize = 240;
                // See key_down/key_pressed: our topmost fullscreen window can
                // be visually in front without real Win32 keyboard focus, so
                // WM_KEYDOWN/WM_CHAR (what minifb's get_keys_pressed reads)
                // never arrive. Poll the OS key-state table directly instead
                // — no focus required — with our own repeat-aware edge
                // detection (key_repeat_fire) so holding a key behaves like
                // the KeyRepeat::Yes path below: one char on press, then
                // repeats after a short hold delay.
                #[cfg(windows)]
                {
                    let topmost = self.gfx.borrow().topmost_window;
                    if topmost {
                        if !window_is_foreground(self.gfx.borrow().hwnd) {
                            return Ok(Value::Str(self.text_buffer.clone()));
                        }
                        let shift = os_key_down(VK_SHIFT);
                        let now = crate::runtime::now_secs();
                        let mut gfx = self.gfx.borrow_mut();
                        let back_idx = VK_BACK as usize;
                        let back_down = os_key_down(VK_BACK);
                        let back_was = gfx.raw_keys_prev[back_idx];
                        let (mut back_since, mut back_fire) = (
                            gfx.raw_keys_down_since[back_idx],
                            gfx.raw_keys_last_fire[back_idx],
                        );
                        if key_repeat_fire(now, back_down, back_was, &mut back_since, &mut back_fire) {
                            self.text_buffer.pop();
                        }
                        gfx.raw_keys_down_since[back_idx] = back_since;
                        gfx.raw_keys_last_fire[back_idx] = back_fire;
                        gfx.raw_keys_prev[back_idx] = back_down;
                        for &vk in TEXT_POLL_VKS {
                            let idx = (vk as usize) & 0xFF;
                            let down = os_key_down(vk);
                            let was = gfx.raw_keys_prev[idx];
                            let (mut since, mut fire) =
                                (gfx.raw_keys_down_since[idx], gfx.raw_keys_last_fire[idx]);
                            if key_repeat_fire(now, down, was, &mut since, &mut fire) {
                                if let Some(c) = vk_char(vk, shift) {
                                    if self.text_buffer.chars().count() < TEXT_BUFFER_MAX {
                                        self.text_buffer.push(c);
                                    }
                                }
                            }
                            gfx.raw_keys_down_since[idx] = since;
                            gfx.raw_keys_last_fire[idx] = fire;
                            gfx.raw_keys_prev[idx] = down;
                        }
                        return Ok(Value::Str(self.text_buffer.clone()));
                    }
                }
                let (keys, shift) = {
                    let gfx = self.gfx.borrow();
                    match gfx.window.as_ref() {
                        Some(w) => (
                            w.get_keys_pressed(minifb::KeyRepeat::Yes),
                            w.is_key_down(minifb::Key::LeftShift)
                                || w.is_key_down(minifb::Key::RightShift),
                        ),
                        None => (Vec::new(), false),
                    }
                };
                for k in keys {
                    if k == minifb::Key::Backspace {
                        self.text_buffer.pop();
                    } else if let Some(c) = key_char(k, shift) {
                        if self.text_buffer.chars().count() < TEXT_BUFFER_MAX {
                            self.text_buffer.push(c);
                        }
                    }
                }
                return Ok(Value::Str(self.text_buffer.clone()));
            },
            #[cfg(target_arch = "wasm32")]
            "text_poll" => {
                return Ok(Value::Str(self.text_buffer.clone()));
            },
            "text_get" => return Ok(Value::Str(self.text_buffer.clone())),
            "text_set" => {
                self.text_buffer = self.arg_str(&args, 0, "");
                return Ok(Value::Unit);
            },
            "text_clear" => {
                self.text_buffer.clear();
                return Ok(Value::Unit);
            },
            // record_frame() — append the current framebuffer as a PPM, return frame #
            #[cfg(not(target_arch = "wasm32"))]
            "record_frame" => {
                let n = self.record_n;
                let (buf, w, h) = {
                    let gfx = self.gfx.borrow();
                    (gfx.buffer.clone(), gfx.width, gfx.height)
                };
                let _ = std::fs::create_dir_all("recordings");
                let mut out = Vec::with_capacity(w * h * 3 + 32);
                out.extend_from_slice(format!("P6\n{w} {h}\n255\n").as_bytes());
                for px in &buf {
                    let p = *px;
                    out.push((p >> 16) as u8);
                    out.push((p >> 8) as u8);
                    out.push(p as u8);
                }
                let _ = std::fs::write(format!("recordings/frame_{n:05}.ppm"), out);
                self.record_n += 1;
                return Ok(Value::Number(n as f64));
            },
            "record_count" => return Ok(Value::Number(self.record_n as f64)),
            // ── screenshot(mode) → PNG in ./screenshots/ with timestamp + mode + size ──
            #[cfg(not(target_arch = "wasm32"))]
            "screenshot" | "บันทึกภาพ" | "عکس‌صفحه" | "لقطة_شاشة" | "צילום_מסך" | "اسکرین_شاٹ" => {
                let mode = self.arg_str(&args, 0, "game");
                let (buf, w, h) = {
                    let gfx = self.gfx.borrow();
                    (gfx.buffer.clone(), gfx.width, gfx.height)
                };
                let _ = std::fs::create_dir_all("screenshots");
                let ts = std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .map(|d| d.as_secs())
                    .unwrap_or(0);
                let safe: String = mode
                    .chars()
                    .map(|c| if c.is_alphanumeric() { c } else { '_' })
                    .collect();
                let path = format!("screenshots/ss_{ts}_{safe}_{w}x{h}.png");
                let mut rgb = Vec::with_capacity(w * h * 3);
                for px in &buf {
                    let p = *px;
                    rgb.push((p >> 16) as u8);
                    rgb.push((p >> 8) as u8);
                    rgb.push(p as u8);
                }
                if let Some(img) = image::RgbImage::from_raw(w as u32, h as u32, rgb) {
                    let _ = img.save(&path);
                }
                return Ok(Value::Str(path));
            },
            // ── microphone → crypto donut ──
            // mic_capture() — append the latest mic samples to the record buffer
            // (call each frame while recording). Returns the buffer length.
            #[cfg(not(target_arch = "wasm32"))]
            "mic_capture" => {
                if let Some(mic) = self.mic.as_ref() {
                    let s = mic.latest_samples();
                    self.mic_buffer.extend_from_slice(&s);
                    let cap = 96_000usize; // ~2 s @ 48 kHz
                    if self.mic_buffer.len() > cap {
                        let drop = self.mic_buffer.len() - cap;
                        self.mic_buffer.drain(0..drop);
                    }
                }
                return Ok(Value::Number(self.mic_buffer.len() as f64));
            },
            // mic_seed() — SHA3-256 hex of the recorded audio, usable as a donut seed
            #[cfg(not(target_arch = "wasm32"))]
            "mic_seed" => {
                let mut bytes = Vec::with_capacity(self.mic_buffer.len() * 4);
                for f in &self.mic_buffer {
                    bytes.extend_from_slice(&f.to_le_bytes());
                }
                return Ok(Value::Str(hex_encode(&ling_crypto::geo::holo_hash(&bytes))));
            },
            #[cfg(not(target_arch = "wasm32"))]
            "mic_clear" => {
                self.mic_buffer.clear();
                return Ok(Value::Number(0.0));
            },
            // flush the 3-D depth queue onto the framebuffer WITHOUT presenting,
            // so 2-D UI drawn afterwards overlays the 3-D scene.
            #[cfg(not(target_arch = "wasm32"))]
            "flush_3d" | "render_3d" => {
                let mut gfx = self.gfx.borrow_mut();
                if !gfx.depth_queue.is_empty() {
                    let w = gfx.width;
                    let h = gfx.height;
                    let dt = gfx.depth_test;
                    let reset_z = gfx.zbuf_needs_clear;
                    let (bm, ba) = (gfx.blend, gfx.alpha);
                    let aa = gfx.antialias;
                    let queue = std::mem::take(&mut gfx.depth_queue);
                    {
                        let g = &mut *gfx;
                        let z = if dt { Some(&mut g.depth_buf) } else { None };
                        queue.flush(&mut g.buffer, z, reset_z, w, h, aa);
                    }
                    gfx.zbuf_needs_clear = false;
                    gfx.depth_queue.set_state(bm, ba); // keep active blend/alpha across the mid-frame flush
                }
                return Ok(Value::Unit);
            },
            #[cfg(target_arch = "wasm32")]
            "flush_3d" | "render_3d" => {
                let mut gfx = self.gfx.borrow_mut();
                if !gfx.depth_queue.is_empty() {
                    let w = gfx.width;
                    let h = gfx.height;
                    let dt = gfx.depth_test;
                    let reset_z = gfx.zbuf_needs_clear;
                    let (bm, ba) = (gfx.blend, gfx.alpha);
                    let aa = gfx.antialias;
                    let queue = std::mem::take(&mut gfx.depth_queue);
                    {
                        let g = &mut *gfx;
                        let z = if dt { Some(&mut g.depth_buf) } else { None };
                        queue.flush(&mut g.buffer, z, reset_z, w, h, aa);
                    }
                    gfx.zbuf_needs_clear = false;
                    gfx.depth_queue.set_state(bm, ba);
                }
                return Ok(Value::Unit);
            },

            // flush_post() — flush the 3-D queue like `flush_3d`, then run the
            // toon post-chain (SSAO → outlines → tone ramp → bloom → FXAA) over
            // the SCENE immediately. `present` skips the chain this frame, so
            // 2-D UI drawn after this call stays exact — no bloom/blur on HUDs.
            "flush_post" | "post_now" | "포스트플러시" | "后期冲刷" => {
                let mut gfx = self.gfx.borrow_mut();
                if !gfx.depth_queue.is_empty() {
                    let w = gfx.width;
                    let h = gfx.height;
                    let dt = gfx.depth_test;
                    let reset_z = gfx.zbuf_needs_clear;
                    let (bm, ba) = (gfx.blend, gfx.alpha);
                    let aa = gfx.antialias;
                    let queue = std::mem::take(&mut gfx.depth_queue);
                    {
                        let g = &mut *gfx;
                        let z = if dt { Some(&mut g.depth_buf) } else { None };
                        queue.flush(&mut g.buffer, z, reset_z, w, h, aa);
                    }
                    gfx.zbuf_needs_clear = false;
                    gfx.depth_queue.set_state(bm, ba);
                }
                gfx.toon_post_process();
                gfx.post_done = true;
                return Ok(Value::Unit);
            },

            // Viscous full-screen distortion (warp/pucker/bloat, edge-wrapped). Call
            // after the 3-D flush and before the UI so only the world layer warps.
            #[cfg(not(target_arch = "wasm32"))]
            "screen_distort" | "บิดจอ" | "屏幕扭曲" | "画面歪み" | "화면왜곡" | "اعوجاج_صفحه" | "شوّه_الشاشة" | "עוות_מסך" | "اسکرین_ڈسٹورٹ" =>
            {
                let amount = self.arg_num(&args, 0, 8.0)? as f32;
                let t = self.arg_num(&args, 1, 0.0)? as f32;
                // optional `step` (default 1 = full res): 2 = half-res block warp
                // (~4× fewer warp computes, slightly softer — suits a liquid look).
                let step = self.arg_num(&args, 2, 1.0)?.max(1.0) as usize;
                let _d = std::time::Instant::now();
                self.gfx.borrow_mut().distort(amount, t, step);
                ling_phase_add(phase::DISTORT, _d.elapsed().as_nanos());
                return Ok(Value::Unit);
            },

            "set_rim" | "设置边缘光" | "リム設定" | "림라이트" | "ตั้งขอบเรือง" | "تنظیم_نور_لبه" | "عيّن_إضاءة_الحافة" | "קבע_תאורת_קצה" | "رم_لائٹ_مقرر_کرو" | "définir_contour_lumineux" | "rimlicht_setzen" | "задать_контурный_свет" =>
            {
                let s = self.arg_num(&args, 0, 0.6)? as f32;
                let r = self.arg_num(&args, 1, 115.)? as f32 / 255.0;
                let g = self.arg_num(&args, 2, 217.)? as f32 / 255.0;
                let b = self.arg_num(&args, 3, 255.)? as f32 / 255.0;
                let mut gfx = self.gfx.borrow_mut();
                gfx.shade.rim = s;
                gfx.shade.rim_color = [r, g, b];
                return Ok(Value::Unit);
            },

            // ══════════════════════════════════════════════════════════════════
            // 3-D PRIMITIVES  (src/gfx/shapes.rs)  — "Inkscape for 3-D"
            //   shape(cx,cy,cz,  sx,sy,sz,  rx,ry,rz,  mode,  e0,e1,e2)
            //     centre (cx,cy,cz), per-axis scale, Euler rotation (radians),
            //     mode: 0 filled · 1 wireframe · 2 both,
            //     e0..e2: shape-specific (segments / sides / ratio …).
            //   Pen colour (set_color) drives fill lighting and wireframe colour.
            // ══════════════════════════════════════════════════════════════════
            n if crate::gfx::shapes::canon(n).is_some() => {
                let kind = crate::gfx::shapes::canon(n).unwrap();
                let cx = self.arg_num(&args, 0, 0.)? as f32;
                let cy = self.arg_num(&args, 1, 0.)? as f32;
                let cz = self.arg_num(&args, 2, 0.)? as f32;
                let sx = self.arg_num(&args, 3, 1.)? as f32;
                let sy = self.arg_num(&args, 4, 1.)? as f32;
                let sz = self.arg_num(&args, 5, 1.)? as f32;
                let rx = self.arg_num(&args, 6, 0.)? as f32;
                let ry = self.arg_num(&args, 7, 0.)? as f32;
                let rz = self.arg_num(&args, 8, 0.)? as f32;
                let mode = self.arg_num(&args, 9, 0.)? as i32;
                let e0 = self.arg_num(&args, 10, 0.)? as f32;
                let e1 = self.arg_num(&args, 11, 0.)? as f32;
                let e2 = self.arg_num(&args, 12, 0.)? as f32;
                if let Some(mesh) = crate::gfx::shapes::build(
                    kind,
                    [cx, cy, cz, sx, sy, sz, rx, ry, rz],
                    e0,
                    e1,
                    e2,
                ) {
                    let mut gfx = self.gfx.borrow_mut();
                    gfx.emit_mesh(&mesh, mode);
                }
                return Ok(Value::Unit);
            },

            _ => {},
        }

        // `form` struct constructor: positional `Name(v0, v1, ...)`.
        if let Some(field_names) = self.structs.get(name).cloned() {
            if args.len() != field_names.len() {
                return Err(EvalErr::from(format!(
                    "{name} expects {} field(s), got {}",
                    field_names.len(),
                    args.len()
                )));
            }
            let fields = field_names.into_iter().zip(args).collect();
            return Ok(Value::Struct { name: name.to_string(), fields });
        }

        // `choose` enum variant constructor: `Variant(...)` or `Enum::Variant(...)`.
        if let Some((enum_name, arity)) = self.enum_variants.get(name).cloned() {
            if args.len() != arity {
                return Err(EvalErr::from(format!(
                    "{name} expects {arity} value(s), got {}",
                    args.len()
                )));
            }
            let variant = name.rsplit("::").next().unwrap_or(name).to_string();
            return Ok(Value::Variant { enum_name, variant, payload: args });
        }

        #[cfg(target_arch = "wasm32")]
        if let Some(v) = wasm_unsupported_builtin(name) {
            return Ok(v);
        }

        Err(EvalErr::from(format!("unknown function '{name}'")))
    }

    fn call_value(&mut self, v: Value, args: Vec<Value>) -> EvalResult {
        match v {
            Value::Fn(params, body, mut captured) => {
                for (p, a) in params.iter().zip(args) {
                    captured.insert(p.clone(), a);
                }
                match self.framed("<closure>", |me| me.exec_block(&body, &mut captured)) {
                    Ok(v) => Ok(v.unwrap_or(Value::Unit)),
                    Err(EvalErr::Return(v)) => Ok(v),
                    Err(e) => Err(e),
                }
            },
            other => Err(EvalErr::from(format!("cannot call {:?}", other))),
        }
    }

    fn call_method(&self, recv: Value, method: &str, args: Vec<Value>) -> EvalResult {
        match (&recv, method) {
            (Value::Str(s), "is_empty" | "是空") => Ok(Value::Bool(s.is_empty())),
            // All of `lingfu normalize`'s per-language spellings of len/push
            // (see ling-fu normalize.rs alias table), not just the Chinese
            // ones — normalize rewrites method calls into whichever language
            // the project is normalized to, and any spelling missing here
            // makes those calls un-callable post-normalize (first hit with
            // `.长度()`, then again with Thai `.ความยาว()`).
            (Value::Str(s), "len" | "长" | "长度" | "長さ" | "길이" | "ความยาว") => Ok(Value::Number(s.len() as f64)),
            (Value::Str(s), "to_string" | "转文") => Ok(Value::Str(s.clone())),
            (Value::Str(s), "contains" | "包含") => {
                if let Some(Value::Str(sub)) = args.first() {
                    Ok(Value::Bool(s.contains(sub.as_str())))
                } else {
                    Ok(Value::Bool(false))
                }
            },
            (Value::Str(s), "push_str" | "推_文") => {
                let mut s2 = s.clone();
                if let Some(Value::Str(a)) = args.first() {
                    s2.push_str(a);
                }
                Ok(Value::Str(s2))
            },
            (Value::List(v), "len" | "长" | "长度" | "長さ" | "길이" | "ความยาว") => Ok(Value::Number(v.len() as f64)),
            (Value::List(v), "push" | "推" | "添加" | "追加" | "추가" | "เพิ่ม") => {
                let mut v2: Vec<Value> = (**v).clone();
                if let Some(a) = args.first() {
                    v2.push(a.clone());
                }
                Ok(Value::List(Rc::new(v2)))
            },
            // `form` field access: `point.x` (no-arg method == field read).
            (Value::Struct { fields, .. }, _) if args.is_empty() => fields
                .iter()
                .find(|(k, _)| k == method)
                .map(|(_, v)| v.clone())
                .ok_or_else(|| EvalErr::from(format!("no field '{method}' on {recv}"))),
            // Enum introspection: `.tag` → variant name, `.is(Name)` not needed for now.
            (Value::Variant { variant, .. }, "tag" | "标签" | "タグ" | "태그" | "ป้าย")
                if args.is_empty() =>
            {
                Ok(Value::Str(variant.clone()))
            },
            (Value::Ok(inner), _) | (Value::Err(inner), _) => Ok(*inner.clone()),
            _ => Err(EvalErr::from(format!("no method '{method}' on {recv}"))),
        }
    }

    // ─── Pattern matching ─────────────────────────────────────────────────────

    fn match_pattern(&self, pat: &Pattern, val: &Value) -> Option<Env> {
        match (pat, val) {
            (Pattern::Wildcard, _) => Some(new_env()),
            (Pattern::Str(s), Value::Str(v)) if s == v => Some(new_env()),
            (Pattern::Number(n), Value::Number(v)) if (n - v).abs() < 1e-12 => Some(new_env()),
            (Pattern::Bool(b), Value::Bool(v)) if b == v => Some(new_env()),
            (Pattern::Ident(name), _) => {
                let mut e = new_env();
                e.insert(name.clone(), val.clone());
                Some(e)
            },
            (Pattern::Constructor(ctor, inner_pat), _) => {
                let (matches, inner_val) = match (ctor.as_str(), val) {
                    ("ok" | "好", Value::Ok(v)) => (true, Some(v.as_ref().clone())),
                    ("bad" | "坏", Value::Err(v)) => (true, Some(v.as_ref().clone())),
                    ("ok" | "好", v) if !matches!(v, Value::Err(_)) => (true, Some(v.clone())),
                    _ => (false, None),
                };
                if !matches {
                    return None;
                }
                match (inner_pat, inner_val) {
                    (Some(p), Some(v)) => self.match_pattern(p, &v),
                    (None, _) => Some(new_env()),
                    (Some(p), None) => self.match_pattern(p, &Value::Unit),
                }
            },
            // User enum variant pattern: `Circle(r)`, `Pair(a, b)`, nullary `Origin()`.
            (Pattern::Variant(vname, sub_pats), Value::Variant { variant, payload, .. }) => {
                if vname != variant || sub_pats.len() != payload.len() {
                    return None;
                }
                let mut bindings = new_env();
                for (p, v) in sub_pats.iter().zip(payload.iter()) {
                    bindings.extend(self.match_pattern(p, v)?);
                }
                Some(bindings)
            },
            // A zero-payload variant pattern also matches the bare result-style `ok`/`bad`
            // values so `Ok()`-style patterns keep working uniformly.
            (Pattern::Variant(vname, sub), Value::Ok(v)) if (vname == "ok" || vname == "好") => {
                match sub.as_slice() {
                    [] => Some(new_env()),
                    [p] => self.match_pattern(p, v),
                    _ => None,
                }
            },
            (Pattern::Variant(vname, sub), Value::Err(v))
                if (vname == "bad" || vname == "坏" || vname == "err") =>
            {
                match sub.as_slice() {
                    [] => Some(new_env()),
                    [p] => self.match_pattern(p, v),
                    _ => None,
                }
            },
            _ => None,
        }
    }

    // ─── Utilities ───────────────────────────────────────────────────────────

    fn value_to_iter(&self, val: Value) -> Result<Vec<Value>, EvalErr> {
        match val {
            Value::List(v) => Ok(Rc::try_unwrap(v).unwrap_or_else(|rc| (*rc).clone())),
            Value::Str(s) => Ok(s.chars().map(|c| Value::Str(c.to_string())).collect()),
            Value::Number(n) => Ok((0..n as i64).map(|i| Value::Number(i as f64)).collect()),
            other => Err(EvalErr::from(format!("cannot iterate over {:?}", other))),
        }
    }

    pub(crate) fn is_truthy(&self, val: &Value) -> bool {
        match val {
            Value::Bool(b) => *b,
            Value::Unit => false,
            Value::Number(n) => *n != 0.0,
            Value::Str(s) => !s.is_empty(),
            Value::List(v) => !v.is_empty(),
            Value::Ok(_) => true,
            Value::Err(_) => false,
            Value::Fn(_, _, _) => true,
            Value::Struct { .. } => true,
            Value::Variant { .. } => true,
        }
    }

    fn to_number(&self, val: &Value) -> Result<f64, EvalErr> {
        match val {
            Value::Number(n) => Ok(*n),
            Value::Str(s) => s
                .parse()
                .map_err(|_| EvalErr::from(format!("cannot convert '{s}' to number"))),
            other => Err(EvalErr::from(format!("expected number, got {:?}", other))),
        }
    }

    /// Get the n-th argument as f64, falling back to `default` if missing.
    fn arg_num(&self, args: &[Value], n: usize, default: f64) -> Result<f64, EvalErr> {
        match args.get(n) {
            Some(v) => self.to_number(v),
            None => Ok(default),
        }
    }

    fn arg_str(&self, args: &[Value], n: usize, default: &str) -> String {
        args.get(n)
            .map(|v| v.to_string())
            .unwrap_or_else(|| default.to_string())
    }

    /// Read a list-of-numbers argument as `Vec<f32>` (empty if absent/not a list).
    #[allow(dead_code)]
    fn arg_list_f32(&self, args: &[Value], n: usize) -> Vec<f32> {
        match args.get(n) {
            Some(Value::List(v)) => v
                .iter()
                .filter_map(|x| match x {
                    Value::Number(n) => Some(*n as f32),
                    _ => None,
                })
                .collect(),
            _ => Vec::new(),
        }
    }

    /// Optional `r,g,b` colour override starting at arg `i` → packed 0x00RRGGBB,
    /// or `default` if those three numeric args aren't present.
    #[cfg(not(target_arch = "wasm32"))]
    fn color_at(&self, args: &[Value], i: usize, default: u32) -> u32 {
        match (args.get(i), args.get(i + 1), args.get(i + 2)) {
            (Some(a), Some(b), Some(c)) => {
                match (self.to_number(a), self.to_number(b), self.to_number(c)) {
                    (Ok(r), Ok(g), Ok(bl)) => {
                        ((r as u32 & 0xFF) << 16) | ((g as u32 & 0xFF) << 8) | (bl as u32 & 0xFF)
                    },
                    _ => default,
                }
            },
            _ => default,
        }
    }

    /// A pitch argument: a note-name string (`"C4"`, `"A#3"`) or a numeric MIDI value.
    #[cfg(not(target_arch = "wasm32"))]
    fn pitch_arg(&self, args: &[Value], i: usize, default: i32) -> i32 {
        match args.get(i) {
            Some(Value::Str(s)) => ling_music::note::parse_pitch(s).unwrap_or(default),
            Some(Value::Number(n)) => *n as i32,
            _ => default,
        }
    }

    /// Current mouse position + left-button-down (native window only).
    #[cfg(not(target_arch = "wasm32"))]
    fn mouse_now(&self) -> (f32, f32, bool) {
        let gfx = self.gfx.borrow();
        let (mx, my) = gfx
            .window
            .as_ref()
            .and_then(|w| w.get_mouse_pos(minifb::MouseMode::Clamp))
            .unwrap_or((0.0, 0.0));
        let down = gfx
            .window
            .as_ref()
            .map(|w| w.get_mouse_down(minifb::MouseButton::Left))
            .unwrap_or(false);
        (mx, my, down)
    }

    /// Rasterize a UI [`ling_ui::widgets::Draw`] into the framebuffer: filled
    /// polygons via the AA scanline fill, polylines via AA lines, honouring the
    /// current blend mode.
    #[cfg(not(target_arch = "wasm32"))]
    fn draw_ui(&self, d: &ling_ui::widgets::Draw) {
        let mut gfx = self.gfx.borrow_mut();
        let (w, h, add) = (gfx.width, gfx.height, gfx.blend == 1);
        for (c, poly) in &d.fills {
            crate::gfx::raster::fill_contours_aa(
                &mut gfx.buffer,
                w,
                h,
                *c,
                add,
                std::slice::from_ref(poly),
            );
        }
        for (c, pl) in &d.strokes {
            for s in pl.windows(2) {
                crate::gfx::raster::draw_line_aa(
                    &mut gfx.buffer,
                    w,
                    h,
                    *c,
                    add,
                    s[0][0],
                    s[0][1],
                    s[1][0],
                    s[1][1],
                );
            }
        }
    }

    /// Parse (dst_x, dst_y, width, height) from the first four args of a tex_* builtin.
    fn tex_rect(&self, args: &[Value]) -> Result<(usize, usize, usize, usize), EvalErr> {
        let tx = self.arg_num(args, 0, 0.0)? as usize;
        let ty = self.arg_num(args, 1, 0.0)? as usize;
        let tw = self.arg_num(args, 2, 256.0)? as usize;
        let th = self.arg_num(args, 3, 256.0)? as usize;
        Ok((tx, ty, tw.max(1), th.max(1)))
    }

    pub(crate) fn apply_binop(&self, op: &BinOp, l: Value, r: Value) -> EvalResult {
        match op {
            BinOp::Add => match (l, r) {
                (Value::Number(a), Value::Number(b)) => Ok(Value::Number(a + b)),
                (Value::Str(a), Value::Str(b)) => Ok(Value::Str(a + &b)),
                (Value::Str(a), b) => Ok(Value::Str(a + &b.to_string())),
                (a, Value::Str(b)) => Ok(Value::Str(a.to_string() + &b)),
                (a, b) => Err(EvalErr::from(format!("cannot add {:?} and {:?}", a, b))),
            },
            BinOp::Sub => Ok(Value::Number(self.to_number(&l)? - self.to_number(&r)?)),
            BinOp::Mul => Ok(Value::Number(self.to_number(&l)? * self.to_number(&r)?)),
            BinOp::Div => Ok(Value::Number(self.to_number(&l)? / self.to_number(&r)?)),
            BinOp::Rem => Ok(Value::Number(self.to_number(&l)? % self.to_number(&r)?)),
            BinOp::Eq => Ok(Value::Bool(values_equal(&l, &r))),
            BinOp::Ne => Ok(Value::Bool(!values_equal(&l, &r))),
            BinOp::Lt => Ok(Value::Bool(self.to_number(&l)? < self.to_number(&r)?)),
            BinOp::Gt => Ok(Value::Bool(self.to_number(&l)? > self.to_number(&r)?)),
            BinOp::Le => Ok(Value::Bool(self.to_number(&l)? <= self.to_number(&r)?)),
            BinOp::Ge => Ok(Value::Bool(self.to_number(&l)? >= self.to_number(&r)?)),
            BinOp::And => Ok(Value::Bool(self.is_truthy(&l) && self.is_truthy(&r))),
            BinOp::Or => Ok(Value::Bool(self.is_truthy(&l) || self.is_truthy(&r))),
        }
    }

    fn builtin_format(&self, args: &[Value]) -> Result<String, EvalErr> {
        if args.is_empty() {
            return Ok(String::new());
        }
        let fmt = match &args[0] {
            Value::Str(s) => s.clone(),
            other => return Ok(other.to_string()),
        };

        let mut result = String::new();
        let mut arg_idx = 1usize;
        let mut chars = fmt.chars().peekable();
        while let Some(c) = chars.next() {
            if c == '{' {
                if chars.peek() == Some(&'}') {
                    chars.next();
                    if arg_idx < args.len() {
                        result.push_str(&args[arg_idx].to_string());
                        arg_idx += 1;
                    }
                } else {
                    let mut spec = String::new();
                    for ch in chars.by_ref() {
                        if ch == '}' {
                            break;
                        }
                        spec.push(ch);
                    }
                    if arg_idx < args.len() {
                        if let Some(suffix) = spec.strip_prefix(":.") {
                            if let Value::Number(n) = &args[arg_idx] {
                                let prec: usize =
                                    suffix.trim_end_matches('f').parse().unwrap_or(2);
                                result.push_str(&format!("{:.prec$}", n));
                                arg_idx += 1;
                                continue;
                            }
                        }
                        result.push_str(&args[arg_idx].to_string());
                        arg_idx += 1;
                    }
                }
            } else {
                result.push(c);
            }
        }
        Ok(result)
    }
}

#[cfg(not(target_arch = "wasm32"))]
/// Map a friendly button name (any vendor / d-pad alias) to a gamepad button.
#[cfg(not(target_arch = "wasm32"))]
fn parse_pad_button(name: &str) -> Option<ling_input::GamepadButton> {
    use ling_input::GamepadButton as B;
    Some(match name.to_ascii_lowercase().as_str() {
        "a" | "south" | "cross" => B::South,
        "b" | "east" | "circle" => B::East,
        "x" | "west" | "square" => B::West,
        "y" | "north" | "triangle" => B::North,
        "lb" | "l1" | "left_shoulder" => B::LeftShoulder,
        "rb" | "r1" | "right_shoulder" => B::RightShoulder,
        "lt" | "l2" | "left_trigger" => B::LeftTrigger,
        "rt" | "r2" | "right_trigger" => B::RightTrigger,
        "start" | "menu" | "options" | "démarrer" | "начать" => B::Start,
        "select" | "back" | "share" | "view" => B::Select,
        "guide" | "home" => B::Guide,
        "l3" | "left_stick" => B::LeftStick,
        "r3" | "right_stick" => B::RightStick,
        "up" | "dpad_up" => B::DpadUp,
        "down" | "dpad_down" => B::DpadDown,
        "left" | "dpad_left" => B::DpadLeft,
        "right" | "dpad_right" => B::DpadRight,
        _ => return None,
    })
}

#[cfg(not(target_arch = "wasm32"))]
fn str_to_minifb_key(name: &str) -> Option<minifb::Key> {
    use minifb::Key;
    Some(match name {
        "numpad0" | "kp0" => Key::NumPad0,
        "numpad1" | "kp1" => Key::NumPad1,
        "numpad2" | "kp2" => Key::NumPad2,
        "numpad3" | "kp3" => Key::NumPad3,
        "numpad4" | "kp4" => Key::NumPad4,
        "numpad5" | "kp5" => Key::NumPad5,
        "numpad6" | "kp6" => Key::NumPad6,
        "numpad7" | "kp7" => Key::NumPad7,
        "numpad8" | "kp8" => Key::NumPad8,
        "numpad9" | "kp9" => Key::NumPad9,
        "numpad+" | "kp+" => Key::NumPadPlus,
        "numpad-" | "kp-" => Key::NumPadMinus,
        "numpad*" | "kp*" => Key::NumPadAsterisk,
        "numpad/" | "kp/" => Key::NumPadSlash,
        "left" => Key::Left,
        "right" => Key::Right,
        "up" => Key::Up,
        "down" => Key::Down,
        "space" => Key::Space,
        "enter" => Key::Enter,
        "escape" => Key::Escape,
        "pageup" => Key::PageUp,
        "pagedown" => Key::PageDown,
        "lshift" | "leftshift" => Key::LeftShift,
        "rshift" | "rightshift" => Key::RightShift,
        "lctrl" | "leftctrl" => Key::LeftCtrl,
        "rctrl" | "rightctrl" => Key::RightCtrl,
        "lalt" | "leftalt" => Key::LeftAlt,
        "ralt" | "rightalt" => Key::RightAlt,
        "tab" => Key::Tab,
        "backspace" => Key::Backspace,
        "delete" => Key::Delete,
        "insert" => Key::Insert,
        "home" => Key::Home,
        "end" => Key::End,
        "a" => Key::A,
        "b" => Key::B,
        "c" => Key::C,
        "d" => Key::D,
        "e" => Key::E,
        "f" => Key::F,
        "g" => Key::G,
        "h" => Key::H,
        "i" => Key::I,
        "j" => Key::J,
        "k" => Key::K,
        "l" => Key::L,
        "m" => Key::M,
        "n" => Key::N,
        "o" => Key::O,
        "p" => Key::P,
        "q" => Key::Q,
        "r" => Key::R,
        "s" => Key::S,
        "t" => Key::T,
        "u" => Key::U,
        "v" => Key::V,
        "w" => Key::W,
        "x" => Key::X,
        "y" => Key::Y,
        "z" => Key::Z,
        "0" => Key::Key0,
        "1" => Key::Key1,
        "2" => Key::Key2,
        "3" => Key::Key3,
        "4" => Key::Key4,
        "5" => Key::Key5,
        "6" => Key::Key6,
        "7" => Key::Key7,
        "8" => Key::Key8,
        "9" => Key::Key9,
        _ => return None,
    })
}

pub(crate) fn values_equal(a: &Value, b: &Value) -> bool {
    match (a, b) {
        (Value::Number(x), Value::Number(y)) => (x - y).abs() < 1e-12,
        (Value::Str(x), Value::Str(y)) => x == y,
        (Value::Bool(x), Value::Bool(y)) => x == y,
        (Value::Unit, Value::Unit) => true,
        _ => false,
    }
}

// Rasteriser functions live in crate::gfx::raster — imported at top of file.

// ── Window platform helpers ────────────────────────────────────────────────────

/// Strip *all* window chrome from `hwnd` and make it cover the whole primary
/// monitor (0,0 → screen_w × screen_h), above the taskbar. This turns the
/// minifb window into a true borderless-fullscreen surface: no title bar, no
/// frame, no resize grips — there is no visible window "handle" left.
#[cfg(all(not(target_arch = "wasm32"), windows))]
fn make_borderless_fullscreen(hwnd: isize, screen_w: i32, screen_h: i32) {
    if hwnd == 0 {
        return;
    }
    unsafe {
        extern "system" {
            fn SetWindowLongPtrW(hwnd: isize, index: i32, new: isize) -> isize;
            fn SetWindowPos(
                hwnd: isize,
                insert_after: isize,
                x: i32,
                y: i32,
                cx: i32,
                cy: i32,
                flags: u32,
            ) -> i32;
            fn ShowWindow(hwnd: isize, cmd: i32) -> i32;
        }
        const GWL_STYLE: i32 = -16;
        const GWL_EXSTYLE: i32 = -20;
        // WS_POPUP (0x80000000) | WS_VISIBLE (0x10000000) — a bare top-level
        // window with no caption, border, or system menu.
        SetWindowLongPtrW(hwnd, GWL_STYLE, 0x9000_0000isize);
        // Clear extended edges (WS_EX_WINDOWEDGE / CLIENTEDGE / DLGMODALFRAME).
        SetWindowLongPtrW(hwnd, GWL_EXSTYLE, 0);
        // HWND_TOPMOST = -1; SWP_FRAMECHANGED (0x0020) | SWP_SHOWWINDOW (0x0040).
        SetWindowPos(hwnd, -1isize, 0, 0, screen_w, screen_h, 0x0020 | 0x0040);
        ShowWindow(hwnd, 3); // SW_MAXIMIZE-equivalent paint; 3 = SW_SHOWMAXIMIZED
    }
}

/// Force real OS keyboard focus onto `hwnd`, not just Z-order prominence.
/// Windows' foreground-lock can leave a freshly-created window topmost — so
/// VISUALLY it covers everything — without actually handing it keyboard
/// focus, e.g. when launched from a terminal that still holds real focus:
/// clicks can nudge focus over (a more "user-driven" event) but typed keys
/// silently keep going to whatever app really has it, which looks exactly
/// like "clicking a text field doesn't focus it". AttachThreadInput is the
/// standard documented workaround — it lets SetForegroundWindow succeed even
/// under the lock by sharing input state with whichever thread currently
/// owns the foreground window. Call this LAST, after every other
/// window-visibility change for this launch (anything that shows/hides a
/// window afterward — e.g. hiding the launching console — can itself
/// reassign the foreground window and undo an earlier focus claim).
#[cfg(all(not(target_arch = "wasm32"), windows))]
fn force_window_focus(hwnd: isize) {
    if hwnd == 0 {
        return;
    }
    unsafe {
        extern "system" {
            fn GetForegroundWindow() -> isize;
            fn GetWindowThreadProcessId(hwnd: isize, pid: *mut u32) -> u32;
            fn GetCurrentThreadId() -> u32;
            fn AttachThreadInput(id_attach: u32, id_attach_to: u32, attach: i32) -> i32;
            fn SetForegroundWindow(hwnd: isize) -> i32;
            fn BringWindowToTop(hwnd: isize) -> i32;
            fn SetFocus(hwnd: isize) -> isize;
            fn SetActiveWindow(hwnd: isize) -> isize;
        }
        let fg = GetForegroundWindow();
        if fg != 0 && fg != hwnd {
            let mut fg_pid: u32 = 0;
            let fg_tid = GetWindowThreadProcessId(fg, &mut fg_pid);
            let my_tid = GetCurrentThreadId();
            if fg_tid != 0 && fg_tid != my_tid {
                AttachThreadInput(my_tid, fg_tid, 1);
                SetForegroundWindow(hwnd);
                BringWindowToTop(hwnd);
                SetFocus(hwnd);
                SetActiveWindow(hwnd);
                AttachThreadInput(my_tid, fg_tid, 0);
                return;
            }
        }
        SetForegroundWindow(hwnd);
        BringWindowToTop(hwnd);
        SetFocus(hwnd);
        SetActiveWindow(hwnd);
    }
}

/// Toggle `hwnd`'s HWND_TOPMOST z-order style without moving/resizing/
/// activating it — used to drop the borderless-fullscreen window's topmost
/// flag on alt-tab (so it stops covering whatever the user switched to) and
/// restore it when the user switches back.
#[cfg(all(not(target_arch = "wasm32"), windows))]
fn set_window_topmost(hwnd: isize, topmost: bool) {
    if hwnd == 0 {
        return;
    }
    unsafe {
        extern "system" {
            fn SetWindowPos(
                hwnd: isize,
                insert_after: isize,
                x: i32,
                y: i32,
                cx: i32,
                cy: i32,
                flags: u32,
            ) -> i32;
        }
        let insert_after: isize = if topmost { -1 } else { -2 }; // HWND_TOPMOST / HWND_NOTOPMOST
        // SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE — pure z-order change,
        // must not steal focus back when restoring topmost on refocus.
        SetWindowPos(hwnd, insert_after, 0, 0, 0, 0, 0x0002 | 0x0001 | 0x0010);
    }
}

/// Pace `win` to `vsync`'s target rate. `LING_FPS_CAP` (0 = uncapped, else an
/// explicit fps) always overrides; otherwise vsync-on paces to the monitor's
/// real refresh rate and vsync-off runs uncapped. minifb has no swap-interval
/// vsync (it owns no GPU present queue), so this is frame-rate pacing to the
/// refresh rate, not a tear-free guarantee.
#[cfg(not(target_arch = "wasm32"))]
fn apply_frame_pacing(win: &mut minifb::Window, vsync: bool) {
    match std::env::var("LING_FPS_CAP")
        .ok()
        .and_then(|v| v.parse::<usize>().ok())
    {
        Some(0) => win.set_target_fps(100_000),
        Some(cap) => win.set_target_fps(cap),
        None if vsync => win.set_target_fps(monitor_info().2.max(30) as usize),
        None => win.set_target_fps(100_000),
    }
}

/// Primary-monitor resolution and refresh rate as `(width, height, hz)`.
/// `hz` falls back to 60 when the driver reports an unknown/`default` rate.
#[cfg(all(not(target_arch = "wasm32"), windows))]
fn monitor_info() -> (i32, i32, i32) {
    unsafe {
        extern "system" {
            fn GetSystemMetrics(index: i32) -> i32;
            fn GetDC(hwnd: isize) -> isize;
            fn ReleaseDC(hwnd: isize, hdc: isize) -> i32;
            fn GetDeviceCaps(hdc: isize, index: i32) -> i32;
        }
        let w = GetSystemMetrics(0).max(1); // SM_CXSCREEN
        let h = GetSystemMetrics(1).max(1); // SM_CYSCREEN
        let hdc = GetDC(0);
        let mut hz = if hdc != 0 { GetDeviceCaps(hdc, 116) } else { 0 }; // VREFRESH
        if hdc != 0 {
            ReleaseDC(0, hdc);
        }
        if hz <= 1 {
            hz = 60; // 0 or 1 means "device default" → assume 60 Hz
        }
        (w, h, hz)
    }
}

/// Non-Windows native fallback: resolution from [`native_screen_size`]; refresh
/// from the active X11/RandR mode (so a 144 Hz panel drives the loop at 144),
/// falling back to 60 Hz when it can't be detected (Wayland, headless, macOS).
#[cfg(all(not(target_arch = "wasm32"), not(windows)))]
fn monitor_info() -> (i32, i32, i32) {
    let (w, h) = native_screen_size();
    (w as i32, h as i32, linux_refresh_hz().unwrap_or(60))
}

/// Active display refresh rate via `xrandr`. Each connected output's active
/// mode is the token flagged with `*` (e.g. `1920x1080 144.00*+`); we take the
/// max across all active outputs so a multi-monitor rig drives the loop at
/// its fastest panel.
#[cfg(all(not(target_arch = "wasm32"), not(windows)))]
fn linux_refresh_hz() -> Option<i32> {
    let out = std::process::Command::new("xrandr")
        .arg("--current")
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    parse_xrandr_max_hz(&String::from_utf8_lossy(&out.stdout))
}

/// Pure parse used by [`linux_refresh_hz`]: the highest `*`-flagged refresh
/// rate across all active outputs in `xrandr --current` output.
#[cfg(all(not(target_arch = "wasm32"), not(windows)))]
fn parse_xrandr_max_hz(text: &str) -> Option<i32> {
    text.split_whitespace()
        .filter(|tok| tok.contains('*'))
        .filter_map(|tok| {
            tok.trim_matches(|c: char| !c.is_ascii_digit() && c != '.')
                .parse::<f64>()
                .ok()
        })
        .map(|hz| hz.round() as i32)
        .filter(|&hz| (24..=1000).contains(&hz))
        .max()
}

/// WASM fallback: the canvas is the display surface; assume 60 Hz.
#[cfg(target_arch = "wasm32")]
fn monitor_info() -> (i32, i32, i32) {
    let (w, h) = crate::gfx::webgl::canvas_size();
    (w as i32, h as i32, 60)
}

#[cfg(all(test, not(target_arch = "wasm32"), not(windows)))]
mod xrandr_tests {
    use super::parse_xrandr_max_hz;

    #[test]
    fn picks_highest_active_output() {
        let text = "\
eDP-1 connected primary 1920x1080+0+0
   1920x1080     60.00*+  59.94
DP-1 connected 2560x1440+1920+0
   2560x1440    144.00*+  120.00  60.00
";
        assert_eq!(parse_xrandr_max_hz(text), Some(144));
    }

    #[test]
    fn single_output() {
        let text = "   1920x1080     75.00*+  60.00\n";
        assert_eq!(parse_xrandr_max_hz(text), Some(75));
    }

    #[test]
    fn no_active_mode_returns_none() {
        let text = "eDP-1 disconnected\n";
        assert_eq!(parse_xrandr_max_hz(text), None);
    }

    #[test]
    fn out_of_range_hz_filtered() {
        let text = "   1x1     5000.00*+\n";
        assert_eq!(parse_xrandr_max_hz(text), None);
    }
}

/// Query the primary display resolution on non-Windows platforms.
/// Falls back to 1920×1080 if the size cannot be determined.
#[cfg(all(not(target_arch = "wasm32"), not(windows)))]
fn native_screen_size() -> (f64, f64) {
    // On Linux/macOS we don't have an easy dependency-free call; return a
    // sensible default. Callers can always pass explicit dimensions.
    (1920.0, 1080.0)
}

// ════════════════════════════════════════════════════════════════════════════
// Builtin call profiler  (env-gated, near-zero cost when off)
//
//   LING_PROFILE=1            enable per-builtin call-count + inclusive-time tally
//   LING_PROFILE_EVERY=N      print the report every N frames (default 240)
//
// Every builtin call funnels through `Interp::call_named` (JIT via `ling_builtin`,
// tree-walker directly), so this captures the full render/physics/audio builtin
// hot-path. Report is sorted by total time and prints calls, calls/frame,
// total_ms and ms/frame — the top-down "what's making so many calls" view.
// ════════════════════════════════════════════════════════════════════════════
struct LingProfileState {
    enabled: bool,
    every: u64,
    frames: u64,
    calls: std::collections::HashMap<String, (u64, u128)>, // name -> (count, nanos)
}

thread_local! {
    static LING_PROFILE: std::cell::RefCell<LingProfileState> = std::cell::RefCell::new({
        let enabled = std::env::var("LING_PROFILE").map(|v| v != "0" && !v.is_empty()).unwrap_or(false);
        let every = std::env::var("LING_PROFILE_EVERY").ok()
            .and_then(|v| v.parse::<u64>().ok()).filter(|&n| n > 0).unwrap_or(240);
        if enabled {
            eprintln!("[ling-profile] ON — report every {every} frames (set LING_PROFILE_EVERY to change)");
        }
        LingProfileState { enabled, every, frames: 0, calls: std::collections::HashMap::new() }
    });
}

#[inline]
fn ling_profile_enabled() -> bool {
    LING_PROFILE.with(|p| p.borrow().enabled)
}

thread_local! {
    static LING_FPS: std::cell::RefCell<(bool, f64, u32, f64)> = std::cell::RefCell::new(
        (std::env::var("LING_FPS").map(|v| v != "0" && !v.is_empty()).unwrap_or(false), 0.0, 0, 0.0)
    );
}

#[cfg(not(target_arch = "wasm32"))]
fn ling_fps_tick() {
    LING_FPS.with(|s| {
        let mut s = s.borrow_mut();
        if !s.0 {
            return;
        }
        let now = crate::runtime::now_secs();
        if s.1 > 0.0 {
            s.3 += now - s.1;
            s.2 += 1;
            if s.2 >= 120 {
                let avg = s.3 / s.2 as f64;
                eprintln!(
                    "[fps] {:.1} fps  ({:.2} ms/frame, wall, {} frames)",
                    1.0 / avg,
                    avg * 1000.0,
                    s.2
                );
                s.2 = 0;
                s.3 = 0.0;
            }
        }
        s.1 = now;
    });
}

// Coarse render-pipeline timers (set LING_PHASE=1). Each accumulates wall-time
// per frame at flush/present granularity, so the cost is negligible. Reports the
// software-rasteriser breakdown the builtin profiler can't separate (the work is
// all inside the `present`/`flush_3d` builtins).
thread_local! {
    static LING_PHASE: std::cell::RefCell<(bool, u64, [u128; 5])> = std::cell::RefCell::new(
        (std::env::var_os("LING_PHASE").is_some(), 0, [0; 5])
    );
}

/// Phase indices for [`ling_phase_add`].
pub mod phase {
    pub const FLUSH: usize = 0;
    pub const TOON: usize = 1;
    pub const BLIT: usize = 2;
    pub const DISTORT: usize = 3;
    pub const SORT: usize = 4;
}

#[cfg(not(target_arch = "wasm32"))]
#[inline]
pub fn ling_phase_add(idx: usize, nanos: u128) {
    LING_PHASE.with(|p| {
        let mut p = p.borrow_mut();
        if p.0 {
            p.2[idx] += nanos;
        }
    });
}

#[cfg(not(target_arch = "wasm32"))]
fn ling_phase_frame() {
    LING_PHASE.with(|p| {
        let mut p = p.borrow_mut();
        if !p.0 {
            return;
        }
        p.1 += 1;
        if p.1 >= 120 {
            let f = p.1 as f64;
            let ms = |i: usize| p.2[i] as f64 / 1e6 / f;
            eprintln!(
                "[phase] sort={:.2} flush={:.2} toon={:.2} blit={:.2} distort={:.2} ms/frame",
                ms(phase::SORT),
                ms(phase::FLUSH),
                ms(phase::TOON),
                ms(phase::BLIT),
                ms(phase::DISTORT)
            );
            p.1 = 0;
            p.2 = [0; 5];
        }
    });
}

fn ling_profile_record(name: &str, nanos: u128) {
    // Frame boundary = a present() call.
    let is_frame = matches!(
        name,
        "present" | "แสดงผล" | "gfx_present" | "show" | "显" | "呈现" | "表示" | "표시"
    );
    LING_PROFILE.with(|p| {
        let mut p = p.borrow_mut();
        let e = p.calls.entry(name.to_string()).or_insert((0, 0));
        e.0 += 1;
        e.1 += nanos;
        if is_frame {
            p.frames += 1;
            if p.frames % p.every == 0 {
                ling_profile_print(&p);
            }
        }
    });
}

fn ling_profile_print(p: &LingProfileState) {
    let mut rows: Vec<(&String, u64, u128)> =
        p.calls.iter().map(|(n, (c, ns))| (n, *c, *ns)).collect();
    use std::cmp::Reverse;
    rows.sort_by_key(|x| Reverse(x.2)); // by total time desc
    let total_ns: u128 = p.calls.values().map(|(_, ns)| *ns).sum();
    let total_calls: u64 = p.calls.values().map(|(c, _)| *c).sum();
    let fr = p.frames.max(1) as f64;
    eprintln!(
        "\n┌─ LING PROFILE ── frames={} ─ builtin calls by total inclusive time ─────────────",
        p.frames
    );
    eprintln!(
        "│ {:<24} {:>9} {:>9} {:>10} {:>9} {:>6}",
        "builtin", "calls", "calls/fr", "total_ms", "ms/frame", "%time"
    );
    eprintln!("├──────────────────────────────────────────────────────────────────────────────");
    for (name, count, ns) in rows.iter().take(30) {
        let ms = *ns as f64 / 1e6;
        let pct = if total_ns > 0 {
            *ns as f64 / total_ns as f64 * 100.0
        } else {
            0.0
        };
        eprintln!(
            "│ {:<24} {:>9} {:>9.1} {:>10.1} {:>9.3} {:>5.1}%",
            truncate_name(name),
            count,
            *count as f64 / fr,
            ms,
            ms / fr,
            pct
        );
    }
    eprintln!("├──────────────────────────────────────────────────────────────────────────────");
    eprintln!(
        "│ TOTAL {} builtin calls, {:.1} ms over {} frames  →  {:.0} calls/frame, {:.2} ms/frame in builtins",
        total_calls,
        total_ns as f64 / 1e6,
        p.frames,
        total_calls as f64 / fr,
        total_ns as f64 / 1e6 / fr
    );
    eprintln!("└──────────────────────────────────────────────────────────────────────────────");
}

/// Trim a builtin name to fit the report column (counts chars, good enough for
/// the mixed-script names).
fn truncate_name(s: &str) -> String {
    let max = 24;
    if s.chars().count() <= max {
        s.to_string()
    } else {
        let mut t: String = s.chars().take(max - 1).collect();
        t.push('…');
        t
    }
}
