use askama::Template;
use axum::body::Body;
use axum::{
    extract::{Path, Query, State},
    http::StatusCode,
    response::{Html, IntoResponse, Response},
};
use axum_extra::extract::Form as ExtraForm;
use serde::Deserialize;
use std::sync::Arc;
use uuid::Uuid;

use crate::db::{
    delete_email_by_id, get_email_by_id, get_email_by_user_id, get_email_count_by_user_id,
};
use crate::web::handlers::ModalTemplate;
use crate::web::i18n::{Locale, Messages};
use crate::web::{AppState, AuthenticatedUser, ThreadMessage};

#[derive(Deserialize)]
pub struct PaginationParams {
    pub page: Option<i64>,
    pub alias: Option<String>,
    pub q: Option<String>,
    pub folder: Option<String>,
}

pub struct DisplayEmail {
    pub id: uuid::Uuid,
    pub alias_address: String,
    pub correspondent_email: String,
    pub subject: String,
    pub date: time::OffsetDateTime,
    pub is_sent_folder: bool,
    pub is_viewed: bool,
    pub status: Option<crate::db::sent_emails::EmailStatus>,
}

#[derive(Template)]
#[template(path = "dashboard.html")]
pub struct DashboardTemplate {
    pub emails: Vec<DisplayEmail>,
    pub user_aliases: Vec<crate::db::Alias>,
    pub current_alias: Option<String>,
    pub query: Option<String>,
    pub current_folder: String,
    pub alias_count: i64,
    pub domain_count: i64,
    pub is_admin: bool,
    pub can_send_firsthand: bool,
    pub locale: Locale,
    pub current_page: i64,
    pub total_pages: i64,
    pub total_emails: i64,
}

impl IntoResponse for DashboardTemplate {
    fn into_response(self) -> Response {
        match self.render() {
            Ok(html) => Html(html).into_response(),
            Err(err) => (
                StatusCode::INTERNAL_SERVER_ERROR,
                format!("Failed to render template: {err}"),
            )
                .into_response(),
        }
    }
}

pub async fn dashboard_handler(
    locale: Locale,
    user: AuthenticatedUser,
    State(state): State<Arc<AppState>>,
    Query(pagination): Query<PaginationParams>,
) -> impl IntoResponse {
    let page = pagination.page.unwrap_or(1).max(1);
    let page_size = 10;
    let offset = (page - 1) * page_size;
    let alias_filter = pagination.alias.filter(|s| !s.is_empty());
    let query_filter = pagination.q.filter(|s| !s.is_empty());
    let current_folder = pagination.folder.unwrap_or_else(|| "inbox".to_string());

    let (emails, total_emails) = if current_folder == "sent" || current_folder == "drafts" {
        let status_filter = if current_folder == "drafts" {
            crate::db::sent_emails::EmailStatus::Draft
        } else {
            crate::db::sent_emails::EmailStatus::Sent
        };

        let sent_emails = crate::db::sent_emails::get_sent_emails_by_user_id(
            &state.db,
            user.user_id,
            status_filter.clone(),
            page_size,
            offset,
            alias_filter.clone(),
            query_filter.clone(),
        )
        .await
        .unwrap_or_default();

        let count = crate::db::sent_emails::get_sent_email_count_by_user_id(
            &state.db,
            user.user_id,
            status_filter,
            alias_filter.clone(),
            query_filter.clone(),
        )
        .await
        .unwrap_or(0);

        let display_emails = sent_emails.into_iter().map(|email| DisplayEmail {
            id: email.id,
            alias_address: email.alias_address,
            correspondent_email: email.to_address,
            subject: email.subject,
            date: email.updated_at,
            is_sent_folder: true,
            is_viewed: true,
            status: Some(email.status),
        }).collect();

        (display_emails, count)
    } else {
        let inbox_emails = match get_email_by_user_id(
            &state.db,
            user.user_id,
            page_size,
            offset,
            alias_filter.clone(),
            query_filter.clone(),
        )
        .await
        {
            Ok(e) => e,
            Err(e) => {
                tracing::error!("Database error fetching emails: {}", e);
                return (StatusCode::INTERNAL_SERVER_ERROR, "Database error").into_response();
            }
        };

        let count = match get_email_count_by_user_id(
            &state.db,
            user.user_id,
            alias_filter.clone(),
            query_filter.clone(),
        )
        .await
        {
            Ok(c) => c,
            Err(_) => 0,
        };

        let display_emails = inbox_emails.into_iter().map(|email| DisplayEmail {
            id: email.id,
            alias_address: email.alias_address.unwrap_or_default(),
            correspondent_email: email.sender_email,
            subject: email.subject,
            date: email.received_at,
            is_sent_folder: false,
            is_viewed: email.viewed,
            status: None,
        }).collect();

        (display_emails, count)
    };

    let total_pages = calculate_total_pages(total_emails, page_size);

    let user_aliases = match crate::db::get_aliases_by_user_id(&state.db, user.user_id).await {
        Ok(aliases) => aliases,
        Err(_) => vec![],
    };

    let alias_count = user_aliases.len() as i64;

    let domain_count = match crate::db::get_domain_count(&state.db).await {
        Ok(count) => count,
        Err(_) => 0,
    };

    (
        [
            ("Cache-Control", "no-store, no-cache, must-revalidate"),
            ("Pragma", "no-cache"),
        ],
        DashboardTemplate {
            emails,
            user_aliases,
            current_alias: alias_filter,
            query: query_filter,
            current_folder,
            alias_count,
            domain_count,
            is_admin: user.is_admin,
            can_send_firsthand: user.can_send_firsthand,
            locale,
            current_page: page,
            total_pages,
            total_emails,
        },
    )
        .into_response()
}

/// Pure function to calculate total pages.
pub fn calculate_total_pages(total_items: i64, page_size: i64) -> i64 {
    if total_items == 0 {
        1
    } else {
        (total_items as f64 / page_size as f64).ceil() as i64
    }
}

#[derive(Deserialize)]
pub struct BatchDeleteEmailsRequest {
    #[serde(default)]
    pub email_ids: Vec<Uuid>,
}

async fn perform_smart_delete(
    state: &Arc<AppState>,
    email_id: Uuid,
    user_id: Uuid,
) -> Result<bool, sqlx::Error> {
    let email_opt = crate::db::get_any_email(&state.db, email_id, user_id).await?;
    if let Some(email) = email_opt {
        let body_key = email.body_key();

        let deleted = if delete_email_by_id(&state.db, email_id, user_id).await? {
            true
        } else {
            crate::db::sent_emails::delete_sent_email_by_id(&state.db, email_id, user_id).await?
        };

        if deleted {
            let file_path = state.storage_dir.join(body_key.to_string());
            let eml_path = state.storage_dir.join(format!("{}.eml", body_key));

            tokio::spawn(async move {
                if file_path.exists() {
                    let _ = tokio::fs::remove_file(&file_path).await;
                }
                if eml_path.exists() {
                    let _ = tokio::fs::remove_file(&eml_path).await;
                }
            });
            return Ok(true);
        }
    }
    Ok(false)
}

fn create_delete_response(should_redirect: bool) -> Response {
    if should_redirect {
        Response::builder()
            .header("HX-Redirect", "/dashboard")
            .status(StatusCode::OK)
            .body(Body::empty())
            .unwrap()
    } else {
        StatusCode::OK.into_response()
    }
}

pub async fn delete_email_handler(
    user: AuthenticatedUser,
    Query(params): Query<std::collections::HashMap<String, String>>,
    State(state): State<Arc<AppState>>,
    Path(email_id): Path<Uuid>,
) -> Response {
    let should_redirect = params.get("redirect").map(|s| s == "true").unwrap_or(false);

    match perform_smart_delete(&state, email_id, user.user_id).await {
        Ok(true) => create_delete_response(should_redirect),
        Ok(false) => StatusCode::NOT_FOUND.into_response(),
        Err(e) => {
            tracing::error!("Error deleting email: {}", e);
            (StatusCode::INTERNAL_SERVER_ERROR, "Database error").into_response()
        }
    }
}

pub async fn batch_delete_emails_handler(
    user: AuthenticatedUser,
    State(state): State<Arc<AppState>>,
    ExtraForm(payload): ExtraForm<BatchDeleteEmailsRequest>,
) -> Response {
    if payload.email_ids.is_empty() {
        return create_delete_response(true);
    }

    for id in payload.email_ids {
        let _ = perform_smart_delete(&state, id, user.user_id).await;
    }

    create_delete_response(true)
}

pub async fn batch_delete_emails_confirm_handler(
    locale: Locale,
    _user: AuthenticatedUser,
    State(_state): State<Arc<AppState>>,
    ExtraForm(payload): ExtraForm<BatchDeleteEmailsRequest>,
) -> impl IntoResponse {
    let count = payload.email_ids.len();
    if count == 0 {
        return StatusCode::OK.into_response();
    }

    ModalTemplate {
        title: locale.batch_delete_modal_title().to_string(),
        message: locale.batch_delete_modal_message(count),
        action_label: locale.modal_delete_confirm().to_string(),
        cancel_label: locale.modal_cancel().to_string(),
        action_url: "/emails/batch-delete".to_string(),
        action_method: "post".to_string(),
        action_color: "danger".to_string(),
        target: "body".to_string(),
        swap: "outerHTML".to_string(),
        include_target: Some("#batch-delete-form".to_string()),
    }
    .into_response()
}

pub async fn delete_email_confirm_handler(
    locale: Locale,
    user: AuthenticatedUser,
    Query(params): Query<std::collections::HashMap<String, String>>,
    State(state): State<Arc<AppState>>,
    Path(email_id): Path<Uuid>,
) -> impl IntoResponse {
    let email = match crate::db::get_any_email(&state.db, email_id, user.user_id).await {
        Ok(Some(e)) => e,
        Ok(None) => return (StatusCode::NOT_FOUND, "Email not found").into_response(),
        Err(e) => {
            tracing::error!("Database error fetching email: {}", e);
            return (StatusCode::INTERNAL_SERVER_ERROR, "Database error").into_response();
        }
    };

    let subject = email.subject();
    let target = params
        .get("target")
        .cloned()
        .unwrap_or_else(|| format!("#email-{}", email_id));
    let swap = params
        .get("swap")
        .cloned()
        .unwrap_or_else(|| "outerHTML".to_string());

    let mut action_url = format!("/emails/{}", email_id);
    if params.get("redirect").map(|s| s == "true").unwrap_or(false) {
        action_url.push_str("?redirect=true");
    }

    ModalTemplate {
        title: locale.delete_email_title().to_string(),
        message: locale.delete_email_message(subject),
        action_label: locale.modal_delete_confirm().to_string(),
        cancel_label: locale.modal_cancel().to_string(),
        action_url,
        action_method: "delete".to_string(),
        action_color: "danger".to_string(),
        target,
        swap,
        include_target: None,
    }
    .into_response()
}

#[derive(Template)]
#[template(path = "email_detail.html")]
pub struct EmailDetailTemplate {
    pub id: uuid::Uuid,
    pub sender: String,
    pub alias_address: String,
    pub subject: String,
    pub body: String,
    pub date: String,
    pub is_forwarded: bool,
    pub is_outbound: bool,
    pub locale: Locale,
    pub replies: Vec<ThreadMessage>,
}

impl IntoResponse for EmailDetailTemplate {
    fn into_response(self) -> Response {
        match self.render() {
            Ok(html) => Html(html).into_response(),
            Err(err) => (
                StatusCode::INTERNAL_SERVER_ERROR,
                format!("Failed to render template: {err}"),
            )
                .into_response(),
        }
    }
}

pub async fn dashboard_sse_handler(
    State(state): State<Arc<AppState>>,
    locale: Locale,
    user: AuthenticatedUser,
) -> impl IntoResponse {
    use crate::web::DashboardEvent;
    use axum::response::sse::{Event, KeepAlive, Sse};

    let mut rx = state.tx.subscribe();

    let stream = async_stream::stream! {
        loop {
            match rx.recv().await {
                Ok(DashboardEvent::NewEmail { user_id, email_id }) if user_id == user.user_id => {
                    if let Ok(Some(email)) = get_email_by_id(&state.db, email_id, user.user_id).await {
                        let display_email = DisplayEmail {
                            id: email.id,
                            alias_address: email.alias_address.unwrap_or_default(),
                            correspondent_email: email.sender_email,
                            subject: email.subject,
                            date: email.received_at,
                            is_sent_folder: false,
                            is_viewed: email.viewed,
                            status: None,
                        };
                        let template = crate::web::handlers::EmailRowTemplate { email: display_email, locale: locale.clone() };
                        if let Ok(html) = askama::Template::render(&template) {
                            yield Ok::<Event, std::convert::Infallible>(Event::default().data(html).event("new_email"));
                        }
                    }
                }
                Ok(_) => continue,
                Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => continue,
                Err(tokio::sync::broadcast::error::RecvError::Closed) => break,
            }
        }
    };

    let mut response = Sse::new(stream)
        .keep_alive(KeepAlive::new().interval(std::time::Duration::from_secs(15)))
        .into_response();

    let headers = response.headers_mut();
    headers.insert(
        axum::http::header::CACHE_CONTROL,
        "no-cache".parse().unwrap(),
    );
    headers.insert(
        axum::http::header::CONNECTION,
        "keep-alive".parse().unwrap(),
    );
    headers.insert(
        axum::http::header::HeaderName::from_static("x-accel-buffering"),
        "no".parse().unwrap(),
    );

    response
}



#[cfg(test)]
mod tests {
    use super::*;
    use time::OffsetDateTime;

    #[test]
    fn test_calculate_total_pages() {
        assert_eq!(calculate_total_pages(0, 10), 1);
        assert_eq!(calculate_total_pages(10, 10), 1);
        assert_eq!(calculate_total_pages(11, 10), 2);
        assert_eq!(calculate_total_pages(20, 10), 2);
        assert_eq!(calculate_total_pages(21, 10), 3);
    }

    #[test]
    fn test_email_row_template_render() {
        let display_email = DisplayEmail {
            id: Uuid::new_v4(),
            alias_address: "alias@example.com".to_string(),
            correspondent_email: "test@example.com".to_string(),
            subject: "Hello Test".to_string(),
            date: OffsetDateTime::now_utc(),
            is_sent_folder: false,
            is_viewed: false,
            status: None,
        };

        let template = crate::web::handlers::EmailRowTemplate {
            email: display_email,
            locale: Locale::En,
        };

        let rendered = askama::Template::render(&template).unwrap();
        assert!(rendered.contains("test@example.com"));
        assert!(rendered.contains("Hello Test"));
    }

    #[test]
    fn test_display_email_from_sent_email_row() {
        use crate::db::sent_emails::{SentEmailRow, EmailStatus};
        
        let sent_row = SentEmailRow {
            id: Uuid::new_v4(),
            user_id: Uuid::new_v4(),
            from_alias_id: Uuid::new_v4(),
            to_address: "recipient@test.com".to_string(),
            cc_addresses: None,
            bcc_addresses: None,
            subject: "Sent Subject".to_string(),
            body_key: Uuid::new_v4(),
            status: EmailStatus::Sent,
            error_message: None,
            message_id: None,
            created_at: OffsetDateTime::now_utc(),
            updated_at: OffsetDateTime::now_utc(),
            sent_at: Some(OffsetDateTime::now_utc()),
            alias_address: "myalias@domain.com".to_string(),
        };

        let display_email = DisplayEmail {
            id: sent_row.id,
            alias_address: sent_row.alias_address.clone(),
            correspondent_email: sent_row.to_address.clone(),
            subject: sent_row.subject.clone(),
            date: sent_row.created_at,
            is_sent_folder: true,
            is_viewed: true,
            status: Some(sent_row.status.clone()),
        };

        assert_eq!(display_email.is_sent_folder, true);
        assert_eq!(display_email.correspondent_email, "recipient@test.com");
        assert_eq!(display_email.alias_address, "myalias@domain.com");
        assert_eq!(display_email.status, Some(EmailStatus::Sent));
    }

    #[test]
    fn test_email_detail_template_outbound_flag() {
        let template = EmailDetailTemplate {
            id: Uuid::new_v4(),
            sender: "alias@domain.com".to_string(),
            alias_address: "recipient@other.com".to_string(),
            subject: "Test".to_string(),
            body: "Body".to_string(),
            date: "2023-01-01".to_string(),
            is_forwarded: false,
            is_outbound: true,
            locale: Locale::En,
            replies: vec![],
        };

        assert!(template.is_outbound);
        let rendered = template.render().unwrap();
        // The reply container should be hidden when is_outbound is true
        assert!(!rendered.contains("id=\"replies-container\""));
    }
}
