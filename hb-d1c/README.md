# hb-d1c

`hb-d1c` is an early-stage, strict SQL-to-Rust generator for SQLite schemas. It has two renderer targets:

- `d1`: asynchronous Cloudflare Workers/D1 functions;
- `rusqlite`: synchronous functions using direct `rusqlite` operations.

The generator is a development-time tool. Generated source is committed by the consuming repository. Rusqlite consumers do not depend on `hb-d1c`, an async runtime, an ORM, a query builder, or a runtime SQL parser.

## Model

```text
migration SQL
    -> replay, in sorted path order, into an in-memory SQLite database
annotated query SQL
    -> sqlparser AST + SQLite prepare and metadata analysis
strict backend-neutral query IR
    -> selected target renderer
committed generated Rust
```

Migration replay and query preparation use the same in-memory schema. Preparation, annotation, inference, migration, and renderer validation errors abort generation before output is changed.

## Configuration

`d1c.toml` is explicit and versioned:

```toml
version = 1
target = "rusqlite" # or "d1"

migrations_dir = "db/migrations"
queries_dir = "db/queries"
out_dir = "src/generated"
module_name = "queries"
emit_schema = true
emit_migrations = true
instrument_by_default = false
```

`version` and `target` are required. Unknown versions, unknown targets, and unknown fields fail. `emit_migrations` defaults to `false` for existing version 1 configurations; when enabled it generates `out_dir/migrations.rs`. `instrument_by_default` is D1-only and must be `false` for rusqlite.

Run `d1c init` for interactive setup. It asks for the target first. D1 setup can discover a single `migrations_dir` from `wrangler.toml`; rusqlite setup does not require Wrangler.

## Commands

```text
d1c init
d1c generate
d1c check
d1c watch
d1c dump-schema
```

Use `--config PATH` with any command. `generate` builds the complete output in memory before replacing files, formats generated modules with the project's `rustfmt`, avoids rewriting unchanged files, and removes stale generated Rust submodules and optional migration manifests. It never overwrites or removes a handwritten `migrations.rs`. `rustfmt` must be available on `PATH`; formatting failures abort generation. `check` runs the same strict pipeline without writing and fails on missing, stale, or extra generated output.

Typical CI:

```sh
d1c --config d1c.toml check
```

## Query format

```sql
-- name: InsertRecord :exec
INSERT INTO records (broker_id, ordinal, payload)
VALUES (:broker_id, :ordinal, :payload);

-- name: CountRecords :scalar
-- columns: count i64
SELECT count(*) AS count FROM records;
```

Cardinalities are `:exec`, `:one`, `:many`, and `:scalar`. Each annotation owns exactly one SQL statement. See [QUERY_FORMAT.md](QUERY_FORMAT.md) for the complete strictness contract.

SQLite `prepare` does not report parameter types. `hb-d1c` infers only direct, unambiguous column-bound parameters. Ambiguous parameters require an exact annotation:

```sql
-- params: ordinal i64
-- params: broker_id String
```

Repeated `-- params:` and `-- columns:` lines are additive, so large contracts can be split into reviewable lines. Names must remain unique across the complete annotation.

Expressions, aggregates, custom declarations, and other result metadata that cannot prove a Rust type require an exact result annotation:

```sql
-- columns: id i64, deleted_at Option<String>
```

Type text is parsed as Rust `syn::Type`; invalid Rust syntax fails generation. Unknown types never fall back to `String`.

## Generated rusqlite API

Parameterized queries always receive a query-specific parameter struct. String and blob inputs borrow data; primitives remain values:

```rust
pub struct InsertRecordParams<'a> {
    pub broker_id: &'a str,
    pub ordinal: i64,
    pub payload: &'a [u8],
    pub note: Option<&'a str>,
}

pub fn insert_record(
    connection: &rusqlite::Connection,
    params: &InsertRecordParams<'_>,
) -> rusqlite::Result<usize>;
```

Rusqlite SQL retains `:named` placeholders and bindings use `rusqlite::named_params!`; consumers never maintain positional correspondence. Generated cardinalities are:

| Annotation | Return type |
|---|---|
| `:exec` | `rusqlite::Result<usize>` |
| `:one` | `rusqlite::Result<Option<Row>>` |
| `:many` | `rusqlite::Result<Vec<Row>>` |
| `:scalar` | `rusqlite::Result<Option<T>>` |

Result rows derive `Debug`, `Clone`, and `PartialEq`, not Serde. Application code owns transaction boundaries. `rusqlite::Transaction` dereferences to `Connection`, so generated functions can be called with `&transaction`; generated code never begins or commits transactions.

A handwritten module such as `src/generated.rs` can expose output configured with `out_dir = "src/generated"`:

```rust
pub mod migrations;
pub mod queries;
```

Then call `crate::generated::queries::records::insert_record(...)`.

## Generated migration manifest

With `emit_migrations = true`, `out_dir/migrations.rs` contains a target-neutral `Migration` type and ordered `MIGRATIONS` slice. Each entry has an immutable ID derived from its `/`-separated path relative to `migrations_dir`, SQL rendered as a Rust string literal, and a `sha256:<hex>` checksum of the same migration contents. SQL and checksum therefore remain one generation snapshot even if a migration file is later edited without rerunning `d1c`. Renaming a file is a removed ID plus a new ID, not the same migration.

The manifest has no `hb-d1c`, rusqlite, Worker, or async runtime dependency. Application code owns migration execution, durable applied-migration metadata, locking, transaction boundaries, and recovery. It must key durable state by `id`, compare `checksum` before treating an applied ID as complete, and fail rather than silently accepting a mismatch. `d1c check` detects source additions, removals, renames, and edits because each changes the committed manifest.

## Generated D1 API

D1 output remains asynchronous, accepts `&worker::D1Database`, binds generator-owned positional SQL, and uses Worker result primitives. Optional tracing instrumentation belongs only to this renderer. D1 row values retain Serde derives required by Worker decoding.

```rust
pub async fn insert_record(
    d1: &worker::D1Database,
    broker_id: &str,
    ordinal: i64,
    payload: &[u8],
) -> worker::Result<()>;
```

D1 output contains no rusqlite operations. Rusqlite output contains no Worker, D1, wasm-bindgen, tracing, or async-runtime operations.

## Library API

The CLI is a thin caller of the reusable library:

```rust
let config = hb_d1c::Config::load("d1c.toml")?;
hb_d1c::generate(&config, ".")?;
hb_d1c::check(&config, ".")?;
```

`Schema::replay`, `parse_query_file`, `analyze_queries`, and `plan` expose schema replay, query analysis, and in-memory output planning for tests and future automation.

## Type policy and limitations

Built-in declarations map as follows:

- `INTEGER` and declarations containing `INT` -> `i64`;
- `BOOLEAN` / `BOOL` -> `bool`;
- `REAL` / `FLOAT` / `DOUBLE` -> `f64`;
- `TEXT` / `CHAR` / `CLOB` -> `String`;
- `BLOB` -> `Vec<u8>`.

`NUMERIC`, `DATE`, `DATETIME`, `JSON`, custom declaration strings, derived expressions, and ambiguous metadata require annotations. Outer joins are treated conservatively: inferred result fields remain optional when non-nullness cannot be proven. Custom annotated types must provide the target runtime conversion traits and be reachable from the generated module.

D1 generated code is token- and golden-tested but is not compiled against `worker` in this repository. The rusqlite fixture is generated, compiled independently with only `rusqlite`, and executed against an in-memory database.
