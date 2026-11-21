//! VPN status and new peer configurations.

use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use axum::Json;
use axum::extract::State;
use axum::http::StatusCode;
use hearthgate_model::{Access, DesiredState, is_valid_name};
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

use crate::ApiState;
use crate::routes::{ApiError, ApiResult, blocking};

/// A peer that shook hands within this long counts as connected.
const ONLINE: u64 = 180;

/// The VPN as it runs.
#[derive(Debug, Serialize, ToSchema)]
pub struct VpnInfo {
    pub enabled: bool,
    /// The interface is up.
    pub running: bool,
    /// `kernel` or `userspace` (wireguard-go).
    pub backend: Option<String>,
    pub interface: String,
    /// This machine's public key.
    pub public_key: Option<String>,
    /// Where peers connect, `host:port`.
    pub endpoint: Option<String>,
    #[schema(value_type = String)]
    pub address: Ipv4Addr,
    pub subnet: String,
    pub port: u16,
    pub problems: Vec<String>,
    pub peers: Vec<VpnPeerInfo>,
    /// Addresses DNS answers on.
    #[schema(value_type = Vec<String>)]
    pub dns: Vec<SocketAddr>,
    /// Names the internal DNS answers, with their addresses.
    pub names: Vec<DnsName>,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct DnsName {
    pub name: String,
    #[schema(value_type = Vec<String>)]
    pub addresses: Vec<IpAddr>,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct VpnPeerInfo {
    pub name: String,
    #[schema(value_type = String)]
    pub address: Ipv4Addr,
    pub public_key: String,
    /// Where the peer last connected from.
    #[schema(value_type = Option<String>)]
    pub endpoint: Option<SocketAddr>,
    /// Unix seconds.
    pub latest_handshake: Option<u64>,
    pub online: bool,
    pub rx_bytes: u64,
    pub tx_bytes: u64,
}

/// Ask for a new peer's keys and configuration. Nothing is stored: add the
/// peer to the desired state with the returned public key and address.
#[derive(Debug, Deserialize, ToSchema)]
pub struct PeerConfigRequest {
    pub name: String,
    /// `"all"` (default) or a list of service names.
    #[serde(default)]
    #[schema(value_type = Object)]
    pub access: Access,
    /// The address to use. Defaults to the next free one.
    #[serde(default)]
    #[schema(value_type = Option<String>)]
    pub address: Option<Ipv4Addr>,
    /// The state the peer will be added to (such as the dashboard's draft),
    /// to pick a free address and the routes. Defaults to the current state.
    #[serde(default)]
    pub state: Option<DesiredState>,
}

/// A new peer's configuration. The private key is in `config` only, and is
/// not kept anywhere: show it once.
#[derive(Debug, Serialize, ToSchema)]
pub struct PeerConfigResponse {
    pub public_key: String,
    #[schema(value_type = String)]
    pub address: Ipv4Addr,
    /// The WireGuard configuration file.
    pub config: String,
    /// The configuration as a QR code (SVG), for phone apps.
    pub qr_svg: String,
}

async fn current_state(state: &ApiState) -> Result<DesiredState, ApiError> {
    let store = Arc::clone(&state.store);
    blocking(move || store.current_state()).await
}

fn public_ip(state: &ApiState) -> Option<IpAddr> {
    state
        .router
        .status()
        .public_ip
        .filter(|ip| !hearthgate_upnp::is_private_public_ip(*ip))
}

#[utoipa::path(get, path = "/api/v1/vpn", tag = "vpn", responses((status = 200, body = VpnInfo)))]
pub(crate) async fn info(State(state): State<ApiState>) -> ApiResult<VpnInfo> {
    let desired = current_state(&state).await?;
    let runtime = state.vpn.info();
    let live = if runtime.running {
        state.vpn.device.peers().await.unwrap_or_default()
    } else {
        Vec::new()
    };
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_secs());
    let peers = desired
        .vpn
        .peers
        .iter()
        .map(|p| {
            let seen = live.iter().find(|l| l.public_key == p.public_key);
            let handshake = seen.and_then(|s| s.latest_handshake);
            VpnPeerInfo {
                name: p.name.clone(),
                address: p.address,
                public_key: p.public_key.clone(),
                endpoint: seen.and_then(|s| s.endpoint),
                latest_handshake: handshake,
                online: handshake.is_some_and(|t| now.saturating_sub(t) < ONLINE),
                rx_bytes: seen.map_or(0, |s| s.rx_bytes),
                tx_bytes: seen.map_or(0, |s| s.tx_bytes),
            }
        })
        .collect();
    let plan_dns = if runtime.running && desired.dns.enabled {
        vec![SocketAddr::new(
            IpAddr::V4(desired.vpn.server_address()),
            53,
        )]
    } else {
        Vec::new()
    };
    let names = if desired.dns.enabled {
        hearthgate_vpn::Plan::new(&desired, &runtime.resolved, &[])
            .zone
            .names
            .into_iter()
            .map(|(name, addresses)| DnsName { name, addresses })
            .collect()
    } else {
        Vec::new()
    };
    Ok(Json(VpnInfo {
        enabled: desired.vpn.enabled,
        running: runtime.running,
        backend: runtime.backend.map(|b| match b {
            hearthgate_vpn::Backend::Kernel => "kernel".into(),
            hearthgate_vpn::Backend::Userspace => "userspace".into(),
        }),
        interface: state.vpn.device.name.clone(),
        public_key: runtime.public_key,
        endpoint: hearthgate_vpn::endpoint(&desired, public_ip(&state)),
        address: desired.vpn.server_address(),
        subnet: desired.vpn.subnet.to_string(),
        port: desired.vpn.port,
        problems: runtime.problems,
        peers,
        dns: plan_dns,
        names,
    }))
}

#[utoipa::path(post, path = "/api/v1/vpn/peer-config", tag = "vpn",
    request_body = PeerConfigRequest,
    responses(
        (status = 200, body = PeerConfigResponse),
        (status = 409, description = "No endpoint is known yet", body = crate::routes::Problem),
        (status = 422, description = "Bad name, address or access", body = crate::routes::Problem)))]
pub(crate) async fn peer_config(
    State(state): State<ApiState>,
    Json(request): Json<PeerConfigRequest>,
) -> ApiResult<PeerConfigResponse> {
    let desired = match request.state {
        Some(s) => s.normalized(),
        None => current_state(&state).await?,
    };
    if !is_valid_name(&request.name) {
        return Err(ApiError::new(
            StatusCode::UNPROCESSABLE_ENTITY,
            "use lowercase letters, digits and dashes for the name",
        ));
    }
    if let Access::Services(list) = &request.access
        && let Some(missing) = list
            .iter()
            .find(|n| !desired.services.iter().any(|s| &s.name == *n))
    {
        return Err(ApiError::new(
            StatusCode::UNPROCESSABLE_ENTITY,
            format!("no service is called `{missing}`"),
        ));
    }
    let vpn = &desired.vpn;
    let address = match request.address {
        Some(a) => {
            let taken = vpn
                .peers
                .iter()
                .any(|p| p.address == a && p.name != request.name);
            if !vpn.subnet.contains(&a) || a == vpn.server_address() || taken {
                return Err(ApiError::new(
                    StatusCode::UNPROCESSABLE_ENTITY,
                    format!("{a} is not a free address in {}", vpn.subnet),
                ));
            }
            a
        }
        None => vpn.free_address().ok_or_else(|| {
            ApiError::new(
                StatusCode::UNPROCESSABLE_ENTITY,
                format!("{} is full", vpn.subnet),
            )
        })?,
    };
    let Some(endpoint) = hearthgate_vpn::endpoint(&desired, public_ip(&state)) else {
        return Err(ApiError::new(
            StatusCode::CONFLICT,
            "set the address peers connect to (the VPN endpoint) first",
        ));
    };
    let runtime = Arc::clone(&state.vpn);
    let server_key = tokio::task::spawn_blocking(move || runtime.public_key())
        .await
        .map_err(|e| ApiError::new(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?
        .map_err(|e| ApiError::new(StatusCode::INTERNAL_SERVER_ERROR, format!("VPN key: {e}")))?;
    let pair = hearthgate_vpn::keys::generate()
        .map_err(|e| ApiError::new(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
    let info = state.vpn.info();
    let routes =
        hearthgate_vpn::client_routes(&desired, &request.access, &info.lan, &info.resolved);
    let config = hearthgate_vpn::ClientConfig {
        private_key: pair.private.clone(),
        address,
        server_public_key: server_key,
        endpoint,
        allowed_ips: routes,
        dns: desired
            .dns
            .enabled
            .then(|| (vpn.server_address(), desired.dns.zone.clone())),
    }
    .render();
    let qr_svg = hearthgate_vpn::qr_svg(&config).unwrap_or_default();
    Ok(Json(PeerConfigResponse {
        public_key: pair.public,
        address,
        config,
        qr_svg,
    }))
}
