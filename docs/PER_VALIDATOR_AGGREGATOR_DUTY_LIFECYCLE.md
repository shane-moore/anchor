# Per-Validator Aggregator Duty Lifecycle in Anchor

## Overview

This document describes the **current implementation** of the per-validator aggregator duty lifecycle in Anchor. Note that there is also an **Aggregator Committee** feature planned (documented in [AGGREGATOR_COMMITTEE_IMPLEMENTATION_PLAN.md](../AGGREGATOR_COMMITTEE_IMPLEMENTATION_PLAN.md)) which consolidates aggregator duties into committee-based consensus, but this describes the existing per-validator flow.

---

## High-Level Architecture Diagram

```
┌─────────────────────────────────────────────────────────────────────────────────────────┐
│                              ANCHOR CLIENT ARCHITECTURE                                  │
├─────────────────────────────────────────────────────────────────────────────────────────┤
│                                                                                          │
│  ┌────────────────────┐       ┌───────────────────────┐      ┌───────────────────────┐  │
│  │   Lighthouse VC    │       │   MetadataService     │      │    DutiesTracker      │  │
│  │   (DutiesService)  │◄─────►│   (SlotMetadata)      │      │   (SyncCommittee)     │  │
│  └─────────┬──────────┘       └───────────┬───────────┘      └───────────────────────┘  │
│            │                              │                                              │
│            │ Attester/Sync Duties         │ BeaconVote + Validators                      │
│            ▼                              ▼                                              │
│  ┌─────────────────────────────────────────────────────────────────────────────────────┐ │
│  │                           AnchorValidatorStore                                       │ │
│  │  ┌───────────────────┐  ┌───────────────────┐  ┌───────────────────────────────┐   │ │
│  │  │ produce_selection │  │produce_signed_agg │  │ produce_signed_contribution_  │   │ │
│  │  │      _proof()     │  │  _and_proof()     │  │      and_proof()              │   │ │
│  │  └─────────┬─────────┘  └─────────┬─────────┘  └───────────────┬───────────────┘   │ │
│  └────────────┼──────────────────────┼────────────────────────────┼───────────────────┘ │
│               │                      │                            │                      │
│               │                      │ QBFT Consensus             │                      │
│               │                      ▼                            ▼                      │
│               │         ┌───────────────────────────────────────────────────┐            │
│               │         │               QbftManager                          │            │
│               │         │  ┌─────────────────────────────────────────────┐  │            │
│               │         │  │  validator_consensus_data_instances          │  │            │
│               │         │  │  (Map<ValidatorInstanceId, VCD>)            │  │            │
│               │         │  └─────────────────────────────────────────────┘  │            │
│               │         │  ┌─────────────────────────────────────────────┐  │            │
│               │         │  │  beacon_vote_instances                       │  │            │
│               │         │  │  (Map<CommitteeInstanceId, BeaconVote>)     │  │            │
│               │         │  └─────────────────────────────────────────────┘  │            │
│               │         └───────────────────────────────────────────────────┘            │
│               │                      │                                                   │
│               │                      │ Outgoing QbftMessages                             │
│               ▼                      ▼                                                   │
│  ┌─────────────────────────────────────────────────────────────────────────┐             │
│  │                    SignatureCollectorManager                             │             │
│  │  ┌─────────────────────────────────────────────────────────────────────┐│             │
│  │  │signature_collectors: DashMap<(Hash256, ValidatorIndex), Collector> ││             │
│  │  └─────────────────────────────────────────────────────────────────────┘│             │
│  │  ┌─────────────────────────────────────────────────────────────────────┐│             │
│  │  │committee_signatures: DashMap<(Hash256, CommitteeId), Signatures>   ││             │
│  │  └─────────────────────────────────────────────────────────────────────┘│             │
│  └─────────────────────────────────────────────────────────────────────────┘             │
│               │                      │                                                   │
│               │ PartialSignatures    │ QbftMessages                                      │
│               ▼                      ▼                                                   │
│  ┌─────────────────────────────────────────────────────────────────────────┐             │
│  │                         MessageSender                                    │             │
│  │                    (sign_and_send to network)                           │             │
│  └─────────────────────────────────────────────────────────────────────────┘             │
│                              │                                                           │
│                              ▼                                                           │
│  ┌─────────────────────────────────────────────────────────────────────────┐             │
│  │                   Network (libp2p/gossipsub)                            │             │
│  └─────────────────────────────────────────────────────────────────────────┘             │
│                              │                                                           │
│                              ▼ Incoming Messages                                         │
│  ┌─────────────────────────────────────────────────────────────────────────┐             │
│  │                   NetworkMessageReceiver                                 │             │
│  │  ┌──────────────┐   ┌────────────────────────────────────────────────┐  │             │
│  │  │   Validator  │──►│ Route to QbftManager or SignatureCollector    │  │             │
│  │  │  (validate)  │   └────────────────────────────────────────────────┘  │             │
│  │  └──────────────┘                                                        │             │
│  └─────────────────────────────────────────────────────────────────────────┘             │
│                                                                                          │
└─────────────────────────────────────────────────────────────────────────────────────────┘
```

---

## Component Descriptions

### 1. DutiesService (Lighthouse's Validator Client Component)

**Location**: External dependency `validator_services::duties_service::DutiesService`

The DutiesService is part of Lighthouse's validator client code that Anchor reuses. It:
- Polls the beacon node for attestation duties (`/eth/v1/validator/duties/attester/{epoch}`)
- Polls for sync committee duties (`/eth/v1/validator/duties/sync/{epoch}`)
- Calculates **selection proofs** for aggregator eligibility
- Maintains duty state per epoch

### 2. MetadataService

**Location**: `anchor/validator_store/src/metadata_service.rs`

The MetadataService runs at **1/3rd into each slot** and:
- Fetches attestation data from the beacon node
- Creates `BeaconVote` (block_root, source, target checkpoints)
- Collects attesting and sync committee validator indices
- Tracks multi-sync aggregators (validators aggregating for multiple subnets)
- Updates `SlotMetadata` in the validator store

```rust
pub struct SlotMetadata<E: EthSpec> {
    slot: Slot,
    beacon_vote: BeaconVote,
    attesting_validator_indices: Vec<ValidatorIndex>,
    attesting_validator_committees: HashMap<PublicKeyBytes, u64>,
    sync_validators: Vec<ValidatorIndex>,
    multi_sync_aggregators: HashMap<PublicKeyBytes, ContributionWaiter<E>>,
}
```

### 3. DutiesTracker

**Location**: `anchor/duties_tracker/src/duties_tracker.rs`

Maintains duty tracking for:
- Sync committee duties per period
- Voluntary exit tracking  
- Provides `DutiesProvider` trait for message validation

### 4. AnchorValidatorStore

**Location**: `anchor/validator_store/src/lib.rs`

The core implementation of Lighthouse's `ValidatorStore` trait. Handles:
- Key management (decryption of operator's key shares)
- Slashing protection
- Signature collection orchestration
- QBFT consensus initiation

### 5. QbftManager

**Location**: `anchor/qbft_manager/src/lib.rs`

Manages QBFT consensus instances:
- **ValidatorInstanceId** → `ValidatorConsensusData` (for proposer, aggregator, sync committee aggregator)
- **CommitteeInstanceId** → `BeaconVote` (for committee attestations)

### 6. SignatureCollectorManager

**Location**: `anchor/signature_collector/src/lib.rs`

Handles threshold signature collection:
- Creates partial signatures with operator's key share
- Broadcasts partial signatures to network
- Collects partial signatures from other operators
- Uses Lagrange interpolation to reconstruct full signatures

### 7. Processor

**Location**: `anchor/processor/src/lib.rs`

Central work queue with priority-based scheduling:
- `permitless` queue: Background tasks (cleanup, spawning)
- `urgent_consensus` queue: Time-critical consensus work

### 8. NetworkMessageReceiver

**Location**: `anchor/message_receiver/src/manager.rs`

Routes incoming network messages:
- Validates messages via `Validator`
- Routes QBFT messages to `QbftManager`
- Routes partial signatures to `SignatureCollectorManager`

---

## Key Data Types

### Roles and Message Identification

**`Role`** (defined in `anchor/common/ssv_types/src/msgid.rs`):

```rust
pub enum Role {
    Committee,             // 0 - Attesting (committee-based)
    Aggregator,            // 1 - Per-validator aggregator
    Proposer,              // 2 - Block proposal
    SyncCommittee,         // 3 - Per-validator sync committee aggregator
    ValidatorRegistration, // 4
    VoluntaryExit,         // 5
    AggregatorCommittee,   // 6 - Future: combined aggregators (per-committee)
}
```

**`DutyExecutor`**:

```rust
pub enum DutyExecutor {
    Committee(CommitteeId),      // For committee-based duties
    Validator(PublicKeyBytes),   // For per-validator duties
}
```

**`MessageId`**: 56-byte identifier containing:
- Domain (4 bytes)
- Role (4 bytes)
- Validator pubkey (48 bytes) OR Committee ID (32 bytes at offset 24)

### QBFT Instance Types

**`ValidatorInstanceId`** (defined in `anchor/qbft_manager/src/lib.rs`):

```rust
pub struct ValidatorInstanceId {
    pub validator: PublicKeyBytes,
    pub duty: ValidatorDutyKind,      // Proposal | Aggregator | SyncCommitteeAggregator
    pub instance_height: InstanceHeight,
}
```

**`CommitteeInstanceId`**:

```rust
pub struct CommitteeInstanceId {
    pub committee: CommitteeId,
    pub instance_height: InstanceHeight,
}
```

### Consensus Data Types

**`ValidatorConsensusData`** (defined in `anchor/common/ssv_types/src/consensus.rs`):

```rust
pub struct ValidatorConsensusData {
    pub duty: ValidatorDuty,
    pub version: DataVersion,
    pub data_ssz: VariableList<u8, ValidatorConsensusDataLen>,
}
```

**`ValidatorDuty`**:

```rust
pub struct ValidatorDuty {
    pub r#type: BeaconRole,              // AGGREGATOR, SYNC_COMMITTEE_CONTRIBUTION, etc.
    pub pub_key: PublicKeyBytes,
    pub slot: Slot,
    pub validator_index: ValidatorIndex,
    pub committee_index: CommitteeIndex,
    pub committee_length: u64,
    pub committees_at_slot: u64,
    pub validator_committee_index: u64,
    pub validator_sync_committee_indices: VariableList<u64, U13>,
}
```

### Partial Signature Types

**`PartialSignatureKind`** (defined in `anchor/common/ssv_types/src/partial_sig.rs`):

```rust
pub enum PartialSignatureKind {
    PostConsensus = 0,              // After QBFT decides
    RandaoPartialSig = 1,           // For block proposals
    SelectionProofPartialSig = 2,   // Aggregator selection proof
    ContributionProofs = 3,         // Sync committee contribution proofs
    ValidatorRegistration = 4,
    VoluntaryExit = 5,
    AggregatorCommitteePartialSig = 6, // Future: combined pre-consensus
}
```

---

## Detailed Per-Validator Aggregator Duty Lifecycle

### Phase 0: Duty Discovery (Every Epoch)

```
┌─────────────────────────────────────────────────────────────────────────────────┐
│                           DUTY DISCOVERY PHASE                                   │
├─────────────────────────────────────────────────────────────────────────────────┤
│                                                                                  │
│  Beacon Node                     DutiesService                                   │
│       │                              │                                           │
│       │  GET /eth/v1/validator/      │                                           │
│       │     duties/attester/{epoch}  │                                           │
│       │◄─────────────────────────────┤                                           │
│       │                              │                                           │
│       │  [{validator_index, slot,    │                                           │
│       │    committee_index, ...}]    │                                           │
│       ├─────────────────────────────►│                                           │
│       │                              │                                           │
│       │                              │ For each duty where                        │
│       │                              │ validator is potential aggregator:        │
│       │                              │                                           │
│       │                              │ ┌─────────────────────────────────────┐   │
│       │                              │ │ selection_proof = sign(slot,        │   │
│       │                              │ │                    DOMAIN_SELECTION)│   │
│       │                              │ │ is_aggregator = modulo_check(       │   │
│       │                              │ │   selection_proof, modulo)          │   │
│       │                              │ └─────────────────────────────────────┘   │
│       │                              │                                           │
│       │                              │ Store: slot → (duty, selection_proof,     │
│       │                              │               is_aggregator)              │
│       │                              │                                           │
└─────────────────────────────────────────────────────────────────────────────────┘
```

**Timeline**: Runs at epoch boundaries, looking ahead 1-2 epochs

**Key Functions**:
- `DutiesService::poll_beacon_attesters()` 
- Selection proof calculation happens in Lighthouse's duties service

### Phase 1: Pre-Consensus - Selection Proof Collection

```
┌─────────────────────────────────────────────────────────────────────────────────┐
│                    PRE-CONSENSUS: SELECTION PROOF PHASE                          │
│                      (At slot start, before 2/3 mark)                            │
├─────────────────────────────────────────────────────────────────────────────────┤
│                                                                                  │
│  Time: slot_start → slot_start + (2/3 * slot_duration)                          │
│                                                                                  │
│  AnchorValidatorStore                SignatureCollectorManager                   │
│       │                                      │                                   │
│       │ produce_selection_proof()            │                                   │
│       │───────────────────────────────────►  │                                   │
│       │                                      │                                   │
│       │  1. Get domain = DOMAIN_SELECTION    │                                   │
│       │  2. signing_root = slot.sign(domain) │                                   │
│       │  3. collect_signature():             │                                   │
│       │                                      │                                   │
│       │         ┌────────────────────────────┼────────────────────────────────┐  │
│       │         │  sign_and_collect()        │                                │  │
│       │         │    │                       │                                │  │
│       │         │    ▼                       │                                │  │
│       │         │  ┌─────────────────────────┴──────────────────────────────┐ │  │
│       │         │  │ 1. Decrypt operator's key share                        │ │  │
│       │         │  │ 2. Create partial signature                            │ │  │
│       │         │  │ 3. Broadcast PartialSignatureMessage                   │ │  │
│       │         │  │    (kind: SelectionProofPartialSig, role: Aggregator)  │ │  │
│       │         │  │ 4. Register with collector instance                    │ │  │
│       │         │  │ 5. Wait for threshold (2f+1) partial sigs              │ │  │
│       │         │  │ 6. Lagrange interpolation → full signature             │ │  │
│       │         │  └────────────────────────────────────────────────────────┘ │  │
│       │         └─────────────────────────────────────────────────────────────┘  │
│       │                                      │                                   │
│       │◄─────────────────────────────────────┤  SelectionProof                   │
│       │                                      │                                   │
│  4. Check is_aggregator(selection_proof)     │                                   │
│     - If NOT aggregator: duty complete       │                                   │
│     - If IS aggregator: continue to QBFT    │                                   │
│                                                                                  │
└─────────────────────────────────────────────────────────────────────────────────┘
```

**Key Code Path**:
1. `AnchorValidatorStore::produce_selection_proof()` (lib.rs lines 1272-1327)
2. `SignatureCollectorManager::sign_and_collect()` (lib.rs lines 92-146)
3. Timeout at 2/3 slot duration

**Signature Collection Flow**:

```rust
SignatureMetadata {
    kind: PartialSignatureKind::SelectionProofPartialSig,
    role: Role::Aggregator,
    threshold: 2f + 1,  // Calculated from cluster
    slot,
    committee_id,
}
```

### Phase 2: QBFT Consensus on AggregateAndProof

```
┌─────────────────────────────────────────────────────────────────────────────────┐
│                    CONSENSUS PHASE: QBFT INSTANCE                                │
│                    (At 2/3 slot mark)                                            │
├─────────────────────────────────────────────────────────────────────────────────┤
│                                                                                  │
│  Time: slot_start + (2/3 * slot_duration) → slot_end                            │
│                                                                                  │
│  AnchorValidatorStore              QbftManager                                   │
│       │                                │                                         │
│       │ produce_signed_aggregate_and_proof()                                     │
│       │                                │                                         │
│  1. Build AggregateAndProof:          │                                         │
│     - aggregator_index               │                                         │
│     - aggregate (from beacon node)    │                                         │
│     - selection_proof                 │                                         │
│       │                                │                                         │
│  2. Package as ValidatorConsensusData:│                                         │
│       │                                │                                         │
│       │    ValidatorConsensusData {   │                                         │
│       │      duty: ValidatorDuty {    │                                         │
│       │        type: BEACON_ROLE_AGGREGATOR,                                    │
│       │        pub_key,               │                                         │
│       │        slot,                  │                                         │
│       │        validator_index,       │                                         │
│       │        committee_index,       │                                         │
│       │      },                       │                                         │
│       │      version: ForkName,       │                                         │
│       │      data_ssz: AggregateAndProof.as_ssz_bytes()                        │
│       │    }                          │                                         │
│       │                                │                                         │
│       │ decide_instance()             │                                         │
│       ├───────────────────────────────►│                                         │
│       │                                │                                         │
│       │    ┌───────────────────────────┴──────────────────────────────────────┐ │
│       │    │  QBFT Instance (ValidatorDutyKind::Aggregator)                   │ │
│       │    │                                                                   │ │
│       │    │  instance_id = ValidatorInstanceId {                             │ │
│       │    │    validator: pubkey,                                             │ │
│       │    │    duty: ValidatorDutyKind::Aggregator,                          │ │
│       │    │    instance_height: slot,                                         │ │
│       │    │  }                                                                │ │
│       │    │                                                                   │ │
│       │    │  Rounds: max_rounds = 12 (per Role::Aggregator.max_round())      │ │
│       │    │                                                                   │ │
│       │    │  Messages exchanged:                                              │ │
│       │    │  - PROPOSAL  (leader broadcasts their data)                       │ │
│       │    │  - PREPARE   (operators vote on proposal)                         │ │
│       │    │  - COMMIT    (operators commit to decision)                       │ │
│       │    │  - ROUND_CHANGE (if timeout, advance round)                       │ │
│       │    │                                                                   │ │
│       │    │  All messages use MessageId with:                                 │ │
│       │    │    role: Role::Aggregator                                         │ │
│       │    │    duty_executor: DutyExecutor::Validator(pubkey)                │ │
│       │    └───────────────────────────────────────────────────────────────────┘ │
│       │                                │                                         │
│       │◄───────────────────────────────┤  Completed::Success(data)              │
│       │                                │                    or                   │
│       │                                │  Completed::TimedOut                    │
│       │                                │                                         │
└─────────────────────────────────────────────────────────────────────────────────┘
```

**Key Code Path**:
1. `AnchorValidatorStore::produce_signed_aggregate_and_proof()` (lib.rs lines 1121-1222)
2. `QbftManager::decide_instance()` (qbft_manager/src/lib.rs lines 147-204)
3. QBFT instance runs via `qbft_instance()` (qbft_manager/src/instance.rs)

**QBFT Message Flow**:

```
  Operator 1 (Leader)         Operator 2              Operator 3              Operator 4
       │                          │                       │                       │
       │  PROPOSAL(data)          │                       │                       │
       ├─────────────────────────►├──────────────────────►├──────────────────────►│
       │                          │                       │                       │
       │◄─────────────────────────┤  PREPARE              │                       │
       │◄─────────────────────────┼───────────────────────┤  PREPARE              │
       │◄─────────────────────────┼───────────────────────┼───────────────────────┤
       │                          │                       │                       │
       │  COMMIT                  │                       │                       │
       ├─────────────────────────►├──────────────────────►├──────────────────────►│
       │◄─────────────────────────┤  COMMIT               │                       │
       │◄─────────────────────────┼───────────────────────┤  COMMIT               │
       │                          │                       │                       │
       │     ═══════════════════════════════════════════════════════════════      │
       │                  CONSENSUS REACHED (2f+1 commits)                        │
       │     ═══════════════════════════════════════════════════════════════      │
```

### Phase 3: Post-Consensus - Sign Decided Data

```
┌─────────────────────────────────────────────────────────────────────────────────┐
│                    POST-CONSENSUS: FINAL SIGNATURE                               │
├─────────────────────────────────────────────────────────────────────────────────┤
│                                                                                  │
│  AnchorValidatorStore              SignatureCollectorManager                     │
│       │                                      │                                   │
│  After QBFT decides:                         │                                   │
│  - Decode decided ValidatorConsensusData     │                                   │
│  - Extract AggregateAndProof                 │                                   │
│       │                                      │                                   │
│       │ collect_signature()                  │                                   │
│       │   kind: PostConsensus                │                                   │
│       │   role: Aggregator                   │                                   │
│       ├─────────────────────────────────────►│                                   │
│       │                                      │                                   │
│       │    ┌─────────────────────────────────┴───────────────────────────────┐  │
│       │    │ 1. domain = DOMAIN_AGGREGATE_AND_PROOF                          │  │
│       │    │ 2. signing_root = aggregate_and_proof.signing_root(domain)      │  │
│       │    │ 3. Create partial signature with key share                      │  │
│       │    │ 4. Broadcast PartialSignatureMessage {                          │  │
│       │    │      kind: PostConsensus,                                       │  │
│       │    │      slot,                                                      │  │
│       │    │      signing_root,                                              │  │
│       │    │      validator_index,                                           │  │
│       │    │      partial_signature,                                         │  │
│       │    │      signer (operator_id),                                      │  │
│       │    │    }                                                            │  │
│       │    │ 5. Collect 2f+1 partial signatures                              │  │
│       │    │ 6. Reconstruct via Lagrange interpolation                       │  │
│       │    └─────────────────────────────────────────────────────────────────┘  │
│       │                                      │                                   │
│       │◄─────────────────────────────────────┤ Final Signature                   │
│       │                                      │                                   │
│  Build SignedAggregateAndProof {            │                                   │
│    message: decided_aggregate_and_proof,    │                                   │
│    signature: reconstructed_sig,            │                                   │
│  }                                           │                                   │
│       │                                      │                                   │
│       ▼                                      │                                   │
│  Return to Lighthouse's AggregationService  │                                   │
│  for beacon node submission                  │                                   │
│                                                                                  │
└─────────────────────────────────────────────────────────────────────────────────┘
```

**Key Code Path**:
1. Post-consensus signature in `produce_signed_aggregate_and_proof()` (lib.rs lines 1198-1218)
2. `collect_signature()` with `PartialSignatureKind::PostConsensus`
3. Final `SignedAggregateAndProof` returned to caller

---

## Complete Sequence Diagram

```
┌─────────────────────────────────────────────────────────────────────────────────────────────────────────┐
│                                    COMPLETE AGGREGATOR DUTY TIMELINE                                     │
├─────────────────────────────────────────────────────────────────────────────────────────────────────────┤
│                                                                                                          │
│  Time ───────────────────────────────────────────────────────────────────────────────────────────────►  │
│                                                                                                          │
│  Epoch N-1                        Slot S                                                                 │
│      │                                │                                                                  │
│      │                                │ 0s              4s             8s             12s               │
│      │                                │  ├───────────────┼──────────────┼──────────────┤                │
│      │                                │  │    1/3 slot   │   2/3 slot   │  slot end    │                │
│      │                                │  │               │              │              │                │
│      │ Poll duties                    │  │               │              │              │                │
│      │ ─────────────────────►         │  │               │              │              │                │
│      │                                │  │               │              │              │                │
│                                       │  ▼               │              │              │                │
│                                       │  MetadataService │              │              │                │
│                                       │  update_metadata()              │              │                │
│                                       │  creates BeaconVote             │              │                │
│                                       │                  │              │              │                │
│                                       │                  ▼              │              │                │
│                                       │            Selection Proof      │              │                │
│                                       │            Signing (if not      │              │                │
│                                       │            already computed     │              │                │
│                                       │            by DutiesService)    │              │                │
│                                       │                  │              │              │                │
│                                       │                  │  Timeout     │              │                │
│                                       │                  ├──────────────►              │                │
│                                       │                  │              │              │                │
│                                       │                  │              ▼              │                │
│                                       │                  │         QBFT Consensus      │                │
│                                       │                  │         on AggregateAndProof│                │
│                                       │                  │              │              │                │
│                                       │                  │              ▼              │                │
│                                       │                  │         Post-Consensus      │                │
│                                       │                  │         Signing             │                │
│                                       │                  │              │              │                │
│                                       │                  │              ▼              │                │
│                                       │                  │    SignedAggregateAndProof  │                │
│                                       │                  │         submitted to        │                │
│                                       │                  │         beacon node         │                │
│                                                                                                          │
└─────────────────────────────────────────────────────────────────────────────────────────────────────────┘
```

---

## Message Types and Routing

### Network Message Structure

All SSV messages follow this structure:

```rust
UnsignedSSVMessage {
    ssv_message: SSVMessage {
        msg_type: MsgType,        // ConsensusMsg or SSVPartialSignatureMsgType
        msg_id: MessageId,        // 56 bytes: domain + role + executor
        data: Vec<u8>,            // Encoded payload
    },
    full_data: Vec<u8>,           // Consensus data for proposals
}
```

### Message Routing in NetworkMessageReceiver

```rust
// anchor/message_receiver/src/manager.rs
match ssv_message {
    ValidatedSSVMessage::QbftMessage(qbft_message) => {
        // Route to QbftManager
        receiver.qbft_manager.receive_data(signed_ssv_message, qbft_message)
    }
    ValidatedSSVMessage::PartialSignatureMessages(messages) => {
        // Route to SignatureCollectorManager
        receiver.signature_collector.receive_partial_signatures(messages)
    }
}
```

### QBFT Manager Routing by DutyExecutor

```rust
// anchor/qbft_manager/src/lib.rs
match msg_id.duty_executor() {
    Some(DutyExecutor::Validator(validator)) => {
        // Per-validator duty (Proposer, Aggregator, SyncCommitteeAggregator)
        let duty = match msg_id.role() {
            Some(Role::Proposer) => ValidatorDutyKind::Proposal,
            Some(Role::Aggregator) => ValidatorDutyKind::Aggregator,
            Some(Role::SyncCommittee) => ValidatorDutyKind::SyncCommitteeAggregator,
            _ => return Err(...),
        };
        // Route to validator_consensus_data_instances map
    }
    Some(DutyExecutor::Committee(committee)) => {
        // Committee duty (attestations/sync messages)
        // Route to beacon_vote_instances map
    }
}
```

---

## Sync Committee Contribution Flow (Similar Pattern)

The sync committee contribution duty follows a similar pattern with these differences:

| Aspect | Aggregator | Sync Committee Contribution |
|--------|------------|----------------------------|
| **Selection Proof** | Signs `slot` with `DOMAIN_SELECTION` | Signs `SyncAggregatorSelectionData { slot, subcommittee_index }` with `DOMAIN_SYNC_COMMITTEE_SELECTION_PROOF` |
| **Pre-Consensus Kind** | `SelectionProofPartialSig` | `ContributionProofs` |
| **Consensus Data** | `AggregateAndProof` | `Contributions<E>` (list of `SyncCommitteeContribution`) |
| **Role** | `Role::Aggregator` | `Role::SyncCommittee` |
| **Duty Kind** | `ValidatorDutyKind::Aggregator` | `ValidatorDutyKind::SyncCommitteeAggregator` |
| **Multi-Aggregator** | N/A | Uses `ContributionWaiter` for validators aggregating multiple subnets |

```rust
// anchor/validator_store/src/lib.rs - produce_signed_contribution_and_proof()
ValidatorInstanceId {
    validator: aggregator_pubkey,
    duty: ValidatorDutyKind::SyncCommitteeAggregator,
    instance_height: slot.as_usize().into(),
}
```

---

## Future: Aggregator Committee (Per-Committee)

The [AGGREGATOR_COMMITTEE_IMPLEMENTATION_PLAN.md](../AGGREGATOR_COMMITTEE_IMPLEMENTATION_PLAN.md) describes a future optimization where multiple validators' aggregator duties are combined:

| Aspect | Current Per-Validator | Future Per-Committee |
|--------|----------------------|---------------------|
| **Role** | `Role::Aggregator` (1) | `Role::AggregatorCommittee` (6) |
| **DutyExecutor** | `DutyExecutor::Validator(pubkey)` | `DutyExecutor::Committee(committee_id)` |
| **Pre-Consensus** | Individual `SelectionProofPartialSig` | Combined `AggregatorCommitteePartialSig` |
| **Consensus Data** | `ValidatorConsensusData` per validator | `AggregatorCommitteeConsensusData` for all |
| **Consensus Rounds** | O(n) for n validators | O(1) regardless of validator count |

This reduces consensus rounds from O(n) to O(1) for n validators with aggregator duties in the same slot.

---

## Error Handling and Timeouts

### Timeout Boundaries

| Phase | Timeout | Behavior |
|-------|---------|----------|
| Selection Proof | 2/3 slot | Returns error, no aggregation attempted |
| QBFT Consensus | 12 rounds max | `Completed::TimedOut`, returns error |
| Post-Consensus Sig | Implicit (collector cleanup) | Collector cleaned after 1 slot |

### Cleanup Mechanisms

- **QbftManager**: Cleans instances older than `QBFT_RETAIN_SLOTS = 1`
- **SignatureCollectorManager**: Cleans collectors older than `SIGNATURE_COLLECTOR_RETAIN_SLOTS = 1`
- Both run cleanup tasks on slot boundaries

---

## Metrics and Observability

Key metrics are tracked via:

| Metric | Description |
|--------|-------------|
| `validator_metrics::CONSENSUS_TIMES` | Timer for QBFT duration |
| `validator_metrics::SIGNED_AGGREGATES_TOTAL` | Counter for aggregate signing |
| `validator_metrics::SIGNED_SELECTION_PROOFS_TOTAL` | Counter for selection proofs |
| `metrics::ANCHOR_PROCESSOR_*` | Processor queue metrics |

---

## File Reference Summary

| Component | File Path |
|-----------|-----------|
| AnchorValidatorStore | `anchor/validator_store/src/lib.rs` |
| MetadataService | `anchor/validator_store/src/metadata_service.rs` |
| QbftManager | `anchor/qbft_manager/src/lib.rs` |
| QBFT Instance | `anchor/qbft_manager/src/instance.rs` |
| SignatureCollectorManager | `anchor/signature_collector/src/lib.rs` |
| NetworkMessageReceiver | `anchor/message_receiver/src/manager.rs` |
| DutiesTracker | `anchor/duties_tracker/src/duties_tracker.rs` |
| Processor | `anchor/processor/src/lib.rs` |
| Role enum | `anchor/common/ssv_types/src/msgid.rs` |
| ValidatorDuty | `anchor/common/ssv_types/src/consensus.rs` |
| PartialSignatureKind | `anchor/common/ssv_types/src/partial_sig.rs` |
