use crate::*;
use ast::{Expr, GroupByExpr, JoinConstraint, JoinOperator, SelectItem, SetExpr, TableFactor};
use std::cmp::Ordering;
impl Runtime<'_> {
    pub fn query(&self, q: &ast::Query) -> Result<Relation> {
        self.tick()?;
        let depth = self.depth.get();
        if depth >= 64 {
            return Err(Error::new("54000", "query nesting"));
        }
        self.depth.set(depth + 1);
        let old = self.ctes.borrow().clone();
        let result = self.query_inner(q);
        self.ctes.replace(old);
        self.depth.set(depth);
        result
    }
    fn query_inner(&self, q: &ast::Query) -> Result<Relation> {
        if q.fetch.is_some()
            || !q.locks.is_empty()
            || !q.limit_by.is_empty()
            || q.for_clause.is_some()
            || q.settings.is_some()
            || q.format_clause.is_some()
        {
            return Err(Error::unsupported("query options"));
        }
        if let Some(with) = &q.with {
            if with.recursive {
                return Err(Error::unsupported("recursive CTE"));
            }
            for cte in &with.cte_tables {
                let mut rel = self.query(&cte.query)?;
                alias(&mut rel, &cte.alias)?;
                self.ctes
                    .borrow_mut()
                    .insert(identifier(&cte.alias.name), rel);
            }
        }
        let mut rel = match q.body.as_ref() {
            SetExpr::Select(s) => self.select(s, q.order_by.as_ref())?,
            _ => {
                let mut rel = self.set(q.body.as_ref())?;
                if let Some(order) = &q.order_by {
                    self.sort(&mut rel, &order.exprs)?
                }
                rel
            }
        };
        let offset = q
            .offset
            .as_ref()
            .map(|o| self.limit_value(&o.value))
            .transpose()?
            .unwrap_or(0);
        let limit = q
            .limit
            .as_ref()
            .map(|e| {
                if matches!(e, Expr::Value(ast::Value::Null)) {
                    Ok(usize::MAX)
                } else {
                    self.limit_value(e)
                }
            })
            .transpose()?
            .unwrap_or(usize::MAX);
        rel.rows = rel.rows.into_iter().skip(offset).take(limit).collect();
        self.bounded(&rel)?;
        Ok(rel)
    }
    fn limit_value(&self, e: &Expr) -> Result<usize> {
        match self.eval(e, &[], &[], None)? {
            Scalar::Int(n) if n >= 0 => {
                usize::try_from(n).map_err(|_| Error::new("22003", "limit range"))
            }
            Scalar::Null => Ok(0),
            _ => Err(Error::new("2201W", "nonnegative integer limit required")),
        }
    }
    pub fn set(&self, s: &SetExpr) -> Result<Relation> {
        match s {
            SetExpr::Select(s) => self.select(s, None),
            SetExpr::Query(q) => self.query(q),
            SetExpr::Values(v) => {
                if v.explicit_row {
                    return Err(Error::unsupported("explicit ROW"));
                }
                let mut rel = Relation::default();
                for row in &v.rows {
                    let row = row
                        .iter()
                        .map(|e| self.eval(e, &[], &[], None))
                        .collect::<Result<Vec<_>>>()?;
                    if rel.columns.is_empty() {
                        rel.columns = row
                            .iter()
                            .enumerate()
                            .map(|(i, v)| Column {
                                name: format!("column{}", i + 1),
                                qualifier: String::new(),
                                ty: scalar_type(v),
                                table: None,
                                id: None,
                            })
                            .collect()
                    }
                    if row.len() != rel.columns.len() {
                        return Err(Error::new("42601", "VALUES column count"));
                    }
                    rel.rows.push(row);
                    self.bounded(&rel)?
                }
                Ok(rel)
            }
            SetExpr::SetOperation {
                op,
                set_quantifier,
                left,
                right,
            } => {
                let mut a = self.set(left)?;
                let b = self.set(right)?;
                if a.columns.len() != b.columns.len() {
                    return Err(Error::new("42601", "set operation column count"));
                }
                let all = matches!(set_quantifier, ast::SetQuantifier::All);
                match op {
                    ast::SetOperator::Union => {
                        a.rows.extend(b.rows);
                        if !all {
                            distinct(&mut a)?
                        }
                    }
                    ast::SetOperator::Intersect | ast::SetOperator::Except => {
                        if all {
                            return Err(Error::unsupported("INTERSECT/EXCEPT ALL"));
                        }
                        let mut rows = vec![];
                        for row in a.rows {
                            let present =
                                b.rows.iter().any(|b| row_equal(&row, b).unwrap_or(false));
                            if present == matches!(op, ast::SetOperator::Intersect) {
                                rows.push(row)
                            }
                        }
                        a.rows = rows;
                        distinct(&mut a)?
                    }
                }
                self.bounded(&a)?;
                Ok(a)
            }
            _ => Err(Error::unsupported("query body")),
        }
    }
    pub fn factor(&self, f: &TableFactor) -> Result<Relation> {
        match f {
            TableFactor::Table {
                name: n,
                alias: a,
                args,
                with_hints,
                version,
                partitions,
                ..
            } => {
                if args.is_some()
                    || !with_hints.is_empty()
                    || version.is_some()
                    || !partitions.is_empty()
                {
                    return Err(Error::unsupported("table options"));
                }
                let n = name(n);
                let mut rel = if let Some(r) = self.ctes.borrow().get(&n) {
                    r.clone()
                } else if let Some(v) = self
                    .catalog
                    .views
                    .values()
                    .find(|v| !v.dropped && v.name == catalog::qualify(&n))
                {
                    let statements = parse(&v.query)?;
                    let Some(ast::Statement::Query(q)) = statements.first() else {
                        return Err(Error::new("XX001", "view query"));
                    };
                    self.query(q)?
                } else {
                    let t = self.catalog.table(&n)?;
                    self.access.read(t.id)?;
                    Relation {
                        columns: Self::columns(t, None),
                        rows: self.rows(t)?.into_iter().map(|(_, r)| r).collect(),
                    }
                };
                if let Some(a) = a {
                    alias(&mut rel, a)?
                }
                self.bounded(&rel)?;
                Ok(rel)
            }
            TableFactor::Derived {
                subquery,
                alias: a,
                lateral,
            } => {
                if *lateral {
                    return Err(Error::unsupported("LATERAL"));
                }
                let mut rel = self.query(subquery)?;
                if let Some(a) = a {
                    alias(&mut rel, a)?
                }
                Ok(rel)
            }
            _ => Err(Error::unsupported("table factor")),
        }
    }
    fn select(&self, s: &ast::Select, order: Option<&ast::OrderBy>) -> Result<Relation> {
        if s.top.is_some()
            || s.into.is_some()
            || !s.lateral_views.is_empty()
            || s.prewhere.is_some()
            || !s.cluster_by.is_empty()
            || !s.distribute_by.is_empty()
            || !s.sort_by.is_empty()
            || !s.named_window.is_empty()
            || s.qualify.is_some()
            || s.value_table_mode.is_some()
            || s.connect_by.is_some()
        {
            return Err(Error::unsupported("SELECT options"));
        }
        let mut input = Relation {
            columns: vec![],
            rows: vec![vec![]],
        };
        for from in &s.from {
            let mut right = self.factor(&from.relation)?;
            for join in &from.joins {
                right = self.join(right, self.factor(&join.relation)?, &join.join_operator)?
            }
            input = self.join(input, right, &JoinOperator::CrossJoin)?
        }
        if s.from.len() == 1 && s.from[0].joins.is_empty() {
            if let (
                TableFactor::Table {
                    name: n, alias: a, ..
                },
                Some(predicate),
            ) = (&s.from[0].relation, &s.selection)
            {
                if let (false, Ok(table)) = (
                    self.ctes.borrow().contains_key(&name(n)),
                    self.catalog.table(&name(n)),
                ) {
                    if let Some(equalities) = point_equalities(predicate) {
                        for index in &table.indexes {
                            let mut values = vec![];
                            let mut eligible = true;
                            for id in &index.columns {
                                let c = table
                                    .columns
                                    .iter()
                                    .find(|c| c.id == *id)
                                    .ok_or_else(|| Error::new("XX001", "index column"))?;
                                if let Some((_, value)) =
                                    equalities.iter().find(|(n, _)| n == &c.name)
                                {
                                    values.push(self.eval(value, &[], &[], None)?);
                                } else {
                                    eligible = false;
                                    break;
                                }
                            }
                            if eligible {
                                input.rows = self.index_point(table, index, &values)?;
                                input.columns = Self::columns(
                                    table,
                                    a.as_ref().map(|a| identifier(&a.name)).as_deref(),
                                );
                                break;
                            }
                        }
                    }
                }
            }
        }
        let dummy = vec![Scalar::Null; input.columns.len()];
        if let Some(filter) = &s.selection {
            self.eval(filter, &input.columns, &dummy, None)?.truth()?;
            let mut rows = vec![];
            for row in input.rows {
                if self.eval(filter, &input.columns, &row, None)?.truth()? == Some(true) {
                    rows.push(row)
                }
            }
            input.rows = rows;
        }
        let group_exprs = match &s.group_by {
            GroupByExpr::Expressions(e, options) if options.is_empty() => e,
            _ => return Err(Error::unsupported("grouping sets")),
        };
        let expressions = self.expand(&s.projection, &input.columns)?;
        let aggregated = !group_exprs.is_empty()
            || expressions.iter().any(|(_, e)| aggregate(e))
            || s.having.as_ref().is_some_and(aggregate);
        if aggregated {
            for (_, e) in &expressions {
                check_group(e, group_exprs, false)?
            }
            if let Some(h) = &s.having {
                check_group(h, group_exprs, false)?
            }
        }
        let mut groups: Vec<Vec<Vec<Scalar>>> = vec![];
        if aggregated {
            if group_exprs.is_empty() {
                groups.push(input.rows.clone())
            } else {
                let mut keys: Vec<Vec<Scalar>> = vec![];
                for row in &input.rows {
                    let key = group_exprs
                        .iter()
                        .map(|e| self.eval(e, &input.columns, row, None))
                        .collect::<Result<Vec<_>>>()?;
                    let mut index = None;
                    for (i, k) in keys.iter().enumerate() {
                        if row_equal(k, &key)? {
                            index = Some(i);
                            break;
                        }
                    }
                    let i = index.unwrap_or_else(|| {
                        keys.push(key);
                        groups.push(vec![]);
                        groups.len() - 1
                    });
                    groups[i].push(row.clone())
                }
            }
        } else {
            groups = input.rows.iter().map(|r| vec![r.clone()]).collect()
        }
        let mut output = Relation {
            columns: expressions
                .iter()
                .map(|(n, e)| {
                    Ok(Column {
                        name: n.clone(),
                        qualifier: String::new(),
                        ty: self.infer(e, &input.columns)?,
                        table: None,
                        id: None,
                    })
                })
                .collect::<Result<Vec<_>>>()?,
            rows: vec![],
        };
        // Resolve expressions even on an empty input so invalid names/functions never disappear.
        for (_, e) in &expressions {
            if !window(e) {
                self.eval(
                    e,
                    &input.columns,
                    &dummy,
                    if aggregated { Some(&[]) } else { None },
                )?;
            }
        }
        let mut sort_keys = vec![];
        for (source_position, group) in groups.into_iter().enumerate() {
            let source = group.first().unwrap_or(&dummy);
            let g = if aggregated {
                Some(group.as_slice())
            } else {
                None
            };
            if let Some(h) = &s.having {
                if self.eval(h, &input.columns, source, g)?.truth()? != Some(true) {
                    continue;
                }
            }
            let mut row = vec![];
            for (_, e) in &expressions {
                row.push(if let Expr::Function(f) = e {
                    if f.over.is_some() {
                        self.window(f, &input, source, source_position)?
                    } else {
                        self.eval(e, &input.columns, source, g)?
                    }
                } else {
                    self.eval(e, &input.columns, source, g)?
                })
            }
            if let Some(order) = order {
                let mut keys = vec![];
                for o in &order.exprs {
                    let value = match &o.expr {
                        Expr::Value(ast::Value::Number(n, _)) => {
                            let n = n
                                .parse::<usize>()
                                .map_err(|_| Error::new("42P10", "ORDER BY position"))?;
                            row.get(n.wrapping_sub(1))
                                .cloned()
                                .ok_or_else(|| Error::new("42P10", "ORDER BY position"))?
                        }
                        Expr::Identifier(i)
                            if output.columns.iter().any(|c| c.name == identifier(i)) =>
                        {
                            self.resolve(&[identifier(i)], &output.columns, &row)?
                        }
                        e => self.eval(e, &input.columns, source, g)?,
                    };
                    keys.push(value)
                }
                sort_keys.push(keys)
            }
            output.rows.push(row);
            self.bounded(&output)?
        }
        if let Some(d) = &s.distinct {
            if !matches!(d, ast::Distinct::Distinct) {
                return Err(Error::unsupported("DISTINCT ON"));
            }
            if order.is_some() {
                let mut seen: Vec<Vec<Scalar>> = vec![];
                let mut rows = vec![];
                let mut keys = vec![];
                for (row, key) in output.rows.into_iter().zip(sort_keys) {
                    if !seen.iter().any(|s| row_equal(s, &row).unwrap_or(false)) {
                        seen.push(row.clone());
                        rows.push(row);
                        keys.push(key)
                    }
                }
                output.rows = rows;
                sort_keys = keys
            } else {
                distinct(&mut output)?
            }
        }
        if let Some(order) = order {
            sort_by_keys(&mut output.rows, &sort_keys, &order.exprs)?
        }
        Ok(output)
    }
    pub fn expand(&self, items: &[SelectItem], columns: &[Column]) -> Result<Vec<(String, Expr)>> {
        let mut result = vec![];
        for item in items {
            match item {
                SelectItem::UnnamedExpr(e) => result.push((
                    match e {
                        Expr::Identifier(i) => identifier(i),
                        Expr::CompoundIdentifier(v) => identifier(v.last().unwrap()),
                        Expr::Function(f) => name(&f.name),
                        _ => "?column?".into(),
                    },
                    e.clone(),
                )),
                SelectItem::ExprWithAlias { expr, alias } => {
                    result.push((identifier(alias), expr.clone()))
                }
                SelectItem::Wildcard(_) => {
                    for c in columns {
                        result.push((
                            c.name.clone(),
                            Expr::CompoundIdentifier(vec![
                                ast::Ident::with_quote('"', &c.qualifier),
                                ast::Ident::with_quote('"', &c.name),
                            ]),
                        ))
                    }
                }
                SelectItem::QualifiedWildcard(n, _) => {
                    let n = name(n);
                    let cs = columns
                        .iter()
                        .filter(|c| c.qualifier == n)
                        .collect::<Vec<_>>();
                    if cs.is_empty() {
                        return Err(Error::new("42P01", "qualified wildcard"));
                    }
                    for c in cs {
                        result.push((
                            c.name.clone(),
                            Expr::CompoundIdentifier(vec![
                                ast::Ident::with_quote('"', &c.qualifier),
                                ast::Ident::with_quote('"', &c.name),
                            ]),
                        ))
                    }
                }
            }
        }
        Ok(result)
    }
    pub fn infer(&self, e: &Expr, c: &[Column]) -> Result<Type> {
        Ok(match e {
            Expr::Identifier(i) => column_type(&[identifier(i)], c)?,
            Expr::CompoundIdentifier(n) => {
                column_type(&n.iter().map(identifier).collect::<Vec<_>>(), c)?
            }
            Expr::Cast { data_type: ty, .. } | Expr::TypedString { data_type: ty, .. } => {
                data_type(ty)?
            }
            Expr::Function(f) => match name(&f.name).as_str() {
                "count" | "length" | "octet_length" | "row_number" | "rank" | "dense_rank" => {
                    Type::Int8
                }
                "avg" => Type::Numeric(None),
                "sum" => {
                    let arg = eval::function_args(f)?
                        .first()
                        .copied()
                        .ok_or_else(|| Error::new("42883", "sum arity"))?;
                    let ast::FunctionArgExpr::Expr(e) = arg else {
                        return Err(Error::new("42883", "sum argument"));
                    };
                    match self.infer(e, c)? {
                        Type::Int2 | Type::Int4 => Type::Int8,
                        Type::Float4 | Type::Float8 => Type::Float8,
                        _ => Type::Numeric(None),
                    }
                }
                "now" | "current_timestamp" | "transaction_timestamp" => Type::Timestamptz,
                _ => eval::function_args(f)?
                    .first()
                    .and_then(|a| {
                        if let ast::FunctionArgExpr::Expr(e) = a {
                            Some(e)
                        } else {
                            None
                        }
                    })
                    .map(|e| self.infer(e, c))
                    .transpose()?
                    .unwrap_or(Type::Text),
            },
            Expr::BinaryOp { op, left, .. } => match op {
                ast::BinaryOperator::Eq
                | ast::BinaryOperator::NotEq
                | ast::BinaryOperator::Lt
                | ast::BinaryOperator::Gt
                | ast::BinaryOperator::LtEq
                | ast::BinaryOperator::GtEq
                | ast::BinaryOperator::And
                | ast::BinaryOperator::Or
                | ast::BinaryOperator::AtArrow
                | ast::BinaryOperator::ArrowAt => Type::Bool,
                ast::BinaryOperator::LongArrow | ast::BinaryOperator::StringConcat => Type::Text,
                ast::BinaryOperator::Arrow => Type::Jsonb,
                _ => self.infer(left, c)?,
            },
            Expr::Nested(e) | Expr::UnaryOp { expr: e, .. } => self.infer(e, c)?,
            Expr::IsNull(_)
            | Expr::IsNotNull(_)
            | Expr::Exists { .. }
            | Expr::InList { .. }
            | Expr::InSubquery { .. }
            | Expr::Between { .. } => Type::Bool,
            Expr::Value(ast::Value::Placeholder(p)) => {
                let n = p
                    .trim_start_matches('$')
                    .parse::<usize>()
                    .map_err(|_| Error::new("42P02", "parameter"))?;
                self.parameters
                    .get(n.wrapping_sub(1))
                    .map(scalar_type)
                    .ok_or_else(|| Error::new("42P02", "parameter count"))?
            }
            Expr::Value(ast::Value::Number(n, _)) if n.parse::<i32>().is_ok() => Type::Int4,
            _ => scalar_type(&self.eval(e, c, &vec![Scalar::Null; c.len()], None)?),
        })
    }
    pub fn join(&self, a: Relation, b: Relation, op: &JoinOperator) -> Result<Relation> {
        let (constraint, left, right) = match op {
            JoinOperator::Inner(c) => (Some(c), false, false),
            JoinOperator::LeftOuter(c) => (Some(c), true, false),
            JoinOperator::RightOuter(c) => (Some(c), false, true),
            JoinOperator::FullOuter(c) => (Some(c), true, true),
            JoinOperator::CrossJoin => (None, false, false),
            _ => return Err(Error::unsupported("join kind")),
        };
        let mut columns = a.columns.clone();
        columns.extend(b.columns.clone());
        let mut out = Relation {
            columns,
            rows: vec![],
        };
        let mut matched = vec![false; b.rows.len()];
        for ar in &a.rows {
            let mut found = false;
            for (i, br) in b.rows.iter().enumerate() {
                self.tick()?;
                let mut row = ar.clone();
                row.extend(br.clone());
                let yes = match constraint {
                    None | Some(JoinConstraint::None) => true,
                    Some(JoinConstraint::On(e)) => {
                        self.eval(e, &out.columns, &row, None)?.truth()? == Some(true)
                    }
                    Some(JoinConstraint::Using(_)) | Some(JoinConstraint::Natural) => {
                        return Err(Error::unsupported("USING/NATURAL join"));
                    }
                };
                if yes {
                    found = true;
                    matched[i] = true;
                    out.rows.push(row);
                    self.bounded(&out)?
                }
            }
            if left && !found {
                let mut row = ar.clone();
                row.extend(vec![Scalar::Null; b.columns.len()]);
                out.rows.push(row);
                self.bounded(&out)?
            }
        }
        if right {
            for (i, br) in b.rows.into_iter().enumerate() {
                if !matched[i] {
                    let mut row = vec![Scalar::Null; a.columns.len()];
                    row.extend(br);
                    out.rows.push(row);
                    self.bounded(&out)?
                }
            }
        }
        Ok(out)
    }
    fn sort(&self, r: &mut Relation, order: &[ast::OrderByExpr]) -> Result<()> {
        let keys = r
            .rows
            .iter()
            .map(|row| {
                order
                    .iter()
                    .map(|o| self.eval(&o.expr, &r.columns, row, None))
                    .collect::<Result<Vec<_>>>()
            })
            .collect::<Result<Vec<_>>>()?;
        sort_by_keys(&mut r.rows, &keys, order)
    }
    fn window(
        &self,
        f: &ast::Function,
        input: &Relation,
        row: &[Scalar],
        source_position: usize,
    ) -> Result<Scalar> {
        let Some(ast::WindowType::WindowSpec(w)) = &f.over else {
            return Err(Error::unsupported("named window"));
        };
        if w.window_name.is_some() || w.window_frame.is_some() {
            return Err(Error::unsupported("explicit window frame"));
        }
        let partition = w
            .partition_by
            .iter()
            .map(|e| self.eval(e, &input.columns, row, None))
            .collect::<Result<Vec<_>>>()?;
        let mut indexed = vec![];
        let mut keys = vec![];
        for (i, r) in input.rows.iter().enumerate() {
            let key = w
                .partition_by
                .iter()
                .map(|e| self.eval(e, &input.columns, r, None))
                .collect::<Result<Vec<_>>>()?;
            if row_equal(&partition, &key)? {
                indexed.push((i, r.clone()));
                keys.push(
                    w.order_by
                        .iter()
                        .map(|o| self.eval(&o.expr, &input.columns, r, None))
                        .collect::<Result<Vec<_>>>()?,
                )
            }
        }
        let mut positions = (0..indexed.len()).collect::<Vec<_>>();
        let error = RefCell::new(None);
        positions.sort_by(
            |a, b| match compare_keys(&keys[*a], &keys[*b], &w.order_by) {
                Ok(o) => o,
                Err(e) => {
                    error.replace(Some(e));
                    Ordering::Equal
                }
            },
        );
        if let Some(e) = error.into_inner() {
            return Err(e);
        }
        let current = positions
            .iter()
            .position(|i| indexed[*i].0 == source_position)
            .ok_or_else(|| Error::unsupported("window after grouping"))?;
        let mut rank = 1;
        let mut dense = 1;
        for i in 1..=current {
            if !row_equal(&keys[positions[i - 1]], &keys[positions[i]])? {
                rank = i + 1;
                dense += 1
            }
        }
        match name(&f.name).as_str() {
            "row_number" => Ok(Scalar::Int((current + 1) as i64)),
            "rank" => Ok(Scalar::Int(rank as i64)),
            "dense_rank" => Ok(Scalar::Int(dense)),
            _ => {
                let current_key = keys[positions[current]].clone();
                let mut frame = vec![];
                for i in positions {
                    if w.order_by.is_empty()
                        || compare_keys(&keys[i], &current_key, &w.order_by)? != Ordering::Greater
                    {
                        frame.push(indexed[i].1.clone())
                    }
                }
                let mut f = f.clone();
                f.over = None;
                self.function(&f, &input.columns, row, Some(&frame))
            }
        }
    }
}
fn alias(r: &mut Relation, a: &ast::TableAlias) -> Result<()> {
    if !a.columns.is_empty() && a.columns.len() != r.columns.len() {
        return Err(Error::new("42601", "alias column count"));
    }
    for (i, c) in r.columns.iter_mut().enumerate() {
        c.qualifier = identifier(&a.name);
        if let Some(n) = a.columns.get(i) {
            c.name = identifier(&n.name)
        }
    }
    Ok(())
}
pub fn scalar_type(v: &Scalar) -> Type {
    match v {
        Scalar::Null | Scalar::Text(_) => Type::Text,
        Scalar::Bool(_) => Type::Bool,
        Scalar::Int(_) => Type::Int8,
        Scalar::Float(_) => Type::Float8,
        Scalar::Bytes(_) => Type::Bytea,
        Scalar::Uuid(_) => Type::Uuid,
        Scalar::Numeric(_) => Type::Numeric(None),
        Scalar::Date(_) => Type::Date,
        Scalar::Time(_) => Type::Time,
        Scalar::Timestamp(_) => Type::Timestamp,
        Scalar::Timestamptz(_) => Type::Timestamptz,
        Scalar::Json(_) => Type::Jsonb,
    }
}
fn column_type(n: &[String], c: &[Column]) -> Result<Type> {
    let matches = c
        .iter()
        .filter(|c| {
            Some(&c.name) == n.last() && (n.len() == 1 || Some(&c.qualifier) == n.get(n.len() - 2))
        })
        .collect::<Vec<_>>();
    if matches.len() > 1 {
        return Err(Error::new("42702", "ambiguous column"));
    }
    matches
        .first()
        .map(|c| c.ty.clone())
        .ok_or_else(|| Error::new("42703", "column does not exist"))
}
pub fn row_equal(a: &[Scalar], b: &[Scalar]) -> Result<bool> {
    if a.len() != b.len() {
        return Ok(false);
    }
    for (a, b) in a.iter().zip(b) {
        if a == &Scalar::Null || b == &Scalar::Null {
            if a != b {
                return Ok(false);
            }
        } else if a.compare(b)? != Some(Ordering::Equal) {
            return Ok(false);
        }
    }
    Ok(true)
}
fn distinct(r: &mut Relation) -> Result<()> {
    let mut rows: Vec<Vec<Scalar>> = vec![];
    for row in std::mem::take(&mut r.rows) {
        let mut exists = false;
        for old in &rows {
            if row_equal(old, &row)? {
                exists = true;
                break;
            }
        }
        if !exists {
            rows.push(row)
        }
    }
    r.rows = rows;
    Ok(())
}
fn compare_keys(a: &[Scalar], b: &[Scalar], order: &[ast::OrderByExpr]) -> Result<Ordering> {
    for ((a, b), o) in a.iter().zip(b).zip(order) {
        let null_first = o.nulls_first.unwrap_or(o.asc == Some(false));
        let mut cmp = if a == &Scalar::Null {
            if b == &Scalar::Null {
                Ordering::Equal
            } else if null_first {
                Ordering::Less
            } else {
                Ordering::Greater
            }
        } else if b == &Scalar::Null {
            if null_first {
                Ordering::Greater
            } else {
                Ordering::Less
            }
        } else {
            a.compare(b)?.unwrap()
        };
        if o.asc == Some(false) && a != &Scalar::Null && b != &Scalar::Null {
            cmp = cmp.reverse()
        }
        if cmp != Ordering::Equal {
            return Ok(cmp);
        }
    }
    Ok(Ordering::Equal)
}
fn sort_by_keys(
    rows: &mut Vec<Vec<Scalar>>,
    keys: &[Vec<Scalar>],
    order: &[ast::OrderByExpr],
) -> Result<()> {
    let mut indexes = (0..rows.len()).collect::<Vec<_>>();
    let error = RefCell::new(None);
    indexes.sort_by(|a, b| match compare_keys(&keys[*a], &keys[*b], order) {
        Ok(c) => c,
        Err(e) => {
            error.replace(Some(e));
            Ordering::Equal
        }
    });
    if let Some(e) = error.into_inner() {
        return Err(e);
    }
    let old = std::mem::take(rows);
    *rows = indexes.into_iter().map(|i| old[i].clone()).collect();
    Ok(())
}
fn window(e: &Expr) -> bool {
    matches!(e,Expr::Function(f)if f.over.is_some())
}
fn aggregate(e: &Expr) -> bool {
    match e {
        Expr::Function(f) => {
            f.over.is_none()
                && ["count", "sum", "avg", "min", "max"].contains(&name(&f.name).as_str())
        }
        Expr::BinaryOp { left, right, .. } => aggregate(left) || aggregate(right),
        Expr::Nested(e) | Expr::Cast { expr: e, .. } | Expr::UnaryOp { expr: e, .. } => {
            aggregate(e)
        }
        _ => false,
    }
}
fn check_group(e: &Expr, group: &[Expr], inside: bool) -> Result<()> {
    if inside || group.iter().any(|g| g == e) {
        return Ok(());
    }
    match e {
        Expr::Identifier(_) | Expr::CompoundIdentifier(_) => {
            Err(Error::new("42803", "column must appear in GROUP BY"))
        }
        Expr::Function(f) if aggregate(e) => {
            let _ = f;
            Ok(())
        }
        Expr::BinaryOp { left, right, .. } => {
            check_group(left, group, false)?;
            check_group(right, group, false)
        }
        Expr::Nested(e) | Expr::Cast { expr: e, .. } | Expr::UnaryOp { expr: e, .. } => {
            check_group(e, group, false)
        }
        _ => Ok(()),
    }
}

fn point_equalities(e: &Expr) -> Option<Vec<(String, &Expr)>> {
    match e {
        Expr::BinaryOp {
            left,
            op: ast::BinaryOperator::And,
            right,
        } => {
            let mut a = point_equalities(left)?;
            a.extend(point_equalities(right)?);
            Some(a)
        }
        Expr::BinaryOp {
            left,
            op: ast::BinaryOperator::Eq,
            right,
        } => {
            let (column, value) = match (left.as_ref(), right.as_ref()) {
                (Expr::Identifier(i), v) if constant(v) => (identifier(i), v),
                (Expr::CompoundIdentifier(i), v) if constant(v) => (identifier(i.last()?), v),
                (v, Expr::Identifier(i)) if constant(v) => (identifier(i), v),
                _ => return None,
            };
            Some(vec![(column, value)])
        }
        _ => None,
    }
}
fn constant(e: &Expr) -> bool {
    match e {
        Expr::Value(_) => true,
        Expr::Cast { expr, .. } | Expr::Nested(expr) | Expr::UnaryOp { expr, .. } => constant(expr),
        _ => false,
    }
}
