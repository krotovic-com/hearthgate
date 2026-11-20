//! Acceptance: stopping an upstream takes it out of rotation with no failed
//! requests, and starting it again brings it back.

use std::convert::Infallible;
use std::time::Duration;

use bytes::Bytes;
use hearthgate_model::DesiredState;
use hearthgate_proxy::{Gateway, NoContainers, compile};
use hearthgate_services::{HealthChecker, health};
use http_body_util::{BodyExt, Empty, Full};
use hyper::{Request, Response};
use hyper_util::rt::TokioIo;
use tokio::net::{TcpListener, TcpStream};
use tokio::task::JoinHandle;

async fn serve(port: u16, tag: &'static str) -> JoinHandle<()> {
    let listener = TcpListener::bind(("127.0.0.1", port)).await.unwrap();
    tokio::spawn(async move {
        loop {
            let (s, _) = listener.accept().await.unwrap();
            tokio::spawn(async move {
                let svc = hyper::service::service_fn(
                    move |_req: Request<hyper::body::Incoming>| async move {
                        Ok::<_, Infallible>(Response::new(Full::new(Bytes::from(tag))))
                    },
                );
                let _ = hyper::server::conn::http1::Builder::new()
                    .keep_alive(false)
                    .serve_connection(TokioIo::new(s), svc)
                    .await;
            });
        }
    })
}

fn free_port() -> u16 {
    std::net::TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
        .port()
}

async fn get(port: u16) -> (u16, String) {
    let stream = TcpStream::connect(("127.0.0.1", port)).await.unwrap();
    let (mut send, conn) = hyper::client::conn::http1::handshake(TokioIo::new(stream))
        .await
        .unwrap();
    tokio::spawn(conn);
    let resp = send
        .send_request(
            Request::get("/")
                .header("host", "app.example.com")
                .body(Empty::<Bytes>::new())
                .unwrap(),
        )
        .await
        .unwrap();
    let status = resp.status().as_u16();
    let body = resp.into_body().collect().await.unwrap().to_bytes();
    (status, String::from_utf8_lossy(&body).into_owned())
}

#[tokio::test]
async fn health_checks_drive_rotation() {
    let (pa, pb, listen) = (free_port(), free_port(), free_port());
    let a = serve(pa, "a").await;
    let _b = serve(pb, "b").await;
    let state = DesiredState::from_toml(&format!(
        r#"
[[interfaces]]
name = "web"
bind = ["127.0.0.1"]
domains = ["app.example.com"]
listeners = [{{ port = {listen}, mode = "http" }}]
[[services]]
name = "a"
kind = "external"
host = "127.0.0.1"
ports = {{ http = {pa} }}
health = {{ type = "http", interval = "100ms", timeout = "100ms", unhealthy_threshold = 2, healthy_threshold = 2 }}
[[services]]
name = "b"
kind = "external"
host = "127.0.0.1"
ports = {{ http = {pb} }}
health = {{ type = "http", interval = "100ms", timeout = "100ms", unhealthy_threshold = 2, healthy_threshold = 2 }}
[[routes]]
name = "app"
interface = "web"
listener = {listen}
targets = [{{ service = "a", port = "http" }}, {{ service = "b", port = "http" }}]
"#
    ))
    .unwrap();
    let gw = Gateway::new();
    let plan = compile(&state, gw.upstreams(), &NoContainers);
    assert!(gw.apply(&plan).await.is_empty());
    let mut checker = HealthChecker::default();
    checker.update(health::jobs(&state, gw.upstreams()), gw.upstreams());

    // Both upstreams share the traffic.
    let mut seen = Vec::new();
    for _ in 0..4 {
        seen.push(get(listen).await.1);
    }
    seen.sort();
    assert_eq!(seen, ["a", "a", "b", "b"]);

    // Stop a. Requests never fail: connect errors retry on b at once, and
    // the health check takes a out of rotation.
    a.abort();
    let _ = a.await;
    for _ in 0..20 {
        assert_eq!(get(listen).await, (200, "b".to_owned()));
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
    let ua = gw
        .upstreams()
        .all()
        .into_iter()
        .find(|u| u.service == "a")
        .unwrap();
    assert!(!ua.healthy(), "a is marked unhealthy");

    // Start a again: it comes back after healthy_threshold checks.
    let _a = serve(pa, "a").await;
    for _ in 0..50 {
        if ua.healthy() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    assert!(ua.healthy(), "a is healthy again");
    let mut seen = Vec::new();
    for _ in 0..4 {
        seen.push(get(listen).await.1);
    }
    assert!(seen.contains(&"a".to_owned()), "{seen:?}");
    checker.stop();
    gw.shutdown(Duration::from_millis(100)).await;
}
