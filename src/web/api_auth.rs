use crate::db::{User, get_user_by_api_key_hash};
use crate::web::AppState;
use crate::web::ax_extract_from_ref::FromRef;
use axum::{
    extract::FromRequestParts,
    http::{StatusCode, request::Parts},
};
use rand::{Rng, distributions::Alphanumeric, thread_rng};
use sha2::{Digest, Sha256};
use std::sync::Arc;

pub const API_KEY_LEN: usize = 32;

/// Generates a random 32-character alphanumeric API key token.
pub fn generate_api_key() -> String {
    thread_rng()
        .sample_iter(&Alphanumeric)
        .take(API_KEY_LEN)
        .map(char::from)
        .collect()
}

/// Pure function that computes the lowercase hex SHA-256 digest of an API key token.
pub fn hash_api_key(token: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(token.as_bytes());
    format!("{:x}", hasher.finalize())
}

/// Pure function that extracts the raw API key token from a `Bearer <token>` header.
pub fn extract_bearer_token(auth_header: &str) -> Option<&str> {
    auth_header
        .strip_prefix("Bearer ")
        .filter(|token| !token.is_empty())
}

pub struct ApiUser {
    pub user: User,
}

impl<S> FromRequestParts<S> for ApiUser
where
    Arc<AppState>: FromRef<S>,
    S: Send + Sync,
{
    type Rejection = (StatusCode, String);

    async fn from_request_parts(parts: &mut Parts, state: &S) -> Result<Self, Self::Rejection> {
        // 1. Get the Authorization header
        let auth_header = parts
            .headers
            .get(axum::http::header::AUTHORIZATION)
            .and_then(|h| h.to_str().ok())
            .ok_or((StatusCode::UNAUTHORIZED, "Missing API key".to_string()))?;

        let token = extract_bearer_token(auth_header).ok_or((
            StatusCode::UNAUTHORIZED,
            "Invalid API key format".to_string(),
        ))?;

        // 2. Hash the token
        let hash = hash_api_key(token);

        // 3. Get AppState to access DB
        let app_state = Arc::<AppState>::from_ref(state);

        // 4. Look up user by hash
        let user = get_user_by_api_key_hash(&app_state.db, &hash)
            .await
            .map_err(|e| {
                tracing::error!("DB error verifying API key: {}", e);
                (StatusCode::INTERNAL_SERVER_ERROR, "DB error".to_string())
            })?
            .ok_or((StatusCode::UNAUTHORIZED, "Invalid API key".to_string()))?;

        Ok(ApiUser { user })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_generate_api_key_properties() {
        let k1 = generate_api_key();
        let k2 = generate_api_key();

        assert_eq!(k1.len(), API_KEY_LEN);
        assert_eq!(k2.len(), API_KEY_LEN);
        assert!(k1.chars().all(|c| c.is_ascii_alphanumeric()));
        assert_ne!(k1, k2);
    }

    #[test]
    fn test_hash_api_key_determinism_and_format() {
        let h1 = hash_api_key("test-api-key-123");
        let h2 = hash_api_key("test-api-key-123");
        let h3 = hash_api_key("test-api-key-124");

        assert_eq!(h1, h2);
        assert_ne!(h1, h3);
        assert_eq!(h1.len(), 64);
        assert!(
            h1.chars()
                .all(|c| c.is_ascii_hexdigit() && !c.is_uppercase())
        );
    }

    #[test]
    fn test_extract_bearer_token() {
        assert_eq!(extract_bearer_token("Bearer abc123"), Some("abc123"));
        assert_eq!(extract_bearer_token("Bearer "), None);
        assert_eq!(extract_bearer_token("Basic abc123"), None);
        assert_eq!(extract_bearer_token("abc123"), None);
    }
}
