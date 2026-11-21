#!/usr/bin/env bash
# Runs every fuzz target for a fixed time, starting from the seeds in
# fuzz/seeds. Needs a nightly toolchain and cargo-fuzz:
#
#   rustup toolchain install nightly
#   cargo install cargo-fuzz
#
# Usage: scripts/fuzz.sh [SECONDS] [TARGET...]   (default 60 s, all targets)
# A crash stops the run and leaves the input in fuzz/artifacts/TARGET/.
set -euo pipefail

seconds="${1:-60}"
shift || true
cd "$(dirname "$0")/../fuzz"

targets=("$@")
if ((${#targets[@]} == 0)); then
  mapfile -t targets < <(cargo +nightly fuzz list)
fi

cargo +nightly fuzz build
for target in "${targets[@]}"; do
  echo "==> $target for ${seconds}s"
  mkdir -p "corpus/$target"
  log="$(mktemp)"
  status=0
  cargo +nightly fuzz run "$target" "corpus/$target" "seeds/$target" -- \
    -max_total_time="$seconds" -max_len=65536 -timeout=10 -rss_limit_mb=2048 \
    -print_final_stats=1 >"$log" 2>&1 || status=$?
  grep -E '^stat::(number_of_executed_units|peak_rss_mb)' "$log" || true
  if ((status != 0)); then
    tail -n 40 "$log" >&2
    rm -f "$log"
    echo "!! $target failed: see fuzz/artifacts/$target/" >&2
    exit 1
  fi
  rm -f "$log"
done
