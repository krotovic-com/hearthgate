//! VPN state shared between the daemon, which runs the interface, and the
//! admin API, which shows it and creates peer configs.

use std::collections::BTreeMap;
use std::io::Write as _;
use std::net::IpAddr;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, PoisonError};

use ipnet::IpNet;

use crate::{Backend, Device, INTERFACE, Tools, keys};

/// What the API shows and needs.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Info {
    /// The interface is up.
    pub running: bool,
    pub backend: Option<Backend>,
    /// This machine's public key, for peer configs.
    pub public_key: Option<String>,
    /// Problems to show, such as a missing `wg` tool or forwarding off.
    pub problems: Vec<String>,
    /// Service addresses only the running gateway knows (containers,
    /// resolved host names).
    pub resolved: BTreeMap<String, Vec<IpAddr>>,
    /// This machine's LAN and container networks, for full-access peer
    /// routes.
    pub lan: Vec<IpNet>,
}

/// The VPN interface and its state.
#[derive(Debug)]
pub struct Runtime {
    pub device: Device,
    /// Where this machine's key lives; `None` turns the VPN off (tests).
    key_dir: Option<PathBuf>,
    info: Mutex<Info>,
}

impl Default for Runtime {
    fn default() -> Self {
        Self::new(Device::new(INTERFACE, Tools::default()), None)
    }
}

impl Runtime {
    #[must_use]
    pub fn new(device: Device, key_dir: Option<PathBuf>) -> Self {
        Self {
            device,
            key_dir,
            info: Mutex::default(),
        }
    }

    /// This machine's private key, created on first use.
    pub fn private_key(&self) -> std::io::Result<String> {
        match &self.key_dir {
            Some(dir) => load_or_create_key(dir),
            None => Err(std::io::Error::new(
                std::io::ErrorKind::NotFound,
                "the VPN has no key directory",
            )),
        }
    }

    /// This machine's public key, for peer configs.
    pub fn public_key(&self) -> std::io::Result<String> {
        let private = self.private_key()?;
        keys::public_key(&private)
            .ok_or_else(|| std::io::Error::new(std::io::ErrorKind::InvalidData, "bad key"))
    }

    #[must_use]
    pub fn info(&self) -> Info {
        self.info
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
    }

    pub fn update(&self, f: impl FnOnce(&mut Info)) {
        f(&mut self.info.lock().unwrap_or_else(PoisonError::into_inner));
    }
}

/// Reads this machine's private key from `dir/private.key`, creating it on
/// first use. Only the owner can read it.
pub fn load_or_create_key(dir: &Path) -> std::io::Result<String> {
    let path = dir.join("private.key");
    if let Ok(text) = std::fs::read_to_string(&path) {
        let key = text.trim().to_owned();
        if keys::public_key(&key).is_some() {
            return Ok(key);
        }
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            format!("{} is not a WireGuard private key", path.display()),
        ));
    }
    std::fs::create_dir_all(dir)?;
    let pair = keys::generate().map_err(|e| std::io::Error::other(e.to_string()))?;
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt as _;
        options.mode(0o600);
    }
    let mut file = options.open(&path)?;
    writeln!(file, "{}", pair.private)?;
    file.sync_all()?;
    Ok(pair.private)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn creates_the_key_once() {
        let dir = tempfile::tempdir().unwrap();
        let first = load_or_create_key(&dir.path().join("vpn")).unwrap();
        let again = load_or_create_key(&dir.path().join("vpn")).unwrap();
        assert_eq!(first, again);
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt as _;
            let mode = std::fs::metadata(dir.path().join("vpn/private.key"))
                .unwrap()
                .permissions()
                .mode();
            assert_eq!(mode & 0o777, 0o600);
        }
        std::fs::write(dir.path().join("vpn/private.key"), "garbage").unwrap();
        assert!(load_or_create_key(&dir.path().join("vpn")).is_err());
    }
}
