mod support;
use support::*;
use vetra_recovery::*;
use vetra_storage::codec::{Address, Body, Page};
use vetra_wal::{Kind, Log, Record, Wal, codec::*};
fn page(n: u8) -> Page {
    Page {
        address: Address {
            id: 2,
            generation: 1,
        },
        owner: 9,
        lsn: 0,
        body: Body::Overflow {
            next: Address::default(),
            owner_record: 1,
            chunk: vec![n; 16],
        },
    }
}
#[test]
fn torn_page_is_repaired_redone_and_loser_undone_with_restartable_clrs() {
    for boundary in 0..8 {
        let d = Directory::default();
        let mut log = Wal::open(d.clone(), LINEAGE).unwrap();
        let begin = log.append(Kind::Begin, 1, 0, (0, 0), vec![]).unwrap();
        let last = log_patch(&mut log, 1, begin, &page(1), &page(2)).unwrap();
        log.flush(last).unwrap();
        let mut pages = MemoryPages::default();
        pages.pages.insert(page(1).address, [42; 8192]);
        d.fail_after(boundary);
        let first = recover(&mut log, &mut pages);
        drop(log);
        d.crash();
        let mut log = Wal::open(d.clone(), LINEAGE).unwrap();
        let second = recover(&mut log, &mut pages).unwrap();
        let third = recover(&mut log, &mut pages).unwrap();
        assert_eq!(third.compensated, 0);
        assert_eq!(third.ended, 0);
        let b = pages.pages[&page(1).address];
        let decoded = Page::decode(&b, page(1).address, 9).unwrap();
        assert_eq!(
            decoded.body,
            page(1).body,
            "boundary={boundary}, first={first:?}, second={second:?}"
        );
        assert_eq!(
            log.records().iter().filter(|r| r.kind == Kind::Clr).count(),
            1
        );
    }
    let d = Directory::default();
    let mut log = Wal::open(d, LINEAGE).unwrap();
    let begin = log.append(Kind::Begin, 1, 0, (0, 0), vec![]).unwrap();
    let patch = log_patch(&mut log, 1, begin, &page(1), &page(2)).unwrap();
    let abort = log.append(Kind::Abort, 1, patch, (0, 0), vec![]).unwrap();
    let end = log.append(Kind::End, 1, abort, (0, 0), vec![]).unwrap();
    log.flush(end).unwrap();
    let mut pages = MemoryPages::default();
    assert!(recover(&mut log, &mut pages).is_err());
    assert!(
        pages.pages.is_empty(),
        "invalid END must fail before replay"
    );
}
#[test]
fn completed_top_survives_abort_and_incomplete_top_is_never_installed() {
    for completed in [false, true] {
        let d = Directory::default();
        let mut log = Wal::open(d.clone(), LINEAGE).unwrap();
        log.append(Kind::TopBegin, 0, 0, (0, 0), 9u64.to_le_bytes().to_vec())
            .unwrap();
        log.append(
            Kind::FullPage,
            0,
            0,
            (2, 1),
            page(1).encode().unwrap().to_vec(),
        )
        .unwrap();
        let mut p = 9u64.to_le_bytes().to_vec();
        p.extend(96u16.to_le_bytes());
        p.extend(16u16.to_le_bytes());
        p.extend([0; 4]);
        p.extend([1; 16]);
        p.extend([2; 16]);
        let patch = log.append(Kind::TopPatch, 0, 0, (2, 1), p).unwrap();
        let last = if completed {
            log.append(Kind::TopEnd, 0, 0, (0, 0), 9u64.to_le_bytes().to_vec())
                .unwrap()
        } else {
            patch
        };
        log.flush(last).unwrap();
        let mut pages = MemoryPages::default();
        pages
            .pages
            .insert(page(1).address, page(1).encode().unwrap());
        let result = recover(&mut log, &mut pages).unwrap();
        let decoded = Page::decode(&pages.pages[&page(1).address], page(1).address, 9).unwrap();
        assert_eq!(decoded.body, page(if completed { 2 } else { 1 }).body);
        assert_eq!(result.completed_tops, usize::from(completed));
        recover(&mut log, &mut pages).unwrap();
    }
}
#[test]
fn fuzzy_checkpoint_sync_failure_never_becomes_a_recovery_basis() {
    for boundary in 0..5 {
        let d = Directory::default();
        let mut log = Wal::open(d.clone(), LINEAGE).unwrap();
        let table = Checkpoint {
            dirty: vec![(2, 1, 64)],
            active: vec![(1, 64, 64, 0, 1)],
            tops: vec![],
        };
        assert_eq!(table.retention(100, &[50]), 50);
        d.fail_after(boundary);
        let ack = checkpoint(&mut log, 1, &table).is_ok();
        drop(log);
        d.crash();
        let log = Wal::open(d.clone(), LINEAGE).unwrap();
        let completed = log.records().iter().any(|r| r.kind == Kind::CheckpointEnd);
        assert!(!ack || completed);
        assert!(
            !completed
                || log
                    .records()
                    .iter()
                    .any(|r| r.kind == Kind::CheckpointBegin)
        );
    }
}
#[test]
fn framing_golden_fixtures_corruption_lineage_unknown_codecs_and_short_io() {
    fn fixture(name: &str) -> Vec<u8> {
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../docs/fixtures/foundation")
            .join(name);
        let s = std::fs::read_to_string(path).unwrap();
        let hex: String = s.chars().filter(|c| !c.is_whitespace()).collect();
        (0..hex.len())
            .step_by(2)
            .map(|i| u8::from_str_radix(&hex[i..i + 2], 16).unwrap())
            .collect()
    }
    let segment = fixture("segment.hex");
    let lineage = vetra_wal::Lineage {
        database: segment[16..32].try_into().unwrap(),
        timeline: segment[32..48].try_into().unwrap(),
    };
    check_segment(&segment, lineage, u64at(&segment, 48)).unwrap();
    assert_eq!(crc32c(b"123456789"), 0xe3069283);
    for name in ["commit.hex", "envelope-chunk.hex"] {
        let b = fixture(name);
        let r = Record::decode(&b, u64at(&b, 16)).unwrap();
        assert_eq!(r.encode().unwrap(), b);
    }
    let b = fixture("row-envelope.hex");
    let e = vetra_wal::envelope::Envelope::decode(&b, lineage).unwrap();
    assert_eq!(e.encode().unwrap(), b);
    let d = Directory::default();
    let mut log = Wal::open(d.clone(), LINEAGE).unwrap();
    d.short_writes(7);
    let lsn = log.append(Kind::Begin, 1, 0, (0, 0), vec![]).unwrap();
    log.flush(lsn).unwrap();
    drop(log);
    d.crash();
    assert_eq!(Wal::open(d.clone(), LINEAGE).unwrap().records().len(), 1);
    let original = d.durable("0000000000000000.wal");
    for offset in [0, 8, 16, 32, 56, 64, 72, 80, 124] {
        let mut b = original.clone();
        b[offset] ^= 1;
        d.overwrite("0000000000000000.wal", b);
        assert!(Wal::open(d.clone(), LINEAGE).is_err(), "offset={offset}");
    }
    d.overwrite("0000000000000000.wal", original);
    assert!(
        Wal::open(
            d.clone(),
            vetra_wal::Lineage {
                database: [3; 16],
                ..LINEAGE
            }
        )
        .is_err()
    );
}
#[test]
fn final_short_record_truncates_but_valid_later_records_and_complete_bad_crc_fail() {
    let d = Directory::default();
    let mut log = Wal::open(d.clone(), LINEAGE).unwrap();
    let first = log.append(Kind::Begin, 1, 0, (0, 0), vec![]).unwrap();
    log.flush(first).unwrap();
    let base = d.durable("0000000000000000.wal");
    let r = Record {
        kind: Kind::Abort,
        lsn: 128,
        tx: 1,
        prev: first,
        page: (0, 0),
        payload: vec![],
    }
    .encode()
    .unwrap();
    for cut in 1..64 {
        let mut b = base.clone();
        b.extend(&r[..cut]);
        d.overwrite("0000000000000000.wal", b);
        assert_eq!(
            Wal::open(d.clone(), LINEAGE).unwrap().records().len(),
            1,
            "cut={cut}"
        );
    }
    let mut b = base.clone();
    let mut corrupt = r.clone();
    corrupt[60] ^= 1;
    b.extend(corrupt);
    d.overwrite("0000000000000000.wal", b);
    assert!(Wal::open(d.clone(), LINEAGE).is_err());
    let mut b = base;
    let mut bad_length = r.clone();
    put32(&mut bad_length, 4, 512);
    b.extend(bad_length);
    let later = Record {
        kind: Kind::Abort,
        lsn: 192,
        tx: 1,
        prev: first,
        page: (0, 0),
        payload: vec![],
    }
    .encode()
    .unwrap();
    b.extend(later);
    d.overwrite("0000000000000000.wal", b);
    assert!(Wal::open(d.clone(), LINEAGE).is_err());
}
#[test]
fn rollover_directory_sync_and_missing_segment_are_checked() {
    let d = Directory::default();
    let mut log = Wal::open(d.clone(), LINEAGE).unwrap();
    let begin = log.append(Kind::Begin, 1, 0, (0, 0), vec![]).unwrap();
    let mut prev = begin;
    // Legal envelope chunks exercise framing across a real 64 MiB segment boundary.
    for _ in 0..65 {
        let mut p = Vec::new();
        for n in [0, 1, CHUNK_BYTES, CHUNK_BYTES] {
            p.extend((n as u32).to_le_bytes());
        }
        p.resize(16 + CHUNK_BYTES, 0);
        prev = log.append(Kind::Envelope, 1, prev, (0, 0), p).unwrap();
    }
    log.flush(prev).unwrap();
    drop(log);
    d.crash();
    let log = Wal::open(d.clone(), LINEAGE).unwrap();
    assert!(log.records().last().unwrap().lsn > SEGMENT_SIZE);
    assert_eq!(
        d.trace()
            .iter()
            .filter(|s| s.as_str() == "directory sync")
            .count(),
        3 // two segment creations plus the reopen namespace barrier
    );
    drop(log);
    use vetra_io::DirectoryIo;
    let mut directory = d.clone();
    directory.rename("0000000000000000.wal", "missing").unwrap();
    assert!(Wal::open(directory, LINEAGE).is_err());
}

#[test]
fn positioned_data_file_repairs_partial_page_and_fails_on_unlogged_generation() {
    use vetra_io::{DirectoryIo, FileIo};
    let d = Directory::default();
    let mut directory = d.clone();
    let file = directory.open("data.v1").unwrap();
    let mut pages = vetra_engine::DataPages::open(file, LINEAGE).unwrap();
    directory.sync_directory().unwrap();
    let mut log = Wal::open(d.clone(), LINEAGE).unwrap();
    let begin = log.append(Kind::Begin, 1, 0, (0, 0), vec![]).unwrap();
    let last = log_patch(&mut log, 1, begin, &page(1), &page(2)).unwrap();
    log.flush(last).unwrap();
    let mut file = pages.into_file();
    let original = page(2).encode().unwrap();
    vetra_io::write_all_at(&mut file, 16384, &original[..4096]).unwrap();
    file.sync_all().unwrap();
    let file_len = file.len().unwrap();
    assert_eq!(file_len, 20480);
    pages = vetra_engine::DataPages::open(file, LINEAGE).unwrap();
    recover(&mut log, &mut pages).unwrap();
    pages.sync().unwrap();
    let b = pages.load(page(1).address).unwrap().unwrap();
    assert_eq!(
        Page::decode(&b, page(1).address, 9).unwrap().body,
        page(1).body
    );
    let d = Directory::default();
    let mut directory = d.clone();
    let mut pages =
        vetra_engine::DataPages::open(directory.open("data.v1").unwrap(), LINEAGE).unwrap();
    let mut newer = page(3);
    newer.address.generation = 2;
    pages
        .install(newer.address, newer.encode().unwrap())
        .unwrap();
    pages.sync().unwrap();
    let mut log = Wal::open(d, LINEAGE).unwrap();
    let begin = log.append(Kind::Begin, 1, 0, (0, 0), vec![]).unwrap();
    log_patch(&mut log, 1, begin, &page(1), &page(2)).unwrap();
    assert!(recover(&mut log, &mut pages).is_err());
    let mut future = page(3);
    future.lsn = u64::MAX;
    pages
        .install(future.address, future.encode().unwrap())
        .unwrap();
    pages.sync().unwrap();
    let before = pages.load(future.address).unwrap();
    assert!(recover(&mut log, &mut pages).is_err());
    assert_eq!(pages.load(future.address).unwrap(), before);
}

#[test]
fn real_wal_barrier_blocks_dirty_eviction_after_failed_sync() {
    use vetra_io::DirectoryIo;
    use vetra_storage::buffer::BufferPool;
    let d = Directory::default();
    let mut directory = d.clone();
    let mut pages =
        vetra_engine::DataPages::open(directory.open("data.v1").unwrap(), LINEAGE).unwrap();
    pages
        .install(page(1).address, page(1).encode().unwrap())
        .unwrap();
    let mut second = page(3);
    second.address.id = 3;
    pages
        .install(second.address, second.encode().unwrap())
        .unwrap();
    pages.sync().unwrap();
    directory.sync_directory().unwrap();
    let mut log = Wal::open(d.clone(), LINEAGE).unwrap();
    let begin = log.append(Kind::Begin, 1, 0, (0, 0), vec![]).unwrap();
    let last = log_patch(&mut log, 1, begin, &page(1), &page(2)).unwrap();
    let pool = BufferPool::new(pages, 1, 2).unwrap();
    let pin = pool.pin(page(1).address, 9, &mut log).unwrap();
    let mut changed = page(2);
    changed.lsn = last;
    pin.replace(0, changed).unwrap();
    assert_eq!(pin.dirty_lsns().unwrap(), (Some(last), last));
    drop(pin);
    let before = d.durable("data.v1");
    d.fail_after(0);
    assert!(pool.pin(second.address, 9, &mut log).is_err());
    assert!(log.is_poisoned());
    assert_eq!(before, d.durable("data.v1"));
    assert_eq!(pool.frames().unwrap(), 1);
    drop(pool);
    drop(log);
    d.crash();
    let mut log = Wal::open(d.clone(), LINEAGE).unwrap();
    let mut directory = d.clone();
    let mut pages =
        vetra_engine::DataPages::open(directory.open("data.v1").unwrap(), LINEAGE).unwrap();
    recover(&mut log, &mut pages).unwrap();
    pages.sync().unwrap();
    assert_eq!(
        Page::decode(
            &pages.load(page(1).address).unwrap().unwrap(),
            page(1).address,
            9
        )
        .unwrap()
        .body,
        page(1).body
    );
}

#[test]
fn checksum_repaired_manifest_owner_corruption_is_detected_by_structural_oracle() {
    use vetra_engine::{Context, Isolation, Key, Participant, Value};
    let d = Directory::default();
    let db = d.open_database();
    let t = db
        .begin(
            Isolation::ReadCommitted,
            Context::object(1, Participant::Row, 10),
        )
        .unwrap();
    t.put(
        Key {
            participant: Participant::Row,
            object: 10,
            bytes: vec![1],
        },
        1,
        [(1, Value::U64(1))].into_iter().collect(),
    )
    .unwrap();
    t.commit(0).unwrap();
    drop(t);
    drop(db);
    let log = Wal::open(d.clone(), LINEAGE).unwrap();
    let mut record = log
        .records()
        .iter()
        .find(|r| r.kind == Kind::FullPage && r.page.0 == vetra_recovery::journal::MANIFEST_START)
        .unwrap()
        .clone();
    let mut page = Page::decode(
        &record.payload,
        Address {
            id: record.page.0,
            generation: record.page.1,
        },
        1,
    )
    .unwrap();
    if let Body::Overflow { chunk, .. } = &mut page.body {
        put64(chunk, 12, 999);
    } else {
        panic!("expected manifest overflow");
    }
    record.payload = page.encode().unwrap().to_vec();
    let mut bytes = d.durable("0000000000000000.wal");
    let encoded = record.encode().unwrap();
    bytes[record.lsn as usize..record.lsn as usize + encoded.len()].copy_from_slice(&encoded);
    drop(log);
    d.overwrite("0000000000000000.wal", bytes);
    let log = Wal::open(d.clone(), LINEAGE).unwrap();
    assert!(vetra_recovery::journal::actions(log.records()).is_err());
    assert!(d.try_database().is_err());
}
