//! DNS-01 challenges, end to end: Pebble checks TXT records on a real BIND
//! server, which Hearthgate updates with TSIG-signed RFC 2136 updates. The
//! certificate includes a wildcard name.
//!
//! Needs `pebble` (see pebble.rs) and BIND's `named` (`apt install bind9`).
//! Skipped when they are missing, unless HEARTHGATE_DNS_TESTS is set.

use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::Arc;
use std::time::Duration;

use hearthgate_acme::dns::{DnsSolver, challenge_name};
use hearthgate_acme::{CertStore, Issuer, Outcome, jobs};
use hearthgate_model::DesiredState;
use hearthgate_proxy::Gateway;

const ACME_PORT: u16 = 14010;
const DNS_PORT: u16 = 18153;
const SECRET: &str = "c2VjcmV0LWtleS1mb3ItaGVhcnRoZ2F0ZS10ZXN0cyE=";

struct Kill(Child);
impl Drop for Kill {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

fn find_bin(name: &str) -> Option<PathBuf> {
    let home = std::env::var("HOME").unwrap_or_default();
    let mut candidates = vec![
        PathBuf::from(&home).join("go/bin").join(name),
        PathBuf::from("/usr/sbin").join(name),
    ];
    if let Ok(path) = std::env::var("PATH") {
        candidates.extend(std::env::split_paths(&path).map(|p| p.join(name)));
    }
    candidates.into_iter().find(|p| p.is_file())
}

fn pebble_dir() -> Option<PathBuf> {
    if let Ok(dir) = std::env::var("PEBBLE_DIR") {
        return Some(PathBuf::from(dir));
    }
    let home = std::env::var("HOME").unwrap_or_default();
    let base = Path::new(&home).join("go/pkg/mod/github.com/letsencrypt/pebble");
    let mut found: Vec<PathBuf> = std::fs::read_dir(base)
        .ok()?
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.join("test/certs/pebble.minica.pem").is_file())
        .collect();
    found.sort();
    found.pop()
}

async fn wait_port(port: u16) {
    for _ in 0..100 {
        if tokio::net::TcpStream::connect(("127.0.0.1", port))
            .await
            .is_ok()
        {
            return;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    panic!("port {port} did not open");
}

/// Starts BIND, authoritative for `example.test`, accepting updates signed
/// with the test key.
fn start_named(named: &Path, dir: &Path) -> Kill {
    std::fs::write(
        dir.join("example.test.zone"),
        "$TTL 60\n\
         @ IN SOA ns.example.test. admin.example.test. 1 60 60 3600 60\n\
         @ IN NS ns.example.test.\n\
         ns IN A 127.0.0.1\n",
    )
    .unwrap();
    std::fs::write(
        dir.join("named.conf"),
        format!(
            r#"key "hearthgate" {{ algorithm hmac-sha256; secret "{SECRET}"; }};
options {{
    directory "{d}";
    pid-file none;
    listen-on port {DNS_PORT} {{ 127.0.0.1; }};
    listen-on-v6 {{ none; }};
    recursion no;
    dnssec-validation no;
}};
zone "example.test" {{
    type primary;
    file "{d}/example.test.zone";
    update-policy {{ grant hearthgate wildcard *.example.test. TXT; }};
}};
"#,
            d = dir.display()
        ),
    )
    .unwrap();
    Kill(
        Command::new(named)
            .args(["-g", "-c"])
            .arg(dir.join("named.conf"))
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .unwrap(),
    )
}

async fn txt(name: &str) -> Vec<String> {
    let out = tokio::process::Command::new("dig")
        .args([
            "+short",
            "-p",
            &DNS_PORT.to_string(),
            "@127.0.0.1",
            name,
            "TXT",
        ])
        .output()
        .await;
    match out {
        Ok(o) => String::from_utf8_lossy(&o.stdout)
            .lines()
            .map(|l| l.trim_matches('"').to_owned())
            .collect(),
        Err(_) => Vec::new(),
    }
}

#[tokio::test]
async fn issues_a_wildcard_with_rfc2136() {
    let (Some(pebble), Some(named), Some(dir)) =
        (find_bin("pebble"), find_bin("named"), pebble_dir())
    else {
        assert!(
            std::env::var_os("HEARTHGATE_DNS_TESTS").is_none(),
            "HEARTHGATE_DNS_TESTS is set, but pebble or named is missing"
        );
        eprintln!("skipping: pebble or named not found");
        return;
    };
    let tmp = tempfile::tempdir().unwrap();
    let _named = start_named(&named, tmp.path());
    wait_port(DNS_PORT).await;

    let key = tmp.path().join("tsig.key");
    std::fs::write(
        &key,
        format!("key \"hearthgate\" {{\n\talgorithm hmac-sha256;\n\tsecret \"{SECRET}\";\n}};\n"),
    )
    .unwrap();
    let config = tmp.path().join("pebble.json");
    std::fs::write(
        &config,
        format!(
            r#"{{"pebble": {{
                "listenAddress": "127.0.0.1:{ACME_PORT}",
                "managementListenAddress": "127.0.0.1:15010",
                "certificate": "{d}/test/certs/localhost/cert.pem",
                "privateKey": "{d}/test/certs/localhost/key.pem",
                "httpPort": 15012,
                "tlsPort": 15011,
                "ocspResponderURL": "",
                "externalAccountBindingRequired": false
            }}}}"#,
            d = dir.display()
        ),
    )
    .unwrap();
    let _pebble = Kill(
        Command::new(pebble)
            .arg("-config")
            .arg(&config)
            .args(["-dnsserver", &format!("127.0.0.1:{DNS_PORT}")])
            .env("PEBBLE_VA_NOSLEEP", "1")
            .env("PEBBLE_WFE_NONCEREJECT", "0")
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .unwrap(),
    );
    wait_port(ACME_PORT).await;

    let state = DesiredState::from_toml(&format!(
        r#"
[acme]
directory = "https://127.0.0.1:{ACME_PORT}/dir"
directory_root = "{root}"
accept_terms = true
challenge = "dns-01"

[acme.dns]
provider = "rfc2136"
server = "127.0.0.1:{DNS_PORT}"
zone = "example.test"
key_name = "hearthgate"
key_file = "{key}"
propagation = "0s"

[[interfaces]]
name = "web"
domains = ["example.test", "*.example.test"]
listeners = [{{ port = 443, mode = "https" }}]
"#,
        root = dir.join("test/certs/pebble.minica.pem").display(),
        key = key.display(),
    ))
    .unwrap();
    state.validate().unwrap();

    // Records are set and removed again.
    let solver = DnsSolver::new(state.acme.dns.as_ref().unwrap());
    let name = challenge_name("*.example.test");
    let a = solver.set(&name, "first").await.unwrap();
    let b = solver.set(&name, "second").await.unwrap();
    let mut values = txt(&name).await;
    values.sort();
    if which("dig") {
        assert_eq!(values, ["first", "second"]);
    }
    solver.remove(&a).await.unwrap();
    solver.remove(&b).await.unwrap();
    if which("dig") {
        assert!(txt(&name).await.is_empty(), "records are removed");
    }

    // A wrong key is refused with a clear message.
    let bad_key = tmp.path().join("bad.key");
    std::fs::write(&bad_key, "d3Jvbmcta2V5LXdyb25nLWtleS13cm9uZy1rZXkhIQ==").unwrap();
    let mut bad = state.acme.dns.clone().unwrap();
    bad.key_file = Some(bad_key.display().to_string());
    let err = DnsSolver::new(&bad).set(&name, "x").await.unwrap_err();
    assert!(err.contains("NOTAUTH"), "{err}");

    // The whole issuance, wildcard included.
    let gw = Gateway::new();
    let store = Arc::new(CertStore::new(tmp.path()));
    let issuer = Issuer::new(
        tmp.path(),
        Arc::clone(&store),
        Arc::clone(gw.certificates()),
        Arc::clone(gw.http_challenges()),
    );
    let results = issuer.run(&state.acme, &jobs(&state)).await;
    match &results[0].1 {
        Outcome::Issued(info) => {
            let mut got = info.domains.clone();
            got.sort();
            assert_eq!(got, ["*.example.test", "example.test"]);
        }
        other => panic!("expected a certificate, got {other:?}"),
    }
    if which("dig") {
        assert!(txt(&name).await.is_empty(), "challenge records are removed");
    }
}

fn which(bin: &str) -> bool {
    find_bin(bin).is_some()
}

/// A fake Cloudflare API: one zone, `example.com`, holding TXT records.
async fn fake_cloudflare() -> (String, Arc<std::sync::Mutex<Vec<(String, String, String)>>>) {
    use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}/client/v4", listener.local_addr().unwrap());
    let records = Arc::new(std::sync::Mutex::new(Vec::<(String, String, String)>::new()));
    let shared = Arc::clone(&records);
    tokio::spawn(async move {
        let mut next_id = 0;
        while let Ok((stream, _)) = listener.accept().await {
            let mut reader = BufReader::new(stream);
            let mut line = String::new();
            reader.read_line(&mut line).await.unwrap();
            let mut parts = line.split_whitespace();
            let method = parts.next().unwrap_or_default().to_owned();
            let path = parts.next().unwrap_or_default().to_owned();
            let mut length = 0;
            let mut auth = String::new();
            loop {
                let mut header = String::new();
                reader.read_line(&mut header).await.unwrap();
                let header = header.trim_end();
                if header.is_empty() {
                    break;
                }
                let (k, v) = header.split_once(':').unwrap();
                match k.to_ascii_lowercase().as_str() {
                    "content-length" => length = v.trim().parse().unwrap(),
                    "authorization" => auth = v.trim().to_owned(),
                    _ => {}
                }
            }
            let mut body = vec![0u8; length];
            reader.read_exact(&mut body).await.unwrap();
            let body: serde_json::Value = serde_json::from_slice(&body).unwrap_or_default();
            let ok = |result: serde_json::Value| serde_json::json!({"success": true, "errors": [], "result": result});
            let (status, reply) = if auth != "Bearer test-token" {
                (
                    403,
                    serde_json::json!({"success": false, "errors": [{"code": 9109, "message": "Invalid access token"}]}),
                )
            } else if method == "GET" && path == "/client/v4/zones?name=example.com" {
                (
                    200,
                    ok(serde_json::json!([{"id": "zone1", "name": "example.com"}])),
                )
            } else if method == "GET" && path.starts_with("/client/v4/zones?name=") {
                (200, ok(serde_json::json!([])))
            } else if method == "POST" && path == "/client/v4/zones/zone1/dns_records" {
                next_id += 1;
                let id = format!("rec{next_id}");
                shared.lock().unwrap().push((
                    id.clone(),
                    body["name"].as_str().unwrap().to_owned(),
                    body["content"].as_str().unwrap().to_owned(),
                ));
                (200, ok(serde_json::json!({"id": id})))
            } else if method == "DELETE" && path.starts_with("/client/v4/zones/zone1/dns_records/")
            {
                let id = path.rsplit('/').next().unwrap();
                shared.lock().unwrap().retain(|(r, _, _)| r != id);
                (200, ok(serde_json::json!({"id": id})))
            } else {
                (
                    404,
                    serde_json::json!({"success": false, "errors": [{"message": "not found"}]}),
                )
            };
            let text = reply.to_string();
            let mut stream = reader.into_inner();
            let _ = stream
                .write_all(
                    format!(
                        "HTTP/1.1 {status} X\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{text}",
                        text.len()
                    )
                    .as_bytes(),
                )
                .await;
        }
    });
    (url, records)
}

#[tokio::test]
async fn sets_and_removes_cloudflare_records() {
    let (url, records) = fake_cloudflare().await;
    let tmp = tempfile::tempdir().unwrap();
    let token = tmp.path().join("token");
    std::fs::write(&token, "test-token\n").unwrap();
    let state = DesiredState::from_toml(&format!(
        r#"
[acme]
challenge = "dns-01"
[acme.dns]
provider = "cloudflare"
api_token_file = "{}"
api_url = "{url}"
"#,
        token.display()
    ))
    .unwrap();
    state.validate().unwrap();
    let mut settings = state.acme.dns.clone().unwrap();
    let solver = DnsSolver::new(&settings);
    let name = challenge_name("*.app.example.com");
    let record = solver.set(&name, "value").await.unwrap();
    assert_eq!(
        records.lock().unwrap().clone(),
        [(
            "rec1".to_owned(),
            "_acme-challenge.app.example.com".to_owned(),
            "value".to_owned()
        )]
    );
    solver.remove(&record).await.unwrap();
    assert!(records.lock().unwrap().is_empty());

    let err = solver
        .set("_acme-challenge.other.org", "v")
        .await
        .unwrap_err();
    assert!(err.contains("no Cloudflare zone"), "{err}");

    std::fs::write(&token, "wrong").unwrap();
    let err = solver.set(&name, "v").await.unwrap_err();
    assert!(err.contains("Invalid access token"), "{err}");

    settings.api_token_file = Some("/nonexistent/token".into());
    let err = DnsSolver::new(&settings).set(&name, "v").await.unwrap_err();
    assert!(err.contains("cannot read"), "{err}");
}
