# hb-auth

**Identity and permissions for Cloudflare Workers.**

`hb-auth` provides drop-in Cloudflare Access JWT validation with a strongly-typed permission DSL. It handles key rotation, signature verification, and identity extraction so you can focus on your business logic.

Part of the [**hb** stack](../).

---

## Features

- ✅ **Zero-Config Validation** – Automatically fetches and caches JWKS from your Cloudflare Access team domain.
- ✅ **Type-Safe Identity** – Extract a strongly-typed `User` struct from requests.
- ✅ **Role-Based Access** – Map Access Groups (e.g., "super-admins") to internal Rust enums automatically.
- ✅ **Framework Agnostic** – First-class support for `axum` (via extractors) and raw `worker::Request`.
- ✅ **Optional KV Caching** – Opt-in cross-isolate JWKS caching via Cloudflare KV.

---

## Installation

```toml
[dependencies]
hb-auth = { version = "0.1", features = ["axum"] }
```

### Optional: KV-backed JWKS Caching

By default, `hb-auth` caches JWKS keys in memory. Since Workers isolates are ephemeral, this cache is lost on restart. Enable the `kv` feature for cross-isolate caching:

```toml
[dependencies]
hb-auth = { version = "0.1", features = ["axum", "kv"] }
```

This stores JWKS keys in Cloudflare KV with a 10-minute TTL, so all isolates share the same cached keys.

---

## Quick Start (Axum)

### 1. Configure

In your router setup, initialize the config and add it to your state. Your state must implement `HasAuthConfig`.

```rust
use hb_auth::{AuthConfig, HasAuthConfig};

#[derive(Clone)]
struct AppState {
    auth_config: AuthConfig,
    // ... other state
}

impl HasAuthConfig for AppState {
    fn auth_config(&self) -> &AuthConfig {
        &self.auth_config
    }
}

fn router(env: Env) -> Router {
    let auth_config = AuthConfig::new(
        "https://my-team.cloudflareaccess.com",
        env.var("ACCESS_AUD").unwrap().to_string()
    );
    
    let state = AppState { auth_config };
    
    Router::new()
        .route("/secure", get(handler))
        .with_state(state)
}
```

### 2. Protect Routes

Add `auth: User` to your handler arguments. The handler will only run if a valid Cloudflare Access JWT is present.

```rust
use hb_auth::User;

async fn handler(auth: User) -> &'static str {
    format!("Hello, {}!", auth.email())
}
```

---

## Setting Up KV Caching

If you enabled the `kv` feature, you need to:

### 1. Add a KV Namespace

In your `wrangler.toml`:

```toml
[[kv_namespaces]]
binding = "AUTH_CACHE"
id = "<your-kv-namespace-id>"
```

### 2. Implement `HasJwksCache`

Your state must implement `HasJwksCache` to provide the KV binding:

```rust
use hb_auth::{AuthConfig, HasAuthConfig, HasJwksCache};

#[derive(Clone)]
struct AppState {
    env: SendWrapper<Env>,
    auth_config: AuthConfig,
}

impl HasAuthConfig for AppState {
    fn auth_config(&self) -> &AuthConfig {
        &self.auth_config
    }
}

impl HasJwksCache for AppState {
    fn jwks_kv(&self) -> Option<worker::kv::KvStore> {
        self.env.kv("AUTH_CACHE").ok()
    }
}
```

The extractor automatically uses KV when available, falling back to in-memory caching if `jwks_kv()` returns `None`.

---

## Advanced: Role-Based Access

You can map Cloudflare Access Groups (available in the JWT `groups` claim) to your own internal roles.

### 1. Define Roles

```rust
use hb_auth::{RoleMapper, Claims};

#[derive(Debug, PartialEq, Clone)]
pub enum Role {
    Admin,
    Editor,
    Viewer,
}

impl RoleMapper for Role {
    fn from_claims(claims: &Claims) -> Vec<Self> {
        let mut roles = vec![];
        
        // Map groups from Cloudflare Access
        for group in &claims.groups {
            match group.as_str() {
                "000-my-app-admins" => roles.push(Role::Admin),
                "000-my-app-editors" => roles.push(Role::Editor),
                _ => {}
            }
        }
        
        // Or map specific emails
        if claims.email.ends_with("@saavylab.com") {
            roles.push(Role::Viewer);
        }
        
        roles
    }
}
```

### 2. Enforce Permissions

Use `User<Role>` instead of the default `User`.

```rust
async fn delete_db(auth: User<Role>) -> Result<impl IntoResponse, StatusCode> {
    if !auth.has_role(Role::Admin) {
        return Err(StatusCode::FORBIDDEN);
    }
    
    // ... unsafe logic
    Ok("Deleted")
}
```

---

## Usage with Raw Workers

If you aren't using Axum, you can still use `hb-auth` to validate requests.

```rust
use hb_auth::User;

async fn fetch(req: Request, env: Env, _ctx: Context) -> Result<Response> {
    let config = AuthConfig::new(/* ... */);
    
    let user = match User::<()>::from_worker_request(&req, &config).await {
        Ok(u) => u,
        Err(e) => return Response::error(e, 401),
    };
    
    Response::ok(format!("Welcome {}", user.email()))
}
```

With KV caching enabled:

```rust
use hb_auth::User;

async fn fetch(req: Request, env: Env, _ctx: Context) -> Result<Response> {
    let config = AuthConfig::new(/* ... */);
    let kv = env.kv("AUTH_CACHE")?;
    
    let user = match User::<()>::from_worker_request_cached(&req, &config, &kv).await {
        Ok(u) => u,
        Err(e) => return Response::error(e, 401),
    };
    
    Response::ok(format!("Welcome {}", user.email()))
}
```

---

## Configuring Cloudflare Access

To get the `groups` claim in your JWT:

1. Go to **Zero Trust Dashboard** > **Access** > **Applications**.
2. Edit your application.
3. Under **Settings** (or "Overview" -> "Edit"), find **OIDC Claims** or **Additional Settings**.
4. Enable **Groups** (this might require adding "groups" to the scope depending on your configuration).
5. Ensure the groups you want to map are assigned to the application policy.

The `audience` (AUD) is found in the **Overview** tab of your Access Application.

---

## License

MIT
