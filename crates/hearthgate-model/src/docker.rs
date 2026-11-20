//! Docker containers and Compose stacks that Hearthgate runs.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

/// A container Hearthgate creates and keeps running. It joins the
/// `hearthgate` Docker network, so routes reach it without publishing host
/// ports: point a `docker` service at it by name.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(deny_unknown_fields)]
pub struct Container {
    /// The container's name in Docker, such as `whoami`.
    pub name: String,
    /// Image reference, such as `traefik/whoami:v1.10`.
    pub image: String,
    /// Whether it should run. A stopped container keeps its settings.
    #[serde(default = "yes", skip_serializing_if = "is_true")]
    pub running: bool,
    /// Overrides the image's command.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub command: Vec<String>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub env: BTreeMap<String, String>,
    /// `/host/path:/container/path[:ro]` or `volume-name:/container/path[:ro]`.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub volumes: Vec<String>,
    /// Host ports to publish, `[address:]host:container[/udp]`. Usually not
    /// needed: routes reach the container over the `hearthgate` network.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub publish: Vec<String>,
    #[serde(default, skip_serializing_if = "Restart::is_default")]
    pub restart: Restart,
}

/// When Docker restarts a container that stopped.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(rename_all = "kebab-case")]
pub enum Restart {
    No,
    Always,
    #[default]
    UnlessStopped,
    OnFailure,
}

impl Restart {
    #[must_use]
    pub fn is_default(&self) -> bool {
        *self == Self::default()
    }

    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::No => "no",
            Self::Always => "always",
            Self::UnlessStopped => "unless-stopped",
            Self::OnFailure => "on-failure",
        }
    }
}

/// A Compose stack, run with `docker compose`. Its containers join the
/// `hearthgate` network as well.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(deny_unknown_fields)]
pub struct Stack {
    /// The Compose project name. Its containers are called
    /// `NAME-SERVICE-1`.
    pub name: String,
    /// The Compose file (YAML).
    pub compose: String,
    /// Variables for the Compose file, written to its `.env` file.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub env: BTreeMap<String, String>,
    #[serde(default = "yes", skip_serializing_if = "is_true")]
    pub running: bool,
}

fn yes() -> bool {
    true
}

#[allow(clippy::trivially_copy_pass_by_ref)]
fn is_true(v: &bool) -> bool {
    *v
}

/// A parsed `publish` entry.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Publish {
    pub address: Option<std::net::IpAddr>,
    pub host: u16,
    pub container: u16,
    pub udp: bool,
}

impl Publish {
    /// Parses `[address:]host:container[/tcp|/udp]`.
    #[must_use]
    pub fn parse(text: &str) -> Option<Self> {
        let (ports, udp) = match text.rsplit_once('/') {
            Some((p, "udp")) => (p, true),
            Some((p, "tcp")) => (p, false),
            Some(_) => return None,
            None => (text, false),
        };
        let (rest, container) = ports.rsplit_once(':')?;
        let (address, host) = match rest.rsplit_once(':') {
            Some((a, h)) => (
                Some(
                    a.trim_start_matches('[')
                        .trim_end_matches(']')
                        .parse()
                        .ok()?,
                ),
                h,
            ),
            None => (None, rest),
        };
        let host: u16 = host.parse().ok().filter(|p| *p != 0)?;
        let container: u16 = container.parse().ok().filter(|p| *p != 0)?;
        Some(Self {
            address,
            host,
            container,
            udp,
        })
    }
}

/// A parsed `volumes` entry.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Volume {
    /// An absolute host path, or a named volume.
    pub source: String,
    pub target: String,
    pub read_only: bool,
}

impl Volume {
    #[must_use]
    pub fn parse(text: &str) -> Option<Self> {
        let parts: Vec<&str> = text.split(':').collect();
        let (source, target, read_only) = match parts.as_slice() {
            [s, t] | [s, t, "rw"] => (*s, *t, false),
            [s, t, "ro"] => (*s, *t, true),
            _ => return None,
        };
        let named = source
            .chars()
            .next()
            .is_some_and(|c| c.is_ascii_alphanumeric())
            && source
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || "_.-".contains(c));
        if !(source.starts_with('/') || named) || !target.starts_with('/') {
            return None;
        }
        Some(Self {
            source: source.to_owned(),
            target: target.to_owned(),
            read_only,
        })
    }

    #[must_use]
    pub fn is_bind(&self) -> bool {
        self.source.starts_with('/')
    }
}

/// Whether `key` can be an environment variable name.
#[must_use]
pub fn is_env_name(key: &str) -> bool {
    let mut chars = key.chars();
    chars
        .next()
        .is_some_and(|c| c.is_ascii_alphabetic() || c == '_')
        && chars.all(|c| c.is_ascii_alphanumeric() || c == '_')
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_publish_and_volumes() {
        let p = Publish::parse("8080:80").unwrap();
        assert_eq!(
            (p.host, p.container, p.udp, p.address),
            (8080, 80, false, None)
        );
        let p = Publish::parse("127.0.0.1:53:53/udp").unwrap();
        assert!(p.udp);
        assert_eq!(p.address, Some("127.0.0.1".parse().unwrap()));
        assert!(Publish::parse("80").is_none());
        assert!(Publish::parse("0:80").is_none());
        assert!(Publish::parse("80:80/sctp").is_none());

        let v = Volume::parse("/srv/data:/data:ro").unwrap();
        assert!(v.read_only && v.is_bind());
        assert!(!Volume::parse("db-data:/var/lib/db").unwrap().is_bind());
        assert!(Volume::parse("relative:/x").is_some());
        assert!(Volume::parse("./rel:/x").is_none());
        assert!(Volume::parse("/a:rel").is_none());
        assert!(
            is_env_name("TZ") && is_env_name("_A1") && !is_env_name("1A") && !is_env_name("A-B")
        );
    }
}
