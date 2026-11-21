//! Sending a test notification, to check the settings before applying
//! them.

use axum::Json;
use hearthgate_model::Notifications;
use serde::Serialize;
use utoipa::ToSchema;

use crate::routes::ApiResult;

/// The result of one target.
#[derive(Debug, Serialize, ToSchema)]
pub struct TestResult {
    /// Such as `webhook phone` or `email to me@example.com`.
    pub target: String,
    pub ok: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

#[utoipa::path(post, path = "/api/v1/notifications/test", tag = "settings",
    description = "Send a test message with these settings (usually the draft's), without storing them.",
    request_body = Notifications,
    responses((status = 200, body = Vec<TestResult>)))]
pub(crate) async fn test(Json(settings): Json<Notifications>) -> ApiResult<Vec<TestResult>> {
    let results = hearthgate_notify::send(&settings, &hearthgate_notify::test_event()).await;
    Ok(Json(
        results
            .into_iter()
            .map(|(target, r)| TestResult {
                target,
                ok: r.is_ok(),
                error: r.err(),
            })
            .collect(),
    ))
}
