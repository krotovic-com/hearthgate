# Router port forwarding

Hearthgate can keep your router's port forwards in line with your public interfaces.

```toml
[router]
upnp = true
```

Off by default, because any program on your network can use UPnP. When on, Hearthgate asks your router, with UPnP IGD or NAT-PMP, to forward each public interface's ports to this machine. Forwards are renewed before their lease ends, removed when an interface goes away, and removed when Hearthgate stops. Settings, Router shows the router, your public address and each forward. If the router's own public address is private, another router or your provider's carrier-grade NAT sits in front of it, and the dashboard warns that forwarded ports may stay unreachable.
