//! Address book, bans, anchors, and dial backoff. Port of ling-net
//! `lwp/peerdb.rs`; the same line-based files, stored through the kernel vfs
//! (`sys::read_file` / `sys::write_file`) in the network's data dir:
//!
//! - `peers.dat`   : `addr last_success_unix failures` per line (gossip-learned)
//! - `bans.dat`    : `key until_unix` (key = `ip:<ip>`, `id:<hex>`, `addr:<addr>`)
//! - `anchors.dat` : one `addr` per line (≤ 2), tried first after restart
//!
//! Without writable storage the book simply lives in memory.

use alloc::collections::BTreeMap;
use alloc::format;
use alloc::string::{String, ToString};
use alloc::vec::Vec;

use crate::lwp::addr::{ip_is_routable, Host, NetAddr};
use crate::lwp::sys;

pub const MAX_ENTRIES: usize = 4096;
/// Cap per network group so one /16 cannot flood the address book.
pub const MAX_PER_GROUP: usize = 32;
const MAX_FILE: usize = 4 << 20;
pub const MAX_ANCHORS: usize = 2;

pub(crate) fn unix_now() -> u64 {
    sys::unix_now()
}

#[derive(Clone, Debug)]
pub(crate) struct Entry {
    pub addr: NetAddr,
    pub last_success: u64,
    pub failures: u32,
}

pub(crate) struct PeerDb {
    dir: String,
    pub entries: BTreeMap<String, Entry>,
    bans: BTreeMap<String, u64>,
    pub anchors: Vec<String>,
    /// addr → (retry-not-before ms, consecutive failures)
    backoff: BTreeMap<String, (u64, u32)>,
}

fn read_capped(dir: &str, name: &str) -> String {
    let mut b = sys::read_file(dir, name).unwrap_or_default();
    b.truncate(MAX_FILE);
    String::from_utf8_lossy(&b).into_owned()
}

impl PeerDb {
    pub fn load(dir: &str) -> PeerDb {
        let mut db = PeerDb {
            dir: dir.to_string(),
            entries: BTreeMap::new(),
            bans: BTreeMap::new(),
            anchors: Vec::new(),
            backoff: BTreeMap::new(),
        };
        for line in read_capped(dir, "peers.dat").lines() {
            let mut it = line.split_whitespace();
            let (Some(a), Some(ls), Some(f)) = (it.next(), it.next(), it.next()) else { continue };
            let (Some(addr), Ok(ls), Ok(f)) = (NetAddr::parse(a), ls.parse(), f.parse()) else { continue };
            if db.entries.len() < MAX_ENTRIES {
                db.entries.insert(addr.to_string(), Entry { addr, last_success: ls, failures: f });
            }
        }
        let now = unix_now();
        for line in read_capped(dir, "bans.dat").lines() {
            let mut it = line.split_whitespace();
            let (Some(k), Some(u)) = (it.next(), it.next()) else { continue };
            if let Ok(u) = u.parse::<u64>() {
                if u > now && k.len() <= 300 && db.bans.len() < 100_000 {
                    db.bans.insert(k.to_string(), u);
                }
            }
        }
        for line in read_capped(dir, "anchors.dat").lines().take(MAX_ANCHORS) {
            if let Some(a) = NetAddr::parse(line.trim()) {
                db.anchors.push(a.to_string());
            }
        }
        db
    }

    pub fn save(&self) {
        let mut s = String::new();
        for (k, e) in &self.entries {
            s.push_str(&format!("{k} {} {}\n", e.last_success, e.failures));
        }
        sys::write_file(&self.dir, "peers.dat", s.as_bytes());
        let now = unix_now();
        let mut b = String::new();
        for (k, u) in &self.bans {
            if *u > now {
                b.push_str(&format!("{k} {u}\n"));
            }
        }
        sys::write_file(&self.dir, "bans.dat", b.as_bytes());
        let mut a = String::new();
        for x in self.anchors.iter().take(MAX_ANCHORS) {
            a.push_str(x);
            a.push('\n');
        }
        sys::write_file(&self.dir, "anchors.dat", a.as_bytes());
    }

    /// Insert a gossip-learned address, respecting total and per-group caps.
    /// When full, evicts a random never-successful entry.
    pub fn add(&mut self, addr: NetAddr) -> bool {
        let key = addr.to_string();
        if self.entries.contains_key(&key) {
            return false;
        }
        let g = addr.group();
        if self.entries.values().filter(|e| e.addr.group() == g).count() >= MAX_PER_GROUP {
            return false;
        }
        if self.entries.len() >= MAX_ENTRIES {
            let victims: Vec<String> =
                self.entries.iter().filter(|(_, e)| e.last_success == 0).map(|(k, _)| k.clone()).collect();
            if victims.is_empty() {
                return false;
            }
            let v = &victims[sys::gen_index(victims.len())];
            self.entries.remove(v);
        }
        self.entries.insert(key, Entry { addr, last_success: 0, failures: 0 });
        true
    }

    pub fn can_try(&self, addr: &str, now_ms: u64) -> bool {
        self.backoff.get(addr).map(|(t, _)| now_ms >= *t).unwrap_or(true)
    }

    pub fn on_success(&mut self, addr: &str) {
        self.backoff.remove(addr);
        if let Some(e) = self.entries.get_mut(addr) {
            e.last_success = unix_now();
            e.failures = 0;
        }
    }

    pub fn on_failure(&mut self, addr: &str, base_ms: u64, max_ms: u64) {
        let n = self.backoff.get(addr).map(|b| b.1).unwrap_or(0).saturating_add(1);
        let d = base_ms.saturating_mul(1u64 << n.min(16).saturating_sub(1)).min(max_ms);
        self.backoff.insert(addr.to_string(), (sys::now_ms() + d, n));
        if let Some(e) = self.entries.get_mut(addr) {
            e.failures = e.failures.saturating_add(1);
            if e.failures >= 10 && e.last_success == 0 {
                self.entries.remove(addr);
            }
        }
    }

    pub fn is_banned(&self, key: &str) -> bool {
        self.bans.get(key).map(|u| *u > unix_now()).unwrap_or(false)
    }

    pub fn ban(&mut self, key: String, secs: u64) {
        if self.bans.len() >= 100_000 {
            let now = unix_now();
            self.bans.retain(|_, u| *u > now);
            if self.bans.len() >= 100_000 {
                return;
            }
        }
        self.bans.insert(key, unix_now() + secs.max(1));
    }

    /// Up to `n` random addresses suitable for gossip: publicly routable IPs
    /// only unless `allow_local` (tests).
    pub fn gossip_sample(&self, n: usize, allow_local: bool) -> Vec<String> {
        let mut v: Vec<String> = self
            .entries
            .values()
            .filter(|e| match e.addr.host {
                Host::Ip(ip) => allow_local || ip_is_routable(ip),
                _ => false,
            })
            .map(|e| e.addr.to_string())
            .collect();
        sys::shuffle(&mut v);
        v.truncate(n);
        v
    }
}
