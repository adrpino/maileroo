use base64::Engine;
use std::sync::Arc;
use tokio::io::BufReader;
use tokio::net::TcpStream;
use tokio_rustls::TlsConnector;
use tokio_rustls::rustls::ClientConfig;
use tokio_rustls::rustls::pki_types::ServerName;
use tracing::info;

use crate::outbound::{AnyStream, OutboundService};

#[derive(Clone, Debug)]
pub struct RelayConfig {
    pub host: String,
    pub port: u16,
    pub user: String,
    pub pass: String,
}

pub async fn send_via_relay(
    client_config: &Arc<ClientConfig>,
    identity_domain: &str,
    relay_config: &RelayConfig,
    to: &str,
    from_envelope: &str,
    body: &[u8],
) -> anyhow::Result<()> {
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

    // 3. Upgrade to STARTTLS if supported and using port 587
    let supports_tls = capabilities.iter().any(|c| c.contains("STARTTLS"));
    let mut authenticated_stream = if supports_tls && relay_config.port == 587 {
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
                Err(e) => return Err(anyhow::anyhow!("TLS handshake with relay failed: {}", e)),
            }
        } else {
            return Err(anyhow::anyhow!(
                "Cannot upgrade: stream is not in a plain TCP state"
            ));
        }
    } else {
        buf_reader
    };

    // 4. Authenticate using AUTH PLAIN if credentials are provided
    if !relay_config.user.is_empty() && !relay_config.pass.is_empty() {
        info!("Authenticating with SMTP relay...");
        // Generate standard AUTH PLAIN payload format: \0username\0password
        let raw_payload = format!("\0{}\0{}", relay_config.user, relay_config.pass);
        let encoded_payload =
            base64::engine::general_purpose::STANDARD.encode(raw_payload.as_bytes());

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
