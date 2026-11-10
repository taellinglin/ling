//! Live counters the kernel keeps for the system monitor (`os_info("net_rx")`
//! and friends in lingapp.rs). Plain relaxed atomics bumped at the lowest
//! layer that sees the bytes: the NIC drivers' transmit/receive, the disk
//! drivers' sector read/write, and the scheduler's task switch.

use core::sync::atomic::{AtomicU64, Ordering::Relaxed};

static NET_RX: AtomicU64 = AtomicU64::new(0);
static NET_TX: AtomicU64 = AtomicU64::new(0);
static DISK_RD: AtomicU64 = AtomicU64::new(0);
static DISK_WR: AtomicU64 = AtomicU64::new(0);
static SWITCHES: AtomicU64 = AtomicU64::new(0);
static TOTAL_FRAMES: AtomicU64 = AtomicU64::new(0);

pub fn net_rx(bytes: usize) {
    NET_RX.fetch_add(bytes as u64, Relaxed);
}
pub fn net_tx(bytes: usize) {
    NET_TX.fetch_add(bytes as u64, Relaxed);
}
pub fn disk_rd(bytes: usize) {
    DISK_RD.fetch_add(bytes as u64, Relaxed);
}
pub fn disk_wr(bytes: usize) {
    DISK_WR.fetch_add(bytes as u64, Relaxed);
}
pub fn switched() {
    SWITCHES.fetch_add(1, Relaxed);
}

/// Boot-time memory map: frames handed to the allocator so far. Runs before
/// the MMU is on (all memory Device-typed, where an exclusive-store loop never
/// succeeds on real hardware), on the single boot core: a plain load + store,
/// not an atomic read-modify-write.
pub fn note_frames_added(n: u64) {
    TOTAL_FRAMES.store(TOTAL_FRAMES.load(Relaxed) + n, Relaxed);
}

pub fn get_net_rx() -> u64 {
    NET_RX.load(Relaxed)
}
pub fn get_net_tx() -> u64 {
    NET_TX.load(Relaxed)
}
pub fn get_disk_rd() -> u64 {
    DISK_RD.load(Relaxed)
}
pub fn get_disk_wr() -> u64 {
    DISK_WR.load(Relaxed)
}
pub fn get_switches() -> u64 {
    SWITCHES.load(Relaxed)
}
pub fn total_frames() -> u64 {
    TOTAL_FRAMES.load(Relaxed)
}
