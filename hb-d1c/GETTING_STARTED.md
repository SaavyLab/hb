# Getting Started with d1c

This guide will help you set up **d1c** in your Cloudflare Worker project.

## Prerequisites

1.  **Rust & Cargo**: You should have a working Rust environment.
2.  **Wrangler**: Cloudflare's CLI tool (`npm install -g wrangler`).
3.  **A D1 Database**: Created via `wrangler d1 create <name>`.

---

## 1. Installation

Install the `d1c` CLI tool:

```bash
cargo install d1c
```

---

## 2. Setup

Navigate to your Worker's crate directory (where `wrangler.toml` is) and run:

```bash
d1c init
```

This interactive command will:
1.  Detect your D1 database configuration in `wrangler.toml`.
2.  Ask where your migrations and queries are located.
3.  Create a `d1c.toml` configuration file.
4.  Generate a sample query file (`example.sql`) if your query directory is empty.

---

## 3. The Workflow

d1c follows a simple cycle: **Migrate → Query → Generate**.

### Step 3a: Write Migrations
Create your D1 migrations as usual. d1c uses these to understand your schema.

```sql
-- migrations/0001_init.sql
CREATE TABLE users (
  id TEXT PRIMARY KEY,
  email TEXT NOT NULL,
  active BOOLEAN NOT NULL DEFAULT FALSE
);
```

### Step 3b: Write Queries
Create `.sql` files in your queries directory (e.g., `db/queries/users.sql`). Use the `-- name:` header to define the function name and return type.

```sql
-- name: GetUser :one
SELECT id, email, active FROM users WHERE id = :id;

-- name: CreateUser :one
INSERT INTO users (id, email) VALUES (:id, :email) RETURNING *;
```

**Supported Cardinalities:**
- `:one`    -> Returns `Result<Option<Row>>`
- `:many`   -> Returns `Result<Vec<Row>>`
- `:exec`   -> Returns `Result<()>` (no return value)
- `:scalar` -> Returns `Result<Option<T>>` (single primitive value)

**Batch Operations:**
Append `:stmt` to the header (e.g. `-- name: CreateUser :exec :stmt`) to generate a `{name}_stmt` function that returns a prepared statement for use with `d1.batch()`.

### Step 3c: Generate Code
Run the generator:

```bash
d1c generate
```

This creates a Rust file (default: `src/d1c/queries.rs`) containing type-safe functions for your queries.

> **Pro Tip**: Run `d1c watch` in a separate terminal to automatically regenerate code whenever you save a `.sql` file.

---

## 4. Using the Generated Code

Import the module in your Worker and use the functions. They are `async` and take the `D1Database` as the first argument.

```rust
// src/lib.rs or src/main.rs
mod d1c; // matching the directory you generated into
use crate::d1c::queries::*; // import generated functions

#[worker::event(fetch)]
async fn fetch(req: Request, env: Env, _ctx: Context) -> Result<Response> {
    let d1 = env.d1("DB")?;
    
    // 1. Fetch a user (Type-safe!)
    let user = get_user(&d1, "user_123").await?;
    
    if let Some(u) = user {
        console_log!("Found user: {}", u.email);
    }

    // 2. Create a user (Compiler enforces correct arguments!)
    create_user(&d1, "user_456", "new@example.com").await?;

    Response::ok("Done")
}
```

---

## Next Steps

Now that you have d1c set up, check out:

- **[QUERY_FORMAT.md](QUERY_FORMAT.md)** – Complete reference for query syntax, cardinalities, and headers
- **[README.md#observability](README.md#observability)** – Enable automatic tracing for database queries
- **`d1c watch`** – Run in a separate terminal to auto-regenerate on file changes

### Tips for Success

1. **Commit generated code** – Treat `src/d1c/queries.rs` like any other source file
2. **Use `d1c watch`** – Faster workflow during development
3. **Name queries clearly** – `get_user_by_email` beats `user_query_1`
4. **Review the QUERY_FORMAT** – Learn about `:one`, `:many`, `:exec`, `:scalar` and when to use each

Happy querying! 🚀
