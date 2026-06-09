use base32::Alphabet;
use hmac::{Hmac, Mac};
use sha2::Sha256;

type HmacSha256 = Hmac<Sha256>;

/// Generates the current SRS timestamp segment (number of days since 1970 modulo 1024) safely.
/// Returns 0 if the host clock is set to a date prior to the Unix Epoch (1970).
fn get_current_srs_timestamp() -> u64 {
    (std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
        / 86400)
        % 1024
}

pub fn encode_srs(original_sender: &str, domain: &str, secret: &str) -> String {
    let (local, sender_domain) = original_sender
        .split_once('@')
        .unwrap_or((original_sender, "unknown"));

    // 1. Generate a timestamp (e.g., number of days since epoch)
    let timestamp = get_current_srs_timestamp();

    // 2. Generate a hash to prevent tampering
    let mut mac =
        HmacSha256::new_from_slice(secret.as_bytes()).expect("HMAC can take key of any size");
    mac.update(format!("{}{}{}", timestamp, sender_domain, local).as_bytes());
    let result = mac.finalize().into_bytes();
    let hash = &base32::encode(Alphabet::Crockford, &result)[..4]; // Just a few chars for brevity

    // 3. Format: SRS0+Hash=Timestamp=OriginalDomain=LocalPart@YourDomain
    format!(
        "SRS0+{}={}={}={}@{}",
        hash, timestamp, sender_domain, local, domain
    )
}

/// Decodes an SRS-rewritten address back to its original sender.
/// Returns `Some(original_sender_email)` if valid, or `None` if invalid, expired, or tampered.
pub fn decode_srs(srs_address: &str, secret: &str) -> Option<String> {
    let (local_part, _) = srs_address.split_once('@')?;

    if !local_part.to_lowercase().starts_with("srs0+") {
        return None;
    }

    let (_, data) = local_part.split_once('+')?;
    let parts: Vec<&str> = data.splitn(4, '=').collect();
    if parts.len() != 4 {
        return None;
    }

    let hash = parts[0];
    let timestamp_str = parts[1];
    let sender_domain = parts[2];
    let sender_local = parts[3];

    let timestamp: u64 = timestamp_str.parse().ok()?;
    let current_timestamp = get_current_srs_timestamp();

    // Verify timestamp expiration (21 days limit) with modulo 1024 wrap-around support
    let diff = if current_timestamp >= timestamp {
        current_timestamp - timestamp
    } else {
        (current_timestamp + 1024) - timestamp
    };
    if diff > 21 {
        return None;
    }

    let mut mac =
        HmacSha256::new_from_slice(secret.as_bytes()).ok()?;
    mac.update(format!("{}{}{}", timestamp, sender_domain, sender_local).as_bytes());
    let result = mac.finalize().into_bytes();
    let expected_hash = &base32::encode(Alphabet::Crockford, &result)[..4];

    if hash.to_uppercase() != expected_hash.to_uppercase() {
        return None;
    }

    Some(format!("{}@{}", sender_local, sender_domain))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_get_current_srs_timestamp_range() {
        let ts = get_current_srs_timestamp();
        assert!(ts < 1024);
    }

    #[test]
    fn test_encode_srs_format() {
        let secret = "secret";
        let domain = "example.com";
        let sender = "alice@gmail.com";
        let encoded = encode_srs(sender, domain, secret);

        assert!(encoded.starts_with("SRS0+"));
        assert!(encoded.contains("=gmail.com=alice@example.com"));
    }

    #[test]
    fn test_encode_srs_consistency() {
        let secret = "secret";
        let domain = "example.com";
        let sender = "alice@gmail.com";

        let encoded1 = encode_srs(sender, domain, secret);
        let encoded2 = encode_srs(sender, domain, secret);

        assert_eq!(encoded1, encoded2);
    }

    #[test]
    fn test_encode_srs_different_domains() {
        let secret = "secret";
        let sender = "alice@gmail.com";

        let encoded1 = encode_srs(sender, "example.com", secret);
        let encoded2 = encode_srs(sender, "other.com", secret);

        assert_ne!(encoded1, encoded2);
        assert!(encoded1.ends_with("@example.com"));
        assert!(encoded2.ends_with("@other.com"));
    }

    #[test]
    fn test_decode_srs_valid() {
        let secret = "secret";
        let domain = "example.com";
        let sender = "alice@gmail.com";

        let encoded = encode_srs(sender, domain, secret);
        let decoded = decode_srs(&encoded, secret);

        assert_eq!(decoded, Some(sender.to_string()));
    }

    #[test]
    fn test_decode_srs_invalid_secret() {
        let secret = "secret";
        let domain = "example.com";
        let sender = "alice@gmail.com";

        let encoded = encode_srs(sender, domain, secret);
        let decoded = decode_srs(&encoded, "wrong_secret");

        assert_eq!(decoded, None);
    }

    #[test]
    fn test_decode_srs_tampered_hash() {
        let secret = "secret";
        let domain = "example.com";
        let sender = "alice@gmail.com";

        let encoded = encode_srs(sender, domain, secret);
        // Replace first character after SRS0+ with something else
        let tampered = encoded.replace("SRS0+", "SRS0+A");
        let decoded = decode_srs(&tampered, secret);

        assert_eq!(decoded, None);
    }

    #[test]
    fn test_decode_srs_expired_timestamp() {
        let secret = "secret";
        // Create an SRS manually with an expired timestamp (e.g. current_timestamp - 30)
        let current_timestamp = (std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs()
            / 86400)
            % 1024;
        let expired_timestamp = (current_timestamp + 1024 - 30) % 1024;

        let mut mac =
            HmacSha256::new_from_slice(secret.as_bytes()).unwrap();
        mac.update(format!("{}gmail.comalice", expired_timestamp).as_bytes());
        let result = mac.finalize().into_bytes();
        let hash = &base32::encode(Alphabet::Crockford, &result)[..4];

        let expired_srs = format!("SRS0+{}={}={}={}@example.com", hash, expired_timestamp, "gmail.com", "alice");
        let decoded = decode_srs(&expired_srs, secret);

        assert_eq!(decoded, None);
    }

    #[test]
    fn test_decode_non_srs() {
        assert_eq!(decode_srs("alice@gmail.com", "secret"), None);
        assert_eq!(decode_srs("SRS1+hash=timestamp=domain=local@domain.com", "secret"), None);
    }
}
