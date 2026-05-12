use bls::{SecretKey, Signature};
use bls_lagrange::{KeyId, split_with_rng};
use database::{
    PendingStateUpdates,
    test_utils::{InMemoryTestFixture, generators},
};
use rand::{prelude::*, rngs::StdRng};
use ssv_types::{Share, ValidatorIndex, ValidatorMetadata};
use tokio::sync::{Mutex, oneshot};
use types::Graffiti;

use super::*;

const TEST_RNG_SEED: u64 = 0xDEAD_BEEF_CAFE_0001;
const TOTAL_SHARES: u64 = 4;
const THRESHOLD: u64 = 3;
const SIGNING_ROOT: Hash256 = Hash256::repeat_byte(0xAB);

/// Master pubkey + per-operator share keys produced by a single split.
struct TestQuorum {
    master_pubkey: PublicKeyBytes,
    shares: Vec<(OperatorId, SecretKey)>,
}

fn split_random_master() -> TestQuorum {
    let rng = &mut StdRng::seed_from_u64(TEST_RNG_SEED);
    let master = SecretKey::random();
    let master_pubkey = master.public_key().compress();

    let shares = split_with_rng(
        &master,
        THRESHOLD,
        (1..=TOTAL_SHARES).map(|x| KeyId::try_from(x).unwrap()),
        rng,
    )
    .expect("split should succeed")
    .into_iter()
    .map(|(kid, sk)| (OperatorId(u64::from(kid)), sk))
    .collect();

    TestQuorum {
        master_pubkey,
        shares,
    }
}

fn fresh_state() -> SignatureCollectorState {
    SignatureCollectorState::new(SIGNING_ROOT)
}

fn register_notifier(
    state: &mut SignatureCollectorState,
    threshold: u64,
    validator_pubkey: PublicKeyBytes,
) -> oneshot::Receiver<Arc<Signature>> {
    let (notify, rx) = oneshot::channel();
    let outcome = state.register_request(notify, threshold, validator_pubkey);
    assert!(
        matches!(outcome, CollectorAction::Recv),
        "register_notifier should not break the collector"
    );
    rx
}

fn feed_partial_sig(
    state: &mut SignatureCollectorState,
    operator_id: OperatorId,
    signature: Signature,
) {
    let outcome = state.add_partial_signature(operator_id, signature);
    assert!(
        matches!(outcome, CollectorAction::Recv),
        "feed_partial_sig should not break the collector"
    );
}

fn expect_signature(rx: &mut oneshot::Receiver<Arc<Signature>>, context: &str) {
    rx.try_recv().expect(context);
}

/// Once `THRESHOLD` valid partial signatures have been fed in, the state
/// reconstructs the master signature and delivers it to a registered notifier.
#[test]
fn state_processes_full_quorum_and_notifies() {
    let TestQuorum {
        master_pubkey,
        shares,
    } = split_random_master();
    let mut state = fresh_state();

    let mut result_rx = register_notifier(&mut state, THRESHOLD, master_pubkey);

    for (op_id, sk) in &shares[..THRESHOLD as usize] {
        feed_partial_sig(&mut state, *op_id, sk.sign(SIGNING_ROOT));
    }

    expect_signature(
        &mut result_rx,
        "Notifier should receive the reconstructed signature",
    );
}

/// Two `RegisterNotifier` messages disagree on the threshold; the state must
/// `Break`. In production, the recv loop drops the state on `Break`, which
/// drops every queued notifier and surfaces `RecvError` to the callers. The
/// test models that lifetime explicitly via `drop(state)`.
#[test]
fn state_breaks_on_conflicting_thresholds() {
    let TestQuorum { master_pubkey, .. } = split_random_master();
    let mut state = fresh_state();
    let _first_rx = register_notifier(&mut state, THRESHOLD, master_pubkey);

    let (second_notify, mut second_rx) = oneshot::channel();
    let outcome = state.register_request(second_notify, THRESHOLD + 1, master_pubkey);

    assert!(
        matches!(outcome, CollectorAction::Exit),
        "State should Break when a second notifier disagrees on threshold"
    );

    drop(state);

    assert!(
        matches!(
            second_rx.try_recv(),
            Err(oneshot::error::TryRecvError::Closed)
        ),
        "Conflicting notifier sender should be dropped, surfacing RecvError"
    );
}

/// A notifier that registers after reconstruction has already completed should
/// receive the cached signature immediately rather than waiting on a fresh
/// quorum.
#[test]
fn state_delivers_cached_signature_to_late_registrant() {
    let TestQuorum {
        master_pubkey,
        shares,
    } = split_random_master();
    let mut state = fresh_state();

    // Reach quorum on the first registrant.
    let mut first_rx = register_notifier(&mut state, THRESHOLD, master_pubkey);
    for (op_id, sk) in &shares[..THRESHOLD as usize] {
        feed_partial_sig(&mut state, *op_id, sk.sign(SIGNING_ROOT));
    }
    expect_signature(
        &mut first_rx,
        "First registrant should receive the reconstructed signature",
    );

    // Late registrant arrives after `full_signature` is set.
    let mut late_rx = register_notifier(&mut state, THRESHOLD, master_pubkey);
    expect_signature(
        &mut late_rx,
        "Late registrant should receive the cached signature immediately",
    );
}

/// Multiple notifiers registered before quorum should all be notified on a
/// single reconstruction (the `Vec<oneshot::Sender>` fan-out).
#[test]
fn state_notifies_all_registrants_on_reconstruction() {
    let TestQuorum {
        master_pubkey,
        shares,
    } = split_random_master();
    let mut state = fresh_state();

    let mut rx_a = register_notifier(&mut state, THRESHOLD, master_pubkey);
    let mut rx_b = register_notifier(&mut state, THRESHOLD, master_pubkey);
    let mut rx_c = register_notifier(&mut state, THRESHOLD, master_pubkey);

    for (op_id, sk) in &shares[..THRESHOLD as usize] {
        feed_partial_sig(&mut state, *op_id, sk.sign(SIGNING_ROOT));
    }

    expect_signature(&mut rx_a, "registrant a should be notified");
    expect_signature(&mut rx_b, "registrant b should be notified");
    expect_signature(&mut rx_c, "registrant c should be notified");
}

/// Partial signatures that arrive before any `RegisterNotifier` is buffered.
/// Reconstruction triggers when a notifier and the threshold arrives.
#[test]
fn state_buffers_shares_arriving_before_first_notifier_register() {
    let TestQuorum {
        master_pubkey,
        shares,
    } = split_random_master();
    let mut state = fresh_state();

    for (op_id, sk) in &shares[..THRESHOLD as usize] {
        feed_partial_sig(&mut state, *op_id, sk.sign(SIGNING_ROOT));
    }

    assert!(
        state.full_signature.is_none(),
        "State should not reconstruct without a threshold"
    );

    let mut result_rx = register_notifier(&mut state, THRESHOLD, master_pubkey);
    expect_signature(
        &mut result_rx,
        "Notifier should receive the signature reconstructed from buffered shares",
    );
}

/// Signing root used to produce shares that verify under their own share
/// pubkey but fail master-verify.
const BAD_SIGNING_ROOT: Hash256 = Hash256::repeat_byte(0xCC);

/// Build the operator -> share-pubkey map for direct calls to
/// `find_invalid_shares` (skipping the DB).
fn share_pubkey_map(quorum: &TestQuorum) -> HashMap<OperatorId, PublicKeyBytes> {
    quorum
        .shares
        .iter()
        .map(|(op_id, sk)| (*op_id, sk.public_key().compress()))
        .collect()
}

/// Seed an in-memory `NetworkDatabase` with operators + cluster + validator +
/// shares so a lookup keyed by `master_pubkey` returns each operator's real
/// BLS share pubkey.
fn seed_share_pubkeys(
    master_pubkey: PublicKeyBytes,
    shares: &[(OperatorId, SecretKey)],
) -> Arc<NetworkDatabase> {
    let fixture = InMemoryTestFixture::new_empty();
    let db = fixture.data.db;

    let operators: Vec<_> = shares
        .iter()
        .map(|(op_id, _)| generators::operator::with_id(op_id.0))
        .collect();

    let cluster = generators::cluster::with_operators(&operators);
    let validator = ValidatorMetadata {
        public_key: master_pubkey,
        cluster_id: cluster.cluster_id,
        index: Some(ValidatorIndex(rand::rng().random_range(0..1000))),
        graffiti: Graffiti::default(),
    };
    let db_shares: Vec<Share> = shares
        .iter()
        .map(|(op_id, sk)| Share {
            validator_pubkey: master_pubkey,
            operator_id: *op_id,
            cluster_id: cluster.cluster_id,
            share_pubkey: sk.public_key().compress(),
            encrypted_private_key: [0u8; ssv_types::ENCRYPTED_KEY_LENGTH],
        })
        .collect();

    let mut conn = db.connection().expect("test db connection");
    let tx = conn.transaction().expect("test db transaction");
    let mut pending = PendingStateUpdates::default();
    for op in &operators {
        db.insert_operator_tx(op, &tx, &mut pending)
            .expect("insert operator");
    }
    db.insert_validator_tx(cluster, &validator, db_shares, &tx, &mut pending)
        .expect("insert validator");
    tx.commit().expect("commit");
    db.publish_pending_state_updates(pending);

    Arc::new(db)
}

/// Pair a fresh state with a DB seeded for the given quorum. State no longer
/// holds the DB; integration tests pass it explicitly into `drive`.
fn seeded_state_and_db(quorum: &TestQuorum) -> (SignatureCollectorState, Arc<NetworkDatabase>) {
    (
        fresh_state(),
        seed_share_pubkeys(quorum.master_pubkey, &quorum.shares),
    )
}

/// Drive a `CollectorAction` to completion. Mirrors the recv loop in
/// `signature_collector`: when the state requests share pubkeys, perform the
/// DB lookup and feed the result back via `apply_fallback`. The counter
/// increment matches the production recv-loop seam so metric assertions hold.
async fn drive(
    state: &mut SignatureCollectorState,
    database: &Arc<NetworkDatabase>,
    initial: CollectorAction,
) -> CollectorAction {
    let mut outcome = initial;
    loop {
        match outcome {
            CollectorAction::Recv | CollectorAction::Exit => return outcome,
            CollectorAction::FetchSharePubkeys(pk) => {
                metrics::inc_counter(&metrics::SIGNATURE_VERIFICATION_FAILURES_TOTAL);
                let share_pubkeys = fetch_share_pubkeys(database, &pk)
                    .await
                    .expect("test fetch_share_pubkeys");
                outcome = state.apply_fallback(share_pubkeys);
            }
        }
    }
}

/// Feed a partial signature then drive any resulting fallback to completion.
async fn feed_and_drive(
    state: &mut SignatureCollectorState,
    database: &Arc<NetworkDatabase>,
    operator_id: OperatorId,
    signature: Signature,
) -> CollectorAction {
    let outcome = state.add_partial_signature(operator_id, signature);
    drive(state, database, outcome).await
}

/// Snapshot the global verification-failures counter; tests assert relative
/// increments.
fn verification_failures_count() -> u64 {
    metrics::SIGNATURE_VERIFICATION_FAILURES_TOTAL
        .as_ref()
        .expect("counter registered")
        .get()
}

/// Serializes tests that read the global counter so deltas aren't poisoned by
/// parallelism.
static METRICS_TEST_LOCK: Mutex<()> = Mutex::const_new(());

// ==================== `find_invalid_shares` unit tests ====================

/// One bad share among an otherwise clean quorum should be the only operator
/// flagged for eviction.
#[test]
fn find_invalid_shares_identifies_bad() {
    // Arrange: three operators sign `SIGNING_ROOT`; one signs `BAD_SIGNING_ROOT`.
    let quorum = split_random_master();
    let share_pubkeys = share_pubkey_map(&quorum);
    let (bad_op, bad_sk) = &quorum.shares[0];
    let shares: HashMap<OperatorId, Signature> =
        std::iter::once((*bad_op, bad_sk.sign(BAD_SIGNING_ROOT)))
            .chain(
                quorum.shares[1..]
                    .iter()
                    .map(|(op, sk)| (*op, sk.sign(SIGNING_ROOT))),
            )
            .collect();

    let invalid = find_invalid_shares(&shares, SIGNING_ROOT, &share_pubkeys);

    assert_eq!(invalid, HashSet::from([*bad_op]));
}

/// A clean quorum has no invalid shares.
#[test]
fn find_invalid_shares_all_valid() {
    let quorum = split_random_master();
    let share_pubkeys = share_pubkey_map(&quorum);
    let shares: HashMap<OperatorId, Signature> = quorum
        .shares
        .iter()
        .map(|(op, sk)| (*op, sk.sign(SIGNING_ROOT)))
        .collect();

    let invalid = find_invalid_shares(&shares, SIGNING_ROOT, &share_pubkeys);

    assert!(invalid.is_empty(), "no shares should be invalid");
}

/// When a share pubkey is missing from the map, the share is unverifiable and
/// must be marked invalid for eviction even if the signature itself is good.
#[test]
fn find_invalid_shares_missing_pubkey_marked_invalid() {
    // Drop the first operator's entry from `share_pubkeys` to simulate an
    // unverifiable share.
    let quorum = split_random_master();
    let mut share_pubkeys = share_pubkey_map(&quorum);
    let (missing_op, _) = &quorum.shares[0];
    share_pubkeys.remove(missing_op);
    let shares: HashMap<OperatorId, Signature> = quorum
        .shares
        .iter()
        .map(|(op, sk)| (*op, sk.sign(SIGNING_ROOT)))
        .collect();

    let invalid = find_invalid_shares(&shares, SIGNING_ROOT, &share_pubkeys);

    assert_eq!(
        invalid,
        HashSet::from([*missing_op]),
        "operator without a known share pubkey must be flagged invalid"
    );
}

// ==================== `try_combine_and_verify` unit tests ====================

/// A clean quorum's combined signature verifies against the validator master
/// pubkey.
#[test]
fn try_combine_valid_quorum_succeeds() {
    let quorum = split_random_master();
    let shares: HashMap<OperatorId, Signature> = quorum.shares[..THRESHOLD as usize]
        .iter()
        .map(|(op, sk)| (*op, sk.sign(SIGNING_ROOT)))
        .collect();

    let outcome = try_combine_and_verify(&shares, &quorum.master_pubkey, SIGNING_ROOT);

    assert!(
        matches!(outcome, CombineOutcome::Success(_)),
        "clean quorum should reconstruct and verify"
    );
}

/// One bad share in the quorum produces a combined signature that does not
/// verify against the master pubkey: the outcome is `VerificationFailed` (not
/// a structural `CombineFailed`).
#[test]
fn try_combine_poisoned_quorum_fails_verify() {
    // One share signs the wrong root; the other two are clean.
    let quorum = split_random_master();
    let mut signed: Vec<(OperatorId, Signature)> = quorum.shares[..THRESHOLD as usize]
        .iter()
        .map(|(op, sk)| (*op, sk.sign(SIGNING_ROOT)))
        .collect();
    let (bad_op, bad_sk) = &quorum.shares[0];
    signed[0] = (*bad_op, bad_sk.sign(BAD_SIGNING_ROOT));
    let shares: HashMap<OperatorId, Signature> = signed.into_iter().collect();

    let outcome = try_combine_and_verify(&shares, &quorum.master_pubkey, SIGNING_ROOT);

    assert!(
        matches!(outcome, CombineOutcome::VerificationFailed),
        "poisoned quorum should reach verification, then fail it"
    );
}

// ==================== `try_reconstruct` integration tests ====================

/// End-to-end fallback path: a quorum containing one bad share fails master
/// verify, the fallback identifies and evicts the offending operator, and a
/// later valid share completes a clean quorum. The verification-failures
/// counter records the one failed reconstruction.
#[tokio::test]
async fn integration_fallback_removes_bad_and_succeeds() {
    let _metrics_guard = METRICS_TEST_LOCK.lock().await;
    let quorum = split_random_master();
    let (mut state, database) = seeded_state_and_db(&quorum);
    let mut rx = register_notifier(&mut state, THRESHOLD, quorum.master_pubkey);
    let failures_before = verification_failures_count();

    let (op0, sk0) = &quorum.shares[0];
    let (op1, sk1) = &quorum.shares[1];
    let (bad_op, bad_sk) = &quorum.shares[2];
    let (op3, sk3) = &quorum.shares[3];
    feed_and_drive(&mut state, &database, *op0, sk0.sign(SIGNING_ROOT)).await;
    feed_and_drive(&mut state, &database, *op1, sk1.sign(SIGNING_ROOT)).await;
    feed_and_drive(
        &mut state,
        &database,
        *bad_op,
        bad_sk.sign(BAD_SIGNING_ROOT),
    )
    .await;
    // After the bad share triggers eviction, only 2 valid shares remain, so
    // no signature has been produced yet.
    assert!(
        state.full_signature.is_none(),
        "fallback should have evicted the bad share without producing a signature"
    );
    feed_and_drive(&mut state, &database, *op3, sk3.sign(SIGNING_ROOT)).await;

    expect_signature(
        &mut rx,
        "Clean quorum after eviction should reconstruct successfully",
    );
    assert_eq!(
        verification_failures_count(),
        failures_before + 1,
        "exactly one verification failure should have been recorded"
    );
}

/// Verification fails against the master pubkey, but every individual share
/// verifies against its own share pubkey. The collector cannot attribute the
/// failure to any operator and must safety-abort. We model that by passing a
/// validator pubkey that doesn't correspond to the actual master used to
/// split the shares: the DB lookup keyed by that pubkey returns the correct
/// share-pubkey set, every share verifies individually, but the combined
/// signature won't verify against the unrelated `wrong_master_pubkey`.
#[tokio::test]
async fn integration_aborts_when_no_individual_share_invalid() {
    let _metrics_guard = METRICS_TEST_LOCK.lock().await;
    let quorum = split_random_master();
    let wrong_master_pubkey = SecretKey::random().public_key().compress();
    // Seed the DB keyed by the WRONG pubkey so the lookup returns the real
    // share pubkeys (every share verifies individually), but the combined
    // signature won't verify against `wrong_master_pubkey`.
    let database = seed_share_pubkeys(wrong_master_pubkey, &quorum.shares);
    let mut state = SignatureCollectorState::new(SIGNING_ROOT);
    let (notify, mut rx) = oneshot::channel();
    let register_outcome = state.register_request(notify, THRESHOLD, wrong_master_pubkey);
    assert!(
        matches!(register_outcome, CollectorAction::Recv),
        "registration before any shares should not break"
    );

    let (op0, sk0) = &quorum.shares[0];
    let (op1, sk1) = &quorum.shares[1];
    let (op2, sk2) = &quorum.shares[2];
    feed_and_drive(&mut state, &database, *op0, sk0.sign(SIGNING_ROOT)).await;
    feed_and_drive(&mut state, &database, *op1, sk1.sign(SIGNING_ROOT)).await;
    let last_outcome = feed_and_drive(&mut state, &database, *op2, sk2.sign(SIGNING_ROOT)).await;

    assert!(
        matches!(last_outcome, CollectorAction::Exit),
        "collector should `Break` when verification fails but no share is individually invalid"
    );
    drop(state);
    assert!(
        matches!(rx.try_recv(), Err(oneshot::error::TryRecvError::Closed)),
        "notifier sender should be dropped after the collector aborts"
    );
}

/// Multi-iteration fallback: a quorum with two bad shares triggers eviction
/// of both inside a single `try_reconstruct` call, and the cleaner remaining
/// shares immediately reconstruct successfully without leaving the loop.
///
/// We use a 5-share split (still 3-of-N) so that after evicting the two bad
/// shares we still have a clean quorum of 3 in the same call.
#[tokio::test]
async fn integration_while_loop_evicts_two_then_succeeds() {
    // 5-of-3 split: operators 1 and 2 sign the wrong root; 3, 4, 5 sign
    // correctly.
    let _metrics_guard = METRICS_TEST_LOCK.lock().await;
    const TOTAL_FIVE: u64 = 5;
    let rng = &mut StdRng::seed_from_u64(TEST_RNG_SEED);
    let master = SecretKey::random();
    let master_pubkey = master.public_key().compress();
    let shares: Vec<(OperatorId, SecretKey)> = split_with_rng(
        &master,
        THRESHOLD,
        (1..=TOTAL_FIVE).map(|x| KeyId::try_from(x).expect("test KeyId in range")),
        rng,
    )
    .expect("split should succeed")
    .into_iter()
    .map(|(kid, sk)| (OperatorId(u64::from(kid)), sk))
    .collect();
    let quorum = TestQuorum {
        master_pubkey,
        shares,
    };
    let (mut state, database) = seeded_state_and_db(&quorum);
    let mut rx = register_notifier(&mut state, THRESHOLD, master_pubkey);
    let failures_before = verification_failures_count();

    for (idx, (op_id, sk)) in quorum.shares.iter().enumerate() {
        let sig = if idx < 2 {
            sk.sign(BAD_SIGNING_ROOT)
        } else {
            sk.sign(SIGNING_ROOT)
        };
        feed_and_drive(&mut state, &database, *op_id, sig).await;
    }

    expect_signature(
        &mut rx,
        "Multi-eviction loop should reconstruct from the remaining valid shares",
    );
    assert_eq!(
        verification_failures_count(),
        failures_before + 1,
        "the single verification failure should be recorded once"
    );
    assert!(
        state.signature_share.is_empty(),
        "shares should be cleared after a successful reconstruction"
    );
}

/// Partial signatures that arrive after reconstruction are silently dropped:
/// no panic, no Break, no re-buffering of the late share. This is the common
/// case in production: slow operators' shares routinely arrive after a fast
/// majority has already reconstructed the signature.
#[test]
fn state_drops_partial_signatures_after_reconstruction() {
    let TestQuorum {
        master_pubkey,
        shares,
    } = split_random_master();
    let mut state = fresh_state();

    let mut rx = register_notifier(&mut state, THRESHOLD, master_pubkey);
    for (op_id, sk) in &shares[..THRESHOLD as usize] {
        feed_partial_sig(&mut state, *op_id, sk.sign(SIGNING_ROOT));
    }
    expect_signature(&mut rx, "first registrant should be notified");

    let (late_op, late_sk) = &shares[THRESHOLD as usize];
    feed_partial_sig(&mut state, *late_op, late_sk.sign(SIGNING_ROOT));

    assert!(state.full_signature.is_some(), "cached signature persists");
    assert!(
        state.signature_share.is_empty(),
        "post-reconstruction shares are not buffered"
    );
}
