//! Turns what the daemon sees into notifications (see `hearthgate-notify`).

use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::time::{Duration, Instant};

use hearthgate_model::EventKind;
use hearthgate_notify::{Event, Severity, emit};
use hearthgate_proxy::Gateway;
use tokio_util::sync::CancellationToken;

/// A service must fail its health checks this long before anyone is told,
/// so a restart or a short blip stays quiet.
const DOWN_FOR: Duration = Duration::from_secs(60);
const CHECK_EVERY: Duration = Duration::from_secs(15);

/// Reports services that stop answering their health checks, and when they
/// come back.
pub async fn watch_health(gateway: Arc<Gateway>, stop: CancellationToken) {
    let mut down_since: HashMap<(String, u16), Instant> = HashMap::new();
    let mut reported: HashSet<(String, u16)> = HashSet::new();
    loop {
        tokio::select! {
            () = stop.cancelled() => return,
            () = tokio::time::sleep(CHECK_EVERY) => {}
        }
        let now = Instant::now();
        let upstreams = gateway.upstreams().all();
        let present: HashSet<(String, u16)> = upstreams
            .iter()
            .map(|u| (u.service.clone(), u.port))
            .collect();
        down_since.retain(|k, _| present.contains(k));
        reported.retain(|k| present.contains(k));
        for upstream in upstreams {
            let key = (upstream.service.clone(), upstream.port);
            let name = format!("{} (port {})", upstream.service, upstream.port);
            let event_key = format!("health:{}:{}", upstream.service, upstream.port);
            if upstream.healthy() {
                down_since.remove(&key);
                if reported.remove(&key) {
                    emit(Event::new(
                        EventKind::Health,
                        Severity::Info,
                        format!("{event_key}:ok"),
                        format!("{name} is back"),
                        format!("{name} passes its health checks again."),
                    ));
                }
            } else {
                let since = *down_since.entry(key.clone()).or_insert(now);
                if now.duration_since(since) >= DOWN_FOR && reported.insert(key) {
                    emit(Event::new(
                        EventKind::Health,
                        Severity::Warning,
                        event_key,
                        format!("{name} is down"),
                        format!(
                            "{name} has failed its health checks for over a minute. Requests go \
                             to other targets of its routes, if there are any. Check the service, \
                             and the Services page of the dashboard."
                        ),
                    ));
                }
            }
        }
    }
}

/// Reports a certificate that could not be issued or renewed.
pub fn certificate_failed(interface: &str, error: &str) {
    emit(Event::new(
        EventKind::Certificates,
        Severity::Warning,
        format!("certificate:{interface}"),
        format!("The certificate of {interface} could not be renewed"),
        format!(
            "Requesting a certificate for the interface {interface} failed: {error}\n\
             Hearthgate keeps retrying. An existing certificate keeps working until it expires."
        ),
    ));
}
