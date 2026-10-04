//! Protocol 3.0 response state values. No listener or SQL executor exists yet.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum TransactionState {
    Idle,
    InTransaction,
    FailedTransaction,
}

impl TransactionState {
    pub const fn ready_for_query(self) -> u8 {
        match self {
            Self::Idle => b'I',
            Self::InTransaction => b'T',
            Self::FailedTransaction => b'E',
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn failed_transaction_cannot_be_reported_idle() {
        assert_eq!(TransactionState::Idle.ready_for_query(), b'I');
        assert_eq!(TransactionState::InTransaction.ready_for_query(), b'T');
        assert_eq!(TransactionState::FailedTransaction.ready_for_query(), b'E');
    }
}
