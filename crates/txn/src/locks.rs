//! Strict transaction locks, independent of page latches. Values never enter diagnostics.
use crate::{Error, Result};
use std::collections::{BTreeMap, BTreeSet, VecDeque};
use vetra_wal::envelope::{Key, Participant};
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Resource {
    pub participant: Participant,
    pub object: u64,
    pub lower: Option<Vec<u8>>,
    pub upper: Option<Vec<u8>>,
}
impl Resource {
    pub fn key(key: &Key) -> Self {
        Self {
            participant: key.participant,
            object: key.object,
            lower: Some(key.bytes.clone()),
            upper: Some(key.bytes.clone()),
        }
    }
    pub fn table(participant: Participant, object: u64) -> Self {
        Self {
            participant,
            object,
            lower: None,
            upper: None,
        }
    }
    fn overlaps(&self, other: &Self) -> bool {
        self.participant == other.participant
            && self.object == other.object
            && self
                .upper
                .as_ref()
                .zip(other.lower.as_ref())
                .is_none_or(|(a, b)| a >= b)
            && other
                .upper
                .as_ref()
                .zip(self.lower.as_ref())
                .is_none_or(|(a, b)| a >= b)
    }
    fn validate(&self) -> Result<()> {
        if self.object == 0
            || self.lower.as_ref().is_some_and(|b| b.len() > 2048)
            || self.upper.as_ref().is_some_and(|b| b.len() > 2048)
            || self
                .lower
                .as_ref()
                .zip(self.upper.as_ref())
                .is_some_and(|(a, b)| a > b)
        {
            return Err(Error::Limit);
        }
        Ok(())
    }
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Mode {
    Shared,
    Exclusive,
}
#[derive(Clone, Debug)]
struct Request {
    tx: u64,
    resource: Resource,
    mode: Mode,
}
impl Request {
    fn conflicts(&self, other: &Self) -> bool {
        self.tx != other.tx
            && (self.mode == Mode::Exclusive || other.mode == Mode::Exclusive)
            && self.resource.overlaps(&other.resource)
    }
}
#[derive(Default)]
pub struct LockTable {
    held: Vec<Request>,
    waiting: VecDeque<Request>,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Diagnostics {
    pub held: usize,
    pub waiting: usize,
    pub owners: Vec<u64>,
}
impl LockTable {
    /// FIFO among overlapping incompatible requests. Deadlock victim is the youngest attempt.
    pub fn acquire(
        &mut self,
        tx: u64,
        resource: Resource,
        mode: Mode,
        limit: usize,
    ) -> Result<bool> {
        resource.validate()?;
        let request = Request { tx, resource, mode };
        if self.held.iter().any(|r| {
            r.tx == tx
                && r.resource == request.resource
                && (r.mode == mode || r.mode == Mode::Exclusive)
        }) {
            self.waiting.retain(|r| r.tx != tx);
            return Ok(true);
        }
        if self.held.len() + self.waiting.len() >= limit && !self.waiting.iter().any(|r| r.tx == tx)
        {
            return Err(Error::Limit);
        }
        if let Some(old) = self.waiting.iter().find(|r| r.tx == tx) {
            if old.resource != request.resource || old.mode != mode {
                return Err(Error::State);
            }
        } else {
            self.waiting.push_back(request.clone());
        }
        let blocked = self.held.iter().any(|r| request.conflicts(r))
            || self
                .waiting
                .iter()
                .take_while(|r| r.tx != tx)
                .any(|r| request.conflicts(r));
        if !blocked {
            self.waiting.retain(|r| r.tx != tx);
            self.held.push(request);
            return Ok(true);
        }
        Ok(false)
    }
    pub fn cancel(&mut self, tx: u64) {
        self.waiting.retain(|r| r.tx != tx);
    }
    pub fn release(&mut self, tx: u64) {
        self.held.retain(|r| r.tx != tx);
        self.cancel(tx);
    }
    pub fn diagnostics(&self) -> Diagnostics {
        Diagnostics {
            held: self.held.len(),
            waiting: self.waiting.len(),
            owners: self
                .held
                .iter()
                .map(|r| r.tx)
                .collect::<BTreeSet<_>>()
                .into_iter()
                .take(64)
                .collect(),
        }
    }
    pub fn victim(&self) -> Option<u64> {
        let mut edges: BTreeMap<u64, BTreeSet<u64>> = BTreeMap::new();
        for request in &self.waiting {
            let mut blockers = BTreeSet::new();
            for held in &self.held {
                if request.conflicts(held) {
                    blockers.insert(held.tx);
                }
            }
            for earlier in self.waiting.iter().take_while(|r| r.tx != request.tx) {
                if request.conflicts(earlier) {
                    blockers.insert(earlier.tx);
                }
            }
            edges.insert(request.tx, blockers);
        }
        fn visit(
            tx: u64,
            edges: &BTreeMap<u64, BTreeSet<u64>>,
            path: &mut Vec<u64>,
            done: &mut BTreeSet<u64>,
        ) -> Option<u64> {
            if let Some(i) = path.iter().position(|t| *t == tx) {
                return path[i..].iter().copied().max();
            }
            if !done.insert(tx) {
                return None;
            }
            path.push(tx);
            if let Some(next) = edges.get(&tx) {
                for &n in next {
                    if let Some(v) = visit(n, edges, path, done) {
                        return Some(v);
                    }
                }
            }
            path.pop();
            None
        }
        for &tx in edges.keys() {
            if let Some(v) = visit(tx, &edges, &mut Vec::new(), &mut BTreeSet::new()) {
                return Some(v);
            }
        }
        None
    }
}
