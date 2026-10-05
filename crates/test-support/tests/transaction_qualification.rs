//! Independent foundation oracles applied to the real native engine, with synthetic workloads.
use vetra_engine::{Context, Database, Image, Isolation, Key, Limits, Lineage, Participant, Value};
use vetra_test_support::{
    Outcome, Read, State, Transaction as History, check_atomicity, serial_witness,
};
fn key(name: &str) -> Key {
    Key {
        participant: Participant::Row,
        object: 10,
        bytes: name.bytes().collect(),
    }
}
fn image(n: u64) -> Image {
    [(1, Value::U64(n))].into_iter().collect()
}
fn value(image: Option<Image>) -> Option<String> {
    image.map(|i| match i[&1] {
        Value::U64(n) => n.to_string(),
        _ => panic!("unexpected scalar"),
    })
}
fn directory(name: &str) -> std::path::PathBuf {
    std::fs::canonicalize(std::env::temp_dir())
        .unwrap()
        .join(format!("vetra-qualification-{}-{name}", std::process::id()))
}
#[test]
fn seeded_serializable_and_mixed_histories_have_independent_serial_witnesses() {
    for seed in 0..32 {
        let path = directory(&format!("serial-{seed}"));
        let db = Database::open(
            &path,
            Lineage {
                database: [1; 16],
                timeline: [2; 16],
            },
            Limits::default(),
        )
        .unwrap();
        let context = Context::object(1, Participant::Row, 10);
        let t = db.begin(Isolation::ReadCommitted, context.clone()).unwrap();
        t.put(key("a"), 1, image(1)).unwrap();
        t.put(key("b"), 1, image(1)).unwrap();
        t.commit(0).unwrap();
        drop(t);
        let barrier = std::sync::Barrier::new(2);
        let histories = std::thread::scope(|scope| {
            let mut handles = Vec::new();
            for index in 0..2 {
                let db = &db;
                let barrier = &barrier;
                let context = context.clone();
                handles.push(scope.spawn(move || {
                    let isolation = if index == 0 || seed % 3 == 0 {
                        Isolation::Serializable
                    } else if seed % 3 == 1 {
                        Isolation::ReadCommitted
                    } else {
                        Isolation::RepeatableRead
                    };
                    let tx = db.begin(isolation, context).unwrap();
                    let a = value(tx.read(&key("a")).unwrap());
                    let b = value(tx.read(&key("b")).unwrap());
                    barrier.wait();
                    let target = if index == 0 { "a" } else { "b" };
                    let outcome = tx
                        .put(key(target), 1, image(0))
                        .and_then(|_| tx.commit(seed));
                    if outcome.is_ok() {
                        Some(History {
                            id: tx.id,
                            reads: vec![
                                Read::Point {
                                    key: "a".into(),
                                    observed: a,
                                },
                                Read::Point {
                                    key: "b".into(),
                                    observed: b,
                                },
                            ],
                            writes: [(target.into(), Some("0".into()))].into_iter().collect(),
                            must_follow: vec![],
                        })
                    } else {
                        None
                    }
                }));
            }
            handles
                .into_iter()
                .filter_map(|h| h.join().unwrap())
                .collect::<Vec<_>>()
        });
        let initial: State = [("a".into(), "1".into()), ("b".into(), "1".into())]
            .into_iter()
            .collect();
        assert!(
            serial_witness(&initial, &histories).is_ok(),
            "seed={seed}, histories={histories:?}"
        );
        assert!(!histories.is_empty(), "progress seed={seed}");
        drop(db);
        std::fs::remove_dir_all(path).unwrap();
    }
}
#[test]
fn every_oracle_rejects_a_deliberate_counterexample() {
    let expected = [
        ("row".into(), "42".into()),
        ("ledger".into(), "42".into()),
        ("job".into(), "42".into()),
        ("event".into(), "42".into()),
    ]
    .into_iter()
    .collect();
    let partial = [("row".into(), "42".into())].into_iter().collect();
    assert!(check_atomicity(Outcome::Acknowledged, &expected, Some(&partial)).is_err());
    assert!(check_atomicity(Outcome::Unknown, &expected, Some(&partial)).is_err());
    assert!(check_atomicity(Outcome::Aborted, &expected, Some(&expected)).is_err());
    let initial: State = [("a".into(), "1".into()), ("b".into(), "1".into())]
        .into_iter()
        .collect();
    let histories = vec![
        History {
            id: 1,
            reads: vec![Read::Point {
                key: "b".into(),
                observed: Some("1".into()),
            }],
            writes: [("a".into(), Some("0".into()))].into_iter().collect(),
            must_follow: vec![],
        },
        History {
            id: 2,
            reads: vec![Read::Point {
                key: "a".into(),
                observed: Some("1".into()),
            }],
            writes: [("b".into(), Some("0".into()))].into_iter().collect(),
            must_follow: vec![],
        },
    ];
    assert!(serial_witness(&initial, &histories).is_err());
}
#[test]
fn external_oracle_compares_real_rows_primary_indexes_ledger_and_service_participants() {
    let path = directory("atomicity");
    let lineage = Lineage {
        database: [1; 16],
        timeline: [2; 16],
    };
    let db = Database::open(&path, lineage, Limits::default()).unwrap();
    let mut context = Context::object(1, Participant::Row, 10);
    for p in [
        Participant::Schema,
        Participant::Job,
        Participant::Event,
        Participant::Offset,
    ] {
        context.writable.push((p, 10));
        context.readable.push((p, 10));
    }
    let tx = db.begin(Isolation::ReadCommitted, context.clone()).unwrap();
    let mut expected = State::new();
    for p in [
        Participant::Row,
        Participant::Schema,
        Participant::Job,
        Participant::Event,
        Participant::Offset,
    ] {
        tx.put(
            Key {
                participant: p,
                ..key("a")
            },
            if p == Participant::Row || p == Participant::Schema {
                1
            } else {
                0
            },
            image(42),
        )
        .unwrap();
        expected.insert(format!("{p:?}"), "42".into());
    }
    tx.commit(0).unwrap();
    expected.insert("ledger".into(), "5".into());
    drop(tx);
    drop(db);
    let db = Database::open(&path, lineage, Limits::default()).unwrap();
    let read = db.begin(Isolation::ReadCommitted, context).unwrap();
    let mut actual = State::new();
    for p in [
        Participant::Row,
        Participant::Schema,
        Participant::Job,
        Participant::Event,
        Participant::Offset,
    ] {
        actual.insert(
            format!("{p:?}"),
            value(
                read.read(&Key {
                    participant: p,
                    ..key("a")
                })
                .unwrap(),
            )
            .unwrap(),
        );
    }
    actual.insert(
        "ledger".into(),
        db.transactions().ledger().unwrap()[0]
            .operations
            .len()
            .to_string(),
    );
    check_atomicity(Outcome::Acknowledged, &expected, Some(&actual)).unwrap();
    drop(read);
    drop(db);
    std::fs::remove_dir_all(path).unwrap();
}
