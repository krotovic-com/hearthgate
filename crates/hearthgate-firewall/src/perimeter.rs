//! The firewall's runtime: what is installed, the safe-apply window, and
//! the addresses loaded from feeds and bans. Shared by the daemon (which
//! reconciles) and the admin API (which shows status and confirms changes).

use std::collections::BTreeMap;
use std::net::IpAddr;
use std::sync::{Mutex, MutexGuard, PoisonError};
use std::time::{Duration, Instant, SystemTime};

use hearthgate_model::Firewall;
use ipnet::IpNet;
use tracing::{info, warn};

use crate::nft::{Dropped, Nft, NftError, RecentDrop};
use crate::ruleset::{self, Dynamic, Inputs};

/// How long a firewall change has to be confirmed before it is undone.
pub const CONFIRM_WINDOW: Duration = Duration::from_secs(60);

/// A firewall change waiting for confirmation.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct Pending {
    /// Revision that made the change.
    pub revision: i64,
    /// Firewall settings to go back to if nobody confirms.
    pub previous: Firewall,
    /// When the change was installed, as seconds since the Unix epoch.
    pub applied_unix: u64,
    /// Deadline, as seconds since the Unix epoch.
    pub deadline_unix: u64,
    /// Monotonic install time, to check that a confirmation arrived over a
    /// connection opened after the change.
    #[serde(skip, default = "Instant::now")]
    pub applied_at: Instant,
}

impl Pending {
    #[must_use]
    pub fn new(revision: i64, previous: Firewall) -> Self {
        let now = unix_now();
        Self {
            revision,
            previous,
            applied_unix: now,
            deadline_unix: now + CONFIRM_WINDOW.as_secs(),
            applied_at: Instant::now(),
        }
    }

    /// Seconds left before the change is undone.
    #[must_use]
    pub fn seconds_left(&self) -> u64 {
        self.deadline_unix.saturating_sub(unix_now())
    }

    #[must_use]
    pub fn expired(&self) -> bool {
        unix_now() >= self.deadline_unix
    }
}

/// Why a confirmation was refused.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ConfirmError {
    #[error("no firewall change is waiting for confirmation")]
    NothingPending,
    #[error(
        "this connection was opened before the change, so it does not show that the new rules let you in; retry"
    )]
    OldConnection,
}

/// What the dashboard shows about the firewall.
#[derive(Debug, Clone, Default, PartialEq, Eq, serde::Serialize)]
pub struct Status {
    /// The desired state turns the firewall on.
    pub enabled: bool,
    /// The table is in place.
    pub installed: bool,
    /// Why the last install failed, if it did.
    pub error: Option<String>,
    /// Revision waiting for confirmation, and seconds left.
    pub pending: Option<(i64, u64)>,
    /// Addresses loaded from feeds and countries.
    pub feed_entries: usize,
    /// Per feed: entries and last error.
    pub feeds: BTreeMap<String, FeedStatus>,
    /// Active bans.
    pub bans: usize,
}

/// One feed's state.
#[derive(Debug, Clone, Default, PartialEq, Eq, serde::Serialize)]
pub struct FeedStatus {
    pub entries: usize,
    /// When the list was last downloaded, as seconds since the Unix epoch.
    pub updated_unix: Option<u64>,
    pub error: Option<String>,
}

#[derive(Default)]
struct State {
    installed: Option<Inputs>,
    error: Option<String>,
    pending: Option<Pending>,
    /// Per feed, the networks it lists.
    feeds: BTreeMap<String, Vec<IpNet>>,
    feed_status: BTreeMap<String, FeedStatus>,
    bans: usize,
}

/// The firewall runtime.
pub struct Perimeter {
    nft: Nft,
    state: Mutex<State>,
    /// Serialises nft transactions.
    apply_lock: tokio::sync::Mutex<()>,
}

impl Perimeter {
    #[must_use]
    pub fn new(nft: Nft) -> Self {
        Self {
            nft,
            state: Mutex::default(),
            apply_lock: tokio::sync::Mutex::new(()),
        }
    }

    fn lock(&self) -> MutexGuard<'_, State> {
        self.state.lock().unwrap_or_else(PoisonError::into_inner)
    }

    #[must_use]
    pub fn nft(&self) -> &Nft {
        &self.nft
    }

    /// The inputs installed now, if the firewall is on.
    #[must_use]
    pub fn installed(&self) -> Option<Inputs> {
        self.lock().installed.clone()
    }

    /// Installs `inputs`, or removes the table when the firewall is off.
    /// Does nothing when the same inputs are already installed, unless
    /// `force` is set.
    pub async fn apply(&self, inputs: &Inputs, force: bool) -> Result<(), NftError> {
        let _guard = self.apply_lock.lock().await;
        let was = self.lock().installed.clone();
        if !inputs.needs_table() {
            if was.is_some() || force || self.nft.installed().await {
                self.nft.apply(&ruleset::remove()).await?;
                info!("firewall is off; the hearthgate table was removed");
            }
            let mut s = self.lock();
            s.installed = None;
            s.error = None;
            return Ok(());
        }
        if !force && was.as_ref() == Some(inputs) {
            return Ok(());
        }
        let result = self.install(inputs).await;
        let mut s = self.lock();
        match &result {
            Ok(()) => {
                s.installed = Some(inputs.clone());
                s.error = None;
            }
            Err(e) => s.error = Some(e.to_string()),
        }
        result
    }

    /// Re-renders with the current feeds, keeping bans.
    async fn install(&self, inputs: &Inputs) -> Result<(), NftError> {
        let bans = if self.nft.installed().await {
            self.nft.bans().await.unwrap_or_default()
        } else {
            Vec::new()
        };
        let feeds = self.feed_networks(&inputs.firewall);
        let dynamic = Dynamic {
            feeds,
            bans: bans.clone(),
        };
        self.nft.apply(&ruleset::render(inputs, &dynamic)).await?;
        self.lock().bans = bans.len();
        Ok(())
    }

    /// Networks from the feeds the settings subscribe to.
    fn feed_networks(&self, firewall: &Firewall) -> Vec<IpNet> {
        let wanted = wanted_feeds(firewall);
        let s = self.lock();
        let mut out = Vec::new();
        for name in wanted {
            if let Some(nets) = s.feeds.get(&name) {
                out.extend(nets.iter().copied());
            }
        }
        out
    }

    /// Stores a feed's networks and reinstalls if the firewall is on.
    pub async fn set_feed(&self, name: &str, nets: Vec<IpNet>, updated_unix: u64) {
        {
            let mut s = self.lock();
            let status = s.feed_status.entry(name.to_owned()).or_default();
            status.entries = nets.len();
            status.updated_unix = Some(updated_unix);
            status.error = None;
            s.feeds.insert(name.to_owned(), nets);
        }
        self.reinstall().await;
    }

    /// Records a failed download. The previous copy stays in use.
    pub fn feed_failed(&self, name: &str, error: String) {
        let mut s = self.lock();
        s.feed_status.entry(name.to_owned()).or_default().error = Some(error);
    }

    /// Forgets feeds that are no longer subscribed.
    pub fn retain_feeds(&self, names: &[String]) {
        let mut s = self.lock();
        s.feeds.retain(|k, _| names.contains(k));
        s.feed_status.retain(|k, _| names.contains(k));
    }

    async fn reinstall(&self) {
        let Some(inputs) = self.installed() else {
            return;
        };
        if let Err(e) = self.apply(&inputs, true).await {
            warn!("cannot reload the firewall: {e}");
        }
    }

    /// Bans `ip` for `time`.
    pub async fn ban(&self, ip: IpAddr, time: Duration) -> Result<(), NftError> {
        if self.installed().is_none() {
            return Ok(());
        }
        let _guard = self.apply_lock.lock().await;
        self.nft.apply(&ruleset::ban(ip, time)).await?;
        self.lock().bans += 1;
        Ok(())
    }

    /// Lifts a ban.
    pub async fn unban(&self, ip: IpAddr) -> Result<(), NftError> {
        let _guard = self.apply_lock.lock().await;
        self.nft.apply(&ruleset::unban(ip)).await
    }

    /// Current bans with time left.
    pub async fn bans(&self) -> Vec<(IpAddr, Duration)> {
        if self.installed().is_none() {
            return Vec::new();
        }
        let bans = self.nft.bans().await.unwrap_or_default();
        self.lock().bans = bans.len();
        bans
    }

    /// Recent drops (empty when the firewall is off).
    pub async fn recent(&self) -> Vec<RecentDrop> {
        if self.installed().is_none() {
            return Vec::new();
        }
        self.nft.recent().await.unwrap_or_default()
    }

    /// Drop counters (zero when the firewall is off).
    pub async fn dropped(&self) -> Dropped {
        if self.installed().is_none() {
            return Dropped::default();
        }
        self.nft.dropped().await.unwrap_or_default()
    }

    /// Starts the confirmation window for a change.
    pub fn begin_pending(&self, pending: Pending) {
        warn!(
            "firewall changed by revision {}; confirm within {}s or it is undone",
            pending.revision,
            CONFIRM_WINDOW.as_secs()
        );
        self.lock().pending = Some(pending);
    }

    #[must_use]
    pub fn pending(&self) -> Option<Pending> {
        self.lock().pending.clone()
    }

    /// Clears the pending change without checking (CLI confirmation, or
    /// after a rollback).
    pub fn clear_pending(&self) -> Option<Pending> {
        self.lock().pending.take()
    }

    /// Confirms the pending change from a connection accepted at
    /// `accepted`. Only a connection opened after the change shows that the
    /// new rules let the admin in; older ones survive any ruleset because
    /// established connections are always accepted.
    pub fn confirm(&self, accepted: Instant) -> Result<i64, ConfirmError> {
        let mut s = self.lock();
        let pending = s.pending.as_ref().ok_or(ConfirmError::NothingPending)?;
        if accepted <= pending.applied_at {
            return Err(ConfirmError::OldConnection);
        }
        let revision = pending.revision;
        s.pending = None;
        info!("firewall change in revision {revision} confirmed");
        Ok(revision)
    }

    #[must_use]
    pub fn status(&self) -> Status {
        let s = self.lock();
        Status {
            enabled: s.installed.as_ref().is_some_and(|i| i.firewall.enabled),
            installed: s.installed.is_some(),
            error: s.error.clone(),
            pending: s.pending.as_ref().map(|p| (p.revision, p.seconds_left())),
            feed_entries: s.feeds.values().map(Vec::len).sum(),
            feeds: s.feed_status.clone(),
            bans: s.bans,
        }
    }
}

/// Feed names a firewall setting subscribes to: enabled blocklists, and
/// `country-xx` for each blocked country.
#[must_use]
pub fn wanted_feeds(firewall: &Firewall) -> Vec<String> {
    let mut names: Vec<String> = firewall
        .blocklists
        .iter()
        .filter(|b| b.enabled)
        .map(|b| b.name.clone())
        .collect();
    names.extend(firewall.countries.iter().map(|c| format!("country-{c}")));
    names
}

fn unix_now() -> u64 {
    SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn confirmation_needs_a_new_connection() {
        let p = Perimeter::new(Nft::new(vec!["false".into()]));
        assert_eq!(p.confirm(Instant::now()), Err(ConfirmError::NothingPending));
        let before = Instant::now();
        std::thread::sleep(Duration::from_millis(2));
        p.begin_pending(Pending::new(7, Firewall::default()));
        assert_eq!(p.status().pending.map(|(r, _)| r), Some(7));
        assert_eq!(p.confirm(before), Err(ConfirmError::OldConnection));
        std::thread::sleep(Duration::from_millis(2));
        assert_eq!(p.confirm(Instant::now()), Ok(7));
        assert!(p.pending().is_none());
    }

    #[test]
    fn pending_survives_serialisation_without_the_instant() {
        let p = Pending::new(3, Firewall::default());
        let json = serde_json::to_string(&p).unwrap();
        let back: Pending = serde_json::from_str(&json).unwrap();
        assert_eq!(back.revision, 3);
        assert!(back.seconds_left() <= CONFIRM_WINDOW.as_secs());
        assert!(!back.expired());
    }
}
