//! E2 real SIGKILL on local files. This does not simulate loss of the OS page cache.
use std::{
    io::{BufRead, BufReader, Write},
    process::{Command, Stdio},
};
use vetra_engine::*;
#[test]
fn kill_at_real_wal_commit_and_ack_boundaries_reopens_atomic_prefix() {
    for point in ["begin", "envelope", "commit-record", "durable", "ack"] {
        let path = std::fs::canonicalize(std::env::temp_dir())
            .unwrap()
            .join(format!("vetra-m02-{}-{point}", std::process::id()));
        std::fs::create_dir_all(&path).unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o700)).unwrap();
        }
        let mut child = Command::new(env!("CARGO_BIN_EXE_crash-transactions"))
            .args([path.to_str().unwrap(), point])
            .stdout(Stdio::piped())
            .stdin(Stdio::piped())
            .spawn()
            .unwrap();
        let mut reader = BufReader::new(child.stdout.take().unwrap());
        let mut acknowledged = false;
        loop {
            let mut line = String::new();
            assert!(
                reader.read_line(&mut line).unwrap() > 0,
                "child exited before {point}"
            );
            if line.starts_with("ACK ") {
                acknowledged = true;
                let mut oracle = std::fs::File::create(path.with_extension("oracle")).unwrap();
                oracle.write_all(line.as_bytes()).unwrap();
                oracle.sync_all().unwrap();
            }
            if line.starts_with("BOUNDARY ") {
                assert_eq!(line.trim(), format!("BOUNDARY {point}"));
                break;
            }
        }
        child.kill().unwrap();
        child.wait().unwrap();
        // Only this test-created directory: the former owner is proven dead by wait().
        std::fs::remove_file(path.join("owner.lock")).unwrap();
        let db = Database::open(
            &path,
            Lineage {
                database: [1; 16],
                timeline: [2; 16],
            },
            Limits::default(),
        )
        .unwrap();
        let mut c = Context::object(1, Participant::Row, 10);
        for p in [Participant::Job, Participant::Event, Participant::Offset] {
            c.readable.push((p, 10));
        }
        let r = db.begin(Isolation::ReadCommitted, c).unwrap();
        let image: Image = [(1, Value::U64(42))].into_iter().collect();
        let present = r
            .read(&Key {
                participant: Participant::Row,
                object: 10,
                bytes: vec![1],
            })
            .unwrap()
            .is_some();
        if acknowledged || point == "durable" {
            assert!(present, "{point}");
        }
        for p in [
            Participant::Row,
            Participant::Job,
            Participant::Event,
            Participant::Offset,
        ] {
            assert_eq!(
                r.read(&Key {
                    participant: p,
                    object: 10,
                    bytes: vec![1]
                })
                .unwrap(),
                if present { Some(image.clone()) } else { None },
                "{point}"
            );
        }
        assert_eq!(
            db.transactions().ledger().unwrap().len(),
            usize::from(present)
        );
        drop(r);
        drop(db);
        if point == "ack" {
            let data = std::fs::read(path.join("data.v1")).unwrap();
            assert!(
                Database::open(
                    &path,
                    Lineage {
                        database: [3; 16],
                        timeline: [2; 16]
                    },
                    Limits::default()
                )
                .is_err()
            );
            assert_eq!(std::fs::read(path.join("data.v1")).unwrap(), data);
            std::fs::rename(path.join("0000000000000000.wal"), path.join("missing-wal")).unwrap();
            assert!(
                Database::open(
                    &path,
                    Lineage {
                        database: [1; 16],
                        timeline: [2; 16]
                    },
                    Limits::default()
                )
                .is_err()
            );
            assert_eq!(std::fs::read(path.join("data.v1")).unwrap(), data);
        }
        std::fs::remove_dir_all(&path).unwrap();
        if acknowledged {
            std::fs::remove_file(path.with_extension("oracle")).unwrap();
        }
    }
}
