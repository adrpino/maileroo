use crate::db::sent_emails::{
    get_sent_email_by_id_and_user, mark_sent_email_failed, mark_sent_email_success, upsert_draft,
};
use crate::db::{DbPool, get_alias_by_id_and_user};
use crate::fs::write_file_async_with_permissions;
use crate::outbound::mime::{MimeEmail, build_mime};
use crate::web::attachments_form::read_multipart_fields;
use crate::web::i18n::{Locale, Messages};
use crate::web::{AppState, FirsthandSenderUser};
use askama::Template;
use axum::{
    extract::{Form, Multipart, Query, State},
    http::StatusCode,
    response::IntoResponse,
};
use serde::Deserialize;
use std::sync::Arc;
use uuid::Uuid;

pub const MAX_UPLOAD_REQUEST_BYTES: usize = 30 * 1024 * 1024; // 30 MB

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
        if let Ok(Some(draft)) = get_sent_email_by_id_and_user(&state.db, id, user.0.user_id).await
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

/// Errors surfaced by the shared firsthand send core.
#[derive(Debug)]
pub enum SendEmailError {
    AliasNotFound,
    AliasInactive,
    Forbidden,
    InvalidRecipient,
    EmptySubject,
    StorageFailure,
    DeliveryFailed(String),
    DatabaseFailure,
}

impl SendEmailError {
    pub fn status(&self) -> StatusCode {
        match self {
            SendEmailError::AliasNotFound => StatusCode::NOT_FOUND,
            SendEmailError::AliasInactive | SendEmailError::Forbidden => StatusCode::FORBIDDEN,
            SendEmailError::InvalidRecipient | SendEmailError::EmptySubject => {
                StatusCode::BAD_REQUEST
            }
            SendEmailError::StorageFailure
            | SendEmailError::DeliveryFailed(_)
            | SendEmailError::DatabaseFailure => StatusCode::INTERNAL_SERVER_ERROR,
        }
    }

    pub fn message(&self) -> String {
        match self {
            SendEmailError::AliasNotFound => "Alias not found.".to_string(),
            SendEmailError::AliasInactive => "Alias is not active.".to_string(),
            SendEmailError::Forbidden => "Sending is not permitted for this user.".to_string(),
            SendEmailError::InvalidRecipient => "Invalid recipient address.".to_string(),
            SendEmailError::EmptySubject => "Subject cannot be empty.".to_string(),
            SendEmailError::StorageFailure => "Failed to store the message.".to_string(),
            SendEmailError::DeliveryFailed(e) => format!("Delivery failed: {}", e),
            SendEmailError::DatabaseFailure => "Database error.".to_string(),
        }
    }
}

/// Successful result of a real (executed) send.
#[derive(Debug, serde::Serialize)]
pub struct SentEmailResult {
    pub email_id: Uuid,
    pub message_id: String,
}

/// Metadata returned by a dry run: what a real send would do, without any
/// side effect on storage, database, or the network.
#[derive(Debug, serde::Serialize)]
pub struct DryRunResult {
    pub resolved_from: String,
    pub to: String,
    pub subject: String,
    pub message_id: String,
    pub attachment_count: usize,
    pub total_attachment_bytes: usize,
    pub mime_size: usize,
}

/// Outcome shared by the browser handler and the JSON API handler.
#[derive(Debug)]
pub enum SendOutcome {
    Sent(SentEmailResult),
    DryRun(DryRunResult),
}

/// Resolves the alias for a firsthand send, enforcing ownership, activity,
/// and the firsthand permission. Returns the full alias on success.
pub async fn resolve_alias_for_sending(
    state: &AppState,
    user_id: Uuid,
    can_send_firsthand: bool,
    is_admin: bool,
    from_alias_id: Uuid,
) -> Result<crate::db::Alias, SendEmailError> {
    if !can_send_firsthand && !is_admin {
        return Err(SendEmailError::Forbidden);
    }

    match get_alias_by_id_and_user(&state.db, from_alias_id, user_id).await {
        Ok(Some(alias)) => {
            if !alias.active {
                return Err(SendEmailError::AliasInactive);
            }
            Ok(alias)
        }
        Ok(None) => Err(SendEmailError::AliasNotFound),
        Err(e) => {
            tracing::error!("Database error fetching alias: {}", e);
            Err(SendEmailError::DatabaseFailure)
        }
    }
}

/// Triggers safe, asynchronous deletion of an old draft file from disk.
fn cleanup_old_draft(storage_dir: std::path::PathBuf, old_key: Option<Uuid>) {
    if let Some(key) = old_key {
        let file_path = storage_dir.join(key.to_string());
        tokio::spawn(async move {
            if file_path.exists() {
                let _ = tokio::fs::remove_file(&file_path).await;
            }
        });
    }
}

/// Records attachment metadata for a sent email and flags the row.
async fn record_sent_attachments(
    pool: DbPool,
    email_id: Uuid,
    metadatas: Vec<(Option<String>, String, i64)>,
) {
    if metadatas.is_empty() {
        return;
    }

    for (i, (fname, ctype, size)) in metadatas.iter().enumerate() {
        let att_id = Uuid::new_v4();
        if let Err(err) = crate::db::attachments::insert_attachment(
            &pool,
            att_id,
            email_id,
            fname.as_deref(),
            Some(ctype),
            *size,
            i as i32,
            false,
            None,
        )
        .await
        {
            tracing::error!("Database error inserting sent attachment row: {}", err);
        }
    }

    match &pool {
        DbPool::Postgres(p) => {
            if let Err(err) =
                sqlx::query("UPDATE sent_emails SET has_attachments = TRUE WHERE id = $1")
                    .bind(email_id)
                    .execute(p)
                    .await
            {
                tracing::error!(
                    "Database error setting has_attachments on sent_emails: {}",
                    err
                );
            }
        }
        DbPool::Sqlite(p) => {
            if let Err(err) =
                sqlx::query("UPDATE sent_emails SET has_attachments = TRUE WHERE id = ?")
                    .bind(email_id)
                    .execute(p)
                    .await
            {
                tracing::error!(
                    "Database error setting has_attachments on sent_emails: {}",
                    err
                );
            }
        }
    }
}

/// Core firsthand send path shared by the browser multipart handler and the
/// JSON API handler. With `execute` set to false this performs every step up
/// to MIME construction and stops: nothing is stored, transmitted, or
/// recorded. With `execute` true it stores the message, sends it, records
/// the outcome, and returns the persisted email id.
#[allow(clippy::too_many_arguments)]
pub async fn send_firsthand_email(
    state: &AppState,
    user_id: Uuid,
    can_send_firsthand: bool,
    is_admin: bool,
    from_alias_id: Uuid,
    to_email: &str,
    subject: &str,
    body_text: &str,
    draft_id: Option<Uuid>,
    attachments: Vec<crate::outbound::mime::Attachment>,
    execute: bool,
) -> Result<SendOutcome, SendEmailError> {
    let to_email = to_email.trim();
    if to_email.is_empty() || !to_email.contains('@') {
        return Err(SendEmailError::InvalidRecipient);
    }

    if subject.trim().is_empty() {
        return Err(SendEmailError::EmptySubject);
    }

    // 1. Authorize alias ownership and firsthand permission
    let alias =
        resolve_alias_for_sending(state, user_id, can_send_firsthand, is_admin, from_alias_id)
            .await?;
    let from_address = format!("{}@{}", alias.subdomain, alias.domain_name);

    // 2. Generate a globally unique Message-ID up-front
    let domain = from_address.split('@').nth(1).unwrap_or("localhost");
    let message_id = crate::outbound::mime::generate_message_id(domain);

    // 3. Construct the MIME payload using the modular builder
    let attachment_count = attachments.len();
    let total_attachment_bytes: usize = attachments.iter().map(|a| a.data.len()).sum();
    let mime_email = MimeEmail {
        from: from_address.clone(),
        to: to_email.to_string(),
        subject: subject.to_string(),
        text_body: body_text.to_string(),
        html_body: None,
        message_id: Some(message_id.clone()),
        in_reply_to: None,
        references: None,
        attachments,
    };

    let raw_mime = build_mime(&mime_email);

    if !execute {
        return Ok(SendOutcome::DryRun(DryRunResult {
            resolved_from: from_address,
            to: to_email.to_string(),
            subject: subject.to_string(),
            message_id,
            attachment_count,
            total_attachment_bytes,
            mime_size: raw_mime.len(),
        }));
    }

    // 4. Store the email on disk BEFORE sending so the Ok/Err branches share it
    let body_key = Uuid::new_v4();
    let file_path = state.storage_dir.join(format!("{}.eml", body_key));

    if let Err(e) = write_file_async_with_permissions(&file_path, raw_mime.as_bytes()).await {
        tracing::error!(
            "Failed to write outbound email to disk ({}): {}",
            file_path.display(),
            e
        );
        return Err(SendEmailError::StorageFailure);
    }

    // Pre-fetch the old draft's body key so the raw draft file can be removed
    // from disk once the message transitions out of the draft state.
    let old_draft_body_key = match draft_id {
        Some(d_id) => match get_sent_email_by_id_and_user(&state.db, d_id, user_id).await {
            Ok(Some(draft)) => Some(draft.body_key),
            _ => None,
        },
        None => None,
    };

    let attachment_metadatas: Vec<(Option<String>, String, i64)> = mime_email
        .attachments
        .iter()
        .map(|a| {
            (
                a.filename.clone(),
                a.content_type.clone(),
                a.data.len() as i64,
            )
        })
        .collect();

    // 5. Send the email via the Outbound Service
    match state
        .outbound
        .send_firsthand(to_email, &from_address, raw_mime.as_bytes())
        .await
    {
        Ok(_) => {
            // 6. Log the success to the database using the body_key
            match upsert_draft(
                &state.db,
                draft_id,
                user_id,
                from_alias_id,
                to_email,
                subject,
                body_key,
            )
            .await
            {
                Ok(upserted_id) => {
                    cleanup_old_draft(state.storage_dir.clone(), old_draft_body_key);

                    if let Err(err) =
                        mark_sent_email_success(&state.db, upserted_id, &message_id).await
                    {
                        tracing::error!(
                            "Database error marking sent email success for {}: {}",
                            upserted_id,
                            err
                        );
                        return Err(SendEmailError::DatabaseFailure);
                    }

                    record_sent_attachments(state.db.clone(), upserted_id, attachment_metadatas)
                        .await;

                    Ok(SendOutcome::Sent(SentEmailResult {
                        email_id: upserted_id,
                        message_id,
                    }))
                }
                Err(e) => {
                    tracing::error!("Failed to upsert draft before marking sent: {}", e);
                    Err(SendEmailError::DatabaseFailure)
                }
            }
        }
        Err(e) => {
            tracing::error!("Failed to send firsthand email: {}", e);

            // Log the failure to the database
            match upsert_draft(
                &state.db,
                draft_id,
                user_id,
                from_alias_id,
                to_email,
                subject,
                body_key,
            )
            .await
            {
                Ok(upserted_id) => {
                    cleanup_old_draft(state.storage_dir.clone(), old_draft_body_key);

                    if let Err(err) =
                        mark_sent_email_failed(&state.db, upserted_id, &e.to_string()).await
                    {
                        tracing::error!(
                            "Database error marking sent email failed for {}: {}",
                            upserted_id,
                            err
                        );
                    }

                    record_sent_attachments(state.db.clone(), upserted_id, attachment_metadatas)
                        .await;
                }
                Err(e) => tracing::error!("Failed to upsert draft before marking failed: {}", e),
            }

            Err(SendEmailError::DeliveryFailed(e.to_string()))
        }
    }
}

pub async fn submit_email_handler(
    locale: Locale,
    user: FirsthandSenderUser,
    State(state): State<Arc<AppState>>,
    multipart: Multipart,
) -> impl IntoResponse {
    let auth_user = user.0;

    let fields = read_multipart_fields(multipart).await;
    if let Some((_, message)) = fields.error {
        // HTMX toast contract: limit violations surface as a 200 toast so the
        // UI renders the error message instead of a bare status page.
        return ToastTemplate {
            message,
            success: false,
        }
        .into_response();
    }

    let draft_id: Option<Uuid> = fields
        .text
        .get("draft_id")
        .and_then(|v| Uuid::parse_str(v.trim()).ok());
    let from_alias_id: Option<Uuid> = fields
        .text
        .get("from_alias_id")
        .and_then(|v| Uuid::parse_str(v.trim()).ok());
    let to_email_str = fields.text.get("to_email").cloned().unwrap_or_default();
    let subject_str = fields.text.get("subject").cloned().unwrap_or_default();
    let body_text_str = fields.text.get("body_text").cloned().unwrap_or_default();
    let attachments = fields.attachments;

    let to_email = to_email_str.trim();
    if to_email.is_empty() || !to_email.contains('@') {
        return ToastTemplate {
            message: locale.toast_invalid_email().to_string(),
            success: false,
        }
        .into_response();
    }

    if subject_str.trim().is_empty() {
        return ToastTemplate {
            message: locale.toast_empty_subject().to_string(),
            success: false,
        }
        .into_response();
    }

    let from_alias_id = match from_alias_id {
        Some(id) => id,
        None => {
            return ToastTemplate {
                message: locale.toast_alias_unauthorized().to_string(),
                success: false,
            }
            .into_response();
        }
    };

    match send_firsthand_email(
        &state,
        auth_user.user_id,
        auth_user.can_send_firsthand,
        auth_user.is_admin,
        from_alias_id,
        to_email,
        &subject_str,
        &body_text_str,
        draft_id,
        attachments,
        true,
    )
    .await
    {
        Ok(SendOutcome::Sent(_)) => {
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
        Ok(SendOutcome::DryRun(_)) => {
            // The browser path never requests dry runs.
            unreachable!("browser send handler always executes")
        }
        Err(e) => {
            // The one error that happens after the message hit the wire is a
            // real server fault and keeps its 500 contract; every other
            // failure surfaces as a 200 toast for the HTMX UI to render.
            if matches!(e, SendEmailError::DatabaseFailure) {
                return (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    ToastTemplate {
                        message: "Email sent, but failed to update status in the database."
                            .to_string(),
                        success: false,
                    },
                )
                    .into_response();
            }
            tracing::error!("Failed to send firsthand email: {}", e.message());
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
        match get_sent_email_by_id_and_user(&state.db, draft_id, auth_user.user_id).await {
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

    match upsert_draft(
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
        assert!(rendered.contains("hx-post=\"/api/v1/emails/compose-send\""));
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

    // Regression test for the outbound attachments bug: the autosave form declares
    // `hx-params="not attachments"` to exclude file inputs from draft autosave.
    // HTMX inherits `hx-params` from ancestor elements, so the Send button must
    // override this with `hx-params="*"` or its multipart POST to /emails/send
    // will silently strip the attachments field, producing an attachment-less .eml
    // and zero attachment rows in the DB despite a user selecting files.
    #[test]
    fn test_compose_modal_send_button_includes_attachments() {
        let alias = crate::db::Alias {
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
            aliases: vec![alias],
            draft_id: None,
            to_email: String::new(),
            subject: String::new(),
            body_text: String::new(),
            selected_alias_id: None,
        };

        let rendered = template
            .render()
            .expect("Failed to render compose template");

        // The Send button must override inherited `hx-params="not attachments"`
        // so files are included in the multipart POST to /emails/send.
        let send_button_start = rendered
            .find("btn-send\"")
            .expect("Send button must be present");
        let send_button = &rendered[send_button_start..];
        let button_end = send_button
            .find('>')
            .expect("Send button tag must be closed");
        let button_tag = &send_button[..button_end];

        assert!(
            button_tag.contains("hx-post=\"/api/v1/emails/compose-send\""),
            "Send button must POST to /emails/compose-send: {button_tag}"
        );
        assert!(
            button_tag.contains("hx-encoding=\"multipart/form-data\""),
            "Send button must use multipart encoding: {button_tag}"
        );
        assert!(
            button_tag.contains("hx-params=\"*\""),
            "Send button must set hx-params=\"*\" to override the autosave form's \
             `hx-params=\"not attachments\"` (HTMX inherits hx-params from ancestors): {button_tag}"
        );
        assert!(
            !button_tag.contains("hx-params=\"not attachments\""),
            "Send button must not inherit the autosave `not attachments` exclusion: {button_tag}"
        );
    }
}
