//! Synchronous SQL execution over native transactions. All effects share the WAL boundary.
pub mod eval;
pub mod relational;
pub mod statements;
use std::{
    cell::{Cell, RefCell},
    collections::BTreeMap,
    sync::atomic::{AtomicBool, Ordering},
};
pub use vetra_catalog::{self as catalog, Catalog, transaction_error};
pub use vetra_planner::{
    Plan, ast, bind::Binder, data_type, default_expression, expression, identifier, immutable,
    name, parse, rename_column, rename_relation,
};
use vetra_txn::{Image, Key, Manager, Participant, Transaction, Value};
pub use vetra_types::sql::{Error, Result, Scalar, Type};
#[derive(Clone, Debug)]
pub struct Column {
    pub name: String,
    pub qualifier: String,
    pub ty: Type,
    pub table: Option<u64>,
    pub id: Option<u64>,
}
#[derive(Clone, Debug, Default)]
pub struct Relation {
    pub columns: Vec<Column>,
    pub rows: Vec<Vec<Scalar>>,
}
#[derive(Clone, Debug)]
pub struct Output {
    pub relation: Relation,
    pub tag: String,
    pub affected: u64,
}
#[derive(Clone, Debug)]
pub struct Budget {
    pub rows: usize,
    pub bytes: usize,
    pub steps: usize,
}
impl Default for Budget {
    fn default() -> Self {
        Self {
            rows: 10000,
            bytes: 16 * 1024 * 1024,
            steps: 1_000_000,
        }
    }
}
#[derive(Clone, Debug, Default)]
pub struct Access {
    pub admin: bool,
    pub readable: std::collections::BTreeSet<u64>,
    pub writable: std::collections::BTreeSet<u64>,
}
impl Access {
    pub fn read(&self, table: u64) -> Result<()> {
        if self.admin || self.readable.contains(&table) || self.writable.contains(&table) {
            Ok(())
        } else {
            Err(Error::new("42501", "relation read denied"))
        }
    }
    pub fn write(&self, table: u64) -> Result<()> {
        if self.admin || self.writable.contains(&table) {
            Ok(())
        } else {
            Err(Error::new("42501", "relation write denied"))
        }
    }
}
pub struct Execution<'a> {
    pub parameters: &'a [Scalar],
    pub timestamp: i64,
    pub cancel: &'a AtomicBool,
    pub budget: Budget,
    pub access: Access,
}
pub struct Runtime<'a> {
    pub tx: &'a Transaction,
    pub manager: &'a Manager,
    pub catalog: Catalog,
    pub parameters: &'a [Scalar],
    pub timestamp: i64,
    pub cancel: &'a AtomicBool,
    pub budget: Budget,
    pub access: Access,
    pub ctes: RefCell<BTreeMap<String, Relation>>,
    pub steps: Cell<usize>,
    pub depth: Cell<usize>,
    pub outer: RefCell<Vec<(Vec<Column>, Vec<Scalar>)>>,
}
impl<'a> Runtime<'a> {
    pub fn new(
        tx: &'a Transaction,
        manager: &'a Manager,
        execution: Execution<'a>,
    ) -> Result<Self> {
        Ok(Self {
            tx,
            manager,
            catalog: Catalog::load(tx)?,
            parameters: execution.parameters,
            timestamp: execution.timestamp,
            cancel: execution.cancel,
            budget: execution.budget,
            access: execution.access,
            ctes: RefCell::new(BTreeMap::new()),
            steps: Cell::new(0),
            depth: Cell::new(0),
            outer: RefCell::new(vec![]),
        })
    }
    pub fn tick(&self) -> Result<()> {
        if self.cancel.load(Ordering::Relaxed) {
            return Err(Error::new("57014", "query cancelled"));
        }
        let n = self
            .steps
            .get()
            .checked_add(1)
            .ok_or_else(|| Error::new("54000", "execution budget"))?;
        self.steps.set(n);
        if n > self.budget.steps {
            return Err(Error::new("54000", "execution budget"));
        }
        Ok(())
    }
    pub fn bounded(&self, r: &Relation) -> Result<()> {
        self.tick()?;
        if r.rows.len() > self.budget.rows {
            return Err(Error::new("54000", "row budget"));
        }
        let mut bytes = 0usize;
        for row in &r.rows {
            for v in row {
                self.tick()?;
                bytes = bytes
                    .checked_add(v.encode()?.len())
                    .ok_or_else(|| Error::new("54000", "memory budget"))?;
                if bytes > self.budget.bytes {
                    return Err(Error::new("54000", "operator memory budget"));
                }
            }
        }
        Ok(())
    }
    pub fn rows(&self, t: &catalog::Table) -> Result<Vec<(u64, Vec<Scalar>)>> {
        let lo = t.id.to_be_bytes().to_vec();
        let mut hi = lo.clone();
        hi.extend([255; 8]);
        let rows = self
            .tx
            .scan(Participant::Row, catalog::ROWS, Some(lo), Some(hi))
            .map_err(transaction_error)?
            .into_iter()
            .map(|(k, i)| {
                if k.bytes.len() != 16 {
                    return Err(Error::new("XX001", "SQL row key"));
                }
                let id = u64::from_be_bytes(k.bytes[8..].try_into().unwrap());
                let values = t
                    .columns
                    .iter()
                    .map(|c| match i.get(&c.id) {
                        None | Some(Value::Null) => Ok(Scalar::Null),
                        Some(Value::Bytes(b)) => Scalar::decode(b),
                        _ => Err(Error::new("XX001", "SQL row payload")),
                    })
                    .collect::<Result<Vec<_>>>()?;
                Ok((id, values))
            })
            .collect::<Result<Vec<_>>>()?;
        self.bounded(&Relation {
            columns: Self::columns(t, None),
            rows: rows.iter().map(|(_, r)| r.clone()).collect(),
        })?;
        Ok(rows)
    }
    pub fn index_point(
        &self,
        t: &catalog::Table,
        index: &catalog::Index,
        values: &[Scalar],
    ) -> Result<Vec<Vec<Scalar>>> {
        self.access.read(t.id)?;
        if values.len() != index.columns.len() {
            return Err(Error::new("42601", "index key arity"));
        }
        let mut prefix = index.id.to_be_bytes().to_vec();
        for (id, value) in index.columns.iter().zip(values) {
            let c = t
                .columns
                .iter()
                .find(|c| c.id == *id)
                .ok_or_else(|| Error::new("XX001", "index column"))?;
            prefix.extend(value.cast(&c.ty)?.ordered()?);
        }
        if prefix.len() > 2048 {
            return Err(Error::new("54000", "index key budget"));
        }
        let mut upper = prefix.clone();
        upper.resize(2048, 255);
        let mut rows = vec![];
        for (key, image) in self
            .tx
            .scan(
                Participant::Row,
                catalog::INDEXES,
                Some(prefix.clone()),
                Some(upper),
            )
            .map_err(transaction_error)?
        {
            if !key.bytes.starts_with(&prefix) {
                continue;
            }
            let Some(Value::U64(id)) = image.get(&1) else {
                return Err(Error::new("XX001", "index pointer"));
            };
            let image = self
                .tx
                .read(&Self::row_key(t.id, *id))
                .map_err(transaction_error)?
                .ok_or_else(|| Error::new("XX001", "visible index points to absent row"))?;
            rows.push(
                t.columns
                    .iter()
                    .map(|c| match image.get(&c.id) {
                        None | Some(Value::Null) => Ok(Scalar::Null),
                        Some(Value::Bytes(b)) => Scalar::decode(b),
                        _ => Err(Error::new("XX001", "row codec")),
                    })
                    .collect::<Result<Vec<_>>>()?,
            );
            self.bounded(&Relation {
                columns: vec![],
                rows: rows.clone(),
            })?;
        }
        Ok(rows)
    }
    pub fn row_key(table: u64, row: u64) -> Key {
        let mut bytes = table.to_be_bytes().to_vec();
        bytes.extend(row.to_be_bytes());
        Key {
            participant: Participant::Row,
            object: catalog::ROWS,
            bytes,
        }
    }
    pub fn image(t: &catalog::Table, row: &[Scalar]) -> Result<Image> {
        t.columns
            .iter()
            .zip(row)
            .map(|(c, v)| Ok((c.id, Value::Bytes(v.encode()?))))
            .collect()
    }
    pub fn columns(t: &catalog::Table, alias: Option<&str>) -> Vec<Column> {
        t.columns
            .iter()
            .map(|c| Column {
                name: c.name.clone(),
                qualifier: alias
                    .unwrap_or(t.name.rsplit('.').next().unwrap_or(&t.name))
                    .into(),
                ty: c.ty.clone(),
                table: Some(t.id),
                id: Some(c.id),
            })
            .collect()
    }
}
pub fn execute(
    tx: &Transaction,
    manager: &Manager,
    statement: &ast::Statement,
    execution: Execution<'_>,
) -> Result<Output> {
    let resource = vetra_txn::locks::Resource::key(&catalog::key());
    tx.acquire(resource, vetra_txn::locks::Mode::Shared, execution.cancel)
        .map_err(transaction_error)?;
    if !matches!(statement, ast::Statement::Query(_)) {
        tx.acquire(
            vetra_txn::locks::Resource::table(Participant::Row, catalog::ROWS),
            vetra_txn::locks::Mode::Exclusive,
            execution.cancel,
        )
        .map_err(transaction_error)?;
    }
    tx.savepoint("__vetra_statement")
        .map_err(transaction_error)?;
    tx.begin_statement().map_err(transaction_error)?;
    let result = (|| {
        let catalog = Catalog::load(tx)?;
        let mut binder = Binder::new(&catalog, vec![]);
        let columns = binder.statement(statement)?;
        if binder.parameters.len() != execution.parameters.len() {
            return Err(Error::new("42P02", "parameter count"));
        }
        let parameters = execution
            .parameters
            .iter()
            .zip(&binder.parameters)
            .map(|(v, t)| v.cast(t.as_ref().unwrap_or(&Type::Text)))
            .collect::<Result<Vec<_>>>()?;
        let mut rt = Runtime::new(
            tx,
            manager,
            Execution {
                parameters: &parameters,
                timestamp: execution.timestamp,
                cancel: execution.cancel,
                budget: execution.budget,
                access: execution.access,
            },
        )?;
        let mut output = rt.statement(statement)?;
        if output.relation.columns.len() == columns.len() {
            for (c, bound) in output.relation.columns.iter_mut().zip(columns) {
                c.ty = bound.ty;
                c.table = bound.table;
                c.id = bound.column;
            }
        }
        Ok(output)
    })();
    tx.end_statement().map_err(transaction_error)?;
    match result {
        Ok(o) => {
            tx.release("__vetra_statement").map_err(transaction_error)?;
            Ok(o)
        }
        Err(e) => {
            if matches!(
                tx.status(),
                Ok(vetra_txn::Status::Active | vetra_txn::Status::Failed)
            ) {
                tx.rollback_to("__vetra_statement")
                    .map_err(transaction_error)?;
                tx.release("__vetra_statement").map_err(transaction_error)?;
                tx.statement_error().map_err(transaction_error)?;
            }
            Err(e)
        }
    }
}
