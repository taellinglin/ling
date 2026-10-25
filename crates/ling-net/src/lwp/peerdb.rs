//! Address book, bans, anchors, and dial backoff, persisted as simple
//! line-based text files in `data_dir`:
//!
//! - `peers.dat`   : `addr last_success_unix failures` per line (gossip-learned)
//! - `bans.dat`    : `key until_unix` (key = `ip:<ip>`, `id:<hex>`, `addr:<addr>`)
//! - `anchors.dat` : one `addr` per line (≤ 2), tried first after restart
//!
//! Manual peers and seeds come from the config on each start and are never
//! stored here, so they are never gossiped. Files are read with a size cap
//! and malformed lines are skipped.

use std::collections::HashMap;
use std::fs;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use rand::seq::SliceRandom;

use crate::lwp::addr::{ip_is_routable, Host, NetAddr};

pub const MAX_ENTRIES: usize = 4096;
/// Cap per network group so one /16 cannot flood the address book.
pub const MAX_PER_GROUP: usize = 32;
const MAX_FILE: u64 = 4 << 20;
pub const MAX_ANCHORS: usize = 2;

pub(crate) fn unix_now() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0)
}

#[derive(Clone, Debug)]
pub(crate) struct Entry {
    pub addr: NetAddr,
    pub last_success: u64,
    pub failures: u32,
}

pub(crate) struct PeerDb {
    dir: PathBuf,
    pub entries: HashMap<String, Entry>,
    bans: HashMap<String, u64>,
    pub anchors: Vec<String>,
    backoff: HashMap<String, (Instant, u32)>,
}

fn read_capped(p: &Path) -> String {
    let mut b = Vec::new();
    if let Ok(f) = fs::File::open(p) {
        let _ = f.take(MAX_FILE).read_to_end(&mut b);
    }
    String::from_utf8_lossy(&b).into_owned()
}

fn write_atomic(p: &Path, data: &str) {
    let tmp = p.with_extension("tmp");
    if fs::write(&tmp, data).is_ok() {
        let _ = fs::rename(&tmp, p);
    }
}

impl PeerDb {
    pub fn load(dir: &Path) -> PeerDb {
        let _ = fs::create_dir_all(dir);
        let mut db = PeerDb {
            dir: dir.to_path_buf(),
            entries: HashMap::new(),
            bans: HashMap::new(),
            anchors: Vec::new(),
            backoff: HashMap::new(),
        };
        for line in read_capped(&dir.join("peers.dat")).lines() {
            let mut it = line.split_whitespace();
            let (Some(a), Some(ls), Some(f)) = (it.next(), it.next(), it.next()) else { continue };
            let (Some(addr), Ok(ls), Ok(f)) = (NetAddr::parse(a), ls.parse(), f.parse()) else { continue };
            if db.entries.len() < MAX_ENTRIES {
                db.entries.insert(addr.to_string(), Entry { addr, last_success: ls, failures: f });
            }
        }
        let now = unix_now();
        for line in read_capped(&dir.join("bans.dat")).lines() {
            let mut it = line.split_whitespace();
            let (Some(k), Some(u)) = (it.next(), it.next()) else { continue };
            if let Ok(u) = u.parse::<u64>() {
                if u > now && k.len() <= 300 && db.bans.len() < 100_000 {
                    db.bans.insert(k.to_string(), u);
                }
            }
        }
        for line in read_capped(&dir.join("anchors.dat")).lines().take(MAX_ANCHORS) {
            if let Some(a) = NetAddr::parse(line.trim()) {
                db.anchors.push(a.to_string());
            }
        }
        db
    }

    pub fn save(&self) {
        let mut s = String::from("");
        for (k, e) in &self.entries {
            s.push_str(&format!("{k} {} {}\n", e.last_success, e.failures));
        }
        write_atomic(&self.dir.join("peers.dat"), &s);
        let now = unix_now();
        let mut b = String::new();
        for (k, u) in &self.bans {
            if *u > now {
                b.push_str(&format!("{k} {u}\n"));
            }
        }
        write_atomic(&self.dir.join("bans.dat"), &b);
        let mut a = String::new();
        for x in self.anchors.iter().take(MAX_ANCHORS) {
            a.push_str(x);
            a.push('\n');
        }
        write_atomic(&self.dir.join("anchors.dat"), &a);
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
            match victims.choose(&mut rand::thread_rng()) {
                Some(v) => {
                    self.entries.remove(v);
                }
                None => return false,
            }
        }
        self.entries.insert(key, Entry { addr, last_success: 0, failures: 0 });
        true
    }

    pub fn can_try(&self, addr: &str, now: Instant) -> bool {
        self.backoff.get(addr).map(|(t, _)| now >= *t).unwrap_or(true)
    }

    pub fn on_success(&mut self, addr: &str) {
        self.backoff.remove(addr);
        if let Some(e) = self.entries.get_mut(addr) {
            e.last_success = unix_now();
            e.failures = 0;
        }
    }

    pub fn on_failure(&mut self, addr: &str, base: Duration, max: Duration) {
        let n = self.backoff.get(addr).map(|b| b.1).unwrap_or(0).saturating_add(1);
        let d = base.saturating_mul(1u32 << n.min(16).saturating_sub(1)).min(max);
        self.backoff.insert(addr.to_string(), (Instant::now() + d, n));
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

    pub fn ban(&mut self, key: String, d: Duration) {
        if self.bans.len() >= 100_000 {
            let now = unix_now();
            self.bans.retain(|_, u| *u > now);
            if self.bans.len() >= 100_000 {
                return;
            }
        }
        self.bans.insert(key, unix_now() + d.as_secs().max(1));
    }

    /// Up to `n` random addresses suitable for gossip: publicly routable IPs
    /// only (never onion, names, manual peers or seeds, which are not stored
    /// here at all) unless `allow_local` (tests).
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
        v.shuffle(&mut rand::thread_rng());
        v.truncate(n);
        v
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn persist_and_caps() {
        let dir = std::env::temp_dir().join(format!("lwnet-db-{}", rand::random::<u64>()));
        let mut db = PeerDb::load(&dir);
        for i in 0..100u32 {
            db.add(NetAddr::parse(&format!("8.8.{}.{}:1", i / 256, i % 256)).unwrap());
        }
        assert_eq!(db.entries.len(), MAX_PER_GROUP);
        db.add(NetAddr::parse("9.9.9.9:5").unwrap());
        db.ban("ip:1.2.3.4".into(), Duration::from_secs(100));
        db.anchors = vec!["9.9.9.9:5".into()];
        db.save();
        let db2 = PeerDb::load(&dir);
        assert_eq!(db2.entries.len(), MAX_PER_GROUP + 1);
        assert!(db2.is_banned("ip:1.2.3.4"));
        assert_eq!(db2.anchors, vec!["9.9.9.9:5".to_string()]);
        assert!(db2.gossip_sample(10, false).len() == 10);
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn garbage_files_are_ignored() {
        let dir = std::env::temp_dir().join(format!("lwnet-db-{}", rand::random::<u64>()));
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join("peers.dat"), b"\xff\xfe garbage\nnot an addr 1 2\n1.2.3.4:5 x y\n").unwrap();
        fs::write(dir.join("bans.dat"), b"k\n").unwrap();
        let db = PeerDb::load(&dir);
        assert!(db.entries.is_empty());
        let _ = fs::remove_dir_all(dir);
    }
}
