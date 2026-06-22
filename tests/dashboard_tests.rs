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

        // Write the physical files representing draft bodies on disk matching draft.body_key
        let draft1_file_path = temp_storage_dir.path().join(draft1.body_key.to_string());
        tokio::fs::write(&draft1_file_path, b"Mock raw draft body payload 1").await.unwrap();
        assert!(draft1_file_path.exists());

        let draft2_file_path = temp_storage_dir.path().join(draft2.body_key.to_string());
        tokio::fs::write(&draft2_file_path, b"Mock raw draft body payload 2").await.unwrap();
        assert!(draft2_file_path.exists());

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

        // 8. Verify both raw draft files were successfully cleaned up and deleted from disk!
        for draft_file_path in &[draft1_file_path, draft2_file_path] {
            let mut file_deleted = false;
            for _ in 0..100 {
                if !draft_file_path.exists() {
                    file_deleted = true;
                    break;
                }
                tokio::time::sleep(std::time::Duration::from_millis(50)).await;
            }
            assert!(file_deleted, "Storage draft file {:?} was not physically deleted from disk after sending!", draft_file_path);
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

#[tokio::test]
async fn test_send_already_sent_email_fails_and_prevents_file_deletion() {
    common::run_on_all_dbs(|db| async move {
        // 1. Setup temporary storage directory
        let temp_storage_dir = tempfile::tempdir().unwrap();

        // 2. Setup mock test user, alias and an already SENT email (not draft)
        let user = common::create_test_user(&db, "test_admin@example.com", "my_secure_password123").await;
        common::grant_user_sender_permissions(&db, user.id).await;

        let alias = common::create_test_alias(&db, user.id, "example.com", "hello", "dest@gmail.com").await;
        let sent_email = common::create_test_draft(
            &db,
            user.id,
            alias.id,
            "someone@external.com",
            "Already-Sent-Subject",
            EmailStatus::Sent,
        )
        .await;

        // Write the physical file representing the sent email's body on disk matching sent_email.body_key
        let sent_email_file_path = temp_storage_dir.path().join(sent_email.body_key.to_string());
        tokio::fs::write(&sent_email_file_path, b"My precious sent email payload").await.unwrap();
        assert!(sent_email_file_path.exists());

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
        let csrf_token = common::extract_csrf_token(&auth_cookie);

        // 5. Try to POST a request to send this email using the sent_email's ID as the draft_id
        let payload = format!(
            "draft_id={}&from_alias_id={}&to_email=recipient@external.com&subject=Test-Subject&body_text=Stale-Body",
            sent_email.id, alias.id
        );

        let request = Request::builder()
            .method("POST")
            .uri("/api/v1/emails/send")
            .header(axum::http::header::COOKIE, auth_cookie)
            .header("X-CSRF-Token", csrf_token)
            .header(axum::http::header::CONTENT_TYPE, "application/x-www-form-urlencoded")
            .body(Body::from(payload))
            .unwrap();

        let response = app_router.oneshot(request).await.unwrap();

        // 6. Assert that the request failed (the handler now correctly propagates database failure as a 500 Internal Server Error)
        assert_eq!(response.status(), StatusCode::INTERNAL_SERVER_ERROR);

        // 7. Wait a moment and verify that the sent email's body file was NOT deleted!
        tokio::time::sleep(std::time::Duration::from_millis(150)).await;
        assert!(sent_email_file_path.exists(), "The sent email's body file was wrongly deleted from disk!");
    }).await;
}

#[tokio::test]
async fn test_attachment_deletion_and_security() {
    common::run_on_all_dbs(|db| async move {
        // 1. Setup temporary storage directory
        let temp_storage_dir = tempfile::tempdir().unwrap();

        // 2. Setup mock test user, alias and a draft email
        let user = common::create_test_user(&db, "attachment_user@example.com", "password").await;
        let alias = common::create_test_alias(&db, user.id, "example.com", "attach", "dest@gmail.com").await;

        let _email_id = uuid::Uuid::new_v4();
        let body_key = uuid::Uuid::new_v4();
        let received_at_val = time::OffsetDateTime::now_utc();

        // Write a mock raw email body file to the storage directory
        let email_file_path = temp_storage_dir.path().join(format!("{}.eml", body_key));
        let mock_eml_content = b"From: sender@example.com\r\nTo: attach@example.com\r\nSubject: Secret File\r\nContent-Type: multipart/mixed; boundary=bound123\r\n\r\n--bound123\r\nContent-Type: text/plain\r\n\r\nHello\r\n--bound123\r\nContent-Type: text/plain; name=secret.txt\r\nContent-Disposition: attachment; filename=secret.txt\r\n\r\nTopSecretData\r\n--bound123--\r\n";
        tokio::fs::write(&email_file_path, mock_eml_content)
            .await
            .unwrap();

        // Parse and extract metadata
        let (metadata, attachments) = maileroo::inbound::parser::extract_full_metadata(mock_eml_content, "sender@example.com");

        let email = maileroo::db::attachments::insert_email_with_attachments(
            &db,
            alias.id,
            &metadata.sender,
            &metadata.subject,
            body_key,
            Some(received_at_val),
            metadata.message_id,
            None,
            &attachments,
        ).await.unwrap();

        let email_id = email.id;

        // Fetch attachment metadata to verify it was inserted
        let db_attachments = maileroo::db::attachments::get_attachments_for_email(&db, email_id).await.unwrap();
        assert_eq!(db_attachments.len(), 1);
        let attachment_id = db_attachments[0].id;

        // 3. Create App State
        let resolver = hickory_resolver::TokioResolver::builder_tokio().unwrap().build().unwrap();
        let dns_scanner = maileroo::dns::DnsScanner::new(resolver.clone());
        let outbound = std::sync::Arc::new(maileroo::outbound::OutboundService::new(
            "srs_secret_key_123".to_string(),
            resolver,
            "example.com".to_string(),
            db.clone(),
            temp_storage_dir.path().to_path_buf(),
        ));

        let state = maileroo::web::AppState {
            db: db.clone(),
            storage_dir: temp_storage_dir.path().to_path_buf(),
            dns_scanner,
            tx: tokio::sync::broadcast::channel::<maileroo::web::DashboardEvent>(100).0,
            outbound,
            config: maileroo::config::AppConfig { auto_tls: None },
        };

        let app_router = maileroo::web::create_app(state).await;

        // 4. Authenticate via login
        let auth_cookie = common::get_auth_cookie(
            app_router.clone(),
            "attachment_user@example.com",
            "password",
        )
        .await;
        let csrf_token = common::extract_csrf_token(&auth_cookie);

        // Security Test: Accessing the endpoint with proper auth should succeed
        let download_uri = format!("/dashboard/email/{}/attachment/{}", email_id, attachment_id);
        let req = axum::http::Request::builder()
            .method("GET")
            .uri(&download_uri)
            .header(axum::http::header::COOKIE, &auth_cookie)
            .body(axum::body::Body::empty())
            .unwrap();

        let response = app_router.clone().oneshot(req).await.unwrap();
        assert_eq!(response.status(), axum::http::StatusCode::OK);

        let headers = response.headers();
        assert_eq!(headers.get("Content-Disposition").unwrap().to_str().unwrap(), "attachment; filename=\"secret.txt\"");
        assert_eq!(headers.get("X-Content-Type-Options").unwrap().to_str().unwrap(), "nosniff");

        // Security Test: Another user trying to access the endpoint should fail (404/403)
        let _hacker = common::create_test_user(&db, "hacker@example.com", "password").await;
        let hacker_cookie = common::get_auth_cookie(
            app_router.clone(),
            "hacker@example.com",
            "password",
        )
        .await;

        let req = axum::http::Request::builder()
            .method("GET")
            .uri(&download_uri)
            .header(axum::http::header::COOKIE, &hacker_cookie)
            .body(axum::body::Body::empty())
            .unwrap();

        let response = app_router.clone().oneshot(req).await.unwrap();
        assert_eq!(response.status(), axum::http::StatusCode::NOT_FOUND);

        // Deletion Test: Delete the email
        let req = axum::http::Request::builder()
            .method("DELETE")
            .uri(format!("/emails/{}", email_id))
            .header(axum::http::header::COOKIE, &auth_cookie)
            .header("X-CSRF-Token", csrf_token)
            .body(axum::body::Body::empty())
            .unwrap();

        let response = app_router.oneshot(req).await.unwrap();
        assert_eq!(response.status(), axum::http::StatusCode::OK);

        // Verify attachments are gone (cascade)
        let db_attachments_after = maileroo::db::attachments::get_attachments_for_email(&db, email_id).await.unwrap();
        assert_eq!(db_attachments_after.len(), 0, "Attachment rows were not cascaded deleted");

        // Verify file is gone
        let mut file_deleted = false;
        for _ in 0..100 {
            if !email_file_path.exists() {
                file_deleted = true;
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        }
        assert!(file_deleted, "Storage file was not physically deleted from disk!");

    }).await;
}

#[tokio::test]
async fn test_attachment_part_index_resolution() {
    common::run_on_all_dbs(|db| async move {
        let temp_storage_dir = tempfile::tempdir().unwrap();

        let user = common::create_test_user(&db, "partindex@example.com", "password").await;
        let alias = common::create_test_alias(&db, user.id, "example.com", "parts", "dest@gmail.com").await;

        let body_key = uuid::Uuid::new_v4();
        let received_at_val = time::OffsetDateTime::now_utc();

        let email_file_path = temp_storage_dir.path().join(format!("{}.eml", body_key));
        let mock_eml_content = b"From: sender@example.com\r\nTo: parts@example.com\r\nSubject: Files\r\nContent-Type: multipart/mixed; boundary=bound123\r\n\r\n--bound123\r\nContent-Type: text/plain; name=first.txt\r\nContent-Disposition: attachment; filename=first.txt\r\n\r\nFileOne\r\n--bound123\r\nContent-Type: text/plain; name=second.txt\r\nContent-Disposition: attachment; filename=second.txt\r\n\r\nFileTwo\r\n--bound123--\r\n";
        tokio::fs::write(&email_file_path, mock_eml_content).await.unwrap();

        let (metadata, attachments) = maileroo::inbound::parser::extract_full_metadata(mock_eml_content, "sender@example.com");

        let email = maileroo::db::attachments::insert_email_with_attachments(
            &db, alias.id, &metadata.sender, &metadata.subject, body_key,
            Some(received_at_val), metadata.message_id, None, &attachments,
        ).await.unwrap();

        let email_id = email.id;

        let db_attachments = maileroo::db::attachments::get_attachments_for_email(&db, email_id).await.unwrap();
        assert_eq!(db_attachments.len(), 2);

        let att1 = db_attachments.iter().find(|a| a.filename.as_deref() == Some("first.txt")).unwrap();
        let att2 = db_attachments.iter().find(|a| a.filename.as_deref() == Some("second.txt")).unwrap();

        let resolver = hickory_resolver::TokioResolver::builder_tokio().unwrap().build().unwrap();
        let dns_scanner = maileroo::dns::DnsScanner::new(resolver.clone());
        let outbound = std::sync::Arc::new(maileroo::outbound::OutboundService::new(
            "srs_secret_key_123".to_string(), resolver, "example.com".to_string(),
            db.clone(), temp_storage_dir.path().to_path_buf(),
        ));

        let state = maileroo::web::AppState {
            db: db.clone(),
            storage_dir: temp_storage_dir.path().to_path_buf(),
            dns_scanner,
            tx: tokio::sync::broadcast::channel::<maileroo::web::DashboardEvent>(100).0,
            outbound,
            config: maileroo::config::AppConfig { auto_tls: None },
        };

        let app_router = maileroo::web::create_app(state).await;

        let auth_cookie = common::get_auth_cookie(app_router.clone(), "partindex@example.com", "password").await;

        // Fetch First Attachment
        let req1 = axum::http::Request::builder()
            .method("GET")
            .uri(&format!("/dashboard/email/{}/attachment/{}", email_id, att1.id))
            .header(axum::http::header::COOKIE, &auth_cookie)
            .body(axum::body::Body::empty())
            .unwrap();

        let response1 = app_router.clone().oneshot(req1).await.unwrap();
        assert_eq!(response1.status(), axum::http::StatusCode::OK);
        let body1 = axum::body::to_bytes(response1.into_body(), usize::MAX).await.unwrap();
        assert_eq!(&body1[..], b"FileOne");

        // Fetch Second Attachment
        let req2 = axum::http::Request::builder()
            .method("GET")
            .uri(&format!("/dashboard/email/{}/attachment/{}", email_id, att2.id))
            .header(axum::http::header::COOKIE, &auth_cookie)
            .body(axum::body::Body::empty())
            .unwrap();

        let response2 = app_router.oneshot(req2).await.unwrap();
        assert_eq!(response2.status(), axum::http::StatusCode::OK);
        let body2 = axum::body::to_bytes(response2.into_body(), usize::MAX).await.unwrap();
        assert_eq!(&body2[..], b"FileTwo");

    }).await;
}

#[tokio::test]
async fn test_inline_image_e2e_dashboard_render() {
    common::run_on_all_dbs(|db| async move {
        let temp_storage_dir = tempfile::tempdir().unwrap();

        let user = common::create_test_user(&db, "inlinee2e@example.com", "password").await;
        let alias = common::create_test_alias(&db, user.id, "example.com", "inline", "dest@gmail.com").await;

        let body_key = uuid::Uuid::new_v4();
        let received_at_val = time::OffsetDateTime::now_utc();

        let email_file_path = temp_storage_dir.path().join(format!("{}.eml", body_key));
        let mock_eml_content = b"From: sender@example.com\r\nTo: inline@example.com\r\nSubject: Images\r\nContent-Type: multipart/related; boundary=bound123\r\n\r\n--bound123\r\nContent-Type: text/html\r\n\r\n<html><body><img src=\"cid:logo-123\"></body></html>\r\n--bound123\r\nContent-Type: image/png\r\nContent-ID: <logo-123>\r\n\r\nFakeImageBytes\r\n--bound123--\r\n";
        tokio::fs::write(&email_file_path, mock_eml_content).await.unwrap();

        let (metadata, attachments) = maileroo::inbound::parser::extract_full_metadata(mock_eml_content, "sender@example.com");

        let email = maileroo::db::attachments::insert_email_with_attachments(
            &db, alias.id, &metadata.sender, &metadata.subject, body_key,
            Some(received_at_val), metadata.message_id, None, &attachments,
        ).await.unwrap();

        let email_id = email.id;

        let resolver = hickory_resolver::TokioResolver::builder_tokio().unwrap().build().unwrap();
        let dns_scanner = maileroo::dns::DnsScanner::new(resolver.clone());
        let outbound = std::sync::Arc::new(maileroo::outbound::OutboundService::new(
            "srs_secret_key_123".to_string(), resolver, "example.com".to_string(),
            db.clone(), temp_storage_dir.path().to_path_buf(),
        ));

        let state = maileroo::web::AppState {
            db: db.clone(),
            storage_dir: temp_storage_dir.path().to_path_buf(),
            dns_scanner,
            tx: tokio::sync::broadcast::channel::<maileroo::web::DashboardEvent>(100).0,
            outbound,
            config: maileroo::config::AppConfig { auto_tls: None },
        };

        let app_router = maileroo::web::create_app(state).await;
        let auth_cookie = common::get_auth_cookie(app_router.clone(), "inlinee2e@example.com", "password").await;

        // Fetch Email Detail Page
        let req = axum::http::Request::builder()
            .method("GET")
            .uri(&format!("/emails/{}", email_id))
            .header(axum::http::header::COOKIE, &auth_cookie)
            .body(axum::body::Body::empty())
            .unwrap();

        let response = app_router.clone().oneshot(req).await.unwrap();
        assert_eq!(response.status(), axum::http::StatusCode::OK);

        let html_bytes = axum::body::to_bytes(response.into_body(), usize::MAX).await.unwrap();
        let html_str = String::from_utf8_lossy(&html_bytes);
        // Verify HTML body cid: rewrite (Askama escapes " as &#34;)
        assert!(html_str.contains(&format!("src=&#34;/dashboard/email/{}/inline/logo-123&#34;", email_id)));

        // Verify Inline endpoint
        let inline_req = axum::http::Request::builder()
            .method("GET")
            .uri(&format!("/dashboard/email/{}/inline/logo-123", email_id))
            .header(axum::http::header::COOKIE, &auth_cookie)
            .body(axum::body::Body::empty())
            .unwrap();

        let inline_resp = app_router.oneshot(inline_req).await.unwrap();
        let status = inline_resp.status();
        let img_bytes = axum::body::to_bytes(inline_resp.into_body(), usize::MAX).await.unwrap();
        if status != axum::http::StatusCode::OK {
            println!("INLINE ERROR: {}", String::from_utf8_lossy(&img_bytes));
        }
        assert_eq!(status, axum::http::StatusCode::OK);
        assert_eq!(&img_bytes[..], b"FakeImageBytes");

    }).await;
}
