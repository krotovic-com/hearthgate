//! HTTP reverse proxy: host and path routing, redirects, ACME HTTP-01
//! answers, WebSockets and `X-Forwarded-*` headers.

use std::collections::HashMap;
use std::convert::Infallible;
use std::net::SocketAddr;
use std::pin::Pin;
use std::sync::{Arc, Mutex, PoisonError};
use std::task::{Context, Poll};

use bytes::Bytes;
use hearthgate_model::{Limits, host_matches};
use http::header::{self, HeaderMap, HeaderName, HeaderValue};
use http::{Request, Response, StatusCode, Uri};
use http_body_util::combinators::BoxBody;
use http_body_util::{BodyExt, Empty, Full, Limited};
use hyper::body::{Body, Frame, Incoming, SizeHint};
use hyper_util::client::legacy::Client;
use hyper_util::client::legacy::connect::HttpConnector;
use hyper_util::rt::{TokioExecutor, TokioIo, TokioTimer};
use tokio::io::{AsyncRead, AsyncWrite};
use tokio_util::sync::CancellationToken;
use tracing::debug;

use crate::l4::client_ip;
use crate::pool::{ActiveGuard, Endpoint, Pool, UpstreamState};

/// Body type used throughout the proxy.
pub type ProxyBody = BoxBody<Bytes, BoxError>;
/// Error type of proxied bodies.
pub type BoxError = Box<dyn std::error::Error + Send + Sync>;

/// Path prefix of ACME HTTP-01 challenges.
pub const ACME_CHALLENGE_PREFIX: &str = "/.well-known/acme-challenge/";

/// One HTTP route on a socket.
#[derive(Debug)]
pub struct HttpRoute {
    pub name: String,
    pub path_prefix: String,
    pub strip_prefix: bool,
    pub pool: Arc<Pool>,
}

/// Routes for one host pattern, longest path prefix first.
#[derive(Debug)]
pub struct HostRoutes {
    /// Exact host, `*.suffix` wildcard, or `*` for any host.
    pub pattern: String,
    pub routes: Vec<HttpRoute>,
}

/// Everything one HTTP or HTTPS socket routes.
#[derive(Debug, Default)]
pub struct HttpTable {
    /// Exact hosts first, then wildcards, then `*`.
    pub hosts: Vec<HostRoutes>,
    /// Hosts to redirect to HTTPS when no route matches, with the HTTPS port.
    pub redirects: Vec<(String, u16)>,
}

fn pattern_rank(pattern: &str) -> u8 {
    if pattern == "*" {
        2
    } else if pattern.starts_with("*.") {
        1
    } else {
        0
    }
}

fn pattern_matches(pattern: &str, host: &str) -> bool {
    pattern == "*" || host_matches(pattern, host)
}

/// Whether `path` is under `prefix`: `/api` matches `/api`, `/api/` and
/// `/api/x`, but not `/apix`.
#[must_use]
pub fn path_matches(prefix: &str, path: &str) -> bool {
    if prefix == "/" {
        return true;
    }
    match path.strip_prefix(prefix) {
        Some(rest) => prefix.ends_with('/') || rest.is_empty() || rest.starts_with('/'),
        None => false,
    }
}

impl HttpTable {
    /// Adds a route for `host`.
    pub fn add(&mut self, host: &str, route: HttpRoute) {
        if let Some(entry) = self.hosts.iter_mut().find(|h| h.pattern == host) {
            entry.routes.push(route);
        } else {
            self.hosts.push(HostRoutes {
                pattern: host.to_owned(),
                routes: vec![route],
            });
        }
    }

    /// Sorts hosts and routes so that the most specific match wins.
    pub fn finish(&mut self) {
        self.hosts.sort_by(|a, b| {
            (pattern_rank(&a.pattern), &a.pattern).cmp(&(pattern_rank(&b.pattern), &b.pattern))
        });
        for host in &mut self.hosts {
            host.routes
                .sort_by_key(|r| std::cmp::Reverse(r.path_prefix.len()));
        }
    }

    /// The route for a request.
    #[must_use]
    pub fn find(&self, host: &str, path: &str) -> Option<&HttpRoute> {
        self.hosts
            .iter()
            .filter(|h| pattern_matches(&h.pattern, host))
            .find_map(|h| h.routes.iter().find(|r| path_matches(&r.path_prefix, path)))
    }

    /// The HTTPS port to redirect `host` to, if it has one.
    #[must_use]
    pub fn redirect_port(&self, host: &str) -> Option<u16> {
        self.redirects
            .iter()
            .find(|(p, _)| pattern_matches(p, host))
            .map(|(_, port)| *port)
    }

    /// Whether this table has anything for `host`.
    #[must_use]
    pub fn serves(&self, host: &str) -> bool {
        self.hosts.iter().any(|h| pattern_matches(&h.pattern, host))
            || self.redirect_port(host).is_some()
    }
}

/// Pending ACME HTTP-01 challenges: token to key authorization.
#[derive(Debug, Default)]
pub struct HttpChallenges(Mutex<HashMap<String, String>>);

impl HttpChallenges {
    pub fn set(&self, token: &str, key_authorization: &str) {
        self.0
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .insert(token.to_owned(), key_authorization.to_owned());
    }
    pub fn clear(&self, token: &str) {
        self.0
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .remove(token);
    }
    fn get(&self, token: &str) -> Option<String> {
        self.0
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .get(token)
            .cloned()
    }
}

/// Shared by every HTTP connection.
pub struct HttpContext {
    client: Client<HttpConnector, ProxyBody>,
    pub challenges: Arc<HttpChallenges>,
}

impl std::fmt::Debug for HttpContext {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("HttpContext").finish_non_exhaustive()
    }
}

impl HttpContext {
    #[must_use]
    pub fn new(challenges: Arc<HttpChallenges>) -> Self {
        let mut connector = HttpConnector::new();
        connector.set_nodelay(true);
        connector.set_connect_timeout(Some(std::time::Duration::from_secs(5)));
        let client = Client::builder(TokioExecutor::new())
            .pool_idle_timeout(std::time::Duration::from_secs(60))
            .pool_timer(TokioTimer::new())
            .build(connector);
        Self { client, challenges }
    }
}

/// One accepted connection, as seen by the request handler.
#[derive(Clone)]
pub struct ConnInfo {
    pub peer: SocketAddr,
    pub tls: bool,
    /// Local port the client connected to.
    pub port: u16,
}

/// Serves HTTP/1.1 and HTTP/2 on `io` until the client is done or `cancel`
/// fires (then in-flight requests finish and the connection closes).
pub async fn serve<I>(
    io: I,
    info: ConnInfo,
    table: Arc<HttpTable>,
    limits: Arc<Limits>,
    ctx: Arc<HttpContext>,
    cancel: CancellationToken,
) where
    I: AsyncRead + AsyncWrite + Unpin + Send + 'static,
{
    let header_timeout = limits.header_timeout;
    let service = hyper::service::service_fn(move |req| {
        let (info, table, limits, ctx) = (
            info.clone(),
            Arc::clone(&table),
            Arc::clone(&limits),
            Arc::clone(&ctx),
        );
        async move { Ok::<_, Infallible>(handle(req, &info, &table, &limits, &ctx).await) }
    });
    let mut builder = hyper_util::server::conn::auto::Builder::new(TokioExecutor::new());
    builder
        .http1()
        .timer(TokioTimer::new())
        .header_read_timeout(header_timeout);
    builder.http2().timer(TokioTimer::new());
    let conn = builder.serve_connection_with_upgrades(TokioIo::new(io), service);
    tokio::pin!(conn);
    tokio::select! {
        r = conn.as_mut() => {
            if let Err(e) = r {
                debug!("http connection ended: {e}");
            }
        }
        () = cancel.cancelled() => {
            conn.as_mut().graceful_shutdown();
            let _ = conn.await;
        }
    }
}

fn text(status: StatusCode, body: &'static str) -> Response<ProxyBody> {
    let mut resp = Response::new(
        Full::new(Bytes::from_static(body.as_bytes()))
            .map_err(|never| match never {})
            .boxed(),
    );
    *resp.status_mut() = status;
    resp.headers_mut().insert(
        header::CONTENT_TYPE,
        HeaderValue::from_static("text/plain; charset=utf-8"),
    );
    resp
}

/// Host of a request without the port, lowercased.
fn request_host(req: &Request<Incoming>) -> Option<String> {
    let raw = req.uri().host().map(str::to_owned).or_else(|| {
        req.headers()
            .get(header::HOST)
            .and_then(|h| h.to_str().ok())
            .map(str::to_owned)
    })?;
    let host = if raw.starts_with('[') {
        // IPv6 literal: keep the brackets, drop a port.
        raw.split_once(']')
            .map_or(raw.clone(), |(h, _)| format!("{h}]"))
    } else {
        raw.split(':').next().unwrap_or_default().to_owned()
    };
    Some(host.trim_end_matches('.').to_ascii_lowercase())
}

const HOP_BY_HOP: &[HeaderName] = &[
    header::CONNECTION,
    header::PROXY_AUTHENTICATE,
    header::PROXY_AUTHORIZATION,
    header::TE,
    header::TRAILER,
    header::TRANSFER_ENCODING,
    header::UPGRADE,
];

fn remove_hop_by_hop(headers: &mut HeaderMap) {
    // Headers named in Connection are hop-by-hop too.
    let named: Vec<HeaderName> = headers
        .get_all(header::CONNECTION)
        .iter()
        .filter_map(|v| v.to_str().ok())
        .flat_map(|v| v.split(','))
        .filter_map(|n| HeaderName::from_bytes(n.trim().as_bytes()).ok())
        .collect();
    for name in named.iter().chain(HOP_BY_HOP) {
        headers.remove(name);
    }
    headers.remove("keep-alive");
    headers.remove("proxy-connection");
}

fn is_upgrade(headers: &HeaderMap) -> Option<HeaderValue> {
    let connection_upgrade = headers
        .get_all(header::CONNECTION)
        .iter()
        .filter_map(|v| v.to_str().ok())
        .flat_map(|v| v.split(','))
        .any(|t| t.trim().eq_ignore_ascii_case("upgrade"));
    if connection_upgrade {
        headers.get(header::UPGRADE).cloned()
    } else {
        None
    }
}

async fn handle(
    mut req: Request<Incoming>,
    info: &ConnInfo,
    table: &HttpTable,
    limits: &Limits,
    ctx: &HttpContext,
) -> Response<ProxyBody> {
    let Some(host) = request_host(&req) else {
        return text(StatusCode::BAD_REQUEST, "missing Host header\n");
    };
    let path = req.uri().path().to_owned();

    if !info.tls {
        if let Some(token) = path.strip_prefix(ACME_CHALLENGE_PREFIX) {
            if let Some(answer) = ctx.challenges.get(token) {
                let mut resp = Response::new(
                    Full::new(Bytes::from(answer))
                        .map_err(|n| match n {})
                        .boxed(),
                );
                resp.headers_mut().insert(
                    header::CONTENT_TYPE,
                    HeaderValue::from_static("application/octet-stream"),
                );
                return resp;
            }
        }
    }

    let Some(route) = table.find(&host, &path) else {
        if !info.tls {
            if let Some(port) = table.redirect_port(&host) {
                return redirect(&host, port, req.uri());
            }
        }
        return text(StatusCode::NOT_FOUND, "no route for this address\n");
    };

    let declared_len = req
        .headers()
        .get(header::CONTENT_LENGTH)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.parse::<u64>().ok());
    if declared_len.is_some_and(|n| n > limits.max_body_bytes) {
        return text(StatusCode::PAYLOAD_TOO_LARGE, "request body too large\n");
    }

    let upgrade = is_upgrade(req.headers());
    let client_upgrade = upgrade.as_ref().map(|_| hyper::upgrade::on(&mut req));
    let (parts, body) = req.into_parts();
    let body_empty = body.is_end_stream() || declared_len == Some(0);
    let max_body = usize::try_from(limits.max_body_bytes).unwrap_or(usize::MAX);
    let mut body = Some(Limited::new(body, max_body).boxed());

    // Path sent upstream.
    let mut upstream_path = parts
        .uri
        .path_and_query()
        .map_or_else(|| "/".to_owned(), ToString::to_string);
    if route.strip_prefix {
        let stripped = upstream_path
            .strip_prefix(route.path_prefix.trim_end_matches('/'))
            .unwrap_or(&upstream_path);
        upstream_path = if stripped.is_empty() || stripped.starts_with('?') {
            format!("/{stripped}")
        } else {
            stripped.to_owned()
        };
    }

    let mut headers = parts.headers.clone();
    remove_hop_by_hop(&mut headers);
    if let Some(proto) = &upgrade {
        headers.insert(header::CONNECTION, HeaderValue::from_static("upgrade"));
        headers.insert(header::UPGRADE, proto.clone());
    }
    if !headers.contains_key(header::HOST) {
        if let Ok(v) = HeaderValue::from_str(&host) {
            headers.insert(header::HOST, v);
        }
    }
    let client = client_ip(info.peer).to_string();
    // Never trust forwarding headers from the internet: replace them.
    for name in [
        "x-forwarded-for",
        "x-forwarded-proto",
        "x-forwarded-host",
        "x-forwarded-port",
        "x-real-ip",
        "forwarded",
    ] {
        headers.remove(name);
    }
    let set = |headers: &mut HeaderMap, name: &'static str, value: &str| {
        if let Ok(v) = HeaderValue::from_str(value) {
            headers.insert(name, v);
        }
    };
    set(&mut headers, "x-forwarded-for", &client);
    set(&mut headers, "x-real-ip", &client);
    set(
        &mut headers,
        "x-forwarded-proto",
        if info.tls { "https" } else { "http" },
    );
    set(&mut headers, "x-forwarded-host", &host);
    set(&mut headers, "x-forwarded-port", &info.port.to_string());

    let mut tried = Vec::new();
    let pool = &route.pool;
    while let Some(i) = pool.pick(client_ip(info.peer), &tried) {
        tried.push(i);
        let state = &pool.members[i].state;
        let authority = match state.endpoint() {
            Endpoint::Socket(addr) => addr.to_string(),
            Endpoint::Host(h, p) => format!("{h}:{p}"),
            Endpoint::Unresolved => continue,
        };
        let Ok(uri) = Uri::try_from(format!("http://{authority}{upstream_path}")) else {
            return text(StatusCode::BAD_REQUEST, "invalid request path\n");
        };
        let this_body = if body_empty {
            Empty::new().map_err(|n| match n {}).boxed()
        } else {
            match body.take() {
                Some(b) => b,
                None => break,
            }
        };
        let mut upstream_req = Request::new(this_body);
        *upstream_req.method_mut() = parts.method.clone();
        *upstream_req.uri_mut() = uri;
        *upstream_req.headers_mut() = headers.clone();
        let guard = state.start();
        let sent =
            tokio::time::timeout(limits.upstream_timeout, ctx.client.request(upstream_req)).await;
        match sent {
            Ok(Ok(mut resp)) => {
                state.record_success();
                if resp.status() == StatusCode::SWITCHING_PROTOCOLS {
                    if let Some(client_upgrade) = client_upgrade {
                        let upstream_upgrade = hyper::upgrade::on(&mut resp);
                        tokio::spawn(splice_upgrade(
                            client_upgrade,
                            upstream_upgrade,
                            guard,
                            limits.tcp_idle_timeout,
                        ));
                        let (mut parts, _) = resp.into_parts();
                        let proto = parts.headers.get(header::UPGRADE).cloned();
                        remove_hop_by_hop(&mut parts.headers);
                        parts
                            .headers
                            .insert(header::CONNECTION, HeaderValue::from_static("upgrade"));
                        if let Some(p) = proto {
                            parts.headers.insert(header::UPGRADE, p);
                        }
                        return Response::from_parts(
                            parts,
                            Empty::new().map_err(|n| match n {}).boxed(),
                        );
                    }
                }
                let (mut parts, body) = resp.into_parts();
                remove_hop_by_hop(&mut parts.headers);
                let body = Guarded {
                    inner: body.map_err(BoxError::from).boxed(),
                    _guard: guard,
                    state: Arc::clone(state),
                };
                return Response::from_parts(parts, body.boxed());
            }
            Ok(Err(e)) => {
                drop(guard);
                state.record_failure();
                debug!(route = %route.name, upstream = %authority, "upstream request failed: {e}");
                // Only retry when the request body was not consumed.
                if e.is_connect() && body_empty {
                    continue;
                }
                return text(StatusCode::BAD_GATEWAY, "upstream unavailable\n");
            }
            Err(_) => {
                drop(guard);
                state.record_failure();
                return text(StatusCode::GATEWAY_TIMEOUT, "upstream timed out\n");
            }
        }
    }
    text(StatusCode::BAD_GATEWAY, "no upstream available\n")
}

fn redirect(host: &str, port: u16, uri: &Uri) -> Response<ProxyBody> {
    let path = uri
        .path_and_query()
        .map_or("/", http::uri::PathAndQuery::as_str);
    let location = if port == 443 {
        format!("https://{host}{path}")
    } else {
        format!("https://{host}:{port}{path}")
    };
    let mut resp = text(StatusCode::PERMANENT_REDIRECT, "redirecting to HTTPS\n");
    if let Ok(v) = HeaderValue::from_str(&location) {
        resp.headers_mut().insert(header::LOCATION, v);
    }
    resp
}

async fn splice_upgrade(
    client: hyper::upgrade::OnUpgrade,
    upstream: hyper::upgrade::OnUpgrade,
    _guard: ActiveGuard,
    idle: std::time::Duration,
) {
    let (Ok(client), Ok(upstream)) = (client.await, upstream.await) else {
        return;
    };
    let never = CancellationToken::new();
    crate::l4::pipe(TokioIo::new(client), TokioIo::new(upstream), idle, &never).await;
}

/// A response body that keeps its upstream counted as busy until the body
/// is finished, and counts bytes.
struct Guarded {
    inner: ProxyBody,
    _guard: ActiveGuard,
    state: Arc<UpstreamState>,
}

impl Body for Guarded {
    type Data = Bytes;
    type Error = BoxError;

    fn poll_frame(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
    ) -> Poll<Option<Result<Frame<Bytes>, BoxError>>> {
        let poll = Pin::new(&mut self.inner).poll_frame(cx);
        if let Poll::Ready(Some(Ok(frame))) = &poll {
            if let Some(data) = frame.data_ref() {
                self.state
                    .bytes_received
                    .fetch_add(data.len() as u64, std::sync::atomic::Ordering::Relaxed);
            }
        }
        poll
    }

    fn is_end_stream(&self) -> bool {
        self.inner.is_end_stream()
    }

    fn size_hint(&self) -> SizeHint {
        self.inner.size_hint()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn path_prefix_matching() {
        assert!(path_matches("/", "/anything"));
        assert!(path_matches("/api", "/api"));
        assert!(path_matches("/api", "/api/v1"));
        assert!(!path_matches("/api", "/apiv1"));
        assert!(path_matches("/api/", "/api/v1"));
        assert!(!path_matches("/api/", "/api"));
    }
}
