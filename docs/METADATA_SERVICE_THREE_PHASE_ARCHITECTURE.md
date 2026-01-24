# MetadataService Three-Phase Architecture

## Problem Statement

In the Anchor SSV client, validator duties require different pieces of information at different points within a slot. The challenge is that some information (like selection proofs) is computed **asynchronously** by Lighthouse's DutiesService and isn't available until later in the slot.

### The Core Timing Problem

```
Slot Timeline
─────────────────────────────────────────────────────────────────────────────────
│                                                                               │
│  Slot Start                    1/3 Slot                    2/3 Slot          │
│      │                            │                            │              │
│      ▼                            ▼                            ▼              │
│                                                                               │
│  Selection proofs              Attestations              Aggregations         │
│  requested here                produced here             produced here        │
│  (async computation            (need beacon vote)        (need aggregator     │
│   starts)                                                 status)             │
│                                                                               │
─────────────────────────────────────────────────────────────────────────────────
```

**Key insight**: `DutyAndProof.selection_proof` is `None` at slot start and only gets filled in asynchronously by Lighthouse. At 2/3 slot when `produce_signed_aggregate_and_proof` is called, `selection_proof.is_some()` accurately indicates `is_aggregator = true`.

### Why This Matters for AggregatorCommittee Consensus

For the Boolegade fork's `AggregatorCommitteeConsensusData`, we need to know:
1. **How many aggregators** exist in a committee (to know when consensus data is "full")
2. **Which validators are aggregators** (selection_proof.is_some())
3. **Sync committee barriers** for multi-subnet aggregators

This information simply isn't available at slot start - we must wait until 2/3 slot.

## Solution: Three-Phase Architecture

We split the MetadataService into three phases, each publishing data at the appropriate time:

```
┌─────────────────────────────────────────────────────────────────────────────┐
│                        MetadataService Phases                                │
├─────────────────────────────────────────────────────────────────────────────┤
│                                                                             │
│  PHASE 1 (Slot Start)          PHASE 2 (1/3 Slot)       PHASE 3 (2/3 Slot) │
│  ─────────────────────         ─────────────────────    ───────────────────│
│                                                                             │
│  ValidatorDutyInfo             SlotMetadata             AggregatorDutyInfo │
│  ┌─────────────────┐           ┌─────────────────┐      ┌────────────────┐ │
│  │ slot            │           │ duty_info (Arc) │      │ slot           │ │
│  │ attesting_      │           │ beacon_vote     │      │ aggregating_   │ │
│  │   validators    │           │                 │      │   attesters    │ │
│  │ attesting_      │           └─────────────────┘      │ aggregator_    │ │
│  │   committees    │                                    │   committees   │ │
│  │ sync_validators │                                    │ sync_agg_by_   │ │
│  │   _by_subnet    │                                    │   subnet       │ │
│  └─────────────────┘                                    │ barriers       │ │
│                                                         └────────────────┘ │
│         │                              │                        │          │
│         ▼                              ▼                        ▼          │
│  produce_selection_proof     produce_attestation      produce_aggregate    │
│  produce_sync_selection_     produce_sync_committee_  produce_contribution │
│    proof                       signature                                   │
│                                                                             │
└─────────────────────────────────────────────────────────────────────────────┘
```

### Phase 1: ValidatorDutyInfo (Slot Start)

**Purpose**: Cache duty lists immediately so selection proof methods can start.

**Data**:
- `attesting_validators: Vec<ValidatorIndex>` - validators attesting this slot
- `attesting_committees: HashMap<PublicKeyBytes, u64>` - pubkey → committee_index
- `sync_validators_by_subnet: HashMap<ValidatorIndex, HashSet<SyncSubnetId>>` - granular sync duty info

**Consumers**: `produce_selection_proof`, `produce_sync_selection_proof`

**Why at slot start**: Selection proofs need to start computing immediately. We know WHO will attest/sync, just not who will be AGGREGATORS yet.

### Phase 2: SlotMetadata (1/3 Slot)

**Purpose**: Fetch beacon vote from beacon node for attestation/sync message signing.

**Data**:
- `duty_info: Arc<ValidatorDutyInfo>` - reuses Phase 1 data
- `beacon_vote: BeaconVote` - block_root, source, target from beacon node

**Consumers**: `produce_attestation`, `produce_sync_committee_signature`

**Why at 1/3 slot**: Attestations are produced at 1/3 slot. The beacon vote must be fresh (current head).

### Phase 3: AggregatorDutyInfo (2/3 Slot)

**Purpose**: Re-query duties_service AFTER selection proofs are computed to get accurate aggregator information.

**Data**:
- `aggregating_attesters: HashSet<ValidatorIndex>` - validators where selection_proof.is_some()
- `aggregator_committees: HashMap<PublicKeyBytes, u64>` - aggregator pubkey → committee
- `sync_aggregators_by_subnet: HashMap<ValidatorIndex, HashSet<SyncSubnetId>>` - sync aggregators
- `barriers: HashMap<PublicKeyBytes, ContributionWaiter<E>>` - for multi-subnet sync aggregators

**Consumers**: `produce_signed_aggregate_and_proof`, `produce_signed_contribution_and_proof`

**Why at 2/3 slot**: By this time, Lighthouse has computed all selection proofs. `DutyAndProof.selection_proof.is_some()` is now accurate.

## Key Design Decisions

### 1. Why Three Separate Phases Instead of One?

**Rejected approach**: Compute everything at slot start
**Problem**: Selection proofs aren't ready yet. We'd have incorrect aggregator counts.

**Rejected approach**: Wait until 2/3 slot for everything
**Problem**: Selection proof methods need duty info at slot start. They'd block unnecessarily.

**Chosen approach**: Three phases matching the natural timing of data availability
**Benefit**: Each consumer gets exactly the data it needs, exactly when it needs it.

### 2. Why Use Tokio Watch Channels?

Watch channels provide:
- **Multiple subscribers**: Many methods can await the same data
- **Latest value semantics**: Late subscribers get the current value immediately
- **Efficient updates**: Single producer, multiple consumers

Alternative considered: `broadcast` channels
**Rejected because**: Watch is simpler for "latest value" pattern; broadcast is for event streams.

### 3. Why Wrap in Arc?

```rust
aggregator_duty_info_tx: watch::Sender<Option<Arc<AggregatorDutyInfo<E>>>>
```

**Benefit**: Eliminates need for `Clone` on `AggregatorDutyInfo`. Since it contains `ContributionWaiter` with `RwLock` and `Barrier`, those can't be cloned. Arc lets multiple consumers share the same instance.

### 4. Why Merge Barriers into AggregatorDutyInfo?

**Original design (rejected)**:
```rust
// Two separate watch channels
aggregator_duty_info_tx: watch::Sender<Option<Arc<AggregatorDutyInfo>>>
sync_aggregator_barriers_tx: watch::Sender<Option<Arc<SyncAggregatorBarriers<E>>>>
```

**Problem**: Unnecessary complexity. Both are published at exactly the same time (2/3 slot) and consumed together.

**Final design**:
```rust
pub struct AggregatorDutyInfo<E: EthSpec> {
    // ... counts ...
    barriers: HashMap<PublicKeyBytes, ContributionWaiter<E>>,
}

impl<E: EthSpec> AggregatorDutyInfo<E> {
    pub(crate) fn get_barrier(&self, pubkey: &PublicKeyBytes) -> Option<&ContributionWaiter<E>> {
        self.barriers.get(pubkey)
    }
}
```

**Benefit**: Single channel, single struct, simpler code path.

### 5. Barrier Creation Pattern

The barrier creation follows the original `SlotMetadata` pattern exactly:

```rust
let barriers = sync_duties
    .map(|duties| {
        let mut aggregators_by_validator = HashMap::new();
        for (_, aggregators) in duties.aggregators {
            for (_, pk, _) in aggregators {
                *aggregators_by_validator.entry(pk).or_insert(0usize) += 1;
            }
        }
        aggregators_by_validator
            .into_iter()
            .filter(|(_, count)| *count > 1)
            .map(|(pk, count)| (pk, ContributionWaiter::new(count)))
            .collect()
    })
    .unwrap_or_default();
```

**Logic**: Count how many subnets each pubkey aggregates. If > 1, create a barrier so all contributions can be batched before consensus.

### 6. Why ValidatorDutyInfo Has Two Counting Methods

```rust
impl ValidatorDutyInfo {
    /// Selection proofs: +1 per attester, +N per sync (N = subnet count)
    pub fn selection_proof_count_for_committee<F>(&self, is_in_committee: F) -> usize

    /// Committee messages: +1 per attester, +1 per sync (flat)
    pub fn committee_message_count_for_committee<F>(&self, is_in_committee: F) -> usize
}
```

**Reason**: Different collection patterns need different counts:
- **Selection proofs**: One proof per subnet per validator (aggregator committee pre-consensus)
- **Committee messages**: One message per validator regardless of subnets (post-consensus)

## Data Flow Diagram

```
                            Lighthouse DutiesService
                                     │
         ┌───────────────────────────┼───────────────────────────┐
         │                           │                           │
         ▼                           ▼                           ▼
    Slot Start                  1/3 Slot                    2/3 Slot
         │                           │                           │
         │                           │                           │
         ▼                           ▼                           ▼
┌─────────────────┐         ┌─────────────────┐         ┌─────────────────┐
│ MetadataService │         │ MetadataService │         │ MetadataService │
│    Phase 1      │         │    Phase 2      │         │    Phase 3      │
└────────┬────────┘         └────────┬────────┘         └────────┬────────┘
         │                           │                           │
         │ duties_service            │ beacon_node               │ duties_service
         │ .attesters(slot)          │ .get_attestation_data()   │ .attesters(slot)
         │ .sync_duties              │                           │ NOW WITH
         │                           │                           │ SELECTION PROOFS
         │                           │                           │
         ▼                           ▼                           ▼
┌─────────────────┐         ┌─────────────────┐         ┌─────────────────┐
│ValidatorDutyInfo│         │  SlotMetadata   │         │AggregatorDutyInfo│
└────────┬────────┘         └────────┬────────┘         └────────┬────────┘
         │                           │                           │
         │ watch::send               │ watch::send               │ watch::send
         │                           │                           │
         ▼                           ▼                           ▼
┌─────────────────┐         ┌─────────────────┐         ┌─────────────────┐
│   Consumers     │         │   Consumers     │         │   Consumers     │
│                 │         │                 │         │                 │
│ produce_        │         │ produce_        │         │ produce_signed_ │
│ selection_proof │         │ attestation     │         │ aggregate_and_  │
│                 │         │                 │         │ proof           │
│ produce_sync_   │         │ produce_sync_   │         │                 │
│ selection_proof │         │ committee_sig   │         │ produce_signed_ │
│                 │         │                 │         │ contribution_   │
│                 │         │                 │         │ and_proof       │
└─────────────────┘         └─────────────────┘         └─────────────────┘
```

## File Locations

- **`anchor/validator_store/src/lib.rs`**: Contains `ValidatorDutyInfo`, `SlotMetadata`, `AggregatorDutyInfo`, and the `AnchorValidatorStore` with watch channels
- **`anchor/validator_store/src/metadata_service.rs`**: Contains the three-phase timer loops and builder methods

## Testing Considerations

The watch channel pattern can be tested directly without a full `AnchorValidatorStore`:

```rust
#[tokio::test]
async fn test_validator_duty_info_watch_channel_waits_for_update() {
    let (tx, mut rx) = watch::channel::<Option<Arc<ValidatorDutyInfo>>>(None);

    let wait_task = tokio::spawn(async move {
        loop {
            let current = rx.borrow().clone();
            if let Some(duty_info) = current {
                if duty_info.slot == Slot::new(5) {
                    return Ok::<_, ()>(duty_info);
                }
            }
            if rx.changed().await.is_err() {
                return Err(());
            }
        }
    });

    // Update triggers the waiting task
    let duty_info = ValidatorDutyInfo { slot: Slot::new(5), ... };
    tx.send_replace(Some(Arc::new(duty_info)));

    let result = wait_task.await.unwrap();
    assert!(result.is_ok());
}
```

## Future Considerations

When implementing `AggregatorCommitteeConsensusData`:

1. Use `AggregatorDutyInfo.attestation_aggregator_count()` to know expected entries
2. Use `AggregatorDutyInfo.sync_aggregator_assignment_count()` for sync aggregator entries
3. The barriers in `AggregatorDutyInfo` handle multi-subnet sync committee batching
4. Consider whether `AggregatorCommitteeConsensusData` needs its own QBFT instance type

## Summary

The three-phase architecture solves the fundamental timing mismatch between when different pieces of duty information become available and when they're needed. By separating concerns into three phases with watch channels, each consumer gets exactly the data it needs at exactly the right time, without blocking or using stale data.
