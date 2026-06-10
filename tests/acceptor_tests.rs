mod common;

use maileroo::inbound::acceptor::HotReloadAcceptor;
use maileroo::inbound::protocol::SmtpSession;
use maileroo::inbound::rate_limit::RateLimiter;
use maileroo::outbound::OutboundService;
use maileroo::web::DashboardEvent;
use std::path::PathBuf;
use std::sync::Arc;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::broadcast;

#[tokio::test]
async fn test_hot_reload_acceptor_lazy_loading_and_rejection() {
    common::run_on_all_dbs(|db| async move {
        // 1. Initialize an Acceptor with non-existent certificates (Lazy Load State)
        let temp_dir = tempfile::tempdir().unwrap();
        let non_existent_cert = temp_dir.path().join("non_existent_cert.pem");
        let non_existent_key = temp_dir.path().join("non_existent_key.pem");
        let lazy_acceptor = HotReloadAcceptor::new(
            non_existent_cert.clone(),
            non_existent_key.clone(),
            std::time::Duration::from_millis(100),
        )
        .unwrap();
        assert!(lazy_acceptor.config().is_none());

        // 2. Setup SmtpSession mock dependencies
        let (tx, _) = broadcast::channel::<DashboardEvent>(100);
        let outbound = Arc::new(OutboundService::new(
            "srs_secret".to_string(),
            hickory_resolver::TokioResolver::builder_tokio()
                .unwrap()
                .build()
                .unwrap(),
            "example.com".to_string(),
            db.clone(),
            temp_dir.path().to_path_buf(),
        ));
        let rate_limiter = Arc::new(RateLimiter::new());
        let blocklist = Arc::new(maileroo::inbound::blocklist::Blocklist::new(PathBuf::from(
            "dummy.conf",
        )));
        let limits = maileroo::inbound::rate_limit::InboundLimits::from_env();

        // --- Scenario A: STARTTLS with Lazy Loading Acceptor (Returns 454) ---
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let local_addr = listener.local_addr().unwrap();

        let client_handle = tokio::spawn(async move {
            let mut stream = TcpStream::connect(local_addr).await.unwrap();
            let mut reader = BufReader::new(&mut stream);
            let mut line = String::new();
            // Read welcome
            reader.read_line(&mut line).await.unwrap();
            // Send STARTTLS
            reader.get_mut().write_all(b"STARTTLS\r\n").await.unwrap();
            line.clear();
            reader.read_line(&mut line).await.unwrap();
            line
        });

        let (server_socket, _) = listener.accept().await.unwrap();
        let mut session = SmtpSession::new(
            server_socket,
            Some(lazy_acceptor.clone()), // Use the lazy acceptor
            db.clone(),
            temp_dir.path().to_path_buf(),
            outbound.clone(),
            tx.clone(),
            local_addr.ip(),
            rate_limiter.clone(),
            blocklist.clone(),
            limits.clone(),
        );

        // Run the session handler (it will process the command and drop)
        let _ = session.handle().await;

        let client_response = client_handle.await.unwrap();
        assert!(
            client_response.starts_with("454"),
            "Expected 454 TLS temporary error, got: {}",
            client_response
        );

        // --- Scenario B: STARTTLS with None Acceptor (Returns 502) ---
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let local_addr = listener.local_addr().unwrap();

        let client_handle = tokio::spawn(async move {
            let mut stream = TcpStream::connect(local_addr).await.unwrap();
            let mut reader = BufReader::new(&mut stream);
            let mut line = String::new();
            // Read welcome
            reader.read_line(&mut line).await.unwrap();
            // Send STARTTLS
            reader.get_mut().write_all(b"STARTTLS\r\n").await.unwrap();
            line.clear();
            reader.read_line(&mut line).await.unwrap();
            line
        });

        let (server_socket, _) = listener.accept().await.unwrap();
        let mut session_no_tls = SmtpSession::new(
            server_socket,
            None,
            db.clone(),
            temp_dir.path().to_path_buf(),
            outbound.clone(),
            tx.clone(),
            local_addr.ip(),
            rate_limiter.clone(),
            blocklist.clone(),
            limits.clone(),
        );

        let _ = session_no_tls.handle().await;
        let client_response = client_handle.await.unwrap();
        assert!(
            client_response.starts_with("502"),
            "Expected 502 Command not implemented, got: {}",
            client_response
        );

        // --- Scenario C: The Actual Hot Reload (Certs arrive on disk) ---

        // 0. macOS APFS/HFS+ has a 1-second granularity for file modification times.
        // Because this test runs so fast, generating the cert immediately results in the exact same
        // `mtime` as when the watcher was initialized. We sleep for 1.1 seconds to guarantee the
        // OS registers a strictly strictly newer timestamp for the file.
        tokio::time::sleep(std::time::Duration::from_millis(1100)).await;

        // 1. Generate the certificates directly to the paths the watcher is looking at
        common::generate_dummy_certs(&non_existent_cert, &non_existent_key);

        // 2. Poll the acceptor until the background worker detects the file and swaps it
        let mut reloaded = false;
        for _ in 0..20 {
            if lazy_acceptor.config().is_some() {
                reloaded = true;
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        }

        // 3. Assert the acceptor's internal state successfully hot-swapped!
        assert!(
            reloaded,
            "Acceptor failed to hot reload the certificate from disk within 1 second!"
        );

        // 4. Verify a real client can now complete the STARTTLS handshake
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let local_addr = listener.local_addr().unwrap();

        let client_handle = tokio::spawn(async move {
            let mut stream = TcpStream::connect(local_addr).await.unwrap();
            let mut reader = BufReader::new(&mut stream);
            let mut line = String::new();
            // Read welcome
            reader.read_line(&mut line).await.unwrap();
            // Send STARTTLS
            reader.get_mut().write_all(b"STARTTLS\r\n").await.unwrap();
            line.clear();
            reader.read_line(&mut line).await.unwrap();
            line
        });

        let (server_socket, _) = listener.accept().await.unwrap();
        let mut session = SmtpSession::new(
            server_socket,
            Some(lazy_acceptor), // Use the SAME instance that was lazy before!
            db.clone(),
            temp_dir.path().to_path_buf(),
            outbound.clone(),
            tx.clone(),
            local_addr.ip(),
            rate_limiter.clone(),
            blocklist.clone(),
            limits.clone(),
        );

        // Spawn session in background so it can do the actual handshake
        tokio::spawn(async move {
            let _ = session.handle().await;
        });

        let client_response = client_handle.await.unwrap();
        assert!(
            client_response.starts_with("220"),
            "Expected 220 Ready to start TLS, got: {}",
            client_response
        );
    })
    .await;
}
