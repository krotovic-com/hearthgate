//! API endpoints and the OpenAPI document.

use std::sync::Arc;
use std::time::UNIX_EPOCH;

use axum::extract::{Path, Query, State};
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

#[derive(OpenApi)]
#[openapi(
    info(
        title = "Hearthgate admin API",
        description = "Manage a Hearthgate gateway. Every request except this document needs \
                       `Authorization: Bearer <admin token>` (see `hearthgate admin-token`).",
        license(name = "MIT")
    ),
    paths(
        status, get_state, put_state, validate_state, list_revisions, get_revision,
        rollback, certificates
    ),
    components(schemas(
        StatusResponse, ListenerStatus, UpstreamStatus, StateResponse, ApplyRequest,
        ApplyResponse, ValidateResponse, Revision, RevisionInfo, CertificateInfo, Problem,
        DesiredState
    )),
    modifiers(&Security),
    tags((name = "hearthgate"))
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
    Router::new()
        .route("/api/v1/status", get(status))
        .route("/api/v1/state", get(get_state).put(put_state))
        .route("/api/v1/state/validate", post(validate_state))
        .route("/api/v1/revisions", get(list_revisions))
        .route("/api/v1/revisions/{id}", get(get_revision))
        .route("/api/v1/revisions/{id}/rollback", post(rollback))
        .route("/api/v1/certificates", get(certificates))
        .route("/api/v1/openapi.json", get(openapi))
        .route("/metrics", get(metrics))
        .layer(axum::middleware::from_fn_with_state(
            state.clone(),
            crate::auth::require_token,
        ))
        .with_state(state)
}

/// An error with a message and, for validation failures, every problem.
#[derive(Debug, Serialize, ToSchema)]
pub struct Problem {
    pub error: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub problems: Vec<String>,
}

struct ApiError(StatusCode, Problem);

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

type ApiResult<T> = Result<Json<T>, ApiError>;

async fn blocking<T, F>(f: F) -> Result<T, ApiError>
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
}

/// Gateway status: listeners, upstream health and problems.
#[utoipa::path(get, path = "/api/v1/status", responses((status = 200, body = StatusResponse)))]
async fn status(State(state): State<ApiState>) -> ApiResult<StatusResponse> {
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
        })
        .collect();
    Ok(Json(StatusResponse {
        version: crate_version(),
        revision: runtime.revision,
        latest_revision: latest,
        listeners,
        upstreams,
        problems: runtime.problems,
    }))
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
    Json(req): Json<ApplyRequest>,
) -> ApiResult<ApplyResponse> {
    let store = Arc::clone(&state.store);
    let message = req.message.unwrap_or_else(|| "Change via API".to_owned());
    let result = blocking(move || store.apply(&req.state.normalized(), "api", &message)).await?;
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
async fn rollback(State(state): State<ApiState>, Path(id): Path<i64>) -> ApiResult<ApplyResponse> {
    let store = Arc::clone(&state.store);
    let result = blocking(move || store.rollback(id, "api")).await?;
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
