use std::{
    collections::{HashMap, HashSet, hash_map},
    future::Future,
    mem,
    pin::Pin,
    sync::Arc,
};

use bls::{PublicKeyBytes, SecretKey, Signature};
use bls_lagrange::KeyId;
use dashmap::{DashMap, Entry};
use database::{DatabaseError, NetworkDatabase, OwnOperatorId};
use fork::ForkSchedule;
use message_sender::MessageSender;
use processor::{Error, Error::Queue, Senders, work::DropOnFinish};
use slot_clock::SlotClock;
use ssv_types::typenum::Unsigned;
pub use ssv_types::{
    CommitteeId, OperatorId, ValidatorIndex,
    consensus::UnsignedSSVMessage,
    domain_type::DomainType,
    message::{MsgType, SSVMessage, SSVMessageError},
    msgid::{DutyExecutor, MessageId, Role},
    partial_sig::{
        PartialSignatureKind, PartialSignatureMessage, PartialSignatureMessages,
        PartialSignatureMessagesLen,
    },
};
use ssz::Encode;
use thiserror::Error;
use tokio::{
    sync::{
        mpsc,
        mpsc::{UnboundedSender, error::TrySendError},
        oneshot,
        oneshot::error::RecvError,
    },
    time::sleep,
};
use tracing::{Instrument, debug_span, error, trace, warn};
use types::{Hash256, Slot};

mod metrics;

const COLLECTOR_NAME: &str = "signature_collector";
const COLLECTOR_MESSAGE_NAME: &str = "signature_collector_message";
const COLLECTOR_CLEANER_NAME: &str = "signature_collector_cleaner";
const SIGNER_NAME: &str = "partial_signer";

/// number of slots to keep before the current slot
const SIGNATURE_COLLECTOR_RETAIN_SLOTS: u64 = 1;

/// Error type for creating partial signature messages.
#[derive(Debug, Error)]
enum CreateMessageError {
    #[error("Too many partial signatures: {count} exceeds maximum {max}")]
    TooManySignatures { count: usize, max: usize },
    #[error("Failed to create SSV message: {0}")]
    SSVMessage(#[from] SSVMessageError),
}

/// A handle to message the instance collecting a single specific signature
struct SignatureCollector {
    sender: UnboundedSender<CollectorMessage>,
    for_slot: Slot,
}

/// Locally accumulated validator partial signatures for one outgoing committee message.
/// As soon as this operator has produced the full validator batch for the committee round, the
/// message is sent.
struct CommitteePartialSignatureBatch {
    batched_validator_partial_signatures: Vec<PartialSignatureMessage>,
    for_slot: Slot,
}

pub struct SignatureCollectorManager<S: SlotClock> {
    /// The handle to the processor, for queueing messages to the instances.
    processor: Senders,
    /// The local operator we act for.
    operator_id: OwnOperatorId,
    /// The fork schedule for looking up the slot-based domain type.
    fork_schedule: Arc<ForkSchedule>,
    /// The slot clock for determining the current epoch.
    slot_clock: S,
    /// Number of slots per epoch (needed for epoch calculation).
    slots_per_epoch: u64,
    /// A message sender used for outgoing messages.
    message_sender: Arc<dyn MessageSender>,
    /// Database handle, shared with each spawned `signature_collector` task so
    /// it can look up per-operator share pubkeys for the per-share fallback
    /// verification path.
    database: Arc<NetworkDatabase>,
    /// A map from the signing root and signing validator to the corresponding signature collector.
    signature_collectors: DashMap<(Hash256, ValidatorIndex), SignatureCollector>,
    /// A map from the hash of a decided committee value and committee ID to the local batch of
    /// validator partial signatures for that committee round.
    /// Note that this hash may differ from the actual signing root.
    committee_partial_signature_batches:
        DashMap<(Hash256, CommitteeId), CommitteePartialSignatureBatch>,
}

impl<S: SlotClock + Clone + 'static> SignatureCollectorManager<S> {
    pub fn new(
        processor: Senders,
        operator_id: OwnOperatorId,
        fork_schedule: Arc<ForkSchedule>,
        slots_per_epoch: u64,
        message_sender: Arc<dyn MessageSender>,
        slot_clock: S,
        database: Arc<NetworkDatabase>,
    ) -> Result<Arc<Self>, CollectionError> {
        let manager = Arc::new(Self {
            processor,
            operator_id,
            fork_schedule,
            slot_clock: slot_clock.clone(),
            slots_per_epoch,
            message_sender,
            database,
            signature_collectors: DashMap::new(),
            committee_partial_signature_batches: DashMap::new(),
        });

        manager
            .processor
            .permitless
            .send_async(Arc::clone(&manager).cleaner(), COLLECTOR_CLEANER_NAME)?;

        Ok(manager)
    }

    /// Get the domain type for a message slot.
    fn domain_type_for_slot(&self, slot: Slot) -> DomainType {
        let epoch = slot.epoch(self.slots_per_epoch);
        self.fork_schedule.active_fork_config(epoch).domain_type
    }

    /// Sign a message and wait until the signature has been reconstructed.
    /// Will timeout if the instance is cleaned up, see [`SIGNATURE_COLLECTOR_RETAIN_SLOTS`].
    /// Check the fields of the parameter structs for more info.
    /// The rough idea behind the separation is that `metadata` will be the same across all calls if
    /// we sign for all validators in a committee, while `validator_signing_data` varies for each.
    pub async fn sign_and_collect(
        self: &Arc<Self>,
        metadata: SignatureMetadata,
        requester: SignatureRequester,
        validator_signing_data: ValidatorSigningData,
    ) -> Result<Arc<Signature>, CollectionError> {
        let Some(signer) = self.operator_id.get() else {
            return Err(CollectionError::OwnOperatorIdUnknown);
        };

        let (result_tx, result_rx) = oneshot::channel();

        trace!(
            ?metadata,
            ?requester,
            root=?validator_signing_data.root,
            index=?validator_signing_data.index,
            "sign_and_collect called",
        );

        // first, register notifier with preexisting or newly spawned instance
        let cloned_metadata = metadata.clone();
        let manager = self.clone();
        self.processor.permitless.send_immediate(
            move |drop_on_finish| {
                let sender = manager.get_or_spawn(
                    validator_signing_data.root,
                    validator_signing_data.index,
                    cloned_metadata.slot,
                );
                let _ = sender.send(CollectorMessage {
                    kind: CollectorMessageKind::RegisterNotifier {
                        notify: result_tx,
                        threshold: cloned_metadata.threshold,
                        validator_pubkey: validator_signing_data.validator_pubkey,
                    },
                    _drop_on_finish: drop_on_finish,
                });
            },
            COLLECTOR_MESSAGE_NAME,
        )?;

        // then, create the partial signature - and maybe send the message.
        let manager = self.clone();
        self.processor.urgent_consensus.send_blocking(
            move || {
                trace!(root = ?validator_signing_data.root, "Signing...");
                // If we have no share, we can not actually sign the message, because we are running
                // in impostor mode.
                let partial_signature = if let Some(share) = &validator_signing_data.share {
                    share.sign(validator_signing_data.root)
                } else {
                    Signature::empty()
                };
                trace!(root = ?validator_signing_data.root, "Signed");

                let message = PartialSignatureMessage {
                    partial_signature,
                    signing_root: validator_signing_data.root,
                    signer,
                    validator_index: validator_signing_data.index,
                };
                match requester {
                    SignatureRequester::SingleValidator { pubkey } => {
                        // we do not have to wait for other partial signatures - send the message
                        // immediately.
                        let msg = match manager.create_message(
                            &metadata,
                            vec![message.clone()],
                            &DutyExecutor::Validator(pubkey),
                        ) {
                            Ok(msg) => msg,
                            Err(err) => {
                                error!(%err, "Failed to create validator partial signature message");
                                return;
                            }
                        };

                        if let Err(err) =
                            manager
                                .message_sender
                                .sign_and_send(msg, metadata.committee_id, None)
                        {
                            error!(?err, "Failed to send validator partial signature");
                        }
                    }
                    SignatureRequester::Committee {
                        validator_partial_signature_batch_size,
                        base_hash,
                    } => {
                        // Batch one locally produced partial signature per validator before
                        // sending a single committee message for this round.
                        let mut entry = match manager
                            .committee_partial_signature_batches
                            .entry((base_hash, metadata.committee_id))
                        {
                            Entry::Occupied(occupied) => occupied,
                            Entry::Vacant(vacant) => vacant.insert_entry(CommitteePartialSignatureBatch {
                                batched_validator_partial_signatures: Vec::with_capacity(
                                    validator_partial_signature_batch_size,
                                ),
                                for_slot: metadata.slot,
                            }),
                        };
                        let validator_partial_signature_batch =
                            &mut entry.get_mut().batched_validator_partial_signatures;

                        // Add the partial signature we just produced for this validator to the
                        // local batch.
                        validator_partial_signature_batch.push(message.clone());

                        trace!(
                            have = validator_partial_signature_batch.len(),
                            need = validator_partial_signature_batch_size,
                            "Checking whether the batch of validator partial signatures is ready to send"
                        );

                        // Once the local batch of validator partial signatures is complete,
                        // create and send the committee message.
                        if validator_partial_signature_batch.len()
                            == validator_partial_signature_batch_size
                        {
                            let signatures =
                                entry.remove().batched_validator_partial_signatures;

                            let msg = match manager.create_message(
                                &metadata,
                                signatures,
                                &DutyExecutor::Committee(metadata.committee_id),
                            ) {
                                Ok(msg) => msg,
                                Err(err) => {
                                    error!(%err, "Failed to create committee partial signature message");
                                    return;
                                }
                            };

                            if let Err(err) =
                                manager
                                    .message_sender
                                    .sign_and_send(msg, metadata.committee_id, None)
                            {
                                error!(?err, "Failed to send committee partial signatures");
                            }
                        }
                    }
                }

                // Finally, make the local instance aware of the partial signature, if it is a real
                // signature.
                if validator_signing_data.share.is_some() {
                    let _ = manager.receive_partial_signature(message, metadata.slot);
                }
            },
            SIGNER_NAME,
        )?;

        // We resolve the collector future - if we are lucky, the signature is even already done
        // because we received enough shares before this fn was even called.
        Ok(result_rx.await?)
    }

    fn create_message(
        &self,
        metadata: &SignatureMetadata,
        signatures: Vec<PartialSignatureMessage>,
        duty_executor: &DutyExecutor,
    ) -> Result<UnsignedSSVMessage, CreateMessageError> {
        let domain = self.domain_type_for_slot(metadata.slot);
        let count = signatures.len();
        let messages = ssv_types::VariableList::new(signatures).map_err(|_| {
            CreateMessageError::TooManySignatures {
                count,
                max: PartialSignatureMessagesLen::USIZE,
            }
        })?;

        let partial_sig_messages = PartialSignatureMessages {
            kind: metadata.kind,
            slot: metadata.slot,
            messages,
        };

        Ok(UnsignedSSVMessage {
            ssv_message: SSVMessage::new(
                MsgType::SSVPartialSignatureMsgType,
                MessageId::new(&domain, metadata.role, duty_executor),
                partial_sig_messages.as_ssz_bytes(),
            )?,
            full_data: vec![],
        })
    }

    pub fn receive_partial_signatures(
        self: &Arc<Self>,
        messages: PartialSignatureMessages,
    ) -> Result<(), CollectionError> {
        for message in messages.messages {
            self.receive_partial_signature(message, messages.slot)?;
        }
        Ok(())
    }

    fn receive_partial_signature(
        self: &Arc<Self>,
        message: PartialSignatureMessage,
        slot: Slot,
    ) -> Result<(), CollectionError> {
        trace!(
            ?slot,
            signing_root=?message.signing_root,
            signer=?message.signer,
            validator=?message.validator_index,
            "Received partial signature message",
        );
        let manager = self.clone();
        self.processor.permitless.send_immediate(
            move |drop_on_finish| {
                let sender =
                    manager.get_or_spawn(message.signing_root, message.validator_index, slot);
                if let Err(err) = sender.send(CollectorMessage {
                    kind: CollectorMessageKind::PartialSignature {
                        operator_id: message.signer,
                        signature: Box::new(message.partial_signature),
                    },
                    _drop_on_finish: drop_on_finish,
                }) {
                    error!(
                        ?err,
                        "failed to send partial signature to collector instance"
                    );
                }
            },
            COLLECTOR_MESSAGE_NAME,
        )?;
        Ok(())
    }

    fn get_or_spawn(
        &self,
        signing_root: Hash256,
        validator_index: ValidatorIndex,
        slot: Slot,
    ) -> UnboundedSender<CollectorMessage> {
        match self
            .signature_collectors
            .entry((signing_root, validator_index))
        {
            Entry::Occupied(entry) => entry.get().sender.clone(),
            Entry::Vacant(entry) => {
                // this channel is effectively limited by the processor permit amount
                let (tx, rx) = mpsc::unbounded_channel();
                let span = debug_span!(
                    "signature_collector",
                    ?slot,
                    ?validator_index,
                    ?signing_root
                );
                entry.insert(SignatureCollector {
                    sender: tx.clone(),
                    for_slot: slot,
                });
                let database = Arc::clone(&self.database);
                let _ = self.processor.permitless.send_async(
                    Box::pin(signature_collector(rx, signing_root, database).instrument(span)),
                    COLLECTOR_NAME,
                );
                trace!(
                    ?signing_root,
                    ?validator_index,
                    "Spawned signature collector"
                );
                tx
            }
        }
    }

    async fn cleaner(self: Arc<Self>) {
        let slot_clock = &self.slot_clock;
        while !self.processor.permitless.is_closed() {
            sleep(
                slot_clock
                    .duration_to_next_slot()
                    .unwrap_or(slot_clock.slot_duration()),
            )
            .await;
            let Some(slot) = slot_clock.now() else {
                continue;
            };
            let cutoff = slot.saturating_sub(SIGNATURE_COLLECTOR_RETAIN_SLOTS);
            self.signature_collectors
                .retain(|_, collector| collector.for_slot >= cutoff);
            self.committee_partial_signature_batches
                .retain(|_, batch| batch.for_slot >= cutoff);
        }
    }
}

/// Metadata around the signature(s) to create.
#[derive(Debug, Clone)]
pub struct SignatureMetadata {
    /// The signature kind to transmit. Only needed for the network message we send.
    pub kind: PartialSignatureKind,
    /// The role to transmit. Only needed for the network message we send.
    pub role: Role,
    /// The threshold of operator shares used by the per-validator reconstruction collector.
    /// Once partial signatures from this many operators have arrived over the network, the full
    /// validator signature can be reconstructed.
    ///
    /// This is distinct from
    /// `SignatureRequester::Committee::validator_partial_signature_batch_size`, which only
    /// controls how many validator partial signatures this operator batches locally before sending
    /// its own committee message.
    pub threshold: u64,
    /// The slot relevant for this signature. The collector instance is cleaned up one slot after
    /// this. Also used in the network message.
    pub slot: Slot,
    /// The committee of the signer(s). Used in the created network message.
    pub committee_id: CommitteeId,
}

/// Describes whether this request signs for a single validator or for multiple validators in a
/// committee. This matters because the committee case sends one message for the whole batch.
#[derive(Debug, Clone)]
pub enum SignatureRequester {
    /// The only validator signing this is the one passed when `sign_and_collect` is called.
    SingleValidator {
        /// The public key of the validator. Used in the created network message.
        pubkey: PublicKeyBytes,
    },
    /// The local operator is signing for multiple validators in one committee round.
    /// We batch those validator partial signatures into a single outgoing committee message instead
    /// of sending one message per validator.
    Committee {
        /// How many validator partial signatures this operator must produce locally before sending
        /// the batched committee message.
        ///
        /// This is a local batching count, not the network reconstruction threshold from
        /// `SignatureMetadata::threshold`.
        validator_partial_signature_batch_size: usize,
        /// Identifies which partial signatures belong in the same outgoing committee message.
        /// We cannot use the signing root because the batched signatures may have different
        /// signing roots.
        base_hash: Hash256,
    },
}

#[derive(Clone)]
pub struct ValidatorSigningData {
    pub root: Hash256,
    pub index: ValidatorIndex,
    pub share: Option<SecretKey>,
    pub validator_pubkey: PublicKeyBytes,
}

struct CollectorMessage {
    kind: CollectorMessageKind,
    _drop_on_finish: DropOnFinish,
}

#[derive(Debug)]
enum CollectorMessageKind {
    /// A new task is waiting for the result of this collector instance.
    RegisterNotifier {
        notify: oneshot::Sender<Arc<Signature>>,
        threshold: u64,
        validator_pubkey: PublicKeyBytes,
    },
    /// A new partial signature is available - either because it arrived from the network, or
    /// because we created it
    PartialSignature {
        /// The signer.
        operator_id: OperatorId,
        /// The signature, boxed because else Clippy complains.
        signature: Box<Signature>,
    },
}

#[derive(Debug, Clone)]
pub enum CollectionError {
    QueueClosedError,
    QueueFullError,
    CollectionTimeout,
    EmptySignature,
    OwnOperatorIdUnknown,
    RecoverError(bls_lagrange::Error),
}

impl From<Error> for CollectionError {
    fn from(value: Error) -> Self {
        match value {
            Queue(TrySendError::Full(_)) => CollectionError::QueueFullError,
            Queue(TrySendError::Closed(_)) => CollectionError::QueueClosedError,
        }
    }
}

impl From<RecvError> for CollectionError {
    fn from(_: RecvError) -> Self {
        CollectionError::QueueClosedError
    }
}

impl From<bls_lagrange::Error> for CollectionError {
    fn from(err: bls_lagrange::Error) -> Self {
        CollectionError::RecoverError(err)
    }
}

/// Trait abstracting signature collection for testability.
///
/// Production code uses `Arc<SignatureCollectorManager<S>>` which implements this trait.
/// Tests can provide a mock that returns canned signatures or errors.
pub trait SignatureCollecting: Send + Sync {
    fn sign_and_collect(
        &self,
        metadata: SignatureMetadata,
        requester: SignatureRequester,
        signing_data: ValidatorSigningData,
    ) -> Pin<Box<dyn Future<Output = Result<Arc<Signature>, CollectionError>> + Send + '_>>;
}

impl<S: SlotClock + Clone + 'static> SignatureCollecting for Arc<SignatureCollectorManager<S>> {
    fn sign_and_collect(
        &self,
        metadata: SignatureMetadata,
        requester: SignatureRequester,
        signing_data: ValidatorSigningData,
    ) -> Pin<Box<dyn Future<Output = Result<Arc<Signature>, CollectionError>> + Send + '_>> {
        Box::pin(SignatureCollectorManager::sign_and_collect(
            self,
            metadata,
            requester,
            signing_data,
        ))
    }
}

/// The actual signature collector task, waiting for messages.
///
/// The recv loop is the only place that matches on [`CollectorMessageKind`]
/// and the only place that performs async I/O (DB lookups). [`SignatureCollectorState`]
/// is a pure synchronous state machine that signals I/O needs via [`CollectorAction`].
async fn signature_collector(
    mut rx: mpsc::UnboundedReceiver<CollectorMessage>,
    signing_root: Hash256,
    database: Arc<NetworkDatabase>,
) {
    let mut state = SignatureCollectorState::new(signing_root);
    while let Some(message) = rx.recv().await {
        trace!(msg=?message.kind, "Signature collector received message");
        let mut outcome = match message.kind {
            CollectorMessageKind::RegisterNotifier {
                notify,
                threshold,
                validator_pubkey,
            } => state.register_request(notify, threshold, validator_pubkey),
            CollectorMessageKind::PartialSignature {
                operator_id,
                signature,
            } => state.add_partial_signature(operator_id, *signature),
        };
        loop {
            match outcome {
                CollectorAction::Recv => break,
                CollectorAction::Exit => return,
                CollectorAction::FetchSharePubkeys(validator_pubkey) => {
                    metrics::inc_counter(&metrics::SIGNATURE_VERIFICATION_FAILURES_TOTAL);
                    warn!(
                        ?signing_root,
                        "Reconstructed signature failed master-key verification; running fallback"
                    );
                    match fetch_share_pubkeys(&database, &validator_pubkey).await {
                        Ok(share_pubkeys) => outcome = state.apply_fallback(share_pubkeys),
                        Err(err) => {
                            error!(?err, "Failed to look up share pubkeys for fallback");
                            return;
                        }
                    }
                }
            }
        }
    }
}

/// Outcome of a single synchronous step against [`SignatureCollectorState`].
/// `FetchSharePubkeys` is an effect request: the recv loop must perform the
/// DB lookup and feed the result back via [`SignatureCollectorState::apply_fallback`].
enum CollectorAction {
    Recv,
    Exit,
    FetchSharePubkeys(PublicKeyBytes),
}

/// Threshold + validator pubkey are recorded together on first `RegisterNotifier`
/// and stay coupled thereafter. A single `Option<Registration>` makes the
/// "registered yet?" check one decision instead of two.
struct Registration {
    threshold: u64,
    validator_pubkey: PublicKeyBytes,
}

/// Invariant: once `full_signature` is `Some`, both `signature_share` and
/// `notifiers` are empty (drained by `try_reconstruct`).
struct SignatureCollectorState {
    notifiers: Vec<oneshot::Sender<Arc<Signature>>>,
    signature_share: HashMap<OperatorId, Signature>,
    full_signature: Option<Arc<Signature>>,
    registration: Option<Registration>,
    signing_root: Hash256,
}

impl SignatureCollectorState {
    fn new(signing_root: Hash256) -> Self {
        Self {
            notifiers: Vec::new(),
            signature_share: HashMap::new(),
            full_signature: None,
            registration: None,
            signing_root,
        }
    }

    /// Register a task waiting for the reconstructed signature.
    ///
    /// If reconstruction has already completed, the cached signature is
    /// delivered immediately. Otherwise the notifier is queued and the
    /// registration recorded. Conflicting thresholds or validator pubkeys
    /// from concurrent registrations return `Exit`.
    fn register_request(
        &mut self,
        notify: oneshot::Sender<Arc<Signature>>,
        threshold: u64,
        validator_pubkey: PublicKeyBytes,
    ) -> CollectorAction {
        if let Some(full_signature) = &self.full_signature {
            if let Err(err) = notify.send(Arc::clone(full_signature)) {
                warn!(?err, "Failed to send recovered signature");
            }
            return CollectorAction::Recv;
        }
        self.notifiers.push(notify);
        if let Some(existing) = &self.registration {
            if existing.threshold != threshold {
                error!(
                    new_threshold = threshold,
                    old_threshold = existing.threshold,
                    "Conflicting thresholds passed!"
                );
                return CollectorAction::Exit;
            }
            if existing.validator_pubkey != validator_pubkey {
                error!(
                    new = ?validator_pubkey,
                    old = ?existing.validator_pubkey,
                    "Conflicting validator pubkeys passed!"
                );
                return CollectorAction::Exit;
            }
        } else {
            self.registration = Some(Registration {
                threshold,
                validator_pubkey,
            });
        }
        self.try_reconstruct()
    }

    /// Ingest a partial signature from one operator.
    ///
    /// Late shares arriving after reconstruction are silently dropped.
    /// Conflicting shares from the same operator are logged but not fatal,
    /// since the source of the discrepancy is not knowable here.
    fn add_partial_signature(
        &mut self,
        operator_id: OperatorId,
        signature: Signature,
    ) -> CollectorAction {
        if self.full_signature.is_some() {
            return CollectorAction::Recv;
        }

        match self.signature_share.entry(operator_id) {
            hash_map::Entry::Vacant(entry) => {
                entry.insert(signature);
            }
            hash_map::Entry::Occupied(entry) => {
                if entry.get() != &signature {
                    // We can not know which signature is correct. This is serious
                    // misbehaviour from the operator!
                    error!(
                        ?operator_id,
                        "Received conflicting signatures from operator"
                    );
                }
            }
        }

        self.try_reconstruct()
    }

    /// Drive reconstruction: combine if quorum is met, verify against the
    /// validator master pubkey, and surface `FetchSharePubkeys` on verification
    /// failure so the recv loop can drive the fallback.
    fn try_reconstruct(&mut self) -> CollectorAction {
        let Some(registration) = &self.registration else {
            return CollectorAction::Recv;
        };
        if (self.signature_share.len() as u64) < registration.threshold {
            return CollectorAction::Recv;
        }
        let validator_pubkey = registration.validator_pubkey;

        match try_combine_and_verify(&self.signature_share, &validator_pubkey, self.signing_root) {
            CombineOutcome::Success(signature) => {
                trace!(?signature, "Successfully recovered signature");
                self.signature_share.clear();
                for notifier in mem::take(&mut self.notifiers) {
                    if notifier.send(Arc::clone(&signature)).is_err() {
                        warn!("Callback dropped - signature is no longer relevant");
                    }
                }
                self.full_signature = Some(signature);
                CollectorAction::Recv
            }
            CombineOutcome::CombineFailed(err) => {
                error!(?err, "Failed to recover signature");
                CollectorAction::Exit
            }
            CombineOutcome::VerificationFailed => {
                CollectorAction::FetchSharePubkeys(validator_pubkey)
            }
        }
    }

    /// Per-share fallback: identify shares that fail individual verification
    /// against `share_pubkeys`, evict them, and re-attempt reconstruction.
    /// `Exit` when verification failed but every share is individually valid
    /// (no operator to attribute the failure to: e.g. mismatched validator
    /// pubkey, corrupted DB state).
    fn apply_fallback(
        &mut self,
        share_pubkeys: HashMap<OperatorId, PublicKeyBytes>,
    ) -> CollectorAction {
        let invalid_operators =
            find_invalid_shares(&self.signature_share, self.signing_root, &share_pubkeys);

        if invalid_operators.is_empty() {
            let operators: Vec<_> = self.signature_share.keys().copied().collect();
            error!(
                signing_root = ?self.signing_root,
                ?operators,
                "Verification failed but every individual share is valid; aborting"
            );
            return CollectorAction::Exit;
        }

        warn!(?invalid_operators, "Evicting invalid shares");
        for op in &invalid_operators {
            self.signature_share.remove(op);
        }
        self.try_reconstruct()
    }
}

/// Outcome of a single combine-and-verify attempt against the validator master pubkey.
enum CombineOutcome {
    Success(Arc<Signature>),
    /// Lagrange interpolation failed. Unrecoverable for this collector instance.
    CombineFailed(CollectionError),
    /// Combined signature did not verify against the master pubkey; caller
    /// should run the per-share fallback to evict bad operator shares.
    VerificationFailed,
}

fn try_combine_and_verify(
    shares: &HashMap<OperatorId, Signature>,
    validator_pubkey: &PublicKeyBytes,
    signing_root: Hash256,
) -> CombineOutcome {
    let combined = match combine_signatures(shares) {
        Ok(sig) => sig,
        Err(err) => return CombineOutcome::CombineFailed(err),
    };

    if verify_compressed(&combined, validator_pubkey, signing_root) {
        CombineOutcome::Success(Arc::new(combined))
    } else {
        CombineOutcome::VerificationFailed
    }
}

/// Verify a BLS signature against a compressed pubkey. Returns `false` on
/// decompression failure (and logs).
fn verify_compressed(
    signature: &Signature,
    pubkey: &PublicKeyBytes,
    signing_root: Hash256,
) -> bool {
    match pubkey.decompress() {
        Ok(pk) => signature.verify(&pk, signing_root),
        Err(err) => {
            error!(
                ?pubkey,
                ?err,
                "Failed to decompress pubkey for verification"
            );
            false
        }
    }
}

fn find_invalid_shares(
    shares: &HashMap<OperatorId, Signature>,
    signing_root: Hash256,
    share_pubkeys: &HashMap<OperatorId, PublicKeyBytes>,
) -> HashSet<OperatorId> {
    let mut invalid = HashSet::new();
    for (operator_id, signature) in shares {
        match share_pubkeys.get(operator_id) {
            Some(pubkey) => {
                if !verify_compressed(signature, pubkey, signing_root) {
                    invalid.insert(*operator_id);
                }
            }
            None => {
                // Unverifiable share: evict so reconstruction can retry without it.
                warn!(
                    %operator_id,
                    "No share pubkey found in database; marking share as invalid"
                );
                invalid.insert(*operator_id);
            }
        }
    }
    invalid
}

/// Look up the per-operator share pubkey map for a validator via the database.
/// Wraps the blocking SQLite call in `spawn_blocking` so it doesn't stall the
/// async runtime.
async fn fetch_share_pubkeys(
    database: &Arc<NetworkDatabase>,
    validator_pubkey: &PublicKeyBytes,
) -> Result<HashMap<OperatorId, PublicKeyBytes>, DatabaseError> {
    let database = Arc::clone(database);
    let validator_pubkey = *validator_pubkey;
    tokio::task::spawn_blocking(move || database.get_share_pubkeys_for_validator(&validator_pubkey))
        .await
        .map_err(|err| {
            // `JoinError` mislabeled as `SQLError` pending a richer variant.
            error!(
                ?err,
                "spawn_blocking task panicked while fetching share pubkeys"
            );
            DatabaseError::SQLError(err.to_string())
        })?
}

fn combine_signatures(
    shares: &HashMap<OperatorId, Signature>,
) -> Result<Signature, CollectionError> {
    let (ids, signatures): (Vec<_>, Vec<_>) = shares
        .iter()
        .map(|(k, s)| KeyId::try_from(**k).map(|k| (k, s.clone())))
        .collect::<Result<_, _>>()?;

    Ok(bls_lagrange::combine_signatures(&signatures, &ids)?)
}

#[cfg(test)]
mod tests;
