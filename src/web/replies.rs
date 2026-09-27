use crate::db::replies::{EmailReply, get_reply_by_id, insert_reply};
use crate::db::{get_alias_details_for_email, get_email_by_id};
use crate::fs::write_file_async_with_permissions;
use crate::web::attachments_form::read_multipart_fields;
use crate::web::i18n::Messages;
use crate::web::{AppState, AuthenticatedUser, ThreadMessage};
use askama::Template;
use axum::{
    extract::{Multipart, Path, State},
    http::StatusCode,
    response::{IntoResponse, Response},
};
use mail_parser::MessageParser;
use std::sync::Arc;
use uuid::Uuid;

use crate::db::attachments::{
    ReplyAttachmentRow, get_attachments_for_reply, insert_reply_attachment,
};

#[derive(serde::Deserialize)]
pub struct ReplyRequest {
    pub body_text: String,
}

#[derive(Template)]
#[template(path = "email_reply_row.html")]
pub struct EmailReplyRowTemplate {
    pub reply: ThreadMessage,
    pub locale: crate::web::i18n::Locale,
}

impl IntoResponse for EmailReplyRowTemplate {
    fn into_response(self) -> axum::response::Response {
        match self.render() {
            Ok(html) => axum::response::Html(html).into_response(),
            Err(err) => (
                StatusCode::INTERNAL_SERVER_ERROR,
                format!("Failed to render template: {err}"),
            )
                .into_response(),
        }
    }
}

pub async fn process_reply(
    state: &AppState,
    user_id: Uuid,
    email_id: Uuid,
    body_text: &str,
    attachments: Vec<crate::outbound::mime::Attachment>,
) -> Result<(EmailReply, Vec<ReplyAttachmentRow>), (StatusCode, String)> {
    // 1. Verify ownership and get email details
    let email = match get_email_by_id(&state.db, email_id, user_id).await {
        Ok(Some(e)) => e,
        _ => return Err((StatusCode::NOT_FOUND, "Email not found".to_string())),
    };

    // 2. Get alias details for FROM address
    let (subdomain, domain_name) =
        match get_alias_details_for_email(&state.db, email_id, user_id).await {
            Ok(Some(details)) => details,
            _ => {
                return Err((
                    StatusCode::INTERNAL_SERVER_ERROR,
                    "Alias not found".to_string(),
                ));
            }
        };
    let from_alias = format!("{}@{}", subdomain, domain_name);

    // 3. Get original message ID for threading
    let path = state.storage_dir.join(format!("{}.eml", email.body_key));
    let original_message_id = if let Ok(bytes) = tokio::fs::read(&path).await {
        let message = MessageParser::default().parse(&bytes).unwrap();
        message.message_id().map(|id| id.to_string())
    } else {
        None
    };

    // 4. Send the reply; on success keep the built MIME so it can be stored
    //    and attachment parts stay downloadable.
    let new_message_id = format!(
        "<{}@{}>",
        uuid::Uuid::new_v4(),
        state.outbound.identity_domain()
    );

    let attachment_metadatas: Vec<(Option<String>, String, usize)> = attachments
        .iter()
        .map(|a| (a.filename.clone(), a.content_type.clone(), a.data.len()))
        .collect();

    let raw_mime = state
        .outbound
        .send_reply(
            &email.sender_email,
            &from_alias,
            &email.subject,
            body_text,
            original_message_id,
            Some(new_message_id.clone()),
            attachments,
        )
        .await
        .map_err(|e| {
            tracing::error!("Failed to send reply: {}", e);
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                "Failed to send email".to_string(),
            )
        })?;

    // 5. Persist the reply and its attachments: the .eml on disk plus one
    //    metadata row per attachment keyed on the reply id.
    let body_key = Uuid::new_v4();
    let eml_path = state.storage_dir.join(format!("{}.eml", body_key));
    if let Err(e) = write_file_async_with_permissions(&eml_path, &raw_mime).await {
        tracing::error!(
            "Failed to write reply email to disk ({}): {}",
            eml_path.display(),
            e
        );
    }

    let reply = insert_reply(
        &state.db,
        email_id,
        body_text,
        Some(new_message_id),
        Some(body_key),
    )
    .await
    .map_err(|e| {
        tracing::error!("Failed to save reply: {}", e);
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            "Failed to save reply".to_string(),
        )
    })?;

    let mut reply_attachments = Vec::new();
    for (index, (fname, ctype, size)) in attachment_metadatas.iter().enumerate() {
        let att_id = Uuid::new_v4();
        insert_reply_attachment(
            &state.db,
            att_id,
            email_id,
            reply.id,
            fname.as_deref(),
            Some(ctype),
            *size as i64,
            index as i32,
        )
        .await
        .map_err(|e| {
            tracing::error!("Failed to save reply attachment row: {}", e);
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                "Failed to save reply attachment".to_string(),
            )
        })?;

        reply_attachments.push(ReplyAttachmentRow {
            id: att_id,
            reply_id: reply.id,
            filename: fname.clone(),
            content_type: Some(ctype.clone()),
            size_bytes: *size as i64,
            part_index: index as i32,
            created_at: reply.sent_at,
        });
    }

    Ok((reply, reply_attachments))
}

pub async fn submit_reply_handler(
    locale: crate::web::i18n::Locale,
    user: AuthenticatedUser,
    State(state): State<Arc<AppState>>,
    Path(email_id): Path<Uuid>,
    multipart: Multipart,
) -> impl IntoResponse {
    let fields = read_multipart_fields(multipart).await;
    if let Some((status, message)) = fields.error {
        return (status, message).into_response();
    }

    let body_text = fields.text.get("body_text").cloned().unwrap_or_default();
    if body_text.trim().is_empty() {
        return (
            StatusCode::BAD_REQUEST,
            locale.reply_empty_body_error().to_string(),
        )
            .into_response();
    }

    match process_reply(
        &state,
        user.user_id,
        email_id,
        &body_text,
        fields.attachments,
    )
    .await
    {
        Ok((reply, reply_attachments)) => {
            let thread_msg = ThreadMessage::Outbound {
                id: reply.id,
                body_text: reply.body_text,
                sent_at: reply.sent_at,
                attachments: reply_attachments,
            };
            EmailReplyRowTemplate {
                reply: thread_msg,
                locale,
            }
            .into_response()
        }
        Err((status, msg)) => (status, msg).into_response(),
    }
}

pub async fn download_reply_attachment_handler(
    State(state): State<Arc<AppState>>,
    Path((reply_id, attachment_id)): Path<(Uuid, Uuid)>,
    user: AuthenticatedUser,
) -> axum::response::Response {
    use axum::http::header;
    use mail_parser::MessageParser;

    // 1. Authorize: the reply's parent email must belong to the user.
    let reply = match get_reply_by_id(&state.db, reply_id).await {
        Ok(Some(r)) => r,
        _ => return (StatusCode::NOT_FOUND, "Reply not found").into_response(),
    };

    match get_email_by_id(&state.db, reply.email_id, user.user_id).await {
        Ok(Some(_)) => {}
        _ => return (StatusCode::NOT_FOUND, "Email not found").into_response(),
    }

    // 2. The attachment must belong to this reply.
    let attachment = match get_attachments_for_reply(&state.db, reply_id).await {
        Ok(rows) => match rows.into_iter().find(|a| a.id == attachment_id) {
            Some(a) => a,
            None => return (StatusCode::NOT_FOUND, "Attachment not found").into_response(),
        },
        Err(_) => return (StatusCode::NOT_FOUND, "Attachment not found").into_response(),
    };

    // 3. Read the stored reply .eml and extract the attachment part.
    let body_key = match reply.body_key {
        Some(k) => k,
        None => return (StatusCode::NOT_FOUND, "Reply content not found").into_response(),
    };

    let path = state.storage_dir.join(format!("{}.eml", body_key));
    let bytes = match tokio::fs::read(&path).await {
        Ok(b) => b,
        Err(_) => return (StatusCode::NOT_FOUND, "File not found").into_response(),
    };

    let message = match MessageParser::default().parse(&bytes) {
        Some(m) => m,
        None => {
            return (StatusCode::INTERNAL_SERVER_ERROR, "Failed to parse email").into_response();
        }
    };

    let part = match message.attachments().nth(attachment.part_index as usize) {
        Some(p) => p,
        None => {
            return (StatusCode::NOT_FOUND, "Attachment part not found in file").into_response();
        }
    };

    let data = part.contents().to_vec();

    // 4. Send with safe headers.
    let raw = attachment.filename.as_deref().unwrap_or("attachment");
    let ascii = crate::outbound::mime::sanitize_header(raw).replace('"', "");

    match Response::builder()
        .header(
            header::CONTENT_DISPOSITION,
            format!("attachment; filename=\"{}\"", ascii),
        )
        .header(header::CONTENT_TYPE, "application/octet-stream")
        .header(header::X_CONTENT_TYPE_OPTIONS, "nosniff")
        .body(axum::body::Body::from(data))
    {
        Ok(resp) => resp,
        Err(_) => (StatusCode::INTERNAL_SERVER_ERROR, "bad header").into_response(),
    }
}
