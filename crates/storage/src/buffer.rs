//! Fixed-capacity synchronous cache. One mutex serializes load and publication;
//! pin guards expose owned copies and never expose a latch to async callers.
use crate::{
    Error, Result,
    codec::{Address, Image, Page},
};
use std::{
    collections::BTreeMap,
    sync::{Arc, Mutex},
};
use vetra_recovery_api::WalBarrier;
pub trait PageIo: Send {
    fn load(&mut self, a: Address) -> Result<Image>;
    fn write(&mut self, a: Address, image: &Image) -> Result<()>;
}
struct Frame {
    image: Image,
    owner: u64,
    pins: usize,
    rec_lsn: Option<u64>,
    page_lsn: u64,
    used: u64,
}
struct State<P> {
    provider: P,
    frames: BTreeMap<Address, Frame>,
    capacity: usize,
    max_pins: usize,
    pins: usize,
    tick: u64,
}
pub struct BufferPool<P> {
    state: Arc<Mutex<State<P>>>,
}
impl<P> Clone for BufferPool<P> {
    fn clone(&self) -> Self {
        Self {
            state: self.state.clone(),
        }
    }
}
pub struct PagePin<P> {
    state: Arc<Mutex<State<P>>>,
    address: Address,
    _reuse_pin: Option<crate::allocator::ReusePin>,
}
impl<P> Drop for PagePin<P> {
    fn drop(&mut self) {
        if let Ok(mut s) = self.state.lock() {
            if let Some(f) = s.frames.get_mut(&self.address) {
                f.pins -= 1;
            }
            s.pins -= 1;
        }
    }
}
impl<P: PageIo> BufferPool<P> {
    pub fn new(provider: P, capacity: usize, max_pins: usize) -> Result<Self> {
        if capacity == 0 || capacity > 65536 || max_pins == 0 {
            return Err(Error::Limit);
        }
        Ok(Self {
            state: Arc::new(Mutex::new(State {
                provider,
                frames: BTreeMap::new(),
                capacity,
                max_pins,
                pins: 0,
                tick: 0,
            })),
        })
    }
    pub fn pin(&self, a: Address, owner: u64, wal: &mut impl WalBarrier) -> Result<PagePin<P>> {
        let mut s = self.state.lock().map_err(|_| Error::Pinned)?;
        if s.pins >= s.max_pins {
            return Err(Error::Pinned);
        }
        s.tick = s.tick.checked_add(1).ok_or(Error::Exhausted)?;
        let tick = s.tick;
        if !s.frames.contains_key(&a) {
            if s.frames.keys().any(|p| p.id == a.id) {
                return Err(Error::Stale);
            }
            if s.frames.len() == s.capacity {
                let victim = s
                    .frames
                    .iter()
                    .filter(|(_, f)| f.pins == 0)
                    .min_by_key(|(_, f)| f.used)
                    .map(|(a, _)| *a)
                    .ok_or(Error::Pinned)?;
                flush(&mut s, victim, wal)?;
                s.frames.remove(&victim);
            }
            let image = s.provider.load(a)?;
            let page = Page::decode(&image, a, owner)?;
            s.frames.insert(
                a,
                Frame {
                    image,
                    owner,
                    pins: 0,
                    rec_lsn: None,
                    page_lsn: page.lsn,
                    used: tick,
                },
            );
        }
        let frame = s.frames.get_mut(&a).unwrap();
        if frame.owner != owner {
            return Err(Error::Ownership);
        }
        frame.pins += 1;
        frame.used = tick;
        s.pins += 1;
        Ok(PagePin {
            state: self.state.clone(),
            address: a,
            _reuse_pin: None,
        })
    }
    /// Binds frame lifetime to the allocator reuse horizon.
    pub fn pin_allocated(
        &self,
        allocator: &crate::allocator::Allocator,
        a: Address,
        owner: u64,
        wal: &mut impl WalBarrier,
    ) -> Result<PagePin<P>> {
        let reuse = allocator.pin(a, owner)?;
        let mut pin = self.pin(a, owner, wal)?;
        pin._reuse_pin = Some(reuse);
        Ok(pin)
    }
    pub fn flush(&self, wal: &mut impl WalBarrier) -> Result<()> {
        let mut s = self.state.lock().map_err(|_| Error::Pinned)?;
        let keys: Vec<_> = s.frames.keys().copied().collect();
        for a in keys {
            if s.frames[&a].pins > 0 {
                return Err(Error::Pinned);
            }
            flush(&mut s, a, wal)?;
        }
        Ok(())
    }
    pub fn frames(&self) -> Result<usize> {
        Ok(self.state.lock().map_err(|_| Error::Pinned)?.frames.len())
    }
    pub fn invalidate(&self, a: Address) -> Result<()> {
        let mut s = self.state.lock().map_err(|_| Error::Pinned)?;
        if let Some(f) = s.frames.get(&a) {
            if f.pins > 0 {
                return Err(Error::Pinned);
            }
            if f.rec_lsn.is_some() {
                return Err(Error::WalOrdering);
            }
        }
        s.frames.remove(&a);
        Ok(())
    }
}
fn flush<P: PageIo>(s: &mut State<P>, a: Address, wal: &mut impl WalBarrier) -> Result<()> {
    let frame = &s.frames[&a];
    if frame.rec_lsn.is_some() {
        wal.durable_through(frame.page_lsn)?;
        let image = frame.image;
        s.provider.write(a, &image)?;
        s.frames.get_mut(&a).unwrap().rec_lsn = None;
    }
    Ok(())
}
impl<P: PageIo> PagePin<P> {
    pub fn read(&self) -> Result<Page> {
        let s = self.state.lock().map_err(|_| Error::Pinned)?;
        let f = s.frames.get(&self.address).ok_or(Error::Stale)?;
        Page::decode(&f.image, self.address, f.owner)
    }
    /// Compare-and-replace prevents lost updates between owned reads.
    pub fn replace(&self, expected_lsn: u64, page: Page) -> Result<()> {
        let image = page.encode()?;
        let mut s = self.state.lock().map_err(|_| Error::Pinned)?;
        let f = s.frames.get_mut(&self.address).ok_or(Error::Stale)?;
        if f.page_lsn != expected_lsn {
            return Err(Error::Retry);
        }
        if page.address != self.address || page.owner != f.owner {
            return Err(Error::Ownership);
        }
        if page.lsn <= expected_lsn {
            return Err(Error::WalOrdering);
        }
        f.rec_lsn.get_or_insert(page.lsn);
        f.page_lsn = page.lsn;
        f.image = image;
        Ok(())
    }
    pub fn dirty_lsns(&self) -> Result<(Option<u64>, u64)> {
        let s = self.state.lock().map_err(|_| Error::Pinned)?;
        let f = s.frames.get(&self.address).ok_or(Error::Stale)?;
        Ok((f.rec_lsn, f.page_lsn))
    }
}
