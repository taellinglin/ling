//! Dandelion++ router state (DESIGN §8.4). Port of ling-net
//! `lwp/dandelion.rs` (HashMap → BTreeMap, Instant → `sys::now_ms`,
//! Arc → Rc). Pure bookkeeping; the network layer does the actual sending.
//!
//! - Per 10-minute epoch we pick up to two stem peers among outbound peers.
//!   Each source (inbound edge, or "local") is mapped to one of them for the
//!   whole epoch.
//! - Locally created txs are always stemmed; relayed txs are stemmed with
//!   probability `stem_probability` (0.9), otherwise fluffed.
//! - Every stemmed tx gets a random embargo timer; if the node hasn't called
//!   `mark_fluffed` by then, we fluff it ourselves.

use alloc::collections::{BTreeMap, VecDeque};
use alloc::rc::Rc;
use alloc::vec::Vec;

use crate::lwp::sys;
use crate::lwp::PeerId;

const MAX_SEEN: usize = 100_000;
const MAX_EMBARGO: usize = 10_000;

pub(crate) enum Decision {
    Stem(PeerId),
    Fluff,
    Drop,
}

pub(crate) struct Embargoed {
    /// `sys::now_ms()` deadline.
    pub deadline: u64,
    pub payload: Rc<Vec<u8>>,
    pub kind: u8,
}

pub(crate) struct Dandelion {
    epoch_start: Option<u64>,
    stems: Vec<PeerId>,
    routes: BTreeMap<Option<PeerId>, PeerId>,
    seen: BTreeMap<[u8; 32], ()>,
    seen_order: VecDeque<[u8; 32]>,
    pub embargo: BTreeMap<[u8; 32], Embargoed>,
}

impl Dandelion {
    pub fn new() -> Self {
        Dandelion {
            epoch_start: None,
            stems: Vec::new(),
            routes: BTreeMap::new(),
            seen: BTreeMap::new(),
            seen_order: VecDeque::new(),
            embargo: BTreeMap::new(),
        }
    }

    fn see(&mut self, id: [u8; 32]) -> bool {
        if self.seen.contains_key(&id) {
            return false;
        }
        if self.seen_order.len() >= MAX_SEEN {
            if let Some(old) = self.seen_order.pop_front() {
                self.seen.remove(&old);
            }
        }
        self.seen.insert(id, ());
        self.seen_order.push_back(id);
        true
    }

    /// Refresh the epoch if it expired or a stem peer went away.
    fn refresh(&mut self, outbound: &[PeerId], epoch_ms: u64) {
        let now = sys::now_ms();
        let expired = self.epoch_start.map(|t| now.saturating_sub(t) >= epoch_ms).unwrap_or(true);
        let lost = self.stems.iter().any(|s| !outbound.contains(s));
        if expired || lost || (self.stems.len() < 2 && outbound.len() > self.stems.len()) {
            let mut pool = outbound.to_vec();
            self.stems.clear();
            while self.stems.len() < 2 && !pool.is_empty() {
                let i = sys::gen_index(pool.len());
                self.stems.push(pool.swap_remove(i));
            }
            self.routes.clear();
            self.epoch_start = Some(now);
        }
    }

    pub fn decide(
        &mut self,
        tx_id: [u8; 32],
        from: Option<PeerId>,
        outbound: &[PeerId],
        epoch_ms: u64,
        stem_probability: f64,
    ) -> Decision {
        if !self.see(tx_id) {
            return Decision::Drop;
        }
        let stem = from.is_none() || sys::gen_bool(stem_probability.clamp(0.0, 1.0));
        if !stem {
            return Decision::Fluff;
        }
        self.refresh(outbound, epoch_ms);
        if let Some(&p) = self.routes.get(&from) {
            if Some(p) != from {
                return Decision::Stem(p);
            }
        }
        let choices: Vec<PeerId> = self.stems.iter().copied().filter(|&s| Some(s) != from).collect();
        if choices.is_empty() {
            return Decision::Fluff;
        }
        let p = choices[sys::gen_index(choices.len())];
        self.routes.insert(from, p);
        Decision::Stem(p)
    }

    pub fn start_embargo(&mut self, tx_id: [u8; 32], e: Embargoed) -> bool {
        if self.embargo.len() >= MAX_EMBARGO {
            return false;
        }
        self.embargo.insert(tx_id, e);
        true
    }

    pub fn mark_fluffed(&mut self, tx_id: [u8; 32]) {
        self.see(tx_id);
        self.embargo.remove(&tx_id);
    }

    pub fn expired(&mut self, now: u64) -> Vec<Embargoed> {
        let ids: Vec<[u8; 32]> = self.embargo.iter().filter(|(_, e)| e.deadline <= now).map(|(k, _)| *k).collect();
        ids.iter().filter_map(|k| self.embargo.remove(k)).collect()
    }
}
