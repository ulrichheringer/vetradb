//! Owned foundational identities and injectable providers; no runtime or storage dependency.
use std::num::NonZeroU64;

/// Database-scoped attempt identity, never an ordering key.
///
/// ```
/// use vetra_types::TransactionId;
/// let attempt = TransactionId::new(7).expect("nonzero attempt");
/// assert_eq!(attempt.get(), 7);
/// assert!(TransactionId::new(0).is_none());
/// ```
#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub struct TransactionId(NonZeroU64);

impl TransactionId {
    pub const fn new(value: u64) -> Option<Self> {
        match NonZeroU64::new(value) {
            Some(value) => Some(Self(value)),
            None => None,
        }
    }

    pub const fn get(self) -> u64 {
        self.0.get()
    }
}

/// Exact historical basis. Commit 0 represents the empty database.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Basis {
    pub database: [u8; 16],
    pub timeline: [u8; 16],
    pub commit: u64,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum IoFailure {
    Interrupted,
    OutOfSpace,
    UnexpectedEof,
    InvalidInput,
    DurabilityFailure,
    OwnershipUnavailable,
}

/// Wall timestamps persist; monotonic process-local ticks never do.
pub trait Clock {
    fn utc_micros(&self) -> i64;
    fn monotonic_ticks(&self) -> u64;
}

/// Production implementations must use a reviewed cryptographic provider.
pub trait RandomSource {
    fn fill(&mut self, output: &mut [u8]) -> Result<(), IoFailure>;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn zero_attempt_identity_is_rejected_without_truncation() {
        assert_eq!(TransactionId::new(0), None);
        assert_eq!(TransactionId::new(u64::MAX).unwrap().get(), u64::MAX);
    }
}
