//! In-session shell command history — a fixed-size ring buffer, recalled
//! with the Up/Down arrow keys in `ling_kernel_read_line` (see
//! `keyboard.rs`'s `UP_ARROW`/`DOWN_ARROW`). Deliberately not persisted to
//! lingfs across boots: no multi-user audit need yet to justify writing
//! every line to disk (and reloading it) — see the "shell history" note in
//! project memory/roadmap discussion. `ling_kernel_read_line_masked`
//! (password entry) never pushes into this.
pub const HISTORY_SIZE: usize = 16;
const MAX_LINE: usize = 256;

static mut ENTRIES: [[u8; MAX_LINE]; HISTORY_SIZE] = [[0u8; MAX_LINE]; HISTORY_SIZE];
static mut LENGTHS: [usize; HISTORY_SIZE] = [0usize; HISTORY_SIZE];
static mut COUNT: usize = 0;
static mut HEAD: usize = 0; // next slot to write

pub fn push(line: &[u8]) {
    if line.is_empty() {
        return;
    }
    unsafe {
        let slot = HEAD;
        let n = line.len().min(MAX_LINE);
        ENTRIES[slot][..n].copy_from_slice(&line[..n]);
        LENGTHS[slot] = n;
        HEAD = (HEAD + 1) % HISTORY_SIZE;
        if COUNT < HISTORY_SIZE {
            COUNT += 1;
        }
    }
}

/// `back == 1` is the most recently entered line, `2` the one before that,
/// etc. `None` past however many entries actually exist.
pub fn get(back: usize) -> Option<&'static [u8]> {
    unsafe {
        if back == 0 || back > COUNT {
            return None;
        }
        let slot = (HEAD + HISTORY_SIZE - back) % HISTORY_SIZE;
        let entries: &[[u8; MAX_LINE]; HISTORY_SIZE] = &*&raw const ENTRIES;
        Some(&entries[slot][..LENGTHS[slot]])
    }
}
