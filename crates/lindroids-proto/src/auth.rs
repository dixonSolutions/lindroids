//! Mutual authentication on the link.
//!
//! Any app can reach a loopback port, and anything can listen on one first, so neither side
//! trusts the other on sight. Both know a secret from an earlier pairing; each proves it with an
//! HMAC over the other's fresh nonce, and the secret itself never crosses the link.
//!
//! The first time, the user pairs with a 6-digit code shown by the CLI; the server then hands the
//! app the secret.

use hmac::{Hmac, Mac};
use sha2::Sha256;

use base64::Engine;

fn mac(secret: &str, label: &str, nonce: &str) -> String {
    let mut m = Hmac::<Sha256>::new_from_slice(secret.as_bytes()).expect("any key length");
    m.update(label.as_bytes());
    m.update(b"\0");
    m.update(nonce.as_bytes());
    base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(m.finalize().into_bytes())
}

/// What the server sends to show it knows the secret (over the app's nonce).
pub fn server_proof(secret: &str, app_nonce: &str) -> String {
    mac(secret, "lindroids-server", app_nonce)
}

/// What the app sends to show it knows the secret (over the server's nonce).
pub fn app_proof(secret: &str, server_nonce: &str) -> String {
    mac(secret, "lindroids-app", server_nonce)
}

/// Constant-time comparison.
pub fn same(a: &str, b: &str) -> bool {
    a.len() == b.len() && a.bytes().zip(b.bytes()).fold(0u8, |acc, (x, y)| acc | (x ^ y)) == 0
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn proofs_differ_by_role_and_nonce() {
        let s = "secret";
        assert_ne!(server_proof(s, "n1"), app_proof(s, "n1"));
        assert_ne!(app_proof(s, "n1"), app_proof(s, "n2"));
        assert!(same(&app_proof(s, "n1"), &app_proof(s, "n1")));
        assert!(!same(&app_proof(s, "n1"), &app_proof("other", "n1")));
    }
}
