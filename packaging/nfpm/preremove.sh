#!/bin/sh
# Stops the service before its files go. On a Debian upgrade the argument
# is "upgrade" and on an RPM upgrade it is 1: keep it running then, and
# postinstall restarts it on the new version.
set -e

case "$1" in
  upgrade | 1) exit 0 ;;
esac
if [ -d /run/systemd/system ]; then
  systemctl disable --now hearthgate-upgrade.path hearthgate >/dev/null 2>&1 || true
fi
exit 0
