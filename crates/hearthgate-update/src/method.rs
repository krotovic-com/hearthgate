//! How Hearthgate was installed, which decides how it is upgraded.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::Version;

/// How this machine got Hearthgate.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(rename_all = "lowercase")]
pub enum Method {
    /// The .deb, through apt.
    Deb,
    /// The .rpm, through dnf, zypper or yum.
    Rpm,
    /// The release archive, through `install.sh`.
    Tarball,
    /// Anything else: a custom path, a development build. Updates are
    /// shown but not installed from the dashboard.
    Manual,
}

impl Method {
    /// Where the binary lives for this method.
    #[must_use]
    pub fn binary(self) -> Option<&'static str> {
        match self {
            Self::Deb | Self::Rpm => Some("/usr/bin/hearthgate"),
            Self::Tarball => Some("/usr/local/bin/hearthgate"),
            Self::Manual => None,
        }
    }

    /// Words for people.
    #[must_use]
    pub fn describe(self) -> &'static str {
        match self {
            Self::Deb => "Debian package · apt",
            Self::Rpm => "RPM package",
            Self::Tarball => "Release archive · install.sh",
            Self::Manual => "Manual install",
        }
    }
}

/// CPU architecture names the release assets use.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Arch {
    pub target: &'static str,
    pub deb: &'static str,
    pub rpm: &'static str,
}

impl Arch {
    /// This machine's architecture, if there are releases for it.
    #[must_use]
    pub fn current() -> Option<Self> {
        Self::from_name(std::env::consts::ARCH)
    }

    #[must_use]
    pub fn from_name(name: &str) -> Option<Self> {
        match name {
            "x86_64" => Some(Self {
                target: "x86_64-unknown-linux-musl",
                deb: "amd64",
                rpm: "x86_64",
            }),
            "aarch64" => Some(Self {
                target: "aarch64-unknown-linux-musl",
                deb: "arm64",
                rpm: "aarch64",
            }),
            "arm" => Some(Self {
                target: "armv7-unknown-linux-musleabihf",
                deb: "armhf",
                rpm: "armv7hl",
            }),
            _ => None,
        }
    }
}

/// Whether `name` is the asset of `version` for `method` on `arch`.
#[must_use]
pub fn is_asset(name: &str, method: Method, version: Version, arch: Arch) -> bool {
    match method {
        Method::Deb => name == format!("hearthgate_{version}_{}.deb", arch.deb),
        Method::Rpm => {
            name.starts_with(&format!("hearthgate-{version}-"))
                && name.ends_with(&format!(".{}.rpm", arch.rpm))
                && name.len() < 80
        }
        Method::Tarball => name == format!("hearthgate-v{version}-{}.tar.gz", arch.target),
        Method::Manual => false,
    }
}

/// Finds the asset among `names`.
pub fn pick_asset<'a>(
    names: impl IntoIterator<Item = &'a str>,
    method: Method,
    version: Version,
    arch: Arch,
) -> Option<&'a str> {
    names
        .into_iter()
        .find(|n| is_asset(n, method, version, arch))
}

/// Looks at the file system (under `root`, `/` outside tests) to tell how
/// the binary at `exe` was installed.
#[must_use]
pub fn detect(root: &Path, exe: &Path) -> Method {
    let at = |p: &str| root.join(p.trim_start_matches('/'));
    match exe.to_str() {
        Some("/usr/bin/hearthgate") if at("/var/lib/dpkg/info/hearthgate.list").is_file() => {
            Method::Deb
        }
        Some("/usr/bin/hearthgate")
            if at("/usr/bin/rpm").exists() && (at("/var/lib/rpm").is_dir() || at("/usr/lib/sysimage/rpm").is_dir()) =>
        {
            Method::Rpm
        }
        Some("/usr/local/bin/hearthgate") if at("/etc/systemd/system/hearthgate.service").is_file() => {
            Method::Tarball
        }
        _ => Method::Manual,
    }
}

/// The running binary's path, without the " (deleted)" Linux adds after
/// the file was replaced.
#[must_use]
pub fn current_exe() -> PathBuf {
    let exe = std::env::current_exe().unwrap_or_default();
    match exe.to_str().and_then(|s| s.strip_suffix(" (deleted)")) {
        Some(s) => PathBuf::from(s),
        None => exe,
    }
}

/// Whether the root helper's units are installed, without which nothing
/// can be installed from the dashboard.
#[must_use]
pub fn helper_installed(root: &Path) -> bool {
    ["usr/lib/systemd/system", "lib/systemd/system", "etc/systemd/system"]
        .iter()
        .any(|d| root.join(d).join("hearthgate-upgrade.path").is_file())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn v(s: &str) -> Version {
        Version::parse(s).unwrap()
    }

    #[test]
    fn picks_the_right_asset() {
        let names = [
            "SHA256SUMS",
            "hearthgate-v1.3.0-x86_64-unknown-linux-musl.tar.gz",
            "hearthgate-v1.3.0-aarch64-unknown-linux-musl.tar.gz",
            "hearthgate_1.3.0_amd64.deb",
            "hearthgate_1.3.0_arm64.deb",
            "hearthgate-1.3.0-1.x86_64.rpm",
            "hearthgate-1.3.0-1.aarch64.rpm",
            "install.sh",
        ];
        let x86 = Arch::from_name("x86_64").unwrap();
        let arm = Arch::from_name("aarch64").unwrap();
        assert_eq!(pick_asset(names, Method::Deb, v("1.3.0"), x86), Some("hearthgate_1.3.0_amd64.deb"));
        assert_eq!(pick_asset(names, Method::Rpm, v("1.3.0"), arm), Some("hearthgate-1.3.0-1.aarch64.rpm"));
        assert_eq!(
            pick_asset(names, Method::Tarball, v("1.3.0"), x86),
            Some("hearthgate-v1.3.0-x86_64-unknown-linux-musl.tar.gz")
        );
        assert_eq!(pick_asset(names, Method::Deb, v("1.3.1"), x86), None);
        assert_eq!(pick_asset(names, Method::Manual, v("1.3.0"), x86), None);
    }

    #[test]
    fn detects_install_methods() {
        let root = tempfile::tempdir().unwrap();
        let r = root.path();
        let exe = Path::new("/usr/bin/hearthgate");
        assert_eq!(detect(r, exe), Method::Manual);
        std::fs::create_dir_all(r.join("var/lib/dpkg/info")).unwrap();
        std::fs::write(r.join("var/lib/dpkg/info/hearthgate.list"), "").unwrap();
        assert_eq!(detect(r, exe), Method::Deb);
        assert_eq!(detect(r, Path::new("/home/me/hearthgate")), Method::Manual);
        let root = tempfile::tempdir().unwrap();
        let r = root.path();
        std::fs::create_dir_all(r.join("usr/bin")).unwrap();
        std::fs::write(r.join("usr/bin/rpm"), "").unwrap();
        std::fs::create_dir_all(r.join("var/lib/rpm")).unwrap();
        assert_eq!(detect(r, exe), Method::Rpm);
        std::fs::create_dir_all(r.join("etc/systemd/system")).unwrap();
        std::fs::write(r.join("etc/systemd/system/hearthgate.service"), "").unwrap();
        assert_eq!(detect(r, Path::new("/usr/local/bin/hearthgate")), Method::Tarball);
        assert!(!helper_installed(r));
        std::fs::write(r.join("etc/systemd/system/hearthgate-upgrade.path"), "").unwrap();
        assert!(helper_installed(r));
    }
}
