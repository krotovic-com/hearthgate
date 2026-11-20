//! The WireGuard interface, driven with the standard tools: `ip` for the
//! link and address, `wg` for keys and peers. The kernel module is used
//! when the kernel has it; otherwise `wireguard-go` provides the same
//! interface in user space.

use std::net::{IpAddr, SocketAddr};
use std::path::Path;
use std::process::Stdio;
use std::sync::Mutex;
use std::time::Duration;

use ipnet::{IpNet, Ipv4Net};
use tokio::io::AsyncWriteExt;
use tokio::process::{Child, Command};
use tracing::{info, warn};

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("`{command}` failed: {message}")]
    Command { command: String, message: String },
    #[error(
        "this kernel has no WireGuard module and `wireguard-go` is not installed; install the \
         `wireguard-go` package, or a kernel with WireGuard (Linux 5.6 or later)"
    )]
    NoBackend,
    #[error("`{0}` is not installed")]
    Missing(String),
    #[error(transparent)]
    Io(#[from] std::io::Error),
}

/// The programs used, as command prefixes, so tests can run them inside a
/// network namespace (`ip netns exec NS wg`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Tools {
    pub ip: Vec<String>,
    pub wg: Vec<String>,
    pub userspace: Vec<String>,
}

impl Default for Tools {
    fn default() -> Self {
        Self {
            ip: vec![find("ip")],
            wg: vec![find("wg")],
            userspace: vec![find("wireguard-go")],
        }
    }
}

/// Looks in the usual places, since services often run with a short PATH.
fn find(program: &str) -> String {
    for dir in [
        "/usr/sbin",
        "/usr/bin",
        "/sbin",
        "/bin",
        "/usr/local/sbin",
        "/usr/local/bin",
    ] {
        let path = Path::new(dir).join(program);
        if path.exists() {
            return path.to_string_lossy().into_owned();
        }
    }
    program.to_owned()
}

/// How the interface is provided.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Backend {
    Kernel,
    Userspace,
}

/// One peer as the interface sees it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PeerConfig {
    pub public_key: String,
    pub allowed_ips: Vec<IpNet>,
}

/// Live state of one peer, from `wg show`.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct PeerStatus {
    pub public_key: String,
    pub endpoint: Option<SocketAddr>,
    /// Unix seconds of the last handshake.
    pub latest_handshake: Option<u64>,
    pub rx_bytes: u64,
    pub tx_bytes: u64,
}

/// The interface's settings.
#[derive(Clone, PartialEq, Eq)]
pub struct Setup {
    pub private_key: String,
    pub port: u16,
    /// This machine's address and the VPN's prefix, such as 10.88.0.1/24.
    pub address: Ipv4Net,
    pub peers: Vec<PeerConfig>,
}

impl std::fmt::Debug for Setup {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Setup")
            .field("port", &self.port)
            .field("address", &self.address)
            .field("peers", &self.peers)
            .finish_non_exhaustive()
    }
}

/// A WireGuard interface Hearthgate owns.
#[derive(Debug)]
pub struct Device {
    pub name: String,
    tools: Tools,
    userspace: Mutex<Option<Child>>,
}

impl Device {
    #[must_use]
    pub fn new(name: &str, tools: Tools) -> Self {
        Self {
            name: name.to_owned(),
            tools,
            userspace: Mutex::new(None),
        }
    }

    async fn run(
        &self,
        tool: &[String],
        args: &[&str],
        stdin: Option<&str>,
    ) -> Result<String, Error> {
        let (program, prefix) = tool.split_first().expect("a command");
        let mut cmd = Command::new(program);
        cmd.args(prefix)
            .args(args)
            .stdin(if stdin.is_some() {
                Stdio::piped()
            } else {
                Stdio::null()
            })
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true);
        let mut child = cmd.spawn().map_err(|e| {
            if e.kind() == std::io::ErrorKind::NotFound {
                Error::Missing(program.clone())
            } else {
                Error::Io(e)
            }
        })?;
        if let (Some(text), Some(mut pipe)) = (stdin, child.stdin.take()) {
            pipe.write_all(text.as_bytes()).await?;
            drop(pipe);
        }
        let out = tokio::time::timeout(Duration::from_secs(10), child.wait_with_output())
            .await
            .map_err(|_| Error::Command {
                command: describe(tool, args),
                message: "timed out".into(),
            })??;
        if out.status.success() {
            Ok(String::from_utf8_lossy(&out.stdout).into_owned())
        } else {
            Err(Error::Command {
                command: describe(tool, args),
                message: String::from_utf8_lossy(&out.stderr).trim().to_owned(),
            })
        }
    }

    /// Whether the interface exists.
    pub async fn exists(&self) -> bool {
        self.run(&self.tools.ip, &["link", "show", "dev", &self.name], None)
            .await
            .is_ok()
    }

    /// Whether the interface is the kernel's or `wireguard-go`'s.
    pub fn backend(&self) -> Backend {
        if self
            .userspace
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .is_some()
        {
            Backend::Userspace
        } else {
            Backend::Kernel
        }
    }

    async fn create(&self) -> Result<Backend, Error> {
        match self
            .run(
                &self.tools.ip,
                &["link", "add", "dev", &self.name, "type", "wireguard"],
                None,
            )
            .await
        {
            Ok(_) => {
                info!(interface = %self.name, "WireGuard interface created (kernel)");
                return Ok(Backend::Kernel);
            }
            Err(e) => info!("no kernel WireGuard ({e}); trying wireguard-go"),
        }
        let (program, prefix) = self.tools.userspace.split_first().expect("a command");
        let child = Command::new(program)
            .args(prefix)
            .args(["-f", &self.name])
            .env("WG_PROCESS_FOREGROUND", "1")
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::inherit())
            .kill_on_drop(true)
            .spawn()
            .map_err(|e| {
                if e.kind() == std::io::ErrorKind::NotFound {
                    Error::NoBackend
                } else {
                    Error::Io(e)
                }
            })?;
        *self
            .userspace
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(child);
        for _ in 0..50 {
            if self.exists().await {
                // The control socket appears just after the link.
                tokio::time::sleep(Duration::from_millis(100)).await;
                info!(interface = %self.name, "WireGuard interface created (wireguard-go)");
                return Ok(Backend::Userspace);
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
        self.stop_userspace().await;
        Err(Error::Command {
            command: describe(&self.tools.userspace, &["-f", &self.name]),
            message: "the interface did not appear".into(),
        })
    }

    async fn stop_userspace(&self) {
        let child = self
            .userspace
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .take();
        if let Some(mut child) = child {
            let _ = child.kill().await;
        }
    }

    /// Creates the interface if needed and brings it in line with `setup`.
    /// Peers that stay keep their sessions.
    pub async fn apply(&self, setup: &Setup) -> Result<(), Error> {
        let userspace_died = {
            let mut guard = self
                .userspace
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            match guard.as_mut().map(Child::try_wait) {
                Some(Ok(Some(status))) => {
                    warn!("wireguard-go exited ({status}); starting it again");
                    *guard = None;
                    true
                }
                _ => false,
            }
        };
        if userspace_died || !self.exists().await {
            self.create().await?;
        }
        self.run(
            &self.tools.wg,
            &["syncconf", &self.name, "/dev/stdin"],
            Some(&render(setup)),
        )
        .await?;
        self.run(
            &self.tools.ip,
            &[
                "address",
                "replace",
                &setup.address.to_string(),
                "dev",
                &self.name,
            ],
            None,
        )
        .await?;
        // Drop any other address left from an earlier subnet.
        let addresses = self
            .run(
                &self.tools.ip,
                &["-o", "-4", "address", "show", "dev", &self.name],
                None,
            )
            .await
            .unwrap_or_default();
        for line in addresses.lines() {
            if let Some(addr) = line.split_whitespace().skip_while(|w| *w != "inet").nth(1) {
                if addr != setup.address.to_string() {
                    let _ = self
                        .run(
                            &self.tools.ip,
                            &["address", "del", addr, "dev", &self.name],
                            None,
                        )
                        .await;
                }
            }
        }
        self.run(
            &self.tools.ip,
            &["link", "set", "dev", &self.name, "mtu", "1420", "up"],
            None,
        )
        .await?;
        Ok(())
    }

    /// Removes the interface.
    pub async fn remove(&self) -> Result<(), Error> {
        let existed = self.exists().await;
        if existed {
            self.run(&self.tools.ip, &["link", "del", "dev", &self.name], None)
                .await?;
        }
        self.stop_userspace().await;
        if existed {
            info!(interface = %self.name, "WireGuard interface removed");
        }
        Ok(())
    }

    /// Peers with their last handshake and traffic.
    pub async fn peers(&self) -> Result<Vec<PeerStatus>, Error> {
        let dump = self
            .run(&self.tools.wg, &["show", &self.name, "dump"], None)
            .await?;
        Ok(parse_dump(&dump))
    }
}

fn describe(tool: &[String], args: &[&str]) -> String {
    let mut parts: Vec<&str> = tool.iter().map(String::as_str).collect();
    parts.extend_from_slice(args);
    parts.join(" ")
}

/// The configuration `wg syncconf` reads.
fn render(setup: &Setup) -> String {
    use std::fmt::Write as _;
    let mut out = format!(
        "[Interface]\nPrivateKey = {}\nListenPort = {}\n",
        setup.private_key, setup.port
    );
    for peer in &setup.peers {
        let ips: Vec<String> = peer.allowed_ips.iter().map(ToString::to_string).collect();
        let _ = write!(
            out,
            "\n[Peer]\nPublicKey = {}\nAllowedIPs = {}\n",
            peer.public_key,
            ips.join(", ")
        );
    }
    out
}

/// Parses `wg show IFACE dump`: a line for the interface, then one per peer
/// with tab-separated public key, preshared key, endpoint, allowed IPs,
/// latest handshake, received bytes, sent bytes and keepalive.
fn parse_dump(dump: &str) -> Vec<PeerStatus> {
    dump.lines()
        .skip(1)
        .filter_map(|line| {
            let f: Vec<&str> = line.split('\t').collect();
            if f.len() < 8 {
                return None;
            }
            Some(PeerStatus {
                public_key: f[0].to_owned(),
                endpoint: f[2].parse().ok(),
                latest_handshake: f[4].parse().ok().filter(|t: &u64| *t > 0),
                rx_bytes: f[5].parse().unwrap_or(0),
                tx_bytes: f[6].parse().unwrap_or(0),
            })
        })
        .collect()
}

/// Whether IPv4 forwarding is on, which peers need to reach anything
/// besides this machine.
#[must_use]
pub fn forwarding_enabled() -> bool {
    std::fs::read_to_string("/proc/sys/net/ipv4/ip_forward").is_ok_and(|v| v.trim() == "1")
}

/// Turns IPv4 forwarding on. Fails where `/proc/sys` is read-only, as under
/// the systemd unit, which ships a sysctl file for it instead.
pub fn enable_forwarding() -> std::io::Result<()> {
    std::fs::write("/proc/sys/net/ipv4/ip_forward", "1\n")
}

/// An address is inside the VPN.
#[must_use]
pub fn in_subnet(subnet: Ipv4Net, ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(v4) => subnet.contains(&v4),
        IpAddr::V6(_) => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn renders_and_parses() {
        let setup = Setup {
            private_key: "priv".into(),
            port: 51820,
            address: "10.88.0.1/24".parse().unwrap(),
            peers: vec![PeerConfig {
                public_key: "pub".into(),
                allowed_ips: vec!["10.88.0.2/32".parse().unwrap()],
            }],
        };
        assert_eq!(
            render(&setup),
            "[Interface]\nPrivateKey = priv\nListenPort = 51820\n\n[Peer]\nPublicKey = pub\nAllowedIPs = 10.88.0.2/32\n"
        );
        assert!(!format!("{setup:?}").contains("priv"));
        let dump = "priv\tpub\t51820\toff\n\
                    AAA=\t(none)\t198.51.100.4:4500\t10.88.0.2/32\t1700000000\t1024\t2048\toff\n\
                    BBB=\t(none)\t(none)\t10.88.0.3/32\t0\t0\t0\toff\n";
        let peers = parse_dump(dump);
        assert_eq!(peers.len(), 2);
        assert_eq!(peers[0].endpoint, "198.51.100.4:4500".parse().ok());
        assert_eq!(peers[0].latest_handshake, Some(1_700_000_000));
        assert_eq!(peers[0].tx_bytes, 2048);
        assert_eq!(peers[1].endpoint, None);
        assert_eq!(peers[1].latest_handshake, None);
    }
}
