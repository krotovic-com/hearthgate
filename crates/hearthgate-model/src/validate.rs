use std::collections::{BTreeMap, BTreeSet};
use std::fmt;
use std::net::IpAddr;

use crate::hostname::{host_matches, is_valid_hostname};
use crate::{
    Access, DesiredState, Interface, ListenerMode, PortRef, Route, RuleAction, Service,
    ServiceKind, is_wireguard_key, split_endpoint,
};

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

/// Whether `name` is a valid name for interfaces, services, routes, rules
/// and peers.
#[must_use]
pub fn is_valid_name(name: &str) -> bool {
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
            validate_interface(
                interface,
                self.acme.challenge == Some(crate::AcmeChallenge::Dns01),
                &mut c,
            );
        }
        self.validate_shared_ports(&mut c);
        for service in &self.services {
            validate_service(service, &mut c);
        }
        self.validate_routes(&mut c);
        self.validate_firewall(&mut c);
        self.validate_vpn(&mut c);
        self.validate_docker(&mut c);
        self.validate_notifications(&mut c);
        self.validate_ai(&mut c);
        if self.updates.window_minutes().is_none() {
            c.push("updates.window", "must be two times such as 03:00-05:00");
        }
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
        if let Some(email) = &self.acme.email
            && !email.contains('@')
        {
            c.push("acme.email", "is not an email address");
        }
        if let Some(url) = &self.acme.directory
            && !url.starts_with("https://")
            && !url.starts_with("http://")
        {
            c.push("acme.directory", "must be an http(s) URL");
        }
        if self.acme.renew_before.is_zero() {
            c.push("acme.renew_before", "must be greater than zero");
        }
        for interface in self.interfaces.iter().filter(|i| i.wants_acme()) {
            let has_http = interface
                .listeners
                .iter()
                .any(|l| l.mode == ListenerMode::Http);
            if self.acme.challenge == Some(crate::AcmeChallenge::Http01) && !has_http {
                c.push(
                    format!("interfaces[{}]", interface.name),
                    "acme.challenge is http-01, but the interface has no http listener to answer it",
                );
            }
        }
        let dns01 = self.acme.challenge == Some(crate::AcmeChallenge::Dns01);
        if dns01 && self.acme.dns.is_none() {
            c.push(
                "acme.dns",
                "acme.challenge is dns-01, so set a DNS provider in [acme.dns]",
            );
        }
        if let Some(dns) = &self.acme.dns {
            validate_acme_dns(dns, c);
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

    fn validate_vpn(&self, c: &mut Collector) {
        let vpn = &self.vpn;
        if vpn.port == 0 {
            c.push("vpn.port", "must be a port from 1 to 65535");
        }
        let udp_taken = self
            .interfaces
            .iter()
            .any(|i| i.listeners.iter().any(|l| l.port == vpn.port && l.udp()));
        if vpn.enabled && udp_taken {
            c.push(
                "vpn.port",
                format!("UDP port {} is used by a public interface", vpn.port),
            );
        }
        if !(16..=30).contains(&vpn.subnet.prefix_len()) {
            c.push(
                "vpn.subnet",
                "use a network from /16 to /30, such as 10.88.0.0/24",
            );
        }
        if !vpn.subnet.network().is_private() {
            c.push(
                "vpn.subnet",
                "use a private network (10/8, 172.16/12 or 192.168/16)",
            );
        }
        if let Some(endpoint) = &vpn.endpoint {
            let valid = split_endpoint(endpoint).is_some_and(|(host, _)| {
                host.parse::<IpAddr>().is_ok() || is_valid_hostname(host, false)
            });
            if !valid {
                c.push(
                    "vpn.endpoint",
                    "use a host name or address, with an optional :port",
                );
            }
        }
        let server = vpn.server_address();
        let mut names = BTreeSet::new();
        let mut keys = BTreeSet::new();
        let mut addresses = BTreeSet::new();
        for peer in &vpn.peers {
            let path = format!("vpn.peers[{}]", peer.name);
            if !is_valid_name(&peer.name) {
                c.push(&path, format!("invalid name: {NAME_RULE}"));
            }
            if !names.insert(peer.name.as_str()) {
                c.push(&path, "name is used more than once");
            }
            if !is_wireguard_key(&peer.public_key) {
                c.push(
                    &path,
                    "public_key is not a WireGuard key (44 characters of base64)",
                );
            }
            if !keys.insert(peer.public_key.as_str()) {
                c.push(&path, "another peer has the same public key");
            }
            let ip = peer.address;
            if !vpn.subnet.contains(&ip)
                || ip == vpn.subnet.network()
                || ip == vpn.subnet.broadcast()
            {
                c.push(
                    &path,
                    format!("address {ip} is not a host address in {}", vpn.subnet),
                );
            } else if ip == server {
                c.push(&path, format!("address {ip} belongs to this machine"));
            }
            if !addresses.insert(ip) {
                c.push(&path, format!("another peer has the address {ip}"));
            }
            if let Access::Services(list) = &peer.access {
                for name in list {
                    if !self.services.iter().any(|s| &s.name == name) {
                        c.push(&path, format!("access: no service is called `{name}`"));
                    }
                }
            }
        }
        let dns = &self.dns;
        if !is_valid_hostname(&dns.zone, false) {
            c.push("dns.zone", "use a domain such as home.internal");
        }
        for upstream in &dns.upstream {
            let ok = upstream.parse::<IpAddr>().is_ok()
                || upstream.parse::<std::net::SocketAddr>().is_ok();
            if !ok {
                c.push(
                    "dns.upstream",
                    format!("`{upstream}` is not an address, like 1.1.1.1 or 192.168.1.1:53"),
                );
            }
        }
    }

    fn validate_ai(&self, c: &mut Collector) {
        let ai = &self.ai;
        if let Some(model) = &ai.model
            && (model.trim().is_empty() || model.len() > 200 || model.contains(char::is_whitespace))
        {
            c.push("ai.model", "must be a model name without spaces");
        }
        if ai.model_or_default().is_none() && ai.enabled {
            c.push("ai.model", "is required for this provider");
        }
        if let Some(endpoint) = &ai.endpoint
            && !(endpoint.starts_with("https://") || endpoint.starts_with("http://"))
        {
            c.push("ai.endpoint", "must be an http(s) URL");
        }
        if ai.endpoint_or_default().is_none() && ai.enabled {
            c.push("ai.endpoint", "is required for an OpenAI-compatible server");
        }
        if let Some(file) = &ai.api_key_file
            && !file.starts_with('/')
        {
            c.push("ai.api_key_file", "must be an absolute path");
        }
        for (i, pattern) in ai.redact.iter().enumerate() {
            if pattern.is_empty() || pattern.len() > 500 {
                c.push(format!("ai.redact[{i}]"), "must be 1 to 500 characters");
            } else if let Err(e) = regex::Regex::new(pattern) {
                c.push(
                    format!("ai.redact[{i}]"),
                    format!("not a valid regular expression: {e}"),
                );
            } else if regex::Regex::new(pattern).is_ok_and(|r| r.is_match("")) {
                c.push(format!("ai.redact[{i}]"), "must not match empty text");
            }
        }
        if ai.timeout < std::time::Duration::from_secs(5)
            || ai.timeout > std::time::Duration::from_secs(900)
        {
            c.push("ai.timeout", "must be between 5s and 15m");
        }
    }

    fn validate_notifications(&self, c: &mut Collector) {
        let n = &self.notifications;
        let mut seen = std::collections::BTreeSet::new();
        for hook in &n.webhooks {
            let path = format!("notifications.webhooks[{}]", hook.name);
            if !is_valid_name(&hook.name) {
                c.push(&path, "invalid name (use lowercase letters, digits and -)");
            } else if !seen.insert(hook.name.as_str()) {
                c.push(&path, "duplicate name");
            }
            if !hook.url.starts_with("https://") && !hook.url.starts_with("http://") {
                c.push(&path, "url must be an http(s) URL");
            }
        }
        if let Some(email) = &n.email {
            if crate::split_endpoint(&email.server).is_none() {
                c.push("notifications.email.server", "must be host or host:port");
            }
            if !crate::is_email(&email.from) {
                c.push("notifications.email.from", "is not an email address");
            }
            if email.to.is_empty() {
                c.push("notifications.email.to", "needs at least one address");
            }
            for to in &email.to {
                if !crate::is_email(to) {
                    c.push(
                        "notifications.email.to",
                        format!("{to} is not an email address"),
                    );
                }
            }
            if let Some(file) = &email.password_file
                && !file.starts_with('/')
            {
                c.push(
                    "notifications.email.password_file",
                    "must be an absolute path",
                );
            }
            if email.password_file.is_some() && email.username.is_none() {
                c.push("notifications.email.username", "is needed with a password");
            }
        }
    }

    fn validate_docker(&self, c: &mut Collector) {
        let mut names = BTreeSet::new();
        for container in &self.containers {
            let path = format!("containers[{}]", container.name);
            if !is_valid_name(&container.name) {
                c.push(&path, format!("invalid name: {NAME_RULE}"));
            }
            if !names.insert(container.name.as_str()) {
                c.push(&path, "name is used more than once");
            }
            if container.image.is_empty() || container.image.contains(char::is_whitespace) {
                c.push(&path, "image: use a reference such as nginx:1.27");
            }
            for key in container.env.keys() {
                if !crate::is_env_name(key) {
                    c.push(&path, format!("env: `{key}` is not a variable name"));
                }
            }
            for volume in &container.volumes {
                if crate::Volume::parse(volume).is_none() {
                    c.push(
                        &path,
                        format!(
                            "volumes: `{volume}` is not /host/path:/path[:ro] or name:/path[:ro]"
                        ),
                    );
                }
            }
            for publish in &container.publish {
                match crate::Publish::parse(publish) {
                    None => c.push(
                        &path,
                        format!("publish: `{publish}` is not [address:]host:container[/udp]"),
                    ),
                    Some(p) => {
                        let taken = self.interfaces.iter().any(|i| {
                            i.listeners
                                .iter()
                                .any(|l| l.port == p.host && if p.udp { l.udp() } else { l.tcp() })
                        });
                        if taken {
                            c.push(
                                &path,
                                format!("publish: port {} is used by a public interface", p.host),
                            );
                        }
                    }
                }
            }
        }
        let mut stacks = BTreeSet::new();
        for stack in &self.stacks {
            let path = format!("stacks[{}]", stack.name);
            if !is_valid_name(&stack.name) {
                c.push(&path, format!("invalid name: {NAME_RULE}"));
            }
            if !stacks.insert(stack.name.as_str()) {
                c.push(&path, "name is used more than once");
            }
            if stack.compose.trim().is_empty() {
                c.push(&path, "compose: the Compose file is empty");
            }
            for key in stack.env.keys() {
                if !crate::is_env_name(key) {
                    c.push(&path, format!("env: `{key}` is not a variable name"));
                }
            }
        }
    }

    fn validate_firewall(&self, c: &mut Collector) {
        let fw = &self.firewall;
        let mut seen = BTreeSet::new();
        for rule in &fw.rules {
            let path = format!("firewall.rules[{}]", rule.name);
            if !is_valid_name(&rule.name) {
                c.push(&path, format!("invalid name: {NAME_RULE}"));
            }
            if !seen.insert(rule.name.as_str()) {
                c.push(&path, "name is used more than once");
            }
            if rule.sources.is_empty() && rule.ports.is_empty() && rule.action == RuleAction::Allow
            {
                c.push(
                    &path,
                    "allows everything from everywhere; give it sources or ports",
                );
            }
        }
        let mut seen = BTreeSet::new();
        for list in &fw.blocklists {
            let path = format!("firewall.blocklists[{}]", list.name);
            if !is_valid_name(&list.name) {
                c.push(&path, format!("invalid name: {NAME_RULE}"));
            }
            if !seen.insert(list.name.as_str()) {
                c.push(&path, "name is used more than once");
            }
            if !list.url.starts_with("https://") {
                c.push(&path, "the URL must start with https://");
            }
            if list.refresh < std::time::Duration::from_secs(600) {
                c.push(&path, "refresh at most every 10 minutes");
            }
        }
        for country in &fw.countries {
            if country.len() != 2 || !country.bytes().all(|b| b.is_ascii_lowercase()) {
                c.push(
                    "firewall.countries",
                    format!("`{country}` is not a two-letter country code"),
                );
            }
        }
        for net in &fw.block {
            if crate::is_special_net(net) {
                c.push(
                    "firewall.block",
                    format!(
                        "{net} is a private or local network; block it with a deny rule instead"
                    ),
                );
            }
        }
        let ban = &fw.auto_ban;
        if ban.threshold == 0 {
            c.push("firewall.auto_ban.threshold", "must be greater than zero");
        }
        if ban.window.is_zero() || ban.ban_time.is_zero() {
            c.push(
                "firewall.auto_ban",
                "window and ban_time must be greater than zero",
            );
        }
        for interface in &self.interfaces {
            if interface.rate_limit == Some(0) {
                c.push(
                    format!("interfaces[{}].rate_limit", interface.name),
                    "must be greater than zero",
                );
            }
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
                    if let Some(other) = owner.insert(domain, &interface.name)
                        && other != interface.name
                    {
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

fn validate_acme_dns(dns: &crate::AcmeDns, c: &mut Collector) {
    let need = |c: &mut Collector, field: &str, value: Option<&String>, why: &str| {
        if value.is_none_or(|v| v.trim().is_empty()) {
            c.push(format!("acme.dns.{field}"), why.to_owned());
        }
    };
    let file = |c: &mut Collector, field: &str, value: Option<&String>| {
        if let Some(v) = value
            && !v.starts_with('/')
        {
            c.push(format!("acme.dns.{field}"), "must be an absolute path");
        }
    };
    match dns.provider {
        crate::DnsProvider::Cloudflare => {
            need(
                c,
                "api_token_file",
                dns.api_token_file.as_ref(),
                "Cloudflare needs a file with an API token",
            );
            file(c, "api_token_file", dns.api_token_file.as_ref());
            if let Some(url) = &dns.api_url
                && !url.starts_with("https://")
                && !url.starts_with("http://")
            {
                c.push("acme.dns.api_url", "must be an http(s) URL");
            }
        }
        crate::DnsProvider::Rfc2136 => {
            need(
                c,
                "server",
                dns.server.as_ref(),
                "the DNS server to update is needed",
            );
            need(c, "zone", dns.zone.as_ref(), "the zone to update is needed");
            need(
                c,
                "key_name",
                dns.key_name.as_ref(),
                "the TSIG key's name is needed",
            );
            need(
                c,
                "key_file",
                dns.key_file.as_ref(),
                "a file with the TSIG secret is needed",
            );
            file(c, "key_file", dns.key_file.as_ref());
            if let Some(zone) = &dns.zone
                && !is_valid_hostname(&crate::normalize_host(zone), false)
            {
                c.push("acme.dns.zone", format!("{zone} is not a domain"));
            }
            if let Some(server) = &dns.server
                && crate::split_endpoint(server).is_none()
            {
                c.push("acme.dns.server", "must be host or host:port");
            }
        }
    }
    if dns.propagation > std::time::Duration::from_secs(3600) {
        c.push("acme.dns.propagation", "must be at most 1h");
    }
}

fn validate_interface(interface: &Interface, dns01: bool, c: &mut Collector) {
    let path = format!("interfaces[{}]", interface.name);
    let wildcard_ok = dns01
        || interface.certificate.is_some()
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
                    "wildcard domain {domain} needs a manual certificate, or automatic \
                     certificates with acme.challenge = \"dns-01\""
                ),
            );
        }
    }
    if let Some(cert) = &interface.certificate
        && !is_valid_name(cert)
    {
        c.push(
            &path,
            format!("invalid certificate name {cert}: {NAME_RULE}"),
        );
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
        if let Some(port) = &health.port
            && matches!(port, PortRef::Name(_))
            && service.resolve_port(port).is_none()
        {
            c.push(&hpath, format!("service has no port named {port}"));
        }
    }
}
