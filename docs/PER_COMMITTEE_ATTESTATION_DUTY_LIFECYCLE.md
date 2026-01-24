# Per-Committee Attestation Duty Lifecycle in Anchor

## Overview

This document describes the **per-committee attestation duty lifecycle** in Anchor. Unlike per-validator duties (proposer, aggregator), attestation and sync committee message duties are consolidated at the **committee level**. This means all validators in the same committee share a single QBFT consensus instance and batch their partial signatures together, significantly reducing network overhead and consensus rounds.

---

## High-Level Architecture Diagram

```
┌─────────────────────────────────────────────────────────────────────────────────────────┐
│                          PER-COMMITTEE ATTESTATION ARCHITECTURE                          │
├─────────────────────────────────────────────────────────────────────────────────────────┤
│                                                                                          │
│  ┌────────────────────┐       ┌───────────────────────┐      ┌───────────────────────┐  │
│  │   Lighthouse VC    │       │   MetadataService     │      │    DutiesTracker      │  │
│  │   (DutiesService)  │◄─────►│   (SlotMetadata)      │      │   (SyncCommittee)     │  │
│  └─────────┬──────────┘       └───────────┬───────────┘      └───────────────────────┘  │
│            │                              │                                              │
│            │ All attesters in             │ BeaconVote at 1/3 slot                       │
│            │ this slot                    │ + validator indices                          │
│            ▼                              ▼                                              │
│  ┌─────────────────────────────────────────────────────────────────────────────────────┐ │
│  │                           AnchorValidatorStore                                       │ │
│  │  ┌───────────────────┐  ┌───────────────────┐  ┌───────────────────────────────┐   │ │
│  │  │  sign_attestation │  │  produce_sync_    │  │   get_slot_metadata()         │   │ │
│  │  │       ()          │  │  committee_sig()  │  │   (waits until 1/3 slot)      │   │ │
│  │  └─────────┬─────────┘  └─────────┬─────────┘  └───────────────────────────────┘   │ │
│  └────────────┼──────────────────────┼────────────────────────────────────────────────┘ │
│               │                      │                                                   │
│               │                      │                                                   │
│               └──────────┬───────────┘                                                   │
│                          │                                                               │
│                          ▼                                                               │
│         ┌───────────────────────────────────────────────────────────────┐               │
│         │                       QbftManager                              │               │
│         │  ┌─────────────────────────────────────────────────────────┐  │               │
│         │  │  beacon_vote_instances                                   │  │               │
│         │  │  Map<CommitteeInstanceId, BeaconVote>                   │  │               │
│         │  │                                                          │  │               │
│         │  │  Key: (CommitteeId, Slot) → Single QBFT Instance        │  │               │
│         │  │  All validators in committee share this instance!       │  │               │
│         │  └─────────────────────────────────────────────────────────┘  │               │
│         └───────────────────────────────────────────────────────────────┘               │
│                          │                                                               │
│                          │ Decided BeaconVote                                            │
│                          ▼                                                               │
│  ┌─────────────────────────────────────────────────────────────────────────┐             │
│  │                    SignatureCollectorManager                             │             │
│  │  ┌─────────────────────────────────────────────────────────────────────┐│             │
│  │  │ Committee Mode: Batch all validator signatures together            ││             │
│  │  │                                                                     ││             │
│  │  │ committee_signatures: Map<(BaseHash, CommitteeId), Signatures>     ││             │
│  │  │   - Waits for ALL validators in committee                          ││             │
│  │  │   - Sends ONE batched message with all partial sigs                ││             │
│  │  └─────────────────────────────────────────────────────────────────────┘│             │
│  │  ┌─────────────────────────────────────────────────────────────────────┐│             │
│  │  │ signature_collectors: Map<(SigningRoot, ValidatorIndex), Collector>││             │
│  │  │   - One collector per signing (validator, root) pair               ││             │
│  │  └─────────────────────────────────────────────────────────────────────┘│             │
│  └─────────────────────────────────────────────────────────────────────────┘             │
│                          │                                                               │
│                          │ Batched PartialSignatureMessages                              │
│                          ▼                                                               │
│  ┌─────────────────────────────────────────────────────────────────────────┐             │
│  │                         MessageSender                                    │             │
│  │            (sign_and_send with DutyExecutor::Committee)                 │             │
│  └─────────────────────────────────────────────────────────────────────────┘             │
│                                                                                          │
└─────────────────────────────────────────────────────────────────────────────────────────┘
```

---

## Per-Committee vs Per-Validator: Key Differences

| Aspect | Per-Validator (Aggregator/Proposer) | Per-Committee (Attestation/Sync Msg) |
|--------|-------------------------------------|--------------------------------------|
| **Instance ID** | `ValidatorInstanceId { validator, duty, slot }` | `CommitteeInstanceId { committee, slot }` |
| **Consensus Data** | `ValidatorConsensusData` | `BeaconVote` |
| **QBFT Instances** | One per validator per slot | **One per committee per slot** |
| **Role** | `Role::Aggregator`, `Role::Proposer` | `Role::Committee` |
| **DutyExecutor** | `DutyExecutor::Validator(pubkey)` | `DutyExecutor::Committee(committee_id)` |
| **Message ID** | Contains validator pubkey (48 bytes) | Contains committee ID (32 bytes at offset 24) |
| **Signature Batching** | Individual messages | **Batched per committee** |

---

## Component Descriptions

### 1. MetadataService

**Location**: `anchor/validator_store/src/metadata_service.rs`

The MetadataService is the **trigger point** for committee duties. It runs at **1/3rd into each slot** and:

```rust
impl<E: EthSpec, T: SlotClock + 'static> MetadataService<E, T> {
    async fn update_metadata(&self) -> Result<(), String> {
        let slot = self.slot_clock.now().ok_or("Failed to read slot clock")?;

        // 1. Fetch attestation data from beacon node
        let attestation_data = self.beacon_nodes
            .first_success(|beacon_node| async move {
                beacon_node.get_validator_attestation_data(slot, 0).await
            }).await?;

        // 2. Create BeaconVote from attestation data
        let beacon_vote = BeaconVote {
            block_root: attestation_data.beacon_block_root,
            source: attestation_data.source,
            target: attestation_data.target,
        };

        // 3. Collect all attesting validators and their committees
        let (attesting_validator_indices, attesting_validator_committees) = 
            self.duties_service.attesters(slot)
                .into_iter()
                .map(|duty| {
                    (ValidatorIndex(duty.duty.validator_index as usize),
                     (duty.duty.pubkey, duty.duty.committee_index))
                })
                .unzip();

        // 4. Collect sync committee validators
        let sync_validators = ...;

        // 5. Update slot metadata
        self.validator_store.update_slot_metadata(SlotMetadata {
            slot,
            beacon_vote,
            attesting_validator_indices,
            attesting_validator_committees,
            sync_validators,
            multi_sync_aggregators,
        });
    }
}
```

**Key Output**: `SlotMetadata` containing:
- `beacon_vote`: The `BeaconVote` to propose in QBFT
- `attesting_validator_indices`: All validators attesting this slot
- `attesting_validator_committees`: Map of pubkey → attestation committee index
- `sync_validators`: All validators in sync committee this slot

### 2. BeaconVote

**Location**: `anchor/common/ssv_types/src/consensus.rs`

The consensus data type for committee duties:

```rust
#[derive(Clone, Debug, TreeHash, PartialEq, Eq, Encode, Decode)]
pub struct BeaconVote {
    pub block_root: Hash256,      // Head block root
    pub source: Checkpoint,       // Source checkpoint (epoch, root)
    pub target: Checkpoint,       // Target checkpoint (epoch, root)
}

impl QbftData for BeaconVote {
    type Hash = Hash256;

    fn hash(&self) -> Self::Hash {
        let bytes = self.as_ssz_bytes();
        let mut hasher = Sha256::new();
        hasher.update(bytes);
        Hash256::from(hasher.finalize())
    }
}
```

**Note**: `BeaconVote` is much smaller than `ValidatorConsensusData` - it contains only the essential checkpoint data needed for attestations.

### 3. BeaconVoteValidator

**Location**: `anchor/common/ssv_types/src/consensus.rs`

Validates proposed `BeaconVote` data before accepting in QBFT:

```rust
pub struct BeaconVoteValidator<E: EthSpec> {
    slot: Slot,
    slashing_database: Option<Arc<SlashingDatabase>>,
    spec: Arc<ChainSpec>,
    validator_attestation_committees: HashMap<PublicKeyBytes, u64>,
    genesis_validators_root: Hash256,
    strict_mfp: bool,  // Majority Fork Protection mode
}

impl<E: EthSpec> QbftDataValidator<BeaconVote> for BeaconVoteValidator<E> {
    fn validate(&self, value: &BeaconVote, our_value: &BeaconVote) -> bool {
        // 1. Check target epoch not too far in future
        // 2. Check source epoch < target epoch
        // 3. Majority Fork Protection (epoch or strict mode)
        // 4. Slashing protection for ALL validators in committee
    }
}
```

**Majority Fork Protection (MFP)**:
- **Epoch MFP** (default): Only source/target epochs must match (allows different roots at epoch boundaries)
- **Strict MFP**: Full checkpoint match required (source + target including roots)

### 4. CommitteeInstanceId

**Location**: `anchor/qbft_manager/src/lib.rs`

The identifier for committee-based QBFT instances:

```rust
#[derive(Debug, Clone, Hash, PartialEq, Eq)]
pub struct CommitteeInstanceId {
    pub committee: CommitteeId,           // 32-byte committee identifier
    pub instance_height: InstanceHeight,  // Slot number
}
```

**Critical Insight**: All validators in the same committee with attestation duties in the same slot share a **single QBFT instance**. This dramatically reduces:
- Network messages (1 consensus instead of N)
- QBFT rounds needed
- Latency for large committees

### 5. Committee Signature Collection

**Location**: `anchor/signature_collector/src/lib.rs`

For committee duties, signatures are **batched** before sending:

```rust
pub enum SignatureRequester {
    SingleValidator { pubkey: PublicKeyBytes },
    Committee {
        num_signatures_to_collect: usize,  // How many validators in committee
        base_hash: Hash256,                 // Hash of decided BeaconVote
    },
}
```

When using `Committee` mode:
1. Each validator's partial signature is collected locally
2. Once ALL validators in the committee have signed, ONE message is sent
3. The message contains all partial signatures bundled together

```rust
// anchor/signature_collector/src/lib.rs - sign_and_collect()
SignatureRequester::Committee { num_signatures_to_collect, base_hash } => {
    // Get or create entry for this committee's signatures
    let mut entry = manager.committee_signatures
        .entry((base_hash, metadata.committee_id));
    
    // Add our partial signature
    collected_signatures.push(message.clone());
    
    // Only send when we have ALL signatures
    if collected_signatures.len() == num_signatures_to_collect {
        let signatures = entry.remove().collected_signatures;
        manager.message_sender.sign_and_send(
            manager.create_message(
                &metadata,
                signatures,  // All signatures in one message!
                &DutyExecutor::Committee(metadata.committee_id),
            ),
            ...
        );
    }
}
```

---

## Key Data Types

### Roles and Message Identification

**`Role::Committee`** (value = 0):

```rust
pub enum Role {
    Committee,             // 0 - Committee-based duties (attestation, sync message)
    Aggregator,            // 1
    Proposer,              // 2
    SyncCommittee,         // 3 - Sync committee AGGREGATOR (per-validator)
    ValidatorRegistration, // 4
    VoluntaryExit,         // 5
    AggregatorCommittee,   // 6
}
```

**Important**: `Role::Committee` is for attestations AND sync committee messages (non-aggregation). `Role::SyncCommittee` is for sync committee **aggregation** (per-validator).

**`DutyExecutor::Committee`**:

```rust
pub enum DutyExecutor {
    Committee(CommitteeId),      // 32-byte committee ID embedded in MessageId
    Validator(PublicKeyBytes),   // 48-byte pubkey embedded in MessageId
}
```

### MessageId Construction for Committee

From `anchor/common/ssv_types/src/msgid.rs`:

```rust
impl MessageId {
    pub fn new(domain: &DomainType, role: Role, duty_executor: &DutyExecutor) -> Self {
        let mut bytes = [0u8; MESSAGE_ID_LEN];
        bytes[0..4].copy_from_slice(&domain.as_bytes());
        bytes[4..8].copy_from_slice(&role.as_bytes());
        
        match duty_executor {
            DutyExecutor::Committee(committee_id) => {
                // Committee ID at offset 24 (leaves space for alignment)
                bytes[24..56].copy_from_slice(committee_id.as_bytes());
            }
            DutyExecutor::Validator(pubkey) => {
                bytes[8..56].copy_from_slice(pubkey.as_bytes());
            }
        }
        MessageId(bytes)
    }
}
```

### Partial Signature Collection

For committee duties, `PartialSignatureKind::PostConsensus` is used:

```rust
pub enum PartialSignatureKind {
    PostConsensus = 0,              // After QBFT decides - used for attestations
    RandaoPartialSig = 1,
    SelectionProofPartialSig = 2,
    ContributionProofs = 3,
    ValidatorRegistration = 4,
    VoluntaryExit = 5,
    AggregatorCommitteePartialSig = 6,
}
```

---

## Detailed Attestation Duty Lifecycle

### Timeline Overview

```
┌─────────────────────────────────────────────────────────────────────────────────────────┐
│                              ATTESTATION DUTY TIMELINE                                   │
├─────────────────────────────────────────────────────────────────────────────────────────┤
│                                                                                          │
│  Epoch N-1                              Slot S (12 seconds)                              │
│      │                                      │                                            │
│      │ Poll attestation duties              │ 0s        4s        8s        12s         │
│      │ for epoch N                          │  │         │         │          │          │
│      ├─────────────────────►                │  │         │         │          │          │
│      │                                      │  │ 1/3     │ 2/3     │ slot end │          │
│      │                                      │  │         │         │          │          │
│                                             │  ▼         │         │          │          │
│                                             │ MetadataService                  │          │
│                                             │ update_metadata()               │          │
│                                             │ creates BeaconVote              │          │
│                                             │  │                              │          │
│                                             │  ▼                              │          │
│                                             │ sign_attestation() called       │          │
│                                             │ for EACH validator              │          │
│                                             │  │                              │          │
│                                             │  │ All validators in           │          │
│                                             │  │ same committee JOIN         │          │
│                                             │  │ same QBFT instance          │          │
│                                             │  ▼                              │          │
│                                             │ ┌─────────────────────────────┐│          │
│                                             │ │   QBFT Consensus            ││          │
│                                             │ │   CommitteeInstanceId {     ││          │
│                                             │ │     committee,              ││          │
│                                             │ │     slot,                   ││          │
│                                             │ │   }                         ││          │
│                                             │ │   Data: BeaconVote          ││          │
│                                             │ └─────────────────────────────┘│          │
│                                             │               │                │          │
│                                             │               ▼                │          │
│                                             │   ┌───────────────────────────┐│          │
│                                             │   │ Post-Consensus Signing    ││          │
│                                             │   │ (batched per committee)   ││          │
│                                             │   └───────────────────────────┘│          │
│                                             │               │                │          │
│                                             │               ▼                │          │
│                                             │   Attestations submitted to    │          │
│                                             │   beacon node                  │          │
│                                             │                                            │
└─────────────────────────────────────────────────────────────────────────────────────────┘
```

### Phase 1: Duty Discovery and Metadata Preparation

```
┌─────────────────────────────────────────────────────────────────────────────────────────┐
│                           PHASE 1: METADATA PREPARATION                                  │
│                           (At 1/3 into slot = 4 seconds)                                │
├─────────────────────────────────────────────────────────────────────────────────────────┤
│                                                                                          │
│  Beacon Node                    MetadataService                  AnchorValidatorStore    │
│       │                              │                                    │              │
│       │ GET /eth/v1/validator/       │                                    │              │
│       │   attestation_data           │                                    │              │
│       │◄─────────────────────────────┤                                    │              │
│       │                              │                                    │              │
│       │ AttestationData {            │                                    │              │
│       │   slot,                      │                                    │              │
│       │   index,                     │                                    │              │
│       │   beacon_block_root,         │                                    │              │
│       │   source: Checkpoint,        │                                    │              │
│       │   target: Checkpoint,        │                                    │              │
│       │ }                            │                                    │              │
│       ├─────────────────────────────►│                                    │              │
│       │                              │                                    │              │
│       │                              │  Create BeaconVote:                │              │
│       │                              │  ┌────────────────────────────────┐│              │
│       │                              │  │ BeaconVote {                   ││              │
│       │                              │  │   block_root,                  ││              │
│       │                              │  │   source: Checkpoint,          ││              │
│       │                              │  │   target: Checkpoint,          ││              │
│       │                              │  │ }                              ││              │
│       │                              │  └────────────────────────────────┘│              │
│       │                              │                                    │              │
│       │                              │  Collect attesting validators:     │              │
│       │                              │  - validator_indices[]             │              │
│       │                              │  - pubkey → committee_index map    │              │
│       │                              │                                    │              │
│       │                              │  update_slot_metadata()            │              │
│       │                              ├───────────────────────────────────►│              │
│       │                              │                                    │              │
│       │                              │         SlotMetadata {             │              │
│       │                              │           slot,                    │              │
│       │                              │           beacon_vote,             │              │
│       │                              │           attesting_validators,    │              │
│       │                              │           sync_validators,         │              │
│       │                              │         }                          │              │
│                                                                                          │
└─────────────────────────────────────────────────────────────────────────────────────────┘
```

**Key Point**: The `MetadataService` prepares the `BeaconVote` ONCE for the entire slot. All validators in the committee will use this same data.

### Phase 2: QBFT Consensus on BeaconVote

```
┌─────────────────────────────────────────────────────────────────────────────────────────┐
│                    PHASE 2: COMMITTEE-BASED QBFT CONSENSUS                               │
│                    (Starting at 1/3 slot)                                                │
├─────────────────────────────────────────────────────────────────────────────────────────┤
│                                                                                          │
│  Lighthouse VC                  AnchorValidatorStore                QbftManager          │
│       │                              │                                   │               │
│       │ sign_attestation(v1, att)    │                                   │               │
│       ├─────────────────────────────►│                                   │               │
│       │ sign_attestation(v2, att)    │                                   │               │
│       ├─────────────────────────────►│                                   │               │
│       │ sign_attestation(v3, att)    │                                   │               │
│       ├─────────────────────────────►│                                   │               │
│       │                              │                                   │               │
│       │     (All 3 validators in     │                                   │               │
│       │      same committee)         │                                   │               │
│       │                              │                                   │               │
│       │                              │ 1. get_slot_metadata(slot)        │               │
│       │                              │    (waits until ready)            │               │
│       │                              │                                   │               │
│       │                              │ 2. get_attesting_validators_in_committee()        │
│       │                              │    → validator_attestation_committees             │
│       │                              │                                   │               │
│       │                              │ 3. decide_instance()              │               │
│       │                              ├──────────────────────────────────►│               │
│       │                              │                                   │               │
│       │                              │   CommitteeInstanceId {           │               │
│       │                              │     committee: committee_id,      │               │
│       │                              │     instance_height: slot,        │               │
│       │                              │   }                               │               │
│       │                              │                                   │               │
│       │                              │   BeaconVote {                    │               │
│       │                              │     block_root,                   │               │
│       │                              │     source,                       │               │
│       │                              │     target,                       │               │
│       │                              │   }                               │               │
│       │                              │                                   │               │
│       │    ┌─────────────────────────┴───────────────────────────────────┴──────────┐   │
│       │    │                                                                         │   │
│       │    │  CRITICAL: All 3 validators JOIN THE SAME QBFT INSTANCE!              │   │
│       │    │                                                                         │   │
│       │    │  beacon_vote_instances: Map<CommitteeInstanceId, Sender>               │   │
│       │    │                                                                         │   │
│       │    │  First validator to call decide_instance() SPAWNS the instance.        │   │
│       │    │  Subsequent validators JOIN the existing instance.                      │   │
│       │    │                                                                         │   │
│       │    │  ┌─────────────────────────────────────────────────────────────────┐   │   │
│       │    │  │                    QBFT Instance                                 │   │   │
│       │    │  │                                                                  │   │   │
│       │    │  │  max_rounds = 12 (per Role::Committee.max_round())              │   │   │
│       │    │  │                                                                  │   │   │
│       │    │  │  Validator: BeaconVoteValidator                                 │   │   │
│       │    │  │    - Checks epoch validity                                      │   │   │
│       │    │  │    - Majority Fork Protection                                   │   │   │
│       │    │  │    - Slashing check for ALL validators in committee            │   │   │
│       │    │  │                                                                  │   │   │
│       │    │  │  Messages use:                                                  │   │   │
│       │    │  │    Role::Committee                                              │   │   │
│       │    │  │    DutyExecutor::Committee(committee_id)                        │   │   │
│       │    │  │                                                                  │   │   │
│       │    │  └─────────────────────────────────────────────────────────────────┘   │   │
│       │    │                                                                         │   │
│       │    └─────────────────────────────────────────────────────────────────────────┘   │
│       │                              │                                   │               │
│       │                              │◄──────────────────────────────────┤               │
│       │                              │   Completed::Success(BeaconVote)  │               │
│       │                              │                                   │               │
│       │                              │ 4. Update attestation data:       │               │
│       │                              │    attestation.data.block_root = data.block_root  │
│       │                              │    attestation.data.source = data.source          │
│       │                              │    attestation.data.target = data.target          │
│       │                              │                                   │               │
│                                                                                          │
└─────────────────────────────────────────────────────────────────────────────────────────┘
```

**Key Code Path** (from `anchor/validator_store/src/lib.rs`):

```rust
async fn sign_attestation(
    &self,
    validator_pubkey: PublicKeyBytes,
    validator_committee_position: usize,
    attestation: &mut Attestation<E>,
    current_epoch: Epoch,
) -> Result<(), Error> {
    // Wait for metadata to be ready (at 1/3 slot)
    let slot_metadata = self.get_slot_metadata(attestation.data().slot).await?;

    // Get validators in our committee that are attesting
    let validator_attestation_committees =
        self.get_attesting_validators_in_committee(&slot_metadata, cluster.committee_id());

    // QBFT consensus on BeaconVote
    let completed = self.qbft_manager
        .decide_instance(
            CommitteeInstanceId {
                committee: cluster.committee_id(),
                instance_height: attestation.data().slot.as_usize().into(),
            },
            BeaconVote {
                block_root: attestation.data().beacon_block_root,
                source: attestation.data().source,
                target: attestation.data().target,
            },
            self.create_beacon_vote_validator(
                attestation.data().slot,
                validator_attestation_committees,
            ),
            start_time,
            &cluster,
        )
        .await?;

    // Apply decided data to attestation
    attestation.data_mut().beacon_block_root = data.block_root;
    attestation.data_mut().source = data.source;
    attestation.data_mut().target = data.target;
    ...
}
```

### Phase 3: Post-Consensus Signature Collection (Batched)

```
┌─────────────────────────────────────────────────────────────────────────────────────────┐
│               PHASE 3: POST-CONSENSUS BATCHED SIGNATURE COLLECTION                       │
├─────────────────────────────────────────────────────────────────────────────────────────┤
│                                                                                          │
│  AnchorValidatorStore              SignatureCollectorManager                Network      │
│       │                                      │                                │          │
│  For EACH validator in committee:            │                                │          │
│       │                                      │                                │          │
│  V1:  │ collect_signature()                  │                                │          │
│       │   kind: PostConsensus                │                                │          │
│       │   role: Committee                    │                                │          │
│       │   mode: Committee { count: 3, hash } │                                │          │
│       ├─────────────────────────────────────►│                                │          │
│       │                                      │                                │          │
│       │                                      │ sign_and_collect():            │          │
│       │                                      │ 1. Decrypt key share for V1    │          │
│       │                                      │ 2. Create partial sig          │          │
│       │                                      │ 3. Add to committee_signatures │          │
│       │                                      │    collected: [sig1]           │          │
│       │                                      │    need: 3                     │          │
│       │                                      │ 4. Not ready to send yet       │          │
│       │                                      │                                │          │
│  V2:  │ collect_signature()                  │                                │          │
│       ├─────────────────────────────────────►│                                │          │
│       │                                      │ 1-4. Same process              │          │
│       │                                      │    collected: [sig1, sig2]     │          │
│       │                                      │    need: 3                     │          │
│       │                                      │ Not ready yet                  │          │
│       │                                      │                                │          │
│  V3:  │ collect_signature()                  │                                │          │
│       ├─────────────────────────────────────►│                                │          │
│       │                                      │ 1-4. Same process              │          │
│       │                                      │    collected: [sig1,sig2,sig3] │          │
│       │                                      │    need: 3                     │          │
│       │                                      │                                │          │
│       │                                      │ 5. ALL COLLECTED! Send batch:  │          │
│       │                                      │                                │          │
│       │                                      │  PartialSignatureMessages {    │          │
│       │                                      │    kind: PostConsensus,        │          │
│       │                                      │    slot,                       │          │
│       │                                      │    messages: [                 │          │
│       │                                      │      {root, signer, v1_idx,    │          │
│       │                                      │       partial_sig},            │          │
│       │                                      │      {root, signer, v2_idx,    │          │
│       │                                      │       partial_sig},            │          │
│       │                                      │      {root, signer, v3_idx,    │          │
│       │                                      │       partial_sig},            │          │
│       │                                      │    ]                           │          │
│       │                                      │  }                             │          │
│       │                                      │                                │          │
│       │                                      │ ONE message with MessageId:    │          │
│       │                                      │   Role::Committee              │          │
│       │                                      │   DutyExecutor::Committee(id)  │          │
│       │                                      ├───────────────────────────────►│          │
│       │                                      │                                │          │
│       │                                      │◄───────────────────────────────┤          │
│       │                                      │ Receive partial sigs from      │          │
│       │                                      │ other operators (also batched) │          │
│       │                                      │                                │          │
│       │◄─────────────────────────────────────┤ Reconstructed signatures       │          │
│       │   V1: Signature                      │ (one per validator)            │          │
│       │   V2: Signature                      │                                │          │
│       │   V3: Signature                      │                                │          │
│       │                                      │                                │          │
│  Add signatures to attestations              │                                │          │
│  and return to Lighthouse                    │                                │          │
│                                                                                          │
└─────────────────────────────────────────────────────────────────────────────────────────┘
```

**Key Code Path** (signature collection in committee mode):

```rust
// anchor/validator_store/src/lib.rs - inside sign_attestation()
let signature = self
    .collect_signature(
        PartialSignatureKind::PostConsensus,
        Role::Committee,
        CollectionMode::Committee {
            slot_metadata,
            base_hash: data_hash,  // Hash of decided BeaconVote
        },
        &validator,
        &cluster,
        signing_root,
        attestation.data().slot,
    )
    .await?;

// Add signature to attestation
attestation.add_signature(&signature, validator_committee_position)?;
```

---

## QBFT Message Flow for Committee Consensus

```
  Operator 1                 Operator 2              Operator 3              Operator 4
  (has V1,V2)               (has V3)                (has V1,V3)             (has V2)
       │                         │                       │                       │
       │                         │                       │                       │
       │  PROPOSAL(BeaconVote)   │                       │                       │
       ├────────────────────────►├──────────────────────►├──────────────────────►│
       │  (only consensus data,  │                       │                       │
       │   not per-validator)    │                       │                       │
       │                         │                       │                       │
       │◄────────────────────────┤  PREPARE              │                       │
       │◄────────────────────────┼───────────────────────┤  PREPARE              │
       │◄────────────────────────┼───────────────────────┼───────────────────────┤
       │                         │                       │                       │
       │  COMMIT                 │                       │                       │
       ├────────────────────────►├──────────────────────►├──────────────────────►│
       │◄────────────────────────┤  COMMIT               │                       │
       │◄────────────────────────┼───────────────────────┤  COMMIT               │
       │                         │                       │                       │
       │     ═══════════════════════════════════════════════════════════════      │
       │           CONSENSUS REACHED on BeaconVote (2f+1 commits)                 │
       │     ═══════════════════════════════════════════════════════════════      │
       │                         │                       │                       │
       │                         │                       │                       │
       │  POST-CONSENSUS         │                       │                       │
       │  PartialSigs [V1,V2]    │                       │                       │
       ├────────────────────────►├──────────────────────►├──────────────────────►│
       │                         │                       │                       │
       │◄────────────────────────┤  PartialSigs [V3]     │                       │
       │◄────────────────────────┼───────────────────────┤  PartialSigs [V1,V3]  │
       │◄────────────────────────┼───────────────────────┼───────────────────────┤  [V2]
       │                         │                       │                       │
       │     ═══════════════════════════════════════════════════════════════      │
       │       Lagrange interpolation for each validator: V1, V2, V3              │
       │     ═══════════════════════════════════════════════════════════════      │
```

**Key Efficiency Gains**:
1. **One QBFT instance** per committee per slot (not per validator)
2. **Batched partial signatures** - operators send all their validators' sigs in one message
3. **Shared validation** - slashing check covers all validators in committee at once

---

## Sync Committee Message Flow (Same Pattern)

Sync committee **messages** (not aggregation) follow the same per-committee pattern:

```rust
// anchor/validator_store/src/lib.rs
async fn produce_sync_committee_signature(
    &self,
    slot: Slot,
    _beacon_block_root: Hash256,
    validator_index: u64,
    validator_pubkey: &PublicKeyBytes,
) -> Result<SyncCommitteeMessage, Error> {
    // Same QBFT on BeaconVote, shared with attestations!
    let completed = self.qbft_manager
        .decide_instance(
            CommitteeInstanceId {
                committee: cluster.committee_id(),
                instance_height: slot.as_usize().into(),
            },
            metadata.beacon_vote.clone(),  // Same BeaconVote as attestations!
            self.create_beacon_vote_validator(slot, validator_attestation_committees),
            start_time,
            &cluster,
        )
        .await?;

    // Same committee-based signature collection
    let signature = self.collect_signature(
        PartialSignatureKind::PostConsensus,
        Role::Committee,
        CollectionMode::Committee { slot_metadata: metadata, base_hash: data.hash() },
        ...
    ).await?;

    Ok(SyncCommitteeMessage { slot, beacon_block_root: data.block_root, validator_index, signature })
}
```

**Important**: Attestations and sync committee messages **share the same QBFT instance** if they're in the same committee and slot!

---

## Message Routing

### Incoming Message Routing

From `anchor/qbft_manager/src/lib.rs`:

```rust
pub fn receive_data(
    &self,
    full_message: SignedSSVMessage,
    qbft_message: QbftMessage,
) -> Result<(), QbftError> {
    let msg_id = full_message.ssv_message().msg_id();
    let instance_height = (qbft_message.height as usize).into();

    match msg_id.duty_executor() {
        Some(DutyExecutor::Committee(committee)) => {
            // Route to beacon_vote_instances map
            let id = CommitteeInstanceId { committee, instance_height };
            self.pass_to_instance::<BeaconVote>(id, WrappedQbftMessage { ... })
        }
        Some(DutyExecutor::Validator(validator)) => {
            // Route to validator_consensus_data_instances map
            ...
        }
    }
}
```

### Outgoing Message Construction

From `anchor/signature_collector/src/lib.rs`:

```rust
fn create_message(
    &self,
    metadata: &SignatureMetadata,
    signatures: Vec<PartialSignatureMessage>,
    duty_executor: &DutyExecutor,  // Committee(committee_id) for attestations
) -> UnsignedSSVMessage {
    UnsignedSSVMessage {
        ssv_message: SSVMessage::new(
            MsgType::SSVPartialSignatureMsgType,
            MessageId::new(&self.domain, metadata.role, duty_executor),
            PartialSignatureMessages { kind: metadata.kind, slot: metadata.slot, messages: signatures }.as_ssz_bytes(),
        ),
        full_data: vec![],
    }
}
```

---

## Error Handling and Edge Cases

### Majority Fork Protection

The `BeaconVoteValidator` implements Majority Fork Protection (MFP) to handle edge cases:

```rust
// Epoch MFP (default, more lenient)
fn epoch_majority_fork_protection(value: &BeaconVote, our_value: &BeaconVote) -> Result<()> {
    // Only epochs must match, roots can differ
    if value.source.epoch != our_value.source.epoch
        || value.target.epoch != our_value.target.epoch {
        Err(BeaconVoteValidationError::EpochMismatch(...))
    } else {
        Ok(())
    }
}

// Strict MFP (optional, stricter)
fn strict_majority_fork_protection(value: &BeaconVote, our_value: &BeaconVote) -> Result<()> {
    // Full checkpoint must match (epoch AND root)
    if value.source != our_value.source || value.target != our_value.target {
        Err(BeaconVoteValidationError::CheckpointMismatch(...))
    } else {
        Ok(())
    }
}
```

**Why Epoch MFP is Default**:
- At epoch boundaries (slot 0), beacon nodes may disagree on target block root
- Network propagation delays cause temporary view differences
- Requiring full checkpoint match causes systematic liveness issues
- Epoch-only comparison maintains slashing protection while improving liveness

### Slashing Protection

The `BeaconVoteValidator` checks slashing for **all validators in the committee**:

```rust
fn check_attestation_slashing(&self, value: &BeaconVote) -> Result<()> {
    for (validator_pubkey, committee_index) in &self.validator_attestation_committees {
        attestation_data.index = *committee_index;
        slashing_database.preliminary_check_attestation(
            validator_pubkey,
            &attestation_data,
            domain_hash,
        )?;
    }
    Ok(())
}
```

### Timeouts

| Phase | Timeout | Behavior |
|-------|---------|----------|
| Metadata Wait | Until 1/3 slot | Blocks until `SlotMetadata` ready |
| QBFT Consensus | 12 rounds max | `Completed::TimedOut`, returns error |
| Signature Collection | Implicit (collector cleanup) | Cleaned after 1 slot |

---

## Efficiency Analysis

### Per-Committee vs Per-Validator Comparison

For a committee with N validators:

| Metric | Per-Validator | Per-Committee |
|--------|---------------|---------------|
| QBFT Instances | N | **1** |
| Consensus Messages | O(N × M) | **O(M)** where M = operators |
| Partial Sig Messages | O(N × M) | **O(M)** (batched) |
| Latency | O(N) | **O(1)** |

**Example**: Committee with 10 validators, 4 operators
- Per-Validator: 10 QBFT instances × ~12 messages each = 120 consensus messages
- Per-Committee: 1 QBFT instance × ~12 messages = **12 consensus messages** (10× reduction)

---

## Complete Sequence Diagram

```
┌─────────────────────────────────────────────────────────────────────────────────────────────────────────┐
│                           COMPLETE COMMITTEE ATTESTATION LIFECYCLE                                       │
├─────────────────────────────────────────────────────────────────────────────────────────────────────────┤
│                                                                                                          │
│  Time ───────────────────────────────────────────────────────────────────────────────────────────────►  │
│                                                                                                          │
│  Epoch N-1          │        Slot S                                                                      │
│      │              │            │ 0s              4s             8s             12s                    │
│      │ Poll duties  │            │  ├───────────────┼──────────────┼──────────────┤                     │
│      ├──────────────┘            │  │    1/3 slot   │   2/3 slot   │  slot end    │                     │
│      │                           │  │               │              │              │                     │
│      │                           │  ▼               │              │              │                     │
│      │                           │  MetadataService │              │              │                     │
│      │                           │  - Fetch AttestationData                       │                     │
│      │                           │  - Create BeaconVote                           │                     │
│      │                           │  - Collect attesting validators                │                     │
│      │                           │  - Update SlotMetadata                         │                     │
│      │                           │                  │              │              │                     │
│      │                           │                  ▼              │              │                     │
│      │                           │      sign_attestation() called  │              │                     │
│      │                           │      for each validator         │              │                     │
│      │                           │                  │              │              │                     │
│      │                           │                  ▼              │              │                     │
│      │                           │      ┌─────────────────────────┐│              │                     │
│      │                           │      │    QBFT Consensus       ││              │                     │
│      │                           │      │    (ONE instance for    ││              │                     │
│      │                           │      │     entire committee)   ││              │                     │
│      │                           │      │                         ││              │                     │
│      │                           │      │  CommitteeInstanceId {  ││              │                     │
│      │                           │      │    committee_id,        ││              │                     │
│      │                           │      │    slot,                ││              │                     │
│      │                           │      │  }                      ││              │                     │
│      │                           │      │                         ││              │                     │
│      │                           │      │  Data: BeaconVote       ││              │                     │
│      │                           │      └─────────────────────────┘│              │                     │
│      │                           │                  │              │              │                     │
│      │                           │                  ▼              │              │                     │
│      │                           │      ┌─────────────────────────┐│              │                     │
│      │                           │      │ Post-Consensus Signing  ││              │                     │
│      │                           │      │ (BATCHED per operator)  ││              │                     │
│      │                           │      │                         ││              │                     │
│      │                           │      │ Wait for all validators ││              │                     │
│      │                           │      │ in committee to sign    ││              │                     │
│      │                           │      │ → Send ONE message      ││              │                     │
│      │                           │      └─────────────────────────┘│              │                     │
│      │                           │                  │              │              │                     │
│      │                           │                  ▼              │              │                     │
│      │                           │   Lagrange interpolation        │              │                     │
│      │                           │   → Full signatures per validator              │                     │
│      │                           │                  │              │              │                     │
│      │                           │                  ▼              │              │                     │
│      │                           │   Attestations submitted to     │              │                     │
│      │                           │   beacon node                   │              │                     │
│                                                                                                          │
└─────────────────────────────────────────────────────────────────────────────────────────────────────────┘
```

---

## File Reference Summary

| Component | File Path |
|-----------|-----------|
| AnchorValidatorStore (sign_attestation) | `anchor/validator_store/src/lib.rs` |
| MetadataService | `anchor/validator_store/src/metadata_service.rs` |
| QbftManager (beacon_vote_instances) | `anchor/qbft_manager/src/lib.rs` |
| SignatureCollectorManager (committee mode) | `anchor/signature_collector/src/lib.rs` |
| BeaconVote | `anchor/common/ssv_types/src/consensus.rs` |
| BeaconVoteValidator | `anchor/common/ssv_types/src/consensus.rs` |
| Role enum | `anchor/common/ssv_types/src/msgid.rs` |
| CommitteeInstanceId | `anchor/qbft_manager/src/lib.rs` |
| PartialSignatureMessages | `anchor/common/ssv_types/src/partial_sig.rs` |

---

## Summary

The per-committee attestation duty lifecycle in Anchor is designed for **efficiency at scale**:

1. **Single QBFT Instance**: All validators in a committee share one consensus instance per slot
2. **Batched Signatures**: Operators collect all their validators' signatures before sending one message
3. **Shared Metadata**: `BeaconVote` is computed once and reused for attestations AND sync messages
4. **Comprehensive Validation**: `BeaconVoteValidator` checks slashing for all validators in one pass
5. **Graceful Edge Cases**: Majority Fork Protection handles epoch boundary disagreements

This architecture reduces network messages and consensus rounds by O(N) compared to per-validator approaches, making Anchor highly scalable for operators managing many validators.
