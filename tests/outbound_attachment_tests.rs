mod common;

use axum::body::Body;
use axum::http::{Request, StatusCode, header};
use maileroo::config::AppConfig;
use maileroo::db::attachments::get_attachments_for_email;
use maileroo::dns::DnsScanner;
use maileroo::outbound::OutboundService;
use maileroo::web::{AppState, DashboardEvent, create_app};
use std::sync::Arc;
use tower::ServiceExt;

fn build_multipart_request(
    boundary: &str,
    from_alias_id: &str,
    to_email: &str,
    subject: &str,
    body_text: &str,
    files: Vec<(&str, &str, &[u8])>,
) -> (Vec<u8>, String) {
    let mut body = Vec::new();

    let mut append_text = |name: &str, value: &str| {
        body.extend_from_slice(format!("--{}\r\n", boundary).as_bytes());
        body.extend_from_slice(
            format!("Content-Disposition: form-data; name=\"{}\"\r\n\r\n", name).as_bytes(),
        );
        body.extend_from_slice(format!("{}\r\n", value).as_bytes());
    };

    append_text("from_alias_id", from_alias_id);
    append_text("to_email", to_email);
    append_text("subject", subject);
    append_text("body_text", body_text);

    for (filename, ctype, bytes) in files {
        body.extend_from_slice(format!("--{}\r\n", boundary).as_bytes());
        body.extend_from_slice(
            format!(
                "Content-Disposition: form-data; name=\"attachments\"; filename=\"{}\"\r\n",
                filename
            )
            .as_bytes(),
        );
        body.extend_from_slice(format!("Content-Type: {}\r\n\r\n", ctype).as_bytes());
        body.extend_from_slice(bytes);
        body.extend_from_slice(b"\r\n");
    }

    body.extend_from_slice(format!("--{}--\r\n", boundary).as_bytes());

    let content_type_header = format!("multipart/form-data; boundary={}", boundary);
    (body, content_type_header)
}

#[tokio::test]
async fn test_outbound_attachments_happy_path() {
    common::run_on_all_dbs(|db| async move {
        let temp_storage_dir = tempdir_cleanup_helper();

        let user = common::create_test_user(&db, "sender@example.com", "password").await;
        common::grant_user_sender_permissions(&db, user.id).await;

        let alias =
            common::create_test_alias(&db, user.id, "example.com", "hello", "dest@gmail.com").await;

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
        let auth_cookie =
            common::get_auth_cookie(app_router.clone(), "sender@example.com", "password").await;
        let csrf_token = common::extract_csrf_token(&auth_cookie);

        // 1. Prepare multipart body with two attachments
        let boundary = "testboundary123";
        let file1_bytes = b"Hello, this is file one content!";
        let file2_bytes = vec![0x00, 0x01, 0x02, 0x03, 0x04];
        let files = vec![
            ("file1.txt", "text/plain", file1_bytes.as_slice()),
            (
                "file2.bin",
                "application/octet-stream",
                file2_bytes.as_slice(),
            ),
        ];

        let (body, content_type) = build_multipart_request(
            boundary,
            &alias.id.to_string(),
            "recipient@example.com",
            "Outbound attachments test",
            "This is the message body.",
            files,
        );

        // 2. Send email via POST /api/v1/emails/send
        let req = Request::builder()
            .method("POST")
            .uri("/api/v1/emails/send")
            .header(header::COOKIE, &auth_cookie)
            .header("X-CSRF-Token", csrf_token)
            .header(header::CONTENT_TYPE, content_type)
            .body(Body::from(body))
            .unwrap();

        let response = app_router.clone().oneshot(req).await.unwrap();
        assert_eq!(response.status(), StatusCode::OK);

        // Retrieve the generated email from database
        let sent_emails = match &db {
            maileroo::db::DbPool::Postgres(pool) => {
                sqlx::query_as::<_, maileroo::db::sent_emails::SentEmailRow>(
                    "SELECT s.*, a.subdomain || '@' || d.name as alias_address
                     FROM sent_emails s
                     JOIN aliases a ON s.from_alias_id = a.id
                     JOIN domains d ON a.domain_id = d.id",
                )
                .fetch_all(pool)
                .await
                .unwrap()
            }
            maileroo::db::DbPool::Sqlite(pool) => {
                sqlx::query_as::<sqlx::Sqlite, maileroo::db::sent_emails::SentEmailRow>(
                    "SELECT s.*, a.subdomain || '@' || d.name as alias_address
                     FROM sent_emails s
                     JOIN aliases a ON s.from_alias_id = a.id
                     JOIN domains d ON a.domain_id = d.id",
                )
                .fetch_all(pool)
                .await
                .unwrap()
            }
        };

        assert_eq!(sent_emails.len(), 1);
        let sent_email = &sent_emails[0];
        assert_eq!(sent_email.has_attachments, true);

        // 3. Fetch attachment rows
        let attachments = get_attachments_for_email(&db, sent_email.id).await.unwrap();
        assert_eq!(attachments.len(), 2);

        // Assert file1
        let att1 = &attachments[0];
        assert_eq!(att1.filename.as_deref(), Some("file1.txt"));
        assert_eq!(att1.content_type.as_deref(), Some("text/plain"));
        assert_eq!(att1.size_bytes, file1_bytes.len() as i64);
        assert_eq!(att1.part_index, 0);

        // Assert file2
        let att2 = &attachments[1];
        assert_eq!(att2.filename.as_deref(), Some("file2.bin"));
        assert_eq!(
            att2.content_type.as_deref(),
            Some("application/octet-stream")
        );
        assert_eq!(att2.size_bytes, file2_bytes.len() as i64);
        assert_eq!(att2.part_index, 1);

        // 4. Download file1 and file2 to verify bytes and headers
        let req_dl1 = Request::builder()
            .method("GET")
            .uri(&format!(
                "/dashboard/email/{}/attachment/{}",
                sent_email.id, att1.id
            ))
            .header(header::COOKIE, &auth_cookie)
            .body(Body::empty())
            .unwrap();

        let resp_dl1 = app_router.clone().oneshot(req_dl1).await.unwrap();
        assert_eq!(resp_dl1.status(), StatusCode::OK);
        assert_eq!(
            resp_dl1
                .headers()
                .get(header::CONTENT_DISPOSITION)
                .unwrap()
                .to_str()
                .unwrap(),
            "attachment; filename=\"file1.txt\""
        );
        assert_eq!(
            resp_dl1
                .headers()
                .get(header::CONTENT_TYPE)
                .unwrap()
                .to_str()
                .unwrap(),
            "application/octet-stream"
        );
        assert_eq!(
            resp_dl1
                .headers()
                .get(header::X_CONTENT_TYPE_OPTIONS)
                .unwrap()
                .to_str()
                .unwrap(),
            "nosniff"
        );
        let body_dl1 = axum::body::to_bytes(resp_dl1.into_body(), usize::MAX)
            .await
            .unwrap();
        assert_eq!(&body_dl1[..], file1_bytes);

        let req_dl2 = Request::builder()
            .method("GET")
            .uri(&format!(
                "/dashboard/email/{}/attachment/{}",
                sent_email.id, att2.id
            ))
            .header(header::COOKIE, &auth_cookie)
            .body(Body::empty())
            .unwrap();

        let resp_dl2 = app_router.clone().oneshot(req_dl2).await.unwrap();
        assert_eq!(resp_dl2.status(), StatusCode::OK);
        let body_dl2 = axum::body::to_bytes(resp_dl2.into_body(), usize::MAX)
            .await
            .unwrap();
        assert_eq!(&body_dl2[..], file2_bytes.as_slice());
    })
    .await;
}

#[tokio::test]
async fn test_outbound_attachments_limits_validation() {
    common::run_on_all_dbs(|db| async move {
        let temp_storage_dir = tempdir_cleanup_helper();

        let user = common::create_test_user(&db, "limit@example.com", "password").await;
        common::grant_user_sender_permissions(&db, user.id).await;

        let alias =
            common::create_test_alias(&db, user.id, "example.com", "limit", "dest@gmail.com").await;

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
        let auth_cookie =
            common::get_auth_cookie(app_router.clone(), "limit@example.com", "password").await;
        let csrf_token = common::extract_csrf_token(&auth_cookie);

        // Create an attachment that exceeds the 10 MB per-file limit (e.g. 11 MB)
        let large_bytes = vec![0; 11 * 1024 * 1024];

        let boundary = "limitboundary";
        let files = vec![(
            "huge.bin",
            "application/octet-stream",
            large_bytes.as_slice(),
        )];

        let (body, content_type) = build_multipart_request(
            boundary,
            &alias.id.to_string(),
            "recipient@example.com",
            "Too large",
            "This has a huge attachment.",
            files,
        );

        let req = Request::builder()
            .method("POST")
            .uri("/api/v1/emails/send")
            .header(header::COOKIE, &auth_cookie)
            .header("X-CSRF-Token", csrf_token)
            .header(header::CONTENT_TYPE, content_type)
            .body(Body::from(body))
            .unwrap();

        let response = app_router.clone().oneshot(req).await.unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let body_bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        let body_str = String::from_utf8_lossy(&body_bytes);
        assert!(body_str.contains("Attachment exceeds maximum limit"));
    })
    .await;
}

#[tokio::test]
async fn test_outbound_attachments_deletion_cleanup() {
    common::run_on_all_dbs(|db| async move {
        let temp_storage_dir = tempdir_cleanup_helper();

        let user = common::create_test_user(&db, "deleter@example.com", "password").await;
        common::grant_user_sender_permissions(&db, user.id).await;

        let alias =
            common::create_test_alias(&db, user.id, "example.com", "del", "dest@gmail.com").await;

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
        let auth_cookie =
            common::get_auth_cookie(app_router.clone(), "deleter@example.com", "password").await;
        let csrf_token = common::extract_csrf_token(&auth_cookie);

        // 1. Send an email with attachments
        let boundary = "delboundary";
        let file_bytes = b"Cleanup test bytes";
        let files = vec![("cleanup.txt", "text/plain", file_bytes.as_slice())];

        let (body, content_type) = build_multipart_request(
            boundary,
            &alias.id.to_string(),
            "recipient@example.com",
            "Delete cleanup",
            "Body content",
            files,
        );

        let req = Request::builder()
            .method("POST")
            .uri("/api/v1/emails/send")
            .header(header::COOKIE, &auth_cookie)
            .header("X-CSRF-Token", &csrf_token)
            .header(header::CONTENT_TYPE, content_type)
            .body(Body::from(body))
            .unwrap();

        let response = app_router.clone().oneshot(req).await.unwrap();
        assert_eq!(response.status(), StatusCode::OK);

        // Retrieve the generated email from database
        let sent_emails = match &db {
            maileroo::db::DbPool::Postgres(pool) => {
                sqlx::query_as::<_, maileroo::db::sent_emails::SentEmailRow>(
                    "SELECT s.*, a.subdomain || '@' || d.name as alias_address
                     FROM sent_emails s
                     JOIN aliases a ON s.from_alias_id = a.id
                     JOIN domains d ON a.domain_id = d.id",
                )
                .fetch_all(pool)
                .await
                .unwrap()
            }
            maileroo::db::DbPool::Sqlite(pool) => {
                sqlx::query_as::<sqlx::Sqlite, maileroo::db::sent_emails::SentEmailRow>(
                    "SELECT s.*, a.subdomain || '@' || d.name as alias_address
                     FROM sent_emails s
                     JOIN aliases a ON s.from_alias_id = a.id
                     JOIN domains d ON a.domain_id = d.id",
                )
                .fetch_all(pool)
                .await
                .unwrap()
            }
        };

        assert_eq!(sent_emails.len(), 1);
        let sent_email = &sent_emails[0];

        // Ensure attachments exist
        let attachments = get_attachments_for_email(&db, sent_email.id).await.unwrap();
        assert_eq!(attachments.len(), 1);

        // 2. Perform delete via DELETE /emails/{id}
        let del_req = Request::builder()
            .method("DELETE")
            .uri(&format!("/emails/{}", sent_email.id))
            .header(header::COOKIE, &auth_cookie)
            .header("X-CSRF-Token", &csrf_token)
            .body(Body::empty())
            .unwrap();

        let del_response = app_router.oneshot(del_req).await.unwrap();
        assert_eq!(del_response.status(), StatusCode::OK);

        // 3. Assert that attachment rows are cleaned up
        let post_del_attachments = get_attachments_for_email(&db, sent_email.id).await.unwrap();
        assert!(post_del_attachments.is_empty());
    })
    .await;
}

/// Helper to create a temp directory that cleans up after being dropped,
/// avoiding leaks across parallel test executions.
fn tempdir_cleanup_helper() -> tempfile::TempDir {
    tempfile::tempdir().unwrap()
}
