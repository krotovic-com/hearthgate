# Updates

Hearthgate checks for new releases once a day and shows them on the Overview and under Settings, Updates. From there one click downloads the release, checks its signature, backs up what runs now, installs it, and rolls back by itself if the new version doesn't come up healthy within two minutes.

Nothing installs by itself unless you turn it on.

## Settings

Under Settings, Updates, or in the state:

```toml
[updates]
check = true              # ask GitHub once a day (default)
auto_install = "off"      # "off" (default), "patch", or "minor"
window = "03:00-05:00"    # when automatic and scheduled installs may start, local time
```

- **off**: you get a notice and install when you choose.
- **patch**: 1.3.0 → 1.3.1 installs by itself in the window; 1.4.0 waits for you.
- **minor**: patch and minor releases install by itself. Major releases (2.0) and releases with breaking changes always wait for you.

`updates.check = false` in the bootstrap file (`/etc/hearthgate/hearthgate.toml`) turns checks off whatever the dashboard says.

## What an install does

1. **Download and verify.** The gateway downloads `SHA256SUMS`, its signature `SHA256SUMS.minisig`, and the one file this machine needs (the `.deb`, the `.rpm`, or the release archive, matching how Hearthgate was installed). It only talks to github.com over HTTPS, and only installs a release whose `SHA256SUMS` is signed with the Hearthgate release key built into the running version, for exactly that version.
2. **Back up.** A separate root helper takes over (see below). It copies the download into its own directory, checks the signature and checksum again, stops Hearthgate, and keeps the current binary, its service file and the state database.
3. **Install.** Through `apt`, `dnf`, `zypper` or `yum` for packages, so the package manager stays in sync; by replacing `/usr/local/bin/hearthgate` for installs made with `install.sh`'s archive.
4. **Restart.** Public traffic pauses for a few seconds.
5. **Health check.** The new version must start and report that its configuration is applied and the firewall loaded within two minutes. If it doesn't, the helper puts the backup back, starts the previous version, and marks the release as skipped so it doesn't install by itself again. **Try again** under Settings, Updates offers it again.

The dashboard follows along and reconnects after the restart; you can close it, the install keeps going on the server.

## Rolling back

The version before the last update is kept. **Roll back to …** under Settings, Updates (or `sudo hearthgate update rollback`) puts it back. Your current settings are kept; only if the older version can't read them do the settings from before the update come back.

## How it is built

The gateway runs as an unprivileged user in a read-only sandbox and can't replace itself, which is how it should be. Installs are done by a small root helper:

- `hearthgate-upgrade.path` watches `/var/lib/hearthgate/updates/request.json`. The gateway writes it when you click install or roll back.
- `hearthgate-upgrade.service` then runs `hearthgate upgrade apply` as root, without network access. It deletes the request first, trusts nothing the gateway wrote (it re-checks the signature on its own copy and refuses symbolic links), and records its progress in `/var/lib/hearthgate-upgrade/`, which the dashboard reads.

So a compromised gateway can at most ask for a validly signed, newer Hearthgate release to be installed. Follow an install on the server with `journalctl -fu hearthgate-upgrade`.

The packages and `install.sh` install and enable these units. Releases before 1.3 don't have them, so upgrade those once by hand (see [Backups and upgrades](backups.md)); every update after that works from the dashboard. Builds you installed some other way show available updates but leave installing to you.

## From the command line

| Command | What it does |
| --- | --- |
| `hearthgate update status` | Installed version, install method, a newer release, the last install |
| `hearthgate update check` | Ask GitHub now; changes nothing |
| `sudo hearthgate update install [VERSION]` | Download, verify and install the newest release, with the same backup and rollback |
| `sudo hearthgate update rollback` | Go back to the kept version |
| `hearthgate update history` | Past installs and rollbacks |

## Checking a release by hand

The release key is in [`packaging/release-key.pub`](https://github.com/krotovic-com/hearthgate/blob/main/packaging/release-key.pub):

```sh
minisign -Vm SHA256SUMS -P RWQDbPNZZtyDzLtuWAw4DNX7OGSMq0cMIjTU24h2jCWHE5yliB4enY2M
sha256sum -c --ignore-missing SHA256SUMS
```

`install.sh` does the same when `minisign` is installed.
