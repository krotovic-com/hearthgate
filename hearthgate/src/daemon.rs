//! The long-running gateway process (`hearthgate run`).
//!
//! The daemon watches the store for new revisions and hands each one to the
//! subsystems, which reconcile the machine to match it. Changes made with
//! `hearthgate apply` are noticed within a second; `SIGHUP` (what
//! `systemctl reload` sends) checks immediately.

use std::sync::Arc;
use std::time::Duration;

use anyhow::{Context, Result};
use hearthgate_model::DesiredState;
use hearthgate_proxy::{Gateway, NoContainers, compile};
use hearthgate_store::Store;
use tokio::signal::unix::{SignalKind, signal};
use tracing::{error, info, warn};
use tracing_subscriber::EnvFilter;

use crate::config::Bootstrap;

/// How often the store is checked for a new revision.
const POLL_INTERVAL: Duration = Duration::from_secs(1);

/// Starts the gateway and blocks until it is told to stop.
pub fn run(bootstrap: &Bootstrap) -> Result<std::process::ExitCode> {
    init_logging(&bootstrap.log);
    let runtime = tokio::runtime::Runtime::new().context("cannot start the async runtime")?;
    runtime.block_on(serve(bootstrap.clone()))?;
    Ok(std::process::ExitCode::SUCCESS)
}

fn init_logging(filter: &str) {
    let filter = EnvFilter::try_from_env("HEARTHGATE_LOG")
        .or_else(|_| EnvFilter::try_new(filter))
        .unwrap_or_else(|_| EnvFilter::new("info"));
    let _ = tracing_subscriber::fmt()
        .with_env_filter(filter)
        .with_target(false)
        .try_init();
}

async fn serve(bootstrap: Bootstrap) -> Result<()> {
    info!("{}", crate::banner());
    let store = Arc::new(Store::open_dir(&bootstrap.data_dir).with_context(|| {
        format!(
            "cannot open the state database in {}",
            bootstrap.data_dir.display()
        )
    })?);
    let mut daemon = Daemon {
        store,
        gateway: Arc::new(Gateway::new()),
        applied: None,
        retry_at: None,
        drain: DesiredState::default().limits.drain_timeout,
    };
    daemon.reconcile(false).await;

    let mut hangup = signal(SignalKind::hangup())?;
    let mut terminate = signal(SignalKind::terminate())?;
    let mut interrupt = signal(SignalKind::interrupt())?;
    let mut tick = tokio::time::interval(POLL_INTERVAL);
    tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    loop {
        tokio::select! {
            _ = tick.tick() => daemon.reconcile(false).await,
            _ = hangup.recv() => {
                info!("reload requested");
                daemon.reconcile(true).await;
            }
            _ = terminate.recv() => break,
            _ = interrupt.recv() => break,
        }
    }
    info!(
        "stopping; open connections get {:?} to finish",
        daemon.drain
    );
    daemon.gateway.shutdown(daemon.drain).await;
    Ok(())
}

/// How long to wait before retrying sockets that could not be bound.
const BIND_RETRY: Duration = Duration::from_secs(10);

struct Daemon {
    store: Arc<Store>,
    gateway: Arc<Gateway>,
    /// Revision running now; 0 stands for "no revisions yet".
    applied: Option<i64>,
    /// When to retry a revision whose sockets did not all bind.
    retry_at: Option<tokio::time::Instant>,
    drain: Duration,
}

impl Daemon {
    /// Applies the current revision if it differs from the one running, or
    /// when `force` is set or a bind retry is due.
    async fn reconcile(&mut self, force: bool) {
        let store = Arc::clone(&self.store);
        let current = tokio::task::spawn_blocking(move || store.current()).await;
        let revision = match current {
            Ok(Ok(revision)) => revision,
            Ok(Err(e)) => {
                error!("cannot read the current revision: {e:#}");
                return;
            }
            Err(e) => {
                error!("store task failed: {e}");
                return;
            }
        };
        let id = revision.as_ref().map(|r| r.meta.id);
        let key = id.unwrap_or(0);
        let retry_due = self
            .retry_at
            .is_some_and(|at| tokio::time::Instant::now() >= at);
        if self.applied == Some(key) && !force && !retry_due {
            return;
        }
        let state = revision.map(|r| r.state).unwrap_or_default();
        self.drain = state.limits.drain_timeout;
        let plan = compile(&state, self.gateway.upstreams(), &NoContainers);
        let errors = self.gateway.apply(&plan).await;
        if errors.is_empty() {
            self.retry_at = None;
        } else {
            for e in &errors {
                warn!("{e}");
            }
            warn!(
                "{} socket(s) could not be started; retrying in {}s",
                errors.len(),
                BIND_RETRY.as_secs()
            );
            self.retry_at = Some(tokio::time::Instant::now() + BIND_RETRY);
        }
        if self.applied != Some(key) {
            if let Some(id) = id {
                info!("running revision {id}");
            } else {
                info!("no revisions yet: nothing is exposed");
            }
        }
        self.applied = Some(key);
    }
}
