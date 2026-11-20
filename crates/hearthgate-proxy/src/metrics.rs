//! Prometheus metrics in the text exposition format.

use std::fmt::Write;
use std::sync::atomic::Ordering;

use crate::gateway::SocketStatus;
use crate::pool::Upstreams;

/// A certificate, for the expiry gauge.
#[derive(Debug, Clone)]
pub struct CertMetric {
    pub name: String,
    pub source: &'static str,
    /// Seconds since the Unix epoch.
    pub not_after: u64,
}

fn escape(value: &str) -> String {
    value
        .replace('\\', "\\\\")
        .replace('"', "\\\"")
        .replace('\n', "\\n")
}

fn header(out: &mut String, name: &str, kind: &str, help: &str) {
    let _ = writeln!(out, "# HELP {name} {help}");
    let _ = writeln!(out, "# TYPE {name} {kind}");
}

/// Renders every metric.
#[must_use]
pub fn render(
    sockets: &[SocketStatus],
    upstreams: &Upstreams,
    local: &[(u16, u64)],
    certs: &[CertMetric],
    revision: Option<i64>,
) -> String {
    let mut out = String::with_capacity(4096);

    header(
        &mut out,
        "hearthgate_revision",
        "gauge",
        "Revision currently running.",
    );
    let _ = writeln!(out, "hearthgate_revision {}", revision.unwrap_or(0));

    header(
        &mut out,
        "hearthgate_listener_open_connections",
        "gauge",
        "Open client connections per listening socket.",
    );
    for s in sockets {
        let _ = writeln!(
            out,
            "hearthgate_listener_open_connections{{socket=\"{}\"}} {}",
            escape(&s.key.to_string()),
            s.open_connections
        );
    }

    let all = upstreams.all();
    let label = |u: &crate::pool::UpstreamState| {
        format!("service=\"{}\",port=\"{}\"", escape(&u.service), u.port)
    };
    header(
        &mut out,
        "hearthgate_upstream_healthy",
        "gauge",
        "1 when the upstream takes traffic, 0 when unhealthy or ejected.",
    );
    for u in &all {
        let _ = writeln!(
            out,
            "hearthgate_upstream_healthy{{{}}} {}",
            label(u),
            u8::from(u.available())
        );
    }
    header(
        &mut out,
        "hearthgate_upstream_active",
        "gauge",
        "Open connections or in-flight requests per upstream.",
    );
    for u in &all {
        let _ = writeln!(
            out,
            "hearthgate_upstream_active{{{}}} {}",
            label(u),
            u.active()
        );
    }
    for (name, help, read) in [
        (
            "hearthgate_upstream_connections_total",
            "Connections or requests sent to the upstream.",
            (|u: &crate::pool::UpstreamState| u.connections_total.load(Ordering::Relaxed))
                as fn(&crate::pool::UpstreamState) -> u64,
        ),
        (
            "hearthgate_upstream_failures_total",
            "Failed connections or requests.",
            |u| u.failures_total.load(Ordering::Relaxed),
        ),
        (
            "hearthgate_upstream_sent_bytes_total",
            "Bytes sent to the upstream.",
            |u| u.bytes_sent.load(Ordering::Relaxed),
        ),
        (
            "hearthgate_upstream_received_bytes_total",
            "Bytes received from the upstream.",
            |u| u.bytes_received.load(Ordering::Relaxed),
        ),
    ] {
        header(&mut out, name, "counter", help);
        for u in &all {
            let _ = writeln!(out, "{name}{{{}}} {}", label(u), read(u));
        }
    }
    header(
        &mut out,
        "hearthgate_http_responses_total",
        "counter",
        "HTTP responses from upstreams by status class.",
    );
    for u in &all {
        for (class, n) in u.responses() {
            if n > 0 {
                let _ = writeln!(
                    out,
                    "hearthgate_http_responses_total{{{},class=\"{class}xx\"}} {n}",
                    label(u)
                );
            }
        }
    }
    header(
        &mut out,
        "hearthgate_http_local_responses_total",
        "counter",
        "Responses Hearthgate answered itself (redirects, 404, 502, ...).",
    );
    for (code, n) in local {
        let _ = writeln!(
            out,
            "hearthgate_http_local_responses_total{{code=\"{code}\"}} {n}"
        );
    }
    header(
        &mut out,
        "hearthgate_certificate_expiry_timestamp_seconds",
        "gauge",
        "When each certificate expires, in seconds since the Unix epoch.",
    );
    for c in certs {
        let _ = writeln!(
            out,
            "hearthgate_certificate_expiry_timestamp_seconds{{name=\"{}\",source=\"{}\"}} {}",
            escape(&c.name),
            c.source,
            c.not_after
        );
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pool::Endpoint;

    #[test]
    fn renders_prometheus_text() {
        let ups = Upstreams::default();
        let u = ups.get("app", 8080, Endpoint::from_host("127.0.0.1", 8080));
        u.record_response(200);
        u.record_response(503);
        u.set_healthy(false);
        let text = render(
            &[],
            &ups,
            &[(404, 2)],
            &[CertMetric {
                name: "web".into(),
                source: "automatic",
                not_after: 1_700_000_000,
            }],
            Some(7),
        );
        assert!(text.contains("hearthgate_revision 7\n"));
        assert!(text.contains("hearthgate_upstream_healthy{service=\"app\",port=\"8080\"} 0\n"));
        assert!(text.contains(
            "hearthgate_http_responses_total{service=\"app\",port=\"8080\",class=\"5xx\"} 1\n"
        ));
        assert!(text.contains("hearthgate_http_local_responses_total{code=\"404\"} 2\n"));
        assert!(text.contains(
            "hearthgate_certificate_expiry_timestamp_seconds{name=\"web\",source=\"automatic\"} 1700000000\n"
        ));
        // Every metric has HELP and TYPE lines.
        assert_eq!(
            text.matches("# HELP").count(),
            text.matches("# TYPE").count()
        );
    }
}
