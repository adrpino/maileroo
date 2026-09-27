//! API-level integration tests for the JSON send/reply endpoints.
//!
//! Outbound delivery is exercised over a real TCP socket against an
//! in-process fake SMTP relay sink, so everything between the JSON request
//! and the SMTP wire (auth, alias resolution, validation, MIME construction,
//! disk persistence, database rows) runs the production code paths.

mod common;

use axum::body::Body;
use axum::http::StatusCode;
use common::{FakeSmtpSink, run_on_all_dbs};
use serde_json::{Value, json};
use tower::ServiceExt;
use uuid::Uuid;

/// Raw API key known only to the test; only its SHA-256 hash is stored.
const API_KEY: &str = "test-api-key-0123456789abcdef";

fn sha256_hex(input: &str) -> String {
    use sha2::{Digest, Sha256};
    let mut hasher = Sha256::new();
    hasher.update(input.as_bytes());
    format!("{:x}", hasher.finalize())
}

/// Creates a user with firsthand sending enabled plus a domain and alias.
/// Each caller uses a distinct domain name to avoid UNIQUE collisions.
async fn setup_sender(
    db: &maileroo::db::DbPool,
    local_part: &str,
) -> (maileroo::db::users::User, maileroo::db::Alias) {
    let user =
        common::create_test_user(db, &format!("{}@example.com", local_part), "password").await;
    maileroo::db::users::toggle_can_send_firsthand(db, user.id)
        .await
        .unwrap();
    let alias = common::create_test_alias(
        db,
        user.id,
        &format!("{}.test", local_part),
        local_part,
        "dest@example.org",
    )
    .await;
    (user, alias)
}

/// Inserts an API key whose hash matches the well-known test key.
async fn create_api_key(db: &maileroo::db::DbPool, user_id: Uuid) {
    maileroo::db::insert_api_key(db, user_id, &sha256_hex(API_KEY), "integration key")
        .await
        .unwrap();
}

/// App-level count helper that works on both engines.
async fn scalar_count(db: &maileroo::db::DbPool, query: &str) -> i64 {
    match db {
        maileroo::db::DbPool::Postgres(p) => sqlx::query_scalar::<_, i64>(query)
            .fetch_one(p)
            .await
            .unwrap(),
        maileroo::db::DbPool::Sqlite(p) => sqlx::query_scalar::<_, i64>(query)
            .fetch_one(p)
            .await
            .unwrap(),
    }
}

/// Builds the router with outbound routed through the fake relay sink.
async fn build_relay_app(
    db: &maileroo::db::DbPool,
    storage_dir: std::path::PathBuf,
    sink: &FakeSmtpSink,
) -> axum::Router {
    common::init_crypto_provider();

    let resolver = hickory_resolver::TokioResolver::builder_tokio()
        .unwrap()
        .build()
        .unwrap();
    let dns_scanner = maileroo::dns::DnsScanner::new(resolver.clone());
    let outbound = std::sync::Arc::new(
        maileroo::outbound::OutboundService::new(
            "srs_secret_key_123".to_string(),
            resolver,
            "example.com".to_string(),
            db.clone(),
            storage_dir.clone(),
        )
        .with_relay_override(sink.relay_config()),
    );

    let state = maileroo::web::AppState {
        db: db.clone(),
        storage_dir,
        dns_scanner,
        tx: tokio::sync::broadcast::channel::<maileroo::web::DashboardEvent>(100).0,
        outbound,
        config: maileroo::config::AppConfig { auto_tls: None },
        filter_engine: maileroo::filter::FilterEngine::default(),
        backfill_engine: maileroo::filter::BackfillEngine::default(),
    };

    maileroo::web::create_app(state).await
}

/// Issues a JSON POST with API auth headers.
async fn api_post_json(app: &axum::Router, uri: &str, body: Value) -> axum::response::Response {
    app.clone()
        .oneshot(
            axum::http::Request::builder()
                .method("POST")
                .uri(uri)
                .header(
                    axum::http::header::AUTHORIZATION,
                    format!("Bearer {}", API_KEY),
                )
                .header(axum::http::header::CONTENT_TYPE, "application/json")
                .extension(axum::extract::ConnectInfo(std::net::SocketAddr::from((
                    [127, 0, 0, 1],
                    12345,
                ))))
                .body(Body::from(body.to_string()))
                .unwrap(),
        )
        .await
        .unwrap()
}

/// Issues a GET with API auth headers.
async fn api_get(app: &axum::Router, uri: &str) -> axum::response::Response {
    app.clone()
        .oneshot(
            axum::http::Request::builder()
                .method("GET")
                .uri(uri)
                .header(
                    axum::http::header::AUTHORIZATION,
                    format!("Bearer {}", API_KEY),
                )
                .extension(axum::extract::ConnectInfo(std::net::SocketAddr::from((
                    [127, 0, 0, 1],
                    12345,
                ))))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap()
}

/// Inserts a received email carrying the given Message-ID and writes its
/// .eml into `storage_dir` so replies/detail can parse it. Returns the id.
async fn seed_received_email(
    db: &maileroo::db::DbPool,
    storage_dir: &std::path::Path,
    alias_id: Uuid,
    message_id: &str,
) -> Uuid {
    let body_key = Uuid::new_v4();
    let mock_eml = format!(
        "From: sender@example.org\r\nTo: alias@example.com\r\nSubject: Parent\r\nMessage-ID: {}\r\n\r\nOriginal body",
        message_id
    );
    let eml_path = storage_dir.join(format!("{}.eml", body_key));
    tokio::fs::write(&eml_path, mock_eml.as_bytes())
        .await
        .unwrap();

    let (metadata, _) =
        maileroo::inbound::parser::extract_full_metadata(mock_eml.as_bytes(), "sender@example.org");

    let email = maileroo::db::attachments::insert_email_with_attachments(
        db,
        alias_id,
        &metadata.sender,
        &metadata.subject,
        body_key,
        None,
        Some(message_id.to_string()),
        None,
        &[],
    )
    .await
    .unwrap();
    email.id
}

fn base64_of(data: &[u8]) -> String {
    use base64::Engine;
    base64::engine::general_purpose::STANDARD.encode(data)
}

fn send_payload(from_alias: &str, dry_run: bool, attachments: Option<Value>) -> Value {
    json!({
        "from_alias": from_alias,
        "to": "recipient@example.org",
        "subject": "API send test",
        "body": "Hello from the API.",
        "dry_run": dry_run,
        "attachments": attachments,
    })
}

/// A real send via the JSON API must traverse the actual SMTP client code
/// into the fake sink with the right envelope, and persist a sent row.
#[tokio::test]
async fn test_api_send_delivers_smtp() {
    run_on_all_dbs(|db| async move {
        let sink = FakeSmtpSink::start().await;
        let temp = tempfile::tempdir().unwrap();

        let (_user, _alias) = setup_sender(&db, "sendapi").await;
        create_api_key(&db, _user.id).await;

        let app = build_relay_app(&db, temp.path().to_path_buf(), &sink).await;

        let res = api_post_json(
            &app,
            "/api/v1/emails/send",
            send_payload("sendapi@sendapi.test", false, None),
        )
        .await;
        assert_eq!(
            res.status(),
            StatusCode::OK,
            "send must succeed: {}",
            res.headers()
                .get("location")
                .map(|v| v.to_str().unwrap().to_string())
                .unwrap_or_default()
        );

        // 1. Exactly one message captured over the wire.
        let messages = sink.messages().await;
        assert_eq!(messages.len(), 1);
        let msg = &messages[0];
        assert_eq!(msg.rcpt_to, "recipient@example.org");
        assert_eq!(msg.mail_from, "sendapi@sendapi.test");

        // 2. Captured bytes are a parseable message with our headers.
        let parsed = mail_parser::MessageParser::default()
            .parse(&msg.body)
            .expect("captured bytes must parse as MIME");
        assert_eq!(parsed.subject().unwrap_or_default(), "API send test");
        assert_eq!(
            parsed
                .from()
                .and_then(|addr| addr.first())
                .and_then(|a| a.address()),
            Some("sendapi@sendapi.test")
        );
        assert!(String::from_utf8_lossy(&msg.body).contains("Hello from the API."));

        // 3. One sent row persisted.
        assert_eq!(
            scalar_count(&db, "SELECT COUNT(*) FROM sent_emails").await,
            1
        );
    })
    .await;
}

/// A dry-run send must stop before any side effect: nothing on the wire,
/// no database rows, response reports what would have happened.
#[tokio::test]
async fn test_api_send_dry_run_has_no_side_effects() {
    run_on_all_dbs(|db| async move {
        let sink = FakeSmtpSink::start().await;
        let temp = tempfile::tempdir().unwrap();

        let (user, _alias) = setup_sender(&db, "dryrun").await;
        create_api_key(&db, user.id).await;

        let app = build_relay_app(&db, temp.path().to_path_buf(), &sink).await;

        let res = api_post_json(
            &app,
            "/api/v1/emails/send",
            send_payload("dryrun@dryrun.test", true, None),
        )
        .await;
        assert_eq!(res.status(), StatusCode::OK);

        let body_bytes = axum::body::to_bytes(res.into_body(), usize::MAX)
            .await
            .unwrap();
        let body: Value = serde_json::from_slice(&body_bytes).expect("JSON response");
        assert_eq!(body["status"], "dry_run");
        assert_eq!(body["resolved_from"], "dryrun@dryrun.test");
        assert_eq!(body["to"], "recipient@example.org");
        assert_eq!(body["subject"], "API send test");
        assert_eq!(body["attachment_count"], 0);

        // Side-effect assertions: no SMTP traffic, no DB rows.
        assert!(
            sink.messages().await.is_empty(),
            "dry run must not transmit"
        );
        assert_eq!(
            scalar_count(&db, "SELECT COUNT(*) FROM sent_emails").await,
            0,
            "dry run must not persist rows"
        );
    })
    .await;
}

/// Sending with base64 attachments must produce byte-exact MIME parts on the
/// wire and matching attachment rows in the database.
#[tokio::test]
async fn test_api_send_with_attachments() {
    run_on_all_dbs(|db| async move {
        let sink = FakeSmtpSink::start().await;
        let temp = tempfile::tempdir().unwrap();

        let (user, _alias) = setup_sender(&db, "attapi").await;
        create_api_key(&db, user.id).await;

        let app = build_relay_app(&db, temp.path().to_path_buf(), &sink).await;

        // A deterministic payload we can assert byte-for-byte after transit.
        let pdf_bytes: Vec<u8> = vec![0x25, 0x50, 0x44, 0x46, 0x2d, 1, 2, 3, 4, 5];
        let attachments = json!([
            {
                "filename": "report.pdf",
                "content_b64": base64_of(&pdf_bytes),
                "content_type": "application/pdf"
            }
        ]);

        let res = api_post_json(
            &app,
            "/api/v1/emails/send",
            send_payload("attapi@attapi.test", false, Some(attachments)),
        )
        .await;
        assert_eq!(res.status(), StatusCode::OK);

        let messages = sink.messages().await;
        assert_eq!(messages.len(), 1);
        let body_str = String::from_utf8_lossy(&messages[0].body);

        // 1. MIME declares a mixed multipart with a PDF part.
        assert!(body_str.contains("multipart/mixed"));
        assert!(body_str.contains("application/pdf"));

        // 2. Parsing the captured message recovers the exact bytes.
        let parsed = mail_parser::MessageParser::default()
            .parse(&messages[0].body)
            .unwrap();
        let attachment_contents: Vec<&[u8]> = parsed.attachments().map(|p| p.contents()).collect();
        assert_eq!(attachment_contents.len(), 1);
        assert_eq!(attachment_contents[0], pdf_bytes.as_slice());

        // 3. Metadata rows persisted and flagged.
        assert_eq!(
            scalar_count(&db, "SELECT COUNT(*) FROM attachments").await,
            1
        );
    })
    .await;
}

/// Replying through the API with attachments must thread against the
/// original Message-ID and deliver byte-exact attachment parts.
#[tokio::test]
async fn test_api_reply_with_attachments() {
    run_on_all_dbs(|db| async move {
        let sink = FakeSmtpSink::start().await;
        let temp = tempfile::tempdir().unwrap();

        let (user, alias) = setup_sender(&db, "replyatt").await;
        create_api_key(&db, user.id).await;

        let original_message_id = "<parent-seed@example.org>";
        let email_id = seed_received_email(&db, temp.path(), alias.id, original_message_id).await;

        let app = build_relay_app(&db, temp.path().to_path_buf(), &sink).await;

        let pdf_bytes: Vec<u8> = vec![9, 8, 7, 6, 5, 4, 3, 2, 1];
        let payload = json!({
            "body_text": "Replying with the file.",
            "dry_run": false,
            "attachments": [
                {
                    "filename": "extra.bin",
                    "content_b64": base64_of(&pdf_bytes),
                    "content_type": "application/octet-stream"
                }
            ]
        });

        let res = api_post_json(&app, &format!("/api/v1/emails/{}/reply", email_id), payload).await;
        assert_eq!(res.status(), StatusCode::OK);

        let messages = sink.messages().await;
        assert_eq!(messages.len(), 1, "reply must be transmitted once");
        assert_eq!(messages[0].rcpt_to, "sender@example.org");

        // Thread header points at the original message.
        let body_str = String::from_utf8_lossy(&messages[0].body);
        assert!(body_str.contains("multipart/mixed"));
        assert!(body_str.contains(&format!("In-Reply-To: {}", original_message_id)));

        // Byte-exact attachment after transit.
        let parsed = mail_parser::MessageParser::default()
            .parse(&messages[0].body)
            .unwrap();
        let attachment_contents: Vec<&[u8]> = parsed.attachments().map(|p| p.contents()).collect();
        assert_eq!(attachment_contents.len(), 1);
        assert_eq!(attachment_contents[0], pdf_bytes.as_slice());

        // Reply and its attachment row persisted.
        assert_eq!(
            scalar_count(&db, "SELECT COUNT(*) FROM email_replies").await,
            1
        );
        assert_eq!(
            scalar_count(
                &db,
                "SELECT COUNT(*) FROM attachments WHERE reply_id IS NOT NULL"
            )
            .await,
            1
        );
    })
    .await;
}

/// Cross-account isolation: a caller can neither send from an alias it does
/// not own nor reply to an email owned by another account.
#[tokio::test]
async fn test_api_send_and_reply_reject_cross_account_access() {
    run_on_all_dbs(|db| async move {
        let sink = FakeSmtpSink::start().await;
        let temp = tempfile::tempdir().unwrap();

        let (_owner, owner_alias) = setup_sender(&db, "owner").await;
        let email_id = seed_received_email(
            &db,
            temp.path(),
            owner_alias.id,
            "<owner-thread@example.org>",
        )
        .await;

        // Second user gets the API key used for the calls below.
        let (intruder, _intruder_alias) = setup_sender(&db, "intruder").await;
        create_api_key(&db, intruder.id).await;

        let app = build_relay_app(&db, temp.path().to_path_buf(), &sink).await;

        // 1. Sending from another user's alias is rejected.
        let res = api_post_json(
            &app,
            "/api/v1/emails/send",
            send_payload("owner@owner.test", false, None),
        )
        .await;
        assert_eq!(res.status(), StatusCode::NOT_FOUND);

        // 2. Replying to another user's email is rejected.
        let res = api_post_json(
            &app,
            &format!("/api/v1/emails/{}/reply", email_id),
            json!({ "body_text": "sneaky" }),
        )
        .await;
        assert_eq!(res.status(), StatusCode::NOT_FOUND);

        // 3. Nothing went out, nothing was recorded.
        assert!(sink.messages().await.is_empty());
    })
    .await;
}

/// The aliases listing must expose full addresses the send endpoint accepts,
/// and the email detail endpoint must round-trip a seeded email.
#[tokio::test]
async fn test_api_aliases_list_and_email_detail() {
    run_on_all_dbs(|db| async move {
        let sink = FakeSmtpSink::start().await;
        let temp = tempfile::tempdir().unwrap();

        let (user, alias) = setup_sender(&db, "listapi").await;
        create_api_key(&db, user.id).await;

        let email_id =
            seed_received_email(&db, temp.path(), alias.id, "<detail@example.org>").await;

        let app = build_relay_app(&db, temp.path().to_path_buf(), &sink).await;

        // 1. Aliases listing contains the full alias address.
        let res = api_get(&app, "/api/v1/aliases").await;
        assert_eq!(res.status(), StatusCode::OK);
        let bytes = axum::body::to_bytes(res.into_body(), usize::MAX)
            .await
            .unwrap();
        let body: Value = serde_json::from_slice(&bytes).unwrap();
        let addresses: Vec<&str> = body["aliases"]
            .as_array()
            .unwrap()
            .iter()
            .map(|a| a["address"].as_str().unwrap())
            .collect();
        assert!(addresses.contains(&"listapi@listapi.test"));

        // 2. Email detail round-trips subject, body, and ownership flags.
        let res = api_get(&app, &format!("/api/v1/emails/{}", email_id)).await;
        assert_eq!(res.status(), StatusCode::OK);
        let bytes = axum::body::to_bytes(res.into_body(), usize::MAX)
            .await
            .unwrap();
        let detail: Value = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(detail["id"], email_id.to_string());
        assert_eq!(detail["subject"], "Parent");
        assert_eq!(detail["is_sent"], false);
        assert!(
            detail["body"]
                .as_str()
                .unwrap_or_default()
                .contains("Original body")
        );
    })
    .await;
}
