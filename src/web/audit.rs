//! Structured audit logging for security-relevant events and per-request context.
//!
//! Audit events are emitted with the `audit` tracing target so they can be
//! filtered independently (e.g. `journalctl | grep 'maileroo::audit'` or
//! `RUST_LOG=audit=info`). Request context helpers feed the `http_request`
//! span so every log line carries the raw peer address and forwarded chain.

use axum::extract::{ConnectInfo, Request};
use axum::http::{Extensions, HeaderMap};
use axum::middleware::Next;
use axum::response::Response;
use sha2::{Digest, Sha256};
use std::fmt::Write;
use std::net::SocketAddr;
use uuid::Uuid;

/// Security-relevant actions recorded in the audit log.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AuditEvent {
    UserRegistered,
    LoginSucceeded,
    LoginFailed,
    Logout,
    AliasCreated,
    AliasDeleted,
    ApiKeyCreated,
    ApiKeyDeleted,
    FilterCreated,
    FilterDeleted,
    AdminUserPermissionChanged,
    RateLimited,
}

impl AuditEvent {
    /// Stable machine-readable event name.
    pub fn as_str(self) -> &'static str {
        match self {
            AuditEvent::UserRegistered => "user_registered",
            AuditEvent::LoginSucceeded => "login_succeeded",
            AuditEvent::LoginFailed => "login_failed",
            AuditEvent::Logout => "logout",
            AuditEvent::AliasCreated => "alias_created",
            AuditEvent::AliasDeleted => "alias_deleted",
            AuditEvent::ApiKeyCreated => "api_key_created",
            AuditEvent::ApiKeyDeleted => "api_key_deleted",
            AuditEvent::FilterCreated => "filter_created",
            AuditEvent::FilterDeleted => "filter_deleted",
            AuditEvent::AdminUserPermissionChanged => "admin_user_permission_changed",
            AuditEvent::RateLimited => "rate_limited",
        }
    }
}

/// Emits an audit event. The surrounding `http_request` span supplies the
/// method, path, peer address and client IP.
pub fn record(event: AuditEvent, user_id: Option<Uuid>, detail: &str) {
    match user_id {
        Some(id) => tracing::info!(
            target: "audit",
            event = event.as_str(),
            user_id = %id,
            detail = detail,
            "audit"
        ),
        None => tracing::info!(target: "audit", event = event.as_str(), detail = detail, "audit"),
    }
}

/// Handle to the top-level `http_request` span, stored in request extensions.
///
/// Inner layers (e.g. the session layer) open nested spans, so
/// `tracing::Span::current()` inside a handler is not the request span.
#[derive(Clone)]
pub struct RequestSpan(tracing::Span);

/// Middleware that stores the current `http_request` span in the request
/// extensions. Must be layered directly inside the tracing layer.
pub async fn capture_request_span(mut req: Request, next: Next) -> Response {
    req.extensions_mut()
        .insert(RequestSpan(tracing::Span::current()));
    next.run(req).await
}

/// Records the authenticated user on the request span so that all
/// subsequent log lines for the request carry `user_id`.
pub fn record_span_user(extensions: &Extensions, user_id: Uuid) {
    if let Some(RequestSpan(span)) = extensions.get::<RequestSpan>() {
        span.record("user_id", tracing::field::display(user_id));
    }
}

/// Pure function producing a stable, non-reversible identifier for an email
/// address so failed logins can be correlated without logging the address.
pub fn hash_email(email: &str) -> String {
    let digest = Sha256::digest(email.trim().to_lowercase().as_bytes());
    digest.iter().take(8).fold(String::new(), |mut out, byte| {
        let _ = write!(out, "{:02x}", byte);
        out
    })
}

/// Pure function returning the raw TCP peer address, or `-` when unavailable.
pub fn peer_addr(extensions: &Extensions) -> String {
    extensions
        .get::<ConnectInfo<SocketAddr>>()
        .map(|ci| ci.0.ip().to_string())
        .unwrap_or_else(|| "-".into())
}

/// Pure function returning the raw `X-Forwarded-For` header, or `-` when absent.
pub fn forwarded_for(headers: &HeaderMap) -> &str {
    headers
        .get("x-forwarded-for")
        .and_then(|val| val.to_str().ok())
        .unwrap_or("-")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_hash_email_is_stable_and_normalized() {
        let a = hash_email("User@Example.com");
        let b = hash_email("  user@example.com ");
        assert_eq!(a, b);
        assert_eq!(a.len(), 16);
        assert!(a.chars().all(|c| c.is_ascii_hexdigit()));
        assert_ne!(a, hash_email("other@example.com"));
    }

    #[test]
    fn test_peer_addr() {
        let mut ext = Extensions::new();
        assert_eq!(peer_addr(&ext), "-");
        ext.insert(ConnectInfo(SocketAddr::from(([10, 0, 0, 1], 4000))));
        assert_eq!(peer_addr(&ext), "10.0.0.1");
    }

    #[test]
    fn test_forwarded_for() {
        let mut headers = HeaderMap::new();
        assert_eq!(forwarded_for(&headers), "-");
        headers.insert("x-forwarded-for", "1.2.3.4, 5.6.7.8".parse().unwrap());
        assert_eq!(forwarded_for(&headers), "1.2.3.4, 5.6.7.8");
    }

    #[test]
    fn test_event_names_are_unique() {
        let all = [
            AuditEvent::UserRegistered,
            AuditEvent::LoginSucceeded,
            AuditEvent::LoginFailed,
            AuditEvent::Logout,
            AuditEvent::AliasCreated,
            AuditEvent::AliasDeleted,
            AuditEvent::ApiKeyCreated,
            AuditEvent::ApiKeyDeleted,
            AuditEvent::FilterCreated,
            AuditEvent::FilterDeleted,
            AuditEvent::AdminUserPermissionChanged,
            AuditEvent::RateLimited,
        ];
        let mut names: Vec<&str> = all.iter().map(|e| e.as_str()).collect();
        names.sort_unstable();
        names.dedup();
        assert_eq!(names.len(), all.len());
    }
}
