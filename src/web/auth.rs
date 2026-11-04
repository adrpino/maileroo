use argon2::{
    Argon2,
    password_hash::{PasswordHash, PasswordHasher, PasswordVerifier, SaltString, rand_core::OsRng},
};
use std::sync::OnceLock;

static DUMMY_PASSWORD_HASH: OnceLock<String> = OnceLock::new();

pub fn hash_password(password: &str) -> Result<String, argon2::password_hash::Error> {
    let salt = SaltString::generate(&mut OsRng);
    let argon2 = Argon2::default();
    let password_hash = argon2.hash_password(password.as_bytes(), &salt);
    match password_hash {
        Ok(h) => Ok(h.to_string()),
        Err(e) => Err(e),
    }
}

pub fn verify_password(password: &str, hash: &str) -> bool {
    let parsed_hash = match PasswordHash::new(hash) {
        Ok(h) => h,
        Err(_) => return false,
    };
    Argon2::default()
        .verify_password(password.as_bytes(), &parsed_hash)
        .is_ok()
}

/// Returns a lazily-generated Argon2id hash with the exact same parameters as
/// [`hash_password`], used to equalize verification work when a user does not exist.
pub fn dummy_password_hash() -> &'static str {
    DUMMY_PASSWORD_HASH.get_or_init(|| {
        hash_password("dummy-password-for-constant-time-login")
            .expect("failed to generate dummy Argon2 hash")
    })
}

/// Verifies `password` against `user_hash` when present, or against [`dummy_password_hash`]
/// when `None`, so login performs a full Argon2id verification on both paths.
pub fn verify_password_constant_time(password: &str, user_hash: Option<&str>) -> bool {
    match user_hash {
        Some(hash) => verify_password(password, hash),
        None => {
            let _ = verify_password(password, dummy_password_hash());
            false
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_verify_password_constant_time_behavior() {
        let real_hash = hash_password("correct-horse-battery-staple").unwrap();

        // Matching password + Some(hash) -> true
        assert!(verify_password_constant_time(
            "correct-horse-battery-staple",
            Some(&real_hash)
        ));

        // Wrong password + Some(hash) -> false
        assert!(!verify_password_constant_time(
            "wrong-password",
            Some(&real_hash)
        ));

        // Non-existent user (None) -> false (even if password matches the dummy plaintext)
        assert!(!verify_password_constant_time("wrong-password", None));
        assert!(!verify_password_constant_time(
            "dummy-password-for-constant-time-login",
            None
        ));
    }

    #[test]
    fn test_dummy_password_hash_is_valid_argon2() {
        let dummy = dummy_password_hash();
        assert!(dummy.starts_with("$argon2"));
        assert!(PasswordHash::new(dummy).is_ok());
    }
}
