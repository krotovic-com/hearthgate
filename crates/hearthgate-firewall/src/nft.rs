//! Runs the `nft` tool and reads its JSON output.

use std::fmt::Write as _;
use std::net::IpAddr;
use std::path::PathBuf;
use std::process::Stdio;
use std::time::Duration;

use serde_json::Value;
use tokio::io::AsyncWriteExt;
use tokio::process::Command;

use crate::ruleset::{Reason, TABLE};

/// Failure running `nft`.
#[derive(Debug, thiserror::Error)]
pub enum NftError {
    #[error("cannot run {program}: {source} (install the nftables package)")]
    Spawn {
        program: String,
        source: std::io::Error,
    },
    #[error("nft refused the ruleset: {0}")]
    Refused(String),
    #[error("unexpected output from nft: {0}")]
    Output(String),
}

/// How to run `nft`. Tests point this at a network namespace with
/// `ip netns exec <name> nft`.
#[derive(Debug, Clone)]
pub struct Nft {
    command: Vec<String>,
}

impl Default for Nft {
    fn default() -> Self {
        Self::new(vec![find_nft().to_string_lossy().into_owned()])
    }
}

/// A recent drop, from the `recent_*` sets.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct RecentDrop {
    pub source: IpAddr,
    pub protocol: String,
    pub port: u16,
    pub reason: Reason,
    pub packets: u64,
    /// Seconds since the last dropped packet (approximately).
    pub last_seen_secs: u64,
}

/// Packets dropped per reason since the table was last replaced.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, serde::Serialize)]
pub struct Dropped {
    pub blocklist: u64,
    pub ban: u64,
    pub policy: u64,
    pub ratelimit: u64,
}

impl Dropped {
    #[must_use]
    pub fn total(&self) -> u64 {
        self.blocklist + self.ban + self.policy + self.ratelimit
    }
}

impl Nft {
    /// `command` is the program and any leading arguments.
    #[must_use]
    pub fn new(command: Vec<String>) -> Self {
        Self { command }
    }

    async fn run(&self, args: &[&str], stdin: Option<&str>) -> Result<String, NftError> {
        let program = self
            .command
            .first()
            .cloned()
            .unwrap_or_else(|| "nft".into());
        let mut cmd = Command::new(&program);
        cmd.args(&self.command[1..])
            .args(args)
            .stdin(if stdin.is_some() {
                Stdio::piped()
            } else {
                Stdio::null()
            })
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true);
        let mut child = cmd
            .spawn()
            .map_err(|source| NftError::Spawn { program, source })?;
        if let Some(text) = stdin {
            if let Some(mut pipe) = child.stdin.take() {
                // A refused script makes nft exit early; its error message
                // says why, so a broken pipe here is not the real problem.
                let _ = pipe.write_all(text.as_bytes()).await;
            }
        }
        let output = tokio::time::timeout(Duration::from_secs(60), child.wait_with_output())
            .await
            .map_err(|_| NftError::Refused("nft did not finish within 60 seconds".into()))?
            .map_err(|e| NftError::Refused(e.to_string()))?;
        if output.status.success() {
            Ok(String::from_utf8_lossy(&output.stdout).into_owned())
        } else {
            let err = String::from_utf8_lossy(&output.stderr);
            Err(NftError::Refused(trim_error(&err)))
        }
    }

    /// Lets VPN traffic through Docker's `DOCKER-USER` chain, which
    /// otherwise drops forwarded packets on hosts that run Docker. With
    /// `None`, removes those rules. Returns whether Docker's chain exists.
    pub async fn docker_user(&self, interface: Option<&str>) -> Result<bool, NftError> {
        const COMMENT: &str = "comment \"hearthgate\"";
        let Ok(listing) = self
            .run(
                &["-a", "list", "chain", "ip", "filter", "DOCKER-USER"],
                None,
            )
            .await
        else {
            return Ok(false);
        };
        let ours: Vec<(&str, &str)> = listing
            .lines()
            .filter(|l| l.contains(COMMENT))
            .filter_map(|l| {
                l.rsplit_once("# handle ")
                    .map(|(rule, h)| (rule.trim(), h.trim()))
            })
            .collect();
        let wanted: Vec<String> = interface
            .map(|i| {
                vec![
                    format!("iifname \"{i}\" accept {COMMENT}"),
                    format!("oifname \"{i}\" ct state established,related accept {COMMENT}"),
                ]
            })
            .unwrap_or_default();
        let current: Vec<String> = ours.iter().map(|(rule, _)| (*rule).to_owned()).collect();
        let mut sorted_current = current.clone();
        sorted_current.sort();
        let mut sorted_wanted = wanted.clone();
        sorted_wanted.sort();
        if sorted_current == sorted_wanted {
            return Ok(true);
        }
        let mut script = String::new();
        for (_, handle) in &ours {
            let _ = writeln!(script, "delete rule ip filter DOCKER-USER handle {handle}");
        }
        for rule in wanted.iter().rev() {
            let _ = writeln!(script, "insert rule ip filter DOCKER-USER {rule}");
        }
        self.apply(&script).await?;
        Ok(true)
    }

    /// Runs a script in one transaction.
    pub async fn apply(&self, script: &str) -> Result<(), NftError> {
        self.run(&["-f", "-"], Some(script)).await.map(|_| ())
    }

    /// Checks a script without changing anything.
    pub async fn check(&self, script: &str) -> Result<(), NftError> {
        self.run(&["-c", "-f", "-"], Some(script)).await.map(|_| ())
    }

    /// Whether the table exists.
    pub async fn installed(&self) -> bool {
        self.run(&["list", "table", "inet", TABLE], None)
            .await
            .is_ok()
    }

    /// Current bans with the time left on each.
    pub async fn bans(&self) -> Result<Vec<(IpAddr, Duration)>, NftError> {
        let mut out = Vec::new();
        for set in ["ban4", "ban6"] {
            let json = self
                .run(&["-j", "list", "set", "inet", TABLE, set], None)
                .await?;
            for element in set_elements(&json)? {
                let (Some(ip), expires) = (element_ip(&element), element_expires(&element)) else {
                    continue;
                };
                out.push((ip, expires));
            }
        }
        Ok(out)
    }

    /// Packets dropped per reason.
    pub async fn dropped(&self) -> Result<Dropped, NftError> {
        let json = self
            .run(&["-j", "list", "counters", "table", "inet", TABLE], None)
            .await?;
        let value: Value =
            serde_json::from_str(&json).map_err(|e| NftError::Output(e.to_string()))?;
        let mut dropped = Dropped::default();
        for item in value["nftables"].as_array().into_iter().flatten() {
            let counter = &item["counter"];
            let packets = counter["packets"].as_u64().unwrap_or(0);
            match counter["name"].as_str() {
                Some("dropped_blocklist") => dropped.blocklist = packets,
                Some("dropped_ban") => dropped.ban = packets,
                Some("dropped_policy") => dropped.policy = packets,
                Some("dropped_ratelimit") => dropped.ratelimit = packets,
                _ => {}
            }
        }
        Ok(dropped)
    }

    /// Sources dropped in the last hour, most packets first.
    pub async fn recent(&self) -> Result<Vec<RecentDrop>, NftError> {
        let mut out = Vec::new();
        for reason in Reason::ALL {
            for family in ["4", "6"] {
                let set = format!("recent_{}{family}", reason.key());
                let json = self
                    .run(&["-j", "list", "set", "inet", TABLE, &set], None)
                    .await?;
                for element in set_elements(&json)? {
                    if let Some(drop) = recent_drop(&element, reason) {
                        out.push(drop);
                    }
                }
            }
        }
        out.sort_by(|a, b| b.packets.cmp(&a.packets).then(a.source.cmp(&b.source)));
        Ok(out)
    }
}

fn find_nft() -> PathBuf {
    for dir in ["/usr/sbin", "/sbin", "/usr/bin", "/bin", "/usr/local/sbin"] {
        let path = PathBuf::from(dir).join("nft");
        if path.exists() {
            return path;
        }
    }
    PathBuf::from("nft")
}

/// nft repeats the offending script line with carets; keep the message.
fn trim_error(stderr: &str) -> String {
    let lines: Vec<&str> = stderr
        .lines()
        .filter(|l| !l.trim().is_empty() && !l.trim_start().starts_with('^'))
        .take(6)
        .collect();
    let text = lines.join("; ");
    if text.len() > 600 {
        format!("{}…", &text[..text.floor_char_boundary_compat(600)])
    } else {
        text
    }
}

trait FloorBoundary {
    fn floor_char_boundary_compat(&self, index: usize) -> usize;
}

impl FloorBoundary for str {
    fn floor_char_boundary_compat(&self, mut index: usize) -> usize {
        while index > 0 && !self.is_char_boundary(index) {
            index -= 1;
        }
        index
    }
}

fn set_elements(json: &str) -> Result<Vec<Value>, NftError> {
    let value: Value = serde_json::from_str(json).map_err(|e| NftError::Output(e.to_string()))?;
    let mut out = Vec::new();
    for item in value["nftables"].as_array().into_iter().flatten() {
        if let Some(elements) = item["set"]["elem"].as_array() {
            out.extend(elements.iter().cloned());
        }
    }
    Ok(out)
}

/// An element is either a bare value or `{"elem": {"val": …, "expires": …}}`.
fn element_value(element: &Value) -> &Value {
    if element["elem"].is_object() {
        &element["elem"]["val"]
    } else {
        element
    }
}

fn element_ip(element: &Value) -> Option<IpAddr> {
    element_value(element).as_str()?.parse().ok()
}

fn element_expires(element: &Value) -> Duration {
    Duration::from_secs(element["elem"]["expires"].as_u64().unwrap_or(0))
}

fn recent_drop(element: &Value, reason: Reason) -> Option<RecentDrop> {
    let concat = element_value(element)["concat"].as_array()?;
    let source = concat.first()?.as_str()?.parse().ok()?;
    let protocol = concat.get(1)?.as_str()?.to_owned();
    let port = u16::try_from(concat.get(2)?.as_u64()?).ok()?;
    let packets = element["elem"]["counter"]["packets"].as_u64().unwrap_or(0);
    let expires = element["elem"]["expires"].as_u64().unwrap_or(0);
    Some(RecentDrop {
        source,
        protocol,
        port,
        reason,
        packets,
        last_seen_secs: crate::ruleset::RECENT_TIMEOUT
            .as_secs()
            .saturating_sub(expires),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_recent_and_ban_elements() {
        let json = r#"{"nftables":[{"metainfo":{}},{"set":{"name":"recent_policy4","elem":[{"elem":{"val":{"concat":["203.0.113.2","tcp",22]},"timeout":3600,"expires":3590,"counter":{"packets":3,"bytes":180}}}]}}]}"#;
        let elements = set_elements(json).unwrap();
        let drop = recent_drop(&elements[0], Reason::Policy).unwrap();
        assert_eq!(drop.source, "203.0.113.2".parse::<IpAddr>().unwrap());
        assert_eq!(
            (drop.protocol.as_str(), drop.port, drop.packets),
            ("tcp", 22, 3)
        );
        assert_eq!(drop.last_seen_secs, 10);

        let json = r#"{"nftables":[{"set":{"name":"ban4","elem":[{"elem":{"val":"198.51.100.7","timeout":3600,"expires":3000}}, "198.51.100.8"]}}]}"#;
        let elements = set_elements(json).unwrap();
        assert_eq!(element_ip(&elements[0]), "198.51.100.7".parse().ok());
        assert_eq!(element_expires(&elements[0]), Duration::from_secs(3000));
        assert_eq!(element_ip(&elements[1]), "198.51.100.8".parse().ok());
    }

    #[test]
    fn keeps_the_useful_part_of_errors() {
        let err = "/dev/stdin:3:5-9: Error: syntax error\n  foo bar\n      ^^^\n";
        assert_eq!(
            trim_error(err),
            "/dev/stdin:3:5-9: Error: syntax error;   foo bar"
        );
    }
}
