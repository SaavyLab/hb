use std::{fs, ops::ControlFlow, path::PathBuf};

use anyhow::{Context, Result};
use sqlparser::{
    ast::{
        visit_expressions_mut, DataType, Expr, SelectItem, SetExpr, Statement, Value, ValueWithSpan,
    },
    dialect::SQLiteDialect,
    parser::Parser,
};

use crate::commands::generate::types::{Cardinality, ParamSpec, Query};

const QUERY_NAME: &str = "-- name:";
const QUERY_PARAMS: &str = "-- params:";
const QUERY_INSTRUMENT: &str = "-- instrument:";

struct ParamVisitor {
    found_params: Vec<String>,
}

pub fn process_query_file(file: &PathBuf) -> Result<Vec<Query>> {
    let sql_content = fs::read_to_string(file)?;
    let file_stem = file
        .file_stem()
        .ok_or_else(|| anyhow::anyhow!("Invalid filename"))?
        .to_string_lossy()
        .to_string();

    let mut lines = sql_content.lines().peekable();
    let mut queries: Vec<Query> = Vec::new();

    while let Some(line) = lines.next() {
        if line.starts_with(QUERY_NAME) {
            let mut query_sql: Vec<String> = Vec::new();
            let query_name = line.to_string();
            let mut explicit_params = Vec::new();
            let mut instrument_skip: Option<Vec<String>> = None;

            // 1. Consume lines belonging to this query block
            while let Some(next_line) = lines.peek() {
                if next_line.starts_with(QUERY_NAME) {
                    break; // Start of next query
                }

                if next_line.starts_with(QUERY_PARAMS) {
                    let parsed_params = parse_query_params(next_line)?;
                    explicit_params.extend(parsed_params);
                } else if next_line.starts_with(QUERY_INSTRUMENT) {
                    let skip_list = parse_instrument_header(next_line)?;
                    // Merge multiple instrument headers if present (e.g. skip(a) \n skip(b))
                    if let Some(existing) = &mut instrument_skip {
                        existing.extend(skip_list);
                    } else {
                        instrument_skip = Some(skip_list);
                    }
                } else {
                    let trimmed = next_line.trim();
                    // Skip empty lines and comments
                    // Note: We must check trimmed for "--" to catch indented comments
                    if !trimmed.is_empty() && !trimmed.starts_with("--") {
                        // Push the original line to preserve indentation/string literals
                        query_sql.push(next_line.to_string());
                    }
                }

                lines.next();
            }

            // 2. Parse header
            let (name, cardinality, gen_stmt) = parse_query_header(query_name.as_str())?;

            // 3. Parse and Rewrite SQL
            let raw_sql = query_sql.join("\n");
            let scalar_type_hint = if matches!(cardinality, Cardinality::Scalar) {
                infer_scalar_type(&raw_sql)
            } else {
                None
            };

            let (transformed_sql, detected_param_names) = rewrite_and_extract_params(&raw_sql)
                .with_context(|| format!("Failed to rewrite SQL for query: {}", name))?;

            // 4. Merge explicit types (from -- params:) with detected names
            // If -- params: header is present, we enforce strict validation to ensure
            // the user-supplied params match the SQL parameters exactly.

            if !explicit_params.is_empty() {
                let explicit_names: std::collections::HashSet<_> =
                    explicit_params.iter().map(|p| &p.name).collect();
                let detected_set: std::collections::HashSet<_> =
                    detected_param_names.iter().collect();

                // Check 1: Are there params used in SQL but missing from the header?
                let missing_from_header: Vec<_> = detected_param_names
                    .iter()
                    .filter(|n| !explicit_names.contains(n))
                    .collect();

                if !missing_from_header.is_empty() {
                    anyhow::bail!(
                        "Query '{}' uses parameters {:?} which are not defined in the '-- params:' header.",
                        name,
                        missing_from_header
                    );
                }

                // Check 2: Are there params defined in the header but not used in SQL?
                let unused_in_sql: Vec<_> = explicit_params
                    .iter()
                    .filter(|p| !detected_set.contains(&p.name))
                    .map(|p| &p.name)
                    .collect();

                if !unused_in_sql.is_empty() {
                    anyhow::bail!(
                        "Query '{}' defines parameters {:?} which are not used in the SQL.",
                        name,
                        unused_in_sql
                    );
                }
            }

            let mut final_params = Vec::new();

            for param_name in detected_param_names {
                let rust_type = explicit_params
                    .iter()
                    .find(|p| p.name == param_name)
                    .map(|p| p.rust_type.clone())
                    .unwrap_or_else(|| "String".to_string());

                final_params.push(ParamSpec {
                    name: param_name,
                    rust_type,
                });
            }

            queries.push(Query {
                name,
                cardinality,
                params: if final_params.is_empty() {
                    None
                } else {
                    Some(final_params)
                },
                sql: query_sql,
                transformed_sql,
                returns: None,
                instrument_skip,
                scalar_type_hint,
                columns: Vec::new(),
                source_file: file_stem.clone(),
                gen_stmt,
            });
        }
    }

    Ok(queries)
}

/// Uses sqlparser to traverse the SQL, extract :params, and rewrite them to ?1, ?2, etc.
fn rewrite_and_extract_params(sql: &str) -> Result<(String, Vec<String>)> {
    let dialect = SQLiteDialect {};

    let mut statements = Parser::parse_sql(&dialect, sql)?;

    let mut visitor = ParamVisitor {
        found_params: Vec::new(),
    };

    let _ = visit_expressions_mut(&mut statements, |expr| {
        if let Expr::Value(ValueWithSpan {
            value: Value::Placeholder(param_name),
            ..
        }) = expr
        {
            let clean_name = param_name.trim_start_matches(':').to_string();
            let idx = if let Some(i) = visitor.found_params.iter().position(|p| p == &clean_name) {
                i + 1
            } else {
                visitor.found_params.push(clean_name);
                visitor.found_params.len()
            };

            *param_name = format!("?{}", idx);
        }
        ControlFlow::<()>::Continue(())
    });

    // Reconstruct the SQL string from the modified AST
    // This has the side effect of normalizing formatting, which is good
    let transformed_sql = statements
        .iter()
        .map(|s| s.to_string())
        .collect::<Vec<String>>()
        .join("\n");

    Ok((transformed_sql, visitor.found_params))
}

pub fn parse_query_header(line: &str) -> Result<(String, Cardinality, bool)> {
    let parts = line
        .strip_prefix(QUERY_NAME)
        .ok_or(anyhow::anyhow!("Invalid query header"))?
        .split_whitespace()
        .collect::<Vec<&str>>();

    if parts.len() < 2 {
        anyhow::bail!("Query header must be: -- name: <function_name> :<cardinality> [:stmt]");
    }

    let name = parts[0].to_string();
    let cardinality = match parts[1] {
        ":one" => Cardinality::One,
        ":many" => Cardinality::Many,
        ":exec" => Cardinality::Exec,
        ":scalar" => Cardinality::Scalar,
        _ => anyhow::bail!("Invalid cardinality: {}", parts[1]),
    };

    let gen_stmt = parts.len() >= 3 && parts[2] == ":stmt";

    Ok((name, cardinality, gen_stmt))
}

fn infer_scalar_type(sql: &str) -> Option<String> {
    let dialect = SQLiteDialect {};
    let statements = Parser::parse_sql(&dialect, sql).ok()?;
    let first = statements.first()?;

    match first {
        Statement::Query(query) => infer_from_set_expr(query.body.as_ref()),
        _ => None,
    }
}

fn infer_from_set_expr(set_expr: &SetExpr) -> Option<String> {
    match set_expr {
        SetExpr::Select(select) => infer_from_select(select),
        _ => None,
    }
}

fn infer_from_select(select: &sqlparser::ast::Select) -> Option<String> {
    if select.projection.len() != 1 {
        return None;
    }

    match &select.projection[0] {
        SelectItem::UnnamedExpr(expr) => infer_from_expr(expr),
        SelectItem::ExprWithAlias { expr, .. } => infer_from_expr(expr),
        _ => None,
    }
}

fn infer_from_expr(expr: &Expr) -> Option<String> {
    match expr {
        Expr::Function(func) => {
            let name = func.name.to_string().to_uppercase();
            match name.as_str() {
                "COUNT" | "TOTAL" | "EXISTS" => Some("i64".to_string()),
                "SUM" | "AVG" => Some("f64".to_string()),
                _ => None,
            }
        }
        Expr::Exists { .. } => Some("i64".to_string()),
        Expr::Value(ValueWithSpan {
            value: Value::Number(num, _),
            ..
        }) => {
            if num.contains('.') {
                Some("f64".to_string())
            } else {
                Some("i64".to_string())
            }
        }
        Expr::Value(ValueWithSpan {
            value: Value::Boolean(_),
            ..
        }) => Some("bool".to_string()),
        Expr::Value(ValueWithSpan {
            value: Value::SingleQuotedString(_),
            ..
        }) => Some("String".to_string()),
        Expr::Cast { data_type, .. } => match data_type {
            DataType::Int(_) | DataType::Integer(_) | DataType::BigInt(_) => {
                Some("i64".to_string())
            }
            DataType::Float(_) | DataType::Double(_) | DataType::Real => Some("f64".to_string()),
            DataType::Boolean => Some("bool".to_string()),
            DataType::Text | DataType::Varchar(_) | DataType::Char(_) | DataType::String(_) => {
                Some("String".to_string())
            }
            _ => None,
        },
        _ => None,
    }
}

pub fn parse_query_params(line: &str) -> Result<Vec<ParamSpec>> {
    let parts = line
        .strip_prefix(QUERY_PARAMS)
        .ok_or(anyhow::anyhow!("Invalid query params"))?
        .split(",")
        .map(|p| p.trim())
        .collect::<Vec<&str>>();

    let mut params = Vec::new();
    for part in parts {
        if part.is_empty() {
            continue;
        }
        let pieces = part.split_whitespace().collect::<Vec<&str>>();
        if pieces.len() != 2 {
            anyhow::bail!("Params must be in the format `name Type`");
        }
        params.push(ParamSpec {
            name: pieces[0].to_string(),
            rust_type: pieces[1].to_string(),
        });
    }

    Ok(params)
}

pub fn parse_instrument_header(line: &str) -> Result<Vec<String>> {
    // Format: -- instrument: skip(password, email)
    // Or: -- instrument: skip_all

    let content = line
        .strip_prefix(QUERY_INSTRUMENT)
        .ok_or(anyhow::anyhow!("Invalid instrument header"))?
        .trim();

    if content == "skip_all" {
        return Ok(vec!["*".to_string()]); // Special marker for skipping everything
    }

    if content.starts_with("skip(") && content.ends_with(")") {
        let inner = &content[5..content.len() - 1];
        let parts: Vec<String> = inner
            .split(',')
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
            .collect();
        return Ok(parts);
    }

    // Support just comma separated list if user omits skip()?
    // No, strict format is better to allow future extensions like `level(debug)`

    anyhow::bail!("Invalid instrument directive. Expected `skip(arg1, arg2)` or `skip_all`");
}
