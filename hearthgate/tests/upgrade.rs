//! Upgrades in one step from every earlier release: data directories made
//! by those releases (tests/upgrade/vX.Y.Z, from
//! scripts/make-upgrade-fixture.sh) must open, keep their state, history,
//! certificates, admin token and accounts, and serve them.

use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

use hearthgate_model::DesiredState;

const BIN: &str = env!("CARGO_BIN_EXE_hearthgate");

struct Daemon(Child);
impl Drop for Daemon {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

#[derive(serde::Deserialize)]
struct Fixture {
    version: String,
    revisions: usize,
    account: bool,
    admin_token: String,
}

fn fixtures() -> Vec<PathBuf> {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/upgrade");
    let mut out: Vec<PathBuf> = std::fs::read_dir(dir)
        .unwrap()
        .filter_map(Result::ok)
        .map(|e| e.path())
        .filter(|p| p.join("fixture.json").is_file())
        .collect();
    out.sort();
    out
}

fn copy_dir(from: &Path, to: &Path) {
    std::fs::create_dir_all(to).unwrap();
    for entry in std::fs::read_dir(from).unwrap() {
        let entry = entry.unwrap();
        let target = to.join(entry.file_name());
        if entry.file_type().unwrap().is_dir() {
            copy_dir(&entry.path(), &target);
        } else {
            std::fs::copy(entry.path(), &target).unwrap();
        }
    }
}

fn cli(config: &Path, data: &Path, args: &[&str]) -> String {
    let out = Command::new(BIN)
        .arg("--config")
        .arg(config)
        .arg("--data-dir")
        .arg(data)
        .args(args)
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "hearthgate {args:?} failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8(out.stdout).unwrap()
}

fn free_port() -> u16 {
    std::net::TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
        .port()
}

/// `curl` against the dashboard's self-signed certificate. Returns the
/// status code and body.
fn curl(port: u16, path: &str, args: &[&str]) -> (u16, String) {
    let out = Command::new("curl")
        .args(["-ks", "-o", "-", "-w", "\n%{http_code}"])
        .args(args)
        .arg(format!("https://127.0.0.1:{port}{path}"))
        .output()
        .expect("curl is needed for this test");
    let text = String::from_utf8_lossy(&out.stdout).into_owned();
    let (body, code) = text.rsplit_once('\n').unwrap_or(("", "0"));
    (code.trim().parse().unwrap_or(0), body.to_owned())
}

#[test]
fn upgrades_from_every_release() {
    let all = fixtures();
    assert!(all.len() >= 2, "fixtures for 0.2.0 and 0.3.0 at least");
    for fixture in all {
        let info: Fixture =
            serde_json::from_str(&std::fs::read_to_string(fixture.join("fixture.json")).unwrap())
                .unwrap();
        let v = &info.version;
        let tmp = tempfile::tempdir().unwrap();
        let data = tmp.path().join("data");
        copy_dir(&fixture.join("data"), &data);
        let port = free_port();
        let config = tmp.path().join("hearthgate.toml");
        std::fs::write(
            &config,
            format!(
                "[admin]\nlisten = [\"127.0.0.1:{port}\"]\nallow = [\"127.0.0.0/8\"]\n\
                 [docker]\nenabled = false\n[backup]\nenabled = false\n"
            ),
        )
        .unwrap();

        // The state is what that release exported, with its history.
        let expected = DesiredState::from_toml(
            &std::fs::read_to_string(fixture.join("expected.toml")).unwrap(),
        )
        .unwrap_or_else(|e| panic!("{v}: its export no longer parses: {e}"))
        .normalized();
        let exported = DesiredState::from_toml(&cli(&config, &data, &["export"])).unwrap();
        assert_eq!(exported.normalized(), expected, "{v}: state changed");
        cli(&config, &data, &["validate"]);
        let revisions = cli(&config, &data, &["revisions"]);
        assert_eq!(
            revisions.lines().filter(|l| l.contains("revision")).count(),
            info.revisions,
            "{v}: {revisions}"
        );
        assert!(revisions.contains("Second revision"), "{v}: {revisions}");
        let certs = cli(&config, &data, &["cert", "list"]);
        assert!(certs.contains("fixture"), "{v}: {certs}");
        assert_eq!(
            cli(&config, &data, &["admin-token"]).trim(),
            info.admin_token,
            "{v}: the admin token changed"
        );
        // A rollback to the first revision still works.
        let rolled = cli(&config, &data, &["rollback", "1", "--dry-run"]);
        assert!(rolled.contains("least-connections"), "{v}: {rolled}");

        // The daemon starts on it and serves the API with the old token.
        let _daemon = Daemon(
            Command::new(BIN)
                .arg("--config")
                .arg(&config)
                .arg("--data-dir")
                .arg(&data)
                .arg("run")
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .spawn()
                .unwrap(),
        );
        let auth = format!("Authorization: Bearer {}", info.admin_token);
        let start = Instant::now();
        // Wait until the API answers and the stored state is running (the
        // listeners can be up a moment before the revision is recorded).
        let status = loop {
            let (code, body) = curl(port, "/api/v1/status", &["-H", &auth]);
            if code == 200
                && serde_json::from_str::<serde_json::Value>(&body)
                    .is_ok_and(|s| !s["revision"].is_null())
            {
                break body;
            }
            assert!(
                start.elapsed() < Duration::from_secs(15),
                "{v}: the API did not come up ({code} {body})"
            );
            std::thread::sleep(Duration::from_millis(100));
        };
        let status: serde_json::Value = serde_json::from_str(&status).unwrap();
        assert_eq!(status["revision"], info.revisions, "{v}: {status}");

        // Dashboard accounts still log in.
        if info.account {
            let (code, body) = curl(
                port,
                "/api/v1/auth/login",
                &[
                    "-H",
                    "content-type: application/json",
                    "-d",
                    r#"{"username":"fixture","password":"fixture-password-1"}"#,
                ],
            );
            assert_eq!(code, 200, "{v}: login failed: {body}");
        } else {
            // No account yet: the setup code works as on a new install.
            cli(&config, &data, &["setup-code"]);
        }

        // And a backup of the upgraded data restores on a new machine.
        let backup = tmp.path().join("backup.tar.gz");
        cli(
            &config,
            &data,
            &["backup", "create", "-o", backup.to_str().unwrap()],
        );
        let fresh = tmp.path().join("fresh");
        cli(
            &config,
            &fresh,
            &["backup", "restore", backup.to_str().unwrap()],
        );
        let restored = DesiredState::from_toml(&cli(&config, &fresh, &["export"])).unwrap();
        assert_eq!(restored.normalized(), expected, "{v}");
        assert!(cli(&config, &fresh, &["cert", "list"]).contains("fixture"));
        assert_eq!(
            cli(&config, &fresh, &["admin-token"]).trim(),
            info.admin_token
        );
    }
}
