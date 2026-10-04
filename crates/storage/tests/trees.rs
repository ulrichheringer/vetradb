use std::{collections::BTreeMap, ops::Bound};
use vetra_recovery_api::{MemoryJournal, StructuralJournal};
use vetra_storage::{Error, codec::*, tree::*};
fn cfg() -> TreeConfig {
    TreeConfig {
        fanout: 3,
        max_pages: 512,
        max_records: 256,
    }
}
fn key(n: u64) -> Vec<u8> {
    n.to_be_bytes().to_vec()
}
#[test]
fn random_insert_delete_queries_scans_resume_and_replay_match_reference() {
    for seed in [1u64, 42, 0x53544f303039] {
        let mut tree = BPlusTree::new(7, cfg()).unwrap();
        let mut journal = MemoryJournal::default();
        let mut model = BTreeMap::new();
        let mut random = seed;
        for step in 0..350 {
            random = random.wrapping_mul(6364136223846793005).wrapping_add(1);
            let k = key((random >> 32) % 50);
            if random % 4 == 0 {
                assert_eq!(
                    tree.delete(&k, &mut journal).unwrap(),
                    model.remove(&k),
                    "seed {seed} step {step}"
                );
            } else {
                let value = key(random);
                assert_eq!(
                    tree.insert(k.clone(), value.clone(), 1, 1, &mut journal)
                        .unwrap(),
                    model.insert(k.clone(), value)
                );
            }
            assert_eq!(tree.get(&k).unwrap(), model.get(&k).cloned());
            tree.validate().unwrap();
            if step % 17 == 0 {
                let snapshot = tree.snapshot().unwrap();
                let ascending = snapshot
                    .scan(Bound::Unbounded, Bound::Unbounded, false, None, 256)
                    .unwrap();
                assert_eq!(
                    ascending,
                    model
                        .iter()
                        .map(|(k, v)| (k.clone(), v.clone()))
                        .collect::<Vec<_>>()
                );
                let descending = snapshot
                    .scan(Bound::Unbounded, Bound::Unbounded, true, None, 256)
                    .unwrap();
                assert_eq!(
                    descending,
                    ascending.iter().rev().cloned().collect::<Vec<_>>()
                );
                let lower = key(10);
                let upper = key(30);
                let range = snapshot
                    .scan(
                        Bound::Excluded(lower.as_slice()),
                        Bound::Included(upper.as_slice()),
                        false,
                        None,
                        256,
                    )
                    .unwrap();
                assert_eq!(
                    range,
                    model
                        .range((Bound::Excluded(lower), Bound::Included(upper)))
                        .map(|(k, v)| (k.clone(), v.clone()))
                        .collect::<Vec<_>>()
                );
                if let Some((k, _)) = ascending.first() {
                    let position = ScanPosition {
                        key: k.clone(),
                        descending: false,
                    };
                    assert_eq!(
                        snapshot
                            .scan(
                                Bound::Unbounded,
                                Bound::Unbounded,
                                false,
                                Some(&position),
                                256
                            )
                            .unwrap(),
                        ascending[1..]
                    );
                }
            }
        }
        let mut reopened = BPlusTree::new(7, cfg()).unwrap();
        for (lsn, action) in &journal.completed {
            reopened.replay(*lsn, action).unwrap();
            reopened.validate().unwrap();
        }
        let (lsn, action) = journal.completed.last().unwrap();
        reopened.replay(*lsn, action).unwrap();
        for (k, v) in &model {
            assert_eq!(reopened.get(k).unwrap().as_ref(), Some(v));
        }
        for k in model.keys() {
            tree.delete(k, &mut journal).unwrap();
            tree.validate().unwrap();
        }
        assert!(tree.root().is_null());
    }
}
#[test]
fn every_split_and_root_publication_failure_keeps_previous_tree_and_allocation() {
    let mut base = BPlusTree::new(7, cfg()).unwrap();
    let mut original = MemoryJournal::default();
    for n in 0..3 {
        base.insert(key(n), key(n), 1, 1, &mut original).unwrap();
    }
    let mut success = BPlusTree::new(7, cfg()).unwrap();
    for (lsn, a) in &original.completed {
        success.replay(*lsn, a).unwrap();
    }
    let mut probe = MemoryJournal::default();
    success.insert(key(3), key(3), 1, 1, &mut probe).unwrap();
    let steps = probe.completed[0].1.pages.len() + 4;
    for step in 0..steps {
        let mut tree = BPlusTree::new(7, cfg()).unwrap();
        for (lsn, a) in &original.completed {
            tree.replay(*lsn, a).unwrap();
        }
        let root = tree.root();
        let mut failure = MemoryJournal {
            fail_at: Some(step),
            ..Default::default()
        };
        assert!(tree.insert(key(3), key(3), 1, 1, &mut failure).is_err());
        assert_eq!(tree.root(), root);
        assert_eq!(tree.get(&key(3)).unwrap(), None);
        tree.validate().unwrap();
        assert!(failure.completed.is_empty());
        failure.fail_at = None;
        tree.insert(key(3), key(3), 1, 1, &mut failure).unwrap();
        tree.validate().unwrap();
    }
}
#[test]
fn merge_root_shrink_failure_and_snapshot_reclamation() {
    let mut tree = BPlusTree::new(7, cfg()).unwrap();
    let mut journal = MemoryJournal::default();
    for n in 0..12 {
        tree.insert(key(n), key(n), 1, 1, &mut journal).unwrap();
    }
    let snapshot = tree.snapshot().unwrap();
    let root = tree.root();
    let mut failure = MemoryJournal {
        fail_at: Some(0),
        ..Default::default()
    };
    assert!(tree.delete(&key(0), &mut failure).is_err());
    assert_eq!(tree.root(), root);
    for n in 0..12 {
        tree.delete(&key(n), &mut journal).unwrap();
    }
    assert!(tree.root().is_null());
    for n in 0..12 {
        assert_eq!(snapshot.get(&key(n)).unwrap(), Some(key(n)));
    }
    drop(snapshot);
    for n in 100..112 {
        tree.insert(key(n), key(n), 1, 1, &mut journal).unwrap();
    }
    tree.validate().unwrap();
}
#[test]
fn allocation_budget_rejects_without_published_root_or_leak() {
    let mut config = cfg();
    config.max_pages = 2;
    let mut tree = BPlusTree::new(7, config).unwrap();
    let mut journal = MemoryJournal::default();
    for n in 0..3 {
        tree.insert(key(n), key(n), 1, 1, &mut journal).unwrap();
    }
    let root = tree.root();
    let count = journal.completed.len();
    for _ in 0..3 {
        assert_eq!(
            tree.insert(key(3), key(3), 1, 1, &mut journal),
            Err(Error::Limit)
        );
        assert_eq!(tree.root(), root);
        assert_eq!(journal.completed.len(), count);
    }
    tree.validate().unwrap();
}
#[test]
fn large_values_and_composite_binary_keys() {
    let mut tree = BPlusTree::new(7, cfg()).unwrap();
    let mut journal = MemoryJournal::default();
    let bytes = vec![255; 100000];
    let keys = [vec![0], vec![0, 0], vec![0, 255], vec![1, 0]];
    for k in &keys {
        tree.insert(k.clone(), bytes.clone(), 1, 1, &mut journal)
            .unwrap();
        assert_eq!(tree.get(k).unwrap(), Some(bytes.clone()));
    }
    tree.validate().unwrap();
    assert!(
        tree.insert(vec![0; MAX_KEY + 1], vec![], 1, 1, &mut journal)
            .is_err()
    );
}
#[test]
fn walker_detects_bad_separator_stale_generation_and_shared_ownership() {
    let mut tree = BPlusTree::new(7, cfg()).unwrap();
    let mut journal = MemoryJournal::default();
    for n in 0..5 {
        tree.insert(key(n), key(n), 1, 1, &mut journal).unwrap();
    }
    let mut bad = journal.completed.last().unwrap().1.clone();
    let root = Address {
        id: bad.root.0,
        generation: bad.root.1,
    };
    let p = bad.pages.iter_mut().find(|p| p.id == root.id).unwrap();
    let mut page = Page::decode(&p.bytes, root, 7).unwrap();
    let Body::Tree { records, .. } = &mut page.body else {
        unreachable!()
    };
    records[0].key = key(1);
    p.bytes = page.encode().unwrap().to_vec();
    let mut reopened = BPlusTree::new(7, cfg()).unwrap();
    for (lsn, a) in &journal.completed[..journal.completed.len() - 1] {
        reopened.replay(*lsn, a).unwrap();
    }
    assert!(
        reopened
            .replay(journal.completed.len() as u64, &bad)
            .is_err()
    );
    let mut bad = journal.completed.last().unwrap().1.clone();
    bad.pages[0].generation += 1;
    assert!(
        reopened
            .replay(journal.completed.len() as u64, &bad)
            .is_err()
    );
    let mut bad = journal.completed.last().unwrap().1.clone();
    bad.pages.push(bad.pages[0].clone());
    assert!(
        reopened
            .replay(journal.completed.len() as u64, &bad)
            .is_err()
    );
}
#[test]
fn logical_rollback_does_not_revert_later_insert() {
    let mut tree = BPlusTree::new(7, cfg()).unwrap();
    let mut journal = MemoryJournal::default();
    for n in 0..4 {
        tree.insert(key(n), key(n), 1, 1, &mut journal).unwrap();
    }
    tree.insert(key(5), key(5), 2, 1, &mut journal).unwrap();
    tree.delete(&key(3), &mut journal).unwrap();
    assert_eq!(tree.get(&key(5)).unwrap(), Some(key(5)));
    tree.validate().unwrap();
}
#[test]
fn replay_rejects_lineage_and_missing_pages_without_changing_root() {
    let mut tree = BPlusTree::new(7, cfg()).unwrap();
    let mut journal = MemoryJournal::default();
    tree.insert(key(1), key(1), 1, 1, &mut journal).unwrap();
    let mut action = journal.completed[0].1.clone();
    action.previous_root = (2, 8);
    let mut restored = BPlusTree::new(7, cfg()).unwrap();
    assert!(restored.replay(1, &action).is_err());
    action = journal.completed[0].1.clone();
    action.pages.clear();
    assert!(restored.replay(1, &action).is_err());
    assert!(restored.root().is_null());
}
#[test]
fn journal_does_not_complete_partial_batch() {
    let mut tree = BPlusTree::new(7, cfg()).unwrap();
    let mut source = MemoryJournal::default();
    tree.insert(key(1), key(1), 1, 1, &mut source).unwrap();
    let mut target = MemoryJournal {
        fail_at: Some(1),
        ..Default::default()
    };
    assert!(
        target
            .seal_batch(vec![source.completed[0].1.clone()])
            .is_err()
    );
    assert!(target.completed.is_empty());
    assert!(target.batches.is_empty());
}
#[test]
fn merge_failure_at_each_stage_and_replay_metadata_corruption() {
    let mut tree = BPlusTree::new(7, cfg()).unwrap();
    let mut source = MemoryJournal::default();
    for n in 0..6 {
        tree.insert(key(n), key(n), 1, 1, &mut source).unwrap();
    }
    let root = tree.root();
    let mut success = MemoryJournal::default();
    tree.delete(&key(0), &mut success).unwrap();
    let action = &success.completed[0].1;
    for step in 0..action.pages.len() + 4 {
        let mut restored = BPlusTree::new(7, cfg()).unwrap();
        for (lsn, a) in &source.completed {
            restored.replay(*lsn, a).unwrap();
        }
        let mut fault = MemoryJournal {
            fail_at: Some(step),
            ..Default::default()
        };
        assert!(restored.delete(&key(0), &mut fault).is_err());
        assert_eq!(fault.trace.len(), step);
        assert_eq!(restored.root(), root);
        assert_eq!(restored.get(&key(0)).unwrap(), Some(key(0)));
        restored.validate().unwrap();
    }
    let mut restored = BPlusTree::new(7, cfg()).unwrap();
    for (lsn, a) in &source.completed {
        restored.replay(*lsn, a).unwrap();
    }
    let mut bad = action.clone();
    bad.retired.clear();
    assert!(restored.replay(7, &bad).is_err());
    bad = action.clone();
    bad.identity_high_water = 0;
    assert!(restored.replay(7, &bad).is_err());
    assert_eq!(restored.root(), root);
}
#[test]
fn maximum_overflow_value_roundtrips_and_oversize_fails_without_mutation() {
    let config = TreeConfig {
        fanout: 3,
        max_pages: 5000,
        max_records: 10,
    };
    let mut tree = BPlusTree::new(7, config).unwrap();
    let mut journal = MemoryJournal::default();
    let value = vec![0xa5; MAX_VALUE];
    tree.insert(key(1), value.clone(), 1, 1, &mut journal)
        .unwrap();
    assert_eq!(tree.get(&key(1)).unwrap(), Some(value));
    tree.validate().unwrap();
    let root = tree.root();
    assert_eq!(
        tree.insert(key(2), vec![0; MAX_VALUE + 1], 1, 1, &mut journal),
        Err(Error::Limit)
    );
    assert_eq!(tree.root(), root);
}
