# Notifications

Hearthgate can tell you when something needs attention: a certificate that can't be renewed, a service that fails its health checks for a minute (and when it's back), a firewall change undone because nobody confirmed it, a failed backup, or a new release. Each problem is sent once, and again after 6 hours if it persists; at most 20 messages go out per hour. Set it up under Settings, Notifications, which can also send a test message, or in the state:

```toml
[notifications]
# events = ["certificates", "health", "security", "backups", "updates"]   # default: all

[[notifications.webhooks]]
name = "phone"
url = "https://ntfy.sh/your-secret-topic"
format = "ntfy"            # or "json" (default): Slack, Mattermost, Discord and scripts

[notifications.email]
server = "smtp.example.com"          # port 587 with STARTTLS by default
# security = "tls"                   # TLS from the start, port 465; "none" for a local relay
username = "hearthgate@example.com"
password_file = "/etc/hearthgate/smtp-password"
from = "hearthgate@example.com"
to = ["you@example.com"]
```

JSON webhooks get `title`, `message`, `kind`, `severity`, `time` and `host`, plus `text` and `content` with the whole message. Webhook URLs often contain a secret and are stored with the state; the SMTP password stays in its file (`chown root:hearthgate`, `chmod 0640`).

**Release checks.** Once a day Hearthgate asks GitHub whether a newer release exists, logs it and sends an `updates` notification. It never installs anything. Turn the check off with `updates.check = false` in the bootstrap file.
