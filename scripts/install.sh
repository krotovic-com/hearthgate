#!/bin/sh
# Installs or upgrades Hearthgate from its GitHub releases.
#
#   curl -fsSL https://github.com/krotovic-com/hearthgate/releases/latest/download/install.sh | sudo sh
#
# Uses the .deb on Debian and Ubuntu, the .rpm on Fedora, RHEL and openSUSE,
# and the static archive elsewhere. Every download is checked against the
# release's SHA256SUMS, and SHA256SUMS is checked against the release
# signing key when minisign is installed. Environment:
#   HEARTHGATE_VERSION   a version such as 1.0.0 (default: the latest)
#   HEARTHGATE_NO_START  set to 1 to install without starting the service
set -eu

repo="krotovic-com/hearthgate"
# The release signing key. The same key is built into Hearthgate, which
# checks it before every update from the dashboard.
pubkey="RWQDbPNZZtyDzLtuWAw4DNX7OGSMq0cMIjTU24h2jCWHE5yliB4enY2M"

say() { printf '%s\n' "$*"; }
fail() {
  printf 'hearthgate install: %s\n' "$*" >&2
  exit 1
}

[ "$(uname -s)" = Linux ] || fail "Hearthgate runs on Linux only."
[ "$(id -u)" -eq 0 ] || fail "run as root, for example: curl -fsSL ... | sudo sh"
command -v systemctl >/dev/null 2>&1 || fail "systemd is required."

if command -v curl >/dev/null 2>&1; then
  fetch() { curl -fsSL --proto '=https' --tlsv1.2 -o "$2" "$1"; }
elif command -v wget >/dev/null 2>&1; then
  fetch() { wget -q --https-only -O "$2" "$1"; }
else
  fail "curl or wget is required."
fi

case "$(uname -m)" in
  x86_64 | amd64) target=x86_64-unknown-linux-musl deb=amd64 rpm=x86_64 ;;
  aarch64 | arm64) target=aarch64-unknown-linux-musl deb=arm64 rpm=aarch64 ;;
  armv7l | armv7*) target=armv7-unknown-linux-musleabihf deb=armhf rpm=armv7hl ;;
  *) fail "no release for $(uname -m)." ;;
esac

if [ -n "${HEARTHGATE_VERSION:-}" ]; then
  base="https://github.com/$repo/releases/download/v${HEARTHGATE_VERSION#v}"
else
  base="https://github.com/$repo/releases/latest/download"
fi

tmp="$(mktemp -d)"
trap 'rm -rf "$tmp"' EXIT INT TERM
fetch "$base/SHA256SUMS" "$tmp/SHA256SUMS" || fail "cannot download $base/SHA256SUMS"
if command -v minisign >/dev/null 2>&1; then
  fetch "$base/SHA256SUMS.minisig" "$tmp/SHA256SUMS.minisig" ||
    fail "cannot download $base/SHA256SUMS.minisig"
  minisign -Vm "$tmp/SHA256SUMS" -P "$pubkey" -q ||
    fail "SHA256SUMS is not signed by the Hearthgate release key; not installing."
else
  say "minisign is not installed, so only checksums are checked (install minisign to check the signature too)."
fi

if command -v dpkg >/dev/null 2>&1 && command -v apt-get >/dev/null 2>&1; then
  kind=deb pattern="_${deb}\.deb$"
elif command -v rpm >/dev/null 2>&1 && { command -v dnf >/dev/null 2>&1 || command -v zypper >/dev/null 2>&1 || command -v yum >/dev/null 2>&1; }; then
  kind=rpm pattern="\.${rpm}\.rpm$"
else
  kind=tar pattern="-${target}\.tar\.gz$"
fi
file="$(awk '{print $2}' "$tmp/SHA256SUMS" | grep -E "$pattern" | head -n 1)"
[ -n "$file" ] || fail "the release has no $kind package for $(uname -m)."

say "Downloading $file"
fetch "$base/$file" "$tmp/$file" || fail "cannot download $base/$file"
(cd "$tmp" && grep -E " $file\$" SHA256SUMS | sha256sum -c --quiet -) ||
  fail "checksum mismatch for $file; not installing it."

case "$kind" in
  deb) apt-get install -y "$tmp/$file" ;;
  rpm)
    if command -v dnf >/dev/null 2>&1; then
      dnf install -y "$tmp/$file"
    elif command -v zypper >/dev/null 2>&1; then
      zypper --non-interactive install --allow-unsigned-rpm "$tmp/$file"
    else
      yum install -y "$tmp/$file"
    fi
    ;;
  tar)
    tar -C "$tmp" -xzf "$tmp/$file"
    dir="$tmp/${file%.tar.gz}"
    install -m 0755 "$dir/hearthgate" /usr/local/bin/hearthgate
    install -D -m 0644 "$dir/hearthgate.sysusers" /usr/lib/sysusers.d/hearthgate.conf
    systemd-sysusers hearthgate.conf
    install -m 0644 "$dir/hearthgate.service" /etc/systemd/system/hearthgate.service
    if [ -e "$dir/hearthgate-upgrade.service" ]; then
      install -m 0644 "$dir/hearthgate-upgrade.service" "$dir/hearthgate-upgrade.path" /etc/systemd/system/
    fi
    install -d -m 0755 /etc/hearthgate
    [ -e /etc/hearthgate/hearthgate.toml ] ||
      install -m 0644 "$dir/examples/hearthgate.toml" /etc/hearthgate/hearthgate.toml
    install -D -m 0644 "$dir/60-hearthgate.conf" /usr/share/hearthgate/60-hearthgate.conf
    systemctl daemon-reload
    systemctl enable --now hearthgate-upgrade.path >/dev/null 2>&1 || true
    ;;
esac

if [ "${HEARTHGATE_NO_START:-0}" = 1 ]; then
  say "Installed. Start it with: systemctl enable --now hearthgate"
  exit 0
fi
systemctl enable hearthgate >/dev/null 2>&1
systemctl restart hearthgate

say ""
say "Hearthgate $(hearthgate --version 2>/dev/null | awk '{print $2}') is running."
say "Open the dashboard from a computer on your network: https://THIS-SERVER:7380/"
# The service creates its data directory as it starts; wait for it.
i=0
while :; do
  if code="$(runuser -u hearthgate -- hearthgate --config /etc/hearthgate/hearthgate.toml setup-code 2>"$tmp/err")"; then
    say "Your setup code for the first account: $code"
    break
  fi
  if grep -q "already exists" "$tmp/err"; then
    say "Log in with your existing account."
    break
  fi
  i=$((i + 1))
  if [ "$i" -ge 20 ]; then
    say "Print your setup code with: sudo -u hearthgate hearthgate setup-code"
    break
  fi
  sleep 1
done
