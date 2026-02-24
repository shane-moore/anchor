use std::str::FromStr;

use serde::Deserialize;
use ssv_types::consensus::{BEACON_ROLE_PROPOSER, BeaconRole};
use ssz::{Decode, Encode};
use types::{BlindedBeaconBlock, ForkName, MainnetEthSpec};

use crate::{
    SpecTest,
    utils::{deserializers::deserialize_base64_or_empty, error_codes, is_bls_validation_error},
};

#[derive(Debug, Deserialize)]
#[serde(rename_all = "PascalCase")]
struct TestDuty {
    #[serde(rename = "Type")]
    duty_type: u64,
}

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

/// Mirrors Go's `ProposerConsensusDataTest.Run()` → `ConsensusData.Validate()`.
///
/// Validated locally because Anchor's production validator needs `SlashingDatabase`, etc.
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
    /// Follows Go's validation order: duty type check, then block decode.
    fn validate(&self) -> Result<(), i64> {
        // BeaconRole has a private inner field; construct via SSZ roundtrip.
        let duty_role =
            BeaconRole::from_ssz_bytes(&self.consensus_data.duty.duty_type.as_ssz_bytes())
                .map_err(|_| error_codes::UNKNOWN_DUTY_ROLE_DATA)?;
        if duty_role != BEACON_ROLE_PROPOSER {
            return Err(error_codes::UNKNOWN_DUTY_ROLE_DATA);
        }

        let fork_name = ForkName::from_str(&self.consensus_data.version)
            .map_err(|_| error_codes::UNKNOWN_BLOCK_VERSION)?;

        let data_ssz = &self.consensus_data.data_ssz;

        // Blinded then regular (same order as Go).
        let blinded_result =
            BlindedBeaconBlock::<MainnetEthSpec>::from_ssz_bytes_for_fork(data_ssz, fork_name);

        if blinded_result.is_ok() {
            return Ok(());
        }

        let regular_result =
            eth2::types::FullBlockContents::<MainnetEthSpec>::from_ssz_bytes_for_fork(
                data_ssz, fork_name,
            );

        if regular_result.is_ok() {
            return Ok(());
        }

        // Synthetic BLS in fixtures: Anchor validates, Go doesn't.
        let blinded_err = blinded_result.unwrap_err();
        let regular_err = regular_result.unwrap_err();

        if is_bls_validation_error(&blinded_err) || is_bls_validation_error(&regular_err) {
            return Ok(());
        }

        Err(error_codes::UNMARSHAL_SSZ)
    }
}
