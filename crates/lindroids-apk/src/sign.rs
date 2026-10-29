//! Signing: APK Signature Scheme v2 with an ECDSA P-256 key, and the self-signed certificate
//! that carries the key. v2 is all Android 7 (the app's minimum) and later need; there is no JAR
//! (v1) signature.
//!
//! The key is made once per phone and kept in Lindroids' state: every program's app is signed
//! with it, so an app can be updated (a new icon, a new port) only from the same phone's
//! Lindroids. Losing the key means uninstalling the apps before installing them again.

use p256::ecdsa::signature::Signer;
use p256::ecdsa::{Signature, SigningKey};
use sha2::{Digest, Sha256};

const V2_BLOCK_ID: u32 = 0x7109_871a;
const SIG_ECDSA_SHA256: u32 = 0x0201;
const CHUNK: usize = 1 << 20;

pub struct Key {
    key: SigningKey,
    cert: Vec<u8>,
}

impl Key {
    /// A new random key and its certificate, named `name` (CN).
    pub fn generate(name: &str) -> Result<Key, String> {
        let mut f = std::fs::File::open("/dev/urandom").map_err(|e| format!("/dev/urandom: {e}"))?;
        for _ in 0..8 {
            let mut secret = [0u8; 32];
            std::io::Read::read_exact(&mut f, &mut secret).map_err(|e| format!("/dev/urandom: {e}"))?;
            if let Ok(k) = SigningKey::from_slice(&secret) {
                let cert = certificate(&k, name);
                return Ok(Key { key: k, cert });
            }
        }
        Err("cannot make a signing key".into())
    }

    /// The key as it is saved: the 32-byte secret, then the certificate.
    pub fn to_bytes(&self) -> Vec<u8> {
        let mut out = self.key.to_bytes().to_vec();
        out.extend_from_slice(&self.cert);
        out
    }

    pub fn from_bytes(b: &[u8]) -> Result<Key, String> {
        if b.len() < 33 {
            return Err("the signing key file is damaged".into());
        }
        let key = SigningKey::from_slice(&b[..32]).map_err(|_| "the signing key file is damaged")?;
        Ok(Key { key, cert: b[32..].to_vec() })
    }

    /// SHA-256 of the certificate: what Android shows as the app's signer.
    pub fn fingerprint(&self) -> String {
        Sha256::digest(&self.cert).iter().map(|b| format!("{b:02X}")).collect::<Vec<_>>().join(":")
    }

    fn sign(&self, data: &[u8]) -> Vec<u8> {
        let sig: Signature = self.key.sign(data);
        ecdsa_der(&sig)
    }
}

// --- DER, as much as a certificate needs

fn der(tag: u8, body: &[u8]) -> Vec<u8> {
    let mut out = vec![tag];
    let n = body.len();
    if n < 0x80 {
        out.push(n as u8);
    } else {
        let bytes: Vec<u8> = n.to_be_bytes().iter().copied().skip_while(|&b| b == 0).collect();
        out.push(0x80 | bytes.len() as u8);
        out.extend_from_slice(&bytes);
    }
    out.extend_from_slice(body);
    out
}

fn seq(parts: &[Vec<u8>]) -> Vec<u8> {
    der(0x30, &parts.concat())
}

fn uint(bytes: &[u8]) -> Vec<u8> {
    let mut b: Vec<u8> = bytes.iter().copied().skip_while(|&x| x == 0).collect();
    if b.is_empty() || b[0] & 0x80 != 0 {
        b.insert(0, 0);
    }
    der(0x02, &b)
}

fn oid(encoded: &[u8]) -> Vec<u8> {
    der(0x06, encoded)
}

// 1.2.840.10045.4.3.2, 1.2.840.10045.2.1, 1.2.840.10045.3.1.7, 2.5.4.3
const ECDSA_WITH_SHA256: &[u8] = &[0x2a, 0x86, 0x48, 0xce, 0x3d, 0x04, 0x03, 0x02];
const EC_PUBLIC_KEY: &[u8] = &[0x2a, 0x86, 0x48, 0xce, 0x3d, 0x02, 0x01];
const PRIME256V1: &[u8] = &[0x2a, 0x86, 0x48, 0xce, 0x3d, 0x03, 0x01, 0x07];
const COMMON_NAME: &[u8] = &[0x55, 0x04, 0x03];

fn ecdsa_der(sig: &Signature) -> Vec<u8> {
    let (r, s) = sig.split_bytes();
    seq(&[uint(&r), uint(&s)])
}

fn public_key_info(k: &SigningKey) -> Vec<u8> {
    let point = k.verifying_key().to_encoded_point(false);
    let mut bits = vec![0u8];
    bits.extend_from_slice(point.as_bytes());
    seq(&[seq(&[oid(EC_PUBLIC_KEY), oid(PRIME256V1)]), der(0x03, &bits)])
}

fn certificate(k: &SigningKey, name: &str) -> Vec<u8> {
    let alg = seq(&[oid(ECDSA_WITH_SHA256)]);
    let dn = seq(&[der(0x31, &seq(&[oid(COMMON_NAME), der(0x0c, name.as_bytes())]))]);
    let mut serial = [0u8; 8];
    serial.copy_from_slice(&Sha256::digest(k.verifying_key().to_encoded_point(false).as_bytes())[..8]);
    serial[0] &= 0x7f;
    let validity = seq(&[der(0x17, b"260101000000Z"), der(0x18, b"21251231235959Z")]);
    let tbs = seq(&[
        der(0xa0, &uint(&[2])),
        uint(&serial),
        alg.clone(),
        dn.clone(),
        validity,
        dn,
        public_key_info(k),
    ]);
    let sig: Signature = k.sign(&tbs);
    let mut bits = vec![0u8];
    bits.extend_from_slice(&ecdsa_der(&sig));
    seq(&[tbs, alg, der(0x03, &bits)])
}

// --- APK Signature Scheme v2

fn lp(b: &[u8]) -> Vec<u8> {
    let mut out = (b.len() as u32).to_le_bytes().to_vec();
    out.extend_from_slice(b);
    out
}

/// The v2 digest of the three signed sections: 1 MB chunks, each hashed with a 0xa5 prefix and
/// its length, then the chunk digests hashed with a 0x5a prefix and their count.
fn content_digest(sections: &[&[u8]]) -> Vec<u8> {
    let mut chunks = Vec::new();
    for s in sections {
        for c in s.chunks(CHUNK) {
            let mut h = Sha256::new();
            h.update([0xa5]);
            h.update((c.len() as u32).to_le_bytes());
            h.update(c);
            chunks.push(h.finalize());
        }
    }
    let mut h = Sha256::new();
    h.update([0x5a]);
    h.update((chunks.len() as u32).to_le_bytes());
    for c in &chunks {
        h.update(c);
    }
    h.finalize().to_vec()
}

/// Signs an aligned, unsigned APK: inserts the APK Signing Block before the central directory.
pub fn sign_v2(apk: &[u8], key: &Key) -> Result<Vec<u8>, String> {
    let e = crate::zip::eocd(apk)?;
    let cd_at = u32::from_le_bytes(apk[e + 16..e + 20].try_into().unwrap()) as usize;
    let entries = &apk[..cd_at];
    let central = &apk[cd_at..e];
    // The EOCD is digested as it will point at the signing block, which starts where the central
    // directory starts now.
    let eocd = apk[e..].to_vec();
    let digest = content_digest(&[entries, central, &eocd]);

    let digests = lp(&lp(&[SIG_ECDSA_SHA256.to_le_bytes().to_vec(), lp(&digest)].concat()));
    let certs = lp(&lp(&key.cert));
    let attrs = lp(&[]);
    let signed_data = [digests, certs, attrs].concat();
    let signature = key.sign(&signed_data);
    let signatures = lp(&lp(&[SIG_ECDSA_SHA256.to_le_bytes().to_vec(), lp(&signature)].concat()));
    let public = lp(&public_key_info(&key.key));
    let signer = [lp(&signed_data), signatures, public].concat();
    let v2 = lp(&lp(&signer));

    let mut pair = ((4 + v2.len()) as u64).to_le_bytes().to_vec();
    pair.extend_from_slice(&V2_BLOCK_ID.to_le_bytes());
    pair.extend_from_slice(&v2);
    let size = (pair.len() + 8 + 16) as u64;
    let mut block = size.to_le_bytes().to_vec();
    block.extend_from_slice(&pair);
    block.extend_from_slice(&size.to_le_bytes());
    block.extend_from_slice(b"APK Sig Block 42");

    let mut out = Vec::with_capacity(apk.len() + block.len());
    out.extend_from_slice(entries);
    out.extend_from_slice(&block);
    out.extend_from_slice(central);
    let mut eocd = eocd;
    eocd[16..20].copy_from_slice(&((cd_at + block.len()) as u32).to_le_bytes());
    out.extend_from_slice(&eocd);
    Ok(out)
}
