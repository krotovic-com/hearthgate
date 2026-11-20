# Hearthgate user guide

This guide covers what Hearthgate 0.3 does: the dashboard, public interfaces, services, routes, load balancing, health checks, certificates, the command line and the admin API. Later releases add the firewall, VPN and managed Docker (see the [roadmap](ROADMAP.md)).

## How it fits together

- A **public interface** is what the internet can reach: one or more domains and the ports they listen on.
- A **service** is something inside your network that Hearthgate can forward to: a program on the host (such as sshd) or an existing Docker container.
- A **route** connects a port of an interface to one or more service ports. One target forwards. Two or more are load-balanced.

Nothing listens on any port until an interface asks for it.

Everything you configure is the **desired state**. It is stored in a SQLite database as numbered **revisions**. Every change creates a new revision, so you always have the history and can roll back. A running gateway applies a new revision within a second, without a restart, and without dropping open connections on ports that did not change.

There are two files:

| File | What it holds | How you change it |
|---|---|---|
| `/etc/hearthgate/hearthgate.toml` | Bootstrap settings: data directory, log level, admin API, Docker socket | Edit it and restart the service |
| A desired-state file, for example `state.toml` | Interfaces, services, routes, certificates settings | The dashboard, or `hearthgate apply state.toml` |

The desired-state file is only an input. After `apply`, the database is the source of truth, and `hearthgate export` prints it back. You don't need a file at all if you use the dashboard.

## Install

Download a release for your architecture as described in the [README](../README.md), then:

```sh
sudo install -m 0755 hearthgate /usr/local/bin/hearthgate
sudo install -m 0644 hearthgate.sysusers /usr/lib/sysusers.d/hearthgate.conf
sudo systemd-sysusers
sudo install -m 0644 hearthgate.service /etc/systemd/system/hearthgate.service
sudo install -d -m 0755 /etc/hearthgate
sudo install -m 0644 examples/hearthgate.toml /etc/hearthgate/hearthgate.toml
sudo systemctl daemon-reload
sudo systemctl enable --now hearthgate
```

The service runs as the unprivileged `hearthgate` user with only the capabilities it needs to bind ports below 1024. Its data lives in `/var/lib/hearthgate`.

Run CLI commands as root or as the `hearthgate` user, because they open the database in the data directory:

```sh
sudo -u hearthgate hearthgate apply state.toml
```

## Dashboard

Open `https://SERVER-LAN-ADDRESS:7380/` from a computer on the same network. Hearthgate finds the server's LAN addresses when it starts, listens on those and on loopback, and closes connections from anywhere else before reading a byte, so the dashboard is never reachable from the internet.

**First visit.** The dashboard asks for a setup code, a username and a password (at least 12 characters). The code proves you have access to the server. It is in the service log, or print it with:

```sh
sudo -u hearthgate hearthgate setup-code
```

The code stops working once the account exists.

**The certificate warning.** The dashboard uses HTTPS with a self-signed certificate made on first start, so your browser warns about it once. Before you accept it, compare its SHA-256 fingerprint (in the browser's certificate details) with the one Hearthgate logged:

```sh
journalctl -u hearthgate | grep fingerprint
```

They must match. To avoid the warning, give Hearthgate a certificate from your own CA with `admin.certificate` and `admin.private_key` in the bootstrap file. Plain HTTP requests to the port are redirected to HTTPS.

**Publishing a service.** Add the service (a container or a program on your network), then a public interface with its domain and ports, then a route between them on the Routes page. Edits go into a draft: the bar at the bottom shows what changed, **Review and apply** shows the diff, and applying stores it as a new revision. The **History** page shows every revision's diff and rolls back to any of them.

**Accounts.** Under Settings, Account you can change the password (other sessions are logged out), turn on two-factor login with an authenticator app, and create API tokens for scripts. Ten failed logins in 15 minutes from one address, or for one username, block further attempts for a while. Sessions end after 12 hours without use and after 7 days at most.

**Who can reach it.** Settings, Dashboard access shows the addresses and networks in effect and lets you choose your own (private ranges only). Changes apply at once; a change that would lock out the browser making it is refused. If you lock yourself out anyway, run `sudo -u hearthgate hearthgate admin-access --reset` and restart the service. Setting `admin.listen` or `admin.allow` in the bootstrap file fixes the values there instead, and the dashboard shows them read-only.

## Desired state

A complete example is in [`packaging/examples/state.toml`](../packaging/examples/state.toml). Check a file without applying it with `hearthgate validate state.toml`. Every problem is reported at once, with where it is.

Names of interfaces, services, routes and certificates use lowercase letters, digits and dashes.

### Interfaces and listeners

```toml
[[interfaces]]
name = "web"
domains = ["app.example.com", "www.example.com"]
listeners = [
  { port = 80, mode = "http" },
  { port = 443, mode = "https" },
]
# bind = ["192.0.2.10"]      # default: every IPv4 and IPv6 address
# certificate = "my-cert"    # use an uploaded certificate instead of ACME
```

Each listener has a `port`, a `mode` and, for raw forwarding, a `protocol`:

| Mode | What it does |
|---|---|
| `https` | Terminates TLS with a certificate for the interface's domains, then routes HTTP/1.1 and HTTP/2 by host and path. |
| `http` | Plain HTTP routing. Requests that match no route are redirected to the interface's `https` listener, and ACME HTTP-01 challenges are answered here. |
| `tls-passthrough` | Routes TLS by the server name (SNI) without decrypting it. The upstream keeps its own certificate. |
| `raw` | Layer 4 forwarding. `protocol` is `tcp` (default), `udp` or `both`. |

Several interfaces can share a port when Hearthgate can tell their traffic apart: `https` and `tls-passthrough` interfaces share by server name, and `http` interfaces share by Host header. Their domains must not overlap. A `raw` listener always owns its port.

### Services

```toml
[[services]]
name = "app"
kind = "external"            # a program reached by host and port
host = "10.0.0.11"           # IP address or host name
ports = { http = 8080 }
health = { type = "http", path = "/healthz", interval = "5s" }

[[services]]
name = "nextcloud"
kind = "docker"              # an existing container, found by name
container = "nextcloud"
# network = "proxy"          # default: the container's first network
ports = { http = 80 }
```

For `docker` services Hearthgate reads the container's address from Docker and follows restarts and network changes. It only reads: it never starts, stops or changes containers. To allow it, uncomment `SupplementaryGroups=docker` in the systemd unit. Access to the Docker socket is equivalent to root access on the host.

**Health checks** are optional. `type = "tcp"` connects, `type = "http"` sends a GET to `path` and expects a 2xx or 3xx status.

| Setting | Default | Meaning |
|---|---|---|
| `interval` | `10s` | Time between checks |
| `timeout` | `2s` | Time allowed for one check |
| `unhealthy_threshold` | `3` | Failures in a row before the upstream leaves rotation |
| `healthy_threshold` | `2` | Successes in a row before it comes back |
| `port` | every port routes use | Port to check, by name or number |

Independently of health checks, an upstream that refuses 3 connections in a row sits out for 10 seconds. If every target of a route is down, Hearthgate still tries them, so a broken health check cannot take a route offline on its own.

### Routes

```toml
[[routes]]
name = "app"
interface = "web"
listener = 443
# hosts = ["app.example.com"]  # default: every domain of the interface
# path_prefix = "/api"         # default: "/"
# strip_prefix = true          # forward /api/users as /users
strategy = "round-robin"       # or "least-connections", "source-hash"
targets = [
  { service = "app-1", port = "http" },
  { service = "app-2", port = "http", weight = 2 },
]
```

- The longest matching `path_prefix` wins. `/api` matches `/api` and `/api/users`, not `/apiary`.
- `round-robin` honours `weight`. `least-connections` picks the target with the fewest open connections. `source-hash` keeps each client on the same target while it is healthy.
- HTTP requests get `X-Forwarded-For`, `X-Forwarded-Proto`, `X-Forwarded-Host`, `X-Forwarded-Port` and `X-Real-IP`. Values sent by the client are replaced, never trusted. The original `Host` header is kept. WebSockets work.
- A `raw` listener has exactly one route. Use several targets in it to load-balance.

### Certificates

`https` interfaces get certificates automatically from Let's Encrypt, or from any ACME CA:

```toml
[acme]
email = "you@example.com"
accept_terms = true          # required: you agree to the CA's terms of service
# staging = true             # Let's Encrypt staging while you test
# directory = "https://ca.example.lan/acme/directory"   # another CA
# directory_root = "/etc/hearthgate/ca.pem"            # its API's root certificate
# challenge = "tls-alpn-01"  # default: http-01 if the interface has an http listener
# renew_before = "30days"
```

For validation, the interface's domains must point at your public IP, and port 80 (HTTP-01) or 443 (TLS-ALPN-01) must reach Hearthgate. Certificates are renewed 30 days before they expire. Failed requests are retried after 5 minutes, then less and less often (up to every 6 hours), so a mistake does not run into the CA's rate limits. Until a certificate exists, Hearthgate serves a self-signed one, so browsers show a warning instead of failing to connect.

Wildcard domains need a certificate you upload yourself (automatic wildcards need DNS challenges, planned for a later release):

```sh
hearthgate cert import my-cert --cert fullchain.pem --key privkey.pem
hearthgate cert list
hearthgate cert remove my-cert
```

A running gateway picks up imported certificates within a second.

### Limits

All optional, shown with their defaults:

```toml
[limits]
max_body_bytes = 104857600      # 100 MiB per HTTP request
header_timeout = "15s"          # to receive request headers (and the TLS handshake)
upstream_timeout = "1m"         # for an upstream to start answering
connect_timeout = "5s"
tcp_idle_timeout = "1h"         # raw TCP, passthrough and WebSockets
udp_session_timeout = "1m"
drain_timeout = "30s"           # removed listeners and shutdown
```

## Command line

| Command | What it does |
|---|---|
| `hearthgate run` | Run the gateway (what the service starts) |
| `hearthgate validate [FILE]` | Check a desired-state file, or the current state |
| `hearthgate apply FILE [-m MESSAGE] [--dry-run]` | Store a file as a new revision and show the diff |
| `hearthgate export [-o FILE] [-r N]` | Print the current (or an older) state as TOML |
| `hearthgate import FILE` | Restore an exported file (same as `apply`) |
| `hearthgate status` | Current revision and interfaces |
| `hearthgate revisions` | History, newest first |
| `hearthgate diff A [B]` | What changed between two revisions |
| `hearthgate rollback N` | Make revision N current again, as a new revision |
| `hearthgate cert import/list/remove` | Uploaded certificates |
| `hearthgate admin-token` | Print the admin API token |
| `hearthgate setup-code` | Print the code for creating the first dashboard account |
| `hearthgate admin-access [--reset]` | Show who can reach the dashboard, or reset that to the default |
| `hearthgate openapi` | Print the admin API's OpenAPI document |

`--data-dir` (or `HEARTHGATE_DATA_DIR`) points any command at another data directory, which is handy for trying things out without root.

`apply` and `rollback` never rewrite history: rolling back to revision 3 creates a new revision with revision 3's content. Applying a file equal to the current state creates nothing.

## Admin API

The dashboard uses the same REST API as scripts, on the same port. Its OpenAPI document is at `/api/v1/openapi.json`. Scripts authenticate with an API token created under Settings, Account, or with the admin token for scripts on the server itself:

```sh
TOKEN=$(sudo -u hearthgate hearthgate admin-token)
curl --cacert /var/lib/hearthgate/admin-tls/cert.pem \
  -H "Authorization: Bearer $TOKEN" https://localhost:7380/api/v1/status
```

| Endpoint | |
|---|---|
| `GET /api/v1/status` | Running revision, listeners, upstream health and traffic rates, problems |
| `GET /api/v1/state`, `PUT /api/v1/state` | Read or replace the desired state (JSON) |
| `POST /api/v1/state/validate`, `POST /api/v1/state/preview` | Check a state, and see its diff against the running one |
| `GET /api/v1/state/export` | The running state as TOML |
| `GET /api/v1/revisions`, `GET /api/v1/revisions/{id}`, `GET /api/v1/revisions/{id}/diff` | History |
| `POST /api/v1/revisions/{id}/rollback` | Roll back |
| `GET /api/v1/certificates` | Stored certificates and expiry |
| `GET /api/v1/events` | Live updates as Server-Sent Events: status every 2 seconds and new log lines |
| `GET /api/v1/logs`, `GET /api/v1/traffic` | The last log lines, and requests per hour for 24 hours |
| `GET`, `PUT /api/v1/settings/admin-access` | Who can reach the dashboard |
| `/api/v1/auth/…`, `/api/v1/tokens` | Login, password, two-factor login and API tokens |
| `GET /metrics` | Prometheus metrics |

Browser sessions also need the `X-CSRF-Token` header (from `GET /api/v1/auth/session`) on every request that changes something; token-authenticated requests don't.

### Metrics

`/metrics` exposes, among others: `hearthgate_upstream_healthy`, `hearthgate_upstream_active`, `hearthgate_upstream_connections_total`, `hearthgate_upstream_failures_total`, sent and received bytes, `hearthgate_http_responses_total` by status class, `hearthgate_http_local_responses_total`, `hearthgate_listener_open_connections`, `hearthgate_certificate_expiry_timestamp_seconds` and `hearthgate_revision`. Set `admin.metrics_without_token = true` to let a Prometheus server on your LAN scrape it without the token.

## Operating it

- `systemctl reload hearthgate` makes the gateway re-read the current revision at once.
- Logs go to the journal: `journalctl -u hearthgate -f`. Set `log = "hearthgate=debug,info"` in the bootstrap file for more detail.
- Back up `/var/lib/hearthgate`: it holds the database (with dashboard accounts), certificates, ACME account keys, the dashboard certificate and the admin token, all readable only by the `hearthgate` user.
- Ports a listener cannot bind (for example because another program uses them) are reported in the log and in `/api/v1/status`, and retried every 10 seconds. Everything else keeps working.
