//! TLS: reading the ClientHello for SNI routing, and choosing certificates.

use std::collections::HashMap;
use std::io;
use std::pin::Pin;
use std::sync::{Arc, Mutex, PoisonError};
use std::task::{Context, Poll};

use arc_swap::ArcSwap;
use rustls::server::{ClientHello, ResolvesServerCert};
use rustls::sign::CertifiedKey;
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, ReadBuf};

/// ALPN protocol of the ACME TLS-ALPN-01 challenge (RFC 8737).
pub const ACME_TLS_ALPN: &[u8] = b"acme-tls/1";

/// Largest ClientHello we accept. Real ones are well under this, even with
/// post-quantum key shares.
const MAX_HELLO: usize = 32 * 1024;

/// What the ClientHello tells us before any decryption.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Hello {
    /// Requested server name, lowercased.
    pub sni: Option<String>,
    /// Offered ALPN protocols.
    pub alpn: Vec<Vec<u8>>,
}

/// Reads the TLS records that make up the ClientHello. Returns every byte
/// read (to replay into the TLS stack or to the upstream) and the parsed
/// hello. A non-TLS client yields `Ok((bytes, None))`.
pub async fn read_client_hello<S: AsyncRead + Unpin>(
    stream: &mut S,
) -> io::Result<(Vec<u8>, Option<Hello>)> {
    let mut raw = Vec::with_capacity(2048);
    let mut handshake = Vec::new();
    loop {
        let mut header = [0u8; 5];
        stream.read_exact(&mut header).await?;
        raw.extend_from_slice(&header);
        if header[0] != 0x16 {
            return Ok((raw, None));
        }
        let len = usize::from(u16::from_be_bytes([header[3], header[4]]));
        if len == 0 || raw.len() + len > MAX_HELLO {
            return Ok((raw, None));
        }
        let start = raw.len();
        raw.resize(start + len, 0);
        stream.read_exact(&mut raw[start..]).await?;
        handshake.extend_from_slice(&raw[start..]);
        // Handshake message: type (1), length (3), body.
        if handshake.len() >= 4 {
            if handshake[0] != 0x01 {
                return Ok((raw, None));
            }
            let body_len = (usize::from(handshake[1]) << 16)
                | (usize::from(handshake[2]) << 8)
                | usize::from(handshake[3]);
            if body_len + 4 > MAX_HELLO {
                return Ok((raw, None));
            }
            if handshake.len() >= body_len + 4 {
                let hello = parse_client_hello(&handshake[4..4 + body_len]);
                return Ok((raw, hello));
            }
        }
    }
}

struct Reader<'a>(&'a [u8]);

impl<'a> Reader<'a> {
    fn take(&mut self, n: usize) -> Option<&'a [u8]> {
        if self.0.len() < n {
            return None;
        }
        let (head, tail) = self.0.split_at(n);
        self.0 = tail;
        Some(head)
    }
    fn u8(&mut self) -> Option<usize> {
        self.take(1).map(|b| usize::from(b[0]))
    }
    fn u16(&mut self) -> Option<usize> {
        self.take(2)
            .map(|b| usize::from(u16::from_be_bytes([b[0], b[1]])))
    }
    fn vec8(&mut self) -> Option<&'a [u8]> {
        let n = self.u8()?;
        self.take(n)
    }
    fn vec16(&mut self) -> Option<&'a [u8]> {
        let n = self.u16()?;
        self.take(n)
    }
}

/// Parses a ClientHello handshake body (after the 4-byte message header).
/// Public for the fuzzers.
#[must_use]
pub fn parse_client_hello(body: &[u8]) -> Option<Hello> {
    let mut r = Reader(body);
    r.take(2 + 32)?; // legacy_version, random
    r.vec8()?; // session id
    r.vec16()?; // cipher suites
    r.vec8()?; // compression methods
    let mut hello = Hello::default();
    if r.0.is_empty() {
        return Some(hello);
    }
    let mut exts = Reader(r.vec16()?);
    while !exts.0.is_empty() {
        let ty = exts.u16()?;
        let data = exts.vec16()?;
        match ty {
            0 => {
                let mut list = Reader(Reader(data).vec16()?);
                while !list.0.is_empty() {
                    let kind = list.u8()?;
                    let name = list.vec16()?;
                    if kind == 0 {
                        let name = std::str::from_utf8(name).ok()?;
                        hello.sni = Some(name.trim_end_matches('.').to_ascii_lowercase());
                    }
                }
            }
            16 => {
                let mut list = Reader(Reader(data).vec16()?);
                while !list.0.is_empty() {
                    hello.alpn.push(list.vec8()?.to_vec());
                }
            }
            _ => {}
        }
    }
    Some(hello)
}

/// A stream that first returns bytes that were already read from it.
pub struct Prefixed<S> {
    prefix: Vec<u8>,
    pos: usize,
    inner: S,
}

impl<S> Prefixed<S> {
    pub fn new(prefix: Vec<u8>, inner: S) -> Self {
        Self {
            prefix,
            pos: 0,
            inner,
        }
    }
}

impl<S: AsyncRead + Unpin> AsyncRead for Prefixed<S> {
    fn poll_read(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<io::Result<()>> {
        if self.pos < self.prefix.len() {
            let n = (self.prefix.len() - self.pos).min(buf.remaining());
            let start = self.pos;
            buf.put_slice(&self.prefix[start..start + n]);
            self.pos += n;
            if self.pos == self.prefix.len() {
                self.prefix = Vec::new();
                self.pos = 0;
            }
            return Poll::Ready(Ok(()));
        }
        Pin::new(&mut self.inner).poll_read(cx, buf)
    }
}

impl<S: AsyncWrite + Unpin> AsyncWrite for Prefixed<S> {
    fn poll_write(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<io::Result<usize>> {
        Pin::new(&mut self.inner).poll_write(cx, buf)
    }
    fn poll_flush(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        Pin::new(&mut self.inner).poll_flush(cx)
    }
    fn poll_shutdown(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        Pin::new(&mut self.inner).poll_shutdown(cx)
    }
}

/// Certificates by host name, swapped atomically on every change.
#[derive(Debug, Default, Clone)]
pub struct CertTable {
    exact: HashMap<String, Arc<CertifiedKey>>,
    /// Keyed by the part after `*.`.
    wildcard: HashMap<String, Arc<CertifiedKey>>,
    /// Used when the client sends no SNI or an unknown name.
    default: Option<Arc<CertifiedKey>>,
}

impl CertTable {
    /// Adds a certificate for `domains` (which may include `*.` wildcards).
    /// The first certificate added becomes the default.
    pub fn insert(&mut self, domains: &[String], key: &Arc<CertifiedKey>) {
        for domain in domains {
            match domain.strip_prefix("*.") {
                Some(suffix) => self.wildcard.insert(suffix.to_owned(), Arc::clone(key)),
                None => self.exact.insert(domain.clone(), Arc::clone(key)),
            };
        }
        if self.default.is_none() {
            self.default = Some(Arc::clone(key));
        }
    }

    /// The certificate for `name`.
    #[must_use]
    pub fn get(&self, name: Option<&str>) -> Option<Arc<CertifiedKey>> {
        let Some(name) = name else {
            return self.default.clone();
        };
        if let Some(key) = self.exact.get(name) {
            return Some(Arc::clone(key));
        }
        if let Some((_, parent)) = name.split_once('.')
            && let Some(key) = self.wildcard.get(parent)
        {
            return Some(Arc::clone(key));
        }
        self.default.clone()
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.default.is_none()
    }
}

/// Chooses the certificate for each handshake. Shared by every TLS socket.
#[derive(Debug, Default)]
pub struct Certificates {
    table: ArcSwap<CertTable>,
    /// TLS-ALPN-01 challenge certificates by domain.
    challenges: Mutex<HashMap<String, Arc<CertifiedKey>>>,
}

impl Certificates {
    /// Replaces the certificate table. Handshakes in progress finish with
    /// the old one; open connections are not affected.
    pub fn set(&self, table: CertTable) {
        self.table.store(Arc::new(table));
    }

    /// Current certificate for a host.
    #[must_use]
    pub fn get(&self, name: Option<&str>) -> Option<Arc<CertifiedKey>> {
        self.table.load().get(name)
    }

    /// Serves `key` to ACME TLS-ALPN-01 validation for `domain`.
    pub fn set_challenge(&self, domain: &str, key: Arc<CertifiedKey>) {
        self.challenges
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .insert(domain.to_owned(), key);
    }

    /// Stops serving the TLS-ALPN-01 challenge for `domain`.
    pub fn clear_challenge(&self, domain: &str) {
        self.challenges
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .remove(domain);
    }
}

impl ResolvesServerCert for Certificates {
    fn resolve(&self, hello: ClientHello<'_>) -> Option<Arc<CertifiedKey>> {
        let name = hello.server_name().map(str::to_ascii_lowercase);
        let acme = hello
            .alpn()
            .is_some_and(|mut protos| protos.any(|p| p == ACME_TLS_ALPN));
        if acme {
            let name = name?;
            return self
                .challenges
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .get(&name)
                .cloned();
        }
        self.get(name.as_deref())
    }
}

/// Builds the server TLS config used by every `https` listener.
#[must_use]
pub fn server_config(certs: Arc<Certificates>) -> Arc<rustls::ServerConfig> {
    let provider = Arc::new(rustls::crypto::ring::default_provider());
    let mut config = rustls::ServerConfig::builder_with_provider(provider)
        .with_safe_default_protocol_versions()
        .expect("ring supports the default protocol versions")
        .with_no_client_auth()
        .with_cert_resolver(certs);
    config.alpn_protocols = vec![b"h2".to_vec(), b"http/1.1".to_vec(), ACME_TLS_ALPN.to_vec()];
    Arc::new(config)
}

/// Loads a PEM certificate chain and private key.
pub fn certified_key_from_pem(cert_pem: &[u8], key_pem: &[u8]) -> Result<CertifiedKey, String> {
    use rustls::pki_types::pem::PemObject;
    use rustls::pki_types::{CertificateDer, PrivateKeyDer};
    let chain: Vec<CertificateDer<'static>> = CertificateDer::pem_slice_iter(cert_pem)
        .collect::<Result<_, _>>()
        .map_err(|e| format!("invalid certificate PEM: {e}"))?;
    if chain.is_empty() {
        return Err("no certificate found in PEM".to_owned());
    }
    let key = PrivateKeyDer::from_pem_slice(key_pem)
        .map_err(|e| format!("invalid private key PEM: {e}"))?;
    let signing = rustls::crypto::ring::sign::any_supported_type(&key)
        .map_err(|e| format!("unsupported private key: {e}"))?;
    let certified = CertifiedKey::new(chain, signing);
    certified
        .keys_match()
        .map_err(|e| format!("certificate and key do not match: {e}"))?;
    Ok(certified)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A ClientHello captured from rustls for `app.example.com` with ALPN
    /// h2 and http/1.1, produced by the test below.
    fn hello_bytes(sni: &str, alpn: &[&[u8]]) -> Vec<u8> {
        let mut roots = rustls::RootCertStore::empty();
        let cert = rcgen::generate_simple_self_signed(vec![sni.to_owned()]).unwrap();
        roots.add(cert.cert.der().clone()).unwrap();
        let provider = Arc::new(rustls::crypto::ring::default_provider());
        let mut config = rustls::ClientConfig::builder_with_provider(provider)
            .with_safe_default_protocol_versions()
            .unwrap()
            .with_root_certificates(roots)
            .with_no_client_auth();
        config.alpn_protocols = alpn.iter().map(|p| p.to_vec()).collect();
        let name = rustls::pki_types::ServerName::try_from(sni.to_owned()).unwrap();
        let mut conn = rustls::ClientConnection::new(Arc::new(config), name).unwrap();
        let mut out = Vec::new();
        conn.write_tls(&mut out).unwrap();
        out
    }

    #[tokio::test]
    async fn parses_sni_and_alpn() {
        let bytes = hello_bytes("App.Example.com", &[b"h2", b"http/1.1"]);
        let mut reader = &bytes[..];
        let (raw, hello) = read_client_hello(&mut reader).await.unwrap();
        assert_eq!(raw, bytes, "every byte read is returned for replay");
        let hello = hello.unwrap();
        assert_eq!(hello.sni.as_deref(), Some("app.example.com"));
        assert_eq!(hello.alpn, vec![b"h2".to_vec(), b"http/1.1".to_vec()]);
    }

    #[tokio::test]
    async fn non_tls_is_detected() {
        let mut reader = &b"GET / HTTP/1.1\r\n\r\n"[..];
        let (raw, hello) = read_client_hello(&mut reader).await.unwrap();
        assert!(hello.is_none());
        assert_eq!(raw, b"GET /");
    }

    #[tokio::test]
    async fn prefixed_replays_then_reads_inner() {
        let mut s = Prefixed::new(b"ab".to_vec(), &b"cd"[..]);
        let mut out = String::new();
        s.read_to_string(&mut out).await.unwrap();
        assert_eq!(out, "abcd");
    }

    #[test]
    fn cert_table_lookup() {
        let make = |name: &str| {
            let c = rcgen::generate_simple_self_signed(vec![name.to_owned()]).unwrap();
            Arc::new(
                certified_key_from_pem(
                    c.cert.pem().as_bytes(),
                    c.signing_key.serialize_pem().as_bytes(),
                )
                .unwrap(),
            )
        };
        let a = make("a.example.com");
        let w = make("*.example.org");
        let mut t = CertTable::default();
        t.insert(&["a.example.com".into()], &a);
        t.insert(&["*.example.org".into()], &w);
        assert!(Arc::ptr_eq(&t.get(Some("a.example.com")).unwrap(), &a));
        assert!(Arc::ptr_eq(&t.get(Some("x.example.org")).unwrap(), &w));
        assert!(Arc::ptr_eq(&t.get(None).unwrap(), &a), "first is default");
        assert!(Arc::ptr_eq(&t.get(Some("unknown.net")).unwrap(), &a));
    }

    #[test]
    fn rejects_mismatched_key() {
        let a = rcgen::generate_simple_self_signed(vec!["a".to_owned()]).unwrap();
        let b = rcgen::generate_simple_self_signed(vec!["b".to_owned()]).unwrap();
        let err = certified_key_from_pem(
            a.cert.pem().as_bytes(),
            b.signing_key.serialize_pem().as_bytes(),
        )
        .unwrap_err();
        assert!(err.contains("do not match"), "{err}");
    }
}
