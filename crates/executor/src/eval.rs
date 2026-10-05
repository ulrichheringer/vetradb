use crate::*;
use ast::{
    BinaryOperator as B, Expr, FunctionArg, FunctionArgExpr, FunctionArguments, UnaryOperator as U,
    Value as Literal,
};
use std::cmp::Ordering;
use vetra_types::sql::{BigDecimal, serde_json};
impl Runtime<'_> {
    pub fn eval(
        &self,
        e: &Expr,
        columns: &[Column],
        row: &[Scalar],
        group: Option<&[Vec<Scalar>]>,
    ) -> Result<Scalar> {
        self.tick()?;
        let depth = self.depth.get();
        if depth >= 64 {
            return Err(Error::new("54000", "expression depth"));
        }
        self.depth.set(depth + 1);
        let result = self.eval_inner(e, columns, row, group);
        self.depth.set(depth);
        result
    }
    fn eval_inner(
        &self,
        e: &Expr,
        c: &[Column],
        r: &[Scalar],
        g: Option<&[Vec<Scalar>]>,
    ) -> Result<Scalar> {
        Ok(match e {
            Expr::Value(v) => match v {
                Literal::Null => Scalar::Null,
                Literal::Boolean(b) => Scalar::Bool(*b),
                Literal::Number(s, _) => s
                    .parse::<i64>()
                    .map(Scalar::Int)
                    .or_else(|_| Scalar::decimal(s))?,
                Literal::SingleQuotedString(s)
                | Literal::EscapedStringLiteral(s)
                | Literal::DoubleQuotedString(s) => Scalar::Text(s.clone()),
                Literal::HexStringLiteral(s) => Scalar::Bytes(vetra_types::sql::unhex(s)?),
                Literal::Placeholder(p) => {
                    let n = p
                        .strip_prefix('$')
                        .and_then(|s| s.parse::<usize>().ok())
                        .filter(|n| *n > 0)
                        .ok_or_else(|| Error::new("42P02", "positional parameter required"))?;
                    self.parameters
                        .get(n - 1)
                        .cloned()
                        .ok_or_else(|| Error::new("42P02", "parameter count"))?
                }
                _ => return Err(Error::unsupported("literal")),
            },
            Expr::Identifier(i) => self.resolve(&[identifier(i)], c, r)?,
            Expr::CompoundIdentifier(ids) => {
                self.resolve(&ids.iter().map(identifier).collect::<Vec<_>>(), c, r)?
            }
            Expr::Nested(e) => self.eval(e, c, r, g)?,
            Expr::Cast {
                expr,
                data_type: ty,
                format,
                ..
            } => {
                if format.is_some() {
                    return Err(Error::unsupported("cast FORMAT"));
                }
                self.eval(expr, c, r, g)?.cast(&data_type(ty)?)?
            }
            Expr::TypedString {
                data_type: ty,
                value,
            } => Scalar::Text(value.clone()).cast(&data_type(ty)?)?,
            Expr::IsNull(e) | Expr::IsUnknown(e) => {
                Scalar::Bool(self.eval(e, c, r, g)? == Scalar::Null)
            }
            Expr::IsNotNull(e) | Expr::IsNotUnknown(e) => {
                Scalar::Bool(self.eval(e, c, r, g)? != Scalar::Null)
            }
            Expr::IsTrue(e) => Scalar::Bool(self.eval(e, c, r, g)?.truth()? == Some(true)),
            Expr::IsFalse(e) => Scalar::Bool(self.eval(e, c, r, g)?.truth()? == Some(false)),
            Expr::IsNotTrue(e) => Scalar::Bool(self.eval(e, c, r, g)?.truth()? != Some(true)),
            Expr::IsNotFalse(e) => Scalar::Bool(self.eval(e, c, r, g)?.truth()? != Some(false)),
            Expr::UnaryOp { op, expr } => {
                let v = self.eval(expr, c, r, g)?;
                if v == Scalar::Null {
                    Scalar::Null
                } else {
                    match op {
                        U::Not => truth(v.truth()?.map(|v| !v)),
                        U::Plus => v,
                        U::Minus => match v {
                            Scalar::Int(n) => Scalar::Int(
                                n.checked_neg()
                                    .ok_or_else(|| Error::new("22003", "integer overflow"))?,
                            ),
                            Scalar::Float(n) => Scalar::Float((-f64::from_bits(n)).to_bits()),
                            _ => Scalar::decimal(&(-v.numeric()?).to_string())?,
                        },
                        _ => return Err(Error::unsupported("unary operator")),
                    }
                }
            }
            Expr::BinaryOp { left, op, right } => {
                let a = self.eval(left, c, r, g)?;
                let b = self.eval(right, c, r, g)?;
                binary(&a, op, &b)?
            }
            Expr::IsDistinctFrom(a, b) | Expr::IsNotDistinctFrom(a, b) => {
                let a = self.eval(a, c, r, g)?;
                let b = self.eval(b, c, r, g)?;
                let equal = if a == Scalar::Null || b == Scalar::Null {
                    a == b
                } else {
                    a.compare(&b)? == Some(Ordering::Equal)
                };
                Scalar::Bool(if matches!(e, Expr::IsDistinctFrom(..)) {
                    !equal
                } else {
                    equal
                })
            }
            Expr::Between {
                expr,
                negated,
                low,
                high,
            } => {
                let a = self.eval(expr, c, r, g)?;
                let lo = self.eval(low, c, r, g)?;
                let hi = self.eval(high, c, r, g)?;
                let v = binary(
                    &binary(&a, &B::GtEq, &lo)?,
                    &B::And,
                    &binary(&a, &B::LtEq, &hi)?,
                )?;
                if *negated {
                    truth(v.truth()?.map(|b| !b))
                } else {
                    v
                }
            }
            Expr::InList {
                expr,
                list,
                negated,
            } => {
                let v = self.eval(expr, c, r, g)?;
                let values = list
                    .iter()
                    .map(|e| self.eval(e, c, r, g))
                    .collect::<Result<Vec<_>>>()?;
                membership(&v, &values, *negated)?
            }
            Expr::Case {
                operand,
                conditions,
                results,
                else_result,
            } => {
                let operand = operand
                    .as_ref()
                    .map(|e| self.eval(e, c, r, g))
                    .transpose()?;
                let mut value = Scalar::Null;
                let mut found = false;
                for (condition, result) in conditions.iter().zip(results) {
                    let condition = self.eval(condition, c, r, g)?;
                    let yes = if let Some(o) = &operand {
                        binary(o, &B::Eq, &condition)?.truth()? == Some(true)
                    } else {
                        condition.truth()? == Some(true)
                    };
                    if yes {
                        value = self.eval(result, c, r, g)?;
                        found = true;
                        break;
                    }
                }
                if !found {
                    if let Some(e) = else_result {
                        value = self.eval(e, c, r, g)?
                    }
                }
                value
            }
            Expr::Function(f) => self.function(f, c, r, g)?,
            Expr::Subquery(q) => {
                let rel = self.correlated(q, c, r)?;
                if rel.columns.len() != 1 || rel.rows.len() > 1 {
                    return Err(Error::new("21000", "scalar subquery cardinality"));
                }
                rel.rows.first().map_or(Scalar::Null, |r| r[0].clone())
            }
            Expr::Exists { subquery, negated } => {
                Scalar::Bool(self.correlated(subquery, c, r)?.rows.is_empty() == *negated)
            }
            Expr::InSubquery {
                expr,
                subquery,
                negated,
            } => {
                let v = self.eval(expr, c, r, g)?;
                let rel = self.correlated(subquery, c, r)?;
                if rel.columns.len() != 1 {
                    return Err(Error::new("42601", "IN subquery column count"));
                }
                membership(
                    &v,
                    &rel.rows
                        .into_iter()
                        .map(|r| r[0].clone())
                        .collect::<Vec<_>>(),
                    *negated,
                )?
            }
            Expr::Collate { expr, collation } => {
                if !["C", "pg_catalog.C"].contains(&name(collation).as_str()) {
                    return Err(Error::unsupported("collation"));
                }
                self.eval(expr, c, r, g)?
            }
            _ => return Err(Error::unsupported(format!("expression {e}"))),
        })
    }
    pub fn resolve(&self, n: &[String], c: &[Column], r: &[Scalar]) -> Result<Scalar> {
        let col = n.last().ok_or_else(|| Error::new("42703", "column"))?;
        let qualifier = if n.len() > 1 {
            n.get(n.len() - 2)
        } else {
            None
        };
        let matching = |c: &Column| &c.name == col && qualifier.is_none_or(|q| q == &c.qualifier);
        let indexes = c
            .iter()
            .enumerate()
            .filter(|(_, c)| matching(c))
            .map(|(i, _)| i)
            .collect::<Vec<_>>();
        if indexes.len() > 1 {
            return Err(Error::new("42702", "ambiguous column"));
        }
        if let Some(i) = indexes.first() {
            return Ok(r[*i].clone());
        }
        for (c, r) in self.outer.borrow().iter().rev() {
            let values = c
                .iter()
                .zip(r)
                .filter(|(c, _)| matching(c))
                .collect::<Vec<_>>();
            if values.len() > 1 {
                return Err(Error::new("42702", "ambiguous outer column"));
            }
            if let Some((_, v)) = values.first() {
                return Ok((*v).clone());
            }
        }
        Err(Error::new(
            "42703",
            format!("column {} does not exist", n.join(".")),
        ))
    }
    pub fn correlated(&self, q: &ast::Query, c: &[Column], r: &[Scalar]) -> Result<Relation> {
        self.outer.borrow_mut().push((c.to_vec(), r.to_vec()));
        let result = self.query(q);
        self.outer.borrow_mut().pop();
        result
    }
    pub fn function(
        &self,
        f: &ast::Function,
        c: &[Column],
        r: &[Scalar],
        g: Option<&[Vec<Scalar>]>,
    ) -> Result<Scalar> {
        let fname = name(&f.name);
        if f.over.is_some() {
            return Err(Error::unsupported("window in scalar context"));
        }
        if f.null_treatment.is_some()
            || !f.within_group.is_empty()
            || !matches!(f.parameters, FunctionArguments::None)
        {
            return Err(Error::unsupported("function options"));
        }
        let args = function_args(f)?;
        if ["count", "sum", "avg", "min", "max"].contains(&fname.as_str()) {
            let group =
                g.ok_or_else(|| Error::new("42803", "aggregate outside grouped context"))?;
            if args.len() != 1 {
                return Err(Error::new("42883", "aggregate arity"));
            }
            let mut values = vec![];
            for row in group {
                if f.filter.is_some()
                    && f.filter
                        .as_ref()
                        .map(|p| self.eval(p, c, row, None).and_then(|v| v.truth()))
                        .transpose()?
                        .flatten()
                        != Some(true)
                {
                    continue;
                }
                let v = match &args[0] {
                    FunctionArgExpr::Wildcard => Scalar::Int(1),
                    FunctionArgExpr::Expr(e) => self.eval(e, c, row, None)?,
                    _ => return Err(Error::unsupported("aggregate argument")),
                };
                if v != Scalar::Null {
                    values.push(v)
                }
            }
            if let FunctionArguments::List(l) = &f.args {
                if l.duplicate_treatment == Some(ast::DuplicateTreatment::Distinct) {
                    let mut distinct = vec![];
                    for v in values {
                        if !distinct
                            .iter()
                            .any(|d: &Scalar| d.compare(&v).ok().flatten() == Some(Ordering::Equal))
                        {
                            distinct.push(v)
                        }
                    }
                    values = distinct
                }
            }
            if fname == "count" {
                return Ok(Scalar::Int(values.len() as i64));
            }
            if values.is_empty() {
                return Ok(Scalar::Null);
            }
            if fname == "min" || fname == "max" {
                let mut best = values[0].clone();
                for v in &values[1..] {
                    let ord = v.compare(&best)?.unwrap();
                    if ord
                        == if fname == "min" {
                            Ordering::Less
                        } else {
                            Ordering::Greater
                        }
                    {
                        best = v.clone()
                    }
                }
                return Ok(best);
            }
            let mut n = BigDecimal::from(0);
            for v in &values {
                n += v.numeric()?
            }
            if fname == "avg" {
                return vetra_types::sql::divide(&n, &BigDecimal::from(values.len() as i64));
            }
            if fname == "sum"
                && matches!(&args[0],FunctionArgExpr::Expr(e) if matches!(self.infer(e,c)?,Type::Int2|Type::Int4))
            {
                return n
                    .to_string()
                    .parse::<i64>()
                    .map(Scalar::Int)
                    .map_err(|_| Error::new("22003", "sum overflow"));
            }
            return Scalar::decimal(&n.to_string());
        }
        let mut values = vec![];
        for arg in args {
            let FunctionArgExpr::Expr(e) = arg else {
                return Err(Error::unsupported("scalar wildcard"));
            };
            values.push(self.eval(e, c, r, g)?)
        }
        match fname.as_str() {
            "coalesce" => Ok(values
                .into_iter()
                .find(|v| v != &Scalar::Null)
                .unwrap_or(Scalar::Null)),
            "nullif" if values.len() == 2 => {
                Ok(if values[0].compare(&values[1])? == Some(Ordering::Equal) {
                    Scalar::Null
                } else {
                    values[0].clone()
                })
            }
            "now" | "current_timestamp" | "transaction_timestamp" if values.is_empty() => {
                Ok(Scalar::Timestamptz(self.timestamp))
            }
            "lower" | "upper" | "length" | "octet_length" | "abs" if values.len() == 1 => {
                if values[0] == Scalar::Null {
                    return Ok(Scalar::Null);
                }
                match fname.as_str() {
                    "lower" => Ok(Scalar::Text(values[0].text().to_lowercase())),
                    "upper" => Ok(Scalar::Text(values[0].text().to_uppercase())),
                    "length" => Ok(Scalar::Int(values[0].text().chars().count() as i64)),
                    "octet_length" => Ok(Scalar::Int(values[0].text().len() as i64)),
                    _ => Scalar::decimal(&values[0].numeric()?.abs().to_string()),
                }
            }
            "concat" => Ok(Scalar::Text(
                values
                    .iter()
                    .filter(|v| **v != Scalar::Null)
                    .map(Scalar::text)
                    .collect::<String>(),
            )),
            "jsonb_typeof" | "json_typeof" if values.len() == 1 => {
                if values[0] == Scalar::Null {
                    return Ok(Scalar::Null);
                }
                let json: serde_json::Value = serde_json::from_str(&values[0].text())
                    .map_err(|_| Error::new("22P02", "JSON"))?;
                Ok(Scalar::Text(
                    match json {
                        serde_json::Value::Null => "null",
                        serde_json::Value::Bool(_) => "boolean",
                        serde_json::Value::Number(_) => "number",
                        serde_json::Value::String(_) => "string",
                        serde_json::Value::Array(_) => "array",
                        serde_json::Value::Object(_) => "object",
                    }
                    .into(),
                ))
            }
            _ => Err(Error::unsupported(format!("function/signature {fname}"))),
        }
    }
}
pub fn function_args(f: &ast::Function) -> Result<Vec<&FunctionArgExpr>> {
    match &f.args {
        FunctionArguments::None => Ok(vec![]),
        FunctionArguments::List(l) if l.clauses.is_empty() => l
            .args
            .iter()
            .map(|a| match a {
                FunctionArg::Unnamed(e) => Ok(e),
                _ => Err(Error::unsupported("named function argument")),
            })
            .collect(),
        _ => Err(Error::unsupported("function argument clauses")),
    }
}
pub fn truth(v: Option<bool>) -> Scalar {
    v.map_or(Scalar::Null, Scalar::Bool)
}
pub fn membership(v: &Scalar, values: &[Scalar], negated: bool) -> Result<Scalar> {
    let mut unknown = false;
    for other in values {
        match v.compare(other)? {
            Some(Ordering::Equal) => return Ok(Scalar::Bool(!negated)),
            None => unknown = true,
            _ => {}
        }
    }
    Ok(if unknown {
        Scalar::Null
    } else {
        Scalar::Bool(negated)
    })
}
pub fn binary(a: &Scalar, op: &B, b: &Scalar) -> Result<Scalar> {
    if matches!(op, B::And | B::Or) {
        let (a, b) = (a.truth()?, b.truth()?);
        return Ok(truth(match op {
            B::And => {
                if a == Some(false) || b == Some(false) {
                    Some(false)
                } else if a == Some(true) && b == Some(true) {
                    Some(true)
                } else {
                    None
                }
            }
            _ => {
                if a == Some(true) || b == Some(true) {
                    Some(true)
                } else if a == Some(false) && b == Some(false) {
                    Some(false)
                } else {
                    None
                }
            }
        }));
    }
    if a == &Scalar::Null || b == &Scalar::Null {
        return Ok(Scalar::Null);
    }
    if matches!(op, B::Eq | B::NotEq | B::Gt | B::GtEq | B::Lt | B::LtEq) {
        let order = a.compare(b)?.unwrap();
        return Ok(Scalar::Bool(match op {
            B::Eq => order == Ordering::Equal,
            B::NotEq => order != Ordering::Equal,
            B::Gt => order == Ordering::Greater,
            B::GtEq => order != Ordering::Less,
            B::Lt => order == Ordering::Less,
            _ => order != Ordering::Greater,
        }));
    }
    if matches!(op, B::StringConcat) {
        return Ok(Scalar::Text(a.text() + &b.text()));
    }
    if matches!(op, B::Arrow | B::LongArrow) {
        let v: serde_json::Value =
            serde_json::from_str(&a.text()).map_err(|_| Error::new("22P02", "JSON operand"))?;
        let value = match b {
            Scalar::Int(n) => {
                if let Some(vs) = v.as_array() {
                    let i = if *n < 0 { vs.len() as i64 + n } else { *n };
                    usize::try_from(i).ok().and_then(|i| vs.get(i))
                } else {
                    None
                }
            }
            Scalar::Text(k) => v.get(k),
            _ => return Err(Error::new("42804", "JSON path operand")),
        };
        return Ok(match value {
            None => Scalar::Null,
            Some(serde_json::Value::Null) if *op == B::LongArrow => Scalar::Null,
            Some(v) if *op == B::LongArrow => {
                Scalar::Text(v.as_str().map_or_else(|| v.to_string(), str::to_owned))
            }
            Some(v) => Scalar::Json(v.to_string()),
        });
    }
    if matches!(op, B::AtArrow | B::ArrowAt) {
        let a: serde_json::Value =
            serde_json::from_str(&a.text()).map_err(|_| Error::new("22P02", "JSON"))?;
        let b: serde_json::Value =
            serde_json::from_str(&b.text()).map_err(|_| Error::new("22P02", "JSON"))?;
        return Ok(Scalar::Bool(if *op == B::AtArrow {
            contains(&a, &b)
        } else {
            contains(&b, &a)
        }));
    }
    if matches!(a, Scalar::Int(_)) && matches!(b, Scalar::Int(_)) {
        let (Scalar::Int(a), Scalar::Int(b)) = (a, b) else {
            unreachable!()
        };
        let n = match op {
            B::Plus => a.checked_add(*b),
            B::Minus => a.checked_sub(*b),
            B::Multiply => a.checked_mul(*b),
            B::Divide => {
                if *b == 0 {
                    return Err(Error::new("22012", "division by zero"));
                }
                a.checked_div(*b)
            }
            B::Modulo => {
                if *b == 0 {
                    return Err(Error::new("22012", "division by zero"));
                }
                a.checked_rem(*b)
            }
            _ => return Err(Error::unsupported("binary operator")),
        };
        return n
            .map(Scalar::Int)
            .ok_or_else(|| Error::new("22003", "integer overflow"));
    }
    let a = a.numeric()?;
    let b = b.numeric()?;
    let n = match op {
        B::Plus => a + b,
        B::Minus => a - b,
        B::Multiply => a * b,
        B::Divide => {
            if b == BigDecimal::from(0) {
                return Err(Error::new("22012", "division by zero"));
            }
            return vetra_types::sql::divide(&a, &b);
        }
        B::Modulo => {
            if b == BigDecimal::from(0) {
                return Err(Error::new("22012", "division by zero"));
            }
            a % b
        }
        _ => return Err(Error::unsupported("binary operator")),
    };
    Scalar::decimal(&n.to_string())
}
fn contains(a: &serde_json::Value, b: &serde_json::Value) -> bool {
    match (a, b) {
        (serde_json::Value::Object(a), serde_json::Value::Object(b)) => b
            .iter()
            .all(|(k, v)| a.get(k).is_some_and(|a| contains(a, v))),
        (serde_json::Value::Array(a), serde_json::Value::Array(b)) => {
            b.iter().all(|v| a.iter().any(|a| contains(a, v)))
        }
        _ => a == b,
    }
}
