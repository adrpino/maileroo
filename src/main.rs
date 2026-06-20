use dotenvy::dotenv;
use maileroo::server::MailerooServer;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    dotenv().ok(); // Load .env file at runtime if present
    tracing_subscriber::fmt::init();
    let _ = tokio_rustls::rustls::crypto::aws_lc_rs::default_provider().install_default();

    // Boot up the core application (validates config, runs migrations, starts workers)
    let server = MailerooServer::bootstrap().await?;

    // Start the open-source version serving its default router
    server.start(None).await?;

    Ok(())
}
