// These are public API re-exports - they may not be used internally yet
#![allow(unused_imports)]

//! Domain Layer
//!
//! This module contains the core domain logic following Domain-Driven Design principles.
//!
//! # Architecture
//!
//! The domain layer is organized into:
//!
//! - **Ports** (`ports.rs`) - Trait abstractions for external dependencies
//! - **Events** (`events.rs`) - Domain events for audit and decoupling
//!
//! # Usage
//!
//! ```ignore
//! use couchestor::domain::ports::{MetricsProvider, VolumeManager};
//! use couchestor::domain::events::DomainEvent;
//!
//! // Use traits for dependency injection
//! async fn migrate_volume<M, V>(
//!     metrics: &M,
//!     volumes: &V,
//!     volume_id: &VolumeId,
//! ) -> Result<()>
//! where
//!     M: MetricsProvider,
//!     V: VolumeManager,
//! {
//!     let heat_score = metrics.get_heat_score(volume_id).await?;
//!     // ...
//! }
//! ```

pub mod events;
pub mod ports;

// Re-export commonly used types
pub use events::DomainEvent;
pub use ports::{
    // Port traits
    EcCodec,
    EncodedData,
    EventPublisher,
    // Value objects
    HeatScore,
    LbaRange,
    MetricsProvider,
    ReplicaInfo,
    ReplicaState,
    ShardLocation,
    StorageTier,
    StripeId,
    StripeMetadata,
    StripeRepository,
    TierClassification,
    VolumeId,
    VolumeInfo,
    VolumeManager,
};
