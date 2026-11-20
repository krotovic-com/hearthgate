//! Firewall status, recent drops, bans and change confirmation.

use std::net::IpAddr;
use std::time::{Duration, Instant};

use axum::Json;
use axum::extract::{Extension, Path, Request, State};
use axum::http::{HeaderValue, StatusCode, header};
use axum::middleware::Next;
use axum::response::Response;
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

use crate::ApiState;
use crate::routes::ApiError;

/// When the connection carrying a request was accepted.
#[derive(Debug, Clone, Copy)]
pub struct Accepted(pub Instant);

/// Ends the connection carrying a request, once the requests on it finish.
#[derive(Debug, Clone)]
pub struct CloseConnection(pub tokio_util::sync::CancellationToken);

/// Firewall overview.
#[derive(Debug, Serialize, ToSchema)]
pub struct FirewallInfo {
    /// The table is installed.
    pub installed: bool,
    /// Why the last install failed.
    pub error: Option<String>,
    /// A change waiting for confirmation.
    pub pending: Option<PendingInfo>,
    /// Packets dropped since the table was last replaced.
    pub dropped: DroppedInfo,
    /// Addresses loaded from blocklists and countries.
    pub feed_entries: usize,
    /// Per blocklist (and `country-xx`): entries, last update, last error.
    pub feeds: Vec<FeedInfo>,
    /// Active bans.
    pub bans: Vec<BanInfo>,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct PendingInfo {
    pub revision: i64,
    pub seconds_left: u64,
}

#[derive(Debug, Default, Serialize, ToSchema)]
pub struct DroppedInfo {
    pub total: u64,
    pub blocklist: u64,
    pub ban: u64,
    pub policy: u64,
    pub ratelimit: u64,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct FeedInfo {
    pub name: String,
    pub entries: usize,
    /// Last download, as seconds since the Unix epoch.
    pub updated: Option<u64>,
    pub error: Option<String>,
}

#[derive(Debug, Serialize, Deserialize, ToSchema)]
pub struct BanInfo {
    #[schema(value_type = String)]
    pub address: IpAddr,
    pub seconds_left: u64,
}

/// A source dropped in the last hour.
#[derive(Debug, Serialize, ToSchema)]
pub struct RecentDropInfo {
    #[schema(value_type = String)]
    pub source: IpAddr,
    pub protocol: String,
    pub port: u16,
    /// `blocklist`, `ban`, `policy` or `rate-limit`.
    pub reason: String,
    pub packets: u64,
    pub last_seen_secs: u64,
}

/// Ban an address by hand.
#[derive(Debug, Deserialize, ToSchema)]
pub struct BanRequest {
    #[schema(value_type = String)]
    pub address: IpAddr,
    /// For example `1h` or `7d`. Defaults to the automatic ban time.
    #[serde(default)]
    pub duration: Option<String>,
}

#[utoipa::path(get, path = "/api/v1/firewall", tag = "firewall",
    responses((status = 200, body = FirewallInfo)))]
pub(crate) async fn info(State(state): State<ApiState>) -> Json<FirewallInfo> {
    let p = &state.perimeter;
    let status = p.status();
    let dropped = p.dropped().await;
    let bans = p
        .bans()
        .await
        .into_iter()
        .map(|(address, left)| BanInfo {
            address,
            seconds_left: left.as_secs(),
        })
        .collect();
    Json(FirewallInfo {
        installed: status.installed,
        error: status.error,
        pending: status.pending.map(|(revision, seconds_left)| PendingInfo {
            revision,
            seconds_left,
        }),
        dropped: DroppedInfo {
            total: dropped.total(),
            blocklist: dropped.blocklist,
            ban: dropped.ban,
            policy: dropped.policy,
            ratelimit: dropped.ratelimit,
        },
        feed_entries: status.feed_entries,
        feeds: status
            .feeds
            .into_iter()
            .map(|(name, f)| FeedInfo {
                name,
                entries: f.entries,
                updated: f.updated_unix,
                error: f.error,
            })
            .collect(),
        bans,
    })
}

#[utoipa::path(get, path = "/api/v1/firewall/recent", tag = "firewall",
    responses((status = 200, body = Vec<RecentDropInfo>)))]
pub(crate) async fn recent(State(state): State<ApiState>) -> Json<Vec<RecentDropInfo>> {
    let drops = state.perimeter.recent().await;
    Json(
        drops
            .into_iter()
            .take(500)
            .map(|d| RecentDropInfo {
                source: d.source,
                protocol: d.protocol,
                port: d.port,
                reason: serde_json::to_value(d.reason)
                    .ok()
                    .and_then(|v| v.as_str().map(str::to_owned))
                    .unwrap_or_default(),
                packets: d.packets,
                last_seen_secs: d.last_seen_secs,
            })
            .collect(),
    )
}

/// Confirms a firewall change. Must arrive over a connection opened after
/// the change was installed: that is the proof the new rules let you in.
#[utoipa::path(post, path = "/api/v1/firewall/confirm", tag = "firewall",
    responses((status = 204), (status = 404, body = crate::routes::Problem),
              (status = 409, body = crate::routes::Problem)))]
pub(crate) async fn confirm(
    State(state): State<ApiState>,
    accepted: Option<Extension<Accepted>>,
) -> Result<StatusCode, ApiError> {
    use hearthgate_firewall::ConfirmError;
    // Requests served without an accept time (tests) count as new.
    let accepted = accepted.map_or_else(Instant::now, |Extension(a)| a.0);
    match state.perimeter.confirm(accepted) {
        Ok(_) => Ok(StatusCode::NO_CONTENT),
        Err(e @ ConfirmError::NothingPending) => {
            Err(ApiError::new(StatusCode::NOT_FOUND, e.to_string()))
        }
        Err(e @ ConfirmError::OldConnection) => {
            Err(ApiError::new(StatusCode::CONFLICT, e.to_string()))
        }
    }
}

#[utoipa::path(post, path = "/api/v1/firewall/bans", tag = "firewall", request_body = BanRequest,
    responses((status = 204), (status = 409, body = crate::routes::Problem),
              (status = 422, body = crate::routes::Problem)))]
pub(crate) async fn ban(
    State(state): State<ApiState>,
    Json(req): Json<BanRequest>,
) -> Result<StatusCode, ApiError> {
    let firewall = state.perimeter.installed().map(|i| i.firewall);
    let Some(firewall) = firewall else {
        return Err(ApiError::new(
            StatusCode::CONFLICT,
            "turn the firewall on to ban addresses",
        ));
    };
    let net = ipnet::IpNet::from(req.address);
    if hearthgate_model::is_special_net(&net) {
        return Err(ApiError::new(
            StatusCode::UNPROCESSABLE_ENTITY,
            "private and local addresses cannot be banned; use a deny rule",
        ));
    }
    let time = match req.duration.as_deref() {
        None | Some("") => firewall.auto_ban.ban_time,
        Some(text) => humantime::parse_duration(text).map_err(|e| {
            ApiError::new(StatusCode::UNPROCESSABLE_ENTITY, format!("duration: {e}"))
        })?,
    };
    if time.is_zero() || time > Duration::from_secs(365 * 24 * 3600) {
        return Err(ApiError::new(
            StatusCode::UNPROCESSABLE_ENTITY,
            "choose a duration between one second and a year",
        ));
    }
    state
        .perimeter
        .ban(req.address, time)
        .await
        .map_err(|e| ApiError::new(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
    Ok(StatusCode::NO_CONTENT)
}

#[utoipa::path(delete, path = "/api/v1/firewall/bans/{address}", tag = "firewall",
    params(("address" = String, Path)),
    responses((status = 204), (status = 404, body = crate::routes::Problem)))]
pub(crate) async fn unban(
    State(state): State<ApiState>,
    Path(address): Path<String>,
) -> Result<StatusCode, ApiError> {
    let ip: IpAddr = address
        .parse()
        .map_err(|_| ApiError::new(StatusCode::NOT_FOUND, "not an address"))?;
    if !state.perimeter.bans().await.iter().any(|(b, _)| *b == ip) {
        return Err(ApiError::new(
            StatusCode::NOT_FOUND,
            "that address is not banned",
        ));
    }
    state
        .perimeter
        .unban(ip)
        .await
        .map_err(|e| ApiError::new(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
    Ok(StatusCode::NO_CONTENT)
}

/// While a firewall change waits for confirmation, every response closes
/// its connection, so the browser's next request (the confirmation) has to
/// open a new one through the new rules.
pub(crate) async fn close_while_pending(
    State(state): State<ApiState>,
    request: Request,
    next: Next,
) -> Response {
    let pending = state.perimeter.pending();
    // A connection from before the change does not show that the new rules
    // let this client in. Browsers keep connections open and send every
    // request over one HTTP/2 connection, where `Connection: close` means
    // nothing, so end old connections: the next request opens a new one.
    if let Some(p) = &pending {
        let old = request
            .extensions()
            .get::<Accepted>()
            .is_some_and(|a| a.0 <= p.applied_at);
        if old {
            if let Some(close) = request.extensions().get::<CloseConnection>() {
                close.0.cancel();
            }
        }
    }
    let pending = pending.is_some();
    let mut response = next.run(request).await;
    if pending
        && !response
            .headers()
            .get(header::CONTENT_TYPE)
            .is_some_and(|v| v.as_bytes().starts_with(b"text/event-stream"))
    {
        response
            .headers_mut()
            .insert(header::CONNECTION, HeaderValue::from_static("close"));
    }
    response
}

/// Router port forwarding.
#[derive(Debug, Serialize, ToSchema)]
pub struct RouterInfo {
    pub enabled: bool,
    /// `upnp` or `nat-pmp`.
    pub method: Option<String>,
    pub router: Option<String>,
    #[schema(value_type = Option<String>)]
    pub local_ip: Option<IpAddr>,
    #[schema(value_type = Option<String>)]
    pub public_ip: Option<IpAddr>,
    /// The router's public address is private: another NAT is in front.
    pub double_nat: bool,
    pub mappings: Vec<MappingInfo>,
    pub error: Option<String>,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct MappingInfo {
    pub port: u16,
    /// `tcp` or `udp`.
    pub protocol: String,
    pub active: bool,
    pub error: Option<String>,
}

#[utoipa::path(get, path = "/api/v1/router", tag = "firewall",
    responses((status = 200, body = RouterInfo)))]
pub(crate) async fn router(State(state): State<ApiState>) -> Json<RouterInfo> {
    let s = state.router.status();
    Json(RouterInfo {
        enabled: s.enabled,
        method: s.method.map(str::to_owned),
        router: s.router,
        local_ip: s.local_ip,
        public_ip: s.public_ip,
        double_nat: s.double_nat,
        mappings: s
            .mappings
            .into_iter()
            .map(|m| MappingInfo {
                port: m.port,
                protocol: match m.protocol {
                    hearthgate_upnp::Protocol::Tcp => "tcp".into(),
                    hearthgate_upnp::Protocol::Udp => "udp".into(),
                },
                active: m.active,
                error: m.error,
            })
            .collect(),
        error: s.error,
    })
}
