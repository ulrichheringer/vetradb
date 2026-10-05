use crate::*;
use ast::{ColumnOption as O, Expr, SelectItem, Statement as S, TableConstraint as C};
use catalog::{ForeignKey, Index, Table};
use std::collections::BTreeSet;
use vetra_txn::locks::{Mode, Resource};
impl Runtime<'_> {
    pub fn statement(&mut self, s: &S) -> Result<Output> {
        if let S::Query(q) = s {
            return Ok(Output {
                relation: self.query(q)?,
                tag: "SELECT".into(),
                affected: 0,
            });
        }
        if !matches!(s, S::Insert(_) | S::Update { .. } | S::Delete(_)) && !self.access.admin {
            return Err(Error::new("42501", "DDL requires catalog administration"));
        }
        self.tx
            .acquire(
                Resource::table(Participant::Row, catalog::ROWS),
                Mode::Exclusive,
                self.cancel,
            )
            .map_err(transaction_error)?;
        match s {
            S::CreateSchema {
                schema_name,
                if_not_exists,
            } => {
                let ast::SchemaName::Simple(n) = schema_name else {
                    return Err(Error::unsupported("schema authorization"));
                };
                let n = name(n);
                if !self.catalog.schemas.insert(n) && !*if_not_exists {
                    return Err(Error::new("42P06", "schema already exists"));
                }
                self.save_catalog()?;
                command("CREATE SCHEMA", 0)
            }
            S::CreateTable(c) => {
                if c.query.is_some()
                    || c.like.is_some()
                    || c.clone.is_some()
                    || c.temporary
                    || c.external
                    || c.or_replace
                    || c.transient
                    || c.volatile
                    || !c.with_options.is_empty()
                    || !c.table_properties.is_empty()
                    || c.engine.is_some()
                    || c.partition_by.is_some()
                    || c.collation.is_some()
                {
                    return Err(Error::unsupported("CREATE TABLE options"));
                }
                let name = catalog::qualify(&name(&c.name));
                if self.catalog.table(&name).is_ok() {
                    if c.if_not_exists {
                        return command("CREATE TABLE", 0);
                    }
                    return Err(Error::new("42P07", "relation already exists"));
                }
                let schema = name.split('.').next().unwrap();
                if !self.catalog.schemas.contains(schema) {
                    return Err(Error::new("3F000", "schema does not exist"));
                }
                let id = self.allocate(b"identity")?;
                let mut t = Table {
                    id,
                    schema: self.allocate(b"identity")?,
                    name,
                    columns: vec![],
                    indexes: vec![],
                    checks: vec![],
                    foreign_keys: vec![],
                    dropped: false,
                };
                for c in &c.columns {
                    self.add_column(&mut t, c)?
                }
                if t.columns.is_empty() {
                    return Err(Error::unsupported("zero-column table"));
                }
                for constraint in &c.constraints {
                    self.constraint(&mut t, constraint)?
                }
                self.validate_definitions(&t)?;
                self.catalog.tables.insert(id, t);
                self.save_catalog()?;
                command("CREATE TABLE", 0)
            }
            S::Insert(i) => self.insert(i),
            S::Update {
                table,
                assignments,
                from,
                selection,
                returning,
                or,
            } => {
                if from.is_some() || or.is_some() || !table.joins.is_empty() {
                    return Err(Error::unsupported("UPDATE FROM/options"));
                }
                let t = self.target(&table.relation)?.clone();
                self.access.write(t.id)?;
                let columns = Self::columns(&t, None);
                let mut updates = vec![];
                for (id, row) in self.rows(&t)? {
                    if self.selected(selection.as_ref(), &columns, &row)? {
                        let next = self.assign(&t, &columns, &row, assignments)?;
                        updates.push((id, next))
                    }
                }
                for (id, row) in &updates {
                    self.replace(&t, *id, Some(row.clone()))?
                }
                self.validate_all()?;
                self.rebuild_indexes(&t)?;
                self.returning(
                    &t,
                    updates.iter().map(|(_, r)| r.clone()).collect(),
                    returning.as_deref(),
                    "UPDATE",
                )
            }
            S::Delete(d) => {
                if d.using.is_some()
                    || !d.tables.is_empty()
                    || !d.order_by.is_empty()
                    || d.limit.is_some()
                {
                    return Err(Error::unsupported("DELETE options"));
                }
                let from = match &d.from {
                    ast::FromTable::WithFromKeyword(v) | ast::FromTable::WithoutKeyword(v) => v,
                };
                if from.len() != 1 || !from[0].joins.is_empty() {
                    return Err(Error::unsupported("multi-table DELETE"));
                }
                let t = self.target(&from[0].relation)?.clone();
                self.access.write(t.id)?;
                let columns = Self::columns(&t, None);
                let mut deleted = vec![];
                for (id, row) in self.rows(&t)? {
                    if self.selected(d.selection.as_ref(), &columns, &row)? {
                        self.replace(&t, id, None)?;
                        deleted.push(row)
                    }
                }
                self.validate_all()?;
                self.rebuild_indexes(&t)?;
                self.returning(&t, deleted, d.returning.as_deref(), "DELETE")
            }
            S::CreateIndex(i) => {
                if i.concurrently
                    || i.predicate.is_some()
                    || i.using.as_ref().is_some_and(|u| identifier(u) != "btree")
                    || !i.include.is_empty()
                {
                    return Err(Error::unsupported("index options"));
                }
                let mut t = self.catalog.table(&name(&i.table_name))?.clone();
                let idxname = i
                    .name
                    .as_ref()
                    .map(name)
                    .unwrap_or_else(|| format!("{}_idx", t.name));
                if self
                    .catalog
                    .tables
                    .values()
                    .any(|t| t.indexes.iter().any(|i| i.name == idxname))
                {
                    if i.if_not_exists {
                        return command("CREATE INDEX", 0);
                    }
                    return Err(Error::new("42P07", "index already exists"));
                }
                let mut columns = vec![];
                for c in &i.columns {
                    let Expr::Identifier(n) = &c.expr else {
                        return Err(Error::unsupported("expression index"));
                    };
                    if c.asc == Some(false) || c.nulls_first.is_some() {
                        return Err(Error::unsupported("index ordering options"));
                    }
                    columns.push(self.catalog.column(&t, &identifier(n))?.id)
                }
                t.indexes.push(Index {
                    id: self.allocate(b"identity")?,
                    name: idxname,
                    columns,
                    unique: i.unique,
                    primary: false,
                    constraint: false,
                });
                t.schema = self.allocate(b"identity")?;
                self.validate_definitions(&t)?;
                self.validate_table(&t)?;
                self.rebuild_indexes(&t)?;
                self.catalog.tables.insert(t.id, t);
                self.save_catalog()?;
                command("CREATE INDEX", 0)
            }
            S::CreateView {
                name: n,
                query,
                or_replace,
                materialized,
                temporary,
                columns,
                ..
            } => {
                if *materialized || *temporary || !columns.is_empty() {
                    return Err(Error::unsupported("view options"));
                }
                let n = catalog::qualify(&name(n));
                if self.catalog.table(&n).is_ok() {
                    return Err(Error::new("42P07", "relation already exists"));
                }
                let _ = self.query(query)?;
                let mut binder = Binder::new(&self.catalog, vec![]);
                binder.statement(&S::Query(query.clone()))?;
                let dependencies = binder.dependencies;
                let existing = self
                    .catalog
                    .views
                    .values()
                    .find(|v| !v.dropped && v.name == n)
                    .map(|v| v.id);
                if existing.is_some() && !*or_replace {
                    return Err(Error::new("42P07", "view already exists"));
                }
                let id = existing.unwrap_or(self.allocate(b"identity")?);
                self.catalog.views.insert(
                    id,
                    catalog::View {
                        id,
                        name: n,
                        query: query.to_string(),
                        dependencies,
                        dropped: false,
                    },
                );
                self.save_catalog()?;
                command("CREATE VIEW", 0)
            }
            S::Drop {
                object_type,
                names,
                if_exists,
                cascade,
                restrict: _,
                purge,
                temporary,
            } => {
                if *cascade || *purge || *temporary {
                    return Err(Error::unsupported("DROP CASCADE/options"));
                }
                for n in names {
                    let n = catalog::qualify(&name(n));
                    match object_type {
                        ast::ObjectType::Table => match self.catalog.table(&n) {
                            Ok(t) => {
                                let id = t.id;
                                self.catalog.check_drop(id)?;
                                self.catalog.tables.get_mut(&id).unwrap().dropped = true;
                            }
                            Err(_) if *if_exists => {}
                            Err(e) => return Err(e),
                        },
                        ast::ObjectType::View => {
                            let id = self
                                .catalog
                                .views
                                .values()
                                .find(|v| !v.dropped && v.name == n)
                                .map(|v| v.id);
                            if let Some(id) = id {
                                self.catalog.views.get_mut(&id).unwrap().dropped = true
                            } else if !*if_exists {
                                return Err(Error::new("42P01", "view does not exist"));
                            }
                        }
                        ast::ObjectType::Index => {
                            for table in self.catalog.tables.values().filter(|t| !t.dropped) {
                                for index in table
                                    .indexes
                                    .iter()
                                    .filter(|i| catalog::qualify(&i.name) == n)
                                {
                                    if self.catalog.tables.values().any(|t| {
                                        !t.dropped
                                            && t.foreign_keys.iter().any(|f| {
                                                f.target == table.id
                                                    && f.target_columns == index.columns
                                            })
                                    }) {
                                        return Err(Error::new(
                                            "2BP01",
                                            "index has foreign-key dependencies",
                                        ));
                                    }
                                }
                            }
                            let mut found = false;
                            for t in self.catalog.tables.values_mut() {
                                if let Some(i) = t
                                    .indexes
                                    .iter()
                                    .position(|i| catalog::qualify(&i.name) == n)
                                {
                                    if t.indexes[i].constraint {
                                        return Err(Error::new(
                                            "2BP01",
                                            "constraint index requires constraint drop",
                                        ));
                                    }
                                    t.indexes.remove(i);
                                    found = true;
                                }
                            }
                            if !found && !*if_exists {
                                return Err(Error::new("42704", "index does not exist"));
                            }
                        }
                        _ => return Err(Error::unsupported("DROP object")),
                    }
                }
                self.save_catalog()?;
                command("DROP", 0)
            }
            S::AlterTable {
                name: n,
                operations,
                ..
            } => {
                let mut t = self.catalog.table(&name(n))?.clone();
                let original = t.clone();
                for op in operations {
                    match op {
                        ast::AlterTableOperation::RenameTable { table_name } => {
                            let n = catalog::qualify(&name(table_name));
                            if self.catalog.table(&n).is_ok() {
                                return Err(Error::new("42P07", "relation exists"));
                            }
                            for v in self
                                .catalog
                                .views
                                .values_mut()
                                .filter(|v| !v.dropped && v.dependencies.contains(&t.id))
                            {
                                v.query = rename_relation(&v.query, &t.name, &n)?;
                            }
                            t.name = n
                        }
                        ast::AlterTableOperation::RenameColumn {
                            old_column_name,
                            new_column_name,
                        } => {
                            self.catalog.check_drop(t.id)?;
                            let old = identifier(old_column_name);
                            let new = identifier(new_column_name);
                            if t.columns.iter().any(|c| c.name == new) {
                                return Err(Error::new("42701", "column exists"));
                            }
                            let col = t
                                .columns
                                .iter_mut()
                                .find(|c| c.name == old)
                                .ok_or_else(|| Error::new("42703", "column missing"))?;
                            col.name = new.clone();
                            for check in &mut t.checks {
                                *check = rename_column(check, &old, &new)?;
                            }
                            for c in &mut t.columns {
                                if let Some(e) = &mut c.generated {
                                    *e = rename_column(e, &old, &new)?;
                                }
                            }
                        }
                        ast::AlterTableOperation::AddColumn {
                            column_def,
                            if_not_exists,
                            column_position,
                            ..
                        } => {
                            if column_position.is_some() {
                                return Err(Error::unsupported("column position"));
                            }
                            if t.columns
                                .iter()
                                .any(|c| c.name == identifier(&column_def.name))
                            {
                                if !*if_not_exists {
                                    return Err(Error::new("42701", "column exists"));
                                }
                            } else {
                                self.add_column(&mut t, column_def)?
                            }
                        }
                        ast::AlterTableOperation::DropColumn {
                            column_name,
                            if_exists,
                            cascade,
                        } => {
                            if *cascade {
                                return Err(Error::unsupported("DROP COLUMN CASCADE"));
                            }
                            self.catalog.check_drop(t.id)?;
                            let n = identifier(column_name);
                            if let Some(i) = t.columns.iter().position(|c| c.name == n) {
                                let id = t.columns[i].id;
                                if t.indexes.iter().any(|i| i.columns.contains(&id))
                                    || t.foreign_keys.iter().any(|f| f.columns.contains(&id))
                                    || !t.checks.is_empty()
                                {
                                    return Err(Error::new("2BP01", "column dependencies"));
                                }
                                t.columns.remove(i);
                            } else if !*if_exists {
                                return Err(Error::new("42703", "column missing"));
                            }
                        }
                        ast::AlterTableOperation::AlterColumn { column_name, op } => {
                            let n = identifier(column_name);
                            let c = t
                                .columns
                                .iter_mut()
                                .find(|c| c.name == n)
                                .ok_or_else(|| Error::new("42703", "column missing"))?;
                            match op {
                                ast::AlterColumnOperation::SetNotNull => c.nullable = false,
                                ast::AlterColumnOperation::DropNotNull => c.nullable = true,
                                ast::AlterColumnOperation::SetDefault { value } => {
                                    c.default = Some(value.to_string())
                                }
                                ast::AlterColumnOperation::DropDefault => c.default = None,
                                ast::AlterColumnOperation::SetDataType {
                                    data_type: ty,
                                    using,
                                } => {
                                    if using.is_some() {
                                        return Err(Error::unsupported("ALTER TYPE USING"));
                                    }
                                    self.catalog.check_drop(t.id)?;
                                    c.ty = data_type(ty)?
                                }
                                _ => return Err(Error::unsupported("ALTER COLUMN")),
                            }
                        }
                        ast::AlterTableOperation::AddConstraint(c) => self.constraint(&mut t, c)?,
                        _ => return Err(Error::unsupported("ALTER TABLE operation")),
                    }
                }
                t.schema = self.allocate(b"identity")?;
                for (id, row) in self.rows(&original)? {
                    let mut new = vec![];
                    for c in &t.columns {
                        let value =
                            if let Some(i) = original.columns.iter().position(|o| o.id == c.id) {
                                row[i].cast(&c.ty)?
                            } else {
                                self.default(c, &t, &new)?
                            };
                        new.push(value)
                    }
                    self.replace(&t, id, Some(new))?
                }
                self.validate_definitions(&t)?;
                self.validate_definitions(&t)?;
                self.validate_table(&t)?;
                self.rebuild_indexes(&t)?;
                self.catalog.tables.insert(t.id, t);
                self.save_catalog()?;
                command("ALTER TABLE", 0)
            }
            _ => Err(Error::unsupported(format!("statement {s}"))),
        }
    }
    fn allocate(&self, name: &[u8]) -> Result<u64> {
        catalog::allocate(self.manager, name, self.timestamp)
    }
    fn save_catalog(&mut self) -> Result<()> {
        self.catalog.save(self.tx, self.manager, self.timestamp)
    }
    fn target(&self, f: &ast::TableFactor) -> Result<&Table> {
        let ast::TableFactor::Table {
            name: n,
            alias,
            args,
            ..
        } = f
        else {
            return Err(Error::unsupported("DML table"));
        };
        if alias.is_some() || args.is_some() {
            return Err(Error::unsupported("DML table alias/options"));
        }
        self.catalog.table(&name(n))
    }
    fn selected(&self, e: Option<&Expr>, c: &[Column], r: &[Scalar]) -> Result<bool> {
        e.map(|e| self.eval(e, c, r, None).and_then(|v| v.truth()))
            .transpose()
            .map(|v| v.is_none() || v.flatten() == Some(true))
    }
    fn add_column(&self, t: &mut Table, def: &ast::ColumnDef) -> Result<()> {
        let column_name = identifier(&def.name);
        if t.columns.iter().any(|c| c.name == column_name) {
            return Err(Error::new("42701", "column already exists"));
        }
        if def.collation.is_some() {
            return Err(Error::unsupported("column collation"));
        }
        let mut column = catalog::Column {
            id: self.allocate(b"identity")?,
            name: column_name,
            ty: data_type(&def.data_type)?,
            nullable: true,
            default: None,
            generated: None,
            identity: false,
            identity_always: false,
        };
        for option in &def.options {
            match &option.option {
                O::Null => column.nullable = true,
                O::NotNull => column.nullable = false,
                O::Default(e) => column.default = Some(e.to_string()),
                O::Unique {
                    is_primary,
                    characteristics,
                } => {
                    if characteristics.is_some() {
                        return Err(Error::unsupported("constraint characteristics"));
                    }
                    if *is_primary {
                        column.nullable = false
                    }
                    t.indexes.push(Index {
                        id: self.allocate(b"identity")?,
                        name: format!("{}_{}_key", t.name, column.name),
                        columns: vec![column.id],
                        unique: true,
                        primary: *is_primary,
                        constraint: true,
                    })
                }
                O::Check(e) => t.checks.push(e.to_string()),
                O::ForeignKey {
                    foreign_table,
                    referred_columns,
                    on_delete,
                    on_update,
                    characteristics,
                } => {
                    if characteristics.is_some() {
                        return Err(Error::unsupported("deferred FK"));
                    }
                    let target = self.catalog.table(&name(foreign_table))?;
                    t.foreign_keys.push(ForeignKey {
                        columns: vec![column.id],
                        target: target.id,
                        target_columns: referred_columns
                            .iter()
                            .map(|c| self.catalog.column(target, &identifier(c)).map(|c| c.id))
                            .collect::<Result<Vec<_>>>()?,
                        on_delete: action(on_delete)?,
                        on_update: action(on_update)?,
                    })
                }
                O::Generated {
                    generation_expr,
                    generated_as,
                    sequence_options,
                    ..
                } => {
                    if sequence_options.as_ref().is_some_and(|s| !s.is_empty()) {
                        return Err(Error::unsupported("identity sequence options"));
                    }
                    if let Some(e) = generation_expr {
                        column.generated = Some(e.to_string())
                    } else {
                        column.identity = true;
                        column.identity_always = *generated_as == ast::GeneratedAs::Always;
                        column.nullable = false
                    }
                }
                _ => return Err(Error::unsupported("column option")),
            }
        }
        if t.indexes
            .iter()
            .any(|i| i.primary && i.columns.contains(&column.id))
        {
            column.nullable = false;
        }
        t.columns.push(column);
        Ok(())
    }
    fn constraint(&self, t: &mut Table, c: &C) -> Result<()> {
        if matches!(
            c,
            C::Unique {
                nulls_distinct: ast::NullsDistinctOption::NotDistinct,
                ..
            }
        ) {
            return Err(Error::unsupported("NULLS NOT DISTINCT"));
        }
        match c {
            C::PrimaryKey {
                columns,
                name: n,
                characteristics,
                ..
            }
            | C::Unique {
                columns,
                name: n,
                characteristics,
                ..
            } => {
                if characteristics.is_some() {
                    return Err(Error::unsupported("deferred constraints"));
                }
                let primary = matches!(c, C::PrimaryKey { .. });
                let ids = columns
                    .iter()
                    .map(|c| self.catalog.column(t, &identifier(c)).map(|c| c.id))
                    .collect::<Result<Vec<_>>>()?;
                if primary {
                    if t.indexes.iter().any(|i| i.primary) {
                        return Err(Error::new("42P16", "multiple primary keys"));
                    }
                    for col in &mut t.columns {
                        if ids.contains(&col.id) {
                            col.nullable = false
                        }
                    }
                }
                t.indexes.push(Index {
                    id: self.allocate(b"identity")?,
                    name: n
                        .as_ref()
                        .map(identifier)
                        .unwrap_or_else(|| format!("{}_key{}", t.name, t.indexes.len())),
                    columns: ids,
                    primary,
                    constraint: true,
                    unique: true,
                });
            }
            C::Check { expr, .. } => t.checks.push(expr.to_string()),
            C::ForeignKey {
                columns,
                foreign_table,
                referred_columns,
                on_delete,
                on_update,
                characteristics,
                ..
            } => {
                if characteristics.is_some() {
                    return Err(Error::unsupported("deferred FK"));
                }
                let target = if catalog::qualify(&name(foreign_table)) == t.name {
                    &*t
                } else {
                    self.catalog.table(&name(foreign_table))?
                };
                let target_id = target.id;
                let target_columns = referred_columns
                    .iter()
                    .map(|c| self.catalog.column(target, &identifier(c)).map(|c| c.id))
                    .collect::<Result<Vec<_>>>()?;
                t.foreign_keys.push(ForeignKey {
                    columns: columns
                        .iter()
                        .map(|c| self.catalog.column(t, &identifier(c)).map(|c| c.id))
                        .collect::<Result<Vec<_>>>()?,
                    target: target_id,
                    target_columns,
                    on_delete: action(on_delete)?,
                    on_update: action(on_update)?,
                })
            }
            _ => return Err(Error::unsupported("table constraint")),
        }
        Ok(())
    }
    fn validate_definitions(&self, t: &Table) -> Result<()> {
        for index in &t.indexes {
            for id in &index.columns {
                if t.columns
                    .iter()
                    .any(|c| c.id == *id && matches!(c.ty, Type::Json | Type::Jsonb))
                {
                    return Err(Error::unsupported("JSON index ordering"));
                }
            }
        }
        let columns = Self::columns(t, None);
        let row = vec![Scalar::Null; t.columns.len()];
        for check in &t.checks {
            immutable(&expression(check)?)?;
            self.eval(&expression(check)?, &columns, &row, None)?
                .truth()?;
        }
        for c in &t.columns {
            if let Some(e) = &c.default {
                default_expression(&expression(e)?)?;
                self.eval(&expression(e)?, &[], &[], None)?.cast(&c.ty)?;
            }
            if let Some(e) = &c.generated {
                immutable(&expression(e)?)?;
                self.eval(&expression(e)?, &columns, &row, None)?
                    .cast(&c.ty)?;
            }
        }
        for fk in &t.foreign_keys {
            let target = if fk.target == t.id {
                t
            } else {
                self.catalog
                    .tables
                    .get(&fk.target)
                    .ok_or_else(|| Error::new("42P01", "FK target"))?
            };
            if fk.columns.len() != fk.target_columns.len()
                || fk.columns.is_empty()
                || !target
                    .indexes
                    .iter()
                    .any(|i| i.unique && i.columns == fk.target_columns)
            {
                return Err(Error::new("42830", "FK requires matching unique key"));
            }
        }
        Ok(())
    }
    fn default(&self, c: &catalog::Column, t: &Table, row: &[Scalar]) -> Result<Scalar> {
        let v = if c.identity {
            Scalar::Int(
                i64::try_from(self.allocate(format!("sequence:{}:{}", t.id, c.id).as_bytes())?)
                    .map_err(|_| Error::new("22003", "sequence overflow"))?,
            )
        } else if let Some(e) = &c.default {
            self.eval(&expression(e)?, &[], &[], None)?
        } else {
            let _ = row;
            Scalar::Null
        };
        v.cast(&c.ty)
    }
    fn generated(&self, t: &Table, row: &mut [Scalar]) -> Result<()> {
        for (i, c) in t.columns.iter().enumerate() {
            if let Some(e) = &c.generated {
                row[i] = self
                    .eval(&expression(e)?, &Self::columns(t, None), row, None)?
                    .cast(&c.ty)?
            }
        }
        Ok(())
    }
    fn replace(&self, t: &Table, id: u64, row: Option<Vec<Scalar>>) -> Result<()> {
        // One shared constraint epoch makes RR writes conservatively reject stale constraint bases.
        let fence = Key {
            participant: Participant::Offset,
            object: catalog::FENCE,
            bytes: b"constraint-epoch".to_vec(),
        };
        self.tx
            .put(fence, 0, Image::from([(1, Value::I64(self.timestamp))]))
            .map_err(transaction_error)?;
        match row {
            Some(mut r) => {
                self.generated(t, &mut r)?;
                self.tx
                    .put(Self::row_key(t.id, id), t.schema, Self::image(t, &r)?)
                    .map_err(transaction_error)
            }
            None => self
                .tx
                .delete(Self::row_key(t.id, id), t.schema)
                .map_err(transaction_error),
        }
    }
    fn assign(
        &self,
        t: &Table,
        columns: &[Column],
        row: &[Scalar],
        assignments: &[ast::Assignment],
    ) -> Result<Vec<Scalar>> {
        let mut next = row[..t.columns.len()].to_vec();
        let mut seen = BTreeSet::new();
        for a in assignments {
            let ast::AssignmentTarget::ColumnName(n) = &a.target else {
                return Err(Error::unsupported("tuple assignment"));
            };
            let n = name(n);
            let index = t
                .columns
                .iter()
                .position(|c| c.name == n)
                .ok_or_else(|| Error::new("42703", "assignment column"))?;
            if !seen.insert(index) {
                return Err(Error::new("42601", "duplicate assignment"));
            }
            if t.columns[index].generated.is_some() {
                return Err(Error::new("428C9", "generated column write"));
            }
            next[index] = self
                .eval(&a.value, columns, row, None)?
                .cast(&t.columns[index].ty)?;
        }
        self.generated(t, &mut next)?;
        Ok(next)
    }
    fn insert(&self, i: &ast::Insert) -> Result<Output> {
        if i.overwrite
            || i.ignore
            || i.or.is_some()
            || i.partitioned.is_some()
            || i.replace_into
            || i.table_alias.is_some()
        {
            return Err(Error::unsupported("INSERT options"));
        }
        let t = self.catalog.table(&name(&i.table_name))?.clone();
        self.access.write(t.id)?;
        let target = match &i.on {
            None => None,
            Some(ast::OnInsert::OnConflict(on)) => match &on.conflict_target {
                None => {
                    if matches!(on.action, ast::OnConflictAction::DoUpdate(_)) {
                        return Err(Error::new("42601", "DO UPDATE requires conflict target"));
                    }
                    None
                }
                Some(ast::ConflictTarget::Columns(columns)) => {
                    let ids = columns
                        .iter()
                        .map(|c| self.catalog.column(&t, &identifier(c)).map(|c| c.id))
                        .collect::<Result<Vec<_>>>()?;
                    Some(
                        t.indexes
                            .iter()
                            .find(|i| i.unique && i.columns == ids)
                            .ok_or_else(|| Error::new("42P10", "conflict target"))?
                            .id,
                    )
                }
                _ => return Err(Error::unsupported("named conflict target")),
            },
            _ => return Err(Error::unsupported("INSERT conflict mode")),
        };
        let indexes = if i.columns.is_empty() {
            (0..t.columns.len()).collect::<Vec<_>>()
        } else {
            let mut indexes = vec![];
            for n in &i.columns {
                let index = t
                    .columns
                    .iter()
                    .position(|c| c.name == identifier(n))
                    .ok_or_else(|| Error::new("42703", "insert column"))?;
                if indexes.contains(&index) {
                    return Err(Error::new("42701", "duplicate insert column"));
                }
                indexes.push(index)
            }
            indexes
        };
        let source = if let Some(q) = &i.source {
            self.query(q)?.rows
        } else {
            vec![vec![]]
        };
        let mut inserted = vec![];
        let mut affected_ids = BTreeSet::new();
        for values in source {
            if i.source.is_some() && values.len() != indexes.len() {
                return Err(Error::new("42601", "insert column count"));
            }
            let mut row = vec![Scalar::Null; t.columns.len()];
            for (j, c) in t.columns.iter().enumerate() {
                if let Some(k) = indexes.iter().position(|i| *i == j) {
                    if let Some(v) = values.get(k) {
                        if c.generated.is_some() || c.identity_always {
                            return Err(Error::new("428C9", "generated column write"));
                        }
                        row[j] = v.cast(&c.ty)?;
                    } else {
                        row[j] = self.default(c, &t, &row)?
                    }
                } else {
                    row[j] = self.default(c, &t, &row)?
                }
            }
            self.generated(&t, &mut row)?;
            let existing = self.conflict(&t, &row)?;
            let mut id = None;
            if let Some((old_id, old_row, conflict_index)) = existing {
                if target.is_some_and(|t| t != conflict_index) {
                    return Err(Error::new("23505", "conflict outside target"));
                }
                if affected_ids.contains(&old_id)
                    && matches!(&i.on,Some(ast::OnInsert::OnConflict(on))if matches!(on.action,ast::OnConflictAction::DoUpdate(_)))
                {
                    return Err(Error::new(
                        "21000",
                        "ON CONFLICT cannot affect one row twice",
                    ));
                }
                match &i.on {
                    Some(ast::OnInsert::OnConflict(on)) => match &on.action {
                        ast::OnConflictAction::DoNothing => continue,
                        ast::OnConflictAction::DoUpdate(update) => {
                            let Some(ast::ConflictTarget::Columns(target)) = &on.conflict_target
                            else {
                                return Err(Error::unsupported("upsert conflict target"));
                            };
                            let ids = target
                                .iter()
                                .map(|c| self.catalog.column(&t, &identifier(c)).map(|c| c.id))
                                .collect::<Result<Vec<_>>>()?;
                            if !t.indexes.iter().any(|i| i.unique && i.columns == ids) {
                                return Err(Error::new("42P10", "conflict target"));
                            }
                            let mut columns = Self::columns(&t, None);
                            let mut joined = old_row.clone();
                            let mut excluded = Self::columns(&t, Some("excluded"));
                            columns.append(&mut excluded);
                            joined.extend(row.clone());
                            if !self.selected(update.selection.as_ref(), &columns, &joined)? {
                                continue;
                            }
                            row = self.assign(&t, &columns, &joined, &update.assignments)?;
                            id = Some(old_id);
                        }
                    },
                    _ => return Err(Error::new("23505", "unique constraint")),
                }
            }
            let id = match id {
                Some(id) => id,
                None => self.allocate(b"identity")?,
            };
            affected_ids.insert(id);
            self.replace(&t, id, Some(row.clone()))?;
            self.validate_table(&t)?;
            inserted.push(row);
        }
        self.rebuild_indexes(&t)?;
        self.returning(&t, inserted, i.returning.as_deref(), "INSERT")
    }
    fn conflict(&self, t: &Table, row: &[Scalar]) -> Result<Option<(u64, Vec<Scalar>, u64)>> {
        for i in &t.indexes {
            if !i.unique {
                continue;
            }
            let indexes = positions(t, &i.columns)?;
            let key = indexes.iter().map(|i| row[*i].clone()).collect::<Vec<_>>();
            if key.contains(&Scalar::Null) {
                continue;
            }
            for (id, r) in self.rows(t)? {
                let other = indexes.iter().map(|i| r[*i].clone()).collect::<Vec<_>>();
                if relational::row_equal(&key, &other)? {
                    return Ok(Some((id, r, i.id)));
                }
            }
        }
        Ok(None)
    }
    fn validate_table(&self, t: &Table) -> Result<()> {
        let rows = self.rows(t)?;
        for (_, row) in &rows {
            for (c, v) in t.columns.iter().zip(row) {
                if !c.nullable && v == &Scalar::Null {
                    return Err(Error::new("23502", format!("NOT NULL {}", c.name)));
                }
            }
            for check in &t.checks {
                if self
                    .eval(&expression(check)?, &Self::columns(t, None), row, None)?
                    .truth()?
                    == Some(false)
                {
                    return Err(Error::new("23514", "check constraint"));
                }
            }
            for fk in &t.foreign_keys {
                let key = positions(t, &fk.columns)?
                    .into_iter()
                    .map(|i| row[i].clone())
                    .collect::<Vec<_>>();
                if key.contains(&Scalar::Null) {
                    continue;
                }
                let target = if fk.target == t.id {
                    t
                } else {
                    self.catalog
                        .tables
                        .get(&fk.target)
                        .filter(|t| !t.dropped)
                        .ok_or_else(|| Error::new("23503", "FK target missing"))?
                };
                let positions = positions(target, &fk.target_columns)?;
                let mut found = false;
                for (_, r) in self.rows(target)? {
                    let other = positions.iter().map(|i| r[*i].clone()).collect::<Vec<_>>();
                    if relational::row_equal(&key, &other)? {
                        found = true;
                        break;
                    }
                }
                if !found {
                    return Err(Error::new("23503", "foreign key constraint"));
                }
            }
        }
        for index in &t.indexes {
            if !index.unique {
                continue;
            }
            let indexes = positions(t, &index.columns)?;
            let mut keys: Vec<Vec<Scalar>> = vec![];
            for (_, r) in &rows {
                let k = indexes.iter().map(|i| r[*i].clone()).collect::<Vec<_>>();
                if k.contains(&Scalar::Null) && !index.primary {
                    continue;
                }
                for old in &keys {
                    if relational::row_equal(old, &k)? {
                        return Err(Error::new("23505", "unique constraint"));
                    }
                }
                keys.push(k)
            }
        }
        Ok(())
    }
    fn validate_all(&self) -> Result<()> {
        for t in self.catalog.tables.values().filter(|t| !t.dropped) {
            self.validate_table(t)?
        }
        Ok(())
    }
    fn rebuild_indexes(&self, t: &Table) -> Result<()> {
        for index in &t.indexes {
            let prefix = index.id.to_be_bytes().to_vec();
            let mut upper = prefix.clone();
            upper.extend(vec![255; 2040]);
            for (k, _) in self
                .tx
                .scan(
                    Participant::Row,
                    catalog::INDEXES,
                    Some(prefix.clone()),
                    Some(upper),
                )
                .map_err(transaction_error)?
            {
                self.tx.delete(k, t.schema).map_err(transaction_error)?;
            }
            for (id, row) in self.rows(t)? {
                let values = positions(t, &index.columns)?
                    .into_iter()
                    .map(|i| row[i].clone())
                    .collect::<Vec<_>>();
                let mut bytes = prefix.clone();
                for v in &values {
                    bytes.extend(v.ordered()?)
                }
                if !index.unique || values.contains(&Scalar::Null) {
                    bytes.extend(id.to_be_bytes())
                }
                let key = Key {
                    participant: Participant::Row,
                    object: catalog::INDEXES,
                    bytes,
                };
                self.tx
                    .put(key, t.schema, Image::from([(1, Value::U64(id))]))
                    .map_err(transaction_error)?;
            }
        }
        Ok(())
    }
    fn returning(
        &self,
        t: &Table,
        rows: Vec<Vec<Scalar>>,
        items: Option<&[SelectItem]>,
        tag: &str,
    ) -> Result<Output> {
        let affected = rows.len() as u64;
        let relation = if let Some(items) = items {
            let columns = Self::columns(t, None);
            let exprs = self.expand(items, &columns)?;
            let outcolumns = exprs
                .iter()
                .map(|(n, e)| {
                    Ok(Column {
                        name: n.clone(),
                        qualifier: String::new(),
                        ty: self.infer(e, &columns)?,
                        table: Some(t.id),
                        id: None,
                    })
                })
                .collect::<Result<Vec<_>>>()?;
            let mut outrows = vec![];
            for row in rows {
                outrows.push(
                    exprs
                        .iter()
                        .map(|(_, e)| self.eval(e, &columns, &row, None))
                        .collect::<Result<Vec<_>>>()?,
                )
            }
            Relation {
                columns: outcolumns,
                rows: outrows,
            }
        } else {
            Relation::default()
        };
        self.bounded(&relation)?;
        Ok(Output {
            relation,
            tag: tag.into(),
            affected,
        })
    }
}
fn positions(t: &Table, ids: &[u64]) -> Result<Vec<usize>> {
    ids.iter()
        .map(|id| {
            t.columns
                .iter()
                .position(|c| c.id == *id)
                .ok_or_else(|| Error::new("XX001", "column identity"))
        })
        .collect()
}
fn action(a: &Option<ast::ReferentialAction>) -> Result<String> {
    match a {
        None | Some(ast::ReferentialAction::NoAction) | Some(ast::ReferentialAction::Restrict) => {
            Ok("RESTRICT".into())
        }
        _ => Err(Error::unsupported(
            "FK action; initially RESTRICT/NO ACTION",
        )),
    }
}
fn command(tag: &str, affected: u64) -> Result<Output> {
    Ok(Output {
        relation: Relation::default(),
        tag: tag.into(),
        affected,
    })
}
