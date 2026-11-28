//! Release versions: `MAJOR.MINOR.PATCH`, compared as numbers.

use std::fmt;
use std::str::FromStr;

use serde::{Deserialize, Serialize};

/// A release version. Pre-releases and build metadata are not versions
/// Hearthgate installs, so they don't parse.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Version {
    pub major: u64,
    pub minor: u64,
    pub patch: u64,
}

impl Version {
    /// Parses `1.2.3` or `v1.2.3`.
    #[must_use]
    pub fn parse(text: &str) -> Option<Self> {
        let text = text.trim();
        let text = text.strip_prefix('v').unwrap_or(text);
        let mut parts = text.split('.');
        let num = |p: Option<&str>| -> Option<u64> {
            let p = p?;
            if p.is_empty()
                || !p.bytes().all(|b| b.is_ascii_digit())
                || (p.len() > 1 && p.starts_with('0'))
            {
                return None;
            }
            p.parse().ok()
        };
        let v = Self {
            major: num(parts.next())?,
            minor: num(parts.next())?,
            patch: num(parts.next())?,
        };
        parts.next().is_none().then_some(v)
    }

    /// What kind of step it is from `self` to the newer `to`, or `None`
    /// when `to` is not newer.
    #[must_use]
    pub fn bump_to(self, to: Self) -> Option<Bump> {
        if to <= self {
            None
        } else if to.major != self.major {
            Some(Bump::Major)
        } else if to.minor != self.minor {
            Some(Bump::Minor)
        } else {
            Some(Bump::Patch)
        }
    }
}

impl fmt::Display for Version {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}.{}.{}", self.major, self.minor, self.patch)
    }
}

impl FromStr for Version {
    type Err = String;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Self::parse(s).ok_or_else(|| format!("{s:?} is not a version such as 1.2.3"))
    }
}

impl Serialize for Version {
    fn serialize<S: serde::Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        s.collect_str(self)
    }
}

impl<'de> Deserialize<'de> for Version {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let s = String::deserialize(d)?;
        s.parse().map_err(serde::de::Error::custom)
    }
}

/// How big a step a release is.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(rename_all = "lowercase")]
pub enum Bump {
    Patch,
    Minor,
    Major,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn v(s: &str) -> Version {
        Version::parse(s).unwrap()
    }

    #[test]
    fn parses_releases_only() {
        assert_eq!(
            v("v1.2.3"),
            Version {
                major: 1,
                minor: 2,
                patch: 3
            }
        );
        assert_eq!(v("10.0.12").to_string(), "10.0.12");
        for bad in [
            "1.2",
            "1.2.3.4",
            "1.2.3-rc.1",
            "0.0.0-dev",
            "01.2.3",
            "1..3",
            "",
            "v",
            "1.2.x",
        ] {
            assert!(Version::parse(bad).is_none(), "{bad}");
        }
    }

    #[test]
    fn compares_as_numbers() {
        assert!(v("1.10.0") > v("1.9.9"));
        assert_eq!(v("1.2.1").bump_to(v("1.2.2")), Some(Bump::Patch));
        assert_eq!(v("1.2.1").bump_to(v("1.3.0")), Some(Bump::Minor));
        assert_eq!(v("1.2.1").bump_to(v("2.0.0")), Some(Bump::Major));
        assert_eq!(v("1.2.1").bump_to(v("1.2.1")), None);
        assert_eq!(v("1.2.1").bump_to(v("1.2.0")), None);
    }
}
