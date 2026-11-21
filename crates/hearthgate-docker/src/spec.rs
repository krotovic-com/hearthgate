//! What Docker is asked to create for a managed container.

use std::collections::HashMap;

use bollard::models::{
    ContainerCreateBody, EndpointSettings, HostConfig, NetworkingConfig, PortBinding,
    RestartPolicy, RestartPolicyNameEnum,
};
use hearthgate_model::{Container, Publish, Restart};
use sha2::{Digest, Sha256};

/// Label naming the managed container it belongs to.
pub const CONTAINER_LABEL: &str = "com.hearthgate.container";
/// Label with a hash of the settings it was created from.
pub const SPEC_LABEL: &str = "com.hearthgate.spec";
/// Label on the network Hearthgate creates.
pub const NETWORK_LABEL: &str = "com.hearthgate.network";

/// A short hash of the container's settings. A container whose label
/// differs is recreated.
#[must_use]
pub fn spec_hash(container: &Container) -> String {
    let json = serde_json::to_vec(container).unwrap_or_default();
    hex8(&Sha256::digest(&json))
}

/// The first 8 bytes of a digest in hex.
pub(crate) fn hex8(digest: &[u8]) -> String {
    use std::fmt::Write as _;
    digest[..8].iter().fold(String::new(), |mut s, b| {
        let _ = write!(s, "{b:02x}");
        s
    })
}

/// The body of Docker's create-container call.
#[must_use]
#[allow(clippy::zero_sized_map_values)] // Docker's API takes `{"80/tcp": {}}`.
pub fn create_body(container: &Container, network: &str) -> ContainerCreateBody {
    let mut exposed = HashMap::new();
    let mut bindings: HashMap<String, Option<Vec<PortBinding>>> = HashMap::new();
    for publish in container.publish.iter().filter_map(|p| Publish::parse(p)) {
        let key = format!(
            "{}/{}",
            publish.container,
            if publish.udp { "udp" } else { "tcp" }
        );
        exposed.insert(key.clone(), HashMap::new());
        bindings
            .entry(key)
            .or_default()
            .get_or_insert_with(Vec::new)
            .push(PortBinding {
                host_ip: publish.address.map(|a| a.to_string()),
                host_port: Some(publish.host.to_string()),
            });
    }
    let restart = match container.restart {
        Restart::No => RestartPolicyNameEnum::NO,
        Restart::Always => RestartPolicyNameEnum::ALWAYS,
        Restart::UnlessStopped => RestartPolicyNameEnum::UNLESS_STOPPED,
        Restart::OnFailure => RestartPolicyNameEnum::ON_FAILURE,
    };
    let labels = HashMap::from([
        (CONTAINER_LABEL.to_owned(), container.name.clone()),
        (SPEC_LABEL.to_owned(), spec_hash(container)),
    ]);
    ContainerCreateBody {
        image: Some(container.image.clone()),
        cmd: (!container.command.is_empty()).then(|| container.command.clone()),
        env: Some(
            container
                .env
                .iter()
                .map(|(k, v)| format!("{k}={v}"))
                .collect(),
        ),
        labels: Some(labels),
        exposed_ports: (!exposed.is_empty()).then_some(exposed),
        host_config: Some(HostConfig {
            binds: (!container.volumes.is_empty()).then(|| container.volumes.clone()),
            port_bindings: (!bindings.is_empty()).then_some(bindings),
            restart_policy: Some(RestartPolicy {
                name: Some(restart),
                maximum_retry_count: None,
            }),
            ..HostConfig::default()
        }),
        networking_config: Some(NetworkingConfig {
            endpoints_config: Some(HashMap::from([(
                network.to_owned(),
                EndpointSettings::default(),
            )])),
        }),
        ..ContainerCreateBody::default()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn builds_the_create_body() {
        let container: Container = serde_json::from_value(serde_json::json!({
            "name": "whoami",
            "image": "traefik/whoami:v1.10",
            "env": { "TZ": "UTC" },
            "volumes": ["/srv/w:/data:ro"],
            "publish": ["127.0.0.1:8081:80", "5353:53/udp"],
            "restart": "always",
        }))
        .unwrap();
        let body = create_body(&container, "hearthgate");
        assert_eq!(body.env.as_deref(), Some(&["TZ=UTC".to_owned()][..]));
        let host = body.host_config.unwrap();
        let ports = host.port_bindings.unwrap();
        let web = ports["80/tcp"].as_ref().unwrap();
        assert_eq!(web[0].host_ip.as_deref(), Some("127.0.0.1"));
        assert_eq!(web[0].host_port.as_deref(), Some("8081"));
        assert!(ports.contains_key("53/udp"));
        assert_eq!(
            host.restart_policy.unwrap().name,
            Some(RestartPolicyNameEnum::ALWAYS)
        );
        assert!(
            body.networking_config
                .unwrap()
                .endpoints_config
                .unwrap()
                .contains_key("hearthgate")
        );
        let labels = body.labels.unwrap();
        assert_eq!(labels[CONTAINER_LABEL], "whoami");
        let mut changed = container.clone();
        changed.image = "traefik/whoami:v1.11".into();
        assert_ne!(spec_hash(&changed), labels[SPEC_LABEL]);
    }
}
