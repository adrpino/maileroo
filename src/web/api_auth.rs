use crate::db::{User, get_user_by_api_key_hash};
use crate::web::AppState;
use crate::web::ax_extract_from_ref::FromRef;
use axum::{
    extract::FromRequestParts,
    http::{StatusCode, request::Parts},
};
use sha2::{Digest, Sha256};
use std::sync::Arc;

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

        if !auth_header.starts_with("Bearer ") {
            return Err((
                StatusCode::UNAUTHORIZED,
                "Invalid API key format".to_string(),
            ));
        }

        let token = &auth_header[7..];

        // 2. Hash the token
        let mut hasher = Sha256::new();
        hasher.update(token.as_bytes());
        let hash = format!("{:x}", hasher.finalize());

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
