//! Configuration files for peers, and QR codes of them for phone apps.

use std::fmt::Write as _;
use std::net::Ipv4Addr;

use ipnet::IpNet;
use qrcode::QrCode;
use qrcode::render::svg;

/// What goes into a peer's configuration.
#[derive(Clone)]
pub struct ClientConfig {
    pub private_key: String,
    pub address: Ipv4Addr,
    pub server_public_key: String,
    /// `host:port` the peer connects to.
    pub endpoint: String,
    /// Networks sent through the tunnel.
    pub allowed_ips: Vec<IpNet>,
    /// DNS server inside the VPN, and the zone to search.
    pub dns: Option<(Ipv4Addr, String)>,
}

impl ClientConfig {
    /// The file WireGuard apps and `wg-quick` import.
    #[must_use]
    pub fn render(&self) -> String {
        let mut out = format!(
            "[Interface]\nPrivateKey = {}\nAddress = {}/32\n",
            self.private_key, self.address
        );
        if let Some((server, zone)) = &self.dns {
            let _ = writeln!(out, "DNS = {server}, {zone}");
        }
        let ips: Vec<String> = self.allowed_ips.iter().map(ToString::to_string).collect();
        let _ = write!(
            out,
            "\n[Peer]\nPublicKey = {}\nEndpoint = {}\nAllowedIPs = {}\nPersistentKeepalive = 25\n",
            self.server_public_key,
            self.endpoint,
            ips.join(", ")
        );
        out
    }
}

/// A QR code of `text` as an SVG document.
#[must_use]
pub fn qr_svg(text: &str) -> Option<String> {
    let code = QrCode::new(text.as_bytes()).ok()?;
    Some(
        code.render::<svg::Color<'_>>()
            .min_dimensions(256, 256)
            .quiet_zone(true)
            .dark_color(svg::Color("#0b1220"))
            .light_color(svg::Color("#ffffff"))
            .build(),
    )
}

/// A QR code of `text` drawn with block characters, for terminals.
#[must_use]
pub fn qr_terminal(text: &str) -> Option<String> {
    let code = QrCode::new(text.as_bytes()).ok()?;
    let width = code.width();
    let colors = code.to_colors();
    let dark = |x: usize, y: usize| -> bool {
        x < width && y < width && colors[y * width + x] == qrcode::Color::Dark
    };
    // Two rows per line with half blocks, inverted so it scans on dark
    // terminals, and a quiet zone of two modules.
    let mut out = String::new();
    let pad = 2usize;
    let size = width + 2 * pad;
    let mut y = 0;
    while y < size {
        for x in 0..size {
            let at = |y: usize| x >= pad && y >= pad && dark(x - pad, y - pad);
            let top = at(y);
            let bottom = y + 1 < size && at(y + 1);
            out.push(match (top, bottom) {
                (false, false) => '█',
                (true, false) => '▄',
                (false, true) => '▀',
                (true, true) => ' ',
            });
        }
        out.push('\n');
        y += 2;
    }
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn renders_a_config() {
        let config = ClientConfig {
            private_key: "PRIV".into(),
            address: Ipv4Addr::new(10, 88, 0, 2),
            server_public_key: "PUB".into(),
            endpoint: "vpn.example.com:51820".into(),
            allowed_ips: vec![
                "10.88.0.0/24".parse().unwrap(),
                "192.168.1.0/24".parse().unwrap(),
            ],
            dns: Some((Ipv4Addr::new(10, 88, 0, 1), "home.internal".into())),
        };
        assert_eq!(
            config.render(),
            "[Interface]\nPrivateKey = PRIV\nAddress = 10.88.0.2/32\nDNS = 10.88.0.1, home.internal\n\n\
             [Peer]\nPublicKey = PUB\nEndpoint = vpn.example.com:51820\n\
             AllowedIPs = 10.88.0.0/24, 192.168.1.0/24\nPersistentKeepalive = 25\n"
        );
        let svg = qr_svg(&config.render()).unwrap();
        assert!(svg.starts_with("<?xml"));
        assert!(
            !svg.contains("style="),
            "no inline styles under the dashboard's CSP"
        );
        assert!(qr_terminal("hello").unwrap().lines().count() > 10);
    }
}
