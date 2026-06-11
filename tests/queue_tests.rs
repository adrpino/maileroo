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
