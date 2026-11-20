//! Login rate limiting.
//!
//! Failed logins are counted per client address and per username over a
//! sliding window. Past the limit, further attempts are refused without
//! checking the password, so guessing is slow even from many machines
//! against one account.

use std::collections::{HashMap, VecDeque};
use std::net::IpAddr;
use std::sync::Mutex;
use std::time::{Duration, Instant};

/// Failed attempts allowed per key within [`WINDOW`].
pub const MAX_FAILURES: usize = 10;
/// How long a failed attempt counts.
pub const WINDOW: Duration = Duration::from_secs(15 * 60);

/// Counts failed logins.
#[derive(Debug, Default)]
pub struct LoginLimiter {
    failures: Mutex<HashMap<String, VecDeque<Instant>>>,
}

fn ip_key(ip: IpAddr) -> String {
    // IPv6 clients usually control a whole /64, so count it as one.
    match ip.to_canonical() {
        IpAddr::V4(v4) => format!("ip:{v4}"),
        IpAddr::V6(v6) => {
            let s = v6.segments();
            format!("ip:{:x}:{:x}:{:x}:{:x}::/64", s[0], s[1], s[2], s[3])
        }
    }
}

fn user_key(username: &str) -> String {
    format!("user:{}", username.to_lowercase())
}

impl LoginLimiter {
    /// How long until `ip` or `username` may try again, if they are over
    /// the limit.
    pub fn blocked(&self, ip: IpAddr, username: Option<&str>) -> Option<Duration> {
        let now = Instant::now();
        let mut map = self
            .failures
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let keys = std::iter::once(ip_key(ip)).chain(username.map(user_key));
        keys.filter_map(|key| {
            let entries = map.get_mut(&key)?;
            while entries
                .front()
                .is_some_and(|t| now.duration_since(*t) >= WINDOW)
            {
                entries.pop_front();
            }
            if entries.len() < MAX_FAILURES {
                return None;
            }
            let oldest = *entries.front()?;
            Some(WINDOW.saturating_sub(now.duration_since(oldest)))
        })
        .max()
    }

    /// Records a failed attempt.
    pub fn fail(&self, ip: IpAddr, username: Option<&str>) {
        let now = Instant::now();
        let mut map = self
            .failures
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        // Bound memory: forget everything stale once the map gets large.
        if map.len() > 10_000 {
            map.retain(|_, v| v.back().is_some_and(|t| now.duration_since(*t) < WINDOW));
        }
        for key in std::iter::once(ip_key(ip)).chain(username.map(user_key)) {
            let entries = map.entry(key).or_default();
            entries.push_back(now);
            if entries.len() > MAX_FAILURES {
                entries.pop_front();
            }
        }
    }

    /// Forgets the failures of a client that just logged in.
    pub fn succeed(&self, ip: IpAddr) {
        self.failures
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .remove(&ip_key(ip));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn blocks_after_too_many_failures() {
        let limiter = LoginLimiter::default();
        let ip: IpAddr = "192.0.2.1".parse().unwrap();
        let other: IpAddr = "192.0.2.2".parse().unwrap();
        for _ in 0..MAX_FAILURES {
            assert_eq!(limiter.blocked(ip, Some("admin")), None);
            limiter.fail(ip, Some("admin"));
        }
        assert!(limiter.blocked(ip, Some("someone")).is_some());
        // The account is protected from other addresses too.
        assert!(limiter.blocked(other, Some("ADMIN")).is_some());
        assert_eq!(limiter.blocked(other, Some("someone")), None);
        limiter.succeed(ip);
        assert_eq!(limiter.blocked(ip, None), None);
    }

    #[test]
    fn ipv6_counts_per_64() {
        let limiter = LoginLimiter::default();
        for i in 0..MAX_FAILURES {
            let ip: IpAddr = format!("2001:db8::{i:x}").parse().unwrap();
            limiter.fail(ip, None);
        }
        assert!(
            limiter
                .blocked("2001:db8::ffff".parse().unwrap(), None)
                .is_some()
        );
        assert_eq!(
            limiter.blocked("2001:db8:1::1".parse().unwrap(), None),
            None
        );
    }
}
