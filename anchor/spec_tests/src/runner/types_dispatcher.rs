use std::{
    fs,
    path::{Path, PathBuf},
};

use crate::{run_test, types};

/// Outcome of dispatching a single fixture to its test type.
enum DispatchOutcome {
    Executed(Result<(), String>),
    SkippedKnown,
}

/// Dispatch a single fixture file to its test type based on the exact prefix before the first `_`.
fn dispatch_fixture_by_prefix(prefix: &str, path: &Path, contents: &str) -> DispatchOutcome {
    match prefix {
        // Encoding tests
        "aggregatorcommitteeconsensusdata.EncodingTest" => {
            DispatchOutcome::Executed(run_test::<
                types::AggregatorCommitteeConsensusDataEncodingTest,
            >(path, contents))
        }
        "beaconvote.EncodingTest" => {
            DispatchOutcome::Executed(run_test::<types::BeaconVoteEncodingTest>(path, contents))
        }
        "partialsigmessage.EncodingTest" => DispatchOutcome::Executed(run_test::<
            types::PartialSigMessageEncodingTest,
        >(path, contents)),
        "signedssvmsg.EncodingTest" => DispatchOutcome::Executed(run_test::<
            types::SignedSSVMessageEncodingTest,
        >(path, contents)),
        "ssvmsg.EncodingTest" => {
            DispatchOutcome::Executed(run_test::<types::SSVMessageEncodingTest>(path, contents))
        }
        "proposerconsensusdata.EncodingTest" => DispatchOutcome::Executed(run_test::<
            types::ProposerConsensusDataEncodingTest,
        >(path, contents)),

        // Anchor's `Share` is architecturally different from Go spec's `Share`
        // (different fields, decomposed across multiple types). Not applicable.
        "share.EncodingTest" => {
            eprintln!("SKIP (known-inapplicable): {prefix}");
            DispatchOutcome::SkippedKnown
        }

        // Validation tests
        "signedssvmsg.SignedSSVMessageTest" => {
            DispatchOutcome::Executed(run_test::<types::SignedSSVMessageTest>(path, contents))
        }
        "ssvmsg.SSVMessageTest" => {
            DispatchOutcome::Executed(run_test::<types::SSVMessageTest>(path, contents))
        }
        "partialsigmessage.MsgSpecTest" => {
            DispatchOutcome::Executed(run_test::<types::PartialSigMsgSpecTest>(path, contents))
        }

        // Aggregator committee consensus data validation tests
        "aggregatorcommitteeconsensusdata.AggregatorCommitteeConsensusDataTest" => {
            DispatchOutcome::Executed(run_test::<types::AggregatorCommitteeConsensusDataTest>(
                path, contents,
            ))
        }

        // Proposer consensus data validation tests
        "proposerconsensusdata.ProposerConsensusDataTest" => {
            DispatchOutcome::Executed(run_test::<types::ProposerConsensusDataTest>(path, contents))
        }

        // Proposer block data extraction tests
        "consensusdataproposer.ProposerSpecTest" => {
            DispatchOutcome::Executed(run_test::<types::ConsensusDataProposerTest>(path, contents))
        }

        // Encryption tests
        "encryption.EncryptionSpecTest" => {
            DispatchOutcome::Executed(run_test::<types::EncryptionSpecTest>(path, contents))
        }

        // Duty role mapping tests
        "duty.DutySpecTest" => {
            DispatchOutcome::Executed(run_test::<types::DutySpecTest>(path, contents))
        }

        // Deposit data signing root tests
        "beacon.DepositDataSpecTest" => {
            DispatchOutcome::Executed(run_test::<types::BeaconDepositDataSpecTest>(path, contents))
        }

        // SSZ merkleization tests
        "ssz.SSZSpecTest" => {
            DispatchOutcome::Executed(run_test::<types::SSZSpecTest>(path, contents))
        }

        // Committee member quorum tests
        "committeemember.CommitteeMemberTest" => {
            DispatchOutcome::Executed(run_test::<types::CommitteeMemberTest>(path, contents))
        }

        // Structure size tests
        "maxmsgsize.StructureSizeTest" => {
            DispatchOutcome::Executed(run_test::<types::StructureSizeTest>(path, contents))
        }

        _ => panic!("Unknown types test prefix: {prefix}"),
    }
}

/// Run all type spec tests from the fixture directory.
///
/// Iterates over every `.json` fixture, dispatches each to the appropriate test type,
/// and reports failures at the end.
pub fn run_all_type_fixtures() {
    let dir: PathBuf =
        Path::new(env!("CARGO_MANIFEST_DIR")).join("ssv-spec/types/spectest/generate/tests");
    assert!(
        dir.exists(),
        "Fixture directory not found: {}",
        dir.display()
    );

    let mut failures = Vec::new();
    let mut executed_count = 0;
    let mut skipped_known_count = 0;

    for entry in fs::read_dir(&dir).expect("Failed to read fixture directory") {
        let entry = entry.expect("Failed to read directory entry");
        let path = entry.path();

        if path.extension() != Some("json".as_ref()) {
            continue;
        }

        let filename = path.file_name().unwrap().to_string_lossy().to_string();
        let contents = fs::read_to_string(&path)
            .unwrap_or_else(|e| panic!("Failed to read {}: {e}", path.display()));

        // Extract exact type prefix
        let prefix = filename.split('_').next().unwrap_or("");

        match dispatch_fixture_by_prefix(prefix, &path, &contents) {
            DispatchOutcome::Executed(result) => {
                executed_count += 1;
                if let Err(e) = result {
                    failures.push(format!("  {filename}: {e}"));
                }
            }
            DispatchOutcome::SkippedKnown => skipped_known_count += 1,
        }
    }

    assert!(
        executed_count > 0,
        "No type spec test fixtures found in {}",
        dir.display()
    );

    eprintln!("{executed_count} executed, {skipped_known_count} skipped (known-inapplicable)");

    if !failures.is_empty() {
        panic!(
            "\n{} of {executed_count} type spec tests failed:\n{}",
            failures.len(),
            failures.join("\n")
        );
    }
}
