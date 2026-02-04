//! Infrastructure Adapters
//!
//! This module contains adapter implementations for the domain ports,
//! following the Port/Adapter (Hexagonal) architecture pattern.
//!
//! # Architecture
//!
//! ```text
//! ┌─────────────────────────────────────────────────────────────────┐
//! │                        Domain Layer                              │
//! │  ┌────────────────────────────────────────────────────────────┐ │
//! │  │                    Ports (Traits)                           │ │
//! │  │  MetricsProvider │ VolumeManager │ EcCodec │ StripeRepo   │ │
//! │  └────────────────────────────────────────────────────────────┘ │
//! └─────────────────────────────────────────────────────────────────┘
//!                               │
//!                               ▼
//! ┌─────────────────────────────────────────────────────────────────┐
//! │                     Adapters (This Module)                       │
//! │  ┌────────────────────────────────────────────────────────────┐ │
//! │  │ PrometheusAdapter │ MayastorAdapter │ ReedSolomonAdapter  │ │
//! │  │ KubernetesStripeRepository │ LoggingEventPublisher        │ │
//! │  └────────────────────────────────────────────────────────────┘ │
//! └─────────────────────────────────────────────────────────────────┘
//! ```
//!
//! # Usage
//!
//! ```ignore
//! use couchestor::adapters::{
//!     PrometheusMetricsAdapter,
//!     MayastorVolumeAdapter,
//!     ReedSolomonCodecAdapter,
//!     KubernetesStripeRepository,
//! };
//! use couchestor::domain::ports::MetricsProvider;
//!
//! // Create an adapter
//! let metrics = PrometheusMetricsAdapter::new(watcher);
//!
//! // Use it through the domain port trait
//! let heat_score = metrics.get_heat_score(&volume_id).await?;
//! ```

mod kubernetes;
mod mayastor;
mod prometheus;
mod reed_solomon;

#[allow(unused_imports)]
pub use kubernetes::KubernetesStripeRepository;
#[allow(unused_imports)]
pub use mayastor::MayastorVolumeAdapter;
#[allow(unused_imports)]
pub use prometheus::PrometheusMetricsAdapter;
#[allow(unused_imports)]
pub use reed_solomon::ReedSolomonCodecAdapter;

// Re-export event publishers for convenience
mod event_publisher;
#[allow(unused_imports)]
pub use event_publisher::{CompositeEventPublisher, InMemoryEventCollector, LoggingEventPublisher};
