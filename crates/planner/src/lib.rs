//! Owned plans retain catalog epoch, exact statement basis and parameter identity.
pub use vetra_catalog;
pub use vetra_sql::{
    ast, data_type, default_expression, expression, identifier, immutable, name, parse,
    rename_column, rename_relation,
};
pub use vetra_types::sql::{Error, Result, Scalar, Type};
#[derive(Clone, Debug)]
pub struct Column {
    pub name: String,
    pub ty: Type,
    pub table: Option<u64>,
    pub column: Option<u64>,
}
#[derive(Clone, Debug)]
pub struct Plan {
    pub statement: ast::Statement,
    pub catalog_epoch: u64,
    pub basis: u64,
    pub parameters: Vec<Option<Type>>,
    pub columns: Vec<Column>,
}
impl Plan {
    pub fn check_epoch(&self, epoch: u64) -> Result<()> {
        if self.catalog_epoch != epoch {
            return Err(Error::new(
                "0A000",
                "cached plan catalog epoch changed; prepare again",
            ));
        }
        Ok(())
    }
}

pub mod bind;
