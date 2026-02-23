use bls::AggregateSignature;
use serde::{Deserialize, de};
use ssv_types::{
    OperatorId,
    consensus::{
        AggregatorCommitteeConsensusData, AssignedAggregator, BeaconVote, DataVersion,
        MaxAggregatedAttestationBytes, MaxCommitteeIndexes, ProposerConsensusData, QbftMessage,
    },
    message::{SSVMessage, SSVMessageFullDataLen, SignedSSVMessage},
    partial_sig::{PartialSignatureMessage, PartialSignatureMessages},
};
use ssz::{Decode, Encode};
use ssz_types::{
    BitVector, FixedVector, VariableList,
    typenum::{Prod, U8, U13, U128, U256, U1024},
};
use types::{AttestationData, Hash256, MainnetEthSpec, Slot, SyncCommitteeContribution};

use crate::{SpecTest, utils::pad_signature_256};

// Go fixtures use raw byte counts that exceed Rust's BitList bit-based bounds,
// so we use a `VariableList<u8, N>` with a large enough N instead.
type MaxAggregationBitsBytes = Prod<U128, U1024>;

/// Field order must match production `AttestationBase` for identical SSZ layout.
#[derive(Debug, ssz_derive::Encode)]
struct RawAttestationBase {
    aggregation_bits: VariableList<u8, MaxAggregationBitsBytes>,
    data: AttestationData,
    signature: AggregateSignature,
}

/// Field order must match production `AttestationElectra` for identical SSZ layout.
#[derive(Debug, ssz_derive::Encode)]
struct RawAttestationElectra {
    aggregation_bits: VariableList<u8, MaxAggregationBitsBytes>,
    data: AttestationData,
    signature: AggregateSignature,
    committee_bits: FixedVector<u8, U8>,
}

/// Go's `StructureSizeTest`: deserialize + SSZ-encode the `Object`, check encoded length.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub struct StructureSizeTest {
    expected_encoded_length: usize,
    #[serde(rename = "Object", deserialize_with = "deserialize_encoded_bytes")]
    encoded_bytes: Vec<u8>,
}

impl SpecTest for StructureSizeTest {
    fn run(&self) -> Result<(), String> {
        if self.encoded_bytes.len() != self.expected_encoded_length {
            return Err(format!(
                "encoded length {}, expected {}",
                self.encoded_bytes.len(),
                self.expected_encoded_length,
            ));
        }
        Ok(())
    }
}

/// JSON -> T -> SSZ bytes.
fn encode_value<T: serde::de::DeserializeOwned + Encode, E: de::Error>(
    value: serde_json::Value,
) -> Result<Vec<u8>, E> {
    let obj: T = serde_json::from_value(value).map_err(|e| de::Error::custom(format!("{e}")))?;
    Ok(obj.as_ssz_bytes())
}

/// Field-based type detection
fn deserialize_encoded_bytes<'de, D: serde::Deserializer<'de>>(
    deserializer: D,
) -> Result<Vec<u8>, D::Error> {
    let value = serde_json::Value::deserialize(deserializer)?;
    let obj = value
        .as_object()
        .ok_or_else(|| de::Error::custom("Object field is not a JSON object"))?;

    // Detection order matches Go's `UnmarshalJSON`
    if obj.contains_key("MsgType") && obj.contains_key("MsgID") {
        return encode_value::<SSVMessage, _>(value);
    }

    if obj.contains_key("Signatures") && obj.contains_key("OperatorIDs") {
        return Ok(parse_signed_ssv_message::<D::Error>(value)?.as_ssz_bytes());
    }

    if obj.contains_key("SigningRoot") && obj.contains_key("Signer") {
        return encode_value::<PartialSignatureMessage, _>(value);
    }

    if obj.contains_key("Type") && obj.contains_key("Slot") && obj.contains_key("Messages") {
        return encode_value::<PartialSignatureMessages, _>(value);
    }

    if obj.contains_key("Round") && obj.contains_key("Height") {
        return encode_value::<QbftMessage, _>(value);
    }

    if obj.contains_key("Duty") && obj.contains_key("DataSSZ") {
        return encode_value::<ProposerConsensusData, _>(value);
    }

    if obj.contains_key("BlockRoot") && obj.contains_key("Source") && obj.contains_key("Target") {
        return encode_value::<BeaconVote, _>(value);
    }

    // Attestations: check Electra first (`committee_bits` is a superset of Phase0 fields)
    if obj.contains_key("aggregation_bits") && obj.contains_key("data") {
        let electra = obj.contains_key("committee_bits");
        return parse_attestation(value, electra);
    }

    if obj.contains_key("Version") && obj.contains_key("Aggregators") {
        return parse_aggregator_consensus_data(value);
    }

    Err(de::Error::custom("unrecognized Object type"))
}

// ==================== SignedSSVMessage ====================

/// SSZ-layout mirror of `SignedSSVMessage`. Go fixtures contain invalid operator IDs and
/// data that fails production validation, so we encode via this struct and round-trip
/// through `from_ssz_bytes()` to skip validation.
#[derive(ssz_derive::Encode)]
struct RawSignedSSVMessage {
    signatures: ssv_types::message::SignatureList,
    operator_ids: VariableList<OperatorId, U13>,
    ssv_message: SSVMessage,
    full_data: VariableList<u8, SSVMessageFullDataLen>,
}

fn parse_signed_ssv_message<E: de::Error>(value: serde_json::Value) -> Result<SignedSSVMessage, E> {
    let raw: crate::utils::TestSignedSSVMessage =
        serde_json::from_value(value).map_err(|e| de::Error::custom(format!("{e}")))?;

    let ssv_message = raw
        .ssv_message
        .ok_or_else(|| de::Error::custom("null SSVMessage in StructureSizeTest fixture"))?;

    let sig_count = raw.signatures.len();

    let sig_vlists: Vec<VariableList<u8, U256>> = raw
        .signatures
        .iter()
        .map(|sig| {
            VariableList::new(pad_signature_256(sig).to_vec())
                .map_err(|_| de::Error::custom("signature too long"))
        })
        .collect::<Result<_, _>>()?;
    let signatures: ssv_types::message::SignatureList =
        VariableList::new(sig_vlists).map_err(|_| de::Error::custom("too many signatures"))?;

    // Fixture operator IDs (e.g. [1,1,1]) violate Anchor's sorted-unique-nonzero validation.
    // Substitute valid IDs with same count; OperatorId is fixed 8 bytes so length is preserved.
    let operator_ids: Vec<OperatorId> = (1..=sig_count as u64).map(OperatorId).collect();
    let operator_ids: VariableList<OperatorId, U13> =
        VariableList::new(operator_ids).map_err(|_| de::Error::custom("too many operator IDs"))?;

    let full_data = VariableList::new(raw.full_data.unwrap_or_default())
        .map_err(|_| de::Error::custom("full_data too long"))?;

    let mirror = RawSignedSSVMessage {
        signatures,
        operator_ids,
        ssv_message,
        full_data,
    };
    let ssz_bytes = mirror.as_ssz_bytes();

    SignedSSVMessage::from_ssz_bytes(&ssz_bytes)
        .map_err(|e| de::Error::custom(format!("SignedSSVMessage SSZ decode failed: {e:?}")))
}

// ==================== Attestation (Phase0 / Electra) ====================

fn decode_0x_hex<E: de::Error>(s: &str) -> Result<Vec<u8>, E> {
    let hex_str = s
        .strip_prefix("0x")
        .ok_or_else(|| de::Error::custom("expected 0x prefix"))?;
    hex::decode(hex_str).map_err(|e| de::Error::custom(format!("hex decode: {e}")))
}

fn parse_attestation<E: de::Error>(value: serde_json::Value, electra: bool) -> Result<Vec<u8>, E> {
    #[derive(Deserialize)]
    struct Raw {
        aggregation_bits: String,
        data: serde_json::Value,
        committee_bits: Option<String>,
    }

    let raw: Raw = serde_json::from_value(value).map_err(|e| de::Error::custom(format!("{e}")))?;

    let agg_bytes = decode_0x_hex::<E>(&raw.aggregation_bits)?;
    let aggregation_bits =
        VariableList::new(agg_bytes).map_err(|_| de::Error::custom("aggregation_bits too long"))?;

    let data: AttestationData = serde_json::from_value(raw.data)
        .map_err(|e| de::Error::custom(format!("AttestationData: {e}")))?;

    if electra {
        let cb_str = raw
            .committee_bits
            .ok_or_else(|| de::Error::custom("missing committee_bits"))?;
        let cb_bytes = decode_0x_hex::<E>(&cb_str)?;
        let committee_bits = FixedVector::new(cb_bytes)
            .map_err(|_| de::Error::custom("committee_bits wrong length"))?;
        Ok(RawAttestationElectra {
            aggregation_bits,
            data,
            signature: AggregateSignature::infinity(),
            committee_bits,
        }
        .as_ssz_bytes())
    } else {
        Ok(RawAttestationBase {
            aggregation_bits,
            data,
            signature: AggregateSignature::infinity(),
        }
        .as_ssz_bytes())
    }
}

// ==================== AggregatorCommitteeConsensusData ====================

fn parse_aggregator_consensus_data<E: de::Error>(value: serde_json::Value) -> Result<Vec<u8>, E> {
    #[derive(Deserialize)]
    #[serde(rename_all = "PascalCase")]
    struct Raw {
        version: DataVersion,
        aggregators: Vec<AssignedAggregator>,
        #[serde(rename = "AggregatorsCommitteeIndexes")]
        aggregator_committee_indexes: Vec<u64>,
        #[serde(
            deserialize_with = "ssv_types::deserializers::deserialize_optional_base64_nested_variable_list"
        )]
        aggregated_attestations:
            VariableList<VariableList<u8, MaxAggregatedAttestationBytes>, MaxCommitteeIndexes>,
        contributors: Vec<AssignedAggregator>,
        sync_committee_contributions: Vec<RawContrib>,
    }

    #[derive(Deserialize)]
    struct RawContrib {
        slot: String,
        beacon_block_root: String,
        subcommittee_index: String,
        aggregation_bits: String,
    }

    let raw: Raw = serde_json::from_value(value).map_err(|e| de::Error::custom(format!("{e}")))?;

    let contributions = raw
        .sync_committee_contributions
        .into_iter()
        .map(|c| {
            let slot = Slot::new(
                c.slot
                    .parse::<u64>()
                    .map_err(|e| de::Error::custom(format!("slot: {e}")))?,
            );
            let root_bytes = decode_0x_hex::<E>(&c.beacon_block_root)?;
            let beacon_block_root = Hash256::from_slice(&root_bytes);
            let subcommittee_index = c
                .subcommittee_index
                .parse::<u64>()
                .map_err(|e| de::Error::custom(format!("subcommittee_index: {e}")))?;
            let agg_bytes = decode_0x_hex::<E>(&c.aggregation_bits)?;
            let aggregation_bits = BitVector::from_ssz_bytes(&agg_bytes)
                .map_err(|e| de::Error::custom(format!("aggregation_bits: {e:?}")))?;
            Ok(SyncCommitteeContribution {
                slot,
                beacon_block_root,
                subcommittee_index,
                aggregation_bits,
                signature: AggregateSignature::infinity(),
            })
        })
        .collect::<Result<Vec<_>, E>>()?;

    let data = AggregatorCommitteeConsensusData::<MainnetEthSpec> {
        version: raw.version,
        aggregators: VariableList::new(raw.aggregators)
            .map_err(|_| de::Error::custom("too many aggregators"))?,
        aggregator_committee_indexes: VariableList::new(raw.aggregator_committee_indexes)
            .map_err(|_| de::Error::custom("too many indexes"))?,
        aggregated_attestations: raw.aggregated_attestations,
        contributors: VariableList::new(raw.contributors)
            .map_err(|_| de::Error::custom("too many contributors"))?,
        sync_committee_contributions: VariableList::new(contributions)
            .map_err(|_| de::Error::custom("too many contributions"))?,
    };

    Ok(data.as_ssz_bytes())
}
