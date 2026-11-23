//! Who can reach the dashboard: the admin listeners and the allowed client
//! ranges.
//!
//! By default Hearthgate finds the machine's private (LAN) addresses at each
//! start, listens on those and on loopback, and lets in only clients from
//! loopback and the LAN networks it found. The dashboard can switch to a
//! fixed list instead. Either way only private ranges can be allowed, so the
//! dashboard is never reachable from the internet. When the bootstrap file
//! sets `admin.listen` or `admin.allow`, those win and the dashboard shows
//! them read-only.
//!
//! Listeners change without a restart: new addresses are bound first, and
//! nothing changes if any of them fails.

use std::collections::BTreeMap;
use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::path::PathBuf;
use std::sync::{Arc, Mutex, OnceLock, PoisonError, RwLock};

use axum::extract::{Extension, State};
use axum::http::StatusCode;
use axum::{Json, Router};
use hearthgate_store::Store;
use serde::{Deserialize, Serialize};
use tokio::net::TcpListener;
use tokio_util::sync::CancellationToken;
use tracing::{error, info, warn};
use utoipa::ToSchema;

use crate::auth::ClientAddr;
use crate::routes::{ApiError, ApiResult, blocking};
use crate::tls::AdminTls;
use crate::{ApiState, Net, accept_loop};

/// Store key of the dashboard's admin access settings.
pub const META_KEY: &str = "admin_access";
/// Default admin port.
pub const DEFAULT_PORT: u16 = 7380;

/// How the admin listeners are chosen.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ToSchema, Default)]
#[serde(rename_all = "lowercase")]
pub enum AccessMode {
    /// This machine's LAN addresses, found again at every start.
    #[default]
    Lan,
    /// The addresses and ranges listed in the settings.
    Custom,
}

/// Admin access settings, as stored by the dashboard.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct AccessSettings {
    pub mode: AccessMode,
    /// Port of the dashboard and admin API.
    pub port: u16,
    /// Addresses to listen on, in `custom` mode. Loopback is always added.
    #[serde(default)]
    #[schema(value_type = Vec<String>)]
    pub listen: Vec<IpAddr>,
    /// Client ranges allowed in, in `custom` mode. Loopback is always
    /// allowed. Only private ranges are accepted.
    #[serde(default)]
    #[schema(value_type = Vec<String>)]
    pub allow: Vec<Net>,
}

impl Default for AccessSettings {
    fn default() -> Self {
        Self {
            mode: AccessMode::Lan,
            port: DEFAULT_PORT,
            listen: Vec::new(),
            allow: Vec::new(),
        }
    }
}

/// Listen addresses and allowed ranges in effect.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, ToSchema)]
pub struct Resolved {
    #[schema(value_type = Vec<String>)]
    pub listen: Vec<SocketAddr>,
    #[schema(value_type = Vec<String>)]
    pub allow: Vec<Net>,
}

/// Where the settings come from.
#[derive(Debug, Clone)]
pub enum Source {
    /// Fixed by the bootstrap file.
    Config(Resolved),
    /// Stored by the dashboard (or the defaults).
    Dashboard,
}

/// Which certificate the admin listener uses.
#[derive(Debug, Clone)]
pub enum TlsMode {
    /// Plain HTTP (behind another TLS proxy).
    Off,
    /// A certificate from the bootstrap file.
    Fixed(Arc<AdminTls>),
    /// A self-signed certificate in the data directory, renewed when the
    /// names change.
    SelfSigned(PathBuf),
}

fn loopback() -> Vec<Net> {
    vec![
        "127.0.0.0/8".parse().expect("valid range"),
        "::1/128".parse().expect("valid range"),
    ]
}

/// Whether `ip` is a private address: RFC 1918, carrier-grade NAT (used by
/// some VPNs), IPv6 unique local, link-local or loopback.
#[must_use]
pub fn is_private(ip: IpAddr) -> bool {
    match ip.to_canonical() {
        IpAddr::V4(v4) => {
            v4.is_private()
                || v4.is_loopback()
                || v4.is_link_local()
                || (v4.octets()[0] == 100 && (v4.octets()[1] & 0xc0) == 64)
        }
        IpAddr::V6(v6) => {
            v6.is_loopback()
                || (v6.segments()[0] & 0xfe00) == 0xfc00
                || (v6.segments()[0] & 0xffc0) == 0xfe80
        }
    }
}

/// Whether every address in `net` is private.
#[must_use]
pub fn is_private_net(net: &Net) -> bool {
    is_private(net.network()) && is_private(net.broadcast())
}

/// This machine's private addresses and their networks (no loopback, no
/// IPv6 link-local, which cannot be bound without a scope), without the
/// bridges of container engines: containers are not the LAN, so they get
/// neither the LAN's firewall allowance nor the dashboard.
#[must_use]
pub fn detect_lan() -> Vec<(IpAddr, Net)> {
    detect(|name| !is_container_bridge(name))
}

/// The networks of container engine bridges on this machine (Docker,
/// Podman, CNI), for routes that VPN devices with full access need.
#[must_use]
pub fn detect_container_networks() -> Vec<(IpAddr, Net)> {
    detect(is_container_bridge)
}

/// Whether an interface is a container engine's bridge: Docker's `docker0`
/// and `br-` plus the 12-character network id, Podman's and CNI's bridges.
/// Bridges that Docker networks name themselves are not recognised.
#[must_use]
pub fn is_container_bridge(name: &str) -> bool {
    let docker_network = name.strip_prefix("br-").is_some_and(|id| {
        id.len() == 12
            && id
                .bytes()
                .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
    });
    docker_network || name == "docker0" || name.starts_with("podman") || name.starts_with("cni")
}

fn detect(include: impl Fn(&str) -> bool) -> Vec<(IpAddr, Net)> {
    let mut found = Vec::new();
    for iface in if_addrs::get_if_addrs().unwrap_or_default() {
        if !include(&iface.name) {
            continue;
        }
        let ip = iface.ip();
        if ip.is_loopback() || !is_private(ip) {
            continue;
        }
        if matches!(ip, IpAddr::V6(v6) if (v6.segments()[0] & 0xffc0) == 0xfe80) {
            continue;
        }
        let net = match &iface.addr {
            if_addrs::IfAddr::V4(a) => Net::with_netmask(IpAddr::V4(a.ip), IpAddr::V4(a.netmask)),
            if_addrs::IfAddr::V6(a) => Net::with_netmask(IpAddr::V6(a.ip), IpAddr::V6(a.netmask)),
        };
        if let Ok(net) = net {
            found.push((ip, net.trunc()));
        }
    }
    found.sort();
    found.dedup();
    found
}

/// Turns settings into listen addresses and allowed ranges.
#[must_use]
pub fn resolve(settings: &AccessSettings, lan: &[(IpAddr, Net)]) -> Resolved {
    let mut listen = vec![SocketAddr::from((Ipv4Addr::LOCALHOST, settings.port))];
    let mut allow = loopback();
    match settings.mode {
        AccessMode::Lan => {
            for (ip, net) in lan {
                listen.push(SocketAddr::new(*ip, settings.port));
                allow.push(*net);
            }
        }
        AccessMode::Custom => {
            listen.extend(
                settings
                    .listen
                    .iter()
                    .map(|ip| SocketAddr::new(*ip, settings.port)),
            );
            allow.extend(settings.allow.iter().copied());
        }
    }
    listen.sort();
    listen.dedup();
    allow.sort();
    allow.dedup();
    Resolved { listen, allow }
}

/// Checks settings from the dashboard.
pub fn check(settings: &AccessSettings) -> Result<(), Vec<String>> {
    let mut problems = Vec::new();
    if settings.port == 0 {
        problems.push("choose a port between 1 and 65535".to_owned());
    }
    if settings.mode == AccessMode::Custom {
        for net in &settings.allow {
            if !is_private_net(net) {
                problems.push(format!(
                    "{net} includes public addresses; the dashboard is only for your LAN and VPN"
                ));
            }
        }
        for ip in &settings.listen {
            if !ip.is_unspecified() && !is_private(*ip) {
                problems.push(format!(
                    "{ip} is a public address; listen on a LAN address instead"
                ));
            }
        }
    }
    if problems.is_empty() {
        Ok(())
    } else {
        Err(problems)
    }
}

#[derive(Debug, Default)]
struct Running {
    resolved: Resolved,
    sockets: BTreeMap<SocketAddr, CancellationToken>,
    /// Addresses that could not be bound at start.
    failed: Vec<String>,
}

/// The admin listeners, changeable while running.
pub struct AccessControl {
    source: Source,
    tls_mode: TlsMode,
    allow: Arc<RwLock<Vec<Net>>>,
    tls: Arc<RwLock<Option<Arc<AdminTls>>>>,
    running: tokio::sync::Mutex<Running>,
    snapshot: Mutex<Resolved>,
    app: OnceLock<Router>,
    stop: CancellationToken,
}

impl std::fmt::Debug for AccessControl {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AccessControl")
            .field("source", &self.source)
            .finish_non_exhaustive()
    }
}

impl AccessControl {
    #[must_use]
    pub fn new(source: Source, tls_mode: TlsMode, stop: CancellationToken) -> Self {
        let tls = match &tls_mode {
            TlsMode::Fixed(t) => Some(Arc::clone(t)),
            _ => None,
        };
        Self {
            source,
            tls_mode,
            allow: Arc::default(),
            tls: Arc::new(RwLock::new(tls)),
            running: tokio::sync::Mutex::default(),
            snapshot: Mutex::default(),
            app: OnceLock::new(),
            stop,
        }
    }

    /// Whether the dashboard can change the settings.
    #[must_use]
    pub fn locked(&self) -> bool {
        matches!(self.source, Source::Config(_))
    }

    /// Whether the listeners speak HTTPS.
    #[must_use]
    pub fn https(&self) -> bool {
        !matches!(self.tls_mode, TlsMode::Off)
    }

    /// Listen addresses and allowed ranges in effect.
    #[must_use]
    pub fn current(&self) -> Resolved {
        self.snapshot
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
    }

    /// The current certificate, if any.
    #[must_use]
    pub fn certificate(&self) -> Option<Arc<AdminTls>> {
        self.tls
            .read()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
    }

    /// Reads the stored settings (or the defaults).
    pub fn stored(store: &Store) -> AccessSettings {
        store
            .meta_get(META_KEY)
            .ok()
            .flatten()
            .and_then(|json| serde_json::from_str(&json).ok())
            .unwrap_or_default()
    }

    /// What the listeners should be now.
    #[must_use]
    pub fn wanted(&self, store: &Store) -> Resolved {
        match &self.source {
            Source::Config(r) => r.clone(),
            Source::Dashboard => resolve(&Self::stored(store), &detect_lan()),
        }
    }

    /// Follows address changes (such as the VPN interface coming up) in LAN
    /// mode. Settings pinned in the bootstrap file stay as they are.
    pub async fn refresh(&self, store: &Arc<Store>) {
        if self.locked() || self.app.get().is_none() {
            return;
        }
        let wanted = {
            let store = Arc::clone(store);
            let source = self.source.clone();
            tokio::task::spawn_blocking(move || match source {
                Source::Config(r) => r,
                Source::Dashboard => resolve(&Self::stored(&store), &detect_lan()),
            })
            .await
        };
        let Ok(wanted) = wanted else { return };
        if wanted != self.current() {
            info!("network addresses changed; updating the dashboard's listeners");
            if let Err(e) = self.apply(wanted, false).await {
                warn!("admin API: {e}");
            }
        }
    }

    /// Starts the listeners. Addresses that cannot be bound are logged and
    /// skipped, so one missing address does not keep the dashboard down.
    pub async fn start(&self, app: Router, wanted: Resolved) {
        let _ = self.app.set(app);
        if let Err(e) = self.apply(wanted, false).await {
            error!("admin API: {e}");
        }
    }

    /// Changes the listeners. With `strict`, nothing changes unless every
    /// new address can be bound.
    pub async fn apply(&self, wanted: Resolved, strict: bool) -> Result<(), String> {
        let Some(app) = self.app.get().cloned() else {
            return Err("the admin API has not started yet".to_owned());
        };
        let mut running = self.running.lock().await;
        let mut bound = Vec::new();
        let mut failed = Vec::new();
        for addr in &wanted.listen {
            if running.sockets.contains_key(addr) {
                continue;
            }
            match TcpListener::bind(addr).await {
                Ok(l) => bound.push((*addr, l)),
                Err(e) => {
                    let message = format!("cannot listen on {addr}: {e}");
                    if strict {
                        return Err(message);
                    }
                    error!("admin API {message}");
                    failed.push(message);
                }
            }
        }
        if let TlsMode::SelfSigned(dir) = &self.tls_mode {
            let mut names = crate::tls::local_names();
            names.extend(
                wanted
                    .listen
                    .iter()
                    .filter(|a| !a.ip().is_unspecified())
                    .map(|a| a.ip().to_string()),
            );
            match crate::tls::load_or_create(dir, &names) {
                Ok(tls) => {
                    let mut current = self.tls.write().unwrap_or_else(PoisonError::into_inner);
                    if current
                        .as_ref()
                        .is_none_or(|c| c.fingerprint != tls.fingerprint)
                    {
                        info!(
                            certificate = %tls.cert_path.display(),
                            "dashboard certificate fingerprint (SHA-256): {}",
                            tls.fingerprint
                        );
                    }
                    *current = Some(Arc::new(tls));
                }
                Err(e) => {
                    let message = format!("cannot prepare the dashboard certificate: {e}");
                    if strict || self.certificate().is_none() {
                        return Err(message);
                    }
                    warn!("{message}");
                }
            }
        }
        self.allow
            .write()
            .unwrap_or_else(PoisonError::into_inner)
            .clone_from(&wanted.allow);
        running.sockets.retain(|addr, token| {
            let keep = wanted.listen.contains(addr);
            if !keep {
                info!(listen = %addr, "dashboard stopped listening");
                token.cancel();
            }
            keep
        });
        let scheme = if self.https() { "https" } else { "http" };
        for (addr, listener) in bound {
            if addr.ip().is_unspecified() {
                warn!(
                    "admin API listens on {addr}; only allowed clients can connect, but binding \
                     a LAN address is safer"
                );
            }
            info!(listen = %addr, "dashboard and admin API listening ({scheme})");
            let token = self.stop.child_token();
            tokio::spawn(accept_loop(
                listener,
                Arc::clone(&self.allow),
                Arc::clone(&self.tls),
                app.clone(),
                token.clone(),
                self.stop.clone(),
            ));
            running.sockets.insert(addr, token);
        }
        running.failed = failed;
        let effective = Resolved {
            listen: running.sockets.keys().copied().collect(),
            allow: wanted.allow,
        };
        running.resolved = effective.clone();
        *self.snapshot.lock().unwrap_or_else(PoisonError::into_inner) = effective;
        Ok(())
    }
}

/// Admin access settings and what is in effect.
#[derive(Debug, Serialize, ToSchema)]
pub struct AccessInfo {
    /// Set in the bootstrap file; the dashboard cannot change it.
    pub locked: bool,
    /// The stored settings.
    pub settings: AccessSettings,
    /// What is in effect now.
    pub effective: Resolved,
    /// LAN addresses and networks found on this machine.
    pub detected: Resolved,
    /// Whether the dashboard uses HTTPS.
    pub https: bool,
    /// SHA-256 fingerprint of the dashboard certificate.
    pub fingerprint: Option<String>,
    /// Your address, as Hearthgate sees it.
    pub client: String,
}

async fn info_for(state: &ApiState, client: IpAddr) -> Result<AccessInfo, ApiError> {
    let store = Arc::clone(&state.store);
    let settings = blocking(move || Ok(AccessControl::stored(&store))).await?;
    let lan = detect_lan();
    let detected = resolve(
        &AccessSettings {
            mode: AccessMode::Lan,
            ..settings.clone()
        },
        &lan,
    );
    Ok(AccessInfo {
        locked: state.access.locked(),
        settings,
        effective: state.access.current(),
        detected,
        https: state.access.https(),
        fingerprint: state.access.certificate().map(|c| c.fingerprint.clone()),
        client: client.to_canonical().to_string(),
    })
}

/// Who can reach the dashboard.
#[utoipa::path(
    get, path = "/api/v1/settings/admin-access", tag = "settings",
    responses((status = 200, body = AccessInfo))
)]
pub(crate) async fn get_access(
    State(state): State<ApiState>,
    Extension(ClientAddr(client)): Extension<ClientAddr>,
) -> ApiResult<AccessInfo> {
    Ok(Json(info_for(&state, client.ip()).await?))
}

/// Change who can reach the dashboard. Takes effect at once. Refused when
/// it would lock out the client making the change.
#[utoipa::path(
    put, path = "/api/v1/settings/admin-access", tag = "settings", request_body = AccessSettings,
    responses(
        (status = 200, body = AccessInfo),
        (status = 409, description = "Set in the bootstrap file", body = crate::routes::Problem),
        (status = 422, description = "Not valid, or would lock you out", body = crate::routes::Problem),
    )
)]
pub(crate) async fn put_access(
    State(state): State<ApiState>,
    Extension(ClientAddr(client)): Extension<ClientAddr>,
    Json(settings): Json<AccessSettings>,
) -> ApiResult<AccessInfo> {
    if state.access.locked() {
        return Err(ApiError::new(
            StatusCode::CONFLICT,
            "admin.listen or admin.allow is set in the bootstrap file; change it there",
        ));
    }
    let mut settings = settings;
    settings.listen.sort();
    settings.listen.dedup();
    settings.allow = settings.allow.iter().map(Net::trunc).collect();
    settings.allow.sort();
    settings.allow.dedup();
    if let Err(problems) = check(&settings) {
        return Err(ApiError::with_problems(
            StatusCode::UNPROCESSABLE_ENTITY,
            "these settings are not valid",
            problems,
        ));
    }
    let wanted = resolve(&settings, &detect_lan());
    if !crate::allowed(&wanted.allow, client.ip()) {
        return Err(ApiError::new(
            StatusCode::UNPROCESSABLE_ENTITY,
            format!(
                "your address {} would not be allowed, so you would lock yourself out; add its \
                 network to the allowed ranges",
                client.ip().to_canonical()
            ),
        ));
    }
    let remote = !client.ip().to_canonical().is_loopback();
    if remote && wanted.listen.iter().all(|a| a.ip().is_loopback()) {
        return Err(ApiError::new(
            StatusCode::UNPROCESSABLE_ENTITY,
            "none of these listen addresses is reachable from your computer, so you would lock \
             yourself out",
        ));
    }
    state
        .access
        .apply(wanted, true)
        .await
        .map_err(|e| ApiError::new(StatusCode::UNPROCESSABLE_ENTITY, e))?;
    let store = Arc::clone(&state.store);
    let json = serde_json::to_string(&settings).unwrap_or_default();
    blocking(move || store.meta_set(META_KEY, &json)).await?;
    info!(mode = ?settings.mode, port = settings.port, "admin access changed from the dashboard");
    Ok(Json(info_for(&state, client.ip()).await?))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn net(s: &str) -> Net {
        s.parse().unwrap()
    }

    #[test]
    fn private_ranges() {
        assert!(is_private("192.168.1.10".parse().unwrap()));
        assert!(is_private("100.100.1.1".parse().unwrap()));
        assert!(is_private("fd00::1".parse().unwrap()));
        assert!(is_private("::ffff:10.0.0.1".parse().unwrap()));
        assert!(!is_private("8.8.8.8".parse().unwrap()));
        assert!(!is_private("2001:db8::1".parse().unwrap()));
        assert!(is_private_net(&net("10.0.0.0/8")));
        assert!(!is_private_net(&net("0.0.0.0/0")));
        assert!(!is_private_net(&net("100.0.0.0/8")));
    }

    #[test]
    fn container_bridges_are_not_lan() {
        for name in [
            "docker0",
            "br-3f2a9c1e5b7d",
            "podman0",
            "cni-podman0",
            "cni0",
        ] {
            assert!(is_container_bridge(name), "{name}");
        }
        for name in [
            "eth0",
            "br0",
            "br-lan",
            "br-3f2a9c1e5b7",
            "br-3F2A9C1E5B7D",
            "wg0",
            "hg0",
        ] {
            assert!(!is_container_bridge(name), "{name}");
        }
    }

    #[test]
    fn lan_mode_uses_detected_networks() {
        let lan = vec![("192.168.1.10".parse().unwrap(), net("192.168.1.0/24"))];
        let r = resolve(&AccessSettings::default(), &lan);
        assert_eq!(
            r.listen,
            vec![
                "127.0.0.1:7380".parse().unwrap(),
                "192.168.1.10:7380".parse().unwrap()
            ]
        );
        assert!(r.allow.contains(&net("192.168.1.0/24")));
        assert!(r.allow.contains(&net("127.0.0.0/8")));
        assert!(!r.allow.contains(&net("10.0.0.0/8")));
    }

    #[test]
    fn custom_mode_refuses_public_ranges() {
        let mut s = AccessSettings {
            mode: AccessMode::Custom,
            allow: vec![net("10.0.0.0/8")],
            listen: vec!["10.0.0.2".parse().unwrap()],
            ..AccessSettings::default()
        };
        assert!(check(&s).is_ok());
        s.allow.push(net("0.0.0.0/0"));
        s.listen.push("203.0.113.1".parse().unwrap());
        assert_eq!(check(&s).unwrap_err().len(), 2);
    }
}
