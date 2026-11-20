use std::collections::BTreeMap;
use std::fmt;
use std::net::IpAddr;
use std::time::Duration;

use serde::{Deserialize, Serialize};

/// The complete desired state: everything a user configures.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(deny_unknown_fields)]
pub struct DesiredState {
    /// Certificate authority settings for automatic certificates.
    #[serde(default)]
    pub acme: AcmeSettings,
    /// Limits and timeouts that apply to every proxied connection.
    #[serde(default)]
    pub limits: Limits,
    /// Public interfaces: what is reachable from the internet.
    #[serde(default)]
    pub interfaces: Vec<Interface>,
    /// Internal services Hearthgate can forward to.
    #[serde(default)]
    pub services: Vec<Service>,
    /// Routes connecting interface listeners to services.
    #[serde(default)]
    pub routes: Vec<Route>,
}

/// Settings for automatic certificates (ACME).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(deny_unknown_fields)]
pub struct AcmeSettings {
    /// Contact email registered with the CA. Optional, but recommended so the
    /// CA can warn about expiring certificates.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub email: Option<String>,
    /// ACME directory URL. Defaults to Let's Encrypt production, or staging
    /// when `staging` is set.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub directory: Option<String>,
    /// Use the Let's Encrypt staging CA (untrusted certificates, generous
    /// rate limits). Ignored when `directory` is set.
    #[serde(default)]
    pub staging: bool,
    /// Agree to the CA's terms of service. Required before any certificate
    /// is requested.
    #[serde(default)]
    pub accept_terms: bool,
    /// Renew a certificate when it has less than this long left.
    #[serde(default = "defaults::renew_before", with = "humantime_serde")]
    #[cfg_attr(feature = "openapi", schema(value_type = String, example = "10s"))]
    pub renew_before: Duration,
    /// Challenge type. By default `http-01` when the interface has an
    /// `http` listener (normally port 80), otherwise `tls-alpn-01` on its
    /// `https` listener (normally port 443).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub challenge: Option<AcmeChallenge>,
    /// PEM file with the root certificate of a private ACME server's API,
    /// for example a home CA or Pebble in tests.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub directory_root: Option<String>,
}

/// ACME challenge type.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub enum AcmeChallenge {
    #[serde(rename = "http-01")]
    Http01,
    #[serde(rename = "tls-alpn-01")]
    TlsAlpn01,
}

impl Default for AcmeSettings {
    fn default() -> Self {
        Self {
            email: None,
            directory: None,
            staging: false,
            accept_terms: false,
            renew_before: defaults::renew_before(),
            challenge: None,
            directory_root: None,
        }
    }
}

/// Let's Encrypt production directory.
pub const LETS_ENCRYPT_PRODUCTION: &str = "https://acme-v02.api.letsencrypt.org/directory";
/// Let's Encrypt staging directory.
pub const LETS_ENCRYPT_STAGING: &str = "https://acme-staging-v02.api.letsencrypt.org/directory";

impl AcmeSettings {
    /// The directory URL to use, after applying defaults.
    #[must_use]
    pub fn directory_url(&self) -> &str {
        match (&self.directory, self.staging) {
            (Some(url), _) => url,
            (None, true) => LETS_ENCRYPT_STAGING,
            (None, false) => LETS_ENCRYPT_PRODUCTION,
        }
    }
}

/// Limits and timeouts for proxied traffic.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(deny_unknown_fields)]
pub struct Limits {
    /// Largest HTTP request body accepted, in bytes.
    #[serde(default = "defaults::max_body_bytes")]
    pub max_body_bytes: u64,
    /// Time allowed to receive request headers.
    #[serde(default = "defaults::header_timeout", with = "humantime_serde")]
    #[cfg_attr(feature = "openapi", schema(value_type = String, example = "10s"))]
    pub header_timeout: Duration,
    /// Time allowed for an upstream to answer an HTTP request.
    #[serde(default = "defaults::upstream_timeout", with = "humantime_serde")]
    #[cfg_attr(feature = "openapi", schema(value_type = String, example = "10s"))]
    pub upstream_timeout: Duration,
    /// Time allowed to connect to an upstream.
    #[serde(default = "defaults::connect_timeout", with = "humantime_serde")]
    #[cfg_attr(feature = "openapi", schema(value_type = String, example = "10s"))]
    pub connect_timeout: Duration,
    /// Close idle TCP connections after this long.
    #[serde(default = "defaults::tcp_idle_timeout", with = "humantime_serde")]
    #[cfg_attr(feature = "openapi", schema(value_type = String, example = "10s"))]
    pub tcp_idle_timeout: Duration,
    /// Forget UDP client sessions after this long without traffic.
    #[serde(default = "defaults::udp_session_timeout", with = "humantime_serde")]
    #[cfg_attr(feature = "openapi", schema(value_type = String, example = "10s"))]
    pub udp_session_timeout: Duration,
    /// How long removed listeners keep serving open connections before they
    /// are closed.
    #[serde(default = "defaults::drain_timeout", with = "humantime_serde")]
    #[cfg_attr(feature = "openapi", schema(value_type = String, example = "10s"))]
    pub drain_timeout: Duration,
}

impl Default for Limits {
    fn default() -> Self {
        Self {
            max_body_bytes: defaults::max_body_bytes(),
            header_timeout: defaults::header_timeout(),
            upstream_timeout: defaults::upstream_timeout(),
            connect_timeout: defaults::connect_timeout(),
            tcp_idle_timeout: defaults::tcp_idle_timeout(),
            udp_session_timeout: defaults::udp_session_timeout(),
            drain_timeout: defaults::drain_timeout(),
        }
    }
}

/// A public interface: one or more domains reachable on a set of ports.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(deny_unknown_fields)]
pub struct Interface {
    /// Unique name, lowercase letters, digits and dashes.
    pub name: String,
    /// Domains served by this interface. Required for `https` listeners.
    #[serde(default)]
    pub domains: Vec<String>,
    /// Local addresses to bind. Empty means all IPv4 and IPv6 addresses.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    #[cfg_attr(feature = "openapi", schema(value_type = Vec<String>))]
    pub bind: Vec<IpAddr>,
    /// Name of a manually uploaded certificate. When unset, `https`
    /// listeners get an automatic certificate for `domains`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub certificate: Option<String>,
    /// Ports this interface listens on.
    pub listeners: Vec<Listener>,
}

impl Interface {
    /// Finds a listener by port.
    #[must_use]
    pub fn listener(&self, port: u16) -> Option<&Listener> {
        self.listeners.iter().find(|l| l.port == port)
    }

    /// Whether this interface terminates TLS on any listener.
    #[must_use]
    pub fn has_https(&self) -> bool {
        self.listeners.iter().any(|l| l.mode == ListenerMode::Https)
    }

    /// Whether this interface needs an automatic certificate.
    #[must_use]
    pub fn wants_acme(&self) -> bool {
        self.has_https() && self.certificate.is_none() && !self.domains.is_empty()
    }
}

/// A port on a public interface.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(deny_unknown_fields)]
pub struct Listener {
    /// Port number.
    pub port: u16,
    /// What Hearthgate does with connections on this port.
    pub mode: ListenerMode,
    /// Transport protocol. Only `raw` listeners may use `udp` or `both`.
    #[serde(default)]
    pub protocol: Protocol,
}

impl Listener {
    /// Whether this listener accepts TCP connections.
    #[must_use]
    pub fn tcp(&self) -> bool {
        matches!(self.protocol, Protocol::Tcp | Protocol::Both)
    }

    /// Whether this listener accepts UDP datagrams.
    #[must_use]
    pub fn udp(&self) -> bool {
        matches!(self.protocol, Protocol::Udp | Protocol::Both)
    }
}

/// How a listener handles traffic.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(rename_all = "kebab-case")]
pub enum ListenerMode {
    /// TLS terminated by Hearthgate, then HTTP routing by host and path.
    Https,
    /// Plain HTTP routing. Without routes of its own it redirects to the
    /// interface's `https` listener and answers ACME challenges.
    Http,
    /// TLS routed by SNI without being decrypted.
    TlsPassthrough,
    /// Raw layer 4 forwarding of TCP connections and/or UDP datagrams.
    Raw,
}

impl fmt::Display for ListenerMode {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::Https => "https",
            Self::Http => "http",
            Self::TlsPassthrough => "tls-passthrough",
            Self::Raw => "raw",
        })
    }
}

/// Transport protocol of a listener.
#[derive(
    Debug, Clone, Copy, Default, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize,
)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(rename_all = "lowercase")]
pub enum Protocol {
    #[default]
    Tcp,
    Udp,
    Both,
}

impl fmt::Display for Protocol {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::Tcp => "tcp",
            Self::Udp => "udp",
            Self::Both => "both",
        })
    }
}

/// Something Hearthgate can forward traffic to.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(deny_unknown_fields)]
pub struct Service {
    /// Unique name, lowercase letters, digits and dashes. Later also the
    /// service's internal DNS name.
    pub name: String,
    /// Where the service runs.
    pub kind: ServiceKind,
    /// Host name or IP address, for `external` services.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub host: Option<String>,
    /// Container name, for `docker` services.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub container: Option<String>,
    /// Docker network to take the container's address from. Defaults to the
    /// first network the container is attached to.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub network: Option<String>,
    /// Named ports, for example `http = 8080`.
    pub ports: BTreeMap<String, u16>,
    /// Optional active health check.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub health: Option<HealthCheck>,
}

/// Where a service runs.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(rename_all = "lowercase")]
pub enum ServiceKind {
    /// A service managed outside Hearthgate, reached by host and port.
    External,
    /// An existing Docker container, found by name.
    Docker,
}

/// Active health check for a service.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(deny_unknown_fields)]
pub struct HealthCheck {
    /// `tcp` connects, `http` sends a GET and expects a 2xx or 3xx status.
    #[serde(rename = "type")]
    pub kind: HealthCheckKind,
    /// Port to check: a named port or a number. Defaults to every port the
    /// routes use.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub port: Option<PortRef>,
    /// Request path for `http` checks.
    #[serde(default = "defaults::health_path")]
    pub path: String,
    /// Time between checks.
    #[serde(default = "defaults::health_interval", with = "humantime_serde")]
    #[cfg_attr(feature = "openapi", schema(value_type = String, example = "10s"))]
    pub interval: Duration,
    /// Time allowed for one check.
    #[serde(default = "defaults::health_timeout", with = "humantime_serde")]
    #[cfg_attr(feature = "openapi", schema(value_type = String, example = "10s"))]
    pub timeout: Duration,
    /// Consecutive successes before a target is healthy again.
    #[serde(default = "defaults::healthy_threshold")]
    pub healthy_threshold: u32,
    /// Consecutive failures before a target is taken out of rotation.
    #[serde(default = "defaults::unhealthy_threshold")]
    pub unhealthy_threshold: u32,
}

/// Kind of health check.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(rename_all = "lowercase")]
pub enum HealthCheckKind {
    Tcp,
    Http,
}

/// A reference to a service port: by name or by number.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(untagged)]
pub enum PortRef {
    Number(u16),
    Name(String),
}

impl fmt::Display for PortRef {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Number(n) => write!(f, "{n}"),
            Self::Name(n) => f.write_str(n),
        }
    }
}

impl Service {
    /// Resolves a port reference against this service's named ports.
    #[must_use]
    pub fn resolve_port(&self, port: &PortRef) -> Option<u16> {
        match port {
            PortRef::Number(n) => Some(*n),
            PortRef::Name(name) => self.ports.get(name).copied(),
        }
    }
}

/// Connects a listener to one or more service ports.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(deny_unknown_fields)]
pub struct Route {
    /// Unique name.
    pub name: String,
    /// Interface the route belongs to.
    pub interface: String,
    /// Listener port on that interface.
    pub listener: u16,
    /// Host names to match (`https`, `http`, `tls-passthrough`). Empty means
    /// every domain of the interface.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub hosts: Vec<String>,
    /// Path prefix to match (`https`, `http`). Defaults to `/`.
    #[serde(default = "defaults::path_prefix")]
    pub path_prefix: String,
    /// Remove the matched prefix before forwarding.
    #[serde(default)]
    pub strip_prefix: bool,
    /// How requests are spread across targets.
    #[serde(default)]
    pub strategy: Strategy,
    /// Upstreams. One forwards, two or more are load-balanced.
    pub targets: Vec<Target>,
}

/// A route upstream.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(deny_unknown_fields)]
pub struct Target {
    /// Service name.
    pub service: String,
    /// Service port, by name or number.
    pub port: PortRef,
    /// Relative weight for round robin.
    #[serde(default = "defaults::weight")]
    pub weight: u32,
}

/// Load balancing strategy.
#[derive(
    Debug, Clone, Copy, Default, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize,
)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(rename_all = "kebab-case")]
pub enum Strategy {
    /// Weighted round robin.
    #[default]
    RoundRobin,
    /// Pick the target with the fewest open connections.
    LeastConnections,
    /// Hash the client address, so a client keeps the same target.
    SourceHash,
}

impl fmt::Display for Strategy {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::RoundRobin => "round-robin",
            Self::LeastConnections => "least-connections",
            Self::SourceHash => "source-hash",
        })
    }
}

pub(crate) mod defaults {
    use std::time::Duration;

    pub fn renew_before() -> Duration {
        Duration::from_secs(30 * 24 * 3600)
    }
    pub fn max_body_bytes() -> u64 {
        100 * 1024 * 1024
    }
    pub fn header_timeout() -> Duration {
        Duration::from_secs(15)
    }
    pub fn upstream_timeout() -> Duration {
        Duration::from_secs(60)
    }
    pub fn connect_timeout() -> Duration {
        Duration::from_secs(5)
    }
    pub fn tcp_idle_timeout() -> Duration {
        Duration::from_secs(3600)
    }
    pub fn udp_session_timeout() -> Duration {
        Duration::from_secs(60)
    }
    pub fn drain_timeout() -> Duration {
        Duration::from_secs(30)
    }
    pub fn health_path() -> String {
        "/".to_owned()
    }
    pub fn health_interval() -> Duration {
        Duration::from_secs(10)
    }
    pub fn health_timeout() -> Duration {
        Duration::from_secs(2)
    }
    pub fn healthy_threshold() -> u32 {
        2
    }
    pub fn unhealthy_threshold() -> u32 {
        3
    }
    pub fn path_prefix() -> String {
        "/".to_owned()
    }
    pub fn weight() -> u32 {
        1
    }
}
