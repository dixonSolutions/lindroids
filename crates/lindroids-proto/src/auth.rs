//! Mutual authentication on the link.
//!
//! Any app can reach a loopback port, and anything can listen on one first, so neither side
//! trusts the other on sight. Both hold ECDSA P-256 keys (the one curve every browser's WebCrypto
//! has): the relay its own, each display (a browser profile, `debug-app`) one it made when it
//! paired. Each signs the other's fresh nonce, bound to the page's origin; private keys never
//! leave their side, and the relay keeps only displays' public keys.
//!
//! The first time, a display pairs with a ticket the CLI put in the page's URL, or with the
//! 6-digit code the CLI shows; it then pins the relay's key, and the relay records its own.
//!
//! On the wire: public keys are SEC1 uncompressed points, signatures r‖s (64 bytes, what
//! WebCrypto makes), both base64url without padding.

use base64::Engine;
use p256::ecdsa::signature::{Signer, Verifier};
use p256::ecdsa::{Signature, SigningKey, VerifyingKey};

const B64: base64::engine::GeneralPurpose = base64::engine::general_purpose::URL_SAFE_NO_PAD;

/// A P-256 key pair.
pub struct KeyPair {
    key: SigningKey,
}

impl KeyPair {
    /// A new key from the kernel's randomness.
    pub fn generate() -> std::io::Result<KeyPair> {
        loop {
            let mut bytes = [0u8; 32];
            random_bytes(&mut bytes)?;
            // A random 32 bytes is a valid scalar unless it is 0 or at least the group order
            // (about 2^-32 of them).
            if let Some(k) = KeyPair::from_bytes(&bytes) {
                return Ok(k);
            }
        }
    }

    /// The private scalar, as `to_bytes` wrote it.
    pub fn from_bytes(bytes: &[u8]) -> Option<KeyPair> {
        SigningKey::from_slice(bytes).ok().map(|key| KeyPair { key })
    }

    pub fn to_bytes(&self) -> Vec<u8> {
        self.key.to_bytes().to_vec()
    }

    /// The public key, as the other side imports it.
    pub fn public(&self) -> String {
        B64.encode(self.key.verifying_key().to_encoded_point(false).as_bytes())
    }

    /// ECDSA with SHA-256 over `msg`.
    pub fn sign(&self, msg: &[u8]) -> String {
        let sig: Signature = self.key.sign(msg);
        B64.encode(sig.to_bytes())
    }
}

/// Whether `sig` is `public`'s signature over `msg`.
pub fn verify(public: &str, msg: &[u8], sig: &str) -> bool {
    let (Ok(pk), Ok(sig)) = (B64.decode(public.trim()), B64.decode(sig.trim())) else { return false };
    let (Ok(vk), Ok(sig)) = (VerifyingKey::from_sec1_bytes(&pk), Signature::from_slice(&sig)) else { return false };
    vk.verify(msg, &sig).is_ok()
}

/// Whether `public` is a P-256 public key at all.
pub fn valid_public(public: &str) -> bool {
    B64.decode(public.trim()).ok().map_or(false, |pk| VerifyingKey::from_sec1_bytes(&pk).is_ok())
}

/// What the relay signs: the page's nonce, its own, and the origin the page was served from
/// (empty for a display that is not a web page), so a signature made for one page or one
/// connection proves nothing anywhere else.
pub fn relay_transcript(origin: &str, display_nonce: &str, relay_nonce: &str) -> Vec<u8> {
    transcript("lindroids/relay", origin, display_nonce, relay_nonce)
}

/// What the display signs: the same, the other way round.
pub fn display_transcript(origin: &str, relay_nonce: &str, display_nonce: &str) -> Vec<u8> {
    transcript("lindroids/display", origin, relay_nonce, display_nonce)
}

fn transcript(label: &str, origin: &str, a: &str, b: &str) -> Vec<u8> {
    [label, origin, a, b].join("\0").into_bytes()
}

/// Constant-time comparison.
pub fn same(a: &str, b: &str) -> bool {
    a.len() == b.len() && a.bytes().zip(b.bytes()).fold(0u8, |acc, (x, y)| acc | (x ^ y)) == 0
}

/// Fills `buf` from the kernel's randomness.
pub fn random_bytes(buf: &mut [u8]) -> std::io::Result<()> {
    use std::io::Read;
    std::fs::File::open("/dev/urandom")?.read_exact(buf)
}

/// A random token for URLs and names: `n` bytes of randomness, base64url.
pub fn random_token(n: usize) -> String {
    let mut b = vec![0u8; n];
    random_bytes(&mut b).expect("/dev/urandom");
    B64.encode(b)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn signatures_verify_only_for_their_key_and_message() {
        let a = KeyPair::generate().unwrap();
        let b = KeyPair::generate().unwrap();
        let m = relay_transcript("http://x.localhost:47000", "np", "nr");
        let s = a.sign(&m);
        assert!(verify(&a.public(), &m, &s));
        assert!(!verify(&b.public(), &m, &s));
        assert!(!verify(&a.public(), &relay_transcript("http://x.localhost:47000", "np", "other"), &s));
        assert!(!verify(&a.public(), &display_transcript("http://x.localhost:47000", "np", "nr"), &s));
        assert!(!verify("garbage", &m, &s));
        assert!(valid_public(&a.public()) && !valid_public("AAAA"));
    }

    #[test]
    fn keys_round_trip() {
        let a = KeyPair::generate().unwrap();
        let b = KeyPair::from_bytes(&a.to_bytes()).unwrap();
        assert_eq!(a.public(), b.public());
        // Uncompressed SEC1: 65 bytes.
        assert_eq!(B64.decode(a.public()).unwrap().len(), 65);
    }

    #[test]
    fn signatures_are_raw_r_and_s() {
        // WebCrypto's ECDSA signatures are r‖s, 64 bytes for P-256, not DER.
        let a = KeyPair::generate().unwrap();
        assert_eq!(B64.decode(a.sign(b"x")).unwrap().len(), 64);
    }
}
