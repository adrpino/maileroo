use std::sync::OnceLock;
use uuid::Uuid;

static MULTIPLE_NEWLINES_REGEX: OnceLock<regex::Regex> = OnceLock::new();
static CID_REGEX: OnceLock<regex::Regex> = OnceLock::new();

pub fn sanitize_email_body(raw_body: &str, email_id: Uuid) -> String {
    let rewritten = rewrite_cid_references(raw_body, email_id);

    let mut builder = ammonia::Builder::default();
    builder
        .add_generic_attributes(&["style", "class", "id"])
        .rm_clean_content_tags(&["style"])
        .add_tags(&["style"])
        .url_relative(ammonia::UrlRelative::PassThrough)
        .link_rel(Some("noopener noreferrer"));

    let sanitized = builder.clean(&rewritten).to_string();

    let re_newlines = MULTIPLE_NEWLINES_REGEX.get_or_init(|| regex::Regex::new(r"\n{3,}").unwrap());
    re_newlines.replace_all(&sanitized, "\n\n").to_string()
}

pub fn rewrite_cid_references(html: &str, email_id: Uuid) -> String {
    let cid_re = CID_REGEX.get_or_init(|| regex::Regex::new(r#"src=["']cid:([^"']+)["']"#).unwrap());
    cid_re
        .replace_all(html, format!(r#"src="/dashboard/email/{}/inline/$1""#, email_id))
        .to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_rewrite_cid_references() {
        let email_id = Uuid::parse_str("550e8400-e29b-41d4-a716-446655440000").unwrap();
        let html = r#"<img src="cid:logo.png" alt="logo"><img src='cid:header_img' />"#;
        
        let rewritten = rewrite_cid_references(html, email_id);
        
        assert_eq!(
            rewritten,
            r#"<img src="/dashboard/email/550e8400-e29b-41d4-a716-446655440000/inline/logo.png" alt="logo"><img src="/dashboard/email/550e8400-e29b-41d4-a716-446655440000/inline/header_img" />"#
        );
    }
}
