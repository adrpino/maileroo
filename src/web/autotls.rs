use crate::config::AutoTlsConfig;
use axum::Router;
use rustls_acme::{AcmeConfig, caches::DirCache};
use std::net::SocketAddr;
use std::sync::Arc;
use tokio_rustls::rustls::ServerConfig;
use tokio_stream::StreamExt;

/// Helper to resolve the deterministic file path where `rustls-acme` stores
/// the certificate inside the configured cache directory.
pub fn resolve_acme_cert_path(
    cache_dir: &std::path::Path,
    domain: &str,
    acme_directory: &str,
) -> std::path::PathBuf {
    use base64::{Engine as _, engine::general_purpose::URL_SAFE_NO_PAD};
    use sha2::{Digest, Sha256};

    let mut hasher = Sha256::new();
    // domains are null-terminated in the rustls-acme SHA256 context
    hasher.update(domain.as_bytes());
    hasher.update(&[0]);
    // followed by the ACME directory URL
    hasher.update(acme_directory.as_bytes());
    let result = hasher.finalize();

    let hash = URL_SAFE_NO_PAD.encode(result);
    cache_dir.join(format!("cached_cert_{}", hash))
}

/// Runs the native Auto-TLS web server on the configured HTTP and HTTPS ports.
/// This handles Let's Encrypt HTTP-01 challenges and redirects standard
/// HTTP requests to HTTPS.
pub async fn run_auto_tls_web_server(app: Router, auto_tls: &AutoTlsConfig) -> anyhow::Result<()> {
    println!(
        "🔒 Starting Native Auto-TLS for domain {} (HTTP: {}, HTTPS: {})",
        auto_tls.domain, auto_tls.http_port, auto_tls.https_port
    );

    // 1. Configure the ACME engine
    let mut state_acme = AcmeConfig::new(vec![&auto_tls.domain])
        .contact(vec![format!("mailto:{}", auto_tls.email)])
        .cache(DirCache::new(auto_tls.cache_dir.clone()))
        .directory(&auto_tls.acme_directory)
        .state();

    // 2. Build the Rustls server configuration utilizing the ACME certificate resolver
    let resolver = state_acme.resolver();
    let rustls_config = Arc::new(
        ServerConfig::builder()
            .with_no_client_auth()
            .with_cert_resolver(resolver),
    );
    let acceptor = state_acme.axum_acceptor(rustls_config);

    // 3. Spawn the ACME background worker (moves state_acme into the task)
    tokio::spawn(async move {
        while let Some(event) = state_acme.next().await {
            match event {
                Ok(ok) => tracing::info!("ACME Event: {:?}", ok),
                Err(err) => tracing::error!("ACME Error: {:?}", err),
            }
        }
    });

    // 4. HTTPS Server with Axum / ACME Acceptor
    let https_addr = format!("0.0.0.0:{}", auto_tls.https_port);
    let https_listener = tokio::net::TcpListener::bind(&https_addr).await?;
    let https_server = axum_server::from_tcp(https_listener.into_std()?)?
        .acceptor(acceptor)
        .serve(app.into_make_service_with_connect_info::<SocketAddr>());

    // 5. HTTP Redirect + ACME Challenge server
    let domain_clone = auto_tls.domain.clone();
    let https_port = auto_tls.https_port;
    let redirect_app = Router::new().fallback(move |uri: axum::http::Uri| {
        let domain = domain_clone.clone();
        async move {
            let redirect_url = if https_port == 443 {
                format!("https://{}{}", domain, uri.path())
            } else {
                format!("https://{}:{}{}", domain, https_port, uri.path())
            };
            axum::response::Redirect::permanent(&redirect_url)
        }
    });
    let http_addr = format!("0.0.0.0:{}", auto_tls.http_port);
    let http_listener = tokio::net::TcpListener::bind(&http_addr).await?;
    let http_server = axum::serve(
        http_listener,
        redirect_app.into_make_service_with_connect_info::<SocketAddr>(),
    );

    tokio::select! {
        res = https_server => { res? }
        res = http_server => { res? }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use rustls_acme::CertCache;
    use tempfile::tempdir;

    #[tokio::test]
    async fn test_resolve_acme_cert_path_alignment() {
        let dir = tempdir().unwrap();
        let domain = "maileroo.test";
        let acme_directory = "https://acme-staging-v02.api.letsencrypt.org/directory";

        // 1. Calculate the expected path using our helper
        let resolved_path = resolve_acme_cert_path(dir.path(), domain, acme_directory);

        // 2. Instantiate rustls-acme's own DirCache pointing to the same directory
        let cache = DirCache::new(dir.path());

        // 3. Let rustls-acme serialize and save a mock certificate
        let dummy_cert = b"mock-pem-certificate-data";
        cache
            .store_cert(&[domain.to_string()], acme_directory, dummy_cert)
            .await
            .unwrap();

        // 4. Verify that the file was written exactly to our helper's resolved path
        assert!(resolved_path.exists(), "The resolved path does not exist!");

        let written_bytes = std::fs::read(&resolved_path).unwrap();
        assert_eq!(written_bytes, dummy_cert, "Written cert data mismatch!");
    }
}
