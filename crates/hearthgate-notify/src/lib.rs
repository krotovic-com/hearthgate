//! Notifications: webhooks and email for the events worth a person's
//! attention.
//!
//! Subsystems call [`emit`] with an [`Event`]; the daemon's [`run`] task
//! sends it to every configured target that wants its kind. Repeats of the
//! same event (same `key`) are held back for [`REPEAT_AFTER`], and at most
//! [`MAX_PER_HOUR`] events go out per hour, so a flapping service can't
//! flood anyone's phone.

use std::collections::HashMap;
use std::sync::{Arc, OnceLock};
use std::time::{Duration, Instant, SystemTime};

use bytes::Bytes;
use hearthgate_model::duration::format_rfc3339;
use hearthgate_model::{Email, EventKind, Notifications, SmtpSecurity, Webhook, WebhookFormat};
use http_body_util::{BodyExt, Full};
use serde::Serialize;
use tokio::sync::{mpsc, watch};
use tokio_util::sync::CancellationToken;
use tracing::{debug, info, warn};

/// The same event is sent again only after this long.
pub const REPEAT_AFTER: Duration = Duration::from_secs(6 * 3600);
/// At most this many events per hour; more are dropped (and logged).
pub const MAX_PER_HOUR: usize = 20;
const TIMEOUT: Duration = Duration::from_secs(15);

/// How bad it is.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Severity {
    Info,
    Warning,
    Critical,
}

/// Something worth telling someone about.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Event {
    pub kind: EventKind,
    pub severity: Severity,
    /// Identifies the condition, such as `health:app-1:8080`, for holding
    /// back repeats. An event whose key is `KEY:ok` clears `KEY`.
    #[serde(skip)]
    pub key: String,
    pub title: String,
    pub message: String,
}

impl Event {
    #[must_use]
    pub fn new(
        kind: EventKind,
        severity: Severity,
        key: impl Into<String>,
        title: impl Into<String>,
        message: impl Into<String>,
    ) -> Self {
        Self {
            kind,
            severity,
            key: key.into(),
            title: title.into(),
            message: message.into(),
        }
    }
}

static SENDER: OnceLock<mpsc::UnboundedSender<Event>> = OnceLock::new();

/// Queues an event. Does nothing until [`channel`] was called (in tests,
/// the CLI).
pub fn emit(event: Event) {
    if let Some(tx) = SENDER.get() {
        let _ = tx.send(event);
    }
}

/// Installs the global queue. Returns its receiving end, for [`run`], the
/// first time; `None` after.
pub fn channel() -> Option<mpsc::UnboundedReceiver<Event>> {
    let (tx, rx) = mpsc::unbounded_channel();
    SENDER.set(tx).ok().map(|()| rx)
}

/// Holds back repeats and enforces the hourly limit.
#[derive(Debug, Default)]
pub struct Gate {
    sent: HashMap<String, Instant>,
    recent: Vec<Instant>,
}

impl Gate {
    /// Whether `event` should go out now. Records it if so.
    pub fn allow(&mut self, event: &Event, now: Instant) -> bool {
        if let Some(base) = event.key.strip_suffix(":ok") {
            // A recovery only matters when the problem was reported.
            if self.sent.remove(base).is_none() {
                return false;
            }
        } else if self
            .sent
            .get(&event.key)
            .is_some_and(|at| now.duration_since(*at) < REPEAT_AFTER)
        {
            return false;
        }
        self.recent
            .retain(|at| now.duration_since(*at) < Duration::from_secs(3600));
        if self.recent.len() >= MAX_PER_HOUR {
            warn!(
                "more than {MAX_PER_HOUR} notifications in an hour; dropping: {}",
                event.title
            );
            return false;
        }
        self.recent.push(now);
        if !event.key.ends_with(":ok") {
            self.sent.insert(event.key.clone(), now);
        }
        true
    }
}

/// Sends queued events until `stop`.
pub async fn run(
    mut events: mpsc::UnboundedReceiver<Event>,
    settings: watch::Receiver<Arc<Notifications>>,
    stop: CancellationToken,
) {
    let mut gate = Gate::default();
    loop {
        let event = tokio::select! {
            () = stop.cancelled() => return,
            e = events.recv() => match e { Some(e) => e, None => return },
        };
        let settings = settings.borrow().clone();
        if !settings.any() || !settings.wants(event.kind) {
            debug!(title = %event.title, "notification not configured for this event");
            continue;
        }
        if !gate.allow(&event, Instant::now()) {
            continue;
        }
        for (target, result) in send(&settings, &event).await {
            match result {
                Ok(()) => info!("notification sent to {target}: {}", event.title),
                Err(e) => warn!("cannot send a notification to {target}: {e}"),
            }
        }
    }
}

/// Sends `event` to every target now, without the gate. Returns each
/// target's result.
pub async fn send(settings: &Notifications, event: &Event) -> Vec<(String, Result<(), String>)> {
    let mut out = Vec::new();
    for hook in &settings.webhooks {
        out.push((format!("webhook {}", hook.name), webhook(hook, event).await));
    }
    if let Some(email) = &settings.email {
        out.push((
            format!("email to {}", email.to.join(", ")),
            mail(email, event).await,
        ));
    }
    out
}

/// A test event, for checking the settings.
#[must_use]
pub fn test_event() -> Event {
    Event::new(
        EventKind::Updates,
        Severity::Info,
        "test",
        "Test notification from Hearthgate",
        "Notifications work. You'll get messages like this one when a certificate can't be \
         renewed, a service stops answering, a firewall change is undone, a backup fails or a \
         new release is out.",
    )
}

#[derive(Serialize)]
struct Payload<'a> {
    #[serde(flatten)]
    event: &'a Event,
    time: String,
    host: &'a str,
    /// Slack and Mattermost show this.
    text: String,
    /// Discord shows this.
    content: String,
}

fn hostname() -> String {
    std::fs::read_to_string("/proc/sys/kernel/hostname")
        .map(|h| h.trim().to_owned())
        .unwrap_or_default()
}

async fn webhook(hook: &Webhook, event: &Event) -> Result<(), String> {
    use hyper_util::client::legacy::Client;
    use hyper_util::rt::TokioExecutor;
    let host = hostname();
    let (body, content_type) = match hook.format {
        WebhookFormat::Json => {
            let full = format!("{}\n{}", event.title, event.message);
            let payload = Payload {
                event,
                time: format_rfc3339(SystemTime::now()),
                host: &host,
                text: full.clone(),
                content: full,
            };
            (
                serde_json::to_vec(&payload).map_err(|e| e.to_string())?,
                "application/json",
            )
        }
        WebhookFormat::Ntfy => (event.message.clone().into_bytes(), "text/plain"),
    };
    let mut req = http::Request::post(&hook.url)
        .header(http::header::CONTENT_TYPE, content_type)
        .header(
            http::header::USER_AGENT,
            concat!("hearthgate/", env!("CARGO_PKG_VERSION")),
        );
    if hook.format == WebhookFormat::Ntfy {
        let priority = match event.severity {
            Severity::Info => "default",
            Severity::Warning => "high",
            Severity::Critical => "urgent",
        };
        // Header values must be ASCII; ntfy reads RFC 2047 for the rest.
        let title: String = event
            .title
            .chars()
            .map(|c| {
                if c.is_ascii() && !c.is_ascii_control() {
                    c
                } else {
                    '?'
                }
            })
            .collect();
        req = req
            .header("Title", title)
            .header("Priority", priority)
            .header("Tags", "hearthgate");
    }
    let req = req
        .body(Full::new(Bytes::from(body)))
        .map_err(|e| format!("bad URL: {e}"))?;
    let connector = hyper_rustls::HttpsConnectorBuilder::new()
        .with_native_roots()
        .map_err(|e| format!("no trusted certificates on this system: {e}"))?
        .https_or_http()
        .enable_http1()
        .build();
    let client: Client<_, Full<Bytes>> = Client::builder(TokioExecutor::new()).build(connector);
    let resp = tokio::time::timeout(TIMEOUT, client.request(req))
        .await
        .map_err(|_| "timed out".to_owned())?
        .map_err(|e| e.to_string())?;
    let status = resp.status();
    // Drain the body so the connection closes cleanly.
    let _ = tokio::time::timeout(TIMEOUT, resp.into_body().collect()).await;
    if status.is_success() {
        Ok(())
    } else {
        Err(format!("the server answered {status}"))
    }
}

async fn mail(email: &Email, event: &Event) -> Result<(), String> {
    use lettre::message::{Mailbox, header::ContentType};
    use lettre::transport::smtp::authentication::Credentials;
    use lettre::transport::smtp::client::{Tls, TlsParameters};
    use lettre::{AsyncSmtpTransport, AsyncTransport, Message, Tokio1Executor};

    let (host, port) = hearthgate_model::split_endpoint(&email.server)
        .ok_or_else(|| format!("bad server {}", email.server))?;
    let port = port.unwrap_or_else(|| email.security.default_port());
    let from: Mailbox = email
        .from
        .parse()
        .map_err(|e| format!("bad sender {}: {e}", email.from))?;
    let mut builder = Message::builder()
        .from(from)
        .subject(format!("[Hearthgate] {}", event.title))
        .header(ContentType::TEXT_PLAIN);
    for to in &email.to {
        builder = builder.to(to.parse().map_err(|e| format!("bad recipient {to}: {e}"))?);
    }
    let host_name = hostname();
    let body = format!(
        "{}\n\n{}\nServer: {}\nTime: {}\n",
        event.title,
        event.message,
        if host_name.is_empty() {
            "unknown"
        } else {
            &host_name
        },
        format_rfc3339(SystemTime::now())
    );
    let message = builder.body(body).map_err(|e| e.to_string())?;

    let tls = || TlsParameters::new(host.to_owned()).map_err(|e| e.to_string());
    let mut transport = AsyncSmtpTransport::<Tokio1Executor>::builder_dangerous(host)
        .port(port)
        .timeout(Some(TIMEOUT));
    transport = match email.security {
        SmtpSecurity::Starttls => transport.tls(Tls::Required(tls()?)),
        SmtpSecurity::Tls => transport.tls(Tls::Wrapper(tls()?)),
        SmtpSecurity::None => transport.tls(Tls::None),
    };
    if let Some(user) = &email.username {
        let password = match &email.password_file {
            Some(path) => std::fs::read_to_string(path)
                .map_err(|e| format!("cannot read the SMTP password from {path}: {e}"))?
                .trim_end_matches(['\r', '\n'])
                .to_owned(),
            None => String::new(),
        };
        transport = transport.credentials(Credentials::new(user.clone(), password));
    }
    transport
        .build()
        .send(message)
        .await
        .map(|_| ())
        .map_err(|e| e.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn event(key: &str) -> Event {
        Event::new(EventKind::Health, Severity::Warning, key, "t", "m")
    }

    #[test]
    fn holds_back_repeats_and_floods() {
        let mut gate = Gate::default();
        let t0 = Instant::now();
        assert!(gate.allow(&event("down:a"), t0));
        assert!(!gate.allow(&event("down:a"), t0 + Duration::from_secs(60)));
        assert!(gate.allow(&event("down:a"), t0 + REPEAT_AFTER));
        // A recovery goes out once, and only after a reported problem.
        assert!(gate.allow(&event("down:a:ok"), t0 + REPEAT_AFTER));
        assert!(!gate.allow(&event("down:a:ok"), t0 + REPEAT_AFTER));
        assert!(!gate.allow(&event("down:b:ok"), t0));
        // After a recovery the problem is news again.
        assert!(gate.allow(&event("down:a"), t0 + REPEAT_AFTER));

        let mut gate = Gate::default();
        let sent = (0..50)
            .filter(|i| gate.allow(&event(&format!("k{i}")), t0))
            .count();
        assert_eq!(sent, MAX_PER_HOUR);
        assert!(gate.allow(&event("later"), t0 + Duration::from_secs(3601)));
    }
}
