//! EC Integration Tests
//!
//! End-to-end tests for erasure coding functionality.

use couchestor::ec::encoder::{EcDecoder, EcEncoder};

// =============================================================================
// Encoder/Decoder Integration Tests
// =============================================================================

#[test]
fn test_encode_decode_full_pipeline() {
    // Create encoder and decoder with same config
    let data_shards = 4;
    let parity_shards = 2;

    let encoder = EcEncoder::new(data_shards, parity_shards).expect("Failed to create encoder");
    let decoder = EcDecoder::new(data_shards, parity_shards).expect("Failed to create decoder");

    // Test data
    let original_data = b"This is test data for the full EC pipeline integration test. It should be long enough to span multiple shards.";
    let original_len = original_data.len();

    // Encode
    let shards = encoder.encode(original_data).expect("Failed to encode");
    assert_eq!(shards.len(), data_shards + parity_shards);

    // Verify parity
    assert!(encoder.verify(&shards).expect("Verify failed"));

    // Decode without any losses
    let mut shards_with_options: Vec<Option<Vec<u8>>> = shards.into_iter().map(Some).collect();
    let recovered = decoder
        .decode(&mut shards_with_options, original_len)
        .expect("Failed to decode");

    assert_eq!(recovered, original_data);
}

#[test]
fn test_encode_decode_with_single_shard_loss() {
    let encoder = EcEncoder::new(4, 2).expect("Failed to create encoder");
    let decoder = EcDecoder::new(4, 2).expect("Failed to create decoder");

    let original_data = b"Data for single shard loss recovery test.";
    let original_len = original_data.len();

    // Encode
    let shards = encoder.encode(original_data).expect("Failed to encode");

    // Simulate single shard loss
    let mut shards_with_loss: Vec<Option<Vec<u8>>> = shards.into_iter().map(Some).collect();
    shards_with_loss[2] = None; // Lose one data shard

    // Should still recover
    let recovered = decoder
        .decode(&mut shards_with_loss, original_len)
        .expect("Failed to decode with loss");

    assert_eq!(recovered, original_data);
}

#[test]
fn test_encode_decode_with_max_shard_loss() {
    let encoder = EcEncoder::new(4, 2).expect("Failed to create encoder");
    let decoder = EcDecoder::new(4, 2).expect("Failed to create decoder");

    let original_data = b"Data for maximum shard loss recovery test.";
    let original_len = original_data.len();

    // Encode
    let shards = encoder.encode(original_data).expect("Failed to encode");

    // Simulate maximum recoverable loss (2 shards for 4+2)
    let mut shards_with_loss: Vec<Option<Vec<u8>>> = shards.into_iter().map(Some).collect();
    shards_with_loss[0] = None; // Lose first data shard
    shards_with_loss[5] = None; // Lose last parity shard

    // Should still recover
    let recovered = decoder
        .decode(&mut shards_with_loss, original_len)
        .expect("Failed to decode with max loss");

    assert_eq!(recovered, original_data);
}

#[test]
fn test_encode_decode_beyond_max_loss_fails() {
    let encoder = EcEncoder::new(4, 2).expect("Failed to create encoder");
    let decoder = EcDecoder::new(4, 2).expect("Failed to create decoder");

    let original_data = b"Data that should not be recoverable.";
    let original_len = original_data.len();

    // Encode
    let shards = encoder.encode(original_data).expect("Failed to encode");

    // Simulate too many losses (3 shards for 4+2)
    let mut shards_with_loss: Vec<Option<Vec<u8>>> = shards.into_iter().map(Some).collect();
    shards_with_loss[0] = None;
    shards_with_loss[1] = None;
    shards_with_loss[2] = None;

    // Should fail to recover
    let result = decoder.decode(&mut shards_with_loss, original_len);
    assert!(result.is_err());
}

#[test]
fn test_different_ec_configurations() {
    // Test various EC configurations
    let configs = vec![
        (2, 1), // Minimal: 2+1
        (4, 2), // Standard: 4+2
        (6, 3), // High: 6+3
        (8, 4), // Maximum: 8+4
    ];

    let test_data = b"Test data for configuration verification across different EC schemes.";

    for (data_shards, parity_shards) in configs {
        let encoder = EcEncoder::new(data_shards, parity_shards).expect("Failed to create encoder");
        let decoder = EcDecoder::new(data_shards, parity_shards).expect("Failed to create decoder");

        // Encode
        let shards = encoder.encode(test_data).expect("Failed to encode");
        assert_eq!(shards.len(), data_shards + parity_shards);

        // Test recovery with maximum allowed losses
        let mut degraded: Vec<Option<Vec<u8>>> = shards.into_iter().map(Some).collect();

        // Remove up to parity_shards
        for i in 0..parity_shards {
            degraded[i] = None;
        }

        let recovered = decoder
            .decode(&mut degraded, test_data.len())
            .unwrap_or_else(|_| {
                panic!(
                    "Failed to recover with {}+{} config",
                    data_shards, parity_shards
                )
            });

        assert_eq!(
            recovered, test_data,
            "Data mismatch with {}+{} config",
            data_shards, parity_shards
        );
    }
}

#[test]
fn test_large_data_encoding() {
    let encoder = EcEncoder::new(4, 2).expect("Failed to create encoder");
    let decoder = EcDecoder::new(4, 2).expect("Failed to create decoder");

    // Create 1MB of test data
    let original_data: Vec<u8> = (0..1024 * 1024).map(|i| (i % 256) as u8).collect();
    let original_len = original_data.len();

    // Encode
    let shards = encoder
        .encode(&original_data)
        .expect("Failed to encode large data");
    assert_eq!(shards.len(), 6);

    // Verify all shards have the same size
    let shard_size = shards[0].len();
    for shard in &shards {
        assert_eq!(shard.len(), shard_size);
    }

    // Decode without losses
    let mut shards_with_options: Vec<Option<Vec<u8>>> = shards.into_iter().map(Some).collect();
    let recovered = decoder
        .decode(&mut shards_with_options, original_len)
        .expect("Failed to decode large data");

    assert_eq!(recovered, original_data);
}

#[test]
fn test_reconstruction_preserves_data_integrity() {
    let encoder = EcEncoder::new(4, 2).expect("Failed to create encoder");
    let decoder = EcDecoder::new(4, 2).expect("Failed to create decoder");

    // Test data with specific pattern
    let original_data: Vec<u8> = (0u8..=255u8).cycle().take(4096).collect();
    let original_len = original_data.len();

    // Encode
    let shards = encoder.encode(&original_data).expect("Failed to encode");

    // Test reconstruction with different combinations of lost shards
    let loss_patterns = vec![
        vec![0],    // Lose shard 0
        vec![3],    // Lose shard 3
        vec![5],    // Lose parity shard
        vec![0, 1], // Lose 2 data shards
        vec![0, 5], // Lose 1 data + 1 parity
        vec![4, 5], // Lose both parity shards
    ];

    for pattern in loss_patterns {
        let mut degraded: Vec<Option<Vec<u8>>> = shards.clone().into_iter().map(Some).collect();
        for idx in &pattern {
            degraded[*idx] = None;
        }

        let recovered = decoder
            .decode(&mut degraded, original_len)
            .unwrap_or_else(|_| panic!("Failed with loss pattern: {:?}", pattern));

        assert_eq!(
            recovered, original_data,
            "Data mismatch with loss pattern: {:?}",
            pattern
        );
    }
}

#[test]
fn test_encoder_decoder_consistency() {
    // Verify that multiple encode/decode cycles produce consistent results
    let encoder = EcEncoder::new(4, 2).expect("Failed to create encoder");
    let decoder = EcDecoder::new(4, 2).expect("Failed to create decoder");

    let original_data = b"Consistency test data for multiple encode/decode cycles.";

    // Encode multiple times and verify consistency
    let shards1 = encoder.encode(original_data).expect("First encode failed");
    let shards2 = encoder.encode(original_data).expect("Second encode failed");

    // Encoding should be deterministic
    assert_eq!(shards1, shards2, "Encoding should be deterministic");

    // Both should decode to same result
    let mut options1: Vec<Option<Vec<u8>>> = shards1.into_iter().map(Some).collect();
    let mut options2: Vec<Option<Vec<u8>>> = shards2.into_iter().map(Some).collect();

    let recovered1 = decoder
        .decode(&mut options1, original_data.len())
        .expect("Decode 1 failed");
    let recovered2 = decoder
        .decode(&mut options2, original_data.len())
        .expect("Decode 2 failed");

    assert_eq!(recovered1, recovered2);
    assert_eq!(recovered1.as_slice(), original_data);
}
