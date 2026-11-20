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
    let mut applied: Option<i64> = None;
    reconcile(&store, &mut applied).await;

    let mut hangup = signal(SignalKind::hangup())?;
    let mut terminate = signal(SignalKind::terminate())?;
    let mut interrupt = signal(SignalKind::interrupt())?;
    let mut tick = tokio::time::interval(POLL_INTERVAL);
    tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    loop {
        tokio::select! {
            _ = tick.tick() => reconcile(&store, &mut applied).await,
            _ = hangup.recv() => {
                info!("reload requested");
                reconcile(&store, &mut applied).await;
            }
            _ = terminate.recv() => break,
            _ = interrupt.recv() => break,
        }
    }
    info!("stopping");
    Ok(())
}

/// Applies the current revision if it differs from the one running.
async fn reconcile(store: &Arc<Store>, applied: &mut Option<i64>) {
    let store = Arc::clone(store);
    let current = tokio::task::spawn_blocking(move || -> Result<_> {
        let id = store.current_id()?;
        if id.is_none() {
            return Ok(None);
        }
        Ok(store.current()?)
    })
    .await;
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
    // Revision 0 stands for "no revisions yet".
    let key = id.unwrap_or(0);
    if *applied == Some(key) {
        return;
    }
    let state = revision.map(|r| r.state).unwrap_or_default();
    if let Err(e) = apply_state(&state) {
        // Keep running the previous configuration and try again next tick.
        warn!("revision {id:?} could not be applied: {e:#}");
        return;
    }
    if let Some(id) = id {
        info!("running revision {id}");
    } else {
        info!("no revisions yet: nothing is exposed");
    }
    *applied = Some(key);
}

#[allow(clippy::unnecessary_wraps)] // subsystems are wired in from M1.2
fn apply_state(_state: &DesiredState) -> Result<()> {
    Ok(())
}
