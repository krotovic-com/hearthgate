//! `hearthgate docker`: managed containers and stacks from the command line.

use std::process::ExitCode;

use anyhow::{Context, Result, bail};
use hearthgate_docker::{Manager, Owner};

use crate::cli::{DockerCommand, open_store};
use crate::config::Bootstrap;

pub(crate) fn run(bootstrap: &Bootstrap, action: DockerCommand) -> Result<ExitCode> {
    if !(bootstrap.docker.enabled && bootstrap.docker.manage) {
        bail!("managed Docker is off in the bootstrap file (docker.enabled and docker.manage)");
    }
    let state = open_store(bootstrap)?.current_state()?;
    let manager = Manager::new(Some(&bootstrap.docker.socket), &bootstrap.data_dir);
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()?;
    runtime.block_on(async {
        manager.refresh(&state).await;
        let status = manager.status();
        if let Some(e) = &status.error {
            bail!(
                "cannot reach Docker at {}: {e}",
                bootstrap.docker.socket.display()
            );
        }
        match action {
            DockerCommand::Status => {
                if status.containers.is_empty() && status.stacks.is_empty() {
                    println!("No managed containers or stacks.");
                }
                for c in &status.containers {
                    let owner = match &c.owner {
                        Owner::Container => String::new(),
                        Owner::Stack { stack, .. } => format!(" (stack {stack})"),
                    };
                    let address = c.address.map(|a| a.to_string()).unwrap_or_default();
                    println!(
                        "  {:<24} {:<10} {:<15} {}{owner}",
                        c.name, c.state, address, c.image
                    );
                }
                for s in &status.stacks {
                    println!("stack {}: {} of {} running", s.name, s.running, s.total);
                    if let Some(e) = &s.error {
                        println!("  {e}");
                    }
                }
                for p in &status.problems {
                    println!("problem: {p}");
                }
            }
            DockerCommand::Logs { name, tail } => {
                for line in manager.logs(&name, tail, None).await? {
                    if line.stderr {
                        eprintln!("{} {}", line.time, line.text);
                    } else {
                        println!("{} {}", line.time, line.text);
                    }
                }
            }
            DockerCommand::Update { name } => {
                if state.stacks.iter().any(|s| s.name == name) {
                    manager.update_stack(&state, &name).await?;
                    println!("Stack {name} is up to date.");
                } else {
                    manager
                        .update_container(&state, &name)
                        .await
                        .with_context(|| format!("cannot update {name}"))?;
                    println!("{name} is up to date.");
                }
            }
            DockerCommand::CheckUpdates => {
                for u in manager.check_updates().await? {
                    let verdict = match (&u.error, u.update_available) {
                        (Some(e), _) => format!("cannot check: {e}"),
                        (None, true) => "update available".into(),
                        (None, false) => "up to date".into(),
                    };
                    println!("  {:<40} {verdict} ({})", u.image, u.used_by.join(", "));
                }
            }
        }
        Ok(ExitCode::SUCCESS)
    })
}
