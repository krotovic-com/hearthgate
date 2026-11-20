//! Internal DNS: answers names in the internal zone (and a few exact names,
//! such as public domains that should reach this machine over the VPN)
//! from Hearthgate's own records, and relays everything else to upstream
//! resolvers. It listens only on the addresses it is given: the VPN
//! address, and the LAN addresses when allowed. Never on public ones.

use std::collections::BTreeMap;
use std::net::{IpAddr, SocketAddr};
use std::sync::Arc;
use std::time::Duration;

use hickory_proto::op::{Message, MessageType, OpCode, ResponseCode};
use hickory_proto::rr::rdata::{A, AAAA};
use hickory_proto::rr::{RData, Record, RecordType};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream, UdpSocket};
use tokio::sync::watch;
use tokio::task::JoinSet;
use tokio_util::sync::CancellationToken;
use tracing::{debug, info, warn};

/// How long answers from the internal zone may be cached.
const TTL: u32 = 30;
const UPSTREAM_TIMEOUT: Duration = Duration::from_secs(2);
/// Queries answered at once; more are dropped (clients retry).
const MAX_IN_FLIGHT: usize = 512;

/// Names and their addresses, lowercase without the trailing dot.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Zone {
    /// The internal zone, such as `home.internal`.
    pub zone: String,
    pub names: BTreeMap<String, Vec<IpAddr>>,
}

impl Zone {
    fn owns(&self, name: &str) -> bool {
        name == self.zone
            || name.ends_with(&format!(".{}", self.zone))
            || self.names.contains_key(name)
    }
}

/// What the server does.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Settings {
    pub listen: Vec<SocketAddr>,
    pub zone: Zone,
    pub upstream: Vec<SocketAddr>,
}

/// Upstream resolvers from `/etc/resolv.conf`, skipping local stub
/// resolvers that would send queries back here.
#[must_use]
pub fn system_resolvers() -> Vec<SocketAddr> {
    let text = std::fs::read_to_string("/etc/resolv.conf").unwrap_or_default();
    parse_resolv_conf(&text)
}

fn parse_resolv_conf(text: &str) -> Vec<SocketAddr> {
    let mut out = Vec::new();
    for line in text.lines() {
        let mut words = line.split_whitespace();
        if words.next() != Some("nameserver") {
            continue;
        }
        let Some(ip) = words.next().and_then(|w| w.parse::<IpAddr>().ok()) else {
            continue;
        };
        out.push(SocketAddr::new(ip, 53));
    }
    out
}

/// Answers one query. Returns the reply, or `None` to stay silent.
async fn answer(query: &[u8], settings: &Settings) -> Option<Vec<u8>> {
    let Ok(request) = Message::from_vec(query) else {
        // Not even a header: nothing sensible to answer.
        if query.len() >= 2 {
            let id = u16::from_be_bytes([query[0], query[1]]);
            return Message::error_msg(id, OpCode::Query, ResponseCode::FormErr)
                .to_vec()
                .ok();
        }
        return None;
    };
    if request.message_type() != MessageType::Query {
        return None;
    }
    let local = request.queries().first().and_then(|q| {
        let name = q.name().to_lowercase().to_ascii();
        let name = name.trim_end_matches('.').to_owned();
        settings.zone.owns(&name).then_some((q.clone(), name))
    });
    if request.op_code() != OpCode::Query || request.queries().len() != 1 {
        return Message::error_msg(request.id(), request.op_code(), ResponseCode::NotImp)
            .to_vec()
            .ok();
    }
    if let Some((q, name)) = local {
        let mut reply = Message::new();
        reply
            .set_id(request.id())
            .set_message_type(MessageType::Response)
            .set_op_code(OpCode::Query)
            .set_authoritative(true)
            .set_recursion_desired(request.recursion_desired())
            .set_recursion_available(true)
            .add_query(q.clone());
        match settings.zone.names.get(&name) {
            Some(addresses) => {
                for ip in addresses {
                    let rdata = match (ip, q.query_type()) {
                        (IpAddr::V4(v4), RecordType::A | RecordType::ANY) => RData::A(A(*v4)),
                        (IpAddr::V6(v6), RecordType::AAAA | RecordType::ANY) => {
                            RData::AAAA(AAAA(*v6))
                        }
                        _ => continue,
                    };
                    reply.add_answer(Record::from_rdata(q.name().clone(), TTL, rdata));
                }
            }
            None if name == settings.zone.zone => {}
            None => {
                reply.set_response_code(ResponseCode::NXDomain);
            }
        }
        return reply.to_vec().ok();
    }
    forward(query, request.id(), &settings.upstream).await
}

/// Relays a query to the first upstream that answers.
async fn forward(query: &[u8], id: u16, upstream: &[SocketAddr]) -> Option<Vec<u8>> {
    for server in upstream {
        let bind: SocketAddr = if server.is_ipv4() {
            ([0, 0, 0, 0], 0).into()
        } else {
            ([0u16; 8], 0).into()
        };
        let Ok(socket) = UdpSocket::bind(bind).await else {
            continue;
        };
        if socket.connect(server).await.is_err() || socket.send(query).await.is_err() {
            continue;
        }
        let mut buf = vec![0u8; 4096];
        match tokio::time::timeout(UPSTREAM_TIMEOUT, socket.recv(&mut buf)).await {
            // A connected socket only receives from `server`; the ID check
            // catches stray answers to an earlier query on the same port.
            Ok(Ok(n)) if n >= 12 && u16::from_be_bytes([buf[0], buf[1]]) == id => {
                buf.truncate(n);
                return Some(buf);
            }
            _ => debug!(%server, "no answer from upstream DNS"),
        }
    }
    Message::error_msg(id, OpCode::Query, ResponseCode::ServFail)
        .to_vec()
        .ok()
}

async fn serve_udp(
    socket: UdpSocket,
    settings: watch::Receiver<Arc<Settings>>,
    stop: CancellationToken,
) {
    let socket = Arc::new(socket);
    let mut tasks = JoinSet::new();
    let mut buf = vec![0u8; 4096];
    loop {
        let received = tokio::select! {
            () = stop.cancelled() => break,
            r = socket.recv_from(&mut buf) => r,
        };
        let Ok((n, from)) = received else { continue };
        while tasks.try_join_next().is_some() {}
        if tasks.len() >= MAX_IN_FLIGHT {
            continue;
        }
        let query = buf[..n].to_vec();
        let socket = Arc::clone(&socket);
        let settings = settings.borrow().clone();
        tasks.spawn(async move {
            if let Some(mut reply) = answer(&query, &settings).await {
                // Plain DNS over UDP fits 512 bytes unless the client says
                // otherwise; mark longer answers truncated so it retries
                // over TCP.
                let limit = edns_size(&query).unwrap_or(512).max(512);
                if reply.len() > limit {
                    reply.truncate(12);
                    reply[2] |= 0x02;
                    reply[6..12].fill(0);
                }
                let _ = socket.send_to(&reply, from).await;
            }
        });
    }
}

/// The UDP payload size a query advertises with EDNS(0).
fn edns_size(query: &[u8]) -> Option<usize> {
    let message = Message::from_vec(query).ok()?;
    message
        .extensions()
        .as_ref()
        .map(|e| usize::from(e.max_payload()))
}

async fn serve_tcp(
    listener: TcpListener,
    settings: watch::Receiver<Arc<Settings>>,
    stop: CancellationToken,
) {
    let mut tasks = JoinSet::new();
    loop {
        let accepted = tokio::select! {
            () = stop.cancelled() => break,
            r = listener.accept() => r,
        };
        let Ok((stream, _)) = accepted else { continue };
        while tasks.try_join_next().is_some() {}
        if tasks.len() >= MAX_IN_FLIGHT {
            continue;
        }
        let settings = settings.clone();
        tasks.spawn(async move {
            let _ =
                tokio::time::timeout(Duration::from_secs(30), tcp_session(stream, settings)).await;
        });
    }
}

async fn tcp_session(
    mut stream: TcpStream,
    settings: watch::Receiver<Arc<Settings>>,
) -> std::io::Result<()> {
    for _ in 0..16 {
        let len = match tokio::time::timeout(Duration::from_secs(10), stream.read_u16()).await {
            Ok(Ok(len)) => usize::from(len),
            _ => return Ok(()),
        };
        let mut query = vec![0u8; len];
        stream.read_exact(&mut query).await?;
        let settings = settings.borrow().clone();
        let Some(reply) = answer(&query, &settings).await else {
            return Ok(());
        };
        let len = u16::try_from(reply.len()).unwrap_or(u16::MAX);
        stream.write_u16(len).await?;
        stream.write_all(&reply[..usize::from(len)]).await?;
    }
    Ok(())
}

/// Serves DNS on the addresses in `settings` until `stop`, following
/// changes. Addresses that can't be bound yet (the VPN interface is still
/// coming up) are retried.
pub async fn run(mut settings: watch::Receiver<Arc<Settings>>, stop: CancellationToken) {
    let mut bound: BTreeMap<SocketAddr, CancellationToken> = BTreeMap::new();
    loop {
        let wanted = settings.borrow_and_update().listen.clone();
        bound.retain(|addr, token| {
            let keep = wanted.contains(addr);
            if !keep {
                token.cancel();
                info!(%addr, "DNS stopped listening");
            }
            keep
        });
        let mut missing = false;
        for addr in &wanted {
            if bound.contains_key(addr) {
                continue;
            }
            match (UdpSocket::bind(addr).await, TcpListener::bind(addr).await) {
                (Ok(udp), Ok(tcp)) => {
                    let token = stop.child_token();
                    tokio::spawn(serve_udp(udp, settings.clone(), token.clone()));
                    tokio::spawn(serve_tcp(tcp, settings.clone(), token.clone()));
                    bound.insert(*addr, token);
                    info!(%addr, "DNS listening");
                }
                (Err(e), _) | (_, Err(e)) => {
                    missing = true;
                    debug!(%addr, "DNS cannot listen yet: {e}");
                }
            }
        }
        let retry = tokio::time::sleep(if missing {
            Duration::from_secs(2)
        } else {
            Duration::from_secs(3600)
        });
        tokio::select! {
            () = stop.cancelled() => break,
            () = retry => {}
            r = settings.changed() => if r.is_err() { break },
        }
    }
    for token in bound.values() {
        token.cancel();
    }
    if !bound.is_empty() {
        warn!("DNS stopped");
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use hickory_proto::op::Query;
    use hickory_proto::rr::Name;

    fn query(name: &str, ty: RecordType) -> Vec<u8> {
        let mut m = Message::new();
        m.set_id(4242).set_recursion_desired(true);
        m.add_query(Query::query(Name::from_ascii(name).unwrap(), ty));
        m.to_vec().unwrap()
    }

    fn settings(upstream: Vec<SocketAddr>) -> Settings {
        let mut names = BTreeMap::new();
        names.insert(
            "ssh.home.internal".into(),
            vec!["10.88.0.1".parse().unwrap()],
        );
        names.insert("app.example.com".into(), vec!["10.88.0.1".parse().unwrap()]);
        Settings {
            listen: vec![],
            zone: Zone {
                zone: "home.internal".into(),
                names,
            },
            upstream,
        }
    }

    #[test]
    fn reads_resolv_conf() {
        let text =
            "# comment\nnameserver 192.168.1.1\nnameserver ::1\nsearch lan\nnameserver bogus\n";
        assert_eq!(
            parse_resolv_conf(text),
            vec![
                "192.168.1.1:53".parse().unwrap(),
                "[::1]:53".parse().unwrap()
            ]
        );
    }

    #[tokio::test]
    async fn answers_the_zone_and_forwards_the_rest() {
        // A fake upstream that answers every query with one A record.
        let upstream = UdpSocket::bind("127.0.0.1:0").await.unwrap();
        let addr = upstream.local_addr().unwrap();
        tokio::spawn(async move {
            let mut buf = [0u8; 512];
            while let Ok((n, from)) = upstream.recv_from(&mut buf).await {
                let request = Message::from_vec(&buf[..n]).unwrap();
                let mut reply = Message::new();
                reply
                    .set_id(request.id())
                    .set_message_type(MessageType::Response);
                let q = request.queries()[0].clone();
                reply.add_answer(Record::from_rdata(
                    q.name().clone(),
                    60,
                    RData::A(A::new(93, 184, 216, 34)),
                ));
                reply.add_query(q);
                upstream
                    .send_to(&reply.to_vec().unwrap(), from)
                    .await
                    .unwrap();
            }
        });
        let s = settings(vec![addr]);

        let reply = Message::from_vec(
            &answer(&query("SSH.home.internal.", RecordType::A), &s)
                .await
                .unwrap(),
        )
        .unwrap();
        assert_eq!(reply.id(), 4242);
        assert!(reply.authoritative());
        assert_eq!(reply.answers().len(), 1);
        assert_eq!(
            reply.answers()[0].data().ip_addr(),
            Some("10.88.0.1".parse().unwrap())
        );

        // Known name, no IPv6 address: an empty answer, not NXDOMAIN.
        let reply = Message::from_vec(
            &answer(&query("ssh.home.internal.", RecordType::AAAA), &s)
                .await
                .unwrap(),
        )
        .unwrap();
        assert_eq!(reply.response_code(), ResponseCode::NoError);
        assert!(reply.answers().is_empty(), "{:?}", reply.answers());

        let reply = Message::from_vec(
            &answer(&query("nope.home.internal.", RecordType::A), &s)
                .await
                .unwrap(),
        )
        .unwrap();
        assert_eq!(reply.response_code(), ResponseCode::NXDomain);

        // A public domain served by this machine resolves to it inside.
        let reply = Message::from_vec(
            &answer(&query("app.example.com.", RecordType::A), &s)
                .await
                .unwrap(),
        )
        .unwrap();
        assert!(reply.authoritative());

        let reply = Message::from_vec(
            &answer(&query("example.org.", RecordType::A), &s)
                .await
                .unwrap(),
        )
        .unwrap();
        assert!(!reply.authoritative());
        assert_eq!(
            reply.answers()[0].data().ip_addr(),
            Some("93.184.216.34".parse().unwrap())
        );

        // No upstream answers: SERVFAIL rather than silence.
        let dead = UdpSocket::bind("127.0.0.1:0")
            .await
            .unwrap()
            .local_addr()
            .unwrap();
        let reply = Message::from_vec(
            &answer(&query("example.org.", RecordType::A), &settings(vec![dead]))
                .await
                .unwrap(),
        )
        .unwrap();
        assert_eq!(reply.response_code(), ResponseCode::ServFail);

        assert!(answer(&[1], &s).await.is_none());
        let garbage = answer(&[0x12, 0x34, 0xff], &s).await.unwrap();
        assert_eq!(&garbage[..2], &[0x12, 0x34]);
    }

    #[tokio::test]
    async fn serves_udp_and_tcp() {
        let (tx, rx) = watch::channel(Arc::new(Settings {
            listen: vec!["127.0.0.1:0".parse().unwrap()],
            ..settings(vec![])
        }));
        let _ = tx;
        // Port 0 can't be known in advance, so bind the sockets here.
        let udp = UdpSocket::bind("127.0.0.1:0").await.unwrap();
        let udp_addr = udp.local_addr().unwrap();
        let tcp = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let tcp_addr = tcp.local_addr().unwrap();
        let stop = CancellationToken::new();
        tokio::spawn(serve_udp(udp, rx.clone(), stop.clone()));
        tokio::spawn(serve_tcp(tcp, rx, stop.clone()));

        let client = UdpSocket::bind("127.0.0.1:0").await.unwrap();
        client
            .send_to(&query("ssh.home.internal.", RecordType::A), udp_addr)
            .await
            .unwrap();
        let mut buf = [0u8; 512];
        let (n, _) = tokio::time::timeout(Duration::from_secs(2), client.recv_from(&mut buf))
            .await
            .unwrap()
            .unwrap();
        assert_eq!(Message::from_vec(&buf[..n]).unwrap().answers().len(), 1);

        let mut stream = TcpStream::connect(tcp_addr).await.unwrap();
        let q = query("ssh.home.internal.", RecordType::A);
        stream
            .write_u16(u16::try_from(q.len()).unwrap())
            .await
            .unwrap();
        stream.write_all(&q).await.unwrap();
        let len = stream.read_u16().await.unwrap();
        let mut reply = vec![0u8; usize::from(len)];
        stream.read_exact(&mut reply).await.unwrap();
        assert_eq!(Message::from_vec(&reply).unwrap().answers().len(), 1);
        stop.cancel();
    }
}
