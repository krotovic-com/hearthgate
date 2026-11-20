//! Read-only Docker discovery: resolves container names to addresses and
//! follows restarts, so `docker` services keep working when a container
//! gets a new IP.
//!
//! Hearthgate only lists containers and watches events here. It never
//! starts, stops or changes anything (that is Phase 5, and opt-in).

use std::collections::HashMap;
use std::net::IpAddr;
use std::sync::{Arc, RwLock};
use std::time::Duration;

use bollard::Docker;
use bollard::query_parameters::{EventsOptionsBuilder, ListContainersOptionsBuilder};
use futures_util::StreamExt;
use hearthgate_proxy::ContainerLookup;
use tokio::sync::Notify;
use tokio_util::sync::CancellationToken;
use tracing::{debug, info, warn};

/// Container name to (network name to address).
type Addresses = HashMap<String, Vec<(String, IpAddr)>>;

/// The latest known container addresses. Cheap to clone.
#[derive(Debug, Clone, Default)]
pub struct Containers {
    map: Arc<RwLock<Addresses>>,
    /// Woken whenever an address changes.
    pub changed: Arc<Notify>,
}

impl ContainerLookup for Containers {
    fn container_ip(&self, container: &str, network: Option<&str>) -> Option<IpAddr> {
        let map = self
            .map
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let networks = map.get(container)?;
        match network {
            Some(n) => networks
                .iter()
                .find(|(name, _)| name == n)
                .map(|(_, ip)| *ip),
            None => networks.first().map(|(_, ip)| *ip),
        }
    }
}

impl Containers {
    fn replace(&self, next: Addresses) -> bool {
        let mut map = self
            .map
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if *map == next {
            return false;
        }
        *map = next;
        true
    }

    /// Sets addresses directly, for tests.
    pub fn set_for_test(&self, container: &str, network: &str, ip: IpAddr) {
        self.map
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .insert(container.to_owned(), vec![(network.to_owned(), ip)]);
        self.changed.notify_one();
    }
}

/// Watches Docker at `socket` until `cancel` fires, keeping `containers`
/// current. Reconnects when Docker restarts.
pub async fn watch(socket: String, containers: Containers, cancel: CancellationToken) {
    let mut logged_unavailable = false;
    loop {
        match watch_once(&socket, &containers, &cancel).await {
            Ok(()) => return,
            Err(e) => {
                if logged_unavailable {
                    debug!("docker unavailable: {e}");
                } else {
                    warn!(
                        "cannot reach Docker at {socket}: {e}; docker services stay unresolved until it is back"
                    );
                    logged_unavailable = true;
                }
            }
        }
        tokio::select! {
            () = cancel.cancelled() => return,
            () = tokio::time::sleep(Duration::from_secs(10)) => {}
        }
    }
}

async fn watch_once(
    socket: &str,
    containers: &Containers,
    cancel: &CancellationToken,
) -> Result<(), bollard::errors::Error> {
    let docker = Docker::connect_with_unix(socket, 30, bollard::API_DEFAULT_VERSION)?;
    docker.ping().await?;
    info!("watching Docker containers at {socket}");
    refresh(&docker, containers).await?;
    let filters: HashMap<&str, Vec<&str>> = HashMap::from([("type", vec!["container", "network"])]);
    let mut events = docker.events(Some(
        EventsOptionsBuilder::default().filters(&filters).build(),
    ));
    loop {
        tokio::select! {
            () = cancel.cancelled() => return Ok(()),
            event = events.next() => match event {
                Some(Ok(_)) => {
                    // Events can come in bursts (a stack restarting); settle
                    // briefly, then read the full picture once.
                    tokio::time::sleep(Duration::from_millis(200)).await;
                    refresh(&docker, containers).await?;
                }
                Some(Err(e)) => return Err(e),
                None => return Err(bollard::errors::Error::DockerStreamError { error: "event stream ended".to_owned() }),
            },
        }
    }
}

async fn refresh(docker: &Docker, containers: &Containers) -> Result<(), bollard::errors::Error> {
    let list = docker
        .list_containers(Some(
            ListContainersOptionsBuilder::default().all(false).build(),
        ))
        .await?;
    let mut next = Addresses::new();
    for c in list {
        let networks: Vec<(String, IpAddr)> = c
            .network_settings
            .and_then(|n| n.networks)
            .map(|nets| {
                let mut v: Vec<(String, IpAddr)> = nets
                    .into_iter()
                    .filter_map(|(name, ep)| {
                        let ip = ep.ip_address.filter(|s| !s.is_empty())?.parse().ok()?;
                        Some((name, ip))
                    })
                    .collect();
                v.sort();
                v
            })
            .unwrap_or_default();
        if networks.is_empty() {
            continue;
        }
        for name in c.names.unwrap_or_default() {
            next.insert(name.trim_start_matches('/').to_owned(), networks.clone());
        }
    }
    if containers.replace(next) {
        debug!("container addresses changed");
        containers.changed.notify_one();
    }
    Ok(())
}
