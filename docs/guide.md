# Hearthgate user guide

This guide covers what Hearthgate 0.2 does: public interfaces, services, routes, load balancing, health checks, certificates, the command line and the admin API. Later releases add the dashboard, firewall, VPN and managed Docker (see the [roadmap](ROADMAP.md)).

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
| A desired-state file, for example `state.toml` | Interfaces, services, routes, certificates settings | `hearthgate apply state.toml` |

The desired-state file is only an input. After `apply`, the database is the source of truth, and `hearthgate export` prints it back.

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
| `hearthgate openapi` | Print the admin API's OpenAPI document |

`--data-dir` (or `HEARTHGATE_DATA_DIR`) points any command at another data directory, which is handy for trying things out without root.

`apply` and `rollback` never rewrite history: rolling back to revision 3 creates a new revision with revision 3's content. Applying a file equal to the current state creates nothing.

## Admin API

The REST API listens on `127.0.0.1:7380` by default. Its OpenAPI document is at `/api/v1/openapi.json`, and every other request needs the token from `hearthgate admin-token`:

```sh
TOKEN=$(sudo -u hearthgate hearthgate admin-token)
curl -H "Authorization: Bearer $TOKEN" http://127.0.0.1:7380/api/v1/status
```

| Endpoint | |
|---|---|
| `GET /api/v1/status` | Running revision, listeners, upstream health and problems |
| `GET /api/v1/state`, `PUT /api/v1/state` | Read or replace the desired state (JSON) |
| `POST /api/v1/state/validate` | Check a state without storing it |
| `GET /api/v1/revisions`, `GET /api/v1/revisions/{id}` | History |
| `POST /api/v1/revisions/{id}/rollback` | Roll back |
| `GET /api/v1/certificates` | Stored certificates and expiry |
| `GET /metrics` | Prometheus metrics |

To reach it from other machines on your LAN, add the server's LAN address to `admin.listen` in the bootstrap file. Clients outside `admin.allow` (by default loopback and private ranges) are disconnected before anything is read, even if `listen` is set to a public address. In this release the API speaks plain HTTP, so keep it on a network you trust. The dashboard release adds HTTPS and proper logins.

### Metrics

`/metrics` exposes, among others: `hearthgate_upstream_healthy`, `hearthgate_upstream_active`, `hearthgate_upstream_connections_total`, `hearthgate_upstream_failures_total`, sent and received bytes, `hearthgate_http_responses_total` by status class, `hearthgate_http_local_responses_total`, `hearthgate_listener_open_connections`, `hearthgate_certificate_expiry_timestamp_seconds` and `hearthgate_revision`. Set `admin.metrics_without_token = true` to let a Prometheus server on your LAN scrape it without the token.

## Operating it

- `systemctl reload hearthgate` makes the gateway re-read the current revision at once.
- Logs go to the journal: `journalctl -u hearthgate -f`. Set `log = "hearthgate=debug,info"` in the bootstrap file for more detail.
- Back up `/var/lib/hearthgate`: it holds the database, certificates, ACME account keys and the admin token, all readable only by the `hearthgate` user.
- Ports a listener cannot bind (for example because another program uses them) are reported in the log and in `/api/v1/status`, and retried every 10 seconds. Everything else keeps working.
