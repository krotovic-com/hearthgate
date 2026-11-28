//! The root helper: `hearthgate upgrade apply`.
//!
//! Started by `hearthgate-upgrade.path` when the daemon writes a request.
//! It runs as root with no network, so it never downloads anything: it
//! copies the daemon's downloads into its own directory, verifies them from
//! scratch against the keys compiled into this binary, and only then stops
//! Hearthgate, backs up the binary, its unit and the state database,
//! installs, starts the new version and waits for it to report healthy. If
//! it doesn't within the time limit, everything from the backup goes back.

use std::io::Read;
use std::path::{Path, PathBuf};
use std::time::Duration;

use minisign_verify::PublicKey;

use crate::files::{copy_limited, ensure_dir, read_json, read_limited, write_atomic, write_json};
use crate::method::{Arch, Method, pick_asset};
use crate::state::{Action, Backup, HISTORY, Health, Job, Layout, Outcome, Request, Step, StepState};
use crate::verify::{MAX_SIGNATURE_BYTES, MAX_SUMS_BYTES, verify_sums};
use crate::{Version, github};

/// Largest release asset accepted.
pub const MAX_ASSET_BYTES: u64 = 64 * 1024 * 1024;
/// Largest file taken out of a release archive.
const MAX_ENTRY_BYTES: u64 = 120 * 1024 * 1024;

/// How the helper reaches the system. [`Host`] is the real one.
pub trait System {
    /// Runs a program and returns its exit status and output.
    fn run(&self, program: &str, args: &[&str]) -> (bool, String);
    fn sleep(&self, d: Duration);
    fn now_unix(&self) -> u64;
}

/// The real system.
#[derive(Debug, Default)]
pub struct Host;

impl System for Host {
    fn run(&self, program: &str, args: &[&str]) -> (bool, String) {
        match std::process::Command::new(program)
            .args(args)
            .env("PATH", "/usr/local/sbin:/usr/local/bin:/usr/sbin:/usr/bin:/sbin:/bin")
            .env("DEBIAN_FRONTEND", "noninteractive")
            .stdin(std::process::Stdio::null())
            .output()
        {
            Ok(out) => {
                let mut text = String::from_utf8_lossy(&out.stdout).into_owned();
                text.push_str(&String::from_utf8_lossy(&out.stderr));
                (out.status.success(), text)
            }
            Err(e) => (false, format!("cannot run {program}: {e}")),
        }
    }
    fn sleep(&self, d: Duration) {
        std::thread::sleep(d);
    }
    fn now_unix(&self) -> u64 {
        crate::state::now_unix()
    }
}

/// What the helper needs to know about this machine.
pub struct Helper<'a> {
    pub layout: Layout,
    /// `/`, or a temporary directory in tests.
    pub root: PathBuf,
    pub system: &'a dyn System,
    /// The version installed now (the helper's own).
    pub current: Version,
    pub method: Method,
    pub arch: Arch,
    pub keys: Vec<PublicKey>,
    /// How long the new version gets to report healthy.
    pub health_timeout: Duration,
}

const SERVICE: &str = "hearthgate";

impl Helper<'_> {
    fn at(&self, path: &str) -> PathBuf {
        self.root.join(path.trim_start_matches('/'))
    }

    /// Takes the pending request, if any, and carries it out. Returns the
    /// finished job.
    #[must_use]
    pub fn run_pending(&self) -> Option<Job> {
        let request_path = self.layout.request();
        let request: Option<Request> = read_json(&request_path);
        let present = std::fs::symlink_metadata(&request_path).is_ok();
        let _ = std::fs::remove_file(&request_path);
        if let Err(e) = ensure_dir(&self.layout.helper_dir, 0o755)
            .and_then(|()| ensure_dir(&self.layout.backup(), 0o700))
            .and_then(|()| ensure_dir(&self.layout.work(), 0o700))
        {
            eprintln!("cannot prepare {}: {e}", self.layout.helper_dir.display());
            return None;
        }
        let job = match request {
            Some(Request::Install { version }) => self.install(version),
            Some(Request::Rollback) => self.rollback(),
            None if present => {
                let mut job = Job::new(Action::Install, self.current, None, &[], self.system.now_unix());
                job.finish(Outcome::Refused, "The request was not readable, so nothing was changed.", self.system.now_unix());
                job
            }
            None => return None,
        };
        self.save(&job, true);
        Some(job)
    }

    fn save(&self, job: &Job, done: bool) {
        let _ = write_json(&self.layout.status(), job, 0o644);
        if done {
            let mut history = crate::state::read_history(&self.layout.history());
            history.insert(0, job.clone());
            history.truncate(HISTORY);
            let _ = write_json(&self.layout.history(), &history, 0o644);
        }
    }

    fn step(&self, job: &mut Job, i: usize, state: StepState, detail: Option<String>) {
        job.steps[i].state = state;
        if let Some(d) = detail {
            job.log(self.system.now_unix(), &format!("{}: {d}", job.steps[i].title));
            job.steps[i].detail = d;
        }
        self.save(job, false);
    }

    fn sh(&self, job: &mut Job, program: &str, args: &[&str]) -> Result<String, String> {
        let (ok, out) = self.system.run(program, args);
        let line = format!("$ {program} {}", args.join(" "));
        job.log(self.system.now_unix(), &line);
        for l in out.lines().filter(|l| !l.trim().is_empty()).take(40) {
            job.log(self.system.now_unix(), &format!("  {l}"));
        }
        if ok {
            Ok(out)
        } else {
            Err(format!("{program} failed: {}", out.lines().last().unwrap_or("no output").trim()))
        }
    }

    // ----- install ---------------------------------------------------------

    fn install(&self, version: Version) -> Job {
        let now = self.system.now_unix();
        let steps = [
            ("Verify signature and checksum", "Checking the downloaded release again"),
            ("Back up settings and the current version", "Waiting"),
            ("Install the new version", self.method.describe()),
            ("Restart Hearthgate", "Public traffic pauses for a few seconds"),
            ("Health check", "Proxy, firewall, VPN and dashboard must come up within 2 minutes"),
        ];
        let mut job = Job::new(Action::Install, self.current, Some(version), &steps, now);
        self.save(&job, false);

        // 1. Verify, before anything changes.
        self.step(&mut job, 0, StepState::Running, None);
        let asset = match self.verify(version) {
            Ok(asset) => asset,
            Err(e) => {
                self.step(&mut job, 0, StepState::Failed, Some(e.clone()));
                job.skip_pending();
                job.finish(Outcome::Refused, &format!("{e}. Nothing was changed."), self.system.now_unix());
                return job;
            }
        };
        self.step(&mut job, 0, StepState::Done, Some(format!("{asset} · signature and SHA-256 match")));

        // 2. Stop and back up.
        self.step(&mut job, 1, StepState::Running, None);
        if let Err(e) = self.sh(&mut job, "systemctl", &["stop", SERVICE]) {
            self.step(&mut job, 1, StepState::Failed, Some(e.clone()));
            job.skip_pending();
            let _ = self.sh(&mut job, "systemctl", &["start", SERVICE]);
            job.finish(Outcome::Refused, &format!("{e}. Nothing was changed."), self.system.now_unix());
            return job;
        }
        if let Err(e) = self.backup() {
            self.step(&mut job, 1, StepState::Failed, Some(e.clone()));
            job.skip_pending();
            let _ = self.sh(&mut job, "systemctl", &["start", SERVICE]);
            job.finish(Outcome::Refused, &format!("The backup failed ({e}), so nothing was changed."), self.system.now_unix());
            return job;
        }
        self.step(
            &mut job,
            1,
            StepState::Done,
            Some(format!("Settings and {} saved to {}", self.current, self.layout.backup().display())),
        );

        // 3. Install.
        self.step(&mut job, 2, StepState::Running, None);
        if let Err(e) = self.put_in_place(&mut job, &asset, version) {
            self.step(&mut job, 2, StepState::Failed, Some(e.clone()));
            job.skip_pending();
            return self.recover(job, &format!("Installing {version} failed: {e}"));
        }
        self.step(&mut job, 2, StepState::Done, Some(format!("{version} installed · {}", self.method.describe())));

        // 4. Start.
        self.step(&mut job, 3, StepState::Running, None);
        let started = self.system.now_unix();
        if let Err(e) = self.sh(&mut job, "systemctl", &["start", SERVICE]) {
            self.step(&mut job, 3, StepState::Failed, Some(e.clone()));
            job.skip_pending();
            return self.recover(job, &format!("{version} did not start: {e}"));
        }
        self.step(&mut job, 3, StepState::Done, Some("Started".into()));

        // 5. Health.
        self.step(&mut job, 4, StepState::Running, None);
        match self.wait_healthy(&mut job, &version.to_string(), started, true) {
            Ok(()) => {
                self.step(&mut job, 4, StepState::Done, Some("Everything came up".into()));
                self.keep_package(&asset, version);
                job.finish(
                    Outcome::Installed,
                    &format!("Hearthgate {version} is running. {} is kept for a rollback.", self.current),
                    self.system.now_unix(),
                );
                job
            }
            Err(e) => {
                self.step(&mut job, 4, StepState::Failed, Some(e.clone()));
                self.recover(job, &format!("{version} failed its health check: {e}"))
            }
        }
    }

    /// Copies the download out of the daemon's reach and verifies the copy.
    fn verify(&self, version: Version) -> Result<String, String> {
        if version <= self.current {
            return Err(format!("{version} is not newer than the installed {}", self.current));
        }
        if self.method == Method::Manual {
            return Err("this Hearthgate was not installed from a package or install.sh".into());
        }
        let from = self.layout.download_dir(version);
        let work = self.layout.work();
        let _ = std::fs::remove_dir_all(&work);
        ensure_dir(&work, 0o700).map_err(|e| e.to_string())?;
        let copy = |name: &str, max: u64| {
            copy_limited(&from.join(name), &work.join(name), max, 0o600)
                .map_err(|e| format!("cannot read the downloaded {name}: {e}"))
        };
        copy("SHA256SUMS", MAX_SUMS_BYTES as u64)?;
        copy("SHA256SUMS.minisig", MAX_SIGNATURE_BYTES as u64)?;
        let sums = read_limited(&work.join("SHA256SUMS"), MAX_SUMS_BYTES as u64).map_err(|e| e.to_string())?;
        let sig = read_limited(&work.join("SHA256SUMS.minisig"), MAX_SIGNATURE_BYTES as u64)
            .map_err(|e| e.to_string())?;
        let sig = String::from_utf8(sig).map_err(|_| "the signature is not text".to_owned())?;
        let checked = verify_sums(&sums, &sig, version, &self.keys).map_err(|e| e.to_string())?;
        let asset = pick_asset(checked.names(), self.method, version, self.arch)
            .ok_or_else(|| format!("the release has no {} for this machine", self.method.describe()))?
            .to_owned();
        copy(&asset, MAX_ASSET_BYTES)?;
        checked.check_file(&asset, &work.join(&asset)).map_err(|e| e.to_string())?;
        Ok(asset)
    }

    fn unit_paths(&self) -> Vec<&'static str> {
        match self.method {
            Method::Tarball => vec!["/etc/systemd/system/hearthgate.service"],
            _ => vec!["/usr/lib/systemd/system/hearthgate.service", "/lib/systemd/system/hearthgate.service"],
        }
    }

    fn db_files(&self) -> Vec<PathBuf> {
        ["state.db", "state.db-wal", "state.db-shm"]
            .iter()
            .map(|n| self.layout.data_dir.join(n))
            .collect()
    }

    fn backup(&self) -> Result<(), String> {
        let dir = self.layout.backup();
        let _ = std::fs::remove_dir_all(&dir);
        ensure_dir(&dir, 0o700).map_err(|e| e.to_string())?;
        let binary = self.method.binary().ok_or("no binary to back up")?;
        copy_limited(&self.at(binary), &dir.join("hearthgate"), MAX_ENTRY_BYTES, 0o755)
            .map_err(|e| format!("{binary}: {e}"))?;
        for unit in self.unit_paths() {
            if self.at(unit).is_file() {
                copy_limited(&self.at(unit), &dir.join("hearthgate.service"), 1 << 20, 0o644)
                    .map_err(|e| format!("{unit}: {e}"))?;
                break;
            }
        }
        ensure_dir(&dir.join("data"), 0o700).map_err(|e| e.to_string())?;
        for db in self.db_files() {
            if db.is_file() {
                let name = db.file_name().unwrap_or_default();
                copy_limited(&db, &dir.join("data").join(name), 1 << 32, 0o600)
                    .map_err(|e| format!("{}: {e}", db.display()))?;
            }
        }
        let package = self.installed_package(self.current);
        if let Some(p) = &package {
            copy_limited(&self.layout.helper_dir.join("packages").join(p), &dir.join(p), MAX_ASSET_BYTES, 0o600)
                .map_err(|e| e.to_string())?;
        }
        let meta = Backup {
            version: self.current,
            method: self.method,
            package,
            taken_unix: self.system.now_unix(),
        };
        write_json(&dir.join("backup.json"), &meta, 0o600).map_err(|e| e.to_string())?;
        write_json(&self.layout.kept(), &meta, 0o644).map_err(|e| e.to_string())
    }

    fn drop_backup(&self) {
        let _ = std::fs::remove_file(self.layout.backup().join("backup.json"));
        let _ = std::fs::remove_file(self.layout.kept());
    }

    /// The package file of `version` that the helper installed earlier.
    fn installed_package(&self, version: Version) -> Option<String> {
        let dir = self.layout.helper_dir.join("packages");
        let names: Vec<String> = std::fs::read_dir(dir)
            .ok()?
            .filter_map(Result::ok)
            .filter_map(|e| e.file_name().into_string().ok())
            .collect();
        pick_asset(names.iter().map(String::as_str), self.method, version, self.arch)
            .filter(|_| matches!(self.method, Method::Deb | Method::Rpm))
            .map(str::to_owned)
    }

    fn keep_package(&self, asset: &str, version: Version) {
        if !matches!(self.method, Method::Deb | Method::Rpm) {
            return;
        }
        let dir = self.layout.helper_dir.join("packages");
        if ensure_dir(&dir, 0o700).is_err() {
            return;
        }
        if let Ok(entries) = std::fs::read_dir(&dir) {
            for e in entries.filter_map(Result::ok) {
                let _ = std::fs::remove_file(e.path());
            }
        }
        let _ = std::fs::rename(self.layout.work().join(asset), dir.join(asset));
        let _ = version;
    }

    fn put_in_place(&self, job: &mut Job, asset: &str, version: Version) -> Result<(), String> {
        let file = self.layout.work().join(asset);
        let file = file.to_str().ok_or("bad path")?.to_owned();
        match self.method {
            Method::Deb => {
                self.sh(job, "apt-get", &["install", "-y", "--allow-downgrades", &file])?;
            }
            Method::Rpm => self.rpm_install(job, &file)?,
            Method::Tarball => self.unpack_tarball(&PathBuf::from(&file), version)?,
            Method::Manual => return Err("manual installs are not upgraded".into()),
        }
        self.sh(job, "systemctl", &["daemon-reload"])?;
        Ok(())
    }

    fn rpm_install(&self, job: &mut Job, file: &str) -> Result<(), String> {
        // The helper has no network: only the local file, no repositories.
        if self.at("/usr/bin/dnf").exists() {
            self.sh(job, "dnf", &["install", "-y", "--disablerepo=*", "--allowerasing", file]).map(drop)
        } else if self.at("/usr/bin/zypper").exists() {
            self.sh(
                job,
                "zypper",
                &["--non-interactive", "--no-refresh", "install", "--allow-unsigned-rpm", "--oldpackage", file],
            )
            .map(drop)
        } else {
            self.sh(job, "yum", &["install", "-y", "--disablerepo=*", file]).map(drop)
        }
    }

    /// Takes the binary and units out of a release archive and puts them in
    /// place. Only these exact files are read; nothing else is unpacked.
    fn unpack_tarball(&self, archive: &Path, version: Version) -> Result<(), String> {
        let prefix = format!("hearthgate-v{version}-{}/", self.arch.target);
        let wanted = [
            ("hearthgate", "/usr/local/bin/hearthgate", 0o755),
            ("hearthgate.service", "/etc/systemd/system/hearthgate.service", 0o644),
            ("hearthgate-upgrade.service", "/etc/systemd/system/hearthgate-upgrade.service", 0o644),
            ("hearthgate-upgrade.path", "/etc/systemd/system/hearthgate-upgrade.path", 0o644),
        ];
        let file = crate::files::open_nofollow(archive).map_err(|e| e.to_string())?;
        let mut tar = tar::Archive::new(flate2::read::GzDecoder::new(file));
        let mut found: Vec<(Vec<u8>, &str, u32)> = Vec::new();
        for entry in tar.entries().map_err(|e| format!("bad archive: {e}"))? {
            let mut entry = entry.map_err(|e| format!("bad archive: {e}"))?;
            if !entry.header().entry_type().is_file() {
                continue;
            }
            let path = entry.path().map_err(|e| e.to_string())?.to_string_lossy().into_owned();
            let Some(name) = path.strip_prefix(&prefix) else { continue };
            let Some((_, dest, mode)) = wanted.iter().find(|(n, _, _)| *n == name) else { continue };
            let mut data = Vec::new();
            (&mut entry).take(MAX_ENTRY_BYTES + 1).read_to_end(&mut data).map_err(|e| e.to_string())?;
            if data.len() as u64 > MAX_ENTRY_BYTES {
                return Err(format!("{name} in the archive is too large"));
            }
            found.push((data, dest, *mode));
        }
        if !found.iter().any(|(_, d, _)| *d == "/usr/local/bin/hearthgate") {
            return Err("the archive has no hearthgate binary".into());
        }
        for (data, dest, mode) in found {
            let target = self.at(dest);
            if let Some(dir) = target.parent() {
                std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
            }
            write_atomic(&target, &data, mode).map_err(|e| format!("{dest}: {e}"))?;
        }
        Ok(())
    }

    /// Waits until `version` reports healthy after `since`.
    fn wait_healthy(&self, job: &mut Job, version: &str, since: u64, strict: bool) -> Result<(), String> {
        let deadline = self.system.now_unix() + self.health_timeout.as_secs();
        let mut active_since: Option<u64> = None;
        loop {
            let (_, state) = self.system.run("systemctl", &["is-active", SERVICE]);
            let state = state.trim().to_owned();
            if state == "failed" {
                return Err("the service stopped with an error".into());
            }
            let now = self.system.now_unix();
            if state == "active" {
                active_since.get_or_insert(now);
            } else {
                active_since = None;
            }
            if let Some(h) = read_json::<Health>(&self.layout.health())
                && h.version == version
                && h.started_unix >= since
            {
                if h.ok {
                    job.log(now, "health: ok");
                    return Ok(());
                }
                return Err(h.problems.join("; "));
            }
            // An older version that doesn't write a health report counts as
            // healthy after 20 seconds of running.
            if !strict && active_since.is_some_and(|t| now >= t + 20) {
                return Ok(());
            }
            if now >= deadline {
                return Err(format!(
                    "no health report within {} seconds",
                    self.health_timeout.as_secs()
                ));
            }
            self.system.sleep(Duration::from_secs(2));
        }
    }

    // ----- rollback --------------------------------------------------------

    /// After a failed install: put everything from the backup back.
    fn recover(&self, mut job: Job, why: &str) -> Job {
        job.log(self.system.now_unix(), why);
        match self.restore(&mut job, true) {
            Ok(()) => {
                // What runs now is the backup; nothing older is kept.
                self.drop_backup();
                job.finish(
                    Outcome::RolledBack,
                    &format!("{why}. {} was restored and runs again.", self.current),
                    self.system.now_unix(),
                );
            }
            Err(e) => {
                job.finish(
                    Outcome::Failed,
                    &format!("{why}, and restoring {} failed too: {e}. See the log.", self.current),
                    self.system.now_unix(),
                );
            }
        }
        job
    }

    fn read_backup(&self) -> Result<Backup, String> {
        read_json(&self.layout.backup().join("backup.json")).ok_or_else(|| "there is no backup to go back to".to_owned())
    }

    /// Puts the backup in place and starts it. `with_data` restores the
    /// state database too.
    fn restore(&self, job: &mut Job, with_data: bool) -> Result<(), String> {
        let backup = self.read_backup()?;
        let dir = self.layout.backup();
        let _ = self.sh(job, "systemctl", &["stop", SERVICE]);
        let binary = backup.method.binary().ok_or("unknown install method")?;
        let mut via_package = false;
        if let Some(p) = &backup.package {
            let file = dir.join(p);
            let file = file.to_str().unwrap_or_default().to_owned();
            via_package = match backup.method {
                Method::Deb => self.sh(job, "apt-get", &["install", "-y", "--allow-downgrades", &file]).is_ok(),
                Method::Rpm => self.rpm_install(job, &file).is_ok(),
                _ => false,
            };
        }
        if !via_package {
            copy_limited(&dir.join("hearthgate"), &self.at(binary), MAX_ENTRY_BYTES, 0o755)
                .map_err(|e| format!("{binary}: {e}"))?;
            if dir.join("hearthgate.service").is_file() {
                let unit = match backup.method {
                    Method::Tarball => "/etc/systemd/system/hearthgate.service",
                    _ => self
                        .unit_paths()
                        .into_iter()
                        .find(|u| self.at(u).is_file())
                        .unwrap_or("/usr/lib/systemd/system/hearthgate.service"),
                };
                copy_limited(&dir.join("hearthgate.service"), &self.at(unit), 1 << 20, 0o644)
                    .map_err(|e| format!("{unit}: {e}"))?;
            }
        }
        if with_data {
            self.restore_data()?;
        }
        self.sh(job, "systemctl", &["daemon-reload"])?;
        let since = self.system.now_unix();
        self.sh(job, "systemctl", &["start", SERVICE])?;
        self.wait_healthy(job, &backup.version.to_string(), since, false)
    }

    fn restore_data(&self) -> Result<(), String> {
        let saved = self.layout.backup().join("data");
        for db in self.db_files() {
            let name = db.file_name().unwrap_or_default();
            let from = saved.join(name);
            let _ = std::fs::remove_file(&db);
            if from.is_file() {
                copy_limited(&from, &db, 1 << 32, 0o600).map_err(|e| format!("{}: {e}", db.display()))?;
                // The daemon runs as its own user and must own its files.
                let path = db.to_str().unwrap_or_default().to_owned();
                let _ = self.system.run("chown", &["hearthgate:hearthgate", &path]);
            }
        }
        Ok(())
    }

    /// A rollback the person asked for, maybe days after the update.
    fn rollback(&self) -> Job {
        let now = self.system.now_unix();
        let backup = self.read_backup();
        let to = backup.as_ref().ok().map(|b| b.version);
        let steps = [
            ("Put the previous version back", "Waiting"),
            ("Restart Hearthgate", "Public traffic pauses for a few seconds"),
            ("Health check", "Waiting for it to come up"),
        ];
        let mut job = Job::new(Action::Rollback, self.current, to, &steps, now);
        self.save(&job, false);
        let Ok(backup) = backup else {
            self.step(&mut job, 0, StepState::Failed, Some("There is no backup to go back to".into()));
            job.skip_pending();
            job.finish(Outcome::Refused, "There is no previous version kept, so nothing was changed.", now);
            return job;
        };
        if backup.version >= self.current {
            self.step(&mut job, 0, StepState::Failed, Some(format!("The kept version is {}", backup.version)));
            job.skip_pending();
            job.finish(Outcome::Refused, "The kept version is not older, so nothing was changed.", now);
            return job;
        }
        self.step(&mut job, 0, StepState::Running, None);
        // Keep the settings as they are now; only if the old version can't
        // use them, go back to the ones from before the update.
        match self.restore(&mut job, false) {
            Ok(()) => {
                self.step(&mut job, 0, StepState::Done, Some(format!("{} is back", backup.version)));
                self.step(&mut job, 1, StepState::Done, Some("Started".into()));
                self.step(&mut job, 2, StepState::Done, Some("It came up with your current settings".into()));
                self.drop_backup();
                job.finish(
                    Outcome::Restored,
                    &format!("Hearthgate {} is running again, with your current settings.", backup.version),
                    self.system.now_unix(),
                );
            }
            Err(first) => {
                job.log(self.system.now_unix(), &format!("with current settings: {first}"));
                match self.restore(&mut job, true) {
                    Ok(()) => {
                        self.step(&mut job, 0, StepState::Done, Some(format!("{} is back", backup.version)));
                        self.step(&mut job, 1, StepState::Done, Some("Started".into()));
                        self.step(
                            &mut job,
                            2,
                            StepState::Done,
                            Some("It needed the settings from before the update".into()),
                        );
                        self.drop_backup();
                        job.finish(
                            Outcome::Restored,
                            &format!(
                                "Hearthgate {} is running again. It could not read the newer settings, so the ones from before the update are back; later changes are in the revision history of the newer version only.",
                                backup.version
                            ),
                            self.system.now_unix(),
                        );
                    }
                    Err(e) => {
                        self.step(&mut job, 2, StepState::Failed, Some(e.clone()));
                        job.finish(Outcome::Failed, &format!("The rollback failed: {e}. See the log."), self.system.now_unix());
                    }
                }
            }
        }
        job
    }
}

impl Job {
    fn new(action: Action, from: Version, to: Option<Version>, steps: &[(&str, &str)], now: u64) -> Self {
        Self {
            action,
            from,
            to,
            steps: steps
                .iter()
                .map(|(t, d)| Step {
                    title: (*t).to_owned(),
                    detail: (*d).to_owned(),
                    state: StepState::Pending,
                })
                .collect(),
            outcome: Outcome::Running,
            detail: None,
            started_unix: now,
            finished_unix: None,
            log: Vec::new(),
        }
    }

    fn log(&mut self, now: u64, line: &str) {
        if self.log.len() < 2000 {
            self.log.push(format!("[{now}] {line}"));
        }
    }

    fn skip_pending(&mut self) {
        for s in &mut self.steps {
            if s.state == StepState::Pending {
                s.state = StepState::Skipped;
            }
        }
    }

    fn finish(&mut self, outcome: Outcome, detail: &str, now: u64) {
        self.outcome = outcome;
        self.detail = Some(detail.to_owned());
        self.finished_unix = Some(now);
        self.log(now, detail);
    }
}

/// The page for a release, for log lines.
#[must_use]
pub fn release_page(version: Version) -> String {
    github::release_page(version)
}

#[cfg(test)]
mod tests;
