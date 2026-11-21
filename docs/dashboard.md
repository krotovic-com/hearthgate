# First steps in the dashboard

Open `https://SERVER-LAN-ADDRESS:7380/` from a computer on the same network. Hearthgate finds the server's LAN addresses when it starts, listens on those and on loopback, and closes connections from anywhere else before reading a byte, so the dashboard is never reachable from the internet.

**First visit.** The dashboard asks for a setup code, a username and a password (at least 12 characters). The code proves you have access to the server. It is in the service log, or print it with:

```sh
sudo -u hearthgate hearthgate setup-code
```

The code stops working once the account exists.

**The certificate warning.** The dashboard uses HTTPS with a self-signed certificate made on first start, so your browser warns about it once. Before you accept it, compare its SHA-256 fingerprint (in the browser's certificate details) with the one Hearthgate logged:

```sh
journalctl -u hearthgate | grep fingerprint
```

They must match. To avoid the warning, give Hearthgate a certificate from your own CA with `admin.certificate` and `admin.private_key` in the bootstrap file. Plain HTTP requests to the port are redirected to HTTPS.

**Publishing a service.** Add the service (a container or a program on your network), then a public interface with its domain and ports, then a route between them on the Routes page. Edits go into a draft: the bar at the bottom shows what changed, **Review and apply** shows the diff, and applying stores it as a new revision. The **History** page shows every revision's diff and rolls back to any of them.

**Accounts.** Under Settings, Account you can change the password (other sessions are logged out), turn on two-factor login with an authenticator app, and create API tokens for scripts. Ten failed logins in 15 minutes from one address, or for one username, block further attempts for a while. Sessions end after 12 hours without use and after 7 days at most.

**Who can reach it.** Settings, Dashboard access shows the addresses and networks in effect and lets you choose your own (private ranges only). Changes apply at once; a change that would lock out the browser making it is refused. If you lock yourself out anyway, run `sudo -u hearthgate hearthgate admin-access --reset` and restart the service. Setting `admin.listen` or `admin.allow` in the bootstrap file fixes the values there instead, and the dashboard shows them read-only.

**Dark mode.** The dashboard follows your system's light or dark setting. To pick one for a browser, use Settings, General, Appearance.
