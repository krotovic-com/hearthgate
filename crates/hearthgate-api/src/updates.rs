//! Updates of Hearthgate itself. Settings live in the desired state
//! (`[updates]`); these endpoints check, install, roll back and skip.
//!
//! An install stops this daemon: the dashboard keeps reading
//! `GET /api/v1/updates` (or the progress stream) and gets the root helper's
//! progress from the new daemon once it answers.

use std::convert::Infallible;
use std::time::Duration;

use axum::Json;
use axum::extract::State;
use axum::http::StatusCode;
use axum::response::sse::{Event, KeepAlive, Sse};
use futures_util::Stream;
use hearthgate_update::state::Job;
use hearthgate_update::{UpdateInfo, Version};
use serde::Deserialize;
use utoipa::ToSchema;

use crate::ApiState;
use crate::routes::{ApiError, ApiResult};

/// When an install starts.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Deserialize, ToSchema)]
#[serde(rename_all = "lowercase")]
pub enum When {
    /// Right away.
    #[default]
    Now,
    /// At the next install window.
    Window,
}

#[derive(Debug, Deserialize, ToSchema)]
pub struct InstallRequest {
    /// The version to install; must be the newest release.
    pub version: String,
    #[serde(default)]
    pub when: When,
}

#[derive(Debug, Deserialize, ToSchema)]
pub struct VersionRequest {
    pub version: String,
}

fn version(text: &str) -> Result<Version, ApiError> {
    Version::parse(text)
        .ok_or_else(|| ApiError::new(StatusCode::UNPROCESSABLE_ENTITY, format!("{text:?} is not a release version")))
}

fn conflict(e: String) -> ApiError {
    ApiError::new(StatusCode::CONFLICT, e)
}

#[utoipa::path(get, path = "/api/v1/updates", tag = "updates",
    responses((status = 200, body = UpdateInfo)))]
pub(crate) async fn info(State(state): State<ApiState>) -> ApiResult<UpdateInfo> {
    Ok(Json(state.updates.info()))
}

#[utoipa::path(post, path = "/api/v1/updates/check", tag = "updates",
    description = "Ask GitHub for releases now.",
    responses((status = 200, body = UpdateInfo), (status = 502, body = crate::routes::Problem)))]
pub(crate) async fn check(State(state): State<ApiState>) -> ApiResult<UpdateInfo> {
    state
        .updates
        .check()
        .await
        .map(Json)
        .map_err(|e| ApiError::new(StatusCode::BAD_GATEWAY, format!("cannot check for updates: {e}")))
}

#[utoipa::path(post, path = "/api/v1/updates/install", tag = "updates",
    description = "Download and verify the release, then hand it to the root upgrade helper, \
                   which backs up, installs, restarts Hearthgate and rolls back if it is not \
                   healthy within two minutes. With `when: window` it waits for the install \
                   window instead.",
    request_body = InstallRequest,
    responses((status = 200, body = UpdateInfo), (status = 409, body = crate::routes::Problem)))]
pub(crate) async fn install(
    State(state): State<ApiState>,
    Json(req): Json<InstallRequest>,
) -> ApiResult<UpdateInfo> {
    let v = version(&req.version)?;
    state.updates.install(v, req.when == When::Now).await.map(Json).map_err(conflict)
}

#[utoipa::path(delete, path = "/api/v1/updates/scheduled", tag = "updates",
    description = "Cancel an install waiting for the window.",
    responses((status = 200, body = UpdateInfo)))]
pub(crate) async fn unschedule(State(state): State<ApiState>) -> ApiResult<UpdateInfo> {
    Ok(Json(state.updates.unschedule()))
}

#[utoipa::path(post, path = "/api/v1/updates/rollback", tag = "updates",
    description = "Go back to the version kept from the last update. Current settings are \
                   kept unless that version cannot read them.",
    responses((status = 200, body = UpdateInfo), (status = 409, body = crate::routes::Problem)))]
pub(crate) async fn rollback(State(state): State<ApiState>) -> ApiResult<UpdateInfo> {
    state.updates.rollback().map(Json).map_err(conflict)
}

#[utoipa::path(post, path = "/api/v1/updates/skip", tag = "updates",
    description = "Stop offering a version, and never install it by itself.",
    request_body = VersionRequest,
    responses((status = 200, body = UpdateInfo)))]
pub(crate) async fn skip(
    State(state): State<ApiState>,
    Json(req): Json<VersionRequest>,
) -> ApiResult<UpdateInfo> {
    Ok(Json(state.updates.skip(version(&req.version)?)))
}

#[utoipa::path(post, path = "/api/v1/updates/unskip", tag = "updates",
    request_body = VersionRequest,
    responses((status = 200, body = UpdateInfo)))]
pub(crate) async fn unskip(
    State(state): State<ApiState>,
    Json(req): Json<VersionRequest>,
) -> ApiResult<UpdateInfo> {
    Ok(Json(state.updates.unskip(version(&req.version)?)))
}

#[utoipa::path(get, path = "/api/v1/updates/history", tag = "updates",
    description = "Past installs and rollbacks, newest first.",
    responses((status = 200, body = Vec<Job>)))]
pub(crate) async fn history(State(state): State<ApiState>) -> ApiResult<Vec<Job>> {
    Ok(Json(state.updates.history()))
}

/// The helper's job as Server-Sent Events: a `job` event whenever it
/// changes, checked every second. The stream ends when this daemon stops
/// for the restart; reconnect to follow the rest.
#[utoipa::path(get, path = "/api/v1/updates/progress", tag = "updates",
    responses((status = 200, description = "An event stream of `job` events", content_type = "text/event-stream")))]
pub(crate) async fn progress(
    State(state): State<ApiState>,
) -> Sse<impl Stream<Item = Result<Event, Infallible>>> {
    let mut tick = tokio::time::interval(Duration::from_secs(1));
    tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    let stream = futures_util::stream::unfold(
        (state, tick, None::<Job>),
        |(state, mut tick, mut last)| async move {
            loop {
                tick.tick().await;
                let job = state.updates.job();
                if job.is_some() && job != last {
                    let event = Event::default()
                        .event("job")
                        .json_data(&job)
                        .unwrap_or_else(|_| Event::default().comment("skipped"));
                    last = job;
                    return Some((Ok(event), (state, tick, last)));
                }
            }
        },
    );
    Sse::new(stream).keep_alive(KeepAlive::default())
}
