//! Checks GitHub for a newer Hearthgate release. Only asks; never updates
//! anything.

use std::time::Duration;

use hearthgate_model::EventKind;
use hearthgate_notify::{Event, Severity, emit};
use tokio_util::sync::CancellationToken;
use tracing::{debug, info};

const LATEST: &str = "https://api.github.com/repos/krotovic-com/hearthgate/releases/latest";
const EVERY: Duration = Duration::from_secs(24 * 3600);

/// The newest release's version, such as `1.0.1`.
pub async fn latest() -> Result<String, String> {
    let text = hearthgate_firewall::feeds::download(LATEST).await?;
    let json: serde_json::Value =
        serde_json::from_str(&text).map_err(|e| format!("unexpected answer from GitHub: {e}"))?;
    json["tag_name"]
        .as_str()
        .map(|t| t.trim_start_matches('v').to_owned())
        .ok_or_else(|| "GitHub listed no release".to_owned())
}

fn parse(version: &str) -> Option<(u64, u64, u64)> {
    let core = version.split(['-', '+']).next()?;
    let mut parts = core.split('.').map(|p| p.parse::<u64>().ok());
    Some((parts.next()??, parts.next()??, parts.next()??))
}

/// Whether `latest` is a newer release than `current`. Development builds
/// (`0.0.0-dev`) never are behind.
#[must_use]
pub fn is_newer(current: &str, latest: &str) -> bool {
    if current.contains("-dev") {
        return false;
    }
    match (parse(current), parse(latest)) {
        (Some(c), Some(l)) => l > c,
        _ => false,
    }
}

/// Checks once a day, starting a few minutes after start.
pub async fn run(stop: CancellationToken) {
    if crate::VERSION.contains("-dev") {
        return;
    }
    let mut wait = Duration::from_secs(300);
    loop {
        tokio::select! {
            () = stop.cancelled() => return,
            () = tokio::time::sleep(wait) => {}
        }
        wait = EVERY;
        match latest().await {
            Ok(latest) if is_newer(crate::VERSION, &latest) => {
                info!(
                    "Hearthgate {latest} is available (this is {}): \
                     https://github.com/krotovic-com/hearthgate/releases",
                    crate::VERSION
                );
                emit(Event::new(
                    EventKind::Updates,
                    Severity::Info,
                    format!("update:{latest}"),
                    format!("Hearthgate {latest} is available"),
                    format!(
                        "This server runs Hearthgate {}. See what's new and how to upgrade at \
                         https://github.com/krotovic-com/hearthgate/releases/tag/v{latest}",
                        crate::VERSION
                    ),
                ));
            }
            Ok(_) => debug!("Hearthgate is up to date"),
            Err(e) => debug!("cannot check for updates: {e}"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn compares_versions() {
        assert!(is_newer("1.0.0", "1.0.1"));
        assert!(is_newer("0.9.9", "1.0.0"));
        assert!(is_newer("1.2.3", "1.10.0"));
        assert!(!is_newer("1.0.0", "1.0.0"));
        assert!(!is_newer("1.1.0", "1.0.9"));
        assert!(!is_newer("0.0.0-dev", "9.0.0"));
        assert!(!is_newer("1.0.0", "garbage"));
    }
}
