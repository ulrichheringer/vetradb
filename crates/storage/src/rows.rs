//! Stable row/version identities and primary serving projection; visibility is
//! supplied by txn in M03. This layer never infers commitment from an ID.
use crate::{
    Error, Result,
    tree::{BPlusTree, MAX_KEY, TreeConfig},
};
use vetra_recovery_api::{MemoryJournal, StructuralAction, StructuralJournal};
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct VersionId {
    pub table: u64,
    pub row: u64,
    pub version: u64,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RowVersion {
    pub id: VersionId,
    pub schema: u64,
    pub creator: u64,
    pub primary: Vec<u8>,
    pub payload: Vec<u8>,
    pub deleted: bool,
}
pub trait Visibility {
    fn visible(&self, creator: u64) -> bool;
}
pub struct RowStore {
    versions: BPlusTree,
    directory: BPlusTree,
}
fn key(id: VersionId) -> Result<Vec<u8>> {
    if id.table == 0 || id.row == 0 || id.version == 0 {
        return Err(Error::Corrupt("row identity"));
    }
    let mut b = Vec::with_capacity(24);
    for n in [id.table, id.row, id.version] {
        b.extend(n.to_be_bytes());
    }
    Ok(b)
}
fn primary(table: u64, bytes: &[u8]) -> Result<Vec<u8>> {
    if table == 0 || bytes.len() + 8 > MAX_KEY {
        return Err(Error::Limit);
    }
    let mut k = table.to_be_bytes().to_vec();
    k.extend(bytes);
    Ok(k)
}
impl RowVersion {
    pub fn encode(&self) -> Result<Vec<u8>> {
        key(self.id)?;
        primary(self.id.table, &self.primary)?;
        if self.schema == 0
            || self.creator == 0
            || self.payload.len() > crate::codec::MAX_VALUE - 64 - self.primary.len()
        {
            return Err(Error::Limit);
        }
        let mut b = Vec::new();
        b.extend(b"VROW0001");
        for n in [
            self.id.table,
            self.id.row,
            self.id.version,
            self.schema,
            self.creator,
        ] {
            b.extend(n.to_le_bytes());
        }
        b.push(u8::from(self.deleted));
        b.extend([0; 7]);
        b.extend((self.primary.len() as u32).to_le_bytes());
        b.extend((self.payload.len() as u32).to_le_bytes());
        b.extend(&self.primary);
        b.extend(&self.payload);
        Ok(b)
    }
    pub fn decode(b: &[u8]) -> Result<Self> {
        if b.len() < 64 || &b[..8] != b"VROW0001" || b[48] > 1 || b[49..56] != [0; 7] {
            return Err(Error::Corrupt("row header"));
        }
        let n = |o| u64::from_le_bytes(b[o..o + 8].try_into().unwrap());
        let k = u32::from_le_bytes(b[56..60].try_into().unwrap()) as usize;
        let v = u32::from_le_bytes(b[60..64].try_into().unwrap()) as usize;
        if 64usize.checked_add(k).and_then(|n| n.checked_add(v)) != Some(b.len()) {
            return Err(Error::Corrupt("row framing"));
        }
        let r = Self {
            id: VersionId {
                table: n(8),
                row: n(16),
                version: n(24),
            },
            schema: n(32),
            creator: n(40),
            primary: b[64..64 + k].to_vec(),
            payload: b[64 + k..].to_vec(),
            deleted: b[48] == 1,
        };
        r.encode()?;
        Ok(r)
    }
}
impl RowStore {
    pub fn new(versions_tree: u64, directory_tree: u64, config: TreeConfig) -> Result<Self> {
        if versions_tree == directory_tree {
            return Err(Error::Ownership);
        }
        let versions = BPlusTree::new(versions_tree, config)?;
        let mut directory = BPlusTree::new(directory_tree, config)?;
        directory.share_allocator(&versions);
        Ok(Self {
            versions,
            directory,
        })
    }
    /// One outer structural action seals both projections. Completed row payloads
    /// are never overwritten; logical uniqueness remains a transaction-layer gate.
    pub fn append(
        &mut self,
        row: RowVersion,
        journal: &mut (impl StructuralJournal + ?Sized),
    ) -> Result<()> {
        let k = key(row.id)?;
        if self.versions.get(&k)?.is_some() {
            return Err(Error::Duplicate);
        }
        let data = row.encode()?;
        let mut versions = self.versions.staged_copy();
        let mut directory = self.directory.staged_copy();
        versions.share_allocator(&directory);
        let mut staged = MemoryJournal::default();
        versions.insert(k.clone(), data, row.creator, row.schema, &mut staged)?;
        // Retain old primary entries so old snapshots can find a row after key changes.
        let pk = primary(row.id.table, &row.primary)?;
        let mut candidate = pk.clone();
        candidate.extend(row.id.row.to_be_bytes());
        candidate.extend(row.id.version.to_be_bytes());
        if candidate.len() > MAX_KEY {
            return Err(Error::Limit);
        }
        directory.share_allocator(&versions);
        directory.insert(candidate, k, row.creator, row.schema, &mut staged)?;
        versions.share_allocator(&directory);
        let actions: Vec<_> = staged.completed.into_iter().map(|(_, a)| a).collect();
        let lsn = journal.seal_batch(actions)?;
        if lsn == 0 {
            return Err(Error::WalOrdering);
        }
        versions.stamp(lsn);
        directory.stamp(lsn);
        self.versions = versions;
        self.directory = directory;
        Ok(())
    }
    /// Isolated projection for transaction preparation; publication swaps the whole store.
    pub fn staged_copy(&self) -> Self {
        let versions = self.versions.staged_copy();
        let mut directory = self.directory.staged_copy();
        directory.share_allocator(&versions);
        Self {
            versions,
            directory,
        }
    }
    pub fn all_versions(&self) -> Result<Vec<RowVersion>> {
        self.versions
            .snapshot()?
            .scan(
                std::ops::Bound::Unbounded,
                std::ops::Bound::Unbounded,
                false,
                None,
                self.config().max_records,
            )?
            .into_iter()
            .map(|(_, v)| RowVersion::decode(&v))
            .collect()
    }
    /// Remove obsolete serving entries through the same atomic two-tree boundary.
    pub fn retain(
        &mut self,
        keep: impl Fn(&RowVersion) -> bool,
        journal: &mut (impl StructuralJournal + ?Sized),
    ) -> Result<()> {
        let rows = self.all_versions()?;
        let mut versions = self.versions.staged_copy();
        let mut directory = self.directory.staged_copy();
        for row in rows.into_iter().filter(|r| !keep(r)) {
            let mut staged = MemoryJournal::default();
            versions.share_allocator(&directory);
            versions.delete(&key(row.id)?, &mut staged)?;
            let mut candidate = primary(row.id.table, &row.primary)?;
            candidate.extend(row.id.row.to_be_bytes());
            candidate.extend(row.id.version.to_be_bytes());
            directory.share_allocator(&versions);
            directory.delete(&candidate, &mut staged)?;
            versions.share_allocator(&directory);
            let actions = staged.completed.into_iter().map(|(_, a)| a).collect();
            let lsn = journal.seal_batch(actions)?;
            versions.stamp(lsn);
            directory.stamp(lsn);
        }
        self.versions = versions;
        self.directory = directory;
        self.validate()
    }
    fn versions_owner(&self) -> u64 {
        self.versions.owner()
    }
    fn directory_owner(&self) -> u64 {
        self.directory.owner()
    }
    fn config(&self) -> TreeConfig {
        self.versions.config()
    }
    pub fn version(&self, id: VersionId) -> Result<RowVersion> {
        let bytes = self
            .versions
            .get(&key(id)?)?
            .ok_or(Error::Corrupt("missing row version"))?;
        let row = RowVersion::decode(&bytes)?;
        if row.id != id {
            return Err(Error::Corrupt("row version reference"));
        }
        Ok(row)
    }
    pub fn visible_row(
        &self,
        table: u64,
        row: u64,
        visibility: &impl Visibility,
    ) -> Result<Option<RowVersion>> {
        let snap = self.versions.snapshot()?;
        let mut prefix = table.to_be_bytes().to_vec();
        prefix.extend(row.to_be_bytes());
        let records = snap.scan(
            std::ops::Bound::Included(prefix.as_slice()),
            std::ops::Bound::Unbounded,
            true,
            None,
            self.config().max_records,
        )?;
        for (k, v) in records {
            if !k.starts_with(&prefix) {
                continue;
            }
            let version = RowVersion::decode(&v)?;
            if visibility.visible(version.creator) {
                return Ok(if version.deleted { None } else { Some(version) });
            }
        }
        Ok(None)
    }
    pub fn lookup(
        &self,
        table: u64,
        pk: &[u8],
        visibility: &impl Visibility,
    ) -> Result<Option<RowVersion>> {
        let prefix = primary(table, pk)?;
        let snapshot = self.directory.snapshot()?;
        for (candidate, reference) in snapshot.scan(
            std::ops::Bound::Included(prefix.as_slice()),
            std::ops::Bound::Unbounded,
            true,
            None,
            self.config().max_records,
        )? {
            if candidate.len() != prefix.len() + 16 || !candidate.starts_with(&prefix) {
                continue;
            }
            if reference.len() != 24 || reference[..8] != table.to_be_bytes() {
                return Err(Error::Corrupt("directory reference"));
            }
            let row = u64::from_be_bytes(reference[8..16].try_into().unwrap());
            let id = VersionId {
                table,
                row,
                version: u64::from_be_bytes(reference[16..24].try_into().unwrap()),
            };
            self.version(id)?;
            if let Some(current) = self.visible_row(table, row, visibility)? {
                if current.primary == pk {
                    return Ok(Some(current));
                }
            }
        }
        Ok(None)
    }
    pub fn replay_batch(&mut self, lsn: u64, actions: &[StructuralAction]) -> Result<()> {
        if actions.len() != 2
            || actions[0].tree != self.versions_owner()
            || actions[1].tree != self.directory_owner()
        {
            return Err(Error::Corrupt("row action batch"));
        }
        let mut versions = self.versions.staged_copy();
        let mut directory = self.directory.staged_copy();
        versions.replay(lsn, &actions[0])?;
        directory.replay(lsn, &actions[1])?;
        versions.share_allocator(&directory);
        self.versions = versions;
        self.directory = directory;
        self.validate()
    }
    pub fn validate(&self) -> Result<()> {
        self.versions.validate()?;
        self.directory.validate()
    }
}
