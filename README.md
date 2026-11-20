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

> **Status:** early development. The current releases contain the project skeleton only. Nothing is proxied or exposed yet.

## What it will do

- **Public interfaces.** A domain plus public ports (TCP, UDP or both), with automatic certificate issuance and renewal.
- **Routes and load balancing.** Point a public interface at one or more services. Two or more targets are load-balanced.
- **Services.** Route to system services managed outside Hearthgate (such as sshd), or to Docker containers and stacks that Hearthgate manages.
- **Built-in VPN.** Peers reach every internal service, published or not, with internal DNS at `name.home.internal`.
- **Firewall and blocklists.** Unexposed ports stay closed, and known-bad sources are blocked.
- **UPnP.** Optionally opens and closes router ports to match your public interfaces.
- **Dashboard.** A web UI that is reachable only from your LAN or over the VPN.
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
sudo install -m 0644 hearthgate.service /etc/systemd/system/hearthgate.service
sudo systemctl daemon-reload
sudo systemctl enable --now hearthgate
```

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
