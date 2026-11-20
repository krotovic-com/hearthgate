//! Upstreams and load-balancing pools.

use std::collections::HashMap;
use std::hash::{Hash, Hasher};
use std::net::{IpAddr, SocketAddr};
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::{Duration, Instant};

use hearthgate_model::Strategy;

/// Consecutive connection failures before a target is ejected passively.
pub const PASSIVE_FAILURES: u32 = 3;
/// How long a passively ejected target sits out before it is tried again.
pub const PASSIVE_EJECTION: Duration = Duration::from_secs(10);

/// Where an upstream lives.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum Endpoint {
    /// A fixed socket address.
    Socket(SocketAddr),
    /// A host name, resolved on every new connection.
    Host(String, u16),
    /// Not known right now (for example a stopped Docker container).
    Unresolved,
}

impl std::fmt::Display for Endpoint {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Socket(addr) => write!(f, "{addr}"),
            Self::Host(host, port) => write!(f, "{host}:{port}"),
            Self::Unresolved => f.write_str("unresolved"),
        }
    }
}

impl Endpoint {
    /// Builds an endpoint from a host string (IP or name) and a port.
    #[must_use]
    pub fn from_host(host: &str, port: u16) -> Self {
        let bare = host.trim_start_matches('[').trim_end_matches(']');
        match bare.parse::<IpAddr>() {
            Ok(ip) => Self::Socket(SocketAddr::new(ip, port)),
            Err(_) => Self::Host(host.to_owned(), port),
        }
    }

    /// Resolves to socket addresses.
    pub async fn resolve(&self) -> std::io::Result<Vec<SocketAddr>> {
        match self {
            Self::Socket(addr) => Ok(vec![*addr]),
            Self::Host(host, port) => Ok(tokio::net::lookup_host((host.as_str(), *port))
                .await?
                .collect()),
            Self::Unresolved => Err(std::io::Error::new(
                std::io::ErrorKind::NotFound,
                "upstream address is not known",
            )),
        }
    }
}

/// Live state of one service port, shared by every route that uses it and
/// kept across reloads.
#[derive(Debug)]
pub struct UpstreamState {
    /// Service name.
    pub service: String,
    /// Service port.
    pub port: u16,
    /// Current address.
    endpoint: Mutex<Endpoint>,
    /// Result of the active health check. True until a check says otherwise.
    healthy: AtomicBool,
    consecutive_failures: AtomicU32,
    /// Milliseconds since `epoch` until which the target is ejected.
    ejected_until_ms: AtomicU64,
    epoch: Instant,
    /// Open connections or in-flight requests.
    active: AtomicUsize,
    /// Totals for metrics.
    pub connections_total: AtomicU64,
    pub failures_total: AtomicU64,
    pub bytes_sent: AtomicU64,
    pub bytes_received: AtomicU64,
}

impl UpstreamState {
    fn new(service: &str, port: u16, endpoint: Endpoint) -> Self {
        Self {
            service: service.to_owned(),
            port,
            endpoint: Mutex::new(endpoint),
            healthy: AtomicBool::new(true),
            consecutive_failures: AtomicU32::new(0),
            ejected_until_ms: AtomicU64::new(0),
            epoch: Instant::now(),
            active: AtomicUsize::new(0),
            connections_total: AtomicU64::new(0),
            failures_total: AtomicU64::new(0),
            bytes_sent: AtomicU64::new(0),
            bytes_received: AtomicU64::new(0),
        }
    }

    /// Current address.
    #[must_use]
    pub fn endpoint(&self) -> Endpoint {
        self.endpoint
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
    }

    fn set_endpoint(&self, endpoint: Endpoint) {
        *self.endpoint.lock().unwrap_or_else(PoisonError::into_inner) = endpoint;
    }

    /// Whether the target can take traffic: resolved, healthy and not
    /// ejected after recent failures.
    #[must_use]
    pub fn available(&self) -> bool {
        self.healthy.load(Ordering::Relaxed)
            && !self.ejected()
            && self.endpoint() != Endpoint::Unresolved
    }

    /// Result of the last active health check.
    #[must_use]
    pub fn healthy(&self) -> bool {
        self.healthy.load(Ordering::Relaxed)
    }

    /// Records the result of an active health check. Returns true when the
    /// state changed.
    pub fn set_healthy(&self, healthy: bool) -> bool {
        let before = self.healthy.swap(healthy, Ordering::Relaxed);
        if healthy {
            self.consecutive_failures.store(0, Ordering::Relaxed);
            self.ejected_until_ms.store(0, Ordering::Relaxed);
        }
        before != healthy
    }

    /// Whether the target is sitting out after repeated failures.
    #[must_use]
    pub fn ejected(&self) -> bool {
        self.now_ms() < self.ejected_until_ms.load(Ordering::Relaxed)
    }

    fn now_ms(&self) -> u64 {
        u64::try_from(self.epoch.elapsed().as_millis()).unwrap_or(u64::MAX)
    }

    /// Records a successful connection.
    pub fn record_success(&self) {
        self.consecutive_failures.store(0, Ordering::Relaxed);
    }

    /// Records a failed connection or request. After
    /// [`PASSIVE_FAILURES`] in a row the target is ejected for
    /// [`PASSIVE_EJECTION`].
    pub fn record_failure(&self) {
        self.failures_total.fetch_add(1, Ordering::Relaxed);
        let n = self.consecutive_failures.fetch_add(1, Ordering::Relaxed) + 1;
        if n >= PASSIVE_FAILURES {
            let until =
                self.now_ms() + u64::try_from(PASSIVE_EJECTION.as_millis()).unwrap_or(u64::MAX);
            self.ejected_until_ms.store(until, Ordering::Relaxed);
            self.consecutive_failures.store(0, Ordering::Relaxed);
        }
    }

    /// Open connections or in-flight requests.
    #[must_use]
    pub fn active(&self) -> usize {
        self.active.load(Ordering::Relaxed)
    }

    /// Marks a connection as open until the guard is dropped.
    #[must_use]
    pub fn start(self: &Arc<Self>) -> ActiveGuard {
        self.active.fetch_add(1, Ordering::Relaxed);
        self.connections_total.fetch_add(1, Ordering::Relaxed);
        ActiveGuard(Arc::clone(self))
    }
}

/// Decrements the active count of an upstream when dropped.
#[derive(Debug)]
pub struct ActiveGuard(Arc<UpstreamState>);

impl Drop for ActiveGuard {
    fn drop(&mut self) {
        self.0.active.fetch_sub(1, Ordering::Relaxed);
    }
}

/// Registry of upstream states, keyed by service and port. States survive
/// reloads so health and counters are not reset by an unrelated change.
#[derive(Debug, Default)]
pub struct Upstreams {
    states: Mutex<HashMap<(String, u16), Arc<UpstreamState>>>,
}

impl Upstreams {
    /// Returns the state for a service port, creating it if needed and
    /// updating its address.
    pub fn get(&self, service: &str, port: u16, endpoint: Endpoint) -> Arc<UpstreamState> {
        let mut states = self.states.lock().unwrap_or_else(PoisonError::into_inner);
        let state = states
            .entry((service.to_owned(), port))
            .or_insert_with(|| Arc::new(UpstreamState::new(service, port, endpoint.clone())));
        if state.endpoint() != endpoint {
            state.set_endpoint(endpoint);
        }
        Arc::clone(state)
    }

    /// Forgets service ports that are no longer used by any route.
    pub fn retain(&self, keep: &std::collections::HashSet<(String, u16)>) {
        self.states
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .retain(|key, _| keep.contains(key));
    }

    /// Every known upstream.
    #[must_use]
    pub fn all(&self) -> Vec<Arc<UpstreamState>> {
        let mut all: Vec<_> = self
            .states
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .values()
            .cloned()
            .collect();
        all.sort_by(|a, b| (&a.service, a.port).cmp(&(&b.service, b.port)));
        all
    }
}

/// One member of a pool.
#[derive(Debug, Clone)]
pub struct Member {
    pub state: Arc<UpstreamState>,
    pub weight: u32,
}

/// The targets of one route and how to choose between them.
#[derive(Debug)]
pub struct Pool {
    /// Route name, for logs and metrics.
    pub route: String,
    pub strategy: Strategy,
    pub members: Vec<Member>,
    total_weight: u64,
    cursor: AtomicU64,
}

impl Pool {
    /// Creates a pool.
    #[must_use]
    pub fn new(route: &str, strategy: Strategy, members: Vec<Member>) -> Self {
        let total_weight = members.iter().map(|m| u64::from(m.weight.max(1))).sum();
        Self {
            route: route.to_owned(),
            strategy,
            members,
            total_weight,
            cursor: AtomicU64::new(0),
        }
    }

    /// Chooses a member for a new connection from `client`, skipping the
    /// indexes in `tried`. Members that are unhealthy or ejected are skipped
    /// too, unless no other member is left, so a broken health check never
    /// takes a whole route down on its own.
    #[must_use]
    pub fn pick(&self, client: IpAddr, tried: &[usize]) -> Option<usize> {
        let candidates: Vec<usize> = (0..self.members.len())
            .filter(|i| !tried.contains(i))
            .collect();
        if candidates.is_empty() {
            return None;
        }
        let available: Vec<usize> = candidates
            .iter()
            .copied()
            .filter(|&i| self.members[i].state.available())
            .collect();
        let resolved: Vec<usize> = candidates
            .iter()
            .copied()
            .filter(|&i| self.members[i].state.endpoint() != Endpoint::Unresolved)
            .collect();
        let set = if !available.is_empty() {
            available
        } else if !resolved.is_empty() {
            resolved
        } else {
            return None;
        };
        Some(match self.strategy {
            Strategy::RoundRobin => self.round_robin(&set),
            Strategy::LeastConnections => self.least_connections(&set),
            Strategy::SourceHash => self.source_hash(client, &set),
        })
    }

    fn round_robin(&self, set: &[usize]) -> usize {
        let n = self.cursor.fetch_add(1, Ordering::Relaxed) % self.total_weight.max(1);
        // Walk the weighted ring to the slot for `n`, then take the first
        // eligible member from there.
        let mut acc = 0;
        let mut start = 0;
        for (i, m) in self.members.iter().enumerate() {
            acc += u64::from(m.weight.max(1));
            if n < acc {
                start = i;
                break;
            }
        }
        (0..self.members.len())
            .map(|k| (start + k) % self.members.len())
            .find(|i| set.contains(i))
            .unwrap_or(set[0])
    }

    fn least_connections(&self, set: &[usize]) -> usize {
        let min = set
            .iter()
            .map(|&i| self.members[i].state.active())
            .min()
            .unwrap_or(0);
        let tied: Vec<usize> = set
            .iter()
            .copied()
            .filter(|&i| self.members[i].state.active() == min)
            .collect();
        self.round_robin(&tied)
    }

    /// Rendezvous hashing: a client keeps its target while that target is
    /// available, and only the clients of a failed target move. Weights do
    /// not apply to this strategy.
    fn source_hash(&self, client: IpAddr, set: &[usize]) -> usize {
        set.iter()
            .copied()
            .max_by_key(|&i| {
                let m = &self.members[i];
                let mut h = std::collections::hash_map::DefaultHasher::new();
                client.hash(&mut h);
                m.state.service.hash(&mut h);
                m.state.port.hash(&mut h);
                h.finish()
            })
            .unwrap_or(set[0])
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pool(strategy: Strategy, weights: &[u32]) -> (Pool, Upstreams) {
        let ups = Upstreams::default();
        let members = weights
            .iter()
            .enumerate()
            .map(|(i, &w)| Member {
                state: ups.get(
                    &format!("s{i}"),
                    80,
                    Endpoint::Socket(SocketAddr::from((
                        [127, 0, 0, 1],
                        8000 + u16::try_from(i).unwrap(),
                    ))),
                ),
                weight: w,
            })
            .collect();
        (Pool::new("r", strategy, members), ups)
    }

    const CLIENT: IpAddr = IpAddr::V4(std::net::Ipv4Addr::new(192, 0, 2, 1));

    #[test]
    fn round_robin_alternates_and_respects_weights() {
        let (p, _) = pool(Strategy::RoundRobin, &[1, 1]);
        let picks: Vec<_> = (0..4).map(|_| p.pick(CLIENT, &[]).unwrap()).collect();
        assert_eq!(picks, [0, 1, 0, 1]);
        let (p, _) = pool(Strategy::RoundRobin, &[3, 1]);
        let zeros = (0..400).filter(|_| p.pick(CLIENT, &[]) == Some(0)).count();
        assert_eq!(zeros, 300);
    }

    #[test]
    fn skips_unhealthy_and_tried_members() {
        let (p, _) = pool(Strategy::RoundRobin, &[1, 1, 1]);
        p.members[1].state.set_healthy(false);
        for _ in 0..10 {
            assert_ne!(p.pick(CLIENT, &[]), Some(1));
        }
        assert_eq!(
            p.pick(CLIENT, &[0, 2]),
            Some(1),
            "fails open when nothing else is left"
        );
        assert_eq!(p.pick(CLIENT, &[0, 1, 2]), None);
    }

    #[test]
    fn passive_ejection() {
        let (p, _) = pool(Strategy::RoundRobin, &[1, 1]);
        for _ in 0..PASSIVE_FAILURES {
            p.members[0].state.record_failure();
        }
        assert!(p.members[0].state.ejected());
        for _ in 0..10 {
            assert_eq!(p.pick(CLIENT, &[]), Some(1));
        }
        p.members[0].state.set_healthy(true);
        assert!(!p.members[0].state.ejected());
    }

    #[test]
    fn least_connections_prefers_idle() {
        let (p, _) = pool(Strategy::LeastConnections, &[1, 1]);
        let _busy = p.members[0].state.start();
        for _ in 0..5 {
            assert_eq!(p.pick(CLIENT, &[]), Some(1));
        }
    }

    #[test]
    fn source_hash_is_sticky() {
        let (p, _) = pool(Strategy::SourceHash, &[1, 1, 1, 1]);
        let first = p.pick(CLIENT, &[]).unwrap();
        for _ in 0..10 {
            assert_eq!(p.pick(CLIENT, &[]), Some(first));
        }
        // Different clients spread over the members.
        let spread: std::collections::HashSet<_> = (0..64u8)
            .map(|n| p.pick(IpAddr::from([10, 0, 0, n]), &[]).unwrap())
            .collect();
        assert!(spread.len() > 1);
    }

    #[test]
    fn unresolved_members_are_never_picked() {
        let ups = Upstreams::default();
        let p = Pool::new(
            "r",
            Strategy::RoundRobin,
            vec![Member {
                state: ups.get("gone", 80, Endpoint::Unresolved),
                weight: 1,
            }],
        );
        assert_eq!(p.pick(CLIENT, &[]), None);
    }
}
