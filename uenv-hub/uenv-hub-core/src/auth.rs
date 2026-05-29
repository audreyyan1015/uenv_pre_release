//! Token generation and Argon2 password-hash verification.
//!
//! Tokens are opaque random hex strings. We persist only an Argon2 hash plus a
//! short non-secret prefix used to look up the hash row quickly (so verifying a
//! token is one indexed lookup + one Argon2 compare rather than scanning).

use argon2::password_hash::{rand_core::OsRng, PasswordHash, PasswordHasher, PasswordVerifier, SaltString};
use argon2::Argon2;
use rand::RngCore;

/// Length of the non-secret prefix kept in plaintext for fast lookup.
pub const PREFIX_LEN: usize = 12;

/// A freshly generated token: the plaintext (shown once) plus storage fields.
pub struct GeneratedToken {
    pub plaintext: String,
    pub prefix: String,
    pub hash: String,
}

/// Generate a new random token and its Argon2 hash.
pub fn generate_token() -> Result<GeneratedToken, String> {
    let mut bytes = [0u8; 32];
    OsRng.fill_bytes(&mut bytes);
    let plaintext = format!("uenvh_{}", hex::encode(bytes));
    let prefix = plaintext.chars().take(PREFIX_LEN).collect::<String>();
    let hash = hash_token(&plaintext)?;
    Ok(GeneratedToken {
        plaintext,
        prefix,
        hash,
    })
}

/// Compute the prefix for an incoming plaintext token.
pub fn prefix_of(plaintext: &str) -> String {
    plaintext.chars().take(PREFIX_LEN).collect()
}

/// Argon2-hash a plaintext token.
pub fn hash_token(plaintext: &str) -> Result<String, String> {
    let salt = SaltString::generate(&mut OsRng);
    Argon2::default()
        .hash_password(plaintext.as_bytes(), &salt)
        .map(|h| h.to_string())
        .map_err(|e| e.to_string())
}

/// Verify a plaintext token against a stored Argon2 hash.
pub fn verify_token(plaintext: &str, stored_hash: &str) -> bool {
    let Ok(parsed) = PasswordHash::new(stored_hash) else {
        return false;
    };
    Argon2::default()
        .verify_password(plaintext.as_bytes(), &parsed)
        .is_ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roundtrip() {
        let t = generate_token().unwrap();
        assert!(t.plaintext.starts_with("uenvh_"));
        assert_eq!(t.prefix, prefix_of(&t.plaintext));
        assert!(verify_token(&t.plaintext, &t.hash));
        assert!(!verify_token("wrong", &t.hash));
    }
}
