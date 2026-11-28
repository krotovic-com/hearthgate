#!/bin/sh
# Takes the AI assistant screenshots on the website (site/assets/screens/) from
# a real Hearthgate with example data and a stand-in for a local model.
#   cargo build -p hearthgate   (with the dashboard built, and HEARTHGATE_VERSION set)
#   sudo scripts/site-screenshots/run.sh   (binds ports 80, 443 and 7380 on 127.0.0.1)
# Needs node with playwright, openssl and python3 with Pillow.
set -eu
here=$(cd "$(dirname "$0")" && pwd)
root=$(cd "$here/../.." && pwd)
bin=${HEARTHGATE_BIN:-$root/target/debug/hearthgate}
work=$(mktemp -d)
trap 'kill $(cat "$work"/pids) 2>/dev/null || true' EXIT

openssl req -x509 -newkey rsa:2048 -nodes -days 3650 -subj "/CN=example.com" \
  -addext "subjectAltName=DNS:example.com,DNS:*.example.com" \
  -keyout "$work/key.pem" -out "$work/cert.pem" 2>/dev/null
printf 'data_dir = "%s/data"\n[admin]\nlisten = ["127.0.0.1:7380"]\n[docker]\nenabled = false\n' "$work" > "$work/hearthgate.toml"

node "$here/fake.js" & echo $! >> "$work/pids"
"$bin" -c "$work/hearthgate.toml" cert import home --cert "$work/cert.pem" --key "$work/key.pem" >/dev/null
"$bin" -c "$work/hearthgate.toml" apply "$here/state.toml" >/dev/null
"$bin" -c "$work/hearthgate.toml" run >"$work/run.log" 2>&1 & echo $! >> "$work/pids"
sleep 3
# Some traffic, so the counters aren't empty.
for i in $(seq 1 150); do for h in cloud media home; do
  curl -sk --noproxy "*" --resolve "$h.example.com:443:127.0.0.1" "https://$h.example.com/" -o /dev/null || true
done; done

mkdir -p "$work/shots"
node "$here/shots.js" "$("$bin" -c "$work/hearthgate.toml" setup-code)" "$work/shots"
python3 - "$work/shots" "$root/site/assets/screens" <<'PY'
import sys
from PIL import Image
src, out = sys.argv[1], sys.argv[2]
for name in ("ask-ai-plan", "ask-ai-step2", "ai-review"):
    im = Image.open(f"{src}/{name}.png").convert("RGB")
    im.save(f"{out}/{name}@2x.webp", quality=82, method=6)
    im.resize((1440, 900), Image.LANCZOS).save(f"{out}/{name}.webp", quality=85, method=6)
PY
echo "site-screenshots: wrote site/assets/screens"
