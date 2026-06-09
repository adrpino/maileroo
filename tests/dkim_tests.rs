mod common;

use maileroo::web::{AppState, DashboardEvent, create_app};
use maileroo::config::AppConfig;
use maileroo::dns::DnsScanner;
use maileroo::outbound::OutboundService;
use std::sync::Arc;
use axum::http::{Request, StatusCode};
use axum::body::Body;
use tower::ServiceExt;
use uuid::Uuid;

#[tokio::test]
async fn test_dkim_rotation_e2e_flow() {
    common::run_on_all_dbs(|db| async move {
        // 1. Setup temporary storage directory
        let temp_storage_dir = tempfile::tempdir().unwrap();

        // 2. Setup mock test user and login to get auth cookie
        let user_email = format!("admin-dkim-{}@example.com", Uuid::new_v4());
        let user = common::create_test_user(&db, &user_email, "super-secure-password").await;
        common::grant_user_sender_permissions(&db, user.id).await;

        // 3. Create App State
        let resolver = hickory_resolver::TokioResolver::builder_tokio().unwrap().build().unwrap();
        let dns_scanner = DnsScanner::new(resolver.clone());
        let outbound = Arc::new(OutboundService::new(
            "srs_secret_key_123".to_string(),
            resolver,
            "example.com".to_string(),
            db.clone(),
            temp_storage_dir.path().to_path_buf(),
        ));

        let state = AppState {
            db: db.clone(),
            storage_dir: temp_storage_dir.path().to_path_buf(),
            dns_scanner,
            tx: tokio::sync::broadcast::channel::<DashboardEvent>(100).0,
            outbound,
            config: AppConfig { auto_tls: None },
        };

        let app_router = create_app(state).await;

        // 4. Authenticate via login
        let auth_cookie = common::get_auth_cookie(app_router.clone(), &user_email, "super-secure-password").await;
        let csrf_token = common::extract_csrf_token(&auth_cookie);

        // 5. Create a new domain using POST /domains
        let req = Request::builder()
            .method("POST")
            .uri("/domains")
            .header(axum::http::header::COOKIE, &auth_cookie)
            .header("X-CSRF-Token", &csrf_token)
            .header(axum::http::header::CONTENT_TYPE, "application/x-www-form-urlencoded")
            .body(Body::from("domain_name=example-rotation-test.com"))
            .unwrap();

        let response = app_router.clone().oneshot(req).await.unwrap();
        assert_eq!(response.status(), StatusCode::OK);

        // Fetch the domain from the DB to verify it was created with active DKIM keys
        let domains = maileroo::db::get_domains(&db).await.unwrap();
        let test_domain = domains.iter().find(|d| d.name == "example-rotation-test.com")
            .expect("Domain should have been inserted into database");

        assert!(test_domain.dkim_private_key.is_some(), "Active DKIM private key should be generated");
        assert!(test_domain.dkim_public_key.is_some(), "Active DKIM public key should be generated");
        assert_eq!(test_domain.dkim_selector, "maileroo");
        assert!(test_domain.pending_dkim_private_key.is_none());
        assert!(test_domain.pending_dkim_public_key.is_none());
        assert!(test_domain.pending_dkim_selector.is_none());

        // 6. Rotate DKIM using POST /domains/{id}/rotate-dkim
        let req_rotate = Request::builder()
            .method("POST")
            .uri(format!("/domains/{}/rotate-dkim", test_domain.id))
            .header(axum::http::header::COOKIE, &auth_cookie)
            .header("X-CSRF-Token", &csrf_token)
            .body(Body::empty())
            .unwrap();

        let response_rotate = app_router.clone().oneshot(req_rotate).await.unwrap();
        assert_eq!(response_rotate.status(), StatusCode::OK);

        let body_bytes = axum::body::to_bytes(response_rotate.into_body(), usize::MAX).await.unwrap();
        let body_string = String::from_utf8_lossy(&body_bytes);
        assert!(body_string.contains("Pending DKIM Key Verification"), "Should render pending verification section");
        assert!(body_string.contains("maileroo2._domainkey"), "Should render new selector host");

        // Verify the database state has the pending key set
        let test_domain_rotated = maileroo::db::get_domain_by_id(&db, test_domain.id).await.unwrap().unwrap();
        assert!(test_domain_rotated.pending_dkim_private_key.is_some());
        assert!(test_domain_rotated.pending_dkim_public_key.is_some());
        assert_eq!(test_domain_rotated.pending_dkim_selector, Some("maileroo2".to_string()));

        // 7. Verify DKIM fails when DNS TXT record is missing
        let req_verify = Request::builder()
            .method("POST")
            .uri(format!("/domains/{}/verify-dkim", test_domain.id))
            .header(axum::http::header::COOKIE, &auth_cookie)
            .header("X-CSRF-Token", &csrf_token)
            .body(Body::empty())
            .unwrap();

        let response_verify = app_router.clone().oneshot(req_verify).await.unwrap();
        assert_eq!(response_verify.status(), StatusCode::OK); // Renders the template with the error inside (standard HTMX pattern)

        let body_bytes_verify = axum::body::to_bytes(response_verify.into_body(), usize::MAX).await.unwrap();
        let body_string_verify = String::from_utf8_lossy(&body_bytes_verify);
        assert!(body_string_verify.contains("DNS Verification failed"), "Should render failure message inside the template");

        // 8. Cancel DKIM rotation using POST /domains/{id}/cancel-dkim-rotation
        let req_cancel = Request::builder()
            .method("POST")
            .uri(format!("/domains/{}/cancel-dkim-rotation", test_domain.id))
            .header(axum::http::header::COOKIE, &auth_cookie)
            .header("X-CSRF-Token", &csrf_token)
            .body(Body::empty())
            .unwrap();

        let response_cancel = app_router.clone().oneshot(req_cancel).await.unwrap();
        assert_eq!(response_cancel.status(), StatusCode::OK);

        let body_bytes_cancel = axum::body::to_bytes(response_cancel.into_body(), usize::MAX).await.unwrap();
        let body_string_cancel = String::from_utf8_lossy(&body_bytes_cancel);
        assert!(!body_string_cancel.contains("Pending DKIM Key Verification"), "Pending section should be gone after cancellation");

        // Verify the database state cleared the pending columns
        let test_domain_cancelled = maileroo::db::get_domain_by_id(&db, test_domain.id).await.unwrap().unwrap();
        assert!(test_domain_cancelled.pending_dkim_private_key.is_none());
        assert!(test_domain_cancelled.pending_dkim_public_key.is_none());
        assert!(test_domain_cancelled.pending_dkim_selector.is_none());
    }).await;
}
