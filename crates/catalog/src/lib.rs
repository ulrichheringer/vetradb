//! Transactional catalog snapshots and stable identity reservation.
use std::{
    collections::{BTreeMap, BTreeSet},
    sync::atomic::AtomicBool,
};
pub use vetra_txn;
use vetra_txn::{
    Context, Image, Isolation, Key, Manager, Participant, Transaction, Value,
    locks::{Mode, Resource},
};
use vetra_types::sql::{
    self, Error, Result, Type,
    serde::{Deserialize, Serialize},
    serde_json,
};
pub const CATALOG: u64 = 100;
pub const ROWS: u64 = 101;
pub const ALLOCATOR: u64 = 102;
pub const INDEXES: u64 = 104;
pub const FENCE: u64 = 103;
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(crate = "vetra_types::sql::serde", deny_unknown_fields)]
pub struct Column {
    pub id: u64,
    pub name: String,
    pub ty: Type,
    pub nullable: bool,
    pub default: Option<String>,
    pub generated: Option<String>,
    pub identity: bool,
    pub identity_always: bool,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(crate = "vetra_types::sql::serde", deny_unknown_fields)]
pub struct Index {
    pub id: u64,
    pub name: String,
    pub columns: Vec<u64>,
    pub unique: bool,
    pub primary: bool,
    pub constraint: bool,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(crate = "vetra_types::sql::serde", deny_unknown_fields)]
pub struct ForeignKey {
    pub columns: Vec<u64>,
    pub target: u64,
    pub target_columns: Vec<u64>,
    pub on_delete: String,
    pub on_update: String,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(crate = "vetra_types::sql::serde", deny_unknown_fields)]
pub struct Table {
    pub id: u64,
    pub schema: u64,
    pub name: String,
    pub columns: Vec<Column>,
    pub indexes: Vec<Index>,
    pub checks: Vec<String>,
    pub foreign_keys: Vec<ForeignKey>,
    pub dropped: bool,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(crate = "vetra_types::sql::serde", deny_unknown_fields)]
pub struct View {
    pub id: u64,
    pub name: String,
    pub query: String,
    pub dependencies: BTreeSet<u64>,
    pub dropped: bool,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(crate = "vetra_types::sql::serde", deny_unknown_fields)]
pub struct Catalog {
    pub epoch: u64,
    pub schemas: BTreeSet<String>,
    pub tables: BTreeMap<u64, Table>,
    pub views: BTreeMap<u64, View>,
}
impl Default for Catalog {
    fn default() -> Self {
        Self {
            epoch: 1,
            schemas: BTreeSet::from(["public".into()]),
            tables: BTreeMap::new(),
            views: BTreeMap::new(),
        }
    }
}
pub fn transaction_error(e: vetra_txn::Error) -> Error {
    Error::new(
        match e {
            vetra_txn::Error::Serialization => "40001",
            vetra_txn::Error::Deadlock => "40P01",
            vetra_txn::Error::Unauthorized => "42501",
            vetra_txn::Error::Failed => "25P02",
            vetra_txn::Error::Cancelled => "57014",
            vetra_txn::Error::Timeout => "55P03",
            vetra_txn::Error::Limit => "54000",
            vetra_txn::Error::UnknownOutcome => "08007",
            _ => "XX000",
        },
        format!("transaction: {e:?}"),
    )
}
pub fn context(principal: u64) -> Context {
    Context {
        principal,
        readable: vec![
            (Participant::Schema, CATALOG),
            (Participant::Row, ROWS),
            (Participant::Offset, ALLOCATOR),
            (Participant::Row, INDEXES),
            (Participant::Offset, FENCE),
        ],
        writable: vec![
            (Participant::Schema, CATALOG),
            (Participant::Row, ROWS),
            (Participant::Offset, ALLOCATOR),
            (Participant::Row, INDEXES),
            (Participant::Offset, FENCE),
        ],
        metadata: BTreeMap::new(),
    }
}
pub fn key() -> Key {
    Key {
        participant: Participant::Schema,
        object: CATALOG,
        bytes: b"catalog-v1".to_vec(),
    }
}
impl Catalog {
    pub fn load(tx: &Transaction) -> Result<Self> {
        tx.acquire(Resource::key(&key()), Mode::Shared, &AtomicBool::new(false))
            .map_err(transaction_error)?;
        let value = tx.read(&key()).map_err(transaction_error)?;
        match value {
            None => Ok(Self::default()),
            Some(i) => Self::decode(&i),
        }
    }
    pub fn decode(i: &Image) -> Result<Self> {
        let Some(Value::Bytes(b)) = i.get(&1) else {
            return Err(Error::new("XX001", "catalog payload"));
        };
        if !b.starts_with(b"VCAT0001") || b.len() > sql::MAX_VALUE {
            return Err(Error::new("XX001", "catalog codec"));
        }
        let catalog: Self =
            serde_json::from_slice(&b[8..]).map_err(|_| Error::new("XX001", "catalog encoding"))?;
        catalog.validate()?;
        Ok(catalog)
    }
    pub fn validate(&self) -> Result<()> {
        if self.epoch == 0
            || self.tables.len() > 4096
            || self.views.len() > 4096
            || self.schemas.len() > 256
        {
            return Err(Error::new("XX001", "catalog bounds"));
        }
        let mut ids = BTreeSet::new();
        let mut names = BTreeSet::new();
        for (&id, t) in &self.tables {
            if id == 0
                || id != t.id
                || t.schema == 0
                || !ids.insert(id)
                || t.columns.len() > 256
                || t.name.is_empty()
                || t.name.len() > 1024
            {
                return Err(Error::new("XX001", "table identity"));
            }
            if !t.dropped && !names.insert(&t.name) {
                return Err(Error::new("XX001", "duplicate relation"));
            }
            let mut columns = BTreeSet::new();
            let mut cnames = BTreeSet::new();
            for c in &t.columns {
                if c.id == 0
                    || !ids.insert(c.id)
                    || !columns.insert(c.id)
                    || !cnames.insert(&c.name)
                    || c.name.is_empty()
                    || c.name.len() > 1024
                {
                    return Err(Error::new("XX001", "column identity"));
                }
            }
            for i in &t.indexes {
                if i.id == 0
                    || !ids.insert(i.id)
                    || i.columns.is_empty()
                    || !i.columns.iter().all(|id| columns.contains(id))
                {
                    return Err(Error::new("XX001", "index identity"));
                }
            }
            for fk in t.foreign_keys.iter().filter(|_| !t.dropped) {
                if fk.columns.is_empty()
                    || fk.columns.len() != fk.target_columns.len()
                    || !fk.columns.iter().all(|id| columns.contains(id))
                {
                    return Err(Error::new("XX001", "foreign-key identity"));
                }
                let target = self
                    .tables
                    .get(&fk.target)
                    .ok_or_else(|| Error::new("XX001", "foreign-key target"))?;
                if !fk
                    .target_columns
                    .iter()
                    .all(|id| target.columns.iter().any(|c| c.id == *id))
                {
                    return Err(Error::new("XX001", "foreign-key columns"));
                }
            }
        }
        for (&id, v) in &self.views {
            if id == 0
                || id != v.id
                || !ids.insert(id)
                || v.query.len() > sql::MAX_VALUE
                || v.name.len() > 1024
            {
                return Err(Error::new("XX001", "view identity"));
            }
            if !v.dropped && !names.insert(&v.name) {
                return Err(Error::new("XX001", "duplicate relation"));
            }
            if v.dependencies
                .iter()
                .any(|id| !self.tables.contains_key(id))
            {
                return Err(Error::new("XX001", "view dependency"));
            }
        }
        Ok(())
    }
    pub fn save(&mut self, tx: &Transaction, manager: &Manager, timestamp: i64) -> Result<()> {
        tx.acquire(
            Resource::key(&key()),
            Mode::Exclusive,
            &AtomicBool::new(false),
        )
        .map_err(transaction_error)?;
        self.epoch = allocate(manager, b"identity", timestamp)?;
        self.validate()?;
        let mut b = b"VCAT0001".to_vec();
        b.extend(
            serde_json::to_vec(self).map_err(|_| Error::new("XX001", "catalog serialization"))?,
        );
        if b.len() > sql::MAX_VALUE {
            return Err(Error::new("54000", "catalog budget"));
        }
        tx.put(key(), self.epoch, Image::from([(1, Value::Bytes(b))]))
            .map_err(transaction_error)
    }
    pub fn table(&self, name: &str) -> Result<&Table> {
        let name = qualify(name);
        self.tables
            .values()
            .find(|t| t.name == name && !t.dropped)
            .ok_or_else(|| Error::new("42P01", format!("relation {name} does not exist")))
    }
    pub fn column<'a>(&self, t: &'a Table, name: &str) -> Result<&'a Column> {
        t.columns
            .iter()
            .find(|c| c.name == name)
            .ok_or_else(|| Error::new("42703", format!("column {name} does not exist")))
    }
    pub fn check_drop(&self, id: u64) -> Result<()> {
        if self
            .tables
            .values()
            .any(|t| !t.dropped && t.id != id && t.foreign_keys.iter().any(|f| f.target == id))
            || self
                .views
                .values()
                .any(|v| !v.dropped && v.dependencies.contains(&id))
        {
            return Err(Error::new("2BP01", "dependent objects exist"));
        }
        Ok(())
    }
}
pub fn qualify(s: &str) -> String {
    if s.contains('.') {
        s.into()
    } else {
        format!("public.{s}")
    }
}
/// An internal durable allocation commit: gaps survive user rollback and identities never reuse.
pub fn allocate(manager: &Manager, name: &[u8], timestamp: i64) -> Result<u64> {
    let tx = manager
        .begin(
            Isolation::ReadCommitted,
            Context::object(1, Participant::Offset, ALLOCATOR),
        )
        .map_err(transaction_error)?;
    let key = Key {
        participant: Participant::Offset,
        object: ALLOCATOR,
        bytes: name.to_vec(),
    };
    tx.acquire(
        Resource::key(&key),
        Mode::Exclusive,
        &AtomicBool::new(false),
    )
    .map_err(transaction_error)?;
    let n = match tx
        .read(&key)
        .map_err(transaction_error)?
        .and_then(|i| i.get(&1).cloned())
    {
        None => {
            if name == b"identity" {
                1000
            } else {
                1
            }
        }
        Some(Value::U64(n)) => n
            .checked_add(1)
            .ok_or_else(|| Error::new("54000", "identity exhausted"))?,
        _ => return Err(Error::new("XX001", "identity encoding")),
    };
    tx.put(key, 0, Image::from([(1, Value::U64(n))]))
        .map_err(transaction_error)?;
    tx.commit(timestamp).map_err(transaction_error)?;
    Ok(n)
}
