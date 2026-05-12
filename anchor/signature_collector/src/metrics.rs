use std::sync::LazyLock;

pub use metrics::*;

pub static SIGNATURE_VERIFICATION_FAILURES_TOTAL: LazyLock<Result<IntCounter>> =
    LazyLock::new(|| {
        try_create_int_counter(
            "anchor_signature_collector_verification_failures_total",
            "Reconstructed signatures that failed verification against the validator master pubkey",
        )
    });
