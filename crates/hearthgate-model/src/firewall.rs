//! Firewall, blocklist, automatic ban and router settings.

use std::fmt;
use std::str::FromStr;
use std::time::Duration;

use ipnet::IpNet;
use serde::{Deserialize, Deserializer, Serialize, Serializer};

use crate::Protocol;

/// Inbound firewall managed by Hearthgate in its own nftables table.
// Independent on/off settings, as the user sees them.
#[allow(clippy::struct_excessive_bools)]
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(deny_unknown_fields)]
pub struct Firewall {
    /// Install the firewall. When off, Hearthgate leaves the host's packet
    /// filter alone and removes its own table.
    #[serde(default)]
    pub enabled: bool,
    /// Accept everything from the LAN (and later the VPN), so SSH and other
    /// local services keep working. When off, LAN clients only reach public
    /// ports, the dashboard and what the rules allow.
    #[serde(default = "yes")]
    pub allow_lan: bool,
    /// Answer ping (ICMP echo) from anywhere.
    #[serde(default = "yes")]
    pub allow_ping: bool,
    /// Ports published by Docker containers (`-p 8080:80`) are reachable only
    /// from the LAN, unless this is set. Publish them through a public
    /// interface instead.
    #[serde(default)]
    pub allow_docker_ports: bool,
    /// Extra allow and deny rules, checked in order before the defaults.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub rules: Vec<FirewallRule>,
    /// Addresses that are never blocked by lists, countries or bans.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    #[cfg_attr(feature = "openapi", schema(value_type = Vec<String>))]
    pub allow: Vec<IpNet>,
    /// Addresses that are always dropped.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    #[cfg_attr(feature = "openapi", schema(value_type = Vec<String>))]
    pub block: Vec<IpNet>,
    /// Subscribed blocklist feeds.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub blocklists: Vec<Blocklist>,
    /// Countries to drop, as two-letter ISO 3166 codes (for example `cn`).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub countries: Vec<String>,
    /// Temporary bans for clients that misbehave on public ports.
    #[serde(default)]
    pub auto_ban: AutoBan,
}

impl Default for Firewall {
    fn default() -> Self {
        Self {
            enabled: false,
            allow_lan: true,
            allow_ping: true,
            allow_docker_ports: false,
            rules: Vec::new(),
            allow: Vec::new(),
            block: Vec::new(),
            blocklists: Vec::new(),
            countries: Vec::new(),
            auto_ban: AutoBan::default(),
        }
    }
}

impl Firewall {
    /// Whether every setting has its default value.
    #[must_use]
    pub fn is_default(&self) -> bool {
        *self == Self::default()
    }
}

/// One allow or deny rule.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(deny_unknown_fields)]
pub struct FirewallRule {
    /// Unique name.
    pub name: String,
    /// What to do with matching traffic.
    pub action: RuleAction,
    /// Source networks. Empty means any source.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    #[cfg_attr(feature = "openapi", schema(value_type = Vec<String>))]
    pub sources: Vec<IpNet>,
    /// Destination ports or ranges, such as `22` or `"8000-8100"`. Empty
    /// means every port.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    #[cfg_attr(feature = "openapi", schema(value_type = Vec<String>))]
    pub ports: Vec<PortRange>,
    /// Transport protocol the ports apply to.
    #[serde(default)]
    pub protocol: Protocol,
    /// Free text shown in the dashboard.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub comment: String,
}

/// Action of a firewall rule.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(rename_all = "lowercase")]
pub enum RuleAction {
    Allow,
    Deny,
}

/// An inclusive port range. Written as a number for a single port, or as a
/// string `"first-last"`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct PortRange {
    pub first: u16,
    pub last: u16,
}

impl PortRange {
    #[must_use]
    pub fn single(port: u16) -> Self {
        Self {
            first: port,
            last: port,
        }
    }

    #[must_use]
    pub fn contains(&self, port: u16) -> bool {
        (self.first..=self.last).contains(&port)
    }
}

impl fmt::Display for PortRange {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.first == self.last {
            write!(f, "{}", self.first)
        } else {
            write!(f, "{}-{}", self.first, self.last)
        }
    }
}

impl FromStr for PortRange {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let parse = |p: &str| {
            p.trim()
                .parse::<u16>()
                .map_err(|_| format!("`{s}` is not a port or port range"))
        };
        let range = match s.split_once('-') {
            Some((a, b)) => Self {
                first: parse(a)?,
                last: parse(b)?,
            },
            None => Self::single(parse(s)?),
        };
        if range.first == 0 || range.first > range.last {
            return Err(format!("`{s}` is not a valid port range"));
        }
        Ok(range)
    }
}

impl Serialize for PortRange {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        if self.first == self.last {
            s.serialize_u16(self.first)
        } else {
            s.collect_str(self)
        }
    }
}

impl<'de> Deserialize<'de> for PortRange {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        #[derive(Deserialize)]
        #[serde(untagged)]
        enum Raw {
            Number(u64),
            Text(String),
        }
        match Raw::deserialize(d)? {
            Raw::Number(n) => u16::try_from(n)
                .ok()
                .filter(|n| *n > 0)
                .map(Self::single)
                .ok_or_else(|| serde::de::Error::custom(format!("{n} is not a port"))),
            Raw::Text(t) => t.parse().map_err(serde::de::Error::custom),
        }
    }
}

/// A subscribed blocklist feed: a list of addresses or networks, one per
/// line, downloaded on a schedule.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(deny_unknown_fields)]
pub struct Blocklist {
    /// Unique name.
    pub name: String,
    /// Where to download it (`https://` only).
    pub url: String,
    /// How often to download it again.
    #[serde(default = "refresh", with = "crate::duration::serde")]
    #[cfg_attr(feature = "openapi", schema(value_type = String, example = "12h"))]
    pub refresh: Duration,
    /// Turn the list off without forgetting it.
    #[serde(default = "yes")]
    pub enabled: bool,
}

/// Temporary bans for clients that misbehave on public ports: repeated
/// failed logins (401 and 403 answers), failed TLS handshakes, requests for
/// unknown domains and probes for well-known exploit paths.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(deny_unknown_fields)]
pub struct AutoBan {
    #[serde(default = "yes")]
    pub enabled: bool,
    /// Points within `window` that get a client banned. A failed login or
    /// handshake counts 1, an unknown domain 2, an exploit probe 5.
    #[serde(default = "ban_threshold")]
    pub threshold: u32,
    #[serde(default = "ban_window", with = "crate::duration::serde")]
    #[cfg_attr(feature = "openapi", schema(value_type = String, example = "10m"))]
    pub window: Duration,
    /// How long a ban lasts.
    #[serde(default = "ban_time", with = "crate::duration::serde")]
    #[cfg_attr(feature = "openapi", schema(value_type = String, example = "1h"))]
    pub ban_time: Duration,
}

impl Default for AutoBan {
    fn default() -> Self {
        Self {
            enabled: true,
            threshold: ban_threshold(),
            window: ban_window(),
            ban_time: ban_time(),
        }
    }
}

/// Router integration.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(deny_unknown_fields)]
pub struct Router {
    /// Ask the router (UPnP IGD or NAT-PMP) to forward each public port to
    /// this machine, and to remove the forwards again when they go away.
    #[serde(default)]
    pub upnp: bool,
}

impl Router {
    #[must_use]
    pub fn is_default(&self) -> bool {
        *self == Self::default()
    }
}

/// Networks that are never public internet sources: private, loopback,
/// link-local, carrier-grade NAT, multicast and other reserved ranges.
/// Blocklists and bans never drop these, so a feed that lists them (some
/// list bogons) cannot cut off the LAN.
pub const SPECIAL_NETS: &[&str] = &[
    "0.0.0.0/8",
    "10.0.0.0/8",
    "100.64.0.0/10",
    "127.0.0.0/8",
    "169.254.0.0/16",
    "172.16.0.0/12",
    "192.0.0.0/24",
    "192.168.0.0/16",
    "198.18.0.0/15",
    "224.0.0.0/3",
    "::/127",
    "::ffff:0:0/96",
    "64:ff9b::/96",
    "fc00::/7",
    "fe80::/10",
    "ff00::/8",
];

/// Whether `net` overlaps any of [`SPECIAL_NETS`].
#[must_use]
pub fn is_special_net(net: &IpNet) -> bool {
    SPECIAL_NETS.iter().any(|s| {
        let s: IpNet = s.parse().expect("valid special network");
        s.contains(net) || net.contains(&s)
    })
}

fn yes() -> bool {
    true
}
fn refresh() -> Duration {
    Duration::from_secs(12 * 3600)
}
fn ban_threshold() -> u32 {
    20
}
fn ban_window() -> Duration {
    Duration::from_secs(600)
}
fn ban_time() -> Duration {
    Duration::from_secs(3600)
}

/// Feeds offered in the dashboard. All are free for this use; check each
/// provider's terms for commercial use.
pub const SUGGESTED_BLOCKLISTS: &[(&str, &str, &str)] = &[
    (
        "spamhaus-drop",
        "https://www.spamhaus.org/drop/drop_v4.json",
        "Spamhaus DROP: networks hijacked or leased by professional spam and crime operations",
    ),
    (
        "spamhaus-drop-v6",
        "https://www.spamhaus.org/drop/drop_v6.json",
        "Spamhaus DROP for IPv6",
    ),
    (
        "firehol-level1",
        "https://raw.githubusercontent.com/firehol/blocklist-ipsets/master/firehol_level1.netset",
        "FireHOL level 1: a conservative mix of attack, malware and abuse lists",
    ),
    (
        "feodo-tracker",
        "https://feodotracker.abuse.ch/downloads/ipblocklist_recommended.txt",
        "abuse.ch Feodo Tracker: botnet command and control servers",
    ),
];

/// Per-country address lists, from ipdeny.com (aggregated zones).
#[must_use]
pub fn country_feeds(code: &str) -> [String; 2] {
    let code = code.to_ascii_lowercase();
    [
        format!("https://www.ipdeny.com/ipblocks/data/aggregated/{code}-aggregated.zone"),
        format!("https://www.ipdeny.com/ipv6/ipaddresses/aggregated/{code}-aggregated.zone"),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn port_ranges() {
        assert_eq!("22".parse::<PortRange>().unwrap(), PortRange::single(22));
        assert_eq!(
            "8000-8100".parse::<PortRange>().unwrap(),
            PortRange {
                first: 8000,
                last: 8100
            }
        );
        assert!("0".parse::<PortRange>().is_err());
        assert!("9-1".parse::<PortRange>().is_err());
        assert!("x".parse::<PortRange>().is_err());
        let json = serde_json::to_string(&vec![
            PortRange::single(22),
            "1-2".parse::<PortRange>().unwrap(),
        ])
        .unwrap();
        assert_eq!(json, r#"[22,"1-2"]"#);
        let back: Vec<PortRange> = serde_json::from_str(&json).unwrap();
        assert_eq!(back.len(), 2);
    }
}
