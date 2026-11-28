use std::cell::{Cell, RefCell};
use std::collections::BTreeSet;
use std::path::Path;

use super::*;
use crate::verify::testkey::TestKey;

fn v(s: &str) -> Version {
    Version::parse(s).unwrap()
}

const X86: &str = "x86_64-unknown-linux-musl";

/// Pretends to be systemd and the package managers. The "binary" is a text
/// file naming its version; starting it writes a health report if that
/// version is in `healthy`.
struct Fake<'a> {
    root: &'a Path,
    layout: Layout,
    binary: &'static str,
    healthy: BTreeSet<String>,
    clock: Cell<u64>,
    calls: RefCell<Vec<String>>,
    running: Cell<bool>,
}

impl<'a> Fake<'a> {
    fn new(root: &'a Path, layout: Layout, binary: &'static str, healthy: &[&str]) -> Self {
        Self {
            root,
            layout,
            binary,
            healthy: healthy.iter().map(|s| (*s).to_owned()).collect(),
            clock: Cell::new(1_000),
            calls: RefCell::new(Vec::new()),
            running: Cell::new(true),
        }
    }

    fn installed(&self) -> String {
        let text = std::fs::read_to_string(self.root.join(self.binary.trim_start_matches('/'))).unwrap_or_default();
        text.trim().trim_start_matches("hearthgate ").to_owned()
    }

    fn called(&self, prefix: &str) -> bool {
        self.calls.borrow().iter().any(|c| c.starts_with(prefix))
    }
}

impl System for Fake<'_> {
    fn run(&self, program: &str, args: &[&str]) -> (bool, String) {
        let line = format!("{program} {}", args.join(" "));
        self.calls.borrow_mut().push(line.clone());
        match (program, args) {
            ("systemctl", ["stop", _]) => {
                self.running.set(false);
                (true, String::new())
            }
            ("systemctl", ["start", _]) => {
                self.running.set(true);
                let version = self.installed();
                if self.healthy.contains(&version) {
                    let h = Health {
                        version,
                        pid: 42,
                        started_unix: self.clock.get(),
                        ok: true,
                        problems: vec![],
                    };
                    write_json(&self.layout.health(), &h, 0o644).unwrap();
                }
                (true, String::new())
            }
            ("systemctl", ["is-active", _]) => {
                (self.running.get(), if self.running.get() { "active\n" } else { "inactive\n" }.into())
            }
            ("systemctl", ["daemon-reload"]) | ("chown", _) => (true, String::new()),
            ("apt-get", ["install", "-y", "--allow-downgrades", file]) => {
                // The fake package holds the binary's text.
                let data = std::fs::read(file).unwrap();
                let dest = self.root.join(self.binary.trim_start_matches('/'));
                std::fs::write(dest, data).unwrap();
                (true, "Setting up hearthgate".into())
            }
            _ => (false, format!("unexpected: {line}")),
        }
    }
    fn sleep(&self, d: Duration) {
        self.clock.set(self.clock.get() + d.as_secs());
    }
    fn now_unix(&self) -> u64 {
        self.clock.get()
    }
}

fn tarball(version: &str, extra: &[(&str, &[u8])]) -> Vec<u8> {
    let mut builder = tar::Builder::new(flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::fast()));
    let dir = format!("hearthgate-v{version}-{X86}");
    let bin = format!("hearthgate {version}\n");
    let mut files: Vec<(String, Vec<u8>)> = vec![
        (format!("{dir}/hearthgate"), bin.into_bytes()),
        (format!("{dir}/hearthgate.service"), format!("unit {version}\n").into_bytes()),
        (format!("{dir}/README.md"), b"readme".to_vec()),
    ];
    for (n, d) in extra {
        files.push(((*n).to_owned(), d.to_vec()));
    }
    for (name, data) in files {
        let mut h = tar::Header::new_gnu();
        h.set_size(data.len() as u64);
        h.set_mode(0o755);
        // Write the raw name so tests can add names `append_data` refuses.
        let raw = &mut h.as_old_mut().name;
        raw[..name.len()].copy_from_slice(name.as_bytes());
        h.set_cksum();
        builder.append(&h, data.as_slice()).unwrap();
    }
    builder.into_inner().unwrap().finish().unwrap()
}

struct Machine {
    _dir: tempfile::TempDir,
    root: PathBuf,
    layout: Layout,
    key: TestKey,
}

impl Machine {
    /// A tarball install of 1.2.0 with some settings.
    fn tarball() -> Self {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("root");
        let layout = Layout {
            data_dir: dir.path().join("data"),
            helper_dir: dir.path().join("helper"),
        };
        std::fs::create_dir_all(root.join("usr/local/bin")).unwrap();
        std::fs::create_dir_all(root.join("etc/systemd/system")).unwrap();
        std::fs::create_dir_all(layout.updates()).unwrap();
        std::fs::write(root.join("usr/local/bin/hearthgate"), "hearthgate 1.2.0\n").unwrap();
        std::fs::write(root.join("etc/systemd/system/hearthgate.service"), "unit 1.2.0\n").unwrap();
        std::fs::write(layout.data_dir.join("state.db"), "settings before").unwrap();
        Self { _dir: dir, root, layout, key: TestKey::new() }
    }

    /// Puts a signed release into the daemon's download directory.
    fn download(&self, version: &str, asset: &str, data: &[u8]) {
        let dir = self.layout.download_dir(v(version));
        std::fs::create_dir_all(&dir).unwrap();
        let (sums, sig) = self.key.release(version, &[(asset, data)]);
        std::fs::write(dir.join("SHA256SUMS"), sums).unwrap();
        std::fs::write(dir.join("SHA256SUMS.minisig"), sig).unwrap();
        std::fs::write(dir.join(asset), data).unwrap();
    }

    fn request(&self, r: &Request) {
        write_json(&self.layout.request(), r, 0o644).unwrap();
    }

    fn helper<'s>(&self, system: &'s dyn System, current: &str, method: Method) -> Helper<'s> {
        Helper {
            layout: self.layout.clone(),
            root: self.root.clone(),
            system,
            current: v(current),
            method,
            arch: Arch::from_name("x86_64").unwrap(),
            keys: vec![self.key.public()],
            health_timeout: Duration::from_secs(120),
        }
    }

    fn read(&self, path: &str) -> String {
        std::fs::read_to_string(self.root.join(path)).unwrap()
    }
}

const TARBALL_BIN: &str = "/usr/local/bin/hearthgate";

#[test]
fn installs_a_tarball_release() {
    let m = Machine::tarball();
    let asset = format!("hearthgate-v1.3.0-{X86}.tar.gz");
    m.download("1.3.0", &asset, &tarball("1.3.0", &[]));
    m.request(&Request::Install { version: v("1.3.0") });
    let fake = Fake::new(&m.root, m.layout.clone(), TARBALL_BIN, &["1.2.0", "1.3.0"]);
    let job = m.helper(&fake, "1.2.0", Method::Tarball).run_pending().unwrap();

    assert_eq!(job.outcome, Outcome::Installed, "{:#?}", job.log);
    assert!(job.steps.iter().all(|s| s.state == StepState::Done));
    assert_eq!(m.read("usr/local/bin/hearthgate"), "hearthgate 1.3.0\n");
    assert_eq!(m.read("etc/systemd/system/hearthgate.service"), "unit 1.3.0\n");
    // Nothing but the wanted files came out of the archive.
    assert!(!m.root.join("usr/local/bin/README.md").exists());
    // The old version and the settings are kept.
    let backup = m.layout.backup();
    assert_eq!(std::fs::read_to_string(backup.join("hearthgate")).unwrap(), "hearthgate 1.2.0\n");
    assert_eq!(std::fs::read_to_string(backup.join("data/state.db")).unwrap(), "settings before");
    // The request is gone, the status and history are written.
    assert!(!m.layout.request().exists());
    let history = crate::state::read_history(&m.layout.history());
    assert_eq!(history.len(), 1);
    assert_eq!(history[0].outcome, Outcome::Installed);
    let status: Job = read_json(&m.layout.status()).unwrap();
    assert_eq!(status.outcome, Outcome::Installed);
}

#[test]
fn refuses_a_release_signed_by_another_key() {
    let m = Machine::tarball();
    let asset = format!("hearthgate-v1.3.0-{X86}.tar.gz");
    m.download("1.3.0", &asset, &tarball("1.3.0", &[]));
    m.request(&Request::Install { version: v("1.3.0") });
    let fake = Fake::new(&m.root, m.layout.clone(), TARBALL_BIN, &["1.2.0", "1.3.0"]);
    let mut helper = m.helper(&fake, "1.2.0", Method::Tarball);
    helper.keys = vec![TestKey::new().public()];
    let job = helper.run_pending().unwrap();

    assert_eq!(job.outcome, Outcome::Refused);
    assert_eq!(job.steps[0].state, StepState::Failed);
    assert!(job.steps[1..].iter().all(|s| s.state == StepState::Skipped));
    assert!(!fake.called("systemctl stop"), "{:?}", fake.calls.borrow());
    assert_eq!(m.read("usr/local/bin/hearthgate"), "hearthgate 1.2.0\n");
}

#[test]
fn refuses_a_changed_download() {
    let m = Machine::tarball();
    let asset = format!("hearthgate-v1.3.0-{X86}.tar.gz");
    m.download("1.3.0", &asset, &tarball("1.3.0", &[]));
    // The daemon (or whoever took it over) swaps the archive afterwards.
    std::fs::write(m.layout.download_dir(v("1.3.0")).join(&asset), tarball("6.6.6", &[])).unwrap();
    m.request(&Request::Install { version: v("1.3.0") });
    let fake = Fake::new(&m.root, m.layout.clone(), TARBALL_BIN, &["1.2.0"]);
    let job = m.helper(&fake, "1.2.0", Method::Tarball).run_pending().unwrap();
    assert_eq!(job.outcome, Outcome::Refused);
    assert!(!fake.called("systemctl stop"));
}

#[test]
fn refuses_a_symlinked_download() {
    let m = Machine::tarball();
    let asset = format!("hearthgate-v1.3.0-{X86}.tar.gz");
    m.download("1.3.0", &asset, &tarball("1.3.0", &[]));
    let dir = m.layout.download_dir(v("1.3.0"));
    std::fs::remove_file(dir.join("SHA256SUMS")).unwrap();
    std::os::unix::fs::symlink("/etc/shadow", dir.join("SHA256SUMS")).unwrap();
    m.request(&Request::Install { version: v("1.3.0") });
    let fake = Fake::new(&m.root, m.layout.clone(), TARBALL_BIN, &["1.2.0"]);
    let job = m.helper(&fake, "1.2.0", Method::Tarball).run_pending().unwrap();
    assert_eq!(job.outcome, Outcome::Refused);
}

#[test]
fn refuses_older_and_manual() {
    let m = Machine::tarball();
    m.request(&Request::Install { version: v("1.1.0") });
    let fake = Fake::new(&m.root, m.layout.clone(), TARBALL_BIN, &["1.2.0"]);
    let job = m.helper(&fake, "1.2.0", Method::Tarball).run_pending().unwrap();
    assert_eq!(job.outcome, Outcome::Refused);
    assert!(job.detail.unwrap().contains("not newer"));

    m.request(&Request::Install { version: v("1.3.0") });
    let job = m.helper(&fake, "1.2.0", Method::Manual).run_pending().unwrap();
    assert_eq!(job.outcome, Outcome::Refused);
    assert!(fake.calls.borrow().is_empty());
}

#[test]
fn no_request_does_nothing_and_junk_is_refused() {
    let m = Machine::tarball();
    let fake = Fake::new(&m.root, m.layout.clone(), TARBALL_BIN, &["1.2.0"]);
    assert!(m.helper(&fake, "1.2.0", Method::Tarball).run_pending().is_none());
    std::fs::write(m.layout.request(), r#"{"action":"rm -rf /"}"#).unwrap();
    let job = m.helper(&fake, "1.2.0", Method::Tarball).run_pending().unwrap();
    assert_eq!(job.outcome, Outcome::Refused);
    assert!(!m.layout.request().exists());
}

#[test]
fn archive_entries_outside_the_release_are_ignored() {
    let m = Machine::tarball();
    let asset = format!("hearthgate-v1.3.0-{X86}.tar.gz");
    let evil: &[(&str, &[u8])] = &[
        ("../../etc/passwd", b"owned"),
        ("hearthgate-v1.3.0-x86_64-unknown-linux-musl/../../evil", b"owned"),
        ("/etc/cron.d/evil", b"owned"),
    ];
    m.download("1.3.0", &asset, &tarball("1.3.0", evil));
    m.request(&Request::Install { version: v("1.3.0") });
    let fake = Fake::new(&m.root, m.layout.clone(), TARBALL_BIN, &["1.2.0", "1.3.0"]);
    let job = m.helper(&fake, "1.2.0", Method::Tarball).run_pending().unwrap();
    assert_eq!(job.outcome, Outcome::Installed, "{:#?}", job.log);
    assert!(!m.root.join("etc/passwd").exists());
    assert!(!m.root.join("etc/cron.d/evil").exists());
    assert!(!m.root.parent().unwrap().join("evil").exists());
}

#[test]
fn rolls_back_when_the_new_version_is_not_healthy() {
    let m = Machine::tarball();
    let asset = format!("hearthgate-v1.3.0-{X86}.tar.gz");
    m.download("1.3.0", &asset, &tarball("1.3.0", &[]));
    m.request(&Request::Install { version: v("1.3.0") });
    // 1.3.0 never reports; 1.2.0 does.
    let fake = Fake::new(&m.root, m.layout.clone(), TARBALL_BIN, &["1.2.0"]);
    let job = m.helper(&fake, "1.2.0", Method::Tarball).run_pending().unwrap();

    assert_eq!(job.outcome, Outcome::RolledBack, "{:#?}", job.log);
    assert_eq!(job.steps[4].state, StepState::Failed);
    assert!(job.detail.as_deref().unwrap().contains("1.2.0 was restored"));
    assert_eq!(m.read("usr/local/bin/hearthgate"), "hearthgate 1.2.0\n");
    assert_eq!(m.read("etc/systemd/system/hearthgate.service"), "unit 1.2.0\n");
    assert_eq!(std::fs::read_to_string(m.layout.data_dir.join("state.db")).unwrap(), "settings before");
    // It waited the full two minutes, and not much more.
    assert!(fake.now_unix() >= 1_000 + 120 && fake.now_unix() < 1_000 + 200);
}

#[test]
fn reports_problems_from_the_health_report() {
    struct Sick<'a>(Fake<'a>);
    impl System for Sick<'_> {
        fn run(&self, program: &str, args: &[&str]) -> (bool, String) {
            let r = self.0.run(program, args);
            if args.first() == Some(&"start") && self.0.installed() == "1.3.0" {
                let h = Health {
                    version: "1.3.0".into(),
                    pid: 1,
                    started_unix: self.0.now_unix(),
                    ok: false,
                    problems: vec!["port 443 did not bind".into()],
                };
                write_json(&self.0.layout.health(), &h, 0o644).unwrap();
            }
            r
        }
        fn sleep(&self, d: Duration) {
            self.0.sleep(d);
        }
        fn now_unix(&self) -> u64 {
            self.0.now_unix()
        }
    }
    let m = Machine::tarball();
    let asset = format!("hearthgate-v1.3.0-{X86}.tar.gz");
    m.download("1.3.0", &asset, &tarball("1.3.0", &[]));
    m.request(&Request::Install { version: v("1.3.0") });
    let sick = Sick(Fake::new(&m.root, m.layout.clone(), TARBALL_BIN, &["1.2.0"]));
    let job = m.helper(&sick, "1.2.0", Method::Tarball).run_pending().unwrap();
    assert_eq!(job.outcome, Outcome::RolledBack);
    assert!(job.detail.unwrap().contains("port 443 did not bind"));
    // It gave up at once instead of waiting.
    assert!(sick.now_unix() < 1_000 + 60);
}

#[test]
fn manual_rollback_keeps_current_settings() {
    let m = Machine::tarball();
    let asset = format!("hearthgate-v1.3.0-{X86}.tar.gz");
    m.download("1.3.0", &asset, &tarball("1.3.0", &[]));
    m.request(&Request::Install { version: v("1.3.0") });
    let fake = Fake::new(&m.root, m.layout.clone(), TARBALL_BIN, &["1.2.0", "1.3.0"]);
    assert_eq!(m.helper(&fake, "1.2.0", Method::Tarball).run_pending().unwrap().outcome, Outcome::Installed);

    std::fs::write(m.layout.data_dir.join("state.db"), "settings after").unwrap();
    m.request(&Request::Rollback);
    let job = m.helper(&fake, "1.3.0", Method::Tarball).run_pending().unwrap();
    assert_eq!(job.outcome, Outcome::Restored, "{:#?}", job.log);
    assert_eq!(job.to, Some(v("1.2.0")));
    assert_eq!(m.read("usr/local/bin/hearthgate"), "hearthgate 1.2.0\n");
    assert_eq!(std::fs::read_to_string(m.layout.data_dir.join("state.db")).unwrap(), "settings after");

    // The backup is used up: a second rollback has nothing to go back to.
    m.request(&Request::Rollback);
    let job = m.helper(&fake, "1.2.0", Method::Tarball).run_pending().unwrap();
    assert_eq!(job.outcome, Outcome::Refused);
    assert_eq!(crate::state::read_history(&m.layout.history()).len(), 3);
}

#[test]
fn rollback_with_no_backup_is_refused() {
    let m = Machine::tarball();
    m.request(&Request::Rollback);
    let fake = Fake::new(&m.root, m.layout.clone(), TARBALL_BIN, &["1.2.0"]);
    let job = m.helper(&fake, "1.2.0", Method::Tarball).run_pending().unwrap();
    assert_eq!(job.outcome, Outcome::Refused);
    assert!(fake.calls.borrow().is_empty());
}

#[test]
fn installs_a_deb_and_rolls_back_through_apt() {
    let m = Machine::tarball();
    std::fs::create_dir_all(m.root.join("usr/bin")).unwrap();
    std::fs::write(m.root.join("usr/bin/hearthgate"), "hearthgate 1.2.0\n").unwrap();
    let fake = Fake::new(&m.root, m.layout.clone(), "/usr/bin/hearthgate", &["1.2.0", "1.3.0", "1.4.0"]);

    m.download("1.3.0", "hearthgate_1.3.0_amd64.deb", b"hearthgate 1.3.0\n");
    m.request(&Request::Install { version: v("1.3.0") });
    let job = m.helper(&fake, "1.2.0", Method::Deb).run_pending().unwrap();
    assert_eq!(job.outcome, Outcome::Installed, "{:#?}", job.log);
    assert_eq!(m.read("usr/bin/hearthgate"), "hearthgate 1.3.0\n");
    // apt got the helper's verified copy, never the daemon's file.
    let work = m.layout.work().join("hearthgate_1.3.0_amd64.deb");
    assert!(fake.called(&format!("apt-get install -y --allow-downgrades {}", work.display())));
    assert!(m.layout.helper_dir.join("packages/hearthgate_1.3.0_amd64.deb").is_file());

    // The next update keeps 1.3.0's package, so going back uses apt.
    m.download("1.4.0", "hearthgate_1.4.0_amd64.deb", b"hearthgate 1.4.0\n");
    m.request(&Request::Install { version: v("1.4.0") });
    let job = m.helper(&fake, "1.3.0", Method::Deb).run_pending().unwrap();
    assert_eq!(job.outcome, Outcome::Installed);
    let backup: Backup = read_json(&m.layout.backup().join("backup.json")).unwrap();
    assert_eq!(backup.package.as_deref(), Some("hearthgate_1.3.0_amd64.deb"));

    fake.calls.borrow_mut().clear();
    m.request(&Request::Rollback);
    let job = m.helper(&fake, "1.4.0", Method::Deb).run_pending().unwrap();
    assert_eq!(job.outcome, Outcome::Restored, "{:#?}", job.log);
    assert_eq!(m.read("usr/bin/hearthgate"), "hearthgate 1.3.0\n");
    assert!(fake.called("apt-get install"));
}
