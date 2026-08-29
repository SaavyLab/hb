use std::{collections::HashSet, fs, ops::ControlFlow, path::Path};

use anyhow::{bail, Context, Result};
use heck::ToSnakeCase;
use sqlparser::{
    ast::{visit_expressions_mut, Expr, Statement, Value, ValueWithSpan},
    dialect::SQLiteDialect,
    parser::Parser,
};

use super::{Cardinality, ColumnAnnotation, Parameter, Query, TypeSpec};

const NAME: &str = "-- name:";
const PARAMS: &str = "-- params:";
const COLUMNS: &str = "-- columns:";
const INSTRUMENT: &str = "-- instrument:";

pub fn parse_query_file(path: impl AsRef<Path>) -> Result<Vec<Query>> {
    let path = path.as_ref();
    let source = fs::read_to_string(path)
        .with_context(|| format!("read query source {}", path.display()))?;
    parse_source(path, &source)
}

fn parse_source(path: &Path, source: &str) -> Result<Vec<Query>> {
    let mut blocks: Vec<(usize, String, Vec<String>)> = Vec::new();
    let mut preamble = Vec::new();

    for (index, line) in source.lines().enumerate() {
        if line.trim_start().starts_with(NAME) {
            blocks.push((index + 1, line.trim_start().to_owned(), Vec::new()));
        } else if let Some((_, _, lines)) = blocks.last_mut() {
            lines.push(line.to_owned());
        } else {
            preamble.push(line);
        }
    }

    if contains_statement(&preamble.join("\n"))? {
        bail!(
            "{}: SQL statement appears before the first -- name: annotation",
            path.display()
        );
    }

    let mut queries = Vec::with_capacity(blocks.len());
    let mut names = HashSet::new();
    let mut rust_names = HashSet::new();
    for (line, header, lines) in blocks {
        let query = parse_block(path, line, &header, &lines)?;
        if !names.insert(query.name.clone()) {
            bail!("{}: duplicate query name `{}`", path.display(), query.name);
        }
        if !rust_names.insert(query.rust_name.clone()) {
            bail!(
                "{}: query `{}` collides on generated Rust identifier `{}`",
                path.display(),
                query.name,
                query.rust_name
            );
        }
        queries.push(query);
    }
    Ok(queries)
}

fn contains_statement(source: &str) -> Result<bool> {
    Ok(!Parser::parse_sql(&SQLiteDialect {}, source)?.is_empty())
}

fn parse_block(path: &Path, line: usize, header: &str, lines: &[String]) -> Result<Query> {
    let (name, cardinality, generate_statement) =
        parse_header(header).with_context(|| format!("{}:{line}", path.display()))?;
    let rust_name = rust_identifier(&name)
        .with_context(|| format!("{}:{line}: query `{name}`", path.display()))?;

    let mut params_header: Option<Vec<(String, TypeSpec)>> = None;
    let mut columns_header: Option<Vec<ColumnAnnotation>> = None;
    let mut instrument_skip = None;
    let mut sql_lines = Vec::new();

    for source_line in lines {
        let trimmed = source_line.trim_start();
        if trimmed.starts_with(PARAMS) {
            if params_header.is_some() {
                bail!(
                    "{}:{line}: query `{name}` has multiple -- params: annotations",
                    path.display()
                );
            }
            params_header = Some(parse_typed_items(trimmed, PARAMS)?);
        } else if trimmed.starts_with(COLUMNS) {
            if columns_header.is_some() {
                bail!(
                    "{}:{line}: query `{name}` has multiple -- columns: annotations",
                    path.display()
                );
            }
            columns_header = Some(
                parse_typed_items(trimmed, COLUMNS)?
                    .into_iter()
                    .map(|(name, rust_type)| ColumnAnnotation { name, rust_type })
                    .collect(),
            );
        } else if trimmed.starts_with(INSTRUMENT) {
            if instrument_skip.is_some() {
                bail!(
                    "{}:{line}: query `{name}` has multiple -- instrument: annotations",
                    path.display()
                );
            }
            instrument_skip = Some(parse_instrument(trimmed)?);
        } else {
            sql_lines.push(source_line.as_str());
        }
    }

    let raw_sql = sql_lines.join("\n");
    let mut statements = Parser::parse_sql(&SQLiteDialect {}, &raw_sql).with_context(|| {
        format!(
            "{}:{line}: query `{name}` contains invalid SQL\n{}",
            path.display(),
            raw_sql.trim()
        )
    })?;
    if statements.is_empty() {
        bail!(
            "{}:{line}: query `{name}` contains no SQL statement",
            path.display()
        );
    }
    if statements.len() != 1 {
        bail!(
            "{}:{line}: query `{name}` must contain exactly one SQL statement; observed {}",
            path.display(),
            statements.len()
        );
    }

    let statement = statements.remove(0);
    let named_sql = statement.to_string();
    let (positional_sql, parameter_names) = positional_sql(&statement);
    let parameters = reconcile_parameters(path, &name, parameter_names, params_header)?;

    Ok(Query {
        name,
        rust_name,
        cardinality,
        source_path: path.to_path_buf(),
        named_sql,
        positional_sql,
        statement,
        parameters,
        explicit_columns: columns_header,
        columns: Vec::new(),
        instrument_skip,
        generate_statement,
    })
}

fn parse_header(line: &str) -> Result<(String, Cardinality, bool)> {
    let parts: Vec<_> = line
        .strip_prefix(NAME)
        .context("invalid query annotation")?
        .split_whitespace()
        .collect();
    if !(parts.len() == 2 || parts.len() == 3) {
        bail!("query annotation must be `-- name: Name :one|:many|:exec|:scalar [:stmt]`");
    }
    let cardinality = match parts[1] {
        ":one" => Cardinality::One,
        ":many" => Cardinality::Many,
        ":exec" => Cardinality::Exec,
        ":scalar" => Cardinality::Scalar,
        other => bail!("unknown cardinality `{other}`"),
    };
    let statement = match parts.get(2) {
        None => false,
        Some(&":stmt") => true,
        Some(other) => bail!("unknown query modifier `{other}`"),
    };
    Ok((parts[0].to_owned(), cardinality, statement))
}

fn parse_typed_items(line: &str, prefix: &str) -> Result<Vec<(String, TypeSpec)>> {
    let content = line
        .strip_prefix(prefix)
        .context("invalid type annotation")?
        .trim();
    if content.is_empty() {
        return Ok(Vec::new());
    }
    let mut result = Vec::new();
    let mut names = HashSet::new();
    for item in content.split(',') {
        let item = item.trim();
        let split = item.find(char::is_whitespace).with_context(|| {
            format!("annotation item `{item}` must have the form `name RustType`")
        })?;
        let name = item[..split].trim().to_owned();
        let type_source = item[split..].trim();
        if name.is_empty() || type_source.is_empty() {
            bail!("annotation item `{item}` must have the form `name RustType`");
        }
        if !names.insert(name.clone()) {
            bail!("duplicate annotation for `{name}`");
        }
        rust_identifier(&name).with_context(|| format!("annotation name `{name}`"))?;
        result.push((name, TypeSpec::parse(type_source)?));
    }
    Ok(result)
}

fn parse_instrument(line: &str) -> Result<Vec<String>> {
    let value = line
        .strip_prefix(INSTRUMENT)
        .context("invalid instrument annotation")?
        .trim();
    if value == "skip_all" {
        return Ok(vec!["*".to_owned()]);
    }
    if let Some(inner) = value
        .strip_prefix("skip(")
        .and_then(|value| value.strip_suffix(')'))
    {
        return Ok(inner
            .split(',')
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(str::to_owned)
            .collect());
    }
    bail!("instrument annotation must be `skip(name, ...)` or `skip_all`")
}

fn positional_sql(statement: &Statement) -> (String, Vec<String>) {
    let mut rewritten = statement.clone();
    let mut names = Vec::new();
    let _ = visit_expressions_mut(&mut rewritten, |expression| {
        if let Expr::Value(ValueWithSpan {
            value: Value::Placeholder(placeholder),
            ..
        }) = expression
        {
            if let Some(name) = placeholder.strip_prefix(':') {
                let index = match names.iter().position(|known| known == name) {
                    Some(index) => index,
                    None => {
                        names.push(name.to_owned());
                        names.len() - 1
                    }
                };
                *placeholder = format!("?{}", index + 1);
            }
        }
        ControlFlow::<()>::Continue(())
    });
    (rewritten.to_string(), names)
}

fn reconcile_parameters(
    path: &Path,
    query: &str,
    names: Vec<String>,
    explicit: Option<Vec<(String, TypeSpec)>>,
) -> Result<Vec<Parameter>> {
    let detected: HashSet<_> = names.iter().map(String::as_str).collect();
    let explicit_map = explicit.as_ref().map(|items| {
        items
            .iter()
            .map(|(name, rust_type)| (name.as_str(), rust_type.clone()))
            .collect::<std::collections::HashMap<_, _>>()
    });
    if let Some(items) = &explicit {
        let annotated: HashSet<_> = items.iter().map(|(name, _)| name.as_str()).collect();
        if annotated != detected {
            let missing: Vec<_> = names
                .iter()
                .filter(|name| !annotated.contains(name.as_str()))
                .collect();
            let unused: Vec<_> = items
                .iter()
                .filter(|(name, _)| !detected.contains(name.as_str()))
                .map(|(name, _)| name)
                .collect();
            bail!(
                "{}: query `{query}` -- params: must exactly match SQL parameters; missing {missing:?}, unused {unused:?}",
                path.display()
            );
        }
    }

    let mut rust_names = HashSet::new();
    names
        .into_iter()
        .map(|sql_name| {
            let rust_name = rust_identifier(&sql_name).with_context(|| {
                format!("{}: query `{query}` parameter `{sql_name}`", path.display())
            })?;
            if !rust_names.insert(rust_name.clone()) {
                bail!(
                    "{}: query `{query}` parameter `{sql_name}` collides on Rust identifier `{rust_name}`",
                    path.display()
                );
            }
            Ok(Parameter {
                rust_name,
                rust_type: explicit_map
                    .as_ref()
                    .and_then(|types| types.get(sql_name.as_str()).cloned()),
                sql_name,
            })
        })
        .collect()
}

pub(crate) fn rust_identifier(source: &str) -> Result<String> {
    let normalized = source.to_snake_case();
    syn::parse_str::<syn::Ident>(&normalized)
        .map_err(|_| anyhow::anyhow!("`{source}` cannot become a valid Rust identifier"))?;
    Ok(normalized)
}

#[cfg(test)]
mod tests {
    use std::io::Write;

    use tempfile::NamedTempFile;

    use super::*;

    fn parse(source: &str) -> Result<Vec<Query>> {
        let mut file = NamedTempFile::new()?;
        file.write_all(source.as_bytes())?;
        parse_query_file(file.path())
    }

    #[test]
    fn preserves_parameter_order_and_deduplicates_repeats() {
        let queries = parse("-- name: Q :exec\n-- params: b i64, a String\nUPDATE t SET a = :a WHERE b = :b OR c = :a;").unwrap();
        assert_eq!(
            queries[0]
                .parameters
                .iter()
                .map(|p| p.sql_name.as_str())
                .collect::<Vec<_>>(),
            ["a", "b"]
        );
        assert!(queries[0].positional_sql.contains("?1"));
    }

    #[test]
    fn ignores_colons_in_literals_and_comments() {
        let queries = parse("-- name: Q :scalar\n-- columns: value String\nSELECT ':not_param' AS value -- :also_not\n;").unwrap();
        assert!(queries[0].parameters.is_empty());
    }

    #[test]
    fn rejects_multiple_statements_and_duplicate_names() {
        assert!(parse("-- name: Q :exec\nDELETE FROM t; DELETE FROM u;").is_err());
        assert!(parse("-- name: Q :scalar\nSELECT 1;\n-- name: Q :scalar\nSELECT 2;").is_err());
    }

    #[test]
    fn rejects_mismatched_annotations_and_invalid_types() {
        assert!(parse("-- name: Q :scalar\n-- params: other i64\nSELECT :id;").is_err());
        assert!(parse("-- name: Q :scalar\n-- params: id Not<Type\nSELECT :id;").is_err());
    }

    #[test]
    fn rejects_empty_unannotated_and_malformed_column_blocks() {
        assert!(parse("-- name: Empty :exec\n-- only a comment").is_err());
        assert!(parse("SELECT 1;\n-- name: Q :scalar\nSELECT 2;").is_err());
        assert!(
            parse("-- name: Q :scalar\n-- columns: value Not<Type\nSELECT 1 AS value;").is_err()
        );
    }

    #[test]
    fn parses_cte_multiline_and_returning() {
        assert!(parse(
            "-- name: Q :many\n-- columns: id i64\nWITH x AS (SELECT 1 AS id)\nSELECT id FROM x;"
        )
        .is_ok());
        assert!(parse("-- name: I :one\n-- params: id i64\n-- columns: id i64\nINSERT INTO t(id) VALUES (:id) RETURNING id;").is_ok());
    }

    #[test]
    fn rejects_identifier_collisions() {
        assert!(
            parse("-- name: foo-bar :scalar\nSELECT 1;\n-- name: foo_bar :scalar\nSELECT 2;")
                .is_err()
        );
    }
}
