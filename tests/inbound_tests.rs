mod common;

use axum::{body::Body, http::{Request, StatusCode}};
use maileroo::db::DbPool;
use maileroo::inbound::acceptor::HotReloadAcceptor;
use maileroo::inbound::protocol::SmtpSession;
use maileroo::inbound::rate_limit::RateLimiter;
use maileroo::outbound::OutboundService;
use maileroo::web::{AppState, DashboardEvent, create_app};
use std::sync::Arc;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::broadcast;
use tower::util::ServiceExt;

#[tokio::test]
async fn test_complete_smtp_to_http_dashboard_lifecycle() {
    common::run_on_all_dbs(|db| async move {
        // 1. Setup isolated temp directory for raw emails
        let temp_dir = tempfile::tempdir().unwrap();

        // 2. Setup user, domain and alias using our shared common fixtures!
        let user = common::create_test_user(&db, "developer@example.com", "password").await;
        let alias = common::create_test_alias(&db, user.id, "example.com", "hello", "dest@gmail.com").await;

        // 3. Create App State
        let (tx, _) = broadcast::channel::<DashboardEvent>(100);
        
        // Generate temporary certificates in the temp_dir instead of hardcoding ./certs
        let cert_path = temp_dir.path().join("smtp_cert.pem");
        let key_path = temp_dir.path().join("smtp_key.pem");
        common::generate_dummy_certs(&cert_path, &key_path);
        let tls_acceptor = HotReloadAcceptor::new(cert_path, key_path, std::time::Duration::from_millis(100)).unwrap();
        let outbound = Arc::new(OutboundService::new(
            "srs_secret_key_123".to_string(),
            hickory_resolver::TokioResolver::builder_tokio().unwrap().build().unwrap(),
            "example.com".to_string(),
            db.clone(),
            temp_dir.path().to_path_buf(),
        ));

        let state = AppState {
            db: db.clone(),
            storage_dir: temp_dir.path().to_path_buf(),
            dns_scanner: maileroo::dns::DnsScanner::new(
                hickory_resolver::TokioResolver::builder_tokio().unwrap().build().unwrap()
            ),
            tx: tx.clone(),
            outbound: outbound.clone(),
            config: maileroo::config::AppConfig { auto_tls: None },
        };

        // 4. Spin up real TCP loopback listener on dynamic port 0
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let local_addr = listener.local_addr().unwrap();

        let rate_limiter = Arc::new(RateLimiter::new());
        let db_clone = db.clone();
        let storage_clone = temp_dir.path().to_path_buf();
        let outbound_clone = outbound.clone();
        let tx_clone = tx.clone();
        let rate_limiter_clone = rate_limiter.clone();

        let block_path = temp_dir.path().join("blockips.conf");
        let blocklist = Arc::new(maileroo::inbound::blocklist::Blocklist::new(block_path));
        let limits = maileroo::inbound::rate_limit::InboundLimits {
            tarpit_threshold: 5,
            block_threshold: 10,
            tarpit_duration_secs: 5,
        };

        // Spawn server accept task
        tokio::spawn(async move {
            let (socket, _) = listener.accept().await.unwrap();
            let mut session = SmtpSession::new(
                socket,
                Some(tls_acceptor),
                db_clone,
                storage_clone,
                outbound_clone,
                tx_clone,
                "127.0.0.1".parse().unwrap(),
                rate_limiter_clone,
                blocklist,
                limits,
            );
            session.handle().await.unwrap();
        });

        // 5. Connect client socket and drive SMTP dialogue
        let stream = TcpStream::connect(local_addr).await.unwrap();
        let mut reader = BufReader::new(stream);
        let mut line = String::new();

        // Read welcome line
        reader.read_line(&mut line).await.unwrap();
        assert!(line.starts_with("220"));

        // Send EHLO
        let mut writer = reader.into_inner();
        writer.write_all(b"EHLO sender.com\r\n").await.unwrap();
        writer.flush().await.unwrap();
        let mut reader = BufReader::new(writer);
        
        line.clear();
        reader.read_line(&mut line).await.unwrap();
        assert!(line.starts_with("250"));
        while line.starts_with("250-") {
            line.clear();
            reader.read_line(&mut line).await.unwrap();
        }

        // Send MAIL FROM
        let mut writer = reader.into_inner();
        writer.write_all(b"MAIL FROM:<customer@sender.com>\r\n").await.unwrap();
        writer.flush().await.unwrap();
        let mut reader = BufReader::new(writer);

        line.clear();
        reader.read_line(&mut line).await.unwrap();
        assert!(line.starts_with("250"));

        // Send RCPT TO (Routes to our hello@example.com alias)
        let mut writer = reader.into_inner();
        writer.write_all(b"RCPT TO:<hello@example.com>\r\n").await.unwrap();
        writer.flush().await.unwrap();
        let mut reader = BufReader::new(writer);

        line.clear();
        reader.read_line(&mut line).await.unwrap();
        assert!(line.starts_with("250"));

        // Send DATA
        let mut writer = reader.into_inner();
        writer.write_all(b"DATA\r\n").await.unwrap();
        writer.flush().await.unwrap();
        let mut reader = BufReader::new(writer);

        line.clear();
        reader.read_line(&mut line).await.unwrap();
        assert!(line.starts_with("354"));

        // Send MIME body data
        let mut writer = reader.into_inner();
        writer.write_all(b"Subject: Feedback Loop\r\nMessage-ID: <loop-123@sender.com>\r\n\r\nHi there, please route this feedback E2E!\r\n.\r\n").await.unwrap();
        writer.flush().await.unwrap();
        let mut reader = BufReader::new(writer);

        line.clear();
        reader.read_line(&mut line).await.unwrap();
        assert!(line.starts_with("250")); // Accepted

        // Send QUIT
        let mut writer = reader.into_inner();
        writer.write_all(b"QUIT\r\n").await.unwrap();
        writer.flush().await.unwrap();

        // 6. E2E Assertions (Verify database has received email records dialect-safely)
        let emails = match &db {
            DbPool::Postgres(p) => {
                use sqlx::Row;
                sqlx::query("SELECT subject, sender_email, message_id FROM received_emails WHERE alias_id = $1")
                    .bind(alias.id)
                    .fetch_all(p)
                    .await
                    .unwrap()
                    .into_iter()
                    .map(|r| (r.get::<Option<String>, _>("subject"), r.get::<String, _>("sender_email"), r.get::<Option<String>, _>("message_id")))
                    .collect::<Vec<_>>()
            }
            DbPool::Sqlite(p) => {
                use sqlx::Row;
                sqlx::query("SELECT subject, sender_email, message_id FROM received_emails WHERE alias_id = ?")
                    .bind(alias.id)
                    .fetch_all(p)
                    .await
                    .unwrap()
                    .into_iter()
                    .map(|r| (r.get::<Option<String>, _>("subject"), r.get::<String, _>("sender_email"), r.get::<Option<String>, _>("message_id")))
                    .collect::<Vec<_>>()
            }
        };

        assert_eq!(emails.len(), 1);
        
        let (subject, sender_email, message_id) = emails[0].clone();

        assert_eq!(subject, Some("Feedback Loop".to_string()));
        assert_eq!(sender_email, "customer@sender.com");
        assert_eq!(message_id, Some("<loop-123@sender.com>".to_string()));

        // 7. Simulate E2E HTTP Axum Request Fetching the Dashboard
        let app_router = create_app(state).await;

        let request = Request::builder()
            .uri("/dashboard")
            .body(Body::empty())
            .unwrap();

        let response = app_router.oneshot(request).await.unwrap();
        assert_eq!(response.status(), StatusCode::SEE_OTHER); // Redirect to login verified!
    }).await;
}

#[tokio::test]
async fn test_incoming_srs_bounce_handling() {
    common::run_on_all_dbs(|db| async move {
        // Setup isolated temp directory for raw emails
        let temp_dir = tempfile::tempdir().unwrap();

        // Setup user, domain and alias using our shared common fixtures!
        let user = common::create_test_user(&db, "developer2@example.com", "password").await;
        let alias = common::create_test_alias(&db, user.id, "example.com", "hello", "dest@gmail.com").await;

        // Create App State
        let (tx, _) = broadcast::channel::<DashboardEvent>(100);
        
        let cert_path = temp_dir.path().join("smtp_cert.pem");
        let key_path = temp_dir.path().join("smtp_key.pem");
        common::generate_dummy_certs(&cert_path, &key_path);
        let tls_acceptor = HotReloadAcceptor::new(cert_path, key_path, std::time::Duration::from_millis(100)).unwrap();
        
        let srs_secret = "srs_secret_key_123".to_string();
        let outbound = Arc::new(OutboundService::new(
            srs_secret.clone(),
            hickory_resolver::TokioResolver::builder_tokio().unwrap().build().unwrap(),
            "example.com".to_string(),
            db.clone(),
            temp_dir.path().to_path_buf(),
        ));

        let _state = AppState {
            db: db.clone(),
            storage_dir: temp_dir.path().to_path_buf(),
            dns_scanner: maileroo::dns::DnsScanner::new(
                hickory_resolver::TokioResolver::builder_tokio().unwrap().build().unwrap()
            ),
            tx: tx.clone(),
            outbound: outbound.clone(),
            config: maileroo::config::AppConfig { auto_tls: None },
        };

        // Spin up real TCP loopback listener on dynamic port 0
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let local_addr = listener.local_addr().unwrap();

        let rate_limiter = Arc::new(RateLimiter::new());
        let db_clone = db.clone();
        let storage_clone = temp_dir.path().to_path_buf();
        let outbound_clone = outbound.clone();
        let tx_clone = tx.clone();
        let rate_limiter_clone = rate_limiter.clone();

        let block_path = temp_dir.path().join("blockips.conf");
        let blocklist = Arc::new(maileroo::inbound::blocklist::Blocklist::new(block_path));
        let limits = maileroo::inbound::rate_limit::InboundLimits {
            tarpit_threshold: 5,
            block_threshold: 10,
            tarpit_duration_secs: 5,
        };

        // Generate valid SRS address: encoding "hello@example.com" via SRS for forwarding
        let valid_srs = maileroo::outbound::srs::encode_srs("hello@example.com", "example.com", &srs_secret);
        
        // Spawn server accept task
        tokio::spawn(async move {
            let (socket, _) = listener.accept().await.unwrap();
            let mut session = SmtpSession::new(
                socket,
                Some(tls_acceptor),
                db_clone,
                storage_clone,
                outbound_clone,
                tx_clone,
                "127.0.0.1".parse().unwrap(),
                rate_limiter_clone,
                blocklist,
                limits,
            );
            session.handle().await.unwrap();
        });

        // 1. Test invalid/tampered SRS address first (rejected with 550)
        let stream = TcpStream::connect(local_addr).await.unwrap();
        let mut reader = BufReader::new(stream);
        let mut line = String::new();

        reader.read_line(&mut line).await.unwrap();
        assert!(line.starts_with("220"));

        let mut writer = reader.into_inner();
        writer.write_all(b"EHLO sender.com\r\n").await.unwrap();
        writer.flush().await.unwrap();
        let mut reader = BufReader::new(writer);
        
        line.clear();
        reader.read_line(&mut line).await.unwrap();
        assert!(line.starts_with("250"));
        while line.starts_with("250-") {
            line.clear();
            reader.read_line(&mut line).await.unwrap();
        }

        let mut writer = reader.into_inner();
        writer.write_all(b"MAIL FROM:<customer@sender.com>\r\n").await.unwrap();
        writer.flush().await.unwrap();
        let mut reader = BufReader::new(writer);

        line.clear();
        reader.read_line(&mut line).await.unwrap();
        assert!(line.starts_with("250"));

        // Tamper with the valid SRS address to trigger signature mismatch
        let tampered_srs = valid_srs.replace("SRS0+", "SRS0+A");
        let mut writer = reader.into_inner();
        writer.write_all(format!("RCPT TO:<{}>\r\n", tampered_srs).as_bytes()).await.unwrap();
        writer.flush().await.unwrap();
        let mut reader = BufReader::new(writer);

        line.clear();
        reader.read_line(&mut line).await.unwrap();
        assert!(line.starts_with("550")); // Rejected!

        let mut writer = reader.into_inner();
        writer.write_all(b"QUIT\r\n").await.unwrap();
        writer.flush().await.unwrap();

        // 2. Test valid SRS address (accepted successfully)
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let local_addr = listener.local_addr().unwrap();

        let db_clone = db.clone();
        let storage_clone = temp_dir.path().to_path_buf();
        let outbound_clone = outbound.clone();
        let tx_clone = tx.clone();
        let rate_limiter_clone = rate_limiter.clone();
        
        let cert_path2 = temp_dir.path().join("smtp_cert2.pem");
        let key_path2 = temp_dir.path().join("smtp_key2.pem");
        common::generate_dummy_certs(&cert_path2, &key_path2);
        let tls_acceptor = HotReloadAcceptor::new(cert_path2, key_path2, std::time::Duration::from_millis(100)).unwrap();
        
        let blocklist_clone = Arc::new(maileroo::inbound::blocklist::Blocklist::new(temp_dir.path().join("blockips2.conf")));

        tokio::spawn(async move {
            let (socket, _) = listener.accept().await.unwrap();
            let mut session = SmtpSession::new(
                socket,
                Some(tls_acceptor),
                db_clone,
                storage_clone,
                outbound_clone,
                tx_clone,
                "127.0.0.1".parse().unwrap(),
                rate_limiter_clone,
                blocklist_clone,
                limits,
            );
            session.handle().await.unwrap();
        });

        let stream = TcpStream::connect(local_addr).await.unwrap();
        let mut reader = BufReader::new(stream);
        let mut line = String::new();

        reader.read_line(&mut line).await.unwrap();
        assert!(line.starts_with("220"));

        let mut writer = reader.into_inner();
        writer.write_all(b"EHLO sender.com\r\n").await.unwrap();
        writer.flush().await.unwrap();
        let mut reader = BufReader::new(writer);
        
        line.clear();
        reader.read_line(&mut line).await.unwrap();
        assert!(line.starts_with("250"));
        while line.starts_with("250-") {
            line.clear();
            reader.read_line(&mut line).await.unwrap();
        }

        let mut writer = reader.into_inner();
        writer.write_all(b"MAIL FROM:<customer@sender.com>\r\n").await.unwrap();
        writer.flush().await.unwrap();
        let mut reader = BufReader::new(writer);

        line.clear();
        reader.read_line(&mut line).await.unwrap();
        assert!(line.starts_with("250"));

        let mut writer = reader.into_inner();
        writer.write_all(format!("RCPT TO:<{}>\r\n", valid_srs).as_bytes()).await.unwrap();
        writer.flush().await.unwrap();
        let mut reader = BufReader::new(writer);

        line.clear();
        reader.read_line(&mut line).await.unwrap();
        assert!(line.starts_with("250")); // Accepted!

        let mut writer = reader.into_inner();
        writer.write_all(b"DATA\r\n").await.unwrap();
        writer.flush().await.unwrap();
        let mut reader = BufReader::new(writer);

        line.clear();
        reader.read_line(&mut line).await.unwrap();
        assert!(line.starts_with("354"));

        let mut writer = reader.into_inner();
        writer.write_all(b"Subject: SRS Bounce Test\r\nMessage-ID: <srs-123@sender.com>\r\n\r\nIncoming bounce message.\r\n.\r\n").await.unwrap();
        writer.flush().await.unwrap();
        let mut reader = BufReader::new(writer);

        line.clear();
        reader.read_line(&mut line).await.unwrap();
        assert!(line.starts_with("250"));

        let mut writer = reader.into_inner();
        writer.write_all(b"QUIT\r\n").await.unwrap();
        writer.flush().await.unwrap();

        // Verify database received it under alias
        let emails = match &db {
            DbPool::Postgres(p) => {
                use sqlx::Row;
                sqlx::query("SELECT subject, sender_email FROM received_emails WHERE alias_id = $1")
                    .bind(alias.id)
                    .fetch_all(p)
                    .await
                    .unwrap()
                    .into_iter()
                    .map(|r| (r.get::<Option<String>, _>("subject"), r.get::<String, _>("sender_email")))
                    .collect::<Vec<_>>()
            }
            DbPool::Sqlite(p) => {
                use sqlx::Row;
                sqlx::query("SELECT subject, sender_email FROM received_emails WHERE alias_id = ?")
                    .bind(alias.id)
                    .fetch_all(p)
                    .await
                    .unwrap()
                    .into_iter()
                    .map(|r| (r.get::<Option<String>, _>("subject"), r.get::<String, _>("sender_email")))
                    .collect::<Vec<_>>()
            }
        };

        assert_eq!(emails.len(), 1);
        assert_eq!(emails[0].0, Some("SRS Bounce Test".to_string()));
    }).await;
}
