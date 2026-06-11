use base64::Engine;
use mail_auth::common::crypto::Ed25519Key;

/// Generates a secure Ed25519 key pair for DKIM.
/// Returns a tuple of `(private_key_b64, public_key_dns)`.
pub fn generate_dkim_key_pair() -> Result<(String, String), anyhow::Error> {
    let pkcs8_bytes = Ed25519Key::generate_pkcs8()
        .map_err(|e| anyhow::anyhow!("Failed to generate Ed25519 key: {}", e))?;

    // Parse the generated PKCS#8 key to get the public key
    let key_pair = Ed25519Key::from_pkcs8_der(&pkcs8_bytes)
        .map_err(|e| anyhow::anyhow!("Failed to parse generated Ed25519 key: {}", e))?;

    let public_der = key_pair.public_key();

    let private_b64 = base64::prelude::BASE64_STANDARD.encode(&pkcs8_bytes);
    let public_dns = base64::prelude::BASE64_STANDARD.encode(public_der);

    Ok((private_b64, public_dns))
}

/// Signs the provided raw RFC 5322 email body using the specified DKIM settings.
/// If signing fails, it returns the original body unmodified.
pub fn sign_message_with_dkim(
    body: &[u8],
    sender_domain: &str,
    private_b64: &str,
    selector: &str,
) -> Vec<u8> {
    if let Ok(der_bytes) = base64::prelude::BASE64_STANDARD.decode(private_b64) {
        if let Ok(pk_ed) = Ed25519Key::from_pkcs8_der(&der_bytes) {
            let signer = mail_auth::dkim::DkimSigner::from_key(pk_ed)
                .domain(sender_domain)
                .selector(selector)
                .headers(["From", "To", "Subject", "Date", "Message-ID"]);

            if let Ok(signature) = signer.sign(body) {
                let sig_header = signature.to_string();
                let mut signed_body = Vec::with_capacity(sig_header.len() + 2 + body.len());
                signed_body.extend_from_slice(sig_header.as_bytes());
                signed_body.extend_from_slice(b"\r\n");
                signed_body.extend_from_slice(body);
                return signed_body;
            }
        }
    }
    body.to_vec()
}
