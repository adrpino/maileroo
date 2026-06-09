use dotenvy::dotenv;
use std::sync::Arc;
use tokio_rustls::rustls::{ClientConfig, RootCertStore};

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    dotenv().ok();
    tracing_subscriber::fmt::init();
    
    // Initialize Rustls CryptoProvider (required for TLS handshake)
    let _ = tokio_rustls::rustls::crypto::aws_lc_rs::default_provider().install_default();

    println!("🧪 SMTP Relay Live Verification Tool");

    // 1. Load Relay Password (looks for TEST_ first, then standard, then SMTP_TOKEN fallbacks)
    let relay_pass = std::env::var("TEST_SMTP_RELAY_TOKEN")
        .or_else(|_| std::env::var("TEST_SMTP_RELAY_PASSWORD"))
        .or_else(|_| std::env::var("SMTP_RELAY_TOKEN"))
        .or_else(|_| std::env::var("SMTP_RELAY_PASSWORD"))
        .or_else(|_| std::env::var("SMTP_TOKEN"))
        .expect("TEST_SMTP_RELAY_PASSWORD must be set in your .env file");
    
    // 2. Load Relay Host (defaults to Mailtrap sandbox for safe local testing)
    let relay_host = std::env::var("TEST_SMTP_RELAY_HOST")
        .or_else(|_| std::env::var("SMTP_RELAY_HOST"))
        .unwrap_or_else(|_| "sandbox.smtp.mailtrap.io".to_string());

    // 3. Load Relay User (defaults to empty string if using sandbox, or "api" for production)
    let relay_user = std::env::var("TEST_SMTP_RELAY_USER")
        .or_else(|_| std::env::var("SMTP_RELAY_USER"))
        .unwrap_or_else(|_| "".to_string());

    // 4. Load Relay Port (defaults to standard STARTTLS sandbox port 2525)
    let relay_port_str = std::env::var("TEST_SMTP_RELAY_PORT")
        .or_else(|_| std::env::var("SMTP_RELAY_PORT"))
        .unwrap_or_else(|_| "2525".to_string());
    let relay_port: u16 = relay_port_str.parse()?;

    // 5. Load Recipient and Sender
    let to_email = std::env::var("TEST_RECIPIENT_EMAIL")
        .expect("TEST_RECIPIENT_EMAIL (your test recipient mailbox) must be set in your .env file or environment");
    let from_email = std::env::var("TEST_SENDER_EMAIL")
        .unwrap_or_else(|_| "hello@example.com".to_string());

    // Initialize Root Certificate Store
    let mut root_store = RootCertStore::empty();
    for cert in rustls_native_certs::load_native_certs().expect("could not load platform certs") {
        root_store.add(cert)?;
    }
    let client_config = Arc::new(
        ClientConfig::builder()
            .with_root_certificates(root_store)
            .with_no_client_auth()
    );

    let config = maileroo::outbound::relay::RelayConfig {
        host: relay_host.clone(),
        port: relay_port,
        user: relay_user,
        pass: relay_pass,
    };

    println!("📡 Connecting to real SMTP host: {}:{}...", relay_host, relay_port);

    let test_body = format!(
        "Subject: Maileroo Relay Live Test\r\nFrom: {}\r\nTo: {}\r\n\r\nThis is a real-world live integration test from Maileroo!",
        from_email, to_email
    );

    match maileroo::outbound::relay::send_via_relay(
        &client_config,
        "example.com",
        &config,
        &to_email,
        &from_email,
        test_body.as_bytes(),
    ).await {
        Ok(_) => {
            println!("🎉 SUCCESS! The SMTP relay accepted our connection, completed the STARTTLS handshake, authenticated, and queued the test email successfully!");
        }
        Err(e) => {
            println!("❌ FAILED! Connection or SMTP Handshake Error: {}", e);
        }
    }

    Ok(())
}
