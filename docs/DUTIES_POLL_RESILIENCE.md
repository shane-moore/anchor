# Duties Poll Resilience and Graceful Degradation

## Overview

This document captures the analysis and design decisions around handling DutiesService poll signals in Anchor's MetadataService. The poll signals ensure that Lighthouse's duties cache is populated before MetadataService reads from it to build `VotingAssignments`.

## Three-Phase Architecture

MetadataService operates in three phases per slot:

### Phase 1: VotingAssignments (Slot Start)

**Purpose:** Build `VotingAssignments` containing which validators are attesting and which are in sync committees.

**Flow:**
1. Wait for `attesters_poll` and `sync_poll` signals from Lighthouse
2. Read cached duties from `duties_service.attesters(slot)` and `duties_service.sync_duties.get_duties_for_slot()`
3. Build `VotingAssignments`:
   - `attesting_validators` - validators attesting this slot
   - `attesting_committees` - pubkey → committee_index mapping
   - `sync_validators_by_subnet` - sync validators → subnet IDs
4. Publish via `validator_store.update_voting_assignments()`

### Phase 2: VotingContext (1/3 Slot)

**Purpose:** Fetch `BeaconVote` from beacon node and combine with cached `VotingAssignments`.

**Flow:**
1. Get cached `VotingAssignments` from Phase 1
2. Fetch `attestation_data` from beacon node → convert to `BeaconVote`
3. Publish `VotingContext` (contains both)

### Phase 3: AggregationAssignments (2/3 Slot)

**Purpose:** Build `AggregatorCommitteeConsensusData` for validators that ARE aggregators.

**Flow:**
1. Re-read duties from `duties_service.attesters(slot)` - now `selection_proof.is_some()` indicates aggregator
2. Build `AggregationAssignments` with aggregator-specific data
3. Build `AggregatorCommitteeConsensusData` per SSV committee

## How Selection Proofs Use VotingAssignments

When Lighthouse calls `produce_selection_proof` or `produce_sync_selection_proof`:

```rust
// In produce_selection_proof (Boole+ fork)
let voting_assignments = self.get_voting_assignments(slot).await?;

// Use VotingAssignments to count how many selection proofs to collect
let num_signatures_to_collect = voting_assignments
    .selection_proof_count_for_committee(|idx| {
        committee_validator_indices.contains(idx)
    });

// This count is used for committee-based signature batching
let collection_mode = CollectionMode::Committee {
    num_signatures_to_collect,
    base_hash,
};
```

**Key insight:** `VotingAssignments` tells us how many validators in the SSV committee need selection proofs, so we know when we've collected enough partial signatures to reconstruct the full signature.

## Why We Wait for Poll Signals

The poll signals ensure:
1. Lighthouse has polled the beacon node for this slot's duties
2. The duties are cached and available to read
3. We don't read stale data from the previous slot

Without waiting:
- `duties_service.attesters(slot)` might return empty or stale data
- `VotingAssignments` would have wrong validator counts
- `produce_selection_proof` would calculate wrong `num_signatures_to_collect`
- Signature collection would timeout waiting for signatures that will never arrive

## Failure Scenarios

### Scenarios Where Signals Are NOT Sent

| Scenario | Attesters | Sync | Impact |
|----------|-----------|------|--------|
| No validators registered | Fixed* | N/A | Signal now sent |
| Duties already cached | Fixed* | N/A | Signal now sent |
| Pre-Altair fork | N/A | Fixed* | Signal now sent |
| Beacon node API failure | No signal | No signal | Handled by timeout |
| Slot clock failure | No signal | No signal | Handled by timeout |
| Channel closed (shutdown) | `is_err()` | `is_err()` | Handled gracefully |

*Fixed in Lighthouse `validator_services` crate (see below)

### Lighthouse Fixes Applied

We added poll signal firing to early return paths in Lighthouse to prevent MetadataService hangs:

**duties_service.rs:**
```rust
// No validators early return - signal now sent
// DESIGN RATIONALE: Even with no validators, MetadataService needs to proceed
// so it doesn't block waiting forever. The duties cache will be empty, and
// VotingAssignments will correctly reflect no attesting validators.
if local_indices.is_empty() {
    debug!(%epoch, "No validators, not downloading duties");
    duties_service.attesters_poll_tx.send_replace(current_slot);
    return Ok(());
}

// Duties already cached early return - signal now sent
// DESIGN RATIONALE: When duties are already cached for this epoch (common on
// epoch boundaries when we poll multiple times), we still need to signal so
// MetadataService can proceed. The cached duties are valid and will be read.
if validators_to_update.is_empty() {
    duties_service.attesters_poll_tx.send_replace(current_slot);
    return Ok(());
}
```

**sync.rs:**
```rust
// Pre-Altair early return - signal now sent
// DESIGN RATIONALE: Before Altair fork, sync committees don't exist, but
// MetadataService still needs to proceed with attester duties. Without this
// signal, MetadataService would hang waiting for sync poll forever.
if spec.altair_fork_epoch.is_none_or(|altair_epoch| current_epoch < altair_epoch) {
    duties_service.sync_poll_tx.send_replace(current_slot);
    return Ok(());
}
```

### Remaining Failure Cases

After the Lighthouse fixes, signals are only NOT sent when:
1. **Beacon node failure** - Network timeout, BN offline, 5xx errors
2. **Data consistency errors** - Rare internal errors in duties response parsing
3. **Slot clock failure** - System clock issues (very rare)

---

## Recommended Implementation

### Design Principles

1. **Never block indefinitely**: Use timeouts to prevent hangs on external failures
2. **Always attempt to build VotingAssignments**: Even if polls fail, try reading from cache
3. **Fail open, not closed**: Prefer potentially stale data over no data
4. **Parallel execution**: Wait for both polls simultaneously to minimize latency
5. **Rich observability**: Log and measure every failure path

### Why Not `tokio::select!`?

`tokio::select!` waits for the **first** future to complete, then cancels the others. We need **both** polls to complete (or timeout independently), so `tokio::join!` is the correct choice.

### The Approach: Parallel Timeout with Cache Fallback

Wait up to 3.5 seconds for both poll signals in parallel. Regardless of whether polls succeed or timeout, always attempt to read duties from the cache and build VotingAssignments.

```rust
// In start_update_service(), the Phase 1 spawned task:
//
// DESIGN RATIONALE:
// We wait for poll signals to ensure Lighthouse has fetched fresh duties from the
// beacon node. However, we don't fail if signals don't arrive - we fall back to
// reading whatever is in the duties cache. This handles several scenarios:
//
// 1. Normal operation: Signals arrive quickly (< 100ms), we read fresh duties
// 2. Beacon node slow: Signal arrives late (< 3s), we still get fresh duties
// 3. Beacon node down: Timeout after 3.5s, we read from cache (stale or empty)
//
// Note: On first slot after restart, if poll succeeds the cache IS populated
// (that's what the signal means). Cache is only empty if poll times out AND
// this is a fresh restart (no prior cached data).
//
// The 3.5 second timeout is chosen because:
// - Lighthouse BN API timeout is 3 seconds (slot_duration / 4)
// - Phase 2 starts at 4 seconds (1/3 slot)
// - This gives 500ms buffer for any post-BN-response processing

let self_clone_phase1 = self.clone();
executor.spawn(
    async move {
        let mut attesters_poll_rx = self_clone_phase1.attesters_poll_rx.clone();
        let mut sync_poll_rx = self_clone_phase1.sync_poll_rx.clone();
        let poll_timeout = Duration::from_millis(3500);

        loop {
            if let Some(duration_to_next_slot) =
                self_clone_phase1.slot_clock.duration_to_next_slot()
            {
                sleep(duration_to_next_slot).await;

                let slot = self_clone_phase1.slot_clock.now().unwrap_or_default();
                let poll_start = Instant::now();

                // ════════════════════════════════════════════════════════════════
                // Wait for poll signals with timeout (parallel execution)
                //
                // We MUST run both waits in parallel using tokio::join!. Running
                // them sequentially would mean worst case 7 seconds (3.5s + 3.5s),
                // which would miss Phase 2's deadline at 4 seconds into the slot.
                // ════════════════════════════════════════════════════════════════

                let (attesters_ready, sync_ready) = tokio::join!(
                    async {
                        tokio::time::timeout(
                            poll_timeout,
                            attesters_poll_rx.wait_for(|&s| s >= slot),
                        )
                        .await
                        .is_ok_and(|r| r.is_ok())
                    },
                    async {
                        tokio::time::timeout(
                            poll_timeout,
                            sync_poll_rx.wait_for(|&s| s >= slot),
                        )
                        .await
                        .is_ok_and(|r| r.is_ok())
                    }
                );

                let poll_duration = poll_start.elapsed();

                // Log poll results
                if !attesters_ready {
                    warn!(
                        %slot,
                        poll_duration_ms = poll_duration.as_millis(),
                        "Attesters poll failed or timed out - will use cached duties"
                    );
                }
                if !sync_ready {
                    warn!(
                        %slot,
                        poll_duration_ms = poll_duration.as_millis(),
                        "Sync poll failed or timed out - will use cached duties"
                    );
                }

                // Record poll telemetry
                if let Some(m) = metrics::METADATA_SERVICE_POLL_TOTAL.as_ref().ok() {
                    m.with_label_values(&[
                        metrics::ATTESTERS,
                        if attesters_ready { metrics::SUCCESS } else { metrics::FAILED },
                    ]).inc();
                    m.with_label_values(&[
                        metrics::SYNC,
                        if sync_ready { metrics::SUCCESS } else { metrics::FAILED },
                    ]).inc();
                }
                if let Some(m) = metrics::METADATA_SERVICE_POLL_DURATION.as_ref().ok() {
                    m.observe(poll_duration.as_secs_f64());
                }

                // ════════════════════════════════════════════════════════════════
                // Always call update_voting_assignments regardless of poll results
                //
                // DESIGN RATIONALE: Even if polls failed, the duties cache may have:
                // - Fresh data from a previous poll this slot
                // - Stale data from previous slot/epoch (better than nothing)
                // - Empty data (first slot after restart)
                //
                // We proceed with whatever is available. Downstream code handles
                // empty data gracefully.
                // ════════════════════════════════════════════════════════════════

                if let Err(err) = self_clone_phase1.update_voting_assignments() {
                    error!(err, "Failed to update validator voting assignments");
                }
            } else {
                error!("Failed to read slot clock");
                sleep(slot_duration).await;
            }
        }
    },
    "voting_assignments_service",
);
```

The `update_voting_assignments()` method remains a synchronous function that reads from the duties cache and builds the `VotingAssignments` struct - it doesn't need to change.

---

## Implications and Edge Cases

### Node Restart Scenarios

**Scenario: Anchor restarts mid-epoch**
- DutiesService starts fresh with empty cache
- First poll will fetch duties from BN and populate cache
- MetadataService waits for poll signal before reading
- Result: First slot after restart works correctly

**Scenario: Lighthouse restarts mid-slot**
- Poll channels are recreated, MetadataService has old receivers
- Old receivers will see channel closed error
- MetadataService logs warning and reads from cache
- Cache may be stale or empty depending on timing
- Result: Graceful degradation with potentially stale data

**Scenario: Beacon node restarts**
- DutiesService poll fails with network error
- No poll signal sent (no early return triggered)
- MetadataService times out after 3.5 seconds
- Reads from cache (may have data from before restart)
- Result: Continues with cached data, logs timeout warning

### Epoch Boundary Behavior

**Scenario: Slot 0 of new epoch**
- DutiesService fetches new epoch's duties
- Takes longer than normal slots (more data to fetch)
- Signal may arrive close to 3 second mark
- MetadataService 3.5s timeout accommodates this

**Scenario: Duties already cached (re-poll same epoch)**
- DutiesService detects no validators need updates
- With Lighthouse fix, signal is sent immediately
- MetadataService proceeds without waiting
- Result: Sub-millisecond poll wait time

### Stale Data Implications

**What can go wrong with stale attester duties:**
1. `attesting_validators` has wrong validators listed
2. `produce_selection_proof` calculates wrong `num_signatures_to_collect`
3. Signature collection waits for N signatures but only M arrive (or vice versa)
4. Collection times out or completes with wrong count

**What can go wrong with stale sync duties:**
1. `sync_validators_by_subnet` has wrong validators per subnet
2. `produce_sync_selection_proof` has wrong counts
3. Similar signature collection issues

**Mitigation:**
- Other SSV operators likely have correct data
- QBFT consensus will use operator with best data
- Only affects this operator's contribution, not final signature

### DutiesService Polling Frequency

DutiesService polls at these intervals:
- **Attesters**: Once per slot, triggered by slot clock
- **Sync committees**: Once per slot, triggered by slot clock

The poll signal indicates completion of the poll, not that duties changed. Same data may be returned if already cached, but signal is still sent (after Lighthouse fixes).

---

## Telemetry Specification

### Metric Definitions (in `validator_store/src/metrics.rs`)

```rust
use std::sync::LazyLock;

pub use metrics::*;

// Poll outcome labels
pub const SUCCESS: &str = "success";
pub const FAILED: &str = "failed";
pub const ATTESTERS: &str = "attesters";
pub const SYNC: &str = "sync";

/// Count of poll attempts by type (attesters/sync) and outcome (success/failed)
pub static METADATA_SERVICE_POLL_TOTAL: LazyLock<Result<IntCounterVec>> =
    LazyLock::new(|| {
        try_create_int_counter_vec(
            "anchor_metadata_service_poll_total",
            "Count of DutiesService poll wait attempts",
            &["type", "outcome"],
        )
    });

/// Duration of poll wait phase (both polls in parallel)
pub static METADATA_SERVICE_POLL_DURATION: LazyLock<Result<Histogram>> =
    LazyLock::new(|| {
        try_create_histogram(
            "anchor_metadata_service_poll_duration_seconds",
            "Duration waiting for DutiesService poll signals",
        )
    });

/// Current count of attesting validators in VotingAssignments
pub static METADATA_SERVICE_ATTESTING_VALIDATORS: LazyLock<Result<IntGauge>> =
    LazyLock::new(|| {
        try_create_int_gauge(
            "anchor_metadata_service_attesting_validators",
            "Count of validators with attestation duties this slot",
        )
    });

/// Current count of sync committee validators in VotingAssignments
pub static METADATA_SERVICE_SYNC_VALIDATORS: LazyLock<Result<IntGauge>> =
    LazyLock::new(|| {
        try_create_int_gauge(
            "anchor_metadata_service_sync_validators",
            "Count of validators with sync committee duties this slot",
        )
    });

/// Count of slots where VotingAssignments was empty
pub static METADATA_SERVICE_EMPTY_ASSIGNMENTS_TOTAL: LazyLock<Result<IntCounter>> =
    LazyLock::new(|| {
        try_create_int_counter(
            "anchor_metadata_service_empty_assignments_total",
            "Count of slots where VotingAssignments had no duties",
        )
    });

/// Total Phase 1 duration (poll wait + cache read)
pub static METADATA_SERVICE_PHASE1_DURATION: LazyLock<Result<Histogram>> =
    LazyLock::new(|| {
        try_create_histogram(
            "anchor_metadata_service_phase1_duration_seconds",
            "Total duration of Phase 1 (VotingAssignments build)",
        )
    });
```

### Usage in MetadataService

```rust
// After poll wait
if let Some(metric) = metrics::METADATA_SERVICE_POLL_TOTAL.as_ref().ok() {
    metric.with_label_values(&[metrics::ATTESTERS, if attesters_ready { metrics::SUCCESS } else { metrics::FAILED }]).inc();
    metric.with_label_values(&[metrics::SYNC, if sync_ready { metrics::SUCCESS } else { metrics::FAILED }]).inc();
}
if let Some(metric) = metrics::METADATA_SERVICE_POLL_DURATION.as_ref().ok() {
    metric.observe(poll_duration.as_secs_f64());
}

// After building VotingAssignments
if let Some(metric) = metrics::METADATA_SERVICE_ATTESTING_VALIDATORS.as_ref().ok() {
    metric.set(attester_count as i64);
}
if let Some(metric) = metrics::METADATA_SERVICE_SYNC_VALIDATORS.as_ref().ok() {
    metric.set(sync_count as i64);
}
if attester_count == 0 && sync_count == 0 {
    if let Some(metric) = metrics::METADATA_SERVICE_EMPTY_ASSIGNMENTS_TOTAL.as_ref().ok() {
        metric.inc();
    }
}
if let Some(metric) = metrics::METADATA_SERVICE_PHASE1_DURATION.as_ref().ok() {
    metric.observe(phase1_duration.as_secs_f64());
}
```

### Recommended Alerts

| Metric | Threshold | Alert |
|--------|-----------|-------|
| `anchor_metadata_service_poll_total{outcome="failed"}` | > 0 in 5 min | BN connectivity issue |
| `anchor_metadata_service_empty_assignments_total` | > 10 in 1 epoch | Missing duties |
| `anchor_metadata_service_poll_duration_seconds` | p99 > 3s | BN latency issue |
| `anchor_metadata_service_phase1_duration_seconds` | p99 > 3.8s | Risk of missing Phase 2 deadline |

### Dashboard Panels (PromQL)

```promql
# Poll Success Rate
sum(rate(anchor_metadata_service_poll_total{outcome="success"}[5m]))
/ sum(rate(anchor_metadata_service_poll_total[5m]))

# Poll Latency p99
histogram_quantile(0.99, rate(anchor_metadata_service_poll_duration_seconds_bucket[5m]))

# Validator Counts
anchor_metadata_service_attesting_validators
anchor_metadata_service_sync_validators

# Failed Polls Rate
sum(rate(anchor_metadata_service_poll_total{outcome="failed"}[5m]))
```

---

## Timeout Value Selection

### Derivation

Lighthouse's beacon node API timeouts for duties:
```rust
// From lighthouse/common/eth2/src/lib.rs
const HTTP_ATTESTER_DUTIES_TIMEOUT_QUOTIENT: u32 = 4;
const HTTP_SYNC_DUTIES_TIMEOUT_QUOTIENT: u32 = 4;

// With base_timeout = slot duration (12 seconds):
attester_duties_timeout = 12s / 4 = 3 seconds
sync_duties_timeout = 12s / 4 = 3 seconds
```

### Recommended Timeout: 3.5 seconds

**Rationale:**
- **Lower bound (3.0s)**: Must be longer than Lighthouse's BN timeout to allow legitimate slow responses
- **Upper bound (4.0s)**: Must complete before Phase 2 starts at 1/3 slot
- **Chosen (3.5s)**: Gives 500ms buffer after BN timeout, 500ms before Phase 2 deadline

**Normal operation timing:**
- Signal typically arrives in < 100ms (duties cached or BN fast)
- Occasional slow BN responses up to 3 seconds
- Timeout at 3.5s only hit during BN outages

---

## Related Files

- `anchor/validator_store/src/metadata_service.rs` - MetadataService implementation
- `anchor/validator_store/src/lib.rs` - `VotingAssignments`, `produce_selection_proof`, `produce_sync_selection_proof`
- `lighthouse/validator_client/validator_services/src/duties_service.rs` - Attesters polling
- `lighthouse/validator_client/validator_services/src/sync.rs` - Sync committee polling
- `lighthouse/common/eth2/src/lib.rs` - BN API timeout constants
