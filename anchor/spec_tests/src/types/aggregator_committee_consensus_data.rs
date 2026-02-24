use serde::Deserialize;
use ssv_types::consensus::{
    AggregatorCommitteeConsensusData, AggregatorCommitteeDataValidator,
    AggregatorCommitteeValidationError,
};
use types::MainnetEthSpec;

use crate::{SpecTest, utils::error_codes};

/// Mirrors Go's `AggregatorCommitteeConsensusDataTest.Run()` -> deserializes structured JSON,
/// calls `Validate()`, asserts error codes.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub struct AggregatorCommitteeConsensusDataTest {
    agg_comm_consensus_data: AggregatorCommitteeConsensusData<MainnetEthSpec>,
    expected_error_code: i64,
}

impl SpecTest for AggregatorCommitteeConsensusDataTest {
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

impl AggregatorCommitteeConsensusDataTest {
    fn validate(&self) -> Result<(), i64> {
        let validator = AggregatorCommitteeDataValidator::<MainnetEthSpec>::new();
        validator
            .do_validation(&self.agg_comm_consensus_data)
            .map_err(|e| match e {
                AggregatorCommitteeValidationError::CommitteeIndexCountMismatch { .. } => {
                    error_codes::AGG_COMM_IDX_CNT_MISMATCH
                }
                AggregatorCommitteeValidationError::AggregatorCommitteeIndexMissing(_) => {
                    error_codes::AGG_COMM_IDX_MISMATCH
                }
                AggregatorCommitteeValidationError::AggregatorCommitteeUnusedIndex => {
                    error_codes::AGG_COMM_UNUSED_IDX
                }
                AggregatorCommitteeValidationError::DuplicateCommitteeIndex(_) => {
                    error_codes::AGG_COMM_DUPLICATED_IDX
                }
                AggregatorCommitteeValidationError::ContributorSubcommitteeMissing(_) => {
                    error_codes::AGG_COMM_SUBNET_MISSING
                }
                AggregatorCommitteeValidationError::DuplicateSyncSubcommittee(_) => {
                    error_codes::AGG_COMM_SUBNET_DUPLICATE
                }
                AggregatorCommitteeValidationError::SyncSubcommitteeUnusedIndex => {
                    error_codes::AGG_COMM_UNUSED_SUBNET
                }
                AggregatorCommitteeValidationError::NoValidatorsAssigned => {
                    error_codes::AGG_COMM_NO_VALIDATOR
                }
                AggregatorCommitteeValidationError::AttestationDecodeError(_) => {
                    error_codes::AGG_COMM_ATTESTATION_DECODE
                }
            })
    }
}
