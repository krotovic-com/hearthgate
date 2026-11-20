//! Serves the dashboard (a Svelte single-page app embedded at build time)
//! and adds security headers to every admin response.

use axum::extract::Request;
use axum::http::{HeaderValue, Method, StatusCode, Uri, header};
use axum::middleware::Next;
use axum::response::{Html, IntoResponse, Response};

// Without a built dashboard nothing is embedded, but the crate stays a
// dependency so both builds resolve the same way.
#[cfg(not(dashboard))]
use rust_embed as _;

#[cfg(dashboard)]
#[derive(rust_embed::RustEmbed)]
#[folder = "../../dashboard/dist"]
struct Assets;

/// Content-Security-Policy for the dashboard: only its own files, no
/// inline scripts, never framed.
const CSP: &str = "default-src 'self'; script-src 'self'; style-src 'self'; img-src 'self' data:; \
                   connect-src 'self'; font-src 'self'; object-src 'none'; base-uri 'none'; \
                   form-action 'self'; frame-ancestors 'none'";

/// Adds security headers to every response.
pub async fn security_headers(req: Request, next: Next) -> Response {
    let mut response = next.run(req).await;
    let headers = response.headers_mut();
    for (name, value) in [
        (header::CONTENT_SECURITY_POLICY, CSP),
        (header::X_CONTENT_TYPE_OPTIONS, "nosniff"),
        (header::X_FRAME_OPTIONS, "DENY"),
        (header::REFERRER_POLICY, "no-referrer"),
    ] {
        headers
            .entry(name)
            .or_insert(HeaderValue::from_static(value));
    }
    headers
        .entry("cross-origin-opener-policy")
        .or_insert(HeaderValue::from_static("same-origin"));
    response
}

/// Serves a dashboard file, or `index.html` for app routes such as
/// `/services`, so reloading any page works.
pub async fn serve(method: Method, uri: Uri) -> Response {
    if method != Method::GET && method != Method::HEAD {
        return StatusCode::METHOD_NOT_ALLOWED.into_response();
    }
    file(uri.path().trim_start_matches('/'))
}

#[cfg(dashboard)]
fn file(path: &str) -> Response {
    let (path, asset) = match Assets::get(path) {
        Some(asset) if !path.is_empty() => (path, asset),
        // A path with an extension is a missing file, not an app route.
        _ if std::path::Path::new(path).extension().is_some() => {
            return StatusCode::NOT_FOUND.into_response();
        }
        _ => match Assets::get("index.html") {
            Some(asset) => ("index.html", asset),
            None => return not_built(),
        },
    };
    // Vite puts content hashes in names under assets/, so those never
    // change; index.html must be checked every time.
    let cache = if path.starts_with("assets/") {
        "public, max-age=31536000, immutable"
    } else {
        "no-cache"
    };
    (
        [
            (header::CONTENT_TYPE, asset.metadata.mimetype().to_owned()),
            (header::CACHE_CONTROL, cache.to_owned()),
        ],
        asset.data.into_owned(),
    )
        .into_response()
}

#[cfg(not(dashboard))]
fn file(_path: &str) -> Response {
    not_built()
}

fn not_built() -> Response {
    (
        StatusCode::SERVICE_UNAVAILABLE,
        [(header::CACHE_CONTROL, "no-cache")],
        Html(
            "<!doctype html><title>Hearthgate</title><h1>Dashboard not included</h1>\
             <p>This build of Hearthgate was made without the dashboard. Build it with \
             <code>npm ci &amp;&amp; npm run build</code> in <code>dashboard/</code>, then \
             rebuild Hearthgate. The API works without it.</p>",
        ),
    )
        .into_response()
}
