# Hearthgate documentation

Hearthgate is the single, secure entry point from the internet to your home server. It runs as one native Linux service and handles everything between your router and the things you host:

- **Public interfaces.** Domains with certificates that renew themselves (HTTP-01, TLS-ALPN-01 or DNS-01, wildcards included), on TCP or UDP ports you choose, routed to one service or load-balanced across several.
- **Services.** Programs anywhere on your network, or Docker containers and compose stacks that Hearthgate runs itself.
- **Firewall.** nftables rules, blocklists and rate limits, with every port closed unless a route opens it. A change that cuts the dashboard off is undone on its own.
- **VPN.** WireGuard for your devices, with internal DNS, so services you don't publish (like SSH) stay reachable from anywhere and from nowhere else.
- **Router.** UPnP opens and closes the router's ports for you, if you allow it.
- **Dashboard.** A web UI reachable only from your LAN or the VPN, where every change is a reviewed, reversible revision.
- **Notifications and backups.** Webhooks, ntfy or email when a certificate fails, a service goes down or a backup fails; daily backups that restore on a new machine.

Start with [Install](install.md), then [First steps in the dashboard](dashboard.md). [How it fits together](concepts.md) explains the three building blocks: public interfaces, services and routes. The source, releases and issue tracker are on [GitHub](https://github.com/krotovic-com/hearthgate). Hearthgate is MIT-licensed.
