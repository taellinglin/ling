//! OS audio mixer + pentatonic synthesizer -- the audio layer between the
//! AC'97 DMA ring (`ac97.rs`) and everything that makes sound.
//!
//! Model, per the design request: Windows-audio-shaped, deliberately NOT a
//! JACK-style routing graph. Every app (window kind / shell / player) owns
//! one implicit stream with its own volume; all active streams are
//! software-mixed into the one hardware ring ("shared mode"). One stream
//! may claim *exclusive mode* (ASIO-flavored takeover): while claimed,
//! only that stream is audible -- others keep rendering into silence
//! rather than blocking, so releasing exclusivity is glitchless.
//!
//! The synthesizer is real synthesis, not samples: pentatonic-tuned
//! (C-major pentatonic, two octaves) triangle/sine voices with attack/
//! exponential-decay envelopes, mixed at the AC'97's native 48 kHz. The
//! sine is Bhaskara's 7th-century approximation (no libm in no_std;
//! max error ~1.8% -- inaudible for chimes). "Sound themes" are named
//! per-event note sequences (boot/click/open/close/error/login), all in
//! the same pentatonic tuning so nothing can sound dissonant next to
//! anything else -- the property the pentatonic scale was chosen for.
//!
//! The tape player ("play" in lsh) is a stream like any other: a seeded
//! generative pentatonic track. Separately, a real *PCM sample* source
//! (`pcm_*`) plays actual WAV audio from lingfs -- now that multi-block
//! files exist, a real song fits -- resampling to 48 kHz and mixing it in
//! alongside the synth voices. That's what the desktop Media player uses.

use crate::drivers::ac97;

pub const SAMPLE_RATE: u32 = 48_000;

// -- Streams (one per app) --------------------------------------------------

pub const APP_SYSTEM: usize = 0; // jingles, UI feedback
pub const APP_SHELL: usize = 1; // lsh beeps
pub const APP_PLAYER: usize = 2; // the tape player
pub const APP_ABOUT: usize = 3;
pub const APP_SETTINGS: usize = 4;
pub const APP_FILES: usize = 5;
pub const NUM_STREAMS: usize = 6;

static STREAM_NAMES: [&str; NUM_STREAMS] =
    ["system", "shell", "player", "about", "settings", "files"];

static mut STREAM_VOL: [u32; NUM_STREAMS] = [80; NUM_STREAMS];
static mut MASTER_VOL: u32 = 80;
static mut EXCLUSIVE: i32 = -1;

pub fn stream_count() -> usize {
    NUM_STREAMS
}

pub fn stream_name(i: usize) -> &'static str {
    STREAM_NAMES.get(i).copied().unwrap_or("")
}

pub fn stream_volume(i: usize) -> u32 {
    unsafe { STREAM_VOL.get(i).copied().unwrap_or(0) }
}

pub fn set_stream_volume(i: usize, vol: u32) {
    unsafe {
        if i < NUM_STREAMS {
            STREAM_VOL[i] = vol.min(100);
        }
    }
}

pub fn master_volume() -> u32 {
    unsafe { MASTER_VOL }
}

pub fn set_master_volume(vol: u32) {
    unsafe { MASTER_VOL = vol.min(100) };
}

/// Claim exclusive mode for a stream (ASIO-style takeover): every other
/// stream renders but is muted at the mix stage. Returns false if a
/// different stream already holds it.
pub fn exclusive_claim(i: usize) -> bool {
    unsafe {
        if EXCLUSIVE >= 0 && EXCLUSIVE != i as i32 {
            return false;
        }
        EXCLUSIVE = i as i32;
        true
    }
}

pub fn exclusive_release(i: usize) {
    unsafe {
        if EXCLUSIVE == i as i32 {
            EXCLUSIVE = -1;
        }
    }
}

pub fn exclusive_holder() -> i32 {
    unsafe { EXCLUSIVE }
}

// -- Synth voices ------------------------------------------------------------

const MAX_VOICES: usize = 12;

#[derive(Clone, Copy)]
struct Voice {
    active: bool,
    stream: usize,
    phase: f64,      // 0..1
    phase_inc: f64,  // freq / SAMPLE_RATE
    level: f64,      // current envelope level 0..1
    attack: f64,     // per-sample level increment while attacking
    decay: f64,      // per-sample multiplicative decay after attack
    attacking: bool,
    sine: bool,      // false = triangle
    /// Samples until this voice starts (scheduler delay for jingle notes).
    delay: u32,
}

const SILENT_VOICE: Voice = Voice {
    active: false,
    stream: 0,
    phase: 0.0,
    phase_inc: 0.0,
    level: 0.0,
    attack: 0.0,
    decay: 0.0,
    attacking: false,
    sine: false,
    delay: 0,
};

static mut VOICES: [Voice; MAX_VOICES] = [SILENT_VOICE; MAX_VOICES];

/// Bhaskara I's sine approximation over 0..pi, mirrored for the full
/// cycle. Input phase 0..1, output -1..1.
fn fast_sin(phase: f64) -> f64 {
    let (p, sign) = if phase < 0.5 { (phase * 2.0, 1.0) } else { ((phase - 0.5) * 2.0, -1.0) };
    // x in 0..pi expressed via p in 0..1: sin = 16p(1-p) / (5 - 4p(1-p))
    let t = p * (1.0 - p);
    sign * (16.0 * t) / (5.0 - 4.0 * t)
}

fn triangle(phase: f64) -> f64 {
    if phase < 0.25 {
        phase * 4.0
    } else if phase < 0.75 {
        2.0 - phase * 4.0
    } else {
        phase * 4.0 - 4.0
    }
}

/// Start one voice. `freq_centihz` is Hz*100 (flat-u64 FFI has no floats),
/// `dur_ms` shapes the decay so the note audibly ends around then,
/// `delay_ms` schedules it into the future (jingle sequencing).
pub fn note(stream: usize, freq_centihz: u32, dur_ms: u32, delay_ms: u32, sine: bool) {
    let freq = freq_centihz as f64 / 100.0;
    if freq <= 0.0 {
        return;
    }
    unsafe {
        let voices = &mut *&raw mut VOICES;
        let Some(v) = voices.iter_mut().find(|v| !v.active) else { return };
        // Decay constant: level falls to ~1% over dur_ms.
        let dur_samples = (dur_ms.max(30) as f64 / 1000.0) * SAMPLE_RATE as f64;
        *v = Voice {
            active: true,
            stream: stream.min(NUM_STREAMS - 1),
            phase: 0.0,
            phase_inc: freq / SAMPLE_RATE as f64,
            level: 0.0,
            attack: 1.0 / (0.004 * SAMPLE_RATE as f64), // 4ms attack
            decay: libm_free_exp_decay(dur_samples),
            attacking: true,
            sine,
            delay: (delay_ms as u64 * SAMPLE_RATE as u64 / 1000) as u32,
        };
    }
}

/// decay-per-sample d with d^n = 0.01 for n = dur_samples, computed
/// without libm: d = exp(ln(0.01)/n), approximated via (1 + x/k)^k with
/// enough halvings -- cheap, once per note.
fn libm_free_exp_decay(dur_samples: f64) -> f64 {
    let x = -4.605_170_186 / dur_samples; // ln(0.01)
    // exp(x) ~ (1 + x/64)^64 for the small negative x this sees.
    let mut base = 1.0 + x / 64.0;
    for _ in 0..6 {
        base *= base;
    }
    base.clamp(0.0, 0.999_999)
}

// -- Pentatonic scale + sound themes ----------------------------------------

/// C-major pentatonic, C4..A5, in centihertz. Every jingle and the
/// generative tape track index into this -- one tuning everywhere.
pub const PENTATONIC_CENTIHZ: [u32; 10] = [
    26163, 29366, 32963, 39200, 44000, // C4 D4 E4 G4 A4
    52325, 58733, 65925, 78399, 88000, // C5 D5 E5 G5 A5
];

pub const EVENT_BOOT: usize = 0;
pub const EVENT_CLICK: usize = 1;
pub const EVENT_OPEN: usize = 2;
pub const EVENT_CLOSE: usize = 3;
pub const EVENT_ERROR: usize = 4;
pub const EVENT_LOGIN: usize = 5;

/// One jingle note: (pentatonic index, start delay ms, duration ms).
type JingleNote = (usize, u32, u32);

struct SoundTheme {
    name: &'static str,
    /// Indexed by EVENT_*; a slice of scheduled notes (empty = silent).
    events: [&'static [JingleNote]; 6],
}

static SOUND_THEMES: [SoundTheme; 3] = [
    SoundTheme {
        name: "Dawn Chimes",
        events: [
            // boot: rising C-E-G-A-C5 sparkle
            &[(0, 0, 350), (2, 120, 350), (3, 240, 350), (4, 360, 400), (5, 520, 700)],
            &[(7, 0, 60)],                            // click: short high tick
            &[(3, 0, 140), (5, 70, 220)],             // open: two rising
            &[(5, 0, 140), (3, 70, 220)],             // close: two falling
            &[(1, 0, 260), (1, 180, 380)],            // error: low double-knock
            &[(2, 0, 200), (4, 130, 200), (7, 260, 500)], // login: triad up
        ],
    },
    SoundTheme {
        name: "Jade Bells",
        events: [
            &[(4, 0, 500), (7, 200, 500), (9, 400, 800)],
            &[(9, 0, 50)],
            &[(5, 0, 180), (8, 90, 260)],
            &[(8, 0, 180), (5, 90, 260)],
            &[(0, 0, 300), (0, 220, 420)],
            &[(4, 0, 240), (7, 150, 240), (9, 300, 600)],
        ],
    },
    SoundTheme {
        name: "Silent",
        events: [&[], &[], &[], &[], &[], &[]],
    },
];

static mut SOUND_THEME: usize = 0;

pub fn sound_theme_count() -> usize {
    SOUND_THEMES.len()
}

pub fn sound_theme() -> usize {
    unsafe { SOUND_THEME }
}

pub fn set_sound_theme(i: usize) {
    unsafe { SOUND_THEME = i % SOUND_THEMES.len() };
}

pub fn sound_theme_name(i: usize) -> &'static str {
    SOUND_THEMES.get(i).map(|t| t.name).unwrap_or("")
}

/// Fire one UI sound event through the active sound theme (system stream,
/// shared mode -- a click never needs exclusivity).
pub fn jingle(event: usize) {
    let theme = &SOUND_THEMES[unsafe { SOUND_THEME } % SOUND_THEMES.len()];
    let Some(notes) = theme.events.get(event) else { return };
    for &(pent, delay, dur) in notes.iter() {
        note(APP_SYSTEM, PENTATONIC_CENTIHZ[pent % 10], dur, delay, event == EVENT_BOOT);
    }
}

// -- Tape player (generative pentatonic track) -------------------------------

pub const PLAYER_STOPPED: u32 = 0;
pub const PLAYER_PLAYING: u32 = 1;
pub const PLAYER_PAUSED: u32 = 2;

static mut PLAYER_STATE: u32 = PLAYER_STOPPED;
static mut PLAYER_POS_SAMPLES: u64 = 0;
static mut PLAYER_LEN_SAMPLES: u64 = 0;
static mut PLAYER_SEED: u64 = 0;
static mut PLAYER_NEXT_NOTE_AT: u64 = 0;
static mut PLAYER_RNG: u64 = 0;

/// Load a generative track: `seed` picks the melody, `len_ms` its length.
/// Real state for a real transport -- position advances only while
/// samples are actually rendered into the ring.
pub fn player_load(seed: u64, len_ms: u32) {
    unsafe {
        PLAYER_SEED = seed;
        PLAYER_RNG = seed | 1;
        PLAYER_POS_SAMPLES = 0;
        PLAYER_NEXT_NOTE_AT = 0;
        PLAYER_LEN_SAMPLES = len_ms as u64 * SAMPLE_RATE as u64 / 1000;
        PLAYER_STATE = PLAYER_STOPPED;
    }
}

pub fn player_play() {
    unsafe {
        if PLAYER_LEN_SAMPLES > 0 {
            PLAYER_STATE = PLAYER_PLAYING;
        }
    }
}

pub fn player_pause() {
    unsafe {
        if PLAYER_STATE == PLAYER_PLAYING {
            PLAYER_STATE = PLAYER_PAUSED;
        } else if PLAYER_STATE == PLAYER_PAUSED {
            PLAYER_STATE = PLAYER_PLAYING;
        }
    }
}

pub fn player_stop() {
    unsafe {
        PLAYER_STATE = PLAYER_STOPPED;
        PLAYER_POS_SAMPLES = 0;
        PLAYER_NEXT_NOTE_AT = 0;
        PLAYER_RNG = PLAYER_SEED | 1;
    }
}

pub fn player_state() -> u32 {
    unsafe { PLAYER_STATE }
}

pub fn player_pos_ms() -> u64 {
    unsafe { PLAYER_POS_SAMPLES * 1000 / SAMPLE_RATE as u64 }
}

pub fn player_len_ms() -> u64 {
    unsafe { PLAYER_LEN_SAMPLES * 1000 / SAMPLE_RATE as u64 }
}

static mut LINE_BUF: [u8; 64] = [0; 64];

/// The tape deck's one-line ASCII transport, built kernel-side (a
/// variable-length bar is a loop `.ling` can't write):
/// `[>] |######--------------| 00:12 / 01:00`. State glyph: `>` playing,
/// `=` paused, `.` stopped. Redraw with `\r` for the in-place effect.
pub fn player_line() -> &'static str {
    let pos = player_pos_ms();
    let len = player_len_ms().max(1);
    let filled = ((pos * 20) / len).min(20) as usize;
    let state = player_state();
    unsafe {
        let buf = &mut *&raw mut LINE_BUF;
        let mut n = 0;
        buf[n] = b'[';
        buf[n + 1] = match state {
            PLAYER_PLAYING => b'>',
            PLAYER_PAUSED => b'=',
            _ => b'.',
        };
        buf[n + 2] = b']';
        buf[n + 3] = b' ';
        buf[n + 4] = b'|';
        n += 5;
        for i in 0..20 {
            buf[n + i] = if i < filled { b'#' } else { b'-' };
        }
        n += 20;
        buf[n] = b'|';
        buf[n + 1] = b' ';
        n += 2;
        for (t, sep) in [(pos, true), (len, false)] {
            let total_s = t / 1000;
            let (m, s) = (total_s / 60, total_s % 60);
            buf[n] = b'0' + ((m / 10) % 10) as u8;
            buf[n + 1] = b'0' + (m % 10) as u8;
            buf[n + 2] = b':';
            buf[n + 3] = b'0' + (s / 10) as u8;
            buf[n + 4] = b'0' + (s % 10) as u8;
            n += 5;
            if sep {
                buf[n] = b' ';
                buf[n + 1] = b'/';
                buf[n + 2] = b' ';
                n += 3;
            }
        }
        core::str::from_utf8(&buf[..n]).unwrap_or("")
    }
}

fn player_advance(frames: usize) {
    unsafe {
        if PLAYER_STATE != PLAYER_PLAYING {
            return;
        }
        // Schedule melody notes as playback crosses eighth-note boundaries
        // (120 BPM -> 250ms per eighth). xorshift picks pentatonic steps;
        // same seed = same melody, honestly reproducible.
        let step = SAMPLE_RATE as u64 / 4;
        let end = PLAYER_POS_SAMPLES + frames as u64;
        while PLAYER_NEXT_NOTE_AT < end {
            PLAYER_RNG ^= PLAYER_RNG << 13;
            PLAYER_RNG ^= PLAYER_RNG >> 7;
            PLAYER_RNG ^= PLAYER_RNG << 17;
            let r = PLAYER_RNG;
            // Rest ~1/4 of the time; otherwise a note, longer on beats.
            if r % 4 != 0 {
                let idx = (r >> 8) as usize % 10;
                let on_beat = (PLAYER_NEXT_NOTE_AT / step) % 4 == 0;
                let dur = if on_beat { 420 } else { 200 };
                note(APP_PLAYER, PENTATONIC_CENTIHZ[idx], dur, 0, on_beat);
            }
            PLAYER_NEXT_NOTE_AT += step;
        }
        PLAYER_POS_SAMPLES = end;
        if PLAYER_POS_SAMPLES >= PLAYER_LEN_SAMPLES {
            player_stop();
        }
    }
}

// -- The mix ------------------------------------------------------------------

// -- PCM sample source (real WAV playback) ----------------------------------
// Plays raw PCM from a caller-owned buffer (media.rs's lingfs read buffer),
// resampled to 48 kHz and summed into the mix on the player stream. u8 and
// s16le, mono or stereo. Fractional resampling via a 16.16 cursor.
pub const PCM_STOPPED: u32 = 0;
pub const PCM_PLAYING: u32 = 1;
pub const PCM_PAUSED: u32 = 2;

static mut PCM_PTR: *const u8 = core::ptr::null();
static mut PCM_FRAMES: u64 = 0; // total source frames
static mut PCM_RATE: u32 = 22050;
static mut PCM_CH: u8 = 1;
static mut PCM_BITS: u8 = 8;
static mut PCM_POS: u64 = 0; // 16.16 fixed-point source frame index
static mut PCM_STEP: u64 = 0; // src frames per output frame, 16.16
static mut PCM_STATE: u32 = PCM_STOPPED;

/// Point the PCM player at a raw sample buffer. `frames` is the number of
/// whole sample frames (already accounting for channels). The buffer must
/// outlive playback (media.rs keeps it in a static). Does not start.
pub fn pcm_load(ptr: *const u8, frames: u64, rate: u32, channels: u8, bits: u8) {
    unsafe {
        PCM_PTR = ptr;
        PCM_FRAMES = frames;
        PCM_RATE = rate.max(1);
        PCM_CH = channels.max(1);
        PCM_BITS = if bits == 16 { 16 } else { 8 };
        PCM_POS = 0;
        PCM_STEP = ((rate as u64) << 16) / SAMPLE_RATE as u64;
        PCM_STATE = PCM_STOPPED;
    }
}

pub fn pcm_play() {
    unsafe {
        if !PCM_PTR.is_null() && PCM_FRAMES > 0 {
            PCM_STATE = PCM_PLAYING;
        }
    }
}
pub fn pcm_pause() {
    unsafe {
        PCM_STATE = match PCM_STATE {
            PCM_PLAYING => PCM_PAUSED,
            PCM_PAUSED => PCM_PLAYING,
            s => s,
        };
    }
}
pub fn pcm_stop() {
    unsafe {
        PCM_STATE = PCM_STOPPED;
        PCM_POS = 0;
    }
}
pub fn pcm_state() -> u32 {
    unsafe { PCM_STATE }
}
pub fn pcm_pos_ms() -> u64 {
    unsafe { (PCM_POS >> 16) * 1000 / PCM_RATE as u64 }
}
pub fn pcm_len_ms() -> u64 {
    unsafe { PCM_FRAMES * 1000 / PCM_RATE as u64 }
}

/// One source sample (mono-summed, -1.0..1.0) at the current cursor, or 0
/// if not playing / past the end. Advances the cursor by one output frame.
unsafe fn pcm_next() -> f64 {
    if PCM_STATE != PCM_PLAYING || PCM_PTR.is_null() {
        return 0.0;
    }
    let idx = PCM_POS >> 16;
    if idx >= PCM_FRAMES {
        PCM_STATE = PCM_STOPPED;
        PCM_POS = 0;
        return 0.0;
    }
    let ch = PCM_CH as u64;
    let bytes_per_sample = (PCM_BITS / 8) as u64;
    let frame_bytes = ch * bytes_per_sample;
    let base = (idx * frame_bytes) as usize;
    let mut sum = 0.0f64;
    for c in 0..ch as usize {
        let off = base + c * bytes_per_sample as usize;
        let v = if PCM_BITS == 16 {
            let lo = *PCM_PTR.add(off) as i16;
            let hi = *PCM_PTR.add(off + 1) as i16;
            let s = (lo | (hi << 8)) as i16;
            s as f64 / 32768.0
        } else {
            // u8 PCM is unsigned, centered at 128.
            (*PCM_PTR.add(off) as f64 - 128.0) / 128.0
        };
        sum += v;
    }
    PCM_POS += PCM_STEP;
    sum / ch as f64
}

/// Render `buf` (interleaved stereo i16) from all active voices, applying
/// per-stream volume, exclusive mode, and master volume.
fn render(buf: &mut [i16]) {
    let frames = buf.len() / 2;
    player_advance(frames);
    unsafe {
        let voices = &mut *&raw mut VOICES;
        let excl = EXCLUSIVE;
        for f in 0..frames {
            let mut acc = 0.0f64;
            for v in voices.iter_mut() {
                if !v.active {
                    continue;
                }
                if v.delay > 0 {
                    // Scheduled note not yet due -- burn its delay one
                    // sample at a time so jingle timing is sample-exact.
                    v.delay -= 1;
                    continue;
                }
                let s = if v.sine { fast_sin(v.phase) } else { triangle(v.phase) };
                v.phase += v.phase_inc;
                if v.phase >= 1.0 {
                    v.phase -= 1.0;
                }
                if v.attacking {
                    v.level += v.attack;
                    if v.level >= 1.0 {
                        v.level = 1.0;
                        v.attacking = false;
                    }
                } else {
                    v.level *= v.decay;
                    if v.level < 0.001 {
                        v.active = false;
                        continue;
                    }
                }
                let stream_gain = if excl >= 0 && excl as usize != v.stream {
                    0.0
                } else {
                    STREAM_VOL[v.stream] as f64 / 100.0
                };
                acc += s * v.level * stream_gain * 0.22; // headroom for ~4 voices
            }
            // Real PCM (WAV) on the player stream, honoring exclusive mode.
            let pcm = pcm_next();
            if pcm != 0.0 {
                let pgain = if excl >= 0 && excl as usize != APP_PLAYER {
                    0.0
                } else {
                    STREAM_VOL[APP_PLAYER] as f64 / 100.0
                };
                acc += pcm * pgain * 0.9;
            }
            let master = MASTER_VOL as f64 / 100.0;
            let sample = (acc * master * 32767.0).clamp(-32767.0, 32767.0) as i16;
            buf[f * 2] = sample;
            buf[f * 2 + 1] = sample;
        }
    }
}

/// Initialize the hardware; true if an AC'97 device exists.
pub fn init() -> bool {
    ac97::init()
}

/// Keep the DMA ring fed -- call once per WM frame / shell-loop iteration.
/// Cheap no-op when the ring is already full or no device exists.
pub fn pump() {
    if !ac97::present() {
        return;
    }
    ac97::pump(&mut render);
}
