//! Certificates on disk.
//!
//! ```text
//! <data_dir>/certs/manual/<name>/cert.pem   uploaded with `hearthgate cert import`
//! <data_dir>/certs/manual/<name>/key.pem
//! <data_dir>/certs/acme/<interface>/cert.pem   issued automatically
//! <data_dir>/certs/acme/<interface>/key.pem
//! ```
//!
//! Directories are `0700` and key files `0600`. Files are written to a
//! temporary name and renamed, so a crash never leaves half a certificate.

use std::collections::HashMap;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use hearthgate_model::host_matches;
use hearthgate_proxy::plan::CertSpec;
use hearthgate_proxy::tls::{CertTable, certified_key_from_pem};
use rustls::sign::CertifiedKey;
use tracing::warn;

/// Errors from the certificate store.
#[derive(Debug, thiserror::Error)]
pub enum CertError {
    #[error("{0}")]
    Invalid(String),
    #[error("certificate {0} does not exist")]
    NotFound(String),
    #[error("{path}: {source}")]
    Io {
        path: PathBuf,
        #[source]
        source: io::Error,
    },
}

fn io_err(path: &Path) -> impl FnOnce(io::Error) -> CertError + '_ {
    move |source| CertError::Io {
        path: path.to_owned(),
        source,
    }
}

/// Where a certificate came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CertSource {
    Manual,
    Acme,
}

/// What a certificate covers and when it expires.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CertInfo {
    pub name: String,
    pub source: CertSource,
    /// Subject alternative names (DNS).
    pub domains: Vec<String>,
    pub not_before: SystemTime,
    pub not_after: SystemTime,
    pub issuer: String,
}

impl CertInfo {
    /// Time left before expiry (zero if expired).
    #[must_use]
    pub fn remaining(&self) -> Duration {
        self.not_after
            .duration_since(SystemTime::now())
            .unwrap_or_default()
    }

    /// Whether the certificate covers every domain in `domains`.
    #[must_use]
    pub fn covers(&self, domains: &[String]) -> bool {
        domains
            .iter()
            .all(|d| self.domains.iter().any(|c| host_matches(c, d)))
    }
}

/// Parses the first certificate of a PEM chain.
pub fn inspect_pem(name: &str, source: CertSource, cert_pem: &[u8]) -> Result<CertInfo, CertError> {
    use x509_parser::prelude::*;
    let (_, pem) = parse_x509_pem(cert_pem)
        .map_err(|e| CertError::Invalid(format!("invalid certificate PEM: {e}")))?;
    let cert = pem
        .parse_x509()
        .map_err(|e| CertError::Invalid(format!("invalid certificate: {e}")))?;
    let to_time =
        |t: ASN1Time| UNIX_EPOCH + Duration::from_secs(u64::try_from(t.timestamp()).unwrap_or(0));
    let mut domains = Vec::new();
    if let Ok(Some(san)) = cert.subject_alternative_name() {
        for name in &san.value.general_names {
            if let GeneralName::DNSName(dns) = name {
                domains.push(dns.to_ascii_lowercase());
            }
        }
    }
    if domains.is_empty() {
        if let Some(cn) = cert.subject().iter_common_name().next() {
            if let Ok(cn) = cn.as_str() {
                domains.push(cn.to_ascii_lowercase());
            }
        }
    }
    Ok(CertInfo {
        name: name.to_owned(),
        source,
        domains,
        not_before: to_time(cert.validity().not_before),
        not_after: to_time(cert.validity().not_after),
        issuer: cert.issuer().to_string(),
    })
}

/// The certificate directory.
#[derive(Debug)]
pub struct CertStore {
    root: PathBuf,
    /// Self-signed stand-ins, by domain list, until a real certificate
    /// exists. Kept in memory so reloads don't change them.
    placeholders: Mutex<HashMap<Vec<String>, Arc<CertifiedKey>>>,
}

impl CertStore {
    /// Opens the store under `data_dir`.
    #[must_use]
    pub fn new(data_dir: &Path) -> Self {
        Self {
            root: data_dir.join("certs"),
            placeholders: Mutex::new(HashMap::new()),
        }
    }

    fn dir(&self, source: CertSource, name: &str) -> PathBuf {
        let kind = match source {
            CertSource::Manual => "manual",
            CertSource::Acme => "acme",
        };
        self.root.join(kind).join(name)
    }

    /// Validates and stores a certificate and key.
    pub fn save(
        &self,
        source: CertSource,
        name: &str,
        cert_pem: &[u8],
        key_pem: &[u8],
    ) -> Result<CertInfo, CertError> {
        if name.is_empty()
            || !name
                .bytes()
                .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
        {
            return Err(CertError::Invalid(
                "certificate names use lowercase letters, digits and dashes".to_owned(),
            ));
        }
        certified_key_from_pem(cert_pem, key_pem).map_err(CertError::Invalid)?;
        let info = inspect_pem(name, source, cert_pem)?;
        let dir = self.dir(source, name);
        for d in [&self.root, &self.dir(source, ""), &dir] {
            create_private_dir(d)?;
        }
        write_atomic(&dir.join("key.pem"), key_pem, 0o600)?;
        write_atomic(&dir.join("cert.pem"), cert_pem, 0o644)?;
        self.bump_generation()?;
        Ok(info)
    }

    /// Loads a stored certificate.
    pub fn load(
        &self,
        source: CertSource,
        name: &str,
    ) -> Result<(Arc<CertifiedKey>, CertInfo), CertError> {
        let dir = self.dir(source, name);
        let cert_path = dir.join("cert.pem");
        let key_path = dir.join("key.pem");
        let cert = match fs::read(&cert_path) {
            Ok(c) => c,
            Err(e) if e.kind() == io::ErrorKind::NotFound => {
                return Err(CertError::NotFound(name.to_owned()));
            }
            Err(e) => return Err(io_err(&cert_path)(e)),
        };
        let key = fs::read(&key_path).map_err(io_err(&key_path))?;
        let certified = certified_key_from_pem(&cert, &key).map_err(CertError::Invalid)?;
        let info = inspect_pem(name, source, &cert)?;
        Ok((Arc::new(certified), info))
    }

    /// Deletes a stored certificate.
    pub fn remove(&self, source: CertSource, name: &str) -> Result<(), CertError> {
        let dir = self.dir(source, name);
        match fs::remove_dir_all(&dir) {
            Ok(()) => self.bump_generation(),
            Err(e) if e.kind() == io::ErrorKind::NotFound => {
                Err(CertError::NotFound(name.to_owned()))
            }
            Err(e) => Err(io_err(&dir)(e)),
        }
    }

    /// Changes whenever a certificate is saved or removed, so a running
    /// daemon notices certificates imported by the CLI.
    #[must_use]
    pub fn generation(&self) -> String {
        fs::read_to_string(self.root.join("generation")).unwrap_or_default()
    }

    fn bump_generation(&self) -> Result<(), CertError> {
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos();
        create_private_dir(&self.root)?;
        write_atomic(
            &self.root.join("generation"),
            now.to_string().as_bytes(),
            0o600,
        )
    }

    /// Every stored certificate.
    pub fn list(&self) -> Result<Vec<CertInfo>, CertError> {
        let mut out = Vec::new();
        for source in [CertSource::Manual, CertSource::Acme] {
            let dir = self.dir(source, "");
            let entries = match fs::read_dir(&dir) {
                Ok(e) => e,
                Err(e) if e.kind() == io::ErrorKind::NotFound => continue,
                Err(e) => return Err(io_err(&dir)(e)),
            };
            for entry in entries.flatten() {
                let name = entry.file_name().to_string_lossy().into_owned();
                let path = entry.path().join("cert.pem");
                match fs::read(&path) {
                    Ok(pem) => out.push(inspect_pem(&name, source, &pem)?),
                    Err(e) => warn!("skipping {}: {e}", path.display()),
                }
            }
        }
        out.sort_by(|a, b| a.name.cmp(&b.name));
        Ok(out)
    }

    /// A self-signed certificate for `domains`, served until a real one is
    /// available, so clients get a clear certificate warning instead of a
    /// failed handshake.
    pub fn placeholder(&self, domains: &[String]) -> Result<Arc<CertifiedKey>, CertError> {
        let mut cache = self
            .placeholders
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        if let Some(key) = cache.get(domains) {
            return Ok(Arc::clone(key));
        }
        let names = if domains.is_empty() {
            vec!["hearthgate.invalid".to_owned()]
        } else {
            domains.to_vec()
        };
        let cert = rcgen::generate_simple_self_signed(names)
            .map_err(|e| CertError::Invalid(format!("cannot create a placeholder: {e}")))?;
        let key = Arc::new(
            certified_key_from_pem(
                cert.cert.pem().as_bytes(),
                cert.signing_key.serialize_pem().as_bytes(),
            )
            .map_err(CertError::Invalid)?,
        );
        cache.insert(domains.to_vec(), Arc::clone(&key));
        Ok(key)
    }

    /// Builds the table of certificates the `https` interfaces need: manual
    /// ones by name, automatic ones by interface, and self-signed stand-ins
    /// for anything missing. Returns the table and a note for each stand-in.
    #[must_use]
    pub fn table(&self, specs: &[CertSpec]) -> (CertTable, Vec<String>) {
        let mut table = CertTable::default();
        let mut notes = Vec::new();
        for spec in specs {
            let (source, name) = match &spec.manual {
                Some(name) => (CertSource::Manual, name.as_str()),
                None => (CertSource::Acme, spec.interface.as_str()),
            };
            let key = match self.load(source, name) {
                Ok((key, info)) if source == CertSource::Manual || info.covers(&spec.domains) => {
                    if !info.covers(&spec.domains) {
                        notes.push(format!(
                            "certificate {name} does not cover every domain of interface {}",
                            spec.interface
                        ));
                    }
                    Some(key)
                }
                Ok(_) => {
                    notes.push(format!(
                        "interface {}: the automatic certificate does not match its domains yet",
                        spec.interface
                    ));
                    None
                }
                Err(CertError::NotFound(_)) => {
                    notes.push(match source {
                        CertSource::Manual => format!(
                            "interface {}: certificate {name} is not uploaded (use `hearthgate cert import`)",
                            spec.interface
                        ),
                        CertSource::Acme => format!(
                            "interface {}: no automatic certificate yet",
                            spec.interface
                        ),
                    });
                    None
                }
                Err(e) => {
                    notes.push(format!("interface {}: {e}", spec.interface));
                    None
                }
            };
            let key = match key {
                Some(k) => k,
                None => match self.placeholder(&spec.domains) {
                    Ok(k) => k,
                    Err(e) => {
                        notes.push(e.to_string());
                        continue;
                    }
                },
            };
            table.insert(&spec.domains, &key);
        }
        (table, notes)
    }
}

#[cfg(unix)]
fn set_mode(path: &Path, mode: u32) -> io::Result<()> {
    use std::os::unix::fs::PermissionsExt;
    fs::set_permissions(path, fs::Permissions::from_mode(mode))
}

#[cfg(not(unix))]
fn set_mode(_path: &Path, _mode: u32) -> io::Result<()> {
    Ok(())
}

/// Creates `dir` (and missing parents) and makes it owner-only.
pub fn create_private_dir(dir: &Path) -> Result<(), CertError> {
    fs::create_dir_all(dir).map_err(io_err(dir))?;
    set_mode(dir, 0o700).map_err(io_err(dir))
}

/// Writes a file through a temporary name and a rename.
pub fn write_atomic(path: &Path, data: &[u8], mode: u32) -> Result<(), CertError> {
    let tmp = path.with_extension("tmp");
    {
        use std::io::Write;
        #[cfg(unix)]
        let mut file = {
            use std::os::unix::fs::OpenOptionsExt;
            fs::OpenOptions::new()
                .write(true)
                .create(true)
                .truncate(true)
                .mode(mode)
                .open(&tmp)
                .map_err(io_err(&tmp))?
        };
        #[cfg(not(unix))]
        let mut file = fs::File::create(&tmp).map_err(io_err(&tmp))?;
        file.write_all(data).map_err(io_err(&tmp))?;
        file.sync_all().map_err(io_err(&tmp))?;
    }
    set_mode(&tmp, mode).map_err(io_err(&tmp))?;
    fs::rename(&tmp, path).map_err(io_err(path))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn self_signed(names: &[&str]) -> (String, String) {
        let c = rcgen::generate_simple_self_signed(
            names.iter().map(|s| (*s).to_owned()).collect::<Vec<_>>(),
        )
        .unwrap();
        (c.cert.pem(), c.signing_key.serialize_pem())
    }

    #[test]
    fn save_load_list_remove() {
        let dir = tempfile::tempdir().unwrap();
        let store = CertStore::new(dir.path());
        let (cert, key) = self_signed(&["a.example.com", "*.b.example.com"]);
        let info = store
            .save(CertSource::Manual, "mine", cert.as_bytes(), key.as_bytes())
            .unwrap();
        assert_eq!(info.domains, ["a.example.com", "*.b.example.com"]);
        assert!(info.covers(&["a.example.com".into(), "x.b.example.com".into()]));
        assert!(!info.covers(&["c.example.com".into()]));
        assert!(info.remaining() > Duration::from_secs(3600));
        let (_, loaded) = store.load(CertSource::Manual, "mine").unwrap();
        assert_eq!(loaded, info);
        assert_eq!(store.list().unwrap().len(), 1);
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = fs::metadata(dir.path().join("certs/manual/mine/key.pem"))
                .unwrap()
                .permissions()
                .mode();
            assert_eq!(mode & 0o777, 0o600);
        }
        store.remove(CertSource::Manual, "mine").unwrap();
        assert!(matches!(
            store.load(CertSource::Manual, "mine"),
            Err(CertError::NotFound(_))
        ));
    }

    #[test]
    fn rejects_bad_input() {
        let dir = tempfile::tempdir().unwrap();
        let store = CertStore::new(dir.path());
        let (cert, _) = self_signed(&["a"]);
        let (_, other_key) = self_signed(&["b"]);
        assert!(
            store
                .save(
                    CertSource::Manual,
                    "x",
                    cert.as_bytes(),
                    other_key.as_bytes()
                )
                .is_err()
        );
        assert!(
            store
                .save(
                    CertSource::Manual,
                    "../x",
                    cert.as_bytes(),
                    other_key.as_bytes()
                )
                .is_err()
        );
    }

    #[test]
    fn table_uses_placeholders_for_missing_certificates() {
        let dir = tempfile::tempdir().unwrap();
        let store = CertStore::new(dir.path());
        let spec = CertSpec {
            interface: "web".into(),
            domains: vec!["app.example.com".into()],
            manual: None,
        };
        let (table, notes) = store.table(std::slice::from_ref(&spec));
        assert!(table.get(Some("app.example.com")).is_some());
        assert_eq!(notes.len(), 1);
        // The placeholder is stable across reloads.
        let (again, _) = store.table(std::slice::from_ref(&spec));
        assert!(Arc::ptr_eq(
            &table.get(Some("app.example.com")).unwrap(),
            &again.get(Some("app.example.com")).unwrap()
        ));
        // Once a certificate exists it is used.
        let (cert, key) = self_signed(&["app.example.com"]);
        store
            .save(CertSource::Acme, "web", cert.as_bytes(), key.as_bytes())
            .unwrap();
        let (table, notes) = store.table(&[spec]);
        assert!(notes.is_empty(), "{notes:?}");
        assert!(!Arc::ptr_eq(
            &table.get(Some("app.example.com")).unwrap(),
            &again.get(Some("app.example.com")).unwrap()
        ));
    }
}
