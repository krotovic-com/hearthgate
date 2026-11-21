#!/usr/bin/env bash
# Builds static Linux binaries for every release target and packages them
# into dist/. Runs the same way locally and in the release workflow.
#
# Requires: rustup, cargo-zigbuild, zig, nfpm and Node.js 22 (see
# CONTRIBUTING.md).
# Usage: scripts/build-release.sh VERSION [TARGET...]
set -euo pipefail

version="${1:?usage: $0 VERSION [TARGET...]}"
shift
targets=("$@")
if ((${#targets[@]} == 0)); then
  targets=(
    x86_64-unknown-linux-musl
    aarch64-unknown-linux-musl
    armv7-unknown-linux-musleabihf
  )
fi

root="$(cd "$(dirname "$0")/.." && pwd)"
cd "$root"

# The binary reads its version from HEARTHGATE_VERSION at compile time, so
# nothing in the repository has to be edited or committed for a release.
export HEARTHGATE_VERSION="$version"

# The dashboard is embedded in the binary, so build it first.
echo "==> Building the dashboard"
(cd dashboard && npm ci && npm run build)

rm -rf dist
mkdir -p dist

for target in "${targets[@]}"; do
  echo "==> Building $target"
  rustup target add "$target" >/dev/null
  cargo zigbuild --locked --release --target "$target"

  name="hearthgate-v$version-$target"
  stage="dist/$name"
  mkdir -p "$stage"
  cp "target/$target/release/hearthgate" "$stage/"
  cp LICENSE README.md "$stage/"
  cp packaging/systemd/hearthgate.service packaging/systemd/hearthgate.sysusers packaging/sysctl/60-hearthgate.conf "$stage/"
  mkdir -p "$stage/examples"
  cp packaging/hearthgate.toml packaging/examples/state.toml "$stage/examples/"
  tar -C dist -czf "dist/$name.tar.gz" "$name"

  # Debian and RPM packages: the binary in /usr/bin, the unit in
  # /usr/lib/systemd/system.
  case "$target" in
    x86_64-*) arch=amd64 ;;
    aarch64-*) arch=arm64 ;;
    armv7-*) arch=arm7 ;;
    *) arch="" ;;
  esac
  if [[ -n "$arch" ]]; then
    rm -rf target/package
    mkdir -p target/package
    cp "target/$target/release/hearthgate" target/package/
    sed 's#/usr/local/bin/hearthgate#/usr/bin/hearthgate#' packaging/systemd/hearthgate.service >target/package/hearthgate.service
    for format in deb rpm; do
      VERSION="$version" ARCH="$arch" \
        nfpm package --config packaging/nfpm/nfpm.yaml --packager "$format" --target dist/
    done
  fi
  rm -rf "$stage"
done

cp scripts/install.sh dist/install.sh
(cd dist && sha256sum ./*.tar.gz ./*.deb ./*.rpm ./install.sh | sed 's# \./# #' >SHA256SUMS)
echo "==> Artifacts"
ls -l dist
