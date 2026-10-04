use std::collections::BTreeMap;
use vetra_storage::{Error, allocator::Allocator, codec::Address};
#[test]
fn pins_reuse_stale_owner_overflow_and_reconstructed_space() {
    let mut a = Allocator::new(2).unwrap();
    let first = a.allocate(7).unwrap();
    let pin = a.pin(first, 7).unwrap();
    a.retire(first, 7).unwrap();
    assert_eq!(a.reclaim().unwrap(), 0);
    assert!(a.check(first, 8).is_err());
    let second = a.allocate(8).unwrap();
    assert_eq!(a.allocate(9), Err(Error::Limit));
    drop(pin);
    assert_eq!(a.reclaim().unwrap(), 1);
    let state = a.encode();
    let mut reopened = Allocator::decode(&state, 2).unwrap();
    assert_eq!(reopened.free_space(), a.free_space());
    let reused = reopened.allocate(8).unwrap();
    assert_eq!(reused.id, first.id);
    assert_eq!(reused.generation, first.generation + 1);
    assert_eq!(reopened.check(first, 8), Err(Error::Stale));
    assert!(reopened.check(second, 8).is_ok());
    let mut exhausted = state;
    exhausted[24..32].copy_from_slice(&u64::MAX.to_le_bytes());
    let mut exhausted = Allocator::decode(&exhausted, 2).unwrap();
    assert_eq!(exhausted.allocate(7), Err(Error::Exhausted));
}
#[test]
fn random_allocate_retire_reopen_against_reference_ownership() {
    let mut allocator = Allocator::new(100).unwrap();
    let mut model: BTreeMap<Address, u64> = BTreeMap::new();
    let mut random = 42u64;
    for _ in 0..1000 {
        random = random.wrapping_mul(6364136223846793005).wrapping_add(1);
        if random % 2 == 0 && !model.is_empty() {
            let (&a, &owner) = model.iter().nth((random as usize) % model.len()).unwrap();
            allocator.retire(a, owner).unwrap();
            allocator.reclaim().unwrap();
            model.remove(&a);
        } else {
            let owner = 2 + (random % 4);
            if let Ok(a) = allocator.allocate(owner) {
                assert!(model.insert(a, owner).is_none());
            }
        }
        allocator = Allocator::decode(&allocator.encode(), 100).unwrap();
        for (&a, &owner) in &model {
            allocator.check(a, owner).unwrap();
        }
        for owner in 2..6 {
            assert_eq!(
                allocator.live(owner).len(),
                model.values().filter(|v| **v == owner).count()
            );
        }
    }
}
