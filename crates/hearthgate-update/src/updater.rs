//! The daemon's half: checks GitHub, downloads and verifies the release this
//! machine needs, and asks the root helper to install it.

use std::collections::BTreeSet;
use std::sync::{Arc, Mutex, PoisonError, RwLock};
use std::time::Duration;

use hearthgate_model::Updates;
use minisign_verify::PublicKey;
use serde::{Deserialize, Serialize};
use tokio_util::sync::CancellationToken;
use tracing::{debug, info, warn};

use crate::apply::MAX_ASSET_BYTES;
use crate::fetch::Fetch;
use crate::files::{ensure_dir, read_json, write_atomic, write_json};
use crate::method::{Arch, Method, pick_asset};
use crate::notes::{Note, NoteKind};
use crate::state::{Backup, Job, Layout, Outcome, Request, now_unix, read_history};
use crate::verify::{MAX_SIGNATURE_BYTES, MAX_SUMS_BYTES, verify_sums};
use crate::{Bump, Version, github, policy};

/// How often GitHub is asked.
const CHECK_EVERY: u64 = 24 * 3600;
/// A job that says "running" for longer than this is taken as dead.
const JOB_STALE: u64 = 30 * 60;

/// A newer release.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct Available {
    #[cfg_attr(feature = "openapi", schema(value_type = String))]
    pub version: Version,
    pub published_at: String,
    pub bump: Bump,
    /// What changed since the installed version, across every release in
    /// between.
    pub notes: Vec<Note>,
    /// The release on GitHub.
    pub page: String,
    /// The file this machine would install, if the release has one.
    pub asset: Option<String>,
}

/// What the daemon remembers, in `updates/state.json`.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
struct Remembered {
    checked_unix: Option<u64>,
    error: Option<String>,
    available: Option<Available>,
    skipped: BTreeSet<Version>,
    /// Install this at the next window.
    scheduled: Option<Version>,
    /// Already told the person about this one.
    notified: Option<Version>,
    /// The newest helper job already looked at.
    seen_job: u64,
}

/// Everything the dashboard shows about updates.
#[derive(Debug, Clone, Serialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct UpdateInfo {
    /// The running version.
    pub current: String,
    pub method: Method,
    /// Words for the install method.
    pub method_text: String,
    /// Whether installs from the dashboard work on this machine.
    pub can_install: bool,
    /// Why not, when they don't.
    pub cannot_install: Option<String>,
    pub settings: Updates,
    pub checked_unix: Option<u64>,
    /// Why the last check failed.
    pub error: Option<String>,
    /// The newest release, when it is newer than this one.
    pub available: Option<Available>,
    #[cfg_attr(feature = "openapi", schema(value_type = Vec<String>))]
    pub skipped: Vec<Version>,
    /// The version that installs at the next window.
    #[cfg_attr(feature = "openapi", schema(value_type = Option<String>))]
    pub scheduled: Option<Version>,
    /// The install or rollback going on now, or the last one.
    pub job: Option<Job>,
    /// The version a rollback goes back to.
    #[cfg_attr(feature = "openapi", schema(value_type = Option<String>))]
    pub rollback_to: Option<Version>,
}

/// What the updater is built from.
pub struct Config {
    pub layout: Layout,
    /// The running version, as the binary names it.
    pub current: String,
    pub method: Method,
    /// Whether the root helper's units are installed.
    pub helper: bool,
    pub arch: Option<Arch>,
    pub fetch: Arc<dyn Fetch>,
    pub keys: Vec<PublicKey>,
    pub settings: Updates,
}

/// Checks for, downloads and requests updates.
pub struct Updater {
    layout: Layout,
    current_text: String,
    current: Option<Version>,
    method: Method,
    helper: bool,
    arch: Option<Arch>,
    fetch: Arc<dyn Fetch>,
    keys: Vec<PublicKey>,
    settings: RwLock<Updates>,
    memory: Mutex<Remembered>,
    /// One check or download at a time.
    busy: tokio::sync::Mutex<()>,
    wake: tokio::sync::Notify,
}

impl Updater {
    #[must_use]
    pub fn new(config: Config) -> Self {
        let memory = read_json(&config.layout.daemon_state()).unwrap_or_default();
        Self {
            current: Version::parse(&config.current),
            current_text: config.current,
            layout: config.layout,
            method: config.method,
            helper: config.helper,
            arch: config.arch,
            fetch: config.fetch,
            keys: config.keys,
            settings: RwLock::new(config.settings),
            memory: Mutex::new(memory),
            busy: tokio::sync::Mutex::new(()),
            wake: tokio::sync::Notify::new(),
        }
    }

    #[must_use]
    pub fn layout(&self) -> &Layout {
        &self.layout
    }

    /// Takes new settings from the desired state.
    pub fn set_settings(&self, settings: Updates) {
        let mut s = self.settings.write().unwrap_or_else(PoisonError::into_inner);
        if *s != settings {
            *s = settings;
            drop(s);
            self.wake.notify_one();
        }
    }

    fn settings(&self) -> Updates {
        self.settings.read().unwrap_or_else(PoisonError::into_inner).clone()
    }

    fn remember<R>(&self, f: impl FnOnce(&mut Remembered) -> R) -> R {
        let mut m = self.memory.lock().unwrap_or_else(PoisonError::into_inner);
        let r = f(&mut m);
        if let Err(e) = ensure_dir(&self.layout.updates(), 0o700)
            .and_then(|()| write_json(&self.layout.daemon_state(), &*m, 0o600))
        {
            warn!("cannot save the update state: {e}");
        }
        r
    }

    fn cannot_install(&self) -> Option<String> {
        if self.current.is_none() {
            return Some("This is a development build. Install a release to get updates.".into());
        }
        if self.arch.is_none() {
            return Some("There are no releases for this CPU.".into());
        }
        if self.method == Method::Manual {
            return Some(
                "This Hearthgate was not installed from a package or install.sh, so update it the way you installed it."
                    .into(),
            );
        }
        if !self.helper {
            return Some(
                "The upgrade helper is not installed. Install this version once more from a package or install.sh to turn it on."
                    .into(),
            );
        }
        None
    }

    /// The helper's current or last job.
    #[must_use]
    pub fn job(&self) -> Option<Job> {
        read_json(&self.layout.status())
    }

    /// Past installs and rollbacks, newest first.
    #[must_use]
    pub fn history(&self) -> Vec<Job> {
        read_history(&self.layout.history())
    }

    fn job_running(&self) -> bool {
        if std::fs::symlink_metadata(self.layout.request()).is_ok() {
            return true;
        }
        self.job()
            .is_some_and(|j| j.outcome == Outcome::Running && now_unix() < j.started_unix + JOB_STALE)
    }

    fn rollback_to(&self) -> Option<Version> {
        let kept: Backup = read_json(&self.layout.kept())?;
        self.current.filter(|c| kept.version < *c).map(|_| kept.version)
    }

    /// Everything the dashboard shows.
    #[must_use]
    pub fn info(&self) -> UpdateInfo {
        let mut m = self.memory.lock().unwrap_or_else(PoisonError::into_inner).clone();
        // Rolled-back versions count as skipped before the loop records it.
        for job in self.history().into_iter().filter(|j| j.started_unix > m.seen_job) {
            if job.outcome == Outcome::RolledBack
                && let Some(v) = job.to
            {
                m.skipped.insert(v);
                if m.scheduled == Some(v) {
                    m.scheduled = None;
                }
            }
        }
        let cannot = self.cannot_install();
        UpdateInfo {
            current: self.current_text.clone(),
            method: self.method,
            method_text: self.method.describe().to_owned(),
            can_install: cannot.is_none(),
            cannot_install: cannot,
            settings: self.settings(),
            checked_unix: m.checked_unix,
            error: m.error,
            available: m.available.filter(|a| self.current.is_none_or(|c| a.version > c)),
            skipped: m.skipped.into_iter().collect(),
            scheduled: m.scheduled,
            job: self.job(),
            rollback_to: self.rollback_to(),
        }
    }

    /// Versions the helper rolled back become skipped, so they don't
    /// install again by themselves.
    fn absorb_jobs(&self) {
        let seen = self.memory.lock().unwrap_or_else(PoisonError::into_inner).seen_job;
        let new: Vec<Job> = self.history().into_iter().filter(|j| j.started_unix > seen).collect();
        if new.is_empty() {
            return;
        }
        self.remember(|m| {
            for job in &new {
                m.seen_job = m.seen_job.max(job.started_unix);
                if job.outcome == Outcome::RolledBack
                    && let Some(v) = job.to
                {
                    info!("Hearthgate {v} was rolled back; it won't install by itself again");
                    m.skipped.insert(v);
                    if m.scheduled == Some(v) {
                        m.scheduled = None;
                    }
                }
            }
        });
    }

    /// Asks GitHub for releases now.
    pub async fn check(&self) -> Result<UpdateInfo, String> {
        let busy = self.busy.lock().await;
        let result = self.fetch_available().await;
        self.remember(|m| {
            m.checked_unix = Some(now_unix());
            match &result {
                Ok(a) => {
                    m.error = None;
                    m.available.clone_from(a);
                }
                Err(e) => m.error = Some(e.clone()),
            }
        });
        drop(busy);
        result.map(|_| self.info())
    }

    /// The newest release if it is newer, without remembering anything.
    pub async fn latest(&self) -> Result<Option<Available>, String> {
        self.fetch_available().await
    }

    async fn fetch_available(&self) -> Result<Option<Available>, String> {
        let json = self.fetch.get(&github::releases_url(), 4 << 20).await?;
        let releases = github::parse_releases(&json)?;
        let Some(newest) = releases.first() else {
            return Ok(None);
        };
        let current = self.current;
        if current.is_some_and(|c| newest.version <= c) {
            return Ok(None);
        }
        let mut notes: Vec<Note> = Vec::new();
        for r in releases.iter().filter(|r| current.is_none_or(|c| r.version > c)) {
            for n in crate::notes::parse(&r.body) {
                if !notes.contains(&n) {
                    notes.push(n);
                }
            }
        }
        notes.sort_by_key(|n| match n.kind {
            NoteKind::Breaking => 0,
            NoteKind::Security => 1,
            NoteKind::Feature => 2,
            NoteKind::Fix => 3,
        });
        notes.truncate(100);
        let bump = current
            .and_then(|c| c.bump_to(newest.version))
            .unwrap_or(Bump::Major);
        let asset = self
            .arch
            .and_then(|a| pick_asset(newest.assets.iter().map(String::as_str), self.method, newest.version, a))
            .map(str::to_owned);
        Ok(Some(Available {
            version: newest.version,
            published_at: newest.published_at.clone(),
            bump,
            notes,
            page: github::release_page(newest.version),
            asset,
        }))
    }

    /// Downloads and verifies `version` into the daemon's update directory.
    async fn download(&self, version: Version) -> Result<(), String> {
        let _busy = self.busy.lock().await;
        let arch = self.arch.ok_or("there are no releases for this CPU")?;
        let get = |name: &'static str, max: usize| {
            let url = github::asset_url(version, name);
            async move { self.fetch.get(&url, max as u64).await.map_err(|e| format!("{name}: {e}")) }
        };
        let sums = get("SHA256SUMS", MAX_SUMS_BYTES).await?;
        let sig = get("SHA256SUMS.minisig", MAX_SIGNATURE_BYTES).await?;
        let sig_text = String::from_utf8(sig.clone()).map_err(|_| "the signature is not text".to_owned())?;
        let checked = verify_sums(&sums, &sig_text, version, &self.keys).map_err(|e| e.to_string())?;
        let asset = pick_asset(checked.names(), self.method, version, arch)
            .ok_or_else(|| format!("the release has no {} for this machine", self.method.describe()))?
            .to_owned();
        let data = self
            .fetch
            .get(&github::asset_url(version, &asset), MAX_ASSET_BYTES)
            .await
            .map_err(|e| format!("{asset}: {e}"))?;
        checked.check_bytes(&asset, &data).map_err(|e| e.to_string())?;

        let updates = self.layout.updates();
        ensure_dir(&updates, 0o700).map_err(|e| e.to_string())?;
        // Only one download is kept.
        if let Ok(entries) = std::fs::read_dir(&updates) {
            for e in entries.filter_map(Result::ok) {
                let name = e.file_name();
                if name.to_string_lossy().starts_with('v') && e.file_type().is_ok_and(|t| t.is_dir()) {
                    let _ = std::fs::remove_dir_all(e.path());
                }
            }
        }
        let dir = self.layout.download_dir(version);
        ensure_dir(&dir, 0o755).map_err(|e| e.to_string())?;
        // Readable by the helper, which copies them before trusting them.
        write_atomic(&dir.join("SHA256SUMS"), &sums, 0o644).map_err(|e| e.to_string())?;
        write_atomic(&dir.join("SHA256SUMS.minisig"), &sig, 0o644).map_err(|e| e.to_string())?;
        write_atomic(&dir.join(&asset), &data, 0o644).map_err(|e| e.to_string())?;
        info!("downloaded and verified {asset}");
        Ok(())
    }

    /// Installs `version` now, or schedules it for the next window.
    pub async fn install(&self, version: Version, now: bool) -> Result<UpdateInfo, String> {
        if let Some(why) = self.cannot_install() {
            return Err(why);
        }
        self.absorb_jobs();
        let available = self.memory.lock().unwrap_or_else(PoisonError::into_inner).available.clone();
        if available.as_ref().map(|a| a.version) != Some(version) {
            return Err(format!("{version} is not the newest release; check for updates first"));
        }
        if self.job_running() {
            return Err("An install or rollback is already going on.".into());
        }
        if !now {
            self.remember(|m| m.scheduled = Some(version));
            self.wake.notify_one();
            return Ok(self.info());
        }
        self.download(version).await?;
        self.remember(|m| {
            m.scheduled = None;
            m.skipped.remove(&version);
        });
        write_json(&self.layout.request(), &Request::Install { version }, 0o644).map_err(|e| e.to_string())?;
        info!("asked the upgrade helper to install {version}");
        Ok(self.info())
    }

    /// Asks the helper to go back to the kept version.
    pub fn rollback(&self) -> Result<UpdateInfo, String> {
        if let Some(why) = self.cannot_install() {
            return Err(why);
        }
        if self.rollback_to().is_none() {
            return Err("There is no previous version kept.".into());
        }
        if self.job_running() {
            return Err("An install or rollback is already going on.".into());
        }
        ensure_dir(&self.layout.updates(), 0o700).map_err(|e| e.to_string())?;
        write_json(&self.layout.request(), &Request::Rollback, 0o644).map_err(|e| e.to_string())?;
        info!("asked the upgrade helper to roll back");
        Ok(self.info())
    }

    /// Don't offer or install `version` by itself.
    pub fn skip(&self, version: Version) -> UpdateInfo {
        self.remember(|m| {
            m.skipped.insert(version);
            if m.scheduled == Some(version) {
                m.scheduled = None;
            }
        });
        self.info()
    }

    pub fn unskip(&self, version: Version) -> UpdateInfo {
        self.remember(|m| m.skipped.remove(&version));
        self.info()
    }

    /// Cancels a scheduled install.
    pub fn unschedule(&self) -> UpdateInfo {
        self.remember(|m| m.scheduled = None);
        self.info()
    }

    /// What should install now, if anything: a scheduled version, or one the
    /// automatic setting allows, inside the window.
    fn due(&self, minute: u16) -> Option<Version> {
        let settings = self.settings();
        if !policy::in_install_window(&settings, minute) || self.cannot_install().is_some() || self.job_running() {
            return None;
        }
        let m = self.memory.lock().unwrap_or_else(PoisonError::into_inner);
        let available = m.available.as_ref()?;
        if m.scheduled == Some(available.version) {
            return Some(available.version);
        }
        let auto = !m.skipped.contains(&available.version)
            && policy::allows(settings.auto_install, available.bump)
            && !available.notes.iter().any(|n| n.kind == NoteKind::Breaking);
        auto.then_some(available.version)
    }

    /// One round of the background loop. `notify` is called once per new
    /// release.
    async fn tick(&self, minute: u16, notify: &(dyn Fn(&str, &Available) + Send + Sync)) {
        self.absorb_jobs();
        let settings = self.settings();
        let checked = self.memory.lock().unwrap_or_else(PoisonError::into_inner).checked_unix;
        let scheduled = self.memory.lock().unwrap_or_else(PoisonError::into_inner).scheduled.is_some();
        let stale = checked.is_none_or(|t| now_unix() >= t + CHECK_EVERY);
        if (settings.check || scheduled) && stale && self.current.is_some() {
            match self.check().await {
                Ok(_) => debug!("checked for Hearthgate updates"),
                Err(e) => debug!("cannot check for updates: {e}"),
            }
        }
        let fresh = {
            let m = self.memory.lock().unwrap_or_else(PoisonError::into_inner);
            m.available
                .clone()
                .filter(|a| m.notified != Some(a.version) && !m.skipped.contains(&a.version))
        };
        if let Some(a) = fresh {
            notify(&self.current_text, &a);
            self.remember(|m| m.notified = Some(a.version));
        }
        if let Some(v) = self.due(minute) {
            info!("installing Hearthgate {v} in the install window");
            if let Err(e) = self.install(v, true).await {
                warn!("cannot install {v}: {e}");
                self.remember(|m| m.error = Some(format!("Installing {v} failed: {e}")));
            }
        }
    }

    /// Checks daily and installs in the window, until `stop`.
    pub async fn run(
        self: Arc<Self>,
        stop: CancellationToken,
        notify: impl Fn(&str, &Available) + Send + Sync + 'static,
    ) {
        if self.current.is_none() {
            return;
        }
        // Let the rest of the daemon come up first.
        tokio::select! {
            () = stop.cancelled() => return,
            () = tokio::time::sleep(Duration::from_secs(120)) => {}
        }
        loop {
            self.tick(policy::local_minute(), &notify).await;
            tokio::select! {
                () = stop.cancelled() => return,
                () = self.wake.notified() => {}
                () = tokio::time::sleep(Duration::from_secs(60)) => {}
            }
        }
    }
}

#[cfg(test)]
mod tests;
