# AggregatorCommittee Duties Implementation Plan

This plan enables Anchor to support AggregatorCommittee duties per SSV-Spec PR [PR #572](https://github.com/ssvlabs/ssv-spec/pull/572), achieving wire and behavior compatibility with Go SSV [PR #2503](https://github.com/ssvlabs/ssv/pull/2503).

---

## 1. Behavioral Contract (What Must Be True)

### 1.1 Duty Composition

| Behavior | Spec Source | Implementation Source |
|----------|-------------|----------------------|
| AggregatorCommittee combines `BNRoleAggregator` AND `BNRoleSyncCommitteeContribution` duties | `ssv-spec/types/beacon_types.go:150-182` | `ssv/runner/aggregator_committee.go:37-63` |
| All validator duties in the duty must have the same slot | `ssv-spec/types/beacon_types.go:162-165` | `ssv/runner/aggregator_committee.go:97-121` |
| Only aggregator and sync committee contribution roles are allowed | `ssv-spec/types/beacon_types.go:166-169` | Validated in `StartNewDuty()` |

### 1.2 Selection Proof Generation (Pre-Consensus)

| Behavior | Spec Source | Implementation Source |
|----------|-------------|----------------------|
| Aggregator selection proof: Sign `Slot` with `DomainSelectionProof` | `ssv-spec/ssv/aggregator_committee.go:496-569` | `ssv/runner/aggregator_committee.go:1435-1450` |
| Sync selection proof: Sign `SyncAggregatorSelectionData{Slot, SubcommitteeIndex}` with `DomainSyncCommitteeSelectionProof` | `ssv-spec/ssv/aggregator_committee.go:496-569` | `ssv/runner/aggregator_committee.go:1452-1479` |
| Multiple sync proofs per validator (one per assigned subcommittee) | `ssv-spec/ssv/aggregator_committee.go:819-861` | `ssv/runner/aggregator_committee.go:1452-1479` |
| Message type: `AggregatorCommitteePartialSig = 6` | `ssv-spec/types/partial_sig_message.go:8-21` | Used in `executeDuty()` |
| Batch all selection proofs into single `PartialSignatureMessages` | Implicit in spec | `ssv/runner/aggregator_committee.go:1414-1536` |

### 1.3 Aggregator Selection Logic

| Behavior | Spec Source | Implementation Source |
|----------|-------------|----------------------|
| Call `IsAggregator(slot, committeeIndex, committeeLength, slotSig)` for attestation | `ssv-spec/ssv/types.go:45-78` | `ssv/runner/aggregator_committee.go:447` |
| Call `IsSyncCommitteeAggregator(proof)` for sync committee | `ssv-spec/ssv/types.go:45-78` | `ssv/runner/aggregator_committee.go:286` |
| Fetch aggregate attestation via `GetAggregateAttestation()` at 2/3 slot | `ssv-spec/ssv/aggregator_committee.go:80-227` | `ssv/runner/aggregator_committee.go:498` |
| Fetch sync contribution via `GetSyncCommitteeContribution()` | `ssv-spec/ssv/aggregator_committee.go:80-227` | `ssv/runner/aggregator_committee.go:300-304` |
| Early exit if no aggregators/contributors selected | `ssv-spec/ssv/aggregator_committee.go:80-227` | `ssv/runner/aggregator_committee.go:483-488` |

### 1.4 Consensus Data Structure

| Behavior | Spec Source | Implementation Source |
|----------|-------------|----------------------|
| `AggregatorCommitteeConsensusData` contains aggregators, attestations, contributors, contributions | `ssv-spec/types/consensus_data.go:247-500` | `ssv/runner/aggregator_committee.go:366-368` |
| Aggregators max: 3000 validators per committee | `ssv-spec/types/consensus_data.go:247-250` | SSZ max annotation |
| Contributors max: 2048 (512 * 4 subnets) | `ssv-spec/types/consensus_data.go:247-250` | SSZ max annotation |
| `AggregatorsCommitteeIndexes` count == `AggregatedAttestations` count | `ssv-spec/types/consensus_data.go:271-350` | Validation in `CheckValue()` |
| No duplicate committee indexes | `ssv-spec/types/consensus_data.go:271-350` | Validation in `Validate()` |

### 1.5 Post-Consensus Signing

| Behavior | Spec Source | Implementation Source |
|----------|-------------|----------------------|
| For aggregators: Sign `AggregateAndProof` with `DomainAggregateAndProof` | `ssv-spec/ssv/aggregator_committee.go:229-328` | `ssv/runner/aggregator_committee.go:605-621` |
| For contributors: Sign `ContributionAndProof` with `DomainContributionAndProof` | `ssv-spec/ssv/aggregator_committee.go:229-328` | `ssv/runner/aggregator_committee.go:648-659` |
| Message type: `PostConsensusPartialSig = 0` | `ssv-spec/types/partial_sig_message.go:8-21` | `ssv/runner/aggregator_committee.go:667-704` |

### 1.6 Submission Tracking

| Behavior | Spec Source | Implementation Source |
|----------|-------------|----------------------|
| Aggregator: One submission per validator (by validator index) | `ssv-spec/ssv/aggregator_committee.go:330-494` | `ssv/runner/aggregator_committee.go:1010-1062` |
| Sync: Multiple submissions per validator (by validator index + root) | `ssv-spec/ssv/aggregator_committee.go:330-494` | `ssv/runner/aggregator_committee.go:1010-1062` |
| Duty complete when ALL expected submissions done | `ssv-spec/ssv/aggregator_committee.go:1111-1167` | `HasSubmittedAllDuties()` |

### 1.7 Message Routing

| Behavior | Spec Source | Implementation Source |
|----------|-------------|----------------------|
| Role value: `RoleAggregatorCommittee = 6` | `ssv-spec/types/runner_role.go:1-31` | Wire format `[6,0,0,0]` |
| Uses `DutyExecutor::Committee(CommitteeId)` in MessageId | Implicit | Same as `Role::Committee` |
| Max QBFT rounds: 12 | `ssv/message/validation/consensus_validation.go:370` | Same as Committee/Aggregator |

### 1.8 Version Support

| Behavior | Spec Source | Implementation Source |
|----------|-------------|----------------------|
| Support Phase0 through Fulu attestations | `ssv-spec/types/consensus_data.go:362-500` | Version-dependent deserialization |
| Electra+ uses `CommitteeBits` in attestations | `ssv-spec/types/consensus_data.go:362-500` | Fork-aware decoding |

### 1.9 Pre-Consensus Edge Cases (from Go SSV PR #2503 Review)

These edge cases were identified from the Go SSV PR #2503 review discussions and must be handled for correct interoperability.

| Edge Case | Go SSV Handling | Anchor Status | Details |
|-----------|-----------------|---------------|---------|
| **Divergent Validator Sets** | `share == nil` check | ✅ **Addressed by Task 16 + Solution A** | Per-validator API naturally filters; `num_signatures_to_collect` uses decided_data filtered by our shares |
| **Root Quorum Per-Validator** | `HasQuorum(validatorIndex, root)` | ✅ **Handled by Architecture** | Anchor's collector key is `(signing_root, validator_index)` - inherently per-validator |
| **Optimistic Quorum + Fallback** | `FallBackAndVerifyEachSignature()` | ❌ **Missing** | When Lagrange interpolation fails, verify each partial sig and remove invalid ones |
| **Re-check Quorum After Fallback** | Loop with `HasQuorum` check | ❌ **Missing** | After removing invalid sigs, may lose quorum for some (root, validator) pairs |
| **Fallback `roots[i:]` Optimization** | Process `roots[i:]` not all roots | ❌ **Addressed by Task 17** | When fallback verification runs, only verify sigs for unprocessed roots |
| **HasQuorum Before Reconstruction** | Explicit threshold check | ✅ **Handled** | `signature_share.len() >= threshold` checked before `combine_signatures()` |
| **Concurrent Fallback Coordination** | Potential N concurrent calls | ✅ **N/A** | Anchor uses single-task-per-collector design - no concurrency issue |
| **Skip Unknown Validators Gracefully** | `continue` on nil share | ✅ **Addressed by Solution A (Tasks 12/13)** | Lighthouse drives calls; `ValidatorNotInConsensus(ValidatorIndex)` for validators not in decided data |
| **Post-Consensus Count Mismatch** | N/A (drives from decided data) | ✅ **Fixed by Solution A** | `num_signatures_to_collect` uses decided_data filtered by shares, not local aggregation_assignments |

**Reference Links:**
- Divergent validator sets: [PR #2503 discussion_r2658117698](https://github.com/ssvlabs/ssv/pull/2503#discussion_r2658117698)
- Quorum re-check after fallback: [PR #2503 discussion_r2658112575](https://github.com/ssvlabs/ssv/pull/2503#discussion_r2658112575)

---

## 1.10 Anchor Architecture Advantages vs. Go SSV

Anchor's architecture provides inherent handling for several edge cases that require explicit code in Go SSV.

### 1.10.1 Per-(Root, Validator) Collector Design

**Go SSV Challenge:**
```go
// Go SSV iterates over roots, then validators, checking quorum for each combination
for root := range rootSet {
    for _, validator := range validators {
        if !cr.BaseRunner.State.PostConsensusContainer.HasQuorum(validator, root) {
            continue  // Explicit check needed
        }
        // ...
    }
}
```

**Anchor Solution:**
```rust
// Anchor's SignatureCollectorManager uses (signing_root, validator_index) as key
signature_collectors: DashMap<(Hash256, ValidatorIndex), SignatureCollector>
```

Each collector is independently responsible for one (root, validator) pair, so quorum is naturally tracked per-pair without explicit loops.

### 1.10.2 Single-Task-Per-Collector Avoids Concurrent Fallback

**Go SSV Challenge:**
> "We've parallelized this loop for committee-duty... we ended up with the current code that might call `FallBackAndVerifyEachSignature` N times concurrently"

**Anchor Solution:**
Each `SignatureCollector` runs as a single async task, receiving messages via an unbounded channel. No parallel execution means no coordination needed for fallback verification.

### 1.10.3 Threshold Check Before Expensive Operations

**Go SSV Pattern:**
```go
gotQuorum, quorumSigners := r.state().PreConsensusContainer.HasQuorum(validatorIndex, root)
if !gotQuorum {
    continue  // Avoid expensive reconstruction
}
```

**Anchor Pattern:**
```rust
if let Some(threshold) = threshold
    && signature_share.len() as u64 >= threshold
{
    // Only attempt Lagrange interpolation when we have enough shares
    let signature = match combine_signatures(...) { ... };
}
```

Same optimization pattern - no wasted reconstruction attempts.

### 1.10.4 IndexSet Avoids Loop Iteration Bugs

**Go SSV Challenge (commit 4ad0e5b11):**
```go
// Bug: continue hits inner loop instead of outer
for _, selection := range selections {
    for i, idx := range committeeIndexes {
        if idx == selection.Index {
            continue  // Bug: continues inner loop, not selection loop
        }
    }
}
// Fixed with labeled continue:
selectionLoop:
for _, selection := range selections {
    // ... continue selectionLoop
}
```

**Anchor Solution:**
Anchor uses `IndexSet` for deduplication, avoiding nested loops entirely. The set structure inherently prevents duplicates without explicit iteration checks.

### 1.10.5 No Sync Selection Proof Deduplication

**Go SSV Approach (removed in commit 8de107174):**
- Initially had `seenSigs` map to deduplicate by partial signature value
- Removed this deduplication - now just appends all partial signatures

**Anchor Approach:**
Anchor doesn't perform sync selection proof deduplication either. Partial signatures are collected as received, with deduplication happening at the operator ID level in the signature collector.

---

## 2. Mapping Table: SSV-Spec + SSV Implementation → Anchor

### 2.1 Roles and Message Types

| Component | SSV-Spec Location | SSV Implementation | Anchor Target | Changes |
|-----------|------------------|-------------------|---------------|---------|
| Role enum | `types/runner_role.go:6-12` | Same | `ssv_types/src/msgid.rs:13-20` | Add `AggregatorCommittee = 6` |
| PartialSigKind | `types/partial_sig_message.go:8-21` | Same | `ssv_types/src/partial_sig.rs:18-32` | Add `AggregatorCommitteePartialSig = 6` |
| BeaconRole mapping | `types/beacon_types.go:108-122` | Same | N/A (Lighthouse handles) | No change |

### 2.2 Consensus Data Types

| Component | SSV-Spec Location | SSV Implementation | Anchor Target | Changes |
|-----------|------------------|-------------------|---------------|---------|
| AssignedAggregator | `types/consensus_data.go:247-254` | Same | `ssv_types/src/consensus.rs` | **NEW**: Add struct |
| AggregatorCommitteeConsensusData | `types/consensus_data.go:256-270` | Same | `ssv_types/src/consensus.rs` | **NEW**: Add struct with QbftData impl |
| Data validation | `types/consensus_data.go:271-350` | `ssv/value_check.go:87-111` | `ssv_types/src/consensus.rs` | **NEW**: Add QbftDataValidator impl |

### 2.3 Pre-Consensus Phase

| Component | SSV-Spec Location | SSV Implementation | Anchor Target | Changes |
|-----------|------------------|-------------------|---------------|---------|
| Selection proof signing | `ssv/aggregator_committee.go:496-569` | `runner/aggregator_committee.go:1414-1536` | `validator_store/src/lib.rs` | Modify `produce_selection_proof`, `produce_sync_selection_proof` |
| Batch count calculation | Implicit | `runner/aggregator_committee.go:1414-1536` | `validator_store/src/lib.rs` | **NEW**: Add `SelectionProofMetadata` |
| Expected roots calculation | `ssv/aggregator_committee.go:819-861` | Same | `validator_store/src/lib.rs` | **NEW**: Add helper functions |

### 2.4 Consensus Phase (QBFT)

| Component | SSV-Spec Location | SSV Implementation | Anchor Target | Changes |
|-----------|------------------|-------------------|---------------|---------|
| QBFT routing | N/A | `runner/aggregator_committee.go:549-705` | `qbft_manager/src/lib.rs:206-257` | Extend `receive_data()` for new role |
| Instance ID | N/A | Via BaseRunner | `qbft_manager/src/lib.rs` | **NEW**: Add `AggregatorCommitteeInstanceId` |
| Value checker | `types/consensus_data.go:271-350` | `ssv/value_check.go:87-111` | `ssv_types/src/consensus.rs` | **NEW**: Add validator |

### 2.5 Post-Consensus Phase

| Component | SSV-Spec Location | SSV Implementation | Anchor Target | Changes |
|-----------|------------------|-------------------|---------------|---------|
| AggregateAndProof signing | `ssv/aggregator_committee.go:229-328` | `runner/aggregator_committee.go:605-621` | `validator_store/src/lib.rs` | Modify `produce_signed_aggregate_and_proof` |
| ContributionAndProof signing | `ssv/aggregator_committee.go:229-328` | `runner/aggregator_committee.go:648-659` | `validator_store/src/lib.rs` | Modify `produce_signed_contribution_and_proof` |
| Submission tracking | `ssv/aggregator_committee.go:1111-1167` | Same | `validator_store/src/lib.rs` | **NEW**: Add tracker |

### 2.6 Message Validation

| Component | SSV-Spec Location | SSV Implementation | Anchor Target | Changes |
|-----------|------------------|-------------------|---------------|---------|
| Role-kind matching | Implicit | `runner/runner_validations.go:19-52` | `message_validator/src/partial_signature.rs:134-151` | Extend `partial_signature_type_matches_role` |
| Message count limits | Implicit | `runner/runner_validations.go:66-167` | `message_validator/src/partial_signature.rs` | Add count validation |
| Slot validation | `runner/runner_validations.go:45-49` | Same (skip root check) | `message_validator/src/partial_signature.rs` | Handle special case |

---

## 3. Step-by-Step Implementation Plan

### 3.0 Boole Fork Gating Strategy (PR #774 Dependency)

AggregatorCommittee duties are part of the **Boole fork** - an upcoming SSV protocol upgrade. Our implementation must be fork-aware to prevent breaking Anchor when merged before Boole activates.

**Key Infrastructure from PR #774:**
- `Fork` enum: `Alan` (current) | `Boole` (upcoming)
- `ForkSchedule`: Tracks activation epochs, provides `active_fork(epoch)`
- Access: `config.global_config.ssv_network.fork_schedule` → `Arc<ForkSchedule>`

**Fork Gating Pattern:**
```rust
use fork::{Fork, ForkSchedule};

fn should_use_aggregator_committee(fork_schedule: &ForkSchedule, epoch: Epoch) -> bool {
    // Use >= to ensure feature stays enabled for all future forks (Charlie, Delta, etc.)
    fork_schedule.active_fork(epoch) >= Fork::Boole
}
```

**Gating Categories:**

| Category | Tasks | Gating Needed? | Rationale |
|----------|-------|----------------|-----------|
| **Type Definitions** | 1, 2, 3, 4, 5 | ❌ No | Adding new types/enums is safe - they won't be used until behavior triggers them |
| **Internal Structs** | 6, 14 | ❌ No | Internal metadata and state tracking, not wire-visible |
| **Duty Processing** | 7, 8, 9, 12, 13 | ✅ Yes | Must check `active_fork(epoch) >= Fork::Boole` before using AggregatorCommittee path |
| **Message Validation** | 10 | ✅ Yes | Must reject Role 6 messages if `active_fork(epoch) < Fork::Boole` |
| **QBFT Routing** | 11 | ✅ Yes | Must reject Role 6 messages if `active_fork(epoch) < Fork::Boole` |
| **Tests** | 15 | ❌ No | Tests only |

**Two-Layer Gating:**
1. **Entry Point Gate (Duty Triggering)** - Primary: In duties service, check fork before starting AggregatorCommittee duties
2. **Safety Net Gate (Message Validation)** - Secondary: Reject incoming Role 6 messages before Boole

**Implementation Note:** After rebasing on PR #774, inject `Arc<ForkSchedule>` into:
- DutiesService (for entry point gating)
- MessageValidator (for safety net gating)
- QbftManager (for routing gating)

---

### Task 1: Add Role::AggregatorCommittee

**Prerequisites**: None

**Files to modify**:
- `anchor/common/ssv_types/src/msgid.rs`

**Changes**:
```rust
// In Role enum (line 13-20)
pub enum Role {
    Committee,
    Aggregator,
    Proposer,
    SyncCommittee,
    ValidatorRegistration,
    VoluntaryExit,
    AggregatorCommittee,  // ADD: value 6
}

// In From<Role> for [u8; 4] (line 22-32)
Role::AggregatorCommittee => [6, 0, 0, 0],

// In TryFrom<&[u8]> for Role (line 35-48)
[6, 0, 0, 0] => Ok(Role::AggregatorCommittee),

// In max_round() (line 51-59)
Role::AggregatorCommittee => Some(12),

// In duty_executor() (line 125-133) - uses Committee routing
Role::AggregatorCommittee => self.0[24..].try_into().ok().map(DutyExecutor::Committee),
```

**Tests to add**:
- Unit test: Role serialization roundtrip for value 6
- Unit test: MessageId construction with AggregatorCommittee role
- Unit test: max_round returns Some(12)

**Validation checklist**:
- [ ] `[6,0,0,0]` encodes/decodes correctly
- [ ] MessageId uses bytes 24-55 for CommitteeId (not validator pubkey)

**Done when**: `Role::AggregatorCommittee` exists with correct wire encoding, max_round=12, and Committee-style MessageId routing.

**Fork Gating**: ❌ Not needed - Adding enum variant is safe; the variant won't be used until behavior code triggers it.

**ssv-fuzz TODO** (post-development):
- Update `ssv-fuzz/fuzz/fuzz_targets/differential/diff_fuzz_message_id_generation.rs` line 45-46 to include role 6 in valid roles:
  ```rust
  // Change from: let is_valid_role = matches!(role, 0..=5);
  // Change to:   let is_valid_role = matches!(role, 0..=6);
  ```
- This ensures MessageId generation with `Role::AggregatorCommittee` is tested against Go SSV's MessageID generation.

---

### Task 2: Add PartialSignatureKind::AggregatorCommitteePartialSig

**Prerequisites**: None (can parallel with Task 1)

**Files to modify**:
- `anchor/common/ssv_types/src/partial_sig.rs`

**Changes**:
```rust
// In PartialSignatureKind enum (line 18-32)
pub enum PartialSignatureKind {
    PostConsensus = 0,
    RandaoPartialSig = 1,
    SelectionProofPartialSig = 2,
    ContributionProofs = 3,
    ValidatorRegistration = 4,
    VoluntaryExit = 5,
    AggregatorCommitteePartialSig = 6,  // ADD
}

// In TryFrom<u64> (line 35-48)
6 => Ok(PartialSignatureKind::AggregatorCommitteePartialSig),
```

**Tests to add**:
- Unit test: Serialization roundtrip for value 6

**Validation checklist**:
- [ ] Value 6 encodes/decodes correctly as u64 little-endian

**Done when**: `PartialSignatureKind::AggregatorCommitteePartialSig = 6` exists with correct SSZ encoding.

**Fork Gating**: ❌ Not needed - Adding enum variant is safe; the variant won't be used until behavior code triggers it.

**ssv-fuzz TODO** (post-development):
- Already covered by `ssv-fuzz/fuzz/fuzz_targets/standard/custom_ssz.rs` which tests `PartialSignatureKind` SSZ roundtrip via `Arbitrary` derive.
- Verify the new variant `AggregatorCommitteePartialSig` is included when `Arbitrary` generates values. If `Arbitrary` is derived on the enum, it should automatically include new variants.

---

### Task 3: Add AssignedAggregator Struct

**Prerequisites**: None (can parallel with Tasks 1-2)

**Files to modify**:
- `anchor/common/ssv_types/src/consensus.rs`

**Changes**:
```rust
// Add after line 26 (imports)
use types::Signature;

// Add new struct
/// Represents a validator assigned as aggregator with their selection proof.
/// Wire-compatible with Go SSV's AssignedAggregator.
#[derive(Clone, Debug, PartialEq, Encode, Decode, TreeHash)]
pub struct AssignedAggregator {
    /// The validator's beacon chain index
    pub validator_index: ValidatorIndex,
    /// The selection proof signature (96 bytes)
    pub selection_proof: Signature,
    /// For attestation aggregators: the committee index
    /// For sync contributors: the subcommittee index
    pub duty_index: u64,
}
```

**Tests to add**:
- Unit test: SSZ encode/decode roundtrip
- Unit test: Verify SSZ byte layout matches Go (validator_index first, then selection_proof, then duty_index)

**Validation checklist**:
- [ ] Field order matches Go SSV exactly
- [ ] SSZ encoding produces identical bytes for same input

**Done when**: `AssignedAggregator` serializes wire-compatibly with Go SSV.

**Fork Gating**: ❌ Not needed - Adding struct is safe; it's only used within AggregatorCommitteeConsensusData.

**ssv-fuzz TODO** (post-development):
- Create new differential fuzz target: `ssv-fuzz/fuzz/fuzz_targets/differential/diff_fuzz_assigned_aggregator.rs`
- Template from `diff_fuzz_validator_consensus_data_decode_encode.rs`:
  ```rust
  // Test AssignedAggregator encoding/decoding between Go and Rust
  let rust_decode_res = AssignedAggregator::from_ssz_bytes(data);
  let (go_success, go_vec) = assigned_aggregator_decode_encode(data); // New FFI function
  // Compare Rust vs Go encoding
  ```
- Add corresponding Go FFI function in `ssv-fuzz/diff_fuzzing/src/lib.rs`
- This is CRITICAL because struct field ordering must match Go exactly (validator_index, selection_proof, duty_index).

---

### Task 4: Add AggregatorCommitteeConsensusData Struct

**Prerequisites**: Task 3

**Files to modify**:
- `anchor/common/ssv_types/src/consensus.rs`

**Changes**:
```rust
// Add type aliases for SSZ max sizes
pub type MaxAggregators = U3000;        // Max validators per committee
pub type MaxContributors = U2048;        // 512 * 4 subnets
pub type MaxCommitteeIndexes = U64;      // Max attestation committees
pub type MaxSyncContributions = U4;      // SYNC_COMMITTEE_SUBNET_COUNT

/// Consensus data for committee-based aggregator duties.
/// Wire-compatible with Go SSV's AggregatorCommitteeConsensusData.
#[derive(Clone, Debug, PartialEq, Encode, Decode, TreeHash)]
pub struct AggregatorCommitteeConsensusData<E: EthSpec> {
    /// Data version (fork)
    pub version: DataVersion,

    /// Validators selected as attestation aggregators
    pub aggregators: VariableList<AssignedAggregator, MaxAggregators>,

    /// Committee indexes that have aggregated attestations
    pub aggregator_committee_indexes: VariableList<u64, MaxCommitteeIndexes>,

    /// Aggregated attestations as SSZ bytes, one per committee index
    /// Using bytes because attestation type varies by fork
    pub aggregated_attestations: VariableList<VariableList<u8, U131308>, MaxCommitteeIndexes>,

    /// Validators selected as sync committee contributors
    pub contributors: VariableList<AssignedAggregator, MaxContributors>,

    /// Sync committee contributions, one per subcommittee
    pub sync_committee_contributions: VariableList<SyncCommitteeContribution<E>, MaxSyncContributions>,
}

impl<E: EthSpec> QbftData for AggregatorCommitteeConsensusData<E> {
    type Hash = Hash256;

    fn hash(&self) -> Self::Hash {
        let bytes = self.as_ssz_bytes();
        let mut hasher = Sha256::new();
        hasher.update(bytes);
        Hash256::from_slice(&hasher.finalize())
    }
}
```

**Tests to add**:
- Unit test: SSZ encode/decode roundtrip with various field combinations
- Unit test: Empty aggregators/contributors case
- Unit test: Max size limits

**Validation checklist**:
- [ ] Field order matches Go SSV exactly (version, aggregators, committee_indexes, attestations, contributors, contributions)
- [ ] Variable list offsets computed correctly
- [ ] Attestation bytes support all fork versions

**Done when**: `AggregatorCommitteeConsensusData` serializes wire-compatibly with Go SSV.

**Fork Gating**: ❌ Not needed - Adding struct is safe; QBFT will only use it when AggregatorCommittee duties are triggered (requires Boole).

**ssv-fuzz TODO** (post-development):
- Create new differential fuzz target: `ssv-fuzz/fuzz/fuzz_targets/differential/diff_fuzz_aggregator_committee_consensus_data.rs`
- Template from `diff_fuzz_validator_consensus_data_decode_encode.rs`:
  ```rust
  // Test AggregatorCommitteeConsensusData encoding/decoding between Go and Rust
  let rust_decode_res = AggregatorCommitteeConsensusData::<MainnetEthSpec>::from_ssz_bytes(data);
  let (go_success, go_vec) = aggregator_committee_consensus_data_decode_encode(data); // New FFI function
  // Compare Rust vs Go encoding
  ```
- Add corresponding Go FFI function in `ssv-fuzz/diff_fuzzing/src/lib.rs`
- HIGH PRIORITY: This is the QBFT consensus payload - wire format MUST match exactly or consensus fails.
- Test with various combinations: empty aggregators, empty contributors, max sizes, different fork versions.

---

### Task 5: Add AggregatorCommitteeConsensusData Validation

**Prerequisites**: Task 4

**Files to modify**:
- `anchor/common/ssv_types/src/consensus.rs`

**Design Decision**: Match Go SSV's simpler `CheckValue()` approach exactly.

Go SSV's `aggregatorCommitteeChecker.CheckValue()` only does:
1. Decode the data
2. Call structural `Validate()`
3. Check non-empty (at least one aggregator or contributor)

It does **NOT** check expected validator indices - that would require pre-consensus state that isn't naturally available at validation time, and QBFT consensus handles disagreements through voting.

**Changes**:
```rust
/// Validation errors for AggregatorCommitteeConsensusData
#[derive(Error, Debug)]
pub enum AggregatorCommitteeValidationError {
    #[error("Aggregator committee indexes count ({indexes}) != attestations count ({attestations})")]
    CommitteeIndexCountMismatch { indexes: usize, attestations: usize },
    #[error("Duplicate committee index: {0}")]
    DuplicateCommitteeIndex(u64),
    #[error("Aggregator committee index {0} not in committee indexes list")]
    AggregatorCommitteeIndexMissing(u64),
    #[error("Unused committee index: {0}")]
    UnusedCommitteeIndex(u64),
    #[error("Duplicate sync subcommittee index: {0}")]
    DuplicateSyncSubcommittee(u64),
    #[error("Contributor subcommittee {0} not in contributions list")]
    ContributorSubcommitteeMissing(u64),
    #[error("Unused sync subcommittee: {0}")]
    UnusedSyncSubcommittee(u64),
    #[error("No validators assigned")]
    NoValidatorsAssigned,
}

/// Validator for AggregatorCommitteeConsensusData during QBFT consensus.
/// Matches Go SSV's CheckValue() - structural validation only, no expected indices check.
pub struct AggregatorCommitteeDataValidator<E: EthSpec> {
    _phantom: PhantomData<E>,
}

impl<E: EthSpec> QbftDataValidator<AggregatorCommitteeConsensusData<E>>
    for AggregatorCommitteeDataValidator<E>
{
    fn validate(
        &self,
        value: &AggregatorCommitteeConsensusData<E>,
        _our_value: &AggregatorCommitteeConsensusData<E>,
    ) -> bool {
        match self.do_validation(value) {
            Ok(_) => true,
            Err(err) => {
                warn!(%err, "Operator proposed invalid aggregator committee consensus data");
                false
            }
        }
    }
}

impl<E: EthSpec> AggregatorCommitteeDataValidator<E> {
    pub fn new() -> Self {
        Self { _phantom: PhantomData }
    }

    /// Structural validation matching Go SSV's Validate() + CheckValue().
    pub fn do_validation(
        &self,
        value: &AggregatorCommitteeConsensusData<E>,
    ) -> Result<(), AggregatorCommitteeValidationError> {
        // Must have at least one aggregator or contributor
        if value.aggregators.is_empty() && value.contributors.is_empty() {
            return Err(AggregatorCommitteeValidationError::NoValidatorsAssigned);
        }

        // Committee indexes count must match attestations count
        if value.aggregator_committee_indexes.len() != value.aggregated_attestations.len() {
            return Err(AggregatorCommitteeValidationError::CommitteeIndexCountMismatch {
                indexes: value.aggregator_committee_indexes.len(),
                attestations: value.aggregated_attestations.len(),
            });
        }

        // No duplicate committee indexes
        let mut seen_committee_indexes = HashSet::new();
        for &idx in value.aggregator_committee_indexes.iter() {
            if !seen_committee_indexes.insert(idx) {
                return Err(AggregatorCommitteeValidationError::DuplicateCommitteeIndex(idx));
            }
        }

        // Every aggregator's committee index must be in the list
        for agg in value.aggregators.iter() {
            if !seen_committee_indexes.contains(&agg.duty_index) {
                return Err(AggregatorCommitteeValidationError::AggregatorCommitteeIndexMissing(
                    agg.duty_index,
                ));
            }
        }

        // Every committee index must be used by at least one aggregator
        let used_committee_indexes: HashSet<_> =
            value.aggregators.iter().map(|a| a.duty_index).collect();
        for &idx in value.aggregator_committee_indexes.iter() {
            if !used_committee_indexes.contains(&idx) {
                return Err(AggregatorCommitteeValidationError::UnusedCommitteeIndex(idx));
            }
        }

        // Sync committee validation: no duplicate subcommittee indexes in contributions
        let mut seen_subcommittee_indexes = HashSet::new();
        for contrib in value.sync_committee_contributions.iter() {
            if !seen_subcommittee_indexes.insert(contrib.subcommittee_index) {
                return Err(AggregatorCommitteeValidationError::DuplicateSyncSubcommittee(
                    contrib.subcommittee_index,
                ));
            }
        }

        // Every contributor's subcommittee must have a contribution
        for contributor in value.contributors.iter() {
            if !seen_subcommittee_indexes.contains(&contributor.duty_index) {
                return Err(AggregatorCommitteeValidationError::ContributorSubcommitteeMissing(
                    contributor.duty_index,
                ));
            }
        }

        // Every subcommittee with a contribution must have at least one contributor
        let used_subcommittee_indexes: HashSet<_> =
            value.contributors.iter().map(|c| c.duty_index).collect();
        for contrib in value.sync_committee_contributions.iter() {
            if !used_subcommittee_indexes.contains(&contrib.subcommittee_index) {
                return Err(AggregatorCommitteeValidationError::UnusedSyncSubcommittee(
                    contrib.subcommittee_index,
                ));
            }
        }

        Ok(())
    }
}
```

**Tests to add**:
- Unit test: Valid data passes `do_validation`
- Unit test: Empty data fails with `NoValidatorsAssigned`
- Unit test: Mismatched committee index/attestation counts fail
- Unit test: Duplicate committee indexes fail
- Unit test: Unused committee indexes fail
- Unit test: Duplicate sync subcommittee indexes fail
- Unit test: Contributor referencing missing subcommittee fails
- Unit test: Unused sync subcommittee fails
- Unit test: `QbftDataValidator::validate()` returns bool correctly

**Validation checklist**:
- [ ] Matches Go SSV's CheckValue() - structural validation only
- [ ] No expected indices validation (QBFT handles disagreement)
- [ ] All structural validation rules from Go's Validate() implemented
- [ ] Error types have clear messages
- [ ] `do_validation` returns `Result`, `validate` trait returns `bool` with logging

**Done when**: Validation logic matches Go SSV's CheckValue() exactly.

**Fork Gating**: ❌ Not needed - Validation only runs when AggregatorCommitteeConsensusData is used (requires Boole).

**ssv-fuzz TODO** (post-development):
- Lower priority: Validation logic may acceptably differ between implementations (Rust may be stricter).
- If desired, add validation result comparison to `diff_fuzz_aggregator_committee_consensus_data.rs` - but focus on encoding first.

---

## Tasks 6: MetadataService Refactoring (Standalone PR - No Fork Gating) - ✅ MOSTLY COMPLETE

**Status**: This section has been implemented with the three-phase architecture described below. The actual implementation uses different struct names than originally proposed.

This section refactors the metadata infrastructure to:
1. Cache voting assignments at slot start (Phase 1)
2. Reuse cached assignments at 1/3 slot with beacon vote (Phase 2)
3. Cache aggregation assignments at 2/3 slot after selection proofs computed (Phase 3)
4. Support both counting patterns needed for committee flows

### Implemented Architecture (Three Phases)

| Phase | Timing | Struct | Purpose |
|-------|--------|--------|---------|
| 1 | Slot start | `VotingAssignments` | Cache voting validators and their duties |
| 2 | 1/3 slot | `VotingContext` | Wrap cached assignments + beacon_vote from beacon node |
| 3 | 2/3 slot | `AggregationAssignments<E>` | Cache aggregators after selection proofs computed |

### Naming Correspondence (Plan → Implementation)

| Original Plan Name | Actual Implementation Name | Location |
|-------------------|---------------------------|----------|
| `ValidatorDutyInfo` | `VotingAssignments` | `validator_store/src/lib.rs:780-863` |
| `SlotMetadata` | `VotingContext` | `validator_store/src/lib.rs:766-771` |
| *(not in plan)* | `AggregationAssignments<E>` | `validator_store/src/lib.rs:876-916` |
| `get_validator_duty_info()` | `get_voting_assignments()` | `validator_store/src/lib.rs:472-492` |
| `update_validator_duty_info()` | `update_voting_assignments()` | `validator_store/src/lib.rs:497-500` |
| `get_slot_metadata()` | `get_voting_context()` | `validator_store/src/lib.rs:439-461` |
| `update_slot_metadata()` | `update_voting_context()` | `validator_store/src/lib.rs:463-466` |
| *(not in plan)* | `get_aggregation_assignments()` | `validator_store/src/lib.rs:509-533` |
| *(not in plan)* | `update_aggregation_assignments()` | `validator_store/src/lib.rs:540-543` |

### Understanding: Two Different Counting Patterns

| Use Case | Counting | Why |
|----------|----------|-----|
| Committee messages (attestation + sync) | `+1` per sync validator | One message per validator (beacon node routes to subnets) |
| Selection proofs (aggregator committee) | `+N` per sync validator | One proof per (validator, subnet) pair (different signing roots) |

**Solution**: `VotingAssignments` caches granular `sync_validators_by_subnet`, provides both counting methods.

---

### Task 6a: Add VotingAssignments Struct - ✅ COMPLETE

**Status**: Implemented as `VotingAssignments` at `validator_store/src/lib.rs:780-863`

**Actual Implementation**:
```rust
/// Cached validator voting assignments for a slot.
/// Supports two different counting patterns:
/// 1. Committee messages (attestation + sync): +1 per sync validator
/// 2. Selection proofs (aggregator committee): +N per sync validator (N = subnets)
#[derive(Debug, Clone)]
pub struct VotingAssignments {
    pub slot: Slot,
    pub attesting_validators: Vec<ValidatorIndex>,
    pub attesting_committees: HashMap<PublicKeyBytes, u64>,
    pub sync_validators_by_subnet: HashMap<ValidatorIndex, HashSet<SyncSubnetId>>,
}

impl VotingAssignments {
    pub fn sync_validators(&self) -> Vec<ValidatorIndex> { ... }
    pub fn selection_proof_count_for_committee<F>(&self, is_in_committee: F) -> usize { ... }
    pub fn committee_message_count_for_committee<F>(&self, is_in_committee: F) -> usize { ... }
}
```

**Key difference from plan**: Uses closure `is_in_committee: F` instead of database lookup, making it more flexible and avoiding tight coupling to database state.

**Tests**: Added in `validator_store/src/lib.rs:1984-2105`

**Fork Gating**: ❌ Not needed - Pure struct addition, no behavioral change.

---

### Task 6b: Add Watch Channel Infrastructure for VotingAssignments - ✅ COMPLETE

**Status**: Implemented with THREE watch channels in `AnchorValidatorStore`:
- `voting_assignments_tx` (Phase 1 - slot start)
- `voting_context_tx` (Phase 2 - 1/3 slot)
- `aggregation_assignments_tx` (Phase 3 - 2/3 slot)

**Actual Implementation** (in `validator_store/src/lib.rs`):
```rust
pub struct AnchorValidatorStore<T: SlotClock + 'static, E: EthSpec> {
    voting_context_tx: watch::Sender<Option<Arc<VotingContext>>>,
    voting_assignments_tx: watch::Sender<Option<Arc<VotingAssignments>>>,
    aggregation_assignments_tx: watch::Sender<Option<Arc<AggregationAssignments<E>>>>,
    // ...
}

// Methods at lines 472-543:
pub async fn get_voting_assignments(&self, slot: Slot) -> Result<Arc<VotingAssignments>, Error>
pub fn update_voting_assignments(&self, voting_assignments: VotingAssignments)
pub async fn get_aggregation_assignments(&self, slot: Slot) -> Result<Arc<AggregationAssignments<E>>, Error>
pub fn update_aggregation_assignments(&self, info: AggregationAssignments<E>)
```

**Error variants added**: `MetadataSlotPassed`, `MetadataChannelClosed`, `AggregatorInfoSlotPassed`, `AggregatorInfoChannelClosed`

**Tests**: Added in `validator_store/src/lib.rs:2107-2173`

**Fork Gating**: ❌ Not needed - Infrastructure only, no behavioral change yet.

---

### Task 6c: Extract Duty Info Builder Helper - ✅ COMPLETE

**Status**: Implemented as `update_voting_assignments()` in `metadata_service.rs:150-211`

**Actual Implementation** (in `metadata_service.rs`):
```rust
/// Phase 1: Build and publish VotingAssignments at slot start.
fn update_voting_assignments(&self) -> Result<(), String> {
    let slot = self.slot_clock.now().ok_or("Failed to read slot clock")?;

    // Get attestation validators
    let (attesting_validators, attesting_committees) = self
        .duties_service
        .attesters(slot)
        .into_iter()
        .map(|duty| {
            (
                ValidatorIndex(duty.duty.validator_index as usize),
                (duty.duty.pubkey, duty.duty.committee_index),
            )
        })
        .unzip();

    // Get sync validators by subnet
    let sync_validators_by_subnet = self
        .duties_service
        .sync_duties
        .get_duties_for_slot::<E>(slot, &self.spec)
        // ... (extracts validators by subnet)

    let voting_assignments = VotingAssignments {
        slot,
        attesting_validators,
        attesting_committees,
        sync_validators_by_subnet,
    };

    self.validator_store.update_voting_assignments(voting_assignments);
    Ok(())
}
```

**Note**: Multi-sync aggregator counts moved to `update_aggregation_assignments()` (Phase 3) since they're only needed at 2/3 slot.

**Fork Gating**: ❌ Not needed - Internal helper function.

---

### Task 6d: Refactor to VotingContext wrapping VotingAssignments - ✅ COMPLETE

**Status**: Implemented as `VotingContext` at `validator_store/src/lib.rs:766-771`

**Actual Implementation**:
```rust
struct VotingContext {
    /// Cached voting assignments (computed at slot start, reused here)
    voting_assignments: Arc<VotingAssignments>,
    /// The BeaconVote (only available at 1/3 slot from beacon node)
    beacon_vote: BeaconVote,
}
```

**Note**: Multi-sync aggregators moved to `AggregationAssignments<E>` (Phase 3), since they're only needed at 2/3 slot:
```rust
pub struct AggregationAssignments<E: EthSpec> {
    pub slot: Slot,
    pub aggregating_attesters: HashSet<ValidatorIndex>,
    pub aggregator_committees: HashMap<PublicKeyBytes, u64>,
    pub sync_aggregators_by_subnet: HashMap<ValidatorIndex, HashSet<SyncSubnetId>>,
    multi_sync_aggregators: HashMap<PublicKeyBytes, ContributionWaiter<E>>,
}
```

**CollectionMode::Committee update** (at `validator_store/src/lib.rs:236-260`):
```rust
CollectionMode::Committee { voting_context_tx, base_hash } => {
    let num_signatures_to_collect = voting_context_tx
        .voting_assignments
        .committee_message_count_for_committee(|idx| {
            committee_validator_indices.contains(idx)
        });
    // ...
}
```

**Fork Gating**: ❌ Not needed - Internal refactoring, same external behavior.

---

### Task 6e: Modify MetadataService for Three-Phase Operation - ✅ COMPLETE

**Status**: Implemented at `metadata_service.rs:50-147` with THREE phases

**Actual Implementation** (in `metadata_service.rs`):
```rust
impl<E: EthSpec, T: SlotClock + 'static> MetadataService<E, T> {
    pub fn start_update_service(self) -> Result<(), String> {
        // ═══════════════════════════════════════════════════════════════════════
        // PHASE 1: VotingAssignments (slot start)
        // Caches voting assignments for use by both selection proofs AND voting context.
        // ═══════════════════════════════════════════════════════════════════════
        executor.spawn(async move {
            loop {
                // Sleep until slot start
                sleep(duration_to_next_slot).await;
                self_clone.update_voting_assignments()?;
            }
        }, "voting_assignments_service");

        // ═══════════════════════════════════════════════════════════════════════
        // PHASE 2: VotingContext (1/3 slot)
        // Gets cached voting assignments, fetches beacon_vote, builds VotingContext.
        // ═══════════════════════════════════════════════════════════════════════
        executor.spawn(async move {
            loop {
                // Sleep until 1/3 into slot
                sleep(duration_to_next_slot + slot_duration / 3).await;
                self_clone.update_voting_context().await?;
            }
        }, "voting_context_service");

        // ═══════════════════════════════════════════════════════════════════════
        // PHASE 3: AggregationAssignments (2/3 slot)
        // Re-fetches duties after selection proofs computed, builds AggregationAssignments.
        // ═══════════════════════════════════════════════════════════════════════
        executor.spawn(async move {
            loop {
                // Sleep until 2/3 into slot
                sleep(duration_to_next_slot + slot_duration * 2 / 3).await;
                self_clone.update_aggregation_assignments()?;
            }
        }, "aggregation_assignments_service");

        Ok(())
    }
}
```

**Key differences from original plan**:
1. **Three phases instead of two**: Added Phase 3 at 2/3 slot for aggregation assignments
2. **Method naming**: `update_voting_assignments()`, `update_voting_context()`, `update_aggregation_assignments()`
3. **Struct naming**: `VotingAssignments`, `VotingContext`, `AggregationAssignments`

**Fork Gating**: ❌ Not needed - Optimization/refactoring only, same external behavior.

---

### Task 6f: Add SelectionProofBatchId for Batch Hash - ✅ COMPLETE

**Status**: Implemented as `SelectionProofBatchId` struct in `ssv_types/src/consensus.rs:846-880`

**Prerequisites**: Task 6b (complete)

**Actual Implementation**:

Instead of a function on `AnchorValidatorStore`, we created a struct in `consensus.rs` (consistent with `BeaconVote::hash()` pattern):

```rust
// In ssv_types/src/consensus.rs

/// Identifies a batch of pre-consensus selection proofs for a committee.
#[derive(Debug, Clone)]
pub struct SelectionProofBatchId {
    pub slot: Slot,
    pub committee_id: CommitteeId,
}

impl SelectionProofBatchId {
    pub fn new(slot: Slot, committee_id: CommitteeId) -> Self {
        Self { slot, committee_id }
    }

    /// Compute deterministic hash for batching correlation.
    /// Uses SSZ encoding of PartialSignatureKind::AggregatorCommitteePartialSig as domain separator.
    pub fn hash(&self) -> Hash256 {
        let mut hasher = Sha256::new();
        hasher.update(PartialSignatureKind::AggregatorCommitteePartialSig.as_ssz_bytes());
        hasher.update(&self.slot.as_u64().to_le_bytes());
        hasher.update(&self.committee_id.0);
        Hash256::from_slice(&hasher.finalize())
    }
}
```

**Usage** (in Task 8/9):
```rust
// Before (removed):
let base_hash = self.compute_selection_proof_batch_hash(slot, committee_id);

// After:
let batch_id = SelectionProofBatchId::new(slot, committee_id);
let base_hash = batch_id.hash();
```

**Why this approach**:
- Consistent with `BeaconVote::hash()` pattern in same file
- Hash logic lives in `consensus.rs` with other SSV types
- Uses `PartialSignatureKind::AggregatorCommitteePartialSig` SSZ encoding as domain separator (ties hash to message type)

**Tests to add**:
- Unit test: Same inputs produce same hash
- Unit test: Different slots produce different hashes
- Unit test: Different committees produce different hashes

**Done when**: Helper function exists and is tested.

**Fork Gating**: ❌ Not needed - Helper function only used by AggregatorCommittee flow.

---

## End of Refactoring Tasks (6a-6f) - ✅ COMPLETE

**Status**: All Tasks 6a-6f are COMPLETE.

**Completed refactoring** provides:
- Three-phase metadata service: `VotingAssignments` → `VotingContext` → `AggregationAssignments`
- Eliminates redundant duty computation (fetched once at slot start, reused at 1/3 slot)
- Adds infrastructure needed for AggregatorCommittee without changing behavior
- All existing tests pass
- No fork gating needed

---

### Task 8: Modify produce_selection_proof for Committee Batching

**Prerequisites**: Tasks 1, 2, 6a-6f (refactoring PR complete)

**Divergent Views**: See **Task 16** for how divergent operator views are handled in pre-consensus.

**Files to modify**:
- `anchor/validator_store/src/lib.rs`

**Required imports**:
```rust
use ssv_types::consensus::SelectionProofBatchId;
```

**Changes** (updated to use actual struct names):
```rust
async fn produce_selection_proof(
    &self,
    validator_pubkey: PublicKeyBytes,
    slot: Slot,
) -> Result<SelectionProof, Error> {
    let future = async {
        let epoch = slot.epoch(E::slots_per_epoch());
        let domain_hash = self.get_domain(epoch, Domain::SelectionProof);
        let signing_root = slot.signing_root(domain_hash);
        let (validator, cluster) = self.get_validator_and_cluster(validator_pubkey)?;
        let committee_id = cluster.committee_id();

        // Get voting assignments (waits until available - cached at slot start)
        let voting_assignments = self.get_voting_assignments(slot).await?;

        // Calculate batch count using selection proof counting (+N per sync validator)
        // Note: Uses closure pattern instead of database state
        let committee_validator_indices: HashSet<ValidatorIndex> = /* get from state */;
        let num_signatures_to_collect = voting_assignments
            .selection_proof_count_for_committee(|idx| committee_validator_indices.contains(idx));

        // Compute deterministic base_hash for batching (uses SelectionProofBatchId from consensus.rs)
        let batch_id = SelectionProofBatchId::new(slot, committee_id);
        let base_hash = batch_id.hash();

        let collection_mode = CollectionMode::Committee {
            num_signatures_to_collect,
            base_hash,
        };

        // Timeout at 2/3 slot (aggregation deadline)
        let delay = Duration::from_secs(self.spec.seconds_per_slot) * 2 / 3;

        let signature = self
            .timeout_within_slot(
                slot,
                delay,
                self.collect_signature(
                    PartialSignatureKind::AggregatorCommitteePartialSig,  // NEW kind
                    Role::AggregatorCommittee,                            // NEW role
                    collection_mode,
                    &validator,
                    &cluster,
                    signing_root,
                    slot,
                ),
            )
            .await?;
        Ok(signature.into())
    };

    run_and_update_metrics(
        SELECTION_PROOF_LOG_NAME,
        &validator_metrics::SIGNED_SELECTION_PROOFS_TOTAL,
        future,
    )
    .await
}
```

**Tests to add**:
- Unit test: Selection proof uses CollectionMode::Committee
- Unit test: Same base_hash for all validators in committee
- Integration test: Multiple validators' proofs batch into one message

**Validation checklist**:
- [ ] Uses `PartialSignatureKind::AggregatorCommitteePartialSig`
- [ ] Uses `Role::AggregatorCommittee`
- [ ] Uses `CollectionMode::Committee` with correct count
- [ ] base_hash is deterministic (same across operators)
- [ ] **Divergent views**: `num_signatures_to_collect` counts only validators we have shares for
- [ ] **Divergent views**: Batched message only contains partial sigs for validators we have shares for
- [ ] **Divergent views**: Receiving batched messages with extra validators (unknown to us) works correctly - sigs stored but not used

**Done when**: Attestation selection proofs batch via CollectionMode::Committee.

**Fork Gating**: ✅ REQUIRED
```rust
// In produce_selection_proof, check fork before using AggregatorCommittee path
let epoch = slot.epoch(E::slots_per_epoch());
if fork_schedule.active_fork(epoch) >= Fork::Boole {
    // Use AggregatorCommittee path (Tasks 8 changes) - Boole and all future forks
    let collection_mode = CollectionMode::Committee { ... };
    self.collect_signature(
        PartialSignatureKind::AggregatorCommitteePartialSig,
        Role::AggregatorCommittee,
        collection_mode,
        ...
    )
} else {
    // Use existing Aggregator path (unchanged from current behavior) - Alan only
    self.collect_signature(
        PartialSignatureKind::SelectionProofPartialSig,
        Role::Aggregator,
        CollectionMode::SingleValidator,
        ...
    )
}
```
- Inject `Arc<ForkSchedule>` into AnchorValidatorStore
- Existing behavior preserved during Alan
- New batching behavior for Boole and all future forks

**ssv-fuzz TODO**: Not applicable - internal Anchor signature collection logic. Wire format of resulting PartialSignatureMessages is already tested by existing ssv-fuzz targets.

---

### Task 9: Modify produce_sync_selection_proof for Committee Batching

**Prerequisites**: Tasks 1, 2, 6, 8

**Divergent Views**: See **Task 16** for how divergent operator views are handled in pre-consensus.

**Files to modify**:
- `anchor/validator_store/src/lib.rs`

**Required imports** (same as Task 8):
```rust
use ssv_types::consensus::SelectionProofBatchId;
```

**Changes** (updated to use actual struct names):
```rust
async fn produce_sync_selection_proof(
    &self,
    validator_pubkey: &PublicKeyBytes,
    slot: Slot,
    subnet_id: SyncSubnetId,
) -> Result<SyncSelectionProof, Error> {
    let future = async {
        let epoch = slot.epoch(E::slots_per_epoch());
        let (validator, cluster) = self.get_validator_and_cluster(*validator_pubkey)?;
        let committee_id = cluster.committee_id();

        // Sync selection proof signing root includes subnet
        let domain_hash = self.get_domain(epoch, Domain::SyncCommitteeSelectionProof);
        let signing_root = SyncAggregatorSelectionData {
            slot,
            subcommittee_index: subnet_id.into()
        }.signing_root(domain_hash);

        // SAME voting_assignments and base_hash as attestation selection proofs
        // This ensures both types batch into ONE message
        let voting_assignments = self.get_voting_assignments(slot).await?;
        let committee_validator_indices: HashSet<ValidatorIndex> = /* get from state */;
        let num_signatures_to_collect = voting_assignments
            .selection_proof_count_for_committee(|idx| committee_validator_indices.contains(idx));

        // Compute deterministic base_hash for batching (uses SelectionProofBatchId from consensus.rs)
        let batch_id = SelectionProofBatchId::new(slot, committee_id);
        let base_hash = batch_id.hash();

        let collection_mode = CollectionMode::Committee {
            num_signatures_to_collect,
            base_hash,
        };

        let delay = Duration::from_secs(self.spec.seconds_per_slot) * 2 / 3;

        let signature = self
            .timeout_within_slot(
                slot,
                delay,
                self.collect_signature(
                    PartialSignatureKind::AggregatorCommitteePartialSig,  // Same as attestation!
                    Role::AggregatorCommittee,
                    collection_mode,
                    &validator,
                    &cluster,
                    signing_root,
                    slot,
                ),
            )
            .await?;

        Ok(signature.into())
    };

    run_and_update_metrics(
        SYNC_SELECTION_PROOF_LOG_NAME,
        &validator_metrics::SIGNED_SYNC_SELECTION_PROOFS_TOTAL,
        future,
    )
    .await
}
```

**Tests to add**:
- Unit test: Sync selection proof uses same base_hash as attestation
- Integration test: Attestation + sync proofs combine into single message

**Validation checklist**:
- [ ] Uses same `base_hash` as attestation selection proofs
- [ ] Same `num_signatures_to_collect` (includes both types)
- [ ] Different `signing_root` (includes subcommittee_index)
- [ ] **Divergent views**: Same handling as Task 8 - only sign for validators with shares, store received sigs blindly

**Done when**: Sync selection proofs batch together with attestation selection proofs.

**Fork Gating**: ✅ REQUIRED - Same pattern as Task 8
```rust
// In produce_sync_selection_proof, check fork before using AggregatorCommittee path
let epoch = slot.epoch(E::slots_per_epoch());
if fork_schedule.active_fork(epoch) >= Fork::Boole {
    // Use AggregatorCommittee path - Boole and all future forks
} else {
    // Use existing SyncCommittee path (unchanged) - Alan only
}
```

**ssv-fuzz TODO**: Not applicable - internal Anchor signature collection logic, same as Task 8.

---

### Task 10: Update Message Validation for AggregatorCommittee

**Prerequisites**: Tasks 1, 2

**Files to modify**:
- `anchor/message_validator/src/partial_signature.rs`

**Changes**:
```rust
// In partial_signature_type_matches_role (line 134-151)
fn partial_signature_type_matches_role(kind: PartialSignatureKind, role: Role) -> bool {
    match role {
        Role::Committee => kind == PartialSignatureKind::PostConsensus,
        Role::Aggregator => {
            kind == PartialSignatureKind::PostConsensus
                || kind == PartialSignatureKind::SelectionProofPartialSig
        }
        Role::Proposer => {
            kind == PartialSignatureKind::PostConsensus
                || kind == PartialSignatureKind::RandaoPartialSig
        }
        Role::SyncCommittee => {
            kind == PartialSignatureKind::PostConsensus
                || kind == PartialSignatureKind::ContributionProofs
        }
        Role::ValidatorRegistration => kind == PartialSignatureKind::ValidatorRegistration,
        Role::VoluntaryExit => kind == PartialSignatureKind::VoluntaryExit,
        // NEW: AggregatorCommittee accepts combined pre-consensus and post-consensus
        Role::AggregatorCommittee => {
            kind == PartialSignatureKind::PostConsensus
                || kind == PartialSignatureKind::AggregatorCommitteePartialSig
        }
    }
}

// Add message count validation
// Formula: min(5*V, V + 4*SYNC_COMMITTEE_SIZE)
// Where:
// - V = number of validators in the committee
// - SYNC_COMMITTEE_SIZE = 512 (Ethereum sync committee size)
//
// The formula uses min() because:
// - For small committees (V ≤ 512): Limit is 5*V (all validators do everything)
// - For large committees (V > 512): Limit is V + 2048 (sync committee size cap)
//
// Pre-consensus: V attestation proofs + up to 2048 sync proofs (4 subnets * 512 validators)
// Post-consensus: V aggregator sigs + up to 2048 contributor sigs (4 subnets * 512 validators)
//
// Examples:
// - V=10: min(50, 2058) = 50
// - V=100: min(500, 2148) = 500
// - V=512: min(2560, 2560) = 2560
// - V=1000: min(5000, 3048) = 3048 (sync committee cap applies!)
// - V=3000: min(15000, 5048) = 5048 (sync committee cap applies!)
//
// This prevents flooding attacks while accurately reflecting Ethereum's global sync committee limit.
fn validate_aggregator_committee_message_count(
    kind: PartialSignatureKind,
    message_count: usize,
    validator_count: usize,
) -> Result<(), ValidationFailure> {
    match kind {
        PartialSignatureKind::AggregatorCommitteePartialSig => {
            // Pre-consensus selection proofs
            // Max: V attestation proofs + V * 4 sync proofs (4 = SYNC_COMMITTEE_SUBNET_COUNT)
            let max_allowed = validator_count + (validator_count * 4);
            if message_count > max_allowed {
                return Err(ValidationFailure::TooManyPartialSignatureMessages {
                    got: message_count,
                    limit: max_allowed,
                });
            }
        }
        PartialSignatureKind::PostConsensus => {
            // Post-consensus aggregate signatures
            // Max: V aggregators + V * 4 contributors (4 = SYNC_COMMITTEE_SUBNET_COUNT)
            let max_allowed = validator_count + (validator_count * 4);
            if message_count > max_allowed {
                return Err(ValidationFailure::TooManyPartialSignatureMessages {
                    got: message_count,
                    limit: max_allowed,
                });
            }
        }
        _ => return Err(ValidationFailure::UnexpectedPartialSignatureType),
    }
    Ok(())
}
```

**Tests to add**:
- Unit test: AggregatorCommittee accepts AggregatorCommitteePartialSig
- Unit test: AggregatorCommittee accepts PostConsensus
- Unit test: Message count limits enforced

**Done when**: Message validator correctly handles AggregatorCommittee messages.

**Fork Gating**: ✅ REQUIRED (Safety Net Gate)
```rust
// In message validator, reject Role 6 messages before Boole
fn validate_role(role: Role, slot: Slot, fork_schedule: &ForkSchedule) -> Result<(), ValidationError> {
    if role == Role::AggregatorCommittee {
        let epoch = slot.epoch(slots_per_epoch);
        // Accept Role 6 only for Boole and later forks
        if fork_schedule.active_fork(epoch) < Fork::Boole {
            return Err(ValidationError::RoleNotActiveBeforeFork {
                role,
                current_fork: fork_schedule.active_fork(epoch),
                minimum_fork: Fork::Boole,
            });
        }
    }
    Ok(())
}
```
- Inject `Arc<ForkSchedule>` into MessageValidator
- This is the **safety net** - even if our code never produces Role 6 during Alan, external messages are rejected
- Add new error type `RoleNotActiveBeforeFork`
- Uses `< Fork::Boole` to reject, which means `>= Fork::Boole` accepts (Boole and all future forks)

**ssv-fuzz TODO** (post-development):
- Update `ssv-fuzz/fuzz/fuzz_targets/differential/diff_fuzz_complete_message_validation.rs` to include test cases with `Role::AggregatorCommittee`.
- Ensure messages with `PartialSignatureKind::AggregatorCommitteePartialSig` are validated correctly.
- Update `ssv-fuzz/fuzz/fuzz_targets/differential/diff_fuzz_partial_signature_validation.rs` if it tests role-kind matching.

---

### Task 11: Add QBFT Routing for AggregatorCommittee

**Prerequisites**: Tasks 1, 4, 5

**Files to modify**:
- `anchor/qbft_manager/src/lib.rs`

**Changes**:
```rust
// Add new instance ID type
#[derive(Debug, Clone, Hash, PartialEq, Eq)]
pub struct AggregatorCommitteeInstanceId {
    pub committee: CommitteeId,
    pub instance_height: InstanceHeight,
}

// Add instance map to QbftManager
pub struct QbftManager<E: EthSpec> {
    // ... existing fields ...

    /// QBFT instances for AggregatorCommitteeConsensusData
    aggregator_committee_instances: InstanceMap<
        AggregatorCommitteeInstanceId,
        AggregatorCommitteeConsensusData<E>,
    >,
}

// Extend receive_data routing
pub fn receive_data(
    &self,
    full_message: SignedSSVMessage,
    qbft_message: ssv_types::consensus::QbftMessage,
) -> Result<(), QbftError> {
    let msg_id = full_message.ssv_message().msg_id();
    let instance_height = (qbft_message.height as usize).into();

    match msg_id.duty_executor() {
        Some(DutyExecutor::Committee(committee)) => {
            match msg_id.role() {
                Some(Role::Committee) => {
                    // Existing BeaconVote routing
                    let id = CommitteeInstanceId { committee, instance_height };
                    self.pass_to_instance::<BeaconVote>(id, full_message, qbft_message)
                }
                Some(Role::AggregatorCommittee) => {
                    // NEW: Route to aggregator committee instances
                    let id = AggregatorCommitteeInstanceId { committee, instance_height };
                    self.pass_to_instance::<AggregatorCommitteeConsensusData<E>>(
                        id, full_message, qbft_message
                    )
                }
                _ => Err(QbftError::InconsistentMessageId)
            }
        }
        Some(DutyExecutor::Validator(validator)) => {
            // Existing validator routing...
        }
        None => Err(QbftError::InconsistentMessageId)
    }
}
```

**Tests to add**:
- Unit test: Messages with Role::AggregatorCommittee route to correct instance map
- Integration test: QBFT consensus completes for AggregatorCommitteeConsensusData

**Done when**: QBFT manager routes AggregatorCommittee messages correctly.

**Fork Gating**: ✅ REQUIRED
```rust
// In QbftManager::receive_data, only route Role 6 if Boole+ is active
Some(Role::AggregatorCommittee) => {
    let epoch = slot.epoch(E::slots_per_epoch());
    // Accept Role 6 only for Boole and later forks
    if fork_schedule.active_fork(epoch) < Fork::Boole {
        warn!(%slot, "Ignoring AggregatorCommittee message before Boole fork");
        return Err(QbftError::RoleNotActive);
    }
    // Route to aggregator committee instances
    let id = AggregatorCommitteeInstanceId { committee, instance_height };
    self.pass_to_instance::<AggregatorCommitteeConsensusData<E>>(id, full_message, qbft_message)
}
```
- Inject `Arc<ForkSchedule>` into QbftManager
- Log and reject Role 6 messages received before Boole
- Uses `< Fork::Boole` to reject, which means `>= Fork::Boole` accepts (Boole and all future forks)
- Note: Message validator (Task 10) should catch this first, this is defense-in-depth

**ssv-fuzz TODO** (post-development):
- Update `ssv-fuzz/fuzz/fuzz_targets/differential/diff_fuzz_qbft.rs` to test `Role::AggregatorCommittee` consensus.
- Key changes needed in diff_fuzz_qbft.rs:
  1. Update `msg_id()` function to support `Role::AggregatorCommittee`
  2. Create test data generators for `AggregatorCommitteeConsensusData`
  3. Ensure Go QBFT instance can be initialized with the new role
- This is CRITICAL for verifying both implementations reach consensus on the same values.

---

### Task 11b: Extend AggregationAssignments to Build AggregatorCommitteeConsensusData

**Prerequisites**: Tasks 4, 5, 6e, 8, 9, 11

**Reference**: See [AGGREGATOR_COMMITTEE_CONSENSUS_DATA_GUIDE.md](./AGGREGATOR_COMMITTEE_CONSENSUS_DATA_GUIDE.md) for detailed data flow and code examples.

**Overview**: This task extends the MetadataService's 2/3 slot processing to:
1. Fetch aggregated attestations and sync contributions from beacon node
2. Combine with selection proofs from duties_service
3. Build **one `AggregatorCommitteeConsensusData` per committee**
4. Cache it for use by per-validator `produce_signed_aggregate_and_proof` / `produce_signed_contribution_and_proof` calls

This follows the same pattern as `VotingContext` (built at 1/3 slot with `BeaconVote`), but for aggregator duties at 2/3 slot.

**Architecture Flow**:

```
┌─────────────────────────────────────────────────────────────────────────────┐
│ PHASE 3: MetadataService at 2/3 slot (update_aggregation_assignments)       │
├─────────────────────────────────────────────────────────────────────────────┤
│ 1. Get selection proofs from duties_service (already computed)              │
│ 2. Fetch aggregated attestations from BN (one per committee index)          │
│ 3. Fetch sync contributions from BN (one per subnet)                        │
│ 4. Build AggregatorCommitteeConsensusData per committee                     │
│    (contains ALL validators in the committee - not filtered)                │
│ 5. Cache in AggregationAssignments.consensus_data_by_committee              │
└─────────────────────────────────────────────────────────────────────────────┘
                                      │
                                      ▼
┌─────────────────────────────────────────────────────────────────────────────┐
│ QBFT CONSENSUS (once per committee, not per validator)                      │
├─────────────────────────────────────────────────────────────────────────────┤
│ 1. All operators propose the SAME full committee consensus data             │
│ 2. QBFT decides on one AggregatorCommitteeConsensusData                     │
│ 3. Decided data contains ALL validators' aggregators/contributors           │
│ 4. Result is cached per committee (subsequent calls reuse cached result)    │
└─────────────────────────────────────────────────────────────────────────────┘
                                      │
                                      ▼
┌─────────────────────────────────────────────────────────────────────────────┐
│ POST-CONSENSUS SIGNING (per validator)                                      │
├─────────────────────────────────────────────────────────────────────────────┤
│ 1. Get decided AggregatorCommitteeConsensusData for committee               │
│ 2. Extract this validator's entry from decided.aggregators/contributors     │
│ 3. Sign the decided aggregate/contribution for this validator               │
│ 4. Return SignedAggregateAndProof / SignedContributionAndProof              │
└─────────────────────────────────────────────────────────────────────────────┘
```

**Key Design Principle**: `AggregatorCommitteeConsensusData` is a **committee-based data object** containing
data for ALL validators in the committee. All operators reach consensus on the same data structure.
Individual validators then sign their portion from the decided data. There is NO per-validator filtering
of the consensus data - the full committee data is what goes through QBFT.

**Files to modify**:
- `anchor/validator_store/src/lib.rs` (extend `AggregationAssignments`)
- `anchor/metadata_service/` (extend `update_aggregation_assignments`)

**Changes**:

**Step 1: Extend AggregationAssignments struct**

```rust
/// Aggregator-specific assignments cached at 2/3 slot.
///
/// Extended to include pre-built AggregatorCommitteeConsensusData per committee,
/// similar to how VotingContext includes BeaconVote.
pub struct AggregationAssignments<E: EthSpec> {
    pub slot: Slot,

    // Existing fields...
    pub aggregating_attesters: HashSet<ValidatorIndex>,
    pub aggregator_committees: HashMap<PublicKeyBytes, u64>,
    pub sync_aggregators_by_subnet: HashMap<ValidatorIndex, HashSet<SyncSubnetId>>,
    multi_sync_aggregators: HashMap<PublicKeyBytes, ContributionWaiter<E>>,

    // NEW: Pre-built consensus data per committee (for Boole+)
    /// Maps CommitteeId -> AggregatorCommitteeConsensusData
    /// Only populated when fork >= Boole
    consensus_data_by_committee: HashMap<CommitteeId, Arc<AggregatorCommitteeConsensusData<E>>>,
}

impl<E: EthSpec> AggregationAssignments<E> {
    /// Get the pre-built consensus data for a committee.
    /// Returns None if fork < Boole or no aggregators in committee.
    ///
    /// NOTE: The returned data contains ALL validators in the committee that passed
    /// the is_aggregator() / is_sync_committee_aggregator() checks.
    /// This is intentional - QBFT consensus runs on the full committee data,
    /// not filtered per-validator data. After QBFT decides, individual validators
    /// extract their entries from the decided data.
    pub fn get_consensus_data(
        &self,
        committee_id: &CommitteeId,
    ) -> Option<Arc<AggregatorCommitteeConsensusData<E>>> {
        self.consensus_data_by_committee.get(committee_id).cloned()
    }

    // NOTE: has_aggregator_duty() and has_contributor_duty() helpers were removed.
    // They are redundant because upstream filtering already ensures only aggregators
    // reach produce_signed_aggregate_and_proof / produce_signed_contribution_and_proof.
    // The selection proof existence check in duties_service is sufficient.
}
```

**Step 2: Extend update_aggregation_assignments in MetadataService**

```rust
/// Phase 3: Build AggregationAssignments at 2/3 slot.
/// For Boole+, also fetches from BN and builds AggregatorCommitteeConsensusData per committee.
async fn update_aggregation_assignments(&self, slot: Slot) -> Result<(), Error> {
    let epoch = slot.epoch(E::slots_per_epoch());
    let is_boole = self.fork_schedule.active_fork(epoch) >= Fork::Boole;

    // Get selection proofs from duties_service (same as before)
    let attesters = self.duties_service.attesters(slot);
    let sync_duties = self.duties_service.sync_duties.get_duties_for_slot::<E>(slot, &self.spec);

    // Build existing AggregationAssignments fields...
    let aggregating_attesters = /* ... existing logic ... */;
    let aggregator_committees = /* ... existing logic ... */;
    let sync_aggregators_by_subnet = /* ... existing logic ... */;

    // NEW: Build consensus data per committee (Boole+ only)
    let consensus_data_by_committee = if is_boole {
        self.build_consensus_data_for_all_committees(
            slot,
            &attesters,
            &sync_duties,
        ).await?
    } else {
        HashMap::new()
    };

    let assignments = AggregationAssignments {
        slot,
        aggregating_attesters,
        aggregator_committees,
        sync_aggregators_by_subnet,
        multi_sync_aggregators: /* ... */,
        consensus_data_by_committee,
    };

    self.validator_store.update_aggregation_assignments(assignments);
    Ok(())
}

/// Build AggregatorCommitteeConsensusData for each committee that has aggregators.
async fn build_consensus_data_for_all_committees(
    &self,
    slot: Slot,
    attesters: &[DutyAndProof],
    sync_duties: &Option<SlotDuties>,
) -> Result<HashMap<CommitteeId, Arc<AggregatorCommitteeConsensusData<E>>>, Error> {
    // Get VotingContext for beacon_vote (cached at 1/3 slot)
    let voting_context = self.validator_store.get_voting_context(slot).await?;

    // Group aggregators by committee_id
    // Returns HashMap<CommitteeId, HashSet<ValidatorIndex>> via database lookup:
    // pubkey → Share → cluster_id → Cluster → committee_id()
    let committees_with_aggregators = self.get_committees_with_aggregators(attesters, sync_duties);

    // Fetch all unique committee indexes and subnet IDs needed
    let all_committee_indexes: HashSet<u64> = /* collect from attesters */;
    let all_subnet_ids: HashSet<SyncSubnetId> = /* collect from sync_duties */;

    // Parallel fetch from beacon node
    let (aggregated_attestations, sync_contributions) = tokio::join!(
        self.fetch_aggregated_attestations(slot, &voting_context.beacon_vote, &all_committee_indexes),
        self.fetch_sync_contributions(slot, voting_context.beacon_vote.block_root, &all_subnet_ids),
    );

    let aggregated_attestations = aggregated_attestations?;
    let sync_contributions = sync_contributions?;

    // Build consensus data per committee
    let mut result = HashMap::new();
    for committee_id in committees_with_aggregators.keys() {
        let consensus_data = self.build_consensus_data_for_committee(
            slot,
            committee_id,
            attesters,
            sync_duties,
            &aggregated_attestations,
            &sync_contributions,
        ).await?;

        if let Some(data) = consensus_data {
            result.insert(*committee_id, Arc::new(data));
        }
    }

    Ok(result)
}

/// Build AggregatorCommitteeConsensusData for a single committee.
///
/// CRITICAL REQUIREMENTS (must match SSV Go/Spec exactly for consensus):
/// 1. Aggregators: Call is_aggregator() on the selection proof before including
/// 2. Contributors: Call is_sync_committee_aggregator() on the selection proof before including
/// 3. Aggregators: Sort by validator_index (all share same signing root)
/// 4. Contributors: Sort by (signing_root, validator_index) to match SSV Go's root-sorted processing
/// 5. Committee indexes: First-seen order from sorted aggregators (not sorted separately)
/// 6. Subnet IDs: First-seen order from sorted contributors (not sorted separately)
async fn build_consensus_data_for_committee(
    &self,
    slot: Slot,
    committee_id: &CommitteeId,
    attesters: &[DutyAndProof],
    sync_duties: &Option<SlotDuties>,
    aggregated_attestations: &HashMap<u64, Attestation<E>>,
    sync_contributions: &HashMap<SyncSubnetId, SyncCommitteeContribution<E>>,
) -> Result<Option<AggregatorCommitteeConsensusData<E>>, Error> {
    // Get ALL validators in this committee (not just aggregators) via database lookup.
    // We need the full committee membership to filter attesters/sync_duties.
    // Uses: state.metadata().get_all_by(&committee_id) → ValidatorMetadata.index
    let committee_validators = self.get_committee_validator_indices(committee_id);

    // === AGGREGATORS ===
    // 1. Filter to validators in this committee with selection proofs
    // 2. CRITICAL: Verify aggregated proof passes is_aggregator() check (SSV Go line 475)
    // 3. Sort by validator_index (all share same root, so this matches SSV Go)
    let mut aggregators: Vec<AssignedAggregator> = Vec::new();
    for d in attesters {
        let Some(selection_proof) = &d.selection_proof else { continue };
        let validator_index = ValidatorIndex(d.duty.validator_index as usize);
        if !committee_validators.contains(&validator_index) {
            continue;
        }

        // CRITICAL: Match SSV Go line 475
        // Verify the aggregated selection proof passes beacon node is_aggregator check.
        // Without this check, validators whose proofs don't meet the modulo threshold
        // would be included, causing consensus hash mismatch with other operators.
        let is_aggregator = self.beacon_node
            .is_aggregator(
                slot,
                d.duty.committee_index,
                d.duty.committee_length,
                selection_proof,
            )
            .await
            .unwrap_or(false);

        if !is_aggregator {
            continue;
        }

        aggregators.push(AssignedAggregator {
            validator_index,
            selection_proof: selection_proof.clone().into(),
            committee_index: d.duty.committee_index,
        });
    }

    // Sort by validator_index (matches SSV Go since all aggregators share same signing root)
    aggregators.sort_by_key(|a| a.validator_index);

    // === CONTRIBUTORS ===
    // 1. Filter to validators in this committee
    // 2. CRITICAL: Verify proof passes is_sync_committee_aggregator() check (SSV Go line 284)
    // 3. Sort by (signing_root, validator_index) to match SSV Go's root-sorted processing order
    //
    // Why (signing_root, validator_index)?
    // SSV Go processes all selection proofs by sorting roots lexicographically first.
    // Each subnet has a different signing root (SyncAggregatorSelectionData{Slot, SubcommitteeIndex}).
    // Contributors in different subnets have different roots, so we must sort by root first.
    let mut contributors_with_roots: Vec<(Hash256, AssignedAggregator)> = Vec::new();
    if let Some(duties) = sync_duties {
        for (subnet_id, aggs) in &duties.aggregators {
            // Compute signing root for this subnet ONCE (all validators in subnet share it)
            let sync_selection_root = self.compute_sync_selection_root(slot, (*subnet_id).into())?;

            for (idx, _, proof) in aggs {
                let validator_index = ValidatorIndex(*idx as usize);
                if !committee_validators.contains(&validator_index) {
                    continue;
                }

                // CRITICAL: Match SSV Go line 284
                // Verify the aggregated selection proof passes beacon node check.
                // Without this check, validators whose proofs don't meet the modulo threshold
                // would be included, causing consensus hash mismatch with other operators.
                let is_aggregator = self.beacon_node
                    .is_sync_committee_aggregator(proof)
                    .await
                    .unwrap_or(false);

                if !is_aggregator {
                    continue;
                }

                contributors_with_roots.push((
                    sync_selection_root,
                    AssignedAggregator {
                        validator_index,
                        selection_proof: proof.clone().into(),
                        committee_index: (*subnet_id).into(),
                    },
                ));
            }
        }
    }

    // Sort by (signing_root, validator_index) to match SSV Go's root-sorted processing
    contributors_with_roots.sort_by(|a, b| {
        a.0.as_bytes().cmp(b.0.as_bytes())
            .then_with(|| a.1.validator_index.cmp(&b.1.validator_index))
    });

    let contributors: Vec<AssignedAggregator> = contributors_with_roots
        .into_iter()
        .map(|(_, c)| c)
        .collect();

    // Early exit if no aggregators/contributors
    if aggregators.is_empty() && contributors.is_empty() {
        return Ok(None);
    }

    // === COMMITTEE INDEXES & ATTESTATIONS ===
    // Extract unique committee indexes in first-seen order from sorted aggregators.
    // SSV Go uses first-seen-wins deduplication (not sorted separately).
    let mut seen_committee_indexes: HashSet<u64> = HashSet::new();
    let mut committee_indexes: Vec<u64> = Vec::new();
    for agg in &aggregators {
        if seen_committee_indexes.insert(agg.committee_index) {
            committee_indexes.push(agg.committee_index);
        }
    }

    // Get attestations in committee_indexes order (1:1 correspondence)
    let attestations_bytes: Vec<VariableList<u8, _>> = committee_indexes
        .iter()
        .filter_map(|idx| aggregated_attestations.get(idx))
        .map(|a| VariableList::from(a.as_ssz_bytes()))
        .collect();

    // === SUBNET IDS & CONTRIBUTIONS ===
    // Extract unique subnet IDs in first-seen order from sorted contributors.
    // SSV Go uses first-seen-wins deduplication (not sorted separately).
    let mut seen_subnet_ids: HashSet<u64> = HashSet::new();
    let mut subnet_ids: Vec<SyncSubnetId> = Vec::new();
    for contrib in &contributors {
        if seen_subnet_ids.insert(contrib.committee_index) {
            subnet_ids.push(SyncSubnetId::from(contrib.committee_index as u8));
        }
    }

    let contributions: Vec<SyncCommitteeContribution<E>> = subnet_ids
        .iter()
        .filter_map(|id| sync_contributions.get(id).cloned())
        .collect();

    Ok(Some(AggregatorCommitteeConsensusData {
        version: DataVersion::from(self.spec.fork_name_at_slot::<E>(slot)),
        aggregators: VariableList::from(aggregators),
        aggregator_committee_indexes: VariableList::from(committee_indexes),
        aggregated_attestations: VariableList::from(attestations_bytes),
        contributors: VariableList::from(contributors),
        sync_committee_contributions: VariableList::from(contributions),
    }))
}

/// Compute the signing root for a sync committee selection proof.
///
/// Used to sort contributors by root to match SSV Go's processing order.
/// Each subnet has a different signing root based on SyncAggregatorSelectionData{Slot, SubcommitteeIndex}.
fn compute_sync_selection_root(&self, slot: Slot, subnet_id: u64) -> Result<Hash256, Error> {
    let data = SyncAggregatorSelectionData {
        slot,
        subcommittee_index: subnet_id,
    };
    let epoch = slot.epoch(E::slots_per_epoch());
    let domain = self.spec.get_domain(
        epoch,
        Domain::SyncCommitteeSelectionProof,
        &self.fork_schedule.fork_at_epoch(epoch),
        self.genesis_validators_root,
    );
    Ok(data.signing_root(domain))
}

/// Groups aggregating validators by their SSV CommitteeId using database lookup.
///
/// Returns a map of CommitteeId -> HashSet<ValidatorIndex> for validators that have
/// aggregation duties (attestation or sync) in this slot.
///
/// Path: pubkey → Share → cluster_id → Cluster → committee_id()
fn get_committees_with_aggregators(
    &self,
    attesters: &[DutyAndProof],
    sync_duties: &Option<SlotDuties>,
) -> HashMap<CommitteeId, HashSet<ValidatorIndex>> {
    // Get database state snapshot for lookups
    let database_state = self.validator_store.database_state();

    let mut committees: HashMap<CommitteeId, HashSet<ValidatorIndex>> = HashMap::new();

    // ══════════════════════════════════════════════════════════════════
    // Collect committees from attestation aggregators
    // ══════════════════════════════════════════════════════════════════
    for duty_and_proof in attesters {
        // Only include validators that are aggregators (have selection_proof)
        if duty_and_proof.selection_proof.is_none() {
            continue;
        }

        let validator_idx = ValidatorIndex(duty_and_proof.duty.validator_index as usize);

        // Look up Share → cluster_id → Cluster → committee_id
        if let Some(share) = database_state.shares().get_by_primary(&duty_and_proof.duty.pubkey) {
            if let Some(cluster) = database_state.clusters().get_by(&share.cluster_id) {
                let committee_id = cluster.committee_id();
                committees
                    .entry(committee_id)
                    .or_default()
                    .insert(validator_idx);
            }
        }
    }

    // ══════════════════════════════════════════════════════════════════
    // Collect committees from sync aggregators
    // ══════════════════════════════════════════════════════════════════
    if let Some(duties) = sync_duties {
        for (_subnet_id, aggregators) in &duties.aggregators {
            for (val_idx, pubkey, _selection_proof) in aggregators {
                let validator_idx = ValidatorIndex(*val_idx as usize);

                // Look up Share → cluster_id → Cluster → committee_id
                if let Some(share) = database_state.shares().get_by_primary(pubkey) {
                    if let Some(cluster) = database_state.clusters().get_by(&share.cluster_id) {
                        let committee_id = cluster.committee_id();
                        committees
                            .entry(committee_id)
                            .or_default()
                            .insert(validator_idx);
                    }
                }
            }
        }
    }

    committees
}

/// Get all validator indices for a committee using database lookup.
///
/// Uses state.metadata().get_all_by(&committee_id) to retrieve ValidatorMetadata
/// for all validators in the committee, then extracts their indices.
fn get_committee_validator_indices(&self, committee_id: &CommitteeId) -> HashSet<ValidatorIndex> {
    let database_state = self.validator_store.database_state();

    database_state
        .metadata()
        .get_all_by(committee_id)
        .filter_map(|metadata| metadata.index)
        .collect()
}
```

**Step 3: Update produce_signed_aggregate_and_proof to use cached data**

```rust
async fn produce_signed_aggregate_and_proof(
    &self,
    validator_pubkey: PublicKeyBytes,
    slot: Slot,
) -> Result<SignedAggregateAndProof<E>, Error> {
    let epoch = slot.epoch(E::slots_per_epoch());

    if self.fork_schedule.active_fork(epoch) >= Fork::Boole {
        // Boole+: Committee-based consensus using cached data
        self.produce_signed_aggregate_and_proof_committee(validator_pubkey, slot).await
    } else {
        // Pre-Boole: Existing per-validator behavior
        self.produce_signed_aggregate_and_proof_single(validator_pubkey, slot).await
    }
}

async fn produce_signed_aggregate_and_proof_committee(
    &self,
    validator_pubkey: PublicKeyBytes,
    slot: Slot,
) -> Result<SignedAggregateAndProof<E>, Error> {
    let (validator, cluster) = self.get_validator_and_cluster(validator_pubkey)?;
    let committee_id = cluster.committee_id();
    let validator_index = validator.index.ok_or(SpecificError::MissingIndex)?;

    // Get cached AggregationAssignments (built at 2/3 slot by MetadataService)
    let aggregation_assignments = self.get_aggregation_assignments(slot).await?;

    // Get the full committee's consensus data (NOT filtered - contains all validators)
    // This is what will be proposed to QBFT
    // NOTE: No has_aggregator_duty check needed here - upstream filtering in duties_service
    // already ensures this function is only called for actual aggregators.
    let consensus_data = aggregation_assignments
        .get_consensus_data(&committee_id)
        .ok_or(SpecificError::NoConsensusDataForCommittee)?;

    // Run QBFT with full committee consensus data
    // All operators propose the same data (all validators in committee)
    // QbftManager handles caching - if already decided for this committee, returns cached result
    let decided_data = self.run_aggregator_committee_qbft(
        slot,
        committee_id,
        &cluster,
        (*consensus_data).clone(), // Full committee data, not filtered
    ).await?;

    // Extract THIS validator's aggregate from decided data and sign
    // The decided_data contains all validators; we extract our portion for signing
    // ... (rest of signing logic from existing Task 12)
}
```

**Important**: All operators propose the SAME `AggregatorCommitteeConsensusData` containing ALL validators
in the committee. After QBFT decides, each validator extracts their entry from the decided data for signing.
This is different from per-validator consensus where each validator proposes their own data.

**Tests to add**:
- Unit test: `test_aggregation_assignments_includes_consensus_data_for_boole`
- Unit test: `test_consensus_data_contains_all_committee_validators`
- Unit test: `test_build_consensus_data_for_committee`
- Unit test: `test_build_consensus_data_filters_with_is_aggregator`
- Unit test: `test_build_consensus_data_filters_with_is_sync_committee_aggregator`
- Unit test: `test_aggregators_sorted_by_validator_index`
- Unit test: `test_contributors_sorted_by_signing_root_then_validator_index`
- Unit test: `test_committee_indexes_in_first_seen_order`
- Unit test: `test_subnet_ids_in_first_seen_order`
- Unit test: `test_no_consensus_data_before_boole`
- Integration test: MetadataService builds consensus data at 2/3 slot
- Integration test: produce_signed_aggregate_and_proof uses full committee data
- Integration test: Multiple operators produce identical consensus data hash

**Validation checklist**:
- [ ] AggregationAssignments extended with `consensus_data_by_committee`
- [ ] MetadataService fetches from BN at 2/3 slot (parallel)
- [ ] Builds one AggregatorCommitteeConsensusData per committee
- [ ] Consensus data contains validators that passed `is_aggregator()` / `is_sync_committee_aggregator()` checks
- [ ] Aggregators sorted by validator_index
- [ ] Contributors sorted by (signing_root, validator_index)
- [ ] Committee indexes in first-seen order from sorted aggregators
- [ ] Subnet IDs in first-seen order from sorted contributors
- [ ] Fork gating: only build consensus data for Boole+
- [ ] Graceful handling when no aggregators in committee

**Done when**: `AggregationAssignments` contains pre-built consensus data per committee, and `produce_signed_aggregate_and_proof` uses it.

**Fork Gating**: ✅ REQUIRED
- `build_consensus_data_for_all_committees` only called when `fork >= Boole`
- `produce_signed_aggregate_and_proof` branches based on fork

---

### Task 12: Implement Post-Consensus Aggregate Signing - ✅ FIX IMPLEMENTED

**Prerequisites**: Tasks 4, 11, 11b

**Status**: The core fix for this task has been implemented. The key insight from the design doc analysis is that **shares = DutiesService validators** (same source of truth via `voting_pubkeys()`), which means Solution A (using decided data count) is the correct minimal fix.

**Key Insight from Design Doc Analysis**:

The SSV database shares and Lighthouse's DutiesService use the **same source of truth**:
```
ValidatorAdded event
  → stored in database.shares()
  → AnchorValidatorStore.voting_pubkeys() reads from shares
  → DutiesService.poll_beacon_attesters() uses voting_pubkeys()
  → DutiesService calls produce_signed_aggregate_and_proof for those validators
```

This means:
- If we have shares for [A, B, C], DutiesService knows [A, B, C] and will call for all three
- We CANNOT have shares for validators that DutiesService doesn't know about
- The only divergence comes from **QBFT consensus** where another operator's proposal may win

**The Fix (Solution A - Implemented)**:

```rust
// BEFORE (buggy - used aggregation_assignments):
let num_signatures_to_collect = aggregation_assignments
    .attestation_aggregator_count(|idx| committee_validator_indices.contains(idx));

// AFTER (correct - uses decided_data filtered by our shares):
let num_signatures_to_collect = decided_data.aggregators
    .iter()
    .filter(|agg| committee_validator_indices.contains(&agg.validator_index))
    .count();
```

**Why This Works for All Edge Cases**:

| Case | Scenario | Behavior |
|------|----------|----------|
| Case 1 | Our local > decided (bug case) | Extra validators rejected with `ValidatorNotInConsensus(ValidatorIndex)`, count matches decided |
| Case 2 | Local = decided (happy path) | Works as expected |
| Case 3 | Decided > our shares | We only count validators we have shares for |
| Case 4 | Divergent operators | Each operator sends their own subset |

**Updated Error Type**:

The `ValidatorNotInConsensus` error now includes the validator index for better debugging:
```rust
// OLD:
.ok_or(SpecificError::ValidatorNotInConsensus)?;

// NEW:
.ok_or(SpecificError::ValidatorNotInConsensus(validator_index))?;
```

**Solution B (NOT NEEDED)**:

The design doc initially considered Solution B (drive signing from decided data with caching), but this is unnecessary because:
1. Lighthouse calls for ALL validators we have shares for (same source of truth)
2. Validators not in decided_data are rejected early with `ValidatorNotInConsensus(ValidatorIndex)`
3. `num_signatures_to_collect` correctly counts only validators in decided_data that we have shares for
4. When all decided validators we have shares for are processed → envelope sent

**Files to modify**:
- `anchor/validator_store/src/lib.rs`

**Changes**:
```rust
/// Produce signed aggregate and proof using committee-based consensus.
/// This extracts the validator's aggregate from decided AggregatorCommitteeConsensusData.
async fn produce_signed_aggregate_and_proof_committee(
    &self,
    validator_pubkey: PublicKeyBytes,
    aggregator_index: u64,
    aggregate: &Attestation<E>,
    selection_proof: SelectionProof,
    slot: Slot,
) -> Result<SignedAggregateAndProof<E>, Error> {
    let future = async {
        let (validator, cluster) = self.get_validator_and_cluster(validator_pubkey)?;
        let committee_id = cluster.committee_id();
        let validator_index = validator.index.ok_or(SpecificError::MissingIndex)?;

        // Get or wait for QBFT consensus result
        let decided_data = self
            .get_aggregator_committee_consensus(slot, committee_id)
            .await?;

        // Find this validator's decided aggregate from consensus data
        let committee_index = aggregate.data().index;
        let decided_aggregate_idx = decided_data.aggregator_committee_indexes
            .iter()
            .position(|&idx| idx == committee_index)
            .ok_or(SpecificError::AggregateNotInConsensus)?;

        let decided_aggregate_bytes = &decided_data.aggregated_attestations[decided_aggregate_idx];

        // Decode based on fork version
        let decided_aggregate = if decided_data.version < DataVersion::from(ForkName::Electra) {
            Attestation::Base(AttestationBase::from_ssz_bytes(decided_aggregate_bytes)?)
        } else {
            Attestation::Electra(AttestationElectra::from_ssz_bytes(decided_aggregate_bytes)?)
        };

        // Find this validator's selection proof from consensus data
        // If not found, another operator's proposal with fewer validators won QBFT
        let decided_selection_proof = decided_data.aggregators
            .iter()
            .find(|agg| agg.validator_index == validator_index)
            .map(|agg| SelectionProof::from(agg.selection_proof.clone()))
            .ok_or(SpecificError::ValidatorNotInConsensus(validator_index))?;

        // Build message to sign
        let message = AggregateAndProof::from_attestation(
            aggregator_index,
            decided_aggregate,
            decided_selection_proof,
        );

        // Sign with PostConsensus kind using Committee collection mode
        // The count is based on decided_data filtered by our shares
        let epoch = slot.epoch(E::slots_per_epoch());
        let domain_hash = self.get_domain(epoch, Domain::AggregateAndProof);
        let signing_root = message.signing_root(domain_hash);

        // Calculate num_signatures_to_collect from decided_data (THE FIX)
        let committee_validator_indices = self.get_committee_validator_indices(&committee_id);
        let num_signatures_to_collect = decided_data.aggregators
            .iter()
            .filter(|agg| committee_validator_indices.contains(&agg.validator_index))
            .count();

        let collection_mode = CollectionMode::Committee {
            num_signatures_to_collect,
            base_hash: decided_data.hash(),
        };

        let signature = self
            .collect_signature(
                PartialSignatureKind::PostConsensus,
                Role::AggregatorCommittee,
                collection_mode,
                &validator,
                &cluster,
                signing_root,
                slot,
            )
            .await?;

        Ok(SignedAggregateAndProof::from_aggregate_and_proof(message, signature))
    };

    run_and_update_metrics(
        AGGREGATE_LOG_NAME,
        &validator_metrics::SIGNED_AGGREGATES_TOTAL,
        future,
    )
    .await
}
```

**Tests to add**:
- Unit test: Extracts correct aggregate from consensus data
- Unit test: Signs with correct domain
- Unit test: `test_post_consensus_signing_rejects_validator_not_in_consensus` - Verify validators not in decided data return `ValidatorNotInConsensus(ValidatorIndex)` error
- Unit test: `test_num_signatures_to_collect_uses_decided_data` - Verify count comes from decided_data filtered by our shares
- Integration test: Full aggregate signing flow

**Validation checklist**:
- [x] Extracts correct aggregate from `AggregatorCommitteeConsensusData`
- [x] Uses `PartialSignatureKind::PostConsensus` with `Role::AggregatorCommittee`
- [x] Uses `CollectionMode::Committee` with count from decided_data
- [x] `num_signatures_to_collect` computed from `decided_data.aggregators` filtered by our shares
- [x] `ValidatorNotInConsensus(ValidatorIndex)` error includes the validator index for debugging

**Divergent View Handling (Simplified)**:

With Solution A, divergent views are handled automatically:
- Lighthouse calls for validators based on OUR shares (same source of truth)
- If a validator is not in decided_data, we return `ValidatorNotInConsensus(ValidatorIndex)` early
- `num_signatures_to_collect` is based on decided_data, so we don't wait for extra validators
- No explicit iteration over decided data needed - Lighthouse drives the calls

**Done when**: Post-consensus aggregate signing uses decided consensus data with correct `num_signatures_to_collect`.

**Fork Gating**: ✅ REQUIRED (inherited from entry point)
- This function is only called after QBFT consensus completes for AggregatorCommitteeConsensusData
- QBFT only starts if fork is Boole+ (Task 8/9 gating)
- No additional gating needed here, but consider adding defensive check:
```rust
debug_assert!(fork_schedule.active_fork(slot.epoch(...)) >= Fork::Boole,
    "post-consensus signing called before Boole fork");
```

**ssv-fuzz TODO**: Not applicable - internal Anchor signing flow. Wire format of signatures is standard BLS and doesn't need differential testing.

---

### Task 13: Implement Post-Consensus Contribution Signing - ✅ FIX IMPLEMENTED

**Prerequisites**: Tasks 4, 11, 11b, 12

**Status**: The core fix for this task follows the same pattern as Task 12. The key insight about **shares = DutiesService validators** applies equally to sync committee contributions.

**The Fix (Same as Task 12)**:

```rust
// BEFORE (buggy - used aggregation_assignments):
// Count came from local aggregation_assignments

// AFTER (correct - uses decided_data filtered by our shares):
let num_signatures_to_collect = decided_data.contributors
    .iter()
    .filter(|contrib| committee_validator_indices.contains(&contrib.validator_index))
    .count();
```

**Updated Error Type**:

The `ValidatorNotInConsensus` error now includes the validator index:
```rust
.ok_or(SpecificError::ValidatorNotInConsensus(validator_index))?;
```

**Files to modify**:
- `anchor/validator_store/src/lib.rs`

**Changes**:
```rust
/// Produce signed contribution and proof using committee-based consensus.
async fn produce_signed_contribution_and_proof_committee(
    &self,
    aggregator_index: u64,
    aggregator_pubkey: PublicKeyBytes,
    contribution: &SyncCommitteeContribution<E>,
    selection_proof: SyncSelectionProof,
    slot: Slot,
) -> Result<SignedContributionAndProof<E>, Error> {
    let future = async {
        let (validator, cluster) = self.get_validator_and_cluster(aggregator_pubkey)?;
        let committee_id = cluster.committee_id();
        let validator_index = validator.index.ok_or(SpecificError::MissingIndex)?;

        // Get consensus result
        let decided_data = self
            .get_aggregator_committee_consensus(slot, committee_id)
            .await?;

        // Find this subcommittee's contribution from consensus data
        let subcommittee_idx = contribution.subcommittee_index;
        let decided_contribution = decided_data.sync_committee_contributions
            .iter()
            .find(|c| c.subcommittee_index == subcommittee_idx)
            .cloned()
            .ok_or(SpecificError::ContributionNotInConsensus)?;

        // Find this validator's selection proof
        // If not found, another operator's proposal with fewer validators won QBFT
        let decided_selection_proof = decided_data.contributors
            .iter()
            .find(|c| c.validator_index == validator_index
                   && c.duty_index == subcommittee_idx)
            .map(|c| SyncSelectionProof::from(c.selection_proof.clone()))
            .ok_or(SpecificError::ValidatorNotInConsensus(validator_index))?;

        // Build and sign
        let message = ContributionAndProof {
            aggregator_index,
            contribution: decided_contribution,
            selection_proof: decided_selection_proof,
        };

        let epoch = slot.epoch(E::slots_per_epoch());
        let domain_hash = self.get_domain(epoch, Domain::ContributionAndProof);
        let signing_root = message.signing_root(domain_hash);

        // Calculate num_signatures_to_collect from decided_data (THE FIX)
        let committee_validator_indices = self.get_committee_validator_indices(&committee_id);
        let num_signatures_to_collect = decided_data.contributors
            .iter()
            .filter(|contrib| committee_validator_indices.contains(&contrib.validator_index))
            .count();

        let collection_mode = CollectionMode::Committee {
            num_signatures_to_collect,
            base_hash: decided_data.hash(),
        };

        let signature = self
            .collect_signature(
                PartialSignatureKind::PostConsensus,
                Role::AggregatorCommittee,
                collection_mode,
                &validator,
                &cluster,
                signing_root,
                slot,
            )
            .await?;

        Ok(SignedContributionAndProof {
            message,
            signature,
        })
    };

    run_and_update_metrics(
        CONTRIBUTION_LOG_NAME,
        &validator_metrics::SIGNED_CONTRIBUTIONS_TOTAL,
        future,
    )
    .await
}
```

**Tests to add**:
- Unit test: Extracts correct contribution from consensus data
- Unit test: `test_post_consensus_contribution_signing_rejects_validator_not_in_consensus` - Verify validators not in decided data return `ValidatorNotInConsensus(ValidatorIndex)` error
- Unit test: `test_contribution_num_signatures_to_collect_uses_decided_data` - Verify count comes from decided_data filtered by our shares
- Integration test: Full contribution signing flow

**Validation checklist**:
- [x] Extracts correct contribution from `AggregatorCommitteeConsensusData`
- [x] Uses `PartialSignatureKind::PostConsensus` with `Role::AggregatorCommittee`
- [x] Uses `CollectionMode::Committee` with count from decided_data
- [x] `num_signatures_to_collect` computed from `decided_data.contributors` filtered by our shares
- [x] `ValidatorNotInConsensus(ValidatorIndex)` error includes the validator index for debugging

**Divergent View Handling (Simplified)**:

Same as Task 12 - with Solution A, divergent views are handled automatically:
- Lighthouse calls for validators based on OUR shares (same source of truth)
- If a validator is not in decided_data, we return `ValidatorNotInConsensus(ValidatorIndex)` early
- `num_signatures_to_collect` is based on decided_data, so we don't wait for extra validators
- No explicit iteration over decided data needed - Lighthouse drives the calls

**Done when**: Post-consensus contribution signing uses decided consensus data with correct `num_signatures_to_collect`.

**Fork Gating**: ✅ REQUIRED (inherited from entry point) - Same as Task 12.

**ssv-fuzz TODO**: Not applicable - internal Anchor signing flow, same as Task 12.

---

### Task 14: Add Submission Tracking

**Prerequisites**: Tasks 12, 13

**Files to modify**:
- `anchor/validator_store/src/lib.rs`

**Changes**:
```rust
/// Tracks submissions to avoid duplicates.
/// Different strategies per duty type per SSV spec.
struct AggregatorSubmissionTracker {
    /// Attestation aggregators: (slot, validator_index) -> submitted
    /// One submission per validator.
    attestation_submissions: DashSet<(Slot, ValidatorIndex)>,

    /// Sync contributors: (slot, validator_index, beacon_block_root) -> submitted
    /// Multiple submissions per validator possible (different subcommittees/roots).
    sync_submissions: DashSet<(Slot, ValidatorIndex, Hash256)>,
}

impl AggregatorSubmissionTracker {
    /// Record attestation aggregate submission.
    pub fn record_attestation(&self, slot: Slot, validator_index: ValidatorIndex) {
        self.attestation_submissions.insert((slot, validator_index));
    }

    /// Check if attestation aggregate already submitted.
    pub fn has_submitted_attestation(&self, slot: Slot, validator_index: ValidatorIndex) -> bool {
        self.attestation_submissions.contains(&(slot, validator_index))
    }

    /// Record sync contribution submission.
    pub fn record_sync(&self, slot: Slot, validator_index: ValidatorIndex, root: Hash256) {
        self.sync_submissions.insert((slot, validator_index, root));
    }

    /// Check if sync contribution already submitted.
    pub fn has_submitted_sync(
        &self,
        slot: Slot,
        validator_index: ValidatorIndex,
        root: Hash256
    ) -> bool {
        self.sync_submissions.contains(&(slot, validator_index, root))
    }

    /// Check if all expected submissions complete.
    pub fn all_submitted(
        &self,
        slot: Slot,
        expected_attestation_validators: &[ValidatorIndex],
        expected_sync_submissions: &[(ValidatorIndex, Hash256)],
    ) -> bool {
        // All attestation aggregators submitted
        for &validator_index in expected_attestation_validators {
            if !self.has_submitted_attestation(slot, validator_index) {
                return false;
            }
        }

        // All sync contributions submitted
        for (validator_index, root) in expected_sync_submissions {
            if !self.has_submitted_sync(slot, *validator_index, *root) {
                return false;
            }
        }

        true
    }

    /// Cleanup old entries (called periodically).
    pub fn cleanup(&self, current_slot: Slot) {
        let cutoff = current_slot.saturating_sub(2u64);
        self.attestation_submissions.retain(|(slot, _)| *slot >= cutoff);
        self.sync_submissions.retain(|(slot, _, _)| *slot >= cutoff);
    }
}
```

**Tests to add**:
- Unit test: Tracks attestation submissions correctly
- Unit test: Tracks sync submissions with root differentiation
- Unit test: all_submitted logic

**Done when**: Submission tracking matches Go SSV behavior.

**Fork Gating**: ❌ Not needed - Internal state tracking, only used during Boole flow.

**ssv-fuzz TODO**: Not applicable - internal Anchor state tracking, no wire format.

---

### Task 15: Wire Compatibility Tests

**Prerequisites**: Tasks 1-14

**Files to create**:
- `anchor/common/ssv_types/src/tests/aggregator_committee_wire_compat.rs`

**Changes**:
```rust
#[cfg(test)]
mod wire_compat_tests {
    use super::*;

    /// Test vectors from Go SSV (generate by running Go test and capturing bytes)
    const ASSIGNED_AGGREGATOR_GO_BYTES: &[u8] = &[/* bytes from Go */];
    const CONSENSUS_DATA_GO_BYTES: &[u8] = &[/* bytes from Go */];

    #[test]
    fn assigned_aggregator_wire_compat() {
        // Create same data as Go
        let agg = AssignedAggregator {
            validator_index: ValidatorIndex::new(12345),
            selection_proof: Signature::empty(),  // Use known test signature
            duty_index: 42,
        };

        let rust_bytes = agg.as_ssz_bytes();

        // Compare with Go bytes
        assert_eq!(rust_bytes, ASSIGNED_AGGREGATOR_GO_BYTES);

        // Decode Go bytes
        let decoded = AssignedAggregator::from_ssz_bytes(ASSIGNED_AGGREGATOR_GO_BYTES).unwrap();
        assert_eq!(decoded, agg);
    }

    #[test]
    fn consensus_data_wire_compat() {
        // Similar test for AggregatorCommitteeConsensusData
    }

    #[test]
    fn message_id_wire_compat() {
        // Test MessageId encoding for Role::AggregatorCommittee
        let domain = DomainType::from([0x00, 0x00, 0x01, 0x00]);  // SSV domain
        let committee_id = CommitteeId::from([/* 32 bytes */]);

        let msg_id = MessageId::new(
            &domain,
            Role::AggregatorCommittee,
            &DutyExecutor::Committee(committee_id),
        );

        let bytes: [u8; 56] = msg_id.into();

        // Verify bytes 4-7 are [6, 0, 0, 0]
        assert_eq!(&bytes[4..8], &[6, 0, 0, 0]);

        // Verify bytes 24-55 are committee ID
        assert_eq!(&bytes[24..56], committee_id.as_slice());
    }

    #[test]
    fn partial_sig_kind_wire_compat() {
        let kind = PartialSignatureKind::AggregatorCommitteePartialSig;
        let bytes = kind.as_ssz_bytes();

        // Should be u64 little-endian value 6
        assert_eq!(bytes, &[6, 0, 0, 0, 0, 0, 0, 0]);
    }
}
```

**Generating Go test vectors**:
```go
// In Go SSV test file
func TestGenerateWireVectors(t *testing.T) {
    agg := &types.AssignedAggregator{
        ValidatorIndex: 12345,
        SelectionProof: [96]byte{},
        CommitteeIndex: 42,
    }
    bytes, _ := agg.MarshalSSZ()
    fmt.Printf("ASSIGNED_AGGREGATOR_GO_BYTES: %v\n", bytes)
}
```

**Done when**: All wire format tests pass with Go-generated test vectors.

**Fork Gating**: ❌ Not needed - Tests only; tests should work regardless of fork.

**ssv-fuzz TODO** (post-development):
- Task 15 tests are exactly what ssv-fuzz provides! Consider integrating these tests INTO ssv-fuzz rather than duplicating in Anchor:
  - `diff_fuzz_assigned_aggregator.rs` (new target from Task 3)
  - `diff_fuzz_aggregator_committee_consensus_data.rs` (new target from Task 4)
  - Updates to `diff_fuzz_message_id_generation.rs` (from Task 1)
  - Updates to `diff_fuzz_qbft.rs` (from Task 11)
- The static test vectors in this task can serve as seed inputs for the fuzz targets.
- Run ssv-fuzz against the implementation to find edge cases not covered by static tests.

---

### Task 16: Divergent Validator Set Handling

**Prerequisites**: None (can be done independently)

**Motivation**: From [Go SSV PR #2503 discussion](https://github.com/ssvlabs/ssv/pull/2503#discussion_r2658117698):
> "For committee duties, we encompass the case in which operators have divergent views on the committee state (what validators belong to it). This inconsistency may arise when operators have processed different SSV smart contract events (e.g., one is lagging behind)."

**Scope - Committee-Based Roles Only**:
- `Role::Committee` - Batched attestation/sync committee messages
- `Role::AggregatorCommittee` - Batched selection proof messages

**Go SSV Pattern** (from `basePartialSigMsgProcessing` in `runner.go`):

Go SSV handles divergent views with a **store blindly, filter at processing** approach:

1. **Receive partial sigs** → Store blindly in container (NO filtering at receive time)
   ```go
   // In basePartialSigMsgProcessing - stores ALL partial sigs
   for _, msg := range signedMsg.Messages {
       container.AddSignature(msg)  // No validator check here!
   }
   ```

2. **Process with quorum** → Filter when iterating over roots/validators
   ```go
   // In ProcessPreConsensus (aggregator_committee.go lines 391-409)
   metadataList, found := r.findValidatorsForPreConsensusRoot(root, aggregatorMap, contributionMap)
   if !found {
       // Edge case: operator doesn't have the validator associated to a root.
       continue
   }

   for _, metadata := range metadataList {
       share := r.BaseRunner.Share[validatorIndex]
       if share == nil {
           continue  // Skip unknown validators
       }
       // ... process
   }
   ```

**Anchor's Approach - Per-Validator API with Architectural Filtering**:

With Anchor's per-validator `produce_selection_proof(validator_pubkey, slot)` API, divergent view handling is **architectural** - no explicit share checks needed for pre-consensus:

1. **Receive**: Store all partial sigs blindly (same as Go SSV)
   - SignatureCollector creates collector for any `(signing_root, validator_index)` pair via `get_or_spawn`
   - Unknown validators' sigs are stored in their collector's `signature_share` map

2. **Reconstruct**: Architecturally impossible for unknown validators
   - Each collector has a `threshold` field that starts as `None`
   - `threshold` is ONLY set when `sign_and_collect` sends `RegisterNotifier`
   - Reconstruction check: `if let Some(threshold) = threshold && signature_share.len() >= threshold`
   - **Without a threshold, reconstruction NEVER happens** even if quorum of partial sigs exists
   - You can only call `sign_and_collect` for validators you have shares for (`get_validator_and_cluster` fails otherwise)
   - Therefore: unknown validators' partial sigs are stored but **never reconstructed**

3. **Post-consensus**: Depends on whether decided data contains a validator list

   **Why `Role::Committee` doesn't need explicit share check:**
   - Decided data is `BeaconVote` (just: block_root, source, target)
   - No validator list in decided data - each validator signs independently
   - Same architectural filtering: `sign_attestation(validator_pubkey)` only called for known validators

   **Why `Role::AggregatorCommittee` also uses architectural filtering (with Solution A):**

   **Key Insight**: shares = DutiesService validators (same source of truth via `voting_pubkeys()`)

   With Solution A implemented (see Tasks 12/13), AggregatorCommittee now also uses architectural filtering:
   - Decided data is `AggregatorCommitteeConsensusData` which contains validator lists
   - However, Lighthouse drives the calls based on OUR shares, not decided data
   - If a validator is in decided_data but we don't have a share, Lighthouse won't call for it
   - If a validator is in our shares but not in decided_data, `ValidatorNotInConsensus(ValidatorIndex)` is returned early
   - `num_signatures_to_collect` is computed from decided_data filtered by our shares

   **Example - Committee (no explicit check needed):**
   ```
   // Each validator independently calls sign_attestation
   sign_attestation(V1, ...)  // V1 registers notifier, gets own signature
   sign_attestation(V2, ...)  // V2 registers notifier, gets own signature
   // No iteration over "decided validator list" - BeaconVote is just data
   ```

   **Example - AggregatorCommittee (Solution A - architectural filtering):**
   ```
   // After QBFT, we get decided AggregatorCommitteeConsensusData
   // Proposer (another operator) included: [V1, V2, V3]
   // But we only have shares for V1 and V2!

   // Lighthouse calls based on OUR shares (same source of truth)
   produce_signed_aggregate_and_proof(V1, ...)  // V1 in decided, sign
   produce_signed_aggregate_and_proof(V2, ...)  // V2 in decided, sign
   // Lighthouse does NOT call for V3 - we don't have share, not in voting_pubkeys()

   // If our local view had MORE validators than decided (another op's proposal won):
   // Our shares: [V1, V2, V3]
   // Decided data: [V1, V2]  (proposer only had V1, V2)
   produce_signed_aggregate_and_proof(V1, ...)  // V1 in decided, sign
   produce_signed_aggregate_and_proof(V2, ...)  // V2 in decided, sign
   produce_signed_aggregate_and_proof(V3, ...)  // V3 NOT in decided → ValidatorNotInConsensus(V3)

   // num_signatures_to_collect = 2 (from decided_data filtered by shares)
   // After V1 + V2 signed → envelope sent, V3 rejection doesn't block
   ```

   | Role | Decided Data | Contains Validator List? | Post-Consensus Check |
   |------|--------------|-------------------------|---------------------|
   | Committee | `BeaconVote` | No | Architectural |
   | AggregatorCommittee | `AggregatorCommitteeConsensusData` | Yes | Architectural (Solution A) |

**Why No Filtering at Receive Time**:
- **Simpler**: No need for validator lookup logic in hot path
- **Consistent**: Matches Go SSV's `basePartialSigMsgProcessing` pattern
- **Safe**: Unused collectors are cleaned up by slot-based pruning
- **Efficient**: Collector lookup is O(1) hashmap operation

**Files to verify** (no changes expected):
- `anchor/signature_collector/src/lib.rs` - Verify stores partial sigs without filtering
- `anchor/message_receiver/src/lib.rs` - Verify no validator membership checks

**What This Task Documents**:
This task documents that **no filtering is needed at receive time** for committee roles, and explains how divergent views are handled:

1. **Pre-consensus** (Tasks 8/9): **Architectural filtering** - no explicit share check needed
   - `sign_and_collect` is only called for validators you know about
   - Calling it registers a notifier which sets the `threshold` in the collector
   - Collectors without a threshold NEVER reconstruct, even with quorum of partial sigs
   - Unknown validators' partial sigs are stored but architecturally cannot trigger reconstruction

2. **Post-consensus** (Tasks 12/13): **Architectural filtering via Solution A**
   - Key insight: shares = DutiesService validators (same source of truth via `voting_pubkeys()`)
   - Lighthouse drives calls based on OUR shares, not decided data
   - If validator is in our shares but not in decided_data → `ValidatorNotInConsensus(ValidatorIndex)` returned early
   - If validator is in decided_data but we don't have share → Lighthouse won't call for it
   - `num_signatures_to_collect` is computed from decided_data filtered by our shares
   - No explicit iteration over decided validator list needed

**Tests to add**:
- Unit test: `test_receive_partial_sigs_stores_unknown_validators` - Verify partial sigs for unknown validators are stored without error
- Unit test: `test_collector_cleanup_removes_unused_instances` - Verify collectors for unknown validators are cleaned up
- Integration test: Operators with divergent views still reach consensus

**Done when**:
- Verified that partial signature reception stores all sigs without filtering
- Verified that unknown validators' partial sigs don't cause errors
- Documentation clarifies the "store blindly, filter at processing" pattern

**Fork Gating**: ❌ Not needed - This is existing behavior documentation, not a change.

---

### Task 17: Add Fallback Signature Verification

**Prerequisites**: Task 16

**Motivation**: From [Go SSV PR #2503 discussion](https://github.com/ssvlabs/ssv/pull/2503#discussion_r2658112575):
> "Spec has an optimistic policy for quorums of messages. For the first msgs, the BLS signatures are not verified. Only once a quorum is reached, the validator signature is reconstructed, and only this single one is verified.
>
> If all goes well (the reconstructed sig succeeds), we only verify 1 BLS verification (the above one). Else, if it goes wrong, we fall back to verify each individual partial BLS signature, and remove the invalid ones.
>
> In case `FallBackAndVerifyEachSignature` is called, previous roots, considered to have an optimistic quorum, may end up not having a quorum anymore (due to the removed sigs)."

**Additional Optimization** (from Go SSV commits 74f9c54a8, a1e1c4f8a - Jan 20, 2026):
When processing multiple signing roots and fallback verification is triggered:
- Only verify partial signatures for the current root and remaining roots (`roots[i:]`)
- Skip verification for already-processed roots (before index `i`)
- This prevents redundant verification of roots that were successfully reconstructed

**Files to modify**:
- `anchor/signature_collector/src/lib.rs`
- `anchor/common/bls_lagrange/src/lib.rs` (potentially add verify function)

**Problem**:
When `combine_signatures` fails (due to invalid partial signatures from malicious operators), Anchor currently just logs an error and exits the collector. We should instead:
1. Verify each partial signature individually
2. Remove invalid ones
3. Re-check if we still have quorum
4. Retry reconstruction with valid sigs only

**Changes**:
```rust
// In signature_collector.rs, modify the combine_signatures error handling:

async fn signature_collector(mut rx: mpsc::UnboundedReceiver<CollectorMessage>) {
    // ... existing setup ...

    while let Some(message) = rx.recv().await {
        // ... existing message handling ...

        if let Some(threshold) = threshold
            && signature_share.len() as u64 >= threshold
        {
            match combine_signatures(mem::take(&mut signature_share)) {
                Ok(signature) => {
                    let signature = Arc::new(signature);
                    trace!(?signature, "Successfully recovered signature");
                    for notifier in mem::take(&mut notifiers) {
                        if notifier.send(Arc::clone(&signature)).is_err() {
                            warn!("Callback dropped - signature is no longer relevant");
                        }
                    }
                    full_signature = Some(signature);
                }
                Err(err) => {
                    // FALLBACK: Verify each partial signature individually
                    warn!(
                        ?err,
                        share_count = signature_share.len(),
                        "Lagrange interpolation failed - falling back to individual verification"
                    );

                    // Verify each partial sig and keep only valid ones
                    let valid_shares = fallback_verify_signatures(
                        &signature_share,
                        &validator_pubkey,  // Need to pass this in
                        &signing_root,      // Need to pass this in
                    ).await;

                    // Check if we still have quorum after removing invalid sigs
                    if valid_shares.len() as u64 >= threshold {
                        match combine_signatures(valid_shares) {
                            Ok(sig) => {
                                let signature = Arc::new(sig);
                                warn!(
                                    "Recovered signature after removing invalid partial sigs"
                                );
                                for notifier in mem::take(&mut notifiers) {
                                    let _ = notifier.send(Arc::clone(&signature));
                                }
                                full_signature = Some(signature);
                            }
                            Err(err) => {
                                error!(
                                    ?err,
                                    "Failed to recover signature even after verification fallback"
                                );
                                // Instance will be cleaned up, notifiers will timeout
                            }
                        }
                    } else {
                        warn!(
                            valid_count = valid_shares.len(),
                            threshold,
                            "Lost quorum after removing invalid signatures"
                        );
                        // Put remaining valid sigs back, wait for more
                        signature_share = valid_shares;
                    }
                }
            }
        }
    }
}

/// Verify each partial signature individually, returning only valid ones.
/// This is the "fallback" path when Lagrange interpolation fails.
async fn fallback_verify_signatures(
    shares: &HashMap<OperatorId, Signature>,
    validator_pubkey: &PublicKey,
    signing_root: &Hash256,
) -> HashMap<OperatorId, Signature> {
    let mut valid_shares = HashMap::new();

    for (operator_id, signature) in shares {
        // Verify partial signature
        // Note: This requires knowing the operator's BLS public key share
        // which maps to the validator's threshold scheme
        if verify_partial_signature(signature, validator_pubkey, signing_root, operator_id) {
            valid_shares.insert(*operator_id, signature.clone());
        } else {
            error!(
                ?operator_id,
                "Removed invalid partial signature from operator"
            );
            // TODO: Consider recording this for slashing evidence
        }
    }

    valid_shares
}
```

**Implementation Note for `roots[i:]` Optimization**:
When processing multiple signing roots (e.g., for sync committee contributions), apply the optimization from Go SSV commits 74f9c54a8 and a1e1c4f8a:
```rust
// In validator_store when processing multiple roots
for (i, root) in roots.iter().enumerate() {
    match signature_collector.collect(root, ...) {
        Ok(sig) => { /* Success, continue to next root */ },
        Err(CollectionError::InvalidSignatures(invalid_ops)) => {
            // Fallback verification triggered
            // Pass remaining_roots = &roots[i..] to fallback handler
            // This ensures we only verify sigs for unprocessed roots
            // Roots before index i were already successfully reconstructed
            fallback_verify_and_retry(&roots[i..], invalid_ops)?;
        }
    }
}
```

**Note on Implementation Complexity**:
Verifying partial signatures requires access to the operator's public key shares, which may require:
1. Adding operator public key share lookup to SignatureCollector
2. Passing additional context when spawning collectors
3. Potentially restructuring the collector to have access to cluster metadata

This task may need to be split into subtasks depending on complexity.

**Tests to add**:
- Unit test: Valid shares pass verification
- Unit test: Invalid shares are removed
- Unit test: Reconstruction succeeds after removing one bad share
- Unit test: Quorum lost after removing too many bad shares
- Unit test: `roots[i:]` optimization - verify fallback only processes remaining roots
- Integration test: Malicious operator sends bad sigs, others still succeed
- Integration test: Multi-root scenario where fallback on root[1] only verifies roots[1..], not root[0]

**Done when**: Signature collection is resilient to malicious operators sending invalid partial signatures.

**Fork Gating**: ❌ Not needed - This is defensive handling for all signature collection flows.

---

### Task 18: Signature Collector Context Enhancement

**Prerequisites**: Task 17 (design dependency)

**Motivation**: Task 17 requires additional context in the SignatureCollector to verify partial signatures. This task adds that context.

**Files to modify**:
- `anchor/signature_collector/src/lib.rs`

**Problem**:
Currently, `SignatureCollector` only tracks:
- `signing_root: Hash256`
- `validator_index: ValidatorIndex`
- `signature_share: HashMap<OperatorId, Signature>`
- `threshold: u64`

For fallback verification, we also need:
- Validator public key (to verify reconstructed signature)
- Operator public key shares (to verify individual partial sigs)
- Committee structure (operator IDs and their share indices)

**Changes**:
```rust
// Add new message type to pass verification context
enum CollectorMessageKind {
    RegisterNotifier {
        notify: oneshot::Sender<Arc<Signature>>,
        threshold: u64,
    },
    PartialSignature {
        operator_id: OperatorId,
        signature: Box<Signature>,
    },
    // NEW: Provide context for fallback verification
    SetVerificationContext {
        validator_pubkey: PublicKeyBytes,
        committee: Arc<Committee>,  // Contains operator shares
    },
}

// Modify sign_and_collect to send verification context
pub async fn sign_and_collect(
    self: &Arc<Self>,
    metadata: SignatureMetadata,
    requester: SignatureRequester,
    validator_signing_data: ValidatorSigningData,
    verification_context: Option<VerificationContext>,  // NEW parameter
) -> Result<Arc<Signature>, CollectionError> {
    // ... existing code ...

    // If verification context provided, send it to collector
    if let Some(ctx) = verification_context {
        let sender = self.get_or_spawn(
            validator_signing_data.root,
            validator_signing_data.index,
            metadata.slot,
        );
        let _ = sender.send(CollectorMessage {
            kind: CollectorMessageKind::SetVerificationContext {
                validator_pubkey: ctx.validator_pubkey,
                committee: ctx.committee,
            },
            _drop_on_finish: DropOnFinish::default(),
        });
    }

    // ... rest of existing code ...
}

/// Context needed for fallback signature verification
#[derive(Clone)]
pub struct VerificationContext {
    pub validator_pubkey: PublicKeyBytes,
    pub committee: Arc<Committee>,
}
```

**Tests to add**:
- Unit test: Verification context is stored and accessible
- Unit test: Fallback verification uses correct operator shares

**Done when**: SignatureCollector has access to all information needed for fallback verification.

**Fork Gating**: ❌ Not needed - Internal enhancement.

---

### Task 19: Rename ValidatorConsensusData to ProposerConsensusData

**Rationale**: Per [SSV-Spec PR #572 commit e51714](https://github.com/ssvlabs/ssv-spec/pull/572/commits/e51714698a47543079a90032b94aab0f46b5d3b8#diff-425437b4b7b0707d0416f8784ff9da39af292f2e94dab2adc39a97ad1ec8dce2), the spec renames `ValidatorConsensusData` to `ProposerConsensusData` to better reflect its purpose (used only for proposer duties, not all validator duties).

**Scope**: This is a codebase-wide rename affecting ~88 occurrences across multiple crates.

**Files to modify**:

| File | Changes |
|------|---------|
| `anchor/common/ssv_types/src/consensus.rs` | Rename struct `ValidatorConsensusData` → `ProposerConsensusData`, `ValidatorConsensusDataValidator` → `ProposerConsensusDataValidator`, `ValidatorConsensusDataLen` → `ProposerConsensusDataLen`, update all impl blocks and tests (~52 occurrences) |
| `anchor/qbft_manager/src/lib.rs` | Rename field `validator_consensus_data_instances` → `proposer_consensus_data_instances`, update imports and type references (~15 occurrences) |
| `anchor/validator_store/src/lib.rs` | Update imports and local variable names (~12 occurrences) |
| `anchor/qbft_manager/src/tests.rs` | Update test references |
| Documentation files (10 files) | Update type references in markdown |

**Related types to keep unchanged**:
- `ValidatorDuty` - Still used by proposer consensus data, name is appropriate
- `ValidatorDutyKind` - Enum in qbft_manager, separate concern
- `ValidatorInstanceId` - Could optionally rename to `ProposerInstanceId` for consistency

**Implementation approach**:
```bash
# Use IDE refactoring or careful find/replace:
# 1. Rename the struct and its validator
ValidatorConsensusData → ProposerConsensusData
ValidatorConsensusDataValidator → ProposerConsensusDataValidator
ValidatorConsensusDataLen → ProposerConsensusDataLen

# 2. Rename snake_case variables
validator_consensus_data → proposer_consensus_data
validator_consensus_data_instances → proposer_consensus_data_instances
validator_consensus_data_validator → proposer_consensus_data_validator
```

**Tests to verify**:
- All existing tests pass after rename
- SSZ encoding remains unchanged (rename is cosmetic)
- Wire compatibility unaffected

**Done when**: All references renamed, tests pass, clippy clean.

**Fork Gating**: ❌ Not needed - Internal refactoring only, no wire format changes.

**Priority**: Low - This is a naming consistency improvement, not a functional change. Can be done as a separate PR after AggregatorCommittee implementation is complete.

---

## 4. Test Plan for Interoperability

### 4.1 Wire Compatibility Tests

| Test | Input | Expected Output |
|------|-------|-----------------|
| Role encoding | `Role::AggregatorCommittee` | `[6, 0, 0, 0]` |
| PartialSigKind encoding | `AggregatorCommitteePartialSig` | `[6, 0, 0, 0, 0, 0, 0, 0]` |
| MessageId encoding | Role + Committee executor | Bytes 4-7 = `[6,0,0,0]`, bytes 24-55 = CommitteeId |
| AssignedAggregator roundtrip | Rust struct | Same bytes as Go encoding |
| AggregatorCommitteeConsensusData roundtrip | Rust struct with all fields | Same bytes as Go encoding |

### 4.2 Behavior Parity Tests

| Test | Scenario | Anchor Behavior | Go SSV Behavior |
|------|----------|-----------------|-----------------|
| Empty duty rejection | Duty with no aggregators/contributors | Return error | Return error |
| Selection proof domains | Attestation vs sync | Different signing roots | Same domains |
| Batch counting | 3 attestation + 2 sync validators | 5 signatures expected | 5 signatures expected |
| Duplicate rejection | Same committee index twice | Validation fails | Validation fails |
| Submission tracking | Aggregator submits twice | Second ignored | Second ignored |

### 4.3 Integration Tests

| Test | Setup | Steps | Verification |
|------|-------|-------|--------------|
| Full pre-consensus flow | 4 operators, 3 validators | Each produces selection proofs | Single batched message per operator |
| QBFT consensus | 4 operators with same consensus data | Run QBFT to completion | All operators decide same value |
| Post-consensus signing | Decided consensus data | Extract and sign aggregates | Valid signed aggregates |
| Cross-operator message exchange | Anchor + Go SSV operators | Exchange messages | Messages parse correctly |

### 4.4 Suggested Fixtures

**Test Keys**:
```rust
const TEST_OPERATOR_KEYS: [&str; 4] = [
    "0x...",  // Operator 1 RSA key
    "0x...",  // Operator 2
    "0x...",  // Operator 3
    "0x...",  // Operator 4
];

const TEST_VALIDATOR_SHARES: [&str; 3] = [
    "0x...",  // Validator 1 BLS share
    "0x...",  // Validator 2
    "0x...",  // Validator 3
];
```

**Test Committee Config**:
```rust
let test_committee = Committee {
    id: CommitteeId::from([/* 32 bytes */]),
    operators: vec![1, 2, 3, 4],
    threshold: 3,
    validators: vec![validator1, validator2, validator3],
};
```

**Sample Messages** (generate from Go SSV):
- Pre-consensus PartialSignatureMessages with AggregatorCommitteePartialSig
- QBFT Proposal with AggregatorCommitteeConsensusData
- Post-consensus PartialSignatureMessages

### 4.5 Cross-Implementation Test Strategy

**Message Vector Testing**:
1. Generate test messages in Go SSV
2. Save as JSON/binary test fixtures
3. Load in Anchor tests
4. Verify parsing and validation

**Network-Level Testing** (future):
1. Run Anchor node and Go SSV node
2. Configure with same committee/validators
3. Trigger aggregator duties
4. Verify both participate in consensus
5. Verify both submit to beacon node

### 4.6 Edge Case Tests (Tasks 16-18)

| Test | Scenario | Expected Behavior |
|------|----------|-------------------|
| **Divergent Validator Sets** | Receive partial sig for validator not in our view | Log and skip gracefully, no error |
| **Fallback Verification - Success** | One malicious operator sends invalid partial sig | Remove invalid sig, reconstruct with remaining 3 (if threshold met) |
| **Fallback Verification - Quorum Lost** | Two malicious operators send invalid partial sigs | Log warning, wait for more valid sigs |
| **Concurrent Partial Sigs** | Multiple partial sigs arrive simultaneously | Single collector handles all sequentially |
| **Unknown Committee** | Receive message for committee we don't know | Skip gracefully, log debug info |

**Malicious Operator Test Scenarios**:
```rust
#[tokio::test]
async fn test_fallback_with_one_invalid_signature() {
    // Setup: 4 operators, threshold 3
    let collector = setup_collector_with_threshold(3);

    // Send 3 valid partial sigs
    collector.receive(valid_sig_from_operator(1));
    collector.receive(valid_sig_from_operator(2));
    collector.receive(valid_sig_from_operator(3));

    // Send 1 invalid partial sig (wrong signing root)
    collector.receive(invalid_sig_from_operator(4));

    // Should reconstruct successfully with operators 1, 2, 3
    let result = collector.await_result().await;
    assert!(result.is_ok());
}

#[tokio::test]
async fn test_fallback_loses_quorum() {
    // Setup: 4 operators, threshold 3
    let collector = setup_collector_with_threshold(3);

    // Send only 3 partial sigs, 2 are invalid
    collector.receive(valid_sig_from_operator(1));
    collector.receive(invalid_sig_from_operator(2));
    collector.receive(invalid_sig_from_operator(3));

    // Lagrange fails, fallback removes 2 invalid, only 1 valid remains
    // Quorum lost - should wait for more sigs
    let result = collector.try_await_result(Duration::from_millis(100)).await;
    assert!(result.is_none()); // Still waiting

    // Add 2 more valid sigs
    collector.receive(valid_sig_from_operator(4));
    // Now we have 2 valid (ops 1 and 4), still need 1 more
    // ... continue until quorum reached
}

#[tokio::test]
async fn test_divergent_validator_set_skipped() {
    let receiver = setup_message_receiver();

    // Create partial sig message for validator we don't know about
    let unknown_validator = ValidatorIndex::from(99999);
    let message = create_partial_sig_message(unknown_validator, &signing_root);

    // Should skip without error
    let result = receiver.process_partial_sig(message, committee_id);
    assert!(result.is_ok());

    // Verify it was logged but not processed
    assert!(!receiver.has_collector_for(signing_root, unknown_validator));
}
```

---

## 5. Risk Log + Mitigations

### 5.1 Phase Translation Risks

| Risk | Impact | Mitigation |
|------|--------|------------|
| Go runner phases don't map cleanly to Anchor modules | Incorrect behavior | Map each Go function to specific Anchor function; create behavior equivalence tests |
| Timing differences between Go polling and Anchor watch channels | Race conditions | Implement timeout fallbacks; test edge cases |
| State management differs (Go uses runner state, Anchor uses watch channels) | Lost state | Document state flow; add tracing for debugging |

### 5.2 Message Routing Risks

| Risk | Impact | Mitigation |
|------|--------|------------|
| MessageId uses Committee executor for new role | Routing failures | Unit test MessageId construction; verify bytes 24-55 usage |
| QBFT manager doesn't handle new instance type | Messages dropped | Add explicit routing case; log unrouted messages |
| Role enum value collision | Interop failure | Verify value 6 matches Go exactly |

### 5.3 Timing Risks

| Risk | Impact | Mitigation |
|------|--------|------------|
| SelectionProofMetadata not ready when selection proofs start | Timeout | Implement wait with timeout; fallback to per-validator mode |
| DutiesService signals delayed | Missed slot | Add slot boundary checks; log late signals |
| 2/3 slot deadline missed for aggregate fetch | No aggregation | Monitor timing; optimize beacon API calls |

### 5.4 Domain/Signing Risks

| Risk | Impact | Mitigation |
|------|--------|------------|
| Wrong domain used for signing | Invalid signatures | Unit test each domain; compare with Go test vectors |
| Signing root computation differs | Signature verification fails | Test signing roots match Go exactly |
| Sync committee subcommittee_index off-by-one | Wrong contributions | Verify SyncAggregatorSelectionData encoding |

### 5.5 Committee Bits/Selection Risks

| Risk | Impact | Mitigation |
|------|--------|------------|
| IsAggregator logic differs from Go | Wrong validators selected | Match Go implementation exactly; test edge cases |
| Committee bits handling for Electra differs | Attestation aggregation fails | Test with Electra attestations |
| Subnet calculation differs | Wrong sync contributions | Test SyncSubnetId computation |

### 5.6 QBFT Instance Lifetime Risks

| Risk | Impact | Mitigation |
|------|--------|------------|
| Instance not cleaned up after slot | Memory leak | Add cleanup on slot boundary |
| Instance created too early | Wasted resources | Lazy creation on first message |
| Multiple instances for same committee/slot | Consensus split | Use instance_height as dedup key |

### 5.7 Caching/Persistence Risks

| Risk | Impact | Mitigation |
|------|--------|------------|
| SelectionProofMetadata cached for wrong slot | Wrong batch count | Verify slot before using cached data |
| Submission tracker not persisted | Duplicate submissions on restart | Accept duplicates (beacon node handles); log warning |
| Consensus data not cached | Re-fetch on every post-consensus call | Cache decided value by (slot, committee) |

### 5.8 UNKNOWN Items Requiring Investigation

| Item | What to Inspect | Location |
|------|-----------------|----------|
| Exact SSZ max sizes for aggregators/contributors | Go struct annotations | `ssv-spec/types/consensus_data.go` |
| Exact attestation bytes format by fork | Go encoding logic | `ssv/runner/aggregator_committee.go` fork handling |
| DutiesService watch channel API | Lighthouse upstream PR | N/A - may need to create |
| Sync committee "lag by 1" slot handling | Go sync duty logic | `ssv/runner/aggregator_committee.go` |

### 5.9 Signature Collection Edge Case Risks

| Risk | Impact | Mitigation |
|------|--------|------------|
| Malicious operator sends invalid partial sigs | Lagrange interpolation fails | Task 17: Fallback verification and removal |
| Divergent validator sets between operators | Processing sigs for unknown validators | Task 16: Skip unknown validators gracefully |
| Lost quorum after removing invalid sigs | Signature never reconstructs | Log warning; wait for more valid sigs |
| Operator public key shares not accessible | Cannot verify partial sigs | Task 18: Add verification context to collector |

---

## Summary

This plan implements AggregatorCommittee duties in Anchor through 19 sequential tasks:

1. **Tasks 1-5**: Type additions (Role, PartialSigKind, AssignedAggregator, ConsensusData, Validation)
2. **Tasks 6-9**: Pre-consensus changes (Metadata, MetadataService, selection proof batching)
3. **Tasks 10-11**: Message validation and QBFT routing
4. **Task 11b**: Duty execution orchestration (build consensus data, start QBFT)
5. **Tasks 12-14**: Post-consensus signing and submission tracking - **Tasks 12/13 FIX IMPLEMENTED**
6. **Task 15**: Wire compatibility tests
7. **Tasks 16-18**: Edge case handling for Go SSV interoperability:
   - Task 16: Divergent validator set handling (architectural filtering via Solution A)
   - Task 17: Fallback signature verification (malicious operator resilience)
   - Task 18: Signature collector context enhancement (verification support)

Each task has clear prerequisites, specific file changes, tests, and done criteria. The plan addresses all behaviors from SSV-Spec PR #572 and Go SSV PR #2503 while respecting Anchor's modular architecture without runners.

### Key Design Insight: shares = DutiesService validators

A critical finding documented in `POST_CONSENSUS_AGGREGATION_SIGNING_DESIGN.md` that simplifies post-consensus handling:

```
ValidatorAdded event
  → stored in database.shares()
  → AnchorValidatorStore.voting_pubkeys() reads from shares
  → DutiesService.poll_beacon_attesters() uses voting_pubkeys()
  → DutiesService calls produce_signed_aggregate_and_proof for those validators
```

This means:
- Lighthouse drives calls based on OUR shares (same source of truth)
- The only divergence comes from QBFT consensus where another operator's proposal may win
- **Solution A** (implemented in Tasks 12/13): Use `decided_data.aggregators/contributors` filtered by our shares for `num_signatures_to_collect`
- **Solution B** (NOT NEEDED): Complex caching and driving signing from decided data is unnecessary

### Related Documentation

| Document | Purpose |
|----------|---------|
| [POST_CONSENSUS_AGGREGATION_SIGNING_DESIGN.md](./POST_CONSENSUS_AGGREGATION_SIGNING_DESIGN.md) | **Key insight**: shares = DutiesService validators, Solution A fix for post-consensus |
| [AGGREGATOR_COMMITTEE_CONSENSUS_DATA_GUIDE.md](./AGGREGATOR_COMMITTEE_CONSENSUS_DATA_GUIDE.md) | Detailed guide for building consensus data at 2/3 slot |
| [AGGREGATOR_COMMITTEE_PARTIAL_SIG_FLOW.md](./AGGREGATOR_COMMITTEE_PARTIAL_SIG_FLOW.md) | Complete flow trace for partial signature lifecycle |
| [COMMITTEE_AGGREGATOR_ARCHITECTURE.md](./COMMITTEE_AGGREGATOR_ARCHITECTURE.md) | Architecture overview |
| [SELECTION_PROOF_BATCHING_ARCHITECTURE.md](./SELECTION_PROOF_BATCHING_ARCHITECTURE.md) | Selection proof batching design |
| [MESSAGE_VALIDATION_QBFT_ROUTING_COMPARISON.md](./MESSAGE_VALIDATION_QBFT_ROUTING_COMPARISON.md) | Go SSV comparison for validation |

### Anchor Architecture Advantages

Several edge cases that require explicit handling in Go SSV are naturally handled by Anchor's architecture:

| Edge Case | Go SSV | Anchor |
|-----------|--------|--------|
| Per-(root, validator) quorum tracking | Explicit loop with `HasQuorum(validator, root)` | Collector keyed by `(signing_root, validator_index)` |
| Concurrent fallback coordination | Complex goroutine coordination needed | Single-task-per-collector design |
| Divergent validator set post-consensus | Iterate over decided data, check share for each | Lighthouse drives calls from shares (same source of truth) |
| Post-consensus count mismatch | N/A (drives from decided) | Solution A: `num_signatures_to_collect` from decided_data filtered by shares |
| Threshold check before reconstruction | Explicit `if !gotQuorum` | `signature_share.len() >= threshold` pattern |

See **Section 1.9** and **Section 1.10** for detailed edge case analysis and architectural comparison.
