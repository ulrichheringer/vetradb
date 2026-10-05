use crate::{Error, Result};
use std::collections::BTreeSet;
pub const PAGE_SIZE: usize = 8192;
pub const MAX_VALUE: usize = 16 * 1024 * 1024;
pub type Image = [u8; PAGE_SIZE];
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord)]
pub struct Address {
    pub id: u64,
    pub generation: u64,
}
impl Address {
    pub fn checked(id: u64, generation: u64) -> Result<Self> {
        if (id == 0 && generation == 0) || (id >= 2 && generation > 0) {
            Ok(Self { id, generation })
        } else {
            Err(Error::Corrupt("page address"))
        }
    }
    pub fn is_null(self) -> bool {
        self == Self::default()
    }
}
const fn crc_table() -> [u32; 256] {
    let mut table = [0; 256];
    let mut i = 0;
    while i < 256 {
        let mut value = i as u32;
        let mut bit = 0;
        while bit < 8 {
            value = (value >> 1) ^ (0x82f63b78 & 0u32.wrapping_sub(value & 1));
            bit += 1;
        }
        table[i] = value;
        i += 1;
    }
    table
}
const CRC_TABLE: [u32; 256] = crc_table();
pub fn crc32c(bytes: &[u8]) -> u32 {
    let mut crc = !0u32;
    for &byte in bytes {
        crc = (crc >> 8) ^ CRC_TABLE[((crc as u8) ^ byte) as usize];
    }
    !crc
}

fn checksum(bytes: &mut Image, offset: usize) {
    bytes[offset..offset + 4].fill(0);
    let crc = crc32c(bytes);
    bytes[offset..offset + 4].copy_from_slice(&crc.to_le_bytes());
}
fn verified(input: &[u8], offset: usize) -> Result<Image> {
    let mut bytes: Image = input
        .try_into()
        .map_err(|_| Error::Corrupt("page length"))?;
    let expected = u32at(&bytes, offset);
    bytes[offset..offset + 4].fill(0);
    if crc32c(&bytes) != expected {
        return Err(Error::Corrupt("checksum"));
    }
    Ok(input.try_into().unwrap())
}
fn u16at(b: &[u8], o: usize) -> u16 {
    u16::from_le_bytes(b[o..o + 2].try_into().unwrap())
}
fn u32at(b: &[u8], o: usize) -> u32 {
    u32::from_le_bytes(b[o..o + 4].try_into().unwrap())
}
fn u64at(b: &[u8], o: usize) -> u64 {
    u64::from_le_bytes(b[o..o + 8].try_into().unwrap())
}
fn put16(b: &mut [u8], o: usize, v: u16) {
    b[o..o + 2].copy_from_slice(&v.to_le_bytes());
}
fn put32(b: &mut [u8], o: usize, v: u32) {
    b[o..o + 4].copy_from_slice(&v.to_le_bytes());
}
fn put64(b: &mut [u8], o: usize, v: u64) {
    b[o..o + 8].copy_from_slice(&v.to_le_bytes());
}
fn address(b: &[u8], o: usize) -> Result<Address> {
    Address::checked(u64at(b, o), u64at(b, o + 8))
}
fn put_address(b: &mut [u8], o: usize, a: Address) {
    put64(b, o, a.id);
    put64(b, o + 8, a.generation);
}
fn zero(b: &[u8]) -> Result<()> {
    if b.iter().any(|v| *v != 0) {
        Err(Error::Corrupt("reserved bytes"))
    } else {
        Ok(())
    }
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Superblock {
    pub database: [u8; 16],
    pub timeline: [u8; 16],
    pub generation: u64,
    pub checkpoint: u64,
    pub wal_start: u64,
    pub catalog: Address,
    pub allocator: Address,
    pub identity_high_water: u64,
}
impl Superblock {
    pub fn encode(&self) -> Result<Image> {
        if self.database == [0; 16] || self.timeline == [0; 16] || self.generation == 0 {
            return Err(Error::Corrupt("superblock identity"));
        }
        Address::checked(self.catalog.id, self.catalog.generation)?;
        Address::checked(self.allocator.id, self.allocator.generation)?;
        let mut b = [0; PAGE_SIZE];
        b[..8].copy_from_slice(b"VETRASB1");
        put16(&mut b, 8, 1);
        put16(&mut b, 10, 128);
        put32(&mut b, 12, PAGE_SIZE as u32);
        b[16..32].copy_from_slice(&self.database);
        b[32..48].copy_from_slice(&self.timeline);
        put64(&mut b, 48, self.generation);
        put64(&mut b, 56, self.checkpoint);
        put64(&mut b, 64, self.wal_start);
        put_address(&mut b, 72, self.catalog);
        put_address(&mut b, 88, self.allocator);
        put64(&mut b, 104, self.identity_high_water);
        checksum(&mut b, 116);
        Ok(b)
    }
    pub fn decode(input: &[u8]) -> Result<Self> {
        let b = verified(input, 116)?;
        if &b[..8] != b"VETRASB1" {
            return Err(Error::Corrupt("superblock magic"));
        }
        if u16at(&b, 8) != 1 {
            return Err(Error::UnsupportedVersion);
        }
        if u16at(&b, 10) != 128 || u32at(&b, 12) != PAGE_SIZE as u32 {
            return Err(Error::Corrupt("superblock dimensions"));
        }
        zero(&b[112..116])?;
        zero(&b[120..])?;
        let s = Self {
            database: b[16..32].try_into().unwrap(),
            timeline: b[32..48].try_into().unwrap(),
            generation: u64at(&b, 48),
            checkpoint: u64at(&b, 56),
            wal_start: u64at(&b, 64),
            catalog: address(&b, 72)?,
            allocator: address(&b, 88)?,
            identity_high_water: u64at(&b, 104),
        };
        s.encode()?;
        Ok(s)
    }
    /// Required WAL coverage is checked by the recovery caller before opening.
    pub fn select(a: &[u8], b: &[u8], retained_start: u64) -> Result<Self> {
        let a = Self::decode(a);
        let b = Self::decode(b);
        // A checksummed future format is not an incomplete alternate write.
        if a == Err(Error::UnsupportedVersion) || b == Err(Error::UnsupportedVersion) {
            return Err(Error::UnsupportedVersion);
        }
        let selected = match (a, b) {
            (Ok(a), Ok(b)) => {
                if a.database != b.database
                    || a.timeline != b.timeline
                    || (a.generation == b.generation && a != b)
                {
                    return Err(Error::Corrupt("conflicting superblocks"));
                }
                if a.generation >= b.generation { a } else { b }
            }
            (Ok(a), Err(_)) | (Err(_), Ok(a)) => a,
            _ => return Err(Error::Corrupt("both superblocks invalid")),
        };
        if retained_start > selected.wal_start {
            return Err(Error::Corrupt("required WAL recycled"));
        }
        Ok(selected)
    }
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Record {
    pub key: Vec<u8>,
    pub value: Vec<u8>,
    pub creator: u64,
    pub schema: u64,
    pub overflow: bool,
}
impl Record {
    pub fn encode(&self, internal: bool) -> Result<Vec<u8>> {
        let len = 32usize
            .checked_add(self.key.len())
            .and_then(|n| n.checked_add(self.value.len()))
            .ok_or(Error::Limit)?;
        if len > 8060
            || (internal
                && (self.creator != 0
                    || self.schema != 0
                    || self.overflow
                    || self.value.len() != 16))
            || (!internal && (self.creator == 0 || self.schema == 0))
        {
            return Err(Error::Corrupt("record fields"));
        }
        if self.overflow {
            OverflowRef::decode(&self.value)?;
        }
        if internal {
            let child = address(&self.value, 0)?;
            if child.is_null() {
                return Err(Error::Corrupt("null child"));
            }
        }
        let mut b = vec![0; len];
        put32(&mut b, 0, self.key.len() as u32);
        put32(&mut b, 4, self.value.len() as u32);
        put64(&mut b, 8, self.creator);
        put64(&mut b, 16, self.schema);
        put16(&mut b, 24, 1);
        put16(&mut b, 26, u16::from(self.overflow));
        b[32..32 + self.key.len()].copy_from_slice(&self.key);
        b[32 + self.key.len()..].copy_from_slice(&self.value);
        Ok(b)
    }
    pub fn decode(b: &[u8], internal: bool) -> Result<Self> {
        if b.len() < 32 {
            return Err(Error::Corrupt("short record"));
        }
        let k = u32at(b, 0) as usize;
        let v = u32at(b, 4) as usize;
        if 32usize.checked_add(k).and_then(|n| n.checked_add(v)) != Some(b.len())
            || u16at(b, 24) != 1
            || u16at(b, 26) > 1
        {
            return Err(Error::Corrupt("record framing"));
        }
        zero(&b[28..32])?;
        let record = Self {
            key: b[32..32 + k].to_vec(),
            value: b[32 + k..].to_vec(),
            creator: u64at(b, 8),
            schema: u64at(b, 16),
            overflow: u16at(b, 26) == 1,
        };
        record.encode(internal)?;
        Ok(record)
    }
    pub fn child(key: Vec<u8>, child: Address) -> Self {
        let mut value = vec![0; 16];
        put_address(&mut value, 0, child);
        Self {
            key,
            value,
            creator: 0,
            schema: 0,
            overflow: false,
        }
    }
    pub fn child_address(&self) -> Result<Address> {
        if self.value.len() != 16 {
            return Err(Error::Corrupt("child length"));
        }
        address(&self.value, 0)
    }
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Body {
    Tree {
        level: u16,
        left: Address,
        right: Address,
        first_child: Address,
        high_key: Option<u16>,
        records: Vec<Record>,
    },
    Overflow {
        next: Address,
        owner_record: u64,
        chunk: Vec<u8>,
    },
    Allocation {
        base: u64,
        generations: Vec<u64>,
    },
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Page {
    pub address: Address,
    pub lsn: u64,
    pub owner: u64,
    pub body: Body,
}
impl Page {
    pub fn encode(&self) -> Result<Image> {
        Address::checked(self.address.id, self.address.generation)?;
        if self.address.is_null() || self.owner == 0 {
            return Err(Error::Corrupt("page identity"));
        }
        let mut b = [0; PAGE_SIZE];
        b[..4].copy_from_slice(b"VPG1");
        put16(&mut b, 4, 1);
        put_address(&mut b, 8, self.address);
        put64(&mut b, 24, self.lsn);
        put64(&mut b, 32, self.owner);
        match &self.body {
            Body::Tree {
                level,
                left,
                right,
                first_child,
                high_key,
                records,
            } => {
                for a in [left, right, first_child] {
                    Address::checked(a.id, a.generation)?;
                }
                if (*level == 0) != first_child.is_null()
                    || high_key.is_some_and(|s| s as usize >= records.len())
                {
                    return Err(Error::Corrupt("tree special"));
                }
                put16(&mut b, 6, if *level == 0 { 1 } else { 2 });
                put_address(&mut b, 64, *left);
                put_address(&mut b, 80, *right);
                put_address(&mut b, 96, *first_child);
                put16(&mut b, 112, *level);
                put16(&mut b, 114, high_key.unwrap_or(u16::MAX));
                let lower = 128usize
                    .checked_add(records.len().checked_mul(4).ok_or(Error::Limit)?)
                    .ok_or(Error::Limit)?;
                if lower > PAGE_SIZE {
                    return Err(Error::Limit);
                }
                let mut upper = PAGE_SIZE;
                let mut previous: Option<&[u8]> = None;
                for (i, record) in records.iter().enumerate() {
                    if Some(i as u16) != *high_key {
                        if previous.is_some_and(|p| p >= record.key.as_slice()) {
                            return Err(Error::Corrupt("key order"));
                        }
                        previous = Some(&record.key);
                    }
                    let bytes = record.encode(*level > 0)?;
                    upper = upper.checked_sub(bytes.len()).ok_or(Error::Limit)?;
                    if upper < lower {
                        return Err(Error::Limit);
                    }
                    b[upper..upper + bytes.len()].copy_from_slice(&bytes);
                    put16(&mut b, 128 + 4 * i, upper as u16);
                    put16(&mut b, 130 + 4 * i, bytes.len() as u16);
                }
                put16(&mut b, 40, lower as u16);
                put16(&mut b, 42, upper as u16);
                put16(&mut b, 44, records.len() as u16);
            }
            Body::Overflow {
                next,
                owner_record,
                chunk,
            } => {
                Address::checked(next.id, next.generation)?;
                if *owner_record == 0 || chunk.len() > 8096 {
                    return Err(Error::Corrupt("overflow fields"));
                }
                put16(&mut b, 6, 3);
                put16(&mut b, 40, 96);
                put16(&mut b, 42, (96 + chunk.len()) as u16);
                put_address(&mut b, 64, *next);
                put64(&mut b, 80, *owner_record);
                put32(&mut b, 88, chunk.len() as u32);
                b[96..96 + chunk.len()].copy_from_slice(chunk);
            }
            Body::Allocation { base, generations } => {
                if self.owner != 1
                    || *base < 2
                    || generations.len() > 1014
                    || base.checked_add(generations.len() as u64).is_none()
                {
                    return Err(Error::Corrupt("allocation fields"));
                }
                put16(&mut b, 6, 4);
                put16(&mut b, 40, 80);
                put16(&mut b, 42, (80 + 8 * generations.len()) as u16);
                put64(&mut b, 64, *base);
                put32(&mut b, 72, generations.len() as u32);
                for (i, g) in generations.iter().enumerate() {
                    put64(&mut b, 80 + 8 * i, *g);
                }
            }
        }
        checksum(&mut b, 48);
        Ok(b)
    }
    pub fn decode(input: &[u8], expected: Address, owner: u64) -> Result<Self> {
        let b = verified(input, 48)?;
        if &b[..4] != b"VPG1" {
            return Err(Error::Corrupt("page magic"));
        }
        if u16at(&b, 4) != 1 {
            return Err(Error::UnsupportedVersion);
        }
        let a = address(&b, 8)?;
        if a != expected {
            return Err(Error::Stale);
        }
        if a.is_null() || owner == 0 || u64at(&b, 32) != owner {
            return Err(Error::Ownership);
        }
        zero(&b[46..48])?;
        zero(&b[52..64])?;
        let lower = u16at(&b, 40) as usize;
        let upper = u16at(&b, 42) as usize;
        let count = u16at(&b, 44) as usize;
        if lower > PAGE_SIZE || upper > PAGE_SIZE {
            return Err(Error::Corrupt("page bounds"));
        }
        let body = match u16at(&b, 6) {
            kind @ (1 | 2) => {
                if lower != 128 + 4 * count || lower > upper {
                    return Err(Error::Corrupt("slot directory"));
                }
                zero(&b[116..128])?;
                let level = u16at(&b, 112);
                if (kind == 1) != (level == 0) {
                    return Err(Error::Corrupt("page level"));
                }
                let high = u16at(&b, 114);
                let high_key = if high == u16::MAX { None } else { Some(high) };
                let mut occupied = vec![false; PAGE_SIZE - upper];
                let mut records = Vec::with_capacity(count);
                for i in 0..count {
                    let o = u16at(&b, 128 + 4 * i) as usize;
                    let n = u16at(&b, 130 + 4 * i) as usize;
                    let end = o.checked_add(n).ok_or(Error::Corrupt("slot overflow"))?;
                    if o < upper || end > PAGE_SIZE || n < 32 {
                        return Err(Error::Corrupt("slot bounds"));
                    }
                    for used in &mut occupied[o - upper..end - upper] {
                        if *used {
                            return Err(Error::Corrupt("overlapping slots"));
                        }
                        *used = true;
                    }
                    records.push(Record::decode(&b[o..end], level > 0)?);
                }
                zero(&b[lower..upper])?;
                for (i, used) in occupied.iter().enumerate() {
                    if !used && b[upper + i] != 0 {
                        return Err(Error::Corrupt("unused record space"));
                    }
                }
                Body::Tree {
                    level,
                    left: address(&b, 64)?,
                    right: address(&b, 80)?,
                    first_child: address(&b, 96)?,
                    high_key,
                    records,
                }
            }
            3 => {
                let n = u32at(&b, 88) as usize;
                if n > 8096 || lower != 96 || upper != 96 + n || count != 0 {
                    return Err(Error::Corrupt("overflow bounds"));
                }
                zero(&b[92..96])?;
                zero(&b[upper..])?;
                Body::Overflow {
                    next: address(&b, 64)?,
                    owner_record: u64at(&b, 80),
                    chunk: b[96..upper].to_vec(),
                }
            }
            4 => {
                let n = u32at(&b, 72) as usize;
                if n > 1014 || lower != 80 || upper != 80 + 8 * n || count != 0 {
                    return Err(Error::Corrupt("allocation bounds"));
                }
                zero(&b[76..80])?;
                zero(&b[upper..])?;
                Body::Allocation {
                    base: u64at(&b, 64),
                    generations: (0..n).map(|i| u64at(&b, 80 + 8 * i)).collect(),
                }
            }
            _ => return Err(Error::Corrupt("page kind")),
        };
        let page = Self {
            address: a,
            lsn: u64at(&b, 24),
            owner,
            body,
        };
        page.encode()?;
        Ok(page)
    }
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct OverflowRef {
    pub first: Address,
    pub length: u64,
}
impl OverflowRef {
    pub fn encode(self) -> Result<Vec<u8>> {
        Address::checked(self.first.id, self.first.generation)?;
        if self.length > MAX_VALUE as u64 || self.first.is_null() {
            return Err(Error::Corrupt("overflow descriptor"));
        }
        let mut b = vec![0; 24];
        put_address(&mut b, 0, self.first);
        put64(&mut b, 16, self.length);
        Ok(b)
    }
    pub fn decode(b: &[u8]) -> Result<Self> {
        if b.len() != 24 {
            return Err(Error::Corrupt("overflow descriptor length"));
        }
        let s = Self {
            first: address(b, 0)?,
            length: u64at(b, 16),
        };
        s.encode()?;
        Ok(s)
    }
    pub fn read(
        self,
        owner: u64,
        record: u64,
        mut load: impl FnMut(Address) -> Result<Page>,
    ) -> Result<Vec<u8>> {
        self.encode()?;
        let mut seen = BTreeSet::new();
        let mut a = self.first;
        let mut bytes = Vec::new();
        while !a.is_null() {
            if seen.len() >= 2073 || !seen.insert(a) {
                return Err(Error::Corrupt("overflow cycle or budget"));
            }
            let p = load(a)?;
            if p.address != a {
                return Err(Error::Stale);
            }
            if p.owner != owner {
                return Err(Error::Ownership);
            }
            let Body::Overflow {
                next,
                owner_record,
                chunk,
            } = p.body
            else {
                return Err(Error::Corrupt("overflow kind"));
            };
            if owner_record != record
                || chunk.len() > 8096
                || chunk.is_empty()
                || bytes
                    .len()
                    .checked_add(chunk.len())
                    .is_none_or(|n| n > self.length as usize)
            {
                return Err(Error::Corrupt("overflow identity or length"));
            }
            bytes.extend(chunk);
            a = next;
        }
        if bytes.len() != self.length as usize {
            return Err(Error::Corrupt("truncated overflow"));
        }
        Ok(bytes)
    }
}
/// Versioned scalar framing, deliberately separate from SQL order/comparison codecs.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Value {
    Null,
    Bytes(Vec<u8>),
    Text(String),
    U64(u64),
    I64(i64),
    Bool(bool),
}
impl Value {
    pub fn encode(&self) -> Result<Vec<u8>> {
        let (tag, data) = match self {
            Self::Null => (0, vec![]),
            Self::Bytes(b) => (1, b.clone()),
            Self::Text(s) => (2, s.as_bytes().to_vec()),
            Self::U64(n) => (3, n.to_le_bytes().to_vec()),
            Self::I64(n) => (4, n.to_le_bytes().to_vec()),
            Self::Bool(b) => (5, vec![u8::from(*b)]),
        };
        if data.len() > MAX_VALUE {
            return Err(Error::Limit);
        }
        let mut out = vec![1, tag, 0, 0];
        out.extend((data.len() as u32).to_le_bytes());
        out.extend(data);
        Ok(out)
    }
    pub fn decode(b: &[u8]) -> Result<Self> {
        if b.len() < 8 || b[0] != 1 || b[2..4] != [0, 0] {
            return Err(Error::Corrupt("value header"));
        }
        let n = u32at(b, 4) as usize;
        if n > MAX_VALUE || n.checked_add(8) != Some(b.len()) {
            return Err(Error::Corrupt("value length"));
        }
        let v = &b[8..];
        match b[1] {
            0 if n == 0 => Ok(Self::Null),
            1 => Ok(Self::Bytes(v.to_vec())),
            2 => Ok(Self::Text(
                std::str::from_utf8(v)
                    .map_err(|_| Error::Corrupt("UTF-8"))?
                    .to_owned(),
            )),
            3 if n == 8 => Ok(Self::U64(u64at(v, 0))),
            4 if n == 8 => Ok(Self::I64(i64::from_le_bytes(v.try_into().unwrap()))),
            5 if n == 1 && v[0] <= 1 => Ok(Self::Bool(v[0] == 1)),
            _ => Err(Error::Corrupt("value tag")),
        }
    }
}
/// Stable logical record IDs map to slots; compaction retains IDs and slot order.
#[derive(Clone, Debug, Default)]
pub struct SlottedRecords {
    slots: Vec<Option<(u64, Record)>>,
    identities: BTreeSet<u64>,
}
impl SlottedRecords {
    pub fn insert(&mut self, id: u64, record: Record) -> Result<usize> {
        if id == 0 || self.identities.contains(&id) {
            return Err(Error::Duplicate);
        }
        record.encode(false)?;
        let slot = self.slots.len();
        if slot >= 2016 {
            return Err(Error::Limit);
        }
        self.identities.insert(id);
        self.slots.push(Some((id, record)));
        Ok(slot)
    }
    pub fn get(&self, id: u64) -> Result<&Record> {
        self.slots
            .iter()
            .flatten()
            .find(|(i, _)| *i == id)
            .map(|(_, r)| r)
            .ok_or(Error::Stale)
    }
    pub fn remove(&mut self, id: u64) -> Result<()> {
        let slot = self
            .slots
            .iter_mut()
            .find(|s| s.as_ref().is_some_and(|(i, _)| *i == id))
            .ok_or(Error::Stale)?;
        *slot = None;
        Ok(())
    }
    pub fn compact(&self) -> Vec<(u64, Record)> {
        self.slots.iter().flatten().cloned().collect()
    }
}
