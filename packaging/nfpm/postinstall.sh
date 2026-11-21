#!/bin/sh
# Creates the hearthgate user and restarts the service on upgrades. A new
# install is left stopped until the person enables it, as Debian and
# Fedora expect.
set -e

if command -v systemd-sysusers >/dev/null 2>&1; then
  systemd-sysusers hearthgate.conf || systemd-sysusers
fi
if ! getent passwd hearthgate >/dev/null 2>&1 && command -v useradd >/dev/null 2>&1; then
  useradd --system --user-group --home-dir /var/lib/hearthgate --no-create-home \
    --shell /usr/sbin/nologin --comment "Hearthgate gateway" hearthgate
fi
if [ -d /run/systemd/system ]; then
  systemctl daemon-reload || true
  if systemctl is-active --quiet hearthgate; then
    systemctl restart hearthgate || true
  fi
fi
exit 0
