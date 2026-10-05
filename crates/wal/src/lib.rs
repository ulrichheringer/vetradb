//! Segmented format-v1 WAL. Mutation failures poison the writer until reopen.
pub mod codec;
pub mod envelope;
/// Experimental resident-log budget; recycling/streamed recovery are subsequent optimizations.
pub const MAX_WAL_BYTES: u64 = 512 * 1024 * 1024;
pub const MUTATION_WAL_BYTES: u64 = 256 * 1024 * 1024;
pub use codec::{Kind, Lineage, Record};
use codec::{MAX_RECORD, SEGMENT_SIZE, check_segment, segment_header, u32at};
use std::collections::BTreeMap;
use vetra_io::{DirectoryIo, FileIo, read_exact_at, write_all_at};
use vetra_recovery_api::WalBarrier;
use vetra_types::IoFailure;
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Error {
    Io(IoFailure),
    Corrupt(&'static str),
    Limit,
    Poisoned,
}
impl From<IoFailure> for Error {
    fn from(e: IoFailure) -> Self {
        Self::Io(e)
    }
}
pub type Result<T> = std::result::Result<T, Error>;
/// WAL storage capability hides concrete files from the transaction layer.
pub trait Log: Send {
    fn append(
        &mut self,
        kind: Kind,
        tx: u64,
        prev: u64,
        page: (u64, u64),
        payload: Vec<u8>,
    ) -> Result<u64>;
    fn flush(&mut self, lsn: u64) -> Result<u64>;
    fn records(&self) -> &[Record];
    fn lineage(&self) -> Lineage;
    fn durable(&self) -> u64;
    fn flush_tip(&mut self) -> Result<u64> {
        let lsn = self.records().last().map_or(0, |r| r.lsn);
        self.flush(lsn)
    }
}
pub struct Wal<D: DirectoryIo> {
    directory: D,
    files: BTreeMap<u64, D::File>,
    lineage: Lineage,
    records: Vec<Record>,
    segment: u64,
    offset: u64,
    durable: u64,
    poisoned: bool,
}
fn sync(file: &mut impl FileIo, all: bool) -> Result<()> {
    loop {
        let r = if all {
            file.sync_all()
        } else {
            file.sync_data()
        };
        match r {
            Err(IoFailure::Interrupted) => continue,
            r => return r.map_err(Into::into),
        }
    }
}
impl<D: DirectoryIo> Wal<D> {
    /// Caller owns the directory exclusively before opening. No missing segment is created during discovery.
    pub fn open(directory: D, lineage: Lineage) -> Result<Self> {
        lineage.validate()?;
        let names = directory.files()?;
        let mut numbers = Vec::new();
        for name in names {
            if name.ends_with(".wal") {
                if name.len() != 20
                    || !name.as_bytes()[..16]
                        .iter()
                        .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(b))
                {
                    return Err(Error::Corrupt("WAL filename"));
                }
                numbers.push(
                    u64::from_str_radix(&name[..16], 16)
                        .map_err(|_| Error::Corrupt("WAL filename"))?,
                );
            }
        }
        numbers.sort_unstable();
        if numbers.iter().enumerate().any(|(i, n)| *n != i as u64) {
            return Err(Error::Corrupt("missing WAL segment"));
        }
        let mut wal = Self {
            directory,
            files: BTreeMap::new(),
            lineage,
            records: Vec::new(),
            segment: 0,
            offset: 64,
            durable: 0,
            poisoned: false,
        };
        if numbers.is_empty() {
            wal.create(0)?;
            return Ok(wal);
        }
        let mut chains = BTreeMap::new();
        let mut total_bytes = 0u64;
        for &number in &numbers {
            let mut file = wal.directory.open(&format!("{number:016x}.wal"))?;
            let len = file.len()?;
            total_bytes = total_bytes.checked_add(len).ok_or(Error::Limit)?;
            if total_bytes > MAX_WAL_BYTES {
                return Err(Error::Limit);
            }
            if !(64..=SEGMENT_SIZE).contains(&len) {
                return Err(Error::Corrupt("segment length"));
            }
            let mut header = [0; 64];
            read_exact_at(&file, 0, &mut header)?;
            check_segment(&header, lineage, number)?;
            let last = number == *numbers.last().unwrap();
            let mut offset = 64;
            while offset < len {
                let remaining = len - offset;
                let lsn = number
                    .checked_mul(SEGMENT_SIZE)
                    .and_then(|n| n.checked_add(offset))
                    .ok_or(Error::Limit)?;
                let mut h = vec![0; remaining.min(64) as usize];
                read_exact_at(&file, offset, &mut h)?;
                if h.iter().all(|b| *b == 0) {
                    // Only a full zero-padded segment may precede another segment.
                    let mut buf = [0; 8192];
                    let mut at = offset;
                    while at < len {
                        let count = (len - at).min(buf.len() as u64) as usize;
                        read_exact_at(&file, at, &mut buf[..count])?;
                        if buf[..count].iter().any(|b| *b != 0) {
                            return Err(Error::Corrupt("interior padding"));
                        }
                        at += count as u64;
                    }
                    if !last && len != SEGMENT_SIZE {
                        return Err(Error::Corrupt("short interior segment"));
                    }
                    if last {
                        file.truncate(offset)?;
                        sync(&mut file, true)?;
                    }
                    break;
                }
                if remaining < 64 {
                    if !last {
                        return Err(Error::Corrupt("interior short record"));
                    }
                    file.truncate(offset)?;
                    sync(&mut file, true)?;
                    break;
                }
                if &h[..4] != b"VWR1" {
                    return Err(Error::Corrupt("record magic"));
                }
                let size = u32at(&h, 4) as u64;
                if size < 64 || size > MAX_RECORD as u64 || offset + size > SEGMENT_SIZE {
                    return Err(Error::Corrupt("record size"));
                }
                if size > remaining {
                    if !last {
                        return Err(Error::Corrupt("interior torn record"));
                    }
                    let mut tail = vec![0; remaining as usize];
                    read_exact_at(&file, offset, &mut tail)?;
                    for at in 1..tail.len().saturating_sub(63) {
                        if &tail[at..at + 4] == b"VWR1" {
                            let n = u32at(&tail, at + 4) as usize;
                            if (64..=MAX_RECORD).contains(&n)
                                && at + n <= tail.len()
                                && Record::decode(&tail[at..at + n], lsn + at as u64).is_ok()
                            {
                                return Err(Error::Corrupt("valid record after torn interior"));
                            }
                        }
                    }
                    file.truncate(offset)?;
                    sync(&mut file, true)?;
                    break;
                }
                let mut bytes = vec![0; size as usize];
                read_exact_at(&file, offset, &mut bytes)?;
                let record = Record::decode(&bytes, lsn)?;
                if record.tx != 0 {
                    let previous = chains.get(&record.tx).copied().unwrap_or(0);
                    if record.prev != previous || (record.kind == Kind::Begin) != (previous == 0) {
                        return Err(Error::Corrupt("transaction chain"));
                    }
                    chains.insert(record.tx, lsn);
                }
                wal.records.push(record);
                offset += size;
            }
            wal.segment = number;
            wal.offset = offset;
            wal.files.insert(number, file);
        }
        // Recovery only sees validated complete records; flush reopening bytes before declaring a barrier.
        for file in wal.files.values_mut() {
            sync(file, true)?;
        }
        wal.directory.sync_directory()?;
        wal.durable = wal.records.last().map_or(0, |r| r.lsn);
        Ok(wal)
    }
    fn create(&mut self, number: u64) -> Result<()> {
        let mut file = self.directory.open(&format!("{number:016x}.wal"))?;
        if !file.is_empty()? {
            return Err(Error::Corrupt("segment creation collision"));
        }
        write_all_at(&mut file, 0, &segment_header(self.lineage, number)?)?;
        sync(&mut file, true)?;
        self.directory.sync_directory()?;
        self.files.insert(number, file);
        self.segment = number;
        self.offset = 64;
        Ok(())
    }
    pub fn into_directory(self) -> D {
        self.directory
    }
    pub fn is_poisoned(&self) -> bool {
        self.poisoned
    }
    fn append_inner(
        &mut self,
        kind: Kind,
        tx: u64,
        prev: u64,
        page: (u64, u64),
        payload: Vec<u8>,
    ) -> Result<u64> {
        if payload.len() > MAX_RECORD - 64 {
            return Err(Error::Limit);
        }
        let lsn = self
            .segment
            .checked_mul(SEGMENT_SIZE)
            .and_then(|n| n.checked_add(self.offset))
            .ok_or(Error::Limit)?;
        let budget = if matches!(kind, Kind::Clr | Kind::Abort | Kind::End) {
            MAX_WAL_BYTES
        } else {
            MUTATION_WAL_BYTES
        };
        if lsn
            .checked_add(64 + payload.len() as u64)
            .is_none_or(|n| n > budget)
        {
            return Err(Error::Limit);
        }
        let mut r = Record {
            kind,
            lsn,
            tx,
            prev,
            page,
            payload,
        };
        r.validate()?;
        if self.offset + 64 + r.payload.len() as u64 > SEGMENT_SIZE {
            let mut at = self.offset;
            let zeros = [0; 8192];
            let file = self.files.get_mut(&self.segment).unwrap();
            while at < SEGMENT_SIZE {
                let n = (SEGMENT_SIZE - at).min(8192) as usize;
                write_all_at(file, at, &zeros[..n])?;
                at += n as u64;
            }
            sync(file, true)?;
            self.create(self.segment.checked_add(1).ok_or(Error::Limit)?)?;
            r.lsn = self
                .segment
                .checked_mul(SEGMENT_SIZE)
                .and_then(|n| n.checked_add(64))
                .ok_or(Error::Limit)?;
        }
        let bytes = r.encode()?;
        write_all_at(
            self.files.get_mut(&self.segment).unwrap(),
            self.offset,
            &bytes,
        )?;
        self.offset += bytes.len() as u64;
        let lsn = r.lsn;
        self.records.push(r);
        Ok(lsn)
    }
}
impl<D: DirectoryIo + Send> Log for Wal<D>
where
    D::File: Send,
{
    fn append(
        &mut self,
        kind: Kind,
        tx: u64,
        prev: u64,
        page: (u64, u64),
        payload: Vec<u8>,
    ) -> Result<u64> {
        if self.poisoned {
            return Err(Error::Poisoned);
        }
        let result = self.append_inner(kind, tx, prev, page, payload);
        if matches!(result, Err(Error::Io(_))) {
            self.poisoned = true;
        }
        result
    }
    fn flush(&mut self, lsn: u64) -> Result<u64> {
        if self.poisoned {
            return Err(Error::Poisoned);
        }
        if lsn == 0 {
            return Ok(self.durable);
        }
        if !self.records.iter().any(|r| r.lsn == lsn) {
            return Err(Error::Corrupt("flush LSN"));
        }
        if lsn <= self.durable {
            return Ok(self.durable);
        }
        let result = (|| {
            for file in self.files.values_mut() {
                sync(file, false)?;
            }
            Ok(())
        })();
        if let Err(e) = result {
            self.poisoned = true;
            return Err(e);
        }
        self.durable = self.records.last().unwrap().lsn;
        Ok(self.durable)
    }
    fn records(&self) -> &[Record] {
        &self.records
    }
    fn lineage(&self) -> Lineage {
        self.lineage
    }
    fn durable(&self) -> u64 {
        self.durable
    }
}
impl<D: DirectoryIo + Send> WalBarrier for Wal<D>
where
    D::File: Send,
{
    fn durable_through(&mut self, lsn: u64) -> std::result::Result<(), IoFailure> {
        self.flush(lsn)
            .map(|_| ())
            .map_err(|_| IoFailure::DurabilityFailure)
    }
}
