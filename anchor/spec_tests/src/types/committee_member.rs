use std::collections::HashSet;

use serde::Deserialize;
use ssv_types::{
    OperatorId,
    message::{SSVMessage, SignedSSVMessage, SignedSSVMessageError},
};

use crate::{SpecTest, utils::deserializers::deserialize_base64_list, utils::error_codes};

/// Intermediate struct for the signed message from the fixture.
///
/// Reuses the same pattern as `TestSignedSSVMessage` in `signed_ssv_msg.rs`.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "PascalCase")]
struct TestSignedSSVMessage {
    #[serde(deserialize_with = "deserialize_base64_list")]
    signatures: Vec<Vec<u8>>,
    #[serde(rename = "OperatorIDs")]
    operator_ids: Vec<OperatorId>,
    #[serde(rename = "SSVMessage")]
    ssv_message: Option<SSVMessage>,
    #[serde(
        rename = "FullData",
        deserialize_with = "crate::utils::deserializers::deserialize_base64_or_null",
        default
    )]
    full_data: Option<Vec<u8>>,
}

/// Committee member fields — only `FaultyNodes` and `Committee.len()` are used.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "PascalCase")]
struct TestCommitteeMember {
    faulty_nodes: u64,
    committee: Vec<serde_json::Value>,
}

/// Mirrors Go's `CommitteeMemberTest`.
///
/// Validates a `SignedSSVMessage`, then checks quorum and full-committee thresholds
/// using deduplicated signer count.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub struct CommitteeMemberTest {
    committee_member: TestCommitteeMember,
    message: TestSignedSSVMessage,
    expected_has_quorum: bool,
    expected_full_committee: bool,
    expected_error_code: i64,
}

/// Map a `SignedSSVMessageError` to Go integer error code (reusing `error_codes` from
/// `signed_ssv_msg`).
fn error_code_for(err: &SignedSSVMessageError) -> i64 {
    match err {
        SignedSSVMessageError::NoSigners => error_codes::NO_SIGNERS,
        SignedSSVMessageError::NoSignatures => error_codes::NO_SIGNATURES,
        SignedSSVMessageError::ZeroSigner => error_codes::ZERO_SIGNER_NOT_ALLOWED,
        SignedSSVMessageError::DuplicatedSigner => error_codes::NON_UNIQUE_SIGNER,
        SignedSSVMessageError::SignersAndSignaturesWithDifferentLength => {
            error_codes::INCORRECT_NUMBER_OF_SIGNATURES
        }
        SignedSSVMessageError::WrongRSASignatureSize { .. } => error_codes::EMPTY_SIGNATURE,
        _ => error_codes::UNMAPPED_ERROR_CODE,
    }
}

impl SpecTest for CommitteeMemberTest {
    fn run(&self) -> Result<(), String> {
        let msg = &self.message;

        // Validate message using same pre-check + new() pattern as SignedSSVMessageTest
        let actual_code = match self.validate_message(msg) {
            Ok(_) => error_codes::NO_ERROR,
            Err(err) => error_code_for(&err),
        };

        if actual_code != self.expected_error_code {
            return Err(format!(
                "expected error code {}, got {actual_code}",
                self.expected_error_code,
            ));
        }

        // Quorum and full-committee checks use deduplicated signer count
        let unique_signers: HashSet<&OperatorId> = msg.operator_ids.iter().collect();
        let unique_count = unique_signers.len() as u64;
        let quorum_threshold = 2 * self.committee_member.faulty_nodes + 1;
        let committee_size = self.committee_member.committee.len() as u64;

        let has_quorum = unique_count >= quorum_threshold;
        if has_quorum != self.expected_has_quorum {
            return Err(format!(
                "has_quorum: expected {}, got {} (unique={unique_count}, threshold={quorum_threshold})",
                self.expected_has_quorum, has_quorum
            ));
        }

        let full_committee = unique_count == committee_size;
        if full_committee != self.expected_full_committee {
            return Err(format!(
                "full_committee: expected {}, got {} (unique={unique_count}, committee={committee_size})",
                self.expected_full_committee, full_committee,
            ));
        }

        Ok(())
    }
}

impl CommitteeMemberTest {
    /// Validate the signed message, returning the error if validation fails.
    fn validate_message(
        &self,
        msg: &TestSignedSSVMessage,
    ) -> Result<SignedSSVMessage, SignedSSVMessageError> {
        // Pre-check: empty signatures
        for sig in &msg.signatures {
            if sig.is_empty() {
                return Err(SignedSSVMessageError::WrongRSASignatureSize {
                    index: 0,
                    length: 0,
                    sig_length: 256,
                });
            }
        }

        // Pre-check: null SSVMessage
        let ssv_message = msg
            .ssv_message
            .as_ref()
            .ok_or(SignedSSVMessageError::NoSigners)?;

        // Pad signatures to [u8; 256]
        let signatures: Vec<[u8; 256]> = msg
            .signatures
            .iter()
            .map(|sig| {
                let mut arr = [0u8; 256];
                let len = sig.len().min(256);
                arr[..len].copy_from_slice(&sig[..len]);
                arr
            })
            .collect();

        SignedSSVMessage::new(
            signatures,
            msg.operator_ids.clone(),
            ssv_message.clone(),
            msg.full_data.clone().unwrap_or_default(),
        )
    }
}
