//! Injectable synchronous I/O contracts. Local filesystem ownership is implemented in STO-001.
use vetra_types::IoFailure;

/// Callers retry interruptions and short writes; a successful write is not durability.
pub trait FileIo {
    fn read_at(&self, offset: u64, output: &mut [u8]) -> Result<usize, IoFailure>;
    fn write_at(&mut self, offset: u64, input: &[u8]) -> Result<usize, IoFailure>;
    fn len(&self) -> Result<u64, IoFailure>;
    fn is_empty(&self) -> Result<bool, IoFailure> {
        self.len().map(|length| length == 0)
    }
    fn truncate(&mut self, length: u64) -> Result<(), IoFailure>;
    fn sync_data(&mut self) -> Result<(), IoFailure>;
    fn sync_all(&mut self) -> Result<(), IoFailure>;
}

/// Lifecycle changes require directory sync as well as file sync.
pub trait DirectoryIo {
    type File: FileIo;
    type Ownership;
    fn acquire_exclusive(&mut self) -> Result<Self::Ownership, IoFailure>;
    fn open(&mut self, name: &str) -> Result<Self::File, IoFailure>;
    fn rename(&mut self, source: &str, destination: &str) -> Result<(), IoFailure>;
    fn sync_directory(&mut self) -> Result<(), IoFailure>;
}
