use time::OffsetDateTime;
use time::format_description::well_known::Rfc2822;
use uuid::Uuid;

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
    let now = OffsetDateTime::now_utc().format(&Rfc2822).unwrap_or_default();

    // 1. Standard Headers (Sanitized to prevent CRLF injection)
    mime.push_str(&format!("From: {}\r\n", sanitize_header(&email.from)));
    mime.push_str(&format!("To: {}\r\n", sanitize_header(&email.to)));
    
    let subject = if email.in_reply_to.is_some() && !email.subject.to_lowercase().starts_with("re:") {
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
    mime.push_str(&format!("Message-ID: {}\r\n", sanitize_header(&format_message_id(&message_id))));

    // 3. Threading Headers
    if let Some(ref irt) = email.in_reply_to {
        mime.push_str(&format!("In-Reply-To: {}\r\n", sanitize_header(&format_message_id(irt))));
    }
    if let Some(ref refs) = email.references {
        mime.push_str(&format!("References: {}\r\n", sanitize_header(&format_message_id(refs))));
    }

    // 4. Content and Body
    mime.push_str("MIME-Version: 1.0\r\n");

    match &email.html_body {
        Some(html) => {
            let boundary = format!("=_Boundary_{}", Uuid::new_v4().to_string().replace("-", ""));
            
            mime.push_str(&format!("Content-Type: multipart/alternative; boundary=\"{}\"\r\n", boundary));
            mime.push_str("\r\n");
            
            // Text Part
            mime.push_str(&format!("--{}\r\n", boundary));
            mime.push_str("Content-Type: text/plain; charset=\"utf-8\"\r\n");
            mime.push_str("Content-Transfer-Encoding: 8bit\r\n");
            mime.push_str("\r\n");
            mime.push_str(&email.text_body);
            if !email.text_body.ends_with("\r\n") {
                mime.push_str("\r\n");
            }
            
            // HTML Part
            mime.push_str(&format!("--{}\r\n", boundary));
            mime.push_str("Content-Type: text/html; charset=\"utf-8\"\r\n");
            mime.push_str("Content-Transfer-Encoding: 8bit\r\n");
            mime.push_str("\r\n");
            mime.push_str(html);
            if !html.ends_with("\r\n") {
                mime.push_str("\r\n");
            }
            
            mime.push_str(&format!("--{}--\r\n", boundary));
        }
        None => {
            mime.push_str("Content-Type: text/plain; charset=\"utf-8\"\r\n");
            mime.push_str("Content-Transfer-Encoding: 8bit\r\n");
            mime.push_str("\r\n");
            mime.push_str(&email.text_body);
        }
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
        let original_sender_name = message.from()
            .and_then(|f| f.first())
            .and_then(|a| a.name())
            .unwrap_or(original_sender);

        let display_from = format!("\"{} via Relay\" <{}>", original_sender_name, reply_from_email);

        let mime_email = MimeEmail {
            from: display_from,
            to: destination_email.to_string(),
            subject: message.subject().unwrap_or("No Subject").to_string(),
            text_body: message.body_text(0).map(|s| s.to_string()).unwrap_or_default(),
            html_body: message.body_html(0).map(|s| s.to_string()),
            message_id: message.message_id().map(|id| id.to_string()),
            in_reply_to: message.in_reply_to().as_text().map(|id| id.to_string()),
            references: message.references().as_text().map(|id| id.to_string()),
        };

        build_mime(&mime_email)
    } else {
        String::from_utf8_lossy(body).into_owned()
    }
}

/// Prepares and sanitizes a reply email received from Gmail, rewriting headers to look as if it was sent directly from the alias.
pub fn prepare_reply_for_relay(
    body: &[u8],
    alias_address: &str,
    original_sender: &str,
) -> String {
    if let Some(message) = mail_parser::MessageParser::default().parse(body) {
        let text_body = message.body_text(0).map(|s| s.to_string()).unwrap_or_default();
        let html_body = message.body_html(0).map(|s| s.to_string());

        let mime_email = MimeEmail {
            from: alias_address.to_string(),
            to: original_sender.to_string(),
            subject: message.subject().unwrap_or("No Subject").to_string(),
            text_body,
            html_body,
            message_id: None,
            in_reply_to: message.in_reply_to().as_text().map(|id| id.to_string()),
            references: message.references().as_text().map(|id| id.to_string()),
        };

        build_mime(&mime_email)
    } else {
        String::from_utf8_lossy(body).into_owned()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
        let prepared = prepare_reply_for_relay(
            raw_gmail_reply,
            "trash@domain.com",
            "support@company.com",
        );

        assert!(prepared.contains("From: trash@domain.com\r\n"));
        assert!(prepared.contains("To: support@company.com\r\n"));
        assert!(prepared.contains("Subject: Re: Hello World\r\n"));
        assert!(prepared.contains("In-Reply-To: <orig1@company.com>\r\n"));
        assert!(prepared.contains("References: <orig1@company.com>\r\n"));
        assert!(prepared.contains("I agree completely."));
    }
}
