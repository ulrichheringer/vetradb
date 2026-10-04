//! Unix local files. Lock-file ownership fails closed after an unclean process exit.
use crate::{DirectoryIo, FileIo};
#[cfg(unix)]
use std::os::unix::fs::{DirBuilderExt, FileExt, OpenOptionsExt, PermissionsExt};
use std::{
    fs::{self, File, OpenOptions},
    io,
    path::{Path, PathBuf},
    sync::Arc,
};
use vetra_types::IoFailure;

fn error(e: io::Error) -> IoFailure {
    match e.kind() {
        io::ErrorKind::Interrupted => IoFailure::Interrupted,
        io::ErrorKind::UnexpectedEof => IoFailure::UnexpectedEof,
        io::ErrorKind::AlreadyExists => IoFailure::OwnershipUnavailable,
        io::ErrorKind::InvalidInput => IoFailure::InvalidInput,
        _ if e.raw_os_error() == Some(28) => IoFailure::OutOfSpace,
        _ => IoFailure::DurabilityFailure,
    }
}
#[derive(Debug)]
struct Lease {
    path: PathBuf,
}
impl Drop for Lease {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.path);
    }
}
#[derive(Clone, Debug)]
pub struct Ownership {
    _lease: Arc<Lease>,
}
pub struct LocalDirectory {
    path: PathBuf,
    lease: Option<Ownership>,
}
pub struct LocalFile {
    file: File,
    _lease: Ownership,
}
impl LocalDirectory {
    #[cfg(unix)]
    pub fn new(path: impl AsRef<Path>) -> Result<Self, IoFailure> {
        let path = path.as_ref();
        // Reject symlinks in every component; the parent must be trusted against rename races.
        let absolute = if path.is_absolute() {
            path.to_path_buf()
        } else {
            std::env::current_dir().map_err(error)?.join(path)
        };
        let mut prefix = PathBuf::new();
        for component in absolute.components() {
            if matches!(component, std::path::Component::ParentDir) {
                return Err(IoFailure::InvalidInput);
            }
            prefix.push(component);
            if let Ok(meta) = fs::symlink_metadata(&prefix) {
                if meta.file_type().is_symlink() {
                    return Err(IoFailure::InvalidInput);
                }
            }
        }
        match fs::DirBuilder::new().mode(0o700).create(&absolute) {
            Ok(()) => {}
            Err(e) if e.kind() == io::ErrorKind::AlreadyExists => {}
            Err(e) => return Err(error(e)),
        }
        let meta = fs::symlink_metadata(&absolute).map_err(error)?;
        if !meta.is_dir() || meta.permissions().mode() & 0o077 != 0 {
            return Err(IoFailure::InvalidInput);
        }
        Ok(Self {
            path: absolute,
            lease: None,
        })
    }
    fn name(&self, name: &str) -> Result<PathBuf, IoFailure> {
        if name.is_empty()
            || name == "."
            || name == ".."
            || name == "owner.lock"
            || name.contains('/')
            || name.contains('\\')
            || name.as_bytes().contains(&0)
        {
            return Err(IoFailure::InvalidInput);
        }
        let path = self.path.join(name);
        if fs::symlink_metadata(&path).is_ok_and(|m| m.file_type().is_symlink() || !m.is_file()) {
            return Err(IoFailure::InvalidInput);
        }
        Ok(path)
    }
    fn owned(&self) -> Result<Ownership, IoFailure> {
        self.lease.clone().ok_or(IoFailure::OwnershipUnavailable)
    }
}
#[cfg(unix)]
impl DirectoryIo for LocalDirectory {
    type File = LocalFile;
    type Ownership = Ownership;
    fn acquire_exclusive(&mut self) -> Result<Ownership, IoFailure> {
        if self.lease.is_some() {
            return Err(IoFailure::OwnershipUnavailable);
        }
        let path = self.path.join("owner.lock");
        let mut lock = OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(&path)
            .map_err(error)?;
        let lease = Ownership {
            _lease: Arc::new(Lease { path }),
        };
        use std::io::Write;
        writeln!(lock, "{}", std::process::id()).map_err(error)?;
        lock.sync_all().map_err(error)?;
        self.sync_directory()?;
        self.lease = Some(lease.clone());
        Ok(lease)
    }
    fn open(&mut self, name: &str) -> Result<LocalFile, IoFailure> {
        let lease = self.owned()?;
        let path = self.name(name)?;
        // O_NOFOLLOW on the supported Linux/macOS targets.
        #[cfg(target_os = "linux")]
        let nofollow = 0x20000;
        #[cfg(target_os = "macos")]
        let nofollow = 0x100;
        #[cfg(not(any(target_os = "linux", target_os = "macos")))]
        return Err(IoFailure::InvalidInput);
        #[cfg(any(target_os = "linux", target_os = "macos"))]
        {
            let file = OpenOptions::new()
                .read(true)
                .write(true)
                .create(true)
                .truncate(false)
                .mode(0o600)
                .custom_flags(nofollow)
                .open(path)
                .map_err(error)?;
            if file.metadata().map_err(error)?.permissions().mode() & 0o077 != 0 {
                return Err(IoFailure::InvalidInput);
            }
            Ok(LocalFile {
                file,
                _lease: lease,
            })
        }
    }
    fn rename(&mut self, source: &str, destination: &str) -> Result<(), IoFailure> {
        self.owned()?;
        fs::rename(self.name(source)?, self.name(destination)?).map_err(error)?;
        self.sync_directory()
    }
    fn sync_directory(&mut self) -> Result<(), IoFailure> {
        File::open(&self.path)
            .map_err(error)?
            .sync_all()
            .map_err(error)
    }
}
#[cfg(unix)]
impl FileIo for LocalFile {
    fn read_at(&self, offset: u64, output: &mut [u8]) -> Result<usize, IoFailure> {
        self.file.read_at(output, offset).map_err(error)
    }
    fn write_at(&mut self, offset: u64, input: &[u8]) -> Result<usize, IoFailure> {
        self.file.write_at(input, offset).map_err(error)
    }
    fn len(&self) -> Result<u64, IoFailure> {
        Ok(self.file.metadata().map_err(error)?.len())
    }
    fn truncate(&mut self, length: u64) -> Result<(), IoFailure> {
        self.file.set_len(length).map_err(error)
    }
    fn sync_data(&mut self) -> Result<(), IoFailure> {
        self.file.sync_data().map_err(error)
    }
    fn sync_all(&mut self) -> Result<(), IoFailure> {
        self.file.sync_all().map_err(error)
    }
}
