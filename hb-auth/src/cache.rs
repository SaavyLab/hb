use serde::{Deserialize, Serialize};

#[derive(Clone, Serialize, Deserialize)]
pub struct CachedJwks {
    pub keys: Vec<CachedJwk>,
    pub fetched_at_ms: f64,
}

#[derive(Clone, Serialize, Deserialize)]
pub struct CachedJwk {
    pub kty: String,
    pub kid: String,
    pub n: String,
    pub e: String,
}

impl CachedJwks {
    pub fn is_expired(&self, now_ms: f64, ttl_ms: f64) -> bool {
        now_ms - self.fetched_at_ms > ttl_ms
    }
}

#[cfg(feature = "kv")]
mod kv_cache {
    use super::{CachedJwk, CachedJwks};
    use js_sys::Date;
    use worker::kv::KvStore;

    const JWKS_CACHE_TTL_MS: f64 = 10.0 * 60.0 * 1000.0; // 10 minutes
    const KV_TTL_SECONDS: u64 = 15 * 60; // 15 minutes (slightly longer than cache TTL)

    fn cache_key(team_domain: &str) -> String {
        format!("jwks:{}", team_domain)
    }

    pub async fn get_cached_jwks(kv: &KvStore, team_domain: &str) -> Option<CachedJwks> {
        let key = cache_key(team_domain);

        match kv.get(&key).json::<CachedJwks>().await {
            Ok(Some(cached)) => {
                if cached.is_expired(Date::now(), JWKS_CACHE_TTL_MS) {
                    None
                } else {
                    Some(cached)
                }
            }
            Ok(None) => None,
            Err(_) => None,
        }
    }

    pub async fn set_cached_jwks(
        kv: &KvStore,
        team_domain: &str,
        keys: Vec<CachedJwk>,
    ) -> Result<(), worker::Error> {
        let key = cache_key(team_domain);
        let cached = CachedJwks {
            keys,
            fetched_at_ms: Date::now(),
        };

        kv.put(&key, &cached)
            .map_err(|e| worker::Error::RustError(format!("KV put error: {e}")))?
            .expiration_ttl(KV_TTL_SECONDS)
            .execute()
            .await
            .map_err(|e| worker::Error::RustError(format!("KV put error: {e}")))
    }
}

#[cfg(feature = "kv")]
pub use kv_cache::{get_cached_jwks, set_cached_jwks};
