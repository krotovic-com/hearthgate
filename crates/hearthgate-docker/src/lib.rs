//! Managed Docker: containers and Compose stacks that Hearthgate creates,
//! keeps running, updates and removes.
//!
//! Containers are created through the Docker API and stacks run with the
//! `docker compose` command. Both join the `hearthgate` network, so routes
//! reach them without publishing host ports. Hearthgate only ever touches
//! containers it created (labelled `com.hearthgate.container`) and stacks
//! it keeps files for.
//!
//! The `hearthgate` network lets packets routed in from other interfaces
//! reach its containers (Docker's `nat-unprotected` gateway mode), so VPN
//! peers can connect to them. Docker 28 and later otherwise drop such
//! packets before any other firewall sees them. Hearthgate's own nftables
//! table decides who gets in instead: VPN peers by their access, and
//! nobody else.

pub mod compose;
pub mod spec;

use std::collections::{BTreeMap, HashMap};
use std::net::IpAddr;
use std::path::{Path, PathBuf};
use std::sync::{PoisonError, RwLock};

use bollard::Docker;
use bollard::models::{
    EndpointSettings, NetworkConnectRequest, NetworkCreateRequest, NetworkDisconnectRequest,
};
use bollard::query_parameters::{
    CreateContainerOptionsBuilder, CreateImageOptionsBuilder, InspectContainerOptions,
    InspectNetworkOptions, ListContainersOptionsBuilder, LogsOptionsBuilder,
    RemoveContainerOptionsBuilder, RestartContainerOptionsBuilder, StartContainerOptions,
    StatsOptionsBuilder, StopContainerOptionsBuilder,
};
use futures_util::StreamExt;
use hearthgate_model::{Container, DesiredState, Stack};
use serde::Serialize;
use tokio::sync::Mutex;
use tracing::{info, warn};

pub use compose::{Compose, StackDir};
pub use spec::{CONTAINER_LABEL, NETWORK_LABEL, SPEC_LABEL, create_body, spec_hash};

/// The Docker network managed containers join.
pub const NETWORK: &str = "hearthgate";
/// Docker's bridge option for how traffic reaches the network's containers.
pub const GATEWAY_MODE: &str = "com.docker.network.bridge.gateway_mode_ipv4";
/// The mode the `hearthgate` network uses: masquerade on the way out, and
/// packets routed in from other interfaces (the VPN) are not dropped.
pub const GATEWAY_MODE_VALUE: &str = "nat-unprotected";
/// The first Docker API version (Docker 28.0) with that mode.
const GATEWAY_MODE_API: (u32, u32) = (1, 48);
/// Label Compose puts on a project's containers.
const PROJECT_LABEL: &str = "com.docker.compose.project";
const SERVICE_LABEL: &str = "com.docker.compose.service";

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("Docker is not available")]
    Unavailable,
    #[error("no managed container is called `{0}`")]
    NoSuchContainer(String),
    #[error("no stack is called `{0}`")]
    NoSuchStack(String),
    #[error("{0}")]
    Docker(#[from] bollard::errors::Error),
    #[error("{0}")]
    Other(String),
}

/// Who a container belongs to.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "kind", rename_all = "lowercase")]
pub enum Owner {
    /// A container from `containers`.
    Container,
    /// A service of a Compose stack.
    Stack { stack: String, service: String },
}

/// A managed container as Docker reports it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ContainerInfo {
    /// The container's name in Docker.
    pub name: String,
    pub owner: Owner,
    pub image: String,
    /// `running`, `exited`, `created`, `restarting`, `paused`, `dead`,
    /// `missing` (not created yet) or `pulling`.
    pub state: String,
    /// Docker's own description, such as "Up 3 minutes".
    pub status: String,
    /// `healthy`, `unhealthy` or `starting` for images with a health check.
    pub health: Option<String>,
    /// Address on the `hearthgate` network.
    pub address: Option<IpAddr>,
    /// Unix seconds.
    pub created: Option<i64>,
}

/// A managed stack.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct StackInfo {
    pub name: String,
    pub running: usize,
    pub total: usize,
    /// The last error from `docker compose`.
    pub error: Option<String>,
}

/// Managed Docker as it is now.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct Status {
    /// Docker answered.
    pub available: bool,
    /// Why it is not available, or why the last change failed.
    pub error: Option<String>,
    /// Subnet of the `hearthgate` network.
    pub subnet: Option<String>,
    pub containers: Vec<ContainerInfo>,
    pub stacks: Vec<StackInfo>,
    /// Things that need the user's attention.
    pub problems: Vec<String>,
}

/// Recent output of a container.
#[derive(Debug, Clone, Serialize)]
pub struct LogLine {
    /// RFC 3339 time from Docker.
    pub time: String,
    pub stderr: bool,
    pub text: String,
}

/// Resource use of a container.
#[derive(Debug, Clone, Serialize)]
pub struct Stats {
    pub cpu_percent: f64,
    pub memory_bytes: u64,
    pub memory_limit: u64,
    pub restarts: i64,
    pub started_at: Option<String>,
}

/// Whether a newer image is available.
#[derive(Debug, Clone, Serialize)]
pub struct ImageUpdate {
    pub image: String,
    /// Containers that use it.
    pub used_by: Vec<String>,
    pub update_available: bool,
    pub error: Option<String>,
}

/// Runs managed containers and stacks.
#[derive(Debug)]
pub struct Manager {
    docker: Option<Docker>,
    compose: Compose,
    stacks_dir: PathBuf,
    status: RwLock<Status>,
    /// One change at a time: a pull can take minutes.
    busy: Mutex<()>,
    /// Last error per stack.
    stack_errors: RwLock<BTreeMap<String, String>>,
}

impl Default for Manager {
    /// A manager without Docker, for tests.
    fn default() -> Self {
        Self::new(None, Path::new("/nonexistent"))
    }
}

impl Manager {
    /// `socket` is the Docker socket; `None` turns managed Docker off.
    #[must_use]
    pub fn new(socket: Option<&Path>, data_dir: &Path) -> Self {
        let docker = socket.and_then(|s| {
            Docker::connect_with_unix(&s.to_string_lossy(), 120, bollard::API_DEFAULT_VERSION).ok()
        });
        Self {
            docker,
            compose: Compose {
                host: socket.map(|s| format!("unix://{}", s.display())),
                ..Compose::default()
            },
            stacks_dir: data_dir.join("stacks"),
            status: RwLock::default(),
            busy: Mutex::new(()),
            stack_errors: RwLock::default(),
        }
    }

    #[must_use]
    pub fn enabled(&self) -> bool {
        self.docker.is_some()
    }

    #[must_use]
    pub fn status(&self) -> Status {
        self.status
            .read()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
    }

    fn docker(&self) -> Result<&Docker, Error> {
        self.docker.as_ref().ok_or(Error::Unavailable)
    }

    fn set_state(&self, name: &str, state: &str) {
        let mut status = self.status.write().unwrap_or_else(PoisonError::into_inner);
        if let Some(c) = status.containers.iter_mut().find(|c| c.name == name) {
            state.clone_into(&mut c.state);
        }
    }

    /// Makes Docker match `state`, then refreshes the status. Returns what
    /// needs the user's attention.
    pub async fn reconcile(&self, state: &DesiredState) -> Vec<String> {
        if self.docker.is_none() {
            return Vec::new();
        }
        let _guard = self.busy.lock().await;
        let mut problems = Vec::new();
        if let Err(e) = self.apply(state, &mut problems).await {
            warn!("managed Docker: {e}");
            problems.push(format!("Docker: {e}"));
        }
        self.refresh_locked(state, problems.clone()).await;
        problems
    }

    async fn apply(&self, state: &DesiredState, problems: &mut Vec<String>) -> Result<(), Error> {
        let docker = self.docker()?;
        docker.ping().await?;
        if state.containers.is_empty() && state.stacks.is_empty() && !self.has_managed().await? {
            return Ok(());
        }
        self.ensure_network().await?;
        for container in &state.containers {
            if let Err(e) = self.apply_container(container, false).await {
                problems.push(format!("container {}: {e}", container.name));
            }
        }
        self.remove_orphans(state).await?;
        for stack in &state.stacks {
            let error = self.apply_stack(stack, false).await.err();
            let mut errors = self
                .stack_errors
                .write()
                .unwrap_or_else(PoisonError::into_inner);
            match error {
                Some(e) => {
                    problems.push(format!("stack {}: {e}", stack.name));
                    errors.insert(stack.name.clone(), e);
                }
                None => {
                    errors.remove(&stack.name);
                }
            }
        }
        self.remove_stacks(state).await;
        self.connect_stacks(state).await?;
        Ok(())
    }

    async fn has_managed(&self) -> Result<bool, Error> {
        let stacks = std::fs::read_dir(&self.stacks_dir)
            .is_ok_and(|mut d| d.any(|e| e.is_ok_and(|e| e.path().is_dir())));
        Ok(stacks || !self.managed_containers().await?.is_empty())
    }

    async fn ensure_network(&self) -> Result<(), Error> {
        let docker = self.docker()?;
        match docker
            .inspect_network(NETWORK, None::<InspectNetworkOptions>)
            .await
        {
            Ok(network) => {
                let mode = network
                    .options
                    .as_ref()
                    .and_then(|o| o.get(GATEWAY_MODE))
                    .map(String::as_str);
                let ours = network
                    .labels
                    .as_ref()
                    .is_some_and(|l| l.contains_key(NETWORK_LABEL));
                if mode == Some(GATEWAY_MODE_VALUE) || !ours || !self.has_gateway_mode().await {
                    return Ok(());
                }
                // A network's options are fixed when it is created, so one
                // from an older Hearthgate is created again, with the same
                // containers on it. A network someone else made is left
                // alone.
                let attached: Vec<String> =
                    network.containers.unwrap_or_default().into_keys().collect();
                info!(
                    "recreating the Docker network `{NETWORK}` so VPN peers can reach its \
                     containers ({} attached)",
                    attached.len()
                );
                for id in &attached {
                    docker
                        .disconnect_network(
                            NETWORK,
                            NetworkDisconnectRequest {
                                container: Some(id.clone()),
                                force: Some(true),
                            },
                        )
                        .await?;
                }
                docker.remove_network(NETWORK).await?;
                self.create_network(true).await?;
                for id in attached {
                    if let Err(e) = docker
                        .connect_network(
                            NETWORK,
                            NetworkConnectRequest {
                                container: Some(id.clone()),
                                endpoint_config: Some(EndpointSettings::default()),
                            },
                        )
                        .await
                    {
                        warn!("cannot join container {id} to `{NETWORK}` again: {e}");
                    }
                }
                Ok(())
            }
            Err(bollard::errors::Error::DockerResponseServerError {
                status_code: 404, ..
            }) => {
                let mode = self.has_gateway_mode().await;
                self.create_network(mode).await
            }
            Err(e) => Err(e.into()),
        }
    }

    async fn create_network(&self, gateway_mode: bool) -> Result<(), Error> {
        let options = gateway_mode
            .then(|| HashMap::from([(GATEWAY_MODE.to_owned(), GATEWAY_MODE_VALUE.to_owned())]));
        if !gateway_mode {
            warn!(
                "this Docker is older than 28.0, so VPN peers cannot reach containers on \
                 `{NETWORK}` directly; update Docker to fix that"
            );
        }
        self.docker()?
            .create_network(NetworkCreateRequest {
                name: NETWORK.into(),
                driver: Some("bridge".into()),
                labels: Some(HashMap::from([(NETWORK_LABEL.into(), "true".into())])),
                options,
                ..NetworkCreateRequest::default()
            })
            .await?;
        info!("created the Docker network `{NETWORK}`");
        Ok(())
    }

    /// Whether Docker knows the `nat-unprotected` gateway mode.
    async fn has_gateway_mode(&self) -> bool {
        let Ok(docker) = self.docker() else {
            return false;
        };
        docker
            .version()
            .await
            .ok()
            .and_then(|v| v.api_version)
            .is_some_and(|v| api_at_least(&v, GATEWAY_MODE_API))
    }

    async fn image_present(&self, image: &str) -> Result<bool, Error> {
        match self.docker()?.inspect_image(image).await {
            Ok(_) => Ok(true),
            Err(bollard::errors::Error::DockerResponseServerError {
                status_code: 404, ..
            }) => Ok(false),
            Err(e) => Err(e.into()),
        }
    }

    async fn pull(&self, image: &str) -> Result<(), Error> {
        info!("pulling {image}");
        let mut stream = self.docker()?.create_image(
            Some(
                CreateImageOptionsBuilder::default()
                    .from_image(image)
                    .build(),
            ),
            None,
            None,
        );
        while let Some(item) = stream.next().await {
            item?;
        }
        Ok(())
    }

    /// Creates, recreates or starts one container. With `pull`, fetches the
    /// image first even if it is present (an update).
    async fn apply_container(&self, container: &Container, pull: bool) -> Result<(), Error> {
        let docker = self.docker()?;
        let hash = spec_hash(container);
        let existing = match docker
            .inspect_container(&container.name, None::<InspectContainerOptions>)
            .await
        {
            Ok(c) => Some(c),
            Err(bollard::errors::Error::DockerResponseServerError {
                status_code: 404, ..
            }) => None,
            Err(e) => return Err(e.into()),
        };
        let mut recreate = existing.is_none();
        if let Some(c) = &existing {
            let labels = c
                .config
                .as_ref()
                .and_then(|cfg| cfg.labels.clone())
                .unwrap_or_default();
            if labels.get(CONTAINER_LABEL) != Some(&container.name) {
                return Err(Error::Other(format!(
                    "a container called `{}` already exists and Hearthgate does not manage it; \
                     rename or remove it",
                    container.name
                )));
            }
            if labels.get(SPEC_LABEL) != Some(&hash) {
                recreate = true;
            }
        }
        if pull || !self.image_present(&container.image).await? {
            self.set_state(&container.name, "pulling");
            let before = docker
                .inspect_image(&container.image)
                .await
                .ok()
                .and_then(|i| i.id);
            self.pull(&container.image).await?;
            let after = docker
                .inspect_image(&container.image)
                .await
                .ok()
                .and_then(|i| i.id);
            if before != after {
                recreate = true;
            }
        }
        if recreate {
            if existing.is_some() {
                docker
                    .remove_container(
                        &container.name,
                        Some(RemoveContainerOptionsBuilder::default().force(true).build()),
                    )
                    .await?;
            }
            docker
                .create_container(
                    Some(
                        CreateContainerOptionsBuilder::default()
                            .name(&container.name)
                            .build(),
                    ),
                    create_body(container, NETWORK),
                )
                .await?;
            info!(container = %container.name, image = %container.image, "created container");
        }
        let status = docker
            .inspect_container(&container.name, None::<InspectContainerOptions>)
            .await?
            .state
            .and_then(|s| s.status)
            .map(|s| s.to_string())
            .unwrap_or_default();
        // A created container starts once. One that exited on its own is
        // left to its restart policy.
        if container.running && status == "created" {
            docker
                .start_container(&container.name, None::<StartContainerOptions>)
                .await?;
        } else if !container.running && (status == "running" || status == "restarting") {
            docker
                .stop_container(
                    &container.name,
                    Some(StopContainerOptionsBuilder::default().t(10).build()),
                )
                .await?;
        }
        Ok(())
    }

    async fn managed_containers(&self) -> Result<Vec<bollard::models::ContainerSummary>, Error> {
        let filters: HashMap<&str, Vec<&str>> = HashMap::from([("label", vec![CONTAINER_LABEL])]);
        Ok(self
            .docker()?
            .list_containers(Some(
                ListContainersOptionsBuilder::default()
                    .all(true)
                    .filters(&filters)
                    .build(),
            ))
            .await?)
    }

    async fn remove_orphans(&self, state: &DesiredState) -> Result<(), Error> {
        for c in self.managed_containers().await? {
            let Some(name) = c.labels.as_ref().and_then(|l| l.get(CONTAINER_LABEL)) else {
                continue;
            };
            if state.containers.iter().any(|d| &d.name == name) {
                continue;
            }
            let id = c.id.clone().unwrap_or_default();
            self.docker()?
                .remove_container(
                    &id,
                    Some(RemoveContainerOptionsBuilder::default().force(true).build()),
                )
                .await?;
            info!(container = %name, "removed container (its volumes are kept)");
        }
        Ok(())
    }

    async fn apply_stack(&self, stack: &Stack, pull: bool) -> Result<(), String> {
        let files = StackDir::new(&self.stacks_dir, &stack.name);
        let hash = files
            .write(stack)
            .map_err(|e| format!("cannot write {}: {e}", files.dir.display()))?;
        if !pull && files.applied().as_deref() == Some(hash.as_str()) {
            return Ok(());
        }
        if stack.running {
            if pull {
                self.compose.run(&files, &["pull", "--quiet"]).await?;
            }
            self.compose
                .run(
                    &files,
                    &["up", "--detach", "--remove-orphans", "--quiet-pull"],
                )
                .await?;
            info!(stack = %stack.name, "stack is up");
        } else {
            self.compose.run(&files, &["stop"]).await?;
            info!(stack = %stack.name, "stack stopped");
        }
        files
            .set_applied(&hash)
            .map_err(|e| format!("cannot write {}: {e}", files.dir.display()))
    }

    async fn remove_stacks(&self, state: &DesiredState) {
        let Ok(dir) = std::fs::read_dir(&self.stacks_dir) else {
            return;
        };
        for entry in dir.flatten() {
            let name = entry.file_name().to_string_lossy().into_owned();
            if !entry.path().is_dir() || state.stacks.iter().any(|s| s.name == name) {
                continue;
            }
            let files = StackDir::new(&self.stacks_dir, &name);
            match self
                .compose
                .run(&files, &["down", "--remove-orphans"])
                .await
            {
                Ok(_) => {
                    info!(stack = %name, "removed stack (its volumes are kept)");
                    if let Err(e) = std::fs::remove_dir_all(&files.dir) {
                        warn!("cannot remove {}: {e}", files.dir.display());
                    }
                }
                Err(e) => warn!("cannot remove stack {name}: {e}"),
            }
        }
    }

    /// Joins every container of every stack to the `hearthgate` network,
    /// including ones Compose recreated.
    async fn connect_stacks(&self, state: &DesiredState) -> Result<(), Error> {
        if state.stacks.is_empty() {
            return Ok(());
        }
        let docker = self.docker()?;
        let projects: Vec<String> = state
            .stacks
            .iter()
            .map(|s| format!("{PROJECT_LABEL}={}", s.name))
            .collect();
        for project in projects {
            let filters: HashMap<&str, Vec<&str>> =
                HashMap::from([("label", vec![project.as_str()])]);
            let list = docker
                .list_containers(Some(
                    ListContainersOptionsBuilder::default()
                        .all(true)
                        .filters(&filters)
                        .build(),
                ))
                .await?;
            for c in list {
                let joined = c
                    .network_settings
                    .as_ref()
                    .and_then(|n| n.networks.as_ref())
                    .is_some_and(|n| n.contains_key(NETWORK));
                if joined {
                    continue;
                }
                let Some(id) = c.id else { continue };
                if let Err(e) = docker
                    .connect_network(
                        NETWORK,
                        NetworkConnectRequest {
                            container: Some(id),
                            endpoint_config: Some(EndpointSettings::default()),
                        },
                    )
                    .await
                {
                    warn!("cannot join a stack container to `{NETWORK}`: {e}");
                }
            }
        }
        Ok(())
    }

    /// Reads the status again without changing anything.
    pub async fn refresh(&self, state: &DesiredState) {
        if self.docker.is_none() {
            return;
        }
        let _guard = self.busy.lock().await;
        let problems = self.status().problems;
        self.refresh_locked(state, problems).await;
    }

    async fn refresh_locked(&self, state: &DesiredState, problems: Vec<String>) {
        let next = match self.read_status(state).await {
            Ok(mut s) => {
                s.problems = problems;
                s
            }
            Err(e) => Status {
                available: false,
                error: Some(e.to_string()),
                problems: vec![format!("Docker: {e}")],
                ..Status::default()
            },
        };
        *self.status.write().unwrap_or_else(PoisonError::into_inner) = next;
    }

    async fn read_status(&self, state: &DesiredState) -> Result<Status, Error> {
        let docker = self.docker()?;
        docker.ping().await?;
        let list = docker
            .list_containers(Some(
                ListContainersOptionsBuilder::default().all(true).build(),
            ))
            .await?;
        let mut containers = Vec::new();
        let mut stacks: BTreeMap<String, StackInfo> = state
            .stacks
            .iter()
            .map(|s| {
                (
                    s.name.clone(),
                    StackInfo {
                        name: s.name.clone(),
                        running: 0,
                        total: 0,
                        error: self
                            .stack_errors
                            .read()
                            .unwrap_or_else(PoisonError::into_inner)
                            .get(&s.name)
                            .cloned(),
                    },
                )
            })
            .collect();
        for c in list {
            let labels = c.labels.clone().unwrap_or_default();
            let owner = if labels
                .get(CONTAINER_LABEL)
                .is_some_and(|n| state.containers.iter().any(|d| &d.name == n))
            {
                Owner::Container
            } else if let Some(stack) = labels.get(PROJECT_LABEL).and_then(|p| stacks.get_mut(p)) {
                stack.total += 1;
                if c.state.map(|s| s.to_string()).as_deref() == Some("running") {
                    stack.running += 1;
                }
                Owner::Stack {
                    stack: stack.name.clone(),
                    service: labels.get(SERVICE_LABEL).cloned().unwrap_or_default(),
                }
            } else {
                continue;
            };
            let name = c
                .names
                .as_ref()
                .and_then(|n| n.first())
                .map(|n| n.trim_start_matches('/').to_owned())
                .unwrap_or_default();
            let status = c.status.clone().unwrap_or_default();
            let health = ["(healthy)", "(unhealthy)", "(health: starting)"]
                .iter()
                .find(|h| status.contains(*h))
                .map(|h| {
                    h.trim_matches(|c| c == '(' || c == ')')
                        .trim_start_matches("health: ")
                        .to_owned()
                });
            let address = c
                .network_settings
                .as_ref()
                .and_then(|n| n.networks.as_ref())
                .and_then(|n| n.get(NETWORK))
                .and_then(|e| e.ip_address.as_deref())
                .and_then(|ip| ip.parse().ok());
            containers.push(ContainerInfo {
                name,
                owner,
                image: c.image.clone().unwrap_or_default(),
                state: c.state.map(|s| s.to_string()).unwrap_or_default(),
                status,
                health,
                address,
                created: c.created,
            });
        }
        for wanted in &state.containers {
            if !containers.iter().any(|c| c.name == wanted.name) {
                containers.push(ContainerInfo {
                    name: wanted.name.clone(),
                    owner: Owner::Container,
                    image: wanted.image.clone(),
                    state: "missing".into(),
                    status: String::new(),
                    health: None,
                    address: None,
                    created: None,
                });
            }
        }
        containers.sort_by(|a, b| a.name.cmp(&b.name));
        let subnet = docker
            .inspect_network(NETWORK, None::<InspectNetworkOptions>)
            .await
            .ok()
            .and_then(|n| n.ipam)
            .and_then(|i| i.config)
            .and_then(|c| c.into_iter().find_map(|c| c.subnet));
        Ok(Status {
            available: true,
            error: None,
            subnet,
            containers,
            stacks: stacks.into_values().collect(),
            problems: Vec::new(),
        })
    }

    fn managed_name(&self, name: &str) -> Result<(), Error> {
        if self.status().containers.iter().any(|c| c.name == name) {
            Ok(())
        } else {
            Err(Error::NoSuchContainer(name.to_owned()))
        }
    }

    pub async fn restart(&self, name: &str) -> Result<(), Error> {
        self.managed_name(name)?;
        self.docker()?
            .restart_container(
                name,
                Some(RestartContainerOptionsBuilder::default().t(10).build()),
            )
            .await?;
        Ok(())
    }

    /// Pulls a managed container's image and recreates it if the image
    /// changed.
    pub async fn update_container(&self, state: &DesiredState, name: &str) -> Result<(), Error> {
        let container = state
            .containers
            .iter()
            .find(|c| c.name == name)
            .ok_or_else(|| Error::NoSuchContainer(name.to_owned()))?;
        let _guard = self.busy.lock().await;
        let result = self.apply_container(container, true).await;
        let problems = self.status().problems;
        self.refresh_locked(state, problems).await;
        result
    }

    /// Pulls a stack's images and brings it up again.
    pub async fn update_stack(&self, state: &DesiredState, name: &str) -> Result<(), Error> {
        let stack = state
            .stacks
            .iter()
            .find(|s| s.name == name)
            .ok_or_else(|| Error::NoSuchStack(name.to_owned()))?;
        let _guard = self.busy.lock().await;
        let result = self.apply_stack(stack, true).await.map_err(Error::Other);
        if result.is_ok() {
            self.connect_stacks(state).await?;
        }
        let problems = self.status().problems;
        self.refresh_locked(state, problems).await;
        result
    }

    /// The last `tail` lines of a managed container's output, optionally
    /// only those containing `search`.
    pub async fn logs(
        &self,
        name: &str,
        tail: usize,
        search: Option<&str>,
    ) -> Result<Vec<LogLine>, Error> {
        self.managed_name(name)?;
        let mut stream = self.docker()?.logs(
            name,
            Some(
                LogsOptionsBuilder::default()
                    .stdout(true)
                    .stderr(true)
                    .timestamps(true)
                    .tail(&tail.to_string())
                    .build(),
            ),
        );
        let mut lines = Vec::new();
        while let Some(item) = stream.next().await {
            let (stderr, bytes) = match item? {
                bollard::container::LogOutput::StdErr { message } => (true, message),
                bollard::container::LogOutput::StdOut { message }
                | bollard::container::LogOutput::Console { message } => (false, message),
                bollard::container::LogOutput::StdIn { .. } => continue,
            };
            for line in String::from_utf8_lossy(&bytes).lines() {
                let (time, text) = line.split_once(' ').unwrap_or(("", line));
                if search.is_some_and(|s| !text.to_lowercase().contains(&s.to_lowercase())) {
                    continue;
                }
                lines.push(LogLine {
                    time: time.to_owned(),
                    stderr,
                    text: text.to_owned(),
                });
            }
        }
        Ok(lines)
    }

    pub async fn stats(&self, name: &str) -> Result<Stats, Error> {
        self.managed_name(name)?;
        let docker = self.docker()?;
        let mut stream = docker.stats(
            name,
            Some(StatsOptionsBuilder::default().stream(false).build()),
        );
        let sample = stream
            .next()
            .await
            .ok_or_else(|| Error::Other("Docker returned no stats".into()))??;
        let cpu = |s: Option<&bollard::models::ContainerCpuStats>| {
            (
                s.and_then(|s| s.cpu_usage.as_ref())
                    .and_then(|u| u.total_usage)
                    .unwrap_or(0),
                s.and_then(|s| s.system_cpu_usage).unwrap_or(0),
            )
        };
        let (total, system) = cpu(sample.cpu_stats.as_ref());
        let (pre_total, pre_system) = cpu(sample.precpu_stats.as_ref());
        let cpus = sample
            .cpu_stats
            .as_ref()
            .and_then(|s| s.online_cpus)
            .unwrap_or(1);
        #[allow(clippy::cast_precision_loss)]
        let cpu_percent = if system > pre_system && total >= pre_total {
            (total - pre_total) as f64 / (system - pre_system) as f64 * f64::from(cpus) * 100.0
        } else {
            0.0
        };
        let memory = sample.memory_stats.unwrap_or_default();
        let inspect = docker
            .inspect_container(name, None::<InspectContainerOptions>)
            .await?;
        Ok(Stats {
            cpu_percent,
            memory_bytes: memory.usage.unwrap_or(0),
            memory_limit: memory.limit.unwrap_or(0),
            restarts: inspect.restart_count.unwrap_or(0),
            started_at: inspect.state.and_then(|s| s.started_at),
        })
    }

    /// Asks the registries whether the images of managed containers and
    /// stacks have newer versions. Changes nothing.
    pub async fn check_updates(&self) -> Result<Vec<ImageUpdate>, Error> {
        let docker = self.docker()?;
        let mut images: BTreeMap<String, Vec<String>> = BTreeMap::new();
        for c in self.status().containers {
            images.entry(c.image.clone()).or_default().push(c.name);
        }
        let mut out = Vec::new();
        for (image, used_by) in images {
            let local = docker
                .inspect_image(&image)
                .await
                .ok()
                .and_then(|i| i.repo_digests)
                .unwrap_or_default();
            let result = docker.inspect_registry_image(&image, None).await;
            let (update_available, error) = match result {
                Ok(remote) => {
                    let digest = remote.descriptor.digest.unwrap_or_default();
                    (
                        !digest.is_empty() && !local.iter().any(|d| d.ends_with(&digest)),
                        None,
                    )
                }
                Err(e) => (false, Some(e.to_string())),
            };
            out.push(ImageUpdate {
                image,
                used_by,
                update_available,
                error,
            });
        }
        Ok(out)
    }

    /// Addresses of managed containers on the `hearthgate` network, for
    /// the internal DNS.
    #[must_use]
    pub fn addresses(&self) -> BTreeMap<String, IpAddr> {
        self.status()
            .containers
            .iter()
            .filter_map(|c| Some((c.name.clone(), c.address?)))
            .collect()
    }
}

/// Whether a Docker API version such as `1.48` is at least `min`.
fn api_at_least(version: &str, min: (u32, u32)) -> bool {
    let mut parts = version.trim().split('.').map(str::parse::<u32>);
    match (parts.next(), parts.next()) {
        (Some(Ok(major)), Some(Ok(minor))) => (major, minor) >= min,
        _ => false,
    }
}

#[cfg(test)]
mod version_tests {
    use super::api_at_least;

    #[test]
    fn compares_api_versions() {
        assert!(api_at_least("1.48", (1, 48)));
        assert!(api_at_least("1.52", (1, 48)));
        assert!(api_at_least("2.0", (1, 48)));
        assert!(!api_at_least("1.47", (1, 48)));
        assert!(!api_at_least("1.9", (1, 48)));
        assert!(!api_at_least("", (1, 48)));
        assert!(!api_at_least("x.y", (1, 48)));
    }
}
