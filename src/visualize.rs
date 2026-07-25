// src/visualize.rs — AST-driven SVG visualiser for .ling source files.
//
// Usage: ling visualize examples/foo.ling > foo.svg
//
// Each function becomes a card showing colour-coded geometric icons for every
// vtex_*/audio_*/control-flow call it makes.  Global constants appear in a
// sidebar.  The entry-point card is highlighted in gold.

use crate::parser::ast::*;
use std::collections::HashSet;
use std::fmt::Write;

// ── Layout constants ─────────────────────────────────────────────────────────

const SVG_W: f32 = 1640.0;
const SIDEBAR_W: f32 = 200.0;
const CARD_W: f32 = 370.0;
const CARD_GAP: f32 = 14.0;
const GRID_X: f32 = SIDEBAR_W + 14.0;
const COLS: usize = 3;
const HEADER_H: f32 = 74.0;
const LEGEND_H: f32 = 64.0;
const CONTENT_Y: f32 = HEADER_H + LEGEND_H;
const ICON_SZ: f32 = 22.0; // icon bounding box (icon radius = ICON_SZ/2)
const ICON_GAP: f32 = 4.0;
const ICONS_ROW: usize = 11;
const CARD_PAD: f32 = 12.0;
const CARD_ROUNDING: f32 = 7.0;

// ── Colour palette ───────────────────────────────────────────────────────────

const BG: &str = "#0b0b1a";
const CARD_BG: &str = "#11112a";
const CARD_BD: &str = "#22225a";
const TEXT: &str = "#c0c0e0";
const TEXT_DIM: &str = "#52528a";
const GOLD: &str = "#ffd700";
const SIDEBAR_BG: &str = "#0d0d22";

// ── Call category ────────────────────────────────────────────────────────────

#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
#[allow(dead_code)]
pub(crate) enum Cat {
    // ── Surface textures (vtex_*) ──
    Rings,
    Spiral,
    Star,
    Flower,
    Lotus,
    Chakra,
    Yantra,
    Hyper,
    Tess,
    Rain,
    Grid,
    Halftone,
    Pagoda,
    Torii,
    Cog,
    // ── Procedural / fractal textures (tex_*) ──
    Fractal,
    // ── Audio ──
    Tone,
    Vol,
    Listen,
    Sfx,
    Spectrum,
    // ── Music / MIDI ──
    Music,
    Note,
    // ── Cryptography ──
    Hash,
    Cipher,
    Sign,
    Kem,
    Shard,
    // ── Physics ──
    Rigid,
    Soft,
    Liquid,
    Force,
    Collide,
    // ── 3D mesh & geometry ──
    Mesh,
    Draw3D,
    Draw2D,
    // ── Math ──
    Trig,
    MathFn,
    Noise,
    // ── Networking & AI ──
    Net,
    Neural,
    Behavior,
    // ── UI / dialog / holograms ──
    Widget,
    Hud,
    Dialog,
    Holo,
    // ── Text & vector ──
    Str,
    Font,
    Vector,
    // ── Colour, shading, scene ──
    Color,
    Shade,
    Camera,
    Light,
    Fill,
    Present,
    Window,
    // ── Input ──
    Key,
    Mouse,
    // ── IO / system / timing ──
    Print,
    File,
    Sys,
    Anim,
    // ── Generic ──
    User,
    Loop,
    Const,
}

impl Cat {
    pub(crate) fn color(self) -> &'static str {
        match self {
            // Surface textures — neon spectrum
            Cat::Rings => "#00e5ff",
            Cat::Spiral => "#00ffb3",
            Cat::Star => "#ffd700",
            Cat::Flower => "#ff79c6",
            Cat::Lotus => "#ff5e8a",
            Cat::Chakra => "#bd93f9",
            Cat::Yantra => "#ffb86c",
            Cat::Hyper => "#5575c8",
            Cat::Tess => "#50fa7b",
            Cat::Rain => "#f1fa8c",
            Cat::Grid => "#8be9fd",
            Cat::Halftone => "#888899",
            Cat::Pagoda => "#ff9a3c",
            Cat::Torii => "#ff5e5e",
            Cat::Cog => "#b0b0c8",
            // Procedural / fractal — violet
            Cat::Fractal => "#c77dff",
            // Audio — magenta family
            Cat::Tone => "#ff1493",
            Cat::Vol => "#ff69b4",
            Cat::Listen => "#db77d9",
            Cat::Sfx => "#ff85c0",
            Cat::Spectrum => "#ff4fa3",
            // Music — purple
            Cat::Music => "#b388ff",
            Cat::Note => "#d8a7ff",
            // Crypto — jade/teal family
            Cat::Hash => "#0fd9b0",
            Cat::Cipher => "#12b48c",
            Cat::Sign => "#5ce0c0",
            Cat::Kem => "#2aa98a",
            Cat::Shard => "#7ff0d8",
            // Physics — warm / amber + water
            Cat::Rigid => "#ff8c42",
            Cat::Soft => "#ffb37b",
            Cat::Liquid => "#4aa3ff",
            Cat::Force => "#ff6b35",
            Cat::Collide => "#ff4d4d",
            // 3D / draw — blue
            Cat::Mesh => "#6aa9ff",
            Cat::Draw3D => "#8fb8ff",
            Cat::Draw2D => "#5fd0ff",
            // Math — lime
            Cat::Trig => "#b6e36b",
            Cat::MathFn => "#d4e157",
            Cat::Noise => "#9ccc65",
            // Net / AI
            Cat::Net => "#18b6e0",
            Cat::Neural => "#ffd54f",
            Cat::Behavior => "#ffca6b",
            // UI / dialog / holo
            Cat::Widget => "#7a8fb0",
            Cat::Hud => "#92a8c8",
            Cat::Dialog => "#e0b97d",
            Cat::Holo => "#66ffe0",
            // Text & vector
            Cat::Str => "#a0b4d0",
            Cat::Font => "#c0c8e0",
            Cat::Vector => "#88d8c0",
            // Colour / shading / scene
            Cat::Color => "#ff7eb6",
            Cat::Shade => "#9d8df1",
            Cat::Camera => "#ffe066",
            Cat::Light => "#ffe4b5",
            Cat::Fill => "#3a3a6a",
            Cat::Present => "#555580",
            Cat::Window => "#7fd4ff",
            // Input
            Cat::Key => "#ff9e7d",
            Cat::Mouse => "#ffbfa3",
            // IO / system / timing
            Cat::Print => "#9aa0b5",
            Cat::File => "#7d8aa8",
            Cat::Sys => "#6b7488",
            Cat::Anim => "#ffd28a",
            // Generic
            Cat::User => "#6ab0f5",
            Cat::Loop => "#ff7f50",
            Cat::Const => "#50fa7b",
        }
    }

    pub(crate) fn label(self) -> &'static str {
        match self {
            Cat::Rings => "rings",
            Cat::Spiral => "spiral",
            Cat::Star => "star",
            Cat::Flower => "flower",
            Cat::Lotus => "lotus",
            Cat::Chakra => "chakra",
            Cat::Yantra => "yantra",
            Cat::Hyper => "hyperbolic",
            Cat::Tess => "tessellated",
            Cat::Rain => "letter rain",
            Cat::Grid => "grid",
            Cat::Halftone => "halftone",
            Cat::Pagoda => "pagoda",
            Cat::Torii => "torii",
            Cat::Cog => "spiked cog",
            Cat::Fractal => "fractal tex",
            Cat::Tone => "audio tone",
            Cat::Vol => "audio vol",
            Cat::Listen => "listener",
            Cat::Sfx => "sfx",
            Cat::Spectrum => "spectrum",
            Cat::Music => "music",
            Cat::Note => "note",
            Cat::Hash => "hash",
            Cat::Cipher => "cipher",
            Cat::Sign => "signature",
            Cat::Kem => "key exch",
            Cat::Shard => "secret share",
            Cat::Rigid => "rigid body",
            Cat::Soft => "soft body",
            Cat::Liquid => "liquid",
            Cat::Force => "force",
            Cat::Collide => "collision",
            Cat::Mesh => "mesh",
            Cat::Draw3D => "draw 3d",
            Cat::Draw2D => "draw 2d",
            Cat::Trig => "trig",
            Cat::MathFn => "math",
            Cat::Noise => "noise",
            Cat::Net => "network",
            Cat::Neural => "neural net",
            Cat::Behavior => "behavior tree",
            Cat::Widget => "ui widget",
            Cat::Hud => "hud",
            Cat::Dialog => "dialog",
            Cat::Holo => "hologram",
            Cat::Str => "string",
            Cat::Font => "font",
            Cat::Vector => "svg vector",
            Cat::Color => "color",
            Cat::Shade => "shading",
            Cat::Camera => "camera",
            Cat::Light => "light",
            Cat::Fill => "fill",
            Cat::Present => "render",
            Cat::Window => "window",
            Cat::Key => "keyboard",
            Cat::Mouse => "mouse",
            Cat::Print => "print",
            Cat::File => "file io",
            Cat::Sys => "system",
            Cat::Anim => "animation",
            Cat::User => "fn call",
            Cat::Loop => "loop",
            Cat::Const => "const",
        }
    }

    /// Coarse domain a category belongs to — used for grouped legends & stats.
    pub(crate) fn domain(self) -> &'static str {
        if is_vtex(self) {
            return "texture";
        }
        if is_audio(self) {
            return "audio";
        }
        if is_crypto(self) {
            return "crypto";
        }
        if is_physics(self) {
            return "physics";
        }
        if is_ai(self) {
            return "ai";
        }
        match self {
            Cat::Music | Cat::Note => "music",
            Cat::Mesh | Cat::Draw3D | Cat::Draw2D | Cat::Vector => "geometry",
            Cat::Trig | Cat::MathFn | Cat::Noise | Cat::Fractal => "math",
            Cat::Widget | Cat::Hud | Cat::Dialog | Cat::Holo => "ui",
            Cat::Str | Cat::Font => "text",
            Cat::Color
            | Cat::Shade
            | Cat::Camera
            | Cat::Light
            | Cat::Fill
            | Cat::Present
            | Cat::Window => "scene",
            Cat::Key | Cat::Mouse => "input",
            Cat::Print | Cat::File | Cat::Sys | Cat::Anim | Cat::Net => "system",
            _ => "code",
        }
    }
}

pub(crate) fn categorize(name: &str) -> Cat {
    // ── Surface textures: vtex_* (+ Thai ลาย* / CJK aliases) ──
    if name.starts_with("vtex_rings")
        || name == "ลายวงซ้อน"
        || name == "纹环"
        || name == "輪模様"
        || name == "輪무늬"
    {
        return Cat::Rings;
    }
    if name.starts_with("vtex_spiral")
        || name == "ลายก้นหอย"
        || name == "ลายเกลียว"
        || name == "ลายเกลียวหมุน"
        || name == "纹螺"
        || name == "螺旋模様"
        || name == "나선무늬"
    {
        return Cat::Spiral;
    }
    if name.starts_with("vtex_star")
        || name == "ลายดาว"
        || name == "纹星"
        || name == "星模様"
        || name == "별무늬"
    {
        return Cat::Star;
    }
    if name.starts_with("vtex_flower")
        || name == "ลายดอกไม้"
        || name == "ลายดอก"
        || name == "纹花"
        || name == "花模様"
        || name == "꽃무늬"
    {
        return Cat::Flower;
    }
    if name.starts_with("vtex_lotus")
        || name == "ลายบัว"
        || name == "ลายดอกบัว"
        || name == "纹莲"
        || name == "蓮模様"
        || name == "연꽃무늬"
    {
        return Cat::Lotus;
    }
    if name.starts_with("vtex_chakra")
        || name == "ลายจักร"
        || name == "纹轮"
        || name == "輪模様"
        || name == "바퀴무늬"
    {
        return Cat::Chakra;
    }
    if name.starts_with("vtex_yantra")
        || name == "ลายยันต์"
        || name == "纹咒"
        || name == "護符模様"
        || name == "부적무늬"
    {
        return Cat::Yantra;
    }
    if name.starts_with("vtex_hyper")
        || name == "ลายไฮเพอร์โบลิก"
        || name == "ลายไฮเปอร์"
        || name == "纹超"
        || name == "超次元模様"
        || name == "초차원무늬"
    {
        return Cat::Hyper;
    }
    if name.starts_with("vtex_tessellated")
        || name == "ลายตาข่าย"
        || name == "纹镶嵌"
        || name == "網目模様"
    {
        return Cat::Tess;
    }
    if name.starts_with("vtex_letter_rain")
        || name.starts_with("vtex_rain")
        || name == "ลายอักษรไหล"
        || name == "ลายฝน"
        || name == "纹雨"
        || name == "文字雨"
        || name == "비무늬"
    {
        return Cat::Rain;
    }
    if name.starts_with("vtex_grid")
        || name == "ลายตาราง"
        || name == "纹格"
        || name == "格子模様"
        || name == "격자무늬"
    {
        return Cat::Grid;
    }
    if name.starts_with("vtex_halftone")
        || name == "ลายจุด"
        || name == "ลายฮาล์ฟโทน"
        || name == "纹半调"
        || name == "網点模様"
        || name == "망점"
    {
        return Cat::Halftone;
    }
    if name.starts_with("vtex_pagoda")
        || name == "ลายเจดีย์"
        || name == "เจดีย์"
        || name == "纹塔"
        || name == "탑"
    {
        return Cat::Pagoda;
    }
    if name.starts_with("vtex_torii")
        || name == "ประตูโทริอิ"
        || name == "纹鸟居"
        || name == "鳥居"
        || name == "도리이"
    {
        return Cat::Torii;
    }
    if name.starts_with("vtex_spiked_cog")
        || name == "ฟันเฟืองหนาม"
        || name == "纹棘轮"
        || name == "歯車模様"
        || name == "톱니바퀴"
    {
        return Cat::Cog;
    }
    if name.starts_with("vtex_") {
        return Cat::Tess;
    }

    // ── Procedural / fractal textures: tex_* ──
    if name.starts_with("tex_") {
        return Cat::Fractal;
    }

    // ── Audio ──
    if name == "audio_tone"
        || name == "เสียงโทน"
        || name == "音調"
        || name == "音调"
        || name == "음조"
    {
        return Cat::Tone;
    }
    if name == "audio_volume"
        || name == "audio_bgm_volume"
        || name == "ระดับเสียง"
        || name == "音量"
        || name == "음량"
    {
        return Cat::Vol;
    }
    if name == "audio_listener"
        || name == "音声リスナー"
        || name == "오디오리스너"
        || name == "音频监听"
    {
        return Cat::Listen;
    }
    if name.starts_with("audio_") {
        return Cat::Sfx;
    }
    if name.starts_with("mic_") || name.starts_with("fft_") {
        return Cat::Spectrum;
    }

    // ── Music / MIDI ──
    if name == "music_note_on"
        || name == "music_note_off"
        || name == "music_note"
        || name == "music_note_name"
    {
        return Cat::Note;
    }
    if name.starts_with("music_") || name.starts_with("midi_") || name.starts_with("MIDI") {
        return Cat::Music;
    }

    // ── Cryptography ──
    if name == "crypto_hash"
        || name == "sha3_512"
        || name == "blake3"
        || name == "hash_int"
        || name == "hash_str"
    {
        return Cat::Hash;
    }
    if name == "crypto_seal" || name == "crypto_open" || name == "encrypt" || name == "aes_gcm_256"
    {
        return Cat::Cipher;
    }
    if name == "ed25519"
        || name == "schnorr_verify"
        || name == "vrf_verify"
        || name == "derive"
        || name == "argon2id"
    {
        return Cat::Sign;
    }
    if name == "mlkem768" || name.starts_with("hybrid_") || name.starts_with("knot_") {
        return Cat::Kem;
    }
    if name == "shamir_reconstruct" {
        return Cat::Shard;
    }

    // ── Physics ──
    if name.starts_with("rb_") || name == "rigidbody" {
        return Cat::Rigid;
    }
    if name.starts_with("soft_") {
        return Cat::Soft;
    }
    if name.starts_with("liquid_") {
        return Cat::Liquid;
    }
    if matches!(
        name,
        "force"
            | "torque"
            | "spring"
            | "friction"
            | "elasticity"
            | "acceleration"
            | "apply_impulse"
            | "gyro"
    ) {
        return Cat::Force;
    }
    if matches!(
        name,
        "collision" | "raycast" | "aabb" | "AABB" | "constraint"
    ) {
        return Cat::Collide;
    }

    // ── 3D mesh & primitives ──
    if matches!(
        name,
        "cube"
            | "box"
            | "capsule"
            | "capsule_chain"
            | "cylinder"
            | "pyramid"
            | "icosphere"
            | "icosahedron"
            | "octahedron"
            | "orb_shell"
            | "stairs"
            | "frustum"
    ) {
        return Cat::Mesh;
    }
    if matches!(
        name,
        "draw_line_3d"
            | "draw_triangle_3d"
            | "line3d"
            | "triangle3d"
            | "project_3d"
            | "render_3d"
            | "flush_3d"
            | "font_text_3d"
    ) {
        return Cat::Draw3D;
    }

    // ── Neural / behavior / dialog (AI) ──
    if name.starts_with("nn_") {
        return Cat::Neural;
    }
    if name.starts_with("bt_") {
        return Cat::Behavior;
    }
    if name.starts_with("dialog_") {
        return Cat::Dialog;
    }

    // ── Networking ──
    if name.starts_with("net_") {
        return Cat::Net;
    }

    // ── UI widgets / HUD ──
    if matches!(
        name,
        "ui_radar"
            | "ui_radar3d"
            | "ui_minimap"
            | "ui_compass"
            | "ui_healthbar"
            | "ui_reticle"
            | "ui_target"
            | "ui_gauge"
            | "ui_gauge3d"
            | "ui_vu"
            | "ui_battery"
            | "ui_segbar"
            | "ui_bar"
            | "ui_progress"
            | "ui_cooldown"
            | "ui_scanlines"
            | "ui_vignette"
            | "ui_spark"
            | "ui_ring"
            | "ui_counter"
    ) {
        return Cat::Hud;
    }
    if name.starts_with("ui_") {
        return Cat::Widget;
    }

    // ── Holograms / particles ──
    if matches!(
        name,
        "holo_points" | "holo_fragment_count" | "knot_points" | "sparkle" | "neon" | "psychedelic"
    ) {
        return Cat::Holo;
    }

    // ── Fonts & vector text ──
    if name.starts_with("font_") {
        return Cat::Font;
    }
    if name.starts_with("svg_") {
        return Cat::Vector;
    }

    // ── Strings ──
    if name.starts_with("str_")
        || matches!(
            name,
            "split"
                | "join"
                | "trim"
                | "substr"
                | "format"
                | "to_str"
                | "num_str"
                | "starts_with"
                | "ends_with"
        )
    {
        return Cat::Str;
    }

    // ── Math: trig / noise / general ──
    if matches!(
        name,
        "sin"
            | "cos"
            | "tan"
            | "asin"
            | "acos"
            | "atan"
            | "atan2"
            | "arcsin"
            | "arccos"
            | "arctan"
            | "arctan2"
            | "tanh"
            | "tanhf"
            | "hypot"
    ) {
        return Cat::Trig;
    }
    if matches!(
        name,
        "perlin" | "perlin3" | "noise2" | "fbm" | "vnoise" | "smoothstep"
    ) {
        return Cat::Noise;
    }
    if matches!(
        name,
        "sqrt"
            | "cbrt"
            | "pow"
            | "exp"
            | "ln"
            | "log"
            | "log2"
            | "log10"
            | "abs"
            | "floor"
            | "ceil"
            | "round"
            | "trunc"
            | "fract"
            | "sign"
            | "clamp"
            | "lerp"
            | "min"
            | "max"
            | "rand"
            | "pi"
            | "tau"
    ) {
        return Cat::MathFn;
    }

    // ── Colour & shading ──
    if matches!(
        name,
        "hsl_color"
            | "hsv_to_rgb"
            | "set_color"
            | "set_color_hsl"
            | "color"
            | "lerp_color"
            | "gfx_color"
    ) {
        return Cat::Color;
    }
    if matches!(
        name,
        "set_shade_mode"
            | "set_rim"
            | "set_fog"
            | "set_ambient"
            | "set_cel_bands"
            | "set_blend"
            | "set_shadow_color"
            | "set_projection"
            | "set_zdist"
    ) {
        return Cat::Shade;
    }

    // ── Camera / lights ──
    if matches!(name, "set_camera" | "set_camera_pos" | "move_camera") {
        return Cat::Camera;
    }
    if matches!(name, "add_light" | "clear_lights") {
        return Cat::Light;
    }

    // ── Window / present / fill ──
    if matches!(
        name,
        "open_window"
            | "open_fullscreen"
            | "gfx_window"
            | "fullscreen"
            | "windowed"
            | "wait_window"
            | "is_open"
            | "window_is_open"
            | "gfx_is_open"
            | "gfx_wait"
            | "resolution"
            | "get_width"
            | "get_height"
    ) {
        return Cat::Window;
    }
    if matches!(
        name,
        "present" | "แสดงผล" | "gfx_present" | "render" | "render_3d" | "flush_3d"
    ) {
        return Cat::Present;
    }
    if matches!(name, "fill" | "เติม" | "gfx_fill" | "clear") {
        return Cat::Fill;
    }

    // ── 2D drawing ──
    if name.starts_with("draw_")
        || matches!(
            name,
            "gfx_line" | "gfx_pixel" | "gfx_triangle" | "line" | "triangle" | "pixel"
        )
    {
        return Cat::Draw2D;
    }

    // ── Input ──
    if name.starts_with("mouse_") || matches!(name, "capture_mouse" | "release_mouse") {
        return Cat::Mouse;
    }
    if name.starts_with("key_") || matches!(name, "keys" | "text_poll" | "text_get") {
        return Cat::Key;
    }

    // ── IO / system ──
    if matches!(
        name,
        "print" | "println" | "print_file" | "imprimir" | "afficher" | "вывести" | "พิมพ์" | "打印"
    ) {
        return Cat::Print;
    }
    if matches!(name, "read_file" | "write_file" | "อ่านไฟล์" | "เขียนไฟล์")
    {
        return Cat::File;
    }
    if matches!(
        name,
        "system" | "get_args" | "time_now" | "sleep" | "sleep_ms"
    ) {
        return Cat::Sys;
    }

    // ── Animation / timing (Anima organic drivers + classic timing) ──
    if matches!(
        name,
        "animation"
            | "ease"
            | "tick"
            | "delta_time"
            | "frame_count"
            | "frame"
            | "record_frame"
            | "record_count"
            | "tween"
            | "tween_ease"
            | "breathe"
            | "wobble"
            | "gait_phase"
            | "gait_swing"
            | "gait_lift"
            | "spring_to"
            | "ik2"
    ) {
        return Cat::Anim;
    }
    // ── Anima mechanical drivers — reuse the gear (Cog) glyph ──
    if matches!(
        name,
        "gear_couple" | "gear_train" | "cam_lift" | "piston" | "rack"
    ) {
        return Cat::Cog;
    }

    Cat::User
}

pub(crate) fn is_vtex(c: Cat) -> bool {
    matches!(
        c,
        Cat::Rings
            | Cat::Spiral
            | Cat::Star
            | Cat::Flower
            | Cat::Lotus
            | Cat::Chakra
            | Cat::Yantra
            | Cat::Hyper
            | Cat::Tess
            | Cat::Rain
            | Cat::Grid
            | Cat::Halftone
            | Cat::Pagoda
            | Cat::Torii
            | Cat::Cog
    )
}
pub(crate) fn is_audio(c: Cat) -> bool {
    matches!(
        c,
        Cat::Tone | Cat::Vol | Cat::Listen | Cat::Sfx | Cat::Spectrum
    )
}
pub(crate) fn is_crypto(c: Cat) -> bool {
    matches!(
        c,
        Cat::Hash | Cat::Cipher | Cat::Sign | Cat::Kem | Cat::Shard
    )
}
pub(crate) fn is_physics(c: Cat) -> bool {
    matches!(
        c,
        Cat::Rigid | Cat::Soft | Cat::Liquid | Cat::Force | Cat::Collide
    )
}
pub(crate) fn is_ai(c: Cat) -> bool {
    matches!(c, Cat::Neural | Cat::Behavior)
}

const ENTRY_NAMES: &[&str] = &[
    "start",
    "main",
    "启",
    "เริ่ม",
    "시작",
    "начать",
    "начало",
    "inicio",
    "comenzar",
    "début",
    "commencer",
    "anfang",
    "starten",
    "início",
];
fn is_entry(name: &str) -> bool {
    ENTRY_NAMES.contains(&name)
}

// ── Data model ────────────────────────────────────────────────────────────────

struct GlobalConst {
    name: String,
    value: String,
}

#[derive(Clone)]
struct CallItem {
    name: String,
    cat: Cat,
    count: usize,
}

struct FuncCard {
    name: String,
    params: Vec<String>,
    calls: Vec<CallItem>,
    has_loop: bool,
    is_entry: bool,
    vtex_count: usize,
    audio_count: usize,
}

struct Document {
    filename: String,
    globals: Vec<GlobalConst>,
    funcs: Vec<FuncCard>,
    #[allow(dead_code)]
    fn_names: HashSet<String>,
}

// ── AST walking ───────────────────────────────────────────────────────────────

struct RawCall {
    name: String,
    cat: Cat,
}

fn walk_stmts(stmts: &[Stmt], fns: &HashSet<String>, out: &mut Vec<RawCall>, loop_: &mut bool) {
    for s in stmts {
        let e = match s {
            Stmt::Expr(e) | Stmt::Return(e) => e,
            Stmt::Bind(_, e) => e,
        };
        walk_expr(e, fns, out, loop_);
    }
}

fn walk_expr(e: &Expr, fns: &HashSet<String>, out: &mut Vec<RawCall>, loop_: &mut bool) {
    match e {
        Expr::Call(func, args) => {
            if let Expr::Ident(name) = func.as_ref() {
                let cat = if fns.contains(name.as_str()) {
                    let base = categorize(name);
                    if base == Cat::User {
                        Cat::User
                    } else {
                        base
                    }
                } else {
                    categorize(name)
                };
                out.push(RawCall { name: name.clone(), cat });
            }
            for a in args {
                walk_expr(a, fns, out, loop_);
            }
        },
        Expr::While { cond, body } => {
            *loop_ = true;
            walk_expr(cond, fns, out, loop_);
            walk_stmts(body, fns, out, loop_);
        },
        Expr::Do(ss) => walk_stmts(ss, fns, out, loop_),
        Expr::If { cond, then, elseifs, else_body } => {
            walk_expr(cond, fns, out, loop_);
            walk_stmts(then, fns, out, loop_);
            for (c, b) in elseifs {
                walk_expr(c, fns, out, loop_);
                walk_stmts(b, fns, out, loop_);
            }
            if let Some(b) = else_body {
                walk_stmts(b, fns, out, loop_);
            }
        },
        Expr::For { iter, body, .. } => {
            walk_expr(iter, fns, out, loop_);
            walk_stmts(body, fns, out, loop_);
        },
        Expr::BinOp(_, a, b) => {
            walk_expr(a, fns, out, loop_);
            walk_expr(b, fns, out, loop_);
        },
        Expr::Array(es) => {
            for a in es {
                walk_expr(a, fns, out, loop_);
            }
        },
        _ => {},
    }
}

fn aggregate(raw: Vec<RawCall>) -> Vec<CallItem> {
    let mut out: Vec<CallItem> = Vec::new();
    for r in raw {
        if let Some(last) = out.last_mut() {
            if last.name == r.name {
                last.count += 1;
                continue;
            }
        }
        out.push(CallItem { name: r.name, cat: r.cat, count: 1 });
    }
    out
}

fn make_card(
    name: String,
    params: Vec<String>,
    raw: Vec<RawCall>,
    has_loop: bool,
    is_entry: bool,
) -> FuncCard {
    let calls = aggregate(raw);
    let vtex_count = calls
        .iter()
        .filter(|c| is_vtex(c.cat))
        .map(|c| c.count)
        .sum();
    let audio_count = calls
        .iter()
        .filter(|c| is_audio(c.cat))
        .map(|c| c.count)
        .sum();
    FuncCard {
        name,
        params,
        calls,
        has_loop,
        is_entry,
        vtex_count,
        audio_count,
    }
}

impl Document {
    fn build(filename: &str, prog: &Program) -> Self {
        let fn_names: HashSet<String> = prog
            .items
            .iter()
            .filter_map(|i| {
                if let Item::Fn(f) = i {
                    Some(f.name.clone())
                } else {
                    None
                }
            })
            .collect();

        let mut globals = Vec::new();
        let mut entries = Vec::new();
        let mut funcs = Vec::new();

        for item in &prog.items {
            match item {
                Item::Bind(name, expr) => match expr {
                    Expr::Number(n) => globals.push(GlobalConst {
                        name: name.clone(),
                        value: if n.fract() == 0.0 {
                            format!("{}", *n as i64)
                        } else {
                            format!("{:.2}", n)
                        },
                    }),
                    Expr::Do(body) => {
                        let mut raw = Vec::new();
                        let mut lp = false;
                        // Scan inside the do block for a while loop too
                        for s in body {
                            if let Stmt::Expr(Expr::While { body: wb, .. }) = s {
                                lp = true;
                                walk_stmts(wb, &fn_names, &mut raw, &mut lp);
                            }
                        }
                        walk_stmts(body, &fn_names, &mut raw, &mut lp);
                        entries.push(make_card(name.clone(), vec![], raw, lp, is_entry(name)));
                    },
                    _ => {},
                },
                Item::Fn(f) => {
                    let mut raw = Vec::new();
                    let mut lp = false;
                    walk_stmts(&f.body, &fn_names, &mut raw, &mut lp);
                    funcs.push(make_card(f.name.clone(), f.params.clone(), raw, lp, false));
                },
                _ => {},
            }
        }

        entries.extend(funcs);
        Document {
            filename: filename.to_string(),
            globals,
            funcs: entries,
            fn_names,
        }
    }
}

// ── SVG icons ─────────────────────────────────────────────────────────────────
// Each icon is drawn centred at (cx, cy) within a radius of r≈10.

fn xe(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}
fn p(v: f32) -> String {
    format!("{:.2}", v)
}

pub(crate) fn icon(cat: Cat, cx: f32, cy: f32, r: f32) -> String {
    let c = cat.color();
    match cat {
        Cat::Rings => {
            format!(
                r#"<circle cx="{}" cy="{}" r="{}" fill="none" stroke="{c}" stroke-width="1.2" opacity="0.9"/>
                   <circle cx="{}" cy="{}" r="{}" fill="none" stroke="{c}" stroke-width="1.2" opacity="0.65"/>
                   <circle cx="{}" cy="{}" r="{}" fill="none" stroke="{c}" stroke-width="1.2" opacity="0.4"/>"#,
                p(cx),
                p(cy),
                p(r),
                p(cx),
                p(cy),
                p(r * 0.63),
                p(cx),
                p(cy),
                p(r * 0.33)
            )
        },
        Cat::Spiral => {
            // Approximate spiral with two arcs
            let (x0, y0) = (cx, cy - r * 0.1);
            let (x1, y1) = (cx + r * 0.85, cy);
            let (x2, y2) = (cx, cy + r * 0.9);
            let (x3, y3) = (cx - r * 0.85, cy + r * 0.1);
            let (x4, y4) = (cx, cy - r * 0.9);
            format!(
                r#"<path d="M {},{} C {},{} {},{} {},{} S {},{} {},{} S {},{} {},{} S {},{} {},{}"
                        fill="none" stroke="{c}" stroke-width="1.4" opacity="0.9" stroke-linecap="round"/>"#,
                p(x0),
                p(y0),
                p(cx + r),
                p(y0),
                p(x1),
                p(cy - r * 0.5),
                p(x1),
                p(y1),
                p(cx + r * 0.3),
                p(y2),
                p(x2),
                p(y2),
                p(x3),
                p(y3 - r * 0.4),
                p(x3),
                p(y3),
                p(cx),
                p(y4),
                p(x4),
                p(y4)
            )
        },
        Cat::Star => {
            // 5-point star
            let pts: String = (0..5)
                .flat_map(|i| {
                    let ao = (i as f32 * 72.0 - 90.0).to_radians();
                    let ai = ao + 36.0_f32.to_radians();
                    let ri = r * 0.42;
                    vec![
                        format!("{},{}", p(cx + r * ao.cos()), p(cy + r * ao.sin())),
                        format!("{},{}", p(cx + ri * ai.cos()), p(cy + ri * ai.sin())),
                    ]
                })
                .collect::<Vec<_>>()
                .join(" ");
            format!(r#"<polygon points="{pts}" fill="{c}" opacity="0.85"/>"#)
        },
        Cat::Flower => {
            // 6 petals as ellipses
            (0..6)
                .map(|i| {
                    let angle = i as f32 * 60.0;
                    format!(
                        r#"<ellipse cx="{}" cy="{}" rx="{}" ry="{}"
                           fill="{c}" opacity="0.5"
                           transform="rotate({angle},{},{})"/>"#,
                        p(cx),
                        p(cy - r * 0.45),
                        p(r * 0.28),
                        p(r * 0.52),
                        p(cx),
                        p(cy)
                    )
                })
                .collect::<String>()
        },
        Cat::Lotus => {
            // 8 petals, more elongated
            (0..8)
                .map(|i| {
                    let angle = i as f32 * 45.0;
                    format!(
                        r#"<ellipse cx="{}" cy="{}" rx="{}" ry="{}"
                           fill="{c}" opacity="0.45"
                           transform="rotate({angle},{},{})"/>"#,
                        p(cx),
                        p(cy - r * 0.50),
                        p(r * 0.22),
                        p(r * 0.55),
                        p(cx),
                        p(cy)
                    )
                })
                .collect::<String>()
                + &format!(
                    r#"<circle cx="{}" cy="{}" r="{}" fill="{c}" opacity="0.8"/>"#,
                    p(cx),
                    p(cy),
                    p(r * 0.22)
                )
        },
        Cat::Chakra => {
            // Central circle + 8 spokes
            let spokes: String = (0..8).map(|i| {
                let a = (i as f32 * 45.0).to_radians();
                format!(r#"<line x1="{}" y1="{}" x2="{}" y2="{}" stroke="{c}" stroke-width="1.2" opacity="0.8"/>"#,
                    p(cx + r*0.28*a.cos()), p(cy + r*0.28*a.sin()),
                    p(cx + r*0.88*a.cos()), p(cy + r*0.88*a.sin()))
            }).collect();
            spokes
                + &format!(
                    r#"<circle cx="{}" cy="{}" r="{}" fill="none" stroke="{c}" stroke-width="1.2" opacity="0.7"/>
                   <circle cx="{}" cy="{}" r="{}" fill="{c}" opacity="0.8"/>"#,
                    p(cx),
                    p(cy),
                    p(r * 0.88),
                    p(cx),
                    p(cy),
                    p(r * 0.2)
                )
        },
        Cat::Yantra => {
            // Hexagram (Star of David) from two triangles
            let h = r * 0.866;
            let t1 = format!(
                "{},{} {},{} {},{}",
                p(cx),
                p(cy - r),
                p(cx + h),
                p(cy + r * 0.5),
                p(cx - h),
                p(cy + r * 0.5)
            );
            let t2 = format!(
                "{},{} {},{} {},{}",
                p(cx),
                p(cy + r),
                p(cx - h),
                p(cy - r * 0.5),
                p(cx + h),
                p(cy - r * 0.5)
            );
            format!(
                r#"<polygon points="{t1}" fill="none" stroke="{c}" stroke-width="1.3" opacity="0.85"/>
                   <polygon points="{t2}" fill="none" stroke="{c}" stroke-width="1.3" opacity="0.85"/>
                   <circle cx="{}" cy="{}" r="{}" fill="{c}" opacity="0.25"/>"#,
                p(cx),
                p(cy),
                p(r * 0.38)
            )
        },
        Cat::Hyper => {
            // Radial + concentric = Poincaré disc
            let rays: String = (0..6).map(|i| {
                let a = (i as f32 * 30.0).to_radians();
                format!(r#"<line x1="{}" y1="{}" x2="{}" y2="{}" stroke="{c}" stroke-width="1.0" opacity="0.55"/>"#,
                    p(cx - r*a.cos()), p(cy - r*a.sin()),
                    p(cx + r*a.cos()), p(cy + r*a.sin()))
            }).collect();
            rays + &format!(
                r#"<circle cx="{}" cy="{}" r="{}" fill="none" stroke="{c}" stroke-width="1.0" opacity="0.8"/>
                   <circle cx="{}" cy="{}" r="{}" fill="none" stroke="{c}" stroke-width="1.0" opacity="0.5"/>
                   <circle cx="{}" cy="{}" r="{}" fill="none" stroke="{c}" stroke-width="1.0" opacity="0.3"/>"#,
                p(cx),
                p(cy),
                p(r),
                p(cx),
                p(cy),
                p(r * 0.6),
                p(cx),
                p(cy),
                p(r * 0.3)
            )
        },
        Cat::Tess => {
            // Wavy horizontal lines
            (0..4)
                .map(|i| {
                    let y = cy - r * 0.7 + i as f32 * r * 0.46;
                    let amp = r * 0.15;
                    format!(
                        r#"<path d="M {},{} C {},{} {},{} {},{} S {},{} {},{}"
                            fill="none" stroke="{c}" stroke-width="1.1" opacity="0.7"/>"#,
                        p(cx - r),
                        p(y),
                        p(cx - r + r * 0.4),
                        p(y - amp),
                        p(cx - r * 0.1),
                        p(y - amp),
                        p(cx),
                        p(y),
                        p(cx + r * 0.5),
                        p(y + amp),
                        p(cx + r),
                        p(y)
                    )
                })
                .collect::<String>()
        },
        Cat::Rain => {
            // Falling dashes (letter rain columns)
            (0..5).flat_map(|col| {
                let x = cx - r*0.9 + col as f32 * r*0.45;
                (0..3).map(move |row| {
                    let y = cy - r*0.8 + row as f32 * r*0.55;
                    let opa = 0.9 - row as f32 * 0.22;
                    format!(r#"<rect x="{}" y="{}" width="{}" height="{}" rx="1" fill="{c}" opacity="{:.2}"/>"#,
                        p(x - r*0.06), p(y), p(r*0.12), p(r*0.30), opa)
                })
            }).collect::<String>()
        },
        Cat::Grid => {
            // # hash grid
            let n = 3;
            let step = r * 2.0 / n as f32;
            let mut s = String::new();
            for i in 0..=n {
                let off = -r + i as f32 * step;
                write!(s, r#"<line x1="{}" y1="{}" x2="{}" y2="{}" stroke="{c}" stroke-width="1.0" opacity="0.65"/>
                             <line x1="{}" y1="{}" x2="{}" y2="{}" stroke="{c}" stroke-width="1.0" opacity="0.65"/>"#,
                    p(cx+off),p(cy-r), p(cx+off),p(cy+r),
                    p(cx-r),p(cy+off), p(cx+r),p(cy+off)).ok();
            }
            s
        },
        Cat::Halftone => {
            // 4×4 dot grid
            (0..4)
                .flat_map(|row| {
                    (0..4).map(move |col| {
                        let x = cx - r * 0.75 + col as f32 * r * 0.5;
                        let y = cy - r * 0.75 + row as f32 * r * 0.5;
                        format!(
                            r#"<circle cx="{}" cy="{}" r="{}" fill="{c}" opacity="0.6"/>"#,
                            p(x),
                            p(y),
                            p(r * 0.14)
                        )
                    })
                })
                .collect::<String>()
        },
        Cat::Tone => {
            // Sine wave
            let x0 = cx - r;
            let x3 = cx + r;
            let xm = cx;
            let amp = r * 0.65;
            format!(
                r#"<path d="M {},{} C {},{} {},{} {},{} S {},{} {},{}"
                        fill="none" stroke="{c}" stroke-width="1.6" opacity="0.9"/>"#,
                p(x0),
                p(cy),
                p(x0 + r * 0.5),
                p(cy - amp),
                p(xm - r * 0.1),
                p(cy - amp),
                p(xm),
                p(cy),
                p(xm + r * 0.6),
                p(cy + amp),
                p(x3),
                p(cy)
            )
        },
        Cat::Vol | Cat::Listen => {
            // Concentric arcs (speaker/ear)
            let arcs: String = (1..=3)
                .map(|i| {
                    let ri = r * 0.3 * i as f32;
                    format!(
                        r#"<path d="M {},{} A {ri},{ri} 0 0,1 {},{}"
                            fill="none" stroke="{c}" stroke-width="1.3" opacity="{:.2}"/>"#,
                        p(cx),
                        p(cy - ri),
                        p(cx),
                        p(cy + ri),
                        1.0 - i as f32 * 0.22
                    )
                })
                .collect();
            arcs + &format!(
                r#"<circle cx="{}" cy="{}" r="{}" fill="{c}" opacity="0.7"/>"#,
                p(cx - r * 0.15),
                p(cy),
                p(r * 0.22)
            )
        },
        Cat::Camera => {
            // Lens: outer circle + inner circle + crosshair dot
            format!(
                r#"<circle cx="{}" cy="{}" r="{}" fill="none" stroke="{c}" stroke-width="1.5" opacity="0.8"/>
                   <circle cx="{}" cy="{}" r="{}" fill="none" stroke="{c}" stroke-width="1.2" opacity="0.55"/>
                   <circle cx="{}" cy="{}" r="{}" fill="{c}" opacity="0.9"/>"#,
                p(cx),
                p(cy),
                p(r),
                p(cx),
                p(cy),
                p(r * 0.6),
                p(cx),
                p(cy),
                p(r * 0.18)
            )
        },
        Cat::Light => {
            // Sunburst: central circle + 8 rays
            let rays: String = (0..8).map(|i| {
                let a = (i as f32 * 45.0).to_radians();
                let r1 = r * 0.38;
                let r2 = r * 0.90;
                format!(r#"<line x1="{}" y1="{}" x2="{}" y2="{}" stroke="{c}" stroke-width="1.2" opacity="0.75"/>"#,
                    p(cx + r1*a.cos()), p(cy + r1*a.sin()),
                    p(cx + r2*a.cos()), p(cy + r2*a.sin()))
            }).collect();
            rays + &format!(
                r#"<circle cx="{}" cy="{}" r="{}" fill="{c}" opacity="0.85"/>"#,
                p(cx),
                p(cy),
                p(r * 0.32)
            )
        },
        Cat::Fill => {
            format!(
                r#"<rect x="{}" y="{}" width="{}" height="{}" rx="2" fill="{c}" opacity="0.5"/>"#,
                p(cx - r * 0.9),
                p(cy - r * 0.7),
                p(r * 1.8),
                p(r * 1.4)
            )
        },
        Cat::Present => {
            // Triangle (play/render button)
            let pts = format!(
                "{},{} {},{} {},{}",
                p(cx - r * 0.7),
                p(cy - r * 0.8),
                p(cx + r * 0.8),
                p(cy),
                p(cx - r * 0.7),
                p(cy + r * 0.8)
            );
            format!(r#"<polygon points="{pts}" fill="{c}" opacity="0.7"/>"#)
        },
        Cat::User => {
            // Right-pointing chevron (function call arrow)
            format!(
                r#"<path d="M {},{} L {},{} L {},{}" fill="none" stroke="{c}" stroke-width="1.8"
                         stroke-linecap="round" stroke-linejoin="round" opacity="0.85"/>
                   <path d="M {},{} L {},{} L {},{}" fill="none" stroke="{c}" stroke-width="1.8"
                         stroke-linecap="round" stroke-linejoin="round" opacity="0.5"/>"#,
                p(cx - r * 0.6),
                p(cy - r * 0.7),
                p(cx + r * 0.5),
                p(cy),
                p(cx - r * 0.6),
                p(cy + r * 0.7),
                p(cx),
                p(cy - r * 0.7),
                p(cx + r * 0.95),
                p(cy),
                p(cx),
                p(cy + r * 0.7)
            )
        },
        Cat::Loop => {
            // Circular arrow
            format!(
                r#"<path d="M {},{} A {},{} 0 1,1 {},{}"
                        fill="none" stroke="{c}" stroke-width="1.6" opacity="0.9"/>
                   <polygon points="{},{} {},{} {},{}" fill="{c}" opacity="0.9"/>"#,
                p(cx),
                p(cy - r * 0.9),
                p(r * 0.9),
                p(r * 0.9),
                p(cx + r * 0.4),
                p(cy - r * 0.9),
                p(cx + r * 0.4),
                p(cy - r * 0.9),
                p(cx + r * 0.1),
                p(cy - r * 0.55),
                p(cx + r * 0.8),
                p(cy - r * 0.62)
            )
        },
        // ── Surface textures (extra) ──────────────────────────────────────────
        Cat::Pagoda => {
            let mut s = String::new();
            for i in 0..3 {
                let yy = cy - r * 0.7 + i as f32 * r * 0.55;
                let hw = r * (0.4 + i as f32 * 0.22);
                write!(s, r##"<polyline points="{},{} {},{} {},{}" fill="none" stroke="{c}" stroke-width="1.3" opacity="0.85" stroke-linejoin="round"/>"##,
                    p(cx - hw), p(yy), p(cx), p(yy - r*0.35), p(cx + hw), p(yy)).ok();
            }
            write!(s, r##"<line x1="{}" y1="{}" x2="{}" y2="{}" stroke="{c}" stroke-width="1.0" opacity="0.55"/>"##,
                p(cx), p(cy - r*1.05), p(cx), p(cy + r*0.55)).ok();
            s
        },
        Cat::Torii => {
            format!(
                r##"<line x1="{}" y1="{}" x2="{}" y2="{}" stroke="{c}" stroke-width="1.6" opacity="0.9"/>
                   <line x1="{}" y1="{}" x2="{}" y2="{}" stroke="{c}" stroke-width="1.3" opacity="0.85"/>
                   <line x1="{}" y1="{}" x2="{}" y2="{}" stroke="{c}" stroke-width="1.5" opacity="0.9"/>
                   <line x1="{}" y1="{}" x2="{}" y2="{}" stroke="{c}" stroke-width="1.5" opacity="0.9"/>"##,
                p(cx - r * 0.95),
                p(cy - r * 0.6),
                p(cx + r * 0.95),
                p(cy - r * 0.6),
                p(cx - r * 0.75),
                p(cy - r * 0.2),
                p(cx + r * 0.75),
                p(cy - r * 0.2),
                p(cx - r * 0.55),
                p(cy - r * 0.6),
                p(cx - r * 0.55),
                p(cy + r * 0.85),
                p(cx + r * 0.55),
                p(cy - r * 0.6),
                p(cx + r * 0.55),
                p(cy + r * 0.85)
            )
        },
        Cat::Cog => {
            let mut s = String::new();
            for i in 0..8 {
                write!(s, r##"<rect x="{}" y="{}" width="{}" height="{}" fill="{c}" opacity="0.8" transform="rotate({},{},{})"/>"##,
                    p(cx - r*0.13), p(cy - r*1.0), p(r*0.26), p(r*0.3), p(i as f32 * 45.0), p(cx), p(cy)).ok();
            }
            write!(
                s,
                r##"<circle cx="{}" cy="{}" r="{}" fill="{c}" opacity="0.55"/>
                         <circle cx="{}" cy="{}" r="{}" fill="#0b0b1a" opacity="0.9"/>"##,
                p(cx),
                p(cy),
                p(r * 0.7),
                p(cx),
                p(cy),
                p(r * 0.3)
            )
            .ok();
            s
        },
        Cat::Fractal => {
            let outer = format!(
                "{},{} {},{} {},{}",
                p(cx),
                p(cy - r * 0.9),
                p(cx + r * 0.85),
                p(cy + r * 0.6),
                p(cx - r * 0.85),
                p(cy + r * 0.6)
            );
            let inner = format!(
                "{},{} {},{} {},{}",
                p(cx),
                p(cy + r * 0.6),
                p(cx - r * 0.42),
                p(cy - r * 0.12),
                p(cx + r * 0.42),
                p(cy - r * 0.12)
            );
            format!(
                r##"<polygon points="{outer}" fill="{c}" opacity="0.4" stroke="{c}" stroke-width="1.0"/>
                   <polygon points="{inner}" fill="#0b0b1a" opacity="0.9"/>"##
            )
        },
        // ── Audio (extra) ─────────────────────────────────────────────────────
        Cat::Sfx => {
            format!(
                r##"<polygon points="{},{} {},{} {},{} {},{} {},{} {},{}" fill="{c}" opacity="0.85"/>"##,
                p(cx + r * 0.2),
                p(cy - r * 0.9),
                p(cx - r * 0.5),
                p(cy + r * 0.1),
                p(cx - r * 0.05),
                p(cy + r * 0.1),
                p(cx - r * 0.2),
                p(cy + r * 0.9),
                p(cx + r * 0.5),
                p(cy - r * 0.1),
                p(cx + r * 0.05),
                p(cy - r * 0.1)
            )
        },
        Cat::Spectrum => {
            let hs = [0.5f32, 0.9, 0.35, 0.7, 0.55];
            let mut s = String::new();
            for (i, &hh) in hs.iter().enumerate() {
                let xx = cx - r * 0.8 + i as f32 * r * 0.4;
                let bh = r * 1.6 * hh;
                write!(s, r##"<rect x="{}" y="{}" width="{}" height="{}" rx="1" fill="{c}" opacity="0.8"/>"##,
                    p(xx - r*0.12), p(cy + r*0.8 - bh), p(r*0.24), p(bh)).ok();
            }
            s
        },
        // ── Music / MIDI ──────────────────────────────────────────────────────
        Cat::Music => {
            format!(
                r##"<ellipse cx="{}" cy="{}" rx="{}" ry="{}" fill="{c}" opacity="0.85" transform="rotate(-20,{},{})"/>
                   <line x1="{}" y1="{}" x2="{}" y2="{}" stroke="{c}" stroke-width="1.3" opacity="0.85"/>
                   <path d="M {},{} C {},{} {},{} {},{}" fill="none" stroke="{c}" stroke-width="1.3" opacity="0.85"/>"##,
                p(cx - r * 0.35),
                p(cy + r * 0.55),
                p(r * 0.32),
                p(r * 0.24),
                p(cx - r * 0.35),
                p(cy + r * 0.55),
                p(cx - r * 0.05),
                p(cy + r * 0.5),
                p(cx - r * 0.05),
                p(cy - r * 0.8),
                p(cx - r * 0.05),
                p(cy - r * 0.8),
                p(cx + r * 0.5),
                p(cy - r * 0.65),
                p(cx + r * 0.55),
                p(cy - r * 0.2),
                p(cx + r * 0.3),
                p(cy - r * 0.05)
            )
        },
        Cat::Note => {
            format!(
                r##"<circle cx="{}" cy="{}" r="{}" fill="{c}" opacity="0.85"/>
                   <circle cx="{}" cy="{}" r="{}" fill="{c}" opacity="0.85"/>
                   <line x1="{}" y1="{}" x2="{}" y2="{}" stroke="{c}" stroke-width="1.2" opacity="0.85"/>
                   <line x1="{}" y1="{}" x2="{}" y2="{}" stroke="{c}" stroke-width="1.2" opacity="0.85"/>
                   <line x1="{}" y1="{}" x2="{}" y2="{}" stroke="{c}" stroke-width="2.0" opacity="0.85"/>"##,
                p(cx - r * 0.55),
                p(cy + r * 0.55),
                p(r * 0.24),
                p(cx + r * 0.55),
                p(cy + r * 0.3),
                p(r * 0.24),
                p(cx - r * 0.33),
                p(cy + r * 0.55),
                p(cx - r * 0.33),
                p(cy - r * 0.6),
                p(cx + r * 0.77),
                p(cy + r * 0.3),
                p(cx + r * 0.77),
                p(cy - r * 0.85),
                p(cx - r * 0.33),
                p(cy - r * 0.6),
                p(cx + r * 0.77),
                p(cy - r * 0.85)
            )
        },
        // ── Cryptography ──────────────────────────────────────────────────────
        Cat::Hash => {
            let mut s = String::new();
            for i in 0..3 {
                let rr = r * (0.35 + i as f32 * 0.22);
                write!(s, r##"<path d="M {},{} A {},{} 0 1,1 {},{}" fill="none" stroke="{c}" stroke-width="1.1" opacity="{:.2}"/>"##,
                    p(cx - rr), p(cy), p(rr), p(rr), p(cx + rr), p(cy), 0.85 - i as f32*0.18).ok();
            }
            write!(
                s,
                r##"<circle cx="{}" cy="{}" r="{}" fill="{c}" opacity="0.8"/>"##,
                p(cx),
                p(cy),
                p(r * 0.12)
            )
            .ok();
            s
        },
        Cat::Cipher => {
            format!(
                r##"<path d="M {},{} A {},{} 0 0,1 {},{}" fill="none" stroke="{c}" stroke-width="1.4" opacity="0.85"/>
                   <rect x="{}" y="{}" width="{}" height="{}" rx="2" fill="{c}" opacity="0.5" stroke="{c}" stroke-width="1.1"/>
                   <circle cx="{}" cy="{}" r="{}" fill="#0b0b1a" opacity="0.9"/>"##,
                p(cx - r * 0.45),
                p(cy - r * 0.05),
                p(r * 0.45),
                p(r * 0.45),
                p(cx + r * 0.45),
                p(cy - r * 0.05),
                p(cx - r * 0.7),
                p(cy - r * 0.05),
                p(r * 1.4),
                p(r * 0.95),
                p(cx),
                p(cy + r * 0.4),
                p(r * 0.14)
            )
        },
        Cat::Sign => {
            format!(
                r##"<path d="M {},{} C {},{} {},{} {},{} S {},{} {},{}" fill="none" stroke="{c}" stroke-width="1.4" opacity="0.9" stroke-linecap="round"/>
                   <line x1="{}" y1="{}" x2="{}" y2="{}" stroke="{c}" stroke-width="0.8" opacity="0.5"/>"##,
                p(cx - r * 0.9),
                p(cy + r * 0.4),
                p(cx - r * 0.4),
                p(cy - r * 0.7),
                p(cx),
                p(cy + r * 0.6),
                p(cx + r * 0.3),
                p(cy - r * 0.1),
                p(cx + r * 0.7),
                p(cy - r * 0.7),
                p(cx + r * 0.9),
                p(cy + r * 0.3),
                p(cx - r),
                p(cy + r * 0.75),
                p(cx + r),
                p(cy + r * 0.75)
            )
        },
        Cat::Kem => {
            format!(
                r##"<circle cx="{}" cy="{}" r="{}" fill="none" stroke="{c}" stroke-width="1.4" opacity="0.9"/>
                   <line x1="{}" y1="{}" x2="{}" y2="{}" stroke="{c}" stroke-width="1.4" opacity="0.9"/>
                   <line x1="{}" y1="{}" x2="{}" y2="{}" stroke="{c}" stroke-width="1.3" opacity="0.85"/>
                   <line x1="{}" y1="{}" x2="{}" y2="{}" stroke="{c}" stroke-width="1.3" opacity="0.85"/>"##,
                p(cx - r * 0.5),
                p(cy - r * 0.4),
                p(r * 0.4),
                p(cx - r * 0.18),
                p(cy - r * 0.12),
                p(cx + r * 0.8),
                p(cy + r * 0.75),
                p(cx + r * 0.55),
                p(cy + r * 0.5),
                p(cx + r * 0.8),
                p(cy + r * 0.25),
                p(cx + r * 0.8),
                p(cy + r * 0.75),
                p(cx + r * 1.05),
                p(cy + r * 0.5)
            )
        },
        Cat::Shard => {
            format!(
                r##"<polygon points="{},{} {},{} {},{}" fill="{c}" opacity="0.5"/>
                   <polygon points="{},{} {},{} {},{}" fill="{c}" opacity="0.3"/>
                   <polygon points="{},{} {},{} {},{}" fill="none" stroke="{c}" stroke-width="1.0" opacity="0.8"/>"##,
                p(cx),
                p(cy - r),
                p(cx - r * 0.85),
                p(cy + r * 0.5),
                p(cx),
                p(cy + r * 0.2),
                p(cx),
                p(cy - r),
                p(cx + r * 0.85),
                p(cy + r * 0.5),
                p(cx),
                p(cy + r * 0.2),
                p(cx),
                p(cy - r),
                p(cx + r * 0.85),
                p(cy + r * 0.5),
                p(cx - r * 0.85),
                p(cy + r * 0.5)
            )
        },
        // ── Physics ───────────────────────────────────────────────────────────
        Cat::Rigid => {
            format!(
                r##"<polygon points="{},{} {},{} {},{} {},{}" fill="{c}" opacity="0.35" stroke="{c}" stroke-width="1.0"/>
                   <path d="M {},{} L {},{} L {},{} L {},{} Z" fill="{c}" opacity="0.2" stroke="{c}" stroke-width="1.0"/>
                   <path d="M {},{} L {},{} L {},{} L {},{} Z" fill="{c}" opacity="0.28" stroke="{c}" stroke-width="1.0"/>"##,
                p(cx),
                p(cy - r * 0.9),
                p(cx + r * 0.85),
                p(cy - r * 0.4),
                p(cx),
                p(cy + r * 0.1),
                p(cx - r * 0.85),
                p(cy - r * 0.4),
                p(cx - r * 0.85),
                p(cy - r * 0.4),
                p(cx),
                p(cy + r * 0.1),
                p(cx),
                p(cy + r * 0.95),
                p(cx - r * 0.85),
                p(cy + r * 0.45),
                p(cx + r * 0.85),
                p(cy - r * 0.4),
                p(cx),
                p(cy + r * 0.1),
                p(cx),
                p(cy + r * 0.95),
                p(cx + r * 0.85),
                p(cy + r * 0.45)
            )
        },
        Cat::Soft => {
            format!(
                r##"<path d="M {},{} C {},{} {},{} {},{} C {},{} {},{} {},{} C {},{} {},{} {},{} C {},{} {},{} {},{} Z" fill="{c}" opacity="0.45" stroke="{c}" stroke-width="1.0"/>"##,
                p(cx),
                p(cy - r * 0.85),
                p(cx + r * 0.7),
                p(cy - r * 0.9),
                p(cx + r * 0.95),
                p(cy - r * 0.2),
                p(cx + r * 0.8),
                p(cy + r * 0.4),
                p(cx + r * 0.6),
                p(cy + r * 0.95),
                p(cx - r * 0.1),
                p(cy + r * 0.9),
                p(cx - r * 0.6),
                p(cy + r * 0.7),
                p(cx - r * 0.95),
                p(cy + r * 0.5),
                p(cx - r * 0.9),
                p(cy - r * 0.3),
                p(cx - r * 0.7),
                p(cy - r * 0.6),
                p(cx - r * 0.5),
                p(cy - r * 0.85),
                p(cx - r * 0.2),
                p(cy - r * 0.95),
                p(cx),
                p(cy - r * 0.85)
            )
        },
        Cat::Liquid => {
            format!(
                r##"<path d="M {},{} C {},{} {},{} {},{} C {},{} {},{} {},{} Z" fill="{c}" opacity="0.5" stroke="{c}" stroke-width="1.0"/>
                   <ellipse cx="{}" cy="{}" rx="{}" ry="{}" fill="#ffffff" opacity="0.25"/>"##,
                p(cx),
                p(cy - r * 0.9),
                p(cx + r * 0.75),
                p(cy - r * 0.1),
                p(cx + r * 0.6),
                p(cy + r * 0.8),
                p(cx),
                p(cy + r * 0.85),
                p(cx - r * 0.6),
                p(cy + r * 0.8),
                p(cx - r * 0.75),
                p(cy - r * 0.1),
                p(cx),
                p(cy - r * 0.9),
                p(cx - r * 0.25),
                p(cy + r * 0.25),
                p(r * 0.16),
                p(r * 0.28)
            )
        },
        Cat::Force => {
            format!(
                r##"<line x1="{}" y1="{}" x2="{}" y2="{}" stroke="{c}" stroke-width="1.8" opacity="0.9" stroke-linecap="round"/>
                   <polygon points="{},{} {},{} {},{}" fill="{c}" opacity="0.9"/>"##,
                p(cx - r * 0.7),
                p(cy + r * 0.7),
                p(cx + r * 0.45),
                p(cy - r * 0.45),
                p(cx + r * 0.8),
                p(cy - r * 0.8),
                p(cx + r * 0.2),
                p(cy - r * 0.7),
                p(cx + r * 0.7),
                p(cy - r * 0.15)
            )
        },
        Cat::Collide => {
            let mut s = String::new();
            for i in 0..8 {
                let a = (i as f32 * 45.0).to_radians();
                let r1 = r * 0.3;
                let r2 = if i % 2 == 0 { r * 0.95 } else { r * 0.6 };
                write!(s, r##"<line x1="{}" y1="{}" x2="{}" y2="{}" stroke="{c}" stroke-width="1.3" opacity="0.85"/>"##,
                    p(cx + r1*a.cos()), p(cy + r1*a.sin()), p(cx + r2*a.cos()), p(cy + r2*a.sin())).ok();
            }
            write!(
                s,
                r##"<circle cx="{}" cy="{}" r="{}" fill="{c}" opacity="0.8"/>"##,
                p(cx),
                p(cy),
                p(r * 0.18)
            )
            .ok();
            s
        },
        // ── 3D mesh & drawing ─────────────────────────────────────────────────
        Cat::Mesh => {
            format!(
                r##"<circle cx="{}" cy="{}" r="{}" fill="none" stroke="{c}" stroke-width="1.1" opacity="0.85"/>
                   <ellipse cx="{}" cy="{}" rx="{}" ry="{}" fill="none" stroke="{c}" stroke-width="0.9" opacity="0.6"/>
                   <ellipse cx="{}" cy="{}" rx="{}" ry="{}" fill="none" stroke="{c}" stroke-width="0.9" opacity="0.6"/>
                   <line x1="{}" y1="{}" x2="{}" y2="{}" stroke="{c}" stroke-width="0.9" opacity="0.6"/>"##,
                p(cx),
                p(cy),
                p(r * 0.9),
                p(cx),
                p(cy),
                p(r * 0.38),
                p(r * 0.9),
                p(cx),
                p(cy),
                p(r * 0.9),
                p(r * 0.38),
                p(cx - r * 0.9),
                p(cy),
                p(cx + r * 0.9),
                p(cy)
            )
        },
        Cat::Draw3D => {
            format!(
                r##"<line x1="{}" y1="{}" x2="{}" y2="{}" stroke="{c}" stroke-width="1.4" opacity="0.9"/>
                   <line x1="{}" y1="{}" x2="{}" y2="{}" stroke="{c}" stroke-width="1.4" opacity="0.75"/>
                   <line x1="{}" y1="{}" x2="{}" y2="{}" stroke="{c}" stroke-width="1.4" opacity="0.6"/>
                   <circle cx="{}" cy="{}" r="{}" fill="{c}" opacity="0.9"/>"##,
                p(cx),
                p(cy + r * 0.3),
                p(cx),
                p(cy - r * 0.9),
                p(cx),
                p(cy + r * 0.3),
                p(cx + r * 0.9),
                p(cy + r * 0.7),
                p(cx),
                p(cy + r * 0.3),
                p(cx - r * 0.85),
                p(cy + r * 0.6),
                p(cx),
                p(cy + r * 0.3),
                p(r * 0.13)
            )
        },
        Cat::Draw2D => {
            format!(
                r##"<line x1="{}" y1="{}" x2="{}" y2="{}" stroke="{c}" stroke-width="1.6" opacity="0.9" stroke-linecap="round"/>
                   <circle cx="{}" cy="{}" r="{}" fill="{c}" opacity="0.9"/>
                   <circle cx="{}" cy="{}" r="{}" fill="{c}" opacity="0.9"/>"##,
                p(cx - r * 0.8),
                p(cy + r * 0.8),
                p(cx + r * 0.8),
                p(cy - r * 0.8),
                p(cx - r * 0.8),
                p(cy + r * 0.8),
                p(r * 0.16),
                p(cx + r * 0.8),
                p(cy - r * 0.8),
                p(r * 0.16)
            )
        },
        // ── Math ──────────────────────────────────────────────────────────────
        Cat::Trig => {
            format!(
                r##"<polygon points="{},{} {},{} {},{}" fill="none" stroke="{c}" stroke-width="1.3" opacity="0.85"/>
                   <path d="M {},{} A {},{} 0 0,0 {},{}" fill="none" stroke="{c}" stroke-width="1.0" opacity="0.7"/>"##,
                p(cx - r * 0.8),
                p(cy + r * 0.7),
                p(cx + r * 0.8),
                p(cy + r * 0.7),
                p(cx - r * 0.8),
                p(cy - r * 0.7),
                p(cx - r * 0.35),
                p(cy + r * 0.7),
                p(r * 0.45),
                p(r * 0.45),
                p(cx - r * 0.8),
                p(cy + r * 0.38)
            )
        },
        Cat::MathFn => {
            format!(
                r##"<line x1="{}" y1="{}" x2="{}" y2="{}" stroke="{c}" stroke-width="0.8" opacity="0.4"/>
                   <line x1="{}" y1="{}" x2="{}" y2="{}" stroke="{c}" stroke-width="0.8" opacity="0.4"/>
                   <path d="M {},{} Q {},{} {},{}" fill="none" stroke="{c}" stroke-width="1.5" opacity="0.9"/>"##,
                p(cx - r * 0.9),
                p(cy),
                p(cx + r * 0.9),
                p(cy),
                p(cx),
                p(cy - r * 0.9),
                p(cx),
                p(cy + r * 0.9),
                p(cx - r * 0.8),
                p(cy - r * 0.7),
                p(cx),
                p(cy + r * 1.1),
                p(cx + r * 0.8),
                p(cy - r * 0.7)
            )
        },
        Cat::Noise => {
            format!(
                r##"<polyline points="{},{} {},{} {},{} {},{} {},{} {},{} {},{} {},{} {},{}" fill="none" stroke="{c}" stroke-width="1.3" opacity="0.85" stroke-linejoin="round"/>"##,
                p(cx - r * 0.9),
                p(cy + r * 0.1),
                p(cx - r * 0.65),
                p(cy - r * 0.6),
                p(cx - r * 0.4),
                p(cy + r * 0.5),
                p(cx - r * 0.15),
                p(cy - r * 0.4),
                p(cx + r * 0.1),
                p(cy + r * 0.7),
                p(cx + r * 0.35),
                p(cy - r * 0.5),
                p(cx + r * 0.6),
                p(cy + r * 0.3),
                p(cx + r * 0.8),
                p(cy - r * 0.3),
                p(cx + r * 0.95),
                p(cy + r * 0.2)
            )
        },
        // ── Networking & AI ───────────────────────────────────────────────────
        Cat::Net => {
            let mut s = String::new();
            let nodes = [(0.0f32, -0.85f32), (0.8, 0.4), (-0.8, 0.4)];
            for &(nx, ny) in nodes.iter() {
                write!(s, r##"<line x1="{}" y1="{}" x2="{}" y2="{}" stroke="{c}" stroke-width="1.0" opacity="0.6"/>"##,
                    p(cx), p(cy), p(cx + nx*r), p(cy + ny*r)).ok();
            }
            write!(
                s,
                r##"<circle cx="{}" cy="{}" r="{}" fill="{c}" opacity="0.9"/>"##,
                p(cx),
                p(cy),
                p(r * 0.2)
            )
            .ok();
            for &(nx, ny) in nodes.iter() {
                write!(
                    s,
                    r##"<circle cx="{}" cy="{}" r="{}" fill="{c}" opacity="0.8"/>"##,
                    p(cx + nx * r),
                    p(cy + ny * r),
                    p(r * 0.18)
                )
                .ok();
            }
            s
        },
        Cat::Neural => {
            let mut s = String::new();
            let l0 = [-0.6f32, 0.0, 0.6];
            let l1 = [-0.3f32, 0.3];
            let x0 = cx - r * 0.7;
            let x1 = cx;
            let x2 = cx + r * 0.7;
            for &a in l0.iter() {
                for &b in l1.iter() {
                    write!(s, r##"<line x1="{}" y1="{}" x2="{}" y2="{}" stroke="{c}" stroke-width="0.6" opacity="0.4"/>"##,
                    p(x0), p(cy + a*r), p(x1), p(cy + b*r)).ok();
                }
            }
            for &b in l1.iter() {
                write!(s, r##"<line x1="{}" y1="{}" x2="{}" y2="{}" stroke="{c}" stroke-width="0.6" opacity="0.4"/>"##,
                    p(x1), p(cy + b*r), p(x2), p(cy)).ok();
            }
            for &a in l0.iter() {
                write!(
                    s,
                    r##"<circle cx="{}" cy="{}" r="{}" fill="{c}" opacity="0.85"/>"##,
                    p(x0),
                    p(cy + a * r),
                    p(r * 0.14)
                )
                .ok();
            }
            for &b in l1.iter() {
                write!(
                    s,
                    r##"<circle cx="{}" cy="{}" r="{}" fill="{c}" opacity="0.85"/>"##,
                    p(x1),
                    p(cy + b * r),
                    p(r * 0.14)
                )
                .ok();
            }
            write!(
                s,
                r##"<circle cx="{}" cy="{}" r="{}" fill="{c}" opacity="0.85"/>"##,
                p(x2),
                p(cy),
                p(r * 0.14)
            )
            .ok();
            s
        },
        Cat::Behavior => {
            format!(
                r##"<line x1="{}" y1="{}" x2="{}" y2="{}" stroke="{c}" stroke-width="1.0" opacity="0.7"/>
                   <line x1="{}" y1="{}" x2="{}" y2="{}" stroke="{c}" stroke-width="1.0" opacity="0.7"/>
                   <circle cx="{}" cy="{}" r="{}" fill="{c}" opacity="0.85"/>
                   <circle cx="{}" cy="{}" r="{}" fill="{c}" opacity="0.8"/>
                   <circle cx="{}" cy="{}" r="{}" fill="{c}" opacity="0.8"/>"##,
                p(cx),
                p(cy - r * 0.6),
                p(cx - r * 0.7),
                p(cy + r * 0.6),
                p(cx),
                p(cy - r * 0.6),
                p(cx + r * 0.7),
                p(cy + r * 0.6),
                p(cx),
                p(cy - r * 0.6),
                p(r * 0.2),
                p(cx - r * 0.7),
                p(cy + r * 0.6),
                p(r * 0.18),
                p(cx + r * 0.7),
                p(cy + r * 0.6),
                p(r * 0.18)
            )
        },
        // ── UI / dialog / holograms ───────────────────────────────────────────
        Cat::Widget => {
            format!(
                r##"<rect x="{}" y="{}" width="{}" height="{}" rx="2" fill="none" stroke="{c}" stroke-width="1.2" opacity="0.85"/>
                   <line x1="{}" y1="{}" x2="{}" y2="{}" stroke="{c}" stroke-width="1.0" opacity="0.6"/>
                   <rect x="{}" y="{}" width="{}" height="{}" rx="2" fill="{c}" opacity="0.5"/>"##,
                p(cx - r * 0.85),
                p(cy - r * 0.75),
                p(r * 1.7),
                p(r * 1.5),
                p(cx - r * 0.85),
                p(cy - r * 0.3),
                p(cx + r * 0.85),
                p(cy - r * 0.3),
                p(cx - r * 0.5),
                p(cy + r * 0.1),
                p(r * 1.0),
                p(r * 0.45)
            )
        },
        Cat::Hud => {
            format!(
                r##"<circle cx="{}" cy="{}" r="{}" fill="none" stroke="{c}" stroke-width="1.1" opacity="0.85"/>
                   <line x1="{}" y1="{}" x2="{}" y2="{}" stroke="{c}" stroke-width="0.8" opacity="0.6"/>
                   <line x1="{}" y1="{}" x2="{}" y2="{}" stroke="{c}" stroke-width="0.8" opacity="0.6"/>
                   <line x1="{}" y1="{}" x2="{}" y2="{}" stroke="{c}" stroke-width="1.3" opacity="0.9"/>
                   <circle cx="{}" cy="{}" r="{}" fill="{c}" opacity="0.9"/>"##,
                p(cx),
                p(cy),
                p(r * 0.9),
                p(cx - r * 0.9),
                p(cy),
                p(cx + r * 0.9),
                p(cy),
                p(cx),
                p(cy - r * 0.9),
                p(cx),
                p(cy + r * 0.9),
                p(cx),
                p(cy),
                p(cx + r * 0.64),
                p(cy - r * 0.64),
                p(cx + r * 0.4),
                p(cy - r * 0.4),
                p(r * 0.13)
            )
        },
        Cat::Dialog => {
            format!(
                r##"<rect x="{}" y="{}" width="{}" height="{}" rx="4" fill="{c}" opacity="0.4" stroke="{c}" stroke-width="1.0"/>
                   <polygon points="{},{} {},{} {},{}" fill="{c}" opacity="0.4"/>
                   <circle cx="{}" cy="{}" r="{}" fill="#0b0b1a" opacity="0.85"/>
                   <circle cx="{}" cy="{}" r="{}" fill="#0b0b1a" opacity="0.85"/>
                   <circle cx="{}" cy="{}" r="{}" fill="#0b0b1a" opacity="0.85"/>"##,
                p(cx - r * 0.9),
                p(cy - r * 0.8),
                p(r * 1.8),
                p(r * 1.2),
                p(cx - r * 0.4),
                p(cy + r * 0.4),
                p(cx - r * 0.1),
                p(cy + r * 0.4),
                p(cx - r * 0.55),
                p(cy + r * 0.9),
                p(cx - r * 0.4),
                p(cy - r * 0.2),
                p(r * 0.1),
                p(cx),
                p(cy - r * 0.2),
                p(r * 0.1),
                p(cx + r * 0.4),
                p(cy - r * 0.2),
                p(r * 0.1)
            )
        },
        Cat::Holo => {
            let mut s = String::new();
            for i in 0..3 {
                let off = (i as f32 - 1.0) * r * 0.22;
                write!(s, r##"<polygon points="{},{} {},{} {},{}" fill="{c}" opacity="0.3" stroke="{c}" stroke-width="0.8" stroke-opacity="0.7"/>"##,
                    p(cx + off), p(cy - r*0.8), p(cx + off + r*0.8), p(cy + r*0.6), p(cx + off - r*0.8), p(cy + r*0.6)).ok();
            }
            s
        },
        // ── Text & vector ─────────────────────────────────────────────────────
        Cat::Str => {
            let mut s = String::new();
            for &dx in [-0.5f32, 0.3].iter() {
                write!(
                    s,
                    r##"<circle cx="{}" cy="{}" r="{}" fill="{c}" opacity="0.85"/>
                             <circle cx="{}" cy="{}" r="{}" fill="{c}" opacity="0.85"/>"##,
                    p(cx + dx * r),
                    p(cy - r * 0.4),
                    p(r * 0.16),
                    p(cx + dx * r + r * 0.3),
                    p(cy - r * 0.4),
                    p(r * 0.16)
                )
                .ok();
            }
            for j in 0..2 {
                let yy = cy + r * 0.3 + j as f32 * r * 0.45;
                let ww = if j == 0 { r * 1.6 } else { r * 1.0 };
                write!(s, r##"<line x1="{}" y1="{}" x2="{}" y2="{}" stroke="{c}" stroke-width="1.2" opacity="0.6"/>"##,
                    p(cx - r*0.8), p(yy), p(cx - r*0.8 + ww), p(yy)).ok();
            }
            s
        },
        Cat::Font => {
            format!(
                r##"<polyline points="{},{} {},{} {},{}" fill="none" stroke="{c}" stroke-width="1.5" opacity="0.9" stroke-linejoin="round"/>
                   <line x1="{}" y1="{}" x2="{}" y2="{}" stroke="{c}" stroke-width="1.3" opacity="0.85"/>"##,
                p(cx - r * 0.7),
                p(cy + r * 0.8),
                p(cx),
                p(cy - r * 0.85),
                p(cx + r * 0.7),
                p(cy + r * 0.8),
                p(cx - r * 0.38),
                p(cy + r * 0.15),
                p(cx + r * 0.38),
                p(cy + r * 0.15)
            )
        },
        Cat::Vector => {
            format!(
                r##"<path d="M {},{} C {},{} {},{} {},{}" fill="none" stroke="{c}" stroke-width="1.4" opacity="0.9"/>
                   <line x1="{}" y1="{}" x2="{}" y2="{}" stroke="{c}" stroke-width="0.7" opacity="0.5"/>
                   <line x1="{}" y1="{}" x2="{}" y2="{}" stroke="{c}" stroke-width="0.7" opacity="0.5"/>
                   <rect x="{}" y="{}" width="{}" height="{}" fill="{c}" opacity="0.85"/>
                   <rect x="{}" y="{}" width="{}" height="{}" fill="{c}" opacity="0.85"/>
                   <circle cx="{}" cy="{}" r="{}" fill="none" stroke="{c}" stroke-width="1.0" opacity="0.7"/>
                   <circle cx="{}" cy="{}" r="{}" fill="none" stroke="{c}" stroke-width="1.0" opacity="0.7"/>"##,
                p(cx - r * 0.8),
                p(cy + r * 0.7),
                p(cx - r * 0.3),
                p(cy - r * 0.9),
                p(cx + r * 0.3),
                p(cy - r * 0.9),
                p(cx + r * 0.8),
                p(cy + r * 0.7),
                p(cx - r * 0.8),
                p(cy + r * 0.7),
                p(cx - r * 0.3),
                p(cy - r * 0.9),
                p(cx + r * 0.8),
                p(cy + r * 0.7),
                p(cx + r * 0.3),
                p(cy - r * 0.9),
                p(cx - r * 0.9),
                p(cy + r * 0.6),
                p(r * 0.2),
                p(r * 0.2),
                p(cx + r * 0.7),
                p(cy + r * 0.6),
                p(r * 0.2),
                p(r * 0.2),
                p(cx - r * 0.3),
                p(cy - r * 0.9),
                p(r * 0.14),
                p(cx + r * 0.3),
                p(cy - r * 0.9),
                p(r * 0.14)
            )
        },
        // ── Colour / shading / scene ──────────────────────────────────────────
        Cat::Color => {
            format!(
                r##"<circle cx="{}" cy="{}" r="{}" fill="#ff5e8a" opacity="0.5"/>
                   <circle cx="{}" cy="{}" r="{}" fill="#50fa7b" opacity="0.5"/>
                   <circle cx="{}" cy="{}" r="{}" fill="#6ab0f5" opacity="0.5"/>"##,
                p(cx),
                p(cy - r * 0.4),
                p(r * 0.6),
                p(cx - r * 0.45),
                p(cy + r * 0.35),
                p(r * 0.6),
                p(cx + r * 0.45),
                p(cy + r * 0.35),
                p(r * 0.6)
            )
        },
        Cat::Shade => {
            format!(
                r##"<circle cx="{}" cy="{}" r="{}" fill="{c}" opacity="0.25" stroke="{c}" stroke-width="1.0"/>
                   <path d="M {},{} A {},{} 0 0,1 {},{} A {},{} 0 0,0 {},{} Z" fill="{c}" opacity="0.6"/>
                   <circle cx="{}" cy="{}" r="{}" fill="#ffffff" opacity="0.3"/>"##,
                p(cx),
                p(cy),
                p(r * 0.9),
                p(cx),
                p(cy - r * 0.9),
                p(r * 0.9),
                p(r * 0.9),
                p(cx),
                p(cy + r * 0.9),
                p(r * 0.45),
                p(r * 0.9),
                p(cx),
                p(cy - r * 0.9),
                p(cx + r * 0.35),
                p(cy - r * 0.35),
                p(r * 0.18)
            )
        },
        Cat::Window => {
            format!(
                r##"<rect x="{}" y="{}" width="{}" height="{}" rx="2" fill="none" stroke="{c}" stroke-width="1.3" opacity="0.85"/>
                   <line x1="{}" y1="{}" x2="{}" y2="{}" stroke="{c}" stroke-width="1.0" opacity="0.7"/>
                   <line x1="{}" y1="{}" x2="{}" y2="{}" stroke="{c}" stroke-width="1.0" opacity="0.7"/>"##,
                p(cx - r * 0.85),
                p(cy - r * 0.85),
                p(r * 1.7),
                p(r * 1.7),
                p(cx),
                p(cy - r * 0.85),
                p(cx),
                p(cy + r * 0.85),
                p(cx - r * 0.85),
                p(cy),
                p(cx + r * 0.85),
                p(cy)
            )
        },
        // ── Input ─────────────────────────────────────────────────────────────
        Cat::Key => {
            format!(
                r##"<rect x="{}" y="{}" width="{}" height="{}" rx="3" fill="{c}" opacity="0.35" stroke="{c}" stroke-width="1.2"/>
                   <rect x="{}" y="{}" width="{}" height="{}" rx="2" fill="none" stroke="{c}" stroke-width="1.0" opacity="0.7"/>"##,
                p(cx - r * 0.8),
                p(cy - r * 0.8),
                p(r * 1.6),
                p(r * 1.6),
                p(cx - r * 0.45),
                p(cy - r * 0.45),
                p(r * 0.9),
                p(r * 0.9)
            )
        },
        Cat::Mouse => {
            format!(
                r##"<rect x="{}" y="{}" width="{}" height="{}" rx="{}" fill="none" stroke="{c}" stroke-width="1.2" opacity="0.85"/>
                   <line x1="{}" y1="{}" x2="{}" y2="{}" stroke="{c}" stroke-width="1.0" opacity="0.6"/>
                   <rect x="{}" y="{}" width="{}" height="{}" rx="1" fill="{c}" opacity="0.85"/>"##,
                p(cx - r * 0.55),
                p(cy - r * 0.85),
                p(r * 1.1),
                p(r * 1.7),
                p(r * 0.55),
                p(cx),
                p(cy - r * 0.85),
                p(cx),
                p(cy - r * 0.1),
                p(cx - r * 0.08),
                p(cy - r * 0.7),
                p(r * 0.16),
                p(r * 0.4)
            )
        },
        // ── IO / system / timing ──────────────────────────────────────────────
        Cat::Print => {
            let mut s = format!(
                r##"<rect x="{}" y="{}" width="{}" height="{}" rx="1.5" fill="none" stroke="{c}" stroke-width="1.2" opacity="0.85"/>"##,
                p(cx - r * 0.6),
                p(cy - r * 0.85),
                p(r * 1.2),
                p(r * 1.7)
            );
            for j in 0..4 {
                let yy = cy - r * 0.45 + j as f32 * r * 0.35;
                write!(s, r##"<line x1="{}" y1="{}" x2="{}" y2="{}" stroke="{c}" stroke-width="1.0" opacity="0.6"/>"##,
                    p(cx - r*0.4), p(yy), p(cx + r*0.4), p(yy)).ok();
            }
            s
        },
        Cat::File => {
            format!(
                r##"<path d="M {},{} L {},{} L {},{} L {},{} L {},{} Z" fill="{c}" opacity="0.35" stroke="{c}" stroke-width="1.1"/>
                   <polyline points="{},{} {},{} {},{}" fill="none" stroke="{c}" stroke-width="1.0" opacity="0.8"/>"##,
                p(cx - r * 0.6),
                p(cy - r * 0.85),
                p(cx + r * 0.25),
                p(cy - r * 0.85),
                p(cx + r * 0.6),
                p(cy - r * 0.45),
                p(cx + r * 0.6),
                p(cy + r * 0.85),
                p(cx - r * 0.6),
                p(cy + r * 0.85),
                p(cx + r * 0.25),
                p(cy - r * 0.85),
                p(cx + r * 0.25),
                p(cy - r * 0.45),
                p(cx + r * 0.6),
                p(cy - r * 0.45)
            )
        },
        Cat::Sys => {
            format!(
                r##"<rect x="{}" y="{}" width="{}" height="{}" rx="2" fill="none" stroke="{c}" stroke-width="1.1" opacity="0.8"/>
                   <polyline points="{},{} {},{} {},{}" fill="none" stroke="{c}" stroke-width="1.3" opacity="0.9" stroke-linecap="round" stroke-linejoin="round"/>
                   <line x1="{}" y1="{}" x2="{}" y2="{}" stroke="{c}" stroke-width="1.3" opacity="0.9" stroke-linecap="round"/>"##,
                p(cx - r * 0.9),
                p(cy - r * 0.7),
                p(r * 1.8),
                p(r * 1.4),
                p(cx - r * 0.45),
                p(cy - r * 0.2),
                p(cx - r * 0.1),
                p(cy + r * 0.15),
                p(cx - r * 0.45),
                p(cy + r * 0.5),
                p(cx + r * 0.1),
                p(cy + r * 0.5),
                p(cx + r * 0.5),
                p(cy + r * 0.5)
            )
        },
        Cat::Anim => {
            let mut s = String::new();
            for i in 0..6 {
                let a = (i as f32 * 60.0).to_radians();
                let rad = r * 0.7;
                let dotr = r * (0.08 + 0.16 * (i as f32 / 6.0));
                write!(
                    s,
                    r##"<circle cx="{}" cy="{}" r="{}" fill="{c}" opacity="{:.2}"/>"##,
                    p(cx + rad * a.cos()),
                    p(cy + rad * a.sin()),
                    p(dotr),
                    0.35 + i as f32 * 0.1
                )
                .ok();
            }
            s
        },
        Cat::Const => {
            format!(
                r#"<rect x="{}" y="{}" width="{}" height="{}" rx="2"
                           fill="none" stroke="{c}" stroke-width="1.2" opacity="0.7"/>
                       <line x1="{}" y1="{}" x2="{}" y2="{}" stroke="{c}" stroke-width="1.1" opacity="0.5"/>"#,
                p(cx - r * 0.7),
                p(cy - r * 0.55),
                p(r * 1.4),
                p(r * 1.1),
                p(cx - r * 0.4),
                p(cy),
                p(cx + r * 0.4),
                p(cy)
            )
        },
    }
}

// ── Card rendering ────────────────────────────────────────────────────────────

fn card_height(card: &FuncCard) -> f32 {
    let icon_rows = (card.calls.len() + ICONS_ROW - 1).max(1) / ICONS_ROW + 1;
    let base = CARD_PAD * 2.0          // top+bottom padding
        + 32.0                          // name header
        + 20.0; // params + stats row
    base + icon_rows as f32 * (ICON_SZ + ICON_GAP) + ICON_GAP
}

fn render_card(card: &FuncCard, x: f32, y: f32) -> String {
    let h = card_height(card);
    let w = CARD_W;
    let r = CARD_ROUNDING;

    // Dominant colour from most-common call category (by raw count)
    let dominant = card
        .calls
        .iter()
        .max_by_key(|c| c.count)
        .map(|c| c.cat.color())
        .unwrap_or(Cat::User.color());

    let border_color = if card.is_entry { GOLD } else { dominant };
    let border_w = if card.is_entry { 2.5 } else { 1.2 };
    let glow_filter = if card.is_entry {
        r#" filter="url(#glow-gold)""#
    } else {
        ""
    };

    let mut s = String::new();

    // Card background
    write!(s, r#"<rect x="{}" y="{}" width="{w}" height="{h}" rx="{r}"
                      fill="{CARD_BG}" stroke="{border_color}" stroke-width="{border_w}"{glow_filter}/>
                 <line x1="{}" y1="{}" x2="{}" y2="{}"
                      stroke="{border_color}" stroke-width="3" opacity="0.6"/>
"#,
        p(x), p(y),
        p(x+r), p(y+h), p(x+r), p(y)   // left accent stripe
    ).ok();

    // Function name
    let name_y = y + CARD_PAD + 18.0;
    let entry_badge = if card.is_entry {
        format!(
            r#" <text x="{}" y="{}" fill="{GOLD}" font-size="9" font-weight="bold" opacity="0.8">⬡ ENTRY</text>"#,
            p(x + w - CARD_PAD - 48.0),
            p(name_y)
        )
    } else {
        String::new()
    };

    write!(
        s,
        r#"<text x="{}" y="{}" fill="{}" font-size="13" font-weight="bold">{}</text>{}"#,
        p(x + CARD_PAD + 10.0),
        p(name_y),
        if card.is_entry { GOLD } else { TEXT },
        xe(&card.name),
        entry_badge
    )
    .ok();

    // Params + stats row
    let stats_y = name_y + 18.0;
    let params_str = if card.params.is_empty() {
        String::new()
    } else {
        format!("({})", card.params.join(", "))
    };
    let stats_str = {
        let mut parts = Vec::new();
        if card.vtex_count > 0 {
            parts.push(format!("{} vtex", card.vtex_count));
        }
        if card.audio_count > 0 {
            parts.push(format!("{} audio", card.audio_count));
        }
        if card.has_loop {
            parts.push("↺ loop".into());
        }
        parts.join("  ·  ")
    };
    write!(
        s,
        r#"<text x="{}" y="{}" fill="{TEXT_DIM}" font-size="10">{}</text>
                 <text x="{}" y="{}" fill="{TEXT_DIM}" font-size="10" text-anchor="end">{}</text>
"#,
        p(x + CARD_PAD + 10.0),
        p(stats_y),
        xe(&params_str),
        p(x + w - CARD_PAD),
        p(stats_y),
        xe(&stats_str)
    )
    .ok();

    // Divider
    let div_y = stats_y + 6.0;
    write!(
        s,
        r#"<line x1="{}" y1="{}" x2="{}" y2="{}" stroke="{CARD_BD}" stroke-width="1"/>"#,
        p(x + CARD_PAD),
        p(div_y),
        p(x + w - CARD_PAD),
        p(div_y)
    )
    .ok();

    // Icons
    let icon_y0 = div_y + ICON_GAP + ICON_SZ / 2.0;
    let icon_x0 = x + CARD_PAD + ICON_SZ / 2.0 + 6.0;
    let ir = ICON_SZ / 2.0 * 0.82; // icon radius slightly smaller than half-cell

    for (i, call) in card.calls.iter().enumerate() {
        let row = i / ICONS_ROW;
        let col = i % ICONS_ROW;
        let ix = icon_x0 + col as f32 * (ICON_SZ + ICON_GAP);
        let iy = icon_y0 + row as f32 * (ICON_SZ + ICON_GAP);

        // Icon cell background
        write!(
            s,
            r#"<rect x="{}" y="{}" width="{ICON_SZ}" height="{ICON_SZ}" rx="3"
                          fill="{}" opacity="0.12"/>
"#,
            p(ix - ICON_SZ / 2.0),
            p(iy - ICON_SZ / 2.0),
            call.cat.color()
        )
        .ok();

        // Icon shape
        s.push_str(&icon(call.cat, ix, iy, ir));

        // Count badge (if > 1)
        if call.count > 1 {
            write!(s, r##"<rect x="{}" y="{}" width="13" height="10" rx="3" fill="{}" opacity="0.9"/>
                         <text x="{}" y="{}" fill="#0a0a1a" font-size="8" font-weight="bold" text-anchor="middle">{}</text>"##,
                p(ix + ir - 2.0), p(iy - ir - 1.0), call.cat.color(),
                p(ix + ir + 4.5), p(iy - ir + 7.0), call.count
            ).ok();
        }
    }

    s
}

// ── Legend, header, sidebar ───────────────────────────────────────────────────

fn render_header(filename: &str, funcs: &[FuncCard]) -> String {
    let name = std::path::Path::new(filename)
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| filename.to_string());
    let n_fns = funcs.len();
    let count = |pred: fn(Cat) -> bool| -> usize {
        funcs
            .iter()
            .flat_map(|f| &f.calls)
            .filter(|c| pred(c.cat))
            .map(|c| c.count)
            .sum()
    };
    let n_vtex = count(is_vtex);
    let n_audio = count(is_audio);
    let n_crypto = count(is_crypto);
    let n_physics = count(is_physics);
    let n_ai = count(is_ai);
    let n_calls: usize = funcs.iter().map(|f| f.calls.len()).sum();

    // Domain stat chips — only show those actually present, so the header stays tight.
    let mut stats = vec![format!("{n_fns} fn"), format!("{n_calls} call types")];
    for (n, lbl) in [
        (n_vtex, "vtex"),
        (n_audio, "audio"),
        (n_crypto, "crypto"),
        (n_physics, "physics"),
        (n_ai, "ai"),
    ] {
        if n > 0 {
            stats.push(format!("{n} {lbl}"));
        }
    }

    format!(
        r##"<rect x="0" y="0" width="{SVG_W}" height="{HEADER_H}" fill="#080814"/>
           <line x1="0" y1="{HEADER_H}" x2="{SVG_W}" y2="{HEADER_H}" stroke="#1a1a3a" stroke-width="1"/>
           <text x="20" y="22" fill="{GOLD}" font-size="9" font-weight="bold" opacity="0.6" letter-spacing="2">LING VISUALIZER</text>
           <text x="20" y="56" fill="{TEXT}" font-size="26" font-weight="bold">{}</text>
           <text x="{}" y="56" fill="{TEXT_DIM}" font-size="12" text-anchor="end">{}</text>"##,
        xe(&name),
        SVG_W - 20.0,
        stats.join("  ·  ")
    )
}

fn render_legend(funcs: &[FuncCard]) -> String {
    // Collect present categories, then order them by coarse domain so related
    // icons cluster together; a faint tag introduces each domain group.
    let mut present: Vec<Cat> = Vec::new();
    let mut seen = HashSet::new();
    for f in funcs {
        for c in &f.calls {
            if seen.insert(c.cat) {
                present.push(c.cat);
            }
        }
    }
    present.sort_by(|a, b| a.domain().cmp(b.domain()).then(a.label().cmp(b.label())));

    let mut s = format!(
        r##"<rect x="0" y="{HEADER_H}" width="{SVG_W}" height="{LEGEND_H}" fill="#0a0a18"/>
           <line x1="0" y1="{}" x2="{SVG_W}" y2="{}" stroke="#171730" stroke-width="1"/>"##,
        HEADER_H + LEGEND_H,
        HEADER_H + LEGEND_H
    );

    let row_h = 21.0;
    let x_start = GRID_X + 8.0;
    let x_max = SVG_W - 24.0;
    let mut lx = x_start;
    let mut row = 0usize;
    let mut cur_domain = "";
    let row_y = |r: usize| HEADER_H + 17.0 + r as f32 * row_h;

    for cat in present {
        let c = cat.color();
        let lbl = cat.label();
        let dom = cat.domain();

        // Domain tag whenever the group changes (and a little extra lead space).
        if dom != cur_domain {
            let tag_w = dom.len() as f32 * 6.0 + 12.0;
            if lx > x_start && lx + tag_w > x_max {
                row += 1;
                lx = x_start;
            }
            if row > 1 {
                break;
            } // at most two rows in the band
            let ly = row_y(row);
            s.push_str(&format!(
                r##"<text x="{}" y="{}" fill="#3f3f66" font-size="8" font-weight="bold" letter-spacing="1">{}</text>"##,
                p(lx), p(ly + 3.0), dom.to_uppercase()
            ));
            lx += tag_w;
            cur_domain = dom;
        }

        let item_w = 20.0 + lbl.len() as f32 * 6.2 + 12.0;
        if lx + item_w > x_max {
            row += 1;
            lx = x_start;
        }
        if row > 1 {
            break;
        }
        let ly = row_y(row);
        let icon_svg = icon(cat, lx + 7.0, ly, 6.0);
        s.push_str(&format!(
            r#"<rect x="{}" y="{}" width="14" height="14" rx="3" fill="{c}" opacity="0.12"/>
               {}
               <text x="{}" y="{}" fill="{TEXT_DIM}" font-size="10">{lbl}</text>"#,
            p(lx),
            p(ly - 7.0),
            icon_svg,
            p(lx + 18.0),
            p(ly + 4.0)
        ));
        lx += item_w;
    }
    s
}

fn render_sidebar(globals: &[GlobalConst], total_h: f32) -> String {
    let h = total_h - CONTENT_Y;
    let mut s = format!(
        r##"<rect x="0" y="{CONTENT_Y}" width="{SIDEBAR_W}" height="{h}" fill="{SIDEBAR_BG}"/>
           <line x1="{SIDEBAR_W}" y1="{CONTENT_Y}" x2="{SIDEBAR_W}" y2="{total_h}" stroke="#1a1a3a" stroke-width="1"/>
           <text x="14" y="{}" fill="{TEXT_DIM}" font-size="9" font-weight="bold" letter-spacing="2">CONSTANTS</text>"##,
        CONTENT_Y + 20.0
    );

    let mut gy = CONTENT_Y + 38.0;
    for g in globals {
        // Small colored diamond
        write!(
            s,
            r#"<rect x="{}" y="{}" width="8" height="8" rx="1" fill="{}" opacity="0.7"
                    transform="rotate(45,{},{})"/>
               <text x="{}" y="{}" fill="{}" font-size="12" font-weight="bold">{}</text>
               <text x="{}" y="{}" fill="{TEXT_DIM}" font-size="12" text-anchor="end">{}</text>"#,
            p(14.0),
            p(gy - 7.0),
            Cat::Star.color(),
            p(18.0),
            p(gy - 3.0),
            p(28.0),
            p(gy),
            Cat::Grid.color(),
            xe(&g.name),
            p(SIDEBAR_W - 10.0),
            p(gy),
            xe(&g.value)
        )
        .ok();
        gy += 22.0;
    }
    s
}

// ── SVG defs (filters, patterns) ─────────────────────────────────────────────

const DEFS: &str = r##"<defs>
  <filter id="glow-gold" x="-30%" y="-30%" width="160%" height="160%">
    <feGaussianBlur in="SourceGraphic" stdDeviation="3" result="blur"/>
    <feMerge><feMergeNode in="blur"/><feMergeNode in="SourceGraphic"/></feMerge>
  </filter>
  <pattern id="grid-pat" x="0" y="0" width="40" height="40" patternUnits="userSpaceOnUse">
    <path d="M 40 0 L 0 0 0 40" fill="none" stroke="#ffffff" stroke-width="0.4"/>
  </pattern>
</defs>"##;

// ── Public entry point ────────────────────────────────────────────────────────

pub fn render(filename: &str, program: &Program) -> String {
    let doc = Document::build(filename, program);

    // Layout: pack cards into COLS columns using shortest-column strategy
    let heights: Vec<f32> = doc.funcs.iter().map(card_height).collect();
    let mut col_y = [CONTENT_Y; COLS];
    let mut positions: Vec<(f32, f32)> = Vec::with_capacity(doc.funcs.len());

    for &h in &heights {
        let (col, &cy) = col_y
            .iter()
            .enumerate()
            .min_by(|a, b| a.1.partial_cmp(b.1).unwrap())
            .unwrap();
        let cx = GRID_X + col as f32 * (CARD_W + CARD_GAP);
        positions.push((cx, cy));
        col_y[col] += h + CARD_GAP;
    }

    let total_h = col_y.iter().cloned().fold(0.0f32, f32::max) + 40.0;

    let mut svg = String::new();
    write!(
        svg,
        r#"<?xml version="1.0" encoding="UTF-8"?>
<svg xmlns="http://www.w3.org/2000/svg" width="{SVG_W}" height="{h}" viewBox="0 0 {SVG_W} {h}"
     style="font-family:'JetBrains Mono','Fira Code',monospace,sans-serif;background:{BG}">"#,
        h = total_h
    )
    .ok();

    svg.push_str(DEFS);

    // Background
    write!(svg, r#"<rect width="{SVG_W}" height="{total_h}" fill="{BG}"/>
                   <rect width="{SVG_W}" height="{total_h}" fill="url(#grid-pat)" opacity="0.05"/>"#,
        total_h = total_h).ok();

    svg.push_str(&render_header(&doc.filename, &doc.funcs));
    svg.push_str(&render_legend(&doc.funcs));
    svg.push_str(&render_sidebar(&doc.globals, total_h));

    for (card, &(cx, cy)) in doc.funcs.iter().zip(positions.iter()) {
        svg.push_str(&render_card(card, cx, cy));
    }

    svg.push_str("\n</svg>");
    svg
}
