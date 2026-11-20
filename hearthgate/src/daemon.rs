//! The long-running gateway process (`hearthgate run`).
//!
//! The daemon watches the store for new revisions and hands each one to the
//! subsystems, which reconcile the machine to match it. Changes made with
//! `hearthgate apply` are noticed within a second, changes made through the
//! admin API at once, and `SIGHUP` (what `systemctl reload` sends) checks
//! immediately.

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex, PoisonError, RwLock};
use std::time::Duration;

use anyhow::{Context, Result};
use hearthgate_acme::{CertJob, CertStore, Issuer, Outcome, acme};
use hearthgate_api::{ApiState, RuntimeInfo};
use hearthgate_model::{AcmeSettings, DesiredState};
use hearthgate_proxy::plan::CertSpec;
use hearthgate_proxy::{Gateway, compile};
use hearthgate_services::{Containers, HealthChecker, docker, health};
use hearthgate_store::Store;
use sd_notify::NotifyState;
use tokio::signal::unix::{SignalKind, signal};
use tokio::sync::{Notify, watch};
use tokio_util::sync::CancellationToken;
use tracing::{error, info, warn};
use tracing_subscriber::EnvFilter;

use crate::config::Bootstrap;

/// How often the store is checked for a new revision.
const POLL_INTERVAL: Duration = Duration::from_secs(1);
/// How long to wait before retrying sockets that could not be bound.
const BIND_RETRY: Duration = Duration::from_secs(10);
/// How often certificates are checked for renewal.
const RENEWAL_CHECK: Duration = Duration::from_secs(3600);

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
    // journald adds its own timestamps.
    let under_systemd = std::env::var_os("JOURNAL_STREAM").is_some();
    let builder = tracing_subscriber::fmt()
        .with_env_filter(filter)
        .with_target(false)
        .with_ansi(!under_systemd);
    let _ = if under_systemd {
        builder.without_time().try_init()
    } else {
        builder.try_init()
    };
}

/// Tells systemd about state changes. A no-op outside systemd.
fn notify(state: &[NotifyState<'_>]) {
    let _ = sd_notify::notify(false, state);
}

async fn serve(bootstrap: Bootstrap) -> Result<()> {
    info!("{}", crate::banner());
    let store = Arc::new(Store::open_dir(&bootstrap.data_dir).with_context(|| {
        format!(
            "cannot open the state database in {}",
            bootstrap.data_dir.display()
        )
    })?);
    let stop = CancellationToken::new();
    let gateway = Arc::new(Gateway::new());
    let certs = Arc::new(CertStore::new(&bootstrap.data_dir));
    let issuer = Arc::new(Issuer::new(
        &bootstrap.data_dir,
        Arc::clone(&certs),
        Arc::clone(gateway.certificates()),
        Arc::clone(gateway.http_challenges()),
    ));
    let acme_problems = Arc::new(Mutex::new(BTreeMap::new()));
    let (acme_tx, acme_rx) = watch::channel((AcmeSettings::default(), Vec::new()));
    tokio::spawn(acme_loop(
        Arc::clone(&issuer),
        acme_rx,
        Arc::clone(&acme_problems),
        stop.clone(),
    ));

    let containers = Containers::default();
    if bootstrap.docker.enabled && bootstrap.docker.socket.exists() {
        tokio::spawn(docker::watch(
            bootstrap.docker.socket.to_string_lossy().into_owned(),
            containers.clone(),
            stop.clone(),
        ));
    }

    let runtime = Arc::new(RwLock::new(RuntimeInfo::default()));
    let api_changed = Arc::new(Notify::new());
    if !bootstrap.admin.listen.is_empty() {
        let token = hearthgate_api::load_or_create_token(&bootstrap.data_dir)
            .context("cannot read or create the admin token")?;
        let api = ApiState {
            store: Arc::clone(&store),
            gateway: Arc::clone(&gateway),
            certs: Arc::clone(&certs),
            runtime: Arc::clone(&runtime),
            changed: Arc::clone(&api_changed),
            token: token.into(),
            open_metrics: bootstrap.admin.metrics_without_token,
        };
        for addr in &bootstrap.admin.listen {
            if addr.ip().is_unspecified() {
                warn!(
                    "admin API listens on {addr}; only clients in admin.allow can connect, but \
                     binding the LAN or loopback address is safer"
                );
            }
            let (api, allow, stop) = (api.clone(), bootstrap.admin.allow.clone(), stop.clone());
            let addr = *addr;
            tokio::spawn(async move {
                if let Err(e) = hearthgate_api::serve(addr, allow, api, stop).await {
                    error!("admin API cannot listen on {addr}: {e}");
                }
            });
        }
    }

    let mut daemon = Daemon {
        health: HealthChecker::default(),
        store,
        certs,
        issuer,
        acme: acme_tx,
        acme_problems,
        containers: containers.clone(),
        runtime,
        cert_generation: String::new(),
        cert_specs: Vec::new(),
        cert_notes: Vec::new(),
        bind_errors: Vec::new(),
        gateway,
        applied: None,
        retry_at: None,
        drain: DesiredState::default().limits.drain_timeout,
    };
    daemon.reconcile(false).await;
    notify(&[NotifyState::Ready]);

    let mut watchdog_usec = 0;
    let watchdog = if sd_notify::watchdog_enabled(false, &mut watchdog_usec) {
        Duration::from_micros(watchdog_usec / 2)
    } else {
        Duration::from_secs(3600)
    };
    let mut watchdog_tick = tokio::time::interval(watchdog);

    let mut hangup = signal(SignalKind::hangup())?;
    let mut terminate = signal(SignalKind::terminate())?;
    let mut interrupt = signal(SignalKind::interrupt())?;
    let mut tick = tokio::time::interval(POLL_INTERVAL);
    tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    loop {
        tokio::select! {
            _ = tick.tick() => daemon.reconcile(false).await,
            () = api_changed.notified() => daemon.reconcile(false).await,
            () = containers.changed.notified() => {
                info!("container addresses changed");
                daemon.reconcile(true).await;
            }
            _ = watchdog_tick.tick() => notify(&[NotifyState::Watchdog]),
            _ = hangup.recv() => {
                info!("reload requested");
                daemon.reconcile(true).await;
            }
            _ = terminate.recv() => break,
            _ = interrupt.recv() => break,
        }
    }
    notify(&[NotifyState::Stopping]);
    stop.cancel();
    daemon.health.stop();
    info!(
        "stopping; open connections get {:?} to finish",
        daemon.drain
    );
    daemon.gateway.shutdown(daemon.drain).await;
    Ok(())
}

struct Daemon {
    health: HealthChecker,
    store: Arc<Store>,
    certs: Arc<CertStore>,
    issuer: Arc<Issuer>,
    /// Automatic certificate work for the ACME task.
    acme: watch::Sender<(AcmeSettings, Vec<CertJob>)>,
    /// Latest ACME failure per interface.
    acme_problems: Arc<Mutex<BTreeMap<String, String>>>,
    containers: Containers,
    runtime: Arc<RwLock<RuntimeInfo>>,
    /// Last seen certificate store generation.
    cert_generation: String,
    /// Certificates the running revision needs.
    cert_specs: Vec<CertSpec>,
    cert_notes: Vec<String>,
    bind_errors: Vec<String>,
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
            self.publish_runtime();
            return;
        }
        let state = revision.map(|r| r.state).unwrap_or_default();
        self.drain = state.limits.drain_timeout;
        let plan = compile(&state, self.gateway.upstreams(), &self.containers);
        // Certificates first, so the first handshake on a new port has one.
        self.cert_specs.clone_from(&plan.certs);
        self.load_certificates();
        let errors = self.gateway.apply(&plan).await;
        self.health.update(
            health::jobs(&state, self.gateway.upstreams()),
            self.gateway.upstreams(),
        );
        self.bind_errors = errors.iter().map(ToString::to_string).collect();
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
        self.publish_runtime();
    }

    fn publish_runtime(&self) {
        let mut problems = self.bind_errors.clone();
        problems.extend(self.cert_notes.iter().cloned());
        problems.extend(
            self.acme_problems
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .iter()
                .map(|(i, e)| format!("interface {i}: certificate request failed: {e}")),
        );
        let mut runtime = self.runtime.write().unwrap_or_else(PoisonError::into_inner);
        runtime.revision = self.applied.filter(|id| *id > 0);
        runtime.problems = problems;
    }

    fn reload_certificates_if_changed(&mut self) {
        if self.certs.generation() != self.cert_generation {
            info!("certificates changed; reloading");
            self.load_certificates();
        }
    }

    /// Loads the certificates the running revision needs into the gateway.
    fn load_certificates(&mut self) {
        self.cert_generation = self.certs.generation();
        let (table, notes) = self.certs.table(&self.cert_specs);
        for note in &notes {
            if !self.cert_notes.contains(note) {
                warn!("{note}; serving a self-signed certificate for now");
            }
        }
        self.cert_notes = notes;
        self.gateway.certificates().set(table);
    }
}

/// Issues and renews automatic certificates in the background. New
/// certificates land in the store, and the daemon's next tick loads them.
async fn acme_loop(
    issuer: Arc<Issuer>,
    mut work: watch::Receiver<(AcmeSettings, Vec<CertJob>)>,
    problems: Arc<Mutex<BTreeMap<String, String>>>,
    stop: CancellationToken,
) {
    loop {
        let (settings, jobs) = work.borrow_and_update().clone();
        let mut next = RENEWAL_CHECK;
        let outcomes = if jobs.is_empty() {
            Vec::new()
        } else {
            tokio::select! {
                () = stop.cancelled() => return,
                o = issuer.run(&settings, &jobs) => o,
            }
        };
        {
            let mut problems = problems.lock().unwrap_or_else(PoisonError::into_inner);
            problems.retain(|interface, _| jobs.iter().any(|j| &j.interface == interface));
            for (interface, outcome) in outcomes {
                match outcome {
                    Outcome::Backoff { until } => {
                        next = next.min(until.saturating_duration_since(std::time::Instant::now()));
                    }
                    Outcome::Failed(e) => {
                        problems.insert(interface, e);
                    }
                    Outcome::Valid(_) | Outcome::Issued(_) => {
                        problems.remove(&interface);
                    }
                }
            }
        }
        tokio::select! {
            () = stop.cancelled() => return,
            changed = work.changed() => if changed.is_err() { return },
            () = tokio::time::sleep(next.max(Duration::from_secs(1))) => {}
        }
    }
}
