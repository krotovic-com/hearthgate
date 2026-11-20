//! Hearthgate's admin REST API.
//!
//! Served only on the admin listener (loopback or LAN by default), and every
//! client address is checked against the allowed ranges before a single byte
//! is read, so a wrong bind address cannot expose it. Requests need the
//! admin token as `Authorization: Bearer <token>`. The OpenAPI document at
//! `/api/v1/openapi.json` is the contract the CLI and dashboard share.

mod auth;
mod routes;

use std::net::{IpAddr, SocketAddr};
use std::sync::{Arc, RwLock};

use hearthgate_acme::CertStore;
use hearthgate_proxy::Gateway;
use hearthgate_store::Store;
use hyper_util::rt::{TokioExecutor, TokioIo};
use hyper_util::service::TowerToHyperService;
use tokio::net::TcpListener;
use tokio_util::sync::CancellationToken;
use tracing::{debug, info, warn};

pub use auth::{TOKEN_FILE, load_or_create_token};
pub use routes::ApiDoc;

/// An allowed client range, such as `192.168.0.0/16`.
pub type Net = ipnet::IpNet;

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
    pub token: Arc<str>,
    /// Serve `/metrics` without the token (for scrapers on the LAN).
    pub open_metrics: bool,
}

/// Whether `ip` is inside one of `allow`.
#[must_use]
pub fn allowed(allow: &[Net], ip: IpAddr) -> bool {
    let ip = ip.to_canonical();
    allow.iter().any(|net| net.contains(&ip))
}

/// Serves the API on `addr` until `cancel` fires.
pub async fn serve(
    addr: SocketAddr,
    allow: Vec<Net>,
    state: ApiState,
    cancel: CancellationToken,
) -> std::io::Result<()> {
    let listener = TcpListener::bind(addr).await?;
    info!(listen = %listener.local_addr()?, "admin API listening");
    serve_listener(listener, allow, state, cancel).await;
    Ok(())
}

/// Serves the API on an already bound listener.
pub async fn serve_listener(
    listener: TcpListener,
    allow: Vec<Net>,
    state: ApiState,
    cancel: CancellationToken,
) {
    let app = routes::router(state);
    loop {
        let (stream, peer) = tokio::select! {
            () = cancel.cancelled() => return,
            r = listener.accept() => match r {
                Ok(v) => v,
                Err(e) => {
                    warn!("admin accept failed: {e}");
                    tokio::time::sleep(std::time::Duration::from_millis(100)).await;
                    continue;
                }
            },
        };
        if !allowed(&allow, peer.ip()) {
            // Close without a response: nothing to probe from outside.
            debug!(client = %peer, "admin connection from a disallowed address dropped");
            drop(stream);
            continue;
        }
        let service = TowerToHyperService::new(app.clone());
        let cancel = cancel.clone();
        tokio::spawn(async move {
            let builder = hyper_util::server::conn::auto::Builder::new(TokioExecutor::new());
            let conn = builder.serve_connection(TokioIo::new(stream), service);
            tokio::pin!(conn);
            tokio::select! {
                _ = conn.as_mut() => {}
                () = cancel.cancelled() => conn.as_mut().graceful_shutdown(),
            }
        });
    }
}
