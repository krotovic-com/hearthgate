//! Backups of everything Hearthgate keeps: the state database (with its
//! history and dashboard accounts), certificates, ACME accounts, the VPN
//! key, the dashboard certificate and the admin token.
//!
//! A backup is a `.tar.gz` with a `hearthgate-backup.json` manifest. It
//! leaves out what Hearthgate rebuilds by itself: blocklist caches and the
//! files of Compose stacks (written again from the state).

use std::fs::{self, File};
use std::io::{Read, Write};
use std::os::unix::fs::{OpenOptionsExt as _, PermissionsExt as _};
use std::path::{Component, Path, PathBuf};
use std::time::{Duration, SystemTime};

use anyhow::{Context, Result, bail};
use flate2::Compression;
use flate2::read::GzDecoder;
use flate2::write::GzEncoder;
use hearthgate_model::duration::format_rfc3339;
use hearthgate_store::{DATABASE_FILE, Store};
use serde::{Deserialize, Serialize};
use tokio_util::sync::CancellationToken;
use tracing::{info, warn};

use crate::config::BackupSettings;

/// Name of the manifest inside a backup.
pub const MANIFEST: &str = "hearthgate-backup.json";
/// Backup format; grows only if the layout changes incompatibly.
pub const FORMAT: u32 = 1;
/// File names start with this.
const PREFIX: &str = "hearthgate-backup-";
/// Entries of the data directory that are never backed up.
const SKIP: &[&str] = &["backups", "stacks", "blocklists"];

/// What a backup says about itself.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Manifest {
    pub format: u32,
    /// Hearthgate version that wrote it.
    pub version: String,
    /// When, in RFC 3339.
    pub created: String,
    /// Schema version of the database.
    pub schema: i64,
}

fn is_tmp(name: &str) -> bool {
    Path::new(name)
        .extension()
        .is_some_and(|e| e.eq_ignore_ascii_case("tmp"))
}

fn skipped(name: &str) -> bool {
    SKIP.contains(&name)
        || name.starts_with(DATABASE_FILE)
        || name.starts_with(PREFIX)
        || is_tmp(name)
        || name.starts_with("before-restore-")
        || name.starts_with(".hearthgate-backup")
}

/// Writes a backup of `data_dir` to `out` (a file path). The archive is
/// written next to it first and renamed, so a half-written backup never
/// replaces a good one.
pub fn create(data_dir: &Path, out: &Path) -> Result<Manifest> {
    let store = Store::open_dir(data_dir).context("cannot open the state database")?;
    let parent = out
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or(Path::new("."));
    fs::create_dir_all(parent).with_context(|| format!("cannot create {}", parent.display()))?;
    let work = tempfile::Builder::new()
        .prefix(".hearthgate-backup")
        .tempdir_in(parent)
        .with_context(|| format!("cannot write in {}", parent.display()))?;
    let snapshot = work.path().join(DATABASE_FILE);
    store
        .snapshot(&snapshot)
        .context("cannot copy the state database")?;
    let manifest = Manifest {
        format: FORMAT,
        version: crate::VERSION.to_owned(),
        created: format_rfc3339(SystemTime::now()),
        schema: Store::schema_version(),
    };

    let tmp = work.path().join("backup.tar.gz");
    let file = fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(&tmp)?;
    let mut tar = tar::Builder::new(GzEncoder::new(file, Compression::default()));
    tar.mode(tar::HeaderMode::Deterministic);
    let json = serde_json::to_vec_pretty(&manifest)?;
    append_bytes(&mut tar, MANIFEST, &json)?;
    tar.append_path_with_name(&snapshot, DATABASE_FILE)?;
    for entry in fs::read_dir(data_dir)? {
        let entry = entry?;
        let name = entry.file_name();
        let Some(name) = name.to_str() else { continue };
        if skipped(name) {
            continue;
        }
        let path = entry.path();
        let kind = entry.file_type()?;
        if kind.is_dir() {
            append_dir(&mut tar, &path, Path::new(name))?;
        } else if kind.is_file() {
            tar.append_path_with_name(&path, name)?;
        }
    }
    let mut gz = tar.into_inner()?;
    gz.flush()?;
    let file = gz.finish()?;
    file.sync_all()?;
    fs::rename(&tmp, out).with_context(|| format!("cannot write {}", out.display()))?;
    fs::set_permissions(out, fs::Permissions::from_mode(0o600))?;
    Ok(manifest)
}

fn append_bytes<W: Write>(tar: &mut tar::Builder<W>, name: &str, data: &[u8]) -> Result<()> {
    let mut header = tar::Header::new_gnu();
    header.set_size(data.len() as u64);
    header.set_mode(0o600);
    header.set_cksum();
    tar.append_data(&mut header, name, data)?;
    Ok(())
}

/// Adds a directory's regular files and subdirectories (no sockets or
/// links).
fn append_dir<W: Write>(tar: &mut tar::Builder<W>, dir: &Path, name: &Path) -> Result<()> {
    tar.append_dir(name, dir)?;
    for entry in fs::read_dir(dir)? {
        let entry = entry?;
        let kind = entry.file_type()?;
        let child = name.join(entry.file_name());
        if kind.is_dir() {
            append_dir(tar, &entry.path(), &child)?;
        } else if kind.is_file() && !is_tmp(&entry.file_name().to_string_lossy()) {
            tar.append_path_with_name(entry.path(), &child)?;
        }
    }
    Ok(())
}

/// Reads the manifest of a backup without restoring it.
pub fn inspect(file: &Path) -> Result<Manifest> {
    let mut archive = open_archive(file)?;
    for entry in archive.entries()? {
        let entry = entry?;
        if entry.path()?.as_ref() == Path::new(MANIFEST) {
            let mut text = String::new();
            entry.take(64 * 1024).read_to_string(&mut text)?;
            return serde_json::from_str(&text)
                .with_context(|| format!("{} has a broken manifest", file.display()));
        }
    }
    bail!(
        "{} is not a Hearthgate backup (no {MANIFEST})",
        file.display()
    )
}

fn open_archive(file: &Path) -> Result<tar::Archive<GzDecoder<File>>> {
    let f = File::open(file).with_context(|| format!("cannot open {}", file.display()))?;
    Ok(tar::Archive::new(GzDecoder::new(f)))
}

/// Restores a backup into `data_dir`. Stop the service first. When the
/// directory already has a state, `force` is needed; what was there is
/// moved into `data_dir/before-restore-TIME/` rather than deleted.
pub fn restore(file: &Path, data_dir: &Path, force: bool) -> Result<Manifest> {
    let manifest = inspect(file)?;
    if manifest.format != FORMAT {
        bail!(
            "{} uses backup format {}, which this version ({}) cannot read",
            file.display(),
            manifest.format,
            crate::VERSION
        );
    }
    if manifest.schema > Store::schema_version() {
        bail!(
            "{} comes from Hearthgate {}, which is newer than this one ({}). Upgrade first.",
            file.display(),
            manifest.version,
            crate::VERSION
        );
    }
    fs::create_dir_all(data_dir)?;
    fs::set_permissions(data_dir, fs::Permissions::from_mode(0o700))?;
    let existing: Vec<PathBuf> = fs::read_dir(data_dir)?
        .filter_map(Result::ok)
        .filter(|e| e.file_name() != "backups")
        .map(|e| e.path())
        .collect();
    if !existing.is_empty() {
        if !force {
            bail!(
                "{} is not empty. Stop the service, then run the restore again with --force: \
                 what is there now is kept in {}/before-restore-TIME/",
                data_dir.display(),
                data_dir.display()
            );
        }
        let stamp = format_rfc3339(SystemTime::now()).replace(':', "");
        let aside = data_dir.join(format!("before-restore-{stamp}"));
        fs::create_dir(&aside)?;
        for path in existing {
            if let Some(name) = path.file_name() {
                fs::rename(&path, aside.join(name))?;
            }
        }
        info!("kept the previous contents in {}", aside.display());
    }

    let mut archive = open_archive(file)?;
    for entry in archive.entries()? {
        let mut entry = entry?;
        let path = entry.path()?.into_owned();
        if path == Path::new(MANIFEST) {
            continue;
        }
        // Only plain relative paths: nothing can land outside data_dir.
        if path
            .components()
            .any(|c| !matches!(c, Component::Normal(_)))
        {
            bail!(
                "{} contains an unsafe path {}",
                file.display(),
                path.display()
            );
        }
        let target = data_dir.join(&path);
        match entry.header().entry_type() {
            tar::EntryType::Directory => {
                fs::create_dir_all(&target)?;
                fs::set_permissions(&target, fs::Permissions::from_mode(0o700))?;
            }
            tar::EntryType::Regular => {
                if let Some(parent) = target.parent() {
                    fs::create_dir_all(parent)?;
                }
                let mut out = fs::OpenOptions::new()
                    .write(true)
                    .create(true)
                    .truncate(true)
                    .mode(0o600)
                    .open(&target)?;
                std::io::copy(&mut entry, &mut out)?;
                out.sync_all()?;
            }
            _ => {}
        }
    }
    // Opening runs the migrations of anything older than this version.
    Store::open_dir(data_dir).context("the restored database does not open")?;
    Ok(manifest)
}

/// The default name for a new backup.
#[must_use]
pub fn file_name() -> String {
    let stamp = format_rfc3339(SystemTime::now()).replace(['-', ':'], "");
    format!("{PREFIX}{stamp}.tar.gz")
}

/// Backups in `dir`, oldest first.
pub fn list(dir: &Path) -> Vec<PathBuf> {
    let mut out: Vec<PathBuf> = fs::read_dir(dir)
        .into_iter()
        .flatten()
        .filter_map(Result::ok)
        .map(|e| e.path())
        .filter(|p| {
            p.file_name()
                .and_then(|n| n.to_str())
                .is_some_and(|n| n.starts_with(PREFIX) && n.ends_with(".tar.gz"))
        })
        .collect();
    // The names sort by time.
    out.sort();
    out
}

/// Writes a backup now and removes the oldest beyond `keep`.
pub fn scheduled(data_dir: &Path, settings: &BackupSettings) -> Result<PathBuf> {
    let dir = settings.dir_or_default(data_dir);
    let path = dir.join(file_name());
    create(data_dir, &path)?;
    let all = list(&dir);
    if all.len() > settings.keep {
        for old in &all[..all.len() - settings.keep] {
            if let Err(e) = fs::remove_file(old) {
                warn!("cannot remove the old backup {}: {e}", old.display());
            }
        }
    }
    Ok(path)
}

/// Writes scheduled backups until `stop`. The first one is due one
/// interval after the newest existing backup, so restarts don't add more.
pub async fn run(data_dir: PathBuf, settings: BackupSettings, stop: CancellationToken) {
    if !settings.enabled || settings.interval.is_zero() {
        return;
    }
    let dir = settings.dir_or_default(&data_dir);
    let last = list(&dir)
        .last()
        .and_then(|p| fs::metadata(p).ok())
        .and_then(|m| m.modified().ok())
        .and_then(|t| t.elapsed().ok());
    let mut wait = match last {
        Some(age) => settings.interval.saturating_sub(age),
        // No backup yet: soon after start, once things settled.
        None => Duration::from_secs(60),
    };
    loop {
        tokio::select! {
            () = stop.cancelled() => return,
            () = tokio::time::sleep(wait) => {}
        }
        let (d, s) = (data_dir.clone(), settings.clone());
        match tokio::task::spawn_blocking(move || scheduled(&d, &s)).await {
            Ok(Ok(path)) => info!("backup written to {}", path.display()),
            Ok(Err(e)) => warn!("backup failed: {e:#}"),
            Err(e) => warn!("backup failed: {e}"),
        }
        wait = settings.interval;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn backs_up_and_restores() {
        let tmp = tempfile::tempdir().unwrap();
        let data = tmp.path().join("data");
        let store = Store::open_dir(&data).unwrap();
        let state = hearthgate_model::DesiredState::from_toml(
            "[[services]]\nname = \"nas\"\nkind = \"external\"\nhost = \"192.168.1.2\"\nports = { web = 80 }\n",
        )
        .unwrap();
        store.apply(&state, "test", "test").unwrap();
        drop(store);
        fs::create_dir_all(data.join("certs/manual/web")).unwrap();
        fs::write(data.join("certs/manual/web/key.pem"), "KEY").unwrap();
        fs::write(data.join("admin-token"), "TOKEN").unwrap();
        fs::create_dir_all(data.join("blocklists")).unwrap();
        fs::write(data.join("blocklists/feed"), "cache").unwrap();

        let file = tmp.path().join("b.tar.gz");
        let manifest = create(&data, &file).unwrap();
        assert_eq!(manifest.format, FORMAT);
        assert_eq!(inspect(&file).unwrap(), manifest);
        assert_eq!(
            fs::metadata(&file).unwrap().permissions().mode() & 0o777,
            0o600
        );

        // Onto a fresh machine.
        let fresh = tmp.path().join("fresh");
        restore(&file, &fresh, false).unwrap();
        assert_eq!(
            fs::read_to_string(fresh.join("admin-token")).unwrap(),
            "TOKEN"
        );
        assert_eq!(
            fs::read_to_string(fresh.join("certs/manual/web/key.pem")).unwrap(),
            "KEY"
        );
        assert!(!fresh.join("blocklists").exists(), "caches are left out");
        let restored = Store::open_dir(&fresh).unwrap();
        assert_eq!(restored.current_state().unwrap(), state);

        // Over existing data only with force, and nothing is lost.
        assert!(restore(&file, &data, false).is_err());
        restore(&file, &data, true).unwrap();
        let aside: Vec<_> = fs::read_dir(&data)
            .unwrap()
            .filter_map(Result::ok)
            .filter(|e| {
                e.file_name()
                    .to_string_lossy()
                    .starts_with("before-restore-")
            })
            .collect();
        assert_eq!(aside.len(), 1);
        assert!(aside[0].path().join("blocklists/feed").exists());
        // A new backup leaves the kept copy out.
        let again = tmp.path().join("again.tar.gz");
        create(&data, &again).unwrap();
        let names: Vec<String> = open_archive(&again)
            .unwrap()
            .entries()
            .unwrap()
            .map(|e| e.unwrap().path().unwrap().display().to_string())
            .collect();
        assert!(
            names.iter().all(|n| !n.starts_with("before-restore")),
            "{names:?}"
        );
    }

    #[test]
    fn refuses_unsafe_and_newer_backups() {
        let tmp = tempfile::tempdir().unwrap();
        let file = tmp.path().join("evil.tar.gz");
        let mut tar = tar::Builder::new(GzEncoder::new(
            File::create(&file).unwrap(),
            Compression::default(),
        ));
        let manifest = Manifest {
            format: FORMAT,
            version: "9.9.9".into(),
            created: String::new(),
            schema: 1,
        };
        append_bytes(&mut tar, MANIFEST, &serde_json::to_vec(&manifest).unwrap()).unwrap();
        let mut header = tar::Header::new_gnu();
        header.set_size(1);
        header.set_mode(0o600);
        // `append_data` refuses `..`, so write the name by hand.
        header.as_gnu_mut().unwrap().name[..12].copy_from_slice(b"../escape.tx");
        header.set_cksum();
        tar.append(&header, &b"x"[..]).unwrap();
        tar.into_inner().unwrap().finish().unwrap();
        let data = tmp.path().join("data");
        let err = restore(&file, &data, false).unwrap_err().to_string();
        assert!(err.contains("unsafe path"), "{err}");
        assert!(!tmp.path().join("escape.tx").exists());

        let newer = tmp.path().join("newer.tar.gz");
        let mut tar = tar::Builder::new(GzEncoder::new(
            File::create(&newer).unwrap(),
            Compression::default(),
        ));
        let manifest = Manifest {
            schema: 999,
            ..manifest
        };
        append_bytes(&mut tar, MANIFEST, &serde_json::to_vec(&manifest).unwrap()).unwrap();
        tar.into_inner().unwrap().finish().unwrap();
        let err = restore(&newer, &tmp.path().join("d2"), false)
            .unwrap_err()
            .to_string();
        assert!(err.contains("newer"), "{err}");
    }

    #[test]
    fn keeps_only_the_newest() {
        let tmp = tempfile::tempdir().unwrap();
        let data = tmp.path().join("data");
        Store::open_dir(&data).unwrap();
        let settings = BackupSettings {
            keep: 2,
            ..BackupSettings::default()
        };
        let dir = settings.dir_or_default(&data);
        fs::create_dir_all(&dir).unwrap();
        for stamp in ["20200101T000000Z", "20210101T000000Z", "20220101T000000Z"] {
            fs::write(dir.join(format!("{PREFIX}{stamp}.tar.gz")), "old").unwrap();
        }
        let new = scheduled(&data, &settings).unwrap();
        let left = list(&dir);
        assert_eq!(left.len(), 2, "{left:?}");
        assert_eq!(left.last(), Some(&new));
        // Backups in the data directory are never inside other backups.
        let names: Vec<String> = open_archive(&new)
            .unwrap()
            .entries()
            .unwrap()
            .map(|e| e.unwrap().path().unwrap().display().to_string())
            .collect();
        assert!(names.iter().all(|n| !n.starts_with("backups")), "{names:?}");
    }
}
