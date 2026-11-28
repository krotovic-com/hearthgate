//! The keys releases are signed with.
//!
//! Only signatures from these keys are accepted. To rotate, add the new key
//! here, release, and only then start signing with it.

/// Minisign public keys, base64.
pub const RELEASE_KEYS: &[&str] = &["RWQDbPNZZtyDzLtuWAw4DNX7OGSMq0cMIjTU24h2jCWHE5yliB4enY2M"];

/// The release keys, parsed.
#[must_use]
pub fn release_keys() -> Vec<minisign_verify::PublicKey> {
    RELEASE_KEYS
        .iter()
        .map(|k| minisign_verify::PublicKey::from_base64(k).expect("release keys are valid"))
        .collect()
}

#[cfg(test)]
mod tests {
    #[test]
    fn release_keys_parse() {
        assert_eq!(super::release_keys().len(), super::RELEASE_KEYS.len());
    }
}
