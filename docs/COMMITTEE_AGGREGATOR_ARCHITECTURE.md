# Committee-Based Aggregator Architecture

## Overview

This document describes the architecture for implementing committee-based aggregator duties (attestation aggregation and sync committee aggregation) in Anchor, following the SSV spec's committee aggregator pattern.

The key insight is to **reuse existing patterns** from `sign_attestation` and `CollectionMode::Committee` rather than creating new infrastructure.

---

## Current Per-Validator Flow (What We're Replacing)

```
┌─────────────────────────────────────────────────────────────────────────────────┐
│                     CURRENT PER-VALIDATOR AGGREGATOR FLOW                        │
├─────────────────────────────────────────────────────────────────────────────────┤
│                                                                                  │
│  DutiesService (parallel_sign: true)                                             │
│       │                                                                          │
│       ├─► produce_selection_proof(V₁) ──┐                                        │
│       ├─► produce_selection_proof(V₂) ──┼─► Each starts INDEPENDENT collection   │
│       └─► produce_selection_proof(V₃) ──┘   using CollectionMode::SingleValidator│
│                                                                                  │
│  For EACH validator:                                                             │
│       - Sign partial with own key share                                          │
│       - Broadcast PartialSignatureMessage (1 validator per message)              │
│       - Wait for 2f+1 partial sigs                                               │
│       - Lagrange combine → full SelectionProof                                   │
│                                                                                  │
│  PROBLEM: 3 validators = 3 messages × 4 operators = 12 network messages          │
│                                                                                  │
│  Then for aggregators:                                                           │
│       - produce_signed_aggregate_and_proof() per validator                       │
│       - Each runs separate QBFT consensus                                        │
│       - Each broadcasts separate partial signature messages                       │
│                                                                                  │
└─────────────────────────────────────────────────────────────────────────────────┘
```

---

## Reference Pattern: How `sign_attestation` Batches Partial Signatures

The existing `sign_attestation` implementation demonstrates the pattern we should follow. **Crucially, it already batches BOTH attestation AND sync committee signatures into ONE message.**

### The Count Calculation (validator_store/src/lib.rs:236-251)

```rust
CollectionMode::Committee { slot_metadata, base_hash } => {
    let num_signatures_to_collect = state
        .metadata()
        .get_all_by(&committee_id)  // All validators in THIS SSV committee
        .map(|validator| {
            let mut duties = 0;
            if let Some(idx) = &validator.index {
                // +1 if this validator is attesting this slot
                if slot_metadata.attesting_validator_indices.contains(idx) {
                    duties += 1;
                }
                // +1 if this validator is in sync committee
                if slot_metadata.sync_validators.contains(idx) {
                    duties += 1;
                }
            }
            duties
        })
        .sum();

    SignatureRequester::Committee {
        num_signatures_to_collect,  // TOTAL: attestation + sync sigs expected
        base_hash,
    }
}
```

**Key Insights:**
1. Per-validator calls are made by Lighthouse's `AttestationService` and `SyncCommitteeService`
2. Coordination happens through shared `SlotMetadata` (prepared by `MetadataService`)
3. `CollectionMode::Committee` batches outgoing partial signatures
4. `base_hash` groups signatures that should be sent together
5. **Both attestation AND sync committee signatures are counted and batched together**

---

## How `CollectionMode::Committee` Works

From `signature_collector/src/lib.rs`:

```rust
pub enum SignatureRequester {
    SingleValidator { pubkey: PublicKeyBytes },
    Committee {
        num_signatures_to_collect: usize,  // How many validators in batch
        base_hash: Hash256,                 // Groups signatures together
    },
}

// In sign_and_collect():
SignatureRequester::Committee { num_signatures_to_collect, base_hash } => {
    // Get or create entry for this batch
    let mut entry = manager.committee_signatures
        .entry((base_hash, metadata.committee_id));

    // Add our signature to the batch
    collected_signatures.push(message.clone());

    // Only send when we have ALL signatures
    if collected_signatures.len() == num_signatures_to_collect {
        let signatures = entry.remove().collected_signatures;
        manager.message_sender.sign_and_send(
            manager.create_message(&metadata, signatures, &DutyExecutor::Committee(...)),
            ...
        );
    }
}
```

**We can reuse this for selection proofs!** Just need to:
1. Know `num_signatures_to_collect` (validators needing selection proofs in this committee)
2. Compute a `base_hash` for the selection proof batch

---

## The Timing Problem

The existing `SlotMetadata` is prepared at **1/3 into the slot** by `MetadataService`. But selection proofs are computed at **slot start**:

```
Slot Timeline
─────────────────────────────────────────────────────────────────────
0s (slot start)     4s (1/3 slot)           8s (2/3 slot)        12s
    │                    │                       │                 │
    ▼                    ▼                       ▼                 ▼
┌────────────┐     ┌──────────────┐        ┌─────────────┐
│ Selection  │     │ MetadataServ │        │ Aggregation │
│ Proofs     │     │ prepares     │        │ Duties      │
│ computed   │     │ SlotMetadata │        │             │
└────────────┘     └──────────────┘        └─────────────┘
      ▲                   ▲
      │                   │
      │                   └── SlotMetadata available HERE
      │
      └── Selection proofs need batch count HERE (too early!)
```

**Solution**: Use **Two-Phase MetadataService** - the recommended approach that leverages `MetadataService`'s access to both `duties_service` and `validator_store`.

---

## Why MetadataService Must Prepare the Metadata

`AnchorValidatorStore` does **not** have access to `DutiesService` - this is by design to avoid circular dependencies:

```
Dependency Flow:
1. AnchorValidatorStore is created first
2. DutiesService is created with Arc<AnchorValidatorStore>
3. MetadataService is created with Arc<DutiesService> AND Arc<AnchorValidatorStore>
```

Only `MetadataService` has access to both components, making it the right place to prepare `SelectionProofMetadata`.

---

## Understanding Selection Proof Timing

### Protocol Requirement

From the Ethereum consensus spec:
- Selection proofs just need to be ready **before 2/3 slot** (when aggregates are broadcast)
- There's **NO protocol requirement** to compute them at slot start
- Lighthouse computes them early to give DVT time for coordination

### How Lighthouse Actually Works

Looking at `DutiesService`:

1. **Every slot start:** `poll_beacon_attesters` runs
2. It fetches duties for **both** current epoch AND next epoch
3. At epoch boundary, current epoch duties were **already fetched in the previous slot**

```
Slot S-1 (last slot of epoch N-1):      │  Slot S (first slot of epoch N):
────────────────────────────────────────┼─────────────────────────────────────
poll_beacon_attesters_for_epoch(N-1)    │  poll_beacon_attesters_for_epoch(N)
poll_beacon_attesters_for_epoch(N) ◄────│    └─► Epoch N duties ALREADY CACHED
  └─► Fetches epoch N duties            │        Only checks dependent_root
                                        │  poll_beacon_attesters_for_epoch(N+1)
                                        │    └─► Fetches epoch N+1 duties
```

### Why 100ms is Usually Sufficient

In **normal operation**:
- Duties for current epoch are cached (fetched previous slot)
- Only a minimal HTTP request to verify `dependent_root` (1 validator)
- This is fast (~10-50ms typically)

### Edge Cases Where 100ms May NOT Be Enough

1. **Node restart**: All duties need fresh fetch
2. **Reorg at epoch boundary**: `dependent_root` changes, duties refetched
3. **Very slow network**: Even minimal HTTP may exceed 100ms
4. **First slot after validator added**: New validator duties fetched

### Lighthouse's Approach

`fill_in_selection_proofs` doesn't run at "slot start + delay". It:
1. Sleeps until **next slot boundary**
2. Then computes proofs for `current_slot + lookahead`

This means if duties are fetched late in slot N, proofs for slot N may be skipped entirely - this is acceptable because:
- Selection proofs are computed per-epoch, not per-slot
- Missing one slot's proof isn't catastrophic
- The proof can potentially be computed later if there's time

---

## Solution: DutiesService Watch Channels (Upstream Change)

The timing problem is solved by adding watch channels to Lighthouse's `DutiesService` that signal when duty polling completes. This eliminates race conditions entirely.

### Upstream Lighthouse Changes

```rust
// In DutiesService (Lighthouse upstream change)
impl<S: ValidatorStore, T: SlotClock> DutiesService<S, T> {
    /// Subscribe to notifications when attestation duty polling completes.
    pub fn subscribe_to_attesters_poll(&self) -> watch::Receiver<Slot> {
        self.attesters_poll_tx.subscribe()
    }

    /// Subscribe to notifications when sync committee duty polling completes.
    pub fn subscribe_to_sync_poll(&self) -> watch::Receiver<Slot> {
        self.sync_poll_tx.subscribe()
    }
}
```

These channels fire after `poll_beacon_attesters` and `poll_sync_committee_duties` complete, guaranteeing duties are cached.

### How It Works

```
Slot N Start
     │
     ├─► DutiesService::poll_beacon_attesters()
     │     - Fetches/confirms attestation duties
     │     - Sends slot N on attesters_poll_tx ◄────────────┐
     │                                                       │
     ├─► DutiesService::poll_sync_committee_duties()         │
     │     - Fetches/confirms sync duties                    │
     │     - Sends slot N on sync_poll_tx ◄─────────────┐    │
     │                                                   │    │
     ▼                                                   │    │
MetadataService (subscribes to both channels)            │    │
     │                                                   │    │
     ├─► wait_for(attesters_poll_rx, slot >= N) ─────────┼────┘
     ├─► wait_for(sync_poll_rx, slot >= N) ──────────────┘
     │
     ▼
Both signals received → Prepare SelectionProofMetadata(slot=N)
     │
     ├─► Count attesters needing selection proofs
     ├─► Count sync validators needing selection proofs (per subnet)
     ├─► Publish via watch channel
     │
     ▼
produce_selection_proof(V₁) called
     │
     ├─► get_selection_proof_metadata(slot=N)  // Waits for metadata
     ├─► Uses CollectionMode::Committee with batch count
     │
produce_selection_proof(V₂) called → same batch
produce_sync_selection_proof(V₁, subnet_0) called → same batch
...
     │
     ▼
Batch complete → Send ONE PartialSignatureMessages over P2P
```

### Benefits

1. **No timing assumptions** - waits for actual signal, not arbitrary delay
2. **Handles all edge cases** - epoch boundary, restart, reorg, slow network
3. **Clean architecture** - explicit coordination via channels
4. **Upstream contribution** - benefits all Lighthouse DVT users

---

## Proposed Architecture

### New: SelectionProofMetadata

```rust
/// Metadata for selection proof batching.
/// Prepared by MetadataService after receiving signals from DutiesService.
pub struct SelectionProofMetadata {
    pub slot: Slot,

    /// Validators needing attestation selection proofs this slot.
    /// Source: duties_service.attesters(slot)
    pub attesting_validators: Vec<ValidatorIndex>,

    /// Validators needing sync selection proofs, mapped to their subnet(s).
    /// Source: duties_service.sync_duties.get_duties_for_slot()
    /// Note: Each (validator, subnet) pair needs a separate selection proof.
    pub sync_validators_by_subnet: HashMap<ValidatorIndex, HashSet<SyncSubnetId>>,
}

impl SelectionProofMetadata {
    /// Calculate total selection proofs expected for a committee.
    /// Used for CollectionMode::Committee batch count.
    pub fn get_selection_proof_count(&self, committee_id: CommitteeId, db: &NetworkDatabase) -> usize {
        db.state()
            .metadata()
            .get_all_by(&committee_id)
            .map(|validator| {
                let mut count = 0;
                if let Some(idx) = &validator.index {
                    // +1 for attestation selection proof
                    if self.attesting_validators.contains(idx) {
                        count += 1;
                    }
                    // +N for sync selection proofs (one per subnet)
                    if let Some(subnets) = self.sync_validators_by_subnet.get(idx) {
                        count += subnets.len();
                    }
                }
                count
            })
            .sum()
    }
}
```

### Extended SlotMetadata (unchanged timing - 1/3 slot)

```rust
pub struct SlotMetadata<E: EthSpec> {
    // EXISTING fields (unchanged)
    pub slot: Slot,
    pub beacon_vote: BeaconVote,
    pub attesting_validator_indices: Vec<ValidatorIndex>,
    pub attesting_validator_committees: HashMap<PublicKeyBytes, u64>,
    pub sync_validators: Vec<ValidatorIndex>,
    pub multi_sync_aggregators: HashMap<PublicKeyBytes, ContributionWaiter<E>>,
}
```

### Modified MetadataService (Watch Channel Pattern)

MetadataService subscribes to DutiesService watch channels and waits for both signals before preparing metadata.

```rust
impl<E: EthSpec, T: SlotClock + 'static> MetadataService<E, T> {
    pub fn new(
        duties_service: Arc<DutiesService<...>>,
        validator_store: Arc<AnchorValidatorStore<T, E>>,
        // ... other fields ...
    ) -> Self {
        Self {
            // Subscribe to DutiesService watch channels
            attesters_poll_rx: duties_service.subscribe_to_attesters_poll(),
            sync_poll_rx: duties_service.subscribe_to_sync_poll(),
            // ... other fields ...
        }
    }

    pub fn start_update_service(self: Arc<Self>) -> Result<(), String> {
        let slot_duration = Duration::from_secs(self.spec.seconds_per_slot);

        // ═══════════════════════════════════════════════════════════════════════
        // TASK 1: Selection Proof Metadata (slot start)
        // Runs independently so it doesn't block the attestation flow.
        // ═══════════════════════════════════════════════════════════════════════
        let self_clone = self.clone();
        self.executor.spawn(
            async move {
                loop {
                    if let Some(duration_to_next_slot) = self_clone.slot_clock.duration_to_next_slot() {
                        // Sleep until slot start
                        sleep(duration_to_next_slot).await;

                        let slot = self_clone.slot_clock.now().unwrap_or_default();

                        // Wait for BOTH DutiesService polls to complete for this slot.
                        // This guarantees duties are cached before we read them.
                        //
                        // NO TIMEOUT: DutiesService polling is fast (cache reads normally).
                        // If this blocks, only selection proof batching is affected.
                        // The ultimate timeout is in produce_selection_proof (2/3 slot).
                        let attesters_ready = self_clone.attesters_poll_rx
                            .clone()
                            .wait_for(|&s| s >= slot);
                        let sync_ready = self_clone.sync_poll_rx
                            .clone()
                            .wait_for(|&s| s >= slot);

                        let _ = futures::future::join(attesters_ready, sync_ready).await;

                        if let Err(err) = self_clone.prepare_selection_proof_metadata(slot).await {
                            error!(?err, "Failed to prepare selection proof metadata");
                        }
                    } else {
                        error!("Failed to read slot clock");
                        sleep(slot_duration).await;
                    }
                }
            },
            "selection_proof_metadata_service",
        );

        // ═══════════════════════════════════════════════════════════════════════
        // TASK 2: Slot Metadata (1/3 slot) - EXISTING BEHAVIOR, UNCHANGED
        // Runs independently - not blocked by selection proof metadata task.
        // ═══════════════════════════════════════════════════════════════════════
        self.executor.spawn(
            async move {
                loop {
                    if let Some(duration_to_next_slot) = self.slot_clock.duration_to_next_slot() {
                        // Sleep until 1/3 into slot (existing behavior)
                        sleep(duration_to_next_slot + slot_duration / 3).await;

                        if let Err(err) = self.update_metadata().await {
                            error!(?err, "Failed to update slot metadata");
                        }
                    } else {
                        error!("Failed to read slot clock");
                        sleep(slot_duration).await;
                    }
                }
            },
            "slot_metadata_service",
        );

        Ok(())
    }

    /// Prepare SelectionProofMetadata after DutiesService signals duties are ready.
    async fn prepare_selection_proof_metadata(&self, slot: Slot) -> Result<(), String> {
        // Get attestation selection proof validators (cache read - guaranteed populated)
        let attestation_duties = self.duties_service.attesters(slot);
        let attesting_validators: Vec<ValidatorIndex> = attestation_duties
            .iter()
            .map(|d| ValidatorIndex(d.duty.validator_index as usize))
            .collect();

        // Get sync selection proof validators with their subnets (cache read - guaranteed populated)
        let sync_duties = self.duties_service.sync_duties
            .get_duties_for_slot::<E>(slot, &self.spec);

        let mut sync_validators_by_subnet: HashMap<ValidatorIndex, HashSet<SyncSubnetId>> =
            HashMap::new();

        if let Some(duties) = sync_duties {
            for duty in &duties.duties {
                let validator_index = ValidatorIndex(duty.validator_index as usize);
                if let Ok(subnet_ids) = SyncSubnetId::compute_subnets_for_sync_committee::<E>(
                    &duty.validator_sync_committee_indices,
                ) {
                    sync_validators_by_subnet
                        .entry(validator_index)
                        .or_default()
                        .extend(subnet_ids);
                }
            }
        }

        let metadata = SelectionProofMetadata {
            slot,
            attesting_validators,
            sync_validators_by_subnet,
        };

        // Publish via watch channel for produce_selection_proof to await
        self.validator_store.update_selection_proof_metadata(metadata);

        Ok(())
    }
}
```

### Modified AnchorValidatorStore (Watch Channel)

```rust
pub struct AnchorValidatorStore<T: SlotClock, E: EthSpec> {
    // ... existing fields ...

    /// Watch channel for SelectionProofMetadata (Phase 1)
    selection_proof_metadata: watch::Sender<Option<Arc<SelectionProofMetadata>>>,
}

impl<T: SlotClock, E: EthSpec> AnchorValidatorStore<T, E> {
    /// Wait for SelectionProofMetadata to be available.
    /// Blocks until MetadataService Phase 1 publishes the metadata.
    pub async fn get_selection_proof_metadata(
        &self,
        slot: Slot,
    ) -> Result<Arc<SelectionProofMetadata>, Error> {
        let metadata = self
            .selection_proof_metadata
            .subscribe()
            .wait_for(|m| m.as_ref().is_some_and(|m| m.slot >= slot))
            .await
            .ok()
            .and_then(|m| m.clone())
            .ok_or(Error::SpecificError(SpecificError::Metadata))?;

        if metadata.slot == slot {
            Ok(metadata)
        } else {
            Err(Error::SpecificError(SpecificError::Metadata))
        }
    }

    pub fn update_selection_proof_metadata(&self, metadata: SelectionProofMetadata) {
        self.selection_proof_metadata.send_replace(Some(Arc::new(metadata)));
    }
}
```

---

## Flow Diagram (Two-Phase MetadataService)

```
┌─────────────────────────────────────────────────────────────────────────────────┐
│ MetadataService (MODIFIED - Two Phases)                                          │
│                                                                                  │
│   PHASE 1 (100ms after slot start):                                              │
│     └─► prepare_selection_proof_metadata()                                       │
│           └─► Read duties_service.attesters(slot)  [CACHE READ - instant]        │
│           └─► Read duties_service.sync_duties      [CACHE READ - instant]        │
│           └─► Build SelectionProofMetadata                                       │
│           └─► Publish via watch channel                                          │
│                                                                                  │
│   PHASE 2 (1/3 slot - unchanged):                                                │
│     └─► update_metadata()                                                        │
│           └─► Fetch attestation_data from BN                                     │
│           └─► Build SlotMetadata                                                 │
│           └─► Publish via existing watch channel                                 │
│                                                                                  │
├─────────────────────────────────────────────────────────────────────────────────┤
│ DutiesService (Lighthouse - UNCHANGED)                                           │
│                                                                                  │
│   poll_beacon_attesters() [runs at slot start]                                   │
│     └─► Confirm duties in cache (or fetch if epoch boundary)                     │
│     └─► spawn fill_in_selection_proofs()                                         │
│           │                                                                      │
│           └─► produce_selection_proof(V₁) ──┐                                    │
│           └─► produce_selection_proof(V₂) ──┼─► Called in PARALLEL               │
│           └─► produce_selection_proof(V₃) ──┘                                    │
│                                                                                  │
├─────────────────────────────────────────────────────────────────────────────────┤
│ AnchorValidatorStore (MODIFIED)                                                  │
│                                                                                  │
│   produce_selection_proof(validator_pubkey, slot)                                │
│     │                                                                            │
│     ▼                                                                            │
│   get_selection_proof_metadata(slot)                                             │
│     └─► wait_for() on watch channel until metadata.slot >= slot                  │
│     └─► Return cached SelectionProofMetadata                                     │
│                                                                                  │
│     │                                                                            │
│     ▼                                                                            │
│   calculate_selection_proof_batch_count(slot, committee_id)                      │
│     └─► Use metadata to count expected partial sigs                              │
│                                                                                  │
│     │                                                                            │
│     ▼                                                                            │
│   collect_signature(CollectionMode::Committee { ... })                           │
│     └─► Batch partial sigs into ONE message                                      │
│                                                                                  │
└─────────────────────────────────────────────────────────────────────────────────┘
```

---

## Combined Partial Signature Message

Both attestation AND sync selection proofs are combined into a **single** `PartialSignatureMessages`:

```
┌─────────────────────────────────────────────────────────────────────────────────┐
│  PartialSignatureMessages {                                                      │
│    Type: AggregatorCommitteePartialSig,                                          │
│    Slot: 1000,                                                                   │
│    Messages: [                                                                   │
│      // Attestation selection proofs (SigningRoot = hash(slot, DOMAIN_SELECTION))│
│      { ValidatorIndex: V₁, SigningRoot: 0x7a8b9c..., PartialSig: σ₁¹ },         │
│      { ValidatorIndex: V₂, SigningRoot: 0x7a8b9c..., PartialSig: σ₁² },         │
│      { ValidatorIndex: V₃, SigningRoot: 0x7a8b9c..., PartialSig: σ₁³ },         │
│                                                                                  │
│      // Sync selection proofs (SigningRoot = hash(slot, subnet, DOMAIN_SYNC))    │
│      // V₁ is in sync committee subnets 0 and 2:                                 │
│      { ValidatorIndex: V₁, SigningRoot: 0xAABB..., PartialSig: σ₁⁴ },           │
│      { ValidatorIndex: V₁, SigningRoot: 0xCCDD..., PartialSig: σ₁⁵ },           │
│    ]                                                                             │
│  }                                                                               │
└─────────────────────────────────────────────────────────────────────────────────┘
```

### Count Calculation for Selection Proofs

Mirrors the existing pattern but accounts for sync proofs being **per subnet**:

```rust
/// Calculate expected partial signature count for selection proof batching.
/// Called by produce_selection_proof and produce_sync_selection_proof.
fn calculate_selection_proof_batch_count(
    &self,
    slot: Slot,
    committee_id: CommitteeId,
) -> usize {
    let selection_metadata = self.get_selection_proof_metadata(slot)?;

    let state = self.database.state();
    state
        .metadata()
        .get_all_by(&committee_id)  // All validators in THIS SSV committee
        .map(|validator| {
            let mut duties = 0;
            if let Some(idx) = &validator.index {
                // +1 for attestation selection proof (if attesting this slot)
                if selection_metadata.attesting_validators.contains(idx) {
                    duties += 1;
                }
                // +N for sync selection proofs (one per subnet)
                if let Some(subnets) = selection_metadata.sync_validators_by_subnet.get(idx) {
                    duties += subnets.len();
                }
            }
            duties
        })
        .sum()
}
```

---

## Modified `produce_selection_proof`

The key change: use `CollectionMode::Committee` with the new count calculation.

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

        // Get selection proof metadata (prepared at slot start by MetadataService)
        let selection_metadata = self.get_selection_proof_metadata(slot)?;

        // Calculate batch count using the SAME pattern as sign_attestation
        let num_signatures_to_collect =
            self.calculate_selection_proof_batch_count(slot, committee_id);

        // base_hash groups all selection proofs for this committee at this slot
        let base_hash = self.compute_selection_proof_batch_hash(slot, committee_id);

        let collection_mode = CollectionMode::Committee {
            num_signatures_to_collect,
            base_hash,
        };

        // Timeout at 2/3 slot
        let delay = Duration::from_secs(self.spec.seconds_per_slot) * 2 / 3;

        let signature = self
            .timeout_within_slot(
                slot,
                delay,
                self.collect_signature(
                    PartialSignatureKind::AggregatorCommitteePartialSig,  // Combined type
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
        SELECTION_PROOF_LOG_NAME,
        &validator_metrics::SIGNED_SELECTION_PROOFS_TOTAL,
        future,
    )
    .await
}
```

---

## Modified `produce_sync_selection_proof`

Same pattern - uses `CollectionMode::Committee` with shared count.

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

        // Signing root includes subnet_id (different from attestation selection proof)
        let domain_hash = self.get_domain(epoch, Domain::SyncCommitteeSelectionProof);
        let signing_root = SyncAggregatorSelectionData { slot, subcommittee_index: subnet_id.into() }
            .signing_root(domain_hash);

        // SAME batch count and base_hash as produce_selection_proof
        // This ensures both types of proofs batch into ONE message
        let num_signatures_to_collect =
            self.calculate_selection_proof_batch_count(slot, committee_id);
        let base_hash = self.compute_selection_proof_batch_hash(slot, committee_id);

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
                    PartialSignatureKind::AggregatorCommitteePartialSig,  // Same type!
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

---

## New Types Required

### In `ssv_types/src/partial_sig.rs`

```rust
pub enum PartialSignatureKind {
    PostConsensus = 0,
    RandaoPartialSig = 1,
    SelectionProofPartialSig = 2,      // Legacy per-validator
    ContributionProofs = 3,             // Legacy per-validator
    ValidatorRegistration = 4,
    VoluntaryExit = 5,
    AggregatorCommitteePartialSig = 6,  // NEW: Combined committee pre-consensus
}
```

### In `ssv_types/src/msgid.rs`

```rust
pub enum Role {
    Committee = 0,
    Proposer = 2,
    Aggregator = 1,              // Legacy per-validator
    SyncCommittee = 3,           // Legacy per-validator
    ValidatorRegistration = 4,
    VoluntaryExit = 5,
    AggregatorCommittee = 6,     // NEW: Committee-based aggregator duties
}
```

### In `validator_store/src/lib.rs`

```rust
/// Metadata for selection proof batching, prepared at slot start.
pub struct SelectionProofMetadata {
    pub slot: Slot,
    pub attesting_validators: Vec<ValidatorIndex>,
    pub sync_validators_by_subnet: HashMap<ValidatorIndex, HashSet<SyncSubnetId>>,
}
```

---

## End-to-End Slot Timeline (DutiesService Watch Channel Approach)

With the watch channel approach, MetadataService waits for signals from DutiesService
indicating that duty polling is complete before preparing `SelectionProofMetadata`.
This eliminates timing race conditions entirely.

```
┌─────────────────────────────────────────────────────────────────────────────────┐
│                COMMITTEE-BASED AGGREGATOR FLOW (Watch Channel Approach)          │
├─────────────────────────────────────────────────────────────────────────────────┤
│                                                                                  │
│  ═══════════════════════════════════════════════════════════════════════════════ │
│  SLOT N (Execution Phase)                                                        │
│  ═══════════════════════════════════════════════════════════════════════════════ │
│                                                                                  │
│  t = 0 (slot N start)                                                            │
│  ══════════════════════                                                          │
│       │                                                                          │
│       ├─► DutiesService::poll_beacon_attesters() [Lighthouse]                    │
│       │     - Fetches/confirms attestation duties in cache                       │
│       │     - Sends slot N on attesters_poll_tx ◄──────────────────────┐         │
│       │                                                                 │         │
│       ├─► DutiesService::poll_sync_committee_duties() [Lighthouse]      │         │
│       │     - Fetches/confirms sync duties in cache                     │         │
│       │     - Sends slot N on sync_poll_tx ◄───────────────────────┐    │         │
│       │                                                             │    │         │
│       └─► MetadataService (subscribed to both watch channels)       │    │         │
│             │                                                       │    │         │
│             ├─► wait_for(attesters_poll_rx, slot >= N) ─────────────┼────┘         │
│             ├─► wait_for(sync_poll_rx, slot >= N) ──────────────────┘              │
│             │                                                                      │
│             ▼                                                                      │
│       Both signals received → prepare_selection_proof_metadata(slot=N)             │
│             │                                                                      │
│             ├─► Query duties_service.attesters(slot N) [cache - instant]           │
│             ├─► Query duties_service.sync_duties for slot N [cache - instant]      │
│             └─► Publish SelectionProofMetadata(slot=N) via watch channel           │
│       │                                                                            │
│       ▼                                                                            │
│  fill_in_selection_proofs() [Lighthouse - spawned at slot start]                   │
│       │                                                                            │
│       ▼                                                                            │
│  produce_selection_proof(V₁) ─────┐                                                │
│       │                           │                                                │
│       ├─► get_selection_proof_metadata(slot=N)                                     │
│       │     └─► Waits for SelectionProofMetadata via watch channel                 │
│       │                           │                                                │
│  produce_selection_proof(V₂) ─────┤                                                │
│  produce_selection_proof(V₃) ─────┼─► All use CollectionMode::Committee            │
│  produce_sync_selection_proof(V₁, subnet_0) ─┤   with SAME base_hash               │
│  produce_sync_selection_proof(V₁, subnet_2) ─┘   for same committee                │
│       │                                                                            │
│       │  SignatureCollector batches ALL partial sigs                               │
│       │  Sends ONE PartialSignatureMessages when count reached                     │
│       │                                                                            │
│       ▼                                                                            │
│  Selection proofs complete → is_aggregator() determines who proceeds               │
│       │                                                                            │
│       │  V₁: IS attestation aggregator ──┐                                         │
│       │  V₂: NOT aggregator (done)       │                                         │
│       │  V₃: IS attestation aggregator ──┤                                         │
│       │  V₁: IS sync aggregator (subnet_0) ─┘                                      │
│       │                                                                            │
│  t = 1/3 slot N (~4s)                                                              │
│  ═════════════════════                                                             │
│       │                                                                            │
│       └─► MetadataService::update_metadata() [for slot N - existing behavior]      │
│             - Fetch attestation_data from beacon node                              │
│             - Create BeaconVote                                                    │
│             - Publish SlotMetadata(slot=N) via existing watch channel              │
│       │                                                                            │
│  t = 2/3 slot N (~8s)                                                              │
│  ═════════════════════                                                             │
│       │                                                                            │
│       ▼                                                                            │
│  AttestationService calls produce_signed_aggregate_and_proof() for aggregators     │
│  SyncCommitteeService calls produce_signed_contribution_and_proof()                │
│       │                                                                            │
│       ▼                                                                            │
│  [Committee QBFT and post-consensus signing - see later sections]                  │
│                                                                                    │
└────────────────────────────────────────────────────────────────────────────────────┘

KEY BENEFITS of Watch Channel Approach:
━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━
• No timing assumptions - waits for actual signal, not arbitrary delay
• Handles all edge cases - epoch boundary, restart, reorg, slow network
• Clean architecture - explicit coordination via channels
• Upstream contribution - benefits all Lighthouse DVT users
• Race-free - SelectionProofMetadata guaranteed available when needed

```

---

## Message Flow Comparison

### Before (Per-Validator)

```
Attestation Selection Proofs: 3 validators × 4 operators = 12 messages
Sync Selection Proofs:        2 proofs × 4 operators = 8 messages
QBFT Consensus:               5 instances × 4 operators × ~3 rounds = ~60 messages
Post-Consensus:               5 validators × 4 operators = 20 messages
────────────────────────────────────────────────────────────────────────
TOTAL: ~100 messages
```

### After (Committee-Based)

```
Selection Proofs (combined):  1 batch message × 4 operators = 4 messages
QBFT Consensus:               1 instance × 4 operators × ~3 rounds = ~12 messages
Post-Consensus:               1 batch message × 4 operators = 4 messages
────────────────────────────────────────────────────────────────────────
TOTAL: ~20 messages (80% reduction)
```

---

## Implementation Steps

### Phase 0: Upstream Lighthouse Changes

1. **Add watch channels to `DutiesService`** (upstream Lighthouse PR):
   - Add `attesters_poll_tx: watch::Sender<Slot>` field
   - Add `sync_poll_tx: watch::Sender<Slot>` field
   - Add `subscribe_to_attesters_poll()` method returning `watch::Receiver<Slot>`
   - Add `subscribe_to_sync_poll()` method returning `watch::Receiver<Slot>`
   - Fire `attesters_poll_tx.send(slot)` after `poll_beacon_attesters()` completes
   - Fire `sync_poll_tx.send(slot)` after `poll_sync_committee_duties()` completes

### Phase 1: Pre-Consensus Selection Proof Batching

2. **Add `SelectionProofMetadata` type** in `validator_store/src/lib.rs`:
   - `slot: Slot`
   - `attesting_validators: Vec<ValidatorIndex>`
   - `sync_validators_by_subnet: HashMap<ValidatorIndex, HashSet<SyncSubnetId>>`
   - `get_selection_proof_count()` method for batch size calculation

3. **Extend `MetadataService`** with watch channel coordination:
   - Subscribe to `duties_service.subscribe_to_attesters_poll()`
   - Subscribe to `duties_service.subscribe_to_sync_poll()`
   - At slot start: wait for both signals with timeout
   - On both signals received: call `prepare_selection_proof_metadata()`
   - At 1/3 slot: existing `update_metadata()` (unchanged)

4. **Add storage and retrieval methods** in `AnchorValidatorStore`:
   - `selection_proof_metadata: watch::Sender<Option<Arc<SelectionProofMetadata>>>`
   - `update_selection_proof_metadata()` - called by MetadataService
   - `get_selection_proof_metadata()` - waits on watch channel
   - `calculate_selection_proof_batch_count()` - uses SelectionProofMetadata
   - `compute_selection_proof_batch_hash()` - deterministic hash for batching

5. **Add new types** in `ssv_types`:
   - `PartialSignatureKind::AggregatorCommitteePartialSig`
   - `Role::AggregatorCommittee`

6. **Modify `produce_selection_proof`** to use `CollectionMode::Committee`:
   - Call `get_selection_proof_metadata()` to wait for metadata
   - Use `calculate_selection_proof_batch_count()` for batch size
   - Use `compute_selection_proof_batch_hash()` for base_hash

7. **Modify `produce_sync_selection_proof`** to use same batch:
   - Use same `base_hash` as attestation selection proofs
   - Use same `num_signatures_to_collect` count

### Phase 2: Post-Consensus Aggregation (Future)

8. **Extend `SlotMetadata`** with aggregator tracking

9. **Add consensus types** in `ssv_types`: `AggregatorConsensusData`

10. **Extend `CommitteeInstanceId`** with `duty_kind` to distinguish aggregator from attester

11. **Modify `produce_signed_aggregate_and_proof`** to use committee QBFT

12. **Modify `produce_signed_contribution_and_proof`** to use committee QBFT

---

## Open Questions

1. **Timeout handling**: What if not all validators' selection proof calls arrive before the batch timeout? Options:
   - Fail the batch and fall back to per-validator mode
   - Send partial batch (requires protocol change)
   - Extend timeout

2. **Partial committee coverage**: What if this operator only manages 2 of 4 validators in a committee? The count should only include validators this operator manages.

3. **Base hash computation**: The `base_hash` must be deterministic across all operators. Suggested: `hash(slot, committee_id, "aggregator_committee_pre_consensus")`

4. **Message validation**: How do receiving operators validate the combined partial signature message? Need to verify each partial sig individually.

5. **Sync committee "lag by 1" pattern**: Sync duties use `duty_slot = wall_clock_slot + 1` internally.
   - Need to ensure `SelectionProofMetadata` accounts for this when listing sync validators
   - The `proof_slot = slot - 1` adjustment in sync proofs compensates for this
