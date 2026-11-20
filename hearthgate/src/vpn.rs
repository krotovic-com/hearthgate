//! Runs the VPN for the desired state: the WireGuard interface, the DNS
//! zone, and what the firewall lets each peer reach.

use std::collections::BTreeMap;
use std::net::{IpAddr, SocketAddr};
use std::sync::Arc;
use std::time::{Duration, Instant};

use hearthgate_firewall::VpnPolicy;
use hearthgate_model::{DesiredState, ServiceKind};
use hearthgate_proxy::ContainerLookup;
use hearthgate_vpn::{Plan, Runtime, Setup, device, dns};
use tokio::sync::watch;
use tracing::{error, info, warn};

/// How often a running interface is checked (for example, that
/// `wireguard-go` is still alive).
const CHECK: Duration = Duration::from_secs(30);

pub struct VpnCtl {
    pub runtime: Arc<Runtime>,
    dns: watch::Sender<Arc<dns::Settings>>,
    /// The plan running now.
    applied: Option<Plan>,
    checked_at: Instant,
    state: DesiredState,
    /// For letting VPN traffic through Docker's forward chain.
    nft: hearthgate_firewall::Nft,
}

impl VpnCtl {
    pub fn new(runtime: Arc<Runtime>) -> (Self, watch::Receiver<Arc<dns::Settings>>) {
        let (tx, rx) = watch::channel(Arc::new(dns::Settings::default()));
        (
            Self {
                runtime,
                dns: tx,
                applied: None,
                checked_at: Instant::now(),
                state: DesiredState::default(),
                nft: hearthgate_firewall::Nft::default(),
            },
            rx,
        )
    }

    /// Brings the VPN in line with `state`, and returns what the firewall
    /// should let peers reach (`None` while the VPN is off).
    pub async fn reconcile(
        &mut self,
        state: &DesiredState,
        containers: &dyn ContainerLookup,
    ) -> Option<VpnPolicy> {
        self.state = state.clone();
        self.docker_forwarding().await;
        if !state.vpn.enabled {
            if self.applied.take().is_some() || self.runtime.device.exists().await {
                if let Err(e) = self.runtime.device.remove().await {
                    error!("cannot remove the VPN interface: {e}");
                }
            }
            self.dns.send_replace(Arc::new(dns::Settings::default()));
            self.runtime
                .update(|i| *i = hearthgate_vpn::Info::default());
            return None;
        }
        let runtime = Arc::clone(&self.runtime);
        let key = match tokio::task::spawn_blocking(move || runtime.private_key()).await {
            Ok(Ok(key)) => key,
            Ok(Err(e)) => {
                return Some(self.fail(format!("VPN: cannot read or create its key: {e}"), state));
            }
            Err(e) => return Some(self.fail(format!("VPN: {e}"), state)),
        };
        let resolved = resolve(state, containers).await;
        let lan: Vec<(IpAddr, ipnet::IpNet)> = hearthgate_api::access::detect_lan()
            .into_iter()
            .filter(|(ip, _)| !device::in_subnet(state.vpn.subnet, *ip))
            .collect();
        let lan_ips: Vec<IpAddr> = lan.iter().map(|(ip, _)| *ip).collect();
        let plan = Plan::new(state, &resolved, &lan_ips);
        let mut problems = Vec::new();
        let setup = Setup {
            private_key: key.clone(),
            port: plan.port,
            address: plan.address,
            peers: plan.peers.clone(),
        };
        let needs_apply =
            self.applied.as_ref() != Some(&plan) || !self.runtime.device.exists().await;
        let mut running = !needs_apply;
        if needs_apply {
            match self.runtime.device.apply(&setup).await {
                Ok(()) => {
                    if self.applied.is_none() {
                        info!(
                            interface = %self.runtime.device.name,
                            address = %plan.address,
                            port = plan.port,
                            peers = plan.peers.len(),
                            "VPN is up"
                        );
                    }
                    self.applied = Some(plan.clone());
                    running = true;
                }
                Err(e) => {
                    error!("cannot set up the VPN interface: {e}");
                    problems.push(format!("VPN: {e}"));
                    self.applied = None;
                }
            }
        }
        if !device::forwarding_enabled() && device::enable_forwarding().is_err() {
            problems.push(
                "VPN: IPv4 forwarding is off, so peers reach only this machine. Turn it on with \
                 `sysctl -w net.ipv4.ip_forward=1` (and in /etc/sysctl.d to keep it)"
                    .into(),
            );
        }
        let upstream = upstream(state, &plan.dns_listen);
        self.dns.send_if_modified(|current| {
            let next = dns::Settings {
                listen: if running {
                    plan.dns_listen.clone()
                } else {
                    Vec::new()
                },
                zone: plan.zone.clone(),
                upstream,
            };
            let changed = **current != next;
            if changed {
                *current = Arc::new(next);
            }
            changed
        });
        let public_key = hearthgate_vpn::keys::public_key(&key);
        let backend = running.then(|| self.runtime.device.backend());
        let lan_nets = lan.iter().map(|(_, net)| *net).collect();
        self.runtime.update(|i| {
            i.running = running;
            i.backend = backend;
            i.public_key = public_key;
            i.problems = problems;
            i.resolved = resolved;
            i.lan = lan_nets;
        });
        Some(VpnPolicy {
            interface: self.runtime.device.name.clone(),
            subnet: state.vpn.subnet,
            server: state.vpn.server_address(),
            dns: state.dns.enabled,
            full: plan.full.clone(),
            allowed: plan.allowed.clone(),
        })
    }

    fn fail(&mut self, problem: String, state: &DesiredState) -> VpnPolicy {
        error!("{problem}");
        self.runtime.update(|i| {
            i.running = false;
            i.problems = vec![problem];
        });
        // Keep peers fenced in even when the interface is broken.
        VpnPolicy {
            interface: self.runtime.device.name.clone(),
            subnet: state.vpn.subnet,
            server: state.vpn.server_address(),
            dns: false,
            full: Vec::new(),
            allowed: Vec::new(),
        }
    }

    /// Whether the interface needs another look (it went away, or the
    /// userspace daemon died). The caller reconciles when this is true.
    pub async fn check(&mut self) -> bool {
        if self.checked_at.elapsed() < CHECK || !self.state.vpn.enabled {
            return false;
        }
        self.checked_at = Instant::now();
        // Docker may have started (or restarted) since.
        self.docker_forwarding().await;
        if self.applied.is_some() && !self.runtime.device.exists().await {
            warn!("the VPN interface disappeared; setting it up again");
            self.applied = None;
            return true;
        }
        false
    }

    /// Docker drops forwarded packets it doesn't know about, which would
    /// keep peers from the LAN and from containers.
    async fn docker_forwarding(&self) {
        let interface = self
            .state
            .vpn
            .enabled
            .then_some(self.runtime.device.name.as_str());
        if let Err(e) = self.nft.docker_user(interface).await {
            warn!("cannot let VPN traffic through Docker's forward chain: {e}");
        }
    }

    /// Takes the interface down when Hearthgate stops.
    pub async fn stop(&self) {
        if self.applied.is_some() {
            if let Err(e) = self.runtime.device.remove().await {
                warn!("cannot remove the VPN interface: {e}");
            }
        }
    }

    #[must_use]
    pub fn problems(&self) -> Vec<String> {
        self.runtime.info().problems
    }
}

/// Upstream resolvers: configured, else the system's, minus our own
/// addresses.
fn upstream(state: &DesiredState, listen: &[SocketAddr]) -> Vec<SocketAddr> {
    let configured: Vec<SocketAddr> = state
        .dns
        .upstream
        .iter()
        .filter_map(|u| {
            u.parse::<SocketAddr>()
                .ok()
                .or_else(|| u.parse::<IpAddr>().ok().map(|ip| SocketAddr::new(ip, 53)))
        })
        .collect();
    let list = if configured.is_empty() {
        dns::system_resolvers()
    } else {
        configured
    };
    list.into_iter().filter(|a| !listen.contains(a)).collect()
}

/// Addresses of services that only the running system knows: containers,
/// and host names.
async fn resolve(
    state: &DesiredState,
    containers: &dyn ContainerLookup,
) -> BTreeMap<String, Vec<IpAddr>> {
    let mut out = BTreeMap::new();
    for service in &state.services {
        match service.kind {
            ServiceKind::Docker => {
                let container = service.container.as_deref().unwrap_or(&service.name);
                if let Some(ip) = containers.container_ip(container, service.network.as_deref()) {
                    out.insert(service.name.clone(), vec![ip]);
                }
            }
            ServiceKind::External => {
                let Some(host) = service.host.as_deref() else {
                    continue;
                };
                if host.parse::<IpAddr>().is_ok() || host.eq_ignore_ascii_case("localhost") {
                    continue;
                }
                let lookup = tokio::net::lookup_host((host, 0));
                if let Ok(Ok(addrs)) = tokio::time::timeout(Duration::from_secs(2), lookup).await {
                    let mut ips: Vec<IpAddr> = addrs.map(|a| a.ip()).collect();
                    ips.sort();
                    ips.dedup();
                    out.insert(service.name.clone(), ips);
                }
            }
        }
    }
    // Managed containers get names too, as `container:NAME`.
    for container in &state.containers {
        if let Some(ip) = containers.container_ip(&container.name, Some(hearthgate_docker::NETWORK))
        {
            out.insert(format!("container:{}", container.name), vec![ip]);
        }
    }
    out
}
