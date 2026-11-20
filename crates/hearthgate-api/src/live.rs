//! Live data for the dashboard: traffic rates and history, the log tail,
//! and the Server-Sent Events stream that carries both.

use std::collections::{HashMap, VecDeque};
use std::convert::Infallible;
use std::fmt::Write as _;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::{Duration, Instant};

use axum::Json;
use axum::extract::{Query, State};
use axum::response::sse::{Event, KeepAlive, Sse};
use futures_util::Stream;
use hearthgate_proxy::Gateway;
use serde::{Deserialize, Serialize};
use tokio::sync::broadcast;
use tokio_util::sync::CancellationToken;
use tracing::field::{Field, Visit};
use tracing_subscriber::layer::Context;
use utoipa::{IntoParams, ToSchema};

use crate::ApiState;
use crate::routes::ApiResult;

/// How often traffic counters are sampled.
pub const SAMPLE_INTERVAL: Duration = Duration::from_secs(5);
/// How often the event stream sends a status update.
const STATUS_INTERVAL: Duration = Duration::from_secs(2);
/// Minutes of traffic history kept in memory.
const HISTORY_MINUTES: usize = 24 * 60;
/// Log lines kept for the dashboard.
const LOG_LINES: usize = 500;

/// Requests per second for one upstream, over the last sample.
#[derive(Debug, Clone, Copy, Default, PartialEq, Serialize, ToSchema)]
pub struct Rate {
    /// Requests (HTTP) or connections (TCP and UDP sessions) per second.
    pub requests: f64,
    /// Failed connections or requests per second.
    pub failures: f64,
}

/// Traffic in one hour.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, ToSchema)]
pub struct TrafficBucket {
    /// Start of the hour, seconds since the Unix epoch.
    pub start: i64,
    pub requests: u64,
    pub failures: u64,
}

#[derive(Debug, Default)]
struct Traffic {
    /// Last seen totals per upstream: (requests, failures).
    totals: HashMap<(String, u16), (u64, u64)>,
    last_sample: Option<Instant>,
    rates: HashMap<(String, u16), Rate>,
    /// (minute start, requests, failures), oldest first.
    minutes: VecDeque<(i64, u64, u64)>,
}

/// One log line.
#[derive(Debug, Clone, Serialize, ToSchema)]
pub struct LogLine {
    /// Increases by one per line, so a client can tell what it has seen.
    pub id: u64,
    /// Milliseconds since the Unix epoch.
    pub time: i64,
    /// `error`, `warn`, `info`, `debug` or `trace`.
    pub level: String,
    /// The message with its fields, as in the service log.
    pub message: String,
}

/// The last log lines, and a channel announcing new ones.
#[derive(Debug)]
pub struct LogBuffer {
    lines: Mutex<VecDeque<LogLine>>,
    next_id: AtomicU64,
    sender: broadcast::Sender<LogLine>,
}

impl Default for LogBuffer {
    fn default() -> Self {
        Self {
            lines: Mutex::new(VecDeque::with_capacity(LOG_LINES)),
            next_id: AtomicU64::new(1),
            sender: broadcast::channel(256).0,
        }
    }
}

impl LogBuffer {
    /// Adds a line.
    pub fn push(&self, level: &str, message: String) {
        let line = LogLine {
            id: self.next_id.fetch_add(1, Ordering::Relaxed),
            time: unix_millis(),
            level: level.to_owned(),
            message,
        };
        {
            let mut lines = self.lines.lock().unwrap_or_else(PoisonError::into_inner);
            if lines.len() == LOG_LINES {
                lines.pop_front();
            }
            lines.push_back(line.clone());
        }
        // Nobody listening is fine.
        let _ = self.sender.send(line);
    }

    /// The last `limit` lines, oldest first.
    #[must_use]
    pub fn tail(&self, limit: usize) -> Vec<LogLine> {
        let lines = self.lines.lock().unwrap_or_else(PoisonError::into_inner);
        lines
            .iter()
            .skip(lines.len().saturating_sub(limit))
            .cloned()
            .collect()
    }

    fn subscribe(&self) -> broadcast::Receiver<LogLine> {
        self.sender.subscribe()
    }
}

/// A `tracing` layer that copies every event into a [`LogBuffer`].
#[derive(Debug, Clone)]
pub struct LogLayer(pub Arc<LogBuffer>);

#[derive(Default)]
struct MessageVisitor {
    message: String,
    fields: String,
}

impl Visit for MessageVisitor {
    fn record_str(&mut self, field: &Field, value: &str) {
        if field.name() == "message" {
            self.message.push_str(value);
        } else {
            let _ = write!(self.fields, " {}={value}", field.name());
        }
    }

    fn record_debug(&mut self, field: &Field, value: &dyn std::fmt::Debug) {
        if field.name() == "message" {
            let _ = write!(self.message, "{value:?}");
        } else {
            let _ = write!(self.fields, " {}={value:?}", field.name());
        }
    }
}

impl<S: tracing::Subscriber> tracing_subscriber::Layer<S> for LogLayer {
    fn on_event(&self, event: &tracing::Event<'_>, _ctx: Context<'_, S>) {
        let mut visitor = MessageVisitor::default();
        event.record(&mut visitor);
        let level = event.metadata().level().as_str().to_ascii_lowercase();
        self.0.push(&level, visitor.message + &visitor.fields);
    }
}

/// Live data shared by the API handlers and the sampler.
#[derive(Debug)]
pub struct Live {
    started: Instant,
    traffic: Mutex<Traffic>,
    pub logs: Arc<LogBuffer>,
}

impl Default for Live {
    fn default() -> Self {
        Self::new(Arc::default())
    }
}

fn unix_millis() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| i64::try_from(d.as_millis()).unwrap_or(i64::MAX))
}

impl Live {
    #[must_use]
    pub fn new(logs: Arc<LogBuffer>) -> Self {
        Self {
            started: Instant::now(),
            traffic: Mutex::default(),
            logs,
        }
    }

    /// Seconds since the gateway started.
    #[must_use]
    pub fn uptime(&self) -> u64 {
        self.started.elapsed().as_secs()
    }

    /// Reads every upstream's counters and updates rates and history.
    pub fn sample(&self, gateway: &Gateway) {
        let now = Instant::now();
        let minute = crate::secret::now() / 60 * 60;
        let mut traffic = self.traffic.lock().unwrap_or_else(PoisonError::into_inner);
        let elapsed = traffic
            .last_sample
            .map_or(0.0, |t| now.duration_since(t).as_secs_f64());
        traffic.last_sample = Some(now);
        let (mut requests, mut failures) = (0u64, 0u64);
        let mut rates = HashMap::new();
        let mut totals = HashMap::new();
        for u in gateway.upstreams().all() {
            let key = (u.service.clone(), u.port);
            let current = (
                u.connections_total.load(Ordering::Relaxed),
                u.failures_total.load(Ordering::Relaxed),
            );
            // A counter that went down was reset; count from zero.
            let delta = match traffic.totals.get(&key) {
                Some(&(r, f)) => (
                    current.0.checked_sub(r).unwrap_or(current.0),
                    current.1.checked_sub(f).unwrap_or(current.1),
                ),
                None if elapsed > 0.0 => current,
                None => (0, 0),
            };
            requests += delta.0;
            failures += delta.1;
            if elapsed > 0.0 {
                #[allow(clippy::cast_precision_loss)]
                rates.insert(
                    key.clone(),
                    Rate {
                        requests: delta.0 as f64 / elapsed,
                        failures: delta.1 as f64 / elapsed,
                    },
                );
            }
            totals.insert(key, current);
        }
        traffic.totals = totals;
        traffic.rates = rates;
        match traffic.minutes.back_mut() {
            Some(last) if last.0 == minute => {
                last.1 += requests;
                last.2 += failures;
            }
            _ => {
                traffic.minutes.push_back((minute, requests, failures));
                if traffic.minutes.len() > HISTORY_MINUTES {
                    traffic.minutes.pop_front();
                }
            }
        }
    }

    /// The current rate for an upstream.
    #[must_use]
    pub fn rate(&self, service: &str, port: u16) -> Rate {
        self.traffic
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .rates
            .get(&(service.to_owned(), port))
            .copied()
            .unwrap_or_default()
    }

    /// The last 24 hours, one bucket per hour, oldest first. Hours before
    /// the gateway started are zero.
    #[must_use]
    pub fn history(&self) -> Vec<TrafficBucket> {
        let this_hour = crate::secret::now() / 3600 * 3600;
        let mut buckets: Vec<TrafficBucket> = (0..24)
            .rev()
            .map(|i| TrafficBucket {
                start: this_hour - i * 3600,
                requests: 0,
                failures: 0,
            })
            .collect();
        let first = buckets[0].start;
        let traffic = self.traffic.lock().unwrap_or_else(PoisonError::into_inner);
        for &(minute, requests, failures) in &traffic.minutes {
            if minute < first {
                continue;
            }
            if let Some(b) = usize::try_from((minute - first) / 3600)
                .ok()
                .and_then(|i| buckets.get_mut(i))
            {
                b.requests += requests;
                b.failures += failures;
            }
        }
        buckets
    }
}

/// Samples traffic until `stop` fires.
pub fn spawn_sampler(live: Arc<Live>, gateway: Arc<Gateway>, stop: CancellationToken) {
    tokio::spawn(async move {
        let mut tick = tokio::time::interval(SAMPLE_INTERVAL);
        loop {
            tokio::select! {
                () = stop.cancelled() => return,
                _ = tick.tick() => live.sample(&gateway),
            }
        }
    });
}

#[derive(Debug, Deserialize, IntoParams)]
pub struct LogQuery {
    /// How many lines to return, newest last (default 200).
    pub limit: Option<usize>,
}

/// The last lines of the service log.
#[utoipa::path(
    get, path = "/api/v1/logs", params(LogQuery),
    responses((status = 200, body = Vec<LogLine>))
)]
pub(crate) async fn logs(
    State(state): State<ApiState>,
    Query(q): Query<LogQuery>,
) -> ApiResult<Vec<LogLine>> {
    Ok(Json(
        state.live.logs.tail(q.limit.unwrap_or(200).min(LOG_LINES)),
    ))
}

/// Requests per hour over the last 24 hours.
#[utoipa::path(
    get, path = "/api/v1/traffic",
    responses((status = 200, body = Vec<TrafficBucket>))
)]
pub(crate) async fn traffic(State(state): State<ApiState>) -> ApiResult<Vec<TrafficBucket>> {
    Ok(Json(state.live.history()))
}

enum Next {
    Status,
    Log(LogLine),
    Lagged,
    Closed,
}

/// Live updates as Server-Sent Events: a `status` event (the same body as
/// `/api/v1/status`) every two seconds, and a `log` event per log line.
#[utoipa::path(
    get, path = "/api/v1/events",
    responses((status = 200, description = "An event stream", content_type = "text/event-stream"))
)]
pub(crate) async fn events(
    State(state): State<ApiState>,
) -> Sse<impl Stream<Item = Result<Event, Infallible>>> {
    let logs = state.live.logs.subscribe();
    let mut tick = tokio::time::interval(STATUS_INTERVAL);
    tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    let stream = futures_util::stream::unfold(
        (state, logs, tick),
        |(state, mut logs, mut tick)| async move {
            let next = tokio::select! {
                _ = tick.tick() => Next::Status,
                line = logs.recv() => match line {
                    Ok(line) => Next::Log(line),
                    Err(broadcast::error::RecvError::Lagged(_)) => Next::Lagged,
                    Err(broadcast::error::RecvError::Closed) => Next::Closed,
                },
            };
            let event = match next {
                Next::Status => match crate::routes::status_snapshot(&state).await {
                    Ok(s) => Event::default().event("status").json_data(s),
                    Err(_) => return None,
                },
                Next::Log(line) => Event::default().event("log").json_data(line),
                // Tell the client to fetch the tail again.
                Next::Lagged => Ok(Event::default().event("log-reset").data("")),
                Next::Closed => return None,
            };
            let event = event.unwrap_or_else(|_| Event::default().comment("skipped"));
            Some((Ok(event), (state, logs, tick)))
        },
    );
    Sse::new(stream).keep_alive(KeepAlive::default())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn log_buffer_keeps_the_tail() {
        let logs = LogBuffer::default();
        for i in 0..(LOG_LINES + 5) {
            logs.push("info", format!("line {i}"));
        }
        let tail = logs.tail(3);
        assert_eq!(tail.len(), 3);
        assert_eq!(tail[2].message, format!("line {}", LOG_LINES + 4));
        assert_eq!(tail[2].id, u64::try_from(LOG_LINES + 5).unwrap());
        assert_eq!(logs.tail(10_000).len(), LOG_LINES);
    }

    #[test]
    fn history_has_24_hours() {
        let live = Live::default();
        let h = live.history();
        assert_eq!(h.len(), 24);
        assert_eq!(h[23].start - h[0].start, 23 * 3600);
        assert!(h.iter().all(|b| b.requests == 0));
    }
}
