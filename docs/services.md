# Services

A service is something inside your network that Hearthgate can forward to: a program reached by host and port, such as sshd, or an existing Docker container.

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

For `docker` services Hearthgate reads the container's address from Docker and follows restarts and network changes. To allow it, and to run managed containers, uncomment `SupplementaryGroups=docker` in the systemd unit. Access to the Docker socket is equivalent to root access on the host.

**Health checks** are optional. `type = "tcp"` connects, `type = "http"` sends a GET to `path` and expects a 2xx or 3xx status.

| Setting | Default | Meaning |
|---|---|---|
| `interval` | `10s` | Time between checks |
| `timeout` | `2s` | Time allowed for one check |
| `unhealthy_threshold` | `3` | Failures in a row before the upstream leaves rotation |
| `healthy_threshold` | `2` | Successes in a row before it comes back |
| `port` | every port routes use | Port to check, by name or number |

Independently of health checks, an upstream that refuses 3 connections in a row sits out for 10 seconds. If every target of a route is down, Hearthgate still tries them, so a broken health check cannot take a route offline on its own.
