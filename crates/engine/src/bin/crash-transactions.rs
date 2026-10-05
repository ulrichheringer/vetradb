//! Test driver only. Parent owns the external acknowledgment oracle and crash decision.
use std::io::{BufRead, Write};
use vetra_engine::*;
use vetra_io::{DirectoryIo, LocalDirectory};
use vetra_wal::{Kind, Record, Wal};
struct BoundaryLog {
    inner: Wal<LocalDirectory>,
    point: String,
}
fn boundary(point: &str) {
    println!("BOUNDARY {point}");
    std::io::stdout().flush().unwrap();
    let _ = std::io::stdin().lock().lines().next();
}
impl Log for BoundaryLog {
    fn append(
        &mut self,
        kind: Kind,
        tx: u64,
        prev: u64,
        page: (u64, u64),
        payload: Vec<u8>,
    ) -> vetra_wal::Result<u64> {
        let lsn = self.inner.append(kind, tx, prev, page, payload)?;
        let name = match kind {
            Kind::Begin => "begin",
            Kind::Envelope => "envelope",
            Kind::Commit => "commit-record",
            _ => "other",
        };
        if self.point == name {
            boundary(name);
        }
        Ok(lsn)
    }
    fn flush(&mut self, lsn: u64) -> vetra_wal::Result<u64> {
        let durable = self.inner.flush(lsn)?;
        if self.point == "durable"
            && self
                .inner
                .records()
                .last()
                .is_some_and(|r| r.kind == Kind::Commit)
        {
            boundary("durable");
        }
        Ok(durable)
    }
    fn records(&self) -> &[Record] {
        self.inner.records()
    }
    fn lineage(&self) -> Lineage {
        self.inner.lineage()
    }
    fn durable(&self) -> u64 {
        self.inner.durable()
    }
}
fn main() {
    let args: Vec<_> = std::env::args().collect();
    assert_eq!(args.len(), 3);
    let lineage = Lineage {
        database: [1; 16],
        timeline: [2; 16],
    };
    let mut d = LocalDirectory::new(&args[1]).unwrap();
    d.acquire_exclusive().unwrap();
    let log = Wal::open(d, lineage).unwrap();
    let db = Database::from_log(
        Box::new(BoundaryLog {
            inner: log,
            point: args[2].clone(),
        }),
        Limits::default(),
    )
    .unwrap();
    let mut context = Context::object(1, Participant::Row, 10);
    for p in [Participant::Job, Participant::Event, Participant::Offset] {
        context.writable.push((p, 10));
    }
    let t = db.begin(Isolation::Serializable, context).unwrap();
    for p in [
        Participant::Row,
        Participant::Job,
        Participant::Event,
        Participant::Offset,
    ] {
        t.put(
            Key {
                participant: p,
                object: 10,
                bytes: vec![1],
            },
            if p == Participant::Row { 1 } else { 0 },
            [(1, Value::U64(42))].into_iter().collect(),
        )
        .unwrap();
    }
    let csn = t.commit(123).unwrap();
    println!("ACK {} {}", t.id, csn);
    std::io::stdout().flush().unwrap();
    boundary("ack");
}
