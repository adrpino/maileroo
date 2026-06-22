use time::OffsetDateTime;
use time::format_description::well_known::Rfc2822;
use uuid::Uuid;

/// Represents a single MIME attachment.
#[derive(Debug, Clone)]
pub struct Attachment {
    pub filename: Option<String>,
    pub content_type: String,
    pub data: Vec<u8>,
    pub is_inline: bool,
    pub content_id: Option<String>,
}

/// Represents the contents needed to build a MIME email.
#[derive(Debug, Clone)]
pub struct MimeEmail {
    pub from: String,
    pub to: String,
    pub subject: String,
    pub text_body: String,
    pub html_body: Option<String>,
    pub message_id: Option<String>,
    pub in_reply_to: Option<String>,
    pub references: Option<String>,
    pub attachments: Vec<Attachment>,
}

/// Helper to sanitize header values to prevent CRLF injection.
/// Removes any \r or \n characters so an attacker cannot inject new headers.
pub fn sanitize_header(value: &str) -> String {
    value.replace('\r', "").replace('\n', "")
}

/// Generates a globally unique Message-ID based on the sending domain.
pub fn generate_message_id(domain: &str) -> String {
    format!("<{}@{}>", Uuid::new_v4(), sanitize_header(domain))
}

/// Helper to ensure Message-IDs are correctly wrapped in angle brackets.
pub fn format_message_id(id: &str) -> String {
    let id = id.trim();
    if id.is_empty() {
        return String::new();
    }
    if id.starts_with('<') && id.ends_with('>') {
        id.to_string()
    } else {
        format!("<{}>", id)
    }
}

/// Builds a raw MIME string from the provided MimeEmail struct.
/// If `html_body` is provided, it generates a `multipart/alternative` email.
/// Otherwise, it generates a simple `text/plain` email.
pub fn build_mime(email: &MimeEmail) -> String {
    let mut mime = String::new();
    let now = OffsetDateTime::now_utc()
        .format(&Rfc2822)
        .unwrap_or_default();

    // 1. Standard Headers (Sanitized to prevent CRLF injection)
    mime.push_str(&format!("From: {}\r\n", sanitize_header(&email.from)));
    mime.push_str(&format!("To: {}\r\n", sanitize_header(&email.to)));

    let subject = if email.in_reply_to.is_some() && !email.subject.to_lowercase().starts_with("re:")
    {
        format!("Re: {}", email.subject)
    } else {
        email.subject.clone()
    };
    mime.push_str(&format!("Subject: {}\r\n", sanitize_header(&subject)));
    mime.push_str(&format!("Date: {}\r\n", now));

    // 2. Message-ID
    let message_id = email.message_id.clone().unwrap_or_else(|| {
        let domain = email.from.split('@').nth(1).unwrap_or("localhost");
        generate_message_id(domain)
    });
    mime.push_str(&format!(
        "Message-ID: {}\r\n",
        sanitize_header(&format_message_id(&message_id))
    ));

    // 3. Threading Headers
    if let Some(ref irt) = email.in_reply_to {
        mime.push_str(&format!(
            "In-Reply-To: {}\r\n",
            sanitize_header(&format_message_id(irt))
        ));
    }
    if let Some(ref refs) = email.references {
        mime.push_str(&format!(
            "References: {}\r\n",
            sanitize_header(&format_message_id(refs))
        ));
    }

    // 4. Content and Body
    mime.push_str("MIME-Version: 1.0\r\n");

    let has_real_attachments = email.attachments.iter().any(|a| !a.is_inline);
    let has_inline_attachments = email.attachments.iter().any(|a| a.is_inline);

    let mixed_boundary = format!("=_Mixed_{}", Uuid::new_v4().to_string().replace("-", ""));
    let related_boundary = format!("=_Related_{}", Uuid::new_v4().to_string().replace("-", ""));
    let alt_boundary = format!("=_Alternative_{}", Uuid::new_v4().to_string().replace("-", ""));

    let write_alt_part = |mime: &mut String| {
        if email.html_body.is_some() {
            mime.push_str(&format!(
                "Content-Type: multipart/alternative; boundary=\"{}\"\r\n\r\n",
                alt_boundary
            ));

            // Text Part
            mime.push_str(&format!("--{}\r\n", alt_boundary));
            mime.push_str("Content-Type: text/plain; charset=\"utf-8\"\r\n");
            mime.push_str("Content-Transfer-Encoding: 8bit\r\n\r\n");
            mime.push_str(&email.text_body);
            if !email.text_body.ends_with("\r\n") {
                mime.push_str("\r\n");
            }

            // HTML Part
            mime.push_str(&format!("--{}\r\n", alt_boundary));
            mime.push_str("Content-Type: text/html; charset=\"utf-8\"\r\n");
            mime.push_str("Content-Transfer-Encoding: 8bit\r\n\r\n");
            mime.push_str(email.html_body.as_ref().unwrap());
            if !email.html_body.as_ref().unwrap().ends_with("\r\n") {
                mime.push_str("\r\n");
            }

            mime.push_str(&format!("--{}--\r\n", alt_boundary));
        } else {
            mime.push_str("Content-Type: text/plain; charset=\"utf-8\"\r\n");
            mime.push_str("Content-Transfer-Encoding: 8bit\r\n\r\n");
            mime.push_str(&email.text_body);
            if !email.text_body.ends_with("\r\n") {
                mime.push_str("\r\n");
            }
        }
    };

    let write_attachment = |mime: &mut String, a: &Attachment| {
        let content_type = sanitize_header(&a.content_type);
        let filename = a.filename.as_deref().unwrap_or("attachment").to_string();
        let filename = sanitize_header(&filename);
        let disposition = if a.is_inline { "inline" } else { "attachment" };
        
        mime.push_str(&format!("Content-Type: {}; name=\"{}\"\r\n", content_type, filename));
        mime.push_str("Content-Transfer-Encoding: base64\r\n");
        mime.push_str(&format!("Content-Disposition: {}; filename=\"{}\"\r\n", disposition, filename));
        if let Some(ref cid) = a.content_id {
            mime.push_str(&format!("Content-ID: <{}>\r\n", sanitize_header(cid)));
        }
        mime.push_str("\r\n");

        use base64::{Engine as _, engine::general_purpose::STANDARD};
        let b64 = STANDARD.encode(&a.data);
        for chunk in b64.as_bytes().chunks(76) {
            mime.push_str(std::str::from_utf8(chunk).unwrap());
            mime.push_str("\r\n");
        }
    };

    if has_real_attachments {
        mime.push_str(&format!(
            "Content-Type: multipart/mixed; boundary=\"{}\"\r\n\r\n",
            mixed_boundary
        ));
        
        mime.push_str(&format!("--{}\r\n", mixed_boundary));
        
        if has_inline_attachments {
            mime.push_str(&format!(
                "Content-Type: multipart/related; boundary=\"{}\"\r\n\r\n",
                related_boundary
            ));
            mime.push_str(&format!("--{}\r\n", related_boundary));
            write_alt_part(&mut mime);
            
            for a in email.attachments.iter().filter(|a| a.is_inline) {
                mime.push_str(&format!("--{}\r\n", related_boundary));
                write_attachment(&mut mime, a);
            }
            mime.push_str(&format!("--{}--\r\n", related_boundary));
        } else {
            write_alt_part(&mut mime);
        }

        for a in email.attachments.iter().filter(|a| !a.is_inline) {
            mime.push_str(&format!("--{}\r\n", mixed_boundary));
            write_attachment(&mut mime, a);
        }
        mime.push_str(&format!("--{}--\r\n", mixed_boundary));
    } else if has_inline_attachments {
        mime.push_str(&format!(
            "Content-Type: multipart/related; boundary=\"{}\"\r\n\r\n",
            related_boundary
        ));
        mime.push_str(&format!("--{}\r\n", related_boundary));
        write_alt_part(&mut mime);
        
        for a in email.attachments.iter().filter(|a| a.is_inline) {
            mime.push_str(&format!("--{}\r\n", related_boundary));
            write_attachment(&mut mime, a);
        }
        mime.push_str(&format!("--{}--\r\n", related_boundary));
    } else {
        write_alt_part(&mut mime);
    }

    mime
}

/// Rewrites an email body's headers for secure two-way reply relaying.
pub fn rewrite_body_for_forward(
    body: &[u8],
    original_sender: &str,
    reply_from_email: &str,
    destination_email: &str,
) -> String {
    if let Some(message) = mail_parser::MessageParser::default().parse(body) {
        let original_sender_name = message
            .from()
            .and_then(|f| f.first())
            .and_then(|a| a.name())
            .unwrap_or(original_sender);

        let display_from = format!(
            "\"{} via Relay\" <{}>",
            original_sender_name, reply_from_email
        );

        let mut attachments = Vec::new();
        for part in message.attachments() {
            use mail_parser::MimeHeaders;
            let is_inline = matches!(part.body, mail_parser::PartType::InlineBinary(_))
                || part.content_disposition().is_some_and(|d| d.is_inline())
                || part.content_id().is_some();
            
            let content_id = part.content_id().map(|id| {
                let s = id.trim();
                s.strip_prefix('<').and_then(|s| s.strip_suffix('>')).unwrap_or(s).to_string()
            });

            attachments.push(Attachment {
                filename: part.attachment_name().map(|s| s.to_string()),
                content_type: part.content_type().map(|c| c.ctype().to_string()).unwrap_or_else(|| "application/octet-stream".to_string()),
                data: part.contents().to_vec(),
                is_inline,
                content_id,
            });
        }

        let mime_email = MimeEmail {
            from: display_from,
            to: destination_email.to_string(),
            subject: message.subject().unwrap_or("No Subject").to_string(),
            text_body: message
                .body_text(0)
                .map(|s| s.to_string())
                .unwrap_or_default(),
            html_body: message.body_html(0).map(|s| s.to_string()),
            message_id: message.message_id().map(|id| id.to_string()),
            in_reply_to: message.in_reply_to().as_text().map(|id| id.to_string()),
            references: message.references().as_text().map(|id| id.to_string()),
            attachments,
        };

        build_mime(&mime_email)
    } else {
        String::from_utf8_lossy(body).into_owned()
    }
}

/// Prepares and sanitizes a reply email received from Gmail, rewriting headers to look as if it was sent directly from the alias.
pub fn prepare_reply_for_relay(body: &[u8], alias_address: &str, original_sender: &str) -> String {
    if let Some(message) = mail_parser::MessageParser::default().parse(body) {
        let text_body = message
            .body_text(0)
            .map(|s| s.to_string())
            .unwrap_or_default();
        let html_body = message.body_html(0).map(|s| s.to_string());

        let mut attachments = Vec::new();
        for part in message.attachments() {
            use mail_parser::MimeHeaders;
            let is_inline = matches!(part.body, mail_parser::PartType::InlineBinary(_))
                || part.content_disposition().is_some_and(|d| d.is_inline())
                || part.content_id().is_some();
            
            let content_id = part.content_id().map(|id| {
                let s = id.trim();
                s.strip_prefix('<').and_then(|s| s.strip_suffix('>')).unwrap_or(s).to_string()
            });

            attachments.push(Attachment {
                filename: part.attachment_name().map(|s| s.to_string()),
                content_type: part.content_type().map(|c| c.ctype().to_string()).unwrap_or_else(|| "application/octet-stream".to_string()),
                data: part.contents().to_vec(),
                is_inline,
                content_id,
            });
        }

        let mime_email = MimeEmail {
            from: alias_address.to_string(),
            to: original_sender.to_string(),
            subject: message.subject().unwrap_or("No Subject").to_string(),
            text_body,
            html_body,
            message_id: None,
            in_reply_to: message.in_reply_to().as_text().map(|id| id.to_string()),
            references: message.references().as_text().map(|id| id.to_string()),
            attachments,
        };

        build_mime(&mime_email)
    } else {
        String::from_utf8_lossy(body).into_owned()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use mail_parser::MimeHeaders;

    #[test]
    fn test_crlf_injection_prevention() {
        let email = MimeEmail {
            from: "alice\r\nBcc: hacker@example.com\r\n@example.com".to_string(),
            to: "bob@example.com".to_string(),
            subject: "Hello\nBcc: hacker@example.com".to_string(),
            text_body: "Test".to_string(),
            html_body: None,
            message_id: None,
            in_reply_to: None,
            references: None,
            attachments: vec![],
        };

        let mime = build_mime(&email);
        assert!(mime.contains("From: aliceBcc: hacker@example.com@example.com\r\n"));
        assert!(mime.contains("Subject: HelloBcc: hacker@example.com\r\n"));
        assert!(!mime.contains("\r\nBcc: hacker@example.com\r\n"));
    }

    #[test]
    fn test_build_mime_threading() {
        let email = MimeEmail {
            from: "alice@example.com".to_string(),
            to: "bob@example.com".to_string(),
            subject: "Re: Hello".to_string(),
            text_body: "Reply content".to_string(),
            html_body: None,
            message_id: Some("new-id@example.com".to_string()),
            in_reply_to: Some("original-id@example.com".to_string()),
            references: Some("original-id@example.com".to_string()),
            attachments: vec![],
        };

        let mime = build_mime(&email);
        assert!(mime.contains("In-Reply-To: <original-id@example.com>\r\n"));
        assert!(mime.contains("References: <original-id@example.com>\r\n"));
        assert!(mime.contains("Subject: Re: Hello\r\n"));
    }

    #[test]
    fn test_auto_re_subject() {
        let email = MimeEmail {
            from: "alice@example.com".to_string(),
            to: "bob@example.com".to_string(),
            subject: "Hello".to_string(),
            text_body: "Reply content".to_string(),
            html_body: None,
            message_id: None,
            in_reply_to: Some("some-id".to_string()),
            references: None,
            attachments: vec![],
        };

        let mime = build_mime(&email);
        assert!(mime.contains("Subject: Re: Hello\r\n"));
    }

    #[test]
    fn test_rewrite_body_for_forward() {
        let raw_eml = b"From: Alice <alice@example.com>\r\nTo: bob@example.com\r\nSubject: Hello World\r\nMessage-ID: <msg1@example.com>\r\n\r\nBody text content";
        let rewritten = rewrite_body_for_forward(
            raw_eml,
            "alice@example.com",
            "reply-token@domain.com",
            "dest@gmail.com",
        );

        assert!(rewritten.contains("From: \"Alice via Relay\" <reply-token@domain.com>\r\n"));
        assert!(rewritten.contains("To: dest@gmail.com\r\n"));
        assert!(rewritten.contains("Subject: Hello World\r\n"));
        assert!(rewritten.contains("Message-ID: <msg1@example.com>\r\n"));
        assert!(rewritten.contains("Body text content"));
    }

    #[test]
    fn test_prepare_reply_for_relay() {
        let raw_gmail_reply = b"From: Me <user@gmail.com>\r\nTo: reply-token@domain.com\r\nSubject: Re: Hello World\r\nIn-Reply-To: <orig1@company.com>\r\nReferences: <orig1@company.com>\r\n\r\nI agree completely.";
        let prepared =
            prepare_reply_for_relay(raw_gmail_reply, "trash@domain.com", "support@company.com");

        assert!(prepared.contains("From: trash@domain.com\r\n"));
        assert!(prepared.contains("To: support@company.com\r\n"));
        assert!(prepared.contains("Subject: Re: Hello World\r\n"));
        assert!(prepared.contains("In-Reply-To: <orig1@company.com>\r\n"));
        assert!(prepared.contains("References: <orig1@company.com>\r\n"));
        assert!(prepared.contains("I agree completely."));
    }

    #[test]
    fn test_relay_attachment_roundtrip() {
        let original_data: Vec<u8> = vec![0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A, 0x00, 0x00, 0x00, 0x0D, 0x49, 0x48, 0x44, 0x52]; // fake PNG
        
        let original_email = MimeEmail {
            from: "alice@example.com".to_string(),
            to: "bob@example.com".to_string(),
            subject: "Look at this image".to_string(),
            text_body: "See attached.".to_string(),
            html_body: None,
            message_id: None,
            in_reply_to: None,
            references: None,
            attachments: vec![
                Attachment {
                    filename: Some("image.png".to_string()),
                    content_type: "image/png".to_string(),
                    data: original_data.clone(),
                    is_inline: false,
                    content_id: None,
                }
            ]
        };

        let raw_email = build_mime(&original_email);

        // Forward path
        let forwarded = rewrite_body_for_forward(
            raw_email.as_bytes(),
            "alice@example.com",
            "reply-token@domain.com",
            "dest@gmail.com"
        );

        // Parse forwarded
        let parsed = mail_parser::MessageParser::default().parse(forwarded.as_bytes()).unwrap();
        
        // Assert
        let atts: Vec<_> = parsed.attachments().collect();
        assert_eq!(atts.len(), 1);
        assert_eq!(atts[0].contents(), original_data.as_slice());
        assert_eq!(atts[0].attachment_name(), Some("image.png"));
    }

    #[test]
    fn test_relay_inline_image_and_pdf() {
        let image_data: Vec<u8> = vec![0x89, 0x50, 0x4E, 0x47]; // fake PNG
        let pdf_data: Vec<u8> = vec![0x25, 0x50, 0x44, 0x46]; // fake PDF
        
        let original_email = MimeEmail {
            from: "alice@example.com".to_string(),
            to: "bob@example.com".to_string(),
            subject: "Report".to_string(),
            text_body: "Here is the report with a logo.".to_string(),
            html_body: Some("<html><body><img src=\"cid:logo-img\" /></body></html>".to_string()),
            message_id: None,
            in_reply_to: None,
            references: None,
            attachments: vec![
                Attachment {
                    filename: Some("logo.png".to_string()),
                    content_type: "image/png".to_string(),
                    data: image_data.clone(),
                    is_inline: true,
                    content_id: Some("logo-img".to_string()),
                },
                Attachment {
                    filename: Some("report.pdf".to_string()),
                    content_type: "application/pdf".to_string(),
                    data: pdf_data.clone(),
                    is_inline: false,
                    content_id: None,
                }
            ]
        };

        let raw_email = build_mime(&original_email);

        // Forward path
        let forwarded = rewrite_body_for_forward(
            raw_email.as_bytes(),
            "alice@example.com",
            "reply-token@domain.com",
            "dest@gmail.com"
        );

        // Parse forwarded
        let parsed = mail_parser::MessageParser::default().parse(forwarded.as_bytes()).unwrap();
        
        // Assert
        let atts: Vec<_> = parsed.attachments().collect();
        assert_eq!(atts.len(), 2);

        let inline_img = atts.iter().find(|a| a.content_id() == Some("logo-img")).unwrap();
        assert_eq!(inline_img.contents(), image_data.as_slice());
        use mail_parser::MimeHeaders;
        assert!(inline_img.content_disposition().is_some_and(|d| d.is_inline()));

        let pdf = atts.iter().find(|a| a.attachment_name() == Some("report.pdf")).unwrap();
        assert_eq!(pdf.contents(), pdf_data.as_slice());
        assert!(!pdf.content_disposition().is_some_and(|d| d.is_inline()));
    }
}
