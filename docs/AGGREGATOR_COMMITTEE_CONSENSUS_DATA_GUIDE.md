# Building AggregatorCommitteeConsensusData

This document describes how to construct `AggregatorCommitteeConsensusData` for committee-based QBFT consensus in the AggregatorCommittee role (post-Boole fork).

## Overview

**Key Design Principle**: `AggregatorCommitteeConsensusData` is a **committee-based data object** containing data for ALL validators in the committee. All operators reach consensus on the same data structure via QBFT. Individual validators then extract and sign their portion from the decided data. There is NO per-validator filtering of the consensus data - the full committee data is what goes through QBFT.

The `AggregatorCommitteeConsensusData` structure combines both attestation aggregation and sync committee contribution data into a single consensus payload:

```rust
pub struct AggregatorCommitteeConsensusData<E: EthSpec> {
    /// Data version (fork) for deserialization of attestations/contributions
    pub version: DataVersion,
    /// Validators selected as attestation aggregators with their selection proofs
    pub aggregators: VariableList<AssignedAggregator, MaxAggregators>,
    /// Committee indexes that have aggregated attestations
    pub aggregator_committee_indexes: VariableList<u64, MaxCommitteeIndexes>,
    /// Aggregated attestations as SSZ bytes, one per committee index
    pub aggregated_attestations:
        VariableList<VariableList<u8, MaxAggregatedAttestationBytes>, MaxCommitteeIndexes>,
    /// Validators selected as sync committee contributors with their selection proofs
    pub contributors: VariableList<AssignedAggregator, MaxContributors>,
    /// Sync committee contributions, one per subcommittee (4 total)
    pub sync_committee_contributions:
        VariableList<SyncCommitteeContribution<E>, MaxSyncContributions>,
}
```

## Current State vs Required State

### Current `AggregatorDutyInfo` (at 2/3 slot)

```rust
pub struct AggregatorDutyInfo {
    pub slot: Slot,
    pub aggregating_attesters: HashSet<ValidatorIndex>,      // ✓ WHO is aggregating
    pub aggregator_committees: HashMap<PublicKeyBytes, u64>, // ✓ WHICH COMMITTEE
    pub sync_aggregators_by_subnet: HashMap<ValidatorIndex, HashSet<SyncSubnetId>>, // ✓ WHO + WHICH SUBNET
}
```

### Gap Analysis

| Field Needed | Type | Source | Currently Available? |
|--------------|------|--------|---------------------|
| `aggregators` | `Vec<AssignedAggregator>` | validator_index + **selection_proof** + committee_index | ❌ **Missing selection_proof** |
| `aggregator_committee_indexes` | `Vec<u64>` | Unique committee indexes | ✓ Derivable from `aggregator_committees.values()` |
| `aggregated_attestations` | `Vec<SSZ bytes>` | Beacon node API | ❌ **Must fetch at 2/3 slot** |
| `contributors` | `Vec<AssignedAggregator>` | validator_index + **selection_proof** + subcommittee_index | ❌ **Missing selection_proof** |
| `sync_committee_contributions` | `Vec<SyncCommitteeContribution>` | Beacon node API | ❌ **Must fetch at 2/3 slot** |

### Key Missing Piece: Selection Proofs

Lighthouse's `DutyAndProof` **already has the selection proof**:

```rust
// From Lighthouse duties_service.rs
pub struct DutyAndProof {
    pub duty: AttesterData,
    pub selection_proof: Option<SelectionProof>,  // ← This wraps a Signature!
    pub subscription_slots: Arc<SubscriptionSlots>,
}
```

And for sync duties, Lighthouse stores proofs in `SlotDuties.aggregators`:

```rust
// From Lighthouse sync.rs
pub struct SlotDuties {
    pub duties: Vec<SyncDuty>,
    /// Map from subnet ID to (validator_index, pubkey, selection_proof)
    pub aggregators: HashMap<SyncSubnetId, Vec<(u64, PublicKeyBytes, SyncSelectionProof)>>,
}
```

---

## Solution: Expanded `AggregatorDutyInfo`

### Step 1: Define New Info Structs

Add to `validator_store/src/lib.rs`:

```rust
use types::{selection_proof::SelectionProof, sync_selection_proof::SyncSelectionProof};

/// Attestation aggregator with their selection proof
#[derive(Debug, Clone)]
pub struct AttestationAggregatorInfo {
    pub validator_index: ValidatorIndex,
    pub pubkey: PublicKeyBytes,
    pub committee_index: u64,
    pub selection_proof: SelectionProof,
}

/// Sync committee contributor with their selection proof
#[derive(Debug, Clone)]
pub struct SyncContributorInfo {
    pub validator_index: ValidatorIndex,
    pub pubkey: PublicKeyBytes,
    pub subnet_id: SyncSubnetId,
    pub selection_proof: SyncSelectionProof,
}
```

### Step 2: Update `AggregatorDutyInfo`

```rust
pub struct AggregatorDutyInfo {
    pub slot: Slot,
    
    /// Attestation aggregators with full info including selection proofs
    pub attestation_aggregators: Vec<AttestationAggregatorInfo>,
    
    /// Sync committee contributors with full info including selection proofs
    pub sync_contributors: Vec<SyncContributorInfo>,
}

impl AggregatorDutyInfo {
    /// Get unique committee indexes that have aggregators
    pub fn aggregator_committee_indexes(&self) -> Vec<u64> {
        self.attestation_aggregators
            .iter()
            .map(|a| a.committee_index)
            .collect::<HashSet<_>>()
            .into_iter()
            .collect()
    }
    
    /// Get unique subnet IDs that have contributors
    pub fn contributor_subnet_ids(&self) -> Vec<SyncSubnetId> {
        self.sync_contributors
            .iter()
            .map(|c| c.subnet_id)
            .collect::<HashSet<_>>()
            .into_iter()
            .collect()
    }
    
    /// Total attestation aggregators for a committee filter
    pub fn attestation_aggregator_count<F>(&self, is_in_committee: F) -> usize
    where
        F: Fn(&ValidatorIndex) -> bool,
    {
        self.attestation_aggregators
            .iter()
            .filter(|a| is_in_committee(&a.validator_index))
            .count()
    }

    /// Total sync contributor assignments for a committee filter
    pub fn sync_contributor_count<F>(&self, is_in_committee: F) -> usize
    where
        F: Fn(&ValidatorIndex) -> bool,
    {
        self.sync_contributors
            .iter()
            .filter(|c| is_in_committee(&c.validator_index))
            .count()
    }
}
```

### Step 3: Update `build_aggregator_duty_info()`

In `metadata_service.rs`:

```rust
fn build_aggregator_duty_info(&self, slot: Slot) -> AggregatorDutyInfo {
    // ═══════════════════════════════════════════════════════════════
    // ATTESTATION AGGREGATORS
    // ═══════════════════════════════════════════════════════════════
    
    // Re-fetch attesters - NOW selection_proof is filled in
    let attesters = self.duties_service.attesters(slot);
    
    let attestation_aggregators: Vec<AttestationAggregatorInfo> = attesters
        .filter_map(|duty_and_proof| {
            // selection_proof.is_some() means is_aggregator = true
            duty_and_proof.selection_proof.as_ref().map(|proof| {
                AttestationAggregatorInfo {
                    validator_index: ValidatorIndex(duty_and_proof.duty.validator_index as usize),
                    pubkey: duty_and_proof.duty.pubkey,
                    committee_index: duty_and_proof.duty.committee_index,
                    selection_proof: proof.clone(),
                }
            })
        })
        .collect();

    // ═══════════════════════════════════════════════════════════════
    // SYNC COMMITTEE CONTRIBUTORS
    // ═══════════════════════════════════════════════════════════════
    
    let sync_duties = self
        .duties_service
        .sync_duties
        .get_duties_for_slot::<E>(slot, &self.spec);

    let sync_contributors: Vec<SyncContributorInfo> = sync_duties
        .map(|duties| {
            duties.aggregators
                .into_iter()
                .flat_map(|(subnet_id, aggregators)| {
                    aggregators.into_iter().map(move |(validator_index, pk, proof)| {
                        SyncContributorInfo {
                            validator_index: ValidatorIndex(validator_index as usize),
                            pubkey: pk,
                            subnet_id,
                            selection_proof: proof,
                        }
                    })
                })
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();

    AggregatorDutyInfo {
        slot,
        attestation_aggregators,
        sync_contributors,
    }
}
```

---

## Building Consensus Data at 2/3 Slot

### Timeline

```
┌─────────────────────────────────────────────────────────────────────────┐
│ SLOT N                                                                  │
├─────────────────────────────────────────────────────────────────────────┤
│ 0/3 (slot start)                                                        │
│   └─► ValidatorDutyInfo cached (who attests, who is in sync committee)  │
│                                                                         │
│ 1/3 slot                                                                │
│   └─► SlotMetadata cached (beacon_vote with block_root, checkpoints)    │
│   └─► Attestations published by non-aggregator validators               │
│   └─► SyncCommitteeMessages published                                   │
│                                                                         │
│ 2/3 slot ← YOU ARE HERE                                                 │
│   └─► AggregatorDutyInfo cached (aggregators + contributors WITH PROOFS)│
│   └─► Fetch aggregated attestations from BN                             │
│   └─► Fetch sync contributions from BN                                  │
│   └─► Build AggregatorCommitteeConsensusData                            │
│   └─► Run committee-based QBFT                                          │
│   └─► Post-consensus: sign and publish                                  │
└─────────────────────────────────────────────────────────────────────────┘
```

### Step-by-Step Process

#### Step 1: Get `AggregatorDutyInfo` (with proofs)

```rust
// Wait for AggregatorDutyInfo to be available at 2/3 slot
let aggregator_info = validator_store.get_aggregator_duty_info(slot).await?;

// Filter to only validators in our committee
let our_attestation_aggregators: Vec<_> = aggregator_info
    .attestation_aggregators
    .iter()
    .filter(|a| committee.validators.contains(&a.validator_index))
    .collect();

let our_sync_contributors: Vec<_> = aggregator_info
    .sync_contributors
    .iter()
    .filter(|c| committee.validators.contains(&c.validator_index))
    .collect();
```

#### Step 2: Fetch Aggregated Attestations from Beacon Node

The `attestation_data_root` is constructed from `SlotMetadata.beacon_vote` (cached at 1/3 slot):

```rust
// GET /eth/v1/validator/aggregate_attestation
// Query params: slot, attestation_data_root, committee_index

// First, get SlotMetadata which was cached at 1/3 slot
let slot_metadata = validator_store.get_slot_metadata(slot).await?;

let committee_indexes = our_attestation_aggregators
    .iter()
    .map(|a| a.committee_index)
    .collect::<HashSet<_>>();

let mut aggregated_attestations: HashMap<u64, Attestation<E>> = HashMap::new();

for committee_index in committee_indexes {
    // Construct AttestationData from beacon_vote + committee_index
    // All fields are available from SlotMetadata!
    let attestation_data = AttestationData {
        slot,
        index: committee_index,
        beacon_block_root: slot_metadata.beacon_vote.block_root,
        source: slot_metadata.beacon_vote.source,
        target: slot_metadata.beacon_vote.target,
    };
    
    // Compute the root for the BN API query
    let attestation_data_root = attestation_data.tree_hash_root();
    
    // Fetch from beacon node (one request per committee)
    // Note: Use v2 API for fork-aware response
    let aggregate = beacon_node
        .get_validator_aggregate_attestation_v2(slot, &attestation_data_root, committee_index)
        .await?
        .ok_or(Error::NoAggregateFound)?
        .data;
    
    aggregated_attestations.insert(committee_index, aggregate);
}
```

**Key Insight**: The `BeaconVote` from `SlotMetadata` contains exactly what we need:

| AttestationData Field | Source |
|----------------------|--------|
| `slot` | Current slot |
| `index` | `aggregator.committee_index` from duty |
| `beacon_block_root` | `slot_metadata.beacon_vote.block_root` |
| `source` | `slot_metadata.beacon_vote.source` |
| `target` | `slot_metadata.beacon_vote.target` |
```

#### Step 3: Fetch Sync Committee Contributions from Beacon Node

For each unique subnet that has a contributor:

```rust
// GET /eth/v1/validator/sync_committee_contribution
// Query params: slot, beacon_block_root, subcommittee_index

let subnet_ids = our_sync_contributors
    .iter()
    .map(|c| c.subnet_id)
    .collect::<HashSet<_>>();

let mut sync_contributions: HashMap<SyncSubnetId, SyncCommitteeContribution<E>> = HashMap::new();

for subnet_id in subnet_ids {
    let contribution_data = SyncContributionData {
        slot,
        beacon_block_root: slot_metadata.beacon_vote.block_root,
        subcommittee_index: subnet_id.into(),
    };
    
    // Fetch from beacon node (one request per subnet)
    let contribution = beacon_node
        .get_validator_sync_committee_contribution(&contribution_data)
        .await?
        .ok_or(Error::NoContributionFound)?
        .data;
    
    sync_contributions.insert(subnet_id, contribution);
}
```

#### Step 4: Build `AggregatorCommitteeConsensusData`

```rust
use ssv_types::consensus::{
    AggregatorCommitteeConsensusData, AssignedAggregator, DataVersion,
};

fn build_aggregator_committee_consensus_data<E: EthSpec>(
    fork_name: ForkName,
    attestation_aggregators: &[&AttestationAggregatorInfo],
    aggregated_attestations: &HashMap<u64, Attestation<E>>,
    sync_contributors: &[&SyncContributorInfo],
    sync_contributions: &HashMap<SyncSubnetId, SyncCommitteeContribution<E>>,
) -> Result<AggregatorCommitteeConsensusData<E>, Error> {
    
    // ═══════════════════════════════════════════════════════════════
    // Build aggregators list
    // ═══════════════════════════════════════════════════════════════
    let aggregators: Vec<AssignedAggregator> = attestation_aggregators
        .iter()
        .map(|a| AssignedAggregator {
            validator_index: a.validator_index,
            selection_proof: a.selection_proof.clone().into(), // SelectionProof -> Signature
            committee_index: a.committee_index,
        })
        .collect();

    // ═══════════════════════════════════════════════════════════════
    // Build committee indexes and attestations (must be parallel arrays)
    // ═══════════════════════════════════════════════════════════════
    let mut aggregator_committee_indexes: Vec<u64> = aggregated_attestations
        .keys()
        .copied()
        .collect();
    aggregator_committee_indexes.sort(); // Ensure deterministic ordering
    
    let aggregated_attestations_bytes: Vec<VariableList<u8, _>> = aggregator_committee_indexes
        .iter()
        .map(|idx| {
            let attestation = &aggregated_attestations[idx];
            VariableList::from(attestation.as_ssz_bytes())
        })
        .collect();

    // ═══════════════════════════════════════════════════════════════
    // Build contributors list
    // ═══════════════════════════════════════════════════════════════
    let contributors: Vec<AssignedAggregator> = sync_contributors
        .iter()
        .map(|c| AssignedAggregator {
            validator_index: c.validator_index,
            selection_proof: c.selection_proof.clone().into(), // SyncSelectionProof -> Signature
            committee_index: c.subnet_id.into(), // Subcommittee index
        })
        .collect();

    // ═══════════════════════════════════════════════════════════════
    // Build sync contributions (ordered by subcommittee index)
    // ═══════════════════════════════════════════════════════════════
    let mut sync_contributions_vec: Vec<_> = sync_contributions
        .iter()
        .map(|(subnet_id, contribution)| (subnet_id, contribution.clone()))
        .collect();
    sync_contributions_vec.sort_by_key(|(subnet_id, _)| **subnet_id);
    
    let sync_committee_contributions: Vec<SyncCommitteeContribution<E>> = sync_contributions_vec
        .into_iter()
        .map(|(_, contribution)| contribution)
        .collect();

    // ═══════════════════════════════════════════════════════════════
    // Assemble final structure
    // ═══════════════════════════════════════════════════════════════
    Ok(AggregatorCommitteeConsensusData {
        version: DataVersion::from(fork_name),
        aggregators: VariableList::from(aggregators),
        aggregator_committee_indexes: VariableList::from(aggregator_committee_indexes),
        aggregated_attestations: VariableList::from(aggregated_attestations_bytes),
        contributors: VariableList::from(contributors),
        sync_committee_contributions: VariableList::from(sync_committee_contributions),
    })
}
```

---

## QBFT Instance Configuration

### Instance ID

```rust
// Use CommitteeInstanceId for committee-based QBFT
let instance_id = CommitteeInstanceId {
    committee_id,
    duty: CommitteeDutyKind::AggregatorCommittee, // New duty kind
    instance_height: slot.as_usize().into(),
};
```

### Role

```rust
// Use Role::AggregatorCommittee (Role 6 in Go SSV)
let role = Role::AggregatorCommittee;
```

### Expected Message Counts

For pre-consensus partial signatures:
- Attestation selection proofs: `N` per aggregator (one per aggregator)
- Sync selection proofs: `M` per contributor (one per subnet they contribute to)

```rust
// Count for signature collection
let expected_selection_proofs = 
    our_attestation_aggregators.len() + 
    our_sync_contributors.len();
```

---

## Performance Considerations

### Latency Budget

At 2/3 slot (8 seconds into a 12-second slot), we have ~4 seconds remaining:

```
┌─────────────────────────────────────────────────────────────────────────┐
│ TIME BUDGET: 4 seconds remaining after 2/3 slot                        │
├─────────────────────────────────────────────────────────────────────────┤
│ Fetch aggregates/contributions (parallel)     ~50-200ms                 │
│ Build consensus data                          ~1-5ms                    │
│ QBFT consensus (3+ rounds)                    ~500-2000ms               │
│ Post-consensus signing                        ~100-500ms                │
│ Publish to network                            ~50-100ms                 │
│                                               ─────────────             │
│ TOTAL                                         ~700-2800ms               │
│ BUFFER                                        ~1200-3300ms ✓            │
└─────────────────────────────────────────────────────────────────────────┘
```

### Critical: Parallelize All Beacon Node Fetches

**DON'T** fetch sequentially:

```rust
// BAD - Sequential fetches (50-100ms × N)
for committee_index in committee_indexes {
    let aggregate = beacon_node.get_aggregate(slot, root, committee_index).await;
    aggregates.push(aggregate);
}
```

**DO** fetch in parallel with fallback:

```rust
// GOOD - Parallel fetches with beacon node fallback (~50-100ms total)

// Each fetch uses first_success for BN failover
let aggregate_futures: Vec<_> = committee_indexes
    .iter()
    .map(|committee_index| {
        let beacon_nodes = beacon_nodes.clone();
        let attestation_data = AttestationData {
            slot,
            index: *committee_index,
            beacon_block_root: slot_metadata.beacon_vote.block_root,
            source: slot_metadata.beacon_vote.source,
            target: slot_metadata.beacon_vote.target,
        };
        let attestation_data_root = attestation_data.tree_hash_root();
        
        async move {
            // first_success tries each BN until one succeeds
            beacon_nodes
                .first_success(|beacon_node| {
                    let root = attestation_data_root;
                    async move {
                        beacon_node
                            .get_validator_aggregate_attestation_v2(slot, &root, *committee_index)
                            .await
                    }
                })
                .await
                .map(|resp| (*committee_index, resp))
        }
    })
    .collect();

let sync_futures: Vec<_> = subnet_ids
    .iter()
    .map(|subnet_id| {
        let beacon_nodes = beacon_nodes.clone();
        let block_root = slot_metadata.beacon_vote.block_root;
        
        async move {
            beacon_nodes
                .first_success(|beacon_node| {
                    let contribution_data = SyncContributionData {
                        slot,
                        beacon_block_root: block_root,
                        subcommittee_index: (*subnet_id).into(),
                    };
                    async move {
                        beacon_node
                            .get_validator_sync_committee_contribution(&contribution_data)
                            .await
                    }
                })
                .await
                .map(|resp| (*subnet_id, resp))
        }
    })
    .collect();

// Fetch ALL in parallel - each with its own BN failover
let (aggregate_results, contribution_results) = tokio::join!(
    futures::future::join_all(aggregate_futures),
    futures::future::join_all(sync_futures),
);
```

> **Note on Parallelism**: `tokio::join!` + `join_all` already provides full **concurrent** 
> execution. All HTTP requests run simultaneously and interleave on the same task. Since 
> HTTP requests are I/O-bound (waiting for network), not CPU-bound, there's no benefit to 
> spawning separate threads with `tokio::spawn`. The async runtime efficiently multiplexes 
> all requests while any is waiting for a response.
```

### Handling Partial Failures

Some fetches may fail even with fallback (all BNs down, no attestations yet, etc.):

```rust
// Process results, collecting successes and logging failures
let mut aggregated_attestations: HashMap<u64, Attestation<E>> = HashMap::new();
let mut sync_contributions: HashMap<SyncSubnetId, SyncCommitteeContribution<E>> = HashMap::new();

for result in aggregate_results {
    match result {
        Ok((committee_index, Some(response))) => {
            aggregated_attestations.insert(committee_index, response.data);
        }
        Ok((committee_index, None)) => {
            // BN returned success but no aggregate available yet
            warn!(
                slot = %slot,
                committee_index = committee_index,
                "No aggregate attestation available from beacon node"
            );
        }
        Err(e) => {
            // All beacon nodes failed for this committee
            warn!(
                slot = %slot,
                error = %e,
                "Failed to fetch aggregate attestation from all beacon nodes"
            );
        }
    }
}

for result in contribution_results {
    match result {
        Ok((subnet_id, Some(response))) => {
            sync_contributions.insert(subnet_id, response.data);
        }
        Ok((subnet_id, None)) => {
            warn!(
                slot = %slot,
                subnet_id = ?subnet_id,
                "No sync contribution available from beacon node"
            );
        }
        Err(e) => {
            warn!(
                slot = %slot,
                error = %e,
                "Failed to fetch sync contribution from all beacon nodes"
            );
        }
    }
}

// Decide: continue with partial data or abort?
// Option 1: Continue with whatever we got (graceful degradation)
// Option 2: Require at least one of each type
if aggregated_attestations.is_empty() && sync_contributions.is_empty() {
    return Err(Error::NoDataAvailable);
}
```

### Failure Modes and Behavior

| Scenario | Behavior |
|----------|----------|
| BN1 fails, BN2 succeeds | `first_success` failover - transparent |
| All BNs fail for 1 committee | Log warning, exclude from consensus data |
| All BNs fail for ALL committees | Return error, skip QBFT |
| BN returns None (no aggregate yet) | Log warning, exclude from consensus data |
| Timeout on one fetch | Doesn't block others (parallel), failover to next BN |
```

### Why This Works

1. **Beacon node is localhost**: Typical latency ~0.1-1ms network + 10-50ms processing
2. **Connection pooling**: Reuse HTTP connections, near-zero handshake overhead
3. **Realistic counts**: An SSV committee typically has:
   - 1-5 beacon committees with aggregators (not 64)
   - 1-3 sync subnets with contributors (not 4)
4. **Parallel execution**: All requests complete in time of slowest (~100ms), not sum

### BN Rate Limiting / Connection Contention?

**Short answer: Not a concern.** Lighthouse's validator client uses the same pattern with no throttling:

| Concern | Reality |
|---------|---------|
| Rate limiting | BN clients (Lighthouse, Prysm, etc.) don't rate-limit localhost connections |
| Connection contention | reqwest's default connection pool handles 5-10 parallel requests trivially |
| Request count | We're making 2-8 requests total, not hundreds |

**What Lighthouse does:**
- Uses `join_all()` for parallel local signing (same as our pattern)
- Uses `first_success()` for BN fetches (sequential with fallback)
- Uses `broadcast()` → `join_all()` for publishing to all BNs
- **No explicit connection pool config** - relies on reqwest defaults
- **No rate limiting or throttling**

The difference: Lighthouse's `first_success()` is sequential per-request because they're fetching the *same* data from fallback BNs. We're fetching *different* data (different committees/subnets) so parallel is correct.

### HTTP Client Configuration

```rust
// Lighthouse approach: mostly defaults, timeout based on slot duration
let client = reqwest::Client::builder()
    .timeout(slot_duration / 4)  // ~3 seconds for 12s slots
    .build()?;

// More conservative (optional):
let client = reqwest::Client::builder()
    .pool_max_idle_per_host(10)
    .pool_idle_timeout(Duration::from_secs(60))
    .timeout(Duration::from_secs(2))
    .build()?;
```

---

## Beacon Node API Reference

### Get Aggregated Attestation

```
GET /eth/v1/validator/aggregate_attestation

Query Parameters:
  - slot: Slot
  - attestation_data_root: Hash256
  - committee_index: u64

Response: { data: Attestation }
```

### Get Sync Committee Contribution

```
GET /eth/v1/validator/sync_committee_contribution

Query Parameters:
  - slot: Slot
  - beacon_block_root: Hash256
  - subcommittee_index: u64

Response: { data: SyncCommitteeContribution }
```

---

## Error Handling

### Empty Committee Cases

If no validators in the committee are aggregators or contributors:

```rust
if our_attestation_aggregators.is_empty() && our_sync_contributors.is_empty() {
    // Nothing to aggregate - skip QBFT entirely
    // This mirrors Go SSV's ErrNoValidDutiesToExecute
    return Ok(());
}
```

### Missing Aggregates/Contributions

If the beacon node returns no aggregate for a committee:

```rust
// Log and continue - some committees may not have enough attestations yet
if aggregate.is_none() {
    warn!(
        slot = %slot,
        committee_index = committee_index,
        "No aggregate attestation available from beacon node"
    );
    // Remove this committee from consensus data
    continue;
}
```

---

## Summary Checklist

- [ ] Add `AttestationAggregatorInfo` and `SyncContributorInfo` structs
- [ ] Update `AggregatorDutyInfo` to store proofs
- [ ] Update `build_aggregator_duty_info()` to capture proofs from Lighthouse duties
- [ ] Add beacon node client methods for aggregate fetching
- [ ] Implement `build_aggregator_committee_consensus_data()` 
- [ ] Add `CommitteeDutyKind::AggregatorCommittee` variant
- [ ] Create new QBFT flow for AggregatorCommittee role
- [ ] Handle empty committee edge cases
- [ ] Implement post-consensus signing for both attestation aggregates and sync contributions
