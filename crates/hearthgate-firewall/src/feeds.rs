//! Blocklist feeds: download, parse, cache and refresh.
//!
//! Each subscribed blocklist (and each blocked country) is downloaded over
//! HTTPS, parsed into networks and kept in `<data dir>/blocklists/` so a
//! restart does not need the network. Failed downloads keep the previous
//! copy and are retried later.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, SystemTime};

use bytes::Bytes;
use hearthgate_model::{Firewall, country_feeds};
use http_body_util::{BodyExt, Empty, Limited};
use ipnet::IpNet;
use tokio::sync::watch;
use tokio_util::sync::CancellationToken;
use tracing::{debug, info, warn};

use crate::Perimeter;

/// Largest download accepted.
const MAX_BYTES: usize = 64 * 1024 * 1024;
/// Most networks taken from one feed.
const MAX_ENTRIES: usize = 1_000_000;
/// How long a download may take.
const DOWNLOAD_TIMEOUT: Duration = Duration::from_secs(120);
/// Wait after a failed download.
const RETRY: Duration = Duration::from_secs(15 * 60);
/// Country lists change slowly.
const COUNTRY_REFRESH: Duration = Duration::from_secs(7 * 24 * 3600);

/// A feed to keep loaded.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Source {
    pub name: String,
    pub urls: Vec<String>,
    pub refresh: Duration,
}

/// The feeds a firewall setting needs.
#[must_use]
pub fn sources(firewall: &Firewall) -> Vec<Source> {
    let mut out: Vec<Source> = firewall
        .blocklists
        .iter()
        .filter(|b| b.enabled)
        .map(|b| Source {
            name: b.name.clone(),
            urls: vec![b.url.clone()],
            refresh: b.refresh,
        })
        .collect();
    for country in &firewall.countries {
        out.push(Source {
            name: format!("country-{country}"),
            urls: country_feeds(country).to_vec(),
            refresh: COUNTRY_REFRESH,
        });
    }
    out
}

/// Parses a feed: one address or network per line. Understands comments
/// (`#`, `;`, `//`), trailing notes after the address, and the JSON lines
/// Spamhaus publishes (`{"cidr":"1.2.3.0/24",…}`). Lines it cannot read are
/// skipped.
#[must_use]
pub fn parse(text: &str) -> Vec<IpNet> {
    let mut out = Vec::new();
    for line in text.lines() {
        if out.len() >= MAX_ENTRIES {
            break;
        }
        let line = line.trim();
        if line.is_empty()
            || line.starts_with('#')
            || line.starts_with(';')
            || line.starts_with("//")
        {
            continue;
        }
        let token = if line.starts_with('{') {
            match json_cidr(line) {
                Some(t) => t,
                None => continue,
            }
        } else {
            line.split(|c: char| c.is_whitespace() || matches!(c, ';' | ',' | '#'))
                .next()
                .unwrap_or("")
        };
        if let Ok(net) = token.parse::<IpNet>() {
            out.push(net.trunc());
        } else if let Ok(ip) = token.parse::<std::net::IpAddr>() {
            out.push(IpNet::from(ip));
        }
    }
    IpNet::aggregate(&out)
}

fn json_cidr(line: &str) -> Option<&str> {
    let start = line.find("\"cidr\"")? + 6;
    let rest = line[start..].trim_start().strip_prefix(':')?.trim_start();
    let rest = rest.strip_prefix('"')?;
    rest.split('"').next()
}

/// Downloads `url` (HTTPS only, at most 5 redirects).
pub async fn download(url: &str) -> Result<String, String> {
    use hyper_util::client::legacy::Client;
    use hyper_util::rt::TokioExecutor;
    let connector = hyper_rustls::HttpsConnectorBuilder::new()
        .with_native_roots()
        .map_err(|e| format!("no trusted certificates on this system: {e}"))?
        .https_only()
        .enable_http1()
        .build();
    let client: Client<_, Empty<Bytes>> = Client::builder(TokioExecutor::new()).build(connector);
    let mut url = url.to_owned();
    for _ in 0..6 {
        let uri: http::Uri = url.parse().map_err(|e| format!("bad URL {url}: {e}"))?;
        let req = http::Request::get(uri.clone())
            .header(
                http::header::USER_AGENT,
                concat!("hearthgate/", env!("CARGO_PKG_VERSION")),
            )
            .body(Empty::new())
            .map_err(|e| e.to_string())?;
        let resp = tokio::time::timeout(DOWNLOAD_TIMEOUT, client.request(req))
            .await
            .map_err(|_| "timed out".to_owned())?
            .map_err(|e| format!("{e}"))?;
        let status = resp.status();
        if status.is_redirection() {
            let location = resp
                .headers()
                .get(http::header::LOCATION)
                .and_then(|v| v.to_str().ok())
                .ok_or("redirect without a location")?;
            url = resolve_location(&uri, location);
            continue;
        }
        if !status.is_success() {
            return Err(format!("the server answered {status}"));
        }
        let body = tokio::time::timeout(
            DOWNLOAD_TIMEOUT,
            Limited::new(resp.into_body(), MAX_BYTES).collect(),
        )
        .await
        .map_err(|_| "timed out".to_owned())?
        .map_err(|e| format!("{e}"))?
        .to_bytes();
        return Ok(String::from_utf8_lossy(&body).into_owned());
    }
    Err("too many redirects".into())
}

fn resolve_location(base: &http::Uri, location: &str) -> String {
    if location.starts_with("https://") || location.starts_with("http://") {
        location.to_owned()
    } else {
        format!(
            "{}://{}{location}",
            base.scheme_str().unwrap_or("https"),
            base.authority().map_or("", http::uri::Authority::as_str)
        )
    }
}

/// A cached feed on disk.
struct Cached {
    urls: Vec<String>,
    updated: SystemTime,
    nets: Vec<IpNet>,
}

fn cache_path(dir: &Path, name: &str) -> PathBuf {
    dir.join(format!("{name}.txt"))
}

fn read_cache(dir: &Path, name: &str) -> Option<Cached> {
    let path = cache_path(dir, name);
    let text = std::fs::read_to_string(&path).ok()?;
    let updated = std::fs::metadata(&path).ok()?.modified().ok()?;
    let urls = text
        .lines()
        .take_while(|l| l.starts_with("# source "))
        .map(|l| l.trim_start_matches("# source ").to_owned())
        .collect();
    Some(Cached {
        urls,
        updated,
        nets: parse(&text),
    })
}

fn write_cache(dir: &Path, name: &str, urls: &[String], nets: &[IpNet]) -> std::io::Result<()> {
    use std::fmt::Write as _;
    std::fs::create_dir_all(dir)?;
    let mut text = String::with_capacity(nets.len() * 18 + 128);
    for url in urls {
        let _ = writeln!(text, "# source {url}");
    }
    for net in nets {
        let _ = writeln!(text, "{net}");
    }
    let tmp = dir.join(format!(".{name}.tmp"));
    std::fs::write(&tmp, text)?;
    std::fs::rename(tmp, cache_path(dir, name))
}

fn unix(t: SystemTime) -> u64 {
    t.duration_since(SystemTime::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs())
}

/// Keeps the subscribed feeds loaded until `stop`.
pub async fn run(
    perimeter: Arc<Perimeter>,
    dir: PathBuf,
    mut settings: watch::Receiver<Firewall>,
    stop: CancellationToken,
) {
    // Per feed: the URLs loaded and when to check again.
    let mut loaded: HashMap<String, (Vec<String>, SystemTime)> = HashMap::new();
    let mut tick = tokio::time::interval(Duration::from_secs(60));
    loop {
        let firewall = settings.borrow_and_update().clone();
        let wanted = if firewall.enabled {
            sources(&firewall)
        } else {
            Vec::new()
        };
        let names: Vec<String> = wanted.iter().map(|s| s.name.clone()).collect();
        perimeter.retain_feeds(&names);
        loaded.retain(|k, _| names.contains(k));
        for source in &wanted {
            let now = SystemTime::now();
            let due = match loaded.get(&source.name) {
                Some((urls, next)) => *urls != source.urls || now >= *next,
                None => true,
            };
            if !due {
                continue;
            }
            // From the cache first, when it is for the same URLs.
            if !loaded.contains_key(&source.name) {
                let dir = dir.clone();
                let name = source.name.clone();
                let cached = tokio::task::spawn_blocking(move || read_cache(&dir, &name))
                    .await
                    .ok()
                    .flatten();
                if let Some(c) = cached.filter(|c| c.urls == source.urls) {
                    let next = c.updated + source.refresh;
                    debug!(feed = %source.name, entries = c.nets.len(), "blocklist loaded from cache");
                    perimeter
                        .set_feed(&source.name, c.nets, unix(c.updated))
                        .await;
                    loaded.insert(source.name.clone(), (source.urls.clone(), next));
                    if now < next {
                        continue;
                    }
                }
            }
            let mut nets = Vec::new();
            let mut failure = None;
            for url in &source.urls {
                match download(url).await {
                    Ok(text) => nets.extend(parse(&text)),
                    Err(e) => {
                        failure = Some(format!("{url}: {e}"));
                        break;
                    }
                }
            }
            if let Some(e) = failure {
                warn!(feed = %source.name, "cannot download the blocklist: {e}");
                perimeter.feed_failed(&source.name, e);
                let urls = loaded
                    .get(&source.name)
                    .map_or_else(|| source.urls.clone(), |(u, _)| u.clone());
                loaded.insert(source.name.clone(), (urls, now + RETRY));
                continue;
            }
            let nets = IpNet::aggregate(&nets);
            info!(feed = %source.name, entries = nets.len(), "blocklist updated");
            {
                let (dir, name, urls, nets) = (
                    dir.clone(),
                    source.name.clone(),
                    source.urls.clone(),
                    nets.clone(),
                );
                let written =
                    tokio::task::spawn_blocking(move || write_cache(&dir, &name, &urls, &nets))
                        .await;
                if let Ok(Err(e)) = written {
                    warn!(feed = %source.name, "cannot cache the blocklist: {e}");
                }
            }
            perimeter.set_feed(&source.name, nets, unix(now)).await;
            loaded.insert(
                source.name.clone(),
                (source.urls.clone(), now + source.refresh),
            );
        }
        tokio::select! {
            () = stop.cancelled() => return,
            _ = tick.tick() => {}
            r = settings.changed() => if r.is_err() { return },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_common_formats() {
        let text = "\
# FireHOL style
1.2.3.0/24
5.6.7.8
; Spamhaus text style
9.9.9.0/24 ; SBL123
{\"cidr\":\"10.20.0.0/16\",\"sblid\":\"SBL1\",\"rir\":\"ripencc\"}
{\"type\":\"metadata\",\"timestamp\":1}
2001:db8::/32
not an address
11.0.0.1/8
";
        let nets = parse(text);
        let shown: Vec<String> = nets.iter().map(ToString::to_string).collect();
        assert_eq!(
            shown,
            [
                "1.2.3.0/24",
                "5.6.7.8/32",
                "9.9.9.0/24",
                "10.20.0.0/16",
                "11.0.0.0/8",
                "2001:db8::/32"
            ]
        );
    }

    #[test]
    fn cache_round_trip() {
        let dir = tempfile::tempdir().unwrap();
        let urls = vec!["https://example.com/list.txt".to_owned()];
        let nets = parse("1.2.3.0/24\n4.5.6.7\n");
        write_cache(dir.path(), "x", &urls, &nets).unwrap();
        let cached = read_cache(dir.path(), "x").unwrap();
        assert_eq!(cached.urls, urls);
        assert_eq!(cached.nets, nets);
    }

    #[test]
    fn countries_become_feeds() {
        let fw = Firewall {
            countries: vec!["cn".into()],
            blocklists: vec![hearthgate_model::Blocklist {
                name: "a".into(),
                url: "https://example.com/a".into(),
                refresh: Duration::from_secs(3600),
                enabled: false,
            }],
            ..Firewall::default()
        };
        let s = sources(&fw);
        assert_eq!(s.len(), 1);
        assert_eq!(s[0].name, "country-cn");
        assert!(s[0].urls[0].contains("cn-aggregated"));
    }
}
