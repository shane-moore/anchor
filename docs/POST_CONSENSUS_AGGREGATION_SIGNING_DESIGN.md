# Post-Consensus Aggregation Signing Design

## Problem Statement

For Boole+ fork, `produce_signed_aggregate_and_proof` uses committee-based consensus with `AggregatorCommitteeConsensusData`. After consensus, we need to send a single `PartialSignatureMessages` envelope containing partial signatures for ALL aggregators in the committee.

### Key Insight: Shares = DutiesService Validators

**Critical finding**: The SSV database shares and Lighthouse's DutiesService use the **same source of truth**:

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

### The Actual Problem

The issue is a mismatch between:
1. **Our local view** (shares/DutiesService): What we expect to sign
2. **Decided consensus data**: What QBFT actually decided (may differ if another operator's proposal won)

## Current Architecture

### Data Flow

```
┌─────────────────────────────────────────────────────────────────────────┐
│                    MetadataService (2/3 slot)                           │
│  Builds AggregatorCommitteeConsensusData per SSV committee              │
│  Stores in AggregationAssignments.consensus_data_by_ssv_committee       │
└─────────────────────────────────────────────────────────────────────────┘
                                    │
                                    ▼
┌─────────────────────────────────────────────────────────────────────────┐
│              Lighthouse calls produce_signed_aggregate_and_proof        │
│              (called per-validator based on Lighthouse's duties)        │
└─────────────────────────────────────────────────────────────────────────┘
                                    │
                    ┌───────────────┴───────────────┐
                    ▼                               ▼
            Validator A                      Validator B
                    │                               │
                    └───────────┬───────────────────┘
                                ▼
┌─────────────────────────────────────────────────────────────────────────┐
│                         QBFT Consensus                                  │
│  Instance ID: AggregatorCommitteeInstanceId { committee, slot }         │
│  Data: AggregatorCommitteeConsensusData                                 │
│  (Same instance for all validators in committee - first caller starts)  │
└─────────────────────────────────────────────────────────────────────────┘
                                    │
                                    ▼
┌─────────────────────────────────────────────────────────────────────────┐
│                    Decided AggregatorCommitteeConsensusData             │
│  aggregators: [                                                         │
│    { validator_index: 100, selection_proof: sig_A, committee_index: 5 },│
│    { validator_index: 200, selection_proof: sig_B, committee_index: 7 } │
│  ]                                                                      │
│  aggregator_committee_indexes: [5, 7]                                   │
│  aggregated_attestations: [att_5_bytes, att_7_bytes]                    │
└─────────────────────────────────────────────────────────────────────────┘
                                    │
                                    ▼
┌─────────────────────────────────────────────────────────────────────────┐
│              Post-Consensus Signing (per validator call)                │
│                                                                         │
│  1. Extract this validator's aggregate from decided data                │
│  2. Build AggregateAndProof with decided selection_proof                │
│  3. Sign → unique signing_root per validator                            │
│  4. Add to signature_collector's committee_signatures                   │
│  5. When len == num_signatures_to_collect → send envelope               │
└─────────────────────────────────────────────────────────────────────────┘
```

### Signature Collector Behavior (CollectionMode::Committee)

```rust
// Key: (base_hash, committee_id) where base_hash = decided_data.hash()
committee_signatures: DashMap<(Hash256, CommitteeId), CommitteeSignatures>

struct CommitteeSignatures {
    collected_signatures: Vec<PartialSignatureMessage>,  // One per validator
    for_slot: Slot,
}

// Each PartialSignatureMessage has:
// - signing_root: UNIQUE per validator (hash of their AggregateAndProof)
// - validator_index: the validator being signed for
// - partial_signature: our operator's partial sig
// - signer: our operator ID
```

**Flow per `collect_signature` call:**
1. Look up or create entry in `committee_signatures[(base_hash, committee_id)]`
2. Push this validator's `PartialSignatureMessage` to `collected_signatures`
3. If `collected_signatures.len() == num_signatures_to_collect`:
   - Remove entry, create `PartialSignatureMessages` envelope with ALL signatures
   - Broadcast envelope to network

### Current num_signatures_to_collect Calculation

```rust
// CURRENT (problematic): Uses local AggregationAssignments
let num_signatures_to_collect = aggregation_assignments
    .attestation_aggregator_count(|idx| committee_validator_indices.contains(idx));
```

## Edge Cases Analysis

### Case 1: Our local view has MORE validators than decided data (THE BUG)

**Scenario**: Another operator's proposal with fewer validators won QBFT consensus.

```
Our shares:                  [A, B, C]
Another op proposes:         [A, B]     (they only have A, B synced)
Decided data aggregators:    [A, B]     (their proposal won QBFT)
Lighthouse calls for:        [A, B, C]  (based on our shares)
Our aggregation_assignments: [A, B, C]  (built locally from our shares)
```

**Flow with BUGGY code (uses aggregation_assignments):**
1. Lighthouse calls for C → lookup in decided_data.aggregators → NOT FOUND
2. Return error: `ValidatorNotInConsensus` ✓ (C is rejected early)
3. Lighthouse calls for A → found, sign, collected = 1
4. Lighthouse calls for B → found, sign, collected = 2
5. **BUG**: num_signatures_to_collect = 3 (from aggregation_assignments)
6. collected = 2, need = 3 → WAIT FOREVER → TIMEOUT ✗

**Flow with CORRECT code (uses decided_data):**
1. Same as above through step 4
5. **CORRECT**: num_signatures_to_collect = 2 (from decided_data filtered by our shares)
6. collected = 2, need = 2 → SEND ENVELOPE ✓

**Result: BUG in current code** - Must use decided_data, not aggregation_assignments.

### Case 2: Lighthouse calls match decided data exactly (HAPPY PATH)

```
Our shares:                  [A, B]
Decided data aggregators:    [A, B]
Lighthouse calls for:        [A, B]
num_signatures_to_collect:   2 (same either way)
```

**Result: WORKS** - Both approaches produce correct count.

### Case 3: Decided data has validators we don't have shares for

**Scenario**: Another operator's proposal won with more validators than we have shares for.

```
Another op has shares:       [A, B, C]
Our shares:                  [A, B]      (we don't have C's share)
Decided data aggregators:    [A, B, C]   (their proposal won)
Lighthouse calls for:        [A, B]      (based on OUR shares, not decided)
num_signatures_to_collect:   2 (from decided [A,B,C] filtered by our shares [A,B])
```

**Flow:**
1. Lighthouse calls for A → found in decided, we have share, sign, collected = 1
2. Lighthouse calls for B → found in decided, we have share, sign, collected = 2 → SEND ENVELOPE ✓
3. Lighthouse does NOT call for C (we don't have share, so not in voting_pubkeys)

**Result: WORKS** - We sign for our subset, other operators sign for theirs.

### Case 4: Different operators have different shares (Divergent Views)

```
Operator 1 shares: [A, B]
Operator 2 shares: [A, B, C]
Operator 3 shares: [B, C]
Decided data:      [A, B, C]
```

Each operator computes different `num_signatures_to_collect`:
- Operator 1: 2 (A, B)
- Operator 2: 3 (A, B, C)
- Operator 3: 2 (B, C)

Each operator signs different subsets and sends envelopes with different counts.
**This is expected and correct** - each operator can only sign for their shares.

The receiving side (signature reconstruction) handles this:
- Each signing_root + validator needs threshold partial sigs from different operators
- As long as enough operators sign for each validator, reconstruction works

## Comparison with sign_attestation

| Aspect | sign_attestation | produce_signed_aggregate_and_proof |
|--------|------------------|-----------------------------------|
| Consensus data | BeaconVote (block_root, source, target) | AggregatorCommitteeConsensusData |
| Specifies who signs? | NO - each operator uses local view | YES - `aggregators` list in decided data |
| Signing root | SAME for all validators (same AttestationData) | DIFFERENT per validator (unique AggregateAndProof) |
| num_signatures_to_collect source | Local VotingAssignments | Decided data (filtered by our shares) |
| Divergence impact | Minor - reconstruction still works | Major - missing sigs = missing aggregates |

**Key difference**: For attestations, consensus doesn't dictate WHO attests. For aggregation, consensus explicitly lists WHO aggregates. We must align with decided data.

## SSV-Go Approach

SSV-Go handles this in `ProcessConsensus`:

```go
// After QBFT decides
for i, aggProof := range aggProofs {
    validatorIndex := consensusData.Aggregators[i].ValidatorIndex

    // Skip validators we don't have shares for
    _, exists := r.BaseRunner.Share[validatorIndex]
    if !exists {
        continue  // Graceful skip, not error
    }

    // Sign and add to messages
    msg, err := signBeaconObject(ctx, r, vDuty, hashRoot, ...)
    messages = append(messages, msg)
}

// Send ONE envelope with ALL signatures we could produce
postConsensusMsg := &spectypes.PartialSignatureMessages{
    Type:     spectypes.PostConsensusPartialSig,
    Slot:     duty.DutySlot(),
    Messages: messages,
}
```

**Key insight**: SSV-Go iterates through decided data, not through externally-triggered calls.

## Proposed Solutions

### Solution A: Use Decided Data Count (RECOMMENDED)

With the corrected understanding that shares = DutiesService validators, Solution A is the correct fix:

```rust
// In produce_signed_aggregate_and_proof, after QBFT:
let num_signatures_to_collect = decided_data.aggregators
    .iter()
    .filter(|agg| committee_validator_indices.contains(&agg.validator_index))
    .count();
```

**Why this works:**
1. Lighthouse calls for ALL validators we have shares for (same source of truth)
2. Validators not in decided_data are rejected early with `ValidatorNotInConsensus`
3. `num_signatures_to_collect` correctly counts only validators in decided_data that we have shares for
4. When all decided validators we have shares for are processed → envelope sent

**This handles all cases correctly:**
- Case 1 (our local > decided): Extra validators rejected, count matches decided ✓
- Case 2 (local = decided): Works as expected ✓
- Case 3 (decided > our shares): We only count validators we have shares for ✓
- Case 4 (divergent operators): Each operator sends their own subset ✓

### Solution B: Drive Signing from Decided Data (NOT NEEDED)

Instead of waiting for Lighthouse to call per-validator, proactively sign all validators after consensus:

```rust
// After QBFT consensus completes (triggered by first produce_signed_aggregate_and_proof call):
fn sign_all_aggregators_from_decided_data(
    &self,
    decided_data: &AggregatorCommitteeConsensusData<E>,
    cluster: &Cluster,
    slot: Slot,
) -> Result<Vec<PartialSignatureMessage>, Error> {
    let mut messages = Vec::new();

    for aggregator in &decided_data.aggregators {
        // Skip validators we don't have shares for (like SSV-Go)
        let Some(validator) = self.get_validator_by_index(aggregator.validator_index) else {
            continue;
        };

        // Build AggregateAndProof from decided data
        let aggregate = self.decode_aggregate_for_validator(decided_data, aggregator)?;
        let message = AggregateAndProof::from_attestation(
            aggregator.validator_index.0 as u64,
            aggregate,
            SelectionProof::from(aggregator.selection_proof.clone()),
        );

        // Sign
        let domain_hash = self.get_domain(epoch, Domain::AggregateAndProof);
        let signing_root = message.signing_root(domain_hash);
        let partial_sig = self.sign_with_share(&validator, signing_root)?;

        messages.push(PartialSignatureMessage {
            signing_root,
            validator_index: aggregator.validator_index,
            partial_signature: partial_sig,
            signer: self.operator_id,
        });
    }

    Ok(messages)
}
```

**Envelope creation and sending:**
```rust
// Immediately after signing all, send envelope:
let envelope = PartialSignatureMessages {
    kind: PartialSignatureKind::PostConsensus,
    slot,
    messages: messages.into(),
};
self.send_partial_signature_envelope(envelope, committee_id)?;
```

**Cache results for Lighthouse:**
```rust
// Cache SignedAggregateAndProof results
// When Lighthouse calls produce_signed_aggregate_and_proof, return from cache
aggregation_results_cache: HashMap<(Slot, ValidatorIndex), SignedAggregateAndProof<E>>
```

### Solution B Implementation Outline

```
┌─────────────────────────────────────────────────────────────────────────┐
│           First produce_signed_aggregate_and_proof call                 │
│           (any validator in committee)                                  │
└─────────────────────────────────────────────────────────────────────────┘
                                    │
                                    ▼
┌─────────────────────────────────────────────────────────────────────────┐
│                         QBFT Consensus                                  │
│           (returns decided AggregatorCommitteeConsensusData)            │
└─────────────────────────────────────────────────────────────────────────┘
                                    │
                                    ▼
┌─────────────────────────────────────────────────────────────────────────┐
│            sign_all_aggregators_from_decided_data()                     │
│  - Iterate through decided_data.aggregators                             │
│  - Skip validators without shares                                       │
│  - Sign each → PartialSignatureMessage                                  │
│  - Collect all into Vec<PartialSignatureMessage>                        │
└─────────────────────────────────────────────────────────────────────────┘
                                    │
                    ┌───────────────┴───────────────┐
                    ▼                               ▼
┌─────────────────────────────┐   ┌─────────────────────────────────────┐
│   Send PartialSignature     │   │   Start signature collectors for    │
│   Messages envelope         │   │   each signing root (to receive     │
│   (broadcast to network)    │   │   partial sigs from other ops)      │
└─────────────────────────────┘   └─────────────────────────────────────┘
                                                    │
                                                    ▼
                                  ┌─────────────────────────────────────┐
                                  │   Cache results (keyed by slot +   │
                                  │   validator_index) for Lighthouse   │
                                  └─────────────────────────────────────┘
                                                    │
                                                    ▼
┌─────────────────────────────────────────────────────────────────────────┐
│         Subsequent produce_signed_aggregate_and_proof calls             │
│         (from Lighthouse for other validators)                          │
│                                                                         │
│  - Check cache for (slot, validator_index)                              │
│  - If found: wait for signature reconstruction, return result           │
│  - If not found (validator not in decided data): return error           │
└─────────────────────────────────────────────────────────────────────────┘
```

## Changes Required for Solution B

### 1. New method: `sign_all_aggregators_from_decided_data`
- Input: decided `AggregatorCommitteeConsensusData`, cluster, slot
- Output: `Vec<PartialSignatureMessage>`
- Iterates decided data, skips unknown validators, signs each

### 2. Modify signature collector or bypass it
- Option A: New method to send pre-built envelope directly
- Option B: Modify `CollectionMode::Committee` to accept pre-built messages

### 3. Add results cache
- `aggregation_results_cache: HashMap<(Slot, ValidatorIndex), oneshot::Receiver<SignedAggregateAndProof>>`
- Populated after signing all validators
- Resolved when signature reconstruction completes

### 4. Modify `produce_signed_aggregate_and_proof`
- First caller triggers QBFT + sign all + send envelope
- All callers wait on cache for their validator's result

### 5. Handle reconstruction
- Each validator's signing_root needs separate reconstruction
- Signature collector already handles this via `signature_collectors: DashMap<(Hash256, ValidatorIndex), SignatureCollector>`

## Open Questions

1. **Concurrency**: What if multiple Lighthouse calls arrive simultaneously? Need mutex/once-cell to ensure sign-all only happens once.

2. **Error handling**: If signing fails for one validator, should we still send partial envelope for others?

3. **Timeout behavior**: If reconstruction doesn't complete for some validators, how long do we wait?

4. **Cache cleanup**: When to clean up the results cache?

5. **Integration with existing collector**: Reuse `SignatureCollectorManager` or create parallel path?

## Resolution

**Chosen Solution**: Solution A - Use decided data count

**Key Insight**: shares = DutiesService validators (same source of truth via `voting_pubkeys()`)

**The Fix**:
```rust
// BEFORE (buggy):
let num_signatures_to_collect = aggregation_assignments
    .attestation_aggregator_count(|idx| committee_validator_indices.contains(idx));

// AFTER (correct):
let num_signatures_to_collect = decided_data.aggregators
    .iter()
    .filter(|agg| committee_validator_indices.contains(&agg.validator_index))
    .count();
```

This minimal change correctly handles all edge cases without architectural changes.
