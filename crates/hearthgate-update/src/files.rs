//! File handling that is safe when a less trusted process shares the
//! directory: no symlinks followed, sizes capped, writes atomic.
//!
//! The root helper reads files the daemon wrote. If the daemon were taken
//! over, it could plant symlinks to make root read or overwrite something
//! else. So every open here refuses symlinks, every write goes to a fresh
//! file that is then renamed into place (a rename replaces a symlink, it
//! never writes through it), and every read stops at a size limit.

use std::fs::{File, OpenOptions};
use std::io::{self, Read, Write};
use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
use std::path::{Path, PathBuf};

/// Opens `path` for reading, refusing a symlink as the last component.
pub fn open_nofollow(path: &Path) -> io::Result<File> {
    let file = OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC)
        .open(path)?;
    if !file.metadata()?.is_file() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "not a regular file",
        ));
    }
    Ok(file)
}

/// Reads at most `max` bytes of a regular file; more is an error.
pub fn read_limited(path: &Path, max: u64) -> io::Result<Vec<u8>> {
    let file = open_nofollow(path)?;
    let mut data = Vec::new();
    file.take(max + 1).read_to_end(&mut data)?;
    if data.len() as u64 > max {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "file is too large",
        ));
    }
    Ok(data)
}

/// Reads and parses a small JSON file; `None` when it is missing or bad.
#[must_use]
pub fn read_json<T: serde::de::DeserializeOwned>(path: &Path) -> Option<T> {
    let data = read_limited(path, 1024 * 1024).ok()?;
    serde_json::from_slice(&data).ok()
}

/// Writes `data` to `path` atomically with `mode` permissions.
pub fn write_atomic(path: &Path, data: &[u8], mode: u32) -> io::Result<()> {
    let dir = path.parent().unwrap_or(Path::new("."));
    let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("file");
    let tmp = fresh_name(dir, name);
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(mode)
        .custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC)
        .open(&tmp)?;
    let result = (|| {
        file.write_all(data)?;
        file.set_permissions(std::fs::Permissions::from_mode(mode))?;
        file.sync_all()?;
        std::fs::rename(&tmp, path)
    })();
    if result.is_err() {
        let _ = std::fs::remove_file(&tmp);
    }
    result?;
    if let Ok(d) = File::open(dir) {
        let _ = d.sync_all();
    }
    Ok(())
}

/// Writes `value` as pretty JSON, atomically.
pub fn write_json<T: serde::Serialize>(path: &Path, value: &T, mode: u32) -> io::Result<()> {
    let body = serde_json::to_vec_pretty(value).map_err(io::Error::other)?;
    write_atomic(path, &body, mode)
}

/// Copies a regular file without following symlinks, at most `max` bytes,
/// to a new file at `to` (atomically).
pub fn copy_limited(from: &Path, to: &Path, max: u64, mode: u32) -> io::Result<()> {
    let mut src = open_nofollow(from)?;
    let dir = to.parent().unwrap_or(Path::new("."));
    let tmp = fresh_name(dir, "copy");
    let mut dst = OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(mode)
        .custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC)
        .open(&tmp)?;
    let result = (|| {
        let copied = io::copy(&mut (&mut src).take(max + 1), &mut dst)?;
        if copied > max {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "file is too large",
            ));
        }
        dst.set_permissions(std::fs::Permissions::from_mode(mode))?;
        dst.sync_all()?;
        std::fs::rename(&tmp, to)
    })();
    if result.is_err() {
        let _ = std::fs::remove_file(&tmp);
    }
    result
}

/// Makes sure `dir` is a real directory (not a symlink) with `mode`.
pub fn ensure_dir(dir: &Path, mode: u32) -> io::Result<()> {
    match std::fs::symlink_metadata(dir) {
        Ok(m) if m.file_type().is_symlink() || !m.is_dir() => {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                format!("{} is not a directory", dir.display()),
            ));
        }
        Ok(_) => {}
        Err(e) if e.kind() == io::ErrorKind::NotFound => {
            std::fs::create_dir_all(dir)?;
        }
        Err(e) => return Err(e),
    }
    std::fs::set_permissions(dir, std::fs::Permissions::from_mode(mode))
}

fn fresh_name(dir: &Path, name: &str) -> PathBuf {
    let mut bytes = [0u8; 8];
    let _ = File::open("/dev/urandom").and_then(|mut f| f.read_exact(&mut bytes));
    let suffix: String = bytes.iter().fold(String::new(), |mut s, b| {
        use std::fmt::Write;
        let _ = write!(s, "{b:02x}");
        s
    });
    dir.join(format!(".{name}.{suffix}.tmp"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn refuses_symlinks() {
        let dir = tempfile::tempdir().unwrap();
        let secret = dir.path().join("secret");
        std::fs::write(&secret, b"root only").unwrap();
        let link = dir.path().join("status.json");
        std::os::unix::fs::symlink(&secret, &link).unwrap();
        assert!(read_limited(&link, 100).is_err());
        // Writing replaces the link instead of writing through it.
        write_atomic(&link, b"{}", 0o644).unwrap();
        assert_eq!(std::fs::read(&secret).unwrap(), b"root only");
        assert!(
            !std::fs::symlink_metadata(&link)
                .unwrap()
                .file_type()
                .is_symlink()
        );
        let ldir = dir.path().join("ldir");
        std::os::unix::fs::symlink(dir.path(), &ldir).unwrap();
        assert!(ensure_dir(&ldir, 0o700).is_err());
    }

    #[test]
    fn caps_sizes() {
        let dir = tempfile::tempdir().unwrap();
        let big = dir.path().join("big");
        std::fs::write(&big, vec![0u8; 101]).unwrap();
        assert!(read_limited(&big, 100).is_err());
        assert!(copy_limited(&big, &dir.path().join("copy"), 100, 0o600).is_err());
        assert!(!dir.path().join("copy").exists());
        copy_limited(&big, &dir.path().join("copy"), 101, 0o600).unwrap();
        assert_eq!(std::fs::read(dir.path().join("copy")).unwrap().len(), 101);
    }
}
