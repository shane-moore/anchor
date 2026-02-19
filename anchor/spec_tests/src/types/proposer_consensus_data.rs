use std::str::FromStr;

use serde::Deserialize;
use ssv_types::consensus::{BeaconRole, BEACON_ROLE_PROPOSER};
use ssz::{Decode, DecodeError, Encode};
use types::{BlindedBeaconBlock, ForkName, MainnetEthSpec};

use crate::SpecTest;

/// Go error codes from ssv-spec `types/error.go` (iota + 1) relevant to
/// `ProposerConsensusData.Validate()`.
mod error_codes {
    pub const NO_ERROR: i64 = 0;
    /// `UnmarshalSSZErrorCode` — SSZ decode failure for both blinded and regular block.
    pub const UNMARSHAL_SSZ: i64 = 1;
    /// `UnknownDutyRoleDataErrorCode` — duty type is not `BNRoleProposer`.
    pub const UNKNOWN_DUTY_ROLE_DATA: i64 = 10;
    /// `UnknownBlockVersionErrorCode` — unrecognized fork version.
    pub const UNKNOWN_BLOCK_VERSION: i64 = 11;
}

// ==================== Deserialization structs ====================

/// Intermediate struct for deserializing `ValidatorDuty` from JSON.
///
/// Needed because Go serializes `Slot` and `ValidatorIndex` as strings and
/// `Type` as a raw integer, while `ValidatorSyncCommitteeIndices` can be null.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "PascalCase")]
struct TestDuty {
    #[serde(rename = "Type")]
    duty_type: u64,
    #[serde(rename = "PubKey")]
    #[expect(dead_code)]
    pub_key: String,
    #[expect(dead_code)]
    slot: String,
    #[expect(dead_code)]
    validator_index: String,
    #[expect(dead_code)]
    committee_index: u64,
    #[expect(dead_code)]
    committee_length: u64,
    #[expect(dead_code)]
    committees_at_slot: u64,
    #[expect(dead_code)]
    validator_committee_index: u64,
    #[expect(dead_code)]
    validator_sync_committee_indices: Option<Vec<u64>>,
}

/// Intermediate struct for deserializing `ProposerConsensusData` from JSON.
///
/// `Version` is a string like "capella", and `DataSSZ` is base64-encoded.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "PascalCase")]
struct TestConsensusData {
    duty: TestDuty,
    version: String,
    #[serde(
        rename = "DataSSZ",
        deserialize_with = "deserialize_base64_or_empty",
        default
    )]
    data_ssz: Vec<u8>,
}

/// Top-level test fixture for `ProposerConsensusDataTest`.
///
/// Mirrors Go's `ProposerConsensusDataTest.Run()` which calls
/// `ConsensusData.Validate()` and asserts the resulting error code.
///
/// Validation is implemented locally because Anchor's `ProposerConsensusDataValidator`
/// requires a `SlashingDatabase`, `ChainSpec`, and other production dependencies.
/// The local validation mirrors Go's `Validate()` logic:
/// 1. Check duty type == BEACON_ROLE_PROPOSER
/// 2. Try SSZ decode of data_ssz as blinded block, then regular block
#[derive(Debug, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub struct ProposerConsensusDataTest {
    consensus_data: TestConsensusData,
    expected_error_code: i64,
}

impl SpecTest for ProposerConsensusDataTest {
    fn run(&self) -> Result<(), String> {
        let actual_code = match self.validate() {
            Ok(()) => error_codes::NO_ERROR,
            Err(code) => code,
        };

        if actual_code != self.expected_error_code {
            return Err(format!(
                "Expected error code {}, got {actual_code}",
                self.expected_error_code,
            ));
        }

        Ok(())
    }
}

impl ProposerConsensusDataTest {
    /// Local validation mirroring Go's `ProposerConsensusData.Validate()`.
    ///
    /// Go's validation order:
    /// 1. Check `Duty.Type == BNRoleProposer` -> `UnknownDutyRoleDataErrorCode` (10)
    /// 2. Call `GetBlockData()` which:
    ///    a. For known versions: try blinded then regular block -> `UnmarshalSSZErrorCode` (1)
    ///    b. For unknown versions: -> `UnknownBlockVersionErrorCode` (11)
    fn validate(&self) -> Result<(), i64> {
        // Step 1: Check duty type.
        // `BeaconRole` has a private inner field, so we construct it via SSZ roundtrip.
        let duty_role = BeaconRole::from_ssz_bytes(
            &self.consensus_data.duty.duty_type.as_ssz_bytes(),
        )
        .map_err(|_| error_codes::UNKNOWN_DUTY_ROLE_DATA)?;
        if duty_role != BEACON_ROLE_PROPOSER {
            return Err(error_codes::UNKNOWN_DUTY_ROLE_DATA);
        }

        // Step 2: Parse the fork version and try decoding the block data.
        // This exercises the same Lighthouse code paths as Anchor's production
        // `validate_block_proposal()` in `consensus.rs`.
        let fork_name = ForkName::from_str(&self.consensus_data.version)
            .map_err(|_| error_codes::UNKNOWN_BLOCK_VERSION)?;

        let data_ssz = &self.consensus_data.data_ssz;

        // Try blinded block first (same order as Go and Anchor production code).
        // This exercises the same `BlindedBeaconBlock::from_ssz_bytes_for_fork` and
        // `FullBlockContents::from_ssz_bytes_for_fork` used by Anchor's production
        // `validate_block_proposal()` in consensus.rs:372-377.
        let blinded_result =
            BlindedBeaconBlock::<MainnetEthSpec>::from_ssz_bytes_for_fork(data_ssz, fork_name);

        if blinded_result.is_ok() {
            return Ok(());
        }

        // Then try regular block contents
        let regular_result =
            eth2::types::FullBlockContents::<MainnetEthSpec>::from_ssz_bytes_for_fork(
                data_ssz, fork_name,
            );

        if regular_result.is_ok() {
            return Ok(());
        }

        // Both failed. Distinguish between structural SSZ errors and BLS validation errors.
        //
        // Go's SSZ library (fastssz) does not validate BLS points during deserialization,
        // while Lighthouse does. The Go spec fixtures contain synthetic BLS signatures
        // (e.g., 0x010203...) that are structurally valid SSZ but fail BLS point-on-curve
        // checks. When both decoders fail solely due to BLS validation, the underlying
        // SSZ structure is valid -- treat this as success to match Go's behavior.
        let blinded_err = blinded_result.unwrap_err();
        let regular_err = regular_result.unwrap_err();

        if is_bls_validation_error(&blinded_err) || is_bls_validation_error(&regular_err) {
            return Ok(());
        }

        Err(error_codes::UNMARSHAL_SSZ)
    }
}

/// Returns `true` if the SSZ decode error is caused by BLS point validation failure.
///
/// Lighthouse validates BLS public keys and signatures during SSZ deserialization
/// (point-on-curve check), while Go's `fastssz` treats them as opaque byte arrays.
/// This function detects BLS-specific failures so we can accept SSZ data that is
/// structurally valid but contains synthetic BLS values (as in the Go spec fixtures).
fn is_bls_validation_error(err: &DecodeError) -> bool {
    match err {
        DecodeError::BytesInvalid(msg) => msg.contains("BLST"),
        _ => false,
    }
}

/// Deserializes a base64-encoded string, treating empty strings as empty `Vec<u8>`.
///
/// Some error fixtures have `"DataSSZ": ""` which is valid — it represents empty data
/// that should fail SSZ decoding.
fn deserialize_base64_or_empty<'de, D: serde::Deserializer<'de>>(
    deserializer: D,
) -> Result<Vec<u8>, D::Error> {
    let s = String::deserialize(deserializer)?;
    if s.is_empty() {
        return Ok(Vec::new());
    }
    base64::Engine::decode(&base64::engine::general_purpose::STANDARD, s)
        .map_err(|e| serde::de::Error::custom(format!("Failed to decode base64: {e}")))
}
