# Certificates

`https` interfaces get certificates automatically from Let's Encrypt, or from any ACME CA:

```toml
[acme]
email = "you@example.com"
accept_terms = true          # required: you agree to the CA's terms of service
# staging = true             # Let's Encrypt staging while you test
# directory = "https://ca.example.lan/acme/directory"   # another CA
# directory_root = "/etc/hearthgate/ca.pem"            # its API's root certificate
# challenge = "tls-alpn-01"  # default: http-01 if the interface has an http listener
# renew_before = "30days"
```

For validation, the interface's domains must point at your public IP, and port 80 (HTTP-01) or 443 (TLS-ALPN-01) must reach Hearthgate. Certificates are renewed 30 days before they expire. Failed requests are retried after 5 minutes, then less and less often (up to every 6 hours), so a mistake does not run into the CA's rate limits. Until a certificate exists, Hearthgate serves a self-signed one, so browsers show a warning instead of failing to connect.

**DNS challenges.** With `challenge = "dns-01"`, Hearthgate proves you own the domain with a TXT record at your DNS provider instead. This works for wildcard domains (`*.example.com`) and needs no port open to the internet, so it also suits services reachable only over the VPN.

```toml
[acme]
accept_terms = true
challenge = "dns-01"

[acme.dns]
provider = "cloudflare"
api_token_file = "/etc/hearthgate/cloudflare-token"
# propagation = "30s"        # wait before the CA checks the records
```

Create the token in Cloudflare under My Profile, API Tokens, with the **Zone, DNS, Edit** permission for your zone. For any DNS server that accepts RFC 2136 updates (BIND, Knot, PowerDNS, Technitium, many routers):

```toml
[acme.dns]
provider = "rfc2136"
server = "192.168.1.2:53"
zone = "example.com"
key_name = "hearthgate"
# key_algorithm = "hmac-sha256"   # or hmac-sha384, hmac-sha512
key_file = "/etc/hearthgate/tsig.key"
```

`tsig-keygen hearthgate` on a BIND server prints a key file you can use as it is. Allow that key to change TXT records, for example `update-policy { grant hearthgate wildcard *.example.com. TXT; };`.

Secrets stay in files, never in the configuration or its history. Make them readable by Hearthgate only: `sudo chown root:hearthgate FILE && sudo chmod 0640 FILE`. The records are removed once the certificate is issued.

To use a certificate from elsewhere instead, upload it:

```sh
hearthgate cert import my-cert --cert fullchain.pem --key privkey.pem
hearthgate cert list
hearthgate cert remove my-cert
```

A running gateway picks up imported certificates within a second.
