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

## Test locally before you push

CI minutes are limited, so treat CI as the final check, not your test runner. Before pushing, run:

```sh
cargo fmt --check
cargo clippy --all-targets -- -D warnings
cargo test
```

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
