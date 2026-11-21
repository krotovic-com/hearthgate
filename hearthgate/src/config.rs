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
    /// Scheduled backups.
    #[serde(default)]
    pub backup: BackupSettings,
    /// Checking for new releases.
    #[serde(default)]
    pub updates: Updates,
}

/// Release checks.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Updates {
    /// Ask GitHub once a day whether a newer release exists, and log and
    /// notify when it does. Nothing is ever installed by itself.
    #[serde(default = "default_true")]
    pub check: bool,
}

impl Default for Updates {
    fn default() -> Self {
        Self { check: true }
    }
}

impl Default for Bootstrap {
    fn default() -> Self {
        Self {
            data_dir: default_data_dir(),
            log: default_log(),
            admin: Admin::default(),
            docker: Docker::default(),
            backup: BackupSettings::default(),
            updates: Updates::default(),
        }
    }
}

/// Admin API settings.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Admin {
    /// Addresses to listen on. When neither this nor `allow` is set,
    /// Hearthgate listens on loopback and this machine's LAN addresses
    /// (port 7380), and the dashboard can change that. Setting it here
    /// fixes the list; empty disables the API.
    #[serde(default)]
    pub listen: Option<Vec<SocketAddr>>,
    /// Client ranges allowed to connect. Everyone else is disconnected
    /// before anything is read, even if `listen` is misconfigured. When
    /// unset, loopback and the LAN networks Hearthgate finds.
    #[serde(default)]
    pub allow: Option<Vec<IpNet>>,
    /// Serve `/metrics` without the admin token (still only to `allow`).
    #[serde(default)]
    pub metrics_without_token: bool,
    /// Serve the dashboard and API over HTTPS. Turn off only behind another
    /// TLS proxy on the same machine.
    #[serde(default = "default_true")]
    pub https: bool,
    /// Certificate (PEM, full chain) for the dashboard, for example from an
    /// internal CA. Without it, a self-signed one is made and kept.
    #[serde(default)]
    pub certificate: Option<PathBuf>,
    /// Private key (PEM) for `certificate`.
    #[serde(default)]
    pub private_key: Option<PathBuf>,
}

impl Default for Admin {
    fn default() -> Self {
        Self {
            listen: None,
            allow: None,
            metrics_without_token: false,
            https: true,
            certificate: None,
            private_key: None,
        }
    }
}

/// Default admin port.
pub const ADMIN_PORT: u16 = 7380;

impl Admin {
    /// Whether the bootstrap file fixes the listen addresses or ranges.
    #[must_use]
    pub fn fixed(&self) -> bool {
        self.listen.is_some() || self.allow.is_some()
    }

    /// Whether the admin API is turned off (`listen = []`).
    #[must_use]
    pub fn disabled(&self) -> bool {
        self.listen.as_ref().is_some_and(Vec::is_empty)
    }

    /// Listen addresses, with the default for an unset value.
    #[must_use]
    pub fn listen_or_default(&self) -> Vec<SocketAddr> {
        self.listen.clone().unwrap_or_else(default_admin_listen)
    }

    /// Allowed ranges, with the default for an unset value.
    #[must_use]
    pub fn allow_or_default(&self) -> Vec<IpNet> {
        self.allow.clone().unwrap_or_else(default_admin_allow)
    }
}

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
    /// Run the containers and stacks of the desired state. Off: Hearthgate
    /// only follows existing containers.
    #[serde(default = "default_true")]
    pub manage: bool,
}

impl Default for Docker {
    fn default() -> Self {
        Self {
            enabled: true,
            socket: default_docker_socket(),
            manage: true,
        }
    }
}

/// Scheduled backups (see `hearthgate backup`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BackupSettings {
    /// Write backups on a schedule.
    #[serde(default = "default_true")]
    pub enabled: bool,
    /// Where to write them. Default: `backups` in the data directory. A
    /// directory elsewhere must be writable by the service (systemd:
    /// `ReadWritePaths=`).
    #[serde(default)]
    pub dir: Option<PathBuf>,
    /// How often.
    #[serde(
        default = "default_backup_interval",
        with = "hearthgate_model::duration::serde"
    )]
    pub interval: std::time::Duration,
    /// How many to keep; older ones are removed.
    #[serde(default = "default_backup_keep")]
    pub keep: usize,
}

impl Default for BackupSettings {
    fn default() -> Self {
        Self {
            enabled: true,
            dir: None,
            interval: default_backup_interval(),
            keep: default_backup_keep(),
        }
    }
}

impl BackupSettings {
    #[must_use]
    pub fn dir_or_default(&self, data_dir: &Path) -> PathBuf {
        self.dir.clone().unwrap_or_else(|| data_dir.join("backups"))
    }
}

fn default_backup_interval() -> std::time::Duration {
    std::time::Duration::from_secs(86_400)
}

fn default_backup_keep() -> usize {
    7
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
