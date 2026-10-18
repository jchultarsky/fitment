//! Queues, lanes and fairness (docs/research/m6-api.md, JS1, JS4, JS5): per
//! lane, one FIFO per owner, owners taken round-robin; at most the lane's
//! slots running, and per owner `jobs.per_owner_running` running and
//! `jobs.per_owner_queued` queued; a lane's queue holds at most its cap,
//! and a submission past it, or past its owner's, is refused before any
//! row is written. The CPU and memory permits (JS2, JS3) are the
//! dispatcher's, in `jobs/mod.rs`. Nothing here blocks or awaits: the
//! lock is held only for these counts.

use std::collections::{BTreeMap, HashMap, VecDeque};
use std::sync::Arc;

use super::Control;
use super::model::{Kind, Lane};
use crate::config::Jobs as JobsConfig;

/// A queued job, as the dispatcher needs it.
#[derive(Debug, Clone)]
pub(crate) struct Entry {
    /// `job.key`.
    pub key: i64,
    /// `j_…`.
    pub id: String,
    /// Its owner's key.
    pub owner: i64,
    /// Its kind.
    pub kind: Kind,
    /// Its CPU permits (JS2).
    pub cpu: u32,
    /// Its memory estimate, in MiB (JS3).
    pub memory_mib: u32,
}

/// Why a submission was refused for load (JS5).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Full {
    /// The lane's queue: `503 queue_full`.
    Lane,
    /// The owner's queued jobs in the lane: `429 too_many_jobs`.
    Owner,
}

/// One lane's queue and counts.
#[derive(Debug, Default)]
struct LaneQueue {
    slots: usize,
    queue_cap: usize,
    owner_queued_cap: usize,
    owner_running_cap: usize,
    queues: BTreeMap<i64, VecDeque<Entry>>,
    /// Owners with queued jobs, in turn order (JS4's round-robin).
    order: VecDeque<i64>,
    queued: usize,
    reserved: usize,
    owner_reserved: BTreeMap<i64, usize>,
    running: usize,
    owner_running: BTreeMap<i64, usize>,
}

impl LaneQueue {
    fn owner_queued(&self, owner: i64) -> usize {
        self.queues.get(&owner).map_or(0, VecDeque::len)
            + self.owner_reserved.get(&owner).copied().unwrap_or(0)
    }

    fn push(&mut self, entry: Entry) {
        let owner = entry.owner;
        let queue = self.queues.entry(owner).or_default();
        if queue.is_empty() && !self.order.contains(&owner) {
            self.order.push_back(owner);
        }
        queue.push_back(entry);
        self.queued += 1;
    }

    /// The next job to run (JS4): the first owner in turn under their
    /// running cap, their oldest job; that owner goes to the back.
    fn pick(&mut self) -> Option<Entry> {
        if self.running >= self.slots {
            return None;
        }
        let turn = (0..self.order.len()).find(|&i| {
            let owner = self.order[i];
            self.owner_running.get(&owner).copied().unwrap_or(0) < self.owner_running_cap
        })?;
        let owner = self.order.remove(turn)?;
        let queue = self.queues.get_mut(&owner)?;
        let entry = queue.pop_front()?;
        if queue.is_empty() {
            self.queues.remove(&owner);
        } else {
            self.order.push_back(owner);
        }
        self.queued -= 1;
        self.running += 1;
        *self.owner_running.entry(owner).or_insert(0) += 1;
        Some(entry)
    }

    fn remove(&mut self, key: i64) -> bool {
        let Some((&owner, queue)) = self
            .queues
            .iter_mut()
            .find(|(_, q)| q.iter().any(|e| e.key == key))
        else {
            return false;
        };
        queue.retain(|e| e.key != key);
        if queue.is_empty() {
            self.queues.remove(&owner);
            self.order.retain(|o| *o != owner);
        }
        self.queued -= 1;
        true
    }

    fn done(&mut self, owner: i64) {
        self.running = self.running.saturating_sub(1);
        if let Some(n) = self.owner_running.get_mut(&owner) {
            *n = n.saturating_sub(1);
            if *n == 0 {
                self.owner_running.remove(&owner);
            }
        }
    }
}

/// Every lane's queue, and the controls of the jobs dispatched.
#[derive(Debug)]
pub(crate) struct Sched {
    lanes: [LaneQueue; 3],
    /// Jobs taken from a queue and not yet ended: waiting for permits or
    /// running (JS8's flags).
    pub live: HashMap<i64, Arc<Control>>,
}

impl Sched {
    /// The lanes of `config` (JS1, JS4, JS5).
    pub fn new(config: &JobsConfig) -> Self {
        let lane = |slots: u32, cap: u32| LaneQueue {
            slots: usize::try_from(slots.max(1)).unwrap_or(1),
            queue_cap: usize::try_from(cap).unwrap_or(usize::MAX),
            owner_queued_cap: usize::try_from(config.per_owner_queued).unwrap_or(usize::MAX),
            owner_running_cap: usize::try_from(config.per_owner_running.max(1)).unwrap_or(1),
            ..LaneQueue::default()
        };
        Self {
            lanes: [
                lane(config.query_slots(), config.queue_query),
                lane(1, config.queue_write),
                lane(1, config.queue_maintenance),
            ],
            live: HashMap::new(),
        }
    }

    /// JS5: room for one more of `owner`'s jobs in `lane`, reserved, or why
    /// there is none.
    pub fn reserve(&mut self, lane: Lane, owner: i64) -> Result<(), Full> {
        let queue = &mut self.lanes[lane.index()];
        if queue.queued + queue.reserved >= queue.queue_cap {
            return Err(Full::Lane);
        }
        if queue.owner_queued(owner) >= queue.owner_queued_cap {
            return Err(Full::Owner);
        }
        queue.reserved += 1;
        *queue.owner_reserved.entry(owner).or_insert(0) += 1;
        Ok(())
    }

    /// Gives a reservation back.
    pub fn unreserve(&mut self, lane: Lane, owner: i64) {
        let queue = &mut self.lanes[lane.index()];
        queue.reserved = queue.reserved.saturating_sub(1);
        if let Some(n) = queue.owner_reserved.get_mut(&owner) {
            *n = n.saturating_sub(1);
            if *n == 0 {
                queue.owner_reserved.remove(&owner);
            }
        }
    }

    /// Queues `entry`, past any cap: recovery's re-queued jobs (SD4).
    pub fn push(&mut self, entry: Entry) {
        self.lanes[entry.kind.lane().index()].push(entry);
    }

    /// The jobs each lane may start now (JS4).
    pub fn picks(&mut self) -> Vec<Entry> {
        let mut out = Vec::new();
        for lane in &mut self.lanes {
            while let Some(entry) = lane.pick() {
                out.push(entry);
            }
        }
        out
    }

    /// A cancelled job leaves its queue (JB5). False if it is not queued.
    pub fn remove(&mut self, key: i64) -> bool {
        self.lanes.iter_mut().any(|lane| lane.remove(key))
    }

    /// A dispatched job ended or gave its slot back.
    pub fn done(&mut self, entry: &Entry) {
        self.lanes[entry.kind.lane().index()].done(entry.owner);
        self.live.remove(&entry.key);
    }

    /// A lane's queued and running counts (OP6's gauges).
    pub fn counts(&self, lane: Lane) -> (usize, usize) {
        let queue = &self.lanes[lane.index()];
        (queue.queued, queue.running)
    }

    /// A lane's cap is reached (DC11's `queues_full`).
    pub fn full(&self) -> bool {
        self.lanes
            .iter()
            .any(|q| q.queued + q.reserved >= q.queue_cap)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(key: i64, owner: i64) -> Entry {
        Entry {
            key,
            id: format!("j_{key}"),
            owner,
            kind: Kind::Features,
            cpu: 1,
            memory_mib: 1,
        }
    }

    fn config(slots: u32) -> JobsConfig {
        JobsConfig {
            cpu: slots * 4,
            per_owner_running: 2,
            per_owner_queued: 3,
            queue_query: 5,
            ..JobsConfig::default()
        }
    }

    /// JS4: owners are taken round-robin, each FIFO, within the slots and
    /// the per-owner running cap.
    #[test]
    fn owners_take_turns() {
        let mut sched = Sched::new(&config(3));
        for (key, owner) in [(1, 10), (2, 10), (3, 10), (4, 20), (5, 30)] {
            sched.push(entry(key, owner));
        }
        let first: Vec<i64> = sched.picks().iter().map(|e| e.key).collect();
        assert_eq!(first, [1, 4, 5]);
        sched.done(&entry(4, 20));
        let next: Vec<i64> = sched.picks().iter().map(|e| e.key).collect();
        assert_eq!(next, [2]);
        sched.done(&entry(5, 30));
        // Owner 10 has two running: its third waits.
        assert!(sched.picks().is_empty());
        sched.done(&entry(1, 10));
        assert_eq!(sched.picks()[0].key, 3);
    }

    /// JS5: a lane's cap and an owner's, reservations counted.
    #[test]
    fn caps_refuse_before_rows() {
        let mut sched = Sched::new(&config(1));
        for _ in 0..3 {
            sched.reserve(Lane::Query, 10).unwrap();
        }
        assert_eq!(sched.reserve(Lane::Query, 10), Err(Full::Owner));
        sched.reserve(Lane::Query, 20).unwrap();
        sched.reserve(Lane::Query, 30).unwrap();
        assert_eq!(sched.reserve(Lane::Query, 40), Err(Full::Lane));
        assert!(sched.full());
        sched.unreserve(Lane::Query, 30);
        assert!(!sched.full());
        assert!(!sched.remove(99));
    }
}
