//! Property-Based Tests for Erasure Coding
//!
//! Uses proptest to systematically verify EC encoder/decoder correctness
//! across a wide range of inputs and configurations.
//!
//! # Test Properties
//!
//! 1. **Roundtrip Correctness**: encode(data) → decode(shards) = data
//! 2. **Fault Tolerance**: Can recover from up to m shard losses
//! 3. **Determinism**: Same input always produces same output
//! 4. **Shard Independence**: Any k shards can reconstruct data

#![cfg(test)]

use proptest::prelude::*;

use super::encoder::{calculate_shard_size, EcDecoder, EcEncoder};

// =============================================================================
// Property Strategies
// =============================================================================

/// Strategy for generating valid k+m configurations.
/// k: 2-8 data shards, m: 1-4 parity shards
fn ec_config_strategy() -> impl Strategy<Value = (usize, usize)> {
    (2usize..=8, 1usize..=4)
}

/// Strategy for generating small k+m configurations for faster tests.
fn small_ec_config_strategy() -> impl Strategy<Value = (usize, usize)> {
    (2usize..=4, 1usize..=2)
}

/// Strategy for generating test data of various sizes.
fn data_strategy() -> impl Strategy<Value = Vec<u8>> {
    prop::collection::vec(any::<u8>(), 1..10000)
}

/// Strategy for generating small test data for faster tests.
fn small_data_strategy() -> impl Strategy<Value = Vec<u8>> {
    prop::collection::vec(any::<u8>(), 1..1000)
}

/// Strategy for generating erasure indices (which shards to "lose").
fn erasure_strategy(total_shards: usize, max_erasures: usize) -> impl Strategy<Value = Vec<usize>> {
    let max = std::cmp::min(max_erasures, total_shards);
    prop::collection::vec(0..total_shards, 0..=max).prop_map(|mut v| {
        v.sort();
        v.dedup();
        v
    })
}

// =============================================================================
// Roundtrip Properties
// =============================================================================

proptest! {
    #![proptest_config(ProptestConfig::with_cases(100))]

    /// Property: Encoding then decoding without any losses returns the original data.
    #[test]
    fn prop_roundtrip_no_loss(
        (k, m) in small_ec_config_strategy(),
        data in small_data_strategy(),
    ) {
        let encoder = EcEncoder::new(k, m)?;
        let decoder = EcDecoder::new(k, m)?;

        // Encode data
        let shards = encoder.encode(&data)?;
        prop_assert_eq!(shards.len(), k + m);

        // Convert to Option<Vec<u8>> format
        let mut optional_shards: Vec<Option<Vec<u8>>> = shards
            .into_iter()
            .map(Some)
            .collect();

        // Decode without any losses
        let recovered = decoder.decode(&mut optional_shards, data.len())?;

        let data_len = data.len();
        prop_assert_eq!(recovered, data, "Roundtrip failed for k={}, m={}, data_len={}", k, m, data_len);
    }

    /// Property: Can recover from loss of up to m shards.
    #[test]
    fn prop_roundtrip_with_erasures(
        (k, m) in small_ec_config_strategy(),
        data in small_data_strategy(),
        erasure_count in 0usize..=2,  // Test 0, 1, or 2 erasures
    ) {
        // Limit erasures to m
        let actual_erasures = std::cmp::min(erasure_count, m);

        let encoder = EcEncoder::new(k, m)?;
        let decoder = EcDecoder::new(k, m)?;

        // Encode data
        let shards = encoder.encode(&data)?;

        // Convert to Option<Vec<u8>> format
        let mut optional_shards: Vec<Option<Vec<u8>>> = shards
            .into_iter()
            .map(Some)
            .collect();

        // Erase some shards (first `actual_erasures` shards)
        for i in 0..actual_erasures {
            optional_shards[i] = None;
        }

        // Should be able to recover
        let recovered = decoder.decode(&mut optional_shards, data.len())?;

        prop_assert_eq!(recovered, data,
            "Recovery failed for k={}, m={}, erasures={}", k, m, actual_erasures);
    }

    /// Property: Recovery works regardless of WHICH shards are lost.
    #[test]
    fn prop_any_erasure_pattern(
        (k, m) in (2usize..=4, 1usize..=2),
        data in prop::collection::vec(any::<u8>(), 100..500),
        erasure_indices in prop::collection::vec(0usize..6, 0..=2),
    ) {
        let encoder = EcEncoder::new(k, m)?;
        let decoder = EcDecoder::new(k, m)?;

        let total = k + m;

        // Deduplicate and limit erasure indices
        let mut erasures: Vec<usize> = erasure_indices
            .into_iter()
            .filter(|&i| i < total)
            .collect();
        erasures.sort();
        erasures.dedup();

        // Skip test if too many erasures
        if erasures.len() > m {
            return Ok(());
        }

        // Encode data
        let shards = encoder.encode(&data)?;

        // Convert to Option<Vec<u8>> format and apply erasures
        let mut optional_shards: Vec<Option<Vec<u8>>> = shards
            .into_iter()
            .map(Some)
            .collect();

        for &i in &erasures {
            optional_shards[i] = None;
        }

        // Should be able to recover
        let recovered = decoder.decode(&mut optional_shards, data.len())?;

        prop_assert_eq!(recovered, data,
            "Recovery failed for k={}, m={}, erasures={:?}", k, m, erasures);
    }
}

// =============================================================================
// Configuration Properties
// =============================================================================

proptest! {
    #![proptest_config(ProptestConfig::with_cases(50))]

    /// Property: Various k+m configurations work correctly.
    #[test]
    fn prop_various_configurations(
        (k, m) in ec_config_strategy(),
        data in prop::collection::vec(any::<u8>(), 100..1000),
    ) {
        let encoder = EcEncoder::new(k, m)?;
        let decoder = EcDecoder::new(k, m)?;

        // Basic roundtrip
        let shards = encoder.encode(&data)?;
        prop_assert_eq!(shards.len(), k + m);

        let mut optional_shards: Vec<Option<Vec<u8>>> = shards
            .into_iter()
            .map(Some)
            .collect();

        let recovered = decoder.decode(&mut optional_shards, data.len())?;
        prop_assert_eq!(recovered, data);
    }

    /// Property: Shard sizes are calculated correctly.
    #[test]
    fn prop_shard_size_calculation(
        k in 2usize..=8,
        data_len in 1usize..10000,
    ) {
        let expected_shard_size = data_len.div_ceil(k);
        let calculated = calculate_shard_size(data_len, k);
        prop_assert_eq!(calculated, expected_shard_size);
    }

    /// Property: All data shards together contain all original data (with padding).
    #[test]
    fn prop_data_shards_contain_all_data(
        (k, m) in small_ec_config_strategy(),
        data in small_data_strategy(),
    ) {
        let encoder = EcEncoder::new(k, m)?;
        let shards = encoder.encode(&data)?;

        // Concatenate data shards (first k shards)
        let mut concatenated: Vec<u8> = Vec::new();
        for i in 0..k {
            concatenated.extend_from_slice(&shards[i]);
        }

        // Original data should be a prefix of concatenated (may have padding)
        prop_assert!(concatenated.len() >= data.len());
        prop_assert_eq!(&concatenated[..data.len()], data.as_slice());
    }
}

// =============================================================================
// Edge Case Properties
// =============================================================================

proptest! {
    #![proptest_config(ProptestConfig::with_cases(30))]

    /// Property: Empty data after first byte still works.
    #[test]
    fn prop_small_data(
        (k, m) in small_ec_config_strategy(),
        data_len in 1usize..=10,
    ) {
        let data: Vec<u8> = (0..data_len).map(|i| i as u8).collect();

        let encoder = EcEncoder::new(k, m)?;
        let decoder = EcDecoder::new(k, m)?;

        let shards = encoder.encode(&data)?;
        let mut optional_shards: Vec<Option<Vec<u8>>> = shards
            .into_iter()
            .map(Some)
            .collect();

        let recovered = decoder.decode(&mut optional_shards, data.len())?;
        prop_assert_eq!(recovered, data);
    }

    /// Property: Data size exactly divisible by k works.
    #[test]
    fn prop_exact_division(
        (k, m) in small_ec_config_strategy(),
        multiplier in 1usize..=100,
    ) {
        let data_len = k * multiplier;
        let data: Vec<u8> = (0..data_len).map(|i| i as u8).collect();

        let encoder = EcEncoder::new(k, m)?;
        let decoder = EcDecoder::new(k, m)?;

        let shards = encoder.encode(&data)?;

        // All shards should have the same size
        let shard_size = shards[0].len();
        for shard in &shards {
            prop_assert_eq!(shard.len(), shard_size);
        }

        let mut optional_shards: Vec<Option<Vec<u8>>> = shards
            .into_iter()
            .map(Some)
            .collect();

        let recovered = decoder.decode(&mut optional_shards, data.len())?;
        prop_assert_eq!(recovered, data);
    }

    /// Property: Reconstruction with exactly k shards works.
    #[test]
    fn prop_minimum_shards_for_recovery(
        (k, m) in (2usize..=4, 2usize..=3),  // Need m >= 2 for this test
        data in prop::collection::vec(any::<u8>(), 100..500),
    ) {
        let encoder = EcEncoder::new(k, m)?;
        let decoder = EcDecoder::new(k, m)?;

        let shards = encoder.encode(&data)?;

        // Keep only the first k shards (erase all m parity shards)
        let mut optional_shards: Vec<Option<Vec<u8>>> = shards
            .into_iter()
            .enumerate()
            .map(|(i, s)| if i < k { Some(s) } else { None })
            .collect();

        // Should still recover (we have exactly k shards)
        let recovered = decoder.decode(&mut optional_shards, data.len())?;
        prop_assert_eq!(recovered, data);
    }
}

// =============================================================================
// Determinism Properties
// =============================================================================

proptest! {
    #![proptest_config(ProptestConfig::with_cases(20))]

    /// Property: Encoding is deterministic.
    #[test]
    fn prop_encoding_deterministic(
        (k, m) in small_ec_config_strategy(),
        data in small_data_strategy(),
    ) {
        let encoder = EcEncoder::new(k, m)?;

        let shards1 = encoder.encode(&data)?;
        let shards2 = encoder.encode(&data)?;

        prop_assert_eq!(shards1, shards2, "Encoding should be deterministic");
    }

    /// Property: Different encoders with same config produce same output.
    #[test]
    fn prop_encoder_consistency(
        (k, m) in small_ec_config_strategy(),
        data in small_data_strategy(),
    ) {
        let encoder1 = EcEncoder::new(k, m)?;
        let encoder2 = EcEncoder::new(k, m)?;

        let shards1 = encoder1.encode(&data)?;
        let shards2 = encoder2.encode(&data)?;

        prop_assert_eq!(shards1, shards2, "Different encoder instances should produce same output");
    }
}

// =============================================================================
// Failure Mode Properties
// =============================================================================

proptest! {
    #![proptest_config(ProptestConfig::with_cases(20))]

    /// Property: Too many erasures causes failure.
    #[test]
    fn prop_too_many_erasures_fails(
        (k, m) in (2usize..=4, 1usize..=2),
        data in prop::collection::vec(any::<u8>(), 100..500),
    ) {
        let encoder = EcEncoder::new(k, m)?;
        let decoder = EcDecoder::new(k, m)?;

        let shards = encoder.encode(&data)?;

        // Erase m+1 shards (one more than allowed)
        let mut optional_shards: Vec<Option<Vec<u8>>> = shards
            .into_iter()
            .map(Some)
            .collect();

        for i in 0..=m {
            optional_shards[i] = None;
        }

        // Should fail
        let result = decoder.decode(&mut optional_shards, data.len());
        prop_assert!(result.is_err(), "Should fail with {} erasures (m={})", m + 1, m);
    }
}
