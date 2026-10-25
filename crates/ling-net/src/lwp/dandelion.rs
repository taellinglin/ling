//! Dandelion++ router state (DESIGN §8.4). Pure bookkeeping; the network
//! layer does the actual sending.
//!
//! - Per 10-minute epoch we pick up to two stem peers among outbound peers.
//!   Each source (inbound edge, or "local") is mapped to one of them for the
//!   whole epoch, so repeated txs from one source take the same path
//!   (resists intersection attacks).
//! - Locally created txs are always stemmed (Dandelion++ §4); relayed txs are
//!   stemmed with probability `stem_probability` (0.9), otherwise fluffed.
//! - Every stemmed tx gets a random embargo timer; if the node hasn't called
//!   `mark_fluffed` by then, we fluff it ourselves.

use std::collections::{HashMap, VecDeque};
use std::time::{Duration, Instant};

use rand::Rng;

use crate::lwp::PeerId;

const MAX_SEEN: usize = 100_000;
const MAX_EMBARGO: usize = 10_000;

pub(crate) enum Decision {
    Stem(PeerId),
    Fluff,
    Drop,
}

pub(crate) struct Embargoed {
    pub deadline: Instant,
    pub payload: std::sync::Arc<Vec<u8>>,
    pub kind: u8,
}

pub(crate) struct Dandelion {
    epoch_start: Option<Instant>,
    stems: Vec<PeerId>,
    routes: HashMap<Option<PeerId>, PeerId>,
    seen: HashMap<[u8; 32], ()>,
    seen_order: VecDeque<[u8; 32]>,
    pub embargo: HashMap<[u8; 32], Embargoed>,
}

impl Dandelion {
    pub fn new() -> Self {
        Dandelion {
            epoch_start: None,
            stems: Vec::new(),
            routes: HashMap::new(),
            seen: HashMap::new(),
            seen_order: VecDeque::new(),
            embargo: HashMap::new(),
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
    fn refresh(&mut self, outbound: &[PeerId], epoch: Duration) {
        let expired = self.epoch_start.map(|t| t.elapsed() >= epoch).unwrap_or(true);
        let lost = self.stems.iter().any(|s| !outbound.contains(s));
        if expired || lost || (self.stems.len() < 2 && outbound.len() > self.stems.len()) {
            let mut pool = outbound.to_vec();
            let mut rng = rand::thread_rng();
            self.stems.clear();
            while self.stems.len() < 2 && !pool.is_empty() {
                let i = rng.gen_range(0..pool.len());
                self.stems.push(pool.swap_remove(i));
            }
            self.routes.clear();
            self.epoch_start = Some(Instant::now());
        }
    }

    pub fn decide(
        &mut self,
        tx_id: [u8; 32],
        from: Option<PeerId>,
        outbound: &[PeerId],
        epoch: Duration,
        stem_probability: f64,
    ) -> Decision {
        if !self.see(tx_id) {
            return Decision::Drop;
        }
        let stem = from.is_none() || rand::thread_rng().gen_bool(stem_probability.clamp(0.0, 1.0));
        if !stem {
            return Decision::Fluff;
        }
        self.refresh(outbound, epoch);
        if let Some(&p) = self.routes.get(&from) {
            if Some(p) != from {
                return Decision::Stem(p);
            }
        }
        let choices: Vec<PeerId> = self.stems.iter().copied().filter(|&s| Some(s) != from).collect();
        if choices.is_empty() {
            return Decision::Fluff;
        }
        let p = choices[rand::thread_rng().gen_range(0..choices.len())];
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

    pub fn expired(&mut self, now: Instant) -> Vec<Embargoed> {
        let ids: Vec<[u8; 32]> = self.embargo.iter().filter(|(_, e)| e.deadline <= now).map(|(k, _)| *k).collect();
        ids.iter().filter_map(|k| self.embargo.remove(k)).collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dedupe_and_no_backflow() {
        let mut d = Dandelion::new();
        let ep = Duration::from_secs(600);
        assert!(matches!(d.decide([1; 32], None, &[5], ep, 0.9), Decision::Stem(5)));
        assert!(matches!(d.decide([1; 32], None, &[5], ep, 0.9), Decision::Drop));
        // only stem candidate is the sender → fluff
        assert!(matches!(d.decide([2; 32], Some(5), &[5], ep, 1.0), Decision::Fluff));
        assert!(matches!(d.decide([3; 32], Some(5), &[5, 6], ep, 1.0), Decision::Stem(6)));
        assert!(matches!(d.decide([4; 32], Some(9), &[5, 6], ep, 0.0), Decision::Fluff));
        d.mark_fluffed([7; 32]);
        assert!(matches!(d.decide([7; 32], None, &[5], ep, 1.0), Decision::Drop));
    }
}
