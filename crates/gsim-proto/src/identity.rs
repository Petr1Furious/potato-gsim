//! Player identity: an Ed25519 key pair made on the player's machine. The public key is the
//! identity; a server ties a name to the first key that uses it and asks every joiner to
//! sign a fresh random challenge. No accounts, no passwords, nothing to leak from a server.

use ed25519_dalek::{Signature, Signer, SigningKey, Verifier, VerifyingKey};

const CONTEXT: &[u8] = b"potato-gsim join v1";

fn message(nonce: &[u8; 32], name: &str) -> Vec<u8> {
    let mut m = Vec::with_capacity(CONTEXT.len() + 32 + name.len());
    m.extend_from_slice(CONTEXT);
    m.extend_from_slice(nonce);
    m.extend_from_slice(name.as_bytes());
    m
}

#[derive(Clone)]
pub struct Identity {
    key: SigningKey,
}

impl Identity {
    /// A fresh random identity.
    pub fn generate() -> Self {
        Self::from_secret(random_bytes())
    }

    pub fn from_secret(secret: [u8; 32]) -> Self {
        Self { key: SigningKey::from_bytes(&secret) }
    }

    /// The secret half: store it privately, never send it anywhere.
    pub fn secret(&self) -> [u8; 32] {
        self.key.to_bytes()
    }

    pub fn public(&self) -> [u8; 32] {
        self.key.verifying_key().to_bytes()
    }

    /// Answer a server's challenge for joining as `name`.
    pub fn sign(&self, nonce: &[u8; 32], name: &str) -> Vec<u8> {
        self.key.sign(&message(nonce, name)).to_bytes().to_vec()
    }
}

/// Does `signature` prove that the holder of `public` wants to join as `name` right now?
pub fn verify(public: &[u8; 32], nonce: &[u8; 32], name: &str, signature: &[u8]) -> bool {
    let (Ok(key), Ok(sig)) = (VerifyingKey::from_bytes(public), Signature::from_slice(signature)) else { return false };
    key.verify(&message(nonce, name), &sig).is_ok()
}

pub fn random_bytes() -> [u8; 32] {
    let mut b = [0u8; 32];
    getrandom::getrandom(&mut b).expect("the operating system provides randomness");
    b
}

pub fn to_hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

pub fn from_hex(s: &str) -> Option<[u8; 32]> {
    let s = s.trim();
    if s.len() != 64 {
        return None;
    }
    let mut out = [0u8; 32];
    for (i, byte) in out.iter_mut().enumerate() {
        *byte = u8::from_str_radix(s.get(2 * i..2 * i + 2)?, 16).ok()?;
    }
    Some(out)
}

/// Short form of a public key for logs and UIs.
pub fn fingerprint(public: &[u8; 32]) -> String {
    to_hex(&public[..4])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn signatures_bind_key_nonce_and_name() {
        let (a, b) = (Identity::generate(), Identity::generate());
        let nonce = random_bytes();
        let sig = a.sign(&nonce, "ann");
        assert!(verify(&a.public(), &nonce, "ann", &sig));
        assert!(!verify(&b.public(), &nonce, "ann", &sig), "another key");
        assert!(!verify(&a.public(), &random_bytes(), "ann", &sig), "another challenge (replay)");
        assert!(!verify(&a.public(), &nonce, "bob", &sig), "another name");
        assert!(!verify(&a.public(), &nonce, "ann", &sig[..63]), "truncated");
        let again = Identity::from_secret(a.secret());
        assert_eq!(again.public(), a.public());
        assert_eq!(from_hex(&to_hex(&a.public())), Some(a.public()));
    }
}

impl Identity {
    /// A key derived from a label. Anyone who knows the label can recreate it, so this is
    /// only for bots and tests, never for people.
    pub fn insecure_from_label(label: &str) -> Self {
        let mut secret = [0u8; 32];
        let mut h: u64 = 0xcbf2_9ce4_8422_2325;
        for (i, slot) in secret.iter_mut().enumerate() {
            for b in label.bytes().chain([i as u8]) {
                h = (h ^ b as u64).wrapping_mul(0x0000_0100_0000_01b3);
            }
            *slot = (h >> 32) as u8;
        }
        Self::from_secret(secret)
    }
}

/// Never prints the secret.
impl std::fmt::Debug for Identity {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Identity({})", fingerprint(&self.public()))
    }
}
