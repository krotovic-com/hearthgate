//! Issues real certificates from Pebble, the ACME test server, with the
//! gateway answering both challenge types.
//!
//! Needs `pebble` and `pebble-challtestsrv` (from
//! `go install github.com/letsencrypt/pebble/v2/cmd/...@v2.8.0`). The test is
//! skipped when they are missing. Set `PEBBLE_DIR` to Pebble's source
//! directory if it is not in the Go module cache.

use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::Arc;
use std::time::Duration;

use hearthgate_acme::{CertSource, CertStore, Issuer, Outcome, jobs};
use hearthgate_model::DesiredState;
use hearthgate_proxy::{Gateway, NoContainers, compile};

const HTTP_PORT: u16 = 15002;
const TLS_PORT: u16 = 15001;
const DIRECTORY: &str = "https://127.0.0.1:14000/dir";

struct Kill(Child);
impl Drop for Kill {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

fn find_bin(name: &str) -> Option<PathBuf> {
    let home = std::env::var("HOME").unwrap_or_default();
    let mut candidates = vec![PathBuf::from(&home).join("go/bin").join(name)];
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

fn state(challenge: &str, domains: [&str; 2], root: &Path) -> DesiredState {
    DesiredState::from_toml(&format!(
        r#"
[acme]
directory = "{DIRECTORY}"
directory_root = "{}"
accept_terms = true
email = "admin@example.com"
challenge = "{challenge}"

[[interfaces]]
name = "web"
bind = ["127.0.0.1"]
domains = ["{}", "{}"]
listeners = [{{ port = {HTTP_PORT}, mode = "http" }}, {{ port = {TLS_PORT}, mode = "https" }}]
"#,
        root.display(),
        domains[0],
        domains[1],
    ))
    .unwrap()
}

#[tokio::test]
async fn issues_with_http01_and_tls_alpn01() {
    let (Some(pebble), Some(challtestsrv), Some(dir)) = (
        find_bin("pebble"),
        find_bin("pebble-challtestsrv"),
        pebble_dir(),
    ) else {
        eprintln!("skipping: pebble, pebble-challtestsrv or its test certificates not found");
        return;
    };
    let tmp = tempfile::tempdir().unwrap();
    let config = tmp.path().join("pebble.json");
    std::fs::write(
        &config,
        format!(
            r#"{{"pebble": {{
                "listenAddress": "127.0.0.1:14000",
                "managementListenAddress": "127.0.0.1:15000",
                "certificate": "{d}/test/certs/localhost/cert.pem",
                "privateKey": "{d}/test/certs/localhost/key.pem",
                "httpPort": {HTTP_PORT},
                "tlsPort": {TLS_PORT},
                "ocspResponderURL": "",
                "externalAccountBindingRequired": false
            }}}}"#,
            d = dir.display()
        ),
    )
    .unwrap();
    let _dns = Kill(
        Command::new(challtestsrv)
            .args([
                "-defaultIPv4",
                "127.0.0.1",
                "-defaultIPv6",
                "",
                "-dns01",
                "127.0.0.1:18053",
                "-http01",
                "",
                "-https01",
                "",
                "-tlsalpn01",
                "",
                "-doh",
                "",
                "-management",
                "127.0.0.1:18055",
            ])
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .unwrap(),
    );
    let _pebble = Kill(
        Command::new(pebble)
            .arg("-config")
            .arg(&config)
            .args(["-dnsserver", "127.0.0.1:18053"])
            .env("PEBBLE_VA_NOSLEEP", "1")
            .env("PEBBLE_WFE_NONCEREJECT", "0")
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .unwrap(),
    );
    wait_port(14000).await;
    let root = dir.join("test/certs/pebble.minica.pem");

    let gw = Gateway::new();
    let store = Arc::new(CertStore::new(tmp.path()));
    let issuer = Issuer::new(
        tmp.path(),
        Arc::clone(&store),
        Arc::clone(gw.certificates()),
        Arc::clone(gw.http_challenges()),
    );

    // Different names per round, so Pebble cannot reuse an authorization
    // and each challenge type is really exercised.
    for (challenge, domains) in [
        ("http-01", ["app.example.com", "www.example.com"]),
        ("tls-alpn-01", ["alpn.example.com", "www.alpn.example.com"]),
    ] {
        let state = state(challenge, domains, &root);
        let plan = compile(&state, gw.upstreams(), &NoContainers);
        let (table, _) = store.table(&plan.certs);
        gw.certificates().set(table);
        assert!(gw.apply(&plan).await.is_empty());

        // Force a fresh certificate for each challenge type.
        let _ = store.remove(CertSource::Acme, "web");
        let results = issuer.run(&state.acme, &jobs(&state)).await;
        match &results[0].1 {
            Outcome::Issued(info) => {
                assert!(
                    info.issuer.contains("Pebble"),
                    "{challenge}: {}",
                    info.issuer
                );
                let mut got = info.domains.clone();
                got.sort();
                let mut want = domains.to_vec();
                want.sort_unstable();
                assert_eq!(got, want);
            }
            other => panic!("{challenge}: expected a certificate, got {other:?}"),
        }
        // A second pass finds the certificate valid and does nothing.
        let again = issuer.run(&state.acme, &jobs(&state)).await;
        assert!(matches!(again[0].1, Outcome::Valid(_)), "{:?}", again[0].1);
    }

    // The account was stored privately and is reused.
    let accounts: Vec<_> = std::fs::read_dir(tmp.path().join("acme"))
        .unwrap()
        .collect();
    assert_eq!(accounts.len(), 1);
    gw.shutdown(Duration::from_millis(100)).await;
}

#[tokio::test]
async fn failures_back_off() {
    let tmp = tempfile::tempdir().unwrap();
    let gw = Gateway::new();
    let store = Arc::new(CertStore::new(tmp.path()));
    let issuer = Issuer::new(
        tmp.path(),
        store,
        Arc::clone(gw.certificates()),
        Arc::clone(gw.http_challenges()),
    );
    // Nothing listens on this directory URL.
    let state = DesiredState::from_toml(
        r#"
[acme]
directory = "https://127.0.0.1:9/dir"
accept_terms = true
[[interfaces]]
name = "web"
domains = ["app.example.com"]
listeners = [{ port = 443, mode = "https" }]
"#,
    )
    .unwrap();
    let first = issuer.run(&state.acme, &jobs(&state)).await;
    assert!(matches!(first[0].1, Outcome::Failed(_)), "{:?}", first[0].1);
    let second = issuer.run(&state.acme, &jobs(&state)).await;
    assert!(
        matches!(second[0].1, Outcome::Backoff { .. }),
        "{:?}",
        second[0].1
    );
    issuer.reset_backoff();
    let third = issuer.run(&state.acme, &jobs(&state)).await;
    assert!(matches!(third[0].1, Outcome::Failed(_)));
}
