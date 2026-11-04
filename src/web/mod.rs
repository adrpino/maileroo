pub mod admin_handlers;
pub mod alias_api;
pub mod api;
pub mod api_auth;
pub mod attachments_form;
pub mod auth;
pub mod autotls;
pub mod dashboard;
pub mod dkim;
pub mod email_body;
pub mod handlers;
pub mod i18n;
pub mod labels;
pub mod names;
pub mod replies;
pub mod send_email;
pub mod session;

use crate::config::get_config;
use crate::db::{DbPool, get_user_by_id};
use axum::extract::{DefaultBodyLimit, FromRequestParts, State};
use axum::{
    Json, Router,
    body::Body,
    extract::Path,
    http::{StatusCode, request::Parts},
    response::{Html, IntoResponse, Response},
    routing::{delete, get, post},
};
use send_email::MAX_UPLOAD_REQUEST_BYTES;
use serde_json::{Value, json};
use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::sync::{Arc, Once};
use uuid::Uuid;

/// Renders an Askama template into an HTML response, logging internal rendering
/// errors instead of exposing them to the client.
pub fn render_template<T: askama::Template>(template: &T) -> Response {
    match template.render() {
        Ok(html) => Html(html).into_response(),
        Err(err) => {
            tracing::error!("Failed to render template: {}", err);
            (StatusCode::INTERNAL_SERVER_ERROR, "Internal server error").into_response()
        }
    }
}

macro_rules! impl_into_response_for_template {
    ($($ty:ty),+ $(,)?) => {
        $(
            impl axum::response::IntoResponse for $ty {
                fn into_response(self) -> axum::response::Response {
                    $crate::web::render_template(&self)
                }
            }
        )+
    };
}
pub(crate) use impl_into_response_for_template;

/// Global configuration for session and CSRF cookie expiration (7 days in seconds).
pub const SESSION_MAX_AGE_SECONDS: i64 = 7 * 24 * 60 * 60;

use crate::db::attachments::get_attachments_for_email;
use crate::dns::DnsScanner;
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
    pub filter_engine: crate::filter::FilterEngine,
    pub backfill_engine: crate::filter::BackfillEngine,
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
    pub is_favorite: bool,
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
        attachments: Vec<crate::db::attachments::ReplyAttachmentRow>,
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

/// Pure function for constant-time byte slice equality comparison.
pub fn constant_time_eq(a: &[u8], b: &[u8]) -> bool {
    if a.len() != b.len() {
        return false;
    }
    let mut diff = 0u8;
    for (&x, &y) in a.iter().zip(b.iter()) {
        diff |= x ^ y;
    }
    diff == 0
}

/// Pure function to validate a CSRF token in constant time.
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
        (Some(h_val), Some(s_val))
            if !s_val.is_empty() && constant_time_eq(h_val.as_bytes(), s_val.as_bytes()) =>
        {
            Ok(())
        }
        _ => Err(AuthError::CsrfVerificationFailed),
    }
}

/// Pure function that explains *why* a CSRF check failed, for logging.
///
/// The distinction matters operationally: a missing session token means the session
/// was created by a flow that never issued one, which no amount of client-side retry
/// will fix, whereas a missing header usually means the cookie never reached the client.
pub fn csrf_failure_reason(header_present: bool, session_present: bool) -> &'static str {
    match (header_present, session_present) {
        (false, false) => {
            "no token in session and none sent by client; session was created without \
             a CSRF token (e.g. registration) - user must log in again"
        }
        (true, false) => {
            "session holds no CSRF token; session was created without one or has been \
             cleared - user must log in again"
        }
        (false, true) => {
            "client sent no X-CSRF-Token header; csrf_token cookie missing, blocked, or \
             scripts did not run"
        }
        (true, true) => "token mismatch; stale cookie from a previous session",
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
        if let Err(err) = validate_csrf_token(&parts.method, header_token, session_token.as_deref())
        {
            let user_agent = parts
                .headers
                .get(axum::http::header::USER_AGENT)
                .and_then(|h| h.to_str().ok())
                .unwrap_or("<none>");

            tracing::warn!(
                user_id = %user_id,
                method = %parts.method,
                path = %parts.uri.path(),
                header_token_present = header_token.is_some(),
                session_token_present = session_token.is_some(),
                user_agent = %user_agent,
                "403 CSRF verification failed: {}",
                csrf_failure_reason(header_token.is_some(), session_token.is_some())
            );

            return Err(err);
        }

        // Load user permissions fresh from the database on each request so
        // administrative permission changes apply immediately without re-login.
        let app_state = Arc::<AppState>::from_ref(state);
        let user = get_user_by_id(&app_state.db, user_id)
            .await
            .map_err(|_| AuthError::SessionError)?
            .ok_or(AuthError::UserNotFound)?;

        Ok(AuthenticatedUser {
            user_id,
            is_admin: user.is_admin,
            bypass_alias_limit: user.bypass_alias_limit,
            can_send_firsthand: user.can_send_firsthand,
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
            tracing::warn!(
                user_id = %auth.user_id,
                method = %parts.method,
                path = %parts.uri.path(),
                "403 admin access required"
            );
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
            tracing::warn!(
                user_id = %auth.user_id,
                method = %parts.method,
                path = %parts.uri.path(),
                "403 firsthand sending not permitted for this user"
            );
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
        tracing::warn!("403 blocked crawler attempt: {}", path);
        return Err(axum::http::StatusCode::FORBIDDEN);
    }

    Ok(next.run(req).await)
}

async fn htmx_js_handler() -> impl IntoResponse {
    (
        [
            (axum::http::header::CONTENT_TYPE, "application/javascript"),
            (axum::http::header::CACHE_CONTROL, "public, max-age=86400"),
        ],
        HTMX_SRC,
    )
}

async fn sse_js_handler() -> impl IntoResponse {
    (
        [
            (axum::http::header::CONTENT_TYPE, "application/javascript"),
            (axum::http::header::CACHE_CONTROL, "public, max-age=86400"),
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

    let cookie_domain = get_config("COOKIE_DOMAIN", "");
    let secure_cookies = get_config("SECURE_COOKIES", "true") == "true";
    let trusted_proxy = parse_trusted_proxy(&get_config("TRUSTED_PROXY", ""));

    use tower_governor::{GovernorLayer, governor::GovernorConfigBuilder};
    let governor_conf = Arc::new(
        GovernorConfigBuilder::default()
            .per_second(2)
            .burst_size(5)
            .key_extractor(TrustedProxyIpKeyExtractor { trusted_proxy })
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
            "/emails/batch-mark-unread",
            post(dashboard::batch_mark_unread_handler),
        )
        .route(
            "/emails/{email_id}",
            get(get_email).delete(dashboard::delete_email_handler),
        )
        .route(
            "/emails/{email_id}/toggle-favorite",
            post(dashboard::toggle_favorite_handler),
        )
        .route(
            "/emails/{email_id}/mark-unread",
            post(dashboard::mark_email_unread_handler),
        )
        .route(
            "/emails/{email_id}/reply",
            post(replies::submit_reply_handler),
        )
        .route(
            "/emails/replies/{reply_id}/attachment/{attachment_id}",
            get(replies::download_reply_attachment_handler),
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
                .route(
                    "/emails/compose-send",
                    post(send_email::submit_email_handler)
                        .layer(DefaultBodyLimit::max(MAX_UPLOAD_REQUEST_BYTES)),
                )
                .route(
                    "/emails/send",
                    post(api::send_email_api)
                        .layer(DefaultBodyLimit::max(MAX_UPLOAD_REQUEST_BYTES)),
                )
                .route("/emails/{id}/reply", post(api::submit_reply_api))
                .route("/emails/{id}", get(api::get_email_api))
                .route("/aliases", get(api::list_aliases_api))
                .route("/aliases/{id}/toggle", post(api::toggle_alias_forward_api))
                .route("/labels/modal", get(labels::get_labels_modal_handler))
                .route("/labels", post(labels::create_label_handler))
                .route("/labels/{id}", delete(labels::delete_label_handler))
                .route("/filters", post(labels::create_filter_handler))
                .route("/filters/{id}", delete(labels::delete_filter_handler))
                .route("/emails/{id}/labels", post(labels::add_email_label_handler))
                .route(
                    "/emails/{id}/labels/{label_id}",
                    delete(labels::remove_email_label_handler),
                ),
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
                .with_expiry(Expiry::OnInactivity(time::Duration::seconds(
                    SESSION_MAX_AGE_SECONDS,
                )));
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
                .with_expiry(Expiry::OnInactivity(time::Duration::seconds(
                    SESSION_MAX_AGE_SECONDS,
                )));
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
        tracing::info!("Web server running at https://{}", addr);
        // 3. Bind and serve
        axum_server::bind_rustls(socket_addr, config)
            .serve(app.into_make_service_with_connect_info::<SocketAddr>())
            .await?;
    } else {
        // HTTP MODE (Fallback)
        tracing::info!("Web server running at http://{}", addr);
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

                    let labels = crate::db::labels::get_labels_for_email(&state.db, email_id)
                        .await
                        .unwrap_or_default();
                    let all_user_labels =
                        crate::db::labels::get_labels_by_user(&state.db, user.user_id)
                            .await
                            .unwrap_or_default();

                    EmailDetailTemplate {
                        id: email_id,
                        email_id,
                        sender,
                        alias_address,
                        subject,
                        body,
                        date,
                        is_forwarded: email.forwarded,
                        is_outbound: false,
                        is_sent: false,
                        locale,
                        replies,
                        attachments,
                        labels,
                        all_user_labels,
                        is_favorite: email.is_favorite,
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

                    let attachments = get_attachments_for_email(&state.db, email_id)
                        .await
                        .unwrap_or_default();

                    EmailDetailTemplate {
                        id: email_id,
                        email_id,
                        sender,
                        alias_address: recipient,
                        subject,
                        body,
                        date,
                        is_forwarded: false,
                        is_outbound: true,
                        is_sent: true,
                        locale,
                        replies: vec![],
                        attachments,
                        labels: vec![],
                        all_user_labels: vec![],
                        is_favorite: email.is_favorite,
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
    // 1. Fetch outbound replies along with their attachments
    let mut outbound_replies = Vec::new();
    for r in crate::db::replies::get_replies_for_email(&state.db, email_id)
        .await
        .unwrap_or_default()
    {
        let attachments = crate::db::attachments::get_attachments_for_reply(&state.db, r.id)
            .await
            .unwrap_or_default();
        outbound_replies.push(ThreadMessage::Outbound {
            id: r.id,
            body_text: r.body_text,
            sent_at: r.sent_at,
            attachments,
        });
    }

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

/// Parses the optional `TRUSTED_PROXY` IP address from configuration.
pub fn parse_trusted_proxy(raw: &str) -> Option<IpAddr> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        None
    } else {
        trimmed.parse().ok()
    }
}

/// Pure function that resolves the effective client IP address.
///
/// `X-Forwarded-For` is trusted **only** when `peer_ip` matches `trusted_proxy`,
/// and even then only its **last** entry (the one appended by the trusted proxy)
/// is used. Otherwise `peer_ip` is returned directly.
pub fn resolve_client_ip(
    peer_ip: Option<IpAddr>,
    x_forwarded_for: Option<&str>,
    trusted_proxy: Option<IpAddr>,
) -> Option<IpAddr> {
    let peer = peer_ip?;
    if trusted_proxy == Some(peer)
        && let Some(xff) = x_forwarded_for
        && let Some(last_hop) = xff.rsplit(',').next()
        && let Ok(forwarded_ip) = last_hop.trim().parse::<IpAddr>()
    {
        return Some(forwarded_ip);
    }
    Some(peer)
}

#[derive(Clone, Copy, Debug)]
pub struct TrustedProxyIpKeyExtractor {
    pub trusted_proxy: Option<IpAddr>,
}

impl tower_governor::key_extractor::KeyExtractor for TrustedProxyIpKeyExtractor {
    type Key = IpAddr;

    fn extract<T>(
        &self,
        req: &axum::http::Request<T>,
    ) -> Result<Self::Key, tower_governor::GovernorError> {
        let peer_ip = req
            .extensions()
            .get::<axum::extract::ConnectInfo<SocketAddr>>()
            .map(|ci| ci.0.ip());
        let xff = req
            .headers()
            .get("x-forwarded-for")
            .and_then(|val| val.to_str().ok());

        let resolved = resolve_client_ip(peer_ip, xff, self.trusted_proxy);
        if resolved.is_none() {
            MISSING_CONNECT_INFO_WARNING.call_once(|| {
                tracing::warn!(
                    "Rate limiter could not resolve the client IP because ConnectInfo is \
                     missing; all clients share a single rate-limit bucket. Serve the router \
                     with `into_make_service_with_connect_info::<SocketAddr>()`."
                );
            });
        }
        Ok(rate_limit_key(resolved))
    }
}

/// Ensures the missing-`ConnectInfo` warning is logged only once per process.
static MISSING_CONNECT_INFO_WARNING: Once = Once::new();

/// Key shared by every client when the peer address cannot be determined.
pub const FALLBACK_RATE_LIMIT_KEY: IpAddr = IpAddr::V4(Ipv4Addr::UNSPECIFIED);

/// Pure function mapping a resolved client IP to its rate-limit key.
///
/// When the client IP is unknown (e.g. the router is served without
/// `ConnectInfo`), all requests share `FALLBACK_RATE_LIMIT_KEY` so that
/// rate limiting degrades gracefully instead of failing every request.
pub fn rate_limit_key(resolved: Option<IpAddr>) -> IpAddr {
    resolved.unwrap_or(FALLBACK_RATE_LIMIT_KEY)
}

pub fn extract_client_ip(
    headers: &axum::http::HeaderMap,
    extensions: &axum::http::Extensions,
) -> String {
    let peer_ip = extensions
        .get::<axum::extract::ConnectInfo<SocketAddr>>()
        .map(|ci| ci.0.ip());
    let xff = headers
        .get("x-forwarded-for")
        .and_then(|val| val.to_str().ok());
    let trusted_proxy = parse_trusted_proxy(&get_config("TRUSTED_PROXY", ""));

    resolve_client_ip(peer_ip, xff, trusted_proxy)
        .map(|ip| ip.to_string())
        .unwrap_or_else(|| "unknown".into())
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
            attachments: vec![],
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
        let alias_id = Uuid::new_v4();
        let user_id = Uuid::new_v4();
        let now = time::OffsetDateTime::now_utc();
        let body_key = Uuid::new_v4();

        let email = ReceivedEmail {
            id,
            alias_id: Some(alias_id),
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
            is_favorite: false,
        };

        let json = serde_json::to_value(&email).unwrap();
        assert_eq!(json["id"], id.to_string());
        assert_eq!(json["alias_id"], alias_id.to_string());
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
    fn test_csrf_failure_reason_distinguishes_causes() {
        // Session never had a token: the client cannot fix this by retrying.
        assert!(csrf_failure_reason(false, false).contains("must log in again"));
        assert!(csrf_failure_reason(true, false).contains("must log in again"));

        // Session has a token but the client did not send one back.
        assert!(csrf_failure_reason(false, true).contains("X-CSRF-Token"));

        // Both present means they simply did not match.
        assert!(csrf_failure_reason(true, true).contains("mismatch"));
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

    #[test]
    fn test_constant_time_eq() {
        assert!(constant_time_eq(b"csrf-token-123", b"csrf-token-123"));
        assert!(!constant_time_eq(b"csrf-token-123", b"csrf-token-124"));
        assert!(!constant_time_eq(b"csrf-token-123", b"short"));
        assert!(constant_time_eq(b"", b""));
    }

    #[test]
    fn test_resolve_client_ip_trusted_proxy() {
        let peer: IpAddr = "198.51.100.5".parse().unwrap();
        let proxy: IpAddr = "10.0.0.1".parse().unwrap();
        let real_client: IpAddr = "203.0.113.99".parse().unwrap();

        // 1. No trusted proxy configured (default): X-Forwarded-For is ignored
        assert_eq!(
            resolve_client_ip(Some(peer), Some("203.0.113.99"), None),
            Some(peer)
        );

        // 2. Trusted proxy configured, but connection comes from an untrusted peer: XFF ignored
        assert_eq!(
            resolve_client_ip(Some(peer), Some("203.0.113.99"), Some(proxy)),
            Some(peer)
        );

        // 3. Connection comes from trusted proxy: only the LAST entry in X-Forwarded-For is used
        assert_eq!(
            resolve_client_ip(
                Some(proxy),
                Some("1.2.3.4, 5.6.7.8, 203.0.113.99"),
                Some(proxy)
            ),
            Some(real_client)
        );

        // 4. Connection comes from trusted proxy, but XFF is malformed: falls back to peer IP
        assert_eq!(
            resolve_client_ip(Some(proxy), Some("not-an-ip"), Some(proxy)),
            Some(proxy)
        );

        // 5. Missing ConnectInfo returns None
        assert_eq!(
            resolve_client_ip(None, Some("203.0.113.99"), Some(proxy)),
            None
        );
    }

    #[test]
    fn test_rate_limit_key_falls_back_when_ip_unknown() {
        let client: IpAddr = "203.0.113.99".parse().unwrap();
        assert_eq!(rate_limit_key(Some(client)), client);
        assert_eq!(rate_limit_key(None), FALLBACK_RATE_LIMIT_KEY);
    }

    #[test]
    fn test_key_extractor_without_connect_info_does_not_error() {
        use tower_governor::key_extractor::KeyExtractor;
        let extractor = TrustedProxyIpKeyExtractor {
            trusted_proxy: None,
        };
        let req = axum::http::Request::builder()
            .uri("/login")
            .body(())
            .unwrap();
        assert_eq!(extractor.extract(&req).unwrap(), FALLBACK_RATE_LIMIT_KEY);
    }

    #[tokio::test]
    async fn test_render_template_hides_error_details_on_500() {
        struct FailDisplay;
        impl std::fmt::Display for FailDisplay {
            fn fmt(&self, _f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                Err(std::fmt::Error)
            }
        }

        #[derive(askama::Template)]
        #[template(source = "{{ val }}", ext = "txt")]
        struct FailingTemplate {
            val: FailDisplay,
        }

        let response = render_template(&FailingTemplate { val: FailDisplay });
        assert_eq!(response.status(), StatusCode::INTERNAL_SERVER_ERROR);
        let body_bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        let body_text = String::from_utf8(body_bytes.to_vec()).unwrap();
        assert_eq!(body_text, "Internal server error");
        assert!(!body_text.contains("formatting error"));
    }
}
