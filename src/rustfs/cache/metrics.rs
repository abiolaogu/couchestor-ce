//! Cache Metrics Collection
//!
//! Performance metrics and statistics for monitoring cache health.

use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

/// Cache metrics collector
#[derive(Debug, Default)]
pub struct CacheMetrics {
    // L1 metrics
    l1_hits: AtomicU64,
    l1_misses: AtomicU64,
    l1_evictions: AtomicU64,
    l1_size_bytes: AtomicU64,
    l1_entries: AtomicU64,

    // L2 metrics
    l2_hits: AtomicU64,
    l2_misses: AtomicU64,
    l2_evictions: AtomicU64,
    l2_size_bytes: AtomicU64,
    l2_entries: AtomicU64,

    // L3 metrics
    l3_hits: AtomicU64,
    l3_misses: AtomicU64,

    // Operation latencies (microseconds, using exponential moving average)
    l1_read_latency_us: AtomicU64,
    l1_write_latency_us: AtomicU64,
    l2_read_latency_us: AtomicU64,
    l2_write_latency_us: AtomicU64,
    l3_read_latency_us: AtomicU64,

    // Promotion/demotion metrics
    promotions_l3_to_l2: AtomicU64,
    promotions_l2_to_l1: AtomicU64,
    demotions_l1_to_l2: AtomicU64,
    demotions_l2_to_l3: AtomicU64,

    // Throughput (bytes per second, sampled)
    read_throughput_bps: AtomicU64,
    write_throughput_bps: AtomicU64,
}

impl CacheMetrics {
    /// Create a new metrics collector
    pub fn new() -> Self {
        Self::default()
    }

    // L1 metrics
    pub fn record_l1_hit(&self) {
        self.l1_hits.fetch_add(1, Ordering::Relaxed);
    }

    pub fn record_l1_miss(&self) {
        self.l1_misses.fetch_add(1, Ordering::Relaxed);
    }

    pub fn record_l1_eviction(&self) {
        self.l1_evictions.fetch_add(1, Ordering::Relaxed);
    }

    pub fn update_l1_stats(&self, size: u64, entries: u64) {
        self.l1_size_bytes.store(size, Ordering::Relaxed);
        self.l1_entries.store(entries, Ordering::Relaxed);
    }

    pub fn l1_hits(&self) -> u64 {
        self.l1_hits.load(Ordering::Relaxed)
    }

    pub fn l1_misses(&self) -> u64 {
        self.l1_misses.load(Ordering::Relaxed)
    }

    pub fn l1_hit_ratio(&self) -> f64 {
        let hits = self.l1_hits() as f64;
        let total = hits + self.l1_misses() as f64;
        if total == 0.0 {
            0.0
        } else {
            hits / total
        }
    }

    // L2 metrics
    pub fn record_l2_hit(&self) {
        self.l2_hits.fetch_add(1, Ordering::Relaxed);
    }

    pub fn record_l2_miss(&self) {
        self.l2_misses.fetch_add(1, Ordering::Relaxed);
    }

    pub fn record_l2_eviction(&self) {
        self.l2_evictions.fetch_add(1, Ordering::Relaxed);
    }

    pub fn update_l2_stats(&self, size: u64, entries: u64) {
        self.l2_size_bytes.store(size, Ordering::Relaxed);
        self.l2_entries.store(entries, Ordering::Relaxed);
    }

    pub fn l2_hits(&self) -> u64 {
        self.l2_hits.load(Ordering::Relaxed)
    }

    pub fn l2_misses(&self) -> u64 {
        self.l2_misses.load(Ordering::Relaxed)
    }

    pub fn l2_hit_ratio(&self) -> f64 {
        let hits = self.l2_hits() as f64;
        let total = hits + self.l2_misses() as f64;
        if total == 0.0 {
            0.0
        } else {
            hits / total
        }
    }

    // L3 metrics
    pub fn record_l3_hit(&self) {
        self.l3_hits.fetch_add(1, Ordering::Relaxed);
    }

    pub fn record_l3_miss(&self) {
        self.l3_misses.fetch_add(1, Ordering::Relaxed);
    }

    pub fn l3_hits(&self) -> u64 {
        self.l3_hits.load(Ordering::Relaxed)
    }

    pub fn l3_misses(&self) -> u64 {
        self.l3_misses.load(Ordering::Relaxed)
    }

    // Latency tracking
    pub fn record_l1_read_latency(&self, duration: Duration) {
        self.update_latency_ema(&self.l1_read_latency_us, duration);
    }

    pub fn record_l1_write_latency(&self, duration: Duration) {
        self.update_latency_ema(&self.l1_write_latency_us, duration);
    }

    pub fn record_l2_read_latency(&self, duration: Duration) {
        self.update_latency_ema(&self.l2_read_latency_us, duration);
    }

    pub fn record_l2_write_latency(&self, duration: Duration) {
        self.update_latency_ema(&self.l2_write_latency_us, duration);
    }

    pub fn record_l3_read_latency(&self, duration: Duration) {
        self.update_latency_ema(&self.l3_read_latency_us, duration);
    }

    fn update_latency_ema(&self, target: &AtomicU64, duration: Duration) {
        let new_us = duration.as_micros() as u64;
        let alpha = 0.1; // EMA smoothing factor

        loop {
            let current = target.load(Ordering::Relaxed);
            let updated = if current == 0 {
                new_us
            } else {
                ((1.0 - alpha) * current as f64 + alpha * new_us as f64) as u64
            };

            if target
                .compare_exchange_weak(current, updated, Ordering::Relaxed, Ordering::Relaxed)
                .is_ok()
            {
                break;
            }
        }
    }

    pub fn l1_read_latency(&self) -> Duration {
        Duration::from_micros(self.l1_read_latency_us.load(Ordering::Relaxed))
    }

    pub fn l1_write_latency(&self) -> Duration {
        Duration::from_micros(self.l1_write_latency_us.load(Ordering::Relaxed))
    }

    pub fn l2_read_latency(&self) -> Duration {
        Duration::from_micros(self.l2_read_latency_us.load(Ordering::Relaxed))
    }

    pub fn l2_write_latency(&self) -> Duration {
        Duration::from_micros(self.l2_write_latency_us.load(Ordering::Relaxed))
    }

    pub fn l3_read_latency(&self) -> Duration {
        Duration::from_micros(self.l3_read_latency_us.load(Ordering::Relaxed))
    }

    // Promotion/demotion tracking
    pub fn record_promotion_l3_to_l2(&self) {
        self.promotions_l3_to_l2.fetch_add(1, Ordering::Relaxed);
    }

    pub fn record_promotion_l2_to_l1(&self) {
        self.promotions_l2_to_l1.fetch_add(1, Ordering::Relaxed);
    }

    pub fn record_demotion_l1_to_l2(&self) {
        self.demotions_l1_to_l2.fetch_add(1, Ordering::Relaxed);
    }

    pub fn record_demotion_l2_to_l3(&self) {
        self.demotions_l2_to_l3.fetch_add(1, Ordering::Relaxed);
    }

    // Throughput
    pub fn update_throughput(&self, read_bps: u64, write_bps: u64) {
        self.read_throughput_bps.store(read_bps, Ordering::Relaxed);
        self.write_throughput_bps
            .store(write_bps, Ordering::Relaxed);
    }

    pub fn read_throughput(&self) -> u64 {
        self.read_throughput_bps.load(Ordering::Relaxed)
    }

    pub fn write_throughput(&self) -> u64 {
        self.write_throughput_bps.load(Ordering::Relaxed)
    }

    /// Get overall cache hit ratio
    pub fn overall_hit_ratio(&self) -> f64 {
        let total_hits = self.l1_hits() + self.l2_hits() + self.l3_hits();
        let total_misses = self.l3_misses(); // Only count final misses
        let total = total_hits + total_misses;

        if total == 0 {
            0.0
        } else {
            total_hits as f64 / total as f64
        }
    }

    /// Get snapshot of all metrics
    pub fn snapshot(&self) -> MetricsSnapshot {
        MetricsSnapshot {
            l1_hits: self.l1_hits(),
            l1_misses: self.l1_misses(),
            l1_evictions: self.l1_evictions.load(Ordering::Relaxed),
            l1_size_bytes: self.l1_size_bytes.load(Ordering::Relaxed),
            l1_entries: self.l1_entries.load(Ordering::Relaxed),
            l1_hit_ratio: self.l1_hit_ratio(),
            l1_read_latency: self.l1_read_latency(),
            l1_write_latency: self.l1_write_latency(),

            l2_hits: self.l2_hits(),
            l2_misses: self.l2_misses(),
            l2_evictions: self.l2_evictions.load(Ordering::Relaxed),
            l2_size_bytes: self.l2_size_bytes.load(Ordering::Relaxed),
            l2_entries: self.l2_entries.load(Ordering::Relaxed),
            l2_hit_ratio: self.l2_hit_ratio(),
            l2_read_latency: self.l2_read_latency(),
            l2_write_latency: self.l2_write_latency(),

            l3_hits: self.l3_hits(),
            l3_misses: self.l3_misses(),
            l3_read_latency: self.l3_read_latency(),

            promotions_l3_to_l2: self.promotions_l3_to_l2.load(Ordering::Relaxed),
            promotions_l2_to_l1: self.promotions_l2_to_l1.load(Ordering::Relaxed),
            demotions_l1_to_l2: self.demotions_l1_to_l2.load(Ordering::Relaxed),
            demotions_l2_to_l3: self.demotions_l2_to_l3.load(Ordering::Relaxed),

            overall_hit_ratio: self.overall_hit_ratio(),
            read_throughput_bps: self.read_throughput(),
            write_throughput_bps: self.write_throughput(),
        }
    }

    /// Reset all metrics
    pub fn reset(&self) {
        self.l1_hits.store(0, Ordering::Relaxed);
        self.l1_misses.store(0, Ordering::Relaxed);
        self.l1_evictions.store(0, Ordering::Relaxed);
        self.l2_hits.store(0, Ordering::Relaxed);
        self.l2_misses.store(0, Ordering::Relaxed);
        self.l2_evictions.store(0, Ordering::Relaxed);
        self.l3_hits.store(0, Ordering::Relaxed);
        self.l3_misses.store(0, Ordering::Relaxed);
        self.promotions_l3_to_l2.store(0, Ordering::Relaxed);
        self.promotions_l2_to_l1.store(0, Ordering::Relaxed);
        self.demotions_l1_to_l2.store(0, Ordering::Relaxed);
        self.demotions_l2_to_l3.store(0, Ordering::Relaxed);
    }
}

/// Snapshot of all cache metrics
#[derive(Debug, Clone)]
pub struct MetricsSnapshot {
    // L1
    pub l1_hits: u64,
    pub l1_misses: u64,
    pub l1_evictions: u64,
    pub l1_size_bytes: u64,
    pub l1_entries: u64,
    pub l1_hit_ratio: f64,
    pub l1_read_latency: Duration,
    pub l1_write_latency: Duration,

    // L2
    pub l2_hits: u64,
    pub l2_misses: u64,
    pub l2_evictions: u64,
    pub l2_size_bytes: u64,
    pub l2_entries: u64,
    pub l2_hit_ratio: f64,
    pub l2_read_latency: Duration,
    pub l2_write_latency: Duration,

    // L3
    pub l3_hits: u64,
    pub l3_misses: u64,
    pub l3_read_latency: Duration,

    // Tier movement
    pub promotions_l3_to_l2: u64,
    pub promotions_l2_to_l1: u64,
    pub demotions_l1_to_l2: u64,
    pub demotions_l2_to_l3: u64,

    // Overall
    pub overall_hit_ratio: f64,
    pub read_throughput_bps: u64,
    pub write_throughput_bps: u64,
}

/// Latency tracker helper
pub struct LatencyTracker {
    start: Instant,
}

impl LatencyTracker {
    /// Start tracking latency
    pub fn start() -> Self {
        Self {
            start: Instant::now(),
        }
    }

    /// Get elapsed duration
    pub fn elapsed(&self) -> Duration {
        self.start.elapsed()
    }
}

// =============================================================================
// Tests
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_metrics_creation() {
        let metrics = CacheMetrics::new();
        assert_eq!(metrics.l1_hits(), 0);
        assert_eq!(metrics.l2_hits(), 0);
        assert_eq!(metrics.l3_hits(), 0);
    }

    #[test]
    fn test_hit_tracking() {
        let metrics = CacheMetrics::new();

        metrics.record_l1_hit();
        metrics.record_l1_hit();
        metrics.record_l1_miss();

        assert_eq!(metrics.l1_hits(), 2);
        assert_eq!(metrics.l1_misses(), 1);
        assert!((metrics.l1_hit_ratio() - 0.666).abs() < 0.01);
    }

    #[test]
    fn test_latency_tracking() {
        let metrics = CacheMetrics::new();

        metrics.record_l1_read_latency(Duration::from_micros(100));
        assert_eq!(metrics.l1_read_latency(), Duration::from_micros(100));

        // EMA should smooth values
        metrics.record_l1_read_latency(Duration::from_micros(200));
        let latency = metrics.l1_read_latency().as_micros();
        assert!(latency > 100 && latency < 200);
    }

    #[test]
    fn test_tier_movement_tracking() {
        let metrics = CacheMetrics::new();

        metrics.record_promotion_l3_to_l2();
        metrics.record_promotion_l2_to_l1();
        metrics.record_demotion_l1_to_l2();
        metrics.record_demotion_l2_to_l3();

        let snapshot = metrics.snapshot();
        assert_eq!(snapshot.promotions_l3_to_l2, 1);
        assert_eq!(snapshot.promotions_l2_to_l1, 1);
        assert_eq!(snapshot.demotions_l1_to_l2, 1);
        assert_eq!(snapshot.demotions_l2_to_l3, 1);
    }

    #[test]
    fn test_overall_hit_ratio() {
        let metrics = CacheMetrics::new();

        // All levels hit
        metrics.record_l1_hit();
        metrics.record_l1_hit();
        metrics.record_l2_hit();
        metrics.record_l3_hit();
        metrics.record_l3_miss();

        assert!((metrics.overall_hit_ratio() - 0.8).abs() < 0.01);
    }

    #[test]
    fn test_snapshot() {
        let metrics = CacheMetrics::new();

        metrics.record_l1_hit();
        metrics.record_l2_hit();
        metrics.update_l1_stats(1024, 10);
        metrics.update_throughput(1_000_000, 500_000);

        let snapshot = metrics.snapshot();
        assert_eq!(snapshot.l1_hits, 1);
        assert_eq!(snapshot.l2_hits, 1);
        assert_eq!(snapshot.l1_size_bytes, 1024);
        assert_eq!(snapshot.l1_entries, 10);
        assert_eq!(snapshot.read_throughput_bps, 1_000_000);
    }

    #[test]
    fn test_reset() {
        let metrics = CacheMetrics::new();

        metrics.record_l1_hit();
        metrics.record_l2_hit();
        metrics.record_l3_miss();

        metrics.reset();

        assert_eq!(metrics.l1_hits(), 0);
        assert_eq!(metrics.l2_hits(), 0);
        assert_eq!(metrics.l3_misses(), 0);
    }

    #[test]
    fn test_latency_tracker() {
        let tracker = LatencyTracker::start();
        std::thread::sleep(Duration::from_millis(10));
        let elapsed = tracker.elapsed();
        assert!(elapsed >= Duration::from_millis(10));
    }
}
