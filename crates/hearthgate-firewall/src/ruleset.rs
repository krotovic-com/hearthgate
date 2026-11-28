//! Renders the `inet hearthgate` nftables table from the desired state.
//!
//! The whole table is replaced in one `nft -f` transaction, so a ruleset is
//! either fully in place or not at all. Packets pass two base chains:
//!
//! - `prerouting` (raw priority, before connection tracking): drops sources
//!   on blocklists, the manual block list and the ban list, for traffic to
//!   this host and to containers alike. Private and other special networks
//!   and the manual allow list are never dropped here.
//! - `input` (filter priority): the default-drop policy for traffic to this
//!   host, with allowances for established connections, essential ICMP,
//!   the user's rules, the dashboard, the LAN and the public ports.
//!
//! A `forward` chain keeps ports that Docker publishes on the host reachable
//! from the LAN only, unless the user allows them.
//!
//! With the VPN on, both chains also limit each peer to what it may reach,
//! and a `postrouting` chain masquerades peers' traffic to the LAN. That
//! part is installed even when the firewall itself is off, since per-peer
//! access depends on it.
//!
//! The `forward` chain also guards the managed containers' network, whose
//! containers Docker leaves reachable to routed traffic so that VPN peers
//! get to them: new connections from anywhere else are dropped, except to
//! ports Docker publishes. That part is installed whenever the network
//! exists, firewall or not, since Docker itself no longer guards it.

use std::fmt::Write as _;
use std::net::{IpAddr, Ipv4Addr};
use std::time::Duration;

use hearthgate_model::{Firewall, PortRange, Protocol, RuleAction, SPECIAL_NETS};
use ipnet::{IpNet, Ipv4Net};

/// Name of the nftables table Hearthgate owns. Nothing else is touched.
pub const TABLE: &str = "hearthgate";

/// A port that must be open to everyone.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct OpenPort {
    pub port: u16,
    pub tcp: bool,
    pub udp: bool,
    /// New connections per second per client address (from the interface).
    pub rate_limit: Option<u32>,
}

/// Everything the ruleset depends on besides the firewall settings.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Inputs {
    pub firewall: Firewall,
    /// Public ports from the interfaces, plus ports subsystems need (the
    /// VPN's UDP port).
    pub open: Vec<OpenPort>,
    /// The dashboard's ports and the networks allowed to reach them.
    pub admin: Option<(Vec<u16>, Vec<IpNet>)>,
    /// Networks this host is attached to.
    pub lan: Vec<IpNet>,
    /// Network interfaces whose traffic is trusted like the LAN.
    pub trusted_interfaces: Vec<String>,
    /// The VPN, when it is on.
    pub vpn: Option<VpnPolicy>,
    /// Subnet of the managed containers' Docker network, when it exists.
    pub containers: Option<Ipv4Net>,
}

impl Inputs {
    /// Whether anything needs the table.
    #[must_use]
    pub fn needs_table(&self) -> bool {
        self.firewall.enabled || self.vpn.is_some() || self.containers.is_some()
    }
}

/// What each VPN peer may reach.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VpnPolicy {
    /// The WireGuard interface.
    pub interface: String,
    pub subnet: Ipv4Net,
    /// This machine's VPN address, where DNS answers.
    pub server: Ipv4Addr,
    pub dns: bool,
    /// Peers that reach everything.
    pub full: Vec<Ipv4Addr>,
    /// `(peer, destination, port)` for peers limited to some services, over
    /// TCP and UDP.
    pub allowed: Vec<(Ipv4Addr, Ipv4Addr, u16)>,
}

/// Addresses loaded from feeds and bans. Kept apart from [`Inputs`] because
/// they change on their own schedule.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Dynamic {
    /// Blocklist feeds and countries, merged.
    pub feeds: Vec<IpNet>,
    /// Banned addresses with the time left on each ban.
    pub bans: Vec<(IpAddr, Duration)>,
}

/// Why a packet was dropped, as recorded in the `recent_*` sets.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, serde::Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum Reason {
    /// A blocklist feed, a blocked country or the manual block list.
    Blocklist,
    /// A temporary ban.
    Ban,
    /// The default policy or a deny rule.
    Policy,
    /// Too many new connections from one address.
    RateLimit,
}

impl Reason {
    pub const ALL: [Self; 4] = [Self::Blocklist, Self::Ban, Self::Policy, Self::RateLimit];

    #[must_use]
    pub fn key(self) -> &'static str {
        match self {
            Self::Blocklist => "blocklist",
            Self::Ban => "ban",
            Self::Policy => "policy",
            Self::RateLimit => "ratelimit",
        }
    }
}

/// How long a dropped source stays in the recent-drops sets.
pub const RECENT_TIMEOUT: Duration = Duration::from_secs(3600);
/// Entries kept per recent-drops set.
const RECENT_SIZE: u32 = 4096;

/// Renders the script that replaces the table. Run it with `nft -f -`.
#[must_use]
pub fn render(inputs: &Inputs, dynamic: &Dynamic) -> String {
    let mut out = String::with_capacity(4096 + dynamic.feeds.len() * 20);
    // `add` first so `delete` cannot fail on a host without the table.
    let _ = writeln!(out, "add table inet {TABLE}");
    let _ = writeln!(out, "delete table inet {TABLE}");
    let _ = writeln!(out, "table inet {TABLE} {{");
    let fw = &inputs.firewall;

    // Counters, one per drop reason, for the dashboard.
    for reason in Reason::ALL {
        let _ = writeln!(out, "  counter dropped_{} {{ }}", reason.key());
    }

    let special: Vec<IpNet> = SPECIAL_NETS
        .iter()
        .map(|s| s.parse().expect("valid special network"))
        .collect();
    let mut admin_nets = inputs
        .admin
        .as_ref()
        .map(|(_, nets)| nets.clone())
        .unwrap_or_default();
    admin_nets = IpNet::aggregate(&admin_nets);
    let lan = IpNet::aggregate(&inputs.lan);
    let mut block = fw.block.clone();
    block.extend(dynamic.feeds.iter().copied());
    let block = IpNet::aggregate(&block);

    if fw.enabled {
        interval_set(&mut out, "special", &special);
        interval_set(&mut out, "allow", &fw.allow);
        interval_set(&mut out, "block", &block);
        interval_set(&mut out, "lan", &lan);
        interval_set(&mut out, "admin", &admin_nets);
        ban_sets(&mut out, &dynamic.bans);
    }
    for reason in Reason::ALL {
        for (family, ty) in [("4", "ipv4_addr"), ("6", "ipv6_addr")] {
            let _ = writeln!(
                out,
                "  set recent_{}{family} {{ type {ty} . inet_proto . inet_service; \
                 flags dynamic,timeout; timeout {}s; size {RECENT_SIZE}; counter; }}",
                reason.key(),
                RECENT_TIMEOUT.as_secs()
            );
        }
    }
    if let Some(vpn) = &inputs.vpn {
        vpn_sets(&mut out, vpn);
    }
    let limited: Vec<(u32, Vec<&OpenPort>)> = rate_groups(&inputs.open);
    for (i, _) in limited.iter().enumerate() {
        for (family, ty) in [("4", "ipv4_addr"), ("6", "ipv6_addr")] {
            let _ = writeln!(
                out,
                "  set rate{i}_{family} {{ type {ty}; flags dynamic,timeout; timeout 1m; size 65536; }}"
            );
        }
    }

    if !fw.enabled {
        // The VPN and the containers' network alone: nothing else is
        // filtered.
        unfiltered(&mut out, inputs);
        return out;
    }

    // prerouting: lists and bans.
    out.push_str(
        "  chain prerouting {\n    type filter hook prerouting priority raw; policy accept;\n",
    );
    out.push_str("    iif \"lo\" accept\n");
    for family in Family::BOTH {
        let (f, a) = (family.suffix(), family.addr());
        let _ = writeln!(out, "    {a} saddr @special{f} accept");
        let _ = writeln!(out, "    {a} saddr @allow{f} accept");
        drop_rule(
            &mut out,
            &format!("{a} saddr @block{f}"),
            family,
            Reason::Blocklist,
        );
        drop_rule(&mut out, &format!("{a} saddr @ban{f}"), family, Reason::Ban);
    }
    out.push_str("  }\n");

    // input: default drop.
    out.push_str("  chain input {\n    type filter hook input priority filter; policy accept;\n");
    out.push_str("    ct state established,related accept\n");
    out.push_str("    ct state invalid drop\n");
    out.push_str("    iif \"lo\" accept\n");
    // IPv6 does not work without neighbour discovery, and path MTU
    // discovery needs these error messages on both families.
    out.push_str(
        "    icmpv6 type { destination-unreachable, packet-too-big, time-exceeded, \
         parameter-problem, nd-router-solicit, nd-router-advert, nd-neighbor-solicit, \
         nd-neighbor-advert, mld-listener-query } accept\n",
    );
    out.push_str(
        "    icmp type { destination-unreachable, time-exceeded, parameter-problem } accept\n",
    );
    // DHCP answers to this host as a client.
    out.push_str("    udp sport 67 udp dport 68 accept\n");
    out.push_str("    ip6 saddr fe80::/10 udp sport 547 udp dport 546 accept\n");
    if fw.allow_ping {
        out.push_str("    icmp type echo-request limit rate 20/second accept\n");
        out.push_str("    icmpv6 type echo-request limit rate 20/second accept\n");
    }
    if let Some(vpn) = &inputs.vpn {
        vpn_input(&mut out, vpn, &inputs.open);
    }
    for rule in &fw.rules {
        let _ = writeln!(out, "    # rule {}", rule.name);
        for matcher in rule_matchers(rule) {
            match rule.action {
                RuleAction::Allow => {
                    let _ = writeln!(out, "    {matcher} accept");
                }
                RuleAction::Deny => {
                    for family in Family::BOTH {
                        if matcher_family(&matcher).is_none_or(|f| f == family) {
                            drop_rule(
                                &mut out,
                                &format!("{} {matcher}", family.nfproto()),
                                family,
                                Reason::Policy,
                            );
                        }
                    }
                }
            }
        }
    }
    if let Some((ports, _)) = inputs.admin.as_ref().filter(|(p, _)| !p.is_empty()) {
        let list: Vec<String> = ports.iter().map(ToString::to_string).collect();
        for family in Family::BOTH {
            let _ = writeln!(
                out,
                "    {} saddr @admin{} tcp dport {{ {} }} accept",
                family.addr(),
                family.suffix(),
                list.join(", ")
            );
        }
    }
    for iface in &inputs.trusted_interfaces {
        let _ = writeln!(out, "    iifname {} accept", quote(iface));
    }
    if fw.allow_lan {
        for family in Family::BOTH {
            let _ = writeln!(
                out,
                "    {} saddr @lan{} accept",
                family.addr(),
                family.suffix()
            );
        }
    }
    for (i, (rate, ports)) in limited.iter().enumerate() {
        for (proto, dports) in proto_ports(ports) {
            for family in Family::BOTH {
                let (f, a) = (family.suffix(), family.addr());
                let _ = writeln!(
                    out,
                    "    {} {proto} dport {dports} ct state new add @rate{i}_{f} {{ {a} saddr limit rate over {rate}/second burst {} packets }} meta l4proto {{ tcp, udp }} update @recent_ratelimit{f} {{ {a} saddr . meta l4proto . th dport }} counter name dropped_ratelimit drop",
                    family.nfproto(),
                    rate.saturating_mul(2).max(10)
                );
            }
        }
    }
    let all: Vec<&OpenPort> = inputs.open.iter().collect();
    for (proto, dports) in proto_ports(&all) {
        let _ = writeln!(out, "    {proto} dport {dports} accept");
    }
    for family in Family::BOTH {
        drop_rule(&mut out, family.nfproto(), family, Reason::Policy);
    }
    out.push_str("  }\n");

    // forward: ports Docker publishes stay on the LAN.
    out.push_str(
        "  chain forward {\n    type filter hook forward priority filter; policy accept;\n",
    );
    if let Some(vpn) = &inputs.vpn {
        vpn_forward(&mut out, vpn);
    }
    if let Some(net) = inputs.containers {
        containers_forward(&mut out, net);
    }
    if !fw.allow_docker_ports {
        for iface in &inputs.trusted_interfaces {
            let _ = writeln!(out, "    iifname {} accept", quote(iface));
        }
        for family in Family::BOTH {
            let (f, a) = (family.suffix(), family.addr());
            let _ = writeln!(
                out,
                "    ct state new ct status dnat {a} saddr != @special{f} {a} saddr != @allow{f} {a} saddr != @lan{f} meta l4proto {{ tcp, udp }} update @recent_policy{f} {{ {a} saddr . meta l4proto . th dport }} counter name dropped_policy drop"
            );
        }
    }
    out.push_str("  }\n");
    if let Some(vpn) = &inputs.vpn {
        vpn_nat(&mut out, vpn);
    }
    out.push_str("}\n");
    out
}

fn vpn_sets(out: &mut String, vpn: &VpnPolicy) {
    let full: Vec<String> = vpn.full.iter().map(ToString::to_string).collect();
    let _ = write!(out, "  set vpn_full {{ type ipv4_addr;");
    if !full.is_empty() {
        let _ = write!(out, " elements = {{ {} }}", full.join(", "));
    }
    out.push_str(" }\n");
    let mut allowed: Vec<String> = Vec::new();
    for (peer, dst, port) in &vpn.allowed {
        for proto in ["tcp", "udp"] {
            allowed.push(format!("{peer} . {dst} . {proto} . {port}"));
        }
    }
    allowed.sort();
    allowed.dedup();
    let _ = write!(
        out,
        "  set vpn_allowed {{ type ipv4_addr . ipv4_addr . inet_proto . inet_service;"
    );
    if !allowed.is_empty() {
        let _ = write!(out, " elements = {{ {} }}", allowed.join(", "));
    }
    out.push_str(" }\n");
}

/// Input rules for traffic from peers: DNS, what each peer may reach, and
/// the public ports (open to everyone anyway); everything else is dropped.
fn vpn_input(out: &mut String, vpn: &VpnPolicy, open: &[OpenPort]) {
    let iface = quote(&vpn.interface);
    let _ = writeln!(out, "    # VPN peers");
    if vpn.dns {
        let _ = writeln!(
            out,
            "    iifname {iface} ip daddr {} meta l4proto {{ tcp, udp }} th dport 53 accept",
            vpn.server
        );
    }
    let _ = writeln!(out, "    iifname {iface} icmp type echo-request accept");
    let _ = writeln!(out, "    iifname {iface} ip saddr @vpn_full accept");
    let _ = writeln!(
        out,
        "    iifname {iface} ip saddr . ip daddr . meta l4proto . th dport @vpn_allowed accept"
    );
    let all: Vec<&OpenPort> = open.iter().collect();
    for (proto, dports) in proto_ports(&all) {
        let _ = writeln!(out, "    iifname {iface} {proto} dport {dports} accept");
    }
    for family in Family::BOTH {
        drop_rule(
            out,
            &format!("iifname {iface} {}", family.nfproto()),
            family,
            Reason::Policy,
        );
    }
}

fn vpn_forward(out: &mut String, vpn: &VpnPolicy) {
    let iface = quote(&vpn.interface);
    let _ = writeln!(out, "    iifname {iface} ip saddr @vpn_full accept");
    let _ = writeln!(
        out,
        "    iifname {iface} ip saddr . ip daddr . meta l4proto . th dport @vpn_allowed accept"
    );
    for family in Family::BOTH {
        drop_rule(
            out,
            &format!("iifname {iface} {}", family.nfproto()),
            family,
            Reason::Policy,
        );
    }
}

/// Peers' traffic leaves with this machine's address, so the LAN and the
/// internet answer without routes back to the VPN.
fn vpn_nat(out: &mut String, vpn: &VpnPolicy) {
    let _ = writeln!(
        out,
        "  chain postrouting {{\n    type nat hook postrouting priority srcnat; policy accept;\n    \
         ip saddr {} oifname != {} masquerade\n  }}",
        vpn.subnet,
        quote(&vpn.interface)
    );
}

/// Keeps the managed containers' network to itself, VPN peers (already
/// accepted or dropped by [`vpn_forward`]) and published ports: Docker
/// does not drop packets routed to it from other interfaces, so a LAN
/// host with a route through this machine, or a container on another
/// network, could otherwise reach every port.
fn containers_forward(out: &mut String, net: Ipv4Net) {
    let net = net.trunc();
    drop_rule(
        out,
        &format!("ip daddr {net} ip saddr != {net} ct state new ct status & dnat == 0"),
        Family::V4,
        Reason::Policy,
    );
}

/// The table without the firewall: the VPN's rules and the containers'
/// network guard.
fn unfiltered(out: &mut String, inputs: &Inputs) {
    if let Some(vpn) = &inputs.vpn {
        out.push_str(
            "  chain input {\n    type filter hook input priority filter; policy accept;\n",
        );
        out.push_str("    ct state established,related accept\n");
        vpn_input(out, vpn, &inputs.open);
        out.push_str("  }\n");
    }
    if inputs.vpn.is_some() || inputs.containers.is_some() {
        out.push_str(
            "  chain forward {\n    type filter hook forward priority filter; policy accept;\n",
        );
        out.push_str("    ct state established,related accept\n");
        if let Some(vpn) = &inputs.vpn {
            vpn_forward(out, vpn);
        }
        if let Some(net) = inputs.containers {
            containers_forward(out, net);
        }
        out.push_str("  }\n");
    }
    if let Some(vpn) = &inputs.vpn {
        vpn_nat(out, vpn);
    }
    out.push_str("}\n");
}

/// Script that removes the table (when the firewall is turned off).
#[must_use]
pub fn remove() -> String {
    format!("add table inet {TABLE}\ndelete table inet {TABLE}\n")
}

/// Script that bans `ip` for `time` without replacing the table.
#[must_use]
pub fn ban(ip: IpAddr, time: Duration) -> String {
    let family = Family::of(ip);
    format!(
        "add element inet {TABLE} ban{} {{ {ip} timeout {}s }}\n",
        family.suffix(),
        time.as_secs().max(1)
    )
}

/// Script that lifts a ban.
#[must_use]
pub fn unban(ip: IpAddr) -> String {
    format!(
        "delete element inet {TABLE} ban{} {{ {ip} }}\n",
        Family::of(ip).suffix()
    )
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Family {
    V4,
    V6,
}

impl Family {
    const BOTH: [Self; 2] = [Self::V4, Self::V6];

    fn of(ip: IpAddr) -> Self {
        if ip.is_ipv4() { Self::V4 } else { Self::V6 }
    }
    fn suffix(self) -> &'static str {
        match self {
            Self::V4 => "4",
            Self::V6 => "6",
        }
    }
    fn addr(self) -> &'static str {
        match self {
            Self::V4 => "ip",
            Self::V6 => "ip6",
        }
    }
    fn nfproto(self) -> &'static str {
        match self {
            Self::V4 => "meta nfproto ipv4",
            Self::V6 => "meta nfproto ipv6",
        }
    }
}

/// Two rules: remember the source (for TCP and UDP) and drop.
fn drop_rule(out: &mut String, matcher: &str, family: Family, reason: Reason) {
    let (f, a, k) = (family.suffix(), family.addr(), reason.key());
    let _ = writeln!(
        out,
        "    {matcher} meta l4proto {{ tcp, udp }} update @recent_{k}{f} {{ {a} saddr . meta l4proto . th dport }}"
    );
    let _ = writeln!(out, "    {matcher} counter name dropped_{k} drop");
}

fn interval_set(out: &mut String, name: &str, nets: &[IpNet]) {
    for (family, ty) in [(Family::V4, "ipv4_addr"), (Family::V6, "ipv6_addr")] {
        let elements: Vec<String> = nets
            .iter()
            .filter(|n| Family::of(n.addr()) == family)
            .map(|n| n.trunc().to_string())
            .collect();
        let _ = write!(
            out,
            "  set {name}{} {{ type {ty}; flags interval; auto-merge;",
            family.suffix()
        );
        if !elements.is_empty() {
            let _ = write!(out, " elements = {{ {} }}", elements.join(", "));
        }
        out.push_str(" }\n");
    }
}

fn ban_sets(out: &mut String, bans: &[(IpAddr, Duration)]) {
    for (family, ty) in [(Family::V4, "ipv4_addr"), (Family::V6, "ipv6_addr")] {
        let elements: Vec<String> = bans
            .iter()
            .filter(|(ip, left)| Family::of(*ip) == family && !left.is_zero())
            .map(|(ip, left)| format!("{ip} timeout {}s", left.as_secs().max(1)))
            .collect();
        let _ = write!(
            out,
            "  set ban{} {{ type {ty}; flags timeout;",
            family.suffix()
        );
        if !elements.is_empty() {
            let _ = write!(out, " elements = {{ {} }}", elements.join(", "));
        }
        out.push_str(" }\n");
    }
}

/// Open ports grouped by rate limit, smallest limit first.
fn rate_groups(open: &[OpenPort]) -> Vec<(u32, Vec<&OpenPort>)> {
    let mut groups: Vec<(u32, Vec<&OpenPort>)> = Vec::new();
    let mut limited: Vec<&OpenPort> = open.iter().filter(|p| p.rate_limit.is_some()).collect();
    limited.sort_by_key(|p| p.rate_limit);
    for port in limited {
        let rate = port.rate_limit.unwrap_or_default();
        match groups.iter_mut().find(|(r, _)| *r == rate) {
            Some((_, ports)) => ports.push(port),
            None => groups.push((rate, vec![port])),
        }
    }
    groups
}

/// `(protocol, port set)` pairs for the given ports.
fn proto_ports(ports: &[&OpenPort]) -> Vec<(&'static str, String)> {
    let mut out = Vec::new();
    for (proto, pick) in [
        ("tcp", (|p: &OpenPort| p.tcp) as fn(&OpenPort) -> bool),
        ("udp", |p: &OpenPort| p.udp),
    ] {
        let mut numbers: Vec<u16> = ports.iter().filter(|p| pick(p)).map(|p| p.port).collect();
        numbers.sort_unstable();
        numbers.dedup();
        if !numbers.is_empty() {
            let list: Vec<String> = numbers.iter().map(ToString::to_string).collect();
            out.push((proto, format!("{{ {} }}", list.join(", "))));
        }
    }
    out
}

/// The match expressions for a rule: one per address family that has
/// sources (or one for both when there are none), combined with ports.
fn rule_matchers(rule: &hearthgate_model::FirewallRule) -> Vec<String> {
    let mut sources = Vec::new();
    if rule.sources.is_empty() {
        sources.push(String::new());
    } else {
        for family in Family::BOTH {
            let nets: Vec<String> = rule
                .sources
                .iter()
                .filter(|n| Family::of(n.addr()) == family)
                .map(|n| n.trunc().to_string())
                .collect();
            if !nets.is_empty() {
                sources.push(format!("{} saddr {{ {} }}", family.addr(), nets.join(", ")));
            }
        }
    }
    let ports = port_match(&rule.ports, rule.protocol);
    sources
        .into_iter()
        .map(|s| match (s.is_empty(), ports.is_empty()) {
            (true, _) => ports.clone(),
            (false, true) => s,
            (false, false) => format!("{s} {ports}"),
        })
        .collect()
}

fn port_match(ports: &[PortRange], protocol: Protocol) -> String {
    if ports.is_empty() {
        return String::new();
    }
    let list: Vec<String> = ports.iter().map(ToString::to_string).collect();
    let set = format!("{{ {} }}", list.join(", "));
    match protocol {
        Protocol::Tcp => format!("tcp dport {set}"),
        Protocol::Udp => format!("udp dport {set}"),
        Protocol::Both => format!("meta l4proto {{ tcp, udp }} th dport {set}"),
    }
}

/// The address family a matcher is limited to, if any.
fn matcher_family(matcher: &str) -> Option<Family> {
    if matcher.starts_with("ip6 ") {
        Some(Family::V6)
    } else if matcher.starts_with("ip ") {
        Some(Family::V4)
    } else {
        None
    }
}

/// Quotes an interface name for nft. Names are validated by the kernel
/// (at most 15 bytes, no spaces or quotes), so this only needs to refuse
/// anything odd.
fn quote(name: &str) -> String {
    let clean: String = name
        .chars()
        .filter(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.'))
        .take(15)
        .collect();
    format!("\"{clean}\"")
}

#[cfg(test)]
mod tests {
    use super::*;

    const GUARD: &str =
        "ip daddr 172.18.0.0/16 ip saddr != 172.18.0.0/16 ct state new ct status & dnat == 0";

    #[test]
    fn guards_the_containers_network_with_the_firewall_off() {
        let inputs = Inputs {
            containers: Some("172.18.0.0/16".parse().unwrap()),
            ..Inputs::default()
        };
        assert!(inputs.needs_table());
        let script = render(&inputs, &Dynamic::default());
        assert!(script.contains("chain forward"), "{script}");
        assert!(script.contains(GUARD), "{script}");
        // Nothing else is filtered.
        assert!(!script.contains("chain input"), "{script}");
        assert!(!script.contains("chain postrouting"), "{script}");
    }

    #[test]
    fn guard_comes_after_the_vpn_peers() {
        for enabled in [false, true] {
            let inputs = Inputs {
                firewall: Firewall {
                    enabled,
                    ..Firewall::default()
                },
                vpn: Some(VpnPolicy {
                    interface: "hearthgate".into(),
                    subnet: "10.88.0.0/24".parse().unwrap(),
                    server: Ipv4Addr::new(10, 88, 0, 1),
                    dns: true,
                    full: vec![Ipv4Addr::new(10, 88, 0, 2)],
                    allowed: Vec::new(),
                }),
                containers: Some("172.18.0.0/16".parse().unwrap()),
                ..Inputs::default()
            };
            let script = render(&inputs, &Dynamic::default());
            let forward = &script[script.find("chain forward").unwrap()..];
            let peers = forward
                .find("iifname \"hearthgate\" ip saddr @vpn_full accept")
                .unwrap();
            let guard = forward.find(GUARD).unwrap();
            assert!(peers < guard, "{script}");
        }
    }

    #[test]
    fn no_table_without_anything_to_filter() {
        assert!(!Inputs::default().needs_table());
    }
}
