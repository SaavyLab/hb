# Query Format Reference

This document describes the complete syntax for writing d1c queries.

---

## Query Structure

Every query file contains one or more queries separated by blank lines. Each query has:

1. **A header section** (comment lines starting with `--`)
2. **The SQL statement** (can span multiple lines)

```sql
-- name: GetUser :one
-- params: id UserId
SELECT id, email, active FROM users WHERE id = :id;

-- name: ListUsers :many
SELECT id, email FROM users WHERE active = true;
```

---

## Headers

Headers are special comments that control code generation. They must appear **before** the SQL statement.

### `-- name: FunctionName :cardinality` (Required)

Specifies the generated function name and return type.

**Syntax:**
```sql
-- name: FunctionName :cardinality
```

**Rules:**
- Function names must be valid Rust identifiers (snake_case recommended)
- Cardinality must be one of: `:one`, `:many`, `:exec`, `:scalar`
- This header is **required** for every query
- Optional `:stmt` flag generates a prepared statement function (see "Batching" below)

**Example:**
```sql
-- name: get_user_by_email :one
SELECT * FROM users WHERE email = :email;
```

Generates:
```rust
pub async fn get_user_by_email(
    d1: &D1Database,
    email: &str,
) -> worker::Result<Option<GetUserByEmailRow>>
```

---

### `-- params: name Type, ...` (Optional)

Override inferred parameter types. By default, d1c infers types from your SQL, but this header lets you specify exact Rust types.

**Syntax:**
```sql
-- params: param_name Type, another_param AnotherType
```

**When to use:**
- Using newtype wrappers (`UserId`, `OrgId`, etc.)
- Type inference fails (complex expressions, functions)
- Want a different type than the default (e.g., `i64` instead of `String`)

**Example:**
```sql
-- name: GetUserBalance :one
-- params: user_id UserId, currency Currency
SELECT balance FROM accounts 
WHERE user_id = :user_id AND currency = :currency;
```

Generates:
```rust
pub async fn get_user_balance(
    d1: &D1Database,
    user_id: UserId,          // ← your custom type
    currency: Currency,       // ← your custom type
) -> worker::Result<Option<GetUserBalanceRow>>
```

**Important:** Types must be in scope when the generated code is used. Import them in your module:
```rust
mod d1c;
use crate::types::{UserId, Currency};
use crate::d1c::queries::*;
```

---

### `-- instrument: skip(param, ...)` (Optional)

Exclude sensitive or large parameters from tracing spans. Only relevant when `instrument_by_default = true` in `d1c.toml`.

**Syntax:**
```sql
-- instrument: skip(param_name, another_param)
```

**When to use:**
- Passwords, API keys, tokens
- Large binary data (images, files)
- PII that shouldn't appear in logs

**Example:**
```sql
-- name: AuthenticateUser :one
-- instrument: skip(password_hash)
SELECT id, email FROM users 
WHERE email = :email AND password_hash = :password_hash;
```

Generates:
```rust
#[tracing::instrument(name = "d1c.authenticate_user", skip(d1, password_hash))]
pub async fn authenticate_user(
    d1: &D1Database,
    email: &str,
    password_hash: &str,  // ← excluded from tracing
) -> worker::Result<Option<AuthenticateUserRow>>
```

**See:** [Observability](README.md#observability) for more details.

---

## Cardinalities

Cardinality determines what the query returns.

### `:one` – Single Row (or None)

Use when you expect **0 or 1 results**.

**Returns:** `Result<Option<Row>>`

**Example:**
```sql
-- name: GetUserById :one
SELECT id, email, active FROM users WHERE id = :id;
```

**Generated:**
```rust
pub async fn get_user_by_id(
    d1: &D1Database,
    id: &str,
) -> worker::Result<Option<GetUserByIdRow>>
```

**Usage:**
```rust
let user = get_user_by_id(&d1, "usr_123").await?;
match user {
    Some(u) => println!("Found: {}", u.email),
    None => println!("Not found"),
}
```

**Common use cases:**
- `SELECT ... WHERE id = :id` (primary key lookup)
- `SELECT ... WHERE unique_column = :value`
- `INSERT ... RETURNING *` (single insert)
- `UPDATE ... WHERE id = :id RETURNING *`

---

### `:many` – Multiple Rows

Use when you expect **0 or more results**.

**Returns:** `Result<Vec<Row>>`

**Example:**
```sql
-- name: ListActiveUsers :many
SELECT id, email FROM users WHERE active = true ORDER BY email;
```

**Generated:**
```rust
pub async fn list_active_users(
    d1: &D1Database,
) -> worker::Result<Vec<ListActiveUsersRow>>
```

**Usage:**
```rust
let users = list_active_users(&d1).await?;
for user in users {
    println!("{}: {}", user.id, user.email);
}
```

**Common use cases:**
- `SELECT ... WHERE org_id = :org_id` (filter by tenant)
- `SELECT ... ORDER BY created_at DESC LIMIT 10` (pagination)
- `SELECT ... JOIN ...` (queries with joins)

---

### `:exec` – No Return Value

Use for **INSERT/UPDATE/DELETE** without `RETURNING`.

**Returns:** `Result<()>`

**Example:**
```sql
-- name: UpdateUserActive :exec
UPDATE users SET active = :active WHERE id = :id;
```

**Generated:**
```rust
pub async fn update_user_active(
    d1: &D1Database,
    active: bool,
    id: &str,
) -> worker::Result<()>
```

**Usage:**
```rust
update_user_active(&d1, true, "usr_123").await?;
// No return value, just success/failure
```

**Common use cases:**
- `UPDATE ... SET ... WHERE ...` (updates without needing result)
- `DELETE FROM ... WHERE ...` (deletions)
- `INSERT INTO ... VALUES (...)` (inserts without needing generated IDs)

**Note:** If you need the affected row, use `:one` with `RETURNING *` instead.

---

### `:scalar` – Single Primitive Value

Use when you select **a single column** (typically aggregates like `COUNT`, `SUM`, `MAX`).

**Returns:** `Result<Option<T>>` where `T` is the primitive type (e.g., `i64`, `String`, `f64`)

**Example:**
```sql
-- name: CountActiveUsers :scalar
SELECT COUNT(*) FROM users WHERE active = true;
```

**Generated:**
```rust
pub async fn count_active_users(
    d1: &D1Database,
) -> worker::Result<Option<i64>>
```

**Usage:**
```rust
let count = count_active_users(&d1).await?;
println!("Active users: {}", count.unwrap_or(0));
```

**Common use cases:**
- `SELECT COUNT(*) FROM ...`
- `SELECT MAX(created_at) FROM ...`
- `SELECT SUM(amount) FROM ...`
- `SELECT EXISTS(SELECT 1 FROM ... WHERE ...)` (returns integer 0 or 1)

**Important:** Query must return exactly **one column**. Multiple columns will cause a compile error.

---

## Named Parameters

d1c uses named parameters (`:param_name`) instead of positional parameters (`?1`, `?2`).

**Syntax:**
```sql
:param_name
```

**Rules:**
- Must start with `:`
- Must be valid Rust identifiers (alphanumeric + underscore)
- Case-sensitive (`:user_id` ≠ `:User_Id`)

**Example:**
```sql
-- name: FindUser :one
SELECT * FROM users 
WHERE email = :email 
  AND org_id = :org_id 
  AND active = :active;
```

**Generated function signature:**
```rust
pub async fn find_user(
    d1: &D1Database,
    email: &str,         // ← parameters in order they appear
    org_id: &str,
    active: bool,
) -> worker::Result<Option<FindUserRow>>
```

**Behind the scenes:** d1c rewrites named parameters to positional parameters (`?1`, `?2`, etc.) so D1 sees standard SQLite syntax. You get the ergonomics without runtime cost.

---

## Type Inference

d1c automatically infers Rust types from SQLite types.

### Basic Type Mapping

| SQLite Type | Rust Type | Notes |
|------------|-----------|-------|
| `TEXT` | `String` | Parameters use `&str` |
| `INTEGER` | `i64` | Also matches `INT`, `BIGINT` |
| `REAL` | `f64` | Also matches `FLOAT`, `DOUBLE` |
| `BLOB` | `Vec<u8>` | Parameters use `&[u8]` |
| `BOOLEAN` | `bool` | SQLite uses 0/1 integers |

**Function parameters use references** for efficiency:
- `String` → `&str`
- `Vec<u8>` → `&[u8]`

**Return types use owned values:**
- Result structs have `String`, `Vec<u8>`, etc.

### Nullability

d1c inspects your schema to determine if columns are nullable.

**Schema:**
```sql
CREATE TABLE users (
  id TEXT PRIMARY KEY,           -- NOT NULL (primary key)
  email TEXT NOT NULL,           -- NOT NULL (explicit)
  phone TEXT,                    -- nullable (no constraint)
  active BOOLEAN NOT NULL DEFAULT TRUE
);
```

**Query:**
```sql
-- name: GetUser :one
SELECT id, email, phone, active FROM users WHERE id = :id;
```

**Generated:**
```rust
pub struct GetUserRow {
    pub id: String,            // NOT NULL
    pub email: String,         // NOT NULL
    pub phone: Option<String>, // nullable
    pub active: bool,          // NOT NULL
}
```

**Handling `Option` fields:**
```rust
let user = get_user(&d1, "usr_123").await?;
if let Some(u) = user {
    println!("Email: {}", u.email);
    match u.phone {
        Some(phone) => println!("Phone: {}", phone),
        None => println!("No phone"),
    }
}
```

---

## Common Patterns

### COUNT Queries

Use `:scalar` for aggregate functions:

```sql
-- name: CountUsersByOrg :scalar
SELECT COUNT(*) FROM users WHERE org_id = :org_id;

-- name: SumOrderTotals :scalar
SELECT SUM(total) FROM orders WHERE user_id = :user_id;
```

### Pagination

Use `:many` with `LIMIT`/`OFFSET`:

```sql
-- name: ListUsersPaginated :many
SELECT id, email FROM users 
ORDER BY created_at DESC 
LIMIT :limit OFFSET :offset;
```

**Generated:**
```rust
pub async fn list_users_paginated(
    d1: &D1Database,
    limit: i64,
    offset: i64,
) -> worker::Result<Vec<ListUsersPaginatedRow>>
```

### JOINs

d1c flattens joined columns:

```sql
-- name: GetUserWithOrg :one
SELECT 
  u.id as user_id,
  u.email,
  o.id as org_id,
  o.name as org_name
FROM users u
JOIN organizations o ON u.org_id = o.id
WHERE u.id = :user_id;
```

**Generated:**
```rust
pub struct GetUserWithOrgRow {
    pub user_id: String,
    pub email: String,
    pub org_id: String,
    pub org_name: String,
}
```

### INSERT with RETURNING

Use `:one` to get the inserted row:

```sql
-- name: CreateUser :one
INSERT INTO users (id, email, org_id, active)
VALUES (:id, :email, :org_id, :active)
RETURNING *;
```

**Generated:**
```rust
pub async fn create_user(
    d1: &D1Database,
    id: &str,
    email: &str,
    org_id: &str,
    active: bool,
) -> worker::Result<Option<CreateUserRow>>
```

### UPDATE with RETURNING

Similar to INSERT:

```sql
-- name: ArchiveMonitor :one
UPDATE monitors 
SET archived = true, archived_at = :archived_at
WHERE id = :id
RETURNING *;
```

### Bulk Inserts (No Return)

Use `:exec` when you don't need results:

```sql
-- name: BulkCreateLogs :exec
INSERT INTO logs (id, level, message, timestamp)
VALUES (:id, :level, :message, :timestamp);
```

### Batching with `:stmt`

If you need to perform batch operations (like `d1.batch()`), you can generate a prepared statement constructor by adding the `:stmt` flag to your query header.

```sql
-- name: CreateLog :exec :stmt
INSERT INTO logs (id, message) VALUES (:id, :message);
```

**Generates two functions:**
1. `create_log_stmt(d1, ...)` -> Returns `Result<D1PreparedStatement>`
2. `create_log(d1, ...)` -> Executes the query (helper wrapper)

**Usage:**

```rust
let stmt1 = create_log_stmt(&d1, "1", "log 1")?;
let stmt2 = create_log_stmt(&d1, "2", "log 2")?;
let results = d1.batch(vec![stmt1, stmt2]).await?;
```

### Exists Checks

Use `:scalar` with `EXISTS`:

```sql
-- name: UserExists :scalar
SELECT EXISTS(SELECT 1 FROM users WHERE email = :email);
```

**Returns:** `Result<Option<i64>>` (0 or 1)

**Usage:**
```rust
let exists = user_exists(&d1, "test@example.com").await?;
if exists == Some(1) {
    println!("User exists");
}
```

---

## Edge Cases

### Complex Expressions

Type inference may fail for complex expressions. Use `-- params:` to specify:

```sql
-- name: ComplexQuery :one
-- params: threshold i64
SELECT * FROM stats 
WHERE value > :threshold * 1.5;  -- expression might confuse inference
```

### JSON Columns

SQLite stores JSON as TEXT. Use `String` and parse manually:

```sql
-- name: GetMetadata :one
SELECT id, metadata FROM resources WHERE id = :id;
```

```rust
let row = get_metadata(&d1, "res_123").await?;
if let Some(r) = row {
    let meta: serde_json::Value = serde_json::from_str(&r.metadata)?;
    // work with JSON
}
```

### Date/Time

SQLite stores dates as TEXT or INTEGER. d1c generates `String` or `i64`—parse as needed:

```sql
-- name: GetRecentLogs :many
SELECT id, created_at FROM logs 
WHERE created_at > :since;  -- TEXT timestamp
```

```rust
let logs = get_recent_logs(&d1, "2024-01-01T00:00:00Z").await?;
for log in logs {
    let dt = chrono::DateTime::parse_from_rfc3339(&log.created_at)?;
    // work with parsed datetime
}
```

---

## Best Practices

**DO:**
- ✅ Use descriptive query names (`get_user_by_email`, not `query1`)
- ✅ Select specific columns instead of `SELECT *` when possible
- ✅ Use `-- params:` for custom types (newtype pattern)
- ✅ Use `-- instrument: skip()` for sensitive data
- ✅ Commit generated code to version control

**DON'T:**
- ❌ Use `SELECT *` if you only need a few columns (generates larger structs)
- ❌ Forget to regenerate after schema changes
- ❌ Put business logic in SQL (keep queries simple)
- ❌ Use positional parameters (`?1`) in your queries (use `:named` instead)

---

## Troubleshooting

### "Parameter type inference failed"

**Problem:** d1c couldn't determine the type for a parameter.

**Solution:** Add `-- params:` header:
```sql
-- name: MyQuery :one
-- params: user_id String, count i64
SELECT ...
```

### "Invalid cardinality"

**Problem:** You used an unsupported cardinality.

**Solution:** Must be one of: `:one`, `:many`, `:exec`, `:scalar` (note the colon).

### "Skip field 'xyz' not found in parameters"

**Problem:** `-- instrument: skip(xyz)` references a parameter that doesn't exist.

**Solution:** Check spelling and ensure the parameter is used in the query.

### "Expected single column for :scalar query"

**Problem:** Your `:scalar` query returns multiple columns.

**Solution:** Use `:one` instead, or modify query to select a single value:
```sql
-- Wrong
-- name: GetCount :scalar
SELECT id, COUNT(*) FROM users;

-- Right
-- name: GetCount :scalar
SELECT COUNT(*) FROM users;
```

---

## See Also

- [README.md](README.md) – Project overview and quick start
- [GETTING_STARTED.md](GETTING_STARTED.md) – Step-by-step tutorial
- [Observability](README.md#observability) – Tracing integration guide
