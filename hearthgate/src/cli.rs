//! The `hearthgate` command line.

use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use anyhow::{Context, Result, bail};
use clap::{Parser, Subcommand};
use hearthgate_acme::{CertSource, CertStore};
use hearthgate_model::{DesiredState, ParseError};
use hearthgate_store::{Applied, RevisionMeta, Store};

use crate::config::Bootstrap;

/// Secure-by-default gateway for home servers.
#[derive(Debug, Parser)]
#[command(name = "hearthgate", version = crate::VERSION, about, long_about = None)]
pub struct Cli {
    /// Bootstrap config file [default: /etc/hearthgate/hearthgate.toml].
    #[arg(short, long, global = true, env = "HEARTHGATE_CONFIG")]
    pub config: Option<PathBuf>,

    /// Data directory, overriding the bootstrap config.
    #[arg(long, global = true, env = "HEARTHGATE_DATA_DIR")]
    pub data_dir: Option<PathBuf>,

    #[command(subcommand)]
    pub command: Command,
}

#[derive(Debug, Subcommand)]
pub enum FirewallCommand {
    /// Show whether the firewall is on and whether a change is waiting.
    Status,
    /// Keep the firewall change that is waiting. Run it from a new SSH
    /// session: if that session gets in, the new rules work.
    Confirm,
    /// Turn the firewall off (a new revision), for when a change locked
    /// you out and you are at the console.
    Off,
}

#[derive(Debug, Subcommand)]
pub enum VpnCommand {
    /// Show the VPN's settings and its peers.
    Status,
    /// Turn the VPN on.
    Enable {
        /// Host name or address peers connect to, such as
        /// `vpn.example.com` (default: the first public domain, or the
        /// router's public address).
        #[arg(long)]
        endpoint: Option<String>,
    },
    /// Turn the VPN off. Peers are kept.
    Disable,
    /// Add a device and print its configuration (and a QR code for phone
    /// apps). Its private key is shown only this once.
    Add {
        name: String,
        /// `all`, or the services it may reach, separated by commas.
        #[arg(long, default_value = "all")]
        access: String,
        /// Its VPN address (default: the next free one).
        #[arg(long)]
        address: Option<std::net::Ipv4Addr>,
        /// Write the configuration to this file instead of printing it.
        #[arg(short, long)]
        output: Option<PathBuf>,
        /// Don't print a QR code.
        #[arg(long)]
        no_qr: bool,
    },
    /// Remove a device. It can't connect any more.
    Remove { name: String },
}

#[derive(Debug, Subcommand)]
pub enum DockerCommand {
    /// Show managed containers and stacks.
    Status,
    /// Print a managed container's recent output.
    Logs {
        name: String,
        /// How many lines.
        #[arg(short = 'n', long, default_value_t = 100)]
        tail: usize,
    },
    /// Pull the newest image of a managed container or stack and recreate
    /// it if the image changed.
    Update { name: String },
    /// Check whether newer images exist, without changing anything.
    CheckUpdates,
}

#[derive(Debug, Subcommand)]
pub enum Command {
    /// Run the gateway (what the systemd service starts).
    Run,
    /// Check a desired-state file without applying it. Without a file,
    /// checks the bootstrap config and the current state.
    Validate {
        /// Desired-state TOML file.
        file: Option<PathBuf>,
    },
    /// Apply a desired-state file as a new revision. A running gateway picks
    /// it up within a second, without a restart.
    Apply {
        /// Desired-state TOML file, or `-` for standard input.
        file: PathBuf,
        /// Describe the change in the revision history.
        #[arg(short, long)]
        message: Option<String>,
        /// Show what would change without applying it.
        #[arg(long)]
        dry_run: bool,
    },
    /// Restore a file written by `export` (same as `apply`).
    Import {
        /// Desired-state TOML file, or `-` for standard input.
        file: PathBuf,
        /// Show what would change without importing it.
        #[arg(long)]
        dry_run: bool,
    },
    /// Print the desired state as TOML.
    Export {
        /// Write to a file instead of standard output.
        #[arg(short, long)]
        output: Option<PathBuf>,
        /// Export an older revision instead of the current one.
        #[arg(short, long)]
        revision: Option<i64>,
    },
    /// Show the current revision and what it contains.
    Status,
    /// List revisions, newest first.
    Revisions {
        /// How many to show.
        #[arg(short = 'n', long, default_value_t = 20)]
        limit: usize,
    },
    /// Show what changed between two revisions (default: the given one and
    /// the current one).
    Diff { from: i64, to: Option<i64> },
    /// Print the admin API token (created on first use).
    AdminToken,
    /// Print the one-time code for creating the first dashboard account.
    SetupCode,
    /// Show who can reach the dashboard. With --reset, go back to the
    /// default (this machine's LAN) after a restart, for when a change in
    /// the dashboard locked you out.
    AdminAccess {
        #[arg(long)]
        reset: bool,
    },
    /// Firewall: show its state, confirm a change, or turn it off.
    Firewall {
        #[command(subcommand)]
        action: FirewallCommand,
    },
    /// VPN: show it, turn it on or off, add or remove devices.
    Vpn {
        #[command(subcommand)]
        action: VpnCommand,
    },
    /// Managed Docker containers and stacks.
    Docker {
        #[command(subcommand)]
        action: DockerCommand,
    },
    /// Print the admin API's OpenAPI document.
    Openapi,
    /// Manage manually uploaded certificates.
    #[command(subcommand)]
    Cert(CertCommand),
    /// Make an older revision current again. History is kept: this creates
    /// a new revision with the old content.
    Rollback {
        revision: i64,
        /// Show what would change without rolling back.
        #[arg(long)]
        dry_run: bool,
    },
}

#[derive(Debug, Subcommand)]
pub enum CertCommand {
    /// Upload a certificate. Reference it from an interface with
    /// `certificate = "NAME"`.
    Import {
        /// Name to store it under.
        name: String,
        /// PEM certificate chain (leaf first).
        #[arg(long)]
        cert: PathBuf,
        /// PEM private key.
        #[arg(long)]
        key: PathBuf,
    },
    /// List stored certificates with their domains and expiry.
    List,
    /// Delete an uploaded certificate.
    Remove { name: String },
}

impl Cli {
    /// Runs the selected command.
    pub fn run(self) -> Result<ExitCode> {
        let mut bootstrap = Bootstrap::load(self.config.as_deref())?;
        if let Some(dir) = self.data_dir {
            bootstrap.data_dir = dir;
        }
        match self.command {
            Command::Run => crate::daemon::run(&bootstrap),
            Command::Validate { file } => validate(&bootstrap, file.as_deref()),
            Command::Apply {
                file,
                message,
                dry_run,
            } => {
                let message = message.unwrap_or_else(|| format!("Apply {}", file.display()));
                apply(&bootstrap, &file, &message, dry_run)
            }
            Command::Import { file, dry_run } => {
                let message = format!("Import {}", file.display());
                apply(&bootstrap, &file, &message, dry_run)
            }
            Command::Export { output, revision } => export(&bootstrap, output.as_deref(), revision),
            Command::Status => status(&bootstrap),
            Command::Revisions { limit } => revisions(&bootstrap, limit),
            Command::Diff { from, to } => diff_revisions(&bootstrap, from, to),
            Command::Rollback { revision, dry_run } => rollback(&bootstrap, revision, dry_run),
            Command::Cert(command) => cert(&bootstrap, command),
            Command::AdminToken => {
                let token = hearthgate_api::load_or_create_token(&bootstrap.data_dir)
                    .with_context(|| {
                        format!(
                            "cannot read the admin token in {}",
                            bootstrap.data_dir.display()
                        )
                    })?;
                println!("{token}");
                Ok(ExitCode::SUCCESS)
            }
            Command::SetupCode => {
                let store = open_store(&bootstrap)?;
                if let Some(code) = hearthgate_api::setup_code(&store)? {
                    println!("{code}");
                    Ok(ExitCode::SUCCESS)
                } else {
                    eprintln!("A dashboard account already exists, so there is no setup code.");
                    Ok(ExitCode::FAILURE)
                }
            }
            Command::AdminAccess { reset } => admin_access(&bootstrap, reset),
            Command::Firewall { action } => firewall(&bootstrap, &action),
            Command::Vpn { action } => crate::cli_vpn::run(&bootstrap, action),
            Command::Docker { action } => crate::cli_docker::run(&bootstrap, action),
            Command::Openapi => {
                use hearthgate_api::ApiDoc;
                use utoipa::OpenApi;
                println!("{}", ApiDoc::openapi().to_pretty_json()?);
                Ok(ExitCode::SUCCESS)
            }
        }
    }
}

fn firewall(bootstrap: &Bootstrap, action: &FirewallCommand) -> Result<ExitCode> {
    use crate::firewall::{CONFIRM_KEY, PENDING_KEY};
    let store = open_store(bootstrap)?;
    let pending: Option<hearthgate_firewall::Pending> = store
        .meta_get(PENDING_KEY)?
        .and_then(|p| serde_json::from_str(&p).ok());
    match action {
        FirewallCommand::Status => {
            let state = store.current_state()?;
            let fw = &state.firewall;
            if fw.enabled {
                println!(
                    "The firewall is on: {} rule(s), {} blocklist(s), {} blocked countr{}.",
                    fw.rules.len(),
                    fw.blocklists.iter().filter(|b| b.enabled).count(),
                    fw.countries.len(),
                    if fw.countries.len() == 1 { "y" } else { "ies" }
                );
                println!(
                    "LAN access to every port: {}.",
                    if fw.allow_lan { "yes" } else { "no" }
                );
            } else {
                println!("The firewall is off.");
            }
            match pending {
                Some(p) if !p.expired() => println!(
                    "Revision {} changed it. Confirm within {}s with `hearthgate firewall confirm`, \
                     or it is undone.",
                    p.revision,
                    p.seconds_left()
                ),
                _ => {}
            }
        }
        FirewallCommand::Confirm => match pending {
            Some(p) if !p.expired() => {
                store.meta_set(CONFIRM_KEY, &p.revision.to_string())?;
                println!("Kept the firewall change from revision {}.", p.revision);
            }
            Some(_) => {
                println!("Too late: the change has expired and is being undone.");
                return Ok(ExitCode::FAILURE);
            }
            None => {
                println!("No firewall change is waiting for confirmation.");
                return Ok(ExitCode::FAILURE);
            }
        },
        FirewallCommand::Off => {
            let mut state = store.current_state()?;
            if !state.firewall.enabled {
                println!("The firewall is already off.");
                return Ok(ExitCode::SUCCESS);
            }
            state.firewall.enabled = false;
            let applied = store.apply(
                &state,
                &author(),
                "Turn the firewall off from the command line",
            )?;
            println!(
                "Revision {}: the firewall is off. A running Hearthgate removes its rules within a \
                 second.",
                applied.meta().id
            );
        }
    }
    Ok(ExitCode::SUCCESS)
}

fn admin_access(bootstrap: &Bootstrap, reset: bool) -> Result<ExitCode> {
    use hearthgate_api::access::{self, AccessControl, AccessMode};
    if bootstrap.admin.fixed() {
        println!("Set in the bootstrap file:");
        for addr in bootstrap.admin.listen_or_default() {
            println!("  listen {addr}");
        }
        for net in bootstrap.admin.allow_or_default() {
            println!("  allow  {net}");
        }
        return Ok(ExitCode::SUCCESS);
    }
    let store = open_store(bootstrap)?;
    if reset {
        store.meta_delete(access::META_KEY)?;
        println!(
            "Reset to the defaults. Restart Hearthgate to apply: systemctl restart hearthgate"
        );
        return Ok(ExitCode::SUCCESS);
    }
    let settings = AccessControl::stored(&store);
    let resolved = access::resolve(&settings, &access::detect_lan());
    match settings.mode {
        AccessMode::Lan => println!("This machine's LAN addresses (found at start):"),
        AccessMode::Custom => println!("Chosen in the dashboard:"),
    }
    for addr in resolved.listen {
        println!("  listen {addr}");
    }
    for net in resolved.allow {
        println!("  allow  {net}");
    }
    Ok(ExitCode::SUCCESS)
}

pub(crate) fn open_store(bootstrap: &Bootstrap) -> Result<Store> {
    Store::open_dir(&bootstrap.data_dir).with_context(|| {
        format!(
            "cannot open the state database in {} (run as root or the hearthgate user, or pass \
             --data-dir)",
            bootstrap.data_dir.display()
        )
    })
}

fn read_state_file(file: &Path) -> Result<DesiredState> {
    let text = if file == Path::new("-") {
        std::io::read_to_string(std::io::stdin()).context("cannot read standard input")?
    } else {
        std::fs::read_to_string(file).with_context(|| format!("cannot read {}", file.display()))?
    };
    match DesiredState::from_toml(&text) {
        Ok(state) => Ok(state),
        Err(ParseError::Invalid(errors)) => bail!("{} is not valid. {errors}", file.display()),
        Err(e) => bail!("{}: {e}", file.display()),
    }
}

/// Who made a change, for the revision history.
pub(crate) fn author() -> String {
    let user = std::env::var("SUDO_USER")
        .or_else(|_| std::env::var("USER"))
        .unwrap_or_else(|_| "unknown".to_owned());
    format!("cli:{user}")
}

fn validate(bootstrap: &Bootstrap, file: Option<&Path>) -> Result<ExitCode> {
    if let Some(file) = file {
        let state = read_state_file(file)?;
        println!("{} is valid: {}.", file.display(), summary(&state));
    } else {
        let store = open_store(bootstrap)?;
        let state = store.current_state()?;
        state.validate()?;
        println!(
            "Bootstrap config and current state are valid: {}.",
            summary(&state)
        );
    }
    Ok(ExitCode::SUCCESS)
}

fn apply(bootstrap: &Bootstrap, file: &Path, message: &str, dry_run: bool) -> Result<ExitCode> {
    let state = read_state_file(file)?;
    let store = open_store(bootstrap)?;
    let current = store.current_state()?;
    let changes = diff(&current, &state);
    if dry_run {
        if changes.is_empty() {
            println!("No changes.");
        } else {
            print!("{changes}");
            println!("Dry run: nothing was applied.");
        }
        return Ok(ExitCode::SUCCESS);
    }
    match store.apply(&state, &author(), message)? {
        Applied::Unchanged(meta) => println!("No changes. Revision {} stays current.", meta.id),
        Applied::Created(meta) => {
            print!("{changes}");
            println!("Applied as revision {}: {}.", meta.id, summary(&state));
            if state.firewall.enabled && state.firewall != current.firewall {
                println!(
                    "The firewall changed. Check that you can still connect (for example open a \
                     new SSH session), then run `hearthgate firewall confirm` within {} seconds, \
                     or the change is undone.",
                    hearthgate_firewall::CONFIRM_WINDOW.as_secs()
                );
            }
        }
    }
    Ok(ExitCode::SUCCESS)
}

fn export(bootstrap: &Bootstrap, output: Option<&Path>, revision: Option<i64>) -> Result<ExitCode> {
    let store = open_store(bootstrap)?;
    let (state, header) = match revision {
        Some(id) => {
            let r = store
                .revision(id)?
                .with_context(|| format!("revision {id} does not exist"))?;
            (r.state, describe(&r.meta))
        }
        None => match store.current()? {
            Some(r) => (r.state, describe(&r.meta)),
            None => (DesiredState::default(), "no revisions yet".to_owned()),
        },
    };
    let text = format!(
        "# Hearthgate desired state, exported by {} ({header}).\n\n{}",
        crate::VERSION,
        state.to_toml()
    );
    match output {
        Some(path) => std::fs::write(path, text)
            .with_context(|| format!("cannot write {}", path.display()))?,
        None => std::io::stdout().write_all(text.as_bytes())?,
    }
    Ok(ExitCode::SUCCESS)
}

fn status(bootstrap: &Bootstrap) -> Result<ExitCode> {
    let store = open_store(bootstrap)?;
    println!("Data directory: {}", bootstrap.data_dir.display());
    let Some(current) = store.current()? else {
        println!("No revisions yet. Apply a desired-state file with `hearthgate apply FILE`.");
        return Ok(ExitCode::SUCCESS);
    };
    println!("Current revision: {}", describe(&current.meta));
    let state = &current.state;
    println!("Contains: {}", summary(state));
    for interface in &state.interfaces {
        let listeners: Vec<String> = interface
            .listeners
            .iter()
            .map(|l| {
                if l.protocol == hearthgate_model::Protocol::Tcp {
                    format!("{}/{}", l.port, l.mode)
                } else {
                    format!("{}/{} ({})", l.port, l.mode, l.protocol)
                }
            })
            .collect();
        let domains = if interface.domains.is_empty() {
            String::new()
        } else {
            format!(" [{}]", interface.domains.join(", "))
        };
        println!(
            "  interface {}{domains}: {}",
            interface.name,
            listeners.join(", ")
        );
    }
    Ok(ExitCode::SUCCESS)
}

fn revisions(bootstrap: &Bootstrap, limit: usize) -> Result<ExitCode> {
    let store = open_store(bootstrap)?;
    let list = store.list(limit)?;
    if list.is_empty() {
        println!("No revisions yet.");
    }
    for (n, meta) in list.iter().enumerate() {
        let marker = if n == 0 { "*" } else { " " };
        println!(
            "{marker} {:>4}  {}  {:<16}  {}",
            meta.id, meta.created_at, meta.author, meta.message
        );
    }
    Ok(ExitCode::SUCCESS)
}

fn diff_revisions(bootstrap: &Bootstrap, from: i64, to: Option<i64>) -> Result<ExitCode> {
    let store = open_store(bootstrap)?;
    let load = |id: i64| -> Result<DesiredState> {
        Ok(store
            .revision(id)?
            .with_context(|| format!("revision {id} does not exist"))?
            .state)
    };
    let old = load(from)?;
    let new = match to {
        Some(id) => load(id)?,
        None => store.current_state()?,
    };
    let changes = diff(&old, &new);
    if changes.is_empty() {
        println!("No differences.");
    } else {
        print!("{changes}");
    }
    Ok(ExitCode::SUCCESS)
}

fn rollback(bootstrap: &Bootstrap, revision: i64, dry_run: bool) -> Result<ExitCode> {
    let store = open_store(bootstrap)?;
    let target = store
        .revision(revision)?
        .with_context(|| format!("revision {revision} does not exist"))?;
    let changes = diff(&store.current_state()?, &target.state);
    if dry_run {
        print!("{changes}");
        println!("Dry run: nothing was rolled back.");
        return Ok(ExitCode::SUCCESS);
    }
    match store.rollback(revision, &author())? {
        Applied::Unchanged(meta) => println!(
            "Revision {revision} matches the current revision {}. Nothing to do.",
            meta.id
        ),
        Applied::Created(meta) => {
            print!("{changes}");
            println!(
                "Rolled back to revision {revision}, stored as revision {}.",
                meta.id
            );
        }
    }
    Ok(ExitCode::SUCCESS)
}

fn cert(bootstrap: &Bootstrap, command: CertCommand) -> Result<ExitCode> {
    let store = CertStore::new(&bootstrap.data_dir);
    match command {
        CertCommand::Import { name, cert, key } => {
            let cert_pem =
                std::fs::read(&cert).with_context(|| format!("cannot read {}", cert.display()))?;
            let key_pem =
                std::fs::read(&key).with_context(|| format!("cannot read {}", key.display()))?;
            let info = store.save(CertSource::Manual, &name, &cert_pem, &key_pem)?;
            println!(
                "Stored certificate {name} for {} (expires in {} days). A running gateway picks it up within a second.",
                info.domains.join(", "),
                info.remaining().as_secs() / 86_400
            );
        }
        CertCommand::List => {
            let list = store.list()?;
            if list.is_empty() {
                println!("No certificates stored.");
            }
            for info in list {
                let source = match info.source {
                    CertSource::Manual => "manual",
                    CertSource::Acme => "automatic",
                };
                println!(
                    "{:<20} {:<9} {:>4} days left  {}  ({})",
                    info.name,
                    source,
                    info.remaining().as_secs() / 86_400,
                    info.domains.join(", "),
                    info.issuer
                );
            }
        }
        CertCommand::Remove { name } => {
            store.remove(CertSource::Manual, &name)?;
            println!("Removed certificate {name}.");
        }
    }
    Ok(ExitCode::SUCCESS)
}

fn describe(meta: &RevisionMeta) -> String {
    format!(
        "revision {} by {} at {}: {}",
        meta.id, meta.author, meta.created_at, meta.message
    )
}

fn plural(n: usize, word: &str) -> String {
    if n == 1 {
        format!("1 {word}")
    } else {
        format!("{n} {word}s")
    }
}

/// One-line summary such as "2 interfaces, 3 services, 2 routes".
#[must_use]
pub fn summary(state: &DesiredState) -> String {
    let mut out = format!(
        "{}, {}, {}",
        plural(state.interfaces.len(), "interface"),
        plural(state.services.len(), "service"),
        plural(state.routes.len(), "route")
    );
    if !state.containers.is_empty() {
        out = format!("{out}, {}", plural(state.containers.len(), "container"));
    }
    if !state.stacks.is_empty() {
        out = format!("{out}, {}", plural(state.stacks.len(), "stack"));
    }
    out
}

/// Unified diff of two states in their TOML form. Empty when equal.
#[must_use]
pub fn diff(old: &DesiredState, new: &DesiredState) -> String {
    old.diff(new)
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::CommandFactory;

    #[test]
    fn cli_is_well_formed() {
        Cli::command().debug_assert();
    }

    #[test]
    fn diff_is_empty_for_equal_states() {
        let s = DesiredState::default();
        assert_eq!(diff(&s, &s), "");
        let mut t = s.clone();
        t.limits.max_body_bytes = 1;
        assert!(diff(&s, &t).contains("+max_body_bytes = 1"));
    }
}
