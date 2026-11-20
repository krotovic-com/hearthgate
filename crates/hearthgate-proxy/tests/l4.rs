//! Layer 4 forwarding through a real gateway on loopback.
#![allow(clippy::format_push_string)]

use std::net::SocketAddr;
use std::time::Duration;

use hearthgate_model::DesiredState;
use hearthgate_proxy::{Gateway, NoContainers, SocketKey, Transport, compile};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream, UdpSocket};

/// A TCP server that answers every read with `tag` followed by the data.
async fn tagged_echo(tag: &'static str) -> u16 {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    tokio::spawn(async move {
        loop {
            let (mut s, _) = listener.accept().await.unwrap();
            tokio::spawn(async move {
                let mut buf = [0u8; 1024];
                loop {
                    match s.read(&mut buf).await {
                        Ok(0) | Err(_) => break,
                        Ok(n) => {
                            let mut out = tag.as_bytes().to_vec();
                            out.extend_from_slice(&buf[..n]);
                            if s.write_all(&out).await.is_err() {
                                break;
                            }
                        }
                    }
                }
            });
        }
    });
    port
}

async fn udp_echo(tag: &'static str) -> u16 {
    let socket = UdpSocket::bind("127.0.0.1:0").await.unwrap();
    let port = socket.local_addr().unwrap().port();
    tokio::spawn(async move {
        let mut buf = [0u8; 1500];
        loop {
            let (n, peer) = socket.recv_from(&mut buf).await.unwrap();
            let mut out = tag.as_bytes().to_vec();
            out.extend_from_slice(&buf[..n]);
            socket.send_to(&out, peer).await.unwrap();
        }
    });
    port
}

fn free_port() -> u16 {
    std::net::TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
        .port()
}

fn raw_state(listen: u16, protocol: &str, targets: &[u16]) -> DesiredState {
    let mut toml = format!(
        r#"
limits = {{ drain_timeout = "2s" }}
[[interfaces]]
name = "raw"
bind = ["127.0.0.1"]
listeners = [{{ port = {listen}, mode = "raw", protocol = "{protocol}" }}]
"#
    );
    for (i, port) in targets.iter().enumerate() {
        toml.push_str(&format!(
            "[[services]]\nname = \"up{i}\"\nkind = \"external\"\nhost = \"127.0.0.1\"\nports = {{ p = {port} }}\n"
        ));
    }
    let list: Vec<String> = (0..targets.len())
        .map(|i| format!("{{ service = \"up{i}\", port = \"p\" }}"))
        .collect();
    toml.push_str(&format!(
        "[[routes]]\nname = \"raw\"\ninterface = \"raw\"\nlistener = {listen}\ntargets = [{}]\n",
        list.join(", ")
    ));
    DesiredState::from_toml(&toml).unwrap()
}

async fn apply(gw: &Gateway, state: &DesiredState) {
    let plan = compile(state, gw.upstreams(), &NoContainers);
    let errors = gw.apply(&plan).await;
    assert!(errors.is_empty(), "{errors:?}");
}

async fn roundtrip(stream: &mut TcpStream, msg: &str) -> String {
    stream.write_all(msg.as_bytes()).await.unwrap();
    let mut buf = [0u8; 1024];
    let n = tokio::time::timeout(Duration::from_secs(5), stream.read(&mut buf))
        .await
        .expect("reply in time")
        .unwrap();
    String::from_utf8_lossy(&buf[..n]).into_owned()
}

fn addr(port: u16) -> SocketAddr {
    SocketAddr::from(([127, 0, 0, 1], port))
}

#[tokio::test]
async fn forwards_tcp_and_reloads_without_dropping_connections() {
    let a = tagged_echo("a:").await;
    let b = tagged_echo("b:").await;
    let listen = free_port();
    let gw = Gateway::new();

    apply(&gw, &raw_state(listen, "tcp", &[a])).await;
    let mut old = TcpStream::connect(addr(listen)).await.unwrap();
    assert_eq!(roundtrip(&mut old, "hi").await, "a:hi");

    // Point the route at b. The open connection stays on a, new ones go to b.
    apply(&gw, &raw_state(listen, "tcp", &[b])).await;
    assert_eq!(roundtrip(&mut old, "still").await, "a:still");
    let mut new = TcpStream::connect(addr(listen)).await.unwrap();
    assert_eq!(roundtrip(&mut new, "hi").await, "b:hi");

    // Remove the interface: the port closes at once, open connections drain.
    apply(&gw, &DesiredState::default()).await;
    assert!(
        TcpStream::connect(addr(listen)).await.is_err(),
        "port must be closed"
    );
    assert_eq!(roundtrip(&mut old, "draining").await, "a:draining");
    assert!(gw.status().await.is_empty());
    gw.shutdown(Duration::from_millis(100)).await;
}

#[tokio::test]
async fn round_robin_over_two_upstreams_and_failover() {
    let a = tagged_echo("a:").await;
    let dead = free_port(); // nothing listens here
    let b = tagged_echo("b:").await;
    let listen = free_port();
    let gw = Gateway::new();
    apply(&gw, &raw_state(listen, "tcp", &[a, b])).await;
    let mut seen = Vec::new();
    for _ in 0..4 {
        let mut s = TcpStream::connect(addr(listen)).await.unwrap();
        seen.push(roundtrip(&mut s, "x").await);
    }
    seen.sort();
    assert_eq!(seen, ["a:x", "a:x", "b:x", "b:x"]);

    // A dead target is skipped: every connection still succeeds.
    apply(&gw, &raw_state(listen, "tcp", &[dead, a])).await;
    for _ in 0..6 {
        let mut s = TcpStream::connect(addr(listen)).await.unwrap();
        assert_eq!(roundtrip(&mut s, "y").await, "a:y");
    }
    gw.shutdown(Duration::from_millis(100)).await;
}

#[tokio::test]
async fn forwards_udp_sessions() {
    let up = udp_echo("u:").await;
    let listen = free_port();
    let gw = Gateway::new();
    apply(&gw, &raw_state(listen, "both", &[up])).await;
    let status = gw.status().await;
    assert_eq!(status.len(), 2, "tcp and udp sockets");

    let client = UdpSocket::bind("127.0.0.1:0").await.unwrap();
    client.connect(addr(listen)).await.unwrap();
    for msg in ["one", "two"] {
        client.send(msg.as_bytes()).await.unwrap();
        let mut buf = [0u8; 100];
        let n = tokio::time::timeout(Duration::from_secs(5), client.recv(&mut buf))
            .await
            .expect("udp reply in time")
            .unwrap();
        assert_eq!(&buf[..n], format!("u:{msg}").as_bytes());
    }
    gw.shutdown(Duration::from_millis(100)).await;
}

#[tokio::test]
async fn changing_protocol_rebinds_only_what_changed() {
    let up = tagged_echo("t:").await;
    let listen = free_port();
    let gw = Gateway::new();
    apply(&gw, &raw_state(listen, "tcp", &[up])).await;
    let mut conn = TcpStream::connect(addr(listen)).await.unwrap();
    apply(&gw, &raw_state(listen, "both", &[up])).await;
    // The TCP socket was unchanged, so the connection is untouched.
    assert_eq!(roundtrip(&mut conn, "ok").await, "t:ok");
    let key = SocketKey {
        addr: addr(listen),
        transport: Transport::Udp,
    };
    assert!(gw.local_addr(&key).await.is_some());
    gw.shutdown(Duration::from_millis(100)).await;
}

#[tokio::test]
async fn reports_ports_it_cannot_bind() {
    let taken = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let port = taken.local_addr().unwrap().port();
    let up = tagged_echo("t:").await;
    let gw = Gateway::new();
    let plan = compile(
        &raw_state(port, "tcp", &[up]),
        gw.upstreams(),
        &NoContainers,
    );
    let errors = gw.apply(&plan).await;
    assert_eq!(errors.len(), 1);
    assert!(gw.status().await.is_empty());
}
