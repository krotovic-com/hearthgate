<p align="center">
  <picture>
    <source media="(prefers-color-scheme: dark)" srcset="docs/brand/logo-on-dark.svg">
    <img src="docs/brand/logo-on-light.svg" alt="hearthgate" width="360">
  </picture>
</p>

<p align="center"><strong>Expose what you choose. Reach the rest over VPN.</strong></p>

<p align="center">
  <a href="https://github.com/krotovic-com/hearthgate/actions/workflows/ci.yml"><img src="https://github.com/krotovic-com/hearthgate/actions/workflows/ci.yml/badge.svg" alt="CI"></a>
  <a href="https://github.com/krotovic-com/hearthgate/releases/latest"><img src="https://img.shields.io/github/v/release/krotovic-com/hearthgate?sort=semver&color=3B6EF5" alt="Latest release"></a>
  <a href="LICENSE"><img src="https://img.shields.io/badge/license-MIT-2BC6A0" alt="MIT license"></a>
</p>

Hearthgate is a secure-by-default gateway for home servers, written in Rust and run as a native Linux systemd service. It is the only way in from the internet. Only the services you explicitly publish get a public interface. Everything else stays private and is reachable over the built-in VPN.

> **Status:** early development. The core gateway and its dashboard work: public interfaces, routes to external services and existing Docker containers, load balancing, health checks, automatic certificates, a web dashboard reachable only from your LAN, a CLI and an admin API. The firewall, VPN and managed Docker stacks are next. See the [roadmap](docs/ROADMAP.md) for what lands in each release, and the [user guide](docs/guide.md) for how to use what is there.

## What it does

Available now:

- **Public interfaces.** A domain plus public ports (TCP, UDP or both), with automatic certificate issuance and renewal (Let's Encrypt or any ACME CA), or certificates you upload.
- **Routes and load balancing.** Point a port at one or more services, by host name and path. Round-robin with weights, least connections or source hash, with active health checks.
- **TLS your way.** Terminate TLS in Hearthgate, or pass it through untouched by server name, on the same port.
- **Services.** Route to programs on your network (such as sshd) or to existing Docker containers, which are followed across restarts.
- **Every change is a revision.** Apply a file, see the diff, roll back. Changes take effect within a second without dropping open connections.
- **Dashboard.** A web UI, reachable only from your LAN over HTTPS, with live health and traffic, a draft-review-apply flow with diffs, history and rollback, two-factor login and API tokens.
- **Admin API and metrics.** A REST API with an OpenAPI document, reachable only from private networks, and Prometheus metrics.
- **Firewall and blocklists.** An nftables firewall that closes every port no public interface opens, with your own rules, subscribed blocklists, country blocking, per-interface rate limits and automatic bans for clients that probe or fail to log in. Changes that would lock you out undo themselves after 60 seconds.
- **Router port forwarding.** Optionally keeps your router's port forwards (UPnP or NAT-PMP) in line with your public interfaces, and warns about double NAT.

Coming next:

- **Managed Docker.** Docker containers and stacks that Hearthgate starts and updates for you.
- **Built-in VPN.** Peers reach every internal service, published or not, with internal DNS at `name.home.internal`.
- **Optional AI assistant.** Bring your own provider (Claude, Gemini, OpenAI, Ollama and others) to review rules and troubleshoot services. Suggestions always wait for your approval, and logs are redacted before they leave the machine.

## Install

Each release ships static (musl) binaries for Linux:

| Architecture | Target |
|---|---|
| x86_64 | `x86_64-unknown-linux-musl` |
| ARM64 (Raspberry Pi 4/5, most ARM servers) | `aarch64-unknown-linux-musl` |
| ARMv7 (Raspberry Pi 2/3 on 32-bit OS) | `armv7-unknown-linux-musleabihf` |

```sh
VERSION=0.1.0                       # pick from the releases page
TARGET=x86_64-unknown-linux-musl
curl -LO "https://github.com/krotovic-com/hearthgate/releases/download/v$VERSION/hearthgate-v$VERSION-$TARGET.tar.gz"
curl -LO "https://github.com/krotovic-com/hearthgate/releases/download/v$VERSION/SHA256SUMS"
sha256sum --check --ignore-missing SHA256SUMS

tar -xzf "hearthgate-v$VERSION-$TARGET.tar.gz"
cd "hearthgate-v$VERSION-$TARGET"
sudo install -m 0755 hearthgate /usr/local/bin/hearthgate
sudo install -m 0644 hearthgate.sysusers /usr/lib/sysusers.d/hearthgate.conf
sudo systemd-sysusers
sudo install -m 0644 hearthgate.service /etc/systemd/system/hearthgate.service
sudo install -d -m 0755 /etc/hearthgate
sudo install -m 0644 examples/hearthgate.toml /etc/hearthgate/hearthgate.toml
sudo systemctl daemon-reload
sudo systemctl enable --now hearthgate
```

Then open `https://SERVER-LAN-ADDRESS:7380/` from a computer on your network and create the admin account with the setup code from `sudo -u hearthgate hearthgate setup-code` (the [guide](docs/guide.md#dashboard) explains the certificate warning). Or describe what to expose in a file (start from `examples/state.toml`) and apply it:

```sh
sudo -u hearthgate hearthgate validate state.toml
sudo -u hearthgate hearthgate apply state.toml
sudo -u hearthgate hearthgate status
```

The [user guide](docs/guide.md) covers every setting, the CLI, certificates and the admin API.

## Build from source

```sh
cargo build --release
./target/release/hearthgate --version
```

See [CONTRIBUTING.md](CONTRIBUTING.md) for the development setup, cross-compiling and how releases work.

## Releases and versioning

Hearthgate follows [Semantic Versioning](https://semver.org). Every commit merged into `main` is checked by the release workflow, which reads the [Conventional Commit](https://www.conventionalcommits.org) messages since the last tag:

| Commit | Release |
|---|---|
| `feat: …` | minor (`0.3.0` → `0.4.0`) |
| `fix: …`, `perf: …` | patch (`0.3.0` → `0.3.1`) |
| `feat!: …` or a `BREAKING CHANGE:` footer | major (minor while the version is `0.x`) |
| `chore:`, `docs:`, `ci:`, `refactor:`, `test:`, `style:`, `build:` | no release |

When a release is due, the workflow builds every target, tags the commit and publishes a GitHub Release with the archives, checksums and generated release notes.

## Contributing

Contributions are welcome. Read [CONTRIBUTING.md](CONTRIBUTING.md) before opening a pull request, and report security issues privately as described in [SECURITY.md](SECURITY.md).

## License

[MIT](LICENSE)
