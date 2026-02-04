//! Erasure Coding Encoder/Decoder
//!
//! Implements Reed-Solomon encoding and decoding using the `reed-solomon-erasure` crate.
//! Provides functions to encode data shards into parity shards and reconstruct
//! missing shards from survivors.

use crate::error::{Error, Result};
use reed_solomon_erasure::galois_8::ReedSolomon;
use std::sync::Arc;
use tracing::{debug, instrument};

// =============================================================================
// EC Encoder
// =============================================================================

/// Erasure coding encoder for creating parity shards from data shards
pub struct EcEncoder {
    /// Reed-Solomon codec instance
    rs: Arc<ReedSolomon>,
    /// Number of data shards (k)
    data_shards: usize,
    /// Number of parity shards (m)
    parity_shards: usize,
}

impl EcEncoder {
    /// Create a new encoder with the specified k+m configuration
    ///
    /// # Arguments
    /// * `data_shards` - Number of data shards (k)
    /// * `parity_shards` - Number of parity shards (m)
    ///
    /// # Returns
    /// Result containing the encoder or an error if configuration is invalid
    pub fn new(data_shards: usize, parity_shards: usize) -> Result<Self> {
        if data_shards == 0 {
            return Err(Error::InvalidEcConfig(
                "data_shards must be greater than 0".to_string(),
            ));
        }
        if parity_shards == 0 {
            return Err(Error::InvalidEcConfig(
                "parity_shards must be greater than 0".to_string(),
            ));
        }

        let rs = ReedSolomon::new(data_shards, parity_shards).map_err(|e| {
            Error::InvalidEcConfig(format!("Failed to create Reed-Solomon codec: {}", e))
        })?;

        Ok(Self {
            rs: Arc::new(rs),
            data_shards,
            parity_shards,
        })
    }

    /// Get the number of data shards
    pub fn data_shards(&self) -> usize {
        self.data_shards
    }

    /// Get the number of parity shards
    pub fn parity_shards(&self) -> usize {
        self.parity_shards
    }

    /// Get the total number of shards
    pub fn total_shards(&self) -> usize {
        self.data_shards + self.parity_shards
    }

    /// Encode data into shards (data + parity)
    ///
    /// Takes raw data and splits it into data shards, then computes parity shards.
    /// The input data will be padded if necessary to make it evenly divisible.
    ///
    /// # Arguments
    /// * `data` - Raw data to encode
    ///
    /// # Returns
    /// Vector of shards (data shards followed by parity shards)
    #[instrument(skip(self, data), fields(data_len = data.len()))]
    pub fn encode(&self, data: &[u8]) -> Result<Vec<Vec<u8>>> {
        // Calculate shard size (round up to handle uneven division)
        let shard_size = data.len().div_ceil(self.data_shards);

        // Create data shards with padding if needed
        let mut shards: Vec<Vec<u8>> = Vec::with_capacity(self.total_shards());

        for i in 0..self.data_shards {
            let start = i * shard_size;
            let end = std::cmp::min(start + shard_size, data.len());

            let mut shard = if start < data.len() {
                data[start..end].to_vec()
            } else {
                Vec::new()
            };

            // Pad shard to shard_size
            shard.resize(shard_size, 0);
            shards.push(shard);
        }

        // Create empty parity shards
        for _ in 0..self.parity_shards {
            shards.push(vec![0u8; shard_size]);
        }

        // Compute parity shards
        self.rs
            .encode(&mut shards)
            .map_err(|e| Error::EcEncodingFailed(format!("Reed-Solomon encoding failed: {}", e)))?;

        debug!(
            "Encoded {} bytes into {} shards of {} bytes each",
            data.len(),
            self.total_shards(),
            shard_size
        );

        Ok(shards)
    }

    /// Encode data shards that are already split
    ///
    /// Takes pre-split data shards and computes parity shards in-place.
    ///
    /// # Arguments
    /// * `shards` - Mutable slice containing data shards followed by empty parity shards
    #[instrument(skip(self, shards))]
    pub fn encode_shards(&self, shards: &mut [Vec<u8>]) -> Result<()> {
        if shards.len() != self.total_shards() {
            return Err(Error::InvalidEcConfig(format!(
                "Expected {} shards, got {}",
                self.total_shards(),
                shards.len()
            )));
        }

        self.rs
            .encode(shards)
            .map_err(|e| Error::EcEncodingFailed(format!("Reed-Solomon encoding failed: {}", e)))?;

        Ok(())
    }

    /// Verify that the parity shards are consistent with data shards
    ///
    /// # Arguments
    /// * `shards` - All shards (data + parity)
    ///
    /// # Returns
    /// true if parity is correct, false otherwise
    pub fn verify(&self, shards: &[Vec<u8>]) -> Result<bool> {
        if shards.len() != self.total_shards() {
            return Err(Error::InvalidEcConfig(format!(
                "Expected {} shards, got {}",
                self.total_shards(),
                shards.len()
            )));
        }

        let result = self
            .rs
            .verify(shards)
            .map_err(|e| Error::EcEncodingFailed(format!("Verification failed: {}", e)))?;

        Ok(result)
    }
}

// =============================================================================
// EC Decoder
// =============================================================================

/// Erasure coding decoder for reconstructing missing shards
pub struct EcDecoder {
    /// Reed-Solomon codec instance
    rs: Arc<ReedSolomon>,
    /// Number of data shards (k)
    data_shards: usize,
    /// Number of parity shards (m)
    parity_shards: usize,
}

impl EcDecoder {
    /// Create a new decoder with the specified k+m configuration
    pub fn new(data_shards: usize, parity_shards: usize) -> Result<Self> {
        if data_shards == 0 {
            return Err(Error::InvalidEcConfig(
                "data_shards must be greater than 0".to_string(),
            ));
        }
        if parity_shards == 0 {
            return Err(Error::InvalidEcConfig(
                "parity_shards must be greater than 0".to_string(),
            ));
        }

        let rs = ReedSolomon::new(data_shards, parity_shards).map_err(|e| {
            Error::InvalidEcConfig(format!("Failed to create Reed-Solomon codec: {}", e))
        })?;

        Ok(Self {
            rs: Arc::new(rs),
            data_shards,
            parity_shards,
        })
    }

    /// Get the number of data shards
    pub fn data_shards(&self) -> usize {
        self.data_shards
    }

    /// Get the number of parity shards
    pub fn parity_shards(&self) -> usize {
        self.parity_shards
    }

    /// Get the total number of shards
    pub fn total_shards(&self) -> usize {
        self.data_shards + self.parity_shards
    }

    /// Reconstruct missing shards
    ///
    /// Takes a vector of optional shards and reconstructs any missing ones.
    /// At least k shards must be present for reconstruction to succeed.
    ///
    /// # Arguments
    /// * `shards` - Vector of optional shards (None for missing shards)
    ///
    /// # Returns
    /// Vector with all shards reconstructed
    #[instrument(skip(self, shards))]
    pub fn reconstruct(&self, shards: &mut [Option<Vec<u8>>]) -> Result<()> {
        if shards.len() != self.total_shards() {
            return Err(Error::InvalidEcConfig(format!(
                "Expected {} shards, got {}",
                self.total_shards(),
                shards.len()
            )));
        }

        // Count available shards
        let available = shards.iter().filter(|s| s.is_some()).count();
        if available < self.data_shards {
            return Err(Error::InsufficientShards {
                available,
                required: self.data_shards,
            });
        }

        // Use reed-solomon-erasure's reconstruct with Option<Vec<u8>> directly
        // The library will allocate and fill in missing shards (None entries)
        self.rs
            .reconstruct(shards)
            .map_err(|e| Error::EcReconstructionFailed {
                stripe_id: 0,
                reason: format!("Reed-Solomon reconstruction failed: {}", e),
            })?;

        debug!(
            "Reconstructed shards from {}/{} available",
            available,
            self.total_shards()
        );

        Ok(())
    }

    /// Reconstruct only the data shards (skip parity reconstruction)
    ///
    /// This is faster when you only need the original data and don't care
    /// about restoring parity shards.
    #[instrument(skip(self, shards))]
    pub fn reconstruct_data(&self, shards: &mut [Option<Vec<u8>>]) -> Result<()> {
        if shards.len() != self.total_shards() {
            return Err(Error::InvalidEcConfig(format!(
                "Expected {} shards, got {}",
                self.total_shards(),
                shards.len()
            )));
        }

        // Count available shards
        let available = shards.iter().filter(|s| s.is_some()).count();
        if available < self.data_shards {
            return Err(Error::InsufficientShards {
                available,
                required: self.data_shards,
            });
        }

        // Reconstruct data shards using reed-solomon-erasure with Option<Vec<u8>> directly
        // The library will allocate and fill in missing shards (None entries)
        self.rs
            .reconstruct_data(shards)
            .map_err(|e| Error::EcReconstructionFailed {
                stripe_id: 0,
                reason: format!("Reed-Solomon data reconstruction failed: {}", e),
            })?;

        debug!(
            "Reconstructed data shards from {}/{} available",
            available,
            self.total_shards()
        );

        Ok(())
    }

    /// Decode shards back to original data
    ///
    /// Reconstructs missing shards if necessary, then combines data shards
    /// back into the original data.
    ///
    /// # Arguments
    /// * `shards` - Vector of optional shards
    /// * `original_size` - Size of the original data (to trim padding)
    ///
    /// # Returns
    /// The reconstructed original data
    #[instrument(skip(self, shards), fields(original_size))]
    pub fn decode(&self, shards: &mut [Option<Vec<u8>>], original_size: usize) -> Result<Vec<u8>> {
        // First reconstruct any missing data shards
        self.reconstruct_data(shards)?;

        // Combine data shards
        let mut data = Vec::with_capacity(original_size);
        for s in shards.iter().take(self.data_shards).flatten() {
            data.extend_from_slice(s);
        }

        // Trim to original size
        data.truncate(original_size);

        Ok(data)
    }
}

// =============================================================================
// Convenience Functions
// =============================================================================

/// Create an encoder/decoder pair with the same configuration
pub fn create_codec(data_shards: usize, parity_shards: usize) -> Result<(EcEncoder, EcDecoder)> {
    let encoder = EcEncoder::new(data_shards, parity_shards)?;
    let decoder = EcDecoder::new(data_shards, parity_shards)?;
    Ok((encoder, decoder))
}

/// Calculate the shard size for given data size and shard count
pub fn calculate_shard_size(data_size: usize, data_shards: usize) -> usize {
    data_size.div_ceil(data_shards)
}

/// Calculate storage overhead ratio (total/data)
pub fn storage_overhead(data_shards: usize, parity_shards: usize) -> f64 {
    (data_shards + parity_shards) as f64 / data_shards as f64
}

/// Calculate storage efficiency ratio (data/total)
pub fn storage_efficiency(data_shards: usize, parity_shards: usize) -> f64 {
    data_shards as f64 / (data_shards + parity_shards) as f64
}

// =============================================================================
// Tests
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    // =========================================================================
    // Encoder Tests
    // =========================================================================

    #[test]
    fn test_encoder_new() {
        let encoder = EcEncoder::new(4, 2).unwrap();
        assert_eq!(encoder.data_shards(), 4);
        assert_eq!(encoder.parity_shards(), 2);
        assert_eq!(encoder.total_shards(), 6);
    }

    #[test]
    fn test_encoder_invalid_config() {
        assert!(EcEncoder::new(0, 2).is_err());
        assert!(EcEncoder::new(4, 0).is_err());
    }

    #[test]
    fn test_encode_basic() {
        let encoder = EcEncoder::new(4, 2).unwrap();
        let data = b"Hello, World! This is a test of erasure coding.";

        let shards = encoder.encode(data).unwrap();

        assert_eq!(shards.len(), 6); // 4 data + 2 parity

        // All shards should be the same size
        let shard_size = shards[0].len();
        for shard in &shards {
            assert_eq!(shard.len(), shard_size);
        }
    }

    #[test]
    fn test_encode_verify() {
        let encoder = EcEncoder::new(4, 2).unwrap();
        let data = b"Test data for verification";

        let shards = encoder.encode(data).unwrap();
        assert!(encoder.verify(&shards).unwrap());
    }

    #[test]
    fn test_encode_small_data() {
        let encoder = EcEncoder::new(4, 2).unwrap();
        let data = b"Hi";

        let shards = encoder.encode(data).unwrap();
        assert_eq!(shards.len(), 6);
    }

    // =========================================================================
    // Decoder Tests
    // =========================================================================

    #[test]
    fn test_decoder_new() {
        let decoder = EcDecoder::new(4, 2).unwrap();
        assert_eq!(decoder.data_shards(), 4);
        assert_eq!(decoder.parity_shards(), 2);
    }

    #[test]
    fn test_roundtrip_no_loss() {
        let encoder = EcEncoder::new(4, 2).unwrap();
        let decoder = EcDecoder::new(4, 2).unwrap();

        let original = b"This is test data for erasure coding roundtrip!";
        let original_len = original.len();

        // Encode
        let shards = encoder.encode(original).unwrap();

        // Convert to Option<Vec<u8>>
        let mut optional_shards: Vec<Option<Vec<u8>>> = shards.into_iter().map(Some).collect();

        // Decode
        let recovered = decoder.decode(&mut optional_shards, original_len).unwrap();

        assert_eq!(recovered, original);
    }

    #[test]
    fn test_roundtrip_with_loss() {
        let encoder = EcEncoder::new(4, 2).unwrap();
        let decoder = EcDecoder::new(4, 2).unwrap();

        let original = b"Testing recovery from shard loss with erasure coding!";
        let original_len = original.len();

        // Encode
        let shards = encoder.encode(original).unwrap();

        // Simulate loss of 2 shards (within parity tolerance)
        let mut optional_shards: Vec<Option<Vec<u8>>> = shards.into_iter().map(Some).collect();
        optional_shards[1] = None; // Lose data shard 1
        optional_shards[4] = None; // Lose parity shard 0

        // Decode should still work
        let recovered = decoder.decode(&mut optional_shards, original_len).unwrap();

        assert_eq!(recovered, original);
    }

    #[test]
    fn test_reconstruct_insufficient_shards() {
        let decoder = EcDecoder::new(4, 2).unwrap();

        // Only 3 shards available (need 4)
        let mut shards: Vec<Option<Vec<u8>>> = vec![
            Some(vec![0u8; 16]),
            Some(vec![0u8; 16]),
            Some(vec![0u8; 16]),
            None,
            None,
            None,
        ];

        let result = decoder.reconstruct(&mut shards);
        assert!(matches!(result, Err(Error::InsufficientShards { .. })));
    }

    // =========================================================================
    // Utility Function Tests
    // =========================================================================

    #[test]
    fn test_calculate_shard_size() {
        assert_eq!(calculate_shard_size(100, 4), 25);
        assert_eq!(calculate_shard_size(101, 4), 26);
        assert_eq!(calculate_shard_size(1000, 10), 100);
    }

    #[test]
    fn test_storage_overhead() {
        assert!((storage_overhead(4, 2) - 1.5).abs() < 0.001);
        assert!((storage_overhead(6, 3) - 1.5).abs() < 0.001);
        assert!((storage_overhead(10, 2) - 1.2).abs() < 0.001);
    }

    #[test]
    fn test_storage_efficiency() {
        assert!((storage_efficiency(4, 2) - 0.666).abs() < 0.01);
        assert!((storage_efficiency(6, 3) - 0.666).abs() < 0.01);
        assert!((storage_efficiency(10, 2) - 0.833).abs() < 0.01);
    }

    #[test]
    fn test_create_codec() {
        let (encoder, decoder) = create_codec(4, 2).unwrap();
        assert_eq!(encoder.total_shards(), decoder.total_shards());
    }

    // =========================================================================
    // Edge Case Tests
    // =========================================================================

    #[test]
    fn test_encode_empty_data() {
        let encoder = EcEncoder::new(4, 2).unwrap();
        let data: &[u8] = &[];

        // Empty data returns an error (reed-solomon-erasure doesn't support zero-length shards)
        let result = encoder.encode(data);
        assert!(result.is_err());
    }

    #[test]
    fn test_various_configurations() {
        // Test various k+m configurations
        let configs = vec![
            (2, 1),  // Simple
            (4, 2),  // Common
            (6, 3),  // Higher
            (8, 4),  // Large
            (10, 4), // Asymmetric
        ];

        for (k, m) in configs {
            let encoder = EcEncoder::new(k, m).unwrap();
            let decoder = EcDecoder::new(k, m).unwrap();

            let data = b"Test data for various configurations";
            let shards = encoder.encode(data).unwrap();

            assert_eq!(shards.len(), k + m);
            assert!(encoder.verify(&shards).unwrap());

            // Test reconstruction with loss up to m shards
            let mut optional: Vec<Option<Vec<u8>>> = shards.into_iter().map(Some).collect();

            // Remove m shards
            for i in 0..m {
                optional[i] = None;
            }

            let recovered = decoder.decode(&mut optional, data.len()).unwrap();
            assert_eq!(recovered, data);
        }
    }
}
