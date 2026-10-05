//! Pinned PostgreSQL grammar with bounded parsing; execution support is explicit.
pub use sqlparser::ast;
use sqlparser::{dialect::PostgreSqlDialect, parser::Parser};
pub use vetra_types::sql::{Error, Result, Scalar, Type};
pub const MAX_SQL: usize = 1024 * 1024;
pub fn parse(text: &str) -> Result<Vec<ast::Statement>> {
    if text.len() > MAX_SQL || text.contains('\0') {
        return Err(Error::new("54000", "SQL input budget or NUL"));
    }
    let tokens = sqlparser::tokenizer::Tokenizer::new(&PostgreSqlDialect {}, text)
        .tokenize()
        .map_err(|e| Error::new("42601", e.to_string()))?;
    if tokens.len() > 4096 {
        return Err(Error::new("54000", "token budget"));
    }
    let mut depth = 0usize;
    for token in &tokens {
        match token {
            sqlparser::tokenizer::Token::LParen | sqlparser::tokenizer::Token::LBracket => {
                depth += 1;
                if depth > 16 {
                    return Err(Error::new("54000", "grammar nesting budget"));
                }
            }
            sqlparser::tokenizer::Token::RParen | sqlparser::tokenizer::Token::RBracket => {
                depth = depth.saturating_sub(1)
            }
            _ => {}
        }
    }
    let mut parser = Parser::new(&PostgreSqlDialect {})
        .with_recursion_limit(16)
        .try_with_sql(text)
        .map_err(|e| Error::new("42601", e.to_string()))?;
    let statements = parser
        .parse_statements()
        .map_err(|e| Error::new("42601", e.to_string()))?;
    if statements.len() > 128 {
        return Err(Error::new("54000", "statement count budget"));
    }
    bounded_ast(&statements)?;
    Ok(statements)
}
pub fn expression(text: &str) -> Result<ast::Expr> {
    let statements = parse(&format!("SELECT {text}"))?;
    let Some(ast::Statement::Query(q)) = statements.first() else {
        return Err(Error::new("42601", "expression required"));
    };
    let ast::SetExpr::Select(s) = q.body.as_ref() else {
        return Err(Error::new("42601", "expression required"));
    };
    if statements.len() != 1 || s.projection.len() != 1 || !s.from.is_empty() {
        return Err(Error::new("42601", "scalar expression required"));
    }
    match &s.projection[0] {
        ast::SelectItem::UnnamedExpr(e) => Ok(e.clone()),
        _ => Err(Error::new("42601", "scalar expression required")),
    }
}
pub fn identifier(i: &ast::Ident) -> String {
    if i.quote_style.is_some() {
        i.value.clone()
    } else {
        i.value.to_lowercase()
    }
}
pub fn name(n: &ast::ObjectName) -> String {
    n.0.iter().map(identifier).collect::<Vec<_>>().join(".")
}
pub fn data_type(ty: &ast::DataType) -> Result<Type> {
    let s = ty.to_string().to_lowercase();
    let base = s.split('(').next().unwrap_or(&s).trim();
    let modifiers = if let Some((_, end)) = s.split_once('(') {
        let end = end.split(')').next().unwrap_or("");
        end.split(',')
            .map(|n| {
                n.trim()
                    .parse::<u16>()
                    .map_err(|_| Error::new("22023", "type modifier"))
            })
            .collect::<Result<Vec<_>>>()?
    } else {
        vec![]
    };
    if matches!(base, "numeric" | "decimal" | "dec")
        && !modifiers.is_empty()
        && (modifiers[0] == 0
            || modifiers[0] > 1000
            || modifiers.get(1).is_some_and(|s| *s > modifiers[0]))
    {
        return Err(Error::new("22023", "numeric modifier bounds"));
    }
    if matches!(base, "varchar" | "character varying") && modifiers.first() == Some(&0) {
        return Err(Error::new("22023", "varchar modifier bounds"));
    }
    if matches!(base, "time" | "timestamp" | "timestamptz")
        && modifiers.first().is_some_and(|p| *p != 6)
    {
        return Err(Error::unsupported(
            "only default microsecond timestamp/time precision",
        ));
    }
    Ok(match base {
        "boolean" | "bool" => Type::Bool,
        "smallint" | "int2" => Type::Int2,
        "integer" | "int" | "int4" => Type::Int4,
        "bigint" | "int8" => Type::Int8,
        "real" | "float4" => Type::Float4,
        "double precision" | "float8" | "float" => Type::Float8,
        "text" => Type::Text,
        "varchar" | "character varying" => Type::Varchar(modifiers.first().map(|n| u32::from(*n))),
        "bytea" => Type::Bytea,
        "uuid" => Type::Uuid,
        "numeric" | "decimal" | "dec" => Type::Numeric(if modifiers.is_empty() {
            None
        } else {
            Some((modifiers[0], *modifiers.get(1).unwrap_or(&0)))
        }),
        "date" => Type::Date,
        "time" | "time without time zone" => Type::Time,
        "timestamp" | "timestamp without time zone" => Type::Timestamp,
        "timestamptz" | "timestamp with time zone" => Type::Timestamptz,
        "json" => Type::Json,
        "jsonb" => Type::Jsonb,
        _ => return Err(Error::unsupported(format!("type {s}"))),
    })
}
/// Persisted CHECK/generated expressions are immutable and parameter-free.
pub fn immutable(expr: &ast::Expr) -> Result<()> {
    stored_expression(expr, false)
}
pub fn default_expression(expr: &ast::Expr) -> Result<()> {
    stored_expression(expr, true)
}
fn stored_expression(expr: &ast::Expr, allow_time: bool) -> Result<()> {
    use ast::{Expr, Visit, Visitor};
    use std::ops::ControlFlow;
    struct Check {
        allow_time: bool,
    }
    impl Visitor for Check {
        type Break = Error;
        fn pre_visit_expr(&mut self, e: &Expr) -> ControlFlow<Error> {
            if matches!(
                e,
                Expr::Subquery(_)
                    | Expr::Exists { .. }
                    | Expr::InSubquery { .. }
                    | Expr::Value(ast::Value::Placeholder(_))
            ) {
                return ControlFlow::Break(Error::unsupported(
                    "stored expression subquery/parameter",
                ));
            }
            if let Expr::Function(f) = e {
                if f.over.is_some()
                    || !([
                        "abs",
                        "lower",
                        "upper",
                        "length",
                        "octet_length",
                        "coalesce",
                        "nullif",
                        "concat",
                        "jsonb_typeof",
                        "json_typeof",
                    ]
                    .contains(&name(&f.name).as_str())
                        || (self.allow_time
                            && ["now", "current_timestamp", "transaction_timestamp"]
                                .contains(&name(&f.name).as_str())))
                {
                    return ControlFlow::Break(Error::unsupported(
                        "stored expression must be immutable",
                    ));
                }
            }
            ControlFlow::Continue(())
        }
    }
    match expr.visit(&mut Check { allow_time }) {
        ControlFlow::Continue(()) => Ok(()),
        ControlFlow::Break(e) => Err(e),
    }
}
pub fn rename_column(text: &str, old: &str, new: &str) -> Result<String> {
    use ast::{Expr, VisitMut, VisitorMut};
    use std::ops::ControlFlow;
    struct Rename<'a> {
        old: &'a str,
        new: &'a str,
    }
    impl VisitorMut for Rename<'_> {
        type Break = ();
        fn pre_visit_expr(&mut self, e: &mut Expr) -> ControlFlow<()> {
            let ident = match e {
                Expr::Identifier(i) => Some(i),
                Expr::CompoundIdentifier(i) => i.last_mut(),
                _ => None,
            };
            if let Some(i) = ident {
                if identifier(i) == self.old {
                    *i = ast::Ident::with_quote('"', self.new)
                }
            }
            ControlFlow::Continue(())
        }
    }
    let mut expr = expression(text)?;
    let _ = expr.visit(&mut Rename { old, new });
    Ok(expr.to_string())
}
fn bounded_ast(statements: &[ast::Statement]) -> Result<()> {
    use ast::{Visit, Visitor};
    use std::ops::ControlFlow;
    struct Bound {
        depth: usize,
        nodes: usize,
    }
    impl Visitor for Bound {
        type Break = Error;
        fn pre_visit_expr(&mut self, _: &ast::Expr) -> ControlFlow<Error> {
            self.depth += 1;
            self.nodes += 1;
            if self.depth > 32 || self.nodes > 4096 {
                ControlFlow::Break(Error::new("54000", "AST budget"))
            } else {
                ControlFlow::Continue(())
            }
        }
        fn post_visit_expr(&mut self, _: &ast::Expr) -> ControlFlow<Error> {
            self.depth -= 1;
            ControlFlow::Continue(())
        }
    }
    let mut bound = Bound { depth: 0, nodes: 0 };
    for statement in statements {
        if let ControlFlow::Break(e) = statement.visit(&mut bound) {
            return Err(e);
        }
    }
    Ok(())
}
pub fn rename_relation(text: &str, old: &str, new: &str) -> Result<String> {
    use ast::{TableFactor, VisitMut, VisitorMut};
    use std::ops::ControlFlow;
    struct Rename<'a> {
        old: &'a str,
        new: ast::ObjectName,
    }
    impl VisitorMut for Rename<'_> {
        type Break = ();
        fn pre_visit_table_factor(&mut self, f: &mut TableFactor) -> ControlFlow<()> {
            if let TableFactor::Table { name: n, alias, .. } = f {
                let current = name(n);
                let current = if current.contains('.') {
                    current
                } else {
                    format!("public.{current}")
                };
                if current == self.old {
                    if alias.is_none() {
                        *alias = Some(ast::TableAlias {
                            name: n.0.last().unwrap().clone(),
                            columns: vec![],
                        })
                    }
                    *n = self.new.clone();
                }
            }
            ControlFlow::Continue(())
        }
    }
    let parsed = parse(&format!("SELECT * FROM {new}"))?;
    let Some(ast::Statement::Query(q)) = parsed.first() else {
        return Err(Error::new("42601", "relation name"));
    };
    let ast::SetExpr::Select(s) = q.body.as_ref() else {
        return Err(Error::new("42601", "relation name"));
    };
    let ast::TableFactor::Table { name: new, .. } = &s.from[0].relation else {
        return Err(Error::new("42601", "relation name"));
    };
    let mut statements = parse(text)?;
    if statements.len() != 1 {
        return Err(Error::new("XX001", "stored query count"));
    }
    let mut statement = statements.remove(0);
    let _ = statement.visit(&mut Rename {
        old,
        new: new.clone(),
    });
    Ok(statement.to_string())
}
