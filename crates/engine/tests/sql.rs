mod support;
use support::*;
use vetra_engine::sql::{Access, Scalar, Session};
use vetra_engine::{Database, Limits};
use vetra_wal::Wal;
fn session(d: &Directory) -> Session {
    let db = Database::from_log(
        Box::new(Wal::open(d.clone(), LINEAGE).unwrap()),
        Limits::default(),
    )
    .unwrap();
    Session::new(
        db.transactions().clone(),
        1,
        Access {
            admin: true,
            ..Access::default()
        },
    )
}
fn run(s: &mut Session, sql: &str) -> Vec<Vec<Scalar>> {
    s.execute(sql, &[], 100)
        .unwrap()
        .last()
        .unwrap()
        .relation
        .rows
        .clone()
}
#[test]
fn sql_ddl_dml_recovery_constraints_and_statement_rollback() {
    let d = Directory::default();
    let mut s = session(&d);
    run(
        &mut s,
        "CREATE TABLE people (id bigint PRIMARY KEY, name text NOT NULL, balance numeric(20,2) DEFAULT 0, CHECK (balance >= 0))",
    );
    assert_eq!(
        run(
            &mut s,
            "INSERT INTO people(id,name) VALUES (1,'Ada'),(2,'Lin') RETURNING id"
        ),
        vec![vec![Scalar::Int(1)], vec![Scalar::Int(2)]]
    );
    run(&mut s, "BEGIN; SAVEPOINT keep");
    assert_eq!(
        s.execute(
            "INSERT INTO people(id,name) VALUES (3,'ok'),(4,NULL)",
            &[],
            100
        )
        .unwrap_err()
        .code,
        "23502"
    );
    assert_eq!(s.ready(), b'E');
    run(&mut s, "ROLLBACK TO keep");
    assert_eq!(
        run(&mut s, "SELECT COUNT(*) FROM people"),
        vec![vec![Scalar::Int(2)]]
    );
    run(&mut s, "COMMIT");
    run(
        &mut s,
        "UPDATE people SET id=5, balance=12.25 WHERE id=1 RETURNING id",
    );
    run(&mut s, "CREATE INDEX name_idx ON people(name)");
    assert_eq!(
        s.execute(
            "INSERT INTO people(id,name) VALUES (5,'duplicate')",
            &[],
            100
        )
        .unwrap_err()
        .code,
        "23505"
    );
    drop(s);
    d.crash();
    let mut s = session(&d);
    assert_eq!(
        run(&mut s, "SELECT id,name,balance FROM people ORDER BY id"),
        vec![
            vec![
                Scalar::Int(2),
                Scalar::Text("Lin".into()),
                Scalar::Numeric("0.00".into())
            ],
            vec![
                Scalar::Int(5),
                Scalar::Text("Ada".into()),
                Scalar::Numeric("12.25".into())
            ]
        ]
    );
    run(
        &mut s,
        "ALTER TABLE people ADD COLUMN enabled boolean DEFAULT true",
    );
    run(&mut s, "ALTER TABLE people RENAME TO users");
    assert_eq!(
        run(&mut s, "SELECT enabled FROM users"),
        vec![vec![Scalar::Bool(true)], vec![Scalar::Bool(true)]]
    );
}
#[test]
fn sql_relational_null_join_groups_cte_subqueries_json_and_windows() {
    let d = Directory::default();
    let mut s = session(&d);
    run(&mut s, "CREATE TABLE a (id int, value int)");
    run(&mut s, "INSERT INTO a VALUES (1,10),(1,20),(2,NULL)");
    run(&mut s, "CREATE TABLE b (id int, label text)");
    run(&mut s, "INSERT INTO b VALUES (1,'x'),(3,'y')");
    assert_eq!(
        run(
            &mut s,
            "SELECT a.id,b.label FROM a LEFT JOIN b ON a.id=b.id ORDER BY a.id"
        ),
        vec![
            vec![Scalar::Int(1), Scalar::Text("x".into())],
            vec![Scalar::Int(1), Scalar::Text("x".into())],
            vec![Scalar::Int(2), Scalar::Null]
        ]
    );
    assert_eq!(
        run(
            &mut s,
            "SELECT id,count(value),sum(value) FROM a GROUP BY id HAVING count(*)>0 ORDER BY id"
        ),
        vec![
            vec![Scalar::Int(1), Scalar::Int(2), Scalar::Int(30)],
            vec![Scalar::Int(2), Scalar::Int(0), Scalar::Null]
        ]
    );
    assert_eq!(
        run(
            &mut s,
            "WITH x AS (SELECT id FROM b) SELECT id FROM x WHERE id IN (SELECT id FROM a)"
        ),
        vec![vec![Scalar::Int(1)]]
    );
    assert_eq!(
        run(
            &mut s,
            "SELECT id FROM b WHERE EXISTS (SELECT 1 FROM a WHERE a.id=b.id)"
        ),
        vec![vec![Scalar::Int(1)]]
    );
    assert_eq!(
        run(
            &mut s,
            "SELECT 3 IN (1,NULL), false AND NULL, true OR NULL, '{\"x\":null}'::jsonb -> 'x', '{\"x\":null}'::jsonb ->> 'x'"
        ),
        vec![vec![
            Scalar::Null,
            Scalar::Bool(false),
            Scalar::Bool(true),
            Scalar::Json("null".into()),
            Scalar::Null
        ]]
    );
    assert_eq!(
        run(
            &mut s,
            "SELECT id,row_number() OVER (ORDER BY id),rank() OVER (ORDER BY id) FROM b ORDER BY id"
        ),
        vec![
            vec![Scalar::Int(1), Scalar::Int(1), Scalar::Int(1)],
            vec![Scalar::Int(3), Scalar::Int(2), Scalar::Int(2)]
        ]
    );
}
#[test]
fn prepared_metadata_epoch_identity_authorization_and_empty_relation_errors() {
    let d = Directory::default();
    let mut s = session(&d);
    run(&mut s, "CREATE TABLE things(id int PRIMARY KEY,label text)");
    let plan = s
        .prepare(
            "INSERT INTO things(id,label) VALUES ($1,$2) RETURNING id,label",
            vec![],
            100,
        )
        .unwrap();
    assert_eq!(
        plan.parameters,
        vec![
            Some(vetra_engine::sql::Type::Int4),
            Some(vetra_engine::sql::Type::Text)
        ]
    );
    assert_eq!(
        plan.columns.iter().map(|c| c.ty.oid()).collect::<Vec<_>>(),
        vec![23, 25]
    );
    assert_eq!(
        s.execute_plan(&plan, &[Scalar::Int(1), Scalar::Text("one".into())], 100)
            .unwrap()
            .affected,
        1
    );
    let cat = vetra_catalog::Catalog::load(
        &s.manager()
            .begin(
                vetra_engine::Isolation::ReadCommitted,
                vetra_catalog::context(1),
            )
            .unwrap(),
    )
    .unwrap();
    let old = cat.table("things").unwrap().id;
    let mut restricted = Session::new(s.manager().clone(), 2, Access::default());
    assert_eq!(
        restricted
            .execute("SELECT * FROM things", &[], 100)
            .unwrap_err()
            .code,
        "42501"
    );
    restricted.access.readable.insert(old);
    assert_eq!(
        run(&mut restricted, "SELECT label FROM things"),
        vec![vec![Scalar::Text("one".into())]]
    );
    assert_eq!(
        restricted
            .execute("UPDATE things SET label='bad'", &[], 100)
            .unwrap_err()
            .code,
        "42501"
    );
    run(
        &mut s,
        "ALTER TABLE things ADD COLUMN enabled boolean DEFAULT true",
    );
    assert_eq!(
        s.execute_plan(&plan, &[Scalar::Int(2), Scalar::Text("two".into())], 100)
            .unwrap_err()
            .code,
        "0A000"
    );
    run(&mut s, "DROP TABLE things; CREATE TABLE things(id int)");
    let cat = vetra_catalog::Catalog::load(
        &s.manager()
            .begin(
                vetra_engine::Isolation::ReadCommitted,
                vetra_catalog::context(1),
            )
            .unwrap(),
    )
    .unwrap();
    assert_ne!(old, cat.table("things").unwrap().id);
    assert_eq!(
        s.execute("SELECT unknown FROM things", &[], 100)
            .unwrap_err()
            .code,
        "42703"
    );
    assert_eq!(
        s.execute("SELECT id FROM things WHERE id", &[], 100)
            .unwrap_err()
            .code,
        "42804"
    );
    assert_eq!(
        s.execute("SELECT id FROM things a CROSS JOIN things b", &[], 100)
            .unwrap_err()
            .code,
        "42702"
    );
    assert_eq!(
        s.execute("SELECT unsupported(id) FROM things", &[], 100)
            .unwrap_err()
            .code,
        "0A000"
    );
}
#[test]
fn identities_upserts_fk_restriction_null_uniqueness_and_view_dependencies() {
    let d = Directory::default();
    let mut s = session(&d);
    run(&mut s, "CREATE TABLE parent(id int PRIMARY KEY)");
    run(
        &mut s,
        "CREATE TABLE child(id int PRIMARY KEY,pid int REFERENCES parent(id),label text UNIQUE)",
    );
    run(&mut s, "INSERT INTO parent VALUES (1)");
    run(&mut s, "INSERT INTO child VALUES (1,1,NULL),(2,1,NULL)");
    assert_eq!(
        s.execute("DELETE FROM parent WHERE id=1", &[], 100)
            .unwrap_err()
            .code,
        "23503"
    );
    assert_eq!(
        s.execute("UPDATE parent SET id=3 WHERE id=1", &[], 100)
            .unwrap_err()
            .code,
        "23503"
    );
    run(
        &mut s,
        "INSERT INTO child VALUES (1,1,'new') ON CONFLICT (id) DO UPDATE SET label=excluded.label",
    );
    assert_eq!(
        run(&mut s, "SELECT label FROM child WHERE id=1"),
        vec![vec![Scalar::Text("new".into())]]
    );
    run(&mut s, "CREATE VIEW v AS SELECT label FROM child");
    assert_eq!(
        run(&mut s, "SELECT label FROM v WHERE label IS NOT NULL"),
        vec![vec![Scalar::Text("new".into())]]
    );
    assert_eq!(
        s.execute("DROP TABLE child", &[], 100).unwrap_err().code,
        "2BP01"
    );
    run(
        &mut s,
        "CREATE TABLE identity_test(id bigint GENERATED ALWAYS AS IDENTITY,name text)",
    );
    assert_eq!(
        run(
            &mut s,
            "INSERT INTO identity_test(name) VALUES ('a') RETURNING id"
        ),
        vec![vec![Scalar::Int(1)]]
    );
    run(
        &mut s,
        "BEGIN; INSERT INTO identity_test(name) VALUES ('discard'); ROLLBACK",
    );
    assert_eq!(
        run(
            &mut s,
            "INSERT INTO identity_test(name) VALUES ('b') RETURNING id"
        ),
        vec![vec![Scalar::Int(3)]]
    );
    drop(s);
    d.crash();
    let mut s = session(&d);
    assert_eq!(
        run(
            &mut s,
            "INSERT INTO identity_test(name) VALUES ('c') RETURNING id"
        ),
        vec![vec![Scalar::Int(4)]]
    );
}
#[test]
fn statement_basis_does_not_refresh_between_native_reads_and_sql_cancel_bounds_fail_cleanly() {
    use vetra_engine::{Context, Image, Isolation, Key, Participant, Value};
    let d = Directory::default();
    let db = Database::from_log(
        Box::new(Wal::open(d.clone(), LINEAGE).unwrap()),
        Limits::default(),
    )
    .unwrap();
    let key = Key {
        participant: Participant::Job,
        object: 999,
        bytes: b"x".to_vec(),
    };
    let reader = db
        .transactions()
        .begin(
            Isolation::ReadCommitted,
            Context::object(1, Participant::Job, 999),
        )
        .unwrap();
    reader.begin_statement().unwrap();
    assert_eq!(reader.read(&key).unwrap(), None);
    let writer = db
        .transactions()
        .begin(
            Isolation::ReadCommitted,
            Context::object(1, Participant::Job, 999),
        )
        .unwrap();
    writer
        .put(key.clone(), 0, Image::from([(1, Value::I64(1))]))
        .unwrap();
    writer.commit(100).unwrap();
    assert_eq!(reader.read(&key).unwrap(), None);
    reader.end_statement().unwrap();
    assert!(reader.read(&key).unwrap().is_some());
    reader.rollback().unwrap();
    let mut s = Session::new(
        db.transactions().clone(),
        1,
        Access {
            admin: true,
            ..Access::default()
        },
    );
    run(&mut s, "CREATE TABLE pressure(id int)");
    run(&mut s, "INSERT INTO pressure VALUES (1),(2),(3)");
    s.budget.rows = 2;
    assert_eq!(
        s.execute("SELECT * FROM pressure", &[], 100)
            .unwrap_err()
            .code,
        "54000"
    );
    assert_eq!(s.ready(), b'I');
    s.budget.rows = 10000;
    s.cancel.store(true, std::sync::atomic::Ordering::Relaxed);
    assert_eq!(
        s.execute("SELECT * FROM pressure", &[], 100)
            .unwrap_err()
            .code,
        "57014"
    );
    assert_eq!(s.ready(), b'I');
}
#[test]
fn unique_and_foreign_key_contenders_cannot_commit_an_invalid_state() {
    use std::sync::{Arc, Barrier};
    use std::thread;
    for isolation in [
        vetra_engine::Isolation::ReadCommitted,
        vetra_engine::Isolation::RepeatableRead,
        vetra_engine::Isolation::Serializable,
    ] {
        let d = Directory::default();
        let mut setup = session(&d);
        run(&mut setup, "CREATE TABLE p(id int PRIMARY KEY)");
        run(
            &mut setup,
            "CREATE TABLE c(id int PRIMARY KEY,pid int REFERENCES p(id))",
        );
        run(&mut setup, "INSERT INTO p VALUES (1)");
        let manager = setup.manager().clone();
        let barrier = Arc::new(Barrier::new(2));
        let mut workers = vec![];
        for _ in 0..2 {
            let manager = manager.clone();
            let barrier = barrier.clone();
            workers.push(thread::spawn(move || {
                let mut s = Session::new(
                    manager,
                    1,
                    Access {
                        admin: true,
                        ..Access::default()
                    },
                );
                s.begin(isolation, 100).unwrap();
                barrier.wait();
                let write = s.execute("INSERT INTO c VALUES (1,1)", &[], 100);
                if write.is_ok() {
                    s.execute("COMMIT", &[], 100).is_ok()
                } else {
                    s.execute("ROLLBACK", &[], 100).unwrap();
                    false
                }
            }));
        }
        let accepted = workers
            .into_iter()
            .filter_map(|w| w.join().ok())
            .filter(|ok| *ok)
            .count();
        assert_eq!(accepted, 1);
        assert_eq!(
            run(&mut setup, "SELECT count(*) FROM c"),
            vec![vec![Scalar::Int(1)]]
        );
        let barrier = Arc::new(Barrier::new(2));
        let mut workers = vec![];
        for sql in ["INSERT INTO c VALUES (2,1)", "DELETE FROM p WHERE id=1"] {
            let manager = manager.clone();
            let barrier = barrier.clone();
            workers.push(thread::spawn(move || {
                let mut s = Session::new(
                    manager,
                    1,
                    Access {
                        admin: true,
                        ..Access::default()
                    },
                );
                s.begin(isolation, 100).unwrap();
                barrier.wait();
                if s.execute(sql, &[], 100).is_ok() {
                    let _ = s.execute("COMMIT", &[], 100);
                } else {
                    let _ = s.execute("ROLLBACK", &[], 100);
                }
            }));
        }
        for w in workers {
            w.join().unwrap()
        }
        assert_eq!(
            run(&mut setup, "SELECT count(*) FROM p"),
            vec![vec![Scalar::Int(1)]]
        );
    }
}
#[test]
fn durable_sql_row_catalog_and_index_publication_survives_every_failed_boundary() {
    let d = Directory::default();
    let mut s = session(&d);
    run(
        &mut s,
        "CREATE TABLE durable(id int PRIMARY KEY,label text UNIQUE)",
    );
    drop(s);
    for boundary in 0..40 {
        let trial = d.fork();
        let mut s = session(&trial);
        trial.fail_after(boundary);
        let acknowledged = s
            .execute(
                "INSERT INTO durable VALUES (1,'one') RETURNING id",
                &[],
                100,
            )
            .is_ok();
        drop(s);
        trial.crash();
        let mut reopened = session(&trial);
        let rows = run(&mut reopened, "SELECT id,label FROM durable ORDER BY id");
        assert!(
            rows.is_empty() || rows == vec![vec![Scalar::Int(1), Scalar::Text("one".into())]],
            "boundary {boundary}"
        );
        if acknowledged {
            assert_eq!(rows.len(), 1, "lost ACK at {boundary}");
        }
        assert_eq!(
            run(
                &mut reopened,
                "SELECT id,label FROM durable WHERE label='one'"
            ),
            rows,
            "index mismatch at {boundary}"
        );
        run(&mut reopened, "INSERT INTO durable VALUES (2,'two')");
        assert_eq!(
            run(&mut reopened, "SELECT id FROM durable WHERE id=2"),
            vec![vec![Scalar::Int(2)]]
        );
    }
}
#[test]
fn parser_boundaries_quoted_names_comments_and_immutable_schema_expressions() {
    let d = Directory::default();
    let mut s = session(&d);
    run(
        &mut s,
        "CREATE TABLE \"Mixed Case\"(\"Name\" text, value int CHECK(value>0))",
    );
    run(
        &mut s,
        "INSERT INTO \"Mixed Case\" VALUES ('semi;colon',1); -- ignored ;\n SELECT \"Name\" FROM \"Mixed Case\"",
    );
    assert_eq!(
        run(&mut s, "SELECT \"Name\" FROM \"Mixed Case\""),
        vec![vec![Scalar::Text("semi;colon".into())]]
    );
    run(
        &mut s,
        "ALTER TABLE \"Mixed Case\" RENAME COLUMN value TO amount",
    );
    assert_eq!(
        s.execute("INSERT INTO \"Mixed Case\" VALUES ('bad',-1)", &[], 100)
            .unwrap_err()
            .code,
        "23514"
    );
    assert_eq!(
        s.execute(
            "CREATE TABLE volatile_check(x int CHECK(now() IS NOT NULL))",
            &[],
            100
        )
        .unwrap_err()
        .code,
        "0A000"
    );
    let nested = format!("SELECT {}1{}", "(".repeat(200), ")".repeat(200));
    assert!(s.execute(&nested, &[], 100).is_err());
    let deep = format!("SELECT {}1", "1+".repeat(500));
    assert_eq!(s.execute(&deep, &[], 100).unwrap_err().code, "54000");
    assert!(
        s.execute("CREATE TABLE unsupported(x int[]) ", &[], 100)
            .is_err()
    );
}
#[test]
fn postgres_golden_result_oid_and_sqlstate_corpus_is_reproducible_without_reference_server() {
    use vetra_types::sql::serde_json;
    let cases: Vec<String> = serde_json::from_str(include_str!(
        "../../../docs/fixtures/sql/postgresql-17.json"
    ))
    .unwrap();
    let golden: serde_json::Value = serde_json::from_str(include_str!(
        "../../../docs/fixtures/sql/postgresql-17-results.json"
    ))
    .unwrap();
    let d = Directory::default();
    let mut s = session(&d);
    for (i, sql) in cases.iter().enumerate() {
        let got = match s.execute(sql, &[], 1_700_000_000_000_000) {
            Ok(o) => {
                let r = &o.last().unwrap().relation;
                serde_json::json!({"rows":r.rows.iter().map(|row|row.iter().map(|v|if v==&Scalar::Null{None}else{Some(v.text())}).collect::<Vec<_>>()).collect::<Vec<_>>(),"oids":r.columns.iter().map(|c|c.ty.oid()).collect::<Vec<_>>()})
            }
            Err(e) => serde_json::json!({"error":e.code}),
        };
        assert_eq!(got, golden["results"][i], "case {i}: {sql}");
    }
}

#[test]
fn cte_shadowing_and_dropped_foreign_keys_do_not_bind_to_current_base_metadata() {
    let d = Directory::default();
    let mut s = session(&d);
    run(
        &mut s,
        "CREATE TABLE shadow (id integer PRIMARY KEY); INSERT INTO shadow VALUES (1)",
    );
    assert_eq!(
        run(
            &mut s,
            "WITH shadow AS (SELECT 2 AS id) SELECT id FROM shadow WHERE id=2"
        ),
        vec![vec![Scalar::Int(2)]]
    );
    run(
        &mut s,
        "CREATE TABLE parent (id integer PRIMARY KEY, obsolete integer); CREATE UNIQUE INDEX obsolete_idx ON parent(obsolete); CREATE TABLE child (id integer REFERENCES parent(obsolete)); DROP TABLE child; DROP INDEX obsolete_idx; ALTER TABLE parent DROP COLUMN obsolete",
    );
    assert_eq!(
        s.execute(
            "CREATE TABLE invalid_default (id integer DEFAULT (SELECT 1))",
            &[],
            100
        )
        .unwrap_err()
        .code,
        "0A000"
    );
    run(&mut s, "CREATE TABLE document (body jsonb)");
    assert_eq!(
        s.execute("CREATE INDEX json_idx ON document(body)", &[], 100)
            .unwrap_err()
            .code,
        "0A000"
    );
    assert_eq!(
        s.execute("SELECT '{\"x\":2}'::jsonb < '{\"x\":10}'::jsonb", &[], 100)
            .unwrap_err()
            .code,
        "0A000"
    );
}
