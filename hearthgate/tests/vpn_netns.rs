//! The VPN with real WireGuard tunnels, in throwaway network namespaces:
//!
//! ```text
//!  phone (203.0.113.2) ──veth──┐
//!                              srv (Hearthgate) ──veth── lan (192.168.66.2: nas :8080, other :2222)
//!  guest (198.51.100.2) ──veth─┘   sshd :22
//! ```
//!
//! `phone` may reach everything; `guest` only the `nas` service. Uses the
//! kernel's WireGuard when it has one, otherwise `wireguard-go`.
//!
//! Needs root, iproute2, nftables, wireguard-tools, python3, and the
//! WireGuard kernel module or `wireguard-go`. Without them the test prints
//! a note and passes; CI runs it as root.

use std::collections::BTreeMap;
use std::process::{Child, Command, Stdio};

use hearthgate_firewall::{Dynamic, Inputs, Nft, OpenPort, VpnPolicy, ruleset};
use hearthgate_model::DesiredState;
use hearthgate_vpn::{ClientConfig, Device, Plan, Setup, Tools, client_routes, keys};

fn sh(cmd: &str) {
    let status = Command::new("sh").args(["-c", cmd]).status().expect(cmd);
    assert!(status.success(), "{cmd}");
}

fn have(program: &str, arg: &str) -> bool {
    Command::new(program)
        .arg(arg)
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .is_ok()
}

struct Lab {
    id: u32,
    namespaces: Vec<String>,
    children: Vec<Child>,
}

impl Lab {
    fn ns(&self, name: &str) -> String {
        format!("hgvpn-{name}-{}", self.id)
    }

    fn in_ns(&self, name: &str, program: &str) -> Vec<String> {
        vec![
            "ip".into(),
            "netns".into(),
            "exec".into(),
            self.ns(name),
            program.into(),
        ]
    }

    fn tools(&self, name: &str) -> Tools {
        Tools {
            ip: self.in_ns(name, "ip"),
            wg: self.in_ns(name, "wg"),
            userspace: self.in_ns(name, "wireguard-go"),
        }
    }

    /// TCP listeners on `ports` in namespace `name`.
    fn listen(&mut self, name: &str, ports: &[u16]) {
        let script = format!(
            "import socket, select\n\
             socks = []\n\
             for p in {ports:?}:\n\
             \x20   s = socket.socket(); s.setsockopt(socket.SOL_SOCKET, socket.SO_REUSEADDR, 1)\n\
             \x20   s.bind(('0.0.0.0', p)); s.listen(64); socks.append(s)\n\
             print('ready', flush=True)\n\
             while True:\n\
             \x20   r, _, _ = select.select(socks, [], [])\n\
             \x20   for s in r:\n\
             \x20       c, _ = s.accept(); c.close()\n"
        );
        let mut child = Command::new("ip")
            .args(["netns", "exec", &self.ns(name), "python3", "-c", &script])
            .stdout(Stdio::piped())
            .spawn()
            .expect("start listeners");
        let mut line = String::new();
        let _ = std::io::BufRead::read_line(
            &mut std::io::BufReader::new(child.stdout.as_mut().unwrap()),
            &mut line,
        );
        assert_eq!(line.trim(), "ready");
        self.children.push(child);
    }

    /// Which of `targets` (`ip:port`) accept a TCP connection from `name`.
    fn reachable(&self, name: &str, targets: &[&str]) -> Vec<String> {
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
    fn client(&self, name: &str, config: &str) -> Device {
        let device = Device::new(
            &format!("hgc{}{}", &name[..1], self.id % 100_000),
            self.tools(name),
        );
        // `wg` reads only the WireGuard fields; `wg-quick` handles the rest.
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
        let ns = self.ns(name);
        let kernel = Command::new("ip")
            .args([
                "netns",
                "exec",
                &ns,
                "ip",
                "link",
                "add",
                &device.name,
                "type",
                "wireguard",
            ])
            .stderr(Stdio::null())
            .status()
            .is_ok_and(|s| s.success());
        if !kernel {
            let child = Command::new("ip")
                .args(["netns", "exec", &ns, "wireguard-go", &device.name])
                .status()
                .expect("wireguard-go");
            assert!(child.success());
        }
        let mut wg = Command::new("ip")
            .args([
                "netns",
                "exec",
                &ns,
                "wg",
                "setconf",
                &device.name,
                "/dev/stdin",
            ])
            .stdin(Stdio::piped())
            .spawn()
            .unwrap();
        std::io::Write::write_all(wg.stdin.as_mut().unwrap(), wg_conf.as_bytes()).unwrap();
        drop(wg.stdin.take());
        assert!(wg.wait().unwrap().success(), "wg setconf");
        sh(&format!(
            "ip -n {ns} address add {address} dev {}",
            device.name
        ));
        sh(&format!("ip -n {ns} link set {} up", device.name));
        for route in routes {
            sh(&format!("ip -n {ns} route add {route} dev {}", device.name));
        }
        device
    }
}

impl Drop for Lab {
    fn drop(&mut self) {
        for child in &mut self.children {
            let _ = child.kill();
            let _ = child.wait();
        }
        for ns in &self.namespaces {
            // Userspace tunnels go away with their namespace's interfaces;
            // their daemons exit when the device does.
            let _ = Command::new("ip").args(["netns", "del", ns]).status();
        }
        let _ = Command::new("sh")
            .args([
                "-c",
                &format!("pkill -f 'wireguard-go hgc.{}' || true", self.id % 100_000),
            ])
            .status();
    }
}

fn state(guest_access: &str, with_guest: bool, phone: &str, guest: &str) -> DesiredState {
    let guest_peer = if with_guest {
        format!(
            "[[vpn.peers]]\nname = \"guest\"\npublic_key = \"{guest}\"\naddress = \"10.88.0.3\"\naccess = {guest_access}\n"
        )
    } else {
        String::new()
    };
    DesiredState::from_toml(&format!(
        r#"
        [[services]]
        name = "sshd"
        kind = "external"
        host = "127.0.0.1"
        ports = {{ ssh = 22 }}

        [[services]]
        name = "nas"
        kind = "external"
        host = "192.168.66.2"
        ports = {{ web = 8080 }}

        [[services]]
        name = "other"
        kind = "external"
        host = "192.168.66.2"
        ports = {{ app = 2222 }}

        [vpn]
        enabled = true
        port = 51999
        endpoint = "203.0.113.1"

        [[vpn.peers]]
        name = "phone"
        public_key = "{phone}"
        address = "10.88.0.2"

        {guest_peer}
        "#
    ))
    .unwrap()
}

async fn install(lab: &Lab, server: &Device, key: &str, state: &DesiredState) {
    let plan = Plan::new(state, &BTreeMap::new(), &[]);
    server
        .apply(&Setup {
            private_key: key.to_owned(),
            port: plan.port,
            address: plan.address,
            peers: plan.peers.clone(),
        })
        .await
        .unwrap();
    let inputs = Inputs {
        open: vec![OpenPort {
            port: plan.port,
            tcp: false,
            udp: true,
            rate_limit: None,
        }],
        vpn: Some(VpnPolicy {
            interface: server.name.clone(),
            subnet: state.vpn.subnet,
            server: state.vpn.server_address(),
            dns: true,
            full: plan.full.clone(),
            allowed: plan.allowed.clone(),
        }),
        ..Inputs::default()
    };
    let nft = Nft::new(lab.in_ns("srv", "nft"));
    nft.apply(&ruleset::render(&inputs, &Dynamic::default()))
        .await
        .unwrap();
}

#[tokio::test]
async fn peers_reach_what_their_access_allows() {
    let root = Command::new("id")
        .arg("-u")
        .output()
        .is_ok_and(|o| String::from_utf8_lossy(&o.stdout).trim() == "0");
    let tools =
        have("ip", "-V") && have("nft", "-v") && have("wg", "--version") && have("python3", "-V");
    if !(root && tools) {
        assert!(
            std::env::var_os("HEARTHGATE_NETNS_TESTS").is_none(),
            "HEARTHGATE_NETNS_TESTS is set, but root or a tool is missing"
        );
        eprintln!("skipping: needs root, iproute2, nftables, wireguard-tools and python3");
        return;
    }
    let mut lab = Lab {
        id: std::process::id(),
        namespaces: Vec::new(),
        children: Vec::new(),
    };
    let id = lab.id % 100_000;
    for name in ["srv", "phone", "guest", "lan"] {
        let ns = lab.ns(name);
        sh(&format!("ip netns add {ns}"));
        sh(&format!("ip -n {ns} link set lo up"));
        lab.namespaces.push(ns);
    }
    let (srv, phone, guest, lan) = (
        lab.ns("srv"),
        lab.ns("phone"),
        lab.ns("guest"),
        lab.ns("lan"),
    );
    for (peer, dev, srv_ip, peer_ip) in [
        (&phone, "p", "203.0.113.1/30", "203.0.113.2/30"),
        (&guest, "g", "198.51.100.1/30", "198.51.100.2/30"),
        (&lan, "l", "192.168.66.1/24", "192.168.66.2/24"),
    ] {
        sh(&format!(
            "ip link add {dev}{id} netns {srv} type veth peer name {dev}{id} netns {peer}"
        ));
        sh(&format!("ip -n {srv} addr add {srv_ip} dev {dev}{id}"));
        sh(&format!("ip -n {peer} addr add {peer_ip} dev {dev}{id}"));
        sh(&format!("ip -n {srv} link set {dev}{id} up"));
        sh(&format!("ip -n {peer} link set {dev}{id} up"));
    }
    sh(&format!(
        "ip -n {guest} route add 203.0.113.0/30 via 198.51.100.1"
    ));
    sh(&format!(
        "ip netns exec {srv} sysctl -qw net.ipv4.ip_forward=1"
    ));
    lab.listen("srv", &[22]);
    lab.listen("lan", &[8080, 2222]);

    let server_key = keys::generate().unwrap();
    let phone_key = keys::generate().unwrap();
    let guest_key = keys::generate().unwrap();
    let server = Device::new(&format!("hgs{id}"), lab.tools("srv"));
    let desired = state(r#"["nas"]"#, true, &phone_key.public, &guest_key.public);
    install(&lab, &server, &server_key.private, &desired).await;

    let config = |pair: &keys::KeyPair, address: &str, access| {
        ClientConfig {
            private_key: pair.private.clone(),
            address: address.parse().unwrap(),
            server_public_key: server_key.public.clone(),
            endpoint: hearthgate_vpn::endpoint(&desired, None).unwrap(),
            allowed_ips: client_routes(
                &desired,
                access,
                &["192.168.66.0/24".parse().unwrap()],
                &BTreeMap::new(),
            ),
            dns: Some((desired.vpn.server_address(), desired.dns.zone.clone())),
        }
        .render()
    };
    let phone_conf = config(&phone_key, "10.88.0.2", &hearthgate_model::Access::All);
    assert!(phone_conf.contains("Endpoint = 203.0.113.1:51999"));
    assert!(phone_conf.contains("AllowedIPs = 10.88.0.0/24, 192.168.66.0/24"));
    let guest_conf = config(
        &guest_key,
        "10.88.0.3",
        &hearthgate_model::Access::Services(vec!["nas".into()]),
    );
    assert!(guest_conf.contains("AllowedIPs = 10.88.0.0/24, 192.168.66.2/32"));
    let _phone_dev = lab.client("phone", &phone_conf);
    let _guest_dev = lab.client("guest", &guest_conf);

    let targets = ["10.88.0.1:22", "192.168.66.2:8080", "192.168.66.2:2222"];
    // The first packets start the handshake.
    let _ = lab.reachable("phone", &targets[..1]);
    assert_eq!(
        lab.reachable("phone", &targets),
        targets,
        "phone has full access"
    );
    let _ = lab.reachable("guest", &targets[1..2]);
    assert_eq!(
        lab.reachable("guest", &targets),
        ["192.168.66.2:8080"],
        "guest reaches only the nas"
    );
    let peers = server.peers().await.unwrap();
    assert!(
        peers.iter().all(|p| p.latest_handshake.is_some()),
        "{peers:?}"
    );

    // Revoking the guest cuts it off at once.
    let without_guest = state("[]", false, &phone_key.public, &guest_key.public);
    install(&lab, &server, &server_key.private, &without_guest).await;
    assert!(
        lab.reachable("guest", &targets).is_empty(),
        "a revoked peer reaches nothing"
    );
    assert_eq!(lab.reachable("phone", &targets), targets);

    server.remove().await.unwrap();
    assert!(!server.exists().await);
}
