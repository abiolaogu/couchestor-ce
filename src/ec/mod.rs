// Allow dead code for library-style API methods not yet used by the binary
#![allow(dead_code)]

//! Erasure Coding Module
//!
//! This module provides erasure coding support for the Smart Storage Operator,
//! enabling storage-efficient cold tier storage.
//!
//! # Architecture
//!
//! ```text
//! ┌─────────────────────────────────────────────────────────────────────────┐
//! │                        Erasure Coding Module                             │
//! ├─────────────────────────────────────────────────────────────────────────┤
//! │                                                                          │
//! │  ┌──────────────┐    ┌──────────────┐    ┌───────────────────────────┐  │
//! │  │   Encoder    │    │   Metadata   │    │   Stripe Manager          │  │
//! │  │   /Decoder   │    │   Manager    │    │   (Journal Destaging)     │  │
//! │  └──────────────┘    └──────────────┘    └───────────────────────────┘  │
//! │         │                   │                         │                  │
//! │         └───────────────────┼─────────────────────────┘                  │
//! │                             │                                            │
//! │                   ┌─────────┴─────────┐                                  │
//! │                   │   Reconstruction  │                                  │
//! │                   │      Engine       │                                  │
//! │                   └───────────────────┘                                  │
//! │                                                                          │
//! └─────────────────────────────────────────────────────────────────────────┘
//! ```
//!
//! # Components
//!
//! - **Encoder/Decoder** (`encoder.rs`): Reed-Solomon encoding and decoding
//!   using the `reed-solomon-erasure` crate. Provides functions to:
//!   - Encode data into k data shards + m parity shards
//!   - Reconstruct missing shards from survivors
//!   - Verify stripe integrity
//!
//! - **Metadata Manager** (`metadata.rs`): Manages EC metadata including:
//!   - LBA-to-stripe mappings for fast lookup
//!   - ECStripe CRD persistence to Kubernetes
//!   - Volume EC state tracking
//!
//! - **Stripe Manager** (`stripe_manager.rs`): Handles background destaging:
//!   - Monitors journal fill level
//!   - Batches writes into full stripes
//!   - Encodes and distributes shards to pools
//!
//! - **Reconstruction Engine** (`reconstruction.rs`): Handles degraded operations:
//!   - Degraded reads with transparent reconstruction
//!   - Background stripe rebuilds
//!   - Scrub verification for bit rot detection
//!
//! # Usage
//!
//! ```rust,ignore
//! use smart_storage_operator::ec::{
//!     EcEncoder, EcDecoder, EcMetadataManager, StripeManager, ReconstructionEngine
//! };
//!
//! // Create encoder/decoder for 4+2 configuration
//! let encoder = EcEncoder::new(4, 2)?;
//! let decoder = EcDecoder::new(4, 2)?;
//!
//! // Encode data
//! let data = b"Hello, World!";
//! let shards = encoder.encode(data)?;
//!
//! // Simulate loss and decode
//! let mut optional_shards: Vec<Option<Vec<u8>>> = shards.into_iter().map(Some).collect();
//! optional_shards[0] = None; // Lose first shard
//! optional_shards[3] = None; // Lose fourth shard
//!
//! let recovered = decoder.decode(&mut optional_shards, data.len())?;
//! assert_eq!(recovered, data);
//! ```

pub mod encoder;
pub mod metadata;
pub mod reconstruction;
pub mod stripe_manager;

#[cfg(test)]
mod proptest;

// Re-export types used by main.rs
pub use metadata::EcMetadataManager;
pub use reconstruction::{ReconstructionConfig, ReconstructionEngine};
pub use stripe_manager::{StripeManager, StripeManagerConfig};
