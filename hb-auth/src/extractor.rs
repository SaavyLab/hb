#[cfg(feature = "axum")]
use axum::{
    extract::FromRequestParts,
    http::{header::COOKIE, request::Parts, StatusCode},
};
use tracing::error;

use crate::{
    config::AuthConfig,
    jwt::{verify_access_jwt, Claims},
};

#[cfg(feature = "kv")]
use crate::jwt::verify_access_jwt_cached;

pub trait HasAuthConfig {
    fn auth_config(&self) -> &AuthConfig;
}

#[cfg(feature = "kv")]
pub trait HasJwksCache {
    fn jwks_kv(&self) -> Option<worker::kv::KvStore>;
}

pub trait RoleMapper: Sized + Send + Sync + 'static {
    fn from_claims(claims: &Claims) -> Vec<Self>;
}

impl RoleMapper for () {
    fn from_claims(_: &Claims) -> Vec<Self> {
        vec![]
    }
}

#[derive(Debug, Clone)]
pub struct User<R: RoleMapper = ()> {
    pub claims: Claims,
    pub roles: Vec<R>,
    pub token: String,
}

impl<R: RoleMapper> User<R> {
    pub fn has_role(&self, role: R) -> bool
    where
        R: PartialEq,
    {
        self.roles.contains(&role)
    }

    // Helper accessors
    pub fn email(&self) -> &str {
        &self.claims.email
    }

    pub fn sub(&self) -> &str {
        &self.claims.sub
    }

    pub async fn from_worker_request(
        req: &worker::Request,
        config: &AuthConfig,
    ) -> Result<Self, String> {
        let token = extract_token_worker(req)
            .or_else(|| extract_token_from_cookies_worker(req))
            .ok_or_else(|| "missing access token".to_string())?;

        let claims = verify_access_jwt(&token, config).await.map_err(|err| {
            error!("JWT verification failed: {err:?}");
            "invalid or expired token".to_string()
        })?;

        let roles = R::from_claims(&claims);

        Ok(User {
            claims,
            roles,
            token,
        })
    }

    #[cfg(feature = "kv")]
    pub async fn from_worker_request_cached(
        req: &worker::Request,
        config: &AuthConfig,
        kv: &worker::kv::KvStore,
    ) -> Result<Self, String> {
        let token = extract_token_worker(req)
            .or_else(|| extract_token_from_cookies_worker(req))
            .ok_or_else(|| "missing access token".to_string())?;

        let claims = verify_access_jwt_cached(&token, config, kv)
            .await
            .map_err(|err| {
                error!("JWT verification failed: {err:?}");
                "invalid or expired token".to_string()
            })?;

        let roles = R::from_claims(&claims);

        Ok(User {
            claims,
            roles,
            token,
        })
    }
}

#[cfg(all(feature = "axum", not(feature = "kv")))]
impl<S, R> FromRequestParts<S> for User<R>
where
    S: HasAuthConfig + Send + Sync,
    R: RoleMapper,
{
    type Rejection = (StatusCode, String);

    #[worker::send]
    async fn from_request_parts(parts: &mut Parts, state: &S) -> Result<Self, Self::Rejection> {
        let token = extract_token(&parts.headers)
            .or_else(|| extract_token_from_cookies(&parts.headers))
            .ok_or((StatusCode::UNAUTHORIZED, "missing access token".to_string()))?;

        let config = state.auth_config();

        let claims = verify_access_jwt(&token, config).await.map_err(|err| {
            error!("JWT verification failed: {err:?}");
            (
                StatusCode::UNAUTHORIZED,
                "invalid or expired token".to_string(),
            )
        })?;

        let roles = R::from_claims(&claims);

        Ok(User {
            claims,
            roles,
            token,
        })
    }
}

#[cfg(all(feature = "axum", feature = "kv"))]
impl<S, R> FromRequestParts<S> for User<R>
where
    S: HasAuthConfig + HasJwksCache + Send + Sync,
    R: RoleMapper,
{
    type Rejection = (StatusCode, String);

    #[worker::send]
    async fn from_request_parts(parts: &mut Parts, state: &S) -> Result<Self, Self::Rejection> {
        let token = extract_token(&parts.headers)
            .or_else(|| extract_token_from_cookies(&parts.headers))
            .ok_or((StatusCode::UNAUTHORIZED, "missing access token".to_string()))?;

        let config = state.auth_config();

        let claims = if let Some(ref kv) = state.jwks_kv() {
            verify_access_jwt_cached(&token, config, kv).await
        } else {
            verify_access_jwt(&token, config).await
        }
        .map_err(|err| {
            error!("JWT verification failed: {err:?}");
            (
                StatusCode::UNAUTHORIZED,
                "invalid or expired token".to_string(),
            )
        })?;

        let roles = R::from_claims(&claims);

        Ok(User {
            claims,
            roles,
            token,
        })
    }
}

#[cfg(feature = "axum")]
fn extract_token(headers: &axum::http::HeaderMap) -> Option<String> {
    headers
        .get("CF_Authorization")
        .or_else(|| headers.get("Cf-Access-Jwt-Assertion"))
        .and_then(|value| value.to_str().ok())
        .map(|s| s.to_string())
}

#[cfg(feature = "axum")]
fn extract_token_from_cookies(headers: &axum::http::HeaderMap) -> Option<String> {
    headers
        .get(COOKIE)
        .and_then(|value| value.to_str().ok())
        .and_then(|cookie_header| {
            cookie_header
                .split(';')
                .map(|kv| kv.trim())
                .find_map(|pair| {
                    let mut parts = pair.splitn(2, '=');
                    match (parts.next(), parts.next()) {
                        (Some("CF_Authorization"), Some(token)) => Some(token.to_string()),
                        _ => None,
                    }
                })
        })
}

fn extract_token_worker(req: &worker::Request) -> Option<String> {
    let headers = req.headers();
    headers
        .get("CF_Authorization")
        .ok()
        .flatten()
        .or_else(|| headers.get("Cf-Access-Jwt-Assertion").ok().flatten())
}

fn extract_token_from_cookies_worker(req: &worker::Request) -> Option<String> {
    req.headers()
        .get("Cookie")
        .ok()
        .flatten()
        .and_then(|cookie_header| {
            cookie_header
                .split(';')
                .map(|kv| kv.trim())
                .find_map(|pair| {
                    let mut parts = pair.splitn(2, '=');
                    match (parts.next(), parts.next()) {
                        (Some("CF_Authorization"), Some(token)) => Some(token.to_string()),
                        _ => None,
                    }
                })
        })
}
