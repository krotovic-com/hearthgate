//! Downloads from GitHub, and nowhere else.

use std::future::Future;
use std::pin::Pin;
use std::time::Duration;

use bytes::Bytes;
use http_body_util::{BodyExt, Empty, Limited};

/// A future that resolves to a download.
pub type Download<'a> = Pin<Box<dyn Future<Output = Result<Vec<u8>, String>> + Send + 'a>>;

/// Something that can download a URL. Tests use a fake.
pub trait Fetch: Send + Sync {
    /// Downloads `url`, refusing answers larger than `max` bytes.
    fn get(&self, url: &str, max: u64) -> Download<'_>;
}

/// Hosts downloads may come from, including the redirects GitHub sends
/// asset downloads through.
#[must_use]
pub fn allowed_host(host: &str) -> bool {
    host == "github.com"
        || host == "api.github.com"
        || host.ends_with(".githubusercontent.com")
}

/// HTTPS downloads with the system's trusted certificates.
#[derive(Debug, Default)]
pub struct HttpFetch;

const CONNECT_TIMEOUT: Duration = Duration::from_secs(30);
const BODY_TIMEOUT: Duration = Duration::from_secs(600);

impl Fetch for HttpFetch {
    fn get(&self, url: &str, max: u64) -> Download<'_> {
        let url = url.to_owned();
        Box::pin(async move { download(&url, max).await })
    }
}

async fn download(url: &str, max: u64) -> Result<Vec<u8>, String> {
    use hyper_util::client::legacy::Client;
    use hyper_util::rt::TokioExecutor;
    let connector = hyper_rustls::HttpsConnectorBuilder::new()
        .with_native_roots()
        .map_err(|e| format!("no trusted certificates on this system: {e}"))?
        .https_only()
        .enable_http1()
        .build();
    let client: Client<_, Empty<Bytes>> = Client::builder(TokioExecutor::new()).build(connector);
    let max = usize::try_from(max).unwrap_or(usize::MAX);
    let mut url = url.to_owned();
    for _ in 0..6 {
        let uri: http::Uri = url.parse().map_err(|e| format!("bad URL {url}: {e}"))?;
        if uri.scheme_str() != Some("https") || !uri.host().is_some_and(allowed_host) {
            return Err(format!("refusing to download from {url}"));
        }
        let req = http::Request::get(uri.clone())
            .header(http::header::USER_AGENT, concat!("hearthgate/", env!("CARGO_PKG_VERSION")))
            .header(http::header::ACCEPT, "application/vnd.github+json, application/octet-stream")
            .body(Empty::new())
            .map_err(|e| e.to_string())?;
        let resp = tokio::time::timeout(CONNECT_TIMEOUT, client.request(req))
            .await
            .map_err(|_| "GitHub did not answer in time".to_owned())?
            .map_err(|e| format!("cannot reach GitHub: {e}"))?;
        let status = resp.status();
        if status.is_redirection() {
            let location = resp
                .headers()
                .get(http::header::LOCATION)
                .and_then(|v| v.to_str().ok())
                .ok_or("redirect without a location")?;
            url = if location.starts_with("https://") {
                location.to_owned()
            } else {
                format!("https://{}{location}", uri.host().unwrap_or_default())
            };
            continue;
        }
        if status == http::StatusCode::FORBIDDEN || status == http::StatusCode::TOO_MANY_REQUESTS {
            return Err("GitHub is rate limiting this address; try again in an hour".into());
        }
        if !status.is_success() {
            return Err(format!("GitHub answered {status}"));
        }
        let body = tokio::time::timeout(BODY_TIMEOUT, Limited::new(resp.into_body(), max).collect())
            .await
            .map_err(|_| "the download took too long".to_owned())?
            .map_err(|e| format!("download failed: {e}"))?
            .to_bytes();
        return Ok(body.to_vec());
    }
    Err("too many redirects".into())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_github() {
        assert!(allowed_host("github.com"));
        assert!(allowed_host("api.github.com"));
        assert!(allowed_host("release-assets.githubusercontent.com"));
        assert!(!allowed_host("githubusercontent.com.evil.example"));
        assert!(!allowed_host("evilgithub.com"));
        assert!(!allowed_host("example.com"));
    }

    #[tokio::test]
    async fn refuses_other_hosts_and_plain_http() {
        assert!(HttpFetch.get("https://example.com/x", 10).await.unwrap_err().contains("refusing"));
        assert!(HttpFetch.get("http://github.com/x", 10).await.unwrap_err().contains("refusing"));
    }
}
