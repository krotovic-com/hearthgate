//! The admin API over real HTTP.

use std::sync::{Arc, RwLock};

use bytes::Bytes;
use hearthgate_acme::CertStore;
use hearthgate_api::{ApiState, Net, RuntimeInfo, serve_listener};
use hearthgate_proxy::Gateway;
use hearthgate_store::Store;
use http_body_util::{BodyExt, Full};
use hyper::Request;
use hyper_util::rt::TokioIo;
use serde_json::{Value, json};
use tokio::net::{TcpListener, TcpStream};
use tokio_util::sync::CancellationToken;

const TOKEN: &str = "test-token";

struct Api {
    port: u16,
    stop: CancellationToken,
    changed: Arc<tokio::sync::Notify>,
    _dir: tempfile::TempDir,
}

async fn start(allow: &[&str]) -> Api {
    let dir = tempfile::tempdir().unwrap();
    let changed = Arc::new(tokio::sync::Notify::new());
    let state = ApiState {
        store: Arc::new(Store::open_dir(&dir.path().join("data")).unwrap()),
        gateway: Arc::new(Gateway::new()),
        certs: Arc::new(CertStore::new(dir.path())),
        runtime: Arc::new(RwLock::new(RuntimeInfo::default())),
        changed: Arc::clone(&changed),
        token: TOKEN.into(),
        open_metrics: false,
    };
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let allow: Vec<Net> = allow.iter().map(|n| n.parse().unwrap()).collect();
    let stop = CancellationToken::new();
    tokio::spawn(serve_listener(listener, allow, state, stop.clone()));
    Api {
        port,
        stop,
        changed,
        _dir: dir,
    }
}

async fn call(
    port: u16,
    method: &str,
    path: &str,
    token: Option<&str>,
    body: Option<Value>,
) -> (u16, String) {
    let stream = TcpStream::connect(("127.0.0.1", port)).await.unwrap();
    let (mut send, conn) = hyper::client::conn::http1::handshake(TokioIo::new(stream))
        .await
        .unwrap();
    tokio::spawn(conn);
    let mut req = Request::builder()
        .method(method)
        .uri(path)
        .header("host", "localhost");
    if let Some(t) = token {
        req = req.header("authorization", format!("Bearer {t}"));
    }
    let body = body.map(|b| b.to_string()).unwrap_or_default();
    if !body.is_empty() {
        req = req.header("content-type", "application/json");
    }
    let resp = send
        .send_request(req.body(Full::new(Bytes::from(body))).unwrap())
        .await
        .unwrap();
    let status = resp.status().as_u16();
    let text = resp.into_body().collect().await.unwrap().to_bytes();
    (status, String::from_utf8_lossy(&text).into_owned())
}

fn json_of(text: &str) -> Value {
    serde_json::from_str(text).unwrap_or_else(|e| panic!("{e}: {text}"))
}

fn ssh_state(port: u16) -> Value {
    json!({
        "interfaces": [{ "name": "ssh", "listeners": [{ "port": port, "mode": "raw" }] }],
        "services": [{ "name": "sshd", "kind": "external", "host": "127.0.0.1", "ports": { "ssh": 22 } }],
        "routes": [{ "name": "ssh", "interface": "ssh", "listener": port, "targets": [{ "service": "sshd", "port": "ssh" }] }]
    })
}

#[tokio::test]
async fn needs_the_token() {
    let api = start(&["127.0.0.0/8"]).await;
    assert_eq!(
        call(api.port, "GET", "/api/v1/status", None, None).await.0,
        401
    );
    assert_eq!(
        call(api.port, "GET", "/api/v1/status", Some("wrong"), None)
            .await
            .0,
        401
    );
    assert_eq!(call(api.port, "GET", "/metrics", None, None).await.0, 401);
    let (status, body) = call(api.port, "GET", "/api/v1/openapi.json", None, None).await;
    assert_eq!(status, 200);
    let doc = json_of(&body);
    assert!(doc["paths"]["/api/v1/state"]["put"].is_object());
    assert!(doc["components"]["schemas"]["DesiredState"].is_object());
    api.stop.cancel();
}

#[tokio::test]
async fn refuses_clients_outside_the_allowed_ranges() {
    let api = start(&["192.168.0.0/16"]).await;
    let stream = TcpStream::connect(("127.0.0.1", api.port)).await.unwrap();
    let handshake =
        hyper::client::conn::http1::handshake::<_, Full<Bytes>>(TokioIo::new(stream)).await;
    let (mut send, conn) = handshake.unwrap();
    tokio::spawn(conn);
    let result = send
        .send_request(
            Request::get("/api/v1/openapi.json")
                .header("host", "x")
                .body(Full::new(Bytes::new()))
                .unwrap(),
        )
        .await;
    assert!(
        result.is_err(),
        "the connection must be closed without a response"
    );
    api.stop.cancel();
}

#[tokio::test]
async fn state_revisions_and_rollback() {
    let api = start(&["127.0.0.0/8"]).await;
    let t = Some(TOKEN);
    let (status, body) = call(api.port, "GET", "/api/v1/state", t, None).await;
    assert_eq!(status, 200);
    assert!(json_of(&body)["revision"].is_null());

    let notified = api.changed.notified();
    let (status, body) = call(
        api.port,
        "PUT",
        "/api/v1/state",
        t,
        Some(json!({ "state": ssh_state(2222), "message": "first" })),
    )
    .await;
    assert_eq!(status, 200, "{body}");
    let r = json_of(&body);
    assert_eq!(
        (r["changed"].as_bool(), r["revision"]["id"].as_i64()),
        (Some(true), Some(1))
    );
    tokio::time::timeout(std::time::Duration::from_secs(1), notified)
        .await
        .expect("daemon is poked");

    let (_, body) = call(
        api.port,
        "PUT",
        "/api/v1/state",
        t,
        Some(json!({ "state": ssh_state(2222) })),
    )
    .await;
    assert_eq!(json_of(&body)["changed"], false);
    call(
        api.port,
        "PUT",
        "/api/v1/state",
        t,
        Some(json!({ "state": ssh_state(2200) })),
    )
    .await;

    let mut bad = ssh_state(2222);
    bad["routes"][0]["targets"][0]["service"] = json!("nope");
    let (status, body) = call(
        api.port,
        "PUT",
        "/api/v1/state",
        t,
        Some(json!({ "state": bad.clone() })),
    )
    .await;
    assert_eq!(status, 422);
    assert!(body.contains("service nope does not exist"), "{body}");
    let (status, body) = call(api.port, "POST", "/api/v1/state/validate", t, Some(bad)).await;
    assert_eq!(status, 200);
    assert_eq!(json_of(&body)["valid"], false);

    let (_, body) = call(api.port, "GET", "/api/v1/revisions", t, None).await;
    let ids: Vec<i64> = json_of(&body)
        .as_array()
        .unwrap()
        .iter()
        .map(|r| r["id"].as_i64().unwrap())
        .collect();
    assert_eq!(ids, [2, 1]);

    let (status, body) = call(api.port, "POST", "/api/v1/revisions/1/rollback", t, None).await;
    assert_eq!(status, 200, "{body}");
    assert_eq!(json_of(&body)["revision"]["rollback_of"], 1);
    let (_, body) = call(api.port, "GET", "/api/v1/revisions/3", t, None).await;
    assert_eq!(
        json_of(&body)["state"]["interfaces"][0]["listeners"][0]["port"],
        2222
    );
    assert_eq!(
        call(api.port, "GET", "/api/v1/revisions/99", t, None)
            .await
            .0,
        404
    );

    let (status, body) = call(api.port, "GET", "/api/v1/status", t, None).await;
    assert_eq!(status, 200);
    assert_eq!(json_of(&body)["latest_revision"], 3);
    let (status, body) = call(api.port, "GET", "/metrics", t, None).await;
    assert_eq!(status, 200);
    assert!(body.contains("# TYPE hearthgate_revision gauge"));
    api.stop.cancel();
}
