# AggregatorCommitteePartialSig Complete Flow Trace

This document traces the complete lifecycle of `AggregatorCommitteePartialSig` messages during the pre-consensus phase, from creation through validation to signature reconstruction.

## Overview

The AggregatorCommittee role batches selection proofs for multiple validators in a committee into a single P2P message, reducing network overhead compared to per-validator messages.

**Key Design Principle**: `AggregatorCommitteeConsensusData` is a **committee-based data object** containing data for ALL validators in the committee. All operators reach consensus on the same data structure via QBFT. Individual validators then extract and sign their portion from the decided data. There is NO per-validator filtering of the consensus data - the full committee data is what goes through QBFT.

```
┌─────────────────────────────────────────────────────────────────────────────┐
│                        OUTBOUND (Creation)                                   │
│  produce_selection_proof() → collect_signature() → sign_and_collect()       │
│                              → create_message() → P2P Network                │
└─────────────────────────────────────────────────────────────────────────────┘
                                      │
                                      ▼
┌─────────────────────────────────────────────────────────────────────────────┐
│                        INBOUND (Reception)                                   │
│  P2P Network → message_receiver → validator.validate()                       │
│              → signature_collector → reconstruct signature                   │
└─────────────────────────────────────────────────────────────────────────────┘
```

---

## 1. OUTBOUND: Creating and Sending

### Entry Point: `produce_selection_proof()`

**File**: `validator_store/src/lib.rs:1514`

```
produce_selection_proof()
│
├── Check if fork >= Boole [line 1530]
│
├── Get voting_assignments [line 1532]
│
├── Calculate num_signatures_to_collect [line 1548-1551]
│   └── Uses selection_proof_count_for_committee()
│
├── Compute base_hash from SelectionProofBatchId [line 1555-1556]
│
└── collect_signature() [line 1566-1574]
    │   kind: AggregatorCommitteePartialSig
    │   role: AggregatorCommittee
    │   mode: Committee { num_signatures_to_collect, base_hash }
    │
    └── [validator_store/src/lib.rs:205-278]
        │
        ├── Create SignatureMetadata [line 216-226]
        │   kind: AggregatorCommitteePartialSig
        │   role: AggregatorCommittee
        │   threshold: 2f+1
        │
        ├── Create SignatureRequester::Committee [line 234-240]
        │
        └── signature_collector.sign_and_collect() [line 274-276]
            │
            └── [signature_collector/src/lib.rs:117-269]
                │
                ├── Register notifier [line 140-156]
                │
                ├── Create local partial signature [line 161-177]
                │
                ├── For Committee mode [line 202-254]:
                │   ├── Store in committee_signatures map
                │   └── When all collected:
                │       ├── create_message() [line 234-244]
                │       │   └── Creates MessageId with Role::AggregatorCommittee
                │       │       and DutyExecutor::Committee
                │       │
                │       └── message_sender.sign_and_send() [line 246-252]
                │           └── → P2P Network
                │
                └── Send own partial to local collector [line 259-261]
```

### Key Points - Outbound

1. **Fork Gating**: Only uses AggregatorCommittee path after Boole fork
2. **Batching**: Collects signatures for all validators in committee before sending
3. **Deterministic Batching**: Uses `SelectionProofBatchId` to ensure all operators use same `base_hash`
4. **Message ID**: Uses `Role::AggregatorCommittee` with `DutyExecutor::Committee(committee_id)`
5. **Committee-Based Data**: `AggregatorCommitteeConsensusData` contains ALL validators in the committee, not filtered per-validator. All operators reach consensus on the same full committee data.

---

## 2. INBOUND: Receiving and Validating

### Entry Point: `NetworkMessageReceiver::receive()`

**File**: `message_receiver/src/manager.rs:68`

```
NetworkMessageReceiver::receive()
│
├── validator.validate(&message.data) [line 80]
│   │
│   └── validate_ssv_message() [message_validator/src/lib.rs]
│       │
│       ├── Extract role from MessageId
│       │   └── Role::AggregatorCommittee (from [6,0,0,0] bytes)
│       │
│       ├── Get committee_info by CommitteeId [line 346-355]
│       │   └── Uses DutyExecutor::Committee (same as Role::Committee)
│       │
│       └── For MsgType::SSVPartialSignatureMsgType:
│           └── validate_partial_signature_message()
│               │
│               └── [partial_signature.rs:29-97]
│                   │
│                   ├── Decode PartialSignatureMessages [line 35-40]
│                   │
│                   ├── Fork gating [line 42-52]
│                   │   └── Reject if epoch < Boole
│                   │
│                   ├── validate_partial_signature_message_semantics() [line 99-155]
│                   │   │
│                   │   ├── Check single signer [line 104-107]
│                   │   │
│                   │   ├── Check no full_data [line 112-114]
│                   │   │
│                   │   ├── partial_signature_type_matches_role() [line 117-122]
│                   │   │   └── [line 174-177]: AggregatorCommittee accepts
│                   │   │       PostConsensus OR AggregatorCommitteePartialSig
│                   │   │
│                   │   └── For each message [line 130-152]:
│                   │       └── Skip validator_index check for is_committee_role()
│                   │
│                   ├── validate_partial_sig_messages_by_duty_logic() [line 182-341]
│                   │   │
│                   │   ├── Skip slot advancement check for is_committee_role()
│                   │   │
│                   │   ├── validate_slot_time() [line 214]
│                   │   │
│                   │   ├── validate_beacon_duty() [line 218]
│                   │   │
│                   │   ├── validate_duty_count() [line 226]
│                   │   │
│                   │   └── For Role::AggregatorCommittee [line 288-325]:
│                   │       │
│                   │       ├── Message count limit: min(5*V, V+4*512)
│                   │       │
│                   │       └── Validator index occurrence: max 5 per index
│                   │
│                   ├── verify_message_signature() [line 77-81]
│                   │
│                   └── duty_state.update_for_partial_signature() [line 90-94]
│
├── Check committee membership [line 140-161]
│   └── DutyExecutor::Committee(committee_id) → check cluster membership
│
└── Route to signature_collector [line 187-193]
    │
    └── signature_collector.receive_partial_signatures(messages)
```

### Key Points - Validation

| Check | Location | AggregatorCommittee Behavior |
|-------|----------|------------------------------|
| Fork gating | `partial_signature.rs:42-52` | Reject before Boole |
| Kind matching | `partial_signature.rs:174-177` | Accept `AggregatorCommitteePartialSig` or `PostConsensus` |
| Validator index | `partial_signature.rs:140` | **Skip** (uses `is_committee_role()`) |
| Slot advancement | `partial_signature.rs:201-209` | **Skip** (uses `is_committee_role()`) |
| Message count | `partial_signature.rs:288-310` | `min(5*V, V + 4*512)` |
| Index occurrences | `partial_signature.rs:314-325` | Max 5 per validator index |

---

## 3. SIGNATURE COLLECTION & RECONSTRUCTION

### Signature Collector Flow

**File**: `signature_collector/src/lib.rs`

```
receive_partial_signatures() [line 301-309]
│
└── For each message in messages:
    │
    └── receive_partial_signature() [line 311-344]
        │
        └── get_or_spawn(signing_root, validator_index, slot) [line 326-327]
            │
            └── Send CollectorMessage::PartialSignature to collector instance

signature_collector async task [line 502-586]
│
├── Receive PartialSignature [line 537-562]
│   └── Store in signature_share map by operator_id
│
└── When signature_share.len() >= threshold [line 565-584]:
    │
    ├── combine_signatures() [line 568] - Lagrange interpolation
    │
    └── Notify all waiters with Arc<Signature> [line 578-582]

sign_and_collect() resolves [line 268]
│
└── Returns to produce_selection_proof() [validator_store/src/lib.rs:1576]
    │
    └── Returns SelectionProof for duty execution
```

### Key Points - Collection

1. **Kind-Agnostic**: Signature collector doesn't differentiate by `PartialSignatureKind`
2. **Routing Key**: Collectors keyed by `(signing_root, validator_index)`
3. **Threshold**: Uses `2f + 1` from cluster configuration
4. **Reconstruction**: Lagrange interpolation via `bls_lagrange::combine_signatures()`

---

## 4. Validation Rules Summary

### AggregatorCommittee-Specific Rules

| Rule | Value | Rationale |
|------|-------|-----------|
| Message count limit | `min(5*V, V + 4*512)` | Each validator: 1 attestation + 4 sync subnets = 5 max |
| Validator index occurrences | Max 5 | Same rationale as message count |
| Skip validator index check | Yes | Operators may have divergent validator sets |
| Skip slot advancement check | Yes | Allows processing "older" slots within window |
| Accepted kinds | `AggregatorCommitteePartialSig`, `PostConsensus` | Pre-consensus and post-consensus phases |

### Message Count Formula Explained

```
max_allowed = min(5 * V, V + 4 * SYNC_COMMITTEE_SIZE)
            = min(5 * V, V + 2048)

Where:
  V = number of validators in committee
  SYNC_COMMITTEE_SIZE = 512

Examples:
  V = 10:   min(50, 2058)   = 50
  V = 100:  min(500, 2148)  = 500
  V = 512:  min(2560, 2560) = 2560  (boundary)
  V = 1000: min(5000, 3048) = 3048
```

---

## 5. Gap Analysis

| Component | Status | Notes |
|-----------|--------|-------|
| Fork gating (3 places) | ✅ Implemented | partial_sig, consensus_msg, qbft_manager |
| Kind validation | ✅ Implemented | `partial_signature_type_matches_role` handles AggregatorCommittee |
| Skip validator index check | ✅ Implemented | `is_committee_role()` |
| Skip slot advancement check | ✅ Implemented | `is_committee_role()` (both partial_sig and consensus_msg) |
| Message count limit | ✅ Implemented | `min(5*V, V+4*512)` |
| Validator index occurrence | ✅ Implemented | max 5 per index |
| Committee membership routing | ✅ Exists | Uses `DutyExecutor::Committee` same as Role::Committee |
| Signature collector reception | ✅ Exists | Kind-agnostic, routes by `(signing_root, validator_index)` |
| Signature reconstruction | ✅ Exists | Lagrange interpolation unchanged |
| Creation side | ✅ Exists | `produce_selection_proof` and `produce_sync_selection_proof` |

---

## 6. Related Files

### Core Implementation
- `anchor/message_validator/src/partial_signature.rs` - Validation logic
- `anchor/message_validator/src/consensus_message.rs` - Consensus message validation
- `anchor/signature_collector/src/lib.rs` - Signature collection and reconstruction
- `anchor/validator_store/src/lib.rs` - Creation side (`produce_selection_proof`)

### Routing
- `anchor/message_receiver/src/manager.rs` - Message entry point and routing
- `anchor/qbft_manager/src/lib.rs` - QBFT consensus routing

### Types
- `anchor/common/ssv_types/src/msgid.rs` - Role enum, `is_committee_role()`
- `anchor/common/ssv_types/src/partial_sig.rs` - `PartialSignatureKind` enum

---

## 7. See Also

- [AGGREGATOR_COMMITTEE_IMPLEMENTATION_PLAN.md](./AGGREGATOR_COMMITTEE_IMPLEMENTATION_PLAN.md) - Full implementation plan
- [COMMITTEE_AGGREGATOR_ARCHITECTURE.md](./COMMITTEE_AGGREGATOR_ARCHITECTURE.md) - Architecture overview
- [SELECTION_PROOF_BATCHING_ARCHITECTURE.md](./SELECTION_PROOF_BATCHING_ARCHITECTURE.md) - Batching design
- [MESSAGE_VALIDATION_QBFT_ROUTING_COMPARISON.md](./MESSAGE_VALIDATION_QBFT_ROUTING_COMPARISON.md) - Go SSV comparison
