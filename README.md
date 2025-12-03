# hb

**Tools for Cloudflare Workers.**

hb is a collection of Rust crates for building correct, high-performance systems on the Cloudflare edge. Each crate is independent, single-purpose, and designed to work natively with Workers primitives.

## Crates

### [hb-d1c](./hb-d1c/)

Type-safe SQL generator for Cloudflare D1. Write `.sql` files with named parameters, get compile-time checked Rust functions. Think sqlc for the edge.

```bash
cargo install hb-d1c
```

### [hb-auth](./hb-auth/)

Drop-in Cloudflare Access JWT validation with a strongly-typed permission DSL. Handles key rotation, signature verification, and role mapping.

```toml
[dependencies]
hb-auth = "0.1"
```

---

Built by [SaavyLab](https://saavylab.dev).
