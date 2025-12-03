#[derive(Debug)]
pub enum Cardinality {
    One,
    Many,
    Exec,
    Scalar,
}

#[derive(Debug)]
pub struct Query {
    // Parsed metadata
    pub name: String,
    pub cardinality: Cardinality,
    #[allow(dead_code)]
    pub sql: Vec<String>,
    pub params: Option<Vec<ParamSpec>>,
    #[allow(dead_code)]
    pub returns: Option<Vec<String>>,

    pub instrument_skip: Option<Vec<String>>, // New field for skip list
    pub scalar_type_hint: Option<String>,

    // Analyzer-populated metadata
    pub columns: Vec<ColumnInfo>,

    pub transformed_sql: String,

    // Source file stem (e.g. "monitors" for "monitors.sql")
    pub source_file: String,

    // Should we generate a statement function?
    pub gen_stmt: bool,
}

impl Query {
    #[allow(dead_code)]
    pub fn sql_text(&self) -> String {
        self.sql.join("\n")
    }
}

#[derive(Debug, Clone)]
pub struct ParamSpec {
    pub name: String,
    pub rust_type: String,
}

#[derive(Debug)]
pub struct ColumnInfo {
    pub name: String,
    #[allow(dead_code)]
    pub decl: String,
    pub rust_type: String,
    pub not_null: bool,
}
