use std::path::PathBuf;

use syn::Type;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Cardinality {
    One,
    Many,
    Exec,
    Scalar,
}

#[derive(Debug, Clone)]
pub struct TypeSpec {
    pub source: String,
    pub syntax: Type,
}

impl TypeSpec {
    pub fn parse(source: &str) -> anyhow::Result<Self> {
        let source = source.trim().to_owned();
        let syntax = syn::parse_str::<Type>(&source)
            .map_err(|error| anyhow::anyhow!("invalid Rust type `{source}`: {error}"))?;
        Ok(Self { source, syntax })
    }

    pub fn option(inner: &str) -> anyhow::Result<Self> {
        Self::parse(&format!("Option<{inner}>"))
    }
}

#[derive(Debug, Clone)]
pub struct Parameter {
    pub sql_name: String,
    pub rust_name: String,
    pub rust_type: Option<TypeSpec>,
}

#[derive(Debug, Clone)]
pub struct Column {
    pub sql_name: String,
    pub rust_name: String,
    pub rust_type: TypeSpec,
    pub declaration: Option<String>,
    pub nullable: bool,
}

#[derive(Debug, Clone)]
pub struct ColumnAnnotation {
    pub name: String,
    pub rust_type: TypeSpec,
}

#[derive(Debug, Clone)]
pub struct Query {
    pub name: String,
    pub rust_name: String,
    pub cardinality: Cardinality,
    pub source_path: PathBuf,
    pub named_sql: String,
    pub positional_sql: String,
    pub statement: sqlparser::ast::Statement,
    pub parameters: Vec<Parameter>,
    pub explicit_columns: Option<Vec<ColumnAnnotation>>,
    pub columns: Vec<Column>,
    pub instrument_skip: Option<Vec<String>>,
    pub generate_statement: bool,
}

impl Query {
    pub fn context(&self) -> String {
        format!("{}: query `{}`", self.source_path.display(), self.name)
    }
}
