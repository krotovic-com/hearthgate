#!/bin/sh
# The data in /var/lib/hearthgate stays, so reinstalling keeps the state,
# certificates and accounts. `apt purge` removes it.
set -e

if [ -d /run/systemd/system ]; then
  systemctl daemon-reload || true
fi
if [ "$1" = "purge" ]; then
  rm -rf /var/lib/hearthgate /etc/hearthgate
fi
exit 0
