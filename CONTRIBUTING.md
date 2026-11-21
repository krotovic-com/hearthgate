# Contributing to Hearthgate

Thanks for helping. Hearthgate guards people's home networks, so we favor small, well-tested changes and clear reasoning over speed. This guide covers the development setup, commit messages, pull requests, code style and how releases happen.

By contributing you agree that your work is released under the [MIT license](LICENSE). Report security issues privately as described in [SECURITY.md](SECURITY.md), never in a public issue.

## Development setup

You need a stable Rust toolchain (the minimum supported version is in `rust-version` in `Cargo.toml`):

```sh
rustup toolchain install stable --component rustfmt clippy
cargo build
cargo run -- --help
```

The dashboard needs Node.js 22. It is built into `dashboard/dist` and embedded in the binary; without a build, the binary serves a short "dashboard not included" page instead:

```sh
cd dashboard
npm ci
npm run build                  # then cargo build embeds it
npm run dev                    # live-reloading UI against a running Hearthgate
```

`npm run dev` sends API calls to `https://127.0.0.1:7380`; point it elsewhere with `HEARTHGATE_URL`. Hearthgate's Content Security Policy allows no inline scripts or styles, so use classes, not `style` attributes.

Hearthgate targets Linux only. You can develop on macOS or Windows (WSL), but parts that touch networking, the firewall or systemd will need a Linux machine or VM to run.

### Cross-compiling release binaries

Release builds are static musl binaries made with [cargo-zigbuild](https://github.com/rust-cross/cargo-zigbuild), so you can build every target from one machine without Docker:

```sh
pip install ziglang            # or install zig 0.15+ from your package manager
cargo install --locked cargo-zigbuild
scripts/build-release.sh 0.0.0-test                              # all targets
scripts/build-release.sh 0.0.0-test aarch64-unknown-linux-musl   # just one
```

Archives and `SHA256SUMS` land in `dist/`. This is the same script the release workflow runs.

### Workspace layout

| Crate | What it holds |
|---|---|
| `hearthgate` | The binary: CLI, bootstrap config and the daemon that wires everything together |
| `crates/hearthgate-model` | The desired-state types, TOML/JSON (de)serialization and validation |
| `crates/hearthgate-store` | SQLite revision store |
| `crates/hearthgate-proxy` | Listeners, L4 and HTTP proxying, TLS, load balancing, metrics |
| `crates/hearthgate-acme` | Certificate storage and ACME issuance |
| `crates/hearthgate-services` | Health checks and read-only Docker discovery |
| `crates/hearthgate-firewall` | nftables ruleset, safe apply, blocklist feeds and automatic bans |
| `crates/hearthgate-upnp` | Router port forwarding with UPnP IGD and NAT-PMP |
| `crates/hearthgate-docker` | Managed containers and Compose stacks |
| `crates/hearthgate-vpn` | WireGuard interface, keys, peer configs and the internal DNS server |
| `crates/hearthgate-api` | Admin REST API, OpenAPI document, logins, admin listeners and the embedded dashboard |
| `dashboard` | The web dashboard (Svelte 5, Vite, TypeScript), built from the Figma UI kit |

### ACME tests

The ACME integration tests run against [Pebble](https://github.com/letsencrypt/pebble), Let's Encrypt's test CA, and are skipped when it isn't installed. To run them:

```sh
go install github.com/letsencrypt/pebble/v2/cmd/pebble@v2.8.0
go install github.com/letsencrypt/pebble/v2/cmd/pebble-challtestsrv@v2.8.0
cargo test -p hearthgate-acme --test pebble
```

They expect `pebble` and `pebble-challtestsrv` in `~/go/bin` or on `PATH`, and use ports 14000, 15001 and 15002.

### Docker tests

`crates/hearthgate-docker/tests/docker.rs` creates, recreates and removes real containers and a Compose stack (all named `hgtest-…`), and is skipped when Docker or the `busybox:1.36` image isn't available. It needs the Docker daemon, the Compose plugin and access to `/var/run/docker.sock`: `cargo test -p hearthgate-docker --test docker`.

### Firewall and VPN tests

`crates/hearthgate-firewall/tests/netns.rs` builds three network namespaces (outside, this host, LAN), loads real rulesets and scans the host from outside. `hearthgate/tests/vpn_netns.rs` starts real WireGuard tunnels from two client namespaces and checks what each peer can reach, and that a removed peer is cut off. They never touch your own host's network, but they need root, `ip` (iproute2), `nft`, `wg` (wireguard-tools), `python3` and the WireGuard kernel module or `wireguard-go`, and are skipped without them. Run them with:

```sh
cargo test -p hearthgate-firewall --test netns -p hearthgate --test vpn_netns --no-run
sudo -E env "PATH=$PATH" cargo test -p hearthgate-firewall --test netns
sudo -E env "PATH=$PATH" cargo test -p hearthgate --test vpn_netns
```

The dashboard has one more end-to-end test, skipped by default, that turns the firewall on for real and checks that the dashboard confirms the change. Run it only as root in a throwaway machine or container: `HEARTHGATE_E2E_FIREWALL=1 npm run test:e2e`.

### Docker discovery

The Docker tests use a fake container table, so they don't need Docker. To try discovery for real, run the daemon with a `docker` service and start or reconnect the container; `hearthgate status` and `/api/v1/status` show the address it resolved.

## Test locally before you push

CI minutes are limited, so treat CI as the final check, not your test runner. Before pushing, run:

```sh
cargo fmt --check
cargo clippy --all-targets -- -D warnings
cargo test
cd dashboard && npm test       # if you touched the dashboard or the API
```

`npm test` runs svelte-check, the component tests, a build, and the Playwright end-to-end tests, which start a real Hearthgate (built with `cargo build`) and check every page with axe for WCAG AA. Playwright needs Chromium: `npx playwright install chromium`.

If you changed dependencies, run `cargo deny check` (install it with `cargo install cargo-deny`): it fails on any RustSec advisory, a license that isn't compatible with MIT, or a source other than crates.io. An advisory that doesn't affect Hearthgate goes in `deny.toml` under `ignore`, with the reason.

If you touched code that parses untrusted input (anything from the network, a feed or an import), run the fuzzers for a few minutes:

```sh
rustup toolchain install nightly
cargo install cargo-fuzz
scripts/fuzz.sh 120              # seconds per target; or name targets after it
```

Targets live in `fuzz/fuzz_targets`, with starting inputs in `fuzz/seeds`. A crash leaves its input in `fuzz/artifacts/TARGET/`; reproduce it with `cargo +nightly fuzz run TARGET fuzz/artifacts/TARGET/crash-…` from `fuzz/`, and add a regular test for it with the fix. New parsers of untrusted input should get a target. See [the threat model](docs/THREAT_MODEL.md) for what is untrusted.

If you touched the build or release scripts, also run `scripts/build-release.sh 0.0.0-test` and `scripts/next-version.sh`. When CI does fail, fix everything it reports in one push rather than one failure at a time.

## Commit messages

We use [Conventional Commits](https://www.conventionalcommits.org). The format is:

```
<type>(<optional scope>)<optional !>: <short summary in the imperative>

<optional body: what and why>

<optional footer, e.g. BREAKING CHANGE: … or Closes #123>
```

Commit types decide the next version, so pick them carefully:

| Type | Use for | Release |
|---|---|---|
| `feat` | A new user-facing capability | minor |
| `fix` | A bug fix | patch |
| `perf` | A performance improvement | patch |
| `refactor` | Code change with no behavior change | none |
| `docs` | Documentation only | none |
| `test` | Adding or fixing tests | none |
| `build` | Build system, dependencies, packaging | none |
| `ci` | GitHub Actions and release tooling | none |
| `style` | Formatting only | none |
| `chore` | Anything else that is not user-facing | none |

Mark breaking changes with `!` after the type (`feat!: drop the v1 config format`) or a `BREAKING CHANGE:` footer that explains the migration. While Hearthgate is `0.x`, a breaking change bumps the minor version.

Useful scopes match the main areas: `proxy`, `tls`, `vpn`, `dns`, `firewall`, `upnp`, `docker`, `dashboard`, `ai`, `config`, `cli`, `systemd`.

Examples:

```
feat(vpn): add peer QR codes to the dashboard
fix(tls): retry ACME orders after a rate-limit response
docs: explain how to run Hearthgate behind CGNAT
feat(config)!: rename `expose` to `public_interface`
```

## Pull requests

1. Open an issue first for anything larger than a small fix, so we can agree on the approach.
2. Branch from `main`. Keep each PR to one logical change.
3. **Title the PR as a Conventional Commit.** PRs are squash-merged, so the title becomes the commit on `main` and decides the release. CI checks the title format.
4. Fill in the PR template: what changed, why, and how you tested it.
5. CI must be green and one maintainer must approve. Address review comments with new commits; they are squashed on merge.
6. Never commit secrets, real domains you don't own, or private IPs from your own network in tests or examples. Use `example.com` and the documentation ranges (`192.0.2.0/24`, `198.51.100.0/24`, `203.0.113.0/24`).

## Code style

- **Formatting:** `rustfmt` with the repository's `rustfmt.toml`. CI runs `cargo fmt --check`.
- **Lints:** Clippy with `pedantic` enabled in `Cargo.toml`. CI fails on any warning. If a lint is wrong for a specific case, allow it locally with `#[allow(clippy::…)]` and a comment saying why.
- **No `unsafe`.** The crate forbids it. If something truly needs it, discuss it in an issue first.
- **Errors:** return errors instead of panicking. No `unwrap()` or `expect()` outside tests unless the invariant is explained in a comment.
- **Secure by default:** new features start closed or off, and opening them up is an explicit user choice.
- **Tests:** new behavior comes with tests. Bug fixes come with a test that fails without the fix.
- **Dependencies:** prefer well-maintained crates, keep the default feature set small, and say why a new dependency is needed in the PR.
- **User-facing text** (logs, CLI output, dashboard copy) follows the voice in [docs/brand/BRAND.md](docs/brand/BRAND.md): state the fact, then the next step. No exclamation marks, emoji or "Oops".

## How releases work

You don't release anything by hand. On every push to `main`, the release workflow:

1. Reads the commits since the last `vX.Y.Z` tag and computes the next version with `scripts/next-version.sh`.
2. Stops if there are only non-releasing commits (`chore`, `docs`, `ci`…).
3. Otherwise runs the tests, builds all targets with `scripts/build-release.sh`, and publishes a GitHub Release tagged `vX.Y.Z` with the archives, `SHA256SUMS` and notes generated by [git-cliff](https://git-cliff.org) from `cliff.toml`.

The version in `Cargo.toml` stays `0.0.0-dev`. Release builds get the real version from the `HEARTHGATE_VERSION` environment variable, so no version-bump commits are needed.

To preview what the next release would be, run `scripts/next-version.sh` locally.

## Brand assets

Logos and the brand guide live in [docs/brand](docs/brand). Follow the rules in `BRAND.md` when using them.
