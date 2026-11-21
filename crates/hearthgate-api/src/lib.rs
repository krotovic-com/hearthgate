//! Hearthgate's admin API and dashboard.
//!
//! Served only on the admin listener, and every client address is checked
//! against the allowed ranges before a single byte is read, so a wrong bind
//! address cannot expose it. The listener speaks HTTPS (plain HTTP on the
//! same port gets a redirect). The OpenAPI document at
//! `/api/v1/openapi.json` is the contract the CLI and dashboard share.

pub mod access;
mod account;
mod auth;
pub mod docker;
pub mod firewall;
pub mod live;
pub mod notify;
mod ratelimit;
mod routes;
mod secret;
pub mod tls;
mod ui;
pub mod vpn;

use std::net::{IpAddr, SocketAddr};
use std::sync::{Arc, PoisonError, RwLock};
use std::time::Duration;

use axum::Router;
use hearthgate_acme::CertStore;
use hearthgate_proxy::Gateway;
use hearthgate_store::Store;
use hyper_util::rt::{TokioExecutor, TokioIo};
use hyper_util::service::TowerToHyperService;
use tokio::io::{AsyncRead, AsyncWrite};
use tokio::net::{TcpListener, TcpStream};
use tokio_util::sync::CancellationToken;
use tracing::{debug, info, warn};

pub use access::AccessControl;
pub use account::setup_code;
pub use auth::{AuthState, TOKEN_FILE, load_or_create_token};
pub use live::{Live, LogBuffer, LogLayer};
pub use routes::ApiDoc;
pub use tls::AdminTls;

/// TOTP helpers, for tests that log in with two-factor codes.
#[doc(hidden)]
pub mod testing {
    pub use crate::secret::{TOTP_STEP, totp_code, unbase32};
}

/// An allowed client range, such as `192.168.0.0/16`.
pub type Net = ipnet::IpNet;

/// How long a client gets to start talking (and finish the TLS handshake).
const HANDSHAKE_TIMEOUT: Duration = Duration::from_secs(10);

/// What the daemon is running right now, for `/api/v1/status`.
#[derive(Debug, Clone, Default)]
pub struct RuntimeInfo {
    /// Revision running (`None` before the first apply).
    pub revision: Option<i64>,
    /// Problems worth showing the user: sockets that did not bind,
    /// certificates missing or failing to renew.
    pub problems: Vec<String>,
}

/// Everything request handlers need.
#[derive(Clone)]
pub struct ApiState {
    pub store: Arc<Store>,
    pub gateway: Arc<Gateway>,
    pub certs: Arc<CertStore>,
    pub runtime: Arc<RwLock<RuntimeInfo>>,
    /// Woken after a change so the daemon applies it at once.
    pub changed: Arc<tokio::sync::Notify>,
    /// The admin token from the data directory.
    pub token: Arc<str>,
    /// Serve `/metrics` without logging in (for scrapers on the LAN).
    pub open_metrics: bool,
    /// Mark session cookies `Secure`. On whenever the listener uses HTTPS.
    pub secure_cookies: bool,
    /// Login rate limits and used TOTP codes.
    pub auth: Arc<AuthState>,
    /// The admin listeners and who may connect.
    pub access: Arc<AccessControl>,
    /// Traffic rates and the log tail.
    pub live: Arc<Live>,
    /// The firewall runtime.
    pub perimeter: Arc<hearthgate_firewall::Perimeter>,
    /// Router port forwarding state.
    pub router: Arc<hearthgate_upnp::Router>,
    pub vpn: Arc<hearthgate_vpn::Runtime>,
    /// Managed containers and stacks.
    pub docker: Arc<hearthgate_docker::Manager>,
}

impl ApiState {
    /// The router for these handlers, to pass to [`AccessControl::start`].
    pub fn router(&self) -> Router {
        routes::router(self.clone())
    }
}

/// Whether `ip` is inside one of `allow`.
#[must_use]
pub fn allowed(allow: &[Net], ip: IpAddr) -> bool {
    let ip = ip.to_canonical();
    allow.iter().any(|net| net.contains(&ip))
}

/// Serves the API on `addr` until `cancel` fires, with a fixed allowlist.
pub async fn serve(
    addr: SocketAddr,
    allow: Vec<Net>,
    tls: Option<Arc<AdminTls>>,
    state: ApiState,
    cancel: CancellationToken,
) -> std::io::Result<()> {
    let listener = TcpListener::bind(addr).await?;
    let scheme = if tls.is_some() { "https" } else { "http" };
    info!(listen = %listener.local_addr()?, "dashboard and admin API listening ({scheme})");
    serve_listener(listener, allow, tls, state, cancel).await;
    Ok(())
}

/// Serves the API on an already bound listener, with a fixed allowlist.
pub async fn serve_listener(
    listener: TcpListener,
    allow: Vec<Net>,
    tls: Option<Arc<AdminTls>>,
    state: ApiState,
    cancel: CancellationToken,
) {
    accept_loop(
        listener,
        Arc::new(RwLock::new(allow)),
        Arc::new(RwLock::new(tls)),
        state.router(),
        cancel.clone(),
        cancel,
    )
    .await;
}

/// Accepts connections until `listener_stop` fires. Open connections end
/// when `stop` fires, so closing a listener does not cut off the request
/// that closed it. The allowlist and certificate are read per connection,
/// so changes apply at once.
pub(crate) async fn accept_loop(
    listener: TcpListener,
    allow: Arc<RwLock<Vec<Net>>>,
    tls: Arc<RwLock<Option<Arc<AdminTls>>>>,
    app: Router,
    listener_stop: CancellationToken,
    stop: CancellationToken,
) {
    loop {
        let (stream, peer) = tokio::select! {
            () = listener_stop.cancelled() => return,
            r = listener.accept() => match r {
                Ok(v) => v,
                Err(e) => {
                    warn!("admin accept failed: {e}");
                    tokio::time::sleep(Duration::from_millis(100)).await;
                    continue;
                }
            },
        };
        let ok = allowed(
            &allow.read().unwrap_or_else(PoisonError::into_inner),
            peer.ip(),
        );
        if !ok {
            // Close without a response: nothing to probe from outside.
            debug!(client = %peer, "admin connection from a disallowed address dropped");
            drop(stream);
            continue;
        }
        // Lets a request end its own connection (see
        // `firewall::close_while_pending`).
        let close = CancellationToken::new();
        let app = app
            .clone()
            .layer(axum::Extension(auth::ClientAddr(peer)))
            .layer(axum::Extension(firewall::Accepted(
                std::time::Instant::now(),
            )))
            .layer(axum::Extension(firewall::CloseConnection(close.clone())));
        let tls = tls.read().unwrap_or_else(PoisonError::into_inner).clone();
        let stop = stop.clone();
        tokio::spawn(async move {
            if let Some(tls) = tls {
                serve_tls(stream, app, &tls, stop, close).await;
            } else {
                serve_connection(stream, app, stop, close).await;
            }
        });
    }
}

/// Answers TLS clients, and redirects plain HTTP clients on the same port.
async fn serve_tls(
    stream: TcpStream,
    app: Router,
    tls: &AdminTls,
    cancel: CancellationToken,
    close: CancellationToken,
) {
    let mut first = [0u8; 1];
    match tokio::time::timeout(HANDSHAKE_TIMEOUT, stream.peek(&mut first)).await {
        Ok(Ok(1)) => {}
        _ => return,
    }
    // 0x16 starts a TLS handshake record.
    if first[0] != 0x16 {
        serve_connection(
            stream,
            Router::new().fallback(redirect_to_https),
            cancel,
            close,
        )
        .await;
        return;
    }
    let acceptor = tokio_rustls::TlsAcceptor::from(Arc::clone(&tls.config));
    match tokio::time::timeout(HANDSHAKE_TIMEOUT, acceptor.accept(stream)).await {
        Ok(Ok(stream)) => serve_connection(stream, app, cancel, close).await,
        Ok(Err(e)) => debug!("admin TLS handshake failed: {e}"),
        Err(_) => debug!("admin TLS handshake timed out"),
    }
}

/// Serves one connection until it ends or `cancel` fires. `close` ends it
/// gracefully: requests in flight finish, and HTTP/2 clients get a GOAWAY,
/// so their next request opens a new connection.
async fn serve_connection<I>(
    io: I,
    app: Router,
    cancel: CancellationToken,
    close: CancellationToken,
) where
    I: AsyncRead + AsyncWrite + Unpin + Send + 'static,
{
    let service = TowerToHyperService::new(app);
    let mut builder = hyper_util::server::conn::auto::Builder::new(TokioExecutor::new());
    builder
        .http1()
        .timer(hyper_util::rt::TokioTimer::new())
        .header_read_timeout(HANDSHAKE_TIMEOUT);
    let conn = builder.serve_connection_with_upgrades(TokioIo::new(io), service);
    tokio::pin!(conn);
    tokio::select! {
        _ = conn.as_mut() => return,
        () = cancel.cancelled() => {
            conn.as_mut().graceful_shutdown();
            return;
        }
        () = close.cancelled() => conn.as_mut().graceful_shutdown(),
    }
    tokio::select! {
        _ = conn.as_mut() => {}
        () = cancel.cancelled() => {}
    }
}

async fn redirect_to_https(
    headers: axum::http::HeaderMap,
    uri: axum::http::Uri,
) -> axum::response::Response {
    use axum::http::{StatusCode, header};
    use axum::response::IntoResponse;
    let host = headers
        .get(header::HOST)
        .and_then(|h| h.to_str().ok())
        .filter(|h| !h.is_empty() && h.bytes().all(|b| b.is_ascii_graphic()));
    let Some(host) = host else {
        return (StatusCode::BAD_REQUEST, "this port speaks HTTPS\n").into_response();
    };
    let path = uri
        .path_and_query()
        .map_or("/", axum::http::uri::PathAndQuery::as_str);
    (
        StatusCode::PERMANENT_REDIRECT,
        [(header::LOCATION, format!("https://{host}{path}"))],
        "this port speaks HTTPS\n",
    )
        .into_response()
}
