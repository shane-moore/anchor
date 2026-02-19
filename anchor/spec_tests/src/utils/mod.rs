use std::{str::FromStr, sync::LazyLock};

use bls::PublicKeyBytes;
use ssz::DecodeError;

pub mod deserializers;
pub mod encoding_helpers;
pub mod error_codes;

pub use encoding_helpers::{check_roundtrip, check_roundtrip_with_root, decode_base64};

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
