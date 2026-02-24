//! Go error codes from ssv-spec `types/error.go`.
//!
//! Go defines these as a single `iota + 1` enumeration. Each test module uses the
//! subset relevant to its validation, but the codes themselves are global constants.
//! Centralizing them here avoids duplication across test files.

/// No error — validation succeeded.
pub const NO_ERROR: i64 = 0;

/// `UnmarshalSSZErrorCode` — SSZ decode failure.
pub const UNMARSHAL_SSZ: i64 = 1;

/// `NonUniqueSignerErrorCode` — duplicate operator ID in signer list.
pub const NON_UNIQUE_SIGNER: i64 = 9;

/// `UnknownDutyRoleDataErrorCode` — duty type is not the expected role.
pub const UNKNOWN_DUTY_ROLE_DATA: i64 = 10;

/// `UnknownBlockVersionErrorCode` — unrecognized fork version.
pub const UNKNOWN_BLOCK_VERSION: i64 = 11;

/// `IncorrectNumberOfSignaturesErrorCode` — signers/signatures length mismatch.
pub const INCORRECT_NUMBER_OF_SIGNATURES: i64 = 12;

/// `EmptySignatureErrorCode` — signature is empty or wrong size.
pub const EMPTY_SIGNATURE: i64 = 13;

/// `NilSSVMessageErrorCode` — SSVMessage pointer is null.
pub const NIL_SSV_MESSAGE: i64 = 14;

/// `NoSignaturesErrorCode` — no signatures provided.
pub const NO_SIGNATURES: i64 = 15;

/// `NoSignersErrorCode` — no signers provided.
pub const NO_SIGNERS: i64 = 16;

/// `ZeroSignerNotAllowedErrorCode` — signer ID 0 is invalid.
pub const ZERO_SIGNER_NOT_ALLOWED: i64 = 17;

/// `InconsistentSignersErrorCode` — signers within a partial signature batch differ.
pub const INCONSISTENT_SIGNERS: i64 = 18;

/// `NoPartialSigMessagesErrorCode` — empty partial signature messages list.
pub const NO_PARTIAL_SIG_MESSAGES: i64 = 19;

/// `SSVMessageHasInvalidSignatureErrorCode` — RSA signature verification failed.
pub const SSV_MESSAGE_HAS_INVALID_SIGNATURE: i64 = 46;

/// `AggCommAggCommIdxCntMismatchErrorCode` — committee indexes count != attestations count.
pub const AGG_COMM_IDX_CNT_MISMATCH: i64 = 71;

/// `AggCommCommIdxMismatchErrorCode` — aggregator's committee index not in allowed list.
pub const AGG_COMM_IDX_MISMATCH: i64 = 72;

/// `AggCommUnusedCommIdxErrorCode` — committee index not used by any aggregator.
pub const AGG_COMM_UNUSED_IDX: i64 = 73;

/// `AggCommDuplicatedCommIdxErrorCode` — duplicate committee index in list.
pub const AGG_COMM_DUPLICATED_IDX: i64 = 74;

/// `AggCommSubnetNotInSCSubnetsErrorCode` — contributor subnet not in contributions list.
pub const AGG_COMM_SUBNET_MISSING: i64 = 75;

/// `AggCommSCCSubnetDuplicateErrorCode` — duplicate sync committee subnet index.
pub const AGG_COMM_SUBNET_DUPLICATE: i64 = 76;

/// `AggCommUnusedSubnetErrorCode` — sync subnet not used by any contributor.
pub const AGG_COMM_UNUSED_SUBNET: i64 = 77;

/// `AggCommConsensusDataNoValidatorErrorCode` — no aggregators or contributors assigned.
pub const AGG_COMM_NO_VALIDATOR: i64 = 78;

/// `AggCommAttestationDecodingErrorCode` — attestation SSZ decode failure.
pub const AGG_COMM_ATTESTATION_DECODE: i64 = 84;

/// Sentinel for Anchor-specific errors without Go equivalents.
/// If a fixture hits this, the test will fail with a clear mismatch.
pub const UNMAPPED_ERROR_CODE: i64 = -1;
