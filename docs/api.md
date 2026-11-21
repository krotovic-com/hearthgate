# Admin API and metrics

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
| `GET /api/v1/firewall`, `GET /api/v1/firewall/recent` | Firewall state, dropped packets, feeds and bans, and sources dropped in the last hour |
| `POST /api/v1/firewall/confirm` | Keep a waiting firewall change (only over a connection opened after the change) |
| `POST /api/v1/firewall/bans`, `DELETE /api/v1/firewall/bans/{address}` | Ban or unban an address |
| `GET /api/v1/router` | Router port forwarding status |
| `GET /api/v1/docker`, `GET /api/v1/docker/updates` | Managed containers and stacks, and image update checks |
| `POST /api/v1/docker/containers/{name}/restart`, `…/update`, `POST /api/v1/docker/stacks/{name}/update` | Restart, or pull and recreate |
| `GET /api/v1/docker/containers/{name}/logs`, `…/stats` | Output (with `tail` and `search`) and resource use |
| `POST /api/v1/notifications/test` | Send a test message with the given notification settings |
| `GET /api/v1/vpn` | VPN state, devices with their last handshake and traffic, and the internal DNS names |
| `POST /api/v1/vpn/peer-config` | Keys and configuration for a new device. Nothing is stored: add the returned public key to the state |
| `/api/v1/auth/…`, `/api/v1/tokens` | Login, password, two-factor login and API tokens |
| `GET /metrics` | Prometheus metrics |

Browser sessions also need the `X-CSRF-Token` header (from `GET /api/v1/auth/session`) on every request that changes something; token-authenticated requests don't.

## Metrics

`/metrics` exposes, among others: `hearthgate_upstream_healthy`, `hearthgate_upstream_active`, `hearthgate_upstream_connections_total`, `hearthgate_upstream_failures_total`, sent and received bytes, `hearthgate_http_responses_total` by status class, `hearthgate_http_local_responses_total`, `hearthgate_listener_open_connections`, `hearthgate_certificate_expiry_timestamp_seconds` and `hearthgate_revision`. Set `admin.metrics_without_token = true` to let a Prometheus server on your LAN scrape it without the token.
