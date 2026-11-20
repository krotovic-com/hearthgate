//! WireGuard keys: Curve25519, written as base64 like `wg genkey`.

use base64::Engine as _;
use base64::engine::general_purpose::STANDARD;
use x25519_dalek::{PublicKey, StaticSecret};

/// A private key and its public key, both in base64.
#[derive(Clone, PartialEq, Eq)]
pub struct KeyPair {
    pub private: String,
    pub public: String,
}

impl std::fmt::Debug for KeyPair {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("KeyPair")
            .field("public", &self.public)
            .finish_non_exhaustive()
    }
}

/// A new random key pair.
pub fn generate() -> Result<KeyPair, getrandom::Error> {
    let mut bytes = [0u8; 32];
    getrandom::fill(&mut bytes)?;
    // Clamped like `wg genkey`, so the stored key reads the same in every
    // WireGuard implementation.
    bytes[0] &= 0b1111_1000;
    bytes[31] &= 0b0111_1111;
    bytes[31] |= 0b0100_0000;
    let secret = StaticSecret::from(bytes);
    bytes.fill(0);
    Ok(KeyPair {
        private: STANDARD.encode(secret.to_bytes()),
        public: STANDARD.encode(PublicKey::from(&secret).as_bytes()),
    })
}

/// The public key for a base64 private key.
#[must_use]
pub fn public_key(private: &str) -> Option<String> {
    let bytes: [u8; 32] = STANDARD.decode(private.trim()).ok()?.try_into().ok()?;
    let secret = StaticSecret::from(bytes);
    Some(STANDARD.encode(PublicKey::from(&secret).as_bytes()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn matches_wg_pubkey() {
        // From `echo ... | wg pubkey`.
        assert_eq!(
            public_key("YAnz5TF+lXXJte14tji3zlMNq+hd2rYUIgJBgB3fBmk=").as_deref(),
            Some("Y2FD6uTgrq+/bbBAXCNOtE+PGk8Papoh7qaWBpQUXn0=")
        );
        let pair = generate().unwrap();
        assert!(hearthgate_model::is_wireguard_key(&pair.private));
        assert!(hearthgate_model::is_wireguard_key(&pair.public));
        assert_eq!(public_key(&pair.private), Some(pair.public.clone()));
        assert!(!format!("{pair:?}").contains(&pair.private));
    }
}
