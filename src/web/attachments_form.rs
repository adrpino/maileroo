use std::collections::HashMap;

use axum::extract::multipart::Multipart;
use axum::http::StatusCode;

use crate::outbound::mime::Attachment;
use crate::outbound::mime::sanitize_header;

pub const MAX_ATTACHMENTS: usize = 10;
pub const MAX_ATTACHMENT_BYTES: usize = 10 * 1024 * 1024; // 10 MB
pub const MAX_TOTAL_ATTACHMENT_BYTES: usize = 25 * 1024 * 1024; // 25 MB

/// Text fields and attachments collected from a multipart form submission.
#[derive(Debug, Default)]
pub struct MultipartFields {
    pub text: HashMap<String, String>,
    pub attachments: Vec<Attachment>,
    /// Set when the multipart body violates the attachment limits; carries
    /// the HTTP status and a user-facing message.
    pub error: Option<(StatusCode, String)>,
}

/// Cleans a client-provided filename: strips path separators and falls back
/// to a stable placeholder when nothing usable remains.
pub fn clean_filename(filename: Option<&str>) -> Option<String> {
    match filename {
        Some(f) => {
            let cleaned = f.replace(['/', '\\'], "").trim().to_string();
            if cleaned.is_empty() {
                Some("attachment".to_string())
            } else {
                Some(cleaned)
            }
        }
        None => Some("attachment".to_string()),
    }
}

/// Resolves the effective content type: keep the client-provided one when
/// usable, otherwise guess from the filename, else a generic binary type.
pub fn resolve_content_type(provided: Option<&str>, filename: Option<&str>) -> String {
    if let Some(ct) = provided.filter(|ct| !ct.trim().is_empty()) {
        return ct.to_string();
    }
    if let Some(fname) = filename {
        return mime_guess::from_path(fname)
            .first_raw()
            .unwrap_or("application/octet-stream")
            .to_string();
    }
    "application/octet-stream".to_string()
}

/// Failure modes for attachment limit validation.
#[derive(Debug, PartialEq, Eq)]
pub enum AttachmentLimitError {
    SingleFileTooLarge,
    TooManyFiles,
    TotalTooLarge,
}

/// Pure limit check for the next candidate attachment.
pub fn check_attachment_limits(
    current_count: usize,
    current_total_bytes: usize,
    next_file_bytes: usize,
) -> Result<(), AttachmentLimitError> {
    if next_file_bytes > MAX_ATTACHMENT_BYTES {
        return Err(AttachmentLimitError::SingleFileTooLarge);
    }
    if current_count + 1 > MAX_ATTACHMENTS {
        return Err(AttachmentLimitError::TooManyFiles);
    }
    if current_total_bytes + next_file_bytes > MAX_TOTAL_ATTACHMENT_BYTES {
        return Err(AttachmentLimitError::TotalTooLarge);
    }
    Ok(())
}

/// Human-readable message for a limit violation.
pub fn limit_error_message(err: &AttachmentLimitError) -> String {
    match err {
        AttachmentLimitError::SingleFileTooLarge => format!(
            "Attachment exceeds maximum limit of {} MB.",
            MAX_ATTACHMENT_BYTES / 1024 / 1024
        ),
        AttachmentLimitError::TooManyFiles => {
            format!("Too many attachments. Maximum is {}.", MAX_ATTACHMENTS)
        }
        AttachmentLimitError::TotalTooLarge => format!(
            "Total attachments size exceeds limit of {} MB.",
            MAX_TOTAL_ATTACHMENT_BYTES / 1024 / 1024
        ),
    }
}

fn total_bytes(attachments: &[Attachment]) -> usize {
    attachments.iter().map(|a| a.data.len()).sum()
}

/// Reads every field of a multipart body into text fields and attachments.
/// Attachment fields must be named `attachments`; every other field is
/// collected as UTF-8 text. Stops early with `error` set when a limit is
/// violated.
pub async fn read_multipart_fields(mut multipart: Multipart) -> MultipartFields {
    let mut result = MultipartFields::default();

    while let Ok(Some(mut field)) = multipart.next_field().await {
        let name = match field.name() {
            Some(name) => name.to_string(),
            None => continue,
        };

        if name == "attachments" {
            let filename = field.file_name().map(sanitize_header);
            let content_type = field.content_type().map(sanitize_header);

            let mut data = Vec::new();
            let mut single_file_error = false;
            while let Ok(Some(chunk)) = field.chunk().await {
                if data.len() + chunk.len() > MAX_ATTACHMENT_BYTES {
                    single_file_error = true;
                }
                data.extend_from_slice(&chunk);
            }

            let limit_error = if single_file_error {
                Some(AttachmentLimitError::SingleFileTooLarge)
            } else if data.is_empty() {
                None
            } else {
                check_attachment_limits(
                    result.attachments.len(),
                    total_bytes(&result.attachments),
                    data.len(),
                )
                .err()
            };

            if let Some(err) = limit_error {
                result.error = Some((StatusCode::PAYLOAD_TOO_LARGE, limit_error_message(&err)));
                break;
            }

            if data.is_empty() {
                continue;
            }

            result.attachments.push(Attachment {
                filename: clean_filename(filename.as_deref()),
                content_type: resolve_content_type(content_type.as_deref(), filename.as_deref()),
                data,
                is_inline: false,
                content_id: None,
            });
        } else {
            match field.text().await {
                Ok(value) => {
                    result.text.insert(name, value);
                }
                Err(e) => {
                    tracing::error!("Failed to read field {}: {}", name, e);
                }
            }
        }
    }

    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_clean_filename_strips_paths() {
        // Separators are stripped, so the result stays a flat name and no
        // path traversal is possible either way.
        assert_eq!(
            clean_filename(Some("/etc/passwd")),
            Some("etcpasswd".to_string())
        );
        assert_eq!(
            clean_filename(Some("..\\win\\evil.pdf")),
            Some("..winevil.pdf".to_string())
        );
    }

    #[test]
    fn test_clean_filename_fallbacks() {
        assert_eq!(clean_filename(Some("   ")), Some("attachment".to_string()));
        assert_eq!(clean_filename(Some("////")), Some("attachment".to_string()));
        assert_eq!(clean_filename(None), Some("attachment".to_string()));
        assert_eq!(
            clean_filename(Some(" report.pdf ")),
            Some("report.pdf".to_string())
        );
    }

    #[test]
    fn test_resolve_content_type_priority() {
        // Client-provided non-empty wins over the guess.
        assert_eq!(
            resolve_content_type(Some("application/pdf"), Some("x.txt")),
            "application/pdf"
        );
        // Empty string falls back to the filename guess.
        assert_eq!(
            resolve_content_type(Some("   "), Some("photo.png")),
            "image/png"
        );
        // Nothing usable anywhere: generic binary.
        assert_eq!(resolve_content_type(None, None), "application/octet-stream");
        // Unknown extension: generic binary.
        assert_eq!(
            resolve_content_type(None, Some("blob.xyzunknown")),
            "application/octet-stream"
        );
    }

    #[test]
    fn test_check_attachment_limits_pass() {
        assert!(check_attachment_limits(0, 0, 1024).is_ok());
        assert!(
            check_attachment_limits(MAX_ATTACHMENTS - 1, MAX_TOTAL_ATTACHMENT_BYTES - 1024, 1024)
                .is_ok()
        );
    }

    #[test]
    fn test_check_attachment_limits_single_file() {
        assert_eq!(
            check_attachment_limits(0, 0, MAX_ATTACHMENT_BYTES + 1),
            Err(AttachmentLimitError::SingleFileTooLarge)
        );
    }

    #[test]
    fn test_check_attachment_limits_count() {
        assert_eq!(
            check_attachment_limits(MAX_ATTACHMENTS, 0, 10),
            Err(AttachmentLimitError::TooManyFiles)
        );
    }

    #[test]
    fn test_check_attachment_limits_total() {
        assert_eq!(
            check_attachment_limits(1, MAX_TOTAL_ATTACHMENT_BYTES, 1),
            Err(AttachmentLimitError::TotalTooLarge)
        );
    }

    #[test]
    fn test_limit_error_messages_are_user_readable() {
        assert!(limit_error_message(&AttachmentLimitError::SingleFileTooLarge).contains("MB."));
        assert!(limit_error_message(&AttachmentLimitError::TooManyFiles).contains("Maximum is"));
        assert!(limit_error_message(&AttachmentLimitError::TotalTooLarge).contains("MB."));
    }
}
