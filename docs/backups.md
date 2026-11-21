# Backups and upgrades

Hearthgate writes a backup once a day to `/var/lib/hearthgate/backups` and keeps the last 7. A backup holds everything it can't rebuild: the state with its whole history, dashboard accounts and API tokens, certificates, ACME accounts, the VPN server key, the dashboard certificate and the admin token. Change the schedule or the place in the bootstrap file:

```toml
[backup]
interval = "1day"
keep = 7
dir = "/srv/backups/hearthgate"   # must be writable: systemctl edit hearthgate, then ReadWritePaths=/srv/backups/hearthgate
```

Backups hold private keys and are readable only by the `hearthgate` user. Copy them off the machine (it's a `.tar.gz` you can inspect with `tar -tzf`), and keep them as safe as the server.

To restore, for example onto a new machine with Hearthgate installed:

```sh
sudo systemctl stop hearthgate
sudo -u hearthgate hearthgate backup restore /path/to/hearthgate-backup-20261121T040000Z.tar.gz
sudo systemctl start hearthgate
```

If the data directory already has a state, add `--force`: what was there is moved to `before-restore-TIME` inside it, not deleted. Managed containers and stacks are started again from the restored state; their volumes are not part of the backup, so back those up with your usual tools.

**Upgrading.** Replace the binary and restart the service. The state database is upgraded on start, from any earlier release in one step, and a backup made by an older release restores on a newer one. Going back to an older release after an upgrade isn't supported: restore a backup made before the upgrade instead.
