//! Runs the sockets of a [`Plan`] and moves between plans without restarts.
//!
//! Each socket keeps its configuration in an [`ArcSwap`]. Applying a new
//! plan diffs the sockets:
//!
//! - unchanged address and kind: the configuration is swapped in place, so
//!   open connections are untouched and new ones use the new routes;
//! - new address: the socket is bound;
//! - removed address (or a changed kind): the socket stops accepting at once,
//!   open connections get `drain_timeout` to finish, then they are closed.

use std::collections::HashMap;
use std::io;
use std::net::{Ipv4Addr, SocketAddr};
use std::sync::Arc;

use arc_swap::ArcSwap;
use hearthgate_model::Limits;
use socket2::{Domain, Socket, Type};
use tokio::net::{TcpListener, TcpStream, UdpSocket};
use tokio::sync::Mutex;
use tokio::task::JoinHandle;
use tokio_util::sync::CancellationToken;
use tokio_util::task::TaskTracker;
use tracing::{debug, info, warn};

use crate::http::{self, ConnInfo, HttpChallenges, HttpContext};
use crate::l4;
use crate::plan::{Plan, SocketKey, SocketKind, SocketPlan, TlsPlan, Transport};
use crate::pool::Upstreams;
use crate::tls::{self, Certificates};

/// Configuration a running socket reads for every new connection.
#[derive(Debug)]
pub struct Shared {
    pub plan: SocketPlan,
    pub limits: Arc<Limits>,
}

struct Running {
    kind: SocketKind,
    shared: Arc<ArcSwap<Shared>>,
    accept: CancellationToken,
    connections: CancellationToken,
    tracker: TaskTracker,
    task: JoinHandle<()>,
    local: SocketAddr,
}

/// A socket that could not be started.
#[derive(Debug)]
pub struct BindError {
    pub key: SocketKey,
    pub error: io::Error,
}

impl std::fmt::Display for BindError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "cannot listen on {}: {}", self.key, self.error)
    }
}

/// State of one running socket, for status output.
#[derive(Debug, Clone)]
pub struct SocketStatus {
    pub key: SocketKey,
    /// The address actually bound (differs from the key for port 0 in tests
    /// or when `[::]` fell back to `0.0.0.0`).
    pub local: SocketAddr,
    pub kind: SocketKind,
    pub open_connections: usize,
}

/// Shared by every socket.
pub(crate) struct Ctx {
    pub(crate) http: Arc<HttpContext>,
    pub(crate) acceptor: tokio_rustls::TlsAcceptor,
}

/// The data plane.
pub struct Gateway {
    upstreams: Arc<Upstreams>,
    certs: Arc<Certificates>,
    challenges: Arc<HttpChallenges>,
    ctx: Arc<Ctx>,
    sockets: Mutex<HashMap<SocketKey, Running>>,
    drains: TaskTracker,
    /// Cut every drain short (on shutdown).
    closing: CancellationToken,
}

impl Default for Gateway {
    fn default() -> Self {
        Self::new()
    }
}

impl Gateway {
    #[must_use]
    pub fn new() -> Self {
        let certs = Arc::new(Certificates::default());
        let challenges = Arc::new(HttpChallenges::default());
        let ctx = Arc::new(Ctx {
            http: Arc::new(HttpContext::new(Arc::clone(&challenges))),
            acceptor: tokio_rustls::TlsAcceptor::from(tls::server_config(Arc::clone(&certs))),
        });
        Self {
            certs,
            challenges,
            ctx,
            upstreams: Arc::new(Upstreams::default()),
            sockets: Mutex::new(HashMap::new()),
            drains: TaskTracker::new(),
            closing: CancellationToken::new(),
        }
    }

    /// Certificates served by `https` listeners.
    #[must_use]
    pub fn certificates(&self) -> &Arc<Certificates> {
        &self.certs
    }

    /// Pending ACME HTTP-01 challenges answered on `http` listeners.
    #[must_use]
    pub fn http_challenges(&self) -> &Arc<HttpChallenges> {
        &self.challenges
    }

    /// The upstream registry plans must be compiled against.
    #[must_use]
    pub fn upstreams(&self) -> &Arc<Upstreams> {
        &self.upstreams
    }

    /// Moves the data plane to `plan`. Sockets that cannot be bound are
    /// reported and skipped; everything else is applied.
    pub async fn apply(&self, plan: &Plan) -> Vec<BindError> {
        let mut sockets = self.sockets.lock().await;

        // Stop removed sockets and those whose kind changes first, so their
        // ports are free before anything is bound.
        let stale: Vec<SocketKey> = sockets
            .iter()
            .filter(|(key, running)| {
                plan.sockets
                    .get(key)
                    .is_none_or(|p| p.kind() != running.kind)
            })
            .map(|(key, _)| *key)
            .collect();
        for key in stale {
            if let Some(running) = sockets.remove(&key) {
                info!(socket = %key, "closing listener");
                self.stop(running, plan.limits.drain_timeout).await;
            }
        }

        let mut errors = Vec::new();
        for (key, socket_plan) in &plan.sockets {
            let shared = Shared {
                plan: socket_plan.clone(),
                limits: Arc::clone(&plan.limits),
            };
            if let Some(running) = sockets.get(key) {
                running.shared.store(Arc::new(shared));
                continue;
            }
            match start(*key, shared, &self.ctx) {
                Ok(running) => {
                    info!(socket = %key, local = %running.local, "listening");
                    sockets.insert(*key, running);
                }
                Err(error) => {
                    warn!(socket = %key, "cannot listen: {error}");
                    errors.push(BindError { key: *key, error });
                }
            }
        }
        errors
    }

    async fn stop(&self, running: Running, drain: std::time::Duration) {
        running.accept.cancel();
        // Wait for the accept loop so the socket is closed before a new one
        // binds the same port.
        let _ = running.task.await;
        running.tracker.close();
        let tracker = running.tracker;
        let connections = running.connections;
        let closing = self.closing.clone();
        self.drains.spawn(async move {
            tokio::select! {
                () = tracker.wait() => return,
                () = tokio::time::sleep(drain) => {
                    debug!("drain timeout reached; closing remaining connections");
                }
                () = closing.cancelled() => {}
            }
            connections.cancel();
            tracker.wait().await;
        });
    }

    /// Stops every socket, giving open connections `drain` to finish.
    pub async fn shutdown(&self, drain: std::time::Duration) {
        let mut sockets = self.sockets.lock().await;
        for (_, running) in sockets.drain() {
            self.stop(running, drain).await;
        }
        drop(sockets);
        self.drains.close();
        if tokio::time::timeout(drain, self.drains.wait())
            .await
            .is_err()
        {
            self.closing.cancel();
            self.drains.wait().await;
        }
    }

    /// Running sockets.
    pub async fn status(&self) -> Vec<SocketStatus> {
        let sockets = self.sockets.lock().await;
        let mut out: Vec<SocketStatus> = sockets
            .iter()
            .map(|(key, r)| SocketStatus {
                key: *key,
                local: r.local,
                kind: r.kind,
                open_connections: r.tracker.len(),
            })
            .collect();
        out.sort_by_key(|s| s.key);
        out
    }

    /// The address actually bound for `key`, if it is running.
    pub async fn local_addr(&self, key: &SocketKey) -> Option<SocketAddr> {
        self.sockets.lock().await.get(key).map(|r| r.local)
    }
}

fn start(key: SocketKey, shared: Shared, ctx: &Arc<Ctx>) -> io::Result<Running> {
    let kind = shared.plan.kind();
    let shared = Arc::new(ArcSwap::from_pointee(shared));
    let accept = CancellationToken::new();
    let connections = CancellationToken::new();
    let tracker = TaskTracker::new();
    let (task, local) = match key.transport {
        Transport::Tcp => {
            let listener = bind_tcp(key.addr)?;
            let local = listener.local_addr()?;
            let task = tokio::spawn(accept_loop(
                listener,
                local,
                Arc::clone(ctx),
                Arc::clone(&shared),
                accept.clone(),
                connections.clone(),
                tracker.clone(),
            ));
            (task, local)
        }
        Transport::Udp => {
            let socket = Arc::new(bind_udp(key.addr)?);
            let local = socket.local_addr()?;
            let shared = Arc::clone(&shared);
            let current = move || {
                let s = shared.load();
                match &s.plan {
                    SocketPlan::Udp(pool) => Some((Arc::clone(pool), Arc::clone(&s.limits))),
                    _ => None,
                }
            };
            let task = tokio::spawn(l4::run_udp(socket, current, accept.clone()));
            (task, local)
        }
    };
    Ok(Running {
        kind,
        shared,
        accept,
        connections,
        tracker,
        task,
        local,
    })
}

async fn accept_loop(
    listener: TcpListener,
    local: SocketAddr,
    ctx: Arc<Ctx>,
    shared: Arc<ArcSwap<Shared>>,
    accept: CancellationToken,
    connections: CancellationToken,
    tracker: TaskTracker,
) {
    loop {
        let (stream, peer) = tokio::select! {
            () = accept.cancelled() => break,
            r = listener.accept() => match r {
                Ok(v) => v,
                Err(e) => {
                    // Usually out of file descriptors; back off briefly.
                    warn!("accept failed: {e}");
                    tokio::time::sleep(std::time::Duration::from_millis(100)).await;
                    continue;
                }
            },
        };
        let current = shared.load_full();
        tracker.spawn(handle(
            stream,
            peer,
            local.port(),
            current,
            Arc::clone(&ctx),
            connections.child_token(),
        ));
    }
}

async fn handle(
    stream: TcpStream,
    peer: SocketAddr,
    port: u16,
    shared: Arc<Shared>,
    ctx: Arc<Ctx>,
    cancel: CancellationToken,
) {
    let limits = Arc::clone(&shared.limits);
    match &shared.plan {
        SocketPlan::RawTcp(pool) => {
            l4::handle_tcp(stream, peer, Arc::clone(pool), limits, cancel).await;
        }
        SocketPlan::Udp(_) => {}
        SocketPlan::Http(table) => {
            let _ = stream.set_nodelay(true);
            let info = ConnInfo {
                peer,
                tls: false,
                port,
            };
            http::serve(
                stream,
                info,
                Arc::clone(table),
                limits,
                Arc::clone(&ctx.http),
                cancel,
            )
            .await;
        }
        SocketPlan::Tls(plan) => {
            handle_tls(stream, peer, port, Arc::clone(plan), limits, ctx, cancel).await;
        }
    }
}

async fn handle_tls(
    mut stream: TcpStream,
    peer: SocketAddr,
    port: u16,
    plan: Arc<TlsPlan>,
    limits: Arc<Limits>,
    ctx: Arc<Ctx>,
    cancel: CancellationToken,
) {
    let _ = stream.set_nodelay(true);
    let hello =
        tokio::time::timeout(limits.header_timeout, tls::read_client_hello(&mut stream)).await;
    let (prefix, hello) = match hello {
        Ok(Ok((prefix, Some(hello)))) => (prefix, hello),
        Ok(Ok((_, None))) => {
            debug!(client = %peer, "not a TLS client hello");
            return;
        }
        Ok(Err(e)) => {
            debug!(client = %peer, "reading client hello failed: {e}");
            return;
        }
        Err(_) => return,
    };
    let sni = hello.sni.as_deref();
    let acme = hello.alpn.iter().any(|p| p == tls::ACME_TLS_ALPN);
    if !acme {
        if let Some(pool) = plan.passthrough_for(sni) {
            let client = tls::Prefixed::new(prefix, stream);
            passthrough(client, peer, pool, &limits, &cancel).await;
            return;
        }
    }
    if !acme && !plan.terminates(sni) {
        debug!(client = %peer, sni = ?sni, "no interface serves this name");
        return;
    }
    let client = tls::Prefixed::new(prefix, stream);
    let accepted = tokio::time::timeout(limits.header_timeout, ctx.acceptor.accept(client)).await;
    let tls_stream = match accepted {
        Ok(Ok(s)) => s,
        Ok(Err(e)) => {
            debug!(client = %peer, sni = ?sni, "TLS handshake failed: {e}");
            return;
        }
        Err(_) => return,
    };
    if tls_stream.get_ref().1.alpn_protocol() == Some(tls::ACME_TLS_ALPN) {
        // TLS-ALPN-01 validation only needs the handshake.
        return;
    }
    let info = ConnInfo {
        peer,
        tls: true,
        port,
    };
    let table = Arc::clone(&plan.https);
    http::serve(
        tls_stream,
        info,
        table,
        limits,
        Arc::clone(&ctx.http),
        cancel,
    )
    .await;
}

async fn passthrough(
    client: tls::Prefixed<TcpStream>,
    peer: SocketAddr,
    pool: &Arc<crate::pool::Pool>,
    limits: &Limits,
    cancel: &CancellationToken,
) {
    let Some((upstream, state, _guard)) =
        l4::connect(pool, l4::client_ip(peer), limits.connect_timeout).await
    else {
        warn!(route = %pool.route, client = %peer, "no upstream available");
        return;
    };
    let (sent, received) = l4::pipe(client, upstream, limits.tcp_idle_timeout, cancel).await;
    state
        .bytes_sent
        .fetch_add(sent, std::sync::atomic::Ordering::Relaxed);
    state
        .bytes_received
        .fetch_add(received, std::sync::atomic::Ordering::Relaxed);
}

/// Creates a socket for `addr`. `[::]` is bound dual-stack; when the host
/// has no IPv6 it falls back to `0.0.0.0`.
fn socket_for(addr: SocketAddr, ty: Type) -> io::Result<(Socket, SocketAddr)> {
    let try_bind = |addr: SocketAddr| -> io::Result<Socket> {
        let socket = Socket::new(Domain::for_address(addr), ty, None)?;
        if addr.is_ipv6() && addr.ip().is_unspecified() {
            socket.set_only_v6(false)?;
        }
        if ty == Type::STREAM {
            socket.set_reuse_address(true)?;
        }
        socket.set_nonblocking(true)?;
        socket.bind(&addr.into())?;
        Ok(socket)
    };
    match try_bind(addr) {
        Ok(s) => Ok((s, addr)),
        Err(e)
            if addr.is_ipv6()
                && addr.ip().is_unspecified()
                && e.raw_os_error() == Some(97 /* EAFNOSUPPORT */) =>
        {
            let v4 = SocketAddr::from((Ipv4Addr::UNSPECIFIED, addr.port()));
            Ok((try_bind(v4)?, v4))
        }
        Err(e) => Err(e),
    }
}

fn bind_tcp(addr: SocketAddr) -> io::Result<TcpListener> {
    let (socket, _) = socket_for(addr, Type::STREAM)?;
    socket.listen(1024)?;
    TcpListener::from_std(socket.into())
}

fn bind_udp(addr: SocketAddr) -> io::Result<UdpSocket> {
    let (socket, _) = socket_for(addr, Type::DGRAM)?;
    UdpSocket::from_std(socket.into())
}
