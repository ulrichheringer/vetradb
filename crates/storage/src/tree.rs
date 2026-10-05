//! Serialized copy-on-write B+Tree. Repacking trades write amplification for an
//! atomic, easy-to-audit M01 structural boundary. Keys use structural byte order.
use crate::{
    Error, Result,
    allocator::{Allocator, ReusePin},
    codec::{Address, Body, MAX_VALUE, OverflowRef, Page, Record},
};
use std::{
    collections::{BTreeMap, BTreeSet},
    ops::Bound,
    sync::Arc,
};
use vetra_recovery_api::{PageImage, StructuralAction, StructuralJournal};
pub const MAX_KEY: usize = 2048;
#[derive(Clone, Copy, Debug)]
pub struct TreeConfig {
    pub fanout: usize,
    pub max_pages: usize,
    pub max_records: usize,
}
impl Default for TreeConfig {
    fn default() -> Self {
        Self {
            fanout: 64,
            max_pages: 65536,
            max_records: 100000,
        }
    }
}
pub struct BPlusTree {
    owner: u64,
    root: Address,
    pages: Arc<BTreeMap<Address, Page>>,
    allocator: Allocator,
    config: TreeConfig,
    record_ids: Arc<BTreeMap<Vec<u8>, u64>>,
    high_water: u64,
    epoch: u64,
}
pub struct Snapshot {
    root: Address,
    owner: u64,
    pages: Arc<BTreeMap<Address, Page>>,
    record_ids: Arc<BTreeMap<Vec<u8>, u64>>,
    _pins: Vec<ReusePin>,
    pub epoch: u64,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ScanPosition {
    pub key: Vec<u8>,
    pub descending: bool,
}
pub type KeyValue = (Vec<u8>, Vec<u8>);
impl BPlusTree {
    pub fn new(owner: u64, config: TreeConfig) -> Result<Self> {
        if owner < 2
            || !(3..=256).contains(&config.fanout)
            || config.max_records == 0
            || config.max_records > 1_000_000
        {
            return Err(Error::Limit);
        }
        Ok(Self {
            owner,
            root: Address::default(),
            pages: Arc::new(BTreeMap::new()),
            allocator: Allocator::new(config.max_pages)?,
            config,
            record_ids: Arc::new(BTreeMap::new()),
            high_water: 0,
            epoch: 0,
        })
    }
    pub(crate) fn staged_copy(&self) -> Self {
        Self {
            owner: self.owner,
            root: self.root,
            pages: self.pages.clone(),
            allocator: self.allocator.clone(),
            config: self.config,
            record_ids: self.record_ids.clone(),
            high_water: self.high_water,
            epoch: self.epoch,
        }
    }
    pub(crate) fn share_allocator(&mut self, other: &Self) {
        self.allocator = other.allocator.clone();
    }
    pub(crate) fn stamp(&mut self, lsn: u64) {
        self.epoch = lsn;
        for p in Arc::make_mut(&mut self.pages).values_mut() {
            p.lsn = lsn;
        }
    }
    pub fn root(&self) -> Address {
        self.root
    }
    pub fn owner(&self) -> u64 {
        self.owner
    }
    pub fn config(&self) -> TreeConfig {
        self.config
    }
    pub fn snapshot(&self) -> Result<Snapshot> {
        let mut pins = Vec::new();
        for &a in self.pages.keys() {
            pins.push(self.allocator.pin(a, self.owner)?);
        }
        Ok(Snapshot {
            root: self.root,
            owner: self.owner,
            pages: self.pages.clone(),
            record_ids: self.record_ids.clone(),
            _pins: pins,
            epoch: self.epoch,
        })
    }
    pub fn get(&self, key: &[u8]) -> Result<Option<Vec<u8>>> {
        lookup(&self.pages, self.root, self.owner, &self.record_ids, key)
    }
    pub fn insert(
        &mut self,
        key: Vec<u8>,
        value: Vec<u8>,
        creator: u64,
        schema: u64,
        journal: &mut impl StructuralJournal,
    ) -> Result<Option<Vec<u8>>> {
        if key.len() > MAX_KEY || value.len() > MAX_VALUE {
            return Err(Error::Limit);
        }
        if creator == 0 || schema == 0 {
            return Err(Error::Corrupt("user record identity"));
        }
        let old = self.get(&key)?;
        let mut records = self.records()?;
        if !records.contains_key(&key) && records.len() >= self.config.max_records {
            return Err(Error::Limit);
        }
        let mut ids = (*self.record_ids).clone();
        let mut high = self.high_water;
        if !ids.contains_key(&key) {
            high = high.checked_add(1).ok_or(Error::Exhausted)?;
            ids.insert(key.clone(), high);
        }
        records.insert(
            key.clone(),
            Record {
                key,
                value,
                creator,
                schema,
                overflow: false,
            },
        );
        self.rebuild(records, ids, high, journal)?;
        Ok(old)
    }
    pub fn delete(
        &mut self,
        key: &[u8],
        journal: &mut impl StructuralJournal,
    ) -> Result<Option<Vec<u8>>> {
        let old = self.get(key)?;
        if old.is_none() {
            return Ok(None);
        }
        let mut records = self.records()?;
        records.remove(key);
        let mut ids = (*self.record_ids).clone();
        ids.remove(key);
        self.rebuild(records, ids, self.high_water, journal)?;
        Ok(old)
    }
    fn records(&self) -> Result<BTreeMap<Vec<u8>, Record>> {
        let mut records = BTreeMap::new();
        for p in self.pages.values() {
            if let Body::Tree {
                level: 0,
                records: rs,
                ..
            } = &p.body
            {
                for r in rs {
                    let mut r = r.clone();
                    r.value = resolve(&self.pages, self.owner, &self.record_ids, &r)?;
                    r.overflow = false;
                    if records.insert(r.key.clone(), r).is_some() {
                        return Err(Error::Corrupt("duplicate leaf key"));
                    }
                }
            }
        }
        Ok(records)
    }
    fn rebuild(
        &mut self,
        records: BTreeMap<Vec<u8>, Record>,
        ids: BTreeMap<Vec<u8>, u64>,
        high: u64,
        journal: &mut impl StructuralJournal,
    ) -> Result<()> {
        self.epoch.checked_add(1).ok_or(Error::Exhausted)?;
        let mut allocation = self.allocator.clone();
        allocation.reclaim()?;
        let mut pages = BTreeMap::new();
        let mut encoded = Vec::new();
        for (_, mut r) in records {
            if r.value.len() + r.key.len() + 36 > 8064 {
                let mut next = Address::default();
                let length = r.value.len() as u64;
                for chunk in r.value.rchunks(8096) {
                    let a = allocation.allocate(self.owner)?;
                    pages.insert(
                        a,
                        Page {
                            address: a,
                            owner: self.owner,
                            lsn: 0,
                            body: Body::Overflow {
                                next,
                                owner_record: ids[&r.key],
                                chunk: chunk.to_vec(),
                            },
                        },
                    );
                    next = a;
                }
                r.value = OverflowRef {
                    first: next,
                    length,
                }
                .encode()?;
                r.overflow = true;
            }
            encoded.push(r);
        }
        let leaf_groups = groups(encoded, self.config.fanout, false)?;
        let mut children = build_level(leaf_groups, 0, self.owner, &mut allocation, &mut pages)?;
        let mut level = 0u16;
        while children.len() > 1 {
            level = level.checked_add(1).ok_or(Error::Exhausted)?;
            // Partition children by both count and encoded separator size. A singleton
            // internal tail is folded into its predecessor where space permits.
            let mut sets: Vec<Vec<(Address, Vec<u8>)>> = Vec::new();
            let mut set = Vec::new();
            let mut bytes = 128;
            for child in children {
                let cost = 52 + child.1.len();
                if !set.is_empty() && (set.len() >= self.config.fanout || bytes + cost > 8192) {
                    sets.push(std::mem::take(&mut set));
                    bytes = 128;
                }
                if !set.is_empty() {
                    bytes += cost;
                }
                set.push(child);
            }
            if !set.is_empty() {
                sets.push(set);
            }
            if sets.len() > 1 && sets.last().unwrap().len() == 1 && sets[sets.len() - 2].len() > 2 {
                let index = sets.len() - 2;
                let child = sets[index].pop().unwrap();
                sets.last_mut().unwrap().insert(0, child);
            }
            let addresses: Vec<_> = sets
                .iter()
                .map(|_| allocation.allocate(self.owner))
                .collect::<Result<_>>()?;
            let mut next = Vec::new();
            for (i, set) in sets.into_iter().enumerate() {
                let a = addresses[i];
                let first = set[0].0;
                let min = set[0].1.clone();
                let records = set
                    .into_iter()
                    .skip(1)
                    .map(|(a, k)| Record::child(k, a))
                    .collect();
                let p = Page {
                    address: a,
                    lsn: 0,
                    owner: self.owner,
                    body: Body::Tree {
                        level,
                        left: i
                            .checked_sub(1)
                            .map_or(Address::default(), |i| addresses[i]),
                        right: addresses.get(i + 1).copied().unwrap_or_default(),
                        first_child: first,
                        high_key: None,
                        records,
                    },
                };
                p.encode()?;
                pages.insert(a, p);
                next.push((a, min));
            }
            children = next;
        }
        let root = children.first().map_or(Address::default(), |c| c.0);
        let retired = self.allocator.live(self.owner);
        for &a in &retired {
            allocation.retire(a, self.owner)?;
        }
        validate(&pages, root, self.owner, &ids)?;
        let action = StructuralAction {
            tree: self.owner,
            previous_root: (self.root.id, self.root.generation),
            root: (root.id, root.generation),
            pages: pages
                .values()
                .map(|p| {
                    Ok(PageImage {
                        id: p.address.id,
                        generation: p.address.generation,
                        bytes: p.encode()?.to_vec(),
                    })
                })
                .collect::<Result<_>>()?,
            allocation: allocation.encode(),
            retired: retired.iter().map(|a| (a.id, a.generation)).collect(),
            record_ids: ids.iter().map(|(k, id)| (k.clone(), *id)).collect(),
            identity_high_water: high,
        };
        let lsn = journal.seal(action)?;
        if lsn == 0 {
            return Err(Error::WalOrdering);
        }
        for page in pages.values_mut() {
            page.lsn = lsn;
        }
        self.root = root;
        self.pages = Arc::new(pages);
        self.allocator = allocation;
        self.record_ids = Arc::new(ids);
        self.high_water = high;
        self.epoch = lsn;
        Ok(())
    }
    /// Replays only sealed whole actions. Repeated latest-action replay is idempotent.
    pub fn replay(&mut self, lsn: u64, action: &StructuralAction) -> Result<()> {
        if lsn == 0
            || action.tree != self.owner
            || action.pages.len() > self.config.max_pages
            || action.record_ids.len() > self.config.max_records
        {
            return Err(Error::Corrupt("action scope"));
        }
        let root = Address::checked(action.root.0, action.root.1)?;
        let repeated = self.root == root && self.epoch == lsn;
        if !repeated && action.previous_root != (self.root.id, self.root.generation) {
            return Err(Error::Corrupt("root publication lineage"));
        }
        if action.identity_high_water < self.high_water {
            return Err(Error::Corrupt("record identity regression"));
        }
        if !repeated {
            let retired: BTreeSet<_> = action.retired.iter().copied().collect();
            if retired.len() != action.retired.len()
                || retired != self.pages.keys().map(|a| (a.id, a.generation)).collect()
            {
                return Err(Error::Corrupt("retired page set"));
            }
        }
        let allocation = self.allocator.restore(&action.allocation)?;
        let mut pages = BTreeMap::new();
        for p in &action.pages {
            let a = Address::checked(p.id, p.generation)?;
            allocation.check(a, self.owner)?;
            if !repeated && self.pages.keys().any(|old| old.id == a.id) {
                return Err(Error::Corrupt("overwrite live page"));
            }
            let mut page = Page::decode(&p.bytes, a, self.owner)?;
            page.lsn = lsn;
            if pages.insert(a, page).is_some() {
                return Err(Error::Corrupt("duplicate action page"));
            }
        }
        let mut ids = BTreeMap::new();
        let mut seen = BTreeSet::new();
        for (key, id) in &action.record_ids {
            if key.len() > MAX_KEY
                || (!repeated
                    && self
                        .record_ids
                        .get(key)
                        .map_or(*id <= self.high_water, |old| old != id))
                || *id == 0
                || *id > action.identity_high_water
                || !seen.insert(*id)
                || ids.insert(key.clone(), *id).is_some()
            {
                return Err(Error::Corrupt("record identity map"));
            }
        }
        validate(&pages, root, self.owner, &ids)?;
        if allocation
            .live(self.owner)
            .into_iter()
            .collect::<BTreeSet<_>>()
            != pages.keys().copied().collect()
        {
            return Err(Error::Corrupt("unreachable allocation"));
        }
        if repeated {
            if pages != *self.pages
                || ids != *self.record_ids
                || allocation.live(self.owner) != self.allocator.live(self.owner)
                || action.identity_high_water != self.high_water
            {
                return Err(Error::Corrupt("conflicting repeated action"));
            }
            return Ok(());
        }
        self.root = root;
        self.pages = Arc::new(pages);
        self.allocator = allocation;
        self.record_ids = Arc::new(ids);
        self.high_water = action.identity_high_water;
        self.epoch = lsn;
        Ok(())
    }
    pub fn validate(&self) -> Result<()> {
        validate(&self.pages, self.root, self.owner, &self.record_ids)?;
        for &a in self.pages.keys() {
            self.allocator.check(a, self.owner)?;
        }
        Ok(())
    }
    pub fn page_images(&self) -> Result<Vec<PageImage>> {
        self.pages
            .values()
            .map(|p| {
                Ok(PageImage {
                    id: p.address.id,
                    generation: p.address.generation,
                    bytes: p.encode()?.to_vec(),
                })
            })
            .collect()
    }
}
fn groups(records: Vec<Record>, fanout: usize, internal: bool) -> Result<Vec<Vec<Record>>> {
    let mut groups = Vec::new();
    let mut current = Vec::new();
    let mut bytes = 128;
    for record in records {
        let cost = record.encode(internal)?.len() + 4;
        if !current.is_empty() && (current.len() >= fanout || bytes + cost > 8192) {
            groups.push(std::mem::take(&mut current));
            bytes = 128;
        }
        if bytes + cost > 8192 {
            return Err(Error::Limit);
        }
        bytes += cost;
        current.push(record);
    }
    if !current.is_empty() {
        groups.push(current);
    }
    Ok(groups)
}
fn build_level(
    groups: Vec<Vec<Record>>,
    level: u16,
    owner: u64,
    allocator: &mut Allocator,
    pages: &mut BTreeMap<Address, Page>,
) -> Result<Vec<(Address, Vec<u8>)>> {
    let addresses: Vec<_> = groups
        .iter()
        .map(|_| allocator.allocate(owner))
        .collect::<Result<_>>()?;
    let mut children = Vec::new();
    for (i, records) in groups.into_iter().enumerate() {
        let a = addresses[i];
        let min = records[0].key.clone();
        let page = Page {
            address: a,
            lsn: 0,
            owner,
            body: Body::Tree {
                level,
                left: i
                    .checked_sub(1)
                    .map_or(Address::default(), |i| addresses[i]),
                right: addresses.get(i + 1).copied().unwrap_or_default(),
                first_child: Address::default(),
                high_key: None,
                records,
            },
        };
        page.encode()?;
        pages.insert(a, page);
        children.push((a, min));
    }
    Ok(children)
}
fn load(pages: &BTreeMap<Address, Page>, a: Address, owner: u64) -> Result<&Page> {
    let p = pages.get(&a).ok_or(Error::Stale)?;
    if p.address != a {
        return Err(Error::Stale);
    }
    if p.owner != owner {
        return Err(Error::Ownership);
    }
    Ok(p)
}
fn resolve(
    pages: &BTreeMap<Address, Page>,
    owner: u64,
    ids: &BTreeMap<Vec<u8>, u64>,
    record: &Record,
) -> Result<Vec<u8>> {
    if record.overflow {
        let id = ids
            .get(&record.key)
            .ok_or(Error::Corrupt("missing record identity"))?;
        OverflowRef::decode(&record.value)?.read(owner, *id, |a| Ok(load(pages, a, owner)?.clone()))
    } else {
        Ok(record.value.clone())
    }
}
fn lookup(
    pages: &BTreeMap<Address, Page>,
    mut a: Address,
    owner: u64,
    ids: &BTreeMap<Vec<u8>, u64>,
    key: &[u8],
) -> Result<Option<Vec<u8>>> {
    if a.is_null() {
        return Ok(None);
    }
    let mut seen = BTreeSet::new();
    loop {
        if !seen.insert(a) || seen.len() > pages.len() {
            return Err(Error::Corrupt("tree cycle"));
        }
        let p = load(pages, a, owner)?;
        let Body::Tree {
            level,
            first_child,
            records,
            ..
        } = &p.body
        else {
            return Err(Error::Corrupt("tree page kind"));
        };
        if *level == 0 {
            return records
                .binary_search_by(|r| r.key.as_slice().cmp(key))
                .ok()
                .map(|i| resolve(pages, owner, ids, &records[i]))
                .transpose();
        }
        let n = records.partition_point(|r| r.key.as_slice() <= key);
        a = if n == 0 {
            *first_child
        } else {
            records[n - 1].child_address()?
        };
    }
}
impl Snapshot {
    pub fn get(&self, key: &[u8]) -> Result<Option<Vec<u8>>> {
        lookup(&self.pages, self.root, self.owner, &self.record_ids, key)
    }
    pub fn scan(
        &self,
        start: Bound<&[u8]>,
        end: Bound<&[u8]>,
        descending: bool,
        resume: Option<&ScanPosition>,
        limit: usize,
    ) -> Result<Vec<KeyValue>> {
        if limit == 0 {
            return Ok(Vec::new());
        }
        if limit > 1_000_000 {
            return Err(Error::Limit);
        }
        if resume.is_some_and(|r| r.descending != descending) {
            return Err(Error::Corrupt("scan direction"));
        }
        let mut a = self.root;
        let mut seen = BTreeSet::new();
        if a.is_null() {
            return Ok(vec![]);
        }
        loop {
            if !seen.insert(a) {
                return Err(Error::Corrupt("scan descent cycle"));
            }
            let p = load(&self.pages, a, self.owner)?;
            let Body::Tree {
                level,
                first_child,
                records,
                ..
            } = &p.body
            else {
                return Err(Error::Corrupt("scan kind"));
            };
            if *level == 0 {
                break;
            }
            a = if descending {
                records
                    .last()
                    .map_or(Ok(*first_child), Record::child_address)?
            } else {
                *first_child
            };
        }
        seen.clear();
        let mut out = Vec::new();
        let mut previous: Option<Vec<u8>> = None;
        while !a.is_null() && out.len() < limit {
            if !seen.insert(a) {
                return Err(Error::Corrupt("sibling cycle"));
            }
            let Body::Tree {
                level: 0,
                left,
                right,
                records,
                ..
            } = &load(&self.pages, a, self.owner)?.body
            else {
                return Err(Error::Corrupt("scan leaf"));
            };
            let indexes: Box<dyn Iterator<Item = usize>> = if descending {
                Box::new((0..records.len()).rev())
            } else {
                Box::new(0..records.len())
            };
            for i in indexes {
                let r = &records[i];
                if previous
                    .as_ref()
                    .is_some_and(|p| if descending { p <= &r.key } else { p >= &r.key })
                {
                    return Err(Error::Corrupt("scan order"));
                }
                previous = Some(r.key.clone());
                let lower = match start {
                    Bound::Unbounded => true,
                    Bound::Included(k) => r.key.as_slice() >= k,
                    Bound::Excluded(k) => r.key.as_slice() > k,
                };
                let upper = match end {
                    Bound::Unbounded => true,
                    Bound::Included(k) => r.key.as_slice() <= k,
                    Bound::Excluded(k) => r.key.as_slice() < k,
                };
                let resumed = resume.is_none_or(|p| {
                    if descending {
                        r.key < p.key
                    } else {
                        r.key > p.key
                    }
                });
                if lower && upper && resumed {
                    out.push((
                        r.key.clone(),
                        resolve(&self.pages, self.owner, &self.record_ids, r)?,
                    ));
                    if out.len() == limit {
                        break;
                    }
                }
            }
            a = if descending { *left } else { *right };
        }
        Ok(out)
    }
}
/// Independent DFS walker verifies ownership, separators, depth, sibling reciprocity,
/// reachable overflow and exact key/record identity coverage.
fn validate(
    pages: &BTreeMap<Address, Page>,
    root: Address,
    owner: u64,
    ids: &BTreeMap<Vec<u8>, u64>,
) -> Result<()> {
    if root.is_null() {
        return if pages.is_empty() && ids.is_empty() {
            Ok(())
        } else {
            Err(Error::Corrupt("empty root with pages"))
        };
    }
    let mut seen = BTreeSet::new();
    let mut levels: BTreeMap<u16, Vec<Address>> = BTreeMap::new();
    let mut keys = BTreeSet::new();
    #[allow(clippy::too_many_arguments)] // Explicit independent walker state, no engine callbacks.
    fn walk(
        a: Address,
        pages: &BTreeMap<Address, Page>,
        owner: u64,
        ids: &BTreeMap<Vec<u8>, u64>,
        seen: &mut BTreeSet<Address>,
        levels: &mut BTreeMap<u16, Vec<Address>>,
        keys: &mut BTreeSet<Vec<u8>>,
        depth: usize,
    ) -> Result<(Vec<u8>, Vec<u8>, u16)> {
        if depth > 64 || !seen.insert(a) {
            return Err(Error::Corrupt("tree cycle or shared child"));
        }
        let p = load(pages, a, owner)?;
        p.encode()?;
        let Body::Tree {
            level,
            first_child,
            records,
            high_key,
            ..
        } = &p.body
        else {
            return Err(Error::Corrupt("walker page kind"));
        };
        if high_key.is_some() {
            return Err(Error::Corrupt("unexpected high key"));
        }
        levels.entry(*level).or_default().push(a);
        if *level == 0 {
            if records.is_empty() {
                return Err(Error::Corrupt("empty leaf"));
            }
            for r in records {
                if !ids.contains_key(&r.key) || !keys.insert(r.key.clone()) {
                    return Err(Error::Corrupt("leaf identities"));
                }
                resolve(pages, owner, ids, r)?;
                if r.overflow {
                    let mut a = OverflowRef::decode(&r.value)?.first;
                    while !a.is_null() {
                        if !seen.insert(a) {
                            return Err(Error::Corrupt("shared overflow"));
                        }
                        let p = load(pages, a, owner)?;
                        p.encode()?;
                        let Body::Overflow { next, .. } = &p.body else {
                            return Err(Error::Corrupt("overflow kind"));
                        };
                        a = *next;
                    }
                }
            }
            return Ok((
                records[0].key.clone(),
                records.last().unwrap().key.clone(),
                0,
            ));
        }
        if records.is_empty() {
            return Err(Error::Corrupt("underfull internal page"));
        }
        let (min, mut max, child_level) = walk(
            *first_child,
            pages,
            owner,
            ids,
            seen,
            levels,
            keys,
            depth + 1,
        )?;
        if child_level.checked_add(1) != Some(*level) {
            return Err(Error::Corrupt("unequal leaf depth"));
        }
        for record in records {
            let (cmin, cmax, cl) = walk(
                record.child_address()?,
                pages,
                owner,
                ids,
                seen,
                levels,
                keys,
                depth + 1,
            )?;
            if cl != child_level || max >= cmin || record.key != cmin {
                return Err(Error::Corrupt("bad separator"));
            }
            max = cmax;
        }
        Ok((min, max, *level))
    }
    walk(
        root,
        pages,
        owner,
        ids,
        &mut seen,
        &mut levels,
        &mut keys,
        0,
    )?;
    if seen.len() != pages.len() || keys != ids.keys().cloned().collect() {
        return Err(Error::Corrupt("unreachable pages or identities"));
    }
    for nodes in levels.values() {
        for (i, a) in nodes.iter().enumerate() {
            let Body::Tree { left, right, .. } = &pages[a].body else {
                unreachable!()
            };
            if *left != i.checked_sub(1).map_or(Address::default(), |i| nodes[i])
                || *right != nodes.get(i + 1).copied().unwrap_or_default()
            {
                return Err(Error::Corrupt("sibling ownership/order"));
            }
        }
    }
    Ok(())
}
