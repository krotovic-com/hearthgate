//! Keeps the nftables table in line with the desired state, and undoes a
//! firewall change nobody confirms within a minute ("safe apply").
//!
//! A change to the `firewall` section of a revision is installed at once and
//! then waits for confirmation: from the dashboard (which does it
//! automatically over a fresh connection, proving the new rules still let it
//! in) or with `hearthgate firewall confirm`. If no confirmation arrives in
//! time, a new revision restores the previous firewall settings. The
//! pending change is also written to the store, so a restart or reboot in
//! the middle of the window rolls it back too.

use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::{Duration, Instant};

use hearthgate_api::AccessControl;
use hearthgate_firewall::{Inputs, OpenPort, Pending, Perimeter, VpnPolicy};
use hearthgate_model::{DesiredState, Firewall};
use hearthgate_store::Store;
use tracing::{error, info, warn};

/// Store key holding the change waiting for confirmation.
pub const PENDING_KEY: &str = "firewall_pending";
/// Store key the CLI writes to confirm a change (the revision number).
pub const CONFIRM_KEY: &str = "firewall_confirm";
/// How often the LAN and dashboard addresses are checked for changes.
const INPUTS_CHECK: Duration = Duration::from_secs(30);

pub struct FirewallCtl {
    pub perimeter: Arc<Perimeter>,
    store: Arc<Store>,
    access: Option<Arc<AccessControl>>,
    /// Settings known to work: confirmed, or running since start.
    confirmed: Option<Firewall>,
    /// Revision whose change is waiting, as last seen here.
    waiting: Option<i64>,
    /// The state the inputs were last computed from.
    state: DesiredState,
    checked_at: Instant,
    /// Why the firewall could not be installed, for the status page.
    pub problem: Option<String>,
    /// What VPN peers may reach, set by the VPN before each reconcile.
    pub vpn: Option<VpnPolicy>,
    /// Managed Docker, for the subnet of its network.
    pub docker: Option<Arc<hearthgate_docker::Manager>>,
}

impl FirewallCtl {
    pub fn new(
        perimeter: Arc<Perimeter>,
        store: Arc<Store>,
        access: Option<Arc<AccessControl>>,
    ) -> Self {
        Self {
            perimeter,
            store,
            access,
            confirmed: None,
            waiting: None,
            state: DesiredState::default(),
            checked_at: Instant::now(),
            problem: None,
            vpn: None,
            docker: None,
        }
    }

    /// Undoes a change that was still waiting when Hearthgate stopped.
    /// Runs before the first revision is applied.
    pub async fn recover(&mut self) {
        let store = Arc::clone(&self.store);
        let pending = tokio::task::spawn_blocking(move || {
            let pending = store.meta_get(PENDING_KEY).ok().flatten();
            let _ = store.meta_delete(PENDING_KEY);
            let _ = store.meta_delete(CONFIRM_KEY);
            pending
        })
        .await
        .ok()
        .flatten();
        let Some(pending) = pending.and_then(|p| serde_json::from_str::<Pending>(&p).ok()) else {
            return;
        };
        warn!(
            "the firewall change in revision {} was not confirmed before Hearthgate stopped; \
             undoing it",
            pending.revision
        );
        self.restore(pending).await;
    }

    /// Installs the firewall for `state`. `startup` marks the first apply,
    /// which needs no confirmation: those settings were running before.
    pub async fn reconcile(&mut self, state: &DesiredState, revision: Option<i64>, startup: bool) {
        self.state = state.clone();
        let inputs = self.inputs(state);
        let changed = self.confirmed.as_ref() != Some(&state.firewall);
        match self.perimeter.apply(&inputs, false).await {
            Ok(()) => self.problem = None,
            Err(e) => {
                error!("cannot install the firewall: {e}");
                self.problem = Some(format!("firewall: {e}"));
                return;
            }
        }
        if startup || self.confirmed.is_none() || !state.firewall.enabled {
            // Nothing to confirm: the first apply, or the firewall is off.
            self.confirmed = Some(state.firewall.clone());
            if self.perimeter.clear_pending().is_some() {
                self.clear_stored().await;
            }
            self.waiting = None;
            return;
        }
        if changed {
            let previous = match self.perimeter.pending() {
                // A second change while one is waiting: going back still
                // means going back to the last confirmed settings.
                Some(p) => p.previous,
                None => self.confirmed.clone().unwrap_or_default(),
            };
            let pending = Pending::new(revision.unwrap_or(0), previous);
            self.waiting = Some(pending.revision);
            let json = serde_json::to_string(&pending).unwrap_or_default();
            self.perimeter.begin_pending(pending);
            let store = Arc::clone(&self.store);
            let _ = tokio::task::spawn_blocking(move || {
                let _ = store.meta_delete(CONFIRM_KEY);
                store.meta_set(PENDING_KEY, &json)
            })
            .await;
        }
    }

    /// Runs every second: confirmations, the deadline, and changes to the
    /// LAN or dashboard addresses. Returns true when it stored a rollback
    /// revision, so the caller applies it.
    pub async fn tick(&mut self) -> bool {
        if let Some(revision) = self.waiting {
            match self.perimeter.pending() {
                None => {
                    // Confirmed through the API.
                    self.accept().await;
                }
                Some(pending) => {
                    let store = Arc::clone(&self.store);
                    let cli = tokio::task::spawn_blocking(move || store.meta_get(CONFIRM_KEY))
                        .await
                        .ok()
                        .and_then(Result::ok)
                        .flatten();
                    if cli.as_deref() == Some(revision.to_string().as_str()) {
                        info!(
                            "firewall change in revision {revision} confirmed from the command line"
                        );
                        self.perimeter.clear_pending();
                        self.accept().await;
                    } else if pending.expired() {
                        warn!(
                            "the firewall change in revision {revision} was not confirmed within \
                             {}s; undoing it",
                            hearthgate_firewall::CONFIRM_WINDOW.as_secs()
                        );
                        self.perimeter.clear_pending();
                        self.waiting = None;
                        hearthgate_notify::emit(hearthgate_notify::Event::new(
                            hearthgate_model::EventKind::Security,
                            hearthgate_notify::Severity::Warning,
                            format!("firewall-undone:{revision}"),
                            "A firewall change was undone",
                            format!(
                                "Revision {revision} changed the firewall, but nobody confirmed \
                                 the change from a new connection within {}s, so Hearthgate went \
                                 back to the previous rules. If you made the change, check that \
                                 it doesn't lock out the dashboard, and apply it again.",
                                hearthgate_firewall::CONFIRM_WINDOW.as_secs()
                            ),
                        ));
                        self.restore(pending).await;
                        return true;
                    }
                }
            }
        }
        if self.checked_at.elapsed() >= INPUTS_CHECK {
            self.checked_at = Instant::now();
            let inputs = self.inputs(&self.state.clone());
            if self.perimeter.installed().is_some_and(|i| i != inputs) {
                info!("network addresses changed; updating the firewall");
                if let Err(e) = self.perimeter.apply(&inputs, false).await {
                    warn!("cannot update the firewall: {e}");
                }
            }
        }
        false
    }

    async fn accept(&mut self) {
        self.confirmed = Some(self.state.firewall.clone());
        self.waiting = None;
        self.clear_stored().await;
    }

    async fn clear_stored(&self) {
        let store = Arc::clone(&self.store);
        let _ = tokio::task::spawn_blocking(move || {
            let _ = store.meta_delete(PENDING_KEY);
            let _ = store.meta_delete(CONFIRM_KEY);
        })
        .await;
    }

    /// Stores a revision with the firewall settings from before `pending`.
    async fn restore(&mut self, pending: Pending) {
        self.clear_stored().await;
        let store = Arc::clone(&self.store);
        let previous = pending.previous.clone();
        let result = tokio::task::spawn_blocking(move || {
            let mut state = store.current_state()?;
            state.firewall = previous;
            store.apply(
                &state,
                "hearthgate",
                &format!(
                    "Undo firewall change from revision {}: not confirmed within {} seconds",
                    pending.revision,
                    hearthgate_firewall::CONFIRM_WINDOW.as_secs()
                ),
            )
        })
        .await;
        match result {
            Ok(Ok(applied)) => {
                info!(
                    "revision {} restores the previous firewall",
                    applied.meta().id
                );
                // The restored settings were confirmed before.
                self.confirmed = Some(pending.previous);
            }
            Ok(Err(e)) => error!("cannot undo the firewall change: {e}"),
            Err(e) => error!("cannot undo the firewall change: {e}"),
        }
    }

    /// What the ruleset is built from.
    fn inputs(&self, state: &DesiredState) -> Inputs {
        let mut open: BTreeMap<u16, OpenPort> = BTreeMap::new();
        for interface in &state.interfaces {
            for listener in &interface.listeners {
                let entry = open.entry(listener.port).or_insert(OpenPort {
                    port: listener.port,
                    tcp: false,
                    udp: false,
                    rate_limit: interface.rate_limit,
                });
                entry.tcp |= listener.tcp();
                entry.udp |= listener.udp();
                entry.rate_limit = match (entry.rate_limit, interface.rate_limit) {
                    (Some(a), Some(b)) => Some(a.min(b)),
                    // A port shared with an unlimited interface is unlimited.
                    _ => None,
                };
            }
        }
        let admin = self.access.as_ref().map(|access| {
            let current = access.current();
            let mut ports: Vec<u16> = current
                .listen
                .iter()
                .map(std::net::SocketAddr::port)
                .collect();
            ports.sort_unstable();
            ports.dedup();
            (ports, current.allow)
        });
        if self.vpn.is_some() {
            // Peers connect from anywhere.
            let entry = open.entry(state.vpn.port).or_insert(OpenPort {
                port: state.vpn.port,
                tcp: false,
                udp: false,
                rate_limit: None,
            });
            entry.udp = true;
            entry.rate_limit = None;
        }
        // The VPN's own network is not LAN: peers get what their access
        // allows, not everything.
        let subnet = state.vpn.subnet;
        let lan = hearthgate_api::access::detect_lan()
            .into_iter()
            .map(|(_, net)| net)
            .filter(|net| match net {
                ipnet::IpNet::V4(v4) => !subnet.contains(v4) && !v4.contains(&subnet.network()),
                ipnet::IpNet::V6(_) => true,
            })
            .collect();
        Inputs {
            firewall: state.firewall.clone(),
            open: open.into_values().collect(),
            admin,
            lan,
            trusted_interfaces: Vec::new(),
            vpn: self.vpn.clone(),
            containers: self.containers_subnet(),
        }
    }

    /// The managed containers' network, once Docker has created it. Read
    /// on every check, so the guard follows the network when it is created
    /// or recreated.
    fn containers_subnet(&self) -> Option<ipnet::Ipv4Net> {
        self.docker
            .as_ref()?
            .status()
            .subnet?
            .parse::<ipnet::Ipv4Net>()
            .ok()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use hearthgate_firewall::Nft;

    fn ctl() -> (FirewallCtl, Arc<Store>) {
        let store = Arc::new(Store::open_in_memory().unwrap());
        // `true` accepts every script, so the logic runs without nftables.
        let perimeter = Arc::new(Perimeter::new(Nft::new(vec!["true".into()])));
        (FirewallCtl::new(perimeter, Arc::clone(&store), None), store)
    }

    fn with_firewall(enabled: bool, allow_ping: bool) -> DesiredState {
        let mut state = DesiredState::default();
        state.firewall.enabled = enabled;
        state.firewall.allow_ping = allow_ping;
        state
    }

    async fn apply(
        ctl: &mut FirewallCtl,
        store: &Store,
        state: &DesiredState,
        startup: bool,
    ) -> i64 {
        let id = store.apply(state, "test", "change").unwrap().meta().id;
        ctl.reconcile(state, Some(id), startup).await;
        id
    }

    #[tokio::test]
    async fn changes_wait_for_confirmation() {
        let (mut ctl, store) = ctl();
        // What runs at start needs no confirmation.
        apply(&mut ctl, &store, &with_firewall(true, true), true).await;
        assert!(ctl.perimeter.pending().is_none());
        assert!(ctl.perimeter.installed().is_some());

        // A change waits, and is written down in case of a restart.
        let id = apply(&mut ctl, &store, &with_firewall(true, false), false).await;
        assert_eq!(ctl.perimeter.pending().map(|p| p.revision), Some(id));
        assert!(store.meta_get(PENDING_KEY).unwrap().is_some());

        // The dashboard confirms over a new connection.
        ctl.perimeter.confirm(Instant::now()).unwrap();
        assert!(!ctl.tick().await);
        assert!(store.meta_get(PENDING_KEY).unwrap().is_none());

        // The next change is confirmed from the command line.
        let id = apply(&mut ctl, &store, &with_firewall(true, true), false).await;
        store.meta_set(CONFIRM_KEY, &id.to_string()).unwrap();
        assert!(!ctl.tick().await);
        assert!(ctl.perimeter.pending().is_none());
        assert!(store.meta_get(PENDING_KEY).unwrap().is_none());

        // Turning the firewall off never waits.
        apply(&mut ctl, &store, &with_firewall(false, true), false).await;
        assert!(ctl.perimeter.pending().is_none());
        assert!(ctl.perimeter.installed().is_none());
    }

    #[tokio::test]
    async fn unconfirmed_changes_are_undone() {
        let (mut ctl, store) = ctl();
        apply(&mut ctl, &store, &with_firewall(true, true), true).await;
        let id = apply(&mut ctl, &store, &with_firewall(true, false), false).await;
        // A second change in the window still goes back to the first one.
        let mut third = with_firewall(true, false);
        third.firewall.allow_lan = false;
        let id2 = apply(&mut ctl, &store, &third, false).await;
        let mut pending = ctl.perimeter.pending().unwrap();
        assert_eq!(pending.revision, id2);
        assert!(pending.previous.allow_ping && pending.previous.allow_lan);
        assert!(id2 > id);

        // Let the deadline pass.
        pending.deadline_unix = 0;
        ctl.perimeter.begin_pending(pending);
        assert!(ctl.tick().await);
        let current = store.current().unwrap().unwrap();
        assert!(current.meta.message.starts_with("Undo firewall change"));
        assert!(current.state.firewall.allow_ping && current.state.firewall.allow_lan);
        // Applying the restored revision does not start another wait.
        ctl.reconcile(&current.state, Some(current.meta.id), false)
            .await;
        assert!(ctl.perimeter.pending().is_none());
    }

    #[tokio::test]
    async fn a_restart_during_the_window_undoes_the_change() {
        let (mut ctl, store) = ctl();
        apply(&mut ctl, &store, &with_firewall(true, true), true).await;
        apply(&mut ctl, &store, &with_firewall(true, false), false).await;
        // Hearthgate stops here. A new process starts with the same store.
        let perimeter = Arc::new(Perimeter::new(Nft::new(vec!["true".into()])));
        let mut fresh = FirewallCtl::new(perimeter, Arc::clone(&store), None);
        fresh.recover().await;
        let current = store.current().unwrap().unwrap();
        assert!(current.state.firewall.allow_ping);
        assert!(store.meta_get(PENDING_KEY).unwrap().is_none());
    }
}
