//! Immutable, bounded format-v1 logical envelopes for internal participants.
use crate::{Error, Kind, Lineage, Record, Result, codec::*};
use std::collections::BTreeMap;
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Value {
    Null,
    Bytes(Vec<u8>),
    Text(String),
    U64(u64),
    I64(i64),
    Bool(bool),
}
pub type Image = BTreeMap<u64, Value>;
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
#[repr(u8)]
pub enum Participant {
    Row = 1,
    Schema = 2,
    Job = 3,
    Schedule = 4,
    Event = 5,
    Offset = 6,
    Permission = 7,
}
impl TryFrom<u8> for Participant {
    type Error = Error;
    fn try_from(n: u8) -> Result<Self> {
        Ok(match n {
            1 => Self::Row,
            2 => Self::Schema,
            3 => Self::Job,
            4 => Self::Schedule,
            5 => Self::Event,
            6 => Self::Offset,
            7 => Self::Permission,
            _ => return Err(Error::Corrupt("participant")),
        })
    }
}
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct Key {
    pub participant: Participant,
    pub object: u64,
    pub bytes: Vec<u8>,
}
impl Key {
    pub fn validate(&self) -> Result<()> {
        if self.object == 0 || self.bytes.is_empty() || self.bytes.len() > 2048 {
            return Err(Error::Limit);
        }
        Ok(())
    }
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Operation {
    pub key: Key,
    pub schema: u64,
    pub before: Option<Image>,
    pub after: Option<Image>,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Envelope {
    pub lineage: Lineage,
    pub tx: u64,
    pub csn: u64,
    pub timestamp: i64,
    pub principal: u64,
    pub metadata: BTreeMap<String, String>,
    pub operations: Vec<Operation>,
}
struct Cursor<'a> {
    b: &'a [u8],
    o: usize,
}
impl<'a> Cursor<'a> {
    fn take(&mut self, n: usize) -> Result<&'a [u8]> {
        let end = self.o.checked_add(n).ok_or(Error::Limit)?;
        let b = self
            .b
            .get(self.o..end)
            .ok_or(Error::Corrupt("truncated envelope"))?;
        self.o = end;
        Ok(b)
    }
    fn end(&self) -> Result<()> {
        if self.o != self.b.len() {
            return Err(Error::Corrupt("trailing envelope bytes"));
        }
        Ok(())
    }
}
pub fn encode_image(image: &Image) -> Result<Vec<u8>> {
    if image.len() > 4096 {
        return Err(Error::Limit);
    }
    let mut b = vec![0; 4];
    put16(&mut b, 0, image.len() as u16);
    for (&id, value) in image {
        if id == 0 {
            return Err(Error::Corrupt("field identity"));
        }
        let (tag, data) = match value {
            Value::Null => (0, vec![]),
            Value::Bytes(b) => (1, b.clone()),
            Value::Text(s) => (2, s.as_bytes().to_vec()),
            Value::U64(n) => (3, n.to_le_bytes().to_vec()),
            Value::I64(n) => (4, n.to_le_bytes().to_vec()),
            Value::Bool(v) => (5, vec![u8::from(*v)]),
        };
        if b.len()
            .checked_add(16 + data.len())
            .is_none_or(|n| n > MAX_ENVELOPE)
        {
            return Err(Error::Limit);
        }
        b.extend(id.to_le_bytes());
        b.push(tag);
        b.extend([0; 3]);
        b.extend((data.len() as u32).to_le_bytes());
        b.extend(data);
    }
    Ok(b)
}
pub fn decode_image(b: &[u8]) -> Result<Image> {
    let mut c = Cursor { b, o: 0 };
    let h = c.take(4)?;
    let n = u16at(h, 0);
    if n > 4096 || h[2..4] != [0; 2] {
        return Err(Error::Corrupt("image header"));
    }
    let mut image = Image::new();
    let mut previous = 0;
    for _ in 0..n {
        let h = c.take(16)?;
        let id = u64at(h, 0);
        if id <= previous || h[9..12] != [0; 3] {
            return Err(Error::Corrupt("field ordering"));
        }
        previous = id;
        let data = c.take(u32at(h, 12) as usize)?;
        let value = match h[8] {
            0 if data.is_empty() => Value::Null,
            1 => Value::Bytes(data.to_vec()),
            2 => Value::Text(
                std::str::from_utf8(data)
                    .map_err(|_| Error::Corrupt("UTF8"))?
                    .to_owned(),
            ),
            3 if data.len() == 8 => Value::U64(u64at(data, 0)),
            4 if data.len() == 8 => Value::I64(i64::from_le_bytes(data.try_into().unwrap())),
            5 if data.len() == 1 && data[0] <= 1 => Value::Bool(data[0] == 1),
            _ => return Err(Error::Corrupt("value codec")),
        };
        image.insert(id, value);
    }
    c.end()?;
    Ok(image)
}
impl Envelope {
    pub fn encode(&self) -> Result<Vec<u8>> {
        self.lineage.validate()?;
        if self.tx == 0
            || self.csn == 0
            || self.principal == 0
            || self.operations.len() > 65535
            || self.metadata.len() > 64
        {
            return Err(Error::Limit);
        }
        let mut meta = vec![0; 4];
        put16(&mut meta, 0, self.metadata.len() as u16);
        for (k, v) in &self.metadata {
            if k.is_empty() || k.len() > 128 || v.len() > 4096 || k.starts_with("vetra.") {
                return Err(Error::Limit);
            }
            meta.extend((k.len() as u16).to_le_bytes());
            meta.extend((v.len() as u16).to_le_bytes());
            meta.extend(k.bytes());
            meta.extend(v.bytes());
        }
        if meta.len() > 16384 {
            return Err(Error::Limit);
        }
        let mut b = vec![0; 96];
        b[..8].copy_from_slice(b"VENV0001");
        put16(&mut b, 8, 1);
        put16(&mut b, 10, 96);
        b[16..32].copy_from_slice(&self.lineage.database);
        b[32..48].copy_from_slice(&self.lineage.timeline);
        put64(&mut b, 48, self.tx);
        put64(&mut b, 56, self.csn);
        put64(&mut b, 64, self.timestamp as u64);
        put32(&mut b, 72, self.operations.len() as u32);
        put32(&mut b, 76, meta.len() as u32);
        put64(&mut b, 80, self.principal);
        b.extend(meta);
        for (position, op) in self.operations.iter().enumerate() {
            op.key.validate()?;
            if matches!(op.key.participant, Participant::Row | Participant::Schema)
                != (op.schema != 0)
                || op.before.is_none() && op.after.is_none()
            {
                return Err(Error::Corrupt("operation images/schema"));
            }
            let before = op
                .before
                .as_ref()
                .map(encode_image)
                .transpose()?
                .unwrap_or_default();
            let after = op
                .after
                .as_ref()
                .map(encode_image)
                .transpose()?
                .unwrap_or_default();
            let payload = 8 + before.len() + after.len();
            if b.len()
                .checked_add(32 + op.key.bytes.len() + payload)
                .is_none_or(|n| n > MAX_ENVELOPE)
            {
                return Err(Error::Limit);
            }
            let mut h = [0; 32];
            put32(&mut h, 0, position as u32);
            h[4] = op.key.participant as u8;
            h[5] = if op.after.is_some() { 1 } else { 2 };
            put16(&mut h, 6, 1);
            put64(&mut h, 8, op.key.object);
            put64(&mut h, 16, op.schema);
            put32(&mut h, 24, op.key.bytes.len() as u32);
            put32(&mut h, 28, payload as u32);
            b.extend(h);
            b.extend(&op.key.bytes);
            b.extend((before.len() as u32).to_le_bytes());
            b.extend((after.len() as u32).to_le_bytes());
            b.extend(before);
            b.extend(after);
        }
        let n = b.len() as u32;
        put32(&mut b, 12, n);
        checksum(&mut b, 88);
        Ok(b)
    }
    pub fn decode(b: &[u8], lineage: Lineage) -> Result<Self> {
        if b.len() < 96
            || b.len() > MAX_ENVELOPE
            || &b[..8] != b"VENV0001"
            || u16at(b, 8) != 1
            || u16at(b, 10) != 96
            || u32at(b, 12) as usize != b.len()
            || b[92..96] != [0; 4]
        {
            return Err(Error::Corrupt("envelope header"));
        }
        verified(b, 88)?;
        if b[16..32] != lineage.database || b[32..48] != lineage.timeline {
            return Err(Error::Corrupt("envelope lineage"));
        }
        let count = u32at(b, 72);
        let meta_len = u32at(b, 76) as usize;
        if count > 65535 || meta_len > 16384 {
            return Err(Error::Limit);
        }
        let mut c = Cursor { b, o: 96 };
        let mut m = Cursor {
            b: c.take(meta_len)?,
            o: 0,
        };
        let h = m.take(4)?;
        if u16at(h, 0) > 64 || h[2..4] != [0; 2] {
            return Err(Error::Corrupt("metadata header"));
        }
        let mut metadata = BTreeMap::new();
        let mut prev = String::new();
        for _ in 0..u16at(h, 0) {
            let h = m.take(4)?;
            let k = std::str::from_utf8(m.take(u16at(h, 0) as usize)?)
                .map_err(|_| Error::Corrupt("metadata UTF8"))?
                .to_owned();
            let v = std::str::from_utf8(m.take(u16at(h, 2) as usize)?)
                .map_err(|_| Error::Corrupt("metadata UTF8"))?
                .to_owned();
            if k <= prev || k.len() > 128 || v.len() > 4096 || k.starts_with("vetra.") {
                return Err(Error::Corrupt("metadata order/reserved"));
            }
            prev = k.clone();
            metadata.insert(k, v);
        }
        m.end()?;
        let mut operations = Vec::new();
        for pos in 0..count {
            let h = c.take(32)?;
            if u32at(h, 0) != pos || u16at(h, 6) != 1 || ![1, 2].contains(&h[5]) {
                return Err(Error::Corrupt("operation codec/position"));
            }
            let key = Key {
                participant: Participant::try_from(h[4])?,
                object: u64at(h, 8),
                bytes: c.take(u32at(h, 24) as usize)?.to_vec(),
            };
            key.validate()?;
            let p = c.take(u32at(h, 28) as usize)?;
            let mut pc = Cursor { b: p, o: 0 };
            let ph = pc.take(8)?;
            let before = pc.take(u32at(ph, 0) as usize)?;
            let after = pc.take(u32at(ph, 4) as usize)?;
            pc.end()?;
            if (h[5] == 1) == after.is_empty() || before.is_empty() && after.is_empty() {
                return Err(Error::Corrupt("operation action"));
            }
            operations.push(Operation {
                key,
                schema: u64at(h, 16),
                before: if before.is_empty() {
                    None
                } else {
                    Some(decode_image(before)?)
                },
                after: if after.is_empty() {
                    None
                } else {
                    Some(decode_image(after)?)
                },
            });
        }
        c.end()?;
        let e = Self {
            lineage,
            tx: u64at(b, 48),
            csn: u64at(b, 56),
            timestamp: u64at(b, 64) as i64,
            principal: u64at(b, 80),
            metadata,
            operations,
        };
        e.encode()?;
        Ok(e)
    }
}
/// Validate complete commit/envelope pairs before any visibility publication.
pub fn committed(records: &[Record], lineage: Lineage) -> Result<Vec<Envelope>> {
    let mut chunks: BTreeMap<u64, Vec<&Record>> = BTreeMap::new();
    let mut finished = BTreeMap::new();
    let mut envelopes = Vec::new();
    let mut previous_csn = 0;
    for record in records {
        if finished.contains_key(&record.tx)
            && matches!(record.kind, Kind::Envelope | Kind::Commit | Kind::Patch)
        {
            return Err(Error::Corrupt("mutation after decision"));
        }
        match record.kind {
            Kind::End if !finished.contains_key(&record.tx) => {
                return Err(Error::Corrupt("END without commit/abort decision"));
            }
            Kind::Envelope => {
                let pending = chunks.entry(record.tx).or_default();
                if pending.len() >= 17 {
                    return Err(Error::Corrupt("too many chunks"));
                }
                pending.push(record);
            }
            Kind::Abort => {
                finished.insert(record.tx, false);
            }
            Kind::Commit => {
                let list = chunks
                    .remove(&record.tx)
                    .ok_or(Error::Corrupt("missing envelope"))?;
                let total = u32at(&record.payload, 8) as usize;
                if u64at(&record.payload, 0) <= previous_csn
                    || list.first().map(|r| r.lsn) != Some(u64at(&record.payload, 16))
                {
                    return Err(Error::Corrupt("commit order/chunk link"));
                }
                let mut bytes = Vec::with_capacity(total);
                for (i, r) in list.iter().enumerate() {
                    let p = &r.payload;
                    if u32at(p, 0) as usize != i
                        || u32at(p, 4) as usize != list.len()
                        || u32at(p, 8) as usize != total
                        || bytes.len() + p.len() - 16 > total
                    {
                        return Err(Error::Corrupt("envelope completeness"));
                    }
                    bytes.extend(&p[16..]);
                }
                if bytes.len() != total || crc32c(&bytes) != u32at(&record.payload, 12) {
                    return Err(Error::Corrupt("commit integrity"));
                }
                let e = Envelope::decode(&bytes, lineage)?;
                if e.tx != record.tx || e.csn != u64at(&record.payload, 0) {
                    return Err(Error::Corrupt("commit identity"));
                }
                previous_csn = e.csn;
                envelopes.push(e);
                finished.insert(record.tx, true);
            }
            _ => {}
        }
    }
    Ok(envelopes)
}
