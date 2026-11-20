//! The built-in WireGuard VPN and the internal DNS zone it serves.

use std::fmt;
use std::net::Ipv4Addr;

use ipnet::Ipv4Net;
use serde::de::{self, Deserializer};
use serde::ser::Serializer;
use serde::{Deserialize, Serialize};

/// The VPN: one WireGuard interface on this machine, and the devices
/// (peers) allowed to connect to it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(deny_unknown_fields)]
pub struct Vpn {
    #[serde(default)]
    pub enabled: bool,
    /// UDP port peers connect to. Opened in the firewall and, with router
    /// port forwarding on, on the router.
    #[serde(default = "default_port")]
    pub port: u16,
    /// Addresses inside the VPN. This machine takes the first one.
    #[serde(default = "default_subnet")]
    #[cfg_attr(feature = "openapi", schema(value_type = String, example = "10.88.0.0/24"))]
    pub subnet: Ipv4Net,
    /// Host name or address peers connect to, with an optional port, such
    /// as `vpn.example.com`. Defaults to the first domain of the first
    /// public interface, or the public address the router reports.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub endpoint: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub peers: Vec<Peer>,
}

impl Default for Vpn {
    fn default() -> Self {
        Self {
            enabled: false,
            port: default_port(),
            subnet: default_subnet(),
            endpoint: None,
            peers: Vec::new(),
        }
    }
}

impl Vpn {
    #[must_use]
    pub fn is_default(&self) -> bool {
        *self == Self::default()
    }

    /// This machine's address inside the VPN.
    #[must_use]
    pub fn server_address(&self) -> Ipv4Addr {
        first_host(self.subnet)
    }

    /// The next address no peer uses yet.
    #[must_use]
    pub fn free_address(&self) -> Option<Ipv4Addr> {
        let server = self.server_address();
        self.subnet
            .hosts()
            .find(|a| *a != server && !self.peers.iter().any(|p| p.address == *a))
    }
}

/// The first usable address of `net`.
fn first_host(net: Ipv4Net) -> Ipv4Addr {
    net.hosts().next().unwrap_or_else(|| net.network())
}

/// A device that may connect to the VPN.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(deny_unknown_fields)]
pub struct Peer {
    /// Unique name, such as `phone`.
    pub name: String,
    /// The device's WireGuard public key (base64). Its private key never
    /// leaves the device, or is shown once when Hearthgate creates it.
    pub public_key: String,
    /// The device's address inside the VPN.
    #[cfg_attr(feature = "openapi", schema(value_type = String, example = "10.88.0.2"))]
    pub address: Ipv4Addr,
    /// What the device may reach.
    #[serde(default)]
    pub access: Access,
}

/// What a peer may reach: everything (this machine, its LAN and every
/// service), or only the named services.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub enum Access {
    #[default]
    All,
    Services(Vec<String>),
}

impl Serialize for Access {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        match self {
            Self::All => s.serialize_str("all"),
            Self::Services(list) => list.serialize(s),
        }
    }
}

impl<'de> Deserialize<'de> for Access {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        #[derive(Deserialize)]
        #[serde(untagged)]
        enum Raw {
            Text(String),
            List(Vec<String>),
        }
        match Raw::deserialize(d)? {
            Raw::Text(t) if t == "all" => Ok(Self::All),
            Raw::Text(t) => Err(de::Error::custom(format!(
                "`{t}`: use \"all\" or a list of service names"
            ))),
            Raw::List(list) => Ok(Self::Services(list)),
        }
    }
}

#[cfg(feature = "openapi")]
impl utoipa::PartialSchema for Access {
    fn schema() -> utoipa::openapi::RefOr<utoipa::openapi::schema::Schema> {
        use utoipa::openapi::schema::{ArrayBuilder, ObjectBuilder, OneOfBuilder, Type};
        OneOfBuilder::new()
            .item(
                ObjectBuilder::new()
                    .schema_type(Type::String)
                    .enum_values(Some(["all"])),
            )
            .item(ArrayBuilder::new().items(ObjectBuilder::new().schema_type(Type::String)))
            .description(Some(
                "\"all\", or the names of the services the peer may reach",
            ))
            .into()
    }
}

#[cfg(feature = "openapi")]
impl utoipa::ToSchema for Access {}

impl fmt::Display for Access {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::All => f.write_str("all"),
            Self::Services(list) => f.write_str(&list.join(", ")),
        }
    }
}

/// Internal DNS: every service resolves as `name.<zone>` for VPN peers
/// (and, if allowed, the LAN). Other names go to the upstream resolvers.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(deny_unknown_fields)]
pub struct Dns {
    /// Answer on the VPN address while the VPN is on.
    #[serde(default = "yes")]
    pub enabled: bool,
    #[serde(default = "default_zone")]
    pub zone: String,
    /// Also answer on this machine's LAN addresses, so LAN devices that use
    /// it as their DNS server get the same names.
    #[serde(default)]
    pub lan: bool,
    /// Resolvers for every other name, such as `1.1.1.1` or `192.168.1.1:53`.
    /// Defaults to the ones in `/etc/resolv.conf`.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub upstream: Vec<String>,
}

impl Default for Dns {
    fn default() -> Self {
        Self {
            enabled: true,
            zone: default_zone(),
            lan: false,
            upstream: Vec::new(),
        }
    }
}

impl Dns {
    #[must_use]
    pub fn is_default(&self) -> bool {
        *self == Self::default()
    }
}

fn yes() -> bool {
    true
}
fn default_port() -> u16 {
    51820
}
fn default_subnet() -> Ipv4Net {
    "10.88.0.0/24".parse().expect("valid subnet")
}
fn default_zone() -> String {
    "home.internal".into()
}

/// Splits `host[:port]`, `[v6]:port` or a bare IPv6 address.
#[must_use]
pub fn split_endpoint(text: &str) -> Option<(&str, Option<u16>)> {
    if let Some(rest) = text.strip_prefix('[') {
        let (host, after) = rest.split_once(']')?;
        return match after {
            "" => Some((host, None)),
            p => Some((host, Some(p.strip_prefix(':')?.parse().ok()?))),
        };
    }
    // No colon, or several (a bare IPv6 address): no port.
    if text.matches(':').count() == 1 {
        let (host, port) = text.split_once(':')?;
        Some((host, Some(port.parse().ok()?)))
    } else {
        Some((text, None))
    }
    .filter(|(host, port)| !host.is_empty() && *port != Some(0))
}

/// Whether `key` is a WireGuard key: 32 bytes in standard base64.
#[must_use]
pub fn is_wireguard_key(key: &str) -> bool {
    // 32 bytes are 43 base64 characters and one `=`.
    key.len() == 44
        && key.ends_with('=')
        && key[..43]
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'+' || b == b'/')
        // The last character before `=` carries 4 bits of padding, so only
        // every fourth symbol is valid there.
        && b"AEIMQUYcgkosw048".contains(&key.as_bytes()[42])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn access_reads_all_or_a_list() {
        #[derive(Deserialize, Serialize)]
        struct T {
            access: Access,
        }
        let all: T = toml::from_str(r#"access = "all""#).unwrap();
        assert_eq!(all.access, Access::All);
        let some: T = toml::from_str(r#"access = ["ssh", "nas"]"#).unwrap();
        assert_eq!(
            some.access,
            Access::Services(vec!["ssh".into(), "nas".into()])
        );
        assert!(toml::from_str::<T>(r#"access = "everything""#).is_err());
        assert_eq!(
            toml::to_string(&some).unwrap().trim(),
            r#"access = ["ssh", "nas"]"#
        );
    }

    #[test]
    fn addresses() {
        let mut vpn = Vpn::default();
        assert_eq!(vpn.server_address(), Ipv4Addr::new(10, 88, 0, 1));
        assert_eq!(vpn.free_address(), Some(Ipv4Addr::new(10, 88, 0, 2)));
        vpn.peers.push(Peer {
            name: "phone".into(),
            public_key: "x".into(),
            address: Ipv4Addr::new(10, 88, 0, 2),
            access: Access::All,
        });
        assert_eq!(vpn.free_address(), Some(Ipv4Addr::new(10, 88, 0, 3)));
    }

    #[test]
    fn endpoints() {
        assert_eq!(
            split_endpoint("vpn.example.com"),
            Some(("vpn.example.com", None))
        );
        assert_eq!(
            split_endpoint("vpn.example.com:4500"),
            Some(("vpn.example.com", Some(4500)))
        );
        assert_eq!(
            split_endpoint("[2001:db8::1]:51820"),
            Some(("2001:db8::1", Some(51820)))
        );
        assert_eq!(split_endpoint("2001:db8::1"), Some(("2001:db8::1", None)));
        assert_eq!(split_endpoint("host:port"), None);
        assert_eq!(split_endpoint(":80"), None);
    }

    #[test]
    fn keys() {
        assert!(is_wireguard_key(
            "YAnz5TF+lXXJte14tji3zlMNq+hd2rYUIgJBgB3fBmk="
        ));
        assert!(!is_wireguard_key(
            "YAnz5TF+lXXJte14tji3zlMNq+hd2rYUIgJBgB3fBml="
        ));
        assert!(!is_wireguard_key("short="));
        assert!(!is_wireguard_key(
            "YAnz5TF+lXXJte14tji3zlMNq+hd2rYUIgJBgB3fBm!="
        ));
    }
}
