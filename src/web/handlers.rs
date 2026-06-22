use crate::db::attachments::AttachmentRow;
use crate::db::{
    Alias, DbPool, Domain, delete_alias_by_id, get_aliases_by_user_id, get_domains,
    get_user_by_email, insert_user, update_last_login,
};
use crate::disposable_domains::is_disposable;
use crate::web::ThreadMessage;
use crate::web::auth::{hash_password, verify_password};
use crate::web::i18n::{Locale, Messages};
use crate::web::{AdminUser, AppState, AuthenticatedUser, extract_domain};
use axum::body::Body;
use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::response::Response;
use rand::SeedableRng;
use rand::rngs::SmallRng;
use regex::Regex;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use std::sync::Arc;
use std::sync::OnceLock;

use askama::Template;
use axum::{
    Form,
    response::{Html, IntoResponse},
};
use tower_sessions::Session;

static EMAIL_REGEX: OnceLock<Regex> = OnceLock::new();

pub const MAX_ALIASES_PER_USER: i64 = 4;

#[derive(Serialize, Deserialize)]
pub struct RegisterRequest {
    pub email: String,
    pub password: String,
}

#[derive(Serialize, Deserialize)]
pub struct LoginRequest {
    pub email: String,
    pub password: String,
}

fn is_valid_email(email: &str) -> bool {
    let re = EMAIL_REGEX.get_or_init(|| Regex::new(r"^[^\s@]+@[^\s@]+\.[^\s@]+$").unwrap());
    re.is_match(email)
}

pub async fn logout_handler(session: Session) -> Response {
    let _ = session.clear().await;

    let cookie_domain = crate::config::get_config("COOKIE_DOMAIN", "");
    let mut cookie = String::from("csrf_token=; Path=/; SameSite=Lax; Max-Age=0");
    if !cookie_domain.is_empty() {
        cookie.push_str(&format!("; Domain={}", cookie_domain));
    }
    if crate::config::get_config("SECURE_COOKIES", "true") == "true" {
        cookie.push_str("; Secure");
    }

    Response::builder()
        .status(StatusCode::OK)
        .header("HX-Redirect", "/login")
        .header(axum::http::header::SET_COOKIE, cookie)
        .body(Body::from("Logged out successfully"))
        .unwrap()
}

#[derive(Template)]
#[template(path = "login.html")]
pub struct LoginTemplate {
    pub error: Option<String>,
    pub locale: Locale,
}
impl IntoResponse for LoginTemplate {
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

#[derive(Template)]
#[template(path = "register.html")]
pub struct RegisterTemplate {
    pub error: Option<String>,
    pub locale: Locale,
}
impl IntoResponse for RegisterTemplate {
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

#[derive(Deserialize)]
pub struct PaginationParams {
    pub page: Option<i64>,
    pub alias: Option<String>,
    pub q: Option<String>,
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
    pub attachments: Vec<AttachmentRow>,
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

pub async fn login_page(locale: Locale) -> LoginTemplate {
    LoginTemplate {
        error: None,
        locale,
    }
}

pub async fn register_page(locale: Locale) -> impl IntoResponse {
    RegisterTemplate {
        error: None,
        locale,
    }
}

pub async fn login_handler(
    locale: Locale,
    session: Session,
    headers: axum::http::HeaderMap,
    extensions: axum::http::Extensions,
    State(state): State<Arc<AppState>>,
    Form(payload): Form<LoginRequest>,
) -> Response {
    if payload.password.len() > 128 {
        return LoginTemplate {
            error: Some(locale.invalid_credentials().to_string()),
            locale,
        }
        .into_response();
    }

    let user = match get_user_by_email(&state.db, &payload.email).await {
        Ok(Some(u)) => u,
        _ => {
            return LoginTemplate {
                error: Some(locale.invalid_credentials().to_string()),
                locale,
            }
            .into_response();
        }
    };

    if verify_password(&payload.password, &user.password_hash) {
        let _ = session.insert("user_id", user.id).await;
        let _ = session.insert("is_admin", user.is_admin).await;
        let _ = session
            .insert("bypass_alias_limit", user.bypass_alias_limit)
            .await;
        let _ = session
            .insert("can_send_firsthand", user.can_send_firsthand)
            .await;
        let _ = session.insert("user_data_loaded", true).await;

        let csrf_token = Uuid::new_v4().to_string();
        let _ = session.insert("csrf_token", &csrf_token).await;

        let client_ip = crate::web::extract_client_ip(&headers, &extensions);
        let _ = update_last_login(&state.db, user.id, Some(client_ip)).await;

        let mut response = Response::builder()
            .header("HX-Redirect", "/dashboard")
            .status(StatusCode::OK)
            .body(Body::empty())
            .unwrap();

        let cookie_domain = crate::config::get_config("COOKIE_DOMAIN", "");
        let mut cookie = format!("csrf_token={}; Path=/; SameSite=Lax", csrf_token);
        if !cookie_domain.is_empty() {
            cookie.push_str(&format!("; Domain={}", cookie_domain));
        }
        if crate::config::get_config("SECURE_COOKIES", "true") == "true" {
            cookie.push_str("; Secure");
        }

        response
            .headers_mut()
            .insert(axum::http::header::SET_COOKIE, cookie.parse().unwrap());

        return response;
    }

    LoginTemplate {
        error: Some(locale.invalid_credentials().to_string()),
        locale,
    }
    .into_response()
}

pub async fn register_handler(
    locale: Locale,
    session: Session,
    State(state): State<Arc<AppState>>,
    Form(payload): Form<RegisterRequest>,
) -> Response {
    let Some(domain) = extract_domain(&payload.email) else {
        return RegisterTemplate {
            error: Some(locale.error_invalid_email().to_string()),
            locale,
        }
        .into_response();
    };
    if is_disposable(domain) {
        return RegisterTemplate {
            error: Some(locale.error_invalid_domain().to_string()),
            locale,
        }
        .into_response();
    }
    if !state.dns_scanner.is_domain_deliverable(domain).await {
        return RegisterTemplate {
            error: Some(locale.error_invalid_domain().to_string()),
            locale,
        }
        .into_response();
    }
    if !is_valid_email(&payload.email) {
        return RegisterTemplate {
            error: Some(locale.error_invalid_email().to_string()),
            locale,
        }
        .into_response();
    }

    if payload.password.len() < 8 || payload.password.len() > 128 {
        return RegisterTemplate {
            error: Some("Password must be between 8 and 128 characters".to_string()),
            locale,
        }
        .into_response();
    }

    let hash = hash_password(&payload.password).unwrap();
    match insert_user(&state.db, &payload.email, &hash).await {
        Ok(user) => {
            let _ = session.insert("user_id", user.id).await;
            let _ = session.insert("is_admin", user.is_admin).await;
            let _ = session
                .insert("bypass_alias_limit", user.bypass_alias_limit)
                .await;
            let _ = session.insert("user_data_loaded", true).await;
            Response::builder()
                .header("HX-Redirect", "/dashboard")
                .status(StatusCode::OK)
                .body(Body::empty())
                .unwrap()
        }
        Err(_) => RegisterTemplate {
            error: Some(locale.error_email_taken().to_string()),
            locale,
        }
        .into_response(),
    }
}

pub struct DomainWithStatus {
    pub domain: Domain,
    pub dns_status: Option<crate::dns::DnsCheckResult>,
}

#[derive(Template)]
#[template(path = "domains.html")]
pub struct DomainsTemplate {
    pub domains: Vec<DomainWithStatus>,
    pub locale: Locale,
}
impl IntoResponse for DomainsTemplate {
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

#[derive(Deserialize)]
pub struct CreateDomainRequest {
    pub domain_name: String,
}

pub async fn domains_page(
    locale: Locale,
    _user: AdminUser,
    State(state): State<Arc<AppState>>,
) -> impl IntoResponse {
    let domains = get_domains(&state.db).await.unwrap_or_default();

    let mut set = tokio::task::JoinSet::new();
    for domain in domains {
        let dns_scanner = state.dns_scanner.clone();
        set.spawn(async move {
            let status = dns_scanner.check_domain(&domain.name).await.ok();
            DomainWithStatus {
                domain,
                dns_status: status,
            }
        });
    }

    let mut domains_with_status = Vec::new();
    while let Some(res) = set.join_next().await {
        if let Ok(d) = res {
            domains_with_status.push(d);
        }
    }

    // Optional: sort them back by name or date since JoinSet order is non-deterministic
    domains_with_status.sort_by(|a, b| a.domain.name.cmp(&b.domain.name));

    DomainsTemplate {
        domains: domains_with_status,
        locale,
    }
}

pub async fn create_domain_handler(
    _user: AdminUser,
    State(state): State<Arc<AppState>>,
    Form(payload): Form<CreateDomainRequest>,
) -> impl IntoResponse {
    let _ = crate::db::insert_domain(&state.db, &payload.domain_name).await;
    let domains = get_domains(&state.db).await.unwrap_or_default();

    let mut set = tokio::task::JoinSet::new();
    for domain in domains {
        let dns_scanner = state.dns_scanner.clone();
        set.spawn(async move {
            let status = dns_scanner.check_domain(&domain.name).await.ok();
            DomainWithStatus {
                domain,
                dns_status: status,
            }
        });
    }

    let mut domains_with_status = Vec::new();
    while let Some(res) = set.join_next().await {
        if let Ok(d) = res {
            domains_with_status.push(d);
        }
    }
    domains_with_status.sort_by(|a, b| a.domain.name.cmp(&b.domain.name));

    Html(
        domains_with_status
            .into_iter()
            .map(|d| {
                let dns_badge = if let Some(ref status) = d.dns_status {
                    let badge_html = if status.is_ok {
                        "<span class='badge' style='background: #e6fffa; color: #2c7a7b;'>DNS OK</span>"
                    } else {
                        "<span class='badge' style='background: #fff5f5; color: #c53030;'>DNS Issue</span>"
                    };

                    format!(
                        "<div class='maileroo-tooltip-container'>{}<div class='maileroo-tooltip-text'><strong>MX:</strong> {}<br><strong>SPF:</strong> {}<br><strong>DMARC:</strong> {}</div></div>",
                        badge_html,
                        status.mx_status.message,
                        status.spf_status.message,
                        status.dmarc_status.message
                    )
                } else {
                    "<span class='badge' style='background: #edf2f7; color: #4a5568;'>DNS Unknown</span>".to_string()
                };

                let dkim_badges = if d.domain.dkim_public_key.is_none() {
                    "<span class='badge' style='background: #fffaf0; border: 1px solid #feebc8; color: #c05621;'>⚠️ No DKIM</span>"
                } else if d.domain.pending_dkim_public_key.is_some() {
                    "<span class='badge' style='background: #ebf8ff; border: 1px solid #bee3f8; color: #2b6cb0;'>⏳ Rotating</span>"
                } else {
                    ""
                };

                let dns_cell = format!(
                    "<div style='display: flex; align-items: center; gap: 5px; flex-wrap: wrap;'>{}{}</div>",
                    dns_badge, dkim_badges
                );

                format!(
                    "<tr id='domain-{}'><td style='font-weight: 500;'>{}</td><td>{}</td><td style='color: #666; font-size: 0.9rem;'>{}</td><td style='text-align: right;'><button class='maileroo-alias-btn' style='background: #edf2f7; color: #4a5568; margin-right: 5px;' hx-get='/domains/{}/dkim-modal' hx-target='#maileroo-modal-placeholder' hx-swap='innerHTML'>DKIM Settings</button><button class='maileroo-alias-btn maileroo-alias-btn-delete' hx-delete='/domains/{}' hx-target='#domain-{}' hx-swap='outerHTML' hx-confirm='Are you sure you want to delete this domain?'>Delete</button></td></tr>",
                    d.domain.id, d.domain.name, dns_cell, &d.domain.created_at.to_string()[..10], d.domain.id, d.domain.id, d.domain.id
                )
            })
            .collect::<String>(),
    )
}

pub async fn delete_domain_handler(
    _user: AdminUser,
    State(state): State<Arc<AppState>>,
    Path(domain_id): Path<Uuid>,
) -> impl IntoResponse {
    use crate::db::delete_domain_by_id;
    match delete_domain_by_id(&state.db, domain_id).await {
        Ok(_) => StatusCode::OK.into_response(),
        Err(e) => {
            tracing::error!("Error deleting domain: {}", e);
            (StatusCode::INTERNAL_SERVER_ERROR, "Database error").into_response()
        }
    }
}

#[derive(Template)]
#[template(path = "aliases.html")]
pub struct AliasesTemplate {
    pub aliases: Vec<Alias>,
    pub domains: Vec<Domain>,
    pub suggestions: Vec<String>,
    pub error: Option<String>,
    pub max_aliases: i64,
    pub locale: Locale,
    pub can_bypass_limit: bool,
}

impl IntoResponse for AliasesTemplate {
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

pub async fn generate_corporate_suggestions(pool: &DbPool, domain_id: Uuid) -> Vec<String> {
    let mut pool_candidates = vec![];
    // Use random seed for fresh suggestions
    let mut rng = SmallRng::from_entropy();

    // Generate 50 unique candidates using Docker-style names
    while pool_candidates.len() < 50 {
        let cand = crate::web::names::generate_name(&mut rng);
        if !pool_candidates.contains(&cand) {
            pool_candidates.push(cand);
        }
    }
    pool_candidates.push("inquiries".to_string());
    pool_candidates.push("contact".to_string());

    // Filter against DB
    let taken_subdomains: Vec<String> = match pool {
        DbPool::Postgres(pool) => sqlx::query_scalar::<_, String>(
            "SELECT subdomain FROM aliases WHERE domain_id = $1 AND subdomain = ANY($2)",
        )
        .bind(domain_id)
        .bind(&pool_candidates)
        .fetch_all(pool)
        .await
        .unwrap_or_default(),
        DbPool::Sqlite(pool) => {
            let placeholders = vec!["?"; pool_candidates.len()].join(", ");
            let sql = format!(
                "SELECT subdomain FROM aliases WHERE domain_id = ? AND subdomain IN ({})",
                placeholders
            );
            let mut q = sqlx::query_scalar::<sqlx::Sqlite, String>(&sql).bind(domain_id);
            for cand in &pool_candidates {
                q = q.bind(cand);
            }
            q.fetch_all(pool).await.unwrap_or_default()
        }
    };

    pool_candidates
        .into_iter()
        .filter(|c| !taken_subdomains.contains(c))
        .take(8)
        .collect()
}

pub async fn aliases_page(
    locale: Locale,
    user: AuthenticatedUser,
    State(state): State<Arc<AppState>>,
) -> impl IntoResponse {
    let aliases = get_aliases_by_user_id(&state.db, user.user_id)
        .await
        .unwrap_or_default();
    let domains = get_domains(&state.db).await.unwrap_or_default();

    let suggestions = if let Some(domain) = domains.first() {
        generate_corporate_suggestions(&state.db, domain.id).await
    } else {
        vec![]
    };

    AliasesTemplate {
        aliases,
        domains,
        suggestions,
        error: None,
        max_aliases: MAX_ALIASES_PER_USER,
        locale,
        can_bypass_limit: user.can_bypass_alias_limit(),
    }
}

pub async fn delete_alias_handler(
    user: AuthenticatedUser,
    State(state): State<Arc<AppState>>,
    Path(alias_id): Path<Uuid>,
) -> impl IntoResponse {
    match delete_alias_by_id(&state.db, alias_id, user.user_id).await {
        Ok(_) => StatusCode::OK.into_response(),
        Err(e) => {
            tracing::error!("Error deleting alias: {}", e);
            (StatusCode::INTERNAL_SERVER_ERROR, "Database error").into_response()
        }
    }
}

#[derive(Template)]
#[template(path = "api_keys.html")]
pub struct ApiKeysTemplate {
    pub keys: Vec<crate::db::ApiKey>,
    pub locale: Locale,
    pub new_key: Option<String>,
}
impl IntoResponse for ApiKeysTemplate {
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

pub async fn api_keys_page(
    locale: Locale,
    user: AdminUser,
    State(state): State<Arc<AppState>>,
) -> impl IntoResponse {
    let keys = crate::db::get_api_keys(&state.db, user.0.user_id)
        .await
        .unwrap_or_default();

    ApiKeysTemplate {
        keys,
        locale,
        new_key: None,
    }
}

#[derive(Deserialize)]
pub struct CreateApiKeyRequest {
    pub name: String,
}

pub async fn create_api_key_handler(
    locale: Locale,
    user: AdminUser,
    State(state): State<Arc<AppState>>,
    Form(payload): Form<CreateApiKeyRequest>,
) -> impl IntoResponse {
    use rand::{Rng, thread_rng};
    use sha2::{Digest, Sha256};

    let token: String = thread_rng()
        .sample_iter(&rand::distributions::Alphanumeric)
        .take(32)
        .map(char::from)
        .collect();

    let mut hasher = Sha256::new();
    hasher.update(token.as_bytes());
    let hash = format!("{:x}", hasher.finalize());

    let _ = crate::db::insert_api_key(&state.db, user.0.user_id, &hash, &payload.name).await;

    let keys = crate::db::get_api_keys(&state.db, user.0.user_id)
        .await
        .unwrap_or_default();

    ApiKeysTemplate {
        keys,
        locale,
        new_key: Some(token),
    }
}

pub async fn delete_api_key_handler(
    user: AdminUser,
    State(state): State<Arc<AppState>>,
    Path(key_id): Path<Uuid>,
) -> impl IntoResponse {
    match crate::db::delete_api_key(&state.db, key_id, user.0.user_id).await {
        Ok(_) => StatusCode::OK.into_response(),
        Err(_) => (StatusCode::INTERNAL_SERVER_ERROR, "Database error").into_response(),
    }
}

pub async fn delete_alias_confirm_handler(
    locale: Locale,
    user: AuthenticatedUser,
    State(state): State<Arc<AppState>>,
    Path(alias_id): Path<Uuid>,
) -> impl IntoResponse {
    let alias = match crate::db::get_aliases_by_user_id(&state.db, user.user_id).await {
        Ok(aliases) => aliases.into_iter().find(|a| a.id == alias_id),
        Err(_) => None,
    };

    let Some(alias) = alias else {
        return (StatusCode::NOT_FOUND, "Alias not found").into_response();
    };

    ModalTemplate {
        title: locale.delete_alias_title().to_string(),
        message: locale.delete_alias_message(&format!("{}@{}", alias.subdomain, alias.domain_name)),
        action_label: locale.modal_delete_confirm().to_string(),
        cancel_label: locale.modal_cancel().to_string(),
        action_url: format!("/aliases/{}", alias_id),
        action_method: "delete".to_string(),
        action_color: "danger".to_string(),
        target: format!("#alias-{}", alias_id),
        swap: "outerHTML".to_string(),
        include_target: None,
    }
    .into_response()
}

#[derive(Deserialize)]
pub struct ToggleAutoForwardRequest {
    #[serde(default)]
    pub auto_forward: bool,
}

pub async fn toggle_alias_forward_handler(
    locale: Locale,
    user: AuthenticatedUser,
    State(state): State<Arc<AppState>>,
    Path(alias_id): Path<Uuid>,
    Form(payload): Form<ToggleAutoForwardRequest>,
) -> impl IntoResponse {
    match crate::db::update_alias_auto_forward(
        &state.db,
        alias_id,
        user.user_id,
        payload.auto_forward,
    )
    .await
    {
        Ok(_) => {
            let checked = if payload.auto_forward { "checked" } else { "" };
            let status_text = if payload.auto_forward {
                locale.status_enabled()
            } else {
                locale.status_disabled()
            };
            let status_class = if payload.auto_forward {
                "maileroo-status-enabled"
            } else {
                "maileroo-status-disabled"
            };

            Html(format!(
                r#"<div class="maileroo-toggle-wrapper">
                    <label class="maileroo-toggle-switch">
                        <input type="checkbox" name="auto_forward" value="true" {} 
                               hx-post="/aliases/{}/toggle-forward" 
                               hx-target="closest td" 
                               hx-swap="innerHTML settle:400ms">
                        <span class="maileroo-toggle-slider"></span>
                    </label>
                    <span class="{}">{}</span>
                </div>"#,
                checked, alias_id, status_class, status_text
            ))
            .into_response()
        }
        Err(e) => {
            tracing::error!("Error toggling alias forward: {}", e);
            (StatusCode::INTERNAL_SERVER_ERROR, "Database error").into_response()
        }
    }
}

#[derive(Template)]
#[template(path = "suggestion_list.html")]
pub struct SuggestionListTemplate {
    pub suggestions: Vec<String>,
}

#[derive(Template)]
#[template(path = "modal.html")]
pub struct ModalTemplate {
    pub title: String,
    pub message: String,
    pub action_label: String,
    pub cancel_label: String,
    pub action_url: String,
    pub action_method: String,
    pub action_color: String,
    pub target: String,
    pub swap: String,
    pub include_target: Option<String>,
}
impl IntoResponse for ModalTemplate {
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
impl IntoResponse for SuggestionListTemplate {
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

use crate::web::dashboard::DisplayEmail;

#[derive(Template)]
#[template(path = "email_row.html")]
pub struct EmailRowTemplate {
    pub email: DisplayEmail,
    pub locale: Locale,
}

#[derive(Template)]
#[template(path = "password_input.html")]
pub struct PasswordInputTemplate {
    pub show: bool,
    pub value: String,
    pub minlength: Option<String>,
    pub placeholder: Option<String>,
    pub autocomplete: Option<String>,
}
impl IntoResponse for PasswordInputTemplate {
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

#[derive(Deserialize)]
pub struct PasswordToggleRequest {
    pub show: bool,
    pub password: Option<String>,
    pub minlength: Option<String>,
    pub placeholder: Option<String>,
    pub autocomplete: Option<String>,
}

pub async fn password_toggle_handler(
    axum::Form(payload): axum::Form<PasswordToggleRequest>,
) -> impl IntoResponse {
    PasswordInputTemplate {
        show: payload.show,
        value: payload.password.unwrap_or_default(),
        minlength: payload.minlength,
        placeholder: payload.placeholder,
        autocomplete: payload.autocomplete,
    }
}

pub async fn alias_suggestions_handler(
    _user: AuthenticatedUser,
    State(state): State<Arc<AppState>>,
    axum::extract::Query(params): axum::extract::Query<std::collections::HashMap<String, String>>,
) -> impl IntoResponse {
    let domain_id = params
        .get("domain_id")
        .and_then(|id| Uuid::parse_str(id).ok());

    let headers = [
        (
            "Cache-Control",
            "no-store, no-cache, must-revalidate, max-age=0",
        ),
        ("Pragma", "no-cache"),
        ("Expires", "0"),
    ];

    if let Some(did) = domain_id {
        let suggestions = generate_corporate_suggestions(&state.db, did).await;
        (headers, SuggestionListTemplate { suggestions }).into_response()
    } else {
        (headers, Html("".to_string())).into_response()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use askama::Template;

    #[test]
    fn test_aliases_template_limit_enforcement() {
        let max_aliases = 5;

        let dummy_alias = crate::db::Alias {
            id: uuid::Uuid::new_v4(),
            user_id: uuid::Uuid::new_v4(),
            domain_id: uuid::Uuid::new_v4(),
            subdomain: "test".to_string(),
            destination_email: "test@example.com".to_string(),
            auto_forward: true,
            active: true,
            created_at: time::OffsetDateTime::now_utc(),
            domain_name: "example.com".to_string(),
        };

        // Create 5 aliases (limit reached)
        let aliases = vec![dummy_alias; 5];

        // 1. User CANNOT bypass limit
        let t_normal = AliasesTemplate {
            aliases: aliases.clone(),
            domains: vec![],
            suggestions: vec![],
            error: None,
            max_aliases,
            locale: Locale::En,
            can_bypass_limit: false,
        };

        let html_normal = t_normal.render().unwrap();
        // Since limits are reached and no bypass, button should be disabled
        assert!(
            html_normal.contains("disabled style=\"background: #cbd5e0; cursor: not-allowed;\"")
        );
        assert!(html_normal.contains("Limit of 5 aliases reached")); // Ensure the limit text exists

        // 2. User CAN bypass limit
        let t_bypass = AliasesTemplate {
            aliases,
            domains: vec![],
            suggestions: vec![],
            error: None,
            max_aliases,
            locale: Locale::En,
            can_bypass_limit: true,
        };

        let html_bypass = t_bypass.render().unwrap();
        // Button should NOT be disabled!
        assert!(
            !html_bypass.contains("disabled style=\"background: #cbd5e0; cursor: not-allowed;\"")
        );
        assert!(!html_bypass.contains("Limit of 5 aliases reached"));
    }
}
