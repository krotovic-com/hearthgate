use std::collections::{BTreeMap, BTreeSet};
use std::fmt;
use std::net::IpAddr;

use crate::hostname::{host_matches, is_valid_hostname};
use crate::{DesiredState, Interface, ListenerMode, PortRef, Route, Service, ServiceKind};

/// One problem found in a desired state.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ValidationError {
    /// Where the problem is, for example `routes[app].targets[0]`.
    pub path: String,
    /// What is wrong, in plain words.
    pub message: String,
}

impl fmt::Display for ValidationError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}: {}", self.path, self.message)
    }
}

/// Every problem found in a desired state.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ValidationErrors(pub Vec<ValidationError>);

impl fmt::Display for ValidationErrors {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let n = self.0.len();
        writeln!(f, "{n} problem{} found:", if n == 1 { "" } else { "s" })?;
        for error in &self.0 {
            writeln!(f, "  - {error}")?;
        }
        Ok(())
    }
}

impl std::error::Error for ValidationErrors {}

#[derive(Default)]
struct Collector(Vec<ValidationError>);

impl Collector {
    fn push(&mut self, path: impl Into<String>, message: impl Into<String>) {
        self.0.push(ValidationError {
            path: path.into(),
            message: message.into(),
        });
    }
}

fn is_valid_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 63
        && !name.starts_with('-')
        && !name.ends_with('-')
        && name
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
}

const NAME_RULE: &str = "names use lowercase letters, digits and dashes (at most 63 characters)";

/// Transport a socket group uses.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum Transport {
    Tcp,
    Udp,
}

impl DesiredState {
    /// Checks every rule a desired state must follow before it can be
    /// applied. Returns all problems at once rather than the first one.
    pub fn validate(&self) -> Result<(), ValidationErrors> {
        let mut c = Collector::default();
        self.validate_settings(&mut c);
        self.validate_names(&mut c);
        for interface in &self.interfaces {
            validate_interface(interface, &mut c);
        }
        self.validate_shared_ports(&mut c);
        for service in &self.services {
            validate_service(service, &mut c);
        }
        self.validate_routes(&mut c);
        if c.0.is_empty() {
            Ok(())
        } else {
            Err(ValidationErrors(c.0))
        }
    }

    fn validate_settings(&self, c: &mut Collector) {
        let l = &self.limits;
        for (name, zero) in [
            ("max_body_bytes", l.max_body_bytes == 0),
            ("header_timeout", l.header_timeout.is_zero()),
            ("upstream_timeout", l.upstream_timeout.is_zero()),
            ("connect_timeout", l.connect_timeout.is_zero()),
            ("tcp_idle_timeout", l.tcp_idle_timeout.is_zero()),
            ("udp_session_timeout", l.udp_session_timeout.is_zero()),
        ] {
            if zero {
                c.push(format!("limits.{name}"), "must be greater than zero");
            }
        }
        if let Some(email) = &self.acme.email {
            if !email.contains('@') {
                c.push("acme.email", "is not an email address");
            }
        }
        if let Some(url) = &self.acme.directory {
            if !url.starts_with("https://") && !url.starts_with("http://") {
                c.push("acme.directory", "must be an http(s) URL");
            }
        }
        if self.interfaces.iter().any(Interface::wants_acme) && !self.acme.accept_terms {
            c.push(
                "acme.accept_terms",
                "an https interface needs an automatic certificate: set acme.accept_terms = true \
                 to agree to the certificate authority's terms of service, or give the interface \
                 a manual certificate",
            );
        }
    }

    fn validate_names(&self, c: &mut Collector) {
        let groups: [(&str, Vec<&str>); 3] = [
            (
                "interfaces",
                self.interfaces.iter().map(|i| i.name.as_str()).collect(),
            ),
            (
                "services",
                self.services.iter().map(|s| s.name.as_str()).collect(),
            ),
            (
                "routes",
                self.routes.iter().map(|r| r.name.as_str()).collect(),
            ),
        ];
        for (kind, names) in groups {
            let mut seen = BTreeSet::new();
            for name in names {
                if !is_valid_name(name) {
                    c.push(
                        format!("{kind}[{name}]"),
                        format!("invalid name: {NAME_RULE}"),
                    );
                }
                if !seen.insert(name) {
                    c.push(format!("{kind}[{name}]"), "name is used more than once");
                }
            }
        }
    }

    /// Several interfaces may share a port when the traffic can be told
    /// apart: TLS by SNI, HTTP by Host. Raw forwarding owns its port.
    fn validate_shared_ports(&self, c: &mut Collector) {
        type Key = (u16, Transport);
        let mut groups: BTreeMap<Key, Vec<(&Interface, ListenerMode)>> = BTreeMap::new();
        for interface in &self.interfaces {
            for listener in &interface.listeners {
                if listener.tcp() {
                    groups
                        .entry((listener.port, Transport::Tcp))
                        .or_default()
                        .push((interface, listener.mode));
                }
                if listener.udp() {
                    groups
                        .entry((listener.port, Transport::Udp))
                        .or_default()
                        .push((interface, listener.mode));
                }
            }
        }
        for ((port, transport), members) in groups {
            if members.len() < 2 {
                continue;
            }
            let proto = match transport {
                Transport::Tcp => "tcp",
                Transport::Udp => "udp",
            };
            let names: Vec<&str> = members.iter().map(|(i, _)| i.name.as_str()).collect();
            let path = format!("port {port}/{proto}");
            let modes: BTreeSet<ListenerMode> = members.iter().map(|(_, m)| *m).collect();
            if modes.contains(&ListenerMode::Raw) {
                c.push(
                    path,
                    format!(
                        "raw forwarding cannot share a port; it is used by {}",
                        names.join(", ")
                    ),
                );
                continue;
            }
            if modes.contains(&ListenerMode::Http) && modes.len() > 1 {
                c.push(
                    path,
                    format!(
                        "plain http and TLS cannot share a port; it is used by {}",
                        names.join(", ")
                    ),
                );
                continue;
            }
            let first_bind: BTreeSet<IpAddr> = members[0].0.bind.iter().copied().collect();
            if members
                .iter()
                .any(|(i, _)| i.bind.iter().copied().collect::<BTreeSet<_>>() != first_bind)
            {
                c.push(
                    &path,
                    format!(
                        "interfaces sharing a port must bind the same addresses: {}",
                        names.join(", ")
                    ),
                );
            }
            let mut owner: BTreeMap<&str, &str> = BTreeMap::new();
            for (interface, _) in &members {
                if interface.domains.is_empty() {
                    c.push(
                        &path,
                        format!(
                            "interface {} has no domains, so it cannot share the port with {}",
                            interface.name,
                            names.join(", ")
                        ),
                    );
                }
                for domain in &interface.domains {
                    if let Some(other) = owner.insert(domain, &interface.name) {
                        if other != interface.name {
                            c.push(
                                &path,
                                format!(
                                    "domain {domain} is served by both {other} and {}",
                                    interface.name
                                ),
                            );
                        }
                    }
                }
            }
        }
    }

    fn validate_routes(&self, c: &mut Collector) {
        // (interface, listener, host, path) must be unique.
        let mut seen: BTreeSet<(&str, u16, String, &str)> = BTreeSet::new();
        let mut raw_owner: BTreeMap<(&str, u16), &str> = BTreeMap::new();
        for route in &self.routes {
            let path = format!("routes[{}]", route.name);
            let Some(interface) = self.interface(&route.interface) else {
                c.push(
                    &path,
                    format!("interface {} does not exist", route.interface),
                );
                continue;
            };
            let Some(listener) = interface.listener(route.listener) else {
                c.push(
                    &path,
                    format!(
                        "interface {} has no listener on port {}",
                        interface.name, route.listener
                    ),
                );
                continue;
            };
            self.validate_targets(route, &path, c);
            if route.strip_prefix && route.path_prefix == "/" {
                c.push(&path, "strip_prefix needs a path_prefix other than /");
            }
            match listener.mode {
                ListenerMode::Raw => {
                    if !route.hosts.is_empty() || route.path_prefix != "/" || route.strip_prefix {
                        c.push(
                            &path,
                            "raw listeners forward everything: hosts and path_prefix do not apply",
                        );
                    }
                    if let Some(other) =
                        raw_owner.insert((&interface.name, listener.port), &route.name)
                    {
                        c.push(
                            &path,
                            format!(
                                "raw listener {} on {} already has route {other}; use several \
                                 targets in one route to load-balance",
                                listener.port, interface.name
                            ),
                        );
                    }
                }
                ListenerMode::TlsPassthrough | ListenerMode::Http | ListenerMode::Https => {
                    if listener.mode == ListenerMode::TlsPassthrough && route.path_prefix != "/" {
                        c.push(
                            &path,
                            "tls-passthrough routes match by host only; path_prefix does not apply",
                        );
                    }
                    if !route.path_prefix.starts_with('/') {
                        c.push(&path, "path_prefix must start with /");
                    }
                    for host in &route.hosts {
                        if !is_valid_hostname(host, true) {
                            c.push(&path, format!("invalid host {host}"));
                        } else if !interface.domains.is_empty()
                            && !interface
                                .domains
                                .iter()
                                .any(|d| d == host || host_matches(d, host))
                        {
                            c.push(
                                &path,
                                format!(
                                    "host {host} is not one of interface {}'s domains",
                                    interface.name
                                ),
                            );
                        }
                    }
                    let hosts: Vec<String> = if route.hosts.is_empty() {
                        if interface.domains.is_empty() {
                            vec!["*".to_owned()]
                        } else {
                            interface.domains.clone()
                        }
                    } else {
                        route.hosts.clone()
                    };
                    for host in hosts {
                        let key = (
                            interface.name.as_str(),
                            listener.port,
                            host,
                            route.path_prefix.as_str(),
                        );
                        if seen.contains(&key) {
                            c.push(
                                &path,
                                format!(
                                    "another route already matches host {} and path {} on port {}",
                                    key.2, key.3, key.1
                                ),
                            );
                        } else {
                            seen.insert(key);
                        }
                    }
                }
            }
        }
    }

    fn validate_targets(&self, route: &Route, path: &str, c: &mut Collector) {
        if route.targets.is_empty() {
            c.push(path, "needs at least one target");
        }
        for (n, target) in route.targets.iter().enumerate() {
            let tpath = format!("{path}.targets[{n}]");
            if target.weight == 0 {
                c.push(&tpath, "weight must be at least 1");
            }
            match self.service(&target.service) {
                None => c.push(&tpath, format!("service {} does not exist", target.service)),
                Some(service) => match service.resolve_port(&target.port) {
                    None => c.push(
                        &tpath,
                        format!("service {} has no port named {}", service.name, target.port),
                    ),
                    Some(0) => c.push(&tpath, "port 0 is not a valid target"),
                    Some(_) => {}
                },
            }
        }
    }
}

fn validate_interface(interface: &Interface, c: &mut Collector) {
    let path = format!("interfaces[{}]", interface.name);
    let wildcard_ok = interface.certificate.is_some()
        || !interface
            .listeners
            .iter()
            .any(|l| l.mode == ListenerMode::Https);
    for domain in &interface.domains {
        if !is_valid_hostname(domain, true) {
            c.push(&path, format!("invalid domain {domain}"));
        } else if domain.starts_with("*.") && !wildcard_ok {
            c.push(
                &path,
                format!(
                    "wildcard domain {domain} needs a manual certificate (automatic wildcard \
                     certificates need DNS challenges, planned for a later release)"
                ),
            );
        }
    }
    if let Some(cert) = &interface.certificate {
        if !is_valid_name(cert) {
            c.push(
                &path,
                format!("invalid certificate name {cert}: {NAME_RULE}"),
            );
        }
    }
    if interface.listeners.is_empty() {
        c.push(&path, "needs at least one listener");
    }
    let mut ports = BTreeSet::new();
    for listener in &interface.listeners {
        let lpath = format!("{path}.listeners[{}]", listener.port);
        if listener.port == 0 {
            c.push(&lpath, "port 0 is not allowed");
        }
        if !ports.insert(listener.port) {
            c.push(&lpath, "port is listed more than once");
        }
        if listener.mode != ListenerMode::Raw && listener.udp() {
            c.push(
                &lpath,
                format!(
                    "{} listeners are TCP only; use mode raw for UDP",
                    listener.mode
                ),
            );
        }
        if listener.mode == ListenerMode::Https && interface.domains.is_empty() {
            c.push(
                &lpath,
                "https listeners need at least one domain on the interface",
            );
        }
    }
}

fn validate_service(service: &Service, c: &mut Collector) {
    let path = format!("services[{}]", service.name);
    match service.kind {
        ServiceKind::External => {
            match &service.host {
                None => c.push(&path, "external services need a host"),
                Some(host) => {
                    if host.parse::<IpAddr>().is_err()
                        && !is_valid_hostname(&host.to_ascii_lowercase(), false)
                    {
                        c.push(&path, format!("invalid host {host}"));
                    }
                }
            }
            if service.container.is_some() || service.network.is_some() {
                c.push(&path, "container and network only apply to docker services");
            }
        }
        ServiceKind::Docker => {
            if service.container.as_deref().is_none_or(str::is_empty) {
                c.push(&path, "docker services need a container name");
            }
            if service.host.is_some() {
                c.push(
                    &path,
                    "docker services take their address from the container; remove host",
                );
            }
        }
    }
    if service.ports.is_empty() {
        c.push(&path, "needs at least one port");
    }
    for (name, port) in &service.ports {
        if !is_valid_name(name) {
            c.push(&path, format!("invalid port name {name}: {NAME_RULE}"));
        }
        if *port == 0 {
            c.push(&path, format!("port {name} is 0"));
        }
    }
    if let Some(health) = &service.health {
        let hpath = format!("{path}.health");
        if health.interval.is_zero() || health.timeout.is_zero() {
            c.push(&hpath, "interval and timeout must be greater than zero");
        }
        if health.timeout > health.interval {
            c.push(&hpath, "timeout must not be longer than interval");
        }
        if health.healthy_threshold == 0 || health.unhealthy_threshold == 0 {
            c.push(&hpath, "thresholds must be at least 1");
        }
        if !health.path.starts_with('/') {
            c.push(&hpath, "path must start with /");
        }
        if let Some(port) = &health.port {
            if matches!(port, PortRef::Name(_)) && service.resolve_port(port).is_none() {
                c.push(&hpath, format!("service has no port named {port}"));
            }
        }
    }
}
