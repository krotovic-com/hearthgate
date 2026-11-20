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
use crate::firewall::FirewallCtl;

/// How often the store is checked for a new revision.
const POLL_INTERVAL: Duration = Duration::from_secs(1);
/// How long to wait before retrying sockets that could not be bound.
const BIND_RETRY: Duration = Duration::from_secs(10);
/// How often certificates are checked for renewal.
const RENEWAL_CHECK: Duration = Duration::from_secs(3600);

/// Starts the gateway and blocks until it is told to stop.
pub fn run(bootstrap: &Bootstrap) -> Result<std::process::ExitCode> {
    let logs = Arc::new(hearthgate_api::LogBuffer::default());
    init_logging(&bootstrap.log, &logs);
    let runtime = tokio::runtime::Runtime::new().context("cannot start the async runtime")?;
    runtime.block_on(serve(bootstrap.clone(), logs))?;
    Ok(std::process::ExitCode::SUCCESS)
}

fn init_logging(filter: &str, logs: &Arc<hearthgate_api::LogBuffer>) {
    use tracing_subscriber::Layer as _;
    use tracing_subscriber::layer::SubscriberExt;
    use tracing_subscriber::util::SubscriberInitExt;
    let filter = EnvFilter::try_from_env("HEARTHGATE_LOG")
        .or_else(|_| EnvFilter::try_new(filter))
        .unwrap_or_else(|_| EnvFilter::new("info"));
    // journald adds its own timestamps.
    let under_systemd = std::env::var_os("JOURNAL_STREAM").is_some();
    let fmt = tracing_subscriber::fmt::layer()
        .with_target(false)
        .with_ansi(!under_systemd);
    let fmt = if under_systemd {
        fmt.without_time().boxed()
    } else {
        fmt.boxed()
    };
    // The dashboard shows the same lines as the service log.
    let _ = tracing_subscriber::registry()
        .with(filter)
        .with(fmt)
        .with(hearthgate_api::LogLayer(Arc::clone(logs)))
        .try_init();
}

/// Tells systemd about state changes. A no-op outside systemd.
fn notify(state: &[NotifyState<'_>]) {
    let _ = sd_notify::notify(false, state);
}

async fn serve(bootstrap: Bootstrap, logs: Arc<hearthgate_api::LogBuffer>) -> Result<()> {
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
    let live = Arc::new(hearthgate_api::Live::new(logs));
    let perimeter = Arc::new(hearthgate_firewall::Perimeter::new(
        hearthgate_firewall::Nft::default(),
    ));
    let router = Arc::new(hearthgate_upnp::Router::default());
    let vpn_runtime = Arc::new(hearthgate_vpn::Runtime::new(
        hearthgate_vpn::Device::new(hearthgate_vpn::INTERFACE, hearthgate_vpn::Tools::default()),
        Some(bootstrap.data_dir.join("vpn")),
    ));
    hearthgate_api::live::spawn_sampler(Arc::clone(&live), Arc::clone(&gateway), stop.clone());
    let access = if bootstrap.admin.disabled() {
        info!("admin API is off (admin.listen = [])");
        None
    } else {
        Some(
            start_admin(
                &bootstrap,
                &store,
                &gateway,
                &certs,
                &runtime,
                &api_changed,
                &live,
                &perimeter,
                &router,
                &vpn_runtime,
                &stop,
            )
            .await?,
        )
    };

    let (firewall_tx, firewall_rx) = watch::channel(hearthgate_model::Firewall::default());
    tokio::spawn(hearthgate_firewall::feeds::run(
        Arc::clone(&perimeter),
        bootstrap.data_dir.join("blocklists"),
        firewall_rx.clone(),
        stop.clone(),
    ));
    {
        // Abuse reports from the proxy feed the automatic bans.
        let (signal_tx, mut signal_rx) = tokio::sync::mpsc::channel(4096);
        gateway.abuse().connect(signal_tx);
        let (report_tx, report_rx) = tokio::sync::mpsc::channel(4096);
        tokio::spawn(async move {
            while let Some((ip, signal)) = signal_rx.recv().await {
                let signal: hearthgate_proxy::abuse::Signal = signal;
                if report_tx
                    .send((ip, signal.points(), signal.describe()))
                    .await
                    .is_err()
                {
                    return;
                }
            }
        });
        tokio::spawn(hearthgate_firewall::bans::run(
            Arc::clone(&perimeter),
            report_rx,
            firewall_rx,
            stop.clone(),
        ));
    }
    let (upnp_tx, upnp_rx) = watch::channel(hearthgate_upnp::Wanted::default());
    let upnp_task = tokio::spawn(hearthgate_upnp::run(
        Arc::clone(&router),
        upnp_rx,
        hearthgate_upnp::Options::default(),
        stop.clone(),
    ));
    let (vpn, dns_settings) = crate::vpn::VpnCtl::new(Arc::clone(&vpn_runtime));
    tokio::spawn(hearthgate_vpn::dns::run(dns_settings, stop.clone()));
    let mut firewall = FirewallCtl::new(Arc::clone(&perimeter), Arc::clone(&store), access.clone());
    firewall.recover().await;
    let mut daemon = Daemon {
        firewall,
        vpn,
        access,
        firewall_settings: firewall_tx,
        upnp: upnp_tx,
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
            _ = tick.tick() => {
                // A rollback is a new revision, which reconcile picks up.
                daemon.firewall.tick().await;
                let vpn_check = daemon.vpn.check().await;
                daemon.reconcile(vpn_check).await;
            }
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
    // Give the router a moment to drop its forwards.
    let _ = tokio::time::timeout(Duration::from_secs(10), upnp_task).await;
    daemon.vpn.stop().await;
    daemon.health.stop();
    info!(
        "stopping; open connections get {:?} to finish",
        daemon.drain
    );
    daemon.gateway.shutdown(daemon.drain).await;
    Ok(())
}

struct Daemon {
    firewall: FirewallCtl,
    vpn: crate::vpn::VpnCtl,
    /// The dashboard's listeners, which follow the VPN address.
    access: Option<Arc<hearthgate_api::AccessControl>>,
    /// Firewall settings for the blocklist and ban tasks.
    firewall_settings: watch::Sender<hearthgate_model::Firewall>,
    /// Ports the router should forward.
    upnp: watch::Sender<hearthgate_upnp::Wanted>,
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
        self.firewall.vpn = self.vpn.reconcile(&state, &self.containers).await;
        self.firewall
            .reconcile(&state, id, self.applied.is_none())
            .await;
        if let Some(access) = &self.access {
            // The dashboard is reachable over the VPN too, at its address.
            access.refresh(&self.store).await;
        }
        let wanted = upnp_wanted(&state);
        self.upnp.send_if_modified(|w| {
            let changed = *w != wanted;
            *w = wanted;
            changed
        });
        self.firewall_settings.send_if_modified(|fw| {
            let changed = *fw != state.firewall;
            fw.clone_from(&state.firewall);
            changed
        });
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
        problems.extend(self.firewall.problem.iter().cloned());
        problems.extend(self.vpn.problems());
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

/// The ports the router should forward: every public port.
fn upnp_wanted(state: &DesiredState) -> hearthgate_upnp::Wanted {
    use hearthgate_upnp::Protocol;
    let mut ports = Vec::new();
    for listener in state.interfaces.iter().flat_map(|i| &i.listeners) {
        if listener.tcp() {
            ports.push((listener.port, Protocol::Tcp));
        }
        if listener.udp() {
            ports.push((listener.port, Protocol::Udp));
        }
    }
    if state.vpn.enabled {
        ports.push((state.vpn.port, Protocol::Udp));
    }
    ports.sort_unstable();
    ports.dedup();
    hearthgate_upnp::Wanted {
        enabled: state.router.upnp,
        ports,
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

/// Starts the dashboard and admin API on the configured or detected
/// addresses.
#[allow(clippy::too_many_arguments)]
async fn start_admin(
    bootstrap: &Bootstrap,
    store: &Arc<Store>,
    gateway: &Arc<Gateway>,
    certs: &Arc<CertStore>,
    runtime: &Arc<RwLock<RuntimeInfo>>,
    changed: &Arc<Notify>,
    live: &Arc<hearthgate_api::Live>,
    perimeter: &Arc<hearthgate_firewall::Perimeter>,
    router: &Arc<hearthgate_upnp::Router>,
    vpn: &Arc<hearthgate_vpn::Runtime>,
    stop: &CancellationToken,
) -> Result<Arc<hearthgate_api::AccessControl>> {
    use hearthgate_api::access::{Resolved, Source, TlsMode};
    let admin = &bootstrap.admin;
    let token = hearthgate_api::load_or_create_token(&bootstrap.data_dir)
        .context("cannot read or create the admin token")?;
    let tls_mode = if admin.https {
        match (&admin.certificate, &admin.private_key) {
            (Some(cert), Some(key)) => {
                let tls = hearthgate_api::tls::from_files(cert, key)
                    .map_err(anyhow::Error::msg)
                    .context("cannot use admin.certificate and admin.private_key")?;
                info!(
                    certificate = %tls.cert_path.display(),
                    "dashboard certificate fingerprint (SHA-256): {}",
                    tls.fingerprint
                );
                TlsMode::Fixed(Arc::new(tls))
            }
            (None, None) => TlsMode::SelfSigned(bootstrap.data_dir.clone()),
            _ => anyhow::bail!("set both admin.certificate and admin.private_key, or neither"),
        }
    } else {
        warn!("admin.https is off: the dashboard password travels unencrypted");
        TlsMode::Off
    };
    let source = if admin.fixed() {
        Source::Config(Resolved {
            listen: admin.listen_or_default(),
            allow: admin.allow_or_default(),
        })
    } else {
        Source::Dashboard
    };
    let https = !matches!(tls_mode, TlsMode::Off);
    let access = Arc::new(hearthgate_api::AccessControl::new(
        source,
        tls_mode,
        stop.clone(),
    ));
    let api = ApiState {
        store: Arc::clone(store),
        gateway: Arc::clone(gateway),
        certs: Arc::clone(certs),
        runtime: Arc::clone(runtime),
        changed: Arc::clone(changed),
        token: token.into(),
        open_metrics: admin.metrics_without_token,
        secure_cookies: https,
        auth: Arc::default(),
        access: Arc::clone(&access),
        live: Arc::clone(live),
        perimeter: Arc::clone(perimeter),
        router: Arc::clone(router),
        vpn: Arc::clone(vpn),
    };
    let wanted = {
        let store = Arc::clone(store);
        let access = Arc::clone(&access);
        tokio::task::spawn_blocking(move || access.wanted(&store))
            .await
            .context("cannot read the admin access settings")?
    };
    if wanted.listen.iter().all(|a| a.ip().is_loopback()) && !admin.fixed() {
        warn!(
            "no LAN address found: the dashboard is only reachable from this machine (or set \
             admin.listen in the bootstrap file)"
        );
    }
    access.start(api.router(), wanted).await;
    if let Some(code) = hearthgate_api::setup_code(store)? {
        let current = access.current();
        let scheme = if https { "https" } else { "http" };
        let addr = current
            .listen
            .iter()
            .find(|a| !a.ip().is_loopback())
            .or_else(|| current.listen.first());
        let url = addr.map(|a| format!("{scheme}://{a}/")).unwrap_or_default();
        info!(
            "no dashboard account yet: open {url} and create one with setup code {code} \
             (`hearthgate setup-code` shows it again)"
        );
    }
    Ok(access)
}
