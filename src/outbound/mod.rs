pub mod mime;
pub mod srs;
pub mod relay;
pub mod dkim;
pub mod queue;

pub use dkim::generate_dkim_key_pair;
pub use queue::{get_job_file_path, enqueue_job, calculate_next_retry, process_queue_tick, start_queue_daemon};

use crate::dns::check_spf_for_domain;
use crate::outbound::mime::{MimeEmail, build_mime, rewrite_body_for_forward};
use hickory_resolver::TokioResolver;
use pin_project_lite::pin_project;
use std::pin::Pin;
use std::sync::Arc;
use std::task::{Context, Poll};
use tokio::io::{AsyncBufReadExt, AsyncRead, AsyncWrite, AsyncWriteExt, BufReader, ReadBuf};
use tokio::net::TcpStream;
use tokio_rustls::TlsConnector;
use tokio_rustls::rustls::pki_types::ServerName;
use tokio_rustls::rustls::{ClientConfig, RootCertStore};
use tracing::info;

pin_project! {
    #[project = AnyStreamProj]
    pub(crate) enum AnyStream {
        Tcp { #[pin] stream: TcpStream },
        Tls { #[pin] stream: tokio_rustls::client::TlsStream<TcpStream> },
    }
}

impl AsyncRead for AnyStream {
    fn poll_read(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<std::io::Result<()>> {
        match self.project() {
            AnyStreamProj::Tcp { stream } => stream.poll_read(cx, buf),
            AnyStreamProj::Tls { stream } => stream.poll_read(cx, buf),
        }
    }
}

impl AsyncWrite for AnyStream {
    fn poll_write(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<std::io::Result<usize>> {
        match self.project() {
            AnyStreamProj::Tcp { stream } => stream.poll_write(cx, buf),
            AnyStreamProj::Tls { stream } => stream.poll_write(cx, buf),
        }
    }

    fn poll_flush(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<std::io::Result<()>> {
        match self.project() {
            AnyStreamProj::Tcp { stream } => stream.poll_flush(cx),
            AnyStreamProj::Tls { stream } => stream.poll_flush(cx),
        }
    }

    fn poll_shutdown(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<std::io::Result<()>> {
        match self.project() {
            AnyStreamProj::Tcp { stream } => stream.poll_shutdown(cx),
            AnyStreamProj::Tls { stream } => stream.poll_shutdown(cx),
        }
    }
}

use std::path::PathBuf;

pub struct OutboundService {
    resolver: TokioResolver,
    client_config: Arc<ClientConfig>,
    srs_secret: String,
    identity_domain: String,
    db: crate::db::DbPool,
    storage_dir: PathBuf,
    pub relay_override: Option<crate::outbound::relay::RelayConfig>,
}

impl OutboundService {
    pub fn new(
        srs_secret: String,
        resolver: TokioResolver,
        identity_domain: String,
        db: crate::db::DbPool,
        storage_dir: PathBuf,
    ) -> Self {
        // Initialize TLS
        let mut root_store: RootCertStore = RootCertStore::empty();
        for cert in rustls_native_certs::load_native_certs().expect("could not load platform certs")
        {
            root_store.add(cert).expect("Failed to add certificate");
        }

        let client_config = ClientConfig::builder()
            .with_root_certificates(root_store)
            .with_no_client_auth();
        Self {
            resolver,
            client_config: Arc::new(client_config),
            srs_secret,
            identity_domain,
            db,
            storage_dir,
            relay_override: None,
        }
    }

    pub fn with_relay_override(mut self, config: crate::outbound::relay::RelayConfig) -> Self {
        self.relay_override = Some(config);
        self
    }

    pub fn identity_domain(&self) -> &str {
        &self.identity_domain
    }

    pub async fn check_spf(&self, domain: &str, client_ip: std::net::IpAddr) -> bool {
        check_spf_for_domain(&self.resolver, domain, client_ip).await
    }

    /// Primary function to send a reply directly to the recipient's mail server
    pub async fn send_reply(
        &self,
        to: &str,
        from_alias: &str,
        subject: &str,
        body: &str,
        original_message_id: Option<String>,
        new_message_id: Option<String>,
    ) -> anyhow::Result<()> {
        let email = MimeEmail {
            from: from_alias.to_string(),
            to: to.to_string(),
            subject: subject.to_string(),
            text_body: body.to_string(),
            html_body: None,
            message_id: new_message_id,
            in_reply_to: original_message_id.clone(),
            references: original_message_id,
        };

        let mime = build_mime(&email);
        self.send_raw(to, from_alias, mime.as_bytes()).await
    }

    /// Primary function to forward an email directly to the recipient's mail server
    pub async fn send_forward(
        &self,
        to: &str,
        from: &str,
        body: &[u8],
    ) -> anyhow::Result<()> {
        // Rewrite the 'from' address using SRS
        let srs_from = crate::outbound::srs::encode_srs(from, &self.identity_domain, &self.srs_secret);
        self.send_raw(to, &srs_from, body).await
    }

    /// Primary function to forward an email with modified headers for two-way reply tracking
    pub async fn send_forward_with_reply_token(
        &self,
        to: &str,
        from: &str,
        reply_token: &str,
        body: &[u8],
    ) -> anyhow::Result<()> {
        // 1. Rewrite the envelope sender using SRS of the reply email address
        let reply_from_email = format!("{}@{}", reply_token, self.identity_domain);
        let srs_from = crate::outbound::srs::encode_srs(&reply_from_email, &self.identity_domain, &self.srs_secret);

        // 2. Rewrite the headers of the MIME body using our pure modular helper
        let clean_body = rewrite_body_for_forward(body, from, &reply_from_email, to);

        // 3. Send out the raw modified email
        self.send_raw(to, &srs_from, clean_body.as_bytes()).await
    }

    /// Primary function to send a direct outbound email to a recipient
    pub async fn send_firsthand(
        &self,
        to: &str,
        from: &str,
        body: &[u8],
    ) -> anyhow::Result<()> {
        self.send_raw(to, from, body).await
    }

    /// Decodes an SRS-rewritten address back to its original sender
    pub fn decode_srs(&self, srs_address: &str) -> Option<String> {
        crate::outbound::srs::decode_srs(srs_address, &self.srs_secret)
    }

    /// Internal function to send raw email data to the recipient's MX
    pub(crate) async fn send_raw(
        &self,
        to: &str,
        from_envelope: &str,
        body: &[u8],
    ) -> anyhow::Result<()> {
        match self.send_raw_inner(to, from_envelope, body).await {
            Ok(()) => Ok(()),
            Err(e) => {
                let err_str = e.to_string();
                let is_permanent = err_str.contains("550")
                    || err_str.contains("554")
                    || err_str.contains("552")
                    || err_str.contains("501")
                    || err_str.contains("Invalid recipient")
                    || err_str.contains("Command contains invalid characters");

                if !is_permanent {
                    tracing::warn!("Transient delivery failure, automatically queuing for retry: {}", err_str);
                    if let Err(enqueue_err) = crate::outbound::queue::enqueue_job(
                        &self.db,
                        &self.storage_dir,
                        from_envelope,
                        to,
                        body,
                    ).await {
                        tracing::error!("Failed to enqueue failed delivery job: {}", enqueue_err);
                        return Err(e);
                    }
                    Ok(())
                } else {
                    tracing::error!("Permanent delivery failure, skipping queue: {}", err_str);
                    Err(e)
                }
            }
        }
    }

    /// Internal function to perform actual MX/Relay SMTP transactions
    async fn send_raw_inner(
        &self,
        to: &str,
        from_envelope: &str,
        body: &[u8],
    ) -> anyhow::Result<()> {
        let sender_domain = from_envelope.split('@').nth(1).unwrap_or("").trim().to_lowercase();
        
        let dkim_info = crate::db::get_dkim_key_by_domain(&self.db, &sender_domain)
            .await
            .ok()
            .flatten();

        let mut signed_body = body.to_vec();
        if let Some((Some(private_b64), selector)) = dkim_info {
            signed_body = crate::outbound::dkim::sign_message_with_dkim(
                &signed_body,
                &sender_domain,
                &private_b64,
                &selector,
            );
        }
        let body = &signed_body;

        // Intercept and route via SMTP Relay if configured
        let relay_config = if let Some(ref config) = self.relay_override {
            Some(config.clone())
        } else {
            let mut pass = crate::config::get_config("SMTP_RELAY_TOKEN", "");
            if pass.is_empty() {
                pass = crate::config::get_config("SMTP_RELAY_PASSWORD", "");
            }
            if pass.is_empty() {
                pass = crate::config::get_config("SMTP_TOKEN", "");
            }
            if !pass.is_empty() {
                let host = crate::config::get_config("SMTP_RELAY_HOST", "live.smtp.mailtrap.io");
                let port_str = crate::config::get_config("SMTP_RELAY_PORT", "587");
                let port: u16 = port_str.parse().unwrap_or(587);
                let user = crate::config::get_config("SMTP_RELAY_USER", "api");
                Some(crate::outbound::relay::RelayConfig {
                    host,
                    port,
                    user,
                    pass,
                })
            } else {
                None
            }
        };

        if let Some(config) = relay_config {
            return crate::outbound::relay::send_via_relay(
                &self.client_config,
                &self.identity_domain,
                &config,
                to,
                from_envelope,
                body,
            ).await;
        }

        let domain = to
            .split('@')
            .nth(1)
            .ok_or_else(|| anyhow::anyhow!("Invalid recipient email address"))?;

        // 1. Resolve MX Records
        info!("Looking up MX records for domain: {}", domain);
        let mx_lookup = self.resolver.mx_lookup(domain).await?;

        let best_mx = mx_lookup
            .answers()
            .iter()
            .filter_map(|r| {
                if let hickory_resolver::proto::rr::RData::MX(mx) = &r.data {
                    Some(mx)
                } else {
                    None
                }
            })
            .min_by_key(|r| r.preference)
            .ok_or_else(|| anyhow::anyhow!("No MX records found for domain: {}", domain))?;

        let host = best_mx.exchange.to_utf8();
        let clean_host = host.trim_end_matches('.').to_string();
        info!("Found best MX for {}: {} (pref: {})", domain, clean_host, best_mx.preference);

        // 2. Connect via IPv4 ONLY (To satisfy Gmail/strict SPF/PTR guidelines)
        info!("Enforcing IPv4 lookup for MX: {}", clean_host);
        let ip_addr = match self.resolver.ipv4_lookup(&clean_host).await {
            Ok(lookup) => lookup
                .answers()
                .iter()
                .filter_map(|r| {
                    if let hickory_resolver::proto::rr::RData::A(a) = &r.data {
                        Some(std::net::IpAddr::V4(a.0))
                    } else {
                        None
                    }
                })
                .next(),
            Err(_) => None,
        }.ok_or_else(|| anyhow::anyhow!("Could not resolve IPv4 for MX {}", clean_host))?;

        info!("Connecting via IPv4 to {}:25 ({})...", ip_addr, clean_host);
        let stream = TcpStream::connect((ip_addr, 25)).await?;

        let any_stream = AnyStream::Tcp { stream };
        let mut response = String::new();

        // 3. SMTP Client Handshake
        let mut buf_reader = BufReader::new(any_stream);
        Self::read_full_response(&mut buf_reader, &mut response).await?;

        let mut capabilities = Self::send_cmd(
            &mut buf_reader,
            &mut response,
            &format!("EHLO {}", self.identity_domain),
            false,
        ).await?;

        // 4. Opportunistic TLS (STARTTLS)
        let supports_tls = capabilities.iter().any(|c| c.contains("STARTTLS"));
        if supports_tls {
            info!("STARTTLS detected, initiating upgrade...");
            Self::send_cmd(&mut buf_reader, &mut response, "STARTTLS", false).await?;

            let connector = TlsConnector::from(self.client_config.clone());
            let any_stream = buf_reader.into_inner();
            if let AnyStream::Tcp { stream } = any_stream {
                let server_name = ServerName::try_from(clean_host.clone())?.to_owned();
                match connector.connect(server_name, stream).await {
                    Ok(tls_stream) => {
                        let any_stream = AnyStream::Tls { stream: tls_stream };
                        let mut buf_reader = BufReader::new(any_stream);

                        // Re-EHLO after encryption
                        capabilities = Self::send_cmd(
                            &mut buf_reader,
                            &mut response,
                            &format!("EHLO {}", self.identity_domain),
                            false,
                        ).await?;

                        Self::send_mail_flow(
                            &mut buf_reader,
                            &mut response,
                            &capabilities,
                            from_envelope.to_string(),
                            to,
                            body,
                        ).await?;
                    }
                    Err(e) => return Err(anyhow::anyhow!("TLS connection failed: {}", e)),
                }
            }
        } else {
            Self::send_mail_flow(
                &mut buf_reader,
                &mut response,
                &capabilities,
                from_envelope.to_string(),
                to,
                body,
            ).await?;
        }

        Ok(())
    }

    pub(crate) async fn send_mail_flow<S: AsyncRead + AsyncWrite + Unpin>(
        reader: &mut BufReader<S>,
        response: &mut String,
        capabilities: &[String],
        srs_from: String,
        to: &str,
        body: &[u8],
    ) -> anyhow::Result<()> {
        let supports_pipelining = capabilities.iter().any(|c| c.contains("PIPELINING"));

        // MAIL FROM
        Self::send_cmd(reader, response, &format!("MAIL FROM:<{}>", srs_from), false).await?;

        // RCPT TO
        Self::send_cmd(reader, response, &format!("RCPT TO:<{}>", to), !supports_pipelining).await?;

        // DATA
        Self::send_cmd(reader, response, "DATA", !supports_pipelining).await?;

        let stuffed_body = Self::apply_dot_stuffing(body);
        let writer = reader.get_mut();
        writer.write_all(&stuffed_body).await?;

        if !stuffed_body.ends_with(b"\r\n") {
            writer.write_all(b"\r\n").await?;
        }

        Self::send_cmd(reader, response, ".", !supports_pipelining).await?;
        Self::send_cmd(reader, response, "QUIT", !supports_pipelining).await?;

        Ok(())
    }

    pub fn apply_dot_stuffing(payload: &[u8]) -> Vec<u8> {
        let mut stuffed = Vec::with_capacity(payload.len() + 10);
        let mut iter = payload.iter().peekable();
        let mut at_line_start = true;

        while let Some(&byte) = iter.next() {
            if at_line_start && byte == b'.' {
                stuffed.push(b'.');
                stuffed.push(b'.');
                at_line_start = false;
            } else {
                stuffed.push(byte);
                if byte == b'\r' {
                    if let Some(&&b'\n') = iter.peek() {
                        stuffed.push(b'\n');
                        iter.next();
                        at_line_start = true;
                    } else {
                        at_line_start = false;
                    }
                } else if byte == b'\n' {
                    at_line_start = true;
                } else {
                    at_line_start = false;
                }
            }
        }
        stuffed
    }

    pub(crate) async fn read_full_response<S: AsyncRead + AsyncWrite + Unpin>(
        reader: &mut BufReader<S>,
        response: &mut String,
    ) -> anyhow::Result<Vec<String>> {
        let mut lines = Vec::new();
        loop {
            response.clear();
            let n = reader.read_line(response).await?;
            if n == 0 { return Err(anyhow::anyhow!("Connection closed unexpectedly")); }
            let trimmed = response.trim();
            lines.push(trimmed.to_string());
            if response.len() < 4 || response.as_bytes()[3] != b'-' { break; }
        }
        Ok(lines)
    }

    pub(crate) async fn send_cmd<S: AsyncRead + AsyncWrite + Unpin>(
        reader: &mut BufReader<S>,
        response: &mut String,
        cmd: &str,
        delay: bool,
    ) -> anyhow::Result<Vec<String>> {
        if cmd.contains('\r') || cmd.contains('\n') {
            return Err(anyhow::anyhow!("Command contains invalid characters"));
        }
        if delay { tokio::time::sleep(std::time::Duration::from_millis(100)).await; }
        let full_cmd = format!("{}\r\n", cmd);
        let writer = reader.get_mut();
        writer.write_all(full_cmd.as_bytes()).await?;
        writer.flush().await?;
        let lines = Self::read_full_response(reader, response).await?;
        let last_line = lines.last().unwrap();
        if !last_line.starts_with('2') && !last_line.starts_with('3') {
            return Err(anyhow::anyhow!("SMTP Error for command '{}': {}", cmd, lines.join("\n")));
        }
        Ok(lines)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::net::TcpListener;
    use tokio::io::AsyncWriteExt;

    #[tokio::test]
    async fn test_outbound_relay_protocol_flow() {
        // 1. Setup a mock SMTP relay loopback server on an ephemeral port
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();

        // Spawn mock SMTP server background task
        tokio::spawn(async move {
            let (socket, _) = listener.accept().await.unwrap();
            let mut reader = BufReader::new(socket);
            let mut buf = String::new();

            // S1: Greeting
            reader.get_mut().write_all(b"220 smtp.mockrelay.com Welcome\r\n").await.unwrap();

            // C2: Read EHLO
            buf.clear();
            reader.read_line(&mut buf).await.unwrap();
            assert!(buf.starts_with("EHLO"));

            // S2: Capabilities response
            reader.get_mut().write_all(b"250-smtp.mockrelay.com\r\n250 AUTH PLAIN\r\n").await.unwrap();

            // C3: Read AUTH PLAIN
            buf.clear();
            reader.read_line(&mut buf).await.unwrap();
            assert!(buf.contains("AUTH PLAIN"));

            // S3: Auth OK
            reader.get_mut().write_all(b"235 Authentication successful\r\n").await.unwrap();

            // C4: MAIL FROM
            buf.clear();
            reader.read_line(&mut buf).await.unwrap();
            assert!(buf.contains("MAIL FROM"));
            reader.get_mut().write_all(b"250 OK\r\n").await.unwrap();

            // C5: RCPT TO
            buf.clear();
            reader.read_line(&mut buf).await.unwrap();
            assert!(buf.contains("RCPT TO"));
            reader.get_mut().write_all(b"250 OK\r\n").await.unwrap();

            // C6: DATA
            buf.clear();
            reader.read_line(&mut buf).await.unwrap();
            assert!(buf.contains("DATA"));
            reader.get_mut().write_all(b"354 Start input\r\n").await.unwrap();

            // C7: Body + dot
            loop {
                buf.clear();
                reader.read_line(&mut buf).await.unwrap();
                if buf == ".\r\n" {
                    break;
                }
            }
            reader.get_mut().write_all(b"250 OK Queued\r\n").await.unwrap();

            // C8: QUIT
            buf.clear();
            reader.read_line(&mut buf).await.unwrap();
            assert!(buf.contains("QUIT"));
            reader.get_mut().write_all(b"221 Bye\r\n").await.unwrap();
        });

        // 2. Setup Client configuration
        let relay_config = crate::outbound::relay::RelayConfig {
            host: "127.0.0.1".to_string(),
            port,
            user: "apikey".to_string(),
            pass: "testpass".to_string(),
        };

        let mut root_store = RootCertStore::empty();
        for cert in rustls_native_certs::load_native_certs().unwrap() {
            root_store.add(cert).ok();
        }
        let client_config = Arc::new(
            ClientConfig::builder()
                .with_root_certificates(root_store)
                .with_no_client_auth()
        );

        // 3. Invoke relay sender
        let res = crate::outbound::relay::send_via_relay(
            &client_config,
            "example.com",
            &relay_config,
            "receiver@domain.com",
            "sender@example.com",
            b"Subject: Hello Relay\r\n\r\nThis is the body.",
        ).await;

        // 4. Assert client exited successfully
        assert!(res.is_ok(), "Relay protocol sender failed: {:?}", res.err());
    }
}
