//! Static binding: preparing never executes expressions, allocates identities or mutates data.
use crate::*;
use ast::{Expr, SelectItem, SetExpr, TableFactor};
use std::collections::{BTreeMap, BTreeSet};
use vetra_catalog::{Catalog, qualify};
#[derive(Clone)]
struct Field {
    column: Column,
    qualifier: String,
}
pub struct Binder<'a> {
    pub catalog: &'a Catalog,
    pub parameters: Vec<Option<Type>>,
    pub dependencies: BTreeSet<u64>,
    ctes: BTreeMap<String, Vec<Field>>,
    outer: Vec<Vec<Field>>,
    nodes: usize,
}
impl<'a> Binder<'a> {
    pub fn new(catalog: &'a Catalog, parameters: Vec<Option<Type>>) -> Self {
        Self {
            catalog,
            parameters,
            dependencies: BTreeSet::new(),
            ctes: BTreeMap::new(),
            outer: vec![],
            nodes: 0,
        }
    }
    pub fn prepare(mut self, text: &str, basis: u64) -> Result<Plan> {
        let mut statements = parse(text)?;
        if statements.len() != 1 {
            return Err(Error::new(
                "42601",
                "prepared statement requires one statement",
            ));
        }
        let statement = statements.remove(0);
        let columns = self.statement(&statement)?;
        Ok(Plan {
            statement,
            catalog_epoch: self.catalog.epoch,
            basis,
            parameters: self.parameters,
            columns,
        })
    }
    pub fn statement(&mut self, s: &ast::Statement) -> Result<Vec<Column>> {
        match s {
            ast::Statement::Query(q) => Ok(self.query(q)?.into_iter().map(|f| f.column).collect()),
            ast::Statement::Insert(i) => {
                let t = self.catalog.table(&name(&i.table_name))?.clone();
                let fields = self.table(&t, None);
                let indexes = if i.columns.is_empty() {
                    (0..t.columns.len()).collect::<Vec<_>>()
                } else {
                    i.columns
                        .iter()
                        .map(|i| {
                            t.columns
                                .iter()
                                .position(|c| c.name == identifier(i))
                                .ok_or_else(|| Error::new("42703", "insert column"))
                        })
                        .collect::<Result<Vec<_>>>()?
                };
                if let Some(q) = &i.source {
                    if let SetExpr::Values(v) = q.body.as_ref() {
                        for row in &v.rows {
                            if row.len() != indexes.len() {
                                return Err(Error::new("42601", "insert column count"));
                            }
                            for (e, i) in row.iter().zip(&indexes) {
                                self.expr(e, &[], Some(t.columns[*i].ty.clone()))?;
                            }
                        }
                    } else {
                        let source = self.query(q)?;
                        if source.len() != indexes.len() {
                            return Err(Error::new("42601", "insert column count"));
                        }
                    }
                }
                self.project(i.returning.as_deref().unwrap_or(&[]), &fields)
                    .map(|f| f.into_iter().map(|f| f.column).collect())
            }
            ast::Statement::Update {
                table,
                assignments,
                selection,
                returning,
                ..
            } => {
                let fields = self.factor(&table.relation)?;
                for a in assignments {
                    let ast::AssignmentTarget::ColumnName(n) = &a.target else {
                        return Err(Error::unsupported("tuple assignment"));
                    };
                    let ty = self
                        .resolve(&n.0.iter().map(identifier).collect::<Vec<_>>(), &fields)?
                        .column
                        .ty;
                    self.expr(&a.value, &fields, Some(ty))?;
                }
                if let Some(e) = selection {
                    self.expr(e, &fields, Some(Type::Bool))?;
                }
                self.project(returning.as_deref().unwrap_or(&[]), &fields)
                    .map(|f| f.into_iter().map(|f| f.column).collect())
            }
            ast::Statement::Delete(d) => {
                let from = match &d.from {
                    ast::FromTable::WithFromKeyword(f) | ast::FromTable::WithoutKeyword(f) => f,
                };
                if from.len() != 1 {
                    return Err(Error::unsupported("multi-table delete"));
                }
                let fields = self.factor(&from[0].relation)?;
                if let Some(e) = &d.selection {
                    self.expr(e, &fields, Some(Type::Bool))?;
                }
                self.project(d.returning.as_deref().unwrap_or(&[]), &fields)
                    .map(|f| f.into_iter().map(|f| f.column).collect())
            }
            ast::Statement::CreateTable(_)
            | ast::Statement::CreateSchema { .. }
            | ast::Statement::AlterTable { .. }
            | ast::Statement::CreateIndex(_)
            | ast::Statement::CreateView { .. }
            | ast::Statement::Drop { .. }
            | ast::Statement::StartTransaction { .. }
            | ast::Statement::Commit { .. }
            | ast::Statement::Rollback { .. }
            | ast::Statement::Savepoint { .. }
            | ast::Statement::ReleaseSavepoint { .. } => Ok(vec![]),
            _ => Err(Error::unsupported("statement binding")),
        }
    }
    fn table(&mut self, t: &vetra_catalog::Table, alias: Option<&str>) -> Vec<Field> {
        self.dependencies.insert(t.id);
        t.columns
            .iter()
            .map(|c| Field {
                qualifier: alias
                    .unwrap_or(t.name.rsplit('.').next().unwrap_or(&t.name))
                    .into(),
                column: Column {
                    name: c.name.clone(),
                    ty: c.ty.clone(),
                    table: Some(t.id),
                    column: Some(c.id),
                },
            })
            .collect()
    }
    fn factor(&mut self, f: &TableFactor) -> Result<Vec<Field>> {
        match f {
            TableFactor::Table { name: n, alias, .. } => {
                let n = name(n);
                let mut fields = if let Some(c) = self.ctes.get(&n) {
                    c.clone()
                } else if let Some(v) = self
                    .catalog
                    .views
                    .values()
                    .find(|v| !v.dropped && v.name == qualify(&n))
                {
                    let statements = parse(&v.query)?;
                    let Some(ast::Statement::Query(q)) = statements.first() else {
                        return Err(Error::new("XX001", "view grammar"));
                    };
                    self.query(q)?
                } else {
                    let t = self.catalog.table(&n)?.clone();
                    self.table(&t, None)
                };
                if let Some(a) = alias {
                    for (i, f) in fields.iter_mut().enumerate() {
                        f.qualifier = identifier(&a.name);
                        if let Some(c) = a.columns.get(i) {
                            f.column.name = identifier(&c.name)
                        }
                    }
                }
                Ok(fields)
            }
            TableFactor::Derived {
                subquery, alias, ..
            } => {
                let mut fields = self.query(subquery)?;
                if let Some(a) = alias {
                    for (i, f) in fields.iter_mut().enumerate() {
                        f.qualifier = identifier(&a.name);
                        if let Some(c) = a.columns.get(i) {
                            f.column.name = identifier(&c.name)
                        }
                    }
                }
                Ok(fields)
            }
            _ => Err(Error::unsupported("table binding")),
        }
    }
    fn query(&mut self, q: &ast::Query) -> Result<Vec<Field>> {
        self.nodes += 1;
        if self.nodes > 4096 || self.outer.len() > 64 {
            return Err(Error::new("54000", "binding budget"));
        }
        let saved = self.ctes.clone();
        if let Some(w) = &q.with {
            if w.recursive {
                return Err(Error::unsupported("recursive CTE"));
            }
            for c in &w.cte_tables {
                let mut fields = self.query(&c.query)?;
                for (i, f) in fields.iter_mut().enumerate() {
                    f.qualifier = identifier(&c.alias.name);
                    if let Some(c) = c.alias.columns.get(i) {
                        f.column.name = identifier(&c.name)
                    }
                }
                self.ctes.insert(identifier(&c.alias.name), fields);
            }
        }
        let fields = self.set(q.body.as_ref())?;
        if let Some(order) = &q.order_by {
            for o in &order.exprs {
                if matches!(&o.expr, Expr::Value(ast::Value::Number(..))) {
                    continue;
                }
                if let Err(e) = self.expr(&o.expr, &fields, None) {
                    if e.code == "42702" {
                        return Err(e);
                    }
                    let SetExpr::Select(select) = q.body.as_ref() else {
                        return Err(e);
                    };
                    let mut source = vec![];
                    for from in &select.from {
                        source.extend(self.factor(&from.relation)?);
                        for join in &from.joins {
                            source.extend(self.factor(&join.relation)?);
                        }
                    }
                    self.expr(&o.expr, &source, None)?;
                }
            }
        }
        if let Some(e) = &q.limit {
            self.expr(e, &[], Some(Type::Int8))?;
        }
        if let Some(o) = &q.offset {
            self.expr(&o.value, &[], Some(Type::Int8))?;
        }
        self.ctes = saved;
        Ok(fields)
    }
    fn set(&mut self, s: &SetExpr) -> Result<Vec<Field>> {
        match s {
            SetExpr::Query(q) => self.query(q),
            SetExpr::Select(s) => {
                let mut fields = vec![];
                for f in &s.from {
                    fields.extend(self.factor(&f.relation)?);
                    for j in &f.joins {
                        fields.extend(self.factor(&j.relation)?);
                        match &j.join_operator {
                            ast::JoinOperator::Inner(c)
                            | ast::JoinOperator::LeftOuter(c)
                            | ast::JoinOperator::RightOuter(c)
                            | ast::JoinOperator::FullOuter(c) => {
                                if let ast::JoinConstraint::On(e) = c {
                                    self.expr(e, &fields, Some(Type::Bool))?;
                                }
                            }
                            ast::JoinOperator::CrossJoin => {}
                            _ => return Err(Error::unsupported("join binding")),
                        }
                    }
                }
                if let Some(e) = &s.selection {
                    self.expr(e, &fields, Some(Type::Bool))?;
                }
                if let ast::GroupByExpr::Expressions(es, _) = &s.group_by {
                    for e in es {
                        self.expr(e, &fields, None)?;
                    }
                }
                if let Some(e) = &s.having {
                    self.expr(e, &fields, Some(Type::Bool))?;
                }
                self.project(&s.projection, &fields)
            }
            SetExpr::Values(v) => {
                let mut fields = vec![];
                for row in &v.rows {
                    for (i, e) in row.iter().enumerate() {
                        let ty = self.expr(e, &[], None)?;
                        if fields.len() <= i {
                            fields.push(Field {
                                qualifier: String::new(),
                                column: Column {
                                    name: format!("column{}", i + 1),
                                    ty,
                                    table: None,
                                    column: None,
                                },
                            })
                        }
                    }
                }
                Ok(fields)
            }
            SetExpr::SetOperation { left, right, .. } => {
                let a = self.set(left)?;
                let b = self.set(right)?;
                if a.len() != b.len() {
                    return Err(Error::new("42601", "set columns"));
                }
                Ok(a)
            }
            _ => Err(Error::unsupported("query binding")),
        }
    }
    fn project(&mut self, p: &[SelectItem], input: &[Field]) -> Result<Vec<Field>> {
        let mut out = vec![];
        for item in p {
            match item {
                SelectItem::Wildcard(_) => out.extend(input.to_vec()),
                SelectItem::QualifiedWildcard(n, _) => {
                    let n = name(n);
                    let fields = input
                        .iter()
                        .filter(|f| f.qualifier == n)
                        .cloned()
                        .collect::<Vec<_>>();
                    if fields.is_empty() {
                        return Err(Error::new("42P01", "wildcard relation"));
                    }
                    out.extend(fields)
                }
                SelectItem::UnnamedExpr(e) | SelectItem::ExprWithAlias { expr: e, .. } => {
                    let ty = self.expr(e, input, None)?;
                    let n = match item {
                        SelectItem::ExprWithAlias { alias, .. } => identifier(alias),
                        _ => match e {
                            Expr::Identifier(i) => identifier(i),
                            Expr::CompoundIdentifier(n) => identifier(n.last().unwrap()),
                            Expr::Function(f) => name(&f.name),
                            _ => "?column?".into(),
                        },
                    };
                    let source = match e {
                        Expr::Identifier(i) => Some(self.resolve(&[identifier(i)], input)?),
                        Expr::CompoundIdentifier(n) => Some(
                            self.resolve(&n.iter().map(identifier).collect::<Vec<_>>(), input)?,
                        ),
                        _ => None,
                    };
                    out.push(Field {
                        qualifier: String::new(),
                        column: Column {
                            name: n,
                            ty,
                            table: source.as_ref().and_then(|s| s.column.table),
                            column: source.as_ref().and_then(|s| s.column.column),
                        },
                    })
                }
            }
        }
        Ok(out)
    }
    fn resolve(&self, n: &[String], fields: &[Field]) -> Result<Field> {
        let matches = fields
            .iter()
            .filter(|f| {
                Some(&f.column.name) == n.last()
                    && (n.len() == 1 || Some(&f.qualifier) == n.get(n.len() - 2))
            })
            .collect::<Vec<_>>();
        if matches.len() > 1 {
            return Err(Error::new("42702", "ambiguous column"));
        }
        if let Some(f) = matches.first() {
            return Ok((*f).clone());
        }
        for outer in self.outer.iter().rev() {
            let matches = outer
                .iter()
                .filter(|f| {
                    Some(&f.column.name) == n.last()
                        && (n.len() == 1 || Some(&f.qualifier) == n.get(n.len() - 2))
                })
                .collect::<Vec<_>>();
            if matches.len() > 1 {
                return Err(Error::new("42702", "ambiguous outer column"));
            }
            if let Some(f) = matches.first() {
                return Ok((*f).clone());
            }
        }
        Err(Error::new("42703", "column does not exist"))
    }
    fn expr(&mut self, e: &Expr, c: &[Field], expected: Option<Type>) -> Result<Type> {
        self.nodes += 1;
        if self.nodes > 4096 {
            return Err(Error::new("54000", "expression binding budget"));
        }
        let ty = match e {
            Expr::Value(ast::Value::Placeholder(p)) => {
                let n = p
                    .strip_prefix('$')
                    .and_then(|s| s.parse::<usize>().ok())
                    .filter(|n| *n > 0 && *n <= 1024)
                    .ok_or_else(|| Error::new("42P02", "parameter identity"))?;
                self.parameters.resize(self.parameters.len().max(n), None);
                if let Some(t) = &expected {
                    if self.parameters[n - 1].as_ref().is_some_and(|p| p != t) {
                        return Err(Error::new("42P08", "inconsistent parameter type"));
                    }
                    self.parameters[n - 1] = Some(t.clone())
                }
                self.parameters[n - 1].clone().unwrap_or(Type::Text)
            }
            Expr::Identifier(i) => self.resolve(&[identifier(i)], c)?.column.ty,
            Expr::CompoundIdentifier(n) => {
                self.resolve(&n.iter().map(identifier).collect::<Vec<_>>(), c)?
                    .column
                    .ty
            }
            Expr::Value(ast::Value::Null) => expected.clone().unwrap_or(Type::Text),
            Expr::Value(ast::Value::Boolean(_)) => Type::Bool,
            Expr::Value(ast::Value::Number(s, _)) => {
                if s.parse::<i32>().is_ok() {
                    Type::Int4
                } else if s.parse::<i64>().is_ok() {
                    Type::Int8
                } else {
                    Type::Numeric(None)
                }
            }
            Expr::Value(_) => Type::Text,
            Expr::Cast {
                expr,
                data_type: ty,
                ..
            } => {
                let ty = data_type(ty)?;
                self.expr(expr, c, Some(ty.clone()))?;
                ty
            }
            Expr::TypedString { data_type: ty, .. } => data_type(ty)?,
            Expr::Nested(e) => self.expr(e, c, expected.clone())?,
            Expr::BinaryOp { left, op, right } => {
                let boolean = matches!(op, ast::BinaryOperator::And | ast::BinaryOperator::Or);
                let a = self.expr(left, c, if boolean { Some(Type::Bool) } else { None })?;
                let b = self.expr(right, c, Some(a.clone()))?;
                if matches!(left.as_ref(), Expr::Value(ast::Value::Placeholder(_))) {
                    self.expr(left, c, Some(b.clone()))?;
                }
                match op {
                    ast::BinaryOperator::Eq
                    | ast::BinaryOperator::NotEq
                    | ast::BinaryOperator::Gt
                    | ast::BinaryOperator::GtEq
                    | ast::BinaryOperator::Lt
                    | ast::BinaryOperator::LtEq
                    | ast::BinaryOperator::And
                    | ast::BinaryOperator::Or
                    | ast::BinaryOperator::AtArrow
                    | ast::BinaryOperator::ArrowAt => Type::Bool,
                    ast::BinaryOperator::StringConcat | ast::BinaryOperator::LongArrow => {
                        Type::Text
                    }
                    ast::BinaryOperator::Arrow => Type::Jsonb,
                    _ => a,
                }
            }
            Expr::UnaryOp { op, expr } => self.expr(
                expr,
                c,
                if *op == ast::UnaryOperator::Not {
                    Some(Type::Bool)
                } else {
                    expected.clone()
                },
            )?,
            Expr::IsNull(e)
            | Expr::IsNotNull(e)
            | Expr::IsTrue(e)
            | Expr::IsFalse(e)
            | Expr::IsNotTrue(e)
            | Expr::IsNotFalse(e)
            | Expr::IsUnknown(e)
            | Expr::IsNotUnknown(e) => {
                self.expr(e, c, None)?;
                Type::Bool
            }
            Expr::IsDistinctFrom(a, b) | Expr::IsNotDistinctFrom(a, b) => {
                let ty = self.expr(a, c, None)?;
                self.expr(b, c, Some(ty))?;
                Type::Bool
            }
            Expr::Between {
                expr, low, high, ..
            } => {
                let ty = self.expr(expr, c, None)?;
                self.expr(low, c, Some(ty.clone()))?;
                self.expr(high, c, Some(ty))?;
                Type::Bool
            }
            Expr::InList { expr, list, .. } => {
                let ty = self.expr(expr, c, None)?;
                for e in list {
                    self.expr(e, c, Some(ty.clone()))?;
                }
                Type::Bool
            }
            Expr::Function(f) => {
                let name = name(&f.name);
                if ![
                    "count",
                    "sum",
                    "avg",
                    "min",
                    "max",
                    "abs",
                    "lower",
                    "upper",
                    "length",
                    "octet_length",
                    "coalesce",
                    "nullif",
                    "concat",
                    "now",
                    "current_timestamp",
                    "transaction_timestamp",
                    "jsonb_typeof",
                    "json_typeof",
                    "row_number",
                    "rank",
                    "dense_rank",
                ]
                .contains(&name.as_str())
                {
                    return Err(Error::unsupported("function/signature"));
                }
                let mut types = vec![];
                if let ast::FunctionArguments::List(l) = &f.args {
                    for a in &l.args {
                        if let ast::FunctionArg::Unnamed(ast::FunctionArgExpr::Expr(e)) = a {
                            types.push(self.expr(e, c, None)?);
                        }
                    }
                }
                if let Some(filter) = &f.filter {
                    self.expr(filter, c, Some(Type::Bool))?;
                }
                if let Some(ast::WindowType::WindowSpec(w)) = &f.over {
                    for e in &w.partition_by {
                        self.expr(e, c, None)?;
                    }
                    for o in &w.order_by {
                        self.expr(&o.expr, c, None)?;
                    }
                }
                match name.as_str() {
                    "count" | "row_number" | "rank" | "dense_rank" => Type::Int8,
                    "length" | "octet_length" => Type::Int4,
                    "sum" => {
                        if types
                            .first()
                            .is_some_and(|t| matches!(t, Type::Int2 | Type::Int4))
                        {
                            Type::Int8
                        } else {
                            Type::Numeric(None)
                        }
                    }
                    "avg" => Type::Numeric(None),
                    "lower" | "upper" | "concat" | "jsonb_typeof" | "json_typeof" => Type::Text,
                    "now" | "current_timestamp" | "transaction_timestamp" => Type::Timestamptz,
                    _ => types.first().cloned().unwrap_or(Type::Text),
                }
            }
            Expr::Subquery(q)
            | Expr::Exists { subquery: q, .. }
            | Expr::InSubquery { subquery: q, .. } => {
                self.outer.push(c.to_vec());
                let result = self.query(q);
                self.outer.pop();
                let fields = result?;
                if let Expr::InSubquery { expr, .. } = e {
                    self.expr(expr, c, fields.first().map(|f| f.column.ty.clone()))?;
                }
                if matches!(e, Expr::Subquery(_)) {
                    if fields.len() != 1 {
                        return Err(Error::new("42601", "scalar subquery column count"));
                    }
                    fields[0].column.ty.clone()
                } else {
                    Type::Bool
                }
            }
            Expr::Case {
                operand,
                conditions,
                results,
                else_result,
            } => {
                let condition_type = operand
                    .as_ref()
                    .map(|e| self.expr(e, c, None))
                    .transpose()?
                    .unwrap_or(Type::Bool);
                for e in conditions {
                    self.expr(e, c, Some(condition_type.clone()))?;
                }
                let mut ty = expected.clone();
                for e in results {
                    ty = Some(self.expr(e, c, ty.clone())?)
                }
                if let Some(e) = else_result {
                    ty = Some(self.expr(e, c, ty.clone())?)
                }
                ty.unwrap_or(Type::Text)
            }
            _ => return Err(Error::unsupported(format!("expression binding {e}"))),
        };
        if let Some(Type::Bool) = expected {
            if ty != Type::Bool {
                return Err(Error::new("42804", "boolean expression required"));
            }
        }
        Ok(ty)
    }
}
