//! Independent bounded persistence and behavioral oracles, not an engine implementation.
use std::collections::{BTreeMap, BTreeSet};
use vetra_io::FileIo;
use vetra_types::IoFailure;

const MAX_FILE_BYTES: usize = 16 * 1024 * 1024;
const MAX_PENDING: usize = 4096;

#[derive(Clone, Debug)]
struct Write {
    offset: usize,
    bytes: Vec<u8>,
}

/// A bounded single-file simulator. Namespace/directory durability is a future STO-001 provider.
#[derive(Clone, Debug, Default)]
pub struct SimFile {
    durable: Vec<u8>,
    visible: Vec<u8>,
    pending: Vec<Write>,
    fail_next_sync: bool,
    poisoned: bool,
}

impl SimFile {
    pub fn fail_next_sync(&mut self) {
        self.fail_next_sync = true;
    }

    /// Explicit physical persistence order. Each pair is (pending write index, prefix byte count).
    /// Synced bytes are never discarded; pending sectors may survive, tear, reorder or vanish.
    pub fn power_loss(&mut self, survivors: &[(usize, usize)]) -> Result<(), IoFailure> {
        let mut seen = BTreeSet::new();
        for &(index, prefix) in survivors {
            let write = self.pending.get(index).ok_or(IoFailure::InvalidInput)?;
            if !seen.insert(index) || prefix > write.bytes.len() {
                return Err(IoFailure::InvalidInput);
            }
        }
        for &(index, prefix) in survivors {
            let write = &self.pending[index];
            apply(&mut self.durable, write.offset, &write.bytes[..prefix]);
        }
        self.visible.clone_from(&self.durable);
        self.pending.clear();
        self.poisoned = false;
        self.fail_next_sync = false;
        Ok(())
    }

    pub fn persisted(&self) -> &[u8] {
        &self.durable
    }
}

fn apply(target: &mut Vec<u8>, offset: usize, bytes: &[u8]) {
    if bytes.is_empty() {
        return;
    }
    target.resize(target.len().max(offset + bytes.len()), 0);
    target[offset..offset + bytes.len()].copy_from_slice(bytes);
}

impl FileIo for SimFile {
    fn read_at(&self, offset: u64, output: &mut [u8]) -> Result<usize, IoFailure> {
        let offset = usize::try_from(offset).map_err(|_| IoFailure::InvalidInput)?;
        if offset >= self.visible.len() {
            return Ok(0);
        }
        let count = output.len().min(self.visible.len() - offset);
        output[..count].copy_from_slice(&self.visible[offset..offset + count]);
        Ok(count)
    }

    fn write_at(&mut self, offset: u64, input: &[u8]) -> Result<usize, IoFailure> {
        if self.poisoned {
            return Err(IoFailure::DurabilityFailure);
        }
        let offset = usize::try_from(offset).map_err(|_| IoFailure::InvalidInput)?;
        let end = offset
            .checked_add(input.len())
            .ok_or(IoFailure::InvalidInput)?;
        if end > MAX_FILE_BYTES || self.pending.len() >= MAX_PENDING {
            return Err(IoFailure::OutOfSpace);
        }
        apply(&mut self.visible, offset, input);
        self.pending.push(Write {
            offset,
            bytes: input.to_vec(),
        });
        Ok(input.len())
    }

    fn len(&self) -> Result<u64, IoFailure> {
        Ok(self.visible.len() as u64)
    }

    fn truncate(&mut self, _length: u64) -> Result<(), IoFailure> {
        // Namespace/length durability is not simulated by a misleading resize.
        Err(IoFailure::InvalidInput)
    }

    fn sync_data(&mut self) -> Result<(), IoFailure> {
        if self.poisoned || self.fail_next_sync {
            self.poisoned = true;
            self.fail_next_sync = false;
            return Err(IoFailure::DurabilityFailure);
        }
        self.durable.clone_from(&self.visible);
        self.pending.clear();
        Ok(())
    }

    fn sync_all(&mut self) -> Result<(), IoFailure> {
        self.sync_data()
    }
}

pub type Effects = BTreeMap<String, String>;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Outcome {
    Acknowledged,
    Aborted,
    Unknown,
}

/// Expected effects come from the external workload; recovered effects are sampled independently.
pub fn check_atomicity(
    outcome: Outcome,
    expected: &Effects,
    recovered: Option<&Effects>,
) -> Result<(), String> {
    let valid = match outcome {
        Outcome::Acknowledged => recovered == Some(expected),
        Outcome::Aborted => recovered.is_none(),
        Outcome::Unknown => recovered.is_none() || recovered == Some(expected),
    };
    valid
        .then_some(())
        .ok_or_else(|| format!("atomicity violation for {outcome:?}"))
}

pub type State = BTreeMap<String, String>;

#[derive(Clone, Debug)]
pub enum Read {
    Point {
        key: String,
        observed: Option<String>,
    },
    Range {
        start: String,
        end: String,
        observed: State,
    },
}

#[derive(Clone, Debug)]
pub struct Transaction {
    pub id: u64,
    pub reads: Vec<Read>,
    pub writes: BTreeMap<String, Option<String>>,
    pub must_follow: Vec<u64>,
}

/// Exhaustively finds a serial witness for <=8 committed transactions, including range predicates.
/// This is intentionally independent of locks, MVCC and engine commit ordering.
pub fn serial_witness(initial: &State, transactions: &[Transaction]) -> Result<Vec<u64>, String> {
    if transactions.len() > 8 || initial.len() > 1024 {
        return Err("oracle budget exceeded".into());
    }
    let ids: BTreeSet<_> = transactions.iter().map(|tx| tx.id).collect();
    if ids.len() != transactions.len() || ids.contains(&0) {
        return Err("invalid transaction identities".into());
    }
    for tx in transactions {
        if tx.reads.len() > 1024 || tx.writes.len() > 1024 || tx.must_follow.len() > 8 {
            return Err("oracle budget exceeded".into());
        }
        if tx.must_follow.iter().any(|id| !ids.contains(id)) {
            return Err("unknown predecessor".into());
        }
        for read in &tx.reads {
            if let Read::Range { start, end, .. } = read {
                if start > end {
                    return Err("invalid predicate range".into());
                }
            }
        }
    }
    search(initial, transactions, &mut Vec::new())
        .ok_or_else(|| "no serial order explains observations".into())
}

fn search(state: &State, transactions: &[Transaction], order: &mut Vec<u64>) -> Option<Vec<u64>> {
    if order.len() == transactions.len() {
        return Some(order.clone());
    }
    for tx in transactions {
        if order.contains(&tx.id) || tx.must_follow.iter().any(|id| !order.contains(id)) {
            continue;
        }
        let reads_match = tx.reads.iter().all(|read| match read {
            Read::Point { key, observed } => state.get(key) == observed.as_ref(),
            Read::Range {
                start,
                end,
                observed,
            } => {
                let actual: State = state
                    .range(start.clone()..end.clone())
                    .map(|(k, v)| (k.clone(), v.clone()))
                    .collect();
                &actual == observed
            }
        });
        if !reads_match {
            continue;
        }
        let mut next = state.clone();
        for (key, value) in &tx.writes {
            match value {
                Some(value) => {
                    next.insert(key.clone(), value.clone());
                }
                None => {
                    next.remove(key);
                }
            }
        }
        order.push(tx.id);
        if let Some(witness) = search(&next, transactions, order) {
            return Some(witness);
        }
        order.pop();
    }
    None
}

#[derive(Clone, Copy, Debug)]
pub enum LeaseEvent {
    Claim { job: u64, token: u64 },
    Expire { job: u64, token: u64 },
    Complete { job: u64, token: u64 },
}

/// Trace contains committed claims/completions, not attempted or external executions.
pub fn check_leases(events: &[LeaseEvent]) -> Result<(), String> {
    let mut active = BTreeMap::new();
    let mut highest = BTreeMap::new();
    let mut completed = BTreeSet::new();
    for event in events {
        match *event {
            LeaseEvent::Claim { job, token } => {
                if job == 0
                    || token == 0
                    || active.contains_key(&job)
                    || completed.contains(&job)
                    || token <= *highest.get(&job).unwrap_or(&0)
                {
                    return Err("duplicate/stale claim".into());
                }
                active.insert(job, token);
                highest.insert(job, token);
            }
            LeaseEvent::Expire { job, token } | LeaseEvent::Complete { job, token } => {
                if active.remove(&job) != Some(token) {
                    return Err("stale ownership transition".into());
                }
                if matches!(event, LeaseEvent::Complete { .. }) {
                    completed.insert(job);
                }
            }
        }
    }
    Ok(())
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Change {
    pub csn: u64,
    pub position: u32,
    pub key: String,
    pub value: Option<String>,
}

/// Reconstruct snapshot and eligible stream from externally recorded committed row mutations.
/// Duplicate deliveries are allowed; gaps, invented payloads and ordering reversals are not.
pub fn check_handoff(
    history: &[Change],
    basis: u64,
    snapshot: &State,
    stream: &[Change],
) -> Result<(), String> {
    let mut expected_snapshot = State::new();
    let mut expected_stream = BTreeMap::new();
    let mut previous = None;
    for change in history {
        let cursor = (change.csn, change.position);
        if change.csn == 0 || previous.is_some_and(|p| p >= cursor) {
            return Err("invalid source history".into());
        }
        previous = Some(cursor);
        if change.csn <= basis {
            match &change.value {
                Some(value) => {
                    expected_snapshot.insert(change.key.clone(), value.clone());
                }
                None => {
                    expected_snapshot.remove(&change.key);
                }
            }
        } else {
            expected_stream.insert(cursor, change);
        }
    }
    if snapshot != &expected_snapshot {
        return Err("incorrect snapshot".into());
    }
    let mut seen = BTreeSet::new();
    let mut last = None;
    for change in stream {
        let cursor = (change.csn, change.position);
        if last.is_some_and(|p| p > cursor) || expected_stream.get(&cursor).copied() != Some(change)
        {
            return Err("invalid stream order/payload".into());
        }
        last = Some(cursor);
        seen.insert(cursor);
    }
    if seen.len() != expected_stream.len() {
        return Err("snapshot-to-stream gap".into());
    }
    Ok(())
}

/// Deterministic bounded campaign: sync survival, unsynced loss and torn-prefix choices.
/// Returns a replayable seed/case error rather than relying on a database recovery codec.
pub fn campaign(seed: u64, cases: u32) -> Result<(), String> {
    if cases > 100_000 {
        return Err("campaign budget exceeded".into());
    }
    let mut random = seed;
    for case in 0..cases {
        random = random.wrapping_mul(6364136223846793005).wrapping_add(1);
        let mut file = SimFile::default();
        let synced = random.to_le_bytes();
        file.write_at(0, &synced).map_err(|e| format!("{e:?}"))?;
        file.sync_all().map_err(|e| format!("{e:?}"))?;
        file.write_at(8, b"pending").map_err(|e| format!("{e:?}"))?;
        let prefix = (random % 8) as usize;
        file.power_loss(&[(0, prefix)])
            .map_err(|e| format!("{e:?}"))?;
        let mut expected = synced.to_vec();
        expected.extend_from_slice(&b"pending"[..prefix]);
        if file.persisted() != expected {
            return Err(format!("seed={seed} case={case} prefix={prefix}"));
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn synced_data_survives_unsynced_loss() {
        let mut file = SimFile::default();
        file.write_at(0, b"old").unwrap();
        file.sync_data().unwrap();
        file.write_at(0, b"new").unwrap();
        file.power_loss(&[]).unwrap();
        assert_eq!(file.persisted(), b"old");
    }

    #[test]
    fn pending_writes_can_tear_and_reorder() {
        let mut file = SimFile::default();
        file.write_at(0, b"aaaa").unwrap();
        file.write_at(0, b"bbbb").unwrap();
        file.power_loss(&[(1, 4), (0, 2)]).unwrap();
        assert_eq!(file.persisted(), b"aabb");
    }

    #[test]
    fn invalid_crash_plan_leaves_state_intact() {
        let mut file = SimFile::default();
        file.write_at(0, b"test").unwrap();
        assert_eq!(
            file.power_loss(&[(0, 2), (9, 1)]),
            Err(IoFailure::InvalidInput)
        );
        assert!(file.persisted().is_empty());
    }

    #[test]
    fn sync_failure_poisons_writes_until_reopen() {
        let mut file = SimFile::default();
        file.write_at(0, b"commit").unwrap();
        file.fail_next_sync();
        assert_eq!(file.sync_data(), Err(IoFailure::DurabilityFailure));
        assert_eq!(file.write_at(0, b"bad"), Err(IoFailure::DurabilityFailure));
        assert_eq!(file.sync_data(), Err(IoFailure::DurabilityFailure));
        file.power_loss(&[(0, 6)]).unwrap();
        assert_eq!(file.persisted(), b"commit");
        file.write_at(6, b"ok").unwrap();
    }

    #[test]
    fn resource_limits_reject_without_partial_write() {
        let mut file = SimFile::default();
        assert_eq!(file.write_at(u64::MAX, b"x"), Err(IoFailure::InvalidInput));
        assert_eq!(
            file.write_at(MAX_FILE_BYTES as u64, b"x"),
            Err(IoFailure::OutOfSpace)
        );
        assert_eq!(file.len().unwrap(), 0);
    }

    #[test]
    fn external_oracle_detects_partial_cross_feature_commit() {
        let expected: Effects = [
            ("row", "order"),
            ("ledger", "tx7"),
            ("job", "fulfill"),
            ("event", "created"),
            ("offset", "9"),
        ]
        .into_iter()
        .map(|(k, v)| (k.into(), v.into()))
        .collect();
        let mut partial = expected.clone();
        partial.remove("job");
        assert!(check_atomicity(Outcome::Acknowledged, &expected, Some(&partial)).is_err());
        assert!(check_atomicity(Outcome::Unknown, &expected, Some(&partial)).is_err());
        assert!(check_atomicity(Outcome::Unknown, &expected, None).is_ok());
        assert!(check_atomicity(Outcome::Unknown, &expected, Some(&expected)).is_ok());
        assert!(check_atomicity(Outcome::Aborted, &expected, Some(&expected)).is_err());
        assert!(check_atomicity(Outcome::Acknowledged, &expected, None).is_err());
    }

    #[test]
    fn serial_oracle_rejects_write_skew() {
        let initial = State::from([("a".into(), "1".into()), ("b".into(), "1".into())]);
        let a = Transaction {
            id: 1,
            reads: vec![Read::Point {
                key: "b".into(),
                observed: Some("1".into()),
            }],
            writes: BTreeMap::from([("a".into(), Some("0".into()))]),
            must_follow: vec![],
        };
        let b = Transaction {
            id: 2,
            reads: vec![Read::Point {
                key: "a".into(),
                observed: Some("1".into()),
            }],
            writes: BTreeMap::from([("b".into(), Some("0".into()))]),
            must_follow: vec![],
        };
        assert!(serial_witness(&initial, &[a, b]).is_err());
    }

    #[test]
    fn serial_oracle_finds_witness_and_detects_phantom_cycle() {
        let a = Transaction {
            id: 1,
            reads: vec![Read::Range {
                start: "a".into(),
                end: "z".into(),
                observed: State::new(),
            }],
            writes: BTreeMap::from([("b".into(), Some("1".into()))]),
            must_follow: vec![],
        };
        let b = Transaction {
            id: 2,
            reads: vec![Read::Range {
                start: "a".into(),
                end: "z".into(),
                observed: State::new(),
            }],
            writes: BTreeMap::from([("c".into(), Some("1".into()))]),
            must_follow: vec![],
        };
        assert_eq!(
            serial_witness(&State::new(), std::slice::from_ref(&a)).unwrap(),
            vec![1]
        );
        assert!(serial_witness(&State::new(), &[a, b]).is_err());
    }

    #[test]
    fn expired_worker_cannot_complete_reclaimed_job() {
        let prefix = [
            LeaseEvent::Claim { job: 1, token: 1 },
            LeaseEvent::Expire { job: 1, token: 1 },
            LeaseEvent::Claim { job: 1, token: 2 },
        ];
        assert!(check_leases(&prefix).is_ok());
        let mut stale = prefix.to_vec();
        stale.push(LeaseEvent::Complete { job: 1, token: 1 });
        assert!(check_leases(&stale).is_err());
        let mut valid = prefix.to_vec();
        valid.push(LeaseEvent::Complete { job: 1, token: 2 });
        assert!(check_leases(&valid).is_ok());
        valid.push(LeaseEvent::Claim { job: 1, token: 3 });
        assert!(check_leases(&valid).is_err());
    }

    #[test]
    fn handoff_detects_gaps_and_allows_matching_duplicates() {
        let history = [
            Change {
                csn: 1,
                position: 0,
                key: "a".into(),
                value: Some("old".into()),
            },
            Change {
                csn: 2,
                position: 0,
                key: "a".into(),
                value: Some("new".into()),
            },
        ];
        let snapshot = State::from([("a".into(), "old".into())]);
        assert!(check_handoff(&history, 1, &snapshot, &[]).is_err());
        assert!(
            check_handoff(
                &history,
                1,
                &snapshot,
                &[history[1].clone(), history[1].clone()]
            )
            .is_ok()
        );
        assert!(check_handoff(&history, 1, &State::new(), &history[1..]).is_err());
        let mut incorrect = history[1].clone();
        incorrect.value = None;
        assert!(check_handoff(&history, 1, &snapshot, &[incorrect]).is_err());
    }

    #[test]
    fn seeded_campaign_is_reproducible_and_bounded() {
        assert_eq!(campaign(42, 500), campaign(42, 500));
        campaign(42, 500).unwrap();
        assert!(campaign(42, 100_001).is_err());
    }
}
