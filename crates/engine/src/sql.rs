//! Owned synchronous SQL sessions with explicit failure and transaction state.
use std::sync::atomic::AtomicBool;
pub use vetra_executor::{Access, Budget, Column, Error, Output, Relation, Result, Scalar, Type};
use vetra_executor::{Execution, ast, identifier, parse, transaction_error};
use vetra_txn::{Isolation, Manager, Status, Transaction};
pub struct Session {
    manager: Manager,
    pub access: Access,
    pub principal: u64,
    pub budget: Budget,
    transaction: Option<Transaction>,
    transaction_timestamp: i64,
    pub cancel: AtomicBool,
    pub default_isolation: Isolation,
}
impl Session {
    pub fn new(manager: Manager, principal: u64, access: Access) -> Self {
        Self {
            manager,
            access,
            principal,
            budget: Budget::default(),
            transaction: None,
            transaction_timestamp: 0,
            cancel: AtomicBool::new(false),
            default_isolation: Isolation::ReadCommitted,
        }
    }
    pub fn ready(&self) -> u8 {
        match self.transaction.as_ref().and_then(|t| t.status().ok()) {
            Some(Status::Failed | Status::Aborted | Status::Unknown) => b'E',
            Some(Status::Active) => b'T',
            _ => b'I',
        }
    }
    pub fn execute(
        &mut self,
        text: &str,
        parameters: &[Scalar],
        timestamp: i64,
    ) -> Result<Vec<Output>> {
        let statements = match parse(text) {
            Ok(s) => s,
            Err(e) => {
                if let Some(t) = &self.transaction {
                    let _ = t.statement_error();
                }
                return Err(e);
            }
        };
        let controls = statements.iter().any(control);
        let implicit = self.transaction.is_none() && !controls && !statements.is_empty();
        if implicit {
            self.begin(self.default_isolation, timestamp)?
        }
        let mut results = vec![];
        for s in statements {
            match self.statement(&s, parameters, timestamp) {
                Ok(o) => results.push(o),
                Err(e) => {
                    if let Some(t) = &self.transaction {
                        if t.status() == Ok(Status::Active) {
                            let _ = t.statement_error();
                        }
                    }
                    if implicit {
                        if let Some(t) = self.transaction.take() {
                            let _ = t.rollback();
                        }
                    }
                    return Err(e);
                }
            }
        }
        if implicit {
            let t = self.transaction.take().unwrap();
            t.commit(timestamp).map_err(transaction_error)?;
        }
        Ok(results)
    }
    pub fn begin(&mut self, isolation: Isolation, timestamp: i64) -> Result<()> {
        if self.transaction.is_some() {
            return Err(Error::new("25001", "transaction already active"));
        }
        self.transaction = Some(
            self.manager
                .begin(isolation, vetra_catalog::context(self.principal))
                .map_err(transaction_error)?,
        );
        self.transaction_timestamp = timestamp;
        Ok(())
    }
    pub fn statement(
        &mut self,
        s: &ast::Statement,
        parameters: &[Scalar],
        timestamp: i64,
    ) -> Result<Output> {
        match s {
            ast::Statement::StartTransaction { modes, .. } => {
                let mut isolation = self.default_isolation;
                for m in modes {
                    match m {
                        ast::TransactionMode::IsolationLevel(i) => {
                            isolation = match i {
                                ast::TransactionIsolationLevel::ReadUncommitted => {
                                    Isolation::ReadUncommitted
                                }
                                ast::TransactionIsolationLevel::ReadCommitted => {
                                    Isolation::ReadCommitted
                                }
                                ast::TransactionIsolationLevel::RepeatableRead => {
                                    Isolation::RepeatableRead
                                }
                                ast::TransactionIsolationLevel::Serializable => {
                                    Isolation::Serializable
                                }
                            }
                        }
                        _ => return Err(Error::unsupported("transaction access mode")),
                    }
                }
                self.begin(isolation, timestamp)?;
                command("BEGIN")
            }
            ast::Statement::Commit { chain } => {
                if *chain {
                    return Err(Error::unsupported("COMMIT AND CHAIN"));
                }
                let Some(t) = self.transaction.take() else {
                    return command("COMMIT");
                };
                if matches!(
                    t.status().map_err(transaction_error)?,
                    Status::Failed | Status::Aborted
                ) {
                    if t.status() != Ok(Status::Aborted) {
                        t.rollback().map_err(transaction_error)?;
                    }
                    command("ROLLBACK")
                } else {
                    t.commit(timestamp).map_err(transaction_error)?;
                    command("COMMIT")
                }
            }
            ast::Statement::Rollback { chain, savepoint } => {
                if *chain {
                    return Err(Error::unsupported("ROLLBACK AND CHAIN"));
                }
                if let Some(n) = savepoint {
                    let t = self
                        .transaction
                        .as_ref()
                        .ok_or_else(|| Error::new("25P01", "no transaction"))?;
                    t.rollback_to(&identifier(n)).map_err(transaction_error)?;
                } else if let Some(t) = self.transaction.take() {
                    t.rollback().map_err(transaction_error)?;
                }
                command("ROLLBACK")
            }
            ast::Statement::Savepoint { name } | ast::Statement::ReleaseSavepoint { name } => {
                let n = identifier(name);
                if n.starts_with("__vetra_") {
                    return Err(Error::new("42602", "reserved savepoint name"));
                }
                let t = self
                    .transaction
                    .as_ref()
                    .ok_or_else(|| Error::new("25P01", "no transaction"))?;
                if matches!(s, ast::Statement::Savepoint { .. }) {
                    t.savepoint(&n).map_err(transaction_error)?;
                    command("SAVEPOINT")
                } else {
                    t.release(&n).map_err(transaction_error)?;
                    command("RELEASE")
                }
            }
            _ => {
                let implicit = self.transaction.is_none();
                if implicit {
                    self.begin(self.default_isolation, timestamp)?
                }
                let t = self.transaction.as_ref().unwrap();
                let result = vetra_executor::execute(
                    t,
                    &self.manager,
                    s,
                    Execution {
                        parameters,
                        timestamp: self.transaction_timestamp,
                        cancel: &self.cancel,
                        budget: self.budget.clone(),
                        access: self.access.clone(),
                    },
                );
                if implicit {
                    let t = self.transaction.take().unwrap();
                    match result {
                        Ok(o) => {
                            t.commit(timestamp).map_err(transaction_error)?;
                            Ok(o)
                        }
                        Err(e) => {
                            let _ = t.rollback();
                            Err(e)
                        }
                    }
                } else {
                    result
                }
            }
        }
    }
    pub fn prepare(
        &self,
        text: &str,
        parameters: Vec<Option<Type>>,
        _timestamp: i64,
    ) -> Result<vetra_executor::Plan> {
        let owned;
        let tx = if let Some(tx) = &self.transaction {
            tx
        } else {
            owned = self
                .manager
                .begin(
                    self.default_isolation,
                    vetra_catalog::context(self.principal),
                )
                .map_err(transaction_error)?;
            &owned
        };
        let catalog = vetra_catalog::Catalog::load(tx)?;
        let basis = tx.begin_statement().map_err(transaction_error)?;
        let result = vetra_executor::Binder::new(&catalog, parameters).prepare(text, basis);
        tx.end_statement().map_err(transaction_error)?;
        result
    }
    pub fn execute_plan(
        &mut self,
        plan: &vetra_executor::Plan,
        parameters: &[Scalar],
        timestamp: i64,
    ) -> Result<Output> {
        if parameters.len() != plan.parameters.len() {
            return Err(Error::new("08P01", "bound parameter count"));
        }
        let implicit = self.transaction.is_none();
        if implicit {
            self.begin(self.default_isolation, timestamp)?
        }
        let result = (|| {
            let catalog = vetra_catalog::Catalog::load(self.transaction.as_ref().unwrap())?;
            plan.check_epoch(catalog.epoch)?;
            let values = parameters
                .iter()
                .zip(&plan.parameters)
                .map(|(v, t)| v.cast(t.as_ref().unwrap_or(&Type::Text)))
                .collect::<Result<Vec<_>>>()?;
            let mut output = self.statement(&plan.statement, &values, timestamp)?;
            if output.relation.columns.len() != plan.columns.len() {
                return Err(Error::new("0A000", "cached plan result changed"));
            }
            for (c, p) in output.relation.columns.iter_mut().zip(&plan.columns) {
                c.ty = p.ty.clone();
                c.name = p.name.clone();
                c.table = p.table;
                c.id = p.column;
            }
            Ok(output)
        })();
        if implicit {
            let tx = self.transaction.take().unwrap();
            match result {
                Ok(o) => {
                    tx.commit(timestamp).map_err(transaction_error)?;
                    Ok(o)
                }
                Err(e) => {
                    let _ = tx.rollback();
                    Err(e)
                }
            }
        } else {
            if result.is_err() {
                let _ = self.transaction.as_ref().unwrap().statement_error();
            }
            result
        }
    }
    pub fn manager(&self) -> &Manager {
        &self.manager
    }
}
fn command(tag: &str) -> Result<Output> {
    Ok(Output {
        relation: Relation::default(),
        tag: tag.into(),
        affected: 0,
    })
}
fn control(s: &ast::Statement) -> bool {
    matches!(
        s,
        ast::Statement::StartTransaction { .. }
            | ast::Statement::Commit { .. }
            | ast::Statement::Rollback { .. }
            | ast::Statement::Savepoint { .. }
            | ast::Statement::ReleaseSavepoint { .. }
    )
}
