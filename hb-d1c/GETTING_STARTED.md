# Getting started

`hb-d1c` is an early-stage development-time generator. Choose either the Cloudflare D1 renderer or the synchronous rusqlite renderer. Generated Rust belongs in source control; the generator is not a runtime framework.

## 1. Configure

Run:

```sh
d1c init
```

The first prompt selects `d1` or `rusqlite`. A complete rusqlite configuration is:

```toml
version = 1
target = "rusqlite"

migrations_dir = "db/migrations"
queries_dir = "db/queries"
out_dir = "src/generated"
module_name = "queries"
emit_schema = true
instrument_by_default = false
```

For D1, set `target = "d1"`. D1 initialization may discover migrations from `wrangler.toml`. Rusqlite initialization has no Wrangler dependency. Tracing instrumentation is D1-only.

## 2. Add migration SQL

```sql
-- db/migrations/001_records.sql
CREATE TABLE records (
    id INTEGER PRIMARY KEY,
    broker_id TEXT NOT NULL,
    payload BLOB NOT NULL,
    note TEXT
);
```

Migration files are replayed by sorted path into one in-memory SQLite database. `schema.sql` is a generated inspection file and is never replayed. An empty migrations directory intentionally represents an empty schema. Any migration error aborts and rolls back the complete replay.

## 3. Add annotated queries

```sql
-- db/queries/records.sql
-- name: InsertRecord :exec
INSERT INTO records (broker_id, payload, note)
VALUES (:broker_id, :payload, :note);

-- name: GetRecord :one
SELECT id, broker_id, payload, note
FROM records
WHERE broker_id = :broker_id;

-- name: CountRecords :scalar
-- columns: count i64
SELECT count(*) AS count FROM records;
```

Parameter names come from SQL. Direct insert/update/comparison contexts can infer types from migration declarations. SQLite does not expose parameter types through `prepare`, so ambiguous cases must declare every parameter exactly:

```sql
-- params: broker_id String
-- params: ordinal i64
```

Expressions and aggregates commonly need exact result declarations:

```sql
-- columns: count i64
```

Repeated `-- params:` and `-- columns:` lines are additive; duplicate names still fail. Annotations use Rust type syntax and are parsed with `syn`. Unknown or ambiguous types fail; they never become `String` by default.

## 4. Generate and expose the module

```sh
d1c generate
git add src/generated db/queries/schema.sql
```

With `out_dir = "src/generated"`, keep a handwritten `src/generated.rs`:

```rust
pub mod queries;
```

The query file above is available as `crate::generated::queries::records`.

Generation requires `rustfmt` on `PATH` and formats output using the consuming project's rustfmt configuration, so a subsequent `cargo fmt` is byte-stable. Run generation twice if desired: unchanged output is not rewritten and both runs are byte-identical. Commit generated source with the query and migration change.

## 5. Call rusqlite output

Generated API shape:

```rust
let changed = insert_record(
    &connection,
    &InsertRecordParams {
        broker_id: "broker-1",
        payload: &[1, 2, 3],
        note: None,
    },
)?;
```

Bindings use `rusqlite::named_params!` with the visible SQL names. `:exec` returns affected rows; `:one` and `:scalar` return `Option`; `:many` returns `Vec`.

Application code owns transactions:

```rust
let transaction = connection.transaction()?;
insert_record(&transaction, &params)?;
transaction.commit()?;
```

No transaction boundary, pool, async wrapper, ORM, query builder, or parser is generated. The runtime dependency is only `rusqlite` plus any crates needed by explicitly annotated custom types.

## 6. Call D1 output

D1 functions remain async and use Worker primitives:

```rust
let row = get_record(&database, "broker-1").await?;
```

D1 uses generator-owned positional binding. Optional `#[tracing::instrument]` generation is configured by `instrument_by_default` and is unavailable to rusqlite.

## 7. Enforce drift in CI

```sh
d1c check
```

`check` reruns migration replay, parse, prepare, analysis, and rendering in memory. It exits nonzero for missing, stale, or extra generated modules and does not rewrite output.

See [QUERY_FORMAT.md](QUERY_FORMAT.md) for strict statement, cardinality, identifier, type, and annotation rules.
