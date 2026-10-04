#![cfg(unix)]
use vetra_io::{DirectoryIo, Fault, FaultFile, FileIo, LocalDirectory};
use vetra_recovery_api::WalBarrier;
use vetra_storage::{Error, codec::*, pager::Pager};
use vetra_types::IoFailure;
struct Wal(bool);
impl WalBarrier for Wal {
    fn durable_through(&mut self, _: u64) -> std::result::Result<(), IoFailure> {
        if self.0 {
            Ok(())
        } else {
            Err(IoFailure::DurabilityFailure)
        }
    }
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
#[test]
fn native_reopen_lineage_page_checksum_wal_barrier_and_torn_alternate() {
    let path = std::env::temp_dir()
        .canonicalize()
        .unwrap()
        .join(format!("vetra-pager-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&path);
    let mut dir = LocalDirectory::new(&path).unwrap();
    let lock = dir.acquire_exclusive().unwrap();
    let file = dir.open("data.v1").unwrap();
    let mut pager = Pager::bootstrap(file, sb()).unwrap();
    dir.sync_directory().unwrap();
    let page = Page {
        address: Address {
            id: 2,
            generation: 1,
        },
        lsn: 5,
        owner: 7,
        body: Body::Tree {
            level: 0,
            left: Address::default(),
            right: Address::default(),
            first_child: Address::default(),
            high_key: None,
            records: vec![],
        },
    };
    assert_eq!(
        pager.write(&page, &mut Wal(false)),
        Err(Error::Io(IoFailure::DurabilityFailure))
    );
    assert_eq!(pager.into_file().len().unwrap(), 16384);
    let mut pager = Pager::open(dir.open("data.v1").unwrap(), 0).unwrap();
    pager.write(&page, &mut Wal(true)).unwrap();
    let mut next = sb();
    next.catalog = page.address;
    next.checkpoint = 5;
    pager.publish_superblock(next, &mut Wal(true)).unwrap();
    let mut file = pager.into_file();
    file.sync_all().unwrap();
    let reopened = Pager::open(file, 0).unwrap();
    assert_eq!(reopened.superblock.database, [1; 16]);
    assert_eq!(reopened.superblock.generation, 2);
    assert_eq!(reopened.read(page.address, 7).unwrap(), page);
    let mut file = FaultFile::new(reopened.into_file());
    file.writes
        .extend([Fault::Short(64), Fault::Fail(IoFailure::OutOfSpace)]);
    let mut pager = Pager::open(file, 0).unwrap();
    let next = pager.superblock.clone();
    assert!(pager.publish_superblock(next, &mut Wal(true)).is_err());
    let recovered = Pager::open(pager.into_file().inner, 0).unwrap();
    assert_eq!(recovered.superblock.generation, 2);
    drop(recovered);
    drop(lock);
    drop(dir);
    std::fs::remove_dir_all(path).unwrap();
}
#[test]
fn tree_pages_roundtrip_through_native_file_and_one_frame_cache() {
    use vetra_recovery_api::MemoryJournal;
    use vetra_storage::{
        buffer::BufferPool,
        pager::PagerIo,
        tree::{BPlusTree, TreeConfig},
    };
    let path = std::env::temp_dir()
        .canonicalize()
        .unwrap()
        .join(format!("vetra-tree-file-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&path);
    let mut dir = LocalDirectory::new(&path).unwrap();
    let lock = dir.acquire_exclusive().unwrap();
    let config = TreeConfig {
        fanout: 3,
        max_pages: 100,
        max_records: 100,
    };
    let mut tree = BPlusTree::new(7, config).unwrap();
    let mut journal = MemoryJournal::default();
    for n in 0u64..10 {
        tree.insert(
            n.to_be_bytes().to_vec(),
            vec![n as u8; 100],
            1,
            1,
            &mut journal,
        )
        .unwrap();
    }
    let mut pager = Pager::bootstrap(dir.open("data.v1").unwrap(), sb()).unwrap();
    dir.sync_directory().unwrap();
    for image in tree.page_images().unwrap() {
        let a = Address {
            id: image.id,
            generation: image.generation,
        };
        let p = Page::decode(&image.bytes, a, 7).unwrap();
        pager.write(&p, &mut Wal(true)).unwrap();
    }
    let mut next = sb();
    next.catalog = tree.root();
    next.checkpoint = journal.completed.last().unwrap().0;
    pager.publish_superblock(next, &mut Wal(true)).unwrap();
    drop(pager);
    let reopened = Pager::open(dir.open("data.v1").unwrap(), 0).unwrap();
    assert_eq!(reopened.superblock.catalog, tree.root());
    let pool = BufferPool::new(
        PagerIo {
            pager: reopened,
            wal: Wal(true),
        },
        1,
        1,
    )
    .unwrap();
    let (lsn, mut action) = journal.completed.last().unwrap().clone();
    action.previous_root = (0, 0);
    action.retired.clear();
    for image in &mut action.pages {
        let pin = pool
            .pin(
                Address {
                    id: image.id,
                    generation: image.generation,
                },
                7,
                &mut Wal(true),
            )
            .unwrap();
        image.bytes = pin.read().unwrap().encode().unwrap().to_vec();
    }
    let mut restored = BPlusTree::new(7, config).unwrap();
    restored.replay(lsn, &action).unwrap();
    for n in 0u64..10 {
        assert_eq!(
            restored.get(&n.to_be_bytes()).unwrap(),
            Some(vec![n as u8; 100])
        );
    }
    restored.validate().unwrap();
    assert_eq!(pool.frames().unwrap(), 1);
    drop(pool);
    drop(lock);
    drop(dir);
    std::fs::remove_dir_all(path).unwrap();
}
