#!/usr/bin/env bash
# Builds a data directory with an earlier release, for the upgrade test
# (hearthgate/tests/upgrade.rs). Run it once for each new release, and
# commit the result:
#
#   scripts/make-upgrade-fixture.sh 1.0.0
#
# It downloads the release's x86_64 binary from GitHub, applies the state in
# hearthgate/tests/upgrade/state.toml twice (two revisions), imports a
# certificate, creates the admin token and, from 0.3.0 on, a dashboard
# account. Needs curl, openssl and, from 0.3.0, a free port 27380.
set -euo pipefail

version="${1:?usage: $0 VERSION}"
root="$(cd "$(dirname "$0")/.." && pwd)"
out="$root/hearthgate/tests/upgrade/v$version"
work="$(mktemp -d)"
trap 'kill "${daemon:-}" 2>/dev/null || true; rm -rf "$work"' EXIT

name="hearthgate-v$version-x86_64-unknown-linux-musl"
curl -fsSL "https://github.com/krotovic-com/hearthgate/releases/download/v$version/$name.tar.gz" |
  tar -xz -C "$work"
bin="$work/$name/hearthgate"
data="$work/data"
export HEARTHGATE_CONFIG=/dev/null
hg() { "$bin" --data-dir "$data" "$@"; }

openssl req -x509 -newkey ec -pkeyopt ec_paramgen_curve:P-256 -nodes -days 3650 \
  -subj /CN=app.example.com -addext subjectAltName=DNS:app.example.com \
  -keyout "$work/key.pem" -out "$work/cert.pem" 2>/dev/null
hg cert import fixture --cert "$work/cert.pem" --key "$work/key.pem" >/dev/null
hg apply "$root/hearthgate/tests/upgrade/state.toml" -m "First revision" >/dev/null
sed 's/strategy = "least-connections"/strategy = "round-robin"/' \
  "$root/hearthgate/tests/upgrade/state.toml" >"$work/second.toml"
hg apply "$work/second.toml" -m "Second revision" >/dev/null
hg admin-token >"$work/admin-token.txt"

account=false
if hg setup-code >/dev/null 2>&1; then
  printf '[admin]\nlisten = ["127.0.0.1:27380"]\nallow = ["127.0.0.0/8"]\n' >"$work/hearthgate.toml"
  "$bin" --config "$work/hearthgate.toml" --data-dir "$data" run >/dev/null 2>&1 &
  daemon=$!
  for _ in $(seq 50); do
    curl -ksf https://127.0.0.1:27380/api/v1/auth/session >/dev/null 2>&1 && break
    sleep 0.2
  done
  code="$(hg setup-code)"
  curl -ksf -H 'content-type: application/json' \
    -d "{\"setup_code\":\"$code\",\"username\":\"fixture\",\"password\":\"fixture-password-1\"}" \
    https://127.0.0.1:27380/api/v1/auth/setup >/dev/null
  kill "$daemon"
  wait "$daemon" 2>/dev/null || true
  daemon=
  account=true
fi

hg export >"$work/expected.toml"
rm -rf "$out"
mkdir -p "$out"
# The daemon's database files are complete once it stopped.
cp -a "$data" "$out/data"
rm -f "$out"/data/state.db-wal "$out"/data/state.db-shm
cp "$work/expected.toml" "$out/expected.toml"
cat >"$out/fixture.json" <<JSON
{
  "version": "$version",
  "revisions": 2,
  "account": $account,
  "admin_token": "$(cat "$work/admin-token.txt")"
}
JSON
echo "Wrote $out"
