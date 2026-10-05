//! Experimental M01 page engine. Structural publication is delegated to recovery-api;
//! no durable transactions or SQL/MVCC are provided at this milestone.
pub mod allocator;
pub mod buffer;
pub mod codec;
pub mod pager;
pub mod rows;
pub mod tree;
use vetra_types::IoFailure;
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Error {
    Io(IoFailure),
    Corrupt(&'static str),
    UnsupportedVersion,
    Limit,
    Exhausted,
    Stale,
    Ownership,
    Pinned,
    WalOrdering,
    Duplicate,
    Retry,
}
impl From<IoFailure> for Error {
    fn from(e: IoFailure) -> Self {
        Self::Io(e)
    }
}
pub type Result<T> = std::result::Result<T, Error>;
