//! `hearthgate vpn`: the VPN from the command line.

use std::collections::BTreeMap;
use std::process::ExitCode;
use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::{Context, Result, bail};
use hearthgate_model::{Access, Peer};
use hearthgate_vpn::{ClientConfig, Device, INTERFACE, Tools};

use crate::cli::{VpnCommand, author, open_store};
use crate::config::Bootstrap;

pub(crate) fn run(bootstrap: &Bootstrap, action: VpnCommand) -> Result<ExitCode> {
    let store = open_store(bootstrap)?;
    let mut state = store.current_state()?;
    let key_dir = bootstrap.data_dir.join("vpn");
    match action {
        VpnCommand::Status => {
            let vpn = &state.vpn;
            println!(
                "The VPN is {}: {} on UDP port {}, this machine is {}.",
                if vpn.enabled { "on" } else { "off" },
                vpn.subnet,
                vpn.port,
                vpn.server_address()
            );
            match hearthgate_vpn::endpoint(&state, None) {
                Some(e) => println!("Devices connect to {e}."),
                None => println!(
                    "Devices connect to the router's public address (or set one with `hearthgate \
                     vpn enable --endpoint HOST`)."
                ),
            }
            if state.dns.enabled {
                println!(
                    "DNS: names under {} on {}.",
                    state.dns.zone,
                    vpn.server_address()
                );
            }
            let live = live_peers();
            let now = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .map_or(0, |d| d.as_secs());
            if vpn.peers.is_empty() {
                println!("No devices yet. Add one with `hearthgate vpn add NAME`.");
            }
            for peer in &vpn.peers {
                let access = match &peer.access {
                    Access::All => "everything".to_owned(),
                    Access::Services(list) => list.join(", "),
                };
                let seen = live
                    .iter()
                    .find(|l| l.public_key == peer.public_key)
                    .and_then(|l| l.latest_handshake)
                    .map_or_else(
                        || "never connected".to_owned(),
                        |t| format!("last handshake {}s ago", now.saturating_sub(t)),
                    );
                println!("  {:<16} {:<15} {access} ({seen})", peer.name, peer.address);
            }
        }
        VpnCommand::Enable { endpoint } => {
            let before = state.vpn.clone();
            state.vpn.enabled = true;
            if endpoint.is_some() {
                state.vpn.endpoint = endpoint;
            }
            if state.vpn == before {
                println!("The VPN is already on.");
                return Ok(ExitCode::SUCCESS);
            }
            let applied = store.apply(&state, &author(), "Turn the VPN on")?;
            println!(
                "Revision {}: the VPN is on, on UDP port {}.",
                applied.meta().id,
                state.vpn.port
            );
        }
        VpnCommand::Disable => {
            if !state.vpn.enabled {
                println!("The VPN is already off.");
                return Ok(ExitCode::SUCCESS);
            }
            state.vpn.enabled = false;
            let applied = store.apply(&state, &author(), "Turn the VPN off")?;
            println!(
                "Revision {}: the VPN is off. Its devices are kept.",
                applied.meta().id
            );
        }
        VpnCommand::Add {
            name,
            access,
            address,
            output,
            no_qr,
        } => {
            if state.vpn.peers.iter().any(|p| p.name == name) {
                bail!("a device is already called `{name}`; remove it first to replace it");
            }
            let access = if access.trim() == "all" {
                Access::All
            } else {
                Access::Services(
                    access
                        .split(',')
                        .map(|s| s.trim().to_owned())
                        .filter(|s| !s.is_empty())
                        .collect(),
                )
            };
            let address = match address {
                Some(a) => a,
                None => state
                    .vpn
                    .free_address()
                    .with_context(|| format!("{} has no free address", state.vpn.subnet))?,
            };
            let Some(endpoint) = hearthgate_vpn::endpoint(&state, None) else {
                bail!(
                    "set the address devices connect to first: hearthgate vpn enable --endpoint \
                     HOST"
                );
            };
            let server_key = hearthgate_vpn::load_or_create_key(&key_dir)
                .with_context(|| format!("cannot read the VPN key in {}", key_dir.display()))?;
            let server_public = hearthgate_vpn::keys::public_key(&server_key)
                .context("the stored VPN key is not valid")?;
            let pair = hearthgate_vpn::keys::generate()?;
            let lan: Vec<ipnet::IpNet> = hearthgate_api::access::detect_lan()
                .into_iter()
                .chain(hearthgate_api::access::detect_container_networks())
                .map(|(_, net)| net)
                .filter(|net| !matches!(net, ipnet::IpNet::V4(v4) if state.vpn.subnet.contains(v4)))
                .collect();
            let enabled_now = !state.vpn.enabled;
            state.vpn.enabled = true;
            state.vpn.peers.push(Peer {
                name: name.clone(),
                public_key: pair.public.clone(),
                address,
                access: access.clone(),
            });
            let routes = hearthgate_vpn::client_routes(&state, &access, &lan, &BTreeMap::new());
            let config = ClientConfig {
                private_key: pair.private.clone(),
                address,
                server_public_key: server_public,
                endpoint,
                allowed_ips: routes,
                dns: state
                    .dns
                    .enabled
                    .then(|| (state.vpn.server_address(), state.dns.zone.clone())),
            }
            .render();
            // Store first: a configuration for a peer that failed
            // validation would be useless.
            let applied = store.apply(&state, &author(), &format!("Add VPN device {name}"))?;
            eprintln!(
                "Revision {}: added {name} at {address}{}.",
                applied.meta().id,
                if enabled_now {
                    ", and turned the VPN on"
                } else {
                    ""
                }
            );
            if let Some(path) = output {
                write_private(&path, &config)?;
                eprintln!(
                    "Wrote its configuration to {}. It holds the device's private key: move it \
                     to the device and delete it here.",
                    path.display()
                );
            } else {
                eprintln!(
                    "Its configuration follows. It holds the device's private key and is not \
                     shown again.\n"
                );
                print!("{config}");
            }
            if !no_qr && let Some(qr) = hearthgate_vpn::qr_terminal(&config) {
                eprintln!("\nOr scan this with the WireGuard app:\n");
                eprintln!("{qr}");
            }
        }
        VpnCommand::Remove { name } => {
            let before = state.vpn.peers.len();
            state.vpn.peers.retain(|p| p.name != name);
            if state.vpn.peers.len() == before {
                bail!("no device is called `{name}`");
            }
            let applied = store.apply(&state, &author(), &format!("Remove VPN device {name}"))?;
            println!(
                "Revision {}: removed {name}. It can't connect any more.",
                applied.meta().id
            );
        }
    }
    Ok(ExitCode::SUCCESS)
}

/// Live peers of the running interface, if it runs and `wg` works.
fn live_peers() -> Vec<hearthgate_vpn::PeerStatus> {
    let Ok(runtime) = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
    else {
        return Vec::new();
    };
    runtime.block_on(async {
        let device = Device::new(INTERFACE, Tools::default());
        if device.exists().await {
            device.peers().await.unwrap_or_default()
        } else {
            Vec::new()
        }
    })
}

fn write_private(path: &std::path::Path, text: &str) -> Result<()> {
    use std::io::Write as _;
    use std::os::unix::fs::OpenOptionsExt as _;
    let mut file = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(path)
        .with_context(|| format!("cannot create {}", path.display()))?;
    file.write_all(text.as_bytes())?;
    Ok(())
}
