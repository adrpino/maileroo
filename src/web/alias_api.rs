use crate::db::{get_aliases_by_user_id, get_domains, insert_alias, is_subdomain_available};
use crate::web::AppState;
use crate::web::AuthenticatedUser;
use crate::web::handlers::{AliasesTemplate, MAX_ALIASES_PER_USER, generate_corporate_suggestions};
use crate::web::i18n::{Locale, Messages};
use axum::body::Body;
use axum::response::Response;
use axum::{
    extract::{Form, State},
    http::StatusCode,
    response::IntoResponse,
};
use regex::Regex;
use serde::Deserialize;
use std::sync::{Arc, OnceLock};
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

pub async fn create_alias_handler(
    locale: Locale,
    user: AuthenticatedUser,
    State(state): State<Arc<AppState>>,
    Form(payload): Form<CreateAliasRequest>,
) -> impl IntoResponse {
    // 0. Limit check for non-admin users
    if !user.can_bypass_alias_limit() {
        let count = crate::db::get_alias_count(&state.db, user.user_id)
            .await
            .unwrap_or(0);
        if count >= MAX_ALIASES_PER_USER {
            let aliases = get_aliases_by_user_id(&state.db, user.user_id)
                .await
                .unwrap_or_default();
            let domains = get_domains(&state.db).await.unwrap_or_default();
            let suggestions = generate_corporate_suggestions(&state.db, payload.domain_id).await;

            return AliasesTemplate {
                aliases,
                domains,
                suggestions,
                error: Some(locale.error_alias_limit_reached(MAX_ALIASES_PER_USER)),
                max_aliases: MAX_ALIASES_PER_USER,
                locale,
                can_bypass_limit: user.can_bypass_alias_limit(),
            }
            .into_response();
        }
    }

    let subdomain = payload
        .custom_subdomain
        .filter(|s| !s.is_empty())
        .or(payload.subdomain)
        .unwrap_or_else(|| "info".to_string());

    // 1. Validation logic
    match validate_alias(&subdomain, user.is_admin) {
        Err(AliasError::TooLong) => {
            let aliases = get_aliases_by_user_id(&state.db, user.user_id)
                .await
                .unwrap_or_default();
            let domains = get_domains(&state.db).await.unwrap_or_default();
            let suggestions = generate_corporate_suggestions(&state.db, payload.domain_id).await;
            return AliasesTemplate {
                aliases,
                domains,
                suggestions,
                error: Some(locale.error_alias_too_long(32)),
                max_aliases: MAX_ALIASES_PER_USER,
                locale,
                can_bypass_limit: user.can_bypass_alias_limit(),
            }
            .into_response();
        }
        Err(AliasError::Reserved) => {
            let aliases = get_aliases_by_user_id(&state.db, user.user_id)
                .await
                .unwrap_or_default();
            let domains = get_domains(&state.db).await.unwrap_or_default();
            let suggestions = generate_corporate_suggestions(&state.db, payload.domain_id).await;
            return AliasesTemplate {
                aliases,
                domains,
                suggestions,
                error: Some(locale.error_alias_reserved().to_string()),
                max_aliases: MAX_ALIASES_PER_USER,
                locale,
                can_bypass_limit: user.can_bypass_alias_limit(),
            }
            .into_response();
        }
        Err(AliasError::InvalidFormat) => {
            let aliases = get_aliases_by_user_id(&state.db, user.user_id)
                .await
                .unwrap_or_default();
            let domains = get_domains(&state.db).await.unwrap_or_default();
            let suggestions = generate_corporate_suggestions(&state.db, payload.domain_id).await;
            return AliasesTemplate {
                aliases,
                domains,
                suggestions,
                error: Some(locale.error_alias_invalid_format().to_string()),
                max_aliases: MAX_ALIASES_PER_USER,
                locale,
                can_bypass_limit: user.can_bypass_alias_limit(),
            }
            .into_response();
        }
        Ok(_) => {}
    }

    // 2. Race condition check: Final availability verification
    if !is_subdomain_available(&state.db, payload.domain_id, &subdomain)
        .await
        .unwrap_or(false)
    {
        let aliases = get_aliases_by_user_id(&state.db, user.user_id)
            .await
            .unwrap_or_default();
        let domains = get_domains(&state.db).await.unwrap_or_default();
        let suggestions = generate_corporate_suggestions(&state.db, payload.domain_id).await;

        return AliasesTemplate {
            aliases,
            domains,
            suggestions,
            error: Some(locale.error_alias_taken().to_string()),
            max_aliases: MAX_ALIASES_PER_USER,
            locale,
            can_bypass_limit: user.can_bypass_alias_limit(),
        }
        .into_response();
    }

    let user_record = match crate::db::get_user_by_id(&state.db, user.user_id).await {
        Ok(Some(u)) => u,
        _ => return (StatusCode::INTERNAL_SERVER_ERROR, "User not found").into_response(),
    };

    let _ = insert_alias(
        &state.db,
        user.user_id,
        payload.domain_id,
        &subdomain,
        &user_record.email,
        payload.auto_forward,
    )
    .await;

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
