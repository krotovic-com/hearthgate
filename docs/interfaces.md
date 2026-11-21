# Public interfaces

A public interface is what the internet can reach: one or more domains and the ports they listen on. Each port is a listener with a mode that says how its traffic is handled.

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
