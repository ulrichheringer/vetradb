#![cfg(unix)]
use std::{fs, process::Command};
use vetra_io::{
    DirectoryIo, Fault, FaultFile, FileIo, LocalDirectory, read_exact_at, write_all_at,
};
use vetra_types::IoFailure;
fn path(label: &str) -> std::path::PathBuf {
    std::env::temp_dir()
        .canonicalize()
        .unwrap()
        .join(format!("vetra-io-{label}-{}", std::process::id()))
}
#[test]
fn lock_child() {
    if let Ok(path) = std::env::var("VETRA_LOCK_PROBE") {
        let mut dir = LocalDirectory::new(path).unwrap();
        assert_eq!(
            dir.acquire_exclusive().unwrap_err(),
            IoFailure::OwnershipUnavailable
        );
    }
}
#[test]
fn real_files_ownership_lifetime_short_io_sync_and_paths() {
    let path = path("lifecycle");
    let _ = fs::remove_dir_all(&path);
    let mut dir = LocalDirectory::new(&path).unwrap();
    let lock = dir.acquire_exclusive().unwrap();
    let result = Command::new(std::env::current_exe().unwrap())
        .args(["--exact", "lock_child"])
        .env("VETRA_LOCK_PROBE", &path)
        .status()
        .unwrap();
    assert!(result.success());
    assert!(dir.open("../escape").is_err());
    assert!(dir.open("owner.lock").is_err());
    let mut file = FaultFile::new(dir.open("data.v1").unwrap());
    file.writes
        .extend([Fault::Interrupt, Fault::Short(2), Fault::Short(1)]);
    write_all_at(&mut file, 4, b"abcdef").unwrap();
    file.reads
        .lock()
        .unwrap()
        .extend([Fault::Interrupt, Fault::Short(1)]);
    let mut b = [0; 6];
    read_exact_at(&file, 4, &mut b).unwrap();
    assert_eq!(&b, b"abcdef");
    file.sync_all().unwrap();
    dir.sync_directory().unwrap();
    dir.rename("data.v1", "renamed.v1").unwrap();
    drop(lock);
    drop(dir);
    // Open file keeps ownership even after directory/explicit lease are dropped.
    let mut next = LocalDirectory::new(&path).unwrap();
    assert_eq!(
        next.acquire_exclusive().unwrap_err(),
        IoFailure::OwnershipUnavailable
    );
    file.writes.push_back(Fault::Fail(IoFailure::OutOfSpace));
    assert_eq!(write_all_at(&mut file, 0, b"x"), Err(IoFailure::OutOfSpace));
    file.syncs
        .push_back(Fault::Fail(IoFailure::DurabilityFailure));
    assert_eq!(file.sync_data(), Err(IoFailure::DurabilityFailure));
    assert_eq!(file.write_at(0, b"x"), Err(IoFailure::DurabilityFailure));
    drop(file);
    let _lock = next.acquire_exclusive().unwrap();
    let reopened = next.open("renamed.v1").unwrap();
    read_exact_at(&reopened, 4, &mut b).unwrap();
    assert_eq!(&b, b"abcdef");
    drop(reopened);
    drop(_lock);
    drop(next);
    fs::remove_dir_all(path).unwrap();
}
#[test]
fn symlinks_and_stale_ownership_fail_closed() {
    use std::os::unix::fs::symlink;
    let path = path("symlink");
    let _ = fs::remove_dir_all(&path);
    let mut dir = LocalDirectory::new(&path).unwrap();
    let lock = dir.acquire_exclusive().unwrap();
    symlink("/etc/passwd", path.join("data.v1")).unwrap();
    assert!(dir.open("data.v1").is_err());
    let alias = path.with_extension("alias");
    let _ = fs::remove_file(&alias);
    symlink(&path, &alias).unwrap();
    assert!(LocalDirectory::new(&alias).is_err());
    drop(lock);
    drop(dir);
    fs::write(path.join("owner.lock"), b"stale").unwrap();
    let mut dir = LocalDirectory::new(&path).unwrap();
    assert_eq!(
        dir.acquire_exclusive().unwrap_err(),
        IoFailure::OwnershipUnavailable
    );
    fs::remove_file(alias).unwrap();
    fs::remove_dir_all(path).unwrap();
}

#[test]
fn interrupted_sync_failed_rename_and_ownership_cleanup() {
    let path = path("lifecycle-failure");
    let _ = fs::remove_dir_all(&path);
    let mut directory = LocalDirectory::new(&path).unwrap();
    let ownership = directory.acquire_exclusive().unwrap();
    let mut file = FaultFile::new(directory.open("source.v1").unwrap());
    write_all_at(&mut file, 0, b"preserved").unwrap();
    file.syncs.push_back(Fault::Interrupt);
    assert_eq!(file.sync_all(), Err(IoFailure::Interrupted));
    file.sync_all().unwrap();
    assert_eq!(
        directory.rename("missing.v1", "destination.v1"),
        Err(IoFailure::DurabilityFailure)
    );
    assert!(!path.join("destination.v1").exists());
    let mut bytes = [0; 9];
    read_exact_at(&file, 0, &mut bytes).unwrap();
    assert_eq!(&bytes, b"preserved");
    drop(file);
    drop(ownership);
    drop(directory);
    assert!(!path.join("owner.lock").exists());
    let mut reopened = LocalDirectory::new(&path).unwrap();
    let ownership = reopened.acquire_exclusive().unwrap();
    drop(ownership);
    drop(reopened);
    fs::remove_dir_all(path).unwrap();
}
