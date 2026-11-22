//! Hearthgate's desired-state model.
//!
//! Everything a user configures (public interfaces, services and routes) is
//! *desired state*. This crate defines those types, how they are read from and
//! written to TOML and JSON, and the rules a state must follow before it can
//! be stored as a revision and applied.
//!
//! The types are deliberately plain data. Runtime concerns (sockets,
//! certificates, health) live in the subsystem crates, which compile a
//! validated [`DesiredState`] into their own runtime structures.

mod ai;
mod docker;
pub mod duration;
mod firewall;
mod hostname;
mod notify;
mod types;
mod validate;
mod vpn;

pub use ai::*;
pub use docker::*;
pub use firewall::*;
pub use hostname::{host_matches, is_valid_hostname, normalize_host};
pub use notify::*;
pub use types::*;
pub use validate::{ValidationError, ValidationErrors, is_valid_name};
pub use vpn::*;

/// Errors from reading a desired state document.
#[derive(Debug, thiserror::Error)]
pub enum ParseError {
    #[error("invalid TOML: {0}")]
    Toml(#[from] toml::de::Error),
    #[error("invalid JSON: {0}")]
    Json(#[from] serde_json::Error),
    #[error(transparent)]
    Invalid(#[from] ValidationErrors),
}

impl DesiredState {
    /// Parses a TOML document, fills in defaults and validates it.
    pub fn from_toml(text: &str) -> Result<Self, ParseError> {
        let state: Self = toml::from_str(text)?;
        let state = state.normalized();
        state.validate()?;
        Ok(state)
    }

    /// Renders the state as TOML, the format used by `export` and `import`.
    ///
    /// # Panics
    ///
    /// Never in practice: every model type serialises to TOML.
    #[must_use]
    pub fn to_toml(&self) -> String {
        toml::to_string_pretty(self).expect("desired state always serialises to TOML")
    }

    /// Unified diff from `self` to `new`, in their TOML form. Empty when
    /// they are equal. This is what revision history and the dashboard's
    /// review step show.
    #[must_use]
    pub fn diff(&self, new: &Self) -> String {
        use std::fmt::Write as _;
        let (old, new) = (self.to_toml(), new.to_toml());
        if old == new {
            return String::new();
        }
        let diff = similar::TextDiff::from_lines(&old, &new);
        let mut out = String::new();
        for hunk in diff.unified_diff().context_radius(3).iter_hunks() {
            let _ = write!(out, "{hunk}");
        }
        out
    }

    /// Parses the canonical JSON form used by the store.
    pub fn from_json(text: &str) -> Result<Self, ParseError> {
        Ok(serde_json::from_str(text)?)
    }

    /// Renders the canonical JSON form. Two equal states always produce the
    /// same bytes, so the store can hash it to detect no-op changes.
    ///
    /// # Panics
    ///
    /// Never in practice: every model type serialises to JSON.
    #[must_use]
    pub fn to_canonical_json(&self) -> String {
        serde_json::to_string(&self.clone().normalized())
            .expect("desired state always serialises to JSON")
    }

    /// Returns the state with names, hosts and domains lowercased and
    /// collections in a stable order, so equal configurations compare equal
    /// regardless of how they were written.
    #[must_use]
    pub fn normalized(mut self) -> Self {
        for interface in &mut self.interfaces {
            interface.name = interface.name.trim().to_owned();
            for domain in &mut interface.domains {
                *domain = normalize_host(domain);
            }
            interface.domains.sort();
            interface.domains.dedup();
            interface.bind.sort();
            interface.bind.dedup();
            interface.listeners.sort_by_key(|l| l.port);
        }
        self.interfaces.sort_by(|a, b| a.name.cmp(&b.name));
        for service in &mut self.services {
            service.name = service.name.trim().to_owned();
        }
        self.services.sort_by(|a, b| a.name.cmp(&b.name));
        for route in &mut self.routes {
            route.name = route.name.trim().to_owned();
            for host in &mut route.hosts {
                *host = normalize_host(host);
            }
            route.hosts.sort();
            route.hosts.dedup();
        }
        self.routes.sort_by(|a, b| a.name.cmp(&b.name));
        for container in &mut self.containers {
            container.name = container.name.trim().to_owned();
            container.image = container.image.trim().to_owned();
        }
        self.containers.sort_by(|a, b| a.name.cmp(&b.name));
        for stack in &mut self.stacks {
            stack.name = stack.name.trim().to_owned();
        }
        self.stacks.sort_by(|a, b| a.name.cmp(&b.name));
        let fw = &mut self.firewall;
        for rule in &mut fw.rules {
            rule.name = rule.name.trim().to_owned();
            rule.sources = ipnet::IpNet::aggregate(&rule.sources);
            rule.ports.sort();
            rule.ports.dedup();
        }
        fw.allow = ipnet::IpNet::aggregate(&fw.allow);
        fw.block = ipnet::IpNet::aggregate(&fw.block);
        for list in &mut fw.blocklists {
            list.name = list.name.trim().to_owned();
            list.url = list.url.trim().to_owned();
        }
        fw.blocklists.sort_by(|a, b| a.name.cmp(&b.name));
        for peer in &mut self.vpn.peers {
            peer.name = peer.name.trim().to_owned();
            peer.public_key = peer.public_key.trim().to_owned();
            if let Access::Services(list) = &mut peer.access {
                list.sort();
                list.dedup();
            }
        }
        self.vpn.peers.sort_by(|a, b| a.name.cmp(&b.name));
        self.vpn.subnet = self.vpn.subnet.trunc();
        if let Some(endpoint) = &mut self.vpn.endpoint {
            *endpoint = endpoint.trim().to_ascii_lowercase();
        }
        self.dns.zone = self.dns.zone.trim().trim_matches('.').to_ascii_lowercase();
        for country in &mut fw.countries {
            *country = country.trim().to_ascii_lowercase();
        }
        fw.countries.sort();
        fw.countries.dedup();
        self
    }

    /// Looks up an interface by name.
    #[must_use]
    pub fn interface(&self, name: &str) -> Option<&Interface> {
        self.interfaces.iter().find(|i| i.name == name)
    }

    /// Looks up a service by name.
    #[must_use]
    pub fn service(&self, name: &str) -> Option<&Service> {
        self.services.iter().find(|s| s.name == name)
    }
}

#[cfg(test)]
mod tests;
