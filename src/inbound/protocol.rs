use crate::db::{
    ReplyMappingLookup, get_or_create_reply_mapping, get_reply_mapping_by_token, insert_email,
};
use crate::fs::write_file_sync_with_permissions;
use crate::inbound::acceptor::HotReloadAcceptor;
use crate::outbound::OutboundService;
use crate::outbound::mime::prepare_reply_for_relay;
use pin_project_lite::pin_project;

use std::pin::Pin;
use std::sync::Arc;
use std::task::{Context, Poll};
use tokio::io::{AsyncBufReadExt, AsyncRead, AsyncWrite, AsyncWriteExt, BufReader, ReadBuf};
use tokio::net::TcpStream;

pin_project! {
    #[project = AnyStreamProj]
    enum AnyStream {
        Tcp { #[pin] stream: TcpStream },
        Tls { #[pin] stream: tokio_rustls::server::TlsStream<TcpStream> },
        Empty,
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
            AnyStreamProj::Empty => Poll::Ready(Err(std::io::Error::new(
                std::io::ErrorKind::BrokenPipe,
                "Stream is empty",
            ))),
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
            AnyStreamProj::Empty => Poll::Ready(Err(std::io::Error::new(
                std::io::ErrorKind::BrokenPipe,
                "Stream is empty",
            ))),
        }
    }

    fn poll_flush(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<std::io::Result<()>> {
        match self.project() {
            AnyStreamProj::Tcp { stream } => stream.poll_flush(cx),
            AnyStreamProj::Tls { stream } => stream.poll_flush(cx),
            AnyStreamProj::Empty => Poll::Ready(Err(std::io::Error::new(
                std::io::ErrorKind::BrokenPipe,
                "Stream is empty",
            ))),
        }
    }

    fn poll_shutdown(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<std::io::Result<()>> {
        match self.project() {
            AnyStreamProj::Tcp { stream } => stream.poll_shutdown(cx),
            AnyStreamProj::Tls { stream } => stream.poll_shutdown(cx),
            AnyStreamProj::Empty => Poll::Ready(Err(std::io::Error::new(
                std::io::ErrorKind::BrokenPipe,
                "Stream is empty",
            ))),
        }
    }
}

enum SmtpState {
    /// Waiting for HELO/EHLO
    Greet,
    /// Waiting for MAIL FROM
    MailFrom,
    /// Waiting for RCPT TO (This is where you check the subdomain!)
    RcptTo,
    /// Waiting for the DATA command
    Data,
    /// Actually receiving the email bytes
    ReadingBody,
}

pub struct EmailMetadata {
    pub sender: String,
    pub subject: String,
    pub message_id: Option<String>,
    pub in_reply_to: Option<String>,
    pub references: Vec<String>,
}

pub struct SmartBodyBuffer {
    state: BodyBufferState,
    max_memory: usize,
    max_total_size: usize,
    bytes_written: usize,
}

enum BodyBufferState {
    Memory(Vec<u8>),
    Disk {
        temp_file: tempfile::NamedTempFile,
        writer: std::io::BufWriter<std::fs::File>,
    },
}

impl SmartBodyBuffer {
    pub fn new(max_memory: usize) -> Self {
        Self {
            state: BodyBufferState::Memory(Vec::new()),
            max_memory,
            max_total_size: 25 * 1024 * 1024, // 25 MB
            bytes_written: 0,
        }
    }

    pub fn clear(&mut self) {
        self.state = BodyBufferState::Memory(Vec::new());
        self.bytes_written = 0;
    }

    pub fn append(&mut self, line_with_newline: &str) -> Result<(), std::io::Error> {
        let content = if line_with_newline.starts_with('.') {
            // Dot un-stuffing: remove the first dot
            &line_with_newline[1..]
        } else {
            line_with_newline
        };
        let content_bytes = content.as_bytes();

        if self.bytes_written + content_bytes.len() > self.max_total_size {
            return Err(std::io::Error::new(
                std::io::ErrorKind::OutOfMemory,
                "Message size exceeds maximum limit",
            ));
        }

        match &mut self.state {
            BodyBufferState::Memory(vec) => {
                if vec.len() + content_bytes.len() > self.max_memory {
                    // Upgrade to Disk-backed buffer
                    let temp_file = tempfile::NamedTempFile::new()?;
                    let file = temp_file.reopen()?;
                    let mut writer = std::io::BufWriter::new(file);

                    // Write buffered contents
                    std::io::Write::write_all(&mut writer, vec)?;

                    // Write new content
                    std::io::Write::write_all(&mut writer, content_bytes)?;

                    self.state = BodyBufferState::Disk { temp_file, writer };
                } else {
                    vec.extend_from_slice(content_bytes);
                }
            }
            BodyBufferState::Disk { writer, .. } => {
                std::io::Write::write_all(writer, content_bytes)?;
            }
        }

        self.bytes_written += content_bytes.len();
        Ok(())
    }

    pub fn len(&self) -> usize {
        self.bytes_written
    }

    pub fn extract_metadata(&self, envelope_sender: &str) -> Result<EmailMetadata, std::io::Error> {
        match &self.state {
            BodyBufferState::Memory(vec) => Ok(SmtpSession::extract_metadata(vec, envelope_sender)),
            BodyBufferState::Disk { temp_file, .. } => {
                use std::io::Read;
                let file = temp_file.reopen()?;
                let mut handle = file.take(131_072); // Take first 128KB (more than enough for headers)
                let mut buf = Vec::new();
                handle.read_to_end(&mut buf)?;
                Ok(SmtpSession::extract_metadata(&buf, envelope_sender))
            }
        }
    }

    pub fn persist_to_path(&mut self, dest_path: &std::path::Path) -> Result<(), std::io::Error> {
        let old_state = std::mem::replace(&mut self.state, BodyBufferState::Memory(Vec::new()));
        match old_state {
            BodyBufferState::Memory(vec) => {
                write_file_sync_with_permissions(dest_path, &vec)?;
                self.state = BodyBufferState::Memory(vec);
                Ok(())
            }
            BodyBufferState::Disk {
                mut writer,
                temp_file,
            } => {
                std::io::Write::flush(&mut writer)?;
                temp_file.persist(dest_path).map_err(|e| {
                    std::io::Error::new(
                        std::io::ErrorKind::Other,
                        format!("Failed to persist temp file to final destination: {}", e),
                    )
                })?;
                Ok(())
            }
        }
    }

    pub fn get_content_bytes(&mut self) -> Result<Vec<u8>, std::io::Error> {
        match &mut self.state {
            BodyBufferState::Memory(vec) => Ok(vec.clone()),
            BodyBufferState::Disk { writer, temp_file } => {
                std::io::Write::flush(writer)?;
                use std::io::Read;
                let mut file = temp_file.reopen()?;
                let mut buf = Vec::new();
                file.read_to_end(&mut buf)?;
                Ok(buf)
            }
        }
    }
}

pub struct SmtpSession {
    //stream: BufReader<TcpStream>,
    stream: BufReader<AnyStream>,
    state: SmtpState,
    sender: Option<String>,
    recipient: Option<String>,
    alias_id: Option<uuid::Uuid>,
    destination_email: Option<String>,
    auto_forward: bool,
    reply_mapping: Option<ReplyMappingLookup>,
    tls_acceptor: Option<HotReloadAcceptor>,
    db_pool: crate::db::DbPool,
    body_buffer: SmartBodyBuffer,
    storage_dir: std::path::PathBuf,
    outbound: Arc<OutboundService>,
    tx: tokio::sync::broadcast::Sender<crate::web::DashboardEvent>,
    peer_ip: std::net::IpAddr,
    rate_limiter: Arc<crate::inbound::rate_limit::RateLimiter>,
    blocklist: Arc<crate::inbound::blocklist::Blocklist>,
    limits: crate::inbound::rate_limit::InboundLimits,
}

impl SmtpSession {
    fn extract_recipient(cmd: &str) -> Option<String> {
        let cmd_lower = cmd.to_lowercase();
        cmd_lower
            .split_once("to:")
            .map(|(_, addr)| {
                addr.trim()
                    .trim_matches(|c| c == '<' || c == '>')
                    .to_string()
            })
            .filter(|s| !s.is_empty())
    }

    fn extract_sender(cmd: &str) -> Option<String> {
        let cmd_lower = cmd.to_lowercase();
        cmd_lower.split_once("from:").map(|(_, addr)| {
            addr.trim()
                .trim_matches(|c| c == '<' || c == '>')
                .to_string()
        })
    }

    /// Extracts metadata from the email body for threading and display.
    fn extract_metadata(body: &[u8], envelope_sender: &str) -> EmailMetadata {
        let message = mail_parser::MessageParser::default().parse(body);

        let subject = message
            .as_ref()
            .and_then(|m| m.subject().map(|s| s.to_string()))
            .unwrap_or_else(|| "No Subject".to_string());

        let friendly_sender = message
            .as_ref()
            .and_then(|m| m.from())
            .and_then(|f| f.first())
            .and_then(|a| a.address())
            .map(|s| s.to_string());

        let sender = friendly_sender.unwrap_or_else(|| envelope_sender.to_string());

        let message_id = message
            .as_ref()
            .and_then(|m| m.message_id())
            .map(|id| crate::outbound::mime::format_message_id(id));

        let in_reply_to = message
            .as_ref()
            .and_then(|m| m.in_reply_to().as_text())
            .map(|id| crate::outbound::mime::format_message_id(id));

        let references = message
            .as_ref()
            .and_then(|m| m.references().as_text())
            .map(|r| {
                r.split_whitespace()
                    .map(|id| crate::outbound::mime::format_message_id(id))
                    .collect()
            })
            .unwrap_or_default();

        EmailMetadata {
            sender,
            subject,
            message_id,
            in_reply_to,
            references,
        }
    }

    pub fn new(
        socket: TcpStream,
        tls_acceptor: Option<HotReloadAcceptor>,
        db_pool: crate::db::DbPool,
        storage_dir: std::path::PathBuf,
        outbound: Arc<OutboundService>,
        tx: tokio::sync::broadcast::Sender<crate::web::DashboardEvent>,
        peer_ip: std::net::IpAddr,
        rate_limiter: Arc<crate::inbound::rate_limit::RateLimiter>,
        blocklist: Arc<crate::inbound::blocklist::Blocklist>,
        limits: crate::inbound::rate_limit::InboundLimits,
    ) -> Self {
        Self {
            stream: BufReader::new(AnyStream::Tcp { stream: socket }),
            state: SmtpState::Greet,
            sender: None,
            recipient: None,
            alias_id: None,
            destination_email: None,
            auto_forward: false,
            reply_mapping: None,
            tls_acceptor,
            db_pool,
            body_buffer: SmartBodyBuffer::new(65_536),
            storage_dir,
            outbound,
            tx,
            peer_ip,
            rate_limiter,
            blocklist,
            limits,
        }
    }
    async fn write_line(&mut self, msg: &str) -> Result<(), std::io::Error> {
        let out = format!("{}\r\n", msg);
        self.stream.get_mut().write_all(out.as_bytes()).await?;
        Ok(())
    }
    async fn handle_greet(&mut self, cmd: &str) -> Result<(), std::io::Error> {
        if cmd.to_uppercase().starts_with("HELO") || cmd.to_uppercase().starts_with("EHLO") {
            self.write_line("250 Hello").await?;
            self.state = SmtpState::MailFrom;
        } else {
            self.write_line("500 Syntax error, command unrecognized")
                .await?;
        }
        Ok(())
    }

    async fn handle_rcpt_to(&mut self, cmd: &str) -> Result<(), std::io::Error> {
        match Self::extract_recipient(cmd) {
            None => {
                self.write_line("501 Syntax error in address").await?;
            }
            Some(addr) => {
                self.recipient = Some(addr.clone());
                let parts: Vec<&str> = addr.split('@').collect();
                if parts.len() != 2 {
                    self.write_line("501 Syntax error in address").await?;
                    return Ok(());
                }

                let local_part = parts[0];
                let full_domain = parts[1];

                if local_part.starts_with("reply-") {
                    let mapping = match get_reply_mapping_by_token(&self.db_pool, local_part).await
                    {
                        Ok(Some(m)) => m,
                        Ok(None) => {
                            tracing::warn!("Unknown or inactive reply token: {}", local_part);
                            self.write_line("550 User Not Found").await?;
                            return Ok(());
                        }
                        Err(e) => {
                            tracing::error!("Database error during reply token lookup: {}", e);
                            self.write_line("451 Requested action aborted").await?;
                            return Ok(());
                        }
                    };

                    let sender_addr = match &self.sender {
                        Some(addr) => addr,
                        None => {
                            self.write_line("503 Bad sequence of commands (MAIL FROM first)")
                                .await?;
                            return Ok(());
                        }
                    };

                    if sender_addr.to_lowercase() != mapping.destination_email.to_lowercase() {
                        tracing::warn!(
                            "Reply token matched but sender {} does not match authorized destination {}",
                            sender_addr,
                            mapping.destination_email
                        );
                        self.write_line("550 Sender Denied").await?;
                        return Ok(());
                    }

                    // Run SPF check to verify the sender is not spoofing mapping.destination_email
                    let sender_parts: Vec<&str> = sender_addr.split('@').collect();
                    if sender_parts.len() == 2 {
                        let sender_domain = sender_parts[1];
                        let is_spf_valid =
                            self.outbound.check_spf(sender_domain, self.peer_ip).await;
                        if !is_spf_valid {
                            tracing::warn!(
                                "SPF Validation failed for {} from IP {} claiming to be {}",
                                sender_domain,
                                self.peer_ip,
                                sender_addr
                            );
                            self.write_line("550 SPF Validation Failed").await?;
                            return Ok(());
                        }
                    }

                    self.reply_mapping = Some(mapping);
                    self.state = SmtpState::Data;
                    self.write_line("250 2.1.5 Destination address accepted")
                        .await?;
                    return Ok(());
                }

                let decoded_srs = if local_part.to_lowercase().starts_with("srs0+") {
                    match self.outbound.decode_srs(&addr) {
                        Some(original_sender) => Some(original_sender),
                        None => {
                            tracing::warn!(
                                "Incoming SRS signature validation failed for recipient: {}",
                                addr
                            );
                            self.write_line("550 User Not Found").await?;
                            return Ok(());
                        }
                    }
                } else {
                    None
                };

                let (resolved_local, resolved_domain) =
                    if let Some(ref original_sender) = decoded_srs {
                        if let Some(parts) = original_sender.split_once('@') {
                            parts
                        } else {
                            self.write_line("550 User Not Found").await?;
                            return Ok(());
                        }
                    } else {
                        (local_part, full_domain)
                    };

                match crate::db::resolve_recipient_alias(
                    &self.db_pool,
                    resolved_local,
                    resolved_domain,
                )
                .await
                {
                    Ok(Some(row)) => {
                        self.alias_id = Some(row.id);
                        self.destination_email = Some(row.destination_email);
                        self.auto_forward = row.auto_forward;
                        self.state = SmtpState::Data;
                        self.write_line("250 2.1.5 Destination address accepted")
                            .await?;
                    }
                    Ok(None) => {
                        tracing::warn!(
                            "No alias found for recipient: {} from IP: {}",
                            addr,
                            self.peer_ip
                        );

                        let failures = self.rate_limiter.record_failure(self.peer_ip);

                        if failures >= self.limits.block_threshold {
                            tracing::warn!(
                                "IP {} reached block threshold ({}). Adding to blocklist and rejecting.",
                                self.peer_ip,
                                failures
                            );

                            let _ = self.blocklist.add_ip(self.peer_ip);

                            self.write_line("554 5.7.1 Connection refused - IP address blocked")
                                .await?;
                            return Err(std::io::Error::new(
                                std::io::ErrorKind::ConnectionAborted,
                                "IP blocked due to excessive failures",
                            ));
                        } else if failures >= self.limits.tarpit_threshold {
                            tracing::warn!(
                                "IP {} exceeded tarpit threshold ({}). Tarpitting and rejecting.",
                                self.peer_ip,
                                failures
                            );

                            // Tarpit the connection using the parameterized duration
                            tokio::time::sleep(std::time::Duration::from_secs(
                                self.limits.tarpit_duration_secs,
                            ))
                            .await;

                            self.write_line("451 4.7.1 Please try again later").await?;
                        } else {
                            self.write_line("550 User Not Found").await?;
                        }
                    }
                    Err(e) => {
                        tracing::error!("Database error during recipient resolution: {}", e);
                        self.write_line("451 Requested action aborted").await?;
                    }
                }
            }
        }
        Ok(())
    }

    async fn handle_mail_from(&mut self, cmd: &str) -> Result<(), std::io::Error> {
        if let Some(addr) = Self::extract_sender(cmd) {
            self.sender = Some(addr);
            self.write_line("250 OK").await?;
            self.state = SmtpState::RcptTo;
        } else {
            self.write_line("501 Syntax error").await?;
        }
        Ok(())
    }

    async fn handle_data_command(&mut self, cmd: &str) -> Result<(), std::io::Error> {
        if cmd.to_uppercase() == "DATA" {
            self.write_line("354 End data with <CR><LF>.<CR><LF>")
                .await?;
            self.state = SmtpState::ReadingBody;
        } else {
            self.write_line("500 Error").await?;
        }
        Ok(())
    }
    async fn handle_body_line(&mut self, line_with_newline: &str) -> Result<(), std::io::Error> {
        let trimmed = line_with_newline.trim_end_matches(['\r', '\n']);
        if trimmed == "." {
            // Check if this is a reply relay session
            if let Some(ref mapping) = self.reply_mapping {
                let alias_address = format!("{}@{}", mapping.alias_subdomain, mapping.domain_name);

                // 1. Prepare and sanitize the body for relaying using our modular helper
                let content_bytes = self.body_buffer.get_content_bytes()?;
                let clean_body = prepare_reply_for_relay(
                    &content_bytes,
                    &alias_address,
                    &mapping.original_sender,
                );

                // 2. Transmit the email outbound to the original sender
                let outbound = self.outbound.clone();
                let original_sender = mapping.original_sender.clone();
                let alias_address_clone = alias_address.clone();

                tokio::spawn(async move {
                    match outbound
                        .send_firsthand(
                            &original_sender,
                            &alias_address_clone,
                            clean_body.as_bytes(),
                        )
                        .await
                    {
                        Ok(_) => {
                            tracing::info!(
                                "Successfully relayed reply from {} to {}",
                                alias_address_clone,
                                original_sender
                            );
                        }
                        Err(e) => {
                            tracing::error!(
                                "Failed to relay reply from {} to {}: {}",
                                alias_address_clone,
                                original_sender,
                                e
                            );
                        }
                    }
                });

                self.write_line("250 OK: reply relayed").await?;
                self.state = SmtpState::Greet; // Reset for next email
                self.body_buffer.clear();
                return Ok(());
            }

            // 1. Extract metadata and determine correct display sender
            let envelope_sender = self.sender.as_deref().unwrap_or_default();
            let metadata = self.body_buffer.extract_metadata(envelope_sender)?;

            let body_key = uuid::Uuid::new_v4();
            let path = self.storage_dir.join(format!("{}.eml", body_key));

            // 2. Persist to disk
            self.body_buffer.persist_to_path(&path)?;
            let alias_id = self.alias_id.expect("Alias ID unset");

            // 3. Thread detection
            let mut refs = metadata.references.clone();
            if let Some(ref in_reply_to) = metadata.in_reply_to {
                if !refs.contains(in_reply_to) {
                    refs.push(in_reply_to.clone());
                }
            }

            let thread_id = crate::db::find_thread_id_by_references(&self.db_pool, &refs)
                .await
                .ok()
                .flatten();

            tracing::info!("Email from sender: {}", &metadata.sender);
            match insert_email(
                &self.db_pool,
                alias_id,
                &metadata.sender,
                &metadata.subject,
                body_key,
                None,
                metadata.message_id,
                thread_id,
            )
            .await
            {
                Ok(email) => {
                    // Send notification to SSE subscribers
                    let _ = self.tx.send(crate::web::DashboardEvent::NewEmail {
                        user_id: email.user_id,
                        email_id: email.id,
                    });

                    // Trigger Forwarding if enabled
                    if self.auto_forward
                        && let Some(dest) = &self.destination_email
                    {
                        let outbound = self.outbound.clone();
                        let pool = self.db_pool.clone();
                        let tx_clone = self.tx.clone();
                        let email_id = email.id;
                        let user_id = email.user_id;
                        let dest = dest.clone();
                        let sender = metadata.sender.clone();
                        let body = self.body_buffer.get_content_bytes()?;
                        tokio::spawn(async move {
                            let res = match get_or_create_reply_mapping(&pool, alias_id, &sender)
                                .await
                            {
                                Ok(mapping) => {
                                    outbound
                                        .send_forward_with_reply_token(
                                            &dest,
                                            &sender,
                                            &mapping.anonymous_token,
                                            &body,
                                        )
                                        .await
                                }
                                Err(e) => {
                                    tracing::error!(
                                        "Failed to get or create reply mapping during forwarding, falling back: {}",
                                        e
                                    );
                                    outbound.send_forward(&dest, &sender, &body).await
                                }
                            };

                            match res {
                                Ok(_) => {
                                    if let Err(e) =
                                        crate::db::mark_email_as_forwarded(&pool, email_id).await
                                    {
                                        tracing::error!("Failed to mark email as forwarded: {}", e);
                                    } else {
                                        // Notify dashboard about the status update
                                        let _ = tx_clone.send(
                                            crate::web::DashboardEvent::EmailForwarded {
                                                user_id,
                                                email_id,
                                            },
                                        );
                                    }
                                }
                                Err(e) => {
                                    tracing::error!("Failed to forward email to {}: {}", dest, e);
                                }
                            }
                        });
                    }
                    self.write_line(&format!("250 OK: queued as {}", body_key))
                        .await?;
                }
                Err(e) => {
                    tracing::error!("Failed to insert email: {}", e);
                    self.write_line("451 Requested action aborted").await?;
                }
            }
            self.state = SmtpState::Greet; // Reset for next email
            self.body_buffer.clear();
        } else {
            self.body_buffer.append(line_with_newline)?;
        }
        Ok(())
    }
    async fn upgrade_to_tls(&mut self) -> Result<(), Box<dyn std::error::Error>> {
        let acceptor = match &self.tls_acceptor {
            Some(a) => a.clone(), // Clone the Arc wrapper so we drop the borrow on self!
            None => {
                // If None was passed on boot, TLS is permanently disabled.
                self.write_line("502 Command not implemented").await?;
                return Ok(());
            }
        };

        // If the acceptor exists but the certificate hasn't been written to disk yet
        if acceptor.config().is_none() {
            self.write_line("454 TLS not available due to temporary reason")
                .await?;
            return Ok(());
        }

        self.write_line("220 Ready to start TLS").await?;
        // Safely swap the stream with Empty to take ownership of the TcpStream
        let any_stream_ref = self.stream.get_mut();
        let old_stream = std::mem::replace(any_stream_ref, AnyStream::Empty);

        if let AnyStream::Tcp { stream } = old_stream {
            match acceptor.accept(stream).await {
                Ok(tls_stream) => {
                    // Replace the Empty variant with the new TLS stream
                    *any_stream_ref = AnyStream::Tls { stream: tls_stream };
                    self.state = SmtpState::Greet;
                }
                Err(e) => {
                    tracing::error!("TLS handshake failed: {}", e);
                    return Err(e.into());
                }
            }
        } else {
            return Err("Cannot upgrade: stream is not in a plain TCP state".into());
        }
        Ok(())
    }
    async fn read_line_with_timeout(&mut self, buf: &mut String) -> Result<usize, std::io::Error> {
        #[cfg(test)]
        let timeout_duration = std::time::Duration::from_millis(100);
        #[cfg(not(test))]
        let timeout_duration = std::time::Duration::from_secs(30); // 30-second idle limit

        match tokio::time::timeout(timeout_duration, self.stream.read_line(buf)).await {
            Ok(read_result) => read_result, // Read completed, return standard result
            Err(_) => {
                // Timeout triggered! Write standard SMTP timeout disconnect code
                tracing::warn!(
                    "SMTP client {} idle timeout exceeded, disconnecting",
                    self.peer_ip
                );
                let _ = self
                    .write_line("421 4.4.2 Connection timeout. Closing connection.")
                    .await;

                Err(std::io::Error::new(
                    std::io::ErrorKind::TimedOut,
                    "Connection idle timeout exceeded",
                ))
            }
        }
    }

    pub async fn handle(&mut self) -> Result<(), Box<dyn std::error::Error>> {
        self.write_line("220 Welcome to my Email Server").await?;

        let mut line = String::new();
        loop {
            line.clear();
            let n = self.read_line_with_timeout(&mut line).await?;
            if n == 0 {
                break;
            }
            // NEW: If we are reading the body, don't trim and don't check for QUIT/STARTTLS
            if matches!(self.state, SmtpState::ReadingBody) {
                if let Err(e) = self.handle_body_line(&line).await {
                    if e.kind() == std::io::ErrorKind::OutOfMemory {
                        let _ = self
                            .write_line("552 5.3.4 Message size exceeds fixed maximum message size")
                            .await;
                    }
                    break;
                }
                continue;
            }
            line = line.trim().to_string();
            if line.to_uppercase() == "QUIT" {
                self.write_line("221 2.0.0 Service closing transmission channel")
                    .await?;
                break;
            }
            if line.to_uppercase() == "STARTTLS" {
                self.upgrade_to_tls().await?;
                // Start loop again
                continue;
            }
            //  This allows a client that gets "stuck" in a state to manually
            // reset the session to the beginning (MailFrom state).
            if line.to_uppercase() == "RSET" {
                self.sender = None;
                self.recipient = None;
                self.alias_id = None;
                self.body_buffer.clear();
                self.state = SmtpState::MailFrom;
                self.write_line("250 OK Resetting").await?;
                continue;
            }
            match self.state {
                SmtpState::Greet => self.handle_greet(&line).await?,
                SmtpState::MailFrom => self.handle_mail_from(&line).await?,
                SmtpState::RcptTo => self.handle_rcpt_to(&line).await?,
                SmtpState::Data => self.handle_data_command(&line).await?,
                SmtpState::ReadingBody => self.handle_body_line(&line).await?,
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::inbound::rate_limit::RateLimiter;
    use std::path::PathBuf;
    use tokio::io::AsyncWriteExt;

    #[test]
    fn test_recipient_extraction() {
        let cases = vec![
            ("RCPT TO:<john@domain.com>", Some("john@domain.com")),
            ("rcpt to: <JOHN@domain.com> ", Some("john@domain.com")),
            ("RCPT TO:alex@sub.net", Some("alex@sub.net")),
            ("RCPT TO:<>", None),
            ("INVALID", None),
        ];
        for (input, expected) in cases {
            assert_eq!(
                SmtpSession::extract_recipient(input),
                expected.map(String::from)
            );
        }
    }

    #[test]
    fn test_sender_extraction() {
        let cases = vec![
            ("MAIL FROM:<meg@gmail.com>", Some("meg@gmail.com")),
            ("mail from: <MEG@GMAIL.COM>", Some("meg@gmail.com")),
            ("MAIL FROM:<>", Some("")), // Bounce address
            ("HELO", None),
        ];
        for (input, expected) in cases {
            assert_eq!(
                SmtpSession::extract_sender(input),
                expected.map(String::from)
            );
        }
    }

    #[test]
    fn test_dot_unstuffing_logic() {
        let mut buffer = SmartBodyBuffer::new(65_536);

        // 1. Normal line is added without dropping anything
        buffer.append("Hello World\r\n").unwrap();

        // 2. Stuffed line '..' becomes '.'
        buffer.append("..This stands for dot\r\n").unwrap();

        // 3. Stuffed line '...' becomes '..'
        buffer.append("...\r\n").unwrap();

        let output = String::from_utf8(buffer.get_content_bytes().unwrap()).unwrap();
        assert_eq!(output, "Hello World\r\n.This stands for dot\r\n..\r\n");
    }

    #[test]
    fn test_oom_payload_limit() {
        let mut buffer = SmartBodyBuffer::new(65_536);
        // A dummy payload of 1MB
        let chunk = "A".repeat(1024 * 1024);

        // Append 25 MB safely
        for _ in 0..25 {
            let res = buffer.append(&chunk);
            assert!(res.is_ok(), "Failed to allocate 25MB but should have");
        }

        // Try to append 1 more byte, it should cross the threshold and fail
        let res = buffer.append("A");
        assert!(res.is_err(), "Should have failed with OOM Error");
        let err = res.unwrap_err();
        assert_eq!(err.kind(), std::io::ErrorKind::OutOfMemory);
    }

    #[test]
    fn test_metadata_extraction_standard() {
        let body = b"From: alice@example.com\r\nSubject: Hello\r\n\r\nBody content";
        let metadata = SmtpSession::extract_metadata(body, "bounce@example.com");
        assert_eq!(metadata.sender, "alice@example.com");
        assert_eq!(metadata.subject, "Hello");
    }

    #[test]
    fn test_metadata_extraction_verp_bounce() {
        let body =
            b"From: Greenhouse <no-reply@greenhouse.io>\r\nSubject: Job Alert\r\n\r\nContent";
        let envelope = "bounce+7bf24a.eb6b8a-hello=example.com@us.greenhouse-mail.io";
        let metadata = SmtpSession::extract_metadata(body, envelope);

        // Should favor the internal From header
        assert_eq!(metadata.sender, "no-reply@greenhouse.io");
        assert_eq!(metadata.subject, "Job Alert");
    }

    #[test]
    fn test_metadata_extraction_fallback() {
        let body = b"Subject: Missing From Header\r\n\r\nOnly subject here";
        let envelope = "real-sender@domain.com";
        let metadata = SmtpSession::extract_metadata(body, envelope);

        assert_eq!(metadata.sender, "real-sender@domain.com");
        assert_eq!(metadata.subject, "Missing From Header");
    }

    #[test]
    fn test_metadata_extraction_no_subject() {
        let body = b"From: someone@somewhere.com\r\n\r\nNo subject line";
        let metadata = SmtpSession::extract_metadata(body, "envelope@test.com");

        assert_eq!(metadata.sender, "someone@somewhere.com");
        assert_eq!(metadata.subject, "No Subject");
    }

    #[test]
    fn test_smart_buffer_ram_only() {
        let mut buffer = SmartBodyBuffer::new(1024); // 1KB threshold
        assert!(matches!(buffer.state, BodyBufferState::Memory(_)));

        buffer
            .append("Subject: Small Email\r\n\r\nThis stays in RAM!")
            .unwrap();
        assert!(matches!(buffer.state, BodyBufferState::Memory(_)));

        let metadata = buffer.extract_metadata("sender@test.com").unwrap();
        assert_eq!(metadata.subject, "Small Email");

        let content = buffer.get_content_bytes().unwrap();
        assert_eq!(content, b"Subject: Small Email\r\n\r\nThis stays in RAM!");
    }

    #[test]
    fn test_smart_buffer_disk_promotion() {
        let mut buffer = SmartBodyBuffer::new(50); // very small threshold: 50 bytes
        assert!(matches!(buffer.state, BodyBufferState::Memory(_)));

        // Append 30 bytes (stays in memory)
        buffer.append("Subject: Test\r\n\r\n").unwrap();
        assert!(matches!(buffer.state, BodyBufferState::Memory(_)));

        // Append 40 more bytes (total 70, triggers promotion to disk!)
        buffer
            .append("This is some body text that will go to disk because it is larger.")
            .unwrap();
        assert!(matches!(buffer.state, BodyBufferState::Disk { .. }));

        // Persist to temporary final path
        let temp_dir = tempfile::tempdir().unwrap();
        let dest = temp_dir.path().join("saved_email.eml");
        buffer.persist_to_path(&dest).unwrap();

        // Check data integrity
        let saved_content = std::fs::read(&dest).unwrap();
        assert!(saved_content.starts_with(b"Subject: Test\r\n\r\nThis is some body text"));
    }

    #[test]
    fn test_smart_buffer_partial_header_extract() {
        let mut buffer = SmartBodyBuffer::new(10); // force disk immediately
        buffer
            .append("From: external@sender.com\r\nSubject: Header Test\r\n\r\n")
            .unwrap();

        // Append a massive amount of body payload to test that header parsing remains lightweight
        let large_payload = "A".repeat(200 * 1024); // 200KB payload
        buffer.append(&large_payload).unwrap();

        assert!(matches!(buffer.state, BodyBufferState::Disk { .. }));

        // Extract metadata (should read only the top part from disk, not the full 200KB body)
        let metadata = buffer.extract_metadata("envelope@sender.com").unwrap();
        assert_eq!(metadata.sender, "external@sender.com");
        assert_eq!(metadata.subject, "Header Test");
    }

    #[test]
    fn test_smart_buffer_auto_cleanup() {
        let temp_file_path;
        {
            let mut buffer = SmartBodyBuffer::new(10); // force disk
            buffer
                .append("From: trigger@cleanup.com\r\n\r\nPayload")
                .unwrap();

            if let BodyBufferState::Disk { temp_file, .. } = &buffer.state {
                temp_file_path = temp_file.path().to_path_buf();
                assert!(temp_file_path.exists());
            } else {
                panic!("Should be on disk!");
            }
            // buffer goes out of scope here and is dropped!
        }

        // Verify that the temporary file was automatically and completely deleted from disk!
        assert!(!temp_file_path.exists());
    }

    #[tokio::test]
    async fn test_read_line_success() {
        let _ = tokio_rustls::rustls::crypto::aws_lc_rs::default_provider().install_default();

        // Setup real loopback listener
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();

        // Spawn client to write a line
        tokio::spawn(async move {
            let mut client = tokio::net::TcpStream::connect(addr).await.unwrap();
            client.write_all(b"HELO localhost\r\n").await.unwrap();
        });

        let (server_stream, _) = listener.accept().await.unwrap();

        // Setup mock SMTP session
        let (tx, _) = tokio::sync::broadcast::channel(10);
        let rate_limiter = Arc::new(RateLimiter::new());
        let db =
            crate::db::DbPool::Sqlite(sqlx::SqlitePool::connect("sqlite::memory:").await.unwrap());
        let temp_dir = tempfile::tempdir().unwrap();
        let tls_acceptor = HotReloadAcceptor::new(
            PathBuf::from("certs/smtp_cert.pem"),
            PathBuf::from("certs/smtp_key.pem"),
            std::time::Duration::from_millis(100),
        )
        .unwrap();
        let outbound = Arc::new(OutboundService::new(
            "secret".to_string(),
            hickory_resolver::TokioResolver::builder_tokio()
                .unwrap()
                .build()
                .unwrap(),
            "example.com".to_string(),
            db.clone(),
            temp_dir.path().to_path_buf(),
        ));

        let blocklist_path = temp_dir.path().join("blockips.conf");
        let blocklist = Arc::new(crate::inbound::blocklist::Blocklist::new(blocklist_path));
        let limits = crate::inbound::rate_limit::InboundLimits {
            tarpit_threshold: 5,
            block_threshold: 10,
            tarpit_duration_secs: 5,
        };

        let mut session = SmtpSession::new(
            server_stream,
            Some(tls_acceptor),
            db,
            temp_dir.path().to_path_buf(),
            outbound,
            tx,
            "127.0.0.1".parse().unwrap(),
            rate_limiter,
            blocklist,
            limits,
        );

        let mut buf = String::new();
        let n = session.read_line_with_timeout(&mut buf).await.unwrap();

        assert_eq!(n, 16);
        assert_eq!(buf, "HELO localhost\r\n");
    }

    #[tokio::test]
    async fn test_read_line_timeout_trigger() {
        let _ = tokio_rustls::rustls::crypto::aws_lc_rs::default_provider().install_default();

        // Setup real loopback listener
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();

        // Spawn client that connects but sends absolutely nothing (slowloris)
        tokio::spawn(async move {
            let _client = tokio::net::TcpStream::connect(addr).await.unwrap();
            // Wait indefinitely
            tokio::time::sleep(std::time::Duration::from_secs(10)).await;
        });

        let (server_stream, _) = listener.accept().await.unwrap();

        // Setup mock SMTP session
        let (tx, _) = tokio::sync::broadcast::channel(10);
        let rate_limiter = Arc::new(RateLimiter::new());
        let db =
            crate::db::DbPool::Sqlite(sqlx::SqlitePool::connect("sqlite::memory:").await.unwrap());
        let temp_dir = tempfile::tempdir().unwrap();
        let tls_acceptor = HotReloadAcceptor::new(
            PathBuf::from("certs/smtp_cert.pem"),
            PathBuf::from("certs/smtp_key.pem"),
            std::time::Duration::from_millis(100),
        )
        .unwrap();
        let outbound = Arc::new(OutboundService::new(
            "secret".to_string(),
            hickory_resolver::TokioResolver::builder_tokio()
                .unwrap()
                .build()
                .unwrap(),
            "example.com".to_string(),
            db.clone(),
            temp_dir.path().to_path_buf(),
        ));

        let blocklist_path = temp_dir.path().join("blockips.conf");
        let blocklist = Arc::new(crate::inbound::blocklist::Blocklist::new(blocklist_path));
        let limits = crate::inbound::rate_limit::InboundLimits {
            tarpit_threshold: 5,
            block_threshold: 10,
            tarpit_duration_secs: 5,
        };

        let mut session = SmtpSession::new(
            server_stream,
            Some(tls_acceptor),
            db,
            temp_dir.path().to_path_buf(),
            outbound,
            tx,
            "127.0.0.1".parse().unwrap(),
            rate_limiter,
            blocklist,
            limits,
        );

        let mut buf = String::new();

        // Await read_line. It will time out after 100 milliseconds
        let result = session.read_line_with_timeout(&mut buf).await;

        // 1. Assert that the helper returned a TimedOut error
        assert!(result.is_err());
        assert_eq!(result.unwrap_err().kind(), std::io::ErrorKind::TimedOut);
    }
}
