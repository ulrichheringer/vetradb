//! Physiological recovery with redo-only CLRs and completed structural top actions.
pub mod journal;
use std::collections::{BTreeMap, BTreeSet};
use vetra_storage::codec::{Address, Image, PAGE_SIZE, Page};
use vetra_wal::{
    Kind, Log, Record,
    codec::{CHUNK_BYTES, checksum, crc32c, put64, u16at, u32at, u64at},
    envelope::committed,
};
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Error {
    Wal(vetra_wal::Error),
    Storage(vetra_storage::Error),
    Corrupt(&'static str),
    Limit,
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
/// Providers may return damaged bytes; only a validated WAL image may repair them.
pub trait Pages {
    fn load(&mut self, address: Address) -> Result<Option<Image>>;
    fn install(&mut self, address: Address, image: Image) -> Result<()>;
}
#[derive(Default)]
pub struct MemoryPages {
    pub pages: BTreeMap<Address, Image>,
}
impl Pages for MemoryPages {
    fn load(&mut self, address: Address) -> Result<Option<Image>> {
        Ok(self.pages.get(&address).copied())
    }
    fn install(&mut self, address: Address, image: Image) -> Result<()> {
        self.pages.insert(address, image);
        Ok(())
    }
}
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Progress {
    pub analyzed: usize,
    pub redone: usize,
    pub compensated: usize,
    pub ended: usize,
    pub committed: usize,
    pub completed_tops: usize,
}
fn validate(image: &Image, address: Address) -> Result<Page> {
    let owner = u64at(image, 32);
    Ok(Page::decode(image, address, owner)?)
}
fn address(r: &Record) -> Result<Address> {
    Ok(Address::checked(r.page.0, r.page.1)?)
}
fn patch(pages: &mut impl Pages, r: &Record, prefix: usize, lsn: u64) -> Result<bool> {
    let a = address(r)?;
    let mut image = pages
        .load(a)?
        .ok_or(Error::Corrupt("patch without base image"))?;
    let page = validate(&image, a)?;
    if page.lsn >= lsn {
        return Ok(false);
    }
    let offset = u16at(&r.payload, prefix) as usize;
    let len = u16at(&r.payload, prefix + 2) as usize;
    let start = prefix + 8;
    let data = if r.kind == Kind::Clr {
        &r.payload[start..start + len]
    } else {
        &r.payload[start + len..start + 2 * len]
    };
    image[offset..offset + len].copy_from_slice(data);
    put64(&mut image, 24, lsn);
    checksum(&mut image, 48);
    validate(&image, a)?;
    pages.install(a, image)?;
    Ok(true)
}
/// All WAL is retained in this bring-up. Checkpoints cannot authorize recycling by themselves.
pub fn recover(log: &mut dyn Log, pages: &mut impl Pages) -> Result<Progress> {
    let records = log.records().to_vec();
    completed_checkpoints(&records)?;
    if let Some(last) = records.last() {
        log.flush(last.lsn)?;
    }
    let structural = journal::actions(&records)?;
    let envelopes = committed(&records, log.lineage())?;
    let committed_ids: BTreeSet<_> = envelopes.iter().map(|e| e.tx).collect();
    let mut progress = Progress {
        analyzed: records.len(),
        committed: envelopes.len(),
        ..Progress::default()
    };
    let by_lsn: BTreeMap<_, _> = records.iter().map(|r| (r.lsn, r)).collect();
    let known = records.last().map_or(0, |r| r.lsn);
    let addresses: BTreeSet<_> = records
        .iter()
        .filter(|r| r.page.0 >= 2 && r.page.0 < journal::MANIFEST_START)
        .map(|r| r.page)
        .collect();
    for (id, generation) in addresses {
        if let Some(raw) = pages.load(Address { id, generation })? {
            let actual = Address {
                id: u64at(&raw, 8),
                generation: u64at(&raw, 16),
            };
            if validate(&raw, actual).is_ok_and(|page| page.lsn > known) {
                return Err(Error::Corrupt("page LSN is ahead of retained WAL"));
            }
        }
    }
    let mut active = BTreeMap::new();
    let mut ended = BTreeSet::new();
    let mut tops: BTreeMap<u64, Vec<&Record>> = BTreeMap::new();
    let mut complete = Vec::new();
    let mut current_top = None;
    for r in &records {
        if r.tx != 0 {
            active.insert(r.tx, r.lsn);
            if r.kind == Kind::End {
                if !committed_ids.contains(&r.tx) {
                    let mut next = r.prev;
                    let mut seen = BTreeSet::new();
                    while next != 0 {
                        if !seen.insert(next) {
                            return Err(Error::Corrupt("completed undo cycle"));
                        }
                        let before = by_lsn
                            .get(&next)
                            .ok_or(Error::Corrupt("completed undo missing link"))?;
                        if before.tx != r.tx {
                            return Err(Error::Corrupt("completed undo owner"));
                        }
                        if before.kind == Kind::Patch {
                            return Err(Error::Corrupt("END before loser undo completed"));
                        }
                        next = if before.kind == Kind::Clr {
                            u64at(&before.payload, 0)
                        } else {
                            before.prev
                        };
                    }
                }
                ended.insert(r.tx);
            }
        }
        match r.kind {
            Kind::TopBegin => {
                let id = u64at(&r.payload, 0);
                if tops.contains_key(&id) {
                    return Err(Error::Corrupt("overlapping top actions"));
                }
                tops.insert(id, Vec::new());
                current_top = Some(id);
            }
            Kind::FullPage if r.tx == 0 => {
                let id = current_top.ok_or(Error::Corrupt("unscoped structural image"))?;
                tops.get_mut(&id).unwrap().push(r);
            }
            Kind::TopPatch => {
                let id = u64at(&r.payload, 0);
                if current_top != Some(id) {
                    return Err(Error::Corrupt("top patch ownership"));
                }
                tops.get_mut(&id).unwrap().push(r);
            }
            Kind::TopEnd => {
                let id = u64at(&r.payload, 0);
                if current_top != Some(id) {
                    return Err(Error::Corrupt("top end ownership"));
                }
                complete.push((r.lsn, tops.remove(&id).unwrap()));
                current_top = None;
            }
            _ => {}
        }
    }
    let top_ends: BTreeMap<u64, &Vec<&Record>> = complete
        .iter()
        .map(|(lsn, patches)| (*lsn, patches))
        .collect();
    progress.completed_tops = complete.len();
    let mut structural_latest = BTreeMap::new();
    for (end, batch) in &structural {
        for a in batch {
            for p in &a.pages {
                structural_latest.insert(p.id, (*end, p));
            }
        }
    }
    let manifest_tops: BTreeSet<_> = structural.iter().map(|(lsn, _)| *lsn).collect();
    for (end, p) in structural_latest.values() {
        if let Some(raw) = pages.load(Address {
            id: p.id,
            generation: p.generation,
        })? {
            let actual = Address {
                id: u64at(&raw, 8),
                generation: u64at(&raw, 16),
            };
            if let Ok(old) = validate(&raw, actual) {
                if actual.generation > p.generation
                    || actual.generation == p.generation && old.owner != u64at(&p.bytes, 32)
                {
                    return Err(Error::Corrupt(
                        "physical generation/owner is not explained by WAL",
                    ));
                }
            }
        }
        let mut image: Image = p
            .bytes
            .as_slice()
            .try_into()
            .map_err(|_| Error::Corrupt("COW page size"))?;
        put64(&mut image, 24, *end);
        checksum(&mut image, 48);
        pages.install(
            Address {
                id: p.id,
                generation: p.generation,
            },
            image,
        )?;
    }
    let top_records: BTreeSet<_> = complete
        .iter()
        .flat_map(|(_, v)| v.iter().map(|r| r.lsn))
        .collect();
    // Repair only from eligible complete images, before applying any patch to a torn page.
    for r in records.iter().filter(|r| {
        r.kind == Kind::FullPage
            && r.page.0 < journal::MANIFEST_START
            && (r.tx != 0 || top_records.contains(&r.lsn))
            && !structural_latest.contains_key(&r.page.0)
    }) {
        let a = address(r)?;
        let old = pages.load(a)?;
        if old.as_ref().is_none_or(|b| validate(b, a).is_err()) {
            if let Some(raw) = old.as_ref() {
                // A valid different generation is not a torn page. Do not overwrite it without logged reuse proof.
                let actual = Address {
                    id: u64at(raw, 8),
                    generation: u64at(raw, 16),
                };
                if actual != a && validate(raw, actual).is_ok() {
                    return Err(Error::Corrupt(
                        "page generation mismatch without allocation proof",
                    ));
                }
            }
            let image: Image = r
                .payload
                .as_slice()
                .try_into()
                .map_err(|_| Error::Corrupt("image size"))?;
            validate(&image, a)?;
            pages.install(a, image)?;
        }
    }
    for r in &records {
        match r.kind {
            Kind::Patch | Kind::Clr => {
                progress.redone += usize::from(patch(
                    pages,
                    r,
                    if r.kind == Kind::Clr { 8 } else { 0 },
                    r.lsn,
                )?);
            }
            Kind::TopEnd => {
                if manifest_tops.contains(&r.lsn) {
                    progress.redone += 1;
                    continue;
                }
                // Validate/stage every page first; expose no partially checked action.
                let mut staged = MemoryPages::default();
                let changes = top_ends
                    .get(&r.lsn)
                    .ok_or(Error::Corrupt("missing top action"))?;
                for change in *changes {
                    let a = address(change)?;
                    if let std::collections::btree_map::Entry::Vacant(entry) = staged.pages.entry(a)
                    {
                        if let Some(image) = pages.load(a)? {
                            entry.insert(image);
                        }
                    }
                    if change.kind == Kind::FullPage {
                        let old = staged.pages.get(&a);
                        if old.is_none_or(|b| validate(b, a).is_err()) {
                            staged
                                .pages
                                .insert(a, change.payload.as_slice().try_into().unwrap());
                        }
                    } else {
                        patch(&mut staged, change, 8, change.lsn)?;
                    }
                }
                for (a, mut image) in staged.pages {
                    let page = validate(&image, a)?;
                    if page.lsn < r.lsn {
                        put64(&mut image, 24, r.lsn);
                        checksum(&mut image, 48);
                    }
                    pages.install(a, image)?;
                }
                progress.redone += 1;
            }
            _ => {}
        }
    }
    for (tx, last) in active {
        if ended.contains(&tx) || committed_ids.contains(&tx) {
            continue;
        }
        let mut prev = last;
        let mut next = last;
        let mut seen = BTreeSet::new();
        while next != 0 {
            if !seen.insert(next) {
                return Err(Error::Corrupt("undo cycle"));
            }
            let r = by_lsn
                .get(&next)
                .ok_or(Error::Corrupt("missing undo record"))?;
            if r.tx != tx {
                return Err(Error::Corrupt("undo owner"));
            }
            match r.kind {
                Kind::Clr => next = u64at(&r.payload, 0),
                Kind::Patch => {
                    let n = u16at(&r.payload, 2) as usize;
                    let mut payload = r.prev.to_le_bytes().to_vec();
                    payload.extend(&r.payload[..8]);
                    payload.extend(&r.payload[8..8 + n]);
                    let lsn = log.append(Kind::Clr, tx, prev, r.page, payload.clone())?;
                    log.flush(lsn)?;
                    let clr = Record {
                        kind: Kind::Clr,
                        lsn,
                        tx,
                        prev,
                        page: r.page,
                        payload,
                    };
                    patch(pages, &clr, 8, lsn)?;
                    prev = lsn;
                    next = r.prev;
                    progress.compensated += 1;
                }
                _ => next = r.prev,
            }
        }
        let abort = log.append(Kind::Abort, tx, prev, (0, 0), vec![])?;
        let end = log.append(Kind::End, tx, abort, (0, 0), vec![])?;
        log.flush(end)?;
        progress.ended += 1;
    }
    Ok(progress)
}
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Checkpoint {
    pub dirty: Vec<(u64, u64, u64)>,
    pub active: Vec<(u64, u64, u64, u64, u8)>,
    pub tops: Vec<(u64, u64)>,
}
impl Checkpoint {
    pub fn encode(&self) -> Result<Vec<u8>> {
        if [self.dirty.len(), self.active.len(), self.tops.len()]
            .iter()
            .any(|n| *n > 262144)
        {
            return Err(Error::Limit);
        }
        let mut b = Vec::new();
        for n in [self.dirty.len(), self.active.len(), self.tops.len(), 0] {
            b.extend((n as u32).to_le_bytes());
        }
        let mut seen = BTreeSet::new();
        for &(id, g, lsn) in &self.dirty {
            if id < 2 || g == 0 || lsn == 0 || !seen.insert((id, g)) {
                return Err(Error::Corrupt("dirty table"));
            }
            for n in [id, g, lsn] {
                b.extend(n.to_le_bytes());
            }
        }
        let mut seen = BTreeSet::new();
        for &(tx, last, next, csn, state) in &self.active {
            if tx == 0 || last == 0 || ![1, 2, 3].contains(&state) || !seen.insert(tx) {
                return Err(Error::Corrupt("active table"));
            }
            for n in [tx, last, next, csn] {
                b.extend(n.to_le_bytes());
            }
            b.push(state);
            b.extend([0; 7]);
        }
        let mut seen = BTreeSet::new();
        for &(id, begin) in &self.tops {
            if id == 0 || begin == 0 || !seen.insert(id) {
                return Err(Error::Corrupt("top table"));
            }
            b.extend(id.to_le_bytes());
            b.extend(begin.to_le_bytes());
        }
        if b.len() > 16 * 1024 * 1024 {
            return Err(Error::Limit);
        }
        Ok(b)
    }
    pub fn decode(b: &[u8]) -> Result<Self> {
        if b.len() < 16 || b.len() > 16 * 1024 * 1024 || b[12..16] != [0; 4] {
            return Err(Error::Corrupt("checkpoint table header"));
        }
        let counts = [
            u32at(b, 0) as usize,
            u32at(b, 4) as usize,
            u32at(b, 8) as usize,
        ];
        if counts.iter().any(|n| *n > 262144)
            || 16 + counts[0] * 24 + counts[1] * 40 + counts[2] * 16 != b.len()
        {
            return Err(Error::Corrupt("checkpoint table bounds"));
        }
        let mut table = Self::default();
        let mut o = 16;
        for _ in 0..counts[0] {
            table
                .dirty
                .push((u64at(b, o), u64at(b, o + 8), u64at(b, o + 16)));
            o += 24;
        }
        for _ in 0..counts[1] {
            if b[o + 33..o + 40] != [0; 7] {
                return Err(Error::Corrupt("checkpoint state flags"));
            }
            table.active.push((
                u64at(b, o),
                u64at(b, o + 8),
                u64at(b, o + 16),
                u64at(b, o + 24),
                b[o + 32],
            ));
            o += 40;
        }
        for _ in 0..counts[2] {
            table.tops.push((u64at(b, o), u64at(b, o + 8)));
            o += 16;
        }
        table.encode()?;
        Ok(table)
    }
    pub fn retention(&self, checkpoint_begin: u64, pins: &[u64]) -> u64 {
        self.dirty
            .iter()
            .map(|e| e.2)
            .chain(
                self.active
                    .iter()
                    .map(|e| if e.2 == 0 { e.1 } else { e.1.min(e.2) }),
            )
            // Active chains may reach back before lastLSN/undoNextLSN.
            // Keep the original log start until a chain-walk proves a newer floor.
            .chain((!self.active.is_empty()).then_some(64))
            .chain(self.tops.iter().map(|e| e.1))
            .chain(pins.iter().copied())
            .chain([checkpoint_begin])
            .filter(|n| *n != 0)
            .min()
            .unwrap_or(0)
    }
}
/// An incomplete checkpoint is ignored; a complete inconsistent one fails closed.
pub fn completed_checkpoints(records: &[Record]) -> Result<Vec<(u64, Checkpoint)>> {
    let mut pending: BTreeMap<u64, (u64, Vec<&Record>)> = BTreeMap::new();
    let mut completed = Vec::new();
    for r in records {
        match r.kind {
            Kind::CheckpointBegin => {
                let id = u64at(&r.payload, 0);
                if pending.insert(id, (r.lsn, Vec::new())).is_some() {
                    return Err(Error::Corrupt("duplicate checkpoint"));
                }
            }
            Kind::CheckpointChunk => {
                let id = u64at(&r.payload, 0);
                let (_, chunks) = pending
                    .get_mut(&id)
                    .ok_or(Error::Corrupt("orphan checkpoint chunk"))?;
                if chunks.len() >= 17 {
                    return Err(Error::Limit);
                }
                chunks.push(r);
            }
            Kind::CheckpointEnd => {
                let id = u64at(&r.payload, 0);
                let (begin, chunks) = pending
                    .remove(&id)
                    .ok_or(Error::Corrupt("orphan checkpoint end"))?;
                if begin != u64at(&r.payload, 8) {
                    return Err(Error::Corrupt("checkpoint begin link"));
                }
                let total = u32at(&r.payload, 16) as usize;
                let mut bytes = Vec::new();
                for (i, chunk) in chunks.iter().enumerate() {
                    let p = &chunk.payload;
                    if u32at(p, 8) as usize != i
                        || u32at(p, 12) as usize != chunks.len()
                        || bytes.len() + p.len() - 24 > total
                    {
                        return Err(Error::Corrupt("checkpoint completeness"));
                    }
                    bytes.extend(&p[24..]);
                }
                if bytes.len() != total || crc32c(&bytes) != u32at(&r.payload, 20) {
                    return Err(Error::Corrupt("checkpoint integrity"));
                }
                completed.push((r.lsn, Checkpoint::decode(&bytes)?));
            }
            _ => {}
        }
    }
    Ok(completed)
}
pub fn checkpoint(log: &mut dyn Log, id: u64, table: &Checkpoint) -> Result<u64> {
    let bytes = table.encode()?;
    let begin = log.append(
        Kind::CheckpointBegin,
        0,
        0,
        (0, 0),
        id.to_le_bytes().to_vec(),
    )?;
    let count = bytes.len().div_ceil(CHUNK_BYTES);
    for (i, chunk) in bytes.chunks(CHUNK_BYTES).enumerate() {
        let mut p = id.to_le_bytes().to_vec();
        for n in [i, count, chunk.len(), 0] {
            p.extend((n as u32).to_le_bytes());
        }
        p.extend(chunk);
        log.append(Kind::CheckpointChunk, 0, 0, (0, 0), p)?;
    }
    let mut p = id.to_le_bytes().to_vec();
    p.extend(begin.to_le_bytes());
    p.extend((bytes.len() as u32).to_le_bytes());
    p.extend(crc32c(&bytes).to_le_bytes());
    let end = log.append(Kind::CheckpointEnd, 0, 0, (0, 0), p)?;
    log.flush(end)?;
    Ok(end)
}
/// WAL image protects every checkpoint-era first mutation. Caller holds the page latch.
pub fn log_patch(log: &mut dyn Log, tx: u64, prev: u64, page: &Page, after: &Page) -> Result<u64> {
    if page.address != after.address || page.owner != after.owner {
        return Err(Error::Corrupt("patch identity"));
    }
    let before = page.encode()?;
    let image = after.encode()?;
    let fpi = log.append(
        Kind::FullPage,
        tx,
        prev,
        (page.address.id, page.address.generation),
        before.to_vec(),
    )?;
    let mut payload = 64u16.to_le_bytes().to_vec();
    payload.extend(((PAGE_SIZE - 64) as u16).to_le_bytes());
    payload.extend([0; 4]);
    payload.extend(&before[64..]);
    payload.extend(&image[64..]);
    Ok(log.append(
        Kind::Patch,
        tx,
        fpi,
        (page.address.id, page.address.generation),
        payload,
    )?)
}
