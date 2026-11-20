//! Dashboard accounts: first-run setup, login, logout, password, two-factor
//! login and API tokens.
//!
//! **First-run setup** needs a one-time setup code, printed in the log and
//! by `hearthgate setup-code`. Without it, anyone on the LAN who opened
//! the dashboard first could make themselves admin.

use std::sync::Arc;

use axum::Json;
use axum::extract::{Extension, Path, State};
use axum::http::{StatusCode, header};
use axum::response::{IntoResponse, Response};
use hearthgate_store::{Session, Store, StoreError};
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

use crate::ApiState;
use crate::auth::{
    API_TOKEN_PREFIX, ClientAddr, Principal, SESSION_IDLE, SessionPrincipal, cookie_token,
    session_cookie,
};
use crate::routes::{ApiError, ApiResult, blocking};
use crate::secret;

/// Metadata key holding the setup code until the first account exists.
const SETUP_CODE_KEY: &str = "setup_code";

/// Returns the setup code while no account exists, creating it on first
/// use. `None` once setup is done.
pub fn setup_code(store: &Store) -> Result<Option<String>, StoreError> {
    if store.user_count()? > 0 {
        store.meta_delete(SETUP_CODE_KEY)?;
        return Ok(None);
    }
    if let Some(code) = store.meta_get(SETUP_CODE_KEY)? {
        return Ok(Some(code));
    }
    let raw = secret::base32(&secret::random_bytes(8));
    let code = format!("{}-{}-{}", &raw[0..4], &raw[4..8], &raw[8..12]);
    store.meta_set(SETUP_CODE_KEY, &code)?;
    Ok(Some(code))
}

fn normalize_code(code: &str) -> String {
    code.chars()
        .filter(char::is_ascii_alphanumeric)
        .map(|c| c.to_ascii_uppercase())
        .collect()
}

fn check_username(name: &str) -> Result<(), ApiError> {
    let ok = (1..=64).contains(&name.len())
        && name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || "._-@".contains(c));
    if ok {
        Ok(())
    } else {
        Err(ApiError::new(
            StatusCode::UNPROCESSABLE_ENTITY,
            "use 1 to 64 letters, digits, dots, dashes, underscores or @ for the username",
        ))
    }
}

fn policy(password: &str) -> Result<(), ApiError> {
    secret::check_password_policy(password)
        .map_err(|e| ApiError::new(StatusCode::UNPROCESSABLE_ENTITY, e))
}

fn internal(e: impl std::fmt::Display) -> ApiError {
    ApiError::new(StatusCode::INTERNAL_SERVER_ERROR, e.to_string())
}

/// Hashes on a blocking thread: argon2 takes tens of milliseconds by design.
async fn hash(password: String) -> Result<String, ApiError> {
    tokio::task::spawn_blocking(move || secret::hash_password(&password))
        .await
        .map_err(internal)?
        .map_err(internal)
}

async fn verify(password: String, hash: String) -> Result<bool, ApiError> {
    tokio::task::spawn_blocking(move || secret::verify_password(&password, &hash))
        .await
        .map_err(internal)
}

fn too_many(retry: std::time::Duration) -> ApiError {
    let minutes = retry.as_secs().div_ceil(60).max(1);
    ApiError::new(
        StatusCode::TOO_MANY_REQUESTS,
        format!("too many failed attempts; try again in {minutes} min"),
    )
}

/// The logged-in user, as the dashboard sees it.
#[derive(Debug, Serialize, ToSchema)]
pub struct UserInfo {
    pub username: String,
    pub totp_enabled: bool,
}

/// Who is logged in, and whether first-run setup is still needed.
#[derive(Debug, Serialize, ToSchema)]
pub struct SessionInfo {
    /// No account exists yet: show the setup screen.
    pub setup_required: bool,
    /// The logged-in user, if any.
    pub user: Option<UserInfo>,
    /// Send this as `X-CSRF-Token` with every change.
    pub csrf: Option<String>,
}

/// Who is logged in.
#[utoipa::path(
    get, path = "/api/v1/auth/session", tag = "auth",
    responses((status = 200, body = SessionInfo)),
    security(())
)]
pub async fn session_info(
    State(state): State<ApiState>,
    headers: axum::http::HeaderMap,
) -> ApiResult<SessionInfo> {
    let store = Arc::clone(&state.store);
    let token = cookie_token(&headers, state.secure_cookies);
    let info = blocking(move || {
        let setup_required = store.user_count()? == 0;
        let mut user = None;
        let mut csrf = None;
        if let Some(token) = token {
            let hash = secret::sha256_hex(&token);
            if let Some(session) = store.session(&hash, secret::now())? {
                if let Some(u) = store.user(session.user_id)? {
                    user = Some(UserInfo {
                        username: u.username,
                        totp_enabled: u.totp_secret.is_some(),
                    });
                    csrf = Some(session.csrf);
                }
            }
        }
        Ok(SessionInfo {
            setup_required,
            user,
            csrf,
        })
    })
    .await?;
    Ok(Json(info))
}

/// First-run setup.
#[derive(Debug, Deserialize, ToSchema)]
pub struct SetupRequest {
    /// From the log or `hearthgate setup-code`.
    pub setup_code: String,
    pub username: String,
    pub password: String,
}

/// Create the first account. Only works while no account exists.
#[utoipa::path(
    post, path = "/api/v1/auth/setup", tag = "auth", request_body = SetupRequest,
    responses(
        (status = 200, description = "Account created and logged in", body = SessionInfo),
        (status = 403, description = "Wrong setup code", body = crate::routes::Problem),
        (status = 409, description = "Setup is already done", body = crate::routes::Problem),
        (status = 429, body = crate::routes::Problem),
    ),
    security(())
)]
pub async fn setup(
    State(state): State<ApiState>,
    Extension(ClientAddr(client)): Extension<ClientAddr>,
    Json(req): Json<SetupRequest>,
) -> Result<Response, ApiError> {
    if let Some(retry) = state.auth.limiter.blocked(client.ip(), None) {
        return Err(too_many(retry));
    }
    check_username(&req.username)?;
    policy(&req.password)?;
    let store = Arc::clone(&state.store);
    let expected = blocking(move || setup_code(&store)).await?;
    let Some(expected) = expected else {
        return Err(ApiError::new(
            StatusCode::CONFLICT,
            "setup is already done; log in instead",
        ));
    };
    if !secret::equal(
        normalize_code(&req.setup_code).as_bytes(),
        normalize_code(&expected).as_bytes(),
    ) {
        state.auth.limiter.fail(client.ip(), None);
        return Err(ApiError::new(
            StatusCode::FORBIDDEN,
            "wrong setup code; run `hearthgate setup-code` on the server to see it",
        ));
    }
    let password_hash = hash(req.password).await?;
    let store = Arc::clone(&state.store);
    let username = req.username.clone();
    let user = blocking(move || {
        let user = store.create_first_user(&username, &password_hash, secret::now())?;
        if user.is_some() {
            store.meta_delete(SETUP_CODE_KEY)?;
        }
        Ok(user)
    })
    .await?
    .ok_or_else(|| {
        ApiError::new(
            StatusCode::CONFLICT,
            "setup is already done; log in instead",
        )
    })?;
    tracing::info!(user = %user.username, %client, "first dashboard account created");
    start_session(&state, user.id, user.username, false).await
}

/// Creates a session and answers with its cookie.
async fn start_session(
    state: &ApiState,
    user_id: i64,
    username: String,
    totp_enabled: bool,
) -> Result<Response, ApiError> {
    let token = secret::random_token(32);
    let csrf = secret::random_token(16);
    let now = secret::now();
    let session = Session {
        user_id,
        csrf: csrf.clone(),
        created_at: now,
        expires_at: now + SESSION_IDLE,
    };
    let store = Arc::clone(&state.store);
    let hash = secret::sha256_hex(&token);
    blocking(move || store.create_session(&hash, &session)).await?;
    let body = SessionInfo {
        setup_required: false,
        user: Some(UserInfo {
            username,
            totp_enabled,
        }),
        csrf: Some(csrf),
    };
    Ok((
        [(
            header::SET_COOKIE,
            session_cookie(state.secure_cookies, Some(&token)),
        )],
        Json(body),
    )
        .into_response())
}

/// Login form.
#[derive(Debug, Deserialize, ToSchema)]
pub struct LoginRequest {
    pub username: String,
    pub password: String,
    /// Six-digit code from the authenticator app, when two-factor login is on.
    pub code: Option<String>,
}

/// A refused login.
#[derive(Debug, Serialize, ToSchema)]
pub struct LoginProblem {
    pub error: String,
    /// The password was right; ask for the authenticator code.
    pub totp_required: bool,
}

/// Log in to the dashboard.
#[utoipa::path(
    post, path = "/api/v1/auth/login", tag = "auth", request_body = LoginRequest,
    responses(
        (status = 200, description = "Logged in; the session cookie is set", body = SessionInfo),
        (status = 401, body = LoginProblem),
        (status = 429, body = crate::routes::Problem),
    ),
    security(())
)]
pub async fn login(
    State(state): State<ApiState>,
    Extension(ClientAddr(client)): Extension<ClientAddr>,
    Json(req): Json<LoginRequest>,
) -> Result<Response, ApiError> {
    let limiter = &state.auth.limiter;
    if let Some(retry) = limiter.blocked(client.ip(), Some(&req.username)) {
        return Err(too_many(retry));
    }
    let store = Arc::clone(&state.store);
    let username = req.username.clone();
    let user = blocking(move || store.user_by_name(&username)).await?;
    let stored = user.as_ref().map_or_else(
        || secret::dummy_hash().to_owned(),
        |u| u.password_hash.clone(),
    );
    let password_ok = verify(req.password, stored).await? && user.is_some();
    let refuse = |message: &str, totp_required: bool| {
        (
            StatusCode::UNAUTHORIZED,
            Json(LoginProblem {
                error: message.to_owned(),
                totp_required,
            }),
        )
            .into_response()
    };
    let Some(user) = user.filter(|_| password_ok) else {
        limiter.fail(client.ip(), Some(&req.username));
        tracing::info!(user = %req.username, %client, "failed dashboard login");
        return Ok(refuse("wrong username or password", false));
    };
    if let Some(totp) = &user.totp_secret {
        let Some(code) = req.code.as_deref().filter(|c| !c.trim().is_empty()) else {
            return Ok(refuse("enter the code from your authenticator app", true));
        };
        let step = secret::totp_match(totp, code, secret::now());
        if !step.is_some_and(|s| state.auth.use_totp_step(user.id, s)) {
            limiter.fail(client.ip(), Some(&req.username));
            return Ok(refuse(
                "wrong or already used code; wait for the next one",
                true,
            ));
        }
    }
    limiter.succeed(client.ip());
    tracing::info!(user = %user.username, %client, "dashboard login");
    start_session(&state, user.id, user.username, user.totp_secret.is_some()).await
}

fn session_of(principal: &Principal) -> Result<&SessionPrincipal, ApiError> {
    match principal {
        Principal::Session(s) => Ok(s),
        _ => Err(ApiError::new(
            StatusCode::FORBIDDEN,
            "only a logged-in dashboard user can do this",
        )),
    }
}

/// Log out (ends this session).
#[utoipa::path(
    post, path = "/api/v1/auth/logout", tag = "auth",
    responses((status = 204, description = "Logged out"))
)]
pub async fn logout(
    State(state): State<ApiState>,
    Extension(principal): Extension<Principal>,
) -> Result<Response, ApiError> {
    if let Principal::Session(s) = principal {
        let store = Arc::clone(&state.store);
        blocking(move || store.delete_session(&s.token_hash)).await?;
    }
    Ok((
        StatusCode::NO_CONTENT,
        [(
            header::SET_COOKIE,
            session_cookie(state.secure_cookies, None),
        )],
    )
        .into_response())
}

/// New password.
#[derive(Debug, Deserialize, ToSchema)]
pub struct PasswordRequest {
    pub current_password: String,
    pub new_password: String,
}

async fn check_current_password(
    state: &ApiState,
    s: &SessionPrincipal,
    client: ClientAddr,
    password: String,
) -> Result<(), ApiError> {
    if let Some(retry) = state.auth.limiter.blocked(client.0.ip(), Some(&s.username)) {
        return Err(too_many(retry));
    }
    let store = Arc::clone(&state.store);
    let id = s.user_id;
    let user = blocking(move || store.user(id))
        .await?
        .ok_or_else(|| ApiError::new(StatusCode::UNAUTHORIZED, "log in again"))?;
    if verify(password, user.password_hash).await? {
        Ok(())
    } else {
        state.auth.limiter.fail(client.0.ip(), Some(&s.username));
        Err(ApiError::new(
            StatusCode::FORBIDDEN,
            "the current password is wrong",
        ))
    }
}

/// Change the password. Every other session of this user is logged out.
#[utoipa::path(
    post, path = "/api/v1/auth/password", tag = "auth", request_body = PasswordRequest,
    responses((status = 204, description = "Changed"), (status = 403, body = crate::routes::Problem))
)]
pub async fn change_password(
    State(state): State<ApiState>,
    Extension(principal): Extension<Principal>,
    Extension(client): Extension<ClientAddr>,
    Json(req): Json<PasswordRequest>,
) -> Result<StatusCode, ApiError> {
    let s = session_of(&principal)?;
    policy(&req.new_password)?;
    check_current_password(&state, s, client, req.current_password).await?;
    let new_hash = hash(req.new_password).await?;
    let store = Arc::clone(&state.store);
    let (id, keep) = (s.user_id, s.token_hash.clone());
    blocking(move || store.set_password(id, &new_hash, Some(&keep))).await?;
    tracing::info!(user = %s.username, "dashboard password changed");
    Ok(StatusCode::NO_CONTENT)
}

/// A new TOTP secret to show as a QR code. Nothing changes until it is
/// confirmed with a code.
#[derive(Debug, Serialize, ToSchema)]
pub struct TotpSecret {
    /// Base32, for typing into the app by hand.
    pub secret: String,
    /// `otpauth://` URI for the QR code.
    pub uri: String,
}

/// Start turning on two-factor login.
#[utoipa::path(
    post, path = "/api/v1/auth/totp/begin", tag = "auth",
    responses((status = 200, body = TotpSecret))
)]
pub async fn totp_begin(Extension(principal): Extension<Principal>) -> ApiResult<TotpSecret> {
    let s = session_of(&principal)?;
    let secret = secret::base32(&secret::random_bytes(20));
    Ok(Json(TotpSecret {
        uri: secret::totp_uri(&secret, &s.username),
        secret,
    }))
}

/// Confirm two-factor login.
#[derive(Debug, Deserialize, ToSchema)]
pub struct TotpEnableRequest {
    /// The secret from `totp/begin`.
    pub secret: String,
    /// A current code from the app, proving it was set up.
    pub code: String,
}

/// Turn on two-factor login.
#[utoipa::path(
    post, path = "/api/v1/auth/totp/enable", tag = "auth", request_body = TotpEnableRequest,
    responses((status = 204, description = "On"), (status = 422, body = crate::routes::Problem))
)]
pub async fn totp_enable(
    State(state): State<ApiState>,
    Extension(principal): Extension<Principal>,
    Json(req): Json<TotpEnableRequest>,
) -> Result<StatusCode, ApiError> {
    let s = session_of(&principal)?;
    let bytes = secret::unbase32(&req.secret).filter(|b| b.len() >= 16);
    let Some(step) = bytes.and_then(|_| secret::totp_match(&req.secret, &req.code, secret::now()))
    else {
        return Err(ApiError::new(
            StatusCode::UNPROCESSABLE_ENTITY,
            "that code doesn't match; check the time on your phone and try the next code",
        ));
    };
    state.auth.use_totp_step(s.user_id, step);
    let store = Arc::clone(&state.store);
    let (id, secret) = (s.user_id, req.secret.to_ascii_uppercase().replace(' ', ""));
    blocking(move || store.set_totp(id, Some(&secret))).await?;
    tracing::info!(user = %s.username, "two-factor login turned on");
    Ok(StatusCode::NO_CONTENT)
}

/// Confirm with the password.
#[derive(Debug, Deserialize, ToSchema)]
pub struct PasswordConfirm {
    pub password: String,
}

/// Turn off two-factor login.
#[utoipa::path(
    post, path = "/api/v1/auth/totp/disable", tag = "auth", request_body = PasswordConfirm,
    responses((status = 204, description = "Off"), (status = 403, body = crate::routes::Problem))
)]
pub async fn totp_disable(
    State(state): State<ApiState>,
    Extension(principal): Extension<Principal>,
    Extension(client): Extension<ClientAddr>,
    Json(req): Json<PasswordConfirm>,
) -> Result<StatusCode, ApiError> {
    let s = session_of(&principal)?;
    check_current_password(&state, s, client, req.password).await?;
    let store = Arc::clone(&state.store);
    let id = s.user_id;
    blocking(move || store.set_totp(id, None)).await?;
    tracing::info!(user = %s.username, "two-factor login turned off");
    Ok(StatusCode::NO_CONTENT)
}

/// An API token, without its secret.
#[derive(Debug, Serialize, ToSchema)]
pub struct ApiTokenInfo {
    pub id: i64,
    pub name: String,
    /// Unix seconds.
    pub created_at: i64,
    /// Unix seconds.
    pub last_used_at: Option<i64>,
}

impl From<hearthgate_store::ApiToken> for ApiTokenInfo {
    fn from(t: hearthgate_store::ApiToken) -> Self {
        Self {
            id: t.id,
            name: t.name,
            created_at: t.created_at,
            last_used_at: t.last_used_at,
        }
    }
}

/// API tokens for scripts.
#[utoipa::path(
    get, path = "/api/v1/tokens", tag = "auth",
    responses((status = 200, body = Vec<ApiTokenInfo>))
)]
pub async fn list_tokens(State(state): State<ApiState>) -> ApiResult<Vec<ApiTokenInfo>> {
    let store = Arc::clone(&state.store);
    let list = blocking(move || store.api_tokens()).await?;
    Ok(Json(list.into_iter().map(Into::into).collect()))
}

/// A name for the new token.
#[derive(Debug, Deserialize, ToSchema)]
pub struct NewToken {
    pub name: String,
}

/// The new token. Its secret is shown only this once.
#[derive(Debug, Serialize, ToSchema)]
pub struct CreatedToken {
    #[serde(flatten)]
    pub info: ApiTokenInfo,
    pub token: String,
}

/// Create an API token.
#[utoipa::path(
    post, path = "/api/v1/tokens", tag = "auth", request_body = NewToken,
    responses((status = 200, body = CreatedToken))
)]
pub async fn create_token(
    State(state): State<ApiState>,
    Json(req): Json<NewToken>,
) -> ApiResult<CreatedToken> {
    let name = req.name.trim().to_owned();
    if name.is_empty() || name.len() > 100 {
        return Err(ApiError::new(
            StatusCode::UNPROCESSABLE_ENTITY,
            "give the token a name of 1 to 100 characters",
        ));
    }
    let token = format!("{API_TOKEN_PREFIX}{}", secret::random_token(32));
    let store = Arc::clone(&state.store);
    let hash = secret::sha256_hex(&token);
    let info = blocking(move || store.create_api_token(&name, &hash, secret::now())).await?;
    Ok(Json(CreatedToken {
        info: info.into(),
        token,
    }))
}

/// Revoke an API token.
#[utoipa::path(
    delete, path = "/api/v1/tokens/{id}", tag = "auth",
    params(("id" = i64, Path, description = "Token id")),
    responses((status = 204, description = "Revoked"), (status = 404, body = crate::routes::Problem))
)]
pub async fn delete_token(
    State(state): State<ApiState>,
    Path(id): Path<i64>,
) -> Result<StatusCode, ApiError> {
    let store = Arc::clone(&state.store);
    if blocking(move || store.delete_api_token(id)).await? {
        Ok(StatusCode::NO_CONTENT)
    } else {
        Err(ApiError::new(StatusCode::NOT_FOUND, "no such token"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn setup_code_lives_until_the_first_account() {
        let store = Store::open_in_memory().unwrap();
        let code = setup_code(&store).unwrap().unwrap();
        assert_eq!(code.len(), 14);
        assert_eq!(setup_code(&store).unwrap().as_deref(), Some(code.as_str()));
        assert_eq!(
            normalize_code(&code.to_lowercase().replace('-', " ")),
            normalize_code(&code)
        );
        store.create_first_user("admin", "h", 0).unwrap();
        assert_eq!(setup_code(&store).unwrap(), None);
    }
}
