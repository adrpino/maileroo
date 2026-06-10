use crate::db::replies::{EmailReply, insert_reply};
use crate::db::{get_alias_details_for_email, get_email_by_id};
use crate::web::i18n::Messages;
use crate::web::{AppState, AuthenticatedUser, ThreadMessage};
use askama::Template;
use axum::{
    Form,
    extract::{Path, State},
    http::StatusCode,
    response::IntoResponse,
};
use mail_parser::MessageParser;
use serde::Deserialize;
use std::sync::Arc;
use uuid::Uuid;

#[derive(Deserialize)]
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
) -> Result<EmailReply, (StatusCode, String)> {
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

    // 4. Send the email
    let new_message_id = format!(
        "<{}@{}>",
        uuid::Uuid::new_v4(),
        state.outbound.identity_domain()
    );

    if let Err(e) = state
        .outbound
        .send_reply(
            &email.sender_email,
            &from_alias,
            &email.subject,
            body_text,
            original_message_id,
            Some(new_message_id.clone()),
        )
        .await
    {
        tracing::error!("Failed to send reply: {}", e);
        return Err((
            StatusCode::INTERNAL_SERVER_ERROR,
            "Failed to send email".to_string(),
        ));
    }

    // 5. Save the reply to DB
    match insert_reply(&state.db, email_id, body_text, Some(new_message_id)).await {
        Ok(reply) => Ok(reply),
        Err(e) => {
            tracing::error!("Failed to save reply: {}", e);
            Err((
                StatusCode::INTERNAL_SERVER_ERROR,
                "Failed to save reply".to_string(),
            ))
        }
    }
}

pub async fn submit_reply_handler(
    locale: crate::web::i18n::Locale,
    user: AuthenticatedUser,
    State(state): State<Arc<AppState>>,
    Path(email_id): Path<Uuid>,
    Form(payload): Form<ReplyRequest>,
) -> impl IntoResponse {
    match process_reply(&state, user.user_id, email_id, &payload.body_text).await {
        Ok(reply) => {
            let thread_msg = ThreadMessage::Outbound {
                id: reply.id,
                body_text: reply.body_text,
                sent_at: reply.sent_at,
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
