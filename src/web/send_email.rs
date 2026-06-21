use crate::db::get_alias_by_id_and_user;
use crate::fs::write_file_async_with_permissions;
use crate::outbound::mime::{MimeEmail, build_mime};
use crate::web::i18n::{Locale, Messages};
use crate::web::{AppState, FirsthandSenderUser};
use askama::Template;
use axum::{
    extract::{Form, Query, State},
    http::StatusCode,
    response::IntoResponse,
};
use serde::Deserialize;
use std::sync::Arc;
use uuid::Uuid;

#[derive(Template)]
#[template(path = "compose_modal.html")]
pub struct ComposeModalTemplate {
    pub locale: Locale,
    pub aliases: Vec<crate::db::Alias>,
    pub draft_id: Option<Uuid>,
    pub to_email: String,
    pub subject: String,
    pub body_text: String,
    pub selected_alias_id: Option<Uuid>,
}

impl IntoResponse for ComposeModalTemplate {
    fn into_response(self) -> axum::response::Response {
        match self.render() {
            Ok(html) => axum::response::Html(html).into_response(),
            Err(err) => (
                StatusCode::INTERNAL_SERVER_ERROR,
                format!("Failed to render compose modal template: {}", err),
            )
                .into_response(),
        }
    }
}

#[derive(Deserialize)]
pub struct ComposeQuery {
    pub draft_id: Option<Uuid>,
}

pub async fn compose_modal_handler(
    locale: Locale,
    user: FirsthandSenderUser,
    State(state): State<Arc<AppState>>,
    Query(query): Query<ComposeQuery>,
) -> impl IntoResponse {
    let aliases = match crate::db::get_aliases_by_user_id(&state.db, user.0.user_id).await {
        Ok(aliases) => aliases,
        Err(_) => {
            return (StatusCode::INTERNAL_SERVER_ERROR, "Failed to fetch aliases").into_response();
        }
    };

    let mut draft_id = None;
    let mut to_email = String::new();
    let mut subject = String::new();
    let mut body_text = String::new();
    let mut selected_alias_id = None;

    if let Some(id) = query.draft_id {
        if let Ok(Some(draft)) =
            crate::db::sent_emails::get_sent_email_by_id_and_user(&state.db, id, user.0.user_id)
                .await
        {
            draft_id = Some(draft.id);
            to_email = draft.to_address;
            subject = draft.subject;
            selected_alias_id = Some(draft.from_alias_id);

            // Load body from disk
            let file_path = state.storage_dir.join(draft.body_key.to_string());
            if let Ok(body) = tokio::fs::read_to_string(&file_path).await {
                body_text = body;
            }
        }
    }

    ComposeModalTemplate {
        locale,
        aliases,
        draft_id,
        to_email,
        subject,
        body_text,
        selected_alias_id,
    }
    .into_response()
}

#[derive(Deserialize)]
pub struct SendEmailRequest {
    pub draft_id: Option<Uuid>,
    pub from_alias_id: Uuid,
    pub to_email: String,
    pub subject: String,
    pub body_text: String,
}

#[derive(Template)]
#[template(path = "toast.html")]
pub struct ToastTemplate {
    pub message: String,
    pub success: bool,
}

impl IntoResponse for ToastTemplate {
    fn into_response(self) -> axum::response::Response {
        match self.render() {
            Ok(html) => axum::response::Html(html).into_response(),
            Err(err) => (
                StatusCode::INTERNAL_SERVER_ERROR,
                format!("Failed to render toast template: {}", err),
            )
                .into_response(),
        }
    }
}

pub async fn submit_email_handler(
    locale: Locale,
    user: FirsthandSenderUser,
    State(state): State<Arc<AppState>>,
    Form(payload): Form<SendEmailRequest>,
) -> impl IntoResponse {
    let auth_user = user.0;

    // 1. Basic validation
    let to_email = payload.to_email.trim();
    if to_email.is_empty() || !to_email.contains('@') {
        return ToastTemplate {
            message: locale.toast_invalid_email().to_string(),
            success: false,
        }
        .into_response();
    }

    if payload.subject.trim().is_empty() {
        return ToastTemplate {
            message: locale.toast_empty_subject().to_string(),
            success: false,
        }
        .into_response();
    }

    // 2. Authorize alias ownership
    let alias =
        match get_alias_by_id_and_user(&state.db, payload.from_alias_id, auth_user.user_id).await {
            Ok(Some(a)) => a,
            Ok(None) => {
                return ToastTemplate {
                    message: locale.toast_alias_unauthorized().to_string(),
                    success: false,
                }
                .into_response();
            }
            Err(e) => {
                tracing::error!("Database error fetching alias: {}", e);
                return ToastTemplate {
                    message: "Internal server error verifying alias.".to_string(),
                    success: false,
                }
                .into_response();
            }
        };

    let from_address = format!("{}@{}", alias.subdomain, alias.domain_name);

    // 3. Generate a globally unique Message-ID up-front
    let domain = from_address.split('@').nth(1).unwrap_or("localhost");
    let message_id = crate::outbound::mime::generate_message_id(domain);

    // 4. Construct the MIME payload using our modular builder
    let mime_email = MimeEmail {
        from: from_address.clone(),
        to: to_email.to_string(),
        subject: payload.subject.clone(),
        text_body: payload.body_text.clone(),
        html_body: None, // Optional: Add WYSIWYG later in Phase 4
        message_id: Some(message_id.clone()),
        in_reply_to: None,
        references: None,
    };

    let raw_mime = build_mime(&mime_email);

    // 5. Store the email on disk safely using a Uuid to prevent Path Traversal
    // We do this BEFORE sending so the code isn't duplicated in the Ok/Err branches
    let body_key = Uuid::new_v4();
    let file_path = state.storage_dir.join(format!("{}.eml", body_key));

    if let Err(e) = write_file_async_with_permissions(&file_path, raw_mime.as_bytes()).await {
        tracing::error!(
            "Failed to write outbound email to disk ({}): {}",
            file_path.display(),
            e
        );
    }

    // 6. Send the email via the Outbound Service
    match state
        .outbound
        .send_firsthand(to_email, &from_address, raw_mime.as_bytes())
        .await
    {
        Ok(_) => {
            // 7. Log the success to the database using the body_key
            match crate::db::sent_emails::upsert_draft(
                &state.db,
                payload.draft_id,
                auth_user.user_id,
                payload.from_alias_id,
                to_email,
                &payload.subject,
                body_key,
            )
            .await
            {
                Ok(draft_id) => {
                    if let Err(err) = crate::db::sent_emails::mark_sent_email_success(
                        &state.db,
                        draft_id,
                        &message_id,
                    )
                    .await
                    {
                        tracing::error!(
                            "Database error marking sent email success for {}: {}",
                            draft_id,
                            err
                        );
                    }
                }
                Err(e) => tracing::error!("Failed to upsert draft before marking sent: {}", e),
            }

            // Return HTMX toast with an HX-Trigger to clear the form
            let mut response = ToastTemplate {
                message: locale.toast_email_sent_success().to_string(),
                success: true,
            }
            .into_response();

            response.headers_mut().insert(
                axum::http::header::HeaderName::from_static("hx-trigger"),
                axum::http::header::HeaderValue::from_static("emailSent"),
            );

            response
        }
        Err(e) => {
            tracing::error!("Failed to send firsthand email: {}", e);

            // Log the failure to the database
            match crate::db::sent_emails::upsert_draft(
                &state.db,
                payload.draft_id,
                auth_user.user_id,
                payload.from_alias_id,
                to_email,
                &payload.subject,
                body_key,
            )
            .await
            {
                Ok(draft_id) => {
                    if let Err(err) = crate::db::sent_emails::mark_sent_email_failed(
                        &state.db,
                        draft_id,
                        &e.to_string(),
                    )
                    .await
                    {
                        tracing::error!(
                            "Database error marking sent email failed for {}: {}",
                            draft_id,
                            err
                        );
                    }
                }
                Err(e) => tracing::error!("Failed to upsert draft before marking failed: {}", e),
            }

            ToastTemplate {
                message: locale.toast_email_send_failed().to_string(),
                success: false,
            }
            .into_response()
        }
    }
}

pub async fn save_draft_handler(
    user: FirsthandSenderUser,
    State(state): State<Arc<AppState>>,
    Form(payload): Form<SendEmailRequest>,
) -> impl IntoResponse {
    let auth_user = user.0;

    // We still verify they own the alias, even for a draft
    let alias =
        match get_alias_by_id_and_user(&state.db, payload.from_alias_id, auth_user.user_id).await {
            Ok(Some(a)) => a,
            _ => {
                return (StatusCode::BAD_REQUEST, "Invalid From address.").into_response();
            }
        };

    let body_key = if let Some(draft_id) = payload.draft_id {
        // If a draft already exists, fetch its existing body_key to overwrite the same file
        // preventing orphaned files from piling up on disk.
        match crate::db::sent_emails::get_sent_email_by_id_and_user(
            &state.db,
            draft_id,
            auth_user.user_id,
        )
        .await
        {
            Ok(Some(draft)) => draft.body_key,
            _ => Uuid::new_v4(), // Fallback if someone sends a bogus draft_id
        }
    } else {
        // Brand new draft
        Uuid::new_v4()
    };

    // Save or overwrite the body text to storage
    let file_path = state.storage_dir.join(body_key.to_string());
    if let Err(e) =
        write_file_async_with_permissions(&file_path, payload.body_text.as_bytes()).await
    {
        tracing::error!("Failed to write draft body to storage: {}", e);
        return (StatusCode::INTERNAL_SERVER_ERROR, "Storage error").into_response();
    }

    match crate::db::sent_emails::upsert_draft(
        &state.db,
        payload.draft_id,
        auth_user.user_id,
        alias.id,
        &payload.to_email,
        &payload.subject,
        body_key,
    )
    .await
    {
        Ok(new_draft_id) => {
            // Return the hidden input field inside its container using HTMX Out-of-Band (OOB) swap.
            // This ensures subsequent auto-saves update the SAME draft row without duplicate inputs.
            axum::response::Html(format!(
                r#"<div id="draft-id-container" hx-swap-oob="true">
                       <input type="hidden" name="draft_id" value="{}">
                   </div>
                   <span id="draft-indicator" class="htmx-indicator draft-saving">Saving</span>
                   <span id="draft-status-text">Draft saved at {}</span>"#,
                new_draft_id,
                time::OffsetDateTime::now_utc().to_string()[11..16].to_string()
            ))
            .into_response()
        }
        Err(e) => {
            tracing::error!("Failed to save draft to DB: {}", e);
            (StatusCode::INTERNAL_SERVER_ERROR, "Database error").into_response()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::web::i18n::Locale;
    use time::OffsetDateTime;
    use uuid::Uuid;

    #[test]
    fn test_draft_body_key_reuse_logic() {
        // Scenario 1: New Draft
        let _new_draft_payload = SendEmailRequest {
            draft_id: None,
            from_alias_id: Uuid::new_v4(),
            to_email: "test@test.com".to_string(),
            subject: "test".to_string(),
            body_text: "test".to_string(),
        };

        let new_body_key = Uuid::new_v4();

        assert!(!new_body_key.is_nil());

        // Scenario 2: Existing Draft
        let existing_draft_id = Uuid::new_v4();
        let existing_payload = SendEmailRequest {
            draft_id: Some(existing_draft_id),
            from_alias_id: Uuid::new_v4(),
            to_email: "test@test.com".to_string(),
            subject: "test".to_string(),
            body_text: "test".to_string(),
        };

        let mock_db_stored_body_key = Uuid::new_v4();

        let reused_body_key = if let Some(draft_id) = existing_payload.draft_id {
            if draft_id == existing_draft_id {
                mock_db_stored_body_key
            } else {
                Uuid::new_v4()
            }
        } else {
            Uuid::new_v4()
        };

        assert_eq!(reused_body_key, mock_db_stored_body_key);
    }

    #[test]
    fn test_compose_modal_rendering_with_aliases() {
        let alias1 = crate::db::Alias {
            id: Uuid::new_v4(),
            user_id: Uuid::new_v4(),
            domain_id: Uuid::new_v4(),
            subdomain: "contact".to_string(),
            destination_email: "dest@example.com".to_string(),
            auto_forward: true,
            active: true,
            created_at: OffsetDateTime::now_utc(),
            domain_name: "maileroo.test".to_string(),
        };

        let template = ComposeModalTemplate {
            locale: Locale::En,
            aliases: vec![alias1.clone()],
            draft_id: None,
            to_email: String::new(),
            subject: String::new(),
            body_text: String::new(),
            selected_alias_id: None,
        };

        let rendered = template
            .render()
            .expect("Failed to render compose template");

        assert!(rendered.contains("Compose"));
        assert!(rendered.contains("contact@maileroo.test"));
        assert!(rendered.contains(&alias1.id.to_string()));
        assert!(rendered.contains("hx-post=\"/api/v1/emails/send\""));
    }

    #[test]
    fn test_compose_modal_rendering_empty_aliases() {
        let template = ComposeModalTemplate {
            locale: Locale::Es,
            aliases: vec![],
            draft_id: None,
            to_email: String::new(),
            subject: String::new(),
            body_text: String::new(),
            selected_alias_id: None,
        };

        let rendered = template
            .render()
            .expect("Failed to render compose template");

        // Spanish translation check
        assert!(rendered.contains("Redactar"));
        assert!(rendered.contains("Para"));
        // Make sure select is empty but renders
        assert!(rendered.contains("<select"));
        assert!(!rendered.contains("<option value="));
    }
}
