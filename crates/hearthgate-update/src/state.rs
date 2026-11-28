//! Files the daemon and the root helper exchange, and where they live.
//!
//! The daemon owns `<data>/updates/` (mode 0700): downloads, the install
//! request, its health report and its own bookkeeping. The root helper owns
//! `/var/lib/hearthgate-upgrade/` (mode 0755): progress and history, which
//! the daemon reads to show in the dashboard, and `backup/` (mode 0700),
//! which the daemon can't reach at all.

use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};

use crate::Version;

/// The root helper's directory.
pub const HELPER_DIR: &str = "/var/lib/hearthgate-upgrade";

/// Where everything is.
#[derive(Debug, Clone)]
pub struct Layout {
    /// The daemon's data directory, such as `/var/lib/hearthgate`.
    pub data_dir: PathBuf,
    /// The helper's directory.
    pub helper_dir: PathBuf,
}

impl Layout {
    #[must_use]
    pub fn new(data_dir: impl Into<PathBuf>) -> Self {
        Self {
            data_dir: data_dir.into(),
            helper_dir: PathBuf::from(HELPER_DIR),
        }
    }

    /// What the helper keeps for a rollback, readable by the daemon.
    #[must_use]
    pub fn kept(&self) -> PathBuf {
        self.helper_dir.join("kept.json")
    }

    /// The daemon's update directory.
    #[must_use]
    pub fn updates(&self) -> PathBuf {
        self.data_dir.join("updates")
    }
    /// Downloads for one version.
    #[must_use]
    pub fn download_dir(&self, version: Version) -> PathBuf {
        self.updates().join(version.to_string())
    }
    /// The request the path unit watches.
    #[must_use]
    pub fn request(&self) -> PathBuf {
        self.updates().join("request.json")
    }
    /// The daemon's own report of how it is doing, read after a restart.
    #[must_use]
    pub fn health(&self) -> PathBuf {
        self.updates().join("health.json")
    }
    /// The daemon's bookkeeping: last check, skipped versions, schedule.
    #[must_use]
    pub fn daemon_state(&self) -> PathBuf {
        self.updates().join("state.json")
    }
    /// Progress of the install or rollback running now, or the last one.
    #[must_use]
    pub fn status(&self) -> PathBuf {
        self.helper_dir.join("status.json")
    }
    /// Past installs and rollbacks.
    #[must_use]
    pub fn history(&self) -> PathBuf {
        self.helper_dir.join("history.json")
    }
    /// What runs now, kept for a rollback.
    #[must_use]
    pub fn backup(&self) -> PathBuf {
        self.helper_dir.join("backup")
    }
    /// The helper's copy of the files it installs from.
    #[must_use]
    pub fn work(&self) -> PathBuf {
        self.helper_dir.join("work")
    }
}

/// Seconds since the Unix epoch.
#[must_use]
pub fn now_unix() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_secs())
}

/// What the daemon asks the root helper to do.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "action", rename_all = "lowercase")]
pub enum Request {
    /// Install a downloaded, verified release.
    Install { version: Version },
    /// Go back to the version kept in the backup.
    Rollback,
}

/// The kind of job.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(rename_all = "lowercase")]
pub enum Action {
    Install,
    Rollback,
}

/// How a job ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(rename_all = "kebab-case")]
pub enum Outcome {
    /// Still going.
    Running,
    /// The new version runs and is healthy.
    Installed,
    /// The new version was not healthy; the old one runs again.
    RolledBack,
    /// A manual rollback finished.
    Restored,
    /// Nothing changed: the files didn't verify, or something else stopped
    /// it before the install.
    Refused,
    /// It went wrong and Hearthgate may not be running. See the log.
    Failed,
}

/// Where a step is.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(rename_all = "lowercase")]
pub enum StepState {
    Pending,
    Running,
    Done,
    Failed,
    Skipped,
}

/// One step of a job, as the dashboard lists them.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct Step {
    pub title: String,
    pub detail: String,
    pub state: StepState,
}

/// An install or rollback: what the helper writes to `status.json` as it
/// goes, and keeps in the history when done.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct Job {
    pub action: Action,
    #[cfg_attr(feature = "openapi", schema(value_type = String))]
    pub from: Version,
    /// The version it goes to. For a rollback, the kept one.
    #[cfg_attr(feature = "openapi", schema(value_type = Option<String>))]
    pub to: Option<Version>,
    pub steps: Vec<Step>,
    pub outcome: Outcome,
    /// One sentence on how it ended.
    pub detail: Option<String>,
    pub started_unix: u64,
    pub finished_unix: Option<u64>,
    /// What happened, line by line.
    pub log: Vec<String>,
}

/// History entries kept.
pub const HISTORY: usize = 30;

/// The daemon's report, written once it has applied its configuration.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Health {
    pub version: String,
    pub pid: u32,
    pub started_unix: u64,
    /// False when the firewall did not load: without it nothing should be
    /// exposed, so the update is rolled back. Other problems are listed but
    /// don't fail it, since the old version would have them too.
    pub ok: bool,
    pub problems: Vec<String>,
}

/// What the backup holds.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct Backup {
    #[cfg_attr(feature = "openapi", schema(value_type = String))]
    pub version: Version,
    pub method: crate::method::Method,
    /// The installed package file of that version, when the helper installed
    /// it, so a rollback goes through the package manager.
    pub package: Option<String>,
    pub taken_unix: u64,
}

/// Writes the daemon's health report for the helper.
pub fn write_health(
    layout: &Layout,
    version: &str,
    started_unix: u64,
    ok: bool,
    problems: Vec<String>,
) -> std::io::Result<()> {
    let health = Health {
        version: version.to_owned(),
        pid: std::process::id(),
        started_unix,
        ok,
        problems,
    };
    crate::files::ensure_dir(&layout.updates(), 0o700)?;
    crate::files::write_json(&layout.health(), &health, 0o644)
}

/// Reads the history, newest first.
#[must_use]
pub fn read_history(path: &Path) -> Vec<Job> {
    crate::files::read_json(path).unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn request_format() {
        let r: Request = serde_json::from_str(r#"{"action":"install","version":"1.3.0"}"#).unwrap();
        assert_eq!(
            r,
            Request::Install {
                version: Version::parse("1.3.0").unwrap()
            }
        );
        let r: Request = serde_json::from_str(r#"{"action":"rollback"}"#).unwrap();
        assert_eq!(r, Request::Rollback);
        assert!(
            serde_json::from_str::<Request>(r#"{"action":"install","version":"../../etc"}"#)
                .is_err()
        );
        assert!(serde_json::from_str::<Request>(r#"{"action":"run","cmd":"sh"}"#).is_err());
    }
}
