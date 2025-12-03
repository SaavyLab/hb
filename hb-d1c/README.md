# d1c

**Type-safe SQL queries for Cloudflare D1 + Rust Workers**

d1c generates compile-time checked Rust functions from your SQL queries. Think `sqlc` for Go, but designed for Cloudflare's edge platform.

```rust
// Write SQL with named parameters
-- name: ListMonitors :many
SELECT id, name, enabled FROM monitors WHERE org_id = :org_id;

// Get type-safe Rust functions
let monitors = d1c::queries::list_monitors(&d1, org_id).await?;
```

No positional parameter bugs. No manual JSON parsing. No runtime type errors.

> **Status:** Early development. Core features work, but expect evolution based on real-world feedback.

---

## The Problem

**Raw D1 queries are painful:**

```rust
// Positional parameters are error-prone
let result = d1.prepare("SELECT * FROM users WHERE org_id = ?1 AND active = ?2")
    .bind(&[org_id.into(), active.into()])?  // did you get the order right?
    .all()
    .await?;

// Manual JSON parsing is boilerplate-heavy
for row in result.results {
    let id = row.get("id").ok_or("missing id")?;  // hope you spelled it right
    let name = row.get("name").ok_or("missing name")?;
    // repeat for every field...
}
```

**With d1c:**

```rust
let users = queries::list_active_users(&d1, org_id).await?;
// That's it. Compile-time checked, zero boilerplate.
```

---

## Quick Start

### 1. Install

```bash
cargo install hb-d1c
```

### 2. Initialize

```bash
cd your-worker-project
d1c init
```

This reads your `wrangler.toml`, creates `d1c.toml`, and adds an example query.

### 3. Write Queries

```sql
-- db/queries/users.sql

-- name: GetUser :one
SELECT id, email, active FROM users WHERE id = :id;

-- name: ListActiveUsers :many
SELECT id, email FROM users WHERE org_id = :org_id AND active = true;

-- name: CreateUser :one
INSERT INTO users (id, email, org_id, active)
VALUES (:id, :email, :org_id, :active)
RETURNING *;
```

### 4. Generate Code

```bash
d1c generate
```

This creates `src/d1c/queries.rs` with type-safe functions for each query.

### 5. Use It

```rust
use crate::d1c::queries;

#[worker::event(fetch)]
async fn fetch(req: Request, env: Env, _ctx: Context) -> Result<Response> {
    let d1 = env.d1("DB")?;
    
    let user = queries::get_user(&d1, "user_123").await?;
    let users = queries::list_active_users(&d1, "org_456").await?;
    
    Response::ok("done")
}
```

**👉 See [GETTING_STARTED.md](GETTING_STARTED.md) for a complete tutorial and [QUERY_FORMAT.md](QUERY_FORMAT.md) for full syntax reference.**

---

## How It Works

d1c uses your Wrangler migrations as the schema source:

```
1. Parse db/migrations/*.sql (your Wrangler migration files)
2. Replay them into a local SQLite database
3. Introspect schema to understand types
4. Read db/queries/*.sql (your query files)
5. Generate type-safe Rust functions
```

**Key insight:** D1 is SQLite, so local SQLite introspection gives us perfect type information.

---

## Query Reference

### Cardinalities

Specify what your query returns with the `:cardinality` annotation:

| Cardinality | Return Type | Use Case |
|-------------|-------------|----------|
| `:one` | `Result<Option<Row>>` | Single row (or none) |
| `:many` | `Result<Vec<Row>>` | Multiple rows |
| `:exec` | `Result<()>` | INSERT/UPDATE/DELETE without RETURNING |
| `:scalar` | `Result<Option<T>>` | Single primitive value (COUNT, SUM, etc.) |

### Named Parameters

Use `:param_name` in queries:

```sql
-- name: FindUser :one
SELECT * FROM users WHERE email = :email AND active = :active;
```

Generated function signature:

```rust
pub async fn find_user(
    d1: &D1Database,
    email: &str,
    active: bool,
) -> worker::Result<Option<FindUserRow>>
```

### Headers

Override default behavior with special comments:

```sql
-- name: GetUserBalance :one
-- params: user_id UserId, currency String
SELECT balance FROM accounts WHERE user_id = :user_id AND currency = :currency;
```

**Available headers:**
- `-- params: name Type, ...` – Override inferred parameter types (useful for newtypes)
- `-- instrument: skip(field, ...)` – Exclude parameters from tracing spans (see Observability section)

**👉 See [QUERY_FORMAT.md](QUERY_FORMAT.md) for complete syntax reference, examples, and edge cases.**

---

## Commands

| Command | Description |
|---------|-------------|
| `d1c init` | Create `d1c.toml` config |
| `d1c generate` (or `gen`) | Generate Rust code from queries |
| `d1c watch` | Auto-regenerate on file changes |
| `d1c dump-schema` | Export current schema to stdout |

---

## Features

- ✅ **Named parameters** – No more positional `?1`, `?2` mistakes
- ✅ **Compile-time safety** – Typos fail at build time, not runtime
- ✅ **Zero boilerplate** – No manual JSON parsing
- ✅ **WASM-optimized** – Tiny generated code, no runtime overhead
- ✅ **Wrangler-native** – Uses your existing migration workflow
- ✅ **Watch mode** – Auto-regenerate during development

---

## Observability

d1c can add `#[tracing::instrument]` to generated functions so database spans flow into whatever telemetry backend you use (Cloudflare Workers Observability picks them up automatically when traces/logs are enabled).

**Enable during setup:**
```bash
d1c init
# → Enable tracing? [y/N] y
```

**Or add to `d1c.toml`:**
```toml
instrument_by_default = true
```

**What you get:**
- Automatic span tracking for every query (`d1c.list_users`, `d1c.get_monitor`, etc.)
- Query parameters logged by default (except sensitive fields)
- Works with built-in Workers tracing/logging (no extra crate required)

**Hide sensitive parameters:**
```sql
-- name: LoginUser :one
-- instrument: skip(password_hash)
SELECT * FROM users WHERE email = :email AND password_hash = :password_hash;
```

This generates:
```rust
#[tracing::instrument(name = "d1c.login_user", skip(d1, password_hash))]
pub async fn login_user(d1: &D1Database, email: &str, password_hash: &str) { ... }
```

---

## Configuration

`d1c.toml` in your project root:

```toml
migrations_dir = "db/migrations"  # Your Wrangler migrations
queries_dir = "db/queries"        # Your query files
codegen_dir = "src/d1c"           # Where to write generated code

# Optional
module_name = "queries"           # Generated module name (default: "queries")
instrument_by_default = false     # Add tracing spans (default: false)
```

---

## Contributing

Found a bug? Query that doesn't parse? We'd love to hear about it:
- **File issues** for bugs or missing features
- **Share your queries** if d1c fails to handle them
- **Contribute docs** for patterns you discover

Especially interested in feedback from production D1 users.

---

## License

MIT

---

**Inspired by [sqlc](https://sqlc.dev).** Built for teams running Rust Workers who want type safety without the weight of traditional ORMs.
