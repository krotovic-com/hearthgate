//! Compose stacks, run with the `docker compose` command.

use std::path::{Path, PathBuf};
use std::process::Stdio;

use hearthgate_model::Stack;
use sha2::{Digest, Sha256};
use tokio::process::Command;

/// File holding the hash of the stack as it was last brought up or stopped.
const APPLIED: &str = ".hearthgate-applied";

/// One stack's files on disk.
#[derive(Debug, Clone)]
pub struct StackDir {
    pub name: String,
    pub dir: PathBuf,
}

impl StackDir {
    #[must_use]
    pub fn new(root: &Path, name: &str) -> Self {
        Self {
            name: name.to_owned(),
            dir: root.join(name),
        }
    }

    /// Writes the Compose file and its `.env`. Returns the stack's hash.
    pub fn write(&self, stack: &Stack) -> std::io::Result<String> {
        use std::io::Write as _;
        use std::os::unix::fs::OpenOptionsExt as _;
        std::fs::create_dir_all(&self.dir)?;
        let files = [
            ("compose.yaml", stack.compose.clone()),
            (
                ".env",
                stack.env.iter().fold(String::new(), |mut out, (k, v)| {
                    use std::fmt::Write as _;
                    let _ = writeln!(out, "{k}={}", quote(v));
                    out
                }),
            ),
        ];
        for (file, text) in files {
            let path = self.dir.join(file);
            if std::fs::read_to_string(&path).ok().as_deref() == Some(text.as_str()) {
                continue;
            }
            let tmp = self.dir.join(format!(".{file}.tmp"));
            let mut f = std::fs::OpenOptions::new()
                .write(true)
                .create(true)
                .truncate(true)
                .mode(0o600)
                .open(&tmp)?;
            f.write_all(text.as_bytes())?;
            f.sync_all()?;
            std::fs::rename(&tmp, &path)?;
        }
        Ok(hash(stack))
    }

    #[must_use]
    pub fn applied(&self) -> Option<String> {
        std::fs::read_to_string(self.dir.join(APPLIED))
            .ok()
            .map(|s| s.trim().to_owned())
    }

    pub fn set_applied(&self, hash: &str) -> std::io::Result<()> {
        std::fs::write(self.dir.join(APPLIED), hash)
    }
}

/// Quotes a `.env` value so Compose reads it back unchanged.
fn quote(value: &str) -> String {
    if value
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || "_-./:@,+".contains(c))
    {
        value.to_owned()
    } else {
        format!("'{}'", value.replace('\'', "'\\''"))
    }
}

/// Hash of everything that decides how the stack runs.
#[must_use]
pub fn hash(stack: &Stack) -> String {
    let json = serde_json::to_vec(stack).unwrap_or_default();
    crate::spec::hex8(&Sha256::digest(&json))
}

/// Runs `docker compose` for one project.
#[derive(Debug, Clone)]
pub struct Compose {
    /// The command, normally `["docker", "compose"]`.
    pub program: Vec<String>,
    /// `DOCKER_HOST` for it.
    pub host: Option<String>,
}

impl Default for Compose {
    fn default() -> Self {
        Self {
            program: vec!["docker".into(), "compose".into()],
            host: None,
        }
    }
}

impl Compose {
    /// Runs `docker compose -p NAME --project-directory DIR -f compose.yaml ARGS`
    /// and returns its output, or the last lines of its error output.
    pub async fn run(&self, stack: &StackDir, args: &[&str]) -> Result<String, String> {
        let (program, prefix) = self
            .program
            .split_first()
            .ok_or_else(|| "no compose command".to_owned())?;
        let mut cmd = Command::new(program);
        cmd.args(prefix)
            .arg("-p")
            .arg(&stack.name)
            .arg("--project-directory")
            .arg(&stack.dir)
            .arg("-f")
            .arg(stack.dir.join("compose.yaml"))
            .args(args)
            .stdin(Stdio::null())
            .kill_on_drop(true);
        if let Some(host) = &self.host {
            cmd.env("DOCKER_HOST", host);
        }
        let out = cmd
            .output()
            .await
            .map_err(|e| format!("cannot run {}: {e}", self.program.join(" ")))?;
        if out.status.success() {
            Ok(String::from_utf8_lossy(&out.stdout).into_owned())
        } else {
            let err = String::from_utf8_lossy(&out.stderr);
            let tail: Vec<&str> = err
                .lines()
                .filter(|l| !l.trim().is_empty())
                .rev()
                .take(6)
                .collect();
            let text: Vec<&str> = tail.into_iter().rev().collect();
            Err(if text.is_empty() {
                format!("docker compose exited with {}", out.status)
            } else {
                text.join("\n")
            })
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn writes_files_and_tracks_changes() {
        let dir = tempfile::tempdir().unwrap();
        let mut stack: Stack = serde_json::from_value(serde_json::json!({
            "name": "notes",
            "compose": "services:\n  app:\n    image: nginx\n",
            "env": { "PASSWORD": "it's secret", "PORT": "80" },
        }))
        .unwrap();
        let files = StackDir::new(dir.path(), "notes");
        let first = files.write(&stack).unwrap();
        let env = std::fs::read_to_string(files.dir.join(".env")).unwrap();
        assert_eq!(env, "PASSWORD='it'\\''s secret'\nPORT=80\n");
        assert!(files.applied().is_none());
        files.set_applied(&first).unwrap();
        assert_eq!(files.applied().as_deref(), Some(first.as_str()));
        stack.running = false;
        assert_ne!(files.write(&stack).unwrap(), first);
    }
}
