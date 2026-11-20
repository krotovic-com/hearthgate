//! Compiles a desired state into the sockets the data plane must run.

use std::collections::{BTreeMap, HashSet};
use std::fmt;
use std::net::{IpAddr, Ipv6Addr, SocketAddr};
use std::sync::Arc;

use hearthgate_model::{DesiredState, Limits, ListenerMode, Service, ServiceKind, host_matches};

use crate::http::{HttpRoute, HttpTable};

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
    /// Plain HTTP routing.
    Http(Arc<HttpTable>),
    /// TLS: passthrough by SNI, or terminate and route HTTP.
    Tls(Arc<TlsPlan>),
}

/// A TLS socket: which names pass through and which are terminated.
#[derive(Debug, Default)]
pub struct TlsPlan {
    /// Host pattern to pool, for `tls-passthrough` routes.
    pub passthrough: Vec<(String, Arc<Pool>)>,
    /// Routes for names Hearthgate terminates.
    pub https: Arc<HttpTable>,
    /// Host patterns of the `https` interfaces on this socket.
    pub terminated: Vec<String>,
}

impl TlsPlan {
    /// The passthrough pool for a server name.
    #[must_use]
    pub fn passthrough_for(&self, sni: Option<&str>) -> Option<&Arc<Pool>> {
        let ranked = |p: &str| {
            if p == "*" {
                2
            } else if p.starts_with("*.") {
                1
            } else {
                0
            }
        };
        let mut matches: Vec<&(String, Arc<Pool>)> = self
            .passthrough
            .iter()
            .filter(|(p, _)| p == "*" || sni.is_some_and(|s| host_matches(p, s)))
            .collect();
        matches.sort_by_key(|(p, _)| ranked(p));
        matches.first().map(|(_, pool)| pool)
    }

    /// Whether Hearthgate terminates TLS for a server name. Without SNI,
    /// any terminated interface will do.
    #[must_use]
    pub fn terminates(&self, sni: Option<&str>) -> bool {
        match sni {
            None => !self.terminated.is_empty(),
            Some(name) => self.terminated.iter().any(|p| host_matches(p, name)),
        }
    }
}

/// A certificate an `https` interface needs.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CertSpec {
    pub interface: String,
    pub domains: Vec<String>,
    /// Manual certificate name; `None` means an automatic one.
    pub manual: Option<String>,
}

/// The kind of a socket plan. A socket whose kind changes is rebound; a
/// socket whose kind stays the same keeps running and only swaps its
/// configuration.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SocketKind {
    RawTcp,
    Udp,
    Http,
    Tls,
}

impl SocketPlan {
    #[must_use]
    pub fn kind(&self) -> SocketKind {
        match self {
            Self::RawTcp(_) => SocketKind::RawTcp,
            Self::Udp(_) => SocketKind::Udp,
            Self::Http(_) => SocketKind::Http,
            Self::Tls(_) => SocketKind::Tls,
        }
    }
}

/// Everything the data plane runs for one revision.
#[derive(Debug, Clone, Default)]
pub struct Plan {
    pub sockets: BTreeMap<SocketKey, SocketPlan>,
    pub limits: Arc<Limits>,
    /// Certificates the `https` interfaces need.
    pub certs: Vec<CertSpec>,
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

#[derive(Default)]
struct TlsBuild {
    passthrough: Vec<(String, Arc<Pool>)>,
    https: HttpTable,
    terminated: Vec<String>,
}

/// Host patterns a route matches.
fn route_hosts(route_hosts: &[String], domains: &[String]) -> Vec<String> {
    if !route_hosts.is_empty() {
        route_hosts.to_vec()
    } else if !domains.is_empty() {
        domains.to_vec()
    } else {
        vec!["*".to_owned()]
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
    let mut sockets = BTreeMap::new();
    let mut http: BTreeMap<SocketKey, HttpTable> = BTreeMap::new();
    let mut tls: BTreeMap<SocketKey, TlsBuild> = BTreeMap::new();
    let mut used = HashSet::new();
    let tcp = |addr| SocketKey {
        addr,
        transport: Transport::Tcp,
    };

    // Sockets exist for every declared listener, even without routes, so
    // redirects and ACME challenges work and the port is predictable.
    let mut certs = Vec::new();
    for interface in &state.interfaces {
        let https_port = interface
            .listeners
            .iter()
            .find(|l| l.mode == ListenerMode::Https)
            .map(|l| l.port);
        for listener in &interface.listeners {
            for addr in bind_addrs(&interface.bind, listener.port) {
                match listener.mode {
                    ListenerMode::Http => {
                        let table = http.entry(tcp(addr)).or_default();
                        if let Some(port) = https_port {
                            for domain in &interface.domains {
                                table.redirects.push((domain.clone(), port));
                            }
                        }
                    }
                    ListenerMode::Https => {
                        let plan = tls.entry(tcp(addr)).or_default();
                        plan.terminated.extend(interface.domains.iter().cloned());
                    }
                    ListenerMode::TlsPassthrough => {
                        tls.entry(tcp(addr)).or_default();
                    }
                    ListenerMode::Raw => {}
                }
            }
        }
        if interface.has_https() {
            certs.push(CertSpec {
                interface: interface.name.clone(),
                domains: interface.domains.clone(),
                manual: interface.certificate.clone(),
            });
        }
    }

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
        let hosts = route_hosts(&route.hosts, &interface.domains);
        for addr in bind_addrs(&interface.bind, listener.port) {
            let http_route = || HttpRoute {
                name: route.name.clone(),
                path_prefix: route.path_prefix.clone(),
                strip_prefix: route.strip_prefix,
                pool: Arc::clone(&pool),
            };
            match listener.mode {
                ListenerMode::Raw => {
                    if listener.tcp() {
                        sockets.insert(tcp(addr), SocketPlan::RawTcp(Arc::clone(&pool)));
                    }
                    if listener.udp() {
                        sockets.insert(
                            SocketKey {
                                addr,
                                transport: Transport::Udp,
                            },
                            SocketPlan::Udp(Arc::clone(&pool)),
                        );
                    }
                }
                ListenerMode::Http => {
                    let table = http.entry(tcp(addr)).or_default();
                    for host in &hosts {
                        table.add(host, http_route());
                    }
                }
                ListenerMode::Https => {
                    let plan = tls.entry(tcp(addr)).or_default();
                    for host in &hosts {
                        plan.https.add(host, http_route());
                    }
                }
                ListenerMode::TlsPassthrough => {
                    let plan = tls.entry(tcp(addr)).or_default();
                    for host in &hosts {
                        plan.passthrough.push((host.clone(), Arc::clone(&pool)));
                    }
                }
            }
        }
    }
    for (key, mut table) in http {
        table.finish();
        sockets.insert(key, SocketPlan::Http(Arc::new(table)));
    }
    for (key, mut build) in tls {
        build.https.finish();
        let plan = TlsPlan {
            passthrough: build.passthrough,
            https: Arc::new(build.https),
            terminated: build.terminated,
        };
        sockets.insert(key, SocketPlan::Tls(Arc::new(plan)));
    }
    upstreams.retain(&used);
    Plan {
        sockets,
        limits: Arc::new(state.limits.clone()),
        certs,
    }
}
