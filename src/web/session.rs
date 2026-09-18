use crate::config::get_config;
use crate::db::User;
use crate::web::SESSION_MAX_AGE_SECONDS;
use tower_sessions::Session;
use uuid::Uuid;

/// Builds the `Set-Cookie` value that carries the CSRF token to the client.
///
/// The cookie is deliberately not `HttpOnly`: the htmx `configRequest` hook in
/// `base.html` reads it and mirrors it into the `X-CSRF-Token` header.
pub fn build_csrf_cookie(token: &str, max_age: i64, cookie_domain: &str, secure: bool) -> String {
    let mut cookie = format!(
        "csrf_token={}; Path=/; SameSite=Lax; Max-Age={}",
        token, max_age
    );
    if !cookie_domain.is_empty() {
        cookie.push_str(&format!("; Domain={}", cookie_domain));
    }
    if secure {
        cookie.push_str("; Secure");
    }
    cookie
}

fn cookie_domain() -> String {
    get_config("COOKIE_DOMAIN", "")
}

fn secure_cookies() -> bool {
    get_config("SECURE_COOKIES", "true") == "true"
}

/// Config-aware cookie header for an active session.
pub fn csrf_cookie_header(token: &str) -> String {
    build_csrf_cookie(
        token,
        SESSION_MAX_AGE_SECONDS,
        &cookie_domain(),
        secure_cookies(),
    )
}

/// Config-aware expired cookie header, used to clear the token on logout.
pub fn csrf_clear_cookie_header() -> String {
    build_csrf_cookie("", 0, &cookie_domain(), secure_cookies())
}

/// Establishes an authenticated session for `user` and returns the fresh CSRF token.
///
/// Every path that logs a user in must go through this function. It caches the user
/// flags in the session and, critically, issues the CSRF token that `AuthenticatedUser`
/// requires on every state-changing request. A session created without one leaves the
/// user able to browse but unable to POST anything until they log in again.
///
/// The caller is responsible for attaching [`csrf_cookie_header`] to the response.
pub async fn establish_session(session: &Session, user: &User) -> String {
    let csrf_token = Uuid::new_v4().to_string();

    let _ = session.insert("user_id", user.id).await;
    let _ = session.insert("is_admin", user.is_admin).await;
    let _ = session
        .insert("bypass_alias_limit", user.bypass_alias_limit)
        .await;
    let _ = session
        .insert("can_send_firsthand", user.can_send_firsthand)
        .await;
    let _ = session.insert("user_data_loaded", true).await;
    let _ = session.insert("csrf_token", &csrf_token).await;

    tracing::info!(
        user_id = %user.id,
        is_admin = user.is_admin,
        "Session established; CSRF token issued"
    );

    csrf_token
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_build_csrf_cookie_minimal() {
        let cookie = build_csrf_cookie("abc123", 604800, "", false);
        assert_eq!(
            cookie,
            "csrf_token=abc123; Path=/; SameSite=Lax; Max-Age=604800"
        );
    }

    #[test]
    fn test_build_csrf_cookie_with_domain_and_secure() {
        let cookie = build_csrf_cookie("abc123", 604800, "maileroo.test", true);
        assert_eq!(
            cookie,
            "csrf_token=abc123; Path=/; SameSite=Lax; Max-Age=604800; Domain=maileroo.test; Secure"
        );
    }

    #[test]
    fn test_build_csrf_cookie_clearing() {
        // Max-Age=0 with an empty value is what expires the cookie on logout.
        let cookie = build_csrf_cookie("", 0, "", false);
        assert_eq!(cookie, "csrf_token=; Path=/; SameSite=Lax; Max-Age=0");
    }
}
