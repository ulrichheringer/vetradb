use std::{
    collections::BTreeMap,
    sync::{Arc, Mutex},
};
use vetra_recovery_api::WalBarrier;
use vetra_storage::{Error, Result, buffer::*, codec::*};
use vetra_types::IoFailure;
#[derive(Default)]
struct Disk {
    images: BTreeMap<Address, Image>,
    loads: usize,
    writes: usize,
    fail: bool,
}
#[derive(Clone)]
struct Provider(Arc<Mutex<Disk>>);
impl PageIo for Provider {
    fn load(&mut self, a: Address) -> Result<Image> {
        let mut d = self.0.lock().unwrap();
        d.loads += 1;
        d.images.get(&a).copied().ok_or(Error::Stale)
    }
    fn write(&mut self, a: Address, image: &Image) -> Result<()> {
        let mut d = self.0.lock().unwrap();
        if d.fail {
            return Err(Error::Io(IoFailure::OutOfSpace));
        }
        d.writes += 1;
        d.images.insert(a, *image);
        Ok(())
    }
}
#[derive(Default)]
struct Wal {
    fail: bool,
    through: u64,
}
impl WalBarrier for Wal {
    fn durable_through(&mut self, lsn: u64) -> std::result::Result<(), IoFailure> {
        if self.fail {
            return Err(IoFailure::DurabilityFailure);
        }
        self.through = lsn;
        Ok(())
    }
}
fn page(id: u64) -> Page {
    Page {
        address: Address { id, generation: 1 },
        owner: 7,
        lsn: 1,
        body: Body::Tree {
            level: 0,
            left: Address::default(),
            right: Address::default(),
            first_child: Address::default(),
            high_key: None,
            records: vec![],
        },
    }
}
fn provider() -> Provider {
    let mut d = Disk::default();
    for id in 2..6 {
        let p = page(id);
        d.images.insert(p.address, p.encode().unwrap());
    }
    Provider(Arc::new(Mutex::new(d)))
}
#[test]
fn pin_pressure_dirty_rec_lsn_cas_and_wal_before_data_eviction() {
    let provider = provider();
    let pool = BufferPool::new(provider.clone(), 1, 2).unwrap();
    let mut wal = Wal::default();
    let pin = pool.pin(page(2).address, 7, &mut wal).unwrap();
    assert!(matches!(
        pool.pin(page(3).address, 7, &mut wal),
        Err(Error::Pinned)
    ));
    let mut changed = pin.read().unwrap();
    changed.lsn = 5;
    pin.replace(1, changed.clone()).unwrap();
    changed.lsn = 8;
    pin.replace(5, changed.clone()).unwrap();
    assert_eq!(pin.replace(5, changed), Err(Error::Retry));
    assert_eq!(pin.dirty_lsns().unwrap(), (Some(5), 8));
    assert_eq!(pool.invalidate(page(2).address), Err(Error::Pinned));
    drop(pin);
    wal.fail = true;
    assert!(pool.pin(page(3).address, 7, &mut wal).is_err());
    assert_eq!(provider.0.lock().unwrap().writes, 0);
    assert_eq!(pool.frames().unwrap(), 1);
    wal.fail = false;
    provider.0.lock().unwrap().fail = true;
    assert!(pool.pin(page(3).address, 7, &mut wal).is_err());
    assert_eq!(pool.frames().unwrap(), 1);
    provider.0.lock().unwrap().fail = false;
    let _pin = pool.pin(page(3).address, 7, &mut wal).unwrap();
    assert_eq!(wal.through, 8);
    assert_eq!(provider.0.lock().unwrap().writes, 1);
    assert_eq!(pool.frames().unwrap(), 1);
}
#[test]
fn concurrent_loads_share_one_frame_and_safe_retry() {
    let provider = provider();
    let pool = BufferPool::new(provider.clone(), 2, 16).unwrap();
    let start = Arc::new(std::sync::Barrier::new(8));
    let finish = Arc::new(std::sync::Barrier::new(8));
    let mut threads = vec![];
    for _ in 0..8 {
        let pool = pool.clone();
        let start = start.clone();
        let finish = finish.clone();
        threads.push(std::thread::spawn(move || {
            let mut wal = Wal::default();
            start.wait();
            let pin = pool.pin(page(2).address, 7, &mut wal).unwrap();
            let mut p = pin.read().unwrap();
            p.lsn = 2;
            finish.wait();
            pin.replace(1, p)
        }));
    }
    let mut successes = 0;
    for thread in threads {
        match thread.join().unwrap() {
            Ok(()) => successes += 1,
            Err(Error::Retry) => {}
            other => panic!("{other:?}"),
        }
    }
    assert_eq!(successes, 1);
    assert_eq!(provider.0.lock().unwrap().loads, 1);
    pool.flush(&mut Wal::default()).unwrap();
    assert_eq!(provider.0.lock().unwrap().writes, 1);
}
#[test]
fn wrong_owner_generation_and_pin_budget_are_explicit() {
    let pool = BufferPool::new(provider(), 2, 1).unwrap();
    let mut wal = Wal::default();
    assert!(pool.pin(page(2).address, 8, &mut wal).is_err());
    let pin = pool.pin(page(2).address, 7, &mut wal).unwrap();
    assert!(matches!(
        pool.pin(page(2).address, 7, &mut wal),
        Err(Error::Pinned)
    ));
    drop(pin);
    assert!(matches!(
        pool.pin(
            Address {
                id: 2,
                generation: 2
            },
            7,
            &mut wal
        ),
        Err(Error::Stale)
    ));
    pool.invalidate(page(2).address).unwrap();
}
#[test]
fn buffer_pin_blocks_allocator_reuse_until_guard_drop() {
    let mut allocator = vetra_storage::allocator::Allocator::new(2).unwrap();
    let a = allocator.allocate(7).unwrap();
    let pool = BufferPool::new(provider(), 1, 2).unwrap();
    let pin = pool
        .pin_allocated(&allocator, a, 7, &mut Wal::default())
        .unwrap();
    allocator.retire(a, 7).unwrap();
    assert_eq!(allocator.reclaim().unwrap(), 0);
    drop(pin);
    assert_eq!(allocator.reclaim().unwrap(), 1);
    let reused = allocator.allocate(7).unwrap();
    assert_eq!(reused.id, a.id);
    assert_ne!(reused.generation, a.generation);
}
