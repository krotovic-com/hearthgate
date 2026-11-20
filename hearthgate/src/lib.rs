//! Hearthgate: a secure-by-default gateway for home servers.
//!
//! This crate is the `hearthgate` binary: the command line, the bootstrap
//! settings and the daemon that wires the subsystems together. The
//! subsystems themselves live in the `hearthgate-*` crates.

pub mod cli;
pub mod config;
pub mod daemon;
pub mod firewall;

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
