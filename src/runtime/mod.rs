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
    let has_sab = js_sys::Reflect::has(&global, &wasm_bindgen::JsValue::from_str("SharedArrayBuffer"))
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
fn wasm_fetch_sync(path: &str, response_type: &str, return_expr: &str) -> Result<wasm_bindgen::JsValue, String> {
    let quoted = js_sys::JSON::stringify(&wasm_bindgen::JsValue::from_str(path))
        .ok()
        .and_then(|s| s.as_string())
        .unwrap_or_else(|| "\"\"".to_string());

    let script = format!(
        "(function(){{\n  var xhr = new XMLHttpRequest();\n  xhr.open('GET', {quoted}, false);\n  xhr.responseType = '{response_type}';\n  xhr.send(null);\n  if ((xhr.status|0) !== 200 && (xhr.status|0) !== 0) {{ throw new Error('HTTP ' + xhr.status + ' for ' + {quoted}); }}\n  return {return_expr};\n}})()"
    );

    js_sys::eval(&script)
        .map_err(|e| e.as_string().unwrap_or_else(|| format!("JS eval failed: {:?}", e)))
}

#[cfg(target_arch = "wasm32")]
fn wasm_fetch_bytes(path: &str) -> Result<Vec<u8>, String> {
    let value = wasm_fetch_sync(path, "arraybuffer", "new Uint8Array(xhr.response || new ArrayBuffer(0))")?;
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
#[cfg(target_arch = "wasm32")]
use js_sys;
use crate::gfx::{GfxState, Light};
use crate::parser::ast::*;
use std::cell::RefCell;
use std::collections::HashMap;
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
    List(Vec<Value>),
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

type Env = HashMap<String, Value>;

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
        "audio_blip" | "提示音" | "ビープ音" | "효과음" | "เสียงบี๊บ"
        | "ui_sound" | "界面音" | "UI音" | "인터페이스음" | "เสียงปุ่ม"
        | "audio_stop_sfx" | "停止音效" | "効果音停止" | "효과음정지" | "หยุดเอฟเฟกต์ทั้งหมด" => {
            Value::Unit
        },

        // music loading / analysis / playback / midi (native-only today)
        "music_load" | "载入音乐" | "音楽読込" | "음악로드" | "โหลดเพลง"
        | "music_patch" | "乐器音色" | "音色読込" | "악기패치" | "แพตช์เครื่องดนตรี"
        | "music_lrc" | "载入歌词" | "歌詞読込" | "가사로드" | "โหลดเนื้อเพลง"
        | "music_midi_load" | "载入MIDI" | "MIDI読込" | "미디로드" | "โหลดมิดี" => {
            Value::Number(-1.0)
        },

        "music_duration" | "音乐时长" | "音楽長さ" | "음악길이" | "ความยาวเพลง"
        | "music_bpm" | "节拍速度" | "テンポ" | "템포" | "จังหวะต่อนาที"
        | "music_pos" | "音乐位置" | "音楽位置" | "음악위치" | "ตำแหน่งเพลง"
        | "music_mic_pitch" | "麦克风音高" | "マイク音程" | "마이크음정" | "ระดับเสียงไมค์"
        | "music_hz" | "音符频率" | "音符周波数" | "음표주파수" | "ความถี่โน้ต"
        | "music_pitch_score" | "音准评分" | "音程スコア" | "음정점수" | "คะแนนเสียง"
        | "music_judge" | "判定" | "判定する" | "판정" | "ตัดสินจังหวะ"
        | "music_midi_count" | "MIDI数量" | "MIDI数" | "미디수" | "จำนวนมิดี" => {
            Value::Number(0.0)
        },

        "music_key" | "调性" | "調性" | "조성" | "คีย์เพลง"
        | "music_lyric" | "当前歌词" | "現在歌詞" | "현재가사" | "เนื้อเพลงปัจจุบัน"
        | "music_note_name" | "音名" | "音名称" | "음이름" | "ชื่อโน้ต"
        | "music_grade_name" | "判定名" | "判定名称" | "판정이름" | "ชื่อการตัดสิน" => {
            Value::Str(String::new())
        },

        "music_onsets" | "音符起点" | "オンセット" | "온셋" | "จุดเริ่มเสียง"
        | "music_beat_grid" | "节拍网格" | "ビートグリッド" | "비트그리드" | "กริดจังหวะ"
        | "music_midi_notes" | "MIDI音符" | "MIDIノート" | "미디음표" | "โน้ตมิดี"
        | "music_midi_bars" | "MIDI音条" | "MIDIバー" | "미디바" | "แท่งมิดี"
        | "music_fft" | "音乐频谱" | "音楽スペクトル" | "음악스펙트럼" | "สเปกตรัมเพลง" => {
            Value::List(Vec::new())
        },

        "music_play" | "播放音乐" | "音楽再生" | "음악재생" | "เล่นเพลง"
        | "music_pause" | "暂停音乐" | "音楽一時停止" | "음악일시정지" | "หยุดเพลงชั่วคราว"
        | "music_stop" | "停止音乐" | "音楽停止" | "음악정지" | "หยุดเพลง"
        | "music_seek" | "定位音乐" | "音楽シーク" | "음악탐색" | "ค้นหาเพลง"
        | "music_volume" | "音乐音量" | "音楽音量" | "음악음량" | "ระดับเพลง"
        | "music_note" | "弹音符" | "音符演奏" | "음표연주" | "เล่นโน้ต"
        | "music_note_on" | "音符开始" | "音符オン" | "음표켜기" | "โน้ตเริ่ม"
        | "music_note_off" | "音符结束" | "音符オフ" | "음표끄기" | "โน้ตจบ" => {
            Value::Unit
        },

        // liquid sim — return handle 0 for new, Unit for everything else
        "liquid_new" | "新建液体" | "液体新規" | "액체생성" | "สร้างของเหลว" => {
            Value::Number(0.0)
        },
        "liquid_mix" | "液体混合" | "液体混合度" | "액체혼합" | "การผสมของเหลว" => {
            Value::Number(0.0)
        },
        "liquid_set_colors" | "液体颜色" | "液体配色" | "액체색상" | "สีของเหลว"
        | "liquid_splat" | "液体注入" | "液体追加" | "액체분사" | "หยดของเหลว"
        | "liquid_gravity" | "液体重力" | "液体重力ベクトル" | "액체중력" | "แรงโน้มถ่วงเหลว"
        | "liquid_step" | "液体步进" | "液体更新" | "액체스텝" | "ก้าวของเหลว"
        | "liquid_step_all" | "液体全步进" | "液体全更新" | "전체액체스텝" | "ก้าวของเหลวทั้งหมด"
        | "liquid_rainbow" | "液体彩虹" | "液体虹" | "액체무지개" | "ของเหลวสายรุ้ง"
        | "liquid_draw" | "绘制液体" | "液体描画" | "액체그리기" | "วาดของเหลว"
        | "liquid_draw_surface" | "液体贴面" | "液体曲面" | "액체곡면" | "ของเหลวบนพื้นผิว" => {
            Value::Unit
        },

        // ── game AI: neural networks ─────────────────────────────────────────
        "nn_new" | "建神经网" | "ニューラル作成" | "신경망생성" | "สร้างโครงข่าย"
        | "nn_load" | "载入网" | "網読込" | "신경망불러오기" | "โหลดโครงข่าย" => {
            Value::Number(-1.0)
        },
        "nn_forward" | "神经前向" | "順伝播" | "순전파" | "ส่งต่อโครงข่าย" => {
            Value::List(Vec::new())
        },
        "nn_train" | "训练网" | "ニューラル学習" | "신경망학습" | "ฝึกโครงข่าย"
        | "nn_dense" | "密集层" | "密層追加" | "밀집층" | "ชั้นหนาแน่น" => {
            Value::Number(0.0)
        },
        "nn_save" | "保存网" | "網保存" | "신경망저장" | "บันทึกโครงข่าย" => {
            Value::Bool(false)
        },

        // ── game AI: behavior trees ─────────────────────────────────────────
        "bt_build" | "建行为树" | "行動木構築" | "행동트리구성" | "สร้างต้นไม้พฤติกรรม" => {
            Value::Number(-1.0)
        },
        "bt_tick" | "行为树滴答" | "行動木更新" | "행동트리틱" | "เดินต้นไม้พฤติกรรม" => {
            Value::Str(String::new())
        },
        "bt_status" | "行为树状态" | "行動木状態" | "행동트리상태" | "สถานะต้นไม้พฤติกรรม" => {
            Value::Number(0.0)
        },
        "bt_set" | "设事实" | "事実設定" | "사실설정" | "ตั้งข้อเท็จจริง" => Value::Unit,

        // ── game AI: dialog LLM ─────────────────────────────────────────────
        "dialog_new" | "建对话模型" | "対話モデル作成" | "대화모델생성" | "สร้างโมเดลสนทนา"
        | "dialog_load_model" | "对话载模" | "対話モデル読込" | "대화모델불러오기"
            | "โหลดโมเดลสนทนา"
        | "dialog_train" | "对话训练" | "対話訓練" | "대화훈련" | "ฝึกสนทนา"
        | "dialog_load" | "对话载入" | "対話読込" | "대화불러오기" | "โหลดชุดสนทนา" => {
            Value::Number(-1.0)
        },
        "dialog_say" | "对话生成" | "対話生成" | "대화생성" | "พูดสนทนา" => {
            Value::Str(String::new())
        },
        "dialog_save" | "对话存模" | "対話モデル保存" | "대화모델저장" | "บันทึกโมเดลสนทนา" => {
            Value::Bool(false)
        },
        "dialog_learn" | "对话学习" | "対話学習" | "대화학습" | "เรียนรู้สนทนา" => Value::Unit,

        // ── networking ──────────────────────────────────────────────────────
        "net_connect" | "联网" | "ネット接続" | "네트연결" | "เชื่อมเน็ต"
        | "net_listen" | "监听" | "待機" | "리슨" | "รอรับ"
        | "net_send" | "发送" | "送信" | "전송" | "ส่ง" => Value::Number(-1.0),
        "net_recv" | "接收" | "受信" | "수신" | "รับ"
        | "net_status" | "连接状态" | "接続状態" | "연결상태" | "สถานะการเชื่อม" => {
            Value::Str(String::new())
        },
        "net_discover" | "发现" | "探索" | "검색" | "ค้นหาเครือข่าย" => {
            Value::List(Vec::new())
        },
        "net_close" | "断开" | "切断" | "연결종료" | "ตัดเชื่อม"
        | "net_test" | "测连接" | "接続テスト" | "연결테스트" | "ทดสอบเน็ต" => {
            Value::Number(0.0)
        },

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
fn key_char(k: minifb::Key) -> Option<char> {
    use minifb::Key::*;
    Some(match k {
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
        Period => '.',
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
    functions: HashMap<String, FnDef>,
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
}

/// Live gamepad input state: a ling-input hub fed by the native `gilrs` backend.
#[cfg(not(target_arch = "wasm32"))]
struct InputState {
    sensorium: ling_input::Sensorium,
    backend: ling_input::backend::GilrsBackend,
}

impl Interpreter {
    pub fn new() -> Self {
        #[cfg(not(target_arch = "wasm32"))]
        let audio = AudioEngine::new()
            .map_err(|e| eprintln!("audio init failed (no sound): {e}"))
            .ok();
        Self {
            globals: HashMap::new(),
            global_seed: HashMap::new(),
            functions: HashMap::new(),
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
            text_buffer: String::new(),
            record_n: 0,
            #[cfg(not(target_arch = "wasm32"))]
            mic_buffer: Vec::new(),
            #[cfg(not(target_arch = "wasm32"))]
            fonts: Vec::new(),
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
            dialog: None,
            dialog_colors: [0xE6F2FF, 0xFFD24A, 0x4AD2FF, 0x6CFF8C], // text · name · place · item
            frames: Vec::new(),
            error_trace: None,
            #[cfg(not(target_arch = "wasm32"))]
            input: RefCell::new(None),
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
            "music_load" | "载入音乐" | "音楽読込" | "음악로드" | "โหลดเพลง" => {
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
            "music_duration" | "音乐时长" | "音楽長さ" | "음악길이" | "ความยาวเพลง" => {
                let id = self.arg_num(args, 0, 0.0)? as i64;
                let d = self
                    .tracks
                    .get(id as usize)
                    .map(|t| t.duration)
                    .unwrap_or(0.0);
                return Ok(Some(Value::Number(d as f64)));
            },
            "music_bpm" | "节拍速度" | "テンポ" | "템포" | "จังหวะต่อนาที" => {
                let id = self.arg_num(args, 0, 0.0)? as i64;
                let b = self
                    .tracks
                    .get(id as usize)
                    .map(|t| ling_music::analysis::bpm(&t.mono, t.rate))
                    .unwrap_or(0.0);
                return Ok(Some(Value::Number(b as f64)));
            },
            "music_key" | "调性" | "調性" | "조성" | "คีย์เพลง" => {
                let id = self.arg_num(args, 0, 0.0)? as i64;
                let k = self
                    .tracks
                    .get(id as usize)
                    .map(|t| ling_music::analysis::key_name(&t.mono, t.rate))
                    .unwrap_or_default();
                return Ok(Some(Value::Str(k)));
            },
            "music_onsets" | "音符起点" | "オンセット" | "온셋" | "จุดเริ่มเสียง" => {
                let id = self.arg_num(args, 0, 0.0)? as i64;
                let v = self
                    .tracks
                    .get(id as usize)
                    .map(|t| ling_music::analysis::onsets(&t.mono, t.rate))
                    .unwrap_or_default();
                return Ok(Some(Value::List(
                    v.into_iter().map(|x| Value::Number(x as f64)).collect(),
                )));
            },
            "music_beat_grid" | "节拍网格" | "ビートグリッド" | "비트그리드" | "กริดจังหวะ" => {
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
                    beats.into_iter().map(|x| Value::Number(x as f64)).collect(),
                )));
            },
            "music_lrc" | "载入歌词" | "歌詞読込" | "가사로드" | "โหลดเนื้อเพลง" => {
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
            "music_lyric" | "当前歌词" | "現在歌詞" | "현재가사" | "เนื้อเพลงปัจจุบัน" => {
                let id = self.arg_num(args, 0, 0.0)? as i64;
                let t = self.arg_num(args, 1, 0.0)? as f32;
                let line = self
                    .lyrics
                    .get(id as usize)
                    .map(|l| l.line_at(t).to_string())
                    .unwrap_or_default();
                return Ok(Some(Value::Str(line)));
            },
            "music_midi_load" | "载入MIDI" | "MIDI読込" | "미디로드" | "โหลดมิดี" => {
                let path = self.arg_str(args, 0, "");
                let resolved = self.wasm_resolve_source_path(&path);
                match wasm_fetch_bytes(&resolved)
                    .and_then(|bytes| ling_music::midi::from_bytes(&bytes).map_err(|e| e.to_string()))
                {
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
            "music_midi_count" | "MIDI数量" | "MIDI数" | "미디수" | "จำนวนมิดี" => {
                let id = self.arg_num(args, 0, 0.0)? as i64;
                let n = self
                    .midis
                    .get(id as usize)
                    .map(|m| m.notes.len())
                    .unwrap_or(0);
                return Ok(Some(Value::Number(n as f64)));
            },
            "music_midi_notes" | "MIDI音符" | "MIDIノート" | "미디음표" | "โน้ตมิดี" => {
                let id = self.arg_num(args, 0, 0.0)? as i64;
                let mut out = Vec::new();
                if let Some(m) = self.midis.get(id as usize) {
                    for n in &m.notes {
                        out.push(Value::Number(n.time as f64));
                        out.push(Value::Number(n.midi as f64));
                    }
                }
                return Ok(Some(Value::List(out)));
            },
            "music_midi_bars" | "MIDI音条" | "MIDIバー" | "미디바" | "แท่งมิดี" => {
                let id = self.arg_num(args, 0, 0.0)? as i64;
                let mut out = Vec::new();
                if let Some(m) = self.midis.get(id as usize) {
                    for n in &m.notes {
                        out.push(Value::Number(n.time as f64));
                        out.push(Value::Number(n.midi as f64));
                        out.push(Value::Number(n.dur as f64));
                    }
                }
                return Ok(Some(Value::List(out)));
            },
            "music_judge" | "判定" | "判定する" | "판정" | "ตัดสินจังหวะ" => {
                let delta_ms = self.arg_num(args, 0, 9999.0)? as f32;
                return Ok(Some(Value::Number(
                    ling_music::Grade::judge(delta_ms).index() as f64,
                )));
            },
            "music_grade_name" | "判定名" | "判定名称" | "판정이름" | "ชื่อการตัดสิน" => {
                let idx = self.arg_num(args, 0, 4.0)? as i32;
                return Ok(Some(Value::Str(
                    ling_music::Grade::from_index(idx).name().to_string(),
                )));
            },
            "music_note_name" | "音名" | "音名称" | "음이름" | "ชื่อโน้ต" => {
                let hz = self.arg_num(args, 0, 0.0)? as f32;
                return Ok(Some(Value::Str(ling_music::note::hz_to_name(hz))));
            },
            "music_hz" | "音符频率" | "音符周波数" | "음표주파수" | "ความถี่โน้ต" => {
                let midi = match args.get(0) {
                    Some(Value::Str(s)) => ling_music::note::parse_pitch(s).unwrap_or(69),
                    Some(Value::Number(n)) => *n as i32,
                    _ => 69,
                };
                return Ok(Some(Value::Number(
                    ling_music::note::midi_to_hz(midi as f32) as f64,
                )));
            },
            "music_pitch_score" | "音准评分" | "音程スコア" | "음정점수" | "คะแนนเสียง" => {
                let hz = self.arg_num(args, 0, 0.0)? as f32;
                let target = self.arg_num(args, 1, 0.0)? as f32;
                return Ok(Some(Value::Number(
                    ling_music::karaoke::pitch_score(hz, target) as f64,
                )));
            },

            // ── Playback ──────────────────────────────────────────────────────
            "music_play" | "播放音乐" | "音楽再生" | "음악재생" | "เล่นเพลง" => {
                let id = self.arg_num(args, 0, 0.0)? as usize;
                if let Some(t) = self.tracks.get(id) {
                    crate::gfx::audio_web::play_music(id, &t.stereo, t.channels, t.rate, 1.0);
                }
                return Ok(Some(Value::Unit));
            },
            "music_pause" | "暂停音乐" | "音楽一時停止" | "음악일시정지" | "หยุดเพลงชั่วคราว"
            | "music_stop" | "停止音乐" | "音楽停止" | "음악정지" | "หยุดเพลง" => {
                let id = self.arg_num(args, 0, 0.0)? as usize;
                crate::gfx::audio_web::stop_music(id);
                return Ok(Some(Value::Unit));
            },
            "music_seek" | "定位音乐" | "音楽シーク" | "음악탐색" | "ค้นหาเพลง" => {
                // Seek is not straightforward on AudioBufferSourceNode; no-op for now.
                return Ok(Some(Value::Unit));
            },
            "music_pos" | "音乐位置" | "音楽位置" | "음악위치" | "ตำแหน่งเพลง" => {
                return Ok(Some(Value::Number(
                    crate::gfx::audio_web::current_music_position(),
                )));
            },
            "music_volume" | "音乐音量" | "音楽音量" | "음악음량" | "ระดับเพลง" => {
                let vol = self.arg_num(args, 0, 0.8)? as f32;
                // Apply to the most-recently started slot (slot 0 is typical).
                crate::gfx::audio_web::set_music_volume(0, vol);
                return Ok(Some(Value::Unit));
            },

            // ── FFT bands at current playback position ─────────────────────
            "music_fft" | "音乐频谱" | "音楽スペクトル" | "음악스펙트럼" | "สเปกตรัมเพลง" => {
                let id = self.arg_num(args, 0, 0.0)? as usize;
                let nbands = self.arg_num(args, 1, 16.0)? as usize;
                let pos = crate::gfx::audio_web::current_music_position() as f32;
                let bands = if let Some(t) = self.tracks.get(id) {
                    ling_music::analysis::fft_bands_at_pos(&t.mono, t.rate, pos, nbands)
                } else {
                    vec![0.0f32; nbands]
                };
                return Ok(Some(Value::List(
                    bands.into_iter().map(|x| Value::Number(x as f64)).collect(),
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

    pub fn run_program(&mut self, program: &Program) -> Result<(), String> {
        for item in &program.items {
            self.register_item("", item)?;
        }
        let entry = self
            .find_entry()
            .ok_or("no entry point — need `bind start = do {...}` or `ผูก เริ่ม = ทำ {...}`")?;
        // Seed the entry env with non-Do globals so top-level `令` bindings
        // are visible in the main Do block (same two-pass logic as call_named).
        let mut env = Env::new();
        let non_do: Vec<_> = self
            .globals
            .iter()
            .filter(|(_, e)| !matches!(e, Expr::Do(_)))
            .map(|(k, e)| (k.clone(), e.clone()))
            .collect();
        let mut pending: Vec<(String, Expr)> = Vec::new();
        for (k, expr) in &non_do {
            let mut tmp = Env::new();
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
        // Cache the evaluated globals so every user-function call can clone this
        // seed instead of re-evaluating all globals (see call_named).
        self.global_seed = env.clone();
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
                self.functions.insert(key, def.clone());
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
                Ok(Value::List(vs))
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
                Ok(Value::List(
                    (lo_n..hi_n).map(|i| Value::Number(i as f64)).collect(),
                ))
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
        }
    }

    // ─── Block execution ─────────────────────────────────────────────────────

    fn exec_block(&mut self, stmts: &[Stmt], env: &mut Env) -> Result<Option<Value>, EvalErr> {
        let mut last: Option<Value> = None;
        for stmt in stmts {
            match stmt {
                Stmt::Bind(name, expr) => {
                    let v = self.eval_expr(expr, env)?;
                    env.insert(name.clone(), v);
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

    // ─── Dispatch helpers ─────────────────────────────────────────────────────

    fn lookup(&self, name: &str, env: &Env) -> EvalResult {
        if let Some(v) = env.get(name) {
            return Ok(v.clone());
        }
        if self.functions.contains_key(name) {
            let def = &self.functions[name];
            return Ok(Value::Fn(def.params.clone(), def.body.clone(), Env::new()));
        }
        // Bare nullary enum variant used as a value (e.g. `bind p = Origin`).
        if let Some((enum_name, 0)) = self.enum_variants.get(name).cloned() {
            let variant = name.rsplit("::").next().unwrap_or(name).to_string();
            return Ok(Value::Variant { enum_name, variant, payload: Vec::new() });
        }
        // Math constants usable as plain identifiers (e.g. `sin(pi)`)
        match name {
            "pi" | "π" | "พาย" | "圆周率" | "円周率" | "파이" => {
                return Ok(Value::Number(std::f64::consts::PI))
            },
            "tau" | "τ" | "双周率" | "タウ" | "타우" | "ทาว" => {
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
        ling_profile_record(name, ((crate::runtime::now_secs() - t0) * 1_000_000_000.0) as u128);
        r
    }

    fn call_named_inner(&mut self, name: &str, args: Vec<Value>, env: &Env) -> EvalResult {
        // A user-defined function shadows any builtin of the same name, matching
        // the JIT/AOT backends (which always resolve a defined function first).
        if let Some(def) = self.functions.get(name).cloned() {
            let mut call_env = self.global_seed.clone();
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
            // ── Print ──
            "print" | "println" | "印" | "打印" | "印刷" | "พิมพ์" | "출력" | "вывести"
            | "imprimir" | "afficher" => {
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
            "print_color" | "พิมพ์สี" => {
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
            | "formater" => {
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
            "ok" | "好" | "良し" | "좋아" | "โอเค" => {
                let val = args.into_iter().next().unwrap_or(Value::Unit);
                return Ok(Value::Ok(Box::new(val)));
            },
            "bad" | "坏" | "err" | "悪い" | "나쁨" | "ผิด" => {
                let val = args.into_iter().next().unwrap_or(Value::Unit);
                return Ok(Value::Err(Box::new(val)));
            },
            // ── Vec constructors ──
            "向量::从" | "Vec::from" => {
                if let Some(Value::List(v)) = args.first() {
                    return Ok(Value::List(v.clone()));
                }
                return Ok(Value::List(args));
            },
            "向量::有容量" | "Vec::with_capacity" => return Ok(Value::List(Vec::new())),
            // ── Timer stubs ──
            "计时::获取当前小时" | "Timer::hour" => return Ok(Value::Number(14.0)),
            "计时::现在" | "Timer::now" => return Ok(Value::Number(1000.0)),
            // ── Sleep ──
            "sleep" | "หยุด" | "นอน" | "sleep_ms" | "睡眠" | "眠る" | "スリープ" | "잠자기"
            | "잠" | "流水::睡眠" | "Flow::sleep" => {
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
            "sin" | "ไซน์" | "正弦" | "サイン" | "사인" => {
                return Ok(Value::Number(self.arg_num(&args, 0, 0.0)?.sin()));
            },
            "cos" | "โคไซน์" | "余弦" | "コサイン" | "코사인" => {
                return Ok(Value::Number(self.arg_num(&args, 0, 0.0)?.cos()));
            },

            // ── Hyperbolic functions ──
            // Hyperbolic tangent
            "tanh" | "tanhf" | "双曲正切" | "双曲線正接" | "쌍곡탄젠트" => {
                return Ok(Value::Number(self.arg_num(&args, 0, 0.0)?.tanh()));
            },

            "tan" | "แทนเจนต์" | "正切" | "タンジェント" | "탄젠트" => {
                return Ok(Value::Number(self.arg_num(&args, 0, 0.0)?.tan()));
            },
            "asin" | "arcsin" | "反正弦" | "アークサイン" | "아크사인" | "อาร์กไซน์" =>
            {
                return Ok(Value::Number(self.arg_num(&args, 0, 0.0)?.asin()));
            },
            "acos" | "arccos" | "反余弦" | "アークコサイン" | "아크코사인" | "อาร์กโคไซน์" =>
            {
                return Ok(Value::Number(self.arg_num(&args, 0, 0.0)?.acos()));
            },
            "atan" | "arctan" | "反正切" | "アークタンジェント" | "아크탄젠트" | "อาร์กแทนเจนต์" =>
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
            "sqrt" | "รากที่สอง" | "平方根" | "根" | "제곱근" => {
                return Ok(Value::Number(self.arg_num(&args, 0, 0.0)?.sqrt()));
            },
            "cbrt" | "立方根" | "세제곱근" | "รากที่สาม" => {
                return Ok(Value::Number(self.arg_num(&args, 0, 0.0)?.cbrt()));
            },
            "pow" | "ยกกำลัง" | "幂" | "べき乗" | "거듭제곱" => {
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
            "ln" | "log" | "ลอการิทึม" | "对数" | "対数" | "로그" => {
                return Ok(Value::Number(self.arg_num(&args, 0, 1.0)?.ln()));
            },
            "log2" | "对数2" | "対数2" | "로그2" => {
                return Ok(Value::Number(self.arg_num(&args, 0, 1.0)?.log2()));
            },
            "log10" | "对数10" | "対数10" | "로그10" => {
                return Ok(Value::Number(self.arg_num(&args, 0, 1.0)?.log10()));
            },

            // ── Rounding / truncation ──
            "abs" | "ค่าสัมบูรณ์" | "绝对值" | "绝对" | "絶対値" | "절댓값" | "절대값" =>
            {
                return Ok(Value::Number(self.arg_num(&args, 0, 0.0)?.abs()));
            },
            "floor" | "ปัดลง" | "向下取整" | "下整" | "床関数" | "내림" => {
                return Ok(Value::Number(self.arg_num(&args, 0, 0.0)?.floor()));
            },
            "ceil" | "ปัดขึ้น" | "向上取整" | "上整" | "天井関数" | "올림" =>
            {
                return Ok(Value::Number(self.arg_num(&args, 0, 0.0)?.ceil()));
            },
            "round" | "ปัดเศษ" | "四舍五入" | "四舍" | "四捨五入" | "반올림" =>
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
            | "버림" => {
                return Ok(Value::Number(self.arg_num(&args, 0, 0.0)?.trunc()));
            },
            "fract" | "小数部分" | "小数部" | "소수부" => {
                return Ok(Value::Number(self.arg_num(&args, 0, 0.0)?.fract()));
            },

            // ── min / max / clamp ──
            "min" | "ต่ำสุด" | "最小" | "최솟값" => {
                let a = self.arg_num(&args, 0, 0.0)?;
                let b = self.arg_num(&args, 1, 0.0)?;
                return Ok(Value::Number(a.min(b)));
            },
            "max" | "สูงสุด" | "最大" | "최댓값" => {
                let a = self.arg_num(&args, 0, 0.0)?;
                let b = self.arg_num(&args, 1, 0.0)?;
                return Ok(Value::Number(a.max(b)));
            },
            "clamp" | "จำกัด" | "截取" | "範囲制限" | "범위제한" => {
                let x = self.arg_num(&args, 0, 0.0)?;
                let lo = self.arg_num(&args, 1, 0.0)?;
                let hi = self.arg_num(&args, 2, 1.0)?;
                return Ok(Value::Number(x.clamp(lo, hi)));
            },

            // ── Constants (also accessible as plain identifiers via lookup) ──
            "pi" | "π" | "พาย" | "圆周率" | "円周率" | "파이" => {
                return Ok(Value::Number(std::f64::consts::PI))
            },
            "tau" | "τ" | "双周率" | "タウ" | "타우" | "ทาว" => {
                return Ok(Value::Number(std::f64::consts::TAU))
            },

            // ══════════════════════════════════════════════════════════════════
            // PHASE 1: DMT TRIP CODER FEATURES
            // ══════════════════════════════════════════════════════════════════

            // ── Step 1: Noise Functions ──
            "vnoise" | "noise2" | "นอยส์2ดี" | "柏林噪声2D" | "バリューノイズ2D" | "값노이즈2D" =>
            {
                let x = self.arg_num(&args, 0, 0.0)? as f32;
                let y = self.arg_num(&args, 1, 0.0)? as f32;
                let seed = self.arg_num(&args, 2, 0.0)? as u32;
                return Ok(Value::Number(tex_vnoise(x, y, seed) as f64));
            },

            "fbm" | "นอยส์ออร์แกนิก" | "分形噪声" | "フラクタルノイズ" | "프랙탈노이즈" =>
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
            | "펄린노이즈3D" => {
                let x = self.arg_num(&args, 0, 0.0)? as f32;
                let y = self.arg_num(&args, 1, 0.0)? as f32;
                let z = self.arg_num(&args, 2, 0.0)? as f32;
                return Ok(Value::Number(perlin3(x, y, z) as f64));
            },

            // ── Step 2: Math Ergonomics ──
            "lerp" | "ค่าระหว่าง" | "线性插值" | "線形補間" | "선형보간" =>
            {
                let a = self.arg_num(&args, 0, 0.0)?;
                let b = self.arg_num(&args, 1, 1.0)?;
                let t = self.arg_num(&args, 2, 0.0)?;
                return Ok(Value::Number(a + (b - a) * t));
            },

            "smoothstep" | "เปลี่ยนแบบนุ่ม" | "平滑步进" | "スムーズステップ" | "스무스스텝" =>
            {
                let lo = self.arg_num(&args, 0, 0.0)?;
                let hi = self.arg_num(&args, 1, 1.0)?;
                let x = self.arg_num(&args, 2, 0.5)?;
                let t = ((x - lo) / (hi - lo)).clamp(0.0, 1.0);
                return Ok(Value::Number(t * t * (3.0 - 2.0 * t)));
            },

            "rand" | "สุ่ม" | "随机" | "乱数" | "난수" => {
                let val = fast_rand_f64(&mut self.rand_state);
                return Ok(Value::Number(val));
            },

            "sign" | "เครื่องหมาย" | "符号" | "符号関数" | "부호" => {
                let x = self.arg_num(&args, 0, 0.0)?;
                return Ok(Value::Number(x.signum()));
            },

            "hsv_to_rgb" | "เอชเอสวีเป็นRGB" | "HSV转RGB" | "HSV変換RGB" | "HSV변환RGB" =>
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
                return Ok(Value::List(vec![
                    Value::Number(r),
                    Value::Number(g),
                    Value::Number(b),
                ]));
            },

            "lerp_color" | "ไล่สี" | "颜色插值" | "色補間" | "색보간" => {
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
            "time_now" | "เวลาปัจจุบัน" | "当前时间" | "経過時間" | "현재시간" =>
            {
                return Ok(Value::Number(crate::runtime::now_secs() - self.start_time_secs));
            },

            // Wall-clock seconds since the Unix epoch (real date/time). Lets a
            // program defer deterministic-yet-evolving generation to the actual
            // datetime — same clock → same world, advancing as real time passes.
            "epoch_now" | "เวลาโลก" | "datetime" | "现在时刻" | "現在時刻" | "현재시각" =>
            {
                return Ok(Value::Number(crate::runtime::now_secs()));
            },

            "frame_count" | "เฟรม" | "帧数" | "フレーム数" | "프레임수" => {
                return Ok(Value::Number(self.frame_num as f64));
            },

            // ── Step 4: Microphone Input ──
            "mic_open" | "เปิดไมค์" | "开麦克风" | "マイク開く" | "마이크열기" =>
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

            "mic_rms" | "เสียงRMS" | "麦克风音量" | "マイクRMS" | "마이크RMS" =>
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

            "mic_peak" | "เสียงพีค" | "麦克风峰值" | "マイクピーク" | "마이크피크" =>
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

            "mic_fft" | "วิเคราะห์เสียงสด" | "实时频谱" | "リアルタイムFFT" | "실시간FFT" =>
            {
                #[cfg(not(target_arch = "wasm32"))]
                {
                    let n = self.arg_num(&args, 0, 8.0)? as usize;
                    if let Some(mic) = self.mic.as_ref() {
                        let samples = mic.latest_samples();
                        self.fft.borrow_mut().push_samples(&samples);
                    }
                    let bands = self.fft.borrow().freq_bands(n);
                    let result = bands.iter().map(|&v| Value::Number(v as f64)).collect();
                    return Ok(Value::List(result));
                }
                #[cfg(target_arch = "wasm32")]
                return Ok(Value::List(vec![]));
            },

            // ── Step 5: Additive Blend Mode ──
            "set_blend" | "โหมดผสม" | "混合模式" | "ブレンドモード" | "블렌드모드" =>
            {
                let mode = self.arg_num(&args, 0, 0.0)? as u8;
                let mut gfx = self.gfx.borrow_mut();
                gfx.blend = mode;
                let a = gfx.alpha;
                gfx.depth_queue.set_state(mode, a); // 3-D queue captures blend for subsequent pushes
                return Ok(Value::Unit);
            },

            // ── Step 6: Circle Primitives ──
            "draw_circle" | "วาดวงกลม" | "画圆" | "円描画" | "원그리기" =>
            {
                let cx = self.arg_num(&args, 0, 0.0)? as i32;
                let cy = self.arg_num(&args, 1, 0.0)? as i32;
                let r = self.arg_num(&args, 2, 10.0)? as i32;
                let mut gfx = self.gfx.borrow_mut();
                let (w, h, color, blend) =
                    (gfx.width as i32, gfx.height as i32, gfx.color, gfx.blend);
                draw_circle_outline(&mut gfx.buffer, w, h, cx, cy, r, color, blend);
                return Ok(Value::Unit);
            },

            "draw_filled_circle"
            | "draw_disc"
            | "วาดวงกลมทึบ"
            | "画实心圆"
            | "塗りつぶし円"
            | "원채우기" => {
                let cx = self.arg_num(&args, 0, 0.0)? as i32;
                let cy = self.arg_num(&args, 1, 0.0)? as i32;
                let r = self.arg_num(&args, 2, 10.0)? as i32;
                let mut gfx = self.gfx.borrow_mut();
                let (w, h, color, blend) =
                    (gfx.width as i32, gfx.height as i32, gfx.color, gfx.blend);
                draw_circle_filled(&mut gfx.buffer, w, h, cx, cy, r, color, blend);
                return Ok(Value::Unit);
            },

            // ── Step 7: Transparent fills, gradient surfaces & colored shadows ──
            // These all write straight into the software framebuffer (gfx.buffer)
            // on both native and web, so no target gating is needed.

            // set_alpha(a) — pen opacity 0..1 for the alpha-blended fills below.
            "set_alpha" | "ตั้งความโปร่งใส" | "设透明" | "アルファ設定" | "투명도설정" =>
            {
                let a = self.arg_num(&args, 0, 1.0)? as f32;
                let mut gfx = self.gfx.borrow_mut();
                gfx.alpha = a.clamp(0.0, 1.0);
                let (m, al) = (gfx.blend, gfx.alpha);
                gfx.depth_queue.set_state(m, al); // 3-D queue captures alpha for subsequent pushes
                return Ok(Value::Unit);
            },

            // set_color_space(mode) — 0 = legacy sRGB compositing (default),
            // 1 = gamma-correct linear-light compositing (blend in linear, store
            // sRGB) so alpha and gradients don't darken/shift hue.
            "set_color_space" | "ปริภูมิสี" | "色彩空间" | "色空間" | "색공간" =>
            {
                let m = self.arg_num(&args, 0, 0.0)? as i64;
                self.gfx.borrow_mut().linear_blend = m != 0;
                return Ok(Value::Unit);
            },

            // set_gradient_space(mode) — 1 = perceptual OkLab gradient interp
            // (default), 0 = legacy sRGB. Affects grad_triangle / grad_rect.
            "set_gradient_space" | "ปริภูมิไล่สี" | "渐变空间" | "グラデ空間" | "그라데이션공간" =>
            {
                let m = self.arg_num(&args, 0, 1.0)? as i64;
                self.gfx.borrow_mut().grad_oklab = m != 0;
                return Ok(Value::Unit);
            },

            // mix_color(r0,g0,b0, r1,g1,b1, t) — set the pen colour to the
            // perceptual OkLab blend of two colours (t in 0..1). Far nicer
            // mid-tones than a raw RGB lerp.
            "mix_color" | "ผสมสี" | "混合颜色" | "色混合" | "색혼합" => {
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
            "set_depth_test" | "ทดสอบความลึก" | "深度测试" | "深度テスト" | "깊이테스트" =>
            {
                let on = self.arg_num(&args, 0, 1.0)? as i64 != 0;
                self.gfx.borrow_mut().depth_test = on;
                return Ok(Value::Unit);
            },

            // set_flat_shade(on) / ตั้งแฟลตเชด — perf test: skip all per-triangle/mesh
            // lighting (compute_lit_color) and draw with the raw pen colour.
            "set_flat_shade" | "ตั้งแฟลตเชด" | "平面着色" | "フラット着色" | "평면음영" =>
            {
                let on = self.arg_num(&args, 0, 1.0)? as i64 != 0;
                self.gfx.borrow_mut().flat_shade = on;
                return Ok(Value::Unit);
            },

            // clear_depth() / ล้างความลึก — force the z-buffer to clear on the next
            // flush. `เติม` already does this; call explicitly to start a fresh
            // depth pass mid-frame (e.g. a separate overlay scene).
            "clear_depth" | "ล้างความลึก" | "清深度" | "深度クリア" | "깊이지우기" =>
            {
                self.gfx.borrow_mut().zbuf_needs_clear = true;
                return Ok(Value::Unit);
            },

            // depth_blur(focus, range, radius) / เบลอความลึก — depth-of-field post
            // pass over the framebuffer using the z-buffer: sharp at camera-space
            // depth `focus`, blurred up to `radius` px as depth departs by `range`.
            // Background (no geometry) blurs fully. Call AFTER `flush_3d` (so the
            // z-buffer is populated) and BEFORE `present`. Needs `set_depth_test(1)`.
            "depth_blur" | "เบลอความลึก" | "dof" | "depth_of_field" | "景深" =>
            {
                let focus = self.arg_num(&args, 0, 30.0)? as f32;
                let range = self.arg_num(&args, 1, 60.0)? as f32;
                let radius = self.arg_num(&args, 2, 3.0)?.max(0.0) as usize;
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
                    );
                }
                return Ok(Value::Unit);
            },

            // grad_triangle(x0,y0,r0,g0,b0, x1,y1,r1,g1,b1, x2,y2,r2,g2,b2)
            // Smooth per-vertex gradient triangle — a cheap lit surface: put the
            // bright colour on the vertex facing the light. Honours set_alpha.
            "grad_triangle" | "สามเหลี่ยมไล่สี" | "渐变三角" | "グラデ三角" | "그라데삼각" =>
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
            "grad_rect" | "สี่เหลี่ยมไล่สี" | "渐变矩形" | "グラデ矩形" | "그라데사각" =>
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
            "shadow_blob" | "เงาวงรี" | "阴影斑" | "影ブロブ" | "그림자블롭" =>
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
            "cast_shadow" | "ทอดเงา" | "投射阴影" | "影を落とす" | "그림자드리우기" =>
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
            "shadow_params" | "ตั้งค่าเงา" | "阴影参数" | "影設定" | "그림자설정" =>
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
            "depth_triangle" | "สามเหลี่ยมเรียงลึก" | "深度三角" | "深度三角形" | "깊이삼각" =>
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
            "depth_line" | "เส้นเรียงลึก" | "深度线" | "深度線" | "깊이선" =>
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
            "เปิดหน้าต่าง" | "open_window" | "gfx_window" | "开窗" | "ウィンドウ開く" | "창열기" =>
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
                    #[allow(deprecated)]
                    win.limit_update_rate(Some(std::time::Duration::from_millis(8)));
                    gfx.buffer = vec![0u32; w * h];
                    gfx.width = w;
                    gfx.height = h;
                    gfx.window = Some(win);
                    gfx.sync_projection();
                    hide_console_window();
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
            | "消去" | "지우기" => {
                let r = self.arg_num(&args, 0, 0.0)? as u32;
                let g = self.arg_num(&args, 1, 0.0)? as u32;
                let b = self.arg_num(&args, 2, 0.0)? as u32;
                #[cfg(not(target_arch = "wasm32"))]
                {
                    let c = (r << 16) | (g << 8) | b;
                    let mut gfx = self.gfx.borrow_mut();
                    gfx.buffer.fill(c);
                    gfx.zbuf_needs_clear = true; // clear color ⇒ clear depth next flush
                    gfx.edge_set.clear();         // reset shared-edge dedup for new frame
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
            "set_color_hsl" | "颜色HSL" | "色相" | "HSL色" | "HSL색설정" | "สีHSLวาด" =>
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
            "สีดินสอ" | "set_color" | "gfx_color" | "color" | "设色" | "色設定" | "색설정" =>
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
            | "삼각형그리기" => {
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
            "วาดเส้น" | "draw_line" | "gfx_line" | "line" | "画线" | "線描く" | "선그리기" =>
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
                    draw_line(&mut gfx.buffer, w, h, color, x0, y0, x1, y1);
                }
                #[cfg(target_arch = "wasm32")]
                gfx.depth_queue.push_line(0.0, color, x0, y0, x1, y1);
                return Ok(Value::Unit);
            },

            // ── วาดจุด(x, y) — plot a single pixel ──
            "วาดจุด" | "draw_pixel" | "gfx_pixel" | "pixel" | "画点" | "点描く" | "점그리기" =>
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
            "แสดงผล" | "present" | "gfx_present" | "show" | "显" | "呈现" | "表示" | "표시" =>
            {
                #[cfg(not(target_arch = "wasm32"))]
                {
                    // Flush depth queue and present — release borrow before reading mouse.
                    {
                        let mut gfx = self.gfx.borrow_mut();
                        if !gfx.depth_queue.is_empty() {
                            let w = gfx.width;
                            let h = gfx.height;
                            let dt = gfx.depth_test;
                            let reset_z = gfx.zbuf_needs_clear;
                            let (bm, ba) = (gfx.blend, gfx.alpha);
                            let queue = std::mem::take(&mut gfx.depth_queue);
                            {
                                let g = &mut *gfx;
                                let z = if dt { Some(&mut g.depth_buf) } else { None };
                                queue.flush(&mut g.buffer, z, reset_z, w, h);
                            }
                            gfx.depth_queue.set_state(bm, ba);
                            gfx.zbuf_needs_clear = false;
                        }
                        gfx.toon_post_process();
                        let buf = gfx.buffer.clone();
                        let w = gfx.width;
                        let h = gfx.height;
                        if let Some(win) = gfx.window.as_mut() {
                            win.update_with_buffer(&buf, w, h)
                                .map_err(|e| EvalErr::from(format!("present error: {e}")))?;
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
                            let queue = std::mem::take(&mut gfx.depth_queue);
                            {
                                let g = &mut *gfx;
                                let z = if dt { Some(&mut g.depth_buf) } else { None };
                                queue.flush(&mut g.buffer, z, reset_z, w, h);
                            }
                            gfx.zbuf_needs_clear = false;
                        }
                        gfx.toon_post_process();
                        crate::gfx::webgl::blit_rgb(&gfx.buffer, w, h);
                    }
                    self.wasm_pace_frame();
                }
                // Update the click-edge latch for interactive UI widgets.
                #[cfg(not(target_arch = "wasm32"))]
                {
                    let (_, _, down) = self.mouse_now();
                    self.mouse_was_down = down;
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
            | "전체화면" => {
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
                        .get(0)
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
                    // Drive the loop at the monitor's real refresh rate instead of
                    // a hard-coded cap, so a 144 Hz panel runs at 144 fps.
                    win.set_target_fps(monitor_info().2.max(30) as usize);
                    // Grab the native handle *before* moving the window into gfx.
                    #[cfg(windows)]
                    let hwnd = win.get_window_handle() as isize;
                    gfx.buffer = vec![0u32; w * h];
                    gfx.width = w;
                    gfx.height = h;
                    gfx.window = Some(win);
                    gfx.sync_projection();
                    // Strip all chrome and cover the full screen, above the taskbar.
                    #[cfg(windows)]
                    make_borderless_fullscreen(hwnd, w as i32, h as i32);
                    hide_console_window();
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
            "get_width" | "ความกว้าง" | "宽" | "幅取得" | "너비" => {
                return Ok(Value::Number(self.gfx.borrow().width as f64));
            },
            "get_height" | "ความสูง" | "高" | "高取得" | "높이" => {
                return Ok(Value::Number(self.gfx.borrow().height as f64));
            },

            // ── monitor detection: physical display, not the framebuffer ──────
            // monitor_width() → primary-monitor pixel width
            "monitor_width" | "screen_width" | "屏宽" | "画面幅" | "화면너비" | "ความกว้างจอ" =>
            {
                return Ok(Value::Number(monitor_info().0 as f64));
            },
            // monitor_height() → primary-monitor pixel height
            "monitor_height" | "screen_height" | "屏高" | "画面高" | "화면높이" | "ความสูงจอ" =>
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
            | "อัตรารีเฟรช" => {
                return Ok(Value::Number(monitor_info().2 as f64));
            },
            // monitor_info() → [width, height, refresh_hz]
            "monitor_info" | "screen_info" | "屏幕信息" | "画面情報" | "화면정보" | "ข้อมูลจอ" =>
            {
                let (w, h, hz) = monitor_info();
                return Ok(Value::List(vec![
                    Value::Number(w as f64),
                    Value::Number(h as f64),
                    Value::Number(hz as f64),
                ]));
            },
            // set_fps(n) → cap the render loop at n frames per second
            "set_fps"
            | "set_target_fps"
            | "target_fps"
            | "设帧率"
            | "フレームレート設定"
            | "프레임설정"
            | "ตั้งเฟรมเรต" => {
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

            // ── หน้าต่างเปิดอยู่() → bool — is the window still open? ──
            "หน้าต่างเปิดอยู่"
            | "window_is_open"
            | "gfx_is_open"
            | "is_open"
            | "窗开"
            | "開いている"
            | "창열림" => {
                #[cfg(not(target_arch = "wasm32"))]
                {
                    let gfx = self.gfx.borrow();
                    let open = gfx
                        .window
                        .as_ref()
                        .map(|w| w.is_open() && !w.is_key_down(minifb::Key::Escape))
                        .unwrap_or(false);
                    return Ok(Value::Bool(open));
                }
                #[cfg(target_arch = "wasm32")]
                return Ok(Value::Bool(true));
            },

            // ── key_down(name) → bool — is a key held? ──
            "key_down" | "กดค้าง" | "按键" | "キー押す" | "키누름" => {
                #[cfg(not(target_arch = "wasm32"))]
                {
                    let name = self.arg_str(&args, 0, "");
                    let gfx = self.gfx.borrow();
                    let down = gfx
                        .window
                        .as_ref()
                        .and_then(|w| str_to_minifb_key(&name).map(|k| w.is_key_down(k)))
                        .unwrap_or(false);
                    return Ok(Value::Bool(down));
                }
                #[cfg(target_arch = "wasm32")]
                return Ok(Value::Bool(false));
            },

            // ── key_pressed(name) → bool — was a key pressed this frame? ──
            "key_pressed" | "กดปุ่ม" | "键按" | "キー押した" | "키눌림" => {
                #[cfg(not(target_arch = "wasm32"))]
                {
                    let name = self.arg_str(&args, 0, "");
                    let pressed = {
                        let gfx = self.gfx.borrow();
                        gfx.window
                            .as_ref()
                            .and_then(|w| {
                                str_to_minifb_key(&name)
                                    .map(|k| w.is_key_pressed(k, minifb::KeyRepeat::No))
                            })
                            .unwrap_or(false)
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
            "mouse_dx" | "เมาส์X" | "鼠ΔX" | "マウスΔX" | "마우스ΔX" => {
                #[cfg(not(target_arch = "wasm32"))]
                return Ok(Value::Number(self.gfx.borrow().mouse_dx as f64));
                #[cfg(target_arch = "wasm32")]
                return Ok(Value::Number(0.0));
            },
            // ── mouse_scroll() → f64 — vertical scroll-wheel delta this frame ──
            #[cfg(not(target_arch = "wasm32"))]
            "mouse_scroll" | "ล้อเมาส์" | "滚轮" | "ホイール" | "스크롤" =>
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
            "mouse_scroll" | "ล้อเมาส์" | "滚轮" | "ホイール" | "스크롤" =>
            {
                return Ok(Value::Number(0.0));
            },
            "mouse_dy" | "เมาส์Y" | "鼠ΔY" | "マウスΔY" | "마우스ΔY" => {
                #[cfg(not(target_arch = "wasm32"))]
                return Ok(Value::Number(self.gfx.borrow().mouse_dy as f64));
                #[cfg(target_arch = "wasm32")]
                return Ok(Value::Number(0.0));
            },

            // ── Gamepad / joystick input (ling-input "Sensorium" + gilrs) ──
            // pad_poll() → number — advance input one frame; returns # connected pads.
            "pad_poll" | "手柄轮询" | "パッド更新" | "패드폴링" | "อัปเดตแพด" =>
            {
                #[cfg(not(target_arch = "wasm32"))]
                return Ok(Value::Number(self.pad_poll() as f64));
                #[cfg(target_arch = "wasm32")]
                return Ok(Value::Number(input_web::poll() as f64));
            },
            // pad_count() → number — connected gamepads.
            "pad_count" | "手柄数" | "パッド数" | "패드수" | "จำนวนแพด" =>
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
            "pad_connected" | "手柄连接" | "パッド接続" | "패드연결" | "แพดเชื่อม" =>
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
            "pad_button" | "手柄按键" | "パッドボタン" | "패드버튼" | "ปุ่มแพด" =>
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
            "pad_pressed" | "手柄按下" | "パッド押下" | "패드눌림" | "แพดกด" =>
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
            "pad_lx" | "手柄左X" | "パッド左X" | "패드왼X" | "แพดซ้ายX" => {
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
            "pad_ly" | "手柄左Y" | "パッド左Y" | "패드왼Y" | "แพดซ้ายY" => {
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
            "pad_rx" | "手柄右X" | "パッド右X" | "패드오X" | "แพดขวาX" => {
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
            "pad_ry" | "手柄右Y" | "パッド右Y" | "패드오Y" | "แพดขวาY" => {
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
            "pad_lt" | "手柄左扳机" | "パッド左トリガー" | "패드왼트리거" | "ไกแพดซ้าย" =>
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
            "pad_rt" | "手柄右扳机" | "パッド右トリガー" | "패드오트리거" | "ไกแพดขวา" =>
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
            "pad_rumble" | "手柄震动" | "パッド振動" | "패드진동" | "แพดสั่น" =>
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
            "set_camera_pos" | "ตั้งตำแหน่งกล้อง" | "镜坐标" | "カメラ座標" | "카메라좌표" =>
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
            "set_zdist" | "ตั้งระยะห่าง" | "镜距" | "Z距離設定" | "Z거리설정" =>
            {
                let d = self.arg_num(&args, 0, 5.0)? as f32;
                self.gfx.borrow_mut().camera.zdist = d;
                return Ok(Value::Unit);
            },

            // ── capture_mouse() — hide cursor and warp to centre each frame ──
            "capture_mouse" | "จับเมาส์" | "捕鼠" | "マウス捕捉" | "마우스잡기" =>
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

            // ══════════════════════════════════════════════════════════════════
            // 3-D / 4-D DRAWING — camera, lights, depth-sorted geometry
            // ══════════════════════════════════════════════════════════════════

            // ── set_camera(cry, sry, crx, srx) — store precomputed camera trig ──
            // Call once per frame after computing cos/sin of your rotation angles.
            "set_camera" | "ตั้งกล้อง" | "设镜" | "设置摄像机" | "カメラ設定" | "카메라설정" =>
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
            "set_projection" | "ตั้งโปรเจกชัน" | "投影" | "投影設定" | "투영설정" =>
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

            // ── draw_mesh(pos, idx, ox, oy, oz, scale, mode) ──
            //   Native batched triangle mesh. pos = flat [x,y,z,…], idx = flat tri indices.
            //   mode 0 = lit with current pen colour; 1 = per-face hue cycle.
            //   Vertices are batch-projected via ling-gpu (CPU fallback, or CUDA when the
            //   `cuda` feature is on); the per-triangle loop runs natively (not in the
            //   interpreter) so dense meshes (imported glTF, grids) stay fast.
            "draw_mesh" | "วาดเมช" => {
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
            "add_light" | "เพิ่มแสง" | "加灯" | "ライト追加" | "조명추가" =>
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
            "clear_lights" | "ล้างแสง" | "清灯" | "ライト消去" | "조명초기화" =>
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
            "set_material" | "ตั้งวัสดุ" | "设置材质" | "マテリアル設定" | "재질설정" =>
            {
                let key = self.arg_str(&args, 0, "");
                let val = self.arg_num(&args, 1, 0.0)?;
                let mut gfx = self.gfx.borrow_mut();
                let mat = gfx.material.get_or_insert_with(crate::gfx::LingMaterial::default);
                match key.as_str() {
                    "albedo"             => mat.albedo             = val as u32,
                    "roughness"          => mat.roughness          = val as f32,
                    "metallic"           => mat.metallic           = val as f32,
                    "emission"           => mat.emission           = val as u32,
                    "emission_strength"  => mat.emission_strength  = val as f32,
                    "specular"           => mat.specular           = val as f32,
                    "specular_tint"      => mat.specular_tint      = val as f32,
                    "subsurface"         => mat.subsurface         = val as f32,
                    "subsurface_color"   => mat.subsurface_color   = val as u32,
                    "clearcoat"          => mat.clearcoat          = val as f32,
                    "clearcoat_roughness"=> mat.clearcoat_roughness= val as f32,
                    "transmission"       => mat.transmission       = val as f32,
                    "ior"                => mat.ior                = val as f32,
                    "iridescence"        => mat.iridescence        = val as f32,
                    "sheen"              => mat.sheen              = val as f32,
                    "anisotropy"         => mat.anisotropy         = val as f32,
                    "anisotropy_angle"   => mat.anisotropy_angle   = val as f32,
                    "toon_bands"         => mat.toon_bands         = val as u32,
                    "shadow_softness"    => mat.shadow_softness    = val as f32,
                    "outline_px"         => mat.outline_px         = val as f32,
                    "outline_color"      => mat.outline_color      = val as u32,
                    "highlight_color"    => mat.highlight_color    = val as u32,
                    _ => {}
                }
                return Ok(Value::Unit);
            },

            // ── reset_material() — disable material override ──
            // After this call, draws use the legacy compute_lit_color_linear path.
            "reset_material" | "รีเซ็ตวัสดุ" | "重置材质" | "マテリアルリセット" | "재질초기화" =>
            {
                self.gfx.borrow_mut().material = None;
                return Ok(Value::Unit);
            },

            // ── toon_outlines(thickness, color, threshold) ──
            // Enable vector-smooth ink outlines on depth discontinuities.
            //   thickness  — ink-line half-width in pixels (0 = off, 1.5 = anime default)
            //   color      — 0xRRGGBB ink colour (default black = 0)
            //   threshold  — depth delta that triggers an edge (0.05 recommended)
            "toon_outlines" | "ตั้งเส้นขอบการ์ตูน" | "卡通轮廓" | "トゥーンアウトライン" | "툰아웃라인" =>
            {
                let px    = self.arg_num(&args, 0, 0.0)? as f32;
                let color = self.arg_num(&args, 1, 0.0)? as u32;
                let thresh= self.arg_num(&args, 2, 0.05)? as f32;
                let mut gfx = self.gfx.borrow_mut();
                gfx.toon.outline_px     = px;
                gfx.toon.outline_color  = color;
                gfx.toon.outline_thresh = thresh;
                return Ok(Value::Unit);
            },

            // ── tone_stop(t, value) ──
            // Add a stop to the tone ramp.
            //   t      — input luminance position [0..1]
            //   value  — output brightness [0..1]
            // Stops are automatically sorted; call tone_ramp_reset() first to clear.
            "tone_stop" | "ตั้งจุดโทน" | "色调停止" | "トーンストップ" | "톤스톱" =>
            {
                let t   = self.arg_num(&args, 0, 0.0)? as f32;
                let val = self.arg_num(&args, 1, 1.0)? as f32;
                let mut gfx = self.gfx.borrow_mut();
                gfx.toon.ramp.stops.push(crate::gfx::toon::ToneStop {
                    t:     t.clamp(0.0, 1.0),
                    value: val.clamp(0.0, 1.0),
                });
                gfx.toon.ramp.stops.sort_by(|a, b|
                    a.t.partial_cmp(&b.t).unwrap_or(std::cmp::Ordering::Equal));
                return Ok(Value::Unit);
            },

            // ── tone_smooth(enabled) ──
            // 0 = hard cel snap between stops (default); 1 = smooth gradient lerp.
            "tone_smooth" | "ตั้งโทนนุ่ม" | "色调平滑" | "トーンスムーズ" | "톤스무스" =>
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
            "tone_bezier" | "ตั้งโทนเบซิเยร์" | "色调贝塞尔" | "トーンベジェ" | "톤베지어" =>
            {
                let y1 = self.arg_num(&args, 0, 1.0/3.0)? as f32;
                let y2 = self.arg_num(&args, 1, 2.0/3.0)? as f32;
                self.gfx.borrow_mut().toon.ramp.bezier = Some([y1, y2]);
                return Ok(Value::Unit);
            },

            // ── tone_bezier_off() — disable Bézier remap ──
            "tone_bezier_off" | "ปิดโทนเบซิเยร์" | "关闭色调贝塞尔" | "トーンベジェオフ" | "톤베지어끄기" =>
            {
                self.gfx.borrow_mut().toon.ramp.bezier = None;
                return Ok(Value::Unit);
            },

            // ── tone_ramp_reset() — restore default 3-band cel ramp ──
            "tone_ramp_reset" | "รีเซ็ตการไล่โทน" | "重置色调渐变" | "トーンランプリセット" | "톤램프리셋" =>
            {
                self.gfx.borrow_mut().toon.ramp = crate::gfx::toon::ToneRamp::default();
                return Ok(Value::Unit);
            },

            // ── tone_ramp_clear() — clear all stops (build your own ramp) ──
            "tone_ramp_clear" | "ล้างการไล่โทน" | "清除色调渐变" | "トーンランプクリア" | "톤램프클리어" =>
            {
                self.gfx.borrow_mut().toon.ramp.stops.clear();
                return Ok(Value::Unit);
            },

            // ── shadow_smooth(softness) [compat] → tone_smooth + tone_bezier ──
            // Deprecated: use tone_smooth + tone_bezier instead.
            "shadow_smooth" | "ตั้งเงานุ่ม" | "柔化阴影" | "影ソフト" | "그림자부드럽게" =>
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
            "toon_highlight" | "ตั้งไฮไลท์การ์ตูน" | "卡通高光" | "トゥーンハイライト" | "툰하이라이트" =>
            {
                // Remap as a lit-band brightness boost: adds a stop near the highlight threshold.
                let _strength = self.arg_num(&args, 0, 0.0)? as f32;
                let _thresh   = self.arg_num(&args, 2, 0.78)? as f32;
                // No-op: configure via tone_stop() for precise control.
                return Ok(Value::Unit);
            },

            // ── set_ambient(v) — ambient light level [0..1] ──
            "set_ambient" | "ตั้งแสงรอบข้าง" | "环境光" | "環境光設定" | "환경광설정" =>
            {
                let v = self.arg_num(&args, 0, 0.15)? as f32;
                self.gfx.borrow_mut().ambient = v;
                return Ok(Value::Unit);
            },

            // ── set_fog(r,g,b, start, end) — distance fog toward (r,g,b).
            //    triangles/lines fade from `start`..`end` camera depth. end<=0 = off.
            "set_fog" | "ตั้งหมอก" | "雾" | "霧設定" | "안개설정" => {
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
            "วาดสามเหลี่ยม3มิติ" | "draw_triangle_3d" | "triangle3d" =>
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

                // World-space face normal  N = (B−A) × (C−A)
                let ux = bx - ax;
                let uy = by - ay;
                let uz = bz - az;
                let vx = cx - ax;
                let vy = cy - ay;
                let vz = cz - az;
                let normal = [uy * vz - uz * vy, uz * vx - ux * vz, ux * vy - uy * vx];

                // Per-vertex lit colours — gradient across face from light positions
                let (c0, c1, c2) = if gfx.flat_shade {
                    (gfx.color, gfx.color, gfx.color)
                } else {
                    crate::gfx::light::compute_lit_color_vertices(
                        gfx.color,
                        normal,
                        [ax, ay, az],
                        [bx, by, bz],
                        [cx, cy, cz],
                        &gfx.lights,
                        gfx.ambient,
                    )
                };

                // ── Near-plane CLIP (Sutherland–Hodgman) ──
                // A vertex just in front of the eye divides by ~0 in the perspective
                // projection and smears the triangle into a screen-filling fan. The old
                // "cull if any vertex past near" still let a vertex sitting just inside
                // near blow up. Clipping the triangle to the near plane keeps only the
                // in-front portion (finite projection — no fan, no over-cull).
                let near = -gfx.camera.zdist + 0.05;
                // (x, y, z, camera_depth, color)
                let vw = [
                    (ax, ay, az, gfx.camera.depth(ax, ay, az), c0),
                    (bx, by, bz, gfx.camera.depth(bx, by, bz), c1),
                    (cx, cy, cz, gfx.camera.depth(cx, cy, cz), c2),
                ];
                // Stack arrays (no per-triangle heap alloc): near-plane clip of a
                // triangle yields ≤4 vertices. This handler runs thousands of
                // times per frame, so avoiding two Vec allocations each call is a
                // real win across every render mode.
                // (x, y, z, color)
                let mut poly: [(f32, f32, f32, u32); 4] = [(0.0, 0.0, 0.0, 0); 4];
                let mut pn = 0usize;
                let mut ei = 0;
                while ei < 3 {
                    let a = vw[ei];
                    let b = vw[(ei + 1) % 3];
                    let ain = a.3 > near;
                    let bin = b.3 > near;
                    if ain && pn < 4 {
                        poly[pn] = (a.0, a.1, a.2, a.4);
                        pn += 1;
                    }
                    if ain != bin && pn < 4 {
                        let tt = (near - a.3) / (b.3 - a.3);
                        poly[pn] = (
                            a.0 + (b.0 - a.0) * tt,
                            a.1 + (b.1 - a.1) * tt,
                            a.2 + (b.2 - a.2) * tt,
                            crate::gfx::light::lerp_color(a.4, b.4, tt),
                        );
                        pn += 1;
                    }
                    ei += 1;
                }
                if pn < 3 {
                    return Ok(Value::Unit);
                }
                // Project the clipped polygon; apply fog per-vertex; fan-triangulate.
                // (sx, sy, sz, color)
                let mut proj: [(f32, f32, f32, u32); 4] = [(0.0, 0.0, 0.0, 0); 4];
                let mut pi = 0;
                while pi < pn {
                    let (sx, sy, sz) = gfx.camera.project(poly[pi].0, poly[pi].1, poly[pi].2);
                    let fc = gfx.fog_apply(poly[pi].3, sz);
                    proj[pi] = (sx, sy, sz, fc);
                    pi += 1;
                }
                let mut fk = 1;
                while fk + 1 < pn {
                    gfx.depth_queue.push_triangle_g_zv(
                        proj[0].0, proj[0].1, proj[0].2, proj[0].3,
                        proj[fk].0, proj[fk].1, proj[fk].2, proj[fk].3,
                        proj[fk + 1].0, proj[fk + 1].1, proj[fk + 1].2, proj[fk + 1].3,
                        3, // 3 bands matches cel_quantize (shadow/mid/lit)
                    );
                    fk += 1;
                }
                return Ok(Value::Unit);
            },

            // ── draw_quad_3d / draw_pent_3d / draw_hex_3d / draw_polygon_3d ──
            // Fan-triangulate convex n-gons.  Lighting, near-plane clip, fog, and
            // Gouraud shading all mirror draw_triangle_3d exactly.
            "draw_quad_3d" | "quad3d" | "วาดสี่เหลี่ยม3มิติ"
            | "draw_pent_3d" | "pent3d" | "วาดห้าเหลี่ยม3มิติ"
            | "draw_hex_3d"  | "hex3d"  | "วาดหกเหลี่ยม3มิติ"
            | "draw_polygon_3d" | "polygon3d" | "วาดรูปหลายเหลี่ยม3มิติ" =>
            {
                // Collect (wx, wy, wz) triples from args or list
                let mut wxs: [f32; 8] = [0.0; 8];
                let mut wys: [f32; 8] = [0.0; 8];
                let mut wzs: [f32; 8] = [0.0; 8];
                let n_verts;

                if args.len() == 1 {
                    // draw_polygon_3d([x0,y0,z0, x1,y1,z1, ...])
                    let list = match &args[0] {
                        Value::List(l) => l.clone(),
                        _ => return Err(EvalErr::from("draw_polygon_3d: expected list".to_string())),
                    };
                    let coords: Vec<f32> = list.iter()
                        .map(|v| match v { Value::Number(n) => *n as f32, _ => 0.0 })
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
                        wxs[i] = self.arg_num(&args, i * 3,     0.0)? as f32;
                        wys[i] = self.arg_num(&args, i * 3 + 1, 0.0)? as f32;
                        wzs[i] = self.arg_num(&args, i * 3 + 2, 0.0)? as f32;
                    }
                }
                if n_verts < 3 { return Ok(Value::Unit); }

                let mut gfx = self.gfx.borrow_mut();

                // Face normal from first triangle of the fan
                let normal = crate::gfx::poly::face_normal(
                    wxs[0], wys[0], wzs[0],
                    wxs[1], wys[1], wzs[1],
                    wxs[2], wys[2], wzs[2],
                );

                // Per-vertex lit colours
                let mut wcs: [u32; 8] = [0; 8];
                if gfx.flat_shade {
                    let c = gfx.color;
                    for i in 0..n_verts { wcs[i] = c; }
                } else if let Some(ref mat) = gfx.material.clone() {
                    let cam = [gfx.camera.cx, gfx.camera.cy, gfx.camera.zdist];
                    let lights: Vec<_> = gfx.lights.clone();
                    let ambient = gfx.ambient;
                    for i in 0..n_verts {
                        let v = [wxs[i], wys[i], wzs[i]];
                        let vd = [cam[0]-v[0], cam[1]-v[1], cam[2]-v[2]];
                        wcs[i] = crate::gfx::material::shade(mat, normal, vd, v, &lights, ambient);
                    }
                } else {
                    let base   = gfx.color;
                    let lights: Vec<_> = gfx.lights.clone();
                    let ambient = gfx.ambient;
                    for i in 0..n_verts {
                        wcs[i] = crate::gfx::light::compute_lit_color_linear(
                            base, normal, [wxs[i], wys[i], wzs[i]], &lights, ambient,
                        );
                    }
                }

                // Near-plane clip (Sutherland-Hodgman per vertex)
                let near = -gfx.camera.zdist + 0.05;
                let mut clip_in:  [(f32,f32,f32,f32,u32); crate::gfx::poly::MAX_CLIP_VERTS]
                    = [(0.0,0.0,0.0,0.0,0); crate::gfx::poly::MAX_CLIP_VERTS];
                for i in 0..n_verts {
                    let d = gfx.camera.depth(wxs[i], wys[i], wzs[i]);
                    clip_in[i] = (wxs[i], wys[i], wzs[i], d, wcs[i]);
                }
                let mut clip_out: [(f32,f32,f32,f32,u32); crate::gfx::poly::MAX_CLIP_VERTS]
                    = [(0.0,0.0,0.0,0.0,0); crate::gfx::poly::MAX_CLIP_VERTS];
                let pn = crate::gfx::poly::clip_near(&clip_in, n_verts, near, &mut clip_out);
                if pn < 3 { return Ok(Value::Unit); }

                // Project + fog
                let mut proj: [(f32,f32,f32,u32); crate::gfx::poly::MAX_CLIP_VERTS]
                    = [(0.0,0.0,0.0,0); crate::gfx::poly::MAX_CLIP_VERTS];
                for i in 0..pn {
                    let (sx, sy, sz) = gfx.camera.project(clip_out[i].0, clip_out[i].1, clip_out[i].2);
                    let fc = gfx.fog_apply(clip_out[i].4, sz);
                    proj[i] = (sx, sy, sz, fc);
                }

                // Fan-triangulate and push
                crate::gfx::poly::fan_emit_proj(&proj, pn, |x0,y0,z0,c0, x1,y1,z1,c1, x2,y2,z2,c2| {
                    gfx.depth_queue.push_triangle_g_zv(
                        x0,y0,z0,c0, x1,y1,z1,c1, x2,y2,z2,c2, 3,
                    );
                });
                return Ok(Value::Unit);
            },

            // ── วาดเส้น3มิติ(ax,ay,az, bx,by,bz) ──
            // Projects two world-space points via the stored camera and pushes
            // a line to the depth queue.
            "วาดเส้น3มิติ" | "draw_line_3d" | "line3d" | "画3D线" | "3D線描く" | "3D선그리기" =>
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
            "orb_shell" | "球壳" | "オーブ殻" | "오브껍질" | "เปลือกทรงกลม" =>
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
            "orb_particles" | "球内粒子" | "オーブ粒子" | "오브입자" | "อนุภาคทรงกลม" =>
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
            "project_3d" | "投影3D" | "3D投影" | "3D투영" | "ฉาย3มิติ" => {
                let x = self.arg_num(&args, 0, 0.0)? as f32;
                let y = self.arg_num(&args, 1, 0.0)? as f32;
                let z = self.arg_num(&args, 2, 0.0)? as f32;
                let gfx = self.gfx.borrow();
                let near = -gfx.camera.zdist + 0.05;
                let d = gfx.camera.depth(x, y, z);
                if d <= near {
                    return Ok(Value::List(vec![
                        Value::Number(-99999.0),
                        Value::Number(-99999.0),
                        Value::Number(d as f64),
                    ]));
                }
                let (sx, sy, depth) = gfx.camera.project(x, y, z);
                return Ok(Value::List(vec![
                    Value::Number(sx as f64),
                    Value::Number(sy as f64),
                    Value::Number(depth as f64),
                ]));
            },
            // draw_poly([x0,y0,x1,y1,…]) — filled 2-D polygon in the current colour,
            // honouring the blend mode (additive → translucent glow). Auto-closes.
            #[cfg(not(target_arch = "wasm32"))]
            "draw_poly" | "填充多边形" | "ポリゴン塗り" | "다각형채우기" | "เติมรูปหลายเหลี่ยม" =>
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
            "vtex_grid" | "ลายตาราง" | "纹格" | "格子模様" | "격자무늬" =>
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
            "vtex_rings" | "ลายวงซ้อน" | "纹环" | "同心円" | "동심원" => {
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
            "vtex_star" | "ลายดาว" | "纹星" | "星模様" | "별무늬" => {
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
            "vtex_spiral" | "ลายเกลียว" | "纹螺" | "螺旋" | "나선" => {
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
            "vtex_flower" | "ลายดอก" | "纹花" | "花模様" | "꽃무늬" => {
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
            "vtex_letter_rain" | "ลายอักษรไหล" | "纹字雨" | "文字雨" | "글자비" =>
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
            "vtex_hyperbolic_uv" | "ลายไฮเพอร์โบลิก" | "纹曲面" | "双曲線" | "쌍곡선" =>
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
            "vtex_halftone" | "ลายจุด" | "纹半调" | "網点模様" | "망점" => {
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
            "vtex_tessellated" | "ลายตาข่าย" | "纹镶嵌" | "網目模様" | "격자망" =>
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
            "vtex_lotus" | "ลายดอกบัว" | "纹莲" | "蓮模様" | "연꽃무늬" =>
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
            "vtex_chakra" | "ลายจักร" | "纹轮" | "輪模様" | "바퀴무늬" => {
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
            "vtex_yantra" | "ลายยันต์" | "纹咒" | "護符模様" | "부적무늬" =>
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
            "vtex_spiked_cog" | "ฟันเฟืองหนาม" | "纹棘轮" | "歯車模様" | "톱니바퀴" =>
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
            "vtex_torii" | "ประตูโทริอิ" | "纹鸟居" | "鳥居" | "도리이" =>
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
            "vtex_pagoda" | "เจดีย์" | "纹塔" | "塔" | "탑" => {
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
            | "공간음" => {
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
            "audio_listener" | "ผู้ฟัง" | "音频监听" | "音声リスナー" | "오디오리스너" =>
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
            "audio_bgm" | "เพลงพื้นหลัง" | "เพลงประกอบ" | "背景乐" | "BGM" | "배경음악" =>
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
            | "배경음악음량" => {
                let vol = self.arg_num(&args, 0, 0.5)? as f32;
                if let Some(audio) = &self.audio {
                    audio.set_bgm_volume(vol);
                }
                return Ok(Value::Unit);
            },

            #[cfg(not(target_arch = "wasm32"))]
            "audio_volume" | "ระดับเสียง" | "音量" | "음량" => {
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
            | "공간음" => {
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
            "audio_listener" | "ผู้ฟัง" | "音频监听" | "音声リスナー" | "오디오리스너" =>
            {
                let cry = self.arg_num(&args, 0, 1.0)? as f32;
                let sry = self.arg_num(&args, 1, 0.0)? as f32;
                let crx = self.arg_num(&args, 2, 1.0)? as f32;
                let srx = self.arg_num(&args, 3, 0.0)? as f32;
                crate::gfx::audio_web::set_listener(cry, sry, crx, srx);
                return Ok(Value::Unit);
            },

            #[cfg(target_arch = "wasm32")]
            "audio_bgm" | "เพลงพื้นหลัง" | "เพลงประกอบ" | "背景乐" | "BGM" | "배경음악" =>
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
            | "배경음악음량" => {
                let vol = self.arg_num(&args, 0, 0.5)? as f32;
                crate::gfx::audio_web::set_bgm_volume(vol);
                return Ok(Value::Unit);
            },

            #[cfg(target_arch = "wasm32")]
            "audio_volume" | "ระดับเสียง" | "音量" | "음량" => {
                let vol = self.arg_num(&args, 0, 0.7)? as f32;
                crate::gfx::audio_web::set_master_volume(vol);
                return Ok(Value::Unit);
            },

            // ── WASM sample load / play / stop / FX (Web Audio pool) ─────────
            #[cfg(target_arch = "wasm32")]
            "audio_sample_load" | "载入采样" | "サンプル読込" | "샘플로드" | "โหลดตัวอย่างเสียง" =>
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
            "audio_sample_play" | "播放采样" | "サンプル再生" | "샘플재생" | "เล่นตัวอย่างเสียง" =>
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
            "audio_sample_stop" | "停止采样" | "サンプル停止" | "샘플정지" | "หยุดตัวอย่างเสียง"
            | "audio_fx_reverb" | "混响" | "リバーブ" | "리버브" | "เสียงก้อง"
            | "audio_fx_delay" | "回声" | "ディレイ効果" | "딜레이" | "เสียงสะท้อน"
            | "audio_fx_lowpass" | "低通滤波" | "ローパス" | "저역통과" | "กรองความถี่ต่ำ" => {
                return Ok(Value::Unit);
            },

            // ── รอหน้าต่าง() — block until window closed / Escape ──
            "รอหน้าต่าง" | "wait_window" | "gfx_wait" => {
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
            "read_file" | "อ่านไฟล์" => {
                #[cfg(target_arch = "wasm32")]
                return Ok(Value::Str(String::new()));
                #[cfg(not(target_arch = "wasm32"))]
                {
                    let path = self.arg_str(&args, 0, "");
                    return std::fs::read_to_string(&path)
                        .map(Value::Str)
                        .map_err(|e| EvalErr::from(format!("read_file '{path}': {e}")));
                }
            },
            // ── networking (TCP, 2-peer co-op) ───────────────────────────────
            #[cfg(not(target_arch = "wasm32"))]
            "net_host" | "เน็ตโฮสต์" => {
                let port = self.arg_num(&args, 0, 7777.0)? as u16;
                net::host(port);
                return Ok(Value::Unit);
            },
            #[cfg(not(target_arch = "wasm32"))]
            "net_join" | "เน็ตจอย" => {
                let ip = self.arg_str(&args, 0, "127.0.0.1");
                let port = self.arg_num(&args, 1, 7777.0)? as u16;
                net::join(&ip, port);
                return Ok(Value::Unit);
            },
            #[cfg(not(target_arch = "wasm32"))]
            "net_send" | "เน็ตส่ง" => {
                let s = self.arg_str(&args, 0, "");
                net::send(&s);
                return Ok(Value::Unit);
            },
            #[cfg(not(target_arch = "wasm32"))]
            "net_recv" | "เน็ตรับ" => {
                return Ok(Value::Str(net::recv()));
            },
            #[cfg(not(target_arch = "wasm32"))]
            "net_status" | "เน็ตสถานะ" => {
                return Ok(Value::Number(net::status() as f64));
            },
            // ── LAN lobby discovery (UDP broadcast) ──
            #[cfg(not(target_arch = "wasm32"))]
            "net_announce" | "เน็ตประกาศ" => {
                let port = self.arg_num(&args, 0, 7778.0)? as u16;
                let info = self.arg_str(&args, 1, "");
                net::announce(port, &info);
                return Ok(Value::Unit);
            },
            #[cfg(not(target_arch = "wasm32"))]
            "net_announce_stop" | "เน็ตหยุดประกาศ" => {
                net::announce_stop();
                return Ok(Value::Unit);
            },
            #[cfg(not(target_arch = "wasm32"))]
            "net_discover" | "เน็ตค้นหา" => {
                let port = self.arg_num(&args, 0, 7778.0)? as u16;
                return Ok(Value::Str(net::discover(port)));
            },
            #[cfg(not(target_arch = "wasm32"))]
            "net_test" | "เน็ตทดสอบ" => {
                let port = self.arg_num(&args, 0, 7777.0)? as u16;
                return Ok(Value::Str(net::test_bind(port)));
            },
            // ── gamepad (gilrs) ──
            #[cfg(not(target_arch = "wasm32"))]
            "gamepad_poll" | "จอยโพล" => {
                gamepad::poll();
                return Ok(Value::Unit);
            },
            #[cfg(not(target_arch = "wasm32"))]
            "gamepad_button" | "จอยปุ่ม" => {
                let name = self.arg_str(&args, 0, "");
                return Ok(Value::Number(if gamepad::button(&name) {
                    1.0
                } else {
                    0.0
                }));
            },
            #[cfg(not(target_arch = "wasm32"))]
            "gamepad_axis" | "จอยแกน" => {
                let name = self.arg_str(&args, 0, "");
                return Ok(Value::Number(gamepad::axis(&name) as f64));
            },
            #[cfg(not(target_arch = "wasm32"))]
            "gamepad_rumble" | "จอยสั่น" => {
                let low = self.arg_num(&args, 0, 0.0)? as f32;
                let high = self.arg_num(&args, 1, 0.0)? as f32;
                let ms = self.arg_num(&args, 2, 200.0)? as u32;
                gamepad::rumble(low, high, ms);
                return Ok(Value::Unit);
            },
            #[cfg(not(target_arch = "wasm32"))]
            "gamepad_list" | "จอยรายการ" => {
                return Ok(Value::Str(gamepad::list()));
            },
            #[cfg(not(target_arch = "wasm32"))]
            "gamepad_any" | "จอยใดๆ" => {
                return Ok(Value::Number(if gamepad::any_button() { 1.0 } else { 0.0 }));
            },
            // wasm32: gamepad not available — return safe no-op values
            #[cfg(target_arch = "wasm32")]
            "gamepad_poll" | "จอยโพล"
            | "gamepad_rumble" | "จอยสั่น" => {
                return Ok(Value::Unit);
            },
            #[cfg(target_arch = "wasm32")]
            "gamepad_button" | "จอยปุ่ม"
            | "gamepad_axis" | "จอยแกน"
            | "gamepad_any" | "จอยใดๆ" => {
                return Ok(Value::Number(0.0));
            },
            #[cfg(target_arch = "wasm32")]
            "gamepad_list" | "จอยรายการ" => {
                return Ok(Value::Str(String::new()));
            },

            // ── game AI: neural networks ─────────────────────────────────────
            // nn_new(inputs[, seed]) → handle
            #[cfg(not(target_arch = "wasm32"))]
            "nn_new" | "建神经网" | "ニューラル作成" | "신경망생성" | "สร้างโครงข่าย" =>
            {
                let n_in = self.arg_num(&args, 0, 1.0)?.max(0.0) as usize;
                let seed = self.arg_num(&args, 1, 1.0)? as u64;
                return Ok(Value::Number(ai::nn_new(n_in, seed) as f64));
            },
            // nn_dense(handle, units[, activation]) — append a layer
            #[cfg(not(target_arch = "wasm32"))]
            "nn_dense" | "密集层" | "密層追加" | "밀집층" | "ชั้นหนาแน่น" =>
            {
                let id = self.arg_num(&args, 0, -1.0)? as i64;
                let units = self.arg_num(&args, 1, 1.0)?.max(1.0) as usize;
                let act = self.arg_str(&args, 2, "relu");
                ai::nn_dense(id, units, &act);
                return Ok(Value::Unit);
            },
            // nn_forward(handle, [inputs]) → [outputs]
            #[cfg(not(target_arch = "wasm32"))]
            "nn_forward" | "神经前向" | "順伝播" | "순전파" | "ส่งต่อโครงข่าย" =>
            {
                let id = self.arg_num(&args, 0, -1.0)? as i64;
                let input = self.arg_list_f32(&args, 1);
                let out = ai::nn_forward(id, &input);
                return Ok(Value::List(
                    out.into_iter().map(|v| Value::Number(v as f64)).collect(),
                ));
            },
            // nn_train(handle, [inputs], [targets][, lr]) → loss
            #[cfg(not(target_arch = "wasm32"))]
            "nn_train" | "训练网" | "ニューラル学習" | "신경망학습" | "ฝึกโครงข่าย" =>
            {
                let id = self.arg_num(&args, 0, -1.0)? as i64;
                let input = self.arg_list_f32(&args, 1);
                let target = self.arg_list_f32(&args, 2);
                let lr = self.arg_num(&args, 3, 0.01)? as f32;
                return Ok(Value::Number(ai::nn_train(id, &input, &target, lr) as f64));
            },
            // nn_save(handle, path) → bool
            #[cfg(not(target_arch = "wasm32"))]
            "nn_save" | "保存网" | "網保存" | "신경망저장" | "บันทึกโครงข่าย" =>
            {
                let id = self.arg_num(&args, 0, -1.0)? as i64;
                let path = self.arg_str(&args, 1, "model.lnn");
                return Ok(Value::Bool(ai::nn_save(id, &path)));
            },
            // nn_load(path) → handle (-1 on failure)
            #[cfg(not(target_arch = "wasm32"))]
            "nn_load" | "载入网" | "網読込" | "신경망불러오기" | "โหลดโครงข่าย" =>
            {
                let path = self.arg_str(&args, 0, "model.lnn");
                return Ok(Value::Number(ai::nn_load(&path) as f64));
            },

            // ── game AI: behavior trees ──────────────────────────────────────
            // bt_build(dsl_string) → handle
            #[cfg(not(target_arch = "wasm32"))]
            "bt_build" | "建行为树" | "行動木構築" | "행동트리구성" | "สร้างต้นไม้พฤติกรรม" =>
            {
                let spec = self.arg_str(&args, 0, "");
                return Ok(Value::Number(ai::bt_build(&spec) as f64));
            },
            // bt_set(handle, key, value) — set a blackboard fact
            #[cfg(not(target_arch = "wasm32"))]
            "bt_set" | "设事实" | "事実設定" | "사실설정" | "ตั้งข้อเท็จจริง" =>
            {
                let id = self.arg_num(&args, 0, -1.0)? as i64;
                let key = self.arg_str(&args, 1, "");
                let val = self.arg_num(&args, 2, 0.0)? as f32;
                ai::bt_set(id, &key, val);
                return Ok(Value::Unit);
            },
            // bt_tick(handle) → chosen action name ("" if none)
            #[cfg(not(target_arch = "wasm32"))]
            "bt_tick" | "行为树滴答" | "行動木更新" | "행동트리틱" | "เดินต้นไม้พฤติกรรม" =>
            {
                let id = self.arg_num(&args, 0, -1.0)? as i64;
                return Ok(Value::Str(ai::bt_tick(id)));
            },
            // bt_status(handle) → 0 fail / 1 success / 2 running
            #[cfg(not(target_arch = "wasm32"))]
            "bt_status" | "行为树状态" | "行動木状態" | "행동트리상태" | "สถานะต้นไม้พฤติกรรม" =>
            {
                let id = self.arg_num(&args, 0, -1.0)? as i64;
                return Ok(Value::Number(ai::bt_status(id) as f64));
            },

            // ── game AI: miniature dialog LLM ────────────────────────────────
            // dialog_new([ctx, embed, hidden, seed]) → handle
            #[cfg(not(target_arch = "wasm32"))]
            "dialog_new" | "建对话模型" | "対話モデル作成" | "대화모델생성" | "สร้างโมเดลสนทนา" =>
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
            "dialog_learn" | "对话学习" | "対話学習" | "대화학습" | "เรียนรู้สนทนา" =>
            {
                let id = self.arg_num(&args, 0, -1.0)? as i64;
                let text = self.arg_str(&args, 1, "");
                ai::dialog_learn(id, &text);
                return Ok(Value::Unit);
            },
            // dialog_load(handle, path) → lines added (-1 on error)
            #[cfg(not(target_arch = "wasm32"))]
            "dialog_load" | "对话载入" | "対話読込" | "대화불러오기" | "โหลดชุดสนทนา" =>
            {
                let id = self.arg_num(&args, 0, -1.0)? as i64;
                let path = self.arg_str(&args, 1, "");
                return Ok(Value::Number(ai::dialog_load(id, &path) as f64));
            },
            // dialog_train(handle[, epochs, lr]) → loss
            #[cfg(not(target_arch = "wasm32"))]
            "dialog_train" | "对话训练" | "対話訓練" | "대화훈련" | "ฝึกสนทนา" =>
            {
                let id = self.arg_num(&args, 0, -1.0)? as i64;
                let epochs = self.arg_num(&args, 1, 20.0)?.max(1.0) as usize;
                let lr = self.arg_num(&args, 2, 0.1)? as f32;
                return Ok(Value::Number(ai::dialog_train(id, epochs, lr) as f64));
            },
            // dialog_say(handle, prompt[, max_tokens, temperature]) → reply text
            #[cfg(not(target_arch = "wasm32"))]
            "dialog_say" | "对话生成" | "対話生成" | "대화생성" | "พูดสนทนา" =>
            {
                let id = self.arg_num(&args, 0, -1.0)? as i64;
                let prompt = self.arg_str(&args, 1, "");
                let max = self.arg_num(&args, 2, 24.0)?.max(1.0) as usize;
                let temp = self.arg_num(&args, 3, 0.8)? as f32;
                return Ok(Value::Str(ai::dialog_say(id, &prompt, max, temp)));
            },
            // dialog_save(handle, path) → bool
            #[cfg(not(target_arch = "wasm32"))]
            "dialog_save" | "对话存模" | "対話モデル保存" | "대화모델저장" | "บันทึกโมเดลสนทนา" =>
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
            | "โหลดโมเดลสนทนา" => {
                let path = self.arg_str(&args, 0, "model.llm");
                return Ok(Value::Number(ai::dialog_load_model(&path) as f64));
            },

            "write_file" | "เขียนไฟล์" => {
                #[cfg(target_arch = "wasm32")]
                return Ok(Value::Unit);
                #[cfg(not(target_arch = "wasm32"))]
                {
                    let path = self.arg_str(&args, 0, "");
                    let content = self.arg_str(&args, 1, "");
                    std::fs::write(&path, content.as_bytes())
                        .map_err(|e| EvalErr::from(format!("write_file '{path}': {e}")))?;
                    return Ok(Value::Unit);
                }
            },
            "print_file" | "พิมพ์ไฟล์" => {
                let content = self.arg_str(&args, 0, "");
                print!("{content}");
                return Ok(Value::Unit);
            },

            // ── CLI arguments ─────────────────────────────────────────────────
            "get_args" | "รับอาร์กิวเมนต์" => {
                let v: Vec<Value> = std::env::args().map(Value::Str).collect();
                return Ok(Value::List(v));
            },

            // ── String utilities ──────────────────────────────────────────────
            "split" | "str_split" | "แยก" => {
                let s = self.arg_str(&args, 0, "");
                let sep = self.arg_str(&args, 1, "\n");
                let sep = if sep.is_empty() { "\n".into() } else { sep };
                let parts: Vec<Value> = s
                    .split(sep.as_str())
                    .map(|p| Value::Str(p.to_string()))
                    .collect();
                return Ok(Value::List(parts));
            },
            "trim" | "str_trim" | "ตัดช่องว่าง" => {
                let s = self.arg_str(&args, 0, "");
                return Ok(Value::Str(s.trim().to_string()));
            },
            "starts_with" | "str_starts_with" | "เริ่มด้วย" => {
                let s = self.arg_str(&args, 0, "");
                let prefix = self.arg_str(&args, 1, "");
                return Ok(Value::Bool(s.starts_with(prefix.as_str())));
            },
            "ends_with" | "str_ends_with" | "ลงท้ายด้วย" => {
                let s = self.arg_str(&args, 0, "");
                let suffix = self.arg_str(&args, 1, "");
                return Ok(Value::Bool(s.ends_with(suffix.as_str())));
            },
            "str_replace" | "แทนสตริง" => {
                let s = self.arg_str(&args, 0, "");
                let from = self.arg_str(&args, 1, "");
                let to = self.arg_str(&args, 2, "");
                return Ok(Value::Str(s.replace(from.as_str(), to.as_str())));
            },
            "str_find" | "หาในสตริง" => {
                let s = self.arg_str(&args, 0, "");
                let needle = self.arg_str(&args, 1, "");
                // Return char index (not byte index) for consistency with substr
                let pos = s
                    .find(needle.as_str())
                    .map(|byte_i| s[..byte_i].chars().count() as f64)
                    .unwrap_or(-1.0);
                return Ok(Value::Number(pos));
            },
            "substr" | "str_slice" | "ส่วนสตริง" => {
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
            "to_str" | "str" | "num_str" | "แปลงสตริง" => {
                let v = args.into_iter().next().unwrap_or(Value::Unit);
                return Ok(Value::Str(v.to_string()));
            },
            "str_repeat" | "ทำซ้ำสตริง" => {
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
            "str_len" | "len" | "ความยาว" | "长度" | "長さ" | "길이" => {
                match args.first() {
                    Some(Value::Str(s)) => return Ok(Value::Number(s.chars().count() as f64)),
                    Some(Value::List(v)) => return Ok(Value::Number(v.len() as f64)),
                    _ => return Ok(Value::Number(0.0)),
                }
            },

            // ── FNV-1a hash (deterministic, normalized 0.0–1.0) ──────────────
            "hash_str" | "แฮช" => {
                let s = self.arg_str(&args, 0, "");
                let mut h: u64 = 14695981039346656037_u64;
                for b in s.bytes() {
                    h ^= b as u64;
                    h = h.wrapping_mul(1099511628211);
                }
                return Ok(Value::Number((h & 0xFFFFFF) as f64 / 16777215.0));
            },
            "hash_int" | "แฮชจำนวน" => {
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
            "list_new" | "รายการใหม่" | "新建列表" | "新規リスト" | "새목록" =>
            {
                return Ok(Value::List(Vec::new()));
            },
            "list_push" | "เพิ่มรายการ" | "列表添加" | "リスト追加" | "목록추가" =>
            {
                let lst = args.first().cloned().unwrap_or(Value::List(vec![]));
                let val = args.get(1).cloned().unwrap_or(Value::Unit);
                if let Value::List(mut v) = lst {
                    v.push(val);
                    return Ok(Value::List(v));
                }
                return Ok(Value::List(vec![val]));
            },
            "list_get" | "รับรายการ" | "取元素" | "要素取得" | "요소가져오기" =>
            {
                // Borrow the list; clone only the element (was cloning the whole list).
                let i = self.arg_num(&args, 1, 0.0)? as usize;
                if let Some(Value::List(v)) = args.first() {
                    return Ok(v.get(i).cloned().unwrap_or(Value::Str(String::new())));
                }
                return Ok(Value::Str(String::new()));
            },
            // list_set(lst, idx, val) → new list with index replaced. Engine builtin
            // (O(n) one copy) to replace the O(n²) ling `ตั้งรายการ` that looped
            // list_push + list_get (each of which copied the whole list).
            "list_set" | "ตั้งรายการ" | "设元素" | "要素設定" | "요소설정" =>
            {
                let idx = self.arg_num(&args, 1, 0.0)? as usize;
                let mut ai = args.into_iter();
                let lst = ai.next().unwrap_or(Value::List(vec![]));
                ai.next(); // skip idx
                let val = ai.next().unwrap_or(Value::Unit);
                if let Value::List(mut v) = lst {
                    if idx < v.len() {
                        v[idx] = val;
                    }
                    return Ok(Value::List(v));
                }
                return Ok(Value::List(vec![]));
            },
            "list_join" | "join" | "รวมรายการ" | "连接" | "連結" | "연결" =>
            {
                let lst = args.first().cloned().unwrap_or(Value::List(vec![]));
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
                        return Ok(Value::List(out));
                    },
                    Err(e) => {
                        eprintln!("blob decode failed: {e}");
                        return Ok(Value::List(vec![]));
                    },
                }
            },

            // ══════════════════════════════════════════════════════════════════
            // SVG EXPORT  (svg_begin / svg_rect / svg_circle / svg_line /
            //              svg_polyline / svg_text / svg_end / hsl_color)
            // Chinese aliases: 开始SVG 结束SVG SVG矩形 SVG圆形 SVG线段 SVG折线 SVG文本 HSL颜色
            // Thai aliases:    เริ่มSVG จบSVG SVGสี่เหลี่ยม SVGวงกลม SVGเส้น SVGเส้นหัก SVGข้อความ สีHSL
            // ══════════════════════════════════════════════════════════════════
            "svg_begin" | "开始SVG" | "เริ่มSVG" => {
                let path = self.arg_str(&args, 0, "output.svg");
                let width = self.arg_num(&args, 1, 800.0)?;
                let height = self.arg_num(&args, 2, 600.0)?;
                *self.svg.borrow_mut() = Some(SvgWriter::new(path, width, height));
                return Ok(Value::Unit);
            },

            "svg_rect" | "SVG矩形" | "SVGสี่เหลี่ยม" => {
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

            "svg_circle" | "SVG圆形" | "SVGวงกลม" => {
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

            "svg_line" | "SVG线段" | "SVGเส้น" => {
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

            "svg_polyline" | "SVG折线" | "SVGเส้นหัก" => {
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

            "svg_text" | "SVG文本" | "SVGข้อความ" => {
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

            "svg_end" | "结束SVG" | "จบSVG" => {
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

            "hsl_color" | "HSL颜色" | "สีHSL" => {
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
            "fft_push" | "วิเคราะห์เสียง" | "频谱输入" | "FFT入力" | "FFT입력" =>
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
            "fft_bands" | "แถบความถี่" | "频段" | "周波数帯" | "주파수대" =>
            {
                let n = self.arg_num(&args, 0, 32.0)? as usize;
                let bands = self.fft.borrow().freq_bands(n);
                *self.fft_bands_cache.borrow_mut() = bands.clone();
                return Ok(Value::List(
                    bands.into_iter().map(|v| Value::Number(v as f64)).collect(),
                ));
            },

            // fft_beat() → bool
            #[cfg(not(target_arch = "wasm32"))]
            "fft_beat" | "จังหวะเสียง" | "节拍检测" | "ビート検出" | "비트" =>
            {
                return Ok(Value::Bool(self.fft.borrow().is_beat()));
            },

            // fft_beat_ratio() → f64  (1.0 = at threshold, >1 = strong beat)
            #[cfg(not(target_arch = "wasm32"))]
            "fft_beat_ratio" | "อัตราจังหวะ" | "节拍比" | "ビート比" | "비트비율" =>
            {
                return Ok(Value::Number(self.fft.borrow().beat_ratio() as f64));
            },

            // fft_rms() → f64
            #[cfg(not(target_arch = "wasm32"))]
            "fft_rms" | "ระดับRMS" | "均方根" | "二乗平均" | "RMS레벨" => {
                return Ok(Value::Number(self.fft.borrow().rms() as f64));
            },

            // fft_dominant_freq() → f64  in Hz
            #[cfg(not(target_arch = "wasm32"))]
            "fft_dominant_freq" | "ความถี่หลัก" | "主频" | "主要周波数" | "주파수" =>
            {
                return Ok(Value::Number(self.fft.borrow().dominant_freq() as f64));
            },

            // ── wasm32 stubs: fft builtins are no-ops on web ───────────────
            #[cfg(target_arch = "wasm32")]
            "fft_push" | "วิเคราะห์เสียง" | "频谱输入" | "FFT入力" | "FFT입력" =>
            {
                return Ok(Value::Unit);
            },
            #[cfg(target_arch = "wasm32")]
            "fft_bands" | "แถบความถี่" | "频段" | "周波数帯" | "주파수대" =>
            {
                let n = self.arg_num(&args, 0, 32.0)? as usize;
                return Ok(Value::List(vec![Value::Number(0.0); n]));
            },
            #[cfg(target_arch = "wasm32")]
            "fft_beat" | "จังหวะเสียง" | "节拍检测" | "ビート検出" | "비트" =>
            {
                return Ok(Value::Bool(false));
            },
            #[cfg(target_arch = "wasm32")]
            "fft_beat_ratio" | "อัตราจังหวะ" | "节拍比" | "ビート比" | "비트비율" =>
            {
                return Ok(Value::Number(1.0));
            },
            #[cfg(target_arch = "wasm32")]
            "fft_rms" | "ระดับRMS" | "均方根" | "二乗平均" | "RMS레벨" => {
                return Ok(Value::Number(0.0));
            },
            #[cfg(target_arch = "wasm32")]
            "fft_dominant_freq" | "ความถี่หลัก" | "主频" | "主要周波数" | "주파수" =>
            {
                return Ok(Value::Number(0.0));
            },

            // ══════════════════════════════════════════════════════════════════
            // PROCEDURAL TEXTURE BLIT BUILTINS  (screen-space)
            // All: name(dst_x, dst_y, width, height, ...params, palette)
            // palette: "rainbow" | "fire" | "ocean" | "psychedelic" | "neon" | "forest"
            // ══════════════════════════════════════════════════════════════════

            // tex_checkerboard(x, y, w, h, tiles, r1,g1,b1, r2,g2,b2)
            "tex_checkerboard" | "ลายตารางหมากรุก" => {
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
            "tex_gradient" | "ลายไล่สี" => {
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
            "tex_noise" | "ลายนอยส์" => {
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
            "tex_freq_map" | "ลายความถี่" => {
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
            "tex_spiral" | "ลายเกลียวหมุน" => {
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
            "tex_ripple" | "ลายระลอก" => {
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
            "tex_mandelbrot" | "ลายแมนเดลบรอต" => {
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
            "tex_julia" | "ลายจูเลีย" => {
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
            "tex_voronoi" | "ลายโวโรนอย" => {
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
            "tex_halftone" | "ลายฮาล์ฟโทน" => {
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
            "set_shade_mode" | "设置着色" | "シェード設定" | "셰이드모드" | "ตั้งการแรเงา" =>
            {
                let m = self.arg_num(&args, 0, 2.0)? as u8;
                self.gfx.borrow_mut().shade_mode = m;
                return Ok(Value::Unit);
            },
            // set_cel_bands(n) — number of posterisation bands (>=2)
            "set_cel_bands" | "设置色阶" | "セル段数" | "셀밴드" | "ตั้งระดับสี" =>
            {
                let n = (self.arg_num(&args, 0, 4.0)? as u32).max(2);
                self.gfx.borrow_mut().shade.bands = n;
                return Ok(Value::Unit);
            },
            // set_shadow_color(r,g,b) — coloured-shadow tint, 0-255
            "set_shadow_color" | "设置阴影色" | "影の色" | "그림자색" | "ตั้งสีเงา" =>
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
            "crypto_hash" | "แฮชเข้ารหัส" | "几何哈希" | "幾何ハッシュ" | "기하해시" =>
            {
                let s = self.arg_str(&args, 0, "");
                return Ok(Value::Str(hex_encode(&ling_crypto::geo::holo_hash(
                    s.as_bytes(),
                ))));
            },
            #[cfg(target_arch = "wasm32")]
            "crypto_hash" | "แฮชเข้ารหัส" | "几何哈希" | "幾何ハッシュ" | "기하해시" =>
            {
                let s = self.arg_str(&args, 0, "");
                return Ok(Value::Str(hex_encode(&ling_crypto::geo::holo_hash(
                    s.as_bytes(),
                ))));
            },
            // 3-D torus-knot fingerprint of any text/key → flat [x,y,z, x,y,z, …]
            #[cfg(not(target_arch = "wasm32"))]
            "knot_points" | "จุดปม" | "结点坐标" | "結び目点" | "매듭점" => {
                let s = self.arg_str(&args, 0, "");
                let shape = ling_crypto::geo::KnotShape::from_bytes(s.as_bytes());
                let mut out = Vec::with_capacity(shape.points.len() * 3);
                for p in &shape.points {
                    out.push(Value::Number(p[0] as f64));
                    out.push(Value::Number(p[1] as f64));
                    out.push(Value::Number(p[2] as f64));
                }
                return Ok(Value::List(out));
            },
            #[cfg(target_arch = "wasm32")]
            "knot_points" | "จุดปม" | "结点坐标" | "結び目点" | "매듭점" => {
                let s = self.arg_str(&args, 0, "");
                let shape = ling_crypto::geo::KnotShape::from_bytes(s.as_bytes());
                let mut out = Vec::with_capacity(shape.points.len() * 3);
                for p in &shape.points {
                    out.push(Value::Number(p[0] as f64));
                    out.push(Value::Number(p[1] as f64));
                    out.push(Value::Number(p[2] as f64));
                }
                return Ok(Value::List(out));
            },
            #[cfg(not(target_arch = "wasm32"))]
            "knot_label" | "ป้ายปม" | "结点标签" | "結び目ラベル" | "매듭라벨" =>
            {
                let s = self.arg_str(&args, 0, "");
                return Ok(Value::Str(
                    ling_crypto::geo::KnotShape::from_bytes(s.as_bytes()).label(),
                ));
            },
            #[cfg(target_arch = "wasm32")]
            "knot_label" | "ป้ายปม" | "结点标签" | "結び目ラベル" | "매듭라벨" =>
            {
                let s = self.arg_str(&args, 0, "");
                return Ok(Value::Str(
                    ling_crypto::geo::KnotShape::from_bytes(s.as_bytes()).label(),
                ));
            },
            // KEM keypair (hybrid X25519+ML-KEM-768) → integer handle
            #[cfg(not(target_arch = "wasm32"))]
            "knot_keygen" | "hybrid_keygen" | "สร้างกุญแจปม" | "生成密钥" | "鍵生成" | "키생성" =>
            {
                self.crypto_ids.push(ling_crypto::KnotIdentity::generate());
                return Ok(Value::Number((self.crypto_ids.len() - 1) as f64));
            },
            #[cfg(not(target_arch = "wasm32"))]
            "knot_public" | "hybrid_public" | "กุญแจสาธารณะปม" | "公钥" | "公開鍵" | "공개키" =>
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
            | "캡슐화" => {
                let pk = hex_decode(&self.arg_str(&args, 0, ""));
                match ling_crypto::geo::knot_encapsulate(&pk) {
                    Ok((ct, ss)) => {
                        return Ok(Value::List(vec![
                            Value::Str(hex_encode(&ct)),
                            Value::Str(hex_encode(&ss)),
                        ]))
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
            | "캡슐해제" => {
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
            "crypto_seal" | "ผนึก" | "封印" | "封印する" | "봉인" => {
                let key = hex_to_32(&self.arg_str(&args, 0, ""));
                let pt = self.arg_str(&args, 1, "");
                match ling_crypto::geo::holo_seal(key, pt.as_bytes()) {
                    Ok(ct) => return Ok(Value::Str(hex_encode(&ct))),
                    Err(e) => return Ok(Value::Err(Box::new(Value::Str(e.to_string())))),
                }
            },
            #[cfg(not(target_arch = "wasm32"))]
            "crypto_open" | "เปิดผนึก" | "解封" | "封印解除" | "봉인해제" =>
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
            "holo_points" | "จุดโฮโลแกรม" | "全息点" | "ホログラム点" | "홀로그램점" =>
            {
                let s = self.arg_str(&args, 0, "");
                let frags = ling_crypto::geo::scatter(s.as_bytes());
                let mut out = Vec::with_capacity(frags.len() * 4);
                for f in &frags {
                    for c in f.coord {
                        out.push(Value::Number(c as f64));
                    }
                }
                return Ok(Value::List(out));
            },
            #[cfg(not(target_arch = "wasm32"))]
            "holo_fragment_count"
            | "จำนวนชิ้นโฮโลแกรม"
            | "全息碎片数"
            | "ホログラム断片数"
            | "홀로그램조각수" => {
                let s = self.arg_str(&args, 0, "");
                return Ok(Value::Number(
                    ling_crypto::geo::scatter(s.as_bytes()).len() as f64
                ));
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
            "tween" | "补间" | "補間" | "트윈" | "แทรกค่า" => {
                let a = self.arg_num(&args, 0, 0.0)?;
                let b = self.arg_num(&args, 1, 0.0)?;
                let t = self.arg_num(&args, 2, 0.0)?.clamp(0.0, 1.0);
                return Ok(Value::Number(a + (b - a) * t));
            },
            "tween_ease" | "缓动补间" | "緩和補間" | "이징트윈" | "แทรกนุ่ม" =>
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
            "breathe" | "呼吸" | "호흡" | "หายใจ" => {
                let t = self.arg_num(&args, 0, 0.0)? as f32;
                let rate = self.arg_num(&args, 1, 1.0)? as f32;
                let depth = self.arg_num(&args, 2, 0.1)? as f32;
                return Ok(Value::Number(
                    ling_animation::scalar::breathe(t, rate, depth) as f64,
                ));
            },
            "wobble" | "摆动" | "揺れ" | "흔들림" | "โยก" => {
                let t = self.arg_num(&args, 0, 0.0)? as f32;
                let freq = self.arg_num(&args, 1, 1.0)? as f32;
                let amp = self.arg_num(&args, 2, 1.0)? as f32;
                let phase = self.arg_num(&args, 3, 0.0)? as f32;
                return Ok(Value::Number(
                    ling_animation::scalar::wobble(t, freq, amp, phase) as f64,
                ));
            },
            "gait_phase" | "步相" | "歩相" | "걸음위상" | "เฟสก้าว" => {
                let t = self.arg_num(&args, 0, 0.0)? as f32;
                let speed = self.arg_num(&args, 1, 1.0)? as f32;
                return Ok(Value::Number(
                    ling_animation::scalar::gait_phase(t, speed) as f64
                ));
            },
            "gait_swing" | "步摆" | "歩振り" | "걸음흔들" | "ก้าวแกว่ง" =>
            {
                let t = self.arg_num(&args, 0, 0.0)? as f32;
                let speed = self.arg_num(&args, 1, 1.0)? as f32;
                let stride = self.arg_num(&args, 2, 1.0)? as f32;
                return Ok(Value::Number(
                    ling_animation::scalar::gait_swing(t, speed, stride) as f64,
                ));
            },
            "gait_lift" | "抬脚" | "足上げ" | "발들기" | "ยกเท้า" => {
                let t = self.arg_num(&args, 0, 0.0)? as f32;
                let speed = self.arg_num(&args, 1, 1.0)? as f32;
                let height = self.arg_num(&args, 2, 1.0)? as f32;
                return Ok(Value::Number(
                    ling_animation::scalar::gait_lift(t, speed, height) as f64,
                ));
            },
            "spring_to" | "弹向" | "バネ寄せ" | "스프링이동" | "สปริงไป" =>
            {
                let pos = self.arg_num(&args, 0, 0.0)? as f32;
                let vel = self.arg_num(&args, 1, 0.0)? as f32;
                let target = self.arg_num(&args, 2, 0.0)? as f32;
                let stiffness = self.arg_num(&args, 3, 120.0)? as f32;
                let damping = self.arg_num(&args, 4, 14.0)? as f32;
                let dt = self.arg_num(&args, 5, 1.0 / 60.0)? as f32;
                let (np, nv) =
                    ling_animation::scalar::spring_step(pos, vel, target, stiffness, damping, dt);
                return Ok(Value::List(vec![
                    Value::Number(np as f64),
                    Value::Number(nv as f64),
                ]));
            },
            "ik2" | "反解" | "逆運動" | "역운동" | "ไอเค2" => {
                let l1 = self.arg_num(&args, 0, 1.0)? as f32;
                let l2 = self.arg_num(&args, 1, 1.0)? as f32;
                let tx = self.arg_num(&args, 2, 0.0)? as f32;
                let ty = self.arg_num(&args, 3, 0.0)? as f32;
                let (sh, el) = ling_animation::scalar::two_bone_ik(l1, l2, tx, ty);
                return Ok(Value::List(vec![
                    Value::Number(sh as f64),
                    Value::Number(el as f64),
                ]));
            },
            // ── Mechanical 机 ──
            "gear_couple" | "齿轮联动" | "歯車連動" | "기어연동" | "เฟืองทด" =>
            {
                let angle = self.arg_num(&args, 0, 0.0)? as f32;
                let ti = self.arg_num(&args, 1, 1.0)? as f32;
                let to = self.arg_num(&args, 2, 1.0)? as f32;
                return Ok(Value::Number(
                    ling_animation::scalar::gear(angle, ti, to) as f64
                ));
            },
            "gear_train" | "齿轮组" | "歯車列" | "기어열" | "ชุดเฟือง" => {
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
                return Ok(Value::List(
                    out.into_iter().map(|a| Value::Number(a as f64)).collect(),
                ));
            },
            "cam_lift" | "凸轮升程" | "カム揚程" | "캠리프트" | "ยกลูกเบี้ยว" =>
            {
                let angle = self.arg_num(&args, 0, 0.0)? as f32;
                let lift = self.arg_num(&args, 1, 1.0)? as f32;
                return Ok(Value::Number(
                    ling_animation::scalar::cam_lift(angle, lift) as f64
                ));
            },
            "piston" | "活塞" | "ピストン" | "피스톤" | "ลูกสูบ" => {
                let angle = self.arg_num(&args, 0, 0.0)? as f32;
                let crank = self.arg_num(&args, 1, 1.0)? as f32;
                let rod = self.arg_num(&args, 2, 2.0)? as f32;
                return Ok(Value::Number(
                    ling_animation::scalar::piston(angle, crank, rod) as f64,
                ));
            },
            "rack" | "齿条" | "ラック" | "랙" | "แร็ค" => {
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
                return Ok(Value::Number(0.0));
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
                return Ok(Value::Number(0.0));
            },
            #[cfg(not(target_arch = "wasm32"))]
            "mouse_down" => {
                let gfx = self.gfx.borrow();
                let d = gfx
                    .window
                    .as_ref()
                    .map(|w| w.get_mouse_down(minifb::MouseButton::Left))
                    .unwrap_or(false);
                return Ok(Value::Bool(d));
            },
            #[cfg(target_arch = "wasm32")]
            "mouse_down" => {
                return Ok(Value::Bool(false));
            },
            #[cfg(not(target_arch = "wasm32"))]
            "mouse_down_right" | "เมาส์ขวา" => {
                let gfx = self.gfx.borrow();
                let d = gfx
                    .window
                    .as_ref()
                    .map(|w| w.get_mouse_down(minifb::MouseButton::Right))
                    .unwrap_or(false);
                return Ok(Value::Bool(d));
            },
            #[cfg(target_arch = "wasm32")]
            "mouse_down_right" | "เมาส์ขวา" => {
                return Ok(Value::Bool(false));
            },
            #[cfg(not(target_arch = "wasm32"))]
            "mouse_down_middle" | "เมาส์กลาง" => {
                let gfx = self.gfx.borrow();
                let d = gfx
                    .window
                    .as_ref()
                    .map(|w| w.get_mouse_down(minifb::MouseButton::Middle))
                    .unwrap_or(false);
                return Ok(Value::Bool(d));
            },
            #[cfg(target_arch = "wasm32")]
            "mouse_down_middle" | "เมาส์กลาง" => {
                return Ok(Value::Bool(false));
            },
            #[cfg(not(target_arch = "wasm32"))]
            "ui_hot" | "热区" | "ホットエリア" | "핫존" | "พื้นที่สัมผัส" =>
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
            "ui_hot" | "热区" | "ホットエリア" | "핫존" | "พื้นที่สัมผัส" =>
            {
                return Ok(Value::Bool(false));
            },
            // ui_text(x, y, scale, "string") — holographic vector text
            "ui_text" | "界面文字" | "UI文字" | "UI텍스트" | "ข้อความหน้าจอ" =>
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
            "font_load" | "โหลดฟอนต์" | "加载字体" | "フォント読込" | "글꼴로드" =>
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
            "font_load" | "โหลดฟอนต์" | "加载字体" | "フォント読込" | "글꼴로드" =>
            {
                // Web runtime does not load host TTF/OTF files yet.
                // Return -1 so scripts can fall back to ui_text.
                return Ok(Value::Number(-1.0));
            },
            // font_text(handle, x, y, px, "string") — anti-aliased *stroked* vector outline
            // in the current set_color / set_blend. (x,y) is the text box top-left.
            #[cfg(not(target_arch = "wasm32"))]
            "font_text" | "ข้อความฟอนต์" | "字体文本" | "フォント文字" | "글꼴텍스트" =>
            {
                let id = self.arg_num(&args, 0, 0.0)? as i64;
                let x = self.arg_num(&args, 1, 0.0)? as f32;
                let y = self.arg_num(&args, 2, 0.0)? as f32;
                let px = self.arg_num(&args, 3, 16.0)? as f32;
                let s = self.arg_str(&args, 4, "");
                if id >= 0 && (id as usize) < self.fonts.len() && px > 0.0 {
                    let strokes = self.font_layout_2d(id as usize, x, y, px, &s);
                    let mut gfx = self.gfx.borrow_mut();
                    let (w, h, color, add) = (gfx.width, gfx.height, gfx.color, gfx.blend == 1);
                    for pl in &strokes {
                        for seg in pl.windows(2) {
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
                        }
                    }
                }
                return Ok(Value::Unit);
            },
            #[cfg(target_arch = "wasm32")]
            "font_text" | "ข้อความฟอนต์" | "字体文本" | "フォント文字" | "글꼴텍스트" =>
            {
                return Ok(Value::Unit);
            },
            // font_text_fill(handle, x, y, px, "string") — anti-aliased *filled* vector glyphs.
            #[cfg(not(target_arch = "wasm32"))]
            "font_text_fill" | "เติมฟอนต์" | "填充字体" | "フォント塗り" | "글꼴채움" =>
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
                    let (w, h, color, add) = (gfx.width, gfx.height, gfx.color, gfx.blend == 1);
                    for contours in &glyphs {
                        crate::gfx::raster::fill_contours_aa(
                            &mut gfx.buffer,
                            w,
                            h,
                            color,
                            add,
                            contours,
                        );
                    }
                }
                return Ok(Value::Unit);
            },
            #[cfg(target_arch = "wasm32")]
            "font_text_fill" | "เติมฟอนต์" | "填充字体" | "フォント塗り" | "글꼴채움" =>
            {
                return Ok(Value::Unit);
            },
            // font_text_3d(handle, cx,cy,cz, ux,uy,uz, vx,vy,vz, size, "string")
            // — stroked vector text on a 3D plane: u = advance dir, v = up dir, size = world/em.
            //   Flows through the depth-sorted line pipeline, so it rotates with the camera (and 4D).
            #[cfg(not(target_arch = "wasm32"))]
            "font_text_3d" | "ข้อความฟอนต์3มิติ" | "字体3D" | "フォント3D" | "글꼴3D" =>
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
                if id >= 0 && (id as usize) < self.fonts.len() && size > 0.0 {
                    // Build world-space polylines: world = C + (pen+ex)*size*U + ey*size*V
                    let font = &mut self.fonts[id as usize];
                    let asc = font.ascent();
                    let mut pen = 0.0f32;
                    let mut lines: Vec<[f32; 6]> = Vec::new();
                    for ch in s.chars() {
                        let go = font.glyph_outline(ch, 0.01);
                        for pl in &go.polylines {
                            for seg in pl.windows(2) {
                                let map = |p: [f32; 2]| {
                                    let a = pen + p[0];
                                    let b = p[1] - asc; // shift so the top of the cap sits near C
                                    [
                                        cx + a * size * ux + b * size * vx,
                                        cy + a * size * uy + b * size * vy,
                                        cz + a * size * uz + b * size * vz,
                                    ]
                                };
                                let p0 = map(seg[0]);
                                let p1 = map(seg[1]);
                                lines.push([p0[0], p0[1], p0[2], p1[0], p1[1], p1[2]]);
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
            "font_text_3d" | "ข้อความฟอนต์3มิติ" | "字体3D" | "フォント3D" | "글꼴3D" =>
            {
                return Ok(Value::Unit);
            },
            // font_width(handle, px, "string") — pixel width of a string in a loaded font.
            #[cfg(not(target_arch = "wasm32"))]
            "font_width" | "ความกว้างฟอนต์" | "字体宽度" | "フォント幅" | "글꼴너비" =>
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
            "font_width" | "ความกว้างฟอนต์" | "字体宽度" | "フォント幅" | "글꼴너비" =>
            {
                return Ok(Value::Number(0.0));
            },
            // ui_frame(x,y,w,h, bracketLen) — sci-fi corner brackets
            "ui_frame" | "边框" | "フレーム枠" | "프레임틀" | "กรอบ" => {
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
            "ui_bevel" | "斜角框" | "ベベル枠" | "베벨틀" | "กรอบเฉียง" =>
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
            "ui_theme" | "界面主题" | "UIテーマ" | "인터페이스테마" | "ธีมส่วนติดต่อ" =>
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

            // ── HUD ──────────────────────────────────────────────────────────
            #[cfg(not(target_arch = "wasm32"))]
            "ui_radar" | "雷达" | "レーダー" | "레이더" | "เรดาร์" => {
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
            "ui_compass" | "罗盘" | "コンパス" | "나침반" | "เข็มทิศ" => {
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
            "ui_reticle" | "准星" | "照準" | "조준선" | "เป้าเล็ง" => {
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
            "ui_target" | "锁定框" | "ターゲット" | "표적" | "กรอบเป้า" =>
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
            "ui_panel" | "面板" | "パネル" | "패널" | "แผง" => {
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
            "ui_scanlines" | "扫描线" | "走査線" | "스캔라인" | "เส้นสแกน" =>
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
            "ui_bar" | "进度条" | "バー" | "막대" | "แถบ" => {
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
            "ui_segbar" | "分段条" | "分割バー" | "분할막대" | "แถบแบ่ง" =>
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
            "ui_gauge" | "仪表" | "ゲージ" | "게이지" | "มาตรวัด" => {
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
            "ui_ring" | "环表" | "リングメーター" | "링미터" | "วงแหวนวัด" =>
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
            "ui_vu" | "音量条" | "VUメーター" | "음량막대" | "มาตรเสียง" =>
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
            "ui_spark" | "迷你图" | "スパークライン" | "스파크라인" | "กราฟจิ๋ว" =>
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
            "ui_battery" | "电池" | "バッテリー" | "배터리" | "แบตเตอรี่" =>
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
            "ui_button" | "按钮" | "ボタン" | "버튼" | "ปุ่ม" => {
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
            "ui_toggle" | "开关" | "トグル" | "토글" | "สวิตช์" => {
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
            "ui_slider" | "滑块" | "スライダー" | "슬라이더" | "แถบเลื่อน" =>
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
                    let frac = ((mx - x) / w0).max(0.0).min(1.0);
                    val = mn + (mx_ - mn) * frac;
                }
                let frac = ((val - mn) / (mx_ - mn).abs().max(1e-6)).max(0.0).min(1.0);
                let th = self.ui_theme;
                let fill = self.color_at(&args, 6, th.primary);
                self.draw_ui(&ling_ui::widgets::slider(
                    x, y, w0, frac, hover, fill, th.track,
                ));
                return Ok(Value::Number(val as f64));
            },
            #[cfg(not(target_arch = "wasm32"))]
            "ui_checkbox" | "复选框" | "チェックボックス" | "체크박스" | "ช่องเลือก" =>
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
            "ui_tabs" | "标签页" | "タブ" | "탭" | "แท็บ" => {
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
            "ui_progress" | "进度" | "プログレス" | "진행바" | "ความคืบหน้า" =>
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
            "ui_tooltip" | "提示框" | "ツールチップ" | "툴팁" | "คำแนะนำ" =>
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
            "ui_stepper" | "步进器" | "ステッパー" | "스테퍼" | "ตัวปรับค่า" =>
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
            "ui_healthbar" | "血条" | "体力バー" | "체력바" | "แถบพลังชีวิต" =>
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
            "ui_cooldown" | "冷却" | "クールダウン" | "쿨다운" | "คูลดาวน์" =>
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
            "ui_counter" | "计数器" | "カウンター" | "카운터" | "ตัวนับ" => {
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
            "ui_minimap" | "小地图" | "ミニマップ" | "미니맵" | "แผนที่ย่อ" =>
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
            "ui_dpad" | "方向键" | "方向パッド" | "방향패드" | "ปุ่มทิศทาง" =>
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
            "ui_slotgrid" | "物品格" | "スロットグリッド" | "슬롯격자" | "ช่องไอเทม" =>
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
            "ui_vignette" | "暗角" | "ビネット" | "비네트" | "ขอบมืด" => {
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
            "ui_gauge3d" | "立体仪表" | "立体ゲージ" | "입체게이지" | "มาตรวัด3มิติ" =>
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
            "ui_panel3d" | "立体面板" | "立体パネル" | "입체패널" | "แผง3มิติ" =>
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
            "ui_radar3d" | "立体雷达" | "立体レーダー" | "입체레이더" | "เรดาร์3มิติ" =>
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
            "audio_blip" | "提示音" | "ビープ音" | "효과음" | "เสียงบี๊บ" =>
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
            "ui_sound" | "界面音" | "UI音" | "인터페이스음" | "เสียงปุ่ม" =>
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
            "music_load" | "载入音乐" | "音楽読込" | "음악로드" | "โหลดเพลง" =>
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
            "music_duration" | "音乐时长" | "音楽長さ" | "음악길이" | "ความยาวเพลง" =>
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
            "music_bpm" | "节拍速度" | "テンポ" | "템포" | "จังหวะต่อนาที" =>
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
            "music_key" | "调性" | "調性" | "조성" | "คีย์เพลง" => {
                let id = self.arg_num(&args, 0, 0.0)? as i64;
                let k = self
                    .tracks
                    .get(id as usize)
                    .map(|t| ling_music::analysis::key_name(&t.mono, t.rate))
                    .unwrap_or_default();
                return Ok(Value::Str(k));
            },
            #[cfg(not(target_arch = "wasm32"))]
            "music_onsets" | "音符起点" | "オンセット" | "온셋" | "จุดเริ่มเสียง" =>
            {
                let id = self.arg_num(&args, 0, 0.0)? as i64;
                let v = self
                    .tracks
                    .get(id as usize)
                    .map(|t| ling_music::analysis::onsets(&t.mono, t.rate))
                    .unwrap_or_default();
                return Ok(Value::List(
                    v.into_iter().map(|x| Value::Number(x as f64)).collect(),
                ));
            },
            #[cfg(not(target_arch = "wasm32"))]
            "music_beat_grid" | "节拍网格" | "ビートグリッド" | "비트그리드" | "กริดจังหวะ" =>
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
                return Ok(Value::List(
                    beats.into_iter().map(|x| Value::Number(x as f64)).collect(),
                ));
            },

            // ── playback ──
            #[cfg(not(target_arch = "wasm32"))]
            "music_play" | "播放音乐" | "音楽再生" | "음악재생" | "เล่นเพลง" =>
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
            "music_pause" | "暂停音乐" | "音楽一時停止" | "음악일시정지" | "หยุดเพลงชั่วคราว" =>
            {
                if let Some(m) = &self.music {
                    m.pause();
                }
                return Ok(Value::Unit);
            },
            #[cfg(not(target_arch = "wasm32"))]
            "music_stop" | "停止音乐" | "音楽停止" | "음악정지" | "หยุดเพลง" =>
            {
                if let Some(m) = &self.music {
                    m.stop();
                }
                return Ok(Value::Unit);
            },
            #[cfg(not(target_arch = "wasm32"))]
            "music_seek" | "定位音乐" | "音楽シーク" | "음악탐색" | "ค้นหาเพลง" =>
            {
                let sec = self.arg_num(&args, 0, 0.0)? as f32;
                if let Some(m) = &self.music {
                    m.seek(sec);
                }
                return Ok(Value::Unit);
            },
            #[cfg(not(target_arch = "wasm32"))]
            "music_pos" | "音乐位置" | "音楽位置" | "음악위치" | "ตำแหน่งเพลง" =>
            {
                let p = self.music.as_ref().map(|m| m.position()).unwrap_or(0.0);
                return Ok(Value::Number(p as f64));
            },
            #[cfg(not(target_arch = "wasm32"))]
            "music_volume" | "音乐音量" | "音楽音量" | "음악음량" | "ระดับเพลง" =>
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
            "music_patch" | "乐器音色" | "音色読込" | "악기패치" | "แพตช์เครื่องดนตรี" =>
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
            "music_note" | "弹音符" | "音符演奏" | "음표연주" | "เล่นโน้ต" =>
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
            "music_note_on" | "音符开始" | "音符オン" | "음표켜기" | "โน้ตเริ่ม" =>
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
            "music_note_off" | "音符结束" | "音符オフ" | "음표끄기" | "โน้ตจบ" =>
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
            "music_judge" | "判定" | "判定する" | "판정" | "ตัดสินจังหวะ" =>
            {
                let delta_ms = self.arg_num(&args, 0, 9999.0)? as f32;
                return Ok(Value::Number(
                    ling_music::Grade::judge(delta_ms).index() as f64
                ));
            },
            #[cfg(not(target_arch = "wasm32"))]
            "music_grade_name" | "判定名" | "判定名称" | "판정이름" | "ชื่อการตัดสิน" =>
            {
                let idx = self.arg_num(&args, 0, 4.0)? as i32;
                return Ok(Value::Str(
                    ling_music::Grade::from_index(idx).name().to_string(),
                ));
            },

            // ── karaoke ──
            #[cfg(not(target_arch = "wasm32"))]
            "music_lrc" | "载入歌词" | "歌詞読込" | "가사로드" | "โหลดเนื้อเพลง" =>
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
            "music_lyric" | "当前歌词" | "現在歌詞" | "현재가사" | "เนื้อเพลงปัจจุบัน" =>
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
            "music_mic_pitch" | "麦克风音高" | "マイク音程" | "마이크음정" | "ระดับเสียงไมค์" =>
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
            "music_note_name" | "音名" | "音名称" | "음이름" | "ชื่อโน้ต" =>
            {
                let hz = self.arg_num(&args, 0, 0.0)? as f32;
                return Ok(Value::Str(ling_music::note::hz_to_name(hz)));
            },
            #[cfg(not(target_arch = "wasm32"))]
            "music_hz" | "音符频率" | "音符周波数" | "음표주파수" | "ความถี่โน้ต" =>
            {
                let midi = self.pitch_arg(&args, 0, 69);
                return Ok(Value::Number(
                    ling_music::note::midi_to_hz(midi as f32) as f64
                ));
            },
            #[cfg(not(target_arch = "wasm32"))]
            "music_pitch_score" | "音准评分" | "音程スコア" | "음정점수" | "คะแนนเสียง" =>
            {
                let hz = self.arg_num(&args, 0, 0.0)? as f32;
                let target = self.arg_num(&args, 1, 0.0)? as f32;
                return Ok(Value::Number(
                    ling_music::karaoke::pitch_score(hz, target) as f64
                ));
            },

            // ── MIDI (inaudible note source: drive coins, cues, etc.) ──
            #[cfg(not(target_arch = "wasm32"))]
            "music_midi_load" | "载入MIDI" | "MIDI読込" | "미디로드" | "โหลดมิดี" =>
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
            "music_midi_count" | "MIDI数量" | "MIDI数" | "미디수" | "จำนวนมิดี" =>
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
            "music_midi_notes" | "MIDI音符" | "MIDIノート" | "미디음표" | "โน้ตมิดี" =>
            {
                let id = self.arg_num(&args, 0, 0.0)? as i64;
                let mut out = Vec::new();
                if let Some(m) = self.midis.get(id as usize) {
                    for n in &m.notes {
                        out.push(Value::Number(n.time as f64));
                        out.push(Value::Number(n.midi as f64));
                    }
                }
                return Ok(Value::List(out));
            },
            // music_midi_bars(id) -> flat [time, midi, dur, …] (for karaoke note bars)
            #[cfg(not(target_arch = "wasm32"))]
            "music_midi_bars" | "MIDI音条" | "MIDIバー" | "미디바" | "แท่งมิดี" =>
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
                return Ok(Value::List(out));
            },

            // music_fft(track_id, nbands) -> spectrum at the current playback position
            #[cfg(not(target_arch = "wasm32"))]
            "music_fft" | "音乐频谱" | "音楽スペクトル" | "음악스펙트럼" | "สเปกตรัมเพลง" =>
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
                return Ok(Value::List(
                    bands.into_iter().map(|x| Value::Number(x as f64)).collect(),
                ));
            },

            // ── stop every one-shot SFX/morph/sample voice (scene cleanup) ──
            #[cfg(not(target_arch = "wasm32"))]
            "audio_stop_sfx" | "停止音效" | "効果音停止" | "효과음정지" | "หยุดเอฟเฟกต์ทั้งหมด" =>
            {
                if let Some(a) = &self.audio {
                    a.stop_all_sfx();
                }
                return Ok(Value::Unit);
            },
            // ── spatial (2D/3D/4D) one-shot SFX ──
            #[cfg(not(target_arch = "wasm32"))]
            "audio_sfx" | "音效" | "空間効果音" | "공간효과음" | "เสียงเอฟเฟกต์" =>
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
            "morph_note" | "โน้ตมอร์ฟ" | "变形音" | "モーフ音" | "모프음" =>
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
            "audio_sample_load" | "载入采样" | "サンプル読込" | "샘플로드" | "โหลดตัวอย่างเสียง" =>
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
            "audio_sample_play" | "播放采样" | "サンプル再生" | "샘플재생" | "เล่นตัวอย่างเสียง" =>
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
            "audio_sample_stop" | "停止采样" | "サンプル停止" | "샘플정지" | "หยุดตัวอย่างเสียง" =>
            {
                let v = self.arg_num(&args, 0, 0.0)? as u32;
                if let Some(a) = &self.audio {
                    a.stop_sample(v);
                }
                return Ok(Value::Unit);
            },
            // ── master FX: delay / reverb / low-pass (underwater) ──
            #[cfg(not(target_arch = "wasm32"))]
            "audio_fx_delay" | "回声" | "ディレイ効果" | "딜레이" | "เสียงสะท้อน" =>
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
            "audio_fx_reverb" | "混响" | "リバーブ" | "리버브" | "เสียงก้อง" =>
            {
                let mix = self.arg_num(&args, 0, 0.3)? as f32;
                if let Some(a) = &self.audio {
                    a.fx_reverb(mix);
                }
                return Ok(Value::Unit);
            },
            #[cfg(not(target_arch = "wasm32"))]
            "audio_fx_lowpass" | "低通滤波" | "ローパス" | "저역통과" | "กรองความถี่ต่ำ" =>
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
            "soft_ball" | "软球" | "ソフトボール" | "소프트볼" | "ลูกบอลนุ่ม" =>
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
            "soft_step" | "软体步进" | "ソフト更新" | "소프트스텝" | "ก้าวนุ่ม" =>
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
            "soft_bounce" | "软体落地" | "ソフト着地" | "소프트바운스" | "เด้งนุ่ม" =>
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
            "soft_contain" | "软体边界" | "ソフト箱" | "소프트경계" | "กล่องนุ่ม" =>
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
            "soft_kick" | "软体踢" | "ソフト衝撃" | "소프트킥" | "เตะนุ่ม" =>
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
            "soft_spin" | "软体自旋" | "ソフト回転" | "소프트회전" | "หมุนนุ่ม" =>
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
            "soft_deform" | "形变量" | "変形量" | "변형량" | "ความบิดเบี้ยว" =>
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
            | "ความเร็วเชิงมุมนุ่ม" => {
                let id = self.arg_num(&args, 0, 0.)? as usize;
                let w = self
                    .soft_bodies
                    .get(id)
                    .map(|b| b.angular_speed())
                    .unwrap_or(0.0);
                return Ok(Value::Number(w as f64));
            },
            #[cfg(not(target_arch = "wasm32"))]
            "soft_centroid" | "软体质心" | "ソフト重心" | "소프트중심" | "จุดศูนย์กลางนุ่ม" =>
            {
                let id = self.arg_num(&args, 0, 0.)? as usize;
                let c = self
                    .soft_bodies
                    .get(id)
                    .map(|b| b.centroid())
                    .unwrap_or(ling_physics::Vec3::ZERO);
                return Ok(Value::List(vec![
                    Value::Number(c.x as f64),
                    Value::Number(c.y as f64),
                    Value::Number(c.z as f64),
                ]));
            },
            // soft_nodes(id) -> flat [x,y,z, x,y,z, …] for rendering the deformed mesh
            #[cfg(not(target_arch = "wasm32"))]
            "soft_nodes" | "软体节点" | "ソフト節点" | "소프트노드" | "จุดนุ่ม" =>
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
                return Ok(Value::List(out));
            },

            // ── rigid bodies with angular dynamics ──
            #[cfg(not(target_arch = "wasm32"))]
            "rb_add" | "刚体添加" | "剛体追加" | "강체추가" | "เพิ่มวัตถุแข็ง" =>
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
            "rb_torque" | "扭矩" | "トルク" | "토크" | "แรงบิด" => {
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
            "rb_spin" | "自旋" | "スピン" | "스핀" | "หมุน" => {
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
            "rb_impulse" | "刚体冲量" | "剛体インパルス" | "강체충격" | "แรงดลแข็ง" =>
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
            "rb_floor" | "刚体落地" | "剛体着地" | "강체바닥" | "พื้นแข็ง" =>
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
            "rb_gravity" | "刚体重力" | "剛体重力" | "강체중력" | "แรงโน้มถ่วงแข็ง" =>
            {
                let gx = self.arg_num(&args, 0, 0.)? as f32;
                let gy = self.arg_num(&args, 1, 9.81)? as f32;
                let gz = self.arg_num(&args, 2, 0.)? as f32;
                self.rigid_world.gravity = ling_physics::Vec3::new(gx, gy, gz);
                return Ok(Value::Unit);
            },
            #[cfg(not(target_arch = "wasm32"))]
            "rb_step" | "刚体步进" | "剛体更新" | "강체스텝" | "ก้าวแข็ง" =>
            {
                let dt = self.arg_num(&args, 0, 0.016)? as f32;
                self.rigid_world.step(dt);
                return Ok(Value::Unit);
            },
            #[cfg(not(target_arch = "wasm32"))]
            "rb_pos" | "刚体位置" | "剛体位置" | "강체위치" | "ตำแหน่งแข็ง" =>
            {
                let i = self.arg_num(&args, 0, 0.)? as usize;
                let p = self
                    .rigid_world
                    .bodies
                    .get(i)
                    .map(|b| b.pos)
                    .unwrap_or(ling_physics::Vec3::ZERO);
                return Ok(Value::List(vec![
                    Value::Number(p.x as f64),
                    Value::Number(p.y as f64),
                    Value::Number(p.z as f64),
                ]));
            },
            #[cfg(not(target_arch = "wasm32"))]
            "rb_rot" | "刚体旋转" | "剛体回転" | "강체회전" | "การหมุนแข็ง" =>
            {
                let i = self.arg_num(&args, 0, 0.)? as usize;
                let q = self
                    .rigid_world
                    .bodies
                    .get(i)
                    .map(|b| b.orientation)
                    .unwrap_or(ling_physics::Quat::IDENTITY);
                return Ok(Value::List(vec![
                    Value::Number(q.x as f64),
                    Value::Number(q.y as f64),
                    Value::Number(q.z as f64),
                    Value::Number(q.w as f64),
                ]));
            },

            // ── native-res mesh (.lmesh): load once, draw fast (unlit, per-tri colour) ──
            #[cfg(not(target_arch = "wasm32"))]
            "mesh_load" | "โหลดเมช" | "载入网格" | "メッシュ読込" | "메시로드" =>
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
            "mesh_load" | "โหลดเมช" | "载入网格" | "メッシュ読込" | "메시로드" =>
            {
                // Native .lmesh loading is file-system based and not wired for wasm yet.
                // Return an invalid handle so scripts can choose a fallback path.
                return Ok(Value::Number(-1.0));
            },
            #[cfg(not(target_arch = "wasm32"))]
            "mesh_draw" | "วาดเมชสี" | "绘制网格" | "メッシュ描画" | "메시그리기" =>
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
            "mesh_draw" | "วาดเมชสี" | "绘制网格" | "メッシュ描画" | "메시그리기" =>
            {
                return Ok(Value::Unit);
            },

            // ── liquid sim (water + oil, immiscible) ──
            "liquid_new" | "新建液体" | "液体新規" | "액체생성" | "สร้างของเหลว" =>
            {
                let w = self.arg_num(&args, 0, 64.)? as usize;
                let h = self.arg_num(&args, 1, 64.)? as usize;
                let id = self.liquids.len();
                self.liquids
                    .push(ling_physics::liquid::LiquidGrid::new(w, h));
                return Ok(Value::Number(id as f64));
            },
            "liquid_set_colors" | "液体颜色" | "液体配色" | "액체색상" | "สีของเหลว" =>
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
            "liquid_splat" | "液体注入" | "液体追加" | "액체분사" | "หยดของเหลว" =>
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
            "liquid_gravity" | "液体重力" | "液体重力ベクトル" | "액체중력" | "แรงโน้มถ่วงเหลว" =>
            {
                let id = self.arg_num(&args, 0, 0.)? as usize;
                let gx = self.arg_num(&args, 1, 0.)? as f32;
                let gy = self.arg_num(&args, 2, 60.)? as f32;
                if let Some(g) = self.liquids.get_mut(id) {
                    g.set_gravity(gx, gy);
                }
                return Ok(Value::Unit);
            },
            "liquid_step" | "液体步进" | "液体更新" | "액체스텝" | "ก้าวของเหลว" =>
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
            "liquid_step_all" | "液体全步进" | "液体全更新" | "전체액체스텝" | "ก้าวของเหลวทั้งหมด" =>
            {
                let dt = self.arg_num(&args, 0, 0.016)? as f32;
                ling_physics::liquid::step_all(&mut self.liquids, dt);
                return Ok(Value::Unit);
            },
            // liquid_rainbow(id, on) — colour the fluid as a flowing ROYGBIV marble
            "liquid_rainbow" | "液体彩虹" | "液体虹" | "액체무지개" | "ของเหลวสายรุ้ง" =>
            {
                let id = self.arg_num(&args, 0, 0.)? as usize;
                let on = self.arg_num(&args, 1, 1.0)? > 0.5;
                if let Some(g) = self.liquids.get_mut(id) {
                    g.rainbow = on;
                }
                return Ok(Value::Unit);
            },
            // liquid_mix(id) -> 0 (oil/water separated) .. 1 (fully intermixed)
            "liquid_mix" | "液体混合" | "液体混合度" | "액체혼합" | "การผสมของเหลว" =>
            {
                let id = self.arg_num(&args, 0, 0.)? as usize;
                let m = self.liquids.get(id).map(|g| g.mix_amount()).unwrap_or(0.0);
                return Ok(Value::Number(m as f64));
            },
            // liquid_draw(id, sx, sy, scale) — fast flat 2-D blit of the colour field
            #[cfg(not(target_arch = "wasm32"))]
            "liquid_draw" | "绘制液体" | "液体描画" | "액체그리기" | "วาดของเหลว" =>
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
            "liquid_draw_surface" | "液体贴面" | "液体曲面" | "액체곡면" | "ของเหลวบนพื้นผิว" =>
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
            "sparkle" | "闪光" | "きらめき" | "반짝임" | "ประกาย" => {
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
                    let tw = (t * 3.0 + phase * 6.2831 + n as f32).sin() * 0.5 + 0.5;
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
            "dialog_show" | "对话显示" | "会話表示" | "대화표시" | "แสดงบทสนทนา" =>
            {
                let text = self.arg_str(&args, 0, "");
                let cps = self.arg_num(&args, 1, 32.0)? as f32;
                self.dialog = Some(ling_game::dialog::Dialog::new(&text, cps));
                return Ok(Value::Unit);
            },
            #[cfg(not(target_arch = "wasm32"))]
            "dialog_step" | "对话步进" | "会話更新" | "대화스텝" | "ก้าวบทสนทนา" =>
            {
                let dt = self.arg_num(&args, 0, 0.016)? as f32;
                if let Some(d) = self.dialog.as_mut() {
                    d.update(dt);
                }
                return Ok(Value::Unit);
            },
            #[cfg(not(target_arch = "wasm32"))]
            "dialog_advance" | "对话推进" | "会話送り" | "대화진행" | "เลื่อนบทสนทนา" =>
            {
                if let Some(d) = self.dialog.as_mut() {
                    d.advance();
                }
                return Ok(Value::Unit);
            },
            #[cfg(not(target_arch = "wasm32"))]
            "dialog_active" | "对话激活" | "会話中" | "대화중" | "บทสนทนาทำงาน" =>
            {
                let a = self
                    .dialog
                    .as_ref()
                    .map(|d| !d.is_closed())
                    .unwrap_or(false);
                return Ok(Value::Bool(a));
            },
            #[cfg(not(target_arch = "wasm32"))]
            "dialog_typing" | "对话打字" | "会話タイプ中" | "대화타이핑" | "กำลังพิมพ์บทสนทนา" =>
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
            "dialog_close" | "对话关闭" | "会話閉じる" | "대화닫기" | "ปิดบทสนทนา" =>
            {
                self.dialog = None;
                return Ok(Value::Unit);
            },
            // dialog_color(role, r, g, b) — role: 0 text · 1 name · 2 place · 3 item
            #[cfg(not(target_arch = "wasm32"))]
            "dialog_color" | "对话颜色" | "会話色" | "대화색" | "สีบทสนทนา" =>
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
            "dialog_draw" | "对话绘制" | "会話描画" | "대화그리기" | "วาดบทสนทนา" =>
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

            // text_poll() — fold newly-typed keys into the input buffer, return it
            #[cfg(not(target_arch = "wasm32"))]
            "text_poll" => {
                let keys = {
                    let gfx = self.gfx.borrow();
                    gfx.window
                        .as_ref()
                        .map(|w| w.get_keys_pressed(minifb::KeyRepeat::No))
                        .unwrap_or_default()
                };
                for k in keys {
                    if k == minifb::Key::Backspace {
                        self.text_buffer.pop();
                    } else if let Some(c) = key_char(k) {
                        self.text_buffer.push(c);
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
            "screenshot" | "บันทึกภาพ" => {
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
                    let queue = std::mem::take(&mut gfx.depth_queue);
                    {
                        let g = &mut *gfx;
                        let z = if dt { Some(&mut g.depth_buf) } else { None };
                        queue.flush(&mut g.buffer, z, reset_z, w, h);
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
                    let queue = std::mem::take(&mut gfx.depth_queue);
                    {
                        let g = &mut *gfx;
                        let z = if dt { Some(&mut g.depth_buf) } else { None };
                        queue.flush(&mut g.buffer, z, reset_z, w, h);
                    }
                    gfx.zbuf_needs_clear = false;
                    gfx.depth_queue.set_state(bm, ba);
                }
                return Ok(Value::Unit);
            },

            // Viscous full-screen distortion (warp/pucker/bloat, edge-wrapped). Call
            // after the 3-D flush and before the UI so only the world layer warps.
            #[cfg(not(target_arch = "wasm32"))]
            "screen_distort" | "บิดจอ" | "屏幕扭曲" | "画面歪み" | "화면왜곡" =>
            {
                let amount = self.arg_num(&args, 0, 8.0)? as f32;
                let t = self.arg_num(&args, 1, 0.0)? as f32;
                // optional `step` (default 1 = full res): 2 = half-res block warp
                // (~4× fewer warp computes, slightly softer — suits a liquid look).
                let step = self.arg_num(&args, 2, 1.0)?.max(1.0) as usize;
                self.gfx.borrow_mut().distort(amount, t, step);
                return Ok(Value::Unit);
            },

            "set_rim" | "设置边缘光" | "リム設定" | "림라이트" | "ตั้งขอบเรือง" =>
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
            (Value::Str(s), "len" | "长") => Ok(Value::Number(s.len() as f64)),
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
            (Value::List(v), "len" | "长") => Ok(Value::Number(v.len() as f64)),
            (Value::List(v), "push" | "推") => {
                let mut v2 = v.clone();
                if let Some(a) = args.first() {
                    v2.push(a.clone());
                }
                Ok(Value::List(v2))
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
            (Pattern::Wildcard, _) => Some(Env::new()),
            (Pattern::Str(s), Value::Str(v)) if s == v => Some(Env::new()),
            (Pattern::Number(n), Value::Number(v)) if (n - v).abs() < 1e-12 => Some(Env::new()),
            (Pattern::Bool(b), Value::Bool(v)) if b == v => Some(Env::new()),
            (Pattern::Ident(name), _) => {
                let mut e = Env::new();
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
                    (None, _) => Some(Env::new()),
                    (Some(p), None) => self.match_pattern(p, &Value::Unit),
                }
            },
            // User enum variant pattern: `Circle(r)`, `Pair(a, b)`, nullary `Origin()`.
            (Pattern::Variant(vname, sub_pats), Value::Variant { variant, payload, .. }) => {
                if vname != variant || sub_pats.len() != payload.len() {
                    return None;
                }
                let mut bindings = Env::new();
                for (p, v) in sub_pats.iter().zip(payload.iter()) {
                    bindings.extend(self.match_pattern(p, v)?);
                }
                Some(bindings)
            },
            // A zero-payload variant pattern also matches the bare result-style `ok`/`bad`
            // values so `Ok()`-style patterns keep working uniformly.
            (Pattern::Variant(vname, sub), Value::Ok(v)) if (vname == "ok" || vname == "好") => {
                match sub.as_slice() {
                    [] => Some(Env::new()),
                    [p] => self.match_pattern(p, v),
                    _ => None,
                }
            },
            (Pattern::Variant(vname, sub), Value::Err(v))
                if (vname == "bad" || vname == "坏" || vname == "err") =>
            {
                match sub.as_slice() {
                    [] => Some(Env::new()),
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
            Value::List(v) => Ok(v),
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
                        if spec.starts_with(":.") {
                            if let Value::Number(n) = &args[arg_idx] {
                                let prec: usize =
                                    spec[2..].trim_end_matches('f').parse().unwrap_or(2);
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
        "start" | "menu" | "options" => B::Start,
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

/// Hide the console window that the OS auto-attaches to console-subsystem
/// processes. No-op on non-Windows and when no console is present.
#[cfg(not(target_arch = "wasm32"))]
fn hide_console_window() {
    #[cfg(windows)]
    unsafe {
        extern "system" {
            fn GetConsoleWindow() -> isize;
            fn ShowWindow(hwnd: isize, nCmdShow: i32) -> i32;
        }
        let hwnd = GetConsoleWindow();
        if hwnd != 0 {
            ShowWindow(hwnd, 0); // SW_HIDE = 0
        }
    }
}

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

/// Non-Windows native fallback: resolution from [`native_screen_size`], 60 Hz.
#[cfg(all(not(target_arch = "wasm32"), not(windows)))]
fn monitor_info() -> (i32, i32, i32) {
    let (w, h) = native_screen_size();
    (w as i32, h as i32, 60)
}

/// WASM fallback: the canvas is the display surface; assume 60 Hz.
#[cfg(target_arch = "wasm32")]
fn monitor_info() -> (i32, i32, i32) {
    let (w, h) = crate::gfx::webgl::canvas_size();
    (w as i32, h as i32, 60)
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
    rows.sort_by(|a, b| b.2.cmp(&a.2)); // by total time desc
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
        let pct = if total_ns > 0 { *ns as f64 / total_ns as f64 * 100.0 } else { 0.0 };
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
