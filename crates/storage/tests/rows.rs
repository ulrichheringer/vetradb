use vetra_recovery_api::MemoryJournal;
use vetra_storage::{Error, rows::*, tree::TreeConfig};
struct Basis(u64);
impl Visibility for Basis {
    fn visible(&self, creator: u64) -> bool {
        creator <= self.0
    }
}
fn row(version: u64, primary: &str, schema: u64) -> RowVersion {
    RowVersion {
        id: VersionId {
            table: 1,
            row: 10,
            version,
        },
        schema,
        creator: version,
        primary: primary.as_bytes().to_vec(),
        payload: format!("payload-{version}").into_bytes(),
        deleted: false,
    }
}
fn store() -> RowStore {
    RowStore::new(
        7,
        8,
        TreeConfig {
            fanout: 3,
            max_pages: 128,
            max_records: 100,
        },
    )
    .unwrap()
}
#[test]
fn stable_history_across_primary_schema_changes_visibility_delete_and_replay() {
    let mut store = store();
    let mut journal = MemoryJournal::default();
    let first = row(1, "old", 1);
    store.append(first.clone(), &mut journal).unwrap();
    store.append(row(2, "new", 2), &mut journal).unwrap();
    assert_eq!(store.version(first.id).unwrap(), first);
    assert_eq!(
        store.lookup(1, b"old", &Basis(1)).unwrap(),
        Some(first.clone())
    );
    assert_eq!(store.lookup(1, b"old", &Basis(2)).unwrap(), None);
    assert_eq!(store.lookup(1, b"new", &Basis(1)).unwrap(), None);
    assert_eq!(
        store.lookup(1, b"new", &Basis(2)).unwrap().unwrap().id.row,
        10
    );
    assert_eq!(
        store.append(first.clone(), &mut journal),
        Err(Error::Duplicate)
    );
    let mut deleted = row(3, "new", 2);
    deleted.deleted = true;
    store.append(deleted, &mut journal).unwrap();
    assert_eq!(store.lookup(1, b"new", &Basis(3)).unwrap(), None);
    assert!(store.visible_row(1, 10, &Basis(2)).unwrap().is_some());
    store.validate().unwrap();
    let mut restored = super_store();
    for (lsn, batch) in &journal.batches {
        restored.replay_batch(*lsn, batch).unwrap();
    }
    assert_eq!(restored.version(first.id).unwrap(), first);
    assert_eq!(restored.lookup(1, b"new", &Basis(3)).unwrap(), None);
}
fn super_store() -> RowStore {
    store()
}
#[test]
fn two_projection_publication_is_atomic_on_each_failure_boundary() {
    let mut probe = store();
    let mut journal = MemoryJournal::default();
    probe.append(row(1, "a", 1), &mut journal).unwrap();
    let steps = 2 + journal.batches[0]
        .1
        .iter()
        .map(|a| a.pages.len() + 2)
        .sum::<usize>();
    for step in 0..steps {
        let mut store = store();
        let mut journal = MemoryJournal {
            fail_at: Some(step),
            ..Default::default()
        };
        assert!(store.append(row(1, "a", 1), &mut journal).is_err());
        assert_eq!(store.lookup(1, b"a", &Basis(1)).unwrap(), None);
        assert!(store.version(row(1, "a", 1).id).is_err());
        store.validate().unwrap();
        assert!(journal.completed.is_empty());
        journal.fail_at = None;
        store.append(row(1, "a", 1), &mut journal).unwrap();
        assert!(store.lookup(1, b"a", &Basis(1)).unwrap().is_some());
    }
}
#[test]
fn malformed_missing_versions_and_reference_fail_typed() {
    let store = store();
    assert!(matches!(
        store.version(row(1, "a", 1).id),
        Err(Error::Corrupt(_))
    ));
    let bytes = row(1, "a", 1).encode().unwrap();
    for length in 0..bytes.len() {
        assert!(RowVersion::decode(&bytes[..length]).is_err());
    }
    let mut corrupt = bytes;
    corrupt[48] = 2;
    assert!(RowVersion::decode(&corrupt).is_err());
}
#[test]
fn row_projections_have_disjoint_physical_page_ownership() {
    let mut rows = store();
    let mut journal = MemoryJournal::default();
    for n in 1..12 {
        rows.append(row(n, "key", n), &mut journal).unwrap();
        let (_, actions) = journal.batches.last().unwrap();
        let versions: std::collections::BTreeSet<_> =
            actions[0].pages.iter().map(|p| p.id).collect();
        let directory: std::collections::BTreeSet<_> =
            actions[1].pages.iter().map(|p| p.id).collect();
        assert!(versions.is_disjoint(&directory));
    }
    let mut restored = store();
    for (lsn, actions) in &journal.batches {
        restored.replay_batch(*lsn, actions).unwrap();
    }
    let (lsn, actions) = journal.batches.last().unwrap();
    restored.replay_batch(*lsn, actions).unwrap();
}
