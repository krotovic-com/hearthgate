//! The optional AI assistant: reviews, troubleshooting and fixes.
//!
//! Every request to a provider goes through [`hearthgate_ai::Client`],
//! which refuses while AI is off, and every payload is redacted first.
//! `POST /api/v1/ai/preview` shows the exact body without sending it.

use std::fmt::Write as _;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use axum::Json;
use axum::extract::{Path, State};
use axum::http::StatusCode;
use hearthgate_ai::{Client, Finding, Outgoing, Redactor, Review, Scope, Severity};
use hearthgate_model::{Ai, AiProvider, DesiredState, ServiceKind};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use tokio_util::sync::CancellationToken;
use tracing::{info, warn};
use utoipa::ToSchema;

use crate::ApiState;
use crate::routes::{ApiError, ApiResult, blocking};

/// Where the API key lives unless `ai.api_key_file` is set.
pub const KEY_FILE: &str = "secrets/ai-api-key";
const REVIEW_DIR: &str = "ai/reviews";
/// Stored reviews kept.
const KEEP: usize = 20;
const WEEK: Duration = Duration::from_secs(7 * 86_400);

/// The assistant's files, and a lock so one review runs at a time.
#[derive(Debug)]
pub struct Assistant {
    data_dir: PathBuf,
    busy: tokio::sync::Mutex<()>,
}

impl Assistant {
    #[must_use]
    pub fn new(data_dir: impl Into<PathBuf>) -> Self {
        Self {
            data_dir: data_dir.into(),
            busy: tokio::sync::Mutex::new(()),
        }
    }

    fn key_path(&self, settings: &Ai) -> PathBuf {
        settings
            .api_key_file
            .as_ref()
            .map_or_else(|| self.data_dir.join(KEY_FILE), PathBuf::from)
    }

    fn key(&self, settings: &Ai) -> Option<String> {
        std::fs::read_to_string(self.key_path(settings))
            .ok()
            .map(|k| k.trim().to_owned())
            .filter(|k| !k.is_empty())
    }

    fn review_dir(&self) -> PathBuf {
        self.data_dir.join(REVIEW_DIR)
    }

    /// Stored reviews, newest first.
    #[must_use]
    pub fn reviews(&self) -> Vec<Review> {
        let Ok(dir) = std::fs::read_dir(self.review_dir()) else {
            return Vec::new();
        };
        let mut all: Vec<Review> = dir
            .filter_map(Result::ok)
            .filter(|e| e.path().extension().is_some_and(|x| x == "json"))
            .filter_map(|e| std::fs::read(e.path()).ok())
            .filter_map(|b| serde_json::from_slice(&b).ok())
            .collect();
        all.sort_by(|a, b| b.created_unix.cmp(&a.created_unix).then(b.id.cmp(&a.id)));
        all
    }

    fn save(&self, review: &Review) -> std::io::Result<()> {
        let dir = self.review_dir();
        std::fs::create_dir_all(&dir)?;
        let body = serde_json::to_string_pretty(review).map_err(std::io::Error::other)?;
        crate::auth::write_private(&dir.join(format!("{}.json", review.id)), &body)?;
        for old in self.reviews().iter().skip(KEEP) {
            let _ = std::fs::remove_file(dir.join(format!("{}.json", old.id)));
        }
        Ok(())
    }
}

/// The assistant's settings and whether it can run.
#[derive(Debug, Serialize, ToSchema)]
#[allow(clippy::struct_excessive_bools)]
pub struct AiInfo {
    pub enabled: bool,
    pub provider: AiProvider,
    /// The model in use, after defaults.
    pub model: Option<String>,
    /// The endpoint in use, after defaults.
    pub endpoint: Option<String>,
    /// Whether an API key is stored.
    pub key_set: bool,
    /// The key comes from `ai.api_key_file`, so the dashboard can't change it.
    pub key_from_file: bool,
    /// Whether the provider needs a key.
    pub needs_key: bool,
    /// Every provider with its defaults, for the settings form.
    pub providers: Vec<ProviderInfo>,
    /// A review is running now.
    pub busy: bool,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct ProviderInfo {
    pub id: AiProvider,
    pub default_model: Option<String>,
    pub default_endpoint: Option<String>,
    pub needs_key: bool,
}

#[derive(Debug, Deserialize, ToSchema)]
pub struct KeyRequest {
    /// The API key. Empty removes it.
    pub key: String,
}

#[derive(Debug, Deserialize, ToSchema)]
pub struct ReviewRequest {
    #[serde(default = "all")]
    pub scope: Scope,
}

fn all() -> Scope {
    Scope::All
}

#[derive(Debug, Deserialize, ToSchema)]
pub struct PreviewRequest {
    #[serde(default = "all")]
    pub scope: Scope,
    /// Settings to preview with, usually the draft's. Defaults to the
    /// current ones.
    #[serde(default)]
    pub settings: Option<Ai>,
}

/// What would be sent, exactly.
#[derive(Debug, Serialize, ToSchema)]
pub struct AiPreview {
    #[serde(flatten)]
    pub outgoing: Outgoing,
    /// How many distinct values were replaced by placeholders.
    pub redacted: usize,
}

/// The configuration with a finding's fix applied, to load into the draft.
#[derive(Debug, Serialize, ToSchema)]
pub struct FixDraft {
    pub state: DesiredState,
    pub summary: String,
}

async fn current_state(state: &ApiState) -> Result<DesiredState, ApiError> {
    let store = Arc::clone(&state.store);
    Ok(blocking(move || store.current())
        .await?
        .map(|r| r.state)
        .unwrap_or_default())
}

fn conflict(e: &hearthgate_ai::Error) -> ApiError {
    let status = match e {
        hearthgate_ai::Error::Disabled | hearthgate_ai::Error::Config(_) => StatusCode::CONFLICT,
        hearthgate_ai::Error::Provider(_) | hearthgate_ai::Error::Answer(_) => {
            StatusCode::BAD_GATEWAY
        }
    };
    ApiError::new(status, e.to_string())
}

#[utoipa::path(get, path = "/api/v1/ai", tag = "ai",
    responses((status = 200, body = AiInfo)))]
pub(crate) async fn info(State(state): State<ApiState>) -> ApiResult<AiInfo> {
    let settings = current_state(&state).await?.ai;
    Ok(Json(AiInfo {
        enabled: settings.enabled,
        provider: settings.provider,
        model: settings.model_or_default().map(str::to_owned),
        endpoint: settings.endpoint_or_default().map(str::to_owned),
        key_set: state.ai.key(&settings).is_some(),
        key_from_file: settings.api_key_file.is_some(),
        needs_key: settings.provider.needs_key(),
        providers: AiProvider::ALL
            .iter()
            .map(|p| ProviderInfo {
                id: *p,
                default_model: p.default_model().map(str::to_owned),
                default_endpoint: p.default_endpoint().map(str::to_owned),
                needs_key: p.needs_key(),
            })
            .collect(),
        busy: state.ai.busy.try_lock().is_err(),
    }))
}

#[utoipa::path(put, path = "/api/v1/ai/key", tag = "ai",
    description = "Store the provider API key in the data directory, readable only by \
                   Hearthgate. It never appears in the configuration, exports or backups' \
                   configuration file.",
    request_body = KeyRequest,
    responses((status = 204), (status = 409, body = crate::routes::Problem)))]
pub(crate) async fn put_key(
    State(state): State<ApiState>,
    Json(req): Json<KeyRequest>,
) -> Result<StatusCode, ApiError> {
    let settings = current_state(&state).await?.ai;
    if let Some(file) = &settings.api_key_file {
        return Err(ApiError::new(
            StatusCode::CONFLICT,
            format!("the key is read from {file}; change it there"),
        ));
    }
    let key = req.key.trim();
    if key.len() > 4096 || key.chars().any(char::is_control) {
        return Err(ApiError::new(
            StatusCode::BAD_REQUEST,
            "that doesn't look like an API key",
        ));
    }
    let path = state.ai.key_path(&settings);
    let key = key.to_owned();
    tokio::task::spawn_blocking(move || {
        if key.is_empty() {
            match std::fs::remove_file(&path) {
                Err(e) if e.kind() != std::io::ErrorKind::NotFound => Err(e),
                _ => Ok(()),
            }
        } else {
            if let Some(dir) = path.parent() {
                std::fs::create_dir_all(dir)?;
            }
            crate::auth::write_private(&path, &format!("{key}\n"))
        }
    })
    .await
    .map_err(|e| ApiError::new(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?
    .map_err(|e| {
        ApiError::new(
            StatusCode::INTERNAL_SERVER_ERROR,
            format!("cannot store the key: {e}"),
        )
    })?;
    Ok(StatusCode::NO_CONTENT)
}

#[utoipa::path(post, path = "/api/v1/ai/preview", tag = "ai",
    description = "The exact request a review would send, after redaction. Sends nothing, \
                   and works while AI is off.",
    request_body = PreviewRequest,
    responses((status = 200, body = AiPreview), (status = 409, body = crate::routes::Problem)))]
pub(crate) async fn preview(
    State(state): State<ApiState>,
    Json(req): Json<PreviewRequest>,
) -> ApiResult<AiPreview> {
    let desired = current_state(&state).await?;
    let settings = req.settings.unwrap_or_else(|| desired.ai.clone());
    let client = Client::for_preview(&settings).map_err(|e| conflict(&e))?;
    let (prompt, redactor) = build(&state, &desired, &settings, &req.scope).await?;
    Ok(Json(AiPreview {
        outgoing: client.outgoing(&prompt),
        redacted: redactor.len(),
    }))
}

#[utoipa::path(post, path = "/api/v1/ai/review", tag = "ai",
    description = "Ask the AI provider to review the configuration, the firewall, or one \
                   unhealthy service. Only works while AI is on.",
    request_body = ReviewRequest,
    responses((status = 200, body = Review), (status = 409, body = crate::routes::Problem),
              (status = 502, body = crate::routes::Problem)))]
pub(crate) async fn review(
    State(state): State<ApiState>,
    Json(req): Json<ReviewRequest>,
) -> ApiResult<Review> {
    Ok(Json(run(&state, req.scope, false).await?))
}

#[utoipa::path(get, path = "/api/v1/ai/reviews", tag = "ai",
    description = "Past reviews, newest first.",
    responses((status = 200, body = Vec<Review>)))]
pub(crate) async fn reviews(State(state): State<ApiState>) -> Json<Vec<Review>> {
    let ai = Arc::clone(&state.ai);
    Json(
        tokio::task::spawn_blocking(move || ai.reviews())
            .await
            .unwrap_or_default(),
    )
}

#[utoipa::path(post, path = "/api/v1/ai/reviews/{id}/findings/{index}/draft", tag = "ai",
    description = "The current configuration with this finding's fix applied, checked like \
                   any change. Changes nothing: load it into the draft, review the diff and \
                   apply it, or roll it back later like any revision.",
    params(("id" = String, Path), ("index" = usize, Path)),
    responses((status = 200, body = FixDraft), (status = 404, body = crate::routes::Problem),
              (status = 422, body = crate::routes::Problem)))]
pub(crate) async fn fix_draft(
    State(state): State<ApiState>,
    Path((id, index)): Path<(String, usize)>,
) -> ApiResult<FixDraft> {
    let ai = Arc::clone(&state.ai);
    let found = tokio::task::spawn_blocking(move || ai.reviews())
        .await
        .unwrap_or_default()
        .into_iter()
        .find(|r| r.id == id)
        .and_then(|r| r.findings.into_iter().nth(index))
        .and_then(|f| f.fix);
    let Some(fix) = found else {
        return Err(ApiError::new(
            StatusCode::NOT_FOUND,
            "no such finding, or it has no fix",
        ));
    };
    let current = current_state(&state).await?;
    let next = hearthgate_ai::review::propose(&current, &fix).map_err(|why| {
        ApiError::new(
            StatusCode::UNPROCESSABLE_ENTITY,
            format!("the fix no longer applies to the current configuration: {why}"),
        )
    })?;
    Ok(Json(FixDraft {
        state: next,
        summary: fix.summary,
    }))
}

/// Runs a review now and stores it.
async fn run(state: &ApiState, scope: Scope, scheduled: bool) -> Result<Review, ApiError> {
    let desired = current_state(state).await?;
    let settings = desired.ai.clone();
    // Before anything else: no settings, no client, no connection.
    let client = Client::new(&settings, state.ai.key(&settings)).map_err(|e| conflict(&e))?;
    let Ok(_busy) = state.ai.busy.try_lock() else {
        return Err(ApiError::new(
            StatusCode::CONFLICT,
            "a review is already running",
        ));
    };
    let (prompt, redactor) = build(state, &desired, &settings, &scope).await?;
    let answer = client.complete(&prompt).await.map_err(|e| conflict(&e))?;
    let (summary, mut findings) =
        hearthgate_ai::review::parse(&answer, &redactor).map_err(|e| conflict(&e))?;
    hearthgate_ai::review::check_fixes(&desired, &mut findings);
    let mut review = Review::new(scope, client.provider(), client.model(), summary, findings);
    review.scheduled = scheduled;
    review.revision = state
        .runtime
        .read()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .revision;
    let ai = Arc::clone(&state.ai);
    let stored = review.clone();
    let saved = tokio::task::spawn_blocking(move || ai.save(&stored)).await;
    if let Ok(Err(e)) = saved {
        warn!("cannot store the AI review: {e}");
    }
    Ok(review)
}

/// The redacted question for `scope`, and the redactor to read the answer
/// with.
async fn build(
    state: &ApiState,
    desired: &DesiredState,
    settings: &Ai,
    scope: &Scope,
) -> Result<(hearthgate_ai::Prompt, Redactor), ApiError> {
    let mut redactor = Redactor::new(&settings.redact).map_err(|e| {
        ApiError::new(
            StatusCode::CONFLICT,
            format!("a redaction pattern is invalid: {e}"),
        )
    })?;
    let mut material = material(state, desired, scope).await?;
    redactor.redact_json(&mut material);
    Ok((hearthgate_ai::review::prompt(scope, &material), redactor))
}

/// The configuration as the model sees it: without the assistant's own
/// settings and the notification targets, which it has no use for.
fn config_for_model(desired: &DesiredState) -> Value {
    let mut config = serde_json::to_value(desired).unwrap_or_default();
    if let Value::Object(map) = &mut config {
        map.remove("ai");
        map.remove("notifications");
    }
    config
}

/// Recent warnings and errors from Hearthgate's own log, optionally only
/// those that mention `about`.
fn log_lines(state: &ApiState, about: Option<&str>, limit: usize) -> Vec<Value> {
    let lines = state.live.logs.tail(2000);
    let mut out: Vec<Value> = lines
        .iter()
        .filter(|l| l.level == "error" || l.level == "warn")
        .filter(|l| about.is_none_or(|a| l.message.contains(a)))
        .map(|l| json!({"time": l.time, "level": l.level, "message": l.message}))
        .collect();
    let skip = out.len().saturating_sub(limit);
    out.drain(..skip);
    out
}

async fn material(
    state: &ApiState,
    desired: &DesiredState,
    scope: &Scope,
) -> Result<Value, ApiError> {
    let status = crate::routes::status_snapshot(state).await?;
    let certificates = crate::routes::cert_list(state).unwrap_or_default();
    let mut m = json!({
        "configuration": config_for_model(desired),
        "status": status,
    });
    match scope {
        Scope::All => {
            m["certificates"] = json!(certificates);
            m["firewall"] = json!(crate::firewall::info(State(state.clone())).await.0);
            m["recent_drops"] = recent_drops(state, 50).await;
            m["log"] = json!(log_lines(state, None, 100));
        }
        Scope::Firewall => {
            m["firewall"] = json!(crate::firewall::info(State(state.clone())).await.0);
            m["recent_drops"] = recent_drops(state, 200).await;
            m["log"] = json!(log_lines(state, Some("firewall"), 50));
        }
        Scope::Service { service } => {
            let Some(svc) = desired.services.iter().find(|s| &s.name == service) else {
                return Err(ApiError::new(
                    StatusCode::NOT_FOUND,
                    format!("no service named {service}"),
                ));
            };
            m["service"] = json!(svc);
            m["routes"] = json!(
                desired
                    .routes
                    .iter()
                    .filter(|r| r.targets.iter().any(|t| &t.service == service))
                    .collect::<Vec<_>>()
            );
            m["log"] = json!(log_lines(state, Some(service), 100));
            let container = match svc.kind {
                ServiceKind::External => None,
                ServiceKind::Docker => {
                    Some(svc.container.clone().unwrap_or_else(|| service.clone()))
                }
            };
            if let Some(name) = container {
                m["container"] = json!(desired.containers.iter().find(|c| c.name == name));
                m["container_logs"] = match state.docker.logs(&name, 150, None).await {
                    Ok(lines) => json!(lines),
                    Err(e) => json!(format!("container logs unavailable: {e}")),
                };
            }
        }
    }
    Ok(m)
}

async fn recent_drops(state: &ApiState, limit: usize) -> Value {
    json!(
        crate::firewall::recent(State(state.clone()))
            .await
            .0
            .into_iter()
            .take(limit)
            .collect::<Vec<_>>()
    )
}

/// Whether a scheduled review is due: a week after the last one.
fn due(reviews: &[Review], now: std::time::SystemTime) -> bool {
    let Some(last) = reviews.iter().find(|r| r.scheduled) else {
        return true;
    };
    let at = std::time::UNIX_EPOCH + Duration::from_secs(last.created_unix);
    now.duration_since(at).unwrap_or_default() >= WEEK
}

/// Runs the weekly review while it is on, until `stop`. Checks hourly, so
/// turning it on takes effect within an hour, and restarts don't add runs.
pub async fn run_weekly(state: ApiState, stop: CancellationToken) {
    let mut first = true;
    loop {
        let wait = if first {
            Duration::from_secs(300)
        } else {
            Duration::from_secs(3600)
        };
        first = false;
        tokio::select! {
            () = stop.cancelled() => return,
            () = tokio::time::sleep(wait) => {}
        }
        let Ok(desired) = current_state(&state).await else {
            continue;
        };
        if !desired.ai.enabled || !desired.ai.weekly_review {
            continue;
        }
        let ai = Arc::clone(&state.ai);
        let past = tokio::task::spawn_blocking(move || ai.reviews())
            .await
            .unwrap_or_default();
        if !due(&past, std::time::SystemTime::now()) {
            continue;
        }
        match run(&state, Scope::All, true).await {
            Ok(review) => {
                info!(
                    findings = review.findings.len(),
                    "weekly AI review finished"
                );
                notify(&review);
            }
            Err(e) => {
                warn!("weekly AI review failed: {}", e.1.error);
                hearthgate_notify::emit(hearthgate_notify::Event::new(
                    hearthgate_model::EventKind::Reviews,
                    hearthgate_notify::Severity::Warning,
                    "ai-review-failed",
                    "The weekly AI review failed",
                    e.1.error,
                ));
            }
        }
    }
}

fn notify(review: &Review) {
    let high = review
        .findings
        .iter()
        .filter(|f| f.severity == Severity::High)
        .count();
    let (severity, title) = match review.findings.len() {
        0 => (
            hearthgate_notify::Severity::Info,
            "Weekly AI review: all clear".to_owned(),
        ),
        n => (
            if high > 0 {
                hearthgate_notify::Severity::Warning
            } else {
                hearthgate_notify::Severity::Info
            },
            format!(
                "Weekly AI review: {n} finding{}",
                if n == 1 { "" } else { "s" }
            ),
        ),
    };
    let mut message = review.summary.clone();
    for f in review.findings.iter().take(5) {
        let _ = write!(message, "\n• [{}] {}", severity_word(f), f.title);
    }
    message.push_str("\n\nOpen AI review in the dashboard to see the details and fixes.");
    hearthgate_notify::emit(hearthgate_notify::Event::new(
        hearthgate_model::EventKind::Reviews,
        severity,
        format!("ai-review:{}", review.id),
        title,
        message,
    ));
}

fn severity_word(f: &Finding) -> &'static str {
    match f.severity {
        Severity::High => "high",
        Severity::Medium => "medium",
        Severity::Low => "low",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn weekly_due() {
        let now = std::time::SystemTime::now();
        assert!(due(&[], now));
        let mut r = Review::new(
            Scope::All,
            AiProvider::Anthropic,
            "m",
            String::new(),
            vec![],
        );
        assert!(
            due(std::slice::from_ref(&r), now),
            "a manual review doesn't count"
        );
        r.scheduled = true;
        assert!(!due(std::slice::from_ref(&r), now));
        assert!(due(std::slice::from_ref(&r), now + WEEK));
    }
}
