# Role::Committee Check Analysis

## Summary
- Total occurrences found: 15 significant checks
- Needs fix: 4 (2 already known, 2 new findings)
- Already correct: 7
- Unclear/needs discussion: 4

## Detailed Findings

### 1. [message_validator/src/partial_signature.rs:135-139] - Validator Index Validation Skip ✅ ALREADY FIXED
**Current Code:**
```rust
let is_committee_role = matches!(
    validation_context.role,
    Role::Committee | Role::AggregatorCommittee
);
if !is_committee_role
    && !validation_context.committee_info.validator_indexes.contains(&validator_index)
```

**Context**: Skips validator index validation for committee roles because operators may have different views of the committee's validator set.

**Current Behavior**:
- Committee: Skips validation
- AggregatorCommittee: Skips validation

**Should include AggregatorCommittee?**: YES - Already included!

**Rationale**: Both committee roles need this relaxation due to operator view divergence.

**Impact if wrong**: N/A - Already correct

**Priority**: N/A

---

### 2. [message_validator/src/partial_signature.rs:156-164] - Partial Signature Type Matching
**Current Code:**
```rust
fn partial_signature_type_matches_role(kind: PartialSignatureKind, role: Role) -> bool {
    match role {
        Role::Committee => kind == PartialSignatureKind::PostConsensus,
        Role::Aggregator => {
            kind == PartialSignatureKind::PostConsensus
                || kind == PartialSignatureKind::SelectionProofPartialSig
        }
        Role::AggregatorCommittee => {
            kind == PartialSignatureKind::AggregatorCommitteePartialSig
                || kind == PartialSignatureKind::PostConsensus
        }
        // ... other roles
    }
}
```

**Context**: Validates that the partial signature type is appropriate for the role.

**Current Behavior**:
- Committee: Only accepts PostConsensus
- AggregatorCommittee: Accepts both AggregatorCommitteePartialSig and PostConsensus

**Should include AggregatorCommittee?**: Already has its own case - CORRECT

**Rationale**: Each role has specific signature types it accepts. This is correct as-is.

**Impact if wrong**: N/A - Already correct

**Priority**: N/A

---

### 3. [message_validator/src/partial_signature.rs:202-210] - Slot Advancement Check Skip ❌ NEEDS FIX
**Current Code:**
```rust
// Rule: Slot must not be "old" - signer must not have already advanced to a later slot
// Skip for committee role
if role != Role::Committee {
    let max_slot = operator_state.max_slot();
    if max_slot.as_u64() != 0 && max_slot > message_slot {
        return Err(ValidationFailure::TooOldSlot);
    }
}
```

**Context**: Skips the "old slot" check for committee roles to allow processing messages within the 34-slot window.

**Current Behavior**:
- Committee: Skips validation (allows old slots)
- AggregatorCommittee: APPLIES validation (rejects old slots)

**Should include AggregatorCommittee?**: YES

**Rationale**: AggregatorCommittee also operates on a 34-slot window and needs the same relaxed timing constraints as Committee.

**Impact if wrong**: AggregatorCommittee messages will be incorrectly rejected if they arrive after the operator has seen a later slot, breaking the 34-slot window design.

**Priority**: CRITICAL

---

### 4. [message_validator/src/partial_signature.rs:250-275] - Message Count Validation
**Current Code:**
```rust
match role {
    Role::Committee => {
        // Rule: Number of signatures must be <= min(2*V, V + SYNC_COMMITTEE_SIZE)
        let max_allowed = std::cmp::min(
            2 * validator_count,
            validator_count + validation_context.sync_committee_size,
        );
        // ... check and validator index occurrence limit (max 2)
    }
    Role::AggregatorCommittee => match kind {
        PartialSignatureKind::AggregatorCommitteePartialSig
        | PartialSignatureKind::PostConsensus => {
            let max_allowed = std::cmp::min(
                5 * validator_count,
                validator_count + 4 * validation_context.sync_committee_size,
            );
            // ... check but NO validator index occurrence limit!
        }
        // ...
    }
    // ...
}
```

**Context**: Validates message count limits and validator index occurrence limits per role.

**Current Behavior**:
- Committee: Max 2*V messages, max 2 occurrences per validator index
- AggregatorCommittee: Max 5*V messages, but MISSING occurrence limit check!

**Should include AggregatorCommittee?**: Partially - needs occurrence limit

**Rationale**: AggregatorCommittee should allow up to 5 occurrences per validator index (matching Go SSV).

**Impact if wrong**: Could accept invalid messages with too many occurrences of the same validator index.

**Priority**: HIGH

---

### 5. [message_validator/src/consensus_message.rs:423-434] - Consensus Height Check Skip ❌ NEEDS FIX
**Current Code:**
```rust
// Rule: Height must not be "old". I.e., signer must not have already advanced to a later slot.
if role != Role::Committee {
    for &signer in signed_ssv_message.operator_ids() {
        let signer_state = duty_state.get_or_create_operator(&signer);
        let max_slot = signer_state.max_slot();
        if max_slot.as_u64() != 0 && max_slot > msg_slot {
            return Err(ValidationFailure::TooOldSlot(signer));
        }
    }
}
```

**Context**: Skips the "old height" check for committee roles to allow processing consensus messages within the 34-slot window.

**Current Behavior**:
- Committee: Skips validation (allows old heights)
- AggregatorCommittee: APPLIES validation (rejects old heights)

**Should include AggregatorCommittee?**: YES

**Rationale**: Same as partial signature - AggregatorCommittee needs the 34-slot window for consensus messages.

**Impact if wrong**: AggregatorCommittee consensus messages will be incorrectly rejected, breaking the protocol.

**Priority**: CRITICAL

---

### 6. [message_validator/src/lib.rs:366-386] - Committee Info Retrieval
**Current Code:**
```rust
let committee_info = match role {
    Role::Committee => {
        let committee_id = match ssv_message.msg_id().duty_executor() {
            Some(DutyExecutor::Committee(id)) => id,
            _ => return Err(ValidationFailure::NonExistentCommitteeID),
        };
        network_state.get_committee(&committee_id)
            .ok_or(ValidationFailure::NonExistentCommitteeID)?
    }
    Role::AggregatorCommittee => {
        let committee_id = match ssv_message.msg_id().duty_executor() {
            Some(DutyExecutor::Committee(id)) => id,
            _ => return Err(ValidationFailure::NonExistentCommitteeID),
        };
        network_state.get_committee(&committee_id)
            .ok_or(ValidationFailure::NonExistentCommitteeID)?
    }
    // ... other roles
}
```

**Context**: Retrieves committee information based on role.

**Current Behavior**:
- Committee: Gets committee from network state
- AggregatorCommittee: Gets committee from network state (same logic)

**Should include AggregatorCommittee?**: Already has its own case - CORRECT

**Rationale**: Both roles need committee info, handled separately.

**Impact if wrong**: N/A - Already correct

**Priority**: N/A

---

### 7. [message_validator/src/lib.rs:672-678] - TTL Calculation
**Current Code:**
```rust
let ttl = match validation_context.role {
    Role::Proposer | Role::SyncCommittee => 1 + LATE_SLOT_ALLOWANCE,
    Role::Committee
    | Role::Aggregator
    | Role::ValidatorRegistration
    | Role::VoluntaryExit
    | Role::AggregatorCommittee => QBFT_RETAIN_SLOTS + LATE_SLOT_ALLOWANCE,
    _ => unreachable!("role should be validated"),
};
```

**Context**: Calculates message time-to-live based on role.

**Current Behavior**:
- Committee: 34 slots + late allowance
- AggregatorCommittee: 34 slots + late allowance

**Should include AggregatorCommittee?**: Already included - CORRECT

**Rationale**: Both committee roles need the extended TTL for the 34-slot window.

**Impact if wrong**: N/A - Already correct

**Priority**: N/A

---

### 8. [qbft_manager/src/lib.rs:258-298] - Message Routing by Role
**Current Code:**
```rust
Some(DutyExecutor::Committee(committee)) => {
    match msg_id.role() {
        Some(Role::Committee) => {
            // Existing BeaconVote routing
            let id = CommitteeInstanceId { committee, instance_height };
            self.pass_to_instance::<BeaconVote>(id, ...)
        }
        Some(Role::AggregatorCommittee) => {
            // Route to aggregator committee instances with fork gating
            let id = AggregatorCommitteeInstanceId { committee, instance_height };
            self.pass_to_instance::<AggregatorCommitteeConsensusData<E>>(id, ...)
        }
        _ => Err(QbftError::InconsistentMessageId),
    }
}
```

**Context**: Routes messages to appropriate QBFT instances based on role.

**Current Behavior**:
- Committee: Routes to BeaconVote instances
- AggregatorCommittee: Routes to AggregatorCommitteeConsensusData instances

**Should include AggregatorCommittee?**: Already handled separately - CORRECT

**Rationale**: Different consensus data types require different routing.

**Impact if wrong**: N/A - Already correct

**Priority**: N/A

---

### 9. [ssv_types/src/msgid.rs:27-30] - Role to Bytes Conversion
**Current Code:**
```rust
match value {
    Role::Committee => [0, 0, 0, 0],
    Role::Aggregator => [1, 0, 0, 0],
    Role::Proposer => [2, 0, 0, 0],
    Role::SyncCommittee => [3, 0, 0, 0],
    Role::AggregatorCommittee => [5, 0, 0, 0],
    // ...
}
```

**Context**: Converts Role enum to byte representation for message IDs.

**Current Behavior**:
- Committee: [0, 0, 0, 0]
- AggregatorCommittee: [5, 0, 0, 0]

**Should include AggregatorCommittee?**: Already has its own case - CORRECT

**Rationale**: Each role needs unique byte representation.

**Impact if wrong**: N/A - Already correct

**Priority**: N/A

---

### 10. [ssv_types/src/msgid.rs:59] - Max Round Determination
**Current Code:**
```rust
match self {
    Role::Committee | Role::Aggregator | Role::AggregatorCommittee => Some(12),
    Role::Proposer | Role::SyncCommittee => Some(6),
    _ => None,
}
```

**Context**: Determines maximum consensus round number for each role.

**Current Behavior**:
- Committee: 12 rounds max
- AggregatorCommittee: 12 rounds max

**Should include AggregatorCommittee?**: Already included - CORRECT

**Rationale**: Both committee roles need the same round limit.

**Impact if wrong**: N/A - Already correct

**Priority**: N/A

---

### 11. [ssv_types/src/msgid.rs:132-134] - Duty Executor Extraction
**Current Code:**
```rust
match self.role()? {
    Role::Committee | Role::AggregatorCommittee => {
        self.0[24..].try_into().ok().map(DutyExecutor::Committee)
    }
    _ => // ... validator executor
}
```

**Context**: Extracts duty executor from message ID based on role.

**Current Behavior**:
- Committee: Extracts as DutyExecutor::Committee
- AggregatorCommittee: Extracts as DutyExecutor::Committee

**Should include AggregatorCommittee?**: Already included - CORRECT

**Rationale**: Both use the same committee executor type.

**Impact if wrong**: N/A - Already correct

**Priority**: N/A

---

### 12. [validator_store/src/lib.rs:1322,1776] - Test Helper Usage
**Current Code:**
```rust
.collect_signature(
    PartialSignatureKind::PostConsensus,
    Role::Committee,
    CollectionMode::Committee { num_signatures_to_collect },
    // ...
)
```

**Context**: Test code creating committee signatures.

**Current Behavior**:
- Uses Role::Committee for test setup

**Should include AggregatorCommittee?**: NO - Test specific

**Rationale**: This is test helper code, not validation logic.

**Impact if wrong**: None - test code only

**Priority**: N/A

---

### 13. [duty_state.rs:341,386,401] - Test Helper Usage
**Current Code:**
```rust
let qbft_message = QbftMessageBuilder::new(Role::Committee, QbftMessageType::Proposal).build();
```

**Context**: Test code creating QBFT messages.

**Current Behavior**:
- Uses Role::Committee for test setup

**Should include AggregatorCommittee?**: MAYBE - Could add tests

**Rationale**: Tests should cover AggregatorCommittee scenarios too.

**Impact if wrong**: Missing test coverage

**Priority**: LOW

---

### 14. [consensus_message.rs tests] - Multiple Test Uses
**Current Code:**
```rust
// Many test occurrences like:
QbftMessageBuilder::new(Role::Committee, QbftMessageType::Proposal)
```

**Context**: Test code throughout consensus_message tests.

**Current Behavior**:
- All tests use Role::Committee

**Should include AggregatorCommittee?**: MAYBE - Could add coverage

**Rationale**: Should have parallel tests for AggregatorCommittee.

**Impact if wrong**: Missing test coverage for AggregatorCommittee paths

**Priority**: MEDIUM

---

### 15. [operator_doppelganger/src/service.rs:284] - Test Message Creation
**Current Code:**
```rust
let message_id = MessageId::new(
    &DomainType([0; 4]),
    Role::Committee,
    &DutyExecutor::Committee(committee_id),
);
```

**Context**: Test helper creating a message ID.

**Current Behavior**:
- Uses Role::Committee for test

**Should include AggregatorCommittee?**: NO - Test specific

**Rationale**: Test helper code, not validation logic.

**Impact if wrong**: None

**Priority**: N/A

---

## Critical Findings Summary

**STATUS: ALL ISSUES RESOLVED** (Verified 2025-01-20)

### All Bugs Fixed:

1. ✅ **partial_signature.rs:198** - Now uses `is_committee_role()` helper
   ```rust
   // Fixed - uses is_committee_role() which returns true for both Committee and AggregatorCommittee
   if !role.is_committee_role() {
       // ... slot check
   }
   ```

2. ✅ **consensus_message.rs:424** - Now uses `is_committee_role()` helper
   ```rust
   // Fixed - uses is_committee_role() which returns true for both Committee and AggregatorCommittee
   if !role.is_committee_role() {
       // ... height check
   }
   ```

3. ✅ **partial_signature.rs:302-315** - Validator index occurrence limit for AggregatorCommittee implemented
   - Limits each validator index to max 5 occurrences for AggregatorCommittee

### Already Correct:
- Validator index validation skip (partial_signature.rs:135) - uses `is_committee_role()`
- TTL calculation (lib.rs:676-680) - includes AggregatorCommittee
- Duty count validation (lib.rs:761) - uses `Role::Committee | Role::AggregatorCommittee`
- Message routing (qbft_manager) - routes AggregatorCommittee with fork gating
- Role conversions and utilities (ssv_types/msgid.rs) - `is_committee_role()` helper implemented
- Partial signature type matching (has separate logic per role)

### Test Coverage:
- ✅ `test_aggregator_committee_skips_slot_advancement_check`
- ✅ `test_aggregator_committee_validator_index_occurrence_limit`
- ✅ `test_aggregator_committee_skips_height_advancement_check`
- ✅ `test_aggregator_committee_role_skips_validator_index_check`
- ✅ `test_aggregator_committee_accepts_aggregator_committee_partial_sig`
- ✅ `test_aggregator_committee_accepts_post_consensus`
- ✅ `test_aggregator_committee_message_count_*` (multiple tests)

## Recommendations (All Completed)

1. ✅ **Immediate Action**: Fixed the slot/height validation bugs using `is_committee_role()`
2. ✅ **High Priority**: Added validator index occurrence limit for AggregatorCommittee (max 5)
3. ✅ **Medium Priority**: Added comprehensive tests for AggregatorCommittee
4. ✅ **Consider**: Added `is_committee_role()` helper method to Role enum (ssv_types/msgid.rs:65)