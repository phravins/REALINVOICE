//! Password hashing and local session tokens.
//!
//! bcrypt, because it is one function each way with the salt and cost carried inside the
//! hash string — nothing for a caller to store, configure or get wrong. Passwords are
//! hashed here and nowhere else; no other module sees a plaintext password.

use bcrypt::{hash, verify, DEFAULT_COST};
use rand::Rng;

use crate::error::{CoreError, Result};

/// Cost factor. bcrypt's default (12) costs roughly a quarter-second on the low-spec
/// machines this runs on — slow enough to matter to an attacker, fast enough that a
/// cashier signing in does not notice.
const COST: u32 = DEFAULT_COST;

/// Shortest password the seeder or a future admin screen may set.
pub const MIN_PASSWORD_LEN: usize = 8;

/// Hashes a password for storage. The returned string carries its own salt and cost.
pub fn hash_password(password: &str) -> Result<String> {
    if password.len() < MIN_PASSWORD_LEN {
        return Err(CoreError::Invalid(format!(
            "password must be at least {MIN_PASSWORD_LEN} characters"
        )));
    }
    hash(password, COST).map_err(|e| CoreError::Invalid(format!("could not hash password: {e}")))
}

/// Checks a password against a stored hash.
///
/// A malformed or truncated hash returns `false` rather than an error: a corrupted row
/// must fail the sign-in, never wave it through.
pub fn verify_password(password: &str, password_hash: &str) -> bool {
    verify(password, password_hash).unwrap_or(false)
}

/// A session token for the life of this process.
///
/// Local only: the app is one desktop process, so there is nothing to sign, no issuer to
/// trust and no expiry to enforce. This exists so a caller must present something it was
/// given rather than assert it is signed in. Random enough not to be guessed by anything
/// else running on the machine.
pub fn new_session_token() -> String {
    let bytes: [u8; 16] = rand::rng().random();
    uuid::Uuid::from_bytes(bytes).to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_password_verifies_against_its_own_hash() {
        let stored = hash_password("correct horse battery").unwrap();
        assert!(verify_password("correct horse battery", &stored));
        assert!(!verify_password("Correct horse battery", &stored));
        assert!(!verify_password("", &stored));
    }

    #[test]
    fn the_hash_is_not_the_password() {
        let stored = hash_password("hunter2hunter2").unwrap();
        assert!(!stored.contains("hunter2"));
        assert!(stored.starts_with("$2"), "a bcrypt hash: {stored}");
    }

    #[test]
    fn the_same_password_hashes_differently_every_time() {
        // Each hash carries its own salt, so two hashes of one password never match.
        let a = hash_password("same password").unwrap();
        let b = hash_password("same password").unwrap();
        assert_ne!(a, b);
        assert!(verify_password("same password", &a));
        assert!(verify_password("same password", &b));
    }

    #[test]
    fn short_passwords_are_refused() {
        assert!(hash_password("short").is_err());
        assert!(hash_password("12345678").is_ok());
    }

    #[test]
    fn a_corrupt_hash_fails_closed() {
        assert!(!verify_password("anything", "not-a-hash"));
        assert!(!verify_password("anything", ""));
    }

    #[test]
    fn session_tokens_are_unique() {
        let tokens: std::collections::HashSet<String> =
            (0..64).map(|_| new_session_token()).collect();
        assert_eq!(tokens.len(), 64);
        assert_eq!(tokens.iter().next().unwrap().len(), 36, "uuid-shaped");
    }
}
