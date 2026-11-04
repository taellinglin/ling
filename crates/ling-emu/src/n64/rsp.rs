//! RSP task identification — reads the OSTask block libultra places at
//! DMEM 0xFC0 when a task is kicked, for honest status reporting. The task
//! itself now EXECUTES on the LLE core in `rsp_core.rs`.

use super::mem::Bus;

pub struct TaskInfo {
    pub kind: u32, // 1 = graphics, 2 = audio, 4 = (de)compression …
    pub ucode_name: Option<String>,
}

/// Peek the task block (no side effects on SP state).
pub fn peek_task(bus: &Bus) -> TaskInfo {
    let be32 = |b: &[u8], i: usize| -> u32 {
        u32::from_be_bytes([b[i], b[i + 1], b[i + 2], b[i + 3]])
    };
    let kind = be32(&bus.dmem, 0xFC0);
    let ucode_data = be32(&bus.dmem, 0xFC0 + 0x10) & 0x00FF_FFFF;

    // Microcodes carry an ID string ("RSP Gfx ucode F3DEX…") near their data
    // segment in RDRAM — scan a window for it.
    let mut ucode_name = None;
    let start = ucode_data as usize;
    let end = (start + 0x1000).min(bus.rdram.len());
    if start < bus.rdram.len() {
        let window = &bus.rdram[start.saturating_sub(0x800)..end];
        if let Some(pos) = window.windows(4).position(|w| w == b"RSP ") {
            let tail = &window[pos..(pos + 64).min(window.len())];
            let text: String = tail
                .iter()
                .take_while(|&&c| (0x20..0x7F).contains(&c))
                .map(|&c| c as char)
                .collect();
            if text.len() > 8 {
                ucode_name = Some(text);
            }
        }
    }
    TaskInfo { kind, ucode_name }
}
