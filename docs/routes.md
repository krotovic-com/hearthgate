# Routes and load balancing

A route connects a port of a public interface to one or more service ports. One target forwards. Two or more are load-balanced, and health checks take unhealthy targets out until they recover.

## Add a route

You need a [public interface](interfaces.md) and at least one [service](services.md) first. In the dashboard, open **Routes**, choose **New route**, pick the interface and port, and add the targets. Or describe it in the desired state:

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

With round-robin and these weights, `app-2` gets two of every three requests.

## Matching requests

On `https` and `http` listeners, a request picks its route by host name and path. Leave out `hosts` to match every domain of the interface.

- The longest matching `path_prefix` wins. `/api` matches `/api` and `/api/users`, not `/apiary`.
- With `strip_prefix = true`, the service sees `/users` for a request to `/api/users`.

## Strategies

| `strategy` | How it picks a target |
|---|---|
| `round-robin` | Takes turns, honouring `weight`. |
| `least-connections` | The target with the fewest open connections. |
| `source-hash` | Keeps each client on the same target while it is healthy. |

> Health checks decide who is in the pool. A target leaves the rotation when its service's health check fails and comes back once it passes again. If every target is down, Hearthgate still tries them, so a broken health check can't take a route offline on its own. See [Services](services.md) for the `health` setting.

## Forwarded headers

HTTP requests get `X-Forwarded-For`, `X-Forwarded-Proto`, `X-Forwarded-Host`, `X-Forwarded-Port` and `X-Real-IP`. Values sent by the client are replaced, never trusted. The original `Host` header is kept. WebSockets work.

## Raw listeners

A `raw` listener has exactly one route. Use several targets in it to load-balance.
