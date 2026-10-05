mod support;
use support::*;
use vetra_engine::*;
fn key(n: u8) -> Key {
    Key {
        participant: Participant::Row,
        object: 10,
        bytes: vec![n],
    }
}
fn image(n: u64) -> Image {
    [(1, Value::U64(n)), (2, Value::Null)].into_iter().collect()
}
fn context() -> Context {
    let mut c = Context::object(1, Participant::Row, 10);
    for p in [
        Participant::Schema,
        Participant::Job,
        Participant::Schedule,
        Participant::Event,
        Participant::Offset,
        Participant::Permission,
    ] {
        c.writable.push((p, 10));
        c.readable.push((p, 10));
    }
    c
}
fn seed(db: &Database) {
    let t = db.begin(Isolation::ReadCommitted, context()).unwrap();
    t.put(key(1), 1, image(1)).unwrap();
    t.put(key(2), 1, image(1)).unwrap();
    t.commit(100).unwrap();
}
#[test]
fn visibility_savepoints_cross_participant_ledger_and_restart() {
    let directory = Directory::default();
    let db = directory.open_database();
    seed(&db);
    let rr = db.begin(Isolation::RepeatableRead, context()).unwrap();
    let rc = db.begin(Isolation::ReadCommitted, context()).unwrap();
    let writer = db.begin(Isolation::ReadCommitted, context()).unwrap();
    writer.put(key(1), 1, image(2)).unwrap();
    assert_eq!(rc.read(&key(1)).unwrap(), Some(image(1)));
    writer.savepoint("outer").unwrap();
    for p in [Participant::Job, Participant::Event, Participant::Offset] {
        writer
            .put(
                Key {
                    participant: p,
                    ..key(3)
                },
                0,
                image(3),
            )
            .unwrap();
    }
    writer.savepoint("nested").unwrap();
    writer.put(key(4), 1, image(4)).unwrap();
    writer.statement_error().unwrap();
    assert_eq!(writer.read(&key(1)), Err(Error::Failed));
    writer.rollback_to("outer").unwrap();
    writer.put(key(1), 1, image(5)).unwrap();
    writer.commit(101).unwrap();
    assert_eq!(rr.read(&key(1)).unwrap(), Some(image(1)));
    assert_eq!(rc.read(&key(1)).unwrap(), Some(image(5)));
    assert!(
        db.transactions().ledger().unwrap()[1]
            .operations
            .iter()
            .all(|op| op.key.participant == Participant::Row)
    );
    rr.rollback().unwrap();
    rc.rollback().unwrap();
    drop(writer);
    drop(rr);
    drop(rc);
    drop(db);
    directory.crash();
    let db = directory.open_database();
    let read = db.begin(Isolation::ReadCommitted, context()).unwrap();
    assert_eq!(read.read(&key(1)).unwrap(), Some(image(5)));
    assert_eq!(read.read(&key(4)).unwrap(), None);
    for p in [Participant::Job, Participant::Event, Participant::Offset] {
        assert_eq!(
            read.read(&Key {
                participant: p,
                ..key(3)
            })
            .unwrap(),
            None
        );
    }
}
#[test]
fn repeatable_read_conflict_and_documented_write_skew() {
    let d = Directory::default();
    let db = d.open_database();
    seed(&db);
    let a = db.begin(Isolation::RepeatableRead, context()).unwrap();
    let b = db.begin(Isolation::RepeatableRead, context()).unwrap();
    assert_eq!(a.read(&key(2)).unwrap(), Some(image(1)));
    assert_eq!(b.read(&key(1)).unwrap(), Some(image(1)));
    a.put(key(1), 1, image(0)).unwrap();
    b.put(key(2), 1, image(0)).unwrap();
    a.commit(1).unwrap();
    b.commit(2).unwrap();
    let old = db.begin(Isolation::RepeatableRead, context()).unwrap();
    let fresh = db.begin(Isolation::ReadCommitted, context()).unwrap();
    fresh.put(key(1), 1, image(9)).unwrap();
    fresh.commit(3).unwrap();
    assert_eq!(old.put(key(1), 1, image(8)), Err(Error::Serialization));
    assert_eq!(old.status().unwrap(), Status::Failed);
}
#[test]
fn serializable_prevents_write_skew_and_missing_key_phantoms() {
    let d = Directory::default();
    let db = d.open_database();
    seed(&db);
    let a = db.begin(Isolation::Serializable, context()).unwrap();
    let b = db.begin(Isolation::Serializable, context()).unwrap();
    a.read(&key(2)).unwrap();
    b.read(&key(1)).unwrap();
    std::thread::scope(|s| {
        let h = s.spawn(|| a.put(key(1), 1, image(0)));
        std::thread::sleep(std::time::Duration::from_millis(20));
        assert_eq!(b.put(key(2), 1, image(0)), Err(Error::Deadlock));
        assert_eq!(h.join().unwrap(), Ok(()));
    });
    a.commit(0).unwrap();
    assert_eq!(b.status().unwrap(), Status::Aborted);
    let scan = db.begin(Isolation::Serializable, context()).unwrap();
    scan.scan(Participant::Row, 10, None, None).unwrap();
    let insert = db.begin(Isolation::ReadCommitted, context()).unwrap();
    std::thread::scope(|s| {
        let h = s.spawn(|| insert.insert(key(9), 1, image(9)));
        std::thread::sleep(std::time::Duration::from_millis(20));
        assert_eq!(db.transactions().lock_diagnostics().unwrap().waiting, 1);
        scan.commit(1).unwrap();
        h.join().unwrap().unwrap();
    });
    insert.commit(2).unwrap();
    // A serializable snapshot whose predicate was changed before locking must retry.
    let old = db.begin(Isolation::Serializable, context()).unwrap();
    let w = db.begin(Isolation::ReadCommitted, context()).unwrap();
    w.insert(key(10), 1, image(10)).unwrap();
    w.commit(3).unwrap();
    assert_eq!(
        old.scan(Participant::Row, 10, None, None),
        Err(Error::Serialization)
    );
}
#[test]
fn unique_wait_cancel_drop_and_resource_limits() {
    let d = Directory::default();
    let db = d.open_database();
    let a = db.begin(Isolation::ReadCommitted, context()).unwrap();
    let b = db.begin(Isolation::ReadCommitted, context()).unwrap();
    a.insert(key(1), 1, image(1)).unwrap();
    std::thread::scope(|s| {
        let h = s.spawn(|| b.insert(key(1), 1, image(2)));
        std::thread::sleep(std::time::Duration::from_millis(20));
        a.commit(1).unwrap();
        assert!(matches!(h.join().unwrap(), Err(Error::Storage(_))));
    });
    b.rollback().unwrap();
    let a = db.begin(Isolation::ReadCommitted, context()).unwrap();
    a.put(key(2), 1, image(2)).unwrap();
    let b = db.begin(Isolation::ReadCommitted, context()).unwrap();
    let cancelled = std::sync::atomic::AtomicBool::new(true);
    assert_eq!(
        b.write(key(2), 1, Some(image(3)), false, &cancelled),
        Err(Error::Cancelled)
    );
    drop(a);
    drop(b);
    assert_eq!(db.transactions().lock_diagnostics().unwrap().held, 0);
    let mut denied = context();
    denied.writable.clear();
    let t = db.begin(Isolation::ReadCommitted, denied).unwrap();
    assert_eq!(t.put(key(4), 1, image(4)), Err(Error::Unauthorized));
    assert_eq!(t.status().unwrap(), Status::Failed);
}
#[test]
fn group_commit_uses_one_flush_and_publication_is_common() {
    let d = Directory::default();
    let db = d.open_database();
    let a = db.begin(Isolation::ReadCommitted, context()).unwrap();
    let b = db.begin(Isolation::ReadCommitted, context()).unwrap();
    for p in [
        Participant::Row,
        Participant::Schema,
        Participant::Job,
        Participant::Schedule,
        Participant::Event,
        Participant::Offset,
        Participant::Permission,
    ] {
        a.put(
            Key {
                participant: p,
                ..key(1)
            },
            if [Participant::Row, Participant::Schema].contains(&p) {
                1
            } else {
                0
            },
            image(1),
        )
        .unwrap();
    }
    b.put(key(2), 1, image(2)).unwrap();
    let before = d.trace().len();
    assert_eq!(
        db.transactions().commit_group(&[a.id, b.id], 0).unwrap(),
        vec![1, 2]
    );
    assert_eq!(
        d.trace()[before..]
            .iter()
            .filter(|s| s.as_str() == "file sync")
            .count(),
        1
    );
    assert_eq!(db.transactions().visible().unwrap(), 2);
    assert_eq!(db.transactions().ledger().unwrap()[0].operations.len(), 7);
}
#[test]
fn vacuum_horizons_preserve_ledger_and_expired_snapshot_fails() {
    let d = Directory::default();
    let db = d.open_database();
    seed(&db);
    let pin = db
        .transactions()
        .snapshot(1, PinKind::Reader, Some(10))
        .unwrap();
    let backup = db
        .transactions()
        .snapshot(2, PinKind::Backup, None)
        .unwrap();
    for n in 2..8 {
        let t = db.begin(Isolation::ReadCommitted, context()).unwrap();
        t.put(key(1), 1, image(n)).unwrap();
        t.commit(n as i64).unwrap();
    }
    assert_eq!(db.transactions().vacuum(9).unwrap(), 0);
    assert_eq!(pin.read(&key(1)).unwrap(), Some(image(1)));
    assert_eq!(db.transactions().vacuum(10).unwrap(), 0);
    assert_eq!(pin.read(&key(1)), Err(Error::Expired));
    drop(backup);
    assert!(db.transactions().vacuum(11).unwrap() > 0);
    assert_eq!(db.transactions().ledger().unwrap().len(), 7);
}
#[test]
fn commit_failure_at_each_io_boundary_never_acknowledges_partial_state() {
    // Independent acknowledgment recorder is owned by the harness, never queried from statuses.
    for boundary in include_str!("../../../docs/fixtures/transactions/commit-boundaries.txt")
        .lines()
        .map(|s| s.parse::<usize>().unwrap())
    {
        let d = Directory::default();
        let db = d.open_database();
        seed(&db);
        let t = db.begin(Isolation::ReadCommitted, context()).unwrap();
        for p in [
            Participant::Row,
            Participant::Job,
            Participant::Event,
            Participant::Offset,
        ] {
            t.put(
                Key {
                    participant: p,
                    ..key(3)
                },
                if p == Participant::Row { 1 } else { 0 },
                image(3),
            )
            .unwrap();
        }
        d.fail_after(boundary);
        let acknowledged = t.commit(1).is_ok();
        if !acknowledged {
            assert!(db.begin(Isolation::ReadCommitted, context()).is_err());
        }
        drop(t);
        drop(db);
        d.crash();
        let db = d.open_database();
        let r = db.begin(Isolation::ReadCommitted, context()).unwrap();
        let present = r.read(&key(3)).unwrap().is_some();
        if acknowledged {
            assert!(present, "boundary={boundary}");
        }
        for p in [Participant::Job, Participant::Event, Participant::Offset] {
            assert_eq!(
                r.read(&Key {
                    participant: p,
                    ..key(3)
                })
                .unwrap()
                .is_some(),
                present,
                "boundary={boundary}"
            );
        }
        assert_eq!(
            db.transactions().ledger().unwrap().len(),
            if present { 2 } else { 1 }
        );
        r.rollback().unwrap();
        let next = db.begin(Isolation::ReadCommitted, context()).unwrap();
        next.put(key(4), 1, image(4)).unwrap();
        next.commit(2).unwrap();
        drop(next);
        drop(r);
        drop(db);
        d.crash();
        let db = d.open_database();
        let r = db.begin(Isolation::ReadCommitted, context()).unwrap();
        assert_eq!(r.read(&key(4)).unwrap(), Some(image(4)));
    }
}
#[test]
fn lost_reply_recovers_committed_outcome_and_unknown_is_not_abort() {
    let d = Directory::default();
    let db = d.open_database();
    let t = db.begin(Isolation::ReadCommitted, context()).unwrap();
    let tx = t.id;
    t.put(key(1), 1, image(1)).unwrap();
    let csn = t.commit(0).unwrap();
    drop(t);
    drop(db);
    d.crash();
    let db = d.open_database();
    assert_eq!(
        db.transactions().outcome(LINEAGE, tx).unwrap(),
        Status::Committed(csn)
    );
    assert_eq!(
        db.transactions().outcome(LINEAGE, 9999).unwrap(),
        Status::Unknown
    );
}

#[test]
fn durable_vacuum_checkpoint_and_multiple_restart_continue_root_lineage() {
    let d = Directory::default();
    let db = d.open_database();
    seed(&db);
    let reader = db.begin(Isolation::RepeatableRead, context()).unwrap();
    for n in 2..6 {
        let t = db.begin(Isolation::ReadCommitted, context()).unwrap();
        t.put(key(1), 1, image(n)).unwrap();
        t.commit(n as i64).unwrap();
    }
    db.transactions().checkpoint().unwrap();
    assert_eq!(db.transactions().vacuum(0).unwrap(), 0);
    reader.rollback().unwrap();
    assert!(db.transactions().vacuum(0).unwrap() > 0);
    drop(reader);
    drop(db);
    d.crash();
    for n in 6..10 {
        let db = d.open_database();
        let t = db.begin(Isolation::ReadCommitted, context()).unwrap();
        assert_eq!(t.read(&key(1)).unwrap(), Some(image(n - 1)));
        t.put(key(1), 1, image(n)).unwrap();
        t.commit(n as i64).unwrap();
        db.transactions().vacuum(0).unwrap();
        db.transactions().checkpoint().unwrap();
        drop(t);
        drop(db);
        d.crash();
    }
    let db = d.open_database();
    let t = db.begin(Isolation::ReadCommitted, context()).unwrap();
    assert_eq!(t.read(&key(1)).unwrap(), Some(image(9)));
    assert_eq!(db.transactions().ledger().unwrap().len(), 9);
}

#[test]
fn abort_after_durable_split_keeps_survivors_and_never_reuses_reserved_csn() {
    let d = Directory::default();
    let db = d.open_database();
    let seed = db.begin(Isolation::ReadCommitted, context()).unwrap();
    for n in 0..70 {
        seed.put(key(n), 1, image(n as u64)).unwrap();
    }
    seed.commit(0).unwrap();
    drop(seed);
    let probe = d.fork();
    let probe_db = probe.open_database();
    let probe_tx = probe_db.begin(Isolation::ReadCommitted, context()).unwrap();
    probe_tx.put(key(70), 1, image(70)).unwrap();
    let before = probe.events();
    probe_tx.commit(0).unwrap();
    let boundaries = probe.events() - before;
    drop(probe_tx);
    drop(probe_db);
    let loser = db.begin(Isolation::ReadCommitted, context()).unwrap();
    loser.put(key(70), 1, image(70)).unwrap();
    d.fail_after(boundaries - 2);
    assert!(loser.commit(0).is_err());
    drop(loser);
    drop(db);
    d.persist_visible();
    d.crash();
    let db = d.open_database();
    let survivor = db.begin(Isolation::ReadCommitted, context()).unwrap();
    for n in 0..70 {
        assert_eq!(survivor.read(&key(n)).unwrap(), Some(image(n as u64)));
    }
    assert_eq!(survivor.read(&key(70)).unwrap(), None);
    survivor.put(key(71), 1, image(71)).unwrap();
    let csn = survivor.commit(1).unwrap();
    assert!(
        csn > 2,
        "the failed physical reservation must not be reused"
    );
    assert_eq!(db.transactions().ledger().unwrap().len(), 2);
    drop(survivor);
    drop(db);
    d.crash();
    let db = d.open_database();
    let t = db.begin(Isolation::ReadCommitted, context()).unwrap();
    assert_eq!(t.read(&key(71)).unwrap(), Some(image(71)));
    assert_eq!(t.read(&key(70)).unwrap(), None);
}
