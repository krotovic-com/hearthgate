//! Layer 4 forwarding: raw TCP connections and UDP datagrams.

use std::collections::HashMap;
use std::io;
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

use hearthgate_model::Limits;
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};
use tokio::net::{TcpStream, UdpSocket};
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;
use tracing::{debug, warn};

use crate::pool::{ActiveGuard, Pool, UpstreamState};

/// Most UDP client sessions one listener tracks at once.
pub const MAX_UDP_SESSIONS: usize = 4096;

/// The client address with IPv4-mapped IPv6 addresses turned back into
/// IPv4, so `::ffff:192.0.2.1` and `192.0.2.1` are the same client.
#[must_use]
pub fn client_ip(addr: SocketAddr) -> IpAddr {
    addr.ip().to_canonical()
}

/// Connects to a member of `pool`, trying the others when one fails.
/// Returns the stream and a guard that counts the connection as active.
pub async fn connect(
    pool: &Pool,
    client: IpAddr,
    timeout: Duration,
) -> Option<(TcpStream, Arc<UpstreamState>, ActiveGuard)> {
    let mut tried = Vec::new();
    while let Some(i) = pool.pick(client, &tried) {
        tried.push(i);
        let state = &pool.members[i].state;
        match connect_one(state, timeout).await {
            Ok(stream) => {
                state.record_success();
                let guard = state.start();
                return Some((stream, Arc::clone(state), guard));
            }
            Err(e) => {
                state.record_failure();
                debug!(
                    route = %pool.route,
                    upstream = %state.endpoint(),
                    "upstream connect failed: {e}"
                );
            }
        }
    }
    None
}

async fn connect_one(state: &UpstreamState, timeout: Duration) -> io::Result<TcpStream> {
    let addrs = state.endpoint().resolve().await?;
    let mut last = io::Error::new(io::ErrorKind::NotFound, "no addresses");
    for addr in addrs {
        match tokio::time::timeout(timeout, TcpStream::connect(addr)).await {
            Ok(Ok(stream)) => {
                let _ = stream.set_nodelay(true);
                return Ok(stream);
            }
            Ok(Err(e)) => last = e,
            Err(_) => last = io::Error::new(io::ErrorKind::TimedOut, "connect timed out"),
        }
    }
    Err(last)
}

/// Handles one raw TCP connection.
pub async fn handle_tcp(
    client: TcpStream,
    peer: SocketAddr,
    pool: Arc<Pool>,
    limits: Arc<Limits>,
    cancel: CancellationToken,
) {
    let _ = client.set_nodelay(true);
    let Some((upstream, state, _guard)) =
        connect(&pool, client_ip(peer), limits.connect_timeout).await
    else {
        warn!(route = %pool.route, client = %peer, "no upstream available");
        return;
    };
    let (sent, received) = pipe(client, upstream, limits.tcp_idle_timeout, &cancel).await;
    state.bytes_sent.fetch_add(sent, Ordering::Relaxed);
    state.bytes_received.fetch_add(received, Ordering::Relaxed);
}

/// Copies bytes both ways until both sides close, the connection is idle
/// for `idle`, or `cancel` fires. Returns bytes sent to and received from
/// the upstream.
pub async fn pipe<A, B>(a: A, b: B, idle: Duration, cancel: &CancellationToken) -> (u64, u64)
where
    A: AsyncRead + AsyncWrite + Unpin,
    B: AsyncRead + AsyncWrite + Unpin,
{
    let start = Instant::now();
    let last = AtomicU64::new(0);
    let touch = || {
        last.store(
            u64::try_from(start.elapsed().as_millis()).unwrap_or(u64::MAX),
            Ordering::Relaxed,
        );
    };
    let (mut ar, mut aw) = tokio::io::split(a);
    let (mut br, mut bw) = tokio::io::split(b);
    let up = copy_half(&mut ar, &mut bw, &touch);
    let down = copy_half(&mut br, &mut aw, &touch);
    let watchdog = async {
        let idle_ms = u64::try_from(idle.as_millis()).unwrap_or(u64::MAX);
        loop {
            let since = u64::try_from(start.elapsed().as_millis()).unwrap_or(u64::MAX)
                - last.load(Ordering::Relaxed);
            if since >= idle_ms {
                return;
            }
            tokio::time::sleep(Duration::from_millis(idle_ms - since)).await;
        }
    };
    tokio::pin!(up, down, watchdog);
    let (mut sent, mut received) = (None, None);
    loop {
        tokio::select! {
            n = &mut up, if sent.is_none() => sent = Some(n),
            n = &mut down, if received.is_none() => received = Some(n),
            () = &mut watchdog => break,
            () = cancel.cancelled() => break,
        }
        if sent.is_some() && received.is_some() {
            break;
        }
    }
    (sent.unwrap_or(0), received.unwrap_or(0))
}

async fn copy_half<R, W>(r: &mut R, w: &mut W, touch: &impl Fn()) -> u64
where
    R: AsyncRead + Unpin,
    W: AsyncWrite + Unpin,
{
    let mut buf = vec![0u8; 16 * 1024];
    let mut total = 0u64;
    loop {
        let n = match r.read(&mut buf).await {
            Ok(0) | Err(_) => break,
            Ok(n) => n,
        };
        touch();
        if w.write_all(&buf[..n]).await.is_err() {
            break;
        }
        total += n as u64;
    }
    // Pass the half-close on, so protocols that rely on it keep working.
    let _ = w.shutdown().await;
    total
}

struct Session {
    upstream: Arc<UdpSocket>,
    last: Arc<AtomicU64>,
    state: Arc<UpstreamState>,
}

/// Runs a UDP listener until `cancel` fires. `current` returns the pool and
/// limits to use for new sessions, so a reload applies without dropping
/// existing sessions.
pub async fn run_udp(
    socket: Arc<UdpSocket>,
    current: impl Fn() -> (Arc<Pool>, Arc<Limits>),
    cancel: CancellationToken,
) {
    let mut sessions: HashMap<SocketAddr, Session> = HashMap::new();
    let (closed_tx, mut closed_rx) = mpsc::unbounded_channel::<SocketAddr>();
    let mut buf = vec![0u8; 65_535];
    let epoch = Instant::now();
    let mut warned_full = false;
    loop {
        let (n, peer) = tokio::select! {
            () = cancel.cancelled() => break,
            Some(addr) = closed_rx.recv() => {
                sessions.remove(&addr);
                continue;
            }
            r = socket.recv_from(&mut buf) => match r {
                Ok(v) => v,
                Err(e) => {
                    // ICMP errors from earlier sends surface here on Linux.
                    debug!("udp receive error: {e}");
                    continue;
                }
            },
        };
        let now = millis(epoch);
        if let Some(session) = sessions.get(&peer) {
            session.last.store(now, Ordering::Relaxed);
            if session.upstream.send(&buf[..n]).await.is_ok() {
                session
                    .state
                    .bytes_sent
                    .fetch_add(n as u64, Ordering::Relaxed);
            }
            continue;
        }
        if sessions.len() >= MAX_UDP_SESSIONS {
            if !warned_full {
                warn!(
                    local = ?socket.local_addr().ok(),
                    "too many UDP sessions ({MAX_UDP_SESSIONS}); dropping new clients"
                );
                warned_full = true;
            }
            continue;
        }
        warned_full = false;
        let (pool, limits) = current();
        let Some((upstream, state)) = open_session(&pool, peer).await else {
            debug!(route = %pool.route, client = %peer, "no UDP upstream available");
            continue;
        };
        if upstream.send(&buf[..n]).await.is_ok() {
            state.bytes_sent.fetch_add(n as u64, Ordering::Relaxed);
        }
        let last = Arc::new(AtomicU64::new(now));
        sessions.insert(
            peer,
            Session {
                upstream: Arc::clone(&upstream),
                last: Arc::clone(&last),
                state: Arc::clone(&state),
            },
        );
        tokio::spawn(reply_loop(
            Arc::clone(&socket),
            upstream,
            peer,
            state,
            last,
            epoch,
            limits.udp_session_timeout,
            closed_tx.clone(),
            cancel.clone(),
        ));
    }
}

fn millis(epoch: Instant) -> u64 {
    u64::try_from(epoch.elapsed().as_millis()).unwrap_or(u64::MAX)
}

async fn open_session(
    pool: &Pool,
    peer: SocketAddr,
) -> Option<(Arc<UdpSocket>, Arc<UpstreamState>)> {
    let mut tried = Vec::new();
    while let Some(i) = pool.pick(client_ip(peer), &tried) {
        tried.push(i);
        let state = &pool.members[i].state;
        let Ok(addrs) = state.endpoint().resolve().await else {
            state.record_failure();
            continue;
        };
        for addr in addrs {
            let local: SocketAddr = if addr.is_ipv4() {
                (Ipv4Addr::UNSPECIFIED, 0).into()
            } else {
                (Ipv6Addr::UNSPECIFIED, 0).into()
            };
            let Ok(sock) = UdpSocket::bind(local).await else {
                continue;
            };
            if sock.connect(addr).await.is_ok() {
                state.connections_total.fetch_add(1, Ordering::Relaxed);
                return Some((Arc::new(sock), Arc::clone(state)));
            }
        }
        state.record_failure();
    }
    None
}

#[allow(clippy::too_many_arguments)]
async fn reply_loop(
    listener: Arc<UdpSocket>,
    upstream: Arc<UdpSocket>,
    peer: SocketAddr,
    state: Arc<UpstreamState>,
    last: Arc<AtomicU64>,
    epoch: Instant,
    idle: Duration,
    closed: mpsc::UnboundedSender<SocketAddr>,
    cancel: CancellationToken,
) {
    let _guard = state.start();
    // `start` counts a connection; the session was counted when opened.
    state.connections_total.fetch_sub(1, Ordering::Relaxed);
    let idle_ms = u64::try_from(idle.as_millis()).unwrap_or(u64::MAX);
    let mut buf = vec![0u8; 65_535];
    loop {
        let since = millis(epoch).saturating_sub(last.load(Ordering::Relaxed));
        if since >= idle_ms {
            break;
        }
        tokio::select! {
            () = cancel.cancelled() => break,
            () = tokio::time::sleep(Duration::from_millis(idle_ms - since)) => {}
            r = upstream.recv(&mut buf) => match r {
                Ok(n) => {
                    last.store(millis(epoch), Ordering::Relaxed);
                    if listener.send_to(&buf[..n], peer).await.is_ok() {
                        state.bytes_received.fetch_add(n as u64, Ordering::Relaxed);
                    }
                }
                Err(e) => {
                    // The upstream port is closed (ICMP unreachable).
                    debug!(client = %peer, "udp upstream error: {e}");
                    state.record_failure();
                    break;
                }
            },
        }
    }
    let _ = closed.send(peer);
}
