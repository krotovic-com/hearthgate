//! Compiles a desired state into the sockets the data plane must run.

use std::collections::{BTreeMap, HashSet};
use std::fmt;
use std::net::{IpAddr, Ipv6Addr, SocketAddr};
use std::sync::Arc;

use hearthgate_model::{DesiredState, Limits, ListenerMode, Service, ServiceKind};

use crate::pool::{Endpoint, Member, Pool, Upstreams};

/// Transport of a socket.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Transport {
    Tcp,
    Udp,
}

impl fmt::Display for Transport {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::Tcp => "tcp",
            Self::Udp => "udp",
        })
    }
}

/// Identifies one listening socket. `[::]` means every IPv4 and IPv6
/// address.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct SocketKey {
    pub addr: SocketAddr,
    pub transport: Transport,
}

impl fmt::Display for SocketKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}/{}", self.addr, self.transport)
    }
}

/// What a socket does.
#[derive(Debug, Clone)]
pub enum SocketPlan {
    /// Forward TCP connections to a pool.
    RawTcp(Arc<Pool>),
    /// Forward UDP datagrams to a pool, tracking client sessions.
    Udp(Arc<Pool>),
}

/// The kind of a socket plan. A socket whose kind changes is rebound; a
/// socket whose kind stays the same keeps running and only swaps its
/// configuration.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SocketKind {
    RawTcp,
    Udp,
}

impl SocketPlan {
    #[must_use]
    pub fn kind(&self) -> SocketKind {
        match self {
            Self::RawTcp(_) => SocketKind::RawTcp,
            Self::Udp(_) => SocketKind::Udp,
        }
    }
}

/// Everything the data plane runs for one revision.
#[derive(Debug, Clone, Default)]
pub struct Plan {
    pub sockets: BTreeMap<SocketKey, SocketPlan>,
    pub limits: Arc<Limits>,
}

/// Finds the address of a Docker container.
pub trait ContainerLookup {
    /// Address of `container` on `network` (or its first network).
    fn container_ip(&self, container: &str, network: Option<&str>) -> Option<IpAddr>;
}

/// A lookup that knows no containers, for setups without Docker.
#[derive(Debug, Default, Clone, Copy)]
pub struct NoContainers;

impl ContainerLookup for NoContainers {
    fn container_ip(&self, _container: &str, _network: Option<&str>) -> Option<IpAddr> {
        None
    }
}

/// Where a service port is right now.
pub fn endpoint(service: &Service, port: u16, containers: &dyn ContainerLookup) -> Endpoint {
    match service.kind {
        ServiceKind::External => match &service.host {
            Some(host) => Endpoint::from_host(host, port),
            None => Endpoint::Unresolved,
        },
        ServiceKind::Docker => service
            .container
            .as_deref()
            .and_then(|c| containers.container_ip(c, service.network.as_deref()))
            .map_or(Endpoint::Unresolved, |ip| {
                Endpoint::Socket(SocketAddr::new(ip, port))
            }),
    }
}

/// The local addresses an interface binds.
fn bind_addrs(bind: &[IpAddr], port: u16) -> Vec<SocketAddr> {
    if bind.is_empty() {
        vec![SocketAddr::new(IpAddr::V6(Ipv6Addr::UNSPECIFIED), port)]
    } else {
        bind.iter().map(|ip| SocketAddr::new(*ip, port)).collect()
    }
}

/// Compiles a validated desired state into a plan. Upstream states are
/// taken from (and kept in) `upstreams`.
#[must_use]
pub fn compile(
    state: &DesiredState,
    upstreams: &Upstreams,
    containers: &dyn ContainerLookup,
) -> Plan {
    let mut plan = Plan {
        sockets: BTreeMap::new(),
        limits: Arc::new(state.limits.clone()),
    };
    let mut used = HashSet::new();
    for route in &state.routes {
        let Some(interface) = state.interface(&route.interface) else {
            continue;
        };
        let Some(listener) = interface.listener(route.listener) else {
            continue;
        };
        let members: Vec<Member> = route
            .targets
            .iter()
            .filter_map(|t| {
                let service = state.service(&t.service)?;
                let port = service.resolve_port(&t.port)?;
                used.insert((service.name.clone(), port));
                Some(Member {
                    state: upstreams.get(&service.name, port, endpoint(service, port, containers)),
                    weight: t.weight,
                })
            })
            .collect();
        let pool = Arc::new(Pool::new(&route.name, route.strategy, members));
        match listener.mode {
            ListenerMode::Raw => {
                for addr in bind_addrs(&interface.bind, listener.port) {
                    if listener.tcp() {
                        plan.sockets.insert(
                            SocketKey {
                                addr,
                                transport: Transport::Tcp,
                            },
                            SocketPlan::RawTcp(Arc::clone(&pool)),
                        );
                    }
                    if listener.udp() {
                        plan.sockets.insert(
                            SocketKey {
                                addr,
                                transport: Transport::Udp,
                            },
                            SocketPlan::Udp(Arc::clone(&pool)),
                        );
                    }
                }
            }
            ListenerMode::Http | ListenerMode::Https | ListenerMode::TlsPassthrough => {}
        }
    }
    upstreams.retain(&used);
    plan
}
