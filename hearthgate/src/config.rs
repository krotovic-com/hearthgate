//! Bootstrap settings from `/etc/hearthgate/hearthgate.toml`.
//!
//! This file holds only what Hearthgate needs before it can open its state
//! database. Interfaces, services and routes live in the database and are
//! changed with `hearthgate apply`, never here.

use std::net::SocketAddr;
use std::path::{Path, PathBuf};

use ipnet::IpNet;

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

/// Default location of the bootstrap file.
pub const DEFAULT_CONFIG: &str = "/etc/hearthgate/hearthgate.toml";
/// Default data directory (systemd `StateDirectory=hearthgate`).
pub const DEFAULT_DATA_DIR: &str = "/var/lib/hearthgate";

/// Bootstrap settings.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Bootstrap {
    /// Where the state database, certificates and keys are kept.
    #[serde(default = "default_data_dir")]
    pub data_dir: PathBuf,
    /// Log filter, for example `info` or `hearthgate=debug,info`.
    #[serde(default = "default_log")]
    pub log: String,
    /// Admin API.
    #[serde(default)]
    pub admin: Admin,
    /// Docker discovery for `kind = "docker"` services.
    #[serde(default)]
    pub docker: Docker,
}

impl Default for Bootstrap {
    fn default() -> Self {
        Self {
            data_dir: default_data_dir(),
            log: default_log(),
            admin: Admin::default(),
            docker: Docker::default(),
        }
    }
}

/// Admin API settings.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Admin {
    /// Addresses to listen on. Loopback only by default; add the server's
    /// LAN address to reach it from other machines. Empty disables the API.
    #[serde(default = "default_admin_listen")]
    pub listen: Vec<SocketAddr>,
    /// Client ranges allowed to connect. Everyone else is disconnected
    /// before anything is read, even if `listen` is misconfigured.
    #[serde(default = "default_admin_allow")]
    pub allow: Vec<IpNet>,
    /// Serve `/metrics` without the admin token (still only to `allow`).
    #[serde(default)]
    pub metrics_without_token: bool,
}

impl Default for Admin {
    fn default() -> Self {
        Self {
            listen: default_admin_listen(),
            allow: default_admin_allow(),
            metrics_without_token: false,
        }
    }
}

/// Default admin port.
pub const ADMIN_PORT: u16 = 7380;

fn default_admin_listen() -> Vec<SocketAddr> {
    vec![SocketAddr::from(([127, 0, 0, 1], ADMIN_PORT))]
}

/// Loopback and private ranges (RFC 1918, ULA, link-local).
fn default_admin_allow() -> Vec<IpNet> {
    [
        "127.0.0.0/8",
        "::1/128",
        "10.0.0.0/8",
        "172.16.0.0/12",
        "192.168.0.0/16",
        "fc00::/7",
        "fe80::/10",
    ]
    .iter()
    .map(|n| n.parse().expect("valid default range"))
    .collect()
}

/// Docker discovery settings.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Docker {
    /// Watch Docker when its socket exists.
    #[serde(default = "default_true")]
    pub enabled: bool,
    /// Path of the Docker socket.
    #[serde(default = "default_docker_socket")]
    pub socket: PathBuf,
}

impl Default for Docker {
    fn default() -> Self {
        Self {
            enabled: true,
            socket: default_docker_socket(),
        }
    }
}

fn default_true() -> bool {
    true
}

fn default_docker_socket() -> PathBuf {
    PathBuf::from("/var/run/docker.sock")
}

fn default_data_dir() -> PathBuf {
    PathBuf::from(DEFAULT_DATA_DIR)
}

fn default_log() -> String {
    "info".to_owned()
}

impl Bootstrap {
    /// Loads the bootstrap file. When no path was given and the default file
    /// does not exist, the defaults are used. A path that was given must
    /// exist, so a typo never silently falls back to defaults.
    pub fn load(path: Option<&Path>) -> Result<Self> {
        let (path, explicit) = match path {
            Some(p) => (p, true),
            None => (Path::new(DEFAULT_CONFIG), false),
        };
        match std::fs::read_to_string(path) {
            Ok(text) => toml::from_str(&text)
                .with_context(|| format!("invalid bootstrap config {}", path.display())),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound && !explicit => Ok(Self::default()),
            Err(e) => Err(e).with_context(|| format!("cannot read {}", path.display())),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_when_default_file_is_missing() {
        // The default path does not exist in test environments.
        if !Path::new(DEFAULT_CONFIG).exists() {
            assert_eq!(Bootstrap::load(None).unwrap(), Bootstrap::default());
        }
    }

    #[test]
    fn explicit_missing_file_is_an_error() {
        assert!(Bootstrap::load(Some(Path::new("/nonexistent/hearthgate.toml"))).is_err());
    }

    #[test]
    fn example_file_parses() {
        let text = include_str!("../../packaging/hearthgate.toml");
        let parsed: Bootstrap = toml::from_str(text).unwrap();
        assert_eq!(parsed.admin, Admin::default());
        assert_eq!(parsed.docker, Docker::default());
    }

    #[test]
    fn parses_and_rejects_unknown_keys() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("h.toml");
        std::fs::write(&file, "data_dir = \"/tmp/x\"\n").unwrap();
        assert_eq!(
            Bootstrap::load(Some(&file)).unwrap().data_dir,
            PathBuf::from("/tmp/x")
        );
        std::fs::write(&file, "datadir = \"/tmp/x\"\n").unwrap();
        assert!(Bootstrap::load(Some(&file)).is_err());
    }
}
