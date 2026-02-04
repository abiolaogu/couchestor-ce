//! Metrics Collector
//!
//! Lock-free metrics collection for high-performance monitoring.

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use parking_lot::RwLock;

/// Observability configuration
#[derive(Debug, Clone)]
pub struct ObservabilityConfig {
    /// Enable metrics collection
    pub metrics_enabled: bool,
    /// Enable tracing
    pub tracing_enabled: bool,
    /// Trace sampling rate (0.0 - 1.0)
    pub trace_sampling_rate: f64,
    /// Metrics collection interval
    pub collection_interval: Duration,
    /// Enable histograms
    pub histograms_enabled: bool,
}

impl Default for ObservabilityConfig {
    fn default() -> Self {
        Self {
            metrics_enabled: true,
            tracing_enabled: true,
            trace_sampling_rate: 0.1, // 10% sampling
            collection_interval: Duration::from_secs(15),
            histograms_enabled: true,
        }
    }
}

/// Counter metric
#[derive(Default)]
pub struct Counter {
    value: AtomicU64,
}

impl Counter {
    /// Create a new counter
    pub fn new() -> Self {
        Self::default()
    }

    /// Increment by 1
    pub fn inc(&self) {
        self.value.fetch_add(1, Ordering::Relaxed);
    }

    /// Increment by n
    pub fn add(&self, n: u64) {
        self.value.fetch_add(n, Ordering::Relaxed);
    }

    /// Get current value
    pub fn get(&self) -> u64 {
        self.value.load(Ordering::Relaxed)
    }

    /// Reset to zero
    pub fn reset(&self) {
        self.value.store(0, Ordering::Relaxed);
    }
}

/// Gauge metric
#[derive(Default)]
pub struct Gauge {
    value: AtomicU64,
}

impl Gauge {
    /// Create a new gauge
    pub fn new() -> Self {
        Self::default()
    }

    /// Set value
    pub fn set(&self, value: u64) {
        self.value.store(value, Ordering::Relaxed);
    }

    /// Increment by 1
    pub fn inc(&self) {
        self.value.fetch_add(1, Ordering::Relaxed);
    }

    /// Decrement by 1
    pub fn dec(&self) {
        self.value.fetch_sub(1, Ordering::Relaxed);
    }

    /// Get current value
    pub fn get(&self) -> u64 {
        self.value.load(Ordering::Relaxed)
    }
}

/// Histogram bucket
#[derive(Debug, Clone)]
pub struct HistogramBucket {
    /// Upper bound (exclusive)
    pub le: f64,
    /// Count of observations
    pub count: u64,
}

/// Histogram metric
pub struct Histogram {
    /// Bucket boundaries
    boundaries: Vec<f64>,
    /// Bucket counts
    buckets: Vec<AtomicU64>,
    /// Sum of all observations
    sum: AtomicU64,
    /// Count of observations
    count: AtomicU64,
}

impl Histogram {
    /// Create a new histogram with default buckets
    pub fn new() -> Self {
        Self::with_buckets(vec![
            0.001, 0.005, 0.01, 0.025, 0.05, 0.1, 0.25, 0.5, 1.0, 2.5, 5.0, 10.0,
        ])
    }

    /// Create with custom buckets
    pub fn with_buckets(mut boundaries: Vec<f64>) -> Self {
        boundaries.sort_by(|a, b| a.partial_cmp(b).unwrap());
        let buckets: Vec<AtomicU64> = boundaries.iter().map(|_| AtomicU64::new(0)).collect();

        Self {
            boundaries,
            buckets,
            sum: AtomicU64::new(0),
            count: AtomicU64::new(0),
        }
    }

    /// Create for latency tracking (in seconds)
    pub fn latency() -> Self {
        Self::with_buckets(vec![
            0.0001, 0.0005, 0.001, 0.005, 0.01, 0.025, 0.05, 0.1, 0.25, 0.5, 1.0,
        ])
    }

    /// Observe a value
    pub fn observe(&self, value: f64) {
        self.count.fetch_add(1, Ordering::Relaxed);
        self.sum
            .fetch_add((value * 1_000_000.0) as u64, Ordering::Relaxed);

        for (i, &boundary) in self.boundaries.iter().enumerate() {
            if value <= boundary {
                self.buckets[i].fetch_add(1, Ordering::Relaxed);
            }
        }
    }

    /// Observe duration
    pub fn observe_duration(&self, duration: Duration) {
        self.observe(duration.as_secs_f64());
    }

    /// Get count
    pub fn count(&self) -> u64 {
        self.count.load(Ordering::Relaxed)
    }

    /// Get sum (scaled by 1M for precision)
    pub fn sum(&self) -> f64 {
        self.sum.load(Ordering::Relaxed) as f64 / 1_000_000.0
    }

    /// Get buckets
    pub fn buckets(&self) -> Vec<HistogramBucket> {
        self.boundaries
            .iter()
            .zip(self.buckets.iter())
            .map(|(&le, count)| HistogramBucket {
                le,
                count: count.load(Ordering::Relaxed),
            })
            .collect()
    }

    /// Get average
    pub fn average(&self) -> f64 {
        let count = self.count();
        if count == 0 {
            return 0.0;
        }
        self.sum() / count as f64
    }
}

impl Default for Histogram {
    fn default() -> Self {
        Self::new()
    }
}

/// Metrics collector
pub struct MetricsCollector {
    /// Configuration
    config: ObservabilityConfig,
    /// Counters
    counters: RwLock<HashMap<String, Arc<Counter>>>,
    /// Gauges
    gauges: RwLock<HashMap<String, Arc<Gauge>>>,
    /// Histograms
    histograms: RwLock<HashMap<String, Arc<Histogram>>>,
    /// Start time
    start_time: Instant,
}

impl MetricsCollector {
    /// Create a new metrics collector
    pub fn new(config: ObservabilityConfig) -> Self {
        Self {
            config,
            counters: RwLock::new(HashMap::new()),
            gauges: RwLock::new(HashMap::new()),
            histograms: RwLock::new(HashMap::new()),
            start_time: Instant::now(),
        }
    }

    /// Create with default configuration
    pub fn default_config() -> Self {
        Self::new(ObservabilityConfig::default())
    }

    /// Get or create a counter
    pub fn counter(&self, name: &str) -> Arc<Counter> {
        let counters = self.counters.read();
        if let Some(counter) = counters.get(name) {
            return counter.clone();
        }
        drop(counters);

        let mut counters = self.counters.write();
        counters
            .entry(name.to_string())
            .or_insert_with(|| Arc::new(Counter::new()))
            .clone()
    }

    /// Get or create a gauge
    pub fn gauge(&self, name: &str) -> Arc<Gauge> {
        let gauges = self.gauges.read();
        if let Some(gauge) = gauges.get(name) {
            return gauge.clone();
        }
        drop(gauges);

        let mut gauges = self.gauges.write();
        gauges
            .entry(name.to_string())
            .or_insert_with(|| Arc::new(Gauge::new()))
            .clone()
    }

    /// Get or create a histogram
    pub fn histogram(&self, name: &str) -> Arc<Histogram> {
        let histograms = self.histograms.read();
        if let Some(histogram) = histograms.get(name) {
            return histogram.clone();
        }
        drop(histograms);

        let mut histograms = self.histograms.write();
        histograms
            .entry(name.to_string())
            .or_insert_with(|| Arc::new(Histogram::new()))
            .clone()
    }

    /// Get uptime
    pub fn uptime(&self) -> Duration {
        self.start_time.elapsed()
    }

    /// Get configuration
    pub fn config(&self) -> &ObservabilityConfig {
        &self.config
    }

    /// Get all counter values
    pub fn get_counters(&self) -> HashMap<String, u64> {
        self.counters
            .read()
            .iter()
            .map(|(k, v)| (k.clone(), v.get()))
            .collect()
    }

    /// Get all gauge values
    pub fn get_gauges(&self) -> HashMap<String, u64> {
        self.gauges
            .read()
            .iter()
            .map(|(k, v)| (k.clone(), v.get()))
            .collect()
    }

    /// Reset all metrics
    pub fn reset(&self) {
        for counter in self.counters.read().values() {
            counter.reset();
        }
    }

    /// Export metrics as text (Prometheus format)
    pub fn export_text(&self) -> String {
        let mut output = String::new();

        // Counters
        for (name, counter) in self.counters.read().iter() {
            output.push_str(&format!(
                "# TYPE {} counter\n{} {}\n",
                name,
                name,
                counter.get()
            ));
        }

        // Gauges
        for (name, gauge) in self.gauges.read().iter() {
            output.push_str(&format!(
                "# TYPE {} gauge\n{} {}\n",
                name,
                name,
                gauge.get()
            ));
        }

        // Histograms
        for (name, histogram) in self.histograms.read().iter() {
            output.push_str(&format!("# TYPE {} histogram\n", name));
            for bucket in histogram.buckets() {
                output.push_str(&format!(
                    "{}_bucket{{le=\"{}\"}} {}\n",
                    name, bucket.le, bucket.count
                ));
            }
            output.push_str(&format!("{}_sum {}\n", name, histogram.sum()));
            output.push_str(&format!("{}_count {}\n", name, histogram.count()));
        }

        output
    }
}

impl Default for MetricsCollector {
    fn default() -> Self {
        Self::default_config()
    }
}

// =============================================================================
// Tests
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_counter() {
        let counter = Counter::new();
        assert_eq!(counter.get(), 0);

        counter.inc();
        assert_eq!(counter.get(), 1);

        counter.add(10);
        assert_eq!(counter.get(), 11);

        counter.reset();
        assert_eq!(counter.get(), 0);
    }

    #[test]
    fn test_gauge() {
        let gauge = Gauge::new();
        assert_eq!(gauge.get(), 0);

        gauge.set(100);
        assert_eq!(gauge.get(), 100);

        gauge.inc();
        assert_eq!(gauge.get(), 101);

        gauge.dec();
        assert_eq!(gauge.get(), 100);
    }

    #[test]
    fn test_histogram() {
        let histogram = Histogram::new();

        histogram.observe(0.05);
        histogram.observe(0.15);
        histogram.observe(0.50);

        assert_eq!(histogram.count(), 3);
        assert!(histogram.average() > 0.0);

        let buckets = histogram.buckets();
        assert!(!buckets.is_empty());
    }

    #[test]
    fn test_histogram_duration() {
        let histogram = Histogram::latency();

        histogram.observe_duration(Duration::from_millis(5));
        histogram.observe_duration(Duration::from_millis(10));

        assert_eq!(histogram.count(), 2);
    }

    #[test]
    fn test_metrics_collector() {
        let collector = MetricsCollector::default_config();

        let requests = collector.counter("http_requests_total");
        requests.inc();
        requests.inc();

        let active = collector.gauge("active_connections");
        active.set(42);

        let latency = collector.histogram("request_latency_seconds");
        latency.observe(0.05);

        let counters = collector.get_counters();
        assert_eq!(counters.get("http_requests_total"), Some(&2));

        let gauges = collector.get_gauges();
        assert_eq!(gauges.get("active_connections"), Some(&42));
    }

    #[test]
    fn test_metrics_export() {
        let collector = MetricsCollector::default_config();

        collector.counter("requests").add(100);
        collector.gauge("connections").set(10);

        let output = collector.export_text();
        assert!(output.contains("requests 100"));
        assert!(output.contains("connections 10"));
    }

    #[test]
    fn test_uptime() {
        let collector = MetricsCollector::default_config();
        std::thread::sleep(Duration::from_millis(10));
        assert!(collector.uptime() >= Duration::from_millis(10));
    }

    #[test]
    fn test_same_counter_returned() {
        let collector = MetricsCollector::default_config();

        let c1 = collector.counter("test");
        let c2 = collector.counter("test");

        c1.inc();
        assert_eq!(c2.get(), 1);
    }
}
