# VPN and internal DNS

The built-in WireGuard VPN lets your devices reach every internal service, published or not, and its internal DNS resolves them by name at `name.home.internal`.

```toml
[vpn]
enabled = true
endpoint = "vpn.example.com"   # where devices connect; default: the first public domain, or the router's public address
# port = 51820                 # UDP, opened in the firewall and (with [router] upnp) on the router
# subnet = "10.88.0.0/24"      # this machine takes the first address

[[vpn.peers]]
name = "phone"
public_key = "Y2FD6uTgrq+/bbBAXCNOtE+PGk8Papoh7qaWBpQUXn0="
address = "10.88.0.2"
access = "all"                 # or a list of services: ["nas", "sshd"]

[dns]
# enabled = true
# zone = "home.internal"
# lan = false                  # also answer on this machine's LAN addresses
# upstream = ["1.1.1.1"]       # default: the resolvers in /etc/resolv.conf
```

The VPN is WireGuard. Hearthgate uses the kernel's WireGuard when it has one and `wireguard-go` otherwise, and needs `wg` (wireguard-tools) and `ip` (iproute2) installed. The easiest way to add a device is VPN peers, Add device in the dashboard, or `hearthgate vpn add NAME`: Hearthgate creates the device's keys and shows its configuration, with a QR code for the WireGuard phone app, exactly once. The private key is not stored anywhere. To use a key made on the device instead, add the peer with its public key as above.

A device with `access = "all"` reaches this machine, its local network and every service. A device with a list of services reaches only those services' ports. The firewall enforces this even when the firewall itself is off, and removing a peer cuts it off at once. To reach the local network, Hearthgate turns on IPv4 forwarding; if it can't, it says so in the dashboard and peers reach only this machine.

The internal DNS server answers on this machine's VPN address (10.88.0.1 by default). Every service resolves as `NAME.home.internal`, every device as `DEVICE.home.internal`, this machine as `hearthgate.home.internal`, and your public domains resolve to the VPN address so traffic stays inside the tunnel. Everything else is forwarded. Device configurations set it as their DNS server. The dashboard is reachable over the VPN too, at `https://10.88.0.1:7380` (or `hearthgate.home.internal`), unless you limited who can reach it.
