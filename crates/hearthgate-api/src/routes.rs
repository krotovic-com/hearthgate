//! API endpoints and the OpenAPI document.

use std::sync::Arc;
use std::time::UNIX_EPOCH;

use axum::extract::{Extension, Path, Query, State};
use axum::http::{StatusCode, header};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use hearthgate_acme::CertSource;
use hearthgate_model::DesiredState;
use hearthgate_proxy::metrics::CertMetric;
use hearthgate_store::{Applied, RevisionMeta, StoreError};
use serde::{Deserialize, Serialize};
use utoipa::{IntoParams, OpenApi, ToSchema};

use crate::ApiState;
use crate::auth::Principal;

#[derive(OpenApi)]
#[openapi(
    info(
        title = "Hearthgate admin API",
        description = "Manage a Hearthgate gateway. Requests need either a dashboard session \
                       (cookie plus `X-CSRF-Token` for changes) or `Authorization: Bearer <token>` \
                       with an API token or the admin token (`hearthgate admin-token`).",
        license(name = "MIT")
    ),
    paths(
        status, get_state, put_state, validate_state, list_revisions, get_revision,
        rollback, certificates, preview_state, revision_diff, export_state,
        crate::live::logs, crate::live::traffic, crate::live::events,
        crate::access::get_access, crate::access::put_access,
        crate::account::session_info, crate::account::setup, crate::account::login,
        crate::account::logout, crate::account::change_password, crate::account::totp_begin,
        crate::account::totp_enable, crate::account::totp_disable, crate::account::list_tokens,
        crate::account::create_token, crate::account::delete_token,
        crate::firewall::info, crate::firewall::recent, crate::firewall::confirm,
        crate::firewall::ban, crate::firewall::unban, crate::firewall::router
    ),
    components(schemas(
        StatusResponse, ListenerStatus, UpstreamStatus, StateResponse, ApplyRequest,
        ApplyResponse, ValidateResponse, Revision, RevisionInfo, CertificateInfo, Problem,
        DesiredState, crate::account::SessionInfo, crate::account::UserInfo,
        crate::account::SetupRequest, crate::account::LoginRequest, crate::account::LoginProblem,
        crate::account::PasswordRequest, crate::account::TotpSecret,
        crate::account::TotpEnableRequest, crate::account::PasswordConfirm,
        crate::account::ApiTokenInfo, crate::account::NewToken, crate::account::CreatedToken,
        HostInfo, PreviewResponse, DiffResponse, crate::live::Rate, crate::live::TrafficBucket,
        crate::live::LogLine, crate::access::AccessSettings, crate::access::AccessMode,
        crate::access::AccessInfo, crate::access::Resolved, FirewallSummary,
        crate::firewall::FirewallInfo, crate::firewall::PendingInfo,
        crate::firewall::DroppedInfo, crate::firewall::FeedInfo, crate::firewall::BanInfo,
        crate::firewall::RecentDropInfo, crate::firewall::BanRequest,
        crate::firewall::RouterInfo, crate::firewall::MappingInfo
    )),
    modifiers(&Security),
    tags(
        (name = "hearthgate"),
        (name = "auth", description = "Dashboard login and API tokens"),
        (name = "settings", description = "Settings kept outside the desired state"),
        (name = "firewall", description = "Firewall status, bans and change confirmation")
    )
)]
pub struct ApiDoc;

struct Security;
impl utoipa::Modify for Security {
    fn modify(&self, openapi: &mut utoipa::openapi::OpenApi) {
        use utoipa::openapi::security::{HttpAuthScheme, HttpBuilder, SecurityScheme};
        let components = openapi.components.get_or_insert_with(Default::default);
        components.add_security_scheme(
            "token",
            SecurityScheme::Http(HttpBuilder::new().scheme(HttpAuthScheme::Bearer).build()),
        );
        openapi.security = Some(vec![utoipa::openapi::security::SecurityRequirement::new(
            "token",
            Vec::<String>::new(),
        )]);
    }
}

pub fn router(state: ApiState) -> Router {
    use crate::account;
    Router::new()
        .route("/api/v1/auth/session", get(account::session_info))
        .route("/api/v1/auth/setup", post(account::setup))
        .route("/api/v1/auth/login", post(account::login))
        .route("/api/v1/auth/logout", post(account::logout))
        .route("/api/v1/auth/password", post(account::change_password))
        .route("/api/v1/auth/totp/begin", post(account::totp_begin))
        .route("/api/v1/auth/totp/enable", post(account::totp_enable))
        .route("/api/v1/auth/totp/disable", post(account::totp_disable))
        .route(
            "/api/v1/tokens",
            get(account::list_tokens).post(account::create_token),
        )
        .route(
            "/api/v1/tokens/{id}",
            axum::routing::delete(account::delete_token),
        )
        .route("/api/v1/status", get(status))
        .route("/api/v1/state", get(get_state).put(put_state))
        .route("/api/v1/state/validate", post(validate_state))
        .route("/api/v1/state/preview", post(preview_state))
        .route("/api/v1/state/export", get(export_state))
        .route("/api/v1/revisions", get(list_revisions))
        .route("/api/v1/revisions/{id}", get(get_revision))
        .route("/api/v1/revisions/{id}/diff", get(revision_diff))
        .route("/api/v1/logs", get(crate::live::logs))
        .route("/api/v1/traffic", get(crate::live::traffic))
        .route("/api/v1/events", get(crate::live::events))
        .route(
            "/api/v1/settings/admin-access",
            get(crate::access::get_access).put(crate::access::put_access),
        )
        .route("/api/v1/firewall", get(crate::firewall::info))
        .route("/api/v1/router", get(crate::firewall::router))
        .route("/api/v1/firewall/recent", get(crate::firewall::recent))
        .route("/api/v1/firewall/confirm", post(crate::firewall::confirm))
        .route("/api/v1/firewall/bans", post(crate::firewall::ban))
        .route(
            "/api/v1/firewall/bans/{address}",
            axum::routing::delete(crate::firewall::unban),
        )
        .route("/api/v1/revisions/{id}/rollback", post(rollback))
        .route("/api/v1/certificates", get(certificates))
        .route("/api/v1/openapi.json", get(openapi))
        .route("/metrics", get(metrics))
        .route("/api/{*rest}", axum::routing::any(api_not_found))
        .fallback(crate::ui::serve)
        .layer(axum::middleware::from_fn_with_state(
            state.clone(),
            crate::auth::authenticate,
        ))
        .layer(axum::middleware::from_fn(crate::ui::security_headers))
        .layer(axum::middleware::from_fn_with_state(
            state.clone(),
            crate::firewall::close_while_pending,
        ))
        .with_state(state)
}

async fn api_not_found() -> ApiError {
    ApiError::new(StatusCode::NOT_FOUND, "no such API endpoint")
}

/// An error with a message and, for validation failures, every problem.
#[derive(Debug, Serialize, ToSchema)]
pub struct Problem {
    pub error: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub problems: Vec<String>,
}

pub(crate) struct ApiError(pub StatusCode, pub Problem);

impl ApiError {
    pub(crate) fn new(status: StatusCode, message: impl Into<String>) -> Self {
        Self(
            status,
            Problem {
                error: message.into(),
                problems: Vec::new(),
            },
        )
    }
}

impl ApiError {
    pub(crate) fn with_problems(
        status: StatusCode,
        message: impl Into<String>,
        problems: Vec<String>,
    ) -> Self {
        Self(
            status,
            Problem {
                error: message.into(),
                problems,
            },
        )
    }
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        (self.0, Json(self.1)).into_response()
    }
}

impl From<StoreError> for ApiError {
    fn from(e: StoreError) -> Self {
        match e {
            StoreError::Invalid(errors) => Self(
                StatusCode::UNPROCESSABLE_ENTITY,
                Problem {
                    error: "the desired state is not valid".to_owned(),
                    problems: errors.0.iter().map(ToString::to_string).collect(),
                },
            ),
            StoreError::NoSuchRevision(id) => Self(
                StatusCode::NOT_FOUND,
                Problem {
                    error: format!("revision {id} does not exist"),
                    problems: Vec::new(),
                },
            ),
            other => Self(
                StatusCode::INTERNAL_SERVER_ERROR,
                Problem {
                    error: other.to_string(),
                    problems: Vec::new(),
                },
            ),
        }
    }
}

pub(crate) type ApiResult<T> = Result<Json<T>, ApiError>;

pub(crate) async fn blocking<T, F>(f: F) -> Result<T, ApiError>
where
    F: FnOnce() -> Result<T, StoreError> + Send + 'static,
    T: Send + 'static,
{
    tokio::task::spawn_blocking(f)
        .await
        .map_err(|e| {
            ApiError(
                StatusCode::INTERNAL_SERVER_ERROR,
                Problem {
                    error: e.to_string(),
                    problems: Vec::new(),
                },
            )
        })?
        .map_err(ApiError::from)
}

/// A revision without its content.
#[derive(Debug, Serialize, ToSchema)]
pub struct RevisionInfo {
    pub id: i64,
    /// RFC 3339, UTC.
    pub created_at: String,
    pub author: String,
    pub message: String,
    /// SHA-256 of the canonical state.
    pub hash: String,
    pub rollback_of: Option<i64>,
}

impl From<RevisionMeta> for RevisionInfo {
    fn from(m: RevisionMeta) -> Self {
        Self {
            id: m.id,
            created_at: m.created_at,
            author: m.author,
            message: m.message,
            hash: m.hash,
            rollback_of: m.rollback_of,
        }
    }
}

/// A revision with its content.
#[derive(Debug, Serialize, ToSchema)]
pub struct Revision {
    #[serde(flatten)]
    pub info: RevisionInfo,
    pub state: DesiredState,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct ListenerStatus {
    /// For example `[::]:443/tcp`.
    pub socket: String,
    pub kind: String,
    pub open_connections: usize,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct UpstreamStatus {
    pub service: String,
    pub port: u16,
    /// Current address, or `unresolved`.
    pub endpoint: String,
    /// Result of the active health check.
    pub healthy: bool,
    /// Taken out after repeated connection failures.
    pub ejected: bool,
    /// Open connections or in-flight requests.
    pub active: usize,
    /// Traffic over the last few seconds.
    pub rate: crate::live::Rate,
}

/// The machine Hearthgate runs on.
#[derive(Debug, Serialize, ToSchema)]
pub struct HostInfo {
    pub hostname: String,
    /// Operating system, for example `Debian GNU/Linux 13 (trixie)`.
    pub os: Option<String>,
    /// Seconds since Hearthgate started.
    pub uptime: u64,
}

fn host_info(state: &ApiState) -> HostInfo {
    static OS: std::sync::OnceLock<Option<String>> = std::sync::OnceLock::new();
    let os = OS.get_or_init(|| {
        let text = std::fs::read_to_string("/etc/os-release").ok()?;
        text.lines()
            .find_map(|l| l.strip_prefix("PRETTY_NAME="))
            .map(|v| v.trim_matches('"').to_owned())
    });
    HostInfo {
        hostname: crate::tls::hostname().unwrap_or_else(|| "localhost".to_owned()),
        os: os.clone(),
        uptime: state.live.uptime(),
    }
}

#[derive(Debug, Serialize, ToSchema)]
pub struct StatusResponse {
    pub version: String,
    /// Revision running now.
    pub revision: Option<i64>,
    /// Newest stored revision (differs from `revision` for a moment after
    /// a change).
    pub latest_revision: Option<i64>,
    pub listeners: Vec<ListenerStatus>,
    pub upstreams: Vec<UpstreamStatus>,
    /// Things that need attention.
    pub problems: Vec<String>,
    pub host: HostInfo,
    /// Whether the firewall is installed, and a change waiting for
    /// confirmation.
    pub firewall: FirewallSummary,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct FirewallSummary {
    pub installed: bool,
    pub pending: Option<crate::firewall::PendingInfo>,
}

/// Gateway status: listeners, upstream health and problems.
#[utoipa::path(get, path = "/api/v1/status", responses((status = 200, body = StatusResponse)))]
async fn status(State(state): State<ApiState>) -> ApiResult<StatusResponse> {
    Ok(Json(status_snapshot(&state).await?))
}

/// The body of `/api/v1/status`.
pub(crate) async fn status_snapshot(state: &ApiState) -> Result<StatusResponse, ApiError> {
    let store = Arc::clone(&state.store);
    let latest = blocking(move || store.current_id()).await?;
    let runtime = state
        .runtime
        .read()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .clone();
    let listeners = state
        .gateway
        .status()
        .await
        .into_iter()
        .map(|s| ListenerStatus {
            socket: s.key.to_string(),
            kind: format!("{:?}", s.kind).to_ascii_lowercase(),
            open_connections: s.open_connections,
        })
        .collect();
    let upstreams = state
        .gateway
        .upstreams()
        .all()
        .iter()
        .map(|u| UpstreamStatus {
            service: u.service.clone(),
            port: u.port,
            endpoint: u.endpoint().to_string(),
            healthy: u.healthy(),
            ejected: u.ejected(),
            active: u.active(),
            rate: state.live.rate(&u.service, u.port),
        })
        .collect();
    let fw = state.perimeter.status();
    Ok(StatusResponse {
        firewall: FirewallSummary {
            installed: fw.installed,
            pending: fw
                .pending
                .map(|(revision, seconds_left)| crate::firewall::PendingInfo {
                    revision,
                    seconds_left,
                }),
        },
        version: crate_version(),
        revision: runtime.revision,
        latest_revision: latest,
        listeners,
        upstreams,
        problems: runtime.problems,
        host: host_info(state),
    })
}

fn crate_version() -> String {
    option_env!("HEARTHGATE_VERSION")
        .unwrap_or(env!("CARGO_PKG_VERSION"))
        .to_owned()
}

#[derive(Debug, Serialize, ToSchema)]
pub struct StateResponse {
    /// The current revision, or none before the first change.
    pub revision: Option<RevisionInfo>,
    pub state: DesiredState,
}

/// The current desired state.
#[utoipa::path(get, path = "/api/v1/state", responses((status = 200, body = StateResponse)))]
async fn get_state(State(state): State<ApiState>) -> ApiResult<StateResponse> {
    let store = Arc::clone(&state.store);
    let current = blocking(move || store.current()).await?;
    Ok(Json(match current {
        Some(r) => StateResponse {
            revision: Some(r.meta.into()),
            state: r.state,
        },
        None => StateResponse {
            revision: None,
            state: DesiredState::default(),
        },
    }))
}

#[derive(Debug, Deserialize, ToSchema)]
pub struct ApplyRequest {
    pub state: DesiredState,
    /// Shown in the revision history.
    pub message: Option<String>,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct ApplyResponse {
    /// False when the state equals the current revision.
    pub changed: bool,
    pub revision: RevisionInfo,
}

fn applied(state: &ApiState, a: Applied) -> ApplyResponse {
    let changed = matches!(a, Applied::Created(_));
    if changed {
        state.changed.notify_one();
    }
    let meta = match a {
        Applied::Created(m) | Applied::Unchanged(m) => m,
    };
    ApplyResponse {
        changed,
        revision: meta.into(),
    }
}

/// Store a new desired state as a revision and apply it.
#[utoipa::path(
    put, path = "/api/v1/state", request_body = ApplyRequest,
    responses(
        (status = 200, body = ApplyResponse),
        (status = 422, description = "The state is not valid", body = Problem),
    )
)]
async fn put_state(
    State(state): State<ApiState>,
    Extension(principal): Extension<Principal>,
    Json(req): Json<ApplyRequest>,
) -> ApiResult<ApplyResponse> {
    let store = Arc::clone(&state.store);
    let message = req.message.unwrap_or_else(|| "Change via API".to_owned());
    let author = principal.author();
    let result = blocking(move || store.apply(&req.state.normalized(), &author, &message)).await?;
    Ok(Json(applied(&state, result)))
}

#[derive(Debug, Serialize, ToSchema)]
pub struct ValidateResponse {
    pub valid: bool,
    pub problems: Vec<String>,
}

/// Check a desired state without storing it.
#[utoipa::path(
    post, path = "/api/v1/state/validate", request_body = DesiredState,
    responses((status = 200, body = ValidateResponse))
)]
async fn validate_state(Json(desired): Json<DesiredState>) -> Json<ValidateResponse> {
    let problems = match desired.normalized().validate() {
        Ok(()) => Vec::new(),
        Err(e) => e.0.iter().map(ToString::to_string).collect(),
    };
    Json(ValidateResponse {
        valid: problems.is_empty(),
        problems,
    })
}

#[derive(Debug, Serialize, ToSchema)]
pub struct PreviewResponse {
    pub valid: bool,
    pub problems: Vec<String>,
    /// False when the state equals the current revision.
    pub changed: bool,
    /// Unified diff from the current revision, in TOML form.
    pub diff: String,
}

/// Check a draft and show what applying it would change.
#[utoipa::path(
    post, path = "/api/v1/state/preview", request_body = DesiredState,
    responses((status = 200, body = PreviewResponse))
)]
async fn preview_state(
    State(state): State<ApiState>,
    Json(desired): Json<DesiredState>,
) -> ApiResult<PreviewResponse> {
    let desired = desired.normalized();
    let problems = match desired.validate() {
        Ok(()) => Vec::new(),
        Err(e) => e.0.iter().map(ToString::to_string).collect(),
    };
    let store = Arc::clone(&state.store);
    let current = blocking(move || store.current_state()).await?;
    let diff = current.diff(&desired);
    Ok(Json(PreviewResponse {
        valid: problems.is_empty(),
        problems,
        changed: !diff.is_empty(),
        diff,
    }))
}

/// The current desired state as a TOML file, for `hearthgate import`.
#[utoipa::path(
    get, path = "/api/v1/state/export",
    responses((status = 200, description = "TOML", content_type = "application/toml", body = String))
)]
async fn export_state(State(state): State<ApiState>) -> Result<Response, ApiError> {
    let store = Arc::clone(&state.store);
    let (id, desired) = blocking(move || {
        Ok(match store.current()? {
            Some(r) => (r.meta.id, r.state),
            None => (0, DesiredState::default()),
        })
    })
    .await?;
    Ok((
        [
            (
                header::CONTENT_TYPE,
                "application/toml; charset=utf-8".to_owned(),
            ),
            (
                header::CONTENT_DISPOSITION,
                format!("attachment; filename=\"hearthgate-revision-{id}.toml\""),
            ),
        ],
        desired.to_toml(),
    )
        .into_response())
}

#[derive(Debug, Serialize, ToSchema)]
pub struct DiffResponse {
    /// The revision compared against (the one before), if any.
    pub previous: Option<i64>,
    /// Unified diff in TOML form.
    pub diff: String,
}

/// What a revision changed compared with the one before it.
#[utoipa::path(
    get, path = "/api/v1/revisions/{id}/diff", params(("id" = i64, Path, description = "Revision number")),
    responses((status = 200, body = DiffResponse), (status = 404, body = Problem))
)]
async fn revision_diff(
    State(state): State<ApiState>,
    Path(id): Path<i64>,
) -> ApiResult<DiffResponse> {
    let store = Arc::clone(&state.store);
    let (previous, old, new) = blocking(move || {
        let new = store.revision(id)?.ok_or(StoreError::NoSuchRevision(id))?;
        let mut prev = id - 1;
        while prev > 0 {
            if let Some(r) = store.revision(prev)? {
                return Ok((Some(prev), r.state, new.state));
            }
            prev -= 1;
        }
        Ok((None, DesiredState::default(), new.state))
    })
    .await?;
    Ok(Json(DiffResponse {
        previous,
        diff: old.diff(&new),
    }))
}

#[derive(Debug, Deserialize, IntoParams)]
pub struct ListQuery {
    /// How many revisions to return, newest first (default 50).
    pub limit: Option<usize>,
}

/// Revision history, newest first.
#[utoipa::path(
    get, path = "/api/v1/revisions", params(ListQuery),
    responses((status = 200, body = Vec<RevisionInfo>))
)]
async fn list_revisions(
    State(state): State<ApiState>,
    Query(q): Query<ListQuery>,
) -> ApiResult<Vec<RevisionInfo>> {
    let store = Arc::clone(&state.store);
    let limit = q.limit.unwrap_or(50).min(1000);
    let list = blocking(move || store.list(limit)).await?;
    Ok(Json(list.into_iter().map(Into::into).collect()))
}

/// One revision with its content.
#[utoipa::path(
    get, path = "/api/v1/revisions/{id}", params(("id" = i64, Path, description = "Revision number")),
    responses((status = 200, body = Revision), (status = 404, body = Problem))
)]
async fn get_revision(State(state): State<ApiState>, Path(id): Path<i64>) -> ApiResult<Revision> {
    let store = Arc::clone(&state.store);
    let r = blocking(move || store.revision(id)?.ok_or(StoreError::NoSuchRevision(id))).await?;
    Ok(Json(Revision {
        info: r.meta.into(),
        state: r.state,
    }))
}

/// Make an older revision current again (stored as a new revision).
#[utoipa::path(
    post, path = "/api/v1/revisions/{id}/rollback", params(("id" = i64, Path, description = "Revision number")),
    responses((status = 200, body = ApplyResponse), (status = 404, body = Problem))
)]
async fn rollback(
    State(state): State<ApiState>,
    Extension(principal): Extension<Principal>,
    Path(id): Path<i64>,
) -> ApiResult<ApplyResponse> {
    let store = Arc::clone(&state.store);
    let author = principal.author();
    let result = blocking(move || store.rollback(id, &author)).await?;
    Ok(Json(applied(&state, result)))
}

#[derive(Debug, Serialize, ToSchema)]
pub struct CertificateInfo {
    pub name: String,
    /// `manual` or `automatic`.
    pub source: String,
    pub domains: Vec<String>,
    /// Expiry in seconds since the Unix epoch.
    pub not_after: u64,
    pub days_left: u64,
    pub issuer: String,
}

fn cert_list(state: &ApiState) -> Result<Vec<CertificateInfo>, ApiError> {
    let list = state.certs.list().map_err(|e| {
        ApiError(
            StatusCode::INTERNAL_SERVER_ERROR,
            Problem {
                error: e.to_string(),
                problems: Vec::new(),
            },
        )
    })?;
    Ok(list
        .into_iter()
        .map(|c| CertificateInfo {
            source: match c.source {
                CertSource::Manual => "manual",
                CertSource::Acme => "automatic",
            }
            .to_owned(),
            not_after: c
                .not_after
                .duration_since(UNIX_EPOCH)
                .unwrap_or_default()
                .as_secs(),
            days_left: c.remaining().as_secs() / 86_400,
            name: c.name,
            domains: c.domains,
            issuer: c.issuer,
        })
        .collect())
}

/// Stored certificates.
#[utoipa::path(get, path = "/api/v1/certificates", responses((status = 200, body = Vec<CertificateInfo>)))]
async fn certificates(State(state): State<ApiState>) -> ApiResult<Vec<CertificateInfo>> {
    Ok(Json(cert_list(&state)?))
}

async fn openapi() -> Json<utoipa::openapi::OpenApi> {
    Json(ApiDoc::openapi())
}

async fn metrics(State(state): State<ApiState>) -> Response {
    let sockets = state.gateway.status().await;
    let certs: Vec<CertMetric> = cert_list(&state)
        .unwrap_or_default()
        .into_iter()
        .map(|c| CertMetric {
            source: if c.source == "manual" {
                "manual"
            } else {
                "automatic"
            },
            name: c.name,
            not_after: c.not_after,
        })
        .collect();
    let revision = state
        .runtime
        .read()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .revision;
    let body = hearthgate_proxy::metrics::render(
        &sockets,
        state.gateway.upstreams(),
        &state.gateway.local_responses(),
        &certs,
        revision,
    );
    (
        [(
            header::CONTENT_TYPE,
            "text/plain; version=0.0.4; charset=utf-8",
        )],
        body,
    )
        .into_response()
}
