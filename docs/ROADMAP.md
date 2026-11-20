# Hearthgate roadmap

This is the plan for building Hearthgate from the current skeleton (v0.1.0) to a 1.0 release and beyond. It is split into seven phases. Each phase ends in a release that is useful on its own, so a home server can run Hearthgate from Phase 1 onwards and pick up new features with each upgrade.

| Phase | Theme | Release | Dashboard screens it lights up |
|---|---|---|---|
| 1 | Core gateway: public interfaces, services, routes, TLS | 0.2 | none yet (config file, CLI and API) |
| 2 | Dashboard and admin access | 0.3 | Overview, Public interfaces, Services, Routes, Settings |
| 3 | Perimeter: firewall, blocklists, UPnP | 0.4 | Firewall, Settings → Router |
| 4 | Built-in VPN and internal DNS | 0.5 | VPN peers |
| 5 | Managed Docker containers and stacks | 0.6 | Services (managed), logs |
| 6 | Hardening and 1.0 | 1.0 | Dark mode, backups, updates |
| 7 | Optional AI assistant | 1.1 | AI review, "Review rules with AI" |

Phases 3, 4 and 5 depend only on Phase 1 and 2, so their order can be swapped if priorities change. AI is deliberately last: it reviews and explains everything the earlier phases build, so it needs them to exist and be stable.

Release numbers are a guide. Real versions come from Conventional Commits as described in [CONTRIBUTING.md](../CONTRIBUTING.md), and a phase may span several minor releases.

---

## Architecture decisions

These apply to every phase. Each one is a recommendation that later phases build on. The open ones are listed at the end of this document.

### One binary, one async runtime

- A single `hearthgate` binary runs as a systemd service (`Type=notify`, with `sd-notify` for readiness and watchdog).
- Runtime: **tokio**. Logging and tracing: **tracing** with `tracing-subscriber` to journald and a ring buffer that the dashboard reads.
- The repo becomes a Cargo workspace in Phase 1 so each subsystem has clear boundaries and can be tested alone:

```
crates/
  hearthgate-model     # desired-state types, validation, (de)serialisation
  hearthgate-store     # SQLite storage, revisions, migrations
  hearthgate-proxy     # HTTP(S), TLS passthrough, TCP and UDP proxying, load balancing
  hearthgate-acme      # certificate issuance, renewal and storage
  hearthgate-services  # external services, Docker discovery, health checks
  hearthgate-api       # admin REST API and auth
  hearthgate-ui        # embedded dashboard assets (Phase 2)
  hearthgate-firewall  # nftables and blocklists (Phase 3)
  hearthgate-upnp      # router port mapping (Phase 3)
  hearthgate-vpn       # WireGuard (Phase 4)
  hearthgate-dns       # internal DNS (Phase 4)
  hearthgate-docker    # managed containers and stacks (Phase 5)
  hearthgate-ai        # provider adapters and review engine (Phase 7)
hearthgate/            # the binary: CLI, wiring, supervisor
```

### Desired state and reconciliation

- Everything the user configures (interfaces, services, routes, later firewall rules and peers) is **desired state**. Subsystems are **reconcilers** that make the machine match it.
- Desired state is stored in **SQLite** (`/var/lib/hearthgate/state.db`, `rusqlite` with the bundled feature so musl builds stay static). Every change is saved as a numbered **revision**, which gives an audit log, a diff view and one-click rollback for free. The AI phase reuses revisions as the way to propose changes.
- `/etc/hearthgate/hearthgate.toml` holds only **bootstrap settings**: data directory, admin listen addresses, LAN ranges, log level. It never holds the full config, so the UI and the file never fight.
- `hearthgate export` and `hearthgate import` move the full desired state to and from a TOML file, for backups and people who prefer config as code.
- Applying a revision compiles it into an immutable runtime config that is swapped in atomically (`arc-swap`). Listeners are diffed: new ports bind, removed ports drain open connections and then close. No restart is needed for any config change.

### Secure by default

- Nothing is listening on a public port unless a public interface asks for it.
- The admin API and dashboard never bind to `0.0.0.0` by default. They bind to the LAN address (and later the VPN address) and also check the client address against the allowed ranges, so a wrong bind can't expose them.
- The service runs as a dedicated `hearthgate` user with only `CAP_NET_BIND_SERVICE`, `CAP_NET_ADMIN` and `CAP_NET_RAW`, not as root. The systemd unit is tightened accordingly in Phase 1.
- Secrets (ACME account keys, private keys, WireGuard keys, AI API keys) live in the state directory with `0600` permissions and are never returned by the API in full.

### Config model (Phase 1 core)

- **Public interface**: a name, one or more domains, and one or more **listeners**. A listener is a port, a protocol (`tcp`, `udp` or both) and a mode:
  - `https`: TLS terminated by Hearthgate with an automatic certificate, HTTP routing by host and path.
  - `http`: plain HTTP, normally only used to redirect to HTTPS and answer ACME challenges.
  - `tls-passthrough`: routed by SNI without decrypting.
  - `tcp` and `udp`: raw layer 4 forwarding.
- **Service**: something Hearthgate can forward to. Kind `external` is a host and named ports (for example sshd on `127.0.0.1:22`). Kind `docker` points at an existing container by name and port (Phase 1 only discovers it; Phase 5 manages it). Each service has an optional health check.
- **Route**: connects a listener (plus, for HTTP, a host and path match) to one or more service ports. One target forwards; two or more are load-balanced.

---

## Phase 1: Core gateway (0.2)

**Goal:** Hearthgate can replace a hand-written Nginx or Traefik setup for a home server. A user defines public interfaces, services and routes, and Hearthgate serves them with valid certificates and load balancing.

### Scope

- Workspace layout, desired-state model, SQLite store with revisions and migrations.
- CLI: `hearthgate run`, `validate`, `apply <file>`, `export`, `import`, `status`, `revisions`, `rollback <n>`.
- Admin REST API (axum) on the LAN address, token auth for now (the dashboard adds proper login in Phase 2). OpenAPI spec generated with `utoipa` so the UI and CLI share one contract.
- HTTP(S) reverse proxy on **hyper 1.x** and **tokio-rustls**: host and path-prefix routing, HTTP/1.1 and HTTP/2, WebSockets, `X-Forwarded-*` headers, HTTP to HTTPS redirect, request size and timeout limits.
- TLS passthrough by SNI.
- Raw TCP and UDP forwarding (UDP with per-client session tracking and idle timeouts).
- Load balancing: round robin (default), least connections and source IP hash. Active health checks (TCP connect or HTTP status) and passive ejection after repeated failures.
- Automatic certificates with **instant-acme** (Let's Encrypt and any ACME CA): HTTP-01 and TLS-ALPN-01 challenges, renewal 30 days before expiry, staging CA option, manual certificate upload as a fallback. Certificates are served by a `rustls` resolver that reloads without dropping connections.
- External services and read-only Docker discovery with **bollard** (resolve a container name to its address, follow restarts).
- Metrics endpoint in Prometheus format on the admin listener (requests, bytes, upstream health, certificate expiry).
- Packaging: updated systemd unit (dedicated user, `Type=notify`), example config in the release archive.

### Milestones

1. **M1.1 Foundations.** Workspace, model, store, revisions, CLI `validate`, `apply`, `export`, `rollback`.
2. **M1.2 Layer 4.** TCP and UDP forwarding with hot reload and listener diffing.
3. **M1.3 HTTP and TLS.** Reverse proxy, host and path routing, passthrough by SNI, manual certificates.
4. **M1.4 ACME.** Issuance and renewal against Pebble in tests and Let's Encrypt staging by hand.
5. **M1.5 Load balancing and health.** Strategies, health checks, passive ejection, metrics.
6. **M1.6 API and release.** REST API with OpenAPI, Docker discovery, systemd hardening, docs, release 0.2.

### Acceptance criteria

- A config with an `https` interface for `app.example.com` routing to two upstreams gets a Let's Encrypt staging certificate with no manual steps, serves traffic, and spreads requests across both upstreams.
- Stopping one upstream removes it from rotation within the health check interval, and no request fails after that. Starting it again brings it back.
- A `tcp` interface on port 2222 forwards to `127.0.0.1:22` and SSH works through it. A `udp` interface forwards a game or WireGuard server port.
- Changing, adding or removing a route via `apply` takes effect without a restart and without dropping open connections on unchanged listeners.
- `rollback` restores the previous revision exactly, and `export` then `import` round-trips without changes.
- Ports that no interface declares are not bound by Hearthgate (checked with `ss`).
- Integration tests run locally with Pebble (ACME test server) and throwaway upstreams. Benchmarks show the proxy adds less than 1 ms p50 latency on a Raspberry Pi 4 class device.

---

## Phase 2: Dashboard and admin access (0.3)

**Goal:** everything from Phase 1 can be managed from the web dashboard designed in Figma, reachable only from the LAN.

### Scope

- Dashboard SPA built from the Figma UI kit (file "Hearthgate — Brand & UI Kit", Dashboard page), light mode first. Built to static assets and **embedded in the binary** (`rust-embed`), so there is still one file to install.
- Screens: **Overview** (health, traffic, certificate expiry, recent changes), **Public interfaces**, **Services**, **Routes** (map view and load balancer editor), **Settings** (general, admin access, certificates).
- Changes are staged as a draft revision, shown as a diff, then applied. History view with rollback.
- Live data over Server-Sent Events: upstream health, request rates, log tail.
- Authentication: first-run setup creates an admin account (argon2id password hash), secure session cookies (`HttpOnly`, `SameSite=Strict`), CSRF protection, optional TOTP, login rate limiting. API tokens for scripts.
- Admin access gate: bind addresses plus a source address allowlist (LAN ranges, detected on first run and editable). Requests from anywhere else get a connection close, not a login page.
- The dashboard itself is served over HTTPS with a self-signed or internal CA certificate, with a clear trust prompt in the docs.

### Milestones

1. **M2.1 Shell.** Frontend project, design tokens from Figma, embedded serving, login and first-run setup.
2. **M2.2 Read views.** Overview, interfaces, services, routes with live status.
3. **M2.3 Editing.** Create and edit flows, draft and diff, apply, history and rollback.
4. **M2.4 Settings and release.** Admin access settings, certificates view, accessibility pass, release 0.3.

### Acceptance criteria

- A new user can install Hearthgate, open the dashboard from the LAN, create an admin account and publish a service with HTTPS without touching a config file.
- Requests to the dashboard from a non-LAN address are refused, even if the admin listener is misconfigured to bind publicly (covered by an automated test).
- Every edit produces a revision with a readable diff, and rollback from the UI works.
- The UI matches the Figma screens for layout and components, and passes keyboard navigation and WCAG AA contrast checks.
- Frontend tests (component and end-to-end with Playwright against a real Hearthgate) run locally with one command.

---

## Phase 3: Perimeter: firewall, blocklists, UPnP (0.4)

**Goal:** Hearthgate controls what reaches the host, not just what it proxies, and keeps the router in sync.

### Scope

- **Firewall** with nftables in a dedicated `inet hearthgate` table, so it never touches rules the user or Docker manages. Default policy for inbound: drop, except declared public ports, the admin listener from allowed ranges, and established traffic. Applied atomically from the desired state with a "safe apply" that rolls back automatically if the admin can't confirm within 60 seconds (protects against locking yourself out).
- Rule editor for allow and deny by source, port and protocol, plus per-interface rate limits (connections per second per source).
- **Blocklists**: subscribe to feeds (for example Spamhaus DROP, FireHOL level 1, abuse.ch), refreshed on a schedule into nftables interval sets. Country blocking with an optional GeoIP database. Manual block and allow lists.
- **Automatic bans**: the proxy reports abuse signals (repeated 4xx on auth paths, TLS handshake floods, scan patterns) and offenders get temporary bans, similar to fail2ban but built in.
- **UPnP** with `igd-next` (and NAT-PMP or PCP where available), off by default. When enabled, router port mappings follow public interfaces: added on create, renewed before the lease ends, removed on delete and on shutdown. Shows the detected public IP and warns about double NAT or CGNAT.
- Dashboard: **Firewall** screen (rules, blocklists, recent blocks), Settings → Router.

### Milestones

1. **M3.1 Firewall core.** nftables table, default policy, atomic apply, safe apply with auto-rollback.
2. **M3.2 Blocklists and bans.** Feeds, sets, auto-bans from proxy signals.
3. **M3.3 UPnP.** Port mapping lifecycle, public IP detection, router settings.
4. **M3.4 UI and release.** Firewall screen, router settings, release 0.4.

### Acceptance criteria

- With the firewall on, a port scan from outside the LAN finds only the declared public ports open.
- A bad rule that would cut off the admin is reverted automatically within 60 seconds.
- A blocklisted address is dropped before it reaches the proxy, and a feed of 100,000 ranges loads in under a few seconds on a Raspberry Pi 4.
- With UPnP on, creating and deleting a public interface opens and closes the router port, and stopping Hearthgate removes its mappings.
- Firewall tests run in network namespaces locally, with no changes to the developer's own host rules.

---

## Phase 4: Built-in VPN and internal DNS (0.5)

**Goal:** every internal service is reachable from anywhere over the VPN, published or not, by name.

### Scope

- **WireGuard** using the kernel module, managed over netlink (`defguard_wireguard_rs` or `wireguard-control`), with a userspace fallback (`boringtun`) for kernels without it. Its UDP port is opened in the firewall and through UPnP automatically.
- **Peers**: create from the dashboard, show a QR code and config file, revoke, last handshake and traffic per peer. Keys are generated on the server or supplied by the user.
- Access policy per peer: full access to all services, or a selected list (enforced by the Phase 3 firewall).
- **Internal DNS** with `hickory-server`, listening only on the VPN interface (and optionally the LAN): every service resolves as `name.home.internal`, the zone is configurable, other names forward to an upstream resolver.
- Admin dashboard reachable over the VPN as well as the LAN.
- Dashboard: **VPN peers** screen, DNS settings.

### Milestones

1. **M4.1 WireGuard.** Interface lifecycle, peers, key handling, firewall and UPnP integration.
2. **M4.2 DNS.** Internal zone from services, forwarding, split DNS in peer configs.
3. **M4.3 UI and release.** Peers screen with QR codes, per-peer access, release 0.5.

### Acceptance criteria

- A phone added as a peer by scanning a QR code can reach an unpublished service (for example SSH on port 22) by `name.home.internal` from a mobile network.
- A peer limited to one service cannot reach any other.
- Revoking a peer cuts its access immediately.
- DNS does not answer on the public interface (checked from outside).

---

## Phase 5: Managed Docker containers and stacks (0.6)

**Goal:** Hearthgate can run the services it exposes, not only route to them, replacing Portainer for typical home setups.

### Scope

- Create, start, stop, restart, update (pull and recreate) and remove **containers** through **bollard**. Environment variables, volumes, port mappings and restart policies from the dashboard.
- **Stacks** from Compose files. Recommendation: run the official `docker compose` CLI with a Hearthgate project label rather than reimplementing Compose, and read stack status back through the Docker API.
- Containers join a Hearthgate-managed Docker network so routes reach them without publishing host ports. Their names feed the internal DNS from Phase 4.
- Logs (live tail and search) and basic stats (CPU, memory, restarts) per container.
- Optional image update checks with a notification, never automatic updates without consent.
- Dashboard: Services screen gains "managed" services, a stack editor and log views.

### Milestones

1. **M5.1 Containers.** Lifecycle, managed network, routes to managed containers.
2. **M5.2 Stacks.** Compose import and editing, stack lifecycle.
3. **M5.3 Logs, stats and release.** Log tail, stats, update checks, release 0.6.

### Acceptance criteria

- A user can deploy a Compose stack from the dashboard, publish one of its containers on a domain with HTTPS and reach the rest over the VPN, with no host ports published.
- Recreating a container (for example after an image update) keeps its routes working with no config change.
- Docker access is documented clearly, because access to the Docker socket is equivalent to root, and this phase can be disabled entirely.

---

## Phase 6: Hardening and 1.0 (1.0)

**Goal:** Hearthgate is safe to recommend as the only internet entrypoint of a home server.

### Scope

- Security review of the proxy, auth and firewall paths, fuzzing of parsers (`cargo fuzz`), `cargo audit` and `cargo deny` in CI, and a threat model document.
- DNS-01 challenges for wildcard certificates and hosts without port 80 (common providers such as Cloudflare, plus RFC 2136).
- Backups: scheduled export of state, certificates and keys to a local path, with restore on a fresh install.
- Upgrades: in-place update check, state migrations tested from every earlier release.
- Packaging: `.deb` and `.rpm` packages and an install script, in addition to the tarballs.
- Dashboard dark mode (the Figma kit is light only today), notifications (email or webhook) for certificate, health and security events.
- Full user documentation site.

### Acceptance criteria

- No known high-severity issues open, fuzzers run without crashes for a fixed budget.
- Restoring a backup on a new machine brings back every interface, service, route, peer and certificate.
- Upgrading from 0.2 to 1.0 in one step works and is covered by a test.

---

## Phase 7: Optional AI assistant (1.1)

**Goal:** help users understand and fix their setup, without ever changing it behind their back.

### Scope

- Provider adapters behind one trait: Anthropic Claude, Google Gemini, OpenAI, and any OpenAI-compatible endpoint, which covers **Ollama** and other local models. Off by default; keys stored like other secrets.
- **Redaction** before anything leaves the machine: IP addresses, domains, tokens, keys and user-defined patterns are replaced with stable placeholders, and the exact payload is viewable before sending.
- **AI review** screen: a review of firewall rules, exposed services, certificates and recent errors, producing findings ranked by severity, or an all-clear.
- **"Review rules with AI"** panel on the Firewall screen.
- **Troubleshooting**: explain why a service is unhealthy using its health checks, proxy errors and container logs.
- Every suggestion is a **draft revision** with a diff. Nothing is applied until the user accepts it, using the same apply and rollback flow as manual changes.
- Optional weekly review on a schedule, with results in the dashboard and notifications.

### Acceptance criteria

- With AI disabled (the default), no network call to any AI provider is ever made (covered by a test).
- Redaction tests show no IP addresses, domains or secrets in payloads sent to providers.
- A suggested fix can be reviewed as a diff, accepted, and rolled back like any other change.
- The same review works against a local Ollama model with no internet access.

---

## Cross-cutting practices

- **Test locally first.** Every subsystem gets tests that run on a developer machine with one command: network namespaces for firewall and VPN, Pebble for ACME, throwaway containers for Docker. CI runs the same commands as a final check, so CI minutes stay low.
- **Feature flags per subsystem** in Cargo (`firewall`, `vpn`, `docker`, `ai`), so minimal builds stay small and each part can be turned off at runtime too.
- **Targets:** every phase must work on the three release targets (x86_64, ARM64 and ARMv7 musl). Memory budget: under 50 MB resident for a typical home setup.
- **Docs per phase:** each release updates the README feature list and adds a user guide for what it ships.

## Open decisions

These are worth confirming before the phase that needs them. The recommendation is listed first.

1. **Dashboard framework (Phase 2).** A TypeScript SPA (Svelte 5 with Vite) embedded in the binary, because it maps directly to the Figma kit and has the widest contributor pool. The alternative is a Rust frontend (Leptos), which keeps one language but has a smaller ecosystem.
2. **Desired-state storage (Phase 1).** SQLite with revisions, plus TOML import and export. The alternative is TOML as the only source of truth, which is simpler but makes concurrent UI edits, history and AI drafts harder.
3. **Compose stacks (Phase 5).** Use the `docker compose` CLI, rather than reimplementing Compose in Rust.
4. **Firewall backend (Phase 3).** nftables only. iptables-legacy systems would not be supported.
5. **Phase order.** Firewall before VPN before Docker, as above. Docker could move earlier if replacing Portainer is the most urgent need.
