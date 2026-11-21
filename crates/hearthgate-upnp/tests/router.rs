//! Port mapping lifecycle against fake routers on localhost.

use std::convert::Infallible;
use std::net::SocketAddr;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use bytes::Bytes;
use hearthgate_upnp::{Options, Protocol, Router, Wanted, run};
use http_body_util::{BodyExt, Full};
use hyper::body::Incoming;
use hyper_util::rt::TokioIo;
use tokio::net::{TcpListener, UdpSocket};
use tokio::sync::watch;
use tokio_util::sync::CancellationToken;

type Log = Arc<Mutex<Vec<String>>>;

/// A UPnP router: answers SSDP searches and SOAP calls, logging each
/// action as `Action PROTO PORT`.
async fn fake_igd(public_ip: &'static str) -> (SocketAddr, Log) {
    let log: Log = Arc::default();
    let http = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let http_port = http.local_addr().unwrap().port();
    let ssdp = UdpSocket::bind("127.0.0.1:0").await.unwrap();
    let ssdp_addr = ssdp.local_addr().unwrap();
    tokio::spawn(async move {
        let mut buf = [0u8; 1024];
        loop {
            let Ok((n, from)) = ssdp.recv_from(&mut buf).await else {
                return;
            };
            if String::from_utf8_lossy(&buf[..n]).starts_with("M-SEARCH") {
                let reply = format!(
                    "HTTP/1.1 200 OK\r\nST: urn:schemas-upnp-org:device:InternetGatewayDevice:1\r\n\
                     LOCATION: http://127.0.0.1:{http_port}/rootDesc.xml\r\n\r\n"
                );
                let _ = ssdp.send_to(reply.as_bytes(), from).await;
            }
        }
    });
    let log2 = Arc::clone(&log);
    tokio::spawn(async move {
        loop {
            let Ok((stream, _)) = http.accept().await else {
                return;
            };
            let log = Arc::clone(&log2);
            tokio::spawn(async move {
                let service = hyper::service::service_fn(move |req: hyper::Request<Incoming>| {
                    let log = Arc::clone(&log);
                    async move {
                        let path = req.uri().path().to_owned();
                        let action = req
                            .headers()
                            .get("soapaction")
                            .and_then(|v| v.to_str().ok())
                            .and_then(|v| v.trim_matches('"').split('#').nth(1))
                            .unwrap_or("")
                            .to_owned();
                        let body = req.into_body().collect().await.unwrap().to_bytes();
                        let body = String::from_utf8_lossy(&body).into_owned();
                        let field = |name: &str| {
                            body.split(&format!("<{name}>"))
                                .nth(1)
                                .and_then(|r| r.split('<').next())
                                .unwrap_or("")
                                .to_owned()
                        };
                        let xml = if path == "/rootDesc.xml" {
                            r#"<?xml version="1.0"?><root xmlns="urn:schemas-upnp-org:device-1-0"><device>
                            <friendlyName>Fake Router</friendlyName><deviceList><device><deviceList><device>
                            <serviceList><service>
                            <serviceType>urn:schemas-upnp-org:service:WANIPConnection:1</serviceType>
                            <controlURL>/ctl/IPConn</controlURL></service></serviceList>
                            </device></deviceList></device></deviceList></device></root>"#
                                .to_owned()
                        } else {
                            log.lock().unwrap().push(format!(
                                "{action} {} {}",
                                field("NewProtocol"),
                                field("NewExternalPort")
                            ));
                            format!(
                                "<s:Envelope><s:Body><u:{action}Response>\
                                 <NewExternalIPAddress>{public_ip}</NewExternalIPAddress>\
                                 </u:{action}Response></s:Body></s:Envelope>"
                            )
                        };
                        Ok::<_, Infallible>(hyper::Response::new(Full::new(Bytes::from(xml))))
                    }
                });
                let _ = hyper::server::conn::http1::Builder::new()
                    .serve_connection(TokioIo::new(stream), service)
                    .await;
            });
        }
    });
    (ssdp_addr, log)
}

/// A NAT-PMP router granting every request.
async fn fake_natpmp() -> (SocketAddr, Log) {
    let log: Log = Arc::default();
    let socket = UdpSocket::bind("127.0.0.1:0").await.unwrap();
    let addr = socket.local_addr().unwrap();
    let log2 = Arc::clone(&log);
    tokio::spawn(async move {
        let mut buf = [0u8; 64];
        loop {
            let Ok((n, from)) = socket.recv_from(&mut buf).await else {
                return;
            };
            let op = buf[1];
            let mut reply = vec![0, 128 + op, 0, 0, 0, 0, 0, 1];
            if op == 0 {
                reply.extend_from_slice(&[198, 51, 100, 7]);
            } else if n >= 12 {
                let port = u16::from_be_bytes([buf[4], buf[5]]);
                let life = u32::from_be_bytes([buf[8], buf[9], buf[10], buf[11]]);
                log2.lock().unwrap().push(format!(
                    "{} {port} {life}",
                    if op == 1 { "UDP" } else { "TCP" }
                ));
                reply.extend_from_slice(&buf[4..6]);
                reply.extend_from_slice(&buf[6..8]);
                reply.extend_from_slice(&buf[8..12]);
            }
            let _ = socket.send_to(&reply, from).await;
        }
    });
    (addr, log)
}

async fn wait_for(log: &Log, entry: &str) {
    for _ in 0..100 {
        if log.lock().unwrap().iter().any(|l| l == entry) {
            return;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    panic!("{entry} never happened: {:?}", log.lock().unwrap());
}

#[tokio::test]
async fn upnp_mappings_follow_the_public_ports() {
    let (ssdp, log) = fake_igd("198.51.100.4").await;
    let router = Arc::new(Router::default());
    let (tx, rx) = watch::channel(Wanted {
        enabled: true,
        ports: vec![(443, Protocol::Tcp), (51820, Protocol::Udp)],
    });
    let stop = CancellationToken::new();
    let task = tokio::spawn(run(
        Arc::clone(&router),
        rx,
        Options {
            ssdp,
            natpmp: None,
            search_wait: Duration::from_secs(2),
        },
        stop.clone(),
    ));
    wait_for(&log, "AddPortMapping TCP 443").await;
    wait_for(&log, "AddPortMapping UDP 51820").await;
    let status = router.status();
    assert_eq!(status.method, Some("upnp"));
    assert_eq!(status.public_ip, "198.51.100.4".parse().ok());
    assert!(!status.double_nat);
    assert!(status.router.unwrap().starts_with("Fake Router"));
    assert!(status.mappings.iter().all(|m| m.active));

    // Deleting an interface closes its port on the router.
    tx.send_replace(Wanted {
        enabled: true,
        ports: vec![(443, Protocol::Tcp)],
    });
    wait_for(&log, "DeletePortMapping UDP 51820").await;

    // Stopping Hearthgate removes the rest.
    stop.cancel();
    task.await.unwrap();
    wait_for(&log, "DeletePortMapping TCP 443").await;
}

#[tokio::test]
async fn nat_pmp_when_there_is_no_upnp_and_double_nat_is_detected() {
    // Nothing answers SSDP on this port.
    let silent = UdpSocket::bind("127.0.0.1:0").await.unwrap();
    let ssdp = silent.local_addr().unwrap();
    let (natpmp, log) = fake_natpmp().await;
    let router = Arc::new(Router::default());
    let (tx, rx) = watch::channel(Wanted {
        enabled: true,
        ports: vec![(8443, Protocol::Tcp)],
    });
    let stop = CancellationToken::new();
    let task = tokio::spawn(run(
        Arc::clone(&router),
        rx,
        Options {
            ssdp,
            natpmp: Some(natpmp),
            search_wait: Duration::from_millis(300),
        },
        stop.clone(),
    ));
    wait_for(&log, "TCP 8443 3600").await;
    assert_eq!(router.status().method, Some("nat-pmp"));
    // Turning it off removes the mapping (lifetime 0).
    tx.send_replace(Wanted::default());
    wait_for(&log, "TCP 8443 0").await;
    stop.cancel();
    task.await.unwrap();

    // A router whose public address is private sits behind another NAT.
    let (ssdp, _log) = fake_igd("100.64.12.1").await;
    let router = Arc::new(Router::default());
    let (_tx, rx) = watch::channel(Wanted {
        enabled: true,
        ports: vec![(443, Protocol::Tcp)],
    });
    let stop = CancellationToken::new();
    let task = tokio::spawn(run(
        Arc::clone(&router),
        rx,
        Options {
            ssdp,
            natpmp: None,
            search_wait: Duration::from_secs(2),
        },
        stop.clone(),
    ));
    for _ in 0..100 {
        if router.status().public_ip.is_some() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    assert!(router.status().double_nat);
    stop.cancel();
    task.await.unwrap();
}
