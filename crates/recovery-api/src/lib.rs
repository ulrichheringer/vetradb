//! Owned structural actions. M02 implements durable TOP_BEGIN/PATCH/END framing.
use vetra_types::IoFailure;
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PageImage {
    pub id: u64,
    pub generation: u64,
    pub bytes: Vec<u8>,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StructuralAction {
    pub tree: u64,
    pub previous_root: (u64, u64),
    pub root: (u64, u64),
    pub pages: Vec<PageImage>,
    /// Authoritative allocator state; free-space maps are derived, never authoritative.
    pub allocation: Vec<u8>,
    pub retired: Vec<(u64, u64)>,
    pub record_ids: Vec<(Vec<u8>, u64)>,
    pub identity_high_water: u64,
}
/// Success seals the whole action; failure must expose none of it to replay/readers.
/// A durable provider must sync END before page flush and poison after ambiguous sync failure.
pub trait StructuralJournal {
    fn seal(&mut self, action: StructuralAction) -> Result<u64, IoFailure>;
    /// All enclosed roots publish together or none do.
    fn seal_batch(&mut self, _actions: Vec<StructuralAction>) -> Result<u64, IoFailure> {
        Err(IoFailure::InvalidInput)
    }
}
pub trait WalBarrier {
    fn durable_through(&mut self, lsn: u64) -> Result<(), IoFailure>;
}
/// Explicitly volatile test/bring-up provider. Not a durability implementation.
#[derive(Default, Debug)]
pub struct MemoryJournal {
    pub completed: Vec<(u64, StructuralAction)>,
    pub fail_at: Option<usize>,
    pub batches: Vec<(u64, Vec<StructuralAction>)>,
    pub trace: Vec<JournalStep>,
    pub pending: Vec<PageImage>,
    pub lsn_high_water: u64,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum JournalStep {
    Begin,
    Page(u64),
    Allocation,
    Root,
    End,
}
impl StructuralJournal for MemoryJournal {
    fn seal(&mut self, action: StructuralAction) -> Result<u64, IoFailure> {
        self.prepare(std::slice::from_ref(&action))?;
        let lsn = self.next_lsn()?;
        self.lsn_high_water = lsn;
        self.completed.push((lsn, action));
        self.pending.clear();
        Ok(lsn)
    }
    fn seal_batch(&mut self, actions: Vec<StructuralAction>) -> Result<u64, IoFailure> {
        if actions.is_empty() || actions.len() > 16 {
            return Err(IoFailure::InvalidInput);
        }
        self.prepare(&actions)?;
        let lsn = self.next_lsn()?;
        self.lsn_high_water = lsn;
        self.completed
            .extend(actions.iter().cloned().map(|a| (lsn, a)));
        self.batches.push((lsn, actions));
        self.pending.clear();
        Ok(lsn)
    }
}

impl MemoryJournal {
    fn stage(&mut self, step: JournalStep) -> Result<(), IoFailure> {
        if self.fail_at == Some(self.trace.len()) {
            return Err(IoFailure::DurabilityFailure);
        }
        self.trace.push(step);
        Ok(())
    }
    fn prepare(&mut self, actions: &[StructuralAction]) -> Result<(), IoFailure> {
        self.trace.clear();
        self.pending.clear();
        self.stage(JournalStep::Begin)?;
        for action in actions {
            for p in &action.pages {
                self.stage(JournalStep::Page(p.id))?;
                self.pending.push(p.clone());
            }
            self.stage(JournalStep::Allocation)?;
            self.stage(JournalStep::Root)?;
        }
        self.stage(JournalStep::End)?;
        Ok(())
    }

    fn next_lsn(&self) -> Result<u64, IoFailure> {
        self.completed
            .last()
            .map_or(self.lsn_high_water, |(lsn, _)| {
                (*lsn).max(self.lsn_high_water)
            })
            .checked_add(1)
            .ok_or(IoFailure::InvalidInput)
    }
}
