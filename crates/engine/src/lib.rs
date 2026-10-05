//! Explicit native database lifecycle over the shared durable transaction core.
pub use vetra_txn::{
    Context, Error, Image, Isolation, Key, Limits, Manager, Participant, PinKind, Snapshot, Status,
    Transaction, Value,
};
pub use vetra_wal::{Lineage, Log};
fn recovery_error(error: vetra_recovery::Error) -> Error {
    match error {
        vetra_recovery::Error::Wal(e) => Error::Wal(e),
        vetra_recovery::Error::Storage(e) => Error::Storage(e),
        vetra_recovery::Error::Corrupt(reason) => Error::Corrupt(reason),
        vetra_recovery::Error::Limit => Error::Limit,
    }
}
/// Native API facade. PostgreSQL/SQL, scheduler and network dispatch remain later milestones.
#[derive(Clone)]
pub struct Database {
    transactions: Manager,
}
impl Database {
    pub fn from_log(mut log: Box<dyn Log>, limits: Limits) -> vetra_txn::Result<Self> {
        vetra_recovery::recover(log.as_mut(), &mut vetra_recovery::MemoryPages::default())
            .map_err(recovery_error)?;
        let mut rows =
            vetra_storage::rows::RowStore::new(2, 3, vetra_storage::tree::TreeConfig::default())?;
        let mut physical = false;
        for (lsn, batch) in vetra_recovery::journal::actions(log.records())
            .map_err(|_| Error::Corrupt("structural root/allocator history"))?
        {
            if batch.len() == 2 && batch[0].tree == 2 && batch[1].tree == 3 {
                rows.replay_batch(lsn, &batch)?;
                physical = true;
            }
        }
        let mut row_high = 0;
        let mut csn_high = 0;
        for r in log.records().iter().filter(|r| {
            r.kind == vetra_wal::Kind::FullPage
                && r.page.0 < vetra_recovery::journal::MANIFEST_START
        }) {
            let a = vetra_storage::codec::Address {
                id: r.page.0,
                generation: r.page.1,
            };
            if let Ok(page) = vetra_storage::codec::Page::decode(&r.payload, a, 2) {
                if let vetra_storage::codec::Body::Tree {
                    level: 0, records, ..
                } = page.body
                {
                    for record in records {
                        if record.key.len() == 24 {
                            row_high = row_high
                                .max(u64::from_be_bytes(record.key[8..16].try_into().unwrap()));
                            csn_high = csn_high
                                .max(u64::from_be_bytes(record.key[16..24].try_into().unwrap()));
                        }
                    }
                }
            }
        }
        let shared = SharedLog::new(log);
        let journal = vetra_recovery::journal::Journal::new(shared.clone(), false)
            .map_err(|_| Error::Corrupt("structural journal initialization"))?;
        let manager = Manager::open_with_projection(
            Box::new(shared),
            Box::new(journal),
            physical.then_some((rows, row_high, csn_high)),
            limits,
        )?;
        Ok(Self {
            transactions: manager,
        })
    }
    #[cfg(unix)]
    pub fn open(
        path: impl AsRef<std::path::Path>,
        lineage: Lineage,
        limits: Limits,
    ) -> vetra_txn::Result<Self> {
        use vetra_io::{DirectoryIo, FileIo};
        let mut directory = vetra_io::LocalDirectory::new(path).map_err(vetra_wal::Error::from)?;
        directory
            .acquire_exclusive()
            .map_err(vetra_wal::Error::from)?;
        let names = directory.files().map_err(vetra_wal::Error::from)?;
        if names.iter().any(|name| name == "data.v1")
            && !names.iter().any(|name| name.ends_with(".wal"))
        {
            return Err(Error::Corrupt("data directory is missing required WAL"));
        }
        let file = directory.open("data.v1").map_err(vetra_wal::Error::from)?;
        let (existing, fresh) = if file.is_empty().map_err(vetra_wal::Error::from)? {
            (None, Some(file))
        } else {
            (
                Some(DataPages::open(file, lineage).map_err(recovery_error)?),
                None,
            )
        };
        let mut log = vetra_wal::Wal::open(directory, lineage)?;
        let mut pages = match existing {
            Some(pages) => pages,
            None => DataPages::open(fresh.unwrap(), lineage).map_err(recovery_error)?,
        };
        // Complete loser chains before exposing transaction handles.
        vetra_recovery::recover(&mut log, &mut pages).map_err(recovery_error)?;
        pages
            .sync()
            .map_err(|_| Error::Corrupt("recovered data sync"))?;
        Self::from_log(Box::new(log), limits)
    }
    pub fn begin(&self, isolation: Isolation, context: Context) -> vetra_txn::Result<Transaction> {
        self.transactions.begin(isolation, context)
    }
    pub fn transactions(&self) -> &Manager {
        &self.transactions
    }
}

/// One WAL shared by the transaction and structural capabilities. No participant callbacks.
#[derive(Clone)]
struct SharedLog {
    inner: std::sync::Arc<std::sync::Mutex<Box<dyn Log>>>,
    records: Vec<vetra_wal::Record>,
    lineage: Lineage,
}
impl SharedLog {
    fn new(log: Box<dyn Log>) -> Self {
        let lineage = log.lineage();
        let records = log.records().to_vec();
        Self {
            inner: std::sync::Arc::new(std::sync::Mutex::new(log)),
            records,
            lineage,
        }
    }
}
impl Log for SharedLog {
    fn append(
        &mut self,
        kind: vetra_wal::Kind,
        tx: u64,
        prev: u64,
        page: (u64, u64),
        payload: Vec<u8>,
    ) -> vetra_wal::Result<u64> {
        let mut log = self.inner.lock().map_err(|_| vetra_wal::Error::Poisoned)?;
        let lsn = log.append(kind, tx, prev, page, payload)?;
        self.records
            .extend_from_slice(&log.records()[self.records.len()..]);
        Ok(lsn)
    }
    fn flush(&mut self, lsn: u64) -> vetra_wal::Result<u64> {
        let mut log = self.inner.lock().map_err(|_| vetra_wal::Error::Poisoned)?;
        let durable = log.flush(lsn)?;
        self.records
            .extend_from_slice(&log.records()[self.records.len()..]);
        Ok(durable)
    }
    fn flush_tip(&mut self) -> vetra_wal::Result<u64> {
        let mut log = self.inner.lock().map_err(|_| vetra_wal::Error::Poisoned)?;
        let lsn = log.records().last().map_or(0, |r| r.lsn);
        let durable = log.flush(lsn)?;
        self.records
            .extend_from_slice(&log.records()[self.records.len()..]);
        Ok(durable)
    }
    fn records(&self) -> &[vetra_wal::Record] {
        &self.records
    }
    fn lineage(&self) -> Lineage {
        self.lineage
    }
    fn durable(&self) -> u64 {
        self.inner.lock().map_or(0, |log| log.durable())
    }
}

/// Checked positioned data-page provider for physiological recovery callers.
/// Page file creation additionally requires the caller's directory durability barrier.
pub struct DataPages<F: vetra_io::FileIo> {
    file: F,
}
impl<F: vetra_io::FileIo> DataPages<F> {
    pub fn open(file: F, lineage: Lineage) -> vetra_recovery::Result<Self> {
        use vetra_storage::{
            codec::{Address, Superblock},
            pager::Pager,
        };
        let file = if file.is_empty().map_err(vetra_storage::Error::from)? {
            Pager::bootstrap(
                file,
                Superblock {
                    database: lineage.database,
                    timeline: lineage.timeline,
                    generation: 1,
                    checkpoint: 0,
                    wal_start: 0,
                    catalog: Address::default(),
                    allocator: Address::default(),
                    identity_high_water: 0,
                },
            )?
            .into_file()
        } else {
            let mut a = [0; 8192];
            let mut b = [0; 8192];
            vetra_io::read_exact_at(&file, 0, &mut a).map_err(vetra_storage::Error::from)?;
            vetra_io::read_exact_at(&file, 8192, &mut b).map_err(vetra_storage::Error::from)?;
            let superblock = Superblock::select(&a, &b, 0)?;
            if superblock.database != lineage.database || superblock.timeline != lineage.timeline {
                return Err(vetra_recovery::Error::Corrupt("data lineage"));
            }
            file
        };
        Ok(Self { file })
    }
    pub fn sync(&mut self) -> vetra_recovery::Result<()> {
        if self.file.len().map_err(vetra_storage::Error::from)? % 8192 != 0 {
            return Err(vetra_recovery::Error::Corrupt(
                "unrepairable partial data page",
            ));
        }
        loop {
            match self.file.sync_data() {
                Err(vetra_types::IoFailure::Interrupted) => continue,
                r => return r.map_err(vetra_storage::Error::from).map_err(Into::into),
            }
        }
    }
    pub fn into_file(self) -> F {
        self.file
    }
}
impl<F: vetra_io::FileIo> vetra_recovery::Pages for DataPages<F> {
    fn load(
        &mut self,
        address: vetra_storage::codec::Address,
    ) -> vetra_recovery::Result<Option<vetra_storage::codec::Image>> {
        use vetra_storage::codec::PAGE_SIZE;
        let offset = address
            .id
            .checked_mul(PAGE_SIZE as u64)
            .ok_or(vetra_recovery::Error::Limit)?;
        let len = self.file.len().map_err(vetra_storage::Error::from)?;
        if offset >= len {
            return Ok(None);
        }
        let mut b = [0; PAGE_SIZE];
        let count = (len - offset).min(PAGE_SIZE as u64) as usize;
        vetra_io::read_exact_at(&self.file, offset, &mut b[..count])
            .map_err(vetra_storage::Error::from)?;
        Ok(if b == [0; PAGE_SIZE] { None } else { Some(b) })
    }
    fn install(
        &mut self,
        address: vetra_storage::codec::Address,
        image: vetra_storage::codec::Image,
    ) -> vetra_recovery::Result<()> {
        use vetra_storage::codec::{PAGE_SIZE, Page};
        let owner = vetra_wal::codec::u64at(&image, 32);
        Page::decode(&image, address, owner)?;
        let offset = address
            .id
            .checked_mul(PAGE_SIZE as u64)
            .ok_or(vetra_recovery::Error::Limit)?;
        vetra_io::write_all_at(&mut self.file, offset, &image)
            .map_err(vetra_storage::Error::from)?;
        Ok(())
    }
}

impl<F: vetra_io::FileIo + Send> vetra_storage::buffer::PageIo for DataPages<F> {
    fn load(
        &mut self,
        address: vetra_storage::codec::Address,
    ) -> vetra_storage::Result<vetra_storage::codec::Image> {
        vetra_recovery::Pages::load(self, address)
            .map_err(|e| match e {
                vetra_recovery::Error::Storage(e) => e,
                _ => vetra_storage::Error::Corrupt("recovery page load"),
            })?
            .ok_or(vetra_storage::Error::Stale)
    }
    fn write(
        &mut self,
        address: vetra_storage::codec::Address,
        image: &vetra_storage::codec::Image,
    ) -> vetra_storage::Result<()> {
        vetra_recovery::Pages::install(self, address, *image).map_err(|e| match e {
            vetra_recovery::Error::Storage(e) => e,
            _ => vetra_storage::Error::Corrupt("recovery page write"),
        })
    }
}

pub mod sql;
