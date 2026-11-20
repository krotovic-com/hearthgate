//! Hearthgate: a secure-by-default gateway for home servers.
//!
//! This crate is at the skeleton stage. Proxying, certificates, the VPN,
//! the firewall and the dashboard land in later releases.

/// Version of this build.
///
/// Release builds get it from `HEARTHGATE_VERSION`, which the release
/// workflow computes from Conventional Commits. Local builds fall back to the
/// placeholder version in `Cargo.toml`.
pub const VERSION: &str = match option_env!("HEARTHGATE_VERSION") {
    Some(version) => version,
    None => env!("CARGO_PKG_VERSION"),
};

/// Returns the one-line banner printed at startup.
#[must_use]
pub fn banner() -> String {
    format!("hearthgate {VERSION}: one way in")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn banner_contains_version() {
        assert!(banner().contains(VERSION));
        assert!(banner().starts_with("hearthgate "));
    }
}
