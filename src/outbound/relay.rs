use anyhow::{Result, anyhow};
use base64::Engine;
use base64::engine::general_purpose::STANDARD as BASE64_STANDARD;
use std::sync::Arc;
use tokio::io::BufReader;
use tokio::net::TcpStream;
use tokio_rustls::TlsConnector;
use tokio_rustls::rustls::ClientConfig;
use tokio_rustls::rustls::pki_types::ServerName;
use tracing::{error, info};

use crate::outbound::{AnyStream, OutboundService};

#[derive(Clone, Debug)]
pub struct RelayConfig {
    pub host: String,
    pub port: u16,
    pub user: String,
    pub pass: String,
    pub allow_plaintext_auth: bool,
}

impl RelayConfig {
    /// Creates a `RelayConfig` with plaintext authentication disabled (`false`) by default.
    pub fn new(host: String, port: u16, user: String, pass: String) -> Self {
        Self {
            host,
            port,
            user,
            pass,
            allow_plaintext_auth: false,
        }
    }
}

/// Pure helper to parse the `SMTP_RELAY_ALLOW_PLAINTEXT_AUTH` setting.
/// Defaults to `false` unless explicitly set to `"true"`.
pub fn parse_allow_plaintext_auth(raw: &str) -> bool {
    raw.trim().eq_ignore_ascii_case("true")
}

/// Pure function that determines whether SMTP relay authentication is permitted
/// given the connection's TLS state and explicit configuration.
pub fn check_relay_auth_security(
    has_credentials: bool,
    tls_active: bool,
    allow_plaintext_auth: bool,
    host: &str,
    port: u16,
) -> Result<()> {
    if has_credentials && !tls_active && !allow_plaintext_auth {
        return Err(anyhow!(
            "Refusing to send SMTP AUTH PLAIN credentials over unencrypted connection to {}:{} (STARTTLS not offered by relay)",
            host,
            port
        ));
    }
    Ok(())
}

pub async fn send_via_relay(
    client_config: &Arc<ClientConfig>,
    identity_domain: &str,
    relay_config: &RelayConfig,
    to: &str,
    from_envelope: &str,
    body: &[u8],
) -> Result<()> {
    info!(
        "Routing outbound email via relay: {}:{}",
        relay_config.host, relay_config.port
    );

    // Connect to the SMTP relay
    let stream = TcpStream::connect((&relay_config.host[..], relay_config.port)).await?;
    let any_stream = AnyStream::Tcp { stream };
    let mut buf_reader = BufReader::new(any_stream);
    let mut response = String::new();

    // 1. Initial greeting response from relay
    OutboundService::read_full_response(&mut buf_reader, &mut response).await?;

    // 2. Send EHLO
    let mut capabilities = OutboundService::send_cmd(
        &mut buf_reader,
        &mut response,
        &format!("EHLO {}", identity_domain),
        false,
    )
    .await?;

    // 3. Upgrade to STARTTLS whenever the relay advertises it. Certificate
    // verification stays strict here (unlike opportunistic MX delivery).
    let supports_tls = capabilities.iter().any(|c| c.contains("STARTTLS"));
    let mut authenticated_stream = if supports_tls {
        info!("Relay supports STARTTLS, initiating upgrade...");
        OutboundService::send_cmd(&mut buf_reader, &mut response, "STARTTLS", false).await?;

        let connector = TlsConnector::from(client_config.clone());
        let any_stream = buf_reader.into_inner();
        if let AnyStream::Tcp { stream } = any_stream {
            let server_name = ServerName::try_from(relay_config.host.clone())?.to_owned();
            match connector.connect(server_name, stream).await {
                Ok(tls_stream) => {
                    let any_stream = AnyStream::Tls { stream: tls_stream };
                    let mut secure_reader = BufReader::new(any_stream);

                    // Re-EHLO after encryption
                    capabilities = OutboundService::send_cmd(
                        &mut secure_reader,
                        &mut response,
                        &format!("EHLO {}", identity_domain),
                        false,
                    )
                    .await?;

                    secure_reader
                }
                Err(e) => {
                    return Err(anyhow!("TLS handshake with relay failed: {}", e));
                }
            }
        } else {
            return Err(anyhow!(
                "Cannot upgrade: stream is not in a plain TCP state"
            ));
        }
    } else {
        buf_reader
    };

    // 4. Authenticate using AUTH PLAIN if credentials are provided
    let has_credentials = !relay_config.user.is_empty() && !relay_config.pass.is_empty();
    if let Err(err) = check_relay_auth_security(
        has_credentials,
        supports_tls,
        relay_config.allow_plaintext_auth,
        &relay_config.host,
        relay_config.port,
    ) {
        error!("{}", err);
        return Err(err);
    }

    if has_credentials {
        info!("Authenticating with SMTP relay...");
        // Generate standard AUTH PLAIN payload format: \0username\0password
        let raw_payload = format!("\0{}\0{}", relay_config.user, relay_config.pass);
        let encoded_payload = BASE64_STANDARD.encode(raw_payload.as_bytes());

        OutboundService::send_cmd(
            &mut authenticated_stream,
            &mut response,
            &format!("AUTH PLAIN {}", encoded_payload),
            false,
        )
        .await?;
        info!("Relay authentication successful!");
    }

    // 5. Send Mail Flow using the existing generic function!
    OutboundService::send_mail_flow(
        &mut authenticated_stream,
        &mut response,
        &capabilities,
        from_envelope.to_string(),
        to,
        body,
    )
    .await?;

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::io::{AsyncBufReadExt, AsyncWriteExt};
    use tokio::net::TcpListener;
    use tokio_rustls::rustls::RootCertStore;

    fn test_client_config() -> Arc<ClientConfig> {
        let _ = tokio_rustls::rustls::crypto::aws_lc_rs::default_provider().install_default();
        let mut root_store = RootCertStore::empty();
        for cert in rustls_native_certs::load_native_certs().unwrap() {
            root_store.add(cert).ok();
        }
        Arc::new(
            ClientConfig::builder()
                .with_root_certificates(root_store)
                .with_no_client_auth(),
        )
    }

    #[test]
    fn test_relay_config_defaults_to_disallowing_plaintext_auth() {
        let cfg = RelayConfig::new(
            "smtp.example.com".to_string(),
            587,
            "api".to_string(),
            "secret".to_string(),
        );
        assert!(!cfg.allow_plaintext_auth);

        assert!(!parse_allow_plaintext_auth(""));
        assert!(!parse_allow_plaintext_auth("false"));
        assert!(!parse_allow_plaintext_auth("0"));
        assert!(!parse_allow_plaintext_auth("yes"));
        assert!(!parse_allow_plaintext_auth("1"));
        assert!(parse_allow_plaintext_auth("true"));
        assert!(parse_allow_plaintext_auth("TRUE"));
        assert!(parse_allow_plaintext_auth(" true "));
    }

    #[test]
    fn test_check_relay_auth_security_pure() {
        // 1. Credentials with active TLS is permitted even when allow_plaintext_auth is false
        assert!(check_relay_auth_security(true, true, false, "smtp.example.com", 587).is_ok());

        // 2. Credentials without TLS is refused by default
        let err =
            check_relay_auth_security(true, false, false, "smtp.example.com", 587).unwrap_err();
        assert!(
            err.to_string().contains(
                "Refusing to send SMTP AUTH PLAIN credentials over unencrypted connection"
            )
        );

        // 3. Credentials without TLS is permitted when explicitly opted in
        assert!(check_relay_auth_security(true, false, true, "127.0.0.1", 1025).is_ok());

        // 4. Unauthenticated relay without TLS is permitted even when allow_plaintext_auth is false
        assert!(check_relay_auth_security(false, false, false, "127.0.0.1", 1025).is_ok());
    }

    #[tokio::test]
    async fn test_send_via_relay_refuses_plaintext_auth_without_starttls() {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();

        let server_handle = tokio::spawn(async move {
            let (socket, _) = listener.accept().await.unwrap();
            let mut reader = BufReader::new(socket);
            let mut buf = String::new();

            reader
                .get_mut()
                .write_all(b"220 smtp.mockrelay.test Welcome\r\n")
                .await
                .unwrap();

            reader.read_line(&mut buf).await.unwrap();
            assert!(buf.starts_with("EHLO"));

            // Advertise AUTH PLAIN without STARTTLS
            reader
                .get_mut()
                .write_all(b"250-smtp.mockrelay.test\r\n250 AUTH PLAIN\r\n")
                .await
                .unwrap();

            // Client must disconnect immediately without sending AUTH PLAIN or credentials
            buf.clear();
            let bytes_read = reader.read_line(&mut buf).await.unwrap();
            assert_eq!(
                bytes_read, 0,
                "Client must not send any command after refusing plaintext auth, got: {buf}"
            );
        });

        let relay_config = RelayConfig {
            host: "127.0.0.1".to_string(),
            port,
            user: "relay-user".to_string(),
            pass: "super-secret-relay-password".to_string(),
            allow_plaintext_auth: false,
        };

        let client_config = test_client_config();
        let res = send_via_relay(
            &client_config,
            "example.com",
            &relay_config,
            "recipient@example.org",
            "sender@example.com",
            b"Subject: Test\r\n\r\nBody",
        )
        .await;

        assert!(res.is_err(), "Expected plaintext auth to be refused");
        let err_msg = res.unwrap_err().to_string();
        assert!(
            err_msg.contains(
                "Refusing to send SMTP AUTH PLAIN credentials over unencrypted connection"
            ),
            "Unexpected error message: {err_msg}"
        );

        server_handle.await.unwrap();
    }

    #[tokio::test]
    async fn test_send_via_relay_allows_unauthenticated_plain_relay() {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();

        let server_handle = tokio::spawn(async move {
            let (socket, _) = listener.accept().await.unwrap();
            let mut reader = BufReader::new(socket);
            let mut buf = String::new();

            reader
                .get_mut()
                .write_all(b"220 mailhog.example Welcome\r\n")
                .await
                .unwrap();

            reader.read_line(&mut buf).await.unwrap();
            assert!(buf.starts_with("EHLO"));
            reader
                .get_mut()
                .write_all(b"250-mailhog.example\r\n250 PIPELINING\r\n")
                .await
                .unwrap();

            // Expect MAIL FROM directly (no AUTH PLAIN)
            buf.clear();
            reader.read_line(&mut buf).await.unwrap();
            assert!(buf.starts_with("MAIL FROM:"));
            reader.get_mut().write_all(b"250 OK\r\n").await.unwrap();

            buf.clear();
            reader.read_line(&mut buf).await.unwrap();
            assert!(buf.starts_with("RCPT TO:"));
            reader.get_mut().write_all(b"250 OK\r\n").await.unwrap();

            buf.clear();
            reader.read_line(&mut buf).await.unwrap();
            assert!(buf.starts_with("DATA"));
            reader
                .get_mut()
                .write_all(b"354 End data with <CR><LF>.<CR><LF>\r\n")
                .await
                .unwrap();

            loop {
                buf.clear();
                reader.read_line(&mut buf).await.unwrap();
                if buf == ".\r\n" {
                    break;
                }
            }
            reader.get_mut().write_all(b"250 Queued\r\n").await.unwrap();

            buf.clear();
            reader.read_line(&mut buf).await.unwrap();
            assert!(buf.starts_with("QUIT"));
            reader.get_mut().write_all(b"221 Bye\r\n").await.unwrap();
        });

        let relay_config = RelayConfig {
            host: "127.0.0.1".to_string(),
            port,
            user: String::new(),
            pass: "mailhog".to_string(),
            allow_plaintext_auth: false,
        };

        let client_config = test_client_config();
        let res = send_via_relay(
            &client_config,
            "example.com",
            &relay_config,
            "recipient@example.org",
            "sender@example.com",
            b"Subject: Test\r\n\r\nBody",
        )
        .await;

        assert!(
            res.is_ok(),
            "Unauthenticated plain relay should succeed: {:?}",
            res.err()
        );
        server_handle.await.unwrap();
    }
}
