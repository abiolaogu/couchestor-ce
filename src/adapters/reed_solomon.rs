//! Reed-Solomon Codec Adapter
//!
//! Implements the `EcCodec` port using the Reed-Solomon erasure coding library.

use async_trait::async_trait;

use crate::domain::ports::{EcCodec, EncodedData};
use crate::ec::encoder::{EcDecoder, EcEncoder};
use crate::error::Result;

/// Reed-Solomon based erasure coding adapter.
///
/// Wraps the existing `EcEncoder` and `EcDecoder` to implement the `EcCodec` port.
#[allow(dead_code)]
pub struct ReedSolomonCodecAdapter {
    encoder: EcEncoder,
    decoder: EcDecoder,
    data_shards: usize,
    parity_shards: usize,
}

impl std::fmt::Debug for ReedSolomonCodecAdapter {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ReedSolomonCodecAdapter")
            .field("data_shards", &self.data_shards)
            .field("parity_shards", &self.parity_shards)
            .finish()
    }
}

#[allow(dead_code)]
impl ReedSolomonCodecAdapter {
    /// Create a new Reed-Solomon codec adapter.
    ///
    /// # Arguments
    /// * `data_shards` - Number of data shards (k)
    /// * `parity_shards` - Number of parity shards (m)
    pub fn new(data_shards: usize, parity_shards: usize) -> Result<Self> {
        let encoder = EcEncoder::new(data_shards, parity_shards)?;
        let decoder = EcDecoder::new(data_shards, parity_shards)?;

        Ok(Self {
            encoder,
            decoder,
            data_shards,
            parity_shards,
        })
    }

    /// Create a standard 4+2 configuration (common for storage systems).
    pub fn standard_4_2() -> Result<Self> {
        Self::new(4, 2)
    }

    /// Create a high-redundancy 4+4 configuration.
    pub fn high_redundancy_4_4() -> Result<Self> {
        Self::new(4, 4)
    }

    /// Create a low-overhead 8+2 configuration.
    pub fn low_overhead_8_2() -> Result<Self> {
        Self::new(8, 2)
    }
}

#[async_trait]
impl EcCodec for ReedSolomonCodecAdapter {
    fn data_shards(&self) -> usize {
        self.data_shards
    }

    fn parity_shards(&self) -> usize {
        self.parity_shards
    }

    fn encode(&self, data: &[u8]) -> Result<EncodedData> {
        let shards = self.encoder.encode(data)?;
        let original_len = data.len();

        // Split into data and parity shards
        let data_shards: Vec<Vec<u8>> = shards[..self.data_shards].to_vec();
        let parity_shards: Vec<Vec<u8>> = shards[self.data_shards..].to_vec();

        Ok(EncodedData {
            data_shards,
            parity_shards,
            original_len,
        })
    }

    fn decode(&self, shards: &mut [Option<Vec<u8>>], original_len: usize) -> Result<Vec<u8>> {
        self.decoder.decode(shards, original_len)
    }

    fn reconstruct(&self, shards: &mut [Option<Vec<u8>>]) -> Result<()> {
        self.decoder.reconstruct(shards)
    }

    fn calculate_shard_size(&self, data_len: usize) -> usize {
        data_len.div_ceil(self.data_shards)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_adapter_creation() {
        let codec = ReedSolomonCodecAdapter::new(4, 2).unwrap();
        assert_eq!(codec.data_shards(), 4);
        assert_eq!(codec.parity_shards(), 2);
        assert_eq!(codec.total_shards(), 6);
    }

    #[test]
    fn test_standard_configurations() {
        let standard = ReedSolomonCodecAdapter::standard_4_2().unwrap();
        assert_eq!(standard.data_shards(), 4);
        assert_eq!(standard.parity_shards(), 2);

        let high_redundancy = ReedSolomonCodecAdapter::high_redundancy_4_4().unwrap();
        assert_eq!(high_redundancy.data_shards(), 4);
        assert_eq!(high_redundancy.parity_shards(), 4);

        let low_overhead = ReedSolomonCodecAdapter::low_overhead_8_2().unwrap();
        assert_eq!(low_overhead.data_shards(), 8);
        assert_eq!(low_overhead.parity_shards(), 2);
    }

    #[test]
    fn test_encode_decode_roundtrip() {
        let codec = ReedSolomonCodecAdapter::new(4, 2).unwrap();
        let original_data = b"Hello, World! This is test data for erasure coding.";

        // Encode
        let encoded = codec.encode(original_data).unwrap();
        assert_eq!(encoded.data_shards.len(), 4);
        assert_eq!(encoded.parity_shards.len(), 2);
        assert_eq!(encoded.original_len, original_data.len());

        // Decode without any losses
        let mut shards: Vec<Option<Vec<u8>>> = encoded
            .data_shards
            .into_iter()
            .chain(encoded.parity_shards.into_iter())
            .map(Some)
            .collect();

        let recovered = codec.decode(&mut shards, original_data.len()).unwrap();
        assert_eq!(recovered, original_data);
    }

    #[test]
    fn test_encode_decode_with_erasures() {
        let codec = ReedSolomonCodecAdapter::new(4, 2).unwrap();
        let original_data = b"Test data for recovery after shard loss!";

        // Encode
        let encoded = codec.encode(original_data).unwrap();

        // Create shards with some missing (simulate loss)
        let mut shards: Vec<Option<Vec<u8>>> = encoded
            .data_shards
            .into_iter()
            .chain(encoded.parity_shards.into_iter())
            .map(Some)
            .collect();

        // Lose 2 shards (should still recover with 2 parity)
        shards[0] = None;
        shards[3] = None;

        let recovered = codec.decode(&mut shards, original_data.len()).unwrap();
        assert_eq!(recovered, original_data);
    }

    #[test]
    fn test_reconstruct() {
        let codec = ReedSolomonCodecAdapter::new(4, 2).unwrap();
        let original_data = b"Data for reconstruction test!";

        // Encode
        let encoded = codec.encode(original_data).unwrap();

        // Create shards with some missing
        let mut shards: Vec<Option<Vec<u8>>> = encoded
            .data_shards
            .into_iter()
            .chain(encoded.parity_shards.into_iter())
            .map(Some)
            .collect();

        // Save original shard 1 for comparison
        let original_shard_1 = shards[1].clone().unwrap();

        // Lose shard 1
        shards[1] = None;

        // Reconstruct
        codec.reconstruct(&mut shards).unwrap();

        // Verify reconstruction
        assert!(shards[1].is_some());
        assert_eq!(shards[1].as_ref().unwrap(), &original_shard_1);
    }

    #[test]
    fn test_can_recover() {
        let codec = ReedSolomonCodecAdapter::new(4, 2).unwrap();

        assert!(codec.can_recover(0));
        assert!(codec.can_recover(1));
        assert!(codec.can_recover(2));
        assert!(!codec.can_recover(3));
    }

    #[test]
    fn test_calculate_shard_size() {
        let codec = ReedSolomonCodecAdapter::new(4, 2).unwrap();

        assert_eq!(codec.calculate_shard_size(100), 25);
        assert_eq!(codec.calculate_shard_size(101), 26);
        assert_eq!(codec.calculate_shard_size(1000), 250);
    }
}
