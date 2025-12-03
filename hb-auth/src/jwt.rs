use std::{collections::HashMap, sync::RwLock};

use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use base64::Engine as _;
use js_sys::Date;
use once_cell::sync::Lazy;
use rsa::{
    pkcs1v15::{Signature, VerifyingKey},
    signature::Verifier,
    RsaPublicKey,
};
use serde::{de::DeserializeOwned, Deserialize, Serialize};
use sha2::Sha256;
use worker::{Error, Fetch, Method, Request as WorkerRequest};

#[cfg(feature = "kv")]
use crate::cache::{get_cached_jwks, set_cached_jwks, CachedJwk};
use crate::config::AuthConfig;

type WorkerResult<T> = worker::Result<T>;

const JWKS_CACHE_TTL_MS: f64 = 10.0 * 60.0 * 1000.0; // 10 minutes

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Claims {
    pub aud: Vec<String>,
    pub email: String,
    pub exp: i64,
    pub iss: String,
    pub sub: String,
    pub name: Option<String>,
    #[serde(default)]
    pub groups: Vec<String>,
}

#[derive(Clone, Deserialize)]
struct Jwks {
    keys: Vec<Jwk>,
}

#[derive(Clone, Deserialize)]
struct Jwk {
    kty: String,
    kid: String,
    n: String,
    e: String,
}

#[derive(Deserialize)]
struct JwtHeader {
    alg: String,
    kid: String,
}

#[derive(Clone)]
struct CachedKeys {
    fetched_at_ms: f64,
    keys: Vec<Jwk>,
}

static JWKS_CACHE: Lazy<RwLock<HashMap<String, CachedKeys>>> =
    Lazy::new(|| RwLock::new(HashMap::new()));

#[worker::send]
pub async fn verify_access_jwt(token: &str, config: &AuthConfig) -> WorkerResult<Claims> {
    let token = token.trim();
    let token = token.strip_prefix("Bearer ").unwrap_or(token);
    let (header_b64, payload_b64, signature_b64) = split_jwt(token)?;

    let header: JwtHeader = decode_segment(header_b64)?;
    if header.alg != "RS256" {
        return Err(auth_error("unsupported JWT algorithm"));
    }

    let jwk = find_jwk(config, &header.kid).await?;
    verify_signature(header_b64, payload_b64, signature_b64, &jwk)?;

    let claims: Claims = decode_segment(payload_b64)?;
    validate_claims(&claims, config)?;
    Ok(claims)
}

#[cfg(feature = "kv")]
#[worker::send]
pub async fn verify_access_jwt_cached(
    token: &str,
    config: &AuthConfig,
    kv: &worker::kv::KvStore,
) -> WorkerResult<Claims> {
    let token = token.trim();
    let token = token.strip_prefix("Bearer ").unwrap_or(token);
    let (header_b64, payload_b64, signature_b64) = split_jwt(token)?;

    let header: JwtHeader = decode_segment(header_b64)?;
    if header.alg != "RS256" {
        return Err(auth_error("unsupported JWT algorithm"));
    }

    let jwk = find_jwk_cached(config, &header.kid, kv).await?;
    verify_signature(header_b64, payload_b64, signature_b64, &jwk)?;

    let claims: Claims = decode_segment(payload_b64)?;
    validate_claims(&claims, config)?;
    Ok(claims)
}

fn validate_claims(claims: &Claims, config: &AuthConfig) -> WorkerResult<()> {
    let aud_match = claims.aud.iter().any(|aud| aud == &*config.audience);
    if !aud_match {
        return Err(auth_error("audience mismatch"));
    }

    if claims.iss != config.issuer() {
        return Err(auth_error("issuer mismatch"));
    }

    let now = Date::now() / 1000.0;
    if (claims.exp as f64) <= now {
        return Err(auth_error("token expired"));
    }

    Ok(())
}

fn verify_signature(
    header_b64: &str,
    payload_b64: &str,
    signature_b64: &str,
    jwk: &Jwk,
) -> WorkerResult<()> {
    let signing_input = format!("{header_b64}.{payload_b64}");
    let signature_bytes = decode_segment_raw(signature_b64)?;
    let signature = Signature::try_from(signature_bytes.as_slice())
        .map_err(|_| auth_error("invalid signature bytes"))?;

    let key = jwk_to_rsa(jwk)?;
    let verifying_key = VerifyingKey::<Sha256>::new(key);
    verifying_key
        .verify(signing_input.as_bytes(), &signature)
        .map_err(|_| auth_error("JWT signature verification failed"))?;
    Ok(())
}

#[worker::send]
async fn find_jwk(config: &AuthConfig, kid: &str) -> WorkerResult<Jwk> {
    let keys = load_jwks(config).await?;
    keys.into_iter()
        .find(|key| key.kid == kid)
        .ok_or_else(|| auth_error("kid not found in JWKS"))
}

#[worker::send]
async fn load_jwks(config: &AuthConfig) -> WorkerResult<Vec<Jwk>> {
    {
        let cache = JWKS_CACHE
            .read()
            .map_err(|_| auth_error("failed to read JWKS cache"))?;
        if let Some(entry) = cache.get(config.team_domain.as_ref()) {
            if Date::now() - entry.fetched_at_ms <= JWKS_CACHE_TTL_MS {
                return Ok(entry.keys.clone());
            }
        }
    }

    let url = format!("{}/cdn-cgi/access/certs", config.team_domain.as_ref());
    let request = WorkerRequest::new(&url, Method::Get)?;
    let mut resp = Fetch::Request(request).send().await?;
    let status = resp.status_code();
    if !(200..=299).contains(&status) {
        return Err(auth_error(format!(
            "unable to fetch Access JWKS (status {status})"
        )));
    }
    let body = resp.text().await?;
    let jwks: Jwks =
        serde_json::from_str(&body).map_err(|err| auth_error(format!("invalid JWKS: {err}")))?;

    {
        let mut cache = JWKS_CACHE
            .write()
            .map_err(|_| auth_error("failed to write JWKS cache"))?;
        cache.insert(
            config.team_domain.as_ref().clone(),
            CachedKeys {
                fetched_at_ms: Date::now(),
                keys: jwks.keys.clone(),
            },
        );
    }

    Ok(jwks.keys)
}

#[cfg(feature = "kv")]
#[worker::send]
async fn find_jwk_cached(
    config: &AuthConfig,
    kid: &str,
    kv: &worker::kv::KvStore,
) -> WorkerResult<Jwk> {
    let keys = load_jwks_cached(config, kv).await?;
    keys.into_iter()
        .find(|key| key.kid == kid)
        .ok_or_else(|| auth_error("kid not found in JWKS"))
}

#[cfg(feature = "kv")]
#[worker::send]
async fn load_jwks_cached(
    config: &AuthConfig,
    kv: &worker::kv::KvStore,
) -> WorkerResult<Vec<Jwk>> {
    // Try KV cache first
    if let Some(cached) = get_cached_jwks(kv, config.team_domain.as_ref()).await {
        return Ok(cached
            .keys
            .into_iter()
            .map(|k| Jwk {
                kty: k.kty,
                kid: k.kid,
                n: k.n,
                e: k.e,
            })
            .collect());
    }

    // Fetch from Access
    let url = format!("{}/cdn-cgi/access/certs", config.team_domain.as_ref());
    let request = WorkerRequest::new(&url, Method::Get)?;
    let mut resp = Fetch::Request(request).send().await?;
    let status = resp.status_code();
    if !(200..=299).contains(&status) {
        return Err(auth_error(format!(
            "unable to fetch Access JWKS (status {status})"
        )));
    }
    let body = resp.text().await?;
    let jwks: Jwks =
        serde_json::from_str(&body).map_err(|err| auth_error(format!("invalid JWKS: {err}")))?;

    // Store in KV
    let cached_keys: Vec<CachedJwk> = jwks
        .keys
        .iter()
        .map(|k| CachedJwk {
            kty: k.kty.clone(),
            kid: k.kid.clone(),
            n: k.n.clone(),
            e: k.e.clone(),
        })
        .collect();

    if let Err(e) = set_cached_jwks(kv, config.team_domain.as_ref(), cached_keys).await {
        tracing::warn!("Failed to cache JWKS in KV: {e:?}");
    }

    Ok(jwks.keys)
}

fn jwk_to_rsa(jwk: &Jwk) -> WorkerResult<RsaPublicKey> {
    if jwk.kty != "RSA" {
        return Err(auth_error("unexpected JWK kty"));
    }

    let modulus = decode_segment_raw(&jwk.n)?;
    let exponent = decode_segment_raw(&jwk.e)?;

    let n = rsa::BigUint::from_bytes_be(&modulus);
    let e = rsa::BigUint::from_bytes_be(&exponent);
    RsaPublicKey::new(n, e).map_err(|err| auth_error(format!("invalid JWK: {err}")))
}

fn split_jwt(token: &str) -> WorkerResult<(&str, &str, &str)> {
    let mut segments = token.split('.');
    match (
        segments.next(),
        segments.next(),
        segments.next(),
        segments.next(),
    ) {
        (Some(h), Some(p), Some(s), None) => Ok((h, p, s)),
        _ => Err(auth_error("malformed JWT")),
    }
}

fn decode_segment<T>(segment: &str) -> WorkerResult<T>
where
    T: DeserializeOwned,
{
    let bytes = decode_segment_raw(segment)?;
    serde_json::from_slice(&bytes).map_err(|err| auth_error(format!("invalid JSON: {err}")))
}

fn decode_segment_raw(segment: &str) -> WorkerResult<Vec<u8>> {
    URL_SAFE_NO_PAD
        .decode(segment.as_bytes())
        .map_err(|_| auth_error("invalid base64 segment"))
}

fn auth_error<T: Into<String>>(message: T) -> Error {
    Error::RustError(format!("auth: {}", message.into()))
}
