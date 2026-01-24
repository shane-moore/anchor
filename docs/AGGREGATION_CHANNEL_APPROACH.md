# Aggregation Data Channel Approach

This document describes an alternative approach for building `AggregatorCommitteeConsensusData` that eliminates duplicate beacon node calls by receiving aggregation data through channels from Lighthouse's duties service.

## Problem Statement

In the current Step 11b design, MetadataService makes beacon node calls at 2/3 slot to fetch aggregated attestations and sync contributions. However, Lighthouse's `AttestationService` and `SyncCommitteeService` ALSO fetch the same data at 2/3 slot (before calling `produce_signed_aggregate_and_proof`).

With 10+ aggregators per committee, this duplication becomes significant:
- **Current**: N calls from Lighthouse + N calls from MetadataService = 2N calls
- **Channel approach**: N calls from Lighthouse only = N calls

## Solution Overview

Extend the existing channel pattern (`attesters_poll_tx`/`sync_poll_tx`) in Lighthouse's `DutiesService`. After fetching aggregated data from the beacon node, Lighthouse sends it through a broadcast channel. MetadataService subscribes and receives this data instead of making separate beacon node calls.

## Data Flow

```
Current Flow (Step 11b):
========================
2/3 slot
├─► Lighthouse AttestationService
│   ├─► Fetches aggregated attestation from beacon node
│   └─► Calls produce_signed_aggregate_and_proof()
│
└─► MetadataService Phase 3
    ├─► Fetches aggregated attestation from beacon node  ← DUPLICATE
    └─► Builds AggregatorCommitteeConsensusData


Channel Flow:
=============
2/3 slot
├─► Lighthouse AttestationService
│   ├─► Fetches aggregated attestation from beacon node
│   ├─► Sends to channel: (slot, committee_index, attestation)  ← NEW
│   └─► Calls produce_signed_aggregate_and_proof()
│
└─► MetadataService Phase 3
    ├─► Receives from channel (no beacon node call)  ← CHANGED
    └─► Builds AggregatorCommitteeConsensusData
```

## Lighthouse Changes

### 1. Event Types (duties_service.rs)

```rust
use tokio::sync::broadcast;

/// Event emitted when an aggregated attestation is fetched from the beacon node.
#[derive(Clone, Debug)]
pub struct AggregatedAttestationEvent<E: EthSpec> {
    pub slot: Slot,
    pub committee_index: u64,
    pub attestation: Attestation<E>,
}

/// Event emitted when a sync committee contribution is fetched from the beacon node.
#[derive(Clone, Debug)]
pub struct SyncContributionEvent<E: EthSpec> {
    pub slot: Slot,
    pub subnet_id: SyncSubnetId,
    pub contribution: SyncCommitteeContribution<E>,
}
```

### 2. Channel Infrastructure (duties_service.rs)

```rust
pub struct DutiesService<S, T> {
    // Existing fields...

    /// Channel for aggregated attestation events
    aggregated_attestation_tx: broadcast::Sender<AggregatedAttestationEvent<S::E>>,

    /// Channel for sync contribution events
    sync_contribution_tx: broadcast::Sender<SyncContributionEvent<S::E>>,
}

impl<S: ValidatorStore, T: SlotClock> DutiesService<S, T> {
    /// Subscribe to aggregated attestation events.
    pub fn subscribe_to_aggregated_attestations(&self)
        -> broadcast::Receiver<AggregatedAttestationEvent<S::E>>
    {
        self.aggregated_attestation_tx.subscribe()
    }

    /// Subscribe to sync contribution events.
    pub fn subscribe_to_sync_contributions(&self)
        -> broadcast::Receiver<SyncContributionEvent<S::E>>
    {
        self.sync_contribution_tx.subscribe()
    }

    /// Send an aggregated attestation event. Called by attestation_service.
    pub fn notify_aggregated_attestation(
        &self,
        slot: Slot,
        committee_index: u64,
        attestation: Attestation<S::E>
    ) {
        let _ = self.aggregated_attestation_tx.send(AggregatedAttestationEvent {
            slot,
            committee_index,
            attestation,
        });
    }

    /// Send a sync contribution event. Called by sync_committee_service.
    pub fn notify_sync_contribution(
        &self,
        slot: Slot,
        subnet_id: SyncSubnetId,
        contribution: SyncCommitteeContribution<S::E>
    ) {
        let _ = self.sync_contribution_tx.send(SyncContributionEvent {
            slot,
            subnet_id,
            contribution,
        });
    }
}
```

### 3. Send Events After Fetch (attestation_service.rs)

In `produce_and_publish_aggregates`, after the aggregate is fetched:

```rust
let aggregated_attestation = &self
    .beacon_nodes
    .first_success(|beacon_node| async move {
        // ... existing fetch logic ...
    })
    .await
    .map_err(|e| e.to_string())?;

// NEW: Notify channel subscribers
self.duties_service.notify_aggregated_attestation(
    attestation_data.slot,
    committee_index,
    aggregated_attestation.clone(),
);

// ... existing signing logic continues ...
```

### 4. Send Events After Fetch (sync_committee_service.rs)

After fetching sync committee contribution:

```rust
let contribution = beacon_node
    .get_validator_sync_committee_contribution(&sync_contribution_data)
    .await?;

// NEW: Notify channel subscribers
if let Some(ref contrib) = contribution {
    self.duties_service.notify_sync_contribution(
        slot,
        subnet_id,
        contrib.data.clone(),
    );
}
```

## Anchor Changes

### 1. Subscribe to Channels (metadata_service.rs)

```rust
pub struct MetadataService<E: EthSpec, T: SlotClock + 'static> {
    // Existing fields...

    /// Receiver for aggregated attestation events
    aggregated_attestation_rx: broadcast::Receiver<AggregatedAttestationEvent<E>>,

    /// Receiver for sync contribution events
    sync_contribution_rx: broadcast::Receiver<SyncContributionEvent<E>>,
}

impl<E: EthSpec, T: SlotClock + 'static> MetadataService<E, T> {
    pub fn new(
        duties_service: Arc<DutiesService<AnchorValidatorStore<T, E>, T>>,
        // ... other params ...
    ) -> Self {
        Self {
            // ... existing initialization ...
            aggregated_attestation_rx: duties_service.subscribe_to_aggregated_attestations(),
            sync_contribution_rx: duties_service.subscribe_to_sync_contributions(),
        }
    }
}
```

### 2. Replace Fetch with Channel Collection

The `update_aggregation_assignments` function stays largely the same. Only `build_consensus_data_for_all_committees` changes:

```rust
async fn build_consensus_data_for_all_committees(
    &self,
    slot: Slot,
    aggregating_attesters: &[&DutyAndProof],
    sync_aggregators: Option<&HashMap<SyncSubnetId, SyncAggregatorData>>,
) -> Result<HashMap<CommitteeId, Arc<AggregatorCommitteeConsensusData<E>>>, String> {

    // ... existing grouping by SSV committee (UNCHANGED) ...
    // ... collect attestation_committee_indexes (UNCHANGED) ...
    // ... collect all_subnet_ids (UNCHANGED) ...

    // CHANGED: Collect from channels instead of fetching from beacon node
    let aggregated_attestations = self.collect_attestations_from_channel(
        slot,
        &attestation_committee_indexes,
    ).await;

    let sync_contributions = self.collect_contributions_from_channel(
        slot,
        &all_subnet_ids,
    ).await;

    // ... build_consensus_data_for_committee calls (UNCHANGED) ...
}
```

### 3. Channel Collection Methods

```rust
async fn collect_attestations_from_channel(
    &mut self,
    slot: Slot,
    expected_committee_indexes: &HashSet<u64>,
) -> HashMap<u64, Attestation<E>> {
    let mut collected = HashMap::new();
    let deadline = Instant::now() + Duration::from_millis(500);

    while collected.len() < expected_committee_indexes.len() && Instant::now() < deadline {
        tokio::select! {
            result = self.aggregated_attestation_rx.recv() => {
                match result {
                    Ok(event) if event.slot == slot
                        && expected_committee_indexes.contains(&event.committee_index) =>
                    {
                        collected.insert(event.committee_index, event.attestation);
                    }
                    Ok(_) => continue,  // Different slot or unexpected committee
                    Err(broadcast::error::RecvError::Lagged(n)) => {
                        warn!(%slot, lagged = n, "Missed attestation events");
                    }
                    Err(broadcast::error::RecvError::Closed) => break,
                }
            }
            _ = tokio::time::sleep_until(deadline.into()) => break,
        }
    }

    if collected.len() < expected_committee_indexes.len() {
        warn!(
            %slot,
            expected = expected_committee_indexes.len(),
            received = collected.len(),
            "Proceeding with partial attestation data from channel"
        );
    }

    collected
}

async fn collect_contributions_from_channel(
    &mut self,
    slot: Slot,
    expected_subnet_ids: &HashSet<SyncSubnetId>,
) -> HashMap<SyncSubnetId, SyncCommitteeContribution<E>> {
    let mut collected = HashMap::new();
    let deadline = Instant::now() + Duration::from_millis(500);

    while collected.len() < expected_subnet_ids.len() && Instant::now() < deadline {
        tokio::select! {
            result = self.sync_contribution_rx.recv() => {
                match result {
                    Ok(event) if event.slot == slot
                        && expected_subnet_ids.contains(&event.subnet_id) =>
                    {
                        collected.insert(event.subnet_id, event.contribution);
                    }
                    Ok(_) => continue,
                    Err(broadcast::error::RecvError::Lagged(n)) => {
                        warn!(%slot, lagged = n, "Missed sync contribution events");
                    }
                    Err(broadcast::error::RecvError::Closed) => break,
                }
            }
            _ = tokio::time::sleep_until(deadline.into()) => break,
        }
    }

    if collected.len() < expected_subnet_ids.len() {
        warn!(
            %slot,
            expected = expected_subnet_ids.len(),
            received = collected.len(),
            "Proceeding with partial sync contribution data from channel"
        );
    }

    collected
}
```

### 4. Remove Old Fetch Methods

Delete from `metadata_service.rs`:
- `fetch_aggregated_attestations()`
- `fetch_sync_contributions()`

## What Stays Unchanged

| Component | Status |
|-----------|--------|
| `update_aggregation_assignments()` structure | Unchanged |
| `AggregationAssignments` return type | Unchanged |
| Pre-Boole fork logic | Unchanged |
| `duties_service.attesters(slot)` calls | Unchanged |
| `sync_duties.get_duties_for_slot()` calls | Unchanged |
| SSV committee grouping logic | Unchanged |
| `build_consensus_data_for_committee()` | Unchanged |
| `produce_signed_aggregate_and_proof()` | Unchanged |

## Timing Considerations

### Slot Timeline

```
Slot (12 seconds):
├─ 1/3 slot (4s): Attestations created
├─ 2/3 slot (8s): Aggregation phase starts
│   ├─► Lighthouse fetches aggregates (~100ms typical, 3s timeout max)
│   ├─► Lighthouse sends to channel
│   └─► MetadataService collects from channel (500ms timeout)
├─ 3/4 slot (9s): Aggregates should be published
└─ Slot end (12s)
```

### Timeout Analysis

- **Beacon node HTTP timeout**: 3 seconds (configured in Lighthouse)
- **Channel collection timeout**: 500ms (our timeout)
- **Risk**: If Lighthouse takes >500ms to fetch, MetadataService proceeds with partial data

### Why 500ms?

- Healthy beacon node fetches complete in ~100ms
- 500ms provides buffer for slower responses
- Leaves ~500ms for QBFT consensus before 3/4 slot deadline
- Proceeding with partial data is acceptable (same data across all operators)

## Trade-offs

### Advantages

1. **Zero duplicate beacon node calls** - Data flows from single source
2. **Follows existing pattern** - Similar to `attesters_poll_tx`/`sync_poll_tx`
3. **Simple Lighthouse changes** - Just add send after fetch
4. **Pre-Boole unchanged** - Only affects Boole+ consensus data building

### Disadvantages

1. **Timing dependency** - MetadataService waits for Lighthouse events
2. **Partial data risk** - If Lighthouse slow, MetadataService has incomplete data
3. **Upstream changes required** - Need to modify Lighthouse codebase
4. **Channel management** - Need to handle lagged receivers, cleanup

### Comparison with Step 11b (Direct Fetch)

| Aspect | Step 11b | Channel Approach |
|--------|----------|------------------|
| Beacon node calls | Duplicate | Single |
| Timing | Independent | Dependent on Lighthouse |
| Complexity | Lower | Medium |
| Lighthouse changes | None | Required |
| Failure mode | Fetch fails → retry | Channel miss → partial data |
| Worst case timing | Both finish ~same time | MetadataService waits for Lighthouse |

## Implementation Checklist

### Lighthouse

- [ ] Add `AggregatedAttestationEvent` struct
- [ ] Add `SyncContributionEvent` struct
- [ ] Add broadcast channel senders to `DutiesService`
- [ ] Add `subscribe_to_aggregated_attestations()` method
- [ ] Add `subscribe_to_sync_contributions()` method
- [ ] Add `notify_aggregated_attestation()` method
- [ ] Add `notify_sync_contribution()` method
- [ ] Send event in `attestation_service.rs` after fetch
- [ ] Send event in `sync_committee_service.rs` after fetch

### Anchor

- [ ] Add channel receivers to `MetadataService`
- [ ] Subscribe to channels in `MetadataService::new()`
- [ ] Add `collect_attestations_from_channel()` method
- [ ] Add `collect_contributions_from_channel()` method
- [ ] Update `build_consensus_data_for_all_committees()` to use channel collection
- [ ] Remove `fetch_aggregated_attestations()` method
- [ ] Remove `fetch_sync_contributions()` method
- [ ] Add metrics for channel receive counts vs expected

## Decision

This approach is documented as an alternative to Step 11b. The decision to implement depends on:

1. **Is beacon node load a concern?** If not, Step 11b's simplicity may be preferred.
2. **Is upstream Lighthouse change acceptable?** If not, Step 11b is the only option.
3. **Is timing independence critical?** Step 11b provides better timing isolation.

For most deployments, Step 11b (direct fetch) is recommended due to its simplicity and timing independence. The channel approach should be considered if beacon node load becomes a measurable bottleneck.
