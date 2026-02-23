//! Serde deserializers for Go JSON fixture format.
//! Only compiled when the `serde` feature is enabled.

use std::str::FromStr;

use base64::{Engine, engine::general_purpose::STANDARD};
use bls::PublicKeyBytes;
use serde::{Deserialize, Deserializer, de::Error};
use ssz_types::VariableList;
use typenum::Unsigned;
use types::{Checkpoint, Epoch, Hash256, Slot};

use crate::{ValidatorIndex, msgid::MessageId, partial_sig::PartialSignatureKind};

pub fn deserialize_hex_message_id<'de, D: Deserializer<'de>>(
    deserializer: D,
) -> Result<MessageId, D::Error> {
    let hex_str = String::deserialize(deserializer)?;
    let hex_str = hex_str.strip_prefix("0x").unwrap_or(&hex_str);
    let bytes =
        hex::decode(hex_str).map_err(|e| Error::custom(format!("Failed to decode hex: {e}")))?;
    MessageId::try_from(bytes.as_slice()).map_err(|_| {
        Error::custom(format!(
            "Invalid MessageId: expected 56 bytes, got {}",
            bytes.len()
        ))
    })
}

pub fn deserialize_partial_signature_kind<'de, D: Deserializer<'de>>(
    deserializer: D,
) -> Result<PartialSignatureKind, D::Error> {
    let value = u64::deserialize(deserializer)?;
    PartialSignatureKind::try_from(value)
        .map_err(|_| Error::custom(format!("Invalid PartialSignatureKind value: {value}")))
}

pub fn deserialize_slot<'de, D: Deserializer<'de>>(deserializer: D) -> Result<Slot, D::Error> {
    let slot_str = String::deserialize(deserializer)?;
    slot_str
        .parse::<u64>()
        .map(Slot::new)
        .map_err(|e| Error::custom(format!("Failed to parse slot: {e}")))
}

/// Falls back to `infinity()` for invalid BLS points (Go fixtures use synthetic bytes).
pub fn deserialize_signature<'de, D: Deserializer<'de>>(
    deserializer: D,
) -> Result<bls::Signature, D::Error> {
    let hex_str = String::deserialize(deserializer)?;
    let hex_str = hex_str.strip_prefix("0x").unwrap_or(&hex_str);
    let bytes =
        hex::decode(hex_str).map_err(|e| Error::custom(format!("Failed to decode hex: {e}")))?;
    bls::Signature::deserialize(&bytes).or_else(|_| {
        bls::Signature::infinity().map_err(|e| Error::custom(format!("Signature::infinity: {e:?}")))
    })
}

pub fn deserialize_hash256<'de, D: Deserializer<'de>>(
    deserializer: D,
) -> Result<Hash256, D::Error> {
    let hex_str = String::deserialize(deserializer)?;
    let hex_str = hex_str.strip_prefix("0x").unwrap_or(&hex_str);
    let bytes =
        hex::decode(hex_str).map_err(|e| Error::custom(format!("Failed to decode hex: {e}")))?;
    if bytes.len() != 32 {
        return Err(Error::custom(format!(
            "Expected 32 bytes for Hash256, got {}",
            bytes.len()
        )));
    }
    Ok(Hash256::from_slice(&bytes))
}

pub fn deserialize_validator_index<'de, D: Deserializer<'de>>(
    deserializer: D,
) -> Result<ValidatorIndex, D::Error> {
    let index_str = String::deserialize(deserializer)?;
    index_str
        .parse::<usize>()
        .map(ValidatorIndex)
        .map_err(|e| Error::custom(format!("Failed to parse validator index: {e}")))
}

pub fn deserialize_base64_variable_list<'de, D, N>(
    deserializer: D,
) -> Result<VariableList<u8, N>, D::Error>
where
    D: Deserializer<'de>,
    N: Unsigned,
{
    let b64_str = String::deserialize(deserializer)?;
    if b64_str.is_empty() {
        return Ok(VariableList::empty());
    }
    let bytes = STANDARD
        .decode(&b64_str)
        .map_err(|e| Error::custom(format!("Failed to decode base64: {e}")))?;
    VariableList::new(bytes).map_err(|_| Error::custom("data exceeds maximum length"))
}

/// `null` or missing -> empty list.
pub fn deserialize_optional_variable_list<'de, D, T, N>(
    deserializer: D,
) -> Result<VariableList<T, N>, D::Error>
where
    D: Deserializer<'de>,
    T: Deserialize<'de>,
    N: Unsigned,
{
    let opt = Option::<Vec<T>>::deserialize(deserializer)?;
    let vec = opt.unwrap_or_default();
    VariableList::new(vec).map_err(|_| Error::custom("list exceeds maximum length"))
}

/// `null` or missing -> empty nested list. Used for QbftMessage justification fields.
pub fn deserialize_optional_base64_nested_variable_list<'de, D, N, M>(
    deserializer: D,
) -> Result<VariableList<VariableList<u8, N>, M>, D::Error>
where
    D: Deserializer<'de>,
    N: Unsigned,
    M: Unsigned,
{
    let opt = Option::<Vec<String>>::deserialize(deserializer)?;
    let strings = opt.unwrap_or_default();
    let inner: Vec<VariableList<u8, N>> = strings
        .into_iter()
        .map(|s| {
            let bytes = STANDARD
                .decode(&s)
                .map_err(|e| Error::custom(format!("Failed to decode base64: {e}")))?;
            VariableList::new(bytes).map_err(|_| Error::custom("inner list too long"))
        })
        .collect::<Result<_, _>>()?;
    VariableList::new(inner).map_err(|_| Error::custom("outer list too long"))
}

pub fn deserialize_public_key_bytes<'de, D: Deserializer<'de>>(
    deserializer: D,
) -> Result<PublicKeyBytes, D::Error> {
    let hex_str = String::deserialize(deserializer)?;
    PublicKeyBytes::from_str(&hex_str)
        .map_err(|e| Error::custom(format!("invalid public key: {e:?}")))
}

pub fn deserialize_epoch_string<'de, D: Deserializer<'de>>(
    deserializer: D,
) -> Result<Epoch, D::Error> {
    let s = String::deserialize(deserializer)?;
    s.parse::<u64>()
        .map(Epoch::new)
        .map_err(|e| Error::custom(format!("Failed to parse epoch: {e}")))
}

pub fn deserialize_checkpoint<'de, D: Deserializer<'de>>(
    deserializer: D,
) -> Result<Checkpoint, D::Error> {
    #[derive(Deserialize)]
    struct Raw {
        #[serde(deserialize_with = "deserialize_epoch_string")]
        epoch: Epoch,
        #[serde(deserialize_with = "deserialize_hash256")]
        root: Hash256,
    }

    let raw = Raw::deserialize(deserializer)?;
    Ok(Checkpoint {
        epoch: raw.epoch,
        root: raw.root,
    })
}
