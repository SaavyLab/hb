# Query format

Each `.sql` query file produces one Rust submodule. Files are discovered and rendered in sorted path order. A query block begins with:

```sql
-- name: QueryName :one
```

Supported cardinalities:

- `:exec`: no result columns; affected-row count for rusqlite and `()` for D1;
- `:one`: one or more result columns; optional row;
- `:many`: one or more result columns; row vector;
- `:scalar`: exactly one result column; optional value.

D1 alone also accepts `:stmt` after the cardinality to emit a prepared-statement helper.

## Statement contract

Every block must contain exactly one statement. Generation rejects:

- empty blocks;
- multiple statements under one annotation;
- SQL before the first annotation;
- trailing statements that make the last block contain multiple statements;
- duplicate query names in one file;
- query names or file names that collide after Rust case normalization.

SQL is parsed with `sqlparser` using the SQLite dialect, then prepared by SQLite against the schema produced from configured migrations. String literals, quoted identifiers, and comments containing `:` do not become parameters. CTEs, multiline statements, and `RETURNING` use the same AST and prepare pipeline.

Preparation failure is fatal and reports the path, query name, SQLite error chain, and normalized SQL. Invalid queries never produce output.

## Parameters

Use named placeholders:

```sql
WHERE broker_id = :broker_id
  AND ordinal > :ordinal
  AND owner_id = :broker_id
```

First AST occurrence determines deterministic parameter order. Repeated names produce one generated parameter. D1 receives generator-owned `?1`, `?2`, and repeated-index SQL. Rusqlite retains named SQL and emits:

```rust
rusqlite::named_params! {
    ":broker_id": params.broker_id,
    ":ordinal": params.ordinal,
}
```

There is no manually maintained positional correspondence in rusqlite code.

### Inference

SQLite statement preparation does not report parameter types. The analyzer infers only straightforward column-bound contexts:

- `INSERT` value placeholders paired with target columns;
- `UPDATE column = :parameter` assignments;
- direct comparisons such as `column = :parameter` and `:parameter < column`.

Inference uses the migration declaration and conservative nullability. If no unique type can be proved, generation fails with an instruction to add `-- params:`.

### Explicit parameters

```sql
-- name: EventsAfter :many
-- params: broker_id String, ordinal i64
SELECT id, payload
FROM events
WHERE broker_id = :broker_id AND ordinal > :ordinal;
```

When present, `-- params:` must exactly match SQL parameter names. Missing names, unused names, duplicates, conflicting repeated annotations, invalid Rust types, and generated Rust identifier collisions fail. Type text is parsed as `syn::Type`, not accepted as arbitrary text.

No unknown parameter type defaults to `String`.

### Rusqlite parameter structs

Every parameterized rusqlite query gets a struct. Built-in input policy:

| Declared query type | Generated input field |
|---|---|
| `String` | `&'a str` |
| `Vec<u8>` | `&'a [u8]` |
| `Option<String>` | `Option<&'a str>` |
| `Option<Vec<u8>>` | `Option<&'a [u8]>` |
| copy primitive | value |
| explicit custom/newtype | declared type |

Custom types must implement the conversion trait required by the target runtime. Use a reachable path such as `crate::SessionOrdinal` when necessary.

## Result columns

Direct table columns use SQLite declaration metadata and origin table/column metadata. Primary keys are treated as non-null even when `PRAGMA table_info` reports a zero `notnull` flag. Outer joins are conservative: inferred fields remain `Option<T>` when non-nullness cannot be proven.

Aliases are required when a SQLite result name cannot become a stable Rust field identifier:

```sql
SELECT count(*) AS count FROM events;
```

Generated fields and query functions must be unique after Rust normalization.

### Explicit result annotations

Expressions, functions, aggregates, ambiguous metadata, and custom declarations require exact columns:

```sql
-- name: ResourceSummary :one
-- columns: uid String, event_count i64, deleted_at Option<String>
SELECT r.uid,
       count(e.id) AS event_count,
       max(e.deleted_at) AS deleted_at
FROM resources r
LEFT JOIN events e ON e.resource_id = r.id
GROUP BY r.uid;
```

`-- columns:` names, order, and count must exactly match SQLite result metadata. Types must parse as Rust types. Duplicate generated fields fail.

No unknown result type defaults to `String`. A fixture value observed as non-null does not prove non-nullability.

## Cardinality validation

SQLite result metadata enforces:

- `:scalar` has exactly one column;
- `:one` and `:many` have at least one column;
- `:exec` has zero columns.

Consequently, `SELECT ... :exec` and DML with `RETURNING ... :exec` fail. DML without `RETURNING` cannot use `:one` or `:many`.

Rusqlite renderer behavior:

```text
:exec   -> rusqlite::Result<usize>
:one    -> rusqlite::Result<Option<QueryRow>>
:many   -> rusqlite::Result<Vec<QueryRow>>
:scalar -> rusqlite::Result<Option<T>>
```

D1 uses asynchronous Worker results and preserves its existing `:exec -> Result<()>` behavior.

## SQLite declaration mapping

Strict built-in mapping:

| SQLite declaration family | Rust type |
|---|---|
| `INTEGER`, declarations containing `INT` | `i64` |
| `BOOLEAN`, `BOOL` | `bool` |
| `REAL`, `FLOAT`, `DOUBLE` | `f64` |
| `TEXT`, `CHAR`, `CLOB` | `String` |
| `BLOB` | `Vec<u8>` |

`NUMERIC`, `DATE`, `DATETIME`, `JSON`, custom declaration strings, and expression results require annotations. This deliberately avoids a false guarantee about storage representation or domain semantics.

## Instrumentation

D1 configuration may set `instrument_by_default = true`. Per-query exclusions are D1-only:

```sql
-- instrument: skip(secret)
-- instrument: skip_all
```

A skipped name must be a real query parameter. Instrumentation annotations and `:stmt` fail for the rusqlite target. Rusqlite output never imports tracing, Worker, D1, or wasm-bindgen APIs.

## Migration and output guarantees

Migration `.sql` files are sorted by path, `schema.sql` is excluded, and replay occurs in a transaction. Any failure rolls back the complete schema. An empty migrations directory is explicitly supported as an empty schema.

Generation parses, analyzes, validates, and renders every module before mutating output. It writes deterministic source, leaves unchanged files untouched, and removes stale generated `.rs` submodules. `d1c check` executes the same pipeline in memory and reports path-oriented missing, stale, and extra output.
