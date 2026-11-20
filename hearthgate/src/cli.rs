//! The `hearthgate` command line.

use std::fmt::Write as _;
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
        }
    }
}

fn open_store(bootstrap: &Bootstrap) -> Result<Store> {
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
fn author() -> String {
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
    format!(
        "{}, {}, {}",
        plural(state.interfaces.len(), "interface"),
        plural(state.services.len(), "service"),
        plural(state.routes.len(), "route")
    )
}

/// Unified diff of two states in their TOML form. Empty when equal.
#[must_use]
pub fn diff(old: &DesiredState, new: &DesiredState) -> String {
    let (old, new) = (old.to_toml(), new.to_toml());
    if old == new {
        return String::new();
    }
    let diff = similar::TextDiff::from_lines(&old, &new);
    let mut out = String::new();
    for hunk in diff.unified_diff().context_radius(3).iter_hunks() {
        let _ = write!(out, "{hunk}");
    }
    out
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
        assert!(diff(&s, &s).is_empty());
        let mut t = s.clone();
        t.limits.max_body_bytes = 1;
        assert!(diff(&s, &t).contains("+max_body_bytes = 1"));
    }
}
