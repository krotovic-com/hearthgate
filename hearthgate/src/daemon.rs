//! The long-running gateway process (`hearthgate run`).
//!
//! The daemon watches the store for new revisions and hands each one to the
//! subsystems, which reconcile the machine to match it. Changes made with
//! `hearthgate apply` are noticed within a second; `SIGHUP` (what
//! `systemctl reload` sends) checks immediately.

use std::sync::Arc;
use std::time::Duration;

use anyhow::{Context, Result};
use hearthgate_acme::{CertJob, CertStore, Issuer, Outcome, acme};
use hearthgate_model::AcmeSettings;
use hearthgate_model::DesiredState;
use hearthgate_proxy::plan::CertSpec;
use hearthgate_proxy::{Gateway, NoContainers, compile};
use hearthgate_services::{HealthChecker, health};
use hearthgate_store::Store;
use tokio::signal::unix::{SignalKind, signal};
use tokio::sync::watch;
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
    let gateway = Arc::new(Gateway::new());
    let certs = Arc::new(CertStore::new(&bootstrap.data_dir));
    let issuer = Arc::new(Issuer::new(
        &bootstrap.data_dir,
        Arc::clone(&certs),
        Arc::clone(gateway.certificates()),
        Arc::clone(gateway.http_challenges()),
    ));
    let (acme_tx, acme_rx) = watch::channel((AcmeSettings::default(), Vec::new()));
    let acme_task = tokio::spawn(acme_loop(Arc::clone(&issuer), acme_rx));
    let mut daemon = Daemon {
        health: HealthChecker::default(),
        store,
        certs,
        issuer,
        acme: acme_tx,
        cert_generation: String::new(),
        cert_specs: Vec::new(),
        gateway,
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
    acme_task.abort();
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
    health: HealthChecker,
    store: Arc<Store>,
    certs: Arc<CertStore>,
    issuer: Arc<Issuer>,
    /// Automatic certificate work for the ACME task.
    acme: watch::Sender<(AcmeSettings, Vec<CertJob>)>,
    /// Last seen certificate store generation.
    cert_generation: String,
    /// Certificates the running revision needs.
    cert_specs: Vec<CertSpec>,
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
            self.reload_certificates_if_changed();
            return;
        }
        let state = revision.map(|r| r.state).unwrap_or_default();
        self.drain = state.limits.drain_timeout;
        let plan = compile(&state, self.gateway.upstreams(), &NoContainers);
        // Certificates first, so the first handshake on a new port has one.
        self.cert_specs.clone_from(&plan.certs);
        self.load_certificates(true);
        let errors = self.gateway.apply(&plan).await;
        self.health.update(
            health::jobs(&state, self.gateway.upstreams()),
            self.gateway.upstreams(),
        );
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
        // Challenges are answered by the listeners, so hand the work to the
        // ACME task only once they are up.
        let work = (state.acme.clone(), acme::jobs(&state));
        if *self.acme.borrow() != work {
            self.issuer.reset_backoff();
            let _ = self.acme.send(work);
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

    fn reload_certificates_if_changed(&mut self) {
        if self.certs.generation() != self.cert_generation {
            info!("certificates changed; reloading");
            self.load_certificates(true);
        }
    }

    /// Loads the certificates the running revision needs into the gateway.
    fn load_certificates(&mut self, log_notes: bool) {
        self.cert_generation = self.certs.generation();
        let (table, notes) = self.certs.table(&self.cert_specs);
        if log_notes {
            for note in notes {
                warn!("{note}; serving a self-signed certificate for now");
            }
        }
        self.gateway.certificates().set(table);
    }
}

/// How often certificates are checked for renewal.
const RENEWAL_CHECK: Duration = Duration::from_secs(3600);

/// Issues and renews automatic certificates in the background. New
/// certificates land in the store, and the daemon's next tick loads them.
async fn acme_loop(issuer: Arc<Issuer>, mut work: watch::Receiver<(AcmeSettings, Vec<CertJob>)>) {
    loop {
        let (settings, jobs) = work.borrow_and_update().clone();
        let mut next = RENEWAL_CHECK;
        if !jobs.is_empty() {
            for (_, outcome) in issuer.run(&settings, &jobs).await {
                if let Outcome::Backoff { until } = outcome {
                    next = next.min(until.saturating_duration_since(std::time::Instant::now()));
                }
            }
        }
        tokio::select! {
            changed = work.changed() => if changed.is_err() { return },
            () = tokio::time::sleep(next.max(Duration::from_secs(1))) => {}
        }
    }
}
