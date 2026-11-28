//! The admin API and dashboard login over real HTTP(S).

use std::net::SocketAddr;
use std::sync::{Arc, RwLock};

use bytes::Bytes;
use hearthgate_acme::CertStore;
use hearthgate_api::access::{Resolved, Source, TlsMode};
use hearthgate_api::{AccessControl, AdminTls, ApiState, Net, RuntimeInfo, serve_listener};
use hearthgate_proxy::Gateway;
use hearthgate_store::Store;
use http_body_util::{BodyExt, Full};
use hyper::Request;
use hyper::header::HeaderMap;
use hyper_util::rt::TokioIo;
use rustls::pki_types::{CertificateDer, pem::PemObject};
use serde_json::{Value, json};
use tokio::io::{AsyncRead, AsyncWrite};
use tokio::net::{TcpListener, TcpStream};
use tokio_util::sync::CancellationToken;

const TOKEN: &str = "test-token";
const PASSWORD: &str = "correct horse battery";

struct Api {
    port: u16,
    stop: CancellationToken,
    changed: Arc<tokio::sync::Notify>,
    store: Arc<Store>,
    tls: Option<Arc<AdminTls>>,
    perimeter: Arc<hearthgate_firewall::Perimeter>,
    dir: tempfile::TempDir,
}

struct Options {
    bind: &'static str,
    allow: &'static [&'static str],
    tls: bool,
}

const LOCAL: Options = Options {
    bind: "127.0.0.1:0",
    allow: &["127.0.0.0/8"],
    tls: false,
};

async fn start(options: Options) -> Api {
    let dir = tempfile::tempdir().unwrap();
    let perimeter = Arc::new(hearthgate_firewall::Perimeter::new(
        hearthgate_firewall::Nft::new(vec!["false".into()]),
    ));
    let changed = Arc::new(tokio::sync::Notify::new());
    let store = Arc::new(Store::open_dir(&dir.path().join("data")).unwrap());
    let tls = options.tls.then(|| {
        let names = ["localhost".to_owned()].into();
        Arc::new(hearthgate_api::tls::load_or_create(dir.path(), &names).unwrap())
    });
    let state = ApiState {
        store: Arc::clone(&store),
        gateway: Arc::new(Gateway::new()),
        certs: Arc::new(CertStore::new(dir.path())),
        runtime: Arc::new(RwLock::new(RuntimeInfo::default())),
        changed: Arc::clone(&changed),
        token: TOKEN.into(),
        open_metrics: false,
        secure_cookies: options.tls,
        auth: Arc::default(),
        access: Arc::new(AccessControl::new(
            Source::Dashboard,
            TlsMode::Off,
            CancellationToken::new(),
        )),
        live: Arc::default(),
        perimeter: Arc::clone(&perimeter),
        router: Arc::default(),
        vpn: Arc::new(hearthgate_vpn::Runtime::new(
            hearthgate_vpn::Device::new("hgtest-none", hearthgate_vpn::Tools::default()),
            Some(dir.path().join("vpn")),
        )),
        docker: Arc::default(),
        ai: Arc::new(hearthgate_api::ai::Assistant::new(dir.path().join("data"))),
        updates: test_updater(dir.path()),
    };
    let listener = TcpListener::bind(options.bind).await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let allow: Vec<Net> = options.allow.iter().map(|n| n.parse().unwrap()).collect();
    let stop = CancellationToken::new();
    tokio::spawn(serve_listener(
        listener,
        allow,
        tls.clone(),
        state,
        stop.clone(),
    ));
    Api {
        port,
        stop,
        changed,
        store,
        tls,
        perimeter,
        dir,
    }
}

struct Response {
    status: u16,
    headers: HeaderMap,
    body: String,
}

impl Response {
    fn json(&self) -> Value {
        serde_json::from_str(&self.body).unwrap_or_else(|e| panic!("{e}: {}", self.body))
    }

    /// The session cookie (`name=value`) this response sets.
    fn cookie(&self) -> String {
        let set = self.headers["set-cookie"].to_str().unwrap();
        set.split(';').next().unwrap().to_owned()
    }
}

async fn send<I>(
    io: I,
    method: &str,
    path: &str,
    headers: &[(&str, &str)],
    body: Option<Value>,
) -> Response
where
    I: AsyncRead + AsyncWrite + Unpin + Send + 'static,
{
    let (mut sender, conn) = hyper::client::conn::http1::handshake(TokioIo::new(io))
        .await
        .unwrap();
    tokio::spawn(conn);
    let mut req = Request::builder()
        .method(method)
        .uri(path)
        .header("host", "localhost");
    for (k, v) in headers {
        req = req.header(*k, *v);
    }
    let body = body.map(|b| b.to_string()).unwrap_or_default();
    if !body.is_empty() {
        req = req.header("content-type", "application/json");
    }
    let resp = sender
        .send_request(req.body(Full::new(Bytes::from(body))).unwrap())
        .await
        .unwrap();
    let status = resp.status().as_u16();
    let headers = resp.headers().clone();
    let text = resp.into_body().collect().await.unwrap().to_bytes();
    Response {
        status,
        headers,
        body: String::from_utf8_lossy(&text).into_owned(),
    }
}

async fn request(
    port: u16,
    method: &str,
    path: &str,
    headers: &[(&str, &str)],
    body: Option<Value>,
) -> Response {
    let stream = TcpStream::connect(("127.0.0.1", port)).await.unwrap();
    send(stream, method, path, headers, body).await
}

async fn call(
    port: u16,
    method: &str,
    path: &str,
    token: Option<&str>,
    body: Option<Value>,
) -> (u16, String) {
    let auth = token.map(|t| format!("Bearer {t}"));
    let headers: Vec<(&str, &str)> = auth.iter().map(|a| ("authorization", a.as_str())).collect();
    let r = request(port, method, path, &headers, body).await;
    (r.status, r.body)
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

/// Creates the first account and returns (cookie, csrf).
async fn set_up(api: &Api) -> (String, String) {
    let code = hearthgate_api::setup_code(&api.store).unwrap().unwrap();
    let r = request(
        api.port,
        "POST",
        "/api/v1/auth/setup",
        &[],
        Some(json!({ "setup_code": code, "username": "admin", "password": PASSWORD })),
    )
    .await;
    assert_eq!(r.status, 200, "{}", r.body);
    (r.cookie(), r.json()["csrf"].as_str().unwrap().to_owned())
}

#[tokio::test]
async fn needs_a_token_or_session() {
    let api = start(LOCAL).await;
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
    assert_eq!(
        call(api.port, "GET", "/api/v1/status", Some("hg_nope"), None)
            .await
            .0,
        401
    );
    assert_eq!(call(api.port, "GET", "/metrics", None, None).await.0, 401);
    let (status, body) = call(api.port, "GET", "/api/v1/openapi.json", None, None).await;
    assert_eq!(status, 200);
    let doc = json_of(&body);
    assert!(doc["paths"]["/api/v1/state"]["put"].is_object());
    assert!(doc["paths"]["/api/v1/auth/login"]["post"].is_object());
    assert!(doc["components"]["schemas"]["DesiredState"].is_object());
    // Unknown API paths are JSON 404s, not the dashboard.
    let (status, body) = call(api.port, "GET", "/api/v1/nope", Some(TOKEN), None).await;
    assert_eq!(status, 404);
    assert!(body.contains("no such API endpoint"));
    api.stop.cancel();
}

async fn assert_dropped(port: u16) {
    let stream = TcpStream::connect(("127.0.0.1", port)).await.unwrap();
    let handshake =
        hyper::client::conn::http1::handshake::<_, Full<Bytes>>(TokioIo::new(stream)).await;
    let (mut sender, conn) = handshake.unwrap();
    tokio::spawn(conn);
    let result = sender
        .send_request(
            Request::get("/")
                .header("host", "x")
                .body(Full::new(Bytes::new()))
                .unwrap(),
        )
        .await;
    assert!(
        result.is_err(),
        "the connection must be closed without a response"
    );
}

#[tokio::test]
async fn refuses_clients_outside_the_allowed_ranges() {
    let api = start(Options {
        allow: &["192.168.0.0/16"],
        ..LOCAL
    })
    .await;
    assert_dropped(api.port).await;
    api.stop.cancel();
}

#[tokio::test]
async fn refuses_outsiders_even_when_bound_to_every_address() {
    // A misconfigured `listen = ["0.0.0.0:…"]` must not expose the
    // dashboard: the allowlist still decides, before anything is read.
    let api = start(Options {
        bind: "0.0.0.0:0",
        allow: &["10.0.0.0/8"],
        tls: true,
    })
    .await;
    assert_dropped(api.port).await;
    api.stop.cancel();
}

#[tokio::test]
async fn state_revisions_and_rollback() {
    let api = start(LOCAL).await;
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
    assert_eq!(r["revision"]["author"], "api:admin-token");
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

#[tokio::test]
async fn first_run_setup_login_and_csrf() {
    let api = start(LOCAL).await;
    let session = request(api.port, "GET", "/api/v1/auth/session", &[], None).await;
    assert_eq!(session.json()["setup_required"], true);

    // Setup needs the code from the server.
    let wrong = request(
        api.port,
        "POST",
        "/api/v1/auth/setup",
        &[],
        Some(json!({ "setup_code": "AAAA-BBBB-CCCC", "username": "eve", "password": PASSWORD })),
    )
    .await;
    assert_eq!(wrong.status, 403, "{}", wrong.body);
    let short = request(
        api.port,
        "POST",
        "/api/v1/auth/setup",
        &[],
        Some(json!({ "setup_code": "x", "username": "admin", "password": "short" })),
    )
    .await;
    assert_eq!(short.status, 422);

    let (cookie, csrf) = set_up(&api).await;
    assert!(cookie.starts_with("hg_session="));
    assert_eq!(hearthgate_api::setup_code(&api.store).unwrap(), None);
    let again = request(
        api.port,
        "POST",
        "/api/v1/auth/setup",
        &[],
        Some(json!({ "setup_code": "x", "username": "eve", "password": PASSWORD })),
    )
    .await;
    assert_eq!(again.status, 409);

    let me = request(
        api.port,
        "GET",
        "/api/v1/auth/session",
        &[("cookie", &cookie)],
        None,
    )
    .await;
    assert_eq!(me.json()["user"]["username"], "admin");
    assert_eq!(me.json()["csrf"], csrf.as_str());

    // Reads work with the cookie alone; changes also need the CSRF token.
    let status = request(
        api.port,
        "GET",
        "/api/v1/status",
        &[("cookie", &cookie)],
        None,
    )
    .await;
    assert_eq!(status.status, 200);
    let body = json!({ "state": ssh_state(2222) });
    let no_csrf = request(
        api.port,
        "PUT",
        "/api/v1/state",
        &[("cookie", &cookie)],
        Some(body.clone()),
    )
    .await;
    assert_eq!(no_csrf.status, 403);
    let bad_csrf = request(
        api.port,
        "PUT",
        "/api/v1/state",
        &[("cookie", &cookie), ("x-csrf-token", "nope")],
        Some(body.clone()),
    )
    .await;
    assert_eq!(bad_csrf.status, 403);
    let ok = request(
        api.port,
        "PUT",
        "/api/v1/state",
        &[("cookie", &cookie), ("x-csrf-token", &csrf)],
        Some(body),
    )
    .await;
    assert_eq!(ok.status, 200, "{}", ok.body);
    assert_eq!(ok.json()["revision"]["author"], "dashboard:admin");

    // Logout ends the session.
    let out = request(
        api.port,
        "POST",
        "/api/v1/auth/logout",
        &[("cookie", &cookie), ("x-csrf-token", &csrf)],
        None,
    )
    .await;
    assert_eq!(out.status, 204);
    assert!(
        out.headers["set-cookie"]
            .to_str()
            .unwrap()
            .contains("Max-Age=0")
    );
    let after = request(
        api.port,
        "GET",
        "/api/v1/status",
        &[("cookie", &cookie)],
        None,
    )
    .await;
    assert_eq!(after.status, 401);

    let bad = request(
        api.port,
        "POST",
        "/api/v1/auth/login",
        &[],
        Some(json!({ "username": "admin", "password": "wrong password!" })),
    )
    .await;
    assert_eq!(bad.status, 401);
    assert_eq!(bad.json()["totp_required"], false);
    let good = request(
        api.port,
        "POST",
        "/api/v1/auth/login",
        &[],
        Some(json!({ "username": "admin", "password": PASSWORD })),
    )
    .await;
    assert_eq!(good.status, 200, "{}", good.body);
    let me = request(
        api.port,
        "GET",
        "/api/v1/auth/session",
        &[("cookie", &good.cookie())],
        None,
    )
    .await;
    assert_eq!(me.json()["user"]["username"], "admin");
    api.stop.cancel();
}

#[tokio::test]
async fn login_attempts_are_limited() {
    let api = start(LOCAL).await;
    set_up(&api).await;
    let attempt = |password: &'static str| {
        request(
            api.port,
            "POST",
            "/api/v1/auth/login",
            &[],
            Some(json!({ "username": "admin", "password": password })),
        )
    };
    for _ in 0..10 {
        assert_eq!(attempt("wrong password!").await.status, 401);
    }
    // Even the right password is refused until the window passes.
    let blocked = attempt(PASSWORD).await;
    assert_eq!(blocked.status, 429, "{}", blocked.body);
    assert!(blocked.body.contains("try again in"));
    api.stop.cancel();
}

fn totp_now(secret: &str) -> String {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs();
    let step = i64::try_from(now).unwrap() / hearthgate_api::testing::TOTP_STEP;
    let key = hearthgate_api::testing::unbase32(secret).unwrap();
    format!("{:06}", hearthgate_api::testing::totp_code(&key, step))
}

#[tokio::test]
async fn two_factor_login_and_password_change() {
    let api = start(LOCAL).await;
    let (cookie, csrf) = set_up(&api).await;
    let auth = [("cookie", cookie.as_str()), ("x-csrf-token", csrf.as_str())];

    let begin = request(api.port, "POST", "/api/v1/auth/totp/begin", &auth, None).await;
    assert_eq!(begin.status, 200, "{}", begin.body);
    let secret = begin.json()["secret"].as_str().unwrap().to_owned();
    assert!(
        begin.json()["uri"]
            .as_str()
            .unwrap()
            .starts_with("otpauth://totp/Hearthgate:admin?")
    );
    let wrong = request(
        api.port,
        "POST",
        "/api/v1/auth/totp/enable",
        &auth,
        Some(json!({ "secret": secret, "code": "000000" })),
    )
    .await;
    // (One in a million chance that 000000 is the current code.)
    assert_eq!(wrong.status, 422);
    let first_code = totp_now(&secret);
    let enable = request(
        api.port,
        "POST",
        "/api/v1/auth/totp/enable",
        &auth,
        Some(json!({ "secret": secret, "code": first_code })),
    )
    .await;
    assert_eq!(enable.status, 204, "{}", enable.body);

    let login = |code: Option<String>| {
        request(
            api.port,
            "POST",
            "/api/v1/auth/login",
            &[],
            Some(json!({ "username": "admin", "password": PASSWORD, "code": code })),
        )
    };
    let needs_code = login(None).await;
    assert_eq!(needs_code.status, 401);
    assert_eq!(needs_code.json()["totp_required"], true);
    // The code used to turn it on cannot be replayed, even in a later step.
    assert_eq!(login(Some(first_code)).await.status, 401);

    // Changing the password ends other sessions but keeps this one.
    let other = {
        // Wait for the next TOTP step so a fresh code exists.
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs();
        let wait = 30 - now % 30;
        tokio::time::sleep(std::time::Duration::from_secs(wait)).await;
        let r = login(Some(totp_now(&secret))).await;
        assert_eq!(r.status, 200, "{}", r.body);
        r.cookie()
    };
    let changed = request(
        api.port,
        "POST",
        "/api/v1/auth/password",
        &auth,
        Some(json!({ "current_password": PASSWORD, "new_password": "a whole new password" })),
    )
    .await;
    assert_eq!(changed.status, 204, "{}", changed.body);
    let mine = request(
        api.port,
        "GET",
        "/api/v1/status",
        &[("cookie", &cookie)],
        None,
    )
    .await;
    assert_eq!(mine.status, 200);
    let theirs = request(
        api.port,
        "GET",
        "/api/v1/status",
        &[("cookie", &other)],
        None,
    )
    .await;
    assert_eq!(theirs.status, 401);
    api.stop.cancel();
}

#[tokio::test]
async fn api_tokens_for_scripts() {
    let api = start(LOCAL).await;
    let (cookie, csrf) = set_up(&api).await;
    let auth = [("cookie", cookie.as_str()), ("x-csrf-token", csrf.as_str())];
    let created = request(
        api.port,
        "POST",
        "/api/v1/tokens",
        &auth,
        Some(json!({ "name": "backup" })),
    )
    .await;
    assert_eq!(created.status, 200, "{}", created.body);
    let token = created.json()["token"].as_str().unwrap().to_owned();
    let id = created.json()["id"].as_i64().unwrap();
    assert!(token.starts_with("hg_"));

    // A bearer token needs no CSRF token.
    let (status, body) = call(
        api.port,
        "PUT",
        "/api/v1/state",
        Some(&token),
        Some(json!({ "state": ssh_state(2222) })),
    )
    .await;
    assert_eq!(status, 200, "{body}");
    assert_eq!(
        json_of(&body)["revision"]["author"],
        format!("api:token-{id}")
    );
    let list = request(api.port, "GET", "/api/v1/tokens", &auth, None).await;
    assert!(list.json()[0]["last_used_at"].is_i64());
    assert!(!list.body.contains(&token), "the secret is never listed");

    let deleted = request(
        api.port,
        "DELETE",
        &format!("/api/v1/tokens/{id}"),
        &auth,
        None,
    )
    .await;
    assert_eq!(deleted.status, 204);
    assert_eq!(
        call(api.port, "GET", "/api/v1/status", Some(&token), None)
            .await
            .0,
        401
    );
    api.stop.cancel();
}

#[tokio::test]
async fn dashboard_is_served_with_security_headers() {
    let api = start(LOCAL).await;
    for path in ["/", "/services/web"] {
        let r = request(api.port, "GET", path, &[], None).await;
        // 200 with a built dashboard, 503 with the "not included" page.
        assert!(matches!(r.status, 200 | 503), "{path}: {}", r.status);
        assert!(r.body.contains("<title>Hearthgate"), "{path}: {}", r.body);
        let csp = r.headers["content-security-policy"].to_str().unwrap();
        assert!(csp.contains("frame-ancestors 'none'"));
        assert_eq!(r.headers["x-content-type-options"], "nosniff");
    }
    // A missing file is a 404 (or the "not included" page), never index.html.
    let missing = request(api.port, "GET", "/missing.js", &[], None).await;
    assert!(matches!(missing.status, 404 | 503), "{}", missing.status);
    assert_eq!(request(api.port, "POST", "/", &[], None).await.status, 405);
    api.stop.cancel();
}

#[tokio::test]
async fn https_with_redirect_from_plain_http() {
    let api = start(Options { tls: true, ..LOCAL }).await;
    // Plain HTTP on the same port is redirected.
    let plain = request(api.port, "GET", "/services?x=1", &[], None).await;
    assert_eq!(plain.status, 308);
    assert_eq!(plain.headers["location"], "https://localhost/services?x=1");

    // TLS works with the generated certificate.
    let pem = std::fs::read(api.dir.path().join("admin-tls/cert.pem")).unwrap();
    let mut roots = rustls::RootCertStore::empty();
    for cert in CertificateDer::pem_slice_iter(&pem) {
        roots.add(cert.unwrap()).unwrap();
    }
    let config = rustls::ClientConfig::builder_with_provider(Arc::new(
        rustls::crypto::ring::default_provider(),
    ))
    .with_safe_default_protocol_versions()
    .unwrap()
    .with_root_certificates(roots)
    .with_no_client_auth();
    let connector = tokio_rustls::TlsConnector::from(Arc::new(config));
    let addr: SocketAddr = ([127, 0, 0, 1], api.port).into();
    let tcp = TcpStream::connect(addr).await.unwrap();
    let tls = connector
        .connect("localhost".try_into().unwrap(), tcp)
        .await
        .unwrap();
    let r = send(tls, "GET", "/api/v1/auth/session", &[], None).await;
    assert_eq!(r.status, 200);
    assert_eq!(r.json()["setup_required"], true);
    assert!(api.tls.as_ref().unwrap().fingerprint.contains(':'));

    // Session cookies are `Secure` and `__Host-` prefixed over HTTPS.
    let code = hearthgate_api::setup_code(&api.store).unwrap().unwrap();
    let tcp = TcpStream::connect(addr).await.unwrap();
    let tls = connector
        .connect("localhost".try_into().unwrap(), tcp)
        .await
        .unwrap();
    let r = send(
        tls,
        "POST",
        "/api/v1/auth/setup",
        &[],
        Some(json!({ "setup_code": code, "username": "admin", "password": PASSWORD })),
    )
    .await;
    assert_eq!(r.status, 200, "{}", r.body);
    let set = r.headers["set-cookie"].to_str().unwrap();
    assert!(
        set.starts_with("__Host-hg_session=") && set.contains("; Secure"),
        "{set}"
    );
    api.stop.cancel();
}

fn free_port() -> u16 {
    std::net::TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
        .port()
}

#[tokio::test]
async fn dashboard_changes_admin_access_without_a_restart() {
    let dir = tempfile::tempdir().unwrap();
    let store = Arc::new(Store::open_dir(&dir.path().join("data")).unwrap());
    let stop = CancellationToken::new();
    let access = Arc::new(AccessControl::new(
        Source::Dashboard,
        TlsMode::Off,
        stop.clone(),
    ));
    let state = ApiState {
        store: Arc::clone(&store),
        gateway: Arc::new(Gateway::new()),
        certs: Arc::new(CertStore::new(dir.path())),
        runtime: Arc::new(RwLock::new(RuntimeInfo::default())),
        changed: Arc::new(tokio::sync::Notify::new()),
        token: TOKEN.into(),
        open_metrics: false,
        secure_cookies: false,
        auth: Arc::default(),
        access: Arc::clone(&access),
        live: Arc::default(),
        perimeter: Arc::new(hearthgate_firewall::Perimeter::new(
            hearthgate_firewall::Nft::new(vec!["false".into()]),
        )),
        router: Arc::default(),
        vpn: Arc::default(),
        docker: Arc::default(),
        ai: Arc::new(hearthgate_api::ai::Assistant::new(dir.path().join("data"))),
        updates: test_updater(dir.path()),
    };
    let first = free_port();
    access
        .start(
            state.router(),
            Resolved {
                listen: vec![SocketAddr::from(([127, 0, 0, 1], first))],
                allow: vec!["127.0.0.0/8".parse().unwrap()],
            },
        )
        .await;
    let api = Api {
        port: first,
        stop: stop.clone(),
        changed: Arc::new(tokio::sync::Notify::new()),
        store: Arc::clone(&store),
        tls: None,
        perimeter: Arc::clone(&state.perimeter),
        dir,
    };
    let (cookie, csrf) = set_up(&api).await;
    let auth = [("cookie", cookie.as_str()), ("x-csrf-token", csrf.as_str())];

    let r = request(first, "GET", "/api/v1/settings/admin-access", &auth, None).await;
    assert_eq!(r.status, 200, "{}", r.body);
    let info = r.json();
    assert_eq!(info["locked"], false);
    assert_eq!(info["settings"]["mode"], "lan");
    assert_eq!(info["client"], "127.0.0.1");

    // Public ranges are refused.
    let r = request(
        first,
        "PUT",
        "/api/v1/settings/admin-access",
        &auth,
        Some(json!({ "mode": "custom", "port": first, "listen": [], "allow": ["0.0.0.0/0"] })),
    )
    .await;
    assert_eq!(r.status, 422, "{}", r.body);
    assert!(r.json()["problems"][0].as_str().unwrap().contains("public"));

    // Moving to another port takes effect at once.
    let second = free_port();
    let r = request(
        first,
        "PUT",
        "/api/v1/settings/admin-access",
        &auth,
        Some(json!({ "mode": "custom", "port": second, "listen": [], "allow": [] })),
    )
    .await;
    assert_eq!(r.status, 200, "{}", r.body);
    assert_eq!(
        r.json()["effective"]["listen"],
        json!([format!("127.0.0.1:{second}")])
    );
    let r = request(second, "GET", "/api/v1/auth/session", &auth, None).await;
    assert_eq!(r.json()["user"]["username"], "admin");
    assert!(TcpStream::connect(("127.0.0.1", first)).await.is_err());
    let stored = store.meta_get("admin_access").unwrap().unwrap();
    assert!(stored.contains("custom"), "{stored}");
    api.stop.cancel();
}

#[tokio::test]
async fn live_endpoints() {
    let api = start(LOCAL).await;
    let auth = format!("Bearer {TOKEN}");
    let h = [("authorization", auth.as_str())];
    let r = request(api.port, "GET", "/api/v1/traffic", &h, None).await;
    assert_eq!(r.json().as_array().unwrap().len(), 24);
    let r = request(api.port, "GET", "/api/v1/logs?limit=5", &h, None).await;
    assert_eq!(r.status, 200);
    let r = request(api.port, "GET", "/api/v1/status", &h, None).await;
    assert!(r.json()["host"]["hostname"].is_string());

    // Preview shows the diff of a draft against the current state.
    let r = request(
        api.port,
        "POST",
        "/api/v1/state/preview",
        &h,
        Some(ssh_state(2222)),
    )
    .await;
    let preview = r.json();
    assert_eq!(preview["valid"], true, "{preview}");
    assert_eq!(preview["changed"], true);
    assert!(
        preview["diff"]
            .as_str()
            .unwrap()
            .contains("+name = \"ssh\"")
    );
    let (status, _) = call(
        api.port,
        "PUT",
        "/api/v1/state",
        Some(TOKEN),
        Some(json!({ "state": ssh_state(2222) })),
    )
    .await;
    assert_eq!(status, 200);
    let r = request(api.port, "GET", "/api/v1/revisions/1/diff", &h, None).await;
    assert_eq!(r.json()["previous"], Value::Null);
    assert!(r.json()["diff"].as_str().unwrap().contains("+port = 2222"));

    // The event stream starts with a status event.
    let stream = TcpStream::connect(("127.0.0.1", api.port)).await.unwrap();
    let (mut sender, conn) = hyper::client::conn::http1::handshake(TokioIo::new(stream))
        .await
        .unwrap();
    tokio::spawn(conn);
    let req = Request::builder()
        .uri("/api/v1/events")
        .header("host", "localhost")
        .header("authorization", &auth)
        .body(Full::new(Bytes::new()))
        .unwrap();
    let resp = sender.send_request(req).await.unwrap();
    assert_eq!(resp.headers()["content-type"], "text/event-stream");
    let mut body = resp.into_body();
    let frame = tokio::time::timeout(std::time::Duration::from_secs(5), body.frame())
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    let text = String::from_utf8_lossy(frame.data_ref().unwrap()).into_owned();
    assert!(text.starts_with("event: status"), "{text}");
    api.stop.cancel();
}

#[tokio::test]
async fn firewall_endpoints_and_change_confirmation() {
    let api = start(LOCAL).await;
    let (status, body) = call(api.port, "GET", "/api/v1/firewall", Some(TOKEN), None).await;
    assert_eq!(status, 200, "{body}");
    let info = json_of(&body);
    assert_eq!(info["installed"], false);
    assert!(info["pending"].is_null());
    assert_eq!(info["dropped"]["total"], 0);
    let (status, body) = call(api.port, "GET", "/api/v1/router", Some(TOKEN), None).await;
    assert_eq!(status, 200, "{body}");
    assert_eq!(json_of(&body)["enabled"], false);

    // Nothing to confirm, and nothing to ban into while the firewall is off.
    let (status, _) = call(
        api.port,
        "POST",
        "/api/v1/firewall/confirm",
        Some(TOKEN),
        None,
    )
    .await;
    assert_eq!(status, 404);
    let (status, body) = call(
        api.port,
        "POST",
        "/api/v1/firewall/bans",
        Some(TOKEN),
        Some(json!({ "address": "203.0.113.9" })),
    )
    .await;
    assert_eq!(status, 409, "{body}");

    // A connection opened before the change can't confirm it. The server
    // ends that connection so the client's next request opens a new one.
    let stream = TcpStream::connect(("127.0.0.1", api.port)).await.unwrap();
    let (mut old, conn) = hyper::client::conn::http1::handshake(TokioIo::new(stream))
        .await
        .unwrap();
    let conn = tokio::spawn(conn);
    let get = |path: &str, method: &str| {
        Request::builder()
            .method(method)
            .uri(path)
            .header("host", "localhost")
            .header("authorization", format!("Bearer {TOKEN}"))
            .body(Full::new(Bytes::new()))
            .unwrap()
    };
    let first = old
        .send_request(get("/api/v1/status", "GET"))
        .await
        .unwrap();
    assert_eq!(first.status(), 200);
    assert!(first.headers().get("connection").is_none());
    first.into_body().collect().await.unwrap();
    api.perimeter
        .begin_pending(hearthgate_firewall::Pending::new(
            7,
            hearthgate_model::Firewall::default(),
        ));
    let refused = old
        .send_request(get("/api/v1/firewall/confirm", "POST"))
        .await
        .unwrap();
    assert_eq!(refused.status(), 409);
    assert_eq!(refused.headers()["connection"], "close");
    refused.into_body().collect().await.unwrap();
    tokio::time::timeout(std::time::Duration::from_secs(5), conn)
        .await
        .expect("the old connection is closed")
        .unwrap()
        .unwrap();

    let (status, body) = call(api.port, "GET", "/api/v1/status", Some(TOKEN), None).await;
    assert_eq!(
        json_of(&body)["firewall"]["pending"]["revision"],
        7,
        "{status} {body}"
    );
    // A new connection confirms it.
    let (status, body) = call(
        api.port,
        "POST",
        "/api/v1/firewall/confirm",
        Some(TOKEN),
        None,
    )
    .await;
    assert_eq!(status, 204, "{body}");
    api.stop.cancel();
}

#[tokio::test]
async fn vpn_status_and_new_peer_configs() {
    let api = start(LOCAL).await;
    let (status, body) = call(api.port, "GET", "/api/v1/vpn", Some(TOKEN), None).await;
    assert_eq!(status, 200, "{body}");
    let info = json_of(&body);
    assert_eq!(info["enabled"], false);
    assert_eq!(info["running"], false);
    assert_eq!(info["address"], "10.88.0.1");
    assert!(info["endpoint"].is_null());

    // Without an endpoint, peers would not know where to connect.
    let ask = |body: Value| {
        call(
            api.port,
            "POST",
            "/api/v1/vpn/peer-config",
            Some(TOKEN),
            Some(body),
        )
    };
    let (status, body) = ask(json!({ "name": "phone" })).await;
    assert_eq!(status, 409, "{body}");

    // A draft state (as the dashboard sends it) picks the address and routes.
    let draft = json!({
        "services": [{ "name": "nas", "kind": "external", "host": "192.168.1.20", "ports": { "web": 5000 } }],
        "vpn": {
            "enabled": true,
            "endpoint": "vpn.example.com",
            "peers": [{ "name": "laptop", "public_key": "Y2FD6uTgrq+/bbBAXCNOtE+PGk8Papoh7qaWBpQUXn0=", "address": "10.88.0.2" }]
        }
    });
    let (status, body) = ask(json!({ "name": "guest", "access": ["nas"], "state": draft })).await;
    assert_eq!(status, 200, "{body}");
    let peer = json_of(&body);
    assert_eq!(peer["address"], "10.88.0.3");
    let config = peer["config"].as_str().unwrap();
    assert!(
        config.contains("Endpoint = vpn.example.com:51820"),
        "{config}"
    );
    assert!(
        config.contains("AllowedIPs = 10.88.0.0/24, 192.168.1.20/32"),
        "{config}"
    );
    assert!(
        config.contains("DNS = 10.88.0.1, home.internal"),
        "{config}"
    );
    assert!(peer["qr_svg"].as_str().unwrap().starts_with("<?xml"));
    let key = peer["public_key"].as_str().unwrap();
    assert!(hearthgate_model::is_wireguard_key(key));
    // Nothing was stored.
    let stored = api.store.current_state().unwrap().vpn.peers;
    assert!(stored.is_empty(), "{stored:?}");
    // The same server key every time.
    let server_key = |c: &str| {
        c.lines()
            .find(|l| l.starts_with("PublicKey"))
            .unwrap()
            .to_owned()
    };
    let (_, again) = ask(json!({ "name": "tv", "state": draft })).await;
    let again = json_of(&again);
    assert_eq!(
        server_key(config),
        server_key(again["config"].as_str().unwrap())
    );
    assert_ne!(again["public_key"], peer["public_key"]);

    for (bad, why) in [
        (json!({ "name": "Bad Name", "state": draft }), "name"),
        (
            json!({ "name": "x", "access": ["nope"], "state": draft }),
            "service",
        ),
        (
            json!({ "name": "x", "address": "10.88.0.2", "state": draft }),
            "taken address",
        ),
        (
            json!({ "name": "x", "address": "10.99.0.2", "state": draft }),
            "outside the subnet",
        ),
    ] {
        let (status, body) = ask(bad).await;
        assert_eq!(status, 422, "{why}: {body}");
    }
}

#[tokio::test]
async fn docker_endpoints_without_docker() {
    let api = start(LOCAL).await;
    let (status, body) = call(api.port, "GET", "/api/v1/docker", Some(TOKEN), None).await;
    assert_eq!(status, 200, "{body}");
    let info = json_of(&body);
    assert_eq!(info["enabled"], false);
    assert_eq!(info["containers"], json!([]));
    for (method, path, want) in [
        ("POST", "/api/v1/docker/containers/web/restart", 404),
        ("POST", "/api/v1/docker/containers/web/update", 404),
        ("POST", "/api/v1/docker/stacks/notes/update", 404),
        ("GET", "/api/v1/docker/containers/web/logs?tail=5", 404),
        ("GET", "/api/v1/docker/updates", 503),
    ] {
        let (status, body) = call(api.port, method, path, Some(TOKEN), None).await;
        assert_eq!(status, want, "{method} {path}: {body}");
    }
}

/// A stand-in AI provider that counts connections and keeps each request.
struct FakeProvider {
    port: u16,
    connections: Arc<std::sync::atomic::AtomicUsize>,
    requests: Arc<std::sync::Mutex<Vec<(String, String)>>>,
}

impl FakeProvider {
    /// Answers every request with `answer(request body)` as an
    /// OpenAI-style chat completion.
    async fn start(answer: fn(&str) -> Value) -> Self {
        use std::sync::atomic::Ordering;
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        let connections = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let requests = Arc::new(std::sync::Mutex::new(Vec::new()));
        let (count, kept) = (Arc::clone(&connections), Arc::clone(&requests));
        tokio::spawn(async move {
            loop {
                let (mut stream, _) = listener.accept().await.unwrap();
                count.fetch_add(1, Ordering::SeqCst);
                let kept = Arc::clone(&kept);
                tokio::spawn(async move {
                    let mut buf = Vec::new();
                    let mut chunk = [0u8; 8192];
                    let (head, body) = loop {
                        let n = stream.read(&mut chunk).await.unwrap();
                        if n == 0 {
                            return;
                        }
                        buf.extend_from_slice(&chunk[..n]);
                        let text = String::from_utf8_lossy(&buf).into_owned();
                        if let Some(end) = text.find("\r\n\r\n") {
                            let head = text[..end].to_owned();
                            let len: usize = head
                                .lines()
                                .find_map(|l| {
                                    l.to_ascii_lowercase()
                                        .strip_prefix("content-length:")
                                        .map(|v| v.trim().parse().unwrap())
                                })
                                .unwrap_or(0);
                            if buf.len() >= end + 4 + len {
                                break (
                                    head,
                                    String::from_utf8_lossy(&buf[end + 4..]).into_owned(),
                                );
                            }
                        }
                    };
                    let reply = json!({
                        "choices": [{ "message": { "role": "assistant", "content": answer(&body).to_string() } }]
                    })
                    .to_string();
                    kept.lock().unwrap().push((head, body));
                    let resp = format!(
                        "HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{reply}",
                        reply.len()
                    );
                    let _ = stream.write_all(resp.as_bytes()).await;
                });
            }
        });
        Self {
            port,
            connections,
            requests,
        }
    }

    fn connections(&self) -> usize {
        self.connections.load(std::sync::atomic::Ordering::SeqCst)
    }
}

/// The placeholder the request used for the service's host.
fn host_placeholder(body: &str) -> String {
    let at = body.find(r#"\"host\": \""#).expect("host in the material") + 12;
    let end = body[at..].find('>').unwrap();
    body[at..=at + end].to_owned()
}

fn ai_state(port: u16, enabled: bool) -> Value {
    json!({
        "interfaces": [{
            "name": "web", "domains": ["photos.familyhome.net"],
            "listeners": [{ "port": 18443, "mode": "raw" }]
        }],
        "services": [{
            "name": "photos", "kind": "external", "host": "192.168.1.20",
            "ports": { "http": 8080 }
        }],
        "routes": [{
            "name": "photos", "interface": "web", "listener": 18443,
            "targets": [{ "service": "photos", "port": "http" }]
        }],
        "notifications": {
            "webhooks": [{ "name": "phone", "url": "https://ntfy.familyhome.net/s3cr3t-topic" }]
        },
        "ai": {
            "enabled": enabled,
            "provider": "openai-compatible",
            "model": "llama3.1",
            "endpoint": format!("http://127.0.0.1:{port}/v1"),
            "redact": ["photos"]
        }
    })
}

#[tokio::test]
async fn ai_review_is_off_by_default_redacted_and_fixes_roll_back() {
    let api = start(LOCAL).await;
    let t = Some(TOKEN);
    let fake = FakeProvider::start(|body| {
        let host = host_placeholder(body);
        json!({
            "summary": "One problem.",
            "findings": [
                { "severity": "low", "area": "routes", "title": "Minor", "detail": "Nothing to change.",
                  "fix": { "summary": "", "changes": [] } },
                { "severity": "high", "area": "services", "title": format!("{host} answers on the wrong port"),
                  "detail": "Health checks fail.",
                  "fix": { "summary": "Use port 8081", "changes": [
                      { "op": "replace", "path": "/services/0/ports/http", "value_json": "8081" },
                      { "op": "replace", "path": "/services/0/host", "value_json": format!("\"{host}\"") }
                  ] } }
            ]
        })
    })
    .await;
    let leaks = ["192.168.1.20", "familyhome.net", "s3cr3t", "photos"];

    // Off: nothing reaches the provider, but the payload can be seen.
    let (status, body) = call(api.port, "POST", "/api/v1/ai/review", t, Some(json!({}))).await;
    assert_eq!(status, 409, "no state yet: {body}");
    let (status, body) = call(
        api.port,
        "PUT",
        "/api/v1/state",
        t,
        Some(json!({ "state": ai_state(fake.port, false) })),
    )
    .await;
    assert_eq!(status, 200, "{body}");
    let (status, body) = call(api.port, "POST", "/api/v1/ai/review", t, Some(json!({}))).await;
    assert_eq!(status, 409, "{body}");
    assert!(body.contains("turned off"), "{body}");
    let (status, body) = call(
        api.port,
        "POST",
        "/api/v1/ai/review",
        t,
        Some(json!({ "scope": { "kind": "service", "service": "photos" } })),
    )
    .await;
    assert_eq!(status, 409, "{body}");
    let (status, body) = call(api.port, "POST", "/api/v1/ai/preview", t, Some(json!({}))).await;
    assert_eq!(status, 200, "{body}");
    let preview = json_of(&body);
    assert_eq!(
        preview["url"],
        format!("http://127.0.0.1:{}/v1/chat/completions", fake.port)
    );
    assert!(preview["redacted"].as_u64().unwrap() >= 2);
    let sent = preview["body"].to_string();
    for leak in leaks {
        assert!(!sent.contains(leak), "{leak} in the preview: {sent}");
    }
    let (_, body) = call(api.port, "GET", "/api/v1/ai", t, None).await;
    assert_eq!(json_of(&body)["enabled"], false);
    assert_eq!(fake.connections(), 0, "AI is off: no connection at all");

    // On, against a local OpenAI-compatible model without a key.
    call(
        api.port,
        "PUT",
        "/api/v1/state",
        t,
        Some(json!({ "state": ai_state(fake.port, true) })),
    )
    .await;
    let (status, body) = call(api.port, "POST", "/api/v1/ai/review", t, Some(json!({}))).await;
    assert_eq!(status, 200, "{body}");
    assert_eq!(fake.connections(), 1);
    let (head, sent) = fake.requests.lock().unwrap()[0].clone();
    assert!(head.starts_with("POST /v1/chat/completions"), "{head}");
    assert!(
        !head.to_ascii_lowercase().contains("authorization"),
        "{head}"
    );
    for leak in leaks {
        assert!(!sent.contains(leak), "{leak} was sent: {sent}");
    }
    let review = json_of(&body);
    assert_eq!(review["model"], "llama3.1");
    assert_eq!(review["findings"][0]["severity"], "high");
    assert_eq!(
        review["findings"][0]["title"],
        "192.168.1.20 answers on the wrong port"
    );
    assert!(review["findings"][1].get("fix").is_none());
    let id = review["id"].as_str().unwrap().to_owned();

    // The fix becomes a draft, applies as a revision and rolls back.
    let (status, body) = call(
        api.port,
        "POST",
        &format!("/api/v1/ai/reviews/{id}/findings/0/draft"),
        t,
        None,
    )
    .await;
    assert_eq!(status, 200, "{body}");
    let draft = json_of(&body)["state"].clone();
    assert_eq!(draft["services"][0]["host"], "192.168.1.20");
    assert_eq!(draft["services"][0]["ports"]["http"], 8081);
    assert_eq!(
        call(
            api.port,
            "POST",
            &format!("/api/v1/ai/reviews/{id}/findings/1/draft"),
            t,
            None
        )
        .await
        .0,
        404
    );
    let (status, body) = call(
        api.port,
        "PUT",
        "/api/v1/state",
        t,
        Some(json!({ "state": draft, "message": "AI fix" })),
    )
    .await;
    assert_eq!(status, 200, "{body}");
    assert_eq!(json_of(&body)["revision"]["id"], 3);
    let (status, body) = call(api.port, "POST", "/api/v1/revisions/2/rollback", t, None).await;
    assert_eq!(status, 200, "{body}");
    let (_, body) = call(api.port, "GET", "/api/v1/state", t, None).await;
    assert_eq!(
        json_of(&body)["state"]["services"][0]["ports"]["http"],
        8080
    );

    let (_, body) = call(api.port, "GET", "/api/v1/ai/reviews", t, None).await;
    assert_eq!(json_of(&body).as_array().unwrap().len(), 1);
    api.stop.cancel();
}

#[tokio::test]
async fn ai_key_is_stored_privately() {
    let api = start(LOCAL).await;
    let t = Some(TOKEN);
    let (status, body) = call(
        api.port,
        "PUT",
        "/api/v1/ai/key",
        t,
        Some(json!({ "key": "sk-test-123" })),
    )
    .await;
    assert_eq!(status, 204, "{body}");
    let path = api
        .dir
        .path()
        .join("data")
        .join(hearthgate_api::ai::KEY_FILE);
    assert_eq!(
        std::fs::read_to_string(&path).unwrap().trim(),
        "sk-test-123"
    );
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(
            std::fs::metadata(&path).unwrap().permissions().mode() & 0o777,
            0o600
        );
    }
    let (_, body) = call(api.port, "GET", "/api/v1/ai", t, None).await;
    let info = json_of(&body);
    assert_eq!(info["key_set"], true);
    assert_eq!(info["model"], "claude-opus-5-5");
    let (_, body) = call(api.port, "GET", "/api/v1/state/export", t, None).await;
    assert!(!body.contains("sk-test"), "{body}");
    call(
        api.port,
        "PUT",
        "/api/v1/ai/key",
        t,
        Some(json!({ "key": "" })),
    )
    .await;
    assert!(!path.exists());
    api.stop.cancel();
}

fn test_updater(dir: &std::path::Path) -> Arc<hearthgate_update::Updater> {
    Arc::new(hearthgate_update::Updater::new(hearthgate_update::Config {
        layout: hearthgate_update::state::Layout {
            data_dir: dir.join("data"),
            helper_dir: dir.join("helper"),
        },
        current: "0.0.0-dev".into(),
        method: hearthgate_update::method::Method::Manual,
        helper: false,
        arch: None,
        fetch: Arc::new(hearthgate_update::fetch::HttpFetch),
        keys: Vec::new(),
        settings: hearthgate_model::Updates::default(),
    }))
}

#[tokio::test]
async fn updates_endpoints_answer() {
    let api = start(LOCAL).await;
    let t = Some(TOKEN);
    let (status, body) = call(api.port, "GET", "/api/v1/updates", t, None).await;
    assert_eq!(status, 200, "{body}");
    let body = json_of(&body);
    assert_eq!(body["current"], "0.0.0-dev");
    assert_eq!(body["can_install"], false);
    assert!(body["cannot_install"].as_str().unwrap().contains("development build"));
    let (status, body) = call(
        api.port,
        "POST",
        "/api/v1/updates/install",
        t,
        Some(json!({ "version": "9.9.9" })),
    )
    .await;
    assert_eq!(status, 409, "{body}");
    let (status, _) = call(api.port, "POST", "/api/v1/updates/skip", t, Some(json!({ "version": "nope" }))).await;
    assert_eq!(status, 422);
    let (status, body) = call(api.port, "POST", "/api/v1/updates/skip", t, Some(json!({ "version": "9.9.9" }))).await;
    assert_eq!(status, 200);
    assert_eq!(json_of(&body)["skipped"], json!(["9.9.9"]));
    let (status, body) = call(api.port, "GET", "/api/v1/updates/history", t, None).await;
    assert_eq!(status, 200);
    assert_eq!(json_of(&body), json!([]));
    let (status, _) = call(api.port, "GET", "/api/v1/updates", None, None).await;
    assert_eq!(status, 401);
}
