//! HTTP, HTTPS, WebSockets and TLS passthrough through a real gateway.
#![allow(clippy::format_push_string)]

use std::convert::Infallible;
use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;

use bytes::Bytes;
use hearthgate_model::DesiredState;
use hearthgate_proxy::tls::{CertTable, certified_key_from_pem};
use hearthgate_proxy::{Gateway, NoContainers, compile};
use http_body_util::{BodyExt, Empty, Full};
use hyper::{Request, Response};
use hyper_util::rt::{TokioExecutor, TokioIo};
use rustls::pki_types::{CertificateDer, ServerName};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};

fn free_port() -> u16 {
    std::net::TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
        .port()
}

fn addr(port: u16) -> SocketAddr {
    SocketAddr::from(([127, 0, 0, 1], port))
}

/// An HTTP upstream that describes the request it got.
async fn describe_server(tag: &'static str) -> u16 {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    tokio::spawn(async move {
        loop {
            let (s, _) = listener.accept().await.unwrap();
            tokio::spawn(async move {
                let svc = hyper::service::service_fn(
                    move |req: Request<hyper::body::Incoming>| async move {
                        let h = |n: &str| {
                            req.headers()
                                .get(n)
                                .and_then(|v| v.to_str().ok())
                                .unwrap_or("-")
                                .to_owned()
                        };
                        let line = format!(
                            "{tag} {} {} host={} xff={} proto={} xfh={}",
                            req.method(),
                            req.uri(),
                            h("host"),
                            h("x-forwarded-for"),
                            h("x-forwarded-proto"),
                            h("x-forwarded-host"),
                        );
                        let body = req.into_body().collect().await.unwrap().to_bytes();
                        let text = if body.is_empty() {
                            line
                        } else {
                            format!("{line} body={}", String::from_utf8_lossy(&body))
                        };
                        Ok::<_, Infallible>(Response::new(Full::new(Bytes::from(text))))
                    },
                );
                let _ = hyper::server::conn::http1::Builder::new()
                    .serve_connection(TokioIo::new(s), svc)
                    .await;
            });
        }
    });
    port
}

/// A raw upstream that accepts a WebSocket-style upgrade and echoes bytes.
async fn upgrade_echo_server() -> u16 {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    tokio::spawn(async move {
        loop {
            let (mut s, _) = listener.accept().await.unwrap();
            tokio::spawn(async move {
                let mut head = Vec::new();
                let mut b = [0u8; 1];
                while !head.ends_with(b"\r\n\r\n") {
                    if s.read(&mut b).await.unwrap_or(0) == 0 {
                        return;
                    }
                    head.push(b[0]);
                }
                let head = String::from_utf8_lossy(&head).to_ascii_lowercase();
                assert!(head.contains("upgrade: websocket"), "{head}");
                s.write_all(
                    b"HTTP/1.1 101 Switching Protocols\r\nConnection: Upgrade\r\nUpgrade: websocket\r\n\r\n",
                )
                .await
                .unwrap();
                let mut buf = [0u8; 256];
                loop {
                    match s.read(&mut buf).await {
                        Ok(0) | Err(_) => break,
                        Ok(n) => {
                            let mut out = b"echo:".to_vec();
                            out.extend_from_slice(&buf[..n]);
                            s.write_all(&out).await.unwrap();
                        }
                    }
                }
            });
        }
    });
    port
}

struct SelfSigned {
    cert_pem: String,
    key_pem: String,
    der: CertificateDer<'static>,
}

fn self_signed(names: &[&str]) -> SelfSigned {
    let c = rcgen::generate_simple_self_signed(
        names.iter().map(|s| (*s).to_owned()).collect::<Vec<_>>(),
    )
    .unwrap();
    SelfSigned {
        cert_pem: c.cert.pem(),
        key_pem: c.signing_key.serialize_pem(),
        der: c.cert.der().clone(),
    }
}

/// A TLS upstream (for passthrough) that answers every connection with
/// `tag`.
async fn tls_server(cert: &SelfSigned, tag: &'static str) -> u16 {
    let key = certified_key_from_pem(cert.cert_pem.as_bytes(), cert.key_pem.as_bytes()).unwrap();
    let mut table = CertTable::default();
    table.insert(&["*".to_owned()], &Arc::new(key));
    let certs = Arc::new(hearthgate_proxy::tls::Certificates::default());
    certs.set(table);
    let acceptor = tokio_rustls::TlsAcceptor::from(hearthgate_proxy::tls::server_config(certs));
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    tokio::spawn(async move {
        loop {
            let (s, _) = listener.accept().await.unwrap();
            let acceptor = acceptor.clone();
            tokio::spawn(async move {
                if let Ok(mut tls) = acceptor.accept(s).await {
                    let _ = tls.write_all(tag.as_bytes()).await;
                    let _ = tls.shutdown().await;
                }
            });
        }
    });
    port
}

fn tls_client(root: &CertificateDer<'static>, alpn: &[&[u8]]) -> tokio_rustls::TlsConnector {
    let mut roots = rustls::RootCertStore::empty();
    roots.add(root.clone()).unwrap();
    let provider = Arc::new(rustls::crypto::ring::default_provider());
    let mut config = rustls::ClientConfig::builder_with_provider(provider)
        .with_safe_default_protocol_versions()
        .unwrap()
        .with_root_certificates(roots)
        .with_no_client_auth();
    config.alpn_protocols = alpn.iter().map(|p| p.to_vec()).collect();
    tokio_rustls::TlsConnector::from(Arc::new(config))
}

async fn http_get(port: u16, host: &str, path: &str) -> (u16, String, hyper::HeaderMap) {
    let stream = TcpStream::connect(addr(port)).await.unwrap();
    let (mut send, conn) = hyper::client::conn::http1::handshake(TokioIo::new(stream))
        .await
        .unwrap();
    tokio::spawn(conn);
    let req = Request::get(path)
        .header("host", host)
        .header("x-forwarded-for", "6.6.6.6")
        .body(Empty::<Bytes>::new())
        .unwrap();
    let resp = send.send_request(req).await.unwrap();
    let status = resp.status().as_u16();
    let headers = resp.headers().clone();
    let body = resp.into_body().collect().await.unwrap().to_bytes();
    (status, String::from_utf8_lossy(&body).into_owned(), headers)
}

async fn https_get(
    port: u16,
    host: &str,
    path: &str,
    root: &CertificateDer<'static>,
    h2: bool,
) -> (u16, String) {
    let alpn: &[&[u8]] = if h2 { &[b"h2"] } else { &[b"http/1.1"] };
    let stream = TcpStream::connect(addr(port)).await.unwrap();
    let tls = tls_client(root, alpn)
        .connect(ServerName::try_from(host.to_owned()).unwrap(), stream)
        .await
        .expect("TLS handshake with the gateway");
    let io = TokioIo::new(tls);
    let resp = if h2 {
        let (mut send, conn) = hyper::client::conn::http2::handshake(TokioExecutor::new(), io)
            .await
            .unwrap();
        tokio::spawn(conn);
        let req = Request::get(format!("https://{host}{path}"))
            .body(Empty::<Bytes>::new())
            .unwrap();
        send.send_request(req).await.unwrap()
    } else {
        let (mut send, conn) = hyper::client::conn::http1::handshake(io).await.unwrap();
        tokio::spawn(conn);
        let req = Request::get(path)
            .header("host", host)
            .body(Empty::<Bytes>::new())
            .unwrap();
        send.send_request(req).await.unwrap()
    };
    let status = resp.status().as_u16();
    let body = resp.into_body().collect().await.unwrap().to_bytes();
    (status, String::from_utf8_lossy(&body).into_owned())
}

async fn start(state: &str, certs: &[(&[&str], &SelfSigned)]) -> Gateway {
    let state = DesiredState::from_toml(state).unwrap();
    let gw = Gateway::new();
    let plan = compile(&state, gw.upstreams(), &NoContainers);
    let mut table = CertTable::default();
    for (domains, cert) in certs {
        let key =
            certified_key_from_pem(cert.cert_pem.as_bytes(), cert.key_pem.as_bytes()).unwrap();
        let domains: Vec<String> = domains.iter().map(|d| (*d).to_owned()).collect();
        table.insert(&domains, &Arc::new(key));
    }
    gw.certificates().set(table);
    let errors = gw.apply(&plan).await;
    assert!(errors.is_empty(), "{errors:?}");
    gw
}

#[tokio::test]
async fn routes_by_host_and_path_with_forwarding_headers() {
    let app = describe_server("app").await;
    let api = describe_server("api").await;
    let http = free_port();
    let gw = start(
        &format!(
            r#"
[[interfaces]]
name = "web"
bind = ["127.0.0.1"]
domains = ["a.example.com", "b.example.com"]
listeners = [{{ port = {http}, mode = "http" }}]
[[services]]
name = "app"
kind = "external"
host = "127.0.0.1"
ports = {{ http = {app} }}
[[services]]
name = "api"
kind = "external"
host = "127.0.0.1"
ports = {{ http = {api} }}
[[routes]]
name = "app"
interface = "web"
listener = {http}
hosts = ["a.example.com"]
targets = [{{ service = "app", port = "http" }}]
[[routes]]
name = "api"
interface = "web"
listener = {http}
hosts = ["a.example.com"]
path_prefix = "/api"
strip_prefix = true
targets = [{{ service = "api", port = "http" }}]
"#
        ),
        &[],
    )
    .await;

    let (status, body, _) = http_get(http, "a.example.com", "/hello?x=1").await;
    assert_eq!(status, 200);
    assert_eq!(
        body, "app GET /hello?x=1 host=a.example.com xff=127.0.0.1 proto=http xfh=a.example.com",
        "forged X-Forwarded-For from the client is replaced"
    );
    let (_, body, _) = http_get(http, "A.Example.com:80", "/api/users").await;
    assert!(body.starts_with("api GET /users "), "{body}");
    let (_, body, _) = http_get(http, "a.example.com", "/api").await;
    assert!(body.starts_with("api GET / "), "{body}");
    let (_, body, _) = http_get(http, "a.example.com", "/apiary").await;
    assert!(body.starts_with("app GET /apiary"), "{body}");
    let (status, _, _) = http_get(http, "b.example.com", "/").await;
    assert_eq!(status, 404, "b has no route and no https listener");
    gw.shutdown(Duration::from_millis(100)).await;
}

#[tokio::test]
async fn https_termination_redirects_and_acme_http01() {
    let app = describe_server("app").await;
    let (http, https) = (free_port(), free_port());
    let cert = self_signed(&["app.example.com"]);
    let gw = start(
        &format!(
            r#"
[[interfaces]]
name = "web"
bind = ["127.0.0.1"]
domains = ["app.example.com"]
certificate = "mine"
listeners = [{{ port = {http}, mode = "http" }}, {{ port = {https}, mode = "https" }}]
[[services]]
name = "app"
kind = "external"
host = "127.0.0.1"
ports = {{ http = {app} }}
[[routes]]
name = "app"
interface = "web"
listener = {https}
targets = [{{ service = "app", port = "http" }}]
"#
        ),
        &[(&["app.example.com"], &cert)],
    )
    .await;

    for h2 in [false, true] {
        let (status, body) = https_get(https, "app.example.com", "/secure", &cert.der, h2).await;
        assert_eq!(status, 200, "h2={h2}");
        assert!(
            body.starts_with("app GET /secure host=app.example.com"),
            "{body}"
        );
        assert!(body.contains("proto=https"), "{body}");
    }

    let (status, _, headers) = http_get(http, "app.example.com", "/x?y=1").await;
    assert_eq!(status, 308);
    assert_eq!(
        headers.get("location").unwrap(),
        &format!("https://app.example.com:{https}/x?y=1")
    );

    gw.http_challenges().set("tok123", "tok123.thumb");
    let (status, body, _) = http_get(
        http,
        "app.example.com",
        "/.well-known/acme-challenge/tok123",
    )
    .await;
    assert_eq!((status, body.as_str()), (200, "tok123.thumb"));
    let (status, _, _) =
        http_get(http, "app.example.com", "/.well-known/acme-challenge/nope").await;
    assert_eq!(status, 308);

    // Swapping the certificate applies to new handshakes without a reload.
    let other = self_signed(&["app.example.com"]);
    let mut table = CertTable::default();
    let key = certified_key_from_pem(other.cert_pem.as_bytes(), other.key_pem.as_bytes()).unwrap();
    table.insert(&["app.example.com".to_owned()], &Arc::new(key));
    gw.certificates().set(table);
    let (status, _) = https_get(https, "app.example.com", "/", &other.der, false).await;
    assert_eq!(status, 200);
    gw.shutdown(Duration::from_millis(100)).await;
}

#[tokio::test]
async fn passthrough_and_termination_share_a_port() {
    let app = describe_server("app").await;
    let tls_port = free_port();
    let edge = self_signed(&["app.example.com"]);
    let inner = self_signed(&["pass.example.com"]);
    let upstream = tls_server(&inner, "from-upstream").await;
    let gw = start(
        &format!(
            r#"
[[interfaces]]
name = "web"
bind = ["127.0.0.1"]
domains = ["app.example.com"]
certificate = "edge"
listeners = [{{ port = {tls_port}, mode = "https" }}]
[[interfaces]]
name = "pass"
bind = ["127.0.0.1"]
domains = ["pass.example.com"]
listeners = [{{ port = {tls_port}, mode = "tls-passthrough" }}]
[[services]]
name = "app"
kind = "external"
host = "127.0.0.1"
ports = {{ http = {app} }}
[[services]]
name = "inner"
kind = "external"
host = "127.0.0.1"
ports = {{ tls = {upstream} }}
[[routes]]
name = "app"
interface = "web"
listener = {tls_port}
targets = [{{ service = "app", port = "http" }}]
[[routes]]
name = "pass"
interface = "pass"
listener = {tls_port}
targets = [{{ service = "inner", port = "tls" }}]
"#
        ),
        &[(&["app.example.com"], &edge)],
    )
    .await;

    // Terminated by Hearthgate.
    let (status, body) = https_get(tls_port, "app.example.com", "/", &edge.der, false).await;
    assert_eq!(status, 200, "{body}");

    // Passed through: the client sees the upstream's own certificate.
    let stream = TcpStream::connect(addr(tls_port)).await.unwrap();
    let mut tls = tls_client(&inner.der, &[])
        .connect(ServerName::try_from("pass.example.com").unwrap(), stream)
        .await
        .expect("handshake with the upstream through the gateway");
    let mut out = String::new();
    tls.read_to_string(&mut out).await.unwrap();
    assert_eq!(out, "from-upstream");

    // Unknown names get no handshake.
    let stream = TcpStream::connect(addr(tls_port)).await.unwrap();
    let result = tls_client(&edge.der, &[])
        .connect(ServerName::try_from("other.example.com").unwrap(), stream)
        .await;
    assert!(result.is_err());
    gw.shutdown(Duration::from_millis(100)).await;
}

#[tokio::test]
async fn websocket_upgrade_and_body_limits() {
    let ws = upgrade_echo_server().await;
    let app = describe_server("app").await;
    let http = free_port();
    let gw = start(
        &format!(
            r#"
limits = {{ max_body_bytes = 16 }}
[[interfaces]]
name = "web"
bind = ["127.0.0.1"]
listeners = [{{ port = {http}, mode = "http" }}]
[[services]]
name = "ws"
kind = "external"
host = "127.0.0.1"
ports = {{ http = {ws} }}
[[services]]
name = "app"
kind = "external"
host = "127.0.0.1"
ports = {{ http = {app} }}
[[routes]]
name = "ws"
interface = "web"
listener = {http}
path_prefix = "/ws"
targets = [{{ service = "ws", port = "http" }}]
[[routes]]
name = "app"
interface = "web"
listener = {http}
targets = [{{ service = "app", port = "http" }}]
"#
        ),
        &[],
    )
    .await;

    let mut s = TcpStream::connect(addr(http)).await.unwrap();
    s.write_all(
        b"GET /ws HTTP/1.1\r\nHost: any\r\nConnection: Upgrade\r\nUpgrade: websocket\r\nSec-WebSocket-Version: 13\r\nSec-WebSocket-Key: dGhlIHNhbXBsZSBub25jZQ==\r\n\r\n",
    )
    .await
    .unwrap();
    let mut head = Vec::new();
    let mut b = [0u8; 1];
    while !head.ends_with(b"\r\n\r\n") {
        s.read_exact(&mut b).await.unwrap();
        head.push(b[0]);
    }
    assert!(
        String::from_utf8_lossy(&head).starts_with("HTTP/1.1 101"),
        "{}",
        String::from_utf8_lossy(&head)
    );
    s.write_all(b"ping").await.unwrap();
    let mut buf = [0u8; 64];
    let n = tokio::time::timeout(Duration::from_secs(5), s.read(&mut buf))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(&buf[..n], b"echo:ping");

    // A declared body over the limit is refused before it reaches the upstream.
    let mut s = TcpStream::connect(addr(http)).await.unwrap();
    s.write_all(b"POST / HTTP/1.1\r\nHost: any\r\nContent-Length: 100\r\n\r\n")
        .await
        .unwrap();
    let mut buf = [0u8; 64];
    let n = s.read(&mut buf).await.unwrap();
    assert!(String::from_utf8_lossy(&buf[..n]).starts_with("HTTP/1.1 413"));

    // Small bodies pass.
    let stream = TcpStream::connect(addr(http)).await.unwrap();
    let (mut send, conn) = hyper::client::conn::http1::handshake(TokioIo::new(stream))
        .await
        .unwrap();
    tokio::spawn(conn);
    let resp = send
        .send_request(
            Request::post("/")
                .header("host", "any")
                .body(Full::new(Bytes::from("tiny")))
                .unwrap(),
        )
        .await
        .unwrap();
    let body = resp.into_body().collect().await.unwrap().to_bytes();
    assert!(String::from_utf8_lossy(&body).ends_with("body=tiny"));
    gw.shutdown(Duration::from_millis(100)).await;
}
