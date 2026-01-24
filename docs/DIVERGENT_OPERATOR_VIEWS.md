# Complete Edge Case Flow: Operator 4 Missing Validator X

## Setup
- 4-operator cluster, 3-of-4 quorum
- Operators 1, 2, 3 have `Share[ValidatorX]` populated
- Operator 4 does NOT have `Share[ValidatorX]` (hasn't synced that registration event)

---

## Phase 1: Duty Trigger (`StartNewDuty`)

**All operators** receive the duty from the scheduler. But the duty content differs:

| Operator | `duty.ValidatorDuties` contains |
|----------|--------------------------------|
| 1, 2, 3  | [ValidatorX, ValidatorY, ...]  |
| 4        | [ValidatorY, ...]  ← missing X |

Operator 4's duty scheduler doesn't even include ValidatorX because it doesn't know about it.

---

## Phase 2: Execute Duty (`executeDuty`)

Each operator signs selection proofs for validators in their duty:

```go
// aggregator_committee.go lines 1476-1620
for _, vDuty := range aggCommitteeDuty.ValidatorDuties {
    // Sign selection proof for this validator
    partialSig, err := signBeaconObject(...)
    msg.Messages = append(msg.Messages, partialSig)
}
```

**Operator 4's outgoing message:**
```
PartialSignatureMessages {
    Type: AggregatorCommitteePartialSig
    Slot: 12345
    Messages: [
        { ValidatorIndex: Y, SigningRoot: 0xabc..., PartialSignature: ... }
        // NO entry for ValidatorX
    ]
}
```

**Operators 1,2,3's outgoing message:**
```
PartialSignatureMessages {
    Messages: [
        { ValidatorIndex: X, SigningRoot: 0x123..., PartialSignature: ... }
        { ValidatorIndex: Y, SigningRoot: 0xabc..., PartialSignature: ... }
    ]
}
```

---

## Phase 3: Receive Pre-Consensus Messages (`basePartialSigMsgProcessing`)

Operator 4 receives messages from Operators 1, 2, 3 containing ValidatorX's partial sig.

```go
// runner.go lines 359-387
func (b *BaseRunner) basePartialSigMsgProcessing(...) {
    for _, msg := range signedMsg.Messages {
        // KEY: No validation of whether we know this validator!
        // Just blindly add to container
        container.AddSignature(msg)
    }
}
```

**Operator 4's `PreConsensusContainer` state after receiving all messages:**
```
PreConsensusContainer:
  ValidatorX:
    SigningRoot 0x123...:
      OperatorID 1 → signature_1
      OperatorID 2 → signature_2  
      OperatorID 3 → signature_3
      // Operator 4 never signed, but has quorum (3/4)!
  ValidatorY:
    SigningRoot 0xabc...:
      OperatorID 1 → signature_1
      OperatorID 2 → signature_2
      OperatorID 3 → signature_3
      OperatorID 4 → signature_4  // All 4 signed
```

---

## Phase 4: Process Pre-Consensus (`ProcessPreConsensus`)

When quorum is reached, Operator 4 enters `ProcessPreConsensus`:

```go
// aggregator_committee.go lines 344-560
func (r *AggregatorCommitteeRunner) ProcessPreConsensus(...) {
    // Step 1: Get roots that have quorum
    hasQuorum, roots, err := r.BaseRunner.basePreConsensusMsgProcessing(...)
    // roots = [0x123... (ValidatorX), 0xabc... (ValidatorY)]
    
    // Step 2: Build expected roots from OUR view
    aggregatorMap, contributionMap, err := r.expectedPreConsensusRoots(ctx)
    // Operator 4's aggregatorMap = { ValidatorY: 0xabc... }
    // NO ValidatorX because we don't have the duty for it!
    
    // Step 3: For each root with quorum...
    for _, root := range roots {
        // Find validators for this root
        metadataList, found := r.findValidatorsForPreConsensusRoot(root, aggregatorMap, contributionMap)
        
        // For root 0x123... (ValidatorX):
        // found = FALSE for Operator 4! 
        // Because aggregatorMap doesn't contain ValidatorX
        if !found {
            continue  // ← Operator 4 skips ValidatorX here
        }
        
        // For root 0xabc... (ValidatorY):
        // found = TRUE, proceeds normally
        for _, metadata := range metadataList {
            share := r.BaseRunner.Share[validatorIndex]
            if share == nil {
                continue  // ← Second safety check (redundant here but important)
            }
            
            // Reconstruct signature, check IsAggregator, etc.
        }
    }
}
```

**Operator 4's `consensusData` after ProcessPreConsensus:**
```
AggregatorCommitteeConsensusData {
    Aggregators: [
        { ValidatorIndex: Y, SelectionProof: ..., CommitteeIndex: ... }
        // NO ValidatorX!
    ]
    AggregatedAttestations: [attestation_for_Y]
}
```

---

## Phase 5: QBFT Consensus (`decide`)

Now the critical part — who proposes?

### Case A: Operator 1 is the proposer (round 1)

1. **Operator 1 proposes** `consensusData` containing **both** ValidatorX and ValidatorY
2. **Operator 4 receives proposal**, runs `CheckValue()`:
   ```go
   func (v *aggregatorCommitteeChecker) CheckValue(value []byte) error {
       cd := &spectypes.AggregatorCommitteeConsensusData{}
       cd.Decode(value)
       cd.Validate()  // Only structural validation!
       return nil     // ✅ Passes — doesn't check "do I know these validators"
   }
   ```
3. **All 4 operators** vote PREPARE → COMMIT → DECIDE
4. **Decided value includes ValidatorX** ✅

### Case B: Operator 4 is the proposer (round 1)

1. **Operator 4 proposes** `consensusData` containing **only** ValidatorY (missing X)
2. **Operators 1,2,3 receive proposal**, run `CheckValue()`:
   - Passes! (doesn't validate "are all expected validators present")
3. **All 4 operators** vote PREPARE → COMMIT → DECIDE
4. **Decided value is MISSING ValidatorX** ❌

---

## Phase 6: Process Consensus (`ProcessConsensus`)

After consensus decides, each operator signs the decided data:

```go
// aggregator_committee.go lines 588-720
func (r *AggregatorCommitteeRunner) ProcessConsensus(...) {
    // Get decided value
    consensusData := decidedValue.(*spectypes.AggregatorCommitteeConsensusData)
    
    for i, aggProof := range aggProofs {
        validatorIndex := consensusData.Aggregators[i].ValidatorIndex
        
        // KEY CHECK: Do we have a share for this validator?
        _, exists := r.BaseRunner.Share[validatorIndex]
        if !exists {
            continue  // ← Operator 4 skips ValidatorX here
        }
        
        // Sign aggregate and proof
        msg, err := signBeaconObject(...)
        messages = append(messages, msg)
    }
    
    // Broadcast post-consensus partial sigs
    r.GetNetwork().Broadcast(ssvMsg.MsgID, msgToBroadcast)
}
```

**Operator 4's outgoing post-consensus message:**
```
PartialSignatureMessages {
    Type: PostConsensusPartialSig
    Messages: [
        { ValidatorIndex: Y, SigningRoot: ..., PartialSignature: ... }
        // NO ValidatorX — Operator 4 can't sign for it
    ]
}
```

---

## Phase 7: Post-Consensus (`ProcessPostConsensus`)

Similar pattern — Operator 4 receives post-consensus sigs from 1,2,3 for ValidatorX:

```go
// aggregator_committee.go lines 722-920
func (r *AggregatorCommitteeRunner) ProcessPostConsensus(...) {
    for _, root := range roots {
        metadataList, found := r.findValidatorsForPostConsensusRoot(root, aggregatorMap, contributionMap)
        if !found {
            continue  // ← Operator 4 might skip X here too
        }
        
        for _, validator := range validators {
            share := r.BaseRunner.Share[validatorIndex]
            if share == nil {
                return  // ← Skips ValidatorX
            }
            
            // Reconstruct and submit
        }
    }
}
```

**Result for Operator 4:**
- ✅ Submits ValidatorY's aggregated attestation (has quorum from all 4)
- ❌ Does NOT submit ValidatorX (doesn't know about it)

**Result for Operators 1,2,3:**
- ✅ Submit ValidatorY's aggregated attestation
- ✅ Submit ValidatorX's aggregated attestation (quorum from ops 1,2,3)

---

## Summary: Key Handling Points for Anchor

| Phase | Where to Handle | What to Do |
|-------|----------------|------------|
| **Receive pre-consensus sig** | `add_signature()` | Store it blindly — don't validate validator membership |
| **Build expected roots** | `expected_pre_consensus_roots()` | Only include validators in YOUR `shares` map |
| **Find validators for root** | `find_validators_for_root()` | Return `None` if root not in your expected map |
| **Iterate validators** | Every loop over validators | Check `share.get(validator_index).is_some()` before processing |
| **QBFT value check** | `check_value()` | Only structural validation, NOT "do I know all validators" |
| **Post-consensus signing** | `process_consensus()` | Skip validators not in `shares` map |
| **Post-consensus submission** | `process_post_consensus()` | Skip validators not in `shares` map |

---

## Critical Invariant

**The system works correctly as long as ≥ quorum operators know about a validator.**

If only 1 out of 4 operators is missing a validator:
- 3 operators can still reach quorum for pre-consensus
- 3 operators can still sign post-consensus
- 3 operators can still submit

The missing operator just doesn't participate in that validator's signatures — it's a graceful degradation, not a failure.

---

## TL;DR

The **divergent operator views problem** occurs when operators have different views of which validators belong to a cluster, typically because one operator hasn't yet synced the latest SSV contract events (e.g., a new validator registration). 

**If the out-of-sync operator is NOT the QBFT proposer:** The duty succeeds normally. The proposer includes all validators it knows about, the out-of-sync operator accepts the proposal (value validation is structural only), and simply skips signing/submitting for validators it doesn't recognize. The other operators still reach quorum.

**If the out-of-sync operator IS the QBFT proposer:** The validator(s) it doesn't know about will be missing from the proposed consensus data. Other operators accept this incomplete proposal (no validation that "all expected validators are present"), and those missing validators fail their duty for that slot. This is the worst-case scenario — validators can miss duties if the proposer has an incomplete view.

**Mitigation:** Operators should stay synced with SSV contract events. The probability of the out-of-sync operator being proposer is `1/n` (e.g., 25% for a 4-operator cluster), and even then, only affects the specific slot where they propose.
