use std::{str::FromStr, sync::LazyLock};

use bls::PublicKeyBytes;
use serde::Deserialize;
use ssv_types::{OperatorId, message::SSVMessage};
use ssz::DecodeError;

pub mod deserializers;
pub mod encoding_helpers;
pub mod error_codes;

pub use encoding_helpers::{check_roundtrip, check_roundtrip_with_root, decode_base64};

/// Shared intermediate struct for deserializing Go fixture `SignedSSVMessage` JSON.
///
/// `ssv_message` is `Option` because error fixtures can have null messages.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub struct TestSignedSSVMessage {
    #[serde(deserialize_with = "deserializers::deserialize_base64_list")]
    pub signatures: Vec<Vec<u8>>,
    #[serde(rename = "OperatorIDs")]
    pub operator_ids: Vec<OperatorId>,
    #[serde(rename = "SSVMessage")]
    pub ssv_message: Option<SSVMessage>,
    #[serde(
        rename = "FullData",
        deserialize_with = "deserializers::deserialize_base64_or_null",
        default
    )]
    pub full_data: Option<Vec<u8>>,
}

/// Pad a variable-length signature to a fixed `[u8; 256]` array (RSA signature size).
/// Go fixtures can have shorter or empty signatures; Anchor requires exactly 256 bytes.
pub fn pad_signature_256(sig: &[u8]) -> [u8; 256] {
    let mut arr = [0u8; 256];
    let len = sig.len().min(256);
    arr[..len].copy_from_slice(&sig[..len]);
    arr
}

/// Returns `true` if the SSZ decode error is caused by BLS point validation failure.
///
/// Lighthouse validates BLS public keys and signatures during SSZ deserialization
/// (point-on-curve check), while Go's `fastssz` treats them as opaque byte arrays.
/// This function detects BLS-specific failures so we can accept SSZ data that is
/// structurally valid but contains synthetic BLS values (as in the Go spec fixtures).
pub fn is_bls_validation_error(err: &DecodeError) -> bool {
    matches!(err, DecodeError::BytesInvalid(msg) if msg.contains("BLST"))
}

pub static TESTING_VALIDATOR_PUBKEY: LazyLock<PublicKeyBytes> = LazyLock::new(|| {
    PublicKeyBytes::from_str(
        "0x8e80066551a81b318258709edaf7dd1f63cd686a0e4db8b29bbb7acfe65608677af5a527d9448ee47835485e02b50bc0",
    )
    .expect("Failed to create public key")
});
