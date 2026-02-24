use serde::Deserialize;
use ssz::{Decode, DecodeError, Encode};
use tree_hash::TreeHash;
use types::{Hash256, MainnetEthSpec};

use crate::{
    SpecTest,
    utils::{
        deserializers::{deserialize_base64, deserialize_base64_or_null, deserialize_hex_hash256},
        error_codes, is_bls_validation_error,
    },
};

/// Mirrors Go's `ProposerSpecTest.Run()`.
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

enum BlockExtractionResult {
    Decoded(BlockData),
    /// BLS rejected synthetic points; can only determine blinded state.
    BlsValidationOnly {
        is_blinded: bool,
    },
}

struct BlockData {
    is_blinded: bool,
    block_root: Hash256,
    block_ssz: Vec<u8>,
}

impl SpecTest for ConsensusDataProposerTest {
    fn run(&self) -> Result<(), String> {
        // Rust rejects unknown `DataVersion` variants; Go treats as raw `uint64`.
        let cd = match ssv_types::consensus::ProposerConsensusData::from_ssz_bytes(&self.data_cd) {
            Ok(cd) => cd,
            Err(DecodeError::NoMatchingVariant) => {
                return self.assert_error_code(error_codes::UNKNOWN_BLOCK_VERSION);
            }
            Err(e) => {
                return Err(format!("Unexpected SSZ decode failure: {e:?}"));
            }
        };

        let extraction = match self.get_block_data(&cd) {
            Ok(result) => result,
            Err(code) => return self.assert_error_code(code),
        };

        if self.expected_error_code != error_codes::NO_ERROR {
            return Err(format!(
                "Expected error code {}, but block extraction succeeded",
                self.expected_error_code,
            ));
        }

        match extraction {
            BlockExtractionResult::Decoded(block_data) => {
                self.verify_block_data(&block_data)?;
            }
            BlockExtractionResult::BlsValidationOnly { is_blinded } => {
                if is_blinded != self.blinded {
                    return Err(format!(
                        "Block blinded state mismatch (BLS path): got {is_blinded}, expected {}",
                        self.blinded,
                    ));
                }
            }
        }

        let cd_root = cd.tree_hash_root();
        if cd_root != self.expected_cd_root {
            return Err(format!(
                "ConsensusData root mismatch: got {cd_root}, expected {}",
                self.expected_cd_root,
            ));
        }

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
    fn verify_block_data(&self, block_data: &BlockData) -> Result<(), String> {
        if block_data.is_blinded != self.blinded {
            return Err(format!(
                "Block blinded state mismatch: got {}, expected {}",
                block_data.is_blinded, self.blinded,
            ));
        }

        if block_data.block_root != self.expected_blk_root {
            return Err(format!(
                "Block root mismatch: got {}, expected {}",
                block_data.block_root, self.expected_blk_root,
            ));
        }

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

    /// Tries blinded then full decode (same order as Go).
    fn get_block_data(
        &self,
        cd: &ssv_types::consensus::ProposerConsensusData,
    ) -> Result<BlockExtractionResult, i64> {
        match cd.get_blinded_block_data::<MainnetEthSpec>() {
            Ok(blinded) => Ok(BlockExtractionResult::Decoded(BlockData {
                is_blinded: true,
                block_root: blinded.tree_hash_root(),
                block_ssz: blinded.as_ssz_bytes(),
            })),
            Err(blinded_err) => match cd.get_block_data::<MainnetEthSpec>() {
                Ok(full) => {
                    // Go returns `BeaconBlock`, not `FullBlockContents`.
                    let block = full.block();
                    Ok(BlockExtractionResult::Decoded(BlockData {
                        is_blinded: false,
                        block_root: block.tree_hash_root(),
                        block_ssz: block.as_ssz_bytes(),
                    }))
                }
                Err(full_err) => {
                    let blinded_is_bls = is_bls_validation_error(&blinded_err);
                    let regular_is_bls = is_bls_validation_error(&full_err);

                    if blinded_is_bls || regular_is_bls {
                        Ok(BlockExtractionResult::BlsValidationOnly {
                            is_blinded: blinded_is_bls,
                        })
                    } else {
                        Err(error_codes::UNMARSHAL_SSZ)
                    }
                }
            },
        }
    }

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
