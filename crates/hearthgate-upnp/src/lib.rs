//! Router integration: keeps the router forwarding each public port to this
//! machine, with UPnP IGD or NAT-PMP, and removes the forwards when ports go
//! away or Hearthgate stops.
//!
//! Off by default. Many routers ship with UPnP on, and any program on the
//! LAN can use it, so Hearthgate only uses it when the user turns it on.

pub mod igd;
pub mod natpmp;

use std::collections::BTreeMap;
use std::net::{IpAddr, SocketAddr};
use std::sync::{Mutex, PoisonError};
use std::time::{Duration, SystemTime};

use tokio::sync::watch;
use tokio_util::sync::CancellationToken;
use tracing::{debug, info, warn};

/// Lease asked for each mapping; renewed at half time.
pub const LEASE: Duration = Duration::from_secs(3600);
/// How often to look for a router again after none was found.
const SEARCH_RETRY: Duration = Duration::from_secs(300);

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("no router answered (UPnP and NAT-PMP)")]
    NotFound,
    #[error("{0}")]
    Router(String),
    #[error("the router refused: {} (error {})", .0.description, .0.code)]
    Fault(igd::Fault),
    #[error(transparent)]
    Io(#[from] std::io::Error),
}

/// Transport of a mapping.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, serde::Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Protocol {
    Tcp,
    Udp,
}

impl Protocol {
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Tcp => "TCP",
            Self::Udp => "UDP",
        }
    }
}

/// What should be forwarded.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Wanted {
    pub enabled: bool,
    pub ports: Vec<(u16, Protocol)>,
}

/// One mapping, as shown in the dashboard.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct Mapping {
    pub port: u16,
    pub protocol: Protocol,
    /// The router accepted it.
    pub active: bool,
    pub error: Option<String>,
}

/// Router state for the dashboard.
#[derive(Debug, Clone, Default, PartialEq, Eq, serde::Serialize)]
pub struct Status {
    pub enabled: bool,
    /// `upnp` or `nat-pmp`.
    pub method: Option<&'static str>,
    /// The router's name or address.
    pub router: Option<String>,
    /// This machine's address as the router sees it.
    pub local_ip: Option<IpAddr>,
    /// The router's public address.
    pub public_ip: Option<IpAddr>,
    /// The router's "public" address is itself private: there is another
    /// router (or the provider's carrier-grade NAT) in front of it, and
    /// port forwarding on this router alone will not make ports reachable.
    pub double_nat: bool,
    pub mappings: Vec<Mapping>,
    pub error: Option<String>,
}

/// Where to look for routers. Tests point these at fakes.
#[derive(Debug, Clone)]
pub struct Options {
    pub ssdp: SocketAddr,
    /// NAT-PMP gateway; defaults to the IPv4 default gateway.
    pub natpmp: Option<SocketAddr>,
    pub search_wait: Duration,
}

impl Default for Options {
    fn default() -> Self {
        Self {
            ssdp: igd::SSDP.parse().expect("valid"),
            natpmp: None,
            search_wait: Duration::from_secs(3),
        }
    }
}

#[derive(Debug, Clone)]
enum Device {
    Upnp(igd::Gateway),
    NatPmp(natpmp::NatPmp),
}

impl Device {
    fn method(&self) -> &'static str {
        match self {
            Self::Upnp(_) => "upnp",
            Self::NatPmp(_) => "nat-pmp",
        }
    }

    fn addr(&self) -> SocketAddr {
        match self {
            Self::Upnp(g) => g.addr,
            Self::NatPmp(n) => n.addr,
        }
    }

    async fn external_ip(&self) -> Result<IpAddr, Error> {
        match self {
            Self::Upnp(g) => g.external_ip().await,
            Self::NatPmp(n) => n.external_ip().await,
        }
    }

    async fn add(&self, protocol: Protocol, port: u16, local: IpAddr) -> Result<Duration, Error> {
        match self {
            Self::Upnp(g) => g.add(protocol, port, local, LEASE, "hearthgate").await,
            Self::NatPmp(n) => n.map(protocol, port, LEASE).await,
        }
    }

    async fn remove(&self, protocol: Protocol, port: u16) -> Result<(), Error> {
        match self {
            Self::Upnp(g) => g.remove(protocol, port).await,
            Self::NatPmp(n) => n.remove(protocol, port).await,
        }
    }
}

/// Shared router state.
#[derive(Debug, Default)]
pub struct Router {
    status: Mutex<Status>,
}

impl Router {
    #[must_use]
    pub fn status(&self) -> Status {
        self.status
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
    }

    fn update(&self, f: impl FnOnce(&mut Status)) {
        f(&mut self.status.lock().unwrap_or_else(PoisonError::into_inner));
    }
}

/// Whether a router's public address is really private (double NAT).
#[must_use]
pub fn is_private_public_ip(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(v4) => {
            v4.is_private()
                || v4.is_loopback()
                || v4.is_link_local()
                || v4.is_unspecified()
                // 100.64.0.0/10: carrier-grade NAT.
                || (v4.octets()[0] == 100 && (v4.octets()[1] & 0xc0) == 64)
        }
        IpAddr::V6(_) => false,
    }
}

async fn find(options: &Options) -> Result<Device, Error> {
    match igd::discover(options.ssdp, options.search_wait).await {
        Ok(g) => return Ok(Device::Upnp(g)),
        Err(e) => debug!("UPnP: {e}"),
    }
    let addr = options.natpmp.or_else(|| {
        natpmp::default_gateway().map(|gw| SocketAddr::new(IpAddr::V4(gw), natpmp::PORT))
    });
    if let Some(addr) = addr {
        let device = natpmp::NatPmp { addr };
        if device.external_ip().await.is_ok() {
            return Ok(Device::NatPmp(device));
        }
    }
    Err(Error::NotFound)
}

/// This machine's address on the way to `router`.
async fn local_ip(router: SocketAddr) -> Option<IpAddr> {
    let bind: SocketAddr = if router.is_ipv4() {
        "0.0.0.0:0".parse().ok()?
    } else {
        "[::]:0".parse().ok()?
    };
    let socket = tokio::net::UdpSocket::bind(bind).await.ok()?;
    socket.connect(router).await.ok()?;
    socket.local_addr().ok().map(|a| a.ip())
}

/// Keeps the router's mappings in line with `wanted` until `stop`, then
/// removes them.
pub async fn run(
    router: std::sync::Arc<Router>,
    mut wanted: watch::Receiver<Wanted>,
    options: Options,
    stop: CancellationToken,
) {
    let mut device: Option<(Device, IpAddr)> = None;
    let mut search_after = tokio::time::Instant::now();
    // Active mappings and when to renew them.
    let mut active: BTreeMap<(u16, Protocol), tokio::time::Instant> = BTreeMap::new();
    let mut errors: BTreeMap<(u16, Protocol), String> = BTreeMap::new();
    loop {
        let want = wanted.borrow_and_update().clone();
        let now = tokio::time::Instant::now();
        if want.enabled && device.is_none() && now >= search_after {
            match find(&options).await {
                Ok(d) => {
                    let local = local_ip(d.addr()).await;
                    let public = d.external_ip().await.ok();
                    let double = public.is_some_and(is_private_public_ip);
                    info!(
                        method = d.method(),
                        router = %d.addr(),
                        public = ?public,
                        "router found"
                    );
                    if double {
                        warn!(
                            "the router's public address {} is private: another router or the \
                             provider's NAT is in front of it, so forwarded ports may stay \
                             unreachable from the internet",
                            public.map(|p| p.to_string()).unwrap_or_default()
                        );
                    }
                    router.update(|s| {
                        s.method = Some(d.method());
                        s.router = Some(match &d {
                            Device::Upnp(g) => g.name.clone().map_or_else(
                                || g.addr.ip().to_string(),
                                |n| format!("{n} ({})", g.addr.ip()),
                            ),
                            Device::NatPmp(n) => n.addr.ip().to_string(),
                        });
                        s.local_ip = local;
                        s.public_ip = public;
                        s.double_nat = double;
                        s.error = None;
                    });
                    if let Some(local) = local {
                        device = Some((d, local));
                    }
                }
                Err(e) => {
                    warn!(
                        "port forwarding: {e}; trying again in {}m",
                        SEARCH_RETRY.as_secs() / 60
                    );
                    router.update(|s| s.error = Some(e.to_string()));
                    search_after = now + SEARCH_RETRY;
                }
            }
        }
        if let Some((d, local)) = &device {
            let ports: Vec<(u16, Protocol)> = if want.enabled {
                want.ports.clone()
            } else {
                Vec::new()
            };
            // Remove what is no longer wanted.
            let gone: Vec<(u16, Protocol)> = active
                .keys()
                .filter(|k| !ports.contains(k))
                .copied()
                .collect();
            for key in gone {
                match d.remove(key.1, key.0).await {
                    Ok(()) => info!("router no longer forwards {} {}", key.1.as_str(), key.0),
                    Err(e) => warn!(
                        "cannot remove the router forward for {} {}: {e}",
                        key.1.as_str(),
                        key.0
                    ),
                }
                active.remove(&key);
            }
            errors.retain(|k, _| ports.contains(k));
            for key in &ports {
                let due = active
                    .get(key)
                    .is_none_or(|renew| tokio::time::Instant::now() >= *renew);
                if !due {
                    continue;
                }
                match d.add(key.1, key.0, *local).await {
                    Ok(lease) => {
                        if !active.contains_key(key) {
                            info!("router forwards {} {} to {local}", key.1.as_str(), key.0);
                        }
                        let renew = if lease.is_zero() {
                            Duration::from_secs(24 * 3600)
                        } else {
                            lease / 2
                        };
                        active.insert(*key, tokio::time::Instant::now() + renew);
                        errors.remove(key);
                    }
                    Err(e) => {
                        warn!(
                            "router refused to forward {} {}: {e}",
                            key.1.as_str(),
                            key.0
                        );
                        errors.insert(*key, e.to_string());
                        active.remove(key);
                    }
                }
            }
            let mappings = ports
                .iter()
                .map(|k| Mapping {
                    port: k.0,
                    protocol: k.1,
                    active: active.contains_key(k),
                    error: errors.get(k).cloned(),
                })
                .collect();
            router.update(|s| s.mappings = mappings);
            if !want.enabled {
                // Turned off: forget the router too.
                device = None;
            }
        }
        router.update(|s| {
            s.enabled = want.enabled;
            if !want.enabled {
                *s = Status::default();
            }
        });
        let next_renewal = active
            .values()
            .min()
            .copied()
            .unwrap_or_else(|| tokio::time::Instant::now() + Duration::from_secs(60));
        let wake = next_renewal.min(tokio::time::Instant::now() + Duration::from_secs(60));
        tokio::select! {
            () = stop.cancelled() => break,
            () = tokio::time::sleep_until(wake) => {}
            r = wanted.changed() => if r.is_err() { break },
        }
    }
    // Shutting down: take the forwards away so nothing points at a host
    // that is not listening.
    if let Some((d, _)) = device {
        for key in active.keys() {
            let removed =
                tokio::time::timeout(Duration::from_secs(3), d.remove(key.1, key.0)).await;
            if !matches!(removed, Ok(Ok(()))) {
                warn!(
                    "cannot remove the router forward for {} {}",
                    key.1.as_str(),
                    key.0
                );
            }
        }
        if !active.is_empty() {
            info!("router forwards removed");
        }
    }
}

/// Unix seconds, for status timestamps.
#[must_use]
pub fn unix_now() -> u64 {
    SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs())
}
