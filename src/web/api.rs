use crate::db::{get_email_by_user_id, get_email_count_by_user_id, update_alias_auto_forward};
use crate::web::AppState;
use crate::web::api_auth::ApiUser;
use crate::web::handlers::{PaginationParams, ToggleAutoForwardRequest};
use crate::web::replies::{ReplyRequest, process_reply};
use axum::{
    Json,
    extract::{Path, Query, State},
    http::StatusCode,
    response::IntoResponse,
};
use serde_json::json;
use std::sync::Arc;
use uuid::Uuid;

pub async fn list_emails_handler(
    State(state): State<Arc<AppState>>,
    user: ApiUser,
    Query(pagination): Query<PaginationParams>,
) -> impl IntoResponse {
    let page = pagination.page.unwrap_or(1).max(1);
    let page_size = 10;
    let offset = (page - 1) * page_size;
    let alias_filter = pagination.alias.filter(|s| !s.is_empty());

    let emails = match get_email_by_user_id(
        &state.db,
        user.user.id,
        page_size,
        offset,
        alias_filter.clone(),
        pagination.q.clone(),
    )
    .await
    {
        Ok(emails) => emails,
        Err(_) => return (StatusCode::INTERNAL_SERVER_ERROR, "Database error").into_response(),
    };

    let total_emails = match get_email_count_by_user_id(
        &state.db,
        user.user.id,
        alias_filter,
        pagination.q,
    )
    .await {
        Ok(count) => count,
        Err(_) => 0,
    };

    let total_pages = (total_emails as f64 / page_size as f64).ceil() as i64;

    Json(json!({
        "emails": emails,
        "total": total_emails,
        "page": page,
        "total_pages": total_pages,
    }))
    .into_response()
}

pub async fn toggle_alias_forward_api(
    State(state): State<Arc<AppState>>,
    user: ApiUser,
    Path(alias_id): Path<Uuid>,
    Json(payload): Json<ToggleAutoForwardRequest>,
) -> impl IntoResponse {
    match update_alias_auto_forward(&state.db, alias_id, user.user.id, payload.auto_forward).await {
        Ok(_) => Json(json!({
            "status": "success",
            "alias_id": alias_id,
            "auto_forward": payload.auto_forward
        }))
        .into_response(),
        Err(e) => {
            tracing::error!("Error toggling alias forward via API: {}", e);
            (StatusCode::INTERNAL_SERVER_ERROR, "Database error").into_response()
        }
    }
}

pub async fn submit_reply_api(
    State(state): State<Arc<AppState>>,
    user: ApiUser,
    Path(email_id): Path<Uuid>,
    Json(payload): Json<ReplyRequest>,
) -> impl IntoResponse {
    match process_reply(&state, user.user.id, email_id, &payload.body_text).await {
        Ok(reply) => Json(json!({
            "status": "success",
            "reply": reply
        }))
        .into_response(),
        Err((status, msg)) => (status, msg).into_response(),
    }
}
