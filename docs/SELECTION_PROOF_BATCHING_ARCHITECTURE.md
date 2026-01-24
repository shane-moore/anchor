# Selection Proof Batching Architecture

This document explains how Anchor automatically batches multiple validators' selection proofs (both attestation and sync committee) into a single `PartialSignatureMessages` P2P message.

## Overview

In the Boole fork, all selection proofs from validators in the same committee at the same slot batch together into one network message. This batching happens **automatically** through the existing `SignatureCollector` infrastructure - no additional batching code is needed in Tasks 8 & 9.

**Key Insight:** Both attestation selection proofs AND sync committee selection proofs use the **same** `(base_hash, committee_id)` key, causing the `SignatureCollector` to accumulate them in the same batch.

## Concrete Example: How Batching Works

### Setup
- **Committee** with validators: V1, V2, V3
- **Slot**: 1000
- **Committee ID**: 0xABCD
- **Duties**:
  - V1: Attesting + Sync Committee (subnets [0, 2])
  - V2: Attesting only
  - V3: Attesting only
- **Expected signature count**: 5
  - V1 attestation selection proof
  - V2 attestation selection proof
  - V3 attestation selection proof
  - V1 sync selection proof (subnet 0)
  - V1 sync selection proof (subnet 2)

### Step-by-Step Flow

#### Step 1: V1 Calls `produce_selection_proof` (Attestation)

**File:** `anchor/validator_store/src/lib.rs:1540-1590`

```rust
// V1's attestation selection proof
let batch_id = SelectionProofBatchId::new(slot: 1000, committee_id: 0xABCD);
let base_hash = batch_id.hash(); // → 0x7A8B9C... (deterministic!)

let collection_mode = CollectionMode::Committee {
    num_signatures_to_collect: 5,  // Calculated from voting_assignments
    base_hash: 0x7A8B9C...,
};

self.collect_signature(
    PartialSignatureKind::AggregatorCommitteePartialSig,
    Role::AggregatorCommittee,
    collection_mode,
    &validator_V1,
    &cluster,
    signing_root: 0x1111...,  // hash(slot, DOMAIN_SELECTION)
    slot: 1000,
)
```

**Routing:** `collect_signature` converts `CollectionMode::Committee` to `SignatureRequester::Committee` and passes it to `SignatureCollectorManager`.

#### Step 2: SignatureCollectorManager Processes V1's Signature

**File:** `anchor/signature_collector/src/lib.rs:179-223`

```rust
SignatureRequester::Committee {
    num_signatures_to_collect: 5,
    base_hash: 0x7A8B9C...,
} => {
    // Key: (base_hash: 0x7A8B9C..., committee_id: 0xABCD)
    let mut entry = manager
        .committee_signatures
        .entry((0x7A8B9C..., 0xABCD))  // ← Batching key!
        .or_insert(CommitteeSignatures {
            collected_signatures: Vec::with_capacity(5),
            for_slot: 1000,
        });

    // Create PartialSignatureMessage for V1's attestation
    let message = PartialSignatureMessage {
        slot: 1000,
        validator_index: V1,
        signing_root: 0x1111...,  // Attestation selection signing root
        signature: σ₁¹,
    };

    // Add to batch
    collected_signatures.push(message);

    trace!(have = 1, need = 5, "Checking if we have all signatures");
    // Not enough yet, don't send
}
```

**State after Step 2:**
```rust
committee_signatures[(0x7A8B9C..., 0xABCD)] = CommitteeSignatures {
    collected_signatures: [
        { validator_index: V1, signing_root: 0x1111..., signature: σ₁¹ }
    ],
    for_slot: 1000,
}
```

#### Step 3: V2 Calls `produce_selection_proof` (Attestation)

Same flow as Step 1, but for V2. **Critical:** Computes the **SAME base_hash** because it's the same slot and committee!

```rust
let batch_id = SelectionProofBatchId::new(slot: 1000, committee_id: 0xABCD);
let base_hash = batch_id.hash(); // → 0x7A8B9C... (SAME!)
```

**State after Step 3:**
```rust
committee_signatures[(0x7A8B9C..., 0xABCD)] = CommitteeSignatures {
    collected_signatures: [
        { validator_index: V1, signing_root: 0x1111..., signature: σ₁¹ },
        { validator_index: V2, signing_root: 0x1111..., signature: σ₁² },  // SAME signing_root!
    ],
    for_slot: 1000,
}
```
*Count: 2 of 5 needed*

#### Step 4: V3 Calls `produce_selection_proof` (Attestation)

**State after Step 4:**
```rust
committee_signatures[(0x7A8B9C..., 0xABCD)] = CommitteeSignatures {
    collected_signatures: [
        { validator_index: V1, signing_root: 0x1111..., signature: σ₁¹ },
        { validator_index: V2, signing_root: 0x1111..., signature: σ₁² },
        { validator_index: V3, signing_root: 0x1111..., signature: σ₁³ },
    ],
    for_slot: 1000,
}
```
*Count: 3 of 5 needed*

#### Step 5: V1 Calls `produce_sync_selection_proof` (Subnet 0)

**File:** `anchor/validator_store/src/lib.rs:1660-1720` (Task 9)

```rust
// V1's sync selection proof for subnet 0
let batch_id = SelectionProofBatchId::new(slot: 1000, committee_id: 0xABCD);
let base_hash = batch_id.hash(); // → 0x7A8B9C... (SAME base_hash as attestations!!!)

// But DIFFERENT signing_root (includes subnet)
let signing_root = SyncAggregatorSelectionData {
    slot: 1000,
    subcommittee_index: 0,  // Subnet 0
}.signing_root(domain_hash); // → 0xAABB...

self.collect_signature(
    PartialSignatureKind::AggregatorCommitteePartialSig,  // SAME kind!
    Role::AggregatorCommittee,                            // SAME role!
    CollectionMode::Committee {
        num_signatures_to_collect: 5,  // SAME count!
        base_hash: 0x7A8B9C...,        // SAME hash!
    },
    ...
    signing_root: 0xAABB...,  // Different! (includes subnet 0)
)
```

**State after Step 5:**
```rust
committee_signatures[(0x7A8B9C..., 0xABCD)] = CommitteeSignatures {
    collected_signatures: [
        { validator_index: V1, signing_root: 0x1111..., signature: σ₁¹ },  // Attestation
        { validator_index: V2, signing_root: 0x1111..., signature: σ₁² },  // Attestation
        { validator_index: V3, signing_root: 0x1111..., signature: σ₁³ },  // Attestation
        { validator_index: V1, signing_root: 0xAABB..., signature: σ₁⁴ },  // Sync subnet 0
    ],
    for_slot: 1000,
}
```
*Count: 4 of 5 needed*

#### Step 6: V1 Calls `produce_sync_selection_proof` (Subnet 2) - TRIGGERS SEND!

```rust
let signing_root = SyncAggregatorSelectionData {
    slot: 1000,
    subcommittee_index: 2,  // Subnet 2
}.signing_root(domain_hash); // → 0xCCDD...
```

**State after adding:**
```rust
committee_signatures[(0x7A8B9C..., 0xABCD)] = CommitteeSignatures {
    collected_signatures: [
        { validator_index: V1, signing_root: 0x1111..., signature: σ₁¹ },
        { validator_index: V2, signing_root: 0x1111..., signature: σ₁² },
        { validator_index: V3, signing_root: 0x1111..., signature: σ₁³ },
        { validator_index: V1, signing_root: 0xAABB..., signature: σ₁⁴ },
        { validator_index: V1, signing_root: 0xCCDD..., signature: σ₁⁵ },  // Sync subnet 2
    ],
    for_slot: 1000,
}
```
*Count: 5 of 5 ← **THRESHOLD REACHED!***

**File:** `anchor/signature_collector/src/lib.rs:208-222`

```rust
// Batch complete! Create and send the message
if collected_signatures.len() == num_signatures_to_collect {
    let signatures = entry.remove().collected_signatures;  // Extract all 5!

    manager.message_sender.sign_and_send(
        manager.create_message(
            &metadata,
            signatures,  // ← All 5 signatures!
            &DutyExecutor::Committee(metadata.committee_id),
        ),
        metadata.committee_id,
        None,
    )
}
```

#### Step 7: `create_message` Builds PartialSignatureMessages

**File:** `anchor/signature_collector/src/lib.rs:240-261`

```rust
fn create_message(
    &self,
    metadata: &SignatureMetadata,
    signatures: Vec<PartialSignatureMessage>,  // ← All 5 signatures!
    duty_executor: &DutyExecutor,
) -> UnsignedSSVMessage {
    // THIS IS WHERE PartialSignatureMessages IS CREATED!
    let partial_sig_messages = PartialSignatureMessages {
        kind: metadata.kind,  // AggregatorCommitteePartialSig
        slot: metadata.slot,  // 1000
        messages: signatures.into(),  // ← All 5 messages!
    };

    // Wrap in SSVMessage for P2P transmission
    UnsignedSSVMessage {
        ssv_message: SSVMessage::new(
            MsgType::SSVPartialSignatureMsgType,
            MessageId::new(&self.domain, metadata.role, duty_executor),
            partial_sig_messages.as_ssz_bytes(),  // Serialize to wire format
        ),
        full_data: vec![],
    }
}
```

#### Step 8: Final PartialSignatureMessages Object

The final object sent over the network:

```rust
PartialSignatureMessages {
    kind: AggregatorCommitteePartialSig,  // Type: 6
    slot: 1000,
    messages: [
        // Attestation selection proofs (all have SAME signing_root)
        PartialSignatureMessage {
            slot: 1000,
            validator_index: V1,
            signing_root: 0x1111...,  // hash(slot, DOMAIN_SELECTION)
            signature: σ₁¹,
        },
        PartialSignatureMessage {
            slot: 1000,
            validator_index: V2,
            signing_root: 0x1111...,  // hash(slot, DOMAIN_SELECTION)
            signature: σ₁²,
        },
        PartialSignatureMessage {
            slot: 1000,
            validator_index: V3,
            signing_root: 0x1111...,  // hash(slot, DOMAIN_SELECTION)
            signature: σ₁³,
        },

        // Sync selection proofs (each has DIFFERENT signing_root including subnet)
        PartialSignatureMessage {
            slot: 1000,
            validator_index: V1,
            signing_root: 0xAABB...,  // hash(slot, subnet_0, DOMAIN_SYNC)
            signature: σ₁⁴,
        },
        PartialSignatureMessage {
            slot: 1000,
            validator_index: V1,
            signing_root: 0xCCDD...,  // hash(slot, subnet_2, DOMAIN_SYNC)
            signature: σ₁⁵,
        },
    ]
}
```

**This single object gets serialized to SSZ and sent as ONE P2P message!**

## Infrastructure Components

The batching infrastructure already exists in Anchor. Tasks 8 & 9 leverage this existing mechanism by using the appropriate parameters.

### 1. SignatureCollectorManager (Batching Coordinator)

**File:** `anchor/signature_collector/src/lib.rs`

**Purpose:** Manages signature collection and automatic batching based on `(base_hash, committee_id)` keys.

#### Key Data Structures

**Lines 69-70: committee_signatures Map**
```rust
/// A map from a hash of an underlying decided committee value and committee id to a container
/// for all partial signatures based on that value for the committee.
/// Note that this hash may differ from the actual signing root.
committee_signatures: DashMap<(Hash256, CommitteeId), CommitteeSignatures>,
```

**The key `(Hash256, CommitteeId)` is what enables batching:**
- `Hash256` = `base_hash` from `SelectionProofBatchId::new(slot, committee_id).hash()`
- `CommitteeId` = the committee these validators belong to

**Lines 48-53: CommitteeSignatures Struct**
```rust
/// Outgoing partial signature messages that collected for a committee
/// As soon as the partial signature for every validator in the committee is ready, it is sent.
struct CommitteeSignatures {
    collected_signatures: Vec<PartialSignatureMessage>,
    for_slot: Slot,
}
```

This struct accumulates all partial signatures until the batch is complete.

### 2. Batching Logic

**File:** `anchor/signature_collector/src/lib.rs:179-223`

The `sign_and_collect` method handles committee batching:

1. **Lines 185-194:** Creates or retrieves batch entry using `(base_hash, committee_id)` as key
2. **Line 198:** Adds new signature to the batch: `collected_signatures.push(message.clone())`
3. **Lines 200-204:** Logs current count vs. needed count
4. **Lines 208-222:** When complete (`collected_signatures.len() == num_signatures_to_collect`), creates and sends `PartialSignatureMessages`

**Critical Code:**
```rust
SignatureRequester::Committee { num_signatures_to_collect, base_hash } => {
    // Get or create batch entry using (base_hash, committee_id) key
    let mut entry = match manager
        .committee_signatures
        .entry((base_hash, metadata.committee_id))  // ← This key enables batching!
    {
        Entry::Occupied(occupied) => occupied,
        Entry::Vacant(vacant) => vacant.insert_entry(CommitteeSignatures {
            collected_signatures: Vec::with_capacity(num_signatures_to_collect),
            for_slot: metadata.slot,
        }),
    };
    let collected_signatures = &mut entry.get_mut().collected_signatures;

    // Accumulate this signature in the batch
    collected_signatures.push(message.clone());

    trace!(
        have = collected_signatures.len(),
        need = num_signatures_to_collect,
        "Checking if we have all signatures to send"
    );

    // Send when batch is complete
    if collected_signatures.len() == num_signatures_to_collect {
        let signatures = entry.remove().collected_signatures;

        if let Err(err) = manager.message_sender.sign_and_send(
            manager.create_message(
                &metadata,
                signatures,  // ← All batched signatures!
                &DutyExecutor::Committee(metadata.committee_id),
            ),
            metadata.committee_id,
            None,
        ) {
            error!(?err, "Error sending committee partial signatures");
        }
    }
}
```

### 3. PartialSignatureMessages Construction

**File:** `anchor/signature_collector/src/lib.rs:240-261`

The `create_message` method constructs the final `PartialSignatureMessages` object:

```rust
fn create_message(
    &self,
    metadata: &SignatureMetadata,
    signatures: Vec<PartialSignatureMessage>,  // ← All batched signatures
    duty_executor: &DutyExecutor,
) -> UnsignedSSVMessage {
    // Construct PartialSignatureMessages with all batched signatures
    let partial_sig_messages = PartialSignatureMessages {
        kind: metadata.kind,        // AggregatorCommitteePartialSig
        slot: metadata.slot,        // The slot for this batch
        messages: signatures.into(), // ← Batch of all accumulated signatures
    };

    // Wrap in SSVMessage for P2P transmission
    UnsignedSSVMessage {
        ssv_message: SSVMessage::new(
            MsgType::SSVPartialSignatureMsgType,
            MessageId::new(&self.domain, metadata.role, duty_executor),
            partial_sig_messages.as_ssz_bytes(),  // Serialize to SSZ wire format
        ),
        full_data: vec![],
    }
}
```

## How Tasks 8 & 9 Enable Batching

Tasks 8 (`produce_selection_proof`) and Task 9 (`produce_sync_selection_proof`) enable automatic batching by using **identical batching parameters**. This causes the `SignatureCollector` to accumulate both types of selection proofs in the same batch.

### Shared Parameter 1: Same base_hash

Both tasks compute the same `base_hash` from `(slot, committee_id)`:

**Task 8 - File:** `anchor/validator_store/src/lib.rs:1569-1570`
```rust
let batch_id = SelectionProofBatchId::new(slot, committee_id);
let base_hash = batch_id.hash();
```

**Task 9 - File:** `anchor/validator_store/src/lib.rs:1698-1699`
```rust
let batch_id = SelectionProofBatchId::new(slot, committee_id);
let base_hash = batch_id.hash();  // ← SAME hash for same (slot, committee_id)!
```

**Why this matters:** The `SignatureCollectorManager` uses `(base_hash, committee_id)` as the map key. Since both tasks produce the same hash for the same slot and committee, all selection proofs batch together automatically.

**Implementation:** `SelectionProofBatchId::hash()` at `anchor/common/ssv_types/src/consensus.rs:868-876`
```rust
pub fn hash(&self) -> Hash256 {
    let mut hasher = Sha256::new();
    // Domain separator: SSZ encoding of the partial signature kind
    hasher.update(PartialSignatureKind::AggregatorCommitteePartialSig.as_ssz_bytes());
    hasher.update(&self.slot.as_u64().to_le_bytes());
    hasher.update(&self.committee_id.0);
    Hash256::from_slice(&hasher.finalize())
}
```

This hash is **deterministic** - all operators compute the same value for the same `(slot, committee_id)`.

### Shared Parameter 2: Same num_signatures_to_collect

Both tasks use the same counting method: `selection_proof_count_for_committee()`

**Task 8 - File:** `anchor/validator_store/src/lib.rs:1555-1558`
```rust
let num_signatures_to_collect =
    voting_assignments.selection_proof_count_for_committee(|idx| {
        committee_validator_indices.contains(idx)
    });
```

**Task 9 - File:** `anchor/validator_store/src/lib.rs:1691-1694`
```rust
let num_signatures_to_collect =
    voting_assignments.selection_proof_count_for_committee(|idx| {
        committee_validator_indices.contains(idx)
    });
```

**Why this matters:** The `SignatureCollector` waits until it has collected exactly `num_signatures_to_collect` signatures before sending. Both tasks must agree on this count for the batch to complete.

**Counting method:** `VotingAssignments::selection_proof_count_for_committee()` at `anchor/validator_store/src/lib.rs:798-819`
```rust
/// Counts expected signatures for selection proof collection.
///
/// For each validator in the committee:
/// - `+1` if the validator is attesting
/// - `+N` if the validator is in sync committee (N = number of subnets)
pub fn selection_proof_count_for_committee<F>(&self, is_in_committee: F) -> usize
where
    F: Fn(&ValidatorIndex) -> bool,
{
    let mut count = 0;

    // Count attesting validators: +1 each
    for validator_idx in &self.attesting_validators {
        if is_in_committee(validator_idx) {
            count += 1;
        }
    }

    // Count sync validators: +N each (N = number of subnets)
    for (validator_idx, subnets) in &self.sync_validators_by_subnet {
        if is_in_committee(validator_idx) {
            count += subnets.len();  // ← Multi-subnet validators contribute multiple proofs
        }
    }

    count
}
```

### Shared Parameter 3: Same PartialSignatureKind and Role

Both tasks use the same message type and role in the Boole fork:

**Task 8 - File:** `anchor/validator_store/src/lib.rs:1579-1580`
```rust
PartialSignatureKind::AggregatorCommitteePartialSig,
Role::AggregatorCommittee,
```

**Task 9 - File:** `anchor/validator_store/src/lib.rs:1707-1708`
```rust
PartialSignatureKind::AggregatorCommitteePartialSig,  // SAME!
Role::AggregatorCommittee,                            // SAME!
```

**Why this matters:** The `metadata.kind` and `metadata.role` are stored with the batch and used when constructing the final `PartialSignatureMessages`. Using the same values ensures all selection proofs are identified as the same message type.

### Different Parameter: signing_root

The **only** parameter that differs between attestation and sync selection proofs is the `signing_root`:

**Attestation selection proof (Task 8):**
```rust
let signing_root = slot.signing_root(domain_hash);
// Domain: Domain::SelectionProof
```

**Sync selection proof (Task 9):**
```rust
let signing_root = SyncAggregatorSelectionData {
    slot,
    subcommittee_index: subnet_id.into(),  // Includes subnet!
}.signing_root(domain_hash);
// Domain: Domain::SyncCommitteeSelectionProof
```

**Why different signing_roots are okay:** Each `PartialSignatureMessage` in the batch has its own `signing_root`. The `base_hash` (used for batching correlation) is separate from `signing_root` (used for signature verification). This allows messages with different signing roots to batch together.

## Fork Gating

Both Tasks 8 and 9 implement fork gating to maintain backward compatibility:

**Boole Fork (>= Fork::Boole):** Committee batching
- Uses `PartialSignatureKind::AggregatorCommitteePartialSig`
- Uses `Role::AggregatorCommittee`
- Uses `CollectionMode::Committee` with shared parameters

**Alan Fork (< Fork::Boole):** Single-validator collection
- Task 8: Uses `SelectionProofPartialSig`, `Role::Aggregator`, `CollectionMode::SingleValidator`
- Task 9: Uses `ContributionProofs`, `Role::SyncCommittee`, `CollectionMode::SingleValidator`

## Comparison: Anchor vs Go-SSV

### Go-SSV (Explicit Batching)

**File:** `ssv/protocol/v2/ssv/runner/aggregator_committee.go:1476+`

```go
func (r *AggregatorCommitteeRunner) executeDuty(ctx context.Context, logger *zap.Logger, duty spectypes.Duty) error {
    // Explicitly create the batch message
    msg := &spectypes.PartialSignatureMessages{
        Type:     spectypes.AggregatorCommitteePartialSig,
        Slot:     duty.DutySlot(),
        Messages: []*spectypes.PartialSignatureMessage{},
    }

    // Explicitly loop over all validator duties and build batch
    for _, vDuty := range aggCommitteeDuty.ValidatorDuties {
        switch vDuty.Type {
        case spectypes.BNRoleAggregator:
            partialSig, _ := signBeaconObject(...)
            msg.Messages = append(msg.Messages, partialSig)  // ← Explicit append

        case spectypes.BNRoleSyncCommitteeContribution:
            for _, index := range vDuty.ValidatorSyncCommitteeIndices {
                partialSig, _ := signBeaconObject(...)
                msg.Messages = append(msg.Messages, partialSig)  // ← Explicit append
            }
        }
    }

    // Then send the complete batch
    ssvMsg := &spectypes.SSVMessage{...}
    msgToBroadcast := &spectypes.SignedSSVMessage{...}
    r.BaseRunner.Network.Broadcast(msgID, msgToBroadcast)
}
```

**Characteristics:**
- **Explicit batching:** Code explicitly loops and appends to `msg.Messages`
- **Single entry point:** All batching happens in one `executeDuty()` function
- **Visible batch:** The `PartialSignatureMessages` object is clearly visible in the code

### Anchor (Implicit Batching)

**Files:**
- `anchor/validator_store/src/lib.rs` (Tasks 8 & 9)
- `anchor/signature_collector/src/lib.rs`

```rust
// No explicit batching code! Each validator independently calls:
produce_selection_proof(validator, slot)
produce_sync_selection_proof(validator, slot, subnet)

// SignatureCollector automatically batches using (base_hash, committee_id) key
// When count reaches num_signatures_to_collect, it auto-sends
```

**Characteristics:**
- **Implicit batching:** No explicit loops or batch construction in validator_store code
- **Multiple entry points:** Each validator independently calls its duty functions
- **Hidden batch:** The batch accumulates in `SignatureCollectorManager` automatically

### Why Anchor's Approach Works

Anchor's per-validator API design (`produce_selection_proof(validator_pubkey, slot)`) naturally fits its architecture:
1. Each validator is processed independently by the duties service
2. The `SignatureCollector` provides a centralized batching point
3. The `(base_hash, committee_id)` key enables automatic correlation
4. The batch transparently accumulates until complete

This architecture avoids the need for explicit committee-aware duty construction while achieving the same wire format as Go-SSV.

## Verification

The batching behavior is implicitly verified by existing tests:

### Counting Logic Tests

**File:** `anchor/validator_store/src/lib.rs:2092-2177`

These tests verify the counting logic that determines `num_signatures_to_collect`:

- **`test_selection_proof_count_with_multi_subnet_validators`** - Verifies +N counting for sync validators
- **`test_selection_proof_count_with_filter`** - Verifies filtering to specific validators
- **`test_counting_difference_between_methods`** - Verifies selection proof vs. voting message counting
- **`test_overlapping_attesting_and_sync_validators`** - Verifies multi-duty validators

### SignatureCollector Tests

The `signature_collector` crate has its own tests that verify:
- Signature collection and batching behavior
- Threshold-based sending
- Map-based accumulation

### Integration Verification

**What's tested implicitly:**
1. ✅ `SelectionProofBatchId` produces deterministic hashes (tested in `ssv_types`)
2. ✅ Counting logic correctly computes batch size (tested in `validator_store`)
3. ✅ `SignatureCollector` batches and sends when threshold reached (tested in `signature_collector`)

**What could be added (optional):**

An end-to-end integration test that:
1. Sets up a committee with multiple validators
2. Calls both `produce_selection_proof` and `produce_sync_selection_proof`
3. Verifies a single `PartialSignatureMessages` is sent with all signatures

However, this is **not strictly necessary** because the component tests already verify the behavior, and the architecture guarantees correctness through the shared parameters.

## Summary

**Key Takeaways:**

1. ✅ **No additional batching code needed** - Existing `SignatureCollector` infrastructure handles it
2. ✅ **Tasks 8 & 9 use identical batching parameters** - Same `base_hash`, `num_signatures_to_collect`, `kind`, and `role`
3. ✅ **Automatic correlation via `(base_hash, committee_id)` key** - SignatureCollector batches together
4. ✅ **`PartialSignatureMessages` constructed automatically** - When batch threshold reached
5. ✅ **Wire format matches Go-SSV** - Same message structure and content

**The batching is architectural:**
- Per-validator API design (Anchor) + Centralized batching (SignatureCollector) = Implicit batching
- Committee-aware duty construction (Go-SSV) + Explicit loops = Explicit batching
- Both achieve the same result: One `PartialSignatureMessages` per committee per slot

**No missing implementation steps** - Tasks 8 & 9 complete the pre-consensus selection proof batching feature.
