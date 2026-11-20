//! Firewall tests with real traffic, in throwaway network namespaces:
//!
//! ```text
//!  out (203.0.113.2) ──veth── host (203.0.113.1, 192.168.77.1) ──veth── lan (192.168.77.2)
//! ```
//!
//! The host namespace gets the Hearthgate table and TCP listeners; the
//! other two connect to it. Nothing touches the machine's own rules.
//!
//! These need root and `ip` (iproute2). Without them the tests print a note
//! and pass, so `cargo test` works anywhere; CI runs them with sudo.

use std::process::{Child, Command, Stdio};
use std::time::Duration;

use hearthgate_firewall::{Dynamic, Inputs, Nft, OpenPort, Reason, ruleset};
use hearthgate_model::{Firewall, FirewallRule, PortRange, Protocol, RuleAction};

const OUT: &str = "203.0.113.2";
const HOST_WAN: &str = "203.0.113.1";
const HOST_LAN: &str = "192.168.77.1";

struct Lab {
    host: String,
    out: String,
    lan: String,
    listener: Option<Child>,
}

impl Lab {
    fn new(tag: &str) -> Option<Self> {
        let root = Command::new("id")
            .arg("-u")
            .output()
            .is_ok_and(|o| String::from_utf8_lossy(&o.stdout).trim() == "0");
        let ip = Command::new("ip").arg("-V").output().is_ok();
        let nft = Command::new("nft").arg("-v").output().is_ok();
        if !(root && ip && nft) {
            // CI sets this, so a broken setup fails instead of passing quietly.
            assert!(
                std::env::var_os("HEARTHGATE_NETNS_TESTS").is_none(),
                "HEARTHGATE_NETNS_TESTS is set, but root, iproute2 or nftables is missing"
            );
            eprintln!("skipping: needs root, iproute2 and nftables");
            return None;
        }
        let id = std::process::id();
        let lab = Self {
            host: format!("hgfw-{tag}-h{id}"),
            out: format!("hgfw-{tag}-o{id}"),
            lan: format!("hgfw-{tag}-l{id}"),
            listener: None,
        };
        for ns in [&lab.host, &lab.out, &lab.lan] {
            sh(&format!("ip netns add {ns}"));
            sh(&format!("ip -n {ns} link set lo up"));
        }
        let (h, o, l) = (&lab.host, &lab.out, &lab.lan);
        sh(&format!(
            "ip link add w{id} netns {h} type veth peer name w{id} netns {o}"
        ));
        sh(&format!(
            "ip link add l{id} netns {h} type veth peer name l{id} netns {l}"
        ));
        sh(&format!("ip -n {h} addr add {HOST_WAN}/24 dev w{id}"));
        sh(&format!("ip -n {o} addr add {OUT}/24 dev w{id}"));
        sh(&format!("ip -n {h} addr add {HOST_LAN}/24 dev l{id}"));
        sh(&format!("ip -n {l} addr add 192.168.77.2/24 dev l{id}"));
        for (ns, dev) in [(h, "w"), (o, "w"), (h, "l"), (l, "l")] {
            sh(&format!("ip -n {ns} link set {dev}{id} up"));
        }
        sh(&format!("ip -n {o} route add default via {HOST_WAN}"));
        sh(&format!("ip -n {l} route add default via {HOST_LAN}"));
        Some(lab)
    }

    fn nft(&self) -> Nft {
        Nft::new(vec![
            "ip".into(),
            "netns".into(),
            "exec".into(),
            self.host.clone(),
            "nft".into(),
        ])
    }

    /// TCP listeners on `ports` in the host namespace.
    fn listen(&mut self, ports: &[u16]) {
        let script = format!(
            "import socket, select\n\
             socks = []\n\
             for p in {ports:?}:\n\
             \x20   s = socket.socket(); s.setsockopt(socket.SOL_SOCKET, socket.SO_REUSEADDR, 1)\n\
             \x20   s.bind(('0.0.0.0', p)); s.listen(512); socks.append(s)\n\
             print('ready', flush=True)\n\
             while True:\n\
             \x20   r, _, _ = select.select(socks, [], [])\n\
             \x20   for s in r:\n\
             \x20       c, _ = s.accept(); c.close()\n"
        );
        let mut child = Command::new("ip")
            .args(["netns", "exec", &self.host, "python3", "-c", &script])
            .stdout(Stdio::piped())
            .spawn()
            .expect("start listeners");
        let mut line = String::new();
        let _ = std::io::BufRead::read_line(
            &mut std::io::BufReader::new(child.stdout.as_mut().unwrap()),
            &mut line,
        );
        assert_eq!(line.trim(), "ready");
        self.listener = Some(child);
    }

    /// Which of `ports` on `target` accept a TCP connection from `ns`.
    #[allow(clippy::unused_self)] // reads better as a lab method
    fn open_ports(&self, ns: &str, target: &str, ports: &[u16]) -> Vec<u16> {
        let script = format!(
            "import socket\n\
             ok = []\n\
             for p in {ports:?}:\n\
             \x20   try:\n\
             \x20       socket.create_connection(('{target}', p), timeout=0.5).close(); ok.append(p)\n\
             \x20   except OSError:\n\
             \x20       pass\n\
             print(' '.join(map(str, ok)))\n"
        );
        let out = Command::new("ip")
            .args(["netns", "exec", ns, "python3", "-c", &script])
            .output()
            .expect("probe");
        String::from_utf8_lossy(&out.stdout)
            .split_whitespace()
            .filter_map(|p| p.parse().ok())
            .collect()
    }

    /// Opens `n` connections as fast as possible; returns how many worked.
    #[allow(clippy::unused_self)]
    fn burst(&self, ns: &str, target: &str, port: u16, n: usize) -> usize {
        let script = format!(
            "import socket\n\
             ok = 0\n\
             for _ in range({n}):\n\
             \x20   try:\n\
             \x20       socket.create_connection(('{target}', {port}), timeout=0.3).close(); ok += 1\n\
             \x20   except OSError:\n\
             \x20       pass\n\
             print(ok)\n"
        );
        let out = Command::new("ip")
            .args(["netns", "exec", ns, "python3", "-c", &script])
            .output()
            .expect("burst");
        String::from_utf8_lossy(&out.stdout)
            .trim()
            .parse()
            .unwrap_or(0)
    }
}

impl Drop for Lab {
    fn drop(&mut self) {
        if let Some(mut child) = self.listener.take() {
            let _ = child.kill();
            let _ = child.wait();
        }
        for ns in [&self.host, &self.out, &self.lan] {
            let _ = Command::new("ip").args(["netns", "del", ns]).status();
        }
    }
}

fn sh(cmd: &str) {
    let status = Command::new("sh").args(["-c", cmd]).status().expect(cmd);
    assert!(status.success(), "{cmd}");
}

fn inputs() -> Inputs {
    Inputs {
        firewall: Firewall {
            enabled: true,
            ..Firewall::default()
        },
        open: vec![OpenPort {
            port: 443,
            tcp: true,
            udp: false,
            rate_limit: None,
        }],
        admin: Some((vec![7380], vec!["192.168.77.0/24".parse().unwrap()])),
        lan: vec!["192.168.77.0/24".parse().unwrap()],
        trusted_interfaces: Vec::new(),
        vpn: None,
    }
}

const PORTS: [u16; 4] = [22, 443, 7380, 8080];

#[tokio::test(flavor = "current_thread")]
async fn outside_only_reaches_public_ports() {
    let Some(mut lab) = Lab::new("scan") else {
        return;
    };
    lab.listen(&PORTS);
    // Without the table everything is open.
    assert_eq!(lab.open_ports(&lab.out, HOST_WAN, &PORTS), PORTS);

    let nft = lab.nft();
    let inputs = inputs();
    nft.apply(&ruleset::render(&inputs, &Dynamic::default()))
        .await
        .unwrap();
    assert!(nft.installed().await);
    // A port scan from outside finds only the public port.
    assert_eq!(lab.open_ports(&lab.out, HOST_WAN, &PORTS), [443]);
    assert_eq!(lab.open_ports(&lab.out, HOST_LAN, &PORTS), [443]);
    // The LAN keeps reaching everything, SSH included.
    assert_eq!(lab.open_ports(&lab.lan, HOST_LAN, &PORTS), PORTS);

    // The drops were recorded.
    let dropped = nft.dropped().await.unwrap();
    assert!(dropped.policy >= 3, "{dropped:?}");
    let recent = nft.recent().await.unwrap();
    let ssh = recent
        .iter()
        .find(|d| d.port == 22 && d.source.to_string() == OUT)
        .expect("a recent drop for port 22");
    assert_eq!((ssh.reason, ssh.protocol.as_str()), (Reason::Policy, "tcp"));

    // Applying the same ruleset again works (the table is replaced).
    nft.apply(&ruleset::render(&inputs, &Dynamic::default()))
        .await
        .unwrap();

    // Turning the firewall off removes the table and opens everything.
    nft.apply(&ruleset::remove()).await.unwrap();
    assert!(!nft.installed().await);
    assert_eq!(lab.open_ports(&lab.out, HOST_WAN, &PORTS), PORTS);
    // Removing twice is fine.
    nft.apply(&ruleset::remove()).await.unwrap();
}

#[tokio::test(flavor = "current_thread")]
async fn rules_lists_bans_and_rate_limits() {
    let Some(mut lab) = Lab::new("rules") else {
        return;
    };
    lab.listen(&PORTS);
    let nft = lab.nft();
    let mut inputs = inputs();
    inputs.firewall.allow_lan = false;
    inputs.firewall.rules = vec![
        FirewallRule {
            name: "ssh-from-partner".into(),
            action: RuleAction::Allow,
            sources: vec!["203.0.113.0/24".parse().unwrap()],
            ports: vec![PortRange::single(22)],
            protocol: Protocol::Tcp,
            comment: String::new(),
        },
        FirewallRule {
            name: "no-8080".into(),
            action: RuleAction::Deny,
            sources: Vec::new(),
            ports: vec!["8000-8100".parse().unwrap()],
            protocol: Protocol::Both,
            comment: String::new(),
        },
    ];
    nft.apply(&ruleset::render(&inputs, &Dynamic::default()))
        .await
        .unwrap();
    assert_eq!(lab.open_ports(&lab.out, HOST_WAN, &PORTS), [22, 443]);
    // Without allow_lan the LAN gets the public port and the dashboard.
    assert_eq!(lab.open_ports(&lab.lan, HOST_LAN, &PORTS), [443, 7380]);

    // A blocklist entry drops the source before anything else.
    let dynamic = Dynamic {
        feeds: vec!["203.0.113.0/28".parse().unwrap()],
        bans: Vec::new(),
    };
    nft.apply(&ruleset::render(&inputs, &dynamic))
        .await
        .unwrap();
    assert_eq!(
        lab.open_ports(&lab.out, HOST_WAN, &PORTS),
        Vec::<u16>::new()
    );
    assert!(nft.dropped().await.unwrap().blocklist > 0);
    // The manual allow list wins over feeds.
    inputs.firewall.allow = vec![format!("{OUT}/32").parse().unwrap()];
    nft.apply(&ruleset::render(&inputs, &dynamic))
        .await
        .unwrap();
    assert_eq!(lab.open_ports(&lab.out, HOST_WAN, &PORTS), [22, 443]);
    inputs.firewall.allow.clear();
    nft.apply(&ruleset::render(&inputs, &Dynamic::default()))
        .await
        .unwrap();

    // Bans can be added and lifted without replacing the table.
    let out_ip = OUT.parse().unwrap();
    nft.apply(&ruleset::ban(out_ip, Duration::from_secs(600)))
        .await
        .unwrap();
    assert_eq!(
        lab.open_ports(&lab.out, HOST_WAN, &[443]),
        Vec::<u16>::new()
    );
    let bans = nft.bans().await.unwrap();
    assert_eq!(bans.len(), 1);
    assert!(bans[0].1 > Duration::from_secs(590));
    // Bans survive a table replacement when passed back in.
    nft.apply(&ruleset::render(
        &inputs,
        &Dynamic {
            feeds: Vec::new(),
            bans: bans.clone(),
        },
    ))
    .await
    .unwrap();
    assert_eq!(
        lab.open_ports(&lab.out, HOST_WAN, &[443]),
        Vec::<u16>::new()
    );
    nft.apply(&ruleset::unban(out_ip)).await.unwrap();
    assert_eq!(lab.open_ports(&lab.out, HOST_WAN, &[443]), [443]);

    // A rate limit of 5 new connections per second (burst 10) cuts off part
    // of a flood of 60. (Each dropped attempt waits for its timeout, which
    // gives the client time to earn new tokens, so many still get through.)
    inputs.open[0].rate_limit = Some(5);
    nft.apply(&ruleset::render(&inputs, &Dynamic::default()))
        .await
        .unwrap();
    let ok = lab.burst(&lab.out, HOST_WAN, 443, 60);
    assert!((5..60).contains(&ok), "{ok} of 60 connections got through");
    assert!(nft.dropped().await.unwrap().ratelimit > 0);
}

#[tokio::test(flavor = "current_thread")]
async fn a_large_feed_loads_quickly() {
    let Some(lab) = Lab::new("feed") else {
        return;
    };
    let nft = lab.nft();
    // 100,000 /24 networks with gaps, so they cannot be merged.
    let feeds: Vec<ipnet::IpNet> = (0..200_000u32)
        .step_by(2)
        .map(|i| {
            let a = 11 + (i >> 16);
            let b = (i >> 8) & 0xff;
            let c = i & 0xff;
            format!("{a}.{b}.{c}.0/24").parse().unwrap()
        })
        .collect();
    let script = ruleset::render(
        &inputs(),
        &Dynamic {
            feeds,
            bans: Vec::new(),
        },
    );
    let started = std::time::Instant::now();
    nft.apply(&script).await.unwrap();
    let took = started.elapsed();
    eprintln!("loaded 100,000 ranges in {took:?}");
    assert!(took < Duration::from_secs(10), "{took:?}");
}
