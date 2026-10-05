//! Checked format-v1 WAL framing. No allocation follows an unvalidated length.
use crate::{Error, Result};
pub const SEGMENT_SIZE: u64 = 67_108_864;
pub const MAX_RECORD: usize = 1_048_576;
pub const MAX_ENVELOPE: usize = 16_777_216;
pub const CHUNK_BYTES: usize = MAX_RECORD - 80;
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Lineage {
    pub database: [u8; 16],
    pub timeline: [u8; 16],
}
impl Lineage {
    pub fn validate(self) -> Result<()> {
        if self.database == [0; 16] || self.timeline == [0; 16] {
            return Err(Error::Corrupt("zero lineage"));
        }
        Ok(())
    }
}
pub fn crc32c(bytes: &[u8]) -> u32 {
    let mut crc = !0u32;
    for &byte in bytes {
        crc ^= u32::from(byte);
        for _ in 0..8 {
            crc = (crc >> 1) ^ (0x82f63b78 & (0u32.wrapping_sub(crc & 1)));
        }
    }
    !crc
}
pub fn u16at(b: &[u8], o: usize) -> u16 {
    u16::from_le_bytes(b[o..o + 2].try_into().unwrap())
}
pub fn u32at(b: &[u8], o: usize) -> u32 {
    u32::from_le_bytes(b[o..o + 4].try_into().unwrap())
}
pub fn u64at(b: &[u8], o: usize) -> u64 {
    u64::from_le_bytes(b[o..o + 8].try_into().unwrap())
}
pub fn put16(b: &mut [u8], o: usize, n: u16) {
    b[o..o + 2].copy_from_slice(&n.to_le_bytes());
}
pub fn put32(b: &mut [u8], o: usize, n: u32) {
    b[o..o + 4].copy_from_slice(&n.to_le_bytes());
}
pub fn put64(b: &mut [u8], o: usize, n: u64) {
    b[o..o + 8].copy_from_slice(&n.to_le_bytes());
}
pub fn checksum(b: &mut [u8], o: usize) {
    put32(b, o, 0);
    let crc = crc32c(b);
    put32(b, o, crc);
}
pub fn verified(b: &[u8], o: usize) -> Result<()> {
    let mut copy = b.to_vec();
    let expected = u32at(b, o);
    put32(&mut copy, o, 0);
    if crc32c(&copy) != expected {
        return Err(Error::Corrupt("checksum"));
    }
    Ok(())
}
pub fn segment_header(lineage: Lineage, number: u64) -> Result<[u8; 64]> {
    lineage.validate()?;
    number.checked_mul(SEGMENT_SIZE).ok_or(Error::Limit)?;
    let mut b = [0; 64];
    b[..8].copy_from_slice(b"VETRAWL1");
    put16(&mut b, 8, 1);
    put16(&mut b, 10, 64);
    put32(&mut b, 12, SEGMENT_SIZE as u32);
    b[16..32].copy_from_slice(&lineage.database);
    b[32..48].copy_from_slice(&lineage.timeline);
    put64(&mut b, 48, number);
    checksum(&mut b, 56);
    Ok(b)
}
pub fn check_segment(b: &[u8], lineage: Lineage, number: u64) -> Result<()> {
    if b.len() != 64
        || &b[..8] != b"VETRAWL1"
        || u16at(b, 8) != 1
        || u16at(b, 10) != 64
        || u32at(b, 12) != SEGMENT_SIZE as u32
        || b[60..64] != [0; 4]
    {
        return Err(Error::Corrupt("segment header/codec"));
    }
    verified(b, 56)?;
    if b[16..32] != lineage.database || b[32..48] != lineage.timeline || u64at(b, 48) != number {
        return Err(Error::Corrupt("segment lineage/number"));
    }
    Ok(())
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u16)]
pub enum Kind {
    Begin = 1,
    Patch = 2,
    FullPage = 3,
    Clr = 4,
    TopBegin = 5,
    TopPatch = 6,
    TopEnd = 7,
    Envelope = 8,
    Commit = 9,
    Abort = 10,
    End = 11,
    CheckpointBegin = 12,
    CheckpointChunk = 13,
    CheckpointEnd = 14,
}
impl TryFrom<u16> for Kind {
    type Error = Error;
    fn try_from(n: u16) -> Result<Self> {
        Ok(match n {
            1 => Self::Begin,
            2 => Self::Patch,
            3 => Self::FullPage,
            4 => Self::Clr,
            5 => Self::TopBegin,
            6 => Self::TopPatch,
            7 => Self::TopEnd,
            8 => Self::Envelope,
            9 => Self::Commit,
            10 => Self::Abort,
            11 => Self::End,
            12 => Self::CheckpointBegin,
            13 => Self::CheckpointChunk,
            14 => Self::CheckpointEnd,
            _ => return Err(Error::Corrupt("record kind")),
        })
    }
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Record {
    pub kind: Kind,
    pub lsn: u64,
    pub tx: u64,
    pub prev: u64,
    pub page: (u64, u64),
    pub payload: Vec<u8>,
}
fn patch(p: &[u8], prefix: usize, images: usize) -> Result<()> {
    if p.len() < prefix + 8 {
        return Err(Error::Corrupt("patch length"));
    }
    let offset = u16at(p, prefix) as usize;
    let n = u16at(p, prefix + 2) as usize;
    if n == 0
        || offset < 64
        || offset + n > 8192
        || p[prefix + 4..prefix + 8] != [0; 4]
        || p.len() != prefix + 8 + n * images
    {
        return Err(Error::Corrupt("patch bounds"));
    }
    Ok(())
}
impl Record {
    pub fn validate(&self) -> Result<()> {
        if self.lsn % SEGMENT_SIZE < 64
            || self.prev >= self.lsn && self.prev != 0
            || self.payload.len() > MAX_RECORD - 64
        {
            return Err(Error::Corrupt("record bounds"));
        }
        let p = &self.payload;
        let page = matches!(
            self.kind,
            Kind::Patch | Kind::FullPage | Kind::Clr | Kind::TopPatch
        );
        if page {
            if self.page.0 < 2 || self.page.1 == 0 {
                return Err(Error::Corrupt("page address"));
            }
        } else if self.page != (0, 0) {
            return Err(Error::Corrupt("nonpage address"));
        }
        let system = matches!(
            self.kind,
            Kind::TopBegin
                | Kind::TopPatch
                | Kind::TopEnd
                | Kind::CheckpointBegin
                | Kind::CheckpointChunk
                | Kind::CheckpointEnd
        ) || self.kind == Kind::FullPage && self.tx == 0;
        if system {
            if self.tx != 0 || self.prev != 0 {
                return Err(Error::Corrupt("system transaction"));
            }
        } else if self.tx == 0 {
            return Err(Error::Corrupt("transaction identity"));
        }
        match self.kind {
            Kind::Begin | Kind::Abort | Kind::End if !p.is_empty() => {
                return Err(Error::Corrupt("empty payload"));
            }
            Kind::Begin if self.prev != 0 => return Err(Error::Corrupt("begin chain")),
            Kind::Patch => patch(p, 0, 2)?,
            Kind::Clr => {
                patch(p, 8, 1)?;
                if u64at(p, 0) >= self.lsn && u64at(p, 0) != 0 {
                    return Err(Error::Corrupt("CLR link"));
                }
            }
            Kind::TopPatch => {
                patch(p, 8, 2)?;
                if u64at(p, 0) == 0 {
                    return Err(Error::Corrupt("top identity"));
                }
            }
            Kind::FullPage => {
                if p.len() != 8192
                    || &p[..4] != b"VPG1"
                    || u16at(p, 4) != 1
                    || (u64at(p, 8), u64at(p, 16)) != self.page
                {
                    return Err(Error::Corrupt("full page"));
                }
                verified(p, 48)?;
            }
            Kind::TopBegin | Kind::TopEnd | Kind::CheckpointBegin => {
                if p.len() != 8 || u64at(p, 0) == 0 {
                    return Err(Error::Corrupt("action identity"));
                }
            }
            Kind::Envelope => {
                if p.len() < 16
                    || u32at(p, 4) == 0
                    || u32at(p, 4) > 17
                    || u32at(p, 0) >= u32at(p, 4)
                    || u32at(p, 8) as usize > MAX_ENVELOPE
                    || u32at(p, 8) < 96
                    || u32at(p, 12) as usize != p.len() - 16
                    || p.len() - 16 > CHUNK_BYTES
                {
                    return Err(Error::Corrupt("envelope chunk"));
                }
            }
            Kind::Commit => {
                if p.len() != 24
                    || u64at(p, 0) == 0
                    || !(96..=MAX_ENVELOPE as u32).contains(&u32at(p, 8))
                    || u64at(p, 16) == 0
                    || u64at(p, 16) >= self.lsn
                {
                    return Err(Error::Corrupt("commit"));
                }
            }
            Kind::CheckpointChunk => {
                if p.len() < 24
                    || u64at(p, 0) == 0
                    || u32at(p, 12) == 0
                    || u32at(p, 12) > 17
                    || u32at(p, 8) >= u32at(p, 12)
                    || u32at(p, 16) as usize != p.len() - 24
                    || p[20..24] != [0; 4]
                {
                    return Err(Error::Corrupt("checkpoint chunk"));
                }
            }
            Kind::CheckpointEnd
                if p.len() != 24
                    || u64at(p, 0) == 0
                    || u64at(p, 8) == 0
                    || u64at(p, 8) >= self.lsn
                    || u32at(p, 16) as usize > MAX_ENVELOPE =>
            {
                return Err(Error::Corrupt("checkpoint end"));
            }
            _ => {}
        }
        Ok(())
    }
    pub fn encode(&self) -> Result<Vec<u8>> {
        self.validate()?;
        let mut b = vec![0; 64 + self.payload.len()];
        b[..4].copy_from_slice(b"VWR1");
        let len = b.len() as u32;
        put32(&mut b, 4, len);
        put16(&mut b, 8, 1);
        put16(&mut b, 10, self.kind as u16);
        put64(&mut b, 16, self.lsn);
        put64(&mut b, 24, self.tx);
        put64(&mut b, 32, self.prev);
        put64(&mut b, 40, self.page.0);
        put64(&mut b, 48, self.page.1);
        put32(&mut b, 56, self.payload.len() as u32);
        b[64..].copy_from_slice(&self.payload);
        checksum(&mut b, 60);
        Ok(b)
    }
    pub fn decode(b: &[u8], lsn: u64) -> Result<Self> {
        if b.len() < 64
            || b.len() > MAX_RECORD
            || &b[..4] != b"VWR1"
            || u32at(b, 4) as usize != b.len()
            || u16at(b, 8) != 1
            || b[12..16] != [0; 4]
            || u64at(b, 16) != lsn
            || u32at(b, 56) as usize != b.len() - 64
        {
            return Err(Error::Corrupt("record header/codec"));
        }
        verified(b, 60)?;
        let r = Self {
            kind: Kind::try_from(u16at(b, 10))?,
            lsn,
            tx: u64at(b, 24),
            prev: u64at(b, 32),
            page: (u64at(b, 40), u64at(b, 48)),
            payload: b[64..].to_vec(),
        };
        r.validate()?;
        Ok(r)
    }
}
