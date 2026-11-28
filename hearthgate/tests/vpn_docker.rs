//! VPN peers reach managed Docker containers, with a real WireGuard tunnel
//! and a real container:
//!
//! ```text
//!  phone (203.0.113.2) ──veth──┐
//!  guest (198.51.100.2) ──veth─┤ this host (Hearthgate, Docker) ── hearthgate network: web :8080 :9090
//!  lan (192.168.67.2) ──veth───┘
//! ```
//!
//! `phone` may reach everything, `guest` only the `web` service (port
//! 8080), and `lan` is a LAN host with a route to the containers through
//! this host, which must not get in. Docker 28 and later drop packets
//! routed to a container from another interface unless its network says
//! otherwise, so before the fix the internal DNS answered with the
//! container's address and nothing ever arrived.
//!
//! Unlike the other network tests this one needs the host's own network
//! namespace, where Docker runs: it adds interfaces and the `hearthgate`
//! nftables table there. So it only runs when `HEARTHGATE_HOST_TESTS` is
//! set (CI sets it), as root, with Docker, iproute2, nftables,
//! wireguard-tools and python3, and refuses to run when the host already
//! has a `hearthgate` table.

use std::collections::BTreeMap;
use std::process::{Command, Stdio};
use std::time::Duration;

use hearthgate_docker::{GATEWAY_MODE, GATEWAY_MODE_VALUE, Manager, NETWORK, NETWORK_LABEL};
use hearthgate_firewall::{Dynamic, Inputs, Nft, OpenPort, VpnPolicy, ruleset};
use hearthgate_model::{Access, DesiredState, Firewall};
use hearthgate_vpn::{ClientConfig, Device, Plan, Setup, Tools, client_routes, keys};

const IMAGE: &str = "busybox:1.36";
const SOCKET: &str = "/var/run/docker.sock";

fn sh(cmd: &str) {
    let status = Command::new("sh").args(["-c", cmd]).status().expect(cmd);
    assert!(status.success(), "{cmd}");
}

fn ok(cmd: &str) -> bool {
    Command::new("sh")
        .args(["-c", cmd])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .is_ok_and(|s| s.success())
}

fn output(cmd: &str) -> String {
    let out = Command::new("sh").args(["-c", cmd]).output().expect(cmd);
    String::from_utf8_lossy(&out.stdout).trim().to_owned()
}

struct Lab {
    id: u32,
    namespaces: Vec<String>,
    containers: Vec<String>,
    server: Option<String>,
}

impl Lab {
    fn ns(&self, name: &str) -> String {
        format!("hgvd-{name}-{}", self.id)
    }

    /// Which of `targets` (`ip:port`) accept a TCP connection from `name`.
    fn reachable(&self, name: &str, targets: &[String]) -> Vec<String> {
        let list: Vec<(String, u16)> = targets
            .iter()
            .map(|t| {
                let (ip, port) = t.rsplit_once(':').unwrap();
                (ip.to_owned(), port.parse().unwrap())
            })
            .collect();
        let script = format!(
            "import socket\n\
             ok = []\n\
             for ip, p in {list:?}:\n\
             \x20   try:\n\
             \x20       socket.create_connection((ip, p), timeout=1.5).close(); ok.append(f'{{ip}}:{{p}}')\n\
             \x20   except OSError:\n\
             \x20       pass\n\
             print(' '.join(ok))\n"
        );
        let out = Command::new("ip")
            .args(["netns", "exec", &self.ns(name), "python3", "-c", &script])
            .output()
            .expect("probe");
        String::from_utf8_lossy(&out.stdout)
            .split_whitespace()
            .map(str::to_owned)
            .collect()
    }

    /// Sets up a client namespace's tunnel from a rendered configuration,
    /// the way `wg-quick` would.
    fn client(&self, name: &str, config: &str) {
        let ns = self.ns(name);
        let dev = format!("hgc{}{}", &name[..1], self.id % 100_000);
        let mut wg_conf = String::new();
        let mut address = String::new();
        let mut routes = Vec::new();
        for line in config.lines() {
            if let Some(v) = line.strip_prefix("Address = ") {
                v.clone_into(&mut address);
            } else if !line.starts_with("DNS = ") {
                if let Some(v) = line.strip_prefix("AllowedIPs = ") {
                    routes = v.split(", ").map(str::to_owned).collect();
                }
                wg_conf.push_str(line);
                wg_conf.push('\n');
            }
        }
        if !ok(&format!(
            "ip netns exec {ns} ip link add {dev} type wireguard"
        )) {
            sh(&format!("ip netns exec {ns} wireguard-go {dev}"));
        }
        let mut wg = Command::new("ip")
            .args(["netns", "exec", &ns, "wg", "setconf", &dev, "/dev/stdin"])
            .stdin(Stdio::piped())
            .spawn()
            .unwrap();
        std::io::Write::write_all(wg.stdin.as_mut().unwrap(), wg_conf.as_bytes()).unwrap();
        drop(wg.stdin.take());
        assert!(wg.wait().unwrap().success(), "wg setconf");
        sh(&format!("ip -n {ns} address add {address} dev {dev}"));
        sh(&format!("ip -n {ns} link set {dev} up"));
        for route in routes {
            sh(&format!("ip -n {ns} route add {route} dev {dev}"));
        }
    }
}

impl Drop for Lab {
    fn drop(&mut self) {
        let _ = Command::new("nft")
            .args(["delete", "table", "inet", ruleset::TABLE])
            .status();
        for ns in &self.namespaces {
            let _ = Command::new("ip").args(["netns", "del", ns]).status();
        }
        if let Some(server) = &self.server {
            let _ = Command::new("ip").args(["link", "del", server]).status();
        }
        for name in &self.containers {
            let _ = Command::new("docker")
                .args(["rm", "-f", name])
                .stdout(Stdio::null())
                .status();
        }
        let _ = Command::new("sh")
            .args([
                "-c",
                &format!(
                    "pkill -f 'wireguard-go hg[a-z]*{}' || true",
                    self.id % 100_000
                ),
            ])
            .status();
    }
}

fn runnable() -> bool {
    if std::env::var_os("HEARTHGATE_HOST_TESTS").is_none() {
        eprintln!("skipping: changes the host's network; set HEARTHGATE_HOST_TESTS to run it");
        return false;
    }
    let root = output("id -u") == "0";
    let tools = ok("ip -V")
        && ok("nft -v")
        && ok("wg --version")
        && ok("python3 -V")
        && ok("docker info")
        && (ok(&format!("docker image inspect {IMAGE}")) || ok(&format!("docker pull {IMAGE}")));
    assert!(
        root && tools,
        "HEARTHGATE_HOST_TESTS is set, but this is not root or a tool (or Docker) is missing"
    );
    assert!(
        !ok(&format!("nft list table inet {}", ruleset::TABLE)),
        "this host already has a `{}` nftables table; not touching it",
        ruleset::TABLE
    );
    true
}

/// Leaves a `hearthgate` network the way Hearthgate 1.4 created it, with a
/// container on it, so the test also covers upgrading it.
fn old_network(lab: &mut Lab) -> String {
    if ok(&format!("docker network inspect {NETWORK}")) {
        let attached = output(&format!(
            "docker network inspect -f '{{{{len .Containers}}}}' {NETWORK}"
        ));
        assert_eq!(
            attached, "0",
            "the `{NETWORK}` network has containers; not touching it"
        );
        sh(&format!("docker network rm {NETWORK} >/dev/null"));
    }
    sh(&format!(
        "docker network create --label {NETWORK_LABEL}=true {NETWORK} >/dev/null"
    ));
    let old = format!("hgvd-old-{}", lab.id);
    lab.containers.push(old.clone());
    sh(&format!(
        "docker run -d --name {old} --network {NETWORK} {IMAGE} sleep 3600 >/dev/null"
    ));
    old
}

fn state(web: &str, phone: &str, guest: &str) -> DesiredState {
    DesiredState::from_toml(&format!(
        r#"
        [[containers]]
        name = "{web}"
        image = "{IMAGE}"
        command = ["sh", "-c", "httpd -p 9090; exec httpd -f -p 8080"]

        [[services]]
        name = "web"
        kind = "external"
        host = "{web}"
        ports = {{ http = 8080 }}

        [vpn]
        enabled = true
        port = 51998
        endpoint = "203.0.113.1"

        [[vpn.peers]]
        name = "phone"
        public_key = "{phone}"
        address = "10.88.0.2"

        [[vpn.peers]]
        name = "guest"
        public_key = "{guest}"
        address = "10.88.0.3"
        access = ["web"]
        "#
    ))
    .unwrap()
}

#[tokio::test]
async fn peers_reach_managed_containers() {
    if !runnable() {
        return;
    }
    let mut lab = Lab {
        id: std::process::id(),
        namespaces: Vec::new(),
        containers: Vec::new(),
        server: None,
    };
    let id = lab.id % 100_000;
    let old = old_network(&mut lab);

    // Managed Docker brings the network up to date and starts `web`.
    let web = format!("hgvd-web-{}", lab.id);
    lab.containers.push(web.clone());
    let server_key = keys::generate().unwrap();
    let phone_key = keys::generate().unwrap();
    let guest_key = keys::generate().unwrap();
    let desired = state(&web, &phone_key.public, &guest_key.public);
    let dir = tempfile::tempdir().unwrap();
    let manager = Manager::new(Some(std::path::Path::new(SOCKET)), dir.path());
    let problems = manager.reconcile(&desired).await;
    assert!(problems.is_empty(), "{problems:?}");
    assert_eq!(
        output(&format!(
            "docker network inspect -f '{{{{index .Options \"{GATEWAY_MODE}\"}}}}' {NETWORK}"
        )),
        GATEWAY_MODE_VALUE,
        "the network lets routed packets in"
    );
    assert!(
        output(&format!(
            "docker inspect -f '{{{{range $k, $v := .NetworkSettings.Networks}}}}{{{{$k}}}} {{{{end}}}}' {old}"
        ))
        .split_whitespace()
        .any(|n| n == NETWORK),
        "a container on the old network is on the new one too"
    );
    let status = manager.status();
    let address = status
        .containers
        .iter()
        .find(|c| c.name == web)
        .and_then(|c| c.address)
        .expect("web has an address");
    let subnet: ipnet::Ipv4Net = status.subnet.expect("a subnet").parse().unwrap();
    let targets = [format!("{address}:8080"), format!("{address}:9090")];
    let mut up = false;
    for _ in 0..50 {
        up = targets.iter().all(|t| {
            std::net::TcpStream::connect_timeout(&t.parse().unwrap(), Duration::from_millis(200))
                .is_ok()
        });
        if up {
            break;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    assert!(up, "web listens");

    for name in ["phone", "guest", "lan"] {
        let ns = lab.ns(name);
        sh(&format!("ip netns add {ns}"));
        sh(&format!("ip -n {ns} link set lo up"));
        lab.namespaces.push(ns);
    }
    for (name, dev, host_ip, peer_ip) in [
        ("phone", "p", "203.0.113.1/30", "203.0.113.2/30"),
        ("guest", "g", "198.51.100.1/30", "198.51.100.2/30"),
        ("lan", "l", "192.168.67.1/24", "192.168.67.2/24"),
    ] {
        let ns = lab.ns(name);
        let dev = format!("hgv{dev}{id}");
        sh(&format!(
            "ip link add {dev} type veth peer name {dev} netns {ns}"
        ));
        sh(&format!("ip addr add {host_ip} dev {dev}"));
        sh(&format!("ip -n {ns} addr add {peer_ip} dev {dev}"));
        sh(&format!("ip link set {dev} up"));
        sh(&format!("ip -n {ns} link set {dev} up"));
    }
    let guest = lab.ns("guest");
    sh(&format!(
        "ip -n {guest} route add 203.0.113.0/30 via 198.51.100.1"
    ));
    let lan = lab.ns("lan");
    sh(&format!("ip -n {lan} route add {subnet} via 192.168.67.1"));
    sh("sysctl -qw net.ipv4.ip_forward=1");

    // Without Hearthgate's table, the network is open to routed traffic:
    // that is what the guard below is for.
    let _ = lab.reachable("lan", &targets);
    assert_eq!(lab.reachable("lan", &targets), targets);

    let server = Device::new(&format!("hgvs{id}"), Tools::default());
    lab.server = Some(server.name.clone());
    let resolved = BTreeMap::from([
        ("web".to_owned(), vec![address]),
        (format!("container:{web}"), vec![address]),
    ]);
    let plan = Plan::new(&desired, &resolved, &[]);
    server
        .apply(&Setup {
            private_key: server_key.private.clone(),
            port: plan.port,
            address: plan.address,
            peers: plan.peers.clone(),
        })
        .await
        .unwrap();
    let config = |pair: &keys::KeyPair, peer: &str, access: &Access| {
        ClientConfig {
            private_key: pair.private.clone(),
            address: peer.parse().unwrap(),
            server_public_key: server_key.public.clone(),
            endpoint: hearthgate_vpn::endpoint(&desired, None).unwrap(),
            allowed_ips: client_routes(&desired, access, &[subnet.into()], &resolved),
            dns: Some((desired.vpn.server_address(), desired.dns.zone.clone())),
        }
        .render()
    };
    let guest_conf = config(
        &guest_key,
        "10.88.0.3",
        &Access::Services(vec!["web".into()]),
    );
    assert!(
        guest_conf.contains(&format!("{address}/32")),
        "the guest gets a route to web: {guest_conf}"
    );
    lab.client("phone", &config(&phone_key, "10.88.0.2", &Access::All));
    lab.client("guest", &guest_conf);
    // The first packets start the handshakes.
    let _ = lab.reachable("phone", &targets[..1]);
    let _ = lab.reachable("guest", &targets[..1]);

    let nft = Nft::default();
    for enabled in [false, true] {
        let inputs = Inputs {
            firewall: Firewall {
                enabled,
                // The worst case: the LAN is trusted on this host, but it
                // still does not get into the containers' network.
                allow_lan: true,
                ..Firewall::default()
            },
            open: vec![OpenPort {
                port: plan.port,
                tcp: false,
                udp: true,
                rate_limit: None,
            }],
            lan: vec!["192.168.67.0/24".parse().unwrap()],
            vpn: Some(VpnPolicy {
                interface: server.name.clone(),
                subnet: desired.vpn.subnet,
                server: desired.vpn.server_address(),
                dns: true,
                full: plan.full.clone(),
                allowed: plan.allowed.clone(),
            }),
            containers: Some(subnet),
            ..Inputs::default()
        };
        nft.apply(&ruleset::render(&inputs, &Dynamic::default()))
            .await
            .unwrap();
        let mode = if enabled {
            "firewall on"
        } else {
            "firewall off"
        };
        assert_eq!(
            lab.reachable("phone", &targets),
            targets,
            "{mode}: phone has full access, containers included"
        );
        assert_eq!(
            lab.reachable("guest", &targets),
            targets[..1],
            "{mode}: guest reaches only the web service's port"
        );
        assert!(
            lab.reachable("lan", &targets).is_empty(),
            "{mode}: the LAN does not get into the containers' network"
        );
    }

    server.remove().await.unwrap();
    lab.server = None;
}
