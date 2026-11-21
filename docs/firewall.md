# Firewall and blocklists

Off by default. When you turn it on, Hearthgate installs its own nftables table (`inet hearthgate`) and leaves every other table alone, including Docker's and your own. Inbound traffic is then dropped unless it is:

- for a port a public interface listens on,
- for the dashboard, from the addresses allowed to reach it,
- from the local network (unless `allow_lan = false`),
- a reply to a connection this machine opened, or
- allowed by one of your rules.

```toml
[firewall]
enabled = true
# allow_lan = true             # the LAN reaches every port, like SSH
# allow_ping = true            # answer ICMP echo from anywhere
# allow_docker_ports = false   # ports published with `docker run -p` stay LAN-only
allow = ["198.51.100.7"]       # never blocked by lists, countries or bans
block = ["203.0.113.0/24"]     # always dropped
countries = ["xx"]             # two-letter codes, address ranges from ipdeny.com

[[firewall.rules]]             # checked in order; the first match decides
name = "ssh-from-office"
action = "allow"               # or "deny"
sources = ["192.0.2.0/24"]     # default: any address
ports = [22, "8000-8100"]      # default: every port
protocol = "tcp"               # tcp, udp or both
comment = "Office VPN exit"

[[firewall.blocklists]]
name = "firehol-level1"
url = "https://raw.githubusercontent.com/firehol/blocklist-ipsets/master/firehol_level1.netset"
# refresh = "12h"
# enabled = true

[firewall.auto_ban]
# enabled = true
# threshold = 20               # points within the window that get a client banned
# window = "10m"
# ban_time = "1h"
```

Packets are checked in this order: addresses on the `allow` list skip the blocklists; then `block`, blocklists, countries and bans drop what they match; then the LAN, your rules, rate limits and public ports accept what they match; everything else is dropped without an answer. Private, loopback and other non-public ranges are never dropped by lists or bans, so a feed that lists them can't cut off your LAN.

**Blocklists** are plain text with one address or network per line (comments after `#` or `;` are fine), or Spamhaus's JSON format. They are downloaded over HTTPS on their schedule and cached in the data directory, so a restart doesn't wait for the network. The dashboard suggests Spamhaus DROP, FireHOL level 1 and abuse.ch Feodo Tracker.

**Automatic bans** come from what the proxy sees on public ports: a failed login (the service answered 401 or 403) or a failed TLS handshake counts 1 point, a request for a domain no interface serves counts 2, and a request for a well-known exploit path (such as `/.env` or `/wp-login.php`) counts 5. A client that reaches the threshold within the window is banned for `ban_time`. Bans survive configuration changes, and you can ban or unban an address by hand on the Firewall page.

**Rate limits** are set per interface: `rate_limit = 20` on an `[[interfaces]]` entry accepts at most 20 new connections a second from one address across its ports.

**Safe apply.** Any change to the firewall settings while it is on waits for proof that you can still reach Hearthgate. The dashboard sends that proof by itself over a new connection, and shows a banner meanwhile. From a shell, open a new SSH session and run `hearthgate firewall confirm`. If nothing confirms within 60 seconds, Hearthgate stores a new revision with the previous firewall settings, so a rule that locks you out undoes itself. Turning the firewall off never waits. When you are locked out anyway, `hearthgate firewall off` at the console turns it off.

Hearthgate needs `nft` (the `nftables` package) and the `CAP_NET_ADMIN` capability, which the systemd unit grants. When Hearthgate stops, its table stays in place, so the machine is not left open. Turning the firewall off makes a running Hearthgate remove it; with Hearthgate stopped, `sudo nft delete table inet hearthgate` does.
