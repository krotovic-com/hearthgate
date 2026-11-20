//! Managed containers and stacks: status, restarts, updates, logs and
//! stats. Creating, changing and removing them goes through the desired
//! state like everything else.

use std::sync::Arc;

use axum::Json;
use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use hearthgate_docker::{Error, ImageUpdate, LogLine, Stats, Status};
use hearthgate_model::DesiredState;
use serde::{Deserialize, Serialize};
use utoipa::{IntoParams, ToSchema};

use crate::ApiState;
use crate::routes::{ApiError, ApiResult, blocking};

/// Managed Docker as it runs.
#[derive(Debug, Serialize, ToSchema)]
pub struct DockerInfo {
    /// Managing containers is on (Docker is on, and `docker.manage` in the
    /// bootstrap file).
    pub enabled: bool,
    /// Containers, stacks and problems, as Docker reports them.
    #[serde(flatten)]
    #[schema(value_type = Object)]
    pub status: Status,
}

#[derive(Debug, Deserialize, IntoParams)]
pub struct LogQuery {
    /// How many lines, at most 5000 (default 200).
    pub tail: Option<usize>,
    /// Only lines containing this text (any case).
    pub search: Option<String>,
}

fn error(e: &Error) -> ApiError {
    let status = match e {
        Error::Unavailable => StatusCode::SERVICE_UNAVAILABLE,
        Error::NoSuchContainer(_) | Error::NoSuchStack(_) => StatusCode::NOT_FOUND,
        Error::Docker(_) | Error::Other(_) => StatusCode::BAD_GATEWAY,
    };
    ApiError::new(status, e.to_string())
}

async fn current_state(state: &ApiState) -> Result<DesiredState, ApiError> {
    let store = Arc::clone(&state.store);
    blocking(move || store.current_state()).await
}

#[utoipa::path(get, path = "/api/v1/docker", tag = "docker", responses((status = 200, body = DockerInfo)))]
pub(crate) async fn info(State(state): State<ApiState>) -> ApiResult<DockerInfo> {
    Ok(Json(DockerInfo {
        enabled: state.docker.enabled(),
        status: state.docker.status(),
    }))
}

#[utoipa::path(post, path = "/api/v1/docker/containers/{name}/restart", tag = "docker",
    params(("name" = String, Path)),
    responses((status = 204), (status = 404, body = crate::routes::Problem)))]
pub(crate) async fn restart(
    State(state): State<ApiState>,
    Path(name): Path<String>,
) -> Result<StatusCode, ApiError> {
    state.docker.restart(&name).await.map_err(|e| error(&e))?;
    Ok(StatusCode::NO_CONTENT)
}

#[utoipa::path(post, path = "/api/v1/docker/containers/{name}/update", tag = "docker",
    description = "Pull the container's image and recreate it if the image changed.",
    params(("name" = String, Path)),
    responses((status = 204), (status = 404, body = crate::routes::Problem)))]
pub(crate) async fn update_container(
    State(state): State<ApiState>,
    Path(name): Path<String>,
) -> Result<StatusCode, ApiError> {
    let desired = current_state(&state).await?;
    state
        .docker
        .update_container(&desired, &name)
        .await
        .map_err(|e| error(&e))?;
    Ok(StatusCode::NO_CONTENT)
}

#[utoipa::path(post, path = "/api/v1/docker/stacks/{name}/update", tag = "docker",
    description = "Pull the stack's images and bring it up again.",
    params(("name" = String, Path)),
    responses((status = 204), (status = 404, body = crate::routes::Problem)))]
pub(crate) async fn update_stack(
    State(state): State<ApiState>,
    Path(name): Path<String>,
) -> Result<StatusCode, ApiError> {
    let desired = current_state(&state).await?;
    state
        .docker
        .update_stack(&desired, &name)
        .await
        .map_err(|e| error(&e))?;
    Ok(StatusCode::NO_CONTENT)
}

#[utoipa::path(get, path = "/api/v1/docker/containers/{name}/logs", tag = "docker",
    params(("name" = String, Path), LogQuery),
    responses((status = 200, body = Vec<Object>), (status = 404, body = crate::routes::Problem)))]
pub(crate) async fn logs(
    State(state): State<ApiState>,
    Path(name): Path<String>,
    Query(query): Query<LogQuery>,
) -> ApiResult<Vec<LogLine>> {
    let tail = query.tail.unwrap_or(200).clamp(1, 5000);
    let search = query.search.as_deref().filter(|s| !s.is_empty());
    let lines = state
        .docker
        .logs(&name, tail, search)
        .await
        .map_err(|e| error(&e))?;
    Ok(Json(lines))
}

#[utoipa::path(get, path = "/api/v1/docker/containers/{name}/stats", tag = "docker",
    params(("name" = String, Path)),
    responses((status = 200, body = Object), (status = 404, body = crate::routes::Problem)))]
pub(crate) async fn stats(
    State(state): State<ApiState>,
    Path(name): Path<String>,
) -> ApiResult<Stats> {
    Ok(Json(
        state.docker.stats(&name).await.map_err(|e| error(&e))?,
    ))
}

#[utoipa::path(get, path = "/api/v1/docker/updates", tag = "docker",
    description = "Ask the registries whether newer images exist. Changes nothing.",
    responses((status = 200, body = Vec<Object>)))]
pub(crate) async fn updates(State(state): State<ApiState>) -> ApiResult<Vec<ImageUpdate>> {
    Ok(Json(
        state.docker.check_updates().await.map_err(|e| error(&e))?,
    ))
}
