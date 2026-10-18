//! Metrics (docs/research/m6-api.md, OP6, M6-29): Prometheus text written
//! by hand from counters, on `metrics.listen` only, off by default and
//! without authentication. Every label comes from a closed set: a route
//! template of `OPERATIONS`, a status class, a kind, a state, a lane, a
//! reason; never a raw path or a client's text.

use std::collections::BTreeMap;
use std::fmt::Write as _;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Mutex, PoisonError};
use std::time::Duration;

use crate::Shared;
use crate::jobs::model::Lane;
use crate::state::JobRow;

/// The histograms' buckets, in seconds.
const BUCKETS: [f64; 14] = [
    0.005, 0.01, 0.025, 0.05, 0.1, 0.25, 0.5, 1.0, 2.5, 5.0, 10.0, 30.0, 60.0, 300.0,
];

/// One histogram's counts (fixed buckets).
#[derive(Debug, Clone, Default)]
struct Histogram {
    buckets: [u64; BUCKETS.len()],
    sum: f64,
    count: u64,
}

impl Histogram {
    fn observe(&mut self, seconds: f64) {
        for (bucket, le) in self.buckets.iter_mut().zip(BUCKETS) {
            if seconds <= le {
                *bucket += 1;
            }
        }
        self.sum += seconds;
        self.count += 1;
    }
}

/// The server's counters (OP6).
#[derive(Debug, Default)]
pub struct Metrics {
    requests: Mutex<BTreeMap<(String, &'static str), u64>>,
    latency: Mutex<BTreeMap<String, Histogram>>,
    jobs: Mutex<BTreeMap<(&'static str, &'static str), u64>>,
    job_seconds: Mutex<BTreeMap<&'static str, Histogram>>,
    upload_bytes: AtomicU64,
    auth_failures: Mutex<BTreeMap<&'static str, u64>>,
    rate_limited: Mutex<BTreeMap<&'static str, u64>>,
    catalog_busy: AtomicU64,
}

fn lock<T>(mutex: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

/// A status's class: `2xx` … `5xx`.
fn class(status: u16) -> &'static str {
    match status / 100 {
        1 => "1xx",
        2 => "2xx",
        3 => "3xx",
        4 => "4xx",
        _ => "5xx",
    }
}

impl Metrics {
    /// A request answered: its route template, status and latency.
    pub fn request(&self, route: &str, status: u16, latency: Duration) {
        *lock(&self.requests)
            .entry((route.to_owned(), class(status)))
            .or_insert(0) += 1;
        lock(&self.latency)
            .entry(route.to_owned())
            .or_default()
            .observe(latency.as_secs_f64());
    }

    /// An upload's bytes kept.
    pub fn uploaded(&self, bytes: u64) {
        self.upload_bytes.fetch_add(bytes, Ordering::Relaxed);
    }

    /// An authentication failure, by reason.
    pub fn auth_failure(&self, reason: &'static str) {
        *lock(&self.auth_failures).entry(reason).or_insert(0) += 1;
    }

    /// A rate limit met, by what it limits.
    pub fn rate_limited(&self, what: &'static str) {
        *lock(&self.rate_limited).entry(what).or_insert(0) += 1;
    }

    /// The catalog answered busy.
    pub fn catalog_busy(&self) {
        self.catalog_busy.fetch_add(1, Ordering::Relaxed);
    }
}

/// A job ended (`fitment_jobs_total`, `fitment_job_seconds`).
pub(crate) fn job_ended(metrics: &Metrics, row: &JobRow) {
    *lock(&metrics.jobs)
        .entry((row.kind.name(), row.state.name()))
        .or_insert(0) += 1;
    if let (Some(started), Some(finished)) = (row.started_ms, row.finished_ms) {
        let ms = u64::try_from(finished.saturating_sub(started)).unwrap_or(0);
        lock(&metrics.job_seconds)
            .entry(row.kind.name())
            .or_default()
            .observe(Duration::from_millis(ms).as_secs_f64());
    }
}

fn header(out: &mut String, name: &str, kind: &str, help: &str) {
    let _ = writeln!(out, "# HELP {name} {help}");
    let _ = writeln!(out, "# TYPE {name} {kind}");
}

fn histogram(out: &mut String, name: &str, label: &str, value: &str, h: &Histogram) {
    for (count, le) in h.buckets.iter().zip(BUCKETS) {
        let _ = writeln!(
            out,
            "{name}_bucket{{{label}=\"{value}\",le=\"{le}\"}} {count}"
        );
    }
    let _ = writeln!(
        out,
        "{name}_bucket{{{label}=\"{value}\",le=\"+Inf\"}} {}",
        h.count
    );
    let _ = writeln!(out, "{name}_sum{{{label}=\"{value}\"}} {}", h.sum);
    let _ = writeln!(out, "{name}_count{{{label}=\"{value}\"}} {}", h.count);
}

/// The counters' part of the exposition.
fn counters(metrics: &Metrics, out: &mut String) {
    header(
        out,
        "fitment_http_requests_total",
        "counter",
        "requests answered",
    );
    for ((route, class), n) in lock(&metrics.requests).iter() {
        let _ = writeln!(
            out,
            "fitment_http_requests_total{{route=\"{route}\",code_class=\"{class}\"}} {n}"
        );
    }
    header(
        out,
        "fitment_http_request_seconds",
        "histogram",
        "request latency",
    );
    for (route, h) in lock(&metrics.latency).iter() {
        histogram(out, "fitment_http_request_seconds", "route", route, h);
    }
    header(
        out,
        "fitment_jobs_total",
        "counter",
        "jobs ended, by kind and state",
    );
    for ((kind, state), n) in lock(&metrics.jobs).iter() {
        let _ = writeln!(
            out,
            "fitment_jobs_total{{kind=\"{kind}\",state=\"{state}\"}} {n}"
        );
    }
    header(out, "fitment_job_seconds", "histogram", "jobs' run time");
    for (kind, h) in lock(&metrics.job_seconds).iter() {
        histogram(out, "fitment_job_seconds", "kind", kind, h);
    }
    header(
        out,
        "fitment_upload_bytes_total",
        "counter",
        "uploaded bytes kept",
    );
    let _ = writeln!(
        out,
        "fitment_upload_bytes_total {}",
        metrics.upload_bytes.load(Ordering::Relaxed)
    );
    header(
        out,
        "fitment_auth_failures_total",
        "counter",
        "authentication failures",
    );
    for (reason, n) in lock(&metrics.auth_failures).iter() {
        let _ = writeln!(
            out,
            "fitment_auth_failures_total{{reason=\"{reason}\"}} {n}"
        );
    }
    header(
        out,
        "fitment_rate_limited_total",
        "counter",
        "requests refused by a rate limit",
    );
    for (what, n) in lock(&metrics.rate_limited).iter() {
        let _ = writeln!(out, "fitment_rate_limited_total{{what=\"{what}\"}} {n}");
    }
    header(
        out,
        "fitment_catalog_busy_total",
        "counter",
        "catalog_busy answers",
    );
    let _ = writeln!(
        out,
        "fitment_catalog_busy_total {}",
        metrics.catalog_busy.load(Ordering::Relaxed)
    );
}

/// The exposition (OP6). Blocking: it reads the catalog and `server.db`.
pub(crate) fn render(shared: &Shared) -> String {
    let mut out = String::new();
    counters(&shared.metrics, &mut out);
    header(
        &mut out,
        "fitment_jobs_queued",
        "gauge",
        "jobs queued, by lane",
    );
    for lane in Lane::ALL {
        let (queued, _) = shared.jobs.counts(lane);
        let _ = writeln!(
            out,
            "fitment_jobs_queued{{lane=\"{}\"}} {queued}",
            lane.name()
        );
    }
    header(
        &mut out,
        "fitment_jobs_running",
        "gauge",
        "jobs running, by lane",
    );
    for lane in Lane::ALL {
        let (_, running) = shared.jobs.counts(lane);
        let _ = writeln!(
            out,
            "fitment_jobs_running{{lane=\"{}\"}} {running}",
            lane.name()
        );
    }
    header(
        &mut out,
        "fitment_cpu_permits_free",
        "gauge",
        "CPU permits free (JS2)",
    );
    let _ = writeln!(out, "fitment_cpu_permits_free {}", shared.jobs.cpu_free().0);
    header(
        &mut out,
        "fitment_jobs_abandoned",
        "gauge",
        "jobs given up in a call that never returned, their threads not returned (JS8)",
    );
    let _ = writeln!(out, "fitment_jobs_abandoned {}", shared.jobs.abandoned());
    header(
        &mut out,
        "fitment_memory_mib_free",
        "gauge",
        "memory budget free, MiB (JS3)",
    );
    let _ = writeln!(
        out,
        "fitment_memory_mib_free {}",
        shared.jobs.memory_free().0
    );
    if let Ok(info) = shared.catalog.info() {
        header(
            &mut out,
            "fitment_catalog_generation",
            "gauge",
            "the catalog's generation",
        );
        let _ = writeln!(out, "fitment_catalog_generation {}", info.generation);
        header(
            &mut out,
            "fitment_catalog_entries",
            "gauge",
            "live entries, by status",
        );
        for (status, n) in [
            ("indexed", info.indexed),
            ("quarantined", info.quarantined),
            ("stale", info.stale),
        ] {
            let _ = writeln!(out, "fitment_catalog_entries{{status=\"{status}\"}} {n}");
        }
        header(
            &mut out,
            "fitment_catalog_stale",
            "gauge",
            "1 while some entry is stale",
        );
        let _ = writeln!(out, "fitment_catalog_stale {}", u8::from(info.state.stale));
    }
    let bytes = shared.db.object_total().unwrap_or(0);
    header(
        &mut out,
        "fitment_state_object_bytes",
        "gauge",
        "bytes in the object store",
    );
    let _ = writeln!(out, "fitment_state_object_bytes {bytes}");
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    /// OP6: a histogram's buckets are cumulative.
    #[test]
    fn buckets_are_cumulative() {
        let mut h = Histogram::default();
        h.observe(0.003);
        h.observe(0.2);
        assert_eq!(h.buckets[0], 1);
        assert_eq!(h.buckets[5], 2);
        assert_eq!(h.count, 2);
        let metrics = Metrics::default();
        metrics.request("/v1/info", 200, Duration::from_millis(3));
        let mut out = String::new();
        counters(&metrics, &mut out);
        assert!(
            out.contains("fitment_http_requests_total{route=\"/v1/info\",code_class=\"2xx\"} 1")
        );
    }
}
