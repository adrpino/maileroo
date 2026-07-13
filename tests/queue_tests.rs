mod common;

use maileroo::db::DbPool;
use maileroo::outbound::{OutboundService, process_queue_tick};
use std::sync::Arc;
use tempfile::tempdir;
use time::OffsetDateTime;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::TcpListener;

#[tokio::test]
async fn test_outbound_queue_retry_lifecycle_e2e() {
    common::run_on_all_dbs(|db| async move {
        // 1. Setup isolated temporary folders
        let temp_storage_dir = tempdir().unwrap();
        let srs_secret = "test_srs_secret_key_123".to_string();
        let identity_domain = "example.com".to_string();

        // 2. Start a mock SMTP server that will immediately drop the connection to trigger a transient error
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();

        // Spawn a background listener that accepts one connection and drops it immediately
        let drop_handle = tokio::spawn(async move {
            if let Ok((socket, _)) = listener.accept().await {
                // Drop the socket right away to cause a transient connection error
                drop(socket);
            }
        });

        // 4. Instantiate OutboundService with direct RelayConfig override
        let resolver = hickory_resolver::TokioResolver::builder_tokio().unwrap().build().unwrap();
        let outbound = Arc::new(OutboundService::new(
            srs_secret.clone(),
            resolver.clone(),
            identity_domain.clone(),
            db.clone(),
            temp_storage_dir.path().to_path_buf(),
        ).with_relay_override(maileroo::outbound::relay::RelayConfig {
            host: "127.0.0.1".to_string(),
            port,
            user: "api".to_string(),
            pass: "test-auth-token".to_string(),
        }));

        // 5. Trigger Outbound Send (which will hit the drop connection, fail, and automatically queue)
        let rcpt = "rcpt@external.com";
        let sender = "sender@example.com";
        let body_bytes = b"Subject: Retry E2E Test\r\n\r\nHello retry worker!";

        tracing::info!("Sending email to trigger transient fallback queueing...");
        let send_result = outbound.send_firsthand(rcpt, sender, body_bytes).await;

        // The wrapper intercepts the transient failure, enqueues the job, and returns Ok(())
        assert!(send_result.is_ok());

        // Wait for connection handler to settle
        let _ = drop_handle.await;

        // Debug print all database rows to inspect contents
        match db {
            DbPool::Sqlite(ref p) => {
                let rows: Vec<(uuid::Uuid, String, String, i32, String, String, String, Option<String>, Option<String>)> = sqlx::query_as("SELECT id, from_envelope, to_recipient, attempts, status, CAST(next_retry_at AS TEXT), CAST(created_at AS TEXT), strftime('%s', next_retry_at), strftime('%s', 'now') FROM outbound_queue")
                    .fetch_all(p)
                    .await
                    .unwrap();
                println!("DEBUG OUTBOUND QUEUE ROWS (SQLite): {:?}", rows);
            }
            DbPool::Postgres(ref p) => {
                let rows: Vec<(uuid::Uuid, String, String, i32, String, OffsetDateTime, OffsetDateTime)> = sqlx::query_as("SELECT id, from_envelope, to_recipient, attempts, status, next_retry_at, created_at FROM outbound_queue")
                    .fetch_all(p)
                    .await
                    .unwrap();
                println!("DEBUG OUTBOUND QUEUE ROWS (Postgres): {:?}", rows);
            }
        }

        // 6. Assert job is registered in outbound_queue table
        let queued_jobs = maileroo::db::queue::fetch_next_retryable_jobs(&db, 10).await.unwrap();
        assert_eq!(queued_jobs.len(), 1, "There should be exactly 1 queued job after transient delivery failure");

        let job = &queued_jobs[0];
        assert_eq!(job.from_envelope, sender);
        assert_eq!(job.to_recipient, rcpt);
        assert_eq!(job.attempts, 0);
        assert_eq!(job.status, "pending");

        // Assert physical EML exists
        let eml_file = maileroo::outbound::get_job_file_path(temp_storage_dir.path(), job.id);
        assert!(eml_file.exists());
        let disk_bytes = tokio::fs::read(&eml_file).await.unwrap();
        assert_eq!(disk_bytes, body_bytes);

        // 7. Re-bind and start a healthy, fully-functional SMTP mock server to accept the message on retry
        let healthy_listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let healthy_port = healthy_listener.local_addr().unwrap().port();

                let healthy_server_handle = tokio::spawn(async move {
            let (socket, _) = healthy_listener.accept().await.unwrap();
            let mut reader = BufReader::new(socket);
            let mut buf = String::new();

            // S: Banner greeting
            reader.get_mut().write_all(b"220 smtp.mockrelay.com Welcome\r\n").await.unwrap();

            // C: Read EHLO
            buf.clear();
            reader.read_line(&mut buf).await.unwrap();
            assert!(buf.starts_with("EHLO"));

            // S: Capabilities
            reader.get_mut().write_all(b"250-smtp.mockrelay.com\r\n250 AUTH PLAIN\r\n").await.unwrap();

            // C: Read AUTH PLAIN
            buf.clear();
            reader.read_line(&mut buf).await.unwrap();
            assert!(buf.contains("AUTH PLAIN"));

            // S: Auth success
            reader.get_mut().write_all(b"235 Auth successful\r\n").await.unwrap();

            // C: MAIL FROM
            buf.clear();
            reader.read_line(&mut buf).await.unwrap();
            assert!(buf.contains("MAIL FROM"));
            reader.get_mut().write_all(b"250 OK\r\n").await.unwrap();

            // C: RCPT TO
            buf.clear();
            reader.read_line(&mut buf).await.unwrap();
            assert!(buf.contains("RCPT TO"));
            reader.get_mut().write_all(b"250 OK\r\n").await.unwrap();

            // C: DATA
            buf.clear();
            reader.read_line(&mut buf).await.unwrap();
            assert!(buf.contains("DATA"));
            reader.get_mut().write_all(b"354 Start input\r\n").await.unwrap();

            // C: Body lines until dot
            loop {
                buf.clear();
                reader.read_line(&mut buf).await.unwrap();
                if buf == ".\r\n" {
                    break;
                }
            }
            reader.get_mut().write_all(b"250 Message accepted for delivery\r\n").await.unwrap();

            // C: QUIT
            buf.clear();
            reader.read_line(&mut buf).await.unwrap();
            assert!(buf.starts_with("QUIT"));
            reader.get_mut().write_all(b"221 Goodbye\r\n").await.unwrap();
        });

        // 8. Manually force the queued job's next_retry_at to Utc::now() to ensure it is selected for processing
        maileroo::db::queue::update_job_status(
            &db,
            job.id,
            "pending",
            job.attempts,
            job.last_error.as_deref(),
            OffsetDateTime::now_utc() - time::Duration::minutes(5), // 5 minutes in the past
        )
        .await
        .unwrap();

        // 9. Create a healthy outbound service pointing to the healthy mock server port
        let healthy_outbound = Arc::new(OutboundService::new(
            srs_secret.clone(),
            resolver.clone(),
            identity_domain.clone(),
            db.clone(),
            temp_storage_dir.path().to_path_buf(),
        ).with_relay_override(maileroo::outbound::relay::RelayConfig {
            host: "127.0.0.1".to_string(),
            port: healthy_port,
            user: "api".to_string(),
            pass: "test-auth-token".to_string(),
        }));

        // Execute single tick of process_queue_tick with the healthy service
        tracing::info!("Executing queue tick to process and deliver retryable jobs...");
        process_queue_tick(&db, temp_storage_dir.path(), healthy_outbound).await.unwrap();

        // Wait for the healthy SMTP transaction to complete
        healthy_server_handle.await.unwrap();

        // 10. Assert complete success cleanup: DB row and disk EML must be fully purged
        let queued_after = maileroo::db::queue::fetch_next_retryable_jobs(&db, 10).await.unwrap();
        assert!(queued_after.is_empty(), "DB record must be cleanly deleted upon successful delivery");
        assert!(!eml_file.exists(), "EML file must be cleanly deleted from disk upon successful delivery");
    }).await;
}

#[tokio::test]
async fn test_outbound_queue_daemon_periodic_delivery_e2e() {
    common::run_on_all_dbs(|db| async move {
        // 1. Setup isolated temporary folders
        let temp_storage_dir = tempdir().unwrap();
        let srs_secret = "test_srs_secret_key_123".to_string();
        let identity_domain = "example.com".to_string();

        // 2. Start a mock SMTP server to act as a healthy relay for retry delivery
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();

        let server_handle = tokio::spawn(async move {
            let (socket, _) = listener.accept().await.unwrap();
            let mut reader = BufReader::new(socket);
            let mut buf = String::new();

            // S: Greeting
            reader
                .get_mut()
                .write_all(b"220 smtp.mockrelay.com\r\n")
                .await
                .unwrap();

            // C: EHLO
            reader.read_line(&mut buf).await.unwrap();
            assert!(buf.contains("EHLO"));
            reader
                .get_mut()
                .write_all(b"250-smtp.mockrelay.com\r\n250 AUTH PLAIN\r\n")
                .await
                .unwrap();

            // C: AUTH PLAIN
            buf.clear();
            reader.read_line(&mut buf).await.unwrap();
            assert!(buf.contains("AUTH PLAIN"));
            reader
                .get_mut()
                .write_all(b"235 Auth successful\r\n")
                .await
                .unwrap();

            // C: MAIL FROM
            buf.clear();
            reader.read_line(&mut buf).await.unwrap();
            assert!(buf.contains("MAIL FROM"));
            reader.get_mut().write_all(b"250 OK\r\n").await.unwrap();

            // C: RCPT TO
            buf.clear();
            reader.read_line(&mut buf).await.unwrap();
            assert!(buf.contains("RCPT TO"));
            reader.get_mut().write_all(b"250 OK\r\n").await.unwrap();

            // C: DATA
            buf.clear();
            reader.read_line(&mut buf).await.unwrap();
            assert!(buf.contains("DATA"));
            reader
                .get_mut()
                .write_all(b"354 Start input\r\n")
                .await
                .unwrap();

            // C: Body lines until dot
            loop {
                buf.clear();
                reader.read_line(&mut buf).await.unwrap();
                if buf == ".\r\n" {
                    break;
                }
            }
            reader
                .get_mut()
                .write_all(b"250 Message accepted for delivery\r\n")
                .await
                .unwrap();

            // C: QUIT
            buf.clear();
            reader.read_line(&mut buf).await.unwrap();
            assert!(buf.starts_with("QUIT"));
            reader
                .get_mut()
                .write_all(b"221 Goodbye\r\n")
                .await
                .unwrap();
        });

        // 4. Create an OutboundService instance with direct RelayConfig override
        let resolver = hickory_resolver::TokioResolver::builder_tokio()
            .unwrap()
            .build()
            .unwrap();
        let outbound = Arc::new(
            OutboundService::new(
                srs_secret.clone(),
                resolver.clone(),
                identity_domain.clone(),
                db.clone(),
                temp_storage_dir.path().to_path_buf(),
            )
            .with_relay_override(maileroo::outbound::relay::RelayConfig {
                host: "127.0.0.1".to_string(),
                port,
                user: "api".to_string(),
                pass: "test-auth-token".to_string(),
            }),
        );

        // 5. Manually insert a queued job directly into the database (representing a previous transient failure)
        let job_id = uuid::Uuid::new_v4();
        let from_envelope = "sender@example.com";
        let to_recipient = "rcpt@external.com";
        let body_bytes = b"Subject: Daemon Retry E2E Test\r\n\r\nHello daemon worker loop!";

        // Save physical file first to match the storage constraint
        let eml_file = maileroo::outbound::get_job_file_path(temp_storage_dir.path(), job_id);
        tokio::fs::create_dir_all(eml_file.parent().unwrap())
            .await
            .unwrap();
        tokio::fs::write(&eml_file, body_bytes).await.unwrap();

        // Insert database record
        maileroo::db::queue::insert_job(&db, job_id, from_envelope, to_recipient)
            .await
            .unwrap();

        // Ensure next_retry_at is in the past to make it eligible immediately
        maileroo::db::queue::update_job_status(
            &db,
            job_id,
            "pending",
            0,
            None,
            OffsetDateTime::now_utc() - time::Duration::minutes(10),
        )
        .await
        .unwrap();

        // 6. Start the background outbound queue daemon with a fast check interval (50 milliseconds)
        tracing::info!("Starting background queue daemon loop with 50ms interval...");
        maileroo::outbound::start_queue_daemon(
            db.clone(),
            temp_storage_dir.path().to_path_buf(),
            outbound.clone(),
            std::time::Duration::from_millis(50),
        );

        // 7. Wait and poll database until the job is processed and cleared
        let mut job_processed = false;
        for _ in 0..100 {
            // Max 2 seconds timeout (100 * 20ms)
            tokio::time::sleep(std::time::Duration::from_millis(20)).await;
            let queued_jobs = maileroo::db::queue::fetch_next_retryable_jobs(&db, 10)
                .await
                .unwrap();
            if queued_jobs.is_empty() {
                // If it is gone, we also check if the file is gone
                if !eml_file.exists() {
                    job_processed = true;
                    break;
                }
            }
        }

        assert!(
            job_processed,
            "Queue daemon failed to pick up, process, and clean up the retry job within timeout"
        );

        // Wait for the mock SMTP server handle to join/complete successfully
        server_handle.await.unwrap();
    })
    .await;
}

#[tokio::test]
async fn failing_retry_increments_attempts_and_does_not_duplicate() {
    common::run_on_all_dbs(|db| async move {
        let temp = tempfile::tempdir().unwrap();

        // Transient-failure relay: accept the TCP connection, then drop it.
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        tokio::spawn(async move {
            if let Ok((s, _)) = listener.accept().await {
                drop(s);
            }
        });

        let resolver = hickory_resolver::TokioResolver::builder_tokio()
            .unwrap()
            .build()
            .unwrap();
        let outbound = std::sync::Arc::new(
            maileroo::outbound::OutboundService::new(
                "srs".into(),
                resolver,
                "example.com".into(),
                db.clone(),
                temp.path().to_path_buf(),
            )
            .with_relay_override(maileroo::outbound::relay::RelayConfig {
                host: "127.0.0.1".into(),
                port,
                user: "api".into(),
                pass: "tok".into(),
            }),
        );

        // Seed a job (attempts = 0) eligible now.
        let id = uuid::Uuid::new_v4();
        let eml = maileroo::outbound::get_job_file_path(temp.path(), id);
        tokio::fs::create_dir_all(eml.parent().unwrap())
            .await
            .unwrap();
        tokio::fs::write(&eml, b"Subject: x\r\n\r\nbody")
            .await
            .unwrap();
        maileroo::db::queue::insert_job(&db, id, "s@example.com", "r@ext.com")
            .await
            .unwrap();

        maileroo::outbound::process_queue_tick(&db, temp.path(), outbound)
            .await
            .unwrap();

        // Same row, attempts incremented, still pending, backoff in the future, file kept.
        let count = match db {
            DbPool::Sqlite(ref p) => {
                let cnt: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM outbound_queue")
                    .fetch_one(p)
                    .await
                    .unwrap();
                cnt as usize
            }
            DbPool::Postgres(ref p) => {
                let cnt: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM outbound_queue")
                    .fetch_one(p)
                    .await
                    .unwrap();
                cnt as usize
            }
        };
        assert_eq!(count, 1, "must not create a duplicate job on failed retry");

        let (attempts, status, next_retry_at) = match db {
            DbPool::Sqlite(ref p) => {
                let res: (i32, String, OffsetDateTime) = sqlx::query_as(
                    "SELECT attempts, status, next_retry_at FROM outbound_queue WHERE id = ?",
                )
                .bind(id)
                .fetch_one(p)
                .await
                .unwrap();
                (res.0, res.1, res.2)
            }
            DbPool::Postgres(ref p) => {
                let res: (i32, String, OffsetDateTime) = sqlx::query_as(
                    "SELECT attempts, status, next_retry_at FROM outbound_queue WHERE id = $1",
                )
                .bind(id)
                .fetch_one(p)
                .await
                .unwrap();
                (res.0, res.1, res.2)
            }
        };

        assert_eq!(attempts, 1, "attempts must increment");
        assert_eq!(status, "pending");
        assert!(
            next_retry_at > OffsetDateTime::now_utc(),
            "backoff must push next_retry_at into the future, got {next_retry_at}"
        );
        assert!(eml.exists(), "EML must be retained for the next attempt");
    })
    .await;
}

#[tokio::test]
async fn permanent_failure_fails_fast() {
    common::run_on_all_dbs(|db| async move {
        let temp = tempfile::tempdir().unwrap();

        // Relay mock rejecting with permanent 550
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        tokio::spawn(async move {
            if let Ok((socket, _)) = listener.accept().await {
                let mut reader = BufReader::new(socket);
                let mut buf = String::new();

                // Banner
                reader
                    .get_mut()
                    .write_all(b"220 smtp.mockrelay.com\r\n")
                    .await
                    .unwrap();

                // EHLO
                reader.read_line(&mut buf).await.unwrap();
                reader
                    .get_mut()
                    .write_all(b"250-smtp.mockrelay.com\r\n250 AUTH PLAIN\r\n")
                    .await
                    .unwrap();

                // AUTH
                buf.clear();
                reader.read_line(&mut buf).await.unwrap();
                reader
                    .get_mut()
                    .write_all(b"235 Auth successful\r\n")
                    .await
                    .unwrap();

                // MAIL FROM - Reject with 550
                buf.clear();
                reader.read_line(&mut buf).await.unwrap();
                reader
                    .get_mut()
                    .write_all(b"550 User not allowed\r\n")
                    .await
                    .unwrap();
            }
        });

        let resolver = hickory_resolver::TokioResolver::builder_tokio()
            .unwrap()
            .build()
            .unwrap();
        let outbound = std::sync::Arc::new(
            maileroo::outbound::OutboundService::new(
                "srs".into(),
                resolver,
                "example.com".into(),
                db.clone(),
                temp.path().to_path_buf(),
            )
            .with_relay_override(maileroo::outbound::relay::RelayConfig {
                host: "127.0.0.1".into(),
                port,
                user: "api".into(),
                pass: "tok".into(),
            }),
        );

        // Seed a job
        let id = uuid::Uuid::new_v4();
        let eml = maileroo::outbound::get_job_file_path(temp.path(), id);
        tokio::fs::create_dir_all(eml.parent().unwrap())
            .await
            .unwrap();
        tokio::fs::write(&eml, b"Subject: x\r\n\r\nbody")
            .await
            .unwrap();
        maileroo::db::queue::insert_job(&db, id, "s@example.com", "r@ext.com")
            .await
            .unwrap();

        maileroo::outbound::process_queue_tick(&db, temp.path(), outbound)
            .await
            .unwrap();

        let (attempts, status) = match db {
            DbPool::Sqlite(ref p) => {
                let res: (i32, String) =
                    sqlx::query_as("SELECT attempts, status FROM outbound_queue WHERE id = ?")
                        .bind(id)
                        .fetch_one(p)
                        .await
                        .unwrap();
                (res.0, res.1)
            }
            DbPool::Postgres(ref p) => {
                let res: (i32, String) =
                    sqlx::query_as("SELECT attempts, status FROM outbound_queue WHERE id = $1")
                        .bind(id)
                        .fetch_one(p)
                        .await
                        .unwrap();
                (res.0, res.1)
            }
        };

        assert_eq!(attempts, 1, "attempts must increment on failed run");
        assert_eq!(
            status, "failed",
            "permanent failure must immediately fail the job"
        );
    })
    .await;
}

#[tokio::test]
async fn exhausts_max_attempts_then_stops() {
    common::run_on_all_dbs(|db| async move {
        let temp = tempfile::tempdir().unwrap();

        // Transient failure SMTP: drop connection
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        tokio::spawn(async move {
            if let Ok((s, _)) = listener.accept().await {
                drop(s);
            }
        });

        let resolver = hickory_resolver::TokioResolver::builder_tokio()
            .unwrap()
            .build()
            .unwrap();
        let outbound = std::sync::Arc::new(
            maileroo::outbound::OutboundService::new(
                "srs".into(),
                resolver,
                "example.com".into(),
                db.clone(),
                temp.path().to_path_buf(),
            )
            .with_relay_override(maileroo::outbound::relay::RelayConfig {
                host: "127.0.0.1".into(),
                port,
                user: "api".into(),
                pass: "tok".into(),
            }),
        );

        // Seed a job with attempts = 9 (max is 10)
        let id = uuid::Uuid::new_v4();
        let eml = maileroo::outbound::get_job_file_path(temp.path(), id);
        tokio::fs::create_dir_all(eml.parent().unwrap())
            .await
            .unwrap();
        tokio::fs::write(&eml, b"Subject: x\r\n\r\nbody")
            .await
            .unwrap();
        maileroo::db::queue::insert_job(&db, id, "s@example.com", "r@ext.com")
            .await
            .unwrap();

        match db {
            DbPool::Sqlite(ref p) => {
                sqlx::query("UPDATE outbound_queue SET attempts = 9 WHERE id = ?")
                    .bind(id)
                    .execute(p)
                    .await
                    .unwrap();
            }
            DbPool::Postgres(ref p) => {
                sqlx::query("UPDATE outbound_queue SET attempts = 9 WHERE id = $1")
                    .bind(id)
                    .execute(p)
                    .await
                    .unwrap();
            }
        };

        maileroo::outbound::process_queue_tick(&db, temp.path(), outbound)
            .await
            .unwrap();

        let (attempts, status) = match db {
            DbPool::Sqlite(ref p) => {
                let res: (i32, String) =
                    sqlx::query_as("SELECT attempts, status FROM outbound_queue WHERE id = ?")
                        .bind(id)
                        .fetch_one(p)
                        .await
                        .unwrap();
                (res.0, res.1)
            }
            DbPool::Postgres(ref p) => {
                let res: (i32, String) =
                    sqlx::query_as("SELECT attempts, status FROM outbound_queue WHERE id = $1")
                        .bind(id)
                        .fetch_one(p)
                        .await
                        .unwrap();
                (res.0, res.1)
            }
        };

        assert_eq!(attempts, 10);
        assert_eq!(status, "failed");
    })
    .await;
}

#[tokio::test]
async fn test_mx_delivery_succeeds_with_self_signed_cert() {
    common::run_on_all_dbs(|db| async move {
        common::init_crypto_provider();

        let temp = tempfile::tempdir().unwrap();
        let cert_path = temp.path().join("cert.pem");
        let key_path = temp.path().join("key.pem");
        common::generate_dummy_certs(&cert_path, &key_path);

        let cert_file = std::fs::File::open(&cert_path).unwrap();
        let key_file = std::fs::File::open(&key_path).unwrap();
        let certs = rustls_pemfile::certs(&mut std::io::BufReader::new(cert_file))
            .collect::<Result<Vec<_>, _>>()
            .unwrap();
        let key = rustls_pemfile::private_key(&mut std::io::BufReader::new(key_file))
            .unwrap()
            .unwrap();

        let tls_server_config = tokio_rustls::rustls::ServerConfig::builder()
            .with_no_client_auth()
            .with_single_cert(certs, key)
            .unwrap();
        let tls_acceptor = tokio_rustls::TlsAcceptor::from(std::sync::Arc::new(tls_server_config));

        let smtp_listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let smtp_port = smtp_listener.local_addr().unwrap().port();

        let smtp_server_handle = tokio::spawn(async move {
            let (socket, _) = smtp_listener.accept().await.unwrap();
            let mut reader = BufReader::new(socket);
            let mut buf = String::new();

            reader
                .get_mut()
                .write_all(b"220 smtp.mockmx.com Welcome\r\n")
                .await
                .unwrap();

            reader.read_line(&mut buf).await.unwrap();
            assert!(buf.starts_with("EHLO"));

            reader
                .get_mut()
                .write_all(b"250-smtp.mockmx.com\r\n250 STARTTLS\r\n")
                .await
                .unwrap();

            buf.clear();
            reader.read_line(&mut buf).await.unwrap();
            assert!(buf.starts_with("STARTTLS"));

            reader
                .get_mut()
                .write_all(b"220 Go ahead with TLS upgrade\r\n")
                .await
                .unwrap();

            let plain_stream = reader.into_inner();
            let tls_stream = tls_acceptor.accept(plain_stream).await.unwrap();
            let mut reader = BufReader::new(tls_stream);

            buf.clear();
            reader.read_line(&mut buf).await.unwrap();
            assert!(buf.starts_with("EHLO"));

            reader
                .get_mut()
                .write_all(b"250-smtp.mockmx.com\r\n250 PIPELINING\r\n")
                .await
                .unwrap();

            buf.clear();
            reader.read_line(&mut buf).await.unwrap();
            assert!(buf.contains("MAIL FROM"));
            reader.get_mut().write_all(b"250 OK\r\n").await.unwrap();

            buf.clear();
            reader.read_line(&mut buf).await.unwrap();
            assert!(buf.contains("RCPT TO"));
            reader.get_mut().write_all(b"250 OK\r\n").await.unwrap();

            buf.clear();
            reader.read_line(&mut buf).await.unwrap();
            assert!(buf.contains("DATA"));
            reader
                .get_mut()
                .write_all(b"354 Start input\r\n")
                .await
                .unwrap();

            loop {
                buf.clear();
                reader.read_line(&mut buf).await.unwrap();
                if buf == ".\r\n" {
                    break;
                }
            }
            reader
                .get_mut()
                .write_all(b"250 Message accepted for delivery\r\n")
                .await
                .unwrap();

            buf.clear();
            reader.read_line(&mut buf).await.unwrap();
            assert!(buf.starts_with("QUIT"));
            reader
                .get_mut()
                .write_all(b"221 Goodbye\r\n")
                .await
                .unwrap();
        });

        let dns_socket = tokio::net::UdpSocket::bind("127.0.0.1:0").await.unwrap();
        let dns_port = dns_socket.local_addr().unwrap().port();

        tokio::spawn(async move {
            let mut buf = [0u8; 512];
            loop {
                if let Ok((len, src)) = dns_socket.recv_from(&mut buf).await {
                    let query = &buf[..len];
                    let mut response = Vec::new();
                    response.extend_from_slice(&query[0..2]);
                    response.extend_from_slice(&[0x81, 0x80]);
                    response.extend_from_slice(&[0x00, 0x01]);
                    response.extend_from_slice(&[0x00, 0x01]);
                    response.extend_from_slice(&[0x00, 0x00, 0x00, 0x00]);

                    let mut name_end = 12;
                    while name_end < query.len() && query[name_end] != 0 {
                        name_end += 1;
                    }
                    if name_end + 5 <= query.len() {
                        let question_end = name_end + 5;
                        let qtype =
                            (query[name_end + 1] as u16) << 8 | (query[name_end + 2] as u16);

                        response.extend_from_slice(&query[12..question_end]);

                        response.extend_from_slice(&[0xc0, 0x0c]);
                        if qtype == 15 {
                            response.extend_from_slice(&[0x00, 0x0f]);
                            response.extend_from_slice(&[0x00, 0x01]);
                            response.extend_from_slice(&[0x00, 0x00, 0x00, 0x3c]);
                            response.extend_from_slice(&[0x00, 0x0d]);
                            response.extend_from_slice(&[0x00, 0x00]);
                            response.extend_from_slice(b"\x09localhost\x00");
                        } else {
                            response.extend_from_slice(&[0x00, 0x01]);
                            response.extend_from_slice(&[0x00, 0x01]);
                            response.extend_from_slice(&[0x00, 0x00, 0x00, 0x3c]);
                            response.extend_from_slice(&[0x00, 0x04]);
                            response.extend_from_slice(&[127, 0, 0, 1]);
                        }

                        let _ = dns_socket.send_to(&response, src).await;
                    }
                }
            }
        });

        use hickory_resolver::config::{NameServerConfig, ResolverConfig};
        use hickory_resolver::net::runtime::TokioRuntimeProvider;
        use std::net::{IpAddr, Ipv4Addr};

        let mut ns_config = NameServerConfig::udp(IpAddr::V4(Ipv4Addr::new(127, 0, 0, 1)));
        ns_config.connections[0].port = dns_port;

        let dns_config = ResolverConfig::from_parts(None, vec![], vec![ns_config]);
        let resolver = hickory_resolver::TokioResolver::builder_with_config(
            dns_config,
            TokioRuntimeProvider::default(),
        )
        .build()
        .unwrap();

        let outbound = OutboundService::new(
            "srs".into(),
            resolver,
            "example.com".into(),
            db.clone(),
            temp.path().to_path_buf(),
        )
        .with_mx_port(smtp_port);

        let res = outbound
            .send_firsthand(
                "receiver@testmx.com",
                "sender@example.com",
                b"Subject: test\r\n\r\ntest body",
            )
            .await;

        assert!(
            res.is_ok(),
            "Delivery should succeed through opportunistic TLS verifier: {:?}",
            res.err()
        );
        smtp_server_handle.await.unwrap();
    })
    .await;
}

#[tokio::test]
async fn test_relay_delivery_fails_with_self_signed_cert() {
    common::run_on_all_dbs(|db| async move {
        common::init_crypto_provider();

        let temp = tempfile::tempdir().unwrap();
        let cert_path = temp.path().join("cert.pem");
        let key_path = temp.path().join("key.pem");
        common::generate_dummy_certs(&cert_path, &key_path);

        let cert_file = std::fs::File::open(&cert_path).unwrap();
        let key_file = std::fs::File::open(&key_path).unwrap();
        let certs = rustls_pemfile::certs(&mut std::io::BufReader::new(cert_file))
            .collect::<Result<Vec<_>, _>>()
            .unwrap();
        let key = rustls_pemfile::private_key(&mut std::io::BufReader::new(key_file))
            .unwrap()
            .unwrap();

        let tls_server_config = tokio_rustls::rustls::ServerConfig::builder()
            .with_no_client_auth()
            .with_single_cert(certs, key)
            .unwrap();
        let tls_acceptor = tokio_rustls::TlsAcceptor::from(std::sync::Arc::new(tls_server_config));

        let smtp_listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let smtp_port = smtp_listener.local_addr().unwrap().port();

        tokio::spawn(async move {
            if let Ok((socket, _)) = smtp_listener.accept().await {
                let mut reader = BufReader::new(socket);
                let mut buf = String::new();

                reader.get_mut().write_all(b"220 smtp.mockrelay.com Welcome\r\n").await.unwrap();

                reader.read_line(&mut buf).await.unwrap();

                reader.get_mut().write_all(b"250-smtp.mockrelay.com\r\n250 STARTTLS\r\n").await.unwrap();

                buf.clear();
                reader.read_line(&mut buf).await.unwrap();

                reader.get_mut().write_all(b"220 Go ahead with TLS upgrade\r\n").await.unwrap();

                let plain_stream = reader.into_inner();
                let _ = tls_acceptor.accept(plain_stream).await;
            }
        });

        let resolver = hickory_resolver::TokioResolver::builder_tokio().unwrap().build().unwrap();
        let outbound = OutboundService::new(
            "srs".into(),
            resolver,
            "example.com".into(),
            db.clone(),
            temp.path().to_path_buf(),
        ).with_relay_override(maileroo::outbound::relay::RelayConfig {
            host: "127.0.0.1".to_string(),
            port: smtp_port,
            user: "api".to_string(),
            pass: "tok".to_string(),
        });

        let res = outbound.deliver_once("receiver@ext.com", "sender@example.com", b"Subject: test\r\n\r\ntest body").await;

        assert!(res.is_err(), "Relay delivery should fail because the self-signed cert is untrusted under strict verification");
        let err_str = res.unwrap_err().to_string();
        assert!(err_str.contains("TLS connection failed") || err_str.contains("invalid peer certificate") || err_str.contains("UnknownIssuer"), "Error should be a TLS error, got: {}", err_str);
    }).await;
}
