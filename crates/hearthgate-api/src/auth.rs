//! Who is calling: the admin token, an API token or a dashboard session.
//!
//! - **Admin token**: generated on first start into the data directory
//!   (owner-only). For the CLI on the same machine and for recovery.
//! - **API tokens**: created in the dashboard for scripts, stored only as
//!   SHA-256 hashes, sent as `Authorization: Bearer hg_…`.
//! - **Sessions**: the dashboard logs in with a password (and a TOTP code
//!   when enabled) and gets an `HttpOnly`, `SameSite=Strict` cookie. Every
//!   request that changes something must also carry the session's CSRF
//!   token in `X-CSRF-Token`.

use std::collections::HashMap;
use std::net::SocketAddr;
use std::path::Path;
use std::sync::{Arc, Mutex};

use axum::extract::{Request, State};
use axum::http::{HeaderMap, Method, StatusCode, header};
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};

use crate::ApiState;
use crate::ratelimit::LoginLimiter;
use crate::routes::ApiError;
use crate::secret;

/// File name of the admin token inside the data directory.
pub const TOKEN_FILE: &str = "admin-token";
/// Prefix of API tokens, so they are easy to recognize in a leak scan.
pub const API_TOKEN_PREFIX: &str = "hg_";
/// Header carrying the CSRF token.
pub const CSRF_HEADER: &str = "x-csrf-token";
/// A session ends after this long without use.
pub const SESSION_IDLE: i64 = 12 * 3600;
/// A session ends this long after login, used or not.
pub const SESSION_MAX: i64 = 7 * 24 * 3600;

/// The client's address, added to every request by the listener.
#[derive(Debug, Clone, Copy)]
pub struct ClientAddr(pub SocketAddr);

/// The authenticated caller of a request.
#[derive(Debug, Clone)]
pub enum Principal {
    AdminToken,
    ApiToken { id: i64 },
    Session(SessionPrincipal),
}

/// A dashboard user, logged in with a session cookie.
#[derive(Debug, Clone)]
pub struct SessionPrincipal {
    pub user_id: i64,
    pub username: String,
    pub token_hash: String,
    pub csrf: String,
}

impl Principal {
    /// How changes by this caller are recorded in revision history.
    #[must_use]
    pub fn author(&self) -> String {
        match self {
            Self::AdminToken => "api:admin-token".to_owned(),
            Self::ApiToken { id } => format!("api:token-{id}"),
            Self::Session(s) => format!("dashboard:{}", s.username),
        }
    }
}

/// Shared authentication state.
#[derive(Debug, Default)]
pub struct AuthState {
    pub limiter: LoginLimiter,
    /// Last TOTP time step used per user, so a code works only once.
    pub totp_steps: Mutex<HashMap<i64, i64>>,
}

impl AuthState {
    /// Records the use of a TOTP step. Returns false for a replayed code.
    pub fn use_totp_step(&self, user_id: i64, step: i64) -> bool {
        let mut map = self
            .totp_steps
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        match map.get(&user_id) {
            Some(&last) if step <= last => false,
            _ => {
                map.insert(user_id, step);
                true
            }
        }
    }
}

/// Reads the admin token, creating a random one when there is none.
pub fn load_or_create_token(data_dir: &Path) -> std::io::Result<String> {
    let path = data_dir.join(TOKEN_FILE);
    match std::fs::read_to_string(&path) {
        Ok(token) if !token.trim().is_empty() => return Ok(token.trim().to_owned()),
        Ok(_) => {}
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
        Err(e) => return Err(e),
    }
    let token = secret::random_token(32);
    std::fs::create_dir_all(data_dir)?;
    write_private(&path, &format!("{token}\n"))?;
    Ok(token)
}

/// Writes a file only the owner can read.
pub fn write_private(path: &Path, contents: &str) -> std::io::Result<()> {
    #[cfg(unix)]
    {
        use std::io::Write;
        use std::os::unix::fs::OpenOptionsExt;
        let mut f = std::fs::OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(true)
            .mode(0o600)
            .open(path)?;
        f.write_all(contents.as_bytes())?;
        Ok(())
    }
    #[cfg(not(unix))]
    std::fs::write(path, contents)
}

/// The session cookie's name. The `__Host-` prefix makes browsers refuse
/// it unless it is `Secure`, has no domain and covers the whole site.
#[must_use]
pub fn cookie_name(secure: bool) -> &'static str {
    if secure {
        "__Host-hg_session"
    } else {
        "hg_session"
    }
}

/// A `Set-Cookie` value for a session token (or for clearing it).
#[must_use]
pub fn session_cookie(secure: bool, token: Option<&str>) -> String {
    let name = cookie_name(secure);
    let secure_attr = if secure { "; Secure" } else { "" };
    match token {
        Some(t) => format!(
            "{name}={t}; Path=/; HttpOnly; SameSite=Strict; Max-Age={SESSION_MAX}{secure_attr}"
        ),
        None => format!("{name}=; Path=/; HttpOnly; SameSite=Strict; Max-Age=0{secure_attr}"),
    }
}

/// The session token from the request's cookies.
#[must_use]
pub fn cookie_token(headers: &HeaderMap, secure: bool) -> Option<String> {
    let name = cookie_name(secure);
    headers
        .get_all(header::COOKIE)
        .iter()
        .filter_map(|v| v.to_str().ok())
        .flat_map(|v| v.split(';'))
        .filter_map(|pair| pair.trim().split_once('='))
        .find(|(k, _)| *k == name)
        .map(|(_, v)| v.to_owned())
        .filter(|v| !v.is_empty())
}

fn bearer(headers: &HeaderMap) -> Option<&str> {
    headers
        .get(header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("Bearer "))
        .map(str::trim)
}

/// Paths anyone allowed to connect may use without logging in.
fn is_public(path: &str, open_metrics: bool) -> bool {
    match path {
        "/api/v1/openapi.json"
        | "/api/v1/auth/session"
        | "/api/v1/auth/setup"
        | "/api/v1/auth/login" => true,
        "/metrics" => open_metrics,
        // Dashboard files: the app itself asks the API who is logged in.
        _ => !path.starts_with("/api/"),
    }
}

fn unauthorized(message: &str) -> Response {
    let mut response = ApiError::new(StatusCode::UNAUTHORIZED, message).into_response();
    response.headers_mut().insert(
        header::WWW_AUTHENTICATE,
        header::HeaderValue::from_static("Bearer"),
    );
    response
}

/// Finds the principal for a request, or answers 401/403.
pub async fn authenticate(State(state): State<ApiState>, mut req: Request, next: Next) -> Response {
    if is_public(req.uri().path(), state.open_metrics) {
        return next.run(req).await;
    }
    let principal = if let Some(token) = bearer(req.headers()) {
        if secret::equal(token.as_bytes(), state.token.as_bytes()) {
            Principal::AdminToken
        } else if token.starts_with(API_TOKEN_PREFIX) {
            let store = Arc::clone(&state.store);
            let hash = secret::sha256_hex(token);
            match crate::routes::blocking(move || store.use_api_token(&hash, secret::now())).await {
                Ok(Some(id)) => Principal::ApiToken { id },
                Ok(None) => return unauthorized("this API token is not valid; create a new one"),
                Err(e) => return e.into_response(),
            }
        } else {
            return unauthorized("wrong admin token");
        }
    } else if let Some(token) = cookie_token(req.headers(), state.secure_cookies) {
        match session_principal(&state, &token).await {
            Ok(Some(p)) => {
                if !matches!(*req.method(), Method::GET | Method::HEAD | Method::OPTIONS) {
                    let sent = req.headers().get(CSRF_HEADER).and_then(|v| v.to_str().ok());
                    if !sent.is_some_and(|s| secret::equal(s.as_bytes(), p.csrf.as_bytes())) {
                        return ApiError::new(
                            StatusCode::FORBIDDEN,
                            "missing or wrong CSRF token; reload the page",
                        )
                        .into_response();
                    }
                }
                Principal::Session(p)
            }
            Ok(None) => return unauthorized("your session has ended; log in again"),
            Err(e) => return e.into_response(),
        }
    } else {
        return unauthorized("log in, or send an API token as `Authorization: Bearer …`");
    };
    req.extensions_mut().insert(principal);
    next.run(req).await
}

/// Looks up a session cookie, extending it while in use.
async fn session_principal(
    state: &ApiState,
    token: &str,
) -> Result<Option<SessionPrincipal>, ApiError> {
    let store = Arc::clone(&state.store);
    let token_hash = secret::sha256_hex(token);
    crate::routes::blocking(move || {
        let now = secret::now();
        let Some(session) = store.session(&token_hash, now)? else {
            return Ok(None);
        };
        let Some(user) = store.user(session.user_id)? else {
            store.delete_session(&token_hash)?;
            return Ok(None);
        };
        // Slide the idle timeout, at most once a minute, never past the
        // absolute limit.
        let wanted = (now + SESSION_IDLE).min(session.created_at + SESSION_MAX);
        if wanted > session.expires_at + 60 {
            store.extend_session(&token_hash, wanted)?;
        }
        Ok(Some(SessionPrincipal {
            user_id: user.id,
            username: user.username,
            token_hash,
            csrf: session.csrf,
        }))
    })
    .await
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn token_is_created_once_and_private() {
        let dir = tempfile::tempdir().unwrap();
        let a = load_or_create_token(dir.path()).unwrap();
        let b = load_or_create_token(dir.path()).unwrap();
        assert_eq!(a, b);
        assert_eq!(a.len(), 64);
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(dir.path().join(TOKEN_FILE))
                .unwrap()
                .permissions()
                .mode();
            assert_eq!(mode & 0o777, 0o600);
        }
    }

    #[test]
    fn cookies() {
        let mut headers = HeaderMap::new();
        headers.insert(
            header::COOKIE,
            "a=1; __Host-hg_session=abc; b=2".parse().unwrap(),
        );
        assert_eq!(cookie_token(&headers, true).as_deref(), Some("abc"));
        assert_eq!(cookie_token(&headers, false), None);
        let set = session_cookie(true, Some("abc"));
        assert!(
            set.contains("HttpOnly") && set.contains("SameSite=Strict") && set.contains("Secure")
        );
        assert!(session_cookie(false, None).contains("Max-Age=0"));
    }

    #[test]
    fn public_paths() {
        assert!(is_public("/", false));
        assert!(is_public("/assets/app.js", false));
        assert!(is_public("/api/v1/auth/login", false));
        assert!(!is_public("/api/v1/state", false));
        assert!(!is_public("/api/v1/auth/logout", false));
        assert!(!is_public("/metrics", false));
        assert!(is_public("/metrics", true));
    }

    #[test]
    fn totp_codes_work_once() {
        let auth = AuthState::default();
        assert!(auth.use_totp_step(1, 100));
        assert!(!auth.use_totp_step(1, 100));
        assert!(!auth.use_totp_step(1, 99));
        assert!(auth.use_totp_step(1, 101));
        assert!(auth.use_totp_step(2, 100));
    }
}
