use crate::db::{
    get_alias_count, get_aliases_by_user_id, get_domains, get_user_by_id, insert_alias,
    is_subdomain_available,
};
use crate::limits::{Windows, network_of};
use crate::web::audit::{self, AuditEvent};
use crate::web::handlers::{AliasesTemplate, MAX_ALIASES_PER_USER, generate_corporate_suggestions};
use crate::web::i18n::{Locale, Messages};
use crate::web::{AppState, AuthenticatedUser, client_ip};
use axum::body::Body;
use axum::response::Response;
use axum::{
    Extension,
    extract::{Form, State},
    http::{Extensions, HeaderMap, StatusCode},
    response::IntoResponse,
};
use regex::Regex;
use serde::Deserialize;
use std::net::IpAddr;
use std::sync::{Arc, OnceLock};
use std::time::{Duration, Instant};
use uuid::Uuid;

#[derive(Deserialize)]
pub struct CreateAliasRequest {
    pub domain_id: Uuid,
    pub subdomain: Option<String>,
    pub custom_subdomain: Option<String>,
    #[serde(default)]
    pub auto_forward: bool,
}

#[derive(Debug, PartialEq)]
pub enum AliasError {
    TooLong,
    Reserved,
    InvalidFormat,
}

static ALIAS_REGEX: OnceLock<Regex> = OnceLock::new();

/// Pure validation logic for alias subdomains
pub fn validate_alias(subdomain: &str, is_admin: bool) -> Result<(), AliasError> {
    // 1. Length check
    if subdomain.is_empty() || subdomain.len() > 32 {
        return Err(AliasError::TooLong);
    }

    // 2. Strict character check (Regex)
    // Only lowercase letters, numbers, hyphens, and dots.
    // Must not start or end with a hyphen or dot.
    let re = ALIAS_REGEX
        .get_or_init(|| Regex::new(r"^[a-z0-9][a-z0-9.-]*[a-z0-9]$|^[a-z0-9]$").unwrap());
    if !re.is_match(subdomain) {
        return Err(AliasError::InvalidFormat);
    }

    // 3. Banned words check (only for non-admins)
    if !is_admin {
        let banned_aliases = [
            // Standard RFC 2142 / Infrastructure
            "admin",
            "administrator",
            "root",
            "postmaster",
            "webmaster",
            "hostmaster",
            "security",
            "support",
            "billing",
            "abuse",
            "mailer-daemon",
            "sysadmin",
            "noc",
            "dns",
            "host",
            // Security & Compliance
            "compliance",
            "legal",
            "privacy",
            "dpo",
            "secure",
            "proxy",
            // Marketing/Sales Impersonation
            "sales",
            "contact",
            "info",
            "marketing",
            "newsletter",
            "media",
            "press",
            "jobs",
            "careers",
            "hr",
            "office",
            // Technical/System
            "api",
            "dev",
            "web",
            "mail",
            "help",
            "staff",
            "system",
            "no-reply",
            "noreply",
            "do-not-reply",
            "notification",
            "alert",
            // High-Value / Auth
            "verify",
            "auth",
            "login",
            "sso",
            "account",
            "payment",
        ];

        if banned_aliases.contains(&subdomain.to_lowercase().as_str()) {
            return Err(AliasError::Reserved);
        }
    }

    Ok(())
}

/// "Taken" answers one account may get from the alias form in an hour.
pub const TAKEN_ANSWERS_PER_ACCOUNT_PER_HOUR: u32 = 10;
/// "Taken" answers all accounts on one network may get from the alias form in an hour.
pub const TAKEN_ANSWERS_PER_NETWORK_PER_HOUR: u32 = 30;

/// Limits how often the alias form may answer "taken", so it can't be used to find out
/// which addresses exist. Counted per account and per client network; once either is
/// exhausted the form refuses every name until the hour is over, since answering only
/// for free names would still reveal the taken ones. Administrators are not limited.
pub struct AliasGuesses {
    per_account: Windows<Uuid>,
    per_network: Windows<IpAddr>,
}

impl Default for AliasGuesses {
    fn default() -> Self {
        let hour = Duration::from_secs(3600);
        Self {
            per_account: Windows::new(hour),
            per_network: Windows::new(hour),
        }
    }
}

impl AliasGuesses {
    /// Whether `user` (or its network) has had all its "taken" answers this hour.
    pub fn exhausted(&self, user: Uuid, ip: Option<IpAddr>, now: Instant) -> bool {
        self.per_account.current(&user, now) >= TAKEN_ANSWERS_PER_ACCOUNT_PER_HOUR
            || ip.is_some_and(|ip| {
                self.per_network.current(&network_of(ip), now) >= TAKEN_ANSWERS_PER_NETWORK_PER_HOUR
            })
    }

    /// Counts one "taken" answer for `user` and its network.
    pub fn count_taken(&self, user: Uuid, ip: Option<IpAddr>, now: Instant) {
        self.per_account.count(user, now);
        if let Some(ip) = ip {
            self.per_network.count(network_of(ip), now);
        }
    }
}

/// The aliases page again, showing `error` above the form.
async fn aliases_page_with_error(
    state: &AppState,
    user: &AuthenticatedUser,
    locale: Locale,
    domain_id: Uuid,
    error: String,
) -> Response {
    let aliases = get_aliases_by_user_id(&state.db, user.user_id)
        .await
        .unwrap_or_default();
    let domains = get_domains(&state.db).await.unwrap_or_default();
    let suggestions = generate_corporate_suggestions(&state.db, domain_id).await;

    AliasesTemplate {
        aliases,
        domains,
        suggestions,
        error: Some(error),
        max_aliases: MAX_ALIASES_PER_USER,
        locale,
        can_bypass_limit: user.can_bypass_alias_limit(),
    }
    .into_response()
}

pub async fn create_alias_handler(
    locale: Locale,
    user: AuthenticatedUser,
    State(state): State<Arc<AppState>>,
    Extension(guesses): Extension<Arc<AliasGuesses>>,
    headers: HeaderMap,
    extensions: Extensions,
    Form(payload): Form<CreateAliasRequest>,
) -> impl IntoResponse {
    // 0. Limit check for non-admin users
    if !user.can_bypass_alias_limit() {
        let count = get_alias_count(&state.db, user.user_id).await.unwrap_or(0);
        if count >= MAX_ALIASES_PER_USER {
            let error = locale.error_alias_limit_reached(MAX_ALIASES_PER_USER);
            return aliases_page_with_error(&state, &user, locale, payload.domain_id, error).await;
        }
    }

    let subdomain = payload
        .custom_subdomain
        .filter(|s| !s.is_empty())
        .or(payload.subdomain)
        .unwrap_or_else(|| "info".to_string());

    // 1. Validation logic
    let invalid = match validate_alias(&subdomain, user.is_admin) {
        Err(AliasError::TooLong) => Some(locale.error_alias_too_long(32)),
        Err(AliasError::Reserved) => Some(locale.error_alias_reserved().to_string()),
        Err(AliasError::InvalidFormat) => Some(locale.error_alias_invalid_format().to_string()),
        Ok(_) => None,
    };
    if let Some(error) = invalid {
        return aliases_page_with_error(&state, &user, locale, payload.domain_id, error).await;
    }

    // 2. Stop answering once too many names tried were taken
    let now = Instant::now();
    let ip = client_ip(&headers, &extensions);
    if !user.is_admin && guesses.exhausted(user.user_id, ip, now) {
        audit::record(
            AuditEvent::RateLimited,
            Some(user.user_id),
            "alias name checks",
        );
        let error = locale.error_alias_too_many_tries().to_string();
        return aliases_page_with_error(&state, &user, locale, payload.domain_id, error).await;
    }

    // 3. Race condition check: Final availability verification
    if !is_subdomain_available(&state.db, payload.domain_id, &subdomain)
        .await
        .unwrap_or(false)
    {
        guesses.count_taken(user.user_id, ip, now);
        let error = locale.error_alias_taken().to_string();
        return aliases_page_with_error(&state, &user, locale, payload.domain_id, error).await;
    }

    let user_record = match get_user_by_id(&state.db, user.user_id).await {
        Ok(Some(u)) => u,
        _ => return (StatusCode::INTERNAL_SERVER_ERROR, "User not found").into_response(),
    };

    if let Err(e) = insert_alias(
        &state.db,
        user.user_id,
        payload.domain_id,
        &subdomain,
        &user_record.email,
        payload.auto_forward,
    )
    .await
    {
        tracing::error!("Error inserting alias: {}", e);
        guesses.count_taken(user.user_id, ip, now);
        let error = locale.error_alias_taken().to_string();
        return aliases_page_with_error(&state, &user, locale, payload.domain_id, error).await;
    }

    audit::record(
        AuditEvent::AliasCreated,
        Some(user.user_id),
        &format!("subdomain={} domain_id={}", subdomain, payload.domain_id),
    );

    Response::builder()
        .header("HX-Redirect", "/aliases")
        .status(StatusCode::OK)
        .body(Body::empty())
        .unwrap()
        .into_response()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_validate_alias_length() {
        assert_eq!(validate_alias("a", false), Ok(()));
        assert_eq!(validate_alias(&"a".repeat(32), false), Ok(()));
        assert_eq!(
            validate_alias(&"a".repeat(33), false),
            Err(AliasError::TooLong)
        );
    }

    #[test]
    fn test_validate_alias_reserved() {
        assert_eq!(validate_alias("admin", false), Err(AliasError::Reserved));
        assert_eq!(validate_alias("support", false), Err(AliasError::Reserved));
        assert_eq!(validate_alias("my-alias", false), Ok(()));
    }

    #[test]
    fn test_validate_alias_admin_bypass() {
        // Admins can use reserved words
        assert_eq!(validate_alias("admin", true), Ok(()));
        // But admins still can't exceed length limit (system constraint)
        assert_eq!(
            validate_alias(&"a".repeat(33), true),
            Err(AliasError::TooLong)
        );
    }

    #[test]
    fn test_validate_alias_invalid_format() {
        // Valid
        assert_eq!(validate_alias("valid-name", false), Ok(()));
        assert_eq!(validate_alias("valid.name", false), Ok(()));
        assert_eq!(validate_alias("123name", false), Ok(()));
        assert_eq!(validate_alias("name123", false), Ok(()));

        // Invalid Characters (SMTP Splitting / XSS)
        assert_eq!(
            validate_alias("name\r\n", false),
            Err(AliasError::InvalidFormat)
        );
        assert_eq!(
            validate_alias("name<script>", false),
            Err(AliasError::InvalidFormat)
        );
        assert_eq!(
            validate_alias("name!@#", false),
            Err(AliasError::InvalidFormat)
        );
        assert_eq!(
            validate_alias("name spaces", false),
            Err(AliasError::InvalidFormat)
        );
        assert_eq!(
            validate_alias("Uppercase", false),
            Err(AliasError::InvalidFormat)
        ); // only lowercase

        // Invalid Positions
        assert_eq!(
            validate_alias("-name", false),
            Err(AliasError::InvalidFormat)
        );
        assert_eq!(
            validate_alias("name-", false),
            Err(AliasError::InvalidFormat)
        );
        assert_eq!(
            validate_alias(".name", false),
            Err(AliasError::InvalidFormat)
        );
        assert_eq!(
            validate_alias("name.", false),
            Err(AliasError::InvalidFormat)
        );
    }
}
