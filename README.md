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

> **Status:** 1.0. Public interfaces with automatic certificates (wildcards through DNS-01), routes to programs on your network, existing Docker containers or containers and Compose stacks Hearthgate runs itself, load balancing and health checks, an nftables firewall with blocklists and rate limits, router port forwarding, a WireGuard VPN with internal DNS, notifications, backups, and a web dashboard reachable only from your LAN or VPN, plus a CLI and an admin API. The [user guide](docs/guide.md) covers all of it; the [roadmap](docs/ROADMAP.md) shows what comes next.

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
- **Managed Docker.** Containers and Compose stacks that Hearthgate runs, updates on request and shows logs for, reachable by routes without publishing ports.
- **Built-in VPN.** WireGuard devices reach every internal service, published or not, with internal DNS at `name.home.internal`. Add a device by scanning a QR code, and limit a device to a few services if you like.

Coming next:

- **Optional AI assistant.** Bring your own provider (Claude, Gemini, OpenAI, Ollama and others) to review rules and troubleshoot services. Suggestions always wait for your approval, and logs are redacted before they leave the machine.

## Install

On Debian, Ubuntu, Fedora, RHEL, openSUSE or any other Linux with systemd:

```sh
curl -fsSL https://github.com/krotovic-com/hearthgate/releases/latest/download/install.sh | sudo sh
```

The script picks the `.deb`, the `.rpm` or the static archive for your system, checks it against the release's `SHA256SUMS`, installs it, starts the service and prints the setup code for the first account. Run it again to upgrade. Read it first if you prefer: it is [scripts/install.sh](scripts/install.sh).

Or download a package from the [releases page](https://github.com/krotovic-com/hearthgate/releases) yourself:

```sh
sudo apt install ./hearthgate_1.0.0_amd64.deb      # Debian, Ubuntu
sudo dnf install ./hearthgate-1.0.0-1.x86_64.rpm   # Fedora, RHEL
sudo systemctl enable --now hearthgate
```

Packages and archives exist for these architectures. The binaries are static (musl), so they run on any distribution:

| Architecture | Archive target | .deb | .rpm |
|---|---|---|---|
| x86_64 | `x86_64-unknown-linux-musl` | `amd64` | `x86_64` |
| ARM64 (Raspberry Pi 4/5, most ARM servers) | `aarch64-unknown-linux-musl` | `arm64` | `aarch64` |
| ARMv7 (Raspberry Pi 2/3 on 32-bit OS) | `armv7-unknown-linux-musleabihf` | `armhf` | `armv7hl` |

The [guide](docs/guide.md#install) shows the manual install from an archive.

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
| a `Release-As: 1.0.0` footer | exactly that version, if it is newer |

When a release is due, the workflow builds every target, tags the commit and publishes a GitHub Release with the archives, `.deb` and `.rpm` packages, the install script, checksums and generated release notes.

## Contributing

Contributions are welcome. Read [CONTRIBUTING.md](CONTRIBUTING.md) before opening a pull request, and report security issues privately as described in [SECURITY.md](SECURITY.md).

## License

[MIT](LICENSE)
