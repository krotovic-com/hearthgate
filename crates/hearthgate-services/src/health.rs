//! Active health checks.
//!
//! Every service with a `health` section is checked on its own interval.
//! After `unhealthy_threshold` failures in a row its upstreams leave
//! rotation, and after `healthy_threshold` successes they come back. The
//! result is stored on the shared [`UpstreamState`], so every route using
//! the service sees it at once.

use std::collections::HashSet;
use std::sync::Arc;

use bytes::Bytes;
use hearthgate_model::{DesiredState, HealthCheck, HealthCheckKind, PortRef};
use hearthgate_proxy::{Endpoint, UpstreamState, Upstreams};
use http_body_util::Empty;
use hyper_util::rt::TokioIo;
use tokio::net::TcpStream;
use tokio_util::sync::CancellationToken;
use tracing::{info, warn};

/// One service to check.
#[derive(Debug, Clone)]
pub struct HealthJob {
    pub service: String,
    pub check: HealthCheck,
    /// Port to probe; `None` probes each upstream on its own port.
    pub port: Option<u16>,
    /// Upstreams (service ports used by routes) that take the result.
    pub targets: Vec<Arc<UpstreamState>>,
}

/// The health jobs for a desired state, against the upstreams of its
/// compiled plan.
#[must_use]
pub fn jobs(state: &DesiredState, upstreams: &Upstreams) -> Vec<HealthJob> {
    let all = upstreams.all();
    state
        .services
        .iter()
        .filter_map(|service| {
            let check = service.health.clone()?;
            let targets: Vec<_> = all
                .iter()
                .filter(|u| u.service == service.name)
                .cloned()
                .collect();
            if targets.is_empty() {
                return None;
            }
            let port = match &check.port {
                Some(PortRef::Number(n)) => Some(*n),
                Some(name @ PortRef::Name(_)) => service.resolve_port(name),
                None => None,
            };
            Some(HealthJob {
                service: service.name.clone(),
                check,
                port,
                targets,
            })
        })
        .collect()
}

/// Runs health checks until replaced.
#[derive(Debug, Default)]
pub struct HealthChecker {
    running: Option<CancellationToken>,
}

impl HealthChecker {
    /// Replaces the running checks with `jobs`. Health already known is
    /// kept, so a reload never puts a down upstream back in rotation.
    pub fn update(&mut self, jobs: Vec<HealthJob>, upstreams: &Upstreams) {
        if let Some(old) = self.running.take() {
            old.cancel();
        }
        // Upstreams whose service has no check (any more) are healthy.
        let checked: HashSet<&str> = jobs.iter().map(|j| j.service.as_str()).collect();
        for u in upstreams.all() {
            if !checked.contains(u.service.as_str()) {
                u.set_healthy(true);
            }
        }
        let cancel = CancellationToken::new();
        for job in jobs {
            for target in &job.targets {
                // Each target is probed separately: with no explicit port,
                // two ports of one service can differ in health.
                let probe_port = job.port.unwrap_or(target.port);
                let targets = if job.port.is_some() {
                    job.targets.clone()
                } else {
                    vec![Arc::clone(target)]
                };
                tokio::spawn(run(
                    job.service.clone(),
                    job.check.clone(),
                    probe_port,
                    targets,
                    cancel.clone(),
                ));
                if job.port.is_some() {
                    break;
                }
            }
        }
        self.running = Some(cancel);
    }

    /// Stops every check.
    pub fn stop(&mut self) {
        if let Some(c) = self.running.take() {
            c.cancel();
        }
    }
}

impl Drop for HealthChecker {
    fn drop(&mut self) {
        self.stop();
    }
}

async fn run(
    service: String,
    check: HealthCheck,
    port: u16,
    targets: Vec<Arc<UpstreamState>>,
    cancel: CancellationToken,
) {
    let (mut ok_streak, mut fail_streak) = (0u32, 0u32);
    let mut tick = tokio::time::interval(check.interval);
    tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    loop {
        tokio::select! {
            () = cancel.cancelled() => return,
            _ = tick.tick() => {}
        }
        let endpoint = with_port(&targets[0].endpoint(), port);
        let result = tokio::select! {
            () = cancel.cancelled() => return,
            r = probe(&endpoint, &check) => r,
        };
        let healthy_now = targets[0].healthy();
        match result {
            Ok(()) => {
                ok_streak += 1;
                fail_streak = 0;
                if !healthy_now && ok_streak >= check.healthy_threshold {
                    for t in &targets {
                        t.set_healthy(true);
                    }
                    info!(service = %service, upstream = %endpoint, "upstream is healthy again");
                }
            }
            Err(reason) => {
                fail_streak += 1;
                ok_streak = 0;
                if healthy_now && fail_streak >= check.unhealthy_threshold {
                    for t in &targets {
                        t.set_healthy(false);
                    }
                    warn!(
                        service = %service,
                        upstream = %endpoint,
                        "upstream is unhealthy and out of rotation: {reason}"
                    );
                }
            }
        }
    }
}

fn with_port(endpoint: &Endpoint, port: u16) -> Endpoint {
    match endpoint {
        Endpoint::Socket(addr) => Endpoint::Socket(std::net::SocketAddr::new(addr.ip(), port)),
        Endpoint::Host(h, _) => Endpoint::Host(h.clone(), port),
        Endpoint::Unresolved => Endpoint::Unresolved,
    }
}

/// Runs one check.
pub async fn probe(endpoint: &Endpoint, check: &HealthCheck) -> Result<(), String> {
    tokio::time::timeout(check.timeout, probe_inner(endpoint, check))
        .await
        .map_err(|_| format!("no answer within {:?}", check.timeout))?
}

async fn probe_inner(endpoint: &Endpoint, check: &HealthCheck) -> Result<(), String> {
    let addrs = endpoint.resolve().await.map_err(|e| e.to_string())?;
    let mut last = "no address".to_owned();
    for addr in addrs {
        let stream = match TcpStream::connect(addr).await {
            Ok(s) => s,
            Err(e) => {
                last = format!("connect to {addr}: {e}");
                continue;
            }
        };
        if check.kind == HealthCheckKind::Tcp {
            return Ok(());
        }
        let (mut send, conn) = hyper::client::conn::http1::handshake(TokioIo::new(stream))
            .await
            .map_err(|e| e.to_string())?;
        tokio::spawn(conn);
        let host = match endpoint {
            Endpoint::Host(h, _) => h.clone(),
            _ => addr.ip().to_string(),
        };
        let req = http::Request::get(&check.path)
            .header(http::header::HOST, host)
            .header(http::header::USER_AGENT, "hearthgate-health-check")
            .body(Empty::<Bytes>::new())
            .map_err(|e| e.to_string())?;
        let resp = send.send_request(req).await.map_err(|e| e.to_string())?;
        let status = resp.status();
        return if status.is_success() || status.is_redirection() {
            Ok(())
        } else {
            Err(format!("HTTP {}", status.as_u16()))
        };
    }
    Err(last)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    fn check(kind: HealthCheckKind) -> HealthCheck {
        HealthCheck {
            kind,
            port: None,
            path: "/healthz".into(),
            interval: Duration::from_millis(50),
            timeout: Duration::from_millis(500),
            healthy_threshold: 2,
            unhealthy_threshold: 2,
        }
    }

    async fn http_server(status: &'static str) -> u16 {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        let l = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = l.local_addr().unwrap().port();
        tokio::spawn(async move {
            loop {
                let (mut s, _) = l.accept().await.unwrap();
                tokio::spawn(async move {
                    let mut buf = [0u8; 1024];
                    let _ = s.read(&mut buf).await;
                    let _ = s
                        .write_all(
                            format!("HTTP/1.1 {status}\r\ncontent-length: 0\r\n\r\n").as_bytes(),
                        )
                        .await;
                });
            }
        });
        port
    }

    #[tokio::test]
    async fn probes() {
        let ok = http_server("200 OK").await;
        let bad = http_server("503 Service Unavailable").await;
        let ep = |p| Endpoint::from_host("127.0.0.1", p);
        assert!(probe(&ep(ok), &check(HealthCheckKind::Http)).await.is_ok());
        assert_eq!(
            probe(&ep(bad), &check(HealthCheckKind::Http)).await,
            Err("HTTP 503".into())
        );
        assert!(probe(&ep(bad), &check(HealthCheckKind::Tcp)).await.is_ok());
        let closed = std::net::TcpListener::bind("127.0.0.1:0")
            .unwrap()
            .local_addr()
            .unwrap()
            .port();
        assert!(
            probe(&ep(closed), &check(HealthCheckKind::Tcp))
                .await
                .is_err()
        );
    }

    #[tokio::test]
    async fn takes_upstreams_out_and_back() {
        let ups = Upstreams::default();
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        drop(listener);
        let state = ups.get("svc", port, Endpoint::from_host("127.0.0.1", port));
        let mut checker = HealthChecker::default();
        checker.update(
            vec![HealthJob {
                service: "svc".into(),
                check: check(HealthCheckKind::Tcp),
                port: None,
                targets: vec![Arc::clone(&state)],
            }],
            &ups,
        );
        let wait = |want: bool| {
            let state = Arc::clone(&state);
            async move {
                for _ in 0..100 {
                    if state.healthy() == want {
                        return;
                    }
                    tokio::time::sleep(Duration::from_millis(20)).await;
                }
                panic!("health did not become {want}");
            }
        };
        wait(false).await;
        let _l = tokio::net::TcpListener::bind(("127.0.0.1", port))
            .await
            .unwrap();
        wait(true).await;
        // Removing the check marks the upstream healthy.
        state.set_healthy(false);
        checker.update(Vec::new(), &ups);
        assert!(state.healthy());
    }
}
