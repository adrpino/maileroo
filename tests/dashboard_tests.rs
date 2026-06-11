mod common;

use axum::body::Body;
use axum::http::{Request, StatusCode};
use maileroo::config::AppConfig;
use maileroo::db::sent_emails::EmailStatus;
use maileroo::dns::DnsScanner;
use maileroo::outbound::OutboundService;
use maileroo::web::{AppState, DashboardEvent, create_app};
use std::sync::Arc;
use tower::ServiceExt;
use uuid::Uuid;

#[tokio::test]
async fn test_draft_delete_confirmation_page_ok() {
    common::run_on_all_dbs(|db| async move {
        // 1. Setup temporary storage dir
        let temp_storage_dir = tempfile::tempdir().unwrap();

        // 2. Setup mock test user, alias and a draft email
        let user =
            common::create_test_user(&db, "test_admin@example.com", "my_secure_password123").await;
        let alias =
            common::create_test_alias(&db, user.id, "example.com", "hello", "dest@gmail.com").await;
        let draft = common::create_test_draft(
            &db,
            user.id,
            alias.id,
            "someone@external.com",
            "Urgent-Subject-Line",
            EmailStatus::Draft,
        )
        .await;

        // 3. Create App State
        let resolver = hickory_resolver::TokioResolver::builder_tokio()
            .unwrap()
            .build()
            .unwrap();
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
        let auth_cookie = common::get_auth_cookie(
            app_router.clone(),
            "test_admin@example.com",
            "my_secure_password123",
        )
        .await;

        // Extract CSRF token value
        let csrf_token = common::extract_csrf_token(&auth_cookie);

        // 5. Send GET request to the delete-confirm endpoint
        let request = Request::builder()
            .uri(format!("/emails/{}/delete-confirm", draft.id))
            .header(axum::http::header::COOKIE, auth_cookie)
            .header("X-CSRF-Token", csrf_token)
            .body(Body::empty())
            .unwrap();

        let response = app_router.oneshot(request).await.unwrap();

        // 6. Assert success and verify correct template rendering
        assert_eq!(response.status(), StatusCode::OK);

        let body_bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        let body_string = String::from_utf8_lossy(&body_bytes);

        assert!(body_string.contains("Urgent-Subject-Line"));
        assert!(body_string.contains("delete"));
    })
    .await;
}

#[tokio::test]
async fn test_email_deletion_and_disk_cleanup_flow() {
    common::run_on_all_dbs(|db| async move {
        // 1. Setup temporary storage directory
        let temp_storage_dir = tempfile::tempdir().unwrap();

        // 2. Setup mock test user, alias and a draft email
        let user =
            common::create_test_user(&db, "test_admin@example.com", "my_secure_password123").await;
        let alias =
            common::create_test_alias(&db, user.id, "example.com", "hello", "dest@gmail.com").await;
        let draft = common::create_test_draft(
            &db,
            user.id,
            alias.id,
            "someone@external.com",
            "Draft-Subject",
            EmailStatus::Draft,
        )
        .await;

        // 3. Write a mock raw email body file to the storage directory matching the draft's body_key!
        let draft_file_path = temp_storage_dir.path().join(draft.body_key.to_string());
        tokio::fs::write(&draft_file_path, b"Mock raw draft email payload")
            .await
            .unwrap();
        assert!(draft_file_path.exists());

        // 4. Create App State
        let resolver = hickory_resolver::TokioResolver::builder_tokio()
            .unwrap()
            .build()
            .unwrap();
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

        // 5. Authenticate via login
        let auth_cookie = common::get_auth_cookie(
            app_router.clone(),
            "test_admin@example.com",
            "my_secure_password123",
        )
        .await;

        // Extract CSRF token value
        let csrf_token = common::extract_csrf_token(&auth_cookie);

        // 6. Send DELETE request to delete the draft email
        let request = Request::builder()
            .method("DELETE")
            .uri(format!("/emails/{}", draft.id))
            .header(axum::http::header::COOKIE, auth_cookie)
            .header("X-CSRF-Token", csrf_token)
            .body(Body::empty())
            .unwrap();

        let response = app_router.oneshot(request).await.unwrap();

        // 7. Assert database removal and file deletion success!
        assert_eq!(response.status(), StatusCode::OK);

        // Verify database row is deleted in a dialect-safe way
        let db_check_exists = common::email_exists_in_db(&db, draft.id).await;
        assert!(!db_check_exists, "Database row was not deleted!");

        // Verify physical file is cleaned up from disk (allowing a moment for the spawn background task to delete the file)
        let mut file_deleted = false;
        for _ in 0..100 {
            if !draft_file_path.exists() {
                file_deleted = true;
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        }
        assert!(
            file_deleted,
            "Storage file was not physically deleted from disk!"
        );
    })
    .await;
}

#[tokio::test]
async fn test_send_saved_draft_flow_success() {
    // Install the Rustls process-level CryptoProvider
    let _ = tokio_rustls::rustls::crypto::aws_lc_rs::default_provider().install_default();

    common::run_on_all_dbs(|db| async move {
        // 1. Setup temporary storage directory
        let temp_storage_dir = tempfile::tempdir().unwrap();

        // 2. Setup mock test user, alias and TWO draft emails
        let user = common::create_test_user(&db, "test_admin@example.com", "my_secure_password123").await;

        // Grant permissions to the test user to satisfy FirsthandSenderUser extractor
        common::grant_user_sender_permissions(&db, user.id).await;

        let alias = common::create_test_alias(&db, user.id, "example.com", "hello", "dest@gmail.com").await;
        let draft1 = common::create_test_draft(&db, user.id, alias.id, "someone1@external.com", "Draft-Subject-1", EmailStatus::Draft).await;
        let draft2 = common::create_test_draft(&db, user.id, alias.id, "someone2@external.com", "Draft-Subject-2", EmailStatus::Draft).await;

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
        let auth_cookie = common::get_auth_cookie(app_router.clone(), "test_admin@example.com", "my_secure_password123").await;

        // Extract CSRF token value
        let csrf_token = common::extract_csrf_token(&auth_cookie);

        // 5. Send POST request to send the email from the first draft
        let payload1 = format!(
            "draft_id={}&from_alias_id={}&to_email=recipient1@external.com&subject=Test-Subject-1&body_text=Body-Text-1",
            draft1.id, alias.id
        );

        let request1 = Request::builder()
            .method("POST")
            .uri("/api/v1/emails/send")
            .header(axum::http::header::COOKIE, auth_cookie.clone())
            .header("X-CSRF-Token", csrf_token.clone())
            .header(axum::http::header::CONTENT_TYPE, "application/x-www-form-urlencoded")
            .body(Body::from(payload1))
            .unwrap();

        let response1 = app_router.clone().oneshot(request1).await.unwrap();
        assert_ne!(response1.status(), StatusCode::UNPROCESSABLE_ENTITY, "Form binding failed for draft1!");

        // 6. Send POST request to send the email from the second draft
        let payload2 = format!(
            "draft_id={}&from_alias_id={}&to_email=recipient2@external.com&subject=Test-Subject-2&body_text=Body-Text-2",
            draft2.id, alias.id
        );

        let request2 = Request::builder()
            .method("POST")
            .uri("/api/v1/emails/send")
            .header(axum::http::header::COOKIE, auth_cookie)
            .header("X-CSRF-Token", csrf_token)
            .header(axum::http::header::CONTENT_TYPE, "application/x-www-form-urlencoded")
            .body(Body::from(payload2))
            .unwrap();

        let response2 = app_router.oneshot(request2).await.unwrap();
        assert_ne!(response2.status(), StatusCode::UNPROCESSABLE_ENTITY, "Form binding failed for draft2!");

        // 7. Verify both draft statuses transitioned away from 'draft' in the database
        for draft in &[draft1, draft2] {
            let status_str: String = match &db {
                maileroo::db::DbPool::Postgres(p) => {
                    sqlx::query_scalar("SELECT status::text FROM sent_emails WHERE id = $1")
                        .bind(draft.id)
                        .fetch_one(p)
                        .await
                        .unwrap()
                }
                maileroo::db::DbPool::Sqlite(p) => {
                    sqlx::query_scalar("SELECT status FROM sent_emails WHERE id = ?")
                        .bind(draft.id)
                        .fetch_one(p)
                        .await
                        .unwrap()
                }
            };
            assert_ne!(status_str, "draft", "Draft {} status remained 'draft' after sending!", draft.id);
        }
    }).await;
}

#[tokio::test]
async fn test_save_draft_lifecycle_flow() {
    // Install the Rustls process-level CryptoProvider
    let _ = tokio_rustls::rustls::crypto::aws_lc_rs::default_provider().install_default();

    common::run_on_all_dbs(|db| async move {
        // 1. Setup temporary storage directory
        let temp_storage_dir = tempfile::tempdir().unwrap();

        // 2. Setup mock test user and alias
        let user = common::create_test_user(&db, "test_admin@example.com", "my_secure_password123").await;

        // Grant permissions to the test user to satisfy FirsthandSenderUser extractor
        common::grant_user_sender_permissions(&db, user.id).await;

        let alias = common::create_test_alias(&db, user.id, "example.com", "hello", "dest@gmail.com").await;

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
        let auth_cookie = common::get_auth_cookie(app_router.clone(), "test_admin@example.com", "my_secure_password123").await;

        // Extract CSRF token value
        let csrf_token = common::extract_csrf_token(&auth_cookie);

        // 5. FIRST AUTO-SAVE (draft_id is missing, creating a new draft)
        let payload1 = format!(
            "from_alias_id={}&to_email=recipient@external.com&subject=Autosave-Subject-1&body_text=Autosave-Body-1",
            alias.id
        );

        let request1 = Request::builder()
            .method("POST")
            .uri("/api/v1/emails/drafts")
            .header(axum::http::header::COOKIE, auth_cookie.clone())
            .header("X-CSRF-Token", csrf_token.clone())
            .header(axum::http::header::CONTENT_TYPE, "application/x-www-form-urlencoded")
            .body(Body::from(payload1))
            .unwrap();

        let response1 = app_router.clone().oneshot(request1).await.unwrap();
        assert_eq!(response1.status(), StatusCode::OK);

        // Extract the generated draft ID from the returned HTML response
        let body_bytes1 = axum::body::to_bytes(response1.into_body(), usize::MAX).await.unwrap();
        let body_str1 = String::from_utf8(body_bytes1.to_vec()).unwrap();

        let marker = r#"value=""#;
        let start_idx = body_str1.find(marker).expect("draft_id value not found in HTML response!") + marker.len();
        let end_idx = body_str1[start_idx..].find('"').unwrap() + start_idx;
        let draft_uuid_str = &body_str1[start_idx..end_idx];
        let draft_uuid = Uuid::parse_str(draft_uuid_str).unwrap();

        // 6. SECOND AUTO-SAVE (draft_id is passed, updating the existing draft)
        let payload2 = format!(
            "draft_id={}&from_alias_id={}&to_email=recipient@external.com&subject=Autosave-Subject-2&body_text=Autosave-Body-2",
            draft_uuid, alias.id
        );

        let request2 = Request::builder()
            .method("POST")
            .uri("/api/v1/emails/drafts")
            .header(axum::http::header::COOKIE, auth_cookie)
            .header("X-CSRF-Token", csrf_token)
            .header(axum::http::header::CONTENT_TYPE, "application/x-www-form-urlencoded")
            .body(Body::from(payload2))
            .unwrap();

        let response2 = app_router.oneshot(request2).await.unwrap();
        assert_eq!(response2.status(), StatusCode::OK);

        // Extract and verify that the draft ID remains identical (no new row was created)
        let body_bytes2 = axum::body::to_bytes(response2.into_body(), usize::MAX).await.unwrap();
        let body_str2 = String::from_utf8(body_bytes2.to_vec()).unwrap();

        let start_idx2 = body_str2.find(marker).expect("draft_id value not found in HTML response!") + marker.len();
        let end_idx2 = body_str2[start_idx2..].find('"').unwrap() + start_idx2;
        let draft_uuid_str2 = &body_str2[start_idx2..end_idx2];
        let draft_uuid2 = Uuid::parse_str(draft_uuid_str2).unwrap();

        assert_eq!(draft_uuid, draft_uuid2, "A new draft was wrongly created instead of updating the existing one!");

        // Verify there is exactly one draft in the database for this user
        let draft_count: i64 = match &db {
            maileroo::db::DbPool::Postgres(p) => {
                sqlx::query_scalar("SELECT COUNT(*) FROM sent_emails WHERE user_id = $1 AND status = 'draft'::email_status")
                    .bind(user.id)
                    .fetch_one(p)
                    .await
                    .unwrap()
            }
            maileroo::db::DbPool::Sqlite(p) => {
                sqlx::query_scalar("SELECT COUNT(*) FROM sent_emails WHERE user_id = ? AND status = 'draft'")
                    .bind(user.id)
                    .fetch_one(p)
                    .await
                    .unwrap()
            }
        };
        assert_eq!(draft_count, 1, "Expected exactly 1 draft in the database, found {}", draft_count);
    }).await;
}
