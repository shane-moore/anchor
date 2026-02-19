use serde::Deserialize;
use ssz::{Decode, DecodeError, Encode};
use tree_hash::TreeHash;
use types::{Hash256, MainnetEthSpec};

use crate::{
    SpecTest,
    utils::{
        deserializers::{deserialize_base64, deserialize_base64_or_null, deserialize_hex_hash256},
        is_bls_validation_error,
    },
};

/// Go error codes from ssv-spec `types/error.go` relevant to
/// `ProposerSpecTest.Run()` via `GetBlockData()`.
mod error_codes {
    pub const NO_ERROR: i64 = 0;
    /// `UnmarshalSSZErrorCode` -- SSZ decode failure for both blinded and regular block.
    pub const UNMARSHAL_SSZ: i64 = 1;
    /// `UnknownBlockVersionErrorCode` -- unrecognized fork version in `DataVersion`.
    pub const UNKNOWN_BLOCK_VERSION: i64 = 11;
}

/// Top-level test fixture for `consensusdataproposer.ProposerSpecTest`.
///
/// Mirrors Go's `ProposerSpecTest.Run()` which:
/// 1. SSZ-decodes `ProposerConsensusData` from `DataCd`
/// 2. Calls `GetBlockData()` to extract the block (blinded or regular)
/// 3. On error: asserts error code matches, returns early
/// 4. On success: verifies blinded state, block root, cd root, and SSZ roundtrips
#[derive(Debug, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub struct ConsensusDataProposerTest {
    #[serde(deserialize_with = "deserialize_base64")]
    data_cd: Vec<u8>,

    #[serde(
        rename = "DataBlk",
        deserialize_with = "deserialize_base64_or_null",
        default
    )]
    data_blk: Option<Vec<u8>>,

    blinded: bool,

    #[serde(deserialize_with = "deserialize_hex_hash256")]
    expected_blk_root: Hash256,

    #[serde(deserialize_with = "deserialize_hex_hash256")]
    expected_cd_root: Hash256,

    expected_error_code: i64,
}

/// Result of successfully extracting block data from `ProposerConsensusData`.
enum BlockExtractionResult {
    /// Block was decoded successfully with full data available.
    Decoded(BlockData),
    /// Block is structurally valid SSZ but contains synthetic BLS points that
    /// Lighthouse rejects. Go's fastssz does not validate BLS during deserialization,
    /// so the Go spec test accepts these. We determine the blinded state from which
    /// decoder had the BLS error, but cannot extract roots or re-encode.
    BlsValidationOnly { is_blinded: bool },
}

/// Full block data extracted from a successfully decoded block.
struct BlockData {
    /// Whether the block is blinded.
    is_blinded: bool,
    /// The tree hash root of the extracted block (blinded or regular beacon block).
    block_root: Hash256,
    /// The re-encoded SSZ bytes of the extracted block.
    block_ssz: Vec<u8>,
}

impl SpecTest for ConsensusDataProposerTest {
    fn run(&self) -> Result<(), String> {
        // Step 1: SSZ-decode ProposerConsensusData from DataCd.
        //
        // In Go, `cd.Decode(test.DataCd)` always succeeds because Go treats DataVersion
        // as a raw uint64. In Rust, `DataVersion::from_ssz_bytes` rejects unknown values
        // with `DecodeError::NoMatchingVariant`. We detect this and map it to the
        // appropriate error code.
        let cd = match ssv_types::consensus::ProposerConsensusData::from_ssz_bytes(&self.data_cd) {
            Ok(cd) => cd,
            Err(DecodeError::NoMatchingVariant) => {
                // Unknown DataVersion -- maps to UnknownBlockVersionErrorCode (11).
                return self.assert_error_code(error_codes::UNKNOWN_BLOCK_VERSION);
            }
            Err(e) => {
                return Err(format!(
                    "Unexpected SSZ decode failure for ProposerConsensusData: {e:?}"
                ));
            }
        };

        // Step 2: Extract block data (equivalent to Go's `cd.GetBlockData()`).
        let extraction = match self.get_block_data(&cd) {
            Ok(result) => result,
            Err(code) => return self.assert_error_code(code),
        };

        // Step 3: If we got here, the test expects success.
        if self.expected_error_code != error_codes::NO_ERROR {
            return Err(format!(
                "Expected error code {}, but block extraction succeeded",
                self.expected_error_code,
            ));
        }

        match extraction {
            BlockExtractionResult::Decoded(block_data) => {
                // Full verification: blinded state, block root, block SSZ roundtrip.
                self.verify_block_data(&block_data)?;
            }
            BlockExtractionResult::BlsValidationOnly { is_blinded } => {
                // BLS divergence: Lighthouse validates BLS points during SSZ decode,
                // Go's fastssz does not. The block is structurally valid but we cannot
                // decode it in Rust. Verify only the blinded state (determined from
                // which decoder had the BLS error).
                if is_blinded != self.blinded {
                    return Err(format!(
                        "Block blinded state mismatch (BLS path): got {is_blinded}, expected {}",
                        self.blinded,
                    ));
                }
                // Skip block root and block SSZ verification -- Lighthouse cannot
                // decode blocks with synthetic BLS points.
            }
        }

        // Step 7: Verify cd tree hash root matches expected.
        let cd_root = cd.tree_hash_root();
        if cd_root != self.expected_cd_root {
            return Err(format!(
                "ConsensusData root mismatch: got {cd_root}, expected {}",
                self.expected_cd_root,
            ));
        }

        // Step 8: Verify cd SSZ roundtrip.
        let re_encoded = cd.as_ssz_bytes();
        if re_encoded != self.data_cd {
            return Err(format!(
                "ConsensusData SSZ roundtrip mismatch: re-encoded {} bytes, original {} bytes",
                re_encoded.len(),
                self.data_cd.len(),
            ));
        }

        Ok(())
    }
}

impl ConsensusDataProposerTest {
    /// Verifies block data for the fully decoded (non-BLS-error) path.
    fn verify_block_data(&self, block_data: &BlockData) -> Result<(), String> {
        // Verify blinded state.
        if block_data.is_blinded != self.blinded {
            return Err(format!(
                "Block blinded state mismatch: got {}, expected {}",
                block_data.is_blinded, self.blinded,
            ));
        }

        // Verify block root.
        if block_data.block_root != self.expected_blk_root {
            return Err(format!(
                "Block root mismatch: got {}, expected {}",
                block_data.block_root, self.expected_blk_root,
            ));
        }

        // Verify re-encoded block SSZ matches DataBlk.
        if let Some(ref expected_blk) = self.data_blk {
            if block_data.block_ssz != *expected_blk {
                return Err(format!(
                    "Block SSZ roundtrip mismatch: got {} bytes, expected {} bytes",
                    block_data.block_ssz.len(),
                    expected_blk.len(),
                ));
            }
        } else {
            return Err("Expected DataBlk for success case, but got null".to_string());
        }

        Ok(())
    }

    /// Extracts block data from `ProposerConsensusData` using production decode methods.
    ///
    /// Calls `cd.get_blinded_block_data()` then `cd.get_block_data()` — the same production
    /// methods used by `validate_block_proposal()` and post-consensus decode. Mirrors Go's
    /// test calling `cd.GetBlindedBlockData()` / `cd.GetBlockData()` directly.
    fn get_block_data(
        &self,
        cd: &ssv_types::consensus::ProposerConsensusData,
    ) -> Result<BlockExtractionResult, i64> {
        // Try blinded block first (same order as Go).
        match cd.get_blinded_block_data::<MainnetEthSpec>() {
            Ok(blinded) => {
                return Ok(BlockExtractionResult::Decoded(BlockData {
                    is_blinded: true,
                    block_root: blinded.tree_hash_root(),
                    block_ssz: blinded.as_ssz_bytes(),
                }));
            }
            Err(blinded_err) => {
                // Then try regular block contents.
                match cd.get_block_data::<MainnetEthSpec>() {
                    Ok(full) => {
                        // Go returns the inner BeaconBlock (not the full contents with blobs).
                        let block = full.block();
                        return Ok(BlockExtractionResult::Decoded(BlockData {
                            is_blinded: false,
                            block_root: block.tree_hash_root(),
                            block_ssz: block.as_ssz_bytes(),
                        }));
                    }
                    Err(full_err) => {
                        // Both failed. Check for BLS validation errors.
                        //
                        // Go's fastssz does not validate BLS points during deserialization,
                        // while Lighthouse does. Synthetic BLS signatures in fixtures are
                        // structurally valid SSZ but fail point-on-curve checks.
                        let blinded_is_bls = is_bls_validation_error(&blinded_err);
                        let regular_is_bls = is_bls_validation_error(&full_err);

                        if blinded_is_bls || regular_is_bls {
                            return Ok(BlockExtractionResult::BlsValidationOnly {
                                is_blinded: blinded_is_bls,
                            });
                        }

                        return Err(error_codes::UNMARSHAL_SSZ);
                    }
                }
            }
        }
    }

    /// Asserts the actual error code matches the expected one.
    fn assert_error_code(&self, actual_code: i64) -> Result<(), String> {
        if actual_code != self.expected_error_code {
            Err(format!(
                "Expected error code {}, got {actual_code}",
                self.expected_error_code,
            ))
        } else {
            Ok(())
        }
    }
}
