//! Updates of Hearthgate itself: the daemon's checker, `hearthgate update`,
//! and the root helper `hearthgate upgrade apply`. The work is in the
//! `hearthgate-update` crate.

use std::path::Path;
use std::process::ExitCode;
use std::sync::Arc;

use anyhow::{Result, bail};
use hearthgate_model::EventKind;
use hearthgate_notify::{Event, Severity, emit};
use hearthgate_update::apply::{Helper, Host};
use hearthgate_update::fetch::HttpFetch;
use hearthgate_update::method::{Arch, Method, current_exe, detect, helper_installed};
use hearthgate_update::state::{Layout, Outcome};
use hearthgate_update::{Config, Updater, Version, keys};
use tokio_util::sync::CancellationToken;

use crate::cli::UpdateCommand;
use crate::config::Bootstrap;

fn method() -> Method {
    detect(Path::new("/"), &current_exe())
}

/// The daemon's updater, for the API and the background loop.
#[must_use]
pub fn updater(bootstrap: &Bootstrap) -> Arc<Updater> {
    Arc::new(Updater::new(Config {
        layout: Layout::new(&bootstrap.data_dir),
        current: crate::VERSION.to_owned(),
        method: method(),
        helper: helper_installed(Path::new("/")),
        arch: Arch::current(),
        fetch: Arc::new(HttpFetch),
        keys: keys::release_keys(),
        settings: hearthgate_model::Updates::default(),
    }))
}

/// Checks daily, installs in the window, and tells the person about new
/// releases.
pub fn spawn(updater: Arc<Updater>, stop: CancellationToken) {
    tokio::spawn(updater.run(stop, |current, available| {
        let v = available.version;
        tracing::info!(
            "Hearthgate {v} is available (this is {current}): {}",
            available.page
        );
        emit(Event::new(
            EventKind::Updates,
            Severity::Info,
            format!("update:{v}"),
            format!("Hearthgate {v} is available"),
            format!(
                "This server runs Hearthgate {current}. Install it from Settings → Updates in the \
                 dashboard, or see what's new at {}",
                available.page
            ),
        ));
    }));
}

fn runtime() -> Result<tokio::runtime::Runtime> {
    Ok(tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()?)
}

fn when(unix: u64) -> String {
    jiff::Timestamp::from_second(i64::try_from(unix).unwrap_or(0))
        .map(|t| {
            t.to_zoned(jiff::tz::TimeZone::system())
                .strftime("%Y-%m-%d %H:%M")
                .to_string()
        })
        .unwrap_or_default()
}

/// Makes files the CLI wrote as root belong to the daemon again.
fn give_back(bootstrap: &Bootstrap, layout: &Layout) {
    use std::os::unix::fs::MetadataExt;
    let Ok(owner) = std::fs::metadata(&bootstrap.data_dir) else {
        return;
    };
    let (uid, gid) = (owner.uid(), owner.gid());
    let mut paths = vec![layout.updates()];
    if let Ok(entries) = std::fs::read_dir(layout.updates()) {
        for e in entries.filter_map(Result::ok) {
            paths.push(e.path());
            if let Ok(inner) = std::fs::read_dir(e.path()) {
                paths.extend(inner.filter_map(Result::ok).map(|i| i.path()));
            }
        }
    }
    for p in paths {
        let _ = std::os::unix::fs::lchown(&p, Some(uid), Some(gid));
    }
}

/// `hearthgate update ...`
pub(crate) fn run(bootstrap: &Bootstrap, action: UpdateCommand) -> Result<ExitCode> {
    let updater = updater(bootstrap);
    let print_job = |job: &hearthgate_update::state::Job| {
        let to = job.to.map(|v| v.to_string()).unwrap_or_default();
        println!(
            "{} {} → {to}, {}: {:?}. {}",
            when(job.started_unix),
            job.from,
            match job.action {
                hearthgate_update::state::Action::Install => "install",
                hearthgate_update::state::Action::Rollback => "rollback",
            },
            job.outcome,
            job.detail.as_deref().unwrap_or("")
        );
    };
    match action {
        UpdateCommand::Status => {
            let info = updater.info();
            println!("Hearthgate {} ({}).", info.current, info.method_text);
            if let Some(why) = &info.cannot_install {
                println!("{why}");
            }
            match &info.available {
                Some(a) => println!("{} is available ({:?}): {}", a.version, a.bump, a.page),
                None => println!(
                    "No newer release known{}.",
                    info.checked_unix
                        .map(|t| format!(" (checked {})", when(t)))
                        .unwrap_or_default()
                ),
            }
            if let Some(v) = info.scheduled {
                println!(
                    "{v} installs in the next window ({}).",
                    info.settings.window
                );
            }
            if let Some(v) = info.rollback_to {
                println!("{v} is kept for a rollback: hearthgate update rollback");
            }
            if let Some(job) = &info.job {
                print_job(job);
            }
        }
        UpdateCommand::Check => {
            let latest = runtime()?
                .block_on(updater.latest())
                .map_err(anyhow::Error::msg)?;
            match latest {
                Some(a) => {
                    println!(
                        "Hearthgate {} is available (this is {}).",
                        a.version,
                        crate::VERSION
                    );
                    for n in &a.notes {
                        println!("  {:?}: {}", n.kind, n.text);
                    }
                    println!("{}", a.page);
                }
                None => println!("Hearthgate {} is the newest release.", crate::VERSION),
            }
        }
        UpdateCommand::Install { version } => {
            if !nix_root() {
                bail!("run it as root, for example with sudo");
            }
            let rt = runtime()?;
            let info = rt.block_on(updater.check()).map_err(anyhow::Error::msg)?;
            let Some(available) = info.available else {
                println!("Hearthgate {} is the newest release.", crate::VERSION);
                return Ok(ExitCode::SUCCESS);
            };
            let version = match version {
                Some(v) => Version::parse(&v)
                    .ok_or_else(|| anyhow::anyhow!("{v} is not a release version"))?,
                None => available.version,
            };
            let result = rt.block_on(updater.install(version, true));
            give_back(bootstrap, updater.layout());
            result.map_err(anyhow::Error::msg)?;
            println!(
                "Downloaded and verified {version}. The upgrade helper installs it now; follow it with \
                 `journalctl -fu hearthgate-upgrade` or in the dashboard."
            );
        }
        UpdateCommand::Rollback => {
            if !nix_root() {
                bail!("run it as root, for example with sudo");
            }
            let result = updater.rollback();
            give_back(bootstrap, updater.layout());
            result.map_err(anyhow::Error::msg)?;
            println!("The upgrade helper goes back to the kept version now.");
        }
        UpdateCommand::History => {
            for job in updater.history() {
                print_job(&job);
            }
        }
    }
    Ok(ExitCode::SUCCESS)
}

fn nix_root() -> bool {
    use std::os::unix::fs::MetadataExt;
    // /proc/self belongs to the effective user.
    std::fs::metadata("/proc/self").is_ok_and(|m| m.uid() == 0)
}

/// `hearthgate upgrade apply`: what `hearthgate-upgrade.service` runs.
pub(crate) fn apply(bootstrap: &Bootstrap) -> Result<ExitCode> {
    if !nix_root() {
        bail!("the upgrade helper runs as root, from hearthgate-upgrade.service");
    }
    let Some(current) = Version::parse(crate::VERSION) else {
        bail!(
            "this is a development build ({}), which is not upgraded",
            crate::VERSION
        );
    };
    let Some(arch) = Arch::current() else {
        bail!("there are no releases for this CPU");
    };
    let helper = Helper {
        layout: Layout::new(&bootstrap.data_dir),
        root: "/".into(),
        system: &Host,
        current,
        method: method(),
        arch,
        keys: keys::release_keys(),
        health_timeout: std::time::Duration::from_secs(120),
    };
    let Some(job) = helper.run_pending() else {
        println!("Nothing to do.");
        return Ok(ExitCode::SUCCESS);
    };
    for line in &job.log {
        println!("{line}");
    }
    Ok(match job.outcome {
        Outcome::Installed | Outcome::Restored | Outcome::Refused | Outcome::RolledBack => {
            ExitCode::SUCCESS
        }
        Outcome::Running | Outcome::Failed => ExitCode::FAILURE,
    })
}
