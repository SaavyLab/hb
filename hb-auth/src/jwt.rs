use std::collections::HashMap;
use std::sync::{OnceLock, RwLock};

use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use base64::Engine as _;
use js_sys::{Array, Date, Object, Reflect, Uint8Array};
use serde::{de::DeserializeOwned, Deserialize, Serialize};
use wasm_bindgen::JsCast;
use wasm_bindgen_futures::JsFuture;
use web_sys::{CryptoKey, SubtleCrypto};
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

static JWKS_CACHE: OnceLock<RwLock<HashMap<String, CachedKeys>>> = OnceLock::new();

fn cache() -> &'static RwLock<HashMap<String, CachedKeys>> {
    JWKS_CACHE.get_or_init(|| RwLock::new(HashMap::new()))
}

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
    verify_signature(header_b64, payload_b64, signature_b64, &jwk).await?;

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
    verify_signature(header_b64, payload_b64, signature_b64, &jwk).await?;

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

async fn verify_signature(
    header_b64: &str,
    payload_b64: &str,
    signature_b64: &str,
    jwk: &Jwk,
) -> WorkerResult<()> {
    let crypto = get_subtle_crypto()?;
    let crypto_key = import_jwk_as_crypto_key(&crypto, jwk).await?;

    let signing_input = format!("{header_b64}.{payload_b64}");
    let signature_bytes = decode_segment_raw(signature_b64)?;

    let algorithm = Object::new();
    Reflect::set(&algorithm, &"name".into(), &"RSASSA-PKCS1-v1_5".into())
        .map_err(|_| auth_error("failed to set algorithm"))?;

    let data = Uint8Array::from(signing_input.as_bytes());
    let signature = Uint8Array::from(signature_bytes.as_slice());

    let result = JsFuture::from(
        crypto
            .verify_with_object_and_buffer_source_and_buffer_source(
                &algorithm,
                &crypto_key,
                &signature,
                &data,
            )
            .map_err(|_| auth_error("verify call failed"))?,
    )
    .await
    .map_err(|_| auth_error("signature verification failed"))?;

    if !result.as_bool().unwrap_or(false) {
        return Err(auth_error("JWT signature verification failed"));
    }

    Ok(())
}

async fn import_jwk_as_crypto_key(crypto: &SubtleCrypto, jwk: &Jwk) -> WorkerResult<CryptoKey> {
    if jwk.kty != "RSA" {
        return Err(auth_error("unexpected JWK kty"));
    }

    let jwk_obj = Object::new();
    Reflect::set(&jwk_obj, &"kty".into(), &jwk.kty.as_str().into())
        .map_err(|_| auth_error("failed to set kty"))?;
    Reflect::set(&jwk_obj, &"n".into(), &jwk.n.as_str().into())
        .map_err(|_| auth_error("failed to set n"))?;
    Reflect::set(&jwk_obj, &"e".into(), &jwk.e.as_str().into())
        .map_err(|_| auth_error("failed to set e"))?;
    Reflect::set(&jwk_obj, &"alg".into(), &"RS256".into())
        .map_err(|_| auth_error("failed to set alg"))?;

    let algorithm = Object::new();
    Reflect::set(&algorithm, &"name".into(), &"RSASSA-PKCS1-v1_5".into())
        .map_err(|_| auth_error("failed to set algorithm name"))?;
    Reflect::set(&algorithm, &"hash".into(), &"SHA-256".into())
        .map_err(|_| auth_error("failed to set hash"))?;

    let key_usages = Array::new();
    key_usages.push(&"verify".into());

    let promise = crypto
        .import_key_with_object("jwk", &jwk_obj, &algorithm, false, &key_usages)
        .map_err(|_| auth_error("import_key call failed"))?;

    JsFuture::from(promise)
        .await
        .map_err(|_| auth_error("failed to import JWK"))?
        .dyn_into::<CryptoKey>()
        .map_err(|_| auth_error("failed to cast to CryptoKey"))
}

fn get_subtle_crypto() -> WorkerResult<SubtleCrypto> {
    let global = js_sys::global();
    let crypto =
        Reflect::get(&global, &"crypto".into()).map_err(|_| auth_error("crypto not available"))?;
    let subtle = Reflect::get(&crypto, &"subtle".into())
        .map_err(|_| auth_error("subtle crypto not available"))?;
    subtle
        .dyn_into::<SubtleCrypto>()
        .map_err(|_| auth_error("invalid SubtleCrypto"))
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
        let c = cache()
            .read()
            .map_err(|_| auth_error("failed to read JWKS cache"))?;
        if let Some(entry) = c.get(config.team_domain.as_ref()) {
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
        let mut c = cache()
            .write()
            .map_err(|_| auth_error("failed to write JWKS cache"))?;
        c.insert(
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
async fn load_jwks_cached(config: &AuthConfig, kv: &worker::kv::KvStore) -> WorkerResult<Vec<Jwk>> {
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
