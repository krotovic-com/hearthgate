# Threat model

Hearthgate is meant to be the only thing on a home server that the internet can reach. This document says what it defends against, how, and what it leaves to you. It covers Hearthgate 1.0. Report anything that breaks these promises as described in [SECURITY.md](https://github.com/krotovic-com/hearthgate/blob/main/SECURITY.md).

## What is worth protecting

| Asset | Where it lives |
|---|---|
| The services behind the gateway, and the LAN around them | Your network |
| The desired state: interfaces, routes, firewall rules, VPN peers, containers | `/var/lib/hearthgate/state.db` |
| Private keys: certificates, ACME accounts, the WireGuard server key, the dashboard certificate | `/var/lib/hearthgate` (owner-only files) |
| Dashboard accounts: password hashes (Argon2id), TOTP secrets, API token hashes, sessions | `state.db` |
| The admin token for the local CLI | `/var/lib/hearthgate/admin-token` |
| Secrets inside stacks (`.env` files) | `/var/lib/hearthgate/stacks/NAME` (mode 0600) |
| Backups, which hold all of the above | Wherever you keep them |

## Who might attack

1. **Anyone on the internet.** Scanners, botnets and targeted attackers. They can send any bytes to the public ports and nothing else.
2. **A VPN device that was lost, stolen or compromised.** It holds a valid WireGuard key, so it reaches what its access list allows.
3. **Someone on the LAN.** Guests, other people's devices, compromised IoT devices. They can reach the dashboard port but don't know a password.
4. **A compromised internal service or container.** It can talk to Hearthgate from inside the network, and answers proxied requests however it likes.
5. **A malicious or compromised upstream feed or registry.** Blocklist feeds, container registries and ACME servers send data Hearthgate parses.

Out of scope: someone with root or physical access to the server, someone who can read Hearthgate's data directory, a compromised Linux kernel, and attacks on your router or DNS provider beyond what Hearthgate configures.

## Trust boundaries

```
internet ──▶ public ports ──▶ proxy ──▶ internal services
                │                           ▲
                ├──▶ ACME challenges        │
                                            │
VPN devices ─▶ WireGuard ─▶ per-peer rules ─┤──▶ internal DNS
                                            │
LAN ─────────▶ dashboard and API (LAN/VPN only)
                                │
                                ▼
                  desired state ──▶ nftables, WireGuard, Docker, router (UPnP)
```

Data crosses a boundary at each arrow. Every crossing is described below.

## The public side

**Only configured ports are open.** The nftables table that Hearthgate owns drops all inbound traffic except established connections, the ports of public interfaces, WireGuard, and (from LAN and VPN sources only) the dashboard and internal DNS. Changes to the firewall are applied atomically and roll back after 60 seconds unless confirmed from a new connection, so a mistake can't lock you out.

**Parsing before trust.** The proxy reads the TLS ClientHello itself to route by SNI. That parser is bounds-checked, limited to 32 KiB and fuzzed (`fuzz/fuzz_targets/client_hello.rs`). HTTP parsing is done by `hyper`. TLS is `rustls`, with no OpenSSL anywhere in the binary.

**Resource limits.** Headers and TLS handshakes must arrive within `limits.header_timeout` (15 s), bodies are limited to `limits.max_body_bytes`, and idle connections are closed. These keep a single client from tying up memory or file descriptors. They do not stop a large distributed attack; nothing on a home connection does.

**Unknown hosts get nothing.** A request or TLS connection for a domain that no interface serves is refused before reaching any service, which also hides what runs behind the gateway from scanners that connect by IP.

**Automatic bans.** Repeated handshake failures, requests for unknown hosts and authentication failures from an upstream feed the ban logic. Bans are temporary, and LAN and VPN addresses are never banned. Blocklist feeds add known-bad networks. Feed contents are parsed defensively (fuzzed in `fuzz/fuzz_targets/blocklist.rs`), capped in size, and a feed that fails to download keeps its last good list.

**Certificates.** ACME challenges (HTTP-01, TLS-ALPN-01 and DNS-01) are answered only for domains in the desired state, and challenge responses carry only the key authorization. DNS provider credentials are stored like other secrets and are used only to set `_acme-challenge` records.

**What reaches your services.** The proxy adds `X-Forwarded-For`, `X-Forwarded-Proto` and `X-Forwarded-Host`, and removes incoming copies of them so a client can't forge its address. Your services still have to defend themselves: Hearthgate is not a web application firewall, and a vulnerable application behind it is still vulnerable.

## The VPN

**Keys.** The WireGuard server key is generated on first use and never leaves the data directory. Device keys are generated once for the configuration shown to you, and only the public key is stored, so Hearthgate can't hand out a device's private key again.

**What a device reaches.** Each peer has an access list: everything, or named services only. nftables enforces it per peer address on the VPN interface, so a device limited to one service can't reach the rest of the LAN, the other devices or the dashboard. Removing a peer from the state removes its key from the interface at once.

**Internal DNS.** The DNS server listens only on the VPN address (and the LAN when you allow it), never on public addresses. It answers names from the state and relays the rest to the system's resolvers. Queries are parsed with `hickory-proto` and fuzzed through Hearthgate's own handler (`fuzz/fuzz_targets/dns_query.rs`), and the number of queries in flight is capped.

## The dashboard and admin API

**Reachability.** The admin listener accepts connections only from loopback, the LAN networks it detects (or the ones you set) and the VPN. Any other source is closed before a byte is read, and this check runs before TLS, so the dashboard's TLS stack is not exposed to the internet either. Public addresses can't be added to the allow list.

**First setup.** Creating the first account needs a one-time setup code that only someone with access to the server's logs or shell can read. Without it, a LAN attacker can't claim a new installation. The code stops working once an account exists.

**Authentication.** Passwords are hashed with Argon2id and must be 12 to 1024 characters. Optional TOTP two-factor login. Failed logins are rate-limited per address and per username. Sessions are `HttpOnly`, `Secure`, `SameSite=Strict` cookies with idle and absolute lifetimes, every state-changing request from a browser must carry a CSRF token, and changing the password ends other sessions. API tokens are stored only as SHA-256 hashes and shown once.

**Browser hardening.** The dashboard is served with a strict Content Security Policy (scripts and styles only from itself, no inline scripts), `X-Frame-Options: DENY` and no third-party resources, so injected content can't load code or exfiltrate data, and the dashboard can't be framed.

**Every change is reviewable.** Changes go through drafts, validation and a diff, and every applied revision is kept, so an unwanted change (by a person or a script) can be seen and rolled back.

## Docker

Access to the Docker socket is equivalent to root on the host. Hearthgate only uses it when you add the `hearthgate` user to the `docker` group (`SupplementaryGroups=docker` in the systemd unit), and you can turn management off with `docker.manage = false` or Docker entirely with `docker.enabled = false`.

When management is on, Hearthgate only changes containers carrying its own labels, never containers it didn't create, and never deletes volumes. Containers run on Hearthgate's own Docker network, so publishing them on the internet goes through the proxy and firewall rather than through Docker's own port publishing, which bypasses most host firewalls. Images are only pulled or updated when the state asks for a new container or you ask for an update.

Hearthgate does not sandbox the containers you run. A container with a vulnerable application, mounted host paths or extra privileges is as dangerous as it would be without Hearthgate.

## The AI assistant

The assistant is off by default, and while it is off no code path connects to a provider (tested). When on, it only sends something when you start a review or the weekly review runs. Each request goes through redaction first: IP addresses, domains, URLs, email addresses, keys, tokens, passwords, container environment values and your own patterns are replaced by placeholders, and the exact body can be inspected beforehand. Redaction is pattern-based, so an unusual secret format can slip through; use `redact` patterns or a local model when that matters.

The model's answers are treated as untrusted input. They can't run anything or change the configuration: a suggested fix is only a patch that is validated like any edit, loaded into your draft, and applied only when you review and apply it. A model that was tricked by text in a log line can at worst suggest a bad change, which you see as a diff. The API key is kept out of the state, exports and API responses, in a file only Hearthgate can read.

## Updates

The gateway checks GitHub for releases and downloads them, but can't install anything: it can only write a request. A root helper with no network carries it out after copying the files into its own directory, refusing symbolic links, and checking that `SHA256SUMS` carries a valid signature from the release key compiled into the binary, for exactly the requested version, and that the file matches it. It never installs an older release except the one it kept for rollback. So someone who takes over the gateway can at most install a genuine, newer Hearthgate. The signing key lives offline and in a GitHub Actions secret used only by the release job; a stolen key could sign a malicious release, which is why the trusted comment names the version and automatic installs are off by default. If the new version doesn't report healthy within two minutes, the previous one comes back.

## The router

UPnP and NAT-PMP are off by default. When on, Hearthgate only asks the router to forward the ports of public interfaces and WireGuard, removes those mappings when they leave the state or on shutdown, and never forwards the dashboard. Any device on your LAN can ask a UPnP router for the same, so if you don't need it, keep it off on the router too.

## The host

Hearthgate runs as the unprivileged `hearthgate` user with three capabilities (bind low ports, configure the network, raw sockets for the firewall), and a systemd sandbox: read-only system directories, no access to home directories, no new privileges, a restricted system call set, no devices except `/dev/net/tun`, and memory that can't be both writable and executable. Its data directory is readable only by that user. Release binaries are static, built from a locked dependency graph in GitHub Actions, and published with SHA-256 checksums signed with the release key.

## Backups

A backup holds every private key and the account database. Hearthgate writes backups readable only by its own user. Keep them somewhere at least as safe as the server, and encrypt them if they leave it.

## Supply chain

- Dependencies are locked (`Cargo.lock`, `package-lock.json`) and checked on every pull request and weekly with `cargo deny` (RustSec advisories, licenses, sources) and `npm audit`.
- Only crates.io and the npm registry are allowed as sources; no git dependencies.
- The dashboard is compiled into the binary, so nothing is fetched at runtime.

## Testing

- Parsers that handle untrusted input are fuzzed: the TLS ClientHello, DNS queries, the desired state, blocklist feeds and container options (`scripts/fuzz.sh`, and weekly in CI).
- The firewall and VPN are tested against real nftables rulesets and WireGuard tunnels in network namespaces, and Docker management against a real Docker daemon.
- The admin API's authentication, CSRF protection, rate limiting and access checks have their own tests.

## Known limits

- **Denial of service.** Hearthgate limits what one client can use, but a home internet connection can be saturated from outside. That needs your ISP or an upstream service.
- **The LAN is trusted for reachability.** Anyone on the LAN can reach the dashboard login. Strong passwords, two-factor login and the rate limits are what protect it there. Use a separate network for guests and IoT devices if you can.
- **Plain HTTP upstreams.** Traffic between Hearthgate and your services is usually unencrypted inside your network.
- **No WAF.** Hearthgate doesn't inspect request contents for attacks on applications.
