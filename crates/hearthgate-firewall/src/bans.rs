//! Automatic bans: adds up points per client address over a sliding window
//! and bans addresses that reach the threshold.

use std::collections::{HashMap, VecDeque};
use std::net::IpAddr;
use std::sync::Arc;
use std::time::{Duration, Instant};

use hearthgate_model::{AutoBan, Firewall, is_special_net};
use ipnet::IpNet;
use tokio::sync::{mpsc, watch};
use tokio_util::sync::CancellationToken;
use tracing::{info, warn};

use crate::Perimeter;

/// Addresses tracked at once; the oldest are forgotten beyond this.
const MAX_TRACKED: usize = 50_000;

/// A scored event.
#[derive(Debug, Clone, Copy)]
struct Hit {
    at: Instant,
    points: u32,
    kind: u8,
}

/// Points per address within the window.
#[derive(Debug, Default)]
pub struct Scores {
    hits: HashMap<IpAddr, VecDeque<Hit>>,
}

/// A ban decision.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Verdict {
    pub ip: IpAddr,
    pub points: u32,
    /// What the client did, for the log: `(description, count)`.
    pub reasons: Vec<(&'static str, u32)>,
}

impl Scores {
    /// Records a hit and returns a verdict when the address reaches the
    /// threshold. `kinds` names each `kind` index for the verdict.
    pub fn add(
        &mut self,
        ip: IpAddr,
        points: u32,
        kind: u8,
        now: Instant,
        settings: &AutoBan,
        kinds: &[&'static str],
    ) -> Option<Verdict> {
        if self.hits.len() >= MAX_TRACKED && !self.hits.contains_key(&ip) {
            self.prune(now, settings.window);
            if self.hits.len() >= MAX_TRACKED {
                return None;
            }
        }
        let entry = self.hits.entry(ip).or_default();
        entry.push_back(Hit {
            at: now,
            points,
            kind,
        });
        while entry
            .front()
            .is_some_and(|h| now.duration_since(h.at) > settings.window)
        {
            entry.pop_front();
        }
        let total: u32 = entry.iter().map(|h| h.points).sum();
        if total < settings.threshold {
            return None;
        }
        let mut reasons: Vec<(&'static str, u32)> = Vec::new();
        for hit in entry.iter() {
            let name = kinds.get(usize::from(hit.kind)).copied().unwrap_or("other");
            match reasons.iter_mut().find(|(n, _)| *n == name) {
                Some((_, c)) => *c += 1,
                None => reasons.push((name, 1)),
            }
        }
        self.hits.remove(&ip);
        Some(Verdict {
            ip,
            points: total,
            reasons,
        })
    }

    /// Forgets addresses with no hits inside the window.
    pub fn prune(&mut self, now: Instant, window: Duration) {
        self.hits.retain(|_, hits| {
            hits.back()
                .is_some_and(|h| now.duration_since(h.at) <= window)
        });
    }

    #[must_use]
    pub fn tracked(&self) -> usize {
        self.hits.len()
    }
}

/// Whether an address may be banned: never special networks (the LAN,
/// loopback, CGNAT…) and never the manual allow list.
#[must_use]
pub fn bannable(ip: IpAddr, firewall: &Firewall) -> bool {
    let net = IpNet::from(ip);
    !is_special_net(&net) && !firewall.allow.iter().any(|a| a.contains(&ip))
}

/// A report from the data plane: an address, points and a description.
pub type Report = (IpAddr, u32, &'static str);

/// Scores reports and bans offenders until `stop`.
pub async fn run(
    perimeter: Arc<Perimeter>,
    mut reports: mpsc::Receiver<Report>,
    settings: watch::Receiver<Firewall>,
    stop: CancellationToken,
) {
    let mut scores = Scores::default();
    let mut kinds: Vec<&'static str> = Vec::new();
    let mut prune = tokio::time::interval(Duration::from_secs(60));
    loop {
        tokio::select! {
            () = stop.cancelled() => return,
            _ = prune.tick() => {
                let window = settings.borrow().auto_ban.window;
                scores.prune(Instant::now(), window);
            }
            report = reports.recv() => {
                let Some((ip, points, what)) = report else { return };
                let firewall = settings.borrow().clone();
                if !firewall.enabled || !firewall.auto_ban.enabled || !bannable(ip, &firewall) {
                    continue;
                }
                let kind = if let Some(i) = kinds.iter().position(|k| *k == what) {
                    i
                } else {
                    kinds.push(what);
                    kinds.len() - 1
                };
                let kind = u8::try_from(kind).unwrap_or(u8::MAX);
                let Some(verdict) =
                    scores.add(ip, points, kind, Instant::now(), &firewall.auto_ban, &kinds)
                else {
                    continue;
                };
                let why: Vec<String> = verdict
                    .reasons
                    .iter()
                    .map(|(what, n)| format!("{n} {what}{}", if *n == 1 { "" } else { "s" }))
                    .collect();
                match perimeter.ban(ip, firewall.auto_ban.ban_time).await {
                    Ok(()) => info!(
                        "banned {ip} for {}: {} points in {} ({})",
                        humanize(firewall.auto_ban.ban_time),
                        verdict.points,
                        humanize(firewall.auto_ban.window),
                        why.join(", ")
                    ),
                    Err(e) => warn!("cannot ban {ip}: {e}"),
                }
            }
        }
    }
}

fn humanize(d: Duration) -> String {
    let s = d.as_secs();
    if s % 86_400 == 0 {
        format!("{}d", s / 86_400)
    } else if s % 3600 == 0 {
        format!("{}h", s / 3600)
    } else if s % 60 == 0 {
        format!("{}m", s / 60)
    } else {
        format!("{s}s")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bans_at_the_threshold_within_the_window() {
        let settings = AutoBan {
            threshold: 10,
            window: Duration::from_secs(60),
            ..AutoBan::default()
        };
        let kinds = ["failed login", "exploit probe"];
        let ip: IpAddr = "198.51.100.9".parse().unwrap();
        let start = Instant::now();
        let mut scores = Scores::default();
        for i in 0..9 {
            assert!(
                scores
                    .add(ip, 1, 0, start + Duration::from_secs(i), &settings, &kinds)
                    .is_none()
            );
        }
        // Hits older than the window fall out: at 65s only those from 5s
        // on count (4 points), plus this one.
        let later = start + Duration::from_secs(65);
        assert!(scores.add(ip, 1, 0, later, &settings, &kinds).is_none());
        let verdict = scores
            .add(ip, 5, 1, later, &settings, &kinds)
            .expect("10 points within the window");
        assert_eq!(verdict.points, 10);
        assert_eq!(
            verdict.reasons,
            vec![("failed login", 5), ("exploit probe", 1)]
        );
        // Banning resets the score.
        assert_eq!(scores.tracked(), 0);
    }

    #[test]
    fn never_bans_the_lan_or_the_allow_list() {
        let mut fw = Firewall::default();
        assert!(!bannable("192.168.1.20".parse().unwrap(), &fw));
        assert!(!bannable("100.64.0.5".parse().unwrap(), &fw));
        assert!(!bannable("::1".parse().unwrap(), &fw));
        assert!(!bannable("fe80::1".parse().unwrap(), &fw));
        assert!(bannable("198.51.100.9".parse().unwrap(), &fw));
        fw.allow = vec!["198.51.100.0/24".parse().unwrap()];
        assert!(!bannable("198.51.100.9".parse().unwrap(), &fw));
    }
}
