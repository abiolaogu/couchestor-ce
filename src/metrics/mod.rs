//! Metrics module
//!
//! Provides volume metrics collection from Prometheus.

mod watcher;

#[allow(unused_imports)]
pub use watcher::{CacheStats, HeatScore, MetricsConfig, MetricsWatcher};
