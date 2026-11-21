# Install

The quickest way is the install script, which uses the `.deb` or `.rpm` package where it can and checks every download against the release's `SHA256SUMS`:

```sh
curl -fsSL https://github.com/krotovic-com/hearthgate/releases/latest/download/install.sh | sudo sh
```

Set `HEARTHGATE_VERSION=1.0.0` to install a particular release, and `HEARTHGATE_NO_START=1` to install without starting the service. Running it again upgrades. The packages put the binary in `/usr/bin`, the unit in `/usr/lib/systemd/system`, and the bootstrap file in `/etc/hearthgate/hearthgate.toml`, which upgrades never overwrite. Removing the package keeps `/var/lib/hearthgate`; `apt purge` deletes it.

To install from an archive instead, download one for your architecture from the releases page, then:

```sh
sudo install -m 0755 hearthgate /usr/local/bin/hearthgate
sudo install -m 0644 hearthgate.sysusers /usr/lib/sysusers.d/hearthgate.conf
sudo systemd-sysusers
sudo install -m 0644 hearthgate.service /etc/systemd/system/hearthgate.service
sudo install -d -m 0755 /etc/hearthgate
sudo install -m 0644 examples/hearthgate.toml /etc/hearthgate/hearthgate.toml
sudo systemctl daemon-reload
sudo systemctl enable --now hearthgate
```

The service runs as the unprivileged `hearthgate` user with only the capabilities it needs to bind ports below 1024, manage its firewall and set up the VPN. Its data lives in `/var/lib/hearthgate`.

For the VPN, install `wireguard-tools` (and `wireguard-go` if your kernel has no WireGuard module), and let VPN devices reach your local network by turning on IPv4 forwarding at boot:

```sh
sudo install -m 0644 60-hearthgate.conf /etc/sysctl.d/60-hearthgate.conf
sudo sysctl --system
```

The packages and the install script put that file in `/usr/share/hearthgate/60-hearthgate.conf`; copy it from there. Forwarding stays off until you do.

Run CLI commands as root or as the `hearthgate` user, because they open the database in the data directory:

```sh
sudo -u hearthgate hearthgate apply state.toml
```
