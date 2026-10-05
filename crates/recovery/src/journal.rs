//! Durable COW action bridge: existing v1 pages plus a bounded internal action codec.
use crate::{Error, Result};
use std::collections::BTreeMap;
use vetra_recovery_api::{PageImage, StructuralAction, StructuralJournal};
use vetra_storage::{
    codec::{Address, Body, Page},
    tree::{BPlusTree, TreeConfig},
};
use vetra_types::IoFailure;
use vetra_wal::{
    Kind, Log, Record,
    codec::{u32at, u64at},
};
/// Reserved WAL-only metadata arena; user tree arenas are bounded to 1,000,000 pages.
pub const MANIFEST_START: u64 = 1_000_002;
const LIMIT: usize = 16 * 1024 * 1024;
struct Reader<'a> {
    b: &'a [u8],
    o: usize,
}
impl<'a> Reader<'a> {
    fn take(&mut self, n: usize) -> Result<&'a [u8]> {
        let end = self.o.checked_add(n).ok_or(Error::Limit)?;
        let slice = self
            .b
            .get(self.o..end)
            .ok_or(Error::Corrupt("action framing"))?;
        self.o = end;
        Ok(slice)
    }
    fn n(&mut self) -> Result<u64> {
        Ok(u64at(self.take(8)?, 0))
    }
    fn bytes(&mut self) -> Result<Vec<u8>> {
        let n = u32at(self.take(4)?, 0) as usize;
        Ok(self.take(n)?.to_vec())
    }
    fn count(&mut self) -> Result<usize> {
        let n = u32at(self.take(4)?, 0) as usize;
        if n > 1_000_000 {
            return Err(Error::Limit);
        }
        Ok(n)
    }
}
fn blob(out: &mut Vec<u8>, b: &[u8]) -> Result<()> {
    if b.len() > LIMIT || out.len().checked_add(4 + b.len()).is_none_or(|n| n > LIMIT) {
        return Err(Error::Limit);
    }
    out.extend((b.len() as u32).to_le_bytes());
    out.extend(b);
    Ok(())
}
/// Page bytes themselves are FULL_PAGE records. Metadata names those exact images.
fn encode(actions: &[StructuralAction]) -> Result<Vec<u8>> {
    if actions.is_empty() || actions.len() > 16 {
        return Err(Error::Limit);
    }
    let mut b = b"VACT0001".to_vec();
    b.extend((actions.len() as u32).to_le_bytes());
    for a in actions {
        for n in [
            a.tree,
            a.previous_root.0,
            a.previous_root.1,
            a.root.0,
            a.root.1,
            a.identity_high_water,
        ] {
            b.extend(n.to_le_bytes());
        }
        blob(&mut b, &a.allocation)?;
        b.extend((a.pages.len() as u32).to_le_bytes());
        for p in &a.pages {
            b.extend(p.id.to_le_bytes());
            b.extend(p.generation.to_le_bytes());
        }
        b.extend((a.retired.len() as u32).to_le_bytes());
        for &(id, g) in &a.retired {
            b.extend(id.to_le_bytes());
            b.extend(g.to_le_bytes());
        }
        b.extend((a.record_ids.len() as u32).to_le_bytes());
        for (k, id) in &a.record_ids {
            blob(&mut b, k)?;
            b.extend(id.to_le_bytes());
        }
        if b.len() > LIMIT {
            return Err(Error::Limit);
        }
    }
    Ok(b)
}
fn decode(b: &[u8], images: &BTreeMap<(u64, u64), Vec<u8>>) -> Result<Vec<StructuralAction>> {
    if b.len() > LIMIT || b.len() < 12 || &b[..8] != b"VACT0001" {
        return Err(Error::Corrupt("action codec"));
    }
    let mut c = Reader { b, o: 8 };
    let count = c.count()?;
    if !(1..=16).contains(&count) {
        return Err(Error::Limit);
    }
    let mut actions = Vec::new();
    let mut seen = std::collections::BTreeSet::new();
    for _ in 0..count {
        let tree = c.n()?;
        let previous_root = (c.n()?, c.n()?);
        let root = (c.n()?, c.n()?);
        let identity_high_water = c.n()?;
        let allocation = c.bytes()?;
        let n = c.count()?;
        let mut pages = Vec::new();
        for _ in 0..n {
            let id = c.n()?;
            let generation = c.n()?;
            if id >= MANIFEST_START || !seen.insert(id) {
                return Err(Error::Corrupt("action page overlap"));
            }
            let bytes = images
                .get(&(id, generation))
                .ok_or(Error::Corrupt("missing action image"))?
                .clone();
            Page::decode(&bytes, Address { id, generation }, tree)?;
            pages.push(PageImage {
                id,
                generation,
                bytes,
            });
        }
        let n = c.count()?;
        let mut retired = Vec::new();
        for _ in 0..n {
            retired.push((c.n()?, c.n()?));
        }
        let n = c.count()?;
        let mut record_ids = Vec::new();
        for _ in 0..n {
            record_ids.push((c.bytes()?, c.n()?));
        }
        actions.push(StructuralAction {
            tree,
            previous_root,
            root,
            pages,
            allocation,
            retired,
            record_ids,
            identity_high_water,
        });
    }
    if c.o != b.len() || seen.len() != images.len() {
        return Err(Error::Corrupt("action exact consumption"));
    }
    Ok(actions)
}
// Compare completed batch snapshots, never intermediate snapshots of a multi-tree batch.
fn allocation_forward(previous: Option<&[u8]>, next: &[u8]) -> Result<()> {
    vetra_storage::allocator::Allocator::decode(next, 65536)?;
    let Some(previous) = previous else {
        return Ok(());
    };
    let entries: BTreeMap<_, _> = next[16..]
        .chunks_exact(32)
        .map(|e| (u64at(e, 0), (u64at(e, 8), u64at(e, 16))))
        .collect();
    for e in previous[16..].chunks_exact(32) {
        let (old_generation, old_owner) = (u64at(e, 8), u64at(e, 16));
        let &(generation, owner) = entries
            .get(&u64at(e, 0))
            .ok_or(Error::Corrupt("allocator identity disappeared"))?;
        if generation < old_generation
            || (generation == old_generation && owner != 0 && owner != old_owner)
        {
            return Err(Error::Corrupt("allocator generation regressed"));
        }
    }
    Ok(())
}
pub struct Journal<L: Log> {
    pub log: L,
    pub flush_on_seal: bool,
    poisoned: bool,
    trees: BTreeMap<u64, BPlusTree>,
    allocation: Option<Vec<u8>>,
}
impl<L: Log> Journal<L> {
    pub fn new(log: L, flush_on_seal: bool) -> Result<Self> {
        let mut trees = BTreeMap::new();
        let mut allocation = None;
        for (lsn, batch) in actions(log.records())? {
            allocation = batch.last().map(|a| a.allocation.clone());
            for a in batch {
                if let std::collections::btree_map::Entry::Vacant(entry) = trees.entry(a.tree) {
                    entry.insert(BPlusTree::new(a.tree, TreeConfig::default())?);
                }
                trees.get_mut(&a.tree).unwrap().replay(lsn, &a)?;
            }
        }
        Ok(Self {
            log,
            flush_on_seal,
            poisoned: false,
            trees,
            allocation,
        })
    }
    fn seal_inner(&mut self, actions: Vec<StructuralAction>) -> Result<u64> {
        let bytes = encode(&actions)?;
        allocation_forward(
            self.allocation.as_deref(),
            &actions.last().unwrap().allocation,
        )?;
        let id = self
            .log
            .records()
            .last()
            .map_or(1, |r| r.lsn.checked_add(1).unwrap_or(0));
        if id == 0 {
            return Err(Error::Limit);
        }
        let mut staged: BTreeMap<_, _> = self
            .trees
            .iter()
            .map(|(&owner, tree)| (owner, tree.staged_copy()))
            .collect();
        for a in &actions {
            if let std::collections::btree_map::Entry::Vacant(entry) = staged.entry(a.tree) {
                entry.insert(BPlusTree::new(a.tree, TreeConfig::default())?);
            }
            staged.get_mut(&a.tree).unwrap().replay(id, a)?;
        }
        let allocation = vetra_storage::allocator::Allocator::decode(
            &actions.last().unwrap().allocation,
            65536,
        )?;
        let mut seen = std::collections::BTreeSet::new();
        for (&owner, tree) in &staged {
            for p in tree.page_images()? {
                if !seen.insert(p.id) {
                    return Err(Error::Corrupt("action cross-tree ownership"));
                }
                allocation.check(
                    Address {
                        id: p.id,
                        generation: p.generation,
                    },
                    owner,
                )?;
            }
        }
        self.log
            .append(Kind::TopBegin, 0, 0, (0, 0), id.to_le_bytes().to_vec())?;
        for a in &actions {
            for p in &a.pages {
                if p.id >= MANIFEST_START {
                    return Err(Error::Limit);
                }
                self.log
                    .append(Kind::FullPage, 0, 0, (p.id, p.generation), p.bytes.clone())?;
            }
        }
        let count = bytes.len().div_ceil(8096);
        for (i, chunk) in bytes.chunks(8096).enumerate() {
            let address = Address {
                id: MANIFEST_START + i as u64,
                generation: id,
            };
            let page = Page {
                address,
                lsn: 0,
                owner: 1,
                body: Body::Overflow {
                    next: if i + 1 < count {
                        Address {
                            id: address.id + 1,
                            generation: id,
                        }
                    } else {
                        Address::default()
                    },
                    owner_record: id,
                    chunk: chunk.to_vec(),
                },
            };
            self.log.append(
                Kind::FullPage,
                0,
                0,
                (address.id, address.generation),
                page.encode()?.to_vec(),
            )?;
        }
        let end = self
            .log
            .append(Kind::TopEnd, 0, 0, (0, 0), id.to_le_bytes().to_vec())?;
        if self.flush_on_seal {
            self.log.flush(end)?;
        }
        for a in &actions {
            staged.get_mut(&a.tree).unwrap().stamp(end);
        }
        self.allocation = actions.last().map(|a| a.allocation.clone());
        self.trees = staged;
        Ok(end)
    }
}
impl<L: Log> StructuralJournal for Journal<L> {
    fn seal(&mut self, action: StructuralAction) -> std::result::Result<u64, IoFailure> {
        self.seal_batch(vec![action])
    }
    fn seal_batch(
        &mut self,
        actions: Vec<StructuralAction>,
    ) -> std::result::Result<u64, IoFailure> {
        if self.poisoned {
            return Err(IoFailure::DurabilityFailure);
        }
        match self.seal_inner(actions) {
            Ok(lsn) => Ok(lsn),
            Err(_) => {
                self.poisoned = true;
                Err(IoFailure::DurabilityFailure)
            }
        }
    }
}
/// Validate metadata and replay root/allocator/identity histories before exposing any action.
pub fn actions(records: &[Record]) -> Result<Vec<(u64, Vec<StructuralAction>)>> {
    type Pending<'a> = (u64, BTreeMap<(u64, u64), Vec<u8>>, Vec<&'a Record>);
    let mut pending: Option<Pending<'_>> = None;
    let mut completed = Vec::new();
    let mut trees = BTreeMap::new();
    let mut previous_allocation: Option<Vec<u8>> = None;
    for r in records {
        match r.kind {
            Kind::TopBegin => {
                pending = Some((u64at(&r.payload, 0), BTreeMap::new(), Vec::new()));
            }
            Kind::FullPage if r.tx == 0 => {
                let (_, images, metadata) = pending
                    .as_mut()
                    .ok_or(Error::Corrupt("orphan action image"))?;
                if r.page.0 >= MANIFEST_START {
                    metadata.push(r);
                } else if images.insert(r.page, r.payload.clone()).is_some() {
                    return Err(Error::Corrupt("duplicate action image"));
                }
            }
            Kind::TopEnd => {
                let (id, images, mut metadata) = pending
                    .take()
                    .ok_or(Error::Corrupt("orphan structural END"))?;
                if id != u64at(&r.payload, 0) {
                    return Err(Error::Corrupt("structural identity"));
                }
                if metadata.is_empty() {
                    continue;
                }
                metadata.sort_by_key(|r| r.page.0);
                let mut bytes = Vec::new();
                for (i, r) in metadata.iter().enumerate() {
                    let address = Address {
                        id: MANIFEST_START + i as u64,
                        generation: id,
                    };
                    if r.page != (address.id, address.generation) {
                        return Err(Error::Corrupt("manifest address"));
                    }
                    let page = Page::decode(&r.payload, address, 1)?;
                    let Body::Overflow {
                        next,
                        owner_record,
                        chunk,
                    } = page.body
                    else {
                        return Err(Error::Corrupt("manifest kind"));
                    };
                    let expected = if i + 1 < metadata.len() {
                        Address {
                            id: address.id + 1,
                            generation: id,
                        }
                    } else {
                        Address::default()
                    };
                    if next != expected || owner_record != id || bytes.len() + chunk.len() > LIMIT {
                        return Err(Error::Corrupt("manifest chain"));
                    }
                    bytes.extend(chunk);
                }
                let batch = decode(&bytes, &images)?;
                for a in &batch {
                    if let std::collections::btree_map::Entry::Vacant(entry) = trees.entry(a.tree) {
                        entry.insert(BPlusTree::new(a.tree, TreeConfig::default())?);
                    }
                    trees.get_mut(&a.tree).unwrap().replay(r.lsn, a)?;
                }
                let allocation = vetra_storage::allocator::Allocator::decode(
                    &batch.last().unwrap().allocation,
                    65536,
                )?;
                let mut live = std::collections::BTreeSet::new();
                for (&owner, tree) in &trees {
                    for p in tree.page_images()? {
                        if !live.insert(p.id) {
                            return Err(Error::Corrupt("cross-tree page overlap"));
                        }
                        allocation.check(
                            Address {
                                id: p.id,
                                generation: p.generation,
                            },
                            owner,
                        )?;
                    }
                }
                allocation_forward(
                    previous_allocation.as_deref(),
                    &batch.last().unwrap().allocation,
                )?;
                previous_allocation = batch.last().map(|a| a.allocation.clone());
                completed.push((r.lsn, batch));
            }
            _ => {}
        }
    }
    Ok(completed)
}
