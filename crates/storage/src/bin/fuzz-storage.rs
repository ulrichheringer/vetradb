//! Bounded deterministic regression campaign on real storage codecs and trees.
use std::{collections::BTreeMap, ops::Bound};
use vetra_recovery_api::MemoryJournal;
use vetra_storage::{
    codec::*,
    tree::{BPlusTree, TreeConfig},
};
fn main() {
    if let Err(error) = run() {
        eprintln!("{error}");
        std::process::exit(1);
    }
}
fn run() -> Result<(), String> {
    let args: Vec<_> = std::env::args().skip(1).collect();
    let seed = args
        .first()
        .map_or(Ok(42), |s| s.parse::<u64>())
        .map_err(|e| e.to_string())?;
    let steps = args
        .get(1)
        .map_or(Ok(1000), |s| s.parse::<usize>())
        .map_err(|e| e.to_string())?;
    if args.len() > 2 || steps == 0 || steps > 10000 {
        return Err("usage: fuzz-storage [seed:u64] [steps:1..10000]".into());
    }
    let mut random = seed;
    let mut tree = BPlusTree::new(
        7,
        TreeConfig {
            fanout: 3,
            max_pages: 1024,
            max_records: 128,
        },
    )
    .map_err(|e| format!("{e:?}"))?;
    let mut model = BTreeMap::new();
    let mut journal = MemoryJournal::default();
    let mut staged_images = 0usize;
    let mut actions = 0usize;
    for step in 0..steps {
        random = random.wrapping_mul(6364136223846793005).wrapping_add(1);
        let key = ((random >> 32) % 64).to_be_bytes().to_vec();
        let result = if random % 4 == 0 {
            let expected = model.remove(&key);
            tree.delete(&key, &mut journal)
                .map(|actual| actual == expected)
        } else {
            let value = random.to_le_bytes().to_vec();
            let expected = model.insert(key.clone(), value.clone());
            tree.insert(key, value, 1, 1, &mut journal)
                .map(|actual| actual == expected)
        };
        if result != Ok(true) {
            return Err(format!("seed={seed} step={step} map mismatch: {result:?}"));
        }
        tree.validate()
            .map_err(|e| format!("seed={seed} step={step} {e:?}"))?;
        let snapshot = tree.snapshot().map_err(|e| format!("{e:?}"))?;
        let actual = snapshot
            .scan(Bound::Unbounded, Bound::Unbounded, step % 2 == 0, None, 128)
            .map_err(|e| format!("{e:?}"))?;
        let mut expected: Vec<_> = model.iter().map(|(k, v)| (k.clone(), v.clone())).collect();
        if step % 2 == 0 {
            expected.reverse();
        }
        if actual != expected {
            return Err(format!("seed={seed} step={step} scan mismatch"));
        }
        if let Some(image) = tree.page_images().map_err(|e| format!("{e:?}"))?.first() {
            let mut bytes = image.bytes.clone();
            let offset = (random as usize) % 8192;
            bytes[offset] ^= ((random >> 16) as u8) | 1;
            if step % 2 == 0 {
                bytes[48..52].fill(0);
                let crc = crc32c(&bytes);
                bytes[48..52].copy_from_slice(&crc.to_le_bytes());
            }
            let _ = Page::decode(
                &bytes,
                Address {
                    id: image.id,
                    generation: image.generation,
                },
                7,
            );
            let end = (random as usize) % 8193;
            let _ = Page::decode(
                &bytes[..end],
                Address {
                    id: image.id,
                    generation: image.generation,
                },
                7,
            );
            let _ = Record::decode(&bytes[..end], false);
            let _ = Value::decode(&bytes[..end]);
        }
        staged_images += journal
            .completed
            .iter()
            .map(|(_, a)| a.pages.len())
            .sum::<usize>();
        actions += journal.completed.len();
        journal.completed.clear();
        journal.trace.clear();
    }

    let staged_bytes = staged_images * PAGE_SIZE;
    println!(
        "storage campaign passed: seed={seed} steps={steps}; real page/tree oracle (no durability claim)"
    );
    println!(
        "staging measurement: actions={} page_images={staged_images} page_image_bytes={staged_bytes}; input key/value are 8-byte primitives",
        actions
    );
    Ok(())
}
