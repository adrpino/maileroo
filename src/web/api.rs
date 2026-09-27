use crate::db::{
    AnyEmail, get_aliases_by_user_id, get_any_email, get_email_by_user_id,
    get_email_count_by_user_id, update_alias_auto_forward,
};
use crate::web::AppState;
use crate::web::api_auth::ApiUser;
use crate::web::attachments_form::{MAX_ATTACHMENTS, MAX_TOTAL_ATTACHMENT_BYTES};
use crate::web::handlers::{PaginationParams, ToggleAutoForwardRequest};
use crate::web::replies::{ReplyOutcome, ReplyRequest, process_reply};
use crate::web::send_email::{SendOutcome, send_firsthand_email};
use axum::{
    Json,
    extract::{Path, Query, State},
    http::StatusCode,
    response::IntoResponse,
};
use base64::Engine as _;
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
        None,
    )
    .await
    {
        Ok(emails) => emails,
        Err(_) => return (StatusCode::INTERNAL_SERVER_ERROR, "Database error").into_response(),
    };

    let total_emails =
        match get_email_count_by_user_id(&state.db, user.user.id, alias_filter, pagination.q, None)
            .await
        {
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
    Json(mut payload): Json<ReplyRequest>,
) -> impl IntoResponse {
    if payload.body_text.trim().is_empty() {
        return (
            StatusCode::BAD_REQUEST,
            "Reply body cannot be empty.".to_string(),
        )
            .into_response();
    }

    let attachments = match decode_api_attachments(payload.attachments.take()) {
        Ok(a) => a,
        Err(e) => return e.into_response(),
    };

    match process_reply(
        &state,
        user.user.id,
        email_id,
        &payload.body_text,
        attachments,
        !payload.dry_run.unwrap_or(false),
    )
    .await
    {
        Ok(ReplyOutcome::Sent(reply, reply_attachments)) => Json(json!({
            "status": "sent",
            "reply": reply,
            "attachments": reply_attachments,
        }))
        .into_response(),
        Ok(ReplyOutcome::DryRun(dry)) => Json(json!({
            "status": "dry_run",
            "resolved_from": dry.resolved_from,
            "to": dry.to,
            "subject": dry.subject,
            "in_reply_to": dry.in_reply_to,
            "message_id": dry.message_id,
            "attachment_count": dry.attachment_count,
            "mime_size": dry.mime_size,
        }))
        .into_response(),
        Err((status, msg)) => (status, msg).into_response(),
    }
}

/// Lists the aliases of the API user with their full addresses, so clients
/// can pick a valid `from_alias` for the send endpoint.
pub async fn list_aliases_api(
    State(state): State<Arc<AppState>>,
    user: ApiUser,
) -> impl IntoResponse {
    match get_aliases_by_user_id(&state.db, user.user.id).await {
        Ok(aliases) => {
            let aliases: Vec<serde_json::Value> = aliases
                .iter()
                .map(|a| {
                    json!({
                        "id": a.id,
                        "address": format!("{}@{}", a.subdomain, a.domain_name),
                        "active": a.active,
                        "auto_forward": a.auto_forward,
                        "created_at": a.created_at,
                    })
                })
                .collect();
            Json(json!({ "aliases": aliases })).into_response()
        }
        Err(e) => {
            tracing::error!("Error listing aliases via API: {}", e);
            (StatusCode::INTERNAL_SERVER_ERROR, "Database error").into_response()
        }
    }
}

/// Decodes base64 attachment payloads from an API request into
/// `Attachment` values, enforcing the shared attachment limits.
fn decode_api_attachments(
    raw: Option<Vec<crate::web::attachments_form::ApiAttachment>>,
) -> Result<Vec<crate::outbound::mime::Attachment>, (StatusCode, String)> {
    let mut out = Vec::new();
    let Some(raw) = raw else {
        return Ok(out);
    };

    if raw.len() > MAX_ATTACHMENTS {
        return Err((
            StatusCode::PAYLOAD_TOO_LARGE,
            format!("Too many attachments. Maximum is {}.", MAX_ATTACHMENTS),
        ));
    }

    let mut total = 0usize;
    for att in raw {
        let data = base64::engine::general_purpose::STANDARD
            .decode(att.content_b64.as_bytes())
            .map_err(|_| {
                (
                    StatusCode::BAD_REQUEST,
                    format!("Attachment '{}' is not valid base64.", att.filename),
                )
            })?;
        total += data.len();
        if total > MAX_TOTAL_ATTACHMENT_BYTES {
            return Err((
                StatusCode::PAYLOAD_TOO_LARGE,
                format!(
                    "Total attachments size exceeds limit of {} MB.",
                    MAX_TOTAL_ATTACHMENT_BYTES / 1024 / 1024
                ),
            ));
        }

        let content_type = crate::web::attachments_form::resolve_content_type(
            att.content_type.as_deref(),
            Some(&att.filename),
        );
        let filename = crate::web::attachments_form::clean_filename(Some(&att.filename));

        out.push(crate::outbound::mime::Attachment {
            filename,
            content_type,
            data,
            is_inline: false,
            content_id: None,
        });
    }

    Ok(out)
}

/// Resolves the intended alias by full address for the API send endpoint.
async fn resolve_alias_by_address(
    state: &AppState,
    user_id: Uuid,
    address: &str,
) -> Result<crate::db::Alias, (StatusCode, String)> {
    let aliases = get_aliases_by_user_id(&state.db, user_id)
        .await
        .map_err(|e| {
            tracing::error!("Database error fetching aliases: {}", e);
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                "Database error".to_string(),
            )
        })?;

    let wanted = address.trim().to_lowercase();
    aliases
        .into_iter()
        .find(|a| format!("{}@{}", a.subdomain, a.domain_name).to_lowercase() == wanted)
        .ok_or((
            StatusCode::NOT_FOUND,
            format!("Alias '{}' not found for this account.", address),
        ))
}

/// JSON API send endpoint: POST /api/v1/emails/send
pub async fn send_email_api(
    State(state): State<Arc<AppState>>,
    user: ApiUser,
    Json(payload): Json<crate::web::attachments_form::ApiSendRequest>,
) -> impl IntoResponse {
    // 1. Resolve and authorize the alias by address.
    let alias = match resolve_alias_by_address(&state, user.user.id, &payload.from_alias).await {
        Ok(a) => a,
        Err(e) => return e.into_response(),
    };

    if !alias.active {
        return (StatusCode::FORBIDDEN, "Alias is not active.".to_string()).into_response();
    }

    // 2. Enforce the firsthand permission at API level too.
    if !user.user.can_send_firsthand && !user.user.is_admin {
        return (
            StatusCode::FORBIDDEN,
            "Sending is not permitted for this user.".to_string(),
        )
            .into_response();
    }

    // 3. Decode attachments.
    let attachments = match decode_api_attachments(payload.attachments) {
        Ok(a) => a,
        Err(e) => return e.into_response(),
    };

    // 4. Send (or dry-run) through the shared core.
    match send_firsthand_email(
        &state,
        user.user.id,
        user.user.can_send_firsthand,
        user.user.is_admin,
        alias.id,
        &payload.to,
        &payload.subject,
        &payload.body,
        payload.draft_id,
        attachments,
        !payload.dry_run.unwrap_or(false),
    )
    .await
    {
        Ok(SendOutcome::Sent(sent)) => Json(json!({
            "status": "sent",
            "email_id": sent.email_id,
            "message_id": sent.message_id,
        }))
        .into_response(),
        Ok(SendOutcome::DryRun(dry)) => Json(json!({
            "status": "dry_run",
            "resolved_from": dry.resolved_from,
            "to": dry.to,
            "subject": dry.subject,
            "message_id": dry.message_id,
            "attachment_count": dry.attachment_count,
            "total_attachment_bytes": dry.total_attachment_bytes,
            "mime_size": dry.mime_size,
        }))
        .into_response(),
        Err(e) => (e.status(), e.message()).into_response(),
    }
}

/// JSON API endpoint returning a single email (received or sent) with its
/// parsed body, so clients can read conversations without HTML.
pub async fn get_email_api(
    State(state): State<Arc<AppState>>,
    user: ApiUser,
    Path(email_id): Path<Uuid>,
) -> impl IntoResponse {
    use mail_parser::MessageParser;

    let email = match get_any_email(&state.db, email_id, user.user.id).await {
        Ok(Some(e)) => e,
        _ => return (StatusCode::NOT_FOUND, "Email not found").into_response(),
    };

    let body_key = email.body_key();
    let is_outbound = matches!(email, AnyEmail::Sent(_));

    let path = state.storage_dir.join(format!("{}.eml", body_key));
    let (body, date): (String, String) = match tokio::fs::read(&path).await {
        Ok(bytes) => match MessageParser::default().parse(&bytes) {
            Some(m) => {
                let raw_body = m
                    .body_html(0)
                    .or_else(|| m.body_text(0))
                    .unwrap_or_default();
                (
                    crate::web::email_body::sanitize_email_body(&raw_body, email_id),
                    m.date().map(|d| d.to_rfc822()).unwrap_or_default(),
                )
            }
            None => (String::new(), String::new()),
        },
        Err(_) => (String::new(), String::new()),
    };

    let attachments = crate::db::attachments::get_attachments_for_email(&state.db, email_id)
        .await
        .unwrap_or_default();

    Json(json!({
        "id": email_id,
        "subject": email.subject(),
        "is_sent": is_outbound,
        "body": body,
        "date": date,
        "attachments": attachments,
    }))
    .into_response()
}
