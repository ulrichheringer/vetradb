use crate::{Error, Result, codec::Address};
use std::{
    collections::BTreeMap,
    sync::{Arc, Mutex},
};
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Entry {
    pub generation: u64,
    pub owner: Option<u64>,
    pub retired: bool,
}
#[derive(Clone, Debug)]
pub struct Allocator {
    entries: BTreeMap<u64, Entry>,
    max_pages: usize,
    pins: Arc<Mutex<BTreeMap<Address, usize>>>,
}
#[derive(Debug)]
pub struct ReusePin {
    address: Address,
    pins: Arc<Mutex<BTreeMap<Address, usize>>>,
}
impl Drop for ReusePin {
    fn drop(&mut self) {
        if let Ok(mut pins) = self.pins.lock() {
            if let Some(count) = pins.get_mut(&self.address) {
                *count -= 1;
                if *count == 0 {
                    pins.remove(&self.address);
                }
            }
        }
    }
}
impl Allocator {
    pub fn new(max_pages: usize) -> Result<Self> {
        if max_pages == 0 || max_pages > 1_000_000 {
            return Err(Error::Limit);
        }
        Ok(Self {
            entries: BTreeMap::new(),
            max_pages,
            pins: Arc::new(Mutex::new(BTreeMap::new())),
        })
    }
    pub fn allocate(&mut self, owner: u64) -> Result<Address> {
        if owner == 0 {
            return Err(Error::Ownership);
        }
        let pins = self.pins.lock().map_err(|_| Error::Pinned)?;
        for (&id, e) in &mut self.entries {
            if e.owner.is_none()
                && !e.retired
                && !pins.contains_key(&Address {
                    id,
                    generation: e.generation,
                })
            {
                e.generation = e.generation.checked_add(1).ok_or(Error::Exhausted)?;
                e.owner = Some(owner);
                return Ok(Address {
                    id,
                    generation: e.generation,
                });
            }
        }
        if self.entries.len() >= self.max_pages {
            return Err(Error::Limit);
        }
        let id = self
            .entries
            .last_key_value()
            .map_or(Ok(2), |(&id, _)| id.checked_add(1).ok_or(Error::Exhausted))?;
        self.entries.insert(
            id,
            Entry {
                generation: 1,
                owner: Some(owner),
                retired: false,
            },
        );
        Ok(Address { id, generation: 1 })
    }
    pub fn check(&self, a: Address, owner: u64) -> Result<()> {
        let e = self.entries.get(&a.id).ok_or(Error::Stale)?;
        if e.generation != a.generation {
            return Err(Error::Stale);
        }
        if e.owner != Some(owner) {
            return Err(Error::Ownership);
        }
        Ok(())
    }
    pub fn retire(&mut self, a: Address, owner: u64) -> Result<()> {
        self.check(a, owner)?;
        let e = self.entries.get_mut(&a.id).unwrap();
        if e.retired {
            return Err(Error::Stale);
        }
        e.retired = true;
        Ok(())
    }
    pub fn reclaim(&mut self) -> Result<usize> {
        let pins = self.pins.lock().map_err(|_| Error::Pinned)?;
        let mut count = 0;
        for (&id, e) in &mut self.entries {
            if e.retired
                && !pins.contains_key(&Address {
                    id,
                    generation: e.generation,
                })
            {
                e.retired = false;
                e.owner = None;
                count += 1;
            }
        }
        Ok(count)
    }
    pub fn pin(&self, a: Address, owner: u64) -> Result<ReusePin> {
        self.check(a, owner)?;
        let mut pins = self.pins.lock().map_err(|_| Error::Pinned)?;
        if pins.values().sum::<usize>() >= self.max_pages {
            return Err(Error::Pinned);
        }
        let count = pins.entry(a).or_default();
        *count = count.checked_add(1).ok_or(Error::Exhausted)?;
        Ok(ReusePin {
            address: a,
            pins: self.pins.clone(),
        })
    }
    pub fn live(&self, owner: u64) -> Vec<Address> {
        self.entries
            .iter()
            .filter(|(_, e)| e.owner == Some(owner) && !e.retired)
            .map(|(&id, e)| Address {
                id,
                generation: e.generation,
            })
            .collect()
    }
    pub fn free_space(&self) -> BTreeMap<u64, u64> {
        self.entries
            .iter()
            .filter(|(_, e)| e.owner.is_none() && !e.retired)
            .map(|(&id, e)| (id, e.generation))
            .collect()
    }
    pub fn encode(&self) -> Vec<u8> {
        let mut b = Vec::with_capacity(16 + self.entries.len() * 32);
        b.extend(b"VALLOC01");
        b.extend((self.entries.len() as u64).to_le_bytes());
        for (&id, e) in &self.entries {
            b.extend(id.to_le_bytes());
            b.extend(e.generation.to_le_bytes());
            b.extend(e.owner.unwrap_or(0).to_le_bytes());
            b.push(u8::from(e.retired));
            b.extend([0; 7]);
        }
        b
    }
    pub fn decode(b: &[u8], max_pages: usize) -> Result<Self> {
        let mut a = Self::new(max_pages)?;
        if b.len() < 16 || &b[..8] != b"VALLOC01" {
            return Err(Error::Corrupt("allocation state"));
        }
        let n = u64::from_le_bytes(b[8..16].try_into().unwrap());
        if n > max_pages as u64
            || (n as usize).checked_mul(32).and_then(|v| v.checked_add(16)) != Some(b.len())
        {
            return Err(Error::Corrupt("allocation count"));
        }
        for c in b[16..].chunks_exact(32) {
            let id = u64::from_le_bytes(c[..8].try_into().unwrap());
            let generation = u64::from_le_bytes(c[8..16].try_into().unwrap());
            let owner = u64::from_le_bytes(c[16..24].try_into().unwrap());
            Address::checked(id, generation)?;
            if id < 2
                || c[24] > 1
                || c[25..].iter().any(|v| *v != 0)
                || (owner == 0 && c[24] != 0)
                || a.entries.contains_key(&id)
            {
                return Err(Error::Corrupt("allocation entry"));
            }
            a.entries.insert(
                id,
                Entry {
                    generation,
                    owner: if owner == 0 { None } else { Some(owner) },
                    retired: c[24] == 1,
                },
            );
        }
        Ok(a)
    }
    pub(crate) fn restore(&self, b: &[u8]) -> Result<Self> {
        let mut a = Self::decode(b, self.max_pages)?;
        for address in self.pins.lock().map_err(|_| Error::Pinned)?.keys() {
            let old = self.entries.get(&address.id).ok_or(Error::Stale)?;
            let next = a.entries.get(&address.id).ok_or(Error::Pinned)?;
            if next.generation != address.generation || next.owner != old.owner {
                return Err(Error::Pinned);
            }
        }
        a.pins = self.pins.clone();
        Ok(a)
    }
}
