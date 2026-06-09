use argon2::{
    Argon2,
    password_hash::{PasswordHasher, SaltString, rand_core::OsRng},
};
use sqlx::PgPool;
use std::sync::Arc;
use tokio::io::{AsyncBufReadExt, AsyncRead, AsyncWrite, AsyncWriteExt, BufReader};
use tokio::net::TcpStream;
use tokio_rustls::TlsConnector;
use tokio_rustls::rustls::pki_types::{CertificateDer, ServerName, UnixTime};
use tokio_rustls::rustls::{
    ClientConfig, Error as RustlsError, RootCertStore,
    client::danger::{HandshakeSignatureValid, ServerCertVerified, ServerCertVerifier},
};
use uuid::Uuid;

#[derive(Debug)]
struct DummyVerifier;

impl ServerCertVerifier for DummyVerifier {
    fn verify_server_cert(
        &self,
        _end_entity: &CertificateDer<'_>,
        _intermediates: &[CertificateDer<'_>],
        _server_name: &ServerName<'_>,
        _ocsp_response: &[u8],
        _now: UnixTime,
    ) -> Result<ServerCertVerified, RustlsError> {
        Ok(ServerCertVerified::assertion())
    }

    fn verify_tls12_signature(
        &self,
        _message: &[u8],
        _cert: &CertificateDer<'_>,
        _dss: &tokio_rustls::rustls::DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, RustlsError> {
        Ok(HandshakeSignatureValid::assertion())
    }

    fn verify_tls13_signature(
        &self,
        _message: &[u8],
        _cert: &CertificateDer<'_>,
        _dss: &tokio_rustls::rustls::DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, RustlsError> {
        Ok(HandshakeSignatureValid::assertion())
    }

    fn supported_verify_schemes(&self) -> Vec<tokio_rustls::rustls::SignatureScheme> {
        vec![
            tokio_rustls::rustls::SignatureScheme::RSA_PKCS1_SHA1,
            tokio_rustls::rustls::SignatureScheme::ECDSA_NISTP256_SHA256,
            tokio_rustls::rustls::SignatureScheme::ECDSA_NISTP384_SHA384,
            tokio_rustls::rustls::SignatureScheme::ECDSA_NISTP521_SHA512,
            tokio_rustls::rustls::SignatureScheme::RSA_PSS_SHA256,
            tokio_rustls::rustls::SignatureScheme::RSA_PSS_SHA384,
            tokio_rustls::rustls::SignatureScheme::RSA_PSS_SHA512,
            tokio_rustls::rustls::SignatureScheme::ED25519,
            tokio_rustls::rustls::SignatureScheme::ED448,
        ]
    }
}

const EMAIL_BODY: &str = "From: sender@example.com\r\n\
To: hello@maileroo.test\r\n\
Subject: Realistic Test Email\r\n\
\r\n\
This is the body of the email.\r\n\
It can have multiple lines.\r\n\
.";

async fn seed_db() -> Result<(), Box<dyn std::error::Error>> {
    dotenvy::dotenv().ok();
    let db_url = std::env::var("DATABASE_URL").expect("DATABASE_URL must be set");
    let pool = PgPool::connect(&db_url).await?;

    println!("🌱 Seeding DB for test (Syncing with setup_db)...");

    let user_email = "admin@admin.com";
    let password = "b3a6fa7e64458ccb";
    let domain_name = "maileroo.test";
    let subdomain = "hello";

    // Hash password using same logic as setup_db.rs
    let salt = SaltString::generate(&mut OsRng);
    let argon2 = Argon2::default();
    let password_hash = argon2
        .hash_password(password.as_bytes(), &salt)
        .map_err(|e| e.to_string())?
        .to_string();

    // 1. User: Insert or update
    sqlx::query(
        "INSERT INTO users (id, email, password_hash, is_admin) VALUES ($1, $2, $3, $4) ON CONFLICT (email) DO UPDATE SET password_hash = $3, is_admin = $4",
    )
    .bind(Uuid::new_v4())
    .bind(user_email)
    .bind(&password_hash)
    .bind(true)
    .execute(&pool)
    .await?;

    let user_id: Uuid = sqlx::query_scalar("SELECT id FROM users WHERE email = $1")
        .bind(user_email)
        .fetch_one(&pool)
        .await?;

    // 2. Domain: Insert or update
    sqlx::query(
        "INSERT INTO domains (id, name) VALUES ($1, $2) ON CONFLICT (name) DO UPDATE SET name = EXCLUDED.name",
    )
    .bind(Uuid::new_v4())
    .bind(domain_name)
    .execute(&pool)
    .await?;

    let domain_id: Uuid = sqlx::query_scalar("SELECT id FROM domains WHERE name = $1")
        .bind(domain_name)
        .fetch_one(&pool)
        .await?;

    // 3. Alias: Insert or update
    sqlx::query(
        r#"
        INSERT INTO aliases (id, user_id, domain_id, subdomain, destination_email, auto_forward) 
        VALUES ($1, $2, $3, $4, $5, $6) 
        ON CONFLICT (subdomain, domain_id) 
        DO UPDATE SET destination_email = EXCLUDED.destination_email, auto_forward = EXCLUDED.auto_forward
        "#,
    )
    .bind(Uuid::new_v4())
    .bind(user_id)
    .bind(domain_id)
    .bind(subdomain)
    .bind(user_email)
    .bind(true)
    .execute(&pool)
    .await?;

    println!("✅ DB seeded (hello@maileroo.test -> admin@admin.com)");
    pool.close().await;
    Ok(())
}

async fn send_cmd<S: AsyncRead + AsyncWrite + Unpin>(
    reader: &mut BufReader<S>,
    response: &mut String,
    cmd: &str,
) -> Result<(), std::io::Error> {
    println!("> {}", cmd);
    reader
        .get_mut()
        .write_all(format!("{}\r\n", cmd).as_bytes())
        .await?;
    reader.get_mut().flush().await?;
    response.clear();
    reader.read_line(response).await?;
    println!("< {}", response.trim());
    Ok(())
}

async fn run_plain_test() -> Result<(), Box<dyn std::error::Error>> {
    println!("--- Starting Plain SMTP Test ---");
    let stream = TcpStream::connect("127.0.0.1:2525").await?;
    let mut reader = BufReader::new(stream);
    let mut response = String::new();

    reader.read_line(&mut response).await?;
    println!("< {}", response.trim());

    send_cmd(&mut reader, &mut response, "EHLO localhost").await?;
    let email_domain = std::env::var("EMAIL_DOMAINS")
        .unwrap_or_else(|_| "example.com".to_string())
        .split(',')
        .next()
        .unwrap_or("example.com")
        .to_string();
    send_cmd(
        &mut reader,
        &mut response,
        &format!(
            "MAIL FROM:<SRS0+EVHV=21=gmail.com=megannhl99@{}>",
            email_domain
        ),
    )
    .await?;
    send_cmd(&mut reader, &mut response, "RCPT TO:<hello@maileroo.test>").await?;
    send_cmd(&mut reader, &mut response, "DATA").await?;
    send_cmd(&mut reader, &mut response, EMAIL_BODY).await?;
    send_cmd(&mut reader, &mut response, "QUIT").await?;
    Ok(())
}

async fn run_tls_test() -> Result<(), Box<dyn std::error::Error>> {
    println!("\n--- Starting STARTTLS SMTP Test ---");
    let stream = TcpStream::connect("127.0.0.1:2525").await?;
    let mut reader = BufReader::new(stream);
    let mut response = String::new();

    // Initial greeting
    reader.read_line(&mut response).await?;
    println!("< {}", response.trim());

    send_cmd(&mut reader, &mut response, "EHLO localhost").await?;
    send_cmd(&mut reader, &mut response, "STARTTLS").await?;

    // Perform TLS Handshake
    let mut root_store = RootCertStore::empty();
    let cert_file = &mut std::io::BufReader::new(std::fs::File::open("certs/fullchain.pem")?);
    for cert in rustls_pemfile::certs(cert_file) {
        root_store.add(cert?)?;
    }
    // Seed the database so the SMTP server acknowledges hello@maileroo.test
    if let Err(e) = seed_db().await {
        eprintln!("Failed to seed DB: {}. Make sure docker-compose is up.", e);
        return Err(e);
    }

    let config = ClientConfig::builder()
        .dangerous()
        .with_custom_certificate_verifier(std::sync::Arc::new(DummyVerifier))
        .with_no_client_auth();
    let connector = TlsConnector::from(Arc::new(config));

    // Upgrade the stream
    let stream = reader.into_inner();
    let domain = ServerName::try_from("localhost")?.to_owned();
    let tls_stream = connector.connect(domain, stream).await?;
    let mut reader = BufReader::new(tls_stream);

    println!("[TLS Handshake Complete]");

    // SMTP requires EHLO again after STARTTLS
    send_cmd(&mut reader, &mut response, "EHLO localhost").await?;
    let email_domain = std::env::var("EMAIL_DOMAINS")
        .unwrap_or_else(|_| "example.com".to_string())
        .split(',')
        .next()
        .unwrap_or("example.com")
        .to_string();
    send_cmd(
        &mut reader,
        &mut response,
        &format!(
            "MAIL FROM:<SRS0+EVHV=21=gmail.com=megannhl99@{}>",
            email_domain
        ),
    )
    .await?;
    send_cmd(&mut reader, &mut response, "RCPT TO:<hello@maileroo.test>").await?;
    send_cmd(&mut reader, &mut response, "DATA").await?;
    send_cmd(&mut reader, &mut response, EMAIL_BODY).await?;
    send_cmd(&mut reader, &mut response, "QUIT").await?;

    Ok(())
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    // Initialize the crypto provider for the client
    let _ = tokio_rustls::rustls::crypto::ring::default_provider().install_default();
    // SEED HERE FIRST!
    if let Err(e) = seed_db().await {
        eprintln!("❌ Failed to seed DB: {}.", e);
        return Err(e);
    }

    if let Err(e) = run_plain_test().await {
        eprintln!("Plain test failed: {}", e);
    }

    if let Err(e) = run_tls_test().await {
        eprintln!("TLS test failed: {}", e);
    }

    Ok(())
}
