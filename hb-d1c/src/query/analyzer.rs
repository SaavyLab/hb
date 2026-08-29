use std::{
    collections::{BTreeMap, BTreeSet},
    ops::ControlFlow,
};

use anyhow::{bail, Context, Result};
use heck::ToSnakeCase;
use rusqlite::Connection;
use sqlparser::ast::{
    visit_expressions, visit_relations, AssignmentTarget, BinaryOperator, Expr, JoinOperator,
    ObjectName, SetExpr, Statement, TableObject, Value, ValueWithSpan,
};

use super::{Cardinality, Column, Query, TypeSpec};

#[derive(Debug, Clone)]
struct SchemaColumn {
    declaration: String,
    nullable: bool,
}

type Catalog = BTreeMap<String, BTreeMap<String, SchemaColumn>>;

pub fn analyze_queries(connection: &Connection, queries: &mut [Query]) -> Result<()> {
    let catalog = load_catalog(connection)?;
    for query in queries {
        analyze_query(connection, &catalog, query)?;
    }
    Ok(())
}

fn analyze_query(connection: &Connection, catalog: &Catalog, query: &mut Query) -> Result<()> {
    infer_parameter_types(catalog, query)?;

    let statement = connection.prepare(&query.positional_sql).with_context(|| {
        format!(
            "{}: SQLite prepare failed: {}",
            query.context(),
            query.positional_sql
        )
    })?;
    let count = statement.column_count();
    match query.cardinality {
        Cardinality::Scalar if count != 1 => bail!(
            "{}: :scalar requires exactly one result column; observed {count}",
            query.context()
        ),
        Cardinality::One | Cardinality::Many if count == 0 => bail!(
            "{}: {:?} requires one or more result columns; observed 0",
            query.context(),
            query.cardinality
        ),
        Cardinality::Exec if count != 0 => bail!(
            "{}: :exec requires no result columns; observed {count}; SELECT and RETURNING statements need :one, :many, or :scalar",
            query.context()
        ),
        _ => {}
    }

    let basic = statement.columns();
    let metadata = statement.columns_with_metadata();
    let outer_join = has_outer_join(&query.statement);
    let mut columns = Vec::with_capacity(count);
    let mut rust_names = BTreeSet::new();

    for (index, (column, metadata)) in basic.iter().zip(metadata.iter()).enumerate() {
        let sql_name = metadata.name().to_owned();
        let rust_name = sql_name.to_snake_case();
        syn::parse_str::<syn::Ident>(&rust_name).map_err(|_| {
            anyhow::anyhow!(
                "{}: result column {index} `{sql_name}` is not a stable Rust identifier; add an SQL alias",
                query.context()
            )
        })?;
        if !rust_names.insert(rust_name.clone()) {
            bail!(
                "{}: result column `{sql_name}` collides on generated field `{rust_name}`",
                query.context()
            );
        }

        let explicit = query
            .explicit_columns
            .as_ref()
            .and_then(|items| items.get(index));
        if let Some(items) = &query.explicit_columns {
            if items.len() != count {
                bail!(
                    "{}: -- columns: count mismatch; expected {count}, observed {}",
                    query.context(),
                    items.len()
                );
            }
            if explicit.is_some_and(|item| item.name != sql_name) {
                bail!(
                    "{}: -- columns: name mismatch at index {index}; expected `{sql_name}`, observed `{}`",
                    query.context(),
                    explicit.unwrap().name
                );
            }
        }

        let declaration = column.decl_type().map(str::to_owned);
        let (rust_type, nullable) = if let Some(annotation) = explicit {
            (
                annotation.rust_type.clone(),
                type_is_option(&annotation.rust_type.syntax),
            )
        } else {
            let declaration = declaration.as_deref().with_context(|| {
                format!(
                    "{}: result column `{sql_name}` is an expression or has unknown declaration type; add `-- columns: {sql_name} RustType`",
                    query.context()
                )
            })?;
            let base = map_declaration(declaration)
                .with_context(|| format!("{}: result column `{sql_name}`", query.context()))?;
            let nullable = outer_join
                || match (metadata.table_name(), metadata.origin_name()) {
                    (Some(table), Some(origin)) => catalog
                        .get(table)
                        .and_then(|columns| columns.get(origin))
                        .map(|column| column.nullable)
                        .unwrap_or(true),
                    _ => true,
                };
            let rust_type = if nullable {
                TypeSpec::option(base)?
            } else {
                TypeSpec::parse(base)?
            };
            (rust_type, nullable)
        };

        columns.push(Column {
            sql_name,
            rust_name,
            rust_type,
            declaration,
            nullable,
        });
    }
    query.columns = columns;
    Ok(())
}

fn infer_parameter_types(catalog: &Catalog, query: &mut Query) -> Result<()> {
    let unresolved: BTreeSet<_> = query
        .parameters
        .iter()
        .filter(|parameter| parameter.rust_type.is_none())
        .map(|parameter| parameter.sql_name.clone())
        .collect();
    if unresolved.is_empty() {
        return Ok(());
    }

    let mut candidates: BTreeMap<String, Vec<TypeSpec>> = BTreeMap::new();
    infer_insert(catalog, &query.statement, &mut candidates)?;
    infer_update(catalog, &query.statement, &mut candidates)?;
    infer_comparisons(catalog, &query.statement, &mut candidates)?;
    let query_context = query.context();

    for parameter in &mut query.parameters {
        if parameter.rust_type.is_some() {
            continue;
        }
        let types = candidates.remove(&parameter.sql_name).unwrap_or_default();
        let unique: BTreeSet<_> = types.iter().map(|item| item.source.as_str()).collect();
        if unique.len() != 1 {
            bail!(
                "{}: parameter `{}` has ambiguous SQLite type inference ({:?}); add `-- params: {} RustType`",
                query_context,
                parameter.sql_name,
                unique,
                parameter.sql_name
            );
        }
        parameter.rust_type = types.into_iter().next();
    }
    Ok(())
}

fn infer_insert(
    catalog: &Catalog,
    statement: &Statement,
    candidates: &mut BTreeMap<String, Vec<TypeSpec>>,
) -> Result<()> {
    let Statement::Insert(insert) = statement else {
        return Ok(());
    };
    let TableObject::TableName(table) = &insert.table else {
        return Ok(());
    };
    let Some(source) = &insert.source else {
        return Ok(());
    };
    let SetExpr::Values(values) = source.body.as_ref() else {
        return Ok(());
    };
    for row in &values.rows {
        for (index, expression) in row.iter().enumerate() {
            let Some(parameter) = placeholder(expression) else {
                continue;
            };
            let Some(column) = insert.columns.get(index) else {
                continue;
            };
            add_column_candidate(
                catalog,
                &table_name(table),
                &column.value,
                parameter,
                candidates,
            )?;
        }
    }
    Ok(())
}

fn infer_update(
    catalog: &Catalog,
    statement: &Statement,
    candidates: &mut BTreeMap<String, Vec<TypeSpec>>,
) -> Result<()> {
    let Statement::Update {
        table, assignments, ..
    } = statement
    else {
        return Ok(());
    };
    let table_name = relation_name(&table.relation);
    let Some(table_name) = table_name else {
        return Ok(());
    };
    for assignment in assignments {
        let Some(parameter) = placeholder(&assignment.value) else {
            continue;
        };
        let AssignmentTarget::ColumnName(column) = &assignment.target else {
            continue;
        };
        if let Some(column_name) = object_last(column) {
            add_column_candidate(catalog, &table_name, column_name, parameter, candidates)?;
        }
    }
    Ok(())
}

fn infer_comparisons(
    catalog: &Catalog,
    statement: &Statement,
    candidates: &mut BTreeMap<String, Vec<TypeSpec>>,
) -> Result<()> {
    let mut relations = BTreeSet::new();
    let _ = visit_relations(statement, |relation| {
        let name = table_name(relation);
        if catalog.contains_key(&name) {
            relations.insert(name);
        }
        ControlFlow::<()>::Continue(())
    });
    let _ = visit_expressions(statement, |expression| {
        if let Expr::BinaryOp { left, op, right } = expression {
            if matches!(
                op,
                BinaryOperator::Eq
                    | BinaryOperator::NotEq
                    | BinaryOperator::Lt
                    | BinaryOperator::LtEq
                    | BinaryOperator::Gt
                    | BinaryOperator::GtEq
            ) {
                if let (Some(column), Some(parameter)) = (column_name(left), placeholder(right)) {
                    let _ = add_comparison_candidate(
                        catalog, &relations, column, parameter, candidates,
                    );
                }
                if let (Some(parameter), Some(column)) = (placeholder(left), column_name(right)) {
                    let _ = add_comparison_candidate(
                        catalog, &relations, column, parameter, candidates,
                    );
                }
            }
        }
        ControlFlow::<()>::Continue(())
    });
    Ok(())
}

fn add_comparison_candidate(
    catalog: &Catalog,
    relations: &BTreeSet<String>,
    column: (&str, Option<&str>),
    parameter: &str,
    candidates: &mut BTreeMap<String, Vec<TypeSpec>>,
) -> Result<()> {
    let (column_name, qualifier) = column;
    if let Some(qualifier) = qualifier {
        if catalog.contains_key(qualifier) {
            return add_column_candidate(catalog, qualifier, column_name, parameter, candidates);
        }
    }
    let matches = relations
        .iter()
        .filter(|table| {
            catalog
                .get(*table)
                .is_some_and(|columns| columns.contains_key(column_name))
        })
        .cloned()
        .collect::<Vec<_>>();
    if matches.len() == 1 {
        add_column_candidate(catalog, &matches[0], column_name, parameter, candidates)?;
    }
    Ok(())
}

fn add_column_candidate(
    catalog: &Catalog,
    table: &str,
    column: &str,
    parameter: &str,
    candidates: &mut BTreeMap<String, Vec<TypeSpec>>,
) -> Result<()> {
    let Some(schema_column) = catalog.get(table).and_then(|columns| columns.get(column)) else {
        return Ok(());
    };
    let base = map_declaration(&schema_column.declaration)?;
    let rust_type = if schema_column.nullable {
        TypeSpec::option(base)?
    } else {
        TypeSpec::parse(base)?
    };
    candidates
        .entry(parameter.to_owned())
        .or_default()
        .push(rust_type);
    Ok(())
}

fn load_catalog(connection: &Connection) -> Result<Catalog> {
    let mut tables = connection.prepare(
        "SELECT name FROM sqlite_schema WHERE type IN ('table', 'view') AND name NOT LIKE 'sqlite_%' ORDER BY name",
    )?;
    let names = tables
        .query_map([], |row| row.get::<_, String>(0))?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    let mut catalog = Catalog::new();
    for table in names {
        let escaped = table.replace('"', "\"\"");
        let mut statement = connection.prepare(&format!("PRAGMA table_info(\"{escaped}\")"))?;
        let rows = statement.query_map([], |row| {
            let name: String = row.get(1)?;
            let declaration: String = row.get(2)?;
            let not_null: bool = row.get(3)?;
            let primary_key: i64 = row.get(5)?;
            Ok((
                name,
                SchemaColumn {
                    declaration,
                    nullable: !not_null && primary_key == 0,
                },
            ))
        })?;
        catalog.insert(table, rows.collect::<rusqlite::Result<_>>()?);
    }
    Ok(catalog)
}

fn map_declaration(declaration: &str) -> Result<&'static str> {
    let upper = declaration.trim().to_ascii_uppercase();
    if upper.contains("BOOL") {
        Ok("bool")
    } else if upper.contains("INT") {
        Ok("i64")
    } else if upper.contains("CHAR") || upper.contains("CLOB") || upper.contains("TEXT") {
        Ok("String")
    } else if upper.contains("BLOB") {
        Ok("Vec<u8>")
    } else if upper.contains("REAL") || upper.contains("FLOA") || upper.contains("DOUB") {
        Ok("f64")
    } else {
        bail!(
            "SQLite declaration `{declaration}` has no strict built-in Rust mapping; annotate the parameter or result (NUMERIC, DATE, DATETIME, JSON, and custom declarations require annotations)"
        )
    }
}

fn placeholder(expression: &Expr) -> Option<&str> {
    match expression {
        Expr::Value(ValueWithSpan {
            value: Value::Placeholder(value),
            ..
        }) => value.strip_prefix(':'),
        _ => None,
    }
}

fn column_name(expression: &Expr) -> Option<(&str, Option<&str>)> {
    match expression {
        Expr::Identifier(identifier) => Some((&identifier.value, None)),
        Expr::CompoundIdentifier(parts) if parts.len() >= 2 => Some((
            &parts.last()?.value,
            Some(&parts.get(parts.len() - 2)?.value),
        )),
        _ => None,
    }
}

fn table_name(name: &ObjectName) -> String {
    name.to_string()
        .split('.')
        .next_back()
        .unwrap_or_default()
        .trim_matches('"')
        .to_owned()
}

fn object_last(name: &ObjectName) -> Option<&str> {
    name.0
        .last()
        .and_then(|part| part.as_ident())
        .map(|ident| ident.value.as_str())
}

fn relation_name(relation: &sqlparser::ast::TableFactor) -> Option<String> {
    match relation {
        sqlparser::ast::TableFactor::Table { name, .. } => Some(table_name(name)),
        _ => None,
    }
}

fn type_is_option(rust_type: &syn::Type) -> bool {
    matches!(rust_type, syn::Type::Path(path) if path.path.segments.last().is_some_and(|segment| segment.ident == "Option"))
}

fn has_outer_join(statement: &Statement) -> bool {
    let Statement::Query(query) = statement else {
        return false;
    };
    let SetExpr::Select(select) = query.body.as_ref() else {
        return false;
    };
    select
        .from
        .iter()
        .flat_map(|table| &table.joins)
        .any(|join| {
            matches!(
                join.join_operator,
                JoinOperator::Left(_)
                    | JoinOperator::LeftOuter(_)
                    | JoinOperator::Right(_)
                    | JoinOperator::RightOuter(_)
                    | JoinOperator::FullOuter(_)
                    | JoinOperator::OuterApply
            )
        })
}

#[cfg(test)]
mod tests {
    use std::io::Write;

    use tempfile::NamedTempFile;

    use crate::query::parse_query_file;

    use super::*;

    fn analyze(sql: &str, query: &str) -> Result<Vec<Query>> {
        let connection = Connection::open_in_memory()?;
        connection.execute_batch(sql)?;
        let mut file = NamedTempFile::new()?;
        file.write_all(query.as_bytes())?;
        let mut queries = parse_query_file(file.path())?;
        analyze_queries(&connection, &mut queries)?;
        Ok(queries)
    }

    #[test]
    fn maps_declarations_and_nullability() {
        let queries = analyze(
            "CREATE TABLE value_types(id INTEGER PRIMARY KEY, text_value TEXT NOT NULL, real_value REAL NOT NULL, blob_value BLOB, bool_value BOOL NOT NULL);",
            "-- name: Values :many\nSELECT id, text_value, real_value, blob_value, bool_value FROM value_types;",
        ).unwrap();
        let types = queries[0]
            .columns
            .iter()
            .map(|column| column.rust_type.source.as_str())
            .collect::<Vec<_>>();
        assert_eq!(types, ["i64", "String", "f64", "Option<Vec<u8>>", "bool"]);
    }

    #[test]
    fn preparation_errors_are_fatal() {
        let error = analyze("", "-- name: Missing :many\nSELECT id FROM absent;").unwrap_err();
        assert!(format!("{error:#}").contains("SQLite prepare failed"));
    }

    #[test]
    fn missing_columns_are_fatal() {
        let error = analyze(
            "CREATE TABLE present(id INTEGER);",
            "-- name: MissingColumn :many\nSELECT absent FROM present;",
        )
        .unwrap_err();
        assert!(format!("{error:#}").contains("SQLite prepare failed"));
    }

    #[test]
    fn expressions_require_columns_annotations() {
        assert!(analyze(
            "CREATE TABLE t(id INTEGER);",
            "-- name: Count :scalar\nSELECT count(*) AS count FROM t;"
        )
        .is_err());
        assert!(analyze(
            "CREATE TABLE t(id INTEGER);",
            "-- name: Count :scalar\n-- columns: count i64\nSELECT count(*) AS count FROM t;"
        )
        .is_ok());
    }

    #[test]
    fn explicit_columns_must_match_metadata_and_fields_must_be_unique() {
        assert!(analyze(
            "CREATE TABLE t(id INTEGER);",
            "-- name: Count :scalar\n-- columns: wrong i64\nSELECT count(*) AS count FROM t;",
        )
        .is_err());
        assert!(analyze(
            "CREATE TABLE t(id INTEGER);",
            "-- name: Collision :many\nSELECT id AS foo_bar, id AS fooBar FROM t;",
        )
        .is_err());
    }

    #[test]
    fn infers_insert_update_and_comparison_parameters() {
        let queries = analyze(
            "CREATE TABLE t(id INTEGER PRIMARY KEY, name TEXT, payload BLOB NOT NULL);",
            "-- name: Insert :exec\nINSERT INTO t(id, name, payload) VALUES (:id, :name, :payload);\n-- name: Update :exec\nUPDATE t SET name = :name WHERE id = :id;",
        ).unwrap();
        assert_eq!(
            queries[0].parameters[0].rust_type.as_ref().unwrap().source,
            "i64"
        );
        assert_eq!(
            queries[0].parameters[1].rust_type.as_ref().unwrap().source,
            "Option<String>"
        );
        assert_eq!(
            queries[1].parameters[1].rust_type.as_ref().unwrap().source,
            "i64"
        );
    }

    #[test]
    fn unknown_declarations_and_ambiguous_parameters_fail() {
        assert!(analyze(
            "CREATE TABLE t(value JSON);",
            "-- name: Q :many\nSELECT value FROM t;"
        )
        .is_err());
        assert!(analyze(
            "CREATE TABLE t(id INTEGER);",
            "-- name: Q :scalar\n-- columns: value i64\nSELECT :value AS value;"
        )
        .is_err());
    }

    #[test]
    fn primary_keys_are_non_null_and_outer_joins_are_conservative() {
        let queries = analyze(
            "CREATE TABLE a(id INTEGER PRIMARY KEY); CREATE TABLE b(id INTEGER PRIMARY KEY, a_id INTEGER NOT NULL);",
            "-- name: Q :many\nSELECT a.id AS a_id, b.id AS b_id FROM a LEFT JOIN b ON b.a_id = a.id;",
        ).unwrap();
        assert_eq!(queries[0].columns[0].rust_type.source, "Option<i64>");
        assert_eq!(queries[0].columns[1].rust_type.source, "Option<i64>");
    }

    #[test]
    fn invalid_sql_variants_fail() {
        assert!(analyze(
            "CREATE TABLE t(id INTEGER);",
            "-- name: Returning :exec\nINSERT INTO t(id) VALUES (1) RETURNING id;",
        )
        .is_err());
        assert!(analyze(
            "CREATE TABLE t(id INTEGER);",
            "-- name: Q :exec\nSELECT id FROM t;"
        )
        .is_err());
        assert!(analyze(
            "CREATE TABLE t(id INTEGER);",
            "-- name: Q :one\nDELETE FROM t;"
        )
        .is_err());
    }
}
