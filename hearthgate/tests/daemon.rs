//! End to end: the real binary, the CLI and the daemon together.

use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::path::Path;
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

const BIN: &str = env!("CARGO_BIN_EXE_hearthgate");

struct Daemon(Child);

impl Drop for Daemon {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

fn cli(data: &Path, args: &[&str]) -> String {
    let out = Command::new(BIN)
        .arg("--data-dir")
        .arg(data)
        .args(args)
        .env("HEARTHGATE_CONFIG", "/dev/null")
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
    TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
        .port()
}

fn echo_server(tag: &'static str) -> u16 {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    std::thread::spawn(move || {
        for stream in listener.incoming() {
            let Ok(mut s) = stream else { continue };
            std::thread::spawn(move || {
                let mut buf = [0u8; 256];
                while let Ok(n) = s.read(&mut buf) {
                    if n == 0 {
                        break;
                    }
                    let mut out = tag.as_bytes().to_vec();
                    out.extend_from_slice(&buf[..n]);
                    if s.write_all(&out).is_err() {
                        break;
                    }
                }
            });
        }
    });
    port
}

fn state_file(dir: &Path, name: &str, listen: u16, upstream: u16) -> String {
    let path = dir.join(name);
    std::fs::write(
        &path,
        format!(
            r#"
[[interfaces]]
name = "raw"
bind = ["127.0.0.1"]
listeners = [{{ port = {listen}, mode = "raw" }}]
[[services]]
name = "echo"
kind = "external"
host = "127.0.0.1"
ports = {{ p = {upstream} }}
[[routes]]
name = "raw"
interface = "raw"
listener = {listen}
targets = [{{ service = "echo", port = "p" }}]
"#
        ),
    )
    .unwrap();
    path.to_string_lossy().into_owned()
}

fn exchange(stream: &mut TcpStream, msg: &str) -> String {
    stream.write_all(msg.as_bytes()).unwrap();
    let mut buf = [0u8; 256];
    let n = stream.read(&mut buf).unwrap();
    String::from_utf8_lossy(&buf[..n]).into_owned()
}

fn wait_for(mut check: impl FnMut() -> bool) {
    let start = Instant::now();
    while start.elapsed() < Duration::from_secs(10) {
        if check() {
            return;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    panic!("condition not met in time");
}

#[test]
fn apply_and_rollback_take_effect_without_restart() {
    let dir = tempfile::tempdir().unwrap();
    let data = dir.path().join("data");
    let a = echo_server("a:");
    let b = echo_server("b:");
    let listen = free_port();

    let daemon = Daemon(
        Command::new(BIN)
            .arg("--data-dir")
            .arg(&data)
            .arg("run")
            .env("HEARTHGATE_CONFIG", "/dev/null")
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .unwrap(),
    );

    // Nothing is listening before the first apply.
    std::thread::sleep(Duration::from_millis(300));
    assert!(TcpStream::connect(("127.0.0.1", listen)).is_err());

    let first = state_file(dir.path(), "a.toml", listen, a);
    cli(&data, &["apply", &first]);
    let mut conn = None;
    wait_for(|| {
        conn = TcpStream::connect(("127.0.0.1", listen)).ok();
        conn.is_some()
    });
    let mut conn = conn.unwrap();
    conn.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
    assert_eq!(exchange(&mut conn, "1"), "a:1");

    let second = state_file(dir.path(), "b.toml", listen, b);
    cli(&data, &["apply", &second, "-m", "switch to b"]);
    wait_for(|| {
        let mut s = TcpStream::connect(("127.0.0.1", listen)).unwrap();
        s.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
        exchange(&mut s, "2") == "b:2"
    });
    // The connection opened before the change was not dropped.
    assert_eq!(exchange(&mut conn, "3"), "a:3");

    cli(&data, &["rollback", "1"]);
    wait_for(|| {
        let mut s = TcpStream::connect(("127.0.0.1", listen)).unwrap();
        s.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
        exchange(&mut s, "4") == "a:4"
    });

    let revisions = cli(&data, &["revisions"]);
    assert!(revisions.contains("Roll back to revision 1"), "{revisions}");
    drop(daemon);
}

#[test]
fn export_then_import_round_trips() {
    let dir = tempfile::tempdir().unwrap();
    let data = dir.path().join("data");
    let example = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../packaging/examples/state.toml"
    );
    cli(&data, &["validate", example]);
    cli(&data, &["apply", example]);
    let exported = dir.path().join("export.toml");
    cli(&data, &["export", "-o", exported.to_str().unwrap()]);
    let out = cli(&data, &["import", exported.to_str().unwrap()]);
    assert!(out.contains("No changes"), "{out}");
    assert_eq!(cli(&data, &["revisions"]).lines().count(), 1);
}
