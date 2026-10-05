//! Native single-node transaction core. SQL/network adapters are subsequent milestones.
pub mod locks;
use locks::{LockTable, Mode, Resource};
use std::{
    collections::BTreeMap,
    sync::{
        Arc, Condvar, Mutex, MutexGuard,
        atomic::{AtomicBool, Ordering},
    },
    time::{Duration, Instant},
};
pub use vetra_wal::envelope::{Image, Key, Participant, Value};
use vetra_wal::{
    Kind, Lineage, Log,
    codec::{CHUNK_BYTES, crc32c},
    envelope::{Envelope, Operation, committed, encode_image},
};
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Error {
    Wal(vetra_wal::Error),
    Storage(vetra_storage::Error),
    State,
    Failed,
    Serialization,
    Deadlock,
    Timeout,
    Cancelled,
    Limit,
    Unauthorized,
    Expired,
    UnknownOutcome,
    Corrupt(&'static str),
}
impl From<vetra_wal::Error> for Error {
    fn from(e: vetra_wal::Error) -> Self {
        Self::Wal(e)
    }
}
impl From<vetra_storage::Error> for Error {
    fn from(e: vetra_storage::Error) -> Self {
        Self::Storage(e)
    }
}
pub type Result<T> = std::result::Result<T, Error>;
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Isolation {
    ReadUncommitted,
    ReadCommitted,
    RepeatableRead,
    Serializable,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Status {
    Active,
    Failed,
    Committed(u64),
    Aborted,
    Unknown,
}
#[derive(Clone, Debug)]
pub struct Limits {
    pub transactions: usize,
    pub operations: usize,
    pub staged_bytes: usize,
    pub locks: usize,
    pub pins: usize,
    pub wait: Duration,
    pub history_bytes: usize,
}
impl Default for Limits {
    fn default() -> Self {
        Self {
            transactions: 256,
            operations: 4096,
            staged_bytes: 16 * 1024 * 1024,
            locks: 8192,
            pins: 1024,
            wait: Duration::from_secs(5),
            history_bytes: 256 * 1024 * 1024,
        }
    }
}
/// Trusted engine context. Adapters authenticate/authorize before constructing it.
#[derive(Clone, Debug)]
pub struct Context {
    pub principal: u64,
    pub writable: Vec<(Participant, u64)>,
    pub readable: Vec<(Participant, u64)>,
    pub metadata: BTreeMap<String, String>,
}
impl Context {
    pub fn object(principal: u64, participant: Participant, object: u64) -> Self {
        Self {
            principal,
            writable: vec![(participant, object)],
            readable: vec![(participant, object)],
            metadata: BTreeMap::new(),
        }
    }
}
#[derive(Clone)]
struct Savepoint {
    name: String,
    len: usize,
    bytes: usize,
}
struct Attempt {
    isolation: Isolation,
    basis: u64,
    status: Status,
    context: Context,
    operations: Vec<Operation>,
    bytes: usize,
    savepoints: Vec<Savepoint>,
    last: u64,
    serial_start: u64,
    reads: Vec<(Resource, u64)>,
    deadlock: bool,
    statement_basis: Option<u64>,
}
#[derive(Clone)]
struct Version {
    csn: u64,
    image: Option<Image>,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PinKind {
    Reader,
    Undo,
    Backup,
    Service,
}
#[derive(Clone, Debug)]
pub struct PinInfo {
    pub id: u64,
    pub basis: u64,
    pub kind: PinKind,
    pub owner: u64,
    pub expires: Option<u64>,
}
struct PreparedJournal {
    inner: vetra_recovery_api::MemoryJournal,
    bytes: usize,
    limit: usize,
}
impl vetra_recovery_api::StructuralJournal for PreparedJournal {
    fn seal(
        &mut self,
        a: vetra_recovery_api::StructuralAction,
    ) -> std::result::Result<u64, vetra_types::IoFailure> {
        self.seal_batch(vec![a])
    }
    fn seal_batch(
        &mut self,
        actions: Vec<vetra_recovery_api::StructuralAction>,
    ) -> std::result::Result<u64, vetra_types::IoFailure> {
        let bytes = actions
            .iter()
            .map(|a| {
                a.pages.iter().map(|p| p.bytes.len() + 16).sum::<usize>()
                    + a.allocation.len()
                    + a.retired.len() * 16
                    + a.record_ids
                        .iter()
                        .map(|(k, _)| k.len() + 12)
                        .sum::<usize>()
                    + 64
            })
            .sum::<usize>();
        if self.bytes.checked_add(bytes).is_none_or(|n| n > self.limit) {
            return Err(vetra_types::IoFailure::OutOfSpace);
        }
        self.bytes += bytes;
        self.inner.seal_batch(actions)
    }
}
struct Projection {
    rows: vetra_storage::rows::RowStore,
    identities: BTreeMap<Key, u64>,
    next_row: u64,
}
impl Projection {
    fn new() -> Result<Self> {
        Ok(Self {
            rows: vetra_storage::rows::RowStore::new(
                2,
                3,
                vetra_storage::tree::TreeConfig::default(),
            )?,
            identities: BTreeMap::new(),
            next_row: 1,
        })
    }
    fn compact(
        &self,
        versions: &BTreeMap<Key, Vec<Version>>,
        journal: &mut (impl vetra_recovery_api::StructuralJournal + ?Sized),
    ) -> Result<Self> {
        let mut projection = self.staged_copy();
        projection.rows.retain(
            |row| {
                versions
                    .get(&Key {
                        participant: Participant::Row,
                        object: row.id.table,
                        bytes: row.primary.clone(),
                    })
                    .is_some_and(|versions| versions.iter().any(|v| v.csn == row.id.version))
            },
            journal,
        )?;
        Ok(projection)
    }
    fn staged_copy(&self) -> Self {
        Self {
            rows: self.rows.staged_copy(),
            identities: self.identities.clone(),
            next_row: self.next_row,
        }
    }
    fn apply(
        &mut self,
        e: &Envelope,
        journal: &mut (impl vetra_recovery_api::StructuralJournal + ?Sized),
    ) -> Result<()> {
        let mut final_rows = BTreeMap::new();
        for op in e
            .operations
            .iter()
            .filter(|op| op.key.participant == Participant::Row)
        {
            final_rows.insert(op.key.clone(), op);
        }
        for (key, op) in final_rows {
            let row = if let Some(row) = self.identities.get(&key) {
                *row
            } else {
                let row = self.next_row;
                self.next_row = self.next_row.checked_add(1).ok_or(Error::Limit)?;
                self.identities.insert(key.clone(), row);
                row
            };
            let payload = op
                .after
                .as_ref()
                .map(encode_image)
                .transpose()?
                .unwrap_or_default();
            self.rows.append(
                vetra_storage::rows::RowVersion {
                    id: vetra_storage::rows::VersionId {
                        table: key.object,
                        row,
                        version: e.csn,
                    },
                    schema: op.schema,
                    creator: e.tx,
                    primary: key.bytes,
                    payload,
                    deleted: op.after.is_none(),
                },
                journal,
            )?;
        }
        self.rows.validate()?;
        Ok(())
    }
}
struct Core {
    projection: Projection,
    journal: Box<dyn vetra_recovery_api::StructuralJournal + Send>,
    log: Box<dyn Log>,
    lineage: Lineage,
    attempts: BTreeMap<u64, Attempt>,
    versions: BTreeMap<Key, Vec<Version>>,
    ledger: Vec<Envelope>,
    locks: LockTable,
    visible: u64,
    serial_frontier: u64,
    next_tx: u64,
    next_csn: u64,
    pins: BTreeMap<u64, PinInfo>,
    next_pin: u64,
    expired_before: u64,
    history_bytes: usize,
    poisoned: bool,
    limits: Limits,
}
struct Shared {
    core: Mutex<Core>,
    changed: Condvar,
}
#[derive(Clone)]
pub struct Manager {
    shared: Arc<Shared>,
}
pub struct Transaction {
    manager: Manager,
    pub id: u64,
}
pub struct Snapshot {
    manager: Manager,
    id: u64,
    basis: u64,
}
fn current(core: &Core, key: &Key, basis: u64) -> Option<Image> {
    core.versions
        .get(key)
        .and_then(|v| v.iter().rev().find(|v| v.csn <= basis))
        .and_then(|v| v.image.clone())
}
fn projected(core: &Core, key: &Key, basis: u64) -> Result<Option<Image>> {
    if key.participant != Participant::Row {
        return Ok(current(core, key, basis));
    }
    struct Visible<'a> {
        ledger: &'a [Envelope],
        basis: u64,
    }
    impl vetra_storage::rows::Visibility for Visible<'_> {
        fn visible(&self, creator: u64) -> bool {
            self.ledger
                .iter()
                .any(|e| e.tx == creator && e.csn <= self.basis)
        }
    }
    let row = core.projection.rows.lookup(
        key.object,
        &key.bytes,
        &Visible {
            ledger: &core.ledger,
            basis,
        },
    )?;
    let result = row
        .map(|r| vetra_wal::envelope::decode_image(&r.payload))
        .transpose()?;
    if result != current(core, key, basis) {
        return Err(Error::Corrupt("row/index/ledger divergence"));
    }
    Ok(result)
}
fn changed_since(core: &Core, resource: &Resource, basis: u64) -> bool {
    core.versions.iter().any(|(key, versions)| {
        key.participant == resource.participant
            && key.object == resource.object
            && resource.lower.as_ref().is_none_or(|b| &key.bytes >= b)
            && resource.upper.as_ref().is_none_or(|b| &key.bytes <= b)
            && versions.last().is_some_and(|v| v.csn > basis)
    })
}
fn latest(core: &Core, key: &Key) -> u64 {
    core.versions
        .get(key)
        .and_then(|v| v.last())
        .map_or(0, |v| v.csn)
}
fn check_active(a: &Attempt) -> Result<()> {
    match a.status {
        Status::Active => Ok(()),
        Status::Failed => Err(Error::Failed),
        Status::Unknown => Err(Error::UnknownOutcome),
        _ => Err(Error::State),
    }
}
fn apply(core: &mut Core, e: Envelope) -> Result<()> {
    if e.csn <= core.visible {
        return Err(Error::Corrupt("CSN order"));
    }
    let mut overlay = BTreeMap::new();
    for op in &e.operations {
        let before = overlay
            .get(&op.key)
            .cloned()
            .unwrap_or_else(|| current(core, &op.key, core.visible));
        if before != op.before {
            return Err(Error::Corrupt("ledger before image"));
        }
        overlay.insert(op.key.clone(), op.after.clone());
    }
    for (key, image) in overlay {
        core.versions
            .entry(key)
            .or_default()
            .push(Version { csn: e.csn, image });
    }
    core.visible = e.csn;
    core.ledger.push(e);
    Ok(())
}
impl Manager {
    /// Open after physical recovery. Complete envelopes are validated before publication.
    pub fn open(log: Box<dyn Log>, limits: Limits) -> Result<Self> {
        Self::open_with_journal(
            log,
            Box::new(vetra_recovery_api::MemoryJournal::default()),
            limits,
        )
    }
    pub fn open_with_journal(
        mut log: Box<dyn Log>,
        journal: Box<dyn vetra_recovery_api::StructuralJournal + Send>,
        limits: Limits,
    ) -> Result<Self> {
        if limits.transactions == 0
            || limits.operations == 0
            || limits.operations > 65535
            || limits.locks == 0
            || limits.pins == 0
            || limits.staged_bytes > 16 * 1024 * 1024
        {
            return Err(Error::Limit);
        }
        log.flush_tip()?;
        let lineage = log.lineage();
        let envelopes = committed(log.records(), lineage)?;
        let next_tx = log
            .records()
            .iter()
            .map(|r| r.tx)
            .max()
            .unwrap_or(0)
            .checked_add(1)
            .ok_or(Error::Limit)?;
        let next_csn = envelopes
            .last()
            .map_or(0, |e| e.csn)
            .checked_add(1)
            .ok_or(Error::Limit)?;
        let mut core = Core {
            journal,
            projection: Projection::new()?,
            log,
            lineage,
            attempts: BTreeMap::new(),
            versions: BTreeMap::new(),
            ledger: Vec::new(),
            locks: LockTable::default(),
            visible: 0,
            serial_frontier: 0,
            next_tx,
            next_csn,
            pins: BTreeMap::new(),
            next_pin: 1,
            expired_before: 0,
            history_bytes: 0,
            poisoned: false,
            limits,
        };
        for e in envelopes {
            core.history_bytes = core
                .history_bytes
                .checked_add(e.encode()?.len())
                .ok_or(Error::Limit)?;
            core.projection
                .apply(&e, &mut vetra_recovery_api::MemoryJournal::default())?;
            apply(&mut core, e)?;
        }
        // Explicit aborted decisions survive; absence is always unknown.
        let mut states = BTreeMap::new();
        for r in core.log.records() {
            if r.tx != 0 {
                match r.kind {
                    Kind::Begin => {
                        states.insert(r.tx, Status::Unknown);
                    }
                    Kind::Abort => {
                        states.insert(r.tx, Status::Aborted);
                    }
                    _ => {}
                }
            }
        }
        for e in &core.ledger {
            states.insert(e.tx, Status::Committed(e.csn));
        }
        for (tx, status) in states {
            core.attempts.insert(
                tx,
                Attempt {
                    isolation: Isolation::ReadCommitted,
                    basis: 0,
                    status,
                    context: Context::object(1, Participant::Row, 1),
                    operations: Vec::new(),
                    bytes: 0,
                    savepoints: Vec::new(),
                    last: 0,
                    serial_start: 0,
                    reads: Vec::new(),
                    deadlock: false,
                    statement_basis: None,
                },
            );
        }
        Ok(Self {
            shared: Arc::new(Shared {
                core: Mutex::new(core),
                changed: Condvar::new(),
            }),
        })
    }
    pub fn open_with_projection(
        log: Box<dyn Log>,
        journal: Box<dyn vetra_recovery_api::StructuralJournal + Send>,
        rows: Option<(vetra_storage::rows::RowStore, u64, u64)>,
        limits: Limits,
    ) -> Result<Self> {
        let manager = Self::open_with_journal(log, journal, limits)?;
        if let Some((rows, row_high, csn_high)) = rows {
            manager.restore_rows(rows, row_high, csn_high)?;
        }
        Ok(manager)
    }
    fn core(&self) -> Result<MutexGuard<'_, Core>> {
        self.shared.core.lock().map_err(|_| Error::UnknownOutcome)
    }
    /// Engine-only restore of validated physical projections and non-reusable identity high waters.
    fn restore_rows(
        &self,
        rows: vetra_storage::rows::RowStore,
        row_high: u64,
        csn_high: u64,
    ) -> Result<()> {
        let mut c = self.core()?;
        rows.validate()?;
        let mut identities = BTreeMap::new();
        for row in rows.all_versions()? {
            identities.insert(
                Key {
                    participant: Participant::Row,
                    object: row.id.table,
                    bytes: row.primary,
                },
                row.id.row,
            );
        }
        c.projection = Projection {
            rows,
            identities,
            next_row: row_high.checked_add(1).ok_or(Error::Limit)?,
        };
        c.next_csn = c.next_csn.max(csn_high.checked_add(1).ok_or(Error::Limit)?);
        Ok(())
    }
    pub fn begin(&self, isolation: Isolation, context: Context) -> Result<Transaction> {
        let mut c = self.core()?;
        if c.poisoned {
            return Err(Error::UnknownOutcome);
        }
        if c.pins.len()
            + c.attempts
                .values()
                .filter(|a| matches!(a.status, Status::Active | Status::Failed))
                .count()
            >= c.limits.pins
        {
            return Err(Error::Limit);
        }
        if c.attempts
            .values()
            .filter(|a| matches!(a.status, Status::Active | Status::Failed))
            .count()
            >= c.limits.transactions
            || context.principal == 0
        {
            return Err(Error::Limit);
        }
        let id = c.next_tx;
        c.next_tx = c.next_tx.checked_add(1).ok_or(Error::Limit)?;
        // Reserve attempt identity durably before exposing the handle.
        let last = match c
            .log
            .append(Kind::Begin, id, 0, (0, 0), Vec::new())
            .and_then(|lsn| c.log.flush(lsn).map(|_| lsn))
        {
            Ok(lsn) => lsn,
            Err(e) => {
                c.poisoned = true;
                return Err(e.into());
            }
        };
        let basis = c.visible;
        let serial_start = c.serial_frontier;
        c.attempts.insert(
            id,
            Attempt {
                isolation,
                basis,
                status: Status::Active,
                context,
                operations: Vec::new(),
                bytes: 0,
                savepoints: Vec::new(),
                last,
                serial_start,
                reads: Vec::new(),
                deadlock: false,
                statement_basis: None,
            },
        );
        Ok(Transaction {
            manager: self.clone(),
            id,
        })
    }
    pub fn outcome(&self, lineage: Lineage, tx: u64) -> Result<Status> {
        let c = self.core()?;
        if c.lineage != lineage {
            return Err(Error::Corrupt("outcome lineage"));
        }
        Ok(c.attempts.get(&tx).map_or(Status::Unknown, |a| a.status))
    }
    pub fn visible(&self) -> Result<u64> {
        Ok(self.core()?.visible)
    }
    pub fn ledger(&self) -> Result<Vec<Envelope>> {
        Ok(self.core()?.ledger.clone())
    }
    pub fn lock_diagnostics(&self) -> Result<locks::Diagnostics> {
        Ok(self.core()?.locks.diagnostics())
    }
    fn lock(&self, id: u64, resource: Resource, mode: Mode, cancel: &AtomicBool) -> Result<()> {
        let mut c = self.core()?;
        let deadline = Instant::now() + c.limits.wait;
        loop {
            let attempt = c.attempts.get(&id).ok_or(Error::State)?;
            if attempt.status == Status::Aborted && attempt.deadlock {
                return Err(Error::Deadlock);
            }
            check_active(attempt)?;
            if cancel.load(Ordering::Relaxed) {
                c.locks.cancel(id);
                c.attempts.get_mut(&id).unwrap().status = Status::Failed;
                return Err(Error::Cancelled);
            }
            let limit = c.limits.locks;
            if c.locks.acquire(id, resource.clone(), mode, limit)? {
                return Ok(());
            }
            if let Some(victim) = c.locks.victim() {
                // Aborting the victim clears the entire staging area and all ownership.
                Self::abort_core(&mut c, victim)?;
                c.attempts.get_mut(&victim).unwrap().deadlock = true;
                self.shared.changed.notify_all();
                if victim == id {
                    return Err(Error::Deadlock);
                }
                continue;
            }
            let now = Instant::now();
            if now >= deadline {
                c.locks.cancel(id);
                c.attempts.get_mut(&id).unwrap().status = Status::Failed;
                return Err(Error::Timeout);
            }
            let wait = (deadline - now).min(Duration::from_millis(10));
            let (g, _) = self
                .shared
                .changed
                .wait_timeout(c, wait)
                .map_err(|_| Error::UnknownOutcome)?;
            c = g;
        }
    }
    fn abort_core(c: &mut Core, id: u64) -> Result<()> {
        let a = c.attempts.get_mut(&id).ok_or(Error::State)?;
        if matches!(a.status, Status::Committed(_) | Status::Aborted) {
            return Ok(());
        }
        let last = a.last;
        a.operations.clear();
        a.savepoints.clear();
        a.status = Status::Aborted;
        c.locks.release(id);
        if c.poisoned {
            a.status = Status::Unknown;
            return Err(Error::UnknownOutcome);
        }
        let result = c
            .log
            .append(Kind::Abort, id, last, (0, 0), vec![])
            .and_then(|lsn| c.log.append(Kind::End, id, lsn, (0, 0), vec![]))
            .and_then(|lsn| c.log.flush(lsn));
        if let Err(e) = result {
            c.poisoned = true;
            c.attempts.get_mut(&id).unwrap().status = Status::Unknown;
            return Err(e.into());
        }
        Ok(())
    }
    /// Explicit bounded group commit; one durability barrier, one atomic publication.
    pub fn commit_group(&self, ids: &[u64], timestamp: i64) -> Result<Vec<u64>> {
        let mut c = self.core()?;
        if c.poisoned {
            return Err(Error::UnknownOutcome);
        }
        if ids.is_empty() || ids.len() > 256 {
            return Err(Error::Limit);
        }
        let unique: std::collections::BTreeSet<_> = ids.iter().copied().collect();
        if unique.len() != ids.len() {
            return Err(Error::State);
        }
        let mut prepared = Vec::new();
        let mut size = 0;
        for &id in ids {
            let a = c.attempts.get(&id).ok_or(Error::State)?;
            check_active(a)?;
            if a.operations.iter().any(|op| {
                latest(&c, &op.key) > a.basis
                    && matches!(
                        a.isolation,
                        Isolation::RepeatableRead | Isolation::Serializable
                    )
            }) {
                c.attempts.get_mut(&id).unwrap().status = Status::Failed;
                return Err(Error::Serialization);
            }
            if a.isolation != Isolation::Serializable
                && c.serial_frontier > a.serial_start
                && a.reads
                    .iter()
                    .any(|(resource, basis)| changed_since(&c, resource, *basis))
            {
                c.attempts.get_mut(&id).unwrap().status = Status::Failed;
                return Err(Error::Serialization);
            }
            let csn = c.next_csn;
            c.next_csn = c.next_csn.checked_add(1).ok_or(Error::Limit)?;
            let a = &c.attempts[&id];
            let e = Envelope {
                lineage: c.lineage,
                tx: id,
                csn,
                timestamp,
                principal: a.context.principal,
                metadata: a.context.metadata.clone(),
                operations: a.operations.clone(),
            };
            let bytes = match e.encode() {
                Ok(bytes) => bytes,
                Err(error) => {
                    c.attempts.get_mut(&id).unwrap().status = Status::Failed;
                    return Err(error.into());
                }
            };
            size += bytes.len();
            prepared.push((e, bytes));
        }
        if size > 16 * 1024 * 1024
            || c.history_bytes
                .checked_add(size)
                .is_none_or(|n| n > c.limits.history_bytes)
        {
            for id in ids {
                c.attempts.get_mut(id).unwrap().status = Status::Failed;
            }
            return Err(Error::Limit);
        }
        let mut overlay = BTreeMap::new();
        for (e, _) in &prepared {
            for op in &e.operations {
                let before = overlay
                    .get(&op.key)
                    .cloned()
                    .unwrap_or_else(|| current(&c, &op.key, c.visible));
                if before != op.before {
                    c.attempts.get_mut(&e.tx).unwrap().status = Status::Failed;
                    return Err(Error::Corrupt("stale prepared operation"));
                }
                overlay.insert(op.key.clone(), op.after.clone());
            }
        }
        // Prepare real version and primary B+Trees before writing COMMIT.
        let mut projection = c.projection.staged_copy();
        let mut staged = PreparedJournal {
            inner: vetra_recovery_api::MemoryJournal::default(),
            bytes: 0,
            limit: c.limits.staged_bytes,
        };
        for (e, _) in &prepared {
            if let Err(error) = projection.apply(e, &mut staged) {
                c.attempts.get_mut(&e.tx).unwrap().status = Status::Failed;
                return Err(error);
            }
        }
        // Completed COW batches enter the same WAL before the final logical COMMIT.
        for (_, actions) in staged.inner.batches {
            if let Err(error) = c.journal.seal_batch(actions) {
                c.poisoned = true;
                for id in ids {
                    c.attempts.get_mut(id).unwrap().status = Status::Unknown;
                    c.locks.release(*id);
                }
                self.shared.changed.notify_all();
                return Err(Error::Wal(vetra_wal::Error::Io(error)));
            }
        }
        let result = (|| -> std::result::Result<u64, vetra_wal::Error> {
            let mut last = 0;
            for (e, bytes) in &prepared {
                let count = bytes.len().div_ceil(CHUNK_BYTES);
                let mut prev = c.attempts[&e.tx].last;
                let mut first = 0;
                for (i, chunk) in bytes.chunks(CHUNK_BYTES).enumerate() {
                    let mut p = Vec::new();
                    for n in [i, count, bytes.len(), chunk.len()] {
                        p.extend((n as u32).to_le_bytes());
                    }
                    p.extend(chunk);
                    prev = c.log.append(Kind::Envelope, e.tx, prev, (0, 0), p)?;
                    if i == 0 {
                        first = prev;
                    }
                }
                let mut p = e.csn.to_le_bytes().to_vec();
                p.extend((bytes.len() as u32).to_le_bytes());
                p.extend(crc32c(bytes).to_le_bytes());
                p.extend(first.to_le_bytes());
                last = c.log.append(Kind::Commit, e.tx, prev, (0, 0), p)?;
                c.attempts.get_mut(&e.tx).unwrap().last = last;
            }
            c.log.flush(last)?;
            Ok(last)
        })();
        if let Err(e) = result {
            c.poisoned = true;
            for id in ids {
                c.attempts.get_mut(id).unwrap().status = Status::Unknown;
                c.locks.release(*id);
            }
            self.shared.changed.notify_all();
            return Err(e.into());
        }
        c.projection = projection;
        let mut csns = Vec::new();
        for (e, _) in prepared {
            let id = e.tx;
            let csn = e.csn;
            if let Err(e) = apply(&mut c, e) {
                c.poisoned = true;
                return Err(e);
            }
            if c.attempts[&id].isolation == Isolation::Serializable {
                c.serial_frontier = csn;
            }
            let a = c.attempts.get_mut(&id).unwrap();
            a.status = Status::Committed(csn);
            a.operations.clear();
            a.savepoints.clear();
            c.locks.release(id);
            csns.push(csn);
        }
        c.history_bytes += size;
        self.shared.changed.notify_all();
        Ok(csns)
    }
    /// Fuzzy checkpoint of conservative dirty and active tables. All WAL remains retained.
    pub fn checkpoint(&self) -> Result<u64> {
        use vetra_wal::codec::{CHUNK_BYTES, crc32c, u64at};
        let mut c = self.core()?;
        if c.poisoned {
            return Err(Error::UnknownOutcome);
        }
        let id = c
            .log
            .records()
            .iter()
            .filter(|r| r.kind == Kind::CheckpointBegin)
            .map(|r| u64at(&r.payload, 0))
            .max()
            .unwrap_or(0)
            .checked_add(1)
            .ok_or(Error::Limit)?;
        let mut dirty = BTreeMap::new();
        for r in c
            .log
            .records()
            .iter()
            .filter(|r| r.kind == Kind::FullPage && r.page.0 < 1_000_002)
        {
            dirty.entry(r.page).or_insert(r.lsn);
        }
        let active: Vec<_> = c
            .attempts
            .iter()
            .filter(|(_, a)| matches!(a.status, Status::Active | Status::Failed))
            .map(|(&id, a)| (id, a.last))
            .collect();
        let mut table = Vec::new();
        for count in [dirty.len(), active.len(), 0, 0] {
            table.extend((count as u32).to_le_bytes());
        }
        for ((id, g), lsn) in dirty {
            for n in [id, g, lsn] {
                table.extend(n.to_le_bytes());
            }
        }
        for (tx, last) in active {
            for n in [tx, last, 0, 0] {
                table.extend(n.to_le_bytes());
            }
            table.push(1);
            table.extend([0; 7]);
        }
        if table.len() > 16 * 1024 * 1024 {
            return Err(Error::Limit);
        }
        let result = (|| -> std::result::Result<u64, vetra_wal::Error> {
            let begin = c.log.append(
                Kind::CheckpointBegin,
                0,
                0,
                (0, 0),
                id.to_le_bytes().to_vec(),
            )?;
            let count = table.len().div_ceil(CHUNK_BYTES);
            for (i, chunk) in table.chunks(CHUNK_BYTES).enumerate() {
                let mut p = id.to_le_bytes().to_vec();
                for n in [i, count, chunk.len(), 0] {
                    p.extend((n as u32).to_le_bytes());
                }
                p.extend(chunk);
                c.log.append(Kind::CheckpointChunk, 0, 0, (0, 0), p)?;
            }
            let mut p = id.to_le_bytes().to_vec();
            p.extend(begin.to_le_bytes());
            p.extend((table.len() as u32).to_le_bytes());
            p.extend(crc32c(&table).to_le_bytes());
            let end = c.log.append(Kind::CheckpointEnd, 0, 0, (0, 0), p)?;
            c.log.flush(end)?;
            Ok(end)
        })();
        match result {
            Ok(lsn) => Ok(lsn),
            Err(error) => {
                c.poisoned = true;
                Err(error.into())
            }
        }
    }
    pub fn snapshot(&self, owner: u64, kind: PinKind, expires: Option<u64>) -> Result<Snapshot> {
        let mut c = self.core()?;
        if c.pins.len()
            + c.attempts
                .values()
                .filter(|a| matches!(a.status, Status::Active | Status::Failed))
                .count()
            >= c.limits.pins
            || owner == 0
        {
            return Err(Error::Limit);
        }
        let id = c.next_pin;
        c.next_pin = c.next_pin.checked_add(1).ok_or(Error::Limit)?;
        let basis = c.visible;
        c.pins.insert(
            id,
            PinInfo {
                id,
                basis,
                kind,
                owner,
                expires,
            },
        );
        Ok(Snapshot {
            manager: self.clone(),
            id,
            basis,
        })
    }
    pub fn pins(&self) -> Result<Vec<PinInfo>> {
        let c = self.core()?;
        let mut pins: Vec<_> = c.pins.values().cloned().collect();
        pins.extend(
            c.attempts
                .iter()
                .filter(|(_, a)| matches!(a.status, Status::Active | Status::Failed))
                .map(|(&id, a)| PinInfo {
                    id,
                    basis: a.basis,
                    kind: PinKind::Reader,
                    owner: a.context.principal,
                    expires: None,
                }),
        );
        Ok(pins)
    }
    /// Reclaim serving versions only. The immutable ledger is never pruned.
    pub fn vacuum(&self, now: u64) -> Result<usize> {
        let mut c = self.core()?;
        c.pins.retain(|_, p| p.expires.is_none_or(|end| end > now));
        let horizon = c
            .pins
            .values()
            .map(|p| p.basis)
            .chain(
                c.attempts
                    .values()
                    .filter(|a| matches!(a.status, Status::Active | Status::Failed))
                    .map(|a| a.basis),
            )
            .min()
            .unwrap_or(c.visible);
        let mut removed = 0;
        let mut serving_versions = c.versions.clone();
        for versions in serving_versions.values_mut() {
            let keep = versions.iter().rposition(|v| v.csn <= horizon).unwrap_or(0);
            removed += keep;
            versions.drain(..keep);
        }
        let source = c.projection.staged_copy();
        let projection = source.compact(&serving_versions, c.journal.as_mut())?;
        // Maintenance top actions are independently durable before replacing serving roots.
        if let Err(error) = c.log.flush_tip() {
            c.poisoned = true;
            return Err(error.into());
        }
        c.versions = serving_versions;
        c.projection = projection;
        c.expired_before = c.expired_before.max(horizon);
        Ok(removed)
    }
}
impl Transaction {
    fn authorize(&self, key: &Key, write: bool) -> Result<()> {
        key.validate()?;
        let c = self.manager.core()?;
        let a = c.attempts.get(&self.id).ok_or(Error::State)?;
        check_active(a)?;
        let grants = if write {
            &a.context.writable
        } else {
            &a.context.readable
        };
        if !grants.contains(&(key.participant, key.object)) {
            drop(c);
            self.statement_error()?;
            return Err(Error::Unauthorized);
        }
        Ok(())
    }
    /// Freeze one RC statement basis across all operator reads; adapters must end it.
    pub fn begin_statement(&self) -> Result<u64> {
        let mut c = self.manager.core()?;
        let visible = c.visible;
        let a = c.attempts.get_mut(&self.id).ok_or(Error::State)?;
        check_active(a)?;
        if a.statement_basis.is_some() {
            return Err(Error::State);
        }
        if matches!(
            a.isolation,
            Isolation::ReadCommitted | Isolation::ReadUncommitted
        ) {
            a.basis = visible;
        }
        a.statement_basis = Some(a.basis);
        Ok(a.basis)
    }
    pub fn end_statement(&self) -> Result<()> {
        self.manager
            .core()?
            .attempts
            .get_mut(&self.id)
            .ok_or(Error::State)?
            .statement_basis = None;
        Ok(())
    }
    /// Adapter-owned predicate lock, subject to the same grants and wait/cancel policy.
    pub fn acquire(
        &self,
        resource: locks::Resource,
        mode: locks::Mode,
        cancel: &AtomicBool,
    ) -> Result<()> {
        let probe = Key {
            participant: resource.participant,
            object: resource.object,
            bytes: resource.lower.clone().unwrap_or_else(|| vec![0]),
        };
        self.authorize(&probe, mode == locks::Mode::Exclusive)?;
        self.finish_statement(self.manager.lock(self.id, resource, mode, cancel))
    }
    pub fn statement_error(&self) -> Result<()> {
        let mut c = self.manager.core()?;
        let a = c.attempts.get_mut(&self.id).ok_or(Error::State)?;
        check_active(a)?;
        a.status = Status::Failed;
        c.locks.cancel(self.id);
        self.manager.shared.changed.notify_all();
        Ok(())
    }
    /// Each call is one native statement. Scans capture one basis for every returned row.
    pub fn read(&self, key: &Key) -> Result<Option<Image>> {
        self.finish_statement(self.read_inner(key))
    }
    fn finish_statement<T>(&self, result: Result<T>) -> Result<T> {
        if result.is_err() && self.status() == Ok(Status::Active) {
            self.statement_error()?;
        }
        result
    }
    fn read_inner(&self, key: &Key) -> Result<Option<Image>> {
        self.authorize(key, false)?;
        let serial = self.manager.core()?.attempts[&self.id].isolation == Isolation::Serializable;
        if serial {
            self.manager.lock(
                self.id,
                Resource::key(key),
                Mode::Shared,
                &AtomicBool::new(false),
            )?;
        }
        let mut c = self.manager.core()?;
        let visible = c.visible;
        let read_limit = c.limits.operations;
        let a = c.attempts.get_mut(&self.id).unwrap();
        check_active(a)?;
        if matches!(
            a.isolation,
            Isolation::ReadCommitted | Isolation::ReadUncommitted
        ) {
            a.basis = a.statement_basis.unwrap_or(visible);
        }
        let basis = a.basis;
        if a.reads.len() >= read_limit {
            a.status = Status::Failed;
            return Err(Error::Limit);
        }
        a.reads.push((Resource::key(key), basis));
        if let Some(op) = a.operations.iter().rev().find(|op| &op.key == key) {
            return Ok(op.after.clone());
        }
        if serial && latest(&c, key) > basis {
            c.attempts.get_mut(&self.id).unwrap().status = Status::Failed;
            return Err(Error::Serialization);
        }
        projected(&c, key, basis)
    }
    pub fn scan(
        &self,
        participant: Participant,
        object: u64,
        lower: Option<Vec<u8>>,
        upper: Option<Vec<u8>>,
    ) -> Result<Vec<(Key, Image)>> {
        self.finish_statement(self.scan_inner(participant, object, lower, upper))
    }
    fn scan_inner(
        &self,
        participant: Participant,
        object: u64,
        lower: Option<Vec<u8>>,
        upper: Option<Vec<u8>>,
    ) -> Result<Vec<(Key, Image)>> {
        let probe = Key {
            participant,
            object,
            bytes: vec![0],
        };
        self.authorize(&probe, false)?;
        let resource = Resource {
            participant,
            object,
            lower,
            upper,
        };
        let serial = self.manager.core()?.attempts[&self.id].isolation == Isolation::Serializable;
        if serial {
            self.manager.lock(
                self.id,
                resource.clone(),
                Mode::Shared,
                &AtomicBool::new(false),
            )?;
        }
        let mut c = self.manager.core()?;
        let visible = c.visible;
        let read_limit = c.limits.operations;
        let a = c.attempts.get_mut(&self.id).unwrap();
        check_active(a)?;
        if matches!(
            a.isolation,
            Isolation::ReadCommitted | Isolation::ReadUncommitted
        ) {
            a.basis = a.statement_basis.unwrap_or(visible);
        }
        let basis = a.basis;
        if a.reads.len() >= read_limit {
            a.status = Status::Failed;
            return Err(Error::Limit);
        }
        a.reads.push((resource.clone(), basis));
        let ops = a.operations.clone();
        let inside = |key: &Key| {
            key.participant == participant
                && key.object == object
                && resource.lower.as_ref().is_none_or(|b| &key.bytes >= b)
                && resource.upper.as_ref().is_none_or(|b| &key.bytes <= b)
        };
        let mut result = BTreeMap::new();
        for key in c.versions.keys().filter(|k| inside(k)) {
            if serial && latest(&c, key) > basis {
                drop(c);
                self.statement_error()?;
                return Err(Error::Serialization);
            }
            if let Some(image) = current(&c, key, basis) {
                result.insert(key.clone(), image);
            }
        }
        for op in ops.into_iter().filter(|op| inside(&op.key)) {
            if let Some(image) = op.after {
                result.insert(op.key, image);
            } else {
                result.remove(&op.key);
            }
        }
        Ok(result.into_iter().collect())
    }
    pub fn put(&self, key: Key, schema: u64, image: Image) -> Result<()> {
        self.write(key, schema, Some(image), false, &AtomicBool::new(false))
    }
    pub fn insert(&self, key: Key, schema: u64, image: Image) -> Result<()> {
        self.write(key, schema, Some(image), true, &AtomicBool::new(false))
    }
    pub fn delete(&self, key: Key, schema: u64) -> Result<()> {
        self.write(key, schema, None, false, &AtomicBool::new(false))
    }
    pub fn write(
        &self,
        key: Key,
        schema: u64,
        after: Option<Image>,
        insert_only: bool,
        cancel: &AtomicBool,
    ) -> Result<()> {
        self.finish_statement(self.write_inner(key, schema, after, insert_only, cancel))
    }
    fn write_inner(
        &self,
        key: Key,
        schema: u64,
        after: Option<Image>,
        insert_only: bool,
        cancel: &AtomicBool,
    ) -> Result<()> {
        self.authorize(&key, true)?;
        if matches!(key.participant, Participant::Row | Participant::Schema) != (schema != 0) {
            self.statement_error()?;
            return Err(Error::State);
        }
        let image_bytes = after
            .as_ref()
            .map(encode_image)
            .transpose()?
            .map_or(0, |b| b.len());
        self.manager
            .lock(self.id, Resource::key(&key), Mode::Exclusive, cancel)?;
        let mut c = self.manager.core()?;
        let a = &c.attempts[&self.id];
        check_active(a)?;
        if matches!(
            a.isolation,
            Isolation::RepeatableRead | Isolation::Serializable
        ) && latest(&c, &key) > a.basis
        {
            c.attempts.get_mut(&self.id).unwrap().status = Status::Failed;
            return Err(Error::Serialization);
        }
        let before = a
            .operations
            .iter()
            .rev()
            .find(|op| op.key == key)
            .map(|op| op.after.clone())
            .unwrap_or_else(|| current(&c, &key, c.visible));
        if insert_only && before.is_some() {
            c.attempts.get_mut(&self.id).unwrap().status = Status::Failed;
            return Err(Error::Storage(vetra_storage::Error::Duplicate));
        }
        if after.is_none() && before.is_none() {
            return Ok(());
        }
        let bytes = 32
            + key.bytes.len()
            + 8
            + image_bytes
            + before
                .as_ref()
                .map(encode_image)
                .transpose()?
                .map_or(0, |b| b.len());
        let limits = c.limits.clone();
        let a = c.attempts.get_mut(&self.id).unwrap();
        if a.operations.len() >= limits.operations
            || a.bytes
                .checked_add(bytes)
                .is_none_or(|n| n > limits.staged_bytes)
        {
            a.status = Status::Failed;
            return Err(Error::Limit);
        }
        a.bytes += bytes;
        a.operations.push(Operation {
            key,
            schema,
            before,
            after,
        });
        Ok(())
    }
    pub fn savepoint(&self, name: &str) -> Result<()> {
        let mut c = self.manager.core()?;
        let a = c.attempts.get_mut(&self.id).ok_or(Error::State)?;
        check_active(a)?;
        if name.is_empty() || name.len() > 128 || a.savepoints.len() >= 64 {
            a.status = Status::Failed;
            return Err(Error::Limit);
        }
        a.savepoints.push(Savepoint {
            name: name.to_owned(),
            len: a.operations.len(),
            bytes: a.bytes,
        });
        Ok(())
    }
    pub fn rollback_to(&self, name: &str) -> Result<()> {
        let mut c = self.manager.core()?;
        let a = c.attempts.get_mut(&self.id).ok_or(Error::State)?;
        if !matches!(a.status, Status::Active | Status::Failed) {
            return Err(Error::State);
        }
        let Some(i) = a.savepoints.iter().rposition(|s| s.name == name) else {
            a.status = Status::Failed;
            return Err(Error::State);
        };
        let s = a.savepoints[i].clone();
        a.operations.truncate(s.len);
        a.bytes = s.bytes;
        a.savepoints.truncate(i + 1);
        a.status = Status::Active;
        Ok(())
    }
    pub fn release(&self, name: &str) -> Result<()> {
        let mut c = self.manager.core()?;
        let a = c.attempts.get_mut(&self.id).ok_or(Error::State)?;
        check_active(a)?;
        let Some(i) = a.savepoints.iter().rposition(|s| s.name == name) else {
            a.status = Status::Failed;
            return Err(Error::State);
        };
        a.savepoints.truncate(i);
        Ok(())
    }
    pub fn commit(&self, timestamp: i64) -> Result<u64> {
        Ok(self.manager.commit_group(&[self.id], timestamp)?[0])
    }
    pub fn rollback(&self) -> Result<()> {
        let mut c = self.manager.core()?;
        let r = Manager::abort_core(&mut c, self.id);
        self.manager.shared.changed.notify_all();
        r
    }
    pub fn status(&self) -> Result<Status> {
        Ok(self
            .manager
            .core()?
            .attempts
            .get(&self.id)
            .ok_or(Error::State)?
            .status)
    }
}
impl Drop for Transaction {
    fn drop(&mut self) {
        if matches!(self.status(), Ok(Status::Active | Status::Failed)) {
            let _ = self.rollback();
        }
    }
}
impl Snapshot {
    pub fn basis(&self) -> u64 {
        self.basis
    }
    pub fn read(&self, key: &Key) -> Result<Option<Image>> {
        let c = self.manager.core()?;
        if !c.pins.contains_key(&self.id) || self.basis < c.expired_before {
            return Err(Error::Expired);
        }
        projected(&c, key, self.basis)
    }
}
impl Drop for Snapshot {
    fn drop(&mut self) {
        if let Ok(mut c) = self.manager.core() {
            c.pins.remove(&self.id);
        }
    }
}
