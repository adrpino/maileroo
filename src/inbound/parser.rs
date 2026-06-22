use mail_parser::{MessageParser, MimeHeaders, PartType};

pub struct EmailMetadata {
    pub sender: String,
    pub subject: String,
    pub message_id: Option<String>,
    pub in_reply_to: Option<String>,
    pub references: Vec<String>,
}

pub struct AttachmentMetadata {
    pub filename: Option<String>,
    pub content_type: Option<String>,
    pub size_bytes: i64,          // decoded byte length
    pub part_index: i32,
    pub is_inline: bool,
    pub content_id: Option<String>,  // Content-ID without the <> wrapper
}

/// Extracts metadata and attachment metadata from the email body for threading, display, and storage.
pub fn extract_full_metadata(body: &[u8], envelope_sender: &str) -> (EmailMetadata, Vec<AttachmentMetadata>) {
    let message = MessageParser::default().parse(body);

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

    let email_metadata = EmailMetadata {
        sender,
        subject,
        message_id,
        in_reply_to,
        references,
    };

    let mut attachments = Vec::new();
    if let Some(msg) = message.as_ref() {
        for (index, part) in msg.attachments().enumerate() {
            let is_inline = matches!(part.body, PartType::InlineBinary(_))
                || part.content_disposition().is_some_and(|d| d.is_inline())
                || part.content_id().is_some();
            
            let content_id = part.content_id().map(|id| {
                let s = id.trim();
                s.strip_prefix('<').and_then(|s| s.strip_suffix('>')).unwrap_or(s).to_string()
            });

            let size_bytes = part.contents().len() as i64;
            
            attachments.push(AttachmentMetadata {
                filename: part.attachment_name().map(|s| s.to_string()),
                content_type: part.content_type().map(|c| {
                    if let Some(subtype) = c.subtype() {
                        format!("{}/{}", c.ctype(), subtype)
                    } else {
                        c.ctype().to_string()
                    }
                }),
                size_bytes,
                part_index: index as i32,
                is_inline,
                content_id,
            });
        }
    }

    (email_metadata, attachments)
}
