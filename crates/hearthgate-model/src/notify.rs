//! Notifications: where Hearthgate reports certificate, health, security,
//! backup and update events.

use serde::{Deserialize, Serialize};

/// What to report, and where.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(deny_unknown_fields)]
pub struct Notifications {
    /// Kinds of events to send. Empty means all of them.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub events: Vec<EventKind>,
    /// HTTP endpoints that get each event as JSON.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub webhooks: Vec<Webhook>,
    /// Email through an SMTP server.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub email: Option<Email>,
}

impl Notifications {
    #[must_use]
    pub fn is_default(&self) -> bool {
        *self == Self::default()
    }

    /// Whether events of `kind` are sent.
    #[must_use]
    pub fn wants(&self, kind: EventKind) -> bool {
        self.events.is_empty() || self.events.contains(&kind)
    }

    /// Whether anything is configured to receive events.
    #[must_use]
    pub fn any(&self) -> bool {
        !self.webhooks.is_empty() || self.email.is_some()
    }
}

/// A kind of event.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(rename_all = "lowercase")]
pub enum EventKind {
    /// A certificate could not be issued or renewed.
    Certificates,
    /// A service stopped answering its health checks, or came back.
    Health,
    /// A firewall change was undone, or the firewall failed.
    Security,
    /// A scheduled backup failed.
    Backups,
    /// A new Hearthgate release is out.
    Updates,
}

impl EventKind {
    pub const ALL: [Self; 5] = [
        Self::Certificates,
        Self::Health,
        Self::Security,
        Self::Backups,
        Self::Updates,
    ];
}

/// An HTTP endpoint for events. Hearthgate POSTs JSON with `title`,
/// `message`, `kind` and `severity`, plus `text` and `content` with the
/// whole message, which Slack, Mattermost and Discord show as is.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(deny_unknown_fields)]
pub struct Webhook {
    pub name: String,
    /// The URL. It often holds a secret (Slack, Discord, ntfy topics), and
    /// is stored with the state like everything else.
    pub url: String,
    /// `json` (default) or `ntfy`, which sends the message as plain text
    /// with ntfy's title and priority headers.
    #[serde(default, skip_serializing_if = "WebhookFormat::is_default")]
    pub format: WebhookFormat,
}

/// Body format of a webhook.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(rename_all = "lowercase")]
pub enum WebhookFormat {
    #[default]
    Json,
    Ntfy,
}

impl WebhookFormat {
    #[must_use]
    pub fn is_default(&self) -> bool {
        *self == Self::default()
    }
}

/// Email settings.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(deny_unknown_fields)]
pub struct Email {
    /// SMTP server as `host` or `host:port` (default port 587, or 465 with
    /// `security = "tls"`).
    pub server: String,
    #[serde(default, skip_serializing_if = "SmtpSecurity::is_default")]
    pub security: SmtpSecurity,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub username: Option<String>,
    /// File holding the SMTP password, so it stays out of the state.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub password_file: Option<String>,
    pub from: String,
    pub to: Vec<String>,
}

/// How the SMTP connection is protected.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(rename_all = "lowercase")]
pub enum SmtpSecurity {
    /// Plain connection upgraded with STARTTLS, which must succeed.
    #[default]
    Starttls,
    /// TLS from the start (usually port 465).
    Tls,
    /// No encryption, for a relay on this machine or the LAN.
    None,
}

impl SmtpSecurity {
    #[must_use]
    pub fn is_default(&self) -> bool {
        *self == Self::default()
    }

    #[must_use]
    pub fn default_port(self) -> u16 {
        match self {
            Self::Tls => 465,
            Self::Starttls => 587,
            Self::None => 25,
        }
    }
}

/// Whether `text` looks like an email address. Deliberately loose: the
/// server decides.
#[must_use]
pub fn is_email(text: &str) -> bool {
    text.split_once('@').is_some_and(|(user, domain)| {
        !user.is_empty()
            && domain.contains('.')
            && !text.contains(char::is_whitespace)
            && !text.contains(['<', '>', ',', '\r', '\n'])
    })
}
