//! ling-emu — native console-emulation host for Ling's `emu_*` builtins.
//!
//! Kept OUT of the default ling-lang build: the root crate pulls this in only
//! under the non-default `emu` cargo feature, so `cargo install ling-lang`
//! ships no emulation code.
//!
//! Cores:
//! * **SNES** — 65C816, PPU (modes 0-4 + 7, sprites, windows, colour math),
//!   DMA/HDMA, SPC700 running the real boot ROM, cycle-accurate S-DSP.
//! * **N64** — MIPS R4300i, HLE boot, RSP LLE (scalar + vector), VI, PI/SI,
//!   EEPROM, CIC-6105 challenge. RDP rasterizer in progress.
//! * GameCube is declared in [`System`] but unimplemented.

pub mod audio;
pub mod n64;
pub mod snes;

use std::sync::Mutex;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum System {
    N64,
    Snes,
    GameCube,
}

/// What a frontend needs from any core.
pub trait Core: Send {
    fn system(&self) -> System;
    /// Run one video frame.
    fn run_frame(&mut self);
    /// Current video output as packed 0x00RRGGBB rows.
    fn framebuffer(&self) -> (&[u32], usize, usize);
    /// Digital button state. Button indices are system-specific.
    fn set_button(&mut self, pad: usize, button: usize, down: bool);
    /// Analog stick, -1..1 (no-op for systems without one).
    fn set_stick(&mut self, _pad: usize, _x: f32, _y: f32) {}
    /// Honest machine status for the UI: "ok", or what's missing.
    fn status(&self) -> String;
    /// Battery/EEPROM data to persist, if it changed since last call.
    fn take_save(&mut self) -> Option<Vec<u8>> {
        None
    }
    /// Restore previously persisted battery data.
    fn load_save(&mut self, _data: &[u8]) {}
}

struct Slot {
    core: Box<dyn Core>,
    save_path: Option<String>,
    frames: u64,
    /// (window start, frames at window start, last measured emulated fps)
    rate: (std::time::Instant, u64, f64),
}

static CORE: Mutex<Option<Slot>> = Mutex::new(None);

pub fn with_core<R>(f: impl FnOnce(&mut dyn Core) -> R) -> Option<R> {
    CORE.lock().ok()?.as_mut().map(|s| f(s.core.as_mut()))
}

/// One-line status for the UI: emulated speed, audio health, core status.
pub fn status_line() -> String {
    let Ok(g) = CORE.lock() else { return String::new() };
    let Some(slot) = g.as_ref() else { return String::new() };
    let audio = if audio::is_live() {
        format!(
            "audio ok · {} underruns",
            audio::UNDERRUNS.load(std::sync::atomic::Ordering::Relaxed)
        )
    } else {
        "audio idle".to_string()
    };
    format!("{:.1} emu fps · {audio} · {}", slot.rate.2, slot.core.status())
}

pub fn is_loaded() -> bool {
    CORE.lock().map(|g| g.is_some()).unwrap_or(false)
}

fn flush(slot: &mut Slot) {
    if let (Some(path), Some(data)) = (slot.save_path.clone(), slot.core.take_save()) {
        let _ = std::fs::write(path, data);
    }
}

/// Write any dirty battery data now (suspend, exit, power-off).
pub fn flush_save() {
    if let Ok(mut g) = CORE.lock() {
        if let Some(slot) = g.as_mut() {
            flush(slot);
        }
    }
}

pub fn unload() {
    if let Ok(mut g) = CORE.lock() {
        if let Some(slot) = g.as_mut() {
            flush(slot);
        }
        *g = None;
    }
    audio::clear();
}

/// One emulated frame plus save maintenance (flushes dirty battery data
/// every ~5 seconds so a crash never loses a whole session).
pub fn frame() {
    if let Ok(mut g) = CORE.lock() {
        if let Some(slot) = g.as_mut() {
            // one frame normally; 0 or 2 when the audio queue drifts (see
            // audio::frames_wanted). The very first frame always runs.
            let n = if slot.frames == 0 { 1 } else { audio::frames_wanted() };
            for _ in 0..n {
                slot.core.run_frame();
                slot.frames += 1;
                if slot.frames % 300 == 0 {
                    flush(slot);
                }
            }
            let dt = slot.rate.0.elapsed().as_secs_f64();
            if dt >= 1.0 {
                slot.rate.2 = (slot.frames - slot.rate.1) as f64 / dt;
                slot.rate = (std::time::Instant::now(), slot.frames, slot.rate.2);
            }
        }
    }
}

/// Detect the system from the file bytes and boot the right core.
pub fn load(bytes: &[u8]) -> Result<System, String> {
    load_with_save(bytes, None)
}

pub fn load_file(path: &str) -> Result<System, String> {
    let bytes = std::fs::read(path).map_err(|e| format!("{path}: {e}"))?;
    load_with_save(&bytes, Some(format!("{path}.sav")))
}

fn load_with_save(bytes: &[u8], save_path: Option<String>) -> Result<System, String> {
    unload();
    // the status line's underrun count describes this game session only
    audio::UNDERRUNS.store(0, std::sync::atomic::Ordering::Relaxed);
    let (core, sys): (Box<dyn Core>, System) = if bytes.len() >= 4
        && [
            [0x80, 0x37, 0x12, 0x40],
            [0x37, 0x80, 0x40, 0x12],
            [0x40, 0x12, 0x37, 0x80],
        ]
        .contains(&[bytes[0], bytes[1], bytes[2], bytes[3]])
    {
        (Box::new(n64::N64::boot(bytes)?), System::N64)
    } else if snes::looks_like_snes(bytes) {
        (Box::new(snes::Snes::boot(bytes)?), System::Snes)
    } else {
        return Err(
            "unrecognized ROM (N64 .z64/.v64/.n64 and SNES .sfc/.smc supported; GameCube not yet implemented)"
                .into(),
        );
    };
    let mut slot = Slot { core, save_path, frames: 0, rate: (std::time::Instant::now(), 0, 0.0) };
    if let Some(p) = &slot.save_path {
        if let Ok(data) = std::fs::read(p) {
            slot.core.load_save(&data);
        }
    }
    *CORE.lock().unwrap() = Some(slot);
    Ok(sys)
}
