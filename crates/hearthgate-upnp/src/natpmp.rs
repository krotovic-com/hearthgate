//! NAT-PMP (RFC 6886): the simpler protocol many routers speak besides, or
//! instead of, UPnP. Requests go to the default gateway on UDP port 5351.

use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::time::Duration;

use tokio::net::UdpSocket;

use crate::{Error, Protocol};

/// The NAT-PMP port on the gateway.
pub const PORT: u16 = 5351;

/// The IPv4 default gateway, from the routing table (`/proc/self/net/route`
/// still works when systemd hides the rest of `/proc`).
#[must_use]
pub fn default_gateway() -> Option<Ipv4Addr> {
    let text = std::fs::read_to_string("/proc/self/net/route")
        .or_else(|_| std::fs::read_to_string("/proc/net/route"))
        .ok()?;
    parse_route_table(&text)
}

fn parse_route_table(text: &str) -> Option<Ipv4Addr> {
    for line in text.lines().skip(1) {
        let fields: Vec<&str> = line.split_whitespace().collect();
        // Iface Destination Gateway Flags ...
        if fields.len() > 3 && fields[1] == "00000000" {
            let gw = u32::from_str_radix(fields[2], 16).ok()?;
            if gw != 0 {
                // The kernel prints it in host (little-endian) order.
                return Some(Ipv4Addr::from(gw.swap_bytes()));
            }
        }
    }
    None
}

/// A NAT-PMP gateway.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct NatPmp {
    pub addr: SocketAddr,
}

impl NatPmp {
    async fn request(&self, msg: &[u8], expect_op: u8) -> Result<Vec<u8>, Error> {
        let socket = UdpSocket::bind("0.0.0.0:0").await?;
        socket.connect(self.addr).await?;
        // RFC 6886 retries with doubling timeouts; three tries are plenty
        // on a LAN.
        let mut wait = Duration::from_millis(250);
        let mut buf = [0u8; 64];
        for _ in 0..3 {
            socket.send(msg).await?;
            if let Ok(Ok(n)) = tokio::time::timeout(wait, socket.recv(&mut buf)).await
                && n >= 8
                && buf[0] == 0
                && buf[1] == expect_op
            {
                let code = u16::from_be_bytes([buf[2], buf[3]]);
                if code != 0 {
                    return Err(Error::Router(format!(
                        "NAT-PMP error {code} ({})",
                        match code {
                            1 => "unsupported version",
                            2 => "not authorized",
                            3 => "network failure",
                            4 => "out of resources",
                            5 => "unsupported opcode",
                            _ => "unknown",
                        }
                    )));
                }
                return Ok(buf[..n].to_vec());
            }
            wait *= 2;
        }
        Err(Error::NotFound)
    }

    /// The router's public address. Also how support is detected.
    pub async fn external_ip(&self) -> Result<IpAddr, Error> {
        let resp = self.request(&[0, 0], 128).await?;
        if resp.len() < 12 {
            return Err(Error::Router("short NAT-PMP answer".into()));
        }
        Ok(IpAddr::V4(Ipv4Addr::new(
            resp[8], resp[9], resp[10], resp[11],
        )))
    }

    /// Maps `port` to this host for `lease` (zero removes the mapping).
    /// Returns the lifetime the router granted.
    pub async fn map(
        &self,
        protocol: Protocol,
        port: u16,
        lease: Duration,
    ) -> Result<Duration, Error> {
        let op = match protocol {
            Protocol::Udp => 1,
            Protocol::Tcp => 2,
        };
        let lifetime = u32::try_from(lease.as_secs()).unwrap_or(u32::MAX);
        let mut msg = vec![0, op, 0, 0];
        msg.extend_from_slice(&port.to_be_bytes());
        msg.extend_from_slice(&port.to_be_bytes());
        msg.extend_from_slice(&lifetime.to_be_bytes());
        let resp = self.request(&msg, 128 + op).await?;
        if resp.len() < 16 {
            return Err(Error::Router("short NAT-PMP answer".into()));
        }
        let external = u16::from_be_bytes([resp[10], resp[11]]);
        if lifetime > 0 && external != port {
            // Another host has the port; give back what we got.
            let mut undo = msg.clone();
            undo[6..8].copy_from_slice(&external.to_be_bytes());
            undo[8..12].copy_from_slice(&0u32.to_be_bytes());
            let _ = self.request(&undo, 128 + op).await;
            return Err(Error::Router(format!(
                "the router offered port {external} instead of {port}; another device has it"
            )));
        }
        let granted = u32::from_be_bytes([resp[12], resp[13], resp[14], resp[15]]);
        Ok(Duration::from_secs(u64::from(granted)))
    }

    pub async fn remove(&self, protocol: Protocol, port: u16) -> Result<(), Error> {
        self.map(protocol, port, Duration::ZERO).await.map(|_| ())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn finds_the_default_gateway() {
        let table = "Iface\tDestination\tGateway \tFlags\tRefCnt\tUse\tMetric\tMask\t\tMTU\tWindow\tIRTT\n\
                     eth0\t00000000\t0101A8C0\t0003\t0\t0\t100\t00000000\t0\t0\t0\n\
                     eth0\t0001A8C0\t00000000\t0001\t0\t0\t100\t00FFFFFF\t0\t0\t0\n";
        assert_eq!(
            parse_route_table(table),
            Some(Ipv4Addr::new(192, 168, 1, 1))
        );
        assert_eq!(parse_route_table("Iface\tDestination\n"), None);
    }
}
