//! What the VPN should look like for a desired state: the interface's
//! peers, the DNS names, and what each peer may reach.

use std::collections::BTreeMap;
use std::net::{IpAddr, Ipv4Addr, SocketAddr};

use hearthgate_model::{Access, DesiredState, ServiceKind, split_endpoint};
use ipnet::{IpNet, Ipv4Net};

use crate::PeerConfig;
use crate::dns::Zone;

/// The VPN's runtime shape.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Plan {
    pub port: u16,
    /// This machine's address with the VPN's prefix.
    pub address: Ipv4Net,
    pub peers: Vec<PeerConfig>,
    pub zone: Zone,
    pub dns_listen: Vec<SocketAddr>,
    /// Peers that reach everything.
    pub full: Vec<Ipv4Addr>,
    /// `(peer, destination, port)` for limited peers.
    pub allowed: Vec<(Ipv4Addr, Ipv4Addr, u16)>,
}

/// Addresses of each service as seen from the VPN. Services on this
/// machine (loopback) are reached at its VPN address. `resolved` holds
/// what only the running gateway knows: container addresses and resolved
/// host names.
#[must_use]
pub fn service_addresses(
    state: &DesiredState,
    resolved: &BTreeMap<String, Vec<IpAddr>>,
) -> BTreeMap<String, Vec<IpAddr>> {
    let server = IpAddr::V4(state.vpn.server_address());
    let mut out = BTreeMap::new();
    for service in &state.services {
        let mut addresses: Vec<IpAddr> = match (&service.kind, service.host.as_deref()) {
            (ServiceKind::External, Some(host)) => match host.parse::<IpAddr>() {
                Ok(ip) => vec![ip],
                Err(_) if host.eq_ignore_ascii_case("localhost") => vec![server],
                Err(_) => resolved.get(&service.name).cloned().unwrap_or_default(),
            },
            _ => resolved.get(&service.name).cloned().unwrap_or_default(),
        };
        for ip in &mut addresses {
            if ip.is_loopback() || ip.is_unspecified() {
                *ip = server;
            }
        }
        addresses.sort();
        addresses.dedup();
        if !addresses.is_empty() {
            out.insert(service.name.clone(), addresses);
        }
    }
    out
}

impl Plan {
    /// `lan` holds this machine's LAN addresses, for DNS on the LAN.
    #[must_use]
    pub fn new(
        state: &DesiredState,
        resolved: &BTreeMap<String, Vec<IpAddr>>,
        lan: &[IpAddr],
    ) -> Self {
        let vpn = &state.vpn;
        let server = vpn.server_address();
        let services = service_addresses(state, resolved);
        let zone_name = state.dns.zone.clone();
        let mut names: BTreeMap<String, Vec<IpAddr>> = BTreeMap::new();
        names.insert(format!("hearthgate.{zone_name}"), vec![IpAddr::V4(server)]);
        for (name, addresses) in &services {
            names.insert(format!("{name}.{zone_name}"), addresses.clone());
        }
        for peer in &vpn.peers {
            names
                .entry(format!("{}.{zone_name}", peer.name))
                .or_insert_with(|| vec![IpAddr::V4(peer.address)]);
        }
        // Public domains served on every address reach this machine
        // directly over the VPN, without going out and back in.
        for interface in state.interfaces.iter().filter(|i| i.bind.is_empty()) {
            for domain in &interface.domains {
                if !domain.starts_with("*.") {
                    names.insert(domain.clone(), vec![IpAddr::V4(server)]);
                }
            }
        }
        let mut dns_listen = Vec::new();
        if state.dns.enabled {
            dns_listen.push(SocketAddr::new(IpAddr::V4(server), 53));
            if state.dns.lan {
                for ip in lan {
                    let link_local = match ip {
                        IpAddr::V4(v4) => v4.is_link_local(),
                        IpAddr::V6(v6) => (v6.segments()[0] & 0xffc0) == 0xfe80,
                    };
                    if !ip.is_loopback()
                        && !link_local
                        && !crate::device::in_subnet(vpn.subnet, *ip)
                    {
                        dns_listen.push(SocketAddr::new(*ip, 53));
                    }
                }
            }
        }
        let mut full = Vec::new();
        let mut allowed = Vec::new();
        for peer in &vpn.peers {
            match &peer.access {
                Access::All => full.push(peer.address),
                Access::Services(list) => {
                    for name in list {
                        let Some(service) = state.services.iter().find(|s| &s.name == name) else {
                            continue;
                        };
                        for ip in services.get(name).into_iter().flatten() {
                            let IpAddr::V4(dst) = ip else { continue };
                            for port in service.ports.values() {
                                allowed.push((peer.address, *dst, *port));
                            }
                        }
                    }
                }
            }
        }
        allowed.sort_unstable();
        allowed.dedup();
        Self {
            port: vpn.port,
            address: Ipv4Net::new(server, vpn.subnet.prefix_len()).unwrap_or(vpn.subnet),
            peers: vpn
                .peers
                .iter()
                .map(|p| PeerConfig {
                    public_key: p.public_key.clone(),
                    allowed_ips: vec![IpNet::V4(Ipv4Net::from(p.address))],
                })
                .collect(),
            zone: Zone {
                zone: zone_name,
                names,
            },
            dns_listen,
            full,
            allowed,
        }
    }
}

/// Networks a peer's device should send through the tunnel: the VPN, and
/// the networks behind it the peer may reach.
#[must_use]
pub fn client_routes(
    state: &DesiredState,
    access: &Access,
    lan: &[IpNet],
    resolved: &BTreeMap<String, Vec<IpAddr>>,
) -> Vec<IpNet> {
    let subnet = state.vpn.subnet;
    let mut out = vec![IpNet::V4(subnet)];
    let services = service_addresses(state, resolved);
    let reachable: Vec<IpAddr> = match access {
        Access::All => {
            for net in lan {
                if let IpNet::V4(v4) = net {
                    if v4.network().is_private() && !subnet.contains(v4) {
                        out.push(IpNet::V4(v4.trunc()));
                    }
                }
            }
            services.values().flatten().copied().collect()
        }
        Access::Services(list) => list
            .iter()
            .filter_map(|name| services.get(name))
            .flatten()
            .copied()
            .collect(),
    };
    for ip in reachable {
        if let IpAddr::V4(v4) = ip {
            if !out.iter().any(|n| n.contains(&ip)) {
                out.push(IpNet::V4(Ipv4Net::from(v4)));
            }
        }
    }
    out
}

/// The `host:port` peers connect to: the configured endpoint, else the
/// first public domain, else the router's public address.
#[must_use]
pub fn endpoint(state: &DesiredState, public_ip: Option<IpAddr>) -> Option<String> {
    let port = state.vpn.port;
    if let Some(text) = &state.vpn.endpoint {
        return Some(match split_endpoint(text) {
            Some((_, Some(_))) => text.clone(),
            Some((host, None)) if host.contains(':') => format!("[{host}]:{port}"),
            _ => format!("{text}:{port}"),
        });
    }
    let domain = state
        .interfaces
        .iter()
        .flat_map(|i| i.domains.iter())
        .find(|d| !d.starts_with("*."));
    if let Some(domain) = domain {
        return Some(format!("{domain}:{port}"));
    }
    public_ip.map(|ip| SocketAddr::new(ip, port).to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn state() -> DesiredState {
        DesiredState::from_toml(
            r#"
            [acme]
            accept_terms = true

            [[interfaces]]
            name = "web"
            domains = ["app.example.com"]
            listeners = [{ port = 443, mode = "https" }]

            [[services]]
            name = "sshd"
            kind = "external"
            host = "127.0.0.1"
            ports = { ssh = 22 }

            [[services]]
            name = "nas"
            kind = "external"
            host = "192.168.1.20"
            ports = { web = 5000, smb = 445 }

            [vpn]
            enabled = true

            [[vpn.peers]]
            name = "phone"
            public_key = "Y2FD6uTgrq+/bbBAXCNOtE+PGk8Papoh7qaWBpQUXn0="
            address = "10.88.0.2"

            [[vpn.peers]]
            name = "guest"
            public_key = "YAnz5TF+lXXJte14tji3zlMNq+hd2rYUIgJBgB3fBmk="
            address = "10.88.0.3"
            access = ["nas"]
            "#,
        )
        .unwrap()
    }

    #[test]
    fn plans_peers_names_and_access() {
        let state = state();
        let plan = Plan::new(&state, &BTreeMap::new(), &["192.168.1.10".parse().unwrap()]);
        assert_eq!(plan.address.to_string(), "10.88.0.1/24");
        assert_eq!(plan.peers[0].allowed_ips[0].to_string(), "10.88.0.3/32");
        let names = &plan.zone.names;
        assert_eq!(
            names["sshd.home.internal"],
            vec!["10.88.0.1".parse::<IpAddr>().unwrap()]
        );
        assert_eq!(
            names["nas.home.internal"],
            vec!["192.168.1.20".parse::<IpAddr>().unwrap()]
        );
        assert_eq!(
            names["phone.home.internal"],
            vec!["10.88.0.2".parse::<IpAddr>().unwrap()]
        );
        assert_eq!(
            names["app.example.com"],
            vec!["10.88.0.1".parse::<IpAddr>().unwrap()]
        );
        assert_eq!(plan.dns_listen, vec!["10.88.0.1:53".parse().unwrap()]);
        assert_eq!(plan.full, vec![Ipv4Addr::new(10, 88, 0, 2)]);
        let guest = Ipv4Addr::new(10, 88, 0, 3);
        let nas = Ipv4Addr::new(192, 168, 1, 20);
        assert_eq!(plan.allowed, vec![(guest, nas, 445), (guest, nas, 5000)]);
    }

    #[test]
    fn routes_and_endpoint() {
        let state = state();
        let lan: Vec<IpNet> = vec![
            "192.168.1.0/24".parse().unwrap(),
            "fd00::/64".parse().unwrap(),
        ];
        let all = client_routes(&state, &Access::All, &lan, &BTreeMap::new());
        let all: Vec<String> = all.iter().map(ToString::to_string).collect();
        assert_eq!(all, ["10.88.0.0/24", "192.168.1.0/24"]);
        let some = client_routes(
            &state,
            &Access::Services(vec!["nas".into()]),
            &lan,
            &BTreeMap::new(),
        );
        let some: Vec<String> = some.iter().map(ToString::to_string).collect();
        assert_eq!(some, ["10.88.0.0/24", "192.168.1.20/32"]);
        assert_eq!(
            endpoint(&state, None).as_deref(),
            Some("app.example.com:51820")
        );
        let mut other = state.clone();
        other.vpn.endpoint = Some("vpn.example.net".into());
        assert_eq!(
            endpoint(&other, None).as_deref(),
            Some("vpn.example.net:51820")
        );
        other.vpn.endpoint = Some("2001:db8::1".into());
        assert_eq!(
            endpoint(&other, None).as_deref(),
            Some("[2001:db8::1]:51820")
        );
        other.vpn.endpoint = None;
        other.interfaces.clear();
        assert_eq!(
            endpoint(&other, "198.51.100.4".parse().ok()).as_deref(),
            Some("198.51.100.4:51820")
        );
    }
}
