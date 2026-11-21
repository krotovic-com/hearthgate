# Limits

Limits cap request sizes and timeouts, so a slow or misbehaving client can't hold the gateway's resources. All of them are optional. Here they are with their defaults:

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
