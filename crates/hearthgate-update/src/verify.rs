//! Checks that a release really comes from the Hearthgate maintainers.
//!
//! A release has `SHA256SUMS` (one line per asset) and `SHA256SUMS.minisig`,
//! a minisign signature of it whose trusted comment names the version. A
//! file is accepted only when the signature is valid for one of the
//! [release keys](crate::keys), the trusted comment names the version we
//! expect, and the file's SHA-256 matches its line.

use std::collections::BTreeMap;
use std::io::Read;
use std::path::Path;

use minisign_verify::{PublicKey, Signature};
use sha2::{Digest, Sha256};

use crate::Version;

/// Largest `SHA256SUMS` accepted.
pub const MAX_SUMS_BYTES: usize = 64 * 1024;
/// Largest signature file accepted.
pub const MAX_SIGNATURE_BYTES: usize = 4 * 1024;

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum VerifyError {
    #[error("the signature file is not a minisign signature")]
    BadSignatureFile,
    #[error("SHA256SUMS is not signed by the Hearthgate release key")]
    NotSigned,
    #[error("the signature is for {found:?}, not for Hearthgate {expected}")]
    WrongVersion { expected: Version, found: String },
    #[error("SHA256SUMS is malformed: {0}")]
    BadSums(String),
    #[error("{0} is not listed in SHA256SUMS")]
    NotListed(String),
    #[error("{0} does not match its checksum")]
    Mismatch(String),
    #[error("cannot read {0}: {1}")]
    Io(String, String),
}

/// The checksums of a release whose signature checked out.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Sums {
    pub version: Version,
    entries: BTreeMap<String, [u8; 32]>,
}

impl Sums {
    /// File names listed, in order.
    pub fn names(&self) -> impl Iterator<Item = &str> {
        self.entries.keys().map(String::as_str)
    }

    /// Checks `data` against the line for `name`.
    pub fn check_bytes(&self, name: &str, data: &[u8]) -> Result<(), VerifyError> {
        let want = self
            .entries
            .get(name)
            .ok_or_else(|| VerifyError::NotListed(name.to_owned()))?;
        if Sha256::digest(data).as_slice() == want {
            Ok(())
        } else {
            Err(VerifyError::Mismatch(name.to_owned()))
        }
    }

    /// Checks the file at `path` against the line for `name`.
    pub fn check_file(&self, name: &str, path: &Path) -> Result<(), VerifyError> {
        let want = self
            .entries
            .get(name)
            .ok_or_else(|| VerifyError::NotListed(name.to_owned()))?;
        let io = |e: std::io::Error| VerifyError::Io(name.to_owned(), e.to_string());
        let mut file = crate::files::open_nofollow(path).map_err(io)?;
        let mut hasher = Sha256::new();
        let mut buf = vec![0u8; 64 * 1024];
        loop {
            let n = file.read(&mut buf).map_err(io)?;
            if n == 0 {
                break;
            }
            hasher.update(&buf[..n]);
        }
        if hasher.finalize().as_slice() == want {
            Ok(())
        } else {
            Err(VerifyError::Mismatch(name.to_owned()))
        }
    }
}

/// Verifies `SHA256SUMS` for `version` against `keys`, and parses it.
pub fn verify_sums(
    sums: &[u8],
    signature: &str,
    version: Version,
    keys: &[PublicKey],
) -> Result<Sums, VerifyError> {
    if sums.len() > MAX_SUMS_BYTES {
        return Err(VerifyError::BadSums("too large".into()));
    }
    if signature.len() > MAX_SIGNATURE_BYTES {
        return Err(VerifyError::BadSignatureFile);
    }
    let sig = Signature::decode(signature).map_err(|_| VerifyError::BadSignatureFile)?;
    if !keys.iter().any(|k| k.verify(sums, &sig, false).is_ok()) {
        return Err(VerifyError::NotSigned);
    }
    // Only now is the trusted comment trustworthy.
    let comment = sig.trusted_comment();
    let tag = format!("v{version}");
    let words: Vec<&str> = comment.split_whitespace().collect();
    if !(words.contains(&"hearthgate") && words.contains(&tag.as_str())) {
        return Err(VerifyError::WrongVersion {
            expected: version,
            found: comment.chars().take(80).collect(),
        });
    }
    let text = std::str::from_utf8(sums).map_err(|_| VerifyError::BadSums("not text".into()))?;
    let mut entries = BTreeMap::new();
    for line in text.lines().filter(|l| !l.trim().is_empty()) {
        let (hash, name) = line
            .split_once(char::is_whitespace)
            .ok_or_else(|| VerifyError::BadSums(format!("line {line:?}")))?;
        let name = name.trim_start().trim_start_matches('*');
        if !is_plain_name(name) {
            return Err(VerifyError::BadSums(format!("file name {name:?}")));
        }
        let digest =
            parse_hex(hash).ok_or_else(|| VerifyError::BadSums(format!("checksum for {name}")))?;
        entries.insert(name.to_owned(), digest);
    }
    if entries.is_empty() {
        return Err(VerifyError::BadSums("no files".into()));
    }
    Ok(Sums { version, entries })
}

/// A file name with no directories, as release assets have.
#[must_use]
pub fn is_plain_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 200
        && name != "."
        && name != ".."
        && name
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'_' | b'-' | b'+'))
}

fn parse_hex(text: &str) -> Option<[u8; 32]> {
    if text.len() != 64 {
        return None;
    }
    let mut out = [0u8; 32];
    for (i, chunk) in text.as_bytes().chunks(2).enumerate() {
        let s = std::str::from_utf8(chunk).ok()?;
        out[i] = u8::from_str_radix(s, 16).ok()?;
    }
    Some(out)
}

/// Test helpers: a throwaway key and signed releases.
#[cfg(test)]
pub(crate) mod testkey {
    use std::io::Cursor;

    use minisign::KeyPair;
    use sha2::{Digest, Sha256};

    pub struct TestKey {
        pair: KeyPair,
    }

    impl TestKey {
        pub fn new() -> Self {
            Self {
                pair: KeyPair::generate_unencrypted_keypair().unwrap(),
            }
        }

        pub fn public(&self) -> minisign_verify::PublicKey {
            minisign_verify::PublicKey::from_base64(&self.pair.pk.to_base64()).unwrap()
        }

        pub fn sign(&self, data: &[u8], trusted: &str) -> String {
            minisign::sign(
                Some(&self.pair.pk),
                &self.pair.sk,
                Cursor::new(data),
                Some(trusted),
                None,
            )
            .unwrap()
            .to_string()
        }

        /// `SHA256SUMS` for `files` and its signature for `version`.
        pub fn release(&self, version: &str, files: &[(&str, &[u8])]) -> (Vec<u8>, String) {
            let mut sums = String::new();
            for (name, data) in files {
                use std::fmt::Write;
                for b in Sha256::digest(data) {
                    let _ = write!(sums, "{b:02x}");
                }
                let _ = writeln!(sums, "  {name}");
            }
            let sig = self.sign(
                sums.as_bytes(),
                &format!("hearthgate v{version} SHA256SUMS"),
            );
            (sums.into_bytes(), sig)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::testkey::TestKey;
    use super::*;

    fn v(s: &str) -> Version {
        Version::parse(s).unwrap()
    }

    #[test]
    fn accepts_a_signed_release() {
        let key = TestKey::new();
        let (sums, sig) = key.release("1.3.0", &[("a.deb", b"package"), ("b.tar.gz", b"tarball")]);
        let checked = verify_sums(&sums, &sig, v("1.3.0"), &[key.public()]).unwrap();
        assert_eq!(checked.names().collect::<Vec<_>>(), ["a.deb", "b.tar.gz"]);
        checked.check_bytes("a.deb", b"package").unwrap();
        assert_eq!(
            checked.check_bytes("a.deb", b"evil"),
            Err(VerifyError::Mismatch("a.deb".into()))
        );
        assert_eq!(
            checked.check_bytes("c.rpm", b"x"),
            Err(VerifyError::NotListed("c.rpm".into()))
        );
    }

    #[test]
    fn refuses_tampering_and_other_keys() {
        let key = TestKey::new();
        let other = TestKey::new();
        let (sums, sig) = key.release("1.3.0", &[("a.deb", b"package")]);
        let mut tampered = sums.clone();
        tampered[0] = if tampered[0] == b'0' { b'1' } else { b'0' };
        assert_eq!(
            verify_sums(&tampered, &sig, v("1.3.0"), &[key.public()]),
            Err(VerifyError::NotSigned)
        );
        assert_eq!(
            verify_sums(&sums, &sig, v("1.3.0"), &[other.public()]),
            Err(VerifyError::NotSigned)
        );
        assert_eq!(
            verify_sums(&sums, "junk", v("1.3.0"), &[key.public()]),
            Err(VerifyError::BadSignatureFile)
        );
    }

    #[test]
    fn refuses_a_signature_for_another_version() {
        // An old, validly signed release served under a new name.
        let key = TestKey::new();
        let (sums, sig) = key.release("1.2.0", &[("a.deb", b"old package")]);
        assert!(matches!(
            verify_sums(&sums, &sig, v("1.3.0"), &[key.public()]),
            Err(VerifyError::WrongVersion { .. })
        ));
        let sig = key.sign(&sums, "hearthgate v1.3.0.1");
        assert!(matches!(
            verify_sums(&sums, &sig, v("1.3.0"), &[key.public()]),
            Err(VerifyError::WrongVersion { .. })
        ));
    }

    #[test]
    fn refuses_paths_in_names() {
        let key = TestKey::new();
        for name in ["../hearthgate", "/usr/bin/hearthgate", "a/b", ".."] {
            let sums = format!("{}  {name}\n", "a".repeat(64));
            let sig = key.sign(sums.as_bytes(), "hearthgate v1.3.0");
            assert!(
                matches!(
                    verify_sums(sums.as_bytes(), &sig, v("1.3.0"), &[key.public()]),
                    Err(VerifyError::BadSums(_))
                ),
                "{name}"
            );
        }
    }

    #[test]
    fn checks_files_on_disk() {
        let key = TestKey::new();
        let (sums, sig) = key.release("1.3.0", &[("a.deb", b"package")]);
        let checked = verify_sums(&sums, &sig, v("1.3.0"), &[key.public()]).unwrap();
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("a.deb");
        std::fs::write(&path, b"package").unwrap();
        checked.check_file("a.deb", &path).unwrap();
        std::fs::write(&path, b"packagf").unwrap();
        assert!(checked.check_file("a.deb", &path).is_err());
    }
}
