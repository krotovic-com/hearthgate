//! HTTPS for the admin listener.
//!
//! Unless the bootstrap file names a certificate (for example one from an
//! internal CA), Hearthgate makes a self-signed one for this machine's
//! names and addresses and keeps it, so the browser's trust decision lasts.
//! It is replaced only when it is about to expire or the machine gains an
//! address it does not cover. Its SHA-256 fingerprint is logged so the
//! browser warning can be checked against it.

use std::collections::BTreeSet;
use std::net::IpAddr;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use rcgen::{CertificateParams, DistinguishedName, DnType, KeyPair, SanType};
use rustls::pki_types::{CertificateDer, PrivateKeyDer, pem::PemObject};
use sha2::{Digest, Sha256};

/// Directory, inside the data directory, for the generated certificate.
pub const ADMIN_TLS_DIR: &str = "admin-tls";
/// Lifetime of a generated certificate. Browsers refuse trusted
/// certificates valid for more than 825 days.
const VALID_DAYS: i64 = 800;
/// Generate a new certificate this long before the old one expires.
const RENEW_DAYS: i64 = 30;

/// The admin listener's TLS settings.
pub struct AdminTls {
    pub config: Arc<rustls::ServerConfig>,
    /// SHA-256 of the certificate, as `AB:CD:…`.
    pub fingerprint: String,
    /// Where the certificate is, for the log.
    pub cert_path: PathBuf,
}

impl std::fmt::Debug for AdminTls {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AdminTls")
            .field("fingerprint", &self.fingerprint)
            .field("cert_path", &self.cert_path)
            .finish_non_exhaustive()
    }
}

/// This machine's host name.
#[must_use]
pub fn hostname() -> Option<String> {
    let host = std::fs::read_to_string("/proc/sys/kernel/hostname").ok()?;
    let host = host.trim();
    (!host.is_empty()).then(|| host.to_owned())
}

/// Names and addresses this machine is likely reached by: `localhost`, the
/// host name, `<host>.local`, and every interface address.
#[must_use]
pub fn local_names() -> BTreeSet<String> {
    let mut names: BTreeSet<String> = ["localhost", "127.0.0.1", "::1"]
        .into_iter()
        .map(str::to_owned)
        .collect();
    if let Some(host) = hostname().map(|h| h.to_lowercase())
        && host != "localhost"
    {
        names.insert(format!("{host}.local"));
        names.insert(host);
    }
    if let Ok(ifaces) = if_addrs::get_if_addrs() {
        for iface in ifaces {
            let ip = iface.ip();
            // Link-local IPv6 needs a zone, which certificates cannot hold.
            if matches!(ip, IpAddr::V6(v6) if v6.is_unicast_link_local()) {
                continue;
            }
            names.insert(ip.to_string());
        }
    }
    names
}

fn fingerprint(der: &[u8]) -> String {
    Sha256::digest(der)
        .iter()
        .map(|b| format!("{b:02X}"))
        .collect::<Vec<_>>()
        .join(":")
}

fn config_from_pem(
    cert_pem: &[u8],
    key_pem: &[u8],
    cert_path: PathBuf,
) -> Result<AdminTls, String> {
    let certs: Vec<CertificateDer<'static>> = CertificateDer::pem_slice_iter(cert_pem)
        .collect::<Result<_, _>>()
        .map_err(|e| format!("cannot read the certificate: {e}"))?;
    let first = certs
        .first()
        .ok_or("the certificate file holds no certificate")?;
    let fp = fingerprint(first);
    let key = PrivateKeyDer::from_pem_slice(key_pem)
        .map_err(|e| format!("cannot read the private key: {e}"))?;
    let mut config = rustls::ServerConfig::builder_with_provider(Arc::new(
        rustls::crypto::ring::default_provider(),
    ))
    .with_safe_default_protocol_versions()
    .map_err(|e| e.to_string())?
    .with_no_client_auth()
    .with_single_cert(certs, key)
    .map_err(|e| format!("the certificate and key don't work together: {e}"))?;
    config.alpn_protocols = vec![b"h2".to_vec(), b"http/1.1".to_vec()];
    Ok(AdminTls {
        config: Arc::new(config),
        fingerprint: fp,
        cert_path,
    })
}

/// Loads a certificate and key the user provided.
pub fn from_files(cert: &Path, key: &Path) -> Result<AdminTls, String> {
    let read = |p: &Path| std::fs::read(p).map_err(|e| format!("cannot read {}: {e}", p.display()));
    config_from_pem(&read(cert)?, &read(key)?, cert.to_owned())
}

/// Loads the generated certificate from `data_dir`, making a new one when
/// there is none, it expires soon, or it misses one of `names`.
pub fn load_or_create(data_dir: &Path, names: &BTreeSet<String>) -> Result<AdminTls, String> {
    let dir = data_dir.join(ADMIN_TLS_DIR);
    let (cert_path, key_path, meta_path) =
        (dir.join("cert.pem"), dir.join("key.pem"), dir.join("names"));
    let now = time::OffsetDateTime::now_utc().unix_timestamp();
    if let (Ok(cert), Ok(key), Ok(meta)) = (
        std::fs::read(&cert_path),
        std::fs::read(&key_path),
        std::fs::read_to_string(&meta_path),
    ) {
        let mut lines = meta.lines();
        let expires: i64 = lines.next().and_then(|l| l.parse().ok()).unwrap_or(0);
        let covered: BTreeSet<&str> = lines.collect();
        let fresh = expires - now > RENEW_DAYS * 86_400;
        if fresh && names.iter().all(|n| covered.contains(n.as_str())) {
            return config_from_pem(&cert, &key, cert_path);
        }
    }
    let (cert_pem, key_pem, expires) = generate(names)?;
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o700))
            .map_err(|e| e.to_string())?;
    }
    let write = |p: &Path, s: &str| {
        crate::auth::write_private(p, s).map_err(|e| format!("cannot write {}: {e}", p.display()))
    };
    write(&key_path, &key_pem)?;
    write(&cert_path, &cert_pem)?;
    let meta: String = std::iter::once(expires.to_string())
        .chain(names.iter().cloned())
        .collect::<Vec<_>>()
        .join("\n");
    write(&meta_path, &meta)?;
    tracing::info!(names = ?names, "made a new self-signed certificate for the dashboard");
    config_from_pem(cert_pem.as_bytes(), key_pem.as_bytes(), cert_path)
}

fn generate(names: &BTreeSet<String>) -> Result<(String, String, i64), String> {
    let mut params = CertificateParams::default();
    let mut dn = DistinguishedName::new();
    dn.push(DnType::CommonName, "Hearthgate dashboard");
    dn.push(DnType::OrganizationName, "Hearthgate");
    params.distinguished_name = dn;
    params.subject_alt_names = names
        .iter()
        .map(|n| match n.parse::<IpAddr>() {
            Ok(ip) => Ok(SanType::IpAddress(ip)),
            Err(_) => n
                .clone()
                .try_into()
                .map(SanType::DnsName)
                .map_err(|e| format!("{n}: {e}")),
        })
        .collect::<Result<_, _>>()?;
    let now = time::OffsetDateTime::now_utc();
    params.not_before = now - time::Duration::days(1);
    params.not_after = now + time::Duration::days(VALID_DAYS);
    let key = KeyPair::generate().map_err(|e| e.to_string())?;
    let cert = params.self_signed(&key).map_err(|e| e.to_string())?;
    Ok((
        cert.pem(),
        key.serialize_pem(),
        params.not_after.unix_timestamp(),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn generated_certificate_is_kept_until_names_change() {
        let dir = tempfile::tempdir().unwrap();
        let mut names: BTreeSet<String> = ["localhost".to_owned(), "192.0.2.5".to_owned()].into();
        let a = load_or_create(dir.path(), &names).unwrap();
        let b = load_or_create(dir.path(), &names).unwrap();
        assert_eq!(a.fingerprint, b.fingerprint);
        assert_eq!(a.fingerprint.len(), 32 * 3 - 1);
        // A subset is still covered.
        names.remove("192.0.2.5");
        assert_eq!(
            load_or_create(dir.path(), &names).unwrap().fingerprint,
            a.fingerprint
        );
        // A new address is not.
        names.insert("198.51.100.7".to_owned());
        assert_ne!(
            load_or_create(dir.path(), &names).unwrap().fingerprint,
            a.fingerprint
        );
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(dir.path().join(ADMIN_TLS_DIR).join("key.pem"))
                .unwrap()
                .permissions()
                .mode();
            assert_eq!(mode & 0o777, 0o600);
        }
    }

    #[test]
    fn local_names_include_loopback() {
        let names = local_names();
        assert!(names.contains("localhost") && names.contains("127.0.0.1"));
    }
}
