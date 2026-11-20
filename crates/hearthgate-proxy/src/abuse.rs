//! Signs of a misbehaving client, reported to the firewall's ban logic.
//!
//! The data plane only reports; deciding who gets banned (and never banning
//! the LAN) is the firewall's job.

use std::net::IpAddr;
use std::sync::OnceLock;

use tokio::sync::mpsc;

/// Something a client did that legitimate clients rarely do.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Signal {
    /// The upstream answered 401 or 403.
    AuthFailure,
    /// A TLS handshake failed, or a non-TLS client spoke to a TLS port.
    HandshakeFailure,
    /// A request or TLS connection for a domain no interface serves (or no
    /// domain at all, which is how scanners connect by IP).
    UnknownHost,
    /// A request for a well-known exploit path that nothing here serves.
    Probe,
}

impl Signal {
    /// Points toward a ban.
    #[must_use]
    pub fn points(self) -> u32 {
        match self {
            Self::AuthFailure | Self::HandshakeFailure => 1,
            Self::UnknownHost => 2,
            Self::Probe => 5,
        }
    }

    #[must_use]
    pub fn describe(self) -> &'static str {
        match self {
            Self::AuthFailure => "failed login",
            Self::HandshakeFailure => "failed TLS handshake",
            Self::UnknownHost => "unknown domain",
            Self::Probe => "exploit probe",
        }
    }
}

/// Where reports go. Until something connects a receiver, reports are
/// dropped, so the proxy works on its own (and in tests).
#[derive(Debug, Default)]
pub struct AbuseReports {
    tx: OnceLock<mpsc::Sender<(IpAddr, Signal)>>,
}

impl AbuseReports {
    /// Connects the receiver. Only the first call has an effect.
    pub fn connect(&self, tx: mpsc::Sender<(IpAddr, Signal)>) {
        let _ = self.tx.set(tx);
    }

    /// Reports a signal. Never blocks: when the receiver falls behind,
    /// reports are dropped.
    pub fn report(&self, ip: IpAddr, signal: Signal) {
        if let Some(tx) = self.tx.get() {
            let _ = tx.try_send((ip.to_canonical(), signal));
        }
    }
}

/// Paths scanners try on every server: admin pages of software that is
/// probably not installed, leaked secrets and old exploits.
const PROBES: &[&str] = &[
    "/.env",
    "/.git/",
    "/.aws/",
    "/.ssh/",
    "/.DS_Store",
    "/wp-login.php",
    "/xmlrpc.php",
    "/wp-admin/",
    "/wp-content/",
    "/wp-includes/",
    "/phpmyadmin",
    "/pma/",
    "/cgi-bin/",
    "/boaform/",
    "/vendor/phpunit/",
    "/HNAP1",
    "/actuator/",
    "/solr/admin",
    "/setup.cgi",
    "/shell",
    "/manager/html",
    "/console/",
    "/owa/auth/",
    "/autodiscover/",
    "/server-status",
    "/config.json",
    "/backup.zip",
    "/admin.php",
    "/eval-stdin.php",
];

/// Whether `path` looks like a scan for software or files.
#[must_use]
pub fn is_probe_path(path: &str) -> bool {
    let lower = path.to_ascii_lowercase();
    PROBES.iter().any(|p| {
        let p = p.to_ascii_lowercase();
        lower == p.trim_end_matches('/') || lower.starts_with(&p)
    }) || lower.contains("/../")
        || lower.contains("%2e%2e")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn probe_paths() {
        assert!(is_probe_path("/.env"));
        assert!(is_probe_path("/wp-login.php"));
        assert!(is_probe_path("/.git/config"));
        assert!(is_probe_path(
            "/vendor/phpunit/phpunit/src/Util/PHP/eval-stdin.php"
        ));
        assert!(is_probe_path("/PHPMYADMIN/index.php"));
        assert!(!is_probe_path("/"));
        assert!(!is_probe_path("/api/v1/status"));
        assert!(!is_probe_path("/environment"));
    }

    #[tokio::test]
    async fn reports_reach_the_receiver() {
        let reports = AbuseReports::default();
        reports.report("192.0.2.1".parse().unwrap(), Signal::Probe);
        let (tx, mut rx) = mpsc::channel(4);
        reports.connect(tx);
        reports.report("::ffff:192.0.2.1".parse().unwrap(), Signal::UnknownHost);
        let (ip, signal) = rx.recv().await.unwrap();
        assert_eq!(ip.to_string(), "192.0.2.1");
        assert_eq!(signal, Signal::UnknownHost);
    }
}
