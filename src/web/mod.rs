pub mod admin_handlers;
pub mod alias_api;
pub mod api;
pub mod api_auth;
pub mod auth;
pub mod autotls;
pub mod dashboard;
pub mod dkim;
pub mod email_body;
pub mod handlers;
pub mod i18n;
pub mod names;
pub mod replies;
pub mod send_email;

use crate::db::DbPool;
use axum::extract::{FromRequestParts, State};
use axum::{
    Json, Router,
    body::Body,
    extract::Path,
    http::{StatusCode, request::Parts},
    response::{IntoResponse, Response},
    routing::{delete, get, post},
};
use serde_json::{Value, json};
use std::net::SocketAddr;
use std::sync::Arc;
use uuid::Uuid;

use crate::dns::DnsScanner;
use crate::db::attachments::get_attachments_for_email;
use axum_server::tls_rustls::RustlsConfig;

use tower_http::trace::TraceLayer;
use tower_sessions::{Expiry, SessionManagerLayer};
use tower_sessions_sqlx_store::PostgresStore;

const HTMX_SRC: &str = include_str!("../../templates/htmx.min.js");
const SSE_SRC: &str = include_str!("../../templates/sse.js");

#[derive(Clone, serde::Serialize, Copy)]
pub enum DashboardEvent {
    NewEmail { user_id: Uuid, email_id: Uuid },
    EmailForwarded { user_id: Uuid, email_id: Uuid },
}

#[derive(Clone)]
pub struct AppState {
    pub db: DbPool,
    pub storage_dir: std::path::PathBuf,
    pub dns_scanner: DnsScanner,
    pub tx: tokio::sync::broadcast::Sender<DashboardEvent>,
    pub outbound: Arc<crate::outbound::OutboundService>,
    pub config: crate::config::AppConfig,
}

use mail_parser::MessageParser;

#[derive(sqlx::FromRow, serde::Serialize, Debug, Clone)]
pub struct ReceivedEmail {
    pub id: uuid::Uuid,
    pub alias_id: Option<uuid::Uuid>,
    pub alias_address: Option<String>,
    pub user_id: uuid::Uuid,
    pub sender_email: String,
    pub subject: String,
    pub body_key: uuid::Uuid,
    pub received_at: time::OffsetDateTime,
    pub last_activity_at: time::OffsetDateTime,
    pub viewed: bool,
    pub forwarded: bool,
    pub message_id: Option<String>,
    pub thread_id: Option<uuid::Uuid>,
    pub has_attachments: bool,
}

pub enum ThreadMessage {
    Inbound {
        id: uuid::Uuid,
        sender: String,
        body_text: String,
        sent_at: time::OffsetDateTime,
    },
    Outbound {
        id: uuid::Uuid,
        body_text: String,
        sent_at: time::OffsetDateTime,
    },
}

impl ThreadMessage {
    pub fn sent_at(&self) -> time::OffsetDateTime {
        match self {
            ThreadMessage::Inbound { sent_at, .. } => *sent_at,
            ThreadMessage::Outbound { sent_at, .. } => *sent_at,
        }
    }
}

pub fn merge_thread_messages(
    mut inbound: Vec<ThreadMessage>,
    outbound: Vec<ThreadMessage>,
) -> Vec<ThreadMessage> {
    inbound.extend(outbound);
    inbound.sort_by_key(|m| m.sent_at());
    inbound
}

pub struct AuthenticatedUser {
    pub user_id: Uuid,
    pub is_admin: bool,
    pub bypass_alias_limit: bool,
    pub can_send_firsthand: bool,
}

impl AuthenticatedUser {
    /// Pure function to determine if a user can bypass the alias limit.
    pub fn can_bypass_alias_limit(&self) -> bool {
        self.is_admin || self.bypass_alias_limit
    }
}

#[derive(Debug, PartialEq)]
pub enum AuthError {
    NotLoggedIn,
    SessionError,
    UserNotFound,
    Forbidden,
    CsrfVerificationFailed,
}

impl axum::response::IntoResponse for AuthError {
    fn into_response(self) -> Response {
        match self {
            AuthError::Forbidden => {
                (StatusCode::FORBIDDEN, "Admin access required").into_response()
            }
            AuthError::CsrfVerificationFailed => {
                (StatusCode::FORBIDDEN, "CSRF verification failed").into_response()
            }
            _ => Response::builder()
                .status(StatusCode::SEE_OTHER)
                .header("Location", "/login")
                .header("HX-Redirect", "/login")
                .body(Body::empty())
                .unwrap(),
        }
    }
}

/// Pure function to validate a CSRF token.
pub fn validate_csrf_token(
    method: &axum::http::Method,
    header_token: Option<&str>,
    session_token: Option<&str>,
) -> Result<(), AuthError> {
    // Only state-changing methods require CSRF validation
    if method == axum::http::Method::GET
        || method == axum::http::Method::OPTIONS
        || method == axum::http::Method::HEAD
    {
        return Ok(());
    }

    match (header_token, session_token) {
        (Some(h_val), Some(s_val)) if h_val == s_val && !s_val.is_empty() => Ok(()),
        _ => Err(AuthError::CsrfVerificationFailed),
    }
}

impl<S> FromRequestParts<S> for AuthenticatedUser
where
    Arc<AppState>: FromRef<S>,
    S: Send + Sync,
{
    type Rejection = AuthError;

    async fn from_request_parts(parts: &mut Parts, state: &S) -> Result<Self, Self::Rejection> {
        let session = tower_sessions::Session::from_request_parts(parts, state)
            .await
            .map_err(|_| AuthError::SessionError)?;

        let user_id: Uuid = session
            .get("user_id")
            .await
            .map_err(|_| AuthError::SessionError)?
            .ok_or(AuthError::NotLoggedIn)?;

        // CSRF Token Validation Check (Using Pure Function)
        let header_token = parts
            .headers
            .get("X-CSRF-Token")
            .and_then(|h| h.to_str().ok());
        let session_token: Option<String> = session.get("csrf_token").await.unwrap_or(None);
        validate_csrf_token(&parts.method, header_token, session_token.as_deref())?;

        // 1. Try to get user flags from session (Caching)
        let is_admin = session
            .get::<bool>("is_admin")
            .await
            .unwrap_or(Some(false))
            .unwrap_or(false);
        let bypass_alias_limit = session
            .get::<bool>("bypass_alias_limit")
            .await
            .unwrap_or(Some(false))
            .unwrap_or(false);
        let can_send_firsthand = session
            .get::<bool>("can_send_firsthand")
            .await
            .unwrap_or(Some(false))
            .unwrap_or(false);
        let data_loaded = session
            .get::<bool>("user_data_loaded")
            .await
            .unwrap_or(Some(false))
            .unwrap_or(false);

        let (is_admin, bypass_alias_limit, can_send_firsthand) = if !data_loaded {
            // 2. Fallback to DB if not cached or session is incomplete
            let app_state = Arc::<AppState>::from_ref(state);

            let user = crate::db::get_user_by_id(&app_state.db, user_id)
                .await
                .map_err(|_| AuthError::SessionError)?
                .ok_or(AuthError::UserNotFound)?;

            // 3. Cache it back in session for next time
            let _ = session.insert("is_admin", user.is_admin).await;
            let _ = session
                .insert("bypass_alias_limit", user.bypass_alias_limit)
                .await;
            let _ = session
                .insert("can_send_firsthand", user.can_send_firsthand)
                .await;
            let _ = session.insert("user_data_loaded", true).await;
            (
                user.is_admin,
                user.bypass_alias_limit,
                user.can_send_firsthand,
            )
        } else {
            (is_admin, bypass_alias_limit, can_send_firsthand)
        };

        Ok(AuthenticatedUser {
            user_id,
            is_admin,
            bypass_alias_limit,
            can_send_firsthand,
        })
    }
}

pub struct AdminUser(pub AuthenticatedUser);

impl<S> FromRequestParts<S> for AdminUser
where
    Arc<AppState>: FromRef<S>,
    S: Send + Sync,
{
    type Rejection = AuthError;

    async fn from_request_parts(parts: &mut Parts, state: &S) -> Result<Self, Self::Rejection> {
        let auth = AuthenticatedUser::from_request_parts(parts, state).await?;
        if auth.is_admin {
            Ok(AdminUser(auth))
        } else {
            Err(AuthError::Forbidden)
        }
    }
}

pub struct FirsthandSenderUser(pub AuthenticatedUser);

impl<S> FromRequestParts<S> for FirsthandSenderUser
where
    Arc<AppState>: FromRef<S>,
    S: Send + Sync,
{
    type Rejection = AuthError;

    async fn from_request_parts(parts: &mut Parts, state: &S) -> Result<Self, Self::Rejection> {
        let auth = AuthenticatedUser::from_request_parts(parts, state).await?;
        if auth.can_send_firsthand || auth.is_admin {
            Ok(FirsthandSenderUser(auth))
        } else {
            Err(AuthError::Forbidden)
        }
    }
}
use ax_extract_from_ref::FromRef;
mod ax_extract_from_ref {
    pub trait FromRef<T> {
        fn from_ref(input: &T) -> Self;
    }
    impl<T: Clone> FromRef<T> for T {
        fn from_ref(input: &T) -> Self {
            input.clone()
        }
    }
}

async fn crawler_filter(
    req: axum::extract::Request,
    next: axum::middleware::Next,
) -> Result<axum::response::Response, axum::http::StatusCode> {
    let path = req.uri().path().to_lowercase();

    // Patterns that indicate automated scanning
    let forbidden = [
        "wp-admin",
        "wordpress",
        ".php",
        "phpmyadmin",
        "config",
        ".env",
        ".git",
        "setup-config",
    ];

    if forbidden.iter().any(|pattern| path.contains(pattern)) {
        // Silently block without running any other app logic
        tracing::warn!("Blocked crawler attempt: {}", path);
        return Err(axum::http::StatusCode::FORBIDDEN);
    }

    Ok(next.run(req).await)
}

async fn htmx_js_handler() -> impl IntoResponse {
    (
        [
            (axum::http::header::CONTENT_TYPE, "application/javascript"),
            (
                axum::http::header::CACHE_CONTROL,
                "public, max-age=31536000, immutable",
            ),
        ],
        HTMX_SRC,
    )
}

async fn sse_js_handler() -> impl IntoResponse {
    (
        [
            (axum::http::header::CONTENT_TYPE, "application/javascript"),
            (
                axum::http::header::CACHE_CONTROL,
                "public, max-age=31536000, immutable",
            ),
        ],
        SSE_SRC,
    )
}

pub async fn create_app(state: AppState) -> Router {
    use tower_http::compression::CompressionLayer;

    let logging_layer = TraceLayer::new_for_http()
        .make_span_with(|request: &axum::http::Request<Body>| {
            let remote_addr = extract_client_ip(request.headers(), request.extensions());

            tracing::info_span!(
                "http_request",
                method = %request.method(),
                uri = %request.uri(),
                remote_addr = %remote_addr,
            )
        })
        .on_request(|request: &axum::http::Request<_>, _span: &tracing::Span| {
            let remote_addr = extract_client_ip(request.headers(), request.extensions());

            tracing::info!(
                "--> {} {} from {}",
                request.method(),
                request.uri().path(),
                remote_addr
            );
        })
        .on_response(
            |response: &axum::http::Response<_>,
             latency: std::time::Duration,
             _span: &tracing::Span| {
                tracing::info!("<-- {} in {:?}", response.status(), latency);
            },
        );

    let cookie_domain = crate::config::get_config("COOKIE_DOMAIN", "");
    let secure_cookies = crate::config::get_config("SECURE_COOKIES", "true") == "true";

    use tower_governor::{
        GovernorLayer, governor::GovernorConfigBuilder, key_extractor::SmartIpKeyExtractor,
    };
    let governor_conf = Arc::new(
        GovernorConfigBuilder::default()
            .per_second(2)
            .burst_size(5)
            .key_extractor(SmartIpKeyExtractor)
            .finish()
            .unwrap(),
    );

    let app = Router::new()
        .route("/", get(root))
        .route("/health", get(health_check))
        .route(
            "/emails/batch-delete",
            post(dashboard::batch_delete_emails_handler),
        )
        .route(
            "/emails/batch-delete-confirm",
            post(dashboard::batch_delete_emails_confirm_handler),
        )
        .route(
            "/emails/{email_id}",
            get(get_email).delete(dashboard::delete_email_handler),
        )
        .route(
            "/emails/{email_id}/reply",
            post(replies::submit_reply_handler),
        )
        .route(
            "/login",
            get(handlers::login_page)
                .post(handlers::login_handler)
                .layer(GovernorLayer::new(governor_conf.clone())),
        )
        .route(
            "/auth/password-toggle",
            post(handlers::password_toggle_handler),
        )
        .route("/logout", post(handlers::logout_handler))
        .route(
            "/register",
            get(handlers::register_page)
                .post(handlers::register_handler)
                .layer(GovernorLayer::new(governor_conf.clone())),
        )
        .route("/dashboard", get(dashboard::dashboard_handler))
        .route("/api/sse/dashboard", get(dashboard::dashboard_sse_handler))
        .route(
            "/api-keys",
            get(handlers::api_keys_page).post(handlers::create_api_key_handler),
        )
        .route("/api-keys/{id}", delete(handlers::delete_api_key_handler))
        .route(
            "/domains",
            get(handlers::domains_page).post(handlers::create_domain_handler),
        )
        .route("/domains/{id}", delete(handlers::delete_domain_handler))
        .route("/domains/{id}/rotate-dkim", post(dkim::rotate_dkim_handler))
        .route("/domains/{id}/verify-dkim", post(dkim::verify_dkim_handler))
        .route(
            "/domains/{id}/cancel-dkim-rotation",
            post(dkim::cancel_dkim_rotation_handler),
        )
        .route("/domains/{id}/dkim-modal", get(dkim::dkim_modal_handler))
        .route("/admin/users", get(admin_handlers::admin_users_handler))
        .route(
            "/admin/users/{id}/bypass_limit",
            post(admin_handlers::toggle_bypass_limit_handler),
        )
        .route(
            "/admin/users/{id}/disable_autoclean",
            post(admin_handlers::toggle_disable_autoclean_handler),
        )
        .route(
            "/admin/users/{id}/can_send_firsthand",
            post(admin_handlers::toggle_can_send_firsthand_handler),
        )
        .route(
            "/aliases",
            get(handlers::aliases_page).post(alias_api::create_alias_handler),
        )
        .route(
            "/aliases/suggestions",
            get(handlers::alias_suggestions_handler),
        )
        .route(
            "/aliases/{alias_id}",
            delete(handlers::delete_alias_handler),
        )
        .route(
            "/aliases/{alias_id}/delete-confirm",
            get(handlers::delete_alias_confirm_handler),
        )
        .route(
            "/emails/{email_id}/delete-confirm",
            get(dashboard::delete_email_confirm_handler),
        )
        .route(
            "/dashboard/email/{email_id}/attachment/{attachment_id}",
            get(dashboard::download_attachment_handler),
        )
        .route(
            "/dashboard/email/{email_id}/inline/{*content_id}",
            get(dashboard::inline_image_handler),
        )
        .route(
            "/aliases/{id}/toggle-forward",
            post(handlers::toggle_alias_forward_handler),
        )
        .nest(
            "/api/v1",
            Router::new()
                .route("/emails", get(api::list_emails_handler))
                .route("/emails/compose", get(send_email::compose_modal_handler))
                .route("/emails/drafts", post(send_email::save_draft_handler))
                .route("/emails/send", post(send_email::submit_email_handler))
                .route("/emails/{id}/reply", post(api::submit_reply_api))
                .route("/aliases/{id}/toggle", post(api::toggle_alias_forward_api)),
        )
        .route("/static/htmx.min.js", get(htmx_js_handler))
        .route("/static/sse.js", get(sse_js_handler))
        .layer(axum::middleware::from_fn(crawler_filter))
        .layer(CompressionLayer::new());

    // Conditionally apply the correct session store layer depending on the DbPool type
    let app = match state.db.clone() {
        DbPool::Postgres(pool) => {
            let session_store = PostgresStore::new(pool);
            session_store.migrate().await.unwrap();
            let mut session_layer = SessionManagerLayer::new(session_store)
                .with_secure(secure_cookies)
                .with_same_site(tower_sessions::cookie::SameSite::Lax)
                .with_expiry(Expiry::OnInactivity(time::Duration::days(7)));
            if !cookie_domain.is_empty() {
                session_layer = session_layer.with_domain(cookie_domain);
            }
            app.layer(session_layer)
        }
        DbPool::Sqlite(pool) => {
            let session_store = tower_sessions_sqlx_store::SqliteStore::new(pool);
            session_store.migrate().await.unwrap();
            let mut session_layer = SessionManagerLayer::new(session_store)
                .with_secure(secure_cookies)
                .with_same_site(tower_sessions::cookie::SameSite::Lax)
                .with_expiry(Expiry::OnInactivity(time::Duration::days(7)));
            if !cookie_domain.is_empty() {
                session_layer = session_layer.with_domain(cookie_domain);
            }
            app.layer(session_layer)
        }
    };

    app.layer(logging_layer).with_state(Arc::new(state))
}

pub async fn run_web_server(
    addr: &str,
    state: AppState,
    tls_acceptor: Option<crate::inbound::acceptor::HotReloadAcceptor>,
) -> anyhow::Result<()> {
    let app = create_app(state.clone()).await;

    if let Some(ref auto_tls) = state.config.auto_tls {
        autotls::run_auto_tls_web_server(app, auto_tls).await?;
    } else if let Some(acceptor) = tls_acceptor {
        // 1. Wrap the config
        let config = RustlsConfig::from_config(
            acceptor
                .config()
                .expect("Manual TLS mode requires active certificates on boot"),
        );

        // 2. Explicitly parse the address
        let socket_addr: SocketAddr = addr.parse()?;
        // HTTPS MODE
        println!("🚀 Web server running at https://{}", addr);
        // 3. Bind and serve
        axum_server::bind_rustls(socket_addr, config)
            .serve(app.into_make_service_with_connect_info::<SocketAddr>())
            .await?;
    } else {
        // HTTP MODE (Fallback)
        println!("Web server running at http://{}", addr);
        let listener = tokio::net::TcpListener::bind(addr).await?;
        axum::serve(
            listener,
            app.into_make_service_with_connect_info::<SocketAddr>(),
        )
        .await?;
    }
    Ok(())
}

async fn root(session: tower_sessions::Session) -> impl axum::response::IntoResponse {
    use axum::response::Redirect;
    if session
        .get::<Uuid>("user_id")
        .await
        .ok()
        .flatten()
        .is_some()
    {
        Redirect::to("/dashboard").into_response()
    } else {
        Redirect::to("/login").into_response()
    }
}

async fn health_check(State(state): State<Arc<AppState>>) -> Json<Value> {
    Json(json!({
        "status": "ok",
        "storage": state.storage_dir
    }))
}

async fn get_email(
    locale: crate::web::i18n::Locale,
    State(state): State<Arc<AppState>>,
    Path(email_id): Path<Uuid>,
    user: AuthenticatedUser,
) -> Response {
    use crate::db::AnyEmail;
    use crate::web::handlers::EmailDetailTemplate;
    use axum::response::IntoResponse;
    use mail_parser::MessageParser;

    let email = match crate::db::get_any_email(&state.db, email_id, user.user_id).await {
        Ok(Some(e)) => e,
        Ok(None) => {
            return Response::builder()
                .status(StatusCode::NOT_FOUND)
                .body(Body::from("Email not found"))
                .unwrap();
        }
        Err(e) => {
            tracing::error!("Error reading email: {}", e);
            return Response::builder()
                .status(StatusCode::INTERNAL_SERVER_ERROR)
                .body(Body::from("Server error"))
                .unwrap();
        }
    };

    match email {
        AnyEmail::Received(email) => {
            // Mark as viewed
            let _ = crate::db::mark_email_as_viewed(&state.db, email_id, user.user_id).await;

            let path = state.storage_dir.join(format!("{}.eml", email.body_key));
            match tokio::fs::read(&path).await {
                Ok(bytes) => {
                    let message = MessageParser::default().parse(&bytes).unwrap();
                    let sender = message
                        .from()
                        .and_then(|f| f.first())
                        .map(|a| a.address().unwrap_or("Unknown"))
                        .unwrap_or("Unknown")
                        .to_string();
                    let subject = message.subject().unwrap_or("No Subject").to_string();

                    // Prefer HTML body, fall back to text
                    let raw_body = message
                        .body_html(0)
                        .or_else(|| message.body_text(0))
                        .unwrap_or_default();

                    let body = email_body::sanitize_email_body(&raw_body, email_id);
                    let date = message.date().map(|d| d.to_rfc822()).unwrap_or_default();

                    let alias_address =
                        crate::db::get_alias_details_for_email(&state.db, email_id, user.user_id)
                            .await
                            .ok()
                            .flatten()
                            .map(|(s, d)| format!("{}@{}", s, d))
                            .unwrap_or_else(|| "Unknown".to_string());

                    // --- Threading Logic ---
                    let replies = fetch_thread_messages(&state, email_id).await;

                    let attachments = get_attachments_for_email(&state.db, email_id)
                        .await
                        .unwrap_or_default();

                    EmailDetailTemplate {
                        id: email_id,
                        sender,
                        alias_address,
                        subject,
                        body,
                        date,
                        is_forwarded: email.forwarded,
                        is_outbound: false,
                        locale,
                        replies,
                        attachments,
                    }
                    .into_response()
                }
                Err(e) => {
                    tracing::error!("Error accessing the file {}: {}", path.display(), e);
                    Response::builder()
                        .status(StatusCode::NOT_FOUND)
                        .body(Body::from("Email content not found"))
                        .unwrap()
                }
            }
        }
        AnyEmail::Sent(email) => {
            let mut path = state.storage_dir.join(format!("{}.eml", email.body_key));
            if !path.exists() {
                path = state.storage_dir.join(email.body_key.to_string());
            }

            match tokio::fs::read(&path).await {
                Ok(bytes) => {
                    let message = MessageParser::default().parse(&bytes).unwrap();

                    // For outbound emails, "sender" is the alias address, "alias_address" is the recipient
                    let sender = email.alias_address;
                    let recipient = email.to_address;
                    let subject = email.subject;
                    let date =
                        email.sent_at.unwrap_or(email.created_at).to_string()[..16].to_string();

                    // Prefer HTML body, fall back to text
                    let raw_body = message
                        .body_html(0)
                        .or_else(|| message.body_text(0))
                        .map(|s| s.to_string())
                        .unwrap_or_else(|| String::from_utf8_lossy(&bytes).to_string());

                    let body = email_body::sanitize_email_body(&raw_body, email_id);

                    EmailDetailTemplate {
                        id: email_id,
                        sender,
                        alias_address: recipient,
                        subject,
                        body,
                        date,
                        is_forwarded: false,
                        is_outbound: true,
                        locale,
                        replies: vec![],
                        attachments: vec![],
                    }
                    .into_response()
                }
                Err(e) => {
                    tracing::error!(
                        "Error accessing the sent email file {}: {}",
                        path.display(),
                        e
                    );
                    Response::builder()
                        .status(StatusCode::NOT_FOUND)
                        .body(Body::from("Email content not found"))
                        .unwrap()
                }
            }
        }
    }
}



async fn fetch_thread_messages(state: &AppState, email_id: Uuid) -> Vec<ThreadMessage> {
    // 1. Fetch outbound replies
    let outbound_replies = crate::db::replies::get_replies_for_email(&state.db, email_id)
        .await
        .unwrap_or_default()
        .into_iter()
        .map(|r| ThreadMessage::Outbound {
            id: r.id,
            body_text: r.body_text,
            sent_at: r.sent_at,
        })
        .collect::<Vec<_>>();

    // 2. Fetch inbound child emails
    let mut inbound_replies = Vec::new();
    let child_emails = crate::db::get_child_emails(&state.db, email_id)
        .await
        .unwrap_or_default();

    for child in child_emails {
        let child_path = state.storage_dir.join(format!("{}.eml", child.body_key));
        if let Ok(child_bytes) = tokio::fs::read(&child_path).await {
            let child_msg = MessageParser::default().parse(&child_bytes).unwrap();
            let child_raw_body = child_msg
                .body_html(0)
                .or_else(|| child_msg.body_text(0))
                .unwrap_or_default();

            let mut child_body = email_body::sanitize_email_body(&child_raw_body, child.id);

            // Lazy-load swapping for inbound replies
            if child_body.contains(" src=") {
                child_body = child_body.replace(" src=", " data-remote-src=");
            }

            inbound_replies.push(ThreadMessage::Inbound {
                id: child.id,
                sender: child.sender_email,
                body_text: child_body,
                sent_at: child.received_at,
            });
        }
    }

    merge_thread_messages(inbound_replies, outbound_replies)
}

pub fn extract_client_ip(
    headers: &axum::http::HeaderMap,
    extensions: &axum::http::Extensions,
) -> String {
    let x_forwarded_for = headers
        .get("x-forwarded-for")
        .and_then(|val| val.to_str().ok())
        .and_then(|s| s.split(',').next())
        .map(|s| s.trim().to_string());

    x_forwarded_for.unwrap_or_else(|| {
        extensions
            .get::<axum::extract::ConnectInfo<std::net::SocketAddr>>()
            .map(|ci| ci.0.to_string())
            .unwrap_or_else(|| "unknown".into())
    })
}

pub fn extract_domain(email: &str) -> Option<&str> {
    let parts: Vec<&str> = email.split('@').collect();
    if parts.len() != 2 {
        return None;
    }
    let domain = parts[1].trim();
    if domain.is_empty() {
        return None;
    }
    Some(domain)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_merge_thread_messages() {
        use time::OffsetDateTime;
        let now = OffsetDateTime::now_utc();
        let m1 = ThreadMessage::Inbound {
            id: Uuid::new_v4(),
            sender: "a".into(),
            body_text: "1".into(),
            sent_at: now,
        };
        let m2 = ThreadMessage::Outbound {
            id: Uuid::new_v4(),
            body_text: "2".into(),
            sent_at: now + time::Duration::minutes(1),
        };
        let m3 = ThreadMessage::Inbound {
            id: Uuid::new_v4(),
            sender: "b".into(),
            body_text: "3".into(),
            sent_at: now + time::Duration::minutes(2),
        };

        let merged = merge_thread_messages(vec![m3, m1], vec![m2]);
        assert_eq!(merged.len(), 3);
        assert!(matches!(merged[0], ThreadMessage::Inbound { .. }));
        assert!(matches!(merged[1], ThreadMessage::Outbound { .. }));
        assert!(matches!(merged[2], ThreadMessage::Inbound { .. }));
    }

    #[test]
    fn test_received_email_serialization() {
        let id = Uuid::new_v4();
        let alias_id = Some(Uuid::new_v4());
        let user_id = Uuid::new_v4();
        let now = time::OffsetDateTime::now_utc();
        let body_key = Uuid::new_v4();

        let email = ReceivedEmail {
            id,
            alias_id,
            alias_address: None,
            user_id,
            sender_email: "sender@test.com".into(),
            subject: "test".into(),
            body_key,
            received_at: now,
            viewed: false,
            forwarded: false,
            message_id: None,
            thread_id: None,
            last_activity_at: now,
            has_attachments: false,
        };

        let json = serde_json::to_value(&email).unwrap();
        assert_eq!(json["id"], id.to_string());
        assert_eq!(json["alias_id"], alias_id.unwrap().to_string());
        assert_eq!(json["user_id"], user_id.to_string());
    }

    #[test]
    fn test_extract_domain_with_subdomains() {
        let cases = vec![
            ("user@example.com", Some("example.com")),
            ("user@sub.example.com", Some("sub.example.com")),
            (
                "user@deep.sub.example.co.uk",
                Some("deep.sub.example.co.uk"),
            ),
            ("  user@trimmed.com  ", Some("trimmed.com")),
            ("invalid-email", None),
            ("too@many@parts.com", None),
            ("empty@ ", None),
        ];

        for (input, expected) in cases {
            assert_eq!(
                extract_domain(input.trim()),
                expected,
                "Failed on input: {}",
                input
            );
        }
    }

    #[test]
    fn test_validate_csrf_token() {
        use axum::http::Method;
        let p_get = Method::GET;
        let p_post = Method::POST;

        // Valid State-Changing
        assert!(validate_csrf_token(&p_post, Some("token123"), Some("token123")).is_ok());

        // Missing Tokens (POST) should fail
        assert_eq!(
            validate_csrf_token(&p_post, None, Some("token123")),
            Err(AuthError::CsrfVerificationFailed)
        );
        assert_eq!(
            validate_csrf_token(&p_post, Some("token123"), None),
            Err(AuthError::CsrfVerificationFailed)
        );

        // Mismatched Tokens should fail
        assert_eq!(
            validate_csrf_token(&p_post, Some("evil_token"), Some("token123")),
            Err(AuthError::CsrfVerificationFailed)
        );

        // Empty token should fail
        assert_eq!(
            validate_csrf_token(&p_post, Some(""), Some("")),
            Err(AuthError::CsrfVerificationFailed)
        );

        // Safe Methods shouldn't care about tokens
        assert!(validate_csrf_token(&p_get, None, None).is_ok());
        assert!(validate_csrf_token(&p_get, Some("a"), Some("b")).is_ok());
    }

    #[test]
    fn test_can_bypass_alias_limit() {
        let u1 = AuthenticatedUser {
            user_id: uuid::Uuid::new_v4(),
            is_admin: false,
            bypass_alias_limit: false,
            can_send_firsthand: false,
        };
        assert!(!u1.can_bypass_alias_limit());

        let u2 = AuthenticatedUser {
            user_id: uuid::Uuid::new_v4(),
            is_admin: true,
            bypass_alias_limit: false,
            can_send_firsthand: false,
        };
        assert!(u2.can_bypass_alias_limit());

        let u3 = AuthenticatedUser {
            user_id: uuid::Uuid::new_v4(),
            is_admin: false,
            bypass_alias_limit: true,
            can_send_firsthand: false,
        };
        assert!(u3.can_bypass_alias_limit());

        let u4 = AuthenticatedUser {
            user_id: uuid::Uuid::new_v4(),
            is_admin: true,
            bypass_alias_limit: true,
            can_send_firsthand: false,
        };
        assert!(u4.can_bypass_alias_limit());
    }
}
