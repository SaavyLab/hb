use crate::commands::generate::types::{Cardinality, ColumnInfo, Query};
use anyhow::Result;
use rusqlite::Connection;

pub fn analyze_query(conn: &Connection, query: &mut Query) -> Result<()> {
    // 1. Use the transformed SQL from the parser (which handles parameter replacement robustly)
    // instead of re-implementing it naively here.
    let final_sql = &query.transformed_sql;

    // 2. Prepare against local SQLite to get return types
    let stmt = conn.prepare(final_sql);

    match stmt {
        Ok(stmt) => {
            let column_count = stmt.column_count();

            // Strict validation for Scalar queries
            if matches!(query.cardinality, Cardinality::Scalar) {
                if column_count != 1 {
                    anyhow::bail!(
                        "Query '{}' is marked as :scalar but returns {} columns. Scalar queries must return exactly one column.",
                        query.name,
                        column_count
                    );
                }
            }

            let mut columns = Vec::with_capacity(column_count);
            let basic_cols = stmt.columns();
            let metadata_cols = stmt.columns_with_metadata();

            for (column, column_metadata) in basic_cols.iter().zip(metadata_cols.iter()) {
                let decl_type = column.decl_type();
                let rust_type = if let Some(decl_type) = decl_type {
                    sqlite_type_to_rust(decl_type)
                        .unwrap_or("String")
                        .to_string()
                } else if let Some(hint) = &query.scalar_type_hint {
                    hint.clone()
                } else {
                    "String".to_string()
                };

                let is_nullable = if let Some(table) = column_metadata.table_name() {
                    if let Some(col_name) = column_metadata.origin_name() {
                        is_column_nullable(conn, table, col_name).unwrap_or(true)
                    } else {
                        true // Expression or unknown column
                    }
                } else {
                    true // Derived column (aggregate, literal, etc.)
                };

                columns.push(ColumnInfo {
                    name: column_metadata.name().to_string(),
                    decl: decl_type.unwrap_or("TEXT").to_string(),
                    rust_type,
                    not_null: !is_nullable,
                });
            }
            query.columns = columns;
        }
        Err(e) => {
            // It's common for prepare to fail if the query relies on a table
            // that doesn't exist in the local schema yet (if migrations failed).
            eprintln!(
                "⚠️  Warning: Could not analyze query '{}': {}",
                query.name, e
            );
        }
    }

    Ok(())
}

fn sqlite_type_to_rust(decl_type: &str) -> Result<&str> {
    let upper = decl_type.to_uppercase();
    // Handle common SQLite type affinities and aliases
    // https://www.sqlite.org/datatype3.html
    if upper.contains("INT") {
        Ok("i64")
    } else if upper.contains("CHAR") || upper.contains("CLOB") || upper.contains("TEXT") {
        Ok("String")
    } else if upper.contains("BLOB") {
        Ok("Vec<u8>")
    } else if upper.contains("REAL") || upper.contains("FLOA") || upper.contains("DOUB") {
        Ok("f64")
    } else if upper.contains("BOOL") {
        Ok("bool")
    } else if upper == "DATE" || upper == "DATETIME" {
        // D1/SQLite usually stores these as Strings or Numbers.
        // String is the safer default without more context.
        Ok("String")
    } else if upper == "NUMERIC" {
        Ok("f64")
    } else {
        // If we really can't match, fallback is handled by caller,
        // but we can return a generic error here if we want to be strict.
        // For this helper, let's default to String if it looks like text, or error.
        // But given the fallback in caller, we can just error or return String.
        Ok("String")
    }
}

#[allow(dead_code)]
fn is_column_nullable(conn: &Connection, table: &str, col_name: &str) -> Result<bool> {
    // PRAGMA table_info(table_name) returns:
    // cid | name | type | notnull | dflt_value | pk
    let mut stmt = conn.prepare(&format!("PRAGMA table_info({})", table))?;

    let rows = stmt.query_map([], |row| {
        let name: String = row.get(1)?;
        let notnull: i64 = row.get(3)?;
        Ok((name, notnull))
    })?;

    for row in rows {
        let (name, notnull) = row?;
        if name == col_name {
            // notnull is 1 if NOT NULL, 0 otherwise.
            // So nullable is true if notnull == 0
            return Ok(notnull == 0);
        }
    }

    // Column not found? Default to true (nullable)
    Ok(true)
}
