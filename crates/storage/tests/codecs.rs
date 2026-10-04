use vetra_storage::{Error, codec::*};
fn fixture(name: &str) -> Vec<u8> {
    let s = std::fs::read_to_string(format!(
        "{}/../../docs/fixtures/foundation/{name}.hex",
        env!("CARGO_MANIFEST_DIR")
    ))
    .unwrap();
    let hex: String = s.split_whitespace().collect();
    hex.as_bytes()
        .chunks_exact(2)
        .map(|h| u8::from_str_radix(std::str::from_utf8(h).unwrap(), 16).unwrap())
        .collect()
}
fn sb() -> Superblock {
    Superblock {
        database: [1; 16],
        timeline: [2; 16],
        generation: 1,
        checkpoint: 0,
        wal_start: 0,
        catalog: Address::default(),
        allocator: Address::default(),
        identity_high_water: 0,
    }
}
fn leaf() -> Page {
    Page {
        address: Address {
            id: 2,
            generation: 1,
        },
        lsn: 0,
        owner: 7,
        body: Body::Tree {
            level: 0,
            left: Address::default(),
            right: Address::default(),
            first_child: Address::default(),
            high_key: None,
            records: vec![Record {
                key: b"a".to_vec(),
                value: b"b".to_vec(),
                creator: 1,
                schema: 1,
                overflow: false,
            }],
        },
    }
}
fn rechecksum(b: &mut [u8], offset: usize) {
    b[offset..offset + 4].fill(0);
    let crc = crc32c(b);
    b[offset..offset + 4].copy_from_slice(&crc.to_le_bytes());
}
#[test]
fn independently_authored_format_fixtures() {
    assert_eq!(crc32c(b"123456789"), 0xe3069283);
    let bytes = fixture("superblock");
    let decoded = Superblock::decode(&bytes).unwrap();
    assert_eq!(decoded.encode().unwrap().as_slice(), bytes);
    let bytes = fixture("empty-leaf");
    let id = u64::from_le_bytes(bytes[8..16].try_into().unwrap());
    let generation = u64::from_le_bytes(bytes[16..24].try_into().unwrap());
    let owner = u64::from_le_bytes(bytes[32..40].try_into().unwrap());
    let p = Page::decode(&bytes, Address { id, generation }, owner).unwrap();
    assert_eq!(p.encode().unwrap().as_slice(), bytes);
}
#[test]
fn redundant_superblocks_lineage_versions_and_required_wal() {
    let a = sb().encode().unwrap();
    let mut newer = sb();
    newer.generation = 2;
    let b = newer.encode().unwrap();
    assert_eq!(Superblock::select(&a, &b, 0).unwrap(), newer);
    let mut torn = b;
    torn[300] = 1;
    assert_eq!(Superblock::select(&a, &torn, 0).unwrap(), sb());
    let mut foreign = sb();
    foreign.database = [3; 16];
    assert!(Superblock::select(&a, &foreign.encode().unwrap(), 0).is_err());
    foreign = sb();
    foreign.identity_high_water = 1;
    assert!(Superblock::select(&a, &foreign.encode().unwrap(), 0).is_err());
    let mut future = b;
    future[8] = 2;
    rechecksum(&mut future, 116);
    assert_eq!(
        Superblock::select(&a, &future, 0),
        Err(Error::UnsupportedVersion)
    );
    assert!(Superblock::select(&a, &b, 1).is_err());
}
#[test]
fn slots_compaction_values_and_boundary_records() {
    let mut s = SlottedRecords::default();
    let r = Record {
        key: b"a".to_vec(),
        value: b"x".to_vec(),
        creator: 1,
        schema: 1,
        overflow: false,
    };
    s.insert(10, r.clone()).unwrap();
    s.insert(20, r.clone()).unwrap();
    s.remove(10).unwrap();
    assert_eq!(s.get(20).unwrap(), &r);
    assert_eq!(s.compact()[0].0, 20);
    for v in [
        Value::Null,
        Value::Bytes(vec![]),
        Value::Bytes(vec![0, 255]),
        Value::Text("á".into()),
        Value::U64(u64::MAX),
        Value::I64(i64::MIN),
        Value::Bool(true),
    ] {
        assert_eq!(Value::decode(&v.encode().unwrap()).unwrap(), v);
    }
    let mut p = leaf();
    let Body::Tree { records, .. } = &mut p.body else {
        unreachable!()
    };
    records[0].value = vec![7; 8027];
    let image = p.encode().unwrap();
    assert_eq!(Page::decode(&image, p.address, p.owner).unwrap(), p);
    let Body::Tree { records, .. } = &mut p.body else {
        unreachable!()
    };
    records[0].value.push(7);
    assert!(p.encode().is_err());
}
#[test]
fn malformed_slots_fields_and_generations_fail_before_read() {
    let p = leaf();
    let valid = p.encode().unwrap();
    for (offset, value) in [
        (40, 255),
        (42, 255),
        (128, 0),
        (130, 255),
        (46, 1),
        (52, 1),
        (6, 99),
        (16, 2),
        (32, 0),
    ] {
        let mut bad = valid;
        bad[offset] = value;
        rechecksum(&mut bad, 48);
        assert!(
            Page::decode(&bad, p.address, p.owner).is_err(),
            "offset {offset}"
        );
    }
    for n in [0, 1, 63, 127, 8191] {
        assert!(Page::decode(&valid[..n], p.address, p.owner).is_err());
    }
}
#[test]
fn overflow_exact_owner_generation_cycle_truncation_and_budget() {
    let a = Address {
        id: 3,
        generation: 1,
    };
    let b = Address {
        id: 4,
        generation: 1,
    };
    let pages = [
        Page {
            address: a,
            lsn: 0,
            owner: 7,
            body: Body::Overflow {
                next: b,
                owner_record: 10,
                chunk: vec![1; 8096],
            },
        },
        Page {
            address: b,
            lsn: 0,
            owner: 7,
            body: Body::Overflow {
                next: Address::default(),
                owner_record: 10,
                chunk: vec![2; 100],
            },
        },
    ];
    let descriptor = OverflowRef {
        first: a,
        length: 8196,
    };
    let bytes = descriptor
        .read(7, 10, |a| Ok(pages[usize::from(a.id == 4)].clone()))
        .unwrap();
    assert_eq!(bytes.len(), 8196);
    assert!(descriptor.read(7, 11, |_| Ok(pages[0].clone())).is_err());
    let mut cycle = pages[0].clone();
    let Body::Overflow { next, .. } = &mut cycle.body else {
        unreachable!()
    };
    *next = a;
    assert!(descriptor.read(7, 10, |_| Ok(cycle.clone())).is_err());
    assert!(descriptor.read(7, 10, |_| Err(Error::Stale)).is_err());
    let too_long = OverflowRef {
        first: a,
        length: MAX_VALUE as u64,
    };
    assert!(
        too_long
            .read(7, 10, |a| Ok(Page {
                address: a,
                lsn: 0,
                owner: 7,
                body: Body::Overflow {
                    next: Address {
                        id: a.id + 1,
                        generation: 1
                    },
                    owner_record: 10,
                    chunk: vec![1]
                }
            }))
            .is_err()
    );
}
#[test]
fn seeded_codec_fuzzing_with_checksums_to_reach_deep_validators() {
    let p = leaf();
    let seed = 0x53544f303039u64;
    let mut rng = seed;
    let sb = sb().encode().unwrap();
    let page = p.encode().unwrap();
    for i in 0..2000 {
        rng = rng.wrapping_mul(6364136223846793005).wrapping_add(1);
        let mut b = if i % 2 == 0 { page } else { sb };
        let position = (rng as usize) % 8192;
        b[position] ^= ((rng >> 32) as u8) | 1;
        if i % 3 != 0 {
            rechecksum(&mut b, if i % 2 == 0 { 48 } else { 116 });
        }
        if i % 2 == 0 {
            let _ = Page::decode(&b, p.address, p.owner);
        } else {
            let _ = Superblock::decode(&b);
        }
        let end = (rng as usize) % 8193;
        let _ = Value::decode(&b[..end]);
        let _ = Record::decode(&b[..end], i % 2 == 0);
        let _ = OverflowRef::decode(&b[..end]);
    }
}
