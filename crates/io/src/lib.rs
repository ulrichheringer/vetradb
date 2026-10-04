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
    /// Existing file names, sorted; discovery must not create missing files.
    fn files(&self) -> Result<Vec<String>, IoFailure>;
    fn sync_directory(&mut self) -> Result<(), IoFailure>;
}

#[cfg(unix)]
mod local;
#[cfg(unix)]
pub use local::{LocalDirectory, LocalFile, Ownership};

pub fn read_exact_at(
    file: &dyn FileIo,
    mut offset: u64,
    mut output: &mut [u8],
) -> Result<(), IoFailure> {
    while !output.is_empty() {
        match file.read_at(offset, output) {
            Err(IoFailure::Interrupted) => continue,
            Err(e) => return Err(e),
            Ok(0) => return Err(IoFailure::UnexpectedEof),
            Ok(n) if n <= output.len() => {
                offset = offset
                    .checked_add(n as u64)
                    .ok_or(IoFailure::InvalidInput)?;
                output = &mut output[n..];
            }
            Ok(_) => return Err(IoFailure::InvalidInput),
        }
    }
    Ok(())
}
pub fn write_all_at(
    file: &mut dyn FileIo,
    mut offset: u64,
    mut input: &[u8],
) -> Result<(), IoFailure> {
    while !input.is_empty() {
        match file.write_at(offset, input) {
            Err(IoFailure::Interrupted) => continue,
            Err(e) => return Err(e),
            Ok(0) => return Err(IoFailure::OutOfSpace),
            Ok(n) if n <= input.len() => {
                offset = offset
                    .checked_add(n as u64)
                    .ok_or(IoFailure::InvalidInput)?;
                input = &input[n..];
            }
            Ok(_) => return Err(IoFailure::InvalidInput),
        }
    }
    Ok(())
}

/// Deterministic fault adapter around the actual provider; each operation consumes
/// at most one queued fault of its kind. Sync failure poisons future mutations.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Fault {
    Interrupt,
    Short(usize),
    Fail(IoFailure),
}
pub struct FaultFile<F> {
    pub inner: F,
    pub reads: std::sync::Mutex<std::collections::VecDeque<Fault>>,
    pub writes: std::collections::VecDeque<Fault>,
    pub syncs: std::collections::VecDeque<Fault>,
    poisoned: bool,
}
impl<F> FaultFile<F> {
    pub fn new(inner: F) -> Self {
        Self {
            inner,
            reads: Default::default(),
            writes: Default::default(),
            syncs: Default::default(),
            poisoned: false,
        }
    }
}
impl<F: FileIo> FileIo for FaultFile<F> {
    fn read_at(&self, offset: u64, output: &mut [u8]) -> Result<usize, IoFailure> {
        let fault = self
            .reads
            .lock()
            .map_err(|_| IoFailure::DurabilityFailure)?
            .pop_front();
        match fault {
            Some(Fault::Interrupt) => Err(IoFailure::Interrupted),
            Some(Fault::Fail(e)) => Err(e),
            Some(Fault::Short(n)) => {
                let n = n.min(output.len());
                self.inner.read_at(offset, &mut output[..n])
            }
            None => self.inner.read_at(offset, output),
        }
    }
    fn write_at(&mut self, offset: u64, input: &[u8]) -> Result<usize, IoFailure> {
        if self.poisoned {
            return Err(IoFailure::DurabilityFailure);
        }
        match self.writes.pop_front() {
            Some(Fault::Interrupt) => Err(IoFailure::Interrupted),
            Some(Fault::Fail(e)) => Err(e),
            Some(Fault::Short(n)) => self.inner.write_at(offset, &input[..n.min(input.len())]),
            None => self.inner.write_at(offset, input),
        }
    }
    fn len(&self) -> Result<u64, IoFailure> {
        self.inner.len()
    }
    fn truncate(&mut self, length: u64) -> Result<(), IoFailure> {
        if self.poisoned {
            return Err(IoFailure::DurabilityFailure);
        }
        self.inner.truncate(length)
    }
    fn sync_data(&mut self) -> Result<(), IoFailure> {
        self.sync(false)
    }
    fn sync_all(&mut self) -> Result<(), IoFailure> {
        self.sync(true)
    }
}
impl<F: FileIo> FaultFile<F> {
    fn sync(&mut self, all: bool) -> Result<(), IoFailure> {
        if self.poisoned {
            return Err(IoFailure::DurabilityFailure);
        }
        let result = match self.syncs.pop_front() {
            Some(Fault::Interrupt) => Err(IoFailure::Interrupted),
            Some(Fault::Fail(e)) => Err(e),
            Some(Fault::Short(_)) => Err(IoFailure::InvalidInput),
            None => {
                if all {
                    self.inner.sync_all()
                } else {
                    self.inner.sync_data()
                }
            }
        };
        if result.is_err() && result != Err(IoFailure::Interrupted) {
            self.poisoned = true;
        }
        result
    }
}
